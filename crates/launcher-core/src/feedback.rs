//! Operation tracking, activity log and user notifications.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use launcher_shared::{
    ActivityEntry, ActivityEvent, Alert, AuthState, GameEvent, Level, OperationDto, OpsSnapshot,
    ProfilesSnapshot, Text, Toast, ToastAction, UpdateStatus,
};

pub const OPS_THROTTLE: Duration = Duration::from_millis(100);
pub const ACTIVITY_LIMIT: usize = 100;
pub const ACTIVITY_DEDUPE: Duration = Duration::from_secs(2);
/// Alerts whose report context is kept, the newest.
pub const REPORT_CONTEXTS: usize = 16;

/// What a report is about: something that went wrong, or a game that crashed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportKind {
    Error,
    Crash,
}

impl ReportKind {
    /// The report's `type`.
    pub fn as_str(self) -> &'static str {
        match self {
            ReportKind::Error => "error",
            ReportKind::Crash => "crash",
        }
    }
}

/// What a report of an alert needs besides the alert's own words: its English
/// title, where it happened, the details and the files to attach.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportContext {
    pub kind: ReportKind,
    pub title: String,
    pub screen: String,
    pub action: String,
    /// An object merged into the report's metadata.
    pub metadata: serde_json::Value,
    pub attachments: Vec<PathBuf>,
}

/// Receives UI-bound events. `launcher-app` implements it with Tauri `emit`.
pub trait EventSink: Send + Sync + 'static {
    fn ops(&self, snapshot: &OpsSnapshot);
    fn activity(&self, entry: &ActivityEntry);
    fn toast(&self, toast: &Toast);
    fn alert(&self, alert: &Alert);
    /// Launcher self-update state (`app://update`).
    fn update(&self, _status: &UpdateStatus) {}
    /// Profile list changed (`app://profiles`).
    fn profiles(&self, _snapshot: &ProfilesSnapshot) {}
    /// Microsoft sign-in progress (`app://auth`).
    fn auth(&self, _state: &AuthState) {}
    /// A started game changed state (`app://game`).
    fn game(&self, _event: &GameEvent) {}
    /// A module's own event, named by the module (`app://module/<id>/<name>`).
    fn module_event(&self, _event: &str, _payload: &serde_json::Value) {}
}

#[derive(Debug, Default)]
pub struct NullSink;

impl EventSink for NullSink {
    fn ops(&self, _: &OpsSnapshot) {}
    fn activity(&self, _: &ActivityEntry) {}
    fn toast(&self, _: &Toast) {}
    fn alert(&self, _: &Alert) {}
}

pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> Instant;
}

#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

#[derive(Debug, Clone)]
pub struct OperationSpec {
    pub title: Text,
    pub kind: String,
    pub status: Option<Text>,
    pub visible: bool,
}

impl OperationSpec {
    pub fn new(title: Text, kind: impl Into<String>) -> Self {
        Self { title, kind: kind.into(), status: None, visible: true }
    }

    pub fn hidden(mut self) -> Self {
        self.visible = false;
        self
    }

    pub fn status(mut self, status: Text) -> Self {
        self.status = Some(status);
        self
    }
}

struct ActivityRecord {
    entry: ActivityEntry,
    at: Instant,
}

#[derive(Default)]
struct State {
    next_op_id: u64,
    next_notice_id: u64,
    next_seq: u64,
    ops: BTreeMap<u64, OperationDto>,
    root: Option<u64>,
    activity: VecDeque<ActivityRecord>,
    last_ops_emit: Option<Instant>,
    ops_dirty: bool,
    /// Changes of `ops` so far: each snapshot carries it.
    ops_revision: u64,
    /// Report contexts of the latest reportable alerts, by alert id.
    reports: VecDeque<(u64, ReportContext)>,
}

pub struct FeedbackService {
    state: Mutex<State>,
    /// Serialises "take snapshot + deliver it" so the sink never receives an older snapshot
    /// after a newer one.
    emit_lock: Mutex<()>,
    sink: Arc<dyn EventSink>,
    clock: Arc<dyn Clock>,
}

impl FeedbackService {
    pub fn new(sink: Arc<dyn EventSink>) -> Arc<Self> {
        let service = Self::with_clock(sink, Arc::new(SystemClock));
        let weak = Arc::downgrade(&service);
        let _ = std::thread::Builder::new().name("feedback-flush".into()).spawn(move || {
            loop {
                std::thread::sleep(OPS_THROTTLE);
                match weak.upgrade() {
                    Some(service) => service.flush_pending(),
                    None => break,
                }
            }
        });
        service
    }

    pub fn with_clock(sink: Arc<dyn EventSink>, clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(Self { state: Mutex::new(State::default()), emit_lock: Mutex::new(()), sink, clock })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn begin(self: &Arc<Self>, spec: OperationSpec) -> OperationHandle {
        let (id, entry) = {
            let mut st = self.lock();
            st.next_op_id += 1;
            let id = st.next_op_id;
            let parent_id = st.root;
            if st.root.is_none() {
                st.root = Some(id);
            }
            st.ops.insert(
                id,
                OperationDto {
                    id,
                    parent_id,
                    title: spec.title.clone(),
                    kind: spec.kind.clone(),
                    status: spec.status.clone(),
                    progress: None,
                    total: None,
                    visible: spec.visible,
                },
            );
            st.ops_dirty = true;
            st.ops_revision += 1;
            let entry = self.record_activity(
                &mut st,
                ActivityEvent::Begin,
                Level::Info,
                spec.title,
                Some(id),
                Some(spec.kind),
            );
            (id, entry)
        };
        if let Some(e) = entry {
            self.sink.activity(&e);
        }
        self.emit_ops(true);
        OperationHandle { id, service: Arc::downgrade(self), finished: false }
    }

    fn update_op(
        &self,
        id: u64,
        status: Option<Text>,
        progress: Option<f64>,
        total: Option<f64>,
        reveal: bool,
    ) {
        let mut force = false;
        let entry = {
            let mut st = self.lock();
            let Some(op) = st.ops.get_mut(&id) else { return };
            let status_changed = status.is_some() && status != op.status;
            if status.is_some() {
                op.status = status;
            }
            if progress.is_some() {
                op.progress = progress;
            }
            if total.is_some() {
                op.total = total;
            }
            if reveal && !op.visible {
                op.visible = true;
                force = true;
            }
            let kind = op.kind.clone();
            let current_status = op.status.clone();
            st.ops_dirty = true;
            st.ops_revision += 1;
            match (status_changed, current_status) {
                (true, Some(text)) => self.record_activity(
                    &mut st,
                    ActivityEvent::Update,
                    Level::Info,
                    text,
                    Some(id),
                    Some(kind),
                ),
                _ => None,
            }
        };
        if let Some(e) = entry {
            self.sink.activity(&e);
        }
        self.emit_ops(force);
    }

    fn finish_op(&self, id: u64, failure: Option<Text>) {
        let entry = {
            let mut st = self.lock();
            let Some(op) = st.ops.remove(&id) else { return };
            if st.root == Some(id) {
                st.root = st.ops.keys().next().copied();
            }
            let root = st.root;
            for other in st.ops.values_mut() {
                if Some(other.id) == root {
                    other.parent_id = None;
                } else if other.parent_id == Some(id) {
                    other.parent_id = root;
                }
            }
            st.ops_dirty = true;
            st.ops_revision += 1;
            let (level, message) = match failure {
                Some(text) => (Level::Error, text),
                None => (Level::Success, op.title.clone()),
            };
            self.record_activity(&mut st, ActivityEvent::Finish, level, message, Some(id), Some(op.kind))
        };
        if let Some(e) = entry {
            self.sink.activity(&e);
        }
        self.emit_ops(true);
    }

    fn emit_ops(&self, force: bool) {
        let _emit = self.emit_lock.lock().unwrap_or_else(|e| e.into_inner());
        let snapshot = {
            let mut st = self.lock();
            let now = self.clock.now();
            let due = force || st.last_ops_emit.is_none_or(|t| now.duration_since(t) >= OPS_THROTTLE);
            if !due {
                return;
            }
            st.last_ops_emit = Some(now);
            st.ops_dirty = false;
            Self::snapshot_of(&st)
        };
        self.sink.ops(&snapshot);
    }

    /// Emits the latest state if updates were throttled.
    pub fn flush_pending(&self) {
        let dirty = self.lock().ops_dirty;
        if dirty {
            self.emit_ops(true);
        }
    }

    fn snapshot_of(st: &State) -> OpsSnapshot {
        OpsSnapshot {
            busy: !st.ops.is_empty(),
            operations: st.ops.values().cloned().collect(),
            revision: st.ops_revision,
        }
    }

    pub fn snapshot(&self) -> OpsSnapshot {
        Self::snapshot_of(&self.lock())
    }

    pub fn is_busy(&self) -> bool {
        !self.lock().ops.is_empty()
    }

    /// Newest first.
    pub fn activity(&self, limit: usize) -> Vec<ActivityEntry> {
        self.lock().activity.iter().rev().take(limit).map(|r| r.entry.clone()).collect()
    }

    fn record_activity(
        &self,
        st: &mut State,
        event: ActivityEvent,
        level: Level,
        message: Text,
        operation_id: Option<u64>,
        kind: Option<String>,
    ) -> Option<ActivityEntry> {
        let now = self.clock.now();
        if let Some(last) = st.activity.back_mut()
            && last.entry.level == level
            && last.entry.message == message
            && now.duration_since(last.at) < ACTIVITY_DEDUPE
        {
            if last.entry.kind.is_none() && kind.is_some() {
                last.entry.kind = kind;
                last.entry.operation_id = operation_id;
                last.entry.event = event;
            }
            return None;
        }
        st.next_seq += 1;
        let at_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        let entry = ActivityEntry { seq: st.next_seq, at_ms, event, level, message, operation_id, kind };
        st.activity.push_back(ActivityRecord { entry: entry.clone(), at: now });
        while st.activity.len() > ACTIVITY_LIMIT {
            st.activity.pop_front();
        }
        Some(entry)
    }

    pub fn toast(
        &self,
        level: Level,
        title: Text,
        message: Option<Text>,
        action: Option<ToastAction>,
    ) -> u64 {
        let (id, entry) = {
            let mut st = self.lock();
            st.next_notice_id += 1;
            let id = st.next_notice_id;
            let entry =
                self.record_activity(&mut st, ActivityEvent::Notify, level, title.clone(), None, None);
            (id, entry)
        };
        if let Some(e) = entry {
            self.sink.activity(&e);
        }
        self.sink.toast(&Toast { id, level, title, message, action });
        id
    }

    pub fn info(&self, title: Text) -> u64 {
        self.toast(Level::Info, title, None, None)
    }

    pub fn success(&self, title: Text) -> u64 {
        self.toast(Level::Success, title, None, None)
    }

    pub fn alert(&self, level: Level, title: Text, message: Text, allow_report: bool) -> u64 {
        self.raise(level, title, message, allow_report, None)
    }

    /// An alert the user can report: `report` is kept for [`Self::report_context`].
    pub fn alert_with_report(&self, level: Level, title: Text, message: Text, report: ReportContext) -> u64 {
        self.raise(level, title, message, true, Some(report))
    }

    /// What a report of alert `id` needs, while it is among the latest reportable ones.
    pub fn report_context(&self, id: u64) -> Option<ReportContext> {
        self.lock().reports.iter().find(|(alert, _)| *alert == id).map(|(_, report)| report.clone())
    }

    fn raise(
        &self,
        level: Level,
        title: Text,
        message: Text,
        allow_report: bool,
        report: Option<ReportContext>,
    ) -> u64 {
        let (id, entry) = {
            let mut st = self.lock();
            st.next_notice_id += 1;
            let id = st.next_notice_id;
            if let Some(report) = report {
                st.reports.push_back((id, report));
                while st.reports.len() > REPORT_CONTEXTS {
                    st.reports.pop_front();
                }
            }
            let entry =
                self.record_activity(&mut st, ActivityEvent::Notify, level, message.clone(), None, None);
            (id, entry)
        };
        if let Some(e) = entry {
            self.sink.activity(&e);
        }
        self.sink.alert(&Alert { id, title, message, allow_report });
        id
    }

    pub fn warning(&self, message: Text) -> u64 {
        self.alert(Level::Warning, Text::key("warning"), message, false)
    }

    /// Only the activity log hears of it: something else already shows it.
    pub fn note(&self, level: Level, message: Text) {
        let entry = {
            let mut st = self.lock();
            self.record_activity(&mut st, ActivityEvent::Notify, level, message, None, None)
        };
        if let Some(e) = entry {
            self.sink.activity(&e);
        }
    }

    pub fn error(&self, message: Text) -> u64 {
        self.alert(Level::Error, Text::key("warning"), message, false)
    }
}

pub struct OperationHandle {
    id: u64,
    service: Weak<FeedbackService>,
    finished: bool,
}

impl OperationHandle {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn update(&self, status: Option<Text>, progress: Option<f64>, total: Option<f64>) {
        if let Some(service) = self.service.upgrade() {
            service.update_op(self.id, status, progress, total, false);
        }
    }

    pub fn status(&self, status: Text) {
        self.update(Some(status), None, None);
    }

    pub fn progress(&self, done: f64, total: f64) {
        self.update(None, Some(done), Some(total));
    }

    /// Reports this operation's progress from another thread (a blocking task); it never ends the
    /// operation.
    pub fn progress_reporter(&self) -> impl Fn(f64, f64) + Send + Sync + 'static {
        let (id, service) = (self.id, self.service.clone());
        move |done, total| {
            if let Some(service) = service.upgrade() {
                service.update_op(id, None, Some(done), Some(total), false);
            }
        }
    }

    pub fn reveal(&self) {
        if let Some(service) = self.service.upgrade() {
            service.update_op(self.id, None, None, None, true);
        }
    }

    pub fn finish(mut self) {
        self.finished = true;
        if let Some(service) = self.service.upgrade() {
            service.finish_op(self.id, None);
        }
    }

    pub fn fail(mut self, message: Text) {
        self.finished = true;
        if let Some(service) = self.service.upgrade() {
            service.finish_op(self.id, Some(message));
        }
    }
}

impl Drop for OperationHandle {
    fn drop(&mut self) {
        if !self.finished
            && let Some(service) = self.service.upgrade()
        {
            service.finish_op(self.id, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Rec {
        ops: Mutex<Vec<OpsSnapshot>>,
        activity: Mutex<Vec<ActivityEntry>>,
        toasts: Mutex<Vec<Toast>>,
        alerts: Mutex<Vec<Alert>>,
    }
    impl EventSink for Rec {
        fn ops(&self, s: &OpsSnapshot) {
            self.ops.lock().unwrap().push(s.clone());
        }
        fn activity(&self, e: &ActivityEntry) {
            self.activity.lock().unwrap().push(e.clone());
        }
        fn toast(&self, t: &Toast) {
            self.toasts.lock().unwrap().push(t.clone());
        }
        fn alert(&self, a: &Alert) {
            self.alerts.lock().unwrap().push(a.clone());
        }
    }

    struct FakeClock(Mutex<Instant>);
    impl FakeClock {
        fn advance(&self, d: Duration) {
            *self.0.lock().unwrap() += d;
        }
    }
    impl Clock for FakeClock {
        fn now(&self) -> Instant {
            *self.0.lock().unwrap()
        }
    }

    fn setup() -> (Arc<FeedbackService>, Arc<Rec>, Arc<FakeClock>) {
        let rec = Arc::new(Rec::default());
        let clock = Arc::new(FakeClock(Mutex::new(Instant::now())));
        (FeedbackService::with_clock(rec.clone(), clock.clone()), rec, clock)
    }

    fn spec(key: &str) -> OperationSpec {
        OperationSpec::new(Text::key(key), "install")
    }

    #[test]
    fn each_change_of_the_operations_numbers_the_snapshot_after_the_last() {
        // The window asks for the operations at start while their events already flow: whichever
        // arrives last, the later state is kept, by its number.
        let (fb, rec, _c) = setup();
        let before = fb.snapshot();
        let op = fb.begin(spec("a"));
        let begun = fb.snapshot();
        op.status(Text::key("b"));
        let updated = fb.snapshot();
        op.finish();
        let ended = fb.snapshot();
        assert!(before.revision < begun.revision && begun.revision < updated.revision);
        assert!(updated.revision < ended.revision);
        assert!(ended.replaces(&begun) && !begun.replaces(&ended) && ended.replaces(&ended));
        let sent = rec.ops.lock().unwrap().last().cloned().unwrap();
        assert_eq!(sent, ended, "what is sent carries its number too");
    }

    #[test]
    fn first_operation_is_root_and_later_are_children() {
        let (fb, _rec, _c) = setup();
        let root = fb.begin(spec("a"));
        let child = fb.begin(spec("b"));
        let snap = fb.snapshot();
        assert!(snap.busy);
        assert_eq!(snap.operations[0].parent_id, None);
        assert_eq!(snap.operations[1].parent_id, Some(root.id()));
        child.finish();
        root.finish();
        assert!(!fb.is_busy());
        assert!(fb.snapshot().operations.is_empty());
    }

    #[test]
    fn child_is_promoted_when_root_finishes_first() {
        let (fb, _rec, _c) = setup();
        let root = fb.begin(spec("a"));
        let child = fb.begin(spec("b"));
        let child_id = child.id();
        root.finish();
        let snap = fb.snapshot();
        assert!(snap.busy);
        assert_eq!(snap.operations.len(), 1);
        assert_eq!((snap.operations[0].id, snap.operations[0].parent_id), (child_id, None));
        let next = fb.begin(spec("c"));
        assert_eq!(fb.snapshot().operations[1].parent_id, Some(child_id));
        drop(next);
        drop(child);
    }

    #[test]
    fn hidden_operation_is_invisible_until_revealed() {
        let (fb, _rec, _c) = setup();
        let op = fb.begin(spec("sync").hidden());
        assert!(!fb.snapshot().operations[0].visible);
        op.reveal();
        assert!(fb.snapshot().operations[0].visible);
    }

    #[test]
    fn throttles_progress_events() {
        let (fb, rec, clock) = setup();
        let op = fb.begin(spec("download"));
        assert_eq!(rec.ops.lock().unwrap().len(), 1);
        for i in 0..1000 {
            op.progress(i as f64, 1000.0);
        }
        assert_eq!(rec.ops.lock().unwrap().len(), 1, "no emits inside the 100 ms window");
        fb.flush_pending();
        let last = rec.ops.lock().unwrap().last().unwrap().clone();
        assert_eq!(last.operations[0].progress, Some(999.0));
        clock.advance(OPS_THROTTLE);
        op.progress(1000.0, 1000.0);
        assert_eq!(rec.ops.lock().unwrap().len(), 3);
    }

    #[test]
    fn a_progress_reporter_moves_the_operation_from_another_thread() {
        let (fb, rec, _c) = setup();
        let op = fb.begin(spec("extract"));
        let report = op.progress_reporter();
        std::thread::spawn(move || report(5.0, 10.0)).join().unwrap();
        fb.flush_pending();
        assert_eq!(rec.ops.lock().unwrap().last().unwrap().operations[0].progress, Some(5.0));
        assert!(fb.is_busy(), "the reporter does not end the operation");
    }

    #[test]
    fn dropping_handle_finishes_operation() {
        let (fb, _rec, _c) = setup();
        {
            let _op = fb.begin(spec("a"));
            assert!(fb.is_busy());
        }
        assert!(!fb.is_busy());
    }

    #[test]
    fn info_goes_to_toast_and_warning_to_alert() {
        let (fb, rec, _c) = setup();
        let a = fb.info(Text::key("settings_saved"));
        let b = fb.success(Text::key("installation_complete"));
        let c = fb.warning(Text::key("version_not_found"));
        assert!(a < b && b < c);
        assert_eq!(rec.toasts.lock().unwrap().len(), 2);
        let alerts = rec.alerts.lock().unwrap();
        assert_eq!(alerts[0].message, Text::key("version_not_found"));
        assert_eq!(alerts[0].title, Text::key("warning"));
    }

    #[test]
    fn activity_is_deduplicated_and_capped() {
        let (fb, _rec, clock) = setup();
        fb.info(Text::key("same"));
        fb.info(Text::key("same"));
        assert_eq!(fb.activity(10).len(), 1);
        clock.advance(ACTIVITY_DEDUPE);
        fb.info(Text::key("same"));
        assert_eq!(fb.activity(10).len(), 2);
        for i in 0..150 {
            fb.info(Text::raw(format!("m{i}")));
        }
        let all = fb.activity(1000);
        assert_eq!(all.len(), ACTIVITY_LIMIT);
        assert_eq!(all[0].message, Text::raw("m149"));
    }

    /// Sink whose second `ops` delivery is slow, like a busy webview.
    #[derive(Default)]
    struct SlowSecondOps {
        calls: Mutex<u32>,
        last: Mutex<Option<OpsSnapshot>>,
    }
    impl EventSink for SlowSecondOps {
        fn ops(&self, s: &OpsSnapshot) {
            let n = {
                let mut calls = self.calls.lock().unwrap();
                *calls += 1;
                *calls
            };
            if n == 2 {
                std::thread::sleep(Duration::from_millis(300));
            }
            *self.last.lock().unwrap() = Some(s.clone());
        }
        fn activity(&self, _: &ActivityEntry) {}
        fn toast(&self, _: &Toast) {}
        fn alert(&self, _: &Alert) {}
    }

    #[test]
    fn ops_snapshots_reach_the_sink_in_order() {
        let sink = Arc::new(SlowSecondOps::default());
        let clock = Arc::new(FakeClock(Mutex::new(Instant::now())));
        let fb = FeedbackService::with_clock(sink.clone(), clock);
        let a = fb.begin(spec("a"));
        let worker = {
            let fb = fb.clone();
            std::thread::spawn(move || fb.begin(spec("b")))
        };
        std::thread::sleep(Duration::from_millis(100));
        let c = fb.begin(spec("c"));
        let b = worker.join().unwrap();
        let last = sink.last.lock().unwrap().clone().unwrap();
        assert_eq!(last.operations.len(), 3, "UI was left with a stale snapshot");
        drop((a, b, c));
    }

    fn context(action: &str) -> ReportContext {
        ReportContext {
            kind: ReportKind::Error,
            title: "Launcher report".into(),
            screen: "Home".into(),
            action: action.into(),
            metadata: serde_json::json!({"pack_id": "aero"}),
            attachments: Vec::new(),
        }
    }

    #[test]
    fn an_alert_with_a_report_keeps_its_context() {
        let (fb, rec, _) = setup();
        let id = fb.alert_with_report(Level::Error, Text::key("warning"), Text::raw("x"), context("install"));
        let alert = rec.alerts.lock().unwrap()[0].clone();
        assert_eq!((alert.id, alert.allow_report), (id, true));
        assert_eq!(fb.report_context(id), Some(context("install")));
        let plain = fb.warning(Text::raw("y"));
        assert!(!rec.alerts.lock().unwrap()[1].allow_report);
        assert_eq!(fb.report_context(plain), None, "a plain alert has nothing to report");
    }

    #[test]
    fn old_report_contexts_are_forgotten() {
        let (fb, _, _) = setup();
        let ids: Vec<u64> = (0..=REPORT_CONTEXTS)
            .map(|n| {
                fb.alert_with_report(
                    Level::Error,
                    Text::key("warning"),
                    Text::raw("x"),
                    context(&n.to_string()),
                )
            })
            .collect();
        assert_eq!(fb.report_context(ids[0]), None, "the oldest goes");
        assert_eq!(fb.report_context(ids[1]).map(|c| c.action), Some("1".to_string()));
        assert!(fb.report_context(ids[REPORT_CONTEXTS]).is_some());
    }
}

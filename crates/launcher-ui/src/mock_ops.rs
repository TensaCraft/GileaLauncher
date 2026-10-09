//! Operations in the browser preview: one runs for a few seconds with its progress ticking, as the
//! launcher's do (a server build's full sync), so the operations ring and panel can be seen.

use std::cell::RefCell;
use std::time::Duration;

use launcher_shared::{OperationDto, OpsSnapshot, Text, names};
use leptos::prelude::set_timeout;
use ui_kit::ipc;

/// How often a running operation moves.
const TICK: Duration = Duration::from_millis(250);

#[derive(Default)]
struct Ops {
    next_id: u64,
    revision: u64,
    running: Vec<OperationDto>,
}

thread_local! {
    static OPS: RefCell<Ops> = RefCell::new(Ops::default());
}

/// The operations running now, as the launcher sends them.
pub fn snapshot() -> OpsSnapshot {
    OPS.with(|ops| {
        let ops = ops.borrow();
        OpsSnapshot { busy: !ops.running.is_empty(), operations: ops.running.clone(), revision: ops.revision }
    })
}

fn emit() {
    if let Ok(value) = serde_json::to_value(snapshot()) {
        ipc::emit_mock(names::OPS, value);
    }
}

/// Runs an operation titled `title` (with `status`) for about `seconds`; `done` when it ends.
pub fn run(title: Text, kind: &str, status: Text, seconds: u32, done: impl FnOnce() + 'static) {
    let id = OPS.with(|ops| {
        let mut ops = ops.borrow_mut();
        ops.next_id += 1;
        ops.revision += 1;
        let id = ops.next_id;
        ops.running.push(OperationDto {
            id,
            parent_id: None,
            title,
            kind: kind.to_string(),
            status: Some(status),
            progress: Some(0.0),
            total: Some(100.0),
            visible: true,
        });
        id
    });
    emit();
    let ticks = (seconds.max(1) * 1000) as f64 / TICK.as_millis() as f64;
    tick(id, 100.0 / ticks, Box::new(done));
}

fn tick(id: u64, step: f64, done: Box<dyn FnOnce()>) {
    set_timeout(
        move || {
            let finished = OPS.with(|ops| {
                let mut ops = ops.borrow_mut();
                ops.revision += 1;
                let Some(op) = ops.running.iter_mut().find(|op| op.id == id) else { return true };
                let progress = (op.progress.unwrap_or(0.0) + step).min(100.0);
                op.progress = Some(progress);
                if progress >= 100.0 {
                    ops.running.retain(|op| op.id != id);
                    return true;
                }
                false
            });
            emit();
            if finished {
                done();
            } else {
                tick(id, step, done);
            }
        },
        TICK,
    );
}

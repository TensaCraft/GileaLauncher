//! Home's «Продовжити гру»: the builds played last with their last server or world, and the
//! servers' answers (asked once per Home visit, kept while the launcher runs).

use std::collections::BTreeMap;

use launcher_shared::recent::{Activity, MotdSpan, RecentBuild, ServerStatus};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::ipc;

/// The game's default server port: an address on it is shown without one.
const DEFAULT_PORT: u16 = 25565;

/// What a server said when it was last asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ping {
    Asking,
    Answered(ServerStatus),
    Silent,
}

/// When a build was played, as Home says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ago {
    JustNow,
    Minutes(u64),
    Hours(u64),
    Yesterday,
    Days(u64),
    /// Over a week: the date is shown.
    Long,
}

pub fn ago(now_ms: u64, then_ms: u64) -> Ago {
    let minutes = now_ms.saturating_sub(then_ms) / 60_000;
    match minutes {
        0 => Ago::JustNow,
        1..60 => Ago::Minutes(minutes),
        60..1440 => Ago::Hours(minutes / 60),
        1440..2880 => Ago::Yesterday,
        _ if minutes < 7 * 1440 => Ago::Days(minutes / 1440),
        _ => Ago::Long,
    }
}

/// The bars of the game's server list for a round trip of `ms`.
pub fn ping_bars(ms: u32) -> u8 {
    match ms {
        0..150 => 5,
        150..300 => 4,
        300..600 => 3,
        600..1000 => 2,
        _ => 1,
    }
}

/// `host`, with `:port` when it is not the default one.
pub fn address_label(host: &str, port: u16) -> String {
    if port == DEFAULT_PORT { host.to_string() } else { format!("{host}:{port}") }
}

/// A line of a MOTD as the server list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MotdLine {
    /// The server padded it with spaces to stand in the middle (the game shows it there).
    pub centered: bool,
    pub spans: Vec<MotdSpan>,
}

/// How many lines of a MOTD the server list shows.
const MOTD_LINES: usize = 2;

/// Drops the spaces at the start (`front`) or the end of `spans`, and spans left empty.
fn trim_spans(spans: &mut Vec<MotdSpan>, front: bool) {
    loop {
        let at = if front { 0 } else { spans.len().saturating_sub(1) };
        let Some(span) = spans.get_mut(at) else { return };
        span.text = if front { span.text.trim_start().to_string() } else { span.text.trim_end().to_string() };
        if !span.text.is_empty() {
            return;
        }
        spans.remove(at);
    }
}

/// The lines of a MOTD (its spans split at line breaks), at most two and none blank at the end;
/// a line the server starts with two spaces or more is one it centred.
pub fn motd_lines(spans: &[MotdSpan]) -> Vec<MotdLine> {
    let mut lines: Vec<Vec<MotdSpan>> = vec![Vec::new()];
    for span in spans {
        for (i, piece) in span.text.split('\n').enumerate() {
            if i > 0 {
                lines.push(Vec::new());
            }
            if !piece.is_empty()
                && let Some(line) = lines.last_mut()
            {
                line.push(MotdSpan { text: piece.to_string(), ..span.clone() });
            }
        }
    }
    let mut shown: Vec<MotdLine> = lines
        .into_iter()
        .take(MOTD_LINES)
        .map(|mut spans| {
            let leading = spans.iter().flat_map(|s| s.text.chars()).take_while(|c| *c == ' ').count();
            trim_spans(&mut spans, true);
            trim_spans(&mut spans, false);
            MotdLine { centered: leading >= 2, spans }
        })
        .collect();
    while shown.last().is_some_and(|l| l.spans.is_empty()) {
        shown.pop();
    }
    shown
}

/// The key of a server in `RecentState::pings`.
pub fn server_key(host: &str, port: u16) -> String {
    format!("{}:{port}", host.to_ascii_lowercase())
}

/// The i18n keys of a world's mode and difficulty.
pub fn mode_key(mode: &str) -> &'static str {
    match mode {
        "creative" => "world_mode_creative",
        "adventure" => "world_mode_adventure",
        "spectator" => "world_mode_spectator",
        _ => "world_mode_survival",
    }
}

pub fn difficulty_key(difficulty: &str) -> Option<&'static str> {
    match difficulty {
        "peaceful" => Some("world_difficulty_peaceful"),
        "easy" => Some("world_difficulty_easy"),
        "normal" => Some("world_difficulty_normal"),
        "hard" => Some("world_difficulty_hard"),
        _ => None,
    }
}

#[derive(Clone, Copy)]
pub struct RecentState {
    /// `None` until the first answer.
    pub builds: RwSignal<Option<Vec<RecentBuild>>>,
    pub pings: RwSignal<BTreeMap<String, Ping>>,
    /// Bumped by each refresh: an older answer that arrives later is dropped.
    asked: StoredValue<u64>,
}

#[derive(serde::Serialize)]
struct ServerArgs {
    host: String,
    port: u16,
}

impl RecentState {
    /// Asks for the builds played last again, then their servers: every one on a new visit
    /// (`ask_all`), else those not asked yet.
    pub fn refresh(&self, ask_all: bool) {
        let this = *self;
        let turn = self.asked.get_value() + 1;
        self.asked.set_value(turn);
        spawn_local(async move {
            let Ok(found) = ipc::call::<Vec<RecentBuild>>("recent_builds").await else { return };
            if this.asked.get_value() != turn {
                return;
            }
            for recent in &found {
                if let Some(Activity::Server { host, port }) = &recent.activity
                    && (ask_all || this.pings.with_untracked(|p| !p.contains_key(&server_key(host, *port))))
                {
                    this.ask(host.clone(), *port);
                }
            }
            this.builds.set(Some(found));
        });
    }

    /// Asks `host:port` for its status, unless it is being asked.
    fn ask(&self, host: String, port: u16) {
        let key = server_key(&host, port);
        if self.pings.with_untracked(|p| p.get(&key) == Some(&Ping::Asking)) {
            return;
        }
        self.pings.update(|p| {
            p.insert(key.clone(), Ping::Asking);
        });
        let pings = self.pings;
        spawn_local(async move {
            let answer =
                ipc::invoke::<_, Option<ServerStatus>>("server_status", &ServerArgs { host, port }).await;
            let ping = match answer {
                Ok(Some(status)) => Ping::Answered(status),
                _ => Ping::Silent,
            };
            pings.update(|p| {
                p.insert(key, ping);
            });
        });
    }
}

pub fn provide_recent() -> RecentState {
    let state = RecentState {
        builds: RwSignal::new(None),
        pings: RwSignal::new(BTreeMap::new()),
        asked: StoredValue::new(0),
    };
    provide_context(state);
    state
}

pub fn use_recent() -> RecentState {
    expect_context::<RecentState>()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60_000;

    #[test]
    fn when_a_build_was_played_reads_as_a_person_says_it() {
        let now = 1_800_000_000_000;
        assert_eq!(ago(now, now - 20_000), Ago::JustNow);
        assert_eq!(ago(now, now + 5 * MIN), Ago::JustNow, "a clock ahead is not the future");
        assert_eq!(ago(now, now - 40 * MIN), Ago::Minutes(40));
        assert_eq!(ago(now, now - 59 * MIN), Ago::Minutes(59));
        assert_eq!(ago(now, now - 60 * MIN), Ago::Hours(1));
        assert_eq!(ago(now, now - 23 * 60 * MIN - 59 * MIN), Ago::Hours(23));
        assert_eq!(ago(now, now - 30 * 60 * MIN), Ago::Yesterday);
        assert_eq!(ago(now, now - 3 * 1440 * MIN), Ago::Days(3));
        assert_eq!(ago(now, now - 6 * 1440 * MIN - MIN), Ago::Days(6));
        assert_eq!(ago(now, now - 7 * 1440 * MIN), Ago::Long);
    }

    #[test]
    fn the_ping_shows_the_game_s_bars() {
        assert_eq!(
            [0, 149, 150, 299, 300, 599, 600, 999, 1000, 5000].map(ping_bars),
            [5, 5, 4, 4, 3, 3, 2, 2, 1, 1]
        );
    }

    #[test]
    fn an_address_names_its_port_only_when_it_is_not_the_default() {
        assert_eq!(address_label("play.example.net", 25565), "play.example.net");
        assert_eq!(address_label("play.example.net", 25570), "play.example.net:25570");
        assert_eq!(server_key("Play.Example.NET", 25565), server_key("play.example.net", 25565));
    }

    fn text(line: &MotdLine) -> String {
        line.spans.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn a_motd_line_the_server_pads_is_centred_as_in_the_game() {
        let spans = launcher_shared::recent::parse_motd(&serde_json::json!(
            "        §b✦ Aeronautics ✦     \n§fMinecraft 1.21.1 | NeoForge | play.example.net"
        ));
        let lines = motd_lines(&spans);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].centered && !lines[1].centered);
        assert_eq!(text(&lines[0]), "✦ Aeronautics ✦", "the padding goes, the words stay");
        assert_eq!(text(&lines[1]), "Minecraft 1.21.1 | NeoForge | play.example.net");
        assert_eq!(lines[0].spans[0].color.as_deref(), Some("#55ffff"));
    }

    #[test]
    fn a_motd_shows_two_lines_at_most_with_none_empty_at_the_end() {
        let spans = launcher_shared::recent::parse_motd(&serde_json::json!("a\nb\nc"));
        assert_eq!(motd_lines(&spans).iter().map(text).collect::<Vec<_>>(), ["a", "b"]);
        let one = launcher_shared::recent::parse_motd(&serde_json::json!(" one space\n   "));
        let lines = motd_lines(&one);
        assert_eq!(lines.len(), 1, "a blank second line is left out");
        assert!(!lines[0].centered, "one space is no centring");
        assert!(motd_lines(&[]).is_empty());
    }

    #[test]
    fn a_world_s_mode_and_difficulty_have_their_words() {
        assert_eq!(mode_key("creative"), "world_mode_creative");
        assert_eq!(mode_key("anything"), "world_mode_survival");
        assert_eq!(difficulty_key("hard"), Some("world_difficulty_hard"));
        assert_eq!(difficulty_key(""), None);
    }
}

//! Home's «Продовжити гру»: the builds played last with their last server or world, and the
//! servers' answers (asked once per Home visit, kept while the launcher runs).

use std::collections::BTreeMap;

use launcher_shared::recent::{Activity, RecentBuild, ServerStatus};
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
        assert_eq!(address_label("tensa.co.ua", 25565), "tensa.co.ua");
        assert_eq!(address_label("play.example.net", 25570), "play.example.net:25570");
        assert_eq!(server_key("Tensa.co.UA", 25565), server_key("tensa.co.ua", 25565));
    }

    #[test]
    fn a_world_s_mode_and_difficulty_have_their_words() {
        assert_eq!(mode_key("creative"), "world_mode_creative");
        assert_eq!(mode_key("anything"), "world_mode_survival");
        assert_eq!(difficulty_key("hard"), Some("world_difficulty_hard"));
        assert_eq!(difficulty_key(""), None);
    }
}

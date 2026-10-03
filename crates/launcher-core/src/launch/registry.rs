//! Which game folders have Minecraft running, and the pause between launches of one folder.
//! Keys are game folders as `lock::path_key` spells them.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use super::process::SharedProcess;

pub const LAUNCH_COOLDOWN: Duration = Duration::from_secs(3);

#[derive(Default)]
struct State {
    recent: HashMap<String, Instant>,
    active: HashMap<String, Vec<SharedProcess>>,
    /// Processes the launcher stopped: their exit is not a crash.
    stopped: HashSet<u32>,
}

pub struct LaunchRegistry {
    cooldown: Duration,
    state: Mutex<State>,
}

impl LaunchRegistry {
    pub fn new(cooldown: Duration) -> LaunchRegistry {
        LaunchRegistry { cooldown, state: Mutex::new(State::default()) }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Takes the launch slot of `key`, or tells how long until it frees up.
    pub fn try_reserve(&self, key: &str, now: Instant) -> Result<(), Duration> {
        let mut state = self.state();
        let cooldown = self.cooldown;
        state.recent.retain(|k, at| k == key || now.saturating_duration_since(*at) < cooldown);
        if let Some(at) = state.recent.get(key) {
            let elapsed = now.saturating_duration_since(*at);
            if elapsed < cooldown {
                return Err(cooldown - elapsed);
            }
        }
        state.recent.insert(key.to_string(), now);
        Ok(())
    }

    /// A launch that did not start gives its slot back, so a retry is not throttled.
    pub fn release(&self, key: &str) {
        self.state().recent.remove(key);
    }

    pub fn register(&self, key: &str, process: SharedProcess) {
        self.state().active.entry(key.to_string()).or_default().push(process);
    }

    pub fn unregister(&self, key: &str, pid: u32) {
        let mut state = self.state();
        if let Some(list) = state.active.get_mut(key) {
            list.retain(|p| p.lock().unwrap_or_else(|e| e.into_inner()).pid() != pid);
            if list.is_empty() {
                state.active.remove(key);
            }
        }
    }

    /// Process `pid` is one of the registered games.
    pub fn contains(&self, pid: u32) -> bool {
        self.state()
            .active
            .values()
            .flatten()
            .any(|p| p.lock().unwrap_or_else(|e| e.into_inner()).pid() == pid)
    }

    /// Some game the launcher started (or took back) still runs.
    pub fn any_active(&self) -> bool {
        let keys: Vec<String> = self.state().active.keys().cloned().collect();
        keys.iter().any(|key| self.is_active(key))
    }

    /// A game of `key` is still running (exited ones are dropped).
    pub fn is_active(&self, key: &str) -> bool {
        let mut state = self.state();
        let Some(list) = state.active.get_mut(key) else { return false };
        list.retain(|p| !matches!(p.lock().unwrap_or_else(|e| e.into_inner()).try_wait(), Ok(Some(_))));
        let alive = !list.is_empty();
        if !alive {
            state.active.remove(key);
        }
        alive
    }

    /// Stops every game of `key`; returns how many were asked to stop.
    pub fn terminate(&self, key: &str) -> usize {
        let mut state = self.state();
        let processes = state.active.get(key).cloned().unwrap_or_default();
        let mut stopped = 0;
        for process in processes {
            let mut process = process.lock().unwrap_or_else(|e| e.into_inner());
            match process.kill() {
                Ok(()) => {
                    state.stopped.insert(process.pid());
                    stopped += 1;
                }
                Err(e) => tracing::warn!("Unable to stop Minecraft (pid {}): {e}", process.pid()),
            }
        }
        stopped
    }

    /// Whether the launcher stopped `pid` (answers `true` once).
    pub fn take_stopped(&self, pid: u32) -> bool {
        self.state().stopped.remove(&pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::process::fake::FakeProcess;

    #[test]
    fn the_cooldown_applies_per_folder() {
        let registry = LaunchRegistry::new(Duration::from_secs(3));
        let start = Instant::now();
        registry.try_reserve("a", start).unwrap();
        let remaining = registry.try_reserve("a", start + Duration::from_millis(500)).unwrap_err();
        assert_eq!(remaining, Duration::from_millis(2500));
        registry.try_reserve("b", start + Duration::from_millis(500)).unwrap();
        registry.try_reserve("a", start + Duration::from_secs(3)).unwrap();
        registry.release("b");
        registry.try_reserve("b", start + Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn exited_games_are_not_active() {
        let registry = LaunchRegistry::new(LAUNCH_COOLDOWN);
        assert!(!registry.is_active("a"));
        registry.register("a", FakeProcess::running(1).shared());
        registry.register("a", FakeProcess::exiting_after(2, Duration::ZERO, Some(0)).shared());
        assert!(registry.is_active("a"));
        registry.unregister("a", 1);
        assert!(!registry.is_active("a"), "the second one has exited");
    }

    #[test]
    fn terminating_stops_every_game_of_the_folder() {
        let registry = LaunchRegistry::new(LAUNCH_COOLDOWN);
        let first = FakeProcess::running(7).shared();
        registry.register("a", first.clone());
        registry.register("a", FakeProcess::running(8).shared());
        registry.register("b", FakeProcess::running(9).shared());
        assert_eq!(registry.terminate("a"), 2);
        assert_eq!(first.lock().unwrap().try_wait().unwrap(), Some(None));
        assert!(!registry.is_active("a") && registry.is_active("b"));
        assert!(registry.take_stopped(7) && !registry.take_stopped(7), "reported once");
        assert!(!registry.take_stopped(9));
        assert_eq!(registry.terminate("nothing"), 0);
    }
}

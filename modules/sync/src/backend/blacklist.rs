//! What build sync never shows or touches, whatever a group says: worlds, logs and crashes,
//! caches, the launcher's own files, locks and leftovers. One list of patterns on relative paths,
//! not a rule per mod.

/// Folders never synced, at a build's root only (a mod may keep a `config/logs/`).
const ROOT_FOLDERS: [&str; 4] = ["saves", "logs", "crash-reports", "debug"];
/// Folders never synced wherever they are: caches.
const CACHE_FOLDERS: [&str; 4] = [".cache", ".mixin.out", ".pg-native", "dynamic-resource-pack-cache"];
/// Files never synced wherever they are.
const FILES: [&str; 3] = ["usercache.json", "usernamecache.json", "session.lock"];

/// A name of a lock, a leftover or one of Java's dumps.
fn leftover(name: &str) -> bool {
    name.ends_with(".lock")
        || name.ends_with(".tmp")
        || name.ends_with(".part")
        || name.ends_with("_old")
        || ((name.starts_with("hs_err_pid") || name.starts_with("replay_pid")) && name.ends_with(".log"))
        || (name.starts_with("win_event") && name.ends_with(".txt"))
}

/// `relative` (a path in a build's folder, `/`- or `\`-separated) is never synced, nor anything
/// in it.
pub fn blocked(relative: &str) -> bool {
    let lower = relative.replace('\\', "/").to_lowercase();
    let segments: Vec<&str> = lower.split('/').filter(|s| !s.is_empty()).collect();
    let Some(first) = segments.first() else { return false };
    if ROOT_FOLDERS.contains(first) || (segments.len() == 1 && *first == "version.json") {
        return true;
    }
    if segments.windows(2).any(|pair| pair == ["mods", ".backups"]) {
        return true;
    }
    segments.iter().any(|segment| {
        CACHE_FOLDERS.contains(segment)
            || segment.ends_with("_cache")
            || FILES.contains(segment)
            || segment.starts_with(".launcher")
            || segment.starts_with(".tensalauncher")
            || leftover(segment)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_pattern_is_blocked_at_any_depth_and_case() {
        for path in [
            "saves/World/level.dat",
            "SAVES/x",
            "logs/latest.log",
            "crash-reports/a.txt",
            "debug/x",
            "hs_err_pid1.log",
            "replay_pid2.log",
            "win_event_3.txt",
            ".cache/a",
            "config/.cache/a",
            ".mixin.out/a",
            ".pg-native/a",
            "dynamic-resource-pack-cache/a",
            "foo_cache/a",
            "config/xaero_cache/a",
            "usercache.json",
            "usernamecache.json",
            "version.json",
            ".launcher/journal/sync.json",
            ".launcher-x.json",
            ".tensalauncher/x",
            "mods/.backups/a.jar.backup",
            "session.lock",
            "config/a.lock",
            "a.tmp",
            "config/A.TMP",
            "options.txt_old",
            "mods/a.jar.part",
        ] {
            assert!(blocked(path), "{path} is never synced");
        }
    }

    #[test]
    fn ordinary_paths_pass() {
        for path in [
            "options.txt",
            "servers.dat",
            "config/jei/x.json",
            "resourcepacks/x.zip",
            "xaero",
            "xaero/minimap.txt",
            "config/logs/x.toml",
            "config/version.json",
            "mods/sodium.jar",
            "shaderpacks",
        ] {
            assert!(!blocked(path), "{path} may be synced");
        }
    }
}

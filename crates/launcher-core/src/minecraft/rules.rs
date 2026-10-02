//! `rules` of libraries and arguments (MLL `parse_rule_list`): every rule in a list must pass.

use regex::Regex;
use serde_json::Value;

use super::platform::{GameArch, GamePlatform};

/// Launch features that `features` conditions test.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Features {
    pub custom_resolution: bool,
    pub demo: bool,
    pub quick_play_path: bool,
    pub quick_play_singleplayer: bool,
    pub quick_play_multiplayer: bool,
    pub quick_play_realms: bool,
}

fn arch_matches(value: &str, arch: GameArch) -> bool {
    match value {
        "x86" => arch == GameArch::X86,
        "x86_64" | "amd64" | "x64" => arch == GameArch::X64,
        "arm64" | "aarch64" => arch == GameArch::Arm64,
        // MLL ignores other values.
        _ => true,
    }
}

fn conditions_match(rule: &Value, platform: &GamePlatform, features: &Features) -> bool {
    if let Some(os) = rule.get("os").and_then(Value::as_object) {
        if let Some(name) = os.get("name").and_then(Value::as_str)
            && name != platform.rule_os()
        {
            return false;
        }
        if let Some(arch) = os.get("arch").and_then(Value::as_str)
            && !arch_matches(arch, platform.arch)
        {
            return false;
        }
        if let Some(pattern) = os.get("version").and_then(Value::as_str) {
            // Python's `re.match`: anchored at the start only.
            let anchored = Regex::new(&format!("^(?:{pattern})"));
            if !anchored.is_ok_and(|re| re.is_match(&platform.os_version)) {
                return false;
            }
        }
    }
    if let Some(wanted) = rule.get("features").and_then(Value::as_object) {
        for key in wanted.keys() {
            let on = match key.as_str() {
                "has_custom_resolution" => features.custom_resolution,
                "is_demo_user" => features.demo,
                "has_quick_plays_support" => features.quick_play_path,
                "is_quick_play_singleplayer" => features.quick_play_singleplayer,
                "is_quick_play_multiplayer" => features.quick_play_multiplayer,
                "is_quick_play_realms" => features.quick_play_realms,
                _ => true,
            };
            if !on {
                return false;
            }
        }
    }
    true
}

/// An `allow` rule passes when its conditions match, a `disallow` rule when they do not; any
/// other action never passes.
pub fn rule_passes(rule: &Value, platform: &GamePlatform, features: &Features) -> bool {
    match rule.get("action").and_then(Value::as_str) {
        Some("allow") => conditions_match(rule, platform, features),
        Some("disallow") => !conditions_match(rule, platform, features),
        _ => false,
    }
}

/// The `rules` of a library or argument; absent rules always pass.
pub fn rules_pass(rules: Option<&Value>, platform: &GamePlatform, features: &Features) -> bool {
    match rules {
        Some(Value::Array(list)) => list.iter().all(|rule| rule_passes(rule, platform, features)),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Os;
    use serde_json::json;

    fn on(os: Os, arch: GameArch, version: &str) -> GamePlatform {
        GamePlatform { os, arch, os_version: version.to_string() }
    }

    fn none() -> Features {
        Features::default()
    }

    #[test]
    fn os_rules_follow_the_original() {
        let win = on(Os::Windows, GameArch::X64, "10.0");
        let mac = on(Os::MacOs, GameArch::Arm64, "23.1.0");
        let not_on_mac = json!([{"action": "allow"}, {"action": "disallow", "os": {"name": "osx"}}]);
        assert!(rules_pass(Some(&not_on_mac), &win, &none()));
        assert!(!rules_pass(Some(&not_on_mac), &mac, &none()));
        let only_windows = json!([{"action": "allow", "os": {"name": "windows"}}]);
        assert!(rules_pass(Some(&only_windows), &win, &none()));
        assert!(!rules_pass(Some(&only_windows), &mac, &none()));
        assert!(rules_pass(None, &mac, &none()));
    }

    #[test]
    fn arch_and_version_conditions() {
        let x64 = on(Os::Windows, GameArch::X64, "10.0");
        let x86 = on(Os::Windows, GameArch::X86, "6.1");
        let arm = on(Os::Linux, GameArch::Arm64, "6.8.0-45-generic");
        let only_x86 = json!([{"action": "allow", "os": {"arch": "x86"}}]);
        assert!(rules_pass(Some(&only_x86), &x86, &none()));
        assert!(!rules_pass(Some(&only_x86), &x64, &none()));
        let only_arm = json!([{"action": "allow", "os": {"name": "linux", "arch": "arm64"}}]);
        assert!(rules_pass(Some(&only_arm), &arm, &none()));
        assert!(!rules_pass(Some(&only_arm), &x64, &none()));
        let win10 = json!([{"action": "allow", "os": {"name": "windows", "version": "^10\\."}}]);
        assert!(rules_pass(Some(&win10), &x64, &none()));
        assert!(!rules_pass(Some(&win10), &x86, &none()));
        let anchored = json!([{"action": "allow", "os": {"version": "0"}}]);
        assert!(!rules_pass(Some(&anchored), &x64, &none()), "matched at the start only");
        let broken = json!([{"action": "allow", "os": {"version": "("}}]);
        assert!(!rules_pass(Some(&broken), &x64, &none()));
    }

    #[test]
    fn feature_conditions_need_the_feature() {
        let p = on(Os::Linux, GameArch::X64, "6.8");
        let resolution = json!([{"action": "allow", "features": {"has_custom_resolution": true}}]);
        assert!(!rules_pass(Some(&resolution), &p, &none()));
        let custom = Features { custom_resolution: true, ..Features::default() };
        assert!(rules_pass(Some(&resolution), &p, &custom));
        let quick = json!([{"action": "allow", "features": {"is_quick_play_multiplayer": true}}]);
        let multiplayer = Features { quick_play_multiplayer: true, ..Features::default() };
        assert!(rules_pass(Some(&quick), &p, &multiplayer));
        let unknown = json!([{"action": "allow", "features": {"is_future_feature": true}}]);
        assert!(rules_pass(Some(&unknown), &p, &none()), "MLL ignores unknown features");
        let odd = json!([{"action": "maybe"}]);
        assert!(!rules_pass(Some(&odd), &p, &none()));
    }
}

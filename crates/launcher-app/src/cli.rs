//! Command-line arguments: `--smoke-test`, `--launch-version=<id>` and the
//! update helper mode `--apply-update <marker> --wait-pid <pid>`.

use std::path::PathBuf;

pub const MAX_VERSION_ID_LEN: usize = 512;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct CliArgs {
    pub smoke_test: bool,
    pub launch_version: Option<String>,
    pub apply_update: Option<PathBuf>,
    pub wait_pid: Option<u32>,
}

/// Exact build id from a shortcut: non-empty, no control characters, at most 512 chars.
pub fn validate_version_id(value: &str) -> Option<String> {
    let value = value.trim();
    let valid = !value.is_empty()
        && value.chars().count() <= MAX_VERSION_ID_LEN
        && !value.chars().any(char::is_control);
    valid.then(|| value.to_string())
}

fn option_value<I: Iterator<Item = String>>(
    arg: &str,
    name: &str,
    rest: &mut std::iter::Peekable<I>,
) -> Option<Option<String>> {
    if let Some(value) = arg.strip_prefix(name).and_then(|v| v.strip_prefix('=')) {
        return Some(Some(value.to_string()));
    }
    if arg == name {
        let next = rest.next_if(|v| !v.starts_with("--"));
        return Some(next);
    }
    None
}

/// Parses the full argv (the first element is the program path). Unknown arguments are ignored.
pub fn parse_args<I: IntoIterator<Item = String>>(argv: I) -> CliArgs {
    let mut out = CliArgs::default();
    let mut iter = argv.into_iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        if arg == "--smoke-test" {
            out.smoke_test = true;
        } else if let Some(value) = option_value(&arg, "--launch-version", &mut iter) {
            out.launch_version = value.as_deref().and_then(validate_version_id);
        } else if let Some(value) = option_value(&arg, "--apply-update", &mut iter) {
            out.apply_update = value.filter(|v| !v.trim().is_empty()).map(PathBuf::from);
        } else if let Some(value) = option_value(&arg, "--wait-pid", &mut iter) {
            out.wait_pid = value.and_then(|v| v.trim().parse().ok());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn argv(args: &[&str]) -> Vec<String> {
        std::iter::once("Launcher.exe").chain(args.iter().copied()).map(String::from).collect()
    }

    #[test]
    fn parse_launch_version_equals_and_separate_forms() {
        assert_eq!(
            parse_args(argv(&["--launch-version=aeronautics"])).launch_version.as_deref(),
            Some("aeronautics")
        );
        assert_eq!(
            parse_args(argv(&["--launch-version", "my_build"])).launch_version.as_deref(),
            Some("my_build")
        );
        assert_eq!(parse_args(argv(&[])).launch_version, None);
    }

    #[test]
    fn parse_launch_version_rejects_empty_control_and_long_ids() {
        assert_eq!(parse_args(argv(&["--launch-version="])).launch_version, None);
        assert_eq!(parse_args(argv(&["--launch-version"])).launch_version, None);
        assert_eq!(parse_args(argv(&["--launch-version=a\nb"])).launch_version, None);
        let long = format!("--launch-version={}", "x".repeat(513));
        assert_eq!(parse_args(argv(&[&long])).launch_version, None);
        let max = format!("--launch-version={}", "я".repeat(512));
        assert!(parse_args(argv(&[&max])).launch_version.is_some());
    }

    #[test]
    fn parse_smoke_flag_and_ignore_unknown() {
        let a = parse_args(argv(&["--foo", "--smoke-test", "bar"]));
        assert!(a.smoke_test);
        assert_eq!(a.launch_version, None);
    }

    #[test]
    fn parse_apply_update_mode() {
        let marker = "C:\\Users\\Іван & Co\\cache\\pending-update\\pending_update.json";
        let a = parse_args(argv(&["--apply-update", marker, "--wait-pid", "4242"]));
        assert_eq!(a.apply_update.as_deref(), Some(Path::new(marker)));
        assert_eq!(a.wait_pid, Some(4242));
        let b = parse_args(argv(&["--apply-update=/tmp/m.json", "--wait-pid=7"]));
        assert_eq!(b.apply_update.as_deref(), Some(Path::new("/tmp/m.json")));
        assert_eq!(b.wait_pid, Some(7));
        let c = parse_args(argv(&["--apply-update", "--wait-pid", "x"]));
        assert_eq!(c.apply_update, None);
        assert_eq!(c.wait_pid, None);
    }
}

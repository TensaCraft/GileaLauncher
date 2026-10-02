//! Paths that come from the network or from archives: only plain relative paths that
//! stay inside the folder they are joined to.

use std::path::{Component, Path, PathBuf};

/// `raw` as a relative path, or `None` when it is empty, absolute, names a drive or a stream,
/// holds control characters or steps outside with `..`. Both `/` and `\` separate components;
/// empty and `.` components are dropped.
pub fn safe_relative(raw: &str) -> Option<PathBuf> {
    if raw.starts_with(['/', '\\']) {
        return None;
    }
    let mut path = PathBuf::new();
    for part in raw.split(['/', '\\']).filter(|p| !p.is_empty() && *p != ".") {
        let mut components = Path::new(part).components();
        let plain = matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
        if !plain || part == ".." || part.contains(':') || part.chars().any(char::is_control) {
            return None;
        }
        path.push(part);
    }
    (!path.as_os_str().is_empty()).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_relative_paths_pass() {
        let expected = PathBuf::from("com").join("example").join("core-1.0.jar");
        assert_eq!(safe_relative("com/example/core-1.0.jar"), Some(expected));
        assert_eq!(safe_relative("bin\\java.exe"), Some(PathBuf::from("bin").join("java.exe")));
        assert_eq!(safe_relative("a//./b"), Some(PathBuf::from("a").join("b")));
    }

    #[test]
    fn escaping_paths_are_refused() {
        for bad in [
            "",
            ".",
            "/etc/passwd",
            "\\\\server\\share",
            "../x",
            "a/../../x",
            "C:/Windows",
            "C:x",
            "a/b:s",
            "a\u{0}b",
        ] {
            assert_eq!(safe_relative(bad), None, "{bad:?}");
        }
    }
}

//! The machine a game runs on, in the vocabulary of Mojang's metadata.

use crate::paths::Os;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameArch {
    X86,
    X64,
    Arm64,
}

impl GameArch {
    pub fn current() -> GameArch {
        if cfg!(target_arch = "aarch64") {
            GameArch::Arm64
        } else if cfg!(target_pointer_width = "32") {
            GameArch::X86
        } else {
            GameArch::X64
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GamePlatform {
    pub os: Os,
    pub arch: GameArch,
    /// What `os.version` rules match against (MLL `get_os_version`).
    pub os_version: String,
}

impl GamePlatform {
    pub fn current() -> GamePlatform {
        GamePlatform { os: Os::current(), arch: GameArch::current(), os_version: os_version() }
    }

    /// `os.name` in rules and the key of `natives` maps.
    pub fn rule_os(&self) -> &'static str {
        match self.os {
            Os::Windows => "windows",
            Os::MacOs => "osx",
            Os::Linux | Os::Other => "linux",
        }
    }

    /// `${arch}` in natives classifiers.
    pub fn natives_arch(&self) -> &'static str {
        if self.arch == GameArch::X86 { "32" } else { "64" }
    }

    /// Key of Mojang's Java runtime list; `None` where Mojang ships no runtime.
    pub fn java_runtime_key(&self) -> Option<&'static str> {
        match (self.os, self.arch) {
            (Os::Windows, GameArch::X64) => Some("windows-x64"),
            (Os::Windows, GameArch::X86) => Some("windows-x86"),
            (Os::Windows, GameArch::Arm64) => Some("windows-arm64"),
            (Os::Linux, GameArch::X64) => Some("linux"),
            (Os::Linux, GameArch::X86) => Some("linux-i386"),
            (Os::MacOs, GameArch::Arm64) => Some("mac-os-arm64"),
            (Os::MacOs, _) => Some("mac-os"),
            _ => None,
        }
    }

    /// The runtime's launcher binary name.
    pub fn java_binary(&self) -> &'static str {
        if self.os == Os::Windows { "java.exe" } else { "java" }
    }
}

#[cfg(windows)]
fn os_version() -> String {
    // Tauri 2 needs Windows 10 or newer, and Windows 11 reports 10.0 as well.
    "10.0".to_string()
}

#[cfg(unix)]
fn os_version() -> String {
    // SAFETY: `uname` fills the zeroed struct; on success `release` is NUL-terminated.
    let mut name: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut name) } != 0 {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(name.release.as_ptr()) }.to_string_lossy().into_owned()
}

#[cfg(not(any(windows, unix)))]
fn os_version() -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(os: Os, arch: GameArch) -> GamePlatform {
        GamePlatform { os, arch, os_version: String::new() }
    }

    #[test]
    fn names_follow_mojang_metadata() {
        assert_eq!(on(Os::Windows, GameArch::X64).rule_os(), "windows");
        assert_eq!(on(Os::MacOs, GameArch::Arm64).rule_os(), "osx");
        assert_eq!(on(Os::Linux, GameArch::X64).rule_os(), "linux");
        assert_eq!(on(Os::Linux, GameArch::X86).natives_arch(), "32");
        assert_eq!(on(Os::MacOs, GameArch::Arm64).natives_arch(), "64");
        assert_eq!(on(Os::Windows, GameArch::X64).java_binary(), "java.exe");
        assert_eq!(on(Os::Linux, GameArch::X64).java_binary(), "java");
    }

    #[test]
    fn java_runtime_keys_cover_mojang_platforms() {
        let cases = [
            (Os::Windows, GameArch::X64, Some("windows-x64")),
            (Os::Windows, GameArch::X86, Some("windows-x86")),
            (Os::Windows, GameArch::Arm64, Some("windows-arm64")),
            (Os::Linux, GameArch::X64, Some("linux")),
            (Os::Linux, GameArch::X86, Some("linux-i386")),
            (Os::Linux, GameArch::Arm64, None),
            (Os::MacOs, GameArch::X64, Some("mac-os")),
            (Os::MacOs, GameArch::Arm64, Some("mac-os-arm64")),
            (Os::Other, GameArch::X64, None),
        ];
        for (os, arch, key) in cases {
            assert_eq!(on(os, arch).java_runtime_key(), key, "{os:?} {arch:?}");
        }
    }

    #[test]
    fn the_current_platform_reports_an_os_version() {
        let current = GamePlatform::current();
        if cfg!(windows) {
            assert_eq!(current.os_version, "10.0");
        }
        if cfg!(target_os = "linux") {
            assert!(!current.os_version.is_empty());
        }
    }
}

//! GPU preference for the game. Windows: by default (`auto`) Windows decides and the registry is
//! not touched; a GPU the user chose is the per-program value under
//! `HKCU\Software\Microsoft\DirectX\UserGpuPreferences` (what Settings → Display → Graphics
//! writes), taken back when they return to `auto`. Linux: PRIME render offload to NVIDIA for
//! `dgpu` (the default there); macOS: nothing.

use std::io;
use std::path::Path;

use crate::paths::Os;

pub const GPU_PREFERENCES_KEY: &str = r"Software\Microsoft\DirectX\UserGpuPreferences";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuMode {
    Auto,
    Igpu,
    Dgpu,
}

/// The mode a build has when nobody chose one: on Windows `auto` (nothing goes to the registry),
/// elsewhere `dgpu` (on Linux an environment variable, nothing kept).
pub fn platform_default(os: Os) -> GpuMode {
    if os == Os::Windows { GpuMode::Auto } else { GpuMode::Dgpu }
}

impl GpuMode {
    /// `auto`, `igpu` or `dgpu`; anything else is this system's default.
    pub fn parse(raw: Option<&str>) -> GpuMode {
        match raw.map(|r| r.trim().to_ascii_lowercase()).as_deref() {
            Some("auto") => GpuMode::Auto,
            Some("igpu") => GpuMode::Igpu,
            Some("dgpu") => GpuMode::Dgpu,
            _ => platform_default(Os::current()),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            GpuMode::Auto => "auto",
            GpuMode::Igpu => "igpu",
            GpuMode::Dgpu => "dgpu",
        }
    }

    /// The registry value: 0 lets Windows decide, 1 saves power, 2 is high performance.
    pub fn windows_preference(self) -> &'static str {
        match self {
            GpuMode::Auto => "GpuPreference=0;",
            GpuMode::Igpu => "GpuPreference=1;",
            GpuMode::Dgpu => "GpuPreference=2;",
        }
    }
}

/// Where Windows keeps per-program GPU choices; tests use an in-memory store.
pub trait GpuPreferenceStore: Send + Sync {
    fn set(&self, exe: &Path, value: &str) -> io::Result<()>;
    /// Takes `exe`'s choice back (none there is no error).
    fn remove(&self, exe: &Path) -> io::Result<()>;
}

/// Writes nothing (tools and tests).
pub struct NoGpuPreferences;

impl GpuPreferenceStore for NoGpuPreferences {
    fn set(&self, _exe: &Path, _value: &str) -> io::Result<()> {
        Ok(())
    }

    fn remove(&self, _exe: &Path) -> io::Result<()> {
        Ok(())
    }
}

/// The current user's registry (a no-op elsewhere).
pub struct WindowsGpuPreferences;

#[cfg(windows)]
impl GpuPreferenceStore for WindowsGpuPreferences {
    fn set(&self, exe: &Path, value: &str) -> io::Result<()> {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW};
        let wide = |text: &OsStr| text.encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let key = wide(OsStr::new(GPU_PREFERENCES_KEY));
        let name = wide(exe.as_os_str());
        let data = wide(OsStr::new(value));
        // SAFETY: NUL-terminated UTF-16 strings that outlive the call; the size covers the terminator.
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        };
        if status == 0 { Ok(()) } else { Err(io::Error::from_raw_os_error(status as i32)) }
    }

    fn remove(&self, exe: &Path) -> io::Result<()> {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
        use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteKeyValueW};
        let wide = |text: &OsStr| text.encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let key = wide(OsStr::new(GPU_PREFERENCES_KEY));
        let name = wide(exe.as_os_str());
        // SAFETY: NUL-terminated UTF-16 strings that outlive the call.
        let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
        match status {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
            other => Err(io::Error::from_raw_os_error(other as i32)),
        }
    }
}

#[cfg(not(windows))]
impl GpuPreferenceStore for WindowsGpuPreferences {
    fn set(&self, _exe: &Path, _value: &str) -> io::Result<()> {
        Ok(())
    }

    fn remove(&self, _exe: &Path) -> io::Result<()> {
        Ok(())
    }
}

/// PRIME render offload to NVIDIA's driver for `dgpu`; nothing otherwise.
pub fn linux_gpu_env(mode: GpuMode, nvidia: bool) -> Vec<(String, String)> {
    if mode != GpuMode::Dgpu || !nvidia {
        return Vec::new();
    }
    [("DRI_PRIME", "1"), ("__NV_PRIME_RENDER_OFFLOAD", "1"), ("__GLX_VENDOR_LIBRARY_NAME", "nvidia")]
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .to_vec()
}

pub fn nvidia_driver_loaded() -> bool {
    Path::new("/proc/driver/nvidia/version").exists()
}

/// The Java programs whose GPU choice the launcher wrote (config): only those it takes back.
pub const GPU_WRITTEN_KEY: &str = "gpu_preference_paths";

/// Applies `mode` for `java` and returns variables for the game process. On Windows `auto` leaves
/// the registry alone, unless `written` (the programs the launcher wrote a choice for) has `java`:
/// that choice is taken back. A registry failure is logged, never fatal.
pub fn apply_gpu_mode(
    mode: GpuMode,
    java: &Path,
    os: Os,
    store: &dyn GpuPreferenceStore,
    nvidia: bool,
    written: &mut Vec<String>,
) -> Vec<(String, String)> {
    match os {
        Os::Windows => {
            let path = java.to_string_lossy().into_owned();
            let ours = written.iter().any(|p| p.eq_ignore_ascii_case(&path));
            if mode == GpuMode::Auto {
                if ours {
                    match store.remove(java) {
                        Ok(()) => written.retain(|p| !p.eq_ignore_ascii_case(&path)),
                        Err(e) => {
                            tracing::warn!("Unable to take back the GPU choice of {}: {e}", java.display())
                        }
                    }
                }
            } else {
                match store.set(java, mode.windows_preference()) {
                    Ok(()) if !ours => written.push(path),
                    Ok(()) => {}
                    Err(e) => tracing::warn!("Unable to save the GPU preference for {}: {e}", java.display()),
                }
            }
            Vec::new()
        }
        Os::Linux => linux_gpu_env(mode, nvidia),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// What was asked of the registry: `Some(value)` written, `None` removed.
    #[derive(Default)]
    struct Recorded {
        calls: Mutex<Vec<(PathBuf, Option<String>)>>,
        fail: bool,
    }

    impl GpuPreferenceStore for Recorded {
        fn set(&self, exe: &Path, value: &str) -> io::Result<()> {
            self.calls.lock().unwrap().push((exe.to_path_buf(), Some(value.to_string())));
            if self.fail { Err(io::Error::other("registry is read-only")) } else { Ok(()) }
        }

        fn remove(&self, exe: &Path) -> io::Result<()> {
            self.calls.lock().unwrap().push((exe.to_path_buf(), None));
            if self.fail { Err(io::Error::other("registry is read-only")) } else { Ok(()) }
        }
    }

    #[test]
    fn windows_leaves_the_gpu_to_itself_unless_asked() {
        assert_eq!(platform_default(Os::Windows), GpuMode::Auto, "nothing is written by default");
        assert_eq!(platform_default(Os::Linux), GpuMode::Dgpu);
        assert_eq!(platform_default(Os::MacOs), GpuMode::Dgpu);
        assert_eq!(GpuMode::parse(Some("auto")), GpuMode::Auto);
        assert_eq!(GpuMode::parse(Some(" IGPU ")), GpuMode::Igpu);
        assert_eq!(GpuMode::parse(Some("dgpu")), GpuMode::Dgpu);
        assert_eq!(GpuMode::parse(Some("rtx")), platform_default(Os::current()));
        assert_eq!(GpuMode::parse(None), platform_default(Os::current()));
        assert_eq!(GpuMode::Igpu.as_str(), "igpu");
        let values = [GpuMode::Auto, GpuMode::Igpu, GpuMode::Dgpu].map(GpuMode::windows_preference);
        assert_eq!(values, ["GpuPreference=0;", "GpuPreference=1;", "GpuPreference=2;"]);
    }

    #[test]
    fn windows_writes_a_chosen_gpu_and_takes_back_only_its_own() {
        let store = Recorded::default();
        let java = Path::new("C:/mc/runtime/java.exe");
        let mut written = Vec::new();
        // Auto with nothing written by the launcher: the registry is not touched at all.
        assert!(apply_gpu_mode(GpuMode::Auto, java, Os::Windows, &store, false, &mut written).is_empty());
        assert!(store.calls.lock().unwrap().is_empty());
        apply_gpu_mode(GpuMode::Igpu, java, Os::Windows, &store, false, &mut written);
        apply_gpu_mode(GpuMode::Dgpu, java, Os::Windows, &store, false, &mut written);
        assert_eq!(written, ["C:/mc/runtime/java.exe"], "remembered once");
        // Back to Auto: the launcher's own value goes, and is forgotten.
        apply_gpu_mode(GpuMode::Auto, java, Os::Windows, &store, false, &mut written);
        assert!(written.is_empty());
        assert_eq!(
            store.calls.lock().unwrap().clone(),
            [
                (java.to_path_buf(), Some("GpuPreference=1;".to_string())),
                (java.to_path_buf(), Some("GpuPreference=2;".to_string())),
                (java.to_path_buf(), None),
            ]
        );
        apply_gpu_mode(GpuMode::Auto, java, Os::Windows, &store, false, &mut written);
        assert_eq!(store.calls.lock().unwrap().len(), 3, "nothing more to take back");
    }

    #[test]
    fn a_registry_failure_never_stops_the_launch_nor_is_remembered() {
        let failing = Recorded { fail: true, ..Recorded::default() };
        let java = Path::new("C:/mc/runtime/java.exe");
        let mut written = Vec::new();
        assert!(apply_gpu_mode(GpuMode::Dgpu, java, Os::Windows, &failing, false, &mut written).is_empty());
        assert!(written.is_empty(), "what was not written is not taken back later");
        let mut ours = vec!["C:/mc/runtime/java.exe".to_string()];
        apply_gpu_mode(GpuMode::Auto, java, Os::Windows, &failing, false, &mut ours);
        assert_eq!(ours.len(), 1, "still ours until it is gone");
    }

    #[test]
    fn linux_offloads_to_nvidia_only_for_dgpu() {
        let store = Recorded::default();
        let java = Path::new("/mc/runtime/java");
        let mut written = Vec::new();
        let env = apply_gpu_mode(GpuMode::Dgpu, java, Os::Linux, &store, true, &mut written);
        let names: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, ["DRI_PRIME", "__NV_PRIME_RENDER_OFFLOAD", "__GLX_VENDOR_LIBRARY_NAME"]);
        assert!(apply_gpu_mode(GpuMode::Dgpu, java, Os::Linux, &store, false, &mut written).is_empty());
        assert!(apply_gpu_mode(GpuMode::Igpu, java, Os::Linux, &store, true, &mut written).is_empty());
        assert!(apply_gpu_mode(GpuMode::Dgpu, java, Os::MacOs, &store, true, &mut written).is_empty());
        assert!(store.calls.lock().unwrap().is_empty(), "the registry is Windows-only");
        assert!(written.is_empty());
    }
}

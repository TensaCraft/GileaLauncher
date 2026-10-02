//! GPU preference for the game. Windows: the per-program value under
//! `HKCU\Software\Microsoft\DirectX\UserGpuPreferences`, for every mode;
//! Linux: PRIME render offload to NVIDIA for `dgpu`; macOS: nothing.

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

impl GpuMode {
    /// `auto`, `igpu` or `dgpu`; anything else is `dgpu`, the original's default.
    pub fn parse(raw: Option<&str>) -> GpuMode {
        match raw.map(|r| r.trim().to_ascii_lowercase()).as_deref() {
            Some("auto") => GpuMode::Auto,
            Some("igpu") => GpuMode::Igpu,
            _ => GpuMode::Dgpu,
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
}

/// Writes nothing (tools and tests).
pub struct NoGpuPreferences;

impl GpuPreferenceStore for NoGpuPreferences {
    fn set(&self, _exe: &Path, _value: &str) -> io::Result<()> {
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
}

#[cfg(not(windows))]
impl GpuPreferenceStore for WindowsGpuPreferences {
    fn set(&self, _exe: &Path, _value: &str) -> io::Result<()> {
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

/// Applies `mode` for `java` and returns variables for the game process. A registry failure is
/// logged, never fatal (the original ignored it too).
pub fn apply_gpu_mode(
    mode: GpuMode,
    java: &Path,
    os: Os,
    store: &dyn GpuPreferenceStore,
    nvidia: bool,
) -> Vec<(String, String)> {
    match os {
        Os::Windows => {
            if let Err(e) = store.set(java, mode.windows_preference()) {
                tracing::warn!("Unable to save the GPU preference for {}: {e}", java.display());
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

    #[derive(Default)]
    struct Recorded {
        calls: Mutex<Vec<(PathBuf, String)>>,
        fail: bool,
    }

    impl GpuPreferenceStore for Recorded {
        fn set(&self, exe: &Path, value: &str) -> io::Result<()> {
            self.calls.lock().unwrap().push((exe.to_path_buf(), value.to_string()));
            if self.fail { Err(io::Error::other("registry is read-only")) } else { Ok(()) }
        }
    }

    #[test]
    fn modes_parse_with_dgpu_as_the_default() {
        assert_eq!(GpuMode::parse(Some("auto")), GpuMode::Auto);
        assert_eq!(GpuMode::parse(Some(" IGPU ")), GpuMode::Igpu);
        assert_eq!(GpuMode::parse(Some("dgpu")), GpuMode::Dgpu);
        assert_eq!(GpuMode::parse(Some("rtx")), GpuMode::Dgpu);
        assert_eq!(GpuMode::parse(None), GpuMode::Dgpu);
        assert_eq!(GpuMode::Igpu.as_str(), "igpu");
        let values = [GpuMode::Auto, GpuMode::Igpu, GpuMode::Dgpu].map(GpuMode::windows_preference);
        assert_eq!(values, ["GpuPreference=0;", "GpuPreference=1;", "GpuPreference=2;"]);
    }

    #[test]
    fn windows_writes_every_mode_to_the_registry() {
        let store = Recorded::default();
        let java = Path::new("C:/mc/runtime/java.exe");
        assert!(apply_gpu_mode(GpuMode::Igpu, java, Os::Windows, &store, false).is_empty());
        assert!(apply_gpu_mode(GpuMode::Auto, java, Os::Windows, &store, false).is_empty());
        let calls = store.calls.lock().unwrap().clone();
        assert_eq!(
            calls,
            [
                (java.to_path_buf(), "GpuPreference=1;".to_string()),
                (java.to_path_buf(), "GpuPreference=0;".to_string())
            ]
        );
        let failing = Recorded { fail: true, ..Recorded::default() };
        assert!(
            apply_gpu_mode(GpuMode::Dgpu, java, Os::Windows, &failing, false).is_empty(),
            "a failure never stops the launch"
        );
    }

    #[test]
    fn linux_offloads_to_nvidia_only_for_dgpu() {
        let store = Recorded::default();
        let java = Path::new("/mc/runtime/java");
        let env = apply_gpu_mode(GpuMode::Dgpu, java, Os::Linux, &store, true);
        let names: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, ["DRI_PRIME", "__NV_PRIME_RENDER_OFFLOAD", "__GLX_VENDOR_LIBRARY_NAME"]);
        assert!(apply_gpu_mode(GpuMode::Dgpu, java, Os::Linux, &store, false).is_empty());
        assert!(apply_gpu_mode(GpuMode::Igpu, java, Os::Linux, &store, true).is_empty());
        assert!(apply_gpu_mode(GpuMode::Dgpu, java, Os::MacOs, &store, true).is_empty());
        assert!(store.calls.lock().unwrap().is_empty(), "the registry is Windows-only");
    }
}

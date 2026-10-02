//! Opening folders, files and links in the system's programs.
//!
//! On Linux the desktop's file manager shows a folder over D-Bus (`FileManager1.ShowFolders`), as
//! "show in folder" does. Anything else goes to the system opener (`xdg-open` and its kin), started
//! without the AppImage's environment (`child_env`): with it, the program it opens can fail without
//! a word. Elsewhere the system opener does it all.

use std::path::Path;

/// Opens `dir`: the desktop's own way first (`system`), the system opener (`fallback`) if that
/// fails. The error tells both failures.
pub fn open_folder_with(
    dir: &Path,
    system: impl FnOnce(&Path) -> Result<(), String>,
    fallback: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    match system(dir) {
        Ok(()) => Ok(()),
        Err(first) => fallback(dir).map_err(|second| format!("{first}; {second}")),
    }
}

/// Opens `dir` in the file manager; `fallback` is the system opener.
pub fn open_folder(dir: &Path, fallback: impl FnOnce(&Path) -> Result<(), String>) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    return open_folder_with(dir, linux::show_folder, fallback);
    #[cfg(not(target_os = "linux"))]
    return fallback(dir);
}

/// Opens `target` (a link, a file or a folder) in its program on Linux: the first system opener
/// there is, started without the AppImage's environment and not waited for.
#[cfg(target_os = "linux")]
pub fn open_detached(target: &std::ffi::OsStr) -> Result<(), String> {
    use std::process::Stdio;
    let mut tried = Vec::new();
    for mut command in open::commands(target) {
        super::child_env::clean(&mut command);
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        match command.spawn() {
            Ok(mut child) => {
                // Reaped in the background: an opener that quits never stays a zombie.
                std::thread::spawn(move || child.wait());
                return Ok(());
            }
            Err(e) => tried.push(format!("{}: {e}", command.get_program().to_string_lossy())),
        }
    }
    Err(format!("no system opener: {}", tried.join("; ")))
}

#[cfg(target_os = "linux")]
mod linux {
    use std::path::Path;

    #[zbus::proxy(
        interface = "org.freedesktop.FileManager1",
        default_service = "org.freedesktop.FileManager1",
        default_path = "/org/freedesktop/FileManager1"
    )]
    trait FileManager1 {
        #[zbus(name = "ShowFolders")]
        fn show_folders(&self, uris: &[&str], startup_id: &str) -> zbus::Result<()>;
    }

    /// Asks the desktop's file manager to open `dir` (`FileManager1.ShowFolders`).
    pub fn show_folder(dir: &Path) -> Result<(), String> {
        let uri = url::Url::from_directory_path(dir)
            .map_err(|()| format!("not a folder path: {}", dir.display()))?;
        let connection =
            zbus::blocking::Connection::session().map_err(|e| format!("no D-Bus session: {e}"))?;
        let manager =
            FileManager1ProxyBlocking::new(&connection).map_err(|e| format!("no file manager: {e}"))?;
        manager.show_folders(&[uri.as_str()], "").map_err(|e| format!("the file manager refused: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn the_desktop_s_own_way_comes_first_and_the_fallback_only_when_it_fails() {
        let dir = Path::new("/home/player/builds");
        let fallback_used = Cell::new(false);
        let fallback = |_: &Path| {
            fallback_used.set(true);
            Ok(())
        };
        assert_eq!(open_folder_with(dir, |_| Ok(()), fallback), Ok(()));
        assert!(!fallback_used.get(), "the file manager opened it: nothing more to do");

        assert_eq!(open_folder_with(dir, |_| Err("no FileManager1".into()), fallback), Ok(()));
        assert!(fallback_used.get(), "no desktop service: the system opener tries");

        let both = open_folder_with(dir, |_| Err("no FileManager1".into()), |_| Err("no xdg-open".into()));
        assert_eq!(both, Err("no FileManager1; no xdg-open".to_string()));
    }
}

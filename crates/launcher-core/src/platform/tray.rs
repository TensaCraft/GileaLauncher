//! Whether the desktop shows tray icons. On Linux the icon goes to a StatusNotifier host, which
//! stock GNOME lacks: hidden there, the window would have no way back.

/// A tray icon would be shown: always on Windows and macOS; on Linux when a StatusNotifier host is
/// registered on the session bus. Blocking (a D-Bus call on Linux).
pub fn tray_shown() -> bool {
    #[cfg(target_os = "linux")]
    {
        linux::host_registered().unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

#[cfg(target_os = "linux")]
mod linux {
    #[zbus::proxy(
        interface = "org.kde.StatusNotifierWatcher",
        default_service = "org.kde.StatusNotifierWatcher",
        default_path = "/StatusNotifierWatcher"
    )]
    trait StatusNotifierWatcher {
        #[zbus(property, name = "IsStatusNotifierHostRegistered")]
        fn is_status_notifier_host_registered(&self) -> zbus::Result<bool>;
    }

    /// The watcher's word; `None` without a session bus or a watcher.
    pub fn host_registered() -> Option<bool> {
        let connection = zbus::blocking::Connection::session().ok()?;
        let watcher = StatusNotifierWatcherProxyBlocking::new(&connection).ok()?;
        watcher.is_status_notifier_host_registered().ok()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(not(target_os = "linux"))]
    fn windows_and_macos_always_show_the_tray() {
        assert!(super::tray_shown());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn without_a_session_bus_linux_shows_no_tray() {
        // A test has no desktop: no bus, no watcher — the window is minimized instead.
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
            assert!(!super::tray_shown());
        }
    }
}

//! The launcher window's size (Settings → Interface): applied at startup and whenever it changes.
//! A change goes step by step: window managers on Linux leave full screen or maximized in their
//! own time and ignore a size asked for before that, so each step waits for the last one.

use std::sync::Mutex;
use std::time::Duration;

use launcher_shared::{WINDOW_MIN, WindowSize};
use tauri::{LogicalSize, WebviewWindow};

/// The frame around the window's content in logical pixels: outer minus inner size. The launcher
/// draws its own title bar, so mostly none.
pub fn window_frame(outer: (u32, u32), inner: (u32, u32), scale: f64) -> (f64, f64) {
    let side = |o: u32, i: u32| f64::from(o.saturating_sub(i)) / scale;
    (side(outer.0, inner.0), side(outer.1, inner.1))
}

/// The content size a window may take: the monitor's work area (without the taskbar) minus the
/// window frame, in logical pixels.
pub fn available_area(work_area: (f64, f64), frame: (f64, f64)) -> (f64, f64) {
    ((work_area.0 - frame.0).max(0.0), (work_area.1 - frame.1).max(0.0))
}

/// `width`×`height` shrunk to fit the screen (logical pixels), never below the minimum.
pub fn fit_to_screen(width: u32, height: u32, screen: Option<(f64, f64)>) -> (u32, u32) {
    let (max_w, max_h) = screen.map_or((u32::MAX, u32::MAX), |(w, h)| (w as u32, h as u32));
    (width.min(max_w).max(WINDOW_MIN.0), height.min(max_h).max(WINDOW_MIN.1))
}

/// The content size the monitor under `window` leaves it (logical pixels).
fn screen_of(window: &WebviewWindow) -> Option<(f64, f64)> {
    window.current_monitor().ok().flatten().map(|m| {
        let scale = m.scale_factor();
        let work = m.work_area().size.to_logical::<f64>(scale);
        let frame = match (window.outer_size(), window.inner_size()) {
            (Ok(o), Ok(i)) => window_frame((o.width, o.height), (i.width, i.height), scale),
            _ => (0.0, 0.0),
        };
        available_area((work.width, work.height), frame)
    })
}

/// Sizes the new window at startup, before it shows (it is in no other state yet).
pub fn apply_window_size(window: &WebviewWindow, size: WindowSize) {
    let result = match size {
        WindowSize::Fullscreen => window.set_fullscreen(true),
        WindowSize::Maximized => window.set_fullscreen(false).and_then(|()| window.maximize()),
        WindowSize::Size { width, height } => {
            let (w, h) = fit_to_screen(width, height, screen_of(window));
            window
                .set_fullscreen(false)
                .and_then(|()| window.unmaximize())
                .and_then(|()| window.set_size(LogicalSize::new(f64::from(w), f64::from(h))))
                .and_then(|()| window.center())
        }
    };
    if let Err(e) = result {
        tracing::warn!("Unable to resize the launcher window: {e}");
    }
}

/// What changing the size needs of a window: the launcher's, or a stand-in in tests.
pub trait Resizable {
    fn is_fullscreen(&self) -> bool;
    fn is_maximized(&self) -> bool;
    fn set_fullscreen(&self, on: bool);
    fn maximize(&self);
    fn unmaximize(&self);
    /// The content size in logical pixels.
    fn content_size(&self) -> (u32, u32);
    fn set_content_size(&self, size: (u32, u32));
    fn center(&self);
    /// The content size the screen leaves the window (logical pixels), when known.
    fn screen(&self) -> Option<(f64, f64)>;
}

/// How long a window manager gets for each step before the change goes on anyway.
const SETTLE: Duration = Duration::from_millis(1500);
/// How often the window is looked at meanwhile.
const LOOK: Duration = Duration::from_millis(25);
/// How many times a size is asked for: a window manager may drop one asked for right after it
/// restored the window.
const SIZE_TRIES: usize = 3;

/// Changes a shown window to `size`; `pause` waits a moment between looks at the window.
pub fn change_size<W: Resizable>(window: &W, size: WindowSize, pause: impl Fn(Duration)) {
    match size {
        WindowSize::Fullscreen => window.set_fullscreen(true),
        WindowSize::Maximized => {
            leave_fullscreen(window, &pause);
            window.maximize();
        }
        WindowSize::Size { width, height } => {
            leave_fullscreen(window, &pause);
            if window.is_maximized() {
                window.unmaximize();
                wait_until(|| !window.is_maximized(), &pause);
            }
            let target = fit_to_screen(width, height, window.screen());
            let taken = (0..SIZE_TRIES).any(|_| {
                window.set_content_size(target);
                wait_until(|| near(window.content_size(), target), &pause)
            });
            if !taken {
                tracing::warn!(
                    "The window manager kept the launcher window at {:?} instead of {target:?}",
                    window.content_size()
                );
            }
            window.center();
        }
    }
}

/// The title bar's maximize button: a window filling the screen (maximized or full screen) goes
/// back to its size, any other is maximized. Whether it fills the screen then.
pub fn toggle_maximized<W: Resizable>(window: &W) -> bool {
    if window.is_fullscreen() {
        window.set_fullscreen(false);
        false
    } else if window.is_maximized() {
        window.unmaximize();
        false
    } else {
        window.maximize();
        true
    }
}

fn leave_fullscreen<W: Resizable>(window: &W, pause: &impl Fn(Duration)) {
    if window.is_fullscreen() {
        window.set_fullscreen(false);
        wait_until(|| !window.is_fullscreen(), pause);
    }
}

/// Waits until `done` holds, at most `SETTLE`; whether it came to hold.
fn wait_until(done: impl Fn() -> bool, pause: &impl Fn(Duration)) -> bool {
    for _ in 0..(SETTLE.as_millis() / LOOK.as_millis()) {
        if done() {
            return true;
        }
        pause(LOOK);
    }
    done()
}

/// Sizes that differ by a rounding of the scale factor are the same.
fn near(a: (u32, u32), b: (u32, u32)) -> bool {
    a.0.abs_diff(b.0) <= 2 && a.1.abs_diff(b.1) <= 2
}

/// Changes the shown launcher window to `size` on a thread of its own (the steps wait for the
/// window manager); one change at a time.
pub fn change_window_size(window: WebviewWindow, size: WindowSize) {
    static CHANGING: Mutex<()> = Mutex::new(());
    let spawned = std::thread::Builder::new().name("window-size".into()).spawn(move || {
        let _one = CHANGING.lock().unwrap_or_else(|e| e.into_inner());
        change_size(&window, size, std::thread::sleep);
    });
    if let Err(e) = spawned {
        tracing::warn!("Unable to resize the launcher window: {e}");
    }
}

/// Logs a window call that failed; nothing more can be done about it.
fn logged(what: &str, result: tauri::Result<()>) {
    if let Err(e) = result {
        tracing::warn!("Unable to {what} the launcher window: {e}");
    }
}

impl Resizable for WebviewWindow {
    fn is_fullscreen(&self) -> bool {
        WebviewWindow::is_fullscreen(self).unwrap_or(false)
    }
    fn is_maximized(&self) -> bool {
        WebviewWindow::is_maximized(self).unwrap_or(false)
    }
    fn set_fullscreen(&self, on: bool) {
        logged("switch full screen for", WebviewWindow::set_fullscreen(self, on));
    }
    fn maximize(&self) {
        logged("maximize", WebviewWindow::maximize(self));
    }
    fn unmaximize(&self) {
        logged("restore", WebviewWindow::unmaximize(self));
    }
    fn content_size(&self) -> (u32, u32) {
        match (self.inner_size(), self.scale_factor()) {
            (Ok(size), Ok(scale)) => {
                let size = size.to_logical::<f64>(scale);
                (size.width.round() as u32, size.height.round() as u32)
            }
            _ => (0, 0),
        }
    }
    fn set_content_size(&self, (width, height): (u32, u32)) {
        logged("resize", self.set_size(LogicalSize::new(f64::from(width), f64::from(height))));
    }
    fn center(&self) {
        logged("center", WebviewWindow::center(self));
    }
    fn screen(&self) -> Option<(f64, f64)> {
        screen_of(self)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    /// A window whose manager takes a while to leave full screen or maximized, and ignores a
    /// size asked for while it is in either (as window managers on Linux do).
    struct SlowWindow {
        fullscreen: Cell<bool>,
        maximized: Cell<bool>,
        /// A state change asked for: (fullscreen, maximized) once `delay` ticks pass.
        pending: Cell<Option<(bool, bool)>>,
        delay: Cell<u32>,
        size: Cell<(u32, u32)>,
        /// The size the window had when it was centered.
        centered: Cell<Option<(u32, u32)>>,
    }

    impl SlowWindow {
        fn new(fullscreen: bool, maximized: bool) -> SlowWindow {
            SlowWindow {
                fullscreen: Cell::new(fullscreen),
                maximized: Cell::new(maximized),
                pending: Cell::new(None),
                delay: Cell::new(0),
                size: Cell::new((2548, 1519)),
                centered: Cell::new(None),
            }
        }

        fn ask(&self, fullscreen: bool, maximized: bool) {
            self.pending.set(Some((fullscreen, maximized)));
            self.delay.set(3);
        }

        fn tick(&self) {
            if let Some((fullscreen, maximized)) = self.pending.get() {
                self.delay.set(self.delay.get().saturating_sub(1));
                if self.delay.get() == 0 {
                    self.fullscreen.set(fullscreen);
                    self.maximized.set(maximized);
                    self.pending.set(None);
                    // Restored: the size from before, as a window manager gives it back.
                    if !fullscreen && !maximized {
                        self.size.set((1200, 740));
                    }
                }
            }
        }
    }

    impl Resizable for SlowWindow {
        fn is_fullscreen(&self) -> bool {
            self.fullscreen.get()
        }
        fn is_maximized(&self) -> bool {
            self.maximized.get()
        }
        fn set_fullscreen(&self, on: bool) {
            if on != self.fullscreen.get() {
                self.ask(on, self.maximized.get());
            }
        }
        fn maximize(&self) {
            self.ask(self.fullscreen.get(), true);
        }
        fn unmaximize(&self) {
            if self.maximized.get() {
                self.ask(self.fullscreen.get(), false);
            }
        }
        fn content_size(&self) -> (u32, u32) {
            self.size.get()
        }
        fn set_content_size(&self, size: (u32, u32)) {
            if !self.fullscreen.get() && !self.maximized.get() && self.pending.get().is_none() {
                self.size.set(size);
            }
        }
        fn center(&self) {
            self.centered.set(Some(self.size.get()));
        }
        fn screen(&self) -> Option<(f64, f64)> {
            Some((2544.0, 1561.0))
        }
    }

    #[test]
    fn a_maximized_window_takes_its_size_once_restored() {
        let w = SlowWindow::new(false, true);
        change_size(&w, WindowSize::Size { width: 1920, height: 1080 }, |_| w.tick());
        assert!(!w.is_maximized());
        assert_eq!(w.content_size(), (1920, 1080));
        assert_eq!(w.centered.get(), Some((1920, 1080)), "centered with its new size");
    }

    #[test]
    fn a_full_screen_window_takes_its_size_once_it_left_full_screen() {
        let w = SlowWindow::new(true, false);
        change_size(&w, WindowSize::Size { width: 1366, height: 800 }, |_| w.tick());
        assert!(!w.is_fullscreen());
        assert_eq!(w.content_size(), (1366, 800));
    }

    #[test]
    fn a_full_screen_window_is_maximized_once_it_left_full_screen() {
        let w = SlowWindow::new(true, false);
        change_size(&w, WindowSize::Maximized, |_| w.tick());
        for _ in 0..5 {
            w.tick();
        }
        assert!(!w.is_fullscreen() && w.is_maximized());
    }

    #[test]
    fn the_taskbar_and_the_frame_stay_free() {
        assert_eq!(available_area((1920.0, 1040.0), (16.0, 39.0)), (1904.0, 1001.0));
        assert_eq!(window_frame((1216, 779), (1200, 740), 1.0), (16.0, 39.0));
        assert_eq!(window_frame((2432, 1558), (2400, 1480), 2.0), (16.0, 39.0));
        assert_eq!(
            window_frame((1200, 740), (1200, 740), 1.0),
            (0.0, 0.0),
            "the launcher draws its own title bar: a window without a frame gives up no room"
        );
        // A 1366×768 laptop with a 40 px taskbar: the 1366×800 preset shrinks to fit.
        let area = available_area((1366.0, 728.0), window_frame((1366, 800), (1366, 800), 1.0));
        assert_eq!(fit_to_screen(1366, 800, Some(area)), (1366, 728));
    }

    #[test]
    fn the_maximize_button_toggles_and_leaves_full_screen() {
        let settle = |w: &SlowWindow| (0..5).for_each(|_| w.tick());
        let w = SlowWindow::new(false, false);
        assert!(toggle_maximized(&w), "a window in its size is maximized");
        settle(&w);
        assert!(w.is_maximized());
        assert!(!toggle_maximized(&w), "a maximized one goes back to its size");
        settle(&w);
        assert!(!w.is_maximized());
        let w = SlowWindow::new(true, false);
        assert!(!toggle_maximized(&w), "full screen is left, not maximized over");
        settle(&w);
        assert!(!w.is_fullscreen() && !w.is_maximized());
    }

    #[test]
    fn sizes_never_exceed_the_screen() {
        assert_eq!(fit_to_screen(1920, 1080, Some((1536.0, 864.0))), (1536, 864));
        assert_eq!(fit_to_screen(1200, 740, Some((1920.0, 1080.0))), (1200, 740));
        assert_eq!(fit_to_screen(1200, 740, None), (1200, 740));
        assert_eq!(fit_to_screen(1200, 740, Some((800.0, 500.0))), (960, 600), "never below the minimum");
    }
}

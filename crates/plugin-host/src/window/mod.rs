//! Host windows for plug-in editors (WP-2.10): a top-level window
//! Beatlayer owns, with the plug-in's own view embedded into it as a child
//! (CLAP's `set_parent`; VST3 and Audio Unit editors will plug into the
//! same layer).
//!
//! Windows builds one as an HWND, at the scale of the monitor it's shown on
//! (per-monitor DPI); Linux and the other X11 platforms build an X11
//! window, which a Wayland session serves through XWayland, as plug-in
//! editors there draw into X11. macOS builds an AppKit utility panel with
//! an `NSView` the editor's view is added to; AppKit makes it only on the
//! main thread, and its sizes are in points (CLAP's `cocoa` API).
//!
//! [`EditorWindow::open`] makes the window, [`EditorWindow::handle`] is the
//! parent the plug-in's view is embedded into, [`EditorWindow::scale`] is
//! the display scale the plug-in is told with `set_scale` (the win32 and
//! x11 APIs measure in physical pixels), and [`EditorWindow::poll`] reports
//! what the person did to the window since the last look, so the shell can
//! tell the plug-in (`set_size`) or close the editor.

#[cfg(target_os = "windows")]
mod win32;
#[cfg(target_os = "windows")]
use win32 as platform;

#[cfg(all(unix, not(target_os = "macos")))]
mod x11;
#[cfg(all(unix, not(target_os = "macos")))]
use x11 as platform;

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
use mac as platform;

pub use clack_extensions::gui::GuiSize;

/// What the person did to a host window since it was last looked at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WindowEvent {
    /// Its client area is now this size, in physical pixels.
    Resized(GuiSize),
    /// The display's scale changed (the window moved to a monitor of
    /// another DPI).
    ScaleChanged(f64),
    /// The person closed the window: the editor closes with it.
    Closed,
}

/// Beatlayer's window for one plug-in's editor.
pub struct EditorWindow(platform::Window);

impl EditorWindow {
    /// Opens a window titled `title`, its client area sized for an editor
    /// that asked for `size` physical pixels, on the monitor the window
    /// lands on (its scale comes from there). `resizable` is whether the
    /// plug-in takes a new size (`can_resize`): a window the plug-in won't
    /// resize is one the person can't either.
    ///
    /// # Errors
    ///
    /// A description of what went wrong, such as no X display to open the
    /// window on.
    pub fn open(title: &str, size: GuiSize, resizable: bool) -> Result<Self, String> {
        platform::Window::open(title, size, resizable).map(Self)
    }

    /// Whether a window can be made on this thread: on macOS only the main
    /// thread makes windows; elsewhere any thread can.
    pub fn can_open() -> bool {
        #[cfg(target_os = "macos")]
        {
            objc2::MainThreadMarker::new().is_some()
        }
        #[cfg(not(target_os = "macos"))]
        {
            true
        }
    }

    /// The parent window the plug-in's view is embedded into
    /// (`set_parent`).
    pub fn handle(&self) -> clack_extensions::gui::Window<'static, '_> {
        self.0.handle()
    }

    /// The window's X ID, which an LV2 UI embeds its view into.
    #[cfg(all(unix, not(target_os = "macos")))]
    pub fn x11_id(&self) -> u32 {
        self.0.id()
    }

    /// The size of the X window `child`, made inside this one by another
    /// connection (an LV2 UI's view), if it exists.
    #[cfg(all(unix, not(target_os = "macos")))]
    pub fn child_size(&self, child: u32) -> Option<GuiSize> {
        self.0.child_size(child)
    }

    /// The display's scale factor: 1.0 at 96 DPI.
    pub fn scale(&self) -> f64 {
        self.0.scale()
    }

    /// Its client size, in physical pixels (points on macOS): what it was
    /// last made, or what the person last left it.
    pub fn size(&self) -> GuiSize {
        self.0.size()
    }

    /// Makes the client area `size` physical pixels.
    pub fn resize(&self, size: GuiSize) {
        self.0.resize(size);
    }

    /// Shows or hides the window, without closing it.
    pub fn set_visible(&self, shown: bool) {
        self.0.set_visible(shown);
    }

    /// What the person did to the window since the last look, one event at
    /// a time; `None` when there's nothing new.
    pub fn poll(&self) -> Option<WindowEvent> {
        self.0.poll()
    }
}

// On macOS a window opens only on the main thread, where the harness's
// tests don't run; `tests/mac_editor.rs` covers it there.
#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    //! The window layer itself, on the platforms that build windows: a
    //! window opens, resizes, and reports a close. On Linux these need an X
    //! display; CI runs them under Xvfb.

    use super::{EditorWindow, GuiSize, WindowEvent};

    fn size(width: u32, height: u32) -> GuiSize {
        GuiSize { width, height }
    }

    #[test]
    fn a_window_opens_resizes_and_closes() {
        let window =
            EditorWindow::open("Test Gain Echo/1-Audio", size(240, 120), true).expect("a window");
        assert!(window.scale() > 0.0, "a scale from the display");
        assert_eq!(window.size(), size(240, 120));
        window.resize(size(320, 200));
        assert_eq!(window.size(), size(320, 200));
        window.set_visible(false);
        window.set_visible(true);
        assert_eq!(window.poll(), None, "nothing the person did");
    }

    #[test]
    fn a_window_the_person_closes_reports_it() {
        let window =
            EditorWindow::open("Test Gain Echo/1-Audio", size(240, 120), true).expect("a window");
        window.0.pretend_closed();
        assert_eq!(window.poll(), Some(WindowEvent::Closed));
    }
}

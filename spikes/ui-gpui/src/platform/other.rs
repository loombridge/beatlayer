//! Windows and Linux stand-ins. The spike measured macOS only; these keep
//! the spike compiling elsewhere and say what's missing.
//!
//! Pointer lock would be `ClipCursor` plus re-centring with `SetCursorPos` (or
//! raw input) on Windows, `XGrabPointer` plus `XWarpPointer` on X11, and the
//! `zwp_pointer_constraints_v1` and `zwp_relative_pointer_v1` protocols on
//! Wayland. Child embedding would be `CreateWindowEx` with `WS_CHILD` under
//! the parent `HWND`, or an X11 child window under the parent XID.

use raw_window_handle::RawWindowHandle;

pub fn lock_pointer() -> bool {
    false
}

pub fn take_delta() -> (f64, f64) {
    (0.0, 0.0)
}

pub fn unlock_pointer() {}

pub fn pointer_locked() -> bool {
    false
}

/// AccessKit publishes UI Automation (Windows) and AT-SPI (Linux) trees;
/// dumping them needs an external client (Inspect.exe, Accerciser).
pub fn dump_accessibility(_handle: RawWindowHandle, _max_nodes: usize) -> String {
    String::from("in-process accessibility dump is macOS-only in this spike\n")
}

pub fn window_number(_handle: RawWindowHandle) -> Option<isize> {
    None
}

pub fn child_view_draws() -> usize {
    0
}

pub fn child_view_clicks() -> usize {
    0
}

pub static PLUGIN_WINDOW: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

pub fn post_click(_window_number: isize, _x: f64, _y: f64) -> bool {
    false
}

pub fn set_ignores_mouse_events(_handle: RawWindowHandle, _ignore: bool) {}

pub fn post_drag_deltas(_deltas: &[(f64, f64)]) -> usize {
    0
}

pub fn embed_child_view(handle: RawWindowHandle, _top_inset: f64) -> Result<String, String> {
    Err(format!(
        "child view embedding isn't implemented on this platform (parent handle {handle:?})"
    ))
}

//! LV2 editors on X11 (the LV2 hosting spec, R13): the plug-in's X11 UI
//! embedded in Beatlayer's X11 host window.

use bl_model::DeviceId;

use super::host::Main;

/// The open editors, by device.
#[derive(Default)]
pub(super) struct Editors {}

/// The editors' main-thread work: idle, port changes both ways, and the
/// person closing a window. Returns whether anything changed.
pub(super) fn pump(_: &mut Main) -> bool {
    false
}

/// Whether the device's editor is open, and hidden.
pub(super) fn state(_: &Main, _: DeviceId) -> (bool, bool) {
    (false, false)
}

/// Closes the device's editor, if it's open.
pub(super) fn close(_: &mut Main, _: DeviceId) {}

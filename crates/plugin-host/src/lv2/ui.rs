//! LV2 editors on X11 (the LV2 hosting spec, R13): the plug-in's X11 UI
//! embedded in Beatlayer's X11 host window.

use bl_model::DeviceId;

use super::Lv2Error;
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

/// Opens the device's editor.
pub(super) fn open(_: &mut Main, _: DeviceId, _: &str) -> Result<(), Lv2Error> {
    Err(Lv2Error::Failed("not yet".to_owned()))
}

/// Shows or hides the device's editor window.
pub(super) fn set_visible(_: &mut Main, _: DeviceId, _: bool) {}

/// The devices whose editor is open.
pub(super) fn open_devices(_: &mut Main) -> Vec<DeviceId> {
    Vec::new()
}

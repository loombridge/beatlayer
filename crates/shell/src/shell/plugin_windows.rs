//! Plug-in windows floating over the main window.

use super::Shell;
use crate::devices;
use bl_model::DeviceId;
use bl_shell_spec::devices as device_ids;

impl Shell {
    /// Every floating device's window, on any track (WP-2.5b), for the
    /// app to open, close, and place.
    #[must_use]
    pub fn floating_windows(&self) -> device_ids::FloatingWindows {
        let doc = self.editor.document();
        let mut windows = self.devices.floating_windows(
            doc,
            devices::shown_track(&self.arrangement, doc),
            self.prefs.auto_hide_plug_in_windows.0,
            self.plug_in_windows,
        );
        // Spec: device-chains R42. A floating device's title bar shows the
        // hand icon too.
        for window in &mut windows.windows {
            window.view.locks = self.device_locks(window.device);
        }
        windows
    }

    /// Whether any device floats, on any track.
    fn any_floating(&self) -> bool {
        let doc = self.editor.document();
        doc.track_order()
            .into_iter()
            .filter_map(|t| doc.track(t))
            .any(|t| t.devices().any(|d| d.floating))
    }

    /// Whether any window View ▸ Plug-In Windows shows and hides: a
    /// floating device's, or a plug-in's editor (R33).
    pub(super) fn any_plug_in_window(&self) -> bool {
        self.any_floating()
            || !self.plugins.open_editors().is_empty()
            || !self.au.open_editors().is_empty()
            || !self.lv2.open_editors().is_empty()
    }

    /// Whether any plug-in editor window is open (IMP-25).
    pub(super) fn any_plug_in_editor(&self) -> bool {
        !self.plugins.open_editors().is_empty()
            || !self.au.open_editors().is_empty()
            || !self.lv2.open_editors().is_empty()
    }

    /// Closes every open plug-in editor window but `except`'s and says how
    /// many it closed (View ▸ Close All Plug-in Windows, IMP-25, with no
    /// exception). It closes editors only, so a floating native device
    /// window stays as it is.
    pub(super) fn close_plug_in_windows(&mut self, except: Option<DeviceId>) -> usize {
        let mut closed = 0;
        for device in self.plugins.open_editors() {
            if Some(device) != except && self.plugins.close_editor(device).is_ok() {
                closed += 1;
            }
        }
        for device in self.au.open_editors() {
            if Some(device) != except && self.au.close_editor(device).is_ok() {
                closed += 1;
            }
        }
        for device in self.lv2.open_editors() {
            if Some(device) != except && self.lv2.close_editor(device).is_ok() {
                closed += 1;
            }
        }
        closed
    }

    /// Opens the windows of plug-ins that Auto-Open Plug-In Windows queued
    /// when they were inserted, as each one starts. A plug-in that failed
    /// or has no editor gets none, and a device the set lost is forgotten.
    pub(super) fn open_queued_plug_in_windows(&mut self) {
        if !self.prefs.auto_open_plug_in_windows.0 {
            self.auto_open.clear();
        }
        for device in std::mem::take(&mut self.auto_open) {
            if self.editor.document().device(device).is_none() {
                continue;
            }
            let Some(info) =
                crate::plugins::info(&self.plugins, &self.au, &self.vst3, &self.lv2, device)
            else {
                self.auto_open.push(device);
                continue;
            };
            if info.failure.is_some() {
                continue;
            }
            if !info.active {
                self.auto_open.push(device);
            } else if info.has_window {
                let _ = self.run_plugins(
                    bl_shell_spec::plugins::SHOW_WINDOW,
                    serde_json::json!({ "device": device, "shown": true }),
                );
            }
        }
    }

    /// Closes the editors of devices the set no longer has. The plug-ins
    /// stay loaded, so undo brings a deleted device back with its state.
    pub(super) fn close_missing_plug_in_windows(&mut self) {
        let doc = self.editor.document();
        let gone = |device| doc.device(device).is_none();
        for device in self.plugins.open_editors().into_iter().filter(|d| gone(*d)) {
            let _ = self.plugins.close_editor(device);
        }
        for device in self.au.open_editors().into_iter().filter(|d| gone(*d)) {
            let _ = self.au.close_editor(device);
        }
        for device in self.lv2.open_editors().into_iter().filter(|d| gone(*d)) {
            let _ = self.lv2.close_editor(device);
        }
    }

    /// Hides and shows the open plug-in editor windows to follow View ▸
    /// Plug-In Windows and Auto-Hide Plug-In Windows, as their tracks'
    /// floating windows do (R31, R33): a hidden editor stays open.
    pub(super) fn apply_plug_in_windows(&mut self) {
        let doc = self.editor.document();
        let shown_track = devices::shown_track(&self.arrangement, doc);
        let auto_hide = self.prefs.auto_hide_plug_in_windows.0;
        let plug_in_windows = self.plug_in_windows;
        let shown =
            |device| plug_in_windows && (!auto_hide || doc.device_track(device) == shown_track);
        for device in self.plugins.open_editors() {
            let _ = self.plugins.set_editor_visible(device, shown(device));
        }
        for device in self.au.open_editors() {
            let _ = self.au.set_editor_visible(device, shown(device));
        }
        for device in self.lv2.open_editors() {
            let _ = self.lv2.set_editor_visible(device, shown(device));
        }
    }

    /// Whether Multiple Plug-In Windows is on: while it's off, opening a
    /// plug-in's window closes the others (WP-6.75).
    #[must_use]
    pub fn multiple_plug_in_windows(&self) -> bool {
        self.prefs.multiple_plug_in_windows.0
    }

    /// Whether Auto-Open Plug-In Windows is on: a plug-in inserted from the
    /// Browser opens its window (WP-6.75).
    #[must_use]
    pub fn auto_open_plug_in_windows(&self) -> bool {
        self.prefs.auto_open_plug_in_windows.0
    }

    /// Whether Auto-Hide Plug-In Windows is on: a floating device's window
    /// or a plug-in's editor hides while another track is selected
    /// (WP-2.5b).
    #[must_use]
    pub fn auto_hide_plug_in_windows(&self) -> bool {
        self.prefs.auto_hide_plug_in_windows.0
    }
}

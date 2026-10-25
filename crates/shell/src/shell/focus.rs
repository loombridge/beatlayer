//! Which window, area and track has the focus.

use super::{Area, Effect, Shell, WindowKind};
use bl_model::TrackId;

use super::{ShellError, to_value};
use crate::commands as ids;
use crate::commands::SwitchResult;
use crate::settings::OnByDefault;
use bl_commands::NoArgs;
use serde_json::Value;

impl Shell {
    /// The track whose control has the keyboard focus, as the window last
    /// reported it, for the neighbour commands (the accessibility spec,
    /// R4).
    #[must_use]
    pub fn focused_track(&self) -> Option<TrackId> {
        self.focused_track
    }

    /// Records the track whose control has the keyboard focus, or none.
    pub fn set_focused_track(&mut self, track: Option<TrackId>) {
        if self.focused_track != track {
            self.focused_track = track;
            self.touch();
        }
    }

    /// The tracks in the order the neighbour commands move through them:
    /// the mixer's columns, then the returns and Main.
    #[must_use]
    pub fn neighbor_order(&self) -> Vec<TrackId> {
        let doc = self.editor.document();
        let mut tracks = self.mixer_columns();
        tracks.extend(doc.returns());
        tracks.push(doc.main_id());
        tracks
    }

    /// Whether the track with the focus (or the selected track) has a
    /// neighbour that way: Navigate's Next Neighbor and Previous Neighbor
    /// are enabled by it, whatever the Tab setting (observed in Live
    /// 12.4.6; the accessibility spec, R4).
    pub(super) fn has_neighbor(&self, backwards: bool) -> bool {
        let order = self.neighbor_order();
        let Some(track) = self
            .focused_track
            .or_else(|| self.arrangement.selected_tracks().first().copied())
        else {
            return false;
        };
        order.iter().position(|t| *t == track).is_some_and(|at| {
            if backwards {
                at > 0
            } else {
                at + 1 < order.len()
            }
        })
    }

    /// The window with the keyboard focus.
    #[must_use]
    pub fn focused_window(&self) -> WindowKind {
        self.focused_window
    }

    /// The main window's area with the keyboard focus.
    #[must_use]
    pub fn focused_area(&self) -> Area {
        self.focused_area
    }

    /// Records which window has the keyboard focus.
    pub fn set_focused_window(&mut self, window: WindowKind) {
        if window == WindowKind::Second && !self.second.open {
            return;
        }
        if matches!(window, WindowKind::Main | WindowKind::Second) {
            self.front_window = window;
        }
        if self.focused_window != window {
            // Spec: automation-editing R23. The edit ends when the focus
            // leaves.
            self.end_key_edit();
            self.focused_window = window;
            if let WindowKind::Device(device) = window
                && self.editor.document().device(device).is_some()
            {
                // A floating device's window in front selects its device,
                // so keys act on it.
                self.devices.select_only(device);
            }
            self.touch();
        }
    }

    /// Records which area of the main window has the keyboard focus, as
    /// clicking in it does (I-FOCUS-001).
    pub fn set_focused_area(&mut self, area: Area) {
        if self.focused_area != area {
            // Spec: automation-editing R23. The edit ends when the focus
            // leaves.
            self.end_key_edit();
            self.focused_area = area;
            // A device's value control lives in the detail area.
            self.value_focused &= area == Area::Detail;
            self.touch();
        }
    }

    /// Records whether a control other than the Arrangement View, an
    /// automation lane or the Envelope Editor has the keyboard focus: while
    /// one does, Enter, Esc, Delete and Tab are its own, not the envelope
    /// keys' (WP-6.26).
    pub fn set_envelope_control_focused(&mut self, focused: bool) {
        if self.envelope_control_focused != focused {
            self.envelope_control_focused = focused;
            self.touch();
        }
    }

    /// Whether a control other than the view or the envelope has the
    /// keyboard focus.
    #[must_use]
    pub fn envelope_control_focused(&self) -> bool {
        self.envelope_control_focused
    }

    /// Records whether a device's value control has the keyboard: while it
    /// does, the Device View's keys give way to the control's own (Delete
    /// sets its default, as S-DEV-006 does in Live), and global ones still
    /// work.
    pub fn set_value_focused(&mut self, focused: bool) {
        if self.value_focused != focused {
            self.value_focused = focused;
            self.touch();
        }
    }

    /// Records the main window's size, kept for the next start.
    pub fn set_window_size(&mut self, width: f32, height: f32) {
        self.prefs.window_size = Some((width, height));
    }

    /// Runs the `focus` commands.
    pub(super) fn run_focus(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            ids::FOCUS_CLIP_VIEW
            | ids::FOCUS_DEVICE_VIEW
            | ids::FOCUS_CONTROL_BAR
            | ids::FOCUS_ARRANGEMENT => self.run_window_view(id, args),
            ids::FOCUS_TUNING => {
                // Spec: tunings R28. The section shows, in the browser,
                // and its name takes the keys.
                Self::args::<NoArgs>(id, args)?;
                self.tuning_section = true;
                self.show_browser_in(self.acting_window());
                self.focus_area(Area::Browser);
                self.effects.push(Effect::FocusTuning);
                self.touch();
                Ok(self.view_result())
            }
            ids::FOCUS_NEXT | ids::FOCUS_PREVIOUS => {
                Self::args::<NoArgs>(id, args)?;
                self.effects.push(Effect::MoveFocus {
                    backwards: id == ids::FOCUS_PREVIOUS,
                    neighbor: false,
                });
                Ok(Value::Null)
            }
            // Spec: accessibility R4.
            ids::FOCUS_NEXT_NEIGHBOR | ids::FOCUS_PREVIOUS_NEIGHBOR => {
                Self::args::<NoArgs>(id, args)?;
                self.effects.push(Effect::MoveFocus {
                    backwards: id == ids::FOCUS_PREVIOUS_NEIGHBOR,
                    neighbor: true,
                });
                Ok(Value::Null)
            }
            // Spec: accessibility R5.
            ids::FOCUS_TOGGLE_WRAP_NAVIGATION => {
                Self::args::<NoArgs>(id, args)?;
                self.prefs.wrap_navigation = OnByDefault(!self.prefs.wrap_navigation.0);
                self.save_preferences();
                Ok(to_value(&SwitchResult {
                    on: self.prefs.wrap_navigation.0,
                }))
            }
            ids::FOCUS_TOGGLE_TAB_NAVIGATION => {
                Self::args::<NoArgs>(id, args)?;
                self.prefs.tab_navigation = !self.prefs.tab_navigation;
                self.save_preferences();
                Ok(to_value(&SwitchResult {
                    on: self.prefs.tab_navigation,
                }))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }
}

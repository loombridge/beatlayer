//! The small view options through the shell (WP-6.49; the view options spec,
//! R2 to R15): note colours, the mark on empty tracks and the mouse wheel on
//! controls. All three are application preferences, so none is saved with
//! the set or is an undo step.

use bl_commands::NoArgs;
use bl_shell_spec::commands::{SwitchArgs, SwitchResult};
use bl_shell_spec::view_options::{
    self as ids, EmptyTracks, MarkEmptyTracksArgs, NoteColors, NoteColorsArgs, NoteColorsResult,
};
use serde_json::Value;

use super::{Shell, ShellError, to_value};

impl Shell {
    /// How the MIDI Note Editor colours notes (R2).
    #[must_use]
    pub fn note_colors(&self) -> NoteColors {
        self.prefs.note_colors
    }

    /// Whether empty tracks show a mark (R6).
    #[must_use]
    pub fn mark_empty_tracks(&self) -> bool {
        self.prefs.mark_empty_tracks
    }

    /// Whether the mouse wheel moves controls (R10).
    #[must_use]
    pub fn wheel_adjusts_values(&self) -> bool {
        self.prefs.wheel_adjusts_values
    }

    /// Whether a title bar shows the empty-track mark for `track` now (R8).
    #[must_use]
    pub fn shows_empty_mark(&self, track: bl_model::TrackId) -> bool {
        self.prefs.mark_empty_tracks && self.editor.document().is_track_empty(track)
    }

    /// Runs the view options' commands.
    pub(super) fn run_view_options(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            ids::SET_NOTE_COLORS => {
                let NoteColorsArgs { colors } = Self::args(id, args)?;
                self.prefs.note_colors = colors;
                self.save_preferences();
                self.touch();
                Ok(to_value(&NoteColorsResult { colors }))
            }
            ids::SET_MARK_EMPTY_TRACKS => {
                let MarkEmptyTracksArgs { on } = Self::args(id, args)?;
                let on = on.unwrap_or(!self.prefs.mark_empty_tracks);
                self.prefs.mark_empty_tracks = on;
                self.save_preferences();
                self.touch();
                Ok(to_value(&SwitchResult { on }))
            }
            ids::SET_WHEEL_ADJUSTS_VALUES => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.wheel_adjusts_values = on;
                self.save_preferences();
                self.touch();
                Ok(to_value(&SwitchResult { on }))
            }
            _ => {
                Self::args::<NoArgs>(id, args)?;
                Ok(to_value(&EmptyTracks {
                    tracks: self.editor.document().empty_tracks(),
                }))
            }
        }
    }
}

//! The small view options (WP-6.49; the view options spec, R2 to R16): note
//! colours, the mark on empty tracks and the mouse wheel on controls.
//!
//! [`register`] adds their specs, which are pure data; `bl-shell` runs them
//! by ID.

use bl_commands::Context::{Global, Settings};
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_model::TrackId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::commands::{SwitchArgs, SwitchResult};

/// `view.set_note_colors`: choose how the MIDI Note Editor colours notes.
pub const SET_NOTE_COLORS: &str = "view.set_note_colors";
/// `view.set_mark_empty_tracks`: switch the mark on empty tracks.
pub const SET_MARK_EMPTY_TRACKS: &str = "view.set_mark_empty_tracks";
/// `settings.set_wheel_adjusts_values`: switch Adjust Values with Mouse Wheel.
pub const SET_WHEEL_ADJUSTS_VALUES: &str = "settings.set_wheel_adjusts_values";
/// `track.list_empty`: list the empty tracks.
pub const LIST_EMPTY: &str = "track.list_empty";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[
    SET_NOTE_COLORS,
    SET_MARK_EMPTY_TRACKS,
    SET_WHEEL_ADJUSTS_VALUES,
    LIST_EMPTY,
];

/// Whether `id` is one of these commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// How the MIDI Note Editor colours notes (R2, R3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NoteColors {
    /// The clip's colour, paler the softer the note: Live's.
    #[default]
    Velocity,
    /// One hue for each pitch class, the same in every octave.
    PitchClass,
    /// One hue for each degree of the clip's scale, grey outside it.
    ScaleDegree,
}

/// Arguments of `view.set_note_colors`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoteColorsArgs {
    /// The colouring to use.
    pub colors: NoteColors,
}

/// What `view.set_note_colors` returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NoteColorsResult {
    /// The colouring in use afterwards.
    pub colors: NoteColors,
}

/// Arguments of `view.set_mark_empty_tracks`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MarkEmptyTracksArgs {
    /// Whether the mark is on. Left out, the switch flips, as the menu item
    /// does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
}

/// What `track.list_empty` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EmptyTracks {
    /// The empty tracks, in display order.
    pub tracks: Vec<TrackId>,
}

/// Adds the commands to a registry.
///
/// # Errors
///
/// Fails if one of the IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(SET_NOTE_COLORS, "Note Colors")
            .description(
                "Chooses how the MIDI Note Editor colours notes (View \u{25b8} Note Colors): \
                 `velocity` (the clip's colour, paler the softer the note, as Live does; the \
                 default), `pitch_class` (one hue for each of C to B, the same in every octave) \
                 or `scale_degree` (one hue for each degree of the clip's scale, the root first, \
                 and grey for a note outside the scale; a clip whose Scale Mode is off colours \
                 by velocity). Every choice keeps the velocity's paleness, grey deactivated \
                 notes and the selection outline. It is an application setting, kept between \
                 launches; it isn't part of the set and isn't an undo step. `view.state` reads \
                 it in `note_colors`.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<NoteColorsArgs>()
            .result::<NoteColorsResult>(),
    )?;
    registry.register(
        CommandSpec::new(SET_MARK_EMPTY_TRACKS, "Mark Empty Tracks")
            .description(
                "Switches Mark Empty Tracks (View \u{25b8} Mark Empty Tracks): while it's on, \
                 every empty track, one with no clip and no device, shows a hollow ring at the \
                 right end of its title bar in the Arrangement, the Session View and both \
                 mixers. It's off in a new install and kept between launches; it isn't part of \
                 the set and isn't an undo step. `view.state` reads it in `mark_empty_tracks`; \
                 `track.list_empty` lists the empty tracks.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<MarkEmptyTracksArgs>()
            .result::<SwitchResult>(),
    )?;
    registry.register(
        CommandSpec::new(SET_WHEEL_ADJUSTS_VALUES, "Adjust Values with Mouse Wheel")
            .description(
                "Switches Adjust Values with Mouse Wheel, on the Settings window's Display & \
                 Input page: while it's on, the mouse wheel over a knob, slider, fader or number \
                 box moves it a step a notch (Shift: a fine step), a trackpad's scroll a step \
                 for every 20 points, and a run of moves is one undo step; while it's off, as \
                 in a new install and as in Live, the wheel only scrolls and zooms. The setting \
                 is kept between launches; it isn't part of the set and isn't an undo step. \
                 `settings.state` reads it in `display_input.wheel_adjusts_values`.",
            )
            .permission(Permission::Edit)
            .context(Settings)
            .args::<SwitchArgs>()
            .result::<SwitchResult>(),
    )?;
    registry.register(
        CommandSpec::new(LIST_EMPTY, "List Empty Tracks")
            .description(
                "Lists the set's empty tracks, whether or not Mark Empty Tracks shows them: a \
                 track with no clip (on its lanes, take lanes or Session slots) and no device, \
                 a group with no device whose tracks are all empty, or a return track with no \
                 device. Main is never listed. Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<EmptyTracks>(),
    )?;
    Ok(())
}

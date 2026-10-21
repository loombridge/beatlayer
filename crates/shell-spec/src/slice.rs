//! Slice to New MIDI Track (WP-4.16): the command that cuts an audio clip
//! into slices on a new MIDI track, and the slicing dialog's commands.
//!
//! [`register`] adds their specs; `bl-shell` runs them. The behaviour they
//! follow is in the slice-to-midi spec, R1 to R14.
//!
//! A person reaches the command from the Create menu (M-CREATE-011) and an
//! audio clip's context menu (C-CLIP-ARR-027), which run [`OPEN`]: its
//! dialog holds the Slice By chooser, the Slicing Preset chooser, and
//! Preserve warped timing, and its OK runs [`SLICE`]. An agent calls
//! [`SLICE`] directly with the arguments the dialog would gather.

use bl_commands::Context::{Arrangement, ClipView, Global, Session};
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_core::time::{Ticks, TimeSignature};
use bl_model::{ClipId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::arrangement::FixedGrid;

/// `clip.slice_to_midi_track`: cut an audio clip into slices on a new MIDI
/// track (the slice-to-midi spec, R13).
pub const SLICE: &str = "clip.slice_to_midi_track";
/// `clip.open_slice_dialog`: open the slicing dialog (R1).
pub const OPEN: &str = "clip.open_slice_dialog";
/// `clip.close_slice_dialog`: close the slicing dialog.
pub const CLOSE: &str = "clip.close_slice_dialog";

/// The undo step's name.
pub const STEP: &str = "Slice to New MIDI Track";

/// The factory preset that makes a Drum Rack with one sampler per slice
/// (R7, R9): the default.
pub const DRUM_RACK_PRESET: &str = "Drum Rack";
/// The factory preset that makes one sampler in Slice mode (R10).
pub const ONE_SAMPLER_PRESET: &str = "One Sampler";
/// Beatlayer's factory slicing presets, in the order the dialog lists them:
/// the default first.
pub const FACTORY_PRESETS: [&str; 2] = [DRUM_RACK_PRESET, ONE_SAMPLER_PRESET];

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[SLICE, OPEN, CLOSE];

/// Whether `id` is one of the slice-to-midi commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// How the dialog's Slice By chooser cuts the clip (R2, R4): a beat
/// division, the clip's transients, or its warp markers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SliceBy {
    /// A slice every 8 bars.
    EightBars,
    /// A slice every 4 bars.
    FourBars,
    /// A slice every 2 bars.
    TwoBars,
    /// A slice every bar.
    OneBar,
    /// A slice every half note.
    Half,
    /// A slice every quarter note.
    Quarter,
    /// A slice every eighth note.
    Eighth,
    /// A slice every sixteenth note: the dialog's default (R2).
    Sixteenth,
    /// A slice every thirty-second note.
    ThirtySecond,
    /// A slice at each of the clip's file's transients inside the region.
    Transients,
    /// A slice at each of the clip's warp markers inside the region.
    WarpMarkers,
}

impl SliceBy {
    /// The beat divisions, in the dialog's order: the Arrangement's Fixed
    /// Grid choices (R2, S5).
    pub const DIVISIONS: [Self; 9] = [
        Self::EightBars,
        Self::FourBars,
        Self::TwoBars,
        Self::OneBar,
        Self::Half,
        Self::Quarter,
        Self::Eighth,
        Self::Sixteenth,
        Self::ThirtySecond,
    ];

    /// Every choice, in the dialog's order: the divisions, then Transients,
    /// then Warp Markers (R2).
    pub const ALL: [Self; 11] = [
        Self::EightBars,
        Self::FourBars,
        Self::TwoBars,
        Self::OneBar,
        Self::Half,
        Self::Quarter,
        Self::Eighth,
        Self::Sixteenth,
        Self::ThirtySecond,
        Self::Transients,
        Self::WarpMarkers,
    ];

    /// How the dialog lists it (R2).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::EightBars => "8 Bars",
            Self::FourBars => "4 Bars",
            Self::TwoBars => "2 Bars",
            Self::OneBar => "1 Bar",
            Self::Half => "1/2",
            Self::Quarter => "1/4",
            Self::Eighth => "1/8",
            Self::Sixteenth => "1/16",
            Self::ThirtySecond => "1/32",
            Self::Transients => "Transients",
            Self::WarpMarkers => "Warp Markers",
        }
    }

    /// The Fixed Grid spacing a division names, if it is one.
    #[must_use]
    pub const fn grid(self) -> Option<FixedGrid> {
        Some(match self {
            Self::EightBars => FixedGrid::EightBars,
            Self::FourBars => FixedGrid::FourBars,
            Self::TwoBars => FixedGrid::TwoBars,
            Self::OneBar => FixedGrid::OneBar,
            Self::Half => FixedGrid::Half,
            Self::Quarter => FixedGrid::Quarter,
            Self::Eighth => FixedGrid::Eighth,
            Self::Sixteenth => FixedGrid::Sixteenth,
            Self::ThirtySecond => FixedGrid::ThirtySecond,
            Self::Transients | Self::WarpMarkers => return None,
        })
    }

    /// How long one division is under `signature`: whole bars for a bar
    /// spacing, the note's own length otherwise (R4). `None` for Transients
    /// and Warp Markers.
    #[must_use]
    pub fn ticks(self, signature: TimeSignature) -> Option<Ticks> {
        let grid = self.grid()?;
        Some(match (grid.bars(), grid.note()) {
            (Some(bars), _) => Ticks(signature.bar_length().get().saturating_mul(i64::from(bars))),
            (None, Some(note)) => Ticks(Ticks::WHOLE.get() / i64::from(note)),
            (None, None) => unreachable!("every fixed grid spacing is bars or a note"),
        })
    }
}

/// Arguments of `clip.slice_to_midi_track` (R13): the clip, how to cut it,
/// the slicing preset, and whether the slices keep the clip's warped
/// timing. Every one left out takes the dialog's default, so `{}` cuts the
/// selected clip on the sixteenth note with the default preset, keeping its
/// warped timing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SliceArgs {
    /// The audio clip to slice. Left out, the one audio clip selected in
    /// either view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// How to cut it: a beat division, `transients`, or `warp_markers`.
    /// Left out, `sixteenth` (R2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<SliceBy>,
    /// The slicing preset's name: one of Beatlayer's factory presets or a
    /// user preset in the User Library's `Defaults/Slicing/` folder (R11).
    /// Left out, the default preset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// Whether the slices are cut from the clip's warped audio, keeping the
    /// timing warping gave them. Left out, `true` (R2, R6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preserve_warped_timing: Option<bool>,
}

/// What `clip.slice_to_midi_track` made (R13).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SliceResult {
    /// The new MIDI track, directly after the clip's track (R7).
    pub track: TrackId,
    /// The MIDI clip on it, at the audio clip's place (R9).
    pub clip: ClipId,
    /// How many slices it made.
    pub slices: usize,
}

/// Arguments of `clip.open_slice_dialog` (R1).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenArgs {
    /// The audio clip to slice. Left out, the one audio clip selected in
    /// either view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
}

/// The slicing dialog's state, as `clip.open_slice_dialog` returns it and
/// the window draws it (R1, R2).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SliceDialogStatus {
    /// Whether the dialog is open.
    pub open: bool,
    /// The clip it would slice, if one is chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The Slicing Preset chooser's entries, in order: Beatlayer's factory
    /// presets, then the user's presets in `Defaults/Slicing/` (R11).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub presets: Vec<String>,
}

/// Adds the slice-to-midi commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(SLICE, STEP)
            .description(
                "Cuts an audio clip into slices on a new MIDI track. The clip is sliced over \
                 its region: from its Start marker to its End marker, or, with the loop on, \
                 from the earlier of its Start marker and its loop's start to the loop's end. \
                 `by` is a beat division (`eight_bars`, `four_bars`, `two_bars`, `one_bar`, \
                 `half`, `quarter`, `eighth`, `sixteenth` — the default — or `thirty_second`), \
                 `transients` (one slice at each of the clip's transients in the region), or \
                 `warp_markers` (one at each of its warp markers; refused for an unwarped \
                 clip). With `preserve_warped_timing` on (the default) the slices are cut from \
                 the clip's warped audio, so each sounds as it did and its note falls where it \
                 played; off, they are cut from the clip's own file, unwarped. `preset` names \
                 a slicing preset: Beatlayer's factory presets (the default makes a Drum Rack \
                 of one sampler per slice, each chain receiving a note from C1 up and playing \
                 C3; `One Sampler` makes one sampler in Slice mode) or a user preset in the \
                 User Library's Defaults/Slicing folder. The new MIDI track goes directly \
                 after the clip's track, takes the clip's name, and holds a MIDI clip with one \
                 note per slice, rising chromatically, each lasting to the next, at velocity \
                 127. More than 128 slices are refused (64 for `One Sampler`). Slicing leaves \
                 the audio clip as it was and is one undo step.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Arrangement, Session, ClipView])
            .args::<SliceArgs>()
            .result::<SliceResult>(),
    )?;
    registry.register(
        CommandSpec::new(OPEN, "Slice to New MIDI Track")
            .description(
                "Opens the Slice to New MIDI Track dialog for an audio clip (`clip`, or the \
                 one audio clip selected in either view). The dialog holds the Slice By \
                 chooser (the beat divisions, Transients, and Warp Markers), the Slicing \
                 Preset chooser, and Preserve warped timing, and its OK runs \
                 `clip.slice_to_midi_track`. Changes nothing in the set: an agent calls \
                 `clip.slice_to_midi_track` instead.",
            )
            .permission(Permission::Read)
            .contexts([Global, Arrangement, Session, ClipView])
            .args::<OpenArgs>()
            .result::<SliceDialogStatus>(),
    )?;
    registry.register(
        CommandSpec::new(CLOSE, "Close Slice to New MIDI Track")
            .description("Closes the Slice to New MIDI Track dialog. Changes nothing in the set.")
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<SliceDialogStatus>(),
    )
}

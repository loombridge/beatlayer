//! The data of Find and Select Notes (WP-6.4): the eight note filters of
//! the MIDI Note Editor's filter bar, their settings and defaults, and the
//! arguments of `clip_view.note_filters`.
//!
//! The filters belong to the editor, not to the set (Spec:
//! find-and-select-notes R12). `bl-shell` keeps them and works out which
//! notes they let through.

use bl_model::ScaleName;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A note filter, in the order the Filter chooser lists them (Spec:
/// find-and-select-notes R2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "A note filter: `pitch`, `time`, `chance`, `condition`, `count`, `duration`, \
                   `scale` or `velocity`."
)]
pub enum NoteFilter {
    /// Pitch classes, in every octave.
    #[default]
    Pitch,
    /// A window of beats, optionally repeating.
    Time,
    /// A range of note chance.
    Chance,
    /// Whether a note is active, has a chance, or has a velocity deviation.
    Condition,
    /// Every nth note, chord or grid step.
    Count,
    /// A range of note lengths.
    Duration,
    /// Notes in a scale.
    Scale,
    /// A range of velocities.
    Velocity,
}

impl NoteFilter {
    /// The filters in the order the chooser lists them.
    pub const ALL: [Self; 8] = [
        Self::Pitch,
        Self::Time,
        Self::Chance,
        Self::Condition,
        Self::Count,
        Self::Duration,
        Self::Scale,
        Self::Velocity,
    ];

    /// The name the chooser shows.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pitch => "Pitch",
            Self::Time => "Time",
            Self::Chance => "Chance",
            Self::Condition => "Condition",
            Self::Count => "Count",
            Self::Duration => "Duration",
            Self::Scale => "Scale",
            Self::Velocity => "Velocity",
        }
    }
}

/// The Pitch filter: pitch classes C to B.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PitchFilter {
    /// Whether the filter is on.
    pub on: bool,
    /// Whether it lets through the notes it would hold back.
    pub invert: bool,
    /// The pitch classes that are switched on, 0 (C) to 11 (B), in order.
    pub classes: Vec<u8>,
}

/// The Time filter, in beats of the clip's time signature.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TimeFilter {
    /// Whether the filter is on.
    pub on: bool,
    /// Whether it lets through the notes it would hold back.
    pub invert: bool,
    /// Where the window starts, in beats from 1, the clip's 1.1.1.
    pub start: f64,
    /// How long the window is, in beats; above 0.
    pub length: f64,
    /// How far later the window repeats, in beats; 0 for no repeat. `null`
    /// is one bar of the clip's time signature.
    pub repeat: Option<f64>,
}

impl Default for TimeFilter {
    fn default() -> Self {
        Self {
            on: false,
            invert: false,
            start: 1.0,
            length: 1.0,
            repeat: None,
        }
    }
}

/// The Chance filter, in percent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChanceFilter {
    /// Whether the filter is on.
    pub on: bool,
    /// Whether it lets through the notes it would hold back.
    pub invert: bool,
    /// The lowest chance let through, 0 to 100 percent.
    pub min: f64,
    /// The highest chance let through, 0 to 100 percent.
    pub max: f64,
}

impl Default for ChanceFilter {
    fn default() -> Self {
        Self {
            on: false,
            invert: false,
            min: 0.0,
            max: 100.0,
        }
    }
}

/// The Condition filter: a note must pass each switch that is on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConditionFilter {
    /// Whether the filter is on.
    pub on: bool,
    /// Whether it lets through the notes it would hold back.
    pub invert: bool,
    /// Let through the notes that aren't deactivated.
    pub active: bool,
    /// Let through the notes whose chance is below 100 percent.
    pub chance: bool,
    /// Let through the notes whose velocity deviation isn't 0.
    pub velocity_deviation: bool,
}

/// The Count filter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CountFilter {
    /// Whether the filter is on.
    pub on: bool,
    /// Whether it lets through the notes it would hold back.
    pub invert: bool,
    /// Let through each nth note, chord or step, 1 to 16.
    pub every: u8,
    /// Where counting starts, 0 to one less than `every`.
    pub offset: u8,
    /// Count the editing grid's steps instead of the notes.
    pub quantized: bool,
}

impl Default for CountFilter {
    fn default() -> Self {
        Self {
            on: false,
            invert: false,
            every: 2,
            offset: 0,
            quantized: false,
        }
    }
}

/// The Duration filter, in beats of the clip's time signature.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DurationFilter {
    /// Whether the filter is on.
    pub on: bool,
    /// Whether it lets through the notes it would hold back.
    pub invert: bool,
    /// The shortest note let through, in beats.
    pub min: f64,
    /// The longest note let through, in beats. `null` is the length of the
    /// clip's span.
    pub max: Option<f64>,
}

/// The Scale filter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScaleFilter {
    /// Whether the filter is on.
    pub on: bool,
    /// Whether it lets through the notes it would hold back.
    pub invert: bool,
    /// Use the clip's scale, whether or not its Scale Mode is on.
    pub use_clip_scale: bool,
    /// The filter's own root, 0 (C) to 11 (B), while `use_clip_scale` is off.
    pub root: u8,
    /// The filter's own scale, while `use_clip_scale` is off.
    pub name: ScaleName,
}

impl Default for ScaleFilter {
    fn default() -> Self {
        Self {
            on: false,
            invert: false,
            use_clip_scale: true,
            root: 0,
            name: ScaleName::Major,
        }
    }
}

/// The Velocity filter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VelocityFilter {
    /// Whether the filter is on.
    pub on: bool,
    /// Whether it lets through the notes it would hold back.
    pub invert: bool,
    /// The lowest velocity let through, 1 to 127.
    pub min: u8,
    /// The highest velocity let through, 1 to 127.
    pub max: u8,
}

impl Default for VelocityFilter {
    fn default() -> Self {
        Self {
            on: false,
            invert: false,
            min: 1,
            max: 127,
        }
    }
}

/// Find and Select Notes, as the editor holds it and `view.state` reports
/// it: whether the bar shows, the filter it shows, and every filter's
/// settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NoteFilters {
    /// Whether the filter bar shows above the editor's ruler.
    pub visible: bool,
    /// The filter whose controls the bar shows.
    pub shown: NoteFilter,
    /// The Pitch filter.
    pub pitch: PitchFilter,
    /// The Time filter.
    pub time: TimeFilter,
    /// The Chance filter.
    pub chance: ChanceFilter,
    /// The Condition filter.
    pub condition: ConditionFilter,
    /// The Count filter.
    pub count: CountFilter,
    /// The Duration filter.
    pub duration: DurationFilter,
    /// The Scale filter.
    pub scale: ScaleFilter,
    /// The Velocity filter.
    pub velocity: VelocityFilter,
}

impl NoteFilters {
    /// Whether `filter` is on.
    #[must_use]
    pub fn is_on(&self, filter: NoteFilter) -> bool {
        match filter {
            NoteFilter::Pitch => self.pitch.on,
            NoteFilter::Time => self.time.on,
            NoteFilter::Chance => self.chance.on,
            NoteFilter::Condition => self.condition.on,
            NoteFilter::Count => self.count.on,
            NoteFilter::Duration => self.duration.on,
            NoteFilter::Scale => self.scale.on,
            NoteFilter::Velocity => self.velocity.on,
        }
    }

    /// Switches `filter` on or off, keeping its settings.
    pub fn set_on(&mut self, filter: NoteFilter, on: bool) {
        match filter {
            NoteFilter::Pitch => self.pitch.on = on,
            NoteFilter::Time => self.time.on = on,
            NoteFilter::Chance => self.chance.on = on,
            NoteFilter::Condition => self.condition.on = on,
            NoteFilter::Count => self.count.on = on,
            NoteFilter::Duration => self.duration.on = on,
            NoteFilter::Scale => self.scale.on = on,
            NoteFilter::Velocity => self.velocity.on = on,
        }
    }

    /// Whether any filter is on.
    #[must_use]
    pub fn any_on(&self) -> bool {
        NoteFilter::ALL.into_iter().any(|filter| self.is_on(filter))
    }
}

/// Reads a field that may be given as `null`, which is different from leaving
/// it out: `null` is `Some(None)`.
fn given<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}

/// The Pitch filter's controls in `clip_view.note_filters`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PitchArgs {
    /// Switches the filter's Invert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invert: Option<bool>,
    /// The pitch classes switched on, 0 (C) to 11 (B).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classes: Option<Vec<u8>>,
}

/// The Time filter's controls in `clip_view.note_filters`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimeArgs {
    /// Switches the filter's Invert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invert: Option<bool>,
    /// Start, in beats from 1; hundredths of a beat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    /// Length, in beats; above 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<f64>,
    /// Repeat, in beats; 0 for none, `null` for one bar of the clip's time
    /// signature.
    #[serde(
        default,
        deserialize_with = "given",
        skip_serializing_if = "Option::is_none"
    )]
    pub repeat: Option<Option<f64>>,
}

/// The Chance filter's controls in `clip_view.note_filters`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChanceArgs {
    /// Switches the filter's Invert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invert: Option<bool>,
    /// Min, 0 to 100 percent. Moving it past Max takes Max with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// Max, 0 to 100 percent. Moving it past Min takes Min with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

/// The Condition filter's controls in `clip_view.note_filters`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConditionArgs {
    /// Switches the filter's Invert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invert: Option<bool>,
    /// The Active switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    /// The Chance switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chance: Option<bool>,
    /// The Velocity Deviation switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity_deviation: Option<bool>,
}

/// The Count filter's controls in `clip_view.note_filters`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CountArgs {
    /// Switches the filter's Invert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invert: Option<bool>,
    /// Every, 1 to 16.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<u8>,
    /// Offset, 0 to one less than Every.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u8>,
    /// The Quantized switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantized: Option<bool>,
}

/// The Duration filter's controls in `clip_view.note_filters`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DurationArgs {
    /// Switches the filter's Invert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invert: Option<bool>,
    /// Min, in beats. Moving it past Max takes Max with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// Max, in beats; `null` for the length of the clip's span. Moving it
    /// past Min takes Min with it.
    #[serde(
        default,
        deserialize_with = "given",
        skip_serializing_if = "Option::is_none"
    )]
    pub max: Option<Option<f64>>,
}

/// The Scale filter's controls in `clip_view.note_filters`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScaleArgs {
    /// Switches the filter's Invert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invert: Option<bool>,
    /// The Use Clip Scale switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_clip_scale: Option<bool>,
    /// The Scale Root chooser, 0 (C) to 11 (B).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<u8>,
    /// The Scale Name chooser.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<ScaleName>,
}

/// The Velocity filter's controls in `clip_view.note_filters`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VelocityArgs {
    /// Switches the filter's Invert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invert: Option<bool>,
    /// Min, 1 to 127. Moving it past Max takes Max with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<u8>,
    /// Max, 1 to 127. Moving it past Min takes Min with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<u8>,
}

/// Arguments of `clip_view.note_filters`. Fields left out change nothing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoteFiltersArgs {
    /// The MIDI clip. Left out, the one the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<bl_model::ClipId>,
    /// Shows or hides the filter bar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    /// Switches the bar, as Cmd-Shift-F does; `visible` wins if both are given.
    #[serde(default)]
    pub toggle: bool,
    /// Chooses the filter whose controls the bar shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<NoteFilter>,
    /// Pitch controls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch: Option<PitchArgs>,
    /// Time controls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<TimeArgs>,
    /// Chance controls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chance: Option<ChanceArgs>,
    /// Condition controls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<ConditionArgs>,
    /// Count controls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<CountArgs>,
    /// Duration controls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<DurationArgs>,
    /// Scale controls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<ScaleArgs>,
    /// Velocity controls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity: Option<VelocityArgs>,
    /// Filters whose Select button is pressed: each is switched on, and the
    /// notes are selected again.
    #[serde(default)]
    pub select: Vec<NoteFilter>,
    /// Filters to switch off, keeping their settings.
    #[serde(default)]
    pub off: Vec<NoteFilter>,
}

/// What `clip_view.note_filters` returns: the filters and the selection.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NoteFiltersResult {
    /// The filters' settings, with which are on.
    pub filters: NoteFilters,
    /// The selected notes, by their place in the clip's notes in order.
    pub selected: Vec<u32>,
}

/// Arguments of `clip_view.invert_selection`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InvertSelectionArgs {
    /// The MIDI clip. Left out, the one the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<bl_model::ClipId>,
}

//! The MIDI Tools' commands (WP-6.11): the Transform and Generate panels,
//! choosing a tool, its settings, applying it, and the seed.
//!
//! Every action the Clip View's tool panels offer is one of these, run by
//! the panels' controls and by agents alike; a control's drag is a gesture
//! of `midi_tool.set`. A tool is named by its ID, its name in lower case
//! with words joined by underscores (`arpeggiate`). The behaviour is in the
//! MIDI Tools spec.
//!
//! [`register`] adds their specs, which are pure data; `bl-shell` runs them
//! by ID.

use bl_commands::Context::{ClipView, NoteEditor, SampleEditor};
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use bl_model::ClipId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// `midi_tool.list`: every tool with its parameters.
pub const LIST: &str = "midi_tool.list";
/// `midi_tool.state`: the panels, the chosen tools, their settings, the
/// target and the open application.
pub const STATE: &str = "midi_tool.state";
/// `clip_view.show_panel`: show a panel of the Clip View's right group.
pub const SHOW_PANEL: &str = "clip_view.show_panel";
/// `midi_tool.close`: close the open application.
pub const CLOSE: &str = "midi_tool.close";
/// `midi_tool.choose`: choose the tool a track's panel shows.
pub const CHOOSE: &str = "midi_tool.choose";
/// `midi_tool.set`: set parameters of a tool.
pub const SET: &str = "midi_tool.set";
/// `midi_tool.set_auto`: switch a tool's Auto switch.
pub const SET_AUTO: &str = "midi_tool.set_auto";
/// `midi_tool.reset`: put a tool's parameters back to their defaults.
pub const RESET: &str = "midi_tool.reset";
/// `midi_tool.apply`: apply a tool once.
pub const APPLY: &str = "midi_tool.apply";
/// `midi_tool.reseed`: give the open application a new seed and run it
/// again.
pub const RESEED: &str = "midi_tool.reseed";

/// `midi_tool.randomize`: set every voice's Rotation of Euclidean at random.
pub const RANDOMIZE: &str = "midi_tool.randomize";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[
    LIST, STATE, SHOW_PANEL, CLOSE, CHOOSE, SET, SET_AUTO, RESET, APPLY, RESEED, RANDOMIZE,
];

/// Whether `id` is one of the MIDI Tools' commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// A panel of the Clip View's right group.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "A panel of the Clip View's right group: `utilities` (Pitch and Time \
                   Utilities on a MIDI clip, Audio Utilities on an audio clip), `transform` \
                   (the MIDI Transformation Tools) or `generate` (the MIDI Generative Tools; \
                   a MIDI clip's only)."
)]
pub enum Panel {
    /// Pitch and Time Utilities, or Audio Utilities.
    #[default]
    Utilities,
    /// The Transform panel.
    Transform,
    /// The Generate panel.
    Generate,
}

/// Whether a tool changes notes or writes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    /// Takes notes and changes them.
    Transformation,
    /// Writes new notes.
    Generator,
}

/// One parameter of a tool, as `midi_tool.list` gives it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ParamInfo {
    /// The parameter's ID, which `midi_tool.set` names it by.
    pub id: String,
    /// The name the panel shows.
    pub name: String,
    /// The kind of value: `number`, `whole`, `choice`, `switch`, `pitch`,
    /// `pitch_range`, `division`, `breakpoints`, `levels`, `toggles` or
    /// `structured`.
    pub kind: String,
    /// The least value, of a number, a whole number or a display's data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// The greatest value, of a number, a whole number or a display's data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// How many decimals a number shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decimals: Option<u8>,
    /// A choice's options by ID, a note division's names, or how many levels
    /// or toggles a display has.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// The unit the value is shown with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Whether it counts scale degrees where the clip has a scale.
    pub scale_aware: bool,
    /// The value a new set starts with, in the form `midi_tool.set` takes.
    pub default: Value,
    /// What the parameter does.
    pub description: String,
}

/// One tool, as `midi_tool.list` gives it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolInfo {
    /// The tool's ID.
    pub id: String,
    /// The tool's name.
    pub name: String,
    /// Whether it changes notes or writes them.
    pub kind: ToolKind,
    /// Its parameters, in the order the panel shows them.
    pub parameters: Vec<ParamInfo>,
}

/// What `midi_tool.list` returns: the tools in the order the selectors list
/// them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolList {
    /// Every transformation, then every generator.
    pub tools: Vec<ToolInfo>,
}

/// One tool's settings, as `midi_tool.state` gives them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolSettingsState {
    /// The tool's ID.
    pub tool: String,
    /// Whether it changes notes or writes them.
    pub kind: ToolKind,
    /// Its Auto switch.
    pub auto: bool,
    /// Every parameter's value by ID.
    pub values: Map<String, Value>,
    /// Whether every parameter is at its default, so Reset is disabled.
    pub at_defaults: bool,
}

/// What a tool would act on now.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TargetState {
    /// The range: from its start to its end, in ticks of the clip's
    /// contents.
    pub range: (Ticks, Ticks),
    /// The note editor's time selection, if it has one.
    pub time_selection: Option<(Ticks, Ticks)>,
    /// The selected notes' places in the clip's notes in order.
    pub selected: Vec<u32>,
    /// The places of the notes a transformation would take as input.
    pub input: Vec<u32>,
}

/// One step of the open chain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StepState {
    /// The tool's ID.
    pub tool: String,
    /// Every parameter's value by ID, as the step ran with them.
    pub values: Map<String, Value>,
    /// The seed the step's random choices come from.
    pub seed: u32,
}

/// The open application, which is a chain of one or more tools.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ApplicationState {
    /// The chain's last tool.
    pub tool: String,
    /// The clip.
    pub clip: ClipId,
    /// How many input notes it keeps.
    pub input_notes: usize,
    /// The range it keeps.
    pub range: (Ticks, Ticks),
    /// The seed of the chain's last tool.
    pub seed: u32,
    /// The chain: each step's tool, settings and seed, in order.
    #[serde(default)]
    pub chain: Vec<StepState>,
}

/// What the last application wrote.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolRun {
    /// The tool.
    pub tool: String,
    /// The seed.
    pub seed: u32,
    /// How many notes the tool wrote.
    pub wrote: usize,
    /// The written notes' places in the clip's notes in order: the notes
    /// selected afterwards.
    pub notes: Vec<u32>,
    /// Whether the application recorded an undo step.
    pub undo_step: bool,
}

/// What `midi_tool.state` returns, and what the commands that change a
/// tool's choice, settings or application return.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MidiToolState {
    /// The right group's shown panel.
    pub panel: Panel,
    /// The clip the state is for.
    pub clip: Option<ClipId>,
    /// The ID of the tool the Transform and Generate panels' Cmd+Enter,
    /// Edit menu item and `tool`-less commands act on: the chosen tool of
    /// the one of them that showed last.
    pub current_tool: Option<String>,
    /// The ID of the transformation the clip's track shows.
    pub transformation: String,
    /// The ID of the generator the clip's track shows.
    pub generator: String,
    /// Every tool's settings.
    pub tools: Vec<ToolSettingsState>,
    /// What a tool would act on now; none without a MIDI clip.
    pub target: Option<TargetState>,
    /// The open application, if there is one.
    pub application: Option<ApplicationState>,
    /// What the command that returned this state applied, if it applied a
    /// tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<ToolRun>,
}

/// A range of the clip's contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RangeArgs {
    /// Where it starts, in ticks of the clip's contents.
    pub start: Ticks,
    /// Where it ends.
    pub end: Ticks,
}

/// Arguments of `midi_tool.state` and `midi_tool.close`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClipArgs {
    /// The MIDI clip. Left out, the one the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
}

/// Arguments of `clip_view.show_panel`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowPanelArgs {
    /// The panel to show.
    pub panel: Panel,
}

/// Arguments of `midi_tool.choose`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChooseArgs {
    /// The MIDI clip whose track's panel shows the tool. Left out, the one
    /// the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The tool's ID, from `midi_tool.list`.
    pub tool: String,
}

/// Arguments of `midi_tool.set`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetArgs {
    /// The MIDI clip. Left out, the one the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The tool's ID. Left out, the chosen tool of the panel shown last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Which step of the open chain to change, counting from 0, when the tool
    /// is in it more than once. Left out, the tool's last step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<usize>,
    /// The parameters to set, by ID, each in the form `midi_tool.list` gives
    /// its default.
    pub values: Map<String, Value>,
}

/// Arguments of `midi_tool.set_auto`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetAutoArgs {
    /// The MIDI clip. Left out, the one the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The tool's ID. Left out, the chosen tool of the panel shown last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Whether Auto is on.
    pub auto: bool,
}

/// Arguments of `midi_tool.reset`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResetArgs {
    /// The MIDI clip. Left out, the one the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The tool's ID. Left out, the chosen tool of the panel shown last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
}

/// Arguments of `midi_tool.apply`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyArgs {
    /// The MIDI clip. Left out, the one the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The tool's ID. Left out, the chosen tool of the panel shown last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Parameters to set before applying, by ID. They replace the tool's
    /// settings for those parameters and are kept as its settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Map<String, Value>>,
    /// The notes a transformation takes as input, by their place in the
    /// clip's notes in order. Left out, the Clip View's selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
    /// The range to act on. Left out, the note editor's time selection or the
    /// clip's loop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<RangeArgs>,
    /// The seed, from 0 to 4294967295. Left out, a new one is drawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,
}

/// Arguments of `midi_tool.reseed`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReseedArgs {
    /// The MIDI clip. Left out, the one the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The seed, from 0 to 4294967295. Left out, a new one is drawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,
}

/// Arguments of `midi_tool.randomize`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RandomizeArgs {
    /// The MIDI clip. Left out, the one the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The seed, from 0 to 4294967295. Left out, a new one is drawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,
}

/// Adds the MIDI Tools' commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    for spec in specs() {
        registry.register(spec)?;
    }
    Ok(())
}

fn tool(
    id: &'static str,
    label: &'static str,
    permission: Permission,
    text: &'static str,
) -> CommandSpec {
    CommandSpec::new(id, label)
        .description(text)
        .permission(permission)
        .contexts([NoteEditor, ClipView])
        .result::<MidiToolState>()
}

fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(LIST, "List MIDI Tools")
            .description(
                "Lists the MIDI Tools: every transformation, then every generator, in the \
                 order the Transformation and Generator Selectors list them. Each has its ID, \
                 name and kind, and its parameters: ID, name, kind (number, whole, choice, \
                 switch, pitch, pitch_range, division, breakpoints, levels, toggles or \
                 structured), range, default, unit, whether it counts scale degrees, and \
                 what it does. Changes nothing.",
            )
            .permission(Permission::Read)
            .contexts([NoteEditor, ClipView])
            .args::<NoArgs>()
            .result::<ToolList>(),
        tool(
            STATE,
            "MIDI Tool State",
            Permission::Read,
            "Reports the MIDI Tools: the Clip View's right group's shown panel, the \
             transformation and generator the clip's track shows, every tool's settings (its \
             Auto switch, its parameters' values, whether they are all at their defaults), \
             what a tool would act on now (the range, the time selection, the selected notes \
             and the notes a transformation would take), and the open application, a chain of \
             one or more tools, with how many input notes it keeps, its range, and each \
             step's tool, settings and seed in order. Changes nothing.",
        )
        .args::<ClipArgs>(),
        tool(
            SHOW_PANEL,
            "Show Clip View Panel",
            Permission::Read,
            "Shows a panel of the Clip View's right group: `utilities` (Pitch and Time \
             Utilities, or Audio Utilities on an audio clip), `transform` or `generate`, as \
             clicking its button in the panel selector row does. View state: it is no undo \
             step and is not saved with the set.",
        )
        .args::<ShowPanelArgs>(),
        tool(
            CLOSE,
            "Close MIDI Tool Application",
            Permission::Read,
            "Closes the open MIDI Tool application or chain, so the next change of a tool's \
             parameters starts a new one on the notes as they are then. Leaves the notes \
             as they are. View state: no undo step.",
        )
        .args::<ClipArgs>(),
        tool(
            CHOOSE,
            "Choose MIDI Tool",
            Permission::Edit,
            "Chooses the tool the Transform or Generate panel shows for the track of a MIDI \
             clip, as the Transformation or Generator Selector does: a transformation or a \
             generator, by its ID from `midi_tool.list`. Each track remembers its choice, \
             saved with the set. A tool that is in the open chain shows the settings of its \
             last step. Changes no notes and is no undo step; read it back in \
             `midi_tool.state`.",
        )
        .args::<ChooseArgs>(),
        tool(
            SET,
            "Set MIDI Tool Parameters",
            Permission::Edit,
            "Sets one or more parameters of a MIDI Tool, by ID. The settings are one set \
             for the whole set, saved with it, and changing them is no undo step. With the \
             tool's Auto switch on, the change applies the tool at once (a transformation \
             only while notes or a time selection are selected): the first change opens an \
             application on the notes as they are, and each further change runs the tool \
             again on the notes it started from, with the same seed. A tool that is in the \
             open chain runs the chain again from its step (its last, or the one `step` \
             names, counting from 0), each later step with its own settings and seed; \
             another transformation joins the chain. Each change is one undo step named \
             after the tool. A change that would leave the clip fewer notes asks first. Read \
             the settings back in `midi_tool.state` and the notes in the query tool's \
             `clip`.",
        )
        .args::<SetArgs>(),
        tool(
            SET_AUTO,
            "Set MIDI Tool Auto",
            Permission::Edit,
            "Switches a MIDI Tool's Auto switch. Switching it off while the tool is in the \
             open chain takes its last step out, as one undo step: the steps after it run \
             again on the notes the step before it made, and taking out the only step puts \
             the notes back as they were and closes the chain. Switching it on applies \
             nothing until a parameter changes. Read it back in `midi_tool.state`.",
        )
        .args::<SetAutoArgs>(),
        tool(
            RESET,
            "Reset MIDI Tool",
            Permission::Edit,
            "Puts every parameter of a MIDI Tool back to its default and leaves its Auto \
             switch as it is. Changes no notes and is no undo step. Closes the open \
             application. Read it back in `midi_tool.state`.",
        )
        .args::<ResetArgs>(),
        tool(
            APPLY,
            "Apply MIDI Tool",
            Permission::Edit,
            "Applies a MIDI Tool once, with its settings, whether Auto is on or off. A \
             transformation replaces its input (the given `notes`, the selected notes, the \
             notes that start in the time selection, or the notes that start in the clip's \
             loop); a generator writes notes into the range, replacing the notes at the \
             same key that overlap them. `values` replace the tool's settings for those \
             parameters and are kept. `seed` (0 to 4294967295) makes the random choices \
             repeatable; left out, a new one is drawn. The notes written are selected \
             afterwards. One undo step named after the tool, even when the notes come out \
             unchanged. A transformation applied to the notes the open chain wrote (no \
             `notes` or `range` given) joins the chain as a new step; a generator, or notes \
             or a range of its own, starts a new chain. Cmd+Enter (S-NOTE-033) and Edit ▸ \
             Apply Transformation/Generator run it. On an audio clip, whose Transform panel \
             holds Quantize alone, it runs `clip.quantize_audio` with Quantize's Grid, \
             Triplets and Amount. A transformation with no input is refused, and so is a run \
             that would write more than 65536 notes. A call that would leave the clip fewer \
             notes asks first. Read the notes in the query tool's `clip` (`data.midi.notes`).",
        )
        .contexts([NoteEditor, ClipView, SampleEditor])
        .binding("S-NOTE-033")
        .args::<ApplyArgs>(),
        tool(
            RESEED,
            "Reseed MIDI Tool",
            Permission::Edit,
            "Gives the last step of the open MIDI Tool chain a new seed, or the one it is \
             given, and runs it again on the notes the step before it made, as a tool's \
             Reseed button does. Refused while no application is open. A call that would \
             leave the clip fewer notes asks first. Read the seed in `midi_tool.state`.",
        )
        .args::<ReseedArgs>(),
        tool(
            RANDOMIZE,
            "Randomize Euclidean Rotations",
            Permission::Edit,
            "Sets the Rotation of every voice of Euclidean to a random value from 0 to Steps - \
             1, as the Pattern tab's randomise button does. `seed` (0 to 4294967295) makes the \
             draw repeatable; left out, a new one is drawn. With Euclidean's Auto switch on it \
             applies at once, like `midi_tool.set`. Changes no notes by itself and is no undo \
             step; read the Rotations back in `midi_tool.state`.",
        )
        .args::<RandomizeArgs>(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_commands_register_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        assert!(registry.problems().is_empty(), "{:?}", registry.problems());
        assert!(handles(APPLY) && !handles("note.add"));
    }
}

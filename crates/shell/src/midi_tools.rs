//! The MIDI Tools in the Clip View (WP-6.11): the Transform and Generate
//! panels, the tool each track shows, every tool's settings, and applying a
//! tool to a clip's notes.
//!
//! A tool is a description and a pure function of its input
//! (`bl-midi-tools`); this module is what surrounds it: what it acts on
//! (R10 to R12), merging its notes into the clip and selecting them
//! (R12 to R15), the open application with its kept input and seed (R17 to
//! R21), which is a chain of tools (R27 to R33), and one undo step named
//! after the tool (R22). The choices and
//! settings sit beside the document like the folds, so they are never an
//! undo step (R4, R5). The rules are in the MIDI Tools spec.

use std::collections::BTreeMap;

use bl_core::time::Ticks;
use bl_midi_tools::{
    Breakpoint, ChordBank, DIVISION_NAMES, MAX_BANK_BYTES, MidiTool, ParamKind, ParamSpec,
    ParamValue, QUANTIZE_GRIDS, Registry, Rng, ToolDescriptor, ToolError, ToolInput, ToolKind,
    ToolNote, ToolPoint, ToolScale, add_chord, delete_chord, entry_keys, load_bank,
    random_rotations, rotation_id, stack_view,
};
use bl_model::{
    Breakpoint as NotePoint, Clip, ClipData, ClipId, Document, Editor, MidiData, Note, RackKind,
    Receive, TrackId,
};
use bl_project::midi_tools::{ChosenTools, MidiToolSettings, SavedTool};
use bl_shell_spec::midi_tools::{
    self as ids, ApplicationState, ApplyArgs, BankEntryState, BankState, ChooseArgs, ChordState,
    ClipArgs, LoadBankArgs, MidiToolState, Panel, ParamInfo, RandomizeArgs, RangeArgs, ReseedArgs,
    ResetArgs, SetArgs, SetAutoArgs, ShowPanelArgs, StacksState, StepState, TargetState, ToolInfo,
    ToolList, ToolRun, ToolSettingsState,
};
use bl_shell_spec::notes::{QuantizeSettings, QuantizeTarget};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use crate::arrangement::ArrangementView;
use crate::arrangement::editing::{EditFailure, transaction};
use crate::clip_view::{ClipView, notes_in_order, span_of};

/// The transformation a new track shows (the MIDI Tools spec, R4).
const DEFAULT_TRANSFORMATION: &str = "arpeggiate";
/// The generator a new track shows.
const DEFAULT_GENERATOR: &str = "rhythm";
/// The generator whose randomise button `midi_tool.randomize` presses (the
/// MIDI generators spec, R16).
const EUCLIDEAN: &str = "euclidean";
/// The generator whose progression and bank the Stacks commands change (the
/// MIDI generators spec, R17, R26).
const STACKS: &str = "stacks";
/// The Quantize tool, whose settings are the Clip View's Quantize settings
/// (the MIDI transformations spec, R20), and its parameters' places.
const QUANTIZE: &str = "quantize";
const START: usize = 0;
const END: usize = 1;
const GRID: usize = 2;
const TRIPLETS: usize = 3;
const AMOUNT: usize = 4;

fn invalid(message: impl Into<String>) -> EditFailure {
    EditFailure::InvalidArgs(message.into())
}

fn unavailable(reason: impl Into<String>) -> EditFailure {
    EditFailure::Unavailable(reason.into())
}

fn refused(reason: impl Into<String>) -> EditFailure {
    EditFailure::Refused(reason.into())
}

fn parse<T: DeserializeOwned>(args: Value) -> Result<T, EditFailure> {
    let args = if args.is_null() { json!({}) } else { args };
    serde_json::from_value(args).map_err(|error| invalid(error.to_string()))
}

fn to_value<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn failure(error: ToolError) -> EditFailure {
    match error {
        ToolError::Values(message) => invalid(message),
        ToolError::Input(message) => refused(message),
        error @ ToolError::TooManyNotes(_) => refused(error.to_string()),
    }
}

/// One tool's settings: its Auto switch and a value for every parameter.
#[derive(Clone, Debug, PartialEq)]
struct Settings {
    auto: bool,
    values: Vec<ParamValue>,
}

impl Settings {
    fn new(descriptor: &ToolDescriptor) -> Self {
        Self {
            auto: true,
            values: descriptor.defaults(),
        }
    }

    fn at_defaults(&self, descriptor: &ToolDescriptor) -> bool {
        self.values == descriptor.defaults()
    }
}

/// One tool of the open chain (the MIDI Tools spec, R27): its settings and
/// seed, the clip's notes as they were before it ran, and which of them it
/// took as input. A step after the first takes the notes the step before
/// wrote.
#[derive(Clone, Debug, PartialEq)]
struct Step {
    tool: String,
    values: Vec<ParamValue>,
    seed: u32,
    /// The clip's notes in the editor's order before the tool ran.
    base: Vec<Note>,
    /// Which of `base` a transformation took as input.
    input: Vec<usize>,
}

/// The open application, which is a chain (the MIDI Tools spec, R17, R27):
/// the steps with the range they all keep, and what the last one left,
/// which says whether the chain is still the one that last changed the clip
/// (R21).
#[derive(Clone, Debug, PartialEq)]
struct Application {
    clip: ClipId,
    /// The clip the Clip View showed when it began.
    shown: Option<ClipId>,
    steps: Vec<Step>,
    range: (Ticks, Ticks),
    /// The selection and time selection before the first tool ran, which
    /// taking out the only step puts back (R18, R31).
    before: (Vec<u32>, Option<(Ticks, Ticks)>),
    /// The clip's notes, the selection and the time selection the chain
    /// left.
    after: Vec<Note>,
    selected: Vec<u32>,
    time: Option<(Ticks, Ticks)>,
}

impl Application {
    /// The place of the last step of `tool`, or the step `step` names when
    /// it is a step of `tool` (R28).
    fn step_of(&self, tool: &str, step: Option<usize>) -> Result<Option<usize>, EditFailure> {
        match step {
            Some(at) => match self.steps.get(at) {
                Some(found) if found.tool == tool => Ok(Some(at)),
                Some(found) => Err(invalid(format!(
                    "step {at} of the open chain is `{}`, not `{tool}`",
                    found.tool
                ))),
                None => Err(invalid(format!(
                    "the open chain has no step {at}; it has {}",
                    self.steps.len()
                ))),
            },
            None => Ok(self.steps.iter().rposition(|found| found.tool == tool)),
        }
    }
}

/// The tool each track shows, every tool's settings, the Clip View's shown
/// panel and the open application.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MidiTools {
    /// The right group's shown panel: view state (R1).
    panel: Panel,
    /// Whether the Generate panel showed after the Transform panel did.
    generator_last: bool,
    chosen: BTreeMap<TrackId, ChosenIds>,
    settings: BTreeMap<String, Settings>,
    open: Option<Application>,
    /// How many seeds have been drawn, which makes the next.
    draws: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ChosenIds {
    transformation: Option<String>,
    generator: Option<String>,
}

/// Everything a panel draws about one tool.
#[derive(Clone)]
pub struct ToolView<'a> {
    /// The tool's description.
    pub descriptor: &'a ToolDescriptor,
    /// Its parameters' values.
    pub values: Vec<ParamValue>,
    /// Its Auto switch.
    pub auto: bool,
    /// Whether every parameter is at its default, so Reset is disabled.
    pub at_defaults: bool,
}

impl MidiTools {
    /// The right group's shown panel.
    #[must_use]
    pub fn panel(&self) -> Panel {
        self.panel
    }

    /// Shows a panel of the right group (R1). Showing the Transform or the
    /// Generate panel makes it the one Cmd+Enter applies from (R19).
    pub fn show_panel(&mut self, panel: Panel) {
        self.panel = panel;
        match panel {
            Panel::Transform => self.generator_last = false,
            Panel::Generate => self.generator_last = true,
            Panel::Utilities => {}
        }
    }

    /// The kind of the tool Cmd+Enter applies: the one whose panel showed
    /// last (R19).
    #[must_use]
    pub fn current_kind(&self) -> ToolKind {
        if self.generator_last {
            ToolKind::Generator
        } else {
            ToolKind::Transformation
        }
    }

    /// The tool a track shows in the panel of `kind`: its choice, or what a
    /// new track shows, or else the first of the kind (R4).
    #[must_use]
    pub fn chosen(&self, track: Option<TrackId>, kind: ToolKind) -> Option<&'static dyn MidiTool> {
        let registry = Registry::native();
        let saved = track.and_then(|track| self.chosen.get(&track));
        let (saved, default) = match kind {
            ToolKind::Transformation => (
                saved.and_then(|c| c.transformation.as_deref()),
                DEFAULT_TRANSFORMATION,
            ),
            ToolKind::Generator => (
                saved.and_then(|c| c.generator.as_deref()),
                DEFAULT_GENERATOR,
            ),
        };
        saved
            .into_iter()
            .chain([default])
            .find_map(|id| registry.get(kind, id))
            .or_else(|| registry.tools(kind).next())
    }

    /// The ID of the tool a track has chosen in the panel of `kind`, or the
    /// one a new track shows, whether or not the registry has it yet.
    fn chosen_id(&self, track: Option<TrackId>, kind: ToolKind) -> String {
        let saved = track.and_then(|track| self.chosen.get(&track));
        match kind {
            ToolKind::Transformation => saved
                .and_then(|c| c.transformation.as_deref())
                .unwrap_or(DEFAULT_TRANSFORMATION),
            ToolKind::Generator => saved
                .and_then(|c| c.generator.as_deref())
                .unwrap_or(DEFAULT_GENERATOR),
        }
        .to_owned()
    }

    /// What a panel draws about `tool`.
    #[must_use]
    pub fn view_of<'a>(&self, tool: &'a dyn MidiTool) -> ToolView<'a> {
        let descriptor = tool.descriptor();
        let settings = self.settings_of(descriptor);
        ToolView {
            descriptor,
            at_defaults: settings.at_defaults(descriptor),
            auto: settings.auto,
            values: settings.values,
        }
    }

    /// A tool's settings, which are its defaults until they are changed.
    fn settings_of(&self, descriptor: &ToolDescriptor) -> Settings {
        self.settings
            .get(&descriptor.id)
            .cloned()
            .unwrap_or_else(|| Settings::new(descriptor))
    }

    /// Closes the open application (R21).
    pub fn close(&mut self) {
        self.open = None;
    }

    /// The Quantize tool's settings (the MIDI transformations spec, R18).
    fn quantize_settings(&self) -> Vec<ParamValue> {
        let tool = Registry::native()
            .find(QUANTIZE)
            .expect("Quantize is a native tool");
        self.settings_of(tool.descriptor()).values
    }

    /// Whether the Quantize tool moves starts and whether it moves ends.
    #[must_use]
    pub fn quantize_moves(&self) -> (bool, bool) {
        let values = self.quantize_settings();
        (
            values[START] == ParamValue::Switch(true),
            values[END] == ParamValue::Switch(true),
        )
    }

    /// The Quantize tool's settings as the Clip View's Quantize settings
    /// (R20). Start and End both off read as note starts, which
    /// [`Self::quantize_moves`] tells apart.
    #[must_use]
    pub fn quantize(&self) -> QuantizeSettings {
        let values = self.quantize_settings();
        let (start, end) = self.quantize_moves();
        let ParamValue::Choice(grid) = values[GRID] else {
            return QuantizeSettings::default();
        };
        QuantizeSettings {
            grid: grid
                .checked_sub(1)
                .and_then(|at| QUANTIZE_GRIDS.get(at))
                .map(|n| Ticks(Ticks::WHOLE.get() / n)),
            target: match (start, end) {
                (true, true) => QuantizeTarget::Both,
                (false, true) => QuantizeTarget::NoteEnd,
                _ => QuantizeTarget::NoteStart,
            },
            triplets: values[TRIPLETS] == ParamValue::Switch(true),
            amount: match values[AMOUNT] {
                ParamValue::Whole(n) => u8::try_from(n.clamp(0, 100)).unwrap_or(100),
                _ => 100,
            },
        }
    }

    /// Changes the Quantize tool's settings: the fields given, keeping the
    /// rest. A `grid` is a note division in ticks (R18).
    ///
    /// # Errors
    ///
    /// [`EditFailure::InvalidArgs`] for a grid that isn't one.
    pub fn set_quantize(
        &mut self,
        grid: Option<Ticks>,
        target: Option<QuantizeTarget>,
        triplets: Option<bool>,
        amount: Option<u8>,
    ) -> Result<(), EditFailure> {
        let tool = Registry::native()
            .find(QUANTIZE)
            .expect("Quantize is a native tool");
        let descriptor = tool.descriptor();
        let mut settings = self.settings_of(descriptor);
        if let Some(grid) = grid {
            let at = QUANTIZE_GRIDS
                .iter()
                .position(|n| Ticks::WHOLE.get() / n == grid.get())
                .ok_or_else(|| {
                    invalid(format!(
                        "`grid` must be a whole note divided by 1, 2, 4, 8, 16 or 32, \
                         in ticks; {grid} isn't"
                    ))
                })?;
            settings.values[GRID] = ParamValue::Choice(at + 1);
        }
        if let Some(target) = target {
            let (start, end) = match target {
                QuantizeTarget::NoteStart => (true, false),
                QuantizeTarget::NoteEnd => (false, true),
                QuantizeTarget::Both => (true, true),
            };
            settings.values[START] = ParamValue::Switch(start);
            settings.values[END] = ParamValue::Switch(end);
        }
        if let Some(triplets) = triplets {
            settings.values[TRIPLETS] = ParamValue::Switch(triplets);
        }
        if let Some(amount) = amount {
            settings.values[AMOUNT] = ParamValue::Whole(i64::from(amount));
        }
        self.settings.insert(descriptor.id.clone(), settings);
        Ok(())
    }

    /// Shows the Transform panel with Quantize chosen for `track` (R21).
    pub fn show_quantize(&mut self, track: Option<TrackId>) {
        if let Some(track) = track {
            self.chosen.entry(track).or_default().transformation = Some(QUANTIZE.to_owned());
        }
        self.show_panel(Panel::Transform);
    }

    /// The settings as the set saves them (R4, R5): the choices of tracks,
    /// and each tool that isn't at its defaults with Auto on.
    #[must_use]
    pub fn saved(&self) -> MidiToolSettings {
        let registry = Registry::native();
        let tools = self
            .settings
            .iter()
            .filter_map(|(id, settings)| {
                let descriptor = registry.find(id)?.descriptor();
                if settings.auto && settings.at_defaults(descriptor) {
                    return None;
                }
                let values = descriptor
                    .parameters
                    .iter()
                    .zip(&settings.values)
                    .map(|(spec, value)| (spec.id.clone(), value_to_json(spec, value)))
                    .collect();
                Some((
                    id.clone(),
                    SavedTool {
                        auto: settings.auto,
                        values,
                    },
                ))
            })
            .collect();
        MidiToolSettings {
            chosen: self
                .chosen
                .iter()
                .map(|(track, c)| {
                    (
                        *track,
                        ChosenTools {
                            transformation: c.transformation.clone(),
                            generator: c.generator.clone(),
                        },
                    )
                })
                .collect(),
            tools,
        }
    }

    /// Restores settings a set saved, keeping the panel shown. A value
    /// outside its parameter's range, or for a parameter the tool doesn't
    /// have, loads as the default (R5).
    pub fn restore(&mut self, saved: &MidiToolSettings) {
        let registry = Registry::native();
        self.open = None;
        self.chosen = saved
            .chosen
            .iter()
            .map(|(track, c)| {
                (
                    *track,
                    ChosenIds {
                        transformation: c.transformation.clone(),
                        generator: c.generator.clone(),
                    },
                )
            })
            .collect();
        self.settings = saved
            .tools
            .iter()
            .filter_map(|(id, saved)| {
                let tool = registry.find(id)?;
                let descriptor = tool.descriptor();
                let values: Vec<ParamValue> = descriptor
                    .parameters
                    .iter()
                    .map(|spec| {
                        saved
                            .values
                            .get(&spec.id)
                            .and_then(|json| value_from_json(spec, json).ok())
                            .unwrap_or_else(|| spec.default.clone())
                    })
                    .collect();
                // Values that don't suit together load as the defaults too.
                let values = if tool.validate(&values).is_ok() {
                    values
                } else {
                    descriptor.defaults()
                };
                Some((
                    id.clone(),
                    Settings {
                        auto: saved.auto,
                        values,
                    },
                ))
            })
            .collect();
    }

    /// The next seed: one a new application takes when it is given none
    /// (R20).
    fn draw_seed(&mut self) -> u32 {
        self.draws += 1;
        let mut rng = Rng::new(u32::try_from(self.draws % (1 << 32)).unwrap_or(0));
        (rng.next_u64() >> 32) as u32
    }

    /// The open application, if it is still the one that last changed
    /// `clip`: nothing has changed its notes, the selection or the time
    /// selection since, and the clip still shows (R21).
    fn open_for(
        &self,
        doc: &Document,
        view: &ClipView,
        arrangement: &ArrangementView,
        clip: ClipId,
    ) -> Option<&Application> {
        let app = self.open.as_ref().filter(|app| app.clip == clip)?;
        let Some(ClipData::Midi(midi)) = doc.clip(clip).map(|clip| &clip.data) else {
            return None;
        };
        (view.shown_clip(doc, arrangement) == app.shown
            && notes_in_order(midi) == app.after
            && view.selected_notes(doc, arrangement) == app.selected
            && view.time_selection(doc, arrangement) == app.time)
            .then_some(app)
    }

    /// The IDs of the open chain's tools in order, once each: empty while no
    /// chain is open (R29).
    #[must_use]
    pub fn chain_tools(
        &self,
        doc: &Document,
        view: &ClipView,
        arrangement: &ArrangementView,
    ) -> Vec<String> {
        let Some(app) = self
            .open
            .as_ref()
            .and_then(|app| self.open_for(doc, view, arrangement, app.clip))
        else {
            return Vec::new();
        };
        let mut tools: Vec<String> = Vec::new();
        for step in &app.steps {
            if !tools.contains(&step.tool) {
                tools.push(step.tool.clone());
            }
        }
        tools
    }

    /// The open chain's tools by name, in order, separated by " › " (R30).
    #[must_use]
    pub fn chain_status(
        &self,
        doc: &Document,
        view: &ClipView,
        arrangement: &ArrangementView,
    ) -> Option<String> {
        let registry = Registry::native();
        let app = self
            .open
            .as_ref()
            .and_then(|app| self.open_for(doc, view, arrangement, app.clip))?;
        let names: Vec<&str> = app
            .steps
            .iter()
            .filter_map(|step| registry.find(&step.tool))
            .map(|tool| tool.descriptor().name.as_str())
            .collect();
        (!names.is_empty()).then(|| names.join(" › "))
    }
}

// The values in the commands' form, and the form the set saves them in.

/// A value as JSON, in the form `midi_tool.set` takes.
fn value_to_json(spec: &ParamSpec, value: &ParamValue) -> Value {
    match (&spec.kind, value) {
        (ParamKind::Choice { options }, ParamValue::Choice(n)) => {
            json!(options.get(*n).map(|option| option.id.as_str()))
        }
        (_, ParamValue::Number(n)) => json!(n),
        (_, ParamValue::Whole(n)) => json!(n),
        (_, ParamValue::Choice(n)) => json!(n),
        (_, ParamValue::Switch(on)) => json!(on),
        (_, ParamValue::Pitch(key)) => json!(key),
        (_, ParamValue::PitchRange { low, high }) => json!({ "low": low, "high": high }),
        (_, ParamValue::Division(n)) => json!(DIVISION_NAMES.get(*n)),
        (_, ParamValue::Breakpoints(points)) => {
            json!(
                points
                    .iter()
                    .map(|p| json!({ "x": p.x, "y": p.y }))
                    .collect::<Vec<_>>()
            )
        }
        (_, ParamValue::Levels(levels)) => json!(levels),
        (_, ParamValue::Toggles(toggles)) => json!(toggles),
        // The JSON the text holds, so an agent reads and sets Stacks'
        // progression as a list; text that isn't JSON stays text.
        (_, ParamValue::Structured(text)) => {
            serde_json::from_str(text).unwrap_or_else(|_| json!(text))
        }
    }
}

/// A value from JSON, checked against its parameter.
fn value_from_json(spec: &ParamSpec, json: &Value) -> Result<ParamValue, String> {
    let wrong = || format!("`{}` doesn't take {json}", spec.id);
    let value = match &spec.kind {
        ParamKind::Number { .. } => ParamValue::Number(json.as_f64().ok_or_else(wrong)?),
        ParamKind::Whole { .. } => ParamValue::Whole(
            json.as_i64()
                .or_else(|| json.as_f64().filter(|n| n.fract() == 0.0).map(|n| n as i64))
                .ok_or_else(wrong)?,
        ),
        ParamKind::Choice { options } => {
            let id = json.as_str().ok_or_else(wrong)?;
            ParamValue::Choice(options.iter().position(|o| o.id == id).ok_or_else(|| {
                let ids: Vec<&str> = options.iter().map(|o| o.id.as_str()).collect();
                format!("`{}` takes one of {}", spec.id, ids.join(", "))
            })?)
        }
        ParamKind::Switch => ParamValue::Switch(json.as_bool().ok_or_else(wrong)?),
        ParamKind::Pitch => ParamValue::Pitch(
            json.as_u64()
                .and_then(|n| u8::try_from(n).ok())
                .ok_or_else(wrong)?,
        ),
        ParamKind::PitchRange => {
            let key = |name: &str| {
                json.get(name)
                    .and_then(Value::as_u64)
                    .and_then(|n| u8::try_from(n).ok())
            };
            ParamValue::PitchRange {
                low: key("low").ok_or_else(wrong)?,
                high: key("high").ok_or_else(wrong)?,
            }
        }
        ParamKind::Division => {
            let name = json.as_str().ok_or_else(wrong)?;
            ParamValue::Division(DIVISION_NAMES.iter().position(|n| *n == name).ok_or_else(
                || format!("`{}` takes one of {}", spec.id, DIVISION_NAMES.join(", ")),
            )?)
        }
        ParamKind::Breakpoints { .. } => ParamValue::Breakpoints(
            json.as_array()
                .ok_or_else(wrong)?
                .iter()
                .map(|point| {
                    Some(Breakpoint {
                        x: point.get("x")?.as_f64()?,
                        y: point.get("y")?.as_f64()?,
                    })
                })
                .collect::<Option<Vec<_>>>()
                .ok_or_else(wrong)?,
        ),
        ParamKind::Levels { .. } => ParamValue::Levels(
            json.as_array()
                .ok_or_else(wrong)?
                .iter()
                .map(Value::as_f64)
                .collect::<Option<Vec<_>>>()
                .ok_or_else(wrong)?,
        ),
        ParamKind::Toggles { .. } => ParamValue::Toggles(
            json.as_array()
                .ok_or_else(wrong)?
                .iter()
                .map(Value::as_bool)
                .collect::<Option<Vec<_>>>()
                .ok_or_else(wrong)?,
        ),
        ParamKind::Structured => ParamValue::Structured(match json {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        }),
    };
    spec.check(&value)?;
    Ok(value)
}

fn param_info(spec: &ParamSpec) -> ParamInfo {
    let (kind, min, max, decimals, options) = match &spec.kind {
        ParamKind::Number { min, max, decimals } => (
            "number",
            Some(*min),
            Some(*max),
            Some(*decimals),
            Vec::new(),
        ),
        ParamKind::Whole { min, max } => (
            "whole",
            Some(*min as f64),
            Some(*max as f64),
            None,
            Vec::new(),
        ),
        ParamKind::Choice { options } => (
            "choice",
            None,
            None,
            None,
            options.iter().map(|o| o.id.clone()).collect(),
        ),
        ParamKind::Switch => ("switch", None, None, None, Vec::new()),
        ParamKind::Pitch => ("pitch", Some(0.0), Some(127.0), None, Vec::new()),
        ParamKind::PitchRange => ("pitch_range", Some(0.0), Some(127.0), None, Vec::new()),
        ParamKind::Division => (
            "division",
            None,
            None,
            None,
            DIVISION_NAMES.iter().map(|n| (*n).to_owned()).collect(),
        ),
        ParamKind::Breakpoints { min, max } => {
            ("breakpoints", Some(*min), Some(*max), None, Vec::new())
        }
        ParamKind::Levels { count, min, max } => (
            "levels",
            Some(*min),
            Some(*max),
            None,
            vec![count.to_string()],
        ),
        ParamKind::Toggles { count } => ("toggles", None, None, None, vec![count.to_string()]),
        ParamKind::Structured => ("structured", None, None, None, Vec::new()),
    };
    ParamInfo {
        id: spec.id.clone(),
        name: spec.name.clone(),
        kind: kind.to_owned(),
        min,
        max,
        decimals,
        options,
        unit: spec.unit.clone(),
        scale_aware: spec.scale_aware,
        default: value_to_json(spec, &spec.default),
        description: spec.description.clone(),
    }
}

fn kind_of(kind: ToolKind) -> ids::ToolKind {
    match kind {
        ToolKind::Transformation => ids::ToolKind::Transformation,
        ToolKind::Generator => ids::ToolKind::Generator,
    }
}

fn list() -> ToolList {
    let registry = Registry::native();
    ToolList {
        tools: [ToolKind::Transformation, ToolKind::Generator]
            .into_iter()
            .flat_map(|kind| registry.tools(kind))
            .map(|tool| {
                let d = tool.descriptor();
                ToolInfo {
                    id: d.id.clone(),
                    name: d.name.clone(),
                    kind: kind_of(d.kind),
                    parameters: d.parameters.iter().map(param_info).collect(),
                }
            })
            .collect(),
    }
}

// What a tool acts on (R10 to R13).

/// The note editor's notes, selection and range, as a tool would take them.
struct Target {
    range: (Ticks, Ticks),
    time: Option<(Ticks, Ticks)>,
    selected: Vec<u32>,
    /// The places of a transformation's input notes.
    input: Vec<u32>,
}

/// The range a tool acts on when the editor has no time selection (R10): the
/// loop brace while the loop is on, else the Start marker to the End marker.
fn default_range(midi: &MidiData, length: Ticks) -> (Ticks, Ticks) {
    let end = midi.start + length;
    if midi.looping {
        (midi.loop_start, midi.loop_end.unwrap_or(end))
    } else {
        (midi.start, midi.end.unwrap_or(end))
    }
}

fn target(
    doc: &Document,
    view: &ClipView,
    arrangement: &ArrangementView,
    clip: &Clip,
    midi: &MidiData,
    notes: Option<&[u32]>,
    range: Option<RangeArgs>,
) -> Result<Target, EditFailure> {
    let ordered = notes_in_order(midi);
    let shown = view.shown_clip(doc, arrangement) == Some(clip.id);
    let selected = if shown {
        view.selected_notes(doc, arrangement).to_vec()
    } else {
        Vec::new()
    };
    let time = shown
        .then(|| view.time_selection(doc, arrangement))
        .flatten();
    // Notes or a range in the arguments stand in for the editor's selection.
    let explicit = notes.is_some() || range.is_some();
    let range = match range {
        Some(RangeArgs { start, end }) => {
            if start >= end {
                return Err(invalid("`range` must start before it ends"));
            }
            (start, end)
        }
        None if explicit => default_range(midi, clip.length),
        None => time.unwrap_or_else(|| default_range(midi, clip.length)),
    };
    let starting_in = |(from, to): (Ticks, Ticks)| -> Vec<u32> {
        ordered
            .iter()
            .enumerate()
            .filter(|(_, note)| note.start >= from && note.start < to)
            .map(|(place, _)| place as u32)
            .collect()
    };
    let input = if let Some(notes) = notes {
        let mut places = Vec::with_capacity(notes.len());
        for place in notes {
            if *place as usize >= ordered.len() {
                return Err(invalid(format!(
                    "clip {} has no note at place {place}",
                    clip.id.0
                )));
            }
            if !places.contains(place) {
                places.push(*place);
            }
        }
        places
    } else if explicit {
        starting_in(range)
    } else if !selected.is_empty() {
        selected.clone()
    } else {
        starting_in(time.unwrap_or(range))
    };
    Ok(Target {
        range,
        time,
        selected,
        input,
    })
}

/// A note as a tool sees it.
fn tool_note(note: &Note) -> ToolNote {
    ToolNote {
        key: i16::from(note.key),
        start: note.start,
        length: note.length,
        velocity: note.velocity,
        release_velocity: note.release_velocity,
        chance: note.probability,
        velocity_deviation: note.velocity_deviation,
        deactivated: note.mute,
        pitch: note.pitch.iter().map(tool_point).collect(),
        slide: note.slide.iter().map(tool_point).collect(),
        pressure: note.pressure.iter().map(tool_point).collect(),
    }
}

/// A breakpoint as a tool sees it.
fn tool_point(point: &NotePoint) -> ToolPoint {
    ToolPoint {
        at: point.at,
        value: point.value,
        bend: point.bend,
    }
}

/// A note a tool wrote, as the document holds it.
fn note_of(tool: &ToolNote) -> Note {
    Note {
        key: u8::try_from(tool.key.clamp(0, 127)).unwrap_or(0),
        velocity: tool.velocity.clamp(1, 127),
        release_velocity: tool.release_velocity.clamp(1, 127),
        start: tool.start,
        length: tool.length,
        probability: tool.chance.clamp(0.0, 1.0),
        velocity_deviation: tool.velocity_deviation,
        mute: tool.deactivated,
        // A written note joins no probability group (note-editing R21).
        group: None,
        pitch: tool.pitch.iter().map(breakpoint).collect(),
        slide: tool.slide.iter().map(breakpoint).collect(),
        pressure: tool.pressure.iter().map(breakpoint).collect(),
    }
}

/// A breakpoint a tool wrote, as the document holds it.
fn breakpoint(point: &ToolPoint) -> NotePoint {
    NotePoint {
        at: point.at,
        value: point.value,
        bend: point.bend.clamp(-1.0, 1.0),
    }
}

/// The keys the Drum Rack of `track`'s instrument receives, in order (the
/// MIDI Tools spec, R9); `None` on a track with no Drum Rack.
fn drum_keys(doc: &Document, track: TrackId) -> Option<Vec<u8>> {
    let rack = doc
        .track(track)?
        .devices()
        .find(|device| device.rack_kind() == Some(RackKind::Drum))?;
    let mut keys = Vec::new();
    for chain in rack.chains.iter().filter(|chain| !chain.is_return) {
        match chain.receive {
            Receive::AllNotes => return Some((0..=127).collect()),
            Receive::Note(key) => keys.push(key),
        }
    }
    keys.sort_unstable();
    keys.dedup();
    Some(keys)
}

/// What a tool's notes depend on (R16).
fn tool_input(
    doc: &Document,
    arrangement: &ArrangementView,
    clip: &Clip,
    midi: &MidiData,
    app: &Application,
    step: &Step,
) -> ToolInput {
    ToolInput {
        notes: step
            .input
            .iter()
            .map(|at| tool_note(&step.base[*at]))
            .collect(),
        range: app.range,
        grid_step: arrangement.grid_ticks(doc),
        signature: midi
            .signature
            .unwrap_or_else(|| doc.settings().time_signatures.signature_at(clip.start)),
        scale: clip.scale.enabled.then(|| ToolScale {
            root: clip.scale.root,
            steps: clip.scale.steps().to_vec(),
        }),
        drum_keys: doc
            .clip_track(clip.id)
            .and_then(|track| drum_keys(doc, track)),
        seed: step.seed,
    }
}

/// `keys` each moved `degrees` steps as a generator's pitch steps, through
/// the clip's scale or its Drum Rack's keys (the MIDI generators spec, R1,
/// R14); a key that can't move stays where it is.
#[must_use]
pub fn stepped_keys(doc: &Document, clip: ClipId, keys: &[u8], degrees: i32) -> Vec<u8> {
    if doc.clip(clip).is_none() {
        return keys.to_vec();
    }
    let input = plain_input(doc, clip);
    keys.iter()
        .map(|key| input.step_key(*key, degrees).unwrap_or(*key))
        .collect()
}

/// A tool's input with no notes: the clip's scale and Drum Rack keys, which
/// is all a generator's pitch controls need.
fn plain_input(doc: &Document, clip: ClipId) -> ToolInput {
    let value = doc.clip(clip);
    ToolInput {
        notes: Vec::new(),
        range: (Ticks::ZERO, Ticks::ZERO),
        grid_step: Ticks::ZERO,
        signature: value.map_or(bl_core::time::TimeSignature::FOUR_FOUR, |value| {
            doc.settings().time_signatures.signature_at(value.start)
        }),
        scale: value
            .filter(|value| value.scale.enabled)
            .map(|value| ToolScale {
                root: value.scale.root,
                steps: value.scale.steps().to_vec(),
            }),
        drum_keys: doc.clip_track(clip).and_then(|track| drum_keys(doc, track)),
        seed: 0,
    }
}

/// Every entry of Stacks' bank with its keys on `root`, as Stacks writes them
/// for `clip` (the MIDI generators spec, R19): what the Chord Selector Pad
/// draws and names on hover (R21).
#[must_use]
pub fn bank_chords(
    doc: &Document,
    tools: &MidiTools,
    clip: ClipId,
    root: u8,
) -> Vec<(String, Vec<u8>)> {
    let Some(tool) = Registry::native().find(STACKS) else {
        return Vec::new();
    };
    let Ok(view) = stack_view(&tools.settings_of(tool.descriptor()).values) else {
        return Vec::new();
    };
    let input = plain_input(doc, clip);
    view.bank
        .chords
        .into_iter()
        .map(|entry| {
            let keys = entry_keys(&input, &entry, root, 0);
            (entry.name, keys)
        })
        .collect()
}

/// The clip's notes once `written` replace what the tool took (R11, R12),
/// in the editor's order, and the places of the written notes.
fn merged(step: &Step, kind: ToolKind, written: &[Note]) -> (Vec<Note>, Vec<u32>) {
    let mut taken = vec![false; step.base.len()];
    match kind {
        ToolKind::Transformation => {
            for at in &step.input {
                taken[*at] = true;
            }
        }
        // A note at a written note's key whose span overlaps it goes (R12).
        // Per key, the written spans in order of start, each with the
        // furthest end so far, find an overlap in a binary search.
        ToolKind::Generator => {
            let mut spans: BTreeMap<u8, Vec<(Ticks, Ticks)>> = BTreeMap::new();
            for new in written {
                spans
                    .entry(new.key)
                    .or_default()
                    .push((new.start, new.start + new.length));
            }
            for list in spans.values_mut() {
                list.sort_unstable();
                let mut furthest = Ticks::MIN;
                for span in list.iter_mut() {
                    furthest = furthest.max(span.1);
                    span.1 = furthest;
                }
            }
            for (at, note) in step.base.iter().enumerate() {
                let end = note.start + note.length;
                taken[at] = spans.get(&note.key).is_some_and(|list| {
                    let before = list.partition_point(|span| span.0 < end);
                    before > 0 && list[before - 1].1 > note.start
                });
            }
        }
    }
    let mut notes: Vec<(Note, bool)> = step
        .base
        .iter()
        .zip(&taken)
        .filter(|(_, taken)| !**taken)
        .map(|(note, _)| (note.clone(), false))
        .collect();
    notes.extend(written.iter().map(|note| (note.clone(), true)));
    notes.sort_by_key(|(note, _)| (note.start, note.key));
    let places = notes
        .iter()
        .enumerate()
        .filter(|(_, (_, new))| *new)
        .map(|(place, _)| place as u32)
        .collect();
    (notes.into_iter().map(|(note, _)| note).collect(), places)
}

/// Replaces a MIDI clip's notes, keeping its markers, loop and other data.
fn set_notes(
    editor: &mut Editor,
    clip: ClipId,
    midi: &MidiData,
    notes: Vec<Note>,
) -> Result<(), EditFailure> {
    let mut data = midi.clone();
    data.notes = notes.into_iter().collect();
    // The notes a tool took may leave a group with fewer than two.
    data.tidy_groups();
    editor.apply(bl_model::Command::SetClipData {
        clip,
        data: ClipData::Midi(data),
    })?;
    Ok(())
}

/// Writes `notes` into the chain's clip, selects the notes at `places`, lets
/// the time selection follow them (R15), and keeps the chain open on what it
/// left.
fn settle(
    editor: &mut Editor,
    view: &mut ClipView,
    arrangement: &ArrangementView,
    mut app: Application,
    notes: Vec<Note>,
    places: Vec<u32>,
) -> Result<(), EditFailure> {
    let ClipData::Midi(midi) = &editor
        .document()
        .clip(app.clip)
        .ok_or_else(|| invalid(format!("no clip has the ID {}", app.clip.0)))?
        .data
    else {
        return Err(refused("the clip isn't a MIDI clip"));
    };
    let midi = midi.clone();
    set_notes(editor, app.clip, &midi, notes)?;
    view.select(app.clip, places.clone());
    if let Some(span) = span_of(editor.document(), app.clip, &places) {
        view.set_time_selection(app.clip, Some(span));
    }
    let doc = editor.document();
    if let Some(ClipData::Midi(midi)) = doc.clip(app.clip).map(|clip| &clip.data) {
        app.after = notes_in_order(midi);
    }
    app.selected = view.selected_notes(doc, arrangement).to_vec();
    app.time = view.time_selection(doc, arrangement);
    view.tools.open = Some(app);
    Ok(())
}

/// Runs the chain from step `from`: each step on the notes the step before
/// it wrote (the kept input, for the first), with its own settings and seed
/// (R27, R28). The clip's notes become what the last step made: one step
/// named after the tool, recorded even when the notes come out as they were
/// (R22). The written notes are selected and the time selection follows
/// them (R15).
fn replay(
    editor: &mut Editor,
    view: &mut ClipView,
    arrangement: &ArrangementView,
    mut app: Application,
    from: usize,
) -> Result<ToolRun, EditFailure> {
    let doc = editor.document().clone();
    let clip = doc
        .clip(app.clip)
        .ok_or_else(|| invalid(format!("no clip has the ID {}", app.clip.0)))?;
    let ClipData::Midi(midi) = &clip.data else {
        return Err(refused("the clip isn't a MIDI clip"));
    };
    let registry = Registry::native();
    let mut made: Option<(Vec<Note>, Vec<u32>)> = None;
    let mut wrote = 0;
    for at in from..app.steps.len() {
        if let Some((notes, places)) = made.take() {
            let step = &mut app.steps[at];
            step.base = notes;
            step.input = places.iter().map(|place| *place as usize).collect();
        }
        let step = &app.steps[at];
        let tool = registry
            .find(&step.tool)
            .ok_or_else(|| unavailable(format!("the chain's tool `{}` is gone", step.tool)))?;
        let input = tool_input(&doc, arrangement, clip, midi, &app, step);
        let output = bl_midi_tools::run(tool, &input, &step.values).map_err(failure)?;
        let written: Vec<Note> = output.notes.iter().map(note_of).collect();
        wrote = written.len();
        made = Some(merged(step, tool.descriptor().kind, &written));
    }
    let (notes, places) = made.ok_or_else(|| unavailable("the chain has no step to run"))?;
    let last = app
        .steps
        .last()
        .ok_or_else(|| unavailable("the chain has no step"))?;
    let run = ToolRun {
        tool: last.tool.clone(),
        seed: last.seed,
        wrote,
        notes: places.clone(),
        undo_step: false,
    };
    editor.keep_step();
    settle(editor, view, arrangement, app, notes, places)?;
    Ok(run)
}

/// What a command acts on: the clip, its data and its track.
struct Subject {
    clip: ClipId,
    track: Option<TrackId>,
}

fn subject(
    doc: &Document,
    view: &ClipView,
    arrangement: &ArrangementView,
    clip: Option<ClipId>,
) -> Result<Subject, EditFailure> {
    let clip = clip
        .or_else(|| view.shown_clip(doc, arrangement))
        .ok_or_else(|| unavailable("no MIDI clip is shown in the Clip View"))?;
    match doc.clip(clip).map(|value| &value.data) {
        None => Err(invalid(format!("no clip has the ID {}", clip.0))),
        Some(ClipData::Audio(_)) => Err(refused("the clip isn't a MIDI clip")),
        Some(ClipData::Midi(_)) => Ok(Subject {
            clip,
            track: doc.clip_track(clip),
        }),
    }
}

/// The audio clip a command is for, if it is for one: the named clip, or the
/// one the Clip View shows. Its Transform panel holds Quantize alone (the
/// MIDI transformations spec, R22), whose settings are the tool's, not the
/// clip's.
#[must_use]
pub fn audio_clip(
    doc: &Document,
    view: &ClipView,
    arrangement: &ArrangementView,
    clip: Option<ClipId>,
) -> Option<ClipId> {
    let clip = clip.or_else(|| view.shown_audio_clip(doc, arrangement))?;
    matches!(doc.clip(clip)?.data, ClipData::Audio(_)).then_some(clip)
}

/// Quantize, for a command on an audio clip: only it has a Transform panel
/// there.
fn audio_tool(tool: Option<&str>) -> Result<&'static dyn MidiTool, EditFailure> {
    if tool.is_some_and(|tool| tool != QUANTIZE) {
        return Err(refused(
            "an audio clip's Transform panel holds Quantize alone",
        ));
    }
    Registry::native()
        .find(QUANTIZE)
        .ok_or_else(|| unavailable("Quantize is missing"))
}

/// The tool a command names, or the chosen tool of the panel shown last.
fn resolve_tool(
    tools: &MidiTools,
    track: Option<TrackId>,
    name: Option<&str>,
) -> Result<&'static dyn MidiTool, EditFailure> {
    match name {
        Some(id) => Registry::native().find(id).ok_or_else(|| {
            invalid(format!(
                "no MIDI tool has the ID `{id}`; see midi_tool.list"
            ))
        }),
        None => tools
            .chosen(track, tools.current_kind())
            .ok_or_else(|| unavailable("there is no MIDI tool to apply")),
    }
}

/// Checks `values` against the tool's parameters and makes them its
/// settings.
fn set_values(
    tools: &mut MidiTools,
    tool: &dyn MidiTool,
    values: &Map<String, Value>,
) -> Result<Vec<usize>, EditFailure> {
    let descriptor = tool.descriptor();
    let mut settings = tools.settings_of(descriptor);
    let before = settings.values.clone();
    let mut changed = Vec::with_capacity(values.len());
    for (id, json) in values {
        let (at, spec) = descriptor.parameter(id).ok_or_else(|| {
            let known: Vec<&str> = descriptor
                .parameters
                .iter()
                .map(|p| p.id.as_str())
                .collect();
            invalid(format!(
                "{} has no parameter `{id}`; it has {}",
                descriptor.name,
                known.join(", ")
            ))
        })?;
        settings.values[at] = value_from_json(spec, json).map_err(invalid)?;
        changed.push(at);
    }
    // A tool whose parameters depend on one another adjusts them, or refuses.
    tool.settle(&before, &mut settings.values, &changed)
        .map_err(invalid)?;
    descriptor
        .check(&settings.values)
        .map_err(|error| invalid(error.to_string()))?;
    tool.validate(&settings.values).map_err(failure)?;
    tools.settings.insert(descriptor.id.clone(), settings);
    Ok(changed)
}

/// Moves a tool's settings that are still the defaults onto what the clip
/// allows, before its first change or application: Stacks' root onto the
/// clip's scale (the MIDI generators spec, R20).
fn start_on_clip(tools: &mut MidiTools, tool: &dyn MidiTool, doc: &Document, clip: ClipId) {
    let descriptor = tool.descriptor();
    let mut settings = tools.settings_of(descriptor);
    if !settings.at_defaults(descriptor) {
        return;
    }
    tool.start(&mut settings.values, &plain_input(doc, clip));
    if !settings.at_defaults(descriptor) {
        tools.settings.insert(descriptor.id.clone(), settings);
    }
}

/// What a new application acts on besides the editor's selection, and its
/// seed.
#[derive(Clone, Copy)]
struct Request<'a> {
    notes: Option<&'a [u32]>,
    range: Option<RangeArgs>,
    seed: u32,
}

/// A new application on the target as it is now (R17, R19), a chain of the
/// one tool with `values`.
fn begin(
    doc: &Document,
    view: &ClipView,
    arrangement: &ArrangementView,
    clip: ClipId,
    tool: &dyn MidiTool,
    values: Vec<ParamValue>,
    request: &Request<'_>,
) -> Result<Application, EditFailure> {
    let Request { notes, range, seed } = *request;
    let value = doc.clip(clip).ok_or_else(|| invalid("no such clip"))?;
    let ClipData::Midi(midi) = &value.data else {
        return Err(refused("the clip isn't a MIDI clip"));
    };
    let target = target(doc, view, arrangement, value, midi, notes, range)?;
    Ok(Application {
        clip,
        shown: view.shown_clip(doc, arrangement),
        steps: vec![Step {
            tool: tool.descriptor().id.clone(),
            values,
            seed,
            base: notes_in_order(midi),
            input: if tool.descriptor().kind == ToolKind::Transformation {
                target.input.iter().map(|place| *place as usize).collect()
            } else {
                Vec::new()
            },
        }],
        range: target.range,
        before: (target.selected, target.time),
        after: Vec::new(),
        selected: Vec::new(),
        time: None,
    })
}

/// The chain with a transformation added: it takes the notes the chain
/// wrote (R27).
fn join(
    mut app: Application,
    tool: &dyn MidiTool,
    values: Vec<ParamValue>,
    seed: u32,
) -> Application {
    app.steps.push(Step {
        tool: tool.descriptor().id.clone(),
        values,
        seed,
        base: app.after.clone(),
        input: app.selected.iter().map(|place| *place as usize).collect(),
    });
    app
}

/// The state a command returns, with the run it made.
fn state(
    doc: &Document,
    view: &ClipView,
    arrangement: &ArrangementView,
    clip: Option<ClipId>,
    run: Option<ToolRun>,
) -> MidiToolState {
    let tools = &view.tools;
    let clip = clip.or_else(|| view.shown_clip(doc, arrangement));
    let value = clip.and_then(|id| doc.clip(id));
    let midi = value.and_then(|clip| match &clip.data {
        ClipData::Midi(midi) => Some(midi),
        ClipData::Audio(_) => None,
    });
    let track = clip.and_then(|clip| doc.clip_track(clip));
    let registry = Registry::native();
    let current = tools.chosen(track, tools.current_kind());
    MidiToolState {
        panel: tools.panel,
        clip: midi.and(clip),
        current_tool: current.map(|tool| tool.descriptor().id.clone()),
        transformation: tools.chosen_id(track, ToolKind::Transformation),
        generator: tools.chosen_id(track, ToolKind::Generator),
        tools: [ToolKind::Transformation, ToolKind::Generator]
            .into_iter()
            .flat_map(|kind| registry.tools(kind))
            .map(|tool| {
                let d = tool.descriptor();
                let settings = tools.settings_of(d);
                ToolSettingsState {
                    tool: d.id.clone(),
                    kind: kind_of(d.kind),
                    auto: settings.auto,
                    values: d
                        .parameters
                        .iter()
                        .zip(&settings.values)
                        .map(|(spec, value)| (spec.id.clone(), value_to_json(spec, value)))
                        .collect(),
                    at_defaults: settings.at_defaults(d),
                }
            })
            .collect(),
        stacks: stacks_state(tools),
        target: value.zip(midi).and_then(|(value, midi)| {
            let target = target(doc, view, arrangement, value, midi, None, None).ok()?;
            Some(TargetState {
                range: target.range,
                time_selection: target.time,
                selected: target.selected,
                input: target.input,
            })
        }),
        application: clip
            .and_then(|clip| tools.open_for(doc, view, arrangement, clip))
            .and_then(|app| {
                let last = app.steps.last()?;
                Some(ApplicationState {
                    tool: last.tool.clone(),
                    clip: app.clip,
                    input_notes: app.steps.first().map_or(0, |first| first.input.len()),
                    range: app.range,
                    seed: last.seed,
                    chain: app
                        .steps
                        .iter()
                        .map(|step| StepState {
                            tool: step.tool.clone(),
                            values: registry
                                .find(&step.tool)
                                .map(|tool| {
                                    tool.descriptor()
                                        .parameters
                                        .iter()
                                        .zip(&step.values)
                                        .map(|(spec, value)| {
                                            (spec.id.clone(), value_to_json(spec, value))
                                        })
                                        .collect()
                                })
                                .unwrap_or_default(),
                            seed: step.seed,
                        })
                        .collect(),
                })
            }),
        run,
    }
}

/// Stacks' progression and bank as `midi_tool.state` reads them (the MIDI
/// generators spec, R28).
fn stacks_state(tools: &MidiTools) -> StacksState {
    let tool = Registry::native()
        .find(STACKS)
        .expect("Stacks is a native tool");
    let descriptor = tool.descriptor();
    let view = stack_view(&tools.settings_of(descriptor).values)
        .or_else(|_| stack_view(&descriptor.defaults()))
        .expect("Stacks' defaults are a progression and a bank");
    StacksState {
        progression: view
            .chords
            .iter()
            .map(|chord| ChordState {
                chord: chord.chord,
                root: chord.root,
                inversion: chord.inversion,
                duration: chord.duration,
                offset: chord.offset,
            })
            .collect(),
        selected: view.selected,
        bank: BankState {
            name: view.bank.name,
            built_in: view.built_in,
            chords: view
                .bank
                .chords
                .into_iter()
                .map(|entry| BankEntryState {
                    name: entry.name,
                    degrees: entry.degrees,
                    intervals: entry.intervals,
                })
                .collect(),
        },
    }
}

/// Changes Stacks' settings by `edit` and applies it when its Auto switch is
/// on, as `midi_tool.set` does (the MIDI generators spec, R17).
fn edit_stacks(
    editor: &mut Editor,
    view: &mut ClipView,
    arrangement: &ArrangementView,
    clip: Option<ClipId>,
    edit: impl FnOnce(&mut [ParamValue]) -> Result<(), String>,
) -> Result<Value, EditFailure> {
    let doc = editor.document().clone();
    let subject = subject(&doc, view, arrangement, clip)?;
    let found = Registry::native()
        .find(STACKS)
        .ok_or_else(|| unavailable("Stacks is missing"))?;
    let descriptor = found.descriptor();
    let mut settings = view.tools.settings_of(descriptor);
    edit(&mut settings.values).map_err(refused)?;
    view.tools.settings.insert(descriptor.id.clone(), settings);
    let run = if view.tools.settings_of(descriptor).auto {
        auto_apply(editor, view, arrangement, subject.clip, found, None)?
    } else {
        None
    };
    let doc = editor.document().clone();
    Ok(to_value(&state(
        &doc,
        view,
        arrangement,
        Some(subject.clip),
        run,
    )))
}

/// Loads the bank in the file at `path`: refused, saying what is wrong and
/// where, when the file breaks the format (the MIDI generators spec, R25).
fn read_bank(path: &str) -> Result<ChordBank, EditFailure> {
    use std::io::Read;
    let read = |error: std::io::Error| invalid(format!("{path}: {error}"));
    // One byte past the limit is enough to see a file is too long, and keeps
    // a path that never ends (a device file) from filling memory.
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(read)?
        .take(MAX_BANK_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(read)?;
    ChordBank::parse(&bytes).map_err(|error| invalid(format!("{path}: {error}")))
}

/// With Auto on, applies the tool after a change of its parameters (R17): a
/// tool in the open chain runs the chain again from its step, the last one
/// of that tool unless `step` names another (R28); another transformation
/// joins the chain (R27); anything else starts a new chain on the target. A
/// transformation does nothing while it has no input; a generator always
/// runs.
fn auto_apply(
    editor: &mut Editor,
    view: &mut ClipView,
    arrangement: &ArrangementView,
    clip: ClipId,
    tool: &dyn MidiTool,
    step: Option<usize>,
) -> Result<Option<ToolRun>, EditFailure> {
    let doc = editor.document().clone();
    let descriptor = tool.descriptor();
    let values = view.tools.settings_of(descriptor).values;
    let open = view.tools.open_for(&doc, view, arrangement, clip).cloned();
    let found = match &open {
        Some(app) => app.step_of(&descriptor.id, step)?,
        None if step.is_some() => return Err(unavailable("no MIDI tool chain is open")),
        None => None,
    };
    let (app, from) = match (open, found) {
        (Some(mut app), Some(at)) => {
            app.steps[at].values = values;
            (app, at)
        }
        (Some(app), None) if descriptor.kind == ToolKind::Transformation => {
            if app.selected.is_empty() {
                return Ok(None);
            }
            let seed = view.tools.draw_seed();
            let at = app.steps.len();
            (join(app, tool, values, seed), at)
        }
        _ => {
            let seed = view.tools.draw_seed();
            let request = Request {
                notes: None,
                range: None,
                seed,
            };
            let app = begin(&doc, view, arrangement, clip, tool, values, &request)?;
            // A transformation starts only while notes or a time are
            // selected (R17), and has something to take.
            let selected = !app.before.0.is_empty() || app.before.1.is_some();
            if descriptor.kind == ToolKind::Transformation
                && (app.steps[0].input.is_empty() || !selected)
            {
                return Ok(None);
            }
            (app, 0)
        }
    };
    let (run, recorded) = step_named(editor, descriptor.name.clone(), |editor| {
        replay(editor, view, arrangement, app, from)
    })?;
    Ok(Some(ToolRun {
        undo_step: recorded,
        ..run
    }))
}

/// Runs `edit` as one undo step with this name.
fn step_named<T>(
    editor: &mut Editor,
    name: String,
    edit: impl FnOnce(&mut Editor) -> Result<T, EditFailure>,
) -> Result<(T, bool), EditFailure> {
    transaction(editor, name, edit)
}

/// Takes step `at` out of the chain: the steps after it run again on the
/// notes the step before it made, or on the input for the first (R31). Taking
/// out the only step left puts the input and the selection back and closes
/// the chain.
fn take_out(
    editor: &mut Editor,
    view: &mut ClipView,
    arrangement: &ArrangementView,
    mut app: Application,
    at: usize,
) -> Result<(), EditFailure> {
    let removed = app.steps.remove(at);
    if app.steps.is_empty() {
        let ClipData::Midi(midi) = &editor
            .document()
            .clip(app.clip)
            .ok_or_else(|| invalid("no such clip"))?
            .data
        else {
            return Err(refused("the clip isn't a MIDI clip"));
        };
        let midi = midi.clone();
        set_notes(editor, app.clip, &midi, removed.base)?;
        view.select(app.clip, app.before.0.clone());
        view.set_time_selection(app.clip, app.before.1);
        view.tools.close();
    } else if let Some(next) = app.steps.get_mut(at) {
        next.base = removed.base;
        next.input = removed.input;
        replay(editor, view, arrangement, app, at)?;
    } else {
        // The last step went: the notes are what the step before it wrote.
        let places = removed.input.iter().map(|place| *place as u32).collect();
        settle(editor, view, arrangement, app, removed.base, places)?;
    }
    Ok(())
}

/// The name of the undo step a tool's control makes while it is dragged: the
/// tool's name (R22). `None` for a command that isn't a tool's.
#[must_use]
pub fn gesture_label(
    id: &str,
    args: &Value,
    doc: &Document,
    view: &ClipView,
    arrangement: &ArrangementView,
) -> Option<String> {
    if !ids::handles(id) {
        return None;
    }
    let clip = args
        .get("clip")
        .and_then(|clip| serde_json::from_value::<ClipId>(clip.clone()).ok());
    let track = clip
        .or_else(|| view.shown_clip(doc, arrangement))
        .and_then(|clip| doc.clip_track(clip));
    let name = args.get("tool").and_then(Value::as_str);
    resolve_tool(&view.tools, track, name)
        .ok()
        .map(|tool| tool.descriptor().name.clone())
}

/// Runs a MIDI Tools command (`midi_tool.*`, `clip_view.show_panel`).
///
/// # Errors
///
/// [`EditFailure`] saying why nothing changed.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per command keeps each one's behaviour in sight"
)]
pub fn run(
    id: &str,
    args: Value,
    editor: &mut Editor,
    view: &mut ClipView,
    arrangement: &ArrangementView,
) -> Result<Value, EditFailure> {
    let doc = editor.document().clone();
    match id {
        ids::LIST => Ok(to_value(&list())),
        ids::STATE => {
            let ClipArgs { clip } = parse(args)?;
            Ok(to_value(&state(&doc, view, arrangement, clip, None)))
        }
        ids::SHOW_PANEL => {
            let ShowPanelArgs { panel } = parse(args)?;
            view.tools.show_panel(panel);
            Ok(to_value(&state(&doc, view, arrangement, None, None)))
        }
        ids::CLOSE => {
            let ClipArgs { clip } = parse(args)?;
            view.tools.close();
            Ok(to_value(&state(&doc, view, arrangement, clip, None)))
        }
        ids::CHOOSE => {
            let ChooseArgs { clip, tool } = parse(args)?;
            let subject = subject(&doc, view, arrangement, clip)?;
            let found = resolve_tool(&view.tools, subject.track, Some(&tool))?;
            let track = subject
                .track
                .ok_or_else(|| unavailable("the clip has no track"))?;
            let kind = found.descriptor().kind;
            let chosen = view.tools.chosen.entry(track).or_default();
            match kind {
                ToolKind::Transformation => chosen.transformation = Some(tool),
                ToolKind::Generator => chosen.generator = Some(tool),
            }
            view.tools.generator_last = kind == ToolKind::Generator;
            // A tool in the open chain shows its step's settings (R28).
            let descriptor = found.descriptor();
            let values = view
                .tools
                .open_for(&doc, view, arrangement, subject.clip)
                .and_then(|app| {
                    app.step_of(&descriptor.id, None)
                        .ok()
                        .flatten()
                        .map(|at| app.steps[at].values.clone())
                });
            if let Some(values) = values {
                let mut settings = view.tools.settings_of(descriptor);
                settings.values = values;
                view.tools.settings.insert(descriptor.id.clone(), settings);
            }
            Ok(to_value(&state(
                &doc,
                view,
                arrangement,
                Some(subject.clip),
                None,
            )))
        }
        ids::SET => {
            let SetArgs {
                clip,
                tool,
                step,
                values,
            } = parse(args)?;
            if audio_clip(&doc, view, arrangement, clip).is_some() {
                // R22: Quantize's Grid, Triplets and Amount; nothing to apply to.
                if values.is_empty() {
                    return Err(invalid("`values` names no parameter"));
                }
                set_values(&mut view.tools, audio_tool(tool.as_deref())?, &values)?;
                return Ok(to_value(&state(&doc, view, arrangement, clip, None)));
            }
            let subject = subject(&doc, view, arrangement, clip)?;
            let found = resolve_tool(&view.tools, subject.track, tool.as_deref())?;
            if values.is_empty() {
                return Err(invalid("`values` names no parameter"));
            }
            // A named step shows its own settings before the change (R28).
            if let Some(named) = step {
                let descriptor = found.descriptor();
                let app = view
                    .tools
                    .open_for(&doc, view, arrangement, subject.clip)
                    .ok_or_else(|| unavailable("no MIDI tool chain is open"))?;
                let at = app.step_of(&descriptor.id, Some(named))?;
                let values = at.map(|at| app.steps[at].values.clone());
                if let Some(values) = values {
                    let mut settings = view.tools.settings_of(descriptor);
                    settings.values = values;
                    view.tools.settings.insert(descriptor.id.clone(), settings);
                }
            }
            start_on_clip(&mut view.tools, found, &doc, subject.clip);
            let changed = set_values(&mut view.tools, found, &values)?;
            let run = if view.tools.settings_of(found.descriptor()).auto
                && found.changes_notes(&changed)
            {
                auto_apply(editor, view, arrangement, subject.clip, found, step)?
            } else {
                None
            };
            let doc = editor.document().clone();
            Ok(to_value(&state(
                &doc,
                view,
                arrangement,
                Some(subject.clip),
                run,
            )))
        }
        ids::SET_AUTO => {
            let SetAutoArgs { clip, tool, auto } = parse(args)?;
            let subject = subject(&doc, view, arrangement, clip)?;
            let found = resolve_tool(&view.tools, subject.track, tool.as_deref())?;
            let descriptor = found.descriptor();
            let mut settings = view.tools.settings_of(descriptor);
            settings.auto = auto;
            view.tools.settings.insert(descriptor.id.clone(), settings);
            if !auto {
                // Auto off takes the tool's step out of the open chain
                // (R18, R31).
                let chain = view
                    .tools
                    .open_for(&doc, view, arrangement, subject.clip)
                    .cloned();
                if let Some(app) = chain
                    && let Some(at) = app.step_of(&descriptor.id, None)?
                {
                    step_named(editor, format!("Revert {}", descriptor.name), |editor| {
                        take_out(editor, view, arrangement, app, at)
                    })?;
                }
            }
            let doc = editor.document().clone();
            Ok(to_value(&state(
                &doc,
                view,
                arrangement,
                Some(subject.clip),
                None,
            )))
        }
        ids::RESET => {
            let ResetArgs { clip, tool } = parse(args)?;
            if audio_clip(&doc, view, arrangement, clip).is_some() {
                let descriptor = audio_tool(tool.as_deref())?.descriptor();
                let mut settings = view.tools.settings_of(descriptor);
                settings.values = descriptor.defaults();
                view.tools.settings.insert(descriptor.id.clone(), settings);
                return Ok(to_value(&state(&doc, view, arrangement, clip, None)));
            }
            let subject = subject(&doc, view, arrangement, clip)?;
            let found = resolve_tool(&view.tools, subject.track, tool.as_deref())?;
            let descriptor = found.descriptor();
            let mut settings = view.tools.settings_of(descriptor);
            settings.values = descriptor.defaults();
            view.tools.settings.insert(descriptor.id.clone(), settings);
            view.tools.close();
            Ok(to_value(&state(
                &doc,
                view,
                arrangement,
                Some(subject.clip),
                None,
            )))
        }
        ids::APPLY => {
            let ApplyArgs {
                clip,
                tool,
                values,
                notes,
                range,
                seed,
            } = parse(args)?;
            let subject = subject(&doc, view, arrangement, clip)?;
            let found = resolve_tool(&view.tools, subject.track, tool.as_deref())?;
            let descriptor = found.descriptor();
            start_on_clip(&mut view.tools, found, &doc, subject.clip);
            if let Some(values) = &values {
                set_values(&mut view.tools, found, values)?;
            }
            let seed = seed.unwrap_or_else(|| view.tools.draw_seed());
            let values = view.tools.settings_of(descriptor).values;
            // A transformation applied to the notes the open chain wrote
            // joins it (R27); notes or a range of its own start a new one.
            let open =
                (descriptor.kind == ToolKind::Transformation && notes.is_none() && range.is_none())
                    .then(|| {
                        view.tools
                            .open_for(&doc, view, arrangement, subject.clip)
                            .cloned()
                    })
                    .flatten();
            let (app, from) = if let Some(app) = open {
                let at = app.steps.len();
                (join(app, found, values, seed), at)
            } else {
                let request = Request {
                    notes: notes.as_deref(),
                    range,
                    seed,
                };
                (
                    begin(
                        &doc,
                        view,
                        arrangement,
                        subject.clip,
                        found,
                        values,
                        &request,
                    )?,
                    0,
                )
            };
            if descriptor.kind == ToolKind::Transformation
                && app.steps.last().is_none_or(|step| step.input.is_empty())
            {
                return Err(unavailable(
                    "there are no notes to transform: select notes, or a time selection \
                     holding notes, or give `notes` or a `range` that holds some",
                ));
            }
            let (run, recorded) = step_named(editor, descriptor.name.clone(), |editor| {
                replay(editor, view, arrangement, app, from)
            })?;
            let run = ToolRun {
                undo_step: recorded,
                ..run
            };
            let doc = editor.document().clone();
            Ok(to_value(&state(
                &doc,
                view,
                arrangement,
                Some(subject.clip),
                Some(run),
            )))
        }
        ids::RESEED => {
            let ReseedArgs { clip, seed } = parse(args)?;
            let subject = subject(&doc, view, arrangement, clip)?;
            let mut app = view
                .tools
                .open_for(&doc, view, arrangement, subject.clip)
                .cloned()
                .ok_or_else(|| unavailable("no MIDI tool application is open"))?;
            // The chain's last step takes the new seed, and runs again.
            let at = app.steps.len() - 1;
            app.steps[at].seed = seed.unwrap_or_else(|| view.tools.draw_seed());
            let found = Registry::native()
                .find(&app.steps[at].tool)
                .ok_or_else(|| unavailable("the application's tool is gone"))?;
            let (run, recorded) = step_named(editor, found.descriptor().name.clone(), |editor| {
                replay(editor, view, arrangement, app, at)
            })?;
            let run = ToolRun {
                undo_step: recorded,
                ..run
            };
            let doc = editor.document().clone();
            Ok(to_value(&state(
                &doc,
                view,
                arrangement,
                Some(subject.clip),
                Some(run),
            )))
        }
        ids::RANDOMIZE => {
            let RandomizeArgs { clip, seed } = parse(args)?;
            let subject = subject(&doc, view, arrangement, clip)?;
            let found = Registry::native()
                .find(EUCLIDEAN)
                .ok_or_else(|| unavailable("Euclidean is missing"))?;
            let descriptor = found.descriptor();
            let seed = seed.unwrap_or_else(|| view.tools.draw_seed());
            let (at, _) = descriptor.parameter("steps").expect("Euclidean has Steps");
            let steps = match view.tools.settings_of(descriptor).values[at] {
                ParamValue::Whole(steps) => usize::try_from(steps).unwrap_or(1),
                _ => 1,
            };
            let values: Map<String, Value> = random_rotations(steps, seed)
                .into_iter()
                .enumerate()
                .map(|(voice, rotation)| (rotation_id(voice), json!(rotation)))
                .collect();
            set_values(&mut view.tools, found, &values)?;
            let run = if view.tools.settings_of(descriptor).auto {
                auto_apply(editor, view, arrangement, subject.clip, found, None)?
            } else {
                None
            };
            let doc = editor.document().clone();
            Ok(to_value(&state(
                &doc,
                view,
                arrangement,
                Some(subject.clip),
                run,
            )))
        }
        ids::ADD_CHORD => {
            let ClipArgs { clip } = parse(args)?;
            edit_stacks(editor, view, arrangement, clip, add_chord)
        }
        ids::DELETE_CHORD => {
            let ClipArgs { clip } = parse(args)?;
            edit_stacks(editor, view, arrangement, clip, delete_chord)
        }
        ids::LOAD_BANK => {
            let LoadBankArgs {
                clip,
                path,
                built_in,
            } = parse(args)?;
            // Spec: midi-generators R26. The bank replaces Stacks' bank and
            // changes no notes.
            let bank = match (path, built_in) {
                (Some(path), None | Some(false)) => Some(read_bank(&path)?),
                (None, Some(true)) => None,
                _ => {
                    return Err(invalid(
                        "give `path`, a chord bank file, or `built_in` true, not both",
                    ));
                }
            };
            let found = Registry::native()
                .find(STACKS)
                .ok_or_else(|| unavailable("Stacks is missing"))?;
            let descriptor = found.descriptor();
            let mut settings = view.tools.settings_of(descriptor);
            load_bank(&mut settings.values, bank).map_err(refused)?;
            view.tools.settings.insert(descriptor.id.clone(), settings);
            Ok(to_value(&state(&doc, view, arrangement, clip, None)))
        }
        other => Err(invalid(format!("`{other}` isn't a MIDI Tools command"))),
    }
}

/// The notes a transformation would take now and the range it would act on:
/// what its display, such as Recombine's columns, draws (R43). Empty with no
/// MIDI clip.
#[must_use]
pub fn input_notes(
    doc: &Document,
    view: &ClipView,
    arrangement: &ArrangementView,
    clip: ClipId,
) -> (Vec<Note>, (Ticks, Ticks)) {
    let Some(value) = doc.clip(clip) else {
        return (Vec::new(), (Ticks::ZERO, Ticks::ZERO));
    };
    let ClipData::Midi(midi) = &value.data else {
        return (Vec::new(), (Ticks::ZERO, Ticks::ZERO));
    };
    let Ok(target) = target(doc, view, arrangement, value, midi, None, None) else {
        return (Vec::new(), (Ticks::ZERO, Ticks::ZERO));
    };
    let ordered = notes_in_order(midi);
    let notes = target
        .input
        .iter()
        .filter_map(|place| ordered.get(*place as usize).cloned())
        .collect();
    (notes, target.range)
}

/// Whether Cmd+Enter and Edit ▸ Apply Transformation/Generator can apply
/// something now: a MIDI clip shows (R19).
#[must_use]
pub fn can_apply(doc: &Document, view: &ClipView, arrangement: &ArrangementView) -> bool {
    view.shown_clip(doc, arrangement).is_some()
        && view
            .tools
            .chosen(
                view.shown_clip(doc, arrangement)
                    .and_then(|clip| doc.clip_track(clip)),
                view.tools.current_kind(),
            )
            .is_some()
}

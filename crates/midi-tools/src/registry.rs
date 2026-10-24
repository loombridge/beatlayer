//! The tool interface and the registry that lists the tools (the MIDI Tools
//! spec, R3, R16 and R26).

use std::fmt;
use std::sync::OnceLock;

use crate::arpeggiate::Arpeggiate;
use crate::chop::Chop;
use crate::connect::Connect;
use crate::descriptor::{ParamValue, ToolDescriptor, ToolKind};
use crate::input::{ToolInput, ToolOutput, fold_octaves};
use crate::ornament::Ornament;
use crate::quantize::Quantize;
use crate::recombine::Recombine;
use crate::span::Span;
use crate::strum::Strum;
use crate::time_warp::TimeWarp;
use crate::velocity_shaper::VelocityShaper;
use bl_core::time::Ticks;

/// The most notes one run may write; a run that would write more is
/// refused and changes nothing (the MIDI Tools spec, R16).
pub const MAX_NOTES: usize = 65_536;

/// The transformations in the order the selector lists them (R3).
pub const TRANSFORMATION_ORDER: [&str; 12] = [
    "arpeggiate",
    "chop",
    "connect",
    "glissando",
    "lfo",
    "ornament",
    "quantize",
    "recombine",
    "span",
    "strum",
    "time_warp",
    "velocity_shaper",
];

/// The generators in the order the selector lists them (R3).
pub const GENERATOR_ORDER: [&str; 5] = ["rhythm", "seed", "shape", "stacks", "euclidean"];

/// Why a tool wrote nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolError {
    /// The values aren't the tool's parameters' kinds and ranges.
    Values(String),
    /// The input can't be used, such as a transformation with no notes.
    Input(String),
    /// The run would write more than [`MAX_NOTES`].
    TooManyNotes(usize),
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Values(message) | Self::Input(message) => f.write_str(message),
            Self::TooManyNotes(wrote) => write!(
                f,
                "the tool would write {wrote} notes; the most a run writes is {MAX_NOTES}"
            ),
        }
    }
}

impl std::error::Error for ToolError {}

/// A tool: a description, and a function from its input to the notes it
/// writes, with no access to the document (the MIDI Tools spec, R26).
pub trait MidiTool: Send + Sync {
    /// The tool's name, kind and parameters.
    fn descriptor(&self) -> &ToolDescriptor;

    /// Whether `values` suit the tool together, beyond each one's kind and
    /// range, such as Velocity Shaper's envelope. [`run`] and the shell check
    /// it before the settings change.
    ///
    /// # Errors
    ///
    /// [`ToolError::Values`] saying what is wrong.
    fn validate(&self, _values: &[ParamValue]) -> Result<(), ToolError> {
        Ok(())
    }

    /// Settles the values after a change that named the parameters at
    /// `changed`: `after` holds them all, `before` the values the change
    /// started from. A tool whose parameters depend on one another, as
    /// Chop's Gaps writes its Pattern, adjusts them here, or refuses the
    /// change, saying why. The default leaves them as they are.
    ///
    /// # Errors
    ///
    /// Why the values can't stand.
    fn settle(
        &self,
        _before: &[ParamValue],
        _after: &mut [ParamValue],
        _changed: &[usize],
    ) -> Result<(), String> {
        Ok(())
    }

    /// The notes the tool writes for `input` and `values`, one value per
    /// parameter. Call it through [`run`], which checks the values and
    /// finishes the notes.
    ///
    /// # Errors
    ///
    /// [`ToolError`] saying why nothing is written.
    fn run(&self, input: &ToolInput, values: &[ParamValue]) -> Result<ToolOutput, ToolError>;
}

/// Runs a tool as Beatlayer does: checks the values, then finishes what it
/// wrote. A key outside 0 to 127 moves by octaves until it is inside, a note
/// starts no earlier than tick 0 and lasts a tick at least (R13), a generator's
/// notes that start outside the range are left out and the rest are cut at the
/// range's end, and a run writing more than [`MAX_NOTES`] is refused.
///
/// # Errors
///
/// [`ToolError`] saying why nothing is written.
pub fn run(
    tool: &dyn MidiTool,
    input: &ToolInput,
    values: &[ParamValue],
) -> Result<ToolOutput, ToolError> {
    let descriptor = tool.descriptor();
    descriptor.check(values)?;
    tool.validate(values)?;
    let mut output = tool.run(input, values)?;
    if output.notes.len() > MAX_NOTES {
        return Err(ToolError::TooManyNotes(output.notes.len()));
    }
    let (from, to) = input.range;
    if descriptor.kind == ToolKind::Generator {
        output
            .notes
            .retain(|note| note.start >= from && note.start < to);
    }
    for note in &mut output.notes {
        note.key = i16::from(fold_octaves(i32::from(note.key)));
        note.start = note.start.max(Ticks::ZERO);
        if descriptor.kind == ToolKind::Generator {
            note.length = note.length.min(to - note.start);
        }
        note.length = note.length.max(Ticks(1));
    }
    Ok(output)
}

/// The tools Beatlayer ships: each native tool's task adds itself here.
fn native_tools() -> Vec<Box<dyn MidiTool>> {
    vec![
        Box::new(Arpeggiate::new()),
        Box::new(Chop::new()),
        Box::new(Connect::new()),
        Box::new(Ornament::new()),
        Box::new(Quantize::new()),
        Box::new(Recombine::new()),
        Box::new(Span::new()),
        Box::new(Strum::new()),
        Box::new(TimeWarp::new()),
        Box::new(VelocityShaper::new()),
    ]
}

/// The tools, in the order the selectors list them.
pub struct Registry {
    tools: Vec<Box<dyn MidiTool>>,
}

impl Registry {
    /// The tools Beatlayer ships, and with the `testing` feature the test
    /// tools after them.
    #[must_use]
    pub fn native() -> &'static Self {
        static NATIVE: OnceLock<Registry> = OnceLock::new();
        NATIVE.get_or_init(|| {
            let mut tools = native_tools();
            #[cfg(feature = "testing")]
            tools.extend(crate::testing::tools());
            let rank = |tool: &dyn MidiTool| {
                let descriptor = tool.descriptor();
                let order: &[&str] = match descriptor.kind {
                    ToolKind::Transformation => &TRANSFORMATION_ORDER,
                    ToolKind::Generator => &GENERATOR_ORDER,
                };
                order
                    .iter()
                    .position(|id| *id == descriptor.id)
                    .unwrap_or(order.len())
            };
            tools.sort_by_key(|tool| rank(tool.as_ref()));
            Registry { tools }
        })
    }

    /// Every tool of `kind`, in the selector's order.
    pub fn tools(&self, kind: ToolKind) -> impl Iterator<Item = &dyn MidiTool> {
        self.tools
            .iter()
            .map(|tool| tool.as_ref())
            .filter(move |tool| tool.descriptor().kind == kind)
    }

    /// The tool of `kind` with this ID.
    #[must_use]
    pub fn get(&self, kind: ToolKind, id: &str) -> Option<&dyn MidiTool> {
        self.tools(kind).find(|tool| tool.descriptor().id == id)
    }

    /// The tool with this ID, of either kind.
    #[must_use]
    pub fn find(&self, id: &str) -> Option<&dyn MidiTool> {
        self.tools
            .iter()
            .map(|tool| tool.as_ref())
            .find(|tool| tool.descriptor().id == id)
    }
}

//! The MIDI Tools' interface (WP-6.11): a tool described as data, run as a
//! pure function of its input notes, its settings and its seed.
//!
//! Every tool, native or added by an extension in milestone M9, is a
//! [`ToolDescriptor`] (its name, kind and parameters) and a [`MidiTool`]
//! that turns a [`ToolInput`] into the notes it writes ([`ToolOutput`]),
//! changing nothing else. What surrounds the tool is Beatlayer's: what it
//! acts on, replacing and merging notes, undo and the seed's draw. The
//! types are plain data so the same shape can cross a WebAssembly
//! component's boundary later. The rules are in the MIDI Tools spec, R16
//! and R26.
//!
//! The crate depends on `bl-core` only: the document is not visible to a
//! tool.

mod arpeggiate;
mod bank;
mod chop;
mod connect;
mod descriptor;
mod euclidean;
mod glissando;
mod input;
mod lfo;
mod ornament;
mod quantize;
mod random;
mod recombine;
mod registry;
mod rhythm;
mod seed;
mod shape;
mod span;
mod stacks;
mod strum;
mod style;
#[cfg(feature = "testing")]
mod testing;
mod time_warp;
mod value;
mod velocity_shaper;

pub use bank::{
    BANK_EXTENSION, BANK_FORMAT, BANK_SCHEMA, BankError, ChordBank, ChordEntry, MAX_BANK_BYTES,
    MAX_CHORD_NOTES, MAX_ENTRIES as MAX_BANK_ENTRIES, built_in as built_in_bank,
};
pub use chop::{TOGGLES as CHOP_TOGGLES, gaps_pattern, pattern_follows_gaps};
pub use descriptor::{
    Breakpoint, ChoiceOption, DIVISION_NAMES, ParamKind, ParamSpec, ParamValue, ToolDescriptor,
    ToolKind, division_ticks,
};
pub use euclidean::{
    MAX_STEPS as EUCLIDEAN_MAX_STEPS, VOICES as EUCLIDEAN_VOICES, euclid, random_rotations,
    rotation_id,
};
pub use input::{ToolInput, ToolNote, ToolOutput, ToolPoint, ToolScale, fold_octaves};
pub use lfo::lfo_curve;
pub use quantize::{GRIDS as QUANTIZE_GRIDS, grid_ticks};
pub use random::Rng;
pub use registry::{
    GENERATOR_ORDER, MAX_NOTES, MidiTool, Registry, TRANSFORMATION_ORDER, ToolError, run,
};
pub use rhythm::{MAX_STEPS as RHYTHM_MAX_STEPS, pattern_count, rhythm_steps};
pub use shape::{LEVELS as SHAPE_LEVELS, preset_levels};
pub use stacks::{
    Chord, MAX_CHORDS, StackView, add_chord, delete_chord, entry_keys, load_bank, stack_view,
};
pub use strum::strum_position;
pub use style::{
    CHORD_TRIGGER, PLAY_ORDER, RANDOM, RANDOM_ONCE, RANDOM_OTHER, STYLE_NAMES, style_order,
};
pub use value::{key_name, parse_key};
pub use velocity_shaper::ENVELOPE_POINTS;

//! Modular's patch (WP-9.36; the modular device spec, R1 to R11, R17 to R19,
//! R21 to R29): the blocks, cables and patch parameters a Modular device
//! holds, the catalogue of block types with their ports and settings, and
//! the checks a patch passes before it is saved or compiled.
//!
//! The engine compiles a patch into a schedule off the audio thread
//! (`bl-devices`); this module is only the document's side of it, so the
//! set saves it, undo steps it and agents read it back.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ModelError;
use crate::ids::{DeviceId, ParamId};

/// The device type of Modular Instrument (R1).
pub const INSTRUMENT: &str = "beatlayer.modular_instrument";
/// The device type of Modular Effect (R1).
pub const EFFECT: &str = "beatlayer.modular_effect";
/// The device type of Modular MIDI Effect (R1).
pub const MIDI_EFFECT: &str = "beatlayer.modular_midi_effect";

/// The most blocks a patch holds (R11).
pub const MAX_BLOCKS: usize = 256;
/// The most cables a patch holds (R11).
pub const MAX_CABLES: usize = 1024;
/// The most patch parameters a patch holds (R6).
pub const MAX_PARAMS: usize = 64;
/// The most voices an instrument patch plays (R5).
pub const MAX_VOICES: u32 = 32;
/// The parameter that sets how many voices play (R5): 1 to 32, 8 in a new
/// device.
pub const VOICES: ParamId = ParamId(100);

/// The device parameter that patch parameter `slot` (1 to 64) is.
#[must_use]
pub const fn slot_param(slot: u8) -> ParamId {
    ParamId(slot as u32)
}

/// A block's ID within its patch.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct BlockId(pub u32);

/// What a port carries (R2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortType {
    /// A sample per frame.
    Audio,
    /// A value per frame, from −1 to 1 unless the block says otherwise.
    Control,
    /// On above 0.5.
    Gate,
    /// Note and expression events.
    Notes,
}

impl PortType {
    /// Whether a cable can join an output of type `self` to an input of
    /// type `to`: the same type, or control and audio (R2).
    #[must_use]
    pub fn joins(self, to: Self) -> bool {
        self == to
            || matches!(
                (self, to),
                (Self::Audio, Self::Control) | (Self::Control, Self::Audio)
            )
    }
}

/// One port of a block type.
#[derive(Clone, Copy, Debug)]
pub struct PortSpec {
    /// The port's ID within its block, which never changes (R19).
    pub id: &'static str,
    /// What it carries.
    pub ty: PortType,
}

/// One setting of a block type.
#[derive(Clone, Copy, Debug)]
pub struct SettingSpec {
    /// The setting's ID within its block, which never changes (R19).
    pub id: &'static str,
    /// Its name.
    pub name: &'static str,
    /// Its lowest value; 0 for a choice.
    pub min: f64,
    /// Its highest value; the last choice's number for a choice.
    pub max: f64,
    /// The value a new block has.
    pub default: f64,
    /// The text after its number, with its space, or none.
    pub unit: &'static str,
    /// The names of a choice's values, in order; empty for a number.
    pub choices: &'static [&'static str],
}

impl SettingSpec {
    /// Whether the setting is a choice.
    #[must_use]
    pub fn is_choice(&self) -> bool {
        !self.choices.is_empty()
    }

    /// The setting at `travel` (0 to 1) of its range: a choice's nearest
    /// number. A patch parameter's value is a travel (R6).
    #[must_use]
    pub fn at_travel(&self, travel: f64) -> f64 {
        let value = self.min + (self.max - self.min) * travel.clamp(0.0, 1.0);
        if self.is_choice() { value.round() } else { value }
    }

    /// Where `value` sits in the setting's range, 0 to 1.
    #[must_use]
    pub fn travel_of(&self, value: f64) -> f64 {
        if self.max > self.min {
            ((value - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

/// A block type: what its blocks have (R19, R21 to R29).
#[derive(Clone, Copy, Debug)]
pub struct BlockSpec {
    /// The type's ID, which never changes once released (R19).
    pub id: &'static str,
    /// Its name.
    pub name: &'static str,
    /// Its input ports, top to bottom.
    pub inputs: &'static [PortSpec],
    /// Its output ports, top to bottom.
    pub outputs: &'static [PortSpec],
    /// Its settings.
    pub settings: &'static [SettingSpec],
}

impl BlockSpec {
    /// The input port `id`.
    #[must_use]
    pub fn input(&self, id: &str) -> Option<&'static PortSpec> {
        self.inputs.iter().find(|p| p.id == id)
    }

    /// The output port `id`.
    #[must_use]
    pub fn output(&self, id: &str) -> Option<&'static PortSpec> {
        self.outputs.iter().find(|p| p.id == id)
    }

    /// The setting `id`.
    #[must_use]
    pub fn setting(&self, id: &str) -> Option<&'static SettingSpec> {
        self.settings.iter().find(|s| s.id == id)
    }
}

const fn port(id: &'static str, ty: PortType) -> PortSpec {
    PortSpec { id, ty }
}

const fn number(
    id: &'static str,
    name: &'static str,
    min: f64,
    max: f64,
    default: f64,
    unit: &'static str,
) -> SettingSpec {
    SettingSpec {
        id,
        name,
        min,
        max,
        default,
        unit,
        choices: &[],
    }
}

const fn choice(
    id: &'static str,
    name: &'static str,
    choices: &'static [&'static str],
    default: usize,
) -> SettingSpec {
    SettingSpec {
        id,
        name,
        min: 0.0,
        max: (choices.len() - 1) as f64,
        default: default as f64,
        unit: "",
        choices,
    }
}

use PortType::{Audio, Control, Gate, Notes};

/// The block types, in the order the editor lists them (R21 to R29).
pub const BLOCKS: &[BlockSpec] = &[
    BlockSpec {
        id: "oscillator",
        name: "Oscillator",
        inputs: &[port("pitch", Control), port("pm", Audio), port("sync", Gate)],
        outputs: &[port("out", Audio)],
        settings: &[
            choice("wave", "Wave", &["Sine", "Triangle", "Saw", "Square"], 2),
            number("pulse_width", "Pulse Width", 0.01, 0.99, 0.5, ""),
            number("coarse", "Coarse", -48.0, 48.0, 0.0, " st"),
            number("fine", "Fine", -100.0, 100.0, 0.0, " ct"),
        ],
    },
    BlockSpec {
        id: "filter",
        name: "Filter",
        inputs: &[port("in", Audio), port("cutoff", Control)],
        outputs: &[port("out", Audio)],
        settings: &[
            choice(
                "mode",
                "Mode",
                &["Low-pass", "High-pass", "Band-pass", "Notch"],
                0,
            ),
            choice("slope", "Slope", &["12 dB", "24 dB"], 0),
            number("cutoff", "Cutoff", 20.0, 20_000.0, 1000.0, " Hz"),
            number("resonance", "Resonance", 0.0, 1.0, 0.2, ""),
        ],
    },
    BlockSpec {
        id: "envelope",
        name: "Envelope",
        inputs: &[port("gate", Gate)],
        outputs: &[port("out", Control)],
        settings: &[
            number("attack", "Attack", 0.0, 10_000.0, 5.0, " ms"),
            number("decay", "Decay", 0.0, 10_000.0, 200.0, " ms"),
            number("sustain", "Sustain", 0.0, 1.0, 0.7, ""),
            number("release", "Release", 0.0, 10_000.0, 200.0, " ms"),
            number("attack_curve", "Attack Curve", -1.0, 1.0, 0.0, ""),
            number("decay_curve", "Decay Curve", -1.0, 1.0, 0.0, ""),
            number("release_curve", "Release Curve", -1.0, 1.0, 0.0, ""),
        ],
    },
    BlockSpec {
        id: "lfo",
        name: "LFO",
        inputs: &[port("reset", Gate)],
        outputs: &[port("out", Control)],
        settings: &[
            choice(
                "wave",
                "Wave",
                &["Sine", "Triangle", "Saw", "Square", "Random"],
                0,
            ),
            number("rate", "Rate", 0.01, 100.0, 1.0, " Hz"),
            choice("sync", "Sync", &["Off", "On"], 0),
            choice(
                "division",
                "Division",
                &["1/16", "1/8", "1/4", "1/2", "1 bar", "2 bars", "4 bars"],
                2,
            ),
        ],
    },
    BlockSpec {
        id: "amplifier",
        name: "Amplifier",
        inputs: &[port("in", Audio), port("cv", Control)],
        outputs: &[port("out", Audio)],
        settings: &[number("gain", "Gain", -60.0, 24.0, 0.0, " dB")],
    },
    BlockSpec {
        id: "mixer",
        name: "Mixer",
        inputs: &[
            port("in1", Audio),
            port("in2", Audio),
            port("in3", Audio),
            port("in4", Audio),
        ],
        outputs: &[port("out", Audio)],
        settings: &[
            number("level1", "Level 1", 0.0, 1.0, 1.0, ""),
            number("level2", "Level 2", 0.0, 1.0, 1.0, ""),
            number("level3", "Level 3", 0.0, 1.0, 1.0, ""),
            number("level4", "Level 4", 0.0, 1.0, 1.0, ""),
        ],
    },
    BlockSpec {
        id: "arithmetic",
        name: "Arithmetic",
        inputs: &[port("a", Control), port("b", Control)],
        outputs: &[port("out", Control)],
        settings: &[
            choice(
                "op",
                "Operation",
                &[
                    "Add",
                    "Subtract",
                    "Multiply",
                    "Minimum",
                    "Maximum",
                    "Absolute Value",
                    "Constant",
                ],
                0,
            ),
            number("a", "A", -16.0, 16.0, 0.0, ""),
            number("b", "B", -16.0, 16.0, 0.0, ""),
        ],
    },
    BlockSpec {
        id: "delay",
        name: "Delay",
        inputs: &[port("in", Audio), port("time", Control)],
        outputs: &[port("out", Audio)],
        settings: &[number("time", "Time", 0.0, 4000.0, 100.0, " ms")],
    },
    BlockSpec {
        id: "note_in",
        name: "Note In",
        inputs: &[],
        outputs: &[
            port("pitch", Control),
            port("velocity", Control),
            port("gate", Gate),
            port("pressure", Control),
            port("slide", Control),
            port("notes", Notes),
        ],
        settings: &[],
    },
    BlockSpec {
        id: "note_out",
        name: "Note Out",
        inputs: &[port("notes", Notes)],
        outputs: &[],
        settings: &[],
    },
    BlockSpec {
        id: "audio_in",
        name: "Audio In",
        inputs: &[],
        outputs: &[port("left", Audio), port("right", Audio)],
        settings: &[],
    },
    BlockSpec {
        id: "audio_out",
        name: "Audio Out",
        inputs: &[port("left", Audio), port("right", Audio)],
        outputs: &[],
        settings: &[],
    },
    BlockSpec {
        id: "sidechain_in",
        name: "Sidechain In",
        inputs: &[],
        outputs: &[port("left", Audio), port("right", Audio)],
        settings: &[],
    },
];

/// The block type `id`, if Beatlayer knows it (R10).
#[must_use]
pub fn block_spec(id: &str) -> Option<&'static BlockSpec> {
    BLOCKS.iter().find(|b| b.id == id)
}

/// One block of a patch (R2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Block {
    /// The block's ID in the patch.
    pub id: BlockId,
    /// Its type's ID (R19).
    #[serde(rename = "type")]
    pub kind: String,
    /// Where it sits in the editor.
    pub x: f32,
    /// Where it sits in the editor.
    pub y: f32,
    /// The settings that aren't the type's defaults, by setting ID.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub settings: BTreeMap<String, f64>,
}

impl Block {
    /// The block's type, if Beatlayer knows it.
    #[must_use]
    pub fn spec(&self) -> Option<&'static BlockSpec> {
        block_spec(&self.kind)
    }

    /// The value of setting `id`: its own, else the type's default.
    #[must_use]
    pub fn setting(&self, id: &str) -> Option<f64> {
        let spec = self.spec()?.setting(id)?;
        Some(self.settings.get(id).copied().unwrap_or(spec.default))
    }
}

/// A cable from one block's output port to another's input port (R2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Cable {
    /// The block it leaves.
    pub from: BlockId,
    /// The output port it leaves, by ID.
    pub from_port: String,
    /// The block it enters.
    pub to: BlockId,
    /// The input port it enters, by ID.
    pub to_port: String,
}

impl Cable {
    /// The cable from `from_port` of `from` to `to_port` of `to`.
    #[must_use]
    pub fn of(from: BlockId, from_port: &str, to: BlockId, to_port: &str) -> Self {
        Self {
            from,
            from_port: from_port.to_owned(),
            to,
            to_port: to_port.to_owned(),
        }
    }
}

/// A setting made a patch parameter (R6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PatchParam {
    /// Which of the device's 64 patch parameters it is, from 1: its
    /// parameter number is the same.
    pub slot: u8,
    /// The block.
    pub block: BlockId,
    /// The setting's ID.
    pub setting: String,
}

/// A Modular device's patch (R2).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Patch {
    /// The blocks, in the order they were added.
    pub blocks: Vec<Block>,
    /// The cables.
    pub cables: Vec<Cable>,
    /// The patch parameters, by slot.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<PatchParam>,
}

impl Patch {
    /// The block `id`.
    #[must_use]
    pub fn block(&self, id: BlockId) -> Option<&Block> {
        self.blocks.iter().find(|b| b.id == id)
    }

    /// How many blocks Beatlayer doesn't know (R10).
    #[must_use]
    pub fn unknown_blocks(&self) -> usize {
        self.blocks.iter().filter(|b| b.spec().is_none()).count()
    }

    /// The patch parameter that is `block`'s `setting`, if it is one.
    #[must_use]
    pub fn param_of(&self, block: BlockId, setting: &str) -> Option<&PatchParam> {
        self.params
            .iter()
            .find(|p| p.block == block && p.setting == setting)
    }

    /// The new patch of a device of type `device_type` (R1), or `None` for
    /// a type that isn't Modular's.
    #[must_use]
    pub fn new_for(device_type: &str) -> Option<Self> {
        let block = |id, kind: &str, x, y| Block {
            id: BlockId(id),
            kind: kind.to_owned(),
            x,
            y,
            settings: BTreeMap::new(),
        };
        let cable = |from, from_port: &str, to, to_port: &str| Cable {
            from: BlockId(from),
            from_port: from_port.to_owned(),
            to: BlockId(to),
            to_port: to_port.to_owned(),
        };
        match device_type {
            INSTRUMENT => Some(Self {
                blocks: vec![
                    block(1, "note_in", 0.0, 0.0),
                    block(2, "oscillator", 200.0, 0.0),
                    block(3, "amplifier", 400.0, 0.0),
                    block(4, "envelope", 200.0, 160.0),
                    block(5, "audio_out", 600.0, 0.0),
                ],
                cables: vec![
                    cable(1, "pitch", 2, "pitch"),
                    cable(2, "out", 3, "in"),
                    cable(1, "gate", 4, "gate"),
                    cable(4, "out", 3, "cv"),
                    cable(3, "out", 5, "left"),
                    cable(3, "out", 5, "right"),
                ],
                params: Vec::new(),
            }),
            EFFECT => Some(Self {
                blocks: vec![
                    block(1, "audio_in", 0.0, 0.0),
                    block(2, "audio_out", 300.0, 0.0),
                ],
                cables: vec![cable(1, "left", 2, "left"), cable(1, "right", 2, "right")],
                params: Vec::new(),
            }),
            MIDI_EFFECT => Some(Self {
                blocks: vec![
                    block(1, "note_in", 0.0, 0.0),
                    block(2, "note_out", 300.0, 0.0),
                ],
                cables: vec![cable(1, "notes", 2, "notes")],
                params: Vec::new(),
            }),
            _ => None,
        }
    }

    /// Whether the patch can be saved and compiled: within R11's limits,
    /// its blocks' settings in range, its cables joining ports of types R2
    /// lets join, an input with one cable, no cycle but through a Delay
    /// (R3), and its patch parameters made from settings the blocks have
    /// (R6). Blocks Beatlayer doesn't know keep their cables unchecked
    /// (R10).
    ///
    /// # Errors
    ///
    /// The first rule that fails.
    pub fn check(&self) -> Result<(), ModelError> {
        let bad = ModelError::InvalidInput;
        if self.blocks.len() > MAX_BLOCKS {
            return Err(bad("a patch has at most 256 blocks"));
        }
        if self.cables.len() > MAX_CABLES {
            return Err(bad("a patch has at most 1,024 cables"));
        }
        let mut ids = BTreeSet::new();
        for block in &self.blocks {
            if !ids.insert(block.id) {
                return Err(bad("two blocks have the same ID"));
            }
            if !block.x.is_finite() || !block.y.is_finite() {
                return Err(bad("a block's place is a finite number"));
            }
            let Some(spec) = block.spec() else { continue };
            for (id, value) in &block.settings {
                let Some(setting) = spec.setting(id) else {
                    return Err(bad("a block has a setting its type doesn't"));
                };
                if !(setting.min..=setting.max).contains(value)
                    || (setting.is_choice() && value.fract() != 0.0)
                {
                    return Err(bad("a block's setting is outside its range"));
                }
            }
        }
        let mut entered = BTreeSet::new();
        for cable in &self.cables {
            let (Some(from), Some(to)) = (self.block(cable.from), self.block(cable.to)) else {
                return Err(bad("a cable joins a block the patch doesn't have"));
            };
            if !entered.insert((cable.to, cable.to_port.as_str())) {
                return Err(bad("an input takes one cable"));
            }
            let (Some(from), Some(to)) = (from.spec(), to.spec()) else {
                continue;
            };
            let (Some(out), Some(input)) = (
                from.output(&cable.from_port),
                to.input(&cable.to_port),
            ) else {
                return Err(bad("a cable joins a port its block doesn't have"));
            };
            if !out.ty.joins(input.ty) {
                return Err(bad("a cable joins ports of types that don't join"));
            }
        }
        if self.has_cycle() {
            return Err(bad("a cycle of cables needs a Delay block in it"));
        }
        let mut slots = BTreeSet::new();
        let mut settings = BTreeSet::new();
        for param in &self.params {
            if !(1..=MAX_PARAMS as u8).contains(&param.slot)
                || !slots.insert(param.slot)
                || !settings.insert((param.block, param.setting.as_str()))
            {
                return Err(bad("a patch parameter's slot and setting are used once"));
            }
            if self
                .block(param.block)
                .and_then(|b| b.spec()?.setting(&param.setting))
                .is_none()
            {
                return Err(bad("a patch parameter is made from a setting that exists"));
            }
        }
        Ok(())
    }

    /// Whether the cables close a cycle that doesn't pass a Delay block's
    /// audio input (R3): a Delay's output reads what went into it before,
    /// so a cable into that input makes no edge.
    #[must_use]
    pub fn has_cycle(&self) -> bool {
        let delayed = |c: &Cable| {
            c.to_port == "in" && self.block(c.to).is_some_and(|b| b.kind == "delay")
        };
        let mut next: BTreeMap<BlockId, Vec<BlockId>> = BTreeMap::new();
        for cable in self.cables.iter().filter(|c| !delayed(c)) {
            next.entry(cable.from).or_default().push(cable.to);
        }
        // Depth-first: 1 while on the path, 2 once finished.
        fn visit(
            id: BlockId,
            next: &BTreeMap<BlockId, Vec<BlockId>>,
            state: &mut BTreeMap<BlockId, u8>,
        ) -> bool {
            match state.get(&id) {
                Some(1) => return true,
                Some(_) => return false,
                None => {}
            }
            state.insert(id, 1);
            if next
                .get(&id)
                .is_some_and(|to| to.iter().any(|&t| visit(t, next, state)))
            {
                return true;
            }
            state.insert(id, 2);
            false
        }
        let mut state = BTreeMap::new();
        self.blocks.iter().any(|b| visit(b.id, &next, &mut state))
    }
}

impl crate::Document {
    /// Changes `device`'s patch with `edit`, which may also move its
    /// parameter values; the patch is checked after, and nothing changes if
    /// it fails.
    fn edit_patch(
        &mut self,
        device: DeviceId,
        edit: impl FnOnce(&mut Patch, &mut Vec<(ParamId, Option<f64>)>) -> Result<(), ModelError>,
    ) -> Result<(), ModelError> {
        let mut patch = self
            .device_ref(device)?
            .patch
            .clone()
            .ok_or(ModelError::InvalidInput("the device isn't a Modular"))?;
        let mut moves = Vec::new();
        edit(&mut patch, &mut moves)?;
        patch.check()?;
        let target = self.device_mut(device)?;
        for (param, value) in moves {
            match value {
                Some(value) => target.params.insert(param, value),
                None => target.params.remove(&param),
            };
        }
        if target.patch.as_ref() != Some(&patch) {
            target.patch = Some(patch);
        }
        Ok(())
    }

    /// Adds a block of type `kind` (R9). `id` is the block's ID, the next
    /// free one if left out.
    pub(crate) fn modular_add_block(
        &mut self,
        device: DeviceId,
        kind: &str,
        id: Option<BlockId>,
        x: f32,
        y: f32,
    ) -> Result<(), ModelError> {
        if block_spec(kind).is_none() {
            return Err(ModelError::InvalidInput("Modular has no such block type"));
        }
        self.edit_patch(device, |patch, _| {
            let id = id.unwrap_or_else(|| {
                BlockId(patch.blocks.iter().map(|b| b.id.0).max().map_or(1, |m| m + 1))
            });
            patch.blocks.push(Block {
                id,
                kind: kind.to_owned(),
                x,
                y,
                settings: BTreeMap::new(),
            });
            Ok(())
        })
    }

    /// Removes a block, its cables and its patch parameters (R9).
    pub(crate) fn modular_remove_block(
        &mut self,
        device: DeviceId,
        block: BlockId,
    ) -> Result<(), ModelError> {
        self.edit_patch(device, |patch, moves| {
            let before = patch.blocks.len();
            patch.blocks.retain(|b| b.id != block);
            if patch.blocks.len() == before {
                return Err(ModelError::InvalidInput("the patch has no such block"));
            }
            patch.cables.retain(|c| c.from != block && c.to != block);
            patch.params.retain(|p| {
                let gone = p.block == block;
                if gone {
                    moves.push((slot_param(p.slot), None));
                }
                !gone
            });
            Ok(())
        })
    }

    /// Joins an output port to an input port (R9, R2, R3).
    pub(crate) fn modular_connect(
        &mut self,
        device: DeviceId,
        cable: &Cable,
    ) -> Result<(), ModelError> {
        self.edit_patch(device, |patch, _| {
            if patch.cables.contains(cable) {
                return Err(ModelError::InvalidInput("those ports are already joined"));
            }
            patch.cables.push(cable.clone());
            Ok(())
        })
    }

    /// Takes a cable away (R9).
    pub(crate) fn modular_disconnect(
        &mut self,
        device: DeviceId,
        cable: &Cable,
    ) -> Result<(), ModelError> {
        self.edit_patch(device, |patch, _| {
            let before = patch.cables.len();
            patch.cables.retain(|c| c != cable);
            if patch.cables.len() == before {
                return Err(ModelError::InvalidInput("those ports aren't joined"));
            }
            Ok(())
        })
    }

    /// Moves a block in the editor (R9, R20).
    pub(crate) fn modular_move_block(
        &mut self,
        device: DeviceId,
        block: BlockId,
        x: f32,
        y: f32,
    ) -> Result<(), ModelError> {
        self.edit_patch(device, |patch, _| {
            let b = patch
                .blocks
                .iter_mut()
                .find(|b| b.id == block)
                .ok_or(ModelError::InvalidInput("the patch has no such block"))?;
            (b.x, b.y) = (x, y);
            Ok(())
        })
    }

    /// Sets a block's setting (R9); a setting that is a patch parameter
    /// moves the parameter with it.
    pub(crate) fn modular_set(
        &mut self,
        device: DeviceId,
        block: BlockId,
        setting: &str,
        value: f64,
    ) -> Result<(), ModelError> {
        self.edit_patch(device, |patch, moves| {
            let slot = patch.param_of(block, setting).map(|p| p.slot);
            let b = patch
                .blocks
                .iter_mut()
                .find(|b| b.id == block)
                .ok_or(ModelError::InvalidInput("the patch has no such block"))?;
            let spec = b
                .spec()
                .and_then(|s| s.setting(setting))
                .ok_or(ModelError::InvalidInput("the block has no such setting"))?;
            if !(spec.min..=spec.max).contains(&value) {
                return Err(ModelError::InvalidInput("the setting is outside its range"));
            }
            if value == spec.default {
                b.settings.remove(setting);
            } else {
                b.settings.insert(setting.to_owned(), value);
            }
            if let Some(slot) = slot {
                moves.push((slot_param(slot), Some(spec.travel_of(value))));
            }
            Ok(())
        })
    }

    /// Makes a setting a patch parameter, or stops (R6, R9).
    pub(crate) fn modular_expose(
        &mut self,
        device: DeviceId,
        block: BlockId,
        setting: &str,
        exposed: bool,
    ) -> Result<(), ModelError> {
        self.edit_patch(device, |patch, moves| {
            let spec = patch
                .block(block)
                .and_then(|b| b.spec()?.setting(setting))
                .ok_or(ModelError::InvalidInput("the block has no such setting"))?;
            let current = patch.param_of(block, setting).map(|p| p.slot);
            match (exposed, current) {
                (true, None) => {
                    let slot = (1..=MAX_PARAMS as u8)
                        .find(|s| patch.params.iter().all(|p| p.slot != *s))
                        .ok_or(ModelError::InvalidInput("a patch has at most 64 parameters"))?;
                    let value = patch.block(block).and_then(|b| b.setting(setting));
                    moves.push((
                        slot_param(slot),
                        Some(spec.travel_of(value.unwrap_or(spec.default))),
                    ));
                    patch.params.push(PatchParam {
                        slot,
                        block,
                        setting: setting.to_owned(),
                    });
                    patch.params.sort_by_key(|p| p.slot);
                }
                (false, Some(slot)) => {
                    patch.params.retain(|p| p.slot != slot);
                    moves.push((slot_param(slot), None));
                }
                _ => {}
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch() -> Patch {
        Patch::new_for(INSTRUMENT).unwrap()
    }

    // Spec: modular-device R1
    #[test]
    fn a_new_patch_of_each_kind_passes_its_checks() {
        for kind in [INSTRUMENT, EFFECT, MIDI_EFFECT] {
            Patch::new_for(kind).unwrap().check().unwrap();
        }
        assert!(Patch::new_for("beatlayer.utility").is_none());
    }

    // Spec: modular-device R2
    #[test]
    fn a_cable_joins_ports_of_one_type_or_control_and_audio_and_an_input_takes_one() {
        let mut p = patch();
        let cable = |from, fp: &str, to, tp: &str| Cable {
            from: BlockId(from),
            from_port: fp.to_owned(),
            to: BlockId(to),
            to_port: tp.to_owned(),
        };
        // Control into audio, and audio into control.
        let mut ok = p.clone();
        ok.cables.push(cable(1, "velocity", 2, "pm"));
        ok.cables.push(cable(2, "out", 4, "gate").clone());
        assert!(ok.check().is_err(), "audio does not join a gate");
        ok.cables.pop();
        ok.check().unwrap();
        // Notes join notes only.
        p.cables.push(cable(1, "notes", 3, "in"));
        assert!(p.check().is_err());
        // The oscillator's pitch already has a cable.
        let mut twice = patch();
        twice.cables.push(cable(1, "velocity", 2, "pitch"));
        assert!(twice.check().is_err());
    }

    // Spec: modular-device R3
    #[test]
    fn a_cycle_is_allowed_only_through_a_delay() {
        let mut p = Patch::new_for(EFFECT).unwrap();
        let block = |id, kind: &str| Block {
            id: BlockId(id),
            kind: kind.to_owned(),
            x: 0.0,
            y: 0.0,
            settings: BTreeMap::new(),
        };
        let cable = |from, fp: &str, to, tp: &str| Cable {
            from: BlockId(from),
            from_port: fp.to_owned(),
            to: BlockId(to),
            to_port: tp.to_owned(),
        };
        p.blocks.push(block(3, "amplifier"));
        p.blocks.push(block(4, "amplifier"));
        p.cables.push(cable(3, "out", 4, "in"));
        p.cables.push(cable(4, "out", 3, "in"));
        assert!(p.has_cycle() && p.check().is_err());
        p.blocks[3].kind = "delay".to_owned();
        assert!(!p.has_cycle());
        p.check().unwrap();
    }

    // Spec: modular-device R11
    #[test]
    fn a_patch_has_at_most_256_blocks_and_1024_cables() {
        let mut p = Patch::default();
        for id in 0..=256 {
            p.blocks.push(Block {
                id: BlockId(id),
                kind: "mixer".to_owned(),
                x: 0.0,
                y: 0.0,
                settings: BTreeMap::new(),
            });
        }
        assert!(p.check().is_err());
        p.blocks.pop();
        p.check().unwrap();
    }

    // Spec: modular-device R10
    #[test]
    fn a_block_of_an_unknown_type_keeps_its_cables() {
        let mut p = patch();
        p.blocks.push(Block {
            id: BlockId(9),
            kind: "from_the_future".to_owned(),
            x: 0.0,
            y: 0.0,
            settings: BTreeMap::new(),
        });
        p.cables.push(Cable {
            from: BlockId(9),
            from_port: "anything".to_owned(),
            to: BlockId(3),
            to_port: "in".to_owned(),
        });
        // The amplifier's input already has a cable from the oscillator.
        assert!(p.check().is_err());
        p.cables.pop();
        p.cables.push(Cable {
            from: BlockId(9),
            from_port: "anything".to_owned(),
            to: BlockId(4),
            to_port: "gate".to_owned(),
        });
        p.cables.retain(|c| !(c.to == BlockId(4)));
        p.check().unwrap();
        assert_eq!(p.unknown_blocks(), 1);
    }
}

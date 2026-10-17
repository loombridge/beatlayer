//! Kitbag, the eight-slot drum sampler (WP-5.17; the eight-slot drum sampler
//! spec): eight samples, each played by its own key, each with its own
//! start, transpose, stretch, filter, saturator, decay, pan and volume, and
//! a link that lets slot 7 and slot 8 cut each other off.
//!
//! Each slot's sample is decoded audio the media crate gives the engine, sent
//! to this node as node data (the sampler spec, R11). A note copies its
//! slot's parameters when it starts, so a changed parameter reaches the next
//! note and leaves the sounding one alone.

use std::f64::consts::PI;

use bl_core::time::SampleRate;
use bl_engine::{DeviceType, NodeConfig, NodeData, NoteEvent, NoteKind, Registry, SlotSample};
use bl_media::SampleData;
use bl_model::device_layout::{
    BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, SlotGroup, Widget,
};
use bl_model::{
    Device, DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};
use lspk_dsp::filter::{FilterBank, VoiceParams, pitch_of};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::sampler::{FILTER_TYPES, filter_type_of, read};

/// The drum sampler device type's id.
pub const KITBAG: &str = "beatlayer.kitbag";

/// How many sample slots it has.
pub const SLOTS: usize = 8;

/// The key each slot plays: the eight white keys from C3 to C4, the
/// octave the manual puts the slots in, slot 1 on C3.
pub const KEYS: [u8; SLOTS] = [60, 62, 64, 65, 67, 69, 71, 72];

/// How far apart one slot's parameter numbers are from the next's.
pub const STRIDE: u32 = 32;

/// A parameter of slot `slot` (0 to 7): `local` is one of the numbers in
/// [`slot`].
#[must_use]
pub const fn param(slot: usize, local: u32) -> ParamId {
    ParamId(slot as u32 * STRIDE + local)
}

/// A slot parameter's number within its slot; [`param`] puts it in a slot.
pub mod slot {
    /// Where playback starts, in milliseconds into the sample.
    pub const START: u32 = 0;
    /// Transposes the slot, in whole semitones.
    pub const TRANSPOSE: u32 = 1;
    /// How far note velocity moves the transposition.
    pub const TRANSPOSE_VELOCITY: u32 = 2;
    /// How far a random value moves the transposition.
    pub const TRANSPOSE_RANDOM: u32 = 3;
    /// Shortens or lengthens the sample without changing its pitch.
    pub const STRETCH: u32 = 4;
    /// Which of the two stretch algorithms runs.
    pub const STRETCH_MODE: u32 = 5;
    /// How far note velocity moves the stretch.
    pub const STRETCH_VELOCITY: u32 = 6;
    /// Whether the slot's filter runs.
    pub const FILTER: u32 = 7;
    /// The filter's type.
    pub const FILTER_TYPE: u32 = 8;
    /// The filter's cutoff frequency.
    pub const FILTER_FREQ: u32 = 9;
    /// The filter's resonance.
    pub const FILTER_RES: u32 = 10;
    /// How far note velocity moves the cutoff.
    pub const FILTER_VELOCITY: u32 = 11;
    /// How far a random value moves the cutoff.
    pub const FILTER_RANDOM: u32 = 12;
    /// Whether the slot's saturator runs.
    pub const SATURATOR: u32 = 13;
    /// How hard the saturator is driven.
    pub const DRIVE: u32 = 14;
    /// How long the sample takes to fall to silence.
    pub const DECAY: u32 = 15;
    /// Whether the decay starts with the note (Trigger) or at its end (Gate).
    pub const DECAY_MODE: u32 = 16;
    /// Where the slot sits in the stereo field.
    pub const PAN: u32 = 17;
    /// How far note velocity moves the pan.
    pub const PAN_VELOCITY: u32 = 18;
    /// How far a random value moves the pan.
    pub const PAN_RANDOM: u32 = 19;
    /// The slot's level.
    pub const VOLUME: u32 = 20;
    /// How much note velocity scales the level.
    pub const VOLUME_VELOCITY: u32 = 21;
    /// Whether the slot is silenced.
    pub const MUTE: u32 = 22;
    /// Whether the slot is the only one that sounds (with the other soloed
    /// slots).
    pub const SOLO: u32 = 23;
    /// How many slot parameters there are.
    pub const COUNT: u32 = 24;
}

/// The whole instrument's output level.
pub const VOLUME: ParamId = ParamId(256);
/// Transposes every slot, in whole semitones.
pub const TRANSPOSE: ParamId = ParamId(257);
/// Scales every slot's stretch and decay.
pub const TIME: ParamId = ParamId(258);
/// Whether slot 7 and slot 8 cut each other off.
pub const LINK: ParamId = ParamId(259);

/// The stretch modes: A for low sounds, B for high ones (the manual).
pub const STRETCH_MODES: [&str; 2] = ["A", "B"];
/// The decay modes.
pub const DECAY_MODES: [&str; 2] = ["Trigger", "Gate"];

/// Stretch's whole range: a stretch of 100 % doubles the sample's length and
/// −100 % halves it (Beatlayer's own).
const STRETCH_OCTAVES: f64 = 1.0;
/// How far full velocity or random amounts move each target: semitones for
/// the transposition, per cent of stretch, octaves of cutoff and units of
/// pan (Beatlayer's own).
const TRANSPOSE_SPAN: f64 = 24.0;
const STRETCH_SPAN: f64 = 100.0;
const FILTER_SPAN_OCTAVES: f64 = 4.0;
const PAN_SPAN: f64 = 50.0;
/// The most Start can skip, in milliseconds (the manual).
const START_MAX_MS: f64 = 100.0;
/// The longest decay, in milliseconds (the manual).
const DECAY_MAX_MS: f64 = 10_000.0;
/// How long a cut or retriggered voice fades out, in seconds (Beatlayer's
/// own).
const CUT_FADE_SECONDS: f64 = 0.005;
/// The stretch algorithms' grain lengths, in seconds (Beatlayer's own): A's
/// long grains keep low sounds whole, B's short ones follow the quick changes
/// of high ones.
const GRAIN_A_SECONDS: f64 = 0.040;
const GRAIN_B_SECONDS: f64 = 0.012;
/// How many source frames a grain's start is looked for around where the
/// stretch wants it, as a share of the grain's length, and how many frames
/// apart the candidates are.
const SEARCH_SHARE: f64 = 0.25;
const SEARCH_STEP: usize = 2;
/// How many frames the waveform match compares, and how far apart.
const MATCH_FRAMES: usize = 256;
const MATCH_STEP: usize = 4;
/// The most the whole stretch can scale a length by either way.
const STRETCH_LIMIT: f64 = 8.0;
/// The filter's pitches count from A4 at this frequency.
const REFERENCE_HZ: f32 = 440.0;
/// The drive at its top, in decibels.
const DRIVE_DB: f64 = 24.0;
/// The seed of the generator the random amounts draw from, mixed with the
/// device's ID so a render repeats.
const SEED: u32 = 0x2545_F491;

/// Voice entries: two per slot, so a retriggered slot's old voice can fade
/// out while its new one starts.
const VOICES: usize = SLOTS * 2;

/// Where slot `slot`'s parameter `local` sits in the parameter bank: the
/// description lists every slot's parameters in order, then the globals.
const fn bank_index(slot: usize, local: u32) -> usize {
    slot * slot::COUNT as usize + local as usize
}

/// Where the global parameter `id` sits in the parameter bank.
const fn global_index(id: ParamId) -> usize {
    SLOTS * slot::COUNT as usize + (id.0 - VOLUME.0) as usize
}

/// The device type.
#[must_use]
pub fn kitbag_type() -> DeviceType {
    let mut params = Vec::new();
    for s in 0..SLOTS {
        slot_params(s, &mut params);
    }
    params.extend([
        ParamDescriptor::gain(VOLUME, "Volume", 6.0, 0.0)
            .describe("The whole instrument's output level; reaches silence at its bottom."),
        ParamDescriptor::integer(TRANSPOSE, "Transpose", (-48, 48), 0, ParamUnit::Semitones)
            .describe("Transposes every slot, in whole semitones, from the next note."),
        ParamDescriptor::continuous(TIME, "Time", (25.0, 400.0), 100.0, ParamUnit::Percent)
            .logarithmic()
            .describe(
                "Scales every slot's stretch and decay time, from the next note: below 100 % \
                 the sounds get shorter, above they get longer.",
            ),
        ParamDescriptor::toggle(LINK, "Link", false).describe(
            "Slot 7 stops slot 8's playback, and slot 8 stops slot 7's, as a closed hi-hat \
             stops an open one.",
        ),
    ]);
    DeviceType::native(KITBAG, "Kitbag", DeviceRole::Instrument)
        .with_description(DeviceDescription::new(params).with_layout(layout()))
}

/// Slot `s`'s parameters, named `Slot <n> <what>`.
fn slot_params(s: usize, params: &mut Vec<ParamDescriptor>) {
    use slot::*;
    let n = s + 1;
    let name = |what: &str| format!("Slot {n} {what}");
    let id = |local| param(s, local);
    let percent = |local, what: &str, range: (f64, f64), default: f64, text: &str| {
        ParamDescriptor::continuous(id(local), &name(what), range, default, ParamUnit::Percent)
            .describe(text)
    };
    params.extend([
        ParamDescriptor::continuous(
            id(START),
            &name("Start"),
            (0.0, START_MAX_MS),
            0.0,
            ParamUnit::Milliseconds,
        )
        .describe("How far into the sample playback starts, up to 100 ms."),
        ParamDescriptor::integer(
            id(TRANSPOSE),
            &name("Transpose"),
            (-48, 48),
            0,
            ParamUnit::Semitones,
        )
        .describe("Transposes the slot, in whole semitones; it changes the speed too."),
        percent(
            TRANSPOSE_VELOCITY,
            "Transpose Velocity",
            (-100.0, 100.0),
            0.0,
            "How far note velocity moves the transposition: at 100 % the softest note plays \
             24 semitones below the hardest.",
        ),
        percent(
            TRANSPOSE_RANDOM,
            "Transpose Random",
            (0.0, 100.0),
            0.0,
            "How far a random value moves the transposition, up to 24 semitones each way.",
        ),
        percent(
            STRETCH,
            "Stretch",
            (-100.0, 100.0),
            0.0,
            "Shortens (below 0) or lengthens (above) the sample without changing its pitch: \
             at 100 % it plays twice as long, at −100 % half as long.",
        ),
        ParamDescriptor::choice(id(STRETCH_MODE), &name("Stretch Mode"), &STRETCH_MODES, 0)
            .describe("A suits low sounds such as toms and bass, B high ones such as cymbals."),
        percent(
            STRETCH_VELOCITY,
            "Stretch Velocity",
            (-100.0, 100.0),
            0.0,
            "How far note velocity moves the stretch: at 100 % the softest note is 100 % of \
             stretch below the hardest.",
        ),
        ParamDescriptor::toggle(id(FILTER), &name("Filter"), false)
            .describe("Runs the slot through its filter; off bypasses it."),
        ParamDescriptor::choice(id(FILTER_TYPE), &name("Filter Type"), &FILTER_TYPES, 0)
            .describe("The filter's response: low-pass, high-pass, band-pass or notch."),
        ParamDescriptor::continuous(
            id(FILTER_FREQ),
            &name("Filter Freq"),
            (20.0, 20_000.0),
            1_000.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The filter's cutoff frequency."),
        percent(
            FILTER_RES,
            "Filter Res",
            (0.0, 100.0),
            0.0,
            "Boosts frequencies near the cutoff.",
        ),
        percent(
            FILTER_VELOCITY,
            "Filter Velocity",
            (-100.0, 100.0),
            0.0,
            "How far note velocity moves the cutoff: at 100 % the softest note is four \
             octaves below the hardest.",
        ),
        percent(
            FILTER_RANDOM,
            "Filter Random",
            (0.0, 100.0),
            0.0,
            "How far a random value moves the cutoff, up to four octaves each way.",
        ),
        ParamDescriptor::toggle(id(SATURATOR), &name("Saturator"), false)
            .describe("Runs the slot through its saturator; off bypasses it."),
        percent(
            DRIVE,
            "Drive",
            (0.0, 100.0),
            0.0,
            "Boosts the signal into the saturator, up to 24 dB, and adds warmth and distortion.",
        ),
        ParamDescriptor::continuous(
            id(DECAY),
            &name("Decay"),
            (1.0, DECAY_MAX_MS),
            DECAY_MAX_MS,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("How long the sample takes to fall to silence, up to ten seconds."),
        ParamDescriptor::choice(id(DECAY_MODE), &name("Decay Mode"), &DECAY_MODES, 0).describe(
            "Trigger starts the decay with the note; Gate waits for the note's end, which \
             suits sounds of variable length such as hi-hats.",
        ),
        ParamDescriptor::continuous(id(PAN), &name("Pan"), (-50.0, 50.0), 0.0, ParamUnit::None)
            .decimals(0)
            .describe("Where the slot sits in the stereo field."),
        percent(
            PAN_VELOCITY,
            "Pan Velocity",
            (-100.0, 100.0),
            0.0,
            "How far note velocity moves the pan: at 100 % the softest note sits 50 steps \
             left of the hardest.",
        ),
        percent(
            PAN_RANDOM,
            "Pan Random",
            (0.0, 100.0),
            0.0,
            "How far a random value moves the pan, up to 50 steps each way.",
        ),
        ParamDescriptor::gain(id(VOLUME), &name("Volume"), 6.0, 0.0)
            .describe("The slot's level; reaches silence at its bottom."),
        percent(
            VOLUME_VELOCITY,
            "Volume Velocity",
            (0.0, 100.0),
            100.0,
            "How much note velocity scales the level: at 0 it doesn't, at 100 % the level \
             follows it in a straight line.",
        ),
        ParamDescriptor::toggle(id(MUTE), &name("Mute"), false)
            .describe("Silences the slot from the next note."),
        ParamDescriptor::toggle(id(SOLO), &name("Solo"), false)
            .describe("Plays only the soloed slots from the next note, and silences the others."),
    ]);
}

/// The panel: the eight slots along the top as cells, the selected slot's
/// controls under them, the globals beside.
fn layout() -> DeviceLayout {
    use slot::*;
    let cell_height = 36.0;
    let cells: Vec<LayoutNode> = (0..SLOTS)
        .map(|s| {
            LayoutNode::Cell(
                DisplayCell::new(&format!("Slot {}", s + 1), 64.0, cell_height, 1.0)
                    .for_slot(s as u32),
            )
        })
        .collect();
    let controls = [
        (START, Widget::Knob),
        (TRANSPOSE, Widget::Number),
        (TRANSPOSE_VELOCITY, Widget::Knob),
        (TRANSPOSE_RANDOM, Widget::Knob),
        (STRETCH, Widget::Knob),
        (STRETCH_MODE, Widget::Chooser),
        (STRETCH_VELOCITY, Widget::Knob),
        (FILTER, Widget::Toggle),
        (FILTER_TYPE, Widget::Chooser),
        (FILTER_FREQ, Widget::Knob),
        (FILTER_RES, Widget::Knob),
        (FILTER_VELOCITY, Widget::Knob),
        (FILTER_RANDOM, Widget::Knob),
        (SATURATOR, Widget::Toggle),
        (DRIVE, Widget::Knob),
        (DECAY, Widget::Knob),
        (DECAY_MODE, Widget::Chooser),
        (PAN, Widget::Knob),
        (PAN_VELOCITY, Widget::Knob),
        (PAN_RANDOM, Widget::Knob),
        (VOLUME, Widget::Knob),
        (VOLUME_VELOCITY, Widget::Knob),
        (MUTE, Widget::Toggle),
        (SOLO, Widget::Toggle),
    ]
    .map(|(local, widget)| (ParamId(local), widget));
    let selected =
        ControlBand::stacked(BODY_HEIGHT - cell_height, &controls).for_slots(SlotGroup {
            stride: STRIDE,
            count: SLOTS as u32,
        });
    let globals = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (self::VOLUME, Widget::Knob),
            (self::TRANSPOSE, Widget::Number),
            (TIME, Widget::Knob),
            (LINK, Widget::Toggle),
        ],
    );
    DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Column(vec![LayoutNode::Row(cells), LayoutNode::Band(selected)]),
        LayoutNode::Band(globals),
    ]))
}

/// Adds the drum sampler to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, kitbag_type(), Kitbag::new);
}

/// A slot's parameters as one note starts: what its voice plays by.
#[derive(Clone, Copy)]
struct Slot {
    start: f64,
    transpose: f64,
    transpose_velocity: f64,
    transpose_random: f64,
    stretch: f64,
    stretch_b: bool,
    stretch_velocity: f64,
    filter: bool,
    filter_type: usize,
    filter_freq: f64,
    filter_res: f64,
    filter_velocity: f64,
    filter_random: f64,
    saturator: bool,
    drive: f64,
    decay_ms: f64,
    gate: bool,
    pan: f64,
    pan_velocity: f64,
    pan_random: f64,
    volume: f64,
    volume_velocity: f64,
}

/// One grain of a stretched voice: where it reads and how far through its
/// window it is.
#[derive(Clone, Copy, Default)]
struct Grain {
    active: bool,
    position: f64,
    age: u32,
}

/// One voice: a note's copy of its slot's parameters and its place in the
/// sample.
#[derive(Clone, Copy, Default)]
struct Voice {
    active: bool,
    /// Fading out because the slot was retriggered or cut.
    cutting: bool,
    cut_left: u32,
    cut_total: u32,
    slot: usize,
    /// When it started: the older of a slot's two voices is reused first.
    serial: u64,
    /// The sample frame it ends at.
    end: f64,
    /// Source frames per output frame: the transposition's ratio.
    ratio: f64,
    /// Direct playback's position, in source frames.
    position: f64,
    /// Whether the stretch algorithm runs; direct playback otherwise.
    granular: bool,
    /// The stretch's content position, in source frames.
    anchor: f64,
    /// How far the anchor moves per output frame.
    advance: f64,
    grains: [Grain; 2],
    grain_length: u32,
    /// Output frames until the next grain starts.
    until_grain: u32,
    /// The decay: the level, its step per frame, and whether it's running.
    level: f32,
    step: f32,
    decaying: bool,
    gate: bool,
    filter: bool,
    cutoff: f32,
    saturator: bool,
    drive: f32,
    drive_norm: f32,
    gain: f32,
    left: f32,
    right: f32,
}

/// The filter and scratch buffers a voice entry renders through.
struct Lane {
    bank: FilterBank,
    /// The voice's audio for a stretch, a frame of left then right at a
    /// time.
    audio: Vec<f32>,
    cutoff: Vec<f32>,
    filter_type: usize,
}

/// The drum sampler's state between blocks.
pub struct Kitbag {
    data: [Option<NodeData>; SLOTS],
    voices: [Voice; VOICES],
    lanes: Vec<Lane>,
    counter: u64,
    rng: u32,
    sample_rate: f64,
}

impl Kitbag {
    /// Makes the instrument for `config`: every buffer and filter is made
    /// here, off the audio thread.
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let frames = config.max_frames.max(1);
        let rate = config.sample_rate.hz() as f32;
        let seed = (device.id.0.get() as u32).wrapping_mul(0x9E37_79B9) ^ SEED;
        Self {
            data: Default::default(),
            voices: [Voice::default(); VOICES],
            lanes: (0..VOICES)
                .map(|_| Lane {
                    bank: FilterBank::scalar(rate, 1, 2),
                    audio: vec![0.0; frames * 2],
                    cutoff: vec![0.0; frames],
                    filter_type: usize::MAX,
                })
                .collect(),
            counter: 0,
            rng: seed | 1,
            sample_rate: config.sample_rate.as_f64(),
        }
    }

    /// The decoded sample in `slot`, if the media library gave one.
    fn sample(&self, slot: usize) -> Option<&SampleData> {
        self.data[slot]
            .as_ref()
            .and_then(|data| data.downcast_ref::<SlotSample>())
            .map(|slot| slot.sample.as_ref())
    }

    /// The next random number, from -1 up to 1.
    fn random(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        f64::from(self.rng) / f64::from(u32::MAX) * 2.0 - 1.0
    }

    /// Slot `slot`'s parameters at `frame` of the stretch.
    fn slot(params: &ParamBank, slot: usize, frame: usize) -> Slot {
        use slot::*;
        let at = |local: u32| f64::from(params.ramp(bank_index(slot, local))[frame]);
        let share = |local: u32| at(local) / 100.0;
        Slot {
            start: at(START),
            transpose: at(TRANSPOSE),
            transpose_velocity: share(TRANSPOSE_VELOCITY),
            transpose_random: share(TRANSPOSE_RANDOM),
            stretch: at(STRETCH),
            stretch_b: at(STRETCH_MODE) >= 0.5,
            stretch_velocity: share(STRETCH_VELOCITY),
            filter: at(FILTER) >= 0.5,
            filter_type: (at(FILTER_TYPE) as usize).min(FILTER_TYPES.len() - 1),
            filter_freq: at(FILTER_FREQ),
            filter_res: share(FILTER_RES),
            filter_velocity: share(FILTER_VELOCITY),
            filter_random: share(FILTER_RANDOM),
            saturator: at(SATURATOR) >= 0.5,
            drive: share(DRIVE),
            decay_ms: at(DECAY),
            gate: at(DECAY_MODE) >= 0.5,
            pan: at(PAN),
            pan_velocity: share(PAN_VELOCITY),
            pan_random: share(PAN_RANDOM),
            volume: at(VOLUME),
            volume_velocity: share(VOLUME_VELOCITY),
        }
    }

    /// A global parameter at `frame` of the stretch.
    fn global(params: &ParamBank, id: ParamId, frame: usize) -> f64 {
        f64::from(params.ramp(global_index(id))[frame])
    }

    /// Takes a note: starts, releases or cuts voices.
    fn play(&mut self, note: NoteEvent, params: &ParamBank, rate: SampleRate) {
        let Some(slot) = KEYS.iter().position(|&key| key == note.key) else {
            return;
        };
        let frame = note.offset as usize;
        match note.kind {
            NoteKind::On => self.note_on(slot, note, params, frame, rate),
            NoteKind::Off => {
                for voice in &mut self.voices {
                    if voice.active && voice.slot == slot && voice.gate && !voice.cutting {
                        voice.decaying = true;
                    }
                }
            }
            NoteKind::Choke => self.cut(slot),
            NoteKind::Expression { .. } => {}
        }
    }

    /// Fades out every voice of `slot`.
    fn cut(&mut self, slot: usize) {
        let frames = ((CUT_FADE_SECONDS * self.sample_rate) as u32).max(1);
        for voice in &mut self.voices {
            if voice.active && voice.slot == slot && !voice.cutting {
                voice.cutting = true;
                voice.cut_total = frames;
                voice.cut_left = frames;
            }
        }
    }

    fn note_on(
        &mut self,
        slot: usize,
        note: NoteEvent,
        params: &ParamBank,
        frame: usize,
        rate: SampleRate,
    ) {
        // Slots without a sample are silent, and a muted slot, or any slot
        // but the soloed ones, plays nothing.
        let Some(frames) = self.sample(slot).map(SampleData::frames) else {
            return;
        };
        let on = |s: usize, local: u32| params.ramp(bank_index(s, local))[frame] >= 0.5;
        let soloed = (0..SLOTS).any(|s| on(s, slot::SOLO));
        if frames == 0 || on(slot, slot::MUTE) || (soloed && !on(slot, slot::SOLO)) {
            return;
        }
        // The link: slot 7 and slot 8 stop each other's playback.
        if Self::global(params, LINK, frame) >= 0.5 && slot >= SLOTS - 2 {
            self.cut(2 * SLOTS - 3 - slot);
        }
        let s = Self::slot(params, slot, frame);
        let transpose_global = Self::global(params, TRANSPOSE, frame);
        let time = Self::global(params, TIME, frame) / 100.0;
        // The same draws for every note, whatever the settings, so a
        // render repeats.
        let (random_pitch, random_filter, random_pan) =
            (self.random(), self.random(), self.random());
        let soft = f64::from(note.velocity.clamp(0.0, 1.0)) - 1.0;

        let semitones = s.transpose
            + transpose_global
            + s.transpose_velocity * TRANSPOSE_SPAN * soft
            + s.transpose_random * TRANSPOSE_SPAN * random_pitch;
        let ratio = 2f64.powf(semitones / 12.0);
        let stretch = s.stretch + s.stretch_velocity * STRETCH_SPAN * soft;
        let factor = (2f64.powf(stretch / 100.0 * STRETCH_OCTAVES) * time)
            .clamp(1.0 / STRETCH_LIMIT, STRETCH_LIMIT);
        let granular = (factor - 1.0).abs() > 1e-3;
        let rate_hz = rate.as_f64();
        let grain_length = (if s.stretch_b {
            GRAIN_B_SECONDS
        } else {
            GRAIN_A_SECONDS
        } * rate_hz) as u32
            & !1;
        let cutoff = f64::from(pitch_of(s.filter_freq as f32, REFERENCE_HZ))
            + 12.0
                * FILTER_SPAN_OCTAVES
                * (s.filter_velocity * soft + s.filter_random * random_filter);
        let pan = (s.pan + s.pan_velocity * PAN_SPAN * soft + s.pan_random * PAN_SPAN * random_pan)
            .clamp(-PAN_SPAN, PAN_SPAN);
        let balance = (pan / PAN_SPAN) as f32;
        let drive = 10f64.powf(s.drive * DRIVE_DB / 20.0) as f32;
        let start = (s.start / 1000.0 * rate_hz).min((frames - 1) as f64);
        let decay_frames = (s.decay_ms * time / 1000.0 * rate_hz).max(1.0);

        // A retriggered slot fades its sounding voice out, and the new one
        // takes the free entry, or the older of the two.
        self.cut(slot);
        let entries = [slot, slot + SLOTS];
        let entry = *entries
            .iter()
            .find(|&&e| !self.voices[e].active)
            .unwrap_or_else(|| {
                if self.voices[entries[0]].serial < self.voices[entries[1]].serial {
                    &entries[0]
                } else {
                    &entries[1]
                }
            });
        self.counter += 1;
        let lane = &mut self.lanes[entry];
        let kind = filter_type_of(s.filter_type);
        if lane.filter_type != s.filter_type {
            lane.filter_type = s.filter_type;
            lane.bank.set_type(kind);
        }
        lane.bank.reset_voice(
            0,
            &VoiceParams {
                resonance: s.filter_res.clamp(0.0, 1.0) as f32,
                drive: 0.0,
                saturation: 0.0,
                character: 0.0,
                stereo_offset: 0.0,
                mix: 1.0,
                level: 1.0,
                morph_x: 0.0,
                morph_y: 0.0,
            },
        );
        let mut voice = Voice {
            active: true,
            slot,
            serial: self.counter,
            end: frames as f64,
            ratio,
            position: start,
            granular,
            anchor: start,
            advance: ratio / factor,
            grain_length: grain_length.max(2),
            level: 1.0,
            step: (1.0 / decay_frames) as f32,
            decaying: !s.gate,
            gate: s.gate,
            filter: s.filter,
            cutoff: cutoff as f32,
            saturator: s.saturator,
            drive,
            drive_norm: drive.tanh().max(f32::EPSILON),
            gain: (s.volume * (1.0 + s.volume_velocity * soft)).max(0.0) as f32,
            left: 1.0 - balance.max(0.0),
            right: 1.0 + balance.min(0.0),
            ..Voice::default()
        };
        if granular {
            // A grain at the top of its window as the sound starts, and one
            // from the same place rising as it falls, so the attack of a
            // hit isn't faded in.
            voice.grains = [
                Grain {
                    active: true,
                    position: start,
                    age: voice.grain_length / 2,
                },
                Grain {
                    active: true,
                    position: start,
                    age: 0,
                },
            ];
            voice.until_grain = voice.grain_length / 2;
        }
        self.voices[entry] = voice;
    }

    /// Renders every voice over `frames` frames from `from` into `left`
    /// and `right`.
    fn render(&mut self, from: usize, frames: usize, left: &mut [f32], right: Option<&mut [f32]>) {
        let mut right = right;
        for entry in 0..VOICES {
            if !self.voices[entry].active {
                continue;
            }
            let Self {
                voices,
                lanes,
                data,
                ..
            } = self;
            let voice = &mut voices[entry];
            let lane = &mut lanes[entry];
            let Some(sample) = data[voice.slot]
                .as_ref()
                .and_then(|data| data.downcast_ref::<SlotSample>())
                .map(|slot| slot.sample.as_ref())
            else {
                voice.active = false;
                continue;
            };
            let produced = voice.raw(sample, &mut lane.audio[..frames * 2]);
            lane.audio[produced * 2..frames * 2].fill(0.0);
            // The filter runs only while the voice does, so what it would
            // ring on after the voice ends doesn't depend on where a block
            // ends.
            if voice.filter && produced > 0 {
                lane.cutoff[..produced].fill(voice.cutoff);
                lane.bank
                    .process(&lane.cutoff[..produced], &mut lane.audio[..produced * 2]);
            }
            for i in 0..frames {
                let (mut l, mut r) = (lane.audio[2 * i], lane.audio[2 * i + 1]);
                if voice.saturator {
                    l = (voice.drive * l).tanh() / voice.drive_norm;
                    r = (voice.drive * r).tanh() / voice.drive_norm;
                }
                left[from + i] += l * voice.gain * voice.left;
                if let Some(right) = &mut right {
                    right[from + i] += r * voice.gain * voice.right;
                }
            }
        }
    }
}

impl Voice {
    /// Writes up to `out.len() / 2` frames of the voice's sample, shaped by
    /// its decay and fades, and returns how many it wrote before it ended.
    fn raw(&mut self, sample: &SampleData, out: &mut [f32]) -> usize {
        let mut written = 0;
        for frame in out.as_chunks_mut::<2>().0 {
            let (l, r) = if self.granular {
                self.granular_frame(sample)
            } else {
                let value = read(sample, self.position);
                self.position += self.ratio;
                if self.position >= self.end {
                    self.active = false;
                }
                value
            };
            let mut level = self.level;
            if self.cutting {
                level *= self.cut_left as f32 / self.cut_total as f32;
                self.cut_left = self.cut_left.saturating_sub(1);
                if self.cut_left == 0 {
                    self.active = false;
                }
            }
            if self.decaying {
                self.level -= self.step;
                if self.level <= 0.0 {
                    self.level = 0.0;
                    self.active = false;
                }
            }
            frame[0] = l * level;
            frame[1] = r * level;
            written += 1;
            if !self.active {
                break;
            }
        }
        written
    }

    /// One frame of the grains, and the next grain when it's due.
    fn granular_frame(&mut self, sample: &SampleData) -> (f32, f32) {
        if self.until_grain == 0 {
            self.until_grain = self.grain_length / 2;
            if self.anchor < self.end {
                // The grain that is falling now is the one the new grain
                // fades in against; it starts where its waveform matches.
                let falling = self
                    .grains
                    .iter()
                    .filter(|g| g.active)
                    .max_by_key(|g| g.age)
                    .map(|g| g.position);
                let position =
                    falling.map_or(self.anchor, |reference| self.best_match(sample, reference));
                let slot = if self.grains[0].active { 1 } else { 0 };
                self.grains[slot] = Grain {
                    active: true,
                    position,
                    age: 0,
                };
            }
        }
        self.until_grain -= 1;
        let length = f64::from(self.grain_length);
        let (mut l, mut r) = (0.0f32, 0.0f32);
        for grain in &mut self.grains {
            if !grain.active {
                continue;
            }
            // A Hann window: two grains half a length apart add up to one.
            let window = (PI * f64::from(grain.age) / length).sin().powi(2) as f32;
            let (gl, gr) = read(sample, grain.position);
            l += gl * window;
            r += gr * window;
            grain.position += self.ratio;
            grain.age += 1;
            if grain.age >= self.grain_length || grain.position >= self.end {
                grain.active = false;
            }
        }
        self.anchor += self.advance;
        if self.anchor >= self.end && !self.grains.iter().any(|g| g.active) {
            self.active = false;
        }
        (l, r)
    }

    /// Where the next grain should start so its waveform carries on from
    /// the grain it fades in against, which is about to read from
    /// `reference`: the place near the stretch's anchor whose next few
    /// hundred frames match what `reference` reads, best (a waveform
    /// similarity search, so a tone keeps its pitch through the overlaps).
    // shortcut: the search looks at channel 0 only, steps over every second
    // candidate and stays within a quarter of a grain, about 30,000 products
    // for a 40 ms grain at 48 kHz once per half grain; a longer search or a
    // two-channel match is the upgrade if stretched stereo loops smear.
    fn best_match(&self, sample: &SampleData, reference: f64) -> f64 {
        let Some(samples) = sample.channel(0) else {
            return self.anchor;
        };
        let reach = (f64::from(self.grain_length) * SEARCH_SHARE) as i64;
        let at = |position: f64, i: usize| {
            let index = position + (i * MATCH_STEP) as f64 * self.ratio;
            samples.get(index as usize).copied().unwrap_or(0.0)
        };
        let mut wanted = [0.0f32; MATCH_FRAMES / MATCH_STEP];
        for (i, value) in wanted.iter_mut().enumerate() {
            *value = at(reference, i);
        }
        let score = |start: i64| {
            if start < 0 {
                return f32::MIN;
            }
            let (mut dot, mut energy) = (0.0f32, 1e-6f32);
            for (i, wanted) in wanted.iter().enumerate() {
                let value = at(start as f64, i);
                dot += wanted * value;
                energy += value * value;
            }
            dot / energy.sqrt()
        };
        let centre = self.anchor as i64;
        let mut best = (f32::MIN, centre);
        let mut start = centre - reach;
        while start <= centre + reach {
            let s = score(start);
            if s > best.0 {
                best = (s, start);
            }
            start += SEARCH_STEP as i64;
        }
        // Refine between the coarse candidates.
        for start in best.1 - 1..=best.1 + 1 {
            let s = score(start);
            if s > best.0 {
                best = (s, start);
            }
        }
        best.1.max(0) as f64
    }
}

impl NativeDevice for Kitbag {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if frames == 0 {
            return;
        }
        let rate = audio.sample_rate();
        let volume = params.ramp(global_index(VOLUME));
        let mut notes = audio.notes().peekable();
        let mut channels = audio.channels_mut();
        let Some(left) = channels.next() else {
            return;
        };
        let mut right = channels.next();
        left.fill(0.0);
        if let Some(right) = &mut right {
            right.fill(0.0);
        }
        // The notes cut the stretch into pieces in which the voices stay as
        // they are.
        let mut from = 0;
        while from < frames {
            while notes
                .peek()
                .is_some_and(|note| note.offset as usize <= from)
            {
                let note = notes.next().expect("peeked");
                self.play(note, params, rate);
            }
            let to = notes.peek().map_or(frames, |note| {
                (note.offset as usize).clamp(from + 1, frames)
            });
            self.render(from, to - from, left, right.as_deref_mut());
            from = to;
        }
        for (frame, sample) in left.iter_mut().enumerate() {
            *sample *= volume[frame];
        }
        if let Some(right) = &mut right {
            for (frame, sample) in right.iter_mut().enumerate() {
                *sample *= volume[frame];
            }
        }
    }

    fn receive_data(&mut self, data: NodeData) {
        if let Some(slot) = data.downcast_ref::<SlotSample>().map(|s| s.slot)
            && slot < SLOTS
        {
            self.data[slot] = Some(data);
        }
    }
}

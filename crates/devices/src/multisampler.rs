//! Sampler, the native multisampler (WP-5.13 and WP-5.14; the multisampler
//! spec, R4 to R9 and R15 to R46): a list of sample layers, each with a key
//! zone, a velocity zone and a sample select zone, played by notes through
//! zone fades, Round Robin, sustain and release loops, a volume envelope with
//! loop modes, and a voice limit; and per note a pitch envelope, a
//! modulation oscillator, a filter and shaper, an auxiliary envelope, three
//! LFOs and the MIDI tab's sources.
//!
//! The layers reach this node as node data (R31): the compiler decodes each
//! layer's sample off the audio thread and sends the whole list, so a zone
//! or layer edit changes data and never the program. Every voice and buffer
//! is made with the node, and the audio thread only reads the data it was
//! sent.

use std::sync::atomic::Ordering::Relaxed;

use bl_core::time::{Frames, SampleRate, TempoMap, Ticks};
use bl_engine::{
    Audition, DeviceLayers, DeviceType, KeyPitches, LayerSample, NodeConfig, NodeData, NoteEvent,
    NoteExpression, NoteKind, Registry,
};
use bl_model::device_layout::{
    BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Page, PageStyle, PagesNode, Widget,
};
use bl_model::{
    DeviceDescription, DeviceLayout, DeviceRole, FadeMode, ParamDescriptor, ParamId, ParamUnit,
    ReleaseMode, SamplerTab, SustainMode,
};
use lspk_dsp::filter::pitch_of;

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::instrument_filter::{InstrumentFilter, Per, PerFrame, Setup};
use crate::sampler::{lfo_shape, random};

pub use crate::multisampler_tabs::*;

/// The multisampler device type's id.
pub const MULTISAMPLER: &str = "beatlayer.mosaic";

/// The Sample Selector: which layers a new note can play (R6).
pub const SELECTOR: ParamId = ParamId(0);
/// Fade Mode: whether every zone's fades are linear or constant-power (R5).
pub const FADE_MODE: ParamId = ParamId(1);
/// Round Robin on or off (R8).
pub const ROUND_ROBIN: ParamId = ParamId(2);
/// Round Robin Mode: Forward, Backward, Other or Random (R8).
pub const RR_MODE: ParamId = ParamId(3);
/// Reset Interval: when Round Robin starts again (R9).
pub const RESET_INTERVAL: ParamId = ParamId(4);
/// Reverse: plays every layer backwards (R21).
pub const REVERSE: ParamId = ParamId(5);
/// Interpolation: how a voice reads between a sample's frames (R22).
pub const INTERPOLATION: ParamId = ParamId(6);
/// The device's output level (R28).
pub const VOLUME: ParamId = ParamId(7);
/// How much velocity scales the level (R28).
pub const VEL_VOL: ParamId = ParamId(8);
/// Where every layer sits in the stereo field (R28).
pub const PAN: ParamId = ParamId(9);
/// How far each note's pan moves at random (R28).
pub const PAN_RND: ParamId = ParamId(10);
/// How many notes sound at once (R27).
pub const VOICES: ParamId = ParamId(11);
/// Whether a note on a sounding key restarts it (R27).
pub const RETRIGGER: ParamId = ParamId(12);
/// Scales every envelope's times (R26).
pub const TIME: ParamId = ParamId(13);
/// Scales every envelope's times by the note's key (R26).
pub const TIME_KEY: ParamId = ParamId(14);
/// The volume envelope's Initial level (R24).
pub const ENV_INITIAL: ParamId = ParamId(15);
/// The volume envelope's Peak level.
pub const ENV_PEAK: ParamId = ParamId(16);
/// The volume envelope's Sustain level.
pub const ENV_SUSTAIN: ParamId = ParamId(17);
/// The volume envelope's Attack time.
pub const ENV_ATTACK: ParamId = ParamId(18);
/// The volume envelope's Decay time.
pub const ENV_DECAY: ParamId = ParamId(19);
/// The volume envelope's Release time.
pub const ENV_RELEASE: ParamId = ParamId(20);
/// The volume envelope's Attack slope.
pub const ENV_ATTACK_SLOPE: ParamId = ParamId(21);
/// The volume envelope's Decay slope.
pub const ENV_DECAY_SLOPE: ParamId = ParamId(22);
/// The volume envelope's Release slope.
pub const ENV_RELEASE_SLOPE: ParamId = ParamId(23);
/// The volume envelope's Loop mode (R25).
pub const ENV_LOOP: ParamId = ParamId(24);
/// The time a Loop or Trigger envelope takes to return to Initial.
pub const ENV_LOOP_TIME: ParamId = ParamId(25);
/// How often a Beat or Sync envelope starts again.
pub const ENV_REPEAT: ParamId = ParamId(26);
/// How much velocity shortens the volume envelope's segments (R26).
pub const ENV_TIME_VEL: ParamId = ParamId(27);
// 28 to 99 are left for this package; WP-5.14's tabs (R32 to R46) start at
// 100, so their parameters never move these.

/// Fade Mode's choices.
pub const FADE_MODES: [&str; 2] = ["Lin", "Pow"];
/// Round Robin Mode's choices.
pub const RR_MODES: [&str; 4] = ["Forward", "Backward", "Other", "Random"];
/// Reset Interval's choices.
pub const RESET_INTERVALS: [&str; 9] = [
    "None", "1/16", "1/8", "1/4", "1/2", "1 Bar", "2 Bars", "4 Bars", "8 Bars",
];
/// Reset Interval's lengths in whole notes (a bar is a whole note: the
/// engine counts 4/4).
const RESET_WHOLES: [f64; 9] = [0.0, 0.0625, 0.125, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0];
/// Interpolation's choices.
pub const INTERPOLATIONS: [&str; 3] = ["Normal", "Good", "Best"];
/// The envelope's Loop choices.
pub const ENV_LOOPS: [&str; 5] = ["None", "Loop", "Beat", "Sync", "Trigger"];
/// The envelope's Repeat choices.
pub const REPEATS: [&str; 10] = [
    "1/64", "1/32", "1/16", "1/8", "1/4", "1/2", "1 Bar", "2 Bars", "4 Bars", "8 Bars",
];
/// The Repeat choices' lengths in whole notes.
const REPEAT_WHOLES: [f64; 10] = [
    0.015_625, 0.031_25, 0.0625, 0.125, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0,
];

/// The most notes sounding at once (the Voices parameter's top).
pub const MAX_VOICES: usize = 32;

/// How many layers one note can play at once; a note matching more plays
/// the first of them.
const NOTE_LAYERS: usize = 1024;

/// The voices the node holds, one per layer sounding: room for a few notes
/// that each play many layers, and for the ones being stolen while they fade
/// out. A note that finds none free takes the oldest fading one, and plays
/// without the layers it finds no room for.
const SLOTS: usize = 2048;

/// How long a stolen note's voices fade out, in seconds (the sampler's
/// 5 ms steal fade, sampler spec R8).
const STEAL_FADE_SECONDS: f64 = 0.005;

/// The longest envelope time, in seconds.
const ENVELOPE_MAX_SECONDS: f64 = 20.0;

/// How far Spread detunes each of a layer's two voices at 100 %, in cents
/// (R32).
const SPREAD_CENTS: f64 = 25.0;

/// The kit's filters measure pitch with A4 at this frequency.
const REFERENCE_HZ: f32 = 440.0;

/// Taps in the windowed-sinc kernel Best reads with, and how finely the
/// kernel's table divides the gap between two frames.
const SINC_TAPS: usize = 32;
const SINC_PHASES: usize = 2048;

/// The Sampler device type.
#[must_use]
pub fn multisampler_type() -> DeviceType {
    let time = |id, name: &str, default: f64| {
        ParamDescriptor::continuous(
            id,
            name,
            (0.0, ENVELOPE_MAX_SECONDS * 1000.0),
            default,
            ParamUnit::Milliseconds,
        )
    };
    let slope = |id, name: &str| {
        ParamDescriptor::continuous(id, name, (-100.0, 100.0), 0.0, ParamUnit::Percent)
    };
    let level = |id, name: &str, default: f64| {
        ParamDescriptor::continuous(id, name, (0.0, 100.0), default, ParamUnit::Percent)
    };
    let scale = |id, name: &str| {
        ParamDescriptor::continuous(id, name, (-100.0, 100.0), 0.0, ParamUnit::Percent)
    };
    let mut params =
        vec![
        ParamDescriptor::integer(SELECTOR, "Selector", (0, 127), 0, ParamUnit::None).describe(
            "The Sample Selector: a new note plays the layers whose sample select zone holds \
             this value. Moving it changes which layers a new note can play, never a layer \
             already sounding.",
        ),
        ParamDescriptor::choice(FADE_MODE, "Fade Mode", &FADE_MODES, 1).describe(
            "Whether every zone's fades are linear (Lin) or constant-power (Pow), so \
             neighbouring layers crossfade at a steady level.",
        ),
        ParamDescriptor::toggle(ROUND_ROBIN, "Round Robin", false).describe(
            "Turns layers that share a key zone into a rotation: each note plays one of them \
             instead of all.",
        ),
        ParamDescriptor::choice(RR_MODE, "Round Robin Mode", &RR_MODES, 0).describe(
            "How Round Robin chooses: Forward and Backward go down or up the layer list, Other \
             picks at random but never the same layer twice running, and Random picks at \
             random.",
        ),
        ParamDescriptor::choice(RESET_INTERVAL, "Reset Interval", &RESET_INTERVALS, 0).describe(
            "Starts every Round Robin rotation again at each multiple of this length in song \
             time, counted from 1.1.1 while the transport plays.",
        ),
        ParamDescriptor::toggle(REVERSE, "Reverse", false).describe(
            "Plays every layer backwards, from Sample End back through the sustain loop to \
             Sample Start, without making a file.",
        ),
        ParamDescriptor::choice(INTERPOLATION, "Interpolation", &INTERPOLATIONS, 0).describe(
            "How a voice reads between a sample's frames: Normal in a straight line, Good with \
             a four-point cubic, Best with a windowed-sinc kernel that keeps aliases far \
             down. At the root key every mode plays the frames exactly.",
        ),
        ParamDescriptor::gain(VOLUME, "Volume", 24.0, 0.0)
            .describe("The instrument's output level; reaches silence at its bottom."),
        level(VEL_VOL, "Vel > Vol", 0.0).describe(
            "How much a note's velocity scales its level: at 0 it doesn't, at 100 % the level \
             follows velocity in a straight line.",
        ),
        ParamDescriptor::continuous(PAN, "Pan", (-50.0, 50.0), 0.0, ParamUnit::None)
            .decimals(0)
            .describe("Where every layer sits in the stereo field, added to the layer's own pan."),
        level(PAN_RND, "Pan < Rnd", 0.0).describe(
            "Moves each note's pan by a random share up to this amount of the full width.",
        ),
        ParamDescriptor::integer(VOICES, "Voices", (1, MAX_VOICES as i64), 32, ParamUnit::None)
            .describe(
                "How many notes sound at once, a note's layers counting together; past it the \
                 oldest note fades out in favour of the new one.",
            ),
        ParamDescriptor::toggle(RETRIGGER, "Retrigger", false).describe(
            "A note on a key that is already sounding restarts it instead of taking another \
             voice.",
        ),
        scale(TIME, "Time").describe(
            "Scales every envelope's times: longer above 0, shorter below, by up to a factor \
             of ten.",
        ),
        scale(TIME_KEY, "Time < Key").describe(
            "Shortens every envelope's times for high notes and lengthens them for low ones, \
             halving them an octave above C3 at 100 %.",
        ),
        level(ENV_INITIAL, "Env Initial", 0.0)
            .describe("The volume envelope's level when a note starts."),
        level(ENV_PEAK, "Env Peak", 100.0)
            .describe("The level the volume envelope's attack reaches."),
        level(ENV_SUSTAIN, "Env Sustain", 100.0)
            .describe("The level the volume envelope holds while the note sounds."),
        time(ENV_ATTACK, "Env Attack", 0.0)
            .describe("The time the volume envelope takes from Initial to Peak."),
        time(ENV_DECAY, "Env Decay", 0.0)
            .describe("The time the volume envelope takes from Peak to Sustain."),
        time(ENV_RELEASE, "Env Release", 50.0)
            .describe("The time the volume envelope takes to fall to silence after the note ends."),
        slope(ENV_ATTACK_SLOPE, "Attack Slope").describe(
            "The attack's curve: positive moves fast then slowly, negative slowly then fast, \
             zero in a straight line.",
        ),
        slope(ENV_DECAY_SLOPE, "Decay Slope").describe("The decay's curve, as the attack's."),
        slope(ENV_RELEASE_SLOPE, "Release Slope")
            .describe("The release's curve, as the attack's."),
        ParamDescriptor::choice(ENV_LOOP, "Env Loop", &ENV_LOOPS, 0).describe(
            "What the volume envelope does while a note is held: Loop starts it again after the \
             decay, Beat and Sync start it again every Repeat (Sync on the song's grid), and \
             Trigger loops but lets the attack and decay finish before the note's end.",
        ),
        time(ENV_LOOP_TIME, "Env Loop Time", 0.0).describe(
            "How long a looping envelope takes to go back from Sustain to Initial before it \
             starts again.",
        ),
        ParamDescriptor::choice(ENV_REPEAT, "Env Repeat", &REPEATS, 4).describe(
            "How often a Beat or Sync envelope starts again, as a note value from 1/64 to 8 Bars.",
        ),
        scale(ENV_TIME_VEL, "Time < Vel").describe(
            "Shortens the envelope's segments for hard notes: at 100 % a full-velocity note's \
             segments take no time.",
        ),
    ];
    params.extend(crate::multisampler_tabs::params());
    DeviceType::native(MULTISAMPLER, "Sampler", DeviceRole::Instrument)
        .with_description(DeviceDescription::new(params).with_layout(layout()))
}

/// The device's layout (R1, R75): a pages node in the title-bar style named
/// `Tabs`, one page per tab after Zone, in the tabs' order, because the tabs
/// show one page at a time and the device is as wide as the widest of them.
/// The Sample page holds the zone and
/// voice controls and the sample's display; Filter/Global's the filter, its
/// envelope, the shaper, the volume envelope and the output controls. The
/// zone editor and the Sample tab's waveform are the app's own views.
fn layout() -> DeviceLayout {
    let zones = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (SELECTOR, Widget::Number),
            (FADE_MODE, Widget::Chooser),
            (ROUND_ROBIN, Widget::Toggle),
            (RR_MODE, Widget::Chooser),
            (RESET_INTERVAL, Widget::Chooser),
            (REVERSE, Widget::Toggle),
            (INTERPOLATION, Widget::Chooser),
        ],
    );
    let envelope = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (ENV_INITIAL, Widget::Knob),
            (ENV_PEAK, Widget::Knob),
            (ENV_SUSTAIN, Widget::Knob),
            (ENV_ATTACK, Widget::Knob),
            (ENV_DECAY, Widget::Knob),
            (ENV_RELEASE, Widget::Knob),
            (ENV_ATTACK_SLOPE, Widget::Knob),
            (ENV_DECAY_SLOPE, Widget::Knob),
            (ENV_RELEASE_SLOPE, Widget::Knob),
            (ENV_LOOP, Widget::Chooser),
            (ENV_LOOP_TIME, Widget::Knob),
            (ENV_REPEAT, Widget::Chooser),
            (ENV_TIME_VEL, Widget::Knob),
        ],
    );
    let output = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (VOLUME, Widget::Knob),
            (VEL_VOL, Widget::Knob),
            (PAN, Widget::Knob),
            (PAN_RND, Widget::Knob),
            (VOICES, Widget::Number),
            (RETRIGGER, Widget::Toggle),
            (TIME, Widget::Knob),
            (TIME_KEY, Widget::Knob),
        ],
    );
    let display = DisplayCell::new("Sample", 220.0, BODY_HEIGHT, 1.0);
    let mut filter = filter_bands();
    filter.push(LayoutNode::Band(envelope));
    filter.push(LayoutNode::Band(output));
    let nodes = [
        LayoutNode::Row(vec![LayoutNode::Band(zones), LayoutNode::Cell(display)]),
        LayoutNode::Row(pitch_osc_bands()),
        LayoutNode::Row(filter),
        LayoutNode::Row(modulation_bands()),
        LayoutNode::Row(midi_bands()),
    ];
    DeviceLayout::new(LayoutNode::Pages(PagesNode::new(
        SamplerTab::PAGES,
        PageStyle::TitleBar,
        SamplerTab::ALL
            .into_iter()
            .zip(nodes)
            .map(|(tab, node)| Page::new(tab.page_name(), node))
            .collect(),
    )))
}

/// Adds the multisampler to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, multisampler_type(), |device, config| {
        Multisampler::new(config, device.id.0.get())
    });
}

/// How a voice reads between frames (R22).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Interp {
    Normal,
    Good,
    Best,
}

/// The volume envelope's loop mode (R25).
#[derive(Clone, Copy, PartialEq, Eq)]
enum EnvLoop {
    None,
    Loop,
    Beat,
    Sync,
    Trigger,
}

/// What an envelope runs by for a stretch (R24 to R26).
#[derive(Clone, Copy)]
struct EnvSettings {
    initial: f32,
    peak: f32,
    sustain: f32,
    /// Where the release goes: silence for the volume envelope.
    end: f32,
    /// Whether this is the volume envelope, which ends its voice when the
    /// release is over; the others hold their End level.
    volume: bool,
    attack: f64,
    decay: f64,
    release: f64,
    loop_time: f64,
    attack_exp: f32,
    decay_exp: f32,
    release_exp: f32,
    mode: EnvLoop,
    /// Frames between Beat restarts (or Sync's when the transport is
    /// stopped).
    repeat: f64,
}

/// How many notes the device tracks at once: the notes sounding and the ones
/// fading out. A note's place is also its lane in the filter's buffers.
const NOTE_SLOTS: usize = 96;

/// The most modulation routings at once: the auxiliary envelope's two, each
/// of LFOs 2 and 3's two, and the MIDI tab's fourteen.
const MAX_ROUTES: usize = 20;

/// The destinations of R46, as the length of a note's modulation sums.
const DESTS: usize = DESTINATIONS.len();

/// The range an LFO's rate moves over when a routing sends to it, in hertz
/// (R45: a destination's whole range).
const LFO_MIN_HZ: f64 = 0.01;
const LFO_MAX_HZ: f64 = 30.0;

/// Points in the tables the oscillator's limited saws and squares read.
const WAVE_TABLE: usize = 2048;

/// What a modulation routing reads (R40, R42, R43).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Aux,
    Lfo2,
    Lfo3,
    Key,
    Velocity,
    ReleaseVelocity,
    Aftertouch,
    Wheel,
    Foot,
    Bend,
}

/// One source sent to one destination with an amount.
#[derive(Clone, Copy)]
struct Route {
    source: Source,
    dest: usize,
    amount: f32,
}

/// The modulation oscillator's settings (R36).
#[derive(Clone, Copy)]
struct OscSettings {
    on: bool,
    am: bool,
    wave: usize,
    volume: f32,
    vol_vel: f32,
    fixed: bool,
    /// Fixed's frequency times its multiplier, in hertz.
    hz: f64,
    /// Coarse plus Fine/1000.
    ratio: f64,
}

/// The filter's settings (R37, R38).
#[derive(Clone, Copy)]
struct FilterSettings {
    on: bool,
    /// Low-pass, high-pass, band-pass, notch or Morph, as the chooser.
    kind: usize,
    slope24: bool,
    /// Clean, OSR, MS2, SMP or PRD, as the chooser.
    circuit: usize,
    /// The cutoff in the kit filter's semitones.
    pitch: f32,
    res: f32,
    /// Morph, 0 to 1.
    morph: f32,
    /// Drive, 0 to 1 of its 24 dB.
    drive: f32,
    fenv_on: bool,
    fenv_amount: f32,
}

/// The shaper's settings (R39).
#[derive(Clone, Copy)]
struct ShaperSettings {
    on: bool,
    kind: usize,
    amount: f32,
    post: bool,
}

/// One LFO's settings (R41, R42).
#[derive(Clone, Copy)]
struct LfoSettings {
    on: bool,
    wave: usize,
    /// The rate at the stretch's tempo, in hertz.
    hz: f64,
    /// Attack in frames.
    attack: f64,
    retrig: bool,
    /// Where a retriggered LFO starts, 0 to 1 of a cycle.
    offset: f64,
    key: f64,
    /// LFOs 2 and 3: how far ahead the right channel runs, 0 to 1 of a
    /// cycle, and how much faster, 0 to 0.5.
    phase: f64,
    spin: f64,
    /// LFO 1's Vol, Pan, Filter (in semitones) and Pitch.
    amounts: [f32; 4],
}

/// What one stretch plays by.
#[derive(Clone, Copy)]
struct Settings {
    rate: f64,
    selector: u8,
    fade_mode: FadeMode,
    round_robin: bool,
    rr_mode: usize,
    /// Reset Interval in ticks; 0 for none.
    reset_ticks: i64,
    reverse: bool,
    interp: Interp,
    vel_vol: f32,
    pan_random: f32,
    voices: usize,
    retrigger: bool,
    time: f64,
    time_key: f64,
    /// The volume, pitch, filter, auxiliary and oscillator envelopes.
    envs: [EnvSettings; 5],
    /// Each envelope's Time < Vel.
    time_vel: [f64; 5],
    /// Whether the pitch, filter, auxiliary and oscillator envelopes move
    /// anything, so a note steps only those that do.
    env_used: [bool; 4],
    steal_frames: u32,
    power_loops: bool,
    /// The stretch's first frame in ticks, and ticks per frame.
    ticks: f64,
    ticks_per_frame: f64,
    /// Whether the transport plays, so song time moves.
    playing: bool,
    /// For each envelope's Sync: frames until the next multiple of Repeat in
    /// song time, and the frames between them.
    sync_first: [f64; 5],
    sync_period: [f64; 5],
    /// Transpose and Detune together, in semitones.
    tune: f64,
    /// Spread's detune either way, in cents.
    spread: f64,
    key_shift: i32,
    /// Glide's mode (0 off, 1 Glide, 2 Portamento) and time in frames.
    glide: u8,
    glide_frames: f64,
    /// The pitch envelope's Amount in semitones.
    pitch_amount: f64,
    bend_range: f64,
    /// The pitch of every key under the set's tuning (the tunings spec,
    /// R12; the multisampler spec, R74); filled in by the caller.
    keys: KeyPitches,
    osc: OscSettings,
    filter: FilterSettings,
    shaper: ShaperSettings,
    lfos: [LfoSettings; 3],
    routes: [Route; MAX_ROUTES],
    route_count: usize,
}

/// The slope s (−1 to 1) as a segment's exponent: a share x of the time
/// gives x^e of the way, e = 4^−s (R24).
fn exponent(slope: f64) -> f32 {
    4f64.powf(-slope / 100.0) as f32
}

/// Where the volume envelope is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    /// A looping envelope going back from Sustain to Initial.
    Return,
    Release,
    /// A modulation envelope after its release, holding its End level.
    Done,
}

/// One voice's volume envelope (R24, R25).
#[derive(Clone, Copy)]
struct Env {
    stage: Stage,
    level: f32,
    from: f32,
    to: f32,
    elapsed: u32,
    total: u32,
    exp: f32,
    /// Whether the note is still held.
    held: bool,
    /// Trigger: the note ended during the attack or decay, so the release
    /// starts when the decay ends.
    pending: bool,
    /// Frames since the note started, and the age of the next Beat restart.
    age: f64,
    next_beat: f64,
}

impl Env {
    fn idle() -> Self {
        Self {
            stage: Stage::Idle,
            level: 0.0,
            from: 0.0,
            to: 0.0,
            elapsed: 0,
            total: 1,
            exp: 1.0,
            held: false,
            pending: false,
            age: 0.0,
            next_beat: 0.0,
        }
    }

    fn enter(&mut self, stage: Stage, to: f32, frames: f64, exp: f32) {
        self.stage = stage;
        self.from = self.level;
        self.to = to;
        self.elapsed = 0;
        self.total = frames.round().max(0.0) as u32;
        self.exp = exp;
        if self.total == 0 {
            self.level = to;
        }
    }

    /// Starts a note: the level at Initial, heading for Peak.
    fn start(&mut self, set: &EnvSettings, scale: f64) {
        *self = Self::idle();
        self.level = set.initial;
        self.held = true;
        self.next_beat = set.repeat;
        self.enter(Stage::Attack, set.peak, set.attack * scale, set.attack_exp);
    }

    /// The note ended: the release from the level it's at, or, for a
    /// Trigger envelope still in its attack or decay, when they finish.
    fn release(&mut self, set: &EnvSettings, scale: f64) {
        if !self.held || matches!(self.stage, Stage::Release | Stage::Idle | Stage::Done) {
            return;
        }
        self.held = false;
        if set.mode == EnvLoop::Trigger && matches!(self.stage, Stage::Attack | Stage::Decay) {
            self.pending = true;
        } else {
            self.enter(
                Stage::Release,
                set.end,
                set.release * scale,
                set.release_exp,
            );
        }
    }

    /// Fades the voice out over `frames`, whatever it was doing.
    fn fade(&mut self, frames: u32) {
        self.held = false;
        self.pending = false;
        self.enter(Stage::Release, 0.0, f64::from(frames), 1.0);
    }

    /// Goes again from the level it's at, at the attack's rate (Beat and
    /// Sync).
    fn restart(&mut self, set: &EnvSettings, scale: f64) {
        let span = set.peak - set.initial;
        let frames = if span > 0.0 {
            set.attack * scale * f64::from(((set.peak - self.level) / span).clamp(0.0, 1.0))
        } else {
            0.0
        };
        self.enter(Stage::Attack, set.peak, frames, set.attack_exp);
    }

    /// Moves on a frame and gives the level. `sync_hit` is whether this
    /// frame lands on a multiple of Repeat in song time.
    fn step(&mut self, set: &EnvSettings, scale: f64, sync_hit: bool) -> f32 {
        if self.stage == Stage::Idle {
            return 0.0;
        }
        self.age += 1.0;
        if self.held {
            let beat = set.mode == EnvLoop::Beat && self.age >= self.next_beat;
            if beat {
                self.next_beat += set.repeat.max(1.0);
            }
            if beat || (set.mode == EnvLoop::Sync && sync_hit) {
                self.restart(set, scale);
            }
        }
        if self.elapsed < self.total {
            self.elapsed += 1;
            let x = self.elapsed as f32 / self.total as f32;
            self.level = self.from + (self.to - self.from) * x.powf(self.exp);
        }
        // A zero-length segment passes at once; four of them in a row is
        // all a loop's cycle can hold without taking any time.
        for _ in 0..4 {
            if self.elapsed < self.total {
                break;
            }
            match self.stage {
                Stage::Attack => {
                    self.enter(Stage::Decay, set.sustain, set.decay * scale, set.decay_exp);
                }
                Stage::Decay => {
                    if self.pending {
                        self.pending = false;
                        self.enter(
                            Stage::Release,
                            set.end,
                            set.release * scale,
                            set.release_exp,
                        );
                    } else if self.held && matches!(set.mode, EnvLoop::Loop | EnvLoop::Trigger) {
                        self.enter(Stage::Return, set.initial, set.loop_time * scale, 1.0);
                    } else {
                        self.stage = Stage::Sustain;
                        self.level = set.sustain;
                    }
                }
                Stage::Return => {
                    self.enter(Stage::Attack, set.peak, set.attack * scale, set.attack_exp);
                }
                Stage::Release => {
                    if set.volume {
                        self.stage = Stage::Idle;
                        self.level = 0.0;
                    } else {
                        self.stage = Stage::Done;
                        self.level = set.end;
                    }
                    break;
                }
                Stage::Sustain | Stage::Idle | Stage::Done => break,
            }
        }
        self.level
    }
}

/// One layer sounding for a note.
#[derive(Clone, Copy)]
struct Voice {
    active: bool,
    /// Fading out because it was stolen or retriggered.
    fading: bool,
    key: u8,
    /// The note it belongs to: voices are counted and stolen by note.
    note: u64,
    /// The note's place in the note table, which is also its lane in the
    /// filter's buffers.
    lane: u16,
    /// Its layer's place in the layer list.
    layer: u32,
    /// Its position in the sample, in frames at the engine's rate.
    position: f64,
    forward: bool,
    /// Frames per frame: its pitch, and inside a loop with the loop's Detune.
    ratio: f64,
    loop_ratio: f64,
    /// The playable span, and the loops, in frames at the engine's rate.
    start: f64,
    end: f64,
    loop_start: f64,
    loop_end: f64,
    release_loop: f64,
    /// The loop crossfade in frames.
    crossfade: f64,
    sustain: SustainMode,
    release: ReleaseMode,
    reverse: bool,
    /// Plays on to Sample End whatever the note does (R50's `one_shot`).
    one_shot: bool,
    released: bool,
    /// Whether a click in the Sample tab started it (R54).
    audition: bool,
    /// Whether the voice has entered its sustain loop.
    in_loop: bool,
    /// The level the note gives it: its zones' gains, the layer's volume and
    /// the velocity's share, and its pan.
    gain: f32,
    pan: f32,
    /// Its envelope's time scale (R26), fixed when the note starts.
    scale: f64,
    env: Env,
}

impl Voice {
    fn idle() -> Self {
        Self {
            active: false,
            fading: false,
            key: 0,
            note: 0,
            lane: 0,
            layer: 0,
            position: 0.0,
            forward: true,
            ratio: 1.0,
            loop_ratio: 1.0,
            start: 0.0,
            end: 0.0,
            loop_start: 0.0,
            loop_end: 0.0,
            release_loop: 0.0,
            crossfade: 0.0,
            sustain: SustainMode::Off,
            release: ReleaseMode::Off,
            reverse: false,
            one_shot: false,
            released: false,
            audition: false,
            in_loop: false,
            gain: 0.0,
            pan: 0.0,
            scale: 1.0,
            env: Env::idle(),
        }
    }

    /// Whether the sustain loop holds the voice now (R17, R18): a release
    /// mode other than Off lets it out when the note ends.
    fn looping(&self) -> bool {
        self.sustain != SustainMode::Off && (self.release == ReleaseMode::Off || !self.released)
    }

    /// Whether the release loop holds it now (R18); the release loops
    /// aren't mirrored for a reversed layer.
    fn release_looping(&self) -> bool {
        self.sustain != SustainMode::Off
            && self.released
            && !self.reverse
            && matches!(self.release, ReleaseMode::Loop | ReleaseMode::BackAndForth)
    }

    /// The note ended (R18): a release mode other than Off lets the voice
    /// out of the sustain loop and on towards the end.
    fn note_off(&mut self, set: &EnvSettings, time_mul: f64) {
        if self.one_shot {
            return;
        }
        self.released = true;
        if self.release != ReleaseMode::Off && !self.reverse {
            self.forward = true;
        }
        self.env.release(set, self.scale * time_mul);
    }

    /// Moves the position on a frame and through the loops (R16 to R19);
    /// `pitch` is the note's pitch movement as a ratio (R32 to R36).
    fn advance(&mut self, pitch: f64) {
        let ratio = pitch
            * if self.in_loop && self.sustain != SustainMode::Off {
                self.loop_ratio
            } else {
                self.ratio
            };
        if self.forward {
            self.position += ratio;
        } else {
            self.position -= ratio;
        }
        let (ls, le) = (self.loop_start, self.loop_end);
        let length = le - ls;
        if self.looping() && length > 0.0 {
            match self.sustain {
                SustainMode::Loop => {
                    if self.forward {
                        if self.position >= le {
                            // The wrap resumes past the head the crossfade
                            // already played, so the two meet.
                            self.position =
                                ls + (self.position + self.crossfade - le).rem_euclid(length);
                            self.in_loop = true;
                        } else if self.position >= ls {
                            self.in_loop = true;
                        }
                    } else if self.position <= le {
                        if self.position < ls {
                            self.position = le - (ls - self.position).rem_euclid(length);
                        }
                        self.in_loop = true;
                    }
                }
                SustainMode::BackAndForth => {
                    if self.forward {
                        if self.position >= le {
                            // Turns on the loop's last frame, as it turns on
                            // its first going back.
                            self.position = (2.0 * (le - 1.0) - self.position).max(ls);
                            self.forward = false;
                            self.in_loop = true;
                        } else if self.position >= ls {
                            self.in_loop = true;
                        }
                    } else if self.position <= le {
                        self.in_loop = true;
                        if self.position < ls {
                            self.position = ls + (ls - self.position);
                            self.forward = true;
                        }
                    }
                }
                SustainMode::Off => {}
            }
        } else if self.release_looping() && self.release_loop < self.end {
            self.in_loop = false;
            let rl = self.release_loop;
            let span = self.end - rl;
            if self.release == ReleaseMode::Loop {
                if self.forward && self.position >= self.end {
                    self.position = rl + (self.position - self.end).rem_euclid(span);
                }
            } else if self.forward && self.position >= self.end {
                // Sample End is past the last frame, which reads as silence.
                self.position = (2.0 * (self.end - 1.0) - self.position).max(rl);
                self.forward = false;
            } else if !self.forward && self.position < rl {
                self.position = rl + (rl - self.position);
                self.forward = true;
            }
        } else {
            self.in_loop = false;
        }
        if (self.forward && self.position >= self.end)
            || (!self.forward && self.position < self.start)
        {
            self.active = false;
        }
    }
}

/// The windowed-sinc kernel Best reads with: a Kaiser-windowed sinc at
/// [`SINC_PHASES`] fractional positions, each row summing to one.
struct Sinc {
    table: Vec<f32>,
}

impl Sinc {
    fn new() -> Self {
        let beta = 9.0f64;
        let norm = bessel_i0(beta);
        let half = (SINC_TAPS / 2) as f64;
        let mut table = vec![0.0f32; SINC_TAPS * (SINC_PHASES + 1)];
        for phase in 0..=SINC_PHASES {
            let frac = phase as f64 / SINC_PHASES as f64;
            let mut row = [0.0f64; SINC_TAPS];
            for (tap, value) in row.iter_mut().enumerate() {
                // The tap reads the frame (tap − half + 1) away from the
                // floor frame, so its distance from the read position is
                // that minus the fraction.
                let x = tap as f64 - half + 1.0 - frac;
                let sinc = if x.abs() < 1e-12 {
                    1.0
                } else {
                    (core::f64::consts::PI * x).sin() / (core::f64::consts::PI * x)
                };
                let t = x / half;
                let window = if t.abs() >= 1.0 {
                    0.0
                } else {
                    bessel_i0(beta * (1.0 - t * t).sqrt()) / norm
                };
                *value = sinc * window;
            }
            let sum: f64 = row.iter().sum();
            for (tap, value) in row.iter().enumerate() {
                table[phase * SINC_TAPS + tap] = (value / sum) as f32;
            }
        }
        Self { table }
    }

    /// The sample `at` gives, read at `position` through the kernel.
    fn read(&self, at: impl Fn(usize) -> f32, position: f64) -> f32 {
        let index = position.floor();
        let frac = position - index;
        let first = index as i64 - (SINC_TAPS as i64 / 2 - 1);
        let scaled = frac * SINC_PHASES as f64;
        let phase = scaled as usize;
        let blend = (scaled - phase as f64) as f32;
        let a = &self.table[phase * SINC_TAPS..(phase + 1) * SINC_TAPS];
        let b = &self.table[(phase + 1) * SINC_TAPS..(phase + 2) * SINC_TAPS];
        let mut sum = 0.0f32;
        for tap in 0..SINC_TAPS {
            let frame = first + tap as i64;
            if frame >= 0 {
                sum += at(frame as usize) * (a[tap] + (b[tap] - a[tap]) * blend);
            }
        }
        sum
    }
}

/// The modified Bessel function of the first kind, order 0, by its series.
pub(crate) fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let quarter = x * x / 4.0;
    for k in 1..60 {
        term *= quarter / (f64::from(k) * f64::from(k));
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// One channel of a sample at `position`: the frame itself at a whole
/// position in every mode (R22), otherwise as `mode` reads between frames,
/// silence outside the sample. `frame` gives the channel's frames.
fn read_channel(sinc: &Sinc, frame: impl Fn(usize) -> f32, position: f64, mode: Interp) -> f32 {
    if position < 0.0 {
        return 0.0;
    }
    let index = position as usize;
    let frac = (position - index as f64) as f32;
    let at = |i: i64| -> f32 { if i < 0 { 0.0 } else { frame(i as usize) } };
    if frac == 0.0 {
        return at(index as i64);
    }
    match mode {
        Interp::Normal => {
            let (a, b) = (at(index as i64), at(index as i64 + 1));
            a + (b - a) * frac
        }
        Interp::Good => {
            // Catmull-Rom through the four frames round the position.
            let i = index as i64;
            let (p0, p1, p2, p3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
            let t = frac;
            0.5 * ((2.0 * p1)
                + (-p0 + p2) * t
                + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t
                + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t * t * t)
        }
        Interp::Best => sinc.read(frame, position),
    }
}

/// A sample frame at `position` in each channel: a mono sample on both.
fn read(sinc: &Sinc, sample: &LayerSample, position: f64, mode: Interp) -> (f32, f32) {
    (
        read_channel(sinc, |i| sample.at(0, i), position, mode),
        read_channel(sinc, |i| sample.at(1, i), position, mode),
    )
}

/// The channel's controllers, shared by every note (R43).
#[derive(Clone, Copy, Default)]
struct Controls {
    wheel: f32,
    foot: f32,
    bend: f32,
    pressure: f32,
}

/// One LFO's state in one note: the phase of each channel's LFO, the value
/// S&H holds, and how long the note has run it.
#[derive(Clone, Copy)]
struct LfoState {
    phase: [f64; 2],
    held: [f32; 2],
    age: u32,
    rng: u32,
}

/// The values the shaper reads once a block, from the note's last frame (a
/// shortcut), and the filter's, which it reads at every frame (the
/// instrument filter spec, R11).
#[derive(Clone, Copy, Default)]
struct BlockMods {
    res: f32,
    morph: f32,
    drive: f32,
    shaper: f32,
}

/// What a note gives its voices and the stage after them, a frame at a time.
#[derive(Clone, Copy)]
struct NoteOut {
    /// The pitch movement as a ratio (1 when nothing moves it).
    factor: f64,
    /// The level the oscillator's AM gives.
    am: f32,
    /// The level after the filter, left and right (R42, R45).
    vol: [f32; 2],
    /// The balance after the filter, −1 to 1.
    pan: f32,
    /// The filter's cutoff in the kit's semitones.
    cutoff: f32,
}

impl NoteOut {
    const IDLE: Self = Self {
        factor: 1.0,
        am: 1.0,
        vol: [1.0; 2],
        pan: 0.0,
        cutoff: 0.0,
    };
}

/// One note: its layers' voices share its pitch, its envelopes, its LFOs, its
/// oscillator and its filter lane (R27: a note's layers count as one voice).
#[derive(Clone, Copy)]
struct Note {
    active: bool,
    /// Fading out because it was stolen, retriggered or choked.
    fading: bool,
    released: bool,
    id: u64,
    /// The key it was played at, and the key its voices' pitches were made
    /// for (Glide's takeover changes the first).
    key: u8,
    base_key: u8,
    /// The tuned pitch of the key it was played at (the key's number
    /// without a tuning): what its voices' pitches were made for.
    base_pitch: f64,
    velocity: f32,
    release_velocity: f32,
    aftertouch: f32,
    /// A per-note pitch bend, −1 to 1.
    expr_bend: f32,
    /// How many voices it has.
    voices: u32,
    /// Its pitch in keys, and where Glide or Portamento takes it, and by how
    /// much a frame.
    pitch: f64,
    target: f64,
    glide_step: f64,
    /// R26: the times' scale from Time and Time < Key, then each envelope's
    /// own from Time < Vel, and the Time destination's.
    time_common: f64,
    vel_factor: [f64; 5],
    time_mul: f64,
    /// The pitch, filter, auxiliary and oscillator envelopes.
    envs: [Env; 4],
    lfos: [LfoState; 3],
    osc_phase: f64,
    osc_rng: u32,
    /// The last frame's modulation sums: a destination that changes a
    /// source's rate or scale takes effect a frame late.
    mods: [f32; DESTS],
    block: BlockMods,
}

impl Note {
    fn idle() -> Self {
        Self {
            active: false,
            fading: false,
            released: false,
            id: 0,
            key: 0,
            base_key: 0,
            base_pitch: 0.0,
            velocity: 0.0,
            release_velocity: 0.0,
            aftertouch: 0.0,
            expr_bend: 0.0,
            voices: 0,
            pitch: 0.0,
            target: 0.0,
            glide_step: 0.0,
            time_common: 1.0,
            vel_factor: [1.0; 5],
            time_mul: 1.0,
            envs: [Env::idle(); 4],
            lfos: [LfoState {
                phase: [0.0; 2],
                held: [0.0; 2],
                age: 0,
                rng: 1,
            }; 3],
            osc_phase: 0.0,
            osc_rng: 1,
            mods: [0.0; DESTS],
            block: BlockMods::default(),
        }
    }

    /// A note starting at `key`, whose pitch glides from `from`.
    fn start(
        s: &Settings,
        ctl: &Controls,
        free: &[f64; 3],
        (id, key, tuned, velocity): (u64, u8, f64, f32),
        from: f64,
        seed: u32,
        aftertouch: f32,
    ) -> Self {
        let mut note = Self::idle();
        note.active = true;
        note.id = id;
        note.key = key;
        note.base_key = key;
        note.base_pitch = tuned;
        note.velocity = velocity;
        note.aftertouch = aftertouch;
        note.pitch = from;
        note.target = tuned;
        if s.glide != 0 && s.glide_frames >= 1.0 && from != tuned {
            note.glide_step = (tuned - from) / s.glide_frames;
        } else {
            note.pitch = tuned;
        }
        note.time_common =
            10f64.powf(s.time) * 2f64.powf(-s.time_key * (f64::from(key) - 60.0) / 12.0);
        for (factor, vel) in note.vel_factor.iter_mut().zip(s.time_vel) {
            *factor = (1.0 - vel * f64::from(velocity)).max(0.0);
        }
        // The destinations a note starts with: R45's, from what is known
        // before it sounds.
        note.mods = start_mods(s, ctl, &note);
        note.time_mul = time_mul(note.mods[dest::TIME], s.time);
        for (i, env) in note.envs.iter_mut().enumerate() {
            env.start(
                &s.envs[i + 1],
                note.time_common * note.vel_factor[i + 1] * note.time_mul,
            );
        }
        for (n, state) in note.lfos.iter_mut().enumerate() {
            let l = &s.lfos[n];
            let start = if l.retrig { l.offset } else { free[n] };
            state.phase = [start, start + l.phase];
            state.phase.iter_mut().for_each(|p| *p -= p.floor());
            state.rng = (seed ^ (n as u32 + 1).wrapping_mul(0x9E37_79B9)) | 1;
            state.held = [random(&mut state.rng), random(&mut state.rng)];
        }
        note.osc_rng = seed.wrapping_mul(0x85EB_CA6B) | 1;
        note
    }

    /// The note ended (R18, R43): its envelopes release.
    fn release(&mut self, s: &Settings, velocity: f32) {
        self.released = true;
        self.release_velocity = velocity;
        for (i, env) in self.envs.iter_mut().enumerate() {
            env.release(
                &s.envs[i + 1],
                self.time_common * self.vel_factor[i + 1] * self.time_mul,
            );
        }
    }

    /// Moves the note a frame on: its glide, envelopes, LFOs and
    /// oscillator, the modulation they and the MIDI sources send (R45), and
    /// what that does to the pitch, the level, the pan and the filter.
    fn step(
        &mut self,
        s: &Settings,
        ctl: &Controls,
        (hits, waves): (&[bool; 5], &Waves),
    ) -> NoteOut {
        if self.glide_step != 0.0 {
            self.pitch += self.glide_step;
            if (self.glide_step > 0.0 && self.pitch >= self.target)
                || (self.glide_step < 0.0 && self.pitch <= self.target)
            {
                self.pitch = self.target;
                self.glide_step = 0.0;
            }
        }
        let prev = self.mods;
        self.time_mul = time_mul(prev[dest::TIME], s.time);
        let scales: [f64; 4] =
            core::array::from_fn(|i| self.time_common * self.vel_factor[i + 1] * self.time_mul);
        let mut env_values = [0f32; 4];
        for (i, value) in env_values.iter_mut().enumerate() {
            if s.env_used[i] {
                *value = self.envs[i].step(&s.envs[i + 1], scales[i], hits[i + 1]);
            }
        }
        // R41: the LFOs, each value read before its phase moves on.
        let lfo1_scale = (1.0 + prev[dest::LFO1_AMOUNT]).max(0.0);
        let mut lfo = [[0f32; 2]; 3];
        for (n, l) in s.lfos.iter().enumerate() {
            if !l.on {
                continue;
            }
            let state = &mut self.lfos[n];
            let rate_dest = [dest::LFO1_RATE, dest::LFO2_RATE, dest::LFO3_RATE][n];
            let mut hz = (l.hz + f64::from(prev[rate_dest]) * (LFO_MAX_HZ - LFO_MIN_HZ))
                .clamp(LFO_MIN_HZ, LFO_MAX_HZ);
            if l.key != 0.0 {
                hz *= 2f64.powf(l.key * (f64::from(self.key) - 60.0) / 12.0);
            }
            let step = hz / s.rate;
            let ramp = if l.attack <= 0.0 {
                1.0
            } else {
                (f64::from(state.age) / l.attack).min(1.0) as f32
            };
            state.age = state.age.saturating_add(1);
            // LFO 1 has the left channel's wave alone.
            for (channel, value) in lfo[n]
                .iter_mut()
                .enumerate()
                .take(if n == 0 { 1 } else { 2 })
            {
                *value = lfo_shape(l.wave, state.phase[channel], state.held[channel]) * ramp;
                state.phase[channel] += if channel == 0 {
                    step
                } else {
                    step * (1.0 + l.spin)
                };
                if state.phase[channel] >= 1.0 {
                    state.phase[channel] -= state.phase[channel].floor();
                    state.held[channel] = random(&mut state.rng);
                }
            }
        }
        // R45: the routings add up.
        let mut mods = [0f32; DESTS];
        let mut volume_extra = [0f32; 2];
        for route in &s.routes[..s.route_count] {
            let (left, right) = match route.source {
                Source::Aux => (env_values[2], env_values[2]),
                Source::Lfo2 => (lfo[1][0], lfo[1][1]),
                Source::Lfo3 => (lfo[2][0], lfo[2][1]),
                midi => {
                    let value = midi_source(midi, self, ctl);
                    (value, value)
                }
            };
            if route.dest == dest::VOLUME && matches!(route.source, Source::Lfo2 | Source::Lfo3) {
                volume_extra[0] += route.amount * left;
                volume_extra[1] += route.amount * right;
            } else {
                mods[route.dest] += route.amount * 0.5 * (left + right);
            }
        }
        // R32 to R36: the pitch, in semitones off the voices' own.
        let l1 = &s.lfos[0];
        let lfo1 = lfo[0][0];
        // Spec: multisampler R74, tunings R13. Under a tuning the per-note
        // bend moves the note by steps of the tuning; the wheel's stays in
        // semitones.
        let steps = f64::from(self.expr_bend) * s.bend_range;
        let note_bend = match s
            .keys
            .bent(self.base_key, steps)
            .zip(s.keys.pitch(self.base_key))
        {
            Some((bent, at)) if s.keys.is_tuned() && steps != 0.0 => bent - at,
            _ => steps,
        };
        let mut delta = (self.pitch - self.base_pitch)
            + s.tune
            + f64::from(ctl.bend) * s.bend_range
            + note_bend
            + f64::from(mods[dest::PITCH]) * 24.0;
        let env_amount =
            (s.pitch_amount + f64::from(mods[dest::PITCH_ENV]) * 96.0).clamp(-48.0, 48.0);
        let env_pitch = env_amount * f64::from(env_values[0]);
        delta += env_pitch;
        if l1.on {
            delta += 12.0 * f64::from(l1.amounts[3] * lfo1_scale * lfo1);
        }
        let mut am = 1.0;
        if s.osc.on {
            let depth = ((s.osc.volume + mods[dest::OSC_VOLUME])
                * env_values[3]
                * (1.0 - s.osc.vol_vel * (1.0 - self.velocity)).max(0.0))
            .clamp(0.0, 1.0);
            let mut hz = if s.osc.fixed {
                s.osc.hz
            } else {
                440.0 * 2f64.powf((self.pitch + s.tune - 69.0) / 12.0) * s.osc.ratio
            };
            // R35: the pitch envelope moves the oscillator with the voice.
            if mods[dest::OSC_PITCH] != 0.0 || env_pitch != 0.0 {
                hz *= 2f64.powf(f64::from(mods[dest::OSC_PITCH]) * 2.0 + env_pitch / 12.0);
            }
            let value = waves.value(s.osc.wave, self.osc_phase, &mut self.osc_rng);
            self.osc_phase += hz / s.rate;
            self.osc_phase -= self.osc_phase.floor();
            if s.osc.am {
                am = 1.0 - depth * (1.0 - value) / 2.0;
            } else {
                delta += 24.0 * f64::from(depth * value);
            }
        }
        // R37, R38: the cutoff.
        // R45: each sum is held to its parameter's range.
        let mut cutoff = (s.filter.pitch + mods[dest::FILTER_FREQ] * 96.0).clamp(
            pitch_of(20.0, REFERENCE_HZ),
            pitch_of(20_000.0, REFERENCE_HZ),
        );
        if s.filter.fenv_on {
            cutoff += (s.filter.fenv_amount + mods[dest::FILTER_ENV] * 72.0).clamp(0.0, 72.0)
                * env_values[1];
        }
        if l1.on {
            cutoff += l1.amounts[2] * lfo1_scale * lfo1;
        }
        self.block = BlockMods {
            res: (s.filter.res + mods[dest::FILTER_RES]).clamp(0.0, 1.0),
            morph: (s.filter.morph + mods[dest::FILTER_MORPH]).clamp(0.0, 1.0),
            drive: (s.filter.drive + mods[dest::FILTER_DRIVE]).clamp(0.0, 1.0),
            shaper: (s.shaper.amount + mods[dest::SHAPER]).clamp(0.0, 1.0),
        };
        // R42, R45: the level and the pan after the filter.
        let lfo_level = if l1.on {
            (1.0 - l1.amounts[0] * lfo1_scale * (1.0 - lfo1) / 2.0).max(0.0)
        } else {
            1.0
        };
        let level = 1.0 + mods[dest::VOLUME];
        let mut pan = mods[dest::PAN] * 2.0;
        if l1.on {
            pan += l1.amounts[1] * lfo1_scale * lfo1;
        }
        self.mods = mods;
        NoteOut {
            factor: if delta == 0.0 {
                1.0
            } else {
                (delta / 12.0).exp2()
            },
            am,
            vol: [
                (level + volume_extra[0]).max(0.0) * lfo_level,
                (level + volume_extra[1]).max(0.0) * lfo_level,
            ],
            pan: pan.clamp(-1.0, 1.0),
            cutoff,
        }
    }
}

/// What the Time destination multiplies every envelope's times by (R26,
/// R45): the Time control's own law over its whole range, the control
/// (`time`, −1 to 1) and the routings together held to that range.
fn time_mul(sum: f32, time: f64) -> f64 {
    if sum == 0.0 {
        1.0
    } else {
        10f64.powf((time + f64::from(sum) * 2.0).clamp(-1.0, 1.0) - time)
    }
}

/// A MIDI-tab source's value for a note (R43).
fn midi_source(source: Source, note: &Note, ctl: &Controls) -> f32 {
    match source {
        Source::Key => (f32::from(note.key) - 60.0) / 64.0,
        Source::Velocity => note.velocity,
        Source::ReleaseVelocity => note.release_velocity,
        Source::Aftertouch => note.aftertouch,
        Source::Wheel => ctl.wheel,
        Source::Foot => ctl.foot,
        Source::Bend => (ctl.bend + note.expr_bend).clamp(-1.0, 1.0),
        Source::Aux | Source::Lfo2 | Source::Lfo3 => 0.0,
    }
}

/// The modulation sums a note starts with: its MIDI sources, and the
/// auxiliary envelope at its Initial level. Sample Start, Loop Length and the
/// Sample Selector act when the note starts (a shortcut: they can't follow a
/// source that moves while the note sounds, and the LFOs and the running
/// envelope aren't read for them).
fn start_mods(s: &Settings, ctl: &Controls, note: &Note) -> [f32; DESTS] {
    let mut mods = [0f32; DESTS];
    for route in &s.routes[..s.route_count] {
        let value = match route.source {
            Source::Aux => s.envs[3].initial,
            Source::Lfo2 | Source::Lfo3 => continue,
            midi => midi_source(midi, note, ctl),
        };
        mods[route.dest] += route.amount * value;
    }
    mods
}

/// The oscillator's waveforms (R36): tables for the limited saws and
/// squares, which a note reads by linear interpolation.
struct Waves {
    tables: Vec<f32>,
}

impl Waves {
    fn new() -> Self {
        let mut tables = vec![0.0f32; 8 * WAVE_TABLE];
        for (index, limit) in [4usize, 8, 16, 32, 4, 8, 16, 32].into_iter().enumerate() {
            let square = index >= 4;
            for i in 0..WAVE_TABLE {
                let angle = core::f64::consts::TAU * i as f64 / WAVE_TABLE as f64;
                let mut sum = 0.0;
                for k in 1..=limit {
                    let k = k as f64;
                    if square {
                        if (k as usize) % 2 == 1 {
                            sum += (k * angle).sin() / k;
                        }
                    } else {
                        let sign = if (k as usize) % 2 == 1 { 1.0 } else { -1.0 };
                        sum += sign * (k * angle).sin() / k;
                    }
                }
                let scale = if square {
                    4.0 / core::f64::consts::PI
                } else {
                    2.0 / core::f64::consts::PI
                };
                tables[index * WAVE_TABLE + i] = (sum * scale) as f32;
            }
        }
        Self { tables }
    }

    /// The waveform `kind` (R36's choices) at `phase`, 0 to 1: −1 to 1.
    fn value(&self, kind: usize, phase: f64, noise: &mut u32) -> f32 {
        match kind {
            0 => (core::f64::consts::TAU * phase).sin() as f32,
            1 => {
                if phase < 0.25 {
                    (4.0 * phase) as f32
                } else if phase < 0.75 {
                    (2.0 - 4.0 * phase) as f32
                } else {
                    (4.0 * phase - 4.0) as f32
                }
            }
            2 => (2.0 * (phase + 0.5).fract() - 1.0) as f32,
            3 => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            4 => random(noise),
            limited => {
                let table = &self.tables[(limited - 5).min(7) * WAVE_TABLE..][..WAVE_TABLE];
                let at = phase * WAVE_TABLE as f64;
                let i = at as usize % WAVE_TABLE;
                let frac = (at - at.floor()) as f32;
                table[i] + (table[(i + 1) % WAVE_TABLE] - table[i]) * frac
            }
        }
    }
}

/// The shaper's curve (R39) for `x`, driven by `amount` (0 to 1) up to 24 dB.
fn shape(kind: usize, amount: f32, x: f32) -> f32 {
    let driven = x * 10f32.powf(amount * 24.0 / 20.0);
    match kind {
        0 => driven.tanh(),
        1 => driven.clamp(-1.0, 1.0),
        2 => driven.sin(),
        _ => (driven * 8.0).round().clamp(-8.0, 7.0) / 8.0,
    }
}

/// The volume envelope's parameters (R24).
const VOLUME_ENV: EnvIds = EnvIds {
    initial: ENV_INITIAL,
    peak: ENV_PEAK,
    sustain: ENV_SUSTAIN,
    end: None,
    attack: ENV_ATTACK,
    decay: ENV_DECAY,
    release: ENV_RELEASE,
    attack_slope: ENV_ATTACK_SLOPE,
    decay_slope: ENV_DECAY_SLOPE,
    release_slope: ENV_RELEASE_SLOPE,
    mode: ENV_LOOP,
    loop_time: ENV_LOOP_TIME,
    repeat: ENV_REPEAT,
    time_vel: ENV_TIME_VEL,
};

/// The filter and the buffers of the lanes it runs (R37, R76).
struct Lanes {
    filter: InstrumentFilter,
    /// How many values a frame of the per-lane buffers holds.
    stride: usize,
    /// Each lane's audio for the block: per frame, then per channel, one
    /// value per lane. The voices write it; the filter and the shaper
    /// change it in place.
    audio: Vec<f32>,
    /// Each lane's cutoff, Res, Morph and Drive, level and balance: per
    /// frame, one value per lane.
    cutoff: Vec<f32>,
    res: Vec<f32>,
    morph: Vec<f32>,
    drive: Vec<f32>,
    level: [Vec<f32>; 2],
    pan: Vec<f32>,
}

/// The multisampler's state between blocks: its layers, its notes and its
/// voices.
pub struct Multisampler {
    /// The layers and their decoded samples, as the compiler sent them.
    data: Option<NodeData>,
    /// A click in the Sample tab waiting for the next block (WP-5.15, R54).
    audition: Option<Audition>,
    /// The pitch of every key under the set's tuning.
    keys: KeyPitches,
    voices: Vec<Voice>,
    /// How many notes have started: voices are stolen oldest note first.
    counter: u64,
    /// The generator Round Robin's random modes and Pan < Rnd draw from,
    /// seeded from the device so a render repeats (R8).
    rng: u32,
    sinc: Sinc,
    waves: Waves,
    /// The voices sounding, by place in `voices`, so a block's frames visit
    /// only those; and the places left free.
    live: Vec<u16>,
    free: Vec<u16>,
    /// The notes, the ones sounding by place, the places free, and the ones
    /// that ended in this block, which join the free ones after it.
    notes: Vec<Note>,
    /// What each note gave its voices this frame.
    outs: Vec<NoteOut>,
    live_notes: Vec<u16>,
    free_notes: Vec<u16>,
    ended: Vec<u16>,
    /// The lanes whose filter starts afresh in the next block's filtering.
    fresh: Vec<(u16, u32)>,
    /// The voices that ended in this frame, whose notes may end with them.
    finished: Vec<u16>,
    lanes: Lanes,
    /// The channel's controllers (R43).
    controls: Controls,
    /// Where the free-running LFOs are, 0 to 1 of a cycle (R41).
    free_phase: [f64; 3],
    /// The tuned pitch of the key the last note played, where a gliding note
    /// starts (R34).
    last_pitch: Option<f64>,
    /// Whether the transport played in the last stretch.
    was_playing: bool,
    /// The engine's rate, for the steal fade of voices a new layer list
    /// drops.
    rate: f64,
    /// Where each parameter is in the bank, by id.
    index: Vec<u16>,
    seed: u32,
}

impl Multisampler {
    /// Makes a multisampler: its voices, notes, filters, buffers and the
    /// kernel are made here, off the audio thread. `seed` is the device's,
    /// for the random choices.
    fn new(config: &NodeConfig, seed: u64) -> Self {
        let hz = config.sample_rate.hz() as f32;
        let frames = config.max_frames.max(1);
        let filter = InstrumentFilter::new(hz, NOTE_SLOTS, frames);
        let stride = filter.stride();
        let description = multisampler_type().description;
        let top = description.params.iter().map(|p| p.id.0).max().unwrap_or(0) as usize;
        let mut index = vec![u16::MAX; top + 1];
        for (i, param) in description.params.iter().enumerate() {
            index[param.id.0 as usize] = i as u16;
        }
        Self {
            data: None,
            audition: None,
            voices: vec![Voice::idle(); SLOTS],
            counter: 0,
            rng: (seed as u32 ^ (seed >> 32) as u32).wrapping_mul(0x9E37_79B9) | 1,
            sinc: Sinc::new(),
            waves: Waves::new(),
            live: Vec::with_capacity(SLOTS),
            free: (0..SLOTS as u16).rev().collect(),
            notes: vec![Note::idle(); NOTE_SLOTS],
            outs: vec![NoteOut::IDLE; NOTE_SLOTS],
            live_notes: Vec::with_capacity(NOTE_SLOTS),
            free_notes: (0..NOTE_SLOTS as u16).rev().collect(),
            ended: Vec::with_capacity(NOTE_SLOTS),
            fresh: Vec::with_capacity(NOTE_SLOTS),
            finished: Vec::with_capacity(SLOTS),
            lanes: Lanes {
                filter,
                stride,
                audio: vec![0.0; frames * 2 * stride],
                cutoff: vec![0.0; frames * stride],
                res: vec![0.0; frames * stride],
                morph: vec![0.0; frames * stride],
                drive: vec![0.0; frames * stride],
                level: [vec![0.0; frames * stride], vec![0.0; frames * stride]],
                pan: vec![0.0; frames * stride],
            },
            controls: Controls::default(),
            free_phase: [0.0; 3],
            last_pitch: None,
            keys: KeyPitches::twelve(),
            was_playing: false,
            rate: config.sample_rate.as_f64(),
            index,
            seed: (seed as u32 ^ (seed >> 32) as u32) | 1,
        }
    }

    /// The next random number, 0 to below `n`.
    fn random(&mut self, n: usize) -> usize {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        ((u64::from(self.rng) * n as u64) >> 32) as usize
    }

    /// The settings one stretch of a block plays by.
    fn settings(
        &self,
        params: &ParamBank,
        rate: SampleRate,
        tempo: &TempoMap,
        position: Frames,
        speed: f64,
        playing: bool,
    ) -> Settings {
        let v = |id: ParamId| params.value(usize::from(self.index[id.0 as usize]));
        let on = |id: ParamId| v(id) >= 0.5;
        let hz = rate.as_f64();
        let ticks_now = tempo.frames_to_ticks(position, rate);
        let bpm = tempo.bpm_at(ticks_now).get() * speed;
        let frames_per_whole = hz * 60.0 / bpm * 4.0;
        let ticks_per_frame = Ticks::WHOLE.get() as f64 / frames_per_whole;
        let reset_whole = RESET_WHOLES[(v(RESET_INTERVAL) as usize).min(RESET_WHOLES.len() - 1)];
        let percent = |id: ParamId| (v(id) / 100.0) as f32;
        let ids = [VOLUME_ENV, PITCH_ENV, FILTER_ENV, AUX_ENV, OSC_ENV];
        let mut envs = [None; 5];
        let mut sync_first = [0.0; 5];
        let mut sync_period = [1.0; 5];
        let mut time_vel = [0.0; 5];
        for (i, ids) in ids.iter().enumerate() {
            let repeat_whole = REPEAT_WHOLES[(v(ids.repeat) as usize).min(REPEAT_WHOLES.len() - 1)];
            let repeat = (repeat_whole * frames_per_whole).max(1.0);
            let mode = match v(ids.mode) as usize {
                1 => EnvLoop::Loop,
                2 => EnvLoop::Beat,
                3 if playing => EnvLoop::Sync,
                // Stopped, song time doesn't move, so Sync repeats as Beat does.
                3 => EnvLoop::Beat,
                4 => EnvLoop::Trigger,
                _ => EnvLoop::None,
            };
            let ms = |id: ParamId| v(id) / 1000.0 * hz;
            envs[i] = Some(EnvSettings {
                initial: percent(ids.initial),
                peak: percent(ids.peak),
                sustain: percent(ids.sustain),
                end: ids.end.map_or(0.0, percent),
                volume: i == 0,
                attack: ms(ids.attack),
                decay: ms(ids.decay),
                release: ms(ids.release),
                loop_time: ms(ids.loop_time),
                attack_exp: exponent(v(ids.attack_slope)),
                decay_exp: exponent(v(ids.decay_slope)),
                release_exp: exponent(v(ids.release_slope)),
                mode,
                repeat,
            });
            time_vel[i] = v(ids.time_vel) / 100.0;
            // A frame within half a frame's ticks of the grid is on it, so a
            // stretch that starts there (one frame long, say) doesn't lose it.
            let period_ticks = repeat_whole * Ticks::WHOLE.get() as f64;
            sync_first[i] = if period_ticks > 0.0 {
                let past = (ticks_now.get() as f64).rem_euclid(period_ticks);
                let half = ticks_per_frame / 2.0;
                if past < half || period_ticks - past < half {
                    0.0
                } else {
                    (period_ticks - past) / ticks_per_frame
                }
            } else {
                0.0
            };
            sync_period[i] = repeat;
        }
        let envs = envs.map(|e| e.expect("each envelope was made"));
        let osc = OscSettings {
            on: on(OSC_ON),
            am: v(OSC_MODE) as usize == 1,
            wave: v(OSC_TYPE) as usize,
            volume: percent(OSC_VOLUME),
            vol_vel: percent(OSC_VOL_VEL),
            fixed: on(OSC_FIXED),
            hz: v(OSC_FREQ) * [0.01, 0.1, 1.0, 10.0][(v(OSC_MULTI) as usize).min(3)],
            ratio: v(OSC_COARSE) + v(OSC_FINE) / 1000.0,
        };
        let filter = FilterSettings {
            on: on(FILTER_ON),
            kind: (v(FILTER_TYPE) as usize).min(FILTER_TYPES.len() - 1),
            slope24: v(FILTER_SLOPE) as usize == 1,
            circuit: v(FILTER_CIRCUIT) as usize,
            pitch: pitch_of(v(FILTER_FREQ) as f32, REFERENCE_HZ),
            res: percent(FILTER_RES),
            morph: percent(FILTER_MORPH),
            drive: (v(FILTER_DRIVE) / 24.0) as f32,
            fenv_on: on(FENV_ON),
            fenv_amount: v(FENV_AMOUNT) as f32,
        };
        let shaper = ShaperSettings {
            on: on(SHAPER_ON),
            kind: (v(SHAPER_TYPE) as usize).min(SHAPER_TYPES.len() - 1),
            amount: percent(SHAPER_AMOUNT),
            post: on(SHAPER_POST),
        };
        let lfos: [LfoSettings; 3] = core::array::from_fn(|n| {
            let ids = LFOS[n];
            let free = v(ids.rate).clamp(LFO_MIN_HZ, LFO_MAX_HZ);
            let division = REPEAT_WHOLES[(v(ids.division) as usize).min(REPEAT_WHOLES.len() - 1)];
            LfoSettings {
                on: on(ids.on),
                wave: (v(ids.wave) as usize).min(LFO_WAVES.len() - 1),
                hz: if on(ids.sync) {
                    hz / (division * frames_per_whole)
                } else {
                    free
                },
                attack: v(ids.attack) / 1000.0 * hz,
                retrig: on(ids.retrig),
                offset: v(ids.offset) / 360.0,
                key: v(ids.key) / 100.0,
                phase: if n == 0 {
                    0.0
                } else {
                    v(ids.extra(LFO_PHASE)) / 360.0
                },
                spin: if n == 0 {
                    0.0
                } else {
                    v(ids.extra(LFO_SPIN)) / 100.0
                },
                amounts: if n == 0 {
                    [
                        percent(ids.extra(LFO1_VOL)),
                        percent(ids.extra(LFO1_PAN)),
                        v(ids.extra(LFO1_FILTER)) as f32,
                        percent(ids.extra(LFO1_PITCH)),
                    ]
                } else {
                    [0.0; 4]
                },
            }
        });
        // R45: the routings that send something somewhere.
        let mut routes = [Route {
            source: Source::Key,
            dest: 0,
            amount: 0.0,
        }; MAX_ROUTES];
        let mut route_count = 0;
        let mut add = |source: Source, dest_id: ParamId, amount_id: ParamId| {
            let dest = (v(dest_id) as usize).min(DESTS - 1);
            let amount = percent(amount_id);
            if dest != dest::NONE && amount != 0.0 && route_count < MAX_ROUTES {
                routes[route_count] = Route {
                    source,
                    dest,
                    amount,
                };
                route_count += 1;
            }
        };
        if on(AUX_ON) {
            add(Source::Aux, AUX_DEST_A, AUX_AMOUNT_A);
            add(Source::Aux, AUX_DEST_B, AUX_AMOUNT_B);
        }
        for (n, source) in [(1, Source::Lfo2), (2, Source::Lfo3)] {
            if lfos[n].on {
                let ids = LFOS[n];
                add(source, ids.extra(LFO_DEST_A), ids.extra(LFO_AMOUNT_A));
                add(source, ids.extra(LFO_DEST_B), ids.extra(LFO_AMOUNT_B));
            }
        }
        for (i, source) in [
            Source::Key,
            Source::Velocity,
            Source::ReleaseVelocity,
            Source::Aftertouch,
            Source::Wheel,
            Source::Foot,
            Source::Bend,
        ]
        .into_iter()
        .enumerate()
        {
            add(source, midi_param(i, 0), midi_param(i, 2));
            add(source, midi_param(i, 1), midi_param(i, 3));
        }
        let routed = |dest: usize| routes[..route_count].iter().any(|route| route.dest == dest);
        let pitch_amount = v(PITCH_ENV_AMOUNT);
        let env_used = [
            pitch_amount != 0.0 || routed(dest::PITCH_ENV),
            filter.on && filter.fenv_on && (filter.fenv_amount != 0.0 || routed(dest::FILTER_ENV)),
            routes[..route_count]
                .iter()
                .any(|route| route.source == Source::Aux),
            osc.on,
        ];
        Settings {
            rate: hz,
            selector: v(SELECTOR).clamp(0.0, 127.0) as u8,
            fade_mode: if v(FADE_MODE) as usize == 0 {
                FadeMode::Lin
            } else {
                FadeMode::Pow
            },
            round_robin: on(ROUND_ROBIN),
            rr_mode: v(RR_MODE) as usize,
            reset_ticks: (reset_whole * Ticks::WHOLE.get() as f64) as i64,
            reverse: on(REVERSE),
            interp: match v(INTERPOLATION) as usize {
                1 => Interp::Good,
                2 => Interp::Best,
                _ => Interp::Normal,
            },
            vel_vol: percent(VEL_VOL),
            pan_random: percent(PAN_RND),
            voices: (v(VOICES) as usize).clamp(1, MAX_VOICES),
            retrigger: on(RETRIGGER),
            time: v(TIME) / 100.0,
            time_key: v(TIME_KEY) / 100.0,
            envs,
            time_vel,
            env_used,
            steal_frames: ((STEAL_FADE_SECONDS * hz).round() as u32).max(1),
            // Filled in by the caller from the layers' options.
            power_loops: true,
            ticks: ticks_now.get() as f64,
            ticks_per_frame,
            playing,
            sync_first,
            sync_period,
            tune: v(TRANSPOSE) + v(DETUNE) / 100.0,
            spread: v(SPREAD) / 100.0 * SPREAD_CENTS,
            key_shift: v(KEY_ZONE_SHIFT) as i32,
            glide: v(GLIDE) as u8,
            glide_frames: v(GLIDE_TIME) / 1000.0 * hz,
            pitch_amount,
            bend_range: v(BEND_RANGE),
            keys: KeyPitches::twelve(),
            osc,
            filter,
            shaper,
            lfos,
            routes,
            route_count,
        }
    }

    /// Starts, releases or chokes voices for a note (R4, R27), or takes a
    /// channel message (R43).
    fn play(&mut self, layers: &DeviceLayers, note: NoteEvent, s: &Settings, frame: usize) {
        match note.kind {
            NoteKind::On => self.note_on(layers, note, s, frame, None),
            NoteKind::Off => {
                for voice in &mut self.voices {
                    if voice.active && !voice.fading && !voice.released && voice.key == note.key {
                        voice.note_off(&s.envs[0], self.notes[usize::from(voice.lane)].time_mul);
                    }
                }
                for &lane in &self.live_notes {
                    let held = &mut self.notes[usize::from(lane)];
                    if held.key == note.key && !held.fading && !held.released {
                        held.release(s, note.velocity);
                    }
                }
            }
            NoteKind::Choke => {
                for voice in &mut self.voices {
                    if voice.active && !voice.fading && voice.key == note.key {
                        voice.fading = true;
                        voice.env.fade(s.steal_frames);
                        self.notes[usize::from(voice.lane)].fading = true;
                    }
                }
            }
            NoteKind::Expression { expression, value } => {
                for &lane in &self.live_notes {
                    let held = &mut self.notes[usize::from(lane)];
                    if held.key == note.key && !held.fading {
                        match expression {
                            NoteExpression::Bend => held.expr_bend = value.clamp(-1.0, 1.0),
                            NoteExpression::Pressure => held.aftertouch = value.clamp(0.0, 1.0),
                            NoteExpression::Slide => {}
                        }
                    }
                }
            }
            NoteKind::Control { controller, value } => match controller {
                1 => self.controls.wheel = value.clamp(0.0, 1.0),
                4 => self.controls.foot = value.clamp(0.0, 1.0),
                _ => {}
            },
            NoteKind::PitchBend { value } => self.controls.bend = value.clamp(-1.0, 1.0),
            NoteKind::ChannelPressure { value } => {
                self.controls.pressure = value.clamp(0.0, 1.0);
                for &lane in &self.live_notes {
                    self.notes[usize::from(lane)].aftertouch = self.controls.pressure;
                }
            }
        }
    }

    /// Starts the voices a note plays. An audition (R54) names the one layer
    /// to play and the share of its sample to start from: it plays that
    /// layer alone, through its zones' full level, at its root key.
    fn note_on(
        &mut self,
        layers: &DeviceLayers,
        note: NoteEvent,
        s: &Settings,
        frame: usize,
        audition: Option<(usize, f64)>,
    ) {
        let velocity = note.velocity.clamp(0.0, 1.0);
        let vel7 = ((velocity * 127.0).round() as u8).clamp(1, 127);
        let key = note.key;
        // Spec: multisampler R74. A voice reads its sample at its key's
        // tuned pitch, and a key past the tuning's range plays nothing; a
        // click in the Sample tab plays the root key as it is.
        let tuned = if audition.is_some() {
            f64::from(key)
        } else {
            match s.keys.pitch(key) {
                Some(tuned) => tuned,
                None => return,
            }
        };
        // R34: Glide's monophonic mode takes the held note over, which goes
        // on from where it is to the new key; a note already released is
        // let go and the new one starts afresh.
        if s.glide == 1
            && let Some(lane) = self
                .live_notes
                .iter()
                .copied()
                .filter(|&lane| {
                    let held = &self.notes[usize::from(lane)];
                    !held.fading && !held.released
                })
                .max_by_key(|&lane| self.notes[usize::from(lane)].id)
        {
            let held = &mut self.notes[usize::from(lane)];
            let old = held.key;
            held.key = key;
            held.target = tuned;
            held.glide_step = if s.glide_frames >= 1.0 {
                (held.target - held.pitch) / s.glide_frames
            } else {
                held.pitch = held.target;
                0.0
            };
            for voice in &mut self.voices {
                if voice.active && !voice.fading && voice.key == old && voice.lane == lane {
                    voice.key = key;
                }
            }
            self.last_pitch = Some(tuned);
            return;
        }
        self.counter += 1;
        let id = self.counter;
        let from = if s.glide == 2 {
            self.last_pitch.unwrap_or(tuned)
        } else {
            tuned
        };
        self.last_pitch = Some(tuned);
        let proto = Note::start(
            s,
            &self.controls,
            &self.free_phase,
            (id, key, tuned, velocity),
            from,
            self.seed.wrapping_add(id as u32),
            self.controls.pressure,
        );
        // R33: Key Zone Shift moves the key the layers are chosen by, never
        // the pitch. R6, R45: the Sample Selector as the note starts.
        let zone_key = (i32::from(key) + s.key_shift).clamp(0, 127) as u8;
        let selector = (i32::from(s.selector) + (proto.mods[dest::SELECTOR] * 127.0).round() as i32)
            .clamp(0, 127) as u8;
        // R4: the layers the note's three values fall in.
        let matches = |i: usize| {
            let layer = &layers.layers[i].layer;
            layer.key.contains(zone_key)
                && layer.velocity.contains(vel7)
                && layer.select.contains(selector)
        };
        let mut chosen = [0u32; NOTE_LAYERS];
        let mut count = 0;
        let ticks = s.ticks + frame as f64 * s.ticks_per_frame;
        if let Some((layer, _)) = audition {
            chosen[0] = layer as u32;
            count = 1;
        } else if s.round_robin {
            for group in 0..layers.groups.len() {
                if count == NOTE_LAYERS {
                    break;
                }
                if let Some(layer) = self.rotate(layers, group, s, ticks as i64, id, &matches) {
                    chosen[count] = layer;
                    count += 1;
                }
            }
        } else {
            for i in 0..layers.layers.len() {
                if count == NOTE_LAYERS {
                    break;
                }
                if matches(i) {
                    chosen[count] = i as u32;
                    count += 1;
                }
            }
        }
        if count == 0 {
            return;
        }
        // R27: a key already sounding restarts, and a note past the voice
        // limit fades the oldest out.
        if s.retrigger {
            for voice in &mut self.voices {
                if voice.active && !voice.fading && voice.key == key {
                    voice.fading = true;
                    voice.env.fade(s.steal_frames);
                    self.notes[usize::from(voice.lane)].fading = true;
                }
            }
        }
        loop {
            let mut notes = 0;
            let mut last = 0;
            while let Some(next) = self
                .voices
                .iter()
                .filter(|v| v.active && !v.fading && v.note > last)
                .map(|v| v.note)
                .min()
            {
                notes += 1;
                last = next;
            }
            if notes < s.voices {
                break;
            }
            let oldest = self
                .voices
                .iter()
                .filter(|v| v.active && !v.fading)
                .map(|v| v.note)
                .min()
                .unwrap_or(0);
            for voice in &mut self.voices {
                if voice.active && !voice.fading && voice.note == oldest {
                    voice.fading = true;
                    voice.env.fade(s.steal_frames);
                    self.notes[usize::from(voice.lane)].fading = true;
                }
            }
        }
        let Some(lane) = self.free_notes.pop() else {
            return;
        };
        let pan_offset = if s.pan_random > 0.0 {
            let unit = self.random(2001) as f32 / 1000.0 - 1.0;
            unit * s.pan_random * 50.0
        } else {
            0.0
        };
        let level = 1.0 - s.vel_vol * (1.0 - velocity);
        let time_scale = proto.time_common * proto.vel_factor[0];
        // R32: Spread plays each layer as two voices, half the level each.
        let spreads: &[f64] = if s.spread > 0.0 { &[-1.0, 1.0] } else { &[0.0] };
        let split = 1.0 / spreads.len() as f32;
        let mut made = 0;
        for &index in &chosen[..count] {
            let entry = &layers.layers[index as usize];
            let (layer, Some(sample)) = (&entry.layer, entry.sample.as_ref()) else {
                continue;
            };
            let gain = if audition.is_some() {
                layer.volume as f32
            } else {
                layer.key.gain(zone_key, s.fade_mode)
                    * layer.velocity.gain(vel7, s.fade_mode)
                    * layer.select.gain(selector, s.fade_mode)
                    * layer.volume as f32
                    * level
            };
            if gain <= 0.0 || sample.frames() == 0 {
                continue;
            }
            let conv = sample.frames() as f64 / layer.length.max(1) as f64;
            let frames = |marker: u64| marker as f64 * conv;
            let end = frames(layer.end).min(sample.frames() as f64);
            // R45: Sample Start and Loop Length move by their share of the
            // playback span as the note starts.
            let span = (end - frames(layer.start)).max(0.0);
            let moved = |at: f64, share: f32| {
                if share == 0.0 {
                    at
                } else {
                    (at + f64::from(share) * span).clamp(0.0, (end - 1.0).max(0.0))
                }
            };
            let start = if layer.link {
                frames(layer.loop_start)
            } else {
                moved(frames(layer.start), proto.mods[dest::SAMPLE_START])
            };
            if end <= start {
                continue;
            }
            let loop_end = frames(layer.loop_end);
            let loop_start = if proto.mods[dest::LOOP_LENGTH] == 0.0 {
                frames(layer.loop_start)
            } else {
                let length = (loop_end - frames(layer.loop_start)
                    + f64::from(proto.mods[dest::LOOP_LENGTH]) * span)
                    .clamp(1.0, loop_end.max(1.0));
                loop_end - length
            };
            let semitones = tuned - f64::from(layer.root_key) + layer.detune / 100.0;
            let crossfade = (layer.crossfade_ms / 1000.0 * s.rate)
                .min((loop_end - loop_start) / 2.0)
                .max(0.0);
            for &spread in spreads {
                let Some(slot) = self.claim_slot() else {
                    continue;
                };
                let semitones = semitones + spread * s.spread / 100.0;
                let mut voice = Voice::idle();
                voice.active = true;
                voice.key = key;
                voice.note = id;
                voice.lane = lane;
                voice.layer = index;
                voice.reverse = s.reverse;
                voice.one_shot = layer.one_shot;
                voice.forward = !s.reverse;
                voice.audition = audition.is_some();
                voice.position = if let Some((_, from)) = audition {
                    (from.clamp(0.0, 1.0) * sample.frames() as f64).min((end - 1.0).max(0.0))
                } else if s.reverse {
                    (end - 1.0).max(start)
                } else {
                    start
                };
                voice.ratio = 2f64.powf(semitones / 12.0);
                voice.loop_ratio = 2f64.powf((semitones + layer.loop_detune / 100.0) / 12.0);
                voice.start = start;
                voice.end = end;
                voice.loop_start = loop_start;
                voice.loop_end = loop_end.min(end);
                voice.release_loop = frames(layer.release_loop).min(end);
                voice.crossfade = crossfade;
                voice.sustain = layer.sustain;
                voice.release = layer.release;
                voice.gain = gain * split;
                voice.pan = (layer.pan as f32 + pan_offset).clamp(-50.0, 50.0);
                voice.scale = time_scale;
                voice.env.start(&s.envs[0], time_scale * proto.time_mul);
                self.voices[slot] = voice;
                made += 1;
            }
        }
        if made == 0 {
            self.free_notes.push(lane);
            return;
        }
        let mut proto = proto;
        proto.voices = made;
        self.notes[usize::from(lane)] = proto;
        self.live_notes.push(lane);
        self.fresh.push((lane, frame as u32));
    }

    /// A click in the Sample tab (R54): the button going down plays the
    /// layer alone from the clicked point at its root key; going up fades
    /// that voice out.
    fn audition(&mut self, layers: &DeviceLayers, audition: Audition, s: &Settings) {
        let Some(id) = audition.layer else {
            return;
        };
        let Some(index) = layers.layers.iter().position(|entry| entry.layer.id == id) else {
            return;
        };
        if audition.down {
            let note = NoteEvent {
                offset: 0,
                kind: NoteKind::On,
                channel: 0,
                key: layers.layers[index].layer.root_key,
                velocity: 1.0,
                note_id: None,
            };
            self.note_on(layers, note, s, 0, Some((index, audition.from)));
        } else {
            for voice in &mut self.voices {
                if voice.active && voice.audition && !voice.fading {
                    voice.fading = true;
                    voice.env.fade(s.steal_frames);
                }
            }
        }
    }

    /// Chooses one of a group's layers for a note under Round Robin (R8,
    /// R9), among those the note matches: the group's turn. `None` if none
    /// matches or the group already chose for this note.
    fn rotate(
        &mut self,
        layers: &DeviceLayers,
        group: usize,
        s: &Settings,
        ticks: i64,
        note: u64,
        matches: &impl Fn(usize) -> bool,
    ) -> Option<u32> {
        let g = &layers.groups[group];
        let state = &g.state;
        if state.stamp.swap(note, Relaxed) == note {
            return None;
        }
        let mut candidates = [0u32; NOTE_LAYERS];
        let mut n = 0;
        for &member in &g.members {
            if n < NOTE_LAYERS && matches(member as usize) {
                candidates[n] = member;
                n += 1;
            }
        }
        if n == 0 {
            return None;
        }
        // R9: a new multiple of the Reset Interval restarts the rotation.
        if s.reset_ticks > 0 {
            let epoch = ticks.div_euclid(s.reset_ticks) + 1;
            if s.playing && state.epoch.swap(epoch, Relaxed) != epoch {
                state.last.store(0, Relaxed);
                state.restart.store(1, Relaxed);
            }
        }
        let candidates = &candidates[..n];
        let last = state.last.load(Relaxed);
        let first = state.first.load(Relaxed);
        let mut pick = match s.rr_mode {
            // Forward: the next below the last played, from the top after
            // the bottom.
            0 => candidates
                .iter()
                .copied()
                .find(|&m| m + 1 > last)
                .filter(|_| last != 0)
                .unwrap_or(candidates[0]),
            // Backward: the next above, from the bottom after the top.
            1 => candidates
                .iter()
                .rev()
                .copied()
                .find(|&m| m + 1 < last)
                .filter(|_| last != 0)
                .unwrap_or(candidates[n - 1]),
            // Other: random, never the one played last.
            2 if n > 1 => {
                let skip = candidates.iter().position(|&m| m + 1 == last);
                let mut pick = self.random(n - usize::from(skip.is_some()));
                if skip.is_some_and(|skip| pick >= skip) {
                    pick += 1;
                }
                candidates[pick]
            }
            // Random, and Other with one layer to choose.
            _ => candidates[self.random(n)],
        };
        if state.restart.swap(0, Relaxed) == 1
            && s.rr_mode >= 2
            && first != 0
            && candidates.contains(&(first - 1))
        {
            pick = first - 1;
        }
        state.last.store(pick + 1, Relaxed);
        if first == 0 {
            state.first.store(pick + 1, Relaxed);
        }
        Some(pick)
    }

    /// A place for a new voice: a free one, else the oldest fading voice's.
    fn claim_slot(&mut self) -> Option<usize> {
        if let Some(slot) = self.free.pop() {
            self.live.push(slot);
            return Some(usize::from(slot));
        }
        let slot = self
            .voices
            .iter()
            .enumerate()
            .filter(|(_, v)| v.fading)
            .min_by_key(|(_, v)| v.note)
            .map(|(slot, _)| slot)?;
        let lane = self.voices[slot].lane;
        self.end_voice(lane);
        Some(slot)
    }

    /// A voice of the note in lane `lane` is gone: the note ends with its
    /// last voice, and its lane is free after the block.
    fn end_voice(&mut self, lane: u16) {
        let note = &mut self.notes[usize::from(lane)];
        note.voices = note.voices.saturating_sub(1);
        if note.voices == 0 && note.active {
            note.active = false;
            if let Some(at) = self.live_notes.iter().position(|&l| l == lane) {
                self.live_notes.swap_remove(at);
            }
            self.ended.push(lane);
        }
    }

    /// The shaper over every lane's audio (R39), if it sits where `post`
    /// says.
    fn shape_lanes(&mut self, s: &Settings, frames: usize, hi: usize, post: bool) {
        if !s.shaper.on || s.shaper.post != post {
            return;
        }
        let stride = self.lanes.stride;
        for lane in 0..hi {
            let amount = self.notes[lane].block.shaper;
            if amount <= 0.0 {
                continue;
            }
            for frame in 0..frames {
                for channel in 0..2 {
                    let at = (frame * 2 + channel) * stride + lane;
                    let x = &mut self.lanes.audio[at];
                    *x = shape(s.shaper.kind, amount, *x);
                }
            }
        }
    }

    /// The filter over every lane's audio (R37, R38, R76).
    fn filter_lanes(&mut self, s: &Settings, frames: usize, hi: usize) {
        let f = &s.filter;
        let lanes = &mut self.lanes;
        lanes
            .filter
            .set_setup(Setup::new(f.kind, f.slope24, f.circuit));
        for &(lane, frame) in &self.fresh {
            lanes.filter.reset_voice(usize::from(lane), frame as usize);
        }
        let at = frames * lanes.stride;
        let controls = PerFrame {
            cutoff: &lanes.cutoff[..at],
            res: Per::Voice(&lanes.res[..at]),
            morph: Per::Voice(&lanes.morph[..at]),
            drive: Per::Voice(&lanes.drive[..at]),
        };
        let used = frames * 2 * lanes.stride;
        lanes
            .filter
            .process(hi, frames, &controls, &mut lanes.audio[..used]);
    }
}

impl NativeDevice for Multisampler {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if frames == 0 {
            return;
        }
        let playing = audio.playing();
        // The layers, held aside for the stretch: cloning the data only
        // moves a reference, so nothing allocates.
        let Some(data) = self.data.clone() else {
            return;
        };
        let Some(layers) = data.downcast_ref::<DeviceLayers>() else {
            return;
        };
        if playing && !self.was_playing {
            layers.reset_rotations();
        }
        self.was_playing = playing;
        let at = |id: ParamId| params.index(id).expect("the description holds it");
        let rate = audio.sample_rate();
        let mut s = self.settings(
            params,
            rate,
            audio.tempo(),
            audio.song_position(),
            audio.speed(),
            playing,
        );
        s.power_loops = !layers.linear_loop_fades;
        s.keys = self.keys;
        let volume = params.ramp(at(VOLUME));
        let pan = params.ramp(at(PAN));
        if let Some(audition) = self.audition.take() {
            self.audition(layers, audition, &s);
        }
        let mut notes = audio.notes().peekable();
        let idle = self.live.is_empty() && notes.peek().is_none();
        let mut channels = audio.channels_mut();
        let Some(left) = channels.next() else {
            return;
        };
        let mut right = channels.next();
        if idle {
            // The free-running LFOs go on while nothing sounds.
            for (phase, l) in self.free_phase.iter_mut().zip(&s.lfos) {
                if l.on {
                    *phase = (*phase + l.hz / s.rate * frames as f64).fract();
                }
            }
            left.fill(0.0);
            if let Some(right) = &mut right {
                right.fill(0.0);
            }
            return;
        }
        let stride = self.lanes.stride;
        self.lanes.audio[..frames * 2 * stride].fill(0.0);
        let mut hi = self
            .live_notes
            .iter()
            .map(|&lane| usize::from(lane) + 1)
            .max()
            .unwrap_or(0);
        let mut sync_in = s.sync_first;
        for frame in 0..frames {
            while notes
                .peek()
                .is_some_and(|note| note.offset as usize <= frame)
            {
                let note = notes.next().expect("peeked");
                self.play(layers, note, &s, frame);
                for &(lane, _) in &self.fresh {
                    hi = hi.max(usize::from(lane) + 1);
                }
            }
            let mut hits = [false; 5];
            for (i, hit) in hits.iter_mut().enumerate() {
                *hit = s.envs[i].mode == EnvLoop::Sync && frame as f64 >= sync_in[i];
                if *hit {
                    sync_in[i] += s.sync_period[i];
                }
            }
            for (phase, l) in self.free_phase.iter_mut().zip(&s.lfos) {
                if l.on {
                    *phase += l.hz / s.rate;
                    *phase -= phase.floor();
                }
            }
            // The notes: pitch, level, pan and cutoff for this frame.
            for &lane in &self.live_notes {
                let lane = usize::from(lane);
                let out = self.notes[lane].step(&s, &self.controls, (&hits, &self.waves));
                self.outs[lane] = out;
                let at = frame * stride + lane;
                self.lanes.cutoff[at] = out.cutoff;
                let block = self.notes[lane].block;
                self.lanes.res[at] = block.res;
                self.lanes.morph[at] = block.morph;
                self.lanes.drive[at] = block.drive;
                self.lanes.level[0][at] = out.vol[0];
                self.lanes.level[1][at] = out.vol[1];
                self.lanes.pan[at] = out.pan;
            }
            let sinc = &self.sinc;
            let mut at = 0;
            while at < self.live.len() {
                let slot = self.live[at];
                let voice = &mut self.voices[usize::from(slot)];
                let lane = voice.lane;
                if voice.active {
                    let out = self.outs[usize::from(lane)];
                    let time_mul = self.notes[usize::from(lane)].time_mul;
                    let level = voice.env.step(&s.envs[0], voice.scale * time_mul, hits[0]);
                    if voice.env.stage == Stage::Idle {
                        voice.active = false;
                    } else if let Some(sample) = layers
                        .layers
                        .get(voice.layer as usize)
                        .and_then(|entry| entry.sample.as_ref())
                    {
                        // R23: a frame the disk hasn't delivered plays as
                        // silence and counts as an underrun.
                        if sample.pending(voice.position as usize) {
                            layers.underruns.fetch_add(1, Relaxed);
                        }
                        let mut value = read(sinc, sample, voice.position, s.interp);
                        // The loop's wrap: a crossfade from its end into its
                        // start (R19), constant-power unless the title bar
                        // says linear.
                        if voice.sustain == SustainMode::Loop
                            && voice.looping()
                            && voice.forward
                            && voice.crossfade > 0.0
                            && voice.position >= voice.loop_end - voice.crossfade
                        {
                            let t = ((voice.position - (voice.loop_end - voice.crossfade))
                                / voice.crossfade) as f32;
                            let (out, into) = if s.power_loops {
                                (
                                    (t * core::f32::consts::FRAC_PI_2).cos(),
                                    (t * core::f32::consts::FRAC_PI_2).sin(),
                                )
                            } else {
                                (1.0 - t, t)
                            };
                            let head = voice.loop_start
                                + (voice.position - (voice.loop_end - voice.crossfade));
                            let from_start = read(sinc, sample, head, s.interp);
                            value = (
                                value.0 * out + from_start.0 * into,
                                value.1 * out + from_start.1 * into,
                            );
                        }
                        let amount = voice.gain * level * out.am;
                        let balance = ((voice.pan + pan.get(frame).copied().unwrap_or(0.0)) / 50.0)
                            .clamp(-1.0, 1.0);
                        let base = frame * 2 * stride + usize::from(lane);
                        self.lanes.audio[base] += value.0 * amount * (1.0 - balance.max(0.0));
                        self.lanes.audio[base + stride] +=
                            value.1 * amount * (1.0 + balance.min(0.0));
                        voice.advance(out.factor);
                    } else {
                        voice.active = false;
                    }
                }
                if self.voices[usize::from(slot)].active {
                    at += 1;
                } else {
                    self.live.swap_remove(at);
                    self.free.push(slot);
                    self.finished.push(lane);
                }
            }
            while let Some(lane) = self.finished.pop() {
                self.end_voice(lane);
            }
        }
        // The shaper before the filter, the filter, the shaper after it.
        if hi > 0 && (s.filter.on || s.shaper.on) {
            self.shape_lanes(&s, frames, hi, false);
            if s.filter.on {
                self.filter_lanes(&s, frames, hi);
            }
            self.shape_lanes(&s, frames, hi, true);
        }
        self.fresh.clear();
        while let Some(lane) = self.ended.pop() {
            self.free_notes.push(lane);
        }
        // Each lane's level and pan, then the device's Volume.
        let lanes = &self.lanes;
        for frame in 0..frames {
            let (mut l, mut r) = (0.0f32, 0.0f32);
            for lane in 0..hi {
                let at = frame * stride + lane;
                let base = frame * 2 * stride + lane;
                let balance = lanes.pan[at];
                l += lanes.audio[base] * lanes.level[0][at] * (1.0 - balance.max(0.0));
                r += lanes.audio[base + stride] * lanes.level[1][at] * (1.0 + balance.min(0.0));
            }
            let volume = volume.get(frame).copied().unwrap_or(1.0);
            left[frame] = l * volume;
            if let Some(right) = &mut right {
                right[frame] = r * volume;
            }
        }
    }

    fn receive_data(&mut self, data: NodeData) {
        if let Some(keys) = data.downcast_ref::<KeyPitches>() {
            self.keys = *keys;
            return;
        }
        if let Some(audition) = data.downcast_ref::<Audition>() {
            self.audition = Some(*audition);
            return;
        }
        // A voice names its layer by place in the list, and the new list may
        // order its layers otherwise or hold other samples: a voice follows
        // its layer to its new place, and one whose layer isn't kept as it
        // was fades out.
        let steal = ((STEAL_FADE_SECONDS * self.rate).round() as u32).max(1);
        let old = self.data.take();
        if let (Some(old), Some(new)) = (
            old.as_ref().and_then(|d| d.downcast_ref::<DeviceLayers>()),
            data.downcast_ref::<DeviceLayers>(),
        ) {
            for voice in &mut self.voices {
                if !voice.active {
                    continue;
                }
                let kept = old.layers.get(voice.layer as usize).and_then(|was| {
                    new.layers.iter().position(|now| {
                        now.layer.id == was.layer.id
                            && match (&now.sample, &was.sample) {
                                (Some(LayerSample::Whole(a)), Some(LayerSample::Whole(b))) => {
                                    std::sync::Arc::ptr_eq(a, b)
                                }
                                (
                                    Some(LayerSample::Progressive(a)),
                                    Some(LayerSample::Progressive(b)),
                                ) => std::sync::Arc::ptr_eq(a, b),
                                (None, None) => true,
                                _ => false,
                            }
                    })
                });
                match kept {
                    Some(index) => voice.layer = index as u32,
                    None if !voice.fading => {
                        voice.fading = true;
                        voice.env.fade(steal);
                        self.notes[usize::from(voice.lane)].fading = true;
                    }
                    None => {}
                }
            }
        }
        self.data = Some(data);
    }
}

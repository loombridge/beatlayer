//! Mosaic, the native multisampler (WP-5.13; the multisampler spec, R4 to
//! R9 and R15 to R28): a list of sample layers, each with a key zone, a
//! velocity zone and a sample select zone, played by notes through zone
//! fades, Round Robin, sustain and release loops, a volume envelope with
//! loop modes, and a voice limit.
//!
//! The layers reach this node as node data (R31): the compiler decodes each
//! layer's sample off the audio thread and sends the whole list, so a zone
//! or layer edit changes data and never the program. Every voice and buffer
//! is made with the node, and the audio thread only reads the data it was
//! sent.

use std::sync::atomic::Ordering::Relaxed;

use bl_core::time::{Frames, SampleRate, TempoMap, Ticks};
use bl_engine::{
    DeviceLayers, DeviceType, LayerSample, NodeConfig, NodeData, NoteEvent, NoteKind, Registry,
};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{
    DeviceDescription, DeviceLayout, DeviceRole, FadeMode, ParamDescriptor, ParamId, ParamUnit,
    ReleaseMode, SustainMode,
};

use crate::framework::{Audio, NativeDevice, ParamBank, add};

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

/// Taps in the windowed-sinc kernel Best reads with, and how finely the
/// kernel's table divides the gap between two frames.
const SINC_TAPS: usize = 32;
const SINC_PHASES: usize = 2048;

/// The Mosaic device type.
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
    let params = vec![
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
    DeviceType::native(MULTISAMPLER, "Mosaic", DeviceRole::Instrument)
        .with_description(DeviceDescription::new(params).with_layout(layout()))
}

/// The device's layout: its voice and zone controls, the sample's display,
/// the volume envelope and the output controls (R1). The tabs and the zone
/// editor are the app's own views over it.
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
    DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(zones),
        LayoutNode::Cell(display),
        LayoutNode::Band(envelope),
        LayoutNode::Band(output),
    ]))
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

/// What the volume envelope runs by for a stretch (R24 to R26).
#[derive(Clone, Copy)]
struct EnvSettings {
    initial: f32,
    peak: f32,
    sustain: f32,
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
    time_vel: f64,
    env: EnvSettings,
    steal_frames: u32,
    power_loops: bool,
    /// The stretch's first frame in ticks, and ticks per frame.
    ticks: f64,
    ticks_per_frame: f64,
    /// Whether the transport plays, so song time moves.
    playing: bool,
    /// For Sync: frames until the next multiple of Repeat in song time,
    /// and the frames between them.
    sync_first: f64,
    sync_period: f64,
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
        if !self.held || self.stage == Stage::Release || self.stage == Stage::Idle {
            return;
        }
        self.held = false;
        if set.mode == EnvLoop::Trigger && matches!(self.stage, Stage::Attack | Stage::Decay) {
            self.pending = true;
        } else {
            self.enter(Stage::Release, 0.0, set.release * scale, set.release_exp);
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
                        self.enter(Stage::Release, 0.0, set.release * scale, set.release_exp);
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
                    self.stage = Stage::Idle;
                    self.level = 0.0;
                    break;
                }
                Stage::Sustain | Stage::Idle => break,
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
    released: bool,
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
            released: false,
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
    fn note_off(&mut self, set: &EnvSettings) {
        self.released = true;
        if self.release != ReleaseMode::Off && !self.reverse {
            self.forward = true;
        }
        self.env.release(set, self.scale);
    }

    /// Moves the position on a frame and through the loops (R16 to R19).
    fn advance(&mut self) {
        let ratio = if self.in_loop && self.sustain != SustainMode::Off {
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
                            self.position = le - (self.position - le);
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
                self.position = self.end - (self.position - self.end);
                self.forward = false;
            } else if !self.forward && self.position < rl {
                self.position = rl + (rl - self.position);
                self.forward = true;
            }
        } else {
            self.in_loop = false;
        }
        if (self.forward && self.position >= self.end) || (!self.forward && self.position < self.start)
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
fn bessel_i0(x: f64) -> f64 {
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
fn read_channel(
    sinc: &Sinc,
    frame: impl Fn(usize) -> f32,
    position: f64,
    mode: Interp,
) -> f32 {
    if position < 0.0 {
        return 0.0;
    }
    let index = position as usize;
    let frac = (position - index as f64) as f32;
    let at = |i: i64| -> f32 {
        if i < 0 {
            0.0
        } else {
            frame(i as usize)
        }
    };
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

/// The multisampler's state between blocks: its layers and its voices.
pub struct Multisampler {
    /// The layers and their decoded samples, as the compiler sent them.
    data: Option<NodeData>,
    voices: Vec<Voice>,
    /// How many notes have started: voices are stolen oldest note first.
    counter: u64,
    /// The generator Round Robin's random modes and Pan < Rnd draw from,
    /// seeded from the device so a render repeats (R8).
    rng: u32,
    sinc: Sinc,
    /// The voices sounding, by place in `voices`, so a block's frames visit
    /// only those; and the places left free.
    live: Vec<u16>,
    free: Vec<u16>,
    /// Whether the transport played in the last stretch.
    was_playing: bool,
    /// The engine's rate, for the steal fade of voices a new layer list
    /// drops.
    rate: f64,
}

impl Multisampler {
    /// Makes a multisampler: its voices and the kernel are made here, off
    /// the audio thread. `seed` is the device's, for the random choices.
    fn new(config: &NodeConfig, seed: u64) -> Self {
        Self {
            data: None,
            voices: vec![Voice::idle(); SLOTS],
            counter: 0,
            rng: (seed as u32 ^ (seed >> 32) as u32).wrapping_mul(0x9E37_79B9) | 1,
            sinc: Sinc::new(),
            live: Vec::with_capacity(SLOTS),
            free: (0..SLOTS as u16).rev().collect(),
            was_playing: false,
            rate: config.sample_rate.as_f64(),
        }
    }

    /// The next random number, 0 to below `n`.
    fn random(&mut self, n: usize) -> usize {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (u64::from(self.rng) * n as u64 >> 32) as usize
    }

    /// The settings one stretch of a block plays by.
    fn settings(
        params: &ParamBank,
        rate: SampleRate,
        tempo: &TempoMap,
        position: Frames,
        playing: bool,
    ) -> Settings {
        let at = |id: ParamId| params.index(id).expect("the description holds it");
        let v = |id: ParamId| params.value(at(id));
        let on = |id: ParamId| v(id) >= 0.5;
        let hz = rate.as_f64();
        let ticks_now = tempo.frames_to_ticks(position, rate);
        let bpm = tempo.bpm_at(ticks_now).get();
        let frames_per_whole = hz * 60.0 / bpm * 4.0;
        let ticks_per_frame = Ticks::WHOLE.get() as f64 / frames_per_whole;
        let ms = |id: ParamId| v(id) / 1000.0 * hz;
        let repeat_whole = REPEAT_WHOLES[(v(ENV_REPEAT) as usize).min(REPEAT_WHOLES.len() - 1)];
        let repeat = repeat_whole * frames_per_whole;
        let reset_whole = RESET_WHOLES[(v(RESET_INTERVAL) as usize).min(RESET_WHOLES.len() - 1)];
        let period_ticks = repeat_whole * Ticks::WHOLE.get() as f64;
        // A frame within half a frame's ticks of the grid is on it, so a
        // stretch that starts there (one frame long, say) doesn't lose it.
        let ahead = if period_ticks > 0.0 {
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
        let mode = match v(ENV_LOOP) as usize {
            1 => EnvLoop::Loop,
            2 => EnvLoop::Beat,
            3 if playing => EnvLoop::Sync,
            // Stopped, song time doesn't move, so Sync repeats as Beat does.
            3 => EnvLoop::Beat,
            4 => EnvLoop::Trigger,
            _ => EnvLoop::None,
        };
        let percent = |id: ParamId| (v(id) / 100.0) as f32;
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
            time_vel: v(ENV_TIME_VEL) / 100.0,
            env: EnvSettings {
                initial: percent(ENV_INITIAL),
                peak: percent(ENV_PEAK),
                sustain: percent(ENV_SUSTAIN),
                attack: ms(ENV_ATTACK),
                decay: ms(ENV_DECAY),
                release: ms(ENV_RELEASE),
                loop_time: ms(ENV_LOOP_TIME),
                attack_exp: exponent(v(ENV_ATTACK_SLOPE)),
                decay_exp: exponent(v(ENV_DECAY_SLOPE)),
                release_exp: exponent(v(ENV_RELEASE_SLOPE)),
                mode,
                repeat: repeat.max(1.0),
            },
            steal_frames: ((STEAL_FADE_SECONDS * hz).round() as u32).max(1),
            // Filled in by the caller from the layers' options.
            power_loops: true,
            ticks: ticks_now.get() as f64,
            ticks_per_frame,
            playing,
            sync_first: ahead,
            sync_period: repeat.max(1.0),
        }
    }

    /// Starts, releases or chokes voices for a note (R4, R27).
    fn play(&mut self, layers: &DeviceLayers, note: NoteEvent, s: &Settings, frame: usize) {
        match note.kind {
            NoteKind::On => self.note_on(layers, note, s, frame),
            NoteKind::Off => {
                for voice in &mut self.voices {
                    if voice.active && !voice.fading && !voice.released && voice.key == note.key {
                        voice.note_off(&s.env);
                    }
                }
            }
            NoteKind::Choke => {
                for voice in &mut self.voices {
                    if voice.active && !voice.fading && voice.key == note.key {
                        voice.fading = true;
                        voice.env.fade(s.steal_frames);
                    }
                }
            }
            NoteKind::Expression { .. } => {}
        }
    }

    fn note_on(&mut self, layers: &DeviceLayers, note: NoteEvent, s: &Settings, frame: usize) {
        let velocity = note.velocity.clamp(0.0, 1.0);
        let vel7 = ((velocity * 127.0).round() as u8).clamp(1, 127);
        let key = note.key;
        self.counter += 1;
        let id = self.counter;
        // R4: the layers the note's three values fall in.
        let matches = |i: usize| {
            let layer = &layers.layers[i].layer;
            layer.key.contains(key) && layer.velocity.contains(vel7) && layer.select.contains(s.selector)
        };
        let mut chosen = [0u32; NOTE_LAYERS];
        let mut count = 0;
        let ticks = s.ticks + frame as f64 * s.ticks_per_frame;
        if s.round_robin {
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
                }
            }
        }
        let pan_offset = if s.pan_random > 0.0 {
            let unit = self.random(2001) as f32 / 1000.0 - 1.0;
            unit * s.pan_random * 50.0
        } else {
            0.0
        };
        let level = 1.0 - s.vel_vol * (1.0 - velocity);
        let time_scale = 10f64.powf(s.time)
            * 2f64.powf(-s.time_key * (f64::from(key) - 60.0) / 12.0)
            * (1.0 - s.time_vel * f64::from(velocity)).max(0.0);
        for &index in &chosen[..count] {
            let entry = &layers.layers[index as usize];
            let (layer, Some(sample)) = (&entry.layer, entry.sample.as_ref()) else {
                continue;
            };
            let gain = layer.key.gain(key, s.fade_mode)
                * layer.velocity.gain(vel7, s.fade_mode)
                * layer.select.gain(s.selector, s.fade_mode)
                * layer.volume as f32
                * level;
            if gain <= 0.0 || sample.frames() == 0 {
                continue;
            }
            let conv = sample.frames() as f64 / layer.length.max(1) as f64;
            let frames = |marker: u64| marker as f64 * conv;
            let start = if layer.link {
                frames(layer.loop_start)
            } else {
                frames(layer.start)
            };
            let end = frames(layer.end).min(sample.frames() as f64);
            if end <= start {
                continue;
            }
            let (loop_start, loop_end) = (frames(layer.loop_start), frames(layer.loop_end));
            let semitones =
                f64::from(key) - f64::from(layer.root_key) + layer.detune / 100.0;
            let crossfade = (layer.crossfade_ms / 1000.0 * s.rate)
                .min((loop_end - loop_start) / 2.0)
                .max(0.0);
            let Some(slot) = self.claim_slot() else {
                continue;
            };
            let mut voice = Voice::idle();
            voice.active = true;
            voice.key = key;
            voice.note = id;
            voice.layer = index;
            voice.reverse = s.reverse;
            voice.forward = !s.reverse;
            voice.position = if s.reverse {
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
            voice.gain = gain;
            voice.pan = (layer.pan as f32 + pan_offset).clamp(-50.0, 50.0);
            voice.scale = time_scale;
            voice.env.start(&s.env, time_scale);
            self.voices[slot] = voice;
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
        self.voices
            .iter()
            .enumerate()
            .filter(|(_, v)| v.fading)
            .min_by_key(|(_, v)| v.note)
            .map(|(slot, _)| slot)
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
        let mut s = Self::settings(params, rate, audio.tempo(), audio.song_position(), playing);
        s.power_loops = !layers.linear_loop_fades;
        let volume = params.ramp(at(VOLUME));
        let pan = params.ramp(at(PAN));
        let mut notes = audio.notes().peekable();
        let mut channels = audio.channels_mut();
        let Some(left) = channels.next() else {
            return;
        };
        let mut right = channels.next();
        let mut sync_in = s.sync_first;
        for frame in 0..frames {
            while notes
                .peek()
                .is_some_and(|note| note.offset as usize <= frame)
            {
                let note = notes.next().expect("peeked");
                self.play(layers, note, &s, frame);
            }
            let sync_hit = s.env.mode == EnvLoop::Sync && frame as f64 >= sync_in;
            if sync_hit {
                sync_in += s.sync_period;
            }
            let (mut l, mut r) = (0.0f32, 0.0f32);
            let sinc = &self.sinc;
            let mut at = 0;
            while at < self.live.len() {
                let slot = self.live[at];
                let voice = &mut self.voices[usize::from(slot)];
                if voice.active {
                    let level = voice.env.step(&s.env, voice.scale, sync_hit);
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
                        let amount = voice.gain * level;
                        let balance = ((voice.pan + pan.get(frame).copied().unwrap_or(0.0))
                            / 50.0)
                            .clamp(-1.0, 1.0);
                        l += value.0 * amount * (1.0 - balance.max(0.0));
                        r += value.1 * amount * (1.0 + balance.min(0.0));
                        voice.advance();
                    } else {
                        voice.active = false;
                    }
                }
                if self.voices[usize::from(slot)].active {
                    at += 1;
                } else {
                    self.live.swap_remove(at);
                    self.free.push(slot);
                }
            }
            let volume = volume.get(frame).copied().unwrap_or(1.0);
            left[frame] = l * volume;
            if let Some(right) = &mut right {
                right[frame] = r * volume;
            }
        }
    }

    fn receive_data(&mut self, data: NodeData) {
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
                    }
                    None => {}
                }
            }
        }
        self.data = Some(data);
    }
}

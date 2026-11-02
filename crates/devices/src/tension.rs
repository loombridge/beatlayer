//! Tension, the native string instrument (WP-8.22; spec: tension). A waveguide
//! string ([`bl_dsp::physical`]) is set moving by an exciter (a bow, a hammer,
//! a bouncing hammer or a plectrum), rests under a damper, is shortened by a
//! finger and fret, is heard by a pickup and coloured by a body; a multimode
//! filter with its envelope and LFO, the MPE routings, the keyboard,
//! portamento and unison are the Filter/Global tab's.
//!
//! Every law here is Beatlayer's own reading of the manual's controls on the
//! bowed and struck string models of the literature ([`model`]), and every
//! range and default is Beatlayer's own too.
//!
//! The voices are the instruments' voice section's ([`crate::instrument`]).
//! Each voice's unison copies are lanes of one waveguide bank, `copy × 32 +
//! voice`, so the lane arrays of every part below share that layout.

use core::f32::consts::TAU;
use core::ops::Range;

use bl_dsp::physical::{Fittings, Kind, WaveguideBank, WaveguideParams};
use bl_dsp::random::Random;
use bl_engine::{NodeConfig, NodeData, NodeEvent, Registry};
use bl_model::{ParamDescriptor, ParamId};
use lspk_dsp::filter::{FilterBank, VoiceParams, pitch_of};
use lspk_voice::{MAX_VOICES, Start, VoiceBlock, VoiceEvent, VoiceEventKind};

use crate::analog::{FIRST_FORMANT, FORMANT_RESONANCE, formants, kit_type};
use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::instrument::matrix::{
    DestDef, Kind as MatrixKind, Matrix, MatrixLayout, Pick, Range as MatrixRange, Rate, Scope,
    SlotDef, SourceDef,
};
use crate::instrument::{self, Instrument, NoteRange, Parts, Section, Sources, State};

mod model;
mod params;

pub use model::Kind as Exciter;
pub use params::*;

use model::{Damper, Strike, Strings};

/// Tension's device type.
pub const TENSION: &str = "beatlayer.tension";

/// The lanes of every per-voice buffer: a voice's copies side by side.
const LANES: usize = MAX_VOICES * COPIES;

/// Frames the engine plays in one go: the physical models' control grid, so
/// every block size gives the same render.
const CHUNK: usize = bl_dsp::physical::CONTROL;

/// The lowest note a string plays, in hertz.
const LOWEST: f32 = 16.0;

/// C3 (middle C), where Fix. Pos takes the positions' distances from (R5).
const C3_HZ: f32 = 261.625_58;

/// Where the string sounds before the output: its level at full velocity is
/// brought to about a quarter of full scale.
const OUTPUT: f32 = 24.0;

/// Control ticks between a vibrato lane's pitch changes: each is put in place
/// at once, so the loop is redesigned once in this many ticks and not in every
/// one, and the lanes' turns are staggered.
const VIBRATO_EVERY: u64 = 4;

/// Spec: tension R9. The vibrato's reach at Amount 100 %, in semitones, and
/// how far Error varies a note's rate, amount, delay and attack.
const VIBRATO_SEMITONES: f64 = 1.0;
const VIBRATO_ERROR: f64 = 0.5;

/// Spec: tension R13. The filter's modulation depths in semitones: the LFO's
/// four octaves and the envelope's six; and how much a velocity at 100 %
/// shortens the envelope's attack.
const LFO_SEMITONES: f32 = 48.0;
const ENV_SEMITONES: f32 = 72.0;
const ATTACK_VELOCITY: f32 = 16.0;

/// Spec: tension R12. Each size lowers the body's resonances by a fourth,
/// the body's resonances per type as `(first, spacing, curve)` of the
/// ratio `1 + spacing × k + curve × k²` to the first.
const BODY_STEP_SEMITONES: f64 = 5.0;
const BODY_MODES: usize = 16;
const BODIES: [(f64, f64, f64); 5] = [
    (90.0, 0.9, 0.05),
    (100.0, 0.8, 0.04),
    (280.0, 0.5, 0.03),
    (100.0, 0.6, 0.04),
    (55.0, 0.7, 0.05),
];
/// How much louder the body's resonators are made, to sit near the string.
const BODY_GAIN: f32 = 2.0;

const SEED: u64 = 0x07E1_5104;

/// Spec: tension R14. The matrix's sources, Pressure and Slide, and its
/// destinations: every control a route can reach.
#[must_use]
pub fn matrix_layout() -> MatrixLayout {
    let source = |name| SourceDef {
        name,
        range: MatrixRange::Unipolar,
        scope: Scope::Voice,
    };
    let dests = C::ALL.iter().filter(|c| c.is_dest()).map(|c| DestDef {
        name: c.name(),
        param: Some(c.id()),
        scope: if c.is_global() {
            Scope::Global
        } else {
            Scope::Voice
        },
        rate: Rate::Control,
        kind: MatrixKind::Additive,
    });
    MatrixLayout {
        sources: vec![source("Pressure"), source("Slide")],
        dests: dests.collect(),
        slots: MPE
            .iter()
            .enumerate()
            .map(|(n, &(dest, amount))| SlotDef {
                source: Pick::Fixed((n / 2) as u8),
                dest: Pick::Chosen(dest),
                amount,
            })
            .collect(),
        forbidden: Vec::new(),
    }
}

/// Spec: tension R1. Beatlayer's own factory presets: a name and the
/// parameters it sets, every other one at its default.
pub const PRESETS: [(&str, &[(ParamId, f64)]); 4] = [
    (
        "Nylon",
        &[
            (EXC_TYPE, 3.0),
            (C::ExcVelocity.id(), 40.0),
            (C::ExcPosition.id(), 18.0),
            (C::ExcProtrusion.id(), 45.0),
            (C::ExcStiffness.id(), 35.0),
            (C::StrDecay.id(), 2_500.0),
            (C::StrDamping.id(), 60.0),
            (BODY_ON, 1.0),
            (BODY_TYPE, 1.0),
        ],
    ),
    (
        "Felt Piano",
        &[
            (EXC_TYPE, 1.0),
            (C::ExcStiffness.id(), 30.0),
            (C::ExcMass.id(), 60.0),
            (C::ExcPosition.id(), 12.0),
            (C::StrDecay.id(), 6_000.0),
            (C::StrRatio.id(), 70.0),
            (C::StrInharm.id(), 15.0),
            (BODY_ON, 1.0),
            (BODY_TYPE, 0.0),
            (BODY_SIZE, 3.0),
        ],
    ),
    (
        "Slow Bow",
        &[
            (EXC_TYPE, 0.0),
            (C::ExcVelocity.id(), 45.0),
            (C::ExcForce.id(), 55.0),
            (C::ExcPosition.id(), 10.0),
            (C::StrDecay.id(), 4_000.0),
            (VIB_ON, 1.0),
            (C::VibAmount.id(), 25.0),
            (BODY_ON, 1.0),
            (BODY_TYPE, 3.0),
            (FILTER_ON, 1.0),
            (FILTER_MODE, 1.0),
            (C::FilterFreq.id(), 3_500.0),
        ],
    ),
    (
        "Bouncing Mallet",
        &[
            (EXC_TYPE, 2.0),
            (C::ExcMass.id(), 35.0),
            (C::ExcDamping.id(), 35.0),
            (C::ExcStiffness.id(), 65.0),
            (C::StrDecay.id(), 3_000.0),
            (C::StrInharm.id(), 25.0),
        ],
    ),
];

/// What a block reads from the parameters.
#[derive(Clone)]
struct Settings {
    /// Every control's value, and its Vel and Key amounts (0 where it has none).
    base: [f64; CONTROLS],
    vel: [f64; CONTROLS],
    key: [f64; CONTROLS],
    kind: Exciter,
    exc_fix: bool,
    damper_on: bool,
    damper_gated: bool,
    damper_fix: bool,
    vibrato: bool,
    termination: bool,
    pickup: bool,
    body: bool,
    body_type: usize,
    body_size: usize,
    filter: bool,
    mode: usize,
    envelope: bool,
    lfo: bool,
    wave: usize,
    synced: bool,
    division: usize,
    octave: f64,
    detune: f64,
    volume: f32,
}

impl Settings {
    fn new() -> Self {
        Self {
            base: [0.0; CONTROLS],
            vel: [0.0; CONTROLS],
            key: [0.0; CONTROLS],
            kind: Exciter::Hammer,
            exc_fix: false,
            damper_on: true,
            damper_gated: true,
            damper_fix: false,
            vibrato: false,
            termination: true,
            pickup: true,
            body: false,
            body_type: 1,
            body_size: 2,
            filter: false,
            mode: 0,
            envelope: false,
            lfo: false,
            wave: 0,
            synced: false,
            division: 4,
            octave: 0.0,
            detune: 0.0,
            volume: 0.0,
        }
    }

    fn read(&mut self, params: &ParamBank) {
        let v = |id: ParamId| params.index(id).map_or(0.0, |at| params.value(at));
        let on = |id| v(id) >= 0.5;
        for &c in C::ALL {
            let n = c.n();
            self.base[n] = v(c.id());
            let (vel, key) = c.amounts();
            self.vel[n] = if vel { v(c.vel()) / 100.0 } else { 0.0 };
            self.key[n] = if key { v(c.key()) / 100.0 } else { 0.0 };
        }
        self.kind = if on(EXC_ON) {
            Exciter::of(v(EXC_TYPE) as usize)
        } else {
            Exciter::Off
        };
        self.exc_fix = on(EXC_FIX);
        self.damper_on = on(DAMP_ON);
        self.damper_gated = on(DAMP_GATED);
        self.damper_fix = on(DAMP_FIX);
        self.vibrato = on(VIB_ON);
        self.termination = on(TERM_ON);
        self.pickup = on(PICKUP_ON);
        self.body = on(BODY_ON);
        self.body_type = v(BODY_TYPE) as usize;
        self.body_size = v(BODY_SIZE) as usize;
        self.filter = on(FILTER_ON);
        self.mode = v(FILTER_MODE) as usize;
        self.envelope = on(ENV_ON);
        self.lfo = on(LFO_ON);
        self.wave = v(LFO_WAVE) as usize;
        self.synced = on(LFO_SYNC);
        self.division = (v(LFO_DIV) as usize).min(DIVISIONS.len() - 1);
        self.octave = v(OCTAVE);
        self.detune = v(DETUNE);
        self.volume = v(VOLUME) as f32;
    }
}

/// A filter envelope's stage.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Attack,
    Decay,
    Sustain,
    Release,
}

/// What one lane keeps besides its string: its vibrato, filter envelope and
/// LFO, and the settings its string was last set to.
#[derive(Clone)]
struct Lane {
    released: bool,
    /// Frames since the lane's note started.
    age: u64,
    /// The unison copy's offset in cents, and its gain.
    cents: f32,
    gain: f32,
    /// The note's pitch in semitones at the last tick, for the filter's key.
    note: f32,
    /// The vibrato's phase, and how long it has run, in frames, with the
    /// note's Error factors on rate, amount, delay and attack.
    vibrato: f64,
    vibrato_age: f64,
    wander: [f64; 4],
    // The filter's envelope.
    stage: Stage,
    level: f32,
    velocity: f32,
    // The filter's LFO: its phase, the random value it holds and the one it
    // is going to.
    phase: f32,
    held: f32,
    next: f32,
    rng: Random,
    last: Option<(WaveguideParams, Fittings)>,
}

impl Lane {
    fn new() -> Self {
        Self {
            released: false,
            age: 0,
            cents: 0.0,
            gain: 1.0,
            note: 60.0,
            vibrato: 0.0,
            vibrato_age: 0.0,
            wander: [1.0; 4],
            stage: Stage::Release,
            level: 0.0,
            velocity: 0.0,
            phase: 0.0,
            held: 0.0,
            next: 0.0,
            rng: Random::seeded(SEED),
            last: None,
        }
    }
}

/// A voice: how many copies it plays, how loud it has been lately and
/// whether its key has been let go.
#[derive(Clone, Copy, Default)]
struct VoiceState {
    copies: usize,
    /// The loudest sample since the last grid point.
    peak: f32,
    released: bool,
    /// Frames of fade left once a released voice has gone quiet, which end
    /// at silence exactly: 0 before it starts, never ending the voice at a
    /// frame that depends on the block size.
    fade: u32,
    ended: bool,
}

/// Frames a quiet voice takes to fade to nothing before it ends, and the
/// level a released voice must have fallen below to start.
const FADE: u32 = CHUNK as u32;
const QUIET: f32 = 1e-5;

/// One body resonance: a two-pole resonator.
#[derive(Clone, Copy, Default)]
struct Mode {
    b: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

/// The body: sixteen resonances, then a low and a high cut (R12).
struct Body {
    modes: [Mode; BODY_MODES],
    key: (usize, usize, f32),
    high: [f32; 2],
}

impl Body {
    fn new() -> Self {
        Self {
            modes: [Mode::default(); BODY_MODES],
            key: (usize::MAX, 0, 0.0),
            high: [0.0; 2],
        }
    }

    /// Tunes the resonances to `kind` and `size`, ringing for `decay`
    /// seconds, if they aren't already.
    fn tune(&mut self, kind: usize, size: usize, decay: f32, rate: f64) {
        if self.key == (kind, size, decay) {
            return;
        }
        self.key = (kind, size, decay);
        let (first, spacing, curve) = BODIES[kind.min(BODIES.len() - 1)];
        let scale = ((2.0 - size.min(4) as f64) * BODY_STEP_SEMITONES / 12.0).exp2();
        for (k, mode) in self.modes.iter_mut().enumerate() {
            let k_f = k as f64;
            let hz = (first * scale * (1.0 + spacing * k_f + curve * k_f * k_f)).min(rate * 0.45);
            let theta = TAU as f64 * hz / rate;
            // Higher resonances ring for less time and sound less.
            let seconds = f64::from(decay) / (1.0 + 0.15 * k_f);
            let r = (-6.907_755 / (seconds * rate)).exp();
            let gain = 1.0 / (1.0 + 0.15 * k_f);
            let (z1, z2) = (mode.z1, mode.z2);
            *mode = Mode {
                b: ((1.0 - r) * 2.0 * theta.sin() * gain) as f32,
                a1: (2.0 * r * theta.cos()) as f32,
                a2: (-r * r) as f32,
                z1,
                z2,
            };
        }
    }

    /// The body's sound for one frame of the string, `x`, mixed at `share`
    /// (0 the string alone, 1 the body alone) and cut at the one-pole
    /// coefficients `cuts`.
    fn step(&mut self, x: f32, share: f32, cuts: (f32, f32)) -> f32 {
        let mut sum = 0.0;
        for mode in &mut self.modes {
            let y = mode.b * x + mode.a1 * mode.z1 + mode.a2 * mode.z2;
            mode.z2 = mode.z1;
            mode.z1 = y;
            sum += y;
        }
        let body = sum * BODY_GAIN;
        // A low-pass at the high cut, then a high-pass at the low cut.
        self.high[0] += cuts.1 * (body - self.high[0]);
        let high = self.high[0];
        self.high[1] += cuts.0 * (high - self.high[1]);
        let heard = high - self.high[1];
        x * (1.0 - share) + heard * share
    }

    fn reset(&mut self) {
        for mode in &mut self.modes {
            mode.z1 = 0.0;
            mode.z2 = 0.0;
        }
        self.high = [0.0; 2];
    }
}

/// The one-pole coefficient of a cut at `hz`.
fn pole(hz: f32, rate: f64) -> f32 {
    1.0 - (-TAU * hz / rate as f32).exp()
}

/// A pitch in semitones as a twelve-step key counts them, in hertz.
fn hz_of_pitch(pitch: f64) -> f32 {
    (440.0 * ((pitch - 69.0) / 12.0).exp2()) as f32
}

/// The sound engine the voice section plays.
struct Engine {
    rate: f64,
    bpm: f64,
    settings: Settings,
    descriptors: Vec<ParamDescriptor>,
    /// Each control's destination in the matrix, if it has one.
    dests: [Option<usize>; CONTROLS],
    matrix: Matrix,
    /// Every voice's played controls, and the shared controls'.
    played: Vec<[f64; CONTROLS]>,
    shared: [f64; CONTROLS],
    bank: WaveguideBank,
    strings: Strings,
    lanes: Vec<Lane>,
    voices: [VoiceState; MAX_VOICES],
    /// How many notes have started: each note's randomness is seeded from it.
    notes: u64,
    stride: usize,
    input: Vec<f32>,
    heard: Vec<f32>,
    // The filter: the bank, its second for a formant, and a chunk of cutoffs
    // and audio per lane.
    filters: [FilterBank; 2],
    kind_of: Option<usize>,
    resonance_of: Vec<f32>,
    pitch: [Vec<f32>; 2],
    audio: [Vec<f32>; 2],
    body: Body,
    cuts: (f32, f32),
    out: Vec<f32>,
    dc: [f32; 2],
    dc_pole: f32,
}

/// Tension.
pub struct Tension {
    section: Section,
    engine: Engine,
}

impl Tension {
    /// Makes a Tension for `config`'s rate and block size. Every buffer and
    /// table is made here, off the audio thread.
    #[must_use]
    pub fn new(device: &bl_model::Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let ty = tension_type();
        let layout = ty.matrix.clone().expect("Tension has a matrix");
        let bank = WaveguideBank::new(rate as f32, LANES, Kind::String, LOWEST);
        let stride = bank.stride();
        let filter = || {
            let mut bank = FilterBank::new(rate as f32, LANES, 1);
            bank.set_type(kit_type(0));
            bank
        };
        let descriptors: Vec<ParamDescriptor> = C::ALL
            .iter()
            .map(|c| {
                ty.description
                    .param(c.id())
                    .expect("the description lists every control")
                    .clone()
            })
            .collect();
        let mut dests = [None; CONTROLS];
        for (i, c) in C::ALL.iter().filter(|c| c.is_dest()).enumerate() {
            dests[c.n()] = Some(i);
        }
        let mut engine = Engine {
            rate,
            bpm: 120.0,
            settings: Settings::new(),
            descriptors,
            dests,
            matrix: Matrix::of(layout, device),
            played: vec![[0.0; CONTROLS]; MAX_VOICES],
            shared: [0.0; CONTROLS],
            bank,
            strings: Strings::new(LANES),
            lanes: vec![Lane::new(); LANES],
            voices: [VoiceState::default(); MAX_VOICES],
            notes: 0,
            stride,
            input: vec![0.0; CHUNK * stride],
            heard: vec![0.0; CHUNK * stride * 2],
            filters: [filter(), filter()],
            kind_of: None,
            resonance_of: vec![f32::NAN; LANES],
            pitch: [vec![0.0; CHUNK * LANES], vec![0.0; CHUNK * LANES]],
            audio: [vec![0.0; CHUNK * LANES], vec![0.0; CHUNK * LANES]],
            body: Body::new(),
            cuts: (0.0, 1.0),
            out: vec![0.0; config.max_frames.max(1)],
            dc: [0.0; 2],
            dc_pole: (-f64::from(TAU) * 5.0 / rate).exp() as f32,
        };
        engine.shared = engine.settings.base;
        Self {
            section: Section::new(
                Parts {
                    voices: Some(VOICES),
                    voices_one_is_mono: true,
                    priority: Some(PRIORITY),
                    glide_time: Some(PORT_TIME),
                    glide_on: Some(PORT_ON),
                    glide_prop: Some(PORT_PROP),
                    glide_legato: Some(PORT_LEGATO),
                    unison_voices: Some(UNI_VOICES),
                    unison_on: Some(UNI_ON),
                    unison_choice: true,
                    unison_detune: Some(UNI_DETUNE),
                    unison_delay: Some(UNI_DELAY),
                    pb_range: Some(PB_RANGE),
                    note_range: NoteRange::Own(NOTE_PB),
                    transpose: Some(SEMI),
                    stretch: Some(STRETCH),
                    error: Some(ERROR),
                    ..Parts::default()
                },
                MAX_VOICES,
                COPIES,
                rate,
                SEED,
            ),
            engine,
        }
    }
}

impl NativeDevice for Tension {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if frames == 0 || audio.channels() == 0 {
            return;
        }
        let engine = &mut self.engine;
        engine.settings.read(params);
        engine.bpm = audio.bpm();
        let mpe = params
            .index(MPE_ON)
            .is_none_or(|at| params.value(at) >= 0.5);
        engine.matrix.set_amount(if mpe { 1.0 } else { 0.0 });
        engine.matrix.begin(params);
        engine.out[..frames].fill(0.0);
        self.section.process(audio, params, engine);
        for channel in 0..audio.channels() {
            audio
                .channel_mut(channel)
                .copy_from_slice(&self.engine.out[..frames]);
        }
    }

    fn flush(&mut self, _events: &[NodeEvent], _params: &ParamBank) {
        self.section.reset();
        self.engine.silence();
    }

    fn matrix_played(&self, report: &mut dyn FnMut(ParamId, f64)) {
        let engine = &self.engine;
        for &c in C::ALL {
            if let Some(position) = engine.dests[c.n()].and_then(|d| engine.matrix.shown(d)) {
                report(c.id(), engine.descriptors[c.n()].denormalize(position));
            }
        }
    }

    fn receive_data(&mut self, data: NodeData) {
        self.section.receive_data(&data);
        if let Some(routes) = data.downcast_ref::<bl_engine::ModRoutes>() {
            self.engine.matrix.set_free(routes.0);
        }
    }
}

/// What the matrix's sources read for a voice: Pressure and Slide.
fn reader<'a>(sources: &'a Sources<'a>) -> impl Fn(usize, usize, usize) -> f64 + 'a {
    move |source, voice, _frame| {
        if source == 0 {
            sources.pressure(voice)
        } else {
            sources.slide(voice)
        }
    }
}

impl Engine {
    /// Forgets every note, without allocating.
    fn silence(&mut self) {
        self.notes = 0;
        for lane in 0..LANES {
            self.strings.lanes[lane].stop();
            self.lanes[lane] = Lane::new();
            self.bank.reset_voice(lane, &WaveguideParams::default());
        }
        self.voices = [VoiceState::default(); MAX_VOICES];
        for filter in &mut self.filters {
            filter.reset();
        }
        self.resonance_of.fill(f32::NAN);
        self.body.reset();
        self.dc = [0.0; 2];
    }

    /// Control `c`'s value as `voice` plays it (spec: instrument modulation
    /// R5): its base moved by velocity and the key each by its amount over
    /// half the travel, then by the matrix's routes.
    fn play(
        &self,
        c: C,
        voice: usize,
        (velocity, key): (f64, f64),
        at: usize,
        read: &impl Fn(usize, usize, usize) -> f64,
        due: bool,
    ) -> f64 {
        let (n, s) = (c.n(), &self.settings);
        let descriptor = &self.descriptors[n];
        let travel = descriptor.normalize(s.base[n])
            + 0.5 * (s.vel[n] * (2.0 * velocity - 1.0) + s.key[n] * key);
        let travel = travel.clamp(0.0, 1.0);
        let moved = match self.dests[n] {
            Some(dest) => self.matrix.position(dest, travel, voice, at, due, read),
            None => travel,
        };
        descriptor.denormalize(moved)
    }

    /// Works out every control for `voice`: what it plays, from its own
    /// velocity, key and MPE sources.
    fn work_out(&mut self, sources: &Sources<'_>, voice: usize, at: usize, due: bool) {
        let read = reader(sources);
        let amounts = (sources.velocity(voice), sources.key(voice));
        let mut played = [0.0; CONTROLS];
        for &c in C::ALL {
            played[c.n()] = self.play(c, voice, amounts, at, &read, due);
        }
        self.played[voice] = played;
    }

    /// The exciter's settings for a voice's played controls, at `hz`.
    fn strike(&self, p: &[f64; CONTROLS], hz: f32) -> Strike {
        let share = |c: C| (p[c.n()] / 100.0) as f32;
        Strike {
            kind: self.settings.kind,
            velocity: share(C::ExcVelocity),
            force: share(C::ExcForce),
            friction: share(C::ExcFriction),
            mass: share(C::ExcMass),
            stiffness: share(C::ExcStiffness),
            protrusion: share(C::ExcProtrusion),
            damping: share(C::ExcDamping),
            cycles: hz / self.rate as f32,
        }
    }

    /// A string's settings for a voice's played controls, at `hz` (R5, R7,
    /// R10, R11).
    fn string(&self, p: &[f64; CONTROLS], hz: f32, released: bool) -> (WaveguideParams, Fittings) {
        let s = &self.settings;
        let share = |c: C| (p[c.n()] / 100.0) as f32;
        let fixed = |position: f32, fix: bool| {
            let scaled = if fix { position * hz / C3_HZ } else { position };
            scaled.clamp(0.0, 0.5)
        };
        let ratio = share(C::StrRatio);
        let seconds =
            (p[C::StrDecay.n()] / 1000.0) as f32 * if released { 1.0 - 0.9 * ratio } else { 1.0 };
        let (mass, stiff, fret) = (
            share(C::FingerMass),
            share(C::FingerStiff),
            share(C::FretStiff),
        );
        // The finger takes the high harmonics: a heavy, soft one the most; a
        // sharp fret gives some back (R10).
        let end = if s.termination {
            1.0 - 0.5 * mass * (1.0 - stiff) * (1.5 - fret)
        } else {
            1.0
        };
        let pickup = if s.pickup {
            share(C::PickupPosition)
        } else {
            0.5
        };
        (
            WaveguideParams {
                hz,
                decay: seconds.clamp(1e-3, 60.0),
                material: 1.0 - share(C::StrDamping),
                inharm: share(C::StrInharm),
                excite: fixed(share(C::ExcPosition), s.exc_fix),
                pickup_l: pickup,
                pickup_r: pickup,
                ..WaveguideParams::default()
            },
            Fittings {
                damper: fixed(share(C::DampPosition), s.damper_fix),
                end,
            },
        )
    }

    /// The hertz of lane `lane`'s note `at` frames into the stretch, with its
    /// copy's detune and the vibrato's move (R9, R15, R17).
    fn hertz(
        &mut self,
        state: &State,
        voices: &VoiceBlock,
        lane: usize,
        voice: usize,
        frames: &Range<usize>,
        at: usize,
    ) -> f32 {
        let s = &self.settings;
        let note = state.pitch(voices, voice, frames, at - frames.start);
        let l = &mut self.lanes[lane];
        l.note = note as f32;
        let mut pitch = note + 12.0 * s.octave + s.detune / 100.0 + f64::from(l.cents) / 100.0;
        if s.vibrato {
            let p = &self.played[voice];
            let value = |c: C| p[c.n()];
            let frame = 1.0 / self.rate;
            let (delay, attack) = (
                value(C::VibDelay) * 1e-3 * l.wander[2],
                value(C::VibAttack) * 1e-3 * l.wander[3],
            );
            let seconds = l.vibrato_age * frame;
            let fade = if seconds < delay {
                0.0
            } else if attack > 0.0 {
                ((seconds - delay) / attack).min(1.0)
            } else {
                1.0
            };
            let wheel = Sources::of(voices).mod_wheel();
            let amount = ((value(C::VibAmount) / 100.0 + value(C::VibMod) / 100.0 * wheel)
                * l.wander[1])
                .clamp(0.0, 1.0);
            pitch += VIBRATO_SEMITONES * amount * fade * (std::f64::consts::TAU * l.vibrato).sin();
            let rate = value(C::VibRate) * l.wander[0];
            l.vibrato = (l.vibrato + rate * CHUNK as f64 * frame).fract();
            l.vibrato_age += CHUNK as f64;
        }
        let hz = f64::from(hz_of_pitch(pitch));
        hz.clamp(f64::from(LOWEST), self.rate / 4.0) as f32
    }

    /// A note starts on `voice`, `time` frames into the stretch.
    fn start(
        &mut self,
        voices: &mut VoiceBlock,
        state: &State,
        voice: usize,
        time: usize,
        start: Start,
    ) {
        let sources = Sources::of(voices);
        self.matrix.start(voice);
        self.work_out(&sources, voice, time, true);
        let velocity = sources.velocity(voice) as f32;
        let config = *state.config();
        let copies = config.copies;
        self.voices[voice] = VoiceState {
            copies,
            ..VoiceState::default()
        };
        // A legato note carries the string on: it only moves its pitch.
        if start == Start::Legato {
            return;
        }
        self.notes += 1;
        let seed = SEED ^ self.notes.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut random = Random::seeded(seed);
        let error = self.played[voice][C::VibError.n()] / 100.0 * VIBRATO_ERROR;
        let wander: [f64; 4] = std::array::from_fn(|_| 1.0 + error * random.next_bipolar());
        let (p, s) = (self.played[voice], self.settings.clone());
        let damper = Damper {
            on: s.damper_on,
            gated: s.damper_gated,
            mass: (p[C::DampMass.n()] / 100.0) as f32,
            stiffness: (p[C::DampStiffness.n()] / 100.0) as f32,
            velocity: (p[C::DampVelocity.n()] / 100.0) as f32,
            damping: (p[C::DampDamping.n()] / 100.0) as f32,
            frame: 1.0 / self.rate as f32,
        };
        let frames = time..time;
        for copy in 0..COPIES {
            let lane = copy * MAX_VOICES + voice;
            if copy >= copies {
                self.strings.lanes[lane].stop();
                continue;
            }
            let cents = instrument::copy_cents(copy, copies, config.detune);
            self.lanes[lane] = Lane {
                age: 0,
                cents,
                gain: instrument::copy_gains(copy, copies, 0.0).0,
                wander,
                velocity,
                stage: Stage::Attack,
                level: 0.0,
                rng: Random::seeded(seed ^ (copy as u64 + 1)),
                ..Lane::new()
            };
            let hz = self.hertz(state, voices, lane, voice, &frames, time);
            let (string, fittings) = self.string(&p, hz, false);
            self.bank.reset_voice_with(lane, &string, &fittings);
            self.lanes[lane].last = Some((string, fittings));
            let wait = instrument::copy_delay_frames(copy, copies, config.delay_ms, self.rate);
            let strike = self.strike(&p, hz);
            self.strings.lanes[lane].start(&strike, &damper, wait.min(u64::from(u32::MAX)) as u32);
            self.filters[0].reset_voice(lane, &VoiceParams::default());
            self.filters[1].reset_voice(lane, &VoiceParams::default());
            self.resonance_of[lane] = f32::NAN;
        }
    }

    /// The key is let go on `voice`.
    fn release(&mut self, voice: usize) {
        self.voices[voice].released = true;
        for copy in 0..COPIES {
            let lane = copy * MAX_VOICES + voice;
            self.strings.lanes[lane].release();
            self.lanes[lane].released = true;
            self.lanes[lane].stage = Stage::Release;
        }
    }

    /// The lanes of the voices that sound, as the highest one's count.
    fn active_lanes(&self, mask: u32) -> usize {
        let mut top = 0;
        let mut bits = mask;
        while bits != 0 {
            let v = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            top = top.max((self.voices[v].copies.max(1) - 1) * MAX_VOICES + v + 1);
        }
        top
    }

    /// The control tick, on the grid: each sounding voice's controls are
    /// worked out and each of its lanes' strings moved to them.
    fn tick(&mut self, voices: &VoiceBlock, state: &State, frames: &Range<usize>, at: usize) {
        let sources = Sources::of(voices);
        let oldest = state.oldest(voices);
        self.matrix.set_oldest(oldest);
        self.matrix.set_newest(state.newest(voices));
        if let Some(oldest) = oldest {
            self.work_out(&sources, oldest, at, true);
            self.shared = self.played[oldest];
        } else {
            self.shared = self.settings.base;
        }
        let mut bits = voices.active_mask();
        while bits != 0 {
            let v = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            self.work_out(&sources, v, at, true);
            let p = self.played[v];
            for copy in 0..self.voices[v].copies.max(1) {
                let lane = copy * MAX_VOICES + v;
                let hz = self.hertz(state, voices, lane, v, frames, at);
                let vibrato = self.settings.vibrato;
                if vibrato
                    && !(state.clock(at) / CHUNK as u64 + lane as u64).is_multiple_of(VIBRATO_EVERY)
                {
                    continue;
                }
                let released = self.lanes[lane].released;
                let setting = self.string(&p, hz, released);
                if self.lanes[lane].last != Some(setting) {
                    self.bank.set_params(lane, &setting.0);
                    self.bank.set_fittings(lane, &setting.1);
                    if vibrato {
                        self.bank.set_pitch(lane, hz);
                    }
                    self.lanes[lane].last = Some(setting);
                }
                let strike = self.strike(&p, hz);
                self.strings.lanes[lane].set_bow(&strike);
            }
        }
    }

    /// Plays frames `at..at + n` of the stretch, up to the next grid point:
    /// the strings, the filter, the mix and the body.
    fn chunk(
        &mut self,
        voices: &VoiceBlock,
        state: &State,
        frames: &Range<usize>,
        at: usize,
        n: usize,
    ) {
        if state.clock(at).is_multiple_of(CHUNK as u64) {
            self.tick(voices, state, frames, at);
        }
        let mask = voices.active_mask();
        self.quieten(mask, state.clock(at).is_multiple_of(CHUNK as u64));
        let top = self.active_lanes(mask);
        let stride = self.stride;
        if top > 0 {
            self.bank.set_active(top);
            self.input[..n * stride].fill(0.0);
            self.bank.process_with(
                &self.input[..n * stride],
                &mut self.heard[..n * stride * 2],
                &mut self.strings,
            );
        }
        let mut mix = [0.0f32; CHUNK];
        let s = self.settings.clone();
        // The string each lane gives, through the filter if it is on.
        for lane in 0..top {
            for f in 0..n {
                self.audio[0][f * LANES + lane] = self.heard[f * 2 * stride + lane];
            }
        }
        if s.filter && top > 0 {
            self.filter(voices, n, top);
        }
        let mut bits = mask;
        while bits != 0 {
            let v = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let fades = self.fade(v, n);
            for copy in 0..self.voices[v].copies.max(1) {
                let lane = copy * MAX_VOICES + v;
                let gain = self.lanes[lane].gain * OUTPUT * self.strings.lanes[lane].level();
                let mut peak = self.voices[v].peak;
                for f in 0..n {
                    let x = self.audio[0][f * LANES + lane];
                    peak = peak.max(x.abs());
                    mix[f] += x * gain * fades[f];
                }
                self.voices[v].peak = peak;
                self.lanes[lane].age += n as u64;
            }
        }
        let shared = &self.shared;
        let rate = self.rate;
        self.body.tune(
            s.body_type,
            s.body_size,
            (shared[C::BodyDecay.n()] / 1000.0) as f32,
            rate,
        );
        let share = ((shared[C::BodyStrBody.n()] / 100.0 + 1.0) / 2.0) as f32;
        self.cuts = (
            pole(shared[C::BodyLowCut.n()] as f32, rate),
            pole(shared[C::BodyHighCut.n()] as f32, rate),
        );
        for (f, mixed) in mix[..n].iter().enumerate() {
            let heard = if s.body {
                self.body.step(*mixed, share, self.cuts)
            } else {
                *mixed
            };
            let x = heard * s.volume;
            let y = x - self.dc[0] + self.dc_pole * self.dc[1];
            self.dc = [x, y];
            self.out[at + f] = y;
        }
    }

    /// On a grid point, starts the fade of each released voice that has been
    /// quiet since the last one.
    fn quieten(&mut self, mask: u32, on_grid: bool) {
        if !on_grid {
            return;
        }
        let mut bits = mask;
        while bits != 0 {
            let v = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let voice = &mut self.voices[v];
            if voice.released && voice.fade == 0 && voice.peak < QUIET {
                voice.fade = FADE;
            }
            voice.peak = 0.0;
        }
    }

    /// `voice`'s gain for each of the next `n` frames: 1, or falling to 0 when
    /// it is fading out, at which point it has ended.
    fn fade(&mut self, voice: usize, n: usize) -> [f32; CHUNK] {
        let state = &mut self.voices[voice];
        let mut gains = [1.0; CHUNK];
        if state.fade > 0 || state.ended {
            for gain in &mut gains[..n] {
                *gain = state.fade as f32 / FADE as f32;
                state.fade = state.fade.saturating_sub(1);
                state.ended = true;
            }
        }
        gains
    }

    /// The filter, over each lane's string for the chunk (R13).
    fn filter(&mut self, voices: &VoiceBlock, n: usize, top: usize) {
        let s = self.settings.clone();
        if self.kind_of != Some(s.mode) {
            self.kind_of = Some(s.mode);
            for filter in &mut self.filters {
                filter.set_type(kit_type(s.mode));
            }
        }
        let formant = s.mode >= FIRST_FORMANT;
        let frame = 1.0 / self.rate as f32;
        let mut bits = voices.active_mask();
        while bits != 0 {
            let v = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let p = self.played[v];
            let value = |c: C| p[c.n()] as f32;
            let amount = |c: C| (s.base[c.n()] / 100.0) as f32;
            let key_amount = ((self.lanes[v].note - 60.0) / 12.0).clamp(-4.0, 4.0);
            for copy in 0..self.voices[v].copies.max(1) {
                let lane = copy * MAX_VOICES + v;
                let mut resonance = 0.0;
                for f in 0..n {
                    let (env, lfo) = self.lane_signals(lane, f, p, &s, frame);
                    let freq = value(C::FilterFreq);
                    let note = self.lanes[lane].note;
                    let moved = (if s.lfo {
                        LFO_SEMITONES * amount(C::FreqLfo) * lfo
                    } else {
                        0.0
                    }) + (if s.envelope {
                        ENV_SEMITONES * amount(C::FreqEnv) * env
                    } else {
                        0.0
                    }) + amount(C::FreqKey) * (note - 60.0);
                    let reso = (value(C::FilterRes) / 100.0
                        + (if s.lfo { amount(C::ResLfo) * lfo } else { 0.0 })
                        + (if s.envelope {
                            amount(C::ResEnv) * env
                        } else {
                            0.0
                        })
                        + amount(C::ResKey) * key_amount / 4.0)
                        .clamp(0.0, 1.0);
                    if f == 0 {
                        resonance = reso;
                    }
                    let i = f * LANES + lane;
                    if formant {
                        let (f1, f2) = formants(reso, freq);
                        self.pitch[0][i] = pitch_of(f1, 440.0) + moved;
                        self.pitch[1][i] = pitch_of(f2, 440.0) + moved;
                    } else {
                        self.pitch[0][i] = pitch_of(freq, 440.0) + moved;
                    }
                }
                let target = if formant {
                    FORMANT_RESONANCE
                } else {
                    resonance
                };
                if (target - self.resonance_of[lane]).abs() > f32::EPSILON
                    || self.resonance_of[lane].is_nan()
                {
                    self.resonance_of[lane] = target;
                    let params = VoiceParams {
                        resonance: target,
                        ..VoiceParams::default()
                    };
                    self.filters[0].set_params(lane, &params);
                    self.filters[1].set_params(lane, &params);
                }
            }
        }
        if formant {
            let len = n * LANES;
            let (first, second) = self.audio.split_at_mut(1);
            second[0][..len].copy_from_slice(&first[0][..len]);
        }
        let len = n * LANES;
        self.filters[0].process_voices(top, &self.pitch[0][..len], &mut self.audio[0][..len]);
        if formant {
            self.filters[1].process_voices(top, &self.pitch[1][..len], &mut self.audio[1][..len]);
            for i in 0..len {
                self.audio[0][i] += self.audio[1][i];
            }
        }
    }

    /// The filter's envelope and LFO for `lane` at frame `f` of the chunk, each
    /// stepped by one frame of `step` seconds (R13).
    fn lane_signals(
        &mut self,
        lane: usize,
        f: usize,
        p: [f64; CONTROLS],
        s: &Settings,
        step: f32,
    ) -> (f32, f32) {
        let l = &mut self.lanes[lane];
        let seconds = |c: C| (p[c.n()] / 1000.0) as f32;
        let mut env = 0.0;
        if s.envelope {
            let attack = seconds(C::EnvAttack)
                * ATTACK_VELOCITY.powf(-(s.vel[C::EnvAttack.n()] as f32) * l.velocity);
            let sustain = (p[C::EnvSustain.n()] / 100.0) as f32;
            let decay = seconds(C::EnvDecay).max(1e-4);
            let release = seconds(C::EnvRelease).max(1e-4);
            match l.stage {
                Stage::Attack => {
                    l.level += step / attack.max(1e-4);
                    if l.level >= 1.0 {
                        l.level = 1.0;
                        l.stage = Stage::Decay;
                    }
                }
                Stage::Decay => {
                    l.level -= step * (1.0 - sustain) / decay;
                    if l.level <= sustain {
                        l.level = sustain;
                        l.stage = Stage::Sustain;
                    }
                }
                Stage::Sustain => l.level = sustain,
                Stage::Release => l.level = (l.level - step / release).max(0.0),
            }
            env = l.level;
        }
        let mut lfo = 0.0;
        if s.lfo {
            let old = (l.age + f as u64) as f32 * step;
            let (delay, attack) = (seconds(C::LfoDelay), seconds(C::LfoAttack));
            let hz = if s.synced {
                (self.bpm / 60.0 / DIVISION_BEATS[s.division]) as f32
            } else {
                p[C::LfoRate.n()] as f32
            };
            l.phase += hz * step;
            if l.phase >= 1.0 {
                l.phase -= 1.0;
                l.held = l.next;
                l.next = l.rng.next_bipolar() as f32;
            }
            let ph = l.phase;
            let wave = match s.wave {
                0 => (TAU * ph).sin(),
                1 => 1.0 - 4.0 * (ph - 0.5).abs(),
                2 => {
                    if ph < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
                3 => l.held,
                _ => l.held + (l.next - l.held) * ph,
            };
            let fade = if old < delay {
                0.0
            } else if attack > 0.0 {
                ((old - delay) / attack).min(1.0)
            } else {
                1.0
            };
            lfo = wave * fade;
        }
        (env, lfo)
    }

    /// Ends the voices that have faded out.
    fn end_quiet(&mut self, voices: &mut VoiceBlock) {
        let mut bits = voices.active_mask();
        while bits != 0 {
            let v = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let state = self.voices[v];
            if state.ended && state.fade == 0 {
                voices.end(v);
            }
        }
    }
}

impl Instrument for Engine {
    fn voice_event(&mut self, voices: &mut VoiceBlock, state: &State, event: VoiceEvent) {
        let v = event.voice;
        match event.kind {
            VoiceEventKind::Start { start, .. } => {
                self.start(voices, state, v, event.time as usize, start);
            }
            VoiceEventKind::Release => self.release(v),
            VoiceEventKind::End { .. } => {
                for copy in 0..COPIES {
                    self.strings.lanes[copy * MAX_VOICES + v].stop();
                }
                self.voices[v] = VoiceState::default();
            }
        }
    }

    fn render(&mut self, voices: &mut VoiceBlock, state: &State, frames: Range<usize>) {
        let mut at = frames.start;
        while at < frames.end {
            let phase = (state.clock(at) % CHUNK as u64) as usize;
            let n = (CHUNK - phase).min(frames.end - at);
            self.chunk(voices, state, &frames, at, n);
            at += n;
        }
        self.end_quiet(voices);
    }
}

/// Adds Tension to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, tension_type(), |device, config| {
        Tension::new(device, config)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frequency a body's impulse response is strongest at, in hertz.
    fn strongest(kind: usize, size: usize) -> f64 {
        let rate = 48_000.0;
        let mut body = Body::new();
        body.tune(kind, size, 1.0, rate);
        let cuts = (pole(1.0, rate), pole(20_000.0, rate));
        let mut response: Vec<f32> = (0..65_536)
            .map(|n| body.step(if n == 0 { 1.0 } else { 0.0 }, 1.0, cuts))
            .collect();
        let fft = realfft::RealFftPlanner::<f32>::new().plan_fft_forward(response.len());
        let mut spectrum = fft.make_output_vec();
        fft.process(&mut response, &mut spectrum).unwrap();
        let bin = |hz: f64| (hz * 65_536.0 / rate) as usize;
        let (low, high) = (bin(10.0), bin(4_000.0));
        let loudest = (low..high)
            .max_by(|a, b| spectrum[*a].norm().total_cmp(&spectrum[*b].norm()))
            .unwrap();
        loudest as f64 * rate / 65_536.0
    }

    // Spec: tension R12
    #[test]
    fn each_size_lowers_a_bodys_resonances_by_a_fourth() {
        for (kind, (first, ..)) in BODIES.iter().enumerate() {
            for size in 0..5 {
                let want = first * (((2.0 - size as f64) * BODY_STEP_SEMITONES) / 12.0).exp2();
                if want < 25.0 {
                    continue;
                }
                let hz = strongest(kind, size);
                assert!(
                    (hz / want - 1.0).abs() < 0.04,
                    "type {kind} size {size}: {hz} against {want}"
                );
            }
        }
    }
}

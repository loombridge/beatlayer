//! Meld's matrix and modulators (WP-8.20; spec: meld R59 to R65, R71): the
//! sources and destinations Meld declares to the instruments' matrix with
//! its sixty-four free routes, and each voice's envelopes, LFOs and LFO 1 FX.
//!
//! Each engine has five modulators a voice (Amp Env, Mod Env, LFO 1, LFO 1
//! FX and LFO 2). The envelopes are worked out every frame; the LFOs and their
//! effects every step of the control grid (32 frames counted from the node's
//! creation) and read between steps along a straight line from one to the
//! next, which keeps them the same at any block size.
//!
//! The two engines play the same notes, and a voice ends only when both
//! amplitude envelopes have ended, so each engine's sound holds both engines'
//! modulators and works both out alike; it plays its own.

use std::sync::{Arc, OnceLock};

use bl_model::matrix::{
    DestDef, Kind as MatrixKind, MatrixLayout, Pick, Range as MatrixRange, Rate, Scope, SlotDef,
    SourceDef,
};
use bl_model::{ParamDescriptor, ParamId, ParamUnit};
use lspk_voice::{CONTROL_INTERVAL, Start, VoiceBlock};

use crate::framework::{Audio, ParamBank};
use crate::instrument::matrix::{Matrix, Read};
use crate::instrument::{Sources, State};
use crate::meld::{self, MAX_VOICES as V, ROUTES, SLOTS, param, route_amount};
use crate::meld_mod::{self, Fx};
use crate::wavetable_mod::{DIVISIONS, EnvParams, Envelope, Lfo, Loop, segment_step, shape_power};

/// The sources, by place in the layout: five a engine (Amp Env, Mod Env,
/// LFO 1, LFO 1 FX, LFO 2) for A then B, then the shared ones.
pub const PER_ENGINE_SOURCES: usize = 5;
pub const MODULATORS: usize = 2 * PER_ENGINE_SOURCES;
pub const S_VELOCITY: usize = MODULATORS;
pub const S_PITCH: usize = S_VELOCITY + 1;
pub const S_RANDOM: usize = S_VELOCITY + 2;
pub const S_PITCH_BEND: usize = S_VELOCITY + 3;
pub const S_PRESS: usize = S_VELOCITY + 4;
pub const S_MOD_WHEEL: usize = S_VELOCITY + 5;
pub const S_MPE_BEND: usize = S_VELOCITY + 6;
pub const S_MPE_SLIDE: usize = S_VELOCITY + 7;
pub const S_MPE_PRESS: usize = S_VELOCITY + 8;
pub const S_SPREAD: usize = S_VELOCITY + 9;
pub const SOURCES: usize = S_SPREAD + 1;
/// How many shared sources there are.
const SHARED: usize = SOURCES - MODULATORS;

/// An engine's destinations, by place among its [`PER_ENGINE`], in the order
/// the matrix lists them.
pub const L_SEMITONES: usize = 0;
pub const L_CENTS: usize = 1;
pub const L_QUANT: usize = 2;
pub const L_MACRO_1: usize = 3;
pub const L_MACRO_2: usize = 4;
pub const L_FREQ: usize = 5;
pub const L_FILTER_1: usize = 6;
pub const L_FILTER_2: usize = 7;
/// The amplitude envelope's seven: Attack, Decay, Sustain, Release and the
/// three slopes.
pub const L_AMP: usize = 8;
/// The modulation envelope's ten: the same seven, then Initial, Peak and
/// Final.
pub const L_ENV: usize = 15;
pub const L_LFO1_RATE: usize = 25;
pub const L_LFO1_1: usize = 26;
pub const L_LFO1_2: usize = 27;
pub const L_FX1: usize = 28;
pub const L_FX2: usize = 29;
pub const L_LFO2_RATE: usize = 30;
pub const L_VOLUME: usize = 31;
pub const L_PAN: usize = 32;
pub const L_TONE: usize = 33;
pub const PER_ENGINE: usize = 34;

/// The global destinations, after both engines'.
pub const D_DRIVE: usize = 2 * PER_ENGINE;
pub const D_OUTPUT: usize = D_DRIVE + 1;
pub const DESTS: usize = D_OUTPUT + 1;

/// Engine `engine`'s destination `local`.
#[must_use]
pub const fn dest(engine: usize, local: usize) -> usize {
    engine * PER_ENGINE + local
}

/// Pitch Quant's travel in steps: two octaves either way.
const QUANT_STEPS: f64 = 24.0;

/// The parameter offset each of an engine's destinations moves, in the
/// matrix's order; none for Pitch Quant.
const LOCAL_PARAMS: [Option<u32>; PER_ENGINE] = [
    Some(meld::SEMITONES),
    Some(meld::CENTS),
    None,
    Some(meld::MACRO_1),
    Some(meld::MACRO_2),
    Some(meld::FILTER_FREQ),
    Some(meld::FILTER_MACRO_1),
    Some(meld::FILTER_MACRO_2),
    Some(meld::ATTACK),
    Some(meld::DECAY),
    Some(meld::SUSTAIN),
    Some(meld::RELEASE),
    Some(meld::ATTACK_SLOPE),
    Some(meld::DECAY_SLOPE),
    Some(meld::RELEASE_SLOPE),
    Some(meld::MOD_ATTACK),
    Some(meld::MOD_DECAY),
    Some(meld::MOD_SUSTAIN),
    Some(meld::MOD_RELEASE),
    Some(meld::MOD_ATTACK_SLOPE),
    Some(meld::MOD_DECAY_SLOPE),
    Some(meld::MOD_RELEASE_SLOPE),
    Some(meld::MOD_INITIAL),
    Some(meld::MOD_PEAK),
    Some(meld::MOD_FINAL),
    Some(meld::LFO1_RATE),
    Some(meld::LFO1_MACRO_1),
    Some(meld::LFO1_MACRO_2),
    Some(meld::FX1_AMOUNT),
    Some(meld::FX2_AMOUNT),
    Some(meld::LFO2_RATE),
    Some(meld::VOLUME),
    Some(meld::PAN),
    Some(meld::TONE),
];

/// Pitch Quant's descriptor: no parameter of the device, a transposition
/// in steps, whole ones.
fn quant_descriptor() -> ParamDescriptor {
    ParamDescriptor::continuous(
        ParamId(u32::MAX),
        "Pitch Quant",
        (-QUANT_STEPS, QUANT_STEPS),
        0.0,
        ParamUnit::Semitones,
    )
}

/// The parameter each destination moves, in the matrix's order.
fn dest_ids() -> Vec<Option<ParamId>> {
    let mut ids = Vec::with_capacity(DESTS);
    for engine in 0..2 {
        ids.extend(
            LOCAL_PARAMS
                .iter()
                .map(|offset| offset.map(|offset| param(engine, offset))),
        );
    }
    ids.extend([Some(meld::DRIVE), Some(meld::OUTPUT)]);
    ids
}

/// Whether a destination multiplies its routes (R62): the engines' levels and
/// the output's Volume.
fn multiplicative(dest: usize) -> bool {
    dest == D_OUTPUT || dest % PER_ENGINE == L_VOLUME && dest < D_DRIVE
}

/// Whether a destination is worked out every frame (instrument modulation
/// R1): pitch, the filter's frequency, level and pan, and the output.
fn audio_rate(dest: usize) -> bool {
    dest >= D_DRIVE && dest == D_OUTPUT
        || dest < D_DRIVE
            && matches!(
                dest % PER_ENGINE,
                L_SEMITONES | L_CENTS | L_QUANT | L_FREQ | L_VOLUME | L_PAN
            )
}

/// The matrix's sources, in the order of the constants above.
fn source_defs() -> Vec<SourceDef> {
    let engine = |letter: &str| {
        [
            ("Amp Env", MatrixRange::Unipolar),
            ("Mod Env", MatrixRange::Bipolar),
            ("LFO 1", MatrixRange::Bipolar),
            ("LFO 1 FX", MatrixRange::Bipolar),
            ("LFO 2", MatrixRange::Bipolar),
        ]
        .map(|(name, range)| (format!("{letter} {name}"), range, Scope::Voice))
    };
    let shared = [
        ("Velocity", MatrixRange::Unipolar, Scope::Voice),
        ("Pitch", MatrixRange::Bipolar, Scope::Voice),
        ("Random", MatrixRange::Unipolar, Scope::Voice),
        ("Pitch Bend", MatrixRange::Bipolar, Scope::Global),
        ("Press", MatrixRange::Unipolar, Scope::Global),
        ("Mod Wheel", MatrixRange::Unipolar, Scope::Global),
        ("MPE Pitch Bend", MatrixRange::Bipolar, Scope::Voice),
        ("MPE Slide", MatrixRange::Unipolar, Scope::Voice),
        ("MPE Press", MatrixRange::Unipolar, Scope::Voice),
        ("Spread", MatrixRange::Bipolar, Scope::Voice),
    ]
    .map(|(name, range, scope)| (name.to_owned(), range, scope));
    engine("A")
        .into_iter()
        .chain(engine("B"))
        .chain(shared)
        .map(|(name, range, scope)| SourceDef {
            // Held for as long as the program runs, as the parameters' names are.
            name: Box::leak(name.into_boxed_str()),
            range,
            scope,
        })
        .collect()
}

/// Meld's matrix layout (R62), made once.
pub fn layout() -> &'static Arc<MatrixLayout> {
    static LAYOUT: OnceLock<Arc<MatrixLayout>> = OnceLock::new();
    LAYOUT.get_or_init(|| {
        let description = meld::meld_description();
        let dests: Vec<DestDef> = dest_ids()
            .into_iter()
            .enumerate()
            .map(|(dest, id)| {
                let name = match id {
                    Some(id) => description.param(id).expect("a parameter").name.clone(),
                    None => format!("{} Pitch Quant", ["A", "B"][dest / PER_ENGINE]),
                };
                DestDef {
                    name: Box::leak(name.into_boxed_str()),
                    param: id,
                    scope: if dest >= D_DRIVE {
                        Scope::Global
                    } else {
                        Scope::Voice
                    },
                    rate: if audio_rate(dest) {
                        Rate::Audio
                    } else {
                        Rate::Control
                    },
                    kind: if multiplicative(dest) {
                        MatrixKind::Multiplicative
                    } else {
                        MatrixKind::Additive
                    },
                }
            })
            .collect();
        // An engine's modulators reach its own destinations and the global
        // ones, never the other engine's (R62).
        let mut forbidden = Vec::new();
        for engine in 0..2 {
            let other = 1 - engine;
            for source in 0..PER_ENGINE_SOURCES {
                for local in 0..PER_ENGINE {
                    forbidden.push((
                        (engine * PER_ENGINE_SOURCES + source) as u8,
                        dest(other, local) as u8,
                    ));
                }
            }
        }
        Arc::new(MatrixLayout {
            sources: source_defs(),
            dests,
            slots: (0..ROUTES)
                .map(|slot| SlotDef {
                    source: Pick::Free,
                    dest: Pick::Free,
                    amount: route_amount(slot),
                })
                .collect(),
            forbidden,
        })
    })
}

/// Every destination's descriptor, in the matrix's order.
pub(crate) fn descriptors() -> &'static Arc<Vec<ParamDescriptor>> {
    static DESCRIPTORS: OnceLock<Arc<Vec<ParamDescriptor>>> = OnceLock::new();
    DESCRIPTORS.get_or_init(|| {
        let description = meld::meld_description();
        Arc::new(
            dest_ids()
                .into_iter()
                .map(|id| match id {
                    Some(id) => description.param(id).expect("a parameter").clone(),
                    None => quant_descriptor(),
                })
                .collect(),
        )
    })
}

/// How many frames the control grid steps by.
const GRID: u64 = CONTROL_INTERVAL as u64;

/// A modulator read between two steps of the grid: where it was, where it
/// is now, and the frame it got there.
#[derive(Clone, Copy)]
struct Seg {
    from: f32,
    to: f32,
    at: u64,
}

impl Seg {
    const FLAT: Self = Self {
        from: 0.0,
        to: 0.0,
        at: 0,
    };

    fn at(&self, clock: u64) -> f32 {
        let across = (clock.saturating_sub(self.at) as f32 / GRID as f32).min(1.0);
        self.from + (self.to - self.from) * across
    }

    fn step(&mut self, to: f32, clock: u64) {
        *self = Self {
            from: self.to,
            to,
            at: clock,
        };
    }

    fn set(&mut self, value: f32, clock: u64) {
        *self = Self {
            from: value,
            to: value,
            at: clock,
        };
    }
}

/// One LFO's settings for a stretch.
#[derive(Clone, Copy)]
struct LfoSet {
    sync: bool,
    hz: f64,
    beats: f64,
    /// Its phase offset, in cycles.
    offset: f64,
    retrigger: bool,
}

/// What an engine's parameters say for a stretch, as they play with no route.
#[derive(Clone, Copy)]
struct EngineSet {
    amp: EnvParams,
    env: EnvParams,
    link: bool,
    lfo: [LfoSet; 2],
    kind1: usize,
    kind2: usize,
    macros: [f32; 2],
    fx: [(usize, f32); 2],
}

/// One engine's modulators for every voice.
struct EngineMods {
    amp: [Envelope; V],
    env: [Envelope; V],
    lfo: [[Lfo; V]; 2],
    fx: Vec<[Fx; 2]>,
    /// LFO 1, its effects and LFO 2, between steps of the grid.
    seg: [[Seg; 3]; V],
    /// The frame of each voice's last step of the grid.
    last: [u64; V],
    /// What the five modulators were a frame ago, which a modulator's own
    /// parameters read.
    prev: [[f32; PER_ENGINE_SOURCES]; V],
    salt: [[u64; 2]; V],
}

impl EngineMods {
    fn new() -> Self {
        Self {
            amp: [Envelope::IDLE; V],
            env: [Envelope::IDLE; V],
            lfo: [[Lfo::IDLE; V]; 2],
            fx: vec![[Fx::IDLE; 2]; V],
            seg: [[Seg::FLAT; 3]; V],
            last: [0; V],
            prev: [[0.0; PER_ENGINE_SOURCES]; V],
            salt: [[0; 2]; V],
        }
    }
}

/// The matrix and both engines' modulators (see the module's note).
pub struct Mods {
    pub matrix: Matrix,
    /// The newest voice still sounding, whose modulators' destinations a
    /// control shows (R12; modulation R31).
    newest: Option<usize>,
    descs: Arc<Vec<ParamDescriptor>>,
    /// Whether a route reaches each destination this stretch.
    pub has: [bool; DESTS],
    /// Every destination's parameter value, as of the stretch.
    base: [f64; DESTS],
    set: [EngineSet; 2],
    eng: [EngineMods; 2],
    /// The ten modulators' values by engine, source, voice and frame of the
    /// stretch: `(source × V + voice) × cap + frame`.
    src: [Vec<f32>; 2],
    cap: usize,
    /// The amplitude envelopes' levels in the piece being rendered, by
    /// engine: `frame × V + voice`.
    pub level: [Vec<f32>; 2],
    /// The oldest voice still sounding at each frame of the stretch, or −1,
    /// and what the shared sources read for it then: what the global
    /// destinations read.
    oldest_at: Vec<i8>,
    shared_at: Vec<f32>,
    /// The song's place at each frame of the stretch on the control grid, in
    /// song frames and in quarter notes, and whether it is playing.
    song: Vec<(f64, f64)>,
    playing: bool,
    bpm: f64,
    rate: f64,
    serial: u64,
    seed: u64,
}

impl Mods {
    /// Modulators for a device with `device`'s routes, at `rate`, for blocks
    /// of at most `frames`. Allocates: made with the device.
    pub fn new(device: &bl_model::Device, rate: f64, frames: usize, seed: u64) -> Self {
        let cap = frames.max(1);
        let plain = EnvParams::AMP;
        let lfo = LfoSet {
            sync: false,
            hz: 1.0,
            beats: 1.0,
            offset: 0.0,
            retrigger: false,
        };
        let set = EngineSet {
            amp: plain,
            env: plain,
            link: false,
            lfo: [lfo; 2],
            kind1: 0,
            kind2: 0,
            macros: [0.0; 2],
            fx: [(0, 0.0); 2],
        };
        Self {
            matrix: Matrix::of_voices(layout().clone(), device, SLOTS + V),
            newest: None,
            descs: descriptors().clone(),
            has: [false; DESTS],
            base: [0.0; DESTS],
            set: [set; 2],
            eng: [EngineMods::new(), EngineMods::new()],
            src: [
                vec![0.0; PER_ENGINE_SOURCES * V * cap],
                vec![0.0; PER_ENGINE_SOURCES * V * cap],
            ],
            cap,
            level: [vec![0.0; cap * V], vec![0.0; cap * V]],
            oldest_at: vec![-1; cap],
            shared_at: vec![0.0; SHARED * cap],
            song: vec![(0.0, 0.0); cap],
            playing: false,
            bpm: 120.0,
            rate,
            serial: 0,
            seed,
        }
    }

    /// The seed of the note's draws.
    fn salt(&self, engine: usize, which: usize, retrigger: bool) -> u64 {
        let base = self.seed ^ ((engine * 2 + which) as u64 + 1) << 56;
        if retrigger { base ^ self.serial } else { base }
    }

    /// Reads the stretch's parameters, the song's place on the control grid
    /// and the matrix's slots. `clock` is the node's frame count at the
    /// stretch's start.
    pub fn begin(&mut self, audio: &Audio<'_, '_>, params: &ParamBank, clock: u64) {
        let value = |id: ParamId| params.index(id).map_or(0.0, |at| params.value(at));
        for (dest, (to, d)) in self.base.iter_mut().zip(self.descs.iter()).enumerate() {
            let has_param = dest >= D_DRIVE || LOCAL_PARAMS[dest % PER_ENGINE].is_some();
            *to = if has_param { value(d.id) } else { 0.0 };
        }
        for engine in 0..2 {
            let at = |offset: u32| value(param(engine, offset));
            let base = |local: usize| self.base[dest(engine, local)];
            let mut amp = self.env_params(false, &|i| base(L_AMP + i));
            amp.mode = Loop::of(at(meld::ENV_MODE) as usize);
            let mut env = self.env_params(true, &|i| base(L_ENV + i));
            env.mode = Loop::of(at(meld::MOD_MODE) as usize);
            let link = at(meld::LINK) >= 0.5;
            if link {
                (env.step, env.power) = (amp.step, amp.power);
            }
            let lfo = |sync, rate, division, offset, retrigger| LfoSet {
                sync: at(sync) >= 0.5,
                hz: at(rate),
                beats: DIVISIONS[(at(division) as usize).min(DIVISIONS.len() - 1)].1,
                offset: at(offset) / 360.0,
                retrigger: at(retrigger) >= 0.5,
            };
            self.set[engine] = EngineSet {
                amp,
                env,
                link,
                lfo: [
                    lfo(
                        meld::LFO1_SYNC,
                        meld::LFO1_RATE,
                        meld::LFO1_DIVISION,
                        meld::LFO1_OFFSET,
                        meld::LFO1_RETRIGGER,
                    ),
                    lfo(
                        meld::LFO2_SYNC,
                        meld::LFO2_RATE,
                        meld::LFO2_DIVISION,
                        meld::LFO2_OFFSET,
                        meld::LFO2_RETRIGGER,
                    ),
                ],
                kind1: (at(meld::LFO1_TYPE) as usize).min(meld_mod::LFO1_TYPES.len() - 1),
                kind2: (at(meld::LFO2_TYPE) as usize).min(meld_mod::LFO2_TYPES.len() - 1),
                macros: [base(L_LFO1_1), base(L_LFO1_2)].map(|m| (m / 100.0) as f32),
                fx: [
                    (
                        (at(meld::FX1_TYPE) as usize).min(meld_mod::FX_NAMES.len() - 1),
                        (base(L_FX1) / 100.0) as f32,
                    ),
                    (
                        (at(meld::FX2_TYPE) as usize).min(meld_mod::FX_NAMES.len() - 1),
                        (base(L_FX2) / 100.0) as f32,
                    ),
                ],
            };
        }
        self.matrix.begin(params);
        for (dest, has) in self.has.iter_mut().enumerate() {
            *has = self.matrix.has(dest);
        }
        self.playing = audio.playing();
        self.bpm = audio.bpm();
        if self.playing {
            let frames = audio.frames();
            let tempo = audio.tempo();
            let rate = audio.sample_rate();
            for k in (0..frames).filter(|&k| (clock + k as u64).is_multiple_of(GRID)) {
                let frame = audio.song_frame(k);
                let ticks = tempo.frames_to_ticks(bl_core::time::Frames(frame), rate);
                self.song[k] = (frame as f64, ticks.as_quarters());
            }
        }
    }

    /// The envelope parameters of engine `engine`'s amplitude envelope, or its
    /// modulation envelope, from `at` (the destinations' values in order:
    /// Attack, Decay, Sustain, Release, three slopes, and for the modulation
    /// envelope Initial, Peak and Final).
    fn env_params(&self, modulation: bool, at: &dyn Fn(usize) -> f64) -> EnvParams {
        let level = |i: usize, plain: f32| {
            if modulation {
                (at(i) / 100.0) as f32
            } else {
                plain
            }
        };
        EnvParams {
            step: [0, 1, 3].map(|i| segment_step(at(i), self.rate, 1.0)),
            power: [4, 5, 6].map(|i| shape_power((at(i) / 100.0) as f32)),
            sustain: (at(2) / 100.0) as f32,
            initial: level(7, 0.0),
            peak: level(8, 1.0),
            fin: level(9, 0.0),
            mode: Loop::None,
        }
    }

    /// Forgets every voice, as a switched-off device does.
    pub fn silence(&mut self) {
        self.eng = [EngineMods::new(), EngineMods::new()];
    }

    /// A voice starts a note at `clock`, as `start` says: its envelopes
    /// start or carry on, and its LFOs and effects begin again (R59, R60).
    pub fn start(&mut self, voice: usize, start: Start, clock: u64) {
        self.serial += 1;
        for engine in 0..2 {
            let set = self.set[engine];
            let salts = [0, 1].map(|which| self.salt(engine, which, set.lfo[which].retrigger));
            let eng = &mut self.eng[engine];
            match start {
                Start::Fresh | Start::Join { .. } => {
                    eng.amp[voice].start(set.amp.initial);
                    eng.env[voice].start(set.env.initial);
                }
                Start::Steal | Start::Retrigger => {
                    let amp = eng.amp[voice].level();
                    let env = eng.env[voice].level();
                    eng.amp[voice].start(amp);
                    eng.env[voice].start(env);
                }
                Start::Legato => continue,
            }
            eng.salt[voice] = salts;
            eng.lfo[0][voice].start(set.lfo[0].offset, salts[0]);
            eng.lfo[1][voice].start(set.lfo[1].offset, salts[1]);
            eng.fx[voice][0].start(salts[0]);
            eng.fx[voice][1].start(salts[0] ^ 1);
            let one = {
                let lfo = &eng.lfo[0][voice];
                meld_mod::lfo1(set.kind1, lfo.phase as f32, 0, salts[0], set.macros)
            };
            let two = meld_mod::lfo2(set.kind2, eng.lfo[1][voice].phase as f32, 0, salts[1]);
            eng.seg[voice][0].set(one, clock);
            eng.seg[voice][1].set(one, clock);
            eng.seg[voice][2].set(two, clock);
            eng.last[voice] = clock;
            eng.prev[voice] = [0.0; PER_ENGINE_SOURCES];
        }
        self.matrix.start(SLOTS + voice);
    }

    /// A voice's note ends: its envelopes release, unless Trigger runs on.
    pub fn release(&mut self, voice: usize) {
        for engine in 0..2 {
            let set = self.set[engine];
            self.eng[engine].amp[voice].release(set.amp.mode);
            self.eng[engine].env[voice].release(set.env.mode);
        }
    }

    /// A voice ends.
    pub fn end(&mut self, voice: usize) {
        for eng in &mut self.eng {
            eng.amp[voice] = Envelope::IDLE;
            eng.env[voice] = Envelope::IDLE;
            eng.prev[voice] = [0.0; PER_ENGINE_SOURCES];
        }
    }

    /// Whether both amplitude envelopes of `voice` have ended (R13).
    #[must_use]
    pub fn idle(&self, voice: usize) -> bool {
        self.eng.iter().all(|eng| eng.amp[voice].is_idle())
    }

    /// Engine `engine`'s modulator `which` for `voice` at `frame` of the
    /// stretch.
    #[must_use]
    pub fn modulator(&self, engine: usize, which: usize, voice: usize, frame: usize) -> f32 {
        // A render's one-frame look past its end reads its last frame.
        self.src[engine][(which * V + voice) * self.cap + frame.min(self.cap - 1)]
    }

    /// Destination `d` of a device whose parameters play `base` there: the
    /// parameter's value moved by every route into it, as `read` reads the
    /// sources for `index` (a voice, or a slot) at `frame` of the stretch;
    /// `base` itself when no route reaches it (R5).
    pub fn value(
        &self,
        d: usize,
        base: f64,
        index: usize,
        frame: usize,
        due: bool,
        read: &impl Read,
    ) -> f64 {
        if !self.has[d] {
            return base;
        }
        let desc = &self.descs[d];
        let position = self
            .matrix
            .position(d, desc.normalize(base), index, frame, due, read);
        // A modulator's destinations are held at `SLOTS + voice`, which the
        // matrix does not take for the newest voice's slot.
        if self.newest.is_some_and(|voice| index == SLOTS + voice) {
            self.matrix.show(d, position);
        }
        desc.denormalize(position)
    }

    /// The newest voice still sounding and its first slot, if any: what the
    /// controls show the matrix's movement of.
    pub fn set_newest(&mut self, voice: Option<usize>, slot: Option<usize>) {
        self.newest = voice;
        self.matrix.set_newest(slot);
    }

    /// Whether any destination of engine `engine` in `locals` has a route.
    #[must_use]
    pub fn any(&self, engine: usize, locals: std::ops::Range<usize>) -> bool {
        locals.into_iter().any(|l| self.has[dest(engine, l)])
    }

    /// Works out the `frames` of the stretch the piece covers: each active
    /// voice's modulators, the amplitude envelopes' levels, and for the
    /// global destinations the oldest voice and what it reads (R59 to R62).
    pub fn run(&mut self, voices: &VoiceBlock, state: &State, frames: &std::ops::Range<usize>) {
        let count = frames.len();
        for level in &mut self.level {
            level[..count * V].fill(0.0);
        }
        let mask = voices.active_mask();
        let global = self.has[D_DRIVE] || self.has[D_OUTPUT];
        let sources = Sources::of(voices);
        for k in 0..count {
            let frame = frames.start + k;
            let clock = state.clock(frame);
            let grid = clock.is_multiple_of(GRID);
            let mut alive = 0u32;
            for voice in 0..V {
                if mask >> voice & 1 == 1 && !self.idle(voice) {
                    alive |= 1 << voice;
                }
            }
            let oldest = state.oldest_of(alive);
            self.oldest_at[frame] = oldest.map_or(-1, |voice| voice as i8);
            for engine in 0..2 {
                let mut m = mask;
                while m != 0 {
                    let voice = m.trailing_zeros() as usize;
                    m &= m - 1;
                    self.step(engine, voice, (k, frame), (clock, grid), voices, state);
                    self.level[engine][k * V + voice] = self.eng[engine].amp[voice].level();
                }
            }
            if global && let Some(voice) = oldest {
                for (i, to) in (0..SHARED).zip(0..) {
                    self.shared_at[i * self.cap + frame] =
                        shared(&sources, state, MODULATORS + to, voice) as f32;
                }
            }
        }
    }

    /// Voice `voice` of engine `engine` for one frame: its LFOs and effects
    /// on a step of the grid, its envelopes every frame.
    fn step(
        &mut self,
        engine: usize,
        voice: usize,
        (_, frame): (usize, usize),
        (clock, grid): (u64, bool),
        voices: &VoiceBlock,
        state: &State,
    ) {
        let set = self.set[engine];
        let stepping = grid && clock > self.eng[engine].last[voice];
        // What the destinations that move the modulators play, read with
        // the modulators a frame back.
        let (macros, amounts, rates, amp, env) = {
            let reads = Before {
                mods: self,
                voices,
                state,
            };
            let val = |local: usize| {
                self.value(
                    dest(engine, local),
                    self.base[dest(engine, local)],
                    SLOTS + voice,
                    frame,
                    grid,
                    &reads,
                )
            };
            let moved = |locals: std::ops::Range<usize>| self.any(engine, locals);
            let macros = if stepping {
                [val(L_LFO1_1), val(L_LFO1_2)].map(|m| (m / 100.0) as f32)
            } else {
                [0.0; 2]
            };
            let amounts = if stepping {
                [val(L_FX1), val(L_FX2)].map(|m| (m / 100.0) as f32)
            } else {
                [0.0; 2]
            };
            let rates = if stepping {
                [val(L_LFO1_RATE), val(L_LFO2_RATE)]
            } else {
                [0.0; 2]
            };
            let mut amp = set.amp;
            if moved(L_AMP..L_AMP + 7) {
                amp = self.env_params(false, &|i| val(L_AMP + i));
                amp.mode = set.amp.mode;
            }
            let mut env = set.env;
            if moved(L_ENV..L_ENV + 10) {
                env = self.env_params(true, &|i| val(L_ENV + i));
                env.mode = set.env.mode;
            }
            if set.link {
                (env.step, env.power) = (amp.step, amp.power);
            }
            (macros, amounts, rates, amp, env)
        };
        let song = self.song[frame];
        let (playing, bpm, rate) = (self.playing, self.bpm, self.rate);
        let rate_moved = [L_LFO1_RATE, L_LFO2_RATE].map(|l| self.has[dest(engine, l)]);
        let rate_base = [L_LFO1_RATE, L_LFO2_RATE].map(|l| self.base[dest(engine, l)]);
        let fx_moved = [L_FX1, L_FX2].map(|l| self.has[dest(engine, l)]);
        let eng = &mut self.eng[engine];
        if stepping {
            let frames = (clock - eng.last[voice]) as f64;
            for which in 0..2 {
                let lfo_set = set.lfo[which];
                let lfo = &mut eng.lfo[which][voice];
                if playing && !lfo_set.retrigger && !rate_moved[which] {
                    let cycles = if lfo_set.sync {
                        song.1 / lfo_set.beats
                    } else {
                        song.0 / rate * lfo_set.hz
                    };
                    lfo.follow(cycles + lfo_set.offset);
                } else {
                    // A route into a rate slows it for a positive amount and
                    // speeds it for a negative one (R62), mirrored on the
                    // control's travel, which is logarithmic.
                    let base = rate_base[which];
                    let played = if rate_moved[which] {
                        let desc = &self.descs[dest(engine, [L_LFO1_RATE, L_LFO2_RATE][which])];
                        let mirrored = 2.0 * desc.normalize(base) - desc.normalize(rates[which]);
                        desc.denormalize(mirrored.clamp(0.0, 1.0))
                    } else {
                        base
                    };
                    let hz = if lfo_set.sync {
                        bpm / 60.0 / lfo_set.beats * played / base.max(1e-9)
                    } else {
                        played
                    };
                    lfo.run(hz * frames / rate);
                }
            }
            let (phase, cycles) = (eng.lfo[0][voice].phase as f32, eng.lfo[0][voice].cycles);
            let macros = if self.has[dest(engine, L_LFO1_1)] || self.has[dest(engine, L_LFO1_2)] {
                macros
            } else {
                set.macros
            };
            let one = meld_mod::lfo1(set.kind1, phase, cycles, eng.salt[voice][0], macros);
            let mut out = one;
            for (slot, (kind, base_amount)) in set.fx.into_iter().enumerate() {
                if kind != 0 {
                    let amount = if fx_moved[slot] {
                        amounts[slot]
                    } else {
                        base_amount
                    };
                    out = eng.fx[voice][slot].step(kind, amount, out);
                }
            }
            let (phase, cycles) = (eng.lfo[1][voice].phase as f32, eng.lfo[1][voice].cycles);
            let two = meld_mod::lfo2(set.kind2, phase, cycles, eng.salt[voice][1]);
            for (seg, value) in eng.seg[voice].iter_mut().zip([one, out, two]) {
                seg.step(value, clock);
            }
            eng.last[voice] = clock;
        }
        let values = [
            eng.amp[voice].next(&amp),
            eng.env[voice].next(&env),
            eng.seg[voice][0].at(clock),
            eng.seg[voice][1].at(clock),
            eng.seg[voice][2].at(clock),
        ];
        eng.prev[voice] = values;
        for (which, value) in values.into_iter().enumerate() {
            self.src[engine][(which * V + voice) * self.cap + frame] = value;
        }
    }
}

/// What a shared source reads for `voice`, by its place in the layout.
fn shared(sources: &Sources<'_>, state: &State, source: usize, voice: usize) -> f64 {
    match source {
        S_VELOCITY => sources.velocity(voice),
        S_PITCH => sources.key(voice),
        S_RANDOM => state.random(voice),
        S_PITCH_BEND => sources.pitch_bend(),
        S_PRESS => sources.aftertouch(),
        S_MOD_WHEEL => sources.mod_wheel(),
        S_MPE_BEND => state.note_pb(voice),
        S_MPE_SLIDE => sources.slide(voice),
        S_MPE_PRESS => sources.pressure(voice),
        _ => 0.0,
    }
}

/// What a modulator's own parameters read: the modulators a frame earlier.
struct Before<'a> {
    mods: &'a Mods,
    voices: &'a VoiceBlock,
    state: &'a State,
}

impl Read for Before<'_> {
    fn read(&self, source: usize, index: usize, _frame: usize) -> f64 {
        // A modulator's destinations are held past the slots' (SLOTS + voice).
        let voice = index - SLOTS;
        if source < MODULATORS {
            let engine = source / PER_ENGINE_SOURCES;
            f64::from(self.mods.eng[engine].prev[voice][source % PER_ENGINE_SOURCES])
        } else {
            shared(&Sources::of(self.voices), self.state, source, voice)
        }
    }
}

/// What a route into a slot's destination reads: the voice's modulators at
/// the frame, the shared sources as the piece has them, and the slot's own
/// Spread. `of` gives a slot's voice and Spread.
pub struct SlotRead<'a, F> {
    pub mods: &'a Mods,
    pub voices: &'a VoiceBlock,
    pub state: &'a State,
    pub of: F,
}

impl<F: Fn(usize) -> (usize, f32)> Read for SlotRead<'_, F> {
    fn read(&self, source: usize, slot: usize, frame: usize) -> f64 {
        let (voice, spread) = (self.of)(slot);
        if source < MODULATORS {
            let engine = source / PER_ENGINE_SOURCES;
            f64::from(
                self.mods
                    .modulator(engine, source % PER_ENGINE_SOURCES, voice, frame),
            )
        } else if source == S_SPREAD {
            f64::from(spread)
        } else {
            shared(&Sources::of(self.voices), self.state, source, voice)
        }
    }
}

/// What a route into a global destination reads: the oldest voice's
/// modulators and the shared sources as they were then.
pub struct GlobalRead<'a>(pub &'a Mods);

impl Read for GlobalRead<'_> {
    fn read(&self, source: usize, voice: usize, frame: usize) -> f64 {
        let mods = self.0;
        if source < MODULATORS {
            let engine = source / PER_ENGINE_SOURCES;
            f64::from(mods.modulator(engine, source % PER_ENGINE_SOURCES, voice, frame))
        } else if source == S_SPREAD {
            0.0
        } else {
            f64::from(mods.shared_at[(source - MODULATORS) * mods.cap + frame])
        }
    }
}

impl Mods {
    /// The oldest voice still sounding at `frame` of the stretch, for the
    /// global destinations.
    #[must_use]
    pub fn oldest(&self, frame: usize) -> Option<usize> {
        usize::try_from(self.oldest_at[frame]).ok()
    }
}

/// Which engine a source belongs to: A (0) or B (1) for an engine's own
/// modulators, none for the shared sources.
#[must_use]
pub fn source_engine(source: usize) -> Option<usize> {
    (source < MODULATORS).then_some(source / PER_ENGINE_SOURCES)
}

/// Which engine a destination belongs to, none for the global ones.
#[must_use]
pub fn dest_engine(dest: usize) -> Option<usize> {
    (dest < D_DRIVE).then_some(dest / PER_ENGINE)
}

/// Source `source` as engine `to` has it: A's modulators become B's and the
/// other way about, the shared sources stay.
#[must_use]
pub fn source_for(source: usize, to: usize) -> usize {
    source_engine(source).map_or(source, |_| {
        to * PER_ENGINE_SOURCES + source % PER_ENGINE_SOURCES
    })
}

/// Destination `dest` as engine `to` has it; the global ones stay.
#[must_use]
pub fn dest_for(dest: usize, to: usize) -> usize {
    dest_engine(dest).map_or(dest, |_| to * PER_ENGINE + dest % PER_ENGINE)
}

//! The instruments' voice section (WP-8.2; spec: instrument voices). Every
//! Phase 8 instrument embeds a [`Section`] built on the plug-in kit's
//! voices, declares which [`Parts`] it has, and implements [`Instrument`]
//! to render the voices the section manages.
//!
//! The section turns the notes, bends and controllers that reach the device
//! into the kit's events, plays them one at a time so a decision can see
//! the voices as they are at its frame, and keeps what the kit doesn't: the
//! tuning path, Glissando, the per-note sources, Transpose, Stretch and
//! Error, and the clock Hi-Quality's control-rate grid counts. The
//! descriptor helpers give each shared control its name, unit and taper.

use core::ops::Range;

pub mod matrix;

use bl_dsp::random::Random;
use bl_engine::{KeyPitches, NodeData, NoteExpression, NoteKind};
use bl_model::{ParamDescriptor, ParamId, ParamUnit, Scale};
use lspk_plugin::{Event, Expression, Note};
use lspk_voice::unison::pan_gains;
use lspk_voice::{
    CONTROL_INTERVAL, Layout, MAX_VOICES, SameNote, Start, StealPriority, VoiceBlock, VoiceEvent,
    VoiceEventKind, VoiceHandler, Voices,
};

use crate::framework::{Audio, ParamBank};

/// The most notes the section plans in one stretch of a block.
const NOTES: usize = 512;

/// The most unison copies a voice has: the kit's lanes per voice.
pub const MAX_COPIES: usize = 16;

/// The most pitches a Glissando steps through: one per key.
const STEPS: usize = 128;

/// Spec: instrument voices R15. The key source's centre and span.
const KEY_CENTRE: f64 = 60.0;
const KEY_SPAN: f64 = 48.0;

/// Spec: instrument voices R17. Stretch's cents per octave at 100 %, and
/// Error's reach in semitones at 100 %.
const STRETCH_SEMITONES: f64 = 0.12;
const ERROR_SEMITONES: f64 = 0.5;

/// Spec: instrument voices R14. A per-note bend's range where a device
/// declares none: MPE's.
const MPE_RANGE: f32 = 48.0;

/// Where a device's per-note bend range comes from (spec: instrument
/// voices R14).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NoteRange {
    /// The device has none: MPE's 48 semitones.
    #[default]
    Mpe,
    /// The device's PB Range, for the wheel and per-note bends alike
    /// (Drift).
    PbRange,
    /// A Note PB range parameter of its own.
    Own(ParamId),
}

/// Which parts a device has: the parameter of each, if it has one. A part a
/// device doesn't declare isn't there (R1).
#[derive(Clone, Copy, Debug, Default)]
pub struct Parts {
    /// Voices ([`voices`]).
    pub voices: Option<ParamId>,
    /// Whether Voices at 1 plays Mono, in a device that has no Voice Mode
    /// (R6).
    pub voices_one_is_mono: bool,
    /// Poly or Mono ([`voice_mode`]).
    pub voice_mode: Option<ParamId>,
    /// Priority ([`priority`]).
    pub priority: Option<ParamId>,
    /// Legato in Mono ([`legato`]).
    pub legato: Option<ParamId>,
    /// A device that always plays Mono legato (R7).
    pub always_legato: bool,
    /// Glide Time ([`glide_time`]).
    pub glide_time: Option<ParamId>,
    /// Glide Mode, Portamento or Glissando ([`glide_mode`]), and Prop
    /// ([`glide_prop`]).
    pub glide_mode: Option<ParamId>,
    pub glide_prop: Option<ParamId>,
    /// Glide's Legato option ([`glide_legato`]).
    pub glide_legato: Option<ParamId>,
    /// A device whose Glide always works as with Legato on (Bass).
    pub glide_only_held: bool,
    /// Whether Glide works in Poly, each note from the one before (R8).
    pub poly_glide: bool,
    /// The device's Use Current Scale switch (R10, scales R16), and a second
    /// switch that must be on beside it for a Glissando to step degrees
    /// (Meld's Oscillator Scale Awareness).
    pub use_scale: Option<ParamId>,
    pub scale_and: Option<ParamId>,
    /// Retrigger ([`retrigger`]).
    pub retrigger: Option<ParamId>,
    /// Unison's copies, Detune, Spread and Delay (R11 to R13).
    pub unison_voices: Option<ParamId>,
    pub unison_detune: Option<ParamId>,
    pub unison_spread: Option<ParamId>,
    pub unison_delay: Option<ParamId>,
    /// PB Range ([`pb_range`]); Note PB's switch ([`note_pb`]) and range
    /// (R14).
    pub pb_range: Option<ParamId>,
    pub note_pb: Option<ParamId>,
    pub note_range: NoteRange,
    /// Transpose, Stretch and Error (R17).
    pub transpose: Option<ParamId>,
    pub stretch: Option<ParamId>,
    pub error: Option<ParamId>,
    /// Hi-Quality ([`hi_quality`]).
    pub hi_quality: Option<ParamId>,
}

/// Voices: the most notes that sound at once.
#[must_use]
pub fn voices(id: ParamId, max: i64, default: i64) -> ParamDescriptor {
    ParamDescriptor::integer(id, "Voices", (1, max), default, ParamUnit::None)
        .decimals(0)
        .describe("The most notes that sound at once.")
}

/// Voice Mode: Poly or Mono.
#[must_use]
pub fn voice_mode(id: ParamId, default: bool) -> ParamDescriptor {
    ParamDescriptor::choice(id, "Voice Mode", &["Poly", "Mono"], usize::from(default))
        .describe("Poly plays every note on a voice of its own; Mono plays one note at a time.")
}

/// Priority: which notes a new note past Voices keeps (R3).
#[must_use]
pub fn priority(id: ParamId) -> ParamDescriptor {
    ParamDescriptor::choice(id, "Priority", &["High", "Low", "Last"], 2).describe(
        "Which notes are kept when a new note arrives past Voices: the highest, the lowest, or the latest.",
    )
}

/// Legato, in Mono.
#[must_use]
pub fn legato(id: ParamId, default: bool) -> ParamDescriptor {
    ParamDescriptor::toggle(id, "Legato", default).describe(
        "In Mono, a note played over a held one moves the voice without restarting its envelopes.",
    )
}

/// Glide Time, 0 to `max_ms`: a note's slide from the pitch before it.
#[must_use]
pub fn glide_time(id: ParamId, max_ms: f64, default_ms: f64) -> ParamDescriptor {
    ParamDescriptor::continuous(
        id,
        "Glide Time",
        (0.0, max_ms),
        default_ms,
        ParamUnit::Milliseconds,
    )
    .power(2)
    .describe("How long a note takes to glide from the pitch before it; 0 plays it at its pitch.")
}

/// Glide Mode: Portamento slides, Glissando steps.
#[must_use]
pub fn glide_mode(id: ParamId) -> ParamDescriptor {
    ParamDescriptor::choice(id, "Glide Mode", &["Portamento", "Glissando"], 0)
        .describe("Portamento slides smoothly; Glissando moves in steps.")
}

/// Glide Prop: Glide Time is an octave's, scaled by the interval; off, it's
/// the same for every interval (Const).
#[must_use]
pub fn glide_prop(id: ParamId) -> ParamDescriptor {
    ParamDescriptor::toggle(id, "Glide Prop", false)
        .describe("Scales the glide's time with the interval, Glide Time being an octave's; off keeps it the same for every interval.")
}

/// Glide's Legato option: glide only into a note played over a held one.
#[must_use]
pub fn glide_legato(id: ParamId, default: bool) -> ParamDescriptor {
    ParamDescriptor::toggle(id, "Glide Legato", default)
        .describe("Glides only into a note played while another key is held.")
}

/// Retrigger: a note on a key that sounds restarts its voice.
#[must_use]
pub fn retrigger(id: ParamId, default: bool) -> ParamDescriptor {
    ParamDescriptor::toggle(id, "Retrigger", default)
        .describe("Restarts a sounding note's voice when its key is played again, instead of taking another voice.")
}

/// Unison Voices: copies stacked on each voice, 1 to `max`.
#[must_use]
pub fn unison_voices(id: ParamId, max: i64) -> ParamDescriptor {
    ParamDescriptor::integer(id, "Unison Voices", (1, max), 1, ParamUnit::None)
        .decimals(0)
        .describe("How many detuned copies play each note.")
}

/// Unison Detune: the outermost copies' detune, in cents.
#[must_use]
pub fn unison_detune(id: ParamId, max_cents: f64, default_cents: f64) -> ParamDescriptor {
    ParamDescriptor::continuous(
        id,
        "Unison Detune",
        (0.0, max_cents),
        default_cents,
        ParamUnit::Cents,
    )
    .describe("How far the outermost copies sit from the note, in cents.")
}

/// Unison Spread: how far the copies pan.
#[must_use]
pub fn unison_spread(id: ParamId, default: f64) -> ParamDescriptor {
    ParamDescriptor::continuous(
        id,
        "Unison Spread",
        (0.0, 100.0),
        default,
        ParamUnit::Percent,
    )
    .describe("How far the copies pan apart, alternately left and right.")
}

/// Unison Delay: how long after the first the last copy starts.
#[must_use]
pub fn unison_delay(id: ParamId, max_ms: f64) -> ParamDescriptor {
    ParamDescriptor::continuous(
        id,
        "Unison Delay",
        (0.0, max_ms),
        0.0,
        ParamUnit::Milliseconds,
    )
    .describe("How long after the first copy the last one starts.")
}

/// PB Range: the pitch wheel's range, in semitones.
#[must_use]
pub fn pb_range(id: ParamId, max: i64, default: i64) -> ParamDescriptor {
    ParamDescriptor::integer(id, "PB Range", (1, max), default, ParamUnit::Semitones)
        .decimals(0)
        .describe("Sets the instrument's pitch bend range, in semitones.")
}

/// Note PB: follow per-note pitch bend.
#[must_use]
pub fn note_pb(id: ParamId, default: bool) -> ParamDescriptor {
    ParamDescriptor::toggle(id, "Note PB", default)
        .describe("Follows an MPE controller's per-note pitch bend; off ignores it.")
}

/// Note PB Range: a per-note bend's range, in semitones.
#[must_use]
pub fn note_pb_range(id: ParamId, max: i64, default: i64) -> ParamDescriptor {
    ParamDescriptor::integer(id, "Note PB Range", (1, max), default, ParamUnit::Semitones)
        .decimals(0)
        .describe("Sets the range of a per-note pitch bend, in semitones.")
}

/// Transpose, in semitones either way.
#[must_use]
pub fn transpose(id: ParamId, range: i64) -> ParamDescriptor {
    ParamDescriptor::integer(id, "Transpose", (-range, range), 0, ParamUnit::Semitones)
        .decimals(0)
        .describe("Transposes the instrument in semitones.")
}

/// Stretch: widens or narrows the octaves.
#[must_use]
pub fn stretch(id: ParamId) -> ParamDescriptor {
    ParamDescriptor::continuous(id, "Stretch", (-100.0, 100.0), 0.0, ParamUnit::Percent).describe(
        "Widens the octaves above middle C and narrows those below it; negative does the opposite.",
    )
}

/// Error: a random detune per note.
#[must_use]
pub fn error(id: ParamId) -> ParamDescriptor {
    ParamDescriptor::continuous(id, "Error", (0.0, 100.0), 0.0, ParamUnit::Percent)
        .describe("Detunes each note by a random amount, up to half a semitone at 100 %.")
}

/// Hi-Quality: a menu option that works the modulation out for every frame.
/// The device lists it in its type's menu options.
#[must_use]
pub fn hi_quality(id: ParamId) -> ParamDescriptor {
    ParamDescriptor::toggle(id, "Hi-Quality", false)
        .describe("Works the modulation out for every frame and runs the finest settings; off, every 32 frames and lighter.")
}

/// A curve amount from −100 to 100 %, for a velocity or key curve (R16).
#[must_use]
pub fn curve_amount(id: ParamId, name: &str) -> ParamDescriptor {
    ParamDescriptor::continuous(id, name, (-100.0, 100.0), 0.0, ParamUnit::Percent)
        .describe("Bows the curve upward when positive and downward when negative; 0 is straight.")
}

/// Spec: instrument voices R16. `x` (0 to 1) through the curve of amount
/// `c` (−1 to 1): straight at 0, bowing upward when positive, so softer
/// values read higher.
#[must_use]
pub fn curve(x: f64, c: f64) -> f64 {
    x.clamp(0.0, 1.0).powf(4f64.powf(-c.clamp(-1.0, 1.0)))
}

/// Copy `i` of `n`'s detune from the note, in cents, for an outermost
/// detune of `cents` (R11).
#[must_use]
pub fn copy_cents(i: usize, n: usize, cents: f32) -> f32 {
    if n < 2 {
        0.0
    } else {
        (2.0 * i as f32 / (n - 1) as f32 - 1.0) * cents
    }
}

/// Copy `i` of `n`'s gains into the left and right channels at `spread`
/// (0 to 1): the copies pan alternately left and right, copy 0 left, and
/// their sum is scaled by 1 / √n (R12, R13).
#[must_use]
pub fn copy_gains(i: usize, n: usize, spread: f32) -> (f32, f32) {
    let pan = if i.is_multiple_of(2) { -spread } else { spread };
    let (left, right) = pan_gains(pan);
    let level = (n.max(1) as f32).sqrt().recip();
    (left * level, right * level)
}

/// How many frames after the note's the last copy starts is `delay_ms`;
/// copy `i` of `n` starts a share `i / (n − 1)` of it after copy 0 (R12).
#[must_use]
pub fn copy_delay_frames(i: usize, n: usize, delay_ms: f32, rate: f64) -> u64 {
    if n < 2 {
        0
    } else {
        (f64::from(delay_ms) * 1e-3 * rate * i as f64 / (n - 1) as f64) as u64
    }
}

/// What the section reads from the parameters at the start of a stretch.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Voices, 1 to the device's most.
    pub voices: usize,
    pub mono: bool,
    pub legato: bool,
    pub steal: StealPriority,
    pub retrigger: bool,
    /// Glide Time in seconds.
    pub glide_secs: f32,
    pub glide_prop: bool,
    pub glissando: bool,
    /// Whether a note glides from any pitch, not only into a held key.
    pub glide_always: bool,
    pub poly_glide: bool,
    pub scale_aware: bool,
    /// Whether Glissando steps degrees: `scale_aware` and the second switch.
    pub glide_scale: bool,
    /// Unison copies, Detune in cents, Spread (0 to 1) and Delay in ms.
    pub copies: usize,
    pub detune: f32,
    pub spread: f32,
    pub delay_ms: f32,
    pub pb_range: f32,
    pub note_pb: bool,
    pub note_range: f32,
    pub transpose: f32,
    /// Stretch and Error, as fractions.
    pub stretch: f64,
    pub error: f64,
    pub hi_quality: bool,
}

impl Config {
    fn read(parts: &Parts, params: &ParamBank, capacity: usize, copies_max: usize) -> Self {
        let get = |id: Option<ParamId>, default: f64| {
            id.map_or(default, |id| {
                params.value(params.index(id).expect("the device describes its parts"))
            })
        };
        let voices = (get(parts.voices, capacity as f64) as usize).clamp(1, capacity);
        let mono = get(parts.voice_mode, 0.0) >= 0.5
            || (parts.voice_mode.is_none() && parts.voices_one_is_mono && voices == 1);
        let pb_range = get(parts.pb_range, f64::from(MPE_RANGE)) as f32;
        Self {
            voices,
            mono,
            legato: mono && (parts.always_legato || get(parts.legato, 0.0) >= 0.5),
            steal: match get(parts.priority, 2.0) as usize {
                0 => StealPriority::Highest,
                1 => StealPriority::Lowest,
                _ => StealPriority::Newest,
            },
            retrigger: get(parts.retrigger, 0.0) >= 0.5,
            glide_secs: (get(parts.glide_time, 0.0) * 1e-3) as f32,
            glide_prop: get(parts.glide_prop, 0.0) >= 0.5,
            glissando: get(parts.glide_mode, 0.0) >= 0.5,
            glide_always: !parts.glide_only_held && get(parts.glide_legato, 0.0) < 0.5,
            poly_glide: parts.poly_glide,
            scale_aware: get(parts.use_scale, 0.0) >= 0.5,
            glide_scale: get(parts.use_scale, 0.0) >= 0.5 && get(parts.scale_and, 1.0) >= 0.5,
            copies: (get(parts.unison_voices, 1.0) as usize).clamp(1, copies_max),
            detune: get(parts.unison_detune, 0.0) as f32,
            spread: (get(parts.unison_spread, 0.0) / 100.0) as f32,
            delay_ms: get(parts.unison_delay, 0.0) as f32,
            pb_range,
            note_pb: get(parts.note_pb, 1.0) >= 0.5,
            note_range: match parts.note_range {
                NoteRange::Mpe => MPE_RANGE,
                NoteRange::PbRange => pb_range,
                NoteRange::Own(id) => get(Some(id), f64::from(MPE_RANGE)) as f32,
            },
            transpose: get(parts.transpose, 0.0) as f32,
            stretch: get(parts.stretch, 0.0) / 100.0,
            error: get(parts.error, 0.0) / 100.0,
            hi_quality: get(parts.hi_quality, 0.0) >= 0.5,
        }
    }
}

/// A Glissando under way: from where, in how many steps, and for how long.
#[derive(Clone, Copy)]
struct Glissando {
    started: u64,
    frames: u64,
    steps: usize,
    to: f64,
}

impl Glissando {
    const NONE: Self = Self {
        started: 0,
        frames: 0,
        steps: 0,
        to: 0.0,
    };
}

/// What the section knows about each voice beyond the kit's block, and what
/// an instrument reads while it renders.
pub struct State {
    cfg: Config,
    /// The kit's frame count at the start of the stretch being played.
    stretch_start: u64,
    tuned_by: [f32; MAX_VOICES],
    random: [f32; MAX_VOICES],
    error: [f32; MAX_VOICES],
    note_pb: [f32; MAX_VOICES],
    channel_bend: [f32; 16],
    started: [u64; MAX_VOICES],
    glissando: [Glissando; MAX_VOICES],
    /// Each voice's Glissando steps, `STEPS` apiece.
    steps: Vec<f32>,
    /// The voices that have started and not ended, and those held or
    /// sustained.
    live: u32,
    gated: u32,
    /// The voice that started last while it still sounds, and the pitch of
    /// the last key played.
    last_voice: Option<usize>,
    last_key: Option<f64>,
    scale: Scale,
    /// Whether the set's tuning is in force.
    tuned: bool,
    rng: Random,
}

impl State {
    fn new(seed: u64) -> Self {
        Self {
            cfg: Config {
                voices: 1,
                mono: false,
                legato: false,
                steal: StealPriority::Newest,
                retrigger: false,
                glide_secs: 0.0,
                glide_prop: false,
                glissando: false,
                glide_always: true,
                poly_glide: false,
                scale_aware: false,
                glide_scale: false,
                copies: 1,
                detune: 0.0,
                spread: 0.0,
                delay_ms: 0.0,
                pb_range: MPE_RANGE,
                note_pb: true,
                note_range: MPE_RANGE,
                transpose: 0.0,
                stretch: 0.0,
                error: 0.0,
                hi_quality: false,
            },
            stretch_start: 0,
            tuned_by: [0.0; MAX_VOICES],
            random: [0.0; MAX_VOICES],
            error: [0.0; MAX_VOICES],
            note_pb: [0.0; MAX_VOICES],
            channel_bend: [0.0; 16],
            started: [0; MAX_VOICES],
            glissando: [Glissando::NONE; MAX_VOICES],
            steps: vec![0.0; STEPS * MAX_VOICES],
            tuned: false,
            live: 0,
            gated: 0,
            last_voice: None,
            last_key: None,
            scale: Scale::default(),
            rng: Random::seeded(seed),
        }
    }

    /// Forgets every voice, without allocating.
    fn reset(&mut self) {
        self.tuned_by = [0.0; MAX_VOICES];
        self.note_pb = [0.0; MAX_VOICES];
        self.channel_bend = [0.0; 16];
        self.glissando = [Glissando::NONE; MAX_VOICES];
        self.live = 0;
        self.gated = 0;
        self.last_voice = None;
        self.last_key = None;
    }

    /// What the parameters say, as of this stretch.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// The scale the device counts degrees of at the frame being played: its
    /// current scale while its scale switches are on, else the chromatic
    /// one (scales R15, R16).
    #[must_use]
    pub fn scale(&self) -> &Scale {
        &self.scale
    }

    /// How far above its key's twelve-step pitch the voice started, in
    /// semitones: its key's tuned pitch under a tuning (R18).
    #[must_use]
    pub fn tuned_by(&self, voice: usize) -> f32 {
        self.tuned_by[voice]
    }

    /// Whether any sounding voice plays a tuned pitch.
    #[must_use]
    pub fn any_tuned(&self) -> bool {
        self.tuned_by.iter().any(|&by| by != 0.0)
    }

    /// How many frames have passed since `voice`'s note started, `frame`
    /// frames into the stretch: what a unison Delay counts (R12).
    #[must_use]
    pub fn frames_since_start(&self, voice: usize, frame: usize) -> u64 {
        self.clock(frame).saturating_sub(self.started[voice])
    }

    /// The oldest voice still sounding: the one a global destination of the
    /// modulation matrix reads per-voice sources from (spec: instrument
    /// modulation R6).
    #[must_use]
    pub fn oldest(&self, voices: &VoiceBlock) -> Option<usize> {
        voices
            .active_voices()
            .min_by_key(|&voice| self.started[voice])
    }

    /// The newest voice still sounding: the one a control shows the matrix's
    /// movement of (spec: modulation R31).
    #[must_use]
    pub fn newest(&self, voices: &VoiceBlock) -> Option<usize> {
        voices
            .active_voices()
            .max_by_key(|&voice| self.started[voice])
    }

    /// The voice's Random source: from 0 to 1, drawn when its note started
    /// (R15).
    #[must_use]
    pub fn random(&self, voice: usize) -> f64 {
        f64::from(self.random[voice])
    }

    /// The voice's Note PB source: its note's per-note bend, from −1 to 1
    /// (R15).
    #[must_use]
    pub fn note_pb(&self, voice: usize) -> f64 {
        f64::from(self.note_pb[voice])
    }

    /// The kit's frame count at `frame` of the stretch being rendered: a
    /// count from the node's creation, the same at any block size (R20,
    /// R21).
    #[must_use]
    pub fn clock(&self, frame: usize) -> u64 {
        self.stretch_start + frame as u64
    }

    /// Whether modulation is worked out at `frame` of the stretch: every
    /// frame with Hi-Quality on, else on the grid of 32 frames counted from
    /// the node's creation (R20).
    #[must_use]
    pub fn control_due(&self, frame: usize) -> bool {
        self.cfg.hi_quality
            || self
                .clock(frame)
                .is_multiple_of(u64::from(CONTROL_INTERVAL))
    }

    /// Spec: instrument voices R17, R18. `voice`'s pitch `at` frames into
    /// the render call of `frames`, in semitones as a twelve-step key counts
    /// them: its key with glide and bends, its tuned pitch, Glissando's
    /// steps, Transpose, Stretch (not under a tuning) and Error.
    #[must_use]
    pub fn pitch(
        &self,
        voices: &VoiceBlock,
        voice: usize,
        frames: &Range<usize>,
        at: usize,
    ) -> f64 {
        let key = f64::from(voices.info(voice).key);
        let lane = voices.lanes_of(voice).start;
        let mut pitch = f64::from(voices.pitch()[lane])
            + f64::from(voices.pitch_step()[lane]) * at as f64
            + f64::from(self.tuned_by[voice])
            + f64::from(self.cfg.transpose)
            + f64::from(self.error[voice]);
        if !self.tuned {
            pitch += self.cfg.stretch * (key - KEY_CENTRE) / 12.0 * STRETCH_SEMITONES;
        }
        let gliss = &self.glissando[voice];
        if gliss.frames > 0 {
            pitch += self.glissando_at(voice, self.clock(frames.start + at)) - gliss.to;
        }
        pitch
    }

    /// Where a Glissando's voice is at `now`, in tuned semitones.
    fn glissando_at(&self, voice: usize, now: u64) -> f64 {
        let gliss = &self.glissando[voice];
        let elapsed = now.saturating_sub(gliss.started);
        if gliss.frames == 0 || elapsed >= gliss.frames {
            return gliss.to;
        }
        let step = (elapsed as u128 * gliss.steps as u128 / u128::from(gliss.frames)) as usize;
        f64::from(self.steps[voice * STEPS + step.min(gliss.steps - 1)])
    }
}

/// Spec: instrument voices R15. The per-voice and global sources the
/// section gives the modulation matrix, read from the voices' lanes. Random
/// and Note PB are [`State`]'s.
#[derive(Clone, Copy)]
pub struct Sources<'a>(&'a VoiceBlock);

impl<'a> Sources<'a> {
    /// The sources of `voices`.
    #[must_use]
    pub fn of(voices: &'a VoiceBlock) -> Self {
        Self(voices)
    }

    /// The first of `voice`'s lanes: the lane arrays hold a voice's copies
    /// side by side, and every copy has the voice's values.
    fn lane(&self, voice: usize) -> usize {
        self.0.lanes_of(voice).start
    }

    /// The note's velocity, 0 to 1.
    #[must_use]
    pub fn velocity(&self, voice: usize) -> f64 {
        f64::from(self.0.velocity()[self.lane(voice)])
    }

    /// The note's release velocity, 0 to 1; 0 until the note ends.
    #[must_use]
    pub fn release_velocity(&self, voice: usize) -> f64 {
        f64::from(self.0.release_velocity()[self.lane(voice)])
    }

    /// The key, −1 to 1: C3 is 0 and each unit is four octaves.
    #[must_use]
    pub fn key(&self, voice: usize) -> f64 {
        ((f64::from(self.0.info(voice).key) - KEY_CENTRE) / KEY_SPAN).clamp(-1.0, 1.0)
    }

    /// The note's pressure, 0 to 1: its per-note pressure, or the channel's
    /// where none reached it.
    // shortcut: a per-note pressure of exactly 0 reads as none, upgrade when the kit reports
    // whether a note has had one.
    #[must_use]
    pub fn pressure(&self, voice: usize) -> f64 {
        let note = self.0.pressure()[self.lane(voice)];
        f64::from(if note > 0.0 {
            note
        } else {
            self.0.controllers().channel_pressure
        })
    }

    /// The note's slide, 0 to 1.
    #[must_use]
    pub fn slide(&self, voice: usize) -> f64 {
        f64::from(self.0.timbre()[self.lane(voice)])
    }

    /// The mod wheel, 0 to 1.
    #[must_use]
    pub fn mod_wheel(&self) -> f64 {
        f64::from(self.0.controllers().cc(1))
    }

    /// The pitch wheel, −1 to 1.
    #[must_use]
    pub fn pitch_bend(&self) -> f64 {
        f64::from(self.0.controllers().pitch_bend)
    }

    /// The latest channel pressure on a non-member channel, 0 to 1.
    #[must_use]
    pub fn aftertouch(&self) -> f64 {
        f64::from(self.0.controllers().channel_pressure)
    }
}

/// An instrument's sound engine, as the section drives it.
pub trait Instrument {
    /// A voice starts, is released or ends, at `event.time` of the stretch.
    fn voice_event(&mut self, voices: &mut VoiceBlock, state: &State, event: VoiceEvent);

    /// Renders `frames` of the stretch for the active voices.
    fn render(&mut self, voices: &mut VoiceBlock, state: &State, frames: Range<usize>);
}

/// The kit's handler, around an [`Instrument`]: it keeps [`State`] and puts
/// the stretch's frame numbers back, since the section plays a stretch in
/// pieces.
struct Adapter<'a, I> {
    instrument: &'a mut I,
    state: &'a mut State,
    keys: &'a KeyPitches,
    rate: f64,
    offset: usize,
}

impl<I: Instrument> VoiceHandler for Adapter<'_, I> {
    fn voice_event(&mut self, voices: &mut VoiceBlock, mut event: VoiceEvent) {
        event.time += self.offset as u32;
        let now = self.state.clock(event.time as usize);
        let bit = 1u32 << event.voice;
        match event.kind {
            VoiceEventKind::Start { start, .. } => self.start(voices, event.voice, start, now),
            VoiceEventKind::Release => self.state.gated &= !bit,
            VoiceEventKind::End { .. } => {
                let state = &mut *self.state;
                state.gated &= !bit;
                state.live &= !bit;
                if state.last_voice == Some(event.voice) {
                    state.last_voice = None;
                }
                state.tuned_by[event.voice] = 0.0;
                state.note_pb[event.voice] = 0.0;
                state.glissando[event.voice] = Glissando::NONE;
            }
        }
        self.instrument.voice_event(voices, self.state, event);
    }

    fn render(&mut self, voices: &mut VoiceBlock, frames: Range<usize>) {
        let frames = frames.start + self.offset..frames.end + self.offset;
        self.instrument.render(voices, self.state, frames);
    }
}

impl<I> Adapter<'_, I> {
    /// A voice starts a note at `now`.
    fn start(&mut self, voices: &VoiceBlock, voice: usize, start: Start, now: u64) {
        let (state, keys) = (&mut *self.state, self.keys);
        let key = voices.info(voice).key;
        let channel = voices
            .info(voice)
            .note
            .channel
            .map_or(0, |c| usize::from(c & 15));
        let tuned_by = keys.offset(key).unwrap_or(0.0);
        let to = f64::from(key) + tuned_by;
        let bit = 1u32 << voice;
        // Spec: instrument voices R8 to R10. Where a Glissando comes from:
        // the sounding voice's pitch in Mono, else the last note's.
        let from = match start {
            Start::Retrigger | Start::Legato => Some(state.glissando_from(voice, now)),
            _ => state
                .last_voice
                .map(|last| state.glissando_from(last, now))
                .or(state.last_key),
        };
        let held = state.gated != 0;
        let cfg = state.cfg;
        state.glissando[voice] = Glissando::NONE;
        if let Some(from) = from
            && cfg.glissando
            && cfg.glide_secs > 0.0
            && (cfg.mono || cfg.poly_glide)
            && state.live != 0
            && (cfg.glide_always || held)
            && from != to
        {
            let steps = state.fill_steps(voice, keys, from, to);
            state.glissando[voice] = Glissando {
                started: now,
                frames: (f64::from(cfg.glide_secs) * self.rate) as u64,
                steps,
                to,
            };
        }
        state.tuned_by[voice] = tuned_by as f32;
        state.random[voice] = ((state.rng.next_bipolar() + 1.0) / 2.0) as f32;
        state.error[voice] = (state.rng.next_bipolar() * state.cfg.error * ERROR_SEMITONES) as f32;
        state.note_pb[voice] = if cfg.note_pb && channel != 0 {
            state.channel_bend[channel]
        } else {
            0.0
        };
        state.started[voice] = now;
        state.gated |= bit;
        state.live |= bit;
        state.last_voice = Some(voice);
        state.last_key = Some(to);
    }
}

impl State {
    /// Where `voice` is now, as a Glissando's start: its Glissando's pitch,
    /// or its note's where it isn't gliding.
    fn glissando_from(&self, voice: usize, now: u64) -> f64 {
        if self.glissando[voice].frames > 0 {
            self.glissando_at(voice, now)
        } else {
            self.last_key.unwrap_or(0.0)
        }
    }

    /// Spec: instrument voices R10, R18. Fills `voice`'s steps from `from`
    /// to just short of `to`: `from`, then every key between the two, the
    /// scale's degrees when the device's scale awareness is on, each at its
    /// tuned pitch. Returns how many.
    fn fill_steps(&mut self, voice: usize, keys: &KeyPitches, from: f64, to: f64) -> usize {
        let (low, high) = (from.min(to), from.max(to));
        let scale = self.scale;
        let wanted = |key: u8| !self.cfg.glide_scale || scale.holds(key);
        let steps = &mut self.steps[voice * STEPS..(voice + 1) * STEPS];
        steps[0] = from as f32;
        let mut count = 1;
        let mut push = |key: u8| {
            if let Some(pitch) = keys.pitch(key)
                && pitch > low
                && pitch < high
                && wanted(key)
            {
                steps[count] = pitch as f32;
                count += 1;
            }
        };
        if to > from {
            (0..=127).for_each(&mut push);
        } else {
            (0..=127).rev().for_each(&mut push);
        }
        count
    }
}

/// A voice section: the kit's voices, the parts a device declares, and the
/// section's own state.
pub struct Section {
    parts: Parts,
    voices: Voices,
    capacity: usize,
    copies: usize,
    keys: KeyPitches,
    state: State,
    planned: Vec<Planned>,
    rate: f64,
}

/// A note the stretch plays: its frame, the kit's event, and the per-note
/// bend it carries (for Note PB).
#[derive(Clone, Copy)]
struct Planned {
    time: u32,
    event: Event,
    bend: Option<f32>,
}

impl Section {
    /// A section for at most `voices` voices of at most `copies` unison
    /// copies each, at `rate`, with `seed` for its random draws. Everything
    /// it needs is made here (R2).
    ///
    /// # Panics
    ///
    /// If `voices` isn't 1 to the kit's most, or `copies` isn't 1 to
    /// [`MAX_COPIES`].
    #[must_use]
    pub fn new(parts: Parts, voices: usize, copies: usize, rate: f64, seed: u64) -> Self {
        Self {
            parts,
            voices: Voices::new(
                Layout {
                    voices,
                    lanes_per_voice: copies,
                },
                rate as f32,
            ),
            capacity: voices,
            copies,
            keys: KeyPitches::twelve(),
            state: State::new(seed),
            planned: Vec::with_capacity(NOTES),
            rate,
        }
    }

    /// The lanes the section's voices take: a multiple of 16 (R2).
    #[must_use]
    pub fn lane_count(&self) -> usize {
        self.voices.block().lane_count()
    }

    /// The values the parameters gave this stretch, and the section's
    /// per-voice state.
    #[must_use]
    pub fn state(&self) -> &State {
        &self.state
    }

    /// The pitch of every key under the set's tuning, if `data` is it
    /// (tunings R12, R18). Notes already sounding keep the pitch they
    /// started at.
    pub fn receive_data(&mut self, data: &NodeData) {
        if let Some(keys) = data.downcast_ref::<KeyPitches>() {
            self.keys = *keys;
            self.state.tuned = keys.is_tuned();
        }
    }

    /// Frees every voice, as a switched-off device does.
    pub fn reset(&mut self) {
        self.voices.reset();
        self.state.reset();
    }

    /// Plays a stretch: the notes of `audio`, through `instrument`, with the
    /// parts `params` set.
    pub fn process<I: Instrument>(
        &mut self,
        audio: &Audio<'_, '_>,
        params: &ParamBank,
        instrument: &mut I,
    ) {
        let frames = audio.frames();
        self.state.cfg = Config::read(&self.parts, params, self.capacity, self.copies);
        self.state.stretch_start = self.voices.clock();
        self.apply_settings();
        self.plan(audio);
        self.state.scale = self.scale_at(audio, 0);
        let mut changes = audio.scale_changes().peekable();
        let mut done = 0;
        for i in 0..self.planned.len() {
            let Planned { time, event, bend } = self.planned[i];
            let time = time as usize;
            // The scale changes on its frame, so the stretch is played in
            // pieces that each have one.
            while let Some(at) = changes.next_if(|&at| at as usize <= time) {
                self.advance(instrument, audio, at as usize, &mut done);
            }
            if time > done {
                self.play(instrument, std::iter::empty(), time - done, done);
                done = time;
            }
            if self.admit(&event, bend, audio, time) {
                self.play(instrument, std::iter::once(event), 0, done);
            }
        }
        for at in changes {
            self.advance(instrument, audio, at as usize, &mut done);
        }
        self.play(instrument, std::iter::empty(), frames - done, done);
    }

    /// The scale `at` frames into the stretch: the device's current scale
    /// while its scale switches are on and the set's tuning doesn't hide
    /// scales, else the chromatic one.
    fn scale_at(&self, audio: &Audio<'_, '_>, at: usize) -> Scale {
        if self.state.cfg.scale_aware && !audio.scales_hidden() {
            audio.scale_at(at as u32)
        } else {
            Scale::default()
        }
    }

    /// Plays up to frame `to` of the stretch and takes up the scale there.
    fn advance<I: Instrument>(
        &mut self,
        instrument: &mut I,
        audio: &Audio<'_, '_>,
        to: usize,
        done: &mut usize,
    ) {
        if to > *done {
            self.play(instrument, std::iter::empty(), to - *done, *done);
            *done = to;
        }
        self.state.scale = self.scale_at(audio, to);
    }

    /// Plays `frames` frames with `events` from `offset` frames into the
    /// stretch.
    fn play<I: Instrument>(
        &mut self,
        instrument: &mut I,
        events: impl IntoIterator<Item = Event>,
        frames: usize,
        offset: usize,
    ) {
        let mut adapter = Adapter {
            instrument,
            state: &mut self.state,
            keys: &self.keys,
            rate: self.rate,
            offset,
        };
        self.voices.process(events, frames, &mut adapter);
    }

    /// The kit's settings from the stretch's parameters (R3, R5 to R7, R9,
    /// R14).
    fn apply_settings(&mut self) {
        let cfg = self.state.cfg;
        let settings = self.voices.settings_mut();
        settings.voices = cfg.voices;
        settings.steal = cfg.steal;
        settings.same_note = if cfg.retrigger {
            SameNote::On
        } else {
            SameNote::Off
        };
        settings.mono = cfg.mono;
        settings.legato = cfg.legato;
        settings.glide.curve = 0.0;
        settings.glide.always = cfg.glide_always;
        settings.glide.scaled = cfg.glide_prop;
        // The wheel moves every note by PB Range; Note PB follows a
        // per-note bend over its own range.
        settings.mpe.enabled = cfg.note_pb;
        settings.mpe.bend_range = cfg.note_range.clamp(0.0, 96.0);
        // A device without a PB Range (Collision) has no wheel bend: the
        // wheel is only a modulation source there.
        let wheel = if self.parts.pb_range.is_some() {
            cfg.pb_range.clamp(0.0, 96.0)
        } else {
            0.0
        };
        settings.bend_up = wheel;
        settings.bend_down = -wheel;
    }

    /// Turns the stretch's notes into the kit's events (spec: instrument
    /// voices R14, R18; device framework R39).
    fn plan(&mut self, audio: &Audio<'_, '_>) {
        let cfg = self.state.cfg;
        self.planned.clear();
        for note in audio.notes() {
            if self.planned.len() >= self.planned.capacity() {
                break;
            }
            // A key past a tuning's range plays nothing; a note it already
            // plays still gets its release.
            if matches!(note.kind, NoteKind::On) && self.keys.pitch(note.key).is_none() {
                continue;
            }
            let which = Note {
                port: Some(0),
                channel: Some(note.channel),
                key: Some(note.key),
                id: note.note_id,
            };
            let time = note.offset;
            let mut bend = None;
            let event = match note.kind {
                NoteKind::On => Event::NoteOn {
                    time,
                    note: which,
                    velocity: note.velocity,
                },
                NoteKind::Off => Event::NoteOff {
                    time,
                    note: which,
                    velocity: note.velocity,
                },
                NoteKind::Choke => Event::NoteChoke { time, note: which },
                // With Note PB off a per-note bend is ignored; pressure and
                // slide stay.
                NoteKind::Expression {
                    expression: NoteExpression::Bend,
                    ..
                } if !cfg.note_pb => continue,
                NoteKind::Control { .. }
                | NoteKind::PitchBend { .. }
                | NoteKind::ChannelPressure { .. } => {
                    let Some(message) = note.message() else {
                        continue;
                    };
                    Event::Midi {
                        time,
                        port: 0,
                        data: message.padded(),
                    }
                }
                NoteKind::Expression { expression, value } => {
                    let (expression, value) = match expression {
                        // A per-note bend is the kit's tuning expression,
                        // in semitones over the Note PB range; the kit
                        // keeps it on the one note the event names. Under
                        // a tuning it moves the note by steps of the
                        // tuning: the kit adds it to the pitch the voice
                        // started at, the key's tuned pitch (tunings R13).
                        NoteExpression::Bend => {
                            bend = Some(value);
                            let steps = value * cfg.note_range;
                            let moved = if self.keys.is_tuned() {
                                self.keys
                                    .bent(note.key, f64::from(steps))
                                    .zip(self.keys.pitch(note.key))
                                    .map_or(steps, |(bent, tuned)| (bent - tuned) as f32)
                            } else {
                                steps
                            };
                            (Expression::Tuning, moved)
                        }
                        NoteExpression::Pressure => (Expression::Pressure, value),
                        NoteExpression::Slide => (Expression::Brightness, value),
                    };
                    Event::NoteExpression {
                        time,
                        note: which,
                        expression,
                        value: f64::from(value),
                    }
                }
            };
            self.planned.push(Planned { time, event, bend });
        }
    }

    /// Decides what the voices, as they are at the event's frame, mean for
    /// it, and keeps the section's records; false if the note isn't played
    /// (R3, R8, R15).
    fn admit(
        &mut self,
        event: &Event,
        bend: Option<f32>,
        audio: &Audio<'_, '_>,
        time: usize,
    ) -> bool {
        match *event {
            Event::NoteOn { note, .. } => self.admit_note(note, audio, time),
            Event::NoteExpression {
                note,
                expression: Expression::Tuning,
                ..
            } => {
                let block = self.voices.block();
                for voice in block.active_voices() {
                    if note.matches(&block.info(voice).note) {
                        self.state.note_pb[voice] = bend.unwrap_or(0.0);
                    }
                }
                true
            }
            Event::Midi { data, .. } if data[0] & 0xf0 == 0xe0 => {
                let channel = usize::from(data[0] & 15);
                if self.state.cfg.note_pb && channel != 0 {
                    let raw = i32::from(data[1] & 0x7f) | i32::from(data[2] & 0x7f) << 7;
                    let bend = if raw >= 8192 {
                        (raw - 8192) as f32 / 8191.0
                    } else {
                        (raw - 8192) as f32 / 8192.0
                    };
                    self.state.channel_bend[channel] = bend;
                    let block = self.voices.block();
                    for voice in block.active_voices() {
                        if block.info(voice).note.channel == Some(channel as u8) {
                            self.state.note_pb[voice] = bend;
                        }
                    }
                }
                true
            }
            _ => true,
        }
    }

    fn admit_note(&mut self, note: Note, audio: &Audio<'_, '_>, time: usize) -> bool {
        let cfg = self.state.cfg;
        let key = note.key.unwrap_or(0);
        self.state.scale = self.scale_at(audio, time);
        let block = self.voices.block();
        let active = block.active_count();
        let sounding = |v: &usize| block.info(*v).state.is_gated();
        let released = block.active_voices().any(|v| !sounding(&v));
        let reuse = cfg.retrigger && block.active_voices().any(|v| block.info(v).key == key);
        let held_keys = || block.active_voices().map(|v| block.info(v).key);
        let (lowest, highest) = (held_keys().min(), held_keys().max());
        // R8: the first note after silence doesn't glide, and Glissando
        // makes its own steps.
        let settings = self.voices.settings_mut();
        settings.glide.time = if cfg.glissando || active == 0 || !(cfg.mono || cfg.poly_glide) {
            0.0
        } else {
            cfg.glide_secs
        };
        settings.steal = cfg.steal;
        if cfg.mono || reuse || active < cfg.voices {
            return true;
        }
        // R3: released tails go first, the oldest first; else a new note
        // that doesn't beat the note it would take isn't played.
        if released {
            settings.steal = StealPriority::Newest;
            return true;
        }
        match cfg.steal {
            StealPriority::Highest => lowest.is_some_and(|lowest| key > lowest),
            StealPriority::Lowest => highest.is_some_and(|highest| key < highest),
            _ => true,
        }
    }
}

//! What Bass and Poli share (WP-8.16; the bass and Poli spec): the way a
//! voice start reaches the oscillators and envelopes, an ADSR's settings,
//! and the instruments' filter run over mono voices.

use lspk_dsp::envelope::{EnvelopeBank, EnvelopeParams, StealRestart, TimeUnit, Timing, Trigger};
use lspk_voice::Start;
use lspk_wavetable::osc::Onset;

use crate::instrument_filter::{InstrumentFilter, Per, PerFrame, Setup};

/// The key the filter's cutoff is quoted at: Key moves it from this note,
/// which Live calls C3.
pub(crate) const MIDDLE_C: f32 = 60.0;
/// The frequency A4 is tuned to.
pub(crate) const REFERENCE: f32 = 440.0;
/// The shortest envelope time, in milliseconds: a stage of no length is
/// left to the envelope's own handling of none.
const MIN_TIME_MS: f32 = 0.1;

/// How a note starts in a voice whose amplifier envelope is `idle`: a voice
/// the section took while its tail had already ended starts as a free one,
/// as it would have had the block ended where the tail did, so a render
/// doesn't depend on where blocks fall.
pub(crate) fn effective(start: Start, idle: bool) -> Start {
    match start {
        Start::Steal | Start::Retrigger if idle => Start::Fresh,
        other => other,
    }
}

/// The seed of the random draws a note makes, from the device's seed and
/// the note's number: the same note draws the same values at any block
/// size.
pub(crate) fn note_seed(seed: u64, note: u64) -> u64 {
    seed ^ note.wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// How an oscillator starts a note the voice section started as `start`.
pub(crate) fn onset(start: Start) -> Onset {
    match start {
        Start::Fresh => Onset::Fresh,
        Start::Steal => Onset::Steal,
        Start::Retrigger => Onset::Retrigger,
        Start::Legato => Onset::Legato,
        Start::Join { .. } => Onset::Join,
    }
}

/// Starts `voice`'s envelope as `start` says: a free voice from silence, a
/// taken one from the level it had reached, a legato note carrying on.
pub(crate) fn trigger(bank: &mut EnvelopeBank, voice: usize, start: Start) {
    match start {
        Start::Fresh => bank.trigger(voice, Trigger::New),
        Start::Steal | Start::Retrigger => bank.trigger(voice, Trigger::Retrigger),
        Start::Legato => bank.trigger(voice, Trigger::Legato),
        Start::Join { leader } => bank.join(voice, leader),
    }
}

/// An ADSR in milliseconds and a sustain from 0 to 1, with the curves of
/// Drift's envelopes, restarting a taken voice from its level.
pub(crate) fn adsr(attack: f64, decay: f64, sustain: f64, release: f64) -> EnvelopeParams {
    EnvelopeParams {
        unit: TimeUnit::Milliseconds,
        attack: (attack as f32).max(MIN_TIME_MS),
        decay: (decay as f32).max(MIN_TIME_MS),
        sustain: sustain as f32,
        release: (release as f32).max(MIN_TIME_MS),
        attack_curve: 0.5,
        decay_curve: 0.666,
        release_curve: 0.666,
        steal_restart: StealRestart::CurrentLevel,
        ..EnvelopeParams::AMP
    }
}

/// An envelope bank for `voices` voices at `rate`.
pub(crate) fn envelopes(voices: usize, rate: f64) -> EnvelopeBank {
    EnvelopeBank::new(voices, EnvelopeParams::AMP, Timing::new(rate))
}

/// Sets `bank`'s settings when they differ from `kept`.
pub(crate) fn set_envelope(
    bank: &mut EnvelopeBank,
    kept: &mut EnvelopeParams,
    wanted: EnvelopeParams,
) {
    if wanted != *kept {
        *kept = wanted;
        bank.set_all_params(wanted);
    }
}

/// The instruments' filter run over mono voices: the cutoff per frame and
/// slot, and the audio in the filter's two-channel layout, whose second
/// channel stays silent.
pub(crate) struct Filtering {
    filter: InstrumentFilter,
    slots: usize,
    stride: usize,
    cutoff: Vec<f32>,
    audio: Vec<f32>,
    /// A run of zeros for Morph, which neither device has, and for a Drive
    /// that isn't there.
    zero: Vec<f32>,
}

impl Filtering {
    /// A filter for `slots` voices at `rate`, for blocks of up to
    /// `max_frames`. Allocates, so make it off the audio thread.
    pub(crate) fn new(rate: f64, slots: usize, max_frames: usize) -> Self {
        let filter = InstrumentFilter::new(rate as f32, slots, max_frames);
        let stride = filter.stride();
        Self {
            filter,
            slots,
            stride,
            cutoff: vec![0.0; max_frames * stride],
            audio: vec![0.0; max_frames * 2 * stride],
            zero: vec![0.0; max_frames],
        }
    }

    /// Clears the audio for a run of `frames`.
    pub(crate) fn begin(&mut self, frames: usize) {
        self.audio[..frames * 2 * self.stride].fill(0.0);
    }

    /// Sets slot `slot`'s cutoff at `frame`, in the kit's semitones.
    pub(crate) fn set_cutoff(&mut self, frame: usize, slot: usize, pitch: f32) {
        self.cutoff[frame * self.stride + slot] = pitch;
    }

    /// Puts `x` into slot `slot`'s audio at `frame`.
    pub(crate) fn put(&mut self, frame: usize, slot: usize, x: f32) {
        self.audio[frame * 2 * self.stride + slot] = x;
    }

    /// Slot `slot`'s filtered audio at `frame`.
    pub(crate) fn get(&self, frame: usize, slot: usize) -> f32 {
        self.audio[frame * 2 * self.stride + slot]
    }

    /// Starts `slot`'s voice afresh at the first frame of the next run.
    pub(crate) fn reset_voice(&mut self, slot: usize) {
        self.filter.reset_voice(slot, 0);
    }

    /// Filters `frames` frames in `setup`, with Resonance and Drive (none
    /// where a device has no Drive) from 0 to 1 for every frame.
    pub(crate) fn run(&mut self, setup: Setup, frames: usize, res: &[f32], drive: Option<&[f32]>) {
        self.filter.set_setup(setup);
        let controls = PerFrame {
            cutoff: &self.cutoff[..frames * self.stride],
            res: Per::Shared(&res[..frames]),
            morph: Per::Shared(&self.zero[..frames]),
            drive: Per::Shared(drive.map_or(&self.zero[..frames], |d| &d[..frames])),
        };
        self.filter.process(
            self.slots,
            frames,
            &controls,
            &mut self.audio[..frames * 2 * self.stride],
        );
    }
}

//! The first blocks' signal processing (WP-9.36; the modular device spec,
//! R21 to R28): one [`State`] per block per voice, worked out frame by
//! frame so a patch renders the same at any block size (R7).

use std::f64::consts::TAU;

use crate::filters::{Chan, Knobs, Plan, Ring};

/// The most inputs a block has.
pub(super) const MAX_IN: usize = 4;
/// The most outputs a block has.
pub(super) const MAX_OUT: usize = 6;
/// The most settings a block has.
pub(super) const MAX_SET: usize = 7;
/// The longest a Delay block delays, in seconds (R28).
const MAX_DELAY_SECONDS: f64 = 4.0;
/// C3, the pitch a control of 0 octaves plays.
const C3_HZ: f64 = 261.625_565_300_598_6;

/// What a frame brings to a block besides its inputs.
#[derive(Clone, Copy)]
pub(super) struct Frame {
    pub(super) rate: f64,
    pub(super) bpm: f64,
}

/// A block's state: what it remembers from frame to frame. The variant is
/// the block's type.
pub(super) enum State {
    Oscillator {
        phase: f64,
        sync: bool,
    },
    Filter(Box<Chan>),
    Envelope(Envelope),
    Lfo {
        phase: f64,
        seed: u32,
        held: f32,
        reset: bool,
    },
    Amplifier,
    Mixer,
    Arithmetic,
    Delay(Ring),
    NoteIn,
    NoteOut,
    AudioIn,
    AudioOut,
    SidechainIn,
    /// A block Beatlayer doesn't know (R10): it makes nothing.
    Silent,
}

impl State {
    /// A block of type `id` at rest, for `rate`; `seed` starts a random
    /// LFO's values. Allocates: not for the audio thread.
    pub(super) fn new(id: &str, rate: f64, seed: u32) -> Self {
        match id {
            "oscillator" => Self::Oscillator {
                phase: 0.0,
                sync: false,
            },
            "filter" => Self::Filter(Box::new(Chan::new(rate))),
            "envelope" => Self::Envelope(Envelope::default()),
            "lfo" => Self::Lfo {
                phase: 0.0,
                seed: seed | 1,
                held: 0.0,
                reset: false,
            },
            "amplifier" => Self::Amplifier,
            "mixer" => Self::Mixer,
            "arithmetic" => Self::Arithmetic,
            "delay" => Self::Delay(Ring::new((rate * MAX_DELAY_SECONDS) as usize + 4)),
            "note_in" => Self::NoteIn,
            "note_out" => Self::NoteOut,
            "audio_in" => Self::AudioIn,
            "audio_out" => Self::AudioOut,
            "sidechain_in" => Self::SidechainIn,
            _ => Self::Silent,
        }
    }

    /// Back to rest, in place, as a new voice starts (nothing allocates).
    pub(super) fn reset(&mut self) {
        match self {
            Self::Oscillator { phase, sync } => (*phase, *sync) = (0.0, false),
            Self::Filter(chan) => chan.reset(),
            Self::Envelope(envelope) => *envelope = Envelope::default(),
            Self::Lfo {
                phase, held, reset, ..
            } => (*phase, *held, *reset) = (0.0, 0.0, false),
            Self::Delay(ring) => ring.samples.fill(0.0),
            _ => {}
        }
    }

    /// Works out one frame: `ins` are the connected inputs' values, `set`
    /// the settings as they stand, and the outputs go to `out`.
    pub(super) fn tick(
        &mut self,
        frame: Frame,
        ins: [Option<f32>; MAX_IN],
        set: &[f64; MAX_SET],
        out: &mut [f32; MAX_OUT],
    ) {
        let input = |i: usize| ins[i].unwrap_or(0.0);
        let gate = |i: usize| ins[i].is_some_and(|v| v > 0.5);
        match self {
            Self::Oscillator { phase, sync } => {
                // Spec: modular-device R21. The pitch input is in octaves
                // from C3, Coarse and Fine add to it, a phase-modulation
                // input moves the phase by up to half a cycle, and a rising
                // gate on Sync starts the cycle again.
                let octaves = f64::from(input(0)) + set[2] / 12.0 + set[3] / 1200.0;
                let hz = C3_HZ * octaves.exp2();
                let dt = (hz / frame.rate).clamp(0.0, 0.49);
                let synced = gate(2);
                if synced && !*sync {
                    *phase = 0.0;
                }
                *sync = synced;
                let at = (*phase + f64::from(input(1)) * 0.5).rem_euclid(1.0);
                out[0] = wave(set[0] as u8, at, dt, set[1]) as f32;
                *phase = (*phase + dt).rem_euclid(1.0);
            }
            Self::Filter(chan) => {
                // Spec: modular-device R22. The cutoff input is in octaves.
                let hz = (set[2] * f64::from(input(1)).exp2()).clamp(20.0, 20_000.0);
                let plan = Plan::new(
                    set[0] as usize,
                    if set[1] >= 1.0 { 2 } else { 0 },
                    0,
                    Knobs {
                        hz,
                        res: set[3],
                        morph: 0.0,
                        control: 0.0,
                        pitch: 0.0,
                        formant: 0.0,
                    },
                );
                out[0] = chan.tick(f64::from(input(0)), &plan, frame.rate) as f32;
            }
            Self::Envelope(envelope) => out[0] = envelope.tick(frame.rate, gate(0), set) as f32,
            Self::Lfo {
                phase,
                seed,
                held,
                reset,
            } => {
                // Spec: modular-device R24. A rate in hertz, or the song's
                // tempo over a division of a bar; a rising gate on Reset
                // starts the cycle again.
                let hz = if set[2] >= 1.0 {
                    const BEATS: [f64; 7] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0];
                    frame.bpm / 60.0 / BEATS[(set[3] as usize).min(BEATS.len() - 1)]
                } else {
                    set[1]
                };
                let restart = gate(0) && !*reset;
                *reset = gate(0);
                if restart {
                    *phase = 0.0;
                    *held = noise(seed);
                }
                out[0] = match set[0] as u8 {
                    0 => (TAU * *phase).sin() as f32,
                    1 => (1.0 - 4.0 * (*phase - 0.5).abs()) as f32,
                    2 => (2.0 * *phase - 1.0) as f32,
                    3 => {
                        if *phase < 0.5 {
                            1.0
                        } else {
                            -1.0
                        }
                    }
                    _ => *held,
                };
                *phase += hz / frame.rate;
                if *phase >= 1.0 {
                    *phase -= phase.floor();
                    *held = noise(seed);
                }
            }
            Self::Amplifier => {
                // Spec: modular-device R25. A control input left
                // unconnected is 1.
                let gain = 10f64.powf(set[0] / 20.0) as f32;
                out[0] = input(0) * ins[1].unwrap_or(1.0) * gain;
            }
            Self::Mixer => {
                out[0] = (0..4).map(|i| input(i) * set[i] as f32).sum();
            }
            Self::Arithmetic => {
                // Spec: modular-device R17, R27. An unconnected input takes
                // the block's own setting.
                let a = ins[0].unwrap_or(set[1] as f32);
                let b = ins[1].unwrap_or(set[2] as f32);
                out[0] = match set[0] as u8 {
                    0 => a + b,
                    1 => a - b,
                    2 => a * b,
                    3 => a.min(b),
                    4 => a.max(b),
                    5 => a.abs(),
                    _ => set[1] as f32,
                };
            }
            Self::Delay(ring) => {
                // Spec: modular-device R28. At least one sample, at most
                // four seconds; the time input adds seconds. The input is
                // written after the frame (`write`), so a cable from the
                // delay's own output back to its input is no loop.
                let seconds = set[0] / 1000.0 + f64::from(input(1));
                let max = ring.samples.len() as f64 - 3.0;
                out[0] = ring.read((seconds * frame.rate).clamp(1.0, max)) as f32;
            }
            Self::NoteIn
            | Self::NoteOut
            | Self::AudioIn
            | Self::AudioOut
            | Self::SidechainIn
            | Self::Silent => {}
        }
    }

    /// What a Delay block takes in, at the end of the frame.
    pub(super) fn write(&mut self, input: f32) {
        if let Self::Delay(ring) = self {
            ring.write(f64::from(input));
        }
    }
}

/// A random LFO's next value, from −1 to 1.
fn noise(seed: &mut u32) -> f32 {
    *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (*seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0
}

/// An oscillator's wave at `phase` (0 to 1) with `dt` of a cycle a frame,
/// band-limited at its jumps and corners (R21).
fn wave(shape: u8, phase: f64, dt: f64, width: f64) -> f64 {
    match shape {
        0 => (TAU * phase).sin(),
        1 => {
            // A triangle: corners at the top (phase 0) and bottom (0.5).
            let naive = 4.0 * (phase - 0.5).abs() - 1.0;
            naive - 8.0 * blamp(phase, dt) + 8.0 * blamp((phase + 0.5).rem_euclid(1.0), dt)
        }
        2 => 2.0 * phase - 1.0 - blep(phase, dt),
        _ => {
            let naive = if phase < width { 1.0 } else { -1.0 };
            naive + blep(phase, dt) - blep((phase - width).rem_euclid(1.0), dt)
        }
    }
}

/// The polynomial step residual after (`t` just past 0) and before (`t` just
/// short of 1) a jump of 2.
fn blep(t: f64, dt: f64) -> f64 {
    if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

/// The same for a corner, scaled by `dt`: multiply by the change of slope.
fn blamp(t: f64, dt: f64) -> f64 {
    if t < dt {
        let x = t / dt - 1.0;
        -x * x * x * dt / 3.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt + 1.0;
        x * x * x * dt / 3.0
    } else {
        0.0
    }
}

/// An envelope's stages (R23).
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// Attack, Decay, Sustain and Release, started by a gate.
#[derive(Default)]
pub(super) struct Envelope {
    stage: Stage,
    level: f64,
    /// Where the running stage began, and how far through it the envelope is.
    from: f64,
    progress: f64,
    gate: bool,
}

impl Envelope {
    /// One frame: a rising gate attacks from the level the envelope has, a
    /// falling one releases from it. Each stage's curve bends its travel:
    /// 0 is a line, above 0 starts slowly, below 0 quickly. `set` is
    /// Attack, Decay, Sustain, Release and the three curves.
    fn tick(&mut self, rate: f64, gate: bool, set: &[f64; MAX_SET]) -> f64 {
        if gate && !self.gate {
            (self.stage, self.from, self.progress) = (Stage::Attack, self.level, 0.0);
        } else if !gate && self.gate {
            (self.stage, self.from, self.progress) = (Stage::Release, self.level, 0.0);
        }
        self.gate = gate;
        // Moves along a stage of `ms`; true once it has ended.
        let advance = |progress: &mut f64, ms: f64| {
            *progress += if ms > 0.0 { 1000.0 / (ms * rate) } else { 1.0 };
            *progress >= 1.0
        };
        let bend = |progress: f64, curve: f64| progress.min(1.0).powf((curve * 4f64.ln()).exp());
        match self.stage {
            Stage::Idle => {}
            Stage::Attack => {
                if advance(&mut self.progress, set[0]) {
                    (self.level, self.stage, self.from, self.progress) =
                        (1.0, Stage::Decay, 1.0, 0.0);
                } else {
                    self.level = self.from + (1.0 - self.from) * bend(self.progress, set[4]);
                }
            }
            Stage::Decay => {
                if advance(&mut self.progress, set[1]) {
                    (self.level, self.stage) = (set[2], Stage::Sustain);
                } else {
                    self.level = 1.0 + (set[2] - 1.0) * bend(self.progress, set[5]);
                }
            }
            Stage::Sustain => self.level = set[2],
            Stage::Release => {
                if advance(&mut self.progress, set[3]) {
                    (self.level, self.stage) = (0.0, Stage::Idle);
                } else {
                    self.level = self.from * (1.0 - bend(self.progress, set[6]));
                }
            }
        }
        self.level
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Frame = Frame {
        rate: 48_000.0,
        bpm: 120.0,
    };

    fn tick(state: &mut State, ins: [Option<f32>; MAX_IN], set: &[f64; MAX_SET]) -> f32 {
        let mut out = [0.0; MAX_OUT];
        state.tick(FRAME, ins, set, &mut out);
        out[0]
    }

    // Spec: modular-device R21
    #[test]
    fn a_rising_gate_on_sync_starts_the_oscillators_cycle_again() {
        let mut osc = State::new("oscillator", 48_000.0, 1);
        // A saw: 2 * phase - 1, so a fresh cycle reads −1.
        let set = [2.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut last = 0.0;
        for _ in 0..100 {
            last = tick(&mut osc, [None; MAX_IN], &set);
        }
        assert!(last > -0.9);
        // The jump back is smoothed, so the cycle reads −1 a frame later.
        tick(&mut osc, [None, None, Some(1.0), None], &set);
        let next = tick(&mut osc, [None, None, Some(1.0), None], &set);
        assert!(next < -0.95, "{next}");
        // The gate held high doesn't restart it again.
        for _ in 0..50 {
            tick(&mut osc, [None, None, Some(1.0), None], &set);
        }
        assert!(tick(&mut osc, [None, None, Some(1.0), None], &set) > -0.9);
    }

    // Spec: modular-device R24
    #[test]
    fn a_rising_gate_on_reset_starts_the_lfos_cycle_again() {
        let mut lfo = State::new("lfo", 48_000.0, 1);
        // A saw at 1 Hz.
        let set = [2.0, 1.0, 0.0, 2.0, 0.0, 0.0, 0.0];
        for _ in 0..24_000 {
            tick(&mut lfo, [None; MAX_IN], &set);
        }
        assert!(
            tick(&mut lfo, [None; MAX_IN], &set).abs() < 0.01,
            "half way through the cycle"
        );
        assert!((tick(&mut lfo, [Some(1.0), None, None, None], &set) + 1.0).abs() < 0.01);
    }
}

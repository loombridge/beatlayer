//! CV Clock In's follower (`docs/specs/cv-tools.md`, R23 to R25): a clock
//! pulse train on a CV input, followed as the transport spec's R24 and R25
//! follow an outside tempo and as [MIDI clock in](crate::clock_in) follows
//! the clocks of an input.
//!
//! A [`CvClock`] belongs to one CV Clock In device. Once per block, before
//! the tracks run, the program hands it the interface's input for the block
//! ([`Node::follow_cv`](crate::Node::follow_cv)); it finds the pulses on it,
//! each timed to the straight line between the two frames it crosses
//! between, fits their times as a MIDI clock's are fitted, and answers with
//! the tempo and a little speed to keep the song's beat on the pulses. A CV
//! clock carries no start, stop or position, so it never starts, stops or
//! locates the transport.
//!
//! Times are nanoseconds on the engine clock, the frame count at the sample
//! rate, so the same pulses give the same fit at any block size. Nothing
//! here allocates, locks or blocks.

use bl_core::time::TICKS_PER_QUARTER;

use crate::clock_in::{CORRECTION_SPAN, DEADBAND, Fit, MAX_CORRECTION, SIXTEENTH_TICKS};
use crate::follower::{Ask, Follow};

/// How far below Threshold the input has to have been before a rise counts
/// as a pulse, in sample values: 0.1 V (R23).
const HYSTERESIS: f32 = 0.01;
/// The least time without a pulse before the clock counts as silent (R24).
const SILENCE_NS: f64 = 500e6;
/// How long the pulse light stays lit after a pulse, in seconds.
const LAMP_SECONDS: f64 = 0.1;

/// Who already leads the song when a CV Clock In is asked (R25).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Others {
    /// Nothing does.
    #[default]
    Nobody,
    /// A MIDI clock under External Sync.
    Midi,
    /// A CV Clock In earlier in the song's track and chain order.
    Cv,
}

/// The CV Clock In that leads the song's tempo (R25).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Leader {
    /// The device.
    pub device: bl_model::DeviceId,
    /// The tempo it measures, in BPM.
    pub tempo: f64,
}

/// A CV Clock In's controls at the start of a block (R23).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Sync: whether the device leads the song.
    pub sync: bool,
    /// Rate: pulses to the quarter note.
    pub per_quarter: f64,
    /// Threshold, in sample values (1.0 is 10 V).
    pub threshold: f32,
}

/// What a CV clock is told of the block.
pub struct Cx<'a> {
    /// The block the processor is about to play.
    pub ask: &'a Ask,
    /// What the device's CV input reads over the block's frames; `None` for
    /// an input at no channel, or one the interface lacks: 0 V.
    pub input: Option<&'a [f32]>,
    /// The Arrangement loop's start and end in ticks, while it is on.
    pub looped: Option<(f64, f64)>,
    /// Who leads already.
    pub others: Others,
}

/// A CV clock's follower and what its panel shows.
#[derive(Clone, Debug)]
pub struct CvClock {
    fit: Fit,
    per_quarter: f64,
    /// The tempo of the last fit, kept over silence.
    prior: Option<f64>,
    /// When the last pulse came, in nanoseconds on the engine clock.
    last: f64,
    /// Whether `position` is the song position at the fit's time.
    anchored: bool,
    /// The song position, in ticks, at the fit's last pulse.
    position: f64,
    /// Whether the input has been below Threshold by the hysteresis since
    /// its last pulse, and its last sample.
    armed: bool,
    previous: f32,
    pulses: u64,
    last_pulse: Option<u64>,
    /// Frames the pulse light stays lit.
    lamp: f64,
    leads: bool,
}

impl Default for CvClock {
    fn default() -> Self {
        Self::new()
    }
}

impl CvClock {
    /// A clock that has heard nothing.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            fit: Fit::per_quarter(4.0),
            per_quarter: 4.0,
            prior: None,
            last: f64::NEG_INFINITY,
            anchored: false,
            position: 0.0,
            armed: false,
            previous: 0.0,
            pulses: 0,
            last_pulse: None,
            lamp: 0.0,
            leads: false,
        }
    }

    /// The tempo measured on the pulses, in BPM, once there is one: the last
    /// one over silence.
    #[must_use]
    pub fn measured(&self) -> Option<f64> {
        self.fit.bpm().or(self.prior)
    }

    /// Whether the device leads the song.
    #[must_use]
    pub fn leads(&self) -> bool {
        self.leads
    }

    /// Whether the pulse light is lit.
    #[must_use]
    pub fn lit(&self) -> bool {
        self.lamp > 0.0
    }

    /// The engine clock frame of the last pulse.
    #[must_use]
    pub fn last_pulse(&self) -> Option<u64> {
        self.last_pulse
    }

    /// Reads the block's pulses and answers for it: the tempo the device
    /// leads with, or `None` while it leads nothing (R24, R25).
    pub fn block(&mut self, set: Settings, cx: &Cx<'_>, answer: &mut Follow) -> Option<f64> {
        let ask = cx.ask;
        if set.per_quarter != self.per_quarter {
            // A new Rate: the old pulses mean another tempo.
            self.fit = Fit::per_quarter(set.per_quarter);
            self.per_quarter = set.per_quarter;
            self.prior = None;
            self.last = f64::NEG_INFINITY;
            self.anchored = false;
        }
        let per_frame = 1e9 / ask.rate;
        self.take_pulses(set.threshold, cx, per_frame);
        self.lamp = (self.lamp - ask.frames as f64).max(0.0);
        let start = ask.clock as f64 * per_frame;
        let silent = start - self.last > self.silence();
        self.leads = set.sync && cx.others == Others::Nobody && self.measured().is_some();
        let Some(bpm) = self.measured().filter(|_| self.leads) else {
            self.anchored = false;
            return None;
        };
        let ticks = TICKS_PER_QUARTER as f64 / self.per_quarter;
        let mut speed = 1.0;
        if !ask.playing {
            self.anchored = false;
        } else if !silent {
            let heard = start + f64::from(ask.latency) * per_frame;
            let ahead = |at: f64| ticks * (heard - at) / self.fit.period();
            if !self.anchored {
                // Spec: cv-tools R24. The song's own position is the
                // clock's from the first pulse after following began.
                self.position = ask.position - ahead(self.fit.time());
                self.anchored = true;
            }
            let mut error = self.position + ahead(self.fit.time()) - ask.position;
            if let Some((from, to)) = cx.looped {
                let length = to - from;
                let target = self.position + ahead(self.fit.time());
                if target >= to && length > 0.0 {
                    error = from + (target - from) % length - ask.position;
                }
                if length > 0.0 && error.abs() > length / 2.0 {
                    error -= length * error.signum();
                }
            }
            if error.abs() > SIXTEENTH_TICKS {
                // A clock never moves the transport: beginning again keeps
                // the song where it is.
                self.anchored = false;
            } else if error.abs() > DEADBAND {
                let over = error - DEADBAND * error.signum();
                speed += (over / CORRECTION_SPAN).clamp(-MAX_CORRECTION, MAX_CORRECTION);
            }
        }
        answer.tempo(bpm * speed);
        Some(bpm)
    }

    /// How long without a pulse before the clock counts as silent: twice the
    /// last interval, and at least 500 ms (R24).
    fn silence(&self) -> f64 {
        if self.fit.weight() >= 2 {
            SILENCE_NS.max(2.0 * self.fit.period())
        } else {
            SILENCE_NS
        }
    }

    /// Finds the rises through Threshold in the block's input and takes
    /// each as a pulse (R23).
    fn take_pulses(&mut self, threshold: f32, cx: &Cx<'_>, per_frame: f64) {
        let clock = cx.ask.clock as f64;
        let Some(input) = cx.input else {
            // 0 V: below any Threshold above the hysteresis.
            self.previous = 0.0;
            self.armed |= 0.0 <= threshold - HYSTERESIS;
            return;
        };
        for (i, &value) in input.iter().enumerate() {
            if value <= threshold - HYSTERESIS {
                self.armed = true;
            } else if self.armed && value >= threshold && self.previous < threshold {
                self.armed = false;
                // Between frames, by the straight line from the last sample.
                let rise = value - self.previous;
                let part = if rise > 0.0 {
                    f64::from((threshold - self.previous) / rise)
                } else {
                    1.0
                };
                let frame = clock + i as f64 - 1.0 + part;
                self.pulse(frame * per_frame, frame, per_frame);
            }
            self.previous = value;
        }
    }

    /// Takes a pulse at `time` nanoseconds, `frame` frames into the engine
    /// clock of `per_frame` nanoseconds each, as MIDI clock in takes a
    /// Timing Clock.
    fn pulse(&mut self, time: f64, frame: f64, per_frame: f64) {
        self.pulses += 1;
        self.last_pulse = Some(frame.round().max(0.0) as u64);
        self.lamp = LAMP_SECONDS * 1e9 / per_frame;
        let interval = time - self.last;
        let ticks = TICKS_PER_QUARTER as f64 / self.per_quarter;
        if interval > self.silence() || interval < 0.0 {
            // Spec: cv-tools R24. Pulses begin again, keeping the tempo they
            // had until the loop has heard a few.
            match self.prior {
                Some(bpm) => {
                    let weight = self.fit.prior_weight();
                    self.fit.lock(time, 60e9 / (self.per_quarter * bpm), weight);
                }
                None => self.fit.lock(time, self.fit.max_period(), 0),
            }
            self.anchored = false;
        } else {
            let ratio = interval / self.fit.period();
            if self.fit.weight() >= 2 && !(0.5..=1.5).contains(&ratio) {
                // A glitch is counted by its time, not fitted.
                let missed = ratio.round();
                self.fit.coast(missed);
                if self.anchored {
                    self.position += missed * ticks;
                }
                self.last = time;
                return;
            }
            if !(self.fit.min_period()..=self.fit.max_period()).contains(&interval)
                && self.fit.weight() < 2
            {
                return;
            }
            self.fit.push(time);
            if self.anchored {
                self.position += ticks;
            }
        }
        self.last = time;
        self.prior = self.fit.bpm().or(self.prior);
    }
}

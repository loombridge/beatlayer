//! Synthesized metronome click sounds and timing (WP-2.15).
//!
//! The clicks are Beatlayer's own: short, decaying tones shaped per sound,
//! synthesized on the audio thread with no allocation. Timing follows the
//! program's tempo map and time signatures, so a click lands on the exact
//! frame of its beat.

use std::f32::consts::PI;

use bl_core::time::{Frames, SampleRate, TempoMap, Ticks, TimeSignatureMap};
use bl_model::{MetronomeSettings, MetronomeSound};

/// How many clicks can sound at once. Beats are far enough apart that even
/// the fastest rhythm never overlaps more than a couple.
const MAX_ACTIVE_CLICKS: usize = 4;

/// One interleaved block of the engine's output: `channels` channels of
/// `stride` frames each, with this block's `length` frames starting at
/// `offset`.
pub struct Block<'a> {
    /// The interleaved samples.
    pub output: &'a mut [f32],
    /// Frames per channel in `output`.
    pub stride: usize,
    /// The engine's channel count.
    pub channels: usize,
    /// This block's first frame in `output`.
    pub offset: usize,
    /// This block's frame count.
    pub length: usize,
}

impl Block<'_> {
    /// Adds `sample` to every channel of frame `i` of this block.
    #[inline]
    fn add(&mut self, i: usize, sample: f32) {
        for c in 0..self.channels {
            let to = c * self.stride + self.offset + i;
            self.output[to] += sample;
        }
    }
}

/// One active synthesized click voice.
#[derive(Clone, Copy, Debug)]
struct ActiveClick {
    sound: MetronomeSound,
    accented: bool,
    frame: usize,
    total_frames: usize,
}

impl ActiveClick {
    fn new(sound: MetronomeSound, accented: bool, sample_rate: SampleRate) -> Self {
        let rate = sample_rate.as_f64();
        let duration = match sound {
            MetronomeSound::Classic => 0.020,
            MetronomeSound::Click => 0.010,
            MetronomeSound::Wood => 0.025,
        };
        let total_frames = (duration * rate).round() as usize;
        Self {
            sound,
            accented,
            frame: 0,
            total_frames: total_frames.max(1),
        }
    }

    #[inline]
    fn render_sample(&mut self, rate: f32) -> Option<f32> {
        if self.frame >= self.total_frames {
            return None;
        }
        let t = self.frame as f32 / rate;
        self.frame += 1;

        let sample = match self.sound {
            MetronomeSound::Classic => {
                let freq = if self.accented { 2500.0 } else { 1250.0 };
                let decay = if self.accented { 0.005 } else { 0.004 };
                let amp = if self.accented { 0.6 } else { 0.4 };
                let env = libm::expf(-t / decay);
                amp * env * libm::cosf(2.0 * PI * freq * t)
            }
            MetronomeSound::Click => {
                let freq = if self.accented { 3200.0 } else { 2000.0 };
                let decay = 0.0018;
                let amp = if self.accented { 0.7 } else { 0.45 };
                let env = libm::expf(-t / decay);
                let pulse = if t < 0.0004 {
                    if self.accented { 0.3 } else { 0.2 }
                } else {
                    0.0
                };
                (amp * env * libm::cosf(2.0 * PI * freq * t)) + pulse
            }
            MetronomeSound::Wood => {
                let (f1, f2) = if self.accented {
                    (1600.0, 2400.0)
                } else {
                    (1000.0, 1500.0)
                };
                let decay = if self.accented { 0.007 } else { 0.006 };
                let amp = if self.accented { 0.6 } else { 0.4 };
                let env = libm::expf(-t / decay);
                let tone =
                    0.7 * libm::cosf(2.0 * PI * f1 * t) + 0.3 * libm::cosf(2.0 * PI * f2 * t);
                amp * env * tone
            }
        };
        Some(sample)
    }
}

/// The engine's metronome processor on the audio thread (WP-2.15).
#[derive(Clone, Debug, Default)]
pub struct MetronomeEngine {
    settings: MetronomeSettings,
    recording: bool,
    active_clicks: [Option<ActiveClick>; MAX_ACTIVE_CLICKS],
    count_in_remaining: usize,
    count_in_total: usize,
    count_in_bar_frames: usize,
    count_in_beats_per_bar: usize,
}

impl MetronomeEngine {
    /// Creates a metronome processor with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Updates the metronome settings.
    pub fn set_settings(&mut self, settings: MetronomeSettings) {
        self.settings = settings;
    }

    /// Sets whether the transport is recording.
    pub fn set_recording(&mut self, recording: bool) {
        self.recording = recording;
    }

    /// Whether metronome clicks should sound: the switch is on, and the
    /// "only while recording" setting allows it.
    fn is_audible(&self) -> bool {
        self.settings.enabled && (!self.settings.only_while_recording || self.recording)
    }

    /// Whether count-in is actively running.
    #[must_use]
    pub fn is_counting_in(&self) -> bool {
        self.count_in_remaining > 0
    }

    /// Frames of count-in remaining.
    #[must_use]
    pub fn count_in_remaining(&self) -> usize {
        self.count_in_remaining
    }

    /// Resets all active click voices and count-in state (on stop or park).
    pub fn stop(&mut self) {
        self.active_clicks = [None; MAX_ACTIVE_CLICKS];
        self.count_in_remaining = 0;
        self.count_in_total = 0;
        self.count_in_bar_frames = 0;
        self.count_in_beats_per_bar = 0;
    }

    /// Starts count-in before playback begins, if configured. One click
    /// sounds on each beat of the chosen bars, the bar's first accented.
    pub fn start_count_in(
        &mut self,
        sample_rate: SampleRate,
        tempo_map: &TempoMap,
        time_signatures: &TimeSignatureMap,
        start_ticks: Ticks,
    ) {
        self.active_clicks = [None; MAX_ACTIVE_CLICKS];
        self.count_in_remaining = 0;
        self.count_in_total = 0;
        self.count_in_bar_frames = 0;
        self.count_in_beats_per_bar = 0;
        let bars = self.settings.count_in.bars();
        if bars == 0 || !(self.is_audible() || self.recording) {
            return;
        }
        let (bar_idx, _) = time_signatures.bar_at(start_ticks);
        let bar_start = time_signatures.bar_start(bar_idx);
        let sig = time_signatures.signature_at(bar_start);
        let bar_frames = (tempo_map.ticks_to_frames(bar_start + sig.bar_length(), sample_rate)
            - tempo_map.ticks_to_frames(bar_start, sample_rate))
        .get()
        .max(1) as usize;
        let total_frames = bars as usize * bar_frames;
        self.count_in_remaining = total_frames;
        self.count_in_total = total_frames;
        self.count_in_bar_frames = bar_frames;
        self.count_in_beats_per_bar = sig.numerator() as usize;
    }

    fn trigger_click(&mut self, accented: bool, sample_rate: SampleRate) {
        let click = ActiveClick::new(self.settings.sound, accented, sample_rate);
        for slot in &mut self.active_clicks {
            if slot.is_none() {
                *slot = Some(click);
                return;
            }
        }
        // All slots are full: replace the one that has progressed furthest.
        let mut furthest_idx = 0;
        let mut furthest_frame = 0;
        for (i, slot) in self.active_clicks.iter().enumerate() {
            if let Some(c) = slot
                && c.frame > furthest_frame
            {
                furthest_frame = c.frame;
                furthest_idx = i;
            }
        }
        self.active_clicks[furthest_idx] = Some(click);
    }

    /// Renders count-in clicks into `block` and advances the count-in.
    /// Returns how many count-in frames it processed (up to the block's).
    pub fn render_count_in(&mut self, mut block: Block<'_>, sample_rate: SampleRate) -> usize {
        let frames = block.length.min(self.count_in_remaining);
        if frames == 0 {
            return 0;
        }
        let rate_f32 = sample_rate.as_f64() as f32;
        let bar_frames = self.count_in_bar_frames.max(1);
        let beats_per_bar = self.count_in_beats_per_bar.max(1);

        for i in 0..frames {
            let elapsed = self.count_in_total - self.count_in_remaining + i;
            let frame_in_bar = elapsed % bar_frames;

            // A beat's click starts exactly on its frame, the first accented.
            for beat in 0..beats_per_bar {
                let beat_frame = (beat * bar_frames) / beats_per_bar;
                if frame_in_bar == beat_frame {
                    self.trigger_click(beat == 0, sample_rate);
                    break;
                }
            }

            let sample = self.sum_active_clicks(rate_f32);
            block.add(i, sample);
        }

        self.count_in_remaining -= frames;
        frames
    }

    /// Renders active clicks over the block's frames, triggering a click on
    /// every beat the block covers.
    pub fn render_active(
        &mut self,
        mut block: Block<'_>,
        pos_frames: Frames,
        sample_rate: SampleRate,
        tempo_map: &TempoMap,
        time_signatures: &TimeSignatureMap,
    ) {
        if block.length == 0 {
            return;
        }
        let rate_f32 = sample_rate.as_f64() as f32;
        let start_f = pos_frames.get();
        let end_f = start_f + block.length as i64;

        // Up to 8 clicks can start within one block.
        let mut triggers = [(0usize, false); 8];
        let mut trigger_count = 0;

        if self.is_audible() {
            let start_ticks = tempo_map.frames_to_ticks(pos_frames, sample_rate);
            let end_ticks = tempo_map.frames_to_ticks(Frames(end_f), sample_rate);
            let (start_bar, _) = time_signatures.bar_at(start_ticks);
            let (end_bar, _) = time_signatures.bar_at(end_ticks);

            for bar in start_bar..=end_bar + 1 {
                let bar_start = time_signatures.bar_start(bar);
                let sig = time_signatures.signature_at(bar_start);
                let interval = self.settings.rhythm.effective_interval(sig);
                let bar_len = sig.bar_length();

                let mut k = 0i64;
                loop {
                    let tick_offset = Ticks(k * interval.get());
                    if tick_offset >= bar_len {
                        break;
                    }
                    let click_frame = tempo_map
                        .ticks_to_frames(bar_start + tick_offset, sample_rate)
                        .get();
                    if click_frame >= start_f && click_frame < end_f {
                        let sample_offset = (click_frame - start_f) as usize;
                        if trigger_count < triggers.len() {
                            triggers[trigger_count] = (sample_offset, k == 0);
                            trigger_count += 1;
                        }
                    }
                    k += 1;
                }
            }
        }

        if trigger_count > 1 {
            triggers[0..trigger_count].sort_unstable_by_key(|t| t.0);
        }

        let mut next_trig = 0;
        for i in 0..block.length {
            while next_trig < trigger_count && triggers[next_trig].0 == i {
                self.trigger_click(triggers[next_trig].1, sample_rate);
                next_trig += 1;
            }
            let sample = self.sum_active_clicks(rate_f32);
            if sample != 0.0 {
                block.add(i, sample);
            }
        }
    }

    /// Advances every active click by one sample and sums what they emit.
    #[inline]
    fn sum_active_clicks(&mut self, rate: f32) -> f32 {
        let mut sample = 0.0f32;
        for slot in &mut self.active_clicks {
            if let Some(click) = slot {
                match click.render_sample(rate) {
                    Some(s) => sample += s,
                    None => *slot = None,
                }
            }
        }
        sample
    }
}

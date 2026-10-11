//! The preview voice: the browser's audition path (WP-1.10; spec: browser
//! R11, R12).
//!
//! A preview plays a file once from its start, on a voice of its own beside
//! the song, at the preview volume. Until the Cue output exists (M3) the
//! voice mixes into the main output. Its two halves:
//!
//! - [`PreviewControl`], on the main thread: [`PreviewControl::play`] hands
//!   the voice decoded audio, wrapped for the engine's `rtgc` collector
//!   ([`GcHandle`]) so the audio thread can let go of it without freeing it;
//!   [`PreviewControl::stop`] stops it; [`PreviewControl::set_gain`] sets the
//!   volume; [`PreviewControl::status`] says what plays and how far it got.
//! - [`PreviewVoice`], on the audio thread: [`PreviewVoice::render`] adds the
//!   preview into the output channels a block at a time. It allocates
//!   nothing, frees nothing, takes no lock, and makes no system call: audio
//!   arrives on a lock-free queue (`rtrb`), and stopping, the volume, and the
//!   progress report are relaxed atomics.
//!
//! Every play and every stop takes a new generation number, and the voice
//! plays only the newest, so a stop overtakes a play still in the queue and
//! a quick run of selections plays only the last. A preview that stops before
//! its end fades out over [`FADE_FRAMES`] frames instead of clicking; one that
//! reaches its end stops there.
//!
//! The audio must already be at the device's sample rate: the browser decodes
//! and converts it on a background thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use rtrb::{Consumer, Producer, RingBuffer};

use crate::gc::GcHandle;
use crate::node::NodeData;

/// Audio a preview plays: planar channels of equal length at the device's
/// sample rate. Reading it must be real-time safe.
pub trait PreviewSamples: Send + Sync + 'static {
    /// How many channels it has, at least one.
    fn channel_count(&self) -> usize;
    /// Its length in frames.
    fn frames(&self) -> usize;
    /// One channel's samples, [`PreviewSamples::frames`] long.
    fn channel(&self, index: usize) -> &[f32];
}

/// Planar samples held in memory, the simplest [`PreviewSamples`].
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewBuffer {
    channels: Vec<Vec<f32>>,
    frames: usize,
}

impl PreviewBuffer {
    /// A buffer of these channels, cut to the shortest. `None` without
    /// channels.
    #[must_use]
    pub fn new(mut channels: Vec<Vec<f32>>) -> Option<Self> {
        let frames = channels.iter().map(Vec::len).min()?;
        for channel in &mut channels {
            channel.truncate(frames);
        }
        Some(Self { channels, frames })
    }
}

impl PreviewSamples for PreviewBuffer {
    fn channel_count(&self) -> usize {
        self.channels.len()
    }

    fn frames(&self) -> usize {
        self.frames
    }

    fn channel(&self, index: usize) -> &[f32] {
        &self.channels[index]
    }
}

/// How a preview stopped early fades out: over at most this many frames.
pub const FADE_FRAMES: usize = 64;

/// How many plays can wait for the audio thread. More are refused
/// ([`PreviewError::QueueFull`]), which happens only if the audio thread has
/// stalled.
pub const QUEUE_CAPACITY: usize = 16;

/// The highest preview gain: +6 dB.
pub const MAX_GAIN: f32 = 1.995_262_3;

/// What a preview's two halves share. It outlives a stream, so the volume
/// stays when the device is reopened.
#[derive(Debug)]
pub struct PreviewShared {
    /// Raised by every play and stop; the voice plays only this generation.
    generation: AtomicU64,
    /// The gain, as `f32` bits.
    gain: AtomicU32,
    /// The generation the voice is playing, or 0 for none.
    playing: AtomicU64,
    /// The newest generation the voice has started, so a preview shorter
    /// than the time between two looks still counts as played.
    started: AtomicU64,
    /// How far it has played, in frames.
    position: AtomicU64,
    /// Its length, in frames.
    length: AtomicU64,
}

impl Default for PreviewShared {
    fn default() -> Self {
        Self {
            generation: AtomicU64::new(0),
            gain: AtomicU32::new(1.0f32.to_bits()),
            playing: AtomicU64::new(0),
            started: AtomicU64::new(0),
            position: AtomicU64::new(0),
            length: AtomicU64::new(0),
        }
    }
}

impl PreviewShared {
    /// The preview gain: linear, 1 is unity.
    #[must_use]
    pub fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }

    /// Sets the preview gain, linear from 0 (silence) to [`MAX_GAIN`]
    /// (+6 dB); values outside are brought within it, and one that isn't a
    /// number is silence.
    pub fn set_gain(&self, gain: f32) {
        let gain = if gain.is_finite() {
            gain.clamp(0.0, MAX_GAIN)
        } else {
            0.0
        };
        self.gain.store(gain.to_bits(), Ordering::Relaxed);
    }
}

/// What a preview is doing, for the browser's preview strip.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PreviewStatus {
    /// The generation [`PreviewControl::play`] returned for what plays now,
    /// or `None` while nothing plays.
    pub playing: Option<u64>,
    /// The newest generation the voice has started: one at or below it
    /// that isn't `playing` has finished or been replaced.
    pub started: u64,
    /// How far it has played, in frames.
    pub position: u64,
    /// Its length, in frames.
    pub length: u64,
}

/// Why [`PreviewControl::play`] didn't start a preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewError {
    /// The audio thread hasn't taken the plays already queued.
    QueueFull,
    /// The audio has no channels or no frames.
    Empty,
}

impl std::fmt::Display for PreviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QueueFull => f.write_str("the audio thread isn't taking previews"),
            Self::Empty => f.write_str("the file holds no audio"),
        }
    }
}

impl std::error::Error for PreviewError {}

/// One queued play.
struct Message {
    generation: u64,
    data: NodeData,
}

/// A preview's main-thread half. See the [module docs](self).
pub struct PreviewControl {
    producer: Producer<Message>,
    gc: GcHandle,
    shared: Arc<PreviewShared>,
}

impl std::fmt::Debug for PreviewControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreviewControl")
            .field("shared", &self.shared)
            .finish_non_exhaustive()
    }
}

/// Makes a preview's two halves: the control for the main thread and the
/// voice for the audio thread, sharing `shared`, with audio wrapped for the
/// collector behind `gc`.
#[must_use]
pub fn preview_channel(gc: GcHandle, shared: Arc<PreviewShared>) -> (PreviewControl, PreviewVoice) {
    let (producer, consumer) = RingBuffer::new(QUEUE_CAPACITY);
    shared.playing.store(0, Ordering::Relaxed);
    let gain = shared.gain();
    (
        PreviewControl {
            producer,
            gc,
            shared: Arc::clone(&shared),
        },
        PreviewVoice {
            consumer,
            shared,
            current: None,
            fading: None,
            last_gain: gain,
        },
    )
}

impl PreviewControl {
    /// Plays `samples` from the start, replacing any preview playing or
    /// queued. Returns the play's generation, which
    /// [`PreviewControl::status`] reports while it plays. Wraps the audio for
    /// the collector, which allocates, so call it off the audio thread.
    ///
    /// # Errors
    ///
    /// [`PreviewError`] if the audio is empty or the queue is full; the
    /// preview playing before stops either way.
    pub fn play(&mut self, samples: Box<dyn PreviewSamples>) -> Result<u64, PreviewError> {
        let generation = self.shared.generation.fetch_add(1, Ordering::AcqRel) + 1;
        if samples.channel_count() == 0 || samples.frames() == 0 {
            return Err(PreviewError::Empty);
        }
        let data = self.gc.data(samples);
        self.producer
            .push(Message { generation, data })
            .map_err(|_| PreviewError::QueueFull)?;
        Ok(generation)
    }

    /// Stops the preview, fading it out, and drops any still queued.
    pub fn stop(&self) {
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// Sets the preview gain, linear from 0 (silence) to [`MAX_GAIN`]
    /// (+6 dB); values outside are brought within it. A playing preview
    /// ramps to it over the next block.
    pub fn set_gain(&self, gain: f32) {
        self.shared.set_gain(gain);
    }

    /// What plays now, as the audio thread last reported it.
    #[must_use]
    pub fn status(&self) -> PreviewStatus {
        status(&self.shared)
    }

    /// The state this control shares with its voice.
    #[must_use]
    pub fn shared(&self) -> &Arc<PreviewShared> {
        &self.shared
    }
}

/// What plays now, from the shared state.
#[must_use]
pub fn status(shared: &PreviewShared) -> PreviewStatus {
    let playing = shared.playing.load(Ordering::Relaxed);
    PreviewStatus {
        playing: (playing != 0).then_some(playing),
        started: shared.started.load(Ordering::Acquire),
        position: shared.position.load(Ordering::Relaxed),
        length: shared.length.load(Ordering::Relaxed),
    }
}

/// A preview being played.
struct Playing {
    generation: u64,
    data: NodeData,
    position: usize,
    /// While it fades out: the frames of fade left.
    fade: Option<usize>,
}

impl Playing {
    fn samples(&self) -> Option<&dyn PreviewSamples> {
        self.data
            .downcast_ref::<Box<dyn PreviewSamples>>()
            .map(AsRef::as_ref)
    }

    /// How many frames it plays in a block of `frames`.
    fn frames_in(&self, frames: usize) -> usize {
        let left = self
            .samples()
            .map_or(0, |s| s.frames().saturating_sub(self.position));
        let frames = frames.min(left);
        self.fade.map_or(frames, |fade| frames.min(fade))
    }

    /// Adds this block of it into one output channel.
    fn mix(&self, out_index: usize, out: &mut [f32], frames: usize, gains: (f32, f32)) {
        let Some(samples) = self.samples() else {
            return;
        };
        let count = samples.channel_count();
        if count == 0 {
            return;
        }
        let source = samples.channel(out_index.min(count - 1));
        let played = self.frames_in(frames).min(out.len());
        let Some(source) = source.get(self.position..self.position + played) else {
            return;
        };
        let (from, to) = gains;
        let step = if frames > 0 {
            (to - from) / frames as f32
        } else {
            0.0
        };
        for (i, (out, sample)) in out.iter_mut().zip(source).enumerate() {
            let mut gain = from + step * i as f32;
            if let Some(fade) = self.fade {
                gain *= fade.saturating_sub(i) as f32 / FADE_FRAMES as f32;
            }
            *out += sample * gain;
        }
    }

    /// Moves on by a block; returns whether it has finished.
    fn advance(&mut self, frames: usize) -> bool {
        let played = self.frames_in(frames);
        self.position += played;
        if let Some(fade) = &mut self.fade {
            *fade = fade.saturating_sub(played);
            if *fade == 0 {
                return true;
            }
        }
        self.samples()
            .is_none_or(|samples| self.position >= samples.frames())
    }
}

/// A preview's audio-thread half. See the [module docs](self).
pub struct PreviewVoice {
    consumer: Consumer<Message>,
    shared: Arc<PreviewShared>,
    current: Option<Playing>,
    /// A preview stopped early, fading out.
    fading: Option<Playing>,
    last_gain: f32,
}

impl std::fmt::Debug for PreviewVoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreviewVoice")
            .field("playing", &self.current.as_ref().map(|p| p.generation))
            .finish_non_exhaustive()
    }
}

impl PreviewVoice {
    /// Adds a block of `frames` frames of the preview into `outputs`, one
    /// slice per output channel; a mono file plays on every channel. Real-time
    /// safe: no allocation, free, lock, or system call. What it lets go of,
    /// the collector frees.
    pub fn render<'a>(&mut self, frames: usize, outputs: impl Iterator<Item = &'a mut [f32]>) {
        self.take_messages();
        let target = self.shared.gain();
        let gains = (self.last_gain, target);
        if self.current.is_some() || self.fading.is_some() {
            for (index, out) in outputs.enumerate() {
                if let Some(playing) = &self.fading {
                    playing.mix(index, out, frames, gains);
                }
                if let Some(playing) = &self.current {
                    playing.mix(index, out, frames, gains);
                }
            }
        }
        self.last_gain = target;
        // Dropping what finished only lowers its reference count: the
        // collector holds it and frees it on the drop thread.
        if self.fading.as_mut().is_some_and(|p| p.advance(frames)) {
            self.fading = None;
        }
        if self.current.as_mut().is_some_and(|p| p.advance(frames)) {
            self.current = None;
        }
        self.report();
    }

    /// Takes queued plays, keeping the newest, and starts or stops what the
    /// newest generation asks for.
    fn take_messages(&mut self) {
        let mut newest: Option<Message> = None;
        while let Ok(message) = self.consumer.pop() {
            if newest
                .as_ref()
                .is_none_or(|n| message.generation > n.generation)
            {
                newest = Some(message);
            }
        }
        // Loaded after the queue: a play is queued after its generation is
        // taken, so this is at least the newest message's.
        let wanted = self.shared.generation.load(Ordering::Acquire);
        if self
            .current
            .as_ref()
            .is_some_and(|p| p.generation != wanted)
            && let Some(mut old) = self.current.take()
        {
            old.fade = Some(FADE_FRAMES);
            // A third preview cut short while another still fades: the older
            // one goes straight away.
            self.fading = Some(old);
        }
        if let Some(message) = newest.filter(|m| m.generation == wanted) {
            self.shared
                .started
                .store(message.generation, Ordering::Release);
            self.current = Some(Playing {
                generation: message.generation,
                data: message.data,
                position: 0,
                fade: None,
            });
        }
    }

    fn report(&self) {
        let (generation, position, length) = self.current.as_ref().map_or((0, 0, 0), |p| {
            (
                p.generation,
                p.position as u64,
                p.samples().map_or(0, |s| s.frames() as u64),
            )
        });
        self.shared.position.store(position, Ordering::Relaxed);
        self.shared.length.store(length, Ordering::Relaxed);
        self.shared.playing.store(generation, Ordering::Release);
    }

    /// Whether anything plays or fades now.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.current.is_some() || self.fading.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gc::Collector;

    fn ramp(frames: usize, channels: usize) -> Box<dyn PreviewSamples> {
        let channels = (0..channels)
            .map(|c| {
                (0..frames)
                    .map(|i| (c as f32 + 1.0) * 0.001 * (i % 100) as f32)
                    .collect()
            })
            .collect();
        Box::new(PreviewBuffer::new(channels).unwrap())
    }

    fn block(voice: &mut PreviewVoice, out: &mut [Vec<f32>]) {
        for channel in out.iter_mut() {
            channel.fill(0.0);
        }
        let frames = out[0].len();
        voice.render(frames, out.iter_mut().map(Vec::as_mut_slice));
    }

    // Spec: browser R12.
    #[test]
    fn the_voice_plays_replaces_and_stops_without_the_heap() {
        let mut collector = Collector::new();
        let shared = Arc::new(PreviewShared::default());
        let (mut control, mut voice) = preview_channel(collector.handle(), Arc::clone(&shared));
        let mut out = vec![vec![0.0f32; 128]; 2];

        // Nothing queued: silence, nothing reported.
        bl_rt::rt_test!({ block(&mut voice, &mut out) });
        assert!(out.iter().flatten().all(|s| *s == 0.0));
        assert_eq!(control.status().playing, None);

        // A mono file plays on both channels, at unity.
        let first = control.play(ramp(1000, 1)).unwrap();
        bl_rt::rt_test!({ block(&mut voice, &mut out) });
        assert_eq!(control.status().playing, Some(first));
        assert_eq!(control.status().position, 128);
        assert_eq!(control.status().length, 1000);
        assert!((out[0][5] - 0.005).abs() < 1e-6);
        assert_eq!(out[0], out[1]);

        // A second play replaces it; the first fades out alongside.
        let second = control.play(ramp(300, 2)).unwrap();
        bl_rt::rt_test!({ block(&mut voice, &mut out) });
        assert_eq!(control.status().playing, Some(second));
        // The second's right channel is twice its left.
        bl_rt::rt_test!({ block(&mut voice, &mut out) });
        assert!((out[1][100] - 2.0 * out[0][100]).abs() < 1e-6);
        // It ends by itself at its last frame.
        bl_rt::rt_test!({
            for _ in 0..4 {
                block(&mut voice, &mut out);
            }
        });
        assert_eq!(control.status().playing, None);
        assert!(out.iter().flatten().all(|s| *s == 0.0));

        // A stop overtakes a play still queued.
        control.play(ramp(1000, 1)).unwrap();
        control.stop();
        bl_rt::rt_test!({ block(&mut voice, &mut out) });
        assert_eq!(control.status().playing, None);
        assert!(out.iter().flatten().all(|s| *s == 0.0));

        // What the voice let go of waits for the collector.
        assert!(collector.held() >= 3);
        collector.collect();
        assert_eq!(collector.held(), 0);
        drop(voice);
    }

    // Spec: browser R12.
    #[test]
    fn a_stop_fades_out_within_64_frames() {
        let collector = Collector::new();
        let shared = Arc::new(PreviewShared::default());
        let (mut control, mut voice) = preview_channel(collector.handle(), shared);
        let ones: Box<dyn PreviewSamples> =
            Box::new(PreviewBuffer::new(vec![vec![1.0; 10_000]]).unwrap());
        control.play(ones).unwrap();
        let mut out = vec![vec![0.0f32; 32]];
        block(&mut voice, &mut out);
        assert!(out[0].iter().all(|s| (*s - 1.0).abs() < 1e-6));
        control.stop();
        let mut tail = Vec::new();
        for _ in 0..4 {
            block(&mut voice, &mut out);
            tail.extend_from_slice(&out[0]);
        }
        // Falling from full level to silence over 64 frames, then silent.
        assert!(tail[0] > 0.95 && tail[0] <= 1.0, "{}", tail[0]);
        assert!(tail.windows(2).take(64).all(|w| w[1] <= w[0]));
        assert!(tail[63] < 0.05);
        assert!(tail[64..].iter().all(|s| *s == 0.0));
        assert!(!voice.is_active());
    }

    #[test]
    fn the_gain_ramps_and_reaches_silence() {
        let collector = Collector::new();
        let shared = Arc::new(PreviewShared::default());
        let (mut control, mut voice) = preview_channel(collector.handle(), shared);
        control
            .play(Box::new(
                PreviewBuffer::new(vec![vec![1.0; 10_000]]).unwrap(),
            ))
            .unwrap();
        let mut out = vec![vec![0.0f32; 64]];
        block(&mut voice, &mut out);
        control.set_gain(0.0);
        block(&mut voice, &mut out);
        // A ramp from unity down, no step.
        assert!((out[0][0] - 1.0).abs() < 1e-6);
        assert!(out[0][63] < 0.05);
        block(&mut voice, &mut out);
        assert!(out[0].iter().all(|s| *s == 0.0), "0 is silence (rule G1)");
        control.set_gain(f32::NAN);
        assert_eq!(control.shared().gain(), 0.0);
        control.set_gain(10.0);
        assert_eq!(control.shared().gain(), MAX_GAIN);
        assert_eq!(
            control.play(Box::new(PreviewBuffer::new(vec![vec![]]).unwrap())),
            Err(PreviewError::Empty)
        );
    }
}

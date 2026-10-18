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

use bl_core::time::{Frames, SampleRate, TempoMap, Ticks, TimeSignatureMap};
use bl_model::LaunchSettings;
pub use bl_stretch::{Marker, WarpMap};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::gc::GcHandle;
use crate::node::NodeData;

/// Ticks a tempo of 1 BPM covers in a second.
const TICKS_PER_SECOND_PER_BPM: f64 = 705_600.0 / 60.0;

/// A warped preview to play (WP-4.18; spec: browser R19, R20, R23).
pub struct WarpedPreview {
    /// Audio samples in memory.
    pub samples: Box<dyn PreviewSamples>,
    /// The file's warp map.
    pub map: WarpMap,
    /// The start beat in ticks.
    pub start: f64,
    /// The end beat in ticks.
    pub end: f64,
    /// Where the loop starts in ticks.
    pub loop_start: f64,
    /// Where the loop ends in ticks.
    pub loop_end: f64,
    /// File sample rate in frames per second.
    pub sample_rate: f64,
    /// Set tempo in BPM when transport is stopped.
    pub tempo: f64,
}

impl std::fmt::Debug for WarpedPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WarpedPreview")
            .field("start", &self.start)
            .field("end", &self.end)
            .field("loop_start", &self.loop_start)
            .field("loop_end", &self.loop_end)
            .field("tempo", &self.tempo)
            .finish_non_exhaustive()
    }
}

/// Transport state for a warped preview (WP-4.18; spec: browser R20, R22).
#[derive(Clone, Copy, Debug)]
pub struct PreviewTransport<'a> {
    /// Whether the song is playing.
    pub playing: bool,
    /// Song position of the block's first frame.
    pub song_position: Frames,
    /// The running tempo map.
    pub tempo_map: &'a TempoMap,
    /// The running time signature map.
    pub time_signatures: &'a TimeSignatureMap,
    /// Loop region in song frames, if looping.
    pub loop_frames: Option<(Frames, Frames)>,
    /// Device sample rate.
    pub sample_rate: SampleRate,
    /// Global launch settings for launch quantisation.
    pub launch: LaunchSettings,
}

/// The file's sample at `position`, through a Catmull-Rom cubic, silence
/// outside the file.
fn read_sample(samples: &dyn PreviewSamples, channel: usize, position: f64) -> f32 {
    let count = samples.channel_count();
    let channel_data = samples.channel(channel.min(count - 1));
    let at = |i: i64| {
        usize::try_from(i)
            .ok()
            .and_then(|idx| channel_data.get(idx))
            .copied()
            .map_or(0.0, f64::from)
    };
    let floor = position.floor();
    let t = position - floor;
    let i = floor as i64;
    let (a, b, c, d) = (at(i - 1), at(i), at(i + 1), at(i + 2));
    let (m1, m2) = (0.5 * (c - a), 0.5 * (d - b));
    let sample = (2.0 * t.powi(3) - 3.0 * t * t + 1.0) * b
        + (t.powi(3) - 2.0 * t * t + t) * m1
        + (-2.0 * t.powi(3) + 3.0 * t * t) * c
        + (t.powi(3) - t * t) * m2;
    sample as f32
}

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

/// What a queued preview message can ask for.
enum MessageKind {
    Play(NodeData),
    Scrub(f64),
}

/// One queued preview message.
struct Message {
    generation: u64,
    kind: MessageKind,
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
            .push(Message {
                generation,
                kind: MessageKind::Play(data),
            })
            .map_err(|_| PreviewError::QueueFull)?;
        Ok(generation)
    }

    /// Plays `warped` from the start, replacing any preview playing or
    /// queued. Wraps the audio for the collector, which allocates, so call it
    /// off the audio thread (WP-4.18; spec: browser R19, R23).
    ///
    /// # Errors
    ///
    /// [`PreviewError`] if the audio is empty or the queue is full.
    pub fn play_warped(&mut self, warped: Box<WarpedPreview>) -> Result<u64, PreviewError> {
        let generation = self.shared.generation.fetch_add(1, Ordering::AcqRel) + 1;
        if warped.samples.channel_count() == 0 || warped.samples.frames() == 0 {
            return Err(PreviewError::Empty);
        }
        let data = self.gc.data(warped);
        self.producer
            .push(Message {
                generation,
                kind: MessageKind::Play(data),
            })
            .map_err(|_| PreviewError::QueueFull)?;
        Ok(generation)
    }

    /// Scrubs a playing warped preview to `beat` in ticks (WP-4.18; spec: browser R22).
    ///
    /// # Errors
    ///
    /// [`PreviewError`] if the queue is full.
    pub fn scrub(&mut self, beat: f64) -> Result<(), PreviewError> {
        let generation = self.shared.generation.load(Ordering::Acquire);
        self.producer
            .push(Message {
                generation,
                kind: MessageKind::Scrub(beat),
            })
            .map_err(|_| PreviewError::QueueFull)?;
        Ok(())
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

#[derive(Clone, Copy, Debug)]
enum PlayingContent {
    Raw,
    Warped {
        started: bool,
        start_frame: Option<Frames>,
        beat_offset: f64,
        elapsed_ticks: f64,
        pending_scrub: Option<(f64, Option<Ticks>)>,
    },
}

/// A preview being played.
struct Playing {
    generation: u64,
    data: NodeData,
    position: usize,
    /// While it fades out: the frames of fade left.
    fade: Option<usize>,
    content: PlayingContent,
}

impl Playing {
    fn new(generation: u64, data: NodeData, transport: Option<PreviewTransport<'_>>) -> Self {
        let content = if let Some(warped) = data.downcast_ref::<Box<WarpedPreview>>() {
            let (started, start_frame) = match transport {
                Some(t) if t.playing => {
                    let song_tick = Ticks(
                        t.tempo_map
                            .seconds_to_ticks_exact(
                                t.song_position.get() as f64 / t.sample_rate.as_f64(),
                            )
                            .round() as i64,
                    );
                    let (bar_idx, offset) = t.time_signatures.bar_at(song_tick);
                    if offset == Ticks::ZERO {
                        (true, Some(t.song_position))
                    } else {
                        let next_bar_tick = t.time_signatures.bar_start(bar_idx + 1);
                        let next_bar_frame =
                            t.tempo_map.ticks_to_frames(next_bar_tick, t.sample_rate);
                        (false, Some(next_bar_frame))
                    }
                }
                _ => (true, None),
            };
            PlayingContent::Warped {
                started,
                start_frame,
                beat_offset: warped.start - warped.loop_start,
                elapsed_ticks: 0.0,
                pending_scrub: None,
            }
        } else {
            PlayingContent::Raw
        };
        Self {
            generation,
            data,
            position: 0,
            fade: None,
            content,
        }
    }

    fn samples(&self) -> Option<&dyn PreviewSamples> {
        if let Some(warped) = self.data.downcast_ref::<Box<WarpedPreview>>() {
            Some(warped.samples.as_ref())
        } else {
            self.data
                .downcast_ref::<Box<dyn PreviewSamples>>()
                .map(AsRef::as_ref)
        }
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
    fn mix(
        &self,
        out_index: usize,
        out: &mut [f32],
        frames: usize,
        gains: (f32, f32),
        transport: Option<PreviewTransport<'_>>,
    ) {
        let Some(samples) = self.samples() else {
            return;
        };
        let count = samples.channel_count();
        if count == 0 {
            return;
        }
        let (from, to) = gains;
        let step = if frames > 0 {
            (to - from) / frames as f32
        } else {
            0.0
        };

        match self.content {
            PlayingContent::Raw => {
                let source = samples.channel(out_index.min(count - 1));
                let played = self.frames_in(frames).min(out.len());
                let Some(source) = source.get(self.position..self.position + played) else {
                    return;
                };
                for (i, (out, sample)) in out.iter_mut().zip(source).enumerate() {
                    let mut gain = from + step * i as f32;
                    if let Some(fade) = self.fade {
                        gain *= fade.saturating_sub(i) as f32 / FADE_FRAMES as f32;
                    }
                    *out += sample * gain;
                }
            }
            PlayingContent::Warped {
                mut started,
                start_frame,
                mut beat_offset,
                mut elapsed_ticks,
                mut pending_scrub,
            } => {
                let Some(warped) = self.data.downcast_ref::<Box<WarpedPreview>>() else {
                    return;
                };
                let loop_len = (warped.loop_end - warped.loop_start).max(1.0);
                for i in 0..frames.min(out.len()) {
                    let (can_play, delta_ticks) = match transport {
                        Some(t) if t.playing => {
                            let song_frame = t.song_position + Frames(i as i64);
                            let song_tick_f64 = t.tempo_map.seconds_to_ticks_exact(
                                song_frame.get() as f64 / t.sample_rate.as_f64(),
                            );
                            let song_tick = Ticks(song_tick_f64.round() as i64);
                            if !started {
                                if let Some(target) = start_frame {
                                    if song_frame < target {
                                        (false, 0.0)
                                    } else {
                                        started = true;
                                        let t0 = song_tick_f64;
                                        let t1 = t.tempo_map.seconds_to_ticks_exact(
                                            (song_frame + Frames(1)).get() as f64
                                                / t.sample_rate.as_f64(),
                                        );
                                        (true, (t1 - t0).max(0.0))
                                    }
                                } else {
                                    started = true;
                                    let t0 = song_tick_f64;
                                    let t1 = t.tempo_map.seconds_to_ticks_exact(
                                        (song_frame + Frames(1)).get() as f64
                                            / t.sample_rate.as_f64(),
                                    );
                                    (true, (t1 - t0).max(0.0))
                                }
                            } else {
                                if let Some((scrub_beat, target_tick)) = pending_scrub
                                    && target_tick.is_none_or(|target| song_tick >= target)
                                {
                                    beat_offset = (scrub_beat - warped.loop_start) - elapsed_ticks;
                                    pending_scrub = None;
                                }
                                let t0 = song_tick_f64;
                                let t1 = t.tempo_map.seconds_to_ticks_exact(
                                    (song_frame + Frames(1)).get() as f64 / t.sample_rate.as_f64(),
                                );
                                (true, (t1 - t0).max(0.0))
                            }
                        }
                        Some(t) => {
                            let dt = (1.0 / t.sample_rate.as_f64())
                                * warped.tempo
                                * TICKS_PER_SECOND_PER_BPM;
                            (true, dt)
                        }
                        None => {
                            let dt = (1.0 / warped.sample_rate)
                                * warped.tempo
                                * TICKS_PER_SECOND_PER_BPM;
                            (true, dt)
                        }
                    };
                    if can_play {
                        let beat =
                            warped.loop_start + (beat_offset + elapsed_ticks).rem_euclid(loop_len);
                        let file_time = warped.map.beat_to_time(beat);
                        let file_pos = file_time * warped.sample_rate;
                        let sample = read_sample(samples, out_index, file_pos);
                        let mut gain = from + step * i as f32;
                        if let Some(fade) = self.fade {
                            gain *= fade.saturating_sub(i) as f32 / FADE_FRAMES as f32;
                        }
                        out[i] += sample * gain;
                        elapsed_ticks += delta_ticks;
                    }
                }
            }
        }
    }

    /// Moves on by a block; returns whether it has finished.
    fn advance(&mut self, frames: usize, transport: Option<PreviewTransport<'_>>) -> bool {
        match &mut self.content {
            PlayingContent::Raw => {
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
            PlayingContent::Warped {
                started,
                start_frame,
                beat_offset,
                elapsed_ticks,
                pending_scrub,
            } => {
                let Some(warped) = self.data.downcast_ref::<Box<WarpedPreview>>() else {
                    return true;
                };
                for i in 0..frames {
                    match transport {
                        Some(t) if t.playing => {
                            let song_frame = t.song_position + Frames(i as i64);
                            let song_tick_f64 = t.tempo_map.seconds_to_ticks_exact(
                                song_frame.get() as f64 / t.sample_rate.as_f64(),
                            );
                            let song_tick = Ticks(song_tick_f64.round() as i64);
                            if !*started {
                                if let Some(target) = *start_frame {
                                    if song_frame < target {
                                        continue;
                                    }
                                    *started = true;
                                } else {
                                    *started = true;
                                }
                            }
                            if let Some((scrub_beat, target_tick)) = *pending_scrub
                                && target_tick.is_none_or(|target| song_tick >= target)
                            {
                                *beat_offset = (scrub_beat - warped.loop_start) - *elapsed_ticks;
                                *pending_scrub = None;
                            }
                            let t0 = song_tick_f64;
                            let t1 = t.tempo_map.seconds_to_ticks_exact(
                                (song_frame + Frames(1)).get() as f64 / t.sample_rate.as_f64(),
                            );
                            *elapsed_ticks += (t1 - t0).max(0.0);
                        }
                        Some(t) => {
                            *started = true;
                            let dt = (1.0 / t.sample_rate.as_f64())
                                * warped.tempo
                                * TICKS_PER_SECOND_PER_BPM;
                            *elapsed_ticks += dt;
                        }
                        None => {
                            *started = true;
                            let dt = (1.0 / warped.sample_rate)
                                * warped.tempo
                                * TICKS_PER_SECOND_PER_BPM;
                            *elapsed_ticks += dt;
                        }
                    }
                }
                if let Some(fade) = &mut self.fade {
                    *fade = fade.saturating_sub(frames);
                    if *fade == 0 {
                        return true;
                    }
                }
                false
            }
        }
    }

    fn scrub(&mut self, target_beat: f64, transport: Option<PreviewTransport<'_>>) {
        let PlayingContent::Warped {
            beat_offset,
            elapsed_ticks,
            pending_scrub,
            ..
        } = &mut self.content
        else {
            return;
        };
        let Some(warped) = self.data.downcast_ref::<Box<WarpedPreview>>() else {
            return;
        };
        if let Some(t) = transport
            && t.playing
            && t.launch.enabled
        {
            let song_tick = Ticks(
                t.tempo_map
                    .seconds_to_ticks_exact(t.song_position.get() as f64 / t.sample_rate.as_f64())
                    .round() as i64,
            );
            let target_line = t.launch.next_line(song_tick, t.time_signatures);
            *pending_scrub = Some((target_beat, Some(target_line)));
        } else {
            *beat_offset = (target_beat - warped.loop_start) - *elapsed_ticks;
            *pending_scrub = None;
        }
    }

    fn position(&self) -> u64 {
        match &self.content {
            PlayingContent::Raw => self.position as u64,
            PlayingContent::Warped {
                beat_offset,
                elapsed_ticks,
                ..
            } => {
                if let Some(warped) = self.data.downcast_ref::<Box<WarpedPreview>>() {
                    let loop_len = (warped.loop_end - warped.loop_start).max(1.0);
                    let current_beat =
                        warped.loop_start + (beat_offset + elapsed_ticks).rem_euclid(loop_len);
                    let file_time = warped.map.beat_to_time(current_beat);
                    (file_time * warped.sample_rate).max(0.0).round() as u64
                } else {
                    0
                }
            }
        }
    }

    fn length(&self) -> u64 {
        self.samples().map_or(0, |s| s.frames() as u64)
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
        self.render_with_transport(frames, outputs, None);
    }

    /// Adds a block of `frames` frames into `outputs`, syncing with
    /// `transport` if given (WP-4.18; spec: browser R20, R23).
    pub fn render_with_transport<'a>(
        &mut self,
        frames: usize,
        outputs: impl Iterator<Item = &'a mut [f32]>,
        transport: Option<PreviewTransport<'_>>,
    ) {
        self.take_messages(transport);
        let target = self.shared.gain();
        let gains = (self.last_gain, target);
        if self.current.is_some() || self.fading.is_some() {
            for (index, out) in outputs.enumerate() {
                if let Some(playing) = &self.fading {
                    playing.mix(index, out, frames, gains, transport);
                }
                if let Some(playing) = &self.current {
                    playing.mix(index, out, frames, gains, transport);
                }
            }
        }
        self.last_gain = target;
        // Dropping what finished only lowers its reference count: the
        // collector holds it and frees it on the drop thread.
        if self
            .fading
            .as_mut()
            .is_some_and(|p| p.advance(frames, transport))
        {
            self.fading = None;
        }
        if self
            .current
            .as_mut()
            .is_some_and(|p| p.advance(frames, transport))
        {
            self.current = None;
        }
        self.report();
    }

    /// Takes queued plays, keeping the newest, and starts or stops what the
    /// newest generation asks for.
    fn take_messages(&mut self, transport: Option<PreviewTransport<'_>>) {
        let mut newest_play: Option<Message> = None;
        let mut newest_scrub: Option<f64> = None;
        while let Ok(message) = self.consumer.pop() {
            match message.kind {
                MessageKind::Play(_) => {
                    if newest_play
                        .as_ref()
                        .is_none_or(|n| message.generation > n.generation)
                    {
                        newest_play = Some(message);
                    }
                }
                MessageKind::Scrub(beat) => {
                    newest_scrub = Some(beat);
                }
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
        if let Some(message) = newest_play.filter(|m| m.generation == wanted) {
            let MessageKind::Play(data) = message.kind else {
                unreachable!()
            };
            self.shared
                .started
                .store(message.generation, Ordering::Release);
            self.current = Some(Playing::new(message.generation, data, transport));
        }
        if let Some(scrub_beat) = newest_scrub
            && let Some(playing) = &mut self.current
        {
            playing.scrub(scrub_beat, transport);
        }
    }

    fn report(&self) {
        let (generation, position, length) = self
            .current
            .as_ref()
            .map_or((0, 0, 0), |p| (p.generation, p.position(), p.length()));
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

    fn warped_fixture(frames: usize, bpm: f64) -> Box<WarpedPreview> {
        let channels = vec![vec![0.5f32; frames]];
        let samples = Box::new(PreviewBuffer::new(channels).unwrap());
        let beats = (frames as f64 / 48_000.0 * bpm * (705_600.0 / 60.0)).round();
        let map = WarpMap::new(
            vec![
                bl_stretch::Marker { time: 0.0, beat: 0 },
                bl_stretch::Marker {
                    time: frames as f64 / 48_000.0,
                    beat: beats as i64,
                },
            ],
            bpm,
        )
        .unwrap();
        Box::new(WarpedPreview {
            samples,
            map,
            start: 0.0,
            end: beats,
            loop_start: 0.0,
            loop_end: beats,
            sample_rate: 48_000.0,
            tempo: bpm,
        })
    }

    // Spec: browser R20.
    #[test]
    fn a_warped_preview_starts_on_the_next_bar_and_keeps_to_the_beat() {
        let collector = Collector::new();
        let shared = Arc::new(PreviewShared::default());
        let (mut control, mut voice) = preview_channel(collector.handle(), shared);
        let tempo_map = TempoMap::constant(bl_core::time::Bpm::new(120.0).unwrap());
        let time_signatures = TimeSignatureMap::constant(bl_core::time::TimeSignature::FOUR_FOUR);
        let sample_rate = SampleRate::new(48_000).unwrap();
        let launch = LaunchSettings::default();

        let warped = warped_fixture(192_000, 120.0);
        control.play_warped(warped).unwrap();

        // 1 bar at 120 BPM, 48 kHz is 96,000 frames.
        // If song is currently at frame 10,000, it should wait until frame 96,000.
        let mut out = [vec![0.0f32; 128]];
        let transport_wait = PreviewTransport {
            playing: true,
            song_position: Frames(10_000),
            tempo_map: &tempo_map,
            time_signatures: &time_signatures,
            loop_frames: None,
            sample_rate,
            launch,
        };
        voice.render_with_transport(
            128,
            out.iter_mut().map(Vec::as_mut_slice),
            Some(transport_wait),
        );
        assert!(out[0].iter().all(|s| *s == 0.0), "silent before next bar");

        // Now render at the bar line (frame 96,000).
        let transport_start = PreviewTransport {
            playing: true,
            song_position: Frames(96_000),
            tempo_map: &tempo_map,
            time_signatures: &time_signatures,
            loop_frames: None,
            sample_rate,
            launch,
        };
        voice.render_with_transport(
            128,
            out.iter_mut().map(Vec::as_mut_slice),
            Some(transport_start),
        );
        assert!(out[0].iter().any(|s| *s > 0.0), "starts on bar line");
        assert_eq!(control.status().position, 128);

        // Scrubbing with quantisation moves on next quantisation step.
        control.scrub(705_600.0).unwrap();
        let transport_mid = PreviewTransport {
            playing: true,
            song_position: Frames(96_128),
            tempo_map: &tempo_map,
            time_signatures: &time_signatures,
            loop_frames: None,
            sample_rate,
            launch,
        };
        voice.render_with_transport(
            128,
            out.iter_mut().map(Vec::as_mut_slice),
            Some(transport_mid),
        );
        // Still near the start before quantisation step.
        assert!(control.status().position < 10_000);

        // On the next bar line (frame 192,000), the scrub takes effect.
        let transport_quant = PreviewTransport {
            playing: true,
            song_position: Frames(192_000),
            tempo_map: &tempo_map,
            time_signatures: &time_signatures,
            loop_frames: None,
            sample_rate,
            launch,
        };
        voice.render_with_transport(
            128,
            out.iter_mut().map(Vec::as_mut_slice),
            Some(transport_quant),
        );
        // Position jumped forward to 1 beat (24,000 frames) plus block offset!
        assert!(control.status().position >= 24_000);
    }

    // Spec: browser R23.
    #[test]
    fn a_warped_preview_plays_loops_and_stops_without_the_heap() {
        let mut collector = Collector::new();
        let shared = Arc::new(PreviewShared::default());
        let (mut control, mut voice) = preview_channel(collector.handle(), shared);
        let tempo_map = TempoMap::constant(bl_core::time::Bpm::new(120.0).unwrap());
        let time_signatures = TimeSignatureMap::constant(bl_core::time::TimeSignature::FOUR_FOUR);
        let sample_rate = SampleRate::new(48_000).unwrap();
        let launch = LaunchSettings::default();

        let warped = warped_fixture(48_000, 120.0);
        control.play_warped(warped).unwrap();

        let mut out = vec![vec![0.0f32; 128]; 2];
        let transport = PreviewTransport {
            playing: false,
            song_position: Frames(0),
            tempo_map: &tempo_map,
            time_signatures: &time_signatures,
            loop_frames: None,
            sample_rate,
            launch,
        };

        // Render, loop, scrub, stop under the allocation trap.
        bl_rt::rt_test!({
            for _ in 0..10 {
                voice.render_with_transport(
                    128,
                    out.iter_mut().map(Vec::as_mut_slice),
                    Some(transport),
                );
            }
        });
        assert!(out[0].iter().any(|s| *s != 0.0));

        control.scrub(1000.0).unwrap();
        bl_rt::rt_test!({
            voice.render_with_transport(
                128,
                out.iter_mut().map(Vec::as_mut_slice),
                Some(transport),
            );
        });

        control.stop();
        bl_rt::rt_test!({
            for _ in 0..4 {
                voice.render_with_transport(
                    128,
                    out.iter_mut().map(Vec::as_mut_slice),
                    Some(transport),
                );
            }
        });
        assert!(!voice.is_active());
        collector.collect();
    }
}

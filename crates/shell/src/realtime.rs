//! Real-time export with external hardware (WP-5.35; the external devices
//! spec, R22 to R26).
//!
//! An export whose output runs through external hardware renders in real
//! time: the running engine plays the range, the hardware plays its part,
//! and the audio thread copies each rendered output into a lock-free ring
//! at the point an offline render takes it from. A writer thread drains the
//! rings into the files, so the file holds exactly the range's frames.
//!
//! The render waits first, so the hardware's last sound dies away, and a
//! dropout found while it runs starts it again, unless Auto-Restart is
//! off, when the dropouts are listed instead.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use bl_core::time::{Frames, SampleRate, Ticks};
use bl_engine::{RenderOutput, RenderPlan, TransportCommand};
use bl_model::{Device, Document, TrackId};
use rtrb::{Consumer, PopError, RingBuffer};

use crate::audio::AudioHost;
use crate::export::{self, Downsampler, ExportError, Job, Length, Report};

/// How long the render waits before it starts, so the hardware's last sound
/// dies away (R23).
pub const DEFAULT_WAIT: Duration = Duration::from_secs(10);

/// Frames the ring between the audio thread and the writer holds, about
/// 2.7 seconds of stereo at 48 kHz, so a writer that falls behind loses
/// nothing.
const RING_FRAMES: usize = 1 << 17;

/// How long the render waits between checks of its progress and of the
/// device's counters.
const POLL: Duration = Duration::from_millis(2);

/// The channels a render holds: the engine's own.
const CHANNELS: usize = 2;

/// What a real-time render did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    /// How many attempts it took.
    pub attempts: u32,
    /// The dropouts it found, each with the song position it happened at.
    pub dropouts: Vec<Dropout>,
}

/// A dropout the render found (R25).
#[derive(Clone, Debug, PartialEq)]
pub struct Dropout {
    /// The song position it happened at, in ticks from 1.1.1.
    pub position: Ticks,
    /// What it was: the backend's report, or a ring too full.
    pub kind: String,
}

/// What a real-time render is doing now (R23): the app's dialog shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Nothing is rendering.
    Idle,
    /// Waiting for the hardware's sound to die away.
    Waiting,
    /// Rendering the range.
    Rendering,
}

/// What a real-time render is doing now (R23): the app's dialog shows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Status {
    /// What the render is doing.
    pub phase: Phase,
    /// The seconds left before the render starts, while it waits.
    pub seconds: f64,
    /// How much of the range is written, from 0 to 1.
    pub progress: f64,
    /// How many times the render has started.
    pub attempts: u32,
    /// Whether the render starts again when a dropout spoils it.
    pub auto_restart: bool,
}

/// The state of a real-time render, shared with the app (R23): the dialog
/// shows the phase, and its buttons set the flags here. The shell holds
/// one, so the app and the export see the same state.
#[derive(Clone)]
pub struct Session {
    state: Arc<Mutex<State>>,
    skip: Arc<AtomicBool>,
    restart: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    auto_restart: Arc<AtomicBool>,
}

/// The render's state, under the lock.
#[derive(Clone, Copy, Debug)]
struct State {
    phase: Phase,
    /// When the waiting period is over.
    until: Option<Instant>,
    /// How long the waiting period is.
    period: Duration,
    progress: f64,
    attempts: u32,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                phase: Phase::Idle,
                until: None,
                period: Duration::from_secs(10),
                progress: 0.0,
                attempts: 0,
            })),
            skip: Arc::new(AtomicBool::new(false)),
            restart: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
            auto_restart: Arc::new(AtomicBool::new(true)),
        }
    }
}

impl Session {
    /// What the render is doing now.
    #[must_use]
    pub fn status(&self) -> Status {
        let state = self.state.lock().map_or(
            State {
                phase: Phase::Idle,
                until: None,
                period: Duration::from_secs(10),
                progress: 0.0,
                attempts: 0,
            },
            |state| *state,
        );
        let seconds = state.until.map_or(0.0, |until| {
            until
                .saturating_duration_since(Instant::now())
                .as_secs_f64()
        });
        Status {
            phase: state.phase,
            seconds,
            progress: state.progress,
            attempts: state.attempts,
            auto_restart: self.auto_restart(),
        }
    }

    /// Starts the render at once, without waiting (R23).
    pub fn skip(&self) {
        self.skip.store(true, Ordering::Relaxed);
        // The wait is over: the dialog shows no time left.
        self.with_state(|state| {
            if state.phase == Phase::Waiting {
                state.until = Some(Instant::now());
            }
        });
    }

    /// Starts the waiting period again (R23).
    pub fn restart(&self) {
        self.restart.store(true, Ordering::Relaxed);
        // The count starts again, so the dialog shows the whole period.
        self.with_state(|state| {
            if state.phase == Phase::Waiting {
                state.until = Some(Instant::now() + state.period);
            }
        });
    }

    /// Stops the render; its partial file is deleted (R23).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Whether the render was cancelled.
    #[must_use]
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// The flag the export watches, so a Cancel from another thread stops
    /// it.
    #[must_use]
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    /// Whether the render starts again after a dropout spoils it (R23).
    #[must_use]
    pub fn auto_restart(&self) -> bool {
        self.auto_restart.load(Ordering::Relaxed)
    }

    /// Turns Auto-Restart on Drop-Outs on or off (R23).
    pub fn set_auto_restart(&self, on: bool) {
        self.auto_restart.store(on, Ordering::Relaxed);
    }

    /// Forgets what a render was asked to do, so a new one starts afresh: a
    /// cancel from an earlier render doesn't stop this one.
    pub fn reset(&self) {
        self.skip.store(false, Ordering::Relaxed);
        self.restart.store(false, Ordering::Relaxed);
        self.cancel.store(false, Ordering::Relaxed);
        self.set_state(Phase::Idle, 0.0, 0);
    }

    /// Changes the state, under the lock.
    fn with_state(&self, change: impl FnOnce(&mut State)) {
        if let Ok(mut state) = self.state.lock() {
            change(&mut state);
        }
    }

    /// Says what the render is doing now.
    fn set_state(&self, phase: Phase, progress: f64, attempts: u32) {
        self.with_state(|state| {
            state.phase = phase;
            state.progress = progress;
            state.attempts = attempts;
            if phase != Phase::Waiting {
                state.until = None;
            }
        });
    }

    /// Starts the waiting period, so the hardware's last sound dies away
    /// (R23). The dialog shows it.
    pub fn begin_wait(&self, period: Duration) {
        self.with_state(|state| {
            state.phase = Phase::Waiting;
            state.period = period;
            state.until = Some(Instant::now() + period);
        });
    }

    /// Waits `period` before the render starts, so the hardware's last sound
    /// dies away (R23). A Restart starts the count again, a Skip starts the
    /// render at once, and a Cancel stops it. Returns whether the render
    /// should go on.
    pub fn wait(&self, period: Duration) -> bool {
        self.begin_wait(period);
        loop {
            if self.cancelled() {
                return false;
            }
            if self.skip.swap(false, Ordering::Relaxed) {
                break;
            }
            if self.restart.swap(false, Ordering::Relaxed) {
                self.begin_wait(period);
                continue;
            }
            let until = self
                .state
                .lock()
                .ok()
                .and_then(|state| state.until)
                .unwrap_or_else(Instant::now);
            let now = Instant::now();
            if now >= until {
                break;
            }
            thread::sleep(POLL.min(until - now));
        }
        !self.cancelled()
    }
}

/// Whether a rendered output's path runs through external hardware that
/// reaches the interface (R22): an external instrument with a MIDI To port,
/// or an external effect with an Audio To. The output renders in real time.
#[must_use]
pub fn needs_real_time(document: &Document, track: Option<TrackId>) -> bool {
    match track {
        Some(track) => path_devices(document, track)
            .into_iter()
            .any(reaches_interface),
        // The Main output: the devices on the paths that reach it, and
        // Main's own.
        None => document
            .track_order()
            .into_iter()
            .filter(|id| *id != document.main_id() && reaches_main(document, *id))
            .flat_map(|id| {
                document
                    .track(id)
                    .map(|track| track.devices_deep())
                    .into_iter()
                    .flatten()
            })
            .chain(document.main().devices())
            .any(reaches_interface),
    }
}

/// Whether a track's path reaches Main (R22).
fn reaches_main(document: &Document, track: TrackId) -> bool {
    let mut current = Some(track);
    while let Some(id) = current {
        if id == document.main_id() {
            return true;
        }
        current = document.output_of(id);
    }
    false
}

/// The devices on a track's own path to Main: its own and its groups'.
fn path_devices(document: &Document, track: TrackId) -> Vec<&Device> {
    let mut devices = Vec::new();
    let mut current = Some(track);
    while let Some(id) = current {
        let Some(t) = document.track(id) else { break };
        devices.extend(t.devices_deep());
        current = document.output_of(id);
    }
    devices
}

/// Whether a device's routing reaches the audio interface: an external
/// instrument with a MIDI To port, or an external effect with an Audio To
/// (R22), or a CV device with an output, an input or an Audio From chosen
/// (the CV tools spec, R7).
fn reaches_interface(device: &Device) -> bool {
    if !device.enabled {
        return false;
    }
    device.external.as_ref().is_some_and(|external| {
        external.port.is_some() || external.audio_to.is_some() || external.cv_routed()
    })
}

/// Renders the outputs that need the hardware, in one pass on the running
/// engine (R22 to R26). The engine plays the range; the audio thread copies
/// each output into a ring; a writer thread writes the files.
///
/// The render waits `wait` before it starts, in `session`, whose phase the
/// app's dialog shows. A dropout found while it runs starts it again while
/// Auto-Restart is on, up to `max_attempts` times; with it off, the render
/// carries on and the dropouts are listed.
///
/// # Errors
///
/// [`ExportError`]: no audio device is open, the range can't be rendered,
/// the render was cancelled, or writing failed.
#[expect(clippy::too_many_arguments, reason = "the render's inputs")]
pub fn render(
    audio: &mut AudioHost,
    outputs: &[(Option<TrackId>, Job)],
    document: &Document,
    start: Ticks,
    length: Length,
    wait: Duration,
    auto_restart: bool,
    max_attempts: Option<u32>,
    session: &Session,
    progress: &mut dyn FnMut(f64),
) -> Result<(Vec<Report>, Outcome), ExportError> {
    // Spec: external-devices R26. Without a running audio device, an export
    // that needs real time is refused before anything renders.
    if !audio.engine_running() {
        return Err(ExportError::Invalid(
            "the export needs the hardware, and no audio device is open".to_owned(),
        ));
    }
    let mut outcome = Outcome::default();
    if outputs.is_empty() {
        return Ok((Vec::new(), outcome));
    }
    let rate = SampleRate::new(audio.sample_rate().unwrap_or(export::DEFAULT_ENGINE_RATE))
        .ok_or_else(|| ExportError::Invalid("the device's rate is unknown".to_owned()))?;
    let tempo = document.tempo_map();
    let first = tempo.ticks_to_frames(start, rate).get();
    let frames = match length {
        Length::Frames(frames) => frames,
        Length::To(end) => {
            u64::try_from(tempo.ticks_to_frames(end, rate).get() - first).unwrap_or(0)
        }
    };
    if frames == 0 {
        return Err(ExportError::Invalid(
            "the range is shorter than one frame".to_owned(),
        ));
    }
    // The dialog's switch starts as the export's argument says.
    session.set_auto_restart(auto_restart);
    let cancel = session.cancel_flag();
    let mut attempt = 0u32;
    let mut reports: Vec<Report> = Vec::new();
    loop {
        attempt += 1;
        outcome.attempts = attempt;
        if cancel.load(Ordering::Relaxed) {
            return Err(ExportError::Cancelled);
        }
        // Spec: external-devices R23. The render waits, so the hardware's
        // last sound dies away.
        if !session.wait(wait) {
            return Err(ExportError::Cancelled);
        }
        session.set_state(Phase::Rendering, 0.0, attempt);
        let (rendered, found) = match play(
            audio,
            outputs,
            start,
            first,
            frames,
            rate.hz(),
            &cancel,
            session,
            attempt,
            progress,
        ) {
            Ok(rendered) => rendered,
            Err(error) => {
                // Spec: external-devices R23. A render that stops leaves no
                // file behind: its partial file is deleted.
                for (_, job) in outputs {
                    for partial in job.partials() {
                        let _ = std::fs::remove_file(partial);
                    }
                    let _ = std::fs::remove_file(export::hidden_beside(&job.path, "render"));
                }
                session.set_state(Phase::Idle, 0.0, attempt);
                return Err(error);
            }
        };
        reports.extend(rendered);
        outcome.dropouts.extend(found.iter().cloned());
        // Spec: external-devices R25. A dropout starts the render again
        // while Auto-Restart is on; with it off, the render carries on.
        if found.is_empty() || !session.auto_restart() {
            break;
        }
        if max_attempts.is_some_and(|max| attempt >= max) {
            break;
        }
    }
    session.set_state(Phase::Idle, 0.0, attempt);
    Ok((reports, outcome))
}

/// Plays the range on the running engine and captures the outputs, until
/// the range is rendered. Returns the reports and the dropouts it found.
#[expect(clippy::too_many_arguments, reason = "the render's inputs")]
fn play(
    audio: &mut AudioHost,
    outputs: &[(Option<TrackId>, Job)],
    start: Ticks,
    first: i64,
    frames: u64,
    rate: u32,
    cancel: &Arc<AtomicBool>,
    session: &Session,
    attempt: u32,
    progress: &mut dyn FnMut(f64),
) -> Result<(Vec<Report>, Vec<Dropout>), ExportError> {
    // The rings the audio thread fills, and the writers that drain them.
    let mut consumers: Vec<Consumer<f32>> = Vec::with_capacity(outputs.len());
    // The consumers are moved into the writers, one each.
    let mut writers = Vec::with_capacity(outputs.len());
    let mut dropped = Vec::with_capacity(outputs.len());
    let mut captured = Vec::with_capacity(outputs.len());
    let mut producers = Vec::with_capacity(outputs.len());
    let finished = Arc::new(AtomicBool::new(false));
    for _ in outputs {
        let (producer, consumer) = RingBuffer::new(RING_FRAMES);
        producers.push(producer);
        consumers.push(consumer);
        dropped.push(Arc::new(AtomicU64::new(0)));
        captured.push(Arc::new(AtomicU64::new(0)));
    }
    for (_, job) in outputs.iter() {
        let targets = export::targets(job)?;
        let scratch = export::hidden_beside(&job.path, "render");
        let out = export::open(job, &scratch, &targets)?;
        // Spec: export R33. Convert to Mono makes the file one channel.
        let channels = usize::from(targets[0].1.channels);
        let converter = if job.sample_rate < rate {
            Some(Downsampler::new(rate, job.sample_rate, channels, frames)?)
        } else {
            None
        };
        let consumer = consumers.remove(0);
        let job = job.clone();
        let cancel = cancel.clone();
        let finished = Arc::clone(&finished);
        writers.push(thread::spawn(move || {
            write_output(consumer, converter, out, job, cancel, finished)
        }));
    }
    let plan = RenderPlan {
        start: Frames(first),
        frames,
        outputs: outputs
            .iter()
            .enumerate()
            .map(|(at, (track, _))| RenderOutput {
                track: *track,
                producer: producers.remove(0),
                dropped: Arc::clone(&dropped[at]),
                captured: Arc::clone(&captured[at]),
            })
            .collect(),
    };
    // The plan goes to the engine, which plays the range.
    if let Err(why) = audio.render(plan) {
        // Nothing will fill the rings now: the writers stop at once rather
        // than wait for samples that never come.
        finished.store(true, Ordering::Relaxed);
        for writer in writers {
            let _ = writer.join();
        }
        return Err(ExportError::Failed(why));
    }
    // The song plays from the range's start, with the loop off.
    let _ = audio.send_transport(TransportCommand::SetLoop(None));
    let _ = audio.send_transport(TransportCommand::Locate(start));
    let _ = audio.send_transport(TransportCommand::Play);
    let mut seen = Seen::default();
    let mut found: Vec<Dropout> = Vec::new();
    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let done = captured
            .iter()
            .all(|count| count.load(Ordering::Relaxed) >= frames);
        let stats = audio.stats();
        let position = audio.transport_state().position;
        let new = dropouts(&mut seen, stats, &dropped, position);
        found.extend(new);
        if done || !audio.engine_running() {
            break;
        }
        let share = captured.first().map_or(0.0, |count| {
            count.load(Ordering::Relaxed) as f64 / frames as f64
        });
        // Spec: external-devices R23. The dialog shows the render's
        // progress and how many attempts it has taken.
        session.set_state(Phase::Rendering, share, attempt);
        progress(share);
        thread::sleep(POLL);
    }
    // The song stops at the range's end; the rings close when the plan
    // goes, which the empty plan below does.
    let _ = audio.send_transport(TransportCommand::Stop);
    let _ = audio.render(RenderPlan {
        start: Frames(first),
        frames: 0,
        outputs: Vec::new(),
    });
    // The writers stop once the render is over.
    finished.store(true, Ordering::Relaxed);
    let mut reports = Vec::with_capacity(writers.len());
    for writer in writers {
        match writer.join() {
            Ok(Ok(report)) => reports.push(report),
            Ok(Err(error)) => return Err(error),
            Err(_) => return Err(ExportError::Failed("the writer thread stopped".to_owned())),
        }
    }
    Ok((reports, found))
}

/// What the render has seen of the device's counters, so a dropout is
/// listed once (R25).
#[derive(Default)]
struct Seen {
    /// The counters as they were at the last look.
    stats: Option<bl_audio_io::StreamStats>,
    /// The frames the rings had dropped at the last look.
    rings: Vec<u64>,
}

/// The dropouts since the counters were last read, with the song position
/// each happened at (R25).
fn dropouts(
    seen: &mut Seen,
    now: Option<bl_audio_io::StreamStats>,
    dropped: &[Arc<AtomicU64>],
    position: Ticks,
) -> Vec<Dropout> {
    let mut found = Vec::new();
    let Some(now) = now else {
        return found;
    };
    let Some(was) = seen.stats else {
        seen.stats = Some(now);
        seen.rings = dropped
            .iter()
            .map(|count| count.load(Ordering::Relaxed))
            .collect();
        return found;
    };
    for (kind, was, is) in [
        ("the device reported an xrun", was.xruns, now.xruns),
        ("the device's clock broke", was.gaps, now.gaps),
        (
            "a callback overran its block",
            was.late_blocks,
            now.late_blocks,
        ),
    ] {
        if is > was {
            found.push(Dropout {
                position,
                kind: kind.to_owned(),
            });
        }
    }
    for (at, count) in dropped.iter().enumerate() {
        let now_dropped = count.load(Ordering::Relaxed);
        if now_dropped > seen.rings[at] {
            found.push(Dropout {
                position,
                kind: "a ring was too full for a block".to_owned(),
            });
        }
        seen.rings[at] = now_dropped;
    }
    seen.stats = Some(now);
    found
}

/// Drains the ring into the file, and finishes it. The writer runs on its
/// own thread, so the audio thread never waits for the disk.
fn write_output(
    mut consumer: Consumer<f32>,
    mut converter: Option<Downsampler>,
    mut out: export::Out,
    job: Job,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
) -> Result<Report, ExportError> {
    let began = Instant::now();
    let targets = export::targets(&job)?;
    let channels = usize::from(targets[0].1.channels);
    let scratch = export::hidden_beside(&job.path, "render");
    let mut peak = 0.0f32;
    let mut frames = 0u64;
    // The frames the scratch file holds, which the normalise pass reads
    // back: a rate conversion makes them fewer than `frames` (R26).
    let mut staged = 0u64;
    let mut samples: Vec<f32> = Vec::with_capacity(8192 * CHANNELS);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ExportError::Cancelled);
        }
        while samples.len() < 8192 * CHANNELS {
            match consumer.pop() {
                Ok(sample) => samples.push(sample),
                Err(PopError::Empty) => break,
            }
        }
        // Only whole frames are written: the audio thread pushes one sample
        // at a time, so a pop can land between a frame's channels, and the
        // half frame waits here for the rest.
        let whole = samples.len() - samples.len() % CHANNELS;
        if whole > 0 {
            let mixed;
            let block = if job.mono {
                mixed = export::downmix(&samples[..whole], CHANNELS);
                &mixed[..]
            } else {
                &samples[..whole]
            };
            for sample in block {
                if sample.is_finite() {
                    peak = peak.max(sample.abs());
                }
            }
            frames += (block.len() / channels) as u64;
            match converter.as_mut() {
                Some(converter) => {
                    let mut written = Vec::new();
                    converter.push(block, &mut |out| {
                        written.extend_from_slice(out);
                        Ok(())
                    })?;
                    staged += (written.len() / channels) as u64;
                    export::write(&mut out, &written)?;
                }
                None => {
                    staged += (block.len() / channels) as u64;
                    export::write(&mut out, block)?;
                }
            }
            samples.drain(..whole);
        } else if !done.load(Ordering::Relaxed) {
            // Nothing to write for now: the writer waits for more.
            thread::sleep(POLL);
        } else {
            break;
        }
    }
    // The converter's last frames.
    if let Some(converter) = converter.as_mut() {
        let mut written = Vec::new();
        converter.finish(&mut |out| {
            written.extend_from_slice(out);
            Ok(())
        })?;
        staged += (written.len() / channels) as u64;
        export::write(&mut out, &written)?;
    }
    // The normalise pass, or the writers themselves.
    let (writers, gain, peak) = match out {
        export::Out::Writers(writers) => (writers, 1.0, peak),
        out @ export::Out::Scratch(_, _) => export::finish(
            out,
            &targets,
            scratch.as_path(),
            &mut [],
            staged,
            peak,
            job.normalize,
            &cancel,
            &mut |_| {},
        )?,
    };
    let mut written = 0;
    for (writer, (partial, _)) in writers.into_iter().zip(&targets) {
        written = writer
            .finish()
            .map_err(|error| ExportError::Failed(format!("{}: {error}", partial.display())))?;
    }
    Ok(Report {
        frames: written,
        channels: channels as u16,
        render_rate: job.engine_rate,
        rendered_frames: frames,
        peak,
        gain,
        took: began.elapsed(),
        missing_media: Vec::new(),
        unhosted: Vec::new(),
        heap_operations: 0,
    })
}

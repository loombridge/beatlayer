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

use std::path::Path;
use std::sync::Arc;
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
/// (R22).
fn reaches_interface(device: &Device) -> bool {
    if !device.enabled {
        return false;
    }
    device
        .external
        .as_ref()
        .is_some_and(|external| external.port.is_some() || external.audio_to.is_some())
}

/// Renders the outputs that need the hardware, in one pass on the running
/// engine (R22 to R26). The engine plays the range; the audio thread copies
/// each output into a ring; a writer thread writes the files.
///
/// The render waits `wait` before it starts. A dropout found while it runs
/// starts it again while `auto_restart` is on, up to `max_attempts` times;
/// with it off, the render carries on and the dropouts are listed.
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
    cancel: &Arc<AtomicBool>,
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
        if !wait_out(wait, cancel) {
            return Err(ExportError::Cancelled);
        }
        let (rendered, found) = play(
            audio,
            outputs,
            start,
            first,
            frames,
            rate.hz(),
            cancel,
            progress,
        )?;
        reports.extend(rendered);
        outcome.dropouts.extend(found.iter().cloned());
        // Spec: external-devices R25. A dropout starts the render again
        // while Auto-Restart is on; with it off, the render carries on.
        if found.is_empty() || !auto_restart {
            break;
        }
        if max_attempts.is_some_and(|max| attempt >= max) {
            break;
        }
    }
    Ok((reports, outcome))
}

/// Waits for `period`, in short steps, so a cancel is noticed. Returns
/// whether the wait ran out.
fn wait_out(period: Duration, cancel: &AtomicBool) -> bool {
    let until = Instant::now() + period;
    while Instant::now() < until {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        let left = until.saturating_duration_since(Instant::now());
        thread::sleep(POLL.min(left));
    }
    !cancel.load(Ordering::Relaxed)
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
        let spec = export::encode_spec(job)?;
        let partial = export::hidden_beside(&job.path, "partial");
        let scratch = export::hidden_beside(&job.path, "render");
        let out = export::open(job, (partial.as_path(), scratch.as_path()), spec)?;
        let converter = if job.sample_rate < rate {
            Some(Downsampler::new(rate, job.sample_rate, CHANNELS, frames)?)
        } else {
            None
        };
        let consumer = consumers.remove(0);
        let job = job.clone();
        let cancel = Arc::clone(cancel);
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
        for writer in writers {
            let _ = writer.join();
        }
        return Err(ExportError::Failed(why));
    }
    // The song plays from the range's start, with the loop off.
    let _ = audio.send_transport(TransportCommand::SetLoop(None));
    let _ = audio.send_transport(TransportCommand::Locate(start));
    let _ = audio.send_transport(TransportCommand::Play);
    let before = audio.stats();
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
        let new = dropouts(before, stats, &dropped, position);
        found.extend(new);
        if done || !audio.engine_running() {
            break;
        }
        let share = captured
            .first()
            .map_or(0.0, |count| {
                count.load(Ordering::Relaxed) as f64 / frames as f64
            });
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

/// The dropouts since the counters were read, with the song position each
/// happened at (R25).
fn dropouts(
    before: Option<bl_audio_io::StreamStats>,
    now: Option<bl_audio_io::StreamStats>,
    dropped: &[Arc<AtomicU64>],
    position: Ticks,
) -> Vec<Dropout> {
    let mut found = Vec::new();
    let (Some(before), Some(now)) = (before, now) else {
        return found;
    };
    for (kind, was, is) in [
        ("the device reported an xrun", before.xruns, now.xruns),
        ("the device's clock broke", before.gaps, now.gaps),
        ("a callback overran its block", before.late_blocks, now.late_blocks),
    ] {
        if is > was {
            found.push(Dropout {
                position,
                kind: kind.to_owned(),
            });
        }
    }
    for count in dropped {
        if count.load(Ordering::Relaxed) > 0 {
            found.push(Dropout {
                position,
                kind: "a ring was too full for a block".to_owned(),
            });
        }
    }
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
    let spec = export::encode_spec(&job)?;
    let partial = export::hidden_beside(&job.path, "partial");
    let scratch = export::hidden_beside(&job.path, "render");
    let mut peak = 0.0f32;
    let mut frames = 0u64;
    let mut samples: Vec<f32> = Vec::with_capacity(8192 * CHANNELS);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ExportError::Cancelled);
        }
        samples.clear();
        while samples.len() < 8192 * CHANNELS {
            match consumer.pop() {
                Ok(sample) => samples.push(sample),
                Err(PopError::Empty) => break,
            }
        }
        if !samples.is_empty() {
            for sample in &samples {
                if sample.is_finite() {
                    peak = peak.max(sample.abs());
                }
            }
            frames += (samples.len() / CHANNELS) as u64;
            match converter.as_mut() {
                Some(converter) => {
                    let mut written = Vec::new();
                    converter.push(&samples, &mut |out| {
                        written.extend_from_slice(out);
                        Ok(())
                    })?;
                    export::write(&mut out, &written)?;
                }
                None => export::write(&mut out, &samples)?,
            }
        } else if !done.load(Ordering::Relaxed) {
            // The ring is empty for now: the writer waits for more.
            thread::sleep(POLL);
            continue;
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
        export::write(&mut out, &written)?;
    }
    // The normalise pass, or the writer itself.
    let (writer, gain, peak) = match out {
        export::Out::Writer(writer) => (writer, 1.0, peak),
        out @ export::Out::Scratch(_, _) => export::finish(
            out,
            spec,
            partial.as_path(),
            scratch.as_path(),
            &mut [],
            frames,
            peak,
            &cancel,
            &mut |_| {},
        )?,
    };
    let failed = |what: &Path, error: &dyn std::fmt::Display| {
        ExportError::Failed(format!("{}: {error}", what.display()))
    };
    let written = writer.finish().map_err(|error| failed(&partial, &error))?;
    Ok(Report {
        frames: written,
        channels: CHANNELS as u16,
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

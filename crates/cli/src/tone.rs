//! `bl-cli tone`: a 440 Hz sine on a device, with a dropout count.

use std::f64::consts::TAU;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use bl_audio_io::{
    AudioBuffer, AudioBufferMut, AudioCallback, DeviceId, DeviceSelector, PortRequest,
    ProcessContext, Stream, StreamInfo, StreamRequest, StreamStats,
};

use crate::Outcome;
use crate::devices::{parse_backend, resolve};

const FREQUENCY: f64 = 440.0;

/// How long to wait past the end for a device that stops asking for audio.
const STALL_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, clap::Args)]
pub(crate) struct ToneArgs {
    /// The backend: coreaudio, wasapi, asio, pipewire, jack, alsa, or null.
    /// Defaults to the platform's, or the one in a device ID.
    #[arg(long)]
    backend: Option<String>,

    /// The device: an ID or index from `bl-cli devices`, or part of its name.
    /// Defaults to the system default output, which the tone follows if it
    /// changes.
    #[arg(long)]
    device: Option<String>,

    /// The sample rate in hertz. Defaults to the rate the device runs at now,
    /// so the device's setting doesn't change.
    #[arg(long)]
    rate: Option<u32>,

    /// The buffer size in frames. Defaults to the device's.
    #[arg(long)]
    buffer: Option<u32>,

    /// Output channels. Defaults to the device's.
    #[arg(long)]
    channels: Option<u16>,

    /// How long to play, in seconds.
    #[arg(long, default_value_t = 5.0)]
    pub(crate) seconds: f64,

    /// The level in dBFS, 0 or lower.
    #[arg(long, default_value_t = -20.0, allow_negative_numbers = true)]
    pub(crate) level: f64,

    /// Self-test: overrun one callback every this many seconds, by spinning
    /// for three buffers' worth of time, to check that dropouts are caught.
    #[arg(long, value_name = "SECONDS")]
    glitch_every: Option<f64>,
}

/// The test tone: a sine on every output channel that remembers its peak.
struct Sine {
    phase: f64,
    amplitude: f32,
    peak: Arc<AtomicU32>,
    /// Seconds between deliberate overruns, if any.
    glitch_every: Option<f64>,
}

impl Sine {
    /// Spins past the block's deadline if this block crosses a multiple of
    /// `glitch_every`. Spinning breaks the real-time rules on purpose.
    fn glitch(&self, ctx: &ProcessContext) {
        let Some(every) = self.glitch_every else {
            return;
        };
        let every = (every * f64::from(ctx.sample_rate)).round().max(1.0) as u64;
        // The first multiple of `every` at or after this block's start.
        let next = ctx.position.div_ceil(every) * every;
        if next > 0 && next < ctx.position + ctx.frames as u64 {
            let lasts = ctx.frames as f64 / f64::from(ctx.sample_rate);
            let until = Instant::now() + Duration::from_secs_f64(3.0 * lasts);
            while Instant::now() < until {
                std::hint::spin_loop();
            }
        }
    }
}

impl AudioCallback for Sine {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        _input: &AudioBuffer<'_>,
        output: &mut AudioBufferMut<'_>,
    ) {
        self.glitch(ctx);
        let step = FREQUENCY / f64::from(ctx.sample_rate);
        let mut channels = output.iter_mut();
        let Some(first) = channels.next() else {
            return;
        };
        let mut peak = 0.0f32;
        for sample in first.iter_mut() {
            *sample = (self.phase * TAU).sin() as f32 * self.amplitude;
            peak = peak.max(sample.abs());
            self.phase += step;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
        }
        for channel in channels {
            channel.copy_from_slice(first);
        }
        // Non-negative floats order the same way as their bit patterns.
        self.peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
    }
}

pub(crate) fn run(args: &ToneArgs) -> Outcome {
    if !(args.level.is_finite() && args.level <= 0.0) {
        return Err(format!(
            "--level must be 0 dBFS or lower, not {}",
            args.level
        ));
    }
    if !(args.seconds.is_finite() && args.seconds > 0.0) {
        return Err(format!(
            "--seconds must be more than 0, not {}",
            args.seconds
        ));
    }
    if let Some(every) = args.glitch_every
        && !(every.is_finite() && every > 0.0)
    {
        return Err(format!("--glitch-every must be more than 0, not {every}"));
    }

    let id = args
        .device
        .as_deref()
        .and_then(|d| d.parse::<DeviceId>().ok());
    let kind = match (&args.backend, &id) {
        (Some(name), Some(id)) if parse_backend(name)? != id.backend() => {
            return Err(format!("`{id}` isn't a {name} device"));
        }
        (Some(name), _) => parse_backend(name)?,
        (None, Some(id)) => id.backend(),
        (None, None) => bl_audio_io::default_backend(),
    };
    let backend = bl_audio_io::open_backend(kind).map_err(|e| e.to_string())?;
    let device = match &args.device {
        None => DeviceSelector::Default,
        Some(query) => DeviceSelector::Id(resolve(backend.as_ref(), query)?.id),
    };
    let request = StreamRequest {
        output: Some(PortRequest {
            device,
            channels: args.channels,
        }),
        input: None,
        sample_rate: args.rate,
        buffer_size: args.buffer,
    };

    let peak = Arc::new(AtomicU32::new(0));
    let sine = Sine {
        phase: 0.0,
        amplitude: 10f64.powf(args.level / 20.0) as f32,
        peak: Arc::clone(&peak),
        glitch_every: args.glitch_every,
    };
    let mut stream = backend
        .open_stream(&request, Box::new(sine))
        .map_err(|e| e.to_string())?;

    println!(
        "bl-cli tone: {FREQUENCY} Hz sine at {:.1} dBFS for {} s",
        args.level, args.seconds
    );
    print_info(stream.info());
    println!();

    stream.start().map_err(|e| e.to_string())?;
    let played = play(stream.as_mut(), args.seconds);
    let stopped = stream.stop();
    let stats = stream.stats();
    let info = stream.info().clone();
    drop(stream);

    println!();
    print_summary(&info, &stats, f32::from_bits(peak.load(Ordering::Relaxed)));
    played?;
    stopped.map_err(|e| e.to_string())?;
    if stats.dropouts > 0 {
        eprintln!("error: {} dropouts", stats.dropouts);
        return Ok(ExitCode::FAILURE);
    }
    Ok(ExitCode::SUCCESS)
}

/// Waits while the stream plays `seconds` of audio, logging progress and
/// anything the stream reports.
fn play(stream: &mut dyn Stream, seconds: f64) -> Result<(), String> {
    let rate = f64::from(stream.info().sample_rate);
    let target = (seconds * rate).round() as u64;
    let every = if seconds >= 30.0 { 10.0 } else { 1.0 };
    let every_frames = (every * rate).round() as u64;
    let mut next_report = every_frames;
    let deadline = Duration::from_secs_f64(seconds) + STALL_GRACE;
    let began = Instant::now();
    let mut last = StreamStats::default();

    loop {
        thread::sleep(Duration::from_millis(20));
        let stats = stream.stats();
        let at = format!("[{:7.1} s]", stats.frames as f64 / rate);
        for line in events(&last, &stats) {
            println!("{at} {line}");
        }
        if stats.device_changes > last.device_changes {
            match stream.current_buffer_size() {
                Some(frames) => {
                    println!("{at} the backend now reports a buffer of {frames} frames")
                }
                None => println!("{at} the backend doesn't report its buffer size now"),
            }
        }
        if stats.device_lost {
            return Err("the device went away".into());
        }
        if stats.invalidated {
            return Err(
                "the backend says the stream must be rebuilt; the device's sample rate may have changed"
                    .into(),
            );
        }
        while stats.frames >= next_report && next_report <= target {
            println!(
                "[{:7.1} s] {} frames, device buffers {}, longest callback {}, dropouts {}",
                next_report as f64 / rate,
                stats.frames,
                device_buffers(&stats),
                millis(stats.longest_block),
                stats.dropouts,
            );
            next_report += every_frames;
        }
        if stats.frames >= target {
            return Ok(());
        }
        if began.elapsed() > deadline {
            return Err(format!(
                "the device stopped asking for audio after {} frames",
                stats.frames
            ));
        }
        last = stats;
    }
}

/// Lines describing what changed between two snapshots.
fn events(before: &StreamStats, now: &StreamStats) -> Vec<String> {
    let mut lines = Vec::new();
    let mut count = |what: &str, before: u64, now: u64| {
        if now > before {
            lines.push(format!("{what}: {}", now - before));
        }
    };
    count("backend reported xruns", before.xruns, now.xruns);
    count("clock gaps", before.gaps, now.gaps);
    count("late callbacks", before.late_blocks, now.late_blocks);
    count(
        "default output device changes; the tone follows the new default",
        before.device_changes,
        now.device_changes,
    );
    count(
        "other backend errors",
        before.other_errors,
        now.other_errors,
    );
    if now.realtime_denied && !before.realtime_denied {
        lines.push("the system refused real-time scheduling for the audio thread".into());
    }
    lines
}

fn print_info(info: &StreamInfo) {
    println!("  backend:     {}", info.backend);
    if let Some(device) = &info.output_device {
        let follows = if info.follows_default {
            ", following the system default"
        } else {
            ""
        };
        println!("  device:      {} ({}){follows}", device.name, device.id);
    }
    println!("  sample rate: {} Hz", info.sample_rate);
    println!(
        "  channels:    {}, sent to the device as {}",
        info.output_channels, info.sample_format
    );
    let requested = info.requested_buffer_size.map_or_else(
        || "the device's default".to_owned(),
        |n| format!("{n} frames"),
    );
    let reported = info
        .buffer_size
        .map_or_else(|| "nothing yet".to_owned(), |n| format!("{n} frames"));
    println!("  buffer:      requested {requested}, backend reports {reported}");
}

fn print_summary(info: &StreamInfo, stats: &StreamStats, peak: f32) {
    let rate = f64::from(info.sample_rate);
    let budget = Duration::from_secs_f64(
        f64::from(stats.last_device_frames.unwrap_or(info.max_block_frames)) / rate,
    );
    println!("Summary");
    println!(
        "  played:          {:.2} s ({} frames) in {} callbacks",
        stats.frames as f64 / rate,
        stats.frames,
        stats.device_callbacks
    );
    println!("  device buffers:  {}", device_buffers(stats));
    println!(
        "  longest callback: {} of a {} buffer",
        millis(stats.longest_block),
        millis(budget)
    );
    println!(
        "  dropouts:        {} (detections: backend xruns {}, clock gaps {}, late callbacks {})",
        stats.dropouts, stats.xruns, stats.gaps, stats.late_blocks
    );
    println!("  device changes:  {}", stats.device_changes);
    let peak = if peak > 0.0 {
        format!("{:.1} dBFS", 20.0 * f64::from(peak).log10())
    } else {
        "silence".to_owned()
    };
    println!("  peak level:      {peak}");
}

/// The buffer sizes the device actually asked for.
fn device_buffers(stats: &StreamStats) -> String {
    match (stats.min_device_frames, stats.max_device_frames) {
        (Some(min), Some(max)) if min == max => format!("{min} frames"),
        (Some(min), Some(max)) => format!("{min} to {max} frames"),
        _ => "none yet".to_owned(),
    }
}

fn millis(duration: Duration) -> String {
    format!("{:.3} ms", duration.as_secs_f64() * 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bl_audio_io::{Backend, NullBackend, NullOptions, Pacing};

    #[test]
    fn the_sine_has_the_requested_level_on_every_channel() {
        let peak = Arc::new(AtomicU32::new(0));
        let sine = Sine {
            phase: 0.0,
            amplitude: 10f64.powf(-60.0 / 20.0) as f32,
            peak: Arc::clone(&peak),
            glitch_every: None,
        };
        let backend = NullBackend::with_options(NullOptions {
            pacing: Pacing::Free,
            frame_limit: Some(48_000),
        });
        let mut stream = backend
            .open_stream(
                &StreamRequest::output(DeviceSelector::Default),
                Box::new(sine),
            )
            .unwrap();
        stream.start().unwrap();
        while stream.stats().frames < 48_000 {
            thread::sleep(Duration::from_millis(1));
        }
        stream.stop().unwrap();
        let peak = f32::from_bits(peak.load(Ordering::Relaxed));
        let level = 20.0 * f64::from(peak).log10();
        assert!((level + 60.0).abs() < 0.01, "peak at {level} dBFS");
    }

    #[test]
    fn events_name_what_changed() {
        let before = StreamStats::default();
        let now = StreamStats {
            xruns: 2,
            device_changes: 1,
            ..StreamStats::default()
        };
        let lines = events(&before, &now);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("xruns: 2"));
        assert!(lines[1].contains("follows the new default"));
        assert!(events(&now, &now).is_empty());
    }

    #[test]
    fn device_buffers_show_a_range_only_when_sizes_vary() {
        let mut stats = StreamStats::default();
        assert_eq!(device_buffers(&stats), "none yet");
        stats.min_device_frames = Some(128);
        stats.max_device_frames = Some(128);
        assert_eq!(device_buffers(&stats), "128 frames");
        stats.max_device_frames = Some(256);
        assert_eq!(device_buffers(&stats), "128 to 256 frames");
    }
}

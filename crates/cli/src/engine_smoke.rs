//! `bl-cli engine-smoke`: the engine end to end. It builds a small song,
//! compiles and plays it on a device, and recompiles it over and over while
//! it plays, counting dropouts, swaps, and heap use on the audio thread.

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::thread;
use std::time::{Duration, Instant};

use bl_audio_io::{
    AudioBuffer, AudioBufferMut, AudioCallback, BackendKind, DeviceId, DeviceSelector, PortRequest,
    ProcessContext, StreamInfo, StreamRequest, StreamStats,
};
use bl_core::time::SampleRate;
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_engine::testing::{self, Probe};
use bl_engine::{Engine, EngineConfig, EngineState, Processor};
use bl_model::{Command, Device, DeviceKind, Editor, ParamId, TrackId, TrackKind};
use rtrb::{Consumer, RingBuffer};

use crate::Outcome;
use crate::devices::{parse_backend, resolve};

/// The tones' frequencies, a fifth apart.
const FREQUENCIES: [f64; 2] = [220.0, 330.0];

/// The tone tracks' fader: −6 dB, so the two tones together peak no higher
/// than either alone.
const TONE_VOLUME: f64 = 0.5;

#[derive(Debug, clap::Args)]
pub(crate) struct EngineSmokeArgs {
    /// The backend: coreaudio, wasapi, asio, pipewire, jack, alsa, or null.
    /// Defaults to null, which needs no sound hardware, or to the one in a
    /// device ID.
    #[arg(long)]
    backend: Option<String>,

    /// The device: an ID or index from `bl-cli devices`, or part of its name.
    /// Defaults to the backend's default output.
    #[arg(long)]
    device: Option<String>,

    /// The sample rate in hertz. Defaults to the rate the device runs at now,
    /// so the device's setting doesn't change.
    #[arg(long)]
    rate: Option<u32>,

    /// The buffer size in frames. Defaults to the device's.
    #[arg(long)]
    buffer: Option<u32>,

    /// How long to play, in seconds.
    #[arg(long, default_value_t = 5.0)]
    pub(crate) seconds: f64,

    /// Recompiles per second while it plays.
    #[arg(long, default_value_t = 100.0)]
    recompiles: f64,

    /// The output's peak level in dBFS, 0 or lower: each of the two tones
    /// plays at it through a fader at −6 dB.
    #[arg(long, default_value_t = -60.0, allow_negative_numbers = true)]
    pub(crate) level: f64,
}

/// The engine as a stream's callback. It receives its processor once the
/// stream has opened, so the engine is made at the rate the device runs at.
struct EngineCallback {
    incoming: Consumer<Processor>,
    processor: Option<Processor>,
    peak: Arc<AtomicU32>,
}

impl AudioCallback for EngineCallback {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        _input: &AudioBuffer<'_>,
        output: &mut AudioBufferMut<'_>,
    ) {
        if self.processor.is_none() {
            self.processor = self.incoming.pop().ok();
        }
        // Without an engine yet, the block stays silent.
        let Some(processor) = self.processor.as_mut() else {
            return;
        };
        let mut peak = 0.0f32;
        let mut done = 0;
        while done < ctx.frames {
            let frames = (ctx.frames - done).min(processor.config().max_frames);
            let block = processor.process(frames);
            for c in 0..output.channels() {
                let from = block.channel(c.min(block.channels() - 1));
                peak = from.iter().fold(peak, |p, s| p.max(s.abs()));
                output.channel_mut(c)[done..done + frames].copy_from_slice(from);
            }
            done += frames;
        }
        // Non-negative floats order the same way as their bit patterns.
        self.peak.fetch_max(peak.to_bits(), Relaxed);
    }
}

/// The song: two tone tracks, one with a counter in its chain, and a spare
/// track the edits work on.
struct Song {
    editor: Editor,
    tones: [TrackId; 2],
    spare: TrackId,
    /// The device the edits put on the spare track, while it's there.
    extra: Option<bl_model::DeviceId>,
}

impl Song {
    fn new(level: f64) -> Result<Self, String> {
        let mut editor = Editor::new();
        let spare = Self::track(&mut editor, TrackKind::Audio, "Spare")?;
        let mut tones = [spare; 2];
        for (i, frequency) in FREQUENCIES.into_iter().enumerate().rev() {
            let name = if i == 0 { "Tone A" } else { "Tone B" };
            let track = Self::track(&mut editor, TrackKind::Midi, name)?;
            let tone = Self::device(
                &mut editor,
                track,
                TEST_TONE,
                &[(test_tone::FREQUENCY, frequency), (test_tone::LEVEL, level)],
            );
            editor.apply(tone).map_err(|e| e.to_string())?;
            editor
                .apply(Command::SetVolume {
                    track,
                    volume: TONE_VOLUME,
                })
                .map_err(|e| e.to_string())?;
            tones[i] = track;
        }
        // The counter shows that a node keeps its state through every swap.
        let counter = Self::device(&mut editor, tones[0], testing::COUNTER, &[]);
        editor.apply(counter).map_err(|e| e.to_string())?;
        Ok(Self {
            editor,
            tones,
            spare,
            extra: None,
        })
    }

    fn track(editor: &mut Editor, kind: TrackKind, name: &str) -> Result<TrackId, String> {
        let id = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind,
                name: name.to_owned(),
                parent: None,
                index: 0,
            })
            .map_err(|e| e.to_string())?;
        Ok(id)
    }

    fn device(
        editor: &mut Editor,
        track: TrackId,
        device_type: &str,
        params: &[(ParamId, f64)],
    ) -> Command {
        let mut device = Device::new(
            editor.new_id(),
            "",
            DeviceKind::Native {
                device_type: device_type.to_owned(),
            },
        );
        for &(param, value) in params {
            device.params.insert(param, value);
        }
        Command::InsertDevice {
            track,
            index: 0,
            device,
        }
    }

    /// Edit number `n`: a change to the spare track's structure, which needs
    /// a new program. None of them makes a sound: the tone the first adds is
    /// at −120 dBFS.
    fn structural_edit(&mut self, n: u64) -> Command {
        match (n % 4, self.extra) {
            (0, _) | (_, None) => {
                let command = Self::device(
                    &mut self.editor,
                    self.spare,
                    TEST_TONE,
                    &[(test_tone::LEVEL, -120.0)],
                );
                if let Command::InsertDevice { device, .. } = &command {
                    self.extra = Some(device.id);
                }
                command
            }
            (1, _) => Command::MoveTrack {
                track: self.spare,
                parent: None,
                index: if n % 8 == 1 { 0 } else { 2 },
            },
            (2, Some(device)) => Command::SetDeviceEnabled {
                device,
                enabled: false,
            },
            (_, Some(device)) => {
                self.extra = None;
                Command::RemoveDevice { device }
            }
        }
    }

    /// Mixer edit number `n`, which needs no program: Tone B's fader moves
    /// between −6 dB and a little below.
    fn mixer_edit(&self, n: u64) -> Command {
        Command::SetVolume {
            track: self.tones[1],
            volume: if n.is_multiple_of(2) {
                TONE_VOLUME * 0.9
            } else {
                TONE_VOLUME
            },
        }
    }
}

pub(crate) fn run(args: &EngineSmokeArgs) -> Outcome {
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
    if !(args.recompiles.is_finite() && args.recompiles > 0.0 && args.recompiles <= 10_000.0) {
        return Err(format!(
            "--recompiles must be more than 0 and at most 10000, not {}",
            args.recompiles
        ));
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
        (None, None) => BackendKind::Null,
    };
    let backend = bl_audio_io::open_backend(kind).map_err(|e| e.to_string())?;
    let device = match &args.device {
        None => DeviceSelector::Default,
        Some(query) => DeviceSelector::Id(resolve(backend.as_ref(), query)?.id),
    };
    let request = StreamRequest {
        output: Some(PortRequest {
            device,
            channels: None,
        }),
        input: None,
        sample_rate: args.rate,
        buffer_size: args.buffer,
    };

    // Open the stream first, so the engine runs at the rate the device does.
    let (mut handoff, incoming) = RingBuffer::new(1);
    let peak = Arc::new(AtomicU32::new(0));
    let callback = EngineCallback {
        incoming,
        processor: None,
        peak: Arc::clone(&peak),
    };
    let mut stream = backend
        .open_stream(&request, Box::new(callback))
        .map_err(|e| e.to_string())?;
    let info = stream.info().clone();
    let rate = SampleRate::new(info.sample_rate).ok_or("the device runs at 0 Hz")?;
    let config = EngineConfig::new(rate, info.max_block_frames as usize);

    let probe = Arc::new(Probe::default());
    let mut song = Song::new(args.level)?;
    let (mut engine, processor) = Engine::start(config, testing::registry(&probe))
        .map_err(|e| format!("couldn't start the engine's threads: {e}"))?;
    engine.sync(song.editor.document());
    engine.play();
    if !engine.flush(Duration::from_secs(5)) {
        return Err("the engine's compiler didn't send the first program".into());
    }
    if handoff.push(processor).is_err() {
        return Err("couldn't hand the engine to the stream".into());
    }

    println!(
        "bl-cli engine-smoke: two test tones peaking at {:.1} dBFS, {} recompiles a second, for {} s",
        args.level, args.recompiles, args.seconds
    );
    print_info(&info);
    println!();

    stream.start().map_err(|e| e.to_string())?;
    let played = play(&mut engine, &mut song, stream.as_ref(), args);
    let stopped = stream.stop();
    let stats = stream.stats();
    // The stream owns the processor; dropping it lets the drop thread free
    // the last program.
    drop(stream);
    let state = engine.state().clone();
    let counts = engine.counts();
    let peak = f32::from_bits(peak.load(Relaxed));
    let counted = probe.counted.load(Relaxed);
    let counter = probe.counter.load(Relaxed);

    println!();
    println!("Summary");
    let seconds = stats.frames as f64 / rate.as_f64();
    println!(
        "  played:          {seconds:.2} s ({} frames) in {} callbacks",
        stats.frames, stats.device_callbacks
    );
    println!(
        "  programs:        {} swapped in: the first, and {} recompiles during playback",
        state.swaps,
        state.swaps.saturating_sub(1)
    );
    println!(
        "  migrations:      {} programs out of order (any is a bug)",
        state.counters.migration_mismatches
    );
    println!(
        "  mixer edits:     {} synced as parameter changes, without a program; {} folded into the next program",
        counts.without_program, counts.coalesced
    );
    println!(
        "  dropouts:        {} (detections: backend xruns {}, clock gaps {}, late callbacks {})",
        stats.dropouts, stats.xruns, stats.gaps, stats.late_blocks
    );
    let trap = stats.audio_thread.is_some_and(|t| t.trap_installed);
    println!(
        "  heap operations: {} on the audio thread{}",
        stats.heap_operations,
        if trap {
            " (allocation trap on)"
        } else {
            " (allocation trap off: release build)"
        }
    );
    println!(
        "  longest callback: {:.3} ms; engine load peaked at {:.1}% of a block",
        stats.longest_block.as_secs_f64() * 1000.0,
        state.cpu_load_max * 100.0
    );
    println!(
        "  counter node:    instance {counter} counted {counted} of the engine's {} frames",
        state.clock
    );
    println!("  peak level:      {}", level(peak));

    played?;
    stopped.map_err(|e| e.to_string())?;
    let mut failed = Vec::new();
    if stats.dropouts > 0 {
        failed.push(format!("{} dropouts", stats.dropouts));
    }
    if stats.heap_operations > 0 {
        failed.push(format!(
            "{} heap operations on the audio thread",
            stats.heap_operations
        ));
    }
    if state.counters.migration_mismatches > 0 {
        failed.push("programs arrived out of order".to_owned());
    }
    if counter != 1 || counted != state.clock {
        failed.push("the counter node didn't keep its state".to_owned());
    }
    for failure in &failed {
        eprintln!("error: {failure}");
    }
    Ok(if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Plays for `args.seconds`, editing the song as it goes, and reports once
/// a second.
fn play(
    engine: &mut Engine,
    song: &mut Song,
    stream: &dyn bl_audio_io::Stream,
    args: &EngineSmokeArgs,
) -> Result<(), String> {
    let rate = f64::from(stream.info().sample_rate);
    let target = (args.seconds * rate).round() as u64;
    let period = Duration::from_secs_f64(1.0 / args.recompiles);
    let deadline = Duration::from_secs_f64(args.seconds) + Duration::from_secs(5);
    let began = Instant::now();
    let mut next_edit = began;
    let mut next_report = 1.0;
    let mut edits = 0u64;
    loop {
        let stats = stream.stats();
        let without_program = engine.counts().without_program;
        let state = engine.state();
        let at = state.clock as f64 / rate;
        if at >= next_report {
            report(at, &stats, state, without_program);
            next_report += 1.0;
        }
        if state.clock >= target {
            return Ok(());
        }
        if began.elapsed() > deadline {
            return Err(format!(
                "the device stopped asking for audio after {} frames",
                stats.frames
            ));
        }
        if stats.device_lost {
            return Err("the device went away".into());
        }
        if Instant::now() >= next_edit {
            let command = song.structural_edit(edits);
            song.editor.apply(command).map_err(|e| e.to_string())?;
            engine.sync(song.editor.document());
            if edits % 10 == 5 {
                let command = song.mixer_edit(edits / 10);
                song.editor.apply(command).map_err(|e| e.to_string())?;
                engine.sync(song.editor.document());
            }
            edits += 1;
            next_edit += period;
        }
        let wait = next_edit
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(5));
        thread::sleep(wait);
    }
}

fn report(at: f64, stats: &StreamStats, state: &EngineState, without_program: u64) {
    println!(
        "[{at:7.1} s] programs {}, mixer edits without a program {without_program}, dropouts {}, heap operations {}, engine load {:.1}%",
        state.swaps,
        stats.dropouts,
        stats.heap_operations,
        state.cpu_load * 100.0
    );
}

fn print_info(info: &StreamInfo) {
    println!("  backend:     {}", info.backend);
    if let Some(device) = &info.output_device {
        println!("  device:      {} ({})", device.name, device.id);
    }
    println!("  sample rate: {} Hz", info.sample_rate);
    println!("  channels:    {}", info.output_channels);
    let reported = info
        .buffer_size
        .map_or_else(|| "unknown".to_owned(), |n| format!("{n} frames"));
    println!(
        "  buffer:      {reported}; the engine takes blocks of up to {} frames",
        info.max_block_frames
    );
}

fn level(peak: f32) -> String {
    if peak > 0.0 {
        format!("{:.1} dBFS", 20.0 * f64::from(peak).log10())
    } else {
        "silence".to_owned()
    }
}

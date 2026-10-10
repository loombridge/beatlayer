//! `bl-cli device-changes`: the engine on a device through device changes
//! (WP-1.13). It plays a quiet tone through `AudioEngine` and prints what
//! happens to the device as it happens: the message the app would show, and
//! when audio comes back.
//!
//! With `--simulate`, it runs on the null backend's simulated USB interface
//! and puts it through each change in turn (unplug and replug, a sample-rate
//! change, sleep and wake, a stall), checking that the engine stops cleanly,
//! says why, and recovers. On a real device it plays for `--seconds` while
//! someone unplugs and replugs the interface, changes its rate in Audio MIDI
//! Setup, or sleeps the machine.

use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use bl_audio_io::{
    Backend, BackendKind, DeviceId, DeviceSelector, NullBackend, NullControl, NullOptions,
    PortRequest, StreamRequest, StreamStats,
};
use bl_core::time::SampleRate;
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_engine::testing::{self, Probe};
use bl_engine::{AudioEngine, Cause, DeviceEvent, DeviceStatus, EngineConfig, SupervisorOptions};
use bl_model::{Command, Device, DeviceKind, Editor, TrackId, TrackKind};

use crate::Outcome;
use crate::devices::{parse_backend, resolve};

/// How long the simulation waits for each change to show.
const PATIENCE: Duration = Duration::from_secs(10);

#[derive(Debug, clap::Args)]
pub(crate) struct DeviceChangesArgs {
    /// Run on the null backend's simulated USB interface and put it through
    /// each device change in turn, checking the engine recovers from each.
    #[arg(long, conflicts_with_all = ["backend", "device", "rate", "seconds"])]
    simulate: bool,

    /// The backend: coreaudio, wasapi, asio, pipewire, jack, alsa, or null.
    /// Defaults to the platform's, or to the one in a device ID.
    #[arg(long)]
    backend: Option<String>,

    /// The device: an ID or index from `bl-cli devices`, or part of its name.
    /// Defaults to the backend's default output, which the stream follows.
    #[arg(long)]
    device: Option<String>,

    /// The sample rate in hertz. Defaults to the rate the device runs at now,
    /// so the device's setting doesn't change.
    #[arg(long)]
    rate: Option<u32>,

    /// The buffer size in frames. Defaults to the device's.
    #[arg(long)]
    buffer: Option<u32>,

    /// How long to play on a real device, in seconds.
    #[arg(long, default_value_t = 30.0)]
    seconds: f64,

    /// The tone's level in dBFS, 0 or lower.
    #[arg(long, default_value_t = -60.0, allow_negative_numbers = true)]
    level: f64,

    /// Play through the system's default output while the chosen device is
    /// missing.
    #[arg(long)]
    fall_back: bool,
}

/// The song: one 440 Hz tone at `level`.
fn song(level: f64) -> (Editor, TrackId) {
    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: "Tone".to_owned(),
            parent: None,
            index: 0,
        })
        .expect("a new track");
    let mut device = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: TEST_TONE.to_owned(),
        },
    );
    device.params.insert(test_tone::LEVEL, level);
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .expect("a device on the new track");
    (editor, track)
}

pub(crate) fn run(args: &DeviceChangesArgs) -> Outcome {
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
    if args.simulate {
        simulate(args)
    } else {
        on_device(args)
    }
}

/// Prints events as they arrive and keeps them.
struct Log {
    began: Instant,
    events: Vec<DeviceEvent>,
}

impl Log {
    fn new() -> Self {
        Self {
            began: Instant::now(),
            events: Vec::new(),
        }
    }

    fn line(&self, text: &str) {
        println!("[{:7.2} s] {text}", self.began.elapsed().as_secs_f64());
    }

    /// Prints and keeps the events since the last look.
    fn take(&mut self, audio: &AudioEngine) {
        for event in audio.events() {
            self.line(&event.message());
            self.events.push(event);
        }
    }

    /// The heap operations of every stream that stopped, and the one
    /// running, with how many streams there were.
    fn heap_operations(&self, now: Option<StreamStats>) -> (u64, usize) {
        let stopped = self.events.iter().filter_map(|event| match event {
            DeviceEvent::Interrupted(interruption) => interruption.stream_stats,
            _ => None,
        });
        let all: Vec<StreamStats> = stopped.chain(now).collect();
        (all.iter().map(|s| s.heap_operations).sum(), all.len())
    }
}

/// One change the simulation puts the interface through.
struct Check {
    name: &'static str,
    outcome: Result<String, String>,
}

fn simulate(args: &DeviceChangesArgs) -> Outcome {
    let (backend, control) = NullBackend::with_devices(
        NullOptions::default(),
        &[("built-in", "Built-in Output"), ("usb", "USB Interface")],
    );
    let request = StreamRequest {
        output: Some(PortRequest {
            device: DeviceSelector::Id(DeviceId::new(BackendKind::Null, "usb")),
            channels: None,
        }),
        input: None,
        sample_rate: None,
        buffer_size: Some(args.buffer.unwrap_or(256)),
    };
    let options = SupervisorOptions {
        poll_interval: Duration::from_millis(10),
        retry_interval: Duration::from_millis(100),
        stall_timeout: Duration::from_millis(500),
        fall_back_to_default: args.fall_back,
    };
    let probe = Arc::new(Probe::default());
    let mut audio = AudioEngine::start_with_power(
        EngineConfig::new(SampleRate::HZ_48000, 256),
        testing::registry(&probe),
        Box::new(move || Ok(Box::new(backend) as Box<dyn Backend>)),
        request,
        options,
        Box::new(control.power()),
    )
    .map_err(|e| e.to_string())?;
    let (editor, tone) = song(args.level);
    audio.engine().sync(editor.document());

    println!(
        "bl-cli device-changes --simulate: a 440 Hz tone at {:.1} dBFS on the null backend's \
         simulated USB interface, through four device changes",
        args.level
    );
    println!();
    let mut log = Log::new();
    let mut sim = Sim {
        audio: &mut audio,
        control: &control,
        log: &mut log,
        tone,
    };
    sim.play()?;

    let checks = vec![
        Check {
            name: "unplug and replug",
            outcome: sim.unplug_and_replug(),
        },
        Check {
            name: "sample-rate change",
            outcome: sim.rate_change(),
        },
        Check {
            name: "sleep and wake",
            outcome: sim.sleep_and_wake(),
        },
        Check {
            name: "stall",
            outcome: sim.stall(),
        },
    ];
    let stats = settled_stats(&audio);
    drop(audio);

    println!();
    println!("Summary");
    let mut failed = false;
    for check in &checks {
        match &check.outcome {
            Ok(text) => println!("  {:<19} ok: {text}", format!("{}:", check.name)),
            Err(text) => {
                failed = true;
                println!("  {:<19} FAILED: {text}", format!("{}:", check.name));
            }
        }
    }
    let (heap, streams) = log.heap_operations(stats);
    println!(
        "  {:<19} {heap} on the audio thread, across {}{}",
        "heap operations:",
        count(streams, "stream"),
        trap_note(stats)
    );
    if heap > 0 {
        failed = true;
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// The running stream's counters once its audio thread has been set up
/// (the supervisor publishes them every poll), or whatever there is after a
/// second.
fn settled_stats(audio: &AudioEngine) -> Option<StreamStats> {
    let began = Instant::now();
    loop {
        let stats = audio.stats();
        if stats.is_none_or(|s| s.audio_thread.is_some())
            || began.elapsed() > Duration::from_secs(1)
        {
            return stats;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn trap_note(stats: Option<StreamStats>) -> &'static str {
    match stats.and_then(|s| s.audio_thread) {
        Some(report) if report.trap_installed => " (allocation trap on)",
        Some(_) => " (allocation trap off: release build)",
        None => "",
    }
}

/// The simulation's steps.
struct Sim<'a> {
    audio: &'a mut AudioEngine,
    control: &'a NullControl,
    log: &'a mut Log,
    tone: TrackId,
}

impl Sim<'_> {
    /// Waits for `done`, printing events meanwhile.
    fn wait(
        &mut self,
        what: &str,
        mut done: impl FnMut(&mut AudioEngine) -> bool,
    ) -> Result<(), String> {
        let began = Instant::now();
        loop {
            self.log.take(self.audio);
            if done(self.audio) {
                return Ok(());
            }
            if began.elapsed() > PATIENCE {
                return Err(format!(
                    "waited {} s for {what}; the device is {:?}",
                    PATIENCE.as_secs(),
                    self.audio.device()
                ));
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Plays the song and waits until the tone sounds.
    fn play(&mut self) -> Result<(), String> {
        self.audio.engine().play();
        let tone = self.tone;
        self.wait("the tone to play", |audio| {
            let state = audio.state();
            state.playing && state.meter(tone).is_some_and(|peak| peak[0] > 0.0)
        })
    }

    /// Waits for an interruption for `cause`, and checks the engine stopped.
    fn interrupted(&mut self, cause: &Cause) -> Result<String, String> {
        self.wait(
            "the interruption",
            |audio| matches!(audio.device(), DeviceStatus::Interrupted(ref i) if i.cause == *cause),
        )?;
        let message = self.audio.message().unwrap_or_default();
        if self.audio.state().playing {
            return Err("the transport still shows playing".to_owned());
        }
        Ok(message)
    }

    /// Waits for a recovery at `rate`, with the engine at that rate and
    /// playing again, and returns how long the interruption lasted.
    fn recovered(&mut self, rate: u32) -> Result<f64, String> {
        self.wait(&format!("audio at {rate} Hz"), |audio| {
            matches!(audio.device(), DeviceStatus::Running(ref info) if info.sample_rate == rate)
                && audio.state().sample_rate.hz() == rate
        })?;
        self.play()?;
        self.log
            .events
            .iter()
            .rev()
            .find_map(|event| match event {
                DeviceEvent::Recovered { after, .. } => Some(after.as_secs_f64()),
                _ => None,
            })
            .ok_or_else(|| "no recovery was reported".to_owned())
    }

    fn unplug_and_replug(&mut self) -> Result<String, String> {
        self.log
            .line("Unplugging the USB interface during playback.");
        self.control.unplug("usb");
        let message = self.interrupted(&Cause::DeviceLost)?;
        thread::sleep(Duration::from_millis(300));
        if self.audio.state().playing {
            return Err("the engine started again with the device gone".to_owned());
        }
        self.log.line("Plugging it back in.");
        self.control.replug("usb");
        let after = self.recovered(48_000)?;
        Ok(format!(
            "showed \"{message}\", and recovered after {after:.2} s"
        ))
    }

    fn rate_change(&mut self) -> Result<String, String> {
        self.log
            .line("Setting the interface to 44100 Hz, as another program might.");
        self.control.set_sample_rate("usb", 44_100);
        let after = self.recovered(44_100)?;
        Ok(format!(
            "the engine recompiled and runs at 44100 Hz, after {after:.2} s"
        ))
    }

    fn sleep_and_wake(&mut self) -> Result<String, String> {
        self.log.line("Putting the computer to sleep.");
        self.control.sleep();
        let message = self.interrupted(&Cause::Sleep)?;
        thread::sleep(Duration::from_millis(300));
        self.log.line("Waking it.");
        self.control.wake();
        let after = self.recovered(44_100)?;
        Ok(format!(
            "showed \"{message}\", and recovered after {after:.2} s"
        ))
    }

    fn stall(&mut self) -> Result<String, String> {
        self.log
            .line("Making the interface stop calling back, without a word.");
        let seen = self.log.events.len();
        self.control.stall();
        // It restarts at once, so the interruption shows only as an event.
        let began = Instant::now();
        while !self.log.events[seen..].iter().any(|event| {
            matches!(
                event,
                DeviceEvent::Recovered {
                    cause: Cause::Stalled,
                    ..
                }
            )
        }) {
            if began.elapsed() > PATIENCE {
                return Err("the stall wasn't noticed".to_owned());
            }
            thread::sleep(Duration::from_millis(5));
            self.log.take(self.audio);
        }
        let after = self.recovered(44_100)?;
        Ok(format!("noticed and restarted after {after:.2} s"))
    }
}

/// Plays on a real device for `--seconds`, printing what happens.
fn on_device(args: &DeviceChangesArgs) -> Outcome {
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
    let device = match &args.device {
        None => DeviceSelector::Default,
        Some(query) => {
            let backend = bl_audio_io::open_backend(kind).map_err(|e| e.to_string())?;
            match (resolve(backend.as_ref(), query), id) {
                (Ok(device), _) => DeviceSelector::Id(device.id),
                // Named by ID but unplugged now: wait for it.
                (Err(_), Some(id)) => DeviceSelector::Id(id),
                (Err(error), None) => return Err(error),
            }
        }
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
    let options = SupervisorOptions {
        fall_back_to_default: args.fall_back,
        ..SupervisorOptions::default()
    };
    let probe = Arc::new(Probe::default());
    let mut audio = AudioEngine::start(
        EngineConfig::new(SampleRate::HZ_48000, 4096),
        testing::registry(&probe),
        Box::new(move || bl_audio_io::open_backend(kind)),
        request,
        options,
    )
    .map_err(|e| e.to_string())?;
    let (editor, tone) = song(args.level);
    audio.engine().sync(editor.document());
    audio.engine().play();

    println!(
        "bl-cli device-changes: a 440 Hz tone at {:.1} dBFS on {kind} for {} s. Unplug and replug \
         the interface, change its sample rate, or sleep the computer, and watch what's reported.",
        args.level, args.seconds
    );
    println!();
    let mut log = Log::new();
    let mut started = false;
    let mut next_status = Duration::from_secs(5);
    let length = Duration::from_secs_f64(args.seconds);
    while log.began.elapsed() < length {
        log.take(&audio);
        started |= log
            .events
            .iter()
            .any(|e| matches!(e, DeviceEvent::Started(_) | DeviceEvent::Recovered { .. }));
        // Play again after each recovery, as a person would.
        if matches!(log.events.last(), Some(DeviceEvent::Recovered { .. }))
            && !audio.state().playing
        {
            audio.engine().play();
        }
        if log.began.elapsed() >= next_status {
            next_status += Duration::from_secs(5);
            let device = audio.device();
            let state = audio.state();
            let peak = state.meter(tone).map_or(0.0, |p| p[0]);
            log.line(&format!(
                "{}; engine at {} Hz, {}, tone peak {}",
                match &device {
                    DeviceStatus::Running(info) => format!(
                        "running on \"{}\"",
                        info.output_device.as_ref().map_or("?", |d| d.name.as_str())
                    ),
                    other => other.message().unwrap_or_else(|| format!("{other:?}")),
                },
                state.sample_rate.hz(),
                if state.playing { "playing" } else { "stopped" },
                level(peak)
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
    log.take(&audio);
    let device = audio.device();
    let stats = settled_stats(&audio);
    drop(audio);

    let interruptions = log
        .events
        .iter()
        .filter(|e| matches!(e, DeviceEvent::Interrupted(_)))
        .count();
    let recoveries = log
        .events
        .iter()
        .filter(|e| matches!(e, DeviceEvent::Recovered { .. }))
        .count();
    let (heap, streams) = log.heap_operations(stats);
    println!();
    println!("Summary");
    println!("  interruptions:   {interruptions}, recovered from {recoveries}");
    println!(
        "  at the end:      {}",
        device.message().unwrap_or_else(|| "running".to_owned())
    );
    println!(
        "  heap operations: {heap} on the audio thread, across {}{}",
        count(streams, "stream"),
        trap_note(stats)
    );
    let mut failed = Vec::new();
    if !started {
        failed.push("the device never ran".to_owned());
    }
    if !matches!(device, DeviceStatus::Running(_)) {
        failed.push("the device wasn't running at the end".to_owned());
    }
    if heap > 0 {
        failed.push(format!("{heap} heap operations on the audio thread"));
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

/// "1 stream", "5 streams".
fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

fn level(peak: f32) -> String {
    if peak > 0.0 {
        format!("{:.1} dBFS", 20.0 * f64::from(peak).log10())
    } else {
        "silence".to_owned()
    }
}

//! The engine on a device through device changes (WP-1.13), under
//! RealtimeSanitizer: a song plays on the null backend's simulated
//! interface, paced in real time, round the Arrangement loop, while the
//! interface is unplugged and plugged back in, changes its sample rate,
//! stalls, and sleeps and wakes. A locate and a loop change for a frame of
//! the engine clock, sent while it's unplugged, land on the new stream's
//! audio thread.
//! Every block on each stream's real-time thread, the hand-off of the
//! engine's callback and its first blocks included, is a real-time section;
//! RTSan stops the process at the first allocation, free, lock, or blocking
//! call in any of them.
//!
//! As in `audio_thread.rs`, this binary doesn't install the allocation trap,
//! so the real-time path touches none of the trap's thread-locals (macOS
//! allocates a thread's thread-local storage the first time it touches one).

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use bl_audio_engine::{AudioEngine, DeviceStatus, SupervisorOptions};
use bl_audio_io::{
    Backend, BackendKind, DeviceId, DeviceSelector, NullBackend, NullControl, NullOptions,
    StreamRequest,
};
use bl_core::time::{SampleRate, Ticks};
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_engine::testing::{self, Probe};
use bl_engine::{EngineConfig, LoopRegion, TransportCommand, When};
use bl_model::{Command, Device, DeviceKind, Editor, TrackKind};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

fn song() -> Editor {
    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let mut device = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: TEST_TONE.to_owned(),
        },
    );
    device.params.insert(test_tone::LEVEL, -60.0);
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    // A loop of a beat from beat 2, so the song wraps every half second.
    editor
        .apply(Command::SetLoop {
            start: Ticks::QUARTER,
            length: Ticks::QUARTER,
        })
        .unwrap();
    editor
        .apply(Command::SetLoopEnabled { enabled: true })
        .unwrap();
    editor
}

/// Waits until the device runs at `rate` and the engine has processed at
/// least `frames` more frames there.
fn plays_at(audio: &mut AudioEngine, rate: u32, frames: u64) {
    let began = Instant::now();
    let mut from = None;
    loop {
        assert!(
            began.elapsed() < Duration::from_secs(20),
            "waited for audio at {rate} Hz; the device is {:?}",
            audio.device()
        );
        let running =
            matches!(audio.device(), DeviceStatus::Running(info) if info.sample_rate == rate);
        let state = audio.state();
        if running && state.sample_rate.hz() == rate {
            let start = *from.get_or_insert(state.clock);
            if state.clock >= start + frames {
                return;
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn interrupted(audio: &AudioEngine) {
    let began = Instant::now();
    while !matches!(audio.device(), DeviceStatus::Interrupted(_)) {
        assert!(began.elapsed() < Duration::from_secs(20), "no interruption");
        thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn device_changes_pass_under_rtsan() {
    let (backend, control): (NullBackend, NullControl) = NullBackend::with_devices(
        NullOptions::default(),
        &[("built-in", "Built-in Output"), ("usb", "USB Interface")],
    );
    let request = StreamRequest {
        buffer_size: Some(64),
        ..StreamRequest::output(DeviceSelector::Id(DeviceId::new(BackendKind::Null, "usb")))
    };
    let options = SupervisorOptions {
        poll_interval: Duration::from_millis(5),
        retry_interval: Duration::from_millis(40),
        stall_timeout: Duration::from_millis(300),
        fall_back_to_default: false,
    };
    let probe = Arc::new(Probe::default());
    let mut audio = AudioEngine::start_with_power(
        EngineConfig::new(SampleRate::HZ_48000, 64),
        testing::registry(&probe),
        Box::new(move || Ok(Box::new(backend) as Box<dyn Backend>)),
        request,
        options,
        Box::new(control.power()),
    )
    .unwrap();
    let editor = song();
    audio.engine().sync(editor.document());
    audio.engine().play();
    plays_at(&mut audio, 48_000, 9_600);

    control.unplug("usb");
    interrupted(&audio);
    // Held while unplugged: a locate, and a loop of a beat from beat 9 for
    // 50 ms after the stream closed.
    let clock = audio.state().clock;
    audio.engine().locate(Ticks::from_quarters(8));
    audio.engine().transport_at(
        TransportCommand::SetLoop(Some(LoopRegion {
            start: Ticks::from_quarters(8),
            end: Ticks::from_quarters(9),
        })),
        When::Frame(clock + 2_400),
    );
    control.replug("usb");
    audio.engine().play();
    plays_at(&mut audio, 48_000, 9_600);
    let state = audio.state();
    assert_eq!(state.insert_marker, Ticks::from_quarters(8));
    assert_eq!(
        state.loop_region.map(|region| region.start),
        Some(Ticks::from_quarters(8))
    );

    control.set_sample_rate("usb", 44_100);
    plays_at(&mut audio, 44_100, 8_820);
    // On round the carried loop at the new rate, for more than one turn.
    audio.engine().transport(TransportCommand::Continue);
    plays_at(&mut audio, 44_100, 26_460);
    assert!(audio.state().playing);

    control.stall();
    thread::sleep(Duration::from_millis(400));
    plays_at(&mut audio, 44_100, 8_820);

    control.sleep();
    interrupted(&audio);
    control.wake();
    plays_at(&mut audio, 44_100, 8_820);

    let restarts = audio.engine().counts().restarts;
    assert!(restarts >= 5, "{restarts} restarts");
    let stats = audio.stats().expect("a running stream");
    assert!(
        stats
            .audio_thread
            .expect("a set-up report")
            .sanitizer_active
    );
}

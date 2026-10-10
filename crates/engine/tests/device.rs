//! The engine on a device through device changes (WP-1.13): a song plays on
//! the null backend's simulated USB interface, which is unplugged and
//! plugged back in, changes sample rate, and sleeps and wakes, as
//! `docs/specs/device-changes.md` lays out. Every block runs on the null
//! device's real-time thread under the allocation trap, and each stream's
//! heap use is checked.

bl_rt::install_trap!();

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use bl_audio_io::{
    Backend, BackendKind, DeviceId, DeviceSelector, NullBackend, NullControl, NullOptions,
    StreamRequest, StreamStats,
};
use bl_core::time::{SampleRate, Ticks};
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_engine::testing::{self, Probe};
use bl_engine::{AudioEngine, Cause, DeviceEvent, DeviceStatus, EngineConfig, SupervisorOptions};
use bl_model::{Command, Device, DeviceKind, Editor, TrackId, TrackKind};

/// A track with a test tone at −60 dBFS.
fn tone_track(editor: &mut Editor, name: &str) -> TrackId {
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: name.to_owned(),
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
    track
}

struct Rig {
    audio: AudioEngine,
    control: NullControl,
    editor: Editor,
    tone: TrackId,
}

/// The engine on the simulated USB interface, beside a built-in output,
/// playing one tone, with the null control's power events.
fn rig() -> Rig {
    let (backend, control) = NullBackend::with_devices(
        NullOptions::default(),
        &[("built-in", "Built-in Output"), ("usb", "USB Interface")],
    );
    let request = StreamRequest {
        buffer_size: Some(128),
        ..StreamRequest::output(DeviceSelector::Id(DeviceId::new(BackendKind::Null, "usb")))
    };
    let options = SupervisorOptions {
        poll_interval: Duration::from_millis(5),
        retry_interval: Duration::from_millis(40),
        stall_timeout: Duration::from_millis(500),
        fall_back_to_default: false,
    };
    let probe = Arc::new(Probe::default());
    let mut audio = AudioEngine::start_with_power(
        EngineConfig::new(SampleRate::HZ_48000, 128),
        testing::registry(&probe),
        Box::new(move || Ok(Box::new(backend) as Box<dyn Backend>)),
        request,
        options,
        Box::new(control.power()),
    )
    .unwrap();
    let mut editor = Editor::new();
    let tone = tone_track(&mut editor, "Tone");
    audio.engine().sync(editor.document());
    audio.engine().play();
    wait(&mut audio, "the tone to play", |audio| {
        let state = audio.state();
        state.playing && state.meter(tone).is_some_and(|peak| peak[0] > 0.0)
    });
    Rig {
        audio,
        control,
        editor,
        tone,
    }
}

fn wait(audio: &mut AudioEngine, what: &str, mut done: impl FnMut(&mut AudioEngine) -> bool) {
    let began = Instant::now();
    while !done(audio) {
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "waited for {what}; the device is {:?}",
            audio.device()
        );
        thread::sleep(Duration::from_millis(2));
    }
}

/// Whether the engine's clock moves within a second.
fn processing(audio: &mut AudioEngine) -> bool {
    let before = audio.state().clock;
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(1) {
        if audio.state().clock > before + 1_000 {
            return true;
        }
        thread::sleep(Duration::from_millis(2));
    }
    false
}

fn running(audio: &mut AudioEngine, rate: u32) {
    wait(
        audio,
        &format!("the device at {rate} Hz"),
        |audio| matches!(audio.device(), DeviceStatus::Running(info) if info.sample_rate == rate),
    );
}

/// No heap use on the audio thread, in the stream that stopped and in the
/// one running now.
fn clean(stopped: Option<StreamStats>, audio: &AudioEngine) {
    let stopped = stopped.expect("the stopped stream's counters");
    assert!(stopped.frames > 0);
    assert_eq!(stopped.heap_operations, 0);
    let now = audio.stats().expect("a running stream");
    assert_eq!(now.heap_operations, 0);
}

// Spec: device-changes R1, R2, R7
#[test]
fn unplugging_during_playback_reports_and_recovers_on_replug() {
    let Rig {
        mut audio,
        control,
        mut editor,
        tone,
    } = rig();
    assert_eq!(audio.message(), None);

    control.unplug("usb");
    wait(&mut audio, "the interruption", |audio| {
        matches!(audio.device(), DeviceStatus::Interrupted(_))
    });
    let DeviceStatus::Interrupted(interruption) = audio.device() else {
        unreachable!();
    };
    assert_eq!(interruption.cause, Cause::DeviceLost);
    assert_eq!(
        audio.message().unwrap(),
        "The audio device \"USB Interface\" was disconnected. Audio is off until it's connected again."
    );
    // The engine stopped cleanly: the transport shows stopped where it was.
    let state = audio.state().clone();
    assert!(!state.playing);
    assert!(state.position_ticks > Ticks::ZERO);
    let stopped_at = state.position_ticks;
    assert_eq!(state.meter(tone), Some([0.0; 2]));
    assert!(!processing(&mut audio));

    // An edit while it's unplugged is kept, and plays once it's back.
    let second = tone_track(&mut editor, "Second tone");
    audio.engine().sync(editor.document());
    audio.engine().play();
    assert!(audio.engine().flush(Duration::from_secs(1)));

    control.replug("usb");
    running(&mut audio, 48_000);
    assert_eq!(audio.message(), None);
    assert!(processing(&mut audio));
    let state = audio.state().clone();
    assert!(
        !state.playing,
        "play while unplugged doesn't start the song"
    );
    // Where it stopped, to the frame (29.4 ticks at 48 kHz and 120 bpm).
    assert!(
        (state.position_ticks.get() - stopped_at.get()).abs() <= 30,
        "the song is where it stopped: {:?}, not {stopped_at:?}",
        state.position_ticks
    );
    wait(&mut audio, "both tones", |audio| {
        audio
            .state()
            .meter(second)
            .is_some_and(|peak| peak[0] > 0.0)
    });
    audio.engine().play();
    wait(&mut audio, "the song to play on", |audio| {
        audio.state().playing
    });

    let events = audio.events();
    assert!(matches!(&events[0], DeviceEvent::Started(_)));
    assert!(matches!(&events[1], DeviceEvent::Interrupted(i) if i.cause == Cause::DeviceLost));
    assert!(matches!(
        &events[2],
        DeviceEvent::Recovered {
            cause: Cause::DeviceLost,
            ..
        }
    ));
    clean(interruption.stream_stats, &audio);
    assert_eq!(audio.engine().counts().restarts, 2);
}

// Spec: device-changes R3
#[test]
fn a_sample_rate_change_recompiles_at_the_new_rate() {
    let Rig {
        mut audio,
        control,
        tone,
        ..
    } = rig();
    let before = audio.state().position_ticks;
    control.set_sample_rate("usb", 44_100);
    running(&mut audio, 44_100);
    wait(&mut audio, "the engine at 44.1 kHz", |audio| {
        let state = audio.state();
        state.sample_rate == SampleRate::HZ_44100
            && state.meter(tone).is_some_and(|peak| peak[0] > 0.0)
    });
    assert!(processing(&mut audio));
    assert!(audio.state().position_ticks >= before);
    assert_eq!(audio.request().sample_rate, None, "it follows the device");
    let events = audio.events();
    let DeviceEvent::Interrupted(interruption) = &events[1] else {
        panic!("expected an interruption, not {events:?}");
    };
    assert_eq!(
        interruption.cause,
        Cause::SampleRateChanged {
            from: 48_000,
            to: 44_100
        }
    );
    assert!(matches!(&events[2], DeviceEvent::Recovered { .. }));
    clean(interruption.stream_stats, &audio);
}

// Spec: device-changes R4
#[test]
fn sleep_and_wake_stop_and_restart_the_engine() {
    let Rig {
        mut audio, control, ..
    } = rig();
    control.sleep();
    wait(
        &mut audio,
        "the stop for sleep",
        |audio| matches!(audio.device(), DeviceStatus::Interrupted(i) if i.cause == Cause::Sleep),
    );
    assert!(!audio.state().playing);
    assert_eq!(
        audio.message().unwrap(),
        "Audio stopped while the computer slept, and restarts when it wakes."
    );
    assert!(!processing(&mut audio));
    control.wake();
    running(&mut audio, 48_000);
    assert!(processing(&mut audio));
    let events = audio.events();
    let DeviceEvent::Interrupted(interruption) = &events[1] else {
        panic!("expected an interruption, not {events:?}");
    };
    assert!(matches!(&events[2], DeviceEvent::Woke { .. }));
    assert!(matches!(
        &events[3],
        DeviceEvent::Recovered {
            cause: Cause::Sleep,
            ..
        }
    ));
    clean(interruption.stream_stats, &audio);
}

#[test]
fn reconfiguring_moves_the_engine_to_another_device_and_rate() {
    let Rig {
        mut audio, tone, ..
    } = rig();
    let info = audio
        .reconfigure(StreamRequest {
            buffer_size: Some(256),
            sample_rate: Some(96_000),
            ..StreamRequest::output(DeviceSelector::Id(DeviceId::new(
                BackendKind::Null,
                "built-in",
            )))
        })
        .unwrap();
    assert_eq!(info.sample_rate, 96_000);
    wait(&mut audio, "the engine at 96 kHz", |audio| {
        let state = audio.state();
        state.sample_rate == SampleRate::HZ_96000
            && state.meter(tone).is_some_and(|peak| peak[0] > 0.0)
    });
    assert!(processing(&mut audio));
}

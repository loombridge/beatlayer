//! WP-1.7's acceptance: device and buffer changes from the Settings window's
//! Audio page apply without a restart. Driven headless through the
//! `audio.configure` command, on the null device and on a stand-in backend
//! with two devices.
//!
//! Spec: app-shell R6, R7, R8.

use std::time::{Duration, Instant};

use bl_audio_io::{
    AudioCallback, Backend, BackendKind, BufferSizes, DeviceId, DeviceInfo, DeviceSelector, Error,
    ErrorKind, NullBackend, OpenDevice, PortRequest, STANDARD_SAMPLE_RATES, Stream, StreamInfo,
    StreamRequest, StreamStats,
};
use bl_shell::audio::{AudioHost, AudioSettings, AudioStatus};
use bl_shell::files::NoSetFiles;
use bl_shell::settings::Preferences;
use bl_shell::{Effect, Shell};
use serde_json::{Value, json};

/// Polls the shell's audio status until `done` holds, for up to 5 seconds.
fn wait_for(shell: &mut Shell, done: impl Fn(&AudioStatus) -> bool) -> AudioStatus {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let status: AudioStatus =
            serde_json::from_value(shell.run("audio.status", json!({})).unwrap()).unwrap();
        if done(&status) || Instant::now() > until {
            return status;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn status(value: Value) -> AudioStatus {
    serde_json::from_value(value).expect("audio.configure returns the status")
}

#[test]
fn a_buffer_change_applies_to_the_running_stream_without_a_restart() {
    let mut shell = Shell::headless();
    shell.start_audio();
    let first = wait_for(&mut shell, |s| s.buffer_size == Some(256));
    assert!(first.engine_on);
    assert_eq!(first.driver, "null");
    assert_eq!(first.buffer_size, Some(256), "the null device's default");
    assert_eq!(first.sample_rate, Some(48_000));

    for frames in [128, 64, 1024] {
        let applied = status(
            shell
                .run("audio.configure", json!({ "buffer_size": frames }))
                .unwrap(),
        );
        assert_eq!(applied.requested_buffer_size, Some(frames));
        let running = wait_for(&mut shell, |s| s.buffer_size == Some(frames));
        assert_eq!(
            running.buffer_size,
            Some(frames),
            "the device now delivers {frames}-frame buffers"
        );
        assert!(shell.audio().is_running());
    }
    // The same shell kept running throughout: no restart, and the settings
    // are the preferences now.
    assert_eq!(shell.preferences().audio.buffer_size, Some(1024));
}

#[test]
fn rate_and_channel_changes_apply_too() {
    let mut shell = Shell::headless();
    shell.start_audio();
    let applied = status(
        shell
            .run(
                "audio.configure",
                json!({ "sample_rate": 96_000, "output_channels": 1 }),
            )
            .unwrap(),
    );
    assert_eq!(applied.sample_rate, Some(96_000));
    assert_eq!(applied.output_channels, 1);
    let back = status(
        shell
            .run(
                "audio.configure",
                json!({ "sample_rate": 0, "output_channels": 0 }),
            )
            .unwrap(),
    );
    assert_eq!(
        back.sample_rate,
        Some(48_000),
        "0 means the device's own rate"
    );
    assert_eq!(back.output_channels, 2);
}

#[test]
fn a_refused_setting_keeps_the_previous_one_running() {
    let mut shell = Shell::headless();
    shell.start_audio();
    shell
        .run("audio.configure", json!({ "buffer_size": 512 }))
        .unwrap();
    // The null device has two channels, so eight are refused.
    let error = shell
        .run("audio.configure", json!({ "output_channels": 8 }))
        .unwrap_err()
        .to_string();
    assert!(error.contains("8"), "{error}");
    let status = wait_for(&mut shell, |s| s.buffer_size == Some(512));
    assert!(shell.audio().is_running(), "the previous stream reopened");
    assert_eq!(status.output_channels, 2);
    assert_eq!(status.buffer_size, Some(512));
    assert_eq!(status.settings.output_channels, None);
    assert_eq!(status.error, None);

    let error = shell
        .run("audio.configure", json!({ "driver": "dsound" }))
        .unwrap_err()
        .to_string();
    assert!(error.contains("isn't a driver type"), "{error}");
}

#[test]
fn the_engine_switch_closes_and_reopens_the_device() {
    let mut shell = Shell::headless();
    shell.start_audio();
    assert!(shell.audio().is_running());
    let off = shell.invoke("audio.toggle_engine", json!({})).unwrap();
    assert_eq!(off, json!({ "on": false }));
    assert!(!shell.audio().is_running());
    assert_eq!(
        shell.audio_mut().sample_cpu_load(),
        None,
        "the meter reads OFF"
    );
    let message = shell
        .message()
        .expect("the Status Bar says the engine is off");
    assert_eq!(
        message.action.as_ref().map(|a| a.id.as_str()),
        Some("app.settings")
    );
    // A change while the engine is off is kept, and used when it's back on.
    shell
        .run("audio.configure", json!({ "buffer_size": 128 }))
        .unwrap();
    shell.invoke("audio.toggle_engine", json!({})).unwrap();
    assert!(shell.audio().is_running());
    assert!(shell.message().is_none());
    let status = wait_for(&mut shell, |s| s.buffer_size == Some(128));
    assert_eq!(status.buffer_size, Some(128));
}

#[test]
fn the_cpu_meter_measures_the_engine() {
    let mut shell = Shell::headless();
    shell.start_audio();
    std::thread::sleep(Duration::from_millis(50));
    let load = shell.audio_mut().sample_cpu_load().expect("a reading");
    assert!((0.0..100.0).contains(&load), "{load}");
}

#[test]
fn the_test_tone_switch_reaches_the_engine_and_stops_with_the_window() {
    // Spec: app-shell R13.
    let mut shell = Shell::headless();
    shell.start_audio();
    shell
        .invoke("app.settings", json!({ "page": "audio" }))
        .unwrap();
    assert!(shell.take_effects().contains(&Effect::OpenSettings));
    shell
        .run("audio.set_test_tone", json!({ "on": true }))
        .unwrap();
    assert!(shell.audio().test_tone());
    shell.invoke("window.close", json!({})).unwrap();
    assert!(!shell.audio().test_tone());
    assert!(shell.take_effects().contains(&Effect::CloseSettings));
}

/// A backend with two devices, `null:a` and `null:b`, each of which runs on
/// the null device underneath and reports itself as the device it stands
/// for.
struct TwoDevices;

impl TwoDevices {
    fn info(local: &str, name: &str) -> DeviceInfo {
        DeviceInfo {
            id: DeviceId::new(BackendKind::Null, local),
            name: name.to_owned(),
            is_default_output: local == "a",
            is_default_input: false,
            output_channels: 2,
            input_channels: 0,
            sample_rates: STANDARD_SAMPLE_RATES.to_vec(),
            current_sample_rate: Some(48_000),
            buffer_sizes: BufferSizes::Range { min: 16, max: 8192 },
            default_buffer_size: Some(256),
        }
    }
}

struct Renamed {
    inner: Box<dyn Stream>,
    info: StreamInfo,
}

impl Stream for Renamed {
    fn info(&self) -> &StreamInfo {
        &self.info
    }
    fn start(&mut self) -> Result<(), Error> {
        self.inner.start()
    }
    fn stop(&mut self) -> Result<(), Error> {
        self.inner.stop()
    }
    fn stats(&self) -> StreamStats {
        self.inner.stats()
    }
    fn current_buffer_size(&self) -> Option<u32> {
        self.inner.current_buffer_size()
    }
}

impl Backend for TwoDevices {
    fn kind(&self) -> BackendKind {
        BackendKind::Null
    }

    fn devices(&self) -> Result<Vec<DeviceInfo>, Error> {
        Ok(vec![
            Self::info("a", "Device A"),
            Self::info("b", "Device B"),
        ])
    }

    fn open_stream(
        &self,
        request: &StreamRequest,
        callback: Box<dyn AudioCallback>,
    ) -> Result<Box<dyn Stream>, Error> {
        let output = request.output.as_ref().expect("an output");
        let device = match &output.device {
            DeviceSelector::Default => Self::info("a", "Device A"),
            DeviceSelector::Id(id) => self
                .devices()?
                .into_iter()
                .find(|d| d.id == *id)
                .ok_or_else(|| Error::new(ErrorKind::DeviceNotFound, format!("no {id}")))?,
        };
        let inner_request = StreamRequest {
            output: Some(PortRequest {
                device: DeviceSelector::Id(NullBackend::device_id()),
                channels: output.channels,
            }),
            ..request.clone()
        };
        let inner = NullBackend::new().open_stream(&inner_request, callback)?;
        let mut info = inner.info().clone();
        info.output_device = Some(OpenDevice {
            id: device.id.clone(),
            name: device.name,
        });
        info.follows_default = matches!(output.device, DeviceSelector::Default);
        Ok(Box::new(Renamed { inner, info }))
    }
}

fn two_device_shell() -> Shell {
    let audio = AudioHost::with_backends(
        vec![BackendKind::Null],
        Box::new(|_| Ok(Box::new(TwoDevices))),
        AudioSettings::default(),
    );
    let mut shell = Shell::new(Preferences::default(), None, audio, Box::new(NoSetFiles));
    shell.start_audio();
    shell
}

#[test]
fn a_device_change_applies_without_a_restart() {
    let mut shell = two_device_shell();
    let devices = shell.run("audio.devices", json!({})).unwrap();
    let ids: Vec<&str> = devices["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["null:a", "null:b"]);

    let first = wait_for(&mut shell, |s| s.output_device.is_some());
    assert_eq!(first.output_device.unwrap().name, "Device A");
    assert!(first.follows_default);

    let switched = status(
        shell
            .run(
                "audio.configure",
                json!({ "output_device": "null:b", "buffer_size": 128 }),
            )
            .unwrap(),
    );
    assert_eq!(switched.output_device.unwrap().id, "null:b");
    assert!(!switched.follows_default);
    let running = wait_for(&mut shell, |s| s.buffer_size == Some(128));
    assert_eq!(running.buffer_size, Some(128));
    assert!(shell.audio().is_running());

    let back = status(
        shell
            .run("audio.configure", json!({ "output_device": "default" }))
            .unwrap(),
    );
    assert_eq!(back.output_device.unwrap().id, "null:a");
    assert_eq!(back.settings.output_device, None);

    let error = shell
        .run("audio.configure", json!({ "output_device": "null:c" }))
        .unwrap_err()
        .to_string();
    assert!(error.contains("null:c"), "{error}");
    let kept = wait_for(&mut shell, |s| s.output_device.is_some());
    assert_eq!(
        kept.output_device.unwrap().id,
        "null:a",
        "the previous device reopened"
    );
}

/// On this Mac's real output device, playing silence: the buffer size
/// changes while the stream runs. Not run by default (CI has no device):
/// `cargo test -p bl-shell --test audio_settings -- --ignored`.
#[test]
#[ignore = "needs a real audio output device"]
fn on_the_default_device_a_buffer_change_applies_without_a_restart() {
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::new(AudioSettings::default()),
        Box::new(NoSetFiles),
    );
    shell.start_audio();
    let first = wait_for(&mut shell, |s| s.buffer_size.is_some());
    println!(
        "opened: driver {}, device {:?}, {} Hz, {:?} frames",
        first.driver,
        first.output_device.as_ref().map(|d| &d.name),
        first.sample_rate.unwrap_or_default(),
        first.buffer_size
    );
    for frames in [128u32, 512, 256] {
        shell
            .run("audio.configure", json!({ "buffer_size": frames }))
            .unwrap();
        let running = wait_for(&mut shell, |s| s.buffer_size == Some(frames));
        println!(
            "requested {frames}: the device delivers {:?} frames at {:?} Hz, {} dropouts, cpu {:?}%",
            running.buffer_size,
            running.sample_rate,
            running.dropouts,
            shell.audio_mut().sample_cpu_load()
        );
        assert_eq!(running.buffer_size, Some(frames));
    }
    // Leave the device as it was.
    shell
        .run("audio.configure", json!({ "buffer_size": 0 }))
        .unwrap();
}

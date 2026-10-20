//! Real-time export with external hardware (WP-5.35; the external devices
//! spec, R22 to R26): an export whose path runs through the hardware
//! renders in real time, on the running engine, and the file holds exactly
//! the range's frames.

use std::path::PathBuf;

use bl_devices::hardware_effect_type;
use bl_model::{Channel, DeviceId, TrackId};
use bl_audio_io::{BackendKind, NullBackend};
use bl_shell::audio::{AudioHost, AudioSettings};
use bl_shell::export_commands::{self as export_ids, ExportResult};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::Shell;
use serde_json::{Value, json};

/// A folder for the test's files.
struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let folder = std::env::temp_dir().join(format!(
            "bl-realtime-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        Self(folder)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A shell on a null device with a loopback of 1,000 frames, reporting 400
/// frames of input latency and 600 of output latency: the device's round
/// trip is the loopback's delay.
fn shell_on_loopback() -> Shell {
    let backend = NullBackend::default().with_latency(400, 600).with_loopback(1_000);
    let devices = backend.clone();
    let audio = AudioHost::with_backends(
        vec![BackendKind::Null],
        Box::new(move |_| Ok(Box::new(devices.clone()))),
        AudioSettings::default(),
    );
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        audio,
        Box::new(ProjectFiles::new()),
    );
    shell
        .run("audio.configure", json!({ "input_device": "null:null" }))
        .unwrap();
    shell.start_audio();
    shell
}

/// A tone of `frames` frames, as a WAV file.
fn tone(path: &PathBuf, frames: u32) {
    let data = frames * 2;
    let mut bytes = Vec::with_capacity(44 + data as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let sample: i16 = if i % 100 < 50 { 8_000 } else { -8_000 };
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// A track playing a tone of one beat, with an external effect on it that
/// sends to the interface's outputs and hears its inputs back.
fn set_with_effect(shell: &mut Shell, folder: &TestDir) -> TrackId {
    let file = folder.path("Tone.wav");
    tone(&file, 24_000);
    shell
        .run(
            bl_shell::browser_commands::ADD_TO_ARRANGEMENT,
            json!({ "paths": [file.to_string_lossy()], "at": 0 }),
        )
        .unwrap();
    // The effect, on the track that plays the tone.
    let first = shell.editor().document().top_level().next().unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [first] }))
        .unwrap();
    let kind = hardware_effect_type();
    let added: Value = shell
        .run(
            "device.add",
            json!({ "device": { "kind": kind.kind, "name": kind.name, "role": kind.role } }),
        )
        .unwrap();
    let device: DeviceId = serde_json::from_value(added["device"].clone()).unwrap();
    shell
        .run(
            "device.set_external",
            json!({ "device": device,
                    "audio_to": { "type": "pair", "channel": 1 },
                    "audio_from": { "type": "pair", "channel": 1 } }),
        )
        .unwrap();
    first
}

/// The frames of a WAV file's data.
fn frames_of(path: &PathBuf) -> usize {
    let bytes = std::fs::read(path).unwrap();
    let data = &bytes[44..];
    data.len() / 4
}

// Spec: external-devices R22, R24
#[test]
fn an_export_with_external_hardware_renders_in_real_time() {
    let folder = TestDir::new("realtime");
    let mut shell = shell_on_loopback();
    set_with_effect(&mut shell, &folder);

    // The export renders in real time, and the file holds exactly the
    // range's frames: the clip is a beat, which is 24,000 frames at
    // 48 kHz.
    let out = folder.path("Live.wav");
    let result: Value = shell
        .run(
            export_ids::AUDIO,
            json!({ "path": out.to_string_lossy(), "bit_depth": 24,
                    "sample_rate": 48000, "wait_seconds": 0 }),
        )
        .unwrap();
    let result: ExportResult = serde_json::from_value(result).unwrap();
    assert!(result.real_time, "the export rendered in real time");
    assert_eq!(result.attempts, 1, "it took one attempt");
    assert!(out.exists(), "the file is there");
    assert_eq!(
        frames_of(&out),
        24_000,
        "the file holds exactly the range's frames"
    );
    // The song is stopped where the range ended.
    assert!(!shell.audio_mut().transport_state().playing);
}

// Spec: external-devices R22. The outputs whose path runs through the
// hardware render in real time; the others render offline.
#[test]
fn a_track_without_the_hardware_renders_offline() {
    let folder = TestDir::new("offline");
    let mut shell = shell_on_loopback();
    set_with_effect(&mut shell, &folder);
    // A second track, with no hardware on its path.
    shell
        .invoke("track.create", json!({ "kind": "audio", "name": "Other" }))
        .unwrap();

    let out = folder.path("Both.wav");
    let result: Value = shell
        .run(
            export_ids::AUDIO,
            json!({ "path": out.to_string_lossy(), "bit_depth": 24,
                    "sample_rate": 48000, "wait_seconds": 0,
                    "rendered_track": "all_tracks" }),
        )
        .unwrap();
    let result: ExportResult = serde_json::from_value(result).unwrap();
    assert!(result.real_time, "the track with the hardware rendered in real time");
    assert!(result.files.len() > 1, "every track was written");
    let other = result
        .files
        .iter()
        .find(|file| file.contains("Other"))
        .expect("the other track's file");
    assert!(
        PathBuf::from(other).exists(),
        "the other track's file is there: {other}"
    );
}

// Spec: external-devices R26. Without an audio device, an export that
// needs real time is refused before anything renders.
#[test]
fn without_an_audio_device_it_is_refused() {
    let folder = TestDir::new("nodevice");
    let mut shell = Shell::headless();
    set_with_effect(&mut shell, &folder);
    let out = folder.path("None.wav");
    let result = shell.run(
        export_ids::AUDIO,
        json!({ "path": out.to_string_lossy(), "bit_depth": 24,
                "sample_rate": 48000, "wait_seconds": 0 }),
    );
    assert!(result.is_err(), "the export is refused");
    assert!(!out.exists(), "nothing was written");
}

// The channel chooser of the effect lists the interface's outputs.
#[test]
fn the_effects_audio_to_lists_the_interface() {
    let mut shell = Shell::headless();
    set_with_effect(&mut shell, &TestDir::new("choosers"));
    let device = shell
        .editor()
        .document()
        .tracks()
        .flat_map(|track| track.devices_deep())
        .find(|device| device.external.is_some())
        .unwrap();
    let external = device.external.clone().unwrap();
    assert_eq!(external.audio_to, Some(Channel::Pair(1)));
}

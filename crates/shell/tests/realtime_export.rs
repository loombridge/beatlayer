//! Real-time export with external hardware (WP-5.35; the external devices
//! spec, R22 to R26): an export whose path runs through the hardware
//! renders in real time, on the running engine, and the file holds exactly
//! the range's frames.

use std::path::PathBuf;
use std::thread;

use bl_audio_io::{BackendKind, NullBackend, NullControl, NullOptions, Pacing};
use bl_core::time::Ticks;
use bl_devices::hardware_effect_type;
use bl_model::{Channel, DeviceId, TrackId};
use bl_shell::audio::{AudioHost, AudioSettings};
use bl_shell::export_commands::{self as export_ids, ExportResult};
use bl_shell::files::ProjectFiles;
use bl_shell::realtime::Phase;
use bl_shell::settings::Preferences;
use bl_shell::{Shell, ShellError};
use serde_json::{Value, json};

/// A folder for the test's files.
struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let folder =
            std::env::temp_dir().join(format!("bl-realtime-{name}-{}", std::process::id()));
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
    let backend = NullBackend::default()
        .with_latency(400, 600)
        .with_loopback(1_000);
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

/// A shell on a null device paced by hand, with a loopback of 1,000 frames
/// and the control that runs its blocks. The device reports a dropout after
/// `dropout_after` blocks, as one that underruns does.
fn shell_on_manual_loopback(dropout_after: Option<u64>) -> (Shell, NullControl) {
    let (backend, control) = NullBackend::with_devices(
        NullOptions {
            pacing: Pacing::Manual,
            dropout_after,
            ..NullOptions::default()
        },
        &[("null", "Null")],
    );
    let backend = backend.with_latency(400, 600).with_loopback(1_000);
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
    (shell, control)
}

/// Runs the device's blocks on a thread of their own, until the render has
/// finished: the export runs on the test's thread, and the device's blocks
/// are what it plays. The control comes back with the thread, so a test can
/// run the blocks itself again.
fn drive(
    control: NullControl,
    session: bl_shell::realtime::Session,
) -> thread::JoinHandle<NullControl> {
    thread::spawn(move || {
        let mut rendering = false;
        for _ in 0..400 {
            control.run_blocks(10);
            match session.status().phase {
                Phase::Idle if rendering => break,
                Phase::Idle => {}
                _ => rendering = true,
            }
            thread::yield_now();
        }
        control
    })
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

/// The frames of a WAV file, as its header says.
fn frames_of(path: &PathBuf) -> u64 {
    bl_media::probe(path)
        .expect("the file is a WAV")
        .frames
        .expect("the file's header says how many frames it holds")
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
    // The song is stopped where the range ended: the render's end isn't
    // followed by a second Stop, which would send the playhead to the start.
    let state = shell.audio_mut().transport_state();
    assert!(!state.playing);
    assert_eq!(
        state.position,
        Ticks::QUARTER,
        "the song stays at the range's end"
    );
    assert_eq!(
        state.insert_marker,
        Ticks::QUARTER,
        "and so does the insert marker"
    );
    // Spec: external-devices R29. The render's blocks touch no heap: the
    // audio thread copies into rings the export made before it started.
    let stats = shell.audio().stats().expect("a stream is open");
    assert!(stats.blocks > 0);
    assert_eq!(stats.heap_operations, 0, "{stats:?}");
}

// Spec: external-devices R24. The render's plan goes when the render ends,
// so the song plays again from where it stopped.
#[test]
fn the_song_plays_again_after_a_render() {
    let folder = TestDir::new("again");
    let (mut shell, control) = shell_on_manual_loopback(None);
    set_with_effect(&mut shell, &folder);
    let session = shell.realtime_session();
    let driver = drive(control, session);

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
    let control = driver.join().unwrap();

    // The song plays on from the range's end: the render's plan is gone, so
    // nothing stops it at the range's first frame.
    let before = shell.audio_mut().transport_state().position;
    shell.run("transport.play", json!({})).unwrap();
    bl_shell::testing::run_blocks(&control, &mut shell, 4);
    let after = shell.audio_mut().transport_state().position;
    assert!(
        after > before,
        "the song plays again: {before:?} to {after:?}"
    );
}

// Spec: external-devices R26. A real-time render at a rate below the
// device's is converted down, and Normalize's second pass reads the frames
// the conversion wrote.
#[test]
fn a_realtime_render_converts_the_rate_and_normalises() {
    let folder = TestDir::new("convert");
    let mut shell = shell_on_loopback();
    set_with_effect(&mut shell, &folder);

    let out = folder.path("Lower.wav");
    let result: Value = shell
        .run(
            export_ids::AUDIO,
            json!({ "path": out.to_string_lossy(), "bit_depth": 24,
                    "sample_rate": 44100, "wait_seconds": 0, "normalize": true }),
        )
        .unwrap();
    let result: ExportResult = serde_json::from_value(result).unwrap();
    assert!(result.real_time, "the export rendered in real time");
    assert!(out.exists(), "the file is there: {result:?}");
    let frames = frames_of(&out);
    let expected = 24_000 * 44_100 / 48_000;
    assert!(
        frames.abs_diff(expected) <= 1,
        "the file holds the range at the file's rate: {frames} frames"
    );
}

// Spec: external-devices R24. The render turns the Arrangement loop off to
// play the range, and puts the set's loop back when it ends.
#[test]
fn the_arrangement_loop_returns_after_a_render() {
    let folder = TestDir::new("loop");
    let (mut shell, control) = shell_on_manual_loopback(None);
    set_with_effect(&mut shell, &folder);
    // A loop over the first bar, which the one-beat range lies inside.
    let bar = Ticks::QUARTER.get() * 4;
    shell
        .run("transport.set_loop", json!({ "start": 0, "length": bar }))
        .unwrap();
    shell
        .run("transport.set_loop_enabled", json!({ "enabled": true }))
        .unwrap();
    let session = shell.realtime_session();
    let driver = drive(control, session);

    let out = folder.path("Loop.wav");
    let result: Value = shell
        .run(
            export_ids::AUDIO,
            json!({ "path": out.to_string_lossy(), "bit_depth": 24,
                    "sample_rate": 48000, "wait_seconds": 0 }),
        )
        .unwrap();
    let result: ExportResult = serde_json::from_value(result).unwrap();
    assert!(result.real_time, "the export rendered in real time");
    let control = driver.join().unwrap();

    // Play from the start for more than the loop's bar: the playhead wraps.
    shell.run("transport.stop", json!({})).unwrap();
    shell.run("transport.stop", json!({})).unwrap();
    shell.run("transport.play", json!({})).unwrap();
    bl_shell::testing::run_blocks(&control, &mut shell, 2_000);
    let position = shell.audio_mut().transport_state().position;
    assert!(
        position < Ticks(bar),
        "the loop plays again after the render: {position:?}"
    );
}

// Spec: external-devices R26. A real-time render at a rate above the
// device's is converted up, and the file's header and length agree.
#[test]
fn a_realtime_render_converts_the_rate_up() {
    let folder = TestDir::new("convert-up");
    let mut shell = shell_on_loopback();
    set_with_effect(&mut shell, &folder);

    let out = folder.path("Higher.wav");
    let result: Value = shell
        .run(
            export_ids::AUDIO,
            json!({ "path": out.to_string_lossy(), "bit_depth": 24,
                    "sample_rate": 96000, "wait_seconds": 0 }),
        )
        .unwrap();
    let result: ExportResult = serde_json::from_value(result).unwrap();
    assert!(result.real_time, "the export rendered in real time");
    let frames = frames_of(&out);
    assert!(
        frames.abs_diff(24_000 * 96_000 / 48_000) <= 2,
        "the file holds the range at the file's rate: {frames} frames"
    );
    let header = std::fs::read(&out).unwrap();
    assert_eq!(
        u32::from_le_bytes(header[24..28].try_into().unwrap()),
        96_000,
        "the header's rate"
    );
    assert!(
        (result.seconds - 0.5).abs() < 1e-3,
        "the length is the range's: {}",
        result.seconds
    );
}

// Spec: external-devices R24. A render in real time has no tail and no
// returns, so an export that asks for either is refused, and writes nothing.
#[test]
fn render_as_loop_and_returns_are_refused_in_real_time() {
    let folder = TestDir::new("refused");
    let mut shell = shell_on_loopback();
    set_with_effect(&mut shell, &folder);
    for (switch, name) in [
        ("render_as_loop", "Render as Loop"),
        (
            "include_return_and_main_effects",
            "Include Return and Main Effects",
        ),
    ] {
        let out = folder.path("Refused.wav");
        let mut args = json!({ "path": out.to_string_lossy(), "bit_depth": 24,
                "sample_rate": 48000, "wait_seconds": 0,
                "rendered_track": "all_tracks" });
        args[switch] = json!(true);
        match shell.run(export_ids::AUDIO, args) {
            Err(ShellError::InvalidArgs { message, .. }) => {
                assert!(message.contains(name), "{message}");
            }
            other => panic!("{name} is refused, got {other:?}"),
        }
        assert!(
            std::fs::read_dir(&folder.0).unwrap().all(|entry| {
                let name = entry.unwrap().file_name();
                name.to_string_lossy().starts_with("Tone")
            }),
            "nothing was written"
        );
    }
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
    assert!(
        result.real_time,
        "the track with the hardware rendered in real time"
    );
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

// Spec: external-devices R25. A dropout starts the render again, and the
// attempt count says so.
#[test]
fn a_dropout_starts_the_render_again() {
    let folder = TestDir::new("dropout");
    let (mut shell, control) = shell_on_manual_loopback(Some(20));
    set_with_effect(&mut shell, &folder);
    let session = shell.realtime_session();
    let driver = drive(control, session);

    let out = folder.path("Live.wav");
    let result: Value = shell
        .run(
            export_ids::AUDIO,
            json!({ "path": out.to_string_lossy(), "bit_depth": 24,
                    "sample_rate": 48000, "wait_seconds": 0 }),
        )
        .unwrap();
    driver.join().unwrap();
    let result: ExportResult = serde_json::from_value(result).unwrap();
    assert_eq!(result.attempts, 2, "the dropout started the render again");
    assert!(out.exists(), "the render finished and wrote its file");
}

// Spec: external-devices R25. With Auto-Restart off, the render carries on
// and the dropouts are listed with the position they happened at.
#[test]
fn with_auto_restart_off_the_dropout_is_listed() {
    let folder = TestDir::new("nodropout");
    let (mut shell, control) = shell_on_manual_loopback(Some(20));
    set_with_effect(&mut shell, &folder);
    let session = shell.realtime_session();
    let driver = drive(control, session);

    let out = folder.path("Live.wav");
    let result: Value = shell
        .run(
            export_ids::AUDIO,
            json!({ "path": out.to_string_lossy(), "bit_depth": 24,
                    "sample_rate": 48000, "wait_seconds": 0,
                    "auto_restart": false }),
        )
        .unwrap();
    driver.join().unwrap();
    let result: ExportResult = serde_json::from_value(result).unwrap();
    assert_eq!(result.attempts, 1, "the render carried on");
    assert_eq!(
        result.dropouts.len(),
        1,
        "the dropout is listed: {result:?}"
    );
    let dropout = &result.dropouts[0];
    assert!(
        dropout.position > bl_core::time::Ticks::ZERO,
        "the dropout has the position it happened at: {dropout:?}"
    );
}

// Spec: external-devices R23. Cancel stops the render and deletes the
// partial file it was writing.
#[test]
fn a_cancelled_render_leaves_no_file_behind() {
    let folder = TestDir::new("cancel");
    let (mut shell, control) = shell_on_manual_loopback(None);
    set_with_effect(&mut shell, &folder);
    let session = shell.realtime_session();
    let driver = drive(control, session.clone());

    let out = folder.path("Live.wav");
    let partial = folder.path(".Live.wav.partial");
    // A thread of its own cancels the render while it runs.
    let cancel = session.clone();
    let stopper = thread::spawn(move || {
        for _ in 0..20_000 {
            if cancel.status().phase != Phase::Idle {
                cancel.cancel();
                break;
            }
            thread::yield_now();
        }
    });
    let result = shell.run(
        export_ids::AUDIO,
        json!({ "path": out.to_string_lossy(), "bit_depth": 24,
                "sample_rate": 48000, "wait_seconds": 0 }),
    );
    stopper.join().unwrap();
    driver.join().unwrap();
    assert!(result.is_err(), "the export failed: {result:?}");
    assert!(!out.exists(), "nothing was written");
    assert!(!partial.exists(), "the partial file was deleted");
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

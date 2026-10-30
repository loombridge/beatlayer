//! CV Instrument and CV Triggers in the shell (WP-7.17; the CV tools spec,
//! R3, R4, R7, R9, R14, R15, R20): their choosers listing the interface,
//! `device.set_external`'s CV fields, `cv.calibrate` against a simulated
//! oscillator on the null device's cable, and what renders say of them.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use bl_audio_io::{NullBackend, NullControl, NullOptions, Pacing};
use bl_devices::{Tuner, cv_instrument_type, cv_triggers_type};
use bl_engine::DeviceType;
use bl_engine::NodeConfig;
use bl_model::{CvCalibration, DeviceId, External};
use bl_shell::device_commands::DeviceAdded;
use bl_shell::export_commands::{self as export_ids, ExportResult};
use bl_shell::routing::{self, Interface};
use bl_shell::{Shell, perception_commands, testing};
use serde_json::{Value, json};

/// The null device's channels, both ways.
const CHANNELS: u16 = 4;

/// Adds a device of `t` to the first track.
fn add(shell: &mut Shell, t: &DeviceType) -> DeviceId {
    let first = shell.editor().document().top_level().next().unwrap();
    add_to(shell, t, first)
}

/// Adds a device of `t` to `track`.
fn add_to(shell: &mut Shell, t: &DeviceType, track: bl_model::TrackId) -> DeviceId {
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let added: DeviceAdded = serde_json::from_value(
        shell
            .run(
                "device.add",
                json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
}

fn external(shell: &Shell, device: DeviceId) -> External {
    shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .external
        .clone()
        .unwrap()
}

// Spec: cv-tools R3, R4, R10, R16
#[test]
fn a_new_cv_device_has_no_channels_and_its_choosers_list_the_interface() {
    let mut shell = Shell::headless();
    let instrument = add(&mut shell, &cv_instrument_type());
    let second: bl_model::TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            json!({ "id": second, "kind": "midi", "name": "Drums", "parent": null, "index": 1 }),
        )
        .unwrap();
    let triggers = add_to(&mut shell, &cv_triggers_type(), second);

    // The outputs are named as the device names them, all set to none, and
    // Audio From to No Input.
    let held = external(&shell, instrument);
    let names: Vec<_> = held.cv_outputs.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Pitch", "Gate", "Mod 1", "Mod 2"]);
    assert!(held.cv_outputs.iter().all(|p| p.channel.is_none()));
    assert_eq!(held.audio_from, External::NO_INPUT);
    assert_eq!(external(&shell, triggers).cv_outputs.len(), 8);

    let interface = Interface {
        inputs: 4,
        outputs: 4,
        ..Interface::default()
    };
    let choosers =
        routing::external_choosers(shell.editor().document(), instrument, &interface).unwrap();
    let (name, pitch) = &choosers.cv_outputs[0];
    assert_eq!(name, "Pitch");
    assert_eq!(
        pitch.labels(),
        ["No Output", "1", "2", "3", "4", "─", "Configure…"]
    );
    assert_eq!(pitch.text, "No Output");
    assert_eq!(
        choosers.audio_from.labels(),
        [
            "No Input",
            "1",
            "2",
            "3",
            "4",
            "1/2",
            "3/4",
            "─",
            "Configure…"
        ]
    );
    assert_eq!(choosers.audio_from.text, "No Input");

    // Each entry runs the registered command, one undo step named Change
    // "Routing", and the choice reads back and survives undo.
    let steps = shell.editor().undo_history().len();
    let run = choosers.cv_outputs[1].1.run_for("3").unwrap().clone();
    shell.invoke(&run.id, run.args).unwrap();
    assert_eq!(shell.editor().undo_history().len(), steps + 1);
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Change \"Routing\"")
    );
    let held = external(&shell, instrument);
    assert_eq!(held.cv_outputs[1].channel, Some(3), "Gate is on channel 3");
    assert_eq!(held.cv_outputs[0].channel, None, "Pitch stays as it was");
    let again =
        routing::external_choosers(shell.editor().document(), instrument, &interface).unwrap();
    assert_eq!(again.cv_outputs[1].1.text, "3");
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(external(&shell, instrument).cv_outputs[1].channel, None);
    shell.run("edit.redo", json!({})).unwrap();

    // A channel the interface lacks stays, dimmed.
    shell
        .run(
            "device.set_external",
            json!({ "device": instrument, "cv_outputs": [{ "name": "Pitch", "channel": 9 }] }),
        )
        .unwrap();
    let gone =
        routing::external_choosers(shell.editor().document(), instrument, &interface).unwrap();
    let pitch = &gone.cv_outputs[0].1;
    assert_eq!(pitch.text, "9");
    assert!(pitch.run_for("9").is_none(), "dimmed");

    // A port the device lacks is refused, and nothing changes.
    let steps = shell.editor().undo_history().len();
    assert!(
        shell
            .run(
                "device.set_external",
                json!({ "device": instrument, "cv_outputs": [{ "name": "Clock", "channel": 1 }] }),
            )
            .is_err()
    );
    assert_eq!(shell.editor().undo_history().len(), steps);

    // The choices save with the set and open again.
    let folder = std::env::temp_dir().join(format!("bl-cv-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let saved: bl_shell::commands::SetResult = serde_json::from_value(
        shell
            .run(
                "set.save_as",
                json!({ "path": folder.join("Cv.blset").to_string_lossy() }),
            )
            .unwrap(),
    )
    .unwrap();
    shell
        .run("set.open", json!({ "path": saved.path.unwrap() }))
        .unwrap();
    let held = external(&shell, instrument);
    assert_eq!(held.cv_outputs[0].channel, Some(9));
    assert_eq!(held.cv_outputs[1].channel, Some(3));
    let _ = std::fs::remove_dir_all(&folder);
}

// Spec: cv-tools R9
#[test]
fn the_browsers_instruments_label_lists_both_devices() {
    let shell = Shell::headless();
    let listed = bl_devices::device_types()
        .into_iter()
        .filter(|t| t.role == bl_model::DeviceRole::Instrument)
        .map(|t| t.name)
        .collect::<Vec<_>>();
    assert!(listed.contains(&"CV Instrument".to_owned()));
    assert!(listed.contains(&"CV Triggers".to_owned()));
    drop(shell);
}

struct Bench {
    shell: Shell,
    control: NullControl,
    device: DeviceId,
    heard: Arc<Mutex<Vec<f32>>>,
    record: Arc<AtomicBool>,
    silent: Arc<AtomicBool>,
}

/// A shell on the null device paced by hand with the oscillator on its
/// cable, and a CV Instrument on the first track with Pitch on channel 3 and
/// Audio From on input 4.
fn bench() -> Bench {
    let (heard, record, silent) = (
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    let (backend, control) = NullBackend::with_devices(
        NullOptions {
            pacing: Pacing::Manual,
            channels: CHANNELS,
            ..NullOptions::default()
        },
        &[("null", "Null")],
    );
    let backend = backend.with_cable(testing::oscillator(&heard, &record, &silent));
    let (mut shell, control) = testing::rig_on(backend, control);
    shell
        .run("audio.configure", json!({ "input_device": "null:null" }))
        .unwrap();
    shell.start_audio();
    let device = add(&mut shell, &cv_instrument_type());
    shell
        .run(
            "device.set_external",
            json!({ "device": device,
                    "cv_outputs": [{ "name": "Pitch", "channel": 3 }],
                    "audio_from": { "type": "mono", "channel": 4 } }),
        )
        .unwrap();
    Bench {
        shell,
        control,
        device,
        heard,
        record,
        silent,
    }
}

/// Runs the device's blocks on a thread of its own until `stop` is set: the
/// command waits for the calibration on the test's thread, and the device's
/// blocks are what it waits for.
fn drive(control: NullControl, stop: &Arc<AtomicBool>) -> thread::JoinHandle<NullControl> {
    let stop = stop.clone();
    thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            control.run_blocks(8);
            thread::yield_now();
        }
        control
    })
}

/// The undo steps the set holds.
fn steps(shell: &Shell) -> usize {
    shell.editor().undo_history().len()
}

// Spec: cv-tools R14
#[test]
fn calibrate_is_refused_while_pitch_or_audio_from_is_unset_and_fails_on_silence() {
    let mut bench = bench();
    let stop = Arc::new(AtomicBool::new(false));
    let driver = drive(bench.control.clone(), &stop);

    // Pitch at No Output.
    bench
        .shell
        .run(
            "device.set_external",
            json!({ "device": bench.device, "cv_outputs": [{ "name": "Pitch" }] }),
        )
        .unwrap();
    let error = bench
        .shell
        .run("cv.calibrate", json!({ "device": bench.device }))
        .unwrap_err()
        .to_string();
    assert!(error.contains("Pitch"), "{error}");
    // Audio From at No Input.
    bench
        .shell
        .run(
            "device.set_external",
            json!({ "device": bench.device,
                    "cv_outputs": [{ "name": "Pitch", "channel": 3 }],
                    "audio_from": { "type": "mono", "channel": 0 } }),
        )
        .unwrap();
    let before_silence = steps(&bench.shell);
    let error = bench
        .shell
        .run("cv.calibrate", json!({ "device": bench.device }))
        .unwrap_err()
        .to_string();
    assert!(error.contains("Audio From"), "{error}");

    // Silence returning: it fails naming the first step and changes nothing.
    bench
        .shell
        .run(
            "device.set_external",
            json!({ "device": bench.device, "audio_from": { "type": "mono", "channel": 4 } }),
        )
        .unwrap();
    bench.silent.store(true, Ordering::Relaxed);
    let before_silence = before_silence + 1;
    let error = bench
        .shell
        .run("cv.calibrate", json!({ "device": bench.device }))
        .unwrap_err()
        .to_string();
    assert!(error.contains("step 1"), "{error}");
    assert!(error.contains("-2"), "{error}");
    assert_eq!(steps(&bench.shell), before_silence, "nothing changed");
    assert_eq!(external(&bench.shell, bench.device).cv_calibration, None);
    stop.store(true, Ordering::Relaxed);
    driver.join().unwrap();
}

// Spec: cv-tools R14, R15, R20
#[test]
fn calibrate_tunes_every_c_from_c1_to_c6_within_a_cent_as_one_undo_step() {
    let mut bench = bench();
    let stop = Arc::new(AtomicBool::new(false));
    let driver = drive(bench.control.clone(), &stop);
    let before = steps(&bench.shell);
    let result: Value = bench
        .shell
        .run("cv.calibrate", json!({ "device": bench.device }))
        .unwrap();
    stop.store(true, Ordering::Relaxed);
    let control = driver.join().unwrap();

    // One undo step, stored in the device, saved with the set.
    assert_eq!(steps(&bench.shell), before + 1);
    assert_eq!(
        bench.shell.editor().undo_history().next(),
        Some("Calibrate CV Instrument")
    );
    let held = external(&bench.shell, bench.device);
    let calibration = held.cv_calibration.clone().expect("a calibration");
    assert_eq!(
        result["calibration"]["octaves_per_volt"],
        json!(calibration.octaves_per_volt)
    );
    assert!((calibration.octaves_per_volt - 0.97).abs() < 0.01);
    // The steps the detector reads: from 41 Hz to 2 kHz.
    assert!(calibration.steps.len() >= 5, "{:?}", calibration.steps);
    let serialised = serde_json::to_value(bench.shell.editor().document()).unwrap();
    assert!(
        serialised.to_string().contains("octaves_per_volt"),
        "the calibration is saved with the set"
    );

    // Every C from C1 (key 36) to C6 (key 96) sounds its frequency.
    let keys: Vec<u8> = (0..6).map(|octave| 36 + 12 * octave).collect();
    let notes: Vec<Value> = keys
        .iter()
        .enumerate()
        .map(|(at, key)| {
            json!({ "key": key, "velocity": 100, "start": at as i64 * 705_600, "length": 705_600 })
        })
        .collect();
    let track = bench.shell.editor().document().top_level().next().unwrap();
    let clip: bl_model::ClipId = bench.shell.new_id();
    bench
        .shell
        .run(
            "clip.create",
            json!({ "track": track, "clip": {
                "id": clip, "name": "Cs", "start": 0, "length": 6 * 705_600, "offset": 0,
                "mute": false, "data": { "midi": { "notes": notes } } } }),
        )
        .unwrap();
    bench.heard.lock().unwrap().clear();
    bench.record.store(true, Ordering::Relaxed);
    bench.shell.run("transport.play", json!({})).unwrap();
    // 6 beats at 120 BPM is 144,000 frames; blocks of 256 frames.
    testing::run_blocks(&control, &mut bench.shell, 640);
    bench.record.store(false, Ordering::Relaxed);
    let heard = bench.heard.lock().unwrap().clone();
    let config = NodeConfig {
        round_trip_latency: 0,
        sample_rate: bl_core::time::SampleRate::HZ_48000,
        max_frames: 4096,
        channels: 2,
    };
    for (at, key) in keys.iter().enumerate() {
        // The second half of the note, once the oscillator has settled.
        let from = at * 24_000 + 14_000;
        let mut tuner = Tuner::new(&config);
        let mut pitch = None;
        for chunk in heard[from..from + 9_000].chunks(1024) {
            if let Some(found) = tuner.listen(chunk) {
                pitch = found;
            }
        }
        let want = 440.0 * ((f64::from(*key) - 69.0) / 12.0).exp2();
        let cents = 1200.0 * (pitch.expect("a pitch") / want).log2();
        assert!(cents.abs() < 1.0, "key {key}: {cents} cents off");
    }
}

// Spec: cv-tools R14
#[test]
fn cv_clear_calibration_takes_it_away_in_one_undo_step() {
    let mut shell = Shell::headless();
    let device = add(&mut shell, &cv_instrument_type());
    shell
        .run(
            "cv.set_calibration",
            json!({ "device": device, "calibration":
                serde_json::to_value(CvCalibration::fit(&[(0.0, 130.0), (1.0, 260.0)]).unwrap()).unwrap() }),
        )
        .unwrap();
    let before = steps(&shell);
    let cleared = shell
        .run("cv.clear_calibration", json!({ "device": device }))
        .unwrap();
    assert_eq!(cleared["cleared"], json!(true));
    assert_eq!(steps(&shell), before + 1);
    assert_eq!(external(&shell, device).cv_calibration, None);
    // Nothing to clear: nothing changes.
    let cleared = shell
        .run("cv.clear_calibration", json!({ "device": device }))
        .unwrap();
    assert_eq!(cleared["cleared"], json!(false));
    assert_eq!(steps(&shell), before + 1);
    // Another device isn't a CV Instrument.
    let second: bl_model::TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            json!({ "id": second, "kind": "midi", "name": "Drums", "parent": null, "index": 1 }),
        )
        .unwrap();
    let triggers = add_to(&mut shell, &cv_triggers_type(), second);
    assert!(
        shell
            .run("cv.clear_calibration", json!({ "device": triggers }))
            .is_err()
    );
}

// Spec: cv-tools R7
#[test]
fn an_offline_preview_names_a_routed_cv_device_and_it_needs_real_time() {
    let mut shell = Shell::headless();
    let device = add(&mut shell, &cv_instrument_type());
    let track = shell.editor().document().top_level().next().unwrap();
    let clip: bl_model::ClipId = shell.new_id();
    shell
        .run(
            "clip.create",
            json!({ "track": track, "clip": {
                "id": clip, "name": "Gate", "start": 0, "length": 705_600, "offset": 0,
                "mute": false, "data": { "midi": { "notes": [
                    { "key": 60, "velocity": 100, "start": 0, "length": 352_800 }] } } } }),
        )
        .unwrap();
    let name = format!("CV Instrument ({device})");
    // Nothing routed: it reaches no interface, so nothing is silenced.
    let notes = |shell: &mut Shell| -> Vec<String> {
        let preview = shell.run(perception_commands::PREVIEW, json!({})).unwrap();
        preview["rendered"]["notes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n.as_str().map(str::to_owned))
            .collect()
    };
    assert!(!notes(&mut shell).iter().any(|n| n.starts_with(&name)));
    assert!(!bl_shell::realtime::needs_real_time(
        shell.editor().document(),
        None
    ));

    // With an output chosen it does, in a preview and an export alike.
    shell
        .run(
            "device.set_external",
            json!({ "device": device, "cv_outputs": [{ "name": "Gate", "channel": 2 }] }),
        )
        .unwrap();
    assert!(notes(&mut shell).iter().any(|n| n.starts_with(&name)));
    assert!(bl_shell::realtime::needs_real_time(
        shell.editor().document(),
        None
    ));

    // Audio From alone reaches the interface too; a switched-off device
    // doesn't.
    shell
        .run(
            "device.set_external",
            json!({ "device": device, "cv_outputs": [{ "name": "Gate" }],
                    "audio_from": { "type": "pair", "channel": 1 } }),
        )
        .unwrap();
    assert!(bl_shell::realtime::needs_real_time(
        shell.editor().document(),
        None
    ));
    shell
        .run(
            "device.set_enabled",
            json!({ "device": device, "enabled": false }),
        )
        .unwrap();
    assert!(!bl_shell::realtime::needs_real_time(
        shell.editor().document(),
        None
    ));
}

/// A one-beat mono tone as a WAV file at `path`.
fn tone(path: &std::path::Path) {
    let frames = 24_000u32;
    let data = frames * 2;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    for field in [16u32, 1 | 1 << 16, 48_000, 96_000, 2 | 16 << 16] {
        bytes.extend_from_slice(&field.to_le_bytes());
    }
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let sample: i16 = if i % 100 < 50 { 8_000 } else { -8_000 };
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

// Spec: cv-tools R7. An export of a set with a CV device routed renders in
// real time on the running engine, whichever device it is.
#[test]
fn an_export_with_a_routed_cv_device_renders_in_real_time() {
    for t in [cv_instrument_type(), cv_triggers_type()] {
        let folder =
            std::env::temp_dir().join(format!("bl-cv-export-{}-{}", t.name, std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("Tone.wav");
        tone(&file);
        let backend = NullBackend::default()
            .with_latency(400, 600)
            .with_loopback(1_000);
        let devices = backend.clone();
        let audio = bl_shell::audio::AudioHost::with_backends(
            vec![bl_audio_io::BackendKind::Null],
            Box::new(move |_| Ok(Box::new(devices.clone()))),
            bl_shell::audio::AudioSettings::default(),
        );
        let mut shell = Shell::new(
            bl_shell::settings::Preferences::default(),
            None,
            audio,
            Box::new(bl_shell::files::ProjectFiles::new()),
        );
        shell
            .run("audio.configure", json!({ "input_device": "null:null" }))
            .unwrap();
        shell.start_audio();
        shell
            .run(
                bl_shell::browser_commands::ADD_TO_ARRANGEMENT,
                json!({ "paths": [file.to_string_lossy()], "at": 0 }),
            )
            .unwrap();
        let device = add(&mut shell, &t);
        let port = external(&shell, device).cv_outputs[0].name.clone();
        shell
            .run(
                "device.set_external",
                json!({ "device": device, "cv_outputs": [{ "name": port, "channel": 1 }] }),
            )
            .unwrap();
        let out = folder.join("Live.wav");
        let result: ExportResult = serde_json::from_value(
            shell
                .run(
                    export_ids::AUDIO,
                    json!({ "path": out.to_string_lossy(), "bit_depth": 24,
                            "sample_rate": 48000, "wait_seconds": 0 }),
                )
                .unwrap(),
        )
        .unwrap();
        assert!(result.real_time, "{}: rendered in real time", t.name);
        assert!(out.exists());
        let _ = std::fs::remove_dir_all(&folder);
    }
}

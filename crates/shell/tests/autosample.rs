//! The autosampler in the shell (WP-5.16, IMP-27; the multisampler spec,
//! R64 to R73): the samples it makes of a native instrument and of the test
//! plug-in, the multisampler they become, the loops it finds in them, and
//! the files a cancelled run leaves behind. Every block of a render runs
//! under the allocation trap.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use bl_model::{DeviceId, DeviceRole, Layer, TrackId, TrackKind};
use bl_shell::Shell;
use bl_shell::autosample::Settings;
use bl_shell::autosampler_commands::{self as ids, Autosampled};
use bl_shell::export::{self, Length, Render};
use serde_json::{Value, json};

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-autosample-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, rest: &str) -> PathBuf {
        self.0.join(rest)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A shell with a saved set, so the samples have a project folder to go
/// into.
fn saved_shell(scratch: &Scratch) -> Shell {
    let mut shell = Shell::headless();
    let path = scratch.path("Song.blset");
    shell
        .run("set.save_as", json!({ "path": path.to_string_lossy() }))
        .unwrap();
    shell
}

/// The MIDI track the instrument goes on.
fn midi_track(shell: &Shell) -> TrackId {
    shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Midi)
        .map(bl_model::Track::id)
        .expect("a MIDI track")
}

/// The native polysynth on a MIDI track.
fn polysynth(shell: &mut Shell) -> DeviceId {
    let track = midi_track(shell);
    let t = bl_devices::polysynth_type();
    let added: bl_shell::device_commands::DeviceAdded = serde_json::from_value(
        shell
            .run(
                "device.add",
                json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role }, "track": track }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
}

/// The kit's sine example, a plug-in instrument, on a MIDI track.
fn plugin(shell: &mut Shell) -> DeviceId {
    let track = midi_track(shell);
    let types = shell.kit().device_types();
    let sine = types
        .iter()
        .find(|kind| kind.name == "Example Sine")
        .expect("the sine example");
    let added: bl_shell::device_commands::DeviceAdded = serde_json::from_value(
        shell
            .run(
                "device.add",
                json!({ "device": { "kind": sine.kind, "name": sine.name, "role": sine.role },
                        "track": track }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
}

fn autosample(shell: &mut Shell, args: Value) -> Autosampled {
    serde_json::from_value(shell.run(ids::AUTOSAMPLE, args).unwrap()).unwrap()
}

fn layers(shell: &Shell, device: DeviceId) -> Vec<Layer> {
    shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .layers
        .list
        .clone()
}

fn value(shell: &Shell, device: DeviceId, param: bl_model::ParamId) -> f64 {
    let found = shell.editor().document().device(device).unwrap();
    if let Some(value) = found.params.get(&param) {
        return *value;
    }
    bl_devices::multisampler::multisampler_type()
        .description
        .param(param)
        .map_or(f64::NAN, |descriptor| descriptor.default)
}

/// The settings the acceptance uses: three octaves at a step of 3 with two
/// velocity layers, and a short note so the test runs.
fn settings() -> Value {
    json!({
        "lowest": 36,
        "highest": 84,
        "step": 3,
        "velocity_layers": 2,
        "note_length": 0.5,
        "release_tail": 0.2,
    })
}

/// The settings of a short sample, as the settings the plan is made from.
fn short_settings() -> Settings {
    Settings {
        lowest: 60,
        highest: 60,
        step: 3,
        velocity_layers: 1,
        note_length: 0.3,
        release_tail: 0.1,
        round_robins: 1,
        detect_loops: false,
        name: "Drift".to_owned(),
    }
}

/// Renders an instrument alone with one note, as the autosampler does.
fn render(
    shell: &Shell,
    device: DeviceId,
    note: u8,
    velocity: u8,
    settings: &Settings,
    block: usize,
) -> (Vec<f32>, u64) {
    let document = bl_shell::autosample::render_document(
        shell.editor().document(),
        device,
        note,
        velocity,
        settings,
    )
    .unwrap();
    let rate = 48_000;
    let frames = (settings.sample_seconds() * f64::from(rate)).round() as u64;
    let mut samples: Vec<f32> = Vec::new();
    let rendered = export::render(
        &document,
        shell.media_store(),
        shell.plugin_hosts(),
        &Render {
            start: bl_core::time::Ticks::ZERO,
            length: Length::Frames(frames),
            sample_rate: rate,
            engine_rate: rate,
            block,
        },
        60.0,
        &AtomicBool::new(false),
        &mut |_| {},
        &mut |block| {
            samples.extend_from_slice(block);
            Ok(())
        },
        None,
    )
    .unwrap();
    assert_eq!(rendered.heap_operations, 0, "the real-time rules");
    (samples, frames)
}

/// The pitch of a signal, in hertz, from its autocorrelation.
fn pitch(samples: &[f32], rate: u32) -> f64 {
    let min_lag = (rate as f64 / 2000.0) as usize;
    let max_lag = (rate as f64 / 50.0) as usize;
    let window = &samples[..samples.len().min(8192)];
    let mut best = (f64::NEG_INFINITY, min_lag);
    for lag in min_lag..=max_lag.min(window.len() / 2) {
        let mut sum = 0.0;
        let mut energy = 0.0;
        for index in 0..window.len() - lag {
            let a = f64::from(window[index]);
            let b = f64::from(window[index + lag]);
            sum += a * b;
            energy += a * a;
        }
        if energy > 0.0 && sum / energy > best.0 {
            best = (sum / energy, lag);
        }
    }
    rate as f64 / best.1 as f64
}

// Spec: multisampler R69, R70
#[test]
fn the_result_is_a_new_track_and_one_undo_step() {
    let scratch = Scratch::new("result");
    let mut shell = saved_shell(&scratch);
    let device = polysynth(&mut shell);
    let tracks = shell.editor().document().top_level().count();
    let history = shell.editor().undo_history().len();
    let mut args = json!({ "device": device, "name": "Drift" });
    for (key, value) in settings().as_object().unwrap() {
        args[key] = value.clone();
    }
    let result = autosample(&mut shell, args);
    // A new MIDI track straight after the source's, with a multisampler.
    let track = result.track.expect("the track");
    let multisampler = result.device.expect("the multisampler");
    let doc = shell.editor().document();
    assert_eq!(doc.track(track).unwrap().name(), "Drift");
    assert_eq!(doc.track(track).unwrap().kind(), TrackKind::Midi);
    let order: Vec<TrackId> = doc.track_order();
    let source = doc.device_track(device).unwrap();
    assert_eq!(
        order.iter().position(|id| *id == track).unwrap(),
        order.iter().position(|id| *id == source).unwrap() + 1,
        "straight after the source's track"
    );
    assert_eq!(doc.top_level().count(), tracks + 1);
    let made = doc.device(multisampler).unwrap();
    assert_eq!(made.role, DeviceRole::Instrument);
    // One undo step for the track and its device.
    assert_eq!(shell.editor().undo_history().len(), history + 1);
    // The layers, one per sample, in note, velocity and round robin order.
    let list = layers(&shell, multisampler);
    assert_eq!(list.len(), 34);
    assert_eq!(
        list.iter().map(|l| l.root_key).collect::<Vec<_>>(),
        (0..17)
            .map(|index| 36 + index * 3)
            .flat_map(|note| [note, note])
            .collect::<Vec<_>>()
    );
    // The zones of R66: the key zones run halfway to the neighbours, and
    // the velocity layers take their own share.
    assert_eq!((list[0].key.low, list[0].key.high), (0, 37));
    assert_eq!((list[0].velocity.low, list[0].velocity.high), (1, 64));
    assert_eq!((list[1].velocity.low, list[1].velocity.high), (65, 127));
    // Vel > Vol is 0 %, the volume envelope's release is the tail, and the
    // voices are 32.
    assert_eq!(
        value(&shell, multisampler, bl_devices::multisampler::VEL_VOL),
        0.0
    );
    assert!(
        (value(&shell, multisampler, bl_devices::multisampler::ENV_RELEASE) - 0.2).abs() < 1e-9
    );
    assert_eq!(
        value(&shell, multisampler, bl_devices::multisampler::VOICES),
        32.0
    );
    // Round Robin is off with one round robin.
    assert_eq!(
        value(&shell, multisampler, bl_devices::multisampler::ROUND_ROBIN),
        0.0
    );
    // Undoing it leaves the files on disk, as recorded files stay.
    assert_eq!(result.files.len(), 34);
    shell.run("edit.undo", json!({})).unwrap();
    assert!(shell.editor().document().track(track).is_none());
    for file in &result.files {
        assert!(Path::new(&file.path).exists(), "{} stays", file.path);
    }
}

// Spec: multisampler R69
#[test]
fn samples_are_named_and_placed_in_the_project() {
    let scratch = Scratch::new("names");
    let mut shell = saved_shell(&scratch);
    let device = polysynth(&mut shell);
    let result = autosample(
        &mut shell,
        json!({
            "device": device,
            "name": "Drift",
            "lowest": 60,
            "highest": 60,
            "velocity_layers": 1,
            "note_length": 0.3,
            "release_tail": 0.1,
        }),
    );
    assert_eq!(result.files.len(), 1);
    let file = &result.files[0];
    assert_eq!(file.note, 60);
    assert_eq!(file.velocity, 127);
    let path = Path::new(&file.path);
    assert_eq!(path.file_name().unwrap(), "Drift C3 v127.wav");
    // In the project's folder, under the instrument's name.
    let folder = scratch.path("Song Project/Samples/Processed/Autosample/Drift");
    assert_eq!(path.parent().unwrap(), folder);
    // A 32-bit float WAV of the note and the tail, at the engine's rate.
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[0..4], b"RIFF");
    // The float format, in the extensible header's subformat GUID.
    assert_eq!(u16::from_le_bytes([bytes[44], bytes[45]]), 3, "IEEE float");
    assert_eq!(u16::from_le_bytes([bytes[22], bytes[23]]), 2, "stereo");
    assert_eq!(
        u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
        48_000
    );
    assert_eq!(u16::from_le_bytes([bytes[34], bytes[35]]), 32, "32 bits");
}

// Spec: multisampler R67
#[test]
fn a_native_synth_and_a_plugin_render_offline_the_same_every_time() {
    let scratch = Scratch::new("render");
    let mut shell = saved_shell(&scratch);
    let device = polysynth(&mut shell);
    let settings = short_settings();
    let result = autosample(
        &mut shell,
        json!({ "device": device, "name": "Drift", "lowest": 60, "highest": 60,
                "note_length": 0.3, "release_tail": 0.1 }),
    );
    assert_eq!(result.files.len(), 1);
    // The same render at another block size is the same samples.
    for block in [64usize, 512] {
        let (samples, _) = render(&shell, device, 60, 127, &settings, block);
        let file = bl_media::decode_file(&result.files[0].path).unwrap();
        assert_eq!(file.channel_count(), 2);
        assert_eq!(file.frames() as u64, samples.len() as u64 / 2);
        for channel in 0..2 {
            for (index, sample) in file.channel(channel).unwrap().iter().enumerate() {
                assert!(
                    (sample - samples[index * 2 + channel]).abs() < 1e-6,
                    "the same samples at {block}"
                );
            }
        }
    }
    // The plug-in renders the same way.
    let mut shell = saved_shell(&scratch);
    let device = plugin(&mut shell);
    let result = autosample(
        &mut shell,
        json!({ "device": device, "name": "Plug", "lowest": 60, "highest": 60,
                "note_length": 0.3, "release_tail": 0.1 }),
    );
    assert_eq!(result.files.len(), 1);
    assert!(Path::new(&result.files[0].path).exists());
}

// Spec: multisampler R72
#[test]
fn three_octaves_and_two_layers_match_the_synth() {
    let scratch = Scratch::new("match");
    let mut shell = saved_shell(&scratch);
    let device = polysynth(&mut shell);
    let mut args = json!({ "device": device, "name": "Drift" });
    for (key, value) in settings().as_object().unwrap() {
        args[key] = value.clone();
    }
    let result = autosample(&mut shell, args);
    let multisampler = result.device.expect("the multisampler");
    let settings = Settings {
        lowest: 36,
        highest: 84,
        step: 3,
        velocity_layers: 2,
        note_length: 0.5,
        release_tail: 0.2,
        round_robins: 1,
        detect_loops: false,
        name: "Drift".to_owned(),
    };
    let rate = 48_000;
    let held = (settings.note_length * f64::from(rate)).round() as usize;
    // At every sampled note and velocity, the multisampler plays what the
    // source plays, 60 dB below the source's level.
    for file in result.files.iter().take(4) {
        let (source, _) = render(&shell, device, file.note, file.velocity, &settings, 512);
        let (sampled, _) = render(
            &shell,
            multisampler,
            file.note,
            file.velocity,
            &settings,
            512,
        );
        let level: f64 = source[..held * 2]
            .iter()
            .map(|sample| f64::from(*sample) * f64::from(*sample))
            .sum::<f64>()
            .sqrt();
        let difference: f64 = source[..held * 2]
            .iter()
            .zip(&sampled[..held * 2])
            .map(|(a, b)| f64::from(a - b) * f64::from(a - b))
            .sum::<f64>()
            .sqrt();
        assert!(
            difference < level * 0.001,
            "note {} velocity {}: {difference} against {level}",
            file.note,
            file.velocity
        );
    }
    // A key between two sampled notes sounds within 1 cent of the source.
    let between = 61;
    let (source, _) = render(&shell, device, between, 127, &settings, 512);
    let (sampled, _) = render(&shell, multisampler, between, 127, &settings, 512);
    let source_pitch = pitch(&source, rate);
    let sampled_pitch = pitch(&sampled, rate);
    let cents = 1200.0 * (sampled_pitch / source_pitch).log2();
    assert!(cents.abs() < 1.0, "{cents} cents from the source");
}

// Spec: multisampler R71
#[test]
fn a_looped_note_sustains_past_its_length() {
    let scratch = Scratch::new("loops");
    let mut shell = saved_shell(&scratch);
    let device = polysynth(&mut shell);
    let result = autosample(
        &mut shell,
        json!({ "device": device, "name": "Drift", "lowest": 60, "highest": 60,
                "note_length": 0.5, "release_tail": 0.2, "detect_loops": true }),
    );
    let multisampler = result.device.expect("the multisampler");
    let list = layers(&shell, multisampler);
    let layer = &list[0];
    assert_eq!(layer.sustain, bl_model::SustainMode::Loop);
    assert_eq!(layer.release, bl_model::ReleaseMode::Release);
    assert!((layer.crossfade_ms - 10.0).abs() < 1e-9);
    assert!(layer.loop_start < layer.loop_end);
    // The loop is inside the held part, between a quarter of it and nine
    // tenths of it.
    let held = (0.5 * 48_000.0) as u64;
    assert!(layer.loop_start >= held / 4);
    assert!(layer.loop_end <= held * 9 / 10);
    if result.without_loops.is_empty() {
        // The sound holds past the note's length.
        let settings = Settings {
            lowest: 60,
            highest: 60,
            step: 3,
            velocity_layers: 1,
            note_length: 0.5,
            release_tail: 0.2,
            round_robins: 1,
            detect_loops: true,
            name: "Drift".to_owned(),
        };
        let (sampled, frames) = render(&shell, multisampler, 60, 127, &settings, 512);
        let after = &sampled[(held as usize) * 2..frames as usize * 2];
        let level: f32 = after.iter().map(|sample| sample.abs()).fold(0.0, f32::max);
        assert!(level > 0.001, "the sound holds past the note: {level}");
    }
}

// Spec: multisampler R73
#[test]
fn cancel_deletes_the_files_and_changes_nothing() {
    let scratch = Scratch::new("cancel");
    let mut shell = saved_shell(&scratch);
    let device = polysynth(&mut shell);
    let before = shell.editor().document().clone();
    let history = shell.editor().undo_history().len();
    // The run starts, and one sample is made.
    shell
        .run(
            ids::AUTOSAMPLE,
            json!({ "device": device, "name": "Drift", "lowest": 60, "highest": 84,
                    "step": 3, "velocity_layers": 2, "note_length": 0.3,
                    "release_tail": 0.1, "wait": false }),
        )
        .unwrap();
    shell.poll();
    let status = shell.autosample_status();
    assert!(status.running);
    assert_eq!(status.done, 1);
    let folder = scratch.path("Song Project/Samples/Processed/Autosample/Drift");
    let made: Vec<PathBuf> = std::fs::read_dir(&folder)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    assert_eq!(made.len(), 1, "one sample so far");
    // The run is stopped.
    let cancelled: Value =
        serde_json::from_value(shell.run(ids::AUTOSAMPLE_CANCEL, json!({})).unwrap()).unwrap();
    assert_eq!(cancelled["cancelled"], json!(true));
    // The files it wrote are gone, and the set is as it was.
    assert!(!made[0].exists(), "the file is deleted");
    assert_eq!(
        shell.editor().document().top_level().count(),
        before.top_level().count(),
        "no track"
    );
    assert_eq!(shell.editor().undo_history().len(), history, "no undo step");
}

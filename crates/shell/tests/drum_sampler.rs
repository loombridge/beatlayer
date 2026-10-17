//! Strike in the shell (WP-5.18; the one-shot drum sampler spec, R22 and
//! R23): `device.load_sample` gives it a sample by content hash, a saved
//! set reopens with it, and `device.swap_to_snapshot` replaces it with
//! Snapshot, keeping Start and Length, in one undo step. Driven through
//! `Shell::run`, as an agent drives it (D17).

use std::path::{Path, PathBuf};

use bl_devices::drum_sampler::{LENGTH, START, drum_sampler_type};
use bl_devices::sampler::{self, SAMPLER};
use bl_media::BitDepth;
use bl_model::{ContentHash, DeviceId, DeviceKind, TrackKind};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::device_commands::{DeviceAdded, LoadSampleResult};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use serde_json::json;

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-strike-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a short mono WAV file that isn't silence.
fn wav(path: &Path, frames: usize) {
    let samples: Vec<f32> = (0..frames)
        .map(|i| (i as f32 * 440.0 / 48_000.0 * std::f32::consts::TAU).sin() * 0.25)
        .collect();
    let bytes =
        bl_media::wav_bytes(&samples, 1, 48_000, BitDepth::Int16, bl_media::Dither::None).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn headless_with_config(config: &Path) -> Shell {
    Shell::new(
        Preferences::default(),
        Some(config.to_owned()),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    )
}

/// Adds Strike to the default set's MIDI track, as `device.add` adds one
/// from the browser.
fn add_strike(shell: &mut Shell) -> DeviceId {
    let midi = shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Midi)
        .map(bl_model::Track::id)
        .expect("the default set has a MIDI track");
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [midi] }))
        .unwrap();
    let strike = drum_sampler_type();
    let added: DeviceAdded = serde_json::from_value(
        shell
            .run(
                "device.add",
                json!({
                    "device": { "kind": strike.kind, "name": strike.name, "role": strike.role }
                }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
}

fn load(shell: &mut Shell, device: DeviceId, file: &Path) -> LoadSampleResult {
    serde_json::from_value(
        shell
            .run(
                "device.load_sample",
                json!({ "device": device, "path": file.to_string_lossy() }),
            )
            .unwrap(),
    )
    .unwrap()
}

fn sample_of(shell: &Shell, device: DeviceId) -> Option<ContentHash> {
    shell
        .editor()
        .document()
        .device(device)
        .expect("the device")
        .media
}

// Spec: one-shot-drum-sampler R22
#[test]
fn loading_a_sample_keeps_it_by_hash_through_a_save_and_undoes_as_one_step() {
    let scratch = Scratch::new("load");
    let file = scratch.path("Kick.wav");
    wav(&file, 4800);
    let mut shell = headless_with_config(&scratch.path("config"));
    let device = add_strike(&mut shell);
    assert_eq!(sample_of(&shell, device), None, "a new Strike has none");

    let loaded = load(&mut shell, device, &file);
    assert_eq!(sample_of(&shell, device), Some(loaded.media));
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(sample_of(&shell, device), None, "one undo takes it");
    shell.run("edit.redo", json!({})).unwrap();

    let set = scratch.path("Song.blset");
    let saved: bl_shell::commands::SetResult = serde_json::from_value(
        shell
            .run("set.save_as", json!({ "path": set.to_string_lossy() }))
            .unwrap(),
    )
    .unwrap();
    let set = PathBuf::from(saved.path.expect("the set was saved"));
    shell
        .run("set.open", json!({ "path": set.to_string_lossy() }))
        .unwrap();
    assert_eq!(sample_of(&shell, device), Some(loaded.media), "by hash");
    assert!(
        shell
            .media_store()
            .entry(bl_media::ContentHash::from_bytes(loaded.media.0))
            .is_some(),
        "the set's media table named the file"
    );
}

// Spec: one-shot-drum-sampler R23
#[test]
fn swapping_to_snapshot_keeps_the_sample_start_and_length_in_one_undo_step() {
    let scratch = Scratch::new("swap");
    let file = scratch.path("Snare.wav");
    wav(&file, 4800);
    let mut shell = headless_with_config(&scratch.path("config"));
    let strike = add_strike(&mut shell);
    let loaded = load(&mut shell, strike, &file);
    for (param, value) in [(START, 20.0), (LENGTH, 50.0)] {
        shell
            .run(
                "device.set_param",
                json!({ "device": strike, "param": param, "value": value }),
            )
            .unwrap();
    }
    let chain = |shell: &Shell| -> Vec<(DeviceId, DeviceKind)> {
        shell
            .editor()
            .document()
            .tracks()
            .flat_map(|track| {
                track
                    .devices()
                    .map(|d| (d.id, d.kind.clone()))
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    let before = chain(&shell);

    let swapped: DeviceAdded = serde_json::from_value(
        shell
            .run("device.swap_to_snapshot", json!({ "device": strike }))
            .unwrap(),
    )
    .unwrap();
    let after = chain(&shell);
    assert_eq!(after.len(), before.len(), "a replacement, not an addition");
    assert!(after.iter().all(|(id, _)| *id != strike), "Strike went");
    let device = shell.editor().document().device(swapped.device).unwrap();
    assert_eq!(
        device.kind,
        DeviceKind::Native {
            device_type: SAMPLER.to_owned()
        }
    );
    assert_eq!(device.media, Some(loaded.media), "the sample came along");
    assert_eq!(device.params.get(&sampler::MODE), Some(&1.0), "One-Shot");
    assert_eq!(device.params.get(&sampler::START), Some(&20.0));
    assert_eq!(device.params.get(&sampler::LENGTH), Some(&50.0));

    // One undo step brings Strike back as it was.
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(chain(&shell), before);
    let back = shell.editor().document().device(strike).unwrap();
    assert_eq!(back.params.get(&START), Some(&20.0));
    assert_eq!(sample_of(&shell, strike), Some(loaded.media));

    // Only Strike swaps.
    let refused = shell.run(
        "device.swap_to_snapshot",
        json!({ "device": swapped.device }),
    );
    assert!(refused.is_err(), "a device that isn't Strike");
}

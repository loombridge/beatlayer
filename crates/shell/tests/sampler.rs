//! The sampler in the shell (WP-2.16, part 1; the sampler spec, R11):
//! `device.load_sample` imports a file into the set's media, keeps it on
//! the sampler, and undoes as one step; a saved set reopens with its
//! sample. Driven through `Shell::run`, as an agent drives it (D17), on
//! real files and bl-project's real set files.

use std::path::{Path, PathBuf};

use bl_devices::sampler::SAMPLER;
use bl_media::BitDepth;
use bl_model::{ContentHash, DeviceId, DeviceKind, DeviceRole, TrackKind};
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
            "bl-sampler-{name}-{}-{}",
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

/// Adds the sampler to the default set's MIDI track, as `device.add` adds
/// one from the browser.
fn add_sampler(shell: &mut Shell) -> DeviceId {
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
    let added: DeviceAdded = serde_json::from_value(
        shell
            .run(
                "device.add",
                json!({
                    "device": {
                        "kind": DeviceKind::Native { device_type: SAMPLER.to_owned() },
                        "name": "Snapshot",
                        "role": DeviceRole::Instrument,
                    }
                }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
}

/// What `device.load_sample` reports.
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

/// The sampler's sample, if it has one.
fn sample_of(shell: &Shell, device: DeviceId) -> Option<ContentHash> {
    shell
        .editor()
        .document()
        .device(device)
        .expect("the sampler")
        .media
}

// Spec: sampler R11
#[test]
fn loading_a_sample_keeps_it_in_the_set_and_undoes_as_one_step() {
    let scratch = Scratch::new("load");
    let file = scratch.path("Kick.wav");
    wav(&file, 4800);
    let mut shell = headless_with_config(&scratch.path("config"));
    let device = add_sampler(&mut shell);
    assert_eq!(sample_of(&shell, device), None, "a new sampler has none");

    let loaded = load(&mut shell, device, &file);
    assert_eq!(loaded.device, device);
    assert_eq!(loaded.name, "Kick.wav");
    let media = sample_of(&shell, device).expect("the sample arrived");
    assert_eq!(media, loaded.media);
    // The media store holds the file, by the hash of its bytes.
    let store = shell.media_store();
    let hash = store
        .entry(bl_media::ContentHash::from_bytes(media.0))
        .unwrap();
    assert_eq!(hash.path, file, "the store knows where the file is");

    // One undo step: the sample goes and comes back.
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(sample_of(&shell, device), None, "undo takes the sample");
    shell.run("edit.redo", json!({})).unwrap();
    assert_eq!(
        sample_of(&shell, device),
        Some(media),
        "redo brings it back"
    );

    // A device whose type doesn't play a sample is refused.
    let utility = bl_devices::utility::utility_type();
    let added: DeviceAdded = serde_json::from_value(
        shell
            .run(
                "device.add",
                json!({
                    "device": {
                        "kind": utility.kind,
                        "name": utility.name,
                        "role": utility.role,
                    }
                }),
            )
            .unwrap(),
    )
    .unwrap();
    let refused = shell.run(
        "device.load_sample",
        json!({ "device": added.device, "path": file.to_string_lossy() }),
    );
    assert!(refused.is_err(), "a device that doesn't play a sample");
}

// Spec: sampler R11
#[test]
fn a_set_with_a_sampler_reopens_with_its_sample() {
    let scratch = Scratch::new("reopen");
    let file = scratch.path("Snare.wav");
    wav(&file, 4800);
    let mut shell = headless_with_config(&scratch.path("config"));
    let device = add_sampler(&mut shell);
    let loaded = load(&mut shell, device, &file);

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

    let reopened = sample_of(&shell, device);
    assert_eq!(reopened, Some(loaded.media), "the set keeps the sample");
    // And the set's media table points the store at the file again, so the
    // engine finds the audio when it plays.
    let store = shell.media_store();
    assert!(
        store
            .entry(bl_media::ContentHash::from_bytes(loaded.media.0))
            .is_some(),
        "the set's media table named the file"
    );
}

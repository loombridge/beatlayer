//! Wavetables in the shell (WP-8.8; the wavetable spec, R7, R8):
//! `device.load_wavetable` gives an oscillator a factory table or a file,
//! as one undo step; a user table's file is referenced by the set like a
//! sample, reported missing, copied by Collect All and Save and reopened
//! with the same table. Driven through `Shell::run`, as an agent drives
//! it, on a device of the harness type until Wavetable exists.

use std::path::{Path, PathBuf};

use bl_devices::wavetables::{BENCH, user};
use bl_model::{DeviceId, DeviceKind, DeviceRole, TrackKind, WavetableChoice};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::device_commands::{DeviceAdded, LoadWavetableResult};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use serde_json::{Value, json};

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-wavetables-{name}-{}-{}",
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

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/wavetables")
        .join(name)
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn headless_with_config(config: &Path) -> Shell {
    Shell::new(
        Preferences::default(),
        Some(config.to_owned()),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    )
}

/// Adds a device of `kind` and `role` to the default set's MIDI track.
fn add_device(shell: &mut Shell, kind: DeviceKind, role: DeviceRole) -> DeviceId {
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
                        "kind": kind,
                        "name": "Wavetable bench",
                        "role": role,
                    }
                }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
}

fn bench() -> DeviceKind {
    DeviceKind::Native {
        device_type: BENCH.to_owned(),
    }
}

fn load(shell: &mut Shell, device: DeviceId, args: Value) -> LoadWavetableResult {
    let mut args = args;
    args["device"] = json!(device);
    serde_json::from_value(shell.run("device.load_wavetable", args).unwrap()).unwrap()
}

fn choice_of(shell: &Shell, device: DeviceId, oscillator: usize) -> Option<WavetableChoice> {
    shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .wavetable_at(oscillator)
        .cloned()
}

fn undo_names(shell: &Shell) -> Vec<String> {
    shell
        .editor()
        .undo_history()
        .map(str::to_owned)
        .take(1)
        .collect()
}

// Spec: wavetable R8
#[test]
fn load_wavetable_gives_an_oscillator_a_table_as_one_undo_step() {
    let scratch = Scratch::new("load");
    let mut shell = headless_with_config(&scratch.path("config"));
    let device = add_device(&mut shell, bench(), DeviceRole::Instrument);
    assert_eq!(choice_of(&shell, device, 0), None, "a new device has none");

    let factory = load(
        &mut shell,
        device,
        json!({ "oscillator": 1, "category": "Additive", "name": "Odd Rise" }),
    );
    assert_eq!(factory.frames, 32);
    assert_eq!(
        choice_of(&shell, device, 0),
        Some(WavetableChoice::Factory {
            category: "Additive".to_owned(),
            name: "Odd Rise".to_owned()
        })
    );
    assert_eq!(undo_names(&shell), ["Load Wavetable"]);

    let file = fixture("plain-3-frames.wav");
    let user = load(
        &mut shell,
        device,
        json!({ "oscillator": 2, "file": text(&file), "raw": true }),
    );
    assert_eq!(user.frames, 3);
    assert_eq!(
        choice_of(&shell, device, 1),
        Some(WavetableChoice::User {
            raw: true,
            frames: 3
        })
    );
    // The file is the media in the oscillator's own slot, and the media
    // store knows where it is.
    let media = shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .media_at(1)
        .expect("the file is in slot 1");
    let entry = shell
        .media_store()
        .entry(bl_media::ContentHash::from_bytes(media.0))
        .unwrap();
    assert_eq!(entry.path, file);

    // One undo step each way.
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(choice_of(&shell, device, 1), None, "undo takes the file");
    assert_eq!(
        shell
            .editor()
            .document()
            .device(device)
            .unwrap()
            .media_at(1),
        None,
        "and its media"
    );
    assert!(choice_of(&shell, device, 0).is_some(), "the first stays");
    shell.run("edit.redo", json!({})).unwrap();
    assert_eq!(choice_of(&shell, device, 1), Some(user_choice(true, 3)));

    // A factory table after a file clears the file's media.
    load(
        &mut shell,
        device,
        json!({ "oscillator": 2, "category": "FM", "name": "Index Sweep" }),
    );
    assert_eq!(
        shell
            .editor()
            .document()
            .device(device)
            .unwrap()
            .media_at(1),
        None
    );
}

fn user_choice(raw: bool, frames: u16) -> WavetableChoice {
    WavetableChoice::User { raw, frames }
}

// Spec: wavetable R8
#[test]
fn load_wavetable_refuses_what_it_cannot_load() {
    let scratch = Scratch::new("refuse");
    let mut shell = headless_with_config(&scratch.path("config"));
    let device = add_device(&mut shell, bench(), DeviceRole::Instrument);
    let steps = shell.editor().undo_history().count();
    let mut refused = |args: Value| {
        let mut args = args;
        args["device"] = json!(device);
        assert!(
            shell.run("device.load_wavetable", args.clone()).is_err(),
            "{args} should be refused"
        );
    };
    refused(json!({ "oscillator": 1, "category": "Additive", "name": "No Such Table" }));
    refused(json!({ "oscillator": 1, "category": "No Such Category", "name": "Odd Rise" }));
    refused(json!({ "oscillator": 1, "file": text(&fixture("empty.wav")) }));
    refused(json!({ "oscillator": 1, "file": text(&fixture("missing.wav")) }));
    refused(json!({ "oscillator": 3, "category": "Additive", "name": "Odd Rise" }));
    refused(json!({ "oscillator": 0, "category": "Additive", "name": "Odd Rise" }));
    refused(json!({ "oscillator": 1 }));
    refused(
        json!({ "oscillator": 1, "category": "Additive", "name": "Odd Rise",
                    "file": text(&fixture("short-300.wav")) }),
    );
    assert_eq!(
        shell.editor().undo_history().count(),
        steps,
        "nothing changed"
    );

    // A device whose type plays no wavetable is refused.
    let utility = bl_devices::utility::utility_type();
    let other = add_device(&mut shell, utility.kind, utility.role);
    assert!(
        shell
            .run(
                "device.load_wavetable",
                json!({ "device": other, "oscillator": 1, "category": "Additive", "name": "Odd Rise" })
            )
            .is_err()
    );
}

// Spec: wavetable R7
#[test]
fn a_user_table_is_saved_collected_and_reopened_identically() {
    let scratch = Scratch::new("collect");
    let outside = scratch.path("outside/Wave.wav");
    std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
    std::fs::copy(fixture("clm-4-frames.wav"), &outside).unwrap();

    let mut shell = headless_with_config(&scratch.path("config"));
    let device = add_device(&mut shell, bench(), DeviceRole::Instrument);
    let song = PathBuf::from(
        shell
            .run(
                "set.save_as",
                json!({ "path": text(&scratch.path("Song.blset")) }),
            )
            .unwrap()["path"]
            .as_str()
            .unwrap(),
    );
    load(
        &mut shell,
        device,
        json!({ "oscillator": 2, "file": text(&outside) }),
    );
    load(
        &mut shell,
        device,
        json!({ "oscillator": 1, "category": "Voices", "name": "Vowel Walk" }),
    );
    let before = user::read(&outside, false).unwrap();

    // The set reports the file with its missing files while it is gone.
    let moved = scratch.path("outside/Wave-away.wav");
    std::fs::rename(&outside, &moved).unwrap();
    assert_eq!(shell.missing_files(), 1, "the missing table is reported");
    std::fs::rename(&moved, &outside).unwrap();
    assert_eq!(shell.missing_files(), 0);

    let report = shell.run("set.collect_all_and_save", json!({})).unwrap();
    assert_eq!(report["files"], 1);
    let copy = song.parent().unwrap().join("Samples/Imported/Wave.wav");
    assert!(copy.is_file(), "the file was copied into the project");

    let mut reopened = Shell::headless();
    reopened
        .run("set.open", json!({ "path": text(&song) }))
        .unwrap();
    assert_eq!(reopened.missing_files(), 0);
    assert_eq!(
        choice_of(&reopened, device, 0),
        choice_of(&shell, device, 0)
    );
    assert_eq!(choice_of(&reopened, device, 1), Some(user_choice(false, 4)));
    // The reopened set builds the same table from the copy.
    let media = reopened
        .editor()
        .document()
        .device(device)
        .unwrap()
        .media_at(1)
        .unwrap();
    let path = reopened
        .media_store()
        .entry(bl_media::ContentHash::from_bytes(media.0))
        .unwrap()
        .path;
    assert!(path.starts_with(song.parent().unwrap()));
    let after = user::read(&path, false).unwrap();
    assert_eq!(
        bl_devices::wavetables::hash(&after),
        bl_devices::wavetables::hash(&before)
    );
}

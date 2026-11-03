//! WP-8.27's capture in the shell (the Granulator III spec R19, R21, R23,
//! R24): `granulator.capture` by ID captures a track's audio, the capture
//! ends as one undo step with its file in the project's `Samples/Recorded`,
//! and the set reopens playing that file. It is refused with No Input.

use std::path::PathBuf;

use bl_devices::granulator_iii::{CAPTURE_LENGTH, granulator_iii_type};
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_model::{Command, Device, DeviceId, DeviceKind, TrackId};
use bl_shell::Shell;
use bl_shell::testing;
use serde_json::{Value, json};

/// A folder for one test's files, removed when the test ends.
struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "bl-shell-granulator-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A saved set with a tone on an audio track that only the Granulator III
/// on a MIDI track hears, Capture Length at its least.
fn set(dir: &TestDir) -> (Shell, bl_audio_io::NullControl, TrackId, DeviceId, PathBuf) {
    let (mut shell, control) = testing::rig();
    shell.start_audio();
    let saved = shell
        .run(
            "set.save_as",
            json!({ "path": dir.0.join("Capture.blset").to_string_lossy() }),
        )
        .unwrap();
    let path = PathBuf::from(saved["path"].as_str().unwrap());
    for kind in ["audio", "midi"] {
        shell
            .invoke("track.create", json!({ "kind": kind, "name": kind }))
            .unwrap();
    }
    let tracks: Vec<TrackId> = shell.editor().document().top_level().collect();
    let (source, host) = (tracks[tracks.len() - 2], tracks[tracks.len() - 1]);
    let tone_id: DeviceId = shell.new_id();
    let mut tone = Device::new(
        tone_id,
        "",
        DeviceKind::Native {
            device_type: TEST_TONE.to_owned(),
        },
    );
    tone.params.insert(test_tone::LEVEL, -6.0);
    let id: DeviceId = shell.new_id();
    let granulator = granulator_iii_type().device(id);
    shell.edit(|editor| {
        editor
            .apply(Command::InsertDevice {
                track: source,
                index: 0,
                device: tone,
            })
            .unwrap();
        editor
            .apply(Command::InsertDevice {
                track: host,
                index: 0,
                device: granulator,
            })
            .unwrap();
    });
    shell
        .run(
            "routing.set_output",
            json!({ "track": source, "output": { "type": "sends_only" } }),
        )
        .unwrap();
    shell
        .run(
            "device.set_param",
            json!({ "device": id, "param": CAPTURE_LENGTH.0, "value": 1.0 }),
        )
        .unwrap();
    (shell, control, source, id, path)
}

fn capture(shell: &mut Shell, device: DeviceId, action: &str) -> Result<Value, String> {
    shell
        .run(
            "granulator.capture",
            json!({ "device": device, "action": action }),
        )
        .map_err(|error| error.to_string())
}

fn recorded(set: &std::path::Path) -> Vec<PathBuf> {
    let folder = bl_project::ProjectFolder::of_set(set)
        .root()
        .join("Samples")
        .join("Recorded");
    std::fs::read_dir(&folder)
        .map(|files| files.map(|f| f.unwrap().path()).collect())
        .unwrap_or_default()
}

// Spec: granulator iii R19
#[test]
fn a_capture_is_refused_with_no_input() {
    let dir = TestDir::new("refused");
    let (mut shell, control, _, device, _) = set(&dir);
    testing::settle(&control, &mut shell);
    let error = capture(&mut shell, device, "start").unwrap_err();
    assert!(error.contains("No Input"), "{error}");
}

// Spec: granulator iii R19, R21, R23, R24
#[test]
fn a_capture_ends_as_one_undo_step_with_its_file_in_the_project_and_reopens_identically() {
    let dir = TestDir::new("capture");
    let (mut shell, control, source, device, path) = set(&dir);
    shell
        .run(
            "device.set_sidechain_input",
            json!({ "device": device, "input": { "type": "track", "track": source, "tap": "post_fx" } }),
        )
        .unwrap();
    testing::settle(&control, &mut shell);
    let steps = shell.editor().history().len();

    let started = capture(&mut shell, device, "start").unwrap();
    assert_eq!(started["capturing"], true);
    assert!(
        capture(&mut shell, device, "start").is_err(),
        "one at a time"
    );
    testing::advance(&control, &mut shell, 1_200);
    shell.finish_granulator_captures();

    // One undo step, the file in the project folder, the device playing it.
    let history = shell.editor().history();
    assert_eq!(history.len(), steps + 1);
    assert_eq!(
        history
            .iter()
            .filter(|step| step.label == "Capture Audio")
            .count(),
        1
    );
    let files = recorded(&path);
    assert_eq!(files.len(), 1, "{files:?}");
    let name = files[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with("audio Granulator III Capture [")
            || name.contains(" Granulator III Capture ["),
        "{name}"
    );
    let audio = bl_media::decode_file(&files[0]).unwrap();
    assert_eq!(audio.channel_count(), 2);
    assert_eq!(audio.frames(), 48_000);
    let samples = audio.channel(0).unwrap();
    assert!(samples.iter().any(|s| s.abs() > 0.1), "the tone is in it");
    let media = shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .media_at(0);
    assert!(media.is_some());

    // The query tool's capture reading.
    let read = shell
        .capture_of(shell.editor().document().device(device).unwrap())
        .unwrap();
    assert_eq!(read["capturing"], false);
    assert_eq!(read["length"], 1.0);
    assert_eq!(read["file"], json!(files[0]));

    // Undo brings back the previous source; the file stays.
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(
        shell
            .editor()
            .document()
            .device(device)
            .unwrap()
            .media_at(0),
        None
    );
    assert_eq!(recorded(&path).len(), 1);
    shell.run("edit.redo", json!({})).unwrap();

    // Reopened, the set plays the same file.
    shell.run("set.save", json!({})).unwrap();
    shell.run("set.open", json!({ "path": path })).unwrap();
    let reopened = shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .media_at(0);
    assert_eq!(reopened, media);
}

// Spec: granulator iii R19
#[test]
fn stop_ends_a_capture_before_its_length() {
    let dir = TestDir::new("stop");
    let (mut shell, control, source, device, path) = set(&dir);
    shell
        .run(
            "device.set_sidechain_input",
            json!({ "device": device, "input": { "type": "track", "track": source, "tap": "post_fx" } }),
        )
        .unwrap();
    testing::settle(&control, &mut shell);
    capture(&mut shell, device, "start").unwrap();
    testing::advance(&control, &mut shell, 100);
    capture(&mut shell, device, "stop").unwrap();
    testing::advance(&control, &mut shell, 50);
    shell.finish_granulator_captures();
    let files = recorded(&path);
    assert_eq!(files.len(), 1);
    let frames = bl_media::decode_file(&files[0]).unwrap().frames();
    assert!((4_000..48_000).contains(&frames), "{frames}");
}

// Spec: granulator iii R21. A capture that ends while the set is saved is
// in the saved set, with its file.
#[test]
fn a_capture_that_ends_as_the_set_is_saved_is_not_lost() {
    let dir = TestDir::new("saving");
    let (mut shell, control, source, device, path) = set(&dir);
    shell
        .run(
            "device.set_sidechain_input",
            json!({ "device": device, "input": { "type": "track", "track": source, "tap": "post_fx" } }),
        )
        .unwrap();
    testing::settle(&control, &mut shell);
    capture(&mut shell, device, "start").unwrap();
    testing::advance(&control, &mut shell, 1_200);
    shell.run("set.save", json!({})).unwrap();
    shell.run("set.open", json!({ "path": path })).unwrap();
    assert!(
        shell
            .editor()
            .document()
            .device(device)
            .unwrap()
            .media_at(0)
            .is_some()
    );
    assert_eq!(recorded(&path).len(), 1);
}

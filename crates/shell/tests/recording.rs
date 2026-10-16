//! WP-2.13's acceptance with the test backend: arm, monitoring, and
//! Arrangement Record writing each armed track's input to a WAV in the
//! project's `Samples/Recorded/`, and making the clip when recording stops.
//!
//! - A recorded sine is in the file to the sample.
//! - The clip lands where recording started, and undo removes it while the
//!   file stays.
//! - A recording that can't be written is reported, and makes no clip.
//! - The whole path allocates nothing on the audio thread.

bl_rt::install_trap!();

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bl_audio_io::{BackendKind, NullBackend, NullControl, NullOptions};
use bl_core::time::{SampleRate, Ticks};
use bl_media::MediaStore;
use bl_model::TrackId;
use bl_shell::Shell;
use bl_shell::audio::{AudioHost, AudioSettings};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use serde_json::{Value, json};

/// A folder for one test's files, removed when the test ends.
struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "bl-shell-recording-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    /// A path for a command's arguments.
    fn arg(&self, relative: &str) -> String {
        self.0.join(relative).to_string_lossy().into_owned()
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A shell on a four-channel null device, with its input open, and the
/// control over the device.
fn duplex_shell() -> (Shell, NullControl) {
    let (backend, control) = NullBackend::with_devices(
        NullOptions {
            channels: 4,
            ..NullOptions::default()
        },
        &[("null", "Null")],
    );
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

/// An armed audio track whose Audio From is Ext. In `channel`.
fn armed_track(shell: &mut Shell, channel: u16, name: &str) -> TrackId {
    shell
        .invoke("track.create", json!({ "kind": "audio", "name": name }))
        .unwrap();
    let track = shell
        .editor()
        .document()
        .top_level()
        .last()
        .expect("the new track");
    shell
        .run(
            "routing.set_input",
            json!({
                "track": track,
                "input": { "type": "external", "channel": { "type": "mono", "channel": channel } },
            }),
        )
        .unwrap();
    shell
        .run("mixer.set_arm", json!({ "track": track, "arm": true }))
        .unwrap();
    track
}

/// Runs the engine for `millis`, polling as the app does.
fn play_for(shell: &mut Shell, millis: u64) {
    let until = Instant::now() + Duration::from_millis(millis);
    while Instant::now() < until {
        shell.poll_engine();
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The one clip on `track`, once the recording's clips have appeared.
fn wait_for_clip(shell: &mut Shell, track: TrackId) -> bl_model::Clip {
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        shell.poll_engine();
        let clips: Vec<bl_model::Clip> = shell
            .editor()
            .document()
            .track(track)
            .map(|track| track.clips().cloned().collect())
            .unwrap_or_default();
        if let Some(clip) = clips.first() {
            assert_eq!(clips.len(), 1, "one clip per armed track");
            return clip.clone();
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the recording made no clip");
}

/// The one file a recording left in the project's `Samples/Recorded`.
fn recorded_file(folder: &Path) -> PathBuf {
    let recorded = folder.join("Samples").join("Recorded");
    let mut names: Vec<PathBuf> = std::fs::read_dir(&recorded)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "wav"))
        .collect();
    assert_eq!(names.len(), 1, "one file per armed track: {names:?}");
    names.remove(0)
}

/// The sample the null device's quarter-rate tone carries `i` frames into a
/// take that starts on phase `start` (0 to 3 quarter turns), exactly as the
/// backend makes it: the phase advances by a quarter turn a frame, wrapping
/// at one, and every value is exact.
fn tone(amplitude: f32, start: usize, i: usize) -> f32 {
    let phase = ((start + i) % 4) as f64 * 0.25;
    (std::f64::consts::TAU * phase).sin() as f32 * amplitude
}

#[test]
fn a_recorded_tone_is_in_the_file_to_the_sample_and_lands_where_recording_started() {
    let dir = TestDir::new("sine");
    let (mut shell, control) = duplex_shell();
    let saved: Value = shell
        .run("set.save_as", json!({ "path": dir.arg("Take.blset") }))
        .unwrap();
    let set = PathBuf::from(saved["path"].as_str().unwrap());
    let folder = set.parent().unwrap().to_path_buf();

    let track = armed_track(&mut shell, 3, "Guitar");
    // The tone is a quarter of the device's rate, so its samples are a
    // four-frame pattern and the file can be checked against it to the bit.
    let rate = shell.audio().status().sample_rate.expect("a device runs");
    control.set_input_tone(2, f64::from(rate) / 4.0, 0.5);

    // Recording is on; playback starts on two quarter notes, where the take
    // starts, and stops after a moment.
    shell
        .run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    let start = Ticks::QUARTER + Ticks::QUARTER;
    shell
        .run("transport.locate", json!({ "position": start }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    play_for(&mut shell, 300);
    shell.run("transport.stop", json!({})).unwrap();

    let clip = wait_for_clip(&mut shell, track);
    let path = recorded_file(&folder);
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    let label = bl_shell::routing::track_label(shell.editor().document(), track);
    assert!(
        name.starts_with(&format!("{label} 0001 [")) && name.ends_with("].wav"),
        "{name}"
    );

    // The clip lands where recording started: the insert marker's frame,
    // through the tempo map.
    let tempo = shell.editor().document().settings().tempo.clone();
    let rate = SampleRate::new(rate).unwrap();
    let start_frame = tempo.ticks_to_frames(start, rate);
    assert_eq!(clip.start, tempo.frames_to_ticks(start_frame, rate));

    // The file holds the tone, to the sample: the samples are the sine the
    // device carried, one of the four phases it can start on.
    let media = MediaStore::new();
    let hash = media.import(&path).unwrap();
    let audio = media.load(hash).unwrap();
    assert_eq!(audio.channel_count(), 1, "a mono input records a mono file");
    let samples = audio.channel(0).unwrap();
    let fits = (0..4usize)
        .find(|&start| {
            samples
                .iter()
                .enumerate()
                .all(|(i, &sample)| sample == tone(0.5, start, i))
        })
        .unwrap_or_else(|| panic!("the file isn't the tone: {:?}", &samples[..8]));
    assert!(fits < 4);

    // The clip plays the whole file, within the tempo map's rounding.
    let clip_frames = tempo.ticks_to_frames(clip.end(), rate).get()
        - tempo.ticks_to_frames(clip.start, rate).get();
    assert!(
        (clip_frames - samples.len() as i64).abs() <= 2,
        "the clip holds the file: {clip_frames} frames against {}",
        samples.len()
    );
    assert!(samples.len() > 4_000, "recorded for a moment");

    // The whole path stayed off the heap.
    let stats = shell.audio().stats().expect("a stream is open");
    assert!(stats.blocks > 0);
    assert_eq!(stats.heap_operations, 0, "{stats:?}");

    // One undo step: undoing removes the clip, and the file stays, as Live
    // keeps recorded files.
    shell.run("edit.undo", json!({})).unwrap();
    assert!(
        shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .clips()
            .len()
            == 0,
        "the recording is one undo step"
    );
    assert!(path.exists(), "the file stays");

    // The set's media table lists the file, so a save keeps it.
    shell.run("set.save", json!({})).unwrap();
    let (project, _) = bl_project::load(&set).unwrap();
    assert!(
        project
            .media
            .get(&bl_model::ContentHash(*hash.as_bytes()))
            .is_some(),
        "the recording is in the set's media"
    );
}

// Spec: app-shell R13. A recording that can't write its file is reported
// and makes no clip.
#[cfg(unix)]
#[test]
fn a_recording_that_cannot_write_is_reported_and_makes_no_clip() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TestDir::new("unwritable");
    let (mut shell, control) = duplex_shell();
    let saved: Value = shell
        .run("set.save_as", json!({ "path": dir.arg("Take.blset") }))
        .unwrap();
    let set = PathBuf::from(saved["path"].as_str().unwrap());
    let folder = set.parent().unwrap().to_path_buf();
    let recorded = folder.join("Samples").join("Recorded");

    let track = armed_track(&mut shell, 3, "Guitar");
    control.set_input_tone(2, 12_000.0, 0.5);
    // The folder can't hold a file: the writer reports it.
    std::fs::create_dir_all(&recorded).unwrap();
    std::fs::set_permissions(&recorded, std::fs::Permissions::from_mode(0o555)).unwrap();

    shell
        .run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    play_for(&mut shell, 200);
    shell.run("transport.stop", json!({})).unwrap();
    // Give the writer and the shell a few polls to report.
    for _ in 0..40 {
        shell.poll_engine();
        std::thread::sleep(Duration::from_millis(5));
    }

    let message = shell.message().map(|message| message.text.clone());
    assert!(
        message
            .as_deref()
            .is_some_and(|text| text.starts_with("Recording failed")),
        "{message:?}"
    );
    assert_eq!(
        shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .clips()
            .len(),
        0,
        "a failed recording makes no clip"
    );
    std::fs::set_permissions(&recorded, std::fs::Permissions::from_mode(0o755)).unwrap();
}

// Spec: recording R3 (WP-2.14). An armed MIDI track needs no project
// folder: a set that has never been saved refuses the audio track's file
// and still records. With no notes played the track gets no clip, and the
// MIDI path stays off the heap.
#[test]
fn an_armed_midi_track_records_without_a_folder_and_makes_no_clip_without_notes() {
    let (mut shell, _control) = duplex_shell();
    let audio = armed_track(&mut shell, 3, "Guitar");
    shell
        .invoke("track.create", json!({ "kind": "midi", "name": "Keys" }))
        .unwrap();
    let keys = shell.editor().document().top_level().last().unwrap();
    shell
        .run("mixer.set_arm", json!({ "track": keys, "arm": true }))
        .unwrap();

    shell
        .run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    play_for(&mut shell, 200);
    shell.run("transport.stop", json!({})).unwrap();
    for _ in 0..40 {
        shell.poll_engine();
        std::thread::sleep(Duration::from_millis(5));
    }

    let message = shell.message().map(|message| message.text.clone());
    assert!(
        message
            .as_deref()
            .is_some_and(|text| text.starts_with("the set has never been saved")),
        "{message:?}"
    );
    for track in [audio, keys] {
        let clips = shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .clips()
            .len();
        assert_eq!(clips, 0);
    }
    let stats = shell.audio().stats().expect("a stream is open");
    assert!(stats.blocks > 0);
    assert_eq!(stats.heap_operations, 0, "{stats:?}");
}

// Spec: WP-2.14. MIDI Arrangement Overdub is a command, saved with the set,
// and one undo step.
#[test]
fn midi_arrangement_overdub_is_one_undo_step_saved_with_the_set() {
    let dir = TestDir::new("overdub");
    let (mut shell, _control) = duplex_shell();
    assert!(!shell.editor().document().settings().midi_overdub);
    shell
        .run("transport.set_midi_overdub", json!({ "enabled": true }))
        .unwrap();
    assert!(shell.editor().document().settings().midi_overdub);
    let saved: Value = shell
        .run("set.save_as", json!({ "path": dir.arg("Dub.blset") }))
        .unwrap();
    let (project, _) = bl_project::load(Path::new(saved["path"].as_str().unwrap())).unwrap();
    assert!(project.document.settings().midi_overdub);
    shell.run("edit.undo", json!({})).unwrap();
    assert!(!shell.editor().document().settings().midi_overdub);
}

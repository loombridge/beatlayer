//! WP-2.13's acceptance with the test backend: arm, monitoring, and
//! Arrangement Record writing each armed track's input to a WAV in the
//! project's `Samples/Recorded/`, and making the clip when recording stops.
//!
//! - A recorded sine is in the file to the sample.
//! - The clip lands where recording started, and undo removes it while the
//!   file stays.
//! - A recording that can't be written is reported, and makes no clip.
//! - Over a loopback of known delay, a recording lines up to the sample once
//!   the device's latency and Driver Error Compensation add up to the delay,
//!   and changing the compensation moves new recordings by exactly that much.
//! - Punch-In and Punch-Out at the loop brace record only between its ends.
//! - The whole path allocates nothing on the audio thread.

use std::path::{Path, PathBuf};

use bl_audio_io::{BackendKind, NullBackend, NullControl, NullOptions, Pacing};
use bl_core::time::{Bpm, SampleRate, TempoMap, TempoSegment, Ticks};
use bl_media::MediaStore;
use bl_model::TrackId;
use bl_shell::Shell;
use bl_shell::audio::{AudioHost, AudioSettings};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::testing;
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
    let (backend, control) = four_channel_backend();
    let shell = shell_on(backend, &control);
    (shell, control)
}

/// The null backend, paced by hand so a test drives the blocks (T199).
fn four_channel_backend() -> (NullBackend, NullControl) {
    NullBackend::with_devices(
        NullOptions {
            dropout_after: None,
            channels: 4,
            pacing: Pacing::Manual,
            ..NullOptions::default()
        },
        &[("null", "Null")],
    )
}

/// A shell on `backend`, with its input open and its clocks driven by
/// `control` (T199).
fn shell_on(backend: NullBackend, control: &NullControl) -> Shell {
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
    // Before the stream opens: `audio.configure` opens it.
    shell.use_test_clock(control.clock().expect("manual pacing"));
    shell
        .run("audio.configure", json!({ "input_device": "null:null" }))
        .unwrap();
    shell.start_audio();
    shell
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

/// Runs the engine for `millis` of test time, polling as the app does. The
/// null device is paced by hand (T199), so this lands on the same frames
/// however busy the machine is.
fn play_for(control: &NullControl, shell: &mut Shell, millis: u64) {
    testing::advance(control, shell, millis);
}

/// The one clip on `track`, once the recording's clips have appeared.
fn wait_for_clip(control: &NullControl, shell: &mut Shell, track: TrackId) -> bl_model::Clip {
    let mut found = None;
    testing::wait_for(control, shell, "the recording's clip", |shell| {
        let clips: Vec<bl_model::Clip> = shell
            .editor()
            .document()
            .track(track)
            .map(|track| track.clips().cloned().collect())
            .unwrap_or_default();
        match clips.first() {
            Some(clip) => {
                assert_eq!(clips.len(), 1, "one clip per armed track");
                found = Some(clip.clone());
                true
            }
            None => false,
        }
    });
    found.expect("the recording's clip")
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
    play_for(&control, &mut shell, 300);
    shell.run("transport.stop", json!({})).unwrap();

    let clip = wait_for_clip(&control, &mut shell, track);
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

/// `transport.state` as an agent reads it.
fn transport_state(shell: &mut Shell) -> Value {
    shell.run("transport.state", json!({})).unwrap()
}

// A running recording shows in `transport.state`: Arrangement Record, the
// take, the armed tracks and the frames dropped; once it ends, the report
// says what it wrote and that it made its clip.
#[test]
fn a_running_recording_and_the_last_one_s_report_read_back_from_transport_state() {
    let dir = TestDir::new("readback");
    let (mut shell, control) = duplex_shell();
    shell
        .run("set.save_as", json!({ "path": dir.arg("Take.blset") }))
        .unwrap();
    let track = armed_track(&mut shell, 3, "Guitar");
    control.set_input_tone(2, 12_000.0, 0.5);

    let idle = transport_state(&mut shell);
    assert_eq!(idle["arrangement_record"], false);
    assert_eq!(idle["take_running"], false);
    assert_eq!(idle["recording"], Value::Null);
    assert_eq!(idle["last_recording"], Value::Null);

    shell
        .run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    testing::wait_for(&control, &mut shell, "the take to run", |shell| {
        transport_state(shell)["take_running"] == true
    });
    let state = transport_state(&mut shell);
    assert_eq!(state["arrangement_record"], true);
    assert_eq!(state["take_running"], true);
    assert_eq!(state["counting_in"], false);
    assert_eq!(state["recording"]["armed_tracks"], json!([track]));
    assert_eq!(state["recording"]["dropped_frames"], 0);

    shell.run("transport.stop", json!({})).unwrap();
    wait_for_clip(&control, &mut shell, track);
    let state = transport_state(&mut shell);
    assert_eq!(state["arrangement_record"], false);
    assert_eq!(state["take_running"], false);
    assert_eq!(state["recording"], Value::Null);
    let report = &state["last_recording"];
    assert_eq!(report["error"], Value::Null);
    assert_eq!(report["clips_made"], true);
    assert_eq!(report["dropped_frames"], 0);
    assert_eq!(report["files"][0]["track"], json!(track));
    assert_eq!(report["files"][0]["channels"], 1);
    assert!(report["files"][0]["frames"].as_u64().unwrap() > 0);
    assert!(
        report["files"][0]["path"]
            .as_str()
            .unwrap()
            .contains("Recorded")
    );
}

// The count-in shows as counting while it sounds, and no take runs until it
// is over.
#[test]
fn the_count_in_shows_in_transport_state_before_the_take_starts() {
    let (mut shell, control) = duplex_shell();
    armed_track(&mut shell, 3, "Guitar");
    shell
        .run(
            "transport.set_metronome_count_in",
            json!({ "count_in": "4_bars" }),
        )
        .unwrap();
    shell
        .run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    play_for(&control, &mut shell, 300);
    let state = transport_state(&mut shell);
    assert_eq!(state["counting_in"], true);
    assert_eq!(state["take_running"], false);
    assert_eq!(state["arrangement_record"], true);
    shell.run("transport.stop", json!({})).unwrap();
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
    play_for(&control, &mut shell, 200);
    shell.run("transport.stop", json!({})).unwrap();
    // Give the writer and the shell a few polls to report.
    testing::wait_for(
        &control,
        &mut shell,
        "the failure to be reported",
        |shell| shell.message().is_some(),
    );

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
    // The report an agent reads says the same.
    let state = transport_state(&mut shell);
    let report = &state["last_recording"];
    assert_eq!(report["clips_made"], false);
    assert!(
        report["error"].is_string(),
        "the failure is in the report: {report}"
    );
    std::fs::set_permissions(&recorded, std::fs::Permissions::from_mode(0o755)).unwrap();
}

// Spec: recording R3 (WP-2.14). An armed MIDI track needs no project
// folder: a set that has never been saved refuses the audio track's file
// and still records. With no notes played the track gets no clip, and the
// MIDI path stays off the heap.
#[test]
fn an_armed_midi_track_records_without_a_folder_and_makes_no_clip_without_notes() {
    let (mut shell, control) = duplex_shell();
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
    play_for(&control, &mut shell, 200);
    shell.run("transport.stop", json!({})).unwrap();
    testing::wait_for(
        &control,
        &mut shell,
        "the failure to be reported",
        |shell| shell.message().is_some(),
    );

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

// Spec: scales R7. A recorded clip is made from nothing, so it takes the
// set's current scale.
#[test]
fn a_recorded_clip_takes_the_current_scale() {
    let dir = TestDir::new("record-scale");
    let (mut shell, control) = duplex_shell();
    shell
        .run("set.save_as", json!({ "path": dir.arg("Take.blset") }))
        .unwrap();
    let track = armed_track(&mut shell, 3, "Guitar");
    let rate = shell.audio().status().sample_rate.expect("a device runs");
    control.set_input_tone(2, f64::from(rate) / 4.0, 0.5);
    shell
        .run(
            "song.set_scale",
            json!({ "enabled": true, "root": 9, "name": "minor_pentatonic" }),
        )
        .unwrap();
    shell
        .run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    play_for(&control, &mut shell, 300);
    shell.run("transport.stop", json!({})).unwrap();

    let clip = wait_for_clip(&control, &mut shell, track);
    assert_eq!(
        clip.scale,
        bl_model::Scale::new(9, bl_model::ScaleName::MinorPentatonic),
        "the recorded clip takes the current scale"
    );
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

/// A 16-bit mono WAV of `frames` frames at 48 kHz that is silent except for
/// one loud sample at `at`.
fn impulse_wav(path: &Path, frames: u32, at: u32) {
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
        let sample: i16 = if i == at { 30_000 } else { 0 };
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// Records over a loopback of 1,480 frames, on a device reporting 400 frames
/// of input latency and 600 of output latency, with Driver Error
/// Compensation `compensation_ms`, and returns the clip's start in ticks
/// and the frame of the file at which the loud sample of a clip that plays
/// it at song frame 5,000 is found.
fn impulse_in_recording(compensation_ms: f64) -> (Ticks, usize, Value) {
    let (start, loudest, status, _) = impulse_through(compensation_ms, None, 1);
    (start, loudest, status)
}

/// The same, with the impulse's track on Ext. Out `ext_out` (Main otherwise)
/// and the recorded track on Ext. In `listen`; also returns the recording's
/// peak.
fn impulse_through(
    compensation_ms: f64,
    ext_out: Option<Value>,
    listen: u16,
) -> (Ticks, usize, Value, f32) {
    let dir = TestDir::new("loopback");
    let (backend, control) = four_channel_backend();
    let mut shell = shell_on(
        backend.with_latency(400, 600).with_loopback(1_480),
        &control,
    );
    let saved: Value = shell
        .run("set.save_as", json!({ "path": dir.arg("Take.blset") }))
        .unwrap();
    let set = PathBuf::from(saved["path"].as_str().unwrap());
    let folder = set.parent().unwrap().to_path_buf();
    assert_eq!(shell.audio().status().sample_rate, Some(48_000));
    shell
        .run(
            "audio.configure",
            json!({ "driver_error_compensation_ms": compensation_ms }),
        )
        .unwrap();

    // A track plays the impulse; the recorded track hears the interface's
    // first input channel, which the loopback fills from its first output
    // channel. Its Monitor is off, so it doesn't feed back into the loop.
    let file = dir.0.join("Impulse.wav");
    impulse_wav(&file, 24_000, 5_000);
    shell
        .run(
            bl_shell::commands::SETTINGS_SET_CREATE_FADES_ON_CLIP_EDGES,
            json!({ "on": false }),
        )
        .unwrap();
    shell
        .invoke(
            "track.create",
            json!({ "kind": "audio", "name": "Impulse" }),
        )
        .unwrap();
    let player = shell.editor().document().top_level().last().unwrap();
    if let Some(channel) = ext_out {
        shell
            .run(
                "routing.set_output",
                json!({ "track": player, "output": { "type": "external", "channel": channel } }),
            )
            .unwrap();
    }
    shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [file.display().to_string()], "track": player, "at": 0 }),
        )
        .unwrap();
    let track = armed_track(&mut shell, listen, "Loopback");
    shell
        .run(
            "routing.set_monitor",
            json!({ "track": track, "monitor": "off" }),
        )
        .unwrap();
    // Let the clip's stream cue before playback starts.
    play_for(&control, &mut shell, 300);

    shell
        .run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    play_for(&control, &mut shell, 500);
    shell.run("transport.stop", json!({})).unwrap();
    let clip = wait_for_clip(&control, &mut shell, track);

    let media = MediaStore::new();
    let hash = media.import(recorded_file(&folder)).unwrap();
    let audio = media.load(hash).unwrap();
    let samples = audio.channel(0).unwrap();
    let loudest = (0..samples.len())
        .max_by(|&a, &b| samples[a].abs().total_cmp(&samples[b].abs()))
        .unwrap();
    let status = shell.run("audio.status", json!({})).unwrap();
    (clip.start, loudest, status, samples[loudest].abs())
}

// Spec: recording R7; WP-2.13. A sound played at song frame 5,000 comes back
// from a loopback 1,480 frames later; the device reports 1,000 of them and
// Driver Error Compensation of 10 ms (480 frames) the rest, so the recording
// is moved earlier by exactly the loop's delay and the sound is back at frame
// 5,000 of the file. Each 10 ms more moves new recordings 480 frames earlier,
// and the compensation counts in the Audio page's Overall Latency.
#[test]
fn a_loopback_records_aligned_and_the_compensation_moves_recordings_by_exactly_its_value() {
    let (start, at, status) = impulse_in_recording(10.0);
    assert_eq!(start, Ticks::ZERO, "the clip starts where recording did");
    assert_eq!(at, 5_000, "aligned to the sample");
    let overall = status["overall_latency_ms"].as_f64().unwrap();
    assert!((overall - 1_480.0 / 48.0).abs() < 1e-9, "{overall}");
    assert_eq!(status["input_latency_ms"].as_f64().unwrap(), 400.0 / 48.0);
    assert_eq!(status["settings"]["driver_error_compensation_ms"], 10.0);

    assert_eq!(impulse_in_recording(0.0).1, 5_480, "480 frames late");
    assert_eq!(impulse_in_recording(20.0).1, 4_520, "480 frames earlier");
    assert_eq!(
        impulse_in_recording(-10.0).1,
        5_960,
        "negative moves it later"
    );
}

// Spec: routing R3; WP-2.13. Over the loopback, which carries each output
// channel to the input of the same number, a clip on a track set to Ext. Out
// 3/4 is heard on input channels 3 and 4 and not on 1 or 2, so Main doesn't
// carry it.
#[test]
fn a_track_on_ext_out_reaches_its_device_channels_and_not_the_first_pair() {
    let pair_3_4 = json!({ "type": "pair", "channel": 3 });
    let (_, at, _, peak) = impulse_through(10.0, Some(pair_3_4.clone()), 3);
    assert_eq!(at, 5_000, "the impulse is on channel 3");
    assert!(peak > 0.5, "{peak}");
    let (_, _, _, peak) = impulse_through(10.0, Some(pair_3_4.clone()), 4);
    assert!(peak > 0.5, "the pair's right side is on channel 4: {peak}");
    let (_, _, _, peak) = impulse_through(10.0, Some(pair_3_4), 1);
    assert_eq!(peak, 0.0, "not on channel 1, so not in Main");
}

// Spec: recording R7; WP-2.13. The compensation keeps to two decimals and to
// 1,000 ms either way, doesn't restart the stream, and a value that isn't a
// number is refused.
#[test]
fn driver_error_compensation_is_kept_within_its_bounds_without_restarting_the_stream() {
    let (mut shell, _) = duplex_shell();
    let engines = shell.audio().engine_generation();
    for (asked, kept) in [(12.3456, 12.35), (5000.0, 1000.0), (-5000.0, -1000.0)] {
        shell
            .run(
                "audio.configure",
                json!({ "driver_error_compensation_ms": asked }),
            )
            .unwrap();
        assert_eq!(
            shell.audio().status().settings.driver_error_compensation_ms,
            kept
        );
    }
    assert_eq!(
        shell.audio().engine_generation(),
        engines,
        "the stream kept running"
    );
    assert_eq!(
        shell.preferences().audio.driver_error_compensation_ms,
        -1000.0,
        "kept with the Audio page's other settings"
    );
    assert!(
        shell
            .run(
                "audio.configure",
                json!({ "driver_error_compensation_ms": null })
            )
            .is_ok(),
        "null leaves it as it is"
    );
}

// Spec: recording R6; WP-2.13. With Punch-In and Punch-Out on and the Loop
// switch off, a recording started before the brace holds exactly the brace,
// and the clip lands on it.
#[test]
fn punch_records_only_between_the_brace_ends() {
    let dir = TestDir::new("punch");
    let (mut shell, control) = duplex_shell();
    let saved: Value = shell
        .run("set.save_as", json!({ "path": dir.arg("Take.blset") }))
        .unwrap();
    let set = PathBuf::from(saved["path"].as_str().unwrap());
    let folder = set.parent().unwrap().to_path_buf();
    let track = armed_track(&mut shell, 3, "Guitar");
    control.set_input_tone(2, 12_000.0, 0.5);

    // The brace is 6,000 to 12,000 frames at 120 BPM and 48 kHz.
    let start = Ticks::QUARTER / 4;
    shell
        .run(
            "transport.set_loop",
            json!({ "start": start.get(), "length": start.get() }),
        )
        .unwrap();
    for id in ["transport.set_punch_in", "transport.set_punch_out"] {
        shell.run(id, json!({ "enabled": true })).unwrap();
    }
    assert!(shell.editor().document().settings().punch_in);
    assert!(
        !shell
            .editor()
            .document()
            .settings()
            .arrangement_loop
            .enabled
    );

    shell
        .run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    play_for(&control, &mut shell, 600);
    shell.run("transport.stop", json!({})).unwrap();

    let clip = wait_for_clip(&control, &mut shell, track);
    let tempo = shell.editor().document().settings().tempo.clone();
    let rate = SampleRate::new(48_000).unwrap();
    assert_eq!(clip.start, start, "the clip starts at the punch-in point");
    let media = MediaStore::new();
    let hash = media.import(recorded_file(&folder)).unwrap();
    let frames = media.load(hash).unwrap().channel(0).unwrap().len() as i64;
    assert_eq!(frames, 6_000, "the file holds the brace and no more");
    let clip_frames = tempo.ticks_to_frames(clip.end(), rate).get()
        - tempo.ticks_to_frames(clip.start, rate).get();
    assert!((clip_frames - frames).abs() <= 2, "{clip_frames}");
}

/// The track's take lanes, once a recording has made them.
fn wait_for_lanes(
    control: &NullControl,
    shell: &mut Shell,
    track: TrackId,
) -> Vec<bl_model::TakeLane> {
    let mut found = Vec::new();
    testing::wait_for(control, shell, "the recording's take lane", |shell| {
        found = shell
            .editor()
            .document()
            .track(track)
            .map(|track| track.take_lanes().cloned().collect())
            .unwrap_or_default();
        !found.is_empty()
    });
    found
}

/// The track's Arrangement clips, in start order.
fn main_lane(shell: &Shell, track: TrackId) -> Vec<bl_model::Clip> {
    let mut clips: Vec<bl_model::Clip> = shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .clips()
        .cloned()
        .collect();
    clips.sort_by_key(|clip| clip.start);
    clips
}

// Spec: recording R12. Recording over the loop makes a take lane for each
// pass, in order, with the latest pass on the main lane and the part before
// the loop a clip of its own; each pass is one undo step.
#[test]
fn recording_over_a_loop_makes_a_take_lane_for_each_pass() {
    let dir = TestDir::new("loop");
    let (mut shell, control) = duplex_shell();
    let saved: Value = shell
        .run("set.save_as", json!({ "path": dir.arg("Loop.blset") }))
        .unwrap();
    let set = PathBuf::from(saved["path"].as_str().unwrap());
    let folder = set.parent().unwrap().to_path_buf();

    let track = armed_track(&mut shell, 3, "Guitar");
    let rate = shell.audio().status().sample_rate.expect("a device runs");
    control.set_input_tone(2, f64::from(rate) / 4.0, 0.5);

    // A loop a sixteenth note long (an eighth of a second at 120 bpm) on the
    // second beat, with recording starting a sixteenth before it.
    let sixteenth = Ticks::QUARTER / 4;
    shell
        .run(
            "transport.set_loop",
            json!({ "start": Ticks::QUARTER, "length": sixteenth }),
        )
        .unwrap();
    shell
        .run("transport.set_loop_enabled", json!({ "enabled": true }))
        .unwrap();
    shell
        .run(
            "transport.locate",
            json!({ "position": Ticks::QUARTER - sixteenth }),
        )
        .unwrap();
    shell
        .run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    play_for(&control, &mut shell, 700);
    shell.run("transport.stop", json!({})).unwrap();

    let lanes = wait_for_lanes(&control, &mut shell, track);
    assert!(lanes.len() >= 3, "several passes: {}", lanes.len());
    let names: Vec<&str> = lanes.iter().map(|lane| lane.name.as_str()).collect();
    let expected: Vec<String> = (1..=lanes.len()).map(|n| format!("Take {n}")).collect();
    assert_eq!(
        names, expected,
        "Take 1 nearest the main lane, the newest last"
    );
    assert!(lanes.iter().all(|lane| lane.clips.len() == 1));

    // The passes follow one another in the one file, each a loop long.
    let _ = recorded_file(&folder);
    let pass = |n: usize| &lanes[n].clips[0];
    let seconds = 1.0 / 8.0;
    for n in 1..lanes.len() - 1 {
        assert_eq!(pass(n).start, Ticks::QUARTER, "every pass sits on the loop");
        // Recorded audio is warped, so a pass's place in the file is where
        // its Start marker's beat maps to.
        let by = audio_of(pass(n)).in_seconds().start - audio_of(pass(n - 1)).in_seconds().start;
        assert!(
            (by - seconds).abs() < 1e-3,
            "pass {n} follows the one before: {by}"
        );
    }

    // The main lane: the lead-in, then the latest pass over the loop.
    let clips = main_lane(&shell, track);
    assert_eq!(clips[0].start, Ticks::QUARTER - sixteenth, "the lead-in");
    let latest = clips
        .iter()
        .find(|clip| clip.start == Ticks::QUARTER)
        .expect("a clip on the loop");
    let last = pass(lanes.len() - 1);
    assert_eq!(latest.data, last.data);
    assert_eq!(latest.length, last.length);
    assert_ne!(
        latest.id, last.id,
        "the main lane's clip is a copy of its own"
    );

    // The whole path stayed off the heap.
    let stats = shell.audio().stats().expect("a stream is open");
    assert_eq!(stats.heap_operations, 0, "{stats:?}");

    // The set round-trips the take lanes.
    shell.run("set.save", json!({})).unwrap();
    let (project, _) = bl_project::load(&set).unwrap();
    let saved: Vec<bl_model::TakeLane> = project
        .document
        .track(track)
        .unwrap()
        .take_lanes()
        .cloned()
        .collect();
    assert_eq!(saved, lanes, "the lanes come back as they were saved");

    // Each pass is an undo step: undo unrolls them from the last.
    shell.run("edit.undo", json!({})).unwrap();
    let after = shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .take_lanes()
        .len();
    assert_eq!(after, lanes.len() - 1, "one undo takes the last pass off");
    for _ in 1..lanes.len() {
        shell.run("edit.undo", json!({})).unwrap();
    }
    let track_now = shell.editor().document().track(track).unwrap();
    assert_eq!(track_now.take_lanes().len(), 0);
    assert_eq!(track_now.clips().len(), 0, "the recording is undone");
}

/// Records for `millis` from `start` on a new armed track, with the set saved
/// first so recording has a folder, and gives the clip it made.
fn record_take(
    control: &NullControl,
    shell: &mut Shell,
    dir: &TestDir,
    start: Ticks,
    millis: u64,
) -> bl_model::Clip {
    shell
        .run("set.save_as", json!({ "path": dir.arg("Take.blset") }))
        .unwrap();
    let track = armed_track(shell, 3, "Guitar");
    shell
        .run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    shell
        .run("transport.locate", json!({ "position": start }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    play_for(control, shell, millis);
    shell.run("transport.stop", json!({})).unwrap();
    wait_for_clip(control, shell, track)
}

fn audio_of(clip: &bl_model::Clip) -> bl_model::AudioData {
    match &clip.data {
        bl_model::ClipData::Audio(audio) => audio.clone(),
        bl_model::ClipData::Midi(_) => unreachable!("a recorded audio clip"),
    }
}

// Spec: warping R37
#[test]
fn recorded_audio_is_warped_as_the_setting_says() {
    // On, as it is in a new install: a marker at the take's start, and the
    // set's tempo as the clip's.
    let dir = TestDir::new("warped");
    let (mut shell, control) = duplex_shell();
    let start = Ticks::QUARTER * 2;
    let audio = audio_of(&record_take(&control, &mut shell, &dir, start, 300));
    assert!(audio.warp && audio.start == 0.0, "{audio:?}");
    assert_eq!(audio.warp_markers.len(), 1);
    assert_eq!(audio.warp_markers[0].time, 0.0);
    assert_eq!(audio.warp_markers[0].beat, Ticks::ZERO);
    assert_eq!(audio.tempo, Some(120.0));
    assert_eq!(audio.warp_mode, bl_model::WarpMode::Beats);

    // Default Warp Mode is the warp mode of a new warped clip (R33).
    let dir = TestDir::new("warped-mode");
    let (mut shell, control) = duplex_shell();
    shell
        .run(
            "settings.set_default_warp_mode",
            json!({ "mode": "texture" }),
        )
        .unwrap();
    let audio = audio_of(&record_take(&control, &mut shell, &dir, start, 300));
    assert_eq!(audio.warp_mode, bl_model::WarpMode::Texture, "{audio:?}");

    // A tempo change inside the take gets a marker, at the time and beat the
    // file reaches it, and the take's last tempo is the clip's.
    let dir = TestDir::new("warped-change");
    let (mut shell, control) = duplex_shell();
    let change = Ticks::QUARTER * 3;
    let tempo = TempoMap::new([
        TempoSegment::Constant {
            start: Ticks::ZERO,
            bpm: Bpm::new(120.0).unwrap(),
        },
        TempoSegment::Constant {
            start: change,
            bpm: Bpm::new(60.0).unwrap(),
        },
    ])
    .unwrap();
    shell
        .run(
            "song.set_tempo",
            json!({ "tempo": serde_json::to_value(tempo).unwrap() }),
        )
        .unwrap();
    let audio = audio_of(&record_take(&control, &mut shell, &dir, start, 900));
    assert_eq!(audio.warp_markers.len(), 2, "{audio:?}");
    let second = audio.warp_markers[1];
    assert!((second.time - 0.5).abs() < 0.005, "{second:?}");
    assert!((second.beat.get() - Ticks::QUARTER.get()).abs() < Ticks::QUARTER.get() / 50);
    assert_eq!(audio.tempo, Some(60.0));

    // Off, a recorded clip is unwarped.
    let dir = TestDir::new("unwarped");
    let (mut shell, control) = duplex_shell();
    shell
        .run("settings.set_warp_recorded_audio", json!({ "on": false }))
        .unwrap();
    let audio = audio_of(&record_take(&control, &mut shell, &dir, start, 300));
    assert!(!audio.warp && audio.warp_markers.is_empty(), "{audio:?}");
}

/// A new armed MIDI track, with the computer keyboard on to play it.
fn armed_midi_track(shell: &mut Shell, name: &str) -> TrackId {
    shell
        .invoke("track.create", json!({ "kind": "midi", "name": name }))
        .unwrap();
    let track = shell.editor().document().top_level().last().unwrap();
    shell
        .run("mixer.set_arm", json!({ "track": track, "arm": true }))
        .unwrap();
    shell
        .run("midi.toggle_computer_keyboard", json!({ "on": true }))
        .unwrap();
    track
}

/// Plays the keyboard's A key for `millis`, after `wait` milliseconds.
fn tap_a(control: &NullControl, shell: &mut Shell, wait: u64, millis: u64) {
    play_for(control, shell, wait);
    for pressed in [true, false] {
        shell
            .run(
                "midi.computer_keyboard_key",
                json!({ "key": "A", "pressed": pressed }),
            )
            .unwrap();
        if pressed {
            play_for(control, shell, millis);
        }
    }
}

/// The track's take lanes, once `count` clips are on them in all.
fn lanes_with_clips(
    control: &NullControl,
    shell: &mut Shell,
    track: TrackId,
    count: usize,
) -> Vec<bl_model::TakeLane> {
    let mut found = Vec::new();
    testing::wait_for(control, shell, "the take lanes' clips", |shell| {
        found = shell
            .editor()
            .document()
            .track(track)
            .map(|track| track.take_lanes().cloned().collect())
            .unwrap_or_default();
        found.iter().map(|lane| lane.clips.len()).sum::<usize>() >= count
    });
    found
}

fn notes_of(clip: &bl_model::Clip) -> Vec<(u8, i64)> {
    let bl_model::ClipData::Midi(midi) = &clip.data else {
        panic!("a MIDI clip: {clip:?}");
    };
    let mut notes: Vec<(u8, i64)> = midi.notes.iter().map(|n| (n.key, n.start.get())).collect();
    notes.sort_unstable();
    notes
}

/// Records from `start` for `millis` on `track`, then stops.
fn record_from(control: &NullControl, shell: &mut Shell, start: Ticks, millis: u64) {
    shell
        .run("transport.locate", json!({ "position": start }))
        .unwrap();
    shell
        .run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    // The commands reach the audio thread through the compiler's queue, which
    // may still be compiling the last take's clips: every take records the
    // same frames only once they're on the audio thread's queue.
    assert!(
        shell.audio().flush(std::time::Duration::from_secs(10)),
        "the engine never took Play"
    );
    play_for(control, shell, millis);
    shell.run("transport.stop", json!({})).unwrap();
    assert!(
        shell.audio().flush(std::time::Duration::from_secs(10)),
        "the engine never took Stop"
    );
}

// Spec: comping R10, R11; recording R16. An audio take goes on a take lane
// and is copied to the main lane, in the one undo step that places it; a
// take after it reuses the lane, and one over the same bars gets a lane of
// its own.
#[test]
fn an_audio_take_goes_on_a_take_lane_and_a_later_take_reuses_it() {
    let dir = TestDir::new("lane-reuse");
    let (mut shell, control) = duplex_shell();
    shell
        .run("set.save_as", json!({ "path": dir.arg("Lanes.blset") }))
        .unwrap();
    let track = armed_track(&mut shell, 3, "Guitar");
    let rate = shell.audio().status().sample_rate.expect("a device runs");
    control.set_input_tone(2, f64::from(rate) / 4.0, 0.5);

    // A first take of a second, then the same bars again, then a verse after.
    record_from(&control, &mut shell, Ticks::ZERO, 500);
    let lanes = lanes_with_clips(&control, &mut shell, track, 1);
    assert_eq!(lanes.len(), 1);
    assert_eq!(lanes[0].name, "Take 1");
    let on_lane = &lanes[0].clips[0];
    let main = main_lane(&shell, track);
    assert_eq!(main.len(), 1, "the main lane plays the take");
    assert_ne!(main[0].id, on_lane.id, "a copy of its own");
    assert_eq!(
        (main[0].start, main[0].length, &main[0].data, &main[0].name),
        (on_lane.start, on_lane.length, &on_lane.data, &on_lane.name)
    );
    assert!(!on_lane.name.is_empty(), "named after its file");
    assert_eq!(shell.editor().undo_history().next(), Some("Recorded Clip"));

    // Over the same bars: a lane of its own, and the main lane takes the new one.
    record_from(&control, &mut shell, Ticks::ZERO, 500);
    let lanes = lanes_with_clips(&control, &mut shell, track, 2);
    let names: Vec<&str> = lanes.iter().map(|lane| lane.name.as_str()).collect();
    assert_eq!(names, ["Take 1", "Take 2"]);
    let main = main_lane(&shell, track);
    assert_eq!(main.len(), 1);
    assert_eq!(main[0].data, lanes[1].clips[0].data);

    // After both: the first lane is free from there on, so it takes the verse.
    record_from(&control, &mut shell, Ticks::WHOLE * 4, 300);
    let lanes = lanes_with_clips(&control, &mut shell, track, 3);
    assert_eq!(lanes.len(), 2, "no third lane");
    assert_eq!(lanes[0].clips.len(), 2, "the verse is on the first lane");
    assert_eq!(lanes[1].clips.len(), 1);
    assert_eq!(main_lane(&shell, track).len(), 2);

    // Each take is one undo step: undoing the verse takes its lane clip with
    // its main-lane copy.
    shell.run("edit.undo", json!({})).unwrap();
    let held = shell.editor().document().track(track).unwrap();
    assert_eq!(held.clips().len(), 1);
    assert_eq!(
        held.take_lanes()
            .map(|lane| lane.clips.len())
            .sum::<usize>(),
        2
    );
}

// Spec: comping R10 to R12; recording R16. A MIDI take goes on a take lane
// and is copied to the main lane, and a take over the same bars gets a lane
// of its own; with MIDI Arrangement Overdub on the main lane merges them.
#[test]
fn a_midi_take_goes_on_a_take_lane_and_overdub_merges_only_the_main_lane() {
    let (mut shell, control) = duplex_shell();
    let track = armed_midi_track(&mut shell, "Keys");

    shell
        .run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    tap_a(&control, &mut shell, 100, 100);
    play_for(&control, &mut shell, 100);
    shell.run("transport.stop", json!({})).unwrap();
    let lanes = lanes_with_clips(&control, &mut shell, track, 1);
    assert_eq!(lanes.len(), 1);
    assert_eq!(lanes[0].name, "Take 1");
    let main = main_lane(&shell, track);
    assert_eq!(main.len(), 1);
    assert_ne!(main[0].id, lanes[0].clips[0].id);
    assert_eq!(notes_of(&main[0]), notes_of(&lanes[0].clips[0]));
    assert_eq!(notes_of(&main[0]).len(), 1);
    assert_eq!(shell.editor().undo_history().next(), Some("Recorded Clip"));

    // A second take over the same bars replaces the main lane's clip.
    record_from_midi(&control, &mut shell, false);
    let lanes = lanes_with_clips(&control, &mut shell, track, 2);
    assert_eq!(lanes.len(), 2, "a lane of its own");
    let main = main_lane(&shell, track);
    assert_eq!(main.len(), 1, "the main lane plays the latest take");
    assert_eq!(notes_of(&main[0]), notes_of(&lanes[1].clips[0]));

    // With Overdub on, a third take merges into the main lane's clip and
    // the lanes hold each take alone.
    shell
        .run("transport.set_midi_overdub", json!({ "enabled": true }))
        .unwrap();
    record_from_midi(&control, &mut shell, true);
    let lanes = lanes_with_clips(&control, &mut shell, track, 3);
    assert_eq!(lanes.len(), 3);
    assert!(lanes.iter().all(|lane| notes_of(&lane.clips[0]).len() == 1));
    let main = main_lane(&shell, track);
    assert_eq!(main.len(), 1);
    assert_eq!(
        notes_of(&main[0]).len(),
        2,
        "the third take's note joined the second's"
    );
}

/// A MIDI take over the same bars as the first, with a note at another
/// moment if `later`.
fn record_from_midi(control: &NullControl, shell: &mut Shell, later: bool) {
    shell
        .run("transport.locate", json!({ "position": Ticks::ZERO }))
        .unwrap();
    shell
        .run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    tap_a(control, shell, if later { 250 } else { 100 }, 100);
    play_for(control, shell, 100);
    shell.run("transport.stop", json!({})).unwrap();
}

// Spec: comping R12; recording R12, R16. MIDI passes round the loop each go
// on a take lane and are copied to the main lane one by one, each in an undo
// step of its own, rather than merging into one clip.
#[test]
fn midi_passes_round_the_loop_each_go_on_a_take_lane() {
    let (mut shell, control) = duplex_shell();
    let track = armed_midi_track(&mut shell, "Keys");
    let sixteenth = Ticks::QUARTER / 4;
    shell
        .run(
            "transport.set_loop",
            json!({ "start": Ticks::QUARTER, "length": sixteenth }),
        )
        .unwrap();
    shell
        .run("transport.set_loop_enabled", json!({ "enabled": true }))
        .unwrap();
    shell
        .run(
            "transport.locate",
            json!({ "position": Ticks::QUARTER - sixteenth }),
        )
        .unwrap();
    shell
        .run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    // A sixteenth is 125 ms: the lead-in, then a note in the first pass and
    // another in the second.
    tap_a(&control, &mut shell, 150, 20);
    tap_a(&control, &mut shell, 105, 20);
    play_for(&control, &mut shell, 400);
    shell.run("transport.stop", json!({})).unwrap();

    let lanes = lanes_with_clips(&control, &mut shell, track, 2);
    assert_eq!(lanes.len(), 2, "a lane for each pass that was played");
    assert!(lanes.iter().all(|lane| lane.clips.len() == 1));
    for lane in &lanes {
        let clip = &lane.clips[0];
        assert_eq!(
            (clip.start, clip.length),
            (Ticks::QUARTER, sixteenth),
            "{lane:?}"
        );
        assert_eq!(notes_of(clip).len(), 1);
    }
    let main = main_lane(&shell, track);
    assert_eq!(main.len(), 1, "the latest pass over the loop");
    assert_eq!(notes_of(&main[0]), notes_of(&lanes[1].clips[0]));

    // Each pass is an undo step: undo unrolls them from the last.
    shell.run("edit.undo", json!({})).unwrap();
    let held = shell.editor().document().track(track).unwrap();
    assert_eq!(held.take_lanes().len(), 1);
    assert_eq!(
        notes_of(held.clips().next().unwrap()),
        notes_of(&lanes[0].clips[0])
    );
    shell.run("edit.undo", json!({})).unwrap();
    let held = shell.editor().document().track(track).unwrap();
    assert_eq!((held.take_lanes().len(), held.clips().len()), (0, 0));
}

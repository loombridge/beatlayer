//! WP-3.11's acceptance with the test backend: an armed track records into
//! a Session slot, the clip exists from the take's start and grows, the
//! recording is one undo step, an overdub adds a loop pass at a time, audio
//! goes to a file and becomes a looping slot clip, and the commands run by
//! ID. The rules are in the session recording spec.
//!
//! - Session Record on an armed MIDI track makes a looping clip in the
//!   selected scene's slot, or the next empty slot below.
//! - A new recording is one undo step; each pass of an overdub is one.
//! - An armed audio track records its input into `Samples/Recorded/`.
//! - A slot's record button refuses an occupied slot.
//! - The whole path allocates nothing on the audio thread.

use std::path::PathBuf;

use bl_audio_io::{BackendKind, NullBackend, NullControl, NullOptions, Pacing};
use bl_core::time::Ticks;
use bl_engine::SlotState;
use bl_model::{Clip, ClipData, MidiData, Note, SceneId, TrackId};
use bl_shell::Shell;
use bl_shell::audio::{AudioHost, AudioSettings};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::testing;
use serde_json::{Value, json};

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "bl-shell-session-recording-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn arg(&self, relative: &str) -> String {
        self.0.join(relative).to_string_lossy().into_owned()
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A shell on a four-channel null device with its input open, and the
/// control over the device. Launches and stops take effect at once, and the
/// computer MIDI keyboard is on.
fn rig() -> (Shell, NullControl) {
    let (backend, control) = NullBackend::with_devices(
        NullOptions {
            dropout_after: None,
            channels: 4,
            pacing: Pacing::Manual,
            ..NullOptions::default()
        },
        &[("null", "Null")],
    );
    let audio = AudioHost::with_backends(
        vec![BackendKind::Null],
        Box::new(move |_| Ok(Box::new(backend.clone()))),
        AudioSettings::default(),
    );
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        audio,
        Box::new(ProjectFiles::new()),
    );
    shell.use_test_clock(control.clock().expect("manual pacing"));
    shell
        .run("audio.configure", json!({ "input_device": "null:null" }))
        .unwrap();
    shell.start_audio();
    shell
        .run(
            "session.set_quantization",
            json!({ "quantization": "none" }),
        )
        .unwrap();
    shell
        .run("midi.toggle_computer_keyboard", json!({ "on": true }))
        .unwrap();
    (shell, control)
}

/// A new armed MIDI track named `name`.
fn midi_track(shell: &mut Shell, name: &str) -> TrackId {
    shell
        .invoke("track.create", json!({ "kind": "midi", "name": name }))
        .unwrap();
    let track = shell.editor().document().top_level().last().unwrap();
    shell
        .run("mixer.set_arm", json!({ "track": track, "arm": true }))
        .unwrap();
    track
}

fn first_scene(shell: &Shell) -> SceneId {
    shell.editor().document().scenes().next().unwrap().id
}

/// Advances the set until `done` holds, up to about ten seconds of test
/// time. The null device is paced by hand (T199), so this lands on the same
/// frames however busy the machine is.
fn poll_until(
    control: &NullControl,
    shell: &mut Shell,
    what: &str,
    done: impl Fn(&mut Shell) -> bool,
) {
    testing::wait_for(control, shell, what, done);
}

/// Advances the set by `millis` of test time.
fn pass_time(control: &NullControl, shell: &mut Shell, millis: u64) {
    testing::advance(control, shell, millis);
}

/// Plays the keyboard's A key for `millis`.
fn play_a(control: &NullControl, shell: &mut Shell, millis: u64) {
    for pressed in [true, false] {
        shell
            .run(
                "midi.computer_keyboard_key",
                json!({ "key": "A", "pressed": pressed }),
            )
            .unwrap();
        if pressed {
            pass_time(control, shell, millis);
        }
    }
}

fn slot_clip(shell: &Shell, track: TrackId, scene: SceneId) -> Option<Clip> {
    shell.editor().document().slot(track, scene).cloned()
}

fn midi_notes(clip: &Clip) -> Vec<Note> {
    let ClipData::Midi(data) = &clip.data else {
        panic!("a MIDI clip");
    };
    let mut notes: Vec<Note> = data.notes.iter().cloned().collect();
    notes.sort_by_key(|note| note.start);
    notes
}

fn recorded(shell: &mut Shell) -> bool {
    shell.running_recording().is_none() && !shell.editor().in_transaction()
}

// Spec: session-recording R1, R2, R7, R9. Session Record records the armed
// MIDI track into the selected scene's slot; the clip is in the document
// while the take runs, loops, and is one undo step.
#[test]
fn session_record_records_a_midi_clip_into_the_slot_in_one_undo_step() {
    let (mut shell, control) = rig();
    let keys = midi_track(&mut shell, "Keys");
    let scene = first_scene(&shell);
    let before = shell.editor().undo_history().len();

    let started = shell.run("session.record", json!({})).unwrap();
    assert_eq!(
        started["slots"],
        json!([{ "track": keys, "scene": scene, "overdub": false }])
    );
    poll_until(&control, &mut shell, "the clip to appear", |shell| {
        slot_clip(shell, keys, scene).is_some()
    });
    assert!(
        shell.editor().in_transaction(),
        "the clip grows inside the recording's undo step"
    );
    let state = shell.run("transport.state", json!({})).unwrap();
    assert_eq!(
        state["recording"]["session_slots"][0]["track"],
        json!(keys),
        "transport.state reads the recording back"
    );
    assert!(!state["arrangement_record"].as_bool().unwrap());
    play_a(&control, &mut shell, 150);
    let short = slot_clip(&shell, keys, scene).unwrap().length;
    pass_time(&control, &mut shell, 150);
    let longer = slot_clip(&shell, keys, scene).unwrap().length;
    assert!(longer > short, "the clip grows as it records");
    shell.run("session.stop_recording", json!({})).unwrap();
    poll_until(&control, &mut shell, "the recording to end", recorded);

    let clip = slot_clip(&shell, keys, scene).expect("the recorded clip");
    let notes = midi_notes(&clip);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].length > Ticks::ZERO);
    let ClipData::Midi(MidiData { looping, .. }) = clip.data else {
        panic!("a MIDI clip");
    };
    assert!(looping, "a recorded clip loops");
    assert_eq!(clip.start, Ticks::ZERO);
    assert_eq!(
        shell.editor().undo_history().len(),
        before + 1,
        "one undo step"
    );
    // The slot launched where the take ended, and plays on as a loop. The
    // launch is applied in the block after the take ends, so the poll that
    // ends the recording may still see it queued.
    poll_until(&control, &mut shell, "the recorded slot to play", |shell| {
        shell
            .session_states()
            .iter()
            .any(|s| s.track == keys && s.scene == scene && s.state == SlotState::Playing)
    });
    shell.run("edit.undo", json!({})).unwrap();
    assert!(slot_clip(&shell, keys, scene).is_none());
    shell.run("edit.redo", json!({})).unwrap();
    assert!(slot_clip(&shell, keys, scene).is_some());

    let stats = shell.audio().stats().expect("a stream is open");
    assert!(stats.blocks > 0);
    assert_eq!(stats.heap_operations, 0, "{stats:?}");
}

// Spec: scales R7. A Session recording makes a clip from nothing, so it
// takes the set's current scale.
#[test]
fn a_session_recording_takes_the_current_scale() {
    let (mut shell, control) = rig();
    let keys = midi_track(&mut shell, "Keys");
    let scene = first_scene(&shell);
    shell
        .run(
            "song.set_scale",
            json!({ "enabled": true, "root": 3, "name": "dorian" }),
        )
        .unwrap();
    shell.run("session.record", json!({})).unwrap();
    poll_until(&control, &mut shell, "the clip to appear", |shell| {
        slot_clip(shell, keys, scene).is_some()
    });
    play_a(&control, &mut shell, 150);
    shell.run("session.stop_recording", json!({})).unwrap();
    poll_until(&control, &mut shell, "the recording to end", recorded);
    let clip = slot_clip(&shell, keys, scene).expect("the recorded clip");
    assert_eq!(
        clip.scale,
        bl_model::Scale::new(3, bl_model::ScaleName::Dorian),
        "the Session recording takes the current scale"
    );
}

// Spec: session-recording R1. A track whose selected slot is taken records
// into the next empty slot below it; with none, a scene is inserted at the
// end, in the same undo step.
#[test]
fn a_taken_slot_sends_the_recording_to_the_next_empty_one_or_a_new_scene() {
    let (mut shell, control) = rig();
    let keys = midi_track(&mut shell, "Keys");
    let scenes: Vec<SceneId> = shell
        .editor()
        .document()
        .scenes()
        .map(|scene| scene.id)
        .collect();
    // Fill every slot of the track but the third.
    for (index, scene) in scenes.iter().enumerate() {
        if index != 2 {
            shell
                .run(
                    "session.insert_empty_midi_clip",
                    json!({ "track": keys, "scene": scene }),
                )
                .unwrap();
        }
    }
    let first = shell.run("session.record", json!({})).unwrap();
    assert_eq!(first["slots"][0]["scene"], json!(scenes[2]));
    pass_time(&control, &mut shell, 100);
    shell.run("session.stop_recording", json!({})).unwrap();
    poll_until(&control, &mut shell, "the recording to end", recorded);
    shell.run("session.stop_all", json!({})).unwrap();
    poll_until(&control, &mut shell, "the clips to stop", |shell| {
        shell.session_states().is_empty()
    });

    // Now every slot is taken: a scene is inserted.
    let count = shell.editor().document().scenes().len();
    let steps = shell.editor().undo_history().len();
    let second = shell.run("session.record", json!({})).unwrap();
    let scene = shell.editor().document().scenes().last().unwrap().id;
    assert_eq!(shell.editor().document().scenes().len(), count + 1);
    assert_eq!(second["slots"][0]["scene"], json!(scene));
    pass_time(&control, &mut shell, 100);
    shell.run("session.stop_recording", json!({})).unwrap();
    poll_until(&control, &mut shell, "the recording to end", recorded);
    assert!(slot_clip(&shell, keys, scene).is_some());
    assert_eq!(
        shell.editor().undo_history().len(),
        steps + 1,
        "the scene and the clip are one step"
    );
}

// Spec: session-recording R2. A slot's record button works on an empty slot
// of an armed track, and refuses an occupied slot or an unarmed track.
#[test]
fn a_slots_record_button_needs_an_armed_track_and_an_empty_slot() {
    let (mut shell, _) = rig();
    let keys = midi_track(&mut shell, "Keys");
    let scene = first_scene(&shell);
    shell
        .run("mixer.set_arm", json!({ "track": keys, "arm": false }))
        .unwrap();
    let args = json!({ "track": keys, "scene": scene });
    assert!(shell.run("session.record_slot", args.clone()).is_err());
    shell
        .run("mixer.set_arm", json!({ "track": keys, "arm": true }))
        .unwrap();
    shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": keys, "scene": scene }),
        )
        .unwrap();
    assert!(
        shell.run("session.record_slot", args.clone()).is_err(),
        "an occupied slot's button launches"
    );
    assert!(shell.session_states().is_empty());
}

// Spec: session-recording R8. An overdub adds the notes to the clip that
// plays, a loop pass at a time, and each pass is one undo step.
#[test]
fn an_overdub_adds_each_loop_pass_as_an_undo_step() {
    let (mut shell, control) = rig();
    let keys = midi_track(&mut shell, "Keys");
    let scene = first_scene(&shell);
    // A fast tempo keeps the loop short: 240 BPM is a quarter per 250 ms.
    let map = bl_core::time::TempoMap::constant(bl_core::time::Bpm::new(240.0).unwrap());
    shell
        .run("song.set_tempo", json!({ "tempo": map }))
        .unwrap();

    // Record a half-second clip with one note; it loops from where the take
    // ends.
    shell.run("session.record", json!({})).unwrap();
    pass_time(&control, &mut shell, 100);
    play_a(&control, &mut shell, 50);
    pass_time(&control, &mut shell, 250);
    shell.run("session.stop_recording", json!({})).unwrap();
    poll_until(&control, &mut shell, "the recording to end", recorded);
    let clip = slot_clip(&shell, keys, scene).unwrap();
    assert_eq!(midi_notes(&clip).len(), 1);
    // The loop's length in milliseconds at 240 BPM.
    let loop_ms = clip.length.get() as f64 / Ticks::QUARTER.get() as f64 * 250.0;
    poll_until(&control, &mut shell, "the clip to play", |shell| {
        shell
            .session_states()
            .iter()
            .any(|s| s.track == keys && s.state == SlotState::Playing)
    });

    let steps = shell.editor().undo_history().len();
    let started = shell.run("session.record", json!({})).unwrap();
    assert_eq!(started["slots"][0]["overdub"], json!(true));
    // The take begins on the engine's next block.
    pass_time(&control, &mut shell, 60);
    // Three notes 1.3 loops apart land in three different passes.
    for _ in 0..3 {
        play_a(&control, &mut shell, 30);
        pass_time(&control, &mut shell, (loop_ms * 1.3) as u64);
    }
    shell.run("session.stop_recording", json!({})).unwrap();
    poll_until(&control, &mut shell, "the recording to end", recorded);

    let added = shell.editor().undo_history().len() - steps;
    let notes = midi_notes(&slot_clip(&shell, keys, scene).unwrap());
    assert_eq!(added, 3, "a step for each pass");
    assert_eq!(notes.len(), 1 + 3, "{notes:?}");
    // Each Undo takes one pass away.
    for left in [3, 2, 1] {
        shell.run("edit.undo", json!({})).unwrap();
        assert_eq!(
            midi_notes(&slot_clip(&shell, keys, scene).unwrap()).len(),
            left
        );
    }
}

// Spec: launch-quantisation R33. An overdub into a clip that was nudged
// records at the clip's own time. Two armed tracks record the same take, so
// their clips loop alike; one is nudged a quarter note forward, and the one
// note played into both lands as much later in the nudged clip's loop as its
// clip time is ahead of the other's.
#[test]
fn an_overdub_into_a_nudged_clip_records_at_its_clip_time() {
    let (mut shell, control) = rig();
    let plain = midi_track(&mut shell, "Plain");
    let nudged = midi_track(&mut shell, "Nudged");
    let scene = first_scene(&shell);
    let map = bl_core::time::TempoMap::constant(bl_core::time::Bpm::new(240.0).unwrap());
    shell
        .run("song.set_tempo", json!({ "tempo": map }))
        .unwrap();
    // Both record the same half-second take, with one note.
    shell.run("session.record", json!({})).unwrap();
    pass_time(&control, &mut shell, 100);
    play_a(&control, &mut shell, 50);
    pass_time(&control, &mut shell, 250);
    shell.run("session.stop_recording", json!({})).unwrap();
    poll_until(&control, &mut shell, "the recording to end", recorded);
    let length = slot_clip(&shell, plain, scene).unwrap().length;
    assert_eq!(slot_clip(&shell, nudged, scene).unwrap().length, length);
    let first = |track| midi_notes(&slot_clip(&shell, track, scene).unwrap())[0].start;
    let (first_plain, first_nudged) = (first(plain), first(nudged));
    poll_until(&control, &mut shell, "the clips to play", |shell| {
        shell
            .session_states()
            .iter()
            .filter(|s| s.state == SlotState::Playing)
            .count()
            == 2
    });
    // Eight thirty-seconds, which the global value None gives, are a
    // quarter note.
    shell
        .run(
            "session.nudge",
            json!({ "track": nudged, "scene": scene, "steps": 8 }),
        )
        .unwrap();
    pass_time(&control, &mut shell, 10);
    // How far ahead the nudged clip's own time is of the other's, at any
    // one moment: the nudge, less the later launch of one of them.
    let tempo = shell.editor().document().settings().tempo.clone();
    let rate = bl_core::time::SampleRate::new(shell.audio().sample_rate().unwrap()).unwrap();
    let ahead = {
        let states = shell.session_states();
        let of = |track| states.iter().find(|s| s.track == track && s.scene == scene).unwrap();
        let (a, b) = (of(plain), of(nudged));
        assert_eq!(b.offset.ticks, Ticks::QUARTER, "the nudge moved a quarter note");
        assert_eq!(a.offset.ticks, Ticks::ZERO);
        b.offset.ticks - a.offset.ticks
            - (tempo.frames_to_ticks(b.started.unwrap(), rate)
                - tempo.frames_to_ticks(a.started.unwrap(), rate))
    };
    shell.run("session.record", json!({})).unwrap();
    pass_time(&control, &mut shell, 60);
    play_a(&control, &mut shell, 30);
    pass_time(&control, &mut shell, 100);
    shell.run("session.stop_recording", json!({})).unwrap();
    poll_until(&control, &mut shell, "the recording to end", recorded);
    // The note added is the one that isn't the take's own.
    let added = |track, own| {
        let notes = midi_notes(&slot_clip(&shell, track, scene).unwrap());
        assert_eq!(notes.len(), 2, "{notes:?}");
        notes.iter().find(|note| note.start != own).unwrap().start
    };
    let moved = (added(nudged, first_nudged) - added(plain, first_plain)).rem_euclid(length);
    let ahead = ahead.rem_euclid(length);
    assert!(
        (moved - ahead).get().abs() < Ticks::SIXTEENTH.get() / 8,
        "the note moved {moved:?}, not {ahead:?}, in a loop of {length:?}"
    );
}

// Spec: session-recording R6. An armed audio track records its input to a
// WAV and the file becomes a looping clip in the slot.
#[test]
fn an_armed_audio_track_records_a_looping_audio_clip_into_the_slot() {
    let dir = TestDir::new("audio");
    let (mut shell, control) = rig();
    let saved: Value = shell
        .run("set.save_as", json!({ "path": dir.arg("Take.blset") }))
        .unwrap();
    let folder = PathBuf::from(saved["path"].as_str().unwrap())
        .parent()
        .unwrap()
        .to_path_buf();
    shell
        .invoke("track.create", json!({ "kind": "audio", "name": "Guitar" }))
        .unwrap();
    let track = shell.editor().document().top_level().last().unwrap();
    shell
        .run(
            "routing.set_input",
            json!({
                "track": track,
                "input": { "type": "external", "channel": { "type": "mono", "channel": 3 } },
            }),
        )
        .unwrap();
    shell
        .run("mixer.set_arm", json!({ "track": track, "arm": true }))
        .unwrap();
    let rate = shell.audio().status().sample_rate.expect("a device runs");
    control.set_input_tone(2, f64::from(rate) / 4.0, 0.5);
    let scene = first_scene(&shell);

    shell
        .run(
            "session.record_slot",
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    pass_time(&control, &mut shell, 300);
    shell.run("session.stop_recording", json!({})).unwrap();
    poll_until(&control, &mut shell, "the recording to end", recorded);

    let clip = slot_clip(&shell, track, scene).expect("an audio clip in the slot");
    let ClipData::Audio(audio) = &clip.data else {
        panic!("an audio clip");
    };
    assert!(audio.looping);
    assert_eq!(clip.start, Ticks::ZERO);
    assert!(clip.length > Ticks::ZERO);
    let files = std::fs::read_dir(folder.join("Samples").join("Recorded"))
        .unwrap()
        .count();
    assert_eq!(files, 1, "one file for the armed track");
    // The file is the only audio overdub-free recording: the recorded clip
    // is never overdubbed.
    shell
        .run("session.overdub", json!({ "track": track, "scene": scene }))
        .unwrap_err();
}

// Spec: session-recording R10. Start Recording on Scene Launch is off in a
// new install, and, on, launching a scene records into the empty slots of
// armed tracks.
#[test]
fn launching_a_scene_records_only_when_start_recording_on_scene_launch_is_on() {
    let (mut shell, control) = rig();
    let keys = midi_track(&mut shell, "Keys");
    let scene = first_scene(&shell);
    let state = shell.run("settings.state", json!({})).unwrap();
    assert_eq!(
        state["launch"]["start_recording_on_scene_launch"],
        json!(false)
    );

    shell
        .run("session.launch_scene", json!({ "scene": scene }))
        .unwrap();
    pass_time(&control, &mut shell, 100);
    assert!(shell.running_recording().is_none(), "off: nothing records");

    shell
        .run(
            "settings.set_start_recording_on_scene_launch",
            json!({ "on": true }),
        )
        .unwrap();
    shell
        .run("session.launch_scene", json!({ "scene": scene }))
        .unwrap();
    let running = shell.running_recording().expect("on: the scene records");
    assert_eq!(running.session_slots.len(), 1);
    assert_eq!(running.session_slots[0].track, keys);
    pass_time(&control, &mut shell, 100);
    shell.run("session.stop_recording", json!({})).unwrap();
    poll_until(&control, &mut shell, "the recording to end", recorded);
    assert!(slot_clip(&shell, keys, scene).is_some());
}

// Spec: session-recording R4. Session Record again ends the recording.
#[test]
fn session_record_again_ends_the_recording() {
    let (mut shell, control) = rig();
    let keys = midi_track(&mut shell, "Keys");
    let scene = first_scene(&shell);
    shell.run("session.record", json!({})).unwrap();
    pass_time(&control, &mut shell, 100);
    let ended = shell.run("session.record", json!({})).unwrap();
    assert_eq!(ended["slots"], json!([]));
    poll_until(&control, &mut shell, "the recording to end", recorded);
    assert!(
        slot_clip(&shell, keys, scene).is_some(),
        "an empty recording is kept as a clip"
    );
}

// Spec: session-recording R11. Arrangement Record is refused while a Session
// recording runs, and the Session recording goes on.
#[test]
fn arrangement_record_waits_for_a_session_recording_to_end() {
    let (mut shell, control) = rig();
    let keys = midi_track(&mut shell, "Keys");
    let scene = first_scene(&shell);
    shell.run("session.record", json!({})).unwrap();
    pass_time(&control, &mut shell, 100);
    assert!(
        shell
            .run("transport.set_recording", json!({ "recording": true }))
            .is_err()
    );
    assert!(shell.running_recording().is_some(), "still recording");
    shell.run("session.stop_recording", json!({})).unwrap();
    poll_until(&control, &mut shell, "the recording to end", recorded);
    assert!(slot_clip(&shell, keys, scene).is_some());
}

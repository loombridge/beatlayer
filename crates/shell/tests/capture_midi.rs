//! WP-3.14's acceptance with the test backend: Capture MIDI brings back the
//! notes played on a monitoring MIDI track as a looping clip, in the slot
//! the capture MIDI spec's rules choose, as one undo step; with nothing
//! played it refuses and is disabled; a playing clip takes the notes of a
//! later capture; and the command runs by ID.
//!
//! - A capture with the transport stopped makes a clip in the selected
//!   scene's slot, or the next empty one below, or a scene at the end.
//! - The capture is one undo step and empties the buffer.
//! - Nothing played refuses the command and disables it.
//! - The clip plays, and a later capture adds to it.
//! - The whole path allocates nothing on the audio thread.

bl_rt::install_trap!();

use bl_audio_io::{BackendKind, NullBackend, NullControl, NullOptions, Pacing};
use bl_core::time::{Bpm, TempoMap, Ticks};
use bl_engine::SlotState;
use bl_model::{Clip, ClipData, MidiData, Note, SceneId, TrackId};
use bl_shell::Shell;
use bl_shell::audio::{AudioHost, AudioSettings};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::testing;
use serde_json::{Value, json};

/// A shell on a null device with the computer MIDI keyboard on and launches
/// that take effect at once.
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

/// A new armed MIDI track, which monitors its input (Auto).
fn midi_track(shell: &mut Shell, control: &NullControl, name: &str) -> TrackId {
    shell
        .invoke("track.create", json!({ "kind": "midi", "name": name }))
        .unwrap();
    let track = shell.editor().document().top_level().last().unwrap();
    shell
        .run("mixer.set_arm", json!({ "track": track, "arm": true }))
        .unwrap();
    testing::settle(control, shell);
    track
}

fn scenes(shell: &Shell) -> Vec<SceneId> {
    shell
        .editor()
        .document()
        .scenes()
        .map(|scene| scene.id)
        .collect()
}

fn pass_time(control: &NullControl, shell: &mut Shell, millis: u64) {
    testing::advance(control, shell, millis);
}

/// Plays the keyboard's A key `count` times, a tenth of a second long each,
/// a quarter of a second apart.
fn play_a(control: &NullControl, shell: &mut Shell, count: usize) {
    shell.poll_engine();
    let before = shell.capture_midi_released();
    for _ in 0..count {
        for pressed in [true, false] {
            shell
                .run(
                    "midi.computer_keyboard_key",
                    json!({ "key": "A", "pressed": pressed }),
                )
                .unwrap();
            pass_time(control, shell, if pressed { 100 } else { 150 });
        }
    }
    // The engine logs the keys on its own thread: wait until it has every
    // release.
    testing::wait_for(control, shell, "the engine to log every note", |shell| {
        shell.capture_midi_released() >= before + count
    });
}

fn slot_clip(shell: &Shell, track: TrackId, scene: SceneId) -> Option<Clip> {
    shell.editor().document().slot(track, scene).cloned()
}

fn midi_data(clip: &Clip) -> &MidiData {
    let ClipData::Midi(data) = &clip.data else {
        panic!("a MIDI clip");
    };
    data
}

fn notes(clip: &Clip) -> Vec<Note> {
    let mut notes: Vec<Note> = midi_data(clip).notes.iter().cloned().collect();
    notes.sort_by_key(|note| note.start);
    notes
}

fn ready(shell: &mut Shell) -> bool {
    shell.poll_engine();
    shell.run("transport.state", json!({})).unwrap()["capture_midi_ready"]
        .as_bool()
        .unwrap()
}

// Spec: capture-midi R1, R3, R4, R9. Notes played with the transport
// stopped come back as a looping clip in the selected scene's slot, from
// 1.1.1, as one undo step; the capture empties the buffer, so the command is
// refused and disabled until a note is played again.
#[test]
fn a_capture_makes_a_clip_in_the_selected_slot_in_one_undo_step() {
    let (mut shell, control) = rig();
    let keys = midi_track(&mut shell, &control, "Keys");
    let scene = scenes(&shell)[0];
    assert!(!ready(&mut shell), "nothing played");
    assert!(!shell.can_invoke("midi.capture", &json!({})));
    assert!(shell.run("midi.capture", json!({})).is_err());

    play_a(&control, &mut shell, 4);
    assert!(ready(&mut shell));
    assert!(shell.can_invoke("midi.capture", &json!({})));
    let before = shell.editor().undo_history().len();
    let result = shell.run("midi.capture", json!({})).unwrap();
    assert_eq!(
        result["clips"],
        json!([{ "track": keys, "scene": scene, "clip": result["clips"][0]["clip"], "merged": false }])
    );

    let clip = slot_clip(&shell, keys, scene).expect("the captured clip");
    let notes = notes(&clip);
    assert_eq!(notes.len(), 4, "{notes:?}");
    assert_eq!(notes[0].start, Ticks::ZERO, "the first note is on 1.1.1");
    assert!(
        notes
            .iter()
            .all(|n| n.key == notes[0].key && n.length > Ticks::ZERO)
    );
    let data = midi_data(&clip);
    assert!(data.looping);
    assert_eq!(data.loop_start, Ticks::ZERO);
    let bar = Ticks::WHOLE.get();
    let end = data.loop_end.expect("a loop end").get();
    assert!(end > 0 && end % bar == 0, "whole bars: {end}");
    assert_eq!(clip.length.get(), end, "the clip is as long as its loop");
    assert_eq!(
        shell.editor().undo_history().len(),
        before + 1,
        "one undo step"
    );
    assert!(!ready(&mut shell), "the capture emptied the buffer");
    assert!(shell.run("midi.capture", json!({})).is_err());

    shell.run("edit.undo", json!({})).unwrap();
    assert!(slot_clip(&shell, keys, scene).is_none());
    shell.run("edit.redo", json!({})).unwrap();
    assert!(slot_clip(&shell, keys, scene).is_some());
    let stats = shell.audio().stats().expect("a stream is open");
    assert!(stats.blocks > 0);
    assert_eq!(stats.heap_operations, 0, "{stats:?}");
}

// Spec: scales R7. A captured clip is made from nothing, so it takes the
// set's current scale.
#[test]
fn a_captured_clip_takes_the_current_scale() {
    let (mut shell, control) = rig();
    let keys = midi_track(&mut shell, &control, "Keys");
    let scene = scenes(&shell)[0];
    shell
        .run(
            "song.set_scale",
            json!({ "enabled": true, "root": 5, "name": "minor" }),
        )
        .unwrap();
    play_a(&control, &mut shell, 2);
    assert!(ready(&mut shell));
    shell.run("midi.capture", json!({})).unwrap();
    let clip = slot_clip(&shell, keys, scene).expect("the captured clip");
    assert_eq!(
        clip.scale,
        bl_model::Scale::new(5, bl_model::ScaleName::Minor),
        "the captured clip takes the current scale"
    );
}

// Spec: grooves R20. A clip made by recording takes the set's auto-load
// groove while Auto Load Groove is on.
#[test]
fn a_captured_clip_takes_the_auto_load_groove() {
    let (mut shell, control) = rig();
    let keys = midi_track(&mut shell, &control, "Keys");
    let scene = scenes(&shell)[0];
    let groove: bl_model::GrooveId = shell.new_id();
    shell
        .run(
            "groove.add",
            json!({ "groove": { "id": groove, "name": "Swing", "length": Ticks::WHOLE.get() * 4,
                "notes": [{ "position": 0, "velocity": 100 }] } }),
        )
        .unwrap();
    shell
        .run(
            "groove.set_auto_load",
            json!({ "enabled": true, "groove": groove }),
        )
        .unwrap();
    play_a(&control, &mut shell, 2);
    shell.run("midi.capture", json!({})).unwrap();
    let clip = slot_clip(&shell, keys, scene).expect("the captured clip");
    assert_eq!(clip.groove, Some(groove));
}

// Spec: capture-midi R4. A taken slot sends the clip to the next empty one
// below it; with none left, a scene is added at the end.
#[test]
fn a_taken_slot_sends_the_clip_to_the_next_empty_one_or_a_new_scene() {
    let (mut shell, control) = rig();
    let keys = midi_track(&mut shell, &control, "Keys");
    let all = scenes(&shell);
    // Fill every slot of the track but the third.
    for (index, scene) in all.iter().enumerate() {
        if index != 2 {
            shell
                .run(
                    "session.insert_empty_midi_clip",
                    json!({ "track": keys, "scene": scene }),
                )
                .unwrap();
        }
    }
    play_a(&control, &mut shell, 2);
    let first = shell.run("midi.capture", json!({})).unwrap();
    assert_eq!(first["clips"][0]["scene"], json!(all[2]));

    // Stop the clip so the track has none playing, then play again.
    shell.run("session.stop_all", json!({})).unwrap();
    pass_time(&control, &mut shell, 100);
    play_a(&control, &mut shell, 2);
    let second = shell.run("midi.capture", json!({})).unwrap();
    let now = scenes(&shell);
    assert_eq!(now.len(), all.len() + 1, "a scene was added at the end");
    assert_eq!(second["clips"][0]["scene"], json!(now[all.len()]));
}

// Spec: capture-midi R4, R5. A clip that plays takes the notes of the next
// capture, and its length grows by whole loop passes.
#[test]
fn a_playing_clip_takes_the_notes_of_a_later_capture() {
    let (mut shell, control) = rig();
    let keys = midi_track(&mut shell, &control, "Keys");
    let scene = scenes(&shell)[0];
    play_a(&control, &mut shell, 2);
    let first = shell.run("midi.capture", json!({})).unwrap();
    let id = first["clips"][0]["clip"].clone();
    let clip = slot_clip(&shell, keys, scene).unwrap();
    let (before, length) = (notes(&clip).len(), clip.length);
    // The capture launched the clip; wait for it to play.
    testing::wait_for(&control, &mut shell, "the clip to play", |shell| {
        shell
            .session_states()
            .iter()
            .any(|s| s.track == keys && s.scene == scene && s.state == SlotState::Playing)
    });

    play_a(&control, &mut shell, 3);
    let second = shell.run("midi.capture", json!({})).unwrap();
    assert_eq!(second["clips"][0]["merged"], json!(true));
    assert_eq!(second["clips"][0]["clip"], id, "the same clip");
    assert_eq!(
        second["tempo"],
        Value::Null,
        "a running transport keeps the tempo"
    );
    let clip = slot_clip(&shell, keys, scene).unwrap();
    assert!(notes(&clip).len() >= before + 3, "the new notes joined it");
    assert!(clip.length >= length);
    assert_eq!(clip.length.get() % Ticks::WHOLE.get(), 0);
}

// Spec: capture-midi R6, R9. Notes played at a tempo near but not within the
// fit of the song's set it, to 0.5 BPM, in the same undo step as the clip;
// undo puts the old tempo back.
#[test]
fn a_stopped_capture_sets_the_tempo_it_was_played_at_in_the_same_undo_step() {
    let (mut shell, control) = rig();
    let keys = midi_track(&mut shell, &control, "Keys");
    let all = scenes(&shell);
    let set_song = |shell: &mut Shell, bpm: f64| {
        let song = TempoMap::constant(Bpm::new(bpm).unwrap());
        shell
            .run("song.set_tempo", json!({ "tempo": song }))
            .unwrap();
    };
    // First measure how fast the keys come on this machine: at 100 BPM the
    // played tempo is out of range, so the song keeps its tempo and the
    // notes their raw times.
    set_song(&mut shell, 100.0);
    play_a(&control, &mut shell, 4);
    let first = shell.run("midi.capture", json!({})).unwrap();
    assert_eq!(first["tempo"], Value::Null);
    let clip = slot_clip(&shell, keys, all[0]).unwrap();
    let starts: Vec<i64> = notes(&clip).iter().map(|n| n.start.get()).collect();
    let gap = (starts[3] - starts[0]) as f64 / 3.0 / Ticks::QUARTER.get() as f64 * 60.0 / 100.0;
    let played = 60.0 / gap;
    // The capture launched the clip and started the transport.
    shell.run("session.stop_all", json!({})).unwrap();
    shell.run("transport.stop", json!({})).unwrap();
    pass_time(&control, &mut shell, 100);

    // A song a fifth slower than the keys: in range, outside the fit.
    set_song(&mut shell, played / 1.2);
    let before = shell.editor().document().settings().tempo.clone();
    let steps = shell.editor().undo_history().len();
    play_a(&control, &mut shell, 4);
    let result = shell.run("midi.capture", json!({})).unwrap();
    let tempo = result["tempo"].as_f64().expect("the tempo changed");
    assert!(
        (tempo / played - 1.0).abs() < 0.1,
        "{tempo} against {played}"
    );
    assert_eq!(tempo * 2.0, (tempo * 2.0).round(), "to 0.5 BPM");
    let now = shell
        .editor()
        .document()
        .settings()
        .tempo
        .bpm_at(Ticks::ZERO)
        .get();
    assert_eq!(now, tempo);
    assert_eq!(
        shell.editor().undo_history().len(),
        steps + 1,
        "one undo step"
    );
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(shell.editor().document().settings().tempo, before);
}

//! WP-7.32's acceptance with the test backend: Edit ▸ Record Quantization
//! moves the notes an Arrangement take, a Session slot take and a Session
//! overdub make onto its grid, in the undo steps the recording spec's R17 to
//! R23 say, and nothing else is quantised. The rules are in the recording
//! spec.
//!
//! - An Arrangement take is quantised in a step of its own after the take's.
//! - With MIDI Arrangement Overdub on, the notes already in a clip stay.
//! - A take round the loop is quantised in one step after its passes.
//! - A Session slot take is quantised inside its own step.
//! - An overdub of a looping clip takes the value in force as each note
//!   arrives, and wraps the loop's end to its start.
//! - Arrangement and slot recordings hold the value they started with.

use bl_audio_io::{BackendKind, NullBackend, NullControl, NullOptions, Pacing};
use bl_core::time::{Bpm, Frames, SampleRate, TempoMap, Ticks};
use bl_engine::SlotState;
use bl_model::{Clip, ClipData, Note, SceneId, TakeLane, TrackId};
use bl_shell::Shell;
use bl_shell::audio::{AudioHost, AudioSettings};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::testing;
use serde_json::json;

const SIXTEENTH: i64 = Ticks::QUARTER.get() / 4;

/// A shell on a null device paced by hand, with the computer MIDI keyboard on
/// and Session launches that take effect at once.
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

/// A new armed MIDI track.
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

fn set_value(shell: &mut Shell, value: &str) {
    shell
        .run(
            "transport.set_record_quantization",
            json!({ "quantization": value }),
        )
        .unwrap();
}

fn pass_time(control: &NullControl, shell: &mut Shell, millis: u64) {
    testing::advance(control, shell, millis);
}

/// Presses the keyboard's A key after `wait` milliseconds, for `millis`.
fn tap_a(control: &NullControl, shell: &mut Shell, wait: u64, millis: u64) {
    pass_time(control, shell, wait);
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

fn flush(shell: &Shell) {
    assert!(
        shell.audio().flush(std::time::Duration::from_secs(10)),
        "the engine never took the command"
    );
}

/// Records from the start of the song with `transport.set_recording`, runs
/// `play` while it records, then stops.
fn record_arrangement(shell: &mut Shell, play: impl FnOnce(&mut Shell)) {
    shell
        .run("transport.locate", json!({ "position": Ticks::ZERO }))
        .unwrap();
    shell
        .run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    flush(shell);
    play(shell);
    shell.run("transport.stop", json!({})).unwrap();
    flush(shell);
}

fn lanes_with_clips(
    control: &NullControl,
    shell: &mut Shell,
    track: TrackId,
    count: usize,
) -> Vec<TakeLane> {
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

fn main_lane(shell: &Shell, track: TrackId) -> Vec<Clip> {
    let mut clips: Vec<Clip> = shell
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

fn starts(clip: &Clip) -> Vec<i64> {
    let ClipData::Midi(data) = &clip.data else {
        panic!("a MIDI clip: {clip:?}");
    };
    let mut starts: Vec<i64> = data.notes.iter().map(|note| note.start.get()).collect();
    starts.sort_unstable();
    starts
}

fn history(shell: &Shell) -> Vec<String> {
    shell
        .editor()
        .undo_history()
        .map(str::to_owned)
        .collect::<Vec<_>>()
}

fn undo(shell: &mut Shell) {
    shell.run("edit.undo", json!({})).unwrap();
}

// Spec: recording R17, R19. An Arrangement take played off the grid with
// 1/16 on lands on sixteenths in a step of its own: one Undo puts the notes
// back where they were played with the clip still there, a second removes
// the clip.
#[test]
fn an_arrangement_take_is_quantised_in_a_step_after_its_own() {
    let (mut shell, control) = rig();
    let track = midi_track(&mut shell, "Keys");
    set_value(&mut shell, "1/16");

    record_arrangement(&mut shell, |shell| {
        // 120 BPM: a sixteenth is 125 ms, so a note at 100 ms is a fifth of
        // a quarter, off the grid.
        tap_a(&control, shell, 100, 100);
        pass_time(&control, shell, 100);
    });
    let lanes = lanes_with_clips(&control, &mut shell, track, 1);
    let main = main_lane(&shell, track);
    assert_eq!(starts(&main[0]), [SIXTEENTH], "{main:?}");
    assert_eq!(
        starts(&lanes[0].clips[0]),
        [SIXTEENTH],
        "the take lane's copy is quantised too"
    );
    assert_eq!(
        &history(&shell)[..2],
        ["Record Quantization", "Recorded Clip"]
    );

    // One Undo takes back the quantising alone.
    undo(&mut shell);
    let main = main_lane(&shell, track);
    assert_eq!(main.len(), 1, "the clip is still there");
    let played = starts(&main[0]);
    assert_eq!(played.len(), 1);
    assert_ne!(played[0] % SIXTEENTH, 0, "{played:?}: where it was played");
    assert!(
        (played[0] - SIXTEENTH).abs() < SIXTEENTH / 2,
        "{played:?}: near the sixteenth"
    );
    assert_eq!(history(&shell)[0], "Recorded Clip");
    undo(&mut shell);
    assert!(main_lane(&shell, track).is_empty());
}

// Spec: recording R17. No Quantization leaves the take as played, and adds
// no step.
#[test]
fn a_take_without_record_quantization_makes_no_extra_step() {
    let (mut shell, control) = rig();
    let track = midi_track(&mut shell, "Keys");
    record_arrangement(&mut shell, |shell| {
        tap_a(&control, shell, 100, 100);
        pass_time(&control, shell, 100);
    });
    lanes_with_clips(&control, &mut shell, track, 1);
    let main = main_lane(&shell, track);
    assert_ne!(starts(&main[0])[0] % SIXTEENTH, 0);
    assert_eq!(history(&shell)[0], "Recorded Clip");
}

// Spec: recording R19. With MIDI Arrangement Overdub on, the notes already
// in a clip stay where they are; only the take's are quantised.
#[test]
fn an_overdub_take_leaves_the_clips_earlier_notes_alone() {
    let (mut shell, control) = rig();
    let track = midi_track(&mut shell, "Keys");
    record_arrangement(&mut shell, |shell| {
        tap_a(&control, shell, 100, 50);
        pass_time(&control, shell, 200);
    });
    lanes_with_clips(&control, &mut shell, track, 1);
    let first = starts(&main_lane(&shell, track)[0]);

    set_value(&mut shell, "1/16");
    shell
        .run("transport.set_midi_overdub", json!({ "enabled": true }))
        .unwrap();
    record_arrangement(&mut shell, |shell| {
        // 235 ms is 0.47 of a quarter: the line at half a quarter is nearest.
        tap_a(&control, shell, 235, 50);
        pass_time(&control, shell, 100);
    });
    lanes_with_clips(&control, &mut shell, track, 2);
    let main = main_lane(&shell, track);
    assert_eq!(main.len(), 1);
    let merged = starts(&main[0]);
    assert_eq!(merged.len(), 2, "{merged:?}");
    assert_eq!(merged[0], first[0], "the earlier note didn't move");
    assert_eq!(merged[1], Ticks::QUARTER.get() / 2);
}

// Spec: recording R19. A take round the loop is quantised in one step after
// its passes, every pass's notes at once.
#[test]
fn a_take_round_the_loop_is_quantised_in_one_step_after_its_passes() {
    let (mut shell, control) = rig();
    let track = midi_track(&mut shell, "Keys");
    // A thirty-second is 62.5 ms at 120 BPM; the loop is a sixteenth, 125 ms.
    set_value(&mut shell, "1/32");
    shell
        .run(
            "transport.set_loop",
            json!({ "start": Ticks::QUARTER, "length": SIXTEENTH }),
        )
        .unwrap();
    shell
        .run("transport.set_loop_enabled", json!({ "enabled": true }))
        .unwrap();
    shell
        .run(
            "transport.locate",
            json!({ "position": Ticks::QUARTER - Ticks(SIXTEENTH) }),
        )
        .unwrap();
    shell
        .run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    // The lead-in takes 125 ms. Notes at 25, 55 and 15 ms into the three
    // passes, which start at 125, 250 and 375 ms.
    tap_a(&control, &mut shell, 150, 20);
    tap_a(&control, &mut shell, 135, 20);
    tap_a(&control, &mut shell, 65, 20);
    pass_time(&control, &mut shell, 60);
    shell.run("transport.stop", json!({})).unwrap();

    let lanes = lanes_with_clips(&control, &mut shell, track, 3);
    assert_eq!(lanes.len(), 3, "a lane for each pass");
    let thirty_second = SIXTEENTH / 2;
    let passes: Vec<Vec<i64>> = lanes.iter().map(|lane| starts(&lane.clips[0])).collect();
    assert_eq!(passes, [[0], [thirty_second], [0]], "{passes:?}");
    assert_eq!(history(&shell)[0], "Record Quantization");
    assert_eq!(history(&shell)[1], "Record");

    // One Undo takes back the quantising of every pass, and leaves the passes.
    undo(&mut shell);
    let lanes = lanes_with_clips(&control, &mut shell, track, 3);
    assert_eq!(lanes.len(), 3);
    for lane in &lanes {
        assert_ne!(starts(&lane.clips[0])[0] % thirty_second, 0, "{lane:?}");
    }
    assert_eq!(history(&shell)[0], "Record");
}

fn scene_of(shell: &Shell) -> SceneId {
    shell.editor().document().scenes().next().unwrap().id
}

fn slot_clip(shell: &Shell, track: TrackId, scene: SceneId) -> Option<Clip> {
    shell.editor().document().slot(track, scene).cloned()
}

fn recorded(shell: &mut Shell) -> bool {
    shell.running_recording().is_none() && !shell.editor().in_transaction()
}

fn midi_notes(clip: &Clip) -> Vec<Note> {
    let ClipData::Midi(data) = &clip.data else {
        panic!("a MIDI clip");
    };
    let mut notes: Vec<Note> = data.notes.iter().cloned().collect();
    notes.sort_by_key(|note| note.start);
    notes
}

// Spec: recording R20. A Session slot take is quantised to the clip's own
// bars as its notes join, inside its one undo step; a note that would land
// on the clip's end goes to its start.
#[test]
fn a_session_slot_take_is_quantised_inside_its_own_step() {
    let (mut shell, control) = rig();
    let keys = midi_track(&mut shell, "Keys");
    let scene = scene_of(&shell);
    set_value(&mut shell, "1/8");
    let before = shell.editor().undo_history().len();

    shell.run("session.record", json!({})).unwrap();
    pass_time(&control, &mut shell, 240);
    tap_a(&control, &mut shell, 0, 20);
    // A note at 0.8 of a quarter: the nearest line, a quarter, is past the
    // clip's end.
    tap_a(&control, &mut shell, 140, 20);
    pass_time(&control, &mut shell, 30);
    shell.run("session.stop_recording", json!({})).unwrap();
    testing::wait_for(&control, &mut shell, "the recording to end", recorded);

    let clip = slot_clip(&shell, keys, scene).expect("the recorded clip");
    let notes = midi_notes(&clip);
    let eighth = Ticks::QUARTER.get() / 2;
    assert!(clip.length.get() < Ticks::QUARTER.get(), "{clip:?}");
    assert_eq!(
        notes.iter().map(|n| n.start.get()).collect::<Vec<_>>(),
        [0, eighth],
        "{notes:?}"
    );
    assert_eq!(
        shell.editor().undo_history().len(),
        before + 1,
        "no extra undo step"
    );
}

/// The song frame the clip in the slot launched on.
fn launched_on(shell: &mut Shell, track: TrackId, scene: SceneId) -> Frames {
    shell.poll_engine();
    shell
        .session_states()
        .iter()
        .find(|s| s.track == track && s.scene == scene)
        .and_then(|s| s.started)
        .expect("the clip plays")
}

/// How long the clip launched on `started` has played, in ticks.
fn since_launch(shell: &mut Shell, started: Frames, tempo: &TempoMap) -> Ticks {
    shell.poll_engine();
    let rate = SampleRate::new(shell.audio().sample_rate().unwrap()).unwrap();
    let position: Ticks = serde_json::from_value(
        shell.run("transport.state", json!({})).unwrap()["position"].clone(),
    )
    .unwrap();
    position - tempo.frames_to_ticks(started, rate)
}

// Spec: recording R22. An overdub of a looping clip takes the value in
// force as each note arrives: a pass played at 1/4 and the next at 1/16 each
// land on their own grid, a note just before the loop's end goes to its
// start, and each pass stays one undo step.
#[test]
fn an_overdub_quantises_each_note_by_the_value_in_force_when_it_arrives() {
    let (mut shell, control) = rig();
    let keys = midi_track(&mut shell, "Keys");
    let scene = scene_of(&shell);
    let tempo = TempoMap::constant(Bpm::new(240.0).unwrap());
    shell
        .run("song.set_tempo", json!({ "tempo": tempo.clone() }))
        .unwrap();
    // A quarter is 250 ms: record a base clip about 400 ms long.
    shell.run("session.record", json!({})).unwrap();
    pass_time(&control, &mut shell, 100);
    tap_a(&control, &mut shell, 0, 50);
    pass_time(&control, &mut shell, 250);
    shell.run("session.stop_recording", json!({})).unwrap();
    testing::wait_for(&control, &mut shell, "the recording to end", recorded);
    testing::wait_for(&control, &mut shell, "the clip to play", |shell| {
        shell
            .session_states()
            .iter()
            .any(|s| s.track == keys && s.state == SlotState::Playing)
    });
    let clip = slot_clip(&shell, keys, scene).unwrap();
    let length = clip.length;
    let quarter = Ticks::QUARTER.get();
    assert!(
        length.get() > quarter * 3 / 2 && length.get() < 2 * quarter,
        "the loop ends between a line and the next: {length:?}"
    );
    let base = midi_notes(&clip);

    set_value(&mut shell, "1/4");
    let steps = shell.editor().undo_history().len();
    let started = launched_on(&mut shell, keys, scene);
    shell.run("session.record", json!({})).unwrap();
    pass_time(&control, &mut shell, 60);
    assert!(shell.running_recording().is_some());
    // The overdub of a clip that loops leaves the value free to change.
    assert!(shell.can_invoke(
        "transport.set_record_quantization",
        &json!({ "quantization": "1/16" })
    ));

    // Where the clip is in its loop now, in ticks.
    let in_loop = |shell: &mut Shell| since_launch(shell, started, &tempo).get() % length.get();
    // A note 10 ms before the end of the loop, played at 1/4.
    let mut guard = 0;
    while in_loop(&mut shell) < length.get() - quarter / 25 {
        pass_time(&control, &mut shell, 5);
        guard += 1;
        assert!(guard < 400, "the loop never came round");
    }
    tap_a(&control, &mut shell, 0, 1);
    // The value changes for the next pass, which gets three notes.
    set_value(&mut shell, "1/16");
    while !(quarter / 5..quarter * 3 / 10).contains(&in_loop(&mut shell)) {
        pass_time(&control, &mut shell, 5);
    }
    for _ in 0..3 {
        tap_a(&control, &mut shell, 0, 5);
        pass_time(&control, &mut shell, 55);
    }
    shell.run("session.stop_recording", json!({})).unwrap();
    testing::wait_for(&control, &mut shell, "the recording to end", recorded);

    // The set to 1/16 joined the recording's own step, and each pass is
    // a step: no step is the quantising's.
    assert_eq!(history(&shell)[..3], ["Record", "Record", "Overdub Record"]);
    assert_eq!(shell.editor().undo_history().len() - steps, 3);
    let all = midi_notes(&slot_clip(&shell, keys, scene).unwrap());
    // Each Undo takes one pass away: what it took is that pass's notes.
    undo(&mut shell);
    let one_pass_back = midi_notes(&slot_clip(&shell, keys, scene).unwrap());
    undo(&mut shell);
    assert_eq!(midi_notes(&slot_clip(&shell, keys, scene).unwrap()), base);
    let starts_of = |after: &[Note], before: &[Note]| -> Vec<i64> {
        after
            .iter()
            .filter(|note| !before.contains(note))
            .map(|note| note.start.get())
            .collect()
    };
    let first_pass = starts_of(&one_pass_back, &base);
    assert_eq!(first_pass, [0], "the note before the end went to the start");
    let second_pass = starts_of(&all, &one_pass_back);
    assert_eq!(second_pass.len(), 3, "{second_pass:?}");
    assert!(
        second_pass.iter().all(|start| start % SIXTEENTH == 0),
        "{second_pass:?}"
    );
    assert!(
        second_pass.iter().any(|start| start % quarter != 0),
        "{second_pass:?}: on sixteenths, not only quarters"
    );
}

// Spec: recording R21. While an Arrangement recording runs, the value it
// started with holds: the command is refused with the reason and the menu's
// items are disabled, until it ends.
#[test]
fn the_value_is_locked_while_an_arrangement_take_runs() {
    let (mut shell, control) = rig();
    midi_track(&mut shell, "Keys");
    set_value(&mut shell, "1/16");
    let args = json!({ "quantization": "1/8" });
    assert!(shell.can_invoke("transport.set_record_quantization", &args));

    shell
        .run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    flush(&shell);
    pass_time(&control, &mut shell, 50);
    assert!(!shell.can_invoke("transport.set_record_quantization", &args));
    let refused = shell
        .run("transport.set_record_quantization", args.clone())
        .unwrap_err()
        .to_string();
    assert!(refused.contains("recording"), "{refused}");
    let state = shell.run("transport.state", json!({})).unwrap();
    assert_eq!(state["record_quantization"], json!("1/16"));

    shell.run("transport.stop", json!({})).unwrap();
    flush(&shell);
    testing::wait_for(&control, &mut shell, "the recording to end", |shell| {
        shell.running_recording().is_none()
    });
    assert!(shell.can_invoke("transport.set_record_quantization", &args));
    shell
        .run("transport.set_record_quantization", args)
        .unwrap();
}

// Spec: recording R21. A Session slot recording holds the value too.
#[test]
fn the_value_is_locked_while_a_session_slot_take_runs() {
    let (mut shell, control) = rig();
    midi_track(&mut shell, "Keys");
    let args = json!({ "quantization": "1/8" });
    shell.run("session.record", json!({})).unwrap();
    pass_time(&control, &mut shell, 50);
    assert!(!shell.can_invoke("transport.set_record_quantization", &args));
    assert!(
        shell
            .run("transport.set_record_quantization", args.clone())
            .is_err()
    );
    shell.run("session.stop_recording", json!({})).unwrap();
    testing::wait_for(&control, &mut shell, "the recording to end", recorded);
    assert!(shell.can_invoke("transport.set_record_quantization", &args));
}

// Spec: recording R17. The setting is one undo step; `on` returns to the
// last value other than none; the set saves it and a set saved before it
// opens with none.
#[test]
fn the_setting_is_an_undo_step_and_on_returns_to_the_last_value() {
    let (mut shell, _control) = rig();
    let state = |shell: &mut Shell| shell.run("transport.state", json!({})).unwrap();
    assert_eq!(state(&mut shell)["record_quantization"], json!("none"));
    let steps = shell.editor().undo_history().len();
    set_value(&mut shell, "1/8t");
    assert_eq!(shell.editor().undo_history().len(), steps + 1);
    assert_eq!(state(&mut shell)["record_quantization"], json!("1/8t"));
    shell
        .run("transport.set_record_quantization", json!({ "on": false }))
        .unwrap();
    let after = state(&mut shell);
    assert_eq!(after["record_quantization"], json!("none"));
    assert_eq!(after["record_quantization_last"], json!("1/8t"));
    shell
        .run("transport.set_record_quantization", json!({ "on": true }))
        .unwrap();
    assert_eq!(state(&mut shell)["record_quantization"], json!("1/8t"));
    // Both forms or neither is refused.
    for args in [
        json!({}),
        json!({ "quantization": "1/8", "on": true }),
        json!({ "quantization": "1/7" }),
    ] {
        assert!(
            shell
                .run("transport.set_record_quantization", args)
                .is_err()
        );
    }
    undo(&mut shell);
    assert_eq!(state(&mut shell)["record_quantization"], json!("none"));
}

#[test]
fn the_menu_ticks_the_sets_value_and_each_item_chooses_its_own() {
    let (mut shell, _control) = rig();
    let rows = [
        ("none", "No Quantization"),
        ("1/4", "Quarter-Note Quantization"),
        ("1/8", "Eighth-Note Quantization"),
        ("1/8t", "Eighth-Note Triplets Quantization"),
        ("1/8+1/8t", "Eighth-Note and Triplets Quantization"),
        ("1/16", "Sixteenth-Note Quantization"),
        ("1/16t", "Sixteenth-Note Triplets Quantization"),
        ("1/16+1/16t", "Sixteenth-Note and Triplets Quantization"),
        ("1/32", "Thirty-Second Note Quantization"),
    ];
    for (value, _) in rows {
        shell
            .invoke(
                "transport.set_record_quantization",
                json!({ "quantization": value }),
            )
            .unwrap();
        let ticked: Vec<&str> = rows
            .iter()
            .filter(|(other, _)| {
                shell.choice_state(
                    "transport.set_record_quantization",
                    &json!({ "quantization": other }),
                ) == Some(true)
            })
            .map(|(other, _)| *other)
            .collect();
        assert_eq!(ticked, [value], "one item ticked");
    }
}

// Spec: recording R17. The value and the last one other than none are saved
// with the set; a set saved before them, which has neither, opens with no
// quantisation.
#[test]
fn the_setting_saves_and_reopens_with_the_set_and_an_older_set_opens_with_none() {
    let dir = std::env::temp_dir().join(format!("bl-record-quantization-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (mut shell, _control) = rig();
    let settings = |shell: &Shell| shell.editor().document().settings().clone();
    // A new set writes neither field.
    let written = serde_json::to_value(settings(&shell)).unwrap();
    assert!(written.get("record_quantization").is_none(), "{written}");
    assert!(
        written.get("record_quantization_last").is_none(),
        "{written}"
    );
    let older: bl_model::Settings = serde_json::from_value(written).unwrap();
    assert_eq!(
        older.record_quantization,
        bl_model::RecordQuantization::None
    );

    set_value(&mut shell, "1/16t");
    shell
        .run("transport.set_record_quantization", json!({ "on": false }))
        .unwrap();
    let saved = shell
        .run(
            "set.save_as",
            json!({ "path": dir.join("Quantized.blset").to_string_lossy() }),
        )
        .unwrap();
    let (project, _) =
        bl_project::load(std::path::Path::new(saved["path"].as_str().unwrap())).unwrap();
    let loaded = project.document.settings();
    assert_eq!(
        loaded.record_quantization,
        bl_model::RecordQuantization::None
    );
    assert_eq!(
        loaded.record_quantization_last,
        bl_model::RecordQuantization::SixteenthTriplet
    );
    set_value(&mut shell, "1/32");
    let saved = shell.run("set.save", json!({})).unwrap();
    let (project, _) =
        bl_project::load(std::path::Path::new(saved["path"].as_str().unwrap())).unwrap();
    assert_eq!(
        project.document.settings().record_quantization,
        bl_model::RecordQuantization::ThirtySecond
    );
    let _ = std::fs::remove_dir_all(&dir);
}

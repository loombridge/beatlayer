//! The Clip View's Main Properties panel through the shell (WP-6.55): the
//! Set buttons' command moving a marker of a playing clip to where it has
//! reached, on the launch quantisation's grid, and refused while the clip
//! doesn't play; `clip.set_signature` on MIDI and warped audio clips; Start
//! and End typed for several clips; the left group's choice. The rules are
//! in the clip properties spec.

use bl_audio_io::{BackendKind, NullBackend, NullControl, NullOptions, Pacing};
use bl_core::time::{Ticks, TimeSignature};
use bl_model::{Clip, ClipData, ClipId, MidiData, SceneId, TrackId};
use bl_shell::audio::{AudioHost, AudioSettings};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::{Shell, testing};
use serde_json::{Value, json};

/// A shell on a null device paced by hand, with launches at once.
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
    shell.start_audio();
    shell
        .run(
            "session.set_quantization",
            json!({ "quantization": "none" }),
        )
        .unwrap();
    (shell, control)
}

fn first_scene(shell: &Shell) -> SceneId {
    shell.editor().document().scenes().next().unwrap().id
}

/// A new MIDI track with a four-bar clip in the first scene's slot.
fn session_clip(shell: &mut Shell) -> (TrackId, SceneId, ClipId) {
    shell
        .invoke("track.create", json!({ "kind": "midi", "name": "Keys" }))
        .unwrap();
    let track = shell.editor().document().top_level().last().unwrap();
    let scene = first_scene(shell);
    shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    let clip = shell
        .editor()
        .document()
        .slot(track, scene)
        .map(|clip| clip.id)
        .unwrap();
    shell
        .run(
            "clip.set_length",
            json!({ "clip": clip, "length": Ticks::WHOLE.get() * 4 }),
        )
        .unwrap();
    (track, scene, clip)
}

/// A four-bar MIDI clip in the Arrangement, starting at bar 2.
fn arrangement_clip(shell: &mut Shell) -> ClipId {
    shell
        .invoke("track.create", json!({ "kind": "midi", "name": "Pad" }))
        .unwrap();
    let track = shell.editor().document().top_level().last().unwrap();
    let id: ClipId = shell.new_id();
    let clip = Clip::new(
        id,
        Ticks::WHOLE,
        Ticks::WHOLE * 4,
        ClipData::Midi(MidiData::new([])),
    );
    shell
        .run("clip.create", json!({ "track": track, "clip": clip }))
        .unwrap();
    id
}

fn midi(shell: &Shell, clip: ClipId) -> MidiData {
    match &shell.editor().document().clip(clip).unwrap().data {
        ClipData::Midi(midi) => midi.clone(),
        ClipData::Audio(_) => unreachable!("a MIDI clip"),
    }
}

fn steps(shell: &Shell) -> usize {
    shell.editor().undo_history().len()
}

fn set(shell: &mut Shell, clip: ClipId, marker: &str) -> Result<Value, bl_shell::ShellError> {
    shell.run(
        "clip.set_marker_at_playhead",
        json!({ "clip": clip, "marker": marker }),
    )
}

/// Undoes the newest step.
fn undo(shell: &mut Shell) {
    shell.run("edit.undo", json!({})).unwrap();
}

/// Launches the slot and lets about `millis` of test time go by.
fn play_slot(
    shell: &mut Shell,
    control: &NullControl,
    track: TrackId,
    scene: SceneId,
    millis: u64,
) {
    shell
        .run("session.launch", json!({ "track": track, "scene": scene }))
        .unwrap();
    testing::advance(control, shell, millis);
}

// Spec: clip-properties R5, R12. Set is refused while the clip doesn't play,
// and changes nothing; while it plays Start and End move to where the clip
// has reached, each as one undo step.
#[test]
fn set_moves_start_and_end_of_a_playing_session_clip_to_where_it_has_reached() {
    let (mut shell, control) = rig();
    let (track, scene, clip) = session_clip(&mut shell);
    let before = steps(&shell);
    for marker in ["start", "end", "loop_position", "loop_length"] {
        assert!(
            set(&mut shell, clip, marker).is_err(),
            "{marker} while stopped"
        );
    }
    assert_eq!(steps(&shell), before);

    play_slot(&mut shell, &control, track, scene, 1_000);
    // About two beats at 120 BPM have gone by, and the quantisation is None.
    let at = set(&mut shell, clip, "start").unwrap()["position"]
        .as_f64()
        .unwrap();
    assert!(at > 0.0, "the clip has played on");
    assert_eq!(midi(&shell, clip).start, Ticks(at as i64));
    assert_eq!(steps(&shell), before + 1);

    undo(&mut shell);
    assert_eq!(midi(&shell, clip).start, Ticks::ZERO);
    let at = set(&mut shell, clip, "end").unwrap()["position"]
        .as_f64()
        .unwrap();
    assert_eq!(midi(&shell, clip).end, Some(Ticks(at as i64)));
    assert_eq!(steps(&shell), before + 1);
}

// Spec: clip-properties R5. With a quantisation the marker lands on the
// nearest step at or before where the clip has reached.
#[test]
fn set_rounds_down_to_the_global_launch_quantisation() {
    let (mut shell, control) = rig();
    let (track, scene, clip) = session_clip(&mut shell);
    play_slot(&mut shell, &control, track, scene, 5_300);
    let exact = set(&mut shell, clip, "end").unwrap()["position"]
        .as_f64()
        .unwrap() as i64;
    undo(&mut shell);
    shell
        .run(
            "session.set_quantization",
            json!({ "quantization": "1_bar" }),
        )
        .unwrap();
    let rounded = set(&mut shell, clip, "end").unwrap()["position"]
        .as_f64()
        .unwrap() as i64;
    let bar = Ticks::WHOLE.get();
    assert!(rounded > 0 && rounded % bar == 0, "{rounded} is on a bar");
    assert!(
        rounded <= exact + bar && exact - rounded < 2 * bar,
        "{rounded} is the bar before {exact}"
    );
    assert_eq!(midi(&shell, clip).end, Some(Ticks(rounded)));
}

// Spec: clip-properties R8. Position moves the brace's start and switches
// the loop on; Length moves its end, and with the loop off moves the whole
// brace to end there, keeping its length. One undo step each.
#[test]
fn set_moves_the_loop_brace_and_switches_the_loop_on() {
    let (mut shell, control) = rig();
    let (track, scene, clip) = session_clip(&mut shell);
    shell
        .run(
            "clip.set_midi_loop",
            json!({ "clip": clip, "start": 0, "end": Ticks::WHOLE.get() }),
        )
        .unwrap();
    play_slot(&mut shell, &control, track, scene, 5_000);
    let before = steps(&shell);

    // Loop off: Length moves the whole brace so that it ends where the clip
    // has reached, one bar long still.
    let at = set(&mut shell, clip, "loop_length").unwrap()["position"]
        .as_f64()
        .unwrap() as i64;
    let data = midi(&shell, clip);
    assert!(data.looping);
    assert_eq!(data.loop_end, Some(Ticks(at)));
    assert_eq!(data.loop_start, Ticks(at) - Ticks::WHOLE);
    assert_eq!(steps(&shell), before + 1);
    undo(&mut shell);
    assert!(!midi(&shell, clip).looping);
    assert_eq!(midi(&shell, clip).loop_start, Ticks::ZERO);
}

// Spec: clip-properties R5. An Arrangement clip counts from where the
// playhead entered it.
#[test]
fn set_works_on_an_arrangement_clip_while_the_playhead_is_inside_it() {
    let (mut shell, control) = rig();
    let clip = arrangement_clip(&mut shell);
    assert!(set(&mut shell, clip, "start").is_err(), "stopped");
    shell.run("transport.play", json!({})).unwrap();
    // The clip starts at bar 2, two seconds in: the playhead is not there yet.
    testing::advance(&control, &mut shell, 1_000);
    assert!(set(&mut shell, clip, "start").is_err(), "before the clip");
    testing::advance(&control, &mut shell, 2_000);
    let at = set(&mut shell, clip, "start").unwrap()["position"]
        .as_f64()
        .unwrap() as i64;
    assert!(at > 0 && at < Ticks::WHOLE.get(), "about a beat in: {at}");
    assert_eq!(midi(&shell, clip).start, Ticks(at));
}

// Spec: clip-properties R10, R12. A MIDI clip's Signature sets and clears,
// one undo step each, and reads back from the clip's data.
#[test]
fn set_signature_sets_and_clears_a_midi_clips_own_signature() {
    let (mut shell, _control) = rig();
    let (_, _, clip) = session_clip(&mut shell);
    let before = steps(&shell);
    shell
        .run(
            "clip.set_signature",
            json!({ "clip": clip, "signature": { "numerator": 7, "denominator": 8 } }),
        )
        .unwrap();
    assert_eq!(midi(&shell, clip).signature, TimeSignature::new(7, 8).ok());
    assert_eq!(steps(&shell), before + 1);
    shell
        .run(
            "clip.set_signature",
            json!({ "clip": clip, "signature": null }),
        )
        .unwrap();
    assert_eq!(midi(&shell, clip).signature, None);
    assert_eq!(steps(&shell), before + 2);
    // A numerator of 100 and a denominator of 3 aren't signatures.
    for bad in [
        json!({ "numerator": 100, "denominator": 4 }),
        json!({ "numerator": 4, "denominator": 3 }),
    ] {
        assert!(
            shell
                .run(
                    "clip.set_signature",
                    json!({ "clip": clip, "signature": bad })
                )
                .is_err()
        );
    }
    assert_eq!(steps(&shell), before + 2);
}

// Spec: clip-properties R11. Start typed with two clips selected sets both
// in one undo step.
#[test]
fn start_typed_for_several_clips_is_one_undo_step() {
    let (mut shell, _control) = rig();
    let first = arrangement_clip(&mut shell);
    let second = arrangement_clip(&mut shell);
    let before = steps(&shell);
    shell
        .run(
            "clip.set_midi_region",
            json!({ "clips": [first, second], "start": Ticks::QUARTER.get() }),
        )
        .unwrap();
    assert_eq!(steps(&shell), before + 1);
    assert_eq!(midi(&shell, first).start, Ticks::QUARTER);
    assert_eq!(midi(&shell, second).start, Ticks::QUARTER);
    undo(&mut shell);
    assert_eq!(midi(&shell, first).start, Ticks::ZERO);
    assert_eq!(midi(&shell, second).start, Ticks::ZERO);
}

// Spec: clip-properties R1. The left group shows Main Properties until
// Launch Properties is chosen, and an Arrangement clip has Launch Properties
// in neither case; the choice is no undo step.
#[test]
fn the_left_group_shows_main_properties_unless_a_session_clip_chooses_launch() {
    let (mut shell, _control) = rig();
    let (track, scene, _) = session_clip(&mut shell);
    shell
        .run("session.select", json!({ "track": track, "scene": scene }))
        .unwrap();
    let before = steps(&shell);
    let shown = |shell: &Shell| {
        shell
            .clip_view()
            .shown_properties(shell.editor().document(), shell.arrangement())
    };
    assert_eq!(
        shown(&shell),
        bl_shell::clip_properties_commands::Properties::Main
    );
    let result = shell
        .run("clip_view.show_properties", json!({ "panel": "launch" }))
        .unwrap();
    assert_eq!(result["panel"], "launch");
    assert_eq!(
        shown(&shell),
        bl_shell::clip_properties_commands::Properties::Launch
    );
    assert_eq!(steps(&shell), before);
}

// Spec: clip-properties R4. Show names where a marker is, and leaves the set
// alone; on a MIDI clip the note editor already shows its whole length.
#[test]
fn show_marker_reads_where_the_marker_is_and_changes_nothing_in_the_set() {
    let (mut shell, _control) = rig();
    let (_, _, clip) = session_clip(&mut shell);
    shell
        .run(
            "clip.set_midi_loop",
            json!({ "clip": clip, "start": Ticks::QUARTER.get(), "end": Ticks::WHOLE.get() }),
        )
        .unwrap();
    let before = steps(&shell);
    let shown = |shell: &mut Shell, marker: &str| {
        shell
            .run(
                "clip_view.show_marker",
                json!({ "clip": clip, "marker": marker }),
            )
            .unwrap()["position"]
            .as_f64()
            .unwrap() as i64
    };
    assert_eq!(shown(&mut shell, "start"), 0);
    assert_eq!(shown(&mut shell, "end"), Ticks::WHOLE.get() * 4);
    assert_eq!(shown(&mut shell, "loop_position"), Ticks::QUARTER.get());
    assert_eq!(shown(&mut shell, "loop_length"), Ticks::WHOLE.get());
    assert_eq!(steps(&shell), before);
}

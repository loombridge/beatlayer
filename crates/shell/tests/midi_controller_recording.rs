//! WP-2.29b's acceptance with the test backend: the controllers an armed
//! MIDI track's input sends are recorded into the clip with its notes, in
//! the Arrangement, in a Session slot, and by Capture MIDI. The rules are in
//! the MIDI controller data spec, R10 to R16.
//!
//! - An Arrangement take of a controller alone still makes a clip.
//! - A Session recording keeps CC1 on two channels as one envelope and
//!   pitch bend as another, as one undo step.
//! - Capture MIDI keeps controllers up to the last note-off, and is
//!   disabled by controllers alone.
//! - The whole path allocates nothing on the audio thread.

use bl_audio_io::{BackendKind, NullBackend, NullControl, NullOptions, Pacing};
use bl_midi_io::{Direction, TestBackend};
use bl_model::{AutomationTarget, Clip, ClipData, SceneId, TrackId};
use bl_shell::Shell;
use bl_shell::audio::{AudioHost, AudioSettings};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::testing;
use serde_json::json;

const PORT: &str = "Keys";

/// A shell on a null device whose MIDI port "Keys" the test plays, with
/// launches that take effect at once and the computer MIDI keyboard on.
fn rig() -> (Shell, NullControl, TestBackend) {
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
    let test = TestBackend::new();
    test.add_port(Direction::Input, PORT);
    shell.use_midi_backend(Box::new(test.clone()));
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
    (shell, control, test)
}

/// A new armed MIDI track.
fn midi_track(shell: &mut Shell, control: &NullControl) -> TrackId {
    shell
        .invoke("track.create", json!({ "kind": "midi", "name": "Keys" }))
        .unwrap();
    let track = shell.editor().document().top_level().last().unwrap();
    shell
        .run("mixer.set_arm", json!({ "track": track, "arm": true }))
        .unwrap();
    testing::settle(control, shell);
    track
}

fn pass_time(control: &NullControl, shell: &mut Shell, millis: u64) {
    testing::advance(control, shell, millis);
}

fn poll_until(
    control: &NullControl,
    shell: &mut Shell,
    what: &str,
    done: impl Fn(&mut Shell) -> bool,
) {
    testing::wait_for(control, shell, what, done);
}

/// Plays `bytes` on the port and lets the engine take it.
fn play(control: &NullControl, shell: &mut Shell, test: &TestBackend, bytes: &[u8]) {
    let clock = control.clock().expect("manual pacing");
    assert!(test.send(PORT, clock.now(), bytes), "the port is open");
    pass_time(control, shell, 40);
}

fn cc(number: u8) -> AutomationTarget {
    AutomationTarget::MidiCc { number }
}

/// The values a clip's envelope for `target` holds at its breakpoints.
fn values(clip: &Clip, target: AutomationTarget) -> Vec<f64> {
    clip.envelope(&target)
        .map(|e| e.envelope.points().map(|p| p.value).collect())
        .unwrap_or_default()
}

fn notes(clip: &Clip) -> usize {
    let ClipData::Midi(data) = &clip.data else {
        panic!("a MIDI clip");
    };
    data.notes.len()
}

fn slot_clip(shell: &Shell, track: TrackId, scene: SceneId) -> Option<Clip> {
    shell.editor().document().slot(track, scene).cloned()
}

fn recorded(shell: &mut Shell) -> bool {
    shell.running_recording().is_none() && !shell.editor().in_transaction()
}

// Spec: midi-controller-data R10, R12, R16. A take that received only a
// controller, whether or not the track monitors it, still makes a clip with
// its envelope, no notes and no other undo step; the last recording reads
// back that the track was played.
#[test]
fn an_arrangement_take_of_a_controller_alone_makes_a_clip() {
    let (mut shell, control, test) = rig();
    let keys = midi_track(&mut shell, &control);
    shell
        .run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    pass_time(&control, &mut shell, 100);
    play(&control, &mut shell, &test, &[0xB0, 1, 77]);
    pass_time(&control, &mut shell, 100);
    shell.run("transport.stop", json!({})).unwrap();
    poll_until(&control, &mut shell, "the clip", |shell| {
        shell
            .editor()
            .document()
            .track(keys)
            .is_some_and(|track| track.clips().len() == 1)
    });

    let clip = shell
        .editor()
        .document()
        .track(keys)
        .unwrap()
        .clips()
        .next()
        .cloned()
        .unwrap();
    assert_eq!(notes(&clip), 0);
    let held = values(&clip, cc(1));
    assert_eq!(held.first(), Some(&0.0), "rest before the first message");
    assert_eq!(held.last(), Some(&77.0));
    let state = shell.run("transport.state", json!({})).unwrap();
    assert_eq!(
        state["last_recording"]["midi_tracks"],
        json!([keys]),
        "the take read back"
    );
    let stats = shell.audio().stats().expect("a stream is open");
    assert_eq!(stats.heap_operations, 0, "{stats:?}");
}

// Spec: midi-controller-data R10, R11, R14, R16. A Session recording keeps
// CC1 sent on two channels as one envelope that follows the later message,
// pitch bend as its own, and polyphonic pressure on the held note, all in
// the recording's one undo step.
#[test]
fn a_session_recording_keeps_the_controllers_with_its_notes_in_one_undo_step() {
    let (mut shell, control, test) = rig();
    let keys = midi_track(&mut shell, &control);
    let scene = shell.editor().document().scenes().next().unwrap().id;
    let before = shell.editor().undo_history().len();
    shell.run("session.record", json!({})).unwrap();
    poll_until(&control, &mut shell, "the clip to appear", |shell| {
        slot_clip(shell, keys, scene).is_some()
    });
    play(&control, &mut shell, &test, &[0xB0, 1, 20]);
    play(&control, &mut shell, &test, &[0x90, 60, 100]);
    play(&control, &mut shell, &test, &[0xA0, 60, 90]);
    play(&control, &mut shell, &test, &[0xB1, 1, 99]);
    play(&control, &mut shell, &test, &[0xE0, 0x00, 0x60]);
    play(&control, &mut shell, &test, &[0x80, 60, 64]);
    shell.run("session.stop_recording", json!({})).unwrap();
    poll_until(&control, &mut shell, "the recording to end", recorded);

    let clip = slot_clip(&shell, keys, scene).expect("the recorded clip");
    assert_eq!(notes(&clip), 1);
    let ClipData::Midi(data) = &clip.data else {
        panic!("a MIDI clip");
    };
    let pressure = &data.notes.iter().next().unwrap().pressure;
    assert_eq!(pressure.len(), 1);
    assert_eq!(pressure[0].value, 90);
    let held = values(&clip, cc(1));
    assert_eq!(held.first(), Some(&0.0), "rest before the first message");
    assert_eq!(held.last(), Some(&99.0), "the channel is dropped");
    assert!(held.contains(&20.0));
    assert_eq!(
        values(&clip, AutomationTarget::MidiPitchBend)
            .last()
            .copied(),
        Some(f64::from(0x60 * 128 - 8192))
    );
    assert_eq!(clip.envelopes().len(), 2, "no envelope for pressure");
    assert_eq!(
        shell.editor().undo_history().len(),
        before + 1,
        "one undo step"
    );
    shell.run("edit.undo", json!({})).unwrap();
    assert!(slot_clip(&shell, keys, scene).is_none());
    let stats = shell.audio().stats().expect("a stream is open");
    assert_eq!(stats.heap_operations, 0, "{stats:?}");
}

// Spec: midi-controller-data R15, R16. Controllers alone leave Capture MIDI
// disabled; with notes it keeps the controllers up to the last note-off and
// not the reset sent after it, in the one undo step that makes the clip.
#[test]
fn capture_midi_keeps_the_controllers_up_to_the_last_note_off() {
    let (mut shell, control, test) = rig();
    let keys = midi_track(&mut shell, &control);
    let scene = shell.editor().document().scenes().next().unwrap().id;
    play(&control, &mut shell, &test, &[0xB0, 1, 40]);
    shell.poll_engine();
    assert!(!shell.can_invoke("midi.capture", &json!({})), "no notes");

    play(&control, &mut shell, &test, &[0x90, 60, 100]);
    play(&control, &mut shell, &test, &[0xB0, 1, 80]);
    play(&control, &mut shell, &test, &[0xE0, 0x00, 0x50]);
    play(&control, &mut shell, &test, &[0x80, 60, 64]);
    play(&control, &mut shell, &test, &[0xB0, 1, 0]);
    assert!(shell.can_invoke("midi.capture", &json!({})));
    let before = shell.editor().undo_history().len();
    shell.run("midi.capture", json!({})).unwrap();

    let clip = slot_clip(&shell, keys, scene).expect("the captured clip");
    assert_eq!(notes(&clip), 1);
    let held = values(&clip, cc(1));
    assert!(held.contains(&40.0) && held.contains(&80.0), "{held:?}");
    assert_eq!(
        held.last(),
        Some(&80.0),
        "the 0 after the note-off is not kept"
    );
    assert!(clip.envelope(&AutomationTarget::MidiPitchBend).is_some());
    assert_eq!(shell.editor().undo_history().len(), before + 1);
    let stats = shell.audio().stats().expect("a stream is open");
    assert_eq!(stats.heap_operations, 0, "{stats:?}");
}

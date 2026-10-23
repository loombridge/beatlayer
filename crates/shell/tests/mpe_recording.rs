//! WP-6.17's acceptance with the test backend (`docs/specs/mpe.md` R12 and
//! `docs/specs/midi-io.md` R27): a note an MPE port plays is recorded with
//! its pitch, slide and pressure, stepped, wherever a take records notes,
//! in the recording's own undo step. The port's MPE switch runs by ID, is
//! kept in the preferences and read back by `midi.ports`.
//!
//! - An Arrangement take and a MIDI Arrangement Overdub.
//! - A Session recording and a Session overdub.
//! - Capture MIDI.
//! - The whole path allocates nothing on the audio thread.

bl_rt::install_trap!();

use bl_audio_io::{BackendKind, NullBackend, NullControl, NullOptions, Pacing};
use bl_midi_io::{Direction, TestBackend};
use bl_model::{AutomationTarget, Breakpoint, Clip, ClipData, MidiData, Note, SceneId, TrackId};
use bl_shell::Shell;
use bl_shell::audio::{AudioHost, AudioSettings};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::testing;
use serde_json::{Value, json};

const PORT: &str = "Roli";

/// A shell on a null device whose MIDI port "Roli" the test plays, with
/// launches that take effect at once.
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
    // The port's MPE switch, by ID.
    shell
        .run(
            "midi.set_port",
            json!({ "direction": "input", "name": PORT, "mpe": true }),
        )
        .unwrap();
    (shell, control, test)
}

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

fn play(control: &NullControl, shell: &mut Shell, test: &TestBackend, bytes: &[u8]) {
    let clock = control.clock().expect("manual pacing");
    assert!(test.send(PORT, clock.now(), bytes), "the port is open");
    pass_time(control, shell, 40);
}

/// What an MPE controller sends for one note on channel 2: the channel's
/// values before the note, the note, a change of bend and pressure, a
/// message on the master channel, and the note-off.
fn phrase(control: &NullControl, shell: &mut Shell, test: &TestBackend, key: u8) {
    play(control, shell, test, &[0xE1, 0, 80]);
    play(control, shell, test, &[0xB1, 74, 100]);
    play(control, shell, test, &[0xD1, 20]);
    play(control, shell, test, &[0x91, key, 100]);
    play(control, shell, test, &[0xE1, 0, 96]);
    play(control, shell, test, &[0xD1, 90]);
    play(control, shell, test, &[0xB0, 1, 64]);
    play(control, shell, test, &[0x81, key, 64]);
}

fn notes(clip: &Clip) -> Vec<Note> {
    let ClipData::Midi(data) = &clip.data else {
        panic!("a MIDI clip");
    };
    data.notes.iter().cloned().collect()
}

fn slot_clip(shell: &Shell, track: TrackId, scene: SceneId) -> Option<Clip> {
    shell.editor().document().slot(track, scene).cloned()
}

fn recorded(shell: &mut Shell) -> bool {
    shell.running_recording().is_none() && !shell.editor().in_transaction()
}

fn values(points: &[Breakpoint]) -> Vec<f64> {
    points.iter().map(|p| p.value).collect()
}

/// A bend of 12 and then 24 semitones, to what 14 bits of it make.
fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.01
}

/// Checks the envelopes `phrase` makes of a note: its channel's values
/// before it are its starting values, each change steps, and nothing is
/// kept after the note-off.
fn check(note: &Note) {
    let pitch = values(&note.pitch);
    assert!(pitch.len() >= 2, "{pitch:?}");
    assert_eq!(
        note.pitch[0].at.get(),
        0,
        "a breakpoint at the note's start"
    );
    assert!(near(pitch[0], 12.0), "the bend before the note: {pitch:?}");
    assert!(near(*pitch.last().unwrap(), 24.0), "{pitch:?}");
    let steps: Vec<_> = note.pitch.windows(2).map(|w| w[1].at - w[0].at).collect();
    assert!(steps.iter().any(|at| at.get() == 1), "a step: {steps:?}");
    assert_eq!(values(&note.slide), [100.0], "slide never changed");
    assert_eq!(note.slide[0].at.get(), 0);
    let pressure = values(&note.pressure);
    assert_eq!(pressure.first(), Some(&20.0));
    assert_eq!(pressure.last(), Some(&90.0));
    assert!(note.expressions_fit(), "inside the note");
}

fn cc1(clip: &Clip) -> Vec<f64> {
    clip.envelope(&AutomationTarget::MidiCc { number: 1 })
        .map(|e| e.envelope.points().map(|p| p.value).collect())
        .unwrap_or_default()
}

// Spec: midi-io R27. The switch is set by ID, read back by `midi.ports`, and
// kept in the preferences.
#[test]
fn the_ports_mpe_switch_is_set_by_id_read_back_and_kept() {
    let (mut shell, _control, test) = rig();
    let ports = shell.run("midi.ports", json!({})).unwrap();
    let port = &ports["ports"][0];
    assert_eq!(port["name"], PORT);
    assert_eq!(port["mpe"], json!(true));
    assert_eq!(port["track"], json!(true), "Track stays as it was");
    assert_eq!(shell.preferences().midi.ports[0].mpe, true);
    // Off again, and the other switches are left alone.
    let port = shell
        .run(
            "midi.set_port",
            json!({ "direction": "input", "name": PORT, "mpe": false }),
        )
        .unwrap();
    assert_eq!(
        (port["mpe"].clone(), port["track"].clone()),
        (json!(false), json!(true))
    );
    // MPE alone opens nothing: with Track off the port closes.
    shell
        .run(
            "midi.set_port",
            json!({ "direction": "input", "name": PORT, "track": false, "mpe": true }),
        )
        .unwrap();
    assert!(!test.is_open(Direction::Input, PORT));
    // An output has no MPE switch.
    test.add_port(Direction::Output, "Synth");
    shell.run("midi.ports", json!({})).unwrap();
    let refused = shell.run(
        "midi.set_port",
        json!({ "direction": "output", "name": "Synth", "mpe": true }),
    );
    assert!(refused.is_err());
}

// Spec: mpe R12. An Arrangement take gives the note its three envelopes,
// stepped, and the master channel's message becomes the clip's controller
// envelope, all in the take's one undo step.
#[test]
fn an_arrangement_take_gives_the_note_its_envelopes_in_one_undo_step() {
    let (mut shell, control, test) = rig();
    let keys = midi_track(&mut shell, &control);
    let before = shell.editor().undo_history().len();
    shell
        .run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    pass_time(&control, &mut shell, 100);
    phrase(&control, &mut shell, &test, 60);
    pass_time(&control, &mut shell, 100);
    shell.run("transport.stop", json!({})).unwrap();
    testing::wait_for(&control, &mut shell, "the clip", |shell| {
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
    let made = notes(&clip);
    assert_eq!(made.len(), 1);
    assert_eq!(made[0].key, 60);
    check(&made[0]);
    assert_eq!(cc1(&clip).last(), Some(&64.0), "the master channel's CC1");
    assert!(
        clip.envelope(&AutomationTarget::MidiPitchBend).is_none(),
        "a member channel's bend is the note's, not the clip's"
    );
    assert_eq!(shell.editor().undo_history().len(), before + 1);
    let stats = shell.audio().stats().expect("a stream is open");
    assert_eq!(stats.heap_operations, 0, "{stats:?}");
}

// Spec: mpe R12, R4. A MIDI Arrangement Overdub puts the note, envelopes and
// all, into the clip it starts in, in the take's one undo step.
#[test]
fn an_arrangement_overdub_adds_the_note_with_its_envelopes_to_the_clip() {
    let (mut shell, control, test) = rig();
    let keys = midi_track(&mut shell, &control);
    let clip = shell.new_id();
    let existing = Clip::new(
        clip,
        bl_core::time::Ticks::ZERO,
        bl_core::time::Ticks::WHOLE * 8,
        ClipData::Midi(MidiData::new([Note::new(
            72,
            bl_core::time::Ticks::ZERO,
            bl_core::time::Ticks::QUARTER,
            100,
        )])),
    );
    shell
        .run("clip.create", json!({ "track": keys, "clip": existing }))
        .unwrap();
    shell
        .run("transport.set_midi_overdub", json!({ "enabled": true }))
        .unwrap();
    let before = shell.editor().undo_history().len();
    shell
        .run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    pass_time(&control, &mut shell, 100);
    phrase(&control, &mut shell, &test, 60);
    pass_time(&control, &mut shell, 100);
    shell.run("transport.stop", json!({})).unwrap();
    testing::wait_for(&control, &mut shell, "the overdub", |shell| {
        shell
            .editor()
            .document()
            .clip(clip)
            .is_some_and(|clip| notes(clip).len() == 2)
    });
    let overdubbed = shell.editor().document().clip(clip).cloned().unwrap();
    let all = notes(&overdubbed);
    let added = all.iter().find(|n| n.key == 60).expect("the new note");
    check(added);
    assert!(
        !all.iter().find(|n| n.key == 72).unwrap().has_expression(),
        "the note that was there keeps none"
    );
    assert_eq!(shell.editor().undo_history().len(), before + 1);
    let stats = shell.audio().stats().expect("a stream is open");
    assert_eq!(stats.heap_operations, 0, "{stats:?}");
}

// Spec: mpe R12. A Session recording gives the note its envelopes, in the
// recording's one undo step.
#[test]
fn a_session_recording_gives_the_note_its_envelopes_in_one_undo_step() {
    let (mut shell, control, test) = rig();
    let keys = midi_track(&mut shell, &control);
    let scene = shell.editor().document().scenes().next().unwrap().id;
    let before = shell.editor().undo_history().len();
    shell.run("session.record", json!({})).unwrap();
    testing::wait_for(&control, &mut shell, "the clip to appear", |shell| {
        slot_clip(shell, keys, scene).is_some()
    });
    phrase(&control, &mut shell, &test, 60);
    shell.run("session.stop_recording", json!({})).unwrap();
    testing::wait_for(&control, &mut shell, "the recording to end", recorded);
    let clip = slot_clip(&shell, keys, scene).expect("the recorded clip");
    let made = notes(&clip);
    assert_eq!(made.len(), 1);
    check(&made[0]);
    assert_eq!(cc1(&clip).last(), Some(&64.0));
    assert_eq!(shell.editor().undo_history().len(), before + 1);
    shell.run("edit.undo", json!({})).unwrap();
    assert!(slot_clip(&shell, keys, scene).is_none());
    let stats = shell.audio().stats().expect("a stream is open");
    assert_eq!(stats.heap_operations, 0, "{stats:?}");
}

// Spec: mpe R12. A Session overdub adds the note, envelopes and all, to the
// clip that plays, a pass of its loop in one undo step.
#[test]
fn a_session_overdub_adds_the_note_with_its_envelopes() {
    let (mut shell, control, test) = rig();
    let keys = midi_track(&mut shell, &control);
    let scene = shell.editor().document().scenes().next().unwrap().id;
    // A fast tempo keeps the loop short.
    let map = bl_core::time::TempoMap::constant(bl_core::time::Bpm::new(240.0).unwrap());
    shell
        .run("song.set_tempo", json!({ "tempo": map }))
        .unwrap();
    shell.run("session.record", json!({})).unwrap();
    pass_time(&control, &mut shell, 100);
    play(&control, &mut shell, &test, &[0x91, 72, 100]);
    pass_time(&control, &mut shell, 30);
    play(&control, &mut shell, &test, &[0x81, 72, 64]);
    // A loop of about two seconds, longer than the phrase.
    pass_time(&control, &mut shell, 1_800);
    shell.run("session.stop_recording", json!({})).unwrap();
    testing::wait_for(&control, &mut shell, "the recording to end", recorded);
    assert_eq!(notes(&slot_clip(&shell, keys, scene).unwrap()).len(), 1);
    testing::wait_for(&control, &mut shell, "the clip to play", |shell| {
        shell
            .session_states()
            .iter()
            .any(|s| s.track == keys && s.state == bl_engine::SlotState::Playing)
    });

    let steps = shell.editor().undo_history().len();
    shell.run("session.record", json!({})).unwrap();
    pass_time(&control, &mut shell, 60);
    phrase(&control, &mut shell, &test, 60);
    shell.run("session.stop_recording", json!({})).unwrap();
    testing::wait_for(&control, &mut shell, "the recording to end", recorded);
    let all = notes(&slot_clip(&shell, keys, scene).unwrap());
    let added = all.iter().find(|n| n.key == 60).expect("the new note");
    check(added);
    assert_eq!(
        shell.editor().undo_history().len() - steps,
        1,
        "the phrase fell in one pass"
    );
    let stats = shell.audio().stats().expect("a stream is open");
    assert_eq!(stats.heap_operations, 0, "{stats:?}");
}

// Spec: mpe R12. Capture MIDI gives the note its envelopes, in the one undo
// step that makes the clip.
#[test]
fn capture_midi_gives_the_note_its_envelopes() {
    let (mut shell, control, test) = rig();
    let keys = midi_track(&mut shell, &control);
    let scene = shell.editor().document().scenes().next().unwrap().id;
    phrase(&control, &mut shell, &test, 60);
    assert!(shell.can_invoke("midi.capture", &json!({})));
    let before = shell.editor().undo_history().len();
    shell.run("midi.capture", json!({})).unwrap();
    let clip = slot_clip(&shell, keys, scene).expect("the captured clip");
    let made = notes(&clip);
    assert_eq!(made.len(), 1);
    check(&made[0]);
    assert_eq!(cc1(&clip).last(), Some(&64.0));
    assert_eq!(shell.editor().undo_history().len(), before + 1);
    let stats = shell.audio().stats().expect("a stream is open");
    assert_eq!(stats.heap_operations, 0, "{stats:?}");
    let _ = Value::Null;
}

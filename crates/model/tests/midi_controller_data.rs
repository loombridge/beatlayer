//! MIDI controller data in clips (WP-2.29): a MIDI clip's controller
//! envelopes are edited by the clip envelope commands, each one undo step;
//! a controller or a value that breaks the rules is refused; a note's
//! pressure points must obey R2. The saving is in `repr.rs`'s tests.

use bl_commands::validate;
use bl_core::time::Ticks;
use bl_model::registry;
use bl_model::{
    AutomationTarget, Clip, ClipData, ClipId, Command, Editor, EnvelopeKind, MidiData, Note,
    PressurePoint, TrackId, TrackKind,
};
use serde_json::{Value, json};

fn cc(number: u8) -> AutomationTarget {
    AutomationTarget::MidiCc { number }
}

/// A set with a MIDI track holding a MIDI clip, and an audio track holding
/// an audio clip.
struct Song {
    editor: Editor,
    midi_track: TrackId,
    midi: ClipId,
    audio: ClipId,
    audio_track: TrackId,
}

impl Song {
    fn new() -> Self {
        let mut editor = Editor::new();
        let mut track = |kind| {
            let id = editor.new_id();
            editor
                .apply(Command::CreateTrack {
                    id,
                    kind,
                    name: String::new(),
                    parent: None,
                    index: 0,
                })
                .unwrap();
            id
        };
        let (midi_track, audio_track) = (track(TrackKind::Midi), track(TrackKind::Audio));
        let midi = editor.new_id();
        let audio = editor.new_id();
        for (track, clip) in [
            (
                midi_track,
                Clip::new(
                    midi,
                    Ticks::ZERO,
                    Ticks::WHOLE,
                    ClipData::Midi(MidiData::new([])),
                ),
            ),
            (
                audio_track,
                Clip::new(
                    audio,
                    Ticks::ZERO,
                    Ticks::WHOLE,
                    ClipData::Audio(bl_model::AudioData::new(bl_model::ContentHash([1; 32]))),
                ),
            ),
        ] {
            editor.apply(Command::CreateClip { track, clip }).unwrap();
        }
        Self {
            editor,
            midi_track,
            midi,
            audio,
            audio_track,
        }
    }

    /// Runs a command by ID as an agent does: arguments checked against the
    /// schema, and the run one undo step.
    fn run(&mut self, id: &str, args: Value) -> Result<(), String> {
        let spec_args = {
            let mut commands = bl_commands::Registry::new();
            registry::register(&mut commands).unwrap();
            commands.get(id).unwrap().args.clone().unwrap()
        };
        validate(&spec_args, &args).unwrap_or_else(|e| panic!("{id}: {e}"));
        let before = self.editor.undo_history().len();
        registry::run(&mut self.editor, id, args).map_err(|e| e.to_string())?;
        assert_eq!(
            self.editor.undo_history().len(),
            before + 1,
            "{id} is one undo step"
        );
        Ok(())
    }

    fn envelope(&self, target: AutomationTarget) -> Option<Vec<(i64, f64)>> {
        self.editor
            .document()
            .clip(self.midi)
            .unwrap()
            .envelope(&target)
            .map(|e| e.envelope.points().map(|p| (p.at.get(), p.value)).collect())
    }
}

// Spec: midi-controller-data R1, R6, R9
#[test]
fn controller_envelopes_are_edited_by_command_id_one_undo_step_each() {
    let mut song = Song::new();
    let clip = song.midi;
    for (target, value) in [
        (json!({ "type": "midi.cc", "number": 1 }), 100.0),
        (json!({ "type": "midi.cc", "number": 119 }), 0.0),
        (json!({ "type": "midi.pitch_bend" }), -8192.0),
        (json!({ "type": "midi.channel_pressure" }), 127.0),
    ] {
        song.run(
            "clip.set_envelope_breakpoint",
            json!({ "clip": clip, "target": target, "at": 100, "value": value }),
        )
        .unwrap();
    }
    assert_eq!(song.envelope(cc(1)), Some(vec![(100, 100.0)]));
    assert_eq!(
        song.envelope(AutomationTarget::MidiPitchBend),
        Some(vec![(100, -8192.0)])
    );
    assert!(
        song.editor
            .document()
            .clip(clip)
            .unwrap()
            .envelope(&cc(1))
            .unwrap()
            .linked,
        "no channel, and linked as other clip envelopes are"
    );

    let wheel = json!({ "type": "midi.cc", "number": 1 });
    song.run(
        "clip.set_envelope_breakpoint",
        json!({ "clip": clip, "target": wheel, "at": 400, "value": 20.0 }),
    )
    .unwrap();
    song.run(
        "clip.set_envelope_curve",
        json!({ "clip": clip, "target": wheel, "at": 100, "bend": 0.5 }),
    )
    .unwrap();
    song.run(
        "clip.move_envelope_breakpoint",
        json!({ "clip": clip, "target": wheel, "at": 400, "to": 500, "value": 30.0 }),
    )
    .unwrap();
    song.run(
        "clip.set_envelope_linked",
        json!({ "clip": clip, "target": wheel, "linked": false }),
    )
    .unwrap();
    assert_eq!(song.envelope(cc(1)), Some(vec![(100, 100.0), (500, 30.0)]));
    assert!(
        !song
            .editor
            .document()
            .clip(clip)
            .unwrap()
            .envelope(&cc(1))
            .unwrap()
            .linked
    );
    song.run(
        "clip.delete_envelope_breakpoint",
        json!({ "clip": clip, "target": wheel, "at": 500 }),
    )
    .unwrap();
    song.run(
        "clip.delete_envelope",
        json!({ "clip": clip, "target": wheel }),
    )
    .unwrap();
    assert_eq!(song.envelope(cc(1)), None);
    song.editor.undo().unwrap();
    assert_eq!(
        song.envelope(cc(1)),
        Some(vec![(100, 100.0)]),
        "undoing the delete brings the envelope back"
    );
}

// Spec: midi-controller-data R1, R6
#[test]
fn a_controller_or_value_that_breaks_the_rules_is_refused() {
    let mut song = Song::new();
    let set = |song: &mut Song, clip: ClipId, target: AutomationTarget, value: f64| {
        song.editor.apply(Command::SetClipEnvelopeBreakpoint {
            clip,
            kind: EnvelopeKind::Modulation,
            target,
            at: Ticks::ZERO,
            value,
        })
    };
    let midi = song.midi;
    // Bank select, the second half of it and the channel mode messages
    // aren't controllers a clip holds.
    for number in [0, 32, 120, 123, 127, 128] {
        assert!(
            set(&mut song, midi, cc(number), 0.0).is_err(),
            "CC {number}"
        );
    }
    // Values outside the controller's range.
    for (target, value) in [
        (cc(1), 128.0),
        (cc(1), -1.0),
        (AutomationTarget::MidiPitchBend, 8192.0),
        (AutomationTarget::MidiPitchBend, -8193.0),
        (AutomationTarget::MidiChannelPressure, 128.0),
        (cc(1), f64::NAN),
    ] {
        assert!(
            set(&mut song, midi, target, value).is_err(),
            "{target:?} {value}"
        );
    }
    // The range's ends are fine.
    for (target, value) in [
        (cc(1), 127.0),
        (AutomationTarget::MidiPitchBend, 8191.0),
        (AutomationTarget::MidiChannelPressure, 0.0),
    ] {
        set(&mut song, midi, target, value).unwrap();
    }
    // A moved breakpoint's new value is checked too.
    assert!(
        song.editor
            .apply(Command::MoveClipEnvelopeBreakpoint {
                clip: midi,
                kind: EnvelopeKind::Modulation,
                target: cc(1),
                at: Ticks::ZERO,
                to: Ticks::QUARTER,
                value: Some(200.0),
            })
            .is_err()
    );
    // A controller's envelope only modulates: it isn't a Session clip's
    // automation of a parameter.
    let error = song
        .editor
        .apply(Command::SetClipEnvelopeBreakpoint {
            clip: midi,
            kind: EnvelopeKind::Automation,
            target: cc(1),
            at: Ticks::ZERO,
            value: 1.0,
        })
        .unwrap_err();
    assert!(error.to_string().contains("MIDI controller"), "{error}");
    // An audio clip has no controllers.
    let audio = song.audio;
    let error = set(&mut song, audio, cc(1), 1.0).unwrap_err();
    assert!(
        matches!(error.to_string().as_str(), s if s.contains("audio clip")),
        "{error}"
    );
    // A track has no automation for one.
    let result = song.editor.apply(Command::SetAutomationBreakpoint {
        track: song.audio_track,
        target: cc(1),
        at: Ticks::ZERO,
        value: 1.0,
    });
    assert!(result.is_err());
    // Nothing the refusals touched changed the set.
    assert_eq!(song.envelope(cc(1)), Some(vec![(0, 127.0)]));
}

// Spec: midi-controller-data R2, R7
#[test]
fn a_notes_pressure_points_must_lie_in_order_inside_the_note() {
    let point = |at: i64, value: u8| PressurePoint {
        at: Ticks(at),
        value,
    };
    let note = |pressure: Vec<PressurePoint>| Note {
        pressure,
        ..Note::new(60, Ticks::ZERO, Ticks(1000), 100)
    };
    let mut song = Song::new();
    let track = song.midi_track;
    let make = |song: &mut Song, pressure| {
        let id = song.editor.new_id();
        song.editor.apply(Command::CreateClip {
            track,
            clip: Clip::new(
                id,
                Ticks::WHOLE * 2,
                Ticks::WHOLE,
                ClipData::Midi(MidiData::new([note(pressure)])),
            ),
        })
    };
    assert!(make(&mut song, vec![point(0, 0), point(999, 127)]).is_ok());
    assert!(make(&mut song, vec![]).is_ok());
    for bad in [
        vec![point(0, 128)],
        vec![point(1000, 10)],
        vec![point(-1, 10)],
        vec![point(10, 10), point(10, 20)],
        vec![point(20, 10), point(10, 20)],
    ] {
        let error = make(&mut song, bad.clone()).unwrap_err();
        assert!(error.to_string().contains("note"), "{bad:?} gave {error}");
    }
}

// Spec: midi-controller-data R1. A controller belongs to the clip, not to a
// device of its track, so it goes with the clip to another MIDI track.
#[test]
fn a_clip_keeps_its_controller_envelopes_when_it_moves_to_another_midi_track() {
    let mut song = Song::new();
    let clip = song.midi;
    song.editor
        .apply(Command::SetClipEnvelopeBreakpoint {
            clip,
            kind: EnvelopeKind::Modulation,
            target: cc(1),
            at: Ticks::ZERO,
            value: 5.0,
        })
        .unwrap();
    let other = song.editor.new_id();
    song.editor
        .apply(Command::CreateTrack {
            id: other,
            kind: TrackKind::Midi,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    song.editor
        .apply(Command::MoveClip {
            clip,
            track: other,
            start: Ticks::WHOLE,
        })
        .unwrap();
    assert_eq!(song.envelope(cc(1)), Some(vec![(0, 5.0)]));
}

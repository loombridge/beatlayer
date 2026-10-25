//! Tuning systems in the model (WP-6.9; the tunings spec, R1, R2, R7 to R9,
//! R11, R14 and R15): the set's tuning and its commands, each one undo step;
//! what a set file saves; Bypass Tuning; and retuning the set's notes.

use bl_core::time::Ticks;
use bl_model::{
    Chain, Clip, ClipData, ClipId, Command, Device, Editor, MidiData, Note, RackKind, SceneId,
    TrackId, TrackKind, Tuning, TuningNote,
};

/// C3 of twelve equal steps at A3 = 440 Hz: the frequency that puts key 60
/// at exactly 60 semitones.
fn c3() -> f64 {
    440.0 * 2f64.powf(-9.0 / 12.0)
}

/// `steps` equal steps to the octave from key 60 at exactly C3.
fn equal(steps: usize) -> Tuning {
    let cents = (1..=steps)
        .map(|i| 1200.0 * i as f64 / steps as f64)
        .collect();
    Tuning::new(format!("{steps}-edo"), String::new(), cents)
        .unwrap()
        .with_reference(None, None, Some(c3()))
        .unwrap()
}

fn midi_track(editor: &mut Editor) -> TrackId {
    let track: TrackId = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: "Keys".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();
    track
}

/// A clip of `notes` as `(key, start, length)` in ticks, on the Arrangement.
fn clip_of(editor: &mut Editor, track: TrackId, notes: &[(u8, i64, i64)]) -> ClipId {
    let clip: ClipId = editor.new_id();
    let notes = notes
        .iter()
        .map(|&(key, start, length)| Note::new(key, Ticks(start), Ticks(length), 100));
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(Ticks::WHOLE.0 * 4),
                ClipData::Midi(MidiData::new(notes)),
            ),
        })
        .unwrap();
    clip
}

/// The `(key, start, length)` of a clip's notes, in time order.
fn notes_of(editor: &Editor, clip: ClipId) -> Vec<(u8, i64, i64)> {
    let ClipData::Midi(midi) = &editor.document().clip(clip).unwrap().data else {
        panic!("a MIDI clip");
    };
    let mut notes: Vec<_> = midi
        .notes
        .iter()
        .map(|n| (n.key, n.start.0, n.length.0))
        .collect();
    notes.sort_by_key(|&(key, start, _)| (start, key));
    notes
}

// Spec: tunings R1, R2, R9
#[test]
fn a_set_plays_twelve_steps_until_a_tuning_is_loaded_and_each_edit_is_one_undo_step() {
    let mut editor = Editor::new();
    assert!(editor.document().settings().tuning.is_none());
    let steps = editor.undo_history().len();
    let tuning = equal(19);
    editor
        .apply(Command::SetTuning {
            tuning: tuning.clone(),
            retune: false,
        })
        .unwrap();
    assert_eq!(editor.undo_history().len(), steps + 1);
    assert_eq!(editor.undo_history().next(), Some("Load Tuning"));
    assert_eq!(editor.document().settings().tuning.as_ref(), Some(&tuning));

    // A second tuning takes the first one's place.
    let other = equal(31);
    editor
        .apply(Command::SetTuning {
            tuning: other.clone(),
            retune: false,
        })
        .unwrap();
    assert_eq!(editor.document().settings().tuning.as_ref(), Some(&other));
    editor.undo().unwrap();
    assert_eq!(editor.document().settings().tuning.as_ref(), Some(&tuning));

    editor
        .apply(Command::SetTuningReference {
            octave: None,
            degree: None,
            frequency: Some(300.0),
        })
        .unwrap();
    assert_eq!(editor.undo_history().next(), Some("Change Reference Pitch"));
    editor
        .apply(Command::SetTuningRange {
            lowest: Some(TuningNote {
                octave: 0,
                degree: 0,
            }),
            highest: None,
        })
        .unwrap();
    assert_eq!(editor.undo_history().next(), Some("Change Tuning Range"));
    editor
        .apply(Command::RemoveTuning { retune: false })
        .unwrap();
    assert_eq!(editor.undo_history().next(), Some("Remove Tuning"));
    assert!(editor.document().settings().tuning.is_none());
    editor.undo().unwrap();
    assert!(editor.document().settings().tuning.is_some());
}

// Spec: tunings R2, R7, R8, R9, R14. A command that breaks a rule is refused
// and changes nothing, not even the undo history.
#[test]
fn a_command_that_breaks_a_rule_changes_nothing() {
    let mut editor = Editor::new();
    let track = midi_track(&mut editor);
    let audio: TrackId = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: audio,
            kind: TrackKind::Audio,
            name: String::new(),
            parent: None,
            index: 1,
        })
        .unwrap();
    // With no tuning, there is nothing to change or remove.
    for command in [
        Command::RemoveTuning { retune: false },
        Command::SetTuningReference {
            octave: Some(1),
            degree: None,
            frequency: None,
        },
        Command::SetTuningRange {
            lowest: Some(TuningNote {
                octave: 0,
                degree: 0,
            }),
            highest: None,
        },
    ] {
        assert!(editor.apply(command).is_err());
    }
    editor
        .apply(Command::SetTuning {
            tuning: equal(19),
            retune: false,
        })
        .unwrap();
    let before = editor.document().clone();
    let steps = editor.undo_history().len();
    let refused = [
        // A degree the tuning doesn't have, and a frequency of 0 or less.
        Command::SetTuningReference {
            octave: None,
            degree: Some(19),
            frequency: None,
        },
        Command::SetTuningReference {
            octave: None,
            degree: None,
            frequency: Some(0.0),
        },
        Command::SetTuningReference {
            octave: None,
            degree: None,
            frequency: Some(f64::NAN),
        },
        // More than 128 notes, and a range that ends before it starts.
        Command::SetTuningRange {
            lowest: Some(TuningNote {
                octave: -10,
                degree: 0,
            }),
            highest: Some(TuningNote {
                octave: 10,
                degree: 0,
            }),
        },
        Command::SetTuningRange {
            lowest: Some(TuningNote {
                octave: 3,
                degree: 0,
            }),
            highest: Some(TuningNote {
                octave: 2,
                degree: 0,
            }),
        },
        // Bypass Tuning belongs to MIDI tracks.
        Command::SetTuningBypass {
            track: audio,
            bypass: true,
        },
        // A tuning with no pitches.
        Command::SetTuning {
            tuning: Tuning {
                cents: vec![],
                ..equal(5)
            },
            retune: false,
        },
        Command::SetTuningBypass {
            track: TrackId(bl_core::id::Id::new(999).unwrap()),
            bypass: true,
        },
    ];
    for command in refused {
        assert!(editor.apply(command.clone()).is_err(), "{command:?}");
    }
    assert_eq!(editor.document(), &before);
    assert_eq!(editor.undo_history().len(), steps);
    let _ = track;
}

// Spec: tunings R11
#[test]
fn a_set_saves_its_tuning_inside_and_a_set_from_before_tunings_has_none() {
    let mut editor = Editor::new();
    let track = midi_track(&mut editor);
    let saved = serde_json::to_value(editor.document()).unwrap();
    assert!(
        saved["settings"].get("tuning").is_none(),
        "a set with no tuning writes none"
    );
    let loaded: bl_model::Document = serde_json::from_value(saved).unwrap();
    assert!(loaded.settings().tuning.is_none());
    assert_eq!(
        &loaded,
        editor.document(),
        "a set saved before tunings loads unchanged"
    );

    let mut tuning = equal(19);
    tuning.description = "nineteen".to_owned();
    tuning.names = (0..19).map(|d| format!("n{d}")).collect();
    tuning.link = Some("https://example.org/tuning".to_owned());
    let tuning = tuning
        .with_reference(Some(4), Some(7), Some(333.25))
        .unwrap()
        .with_range(
            Some(TuningNote {
                octave: 0,
                degree: 3,
            }),
            Some(TuningNote {
                octave: 5,
                degree: 3,
            }),
        )
        .unwrap();
    editor
        .apply(Command::SetTuning {
            tuning: tuning.clone(),
            retune: false,
        })
        .unwrap();
    editor
        .apply(Command::SetTuningBypass {
            track,
            bypass: true,
        })
        .unwrap();
    let saved = serde_json::to_value(editor.document()).unwrap();
    let loaded: bl_model::Document = serde_json::from_value(saved).unwrap();
    assert_eq!(loaded.settings().tuning.as_ref(), Some(&tuning));
    assert!(loaded.track(track).unwrap().tuning_bypass());
    assert_eq!(&loaded, editor.document());
}

// Spec: tunings R14
#[test]
fn bypass_tuning_is_saved_only_where_it_is_on() {
    let mut editor = Editor::new();
    let track = midi_track(&mut editor);
    let saved = serde_json::to_string(editor.document()).unwrap();
    assert!(!saved.contains("tuning_bypass"));
    assert!(!editor.document().track(track).unwrap().tuning_bypass());
    editor
        .apply(Command::SetTuningBypass {
            track,
            bypass: true,
        })
        .unwrap();
    assert_eq!(editor.undo_history().next(), Some("Bypass Tuning"));
    assert!(
        serde_json::to_string(editor.document())
            .unwrap()
            .contains("tuning_bypass")
    );
    editor.undo().unwrap();
    assert!(!editor.document().track(track).unwrap().tuning_bypass());
}

// Spec: tunings R11. A saved tuning that breaks the rules doesn't load.
#[test]
fn a_saved_tuning_that_breaks_the_rules_does_not_load() {
    let mut editor = Editor::new();
    editor
        .apply(Command::SetTuning {
            tuning: equal(12),
            retune: false,
        })
        .unwrap();
    let saved = serde_json::to_value(editor.document()).unwrap();
    for (field, value) in [
        ("cents", serde_json::json!([])),
        ("cents", serde_json::json!([-100.0])),
        ("names", serde_json::json!(["a"])),
        (
            "range",
            serde_json::json!({
                "lowest": { "octave": 0, "degree": 0 },
                "highest": { "octave": 99, "degree": 0 },
            }),
        ),
        (
            "reference",
            serde_json::json!({ "octave": 3, "degree": 0, "frequency": -1.0 }),
        ),
    ] {
        let mut broken = saved.clone();
        broken["settings"]["tuning"][field] = value;
        assert!(
            serde_json::from_value::<bl_model::Document>(broken).is_err(),
            "{field}"
        );
    }
}

// Spec: tunings R15
#[test]
fn retuning_moves_each_note_to_the_nearest_key_the_lower_on_a_tie() {
    let mut editor = Editor::new();
    let track = midi_track(&mut editor);
    // Six steps to the octave: a step is two semitones, so key 61 (pitch 61)
    // sits between key 60 (60) and key 61 (62) and key 63 between 61 and 62.
    let clip = clip_of(
        &mut editor,
        track,
        &[(60, 0, 100), (61, 100, 100), (63, 200, 100), (72, 300, 100)],
    );
    editor
        .apply(Command::SetTuning {
            tuning: equal(6),
            retune: true,
        })
        .unwrap();
    assert_eq!(
        notes_of(&editor, clip),
        vec![(60, 0, 100), (60, 100, 100), (61, 200, 100), (66, 300, 100)],
        "key 72 at pitch 72 is six steps up"
    );
    assert_eq!(editor.undo_history().next(), Some("Load Tuning"));
    editor.undo().unwrap();
    assert_eq!(
        notes_of(&editor, clip),
        vec![(60, 0, 100), (61, 100, 100), (63, 200, 100), (72, 300, 100)],
        "one undo step puts the notes back with the tuning"
    );
    assert!(editor.document().settings().tuning.is_none());
}

// Spec: tunings R15. With it off, the notes stay on their keys.
#[test]
fn loading_without_retune_leaves_every_note_on_its_key() {
    let mut editor = Editor::new();
    let track = midi_track(&mut editor);
    let clip = clip_of(&mut editor, track, &[(61, 0, 100), (72, 100, 100)]);
    editor
        .apply(Command::SetTuning {
            tuning: equal(6),
            retune: false,
        })
        .unwrap();
    assert_eq!(notes_of(&editor, clip), vec![(61, 0, 100), (72, 100, 100)]);
}

// Spec: tunings R15
#[test]
fn notes_that_overlap_on_one_key_after_a_retune_are_shortened_or_deleted() {
    let mut editor = Editor::new();
    let track = midi_track(&mut editor);
    // Keys 60 and 61 both land on key 60. The first pair overlaps: the
    // earlier is shortened to end where the later starts. The second pair
    // starts together: the shorter is deleted. The third pair doesn't
    // overlap and both stay.
    let clip = clip_of(
        &mut editor,
        track,
        &[
            (60, 0, 480),
            (61, 240, 480),
            (60, 1000, 480),
            (61, 1000, 240),
            (60, 2000, 100),
            (61, 2100, 100),
        ],
    );
    editor
        .apply(Command::SetTuning {
            tuning: equal(6),
            retune: true,
        })
        .unwrap();
    assert_eq!(
        notes_of(&editor, clip),
        vec![
            (60, 0, 240),
            (60, 240, 480),
            (60, 1000, 480),
            (60, 2000, 100),
            (60, 2100, 100),
        ]
    );
}

// Spec: tunings R15. Notes that overlapped before and didn't move are left
// as they were.
#[test]
fn a_retune_leaves_alone_the_notes_it_didnt_move() {
    let mut editor = Editor::new();
    let track = midi_track(&mut editor);
    // Key 60 stays at key 60; two notes on it overlap already.
    let clip = clip_of(&mut editor, track, &[(60, 0, 480), (60, 240, 480)]);
    editor
        .apply(Command::SetTuning {
            tuning: equal(6),
            retune: true,
        })
        .unwrap();
    assert_eq!(notes_of(&editor, clip), vec![(60, 0, 480), (60, 240, 480)]);
}

// Spec: tunings R14, R15. Only tracks that play the tuning are retuned: not
// one that bypasses it, and not one that holds a Drum Rack. Session clips
// are retuned too.
#[test]
fn a_retune_moves_only_the_notes_of_tracks_that_play_the_tuning() {
    let mut editor = Editor::new();
    let plain = midi_track(&mut editor);
    let bypassing = midi_track(&mut editor);
    let drums = midi_track(&mut editor);
    let notes = [(61u8, 0i64, 100i64)];
    let plain_clip = clip_of(&mut editor, plain, &notes);
    let bypass_clip = clip_of(&mut editor, bypassing, &notes);
    let drum_clip = clip_of(&mut editor, drums, &notes);
    editor
        .apply(Command::SetTuningBypass {
            track: bypassing,
            bypass: true,
        })
        .unwrap();
    let mut rack = Device::rack(editor.new_id(), RackKind::Drum);
    let chain = editor.new_id();
    rack = rack.with_chain(Chain::for_pad(chain, "Pad".to_owned(), 36));
    editor
        .apply(Command::InsertDevice {
            track: drums,
            index: 0,
            device: rack,
        })
        .unwrap();
    assert!(editor.document().track(drums).unwrap().holds_drum_rack());
    assert!(!editor.document().track(drums).unwrap().plays_tuning());
    assert!(!editor.document().track(bypassing).unwrap().plays_tuning());
    assert!(editor.document().track(plain).unwrap().plays_tuning());

    // A Session clip on the plain track.
    let scene: SceneId = editor.new_id();
    editor
        .apply(Command::CreateScene {
            id: scene,
            index: 0,
        })
        .unwrap();
    let slot: ClipId = editor.new_id();
    editor
        .apply(Command::CreateSlotClip {
            track: plain,
            scene,
            clip: Clip::new(
                slot,
                Ticks::ZERO,
                Ticks::WHOLE,
                ClipData::Midi(MidiData::new([Note::new(61, Ticks::ZERO, Ticks(100), 100)])),
            ),
        })
        .unwrap();

    editor
        .apply(Command::SetTuning {
            tuning: equal(6),
            retune: true,
        })
        .unwrap();
    assert_eq!(notes_of(&editor, plain_clip), vec![(60, 0, 100)]);
    assert_eq!(notes_of(&editor, slot), vec![(60, 0, 100)]);
    assert_eq!(notes_of(&editor, bypass_clip), vec![(61, 0, 100)]);
    assert_eq!(notes_of(&editor, drum_clip), vec![(61, 0, 100)]);
}

// Spec: tunings R15. Removing a tuning retunes the notes back to the keys
// nearest the pitches they had.
#[test]
fn removing_a_tuning_can_retune_the_notes_back() {
    let mut editor = Editor::new();
    let track = midi_track(&mut editor);
    let clip = clip_of(&mut editor, track, &[(60, 0, 100), (62, 100, 100)]);
    editor
        .apply(Command::SetTuning {
            tuning: equal(6),
            retune: false,
        })
        .unwrap();
    // Under six steps key 62 plays pitch 64: removing the tuning with
    // retune puts that note on key 64, and key 60 stays.
    editor
        .apply(Command::RemoveTuning { retune: true })
        .unwrap();
    assert_eq!(notes_of(&editor, clip), vec![(60, 0, 100), (64, 100, 100)]);
    editor.undo().unwrap();
    assert_eq!(notes_of(&editor, clip), vec![(60, 0, 100), (62, 100, 100)]);
    assert!(editor.document().settings().tuning.is_some());
}

// Spec: tunings R15. A note whose key plays nothing under the old tuning, or
// when no key plays under the new one, stays where it is.
#[test]
fn a_note_on_a_key_that_played_nothing_stays_put() {
    let mut editor = Editor::new();
    let track = midi_track(&mut editor);
    let clip = clip_of(&mut editor, track, &[(100, 0, 100)]);
    let short = equal(12)
        .with_range(
            Some(TuningNote {
                octave: -2,
                degree: 0,
            }),
            Some(TuningNote {
                octave: 3,
                degree: 0,
            }),
        )
        .unwrap();
    editor
        .apply(Command::SetTuning {
            tuning: short,
            retune: false,
        })
        .unwrap();
    editor
        .apply(Command::SetTuning {
            tuning: equal(6),
            retune: true,
        })
        .unwrap();
    assert_eq!(notes_of(&editor, clip), vec![(100, 0, 100)]);
}

// Spec: tunings R22, R23. The MIDI Controller Layout and its custom table
// are one undo step each, and save and reopen.
#[test]
fn the_controller_layout_is_one_undo_step_and_survives_save_and_reopen() {
    let mut editor = Editor::new();
    let track = midi_track(&mut editor);
    let saved = serde_json::to_string(editor.document()).unwrap();
    assert!(!saved.contains("tuning_layout"));
    editor
        .apply(Command::SetTuningLayout {
            track,
            layout: Some(bl_model::LayoutKind::ClosestInPitch),
            keys: None,
        })
        .unwrap();
    assert_eq!(
        editor.undo_history().next(),
        Some("Change MIDI Controller Layout")
    );
    let mut keys: Vec<Option<u8>> = (0..128).map(Some).collect();
    keys[61] = Some(70);
    keys[62] = None;
    editor
        .apply(Command::SetTuningLayout {
            track,
            layout: Some(bl_model::LayoutKind::Custom),
            keys: Some(keys.clone()),
        })
        .unwrap();
    let saved = serde_json::to_value(editor.document()).unwrap();
    let back: bl_model::Document = serde_json::from_value(saved).unwrap();
    let layout = back.track(track).unwrap().tuning_layout();
    assert_eq!(layout.layout, bl_model::LayoutKind::Custom);
    assert_eq!(layout.keys, keys);
    // The table is kept while another layout is chosen.
    editor
        .apply(Command::SetTuningLayout {
            track,
            layout: Some(bl_model::LayoutKind::AllKeys),
            keys: None,
        })
        .unwrap();
    assert_eq!(
        editor.document().track(track).unwrap().tuning_layout().keys,
        keys
    );
    editor.undo().unwrap();
    assert_eq!(
        editor
            .document()
            .track(track)
            .unwrap()
            .tuning_layout()
            .layout,
        bl_model::LayoutKind::Custom
    );
    // A table with the wrong size, or a key past 127, is refused.
    for bad in [vec![None; 5], vec![Some(200); 128]] {
        assert!(
            editor
                .apply(Command::SetTuningLayout {
                    track,
                    layout: None,
                    keys: Some(bad),
                })
                .is_err()
        );
    }
    // Only a MIDI track has a layout.
    let audio: TrackId = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: audio,
            kind: TrackKind::Audio,
            name: "Audio".to_owned(),
            parent: None,
            index: 1,
        })
        .unwrap();
    assert!(
        editor
            .apply(Command::SetTuningLayout {
                track: audio,
                layout: Some(bl_model::LayoutKind::AllKeys),
                keys: None,
            })
            .is_err()
    );
}

// Spec: tunings R20. A plug-in's tuning switch is on in a new device, saved
// only where off, and one undo step.
#[test]
fn a_plugins_tuning_switch_is_on_in_a_new_device_and_saved_only_where_off() {
    let mut editor = Editor::new();
    let track = midi_track(&mut editor);
    let device: bl_model::DeviceId = editor.new_id();
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: Device::new(
                device,
                "Synth",
                bl_model::DeviceKind::Plugin {
                    format: bl_model::PluginFormat::Clap,
                    plugin_id: "com.example.synth".to_owned(),
                },
            ),
        })
        .unwrap();
    assert!(editor.document().device(device).unwrap().mpe.tuning);
    assert!(
        !serde_json::to_string(editor.document())
            .unwrap()
            .contains("\"tuning\"")
    );
    editor
        .apply(Command::SetPluginTuning { device, on: false })
        .unwrap();
    assert_eq!(
        editor.undo_history().next(),
        Some("Apply Tuning to Per-Note Pitch Bend")
    );
    let saved = serde_json::to_value(editor.document()).unwrap();
    let back: bl_model::Document = serde_json::from_value(saved).unwrap();
    assert!(!back.device(device).unwrap().mpe.tuning);
    editor.undo().unwrap();
    assert!(editor.document().device(device).unwrap().mpe.tuning);
    // A device that isn't a plug-in has no such switch.
    let native: bl_model::DeviceId = editor.new_id();
    editor
        .apply(Command::InsertDevice {
            track,
            index: 1,
            device: Device::new(
                native,
                "Gate",
                bl_model::DeviceKind::Native {
                    device_type: "gate".to_owned(),
                },
            ),
        })
        .unwrap();
    assert!(
        editor
            .apply(Command::SetPluginTuning {
                device: native,
                on: false
            })
            .is_err()
    );
}

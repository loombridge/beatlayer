//! Grooves in the model (WP-6.19): the Groove Pool, a clip's groove, the
//! commands that set them, and what a set file saves. Spec: grooves R1 to R3,
//! R5, R20 and R22.

use bl_core::time::Ticks;
use bl_model::{
    Clip, ClipData, ClipId, Command, Document, EditError, Editor, Groove, GrooveBase, GrooveId,
    GrooveNote, MidiData, ModelError, TrackId, TrackKind,
};
use serde_json::json;

fn song() -> (Editor, TrackId, ClipId) {
    let mut editor = Editor::new();
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
    let clip: ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks::WHOLE,
                ClipData::Midi(MidiData::default()),
            ),
        })
        .unwrap();
    (editor, track, clip)
}

fn swing(id: GrooveId) -> Groove {
    let step = GrooveBase::Sixteenth.ticks();
    Groove::new(
        id,
        "Swing".to_owned(),
        Ticks::QUARTER,
        vec![
            GrooveNote {
                position: Ticks::ZERO,
                velocity: 127,
            },
            GrooveNote {
                position: step + step / 5,
                velocity: 100,
            },
        ],
    )
}

fn added() -> (Editor, TrackId, ClipId, GrooveId) {
    let (mut editor, track, clip) = song();
    let groove: GrooveId = editor.new_id();
    editor
        .apply(Command::AddGroove {
            groove: swing(groove),
        })
        .unwrap();
    (editor, track, clip, groove)
}

#[test]
fn a_groove_is_added_to_the_end_of_the_pool_and_undone() {
    // Spec: grooves R1, R5
    let (mut editor, _, _, first) = added();
    let second: GrooveId = editor.new_id();
    editor
        .apply(Command::AddGroove {
            groove: swing(second),
        })
        .unwrap();
    let pool = &editor.document().settings().grooves;
    assert_eq!(
        pool.pool.iter().map(|g| g.id).collect::<Vec<_>>(),
        [first, second]
    );
    assert_eq!(
        (pool.global_amount, pool.auto_load, pool.auto_load_groove),
        (100.0, false, None)
    );
    let groove = &pool.pool[0];
    assert_eq!(
        (
            groove.base,
            groove.quantize,
            groove.timing,
            groove.random,
            groove.velocity
        ),
        (GrooveBase::Sixteenth, 0.0, 100.0, 0.0, 0.0)
    );
    editor.undo().unwrap();
    assert_eq!(editor.document().settings().grooves.pool.len(), 1);
}

#[test]
fn a_groove_that_breaks_the_limits_is_refused() {
    // Spec: grooves R1, R2
    let (mut editor, _, _, _) = added();
    let mut broken = |change: &dyn Fn(&mut Groove)| {
        let id: GrooveId = editor.new_id();
        let mut groove = swing(id);
        change(&mut groove);
        let error = editor
            .apply(Command::AddGroove { groove })
            .expect_err("refused");
        assert!(matches!(error, EditError::Command(_)), "{error:?}");
    };
    broken(&|g| g.length = Ticks::ZERO);
    broken(&|g| g.notes[1].position = g.length);
    broken(&|g| g.notes[1].position = Ticks(-1));
    broken(&|g| g.notes[0].velocity = 0);
    broken(&|g| g.notes[0].velocity = 128);
    broken(&|g| g.notes[1].position = g.notes[0].position);
    broken(&|g| g.quantize = 101.0);
    broken(&|g| g.timing = -1.0);
    broken(&|g| g.random = f64::NAN);
    broken(&|g| g.velocity = -100.5);
    assert_eq!(editor.document().settings().grooves.pool.len(), 1);
}

#[test]
fn a_parameter_change_is_one_undo_step_named_for_it() {
    // Spec: grooves R2, R5
    let (mut editor, _, _, groove) = added();
    let set = |base, quantize, timing, random, velocity| Command::SetGroove {
        groove,
        base,
        quantize,
        timing,
        random,
        velocity,
    };
    editor
        .apply(set(Some(GrooveBase::EighthTriplet), None, None, None, None))
        .unwrap();
    editor
        .apply(set(None, Some(40.0), None, None, None))
        .unwrap();
    editor
        .apply(set(None, None, Some(60.0), None, None))
        .unwrap();
    editor
        .apply(set(None, None, None, Some(25.0), None))
        .unwrap();
    editor
        .apply(set(None, None, None, None, Some(-80.0)))
        .unwrap();
    let names: Vec<_> = editor.undo_history().take(5).collect();
    assert_eq!(
        names,
        [
            "Change Velocity",
            "Change Random",
            "Change Timing",
            "Change Quantize",
            "Change Base"
        ]
    );
    let held = editor
        .document()
        .settings()
        .grooves
        .get(groove)
        .unwrap()
        .clone();
    assert_eq!(
        (
            held.base,
            held.quantize,
            held.timing,
            held.random,
            held.velocity
        ),
        (GrooveBase::EighthTriplet, 40.0, 60.0, 25.0, -80.0)
    );
    // One out of range refuses them all.
    assert!(
        editor
            .apply(set(
                Some(GrooveBase::Quarter),
                Some(10.0),
                Some(101.0),
                None,
                None
            ))
            .is_err()
    );
    assert_eq!(
        editor.document().settings().grooves.get(groove).unwrap(),
        &held
    );
    editor.undo().unwrap();
    assert_eq!(
        editor
            .document()
            .settings()
            .grooves
            .get(groove)
            .unwrap()
            .velocity,
        0.0
    );
}

#[test]
fn the_global_amount_runs_from_0_to_130_percent() {
    // Spec: grooves R2, R5
    let (mut editor, _, _, _) = added();
    for amount in [0.0, 130.0, 62.5] {
        editor
            .apply(Command::SetGlobalGrooveAmount { amount })
            .unwrap();
        assert_eq!(editor.document().settings().grooves.global_amount, amount);
    }
    for amount in [-1.0, 130.5, f64::INFINITY] {
        assert!(
            editor
                .apply(Command::SetGlobalGrooveAmount { amount })
                .is_err()
        );
    }
}

#[test]
fn a_clip_has_at_most_one_groove_and_setting_it_is_one_undo_step() {
    // Spec: grooves R3
    let (mut editor, track, first, groove) = added();
    let second: ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                second,
                Ticks::WHOLE,
                Ticks::WHOLE,
                ClipData::Midi(MidiData::default()),
            ),
        })
        .unwrap();
    assert_eq!(editor.document().clip(first).unwrap().groove, None);
    let steps = editor.undo_history().len();
    editor
        .apply(Command::SetClipGroove {
            clips: vec![first, second],
            groove: Some(groove),
        })
        .unwrap();
    assert_eq!(editor.undo_history().len(), steps + 1);
    assert_eq!(editor.document().clip(second).unwrap().groove, Some(groove));
    // A groove the pool doesn't hold, and a clip that isn't there, change nothing.
    let stranger: GrooveId = editor.new_id();
    assert!(matches!(
        editor.apply(Command::SetClipGroove {
            clips: vec![first],
            groove: Some(stranger)
        }),
        Err(EditError::Command(ModelError::NoSuchGroove(_)))
    ));
    let gone: ClipId = editor.new_id();
    assert!(
        editor
            .apply(Command::SetClipGroove {
                clips: vec![first, gone],
                groove: None
            })
            .is_err()
    );
    assert_eq!(editor.document().clip(first).unwrap().groove, Some(groove));
    editor
        .apply(Command::SetClipGroove {
            clips: vec![first],
            groove: None,
        })
        .unwrap();
    assert_eq!(editor.document().clip(first).unwrap().groove, None);
    // A groove clip isn't a clip with the groove's notes: it stays a plain clip.
    assert!(editor.document().clip(first).unwrap().envelopes.is_empty());
}

#[test]
fn a_clip_that_is_split_or_moved_keeps_its_groove() {
    // Spec: grooves R3
    let (mut editor, _, clip, groove) = added();
    editor
        .apply(Command::SetClipGroove {
            clips: vec![clip],
            groove: Some(groove),
        })
        .unwrap();
    editor
        .apply(Command::MoveClip {
            clip,
            track: editor.document().clip_track(clip).unwrap(),
            start: Ticks::WHOLE,
        })
        .unwrap();
    assert_eq!(editor.document().clip(clip).unwrap().groove, Some(groove));
    let part = editor.document().clip(clip).unwrap().part(
        Ticks::WHOLE,
        Ticks::WHOLE + Ticks::QUARTER * 2,
        &Default::default(),
    );
    assert_eq!(part.groove, Some(groove));
}

#[test]
fn a_new_clip_cannot_name_a_groove_the_pool_does_not_hold() {
    // Spec: grooves R3
    let (mut editor, track, _) = song();
    let groove: GrooveId = editor.new_id();
    let clip: ClipId = editor.new_id();
    let mut named = Clip::new(
        clip,
        Ticks::WHOLE,
        Ticks::WHOLE,
        ClipData::Midi(MidiData::default()),
    );
    named.groove = Some(groove);
    assert!(matches!(
        editor.apply(Command::CreateClip { track, clip: named }),
        Err(EditError::Command(ModelError::NoSuchGroove(_)))
    ));
}

#[test]
fn deleting_a_groove_sets_its_clips_to_none_and_leaves_no_auto_load_groove() {
    // Spec: grooves R5, R20
    let (mut editor, _, clip, groove) = added();
    editor
        .apply(Command::SetClipGroove {
            clips: vec![clip],
            groove: Some(groove),
        })
        .unwrap();
    editor
        .apply(Command::SetAutoLoadGroove {
            enabled: Some(true),
            groove: Some(groove),
        })
        .unwrap();
    assert_eq!(
        editor.document().settings().grooves.auto_loaded(),
        Some(groove)
    );
    let steps = editor.undo_history().len();
    editor.apply(Command::DeleteGroove { groove }).unwrap();
    assert_eq!(editor.undo_history().len(), steps + 1);
    let document = editor.document();
    assert!(document.settings().grooves.pool.is_empty());
    assert_eq!(document.clip(clip).unwrap().groove, None);
    assert_eq!(document.settings().grooves.auto_load_groove, None);
    assert!(document.settings().grooves.auto_load, "the switch stays on");
    assert_eq!(document.settings().grooves.auto_loaded(), None);
    document.check_invariants().unwrap();
    editor.undo().unwrap();
    assert_eq!(editor.document().clip(clip).unwrap().groove, Some(groove));
    assert_eq!(
        editor.document().settings().grooves.auto_loaded(),
        Some(groove)
    );
}

#[test]
fn auto_load_names_a_groove_of_the_pool() {
    // Spec: grooves R20
    let (mut editor, _, _, groove) = added();
    let stranger: GrooveId = editor.new_id();
    assert!(
        editor
            .apply(Command::SetAutoLoadGroove {
                enabled: Some(true),
                groove: Some(stranger)
            })
            .is_err()
    );
    editor
        .apply(Command::SetAutoLoadGroove {
            enabled: Some(true),
            groove: Some(groove),
        })
        .unwrap();
    // Switching it off keeps the groove it names.
    editor
        .apply(Command::SetAutoLoadGroove {
            enabled: Some(false),
            groove: None,
        })
        .unwrap();
    let pool = &editor.document().settings().grooves;
    assert_eq!(
        (pool.auto_load, pool.auto_load_groove),
        (false, Some(groove))
    );
    assert_eq!(pool.auto_loaded(), None);
}

#[test]
fn a_set_saves_its_grooves_and_loads_them_back() {
    // Spec: grooves R22
    let (mut editor, _, clip, groove) = added();
    editor
        .apply(Command::SetClipGroove {
            clips: vec![clip],
            groove: Some(groove),
        })
        .unwrap();
    editor
        .apply(Command::SetGlobalGrooveAmount { amount: 80.0 })
        .unwrap();
    editor
        .apply(Command::SetAutoLoadGroove {
            enabled: Some(true),
            groove: Some(groove),
        })
        .unwrap();
    let saved = serde_json::to_value(editor.document()).unwrap();
    let grooves = &saved["settings"]["grooves"];
    assert_eq!(grooves["global_amount"], json!(80.0));
    assert_eq!(grooves["auto_load"], json!(true));
    assert_eq!(grooves["auto_load_groove"], json!(groove));
    assert_eq!(grooves["pool"][0]["name"], json!("Swing"));
    assert_eq!(grooves["pool"][0]["base"], json!("1/16"));
    let loaded: Document = serde_json::from_value(saved).unwrap();
    assert_eq!(
        loaded.settings().grooves,
        editor.document().settings().grooves
    );
    assert_eq!(loaded.clip(clip).unwrap().groove, Some(groove));
}

#[test]
fn a_set_from_before_grooves_loads_with_an_empty_pool_and_writes_none() {
    // Spec: grooves R22
    let (editor, _, clip) = song();
    let saved = serde_json::to_value(editor.document()).unwrap();
    assert!(saved["settings"].get("grooves").is_none());
    assert!(saved["tracks"][0]["clips"][0].get("groove").is_none());
    let loaded: Document = serde_json::from_value(saved).unwrap();
    let pool = &loaded.settings().grooves;
    assert_eq!(
        (
            pool.pool.len(),
            pool.global_amount,
            pool.auto_load,
            pool.auto_load_groove
        ),
        (0, 100.0, false, None)
    );
    assert_eq!(loaded.clip(clip).unwrap().groove, None);
}

#[test]
fn a_clip_naming_a_groove_that_is_not_in_the_pool_loads_with_none() {
    // Spec: grooves R22
    let (editor, _, clip, groove) = added();
    let mut saved = serde_json::to_value(editor.document()).unwrap();
    saved["tracks"][0]["clips"][0]["groove"] = json!(groove);
    saved["settings"]["grooves"]["auto_load_groove"] = json!(groove);
    let loaded: Document = serde_json::from_value(saved.clone()).unwrap();
    assert_eq!(loaded.clip(clip).unwrap().groove, Some(groove));
    saved["settings"]["grooves"]["pool"] = json!([]);
    let loaded: Document = serde_json::from_value(saved).unwrap();
    assert_eq!(loaded.clip(clip).unwrap().groove, None);
    assert_eq!(loaded.settings().grooves.auto_load_groove, None);
}

#[test]
fn a_saved_groove_that_breaks_the_limits_does_not_load() {
    // Spec: grooves R1, R2, R4
    let (editor, _, _, _) = added();
    let saved = serde_json::to_value(editor.document()).unwrap();
    for (field, value) in [
        ("length", json!(0)),
        ("quantize", json!(120.0)),
        ("base", json!("1/3")),
    ] {
        let mut broken = saved.clone();
        broken["settings"]["grooves"]["pool"][0][field] = value;
        assert!(
            serde_json::from_value::<Document>(broken).is_err(),
            "{field}"
        );
    }
    let mut broken = saved.clone();
    broken["settings"]["grooves"]["global_amount"] = json!(131.0);
    assert!(serde_json::from_value::<Document>(broken).is_err());
}

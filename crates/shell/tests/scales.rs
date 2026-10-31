//! The set's current scale (WP-6.7): the scale the Control Bar's scale
//! controls show, which follows the selection and the clips that play, and
//! which the clips made next take. The behaviour is in the scales spec.

use bl_core::time::Ticks;
use bl_model::{
    Clip, ClipData, ClipId, Command, Editor, MidiData, Scale, ScaleName, TrackId, TrackKind,
};
use bl_shell::testing;
use bl_shell::{Shell, files::ProjectFiles, settings::Preferences};
use serde_json::json;

/// A shell with a MIDI track, and the ids of the track and its first clip.
fn shell_with_a_clip() -> (Shell, TrackId, ClipId) {
    let mut shell = Shell::headless();
    let track: TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            json!({
                "id": track,
                "kind": "midi",
                "name": "Keys",
                "parent": null,
                "index": 0,
            }),
        )
        .unwrap();
    let clip: ClipId = shell.new_id();
    shell
        .run(
            "clip.create",
            json!({
                "track": track,
                "clip": {
                    "id": clip,
                    "name": "Riff",
                    "start": 0,
                    "length": 705_600,
                    "offset": 0,
                    "mute": false,
                    "data": { "midi": { "notes": [] } },
                },
            }),
        )
        .unwrap();
    (shell, track, clip)
}

/// The set's current scale.
fn current(shell: &Shell) -> Scale {
    shell.editor().document().settings().scale
}

/// The clip's own scale.
fn scale_of(shell: &Shell, clip: ClipId) -> Scale {
    shell.editor().document().clip(clip).unwrap().scale
}

// Spec: scales R10, R12.
#[test]
fn selecting_a_clip_makes_its_scale_the_current_scale() {
    let (mut shell, _track, clip) = shell_with_a_clip();
    shell
        .run(
            "clip.set_scale",
            json!({ "clips": [clip], "enabled": true, "root": 2, "name": "dorian" }),
        )
        .unwrap();
    assert_eq!(
        current(&shell),
        Scale::default(),
        "setting a clip's scale alone doesn't change the current scale"
    );
    let steps = shell.editor().undo_history().len();
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    assert_eq!(
        current(&shell),
        Scale::new(2, ScaleName::Dorian),
        "the selected clip's scale is the current scale"
    );
    assert_eq!(
        shell.editor().undo_history().len(),
        steps,
        "no undo step: the current scale follows the selection"
    );
}

// Spec: scales R12.
#[test]
fn a_change_to_the_shown_clip_s_scale_changes_the_current_scale() {
    let (mut shell, _track, clip) = shell_with_a_clip();
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    shell
        .run(
            "clip.set_scale",
            json!({ "clips": [clip], "enabled": true, "root": 9, "name": "minor" }),
        )
        .unwrap();
    assert_eq!(current(&shell), Scale::new(9, ScaleName::Minor));
}

// Spec: scales R10, R12.
#[test]
fn an_agent_s_scale_change_changes_the_current_scale() {
    let (mut shell, _track, clip) = shell_with_a_clip();
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    // The agent's path, which runs its steps through the registry rather
    // than through `Shell::run`.
    shell
        .run_document_edits(
            bl_model::Maker::Agent("test".to_owned()),
            "agent step",
            &[(
                "clip.set_scale".to_owned(),
                json!({ "clips": [clip], "enabled": true, "root": 2, "name": "dorian" }),
            )],
        )
        .unwrap();
    assert_eq!(
        current(&shell),
        Scale::new(2, ScaleName::Dorian),
        "the current scale follows the shown clip's scale, however it changed"
    );
}

// Spec: scales R5.
#[test]
fn a_clip_set_scale_without_clips_sets_the_selected_ones() {
    let (mut shell, _track, clip) = shell_with_a_clip();
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    shell
        .run(
            "clip.set_scale",
            json!({ "enabled": true, "root": 2, "name": "dorian" }),
        )
        .unwrap();
    assert_eq!(
        scale_of(&shell, clip),
        Scale::new(2, ScaleName::Dorian),
        "the selected clip's scale is set"
    );
}

// Spec: scales R7, R9.
#[test]
fn every_clip_making_command_takes_the_current_scale() {
    let (mut shell, track, _clip) = shell_with_a_clip();
    shell
        .run(
            "song.set_scale",
            json!({ "enabled": true, "root": 7, "name": "mixolydian" }),
        )
        .unwrap();
    let wanted = Scale::new(7, ScaleName::Mixolydian);

    // A clip made from nothing takes the current scale.
    let made: ClipId = shell.new_id();
    shell
        .run(
            "clip.create",
            json!({
                "track": track,
                "clip": {
                    "id": made,
                    "name": "",
                    "start": 705_600,
                    "length": 705_600,
                    "offset": 0,
                    "mute": false,
                    "data": { "midi": { "notes": [] } },
                },
            }),
        )
        .unwrap();
    assert_eq!(scale_of(&shell, made), wanted, "clip.create");

    // A clip in a Session slot takes it too.
    let scene = shell.editor().document().scenes().next().unwrap().id;
    let slot: ClipId = shell.new_id();
    shell
        .run(
            "clip.create_in_slot",
            json!({
                "track": track,
                "scene": scene,
                "clip": {
                    "id": slot,
                    "name": "",
                    "start": 0,
                    "length": 705_600,
                    "offset": 0,
                    "mute": false,
                    "data": { "midi": { "notes": [] } },
                },
            }),
        )
        .unwrap();
    assert_eq!(scale_of(&shell, slot), wanted, "clip.create_in_slot");

    // The Session View's empty MIDI clip takes it as well, in another slot.
    let other = shell.editor().document().scenes().nth(1).unwrap().id;
    shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": other }),
        )
        .unwrap();
    let empty = shell
        .editor()
        .document()
        .slot(track, other)
        .expect("the slot holds a clip")
        .id;
    assert_eq!(scale_of(&shell, empty), wanted, "an empty MIDI clip");
}

// Spec: scales R7.
#[test]
fn a_copy_keeps_its_source_s_scale() {
    let (mut shell, track, clip) = shell_with_a_clip();
    shell
        .run(
            "clip.set_scale",
            json!({ "clips": [clip], "enabled": true, "root": 4, "name": "lydian" }),
        )
        .unwrap();
    let wanted = Scale::new(4, ScaleName::Lydian);
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();

    // A duplicate keeps it.
    shell.run("arrangement.duplicate", json!({})).unwrap();
    let duplicated = *shell
        .arrangement()
        .selected_clips()
        .first()
        .expect("a copy");
    assert_eq!(scale_of(&shell, duplicated), wanted, "a duplicate");

    // A split keeps it.
    shell
        .run("arrangement.split", json!({ "at": [352_800] }))
        .unwrap();
    let parts: Vec<ClipId> = shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .clips()
        .map(|clip| clip.id)
        .collect();
    assert_eq!(parts.len(), 2, "the clip is split in two");
    for part in parts {
        assert_eq!(scale_of(&shell, part), wanted, "a part");
    }

    // A paste keeps it.
    shell.run("arrangement.copy", json!({})).unwrap();
    shell.run("arrangement.paste", json!({})).unwrap();
    let pasted = *shell
        .arrangement()
        .selected_clips()
        .first()
        .expect("a copy");
    assert_eq!(scale_of(&shell, pasted), wanted, "a paste");
}

// Spec: scales R7. A clip with Scale Mode off is still the same as one made
// from nothing, so a copy has to be given its scale back.
#[test]
fn a_copy_of_a_clip_with_scale_mode_off_keeps_it() {
    let (mut shell, track, clip) = shell_with_a_clip();
    shell
        .run(
            "song.set_scale",
            json!({ "enabled": true, "root": 2, "name": "minor" }),
        )
        .unwrap();
    shell
        .run(
            "clip.set_scale",
            json!({ "clips": [clip], "enabled": false, "root": 0, "name": "major" }),
        )
        .unwrap();
    assert_eq!(scale_of(&shell, clip), Scale::default());

    // A track duplicate.
    shell
        .run("track.duplicate", json!({ "tracks": [track] }))
        .unwrap();
    let doc = shell.editor().document();
    let copy = doc
        .top_level()
        .find(|&other| other != track && doc.track(other).unwrap().clips().count() == 1)
        .expect("the copy of the track");
    let copied = doc.track(copy).unwrap().clips().next().unwrap().id;
    assert_eq!(
        scale_of(&shell, copied),
        Scale::default(),
        "a duplicated track's clip keeps Scale Mode off"
    );

    // A Session duplicate.
    let scene = shell.editor().document().scenes().next().unwrap().id;
    shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    let slot = shell.editor().document().slot(track, scene).unwrap().id;
    shell
        .run(
            "clip.set_scale",
            json!({ "clips": [slot], "enabled": false, "root": 0, "name": "major" }),
        )
        .unwrap();
    shell
        .run("session.select", json!({ "track": track, "scene": scene }))
        .unwrap();
    shell.run("session.duplicate", json!({})).unwrap();
    let doc = shell.editor().document();
    let slots: Vec<&Clip> = doc
        .track(track)
        .unwrap()
        .slots()
        .map(|(_, clip)| clip)
        .collect();
    assert_eq!(slots.len(), 2, "the slot clip was duplicated");
    for clip in slots {
        assert_eq!(clip.scale, Scale::default(), "a duplicated slot clip");
    }
}

// Spec: scales R11.
#[test]
fn a_launched_clip_makes_its_scale_the_current_scale() {
    let (mut shell, control) = testing::rig();
    shell.start_audio();
    assert!(shell.audio().engine_running());
    let track = shell
        .mixer_columns()
        .into_iter()
        .find(|track| {
            shell
                .editor()
                .document()
                .track(*track)
                .is_some_and(|track| track.kind() == TrackKind::Midi)
        })
        .expect("the default set has a MIDI track");
    let scene = shell.editor().document().scenes().next().unwrap().id;
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
        .expect("the slot holds a clip")
        .id;
    shell
        .run(
            "clip.set_scale",
            json!({ "clips": [clip], "enabled": true, "root": 3, "name": "phrygian" }),
        )
        .unwrap();
    assert_eq!(current(&shell), Scale::default());

    // The launch, paced by hand, so the test doesn't wait on the wall clock.
    shell
        .run("session.launch", json!({ "track": track, "scene": scene }))
        .unwrap();
    testing::run_blocks(&control, &mut shell, 4);
    assert_eq!(
        current(&shell),
        Scale::new(3, ScaleName::Phrygian),
        "the launched clip's scale is the current scale"
    );
    assert!(
        shell.editor().undo_history().next() != Some("Change Scale"),
        "no undo step: the current scale follows the launch"
    );
}

// Spec: scales R11.
#[test]
fn the_playhead_entering_a_clip_makes_its_scale_the_current_scale() {
    let (mut shell, control) = testing::rig();
    shell.start_audio();
    let track: TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            json!({
                "id": track,
                "kind": "midi",
                "name": "Keys",
                "parent": null,
                "index": 0,
            }),
        )
        .unwrap();
    // Two clips in a row: the first in C Major, the second in D Dorian.
    for (at, name) in [(0, "major"), (705_600, "dorian")] {
        let clip: ClipId = shell.new_id();
        shell
            .run(
                "clip.create",
                json!({
                    "track": track,
                    "clip": {
                        "id": clip,
                        "name": "",
                        "start": at,
                        "length": 705_600,
                        "offset": 0,
                        "mute": false,
                        "data": { "midi": { "notes": [] } },
                    },
                }),
            )
            .unwrap();
        shell
            .run(
                "clip.set_scale",
                json!({ "clips": [clip], "enabled": true, "root": if name == "major" { 0 } else { 2 }, "name": name }),
            )
            .unwrap();
    }
    assert_eq!(current(&shell), Scale::default());
    // The engine takes the set on a thread of its own, and reports the
    // transport as stopped until a block has run with it: without this wait
    // the eight blocks below can finish before the set arrives.
    testing::settle(&control, &mut shell);

    // The transport plays from the start: the playhead enters the first
    // clip, and then the second.
    shell
        .run("transport.locate", json!({ "position": 0 }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    testing::run_blocks(&control, &mut shell, 8);
    assert_eq!(
        current(&shell),
        Scale::new(0, ScaleName::Major),
        "the first clip's scale is the current scale"
    );
    testing::advance(&control, &mut shell, 1_500);
    assert_eq!(
        current(&shell),
        Scale::new(2, ScaleName::Dorian),
        "the second clip's scale is the current scale"
    );
    assert!(
        shell.editor().undo_history().next() != Some("Change Scale"),
        "no undo step: the current scale follows the playhead"
    );
}

// Spec: scales R9, R13.
#[test]
fn the_scale_list_is_read_by_its_id() {
    let mut shell = Shell::headless();
    let list = shell.run("song.scales", json!({})).unwrap();
    let scales = list["scales"].as_array().expect("the scales");
    assert_eq!(scales.len(), ScaleName::ALL.len());
    assert_eq!(scales[0]["name"], "Major");
    assert_eq!(scales[0]["steps"], json!([0, 2, 4, 5, 7, 9, 11]));
}

/// The shell's own set, saved and opened again, keeps every scale.
#[test]
fn a_set_saved_and_opened_again_keeps_its_scales() {
    let (mut shell, _track, clip) = shell_with_a_clip();
    shell
        .run(
            "clip.set_scale",
            json!({ "clips": [clip], "enabled": true, "root": 2, "name": "dorian" }),
        )
        .unwrap();
    shell
        .run(
            "song.set_scale",
            json!({ "enabled": true, "root": 5, "name": "minor" }),
        )
        .unwrap();
    let path = std::env::temp_dir().join(format!(
        "bl-scales-{}-{}.blset",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let result = shell
        .run("set.save_as", json!({ "path": path.to_string_lossy() }))
        .unwrap();
    let saved = std::path::PathBuf::from(result["path"].as_str().expect("the file's path"));
    let mut again = Shell::new(
        Preferences::default(),
        None,
        bl_shell::audio::AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    again
        .run("set.open", json!({ "path": saved.to_string_lossy() }))
        .unwrap();
    assert_eq!(scale_of(&again, clip), Scale::new(2, ScaleName::Dorian));
    assert_eq!(current(&again), Scale::new(5, ScaleName::Minor));
}

/// A clip the model made carries the current scale, whichever command made
/// it: the model's one place (the scales spec, R7).
#[test]
fn a_clip_made_by_the_model_takes_the_current_scale() {
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
    editor
        .apply(Command::SetScale {
            enabled: Some(true),
            root: Some(11),
            name: Some("locrian".to_owned()),
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
    assert_eq!(
        editor.document().clip(clip).unwrap().scale,
        Scale::new(11, ScaleName::Locrian)
    );
}

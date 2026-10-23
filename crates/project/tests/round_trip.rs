//! Save and load round trips (WP-0.7 acceptance).
//!
//! Random projects (tracks and groups, clips with notes, devices with
//! parameter values, tempo and time-signature maps, media tables, and ID
//! allocators) must come back from a save exactly as they went in: equal as
//! values, and identical as JSON, so even the sign of a zero and the last
//! bit of every f64 survive.
//!
//! `BL_TEST_DEPTH=full` (pushes to main, manual CI runs, and the nightly run)
//! runs 1,000 encoding cases and 100 saving ones; `quick`, the default for
//! local runs and pull requests, runs a tenth of each.

mod common;

use std::process::Command;

use bl_model::{Command as Edit, Editor, TrackKind};
use bl_project::{
    LoadOptions, Media, Project, SCHEMA_VERSION, SaveOptions, SetFile, decode, decode_json, encode,
    encode_json, load,
};
use common::{TestDir, depth, recipe};
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

fn config(cases: u32) -> ProptestConfig {
    ProptestConfig {
        cases,
        failure_persistence: Some(Box::new(FileFailurePersistence::WithSource(
            "proptest-regressions",
        ))),
        ..ProptestConfig::default()
    }
}

/// Checks a loaded project against the one saved: equal, and the same JSON.
fn same(loaded: &Project, saved: &Project, revision: u64) -> Result<(), TestCaseError> {
    prop_assert_eq!(loaded, saved);
    prop_assert_eq!(
        encode_json(loaded, revision).unwrap(),
        encode_json(saved, revision).unwrap()
    );
    Ok(())
}

proptest! {
    #![proptest_config(config(depth(1_000, 100)))]

    /// Encoding then decoding gives the project back, and encoding is
    /// deterministic.
    #[test]
    fn encode_then_decode(recipe in recipe(60), revision in 0..u64::MAX) {
        let project = recipe.build();
        let bytes = encode(&project, revision).unwrap();
        let (loaded, info) = decode(&bytes, &LoadOptions::default()).unwrap();
        same(&loaded, &project, revision)?;
        prop_assert_eq!(info.revision, revision);
        prop_assert_eq!(info.schema, SCHEMA_VERSION);
        prop_assert_eq!(encode(&loaded, revision).unwrap(), bytes);
    }
}

proptest! {
    #![proptest_config(config(depth(100, 10)))]

    /// The acceptance property: saving to disk and loading gives the project
    /// back, save after save, with the revision counting up.
    #[test]
    fn save_then_load(recipes in prop::collection::vec(recipe(40), 1..4)) {
        let dir = TestDir::new("round-trip");
        let path = dir.path().join("Song Project").join("Song.blset");
        let mut set = SetFile::new(&path);
        for (saves, recipe) in (1..).zip(&recipes) {
            let project = recipe.build();
            let report = set.save(&project).unwrap();
            prop_assert!(report.warnings.is_empty(), "{:?}", report.warnings);
            prop_assert_eq!(report.info.revision, saves);

            let (opened, loaded) = SetFile::open(&path).unwrap();
            same(&loaded, &project, saves)?;
            prop_assert_eq!(opened.revision(), saves);
            prop_assert_eq!(opened.info(), set.info());

            // The editor picks up where the saved one left off.
            let mut editor = loaded.editor();
            let next: bl_core::id::Id = editor.new_id();
            prop_assert_eq!(next, project.ids.peek());
        }
        // The project's own folder holds the set and its backups, nothing
        // else.
        let backups = std::fs::read_dir(dir.path().join("Song Project").join("Backup"))
            .map_or(0, Iterator::count);
        prop_assert_eq!(backups, recipes.len() - 1);
    }
}

/// A scene whose Clip Stop button was removed can be deleted, and the set
/// still saves and loads (WP-3.2): the removal goes with its scene.
#[test]
fn a_deleted_scene_takes_its_removed_stop_button_with_it() {
    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Edit::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: "Slots".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let scene = editor
        .document()
        .scenes()
        .next()
        .expect("a new set opens with scenes")
        .id;
    editor
        .apply(Edit::SetSlotStopButton {
            track,
            scene,
            stop_button: false,
        })
        .unwrap();
    editor.apply(Edit::DeleteScene { scene }).unwrap();
    assert_eq!(
        editor
            .document()
            .track(track)
            .expect("the track stays")
            .removed_stop_buttons()
            .count(),
        0,
        "the removed button goes with the scene"
    );
    let project = Project::from_editor(&editor, Media::new());
    let json = encode_json(&project, 1).unwrap();
    decode_json(&json, &LoadOptions::default()).expect("the set opens again");
}

/// A warped clip saves and loads with every warp field (the warping spec,
/// R11), and an unwarped clip's JSON holds none of them.
#[test]
fn a_warped_clip_keeps_its_warp_map_through_a_save_and_load() {
    use bl_core::time::{Ticks, TimeSignature};
    use bl_model::{AudioData, Clip, ClipData, ContentHash, WarpMarker};

    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Edit::CreateTrack {
            id: track,
            kind: TrackKind::Audio,
            name: "Loop".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let (plain, warped) = (editor.new_id(), editor.new_id());
    for (id, start) in [(plain, Ticks::ZERO), (warped, Ticks::WHOLE * 2)] {
        editor
            .apply(Edit::CreateClip {
                track,
                clip: Clip::new(
                    id,
                    start,
                    Ticks::WHOLE,
                    ClipData::Audio(AudioData::new(ContentHash([5; 32]))),
                ),
            })
            .unwrap();
    }
    editor
        .apply(Edit::SetClipWarp {
            clip: warped,
            warp: true,
        })
        .unwrap();
    editor
        .apply(Edit::InsertWarpMarker {
            clips: vec![warped],
            beat: Some(Ticks::WHOLE),
            time: None,
            pin_neighbours: false,
            transients: None,
        })
        .unwrap();
    // Transient edits (WP-4.3) are saved with the clip too.
    editor
        .apply(Edit::InsertTransient {
            clip: warped,
            time: 0.3,
        })
        .unwrap();
    editor
        .apply(Edit::DeleteTransients {
            clip: warped,
            times: vec![0.7],
        })
        .unwrap();
    // A signature has no command yet; the data carries it.
    let ClipData::Audio(mut audio) = editor.document().clip(warped).unwrap().data.clone() else {
        unreachable!("an audio clip");
    };
    audio.signature = Some(TimeSignature::new(7, 8).unwrap());
    editor
        .apply(Edit::SetClipData {
            clip: warped,
            data: ClipData::Audio(audio.clone()),
        })
        .unwrap();
    // The warp mode, its controls, and the RAM and HiQ switches (WP-4.2).
    editor
        .apply(Edit::SetClipWarpMode {
            clip: warped,
            mode: bl_model::WarpMode::Tones,
        })
        .unwrap();
    editor
        .apply(Edit::SetClipWarpSettings {
            clip: warped,
            beats: None,
            tones: Some(bl_model::TonesUpdate {
                grain_size: Some(45.0),
            }),
            texture: None,
            complex_pro: None,
        })
        .unwrap();
    editor
        .apply(Edit::SetClipRamMode {
            clip: warped,
            ram: true,
        })
        .unwrap();
    editor
        .apply(Edit::SetClipHighQuality {
            clip: warped,
            high_quality: false,
        })
        .unwrap();
    let ClipData::Audio(audio) = editor.document().clip(warped).unwrap().data.clone() else {
        unreachable!("an audio clip");
    };
    let project = Project::from_editor(&editor, Media::new());
    let json = encode_json(&project, 1).unwrap();
    let text = String::from_utf8(json.clone()).unwrap();
    assert_eq!(
        text.matches("\"warp\":true").count(),
        1,
        "only the warped clip"
    );
    for saved in [
        "warp_mode",
        "warp_settings",
        "\"ram\":true",
        "\"high_quality\":false",
    ] {
        assert_eq!(
            text.matches(saved).count(),
            1,
            "{saved}: only the warped clip"
        );
    }
    assert_eq!(text.matches("\"warp_markers\"").count(), 1);
    assert_eq!(text.matches("\"transient_edits\"").count(), 1);
    let (loaded, _) = decode_json(&json, &LoadOptions::default()).unwrap();
    assert_eq!(loaded, project);
    let ClipData::Audio(back) = &loaded.document.clip(warped).unwrap().data else {
        unreachable!("an audio clip");
    };
    assert_eq!(*back, audio);
    assert_eq!(back.transient_edits.added, [0.3]);
    assert_eq!(back.transient_edits.removed, [0.7]);
    assert_eq!(
        back.warp_markers[1],
        WarpMarker {
            time: audio.warp_markers[1].time,
            beat: Ticks::WHOLE
        }
    );
}

/// A song the size of the UI benchmark (200 tracks, 5,000 clips) saves and
/// loads exactly. Prints the sizes and times for the pull request.
#[test]
fn a_large_song_round_trips() {
    let project = common::large_song(200, 25);
    let dir = TestDir::new("large");
    let path = dir.path().join("Large.blset");

    let started = std::time::Instant::now();
    let mut set = SetFile::new(&path);
    set.save_with(&project, &SaveOptions::default()).unwrap();
    let saved = started.elapsed();
    let started = std::time::Instant::now();
    let (loaded, _) = load(&path).unwrap();
    let loaded_in = started.elapsed();
    assert_eq!(loaded, project);

    let json = encode_json(&project, 1).unwrap().len();
    let file = std::fs::metadata(&path).unwrap().len();
    println!(
        "{} tracks, {} clips, {} devices: {json} bytes of JSON, {file} bytes on disk; saved in {saved:?}, loaded in {loaded_in:?} ({} build)",
        project.document.track_count(),
        project.document.clip_count(),
        project.document.device_count(),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
    );
    assert_eq!(project.document.clip_count(), 5_000);
}

/// A set is a standard zstd stream: the reference `zstd` tool decompresses
/// it to the JSON. Skipped where `zstd` isn't installed.
#[test]
fn the_zstd_tool_reads_sets() {
    let Ok(output) = Command::new("zstd").arg("--version").output() else {
        println!("skipped: the zstd tool isn't installed");
        return;
    };
    assert!(output.status.success());

    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    let mut runner = TestRunner::deterministic();
    let project = recipe(200).new_tree(&mut runner).unwrap().current().build();
    let dir = TestDir::new("zstd-tool");
    let path = dir.path().join("Song.blset");
    std::fs::write(&path, encode(&project, 5).unwrap()).unwrap();

    let output = Command::new("zstd")
        .args(["--decompress", "--stdout", "--check"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, encode_json(&project, 5).unwrap());
    println!(
        "zstd {} decompressed a {}-byte set",
        String::from_utf8_lossy(
            &Command::new("zstd")
                .arg("--version")
                .output()
                .unwrap()
                .stdout
        )
        .trim(),
        std::fs::metadata(&path).unwrap().len()
    );
}

// Spec: comping R3. Each take-lane command is one undo step, and a set
// saved after any of them loads back as it was, with the lanes, their clips
// and the audition.
#[test]
fn take_lanes_and_their_commands_survive_a_save_and_a_load() {
    use bl_core::time::Ticks;
    use bl_model::{
        AudioData, Clip, ClipData, ContentHash, NewTakeLane, TakeClip, TakeClipData, TakeLaneId,
        TrackId,
    };
    let mut editor = Editor::new();
    let track: TrackId = editor.new_id();
    editor
        .apply(Edit::CreateTrack {
            id: track,
            kind: TrackKind::Audio,
            name: "Guitar".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let steps = |editor: &Editor| editor.undo_history().len();
    let check = |editor: &Editor| {
        let project = Project::from_editor(editor, Media::new());
        let bytes = encode(&project, 1).unwrap();
        let (loaded, _) = decode(&bytes, &LoadOptions::default()).unwrap();
        assert_eq!(loaded, project);
        assert_eq!(
            encode_json(&loaded, 1).unwrap(),
            encode_json(&project, 1).unwrap()
        );
    };
    let lane: TakeLaneId = editor.new_id();
    let first = steps(&editor);
    editor
        .apply(Edit::AddTakeLane {
            track,
            lane: NewTakeLane {
                id: Some(lane),
                name: "Take 1".to_owned(),
                clips: vec![TakeClip {
                    id: None,
                    name: "Pass".to_owned(),
                    start: Ticks::ZERO,
                    length: Ticks::WHOLE,
                    data: TakeClipData::Audio(AudioData::new(ContentHash([5; 32]))),
                }],
            },
            index: None,
        })
        .unwrap();
    assert_eq!(steps(&editor), first + 1);
    check(&editor);
    let clip = Clip::new(
        editor.new_id(),
        Ticks::WHOLE,
        Ticks::WHOLE,
        ClipData::Audio(AudioData::new(ContentHash([6; 32]))),
    );
    for command in [
        Edit::CreateTakeClip { lane, clip },
        Edit::RenameTakeLane {
            lane,
            name: "Verse".to_owned(),
        },
        Edit::SetAudition { lane, on: true },
    ] {
        let before = steps(&editor);
        editor.apply(command).unwrap();
        assert_eq!(steps(&editor), before + 1);
        check(&editor);
    }
    let second: TakeLaneId = editor.new_id();
    editor
        .apply(Edit::AddTakeLane {
            track,
            lane: NewTakeLane {
                id: Some(second),
                name: "Take 2".to_owned(),
                clips: Vec::new(),
            },
            index: None,
        })
        .unwrap();
    for command in [
        Edit::MoveTakeLanes {
            lanes: vec![second],
            index: 0,
        },
        Edit::DeleteUnusedTakeLanes {
            tracks: vec![track],
        },
    ] {
        let before = steps(&editor);
        editor.apply(command).unwrap();
        assert_eq!(steps(&editor), before + 1);
        check(&editor);
    }
    assert!(editor.document().take_lane(lane).is_none());
    editor.undo().unwrap();
    check(&editor);
    assert!(editor.document().take_lane(lane).is_some());
}

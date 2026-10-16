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

//! The colours of what the shell makes new (WP-6.30b; the themes spec, R19,
//! R20, R23, R24), driven through `Shell::invoke` and `Shell::run` as a
//! person and an agent drive them.

use bl_model::{ClipId, SceneId, TrackId};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::{ClipColour, Preferences, ThemeLook};
use serde_json::{Value, json};

fn shell() -> Shell {
    Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    )
}

fn settings(shell: &mut Shell, id: &str, args: Value) {
    shell.run(id, args).unwrap();
}

fn steps(shell: &Shell) -> usize {
    shell.editor().undo_history().len()
}

fn colour(shell: &Shell, track: TrackId) -> u8 {
    shell.editor().document().track(track).unwrap().colour()
}

/// Inserts an audio track as the Insert Audio Track key does, and returns it
/// and the undo steps it took.
fn insert(shell: &mut Shell) -> (TrackId, usize) {
    let before = steps(shell);
    shell
        .invoke("track.create", json!({ "kind": "audio" }))
        .unwrap();
    // The new track is selected.
    let track = shell.arrangement().selected_tracks()[0];
    (track, steps(shell) - before)
}

// Spec: themes R23. In a new install a new track takes swatch `id mod 14`
// (Auto-Assign Track Colors on, Reduced Automatic Colors off), in one undo
// step with the track.
#[test]
fn new_tracks_take_swatch_id_mod_14_in_a_new_install() {
    let mut shell = shell();
    for _ in 0..3 {
        let (track, taken) = insert(&mut shell);
        assert_eq!(u64::from(colour(&shell, track)), track.0.get() % 14);
        assert_eq!(taken, 1, "the track and its colour are one step");
    }
}

// Spec: themes R23, R24. With Auto-Assign Track Colors off every new track
// takes Default Track Color; with it on, Reduced Automatic Colors cycles the
// top row's even-numbered swatches, 0 to 12.
#[test]
fn the_settings_choose_what_new_tracks_take() {
    let mut shell = shell();
    settings(
        &mut shell,
        "settings.set_auto_assign_track_colours",
        json!({ "on": false }),
    );
    for _ in 0..2 {
        let (track, taken) = insert(&mut shell);
        assert_eq!(
            colour(&shell, track),
            12,
            "Default Track Color, 12 in a new install"
        );
        assert_eq!(taken, 1);
    }
    settings(
        &mut shell,
        "settings.set_default_track_colour",
        json!({ "colour": 58 }),
    );
    let (track, taken) = insert(&mut shell);
    assert_eq!(colour(&shell, track), 58);
    assert_eq!(
        taken, 1,
        "a colour other than the track's own is still one step"
    );

    settings(
        &mut shell,
        "settings.set_auto_assign_track_colours",
        json!({ "on": true }),
    );
    settings(
        &mut shell,
        "settings.set_reduced_automatic_colours",
        json!({ "on": true }),
    );
    for _ in 0..8 {
        let (track, _) = insert(&mut shell);
        assert_eq!(
            u64::from(colour(&shell, track)),
            track.0.get() % 7 * 2,
            "the seven even-numbered swatches"
        );
        assert_eq!(colour(&shell, track) % 2, 0);
    }
    // The setting reads back through settings.state.
    let state = shell.run("settings.state", json!({})).unwrap();
    let colours = &state["theme_colors"];
    assert_eq!(colours["auto_assign_track_colours"], json!(true));
    assert_eq!(colours["default_track_colour"], json!(58));
    assert_eq!(colours["reduced_automatic_colours"], json!(true));
    assert_eq!(colours["clip_colour"], json!("track-color"));
}

// Spec: themes R19. A track made through the agent API takes the same
// colour a key's does, and a swatch outside the palette is refused.
#[test]
fn an_agent_made_track_is_coloured_too() {
    let mut shell = shell();
    settings(
        &mut shell,
        "settings.set_auto_assign_track_colours",
        json!({ "on": false }),
    );
    settings(
        &mut shell,
        "settings.set_default_track_colour",
        json!({ "colour": 41 }),
    );
    let id: TrackId = shell.new_id();
    let index = shell.editor().document().top_level().len();
    shell
        .run(
            "track.create",
            json!({ "id": id, "kind": "midi", "name": "Lead", "index": index }),
        )
        .unwrap();
    assert_eq!(colour(&shell, id), 41);
    assert!(
        shell
            .run("settings.set_default_track_colour", json!({ "colour": 70 }))
            .is_err()
    );
}

// Spec: themes R19. Group Tracks makes a group that takes the colour the
// settings give, and the tracks it puts in keep theirs.
#[test]
fn a_new_group_is_coloured_and_its_tracks_keep_their_colours() {
    let mut shell = shell();
    let tracks: Vec<TrackId> = shell.editor().document().top_level().take(2).collect();
    let before: Vec<u8> = tracks.iter().map(|t| colour(&shell, *t)).collect();
    settings(
        &mut shell,
        "settings.set_auto_assign_track_colours",
        json!({ "on": false }),
    );
    settings(
        &mut shell,
        "settings.set_default_track_colour",
        json!({ "colour": 33 }),
    );
    let steps_before = steps(&shell);
    shell
        .run("track.group", json!({ "tracks": tracks }))
        .unwrap();
    let group = shell
        .editor()
        .document()
        .parent(tracks[0])
        .expect("the tracks are in a group");
    assert_eq!(colour(&shell, group), 33);
    let after: Vec<u8> = tracks.iter().map(|t| colour(&shell, *t)).collect();
    assert_eq!(after, before);
    assert_eq!(steps(&shell) - steps_before, 1);
}

fn first_midi_track(shell: &Shell) -> TrackId {
    let doc = shell.editor().document();
    doc.top_level()
        .find(|t| doc.track(*t).unwrap().kind() == bl_model::TrackKind::Midi)
        .expect("the default set has a MIDI track")
}

fn empty_clip(shell: &mut Shell, track: TrackId, scene: SceneId) -> ClipId {
    shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    shell.editor().document().slot(track, scene).unwrap().id
}

// Spec: themes R20, R23. A new clip takes its track's colour with Clip
// Color on Track Color, and swatch `h mod 70` of a hash of its ID with it on
// Random, so the same clip always gets the same swatch.
#[test]
fn a_new_clip_takes_the_colour_clip_color_gives() {
    let mut shell = shell();
    let track = first_midi_track(&shell);
    let scenes: Vec<SceneId> = shell.editor().document().scenes().map(|s| s.id).collect();
    let before = steps(&shell);
    let plain = empty_clip(&mut shell, track, scenes[0]);
    assert_eq!(steps(&shell) - before, 1);
    let own = colour(&shell, track);
    assert_eq!(
        shell.editor().document().clip(plain).unwrap().colour,
        Some(own),
        "Track Color"
    );

    settings(
        &mut shell,
        "settings.set_clip_colour",
        json!({ "clip_colour": "random" }),
    );
    let look = ThemeLook {
        clip_colour: ClipColour::Random,
        ..ThemeLook::default()
    };
    let mut seen = std::collections::BTreeSet::new();
    for scene in &scenes[1..] {
        let before = steps(&shell);
        let clip = empty_clip(&mut shell, track, *scene);
        assert_eq!(
            steps(&shell) - before,
            1,
            "the clip and its colour are one step"
        );
        let swatch = shell
            .editor()
            .document()
            .clip(clip)
            .unwrap()
            .colour
            .unwrap();
        assert!(swatch < 70);
        assert_eq!(Some(swatch), look.new_clip_colour(clip));
        seen.insert(swatch);
    }
    assert!(seen.len() > 1, "the swatches vary: {seen:?}");
}

// Spec: themes R19, R20. A copy keeps the colours of what it copies: a
// duplicated track its own, a copied clip its own.
#[test]
fn copies_keep_their_colours() {
    let mut shell = shell();
    let track = first_midi_track(&shell);
    shell
        .run(
            "track.set_colour",
            json!({ "tracks": [track], "colour": 47 }),
        )
        .unwrap();
    let scenes: Vec<SceneId> = shell.editor().document().scenes().map(|s| s.id).collect();
    let clip = empty_clip(&mut shell, track, scenes[0]);
    shell
        .run("clip.set_colour", json!({ "clips": [clip], "colour": 9 }))
        .unwrap();
    // The copy is made while Auto-Assign would give another colour.
    settings(
        &mut shell,
        "settings.set_auto_assign_track_colours",
        json!({ "on": false }),
    );
    settings(
        &mut shell,
        "settings.set_clip_colour",
        json!({ "clip_colour": "random" }),
    );

    shell
        .run(
            "session.copy_clip_to_slot",
            json!({ "clip": clip, "track": track, "scene": scenes[1] }),
        )
        .unwrap();
    let copy = shell.editor().document().slot(track, scenes[1]).unwrap().id;
    assert_eq!(
        shell.editor().document().clip(copy).unwrap().colour,
        Some(9)
    );

    shell
        .run("track.duplicate", json!({ "tracks": [track] }))
        .unwrap();
    let doc = shell.editor().document();
    let duplicate = doc
        .top_level()
        .find(|t| *t != track && doc.track(*t).unwrap().colour() == 47)
        .expect("the duplicate keeps the track's colour");
    assert_ne!(duplicate, track);
}

/// A Standard MIDI File with one note, C3 for a quarter note.
const ONE_NOTE: [u8; 34] = [
    b'M', b'T', b'h', b'd', 0, 0, 0, 6, 0, 0, 0, 1, 0, 96, //
    b'M', b'T', b'r', b'k', 0, 0, 0, 12, //
    0, 0x90, 60, 100, 96, 0x80, 60, 0, 0, 0xFF, 0x2F, 0,
];

// Spec: themes R19, R20. A MIDI file imported to a new track makes a track
// and a clip that take the colours the settings give, in the import's one
// step.
#[test]
fn an_imported_midi_file_is_coloured() {
    let mut shell = shell();
    settings(
        &mut shell,
        "settings.set_auto_assign_track_colours",
        json!({ "on": false }),
    );
    settings(
        &mut shell,
        "settings.set_default_track_colour",
        json!({ "colour": 21 }),
    );
    settings(
        &mut shell,
        "settings.set_clip_colour",
        json!({ "clip_colour": "random" }),
    );
    let path = std::env::temp_dir().join(format!("bl-colours-{}.mid", std::process::id()));
    std::fs::write(&path, ONE_NOTE).unwrap();
    let before = steps(&shell);
    let result = shell
        .run("midi.import_file", json!({ "path": path, "at": 0 }))
        .unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(steps(&shell) - before, 1);
    let track: TrackId = serde_json::from_value(result["created_tracks"][0].clone()).unwrap();
    let clip: ClipId = serde_json::from_value(result["clips"][0]["clip"].clone()).unwrap();
    assert_eq!(colour(&shell, track), 21);
    let look = ThemeLook {
        clip_colour: ClipColour::Random,
        ..ThemeLook::default()
    };
    assert_eq!(
        shell.editor().document().clip(clip).unwrap().colour,
        look.new_clip_colour(clip)
    );
}

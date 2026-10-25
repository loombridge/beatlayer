//! Grooves in the app shell (WP-6.19): the commands that load, save,
//! extract, commit and make clips of grooves, the pool query, and a new
//! MIDI clip taking the auto-load groove. Spec: grooves R3, R5, R16 to R20
//! and R23. WP-6.20: the pool's view, hot-swapping, the browser's Grooves
//! label, the bundled grooves and the factory default set (R24, R31 to R33,
//! R36 to R38).

use std::path::PathBuf;
use std::sync::Arc;

use bl_core::time::{TICKS_PER_QUARTER, Ticks};
use bl_engine::testing::{self, Probe};
use bl_engine::{EngineConfig, NoteKind, Offline, TransportCommand};
use bl_model::{
    ClipData, ClipId, Device, DeviceKind, Document, GrooveId, MidiData, Note, TrackId, TrackKind,
    clip_seed,
};
use bl_shell::audio::AudioHost;
use bl_shell::browser_commands::{self as browser_ids, BrowserState, ItemKind};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::{Area, Effect, Shell};
use serde_json::{Value, json};

const BAR: i64 = TICKS_PER_QUARTER * 4;
const STEP: i64 = TICKS_PER_QUARTER / 4;

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-grooves-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Takes the factory default set's groove out of the shell's set, and puts the
/// pool's other settings back as a set made before grooves has them: an empty
/// pool, Auto Load Groove off, a Global Amount of 100 %. These tests build the
/// pool they use.
fn plain_pool(shell: &mut Shell) {
    let first = shell.editor().document().settings().grooves.pool[0].id;
    shell
        .run("groove.set_auto_load", json!({ "enabled": false }))
        .unwrap();
    shell
        .run("groove.delete", json!({ "groove": first }))
        .unwrap();
    shell
        .run("groove.set_global_amount", json!({ "amount": 100.0 }))
        .unwrap();
}

/// A shell with a MIDI track holding the test instrument and a clip of
/// `notes`, looping over the bar when `looped`.
fn midi_shell(notes: &[Note], looped: bool) -> (Shell, TrackId, ClipId) {
    let mut shell = Shell::headless();
    plain_pool(&mut shell);
    let track: TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            json!({ "id": track, "kind": "midi", "name": "Keys", "parent": null, "index": 0 }),
        )
        .unwrap();
    let instrument = Device::new(
        shell.new_id(),
        "",
        DeviceKind::Native {
            device_type: testing::INSTRUMENT.to_owned(),
        },
    );
    shell
        .run(
            "device.insert",
            json!({ "track": track, "index": 0, "device": instrument }),
        )
        .unwrap();
    let clip: ClipId = shell.new_id();
    let mut data = MidiData::new(notes.iter().cloned());
    if looped {
        data.looping = true;
        data.loop_end = Some(Ticks(BAR));
    }
    shell
        .run(
            "clip.create",
            json!({ "track": track, "clip": {
                "id": clip, "name": "Riff", "start": 0, "length": BAR * 2,
                "offset": 0, "mute": false, "data": { "midi": data } } }),
        )
        .unwrap();
    (shell, track, clip)
}

/// Adds a groove of a bar of sixteenths, each at its line's offset (an
/// eighth of a step later on every second line), loud on the beats.
fn add_swing(shell: &mut Shell) -> GrooveId {
    let id: GrooveId = shell.new_id();
    let notes: Vec<Value> = (0..16)
        .map(|i| {
            json!({
                "position": i * STEP + if i % 2 == 1 { STEP / 8 } else { 0 },
                "velocity": if i % 4 == 0 { 127 } else { 65 },
            })
        })
        .collect();
    shell
        .run(
            "groove.add",
            json!({ "groove": { "id": id, "name": "Swing", "length": BAR, "notes": notes } }),
        )
        .unwrap();
    id
}

fn pool(shell: &mut Shell) -> Value {
    shell.run("groove.pool", json!({})).unwrap()
}

fn clip(shell: &Shell, id: ClipId) -> bl_model::Clip {
    shell.editor().document().clip(id).unwrap().clone()
}

fn steps(shell: &Shell) -> usize {
    shell.editor().undo_history().len()
}

fn sixteenths(velocity: u8) -> Vec<Note> {
    (0..16)
        .map(|i| Note::new(60, Ticks(i * STEP + STEP / 16), Ticks(STEP / 2), velocity))
        .collect()
}

// Spec: grooves R5, R23. Every groove command runs by ID, and the pool query
// reads the pool, each groove's parameters, whether it is active and the
// clips that use it, the Global Amount and Auto Load Groove.
#[test]
fn every_groove_command_runs_by_id_and_the_pool_reads_back() {
    let scratch = Scratch::new("by-id");
    let (mut shell, track, clip_id) = midi_shell(&sixteenths(100), true);
    shell.set_user_library(Some(scratch.0.join("Library")));
    assert_eq!(
        pool(&mut shell),
        json!({
            "view": { "shown": false },
            "grooves": [],
            "global_amount": 100.0,
            "auto_load": false
        })
    );
    let swing = add_swing(&mut shell);
    shell
        .run(
            "groove.rename",
            json!({ "groove": swing, "name": "Swing 16ths" }),
        )
        .unwrap();
    shell
        .run(
            "groove.set",
            json!({ "groove": swing, "base": "1/8", "quantize": 25.0, "timing": 80.0,
                    "random": 10.0, "velocity": -30.0 }),
        )
        .unwrap();
    shell
        .run("groove.set_global_amount", json!({ "amount": 120.0 }))
        .unwrap();
    shell
        .run(
            "clip.set_groove",
            json!({ "clips": [clip_id], "groove": swing }),
        )
        .unwrap();
    shell
        .run(
            "groove.set_auto_load",
            json!({ "enabled": true, "groove": swing }),
        )
        .unwrap();
    let state = pool(&mut shell);
    let held = &state["grooves"][0];
    assert_eq!(held["name"], "Swing 16ths");
    assert_eq!(held["base"], "1/8");
    for (field, value) in [
        ("quantize", 25.0),
        ("timing", 80.0),
        ("random", 10.0),
        ("velocity", -30.0),
    ] {
        assert_eq!(held[field], json!(value), "{field}");
    }
    assert_eq!(held["active"], true);
    assert_eq!(held["clips"], json!([clip_id]));
    assert_eq!(held["notes"].as_array().unwrap().len(), 16);
    assert_eq!(state["global_amount"], json!(120.0));
    assert_eq!(state["auto_load"], true);
    assert_eq!(state["auto_load_groove"], json!(swing));
    assert_eq!(clip(&shell, clip_id).groove, Some(swing));

    // Extract, to a clip, save, load, commit, delete.
    let extracted = shell
        .run("groove.extract", json!({ "clips": [clip_id] }))
        .unwrap();
    let made: GrooveId = serde_json::from_value(extracted["grooves"][0].clone()).unwrap();
    let to_clip = shell
        .run(
            "groove.to_clip",
            json!({ "groove": made, "track": track, "at": BAR * 4 }),
        )
        .unwrap();
    assert!(
        shell
            .editor()
            .document()
            .clip(serde_json::from_value(to_clip["clip"].clone()).unwrap())
            .is_some()
    );
    let saved = shell
        .run("groove.save", json!({ "groove": swing }))
        .unwrap();
    let file = PathBuf::from(saved["path"].as_str().unwrap());
    assert_eq!(file, scratch.0.join("Library/Grooves/Swing 16ths.blgroove"));
    assert!(file.is_file());
    let loaded = shell
        .run(
            "groove.load",
            json!({ "path": file.display().to_string(), "clips": [] }),
        )
        .unwrap();
    assert_eq!(loaded["name"], "Swing 16ths");
    let committed = shell
        .run("groove.commit", json!({ "clips": [clip_id] }))
        .unwrap();
    assert_eq!(committed["clips"], json!([clip_id]));
    assert_eq!(clip(&shell, clip_id).groove, None);
    shell
        .run("groove.delete", json!({ "groove": swing }))
        .unwrap();
    let state = pool(&mut shell);
    assert_eq!(
        state["grooves"].as_array().unwrap().len(),
        2,
        "extracted and loaded"
    );
    assert_eq!(state["auto_load_groove"], Value::Null);
}

// Spec: grooves R3. `clip.set_groove` left without clips sets the selected
// ones, as one undo step.
#[test]
fn setting_a_groove_without_clips_sets_the_selected_ones() {
    let (mut shell, _, clip_id) = midi_shell(&[], false);
    let swing = add_swing(&mut shell);
    assert!(
        shell
            .run("clip.set_groove", json!({ "groove": swing }))
            .is_err(),
        "no clip is selected"
    );
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip_id] }))
        .unwrap();
    let before = steps(&shell);
    shell
        .run("clip.set_groove", json!({ "groove": swing }))
        .unwrap();
    assert_eq!(steps(&shell), before + 1);
    assert_eq!(clip(&shell, clip_id).groove, Some(swing));
    shell.run("clip.set_groove", json!({})).unwrap();
    assert_eq!(clip(&shell, clip_id).groove, None, "no groove clears it");
}

// Spec: grooves R17. From a MIDI clip: a note at each note's start with its
// velocity, the loudest where notes start together, as long as the part that
// plays, named after the clip, with the extracted parameters.
#[test]
fn extracting_from_a_midi_clip_gives_its_notes_name_and_parameters() {
    let notes = [
        Note::new(60, Ticks(0), Ticks(STEP), 100),
        Note::new(64, Ticks(0), Ticks(STEP), 120),
        Note::new(60, Ticks(STEP + 1000), Ticks(STEP), 70),
        Note::new(60, Ticks(BAR + STEP), Ticks(STEP), 90),
    ];
    // An unlooped clip: from its Start marker to its End marker, which is
    // the clip's end (two bars) here.
    let (mut shell, _, clip_id) = midi_shell(&notes, false);
    let before = steps(&shell);
    let result = shell
        .run("groove.extract", json!({ "clips": [clip_id] }))
        .unwrap();
    assert_eq!(steps(&shell), before + 1, "one undo step");
    assert_eq!(shell.editor().undo_history().next(), Some("Extract Groove"));
    let groove: GrooveId = serde_json::from_value(result["grooves"][0].clone()).unwrap();
    let state = pool(&mut shell);
    let held = &state["grooves"][0];
    assert_eq!(held["id"], json!(groove));
    assert_eq!(held["name"], "Riff");
    assert_eq!(held["length"], json!(BAR * 2));
    assert_eq!(
        held["notes"],
        json!([
            { "position": 0, "velocity": 120 },
            { "position": STEP + 1000, "velocity": 70 },
            { "position": BAR + STEP, "velocity": 90 },
        ])
    );
    for (field, value) in [
        ("quantize", 0.0),
        ("timing", 100.0),
        ("random", 0.0),
        ("velocity", 0.0),
    ] {
        assert_eq!(held[field], json!(value), "{field}");
    }
    assert_eq!(held["base"], "1/16");
    assert_eq!(held["active"], false);

    // A looped clip: its loop brace, one bar.
    let (mut shell, _, clip_id) = midi_shell(&notes, true);
    shell
        .run("groove.extract", json!({ "clips": [clip_id] }))
        .unwrap();
    let state = pool(&mut shell);
    let held = &state["grooves"][0];
    assert_eq!(held["length"], json!(BAR));
    assert_eq!(held["notes"].as_array().unwrap().len(), 2);
    // A clip with no name gives "Groove", and the selection is the default.
    shell
        .run("clip.rename", json!({ "clip": clip_id, "name": "" }))
        .unwrap();
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip_id] }))
        .unwrap();
    shell.run("groove.extract", json!({})).unwrap();
    assert_eq!(pool(&mut shell)["grooves"][1]["name"], "Groove");
}

// Spec: grooves R18. A groove made into a clip: a note on C3 at each groove
// note, a sixteenth long or as long as the gap, the clip one groove long and
// looping, named after the groove, with no groove of its own.
#[test]
fn a_groove_made_into_a_clip_holds_its_notes() {
    let (mut shell, track, _) = midi_shell(&[], false);
    let id: GrooveId = shell.new_id();
    shell
        .run(
            "groove.add",
            json!({ "groove": { "id": id, "name": "Shuffle", "length": BAR, "notes": [
                { "position": 0, "velocity": 127 },
                { "position": STEP / 2, "velocity": 90 },
                { "position": BAR - STEP / 4, "velocity": 60 },
            ] } }),
        )
        .unwrap();
    shell
        .run(
            "groove.set_auto_load",
            json!({ "enabled": true, "groove": id }),
        )
        .unwrap();
    let before = steps(&shell);
    let made = shell
        .run(
            "groove.to_clip",
            json!({ "groove": id, "track": track, "at": BAR * 3 }),
        )
        .unwrap();
    assert_eq!(steps(&shell), before + 1);
    let made = clip(
        &shell,
        serde_json::from_value(made["clip"].clone()).unwrap(),
    );
    assert_eq!(
        (made.name.as_str(), made.start, made.length),
        ("Shuffle", Ticks(BAR * 3), Ticks(BAR))
    );
    assert_eq!(
        made.groove, None,
        "a clip made of a groove takes no auto-load groove"
    );
    let ClipData::Midi(data) = &made.data else {
        panic!("a MIDI clip");
    };
    assert!(data.looping);
    assert_eq!(data.loop_end, Some(Ticks(BAR)));
    let mut got: Vec<_> = data
        .notes
        .iter()
        .map(|n| (n.key, n.start.get(), n.length.get(), n.velocity))
        .collect();
    got.sort_unstable();
    assert_eq!(
        got,
        [
            (60, 0, STEP / 2, 127),
            (60, STEP / 2, STEP, 90),
            (60, BAR - STEP / 4, STEP / 4, 60),
        ]
    );
}

/// Where a MIDI document plays its notes: the note-ons' frame, key and
/// velocity, for the first `ticks`.
fn played(document: &Document, ticks: i64) -> Vec<(i64, u8, u8)> {
    let probe = Arc::new(Probe::default());
    let registry = testing::registry(&probe);
    let mut offline = Offline::new(
        EngineConfig::new(bl_core::time::SampleRate::HZ_48000, 512),
        registry,
    );
    offline.sync(document);
    offline.sender().transport(TransportCommand::Play);
    let frames = document
        .settings()
        .tempo
        .ticks_to_frames(Ticks(ticks), bl_core::time::SampleRate::HZ_48000)
        .get();
    offline.render(frames as u64, |_| {});
    probe
        .recorded_notes()
        .into_iter()
        .filter(|note| note.event.kind == NoteKind::On)
        .map(|note| {
            (
                note.frame,
                note.event.key,
                (note.event.velocity * 127.0).round() as u8,
            )
        })
        .collect()
}

// Spec: grooves R16. Commit writes the positions and velocities playback
// gave, with the same seed, and sets the groove to none, as one undo step; the
// committed clip then plays the same notes with no groove.
#[test]
fn committing_a_midi_clip_writes_what_playback_gave() {
    let (mut shell, _, clip_id) = midi_shell(&sixteenths(100), false);
    let swing = add_swing(&mut shell);
    shell
        .run(
            "groove.set",
            json!({ "groove": swing, "quantize": 40.0, "timing": 100.0, "random": 50.0,
                    "velocity": 60.0 }),
        )
        .unwrap();
    shell
        .run(
            "clip.set_groove",
            json!({ "clips": [clip_id], "groove": swing }),
        )
        .unwrap();
    let grooved = played(shell.editor().document(), BAR);
    assert_eq!(grooved.len(), 16);
    assert!(
        grooved.iter().any(|on| on.2 != 100),
        "Velocity changed some"
    );
    let before = steps(&shell);
    let notes_before = clip(&shell, clip_id).data.clone();
    shell
        .run(
            "groove.commit",
            json!({ "clips": [clip_id], "seed": clip_seed(clip_id) }),
        )
        .unwrap();
    assert_eq!(steps(&shell), before + 1, "one undo step");
    assert_eq!(shell.editor().undo_history().next(), Some("Commit Groove"));
    assert_eq!(clip(&shell, clip_id).groove, None);
    let committed = played(shell.editor().document(), BAR);
    assert_eq!(
        committed, grooved,
        "the committed clip plays what the groove gave"
    );
    // A note keeps its length.
    let ClipData::Midi(data) = &clip(&shell, clip_id).data else {
        panic!("a MIDI clip");
    };
    assert!(data.notes.iter().all(|n| n.length == Ticks(STEP / 2)));
    // Undo gives the clip back as it was, with its groove.
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(clip(&shell, clip_id).groove, Some(swing));
    assert_eq!(clip(&shell, clip_id).data, notes_before);
    // A commit with no seed draws a new one, and a clip with no groove is
    // left alone.
    shell
        .run("groove.commit", json!({ "clips": [clip_id] }))
        .unwrap();
    let again = shell
        .run("groove.commit", json!({ "clips": [clip_id] }))
        .unwrap();
    assert_eq!(again["clips"], json!([]));
    // Given a groove and no clips it commits every clip that uses it.
    let (mut shell, track, first) = midi_shell(&sixteenths(100), false);
    let swing = add_swing(&mut shell);
    let second: ClipId = shell.new_id();
    shell
        .run(
            "clip.create",
            json!({ "track": track, "clip": { "id": second, "name": "", "start": BAR * 2,
                "length": BAR, "offset": 0, "mute": false, "data": { "midi": { "notes": [] } } } }),
        )
        .unwrap();
    shell
        .run(
            "clip.set_groove",
            json!({ "clips": [first, second], "groove": swing }),
        )
        .unwrap();
    let result = shell
        .run("groove.commit", json!({ "groove": swing }))
        .unwrap();
    assert_eq!(result["clips"].as_array().unwrap().len(), 2);
    assert_eq!(clip(&shell, second).groove, None);
}

// Spec: grooves R19. A groove saves into the User Library's Grooves folder,
// refuses a file already there without `replace`, and loads back with the
// parameters it was saved with; a broken file is refused and changes nothing.
#[test]
fn a_groove_saves_loads_and_a_broken_file_is_refused() {
    let scratch = Scratch::new("files");
    let (mut shell, _, clip_id) = midi_shell(&[], false);
    shell.set_user_library(Some(scratch.0.join("Library")));
    let swing = add_swing(&mut shell);
    shell
        .run(
            "groove.set",
            json!({ "groove": swing, "base": "1/32", "velocity": 55.5 }),
        )
        .unwrap();
    let saved = shell
        .run("groove.save", json!({ "groove": swing }))
        .unwrap();
    let path = saved["path"].as_str().unwrap().to_owned();
    assert!(
        shell
            .run("groove.save", json!({ "groove": swing }))
            .is_err(),
        "an existing file is left alone"
    );
    shell
        .run("groove.save", json!({ "groove": swing, "replace": true }))
        .unwrap();
    let before = steps(&shell);
    let loaded = shell
        .run("groove.load", json!({ "path": path, "clips": [clip_id] }))
        .unwrap();
    assert_eq!(steps(&shell), before + 1, "loading is one undo step");
    assert_eq!(shell.editor().undo_history().next(), Some("Load Groove"));
    let loaded: GrooveId = serde_json::from_value(loaded["groove"].clone()).unwrap();
    assert_ne!(loaded, swing, "loading again adds another");
    let state = pool(&mut shell);
    assert_eq!(state["grooves"].as_array().unwrap().len(), 2);
    assert_eq!(state["grooves"][1]["base"], "1/32");
    assert_eq!(state["grooves"][1]["velocity"], json!(55.5));
    assert_eq!(state["grooves"][1]["notes"], state["grooves"][0]["notes"]);
    assert_eq!(clip(&shell, clip_id).groove, Some(loaded));
    // A broken file.
    let broken = scratch.0.join("broken.blgroove");
    std::fs::write(
        &broken,
        std::fs::read_to_string(&path)
            .unwrap()
            .replace("1/32", "1/3"),
    )
    .unwrap();
    let before = steps(&shell);
    assert!(
        shell
            .run(
                "groove.load",
                json!({ "path": broken.display().to_string() })
            )
            .is_err()
    );
    assert_eq!(steps(&shell), before);
    assert!(
        shell
            .run(
                "groove.load",
                json!({ "path": scratch.0.join("none.blgroove").display().to_string() })
            )
            .is_err()
    );
    // A groove that isn't in the pool.
    let stranger: GrooveId = shell.new_id();
    assert!(
        shell
            .run("groove.save", json!({ "groove": stranger }))
            .is_err()
    );
}

// Spec: grooves R20. A MIDI clip made new and empty takes the auto-load
// groove while the switch is on; a copy keeps its own, and with the switch
// off a new clip takes none.
#[test]
fn a_new_midi_clip_takes_the_auto_load_groove_and_a_copy_keeps_its_own() {
    let (mut shell, track, first) = midi_shell(&sixteenths(100), false);
    let swing = add_swing(&mut shell);
    let scene = shell.editor().document().scenes().next().unwrap().id;
    let insert = |shell: &mut Shell, scene: bl_model::SceneId| {
        shell
            .run(
                "session.insert_empty_midi_clip",
                json!({ "track": track, "scene": scene }),
            )
            .unwrap();
        shell
            .editor()
            .document()
            .slot(track, scene)
            .unwrap()
            .clone()
    };
    assert_eq!(insert(&mut shell, scene).groove, None, "the switch is off");
    shell
        .run(
            "groove.set_auto_load",
            json!({ "enabled": true, "groove": swing }),
        )
        .unwrap();
    let scenes: Vec<_> = shell.editor().document().scenes().map(|s| s.id).collect();
    let made = insert(&mut shell, scenes[1]);
    assert_eq!(
        made.groove,
        Some(swing),
        "a new clip takes the auto-load groove"
    );
    // Copies keep their source's groove, and a clip made by `clip.create`
    // keeps its own: neither takes the auto-load groove.
    assert_eq!(clip(&shell, first).groove, None);
    shell
        .run("arrangement.select_clips", json!({ "clips": [first] }))
        .unwrap();
    let copies = |shell: &Shell| -> Vec<Option<GrooveId>> {
        shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .clips()
            .filter(|c| c.id != first)
            .map(|c| c.groove)
            .collect()
    };
    shell.run("arrangement.duplicate", json!({})).unwrap();
    assert_eq!(
        copies(&shell),
        [None],
        "a copy of a clip with no groove has none"
    );
    shell
        .run(
            "clip.set_groove",
            json!({ "clips": [first], "groove": swing }),
        )
        .unwrap();
    shell
        .run("arrangement.select_clips", json!({ "clips": [first] }))
        .unwrap();
    shell.run("arrangement.duplicate", json!({})).unwrap();
    assert!(
        copies(&shell).contains(&Some(swing)),
        "a copy keeps its source's groove"
    );
    let raw: ClipId = shell.new_id();
    shell
        .run(
            "clip.create",
            json!({ "track": track, "clip": { "id": raw, "name": "", "start": BAR * 8,
                "length": BAR, "offset": 0, "mute": false, "data": { "midi": { "notes": [] } } } }),
        )
        .unwrap();
    assert_eq!(clip(&shell, raw).groove, None);
    // A copy on the clipboard whose groove is deleted pastes with none.
    shell
        .run("arrangement.select_clips", json!({ "clips": [first] }))
        .unwrap();
    shell.run("arrangement.copy", json!({})).unwrap();
    // Deleting the auto-load groove leaves none, and nothing is auto-loaded.
    shell
        .run("groove.delete", json!({ "groove": swing }))
        .unwrap();
    assert_eq!(insert(&mut shell, scenes[2]).groove, None);
    shell.run("arrangement.paste", json!({})).unwrap();
    assert!(copies(&shell).iter().all(Option::is_none));
}

/// The generated drum loop's audio on an audio track, warped, with its
/// analysis done.
fn drum_clip(scratch: &Scratch) -> (Shell, ClipId) {
    let mut shell = Shell::new(
        Preferences::default(),
        Some(scratch.0.join("config")),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    plain_pool(&mut shell);
    let track = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Audio)
        .expect("the default set has audio tracks")
        .id();
    let path = scratch.0.join("drum-loop.wav");
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/drums/drum-loop.wav"),
        &path,
    )
    .unwrap();
    shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [path.display().to_string()], "track": track, "at": 0 }),
        )
        .unwrap();
    let clip = shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .clips()
        .next()
        .unwrap()
        .id;
    shell
        .run("clip.set_warp", json!({ "clip": clip, "warp": true }))
        .unwrap();
    let media = audio(&shell, clip).media;
    shell.wait_analysis(media).expect("the file is analysed");
    (shell, clip)
}

fn audio(shell: &Shell, clip: ClipId) -> bl_model::AudioData {
    match &shell.editor().document().clip(clip).unwrap().data {
        ClipData::Audio(audio) => audio.clone(),
        ClipData::Midi(_) => unreachable!("an audio clip"),
    }
}

/// The hits of the drum loop, in seconds.
fn hits() -> Vec<f64> {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/drums/drum-loop.hits"),
    )
    .unwrap()
    .lines()
    .map(|line| line.parse().unwrap())
    .collect()
}

/// How far each of the tests' groove's sixteenths sits from its line, in
/// steps: on the line, an eighth late, an eighth early, a quarter late.
fn offset(line: i64) -> i64 {
    [0, STEP / 8, -STEP / 8, STEP / 4][line.rem_euclid(4) as usize]
}

fn add_push(shell: &mut Shell, shape: Value) -> GrooveId {
    let id: GrooveId = shell.new_id();
    let notes: Vec<Value> = (0..16)
        .map(|i| {
            json!({ "position": i * STEP + offset(i),
                    "velocity": if i % 4 == 0 { 127 } else { 65 } })
        })
        .collect();
    shell
        .run(
            "groove.add",
            json!({ "groove": { "id": id, "name": "Push", "length": BAR, "notes": notes } }),
        )
        .unwrap();
    shell
        .run("groove.set", {
            let mut set = shape;
            set["groove"] = json!(id);
            set
        })
        .unwrap();
    id
}

// Spec: grooves R17. From an audio clip, a groove note at each transient in
// the clip's beats through its warp map, with a velocity from the audio's
// peak level after it: the loudest 127, the others by their level.
#[test]
fn extracting_from_an_audio_clip_gives_a_note_at_each_transient() {
    let scratch = Scratch::new("extract-audio");
    let (mut shell, clip_id) = drum_clip(&scratch);
    let hits = hits();
    let beat = |seconds: f64| (seconds * 2.0 * TICKS_PER_QUARTER as f64).round() as i64;
    let extract = |shell: &mut Shell| {
        shell
            .run("groove.extract", json!({ "clips": [clip_id] }))
            .unwrap();
        let state = pool(shell);
        state["grooves"].as_array().unwrap().last().unwrap().clone()
    };
    for (what, warped) in [("warped", true), ("unwarped", false)] {
        shell
            .run("clip.set_warp", json!({ "clip": clip_id, "warp": warped }))
            .unwrap();
        let held = extract(&mut shell);
        assert_eq!(held["name"], "drum-loop", "{what}: named after the clip");
        assert_eq!(held["length"], json!(BAR), "{what}: as long as the loop");
        let notes = held["notes"].as_array().unwrap();
        assert_eq!(notes.len(), hits.len(), "{what}: a note at each hit");
        for (note, hit) in notes.iter().zip(&hits) {
            // The transient is within a couple of milliseconds of the hit.
            let at = note["position"].as_i64().unwrap();
            assert!(
                (at - beat(*hit)).abs() < beat(0.003),
                "{what}: note at {at}, hit at {}",
                beat(*hit)
            );
        }
        let velocities: Vec<i64> = notes
            .iter()
            .map(|note| note["velocity"].as_i64().unwrap())
            .collect();
        assert_eq!(
            velocities.iter().max(),
            Some(&127),
            "{what}: the loudest is 127"
        );
        assert!(velocities.iter().all(|v| (1..=127).contains(v)));
        assert!(
            velocities.iter().min() < Some(&127),
            "{what}: the others are by their level: {velocities:?}"
        );
    }
}

// Spec: grooves R16. Commit on a warped audio clip writes a warp marker at
// each grid line where the groove plays it, and with Velocity above 0 a
// Volume clip envelope of the gains, as one undo step; the clip's groove goes.
#[test]
fn committing_an_audio_clip_writes_warp_markers_and_a_volume_envelope() {
    let scratch = Scratch::new("commit-audio");
    let (mut shell, clip_id) = drum_clip(&scratch);
    let push = add_push(&mut shell, json!({ "timing": 100.0, "velocity": 100.0 }));
    shell
        .run(
            "clip.set_groove",
            json!({ "clips": [clip_id], "groove": push }),
        )
        .unwrap();
    let before = steps(&shell);
    let gain = audio(&shell, clip_id).gain;
    let result = shell
        .run("groove.commit", json!({ "clips": [clip_id], "seed": 1 }))
        .unwrap();
    assert_eq!(result["clips"], json!([clip_id]));
    assert_eq!(steps(&shell), before + 1, "one undo step");
    assert_eq!(shell.editor().undo_history().next(), Some("Commit Groove"));
    let clip_after = clip(&shell, clip_id);
    assert_eq!(clip_after.groove, None);
    let committed = audio(&shell, clip_id);
    // A marker at each grid line, on the beat the groove plays it, at the
    // line's time in the file.
    for line in 1..16 {
        let marker = committed
            .warp_markers
            .iter()
            .find(|m| (m.time - line as f64 * 0.125).abs() < 1e-6)
            .unwrap_or_else(|| panic!("a marker at line {line}"));
        assert_eq!(
            marker.beat,
            Ticks(line * STEP + offset(line)),
            "line {line}"
        );
    }
    assert!(
        committed
            .warp_markers
            .windows(2)
            .all(|p| p[0].time < p[1].time && p[0].beat < p[1].beat)
    );
    // The envelope steps through the gains: shares of the loudest, with the
    // clip's gain raised by it. The loud lines (every fourth) are the
    // loudest and play at the clip's own level.
    let loudest = 1.0 + 46.5 / 127.0;
    assert!(
        (committed.gain - gain * loudest).abs() < 1e-9,
        "{}",
        committed.gain
    );
    let envelope = clip_after
        .envelopes
        .iter()
        .find(|e| {
            e.target
                == bl_model::AutomationTarget::Mixer {
                    param: bl_model::MixerParam::Volume,
                }
        })
        .expect("a Volume clip envelope");
    let points: Vec<_> = envelope.envelope.points().collect();
    let at = |tick: i64| points.iter().find(|p| p.at == Ticks(tick)).map(|p| p.value);
    assert_eq!(at(0), Some(1.0), "line 0 is loud: the loudest share");
    let soft = at(STEP + offset(1)).expect("a step at line 1");
    assert!(soft < 1.0 && soft > 0.3, "line 1 is softer: {soft}");
    assert_eq!(
        at(STEP + offset(1) - 1),
        Some(1.0),
        "the old value holds to the tick before"
    );
    // Undoing gives the clip back with its groove and no envelope.
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(clip(&shell, clip_id).groove, Some(push));
    assert!(clip(&shell, clip_id).envelopes.is_empty());
    assert_eq!(audio(&shell, clip_id).gain, gain);
    // An unwarped clip keeps nothing to write: its groove is simply dropped.
    shell
        .run("clip.set_warp", json!({ "clip": clip_id, "warp": false }))
        .unwrap();
    shell
        .run("groove.commit", json!({ "clips": [clip_id] }))
        .unwrap();
    assert_eq!(clip(&shell, clip_id).groove, None);
}

// WP-6.20.

/// Gives `shell` a User Library made in `scratch`.
fn make_library(shell: &mut Shell, scratch: &Scratch) -> PathBuf {
    let library = scratch.0.join("User Library");
    bl_shell::browser::places::create_user_library(&library).unwrap();
    shell.set_user_library(Some(library.clone()));
    library
}

fn browser(shell: &mut Shell) -> BrowserState {
    serde_json::from_value(
        shell
            .run(browser_ids::STATE, json!({ "limit": 1000 }))
            .unwrap(),
    )
    .unwrap()
}

fn text(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

// Spec: grooves R24. The pool shows and hides as view state (no undo step),
// showing it shows the browser, and Navigate's focus shows it and gives it
// the keyboard; the three commands are `read` class and have their keys.
#[test]
fn the_pool_shows_hides_and_takes_the_focus_as_view_state() {
    let mut shell = Shell::headless();
    let before = steps(&shell);
    assert_eq!(pool(&mut shell)["view"], json!({ "shown": false }));
    assert_eq!(
        shell.run("view.toggle_groove_pool", json!({})).unwrap(),
        json!({ "shown": true })
    );
    assert_eq!(shell.switch_state("view.toggle_groove_pool"), Some(true));
    assert_eq!(pool(&mut shell)["view"], json!({ "shown": true }));
    shell
        .run("view.toggle_groove_pool", json!({ "shown": false }))
        .unwrap();
    assert_eq!(shell.switch_state("view.toggle_groove_pool"), Some(false));
    // Focusing it shows it, and the browser as well.
    shell.run("view.toggle_browser", json!({})).unwrap();
    assert!(!shell.browser_visible());
    shell.take_effects();
    assert_eq!(
        shell.run("focus.groove_pool", json!({})).unwrap(),
        json!({ "shown": true })
    );
    assert!(shell.browser_visible());
    assert_eq!(shell.focused_area(), Area::Browser);
    assert!(
        shell
            .take_effects()
            .contains(&Effect::FocusArea(Area::Browser))
    );
    assert_eq!(
        shell.browser_view().focus(),
        bl_shell::browser::Part::GroovePool
    );
    // Only the global keys apply while the pool has the keyboard.
    assert_eq!(
        shell.active_contexts(),
        [bl_commands::Context::Global],
        "the browser's keys give way"
    );
    assert_eq!(steps(&shell), before, "view state is no undo step");
    let class = |id: &str| shell.registry().get(id).unwrap().permission.unwrap();
    for id in [
        "view.toggle_groove_pool",
        "focus.groove_pool",
        "groove.hot_swap",
    ] {
        assert_eq!(class(id), bl_commands::Permission::Read, "{id}");
    }
    let bound = |id: &str| -> Vec<String> {
        shell
            .registry()
            .get(id)
            .unwrap()
            .bindings
            .iter()
            .map(|b| b.shortcut.to_string())
            .collect()
    };
    assert_eq!(bound("view.toggle_groove_pool"), ["S-VIEW-017"]);
    assert_eq!(bound("focus.groove_pool"), ["S-FOCUS-007"]);
}

// Spec: grooves R34. Extract Groove(s) is disabled while no clip is selected.
#[test]
fn extract_groove_is_disabled_without_a_selected_clip() {
    let (mut shell, _track, clip_id) = midi_shell(&sixteenths(100), true);
    assert!(!shell.can_invoke("groove.extract", &json!({})));
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip_id] }))
        .unwrap();
    assert!(shell.can_invoke("groove.extract", &json!({})));
    let extracted = shell.run("groove.extract", json!({})).unwrap();
    assert_eq!(extracted["grooves"].as_array().unwrap().len(), 1);
}

/// Saves `groove` into the User Library's Grooves folder, and returns the
/// file.
fn saved(shell: &mut Shell, library: &std::path::Path, groove: GrooveId, name: &str) -> PathBuf {
    shell
        .run("groove.rename", json!({ "groove": groove, "name": name }))
        .unwrap();
    shell
        .run("groove.save", json!({ "groove": groove }))
        .unwrap();
    let file = library.join("Grooves").join(format!("{name}.blgroove"));
    assert!(file.is_file());
    file
}

// Spec: grooves R32, R33. A groove hot-swapped is replaced by the one the
// browser loads: every clip that used it, and the auto-load setting, take
// the new one, in one undo step, and the hot-swap stays on, aimed at the new
// groove, until it is ended.
#[test]
fn loading_while_a_groove_is_hot_swapped_replaces_it_for_every_clip_in_one_step() {
    let scratch = Scratch::new("hot-swap");
    let (mut shell, _track, first) = midi_shell(&sixteenths(100), true);
    let library = make_library(&mut shell, &scratch);
    let old = add_swing(&mut shell);
    let shuffle = {
        // A second groove, saved to a file the browser will list.
        let id = add_swing(&mut shell);
        saved(&mut shell, &library, id, "Shuffle")
    };
    shell
        .run(
            "clip.set_groove",
            json!({ "clips": [first], "groove": old }),
        )
        .unwrap();
    shell
        .run(
            "groove.set_auto_load",
            json!({ "enabled": true, "groove": old }),
        )
        .unwrap();
    let before = steps(&shell);
    // The pool row's button arms it, and the browser shows the Grooves label.
    let view = shell
        .run("groove.hot_swap", json!({ "groove": old }))
        .unwrap();
    assert_eq!(view["hot_swap"], json!({ "groove": old }));
    assert_eq!(
        shell.browser_view().label(),
        &bl_shell::browser_commands::Label::Grooves
    );
    let listed = browser(&mut shell);
    assert!(
        listed
            .rows
            .iter()
            .any(|row| row.name == "Shuffle.blgroove" && row.kind == ItemKind::Groove),
        "{:?}",
        listed.rows.iter().map(|r| &r.name).collect::<Vec<_>>()
    );
    assert_eq!(steps(&shell), before, "arming is view state");
    // Enter on the file in the browser loads it in the old groove's place.
    shell.set_focused_area(Area::Browser);
    shell
        .run(browser_ids::SELECT, json!({ "paths": [text(&shuffle)] }))
        .unwrap();
    assert!(shell.can_invoke(browser_ids::LOAD_SELECTED, &json!({})));
    shell.run(browser_ids::LOAD_SELECTED, json!({})).unwrap();
    assert_eq!(steps(&shell), before + 1, "one undo step");
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Hot-Swap Groove")
    );
    let state = pool(&mut shell);
    let entries = state["grooves"].as_array().unwrap();
    let new: GrooveId = serde_json::from_value(entries.last().unwrap()["id"].clone()).unwrap();
    assert_ne!(new, old);
    assert!(
        entries.iter().all(|g| g["id"] != json!(old)),
        "the old groove left the pool"
    );
    assert_eq!(clip(&shell, first).groove, Some(new));
    assert_eq!(state["auto_load_groove"], json!(new));
    assert_eq!(state["view"]["hot_swap"], json!({ "groove": new }));
    // Undo puts everything back in one step.
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(clip(&shell, first).groove, Some(old));
    assert_eq!(pool(&mut shell)["auto_load_groove"], json!(old));
    assert_eq!(
        pool(&mut shell)["view"].get("hot_swap"),
        None,
        "a hot-swap aimed at a groove that left the pool is over"
    );
    shell
        .run("groove.hot_swap", json!({ "groove": old }))
        .unwrap();
    // The button again ends it, and loading then only adds.
    shell
        .run("groove.hot_swap", json!({ "groove": old }))
        .unwrap();
    assert_eq!(pool(&mut shell)["view"].get("hot_swap"), None);
    let count = pool(&mut shell)["grooves"].as_array().unwrap().len();
    shell.run(browser_ids::LOAD_SELECTED, json!({})).unwrap();
    assert_eq!(
        pool(&mut shell)["grooves"].as_array().unwrap().len(),
        count + 1
    );
    assert_eq!(clip(&shell, first).groove, Some(old));
}

// Spec: grooves R31, R32. The footer's button aims the hot-swap at the
// auto-load groove: loading replaces it, the clips that used it follow, and
// with none the loaded groove becomes it.
#[test]
fn the_auto_load_groove_is_hot_swapped_and_one_loaded_into_none_becomes_it() {
    let scratch = Scratch::new("hot-swap-auto");
    let (mut shell, _, _) = midi_shell(&sixteenths(100), true);
    let library = make_library(&mut shell, &scratch);
    let file = {
        let id = add_swing(&mut shell);
        saved(&mut shell, &library, id, "Shuffle")
    };
    let added = pool(&mut shell)["grooves"][0]["id"].clone();
    shell
        .run("groove.delete", json!({ "groove": added }))
        .unwrap();
    assert_eq!(pool(&mut shell)["grooves"], json!([]));
    let view = shell
        .run("groove.hot_swap", json!({ "auto_load": true }))
        .unwrap();
    assert_eq!(view["hot_swap"], json!("auto_load"));
    shell
        .run(browser_ids::SELECT, json!({ "paths": [text(&file)] }))
        .unwrap();
    shell.run(browser_ids::LOAD_SELECTED, json!({})).unwrap();
    let state = pool(&mut shell);
    let loaded = state["grooves"][0]["id"].clone();
    assert_eq!(state["auto_load_groove"], loaded, "the new one is it");
    // Again: it replaces the auto-load groove.
    shell.run(browser_ids::LOAD_SELECTED, json!({})).unwrap();
    let state = pool(&mut shell);
    assert_eq!(state["grooves"].as_array().unwrap().len(), 1);
    assert_ne!(state["grooves"][0]["id"], loaded);
    assert_eq!(state["auto_load_groove"], state["grooves"][0]["id"]);
    assert_eq!(state["view"]["hot_swap"], json!("auto_load"));
    // Esc, the bar's close button and the button again end it.
    shell
        .run("groove.hot_swap", json!({ "on": false }))
        .unwrap();
    assert_eq!(pool(&mut shell)["view"].get("hot_swap"), None);
    // A groove that left the pool, by deleting it, ends a hot-swap aimed at it.
    let id: GrooveId =
        serde_json::from_value(pool(&mut shell)["grooves"][0]["id"].clone()).unwrap();
    shell
        .run("groove.hot_swap", json!({ "groove": id }))
        .unwrap();
    shell.run("groove.delete", json!({ "groove": id })).unwrap();
    assert_eq!(pool(&mut shell)["view"].get("hot_swap"), None);
    let wrong = shell.run("groove.hot_swap", json!({ "groove": id }));
    assert!(wrong.is_err(), "a groove that isn't in the pool");
    // A new set ends a hot-swap aimed at the old set's auto-load groove.
    shell
        .run("groove.hot_swap", json!({ "auto_load": true }))
        .unwrap();
    shell.run("set.new", json!({})).unwrap();
    assert_eq!(pool(&mut shell)["view"].get("hot_swap"), None);
}

// Spec: grooves R33. The Grooves label lists Beatlayer's bundled grooves and
// the User Library's Grooves folder by name; search filters them by every
// word typed; Enter loads a groove into the pool without applying it.
#[test]
fn the_grooves_label_lists_the_bundled_grooves_and_the_users_and_enter_loads_one() {
    let scratch = Scratch::new("label");
    let (mut shell, _, clip_id) = midi_shell(&sixteenths(100), true);
    let library = make_library(&mut shell, &scratch);
    let id = add_swing(&mut shell);
    let mine = saved(&mut shell, &library, id, "My Shuffle");
    shell
        .run(browser_ids::OPEN_PLACE, json!({ "label": "grooves" }))
        .unwrap();
    let listed = browser(&mut shell);
    let names: Vec<&str> = listed.rows.iter().map(|row| row.name.as_str()).collect();
    for name in [
        "Swing 8ths 55.blgroove",
        "Swing 16ths 75.blgroove",
        "Straight 8ths.blgroove",
        "Straight 16ths.blgroove",
        "My Shuffle.blgroove",
    ] {
        assert!(names.contains(&name), "{name} in {names:?}");
    }
    assert_eq!(names.len(), 13, "twelve bundled and one of the person's");
    assert!(listed.rows.iter().all(|row| row.kind == ItemKind::Groove));
    // The label's note is gone, and search filters by every word typed.
    let found: BrowserState = serde_json::from_value(
        shell
            .run(browser_ids::SEARCH, json!({ "query": "16ths swing 60" }))
            .unwrap(),
    )
    .unwrap();
    let names: Vec<&str> = found.rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["Swing 16ths 60.blgroove"]);
    shell
        .run(browser_ids::SEARCH, json!({ "query": "shuffle" }))
        .unwrap();
    // Enter on the file loads it into the pool, applying it to nothing.
    shell.set_focused_area(Area::Browser);
    shell
        .run(browser_ids::SELECT, json!({ "paths": [text(&mine)] }))
        .unwrap();
    let before = pool(&mut shell)["grooves"].as_array().unwrap().len();
    shell.run(browser_ids::LOAD_SELECTED, json!({})).unwrap();
    let state = pool(&mut shell);
    assert_eq!(state["grooves"].as_array().unwrap().len(), before + 1);
    assert_eq!(state["grooves"][before]["name"], json!("My Shuffle"));
    assert_eq!(state["grooves"][before]["active"], json!(false));
    assert_eq!(clip(&shell, clip_id).groove, None);
    // A groove file shows in the places as every file does.
    shell
        .run(browser_ids::OPEN_PLACE, json!({ "label": "user_library" }))
        .unwrap();
    shell
        .run(
            browser_ids::OPEN_FOLDER,
            json!({ "path": text(&library.join("Grooves")), "open": true }),
        )
        .unwrap();
    let listed = browser(&mut shell);
    assert!(
        listed
            .rows
            .iter()
            .any(|row| row.name == "My Shuffle.blgroove" && row.kind == ItemKind::Groove)
    );
}

// Spec: grooves R36, R37. The grooves the app carries are the twelve the spec
// lists, found where the Grooves label looks, and every file loads as the
// groove the script makes.
#[test]
fn the_bundled_grooves_ship_as_files_that_load() {
    use bl_project::groove_file::{GROOVE_EXTENSION, GrooveFile, bundled};
    let folder = bl_shell::resources::grooves().expect("the app has its grooves");
    let mut found: Vec<GrooveFile> = Vec::new();
    for entry in std::fs::read_dir(&folder).unwrap() {
        let path = entry.unwrap().path();
        assert_eq!(
            path.extension().and_then(|e| e.to_str()),
            Some(GROOVE_EXTENSION)
        );
        found.push(GrooveFile::load(&path).unwrap());
    }
    let mut made = bundled();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    made.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(found, made);
    assert_eq!(found.len(), 12);
}

// Spec: grooves R38. The factory default set has Swing 16ths 60 in its pool
// as the auto-load groove, Auto Load Groove on and the Global Amount at 0 %,
// so a new MIDI clip takes it and plays straight.
#[test]
fn the_factory_default_set_carries_swing_16ths_60_as_its_auto_load_groove() {
    let editor = bl_shell::files::factory_default_set();
    let pool = &editor.document().settings().grooves;
    assert_eq!(pool.pool.len(), 1);
    assert_eq!(pool.pool[0].name, "Swing 16ths 60");
    assert_eq!(pool.auto_load_groove, Some(pool.pool[0].id));
    assert!(pool.auto_load);
    assert_eq!(pool.global_amount, 0.0);
    assert!(!editor.can_undo(), "no history");
    // A new MIDI clip carries it.
    let mut shell = Shell::headless();
    shell.run("set.new", json!({})).unwrap();
    let track = shell.editor().document().top_level().next().unwrap();
    let scene = shell.editor().document().scenes().next().unwrap().id;
    shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    let made = shell.editor().document().slot(track, scene).unwrap();
    assert_eq!(made.groove, Some(pool.pool[0].id));
}

// Spec: grooves R30. The Control Bar's Global Amount field shows while the
// pool holds a groove and hides while it is empty; it steps, resets and takes
// a typed value as the other fields do, each change `groove.set_global_amount`,
// a drag one undo step, brought within 0 to 130 %.
#[test]
fn the_global_amount_field_shows_with_a_groove_and_edits_the_amount() {
    use bl_shell::transport::{self, Field, Group};
    let (mut shell, _, _) = midi_shell(&sixteenths(100), true);
    let field = Field::new(Group::GlobalAmount, 0);
    assert!(!shell.global_amount_shown(), "an empty pool hides it");
    let swing = add_swing(&mut shell);
    assert!(shell.global_amount_shown());
    assert_eq!(transport::text(&shell, field), "100 %");
    let run = |shell: &mut Shell, invocation: Option<bl_shell::Invocation>| {
        let invocation = invocation.expect("a change");
        assert_eq!(invocation.id, "groove.set_global_amount");
        shell.run(&invocation.id, invocation.args).unwrap();
    };
    let before = steps(&shell);
    let change = transport::step(&shell, field, 5, false);
    run(&mut shell, change);
    assert_eq!(transport::text(&shell, field), "105 %");
    let change = transport::step(&shell, field, -3, true);
    run(&mut shell, change);
    assert_eq!(transport::text(&shell, field), "104.7 %");
    assert_eq!(steps(&shell), before + 2, "each change is one undo step");
    // Brought within 0 to 130 %.
    let change = transport::step(&shell, field, 99, false);
    run(&mut shell, change);
    assert_eq!(transport::number(&shell, field), 130.0);
    assert_eq!(transport::step(&shell, field, 1, false), None, "at the top");
    // Typed, and Delete returns it to 100 %.
    let change = transport::typed(
        &shell,
        Group::GlobalAmount,
        &[Some("62.5".into()), None, None],
    );
    run(&mut shell, change);
    assert_eq!(transport::text(&shell, field), "62.5 %");
    let change = transport::typed(
        &shell,
        Group::GlobalAmount,
        &[Some("500".into()), None, None],
    );
    run(&mut shell, change);
    assert_eq!(transport::number(&shell, field), 130.0);
    let change = transport::reset(&shell, field);
    run(&mut shell, change);
    assert_eq!(transport::text(&shell, field), "100 %");
    assert_eq!(transport::reset(&shell, field), None, "already there");
    // A drag is one undo step, however far it goes.
    let before = steps(&shell);
    shell.begin_gesture(Group::GlobalAmount.gesture_label().unwrap());
    for _ in 0..5 {
        let change = transport::step(&shell, field, -2, false);
        run(&mut shell, change);
    }
    shell.end_gesture();
    assert_eq!(steps(&shell), before + 1);
    assert_eq!(transport::text(&shell, field), "90 %");
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Change Global Amount")
    );
    // Taking the last groove out hides it again.
    shell
        .run("groove.delete", json!({ "groove": swing }))
        .unwrap();
    assert!(!shell.global_amount_shown());
}

// Spec: grooves R3, R22. A groove ID names a groove of the set's pool: tracks
// copied and pasted in the set keep their clips' grooves, a groove deleted
// since the copy is none, and tracks saved as a set, whose pool is empty,
// leave them behind.
#[test]
fn a_copied_track_keeps_its_clips_groove_only_while_the_pool_holds_it() {
    let scratch = Scratch::new("copy-track");
    let (mut shell, track, clip_id) = midi_shell(&sixteenths(100), true);
    let swing = add_swing(&mut shell);
    shell
        .run(
            "clip.set_groove",
            json!({ "clips": [clip_id], "groove": swing }),
        )
        .unwrap();
    let grooves_of = |shell: &Shell, of: TrackId| -> Vec<Option<GrooveId>> {
        shell
            .editor()
            .document()
            .track(of)
            .unwrap()
            .clips()
            .map(|clip| clip.groove)
            .collect()
    };
    let others = |shell: &Shell| -> Vec<TrackId> {
        let doc = shell.editor().document();
        doc.top_level()
            .filter(|t| {
                *t != track
                    && doc.track(*t).unwrap().kind() == TrackKind::Midi
                    && doc.track(*t).unwrap().clips().count() > 0
            })
            .collect()
    };

    // Saved as a set: the export works, and the new set's clip has no groove.
    let folder = scratch.0.join("Songs");
    std::fs::create_dir_all(&folder).unwrap();
    let saved = shell
        .run(
            "set.export_tracks",
            json!({ "tracks": [track], "folder": folder, "name": "Keys" }),
        )
        .unwrap();
    let file = saved["path"].as_str().unwrap().to_owned();
    let (set, project) = bl_project::SetFile::open(std::path::Path::new(&file)).unwrap();
    let _ = set;
    let exported = project.document;
    assert!(exported.settings().grooves.pool.is_empty());
    assert!(
        exported
            .tracks()
            .flat_map(|t| t.clips().map(|c| c.groove).collect::<Vec<_>>())
            .all(|g| g.is_none())
    );

    // Copy and paste in the set keep it.
    shell
        .run("track.copy", json!({ "tracks": [track] }))
        .unwrap();
    shell.run("track.paste", json!({})).unwrap();
    let pasted = others(&shell);
    assert_eq!(pasted.len(), 1);
    assert_eq!(grooves_of(&shell, pasted[0]), [Some(swing)]);

    // A groove deleted before the paste is none.
    shell
        .run("track.copy", json!({ "tracks": [track] }))
        .unwrap();
    shell
        .run("groove.delete", json!({ "groove": swing }))
        .unwrap();
    shell.run("track.paste", json!({})).unwrap();
    for pasted in others(&shell) {
        assert_eq!(grooves_of(&shell, pasted), [None]);
    }
}

// Spec: grooves R34 (M-EDIT-037). Edit > Extract Groove(s) runs
// `groove.extract` for the selected clips, and is disabled while none is.
#[test]
fn the_edit_menus_extract_groove_item_follows_the_selection() {
    use bl_shell::menus::{Action, MenuPlatform, menu_bar};
    let menus = menu_bar(MenuPlatform::Mac);
    let item = menus
        .iter()
        .flat_map(bl_shell::menus::Menu::walk)
        .find(|item| item.row == "M-EDIT-037")
        .expect("the Edit menu's item");
    assert_eq!(item.label, "Extract Groove(s)");
    assert!(matches!(item.action, Action::Command("groove.extract")));
    let (mut shell, _track, clip_id) = midi_shell(&sixteenths(100), true);
    assert!(!shell.can_invoke("groove.extract", &json!({})));
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip_id] }))
        .unwrap();
    assert!(shell.can_invoke("groove.extract", &json!({})));
    let before = steps(&shell);
    shell.invoke("groove.extract", json!({})).unwrap();
    assert_eq!(steps(&shell), before + 1);
    assert_eq!(pool(&mut shell)["grooves"].as_array().unwrap().len(), 1);
}

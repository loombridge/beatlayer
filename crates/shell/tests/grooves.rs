//! Grooves in the app shell (WP-6.19): the commands that load, save,
//! extract, commit and make clips of grooves, the pool query, and a new
//! MIDI clip taking the auto-load groove. Spec: grooves R3, R5, R16 to R20
//! and R23.

use std::path::PathBuf;
use std::sync::Arc;

use bl_core::time::{TICKS_PER_QUARTER, Ticks};
use bl_engine::testing::{self, Probe};
use bl_engine::{EngineConfig, NoteKind, Offline, TransportCommand};
use bl_model::{
    ClipData, ClipId, Device, DeviceKind, Document, GrooveId, MidiData, Note, TrackId, TrackKind,
    clip_seed,
};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
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

/// A shell with a MIDI track holding the test instrument and a clip of
/// `notes`, looping over the bar when `looped`.
fn midi_shell(notes: &[Note], looped: bool) -> (Shell, TrackId, ClipId) {
    let mut shell = Shell::headless();
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
        json!({ "grooves": [], "global_amount": 100.0, "auto_load": false })
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
    // Deleting the auto-load groove leaves none, and nothing is auto-loaded.
    shell
        .run("groove.delete", json!({ "groove": swing }))
        .unwrap();
    assert_eq!(insert(&mut shell, scenes[2]).groove, None);
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

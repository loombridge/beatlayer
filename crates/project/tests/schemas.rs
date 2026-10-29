//! Schema versions and migrations (WP-0.7 acceptance).
//!
//! Every schema version this build reads keeps a fixture in
//! `tests/fixtures/`: `schema-<n>.json`, the JSON a set of that version
//! holds (pretty-printed for review), and for version 1 also `schema-1.blset`,
//! a whole set file as version 1 wrote it. Each must load, through the
//! migrations, as the song [`fixture_project`] builds. Fixtures are never
//! rewritten once merged; a new schema version adds its own.
//!
//! Version 1 is the first. Version 2 (WP-1.4b) moved audio clips' offsets
//! into their Start markers. Version 3 (WP-3.1) added the Session View's
//! scenes and the clips in their slots; a set saved before it opens with the
//! default eight scenes, whose IDs sit after the set's largest. Version 4
//! added the global launch quantisation. Version 5 (WP-5.9) made a clip
//! envelope's values modulation rather than parameter units. Version 6
//! (WP-5.13) added a multisampler's layers and what its views show; a set
//! saved before it opens with none. Version 7 (T197) renamed the native
//! devices to Live's names; a set saved before it opens with the new names.
//! Version 8 (WP-6.21) gave take lanes and their clips IDs. Version 9
//! (WP-6.17) made a note's pressure an envelope of breakpoints; a
//! set saved before it opens with each pressure point a step. Version 10
//! (T348) made Loop Back-and-Forth the default Transient Loop Mode; a set
//! saved before it names Loop Forward on each audio clip that had it by default.
//! Version 11 (T262) added colours. Version 12 (WP-7.1) added the set's MIDI
//! and key mappings; a set saved before it opens with none. Version 13
//! (WP-7.33) gave Simpler's sample a region between Start and End flags; a
//! set saved before it opens with a One-Shot or Slice Simpler's Start and
//! Length as its region.
//! `schema-0-fake.json` is an invented version 0, with a migration to
//! version 1 defined below, to show the scaffold carries an old file through
//! to the current model.

use std::path::{Path, PathBuf};

use bl_core::id::IdAllocator;
use bl_core::time::{
    Bpm, TempoMap, TempoSegment, Ticks, TimeSignature, TimeSignatureChange, TimeSignatureMap,
};
use bl_model::{
    AudioData, AutomationTarget, Breakpoint, Clip, ClipData, Command, ContentHash, Device,
    DeviceKind, DeviceRole, Editor, MidiData, Note, ParamId, PluginFormat, SampleRegion, TrackKind,
    TransientLoopMode,
};
use bl_project::migrate::{MIGRATIONS, OLDEST_SCHEMA_VERSION};
use bl_project::{
    LoadError, LoadOptions, Media, MediaPath, MediaRef, Migration, Project, SCHEMA_VERSION,
    SetInfo, decode, decode_json, encode, encode_json,
};
use serde_json::{Value, json};

/// The revision every fixture was saved as.
const REVISION: u64 = 7;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn read(name: &str) -> Vec<u8> {
    let path = fixtures().join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The song every fixture holds: a group of two tracks, a return, clips of
/// both kinds, a native device and a plug-in, a tempo ramp, a time-signature
/// change, odd mixer values, and media inside and outside the project.
///
/// Its scenes are added last, after its tracks, clips, and devices, so their
/// IDs are the ones a set saved before WP-3.1 loads with: the default eight,
/// after the set's largest. A song built any other way would hold the same
/// scenes at other IDs, and the old fixtures couldn't load as it.
fn fixture_project() -> Project {
    let mut ids = IdAllocator::new();
    let main = bl_model::TrackId(ids.allocate());
    let mut editor = Editor::from_document(bl_model::Document::new(main, []), ids);
    let track = |editor: &mut Editor, kind, name: &str, parent, index| {
        let id = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind,
                name: name.to_owned(),
                parent,
                index,
            })
            .unwrap();
        id
    };
    let group = track(&mut editor, TrackKind::Group, "Drums", None, 0);
    let kick = track(&mut editor, TrackKind::Audio, "Kick", Some(group), 0);
    let hats = track(&mut editor, TrackKind::Midi, "Hats ♪", Some(group), 1);
    let bass = track(&mut editor, TrackKind::Midi, "Bass", None, 1);
    let reverb = track(&mut editor, TrackKind::Return, "A Reverb", None, 0);
    let main = editor.document().main_id();

    for command in [
        Command::SetVolume {
            track: kick,
            volume: 0.501_187_233_627_272_2,
        },
        Command::SetPan {
            track: hats,
            pan: 0.1 + 0.2,
        },
        Command::SetMute {
            track: bass,
            mute: true,
        },
        Command::SetSolo {
            track: hats,
            solo: true,
        },
        Command::SetArm {
            track: bass,
            arm: true,
        },
        Command::SetVolume {
            track: reverb,
            volume: 0.0,
        },
        Command::SetVolume {
            track: main,
            volume: 5e-324,
        },
    ] {
        editor.apply(command).unwrap();
    }

    let kick_sample = ContentHash([0x4b; 32]);
    let mut clip = Clip::new(
        editor.new_id(),
        Ticks::ZERO,
        Ticks::WHOLE * 4,
        // Sets of the versions before 10 name no Transient Loop Mode, which
        // then meant Loop Forward and migrates to it.
        ClipData::Audio({
            let mut audio = AudioData::new(kick_sample);
            audio.warp_settings.beats.transient_loop_mode = TransientLoopMode::LoopForward;
            audio
        }),
    );
    clip.name = "Kick loop".to_owned();
    editor
        .apply(Command::CreateClip { track: kick, clip })
        .unwrap();
    let notes = (0..8).map(|n| {
        Note::new(
            42,
            Ticks::SIXTEENTH * (i64::from(n) * 2),
            Ticks::SIXTEENTH,
            64 + n * 8,
        )
    });
    let mut clip = Clip::new(
        editor.new_id(),
        Ticks::WHOLE,
        Ticks::WHOLE,
        ClipData::Midi(MidiData::new(notes)),
    );
    clip.mute = true;
    editor
        .apply(Command::CreateClip { track: hats, clip })
        .unwrap();
    // A triplet-length note, which only lands on whole ticks at 705,600 per
    // quarter note.
    let triplet = Note::new(
        36,
        Ticks(Ticks::QUARTER.get() / 3),
        Ticks(Ticks::QUARTER.get() * 2 / 3),
        127,
    );
    let clip = Clip::new(
        editor.new_id(),
        Ticks::WHOLE * 2,
        Ticks::WHOLE * 2,
        ClipData::Midi(MidiData::new([triplet])),
    );
    editor
        .apply(Command::CreateClip { track: bass, clip })
        .unwrap();

    let mut utility = Device::new(
        editor.new_id(),
        "Utility",
        DeviceKind::Native {
            device_type: "beatlayer.utility".to_owned(),
        },
    );
    utility.params.insert(ParamId(0), -6.0);
    utility.params.insert(ParamId(1), 1.0 / 3.0);
    utility.params.insert(ParamId(2), -0.0);
    utility.params.insert(ParamId(7), f64::MAX);
    editor
        .apply(Command::InsertDevice {
            track: kick,
            index: 0,
            device: utility,
        })
        .unwrap();
    let mut synth = Device::new(
        editor.new_id(),
        "Example Synth",
        DeviceKind::Plugin {
            format: PluginFormat::Clap,
            plugin_id: "org.example.synth".to_owned(),
        },
    );
    synth.enabled = false;
    synth.params.insert(ParamId(u32::MAX), 0.25);
    editor
        .apply(Command::InsertDevice {
            track: bass,
            index: 0,
            device: synth,
        })
        .unwrap();

    let bpm = |bpm| Bpm::new(bpm).unwrap();
    let tempo = TempoMap::new([
        TempoSegment::Constant {
            start: Ticks::ZERO,
            bpm: bpm(124.0),
        },
        TempoSegment::Ramp {
            start: Ticks::WHOLE * 8,
            from: bpm(124.0),
            to: bpm(98.5),
        },
        TempoSegment::Constant {
            start: Ticks::WHOLE * 12,
            bpm: bpm(98.5),
        },
    ])
    .unwrap();
    editor.apply(Command::SetTempo { tempo }).unwrap();
    let time_signatures = TimeSignatureMap::new([
        TimeSignatureChange {
            at: Ticks::ZERO,
            signature: TimeSignature::FOUR_FOUR,
        },
        TimeSignatureChange {
            at: Ticks::WHOLE * 16,
            signature: TimeSignature::new(7, 8).unwrap(),
        },
    ])
    .unwrap();
    editor
        .apply(Command::SetTimeSignatures { time_signatures })
        .unwrap();

    let media: Media = [
        MediaRef {
            hash: kick_sample,
            path: MediaPath::new("Samples/Imported/Kick loop.wav").unwrap(),
            size: 1_058_444,
            reversed_from: None,
        },
        MediaRef {
            hash: ContentHash([0x50; 32]),
            path: MediaPath::new("../Library/Pads/Warm pad.aif").unwrap(),
            size: u64::from(u32::MAX) + 1,
            reversed_from: None,
        },
    ]
    .into_iter()
    .collect();
    // The scenes come last, so their IDs are the ones a set saved before
    // WP-3.1 loads with: the default eight, without names, after the song's
    // largest ID.
    for index in 0..bl_model::DEFAULT_SCENE_COUNT {
        let id = editor.new_id();
        editor.apply(Command::CreateScene { id, index }).unwrap();
    }
    Project {
        document: editor.document().clone(),
        ids: editor.ids(),
        media,
        windows: bl_model::DeviceWindows::new(),
        rack_views: bl_model::RackViewMap::new(),
        folds: bl_model::ClipFolds::new(),
        scale_folds: bl_model::ClipFolds::new(),
        midi_tools: bl_project::midi_tools::MidiToolSettings::default(),
        launch: bl_model::LaunchSettings::default(),
        multisamplers: bl_model::MultisamplerViews::new(),
        window_views: bl_model::WindowViews::default(),
        mpe_view: bl_model::MpeView::default(),
        lane_views: bl_model::LaneViews::new(),
        column_widths: bl_model::ColumnWidths::new(),
    }
}

/// The JSON this build writes for the fixture song, with `saved_by` as the
/// fixtures have it, so a version bump doesn't count as a format change.
fn written_now() -> Value {
    let mut value: Value =
        serde_json::from_slice(&encode_json(&fixture_project(), REVISION).unwrap()).unwrap();
    value["saved_by"] = json!("Beatlayer 0.0.0");
    value
}

/// Creates the fixtures for the current schema version if they're missing.
/// Run it once, by hand, when the schema version goes up:
/// `cargo test -p bl-project --test project_tests -- schemas:: --ignored`. It never
/// overwrites a fixture.
#[test]
#[ignore = "creates fixture files; run by hand for a new schema version"]
fn write_current_fixtures() {
    let json = fixtures().join(format!("schema-{SCHEMA_VERSION}.json"));
    if !json.exists() {
        let compact = encode_json(&fixture_project(), REVISION).unwrap();
        std::fs::write(&json, pretty(&compact)).unwrap();
    }
    let set = fixtures().join(format!("schema-{SCHEMA_VERSION}.blset"));
    if !set.exists() {
        std::fs::write(&set, encode(&fixture_project(), REVISION).unwrap()).unwrap();
    }
}

/// Lays out compact JSON one value to a line, keeping the order of its
/// fields, so a fixture reads like the file it stands for.
fn pretty(compact: &[u8]) -> String {
    let text = std::str::from_utf8(compact).unwrap();
    let mut out = String::new();
    let mut depth = 0;
    let (mut in_string, mut escaped) = (false, false);
    let mut chars = text.chars().peekable();
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        out.push_str(&"  ".repeat(depth));
    };
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '{' | '[' => {
                out.push(c);
                if matches!(chars.peek(), Some('}' | ']')) {
                    out.push(chars.next().unwrap());
                } else {
                    depth += 1;
                    newline(&mut out, depth);
                }
            }
            '}' | ']' => {
                depth -= 1;
                newline(&mut out, depth);
                out.push(c);
            }
            ',' => {
                out.push(c);
                newline(&mut out, depth);
            }
            ':' => out.push_str(": "),
            _ => out.push(c),
        }
    }
    out.push('\n');
    out
}

#[test]
fn every_readable_version_has_a_fixture() {
    for version in OLDEST_SCHEMA_VERSION..=SCHEMA_VERSION {
        let name = format!("schema-{version}.json");
        assert!(
            fixtures().join(&name).is_file(),
            "schema version {version} has no fixture {name}"
        );
    }
    assert_eq!(
        MIGRATIONS.len(),
        (SCHEMA_VERSION - OLDEST_SCHEMA_VERSION) as usize
    );
}

/// The fixture of every version loads, through the migrations, as the
/// fixture song.
#[test]
fn every_fixture_loads_as_the_fixture_song() {
    let expected = fixture_project();
    for version in OLDEST_SCHEMA_VERSION..=SCHEMA_VERSION {
        let json = read(&format!("schema-{version}.json"));
        let (project, info) = decode_json(&json, &LoadOptions::default()).unwrap();
        assert_eq!(project, expected, "schema {version}");
        assert_eq!(info.schema, version);
        assert_eq!(info.revision, REVISION);
    }
}

/// The name of the fixture song's Utility, after its type and name are made
/// `device_type` and `name` in the version 6 fixture and the set is loaded.
fn utility_loaded_as(device_type: &str, name: &str) -> String {
    fn change(value: &mut Value, device_type: &str, name: &str) -> bool {
        match value {
            Value::Object(map) => {
                if map.get("name") == Some(&json!("Utility")) && map.contains_key("kind") {
                    map["name"] = json!(name);
                    map["kind"]["native"]["device_type"] = json!(device_type);
                    return true;
                }
                map.values_mut().any(|v| change(v, device_type, name))
            }
            Value::Array(items) => items.iter_mut().any(|v| change(v, device_type, name)),
            _ => false,
        }
    }
    let mut value: Value = serde_json::from_slice(&read("schema-6.json")).unwrap();
    assert!(change(&mut value, device_type, name));
    let (project, _) = decode_json(
        &serde_json::to_vec(&value).unwrap(),
        &LoadOptions::default(),
    )
    .unwrap();
    let id = project.document.device_ids().next().unwrap();
    project.document.device(id).unwrap().name.clone()
}

// Spec: device-chains R1
/// T197: a set saved before the devices took Live's names opens with the
/// new name on a device still called what Beatlayer first called it, and
/// keeps the name the user gave one.
#[test]
fn a_set_saved_before_the_devices_took_live_names_opens_with_them() {
    assert_eq!(
        utility_loaded_as("beatlayer.squeeze", "Squeeze"),
        "Compressor"
    );
    assert_eq!(utility_loaded_as("bl.device.reprise", "Reprise"), "Delay");
    assert_eq!(
        utility_loaded_as("beatlayer.squeeze", "Vocal comp"),
        "Vocal comp"
    );
    assert_eq!(utility_loaded_as("beatlayer.utility", "Squeeze"), "Squeeze");
}

/// The version 1 set file, compressed as version 1 wrote it, loads.
#[test]
fn the_schema_1_set_file_loads() {
    let (project, info) = decode(&read("schema-1.blset"), &LoadOptions::default()).unwrap();
    assert_eq!(project, fixture_project());
    assert_eq!(
        info,
        SetInfo {
            revision: REVISION,
            saved_by: "Beatlayer 0.0.0".to_owned(),
            schema: 1,
        }
    );
}

/// What this build writes is exactly the current version's fixture. If this
/// fails, the saved form changed: raise [`SCHEMA_VERSION`], add a migration
/// from the old version, and add the new version's fixture.
#[test]
fn this_build_writes_the_current_fixture() {
    let fixture: Value =
        serde_json::from_slice(&read(&format!("schema-{SCHEMA_VERSION}.json"))).unwrap();
    assert_eq!(written_now(), fixture);
}

/// An invented version 0, which differed from version 1 in three ways: the
/// ID allocator was called `next_id`, there was no `revision` or `saved_by`,
/// and track volumes were in decibels (`gain_db`, with `null` for silence)
/// rather than linear gain.
fn fake_v0_to_v1(mut value: Value) -> Result<Value, String> {
    fn tracks(
        value: &mut Value,
        visit: &mut dyn FnMut(&mut Value) -> Result<(), String>,
    ) -> Result<(), String> {
        visit(value)?;
        if let Some(children) = value.get_mut("children").and_then(Value::as_array_mut) {
            for child in children {
                tracks(child, visit)?;
            }
        }
        Ok(())
    }
    fn gain_to_volume(track: &mut Value) -> Result<(), String> {
        let mixer = track
            .get_mut("mixer")
            .and_then(Value::as_object_mut)
            .ok_or("a track has no mixer")?;
        let volume = match mixer.remove("gain_db") {
            Some(Value::Null) => 0.0,
            Some(db) => {
                let db = db.as_f64().ok_or("gain_db isn't a number")?;
                10f64.powf(db / 20.0)
            }
            None => return Err("a mixer has no gain_db".to_owned()),
        };
        mixer.insert("volume".to_owned(), json!(volume));
        Ok(())
    }

    let set = value.as_object_mut().ok_or("not an object")?;
    let ids = set.remove("next_id").ok_or("no next_id")?;
    set.insert("ids".to_owned(), ids);
    // Version 0 didn't count saves.
    set.insert("revision".to_owned(), json!(0));
    set.insert("saved_by".to_owned(), json!("an earlier Beatlayer"));
    // A migration reads files from anywhere, so it never indexes in a way
    // that could panic.
    let document = set.get_mut("document").ok_or("no document")?;
    for list in ["tracks", "returns"] {
        let list = document
            .get_mut(list)
            .and_then(Value::as_array_mut)
            .ok_or("a track list is missing")?;
        for track in list {
            tracks(track, &mut gain_to_volume)?;
        }
    }
    let main = document.get_mut("main").ok_or("no main track")?;
    tracks(main, &mut gain_to_volume)?;
    Ok(value)
}

static FAKE_MIGRATIONS: &[Migration] = &[
    Migration {
        from: 0,
        summary: "an invented version 0, to test the scaffold",
        apply: fake_v0_to_v1,
    },
    // Then the real chain from version 1.
    MIGRATIONS[0],
    MIGRATIONS[1],
    MIGRATIONS[2],
    MIGRATIONS[3],
    MIGRATIONS[4],
    MIGRATIONS[5],
    MIGRATIONS[6],
    MIGRATIONS[7],
    MIGRATIONS[8],
    MIGRATIONS[9],
    MIGRATIONS[10],
    MIGRATIONS[11],
];

/// The scaffold migrates an old file: the invented version 0 fixture loads
/// as the fixture song once its migration is in the chain.
#[test]
fn a_fake_version_0_migrates() {
    let json = read("schema-0-fake.json");
    let fake = LoadOptions {
        migrations: FAKE_MIGRATIONS,
        ..LoadOptions::default()
    };
    let (project, info) = decode_json(&json, &fake).unwrap();

    // Decibels come back as the same gains, to within rounding.
    let mut expected = fixture_project();
    for (loaded, wanted) in project.document.tracks().zip(expected.document.tracks()) {
        let (a, b) = (loaded.mixer().volume, wanted.mixer().volume);
        assert!((a - b).abs() <= b * 1e-12 + 1e-300, "{a} vs {b}");
    }
    // Apart from those, the songs are the same.
    let volumes: Vec<(bl_model::TrackId, f64)> = project
        .document
        .tracks()
        .map(|track| (track.id(), track.mixer().volume))
        .collect();
    let mut editor = Editor::from_document(expected.document.clone(), expected.ids);
    for (track, volume) in volumes {
        editor.apply(Command::SetVolume { track, volume }).unwrap();
    }
    expected.document = editor.document().clone();
    assert_eq!(project, expected);
    assert_eq!(
        info,
        SetInfo {
            revision: 0,
            saved_by: "an earlier Beatlayer".to_owned(),
            schema: 0,
        }
    );

    // Compressed, through the whole loader, it's the same.
    let bytes = bl_project::format::compress(&json);
    assert_eq!(decode(&bytes, &fake).unwrap().0, project);

    // Without the migration, this build says the file is too old.
    let error = decode_json(&json, &LoadOptions::default()).unwrap_err();
    assert!(
        matches!(
            error,
            LoadError::TooOld {
                found: 0,
                oldest: 1
            }
        ),
        "{error:?}"
    );
    assert!(
        error
            .to_string()
            .contains("older than any this version reads")
    );

    // A migration that fails says so, naming the version.
    let mut broken: Value = serde_json::from_slice(&json).unwrap();
    broken["document"]["main"]["mixer"]["gain_db"] = json!("loud");
    let error = decode_json(&serde_json::to_vec(&broken).unwrap(), &fake).unwrap_err();
    assert!(
        matches!(&error, LoadError::Migration { from: 0, message } if message.contains("gain_db")),
        "{error:?}"
    );
}

// Spec: comping R3. A set saved with WP-2.13's take lanes, whose passes had
// no IDs, opens with each pass a clip of its lane under a new ID after the
// set's largest, and each lane with an ID of its own.
#[test]
fn a_schema_7_set_with_take_lanes_opens_with_ids_for_its_lanes_and_passes() {
    let mut value: Value = serde_json::from_slice(&read("schema-7.json")).unwrap();
    let ids = value["ids"].as_u64().unwrap();
    let pass = |name: &str, start: i64| {
        json!({ "name": name, "start": start, "length": 705_600,
                "data": { "media": "4b".repeat(32), "start": 0.5, "end": 1.5, "gain": 0.7 } })
    };
    fn kick(value: &mut Value) -> &mut Value {
        let group = &mut value["document"]["tracks"][0];
        group["children"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|track| track["name"] == "Kick")
            .unwrap()
    }
    kick(&mut value)["take_lanes"] = json!([
        { "name": "Take 1", "clips": [pass("a", 0), pass("b", 705_600)] },
        { "name": "Take 2", "clips": [pass("c", 0)] },
    ]);
    let (project, info) = decode_json(
        &serde_json::to_vec(&value).unwrap(),
        &LoadOptions::default(),
    )
    .unwrap();
    assert_eq!(info.schema, 7);
    let kick = project
        .document
        .tracks()
        .find(|track| track.name() == "Kick")
        .unwrap();
    let lanes: Vec<_> = kick.take_lanes().collect();
    assert_eq!(lanes.len(), 2);
    assert_eq!(lanes[0].name, "Take 1");
    assert_eq!(lanes[0].clips.len(), 2);
    // New IDs, after the set's allocator, in the order they were saved.
    let new: Vec<u64> = lanes
        .iter()
        .flat_map(|lane| {
            std::iter::once(lane.id.0.get()).chain(lane.clips.iter().map(|c| c.id.0.get()))
        })
        .collect();
    assert_eq!(new, (ids..ids + 5).collect::<Vec<_>>());
    assert!(
        project.ids.peek().get() >= ids + 5,
        "the allocator moved past them"
    );
    // A pass plays what it played, as a clip.
    let second = &lanes[0].clips[1];
    assert_eq!(
        (
            second.name.as_str(),
            second.start.get(),
            second.length.get()
        ),
        ("b", 705_600, 705_600)
    );
    let ClipData::Audio(audio) = &second.data else {
        panic!("{second:?}");
    };
    assert_eq!((audio.start, audio.end, audio.gain), (0.5, Some(1.5), 0.7));
    assert!(!second.mute);
    project.document.check_invariants().unwrap();

    // Saved again, it's the current version, and loads back as it is.
    let saved = encode_json(&project, 8).unwrap();
    let (again, info) = decode_json(&saved, &LoadOptions::default()).unwrap();
    assert_eq!(info.schema, SCHEMA_VERSION);
    assert_eq!(again.document, project.document);

    // An allocator behind the document, which the loader allows, doesn't
    // hand the passes IDs the set already uses.
    value["ids"] = json!(1);
    let (behind, _) = decode_json(
        &serde_json::to_vec(&value).unwrap(),
        &LoadOptions::default(),
    )
    .unwrap();
    behind.document.check_invariants().unwrap();
}

/// Simpler's device type and the IDs of the parameters the version 11
/// fixtures below set: Mode, Start, Length, Slice By and Regions.
const SIMPLER: &str = "beatlayer.snapshot";
const MODE: ParamId = ParamId(0);
const START: ParamId = ParamId(1);
const LENGTH: ParamId = ParamId(2);
const SLICE_BY: ParamId = ParamId(24);
const SLICE_REGIONS: ParamId = ParamId(27);

/// A set as version 11 saved it, with Simplers in all three modes: a Classic
/// one whose Start an envelope drives, a One-Shot one at Start 20 % and
/// Length 30 %, and a Slice one at Start 10 % and Length 60 % cutting two
/// slices. With `driven`, the One-Shot Simpler's Start is automated as well.
/// Each plays one note from a one-bar clip of a track of its own.
fn simplers_project(driven: bool) -> Project {
    let mut ids = IdAllocator::new();
    let main = bl_model::TrackId(ids.allocate());
    let mut editor = Editor::from_document(bl_model::Document::new(main, []), ids);
    let sample = ContentHash([3; 32]);
    let simplers = [
        ("Classic", 0.0, &[][..], 60, true),
        (
            "One-Shot",
            1.0,
            &[(START, 20.0), (LENGTH, 30.0)][..],
            60,
            driven,
        ),
        (
            "Slice",
            2.0,
            &[
                (START, 10.0),
                (LENGTH, 60.0),
                (SLICE_BY, 2.0),
                (SLICE_REGIONS, 2.0),
            ][..],
            61,
            false,
        ),
    ];
    for (index, (name, mode, params, key, automated)) in simplers.into_iter().enumerate() {
        let track = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Midi,
                name: name.to_owned(),
                parent: None,
                index,
            })
            .unwrap();
        let id = editor.new_id();
        let mut device = Device::new(
            id,
            name,
            DeviceKind::Native {
                device_type: SIMPLER.to_owned(),
            },
        )
        .with_role(DeviceRole::Instrument);
        device.set_media(0, Some(sample));
        device.params.insert(MODE, mode);
        device.params.extend(params.iter().copied());
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device,
            })
            .unwrap();
        if automated {
            editor
                .apply(Command::SetAutomationEnvelope {
                    track,
                    target: AutomationTarget::Device {
                        device: id,
                        param: START,
                    },
                    points: vec![Breakpoint {
                        at: Ticks::ZERO,
                        value: 25.0,
                        bend: 0.0,
                    }],
                })
                .unwrap();
        }
        let clip = editor.new_id();
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(
                    clip,
                    Ticks::ZERO,
                    Ticks::WHOLE,
                    ClipData::Midi(MidiData::new([Note::new(
                        key,
                        Ticks::ZERO,
                        Ticks::WHOLE,
                        100,
                    )])),
                ),
            })
            .unwrap();
    }
    Project {
        document: editor.document().clone(),
        ids: editor.ids(),
        media: [MediaRef {
            hash: sample,
            path: MediaPath::new("Samples/Imported/Ramp.wav").unwrap(),
            size: 96_044,
            reversed_from: None,
        }]
        .into_iter()
        .collect(),
        windows: bl_model::DeviceWindows::new(),
        rack_views: bl_model::RackViewMap::new(),
        folds: bl_model::ClipFolds::new(),
        scale_folds: bl_model::ClipFolds::new(),
        midi_tools: bl_project::midi_tools::MidiToolSettings::default(),
        launch: bl_model::LaunchSettings::default(),
        multisamplers: bl_model::MultisamplerViews::new(),
        window_views: bl_model::WindowViews::default(),
        mpe_view: bl_model::MpeView::default(),
        lane_views: bl_model::LaneViews::new(),
        column_widths: bl_model::ColumnWidths::new(),
    }
}

/// Creates the version 11 Simpler fixtures if they're missing: the song above
/// as that version saved it (a Simpler held its Start and Length as
/// parameters and had no region, so today's form with the version's number is
/// the same file). Run it once, by hand; it never overwrites a fixture.
#[test]
#[ignore = "creates fixture files; run by hand"]
fn write_simpler_fixtures() {
    for (name, driven) in [
        ("simplers-schema-11", false),
        ("simplers-driven-schema-11", true),
    ] {
        let path = fixtures().join(format!("{name}.json"));
        if path.exists() {
            continue;
        }
        let compact = encode_json(&simplers_project(driven), REVISION).unwrap();
        let mut value: Value = serde_json::from_slice(&compact).unwrap();
        value["schema"] = json!(11);
        value["saved_by"] = json!("Beatlayer 0.0.0");
        std::fs::write(&path, pretty(&serde_json::to_vec(&value).unwrap())).unwrap();
    }
}

/// A Simpler's name, region, and Start and Length parameters.
type SimplerRow = (String, Option<SampleRegion>, Option<f64>, Option<f64>);

/// The Simplers of a loaded `project`.
fn simplers_of(project: &Project) -> Vec<SimplerRow> {
    project
        .document
        .tracks()
        .flat_map(bl_model::Track::devices)
        .map(|d| {
            (
                d.name.clone(),
                d.region,
                d.params.get(&START).copied(),
                d.params.get(&LENGTH).copied(),
            )
        })
        .collect()
}

// Spec: sampler R24
/// A Classic Simpler is left as it is. A One-Shot or Slice one takes the part
/// its Start and Length chose as its region, as the shares they were, with
/// Start and Length back at their defaults; the next edit of the region would
/// write frames.
#[test]
fn a_set_saved_before_the_region_opens_its_one_shot_and_slice_simplers_with_one() {
    let (project, info) =
        decode_json(&read("simplers-schema-11.json"), &LoadOptions::default()).unwrap();
    assert_eq!(info.schema, 11);
    assert_eq!(
        simplers_of(&project),
        [
            ("Classic".to_owned(), None, None, None),
            (
                "One-Shot".to_owned(),
                Some(SampleRegion::Shares {
                    start_share: 0.2,
                    end_share: 0.5
                }),
                None,
                None
            ),
            (
                "Slice".to_owned(),
                Some(SampleRegion::Shares {
                    start_share: 0.1,
                    end_share: 0.7
                }),
                None,
                None
            ),
        ]
    );
    // Saved, it writes the new form and opens the same again.
    let (again, info) = decode(
        &encode(&project, REVISION).unwrap(),
        &LoadOptions::default(),
    )
    .unwrap();
    assert_eq!(info.schema, SCHEMA_VERSION);
    assert_eq!(again, project);
}

// Spec: sampler R24
/// Automation of a One-Shot or Slice Simpler's Start or Length can't move
/// its flags: the opened set names that device, once, and none of a set
/// whose only automated Start is a Classic Simpler's.
#[test]
fn the_simplers_whose_start_was_automated_are_named_once() {
    let options = LoadOptions::default();
    let (quiet, _) = decode_json(&read("simplers-schema-11.json"), &options).unwrap();
    assert!(bl_project::migrate::simplers_whose_start_was_driven(&quiet.document).is_empty());
    let (driven, _) = decode_json(&read("simplers-driven-schema-11.json"), &options).unwrap();
    assert_eq!(
        bl_project::migrate::simplers_whose_start_was_driven(&driven.document),
        ["One-Shot"]
    );
}

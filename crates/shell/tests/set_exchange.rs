//! Merging sets and saving tracks or Session clips as a set (WP-6.35; the
//! set exchange spec, R1 to R9 and R11 to R15). Driven through `Shell::run`,
//! as an agent drives it (D17), on real folders and sets.

use std::path::{Path, PathBuf};

use bl_core::time::{Bpm, TICKS_PER_QUARTER, TempoMap, Ticks};
use bl_engine::testing as engine;
use bl_model::{
    AudioData, AutomationTarget, Clip, ClipData, ClipId, Command, ContentHash, DeviceId, Editor,
    MidiData, Note, SceneId, TrackId, TrackKind,
};
use bl_project::{Media, MediaPath, MediaRef, Project, SetFile};
use bl_shell::browser_commands::{BrowserState, ItemKind};
use bl_shell::files::ProjectFiles;
use bl_shell::set_exchange_commands::{ExportedSet, Imported};
use bl_shell::settings::Preferences;
use bl_shell::{Shell, ShellError};
use serde_json::{Value, json};

const BEAT: i64 = TICKS_PER_QUARTER;

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-set-exchange-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Writes a short mono WAV file that isn't silence.
fn wav(path: &Path, frames: usize) {
    let samples: Vec<f32> = (0..frames)
        .map(|i| (i as f32 * 440.0 / 48_000.0 * std::f32::consts::TAU).sin() * 0.25)
        .collect();
    let bytes = bl_media::wav_bytes(
        &samples,
        1,
        48_000,
        bl_media::BitDepth::Int16,
        bl_media::Dither::None,
    )
    .unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// A shell with the null audio device and a User Library, on the default
/// set (four tracks and two returns).
fn shell_in(scratch: &Scratch) -> Shell {
    let mut shell = Shell::new(
        Preferences::default(),
        Some(scratch.path("config")),
        bl_shell::audio::AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    let library = scratch.path("User Library");
    bl_shell::browser::places::create_user_library(&library).unwrap();
    shell.set_user_library(Some(library));
    shell
}

fn run<T: serde::de::DeserializeOwned>(shell: &mut Shell, id: &str, args: Value) -> T {
    serde_json::from_value(
        shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}")),
    )
    .unwrap()
}

fn midi_clip(editor: &mut Editor, name: &str, notes: u8) -> Clip {
    let id: ClipId = editor.new_id();
    let notes =
        (0..notes).map(|i| Note::new(48 + i, Ticks(i64::from(i) * BEAT), Ticks(BEAT / 2), 90));
    let mut clip = Clip::new(
        id,
        Ticks::ZERO,
        Ticks(BEAT * 4),
        ClipData::Midi(MidiData::new(notes)),
    );
    clip.name = name.to_owned();
    clip
}

fn add_track(editor: &mut Editor, kind: TrackKind, name: &str, parent: Option<TrackId>) -> TrackId {
    let id: TrackId = editor.new_id();
    let index = match (kind, parent) {
        (TrackKind::Return, _) => editor.document().returns().len(),
        (_, Some(group)) => editor.document().track(group).unwrap().children().len(),
        _ => editor.document().top_level().len(),
    };
    editor
        .apply(Command::CreateTrack {
            id,
            kind,
            name: name.to_owned(),
            parent: if kind == TrackKind::Return {
                None
            } else {
                parent
            },
            index,
        })
        .unwrap();
    id
}

fn scene(editor: &Editor, position: usize) -> SceneId {
    editor.document().scenes().nth(position).unwrap().id
}

fn gain_device(editor: &mut Editor, track: TrackId) -> DeviceId {
    let id: DeviceId = editor.new_id();
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: engine::gain_type().device(id),
        })
        .unwrap();
    id
}

/// What a source set holds, for the tests to name.
struct Source {
    path: PathBuf,
    folder: PathBuf,
    drums: TrackId,
    bass: TrackId,
    pad: TrackId,
    group: TrackId,
    loop_clip: ClipId,
    sample: PathBuf,
}

/// A saved set in a project folder under `scratch`: a MIDI track "Drums" with
/// Session clips in scenes 1 and 4, a group "Strings" holding an audio track
/// "Bass" (playing a sample, sending to both returns) and an audio track
/// "Pad" (with a Gain device, automated), two return tracks, and a tempo of
/// 100 BPM.
fn source_set(scratch: &Scratch, name: &str) -> Source {
    let folder = scratch.path(&format!("{name} Project"));
    let sample = folder.join("Samples/Imported/Bass.wav");
    wav(&sample, 4800);
    let hash = bl_media::ContentHash::of_file(&sample).unwrap();
    let hash = ContentHash(*hash.as_bytes());
    let size = std::fs::metadata(&sample).unwrap().len();
    let mut media = Media::new();
    media.insert(MediaRef {
        hash,
        path: MediaPath::between(&folder, &sample).unwrap(),
        size,
        reversed_from: None,
    });

    let mut editor = Editor::new();
    editor
        .apply(Command::SetTempo {
            tempo: TempoMap::constant(Bpm::new(100.0).unwrap()),
        })
        .unwrap();
    let drums = add_track(&mut editor, TrackKind::Midi, "Drums", None);
    let group = add_track(&mut editor, TrackKind::Group, "Strings", None);
    let bass = add_track(&mut editor, TrackKind::Audio, "Bass", Some(group));
    let pad = add_track(&mut editor, TrackKind::Audio, "Pad", Some(group));
    let a = add_track(&mut editor, TrackKind::Return, "A Return", None);
    let b = add_track(&mut editor, TrackKind::Return, "B Return", None);
    {
        let clip = midi_clip(&mut editor, "Beat", 4);
        let first = scene(&editor, 0);
        editor
            .apply(Command::CreateSlotClip {
                track: drums,
                scene: first,
                clip,
            })
            .unwrap();
        let clip = midi_clip(&mut editor, "Fill", 2);
        let fourth = scene(&editor, 3);
        editor
            .apply(Command::CreateSlotClip {
                track: drums,
                scene: fourth,
                clip,
            })
            .unwrap();
    }
    let clip_id: ClipId = editor.new_id();
    let mut played = Clip::new(
        clip_id,
        Ticks::ZERO,
        Ticks(BEAT * 4),
        ClipData::Audio(AudioData::new(hash)),
    );
    played.name = "Bass line".to_owned();
    editor
        .apply(Command::CreateClip {
            track: bass,
            clip: played,
        })
        .unwrap();
    editor
        .apply(Command::SetSend {
            track: bass,
            to: a,
            level: 0.5,
        })
        .unwrap();
    editor
        .apply(Command::SetSend {
            track: bass,
            to: b,
            level: 0.25,
        })
        .unwrap();
    // An audio Session clip on Pad, in scene 2, playing the same sample.
    let loop_clip: ClipId = editor.new_id();
    let mut looped = Clip::new(
        loop_clip,
        Ticks::ZERO,
        Ticks(BEAT * 4),
        ClipData::Audio(AudioData::new(hash)),
    );
    looped.name = "Loop".to_owned();
    let second = scene(&editor, 1);
    editor
        .apply(Command::CreateSlotClip {
            track: pad,
            scene: second,
            clip: looped,
        })
        .unwrap();
    let pad_gain = gain_device(&mut editor, pad);
    editor
        .apply(Command::SetAutomationBreakpoint {
            track: pad,
            target: AutomationTarget::Device {
                device: pad_gain,
                param: engine::GAIN_PARAM,
            },
            at: Ticks::ZERO,
            value: 0.5,
        })
        .unwrap();
    let path = folder.join(format!("{name}.blset"));
    SetFile::new(path.clone())
        .save(&Project::from_editor(&editor, media))
        .unwrap();
    Source {
        path,
        folder,
        drums,
        bass,
        pad,
        group,
        loop_clip,
        sample,
    }
}

/// A new, saved set with one return track and two scenes: the set the others
/// merge into.
fn target_shell(scratch: &Scratch) -> Shell {
    let mut shell = shell_in(scratch);
    shell.run("set.new", json!({})).unwrap();
    shell.edit(|editor| {
        let doc = editor.document();
        let tracks: Vec<TrackId> = doc.top_level().chain(doc.returns().skip(1)).collect();
        for track in tracks {
            editor.apply(Command::DeleteTrack { track }).unwrap();
        }
        let scenes: Vec<SceneId> = editor.document().scenes().skip(2).map(|s| s.id).collect();
        for scene in scenes {
            editor.apply(Command::DeleteScene { scene }).unwrap();
        }
    });
    shell
}

fn names(shell: &Shell) -> Vec<String> {
    let doc = shell.editor().document();
    doc.track_order()
        .into_iter()
        .filter(|id| doc.track(*id).unwrap().kind() != TrackKind::Main)
        .map(|id| doc.track(id).unwrap().name().to_owned())
        .collect()
}

fn browser(shell: &mut Shell) -> BrowserState {
    run(shell, "browser.state", json!({}))
}

fn row_names(state: &BrowserState) -> Vec<(ItemKind, String, u16)> {
    state
        .rows
        .iter()
        .map(|row| (row.kind, row.name.clone(), row.depth))
        .collect()
}

// Spec: set-exchange R1 (a set opens like a folder and lists its tracks, a
// group's tracks inside it, its return tracks and Main; a track lists its
// Session clips by scene and a Devices row), R15 (the rows name what they are)
#[test]
fn a_set_opens_in_the_browser_to_its_tracks_clips_and_devices() {
    let scratch = Scratch::new("rows");
    let source = source_set(&scratch, "Source");
    let mut shell = shell_in(&scratch);
    run::<Value>(
        &mut shell,
        "browser.add_folder",
        json!({ "path": text(&scratch.0) }),
    );
    run::<Value>(
        &mut shell,
        "browser.open_folder",
        json!({ "path": text(&source.folder) }),
    );
    run::<Value>(
        &mut shell,
        "browser.open_folder",
        json!({ "path": text(&source.path) }),
    );
    shell.wait_for_browser_sets();
    let state = browser(&mut shell);
    let rows = row_names(&state);
    let set_at = rows
        .iter()
        .position(|(kind, name, _)| *kind == ItemKind::Set && name == "Source.blset")
        .expect("the set is listed");
    let depth = rows[set_at].2 + 1;
    let below: Vec<(ItemKind, &str)> = rows[set_at + 1..]
        .iter()
        .take_while(|row| row.2 >= depth)
        .map(|(kind, name, _)| (*kind, name.as_str()))
        .collect();
    assert_eq!(
        below,
        [
            (ItemKind::Track, "Drums"),
            (ItemKind::Group, "Strings"),
            (ItemKind::Return, "A Return"),
            (ItemKind::Return, "B Return"),
            (ItemKind::Main, "Main"),
        ],
        "the tracks in the set's order, then its returns and Main"
    );

    // A group opens to its tracks and its Devices row; a track to its
    // Session clips, by scene, and its Devices row.
    let key = |track: TrackId| format!("{}#t{}", text(&source.path), track.0.get());
    run::<Value>(
        &mut shell,
        "browser.open_folder",
        json!({ "path": key(source.group) }),
    );
    run::<Value>(
        &mut shell,
        "browser.open_folder",
        json!({ "path": key(source.drums) }),
    );
    let state = browser(&mut shell);
    let rows = row_names(&state);
    let from = rows.iter().position(|r| r.1 == "Drums").unwrap();
    let listed: Vec<(ItemKind, &str)> = rows[from..from + 4]
        .iter()
        .map(|(kind, name, _)| (*kind, name.as_str()))
        .collect();
    assert_eq!(
        listed,
        [
            (ItemKind::Track, "Drums"),
            (ItemKind::SessionClip, "Scene 1: Beat"),
            (ItemKind::SessionClip, "Scene 4: Fill"),
            (ItemKind::Devices, "Devices"),
        ]
    );
    let group = rows.iter().position(|r| r.1 == "Strings").unwrap();
    let inside: Vec<&str> = rows[group + 1..group + 4]
        .iter()
        .map(|r| r.1.as_str())
        .collect();
    assert_eq!(inside, ["Bass", "Pad", "Devices"]);
    assert!(
        state.rows.iter().any(|r| r.kind == ItemKind::SessionClip
            && r.clip_kind == Some(bl_shell::browser_commands::ClipFileKind::Midi)),
        "a Session clip says whether it holds audio or MIDI"
    );
    // Closing the set takes its rows away.
    run::<Value>(
        &mut shell,
        "browser.open_folder",
        json!({ "path": text(&source.path), "open": false }),
    );
    assert!(
        !browser(&mut shell)
            .rows
            .iter()
            .any(|r| r.kind == ItemKind::Track)
    );
}

// Spec: set-exchange R1 (a damaged set or a newer one opens to a line saying
// why and offers nothing), R3 (a set that can't be read changes nothing)
#[test]
fn a_newer_or_damaged_set_shows_why_and_merges_nothing() {
    let scratch = Scratch::new("refused");
    let source = source_set(&scratch, "Source");
    // A set from a later Beatlayer: the same set with a schema it doesn't
    // know.
    let (project, _) = bl_project::load(&source.path).unwrap();
    let json = String::from_utf8(bl_project::encode_json(&project, 1).unwrap()).unwrap();
    let schema = format!("\"schema\":{}", bl_project::SCHEMA_VERSION);
    assert!(json.contains(&schema));
    let newer = json.replacen(&schema, "\"schema\":9999", 1);
    let newer_path = source.folder.join("Newer.blset");
    std::fs::write(&newer_path, bl_project::format::compress(newer.as_bytes())).unwrap();
    let damaged_path = source.folder.join("Damaged.blset");
    std::fs::write(&damaged_path, b"this is not a set").unwrap();

    let mut shell = target_shell(&scratch);
    run::<Value>(
        &mut shell,
        "browser.add_folder",
        json!({ "path": text(&scratch.0) }),
    );
    run::<Value>(
        &mut shell,
        "browser.open_folder",
        json!({ "path": text(&source.folder) }),
    );
    for (file, why) in [
        (&newer_path, "newer version of Beatlayer"),
        (&damaged_path, ""),
    ] {
        run::<Value>(
            &mut shell,
            "browser.open_folder",
            json!({ "path": text(file), "keep_others": true }),
        );
        shell.wait_for_browser_sets();
        let state = browser(&mut shell);
        let at = state
            .rows
            .iter()
            .position(|r| r.path == text(file))
            .unwrap();
        let line = &state.rows[at + 1];
        assert_eq!(line.kind, ItemKind::Notice, "{file:?} lists a line");
        assert!(line.name.contains(why), "{}", line.name);
        assert!(!line.name.contains("Reading"), "it says why: {}", line.name);

        let before = names(&shell);
        let steps = shell.editor().undo_history().len();
        let error = shell
            .run("set.import", json!({ "path": text(file) }))
            .unwrap_err();
        assert!(matches!(error, ShellError::Unavailable { .. }), "{error}");
        assert_eq!(names(&shell), before, "nothing was merged");
        assert_eq!(shell.editor().undo_history().len(), steps);
    }
}

// Spec: set-exchange R3 to R7, R9 (a whole set merges but for its returns:
// sends by position, scenes added, one undo step), R15 (it reads back)
#[test]
fn a_set_merges_whole_but_for_its_returns_in_one_undo_step() {
    let scratch = Scratch::new("merge");
    let source = source_set(&scratch, "Source");
    let mut shell = target_shell(&scratch);
    let returns: Vec<TrackId> = shell.editor().document().returns().collect();
    assert_eq!(returns.len(), 1);
    assert_eq!(shell.editor().document().scene_count(), 2);
    let before = shell.editor().document().clone();
    let steps = shell.editor().undo_history().len();

    let imported: Imported = run(
        &mut shell,
        "set.import",
        json!({ "path": text(&source.path) }),
    );
    let doc = shell.editor().document();
    assert_eq!(
        names(&shell),
        ["Drums", "Strings", "Bass", "Pad", "A Return"]
    );
    assert_eq!(imported.tracks.len(), 2, "Drums and the group");
    assert_eq!(imported.scenes_added, 2);
    assert_eq!(
        doc.scene_count(),
        4,
        "the clips in scenes 1 and 4 keep their places"
    );
    assert_eq!(
        doc.returns().collect::<Vec<_>>(),
        returns,
        "its returns were left out"
    );
    assert_eq!(
        doc.settings().tempo,
        before.settings().tempo,
        "the tempo is the open set's"
    );

    let by_name = |name: &str| {
        doc.tracks()
            .find(|t| t.name() == name && t.kind() != TrackKind::Return)
            .unwrap()
    };
    let drums = by_name("Drums");
    let slots: Vec<usize> = drums
        .slots()
        .map(|(scene, _)| doc.scene_position(scene).unwrap())
        .collect();
    assert_eq!(slots.len(), 2);
    assert!(slots.contains(&0) && slots.contains(&3));
    let bass = by_name("Bass");
    assert_eq!(
        bass.sends().collect::<Vec<_>>(),
        [(returns[0], 0.5)],
        "the first send goes to the first return and the extra one is dropped"
    );
    assert_eq!(bass.clips().count(), 1, "its clip came with it");
    let group = doc.parent(bass.id()).expect("Bass is still in its group");
    assert_eq!(doc.track(group).unwrap().name(), "Strings");
    let pad = by_name("Pad");
    assert_eq!(pad.automation().count(), 1, "the automation came with it");
    let (target, _) = pad.automation().next().unwrap();
    let AutomationTarget::Device { device, .. } = target else {
        panic!("the envelope drives a device");
    };
    assert_eq!(
        *device,
        pad.devices().next().unwrap().id,
        "the automation drives the added track's own device"
    );

    // One undo step, which undoes it all.
    assert_eq!(shell.editor().undo_history().len(), steps + 1);
    assert_eq!(shell.editor().undo_history().next(), Some("Import Tracks"));
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(shell.editor().document(), &before);
    shell.run("edit.redo", json!({})).unwrap();
    assert_eq!(names(&shell).len(), 5);

    // The merge survives a save and a reopen.
    shell
        .run(
            "set.save_as",
            json!({ "path": text(&scratch.path("Merged.blset")) }),
        )
        .unwrap();
    let mut other = shell_in(&scratch);
    other
        .run(
            "set.open",
            json!({ "path": text(&scratch.path("Merged Project/Merged.blset")) }),
        )
        .unwrap();
    assert_eq!(names(&other), names(&shell));
    assert_eq!(other.editor().document().scene_count(), 4);
}

// Spec: set-exchange R4, R6 (one track merges alone, in the opened set's
// order; a routing to a track that isn't added goes back to its default)
#[test]
fn one_track_merges_alone_and_a_routing_to_a_track_left_behind_resets() {
    let scratch = Scratch::new("single");
    let source = source_set(&scratch, "Source");
    // Bass plays through Pad.
    let (mut project, _) = bl_project::load(&source.path).unwrap();
    let mut editor = project.editor();
    editor
        .apply(Command::SetOutput {
            track: source.bass,
            output: bl_model::Output::Track { track: source.pad },
        })
        .unwrap();
    project = Project::from_editor(&editor, project.media);
    SetFile::new(source.path.clone()).save(&project).unwrap();

    let mut shell = target_shell(&scratch);
    let imported: Imported = run(
        &mut shell,
        "set.import",
        json!({ "path": text(&source.path), "tracks": [source.bass] }),
    );
    assert_eq!(names(&shell), ["Bass", "A Return"]);
    let doc = shell.editor().document();
    let bass = doc.track(imported.tracks[0]).unwrap();
    assert_eq!(
        bass.routing().output,
        bl_model::Output::Default,
        "its output went back to Main: the track it named wasn't added"
    );
    assert_eq!(doc.parent(bass.id()), None, "it lands at the top level");
    assert_eq!(imported.scenes_added, 0);
    assert_eq!(doc.scene_count(), 2, "no clips, no scenes");

    // Several tracks come in the opened set's order, however they are named,
    // and a group brings its tracks.
    let imported: Imported = run(
        &mut shell,
        "set.import",
        json!({ "path": text(&source.path), "tracks": [source.group, source.drums, source.pad] }),
    );
    assert_eq!(
        names(&shell),
        ["Bass", "Drums", "Strings", "Bass", "Pad", "A Return"],
        "the group brings its tracks; Pad alone isn't added twice"
    );
    assert_eq!(imported.tracks.len(), 2);
}

// Spec: set-exchange R8, R9 (a Devices row adds the chain with its state and
// without automation, after the track's devices, as one undo step)
#[test]
fn a_devices_row_adds_the_chain_without_its_automation() {
    let scratch = Scratch::new("devices");
    let source = source_set(&scratch, "Source");
    let mut shell = target_shell(&scratch);
    let target = {
        let id: TrackId = shell.new_id();
        shell.edit(|editor| {
            editor
                .apply(Command::CreateTrack {
                    id,
                    kind: TrackKind::Audio,
                    name: "Mine".to_owned(),
                    parent: None,
                    index: 0,
                })
                .unwrap();
        });
        id
    };
    let steps = shell.editor().undo_history().len();
    let imported: Imported = run(
        &mut shell,
        "set.import",
        json!({ "path": text(&source.path), "devices": source.pad, "at": target }),
    );
    let doc = shell.editor().document();
    let mine = doc.track(target).unwrap();
    assert_eq!(mine.devices().count(), 1, "the chain came onto the track");
    assert_eq!(imported.devices.len(), 1);
    assert_eq!(mine.devices().next().unwrap().id, imported.devices[0]);
    assert_eq!(mine.automation().count(), 0, "without its automation");
    assert_eq!(doc.top_level().len(), 1, "no track was added");
    assert_eq!(shell.editor().undo_history().len(), steps + 1);
    assert_eq!(shell.editor().undo_history().next(), Some("Insert Device"));

    // In a drop area it makes a track of the chain's kind.
    let imported: Imported = run(
        &mut shell,
        "set.import",
        json!({ "path": text(&source.path), "devices": source.pad, "drop_area": true }),
    );
    let doc = shell.editor().document();
    let made = doc.track(imported.tracks[0]).unwrap();
    assert_eq!(
        made.kind(),
        TrackKind::Audio,
        "an audio effect makes an audio track"
    );
    assert_eq!(made.name(), "Pad");
    assert_eq!(made.devices().count(), 1);
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Insert Device and Track")
    );

    // A chain that doesn't fit the track's kind is refused and changes
    // nothing.
    let error = shell
        .run(
            "set.import",
            json!({ "path": text(&source.path), "devices": source.drums, "at": target }),
        )
        .unwrap_err();
    assert!(matches!(error, ShellError::Unavailable { .. }), "{error}");
}

// Spec: set-exchange R7 (a sample missing at its path is found by content;
// nothing is copied)
#[test]
fn a_sample_missing_at_its_path_is_relinked_by_content() {
    let scratch = Scratch::new("relink");
    let source = source_set(&scratch, "Source");
    // The sample moved within the project, so the media table is stale.
    let moved = source.folder.join("Samples/Moved/Bass renamed.wav");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&source.sample, &moved).unwrap();

    let mut shell = target_shell(&scratch);
    let imported: Imported = run(
        &mut shell,
        "set.import",
        json!({ "path": text(&source.path), "tracks": [source.bass] }),
    );
    assert_eq!(imported.missing_files, 0);
    let doc = shell.editor().document();
    let clip = doc
        .track(imported.tracks[0])
        .unwrap()
        .clips()
        .next()
        .unwrap();
    let ClipData::Audio(audio) = &clip.data else {
        panic!("an audio clip");
    };
    let entry = shell
        .media_store()
        .entry(bl_media::ContentHash::from_bytes(audio.media.0))
        .expect("the media store knows the file");
    assert_eq!(entry.path, moved, "found where it moved to, not copied");

    // A file that is gone altogether is listed as missing, and the track
    // still comes.
    std::fs::remove_file(&moved).unwrap();
    let mut shell = target_shell(&scratch);
    let imported: Imported = run(
        &mut shell,
        "set.import",
        json!({ "path": text(&source.path), "tracks": [source.bass] }),
    );
    assert_eq!(imported.missing_files, 1);
    assert_eq!(imported.tracks.len(), 1);
}

// Spec: set-exchange R11, R13, R14 (tracks saved as a set hold the open set's
// returns, tempo and scenes; the files are copied, so the new set stands
// alone; the open set is untouched)
#[test]
fn tracks_saved_as_a_set_open_on_their_own_and_stand_alone() {
    let scratch = Scratch::new("export-tracks");
    let source = source_set(&scratch, "Source");
    let mut shell = shell_in(&scratch);
    shell
        .run("set.open", json!({ "path": text(&source.path) }))
        .unwrap();
    run::<Value>(
        &mut shell,
        "browser.add_folder",
        json!({ "path": text(&scratch.0) }),
    );
    let folder = scratch.path("Saved");
    std::fs::create_dir_all(&folder).unwrap();
    let steps = shell.editor().undo_history().len();
    let before = shell.editor().document().clone();

    let saved: ExportedSet = run(
        &mut shell,
        "set.export_tracks",
        json!({ "tracks": [source.bass, source.drums], "folder": text(&folder) }),
    );
    // Named after the open set, placed as Save As places it.
    assert_eq!(saved.name, "Source");
    assert_eq!(
        PathBuf::from(&saved.path),
        folder.join("Source Project/Source.blset")
    );
    assert_eq!(
        saved.copied, 1,
        "the sample was copied (Collect Files: Always)"
    );
    assert_eq!(
        shell.editor().document(),
        &before,
        "the open set is unchanged"
    );
    assert_eq!(shell.editor().undo_history().len(), steps);

    // The new row is selected with its name ready to edit.
    let state = browser(&mut shell);
    assert_eq!(state.renaming.as_deref(), Some(saved.path.as_str()));
    assert!(state.selected.contains(&saved.path));

    // Move the source project away: the new set stands alone.
    std::fs::rename(&source.folder, scratch.path("Moved Source")).unwrap();
    let mut other = shell_in(&scratch);
    other
        .run("set.open", json!({ "path": saved.path }))
        .unwrap();
    assert_eq!(names(&other), ["Drums", "Bass", "A Return", "B Return"]);
    let doc = other.editor().document();
    assert_eq!(doc.returns().count(), 2, "the open set's returns came too");
    assert_eq!(doc.settings().tempo, before.settings().tempo, "its tempo");
    assert_eq!(doc.scene_count(), before.scene_count(), "its scenes");
    let bass = doc.tracks().find(|t| t.name() == "Bass").unwrap();
    assert_eq!(bass.sends().count(), 2, "the sends play as they did");
    assert_eq!(
        doc.parent(bass.id()),
        None,
        "a track saved alone isn't in its group"
    );
    let clip = bass.clips().next().unwrap();
    let ClipData::Audio(audio) = &clip.data else {
        panic!("an audio clip");
    };
    let file = other
        .media_store()
        .entry(bl_media::ContentHash::from_bytes(audio.media.0))
        .unwrap();
    assert!(file.path.is_file(), "{}", file.path.display());
    assert!(
        file.path
            .starts_with(PathBuf::from(&saved.path).parent().unwrap())
    );
}

// Spec: set-exchange R12, R13 (Session clips saved as a set: a track for each
// track, the clips laid out by scene from the first)
#[test]
fn session_clips_saved_as_a_set_keep_their_layout_by_scene() {
    let scratch = Scratch::new("export-clips");
    let source = source_set(&scratch, "Source");
    let mut shell = shell_in(&scratch);
    shell
        .run("set.open", json!({ "path": text(&source.path) }))
        .unwrap();
    // Two more Session clips on the Drums track, in scenes 2 and 5.
    let (second, fifth) = {
        let doc = shell.editor().document();
        (
            doc.scenes().nth(1).unwrap().id,
            doc.scenes().nth(4).unwrap().id,
        )
    };
    let mut ids = Vec::new();
    for scene in [second, fifth] {
        let id: ClipId = shell.new_id();
        ids.push(id);
        shell.edit(|editor| {
            let mut clip = Clip::new(
                id,
                Ticks::ZERO,
                Ticks(BEAT * 4),
                ClipData::Midi(MidiData::new([Note::new(
                    60,
                    Ticks::ZERO,
                    Ticks(BEAT),
                    100,
                )])),
            );
            clip.name = "Extra".to_owned();
            editor
                .apply(Command::CreateSlotClip {
                    track: source.drums,
                    scene,
                    clip,
                })
                .unwrap();
        });
    }
    let folder = scratch.path("Saved");
    std::fs::create_dir_all(&folder).unwrap();
    let before = shell.editor().document().clone();
    let saved: ExportedSet = run(
        &mut shell,
        "set.export_clips",
        json!({ "clips": ids, "folder": text(&folder) }),
    );
    assert_eq!(saved.name, "Source Clips");
    assert_eq!(shell.editor().document(), &before);

    let (project, _) = bl_project::load(&saved.path).unwrap();
    let doc = &project.document;
    assert_eq!(doc.scene_count(), 4, "scenes 2 to 5 of the open set");
    assert_eq!(
        doc.top_level().len(),
        1,
        "a track for the track they are on"
    );
    let drums = doc.track(doc.top_level().next().unwrap()).unwrap();
    assert_eq!(drums.name(), "Drums");
    let slots: Vec<usize> = drums
        .slots()
        .map(|(scene, _)| doc.scene_position(scene).unwrap())
        .collect();
    assert_eq!(slots.len(), 2);
    assert!(slots.contains(&0) && slots.contains(&3), "{slots:?}");
    assert_eq!(drums.clips().count(), 0, "none of its other clips");
    assert_eq!(doc.returns().count(), 0);
    assert_eq!(doc.settings().tempo, before.settings().tempo);
}

/// Opens `source` in `shell`'s browser, with its rows of `tracks` open.
fn open_in_browser(shell: &mut Shell, scratch: &Scratch, source: &Source) {
    run::<Value>(
        shell,
        "browser.add_folder",
        json!({ "path": text(&scratch.0) }),
    );
    run::<Value>(
        shell,
        "browser.open_folder",
        json!({ "path": text(&source.folder) }),
    );
    run::<Value>(
        shell,
        "browser.open_folder",
        json!({ "path": text(&source.path) }),
    );
    shell.wait_for_browser_sets();
}

fn row_key(source: &Source, row: &str, id: u64) -> String {
    format!("{}#{row}{id}", text(&source.path))
}

// Spec: set-exchange R4, R8 (Enter on a track of an opened set adds it after
// the last track, and on a Devices row adds the chain to the selected track)
#[test]
fn enter_on_a_track_or_a_devices_row_adds_it() {
    let scratch = Scratch::new("enter");
    let source = source_set(&scratch, "Source");
    let mut shell = target_shell(&scratch);
    open_in_browser(&mut shell, &scratch, &source);
    // Nothing selected in the set: the track goes after the last track.
    run::<Value>(
        &mut shell,
        "browser.select",
        json!({ "paths": [row_key(&source, "t", source.drums.0.get())] }),
    );
    shell.run("browser.load_selected", json!({})).unwrap();
    assert_eq!(names(&shell), ["Drums", "A Return"]);
    assert_eq!(shell.editor().undo_history().next(), Some("Import Tracks"));
    // The Devices row of Pad goes onto the selected track, a new audio track.
    let mine = {
        let id: TrackId = shell.new_id();
        shell.edit(|editor| {
            editor
                .apply(Command::CreateTrack {
                    id,
                    kind: TrackKind::Audio,
                    name: "Mine".to_owned(),
                    parent: None,
                    index: 1,
                })
                .unwrap();
        });
        id
    };
    shell
        .run("arrangement.select_tracks", json!({ "tracks": [mine] }))
        .unwrap();
    run::<Value>(
        &mut shell,
        "browser.select",
        json!({ "paths": [row_key(&source, "d", source.pad.0.get())] }),
    );
    shell.run("browser.load_selected", json!({})).unwrap();
    let doc = shell.editor().document();
    assert_eq!(doc.track(mine).unwrap().devices().count(), 1);
}

// Spec: set-exchange R2 (a Session clip of an opened set loads as a clip file,
// with its track's chain, its audio found from that set's project folder)
#[test]
fn a_session_clip_of_an_opened_set_loads_as_a_clip_file() {
    let scratch = Scratch::new("clip-row");
    let source = source_set(&scratch, "Source");
    let mut shell = target_shell(&scratch);
    open_in_browser(&mut shell, &scratch, &source);
    run::<Value>(
        &mut shell,
        "browser.select",
        json!({ "paths": [row_key(&source, "c", source.loop_clip.0.get())] }),
    );
    let added: bl_shell::browser_commands::AddResult =
        run(&mut shell, "browser.load_selected", json!({}));
    assert_eq!(added.clips.len(), 1);
    let doc = shell.editor().document();
    let track = doc.track(added.clips[0].track).unwrap();
    assert_eq!(track.kind(), TrackKind::Audio);
    assert_eq!(track.name(), "Loop", "named after the clip");
    assert_eq!(track.devices().count(), 1, "its track's chain came with it");
    let clip = doc.clip(added.clips[0].clip).unwrap();
    let ClipData::Audio(audio) = &clip.data else {
        panic!("an audio clip");
    };
    let file = shell
        .media_store()
        .entry(bl_media::ContentHash::from_bytes(audio.media.0))
        .expect("the file was found");
    assert_eq!(file.path, source.sample, "from the set's project folder");

    // A MIDI Session clip lands on a MIDI track.
    let midi = shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [row_key(
                &source,
                "c",
                {
                    let (project, _) = bl_project::load(&source.path).unwrap();
                    project.document.track(source.drums).unwrap().slots().next().unwrap().1.id.0.get()
                }
            )] }),
        )
        .unwrap();
    let track = TrackId(bl_core::id::Id::new(midi["clips"][0]["track"].as_u64().unwrap()).unwrap());
    assert_eq!(
        shell.editor().document().track(track).unwrap().kind(),
        TrackKind::Midi
    );
}

// Spec: set-exchange R13 (one track names the set; a name given wins; typing
// a name and Enter rename the new row), R14 (Never leaves the files where
// they are)
#[test]
fn a_set_of_one_track_is_named_after_it_and_refers_to_the_files_with_never() {
    let scratch = Scratch::new("never");
    let source = source_set(&scratch, "Source");
    let mut shell = shell_in(&scratch);
    shell
        .run("set.open", json!({ "path": text(&source.path) }))
        .unwrap();
    run::<Value>(
        &mut shell,
        "browser.add_folder",
        json!({ "path": text(&scratch.0) }),
    );
    shell
        .run(
            "settings.set_collect_files_on_export",
            json!({ "mode": "never" }),
        )
        .unwrap();
    let folder = scratch.path("Saved");
    std::fs::create_dir_all(&folder).unwrap();
    let saved: ExportedSet = run(
        &mut shell,
        "set.export_tracks",
        json!({ "tracks": [source.bass], "folder": text(&folder) }),
    );
    assert_eq!(saved.name, "Bass", "named after the track");
    assert_eq!(saved.copied, 0, "Never copies nothing");
    let (project, _) = bl_project::load(&saved.path).unwrap();
    let file = project.media.iter().next().expect("the sample is listed");
    let at = file
        .path
        .resolve(PathBuf::from(&saved.path).parent().unwrap())
        .unwrap();
    assert!(at.is_file(), "it names the file where it is: {at:?}");
    assert_eq!(
        std::fs::canonicalize(&at).unwrap(),
        std::fs::canonicalize(&source.sample).unwrap()
    );

    // The new row's name is open for editing; a name and Enter rename it.
    let state = browser(&mut shell);
    assert_eq!(state.renaming.as_deref(), Some(saved.path.as_str()));
    run::<Value>(
        &mut shell,
        "browser.rename_file",
        json!({ "name": "Bass Only" }),
    );
    assert!(
        PathBuf::from(&saved.path)
            .with_file_name("Bass Only.blset")
            .is_file()
    );

    // A second set of the same name in the same folder gets a folder of its
    // own, numbered.
    let again: ExportedSet = run(
        &mut shell,
        "set.export_tracks",
        json!({ "tracks": [source.bass], "folder": text(&folder), "name": "Bass" }),
    );
    assert_eq!(
        PathBuf::from(&again.path),
        folder.join("Bass Project 2/Bass.blset"),
        "the name was taken, so the project folder is numbered"
    );
}

// Spec: set-exchange R14 (under Ask the command waits for the answer)
#[test]
fn ask_waits_for_the_answer_before_saving_a_set() {
    let scratch = Scratch::new("ask");
    let source = source_set(&scratch, "Source");
    let mut shell = shell_in(&scratch);
    shell
        .run("set.open", json!({ "path": text(&source.path) }))
        .unwrap();
    shell
        .run(
            "settings.set_collect_files_on_export",
            json!({ "mode": "ask" }),
        )
        .unwrap();
    let folder = scratch.path("Saved");
    std::fs::create_dir_all(&folder).unwrap();
    let args = json!({ "tracks": [source.bass], "folder": text(&folder) });
    let error = shell.run("set.export_tracks", args.clone()).unwrap_err();
    assert!(matches!(error, ShellError::Unavailable { .. }), "{error}");
    assert_eq!(
        std::fs::read_dir(&folder).unwrap().count(),
        0,
        "nothing was written"
    );
    let mut with_answer = args;
    with_answer["copy"] = json!(true);
    let saved: ExportedSet = run(&mut shell, "set.export_tracks", with_answer);
    assert_eq!(saved.copied, 1);
    // Tracks without files need no answer.
    let saved: ExportedSet = run(
        &mut shell,
        "set.export_tracks",
        json!({ "tracks": [source.drums], "folder": text(&folder) }),
    );
    assert_eq!(saved.copied, 0);
}

// Spec: set-exchange R13 (a set already there under the name is kept; the new
// one is numbered)
#[test]
fn a_set_saved_under_a_name_already_in_the_project_does_not_replace_it() {
    let scratch = Scratch::new("numbered");
    let source = source_set(&scratch, "Source");
    let mut shell = shell_in(&scratch);
    shell
        .run("set.open", json!({ "path": text(&source.path) }))
        .unwrap();
    let args = json!({ "tracks": [source.bass], "folder": text(&source.folder), "name": "Source" });
    let first: ExportedSet = run(&mut shell, "set.export_tracks", args.clone());
    let second: ExportedSet = run(&mut shell, "set.export_tracks", args);
    assert_eq!(
        PathBuf::from(&first.path),
        source.folder.join("Source 2.blset")
    );
    assert_eq!(
        PathBuf::from(&second.path),
        source.folder.join("Source 3.blset")
    );
    let (kept, _) = bl_project::load(&source.path).unwrap();
    assert_eq!(
        kept.document.track_count(),
        7,
        "the set that was there is untouched"
    );
}

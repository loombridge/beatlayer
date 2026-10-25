//! Clip files (WP-6.34; the clip files spec): saving a clip to the browser,
//! the Clips label, placing a clip file in a set, previewing one, and
//! Collect Files on Export (file management R7 to R9). Driven through
//! `Shell::run`, as an agent drives it (D17), on real files.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use bl_core::time::{Bpm, TICKS_PER_QUARTER, TempoMap, Ticks};
use bl_engine::testing as engine;
use bl_media::BitDepth;
use bl_model::{
    AutomationTarget, Clip, ClipData, ClipEnvelope, ClipId, DeviceId, Envelope, MidiData,
    MixerParam, Note, SceneId, TrackId, TrackKind,
};
use bl_project::clip_file::ClipFile;
use bl_shell::browser_commands::{
    AddResult, BrowserState, ClipFileKind, ItemKind, SaveClipFileResult,
};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::{Shell, ShellError, testing};
use serde_json::{Value, json};

const BEAT: i64 = TICKS_PER_QUARTER;

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-clip-files-{name}-{}-{}",
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
    let bytes =
        bl_media::wav_bytes(&samples, 1, 48_000, BitDepth::Int16, bl_media::Dither::None).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// A shell with its own preferences and a User Library, on the default set.
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

fn library(scratch: &Scratch) -> PathBuf {
    scratch.path("User Library")
}

/// The default set's tracks: two MIDI, two audio.
fn tracks(shell: &Shell, kind: TrackKind) -> Vec<TrackId> {
    let doc = shell.editor().document();
    doc.top_level()
        .filter(|id| doc.track(*id).is_some_and(|t| t.kind() == kind))
        .collect()
}

fn steps(shell: &Shell) -> usize {
    shell.editor().undo_history().len()
}

fn run<T: serde::de::DeserializeOwned>(shell: &mut Shell, id: &str, args: Value) -> T {
    serde_json::from_value(
        shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}")),
    )
    .unwrap()
}

fn gain() -> Value {
    let t = engine::gain_type();
    json!({ "kind": t.kind, "name": t.name, "role": t.role })
}

fn add_gain(shell: &mut Shell, track: TrackId) -> DeviceId {
    shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let added: bl_shell::device_commands::DeviceAdded =
        run(shell, "device.add", json!({ "device": gain() }));
    added.device
}

fn devices(shell: &Shell, track: TrackId) -> Vec<DeviceId> {
    shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .devices()
        .map(|d| d.id)
        .collect()
}

/// A MIDI clip of four notes on `track`, with an envelope on the track's
/// volume and, when `device` is given, on that device's gain.
fn midi_clip(
    shell: &mut Shell,
    track: TrackId,
    name: &str,
    start: i64,
    device: Option<DeviceId>,
) -> ClipId {
    let id: ClipId = shell.new_id();
    let notes = (0..4).map(|i| {
        let mut note = Note::new(
            48 + i as u8 * 4,
            Ticks(i * BEAT),
            Ticks(BEAT / 2),
            90 + i as u8,
        );
        note.probability = 0.75;
        note
    });
    let mut clip = Clip::new(
        id,
        Ticks(start),
        Ticks(BEAT * 4),
        ClipData::Midi(MidiData::new(notes)),
    );
    clip.name = name.to_owned();
    let points = |value| {
        Envelope::from_points(vec![
            bl_model::Breakpoint {
                at: Ticks::ZERO,
                value,
                bend: 0.0,
            },
            bl_model::Breakpoint {
                at: Ticks(BEAT),
                value: value / 2.0,
                bend: 0.0,
            },
        ])
        .unwrap()
    };
    clip.envelopes.push(ClipEnvelope::new(
        AutomationTarget::Mixer {
            param: MixerParam::Volume,
        },
        points(0.5),
    ));
    if let Some(device) = device {
        clip.envelopes.push(ClipEnvelope::new(
            AutomationTarget::Device {
                device,
                param: engine::GAIN_PARAM,
            },
            points(0.25),
        ));
    }
    shell
        .run("clip.create", json!({ "track": track, "clip": clip }))
        .unwrap();
    id
}

/// An audio clip of `wav` on `track`, placed by the browser.
fn audio_clip(shell: &mut Shell, track: TrackId, wav: &Path) -> ClipId {
    let placed: AddResult = run(
        shell,
        "browser.add_to_arrangement",
        json!({ "paths": [text(wav)], "track": track }),
    );
    placed.clips[0].clip
}

fn save(shell: &mut Shell, clip: ClipId, extra: Value) -> SaveClipFileResult {
    let mut args = json!({ "clip": clip });
    for (key, value) in extra.as_object().unwrap() {
        args[key] = value.clone();
    }
    run(shell, "clip.save_file", args)
}

fn state(shell: &mut Shell) -> BrowserState {
    run(shell, "browser.state", json!({ "limit": 1000 }))
}

fn clip_count(shell: &Shell, track: TrackId) -> usize {
    shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .clips()
        .len()
}

fn refused(shell: &mut Shell, id: &str, args: Value) -> ShellError {
    let before = shell.editor().document().clone();
    let error = shell.run(id, args).expect_err("the command is refused");
    assert_eq!(
        *shell.editor().document(),
        before,
        "a refused command changes nothing"
    );
    error
}

// Spec: clip files R3, R4, R15.
#[test]
fn a_clip_is_saved_named_and_its_row_opened_for_renaming() {
    let scratch = Scratch::new("save");
    let mut shell = shell_in(&scratch);
    let midi = tracks(&shell, TrackKind::Midi)[0];
    let clip = midi_clip(&mut shell, midi, "Idea", 0, None);
    let before = steps(&shell);

    let saved = save(&mut shell, clip, json!({}));
    assert_eq!(
        saved.path,
        text(&library(&scratch).join("Clips").join("Idea.blclip"))
    );
    assert_eq!(
        (saved.name.as_str(), saved.kind),
        ("Idea", ClipFileKind::Midi)
    );
    assert!(Path::new(&saved.path).is_file());
    assert_eq!(steps(&shell), before, "saving a file is no undo step");

    // The new row is selected with its name ready to edit.
    let browser = state(&mut shell);
    assert_eq!(browser.selected, std::slice::from_ref(&saved.path));
    assert_eq!(browser.renaming.as_deref(), Some(saved.path.as_str()));
    let row = browser
        .rows
        .iter()
        .find(|row| row.path == saved.path)
        .unwrap();
    assert_eq!(
        (row.kind, row.clip_kind),
        (ItemKind::Clip, Some(ClipFileKind::Midi))
    );

    // A name already taken gets " 2"; an unnamed clip takes its track's name.
    let second = save(&mut shell, clip, json!({}));
    assert!(second.path.ends_with("Idea 2.blclip"));
    let unnamed = midi_clip(&mut shell, midi, "", BEAT * 8, None);
    let track_name = shell
        .editor()
        .document()
        .track(midi)
        .unwrap()
        .name()
        .to_owned();
    assert_eq!(save(&mut shell, unnamed, json!({})).name, track_name);

    // Typing a name and pressing Enter renames the file; Esc keeps the name.
    let saved = save(&mut shell, clip, json!({ "name": "Bass line" }));
    assert!(saved.path.ends_with("Bass line.blclip"));
    let browser: BrowserState = run(
        &mut shell,
        "browser.rename_file",
        json!({ "name": "Bass idea" }),
    );
    let renamed = library(&scratch).join("Clips").join("Bass idea.blclip");
    assert!(renamed.is_file() && !Path::new(&saved.path).exists());
    assert_eq!(browser.renaming, None);
    assert_eq!(browser.selected, [text(&renamed)]);
    let kept = save(&mut shell, clip, json!({ "name": "Kept" }));
    let browser: BrowserState = run(&mut shell, "browser.rename_file", json!({}));
    assert_eq!(browser.renaming, None);
    assert!(Path::new(&kept.path).is_file(), "Esc keeps the name");
    // A name another file has is refused, and no name is open any longer.
    assert!(
        shell
            .run("browser.rename_file", json!({ "name": "Kept" }))
            .is_err()
    );
    let again = save(&mut shell, clip, json!({ "name": "Other" }));
    let error = shell
        .run("browser.rename_file", json!({ "name": "Kept" }))
        .expect_err("taken");
    assert!(error.to_string().contains("already"), "{error}");
    assert!(Path::new(&again.path).is_file());
}

// Spec: clip files R3. A Session clip saves the same way, into any folder.
#[test]
fn a_session_clip_is_saved_into_a_folder_of_the_person_s() {
    let scratch = Scratch::new("session-save");
    let mut shell = shell_in(&scratch);
    let midi = tracks(&shell, TrackKind::Midi)[0];
    let scene = shell.editor().document().scenes().next().unwrap().id;
    let id: ClipId = shell.new_id();
    shell
        .run(
            "clip.create_in_slot",
            json!({ "track": midi, "scene": scene, "clip": Clip::new(
                id, Ticks::ZERO, Ticks(BEAT * 4),
                ClipData::Midi(MidiData::new([Note::new(60, Ticks::ZERO, Ticks(BEAT), 100)])),
            ) }),
        )
        .unwrap();
    let folder = scratch.path("Friends");
    let saved = save(
        &mut shell,
        id,
        json!({ "folder": text(&folder), "name": "Slot idea" }),
    );
    assert_eq!(saved.path, text(&folder.join("Slot idea.blclip")));
    let file = ClipFile::load(Path::new(&saved.path)).unwrap();
    assert_eq!(file.clip.start, Ticks::ZERO);
    // A clip that isn't there is refused.
    let missing: ClipId = shell.new_id();
    refused(&mut shell, "clip.save_file", json!({ "clip": missing }));
}

// Spec: clip files R8, R9, R11, R12.
#[test]
fn a_clip_file_on_an_empty_track_brings_its_chain_and_on_a_full_one_only_the_clip() {
    let scratch = Scratch::new("place");
    let mut shell = shell_in(&scratch);
    let midis = tracks(&shell, TrackKind::Midi);
    let source = midis[0];
    let gain_id = add_gain(&mut shell, source);
    let clip = midi_clip(&mut shell, source, "Idea", 0, Some(gain_id));
    let file = PathBuf::from(save(&mut shell, clip, json!({})).path);
    let saved = ClipFile::load(&file).unwrap();
    assert_eq!(saved.devices.len(), 1, "the track's chain is in the file");

    // On an empty MIDI track the chain comes first, with new IDs; one undo
    // step named "Insert Clip".
    let empty = midis[1];
    let before = shell.editor().document().clone();
    let steps_before = steps(&shell);
    let placed: AddResult = run(
        &mut shell,
        "browser.add_to_arrangement",
        json!({ "paths": [text(&file)], "track": empty, "at": BEAT * 8 }),
    );
    assert_eq!(steps(&shell), steps_before + 1);
    assert_eq!(
        shell.editor().undo_history().next().unwrap_or_default(),
        "Insert Clip"
    );
    let doc = shell.editor().document();
    let brought = devices(&shell, empty);
    assert_eq!(brought.len(), 1);
    assert_ne!(brought[0], gain_id, "new IDs");
    let placed_clip = doc.clip(placed.clips[0].clip).unwrap();
    assert_eq!(
        (placed_clip.start, placed_clip.length),
        (Ticks(BEAT * 8), Ticks(BEAT * 4))
    );
    let (ClipData::Midi(a), ClipData::Midi(b)) = (&placed_clip.data, &doc.clip(clip).unwrap().data)
    else {
        panic!("MIDI clips");
    };
    assert_eq!(a.notes, b.notes, "every note value comes back");
    // The envelope on the gain follows the new device; the mixer's stays.
    let targets: Vec<AutomationTarget> = placed_clip.envelopes.iter().map(|e| e.target).collect();
    assert!(targets.contains(&AutomationTarget::Device {
        device: brought[0],
        param: engine::GAIN_PARAM
    }));
    assert!(targets.contains(&AutomationTarget::Mixer {
        param: MixerParam::Volume
    }));
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(*shell.editor().document(), before, "undo takes it all away");
    shell.run("edit.redo", json!({})).unwrap();

    // On a track that has devices and clips only the clip comes, and the
    // clip's envelope on a device that didn't come is left out.
    let devices_before = devices(&shell, source);
    let clips_before = clip_count(&shell, source);
    let placed: AddResult = run(
        &mut shell,
        "browser.add_to_arrangement",
        json!({ "paths": [text(&file)], "track": source, "at": BEAT * 16 }),
    );
    assert_eq!(devices(&shell, source), devices_before);
    assert_eq!(clip_count(&shell, source), clips_before + 1);
    let kept = shell
        .editor()
        .document()
        .clip(placed.clips[0].clip)
        .unwrap();
    assert_eq!(kept.envelopes.len(), 1);
    assert_eq!(
        kept.envelopes[0].target,
        AutomationTarget::Mixer {
            param: MixerParam::Volume
        }
    );

    // A track with only a device brings no devices either (it isn't empty).
    let device_only: TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            json!({ "id": device_only, "kind": "midi", "name": "Pad", "index": 4 }),
        )
        .unwrap();
    let held = add_gain(&mut shell, device_only);
    run::<AddResult>(
        &mut shell,
        "browser.add_to_arrangement",
        json!({ "paths": [text(&file)], "track": device_only }),
    );
    assert_eq!(devices(&shell, device_only), [held]);
}

// Spec: clip files R7. Nothing changes when a clip file can't go.
#[test]
fn a_clip_file_that_cannot_go_where_it_is_dropped_changes_nothing() {
    let scratch = Scratch::new("refuse");
    let mut shell = shell_in(&scratch);
    let midi = tracks(&shell, TrackKind::Midi)[0];
    let audio = tracks(&shell, TrackKind::Audio)[0];
    let clip = midi_clip(&mut shell, midi, "Idea", 0, None);
    let file = text(Path::new(&save(&mut shell, clip, json!({})).path));
    let scene = shell.editor().document().scenes().next().unwrap().id;

    refused(
        &mut shell,
        "browser.add_to_arrangement",
        json!({ "paths": [file], "track": audio }),
    );
    refused(
        &mut shell,
        "session.drop_files",
        json!({ "paths": [file], "track": audio, "scene": scene }),
    );
    // A slot that already holds a clip takes none.
    run::<Value>(
        &mut shell,
        "session.drop_files",
        json!({ "paths": [file], "track": midi, "scene": scene }),
    );
    refused(
        &mut shell,
        "session.drop_files",
        json!({ "paths": [file], "track": midi, "scene": scene }),
    );
    // A clip file isn't added together with audio files.
    let sample = scratch.path("Kick.wav");
    wav(&sample, 4_800);
    refused(
        &mut shell,
        "browser.add_to_arrangement",
        json!({ "paths": [file, text(&sample)] }),
    );
    // A file that isn't a clip file is refused, whatever its name.
    let bad = scratch.path("Bad.blclip");
    std::fs::write(&bad, b"not a clip file").unwrap();
    refused(
        &mut shell,
        "browser.add_to_arrangement",
        json!({ "paths": [text(&bad)] }),
    );
}

// Spec: clip files R7 (S-SES-017). Several clip files go down the scenes of
// a track, or, with Cmd held, across the tracks at one scene.
#[test]
fn several_clip_files_go_down_the_scenes_or_spread_across_the_tracks() {
    let scratch = Scratch::new("spread");
    let mut shell = shell_in(&scratch);
    let midis = tracks(&shell, TrackKind::Midi);
    let audios = tracks(&shell, TrackKind::Audio);
    let a = midi_clip(&mut shell, midis[0], "A", 0, None);
    let b = midi_clip(&mut shell, midis[0], "B", BEAT * 8, None);
    let sample = scratch.path("Pad.wav");
    wav(&sample, 48_000);
    let c = audio_clip(&mut shell, audios[0], &sample);
    let a_file = save(&mut shell, a, json!({})).path;
    let b_file = save(&mut shell, b, json!({})).path;
    let c_file = save(&mut shell, c, json!({})).path;
    let scenes: Vec<SceneId> = shell
        .editor()
        .document()
        .scenes()
        .map(|scene| scene.id)
        .collect();
    let slot = |shell: &Shell, track: TrackId, scene: SceneId| {
        shell.editor().document().slot(track, scene).is_some()
    };

    // Down the scenes of one track without Cmd, as one undo step.
    let before = steps(&shell);
    run::<Value>(
        &mut shell,
        "session.drop_files",
        json!({ "paths": [a_file, b_file], "track": midis[1], "scene": scenes[1] }),
    );
    assert_eq!(steps(&shell), before + 1, "one undo step");
    assert!(slot(&shell, midis[1], scenes[1]) && slot(&shell, midis[1], scenes[2]));

    // With Cmd, each on a track of its own across the tracks at one scene.
    run::<Value>(
        &mut shell,
        "session.drop_files",
        json!({ "paths": [a_file, b_file], "track": midis[0], "scene": scenes[4], "spread": true }),
    );
    assert!(slot(&shell, midis[0], scenes[4]) && slot(&shell, midis[1], scenes[4]));
    // The next track must take the file: a MIDI clip file doesn't go on an
    // audio track, and nothing changes.
    refused(
        &mut shell,
        "session.drop_files",
        json!({ "paths": [a_file, c_file], "track": midis[0], "scene": scenes[5], "spread": true }),
    );
    run::<Value>(
        &mut shell,
        "session.drop_files",
        json!({ "paths": [a_file, c_file], "track": midis[1], "scene": scenes[5], "spread": true }),
    );
    assert!(slot(&shell, midis[1], scenes[5]) && slot(&shell, audios[0], scenes[5]));

    // In the Arrangement each goes after the other on one track, or on a
    // track of its own with `spread`.
    let placed: AddResult = run(
        &mut shell,
        "browser.add_to_arrangement",
        json!({ "paths": [a_file, b_file], "at": BEAT * 32 }),
    );
    assert_eq!(placed.created_tracks.len(), 1);
    assert_eq!(placed.clips[1].start, Ticks(BEAT * 36));
    let placed: AddResult = run(
        &mut shell,
        "browser.add_to_arrangement",
        json!({ "paths": [a_file, b_file], "at": BEAT * 64, "spread": true }),
    );
    assert_eq!(placed.created_tracks.len(), 2);
    assert_eq!(placed.clips[0].start, placed.clips[1].start);
}

// Spec: clip files R7, R8. The Session's drop area makes a track of the
// clip file's kind, named after the file, with its chain.
#[test]
fn a_clip_file_dropped_in_the_session_drop_area_makes_a_track() {
    let scratch = Scratch::new("drop-area");
    let mut shell = shell_in(&scratch);
    let midi = tracks(&shell, TrackKind::Midi)[0];
    let gain_id = add_gain(&mut shell, midi);
    let clip = midi_clip(&mut shell, midi, "Idea", 0, Some(gain_id));
    let file = save(&mut shell, clip, json!({})).path;
    let scene = shell.editor().document().scenes().nth(2).unwrap().id;
    let tracks_before = shell.editor().document().top_level().len();

    run::<Value>(
        &mut shell,
        "session.drop_files",
        json!({ "paths": [file], "scene": scene }),
    );
    let doc = shell.editor().document();
    assert_eq!(doc.top_level().len(), tracks_before + 1);
    let made = doc.top_level().last().unwrap();
    let track = doc.track(made).unwrap();
    assert_eq!((track.kind(), track.name()), (TrackKind::Midi, "Idea"));
    assert_eq!(track.devices().len(), 1, "its chain came with it");
    let slot = doc
        .slot(made, scene)
        .expect("in the scene it was dropped at");
    assert_eq!(slot.start, Ticks::ZERO);
    assert_eq!(slot.envelopes.len(), 2);
}

// Spec: clip files R9. An unwarped audio clip lasts as long as its audio
// plays at the set's tempo where it lands; a warped one keeps its beats.
#[test]
fn an_unwarped_audio_clip_keeps_its_time_and_a_warped_one_its_beats() {
    let scratch = Scratch::new("tempo");
    let mut shell = shell_in(&scratch);
    let audio = tracks(&shell, TrackKind::Audio);
    let sample = scratch.path("Hit.wav");
    wav(&sample, 96_000);
    let clip = audio_clip(&mut shell, audio[0], &sample);
    shell
        .run("clip.set_warp", json!({ "clip": clip, "warp": false }))
        .unwrap();
    let before = shell.editor().document().clip(clip).unwrap().length;
    let file = save(&mut shell, clip, json!({})).path;
    let tempo = serde_json::to_value(TempoMap::constant(Bpm::new(60.0).unwrap())).unwrap();
    shell
        .run("song.set_tempo", json!({ "tempo": tempo }))
        .unwrap();
    let placed: AddResult = run(
        &mut shell,
        "browser.add_to_arrangement",
        json!({ "paths": [file], "track": audio[1] }),
    );
    let length = shell
        .editor()
        .document()
        .clip(placed.clips[0].clip)
        .unwrap()
        .length;
    assert!(
        length < before,
        "two seconds last fewer beats at 60 than at 120 BPM: {length:?} against {before:?}"
    );
}

fn imported(root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(root.join("Samples").join("Imported"))
        .map(|dir| dir.filter_map(Result::ok).map(|e| e.path()).collect())
        .unwrap_or_default()
}

// Spec: file management R7 to R9, clip files R5.
#[test]
fn collect_files_on_export_copies_refers_or_asks() {
    let scratch = Scratch::new("collect");
    let mut shell = shell_in(&scratch);
    let audio = tracks(&shell, TrackKind::Audio)[0];
    let sample = scratch.path("Elsewhere/Pad.wav");
    wav(&sample, 24_000);
    let clip = audio_clip(&mut shell, audio, &sample);
    let library = library(&scratch);

    // Always, the default in a new install, copies the sample into the User
    // Library's Samples/Imported, and a second save uses the same copy.
    let first = save(&mut shell, clip, json!({}));
    assert_eq!(first.copied, 1);
    assert_eq!(imported(&library).len(), 1);
    assert_eq!(
        save(&mut shell, clip, json!({})).copied,
        0,
        "same content is reused"
    );
    assert_eq!(imported(&library).len(), 1);
    let file = ClipFile::load(Path::new(&first.path)).unwrap();
    let reference = file.media.iter().next().unwrap();
    assert_eq!(
        reference
            .path
            .resolve(Path::new(&first.path).parent().unwrap()),
        Some(library.join("Clips").join("../Samples/Imported/Pad.wav")),
    );

    // Never copies nothing and the file names the sample where it is.
    shell
        .run(
            "settings.set_collect_files_on_export",
            json!({ "mode": "never" }),
        )
        .unwrap();
    let never = save(
        &mut shell,
        clip,
        json!({ "folder": text(&scratch.path("Plain")) }),
    );
    assert_eq!(never.copied, 0);
    assert!(imported(&scratch.path("Plain")).is_empty());
    let file = ClipFile::load(Path::new(&never.path)).unwrap();
    assert_eq!(
        file.media
            .iter()
            .next()
            .unwrap()
            .path
            .resolve(&scratch.path("Plain")),
        Some(scratch.path("Plain/../Elsewhere/Pad.wav")),
    );

    // Ask refuses a command given no `copy`, and answers with it.
    shell
        .run(
            "settings.set_collect_files_on_export",
            json!({ "mode": "ask" }),
        )
        .unwrap();
    assert!(shell.clip_save_asks(clip, Some(&scratch.path("Asked"))));
    refused(
        &mut shell,
        "clip.save_file",
        json!({ "clip": clip, "folder": text(&scratch.path("Asked")) }),
    );
    assert!(!scratch.path("Asked").exists(), "nothing was written");
    let dont = save(
        &mut shell,
        clip,
        json!({ "folder": text(&scratch.path("Asked")), "copy": false }),
    );
    assert_eq!(dont.copied, 0);
    let copy = save(
        &mut shell,
        clip,
        json!({ "folder": text(&scratch.path("Asked 2")), "copy": true }),
    );
    assert_eq!(copy.copied, 1);
    // A MIDI clip plays no file, so there is nothing to ask.
    let midi_track = tracks(&shell, TrackKind::Midi)[0];
    let midi = midi_clip(&mut shell, midi_track, "Idea", 0, None);
    assert!(!shell.clip_save_asks(midi, None));
    save(&mut shell, midi, json!({}));

    // The setting reads back.
    let settings: Value = shell.run("settings.state", json!({})).unwrap();
    assert_eq!(settings["library"]["collect_files_on_export"], "ask");
}

// Spec: file management R9. Where the copies go.
#[test]
fn copies_go_into_the_project_the_file_lands_in() {
    let scratch = Scratch::new("homes");
    let mut shell = shell_in(&scratch);
    let audio = tracks(&shell, TrackKind::Audio)[0];
    let sample = scratch.path("Elsewhere/Pad.wav");
    wav(&sample, 24_000);
    let clip = audio_clip(&mut shell, audio, &sample);

    // A folder inside a project folder holding a set: that project.
    let set: bl_shell::commands::SetResult = run(
        &mut shell,
        "set.save_as",
        json!({ "path": text(&scratch.path("Song Project/Song.blset")) }),
    );
    let project = PathBuf::from(set.path.unwrap())
        .parent()
        .unwrap()
        .to_owned();
    let saved = save(
        &mut shell,
        clip,
        json!({ "folder": text(&project.join("Ideas")) }),
    );
    assert_eq!(saved.copied, 1, "the sample is outside the project");
    assert_eq!(imported(&project).len(), 1);
    // A second clip file in the project finds the copy and makes no other.
    let again = save(&mut shell, clip, json!({ "folder": text(&project) }));
    assert_eq!(again.copied, 0);
    assert_eq!(imported(&project).len(), 1);
    // A project folder of another set copies into its own Samples/Imported.
    let other = scratch.path("Other Project");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("Other.blset"), b"set").unwrap();
    assert_eq!(
        save(&mut shell, clip, json!({ "folder": text(&other) })).copied,
        1
    );
    assert_eq!(imported(&other).len(), 1);
    // A folder in neither gets a Samples/Imported folder beside it.
    let plain = scratch.path("Plain");
    assert_eq!(
        save(&mut shell, clip, json!({ "folder": text(&plain) })).copied,
        1
    );
    assert_eq!(imported(&plain).len(), 1);
}

// Spec: clip files R12, file management R6.
#[test]
fn a_sample_that_moved_is_found_by_content_and_a_lost_one_is_listed_missing() {
    let scratch = Scratch::new("relink");
    let mut shell = shell_in(&scratch);
    shell
        .run(
            "settings.set_collect_files_on_export",
            json!({ "mode": "never" }),
        )
        .unwrap();
    let audio = tracks(&shell, TrackKind::Audio);
    let sample = scratch.path("Elsewhere/Pad.wav");
    wav(&sample, 24_000);
    let clip = audio_clip(&mut shell, audio[0], &sample);
    let file = save(
        &mut shell,
        clip,
        json!({ "folder": text(&scratch.path("Clips")) }),
    )
    .path;

    // Open the clip file in a new set after the sample moved to a folder in
    // the places.
    let moved = library(&scratch)
        .join("Samples")
        .join("Moved")
        .join("Renamed.wav");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&sample, &moved).unwrap();
    let mut fresh = shell_in(&scratch);
    let target = tracks(&fresh, TrackKind::Audio)[0];
    fresh.wait_for_browser_index(Duration::from_secs(30));
    run::<AddResult>(
        &mut fresh,
        "browser.add_to_arrangement",
        json!({ "paths": [file], "track": target }),
    );
    let doc = fresh.editor().document();
    let placed = doc.track(target).unwrap().clips().next().unwrap();
    let ClipData::Audio(data) = &placed.data else {
        panic!("audio")
    };
    let hash = bl_media::ContentHash::from_bytes(data.media.0);
    assert_eq!(
        fresh.media_store().entry(hash).unwrap().path,
        moved,
        "found by content"
    );
    assert!(
        fresh
            .message()
            .map(|m| m.text.as_str())
            .is_none_or(|m| !m.contains("missing"))
    );

    // A sample that is nowhere: the clip is placed all the same, and the
    // Status Bar says how many files are missing.
    std::fs::remove_file(&moved).unwrap();
    let mut third = shell_in(&scratch);
    let target = tracks(&third, TrackKind::Audio)[1];
    run::<AddResult>(
        &mut third,
        "browser.add_to_arrangement",
        json!({ "paths": [file], "track": target }),
    );
    assert_eq!(clip_count(&third, target), 1);
    let message = third.message().map(|m| m.text.as_str()).unwrap_or_default();
    assert!(message.contains("1 audio file"), "{message:?}");
}

// Spec: clip files R11. A placement survives save and reopen.
#[test]
fn a_placed_clip_survives_save_and_reopen() {
    let scratch = Scratch::new("reopen");
    let mut shell = shell_in(&scratch);
    let midis = tracks(&shell, TrackKind::Midi);
    let gain_id = add_gain(&mut shell, midis[0]);
    let clip = midi_clip(&mut shell, midis[0], "Idea", 0, Some(gain_id));
    let file = save(&mut shell, clip, json!({})).path;
    let placed: AddResult = run(
        &mut shell,
        "browser.add_to_arrangement",
        json!({ "paths": [file] }),
    );
    let made = placed.created_tracks[0];
    let set: bl_shell::commands::SetResult = run(
        &mut shell,
        "set.save_as",
        json!({ "path": text(&scratch.path("Song Project/Song.blset")) }),
    );
    let path = PathBuf::from(set.path.unwrap());

    let mut reopened = shell_in(&scratch);
    reopened
        .run("set.open", json!({ "path": text(&path) }))
        .unwrap();
    let doc = reopened.editor().document();
    let track = doc.track(made).expect("the new track");
    assert_eq!(
        (track.kind(), track.devices().len(), track.clips().len()),
        (TrackKind::Midi, 1, 1)
    );
    assert_eq!(
        *doc.clip(placed.clips[0].clip).unwrap(),
        *shell
            .editor()
            .document()
            .clip(placed.clips[0].clip)
            .unwrap()
    );
}

// Spec: clip files R6, browser R9.
#[test]
fn the_clips_label_lists_every_clip_file_and_search_finds_clip_files_only() {
    let scratch = Scratch::new("label");
    let mut shell = shell_in(&scratch);
    let midi = tracks(&shell, TrackKind::Midi)[0];
    wav(&library(&scratch).join("Samples").join("Idea.wav"), 4_800);
    let idea = midi_clip(&mut shell, midi, "Idea", 0, None);
    save(&mut shell, idea, json!({}));
    save(
        &mut shell,
        idea,
        json!({ "folder": text(&library(&scratch).join("Presets")) }),
    );
    shell.wait_for_browser_index(Duration::from_secs(30));

    let browser: BrowserState = run(
        &mut shell,
        "browser.open_place",
        json!({ "label": "clips" }),
    );
    assert!(
        browser.note.is_none(),
        "{:?} {:?}",
        browser.note,
        browser.index
    );
    let names: Vec<&str> = browser.rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(
        names,
        ["Idea.blclip", "Idea.blclip"],
        "every clip file in the places, and no sample"
    );
    assert!(browser.rows.iter().all(|row| row.kind == ItemKind::Clip));
    assert!(browser.rows.iter().all(|row| row.folder.is_some()));
    assert!(
        browser
            .rows
            .iter()
            .all(|row| row.clip_kind == Some(ClipFileKind::Midi))
    );

    // Search under Clips finds clip files only.
    let found: BrowserState = run(&mut shell, "browser.search", json!({ "query": "idea" }));
    assert_eq!(found.rows.len(), 2);
    let all: BrowserState = run(
        &mut shell,
        "browser.search",
        json!({ "query": "idea", "label": "all" }),
    );
    assert_eq!(all.rows.len(), 3, "under All the sample is found too");
}

// Spec: clip files R6. Content tags (browser tags R7).
#[test]
fn clip_files_carry_the_content_tags_audio_clip_and_midi_clip() {
    let scratch = Scratch::new("tags");
    let mut shell = shell_in(&scratch);
    let midi = tracks(&shell, TrackKind::Midi)[0];
    let idea = midi_clip(&mut shell, midi, "Idea", 0, None);
    let midi_file = save(&mut shell, idea, json!({})).path;
    let sample = scratch.path("Pad.wav");
    wav(&sample, 24_000);
    let audio_track = tracks(&shell, TrackKind::Audio)[0];
    let pad = audio_clip(&mut shell, audio_track, &sample);
    let audio_file = save(&mut shell, pad, json!({})).path;
    let tags: bl_shell::browser_commands::TagsState = run(
        &mut shell,
        "browser.tags",
        json!({ "items": [midi_file, audio_file] }),
    );
    let carried = |item: &str| -> Vec<String> {
        tags.items
            .iter()
            .find(|i| i.item == item)
            .unwrap()
            .tags
            .iter()
            .map(|t| t.tag.clone())
            .collect()
    };
    assert_eq!(carried(&midi_file), ["Content|MIDI Clip"]);
    assert_eq!(carried(&audio_file), ["Content|Audio Clip"]);
}

// Spec: browser R24.
#[test]
fn the_user_library_is_chosen_in_settings_and_made_at_once() {
    let scratch = Scratch::new("library");
    let mut shell = shell_in(&scratch);
    let chosen = scratch.path("My Library");
    std::fs::create_dir_all(chosen.join("Samples")).unwrap();
    std::fs::write(chosen.join("Samples").join("keep.txt"), b"mine").unwrap();
    let result: bl_shell::commands::UserLibraryResult = run(
        &mut shell,
        "settings.set_user_library",
        json!({ "path": text(&chosen) }),
    );
    assert_eq!(result.path, text(&chosen));
    for folder in bl_shell::browser::places::USER_LIBRARY_FOLDERS {
        assert!(chosen.join(folder).is_dir(), "{folder} is made");
    }
    assert_eq!(
        std::fs::read(chosen.join("Samples").join("keep.txt")).unwrap(),
        b"mine"
    );
    let settings: Value = shell.run("settings.state", json!({})).unwrap();
    assert_eq!(settings["library"]["user_library"], text(&chosen));
    let places = state(&mut shell);
    let library = places
        .labels
        .iter()
        .find(|l| l.name == "User Library")
        .unwrap();
    assert_eq!(library.path.as_deref(), Some(text(&chosen).as_str()));
    // The choice is kept between launches.
    let kept = Preferences::load(&scratch.path("config")).unwrap();
    assert_eq!(kept.browser.user_library, Some(chosen));
    // A path that isn't absolute is refused.
    assert!(
        shell
            .run("settings.set_user_library", json!({ "path": "relative" }))
            .is_err()
    );
}

// Spec: clip files R13. A 100 BPM audio clip file previews at the set's 120
// BPM through its own warp markers.
#[test]
fn an_audio_clip_file_previews_through_its_own_warp_settings() {
    let scratch = Scratch::new("preview");
    let (mut shell, control) = testing::rig();
    let library = library(&scratch);
    bl_shell::browser::places::create_user_library(&library).unwrap();
    shell.set_user_library(Some(library));
    shell.start_audio();
    let (loop_file, _) = bl_analysis::signals::click_track(100.0, 0.0, 4.8, 48_000);
    let path = scratch.path("Loop100.wav");
    let bytes = bl_media::wav_bytes(
        &loop_file,
        1,
        48_000,
        BitDepth::Int16,
        bl_media::Dither::None,
    )
    .unwrap();
    std::fs::write(&path, bytes).unwrap();
    let audio = tracks(&shell, TrackKind::Audio)[0];
    let clip = audio_clip(&mut shell, audio, &path);
    // The clip warps at its own 100 BPM: two bars of the set's 120 BPM last
    // four seconds, and an unwarped play would last 4.8.
    let ClipData::Audio(data) = &shell.editor().document().clip(clip).unwrap().data else {
        panic!("audio")
    };
    assert!(data.warp, "a loop of this length is warped as it is added");
    let file = save(&mut shell, clip, json!({})).path;

    shell
        .run("browser.select", json!({ "paths": [file.clone()] }))
        .unwrap();
    let wait = |shell: &mut Shell, what: &str, done: &mut dyn FnMut(&mut Shell) -> bool| {
        testing::wait_for(&control, shell, what, |shell| {
            shell.poll();
            done(shell)
        });
    };
    wait(&mut shell, "the clip file to play", &mut |shell| {
        shell.browser_preview().playing == Some(file.clone())
    });
    wait(&mut shell, "the preview to pass 3 seconds", &mut |shell| {
        shell.browser_preview().position_seconds > 3.0
    });
    wait(&mut shell, "the preview to loop", &mut |shell| {
        let preview = shell.browser_preview();
        preview.playing.is_some() && preview.position_seconds < 1.0
    });
}

/// A rig with a User Library and the audio running, and a MIDI track holding
/// the native polysynth, then the test plug-in's gain at `gain` if given.
fn rig_with_synth(
    scratch: &Scratch,
    gain: Option<f64>,
) -> (Shell, bl_audio_io::NullControl, TrackId) {
    let (mut shell, control) = testing::rig();
    bl_shell::browser::places::create_user_library(&library(scratch)).unwrap();
    shell.set_user_library(Some(library(scratch)));
    shell.start_audio();
    let track = tracks(&shell, TrackKind::Midi)[0];
    shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let synth = bl_devices::polysynth_type();
    shell
        .run(
            "device.add",
            json!({ "device": { "kind": synth.kind, "name": synth.name, "role": synth.role }, "track": track }),
        )
        .unwrap();
    if let Some(gain) = gain {
        let loaded: bl_shell::plugin_commands::PluginLoaded = run(
            &mut shell,
            "plugin.load",
            json!({ "path": bl_test_clap::bundle(), "track": track }),
        );
        shell
            .run(
                "device.set_param",
                json!({ "device": loaded.device, "param": bl_test_clap::GAIN, "value": gain }),
            )
            .unwrap();
    }
    (shell, control, track)
}

/// Selects `file`, then lets the background render finish.
fn select_and_render(shell: &mut Shell, file: &str) {
    shell
        .run("browser.select", json!({ "paths": [file] }))
        .unwrap();
    render_done(shell);
}

/// Polls until the background render is in: it takes real time, which no
/// step of test time stands for.
fn render_done(shell: &mut Shell) {
    for _ in 0..5_000_000 {
        shell.poll();
        if shell.browser_preview().loading.is_none() {
            return;
        }
        std::thread::yield_now();
    }
    panic!("the render never finished");
}

fn render_of(
    file: &Path,
    bpm: f64,
) -> Result<(bl_media::SampleData, Box<bl_model::AudioData>), String> {
    bl_shell::browser::preview::render_midi(
        file,
        ClipFile::load(file).unwrap(),
        48_000,
        bpm,
        &[],
        &AtomicBool::new(false),
    )
}

// Spec: clip files R16. A MIDI clip file with a native instrument previews as
// its offline render of one loop at the set's tempo, looping.
#[test]
fn a_midi_clip_file_with_an_instrument_previews_as_its_render_of_one_loop() {
    let scratch = Scratch::new("midi-preview");
    let (mut shell, control, track) = rig_with_synth(&scratch, None);
    let clip = midi_clip(&mut shell, track, "Idea", 0, None);
    let file = save(&mut shell, clip, json!({})).path;

    // The render is what exporting the clip's set over its length gives.
    let (data, _) = render_of(Path::new(&file), 120.0).unwrap();
    let mut exported = Vec::new();
    bl_shell::export::render(
        shell.editor().document(),
        shell.media_store(),
        bl_shell::export::Hosts::default(),
        &bl_shell::export::Render {
            start: Ticks::ZERO,
            length: bl_shell::export::Length::To(Ticks(BEAT * 4)),
            sample_rate: 48_000,
            engine_rate: 48_000,
            block: bl_shell::export::BLOCK,
        },
        60.0,
        &AtomicBool::new(false),
        &mut |_| {},
        &mut |block| {
            exported.extend_from_slice(block);
            Ok(())
        },
        None,
    )
    .unwrap();
    let left: Vec<f32> = exported.iter().step_by(2).copied().collect();
    assert_eq!(data.frames(), 96_000, "four beats at 120 BPM");
    assert!(left.iter().any(|s| *s != 0.0), "the synth plays the notes");
    assert_eq!(data.channel(0).unwrap(), &left[..], "sample for sample");
    // At another tempo it's another length.
    assert_eq!(
        render_of(Path::new(&file), 90.0).unwrap().0.frames(),
        128_000
    );

    // Selecting it plays that render, looping: four beats last two seconds.
    shell
        .run("browser.select", json!({ "paths": [file.clone()] }))
        .unwrap();
    let wait = |shell: &mut Shell, what: &str, done: &mut dyn FnMut(&mut Shell) -> bool| {
        testing::wait_for(&control, shell, what, |shell| {
            shell.poll();
            std::thread::yield_now();
            done(shell)
        });
    };
    wait(&mut shell, "the clip file to play", &mut |shell| {
        shell.browser_preview().position_seconds > 1.0
    });
    let preview = shell.browser_preview();
    assert_eq!(preview.playing, Some(file.clone()));
    assert!((preview.length_seconds - 2.0).abs() < 0.01, "{preview:?}");
    wait(&mut shell, "the preview to loop", &mut |shell| {
        let preview = shell.browser_preview();
        preview.playing.is_some() && preview.position_seconds < 1.0
    });
    // The audio thread played it, looping, without touching the heap.
    shell.run("browser.stop_preview", json!({})).unwrap();
    control.run_blocks(40);
    let stats = shell.audio().stats().expect("a stream is open");
    assert_eq!(stats.heap_operations, 0, "{stats:?}");
}

// Spec: clip files R16. While the transport runs the preview starts on the
// next bar; with Raw on it plays at once, once.
#[test]
fn a_midi_clip_file_preview_starts_on_the_next_bar_and_raw_at_once() {
    let scratch = Scratch::new("midi-bar");
    let (mut shell, control, track) = rig_with_synth(&scratch, None);
    let clip = midi_clip(&mut shell, track, "Idea", 0, None);
    let file = save(&mut shell, clip, json!({})).path;
    shell.run("transport.play", json!({})).unwrap();
    testing::advance(&control, &mut shell, 500);
    select_and_render(&mut shell, &file);
    assert!(
        shell.arrangement().transport().position < Ticks(BEAT * 2),
        "selected in the song's first bar"
    );
    let mut waited = 0;
    while shell.browser_preview().position_seconds == 0.0 {
        testing::advance(&control, &mut shell, 5);
        shell.poll();
        waited += 5;
        assert!(waited < 10_000, "the preview never started");
    }
    // It began where the second bar does, not before.
    let at = shell.arrangement().transport().position;
    assert!(
        (Ticks(BEAT * 4)..Ticks(BEAT * 4 + BEAT / 2)).contains(&at),
        "started with the song at {at:?}"
    );

    shell
        .run("browser.set_raw", json!({ "raw": true }))
        .unwrap();
    shell
        .run("browser.preview", json!({ "path": file }))
        .unwrap();
    render_done(&mut shell);
    testing::advance(&control, &mut shell, 100);
    shell.poll();
    assert!(
        shell.browser_preview().position_seconds > 0.0,
        "Raw plays at once"
    );
}

// Spec: clip files R16, offline plug-in renders R3, R11. A plug-in in the
// clip file's chain is made fresh from its saved state: the set's instance
// of a plug-in like it isn't touched.
#[test]
fn a_clip_file_with_a_clap_plugin_renders_through_a_fresh_instance() {
    let scratch = Scratch::new("midi-clap");
    let (mut shell, control, track) = rig_with_synth(&scratch, Some(0.5));
    let clip = midi_clip(&mut shell, track, "Idea", 0, None);
    let file = save(&mut shell, clip, json!({})).path;
    let plugin = shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .devices()
        .last()
        .map(|d| d.id)
        .unwrap();
    testing::wait_for(&control, &mut shell, "the plug-in to run", |shell| {
        shell.poll();
        shell.plugins().status(plugin).is_some_and(|s| s.active)
    });
    let before = (
        shell.plugins().status(plugin),
        shell.plugins().save_state(plugin).unwrap(),
    );

    // The same chain without the plug-in, to hear what it does: a gain of 0.5.
    let (through, _) = render_of(Path::new(&file), 120.0).unwrap();
    let plain = scratch.path("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let mut without = ClipFile::load(Path::new(&file)).unwrap();
    without.devices.pop();
    without.save(&plain.join("Plain.blclip")).unwrap();
    let (dry, _) = render_of(&plain.join("Plain.blclip"), 120.0).unwrap();
    assert!(dry.channel(0).unwrap().iter().any(|s| *s != 0.0));
    for (out, input) in through
        .channel(0)
        .unwrap()
        .iter()
        .zip(dry.channel(0).unwrap())
    {
        assert!((out - input * 0.5).abs() <= 1e-6, "{out} against {input}");
    }
    assert_eq!(
        (
            shell.plugins().status(plugin),
            shell.plugins().save_state(plugin).unwrap()
        ),
        before,
        "the set's instance is as it was"
    );
    assert!(
        shell.plugins().lend().is_ok(),
        "nothing of the set's is lent"
    );
}

// Spec: clip files R16. Selecting another file during a render drops it, and
// a chain without an instrument previews nothing and the strip says why.
#[test]
fn a_chain_without_an_instrument_previews_nothing_and_another_file_drops_a_render() {
    let scratch = Scratch::new("midi-none");
    let (mut shell, _control, track) = rig_with_synth(&scratch, None);
    let clip = midi_clip(&mut shell, track, "Idea", 0, None);
    let with_synth = save(&mut shell, clip, json!({})).path;
    let mut bare = ClipFile::load(Path::new(&with_synth)).unwrap();
    bare.devices.clear();
    let bare_path = library(&scratch).join("Clips").join("Bare.blclip");
    bare.save(&bare_path).unwrap();

    // Another file straight after the first: the render under way is
    // dropped, and the bare chain says why it plays nothing.
    shell
        .run("browser.select", json!({ "paths": [with_synth.clone()] }))
        .unwrap();
    select_and_render(&mut shell, &text(&bare_path));
    let preview = shell.browser_preview();
    assert_eq!(preview.playing, None);
    assert!(
        preview
            .note
            .as_deref()
            .is_some_and(|n| n.contains("no instrument")),
        "{preview:?}"
    );
    assert!(shell.message().is_some_and(|m| m.error));

    // A file that does play clears the note.
    select_and_render(&mut shell, &with_synth);
    let preview = shell.browser_preview();
    assert_eq!(preview.note, None);
    assert_eq!(preview.playing, Some(with_synth.clone()));
    assert!(render_of_cancelled(Path::new(&with_synth)));
}

/// A render told to stop at once gives no audio.
fn render_of_cancelled(file: &Path) -> bool {
    bl_shell::browser::preview::render_midi(
        file,
        ClipFile::load(file).unwrap(),
        48_000,
        120.0,
        &[],
        &AtomicBool::new(true),
    )
    .is_err()
}

// Spec: clip files R10. Enter places the selected clip file.
#[test]
fn enter_places_the_selected_clip_file_in_either_view() {
    let scratch = Scratch::new("enter");
    let mut shell = shell_in(&scratch);
    let midis = tracks(&shell, TrackKind::Midi);
    let clip = midi_clip(&mut shell, midis[0], "Idea", 0, None);
    let file = save(&mut shell, clip, json!({})).path;
    shell
        .run("browser.select", json!({ "paths": [file.clone()] }))
        .unwrap();

    // The Arrangement: the selected track at the insert marker when it is a
    // track of the clip's kind, otherwise a new track below the last one.
    shell
        .run("arrangement.select_tracks", json!({ "tracks": [midis[1]] }))
        .unwrap();
    shell.run("browser.load_selected", json!({})).unwrap();
    assert_eq!(clip_count(&shell, midis[1]), 1);
    let audio = tracks(&shell, TrackKind::Audio)[0];
    shell
        .run("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    let count = shell.editor().document().top_level().len();
    shell.run("browser.load_selected", json!({})).unwrap();
    assert_eq!(shell.editor().document().top_level().len(), count + 1);

    // The Session View: the selected slot when it is empty and on a track of
    // the clip's kind, otherwise a new track in the selected scene.
    shell.run("session.focus", json!({})).unwrap();
    let scene = shell.editor().document().scenes().nth(3).unwrap().id;
    shell
        .run(
            "session.select",
            json!({ "track": midis[1], "scene": scene }),
        )
        .unwrap();
    shell.run("browser.load_selected", json!({})).unwrap();
    assert!(shell.editor().document().slot(midis[1], scene).is_some());
    let count = shell.editor().document().top_level().len();
    shell.run("browser.load_selected", json!({})).unwrap();
    assert_eq!(
        shell.editor().document().top_level().len(),
        count + 1,
        "the slot is full now: a new track"
    );
}

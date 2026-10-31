//! Whole tracks copied, cut, pasted and duplicated (WP-6.52): a copy
//! holds everything its track holds, under new IDs, and plays as its
//! source does. The behaviour is in the track context menu spec, R12 to
//! R16.

use std::path::{Path, PathBuf};

use bl_model::{ClipId, TrackId, TrackKind};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use serde_json::json;

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-whole-tracks-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a 32-bit float WAV of `seconds` at 48 kHz holding a ramp.
fn wav(path: &Path, seconds: f64) {
    let frames = (seconds * 48_000.0) as usize;
    let mut writer = bl_media::AudioWriter::create(
        path,
        bl_media::EncodeSpec {
            file_type: bl_media::FileType::Wav,
            bit_depth: bl_media::BitDepth::Float32,
            dither: bl_media::Dither::None,
            sample_rate: 48_000,
            channels: 2,
        },
    )
    .unwrap();
    for frame in 0..frames {
        let value = 0.25 + frame as f32 / 1_000_000.0;
        writer.write(&[value, value]).unwrap();
    }
    writer.finish().unwrap();
}

fn new_shell(scratch: &Scratch) -> Shell {
    Shell::new(
        Preferences::default(),
        Some(scratch.path("config")),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    )
}

fn audio_track(shell: &Shell) -> TrackId {
    shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Audio)
        .map(|track| track.id())
        .expect("the default set has an audio track")
}

fn place(shell: &mut Shell, track: TrackId, path: &Path, at: i64) -> ClipId {
    let added = shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [path.display().to_string()], "track": track, "at": at }),
        )
        .unwrap();
    serde_json::from_value(added["clips"][0]["clip"].clone()).unwrap()
}

/// Renders the set to a WAV file and gives the frames.
fn render(shell: &mut Shell, path: &Path) -> Vec<u8> {
    shell
        .run(
            "export.audio",
            json!({ "path": path.display().to_string(), "bit_depth": 32,
                    "sample_rate": 48_000 }),
        )
        .unwrap();
    std::fs::read(path).unwrap()
}

// Spec: track-context-menu R13 (a copy plays as its source does)
#[test]
fn a_copied_track_plays_as_its_source_does() {
    let scratch = Scratch::new("render");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0);
    place(&mut shell, track, &file, 0);
    let source = render(&mut shell, &scratch.path("source.wav"));

    // The copy, with the source muted, plays what the source played.
    shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [track], "title_bars": true }),
        )
        .unwrap();
    shell.run("track.duplicate", json!({})).unwrap();
    let copy = shell.arrangement().selected_tracks()[0];
    shell
        .run("mixer.set_mute", json!({ "track": track, "mute": true }))
        .unwrap();
    let copy_render = render(&mut shell, &scratch.path("copy.wav"));
    assert_eq!(source, copy_render, "the copy plays what its source played");
    assert_ne!(copy, track, "the copy has its own ID");
}

// Spec: track-context-menu R13 (a frozen track's copy keeps its frozen
// state and its files)
#[test]
fn a_frozen_tracks_copy_keeps_its_frozen_audio() {
    let scratch = Scratch::new("frozen");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0);
    place(&mut shell, track, &file, 0);
    shell
        .run(
            "arrangement.select_time",
            json!({ "start": 0, "end": 705_600 }),
        )
        .unwrap();
    shell
        .run("track.freeze", json!({ "tracks": [track] }))
        .unwrap();
    let frozen = shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .frozen()
        .cloned()
        .expect("the track is frozen");

    shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [track], "title_bars": true }),
        )
        .unwrap();
    shell.run("track.duplicate", json!({})).unwrap();
    let copy = shell.arrangement().selected_tracks()[0];
    let doc = shell.editor().document();
    let copied = doc
        .track(copy)
        .unwrap()
        .frozen()
        .cloned()
        .expect("the copy is frozen");
    assert_eq!(copied.clips.len(), frozen.clips.len());
    assert_eq!(
        copied.clips[0].media, frozen.clips[0].media,
        "the copy plays the same file"
    );
    assert_ne!(
        copied.clips[0].clip, frozen.clips[0].clip,
        "the copy's clip has its own ID"
    );
    // The copy plays the frozen audio, not the devices behind it.
    shell
        .run("mixer.set_mute", json!({ "track": track, "mute": true }))
        .unwrap();
    let copy_render = render(&mut shell, &scratch.path("frozen-copy.wav"));
    assert!(!copy_render.is_empty(), "the copy rendered");
}

// Spec: track-context-menu R15; set-exchange R7 (the files of pasted
// tracks go into the other set's media table)
#[test]
fn tracks_pasted_into_another_set_bring_their_files() {
    let scratch = Scratch::new("media");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0);
    place(&mut shell, track, &file, 0);
    let hash = match &shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .clips()
        .next()
        .unwrap()
        .data
    {
        bl_model::ClipData::Audio(audio) => audio.media,
        bl_model::ClipData::Midi(_) => unreachable!("an audio clip"),
    };
    shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [track], "title_bars": true }),
        )
        .unwrap();
    shell.run("track.copy", json!({})).unwrap();

    shell.run("set.new", json!({})).unwrap();
    shell.run("track.paste", json!({})).unwrap();
    let saved = shell
        .run(
            "set.save_as",
            json!({ "path": scratch.path("Song.blset").display().to_string() }),
        )
        .unwrap();
    let song = PathBuf::from(saved["path"].as_str().unwrap());
    let saved = bl_project::load(&song).unwrap().0;
    assert!(
        saved.media.iter().any(|entry| entry.hash == hash),
        "the saved set lists the pasted clip's file"
    );

    // The reopened set finds the file.
    let mut reopened = new_shell(&scratch);
    reopened
        .run("set.open", json!({ "path": song.display().to_string() }))
        .unwrap();
    let status = reopened
        .media_store()
        .check(bl_media::ContentHash::from_bytes(hash.0))
        .unwrap();
    assert_eq!(status, bl_media::MediaStatus::Available);
}

/// The track that follows `track` in the set's top-level order.
fn next_track(shell: &Shell, track: TrackId) -> TrackId {
    let order: Vec<TrackId> = shell.editor().document().top_level().collect();
    let at = order.iter().position(|&t| t == track).unwrap();
    order[at + 1]
}

// Spec: track-context-menu R13 (a copy holds everything the track has)
#[test]
fn a_copy_keeps_audition_overrides_and_info_text() {
    let scratch = Scratch::new("holds");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let mut lanes = Vec::new();
    for name in ["Take 1", "Take 2"] {
        let lane: bl_model::TakeLaneId = shell.new_id();
        shell
            .run(
                "track.add_take_lane",
                json!({ "track": track, "lane": { "id": lane, "name": name, "clips": [] } }),
            )
            .unwrap();
        lanes.push(lane);
    }
    shell
        .run("take_lane.audition", json!({ "lanes": [lanes[1]] }))
        .unwrap();
    let volume = json!({ "type": "mixer", "param": "volume" });
    for at in [0, 705_600] {
        shell
            .run(
                "automation.set_breakpoint",
                json!({ "track": track, "target": volume, "at": at, "value": 0.5 }),
            )
            .unwrap();
    }
    shell
        .run(
            "mixer.set_volume",
            json!({ "track": track, "volume": 0.25 }),
        )
        .unwrap();
    shell
        .run(
            "track.set_info_text",
            json!({ "track": track, "info_text": "hello" }),
        )
        .unwrap();
    let overridden = |shell: &Shell, track: TrackId| {
        shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .automation()
            .map(|(_, automation)| automation.enabled)
            .collect::<Vec<_>>()
    };
    assert_eq!(overridden(&shell, track), [false], "a hand move overrides");

    shell
        .run("track.duplicate", json!({ "tracks": [track] }))
        .unwrap();
    let copy = next_track(&shell, track);
    let made = shell.editor().document().track(copy).unwrap();
    let second = made.take_lanes().nth(1).unwrap().id;
    assert_eq!(made.audition(), Some(second), "the copy auditions its lane");
    assert_eq!(made.info_text(), "hello");
    assert_eq!(overridden(&shell, copy), [false], "the override came too");
}

// Spec: track-context-menu R13 (under new IDs throughout)
#[test]
fn a_copy_of_a_frozen_group_has_its_own_output() {
    let scratch = Scratch::new("output");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0);
    place(&mut shell, track, &file, 0);
    let grouped = shell
        .run("track.group", json!({ "tracks": [track] }))
        .unwrap();
    let group: TrackId = serde_json::from_value(grouped["track"].clone()).unwrap();
    shell
        .run("track.freeze", json!({ "tracks": [group] }))
        .unwrap();
    let output_of = |shell: &Shell, track: TrackId| {
        shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .frozen()
            .and_then(|frozen| frozen.output.as_ref().map(|output| output.id))
    };
    let source = output_of(&shell, group).expect("the group is frozen");

    shell
        .run("track.duplicate", json!({ "tracks": [group] }))
        .unwrap();
    let copy = next_track(&shell, group);
    assert_ne!(
        output_of(&shell, copy),
        Some(source),
        "the output has its own ID"
    );
    assert!(output_of(&shell, copy).is_some(), "and the copy is frozen");
}

// Spec: track-context-menu R16 (Duplicate Without Clips makes the copy the
// same way)
#[test]
fn a_copy_without_clips_keeps_the_mixer_and_delay() {
    let scratch = Scratch::new("without");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    for (id, args) in [
        ("mixer.set_pan_mode", json!({ "mode": "split" })),
        ("mixer.set_split_pan_left", json!({ "pan": -0.5 })),
        ("mixer.set_crossfade_assign", json!({ "assign": "a" })),
        (
            "mixer.set_track_delay",
            json!({ "delay": { "unit": "milliseconds", "value": 12.0 } }),
        ),
    ] {
        let mut args = args;
        args["track"] = json!(track);
        shell.run(id, args).unwrap();
    }
    shell
        .run(
            "track.duplicate_without_clips",
            json!({ "tracks": [track] }),
        )
        .unwrap();
    let copy = next_track(&shell, track);
    let doc = shell.editor().document();
    let (source, made) = (doc.track(track).unwrap(), doc.track(copy).unwrap());
    assert_eq!(made.mixer(), source.mixer());
    assert_eq!(made.delay(), source.delay());
}

/// Makes a track of `kind` at `index` in the open set.
fn create_track(shell: &mut Shell, kind: &str, index: usize) -> TrackId {
    let track: TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            json!({ "id": track, "kind": kind, "name": "T", "parent": null, "index": index }),
        )
        .unwrap();
    track
}

// Spec: set-exchange R6, R7. Clips copied with Copy Time land on the track
// that is in the same place in another set, and not on the track that has
// the source's ID.
#[test]
fn time_pasted_into_another_set_lands_by_place() {
    let scratch = Scratch::new("time");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0);
    place(&mut shell, track, &file, 0);
    shell
        .run(
            "arrangement.copy_time",
            json!({ "start": 0, "end": 705_600 }),
        )
        .unwrap();

    // The new set's third place is another audio track, and the track with
    // the source's ID is pushed down to the fourth.
    shell.run("set.new", json!({})).unwrap();
    let other = create_track(&mut shell, "audio", 2);
    shell
        .run("arrangement.paste_time", json!({ "at": 0 }))
        .unwrap();
    let doc = shell.editor().document();
    assert_eq!(doc.track(other).unwrap().clips().count(), 1);
    assert_eq!(doc.track(track).unwrap().clips().count(), 0);

    // The clip's file is in the new set's media table.
    let saved = shell
        .run(
            "set.save_as",
            json!({ "path": scratch.path("Song.blset").display().to_string() }),
        )
        .unwrap();
    let saved = bl_project::load(PathBuf::from(saved["path"].as_str().unwrap()))
        .unwrap()
        .0;
    assert_eq!(saved.media.len(), 1, "the saved set lists the file");
}

// Spec: set-exchange R7. Clips pasted into the Session View of another set
// bring their files.
#[test]
fn a_session_paste_into_another_set_brings_its_files() {
    let scratch = Scratch::new("scene-media");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0);
    let clip = place(&mut shell, track, &file, 0);
    shell
        .run("arrangement.copy", json!({ "clips": [clip] }))
        .unwrap();

    shell.run("set.new", json!({})).unwrap();
    let track = audio_track(&shell);
    let scene = shell.editor().document().scenes().next().unwrap().id;
    shell
        .run("session.select", json!({ "track": track, "scene": scene }))
        .unwrap();
    shell.run("session.paste", json!({})).unwrap();
    assert!(shell.editor().document().slot(track, scene).is_some());
    let saved = shell
        .run(
            "set.save_as",
            json!({ "path": scratch.path("Song.blset").display().to_string() }),
        )
        .unwrap();
    let saved = bl_project::load(PathBuf::from(saved["path"].as_str().unwrap()))
        .unwrap()
        .0;
    assert_eq!(saved.media.len(), 1, "the saved set lists the file");
}

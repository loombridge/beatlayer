//! Freeze Track and unfreezing (WP-4.14). The behaviour is in the freeze
//! and bounce spec, R1 to R17.

use std::path::{Path, PathBuf};

use bl_model::{ClipId, DeviceId, Track, TrackId, TrackKind};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::freeze as ids;
use bl_shell::settings::Preferences;
use serde_json::json;

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-freeze-{name}-{}-{}",
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

/// Writes a 32-bit float WAV of `seconds` at 48 kHz holding a ramp, with
/// `channels` channels.
fn wav(path: &Path, seconds: f64, channels: u16) {
    let frames = (seconds * 48_000.0) as usize;
    let data: Vec<Vec<f32>> = (0..usize::from(channels))
        .map(|channel| {
            (0..frames)
                .map(|i| {
                    if channels == 2 {
                        0.25 + i as f32 / 1_000_000.0
                    } else {
                        0.25 + channel as f32 * 0.1 + i as f32 / 1_000_000.0
                    }
                })
                .collect()
        })
        .collect();
    let mut writer = bl_media::AudioWriter::create(
        path,
        bl_media::EncodeSpec {
            file_type: bl_media::FileType::Wav,
            bit_depth: bl_media::BitDepth::Float32,
            dither: bl_media::Dither::None,
            sample_rate: 48_000,
            channels,
        },
    )
    .unwrap();
    for frame in 0..frames {
        let samples: Vec<f32> = data.iter().map(|channel| channel[frame]).collect();
        writer.write(&samples).unwrap();
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

fn saved(scratch: &Scratch, shell: &mut Shell) -> PathBuf {
    let path = scratch.path("Song.blset");
    let result = shell
        .run("set.save_as", json!({ "path": path.display().to_string() }))
        .unwrap();
    PathBuf::from(result["path"].as_str().unwrap())
}

fn audio_track(shell: &Shell) -> TrackId {
    shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Audio)
        .map(Track::id)
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

fn freeze(shell: &mut Shell, track: TrackId) -> serde_json::Value {
    shell
        .run(ids::FREEZE, json!({ "tracks": [track] }))
        .unwrap()
}

#[test]
fn a_freeze_writes_a_processed_file_and_one_undo_step() {
    let scratch = Scratch::new("processed");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    place(&mut shell, track, &file, 0);
    saved(&scratch, &mut shell);

    let result = freeze(&mut shell, track);
    assert_eq!(result["frozen"], json!([track]));
    let written = result["files"][0].as_str().unwrap();
    let folder = Path::new("Samples").join("Processed").join("Freeze");
    assert!(
        Path::new(written)
            .parent()
            .is_some_and(|parent| parent.ends_with(&folder)),
        "freeze files go into Samples/Processed/Freeze: {written}"
    );
    assert!(Path::new(written).exists());
    let frozen = shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .frozen()
        .unwrap()
        .clone();
    assert_eq!(frozen.clips.len(), 1);

    shell.run("edit.undo", json!({})).unwrap();
    assert!(!shell.editor().document().track(track).unwrap().is_frozen());
    shell.run("edit.redo", json!({})).unwrap();
    assert!(shell.editor().document().track(track).unwrap().is_frozen());
}

#[test]
fn a_mono_track_freezes_to_a_mono_file() {
    let scratch = Scratch::new("mono");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("mono.wav");
    wav(&file, 0.5, 2);
    place(&mut shell, track, &file, 0);

    let result = freeze(&mut shell, track);
    let written = result["files"][0].as_str().unwrap();
    let decoded = bl_media::decode_file(Path::new(written)).unwrap();
    assert_eq!(
        decoded.channels().count(),
        1,
        "equal channels freeze to one channel"
    );
}

#[test]
fn main_and_a_track_without_clips_are_refused() {
    let scratch = Scratch::new("refused");
    let mut shell = new_shell(&scratch);
    let main = shell.editor().document().main_id();
    assert!(shell.run(ids::FREEZE, json!({ "tracks": [main] })).is_err());
    let track = audio_track(&shell);
    assert!(
        shell
            .run(ids::FREEZE, json!({ "tracks": [track] }))
            .is_err()
    );
}

#[test]
fn unfreezing_keeps_edits_made_while_frozen() {
    let scratch = Scratch::new("edits");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    let clip = place(&mut shell, track, &file, 0);
    freeze(&mut shell, track);
    shell
        .run(
            "clip.move",
            json!({ "clip": clip, "track": track, "start": 4 * 705_600 }),
        )
        .unwrap();

    shell
        .run(ids::UNFREEZE, json!({ "tracks": [track] }))
        .unwrap();
    let doc = shell.editor().document();
    assert!(!doc.track(track).unwrap().is_frozen());
    assert_eq!(doc.clip(clip).unwrap().start.get(), 4 * 705_600);
}

#[test]
fn the_clip_view_reports_a_frozen_track() {
    let scratch = Scratch::new("query");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    place(&mut shell, track, &file, 0);
    freeze(&mut shell, track);
    // The saved form carries the frozen state, so a reopened set is frozen
    // without rendering (R13).
    let saved = shell.editor().document().saved_track(track);
    let value = serde_json::to_value(saved).unwrap();
    assert!(value.get("frozen").is_some(), "{value}");
}

#[test]
fn a_frozen_track_nulls_against_its_live_output() {
    let scratch = Scratch::new("null");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    let clip = place(&mut shell, track, &file, 4 * 705_600);
    let doc = shell.editor().document();
    let start = doc.clip(clip).unwrap().start;
    let end = doc.clip(clip).unwrap().end();
    shell
        .run(
            "arrangement.select_time",
            json!({ "start": start.get(), "end": end.get() }),
        )
        .unwrap();
    let live = scratch.path("live.wav");
    shell
        .run(
            "export.audio",
            json!({ "path": live.display().to_string(), "bit_depth": 32,
                    "sample_rate": 48_000 }),
        )
        .unwrap();
    freeze(&mut shell, track);
    let frozen = scratch.path("frozen.wav");
    shell
        .run(
            "export.audio",
            json!({ "path": frozen.display().to_string(), "bit_depth": 32,
                    "sample_rate": 48_000 }),
        )
        .unwrap();
    assert_eq!(
        std::fs::read(&live).unwrap(),
        std::fs::read(&frozen).unwrap(),
        "a frozen track gives the frames the live track gave"
    );
}

#[test]
fn a_frozen_track_refuses_device_and_clip_edits_and_arming() {
    let scratch = Scratch::new("locks");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    let clip = place(&mut shell, track, &file, 0);
    freeze(&mut shell, track);

    let arm = shell.run("mixer.set_arm", json!({ "track": track, "arm": true }));
    assert!(arm.is_err(), "a frozen track can't be armed");
    let gain = shell.run("clip.set_gain", json!({ "clip": clip, "gain": 0.5 }));
    assert!(gain.is_err(), "a frozen clip's settings are locked");
    // R7: the clip still moves.
    shell
        .run(
            "clip.move",
            json!({ "clip": clip, "track": track, "start": 705_600 }),
        )
        .unwrap();
}

#[test]
fn a_group_freezes_as_one_track() {
    let scratch = Scratch::new("group");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    place(&mut shell, track, &file, 0);
    let grouped = shell
        .run("track.group", json!({ "tracks": [track] }))
        .unwrap();
    let group: TrackId = serde_json::from_value(grouped["track"].clone()).unwrap();

    freeze(&mut shell, group);
    let frozen = shell
        .editor()
        .document()
        .track(group)
        .unwrap()
        .frozen()
        .unwrap()
        .clone();
    assert!(frozen.output.is_some(), "a group freezes its whole output");
    assert_eq!(frozen.clips.len(), 0);
    // The tracks inside it play through the group's render: an export still
    // gives the group's audio (R14).
    let out = scratch.path("group.wav");
    shell
        .run(
            "export.audio",
            json!({ "path": out.display().to_string(), "bit_depth": 32,
                    "sample_rate": 48_000 }),
        )
        .unwrap();
    assert!(std::fs::metadata(&out).unwrap().len() > 44);
}

#[test]
fn a_frozen_track_still_mixes() {
    let scratch = Scratch::new("mixes");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    place(&mut shell, track, &file, 0);
    freeze(&mut shell, track);
    let ret = shell
        .editor()
        .document()
        .returns()
        .next()
        .expect("the default set has a return track");

    // R7: volume, pan, sends, the Track Activator and solo still work.
    for (id, args) in [
        ("mixer.set_volume", json!({ "track": track, "volume": 0.5 })),
        ("mixer.set_pan", json!({ "track": track, "pan": 0.3 })),
        (
            "mixer.set_send",
            json!({ "track": track, "to": ret, "level": 1.0 }),
        ),
        ("mixer.set_mute", json!({ "track": track, "mute": true })),
        ("mixer.set_solo", json!({ "track": track, "solo": true })),
    ] {
        shell
            .run(id, args)
            .unwrap_or_else(|error| panic!("{id} on a frozen track: {error}"));
    }
    let doc = shell.editor().document();
    let held = doc.track(track).unwrap();
    assert_eq!(held.mixer().volume, 0.5);
    assert_eq!(held.mixer().pan, 0.3);
    assert!(held.mixer().mute && held.mixer().solo);
    assert_eq!(held.send(ret), 1.0);
    // R8 still refuses arming.
    assert!(
        shell
            .run("mixer.set_arm", json!({ "track": track, "arm": true }))
            .is_err(),
        "a frozen track can't be armed"
    );
}

#[test]
fn a_frozen_track_inside_a_group_nulls_against_its_live_output() {
    let scratch = Scratch::new("grouped-null");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    let clip = place(&mut shell, track, &file, 4 * 705_600);
    let grouped = shell
        .run("track.group", json!({ "tracks": [track] }))
        .unwrap();
    let group: TrackId = serde_json::from_value(grouped["track"].clone()).unwrap();
    // The group's mixer acts on the frozen track's playback, not on its
    // render (R2), so it mustn't be applied twice.
    shell
        .run("mixer.set_volume", json!({ "track": group, "volume": 0.5 }))
        .unwrap();

    let (start, end) = {
        let doc = shell.editor().document();
        let clip = doc.clip(clip).unwrap();
        (clip.start, clip.end())
    };
    shell
        .run(
            "arrangement.select_time",
            json!({ "start": start.get(), "end": end.get() }),
        )
        .unwrap();
    let live = scratch.path("live.wav");
    shell
        .run(
            "export.audio",
            json!({ "path": live.display().to_string(), "bit_depth": 32,
                    "sample_rate": 48_000 }),
        )
        .unwrap();
    freeze(&mut shell, track);
    let frozen = scratch.path("frozen.wav");
    shell
        .run(
            "export.audio",
            json!({ "path": frozen.display().to_string(), "bit_depth": 32,
                    "sample_rate": 48_000 }),
        )
        .unwrap();
    assert_eq!(
        std::fs::read(&live).unwrap(),
        std::fs::read(&frozen).unwrap(),
        "a track frozen inside a group gives the frames the live track gave"
    );
}

#[test]
fn a_frozen_return_renders_what_is_sent_to_it() {
    let scratch = Scratch::new("return");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    place(&mut shell, track, &file, 0);
    let ret = shell
        .editor()
        .document()
        .returns()
        .next()
        .expect("the default set has a return track");
    shell
        .run(
            "mixer.set_send",
            json!({ "track": track, "to": ret, "level": 1.0 }),
        )
        .unwrap();

    let result = freeze(&mut shell, ret);
    let written = result["files"][0].as_str().unwrap();
    let decoded = bl_media::decode_file(Path::new(written)).unwrap();
    let loud = (0..decoded.channel_count()).any(|channel| {
        decoded
            .channel(channel)
            .unwrap()
            .iter()
            .any(|sample| sample.abs() > 1e-6)
    });
    assert!(loud, "R15: a frozen return's file holds what is sent to it");
}

// Spec: freeze-and-bounce R8. A clip on a frozen track keeps its settings:
// reversing it, and reversing a selection it sits in, are refused.
#[test]
fn a_frozen_track_s_clips_arent_reversed() {
    let scratch = Scratch::new("reverse");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    let clip = place(&mut shell, track, &file, 0);
    freeze(&mut shell, track);

    let refused = shell.run("clip.reverse", json!({ "clip": clip }));
    assert!(refused.is_err(), "a clip on a frozen track isn't reversed");
    let refused = shell.run(
        "arrangement.reverse",
        json!({ "tracks": [track], "start": 0, "end": 2_822_400 }),
    );
    assert!(
        refused.is_err(),
        "a selection on a frozen track isn't reversed"
    );
}

fn insert_device(shell: &mut Shell, track: TrackId, device_type: &str) -> DeviceId {
    let device: DeviceId = shell.new_id();
    shell
        .run(
            "device.insert",
            json!({ "track": track, "index": 0, "device": {
                "id": device, "name": device_type, "enabled": true, "params": [],
                "kind": { "native": { "device_type": device_type } } } }),
        )
        .unwrap();
    device
}

fn export(shell: &mut Shell, path: &Path) {
    shell
        .run(
            "export.audio",
            json!({ "path": path.display().to_string(), "bit_depth": 32,
                    "sample_rate": 48_000 }),
        )
        .unwrap();
}

// Spec: freeze-and-bounce R6, delay-compensation R10. A track with a
// look-ahead freezes where it was, beside a track that has none.
#[test]
fn a_frozen_track_lines_up_with_an_unfrozen_neighbour() {
    let scratch = Scratch::new("neighbour");
    let mut shell = new_shell(&scratch);
    let a = audio_track(&shell);
    let b: TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            json!({ "id": b, "kind": "audio", "name": "B", "index": 1 }),
        )
        .unwrap();
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    let clip = place(&mut shell, a, &file, 0);
    place(&mut shell, b, &file, 0);
    // The Limiter's default look-ahead is 3 ms.
    insert_device(&mut shell, a, "beatlayer.brickwall");
    let end = shell.editor().document().clip(clip).unwrap().end();
    shell
        .run(
            "arrangement.select_time",
            json!({ "start": 0, "end": end.get() }),
        )
        .unwrap();
    let live = scratch.path("live.wav");
    export(&mut shell, &live);
    freeze(&mut shell, a);
    let frozen = scratch.path("frozen.wav");
    export(&mut shell, &frozen);
    assert_eq!(
        std::fs::read(&live).unwrap(),
        std::fs::read(&frozen).unwrap(),
        "a frozen track plays where it did, beside a track without a look-ahead"
    );
}

// Spec: freeze-and-bounce R14. A group's tracks' sends to returns would
// vanish with the tracks, so the freeze is refused.
#[test]
fn a_group_whose_tracks_send_to_returns_is_refused() {
    let scratch = Scratch::new("group-sends");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    place(&mut shell, track, &file, 0);
    let ret = shell.editor().document().returns().next().unwrap();
    shell
        .run(
            "mixer.set_send",
            json!({ "track": track, "to": ret, "level": 1.0 }),
        )
        .unwrap();
    let grouped = shell
        .run("track.group", json!({ "tracks": [track] }))
        .unwrap();
    let group: TrackId = serde_json::from_value(grouped["track"].clone()).unwrap();
    assert!(
        shell
            .run(ids::FREEZE, json!({ "tracks": [group] }))
            .is_err()
    );
    assert!(!shell.editor().document().track(group).unwrap().is_frozen());
}

// Spec: freeze-and-bounce R15. A frozen return's file runs to its tail.
#[test]
fn a_frozen_return_keeps_its_tail() {
    let scratch = Scratch::new("return-tail");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 0.25, 2);
    let clip = place(&mut shell, track, &file, 0);
    let ret = shell.editor().document().returns().next().unwrap();
    // Echoes 40 ms apart, so the render's 100 ms quiet rule doesn't end the
    // tail between them.
    let delay = insert_device(&mut shell, ret, "bl.device.reprise");
    for (param, value) in [(0, 0.0), (12, 40.0), (13, 40.0), (6, 0.5), (9, 1.0)] {
        shell
            .run(
                "device.set_param",
                json!({ "device": delay, "param": param, "value": value }),
            )
            .unwrap();
    }
    shell
        .run(
            "mixer.set_send",
            json!({ "track": track, "to": ret, "level": 1.0 }),
        )
        .unwrap();
    let clip_end = shell.editor().document().clip(clip).unwrap().end();

    let result = freeze(&mut shell, ret);
    let output = shell
        .editor()
        .document()
        .track(ret)
        .unwrap()
        .frozen()
        .unwrap()
        .output
        .unwrap();
    assert!(output.length > clip_end, "the output runs past the clip");
    let decoded = bl_media::decode_file(Path::new(result["files"][0].as_str().unwrap())).unwrap();
    let samples = decoded.channel(0).unwrap();
    let clip_frames = 12_000;
    assert!(samples.len() > clip_frames);
    assert!(
        samples[clip_frames..].iter().any(|s| s.abs() > 1e-4),
        "the delay's echoes are in the file"
    );
}

/// Sets the Song Tempo to a ramp from 100 to 140 BPM over four bars, then
/// 140.
fn ramp_tempo(shell: &mut Shell) {
    shell
        .run(
            "song.set_tempo",
            json!({ "tempo": [
                { "kind": "ramp", "start": 0, "from": 100.0, "to": 140.0 },
                { "kind": "constant", "start": 16 * 705_600, "bpm": 140.0 }
            ] }),
        )
        .unwrap();
}

// Spec: freeze-and-bounce R6. Under a tempo ramp a frozen clip's markers keep
// each beat on the second the render reached it.
#[test]
fn a_frozen_clip_keeps_its_beats_on_the_seconds_they_were_rendered_at() {
    let scratch = Scratch::new("ramp");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 4.0, 2);
    let clip = place(&mut shell, track, &file, 0);
    ramp_tempo(&mut shell);
    freeze(&mut shell, track);

    let doc = shell.editor().document();
    let held = doc.clip(clip).unwrap();
    let region = doc.track(track).unwrap().frozen().unwrap().clips[0].clone();
    assert!(!region.markers.is_empty(), "interior markers on a ramp");
    let tempo = doc.tempo_map();
    for marker in &region.markers {
        let expected =
            tempo.ticks_to_seconds(held.start + marker.beat) - tempo.ticks_to_seconds(held.start);
        assert!(
            (marker.time - expected).abs() < 1.0 / 48_000.0,
            "{marker:?} is not on {expected}"
        );
    }
}

// Spec: freeze-and-bounce R6. A leading clip's tempo moves the clips after it.
#[test]
fn a_frozen_clips_span_follows_a_tempo_leader() {
    let scratch = Scratch::new("leader");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    place(&mut shell, track, &file, 0);
    let later = place(&mut shell, track, &file, 4 * 705_600);
    // A leader over the first bar at 60 BPM: four beats take four seconds.
    let leader_track: TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            json!({ "id": leader_track, "kind": "audio", "name": "Lead", "index": 1 }),
        )
        .unwrap();
    let leader: ClipId = shell.new_id();
    let data = bl_model::AudioData {
        end: Some((4 * 705_600) as f64),
        tempo: Some(60.0),
        warp: true,
        warp_markers: vec![
            bl_model::WarpMarker {
                time: 0.0,
                beat: bl_core::time::Ticks(0),
            },
            bl_model::WarpMarker {
                time: 4.0,
                beat: bl_core::time::Ticks(4 * 705_600),
            },
        ],
        ..bl_model::AudioData::new(bl_model::ContentHash([3; 32]))
    };
    let clip = bl_model::Clip::new(
        leader,
        bl_core::time::Ticks(0),
        bl_core::time::Ticks(4 * 705_600),
        bl_model::ClipData::Audio(data),
    );
    shell
        .run(
            "clip.create",
            json!({ "track": leader_track, "clip": clip }),
        )
        .unwrap();
    shell
        .run(
            "clip.set_tempo_leader",
            json!({ "clip": leader, "leader": true }),
        )
        .unwrap();
    freeze(&mut shell, track);

    let frozen = shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .frozen()
        .unwrap()
        .clone();
    let region = frozen
        .clips
        .iter()
        .find(|region| region.clip == later)
        .unwrap();
    assert!(
        (region.start - 4.0).abs() < 1e-3,
        "the later clip starts at {}, after four seconds of leader",
        region.start
    );
}

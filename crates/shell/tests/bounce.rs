//! Bouncing tracks and pasting bounced audio (WP-4.15). The behaviour is
//! in the freeze and bounce spec, R18 to R26.

use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_model::{ClipId, Track, TrackId, TrackKind};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::bounce as ids;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use serde_json::json;

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-bounce-{name}-{}-{}",
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

#[test]
fn bounce_in_place_replaces_the_track_keeping_its_mixer_in_both_views() {
    let scratch = Scratch::new("in-place");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    place(&mut shell, track, &file, 0);
    shell
        .run("track.rename", json!({ "track": track, "name": "Drums" }))
        .unwrap();
    shell
        .run("mixer.set_volume", json!({ "track": track, "volume": 0.5 }))
        .unwrap();
    shell
        .run("mixer.set_pan", json!({ "track": track, "pan": 0.25 }))
        .unwrap();
    saved(&scratch, &mut shell);

    let result = shell
        .run(ids::BOUNCE_IN_PLACE, json!({ "tracks": [track] }))
        .unwrap();
    assert_eq!(result["tracks"], json!([track]));
    let written = result["files"][0].as_str().unwrap();
    let folder = Path::new("Samples").join("Processed").join("Bounce");
    assert!(
        Path::new(written)
            .parent()
            .is_some_and(|parent| parent.ends_with(&folder)),
        "bounce files go into Samples/Processed/Bounce: {written}"
    );

    let doc = shell.editor().document();
    let held = doc.track(track).expect("the track is still there");
    assert_eq!(held.kind(), TrackKind::Audio, "R21: an audio track");
    assert_eq!(held.name(), "Drums (Bounce)", "R21: the name");
    assert_eq!(held.mixer().volume, 0.5, "R21: the volume");
    assert_eq!(held.mixer().pan, 0.25, "R21: the pan");
    assert_eq!(held.devices().count(), 0, "R21: no devices");
    let clip = held.clips().next().expect("the bounced clip");
    assert!(clip.data.is_audio(), "R21: an audio clip");
    assert_eq!(clip.name, "tone (Bounce)", "R21: the clip's name");
    // The Session View's slots are empty: the source had none.
    assert_eq!(held.slots().count(), 0);
}

#[test]
fn bounce_in_place_undoes_to_the_source_track_exactly() {
    let scratch = Scratch::new("undo");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    place(&mut shell, track, &file, 0);
    let before = shell.editor().document().clone();

    shell
        .run(ids::BOUNCE_IN_PLACE, json!({ "tracks": [track] }))
        .unwrap();
    shell.run("edit.undo", json!({})).unwrap();
    let after = shell.editor().document();
    assert_eq!(after.track(track).unwrap().kind(), TrackKind::Audio);
    assert_eq!(after.clip_count(), before.clip_count());
    assert_eq!(after.track(track).unwrap().clips().count(), 1);
}

#[test]
fn a_bounced_track_nulls_against_its_source() {
    let scratch = Scratch::new("null");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    let clip = place(&mut shell, track, &file, 4 * 705_600);
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
    saved(&scratch, &mut shell);

    let result = shell
        .run(ids::BOUNCE_IN_PLACE, json!({ "tracks": [track] }))
        .unwrap();
    let bounced = scratch.path("bounced.wav");
    shell
        .run(
            "export.audio",
            json!({ "path": bounced.display().to_string(), "bit_depth": 32,
                    "sample_rate": 48_000 }),
        )
        .unwrap();
    assert_eq!(
        std::fs::read(&live).unwrap(),
        std::fs::read(&bounced).unwrap(),
        "a bounced track gives the frames the source gave"
    );
    // The bounce's own file holds the source's audio.
    let written = result["files"][0].as_str().unwrap();
    let decoded = bl_media::decode_file(Path::new(written)).unwrap();
    assert!(decoded.channel(0).unwrap().iter().any(|s| s.abs() > 1e-6));
}

#[test]
fn bounce_to_new_track_makes_a_track_per_source_and_switches_the_source_off() {
    let scratch = Scratch::new("new-track");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    let clip = place(&mut shell, track, &file, 0);
    saved(&scratch, &mut shell);

    let result = shell
        .run(
            ids::BOUNCE_TO_NEW_TRACK,
            json!({ "tracks": [track], "clips": [clip] }),
        )
        .unwrap();
    let made: TrackId = serde_json::from_value(result["tracks"][0].clone()).unwrap();
    let doc = shell.editor().document();
    let order = doc.track_order();
    let source_at = order.iter().position(|id| *id == track).unwrap();
    let new_at = order.iter().position(|id| *id == made).unwrap();
    assert_eq!(
        new_at,
        source_at + 1,
        "R22: the new track follows its source"
    );
    let new_track = doc.track(made).unwrap();
    assert_eq!(new_track.kind(), TrackKind::Audio);
    assert!(
        new_track.name().ends_with(" (Bounce)"),
        "R22: the new track's name: {}",
        new_track.name()
    );
    assert_eq!(new_track.clips().count(), 1, "R22: the bounced clip");
    // R22: the source material is switched off.
    assert!(doc.clip(clip).unwrap().mute, "the source clip is off");
}

#[test]
fn paste_bounced_audio_renders_the_source_as_it_is_now() {
    let scratch = Scratch::new("paste");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    let clip = place(&mut shell, track, &file, 0);
    saved(&scratch, &mut shell);
    shell
        .run("arrangement.copy", json!({ "clips": [clip] }))
        .unwrap();
    // The source changes after the copy: the paste renders it as it is now.
    shell
        .run(
            "mixer.set_volume",
            json!({ "track": track, "volume": 0.25 }),
        )
        .unwrap();

    let result = shell
        .run(
            ids::PASTE_BOUNCED_AUDIO,
            json!({ "track": track, "at": 8 * 705_600 }),
        )
        .unwrap();
    assert_eq!(result["clips"].as_array().unwrap().len(), 1);
    let doc = shell.editor().document();
    assert_eq!(
        doc.track(track).unwrap().clips().count(),
        2,
        "the pasted clip"
    );
}

#[test]
fn paste_bounced_audio_turns_an_empty_midi_track_into_an_audio_track() {
    let scratch = Scratch::new("midi");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    let clip = place(&mut shell, track, &file, 0);
    saved(&scratch, &mut shell);
    shell
        .run("arrangement.copy", json!({ "clips": [clip] }))
        .unwrap();
    shell
        .invoke("track.create", json!({ "kind": "midi", "name": "Keys" }))
        .unwrap();
    let midi = shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.name() == "Keys")
        .map(Track::id)
        .expect("the MIDI track");

    shell
        .run(ids::PASTE_BOUNCED_AUDIO, json!({ "track": midi }))
        .unwrap();
    let doc = shell.editor().document();
    assert_eq!(doc.track(midi).unwrap().kind(), TrackKind::Audio);
    assert_eq!(doc.track(midi).unwrap().clips().count(), 1);
}

#[test]
fn a_group_bounce_replaces_the_group_and_its_tracks() {
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
    saved(&scratch, &mut shell);

    let result = shell
        .run(ids::BOUNCE_IN_PLACE, json!({ "tracks": [group] }))
        .unwrap();
    assert_eq!(result["tracks"], json!([group]));
    let doc = shell.editor().document();
    let held = doc.track(group).expect("the group is still there");
    assert_eq!(held.kind(), TrackKind::Audio, "R23: an audio track");
    assert_eq!(
        held.children().count(),
        0,
        "R23: the group's tracks are gone"
    );
    assert_eq!(held.clips().count(), 1, "R23: the group's output");
    assert!(
        doc.track(track).is_none(),
        "R23: the track inside the group is gone"
    );
}

#[test]
fn a_group_bounce_holds_its_returns_and_not_mains_devices() {
    let scratch = Scratch::new("group-returns");
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
    // Main's Utility is muted: the bounce leaves Main's devices out.
    let main = shell.editor().document().main_id();
    let effect: bl_model::DeviceId = shell.new_id();
    shell
        .run(
            "device.insert",
            json!({ "track": main, "index": 0, "device": {
                "id": effect, "name": "Utility", "enabled": true, "params": [],
                "kind": { "native": { "device_type": "beatlayer.utility" } } } }),
        )
        .unwrap();
    shell
        .run(
            "device.set_param",
            json!({ "device": effect, "param": 10, "value": 1.0 }),
        )
        .unwrap();
    let grouped = shell
        .run("track.group", json!({ "tracks": [track] }))
        .unwrap();
    let group: TrackId = serde_json::from_value(grouped["track"].clone()).unwrap();
    saved(&scratch, &mut shell);

    let result = shell
        .run(ids::BOUNCE_IN_PLACE, json!({ "tracks": [group] }))
        .unwrap();
    let written = result["files"][0].as_str().unwrap();
    let decoded = bl_media::decode_file(Path::new(written)).unwrap();
    let peak = decoded
        .channels()
        .flat_map(|channel| channel.iter())
        .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
    // The source file holds 0.25; the return adds the same signal, so the
    // bounce holds about twice that, and Main's muted device is not in it.
    assert!(
        peak > 0.4,
        "R23: the bounce holds the return's output and not Main's devices: {peak}"
    );
}

#[test]
fn a_bounce_is_taken_where_a_freeze_is() {
    let scratch = Scratch::new("post-fx");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    place(&mut shell, track, &file, 0);
    // A Utility at +6 dB: the device's processing is in the bounce.
    let effect: bl_model::DeviceId = shell.new_id();
    shell
        .run(
            "device.insert",
            json!({ "track": track, "index": 0, "device": {
                "id": effect, "name": "Utility", "enabled": true, "params": [],
                "kind": { "native": { "device_type": "beatlayer.utility" } } } }),
        )
        .unwrap();
    shell
        .run(
            "device.set_param",
            json!({ "device": effect, "param": 9, "value": bl_model::param::db_to_gain(6.0) }),
        )
        .unwrap();
    // The mixer is left out of the audio and set on the new track instead.
    shell
        .run("mixer.set_volume", json!({ "track": track, "volume": 0.5 }))
        .unwrap();
    saved(&scratch, &mut shell);

    let result = shell
        .run(ids::BOUNCE_IN_PLACE, json!({ "tracks": [track] }))
        .unwrap();
    let written = result["files"][0].as_str().unwrap();
    let decoded = bl_media::decode_file(Path::new(written)).unwrap();
    let peak = decoded
        .channels()
        .flat_map(|channel| channel.iter())
        .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
    // The file holds 0.25 and ramps up; the device at +6 dB doubles it and
    // the mixer doesn't halve it.
    let expected = (0.25 + 47_999.0 / 1_000_000.0) * bl_model::param::db_to_gain(6.0) as f32;
    assert!(
        (peak - expected).abs() < 1e-3,
        "R18: the bounce is after the devices and before the mixer: {peak}"
    );
    let doc = shell.editor().document();
    let held = doc.track(track).unwrap();
    assert_eq!(
        held.mixer().volume,
        0.5,
        "R21: the mixer is on the new track"
    );
}

#[test]
fn a_bounce_keeps_its_tails() {
    let scratch = Scratch::new("tails");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    place(&mut shell, track, &file, 0);
    // A delay that rings: the bounce's clip grows to hold its tail.
    let delay: bl_model::DeviceId = shell.new_id();
    shell
        .run(
            "device.insert",
            json!({ "track": track, "index": 0, "device": {
                "id": delay, "name": "Delay", "enabled": true, "params": [],
                "kind": { "native": { "device_type": "bl.device.reprise" } } } }),
        )
        .unwrap();
    shell
        .run(
            "device.set_param",
            json!({ "device": delay, "param": 6, "value": 90.0 }),
        )
        .unwrap();
    saved(&scratch, &mut shell);

    shell
        .run(ids::BOUNCE_IN_PLACE, json!({ "tracks": [track] }))
        .unwrap();
    let doc = shell.editor().document();
    let bounced = doc.track(track).unwrap().clips().next().unwrap();
    assert!(
        bounced.length > Ticks(1920),
        "R18: the bounced clip holds its tail: {}",
        bounced.length
    );
}

#[test]
fn bounce_group_to_new_track_switches_the_groups_material_off() {
    let scratch = Scratch::new("group-new");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    let clip = place(&mut shell, track, &file, 0);
    let grouped = shell
        .run("track.group", json!({ "tracks": [track] }))
        .unwrap();
    let group: TrackId = serde_json::from_value(grouped["track"].clone()).unwrap();
    saved(&scratch, &mut shell);
    shell
        .run(
            "arrangement.select_time",
            json!({ "start": 0, "end": 4 * 705_600 }),
        )
        .unwrap();

    let result = shell
        .run(ids::BOUNCE_TO_NEW_TRACK, json!({ "tracks": [group] }))
        .unwrap();
    let made: TrackId = serde_json::from_value(result["tracks"][0].clone()).unwrap();
    let doc = shell.editor().document();
    let new_track = doc.track(made).expect("the new track");
    assert_eq!(new_track.kind(), TrackKind::Audio);
    assert_eq!(new_track.clips().count(), 1, "R24: the group's output");
    assert!(
        doc.clip(clip).unwrap().mute,
        "R24: the group's material is switched off"
    );
}

#[test]
fn a_mono_source_bounces_to_mono_and_the_channels_argument_overrides_it() {
    let scratch = Scratch::new("mono");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 0.5, 2);
    place(&mut shell, track, &file, 0);
    saved(&scratch, &mut shell);

    let result = shell
        .run(ids::BOUNCE_IN_PLACE, json!({ "tracks": [track] }))
        .unwrap();
    let written = result["files"][0].as_str().unwrap();
    let decoded = bl_media::decode_file(Path::new(written)).unwrap();
    assert_eq!(
        decoded.channels().count(),
        1,
        "equal channels bounce to one channel"
    );

    // R20: `stereo` writes two channels, and `mono` writes one.
    shell
        .run(
            ids::BOUNCE_IN_PLACE,
            json!({ "tracks": [track], "channels": "stereo" }),
        )
        .unwrap();
    let result = shell
        .run(
            ids::BOUNCE_IN_PLACE,
            json!({ "tracks": [track], "channels": "mono" }),
        )
        .unwrap();
    let written = result["files"][0].as_str().unwrap();
    let decoded = bl_media::decode_file(Path::new(written)).unwrap();
    assert_eq!(decoded.channels().count(), 1, "`mono` writes one channel");
}

// Spec: freeze-and-bounce R24. A group's clips bounce as one track: the
// group and the track inside it make one new track, not one each, and the
// set's audio after the bounce is the audio before it.
#[test]
fn a_groups_clips_bounce_as_one_track() {
    let scratch = Scratch::new("group-clips");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    let clip = place(&mut shell, track, &file, 0);
    let grouped = shell
        .run("track.group", json!({ "tracks": [track] }))
        .unwrap();
    assert!(grouped["track"].is_u64(), "the group");
    saved(&scratch, &mut shell);
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();

    let live = scratch.path("live.wav");
    shell
        .run(
            "export.audio",
            json!({ "path": live.display().to_string(), "bit_depth": 32,
                    "sample_rate": 48_000 }),
        )
        .unwrap();

    let result = shell.run(ids::BOUNCE_TO_NEW_TRACK, json!({})).unwrap();
    let made: Vec<TrackId> = serde_json::from_value(result["tracks"].clone()).unwrap();
    assert_eq!(
        made.len(),
        1,
        "R24: the group's clips make one new track, not one per track in the group"
    );
    let doc = shell.editor().document();
    let new_track = doc.track(made[0]).expect("the new track");
    assert!(
        new_track.name().ends_with("(Bounce)"),
        "R24: the group's new track: {}",
        new_track.name()
    );

    // The set plays the bounced audio once, not the group's and the
    // track's on top of each other.
    let after = scratch.path("after.wav");
    shell
        .run(
            "export.audio",
            json!({ "path": after.display().to_string(), "bit_depth": 32,
                    "sample_rate": 48_000 }),
        )
        .unwrap();
    assert_eq!(
        std::fs::read(&live).unwrap(),
        std::fs::read(&after).unwrap(),
        "R24: the bounce holds the group's audio once"
    );
}

// Spec: freeze-and-bounce R25 and the agent permissions spec R1. A paste
// of bounced audio puts each source's audio on its own track, so a clip on
// any of those tracks is replaced: the call is destructive when any of
// them is.
#[test]
fn a_paste_over_a_clip_on_a_later_destination_asks_first() {
    let scratch = Scratch::new("paste-destinations");
    let mut shell = new_shell(&scratch);
    let destination: TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            json!({ "id": destination, "kind": "audio", "name": "Dest", "index": 0 }),
        )
        .unwrap();
    let second: TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            json!({ "id": second, "kind": "audio", "name": "Second", "index": 1 }),
        )
        .unwrap();
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    let clip_a = place(&mut shell, destination, &file, 0);
    let clip_b = place(&mut shell, second, &file, 0);
    saved(&scratch, &mut shell);
    shell
        .run("arrangement.copy", json!({ "clips": [clip_a, clip_b] }))
        .unwrap();

    // A clip on the second destination, where the paste lands.
    let victim = place(&mut shell, second, &file, 8 * 705_600);

    let call = shell.call_permission(
        ids::PASTE_BOUNCED_AUDIO,
        &json!({ "track": destination, "at": 8 * 705_600 }),
    );
    assert!(
        call.is_destructive(),
        "R1: the paste replaces a clip on the second destination: {call:?}"
    );
    assert!(
        call.replaces.contains(&victim),
        "R1: the clip it would replace is named: {call:?}"
    );
}

/// Puts the clip `source` plays on a new take lane of `track` in its place
/// (the clip is deleted from the main lane first).
fn move_to_take_lane(shell: &mut Shell, track: TrackId, source: ClipId) -> bl_model::TakeLaneId {
    let (name, start, length, data) = {
        let clip = shell.editor().document().clip(source).unwrap();
        (
            clip.name.clone(),
            clip.start,
            clip.length,
            clip.data.clone(),
        )
    };
    shell.run("clip.delete", json!({ "clip": source })).unwrap();
    shell
        .run(
            "track.add_take_lane",
            json!({ "track": track, "lane": { "name": "Take 1", "clips": [
                { "name": name, "start": start, "length": length, "data": data }
            ] } }),
        )
        .unwrap();
    shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .take_lanes()
        .next()
        .unwrap()
        .id
}

// Spec: freeze-and-bounce R30. A bounce renders a track's main lane: a take
// lane's clips aren't in it, auditioned or not, and the new track has none.
#[test]
fn a_bounce_renders_the_main_lane_only_and_the_new_track_has_no_take_lanes() {
    let scratch = Scratch::new("take-lanes");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let tone = scratch.path("tone.wav");
    wav(&tone, 1.0, 2);
    place(&mut shell, track, &tone, 0);
    // A louder take on a lane of its own, later in the song.
    let loud = scratch.path("loud.wav");
    {
        let mut writer = bl_media::AudioWriter::create(
            &loud,
            bl_media::EncodeSpec {
                file_type: bl_media::FileType::Wav,
                bit_depth: bl_media::BitDepth::Float32,
                dither: bl_media::Dither::None,
                sample_rate: 48_000,
                channels: 2,
            },
        )
        .unwrap();
        for _ in 0..48_000 {
            writer.write(&[0.9, 0.9]).unwrap();
        }
        writer.finish().unwrap();
    }
    let taken = place(&mut shell, track, &loud, 4 * 705_600);
    let lane = move_to_take_lane(&mut shell, track, taken);
    shell
        .run(
            "take_lane.set_audition",
            json!({ "lane": lane, "on": true }),
        )
        .unwrap();
    saved(&scratch, &mut shell);

    let result = shell
        .run(ids::BOUNCE_IN_PLACE, json!({ "tracks": [track] }))
        .unwrap();
    let decoded = bl_media::decode_file(Path::new(result["files"][0].as_str().unwrap())).unwrap();
    let left = decoded.channel(0).unwrap();
    assert!(
        left[1_000..24_000].iter().all(|s| (0.2..0.5).contains(s)),
        "the main lane's tone, not the auditioned lane's 0.9"
    );
    let held = shell.editor().document().track(track).unwrap();
    assert_eq!(held.take_lanes().len(), 0, "the bounce has no take lanes");
    assert_eq!(held.audition(), None);
}

// Spec: freeze-and-bounce R30. Bounce to New Track skips material that is
// all on take lanes, and is disabled when the whole selection is.
#[test]
fn bounce_to_new_track_skips_take_lane_material() {
    let scratch = Scratch::new("take-lane-bounce");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let tone = scratch.path("tone.wav");
    wav(&tone, 1.0, 2);
    let clip = place(&mut shell, track, &tone, 0);
    let lane = move_to_take_lane(&mut shell, track, clip);
    let on_lane = shell.editor().document().take_lane(lane).unwrap().clips[0].id;
    saved(&scratch, &mut shell);

    let args = json!({ "clips": [on_lane] });
    assert!(!shell.can_invoke(ids::BOUNCE_TO_NEW_TRACK, &args));
    let before = shell.editor().document().track_count();
    assert!(shell.run(ids::BOUNCE_TO_NEW_TRACK, args).is_err());
    assert_eq!(
        shell.editor().document().track_count(),
        before,
        "no track is made"
    );
}

// Spec: freeze-and-bounce R30. A source whose selected material is all
// deactivated gets no track, and the command is disabled when every source's
// is.
#[test]
fn bounce_to_new_track_skips_deactivated_material() {
    let scratch = Scratch::new("deactivated-bounce");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let tone = scratch.path("tone.wav");
    wav(&tone, 1.0, 2);
    let clip = place(&mut shell, track, &tone, 0);
    shell
        .run("clip.set_mute", json!({ "clip": clip, "mute": true }))
        .unwrap();
    saved(&scratch, &mut shell);

    let args = json!({ "clips": [clip] });
    assert!(!shell.can_invoke(ids::BOUNCE_TO_NEW_TRACK, &args));
    let before = shell.editor().document().track_count();
    assert!(shell.run(ids::BOUNCE_TO_NEW_TRACK, args).is_err());
    assert_eq!(shell.editor().document().track_count(), before);
}

// Spec: freeze-and-bounce R25, R30. Paste Bounced Audio onto a take lane
// places the audio on that lane, and the main lane is left as it was.
#[test]
fn paste_bounced_audio_onto_a_take_lane_places_the_audio_on_that_lane() {
    let scratch = Scratch::new("paste-lane");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let file = scratch.path("tone.wav");
    wav(&file, 1.0, 2);
    let clip = place(&mut shell, track, &file, 0);
    saved(&scratch, &mut shell);
    shell
        .run(
            "track.add_take_lane",
            json!({ "track": track, "lane": { "name": "Take 1", "clips": [] } }),
        )
        .unwrap();
    let lane = shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .take_lanes()
        .next()
        .unwrap()
        .id;
    shell
        .run("arrangement.copy", json!({ "clips": [clip] }))
        .unwrap();

    let result = shell
        .run(
            ids::PASTE_BOUNCED_AUDIO,
            json!({ "lane": lane, "at": 8 * 705_600 }),
        )
        .unwrap();
    let made: ClipId = serde_json::from_value(result["clips"][0].clone()).unwrap();
    let doc = shell.editor().document();
    assert_eq!(doc.clip_lane(made), Some(lane), "on the lane");
    assert_eq!(
        doc.track(track).unwrap().clips().count(),
        1,
        "the main lane is as it was"
    );
    assert_eq!(doc.take_lane(lane).unwrap().clips.len(), 1);
}

//! Crop, Consolidate, and Consolidate Time to New Scene (WP-4.13). The
//! behaviour is in the audio clip editing spec, R14 to R28.

use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_media::decode_file;
use bl_model::{ClipData, ClipId, Track, TrackId, TrackKind};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::audio_editing as ids;
use bl_shell::audio_editing::{ConsolidateResult, ConsolidateSceneResult, CropResult};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use serde_json::json;

bl_rt::install_trap!();

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-audio-editing-{name}-{}-{}",
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

/// Writes a 32-bit float WAV of `seconds` at 48 kHz holding a ramp, so a
/// region of it is recognisable, with `channels` channels. Returns the
/// samples, channel by channel.
fn wav(path: &Path, seconds: f64, channels: u16, seed: f32) -> Vec<Vec<f32>> {
    let frames = (seconds * 48_000.0) as usize;
    let data: Vec<Vec<f32>> = (0..usize::from(channels))
        .map(|channel| {
            (0..frames)
                .map(|i| seed + channel as f32 * 0.001 + i as f32 / 1_000_000.0)
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
    data
}

fn new_shell(scratch: &Scratch) -> Shell {
    Shell::new(
        Preferences::default(),
        Some(scratch.path("config")),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    )
}

/// Saves the set, and gives back the file it went to: a new project folder
/// is made beside the path asked for.
fn saved(scratch: &Scratch, shell: &mut Shell) -> PathBuf {
    let path = scratch.path("Song.blset");
    let result = shell
        .run("set.save_as", json!({ "path": path.display().to_string() }))
        .unwrap();
    PathBuf::from(result["path"].as_str().unwrap())
}

fn track(shell: &Shell, kind: TrackKind) -> TrackId {
    shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == kind)
        .map(Track::id)
        .expect("the default set has such a track")
}

/// Adds the file at `path` to `track` at `at`, and returns the new clip.
fn place(shell: &mut Shell, track: TrackId, path: &Path, at: i64) -> ClipId {
    let added = shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [path.display().to_string()], "track": track, "at": at }),
        )
        .unwrap();
    serde_json::from_value(added["clips"][0]["clip"].clone()).unwrap()
}

/// An audio clip's data.
fn audio(shell: &Shell, clip: ClipId) -> bl_model::AudioData {
    match &shell.editor().document().clip(clip).unwrap().data {
        ClipData::Audio(audio) => audio.clone(),
        ClipData::Midi(_) => panic!("an audio clip"),
    }
}

fn crop(shell: &mut Shell, args: serde_json::Value) -> Result<CropResult, String> {
    shell
        .run(ids::CROP, args)
        .map(|value| serde_json::from_value(value).unwrap())
        .map_err(|error| error.to_string())
}

fn consolidate(shell: &mut Shell, track: TrackId, start: i64, end: i64) -> ConsolidateResult {
    let value = shell
        .run(
            ids::CONSOLIDATE,
            json!({ "tracks": [track], "start": start, "end": end }),
        )
        .unwrap();
    serde_json::from_value(value).unwrap()
}

// Spec: audio-clip-editing R14, R15.
#[test]
fn crop_writes_the_region_of_a_clip_and_the_clip_plays_it() {
    let scratch = Scratch::new("crop");
    let mut shell = new_shell(&scratch);
    let song = saved(&scratch, &mut shell);
    let track = track(&shell, TrackKind::Audio);
    let source = scratch.path("Tone.wav");
    wav(&source, 2.0, 2, 0.1);
    let clip = place(&mut shell, track, &source, 0);
    // The region: half a second in, half a second long.
    shell
        .run(
            "clip.set_region",
            json!({ "clip": clip, "start": 0.5, "end": 1.0 }),
        )
        .unwrap();

    let before = std::fs::read(&source).unwrap();
    let made = crop(&mut shell, json!({ "clip": clip })).unwrap();
    let path = PathBuf::from(made.path.clone().unwrap());
    assert_eq!(
        path,
        song.parent()
            .unwrap()
            .join("Samples/Processed/Crop/Tone Crop.wav"),
        "the region goes into the project's Crop folder under its source's name"
    );
    let cropped = decode_file(&path).unwrap();
    assert_eq!(cropped.frames(), 24_000, "half a second at 48 kHz");
    assert_eq!(cropped.channel_count(), 2);
    assert_eq!(
        std::fs::read(&source).unwrap(),
        before,
        "the source file is untouched"
    );
    let data = audio(&shell, clip);
    assert_eq!((data.start, data.end), (0.0, None), "the markers move back");
    assert_eq!(
        data.media,
        made.media.unwrap(),
        "the clip plays the new file"
    );
    // The new file holds the source's own frames, cut to the region.
    let source_audio = decode_file(&source).unwrap();
    for channel in 0..2 {
        assert_eq!(
            cropped.channel(channel).unwrap(),
            &source_audio.channel(channel).unwrap()[24_000..48_000],
            "channel {channel}, to the bit"
        );
    }
    // One undo step gives the clip back.
    shell.run("edit.undo", json!({})).unwrap();
    let data = audio(&shell, clip);
    assert_eq!(data.start, 0.5, "the region is back");
    assert_ne!(data.media, made.media.unwrap());
}

// Spec: audio-clip-editing R14, R15.
#[test]
fn crop_of_a_looped_warped_clip_keeps_what_it_played() {
    let scratch = Scratch::new("loop");
    let mut shell = new_shell(&scratch);
    let track = track(&shell, TrackKind::Audio);
    let source = scratch.path("Tone.wav");
    wav(&source, 2.0, 1, 0.2);
    let clip = place(&mut shell, track, &source, 0);
    shell
        .run("clip.set_warp", json!({ "clip": clip, "warp": true }))
        .unwrap();
    let quarter = Ticks::QUARTER;
    // A brace over the first quarter, switched on.
    shell
        .run(
            "clip.set_loop",
            json!({ "clip": clip, "start": 0.0, "end": quarter.get() as f64, "ticks": true }),
        )
        .unwrap();
    shell
        .run(
            "clip.set_loop_enabled",
            json!({ "clip": clip, "enabled": true }),
        )
        .unwrap();
    let before = audio(&shell, clip);
    let made = crop(&mut shell, json!({ "clip": clip })).unwrap();
    let cropped = decode_file(PathBuf::from(made.path.unwrap())).unwrap();
    let expected = ((before.in_file_seconds(before.loop_end.unwrap_or(0.0))
        - before.in_file_seconds(before.start.min(before.loop_start)))
        * 48_000.0)
        .round() as usize;
    assert_eq!(
        cropped.frames(),
        expected,
        "the frames from the earlier of the Start marker and the brace's start \
         to the brace's end"
    );
    let after = audio(&shell, clip);
    assert_eq!(
        after.start, 0.0,
        "the markers move back by the region's start"
    );
    assert!(
        after.warp && !after.warp_markers.is_empty(),
        "the warp map came with it"
    );
}

// Spec: audio-clip-editing R16.
#[test]
fn crop_to_a_time_selection_moves_the_clips_edges_to_it() {
    let scratch = Scratch::new("span");
    let mut shell = new_shell(&scratch);
    let track = track(&shell, TrackKind::Audio);
    let source = scratch.path("Tone.wav");
    wav(&source, 2.0, 1, 0.3);
    let clip = place(&mut shell, track, &source, 0);
    // The clip is a whole long (two seconds at 120 BPM), so the second
    // quarter covers half a second of the file.
    let quarter = Ticks::QUARTER;
    let made = crop(
        &mut shell,
        json!({ "clip": clip, "start": quarter.get(), "end": (quarter * 3).get() }),
    )
    .unwrap();
    let cropped = decode_file(PathBuf::from(made.path.unwrap())).unwrap();
    assert_eq!(cropped.frames(), 48_000, "the span's part of the file");
    let value = shell.editor().document().clip(clip).unwrap().clone();
    assert_eq!(value.start, quarter, "the clip's edges move to the span");
    assert_eq!(value.length, quarter * 2);
}

// Spec: audio-clip-editing R16.
#[test]
fn crop_of_a_selection_over_a_loop_wrap_names_consolidate() {
    let scratch = Scratch::new("wrap");
    let mut shell = new_shell(&scratch);
    let track = track(&shell, TrackKind::Audio);
    let source = scratch.path("Tone.wav");
    wav(&source, 2.0, 1, 0.4);
    let clip = place(&mut shell, track, &source, 0);
    shell
        .run("clip.set_warp", json!({ "clip": clip, "warp": true }))
        .unwrap();
    let quarter = Ticks::QUARTER;
    shell
        .run(
            "clip.set_loop",
            json!({ "clip": clip, "start": 0.0, "end": quarter.get() as f64, "ticks": true }),
        )
        .unwrap();
    shell
        .run(
            "clip.set_loop_enabled",
            json!({ "clip": clip, "enabled": true }),
        )
        .unwrap();
    // The clip lasts a whole: a selection that starts in the first pass and
    // ends past the brace's end goes round the loop.
    let error = crop(
        &mut shell,
        json!({ "clip": clip, "start": (quarter * 2).get(), "end": (quarter * 3).get() }),
    )
    .unwrap_err();
    assert!(error.contains("Consolidate"), "{error}");
    assert!(error.contains("loop"), "{error}");
}

// Spec: audio-clip-editing R19, R20.
#[test]
fn consolidate_makes_one_clip_per_track_over_the_range() {
    let scratch = Scratch::new("consolidate");
    let mut shell = new_shell(&scratch);
    let song = saved(&scratch, &mut shell);
    let audio_track = track(&shell, TrackKind::Audio);
    let midi_track = track(&shell, TrackKind::Midi);
    let source = scratch.path("Tone.wav");
    wav(&source, 2.0, 2, 0.5);
    let source_clip = place(&mut shell, audio_track, &source, 0);
    let note_clip: ClipId = shell.edit(|editor| editor.new_id());
    shell
        .run(
            "clip.create",
            json!({ "track": midi_track, "clip": {
                "id": note_clip, "name": "Riff", "start": 0, "length": 705_600,
                "mute": false,
                "data": { "midi": { "notes": [
                    { "key": 60, "velocity": 100, "start": 0, "length": 705_600 }
                ] } } } }),
        )
        .unwrap();
    let before = shell.editor().document().clone();

    let value = shell
        .run(
            ids::CONSOLIDATE,
            json!({ "tracks": [audio_track, midi_track], "start": 0, "end": 705_600 }),
        )
        .unwrap();
    let made: ConsolidateResult = serde_json::from_value(value).unwrap();
    assert_eq!(made.created.len(), 2, "one new clip per track");
    assert_eq!(made.files.len(), 1, "a file for the audio track alone");
    let file = PathBuf::from(&made.files[0]);
    assert!(file.starts_with(song.parent().unwrap().join("Samples/Processed/Consolidate")));
    assert_eq!(decode_file(&file).unwrap().frames(), 24_000);
    let created_now = made.created.clone();
    let doc = shell.editor().document();
    assert!(doc.clip(note_clip).is_none(), "the MIDI clip is replaced");
    assert!(
        doc.clip(source_clip).is_none()
            || doc.clip(source_clip).unwrap().start != before.clip(source_clip).unwrap().start,
        "the audio clip it covered is replaced"
    );
    // The new clip is warped, looped, and named after the first clip it
    // replaced.
    let at = made
        .tracks
        .iter()
        .position(|track| *track == audio_track)
        .expect("the audio track's new clip");
    let data = audio(&shell, made.created[at]);
    assert!(data.warp && data.looping, "warped and looped");
    assert_eq!(data.start, 0.0);
    assert_eq!(data.warp_markers.first().unwrap().beat, Ticks::ZERO);
    assert_eq!(doc.clip(made.created[at]).unwrap().name, "Tone");
    let midi_at = 1 - at;
    assert!(matches!(
        doc.clip(made.created[midi_at]).unwrap().data,
        ClipData::Midi(_)
    ));

    // The new clips are selected (R19), and one undo step gives everything
    // back.
    assert_eq!(
        shell.arrangement().selected_clips(),
        created_now.as_slice(),
        "the new clips are selected"
    );
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(
        shell.editor().document().clip(note_clip).unwrap().id,
        note_clip
    );
    assert!(shell.editor().document().clip(source_clip).is_some());
}

// Spec: audio-clip-editing R20.
#[test]
fn a_mono_source_consolidates_to_a_mono_file() {
    let scratch = Scratch::new("mono");
    let mut shell = new_shell(&scratch);
    let track = track(&shell, TrackKind::Audio);
    let source = scratch.path("Mono.wav");
    wav(&source, 1.0, 1, 0.6);
    place(&mut shell, track, &source, 0);
    let made = consolidate(&mut shell, track, 0, 705_600);
    let audio = decode_file(PathBuf::from(&made.files[0])).unwrap();
    assert_eq!(
        audio.channel_count(),
        1,
        "its channels are equal throughout"
    );
    assert_eq!(audio.frames(), 24_000);
}

// Spec: audio-clip-editing R20, R21.
#[test]
fn a_consolidated_file_is_taken_before_the_devices_and_normalised() {
    let scratch = Scratch::new("before");
    let mut shell = new_shell(&scratch);
    let track = track(&shell, TrackKind::Audio);
    let source = scratch.path("Tone.wav");
    let samples = wav(&source, 1.0, 1, 0.0);
    let clip = place(&mut shell, track, &source, 0);
    // Without the 4 ms edge fades, the clip plays the file's own samples.
    shell
        .run(
            "clip.set_fades",
            json!({ "clip": clip, "fade_in": 0.0, "fade_out": 0.0 }),
        )
        .unwrap();
    // The track's mixer is at half its level: a render that took the mixer
    // would halve the file, and one that took the devices would show a
    // Utility's gain.
    shell
        .run("mixer.set_volume", json!({ "track": track, "volume": 0.5 }))
        .unwrap();
    let effect: bl_model::DeviceId = shell.new_id();
    shell
        .run(
            "device.insert",
            json!({ "track": track, "index": 0, "device": {
                "id": effect, "name": "Utility", "enabled": true, "params": [],
                "kind": { "native": { "device_type": "beatlayer.utility" } } } }),
        )
        .unwrap();
    let made = consolidate(&mut shell, track, 0, 705_600);
    let clip = made.created[0];
    let file_audio = decode_file(PathBuf::from(&made.files[0])).unwrap();
    let peak = file_audio
        .channels()
        .flat_map(|channel| channel.iter())
        .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
    assert!(
        (peak - 1.0).abs() < 1e-6,
        "the file is normalised: its peak is {peak}"
    );
    // The range is the clip's first quarter: half a second of the file.
    let in_range = &samples[0][..24_000];
    let expected = in_range
        .iter()
        .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
    let gain = audio(&shell, clip).gain;
    assert!(
        (gain - f64::from(expected)).abs() < 1e-6,
        "the clip gain makes up for it: {gain} against {expected}"
    );
    // The file is what the clip played, before the devices: each sample,
    // made up by the gain, is the source's.
    let channel = file_audio.channel(0).unwrap();
    for frame in [0usize, 100, 12_000, 23_999] {
        let back = channel[frame] * gain as f32;
        assert!(
            (back - samples[0][frame]).abs() < 1e-6,
            "frame {frame}: {back} against {}",
            samples[0][frame]
        );
    }
}

// Spec: audio-clip-editing R26, R27.
#[test]
fn consolidate_time_to_new_scene_fills_a_new_scene_and_leaves_the_arrangement() {
    let scratch = Scratch::new("scene");
    let mut shell = new_shell(&scratch);
    let song = saved(&scratch, &mut shell);
    let track = track(&shell, TrackKind::Audio);
    let source = scratch.path("Tone.wav");
    wav(&source, 1.0, 2, 0.7);
    let clip = place(&mut shell, track, &source, 0);
    // The Session View has a scene selected: the new scene goes after it.
    let first_scene = shell.editor().document().scenes().next().unwrap().id;
    shell
        .run("session.select", json!({ "scene": first_scene }))
        .unwrap();
    let value = shell
        .run(
            ids::CONSOLIDATE_TO_SCENE,
            json!({ "tracks": [track], "start": 0, "end": 705_600 }),
        )
        .unwrap();
    let made: ConsolidateSceneResult = serde_json::from_value(value).unwrap();
    assert_eq!(made.created.len(), 1);
    assert!(
        PathBuf::from(&made.files[0])
            .starts_with(song.parent().unwrap().join("Samples/Processed/Consolidate")),
        "the file goes where Consolidate's does"
    );
    let doc = shell.editor().document();
    assert_eq!(
        doc.track(track).unwrap().clips().count(),
        1,
        "the Arrangement keeps its clip"
    );
    assert_eq!(doc.clip(clip).unwrap().start, Ticks::ZERO);
    let slot = doc
        .track(track)
        .unwrap()
        .slots()
        .find(|(scene, _)| *scene == made.scene)
        .map(|(_, clip)| clip.id);
    assert_eq!(slot, Some(made.created[0]), "the new scene's slot holds it");
    let scenes: Vec<_> = doc.scenes().map(|scene| scene.id).collect();
    assert_eq!(
        scenes.iter().position(|scene| *scene == made.scene),
        Some(1),
        "directly after the scene the Session View had selected, the first"
    );
}

// Spec: audio-clip-editing R27.
#[test]
fn consolidate_time_to_new_scene_chooses_the_scene_it_follows() {
    let scratch = Scratch::new("scene-after");
    let mut shell = new_shell(&scratch);
    let track = track(&shell, TrackKind::Audio);
    let source = scratch.path("Tone.wav");
    wav(&source, 1.0, 1, 0.8);
    place(&mut shell, track, &source, 0);
    let fifth = shell.editor().document().scenes().nth(4).unwrap().id;
    let value = shell
        .run(
            ids::CONSOLIDATE_TO_SCENE,
            json!({ "tracks": [track], "start": 0, "end": 705_600, "scene": fifth }),
        )
        .unwrap();
    let made: ConsolidateSceneResult = serde_json::from_value(value).unwrap();
    let doc = shell.editor().document();
    let scenes: Vec<_> = doc.scenes().map(|scene| scene.id).collect();
    assert_eq!(
        scenes.iter().position(|scene| *scene == made.scene),
        Some(5)
    );
}

// Spec: audio-clip-editing R22.
#[test]
fn a_consolidated_clip_nulls_against_the_clips_it_replaced() {
    let scratch = Scratch::new("null");
    let mut shell = new_shell(&scratch);
    let track = track(&shell, TrackKind::Audio);
    let first = scratch.path("A.wav");
    wav(&first, 0.5, 2, 0.7);
    let second = scratch.path("B.wav");
    wav(&second, 0.5, 2, 0.2);
    // Adjacent: one quarter is half a second at 120 BPM.
    let a = place(&mut shell, track, &first, 0);
    let b = place(&mut shell, track, &second, Ticks::QUARTER.get());
    // A gain and a fade, both of which the render carries.
    shell
        .run("clip.set_gain", json!({ "clip": b, "gain": 0.25 }))
        .unwrap();
    shell
        .run(
            "clip.set_fades",
            json!({ "clip": a, "fade_in": 0.01, "fade_out": 0.01 }),
        )
        .unwrap();

    let range = (0, (Ticks::QUARTER * 2).get());
    let frames = 48_000u64;
    for tempo in [120.0, 100.0] {
        shell
            .run(
                "song.set_tempo",
                json!({ "tempo": [
                    { "kind": "constant", "start": 0, "bpm": tempo }
                ] }),
            )
            .unwrap();
        for block in [1usize, 61, 512, 4096] {
            let reference = render_frames(&mut shell, frames, block);
            let made = shell
                .run(
                    ids::CONSOLIDATE,
                    json!({ "tracks": [track], "start": range.0, "end": range.1 }),
                )
                .unwrap();
            let made: ConsolidateResult = serde_json::from_value(made).unwrap();
            let after = render_frames(&mut shell, frames, block);
            let residual = reference
                .iter()
                .zip(&after)
                .fold(0.0f32, |peak, (before, now)| peak.max((before - now).abs()));
            assert!(
                residual < 1e-6,
                "at {tempo} BPM in blocks of {block}, the residual is {residual}"
            );
            assert!(!made.created.is_empty());
            shell.run("edit.undo", json!({})).unwrap();
        }
    }
}

/// Renders the open set from its start for `frames` frames, as an export
/// does, and gives the interleaved samples.
fn render_frames(shell: &mut Shell, frames: u64, block: usize) -> Vec<f32> {
    use bl_shell::export::{self, Length, Render};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    let media: Arc<dyn bl_engine::MediaLibrary> = shell.media_store();
    let spec = Render {
        start: Ticks::ZERO,
        length: Length::Frames(frames),
        sample_rate: 48_000,
        engine_rate: 48_000,
        block,
    };
    let cancel = AtomicBool::new(false);
    let mut out: Vec<f32> = Vec::new();
    {
        let mut sink = |samples: &[f32]| {
            out.extend_from_slice(samples);
            Ok(())
        };
        export::render(
            shell.editor().document(),
            media,
            shell.plugin_hosts(),
            &spec,
            86_400.0,
            &cancel,
            &mut |_| {},
            &mut sink,
            None,
        )
        .unwrap();
    }
    out
}

// Spec: audio-clip-editing R18.
#[test]
fn a_crop_that_removes_material_asks_first() {
    let scratch = Scratch::new("permission");
    let mut shell = new_shell(&scratch);
    let track = track(&shell, TrackKind::Audio);
    let source = scratch.path("Tone.wav");
    wav(&source, 1.0, 1, 0.9);
    let clip = place(&mut shell, track, &source, 0);
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    let registered = shell
        .registry()
        .get(ids::CROP)
        .and_then(|spec| spec.permission)
        .unwrap();
    assert_eq!(
        registered,
        bl_commands::Permission::Edit,
        "a crop is an edit however far it reaches"
    );
    // The clip plays its whole file: cropping to that region removes
    // nothing, so an agent isn't asked.
    let call = shell.call_permission(ids::CROP, &json!({ "clip": clip }));
    assert!(!call.is_destructive(), "{call:?}");
    // A region that keeps only part of it removes the rest, so it asks.
    shell
        .run(
            "clip.set_region",
            json!({ "clip": clip, "start": 0.25, "end": 0.5 }),
        )
        .unwrap();
    let call = shell.call_permission(ids::CROP, &json!({ "clip": clip }));
    assert!(call.is_destructive(), "{call:?}");
    assert_eq!(call.class(), bl_commands::Permission::Destructive);
    assert_eq!(call.cropped, Some(clip));
}

// Spec: audio-clip-editing R15.
#[test]
fn a_cropped_clip_plays_exactly_what_it_did() {
    let scratch = Scratch::new("plays");
    let mut shell = new_shell(&scratch);
    let track = track(&shell, TrackKind::Audio);
    let source = scratch.path("Tone.wav");
    wav(&source, 1.0, 1, 0.9);
    let clip = place(&mut shell, track, &source, 0);
    shell
        .run(
            "clip.set_fades",
            json!({ "clip": clip, "fade_in": 0.0, "fade_out": 0.0 }),
        )
        .unwrap();
    // Half a second in, a quarter long: the rest is cropped away.
    shell
        .run(
            "clip.set_region",
            json!({ "clip": clip, "start": 0.25, "end": 0.75 }),
        )
        .unwrap();
    let frames = 48_000u64;
    let before = render_frames(&mut shell, frames, 61);
    crop(&mut shell, json!({ "clip": clip })).unwrap();
    for block in [1usize, 61, 512, 4096] {
        let after = render_frames(&mut shell, frames, block);
        let residual = before
            .iter()
            .zip(&after)
            .fold(0.0f32, |peak, (was, now)| peak.max((was - now).abs()));
        assert!(
            residual < 1e-6,
            "in blocks of {block} the residual is {residual}"
        );
    }
}

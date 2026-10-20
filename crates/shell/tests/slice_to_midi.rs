//! Slice to New MIDI Track through the shell (WP-4.16; the slice-to-midi
//! spec, R3 to R12). The clip's file is the generated drum loop
//! (`tests/fixtures/gen/gen_drums.py`), whose nine hits and their times the
//! transient tests share.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use bl_core::time::Ticks;
use bl_model::{
    Chain, ClipData, ClipId, ContentHash, Device, DeviceKind, RackKind, Receive, Track, TrackId,
    TrackKind,
};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::slice_commands::{self as ids, SliceResult};
use serde_json::json;

/// The User Library's place is read from `HOME`, so tests that use one take
/// this lock.
static HOME: Mutex<()> = Mutex::new(());

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-slice-{name}-{}-{}",
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

/// The generated drum loop, copied into the scratch folder.
fn loop_path(scratch: &Scratch) -> PathBuf {
    let path = scratch.path("drum-loop.wav");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/drums/drum-loop.wav"),
        &path,
    )
    .unwrap();
    path
}

/// The clip's transients, as times in its file, from `clip.transients`.
fn transients(shell: &mut Shell, clip: ClipId) -> Vec<f64> {
    let result = shell
        .run("clip.transients", json!({ "clip": clip }))
        .unwrap();
    result["transients"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["time"].as_f64().unwrap())
        .collect()
}

fn new_shell(scratch: &Scratch) -> Shell {
    Shell::new(
        Preferences::default(),
        Some(scratch.path("config")),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    )
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

/// A shell with the drum loop on its first audio track, unwarped, and its
/// analysis done: the shell, the track and the clip.
fn loop_clip(scratch: &Scratch) -> (Shell, TrackId, ClipId) {
    let mut shell = new_shell(scratch);
    let track = track(&shell, TrackKind::Audio);
    let path = loop_path(scratch);
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
    let media = audio(&shell, clip).media;
    shell.wait_analysis(media).expect("the file is analysed");
    (shell, track, clip)
}

fn audio(shell: &Shell, clip: ClipId) -> bl_model::AudioData {
    match &shell.editor().document().clip(clip).unwrap().data {
        ClipData::Audio(audio) => audio.clone(),
        ClipData::Midi(_) => unreachable!("an audio clip"),
    }
}

fn slice(shell: &mut Shell, clip: ClipId, args: serde_json::Value) -> SliceResult {
    let mut full = json!({ "clip": clip });
    if let Some(fields) = args.as_object() {
        for (key, value) in fields {
            full[key] = value.clone();
        }
    }
    serde_json::from_value(shell.run(ids::SLICE, full).unwrap()).unwrap()
}

/// The new track's device.
fn device(shell: &Shell, track: TrackId) -> Device {
    shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .devices()
        .next()
        .unwrap()
        .clone()
}

fn sampler<'a>(device: &'a Device, chain: &'a Chain) -> &'a Device {
    chain
        .devices
        .iter()
        .find(|device| {
            matches!(&device.kind, DeviceKind::Native { device_type } if device_type == "beatlayer.snapshot")
        })
        .unwrap_or_else(|| panic!("no sampler in {}", device.name))
}

/// The frames a sampler's Start and Length shares land on, by the sampler's
/// own arithmetic (sampler R3).
fn region(sampler: &Device, file_frames: u64) -> (u64, u64) {
    let value = |id: u32| {
        sampler
            .params
            .get(&bl_model::ParamId(id))
            .copied()
            .unwrap_or(0.0)
    };
    let start = value(1) / 100.0;
    let length = value(2) / 100.0;
    let frames = file_frames as f64;
    (
        (start * frames) as u64,
        ((start + length).min(1.0) * frames) as u64,
    )
}

/// The media a device plays.
fn media(sampler: &Device) -> ContentHash {
    sampler.media_at(0).expect("the sampler plays a sample")
}

fn file_frames(shell: &Shell, media: ContentHash) -> u64 {
    shell
        .media_store()
        .load_at_rate(bl_media::ContentHash::from_bytes(media.0), 48_000)
        .unwrap()
        .frames() as u64
}

// Spec: slice-to-midi R4. The region's start and each of the loop's
// transients inside it become a slice, and a quarter-note division gives
// one every half second.
#[test]
fn slices_start_at_each_transient_warp_marker_or_division() {
    let scratch = Scratch::new("transients");
    let (mut shell, track, clip) = loop_clip(&scratch);
    let listed = transients(&mut shell, clip);
    assert_eq!(listed.len(), 9, "the loop has nine hits");
    // R4: the region's start is a slice of its own, then each transient
    // inside the region.
    let expected = 1 + listed.len();

    let by_transients = slice(&mut shell, clip, json!({ "by": "transients" }));
    assert_eq!(by_transients.slices, expected);
    let order = shell.editor().document().track_order();
    let at = order.iter().position(|other| *other == track).unwrap();
    assert_eq!(
        order[at + 1],
        by_transients.track,
        "directly after its track"
    );

    // One note per slice, rising from C1, each lasting to the next (R8, R9).
    let midi = shell
        .editor()
        .document()
        .clip(by_transients.clip)
        .unwrap()
        .data
        .clone();
    let ClipData::Midi(data) = midi else {
        panic!("the new clip is a MIDI clip")
    };
    assert_eq!(data.notes.len(), expected);
    for (at, note) in data.notes.iter().enumerate() {
        assert_eq!(note.key, 36 + u8::try_from(at).unwrap());
        assert_eq!(note.velocity, 127);
    }
    // The first slice is the region's start, at 0; the second is the first
    // hit, at 0.25 s.
    assert_eq!(data.notes[0].start, Ticks::ZERO);
    let seconds_to_ticks =
        |seconds: f64| Ticks((seconds * 120.0 / 60.0 * Ticks::QUARTER.get() as f64).round() as i64);
    let first = data.notes[1].start.get() - seconds_to_ticks(listed[0]).get();
    assert!(
        first.abs() <= 2,
        "the second note is at the first hit: {first}"
    );
    let second = data.notes[2].start.get() - seconds_to_ticks(listed[1]).get();
    assert!(second.abs() <= 2, "the third note is at the second hit");

    // A division counts from the region's start in the set's tempo.
    let quarter = slice(&mut shell, clip, json!({ "by": "quarter" }));
    assert_eq!(quarter.slices, 4);
}

// Spec: slice-to-midi R3. A looped clip is sliced over its loop brace.
#[test]
fn a_looped_clip_is_sliced_over_its_loop() {
    let scratch = Scratch::new("loop");
    let (mut shell, _track, clip) = loop_clip(&scratch);
    // The second half of the file, looped. Only a warped clip loops.
    shell
        .run("clip.set_warp", json!({ "clip": clip, "warp": true }))
        .unwrap();
    let quarter = Ticks::QUARTER.get();
    shell
        .run(
            "clip.set_region",
            json!({ "clip": clip, "start": 2 * quarter, "end": 4 * quarter, "ticks": true }),
        )
        .unwrap();
    shell
        .run(
            "clip.set_loop",
            json!({ "clip": clip, "start": 2 * quarter, "end": 4 * quarter, "ticks": true }),
        )
        .unwrap();
    shell
        .run(
            "clip.set_loop_enabled",
            json!({ "clip": clip, "enabled": true }),
        )
        .unwrap();
    // Preserve off cuts the clip's own file: one second of the loop, in
    // half-second divisions, gives two slices.
    let cut = slice(
        &mut shell,
        clip,
        json!({ "by": "quarter", "preserve_warped_timing": false }),
    );
    assert_eq!(cut.slices, 2);
}

// Spec: slice-to-midi R5.
#[test]
fn more_than_128_slices_or_64_for_one_sampler_is_refused() {
    let scratch = Scratch::new("limit");
    // Eight seconds of a steady tone at 120 BPM: a 1/32 division gives
    // exactly 128 slices, the most a slicing makes.
    let (mut shell, clip) = long_clip(&scratch, 8.0);
    let cut = slice(&mut shell, clip, json!({ "by": "thirty_second" }));
    assert_eq!(cut.slices, 128);
    // One more second gives 144, which is refused.
    let (mut shell, clip) = long_clip(&scratch, 10.0);
    let refused = shell.run(ids::SLICE, json!({ "clip": clip, "by": "thirty_second" }));
    assert!(refused.is_err(), "129 slices are refused");
    // Six seconds give 96: a Drum Rack takes them, the one-sampler preset,
    // whose limit is 64, doesn't.
    let (mut shell, clip) = long_clip(&scratch, 6.0);
    let cut = slice(&mut shell, clip, json!({ "by": "thirty_second" }));
    assert_eq!(cut.slices, 96);
    let refused = shell.run(
        ids::SLICE,
        json!({ "clip": clip, "by": "thirty_second", "preset": "One Sampler" }),
    );
    assert!(refused.is_err(), "96 slices are refused for One Sampler");
}

/// A shell with a `seconds`-long steady tone on its first audio track: the
/// shell and the clip.
fn long_clip(scratch: &Scratch, seconds: f64) -> (Shell, ClipId) {
    let mut shell = new_shell(scratch);
    let track = track(&shell, TrackKind::Audio);
    let path = scratch.path(&format!("tone-{seconds}.wav"));
    let frames = (seconds * 48_000.0) as usize;
    let samples: Vec<f32> = (0..frames)
        .map(|i| (i as f32 * 220.0 / 48_000.0 * std::f32::consts::TAU).sin() * 0.2)
        .collect();
    let bytes = bl_media::wav_bytes(
        &samples,
        1,
        48_000,
        bl_media::BitDepth::Float32,
        bl_media::Dither::None,
    )
    .unwrap();
    std::fs::write(&path, bytes).unwrap();
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
    (shell, clip)
}

// Spec: slice-to-midi R6, R12. Preserve warped timing cuts the clip's
// warped audio into a new file; off, the clip's own file. Either way the
// audio clip is left as it was.
#[test]
fn preserve_warped_timing_slices_the_warped_audio_and_off_the_file() {
    let scratch = Scratch::new("preserve");
    let (mut shell, _track, clip) = loop_clip(&scratch);
    shell
        .run("clip.set_warp", json!({ "clip": clip, "warp": true }))
        .unwrap();
    let before = shell.editor().document().clip(clip).unwrap().clone();

    let off = slice(
        &mut shell,
        clip,
        json!({ "by": "quarter", "preserve_warped_timing": false }),
    );
    let rack = device(&shell, off.track);
    assert_eq!(rack.rack_kind(), Some(RackKind::Drum));
    for chain in &rack.chains {
        assert_eq!(media(sampler(&rack, chain)), before_media(&before));
    }

    let on = slice(&mut shell, clip, json!({ "by": "quarter" }));
    let rack = device(&shell, on.track);
    let played = media(sampler(&rack, &rack.chains[0]));
    assert_ne!(
        played,
        before_media(&before),
        "the warped audio is a new file"
    );

    // R12: the audio clip and its track are as they were.
    assert_eq!(shell.editor().document().clip(clip).unwrap(), &before);
}

fn before_media(clip: &bl_model::Clip) -> ContentHash {
    match &clip.data {
        ClipData::Audio(audio) => audio.media,
        ClipData::Midi(_) => unreachable!("an audio clip"),
    }
}

// Spec: slice-to-midi R7, R8. The Drum Rack holds one chain per slice, each
// a sampler in One-Shot mode whose region lands on the slice's frames, and
// its chain receives its slice's note from C1 up and plays C3.
#[test]
fn a_drum_rack_of_one_shot_samplers_plays_each_slice_to_the_frame() {
    let scratch = Scratch::new("rack");
    let (mut shell, _track, clip) = loop_clip(&scratch);
    let cut = slice(&mut shell, clip, json!({ "by": "transients" }));
    let rack = device(&shell, cut.track);
    assert_eq!(rack.rack_kind(), Some(RackKind::Drum));
    assert_eq!(rack.chains.len(), cut.slices);

    let media = before_media(shell.editor().document().clip(clip).unwrap());
    let frames = file_frames(&shell, media);
    // R4: the region's start, then each transient inside it.
    let expected: Vec<u64> = std::iter::once(0.0)
        .chain(transients(&mut shell, clip))
        .map(|seconds| (seconds * 48_000.0).round() as u64)
        .collect();
    assert_eq!(expected.len(), cut.slices);
    for (at, chain) in rack.chains.iter().enumerate() {
        assert_eq!(chain.receive, Receive::Note(36 + u8::try_from(at).unwrap()));
        assert_eq!(chain.play, 60);
        let sampler = sampler(&rack, chain);
        let mode = sampler
            .params
            .get(&bl_model::ParamId(0))
            .copied()
            .unwrap_or(0.0);
        assert_eq!(mode, 1.0, "One-Shot");
        let (start, end) = region(sampler, frames);
        assert_eq!(start, expected[at], "slice {at} starts on its frame");
        let next = expected.get(at + 1).copied().unwrap_or(frames);
        assert_eq!(end, next, "slice {at} ends where the next starts");
    }
}

// Spec: slice-to-midi R12. Slicing is one undo step.
#[test]
fn slicing_leaves_the_audio_clip_as_it_was() {
    let scratch = Scratch::new("undo");
    let (mut shell, _track, clip) = loop_clip(&scratch);
    let before = shell.editor().document().clone();
    let cut = slice(&mut shell, clip, json!({ "by": "quarter" }));
    assert!(shell.editor().document().track(cut.track).is_some());
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(
        shell.editor().document().tracks().count(),
        before.tracks().count()
    );
    assert_eq!(
        shell.editor().document().clip(clip).unwrap().data,
        before.clip(clip).unwrap().data
    );
}

// Spec: slice-to-midi R11. A user preset in the User Library's
// Defaults/Slicing folder is offered and used.
#[test]
fn a_user_slicing_preset_shapes_every_chain() {
    let _home = HOME.lock().unwrap();
    let scratch = Scratch::new("preset");
    let (mut shell, _track, clip) = loop_clip(&scratch);
    let library = scratch.path("Music/Beatlayer/User Library/Defaults/Slicing");
    std::fs::create_dir_all(&library).unwrap();
    let _guard = HomeGuard::set(&scratch.0);

    // A rack preset saved from a slicing becomes a user preset.
    let first = slice(&mut shell, clip, json!({ "by": "quarter" }));
    let rack = device(&shell, first.track);
    let path = library.join("Pattern Rack.blpreset");
    shell
        .run(
            "device.save_preset",
            json!({ "device": rack.id, "path": path.display().to_string() }),
        )
        .unwrap();
    assert!(path.is_file());

    // The chooser offers it after the factory presets.
    let status = shell.run(ids::OPEN, json!({ "clip": clip })).unwrap();
    let presets: Vec<String> = serde_json::from_value(status["presets"].clone()).unwrap();
    assert_eq!(presets.first().unwrap(), "Drum Rack");
    assert!(presets.contains(&"Pattern Rack".to_owned()), "{presets:?}");

    // And using it shapes every chain from the preset's first chain.
    let cut = slice(
        &mut shell,
        clip,
        json!({ "by": "quarter", "preset": "Pattern Rack" }),
    );
    let rack = device(&shell, cut.track);
    assert_eq!(rack.chains.len(), cut.slices);
    for chain in &rack.chains {
        assert_eq!(chain.devices.len(), 1, "the pattern's chain is copied");
        let mode = sampler(&rack, chain)
            .params
            .get(&bl_model::ParamId(0))
            .copied()
            .unwrap_or(0.0);
        assert_eq!(mode, 1.0, "One-Shot");
    }
}

/// Sets `HOME` for the test and puts it back.
struct HomeGuard(Option<std::ffi::OsString>);

impl HomeGuard {
    fn set(home: &Path) -> Self {
        let before = std::env::var_os("HOME");
        // SAFETY: the test holds `HOME`, the lock every test in this file
        // that reads the user library takes, so no other thread runs
        // meanwhile.
        unsafe { std::env::set_var("HOME", home) };
        Self(before)
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match self.0.take() {
            // SAFETY: as in `set`: the test still holds `HOME`.
            Some(home) => unsafe { std::env::set_var("HOME", home) },
            // SAFETY: as in `set`: the test still holds `HOME`.
            None => unsafe { std::env::remove_var("HOME") },
        }
    }
}

// Spec: slice-to-midi R9, R10. The new track plays the loop back where the
// audio clip played it, for the Drum Rack and the one-sampler preset.
#[test]
fn the_sliced_track_plays_the_loop_back_within_a_millisecond() {
    for preset in ["Drum Rack", "One Sampler"] {
        let scratch = Scratch::new("golden");
        let (mut shell, track, clip) = loop_clip(&scratch);
        // The source's own edge fades go: a slicing cuts the clip's audio
        // without them (R6), so the two renders are compared without them.
        shell
            .run(
                "clip.set_fades",
                json!({ "clip": clip, "fade_in": 0.0, "fade_out": 0.0 }),
            )
            .unwrap();
        let length = Ticks::WHOLE.get() * 4;
        let source = render_track(&mut shell, &scratch, track, "source.wav", length);
        let cut = slice(
            &mut shell,
            clip,
            json!({ "by": "transients", "preset": preset }),
        );
        assert!(cut.slices > 0);
        let sliced = render_track(&mut shell, &scratch, cut.track, "sliced.wav", length);
        assert_eq!(source.len(), sliced.len());
        // Each sample is where the clip played it, within a sample: the
        // slice's frame is exact and its note is rounded to the tick, which
        // at 120 BPM is a thirtieth of a sample.
        let mut worst = 0.0f64;
        for (i, &got) in sliced.iter().enumerate() {
            let best = [i.saturating_sub(1), i, i + 1]
                .into_iter()
                .filter_map(|at| source.get(at).copied())
                .map(|want| f64::from((got - want).abs()))
                .fold(f64::INFINITY, f64::min);
            worst = worst.max(best);
        }
        assert!(worst < 1e-3, "{preset}: the loop back, off by {worst}");
    }
}

/// Renders `track` over `length` ticks as 32-bit float WAV and decodes it.
fn render_track(
    shell: &mut Shell,
    scratch: &Scratch,
    track: TrackId,
    name: &str,
    length: i64,
) -> Vec<f32> {
    let path = scratch.path(name);
    shell
        .run(
            "export.audio",
            json!({
                "path": path.display().to_string(),
                "bit_depth": 32,
                "sample_rate": 48_000,
                "length": length,
                "rendered_track": { "track": track },
                "replace": true,
            }),
        )
        .unwrap();
    let file = bl_media::decode_file(&path).unwrap();
    let mut out = Vec::with_capacity(file.frames() * 2);
    for frame in 0..file.frames() {
        for channel in 0..2 {
            let c = channel.min(file.channel_count() - 1);
            out.push(
                file.channel(c)
                    .and_then(|samples| samples.get(frame))
                    .copied()
                    .unwrap_or(0.0),
            );
        }
    }
    out
}

// Spec: slice-to-midi R14. The set saves and reopens with the same track,
// devices, slices and clip.
#[test]
fn a_sliced_track_reopens_as_it_was() {
    let scratch = Scratch::new("reopen");
    let (mut shell, _track, clip) = loop_clip(&scratch);
    let cut = slice(&mut shell, clip, json!({ "by": "transients" }));
    let before = shell.editor().document().track(cut.track).unwrap().clone();
    let before_clip = shell.editor().document().clip(cut.clip).unwrap().clone();
    let path = scratch.path("Song.blset");
    let saved = shell
        .run("set.save_as", json!({ "path": path.display().to_string() }))
        .unwrap();
    let path = PathBuf::from(saved["path"].as_str().unwrap());
    shell
        .run("set.open", json!({ "path": path.display().to_string() }))
        .unwrap();
    assert_eq!(shell.editor().document().track(cut.track).unwrap(), &before);
    assert_eq!(
        shell.editor().document().clip(cut.clip).unwrap(),
        &before_clip
    );
}

// Spec: slice-to-midi R13. The command runs by ID, as an agent runs it.
#[test]
fn the_command_runs_by_id_and_reports_the_track() {
    let scratch = Scratch::new("by-id");
    let (mut shell, _track, clip) = loop_clip(&scratch);
    let result = slice(&mut shell, clip, json!({ "by": "transients" }));
    assert!(shell.editor().document().track(result.track).is_some());
    assert!(shell.editor().document().clip(result.clip).is_some());
    assert!(result.slices > 0);
    // The dialog opens and closes, changing nothing.
    let before = shell.editor().document().clone();
    let status = shell.run(ids::OPEN, json!({ "clip": clip })).unwrap();
    assert_eq!(status["open"], true);
    assert!(status["presets"].as_array().unwrap().len() >= 2);
    shell.run(ids::CLOSE, json!({})).unwrap();
    assert_eq!(
        shell.editor().document().tracks().count(),
        before.tracks().count()
    );
}

// Spec: slice-to-midi R6; audio clip editing R20. The warped render is the
// clip's audio before the track's devices and mixer, as Consolidate renders
// it: a track muted or turned down doesn't come out silent in the slices.
#[test]
fn the_warped_render_runs_before_the_tracks_devices_and_mixer() {
    let scratch = Scratch::new("before-devices");
    let (mut shell, track, clip) = loop_clip(&scratch);
    shell
        .run("clip.set_warp", json!({ "clip": clip, "warp": true }))
        .unwrap();
    shell
        .run("mixer.set_mute", json!({ "track": track, "mute": true }))
        .unwrap();
    shell
        .run("mixer.set_volume", json!({ "track": track, "volume": 0.0 }))
        .unwrap();
    let cut = slice(&mut shell, clip, json!({ "by": "quarter" }));
    let rendered = render_track(
        &mut shell,
        &scratch,
        cut.track,
        "sliced.wav",
        Ticks::WHOLE.get() * 4,
    );
    assert!(
        rendered.iter().any(|sample| sample.abs() > 1e-6),
        "the slices hold the clip's audio, not the track's silence"
    );
}

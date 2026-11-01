//! Separate Stems to New Audio Tracks (IMP-11) through the shell, headless.
//! The behaviour is in the stem separation spec.
//!
//! The separation itself needs the packages a person downloads on first
//! use. Where `BL_STEMS_RUNTIME` names ONNX Runtime's library and
//! `BL_STEMS_MODEL` the model, they're linked into the test's packages
//! folder and the whole separation runs; without them (CI) those tests
//! pass untested, and the rest run.

use std::path::{Path, PathBuf};

use bl_media::{AudioWriter, BitDepth, Dither, EncodeSpec, FileType};
use bl_model::{ClipData, ClipId, TrackKind};
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::stems_commands::StemKind;
use bl_shell::{Shell, ShellError};
use serde_json::{Value, json};

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-stems-{name}-{}-{}",
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

fn shell(config: &Path) -> Shell {
    Shell::new(
        Preferences::default(),
        Some(config.to_owned()),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    )
}

/// Eight seconds at 48 kHz of a bass note and a noise burst every half
/// second, written as a WAV.
fn loop_file(path: &Path) {
    let rate = 48_000u32;
    let mut writer = AudioWriter::create(
        path,
        EncodeSpec {
            file_type: FileType::Wav,
            bit_depth: BitDepth::Float32,
            dither: Dither::None,
            sample_rate: rate,
            channels: 2,
        },
    )
    .unwrap();
    let mut seed = 7u32;
    let samples: Vec<f32> = (0..rate as usize * 8)
        .flat_map(|i| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = f64::from(seed >> 8) / f64::from(1u32 << 24) * 2.0 - 1.0;
            let t = i as f64 / f64::from(rate);
            let bass = 0.4 * (std::f64::consts::TAU * 55.0 * t).sin();
            let since = t % 0.5;
            let hat = if since < 0.03 { 0.5 * noise } else { 0.0 };
            let v = (bass + hat) as f32;
            [v, v]
        })
        .collect();
    writer.write(&samples).unwrap();
    writer.finish().unwrap();
}

/// A shell with the loop on a new audio track; the clip's ID.
fn with_clip(scratch: &Scratch) -> (Shell, ClipId) {
    let mut shell = shell(&scratch.path("config"));
    let file = scratch.path("Loop.wav");
    loop_file(&file);
    let placed = shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [text(&file)], "at": 0 }),
        )
        .unwrap();
    let clip: ClipId = serde_json::from_value(placed["clips"][0]["clip"].clone()).unwrap();
    (shell, clip)
}

/// Links the packages named by the environment into the shell's packages
/// folder; false if they aren't named.
fn link_packages(shell: &Shell) -> bool {
    let (Some(runtime), Some(model)) = (
        std::env::var_os("BL_STEMS_RUNTIME").map(PathBuf::from),
        std::env::var_os("BL_STEMS_MODEL").map(PathBuf::from),
    ) else {
        return false;
    };
    let folder = shell.packages_folder();
    for (from, package) in [
        (runtime, bl_shell::stems::runtime().unwrap()),
        (model, bl_shell::stems::model()),
    ] {
        let to = package.path(&folder);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&from, &to).unwrap();
        #[cfg(not(unix))]
        std::fs::copy(&from, &to).unwrap();
    }
    true
}

fn status(shell: &mut Shell) -> Value {
    shell.run("stems.status", Value::Null).unwrap()
}

// Spec: stem-separation R1, R2, R3.
#[test]
fn the_first_time_the_dialog_lists_the_packages_to_download() {
    if bl_shell::stems::packages().is_empty() {
        return;
    }
    let scratch = Scratch::new("first-time");
    let (mut shell, clip) = with_clip(&scratch);
    let listed = status(&mut shell);
    assert_eq!(listed["installed"], false);
    let packages = listed["packages"].as_array().unwrap();
    assert_eq!(packages.len(), 2);
    for package in packages {
        assert!(
            package["url"]
                .as_str()
                .unwrap()
                .starts_with("https://github.com/")
        );
        assert!(package["size"].as_u64().unwrap() > 1_000_000);
        assert_eq!(package["installed"], false);
    }
    assert!(
        listed["folder"]
            .as_str()
            .unwrap()
            .starts_with(&text(&scratch.path("config")))
    );
    let opened = shell.run("stems.open", json!({ "clip": clip })).unwrap();
    assert_eq!(opened["dialog"]["step"], "packages");
    // Separating without the packages says what to do.
    let refused = shell.run("stems.separate", json!({ "clip": clip }));
    assert!(
        matches!(&refused, Err(ShellError::Unavailable { reason, .. }) if reason.contains("stems.install")),
        "{refused:?}"
    );
    shell.run("stems.close", Value::Null).unwrap();
    assert!(status(&mut shell).get("dialog").is_none());
}

// Spec: stem-separation R1.
#[test]
fn the_dialog_opens_on_one_audio_clip() {
    if bl_shell::stems::packages().is_empty() {
        return;
    }
    let scratch = Scratch::new("audio-only");
    let (mut shell, clip) = with_clip(&scratch);
    assert!(shell.can_invoke("stems.open", &json!({ "clip": clip })));
    assert!(!shell.can_invoke("stems.open", &json!({ "clip": 999_999 })));
    let refused = shell.run("stems.open", json!({ "clip": 999_999 }));
    assert!(
        matches!(refused, Err(ShellError::Unavailable { .. })),
        "{refused:?}"
    );
}

// Spec: stem-separation R5.
#[test]
fn merge_needs_two_or_three_parts() {
    let scratch = Scratch::new("merge");
    let (mut shell, clip) = with_clip(&scratch);
    if !link_packages(&shell) {
        return;
    }
    for stems in [
        json!(["vocals"]),
        json!(["vocals", "drums", "bass", "others"]),
    ] {
        let refused = shell.run(
            "stems.separate",
            json!({ "clip": clip, "stems": stems, "merge": true }),
        );
        assert!(
            matches!(refused, Err(ShellError::Unavailable { .. })),
            "{refused:?}"
        );
    }
}

// Spec: stem-separation R6, R8, R9, R10, R11; R33 (`stems.status` names the
// provider).
#[test]
fn separating_makes_a_group_of_stem_tracks_and_deactivates_the_clip() {
    let scratch = Scratch::new("separate");
    let (mut shell, clip) = with_clip(&scratch);
    if !link_packages(&shell) {
        return;
    }
    shell
        .run(
            "set.save_as",
            json!({ "path": text(&scratch.path("Song.als")) }),
        )
        .unwrap();
    let source_track = shell.editor().document().clip_track(clip).unwrap();
    let before = shell.editor().document().top_level().count();
    shell.run("stems.open", json!({ "clip": clip })).unwrap();
    let started = shell
        .run("stems.separate", json!({ "clip": clip }))
        .unwrap();
    assert_eq!(started["separation"]["state"], "running");
    assert_eq!(started["dialog"]["step"], "separating");
    shell.wait_stems();
    let status = status(&mut shell);
    assert_eq!(status["separation"]["state"], "done", "{status}");
    assert!(status.get("dialog").is_none());
    let provider = status["provider"].as_str().unwrap();
    assert!(
        provider.starts_with("CPU") || provider.starts_with("Core ML"),
        "{status}"
    );
    let created: Vec<bl_model::TrackId> =
        serde_json::from_value(status["created_tracks"].clone()).unwrap();
    assert_eq!(created.len(), 5);
    let doc = shell.editor().document();
    // One group below the clip's track, holding a track per part.
    assert_eq!(doc.top_level().count(), before + 1);
    let order: Vec<_> = doc.top_level().collect();
    let at = order.iter().position(|t| *t == source_track).unwrap();
    assert_eq!(order[at + 1], created[0]);
    let group = doc.track(created[0]).unwrap();
    assert_eq!(group.kind(), TrackKind::Group);
    assert_eq!(group.name(), "Loop");
    let names: Vec<_> = group
        .children()
        .map(|t| doc.track(t).unwrap().name().to_owned())
        .collect();
    assert_eq!(names, ["Vocals", "Drums", "Bass", "Others"]);
    // Each holds a copy of the clip playing its part, from the project's
    // Stems folder; the original is deactivated.
    let original = doc.clip(clip).unwrap().clone();
    assert!(original.mute);
    for track in &created[1..] {
        let copy = doc.track(*track).unwrap().clips().next().unwrap();
        assert_eq!((copy.start, copy.length), (original.start, original.length));
        assert!(!copy.mute);
        let ClipData::Audio(audio) = &copy.data else {
            panic!("an audio clip");
        };
        let ClipData::Audio(source) = &original.data else {
            panic!("an audio clip");
        };
        assert_ne!(audio.media, source.media);
    }
    let stems = shell.processed_folder(bl_shell::processed::Kind::Stems);
    assert!(
        stems.ends_with("Samples/Processed/Stems"),
        "{}",
        stems.display()
    );
    let mut files: Vec<_> = std::fs::read_dir(&stems)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(
        files,
        [
            "Loop Bass.wav",
            "Loop Drums.wav",
            "Loop Others.wav",
            "Loop Vocals.wav"
        ]
    );
    // One undo step takes it all back.
    shell.run("edit.undo", json!({})).unwrap();
    let doc = shell.editor().document();
    assert_eq!(doc.top_level().count(), before);
    assert!(!doc.clip(clip).unwrap().mute);
}

// Spec: stem-separation R10.
#[test]
fn merged_parts_share_one_track_and_keep_their_own_files() {
    let scratch = Scratch::new("merged");
    let (mut shell, clip) = with_clip(&scratch);
    if !link_packages(&shell) {
        return;
    }
    let before = shell.editor().document().top_level().count();
    shell
        .run(
            "stems.separate",
            json!({ "clip": clip, "stems": ["drums", "bass"], "merge": true }),
        )
        .unwrap();
    shell.wait_stems();
    let status = status(&mut shell);
    let created: Vec<bl_model::TrackId> =
        serde_json::from_value(status["created_tracks"].clone()).unwrap();
    assert_eq!(created.len(), 1);
    let doc = shell.editor().document();
    assert_eq!(doc.top_level().count(), before + 1);
    let track = doc.track(created[0]).unwrap();
    assert_eq!(track.kind(), TrackKind::Audio);
    assert_eq!(track.name(), "Drums + Bass");
    let folder = shell.processed_folder(bl_shell::processed::Kind::Stems);
    let mut files: Vec<_> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(
        files,
        ["Loop Bass.wav", "Loop Drums + Bass.wav", "Loop Drums.wav"]
    );
}

// Spec: stem-separation R3, R4. Downloads about 350 MB, so it runs only
// where `BL_STEMS_DOWNLOAD` is set.
#[test]
fn the_packages_download_check_and_install() {
    if std::env::var_os("BL_STEMS_DOWNLOAD").is_none() || bl_shell::stems::packages().is_empty() {
        return;
    }
    let scratch = Scratch::new("download");
    let (mut shell, clip) = with_clip(&scratch);
    shell.run("stems.open", json!({ "clip": clip })).unwrap();
    let started = shell.run("stems.install", Value::Null).unwrap();
    assert_eq!(started["install"]["state"], "running");
    assert_eq!(started["dialog"]["step"], "installing");
    shell.wait_stems();
    let done = status(&mut shell);
    assert_eq!(done["install"]["state"], "done", "{done}");
    assert_eq!(done["installed"], true);
    // Spec: stem-separation R4. Then the dialog offers the stems.
    assert_eq!(done["dialog"]["step"], "options");
    let runtime = bl_shell::stems::runtime().unwrap();
    let folder = shell.packages_folder().join(runtime.folder);
    assert!(folder.join("LICENSE").is_file());
    shell
        .run("stems.separate", json!({ "clip": clip, "stems": ["bass"] }))
        .unwrap();
    shell.wait_stems();
    assert_eq!(status(&mut shell)["separation"]["state"], "done");
}

/// Where the loudest moments of a rendered file fall, in seconds: one per
/// half-second window.
fn hits(path: &Path) -> Vec<f64> {
    let audio = bl_media::decode_file(path).unwrap();
    let rate = f64::from(audio.sample_rate());
    let left = audio.channel(0).unwrap();
    let window = (rate / 2.0) as usize;
    left.chunks(window)
        .enumerate()
        .filter(|(_, c)| c.len() == window)
        .map(|(i, chunk)| {
            let (at, _) = chunk.iter().enumerate().fold((0, 0.0f32), |best, (j, x)| {
                if x.abs() > best.1 { (j, x.abs()) } else { best }
            });
            (i * window + at) as f64 / rate
        })
        .collect()
}

// Spec: stem-separation R10, R11. A stem plays at the clip's speed: its
// drum hits land where the mix's do, though the stems are 44.1 kHz and the
// clip's file 48 kHz.
#[test]
fn a_stem_plays_in_time_with_its_clip() {
    let scratch = Scratch::new("in-time");
    let (mut shell, clip) = with_clip(&scratch);
    if !link_packages(&shell) {
        return;
    }
    shell
        .run(
            "stems.separate",
            json!({ "clip": clip, "stems": ["drums"] }),
        )
        .unwrap();
    shell.wait_stems();
    let status = status(&mut shell);
    let drums: bl_model::TrackId =
        serde_json::from_value(status["created_tracks"][0].clone()).unwrap();
    let path = scratch.path("drums.wav");
    shell
        .run(
            "export.audio",
            json!({
                "path": text(&path),
                "bit_depth": 32,
                "sample_rate": 48000,
                "rendered_track": { "track": drums },
            }),
        )
        .unwrap();
    let hits = hits(&path);
    // Eight seconds, a hit every half second: not sped up or slowed down.
    assert_eq!(hits.len(), 16, "{hits:?}");
    for (i, at) in hits.iter().enumerate() {
        assert!((at - i as f64 * 0.5).abs() < 0.04, "{hits:?}");
    }
}

// Spec: stem-separation R9, R11. The stems sound through their group: the
// set's output after separating is the mix, not silence.
#[test]
fn the_stems_sound_through_their_group() {
    let scratch = Scratch::new("audible");
    let (mut shell, clip) = with_clip(&scratch);
    if !link_packages(&shell) {
        return;
    }
    shell
        .run("stems.separate", json!({ "clip": clip }))
        .unwrap();
    shell.wait_stems();
    let path = scratch.path("main.wav");
    let exported = shell
        .run(
            "export.audio",
            json!({ "path": text(&path), "bit_depth": 32, "sample_rate": 48000 }),
        )
        .unwrap();
    let peak = exported["peak_dbfs"].as_f64();
    assert!(peak.is_some_and(|p| p > -20.0), "{exported}");
}

// The tests below run with `use_stem_fixture`: a generated stand-in splits
// the mix into four fixed shares that sum to it, so they run without the
// packages or the network (WP-9.2, stem separation R22 to R31).

const WHOLE: i64 = bl_core::time::Ticks::WHOLE.get();

fn ticks(bars: i64) -> bl_core::time::Ticks {
    bl_core::time::Ticks(bars * WHOLE)
}

/// Eight seconds at 48 kHz of two low tones, which resample and render back
/// to themselves closely enough for a null test.
fn tone_file(path: &Path) {
    let rate = 48_000u32;
    let mut writer = AudioWriter::create(
        path,
        EncodeSpec {
            file_type: FileType::Wav,
            bit_depth: BitDepth::Float32,
            dither: Dither::None,
            sample_rate: rate,
            channels: 2,
        },
    )
    .unwrap();
    let samples: Vec<f32> = (0..rate as usize * 8)
        .flat_map(|i| {
            let t = i as f64 / f64::from(rate);
            let tone = 0.3 * (std::f64::consts::TAU * 110.0 * t).sin()
                + 0.2 * (std::f64::consts::TAU * 330.0 * t).sin();
            [tone as f32, tone as f32]
        })
        .collect();
    writer.write(&samples).unwrap();
    writer.finish().unwrap();
}

fn fixture_shell(scratch: &Scratch) -> Shell {
    let mut shell = shell(&scratch.path("config"));
    shell.use_stem_fixture();
    shell
}

fn audio_tracks(shell: &Shell) -> Vec<bl_model::TrackId> {
    let doc = shell.editor().document();
    doc.top_level()
        .filter(|t| doc.track(*t).unwrap().kind() == TrackKind::Audio)
        .collect()
}

/// Puts the tone file on `track` at `bar`; the clip.
fn place(shell: &mut Shell, file: &Path, track: bl_model::TrackId, bar: i64) -> ClipId {
    let placed = shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [text(file)], "track": track, "at": ticks(bar) }),
        )
        .unwrap();
    serde_json::from_value(placed["clips"][0]["clip"].clone()).unwrap()
}

/// Separates and waits; the status.
fn separate(shell: &mut Shell, args: Value) -> Value {
    shell.run("stems.separate", args).unwrap();
    shell.wait_stems();
    let status = status(shell);
    assert_eq!(status["separation"]["state"], "done", "{status}");
    status
}

fn ids<T: serde::de::DeserializeOwned>(status: &Value, key: &str) -> Vec<T> {
    serde_json::from_value(status[key].clone()).unwrap()
}

fn colours(shell: &Shell, tracks: &[bl_model::TrackId]) -> Vec<u8> {
    let doc = shell.editor().document();
    tracks
        .iter()
        .map(|t| doc.track(*t).unwrap().colour())
        .collect()
}

fn names_of(shell: &Shell, tracks: &[bl_model::TrackId]) -> Vec<String> {
    let doc = shell.editor().document();
    tracks
        .iter()
        .map(|t| doc.track(*t).unwrap().name().to_owned())
        .collect()
}

/// The undo steps' names, the newest first.
fn step_names(shell: &Shell) -> Vec<String> {
    shell.editor().undo_history().map(str::to_owned).collect()
}

// Spec: stem-separation R22, R23, R26, R29, R30. A Session clip's stems go in
// new tracks after its own, each in the slot of the clip's scene with the
// clip's launch settings; the clip is deactivated, and the group takes its
// colour.
#[test]
fn a_session_clips_stems_go_in_its_scenes_slots() {
    let scratch = Scratch::new("session");
    let mut shell = fixture_shell(&scratch);
    let file = scratch.path("Loop.wav");
    loop_file(&file);
    let track = audio_tracks(&shell)[0];
    let scene = shell.editor().document().scenes().nth(1).unwrap().id;
    shell
        .run(
            "session.drop_files",
            json!({ "paths": [text(&file)], "track": track, "scene": scene }),
        )
        .unwrap();
    let clip = shell.editor().document().slot(track, scene).unwrap().id;
    shell
        .run(
            "clip.set_launch_mode",
            json!({ "clip": clip, "mode": "gate" }),
        )
        .unwrap();
    shell
        .run("clip.set_colour", json!({ "clips": [clip], "colour": 33 }))
        .unwrap();
    let before = shell.editor().document().clone();
    let source = shell
        .editor()
        .document()
        .slot(track, scene)
        .unwrap()
        .clone();

    let status = separate(
        &mut shell,
        json!({ "source": { "kind": "clip", "clip": clip } }),
    );
    let created: Vec<bl_model::TrackId> = ids(&status, "created_tracks");
    let clips: Vec<ClipId> = ids(&status, "created_clips");
    assert_eq!((created.len(), clips.len()), (5, 4));
    let doc = shell.editor().document();
    // After the clip's track, in a group named for the clip.
    let order: Vec<_> = doc.top_level().collect();
    let at = order.iter().position(|t| *t == track).unwrap();
    assert_eq!(order[at + 1], created[0]);
    assert_eq!(doc.track(created[0]).unwrap().name(), "Loop");
    assert_eq!(
        names_of(&shell, &created[1..]),
        ["Vocals", "Drums", "Bass", "Others"]
    );
    // Each part's clip is in its track's slot in the clip's scene, with the
    // clip's launch settings and no clip in the Arrangement.
    for (track, clip) in created[1..].iter().zip(&clips) {
        let held = doc.track(*track).unwrap();
        assert_eq!(held.clips().count(), 0);
        let slot = doc.slot(*track, scene).unwrap();
        assert_eq!(slot.id, *clip);
        assert_eq!(slot.launch, source.launch);
        assert!(!slot.mute);
        assert_eq!(held.slots().count(), 1);
    }
    // Spec: R26. The group takes the clip's colour, each part its own.
    assert_eq!(colours(&shell, &created[..1]), [33]);
    assert_eq!(colours(&shell, &created[1..]), [10, 2, 6, 4]);
    // The source is deactivated, and one undo step takes it all back.
    assert!(doc.slot(track, scene).unwrap().mute);
    assert_eq!(step_names(&shell)[0], "Separate Stems");
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(*shell.editor().document(), before);
}

// Spec: stem-separation R22, R24, R26, R28, R29. A file's stems go after the
// selected track, their clips at the insert marker.
#[test]
fn a_browser_files_stems_follow_the_selected_track_at_the_insert_marker() {
    let scratch = Scratch::new("file");
    let mut shell = fixture_shell(&scratch);
    let file = scratch.path("Pad.wav");
    tone_file(&file);
    let tracks = audio_tracks(&shell);
    shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[0]] }),
        )
        .unwrap();
    shell
        .run("arrangement.set_insert_marker", json!({ "at": ticks(3) }))
        .unwrap();
    let before = shell.editor().document().clone();

    let status = separate(
        &mut shell,
        json!({ "source": { "kind": "file", "path": text(&file) }, "stems": ["vocals", "bass"] }),
    );
    let created: Vec<bl_model::TrackId> = ids(&status, "created_tracks");
    assert_eq!(created.len(), 3);
    let doc = shell.editor().document();
    let order: Vec<_> = doc.top_level().collect();
    let at = order.iter().position(|t| *t == tracks[0]).unwrap();
    assert_eq!(order[at + 1], created[0]);
    assert_eq!(doc.track(created[0]).unwrap().name(), "Pad");
    // The group takes the selected track's colour.
    assert_eq!(
        colours(&shell, &created[..1]),
        [doc.track(tracks[0]).unwrap().colour()]
    );
    for (track, part) in created[1..].iter().zip(["Vocals", "Bass"]) {
        let clip = doc.track(*track).unwrap().clips().next().unwrap();
        assert_eq!(clip.start, ticks(3));
        assert_eq!(clip.name, part);
        let ClipData::Audio(audio) = &clip.data else {
            panic!("an audio clip");
        };
        assert!(audio.start.abs() < 1e-9, "the whole file");
    }
    let folder = shell.processed_folder(bl_shell::processed::Kind::Stems);
    let mut files: Vec<_> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(files, ["Pad Bass.wav", "Pad Vocals.wav"]);
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(*shell.editor().document(), before);
}

// Spec: stem-separation R24. With no track selected, they go after the last.
#[test]
fn a_file_with_no_track_selected_goes_after_the_last_track() {
    let scratch = Scratch::new("file-last");
    let mut shell = fixture_shell(&scratch);
    let file = scratch.path("Pad.wav");
    tone_file(&file);
    shell
        .run("arrangement.select_tracks", json!({ "tracks": [] }))
        .unwrap();
    let last = shell.editor().document().top_level().last().unwrap();
    let status = separate(
        &mut shell,
        json!({ "source": { "kind": "file", "path": text(&file) }, "stems": ["drums"] }),
    );
    let created: Vec<bl_model::TrackId> = ids(&status, "created_tracks");
    assert_eq!(created.len(), 1);
    let order: Vec<_> = shell.editor().document().top_level().collect();
    let at = order.iter().position(|t| *t == last).unwrap();
    assert_eq!(order[at + 1], created[0]);
}

/// The set's output, left channel, at 48 kHz.
fn output(shell: &mut Shell, scratch: &Scratch, name: &str) -> Vec<f32> {
    let path = scratch.path(name);
    shell
        .run(
            "export.audio",
            json!({ "path": text(&path), "bit_depth": 32, "sample_rate": 48000 }),
        )
        .unwrap();
    bl_media::decode_file(&path)
        .unwrap()
        .channel(0)
        .unwrap()
        .to_vec()
}

// Spec: stem-separation R25, R29. A time selection over two clips of a track
// separates only its range: the clips are split at its edges and their
// parts inside deactivated, so the set sounds as it did.
#[test]
fn a_time_selection_separates_its_range_and_leaves_the_set_sounding_the_same() {
    let scratch = Scratch::new("selection");
    let mut shell = fixture_shell(&scratch);
    let file = scratch.path("Tone.wav");
    tone_file(&file);
    let track = audio_tracks(&shell)[0];
    let first = place(&mut shell, &file, track, 0);
    let second = place(&mut shell, &file, track, 6);
    let before = shell.editor().document().clone();
    let heard = output(&mut shell, &scratch, "before.wav");

    // The set's output is the whole set, so no time selection is made: the
    // source names the range.
    let status = separate(
        &mut shell,
        json!({ "source": { "kind": "time_selection", "tracks": [track],
                            "start": ticks(2), "end": ticks(8) } }),
    );
    let created: Vec<bl_model::TrackId> = ids(&status, "created_tracks");
    assert_eq!(created.len(), 5);
    let doc = shell.editor().document();
    // The stems span the selection exactly, on tracks named for it.
    assert_eq!(
        doc.track(created[0]).unwrap().name(),
        format!("{} 3-9", doc.track(track).unwrap().name())
    );
    for stem in &created[1..] {
        let clips: Vec<_> = doc.track(*stem).unwrap().clips().collect();
        assert_eq!(clips.len(), 1);
        assert_eq!((clips[0].start, clips[0].length), (ticks(2), ticks(6)));
    }
    // The source clips are split at the selection's edges, and the parts
    // inside are deactivated.
    let mut parts: Vec<_> = doc
        .track(track)
        .unwrap()
        .clips()
        .map(|c| (c.start, c.end(), c.mute))
        .collect();
    parts.sort();
    assert_eq!(
        parts,
        [
            (ticks(0), ticks(2), false),
            (ticks(2), ticks(4), true),
            (ticks(6), ticks(8), true),
            (ticks(8), ticks(10), false),
        ]
    );
    assert_eq!(doc.clip(first).unwrap().start, ticks(0));
    assert!(doc.clip(second).is_some());
    // The set sounds as it did before: the stems sum to the selection.
    let after = output(&mut shell, &scratch, "after.wav");
    let length = heard.len().min(after.len());
    assert!(length > 48_000 * 10, "{length}");
    let worst = heard[..length]
        .iter()
        .zip(&after[..length])
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    // The stems are 44.1 kHz, so what is left is the conversion's ringing at
    // the clips' edges: under -40 dB of the tones' 0.5 peak.
    assert!(worst < 0.01, "the render nulls to within {worst}");
    // One undo step.
    assert_eq!(step_names(&shell)[0], "Separate Stems");
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(*shell.editor().document(), before);
}

// Spec: stem-separation R25. A time selection over two tracks separates each
// on its own; a track with no audio in it is skipped.
#[test]
fn a_time_selection_over_two_tracks_separates_each() {
    let scratch = Scratch::new("selection-two");
    let mut shell = fixture_shell(&scratch);
    let file = scratch.path("Tone.wav");
    tone_file(&file);
    let tracks = audio_tracks(&shell);
    place(&mut shell, &file, tracks[0], 0);
    place(&mut shell, &file, tracks[1], 0);
    let empty = shell
        .editor()
        .document()
        .top_level()
        .find(|t| shell.editor().document().track(*t).unwrap().kind() == TrackKind::Midi)
        .unwrap();
    let selection = json!({ "kind": "time_selection",
                            "tracks": [tracks[0], tracks[1], tracks[1], empty],
                            "start": ticks(1), "end": ticks(3) });
    let status = separate(
        &mut shell,
        json!({ "source": selection, "stems": ["drums", "bass"] }),
    );
    let created: Vec<bl_model::TrackId> = ids(&status, "created_tracks");
    // A group of two stems after each of the two tracks.
    assert_eq!(created.len(), 6);
    let doc = shell.editor().document();
    for (group, source) in [(created[0], tracks[0]), (created[3], tracks[1])] {
        let order: Vec<_> = doc.top_level().collect();
        let at = order.iter().position(|t| *t == source).unwrap();
        assert_eq!(order[at + 1], group);
        let inside: Vec<_> = doc.track(group).unwrap().children().collect();
        assert_eq!(names_of(&shell, &inside), ["Drums", "Bass"]);
        let parts: Vec<_> = doc
            .track(source)
            .unwrap()
            .clips()
            .map(|c| (c.start, c.end(), c.mute))
            .collect();
        assert_eq!(parts.iter().filter(|p| p.2).count(), 1);
        assert!(parts.contains(&(ticks(1), ticks(3), true)), "{parts:?}");
    }
    assert_eq!(doc.track(empty).unwrap().clips().count(), 0);
}

// Spec: stem-separation R27. The group holds a copy of the source track's
// devices and automation, and the source keeps its own; the one track of a
// single part holds them itself.
#[test]
fn the_group_carries_the_source_tracks_devices_and_automation() {
    let scratch = Scratch::new("group-copy");
    let mut shell = fixture_shell(&scratch);
    let (mut shell, clip) = {
        let file = scratch.path("Loop.wav");
        loop_file(&file);
        let placed = shell
            .run(
                "browser.add_to_arrangement",
                json!({ "paths": [text(&file)], "at": 0 }),
            )
            .unwrap();
        let clip: ClipId = serde_json::from_value(placed["clips"][0]["clip"].clone()).unwrap();
        (shell, clip)
    };
    let track = shell.editor().document().clip_track(clip).unwrap();
    let eq = bl_devices::eq_type();
    shell
        .run(
            "device.add",
            json!({ "track": track, "device": {
                "kind": eq.kind, "name": eq.name, "role": eq.role, "sidechain": eq.sidechain } }),
        )
        .unwrap();
    for (at, value) in [(0, 0.5), (WHOLE, 0.25)] {
        shell
            .run(
                "automation.set_breakpoint",
                json!({ "track": track, "target": { "type": "mixer", "param": "volume" },
                        "at": at, "value": value }),
            )
            .unwrap();
    }
    let envelope = |shell: &Shell, track| {
        shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .automation()
            .map(|(target, automation)| (*target, automation.envelope.points().collect::<Vec<_>>()))
            .collect::<Vec<_>>()
    };
    let source_envelope = envelope(&shell, track);
    assert_eq!(source_envelope.len(), 1);

    let status = separate(&mut shell, json!({ "clip": clip }));
    let created: Vec<bl_model::TrackId> = ids(&status, "created_tracks");
    let doc = shell.editor().document();
    let group = doc.track(created[0]).unwrap();
    let kinds = |t: &bl_model::Track| t.devices().map(|d| d.kind.clone()).collect::<Vec<_>>();
    assert_eq!(kinds(group), kinds(doc.track(track).unwrap()));
    assert_eq!(group.devices().count(), 1);
    // A new device, with the same settings, not the source's own.
    assert_ne!(
        group.devices().next().unwrap().id,
        doc.track(track).unwrap().devices().next().unwrap().id
    );
    assert_eq!(envelope(&shell, created[0]), source_envelope);
    assert_eq!(envelope(&shell, track), source_envelope);
    // The stems themselves hold none.
    for stem in &created[1..] {
        assert_eq!(doc.track(*stem).unwrap().devices().count(), 0);
    }

    // One part: the one new track holds the devices itself.
    shell.run("edit.undo", json!({})).unwrap();
    let status = separate(&mut shell, json!({ "clip": clip, "stems": ["bass"] }));
    let created: Vec<bl_model::TrackId> = ids(&status, "created_tracks");
    assert_eq!(created.len(), 1);
    let doc = shell.editor().document();
    assert_eq!(doc.track(created[0]).unwrap().devices().count(), 1);
    assert_eq!(envelope(&shell, created[0]), source_envelope);
    assert_eq!(
        doc.track(created[0]).unwrap().colour(),
        StemKind::Bass.swatch()
    );
}

// Spec: stem-separation R22, R31. What can't be separated is refused, with
// its reason: a time selection on MIDI tracks only, a folder, a missing file.
#[test]
fn what_holds_no_audio_is_refused_with_its_reason() {
    let scratch = Scratch::new("refused");
    let mut shell = fixture_shell(&scratch);
    let midi = shell
        .editor()
        .document()
        .top_level()
        .find(|t| shell.editor().document().track(*t).unwrap().kind() == TrackKind::Midi)
        .unwrap();
    let selection = json!({ "kind": "time_selection", "tracks": [midi],
                            "start": ticks(0), "end": ticks(2) });
    let folder = json!({ "kind": "file", "path": text(&scratch.path("config")) });
    let missing = json!({ "kind": "file", "path": text(&scratch.path("Nothing.wav")) });
    for source in [selection, folder, missing] {
        assert!(!shell.can_invoke("stems.open", &json!({ "source": source })));
        let refused = shell.run("stems.separate", json!({ "source": source }));
        assert!(
            matches!(&refused, Err(ShellError::Unavailable { .. })),
            "{source}: {refused:?}"
        );
    }
    assert!(status(&mut shell)["created_tracks"].is_null());
}

// Spec: stem-separation R22. With no source named, the Create menu's item acts
// on what is selected: a clip, else a time selection, else the clip the
// Session View shows.
#[test]
fn stems_open_acts_on_the_selection() {
    if bl_shell::stems::packages().is_empty() {
        return;
    }
    let scratch = Scratch::new("open-selected");
    let mut shell = fixture_shell(&scratch);
    let file = scratch.path("Tone.wav");
    tone_file(&file);
    let track = audio_tracks(&shell)[0];
    let clip = place(&mut shell, &file, track, 0);
    assert!(!shell.can_invoke("stems.open", &json!({})));
    shell
        .run(
            "arrangement.select_time",
            json!({ "start": ticks(1), "end": ticks(2), "tracks": [track] }),
        )
        .unwrap();
    let opened = shell.run("stems.open", json!({})).unwrap();
    assert_eq!(opened["dialog"]["source"]["kind"], "time_selection");
    shell.run("stems.close", Value::Null).unwrap();
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    let opened = shell.run("stems.open", json!({})).unwrap();
    assert_eq!(
        opened["dialog"]["source"],
        json!({ "kind": "clip", "clip": clip })
    );
}

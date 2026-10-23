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

// Spec: stem-separation R6, R8, R9, R10, R11.
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

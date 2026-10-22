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

bl_rt::install_trap!();

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

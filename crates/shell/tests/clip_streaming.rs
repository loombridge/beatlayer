//! WP-1.4b in the app, headless: the set's clips stream from its media
//! through the engine the shell plays (the null device here, paced like a
//! sound card), and Create Fades on Clip Edges, a preference on the Settings
//! window's Record, Warp & Launch page, gives new clips 4 ms fades. The
//! rules are in the audio clips spec.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bl_commands::{Context, Permission};
use bl_core::time::Ticks;
use bl_model::{AudioData, ClipData, EDGE_FADE_SECONDS, TrackId, TrackKind};
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::{Shell, commands};
use serde_json::json;

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-clip-streaming-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a 16-bit mono WAV file of `seconds` at 48 kHz: a sine at
/// -60 dBFS, so it isn't silence and is never loud.
fn wav(path: &Path, seconds: u32) {
    let frames = seconds * 48_000;
    let data = frames * 2;
    let mut bytes = Vec::with_capacity(44 + data as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let phase = f64::from(i) * 440.0 / 48_000.0 * std::f64::consts::TAU;
        let sample = (phase.sin() * 32.0) as i16;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

fn audio_track(shell: &Shell) -> TrackId {
    let document = shell.editor().document();
    document
        .tracks()
        .find(|t| t.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .expect("the default set has audio tracks")
}

/// Adds `path` to the Arrangement at 1.1.1 on the first audio track, as a
/// drop from the browser does, and returns the new clip's audio data.
fn add(shell: &mut Shell, path: &Path) -> AudioData {
    let track = audio_track(shell);
    shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [path.display().to_string()], "track": track, "at": 0 }),
        )
        .unwrap();
    let document = shell.editor().document();
    let clip = document
        .track(track)
        .unwrap()
        .clips()
        .find(|c| c.start == Ticks::ZERO)
        .unwrap();
    match clip.data {
        ClipData::Audio(audio) => audio,
        ClipData::Midi(_) => unreachable!("an audio clip"),
    }
}

// Spec: audio-clips R7, R22
#[test]
fn create_fades_on_clip_edges_is_a_kept_preference_that_new_clips_follow() {
    let scratch = Scratch::new("fades");
    let file = scratch.0.join("Tone.wav");
    wav(&file, 1);
    let config = scratch.0.join("config");
    let mut shell = Shell::new(
        Preferences::default(),
        Some(config.clone()),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    let spec = shell
        .registry()
        .get(commands::SETTINGS_SET_CREATE_FADES_ON_CLIP_EDGES)
        .expect("registered");
    assert_eq!(spec.label, "Create Fades on Clip Edges");
    assert_eq!(spec.permission, Some(Permission::Edit));
    assert_eq!(spec.contexts, [Context::Settings]);

    // Off until switched on (its default in a new install is still to
    // observe in Live): a new clip has no fades.
    assert!(!shell.create_fades_on_clip_edges());
    let plain = add(&mut shell, &file);
    assert_eq!((plain.fade_in, plain.fade_out), (0.0, 0.0));

    let result = shell
        .run(
            commands::SETTINGS_SET_CREATE_FADES_ON_CLIP_EDGES,
            json!({ "on": true }),
        )
        .unwrap();
    assert_eq!(result, json!({ "on": true }));
    assert!(shell.create_fades_on_clip_edges());
    let faded = add(&mut shell, &file);
    assert_eq!(
        (faded.fade_in, faded.fade_out),
        (EDGE_FADE_SECONDS, EDGE_FADE_SECONDS)
    );
    // Switching it isn't an edit of the set.
    assert_eq!(shell.editor().undo_history().len(), 2);

    // Kept for the next start.
    assert!(
        Preferences::load(&config)
            .unwrap()
            .create_fades_on_clip_edges
    );
    shell
        .run(
            commands::SETTINGS_SET_CREATE_FADES_ON_CLIP_EDGES,
            json!({ "on": false }),
        )
        .unwrap();
    assert!(
        !Preferences::load(&config)
            .unwrap()
            .create_fades_on_clip_edges
    );
    assert!(
        shell
            .run(
                commands::SETTINGS_SET_CREATE_FADES_ON_CLIP_EDGES,
                json!({ "on": "yes" }),
            )
            .is_err()
    );
}

// Spec: audio-clips R16, R17
#[test]
fn the_app_streams_a_long_clip_from_the_sets_media() {
    let scratch = Scratch::new("stream");
    let long = scratch.0.join("Long.wav");
    wav(&long, 12);
    let mut shell = Shell::headless();
    shell.start_audio();
    assert!(shell.audio().engine_running(), "{:?}", shell.audio());
    add(&mut shell, &long);
    shell.run("transport.play", json!({})).unwrap();

    // The disk thread opens a stream for the clip, which is longer than ten
    // seconds, and reads it ahead of the playhead.
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let stats = shell.audio().disk_stats().expect("an engine runs");
        if stats.streams == 1 && stats.totals.blocks > 0 {
            assert_eq!(stats.totals.errors, 0);
            break;
        }
        assert!(Instant::now() < until, "no stream read: {stats:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
    shell.run("transport.stop", json!({})).unwrap();
}

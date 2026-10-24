//! ARA 2 hosting through the shell (IMP-20; the ARA hosting spec R9, R11,
//! R13): the ARA SDK's example plug-in loads as a device by `plugin.load`,
//! takes its track's clips, follows `clip.move`, `clip.trim_start` and
//! `edit.undo`, and keeps its archive with the set, all through registered
//! commands as an agent runs them, and reported by `ara.state` and
//! `plugin.info`.
//!
//! It runs against `bl-test-ara`, built from source, and against the SDK's
//! example plug-in, built on the test machine and never committed, when
//! `BL_ARA_TEST_PLUGIN` names its bundle.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bl_model::{DeviceId, TrackId, TrackKind};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::plugin_commands::{self as ids, AraState, PluginInfo, PluginLoaded};
use bl_shell::settings::Preferences;
use serde_json::json;

/// A quarter note, in ticks.
const QUARTER: i64 = 705_600;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-shell-ara-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&path);
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

/// A second of 48 kHz stereo audio that differs everywhere.
fn wav(path: &Path) {
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
    for frame in 0..48_000_u32 {
        let sample = (frame % 1999) as f32 / 1999.0 - 0.5;
        writer.write(&[sample, -sample]).unwrap();
    }
    writer.finish().unwrap();
}

fn new_shell(scratch: &Scratch) -> Shell {
    let mut shell = Shell::new(
        Preferences::default(),
        Some(scratch.path("config")),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    shell.start_audio();
    shell
}

fn ara_state(shell: &mut Shell) -> AraState {
    serde_json::from_value(shell.run("ara.state", json!({})).unwrap()).unwrap()
}

/// Polls the shell as the app's frames do until `ready` holds of the state.
fn wait(shell: &mut Shell, ready: impl Fn(&AraState) -> bool) -> AraState {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        shell.poll();
        let state = ara_state(shell);
        if ready(&state) {
            return state;
        }
        assert!(Instant::now() < until, "ARA never got ready: {state:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn info(shell: &mut Shell, device: DeviceId) -> PluginInfo {
    serde_json::from_value(shell.run(ids::INFO, json!({ "device": device })).unwrap()).unwrap()
}

// Spec: ara-hosting R2, R4, R9, R10, R13
#[test]
fn an_ara_plugin_follows_the_set_through_commands_and_keeps_its_archive() {
    follows_the_set(bl_test_ara::bundle().as_os_str());
}

// Spec: ara-hosting R2, R4, R9, R10, R13
#[test]
fn the_sdks_example_plugin_follows_the_set_through_commands_too() {
    let Some(bundle) = std::env::var_os("BL_ARA_TEST_PLUGIN") else {
        eprintln!("skipped: BL_ARA_TEST_PLUGIN names the ARA SDK's example plug-in");
        return;
    };
    follows_the_set(&bundle);
}

fn follows_the_set(bundle: &std::ffi::OsStr) {
    let scratch = Scratch::new();
    let file = scratch.path("Take.wav");
    wav(&file);
    let mut shell = new_shell(&scratch);
    let track: TrackId = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .unwrap();
    let added = shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [file.display().to_string()], "track": track, "at": QUARTER }),
        )
        .unwrap();
    let clip: bl_model::ClipId = serde_json::from_value(added["clips"][0]["clip"].clone()).unwrap();

    let loaded: PluginLoaded = serde_json::from_value(
        shell
            .run(ids::LOAD, json!({ "path": bundle.to_string_lossy(), "track": track }))
            .unwrap(),
    )
    .unwrap();
    let device = loaded.device;
    let state = wait(&mut shell, |state| {
        state.plugins.first().is_some_and(|p| p.regions.len() == 1)
    });
    let plugin = &state.plugins[0];
    assert_eq!(plugin.devices.len(), 1);
    assert_eq!(plugin.devices[0].track, Some(track));
    assert_eq!(plugin.regions[0].clip, clip);
    assert_eq!(plugin.regions[0].start, 0.5);
    assert_eq!(plugin.audio_thread_reads, 0);
    assert!(plugin.frames_read >= 48_000, "the plug-in read the whole file");
    // `plugin.info` says what the device is.
    let ara = info(&mut shell, device).ara.expect("an ARA plug-in");
    assert_eq!((ara.track, ara.regions), (Some(track), 1));

    // Moved, trimmed and undone: edits to the plug-in's document.
    shell
        .run("clip.move", json!({ "clip": clip, "track": track, "start": 2 * QUARTER }))
        .unwrap();
    let state = wait(&mut shell, |s| s.plugins[0].regions[0].start == 1.0);
    assert_eq!(state.plugins[0].modifications, 1);
    shell
        .run("clip.trim_start", json!({ "clip": clip, "start": 2 * QUARTER + QUARTER / 2 }))
        .unwrap();
    let state = wait(&mut shell, |s| s.plugins[0].regions[0].start_in_audio == 0.25);
    assert_eq!(state.plugins[0].regions[0].duration, 0.75);
    shell.run("edit.undo", json!({})).unwrap();
    shell.run("edit.undo", json!({})).unwrap();
    let state = wait(&mut shell, |s| s.plugins[0].regions[0].start == 0.5);
    assert_eq!(state.plugins[0].sources, 1);
    assert_eq!(state.plugins[0].readers_opened, 1, "no object was made again");

    // Saved, and opened by another shell: the archive comes back.
    let saved = shell
        .run("set.save_as", json!({ "path": scratch.path("Song.blset").display().to_string() }))
        .unwrap();
    let path = saved["path"].as_str().unwrap().to_owned();
    drop(shell);
    let mut shell = new_shell(&scratch);
    shell.run("set.open", json!({ "path": path })).unwrap();
    let state = wait(&mut shell, |s| {
        s.plugins.first().is_some_and(|p| p.regions.len() == 1)
    });
    assert_eq!(state.plugins[0].restored.as_deref(), Some("restored"));
    assert!(state.plugins[0].frames_read >= 48_000);
}

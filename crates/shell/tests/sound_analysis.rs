//! Sound analysis in the background (WP-9.8; spec: sound-similarity R1 to
//! R8), driven headless through `Shell::run`, as agents drive it: every
//! sample of 60 seconds or less and every instrument preset in a generated
//! place gets a descriptor once, kept in the database by content; Pause and
//! Resume; and a restart, a move and a rename analyse nothing again.
//! Analysis is paced by hand, so no test waits on the clock.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bl_analysis::similarity::Descriptor;
use bl_shell::Shell;
use bl_shell::browser::db::Db;
use bl_shell::browser_commands::{self as ids, AnalysisState};
use bl_shell::settings::{BROWSER_DATABASE_FILE, Preferences};
use serde_json::json;

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-analysis-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, rest: &str) -> PathBuf {
        self.0.join(rest)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a 16-bit mono WAV of a decaying tone of `hz`, `seconds` long at
/// `rate`.
fn wav(path: &Path, hz: f64, seconds: f64, rate: u32) {
    let frames = (seconds * f64::from(rate)) as u32;
    let data = frames * 2;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let t = f64::from(i) / f64::from(rate);
        let sample = (t * hz * std::f64::consts::TAU).sin() * (-t * 6.0).exp() * 20_000.0;
        bytes.extend_from_slice(&(sample as i16).to_le_bytes());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, bytes).unwrap();
}

/// Saves a Polysynth's preset, which plays by itself, at `path`.
fn preset(path: &Path) {
    let synth = bl_devices::polysynth_type();
    let id = bl_core::id::Id::new(1).unwrap();
    let device = bl_model::Device::new(bl_model::DeviceId(id), synth.name.to_string(), synth.kind)
        .with_role(synth.role);
    bl_project::preset::Preset::of(&device).save(path).unwrap();
}

/// A shell on the null device, with its configuration folder and the
/// place as its User Library, its index read, and analysis paced by hand.
fn shell_on(config: &Path, library: &Path) -> Shell {
    let mut shell = Shell::new(
        Preferences::default(),
        Some(config.to_path_buf()),
        bl_shell::audio::AudioHost::null(Default::default()),
        Box::new(bl_shell::files::ProjectFiles::new()),
    );
    shell.pace_analysis_by_hand();
    shell.set_user_library(Some(library.to_path_buf()));
    assert!(shell.wait_for_browser_index(Duration::from_secs(60)));
    shell
}

fn state(shell: &mut Shell) -> AnalysisState {
    serde_json::from_value(shell.run(ids::ANALYSIS_STATE, json!({})).unwrap()).unwrap()
}

/// A place of two samples, a sample too long, a file that isn't audio, and
/// a preset.
fn place(scratch: &Scratch) -> PathBuf {
    let library = scratch.path("Library");
    wav(&library.join("Kick.wav"), 55.0, 0.4, 22_050);
    wav(&library.join("Snare.wav"), 220.0, 0.3, 48_000);
    wav(&library.join("Long.wav"), 110.0, 61.0, 8_000);
    std::fs::write(library.join("Broken.wav"), b"not audio at all").unwrap();
    preset(&library.join("Lead.blpreset"));
    library
}

// Spec: sound-similarity R1, R2, R4, R8.
#[test]
fn every_short_sample_and_instrument_preset_gets_a_descriptor_once() {
    let scratch = Scratch::new("once");
    let config = scratch.path("config");
    let mut shell = shell_on(&config, &place(&scratch));
    let before = state(&mut shell);
    assert_eq!((before.waiting, before.analysed), (5, 0));

    shell.release_analysis(5);
    let after = state(&mut shell);
    assert_eq!(
        (after.analysed, after.skipped, after.waiting, after.computed),
        (3, 2, 0, 3),
        "the two samples and the preset; the long file and the broken one skipped"
    );
    assert!(after.share.is_none() && after.current.is_none());

    // The descriptors are in the database, by content.
    shell.browser_view().flush_database();
    let stored = Db::open(config.join(BROWSER_DATABASE_FILE)).descriptors();
    assert_eq!(stored.len(), 3);
    for (key, version, bytes) in &stored {
        assert_eq!(*version, bl_analysis::similarity::VERSION, "{key}");
        assert!(Descriptor::from_bytes(bytes).is_some(), "{key}");
    }
    // A preset's descriptor comes from its render: a Polysynth's note is
    // pitched, which a descriptor of silence isn't.
    let pitched = stored
        .iter()
        .filter_map(|(_, _, bytes)| Descriptor::from_bytes(bytes))
        .filter(|d| d.values()[d.values().len() - 1] > 0.5)
        .count();
    assert!(pitched >= 1, "a pitched sound");

    // Asking again analyses nothing.
    shell.release_analysis(3);
    assert_eq!(state(&mut shell).computed, 3);
}

// Spec: sound-similarity R4, R8.
#[test]
fn a_restart_keeps_every_descriptor_and_analyses_nothing_again() {
    let scratch = Scratch::new("restart");
    let config = scratch.path("config");
    let library = place(&scratch);
    {
        let mut shell = shell_on(&config, &library);
        shell.release_analysis(5);
        shell.browser_view().flush_database();
    }
    let mut shell = shell_on(&config, &library);
    let restarted = state(&mut shell);
    assert_eq!(
        (
            restarted.analysed,
            restarted.skipped,
            restarted.waiting,
            restarted.computed
        ),
        (3, 2, 0, 0)
    );
}

// Spec: sound-similarity R4.
#[test]
fn a_moved_and_a_renamed_file_keep_their_descriptors() {
    let scratch = Scratch::new("moved");
    let config = scratch.path("config");
    let library = place(&scratch);
    {
        let mut shell = shell_on(&config, &library);
        shell.release_analysis(5);
        shell.browser_view().flush_database();
    }
    std::fs::rename(library.join("Kick.wav"), library.join("Boom.wav")).unwrap();
    std::fs::create_dir_all(library.join("Drums")).unwrap();
    std::fs::rename(library.join("Snare.wav"), library.join("Drums/Snare.wav")).unwrap();
    // A copy shares the descriptor, too.
    std::fs::copy(library.join("Boom.wav"), library.join("Boom copy.wav")).unwrap();

    let mut shell = shell_on(&config, &library);
    let found = state(&mut shell);
    assert_eq!(
        found.waiting, 3,
        "the two moved files and the copy are looked at"
    );
    shell.release_analysis(3);
    let done = state(&mut shell);
    assert_eq!(
        (done.waiting, done.computed),
        (0, 0),
        "but none is described again"
    );
    assert_eq!((done.analysed, done.skipped), (4, 2));
}

// Spec: sound-similarity R4. A changed file is described again.
#[test]
fn a_changed_file_is_analysed_again() {
    let scratch = Scratch::new("changed");
    let config = scratch.path("config");
    let library = place(&scratch);
    {
        let mut shell = shell_on(&config, &library);
        shell.release_analysis(5);
        shell.browser_view().flush_database();
    }
    wav(&library.join("Kick.wav"), 80.0, 0.5, 22_050);
    let mut shell = shell_on(&config, &library);
    assert_eq!(state(&mut shell).waiting, 1);
    shell.release_analysis(1);
    let done = state(&mut shell);
    assert_eq!((done.computed, done.analysed), (1, 3));
}

// Spec: sound-similarity R6, R7. Pause stops after the file under way and
// Resume goes on; an agent reads it.
#[test]
fn pause_stops_the_analysis_and_resume_goes_on() {
    let scratch = Scratch::new("pause");
    let config = scratch.path("config");
    let mut shell = shell_on(&config, &place(&scratch));

    shell.release_analysis(2);
    let begun = state(&mut shell);
    assert_eq!((begun.waiting, begun.paused), (3, false));
    assert!(
        (begun.share.unwrap() - 0.4).abs() < 1e-9,
        "{:?}",
        begun.share
    );

    let paused: AnalysisState = serde_json::from_value(
        shell
            .run(ids::SET_ANALYSIS_PAUSED, json!({ "paused": true }))
            .unwrap(),
    )
    .unwrap();
    assert!(paused.paused);
    shell.release_analysis(3);
    let held = state(&mut shell);
    assert_eq!(
        (held.waiting, held.paused),
        (3, true),
        "nothing more is begun"
    );

    // With no argument it toggles.
    let resumed: AnalysisState =
        serde_json::from_value(shell.run(ids::SET_ANALYSIS_PAUSED, json!({})).unwrap()).unwrap();
    assert!(!resumed.paused);
    shell.release_analysis(3);
    let done = state(&mut shell);
    assert_eq!((done.waiting, done.paused), (0, false));
    assert_eq!(done.analysed + done.skipped, 5);
}

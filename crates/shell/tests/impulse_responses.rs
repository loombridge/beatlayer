//! Impulse-response slots in the shell (WP-8.30; the impulse responses spec,
//! R1 to R15): `device.load_sample` fills a slot of the convolution test
//! device, one undo step; the set keeps the response by content hash and
//! plays it again after a reopen; a refused file leaves the slot alone;
//! `device.impulse_responses` and the query tool's `device` result read it
//! back; Manage Files lists a missing response. Driven through `Shell::run`,
//! as an agent drives it (D17).

use std::path::{Path, PathBuf};

use bl_core::time::SampleRate;
use bl_devices::testing::{self, CONVOLUTION};
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_engine::{Engine, EngineConfig, Processor};
use bl_media::{BitDepth, Dither};
use bl_model::{DeviceId, TrackKind};
use bl_shell::Shell;
use bl_shell::device_commands::LoadSampleResult;
use serde_json::{Value, json};

const BLOCK: usize = 512;

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-impulse-shell-{name}-{}-{}",
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

/// Writes `channels` as a float WAV file at `rate`.
fn write(path: &Path, channels: &[Vec<f32>], rate: u32) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let frames = channels[0].len();
    let interleaved: Vec<f32> = (0..frames)
        .flat_map(|i| channels.iter().map(move |c| c[i]))
        .collect();
    let bytes = bl_media::wav_bytes(
        &interleaved,
        channels.len() as u16,
        rate,
        BitDepth::Float32,
        Dither::None,
    )
    .unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// A decaying burst of noise, `frames` long.
fn burst(frames: usize, seed: u32) -> Vec<f32> {
    let mut seed = seed;
    (0..frames)
        .map(|i| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((seed >> 8) as f32 / (1 << 23) as f32 - 1.0)
                * (-(i as f32) / (frames as f32 / 6.0)).exp()
        })
        .collect()
}

/// A shell whose track has a test tone and the convolution test device.
fn shell_with_device(scratch: &Scratch) -> (Shell, DeviceId, bl_engine::Registry) {
    let mut registry = testing::registry();
    let mut shell = Shell::headless();
    registry.set_media(shell.media_store());
    shell.set_device_types(registry.device_types());
    let track = shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .expect("the default set has an audio track");
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    // The tone isn't an offered type, so it is named by hand.
    let tone = bl_model::DeviceKind::Native {
        device_type: TEST_TONE.to_owned(),
    };
    let convolution = testing::convolution_type();
    for (kind, name, role) in [
        (tone, "Test Tone", bl_model::DeviceRole::AudioEffect),
        (convolution.kind, "Test Convolution", convolution.role),
    ] {
        shell
            .run(
                "device.add",
                json!({ "device": { "kind": kind, "name": name, "role": role } }),
            )
            .unwrap();
    }
    let device = shell
        .editor()
        .document()
        .tracks()
        .flat_map(|t| t.devices().map(|d| (d.id, d.kind.clone())).collect::<Vec<_>>())
        .find_map(|(id, kind)| {
            matches!(&kind, bl_model::DeviceKind::Native { device_type } if device_type == CONVOLUTION)
                .then_some(id)
        })
        .expect("the convolution device was added");
    shell
        .run(
            "device.set_param",
            json!({ "device": tone_of(&shell), "param": test_tone::LEVEL, "value": -6.0 }),
        )
        .unwrap();
    let _ = scratch;
    (shell, device, registry)
}

fn tone_of(shell: &Shell) -> DeviceId {
    shell
        .editor()
        .document()
        .tracks()
        .flat_map(|t| t.devices().map(|d| (d.id, d.kind.clone())).collect::<Vec<_>>())
        .find_map(|(id, kind)| {
            matches!(&kind, bl_model::DeviceKind::Native { device_type } if device_type == TEST_TONE)
                .then_some(id)
        })
        .expect("the tone was added")
}

fn load(shell: &mut Shell, device: DeviceId, file: &Path) -> LoadSampleResult {
    serde_json::from_value(
        shell
            .run(
                "device.load_sample",
                json!({ "device": device, "path": text(file) }),
            )
            .unwrap(),
    )
    .unwrap()
}

fn responses(shell: &mut Shell, device: DeviceId) -> Value {
    shell
        .run("device.impulse_responses", json!({ "device": device }))
        .unwrap()
}

/// How loud the first output channel gets over `blocks` blocks of an engine
/// running the set.
fn loudest(shell: &Shell, registry: bl_engine::Registry, blocks: usize) -> f32 {
    let (engine, mut processor): (Engine, Processor) =
        Engine::start(EngineConfig::new(SampleRate::HZ_48000, BLOCK), registry).unwrap();
    engine.sync(shell.editor().document());
    assert!(engine.flush(std::time::Duration::from_secs(10)));
    (0..blocks)
        .map(|_| {
            processor
                .process(BLOCK)
                .channel(0)
                .iter()
                .fold(0.0_f32, |m, s| m.max(s.abs()))
        })
        .fold(0.0, f32::max)
}

// Spec: impulse-responses R2, R3, R4, R12
#[test]
fn a_mono_a_stereo_and_a_true_stereo_response_load_at_three_rates_one_undo_step_each() {
    let scratch = Scratch::new("load");
    let (mut shell, device, _) = shell_with_device(&scratch);
    for (channels, rate, layout) in [
        (1, 44_100, "mono"),
        (2, 48_000, "stereo"),
        (4, 96_000, "true_stereo"),
    ] {
        let file = scratch.path(&format!("R{channels}.wav"));
        let data: Vec<Vec<f32>> = (0..channels).map(|c| burst(2000, 10 + c)).collect();
        write(&file, &data, rate);
        let steps = shell.editor().undo_history().count();
        let loaded = load(&mut shell, device, &file);
        assert_eq!(
            shell.editor().undo_history().count(),
            steps + 1,
            "{layout}: one undo step"
        );
        assert_eq!(
            shell.editor().undo_history().next(),
            Some("Load Impulse Response")
        );
        let reading = loaded.impulse_response.expect("the readings come back");
        assert_eq!(reading.layout.as_deref(), Some(layout));
        assert_eq!(reading.file_frames, Some(2000));
        assert_eq!(reading.category.as_deref(), Some("User"));
        let read = responses(&mut shell, device);
        assert_eq!(read["slots"][0]["state"], "ready");
        assert_eq!(read["slots"][0]["media"], json!(loaded.media));
        shell.run("edit.undo", json!({})).unwrap();
        shell.run("edit.redo", json!({})).unwrap();
    }
}

// Spec: impulse-responses R3, R14
#[test]
fn a_three_channel_file_and_a_file_over_thirty_seconds_are_refused_and_the_slot_stays() {
    let scratch = Scratch::new("refused");
    let (mut shell, device, _) = shell_with_device(&scratch);
    let good = scratch.path("Good.wav");
    write(&good, &[burst(2000, 1)], 48_000);
    let kept = load(&mut shell, device, &good).media;
    let steps = shell.editor().undo_history().count();

    let three = scratch.path("Three.wav");
    write(
        &three,
        &[burst(500, 1), burst(500, 2), burst(500, 3)],
        48_000,
    );
    let long = scratch.path("Long.wav");
    write(&long, &[burst(31 * 8000, 4)], 8000);
    for file in [&three, &long] {
        let refused = shell
            .run(
                "device.load_sample",
                json!({ "device": device, "path": text(file) }),
            )
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains("1, 2 or 4 channels") || refused.contains("30 seconds"),
            "{refused}"
        );
        assert!(
            shell.message().is_some_and(|m| m.error),
            "the Status Bar says why"
        );
    }
    assert_eq!(shell.editor().undo_history().count(), steps, "no new step");
    let read = responses(&mut shell, device);
    assert_eq!(
        read["slots"][0]["media"],
        json!(kept),
        "the slot is as it was"
    );
}

// Spec: impulse-responses R2, R12
#[test]
fn set_sample_with_no_file_clears_the_slot_as_one_named_step() {
    let scratch = Scratch::new("clear");
    let (mut shell, device, _) = shell_with_device(&scratch);
    let file = scratch.path("A.wav");
    write(&file, &[burst(2000, 1)], 48_000);
    load(&mut shell, device, &file);
    shell
        .run(
            "device.set_sample",
            json!({ "device": device, "media": null, "slot": 0 }),
        )
        .unwrap();
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Clear Impulse Response")
    );
    assert_eq!(responses(&mut shell, device)["slots"][0]["state"], "empty");
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(responses(&mut shell, device)["slots"][0]["state"], "ready");
}

// Spec: impulse-responses R2; freeze-and-bounce R8
#[test]
fn a_frozen_tracks_slot_is_not_cleared() {
    let scratch = Scratch::new("frozen");
    let (mut shell, device, _) = shell_with_device(&scratch);
    let file = scratch.path("A.wav");
    write(&file, &[burst(2000, 1)], 48_000);
    load(&mut shell, device, &file);
    shell
        .run(
            "set.save_as",
            json!({ "path": text(&scratch.path("Song.blset")) }),
        )
        .unwrap();
    let track = shell.editor().document().device_track(device).unwrap();
    shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [text(&file)], "track": track, "at": 0 }),
        )
        .unwrap();
    shell
        .run("track.freeze", json!({ "tracks": [track] }))
        .unwrap();
    assert!(
        shell
            .run(
                "device.set_sample",
                json!({ "device": device, "media": null, "slot": 0 }),
            )
            .is_err(),
        "a frozen track's devices are locked"
    );
    assert!(
        shell
            .editor()
            .document()
            .device(device)
            .unwrap()
            .media_at(0)
            .is_some()
    );
}

/// The engine's registry for `shell`'s set: the test devices, and the shell's
/// media store for the files they name.
fn registry_for(shell: &Shell) -> bl_engine::Registry {
    let mut registry = testing::registry();
    registry.set_media(shell.media_store());
    registry
}

// Spec: impulse-responses R4, R8, R15
#[test]
fn a_response_plays_and_plays_again_after_the_set_is_saved_and_opened() {
    let scratch = Scratch::new("play");
    let (mut shell, device, registry) = shell_with_device(&scratch);
    assert!(
        loudest(&shell, registry_for(&shell), 24) < 1e-6,
        "no response, no sound"
    );
    let file = scratch.path("Room.wav");
    write(&file, &[burst(4000, 5)], 48_000);
    load(&mut shell, device, &file);
    assert!(loudest(&shell, registry, 24) > 0.01, "the response plays");

    let song = scratch.path("Song.blset");
    let saved = shell
        .run("set.save_as", json!({ "path": text(&song) }))
        .unwrap();
    shell
        .run("set.open", json!({ "path": saved["path"] }))
        .unwrap();
    assert_eq!(
        responses(&mut shell, device)["slots"][0]["state"],
        "ready",
        "the reopened set names the response"
    );
    assert!(
        loudest(&shell, registry_for(&shell), 24) > 0.01,
        "the reopened set plays it"
    );
}

// Spec: impulse-responses R8, R9
#[test]
fn a_missing_response_is_listed_and_relinks_by_content() {
    let scratch = Scratch::new("missing");
    let (mut shell, device, _) = shell_with_device(&scratch);
    let file = scratch.path("Elsewhere/Hall.wav");
    write(&file, &[burst(4000, 6)], 48_000);
    load(&mut shell, device, &file);
    let song = scratch.path("Song.blset");
    let saved = shell
        .run("set.save_as", json!({ "path": text(&song) }))
        .unwrap();
    let _ = saved;
    assert_eq!(shell.missing_files(), 0);

    let moved = scratch.path("Moved/Hall.wav");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&file, &moved).unwrap();
    assert_eq!(shell.missing_files(), 1, "Manage Files lists it");
    assert_eq!(
        responses(&mut shell, device)["slots"][0]["state"],
        "missing"
    );
    let steps = shell.editor().undo_history().count();
    let report = shell
        .run(
            "set.relink_missing",
            json!({ "folders": [text(moved.parent().unwrap())] }),
        )
        .unwrap();
    assert_eq!(report["files"], 1, "found by its content: {report}");
    assert_eq!(shell.missing_files(), 0);
    assert_eq!(
        shell.editor().undo_history().count(),
        steps,
        "relinking is no undo step"
    );
    assert_eq!(responses(&mut shell, device)["slots"][0]["state"], "ready");
}

// Spec: impulse-responses R8
#[test]
fn collect_all_and_save_copies_the_response_in_with_the_sets_samples() {
    let scratch = Scratch::new("collect");
    let (mut shell, device, _) = shell_with_device(&scratch);
    let file = scratch.path("Elsewhere/Hall.wav");
    write(&file, &[burst(4000, 6)], 48_000);
    load(&mut shell, device, &file);
    let saved = shell
        .run(
            "set.save_as",
            json!({ "path": text(&scratch.path("Song.blset")) }),
        )
        .unwrap();
    let song = PathBuf::from(saved["path"].as_str().unwrap());
    let report = shell
        .run(
            "set.collect_all_and_save",
            json!({ "groups": ["elsewhere"] }),
        )
        .unwrap();
    assert_eq!(report["files"], 1, "the response is copied in: {report}");
    assert!(
        song.parent()
            .unwrap()
            .join("Samples/Imported/Hall.wav")
            .is_file(),
        "it sits with the set's samples"
    );
    assert_eq!(shell.missing_files(), 0);
}

// Spec: impulse-responses R10, R11, R13
#[test]
fn the_chooser_lists_the_bundled_categories_then_user_and_a_response_is_chosen_by_command() {
    let scratch = Scratch::new("chooser");
    let (mut shell, device, _) = shell_with_device(&scratch);
    let bundled = scratch.path("Bundled");
    for (category, name) in [
        ("Plates", "Bright Plate"),
        ("Halls", "Big Hall"),
        ("Odd", "Odd One"),
    ] {
        write(
            &bundled.join(category).join(format!("{name}.wav")),
            &[burst(2000, 3)],
            48_000,
        );
    }
    shell.use_bundled_impulse_responses(Some(bundled.clone()));
    let library = scratch.path("User Library");
    write(
        &library.join("Impulse Responses/Mine/Cellar.wav"),
        &[burst(2000, 4)],
        48_000,
    );
    shell.set_user_library(Some(library.clone()));
    let elsewhere = scratch.path("Elsewhere/Garage.wav");
    write(&elsewhere, &[burst(2000, 5)], 48_000);
    load(&mut shell, device, &elsewhere);

    let read = responses(&mut shell, device);
    let categories = read["categories"].as_array().unwrap();
    let names: Vec<&str> = categories
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["Halls", "Plates", "Odd", "User"],
        "R17's order, then the rest, then User"
    );
    let user: Vec<&str> = categories[3]["responses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        user,
        ["Cellar", "Garage"],
        "the User Library's, then the set's"
    );
    assert_eq!(read["slots"][0]["category"], "User");

    // Choosing a bundled response reads back with its category and name.
    let plate = categories[1]["responses"][0]["file"]
        .as_str()
        .unwrap()
        .to_owned();
    shell
        .run(
            "device.load_sample",
            json!({ "device": device, "path": plate }),
        )
        .unwrap();
    let read = responses(&mut shell, device);
    assert_eq!(read["slots"][0]["category"], "Plates");
    assert_eq!(read["slots"][0]["response"], "Bright Plate");
}

// Spec: impulse-responses R11. A file the browser loads while the device is
// hot-swapped is a response too.
#[test]
fn a_file_chosen_in_the_browser_loads_into_the_hot_swapped_device() {
    let scratch = Scratch::new("browser");
    let (mut shell, device, _) = shell_with_device(&scratch);
    let file = scratch.path("Folder/Plate.wav");
    write(&file, &[burst(2000, 8)], 48_000);
    shell
        .run("device.hot_swap", json!({ "device": device, "on": true }))
        .unwrap();
    shell
        .run(
            "browser.open_folder",
            json!({ "path": text(file.parent().unwrap()) }),
        )
        .unwrap();
    shell
        .run("browser.select", json!({ "paths": [text(&file)] }))
        .unwrap();
    shell.run("browser.load_selected", json!({})).unwrap();
    assert_eq!(
        responses(&mut shell, device)["slots"][0]["response"],
        "Plate"
    );
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Load Impulse Response")
    );
}

// Spec: impulse-responses R4. The main thread never reads a file's header or
// decodes it, however long it is.
#[test]
fn the_main_thread_never_decodes_a_long_response() {
    let scratch = Scratch::new("thread");
    let (mut shell, device, _) = shell_with_device(&scratch);
    let file = scratch.path("Twenty.wav");
    write(&file, &[burst(20 * 48_000, 9)], 48_000);
    let before = bl_media::probes_on_this_thread();
    load(&mut shell, device, &file);
    assert_eq!(bl_media::probes_on_this_thread(), before);
}

//! The eight-slot drum sampler in the shell (WP-5.17; the eight-slot drum
//! sampler spec, R16 to R18): `device.load_sample` takes a file into one of
//! its slots, `device.set_sample` clears one, each is one undo step, a
//! saved set reopens with its eight samples by hash, and
//! `device.select_slot` picks the slot the panel shows and a sample goes
//! into. Driven through `Shell::run`, as an agent drives it (D17), on real
//! files and bl-project's real set files.

use std::path::{Path, PathBuf};

use bl_devices::kitbag::{KITBAG, SLOTS};
use bl_media::BitDepth;
use bl_model::{ContentHash, DeviceId, DeviceKind, DeviceRole, TrackKind};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::device_commands::{DeviceAdded, LoadSampleResult};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use serde_json::json;

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-kitbag-{name}-{}-{}",
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

/// Writes a short mono WAV file of `frames` frames that isn't silence; the
/// length makes each file's content, and so its hash, its own.
fn wav(path: &Path, frames: usize) {
    let samples: Vec<f32> = (0..frames)
        .map(|i| (i as f32 * 440.0 / 48_000.0 * std::f32::consts::TAU).sin() * 0.25)
        .collect();
    let bytes =
        bl_media::wav_bytes(&samples, 1, 48_000, BitDepth::Int16, bl_media::Dither::None).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn headless_with_config(config: &Path) -> Shell {
    Shell::new(
        Preferences::default(),
        Some(config.to_owned()),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    )
}

/// Adds the drum sampler to the default set's MIDI track, as `device.add`
/// adds one from the browser.
fn add_kitbag(shell: &mut Shell) -> DeviceId {
    let midi = shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Midi)
        .map(bl_model::Track::id)
        .expect("the default set has a MIDI track");
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [midi] }))
        .unwrap();
    let added: DeviceAdded = serde_json::from_value(
        shell
            .run(
                "device.add",
                json!({
                    "device": {
                        "kind": DeviceKind::Native { device_type: KITBAG.to_owned() },
                        "name": "Kitbag",
                        "role": DeviceRole::Instrument,
                    }
                }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
}

/// What `device.load_sample` reports, loading into `slot` if there is one.
fn load(shell: &mut Shell, device: DeviceId, file: &Path, slot: Option<u8>) -> LoadSampleResult {
    let mut args = json!({ "device": device, "path": file.to_string_lossy() });
    if let Some(slot) = slot {
        args["slot"] = json!(slot);
    }
    serde_json::from_value(shell.run("device.load_sample", args).unwrap()).unwrap()
}

/// The sample in each slot of the device.
fn samples(shell: &Shell, device: DeviceId) -> Vec<Option<ContentHash>> {
    shell
        .editor()
        .document()
        .device(device)
        .expect("the drum sampler")
        .media
        .clone()
}

fn last_step(shell: &Shell) -> String {
    shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// The device's panel as the Device View draws it.
fn panel(shell: &Shell, device: DeviceId) -> bl_shell::device_commands::DevicePanel {
    shell
        .device_view_state()
        .devices
        .into_iter()
        .find(|frame| frame.id == device)
        .and_then(|frame| frame.panel)
        .expect("the drum sampler has a panel")
}

// Spec: eight-slot-drum-sampler R16
#[test]
fn a_set_with_eight_samples_reopens_with_them_by_hash() {
    let scratch = Scratch::new("reopen");
    let mut shell = headless_with_config(&scratch.path("config"));
    let device = add_kitbag(&mut shell);
    let mut loaded = Vec::new();
    for slot in 0..SLOTS {
        let file = scratch.path(&format!("Hit {slot}.wav"));
        wav(&file, 2_000 + 100 * slot);
        loaded.push(load(&mut shell, device, &file, Some(slot as u8)));
    }
    let hashes: Vec<Option<ContentHash>> = loaded.iter().map(|l| Some(l.media)).collect();
    assert_eq!(
        samples(&shell, device),
        hashes,
        "each slot has its own file"
    );
    assert!(
        hashes
            .iter()
            .all(|h| hashes.iter().filter(|o| *o == h).count() == 1),
        "and the eight files are eight hashes"
    );
    assert!(
        loaded
            .iter()
            .enumerate()
            .all(|(s, l)| usize::from(l.slot) == s)
    );

    let set = scratch.path("Kit.blset");
    let saved: bl_shell::commands::SetResult = serde_json::from_value(
        shell
            .run("set.save_as", json!({ "path": set.to_string_lossy() }))
            .unwrap(),
    )
    .unwrap();
    let set = PathBuf::from(saved.path.expect("the set was saved"));
    shell
        .run("set.open", json!({ "path": set.to_string_lossy() }))
        .unwrap();
    assert_eq!(
        samples(&shell, device),
        hashes,
        "the set keeps the eight samples"
    );
    // The set's media table points the store at each file again.
    for l in &loaded {
        assert!(
            shell
                .media_store()
                .entry(bl_media::ContentHash::from_bytes(l.media.0))
                .is_some(),
            "slot {}'s file is in the media table",
            l.slot + 1
        );
    }
}

// Spec: eight-slot-drum-sampler R17
#[test]
fn loading_and_clearing_a_slot_are_one_undo_step_each() {
    let scratch = Scratch::new("undo");
    let mut shell = headless_with_config(&scratch.path("config"));
    let device = add_kitbag(&mut shell);
    let (kick, snare) = (scratch.path("Kick.wav"), scratch.path("Snare.wav"));
    wav(&kick, 3_000);
    wav(&snare, 3_500);
    let kick = load(&mut shell, device, &kick, Some(0)).media;
    let steps = shell.editor().undo_history().len();
    let snare = load(&mut shell, device, &snare, Some(3)).media;
    assert_eq!(
        samples(&shell, device),
        [Some(kick), None, None, Some(snare)]
    );
    assert_eq!(shell.editor().undo_history().len(), steps + 1);
    assert_eq!(last_step(&shell), "Load Sample");

    // Clearing slot 4 (Delete on its cell) is one step, and slot 1 stays.
    shell
        .run(
            "device.set_sample",
            json!({ "device": device, "media": null, "slot": 3 }),
        )
        .unwrap();
    assert_eq!(samples(&shell, device), [Some(kick)], "trailing empties go");
    assert_eq!(shell.editor().undo_history().len(), steps + 2);
    assert_eq!(last_step(&shell), "Clear Sample");

    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(
        samples(&shell, device),
        [Some(kick), None, None, Some(snare)]
    );
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(samples(&shell, device), [Some(kick)]);
    shell.run("edit.redo", json!({})).unwrap();
    shell.run("edit.redo", json!({})).unwrap();
    assert_eq!(samples(&shell, device), [Some(kick)]);
}

// Spec: eight-slot-drum-sampler R18
#[test]
fn a_sample_with_no_slot_goes_into_the_selected_one_and_the_panel_says_so() {
    let scratch = Scratch::new("select");
    let mut shell = headless_with_config(&scratch.path("config"));
    let device = add_kitbag(&mut shell);
    let (a, b) = (scratch.path("A.wav"), scratch.path("B.wav"));
    wav(&a, 2_000);
    wav(&b, 2_100);
    assert_eq!(panel(&shell, device).selected_slot, 0, "slot 1 to start");
    assert_eq!(panel(&shell, device).slot_keys.len(), SLOTS);
    assert_eq!(panel(&shell, device).slot_keys[0], "C3");
    assert_eq!(panel(&shell, device).slot_keys[7], "G3");
    let first = load(&mut shell, device, &a, None);
    assert_eq!(first.slot, 0);

    let steps = shell.editor().undo_history().len();
    shell
        .run("device.select_slot", json!({ "device": device, "slot": 5 }))
        .unwrap();
    assert_eq!(
        shell.editor().undo_history().len(),
        steps,
        "selecting a slot is a view change, not an undo step"
    );
    assert_eq!(panel(&shell, device).selected_slot, 5);
    let second = load(&mut shell, device, &b, None);
    assert_eq!(second.slot, 5);
    assert_eq!(
        panel(&shell, device).loaded_slots,
        [true, false, false, false, false, true]
    );
    assert_eq!(
        samples(&shell, device),
        [
            Some(first.media),
            None,
            None,
            None,
            None,
            Some(second.media)
        ]
    );

    // The panel's controls are the selected slot's, and the selection is
    // not saved with the set's document.
    let placed = panel(&shell, device)
        .description
        .layout
        .place_for(Default::default(), 5);
    assert!(
        placed
            .controls
            .iter()
            .any(|c| c.param == bl_devices::kitbag::param(5, bl_devices::kitbag::slot::PAN))
    );

    // There is no ninth slot, and the sampler has only slot 0.
    assert!(
        shell
            .run("device.select_slot", json!({ "device": device, "slot": 8 }))
            .is_err()
    );
    assert!(
        shell
            .run(
                "device.load_sample",
                json!({ "device": device, "path": a.to_string_lossy(), "slot": 8 }),
            )
            .is_err()
    );
}

// Spec: eight-slot-drum-sampler R17
#[test]
fn hot_swapping_the_drum_sampler_loads_the_browsers_audio_file_into_the_selected_slot() {
    let scratch = Scratch::new("hot-swap");
    let mut shell = headless_with_config(&scratch.path("config"));
    let device = add_kitbag(&mut shell);
    let file = scratch.path("Clap.wav");
    wav(&file, 2_400);
    shell
        .invoke(
            "browser.add_folder",
            json!({ "path": scratch.0.to_string_lossy() }),
        )
        .unwrap();
    shell
        .invoke(
            "browser.open_folder",
            json!({ "path": scratch.0.to_string_lossy(), "open": true }),
        )
        .unwrap();

    shell
        .run("device.select_slot", json!({ "device": device, "slot": 3 }))
        .unwrap();
    shell
        .invoke("device.hot_swap", json!({ "device": device, "on": true }))
        .unwrap();
    shell
        .invoke(
            "browser.select",
            json!({ "paths": [file.to_string_lossy()] }),
        )
        .unwrap();
    shell.set_focused_area(bl_shell::Area::Browser);
    assert!(shell.can_invoke("browser.load_selected", &json!({})));
    shell.invoke("browser.load_selected", json!({})).unwrap();
    let held = samples(&shell, device);
    assert_eq!(held.len(), 4, "{held:?}");
    assert!(held[3].is_some() && held[..3].iter().all(Option::is_none));
    assert_eq!(last_step(&shell), "Load Sample");
}

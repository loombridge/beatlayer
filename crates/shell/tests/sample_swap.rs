//! Similar sample swapping (WP-9.10; spec: sound-similarity R20 to R28,
//! one-shot-drum-sampler R26 to R28, drum-racks R30 and R31), driven headless
//! through `Shell::run` and `Shell::invoke`, as agents and keys drive it: a
//! Simpler, a Drum Sampler and a Drum Rack pad step through the list of
//! samples like their reference, one undo step a swap, keeping every
//! parameter and resetting the region; the reference is kept, returned to,
//! and saved with the set. Analysis is paced by hand, so no test waits on
//! the clock.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bl_devices::DRUM_SAMPLER;
use bl_devices::sampler::SAMPLER;
use bl_model::{ChainId, ContentHash, DeviceId, DeviceKind, DeviceRole, TrackKind};
use bl_shell::device_commands::{DeviceAdded, SimilarState, Swapped};
use bl_shell::pad_commands::PadDropped;
use bl_shell::settings::Preferences;
use bl_shell::{Area, Shell};
use serde_json::{Value, json};

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-swap-{}-{name}", std::process::id()));
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

/// Writes a 16-bit mono WAV of a decaying tone of `hz`, `seconds` long.
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
    std::fs::write(path, bytes).unwrap();
}

/// Four kicks and three hi-hats, in a folder that is the User Library.
fn place(scratch: &Scratch) -> PathBuf {
    let library = scratch.path("Library");
    std::fs::create_dir_all(&library).unwrap();
    for (name, hz) in [
        ("Kick 50", 50.0),
        ("Kick 55", 55.0),
        ("Kick 60", 60.0),
        ("Kick 65", 65.0),
    ] {
        wav(&library.join(format!("{name}.wav")), hz, 0.4, 22_050);
    }
    for (name, hz) in [
        ("Hat 6k", 6_000.0),
        ("Hat 7k", 7_000.0),
        ("Hat 8k", 8_000.0),
    ] {
        wav(&library.join(format!("{name}.wav")), hz, 0.12, 22_050);
    }
    library
}

/// A shell on the null device with `library` as its User Library, indexed,
/// its seven files analysed unless `analyse` says not.
fn shell_on(scratch: &Scratch, analyse: bool) -> (Shell, PathBuf) {
    let library = place(scratch);
    let mut shell = Shell::new(
        Preferences::default(),
        Some(scratch.path("config")),
        bl_shell::audio::AudioHost::null(Default::default()),
        Box::new(bl_shell::files::ProjectFiles::new()),
    );
    shell.pace_analysis_by_hand();
    shell.set_user_library(Some(library.clone()));
    assert!(shell.wait_for_browser_index(Duration::from_secs(60)));
    if analyse {
        shell.release_analysis(7);
    }
    let midi = shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Midi)
        .map(bl_model::Track::id)
        .unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [midi] }))
        .unwrap();
    shell.set_focused_area(Area::Detail);
    (shell, library)
}

fn hash_of(path: &Path) -> ContentHash {
    ContentHash(*bl_media::ContentHash::of_file(path).unwrap().as_bytes())
}

fn run<T: serde::de::DeserializeOwned>(shell: &mut Shell, id: &str, args: Value) -> T {
    serde_json::from_value(
        shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}")),
    )
    .unwrap()
}

/// Adds a device of `device_type` to the shown track, selected.
fn add(shell: &mut Shell, device_type: &str, name: &str) -> DeviceId {
    add_as(shell, device_type, name, DeviceRole::Instrument)
}

fn add_as(shell: &mut Shell, device_type: &str, name: &str, role: DeviceRole) -> DeviceId {
    let added: DeviceAdded = run(
        shell,
        "device.add",
        json!({ "device": {
            "kind": DeviceKind::Native { device_type: device_type.to_owned() },
            "name": name,
            "role": role,
        } }),
    );
    added.device
}

/// A new MIDI track, which is the shown one: a track holds one instrument.
fn fresh_track(shell: &mut Shell) {
    shell
        .invoke("track.create", json!({ "kind": "midi" }))
        .unwrap();
    let track = shell
        .editor()
        .document()
        .tracks()
        .filter(|track| track.kind() == TrackKind::Midi)
        .map(bl_model::Track::id)
        .last()
        .unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
}

/// A device of each swappable type, the sample of `file` loaded into it.
fn loaded(shell: &mut Shell, device_type: &str, file: &Path) -> DeviceId {
    let device = add(shell, device_type, "Sampler");
    run::<Value>(
        shell,
        "device.load_sample",
        json!({ "device": device, "path": file.to_string_lossy() }),
    );
    device
}

fn state(shell: &mut Shell, device: DeviceId) -> SimilarState {
    run(shell, "device.similar", json!({ "device": device }))
}

fn sample(shell: &Shell, device: DeviceId) -> Option<ContentHash> {
    shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .media_at(0)
}

fn last_step(shell: &Shell) -> String {
    shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// The names of the reference's similar list, the reference first.
fn list(shell: &mut Shell, library: &Path, name: &str) -> Vec<String> {
    let found: bl_shell::browser_commands::SimilarList = run(
        shell,
        "browser.similar",
        json!({ "path": library.join(format!("{name}.wav")).to_string_lossy() }),
    );
    found
        .items
        .iter()
        .map(|item| {
            Path::new(&item.path)
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

fn name_of(shell: &Shell, media: ContentHash) -> String {
    let entry = shell
        .media_store()
        .entry(bl_media::ContentHash::from_bytes(media.0))
        .unwrap();
    entry
        .path
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

const DEVICES: [(&str, &str); 2] = [(SAMPLER, "Simpler"), (DRUM_SAMPLER, "Drum Sampler")];

// Spec: sound-similarity R21, R22, R28; one-shot-drum-sampler R26, R28. The
// next and previous swaps step through the reference's list, each one undo
// step named Swap Sample, with every parameter kept and the region reset;
// past either end the command is refused.
#[test]
fn swaps_step_through_the_similar_list_in_one_undo_step_each() {
    for (device_type, name) in DEVICES {
        let scratch = Scratch::new(&format!("steps-{name}").replace(' ', "-"));
        let (mut shell, library) = shell_on(&scratch, true);
        let expected = list(&mut shell, &library, "Kick 50");
        assert!(expected.len() >= 3, "{expected:?}");
        let device = loaded(&mut shell, device_type, &library.join("Kick 50.wav"));
        let volume = if device_type == SAMPLER {
            bl_devices::sampler::VOLUME
        } else {
            bl_devices::drum_sampler::VOLUME
        };
        run::<Value>(
            &mut shell,
            "device.set_param",
            json!({ "device": device, "param": volume.0, "value": -6.0 }),
        );
        if device_type == SAMPLER {
            run::<Value>(
                &mut shell,
                "device.set_sample_region",
                json!({ "device": device, "start": 100, "end": 3_000 }),
            );
            run::<Value>(
                &mut shell,
                "device.add_slice",
                json!({ "device": device, "frame": 500 }),
            );
        }
        let before = shell.editor().document().device(device).unwrap().clone();
        let start = state(&mut shell, device);
        assert_eq!(start.sample, Some(hash_of(&library.join("Kick 50.wav"))));
        assert_eq!(start.reference, start.sample, "{name}: the loaded sample");
        assert_eq!(start.position, Some(0));
        assert_eq!(start.count, Some(expected.len() as u32));
        assert!(start.can_next && !start.can_previous && !start.can_return);

        // The previous of the reference is refused: it is the first.
        let refused = shell.run(
            "device.swap_similar",
            json!({ "device": device, "direction": "previous" }),
        );
        assert!(refused.is_err(), "{name}: before the first");

        let swapped: Swapped = run(
            &mut shell,
            "device.swap_similar",
            json!({ "device": device, "direction": "next" }),
        );
        assert_eq!(swapped.name, expected[1], "{name}");
        assert_eq!(
            (swapped.position, swapped.count),
            (1, expected.len() as u32)
        );
        assert_eq!(last_step(&shell), "Swap Sample");
        let after = shell.editor().document().device(device).unwrap().clone();
        assert_eq!(after.media_at(0), Some(swapped.media));
        assert_eq!(after.params, before.params, "{name}: every parameter");
        assert_eq!(after.region, None, "{name}: the region is the whole");
        assert!(
            after.slices.is_empty(),
            "{name}: slices are worked out again"
        );
        assert_eq!(
            after.similarity_reference,
            before.media_at(0),
            "{name}: the reference stays the loaded sample"
        );
        let there = state(&mut shell, device);
        assert_eq!(there.position, Some(1));
        assert!(there.can_previous && there.can_return);

        // Undo takes the whole swap back: the sample, the region, the slices.
        shell.run("edit.undo", json!({})).unwrap();
        let undone = shell.editor().document().device(device).unwrap().clone();
        assert_eq!(undone, before, "{name}: one step undoes all of it");
        shell.run("edit.redo", json!({})).unwrap();

        // Previous goes back to the reference; the last one stops the list.
        let back: Swapped = run(
            &mut shell,
            "device.swap_similar",
            json!({ "device": device, "direction": "previous" }),
        );
        assert_eq!(back.name, "Kick 50", "{name}");
        for expected in &expected[1..] {
            let next: Swapped = run(
                &mut shell,
                "device.swap_similar",
                json!({ "device": device, "direction": "next" }),
            );
            assert_eq!(&next.name, expected, "{name}");
        }
        let refused = shell.run(
            "device.swap_similar",
            json!({ "device": device, "direction": "next" }),
        );
        assert!(refused.is_err(), "{name}: past the last");
        let last = state(&mut shell, device);
        assert!(!last.can_next && last.can_previous, "{name}");
    }
}

// Spec: sound-similarity R23, R24. Keep the sample as the reference, swap
// away, and go back: Return to Reference is one undo step, and refused while
// the device holds the reference.
#[test]
fn the_reference_is_kept_and_returned_to() {
    for (device_type, name) in DEVICES {
        let scratch = Scratch::new(&format!("keep-{name}").replace(' ', "-"));
        let (mut shell, library) = shell_on(&scratch, true);
        let device = loaded(&mut shell, device_type, &library.join("Kick 50.wav"));
        assert!(
            shell
                .run("device.return_to_reference", json!({ "device": device }))
                .is_err(),
            "{name}: it holds the reference"
        );
        let first: Swapped = run(
            &mut shell,
            "device.swap_similar",
            json!({ "device": device, "direction": "next" }),
        );
        // Keep this one as the reference: the list is now like it.
        run::<Value>(
            &mut shell,
            "device.set_similarity_reference",
            json!({ "device": device }),
        );
        assert_eq!(last_step(&shell), "Save as Similarity Reference");
        let kept = state(&mut shell, device);
        assert_eq!(kept.reference, Some(first.media), "{name}");
        assert!(!kept.can_return, "{name}: it holds the reference now");
        let expected = list(&mut shell, &library, &first.name);
        assert_eq!(kept.count, Some(expected.len() as u32), "{name}");
        assert_eq!(kept.position, Some(0), "{name}: the reference is first");

        let second: Swapped = run(
            &mut shell,
            "device.swap_similar",
            json!({ "device": device, "direction": "next" }),
        );
        assert_eq!(second.name, expected[1], "{name}");
        let returned: Swapped = run(
            &mut shell,
            "device.return_to_reference",
            json!({ "device": device }),
        );
        assert_eq!(returned.media, first.media, "{name}");
        assert_eq!(last_step(&shell), "Return to Reference");
        assert_eq!(sample(&shell, device), Some(first.media));
        assert_eq!(state(&mut shell, device).position, Some(0));
        shell.run("edit.undo", json!({})).unwrap();
        assert_eq!(sample(&shell, device), Some(second.media), "{name}");
    }
}

// Spec: sound-similarity R23; one-shot-drum-sampler R28. The reference
// survives a save and a reload of the set, and rides in the device's preset.
#[test]
fn the_reference_survives_a_save_and_a_reload() {
    for (device_type, name) in DEVICES {
        let scratch = Scratch::new(&format!("save-{name}").replace(' ', "-"));
        let (mut shell, library) = shell_on(&scratch, true);
        let device = loaded(&mut shell, device_type, &library.join("Kick 50.wav"));
        let swapped: Swapped = run(
            &mut shell,
            "device.swap_similar",
            json!({ "device": device, "direction": "next" }),
        );
        let reference = Some(hash_of(&library.join("Kick 50.wav")));
        let set = scratch.path("Song.blset");
        let saved: bl_shell::commands::SetResult = run(
            &mut shell,
            "set.save_as",
            json!({ "path": set.to_string_lossy() }),
        );
        shell
            .run("set.open", json!({ "path": saved.path.expect("saved") }))
            .unwrap();
        let reopened = shell.editor().document().device(device).unwrap().clone();
        assert_eq!(reopened.media_at(0), Some(swapped.media), "{name}");
        assert_eq!(reopened.similarity_reference, reference, "{name}");
        // The query tool's device reads the sample and the reference back.
        let there = state(&mut shell, device);
        assert_eq!(there.reference, reference, "{name}");
        assert_eq!(there.position, Some(1), "{name}");

        // A preset keeps it.
        let preset = bl_project::preset::Preset::of(&reopened);
        assert_eq!(preset.similarity_reference, reference, "{name}");
        let bytes = preset.encode();
        let decoded = bl_project::preset::Preset::decode(&bytes).unwrap();
        assert_eq!(decoded.similarity_reference, reference, "{name}");
    }
}

// Spec: sound-similarity R27. A sample that hasn't been analysed has no
// list: the commands are refused with the reason, and the state says why.
#[test]
fn a_reference_that_isnt_analysed_refuses_the_swap_with_the_reason() {
    let scratch = Scratch::new("unanalysed");
    let (mut shell, library) = shell_on(&scratch, false);
    let device = loaded(&mut shell, SAMPLER, &library.join("Kick 50.wav"));
    let there = state(&mut shell, device);
    assert!(!there.can_next && !there.can_previous && !there.can_return);
    let reason = there.reason.expect("a reason");
    assert!(reason.contains("analysed"), "{reason}");
    let refused = shell
        .run(
            "device.swap_similar",
            json!({ "device": device, "direction": "next" }),
        )
        .unwrap_err()
        .to_string();
    assert!(refused.contains("analysed"), "{refused}");
    // A sampler holding none has no reference.
    fresh_track(&mut shell);
    let empty = add(&mut shell, SAMPLER, "Simpler");
    assert_eq!(state(&mut shell, empty).reference, None);
    assert!(
        shell
            .run(
                "device.swap_similar",
                json!({ "device": empty, "direction": "next" })
            )
            .is_err()
    );
}

// Spec: sound-similarity R20. The multisampler and the eight-slot drum
// sampler don't swap, and nor does any other device.
#[test]
fn only_simpler_and_drum_sampler_swap() {
    let scratch = Scratch::new("others");
    let (mut shell, _) = shell_on(&scratch, true);
    for (device_type, role) in [
        (
            bl_devices::multisampler::MULTISAMPLER,
            DeviceRole::Instrument,
        ),
        (bl_devices::kitbag::KITBAG, DeviceRole::Instrument),
        (bl_devices::utility::UTILITY, DeviceRole::AudioEffect),
    ] {
        fresh_track(&mut shell);
        let device = add_as(&mut shell, device_type, "Other", role);
        assert!(
            shell
                .run(
                    "device.swap_similar",
                    json!({ "device": device, "direction": "next" })
                )
                .is_err(),
            "{device_type}"
        );
        assert!(
            shell
                .run("device.return_to_reference", json!({ "device": device }))
                .is_err()
        );
    }
}

/// A drum rack with a pad on `note` playing `file`, in a Simpler or a Drum
/// Sampler (the chain's own sampler is Simpler, as a drop makes it).
fn pad(shell: &mut Shell, file: &Path, note: u8) -> (DeviceId, ChainId, DeviceId) {
    let rack: DeviceAdded = run(
        shell,
        "device.add",
        json!({ "device": {
            "kind": { "rack": { "kind": "drum" } },
            "name": "Drum Rack",
            "role": "instrument",
        } }),
    );
    let dropped: PadDropped = run(
        shell,
        "pad.drop",
        json!({ "rack": rack.device, "note": note, "samples": [file.to_string_lossy()] }),
    );
    let chain = dropped.chains[0];
    let sampler = shell
        .editor()
        .document()
        .chain(chain)
        .unwrap()
        .devices
        .iter()
        .find(|d| !d.media.is_empty())
        .map(|d| d.id)
        .unwrap();
    (rack.device, chain, sampler)
}

// Spec: drum-racks R30, R31; sound-similarity R22, R24. A pad swaps the
// sample of its sampler: its notes, choke group, chain mixer and the
// sampler's parameters stay, the chain's name follows the sample's when it
// was the old sample's, and the keys act on the selected pad.
#[test]
fn a_pad_swaps_its_sample_and_keeps_everything_else() {
    let scratch = Scratch::new("pad");
    let (mut shell, library) = shell_on(&scratch, true);
    let expected = list(&mut shell, &library, "Kick 50");
    let (rack, chain, sampler) = pad(&mut shell, &library.join("Kick 50.wav"), 36);
    assert_eq!(
        shell.editor().document().chain(chain).unwrap().name,
        "Kick 50"
    );
    run::<Value>(
        &mut shell,
        "chain.set_choke",
        json!({ "chain": chain, "choke": 3 }),
    );
    run::<Value>(
        &mut shell,
        "device.set_param",
        json!({ "device": sampler, "param": bl_devices::sampler::VOLUME.0, "value": -9.0 }),
    );
    let before = shell.editor().document().chain(chain).unwrap().clone();
    let device_before = shell.editor().document().device(sampler).unwrap().clone();

    // By the pad's note.
    let swapped: Swapped = run(
        &mut shell,
        "device.swap_similar",
        json!({ "rack": rack, "note": 36, "direction": "next" }),
    );
    assert_eq!(swapped.device, sampler);
    assert_eq!(swapped.name, expected[1]);
    let after = shell.editor().document().chain(chain).unwrap().clone();
    assert_eq!(after.name, expected[1], "the name followed the sample's");
    assert_eq!(
        (after.receive, after.play, after.choke),
        (before.receive, before.play, before.choke)
    );
    assert_eq!(after.choke, Some(3));
    assert_eq!(
        shell.editor().document().device(sampler).unwrap().params,
        device_before.params
    );
    assert_eq!(last_step(&shell), "Swap Sample");
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(
        shell.editor().document().chain(chain).unwrap().name,
        "Kick 50",
        "the name went back with the sample"
    );

    // With the pad selected, the key's command has no device to name.
    shell
        .invoke("pad.select", json!({ "rack": rack, "note": 36 }))
        .unwrap();
    assert!(shell.can_invoke("device.swap_similar", &json!({ "direction": "next" })));
    shell
        .invoke("device.swap_similar", json!({ "direction": "next" }))
        .unwrap();
    assert_eq!(
        name_of(&shell, sample(&shell, sampler).unwrap()),
        expected[1]
    );
    // A chain a person has renamed keeps its name (R31).
    run::<Value>(
        &mut shell,
        "pad.rename",
        json!({ "rack": rack, "note": 36, "name": "Big Kick" }),
    );
    shell
        .invoke("device.swap_similar", json!({ "direction": "next" }))
        .unwrap();
    assert_eq!(
        shell.editor().document().chain(chain).unwrap().name,
        "Big Kick"
    );
    // Keep and return, by key, on the pad.
    shell
        .invoke("device.set_similarity_reference", json!({}))
        .unwrap();
    assert_eq!(
        shell
            .editor()
            .document()
            .device(sampler)
            .unwrap()
            .similarity_reference,
        sample(&shell, sampler)
    );
    shell
        .invoke("device.swap_similar", json!({ "direction": "next" }))
        .unwrap();
    shell
        .invoke("device.return_to_reference", json!({}))
        .unwrap();
    assert_eq!(
        shell
            .editor()
            .document()
            .device(sampler)
            .unwrap()
            .similarity_reference,
        sample(&shell, sampler)
    );

    // An empty pad has no sampler: the keys have nothing to act on.
    shell
        .invoke("pad.select", json!({ "rack": rack, "note": 40 }))
        .unwrap();
    assert!(!shell.can_invoke("device.swap_similar", &json!({ "direction": "next" })));
    assert!(
        shell
            .run(
                "device.swap_similar",
                json!({ "rack": rack, "note": 40, "direction": "next" })
            )
            .is_err()
    );
}

// Spec: sound-similarity R24. The keys act on the selected sampler.
#[test]
fn the_keys_act_on_the_selected_sampler() {
    let scratch = Scratch::new("keys");
    let (mut shell, library) = shell_on(&scratch, true);
    let device = loaded(&mut shell, SAMPLER, &library.join("Kick 50.wav"));
    shell
        .invoke("device.select", json!({ "device": device }))
        .unwrap();
    assert!(shell.can_invoke("device.swap_similar", &json!({ "direction": "next" })));
    shell
        .invoke("device.swap_similar", json!({ "direction": "next" }))
        .unwrap();
    assert_ne!(
        sample(&shell, device),
        Some(hash_of(&library.join("Kick 50.wav")))
    );
    shell
        .invoke("device.set_similarity_reference", json!({}))
        .unwrap();
    assert_eq!(
        shell
            .editor()
            .document()
            .device(device)
            .unwrap()
            .similarity_reference,
        sample(&shell, device)
    );
    let other = add_as(
        &mut shell,
        bl_devices::utility::UTILITY,
        "Utility",
        DeviceRole::AudioEffect,
    );
    shell
        .invoke("device.select", json!({ "device": other }))
        .unwrap();
    assert!(!shell.can_invoke("device.swap_similar", &json!({ "direction": "next" })));
}

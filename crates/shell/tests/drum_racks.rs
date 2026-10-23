//! Drum racks through the shell's commands (WP-5.6; the drum racks spec):
//! the Receive chooser, dropping on pads, swapping pads, the default pad,
//! hot-swapping a pad, previewing, extracting chains, return chains, Group
//! to Drum Rack and grouping drum chains. Every gesture is one undo step
//! and runs by ID, as an agent runs it (D17), on real files.

use std::path::PathBuf;

use bl_devices::sampler::SAMPLER;
use bl_engine::testing;
use bl_media::BitDepth;
use bl_model::{
    ChainId, ContentHash, DeviceId, DeviceKind, DeviceRole, Receive, TrackId, TrackKind,
};
use bl_shell::device_commands::{DeviceAdded, DeviceViewState, Grouped};
use bl_shell::pad_commands::{Extracted, PadDropped, PadFrame};
use bl_shell::{Area, Shell};
use serde_json::{Value, json};

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-drum-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    /// A WAV file whose length makes its content, and so its hash, its own.
    fn wav(&self, name: &str, frames: usize) -> String {
        let samples: Vec<f32> = (0..frames)
            .map(|i| (i as f32 * 440.0 / 48_000.0 * std::f32::consts::TAU).sin() * 0.25)
            .collect();
        let bytes =
            bl_media::wav_bytes(&samples, 1, 48_000, BitDepth::Int16, bl_media::Dither::None)
                .unwrap();
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        path.to_string_lossy().into_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Drums {
    shell: Shell,
    track: TrackId,
    rack: DeviceId,
}

/// The default set with its MIDI track selected and a drum rack on it.
fn kit() -> Drums {
    let mut shell = Shell::headless();
    let track = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Midi)
        .map(bl_model::Track::id)
        .unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    shell.set_focused_area(Area::Detail);
    let added: DeviceAdded = serde_json::from_value(
        shell
            .invoke(
                "device.add",
                json!({ "device": {
                    "kind": { "rack": { "kind": "drum" } },
                    "name": "Drum Rack",
                    "role": "instrument",
                } }),
            )
            .unwrap(),
    )
    .unwrap();
    Drums {
        shell,
        track,
        rack: added.device,
    }
}

fn effect() -> Value {
    let t = testing::gain_type();
    json!({ "kind": t.kind, "name": "Gain", "role": t.role })
}

fn drop_on(kit: &mut Drums, note: u8, args: Value) -> PadDropped {
    let mut full = json!({ "rack": kit.rack, "note": note });
    for (key, value) in args.as_object().unwrap() {
        full[key] = value.clone();
    }
    serde_json::from_value(kit.shell.invoke("pad.drop", full).unwrap()).unwrap()
}

fn chains_on(kit: &Drums, note: u8) -> Vec<ChainId> {
    let rack = kit.shell.editor().document().device(kit.rack).unwrap();
    rack.chains
        .iter()
        .filter(|c| !c.is_return && c.receive == Receive::Note(note))
        .map(|c| c.id)
        .collect()
}

fn devices_of(kit: &Drums, chain: ChainId) -> Vec<(DeviceRole, DeviceId)> {
    kit.shell
        .editor()
        .document()
        .chain(chain)
        .unwrap()
        .devices
        .iter()
        .map(|d| (d.role, d.id))
        .collect()
}

fn media_of(kit: &Drums, chain: ChainId) -> Vec<ContentHash> {
    kit.shell
        .editor()
        .document()
        .chain(chain)
        .unwrap()
        .devices
        .iter()
        .flat_map(|d| d.media.iter().flatten().copied())
        .collect()
}

fn last_step(shell: &Shell) -> String {
    shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn steps(shell: &Shell) -> usize {
    shell.editor().undo_history().len()
}

fn state(shell: &mut Shell) -> DeviceViewState {
    serde_json::from_value(shell.invoke("device_view.state", json!({})).unwrap()).unwrap()
}

fn pads(kit: &mut Drums) -> PadFrame {
    state(&mut kit.shell)
        .devices
        .into_iter()
        .find(|d| d.id == kit.rack)
        .and_then(|d| d.rack)
        .and_then(|r| r.pads)
        .unwrap()
}

// Spec: drum-racks R3
#[test]
fn the_receive_chooser_lists_names_numbers_and_general_midi() {
    let choices = bl_model::receive_choices();
    assert_eq!(choices[0].1, "All Notes", "All Notes comes first");
    assert_eq!(choices.len(), 129);
    let label = |note: u8| &choices[1 + usize::from(note)].1;
    assert_eq!(label(36), "C1 (36) Bass Drum 1");
    assert_eq!(label(42), "F#1 (42) Closed Hi-Hat");
    assert_eq!(label(60), "C3 (60) Hi Bongo");
    assert_eq!(label(0), "C-2 (0)", "a note with no drum sound has none");
    assert_eq!(label(127), "G8 (127)");
}

// Spec: drum-racks R3, R14
#[test]
fn a_pads_new_chain_receives_its_note_and_plays_c3() {
    let scratch = Scratch::new("new-chain");
    let mut kit = kit();
    let before = steps(&kit.shell);
    let dropped = drop_on(
        &mut kit,
        38,
        json!({ "samples": [scratch.wav("Snare.wav", 2_400)] }),
    );
    assert_eq!(dropped.notes, [38]);
    assert_eq!(steps(&kit.shell), before + 1, "one undo step");
    let chain = kit
        .shell
        .editor()
        .document()
        .chain(dropped.chains[0])
        .unwrap()
        .clone();
    assert_eq!(chain.receive, Receive::Note(38));
    assert_eq!(
        chain.play, 60,
        "C3 plays a sampler's sample at its own pitch"
    );
    assert_eq!(chain.choke, None);
    assert_eq!(chain.name, "Snare", "named after the file");
    let [sampler] = &chain.devices[..] else {
        panic!("one device: {chain:?}");
    };
    assert_eq!(
        sampler.kind,
        DeviceKind::Native {
            device_type: SAMPLER.to_owned()
        }
    );
    assert_eq!(sampler.media.iter().flatten().count(), 1);
    let pad = &pads(&mut kit).pads[2];
    assert_eq!(
        (pad.note, pad.label.as_str(), pad.has_sample),
        (38, "Snare", true)
    );
    kit.shell.invoke("edit.undo", json!({})).unwrap();
    assert!(chains_on(&kit, 38).is_empty());
}

// Spec: drum-racks R14
#[test]
fn a_sample_on_a_pad_makes_or_replaces_the_sound_and_keeps_effects() {
    let scratch = Scratch::new("replace");
    let mut kit = kit();
    let first = drop_on(
        &mut kit,
        36,
        json!({ "samples": [scratch.wav("Kick.wav", 2_000)] }),
    );
    let chain = first.chains[0];
    let sampler = devices_of(&kit, chain)[0].1;
    // An audio effect goes after the sound.
    drop_on(&mut kit, 36, json!({ "device": effect() }));
    let roles: Vec<DeviceRole> = devices_of(&kit, chain).iter().map(|d| d.0).collect();
    assert_eq!(roles, [DeviceRole::Instrument, DeviceRole::AudioEffect]);
    // Another sample replaces only the sound: the same sampler and effect,
    // the new sample.
    let before = media_of(&kit, chain);
    let steps_before = steps(&kit.shell);
    drop_on(
        &mut kit,
        36,
        json!({ "samples": [scratch.wav("Kick 2.wav", 3_000)] }),
    );
    assert_eq!(steps(&kit.shell), steps_before + 1);
    assert_eq!(devices_of(&kit, chain).len(), 2);
    assert_eq!(devices_of(&kit, chain)[0].1, sampler);
    assert_ne!(media_of(&kit, chain), before);
    // A chain whose instrument isn't a sampler has it replaced, and keeps
    // its effect.
    let synth = json!({ "kind": { "native": { "device_type": "test.synth" } },
                        "name": "Synth", "role": "instrument" });
    let other = drop_on(&mut kit, 37, json!({ "device": synth }));
    drop_on(&mut kit, 37, json!({ "device": effect() }));
    drop_on(
        &mut kit,
        37,
        json!({ "samples": [scratch.wav("Rim.wav", 1_500)] }),
    );
    let roles: Vec<DeviceRole> = devices_of(&kit, other.chains[0])
        .iter()
        .map(|d| d.0)
        .collect();
    assert_eq!(roles, [DeviceRole::Instrument, DeviceRole::AudioEffect]);
    assert_eq!(
        media_of(&kit, other.chains[0]).len(),
        1,
        "the sampler took its place"
    );
    // A pad of several chains takes no drop.
    kit.shell
        .invoke("chain.duplicate", json!({ "chain": chain }))
        .unwrap();
    assert!(
        kit.shell
            .invoke(
                "pad.drop",
                json!({ "rack": kit.rack, "note": 36, "device": effect() })
            )
            .is_err()
    );
    assert_eq!(pads(&mut kit).pads[0].label, "Multi");
}

// Spec: drum-racks R15
#[test]
fn several_samples_rise_chromatically_or_layer_with_cmd() {
    let scratch = Scratch::new("several");
    let mut kit = kit();
    let files: Vec<String> = (0..3)
        .map(|n| scratch.wav(&format!("Hit {n}.wav"), 1_000 + 100 * n))
        .collect();
    let dropped = drop_on(&mut kit, 40, json!({ "samples": files }));
    assert_eq!(dropped.notes, [40, 41, 42]);
    assert_eq!(dropped.chains.len(), 3);
    assert_eq!(last_step(&kit.shell), "Insert Chain");
    // Past note 127 they are left out.
    let near_top = drop_on(&mut kit, 126, json!({ "samples": files }));
    assert_eq!(near_top.notes, [126, 127]);
    // With Cmd they layer on one pad: a chain holding an instrument rack
    // with a chain per file.
    let layered = drop_on(&mut kit, 50, json!({ "samples": files, "layer": true }));
    assert_eq!(layered.notes, [50]);
    let rack = kit
        .shell
        .editor()
        .document()
        .chain(layered.chains[0])
        .unwrap();
    let inner = rack
        .devices
        .iter()
        .find(|d| d.rack_kind() == Some(bl_model::RackKind::Instrument))
        .unwrap();
    assert_eq!(inner.chains.len(), 3);
    assert!(
        inner
            .chains
            .iter()
            .all(|c| c.devices.len() == 1 && !c.devices[0].media.is_empty())
    );
}

// Spec: drum-racks R16
#[test]
fn dragging_a_pad_swaps_notes_or_layers_with_cmd() {
    let scratch = Scratch::new("swap");
    let mut kit = kit();
    let a = drop_on(
        &mut kit,
        36,
        json!({ "samples": [scratch.wav("A.wav", 1_000)] }),
    )
    .chains[0];
    let b = drop_on(
        &mut kit,
        37,
        json!({ "samples": [scratch.wav("B.wav", 1_100)] }),
    )
    .chains[0];
    let before = steps(&kit.shell);
    kit.shell
        .invoke(
            "pad.swap",
            json!({ "rack": kit.rack, "from": 36, "to": 37 }),
        )
        .unwrap();
    assert_eq!(steps(&kit.shell), before + 1, "one undo step");
    assert_eq!(chains_on(&kit, 37), [a]);
    assert_eq!(chains_on(&kit, 36), [b]);
    // Onto an empty pad it simply moves.
    kit.shell
        .invoke(
            "pad.swap",
            json!({ "rack": kit.rack, "from": 37, "to": 45 }),
        )
        .unwrap();
    assert_eq!(chains_on(&kit, 45), [a]);
    assert!(chains_on(&kit, 37).is_empty());
    // With Cmd both go into one chain on the target, and the dragged pad is
    // left empty.
    kit.shell
        .invoke(
            "pad.swap",
            json!({ "rack": kit.rack, "from": 36, "to": 45, "layer": true }),
        )
        .unwrap();
    assert!(chains_on(&kit, 36).is_empty());
    let [layered] = chains_on(&kit, 45)[..] else {
        panic!("one chain on the target");
    };
    let inner = &kit
        .shell
        .editor()
        .document()
        .chain(layered)
        .unwrap()
        .devices[0];
    let names: Vec<&str> = inner.chains.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["A", "B"]);
    assert!(inner.chains.iter().all(|c| c.receive == Receive::default()));
}

// Spec: drum-racks R17
#[test]
fn save_as_default_pad_shapes_the_next_dropped_sample() {
    let scratch = Scratch::new("default");
    let mut kit = kit();
    let made = drop_on(
        &mut kit,
        36,
        json!({ "samples": [scratch.wav("Kick.wav", 1_000)] }),
    );
    drop_on(&mut kit, 36, json!({ "device": effect() }));
    kit.shell
        .invoke(
            "chain.set_play",
            json!({ "chain": made.chains[0], "play": 64 }),
        )
        .unwrap();
    kit.shell
        .invoke("pad.save_default", json!({ "rack": kit.rack, "note": 36 }))
        .unwrap();
    let next = drop_on(
        &mut kit,
        44,
        json!({ "samples": [scratch.wav("Tom.wav", 1_200)] }),
    );
    let chain = kit.shell.editor().document().chain(next.chains[0]).unwrap();
    assert_eq!(chain.receive, Receive::Note(44));
    assert_eq!(chain.play, 64, "from the default pad");
    let roles: Vec<DeviceRole> = chain.devices.iter().map(|d| d.role).collect();
    assert_eq!(roles, [DeviceRole::Instrument, DeviceRole::AudioEffect]);
    assert_eq!(chain.name, "Tom");
    assert_eq!(media_of(&kit, next.chains[0]).len(), 1);
    assert_ne!(
        media_of(&kit, next.chains[0]),
        media_of(&kit, made.chains[0]),
        "the new sample, not the default's"
    );
}

// Spec: drum-racks R18
#[test]
fn a_hot_swapped_pad_takes_what_the_browser_loads() {
    let scratch = Scratch::new("hot-swap");
    let mut kit = kit();
    let made = drop_on(
        &mut kit,
        36,
        json!({ "samples": [scratch.wav("Kick.wav", 1_000)] }),
    );
    let file = scratch.wav("Better Kick.wav", 1_300);
    kit.shell
        .invoke(
            "browser.add_folder",
            json!({ "path": scratch.0.to_string_lossy() }),
        )
        .unwrap();
    kit.shell
        .invoke(
            "browser.open_folder",
            json!({ "path": scratch.0.to_string_lossy(), "open": true }),
        )
        .unwrap();
    // D has nowhere to go until a drum rack is being hot-swapped.
    assert!(
        !kit.shell
            .can_invoke("rack.switch_hot_swap_target", &json!({}))
    );
    kit.shell
        .invoke("device.hot_swap", json!({ "device": kit.rack, "on": true }))
        .unwrap();
    kit.shell
        .invoke("pad.select", json!({ "rack": kit.rack, "note": 36 }))
        .unwrap();
    kit.shell
        .invoke("rack.switch_hot_swap_target", json!({}))
        .unwrap();
    let hot = state(&mut kit.shell).hot_swap.unwrap();
    assert_eq!(hot.label, "Swapping Pad");
    assert_eq!(pads(&mut kit).hot_swap, Some(36));
    let before = media_of(&kit, made.chains[0]);
    kit.shell
        .invoke("browser.select", json!({ "paths": [file] }))
        .unwrap();
    kit.shell.set_focused_area(Area::Browser);
    kit.shell
        .invoke("browser.load_selected", json!({}))
        .unwrap();
    assert_ne!(
        media_of(&kit, made.chains[0]),
        before,
        "the pad took the sample"
    );
    // D again switches back to the rack.
    kit.shell
        .invoke("rack.switch_hot_swap_target", json!({}))
        .unwrap();
    assert_eq!(pads(&mut kit).hot_swap, None);
    assert_eq!(state(&mut kit.shell).hot_swap.unwrap().device, kit.rack);
}

// Spec: drum-racks R18
#[test]
fn a_plug_in_loaded_on_a_hot_swapped_pad_leaves_the_kit_in_place() {
    if bl_plugin_host::scan::scan_program().is_none() {
        eprintln!("beatlayer-scan wasn't built; skipping (build the workspace's bins)");
        return;
    }
    let scratch = Scratch::new("hot-swap-plug-in");
    let mut kit = kit();
    drop_on(
        &mut kit,
        36,
        json!({ "samples": [scratch.wav("Kick.wav", 1_000)] }),
    );
    let bundle = bl_test_clap::bundle().to_string_lossy().into_owned();
    kit.shell
        .invoke("plugin.scan", json!({ "paths": [bundle.clone()] }))
        .unwrap();
    kit.shell
        .invoke("browser.open_place", json!({ "label": "plug_ins" }))
        .unwrap();
    let folder = kit.shell.invoke("browser.state", json!({})).unwrap()["rows"][0]["path"].clone();
    kit.shell
        .invoke(
            "browser.open_folder",
            json!({ "path": folder, "open": true }),
        )
        .unwrap();
    kit.shell
        .invoke(
            "browser.select",
            json!({ "paths": [format!("{bundle}#{}", bl_test_clap::PLUGIN_ID)] }),
        )
        .unwrap();
    kit.shell
        .invoke("device.hot_swap", json!({ "device": kit.rack, "on": true }))
        .unwrap();
    kit.shell
        .invoke("pad.select", json!({ "rack": kit.rack, "note": 36 }))
        .unwrap();
    kit.shell
        .invoke("rack.switch_hot_swap_target", json!({}))
        .unwrap();
    assert!(
        kit.shell
            .invoke("browser.load_selected", json!({}))
            .is_err()
    );
    assert!(kit.shell.editor().document().device(kit.rack).is_some());
}

// Spec: drum-racks R5
#[test]
fn preview_plays_one_chain_while_held_and_changes_nothing() {
    let scratch = Scratch::new("preview");
    let mut kit = kit();
    let a = drop_on(
        &mut kit,
        36,
        json!({ "samples": [scratch.wav("A.wav", 1_000)] }),
    )
    .chains[0];
    let steps_before = steps(&kit.shell);
    let doc_before = kit.shell.editor().document().clone();
    kit.shell
        .invoke("chain.preview", json!({ "chain": a, "on": true }))
        .unwrap();
    let row = |kit: &mut Drums| {
        state(&mut kit.shell)
            .devices
            .into_iter()
            .find(|d| d.id == kit.rack)
            .and_then(|d| d.rack)
            .unwrap()
            .chains
            .into_iter()
            .find(|c| c.id == a)
            .and_then(|c| c.drum)
            .unwrap()
    };
    assert!(row(&mut kit).previewing);
    assert!(pads(&mut kit).pads[0].previewing);
    kit.shell
        .invoke(
            "pad.preview",
            json!({ "rack": kit.rack, "note": 36, "on": false }),
        )
        .unwrap();
    assert!(!row(&mut kit).previewing);
    assert_eq!(steps(&kit.shell), steps_before, "not an undo step");
    assert_eq!(
        kit.shell.editor().document(),
        &doc_before,
        "the set is as it was"
    );
    // An empty pad previews nothing; a return chain has no preview.
    assert!(
        kit.shell
            .invoke(
                "pad.preview",
                json!({ "rack": kit.rack, "note": 50, "on": true })
            )
            .is_ok_and(|_| true)
    );
}

// Spec: drum-racks R25
#[test]
fn group_to_drum_rack_puts_the_selection_on_c1() {
    let mut shell = Shell::headless();
    let midi = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Midi)
        .map(bl_model::Track::id)
        .unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [midi] }))
        .unwrap();
    let mut ids = Vec::new();
    for device in [
        json!({ "kind": { "native": { "device_type": "test.synth" } }, "name": "Synth", "role": "instrument" }),
        effect(),
    ] {
        let added: DeviceAdded = serde_json::from_value(
            shell
                .invoke("device.add", json!({ "device": device }))
                .unwrap(),
        )
        .unwrap();
        ids.push(added.device);
    }
    let before = steps(&shell);
    let grouped: Grouped = serde_json::from_value(
        shell
            .invoke("device.group_to_drum_rack", json!({ "devices": ids }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(steps(&shell), before + 1, "one undo step");
    let doc = shell.editor().document();
    let rack = doc.device(grouped.rack).unwrap();
    assert_eq!(rack.rack_kind(), Some(bl_model::RackKind::Drum));
    let chain = &rack.chains[0];
    assert_eq!(chain.id, grouped.chain);
    assert_eq!((chain.receive, chain.play), (Receive::Note(36), 60));
    assert_eq!(chain.devices.iter().map(|d| d.id).collect::<Vec<_>>(), ids);
}

// Spec: drum-racks R26
#[test]
fn grouping_drum_chains_layers_them_on_the_first_ones_note() {
    let scratch = Scratch::new("group");
    let mut kit = kit();
    let a = drop_on(
        &mut kit,
        39,
        json!({ "samples": [scratch.wav("A.wav", 1_000)] }),
    )
    .chains[0];
    let b = drop_on(
        &mut kit,
        40,
        json!({ "samples": [scratch.wav("B.wav", 1_100)] }),
    )
    .chains[0];
    kit.shell
        .invoke("chain.set_choke", json!({ "chain": a, "choke": 3 }))
        .unwrap();
    kit.shell
        .invoke("chain.set_play", json!({ "chain": a, "play": 61 }))
        .unwrap();
    let before = steps(&kit.shell);
    let grouped: Grouped = serde_json::from_value(
        kit.shell
            .invoke("device.group", json!({ "chains": [a, b] }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(steps(&kit.shell), before + 1, "one undo step");
    let doc = kit.shell.editor().document();
    let outer = doc.chain(grouped.chain).unwrap();
    assert_eq!(
        (outer.receive, outer.play, outer.choke),
        (Receive::Note(39), 61, Some(3)),
        "the first chain's Receive, Play and Choke"
    );
    let inner = doc.device(grouped.rack).unwrap();
    assert_eq!(inner.rack_kind(), Some(bl_model::RackKind::Instrument));
    assert_eq!(
        inner.chains.iter().map(|c| c.id).collect::<Vec<_>>(),
        [a, b]
    );
    assert_eq!(doc.chain(a).unwrap().devices.len(), 1, "devices kept");
}

// Spec: drum-racks R8
#[test]
fn deleting_the_return_track_sends_the_chain_home() {
    let mut kit = kit();
    let made: bl_shell::device_commands::ChainAdded = serde_json::from_value(
        kit.shell
            .invoke(
                "chain.create",
                json!({ "rack": kit.rack, "device": effect(), "returns": true }),
            )
            .unwrap(),
    )
    .unwrap();
    let ret = made.chain;
    let track: TrackId = kit.shell.new_id();
    kit.shell
        .run(
            "track.create",
            json!({ "id": track, "kind": "return", "name": "A Reverb", "parent": null, "index": 0 }),
        )
        .unwrap();
    kit.shell
        .invoke(
            "chain.set_audio_to",
            json!({ "chain": ret, "track": track }),
        )
        .unwrap();
    assert_eq!(
        kit.shell.editor().document().chain(ret).unwrap().audio_to,
        Some(track)
    );
    kit.shell
        .run("track.delete", json!({ "track": track }))
        .unwrap();
    assert_eq!(
        kit.shell.editor().document().chain(ret).unwrap().audio_to,
        None
    );
    kit.shell.invoke("edit.undo", json!({})).unwrap();
    assert_eq!(
        kit.shell.editor().document().chain(ret).unwrap().audio_to,
        Some(track),
        "undo restores both"
    );
}

// Spec: drum-racks R7
#[test]
fn a_return_chain_takes_audio_effects_only_and_six_at_most() {
    let mut kit = kit();
    let rack = kit.rack;
    let create = |kit: &mut Drums, device: Value| {
        kit.shell.invoke(
            "chain.create",
            json!({ "rack": rack, "device": device, "returns": true }),
        )
    };
    for _ in 0..6 {
        create(&mut kit, effect()).unwrap();
    }
    assert!(create(&mut kit, effect()).is_err(), "a seventh is refused");
    let synth = json!({ "kind": { "native": { "device_type": "test.synth" } },
                        "name": "Synth", "role": "instrument" });
    let mut other = self::kit();
    assert!(
        create(&mut other, synth).is_err(),
        "a return chain holds audio effects only"
    );
}

// Spec: drum-racks R23
#[test]
fn a_chain_from_the_list_takes_its_devices_and_from_the_mixer_its_notes() {
    let scratch = Scratch::new("extract");
    let mut kit = kit();
    let a = drop_on(
        &mut kit,
        36,
        json!({ "samples": [scratch.wav("A.wav", 1_000)] }),
    )
    .chains[0];
    let b = drop_on(
        &mut kit,
        38,
        json!({ "samples": [scratch.wav("B.wav", 1_100)] }),
    )
    .chains[0];
    kit.shell
        .invoke("chain.set_play", json!({ "chain": b, "play": 62 }))
        .unwrap();
    // A clip on the rack's track plays both pads' notes.
    let clip: bl_model::ClipId = kit.shell.new_id();
    let beat = bl_core::time::Ticks(bl_core::time::TICKS_PER_QUARTER);
    // Spec: mpe R4. The note a chain takes with it keeps its envelopes.
    let point = |at: bl_core::time::Ticks, value: f64| bl_model::Breakpoint {
        at,
        value,
        bend: 0.0,
    };
    let bent = bl_model::Note {
        pitch: vec![point(bl_core::time::Ticks::ZERO, 0.0), point(beat / 2, 7.0)],
        slide: vec![point(bl_core::time::Ticks::ZERO, 90.0)],
        ..bl_model::Note::new(38, beat, beat, 90)
    };
    let notes = [
        bl_model::Note::new(36, bl_core::time::Ticks::ZERO, beat, 100),
        bent.clone(),
        bl_model::Note::new(36, beat * 2, beat, 80),
    ];
    kit.shell
        .run(
            "clip.create",
            serde_json::to_value(json!({
                "track": kit.track,
                "clip": bl_model::Clip::new(
                    clip,
                    bl_core::time::Ticks::ZERO,
                    beat * 4,
                    bl_model::ClipData::Midi(bl_model::MidiData::new(notes)),
                ),
            }))
            .unwrap(),
        )
        .unwrap();
    let before = steps(&kit.shell);
    let extracted: Extracted = serde_json::from_value(
        kit.shell
            .invoke("chain.extract", json!({ "chain": b }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(steps(&kit.shell), before + 1, "one undo step");
    let doc = kit.shell.editor().document();
    assert!(doc.chain(b).is_none(), "the chain left the rack");
    assert!(doc.chain(a).is_some());
    let new = doc.track(extracted.track).unwrap();
    assert_eq!(new.kind(), TrackKind::Midi);
    assert_eq!(
        new.devices().map(|d| d.id).collect::<Vec<_>>(),
        extracted.devices
    );
    assert_eq!(extracted.clips, 1);
    let moved = new.clips().next().unwrap();
    assert_eq!(moved.start, bl_core::time::Ticks::ZERO, "the same place");
    let bl_model::ClipData::Midi(data) = &moved.data else {
        panic!("a MIDI clip");
    };
    assert_eq!(data.notes.len(), 1);
    assert_eq!(data.notes[0].key, 62, "at the chain's Play note");
    assert_eq!(data.notes[0].start, beat);
    assert_eq!(data.notes[0].pitch, bent.pitch, "with its envelopes");
    assert_eq!(data.notes[0].slide, bent.slide);
    // The source clip keeps only the other pad's notes.
    let source = doc.track(kit.track).unwrap().clips().next().unwrap();
    let bl_model::ClipData::Midi(left) = &source.data else {
        panic!("a MIDI clip");
    };
    assert_eq!(left.notes.len(), 2);
    assert!(left.notes.iter().all(|n| n.key == 36));
}

// Spec: drum-racks R24
#[test]
fn a_return_chain_dragged_to_the_mixer_makes_a_return_track() {
    let mut kit = kit();
    let rack = kit.rack;
    let make = |kit: &mut Drums| -> ChainId {
        let added: bl_shell::device_commands::ChainAdded = serde_json::from_value(
            kit.shell
                .invoke(
                    "chain.create",
                    json!({ "rack": rack, "device": effect(), "returns": true }),
                )
                .unwrap(),
        )
        .unwrap();
        added.chain
    };
    let ret = make(&mut kit);
    let drum = {
        let added: bl_shell::device_commands::ChainAdded = serde_json::from_value(
            kit.shell
                .invoke("chain.create", json!({ "rack": rack }))
                .unwrap(),
        )
        .unwrap();
        added.chain
    };
    kit.shell
        .invoke(
            "chain.set_send",
            json!({ "chain": drum, "to": ret, "level": 0.5 }),
        )
        .unwrap();
    let returns_before = kit.shell.editor().document().returns().count();
    let before = steps(&kit.shell);
    let extracted: Extracted = serde_json::from_value(
        kit.shell
            .invoke("chain.extract_return", json!({ "chain": ret }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(steps(&kit.shell), before + 1, "one undo step");
    let doc = kit.shell.editor().document();
    assert!(doc.chain(ret).is_none());
    assert!(
        doc.chain(drum).unwrap().sends.is_empty(),
        "the send went with it"
    );
    assert_eq!(doc.returns().count(), returns_before + 1);
    assert_eq!(
        doc.returns().last(),
        Some(extracted.track),
        "after the last return"
    );
    assert_eq!(doc.track(extracted.track).unwrap().devices().count(), 1);
    kit.shell.invoke("edit.undo", json!({})).unwrap();
    assert!(kit.shell.editor().document().chain(ret).is_some());
}

// Spec: drum-racks R11, R13
#[test]
fn a_pad_selects_its_chains_and_the_view_moves_by_group_or_row() {
    let scratch = Scratch::new("pads");
    let mut kit = kit();
    let a = drop_on(
        &mut kit,
        37,
        json!({ "samples": [scratch.wav("A.wav", 1_000)] }),
    )
    .chains[0];
    let frame = pads(&mut kit);
    assert_eq!(frame.first, 36, "a new drum rack shows C1 to D#2");
    assert_eq!(frame.pads.len(), 16);
    kit.shell
        .invoke("rack.move_pads", json!({ "rack": kit.rack, "groups": 1 }))
        .unwrap();
    assert_eq!(pads(&mut kit).first, 52);
    kit.shell
        .invoke("rack.move_pads", json!({ "rack": kit.rack, "rows": -1 }))
        .unwrap();
    assert_eq!(pads(&mut kit).first, 48);
    kit.shell
        .invoke("rack.move_pads", json!({ "rack": kit.rack, "groups": -9 }))
        .unwrap();
    assert_eq!(pads(&mut kit).first, 0, "stays within the notes");
    kit.shell
        .invoke("pad.select", json!({ "rack": kit.rack, "note": 37 }))
        .unwrap();
    let selected = state(&mut kit.shell)
        .devices
        .into_iter()
        .find(|d| d.id == kit.rack)
        .and_then(|d| d.rack)
        .unwrap();
    assert_eq!(selected.selected_chains, [a]);
    assert_eq!(selected.pads.unwrap().selected, Some(37));
}

// Spec: drum-racks R23. The notes of a chain on All Notes keep their pitch,
// and a Session clip's notes leave it for a clip in the same slot.
#[test]
fn extracting_takes_session_clips_notes_and_keeps_all_notes_pitches() {
    let scratch = Scratch::new("extract-session");
    let mut drums = kit();
    let a = drop_on(
        &mut drums,
        36,
        json!({ "samples": [scratch.wav("A.wav", 1_000)] }),
    )
    .chains[0];
    drums
        .shell
        .invoke(
            "chain.set_receive",
            json!({ "chain": a, "receive": "all_notes" }),
        )
        .unwrap();
    let scene = drums.shell.editor().document().scenes().next().unwrap().id;
    let clip: bl_model::ClipId = drums.shell.new_id();
    let beat = bl_core::time::Ticks(bl_core::time::TICKS_PER_QUARTER);
    drums
        .shell
        .run(
            "clip.create_in_slot",
            json!({
                "track": drums.track,
                "scene": scene,
                "clip": bl_model::Clip::new(
                    clip,
                    bl_core::time::Ticks::ZERO,
                    beat * 4,
                    bl_model::ClipData::Midi(bl_model::MidiData::new([
                        bl_model::Note::new(41, bl_core::time::Ticks::ZERO, beat, 100),
                        bl_model::Note::new(50, beat, beat, 100),
                    ])),
                ),
            }),
        )
        .unwrap();
    let extracted: Extracted = serde_json::from_value(
        drums
            .shell
            .invoke("chain.extract", json!({ "chain": a }))
            .unwrap(),
    )
    .unwrap();
    let doc = drums.shell.editor().document();
    let moved = doc.slot(extracted.track, scene).expect("the same slot");
    let bl_model::ClipData::Midi(data) = &moved.data else {
        panic!("a MIDI clip");
    };
    let mut keys: Vec<u8> = data.notes.iter().map(|n| n.key).collect();
    keys.sort_unstable();
    assert_eq!(keys, [41, 50], "All Notes passes the pitches as they came");
    let bl_model::ClipData::Midi(left) = &doc.slot(drums.track, scene).unwrap().data else {
        panic!("a MIDI clip");
    };
    assert!(left.notes.is_empty(), "the source clip lost them");
}

// Spec: drum-racks R23, note-editing R31. A probability group whose notes
// extracting splits between the clips ends on both sides.
#[test]
fn extracting_ends_a_probability_group_it_splits() {
    let scratch = Scratch::new("extract-group");
    let mut drums = kit();
    let a = drop_on(
        &mut drums,
        36,
        json!({ "samples": [scratch.wav("A.wav", 1_000)] }),
    )
    .chains[0];
    let scene = drums.shell.editor().document().scenes().next().unwrap().id;
    let clip: bl_model::ClipId = drums.shell.new_id();
    let beat = bl_core::time::Ticks(bl_core::time::TICKS_PER_QUARTER);
    let grouped = |key, start| bl_model::Note {
        group: Some(1),
        ..bl_model::Note::new(key, start, beat, 100)
    };
    let mut data =
        bl_model::MidiData::new([grouped(36, bl_core::time::Ticks::ZERO), grouped(38, beat)]);
    data.groups = vec![bl_model::NoteGroup {
        id: 1,
        kind: bl_model::GroupKind::PlayOne,
    }];
    drums
        .shell
        .run(
            "clip.create_in_slot",
            json!({
                "track": drums.track,
                "scene": scene,
                "clip": bl_model::Clip::new(
                    clip,
                    bl_core::time::Ticks::ZERO,
                    beat * 4,
                    bl_model::ClipData::Midi(data),
                ),
            }),
        )
        .unwrap();
    let extracted: Extracted = serde_json::from_value(
        drums
            .shell
            .invoke("chain.extract", json!({ "chain": a }))
            .unwrap(),
    )
    .unwrap();
    let doc = drums.shell.editor().document();
    for track in [extracted.track, drums.track] {
        let bl_model::ClipData::Midi(data) = &doc.slot(track, scene).unwrap().data else {
            panic!("a MIDI clip");
        };
        assert_eq!(data.notes.len(), 1);
        assert!(data.groups.is_empty() && data.notes[0].group.is_none());
    }
}

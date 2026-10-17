//! Racks in the document through the shell's commands (WP-5.1; the racks
//! spec, R7 to R12): device commands inside chains, chain commands, Group
//! and Ungroup. Every edit is one undo step; devices are the engine's test
//! devices.

use bl_engine::testing;
use bl_model::{Chain, ChainId, DeviceId, DeviceRole, RackKind, TrackId};
use bl_shell::device_commands::{ChainAdded, DeviceAdded, Grouped, Ungrouped};
use bl_shell::{Area, Shell};
use serde_json::{Value, json};

struct Set {
    shell: Shell,
    midi: TrackId,
    audio: TrackId,
}

/// The default set with its first audio track selected and the Device View
/// focused.
fn set() -> Set {
    let mut shell = Shell::headless();
    let top: Vec<TrackId> = shell.editor().document().top_level().collect();
    let (midi, audio) = (top[0], top[2]);
    select_track(&mut shell, audio);
    shell.set_focused_area(Area::Detail);
    Set { shell, midi, audio }
}

fn select_track(shell: &mut Shell, track: TrackId) {
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
}

fn effect(name: &str) -> Value {
    let t = testing::gain_type();
    json!({ "kind": t.kind, "name": name, "role": t.role })
}

fn midi_effect(name: &str) -> Value {
    json!({
        "kind": { "native": { "device_type": "test.arp" } },
        "name": name,
        "role": "midi_effect",
    })
}

fn synth() -> Value {
    json!({
        "kind": { "native": { "device_type": "test.synth" } },
        "name": "Synth",
        "role": "instrument",
    })
}

fn rack_of(kind: &str) -> Value {
    json!({ "kind": { "rack": { "kind": kind } }, "name": "Rack", "role": "audio_effect" })
}

fn add(shell: &mut Shell, args: Value) -> DeviceAdded {
    serde_json::from_value(shell.invoke("device.add", args).unwrap()).unwrap()
}

fn add_to_track(shell: &mut Shell, device: Value) -> DeviceAdded {
    add(shell, json!({ "device": device }))
}

fn add_to_chain(shell: &mut Shell, chain: ChainId, device: Value) -> DeviceAdded {
    add(shell, json!({ "device": device, "chain": chain }))
}

/// Adds a rack of `kind` with `chains` empty chains, as an agent would: the
/// rack as a device, then each chain.
fn rack_with_chains(shell: &mut Shell, kind: &str, chains: usize) -> (DeviceId, Vec<ChainId>) {
    let rack = add_to_track(shell, rack_of(kind)).device;
    let ids = (0..chains)
        .map(|n| {
            let id: ChainId = shell.new_id();
            shell
                .invoke(
                    "chain.add",
                    json!({ "rack": rack, "index": n, "chain": { "id": id, "name": "", "devices": [] } }),
                )
                .unwrap();
            id
        })
        .collect();
    (rack, ids)
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

fn chain_devices(shell: &Shell, chain: ChainId) -> Vec<DeviceId> {
    shell
        .editor()
        .document()
        .chain(chain)
        .unwrap()
        .devices
        .iter()
        .map(|d| d.id)
        .collect()
}

fn track_devices(shell: &Shell, track: TrackId) -> Vec<DeviceId> {
    shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .devices()
        .map(|d| d.id)
        .collect()
}

fn select(shell: &mut Shell, device: DeviceId) {
    shell
        .invoke("device.select", json!({ "device": device }))
        .unwrap();
}

// Spec: racks R7
#[test]
fn device_commands_work_inside_chains_and_devices_move_in_and_out() {
    let Set {
        mut shell, audio, ..
    } = set();
    let (rack, chains) = rack_with_chains(&mut shell, "audio_effect", 2);
    // Added to a chain, after the selected device when it is in it.
    let first = add_to_chain(&mut shell, chains[0], effect("One"));
    assert_eq!((first.track, first.new_track), (audio, false));
    assert_eq!(last_step(&shell), "Insert Device");
    let second = add_to_chain(&mut shell, chains[0], effect("Two"));
    select(&mut shell, first.device);
    let third = add_to_chain(&mut shell, chains[0], effect("Three"));
    assert_eq!(
        chain_devices(&shell, chains[0]),
        [first.device, third.device, second.device]
    );

    // Rename, switch off, and delete reach a device in a chain by its ID.
    shell
        .invoke(
            "device.rename",
            json!({ "device": third.device, "name": "Deep" }),
        )
        .unwrap();
    shell
        .invoke(
            "device.set_enabled",
            json!({ "device": third.device, "enabled": false }),
        )
        .unwrap();
    let device = shell.editor().document().device(third.device).unwrap();
    assert_eq!((device.title(), device.enabled), ("Deep", false));

    // The clipboard: cut and paste into the other chain, before the
    // selected device when it is in the chain.
    select(&mut shell, second.device);
    shell
        .invoke("device.cut", json!({ "device": second.device }))
        .unwrap();
    assert_eq!(last_step(&shell), "Delete Device(s)");
    let pasted: DeviceAdded = serde_json::from_value(
        shell
            .invoke("device.paste", json!({ "chain": chains[1] }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(last_step(&shell), "Copy Device");
    assert_eq!(chain_devices(&shell, chains[1]), [pasted.device]);
    // A copy of a device in a chain, pasted before one that is in a chain,
    // goes in that chain.
    shell
        .invoke("device.copy", json!({ "device": first.device }))
        .unwrap();
    let before: DeviceAdded = serde_json::from_value(
        shell
            .invoke("device.paste", json!({ "before": pasted.device }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        chain_devices(&shell, chains[1]),
        [before.device, pasted.device]
    );
    // Duplicate puts a copy straight after, in the same chain.
    let copy: DeviceAdded = serde_json::from_value(
        shell
            .invoke("device.duplicate", json!({ "device": first.device }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(last_step(&shell), "Duplicate Device");
    assert_eq!(
        chain_devices(&shell, chains[0]),
        [first.device, copy.device, third.device]
    );

    // Moving: out of a rack onto the track, into the other chain, and back.
    shell
        .invoke(
            "device.move",
            json!({ "device": third.device, "track": audio, "index": 0 }),
        )
        .unwrap();
    assert_eq!(track_devices(&shell, audio)[0], third.device);
    shell
        .invoke(
            "device.move_to_chain",
            json!({ "device": third.device, "chain": chains[1], "index": 1 }),
        )
        .unwrap();
    assert_eq!(chain_devices(&shell, chains[1])[1], third.device);
    assert_eq!(
        shell.editor().document().device_track(third.device),
        Some(audio)
    );
    shell
        .invoke("device.remove", json!({ "device": third.device }))
        .unwrap();
    assert!(shell.editor().document().device(third.device).is_none());

    // A copy of a whole rack gets new IDs for every chain and device in it.
    shell
        .invoke("device.copy", json!({ "device": rack }))
        .unwrap();
    select(&mut shell, rack);
    let copy: DeviceAdded = serde_json::from_value(
        shell
            .invoke("device.paste", json!({ "track": audio }))
            .unwrap(),
    )
    .unwrap();
    let doc = shell.editor().document();
    let (old, new) = (doc.device(rack).unwrap(), doc.device(copy.device).unwrap());
    assert_eq!(old.chains.len(), new.chains.len());
    let ids: std::collections::BTreeSet<_> = [old, new].iter().flat_map(|r| r.ids()).collect();
    assert_eq!(ids.len(), old.ids().len() * 2, "no ID is shared");
    doc.check_invariants().unwrap();
}

// Spec: racks R8
#[test]
fn chains_are_added_moved_copied_renamed_and_coloured_in_one_step_each() {
    let Set { mut shell, .. } = set();
    let (rack, chains) = rack_with_chains(&mut shell, "audio_effect", 1);
    let (other, others) = rack_with_chains(&mut shell, "audio_effect", 1);
    // An empty chain is named Chain, and one made with a device in it after
    // the device's title.
    let doc = shell.editor().document();
    assert_eq!(doc.chain(chains[0]).unwrap().name, "Chain");
    let device = add_to_chain(&mut shell, chains[0], effect("Reverb")).device;
    let named: ChainId = shell.new_id();
    let fx: DeviceId = shell.new_id();
    let before = steps(&shell);
    shell
        .invoke(
            "chain.add",
            json!({ "rack": rack, "index": 1, "chain": {
                "id": named, "name": "", "devices": [{
                    "id": fx, "name": "Delay", "enabled": true, "params": [],
                    "kind": testing::through_type().kind,
                }],
            } }),
        )
        .unwrap();
    assert_eq!(steps(&shell), before + 1);
    assert_eq!(
        shell.editor().document().chain(named).unwrap().name,
        "Delay"
    );

    // Each of these is one undo step.
    for (command, args) in [
        ("chain.rename", json!({ "chain": named, "name": "Wet" })),
        ("chain.set_colour", json!({ "chain": named, "colour": 5 })),
        (
            "chain.set_info",
            json!({ "chain": named, "info": "echoes" }),
        ),
        (
            "chain.move",
            json!({ "chain": named, "rack": other, "index": 0 }),
        ),
        (
            "chain.move",
            json!({ "chain": named, "rack": rack, "index": 0 }),
        ),
        ("chain.rename", json!({ "chain": named, "name": "" })),
    ] {
        let before = steps(&shell);
        shell.invoke(command, args.clone()).unwrap();
        assert_eq!(steps(&shell), before + 1, "{command} {args}");
    }
    let chain = shell.editor().document().chain(named).unwrap();
    assert_eq!((chain.name.as_str(), chain.colour), ("Delay", Some(5)));
    assert_eq!(chain.info, "echoes");

    // Copy and paste, duplicate, and cut: new IDs, the devices copied.
    shell
        .invoke("chain.copy", json!({ "chain": chains[0] }))
        .unwrap();
    let pasted: ChainAdded = serde_json::from_value(
        shell
            .invoke("chain.paste", json!({ "rack": other, "index": 0 }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(last_step(&shell), "Insert Chain");
    assert_eq!(pasted.rack, other);
    let doc = shell.editor().document();
    assert_eq!(doc.chain_rack(pasted.chain).unwrap().id, other);
    let copied = doc.chain(pasted.chain).unwrap();
    assert_eq!(copied.devices.len(), 1);
    assert_ne!(copied.devices[0].id, device);
    assert_eq!(copied.devices[0].name, "Reverb");
    let duplicate: ChainAdded = serde_json::from_value(
        shell
            .invoke("chain.duplicate", json!({ "chain": named }))
            .unwrap(),
    )
    .unwrap();
    let doc = shell.editor().document();
    let order: Vec<ChainId> = doc
        .device(rack)
        .unwrap()
        .chains
        .iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(order, [named, duplicate.chain, chains[0]]);
    shell
        .invoke("chain.cut", json!({ "chain": named }))
        .unwrap();
    assert_eq!(last_step(&shell), "Delete Chain");
    assert!(shell.editor().document().chain(named).is_none());
    shell.editor().document().check_invariants().unwrap();
    let _ = others;
    // Every one of them undoes.
    while shell.editor().can_undo() {
        shell.invoke("edit.undo", json!({})).unwrap();
    }
    assert!(shell.editor().document().device(rack).is_none());
}

// Spec: racks R9
#[test]
fn group_wraps_the_selection_in_the_kind_that_holds_it() {
    let Set {
        mut shell,
        midi,
        audio,
    } = set();
    // Audio effects alone go in an audio effect rack, in the first one's
    // place, and the rack is selected.
    let one = add_to_track(&mut shell, effect("One")).device;
    let two = add_to_track(&mut shell, effect("Two")).device;
    let three = add_to_track(&mut shell, effect("Three")).device;
    let before = steps(&shell);
    let grouped: Grouped = serde_json::from_value(
        shell
            .invoke("device.group", json!({ "devices": [three, one] }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(steps(&shell), before + 1);
    assert_eq!(last_step(&shell), "Group");
    let doc = shell.editor().document();
    let rack = doc.device(grouped.rack).unwrap();
    assert_eq!(rack.rack_kind(), Some(RackKind::AudioEffect));
    assert_eq!(rack.role, DeviceRole::AudioEffect);
    assert_eq!(track_devices(&shell, audio), [grouped.rack, two]);
    assert_eq!(chain_devices(&shell, grouped.chain), [one, three]);
    assert_eq!(rack.chains[0].name, "One", "named after the first device");
    assert_eq!(shell.device_view_state().selected, Some(grouped.rack));
    // A grouped device keeps its settings and place in the document.
    assert_eq!(shell.editor().document().device_track(one), Some(audio));

    // Grouping devices already in a rack nests the new rack in their chain.
    select(&mut shell, one);
    let nested: Grouped =
        serde_json::from_value(shell.invoke("device.group", json!({})).unwrap()).unwrap();
    assert_eq!(chain_devices(&shell, grouped.chain), [nested.rack, three]);
    assert_eq!(chain_devices(&shell, nested.chain), [one]);
    shell.invoke("edit.undo", json!({})).unwrap();
    shell.invoke("edit.undo", json!({})).unwrap();
    assert_eq!(track_devices(&shell, audio), [one, two, three]);

    // MIDI effects alone go in a MIDI effect rack; anything else in an
    // instrument rack.
    select_track(&mut shell, midi);
    let arp = add_to_track(&mut shell, midi_effect("Arp")).device;
    let reverb = add_to_track(&mut shell, effect("Verb")).device;
    let kind = |shell: &mut Shell, devices: Vec<DeviceId>| {
        let grouped: Grouped = serde_json::from_value(
            shell
                .invoke("device.group", json!({ "devices": devices }))
                .unwrap(),
        )
        .unwrap();
        let rack = shell.editor().document().device(grouped.rack).unwrap();
        let (kind, role) = (rack.rack_kind().unwrap(), rack.role);
        shell.invoke("edit.undo", json!({})).unwrap();
        (kind, role)
    };
    assert_eq!(
        kind(&mut shell, vec![arp]),
        (RackKind::MidiEffect, DeviceRole::MidiEffect)
    );
    assert_eq!(
        kind(&mut shell, vec![reverb]),
        (RackKind::AudioEffect, DeviceRole::AudioEffect)
    );
    assert_eq!(
        kind(&mut shell, vec![arp, reverb]),
        (RackKind::Instrument, DeviceRole::Instrument)
    );
    let synth = add_to_track(&mut shell, synth()).device;
    assert_eq!(
        kind(&mut shell, vec![synth]),
        (RackKind::Instrument, DeviceRole::Instrument)
    );
    // A selection that would break the chain's order is refused: an
    // instrument rack holding the MIDI effect and the audio effect would
    // sit beside the synth.
    let before = steps(&shell);
    assert!(
        shell
            .invoke("device.group", json!({ "devices": [arp, reverb] }))
            .is_err()
    );
    assert_eq!(steps(&shell), before, "refused, so no step");
    // Devices from two chains can't be grouped together.
    let (_, chains) = rack_with_chains(&mut shell, "audio_effect", 1);
    let inside = add_to_chain(&mut shell, chains[0], effect("In")).device;
    assert!(
        shell
            .invoke("device.group", json!({ "devices": [reverb, inside] }))
            .is_err()
    );
    shell.editor().document().check_invariants().unwrap();
}

// Spec: racks R10
#[test]
fn grouping_chains_nests_a_rack_of_the_same_kind() {
    let Set { mut shell, .. } = set();
    let (rack, chains) = rack_with_chains(&mut shell, "audio_effect", 4);
    for (n, chain) in chains.iter().enumerate() {
        shell
            .invoke(
                "chain.set_gain",
                json!({ "chain": chain, "volume": 0.25 * (n + 1) as f64 }),
            )
            .unwrap();
    }
    let before = steps(&shell);
    let grouped: Grouped = serde_json::from_value(
        shell
            .invoke("device.group", json!({ "chains": [chains[2], chains[1]] }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(steps(&shell), before + 1, "one undo step");
    let doc = shell.editor().document();
    let outer: Vec<ChainId> = doc
        .device(rack)
        .unwrap()
        .chains
        .iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(outer, [chains[0], grouped.chain, chains[3]]);
    let inner_rack = &doc.chain(grouped.chain).unwrap().devices[0];
    assert_eq!(inner_rack.id, grouped.rack);
    assert_eq!(inner_rack.rack_kind(), Some(RackKind::AudioEffect));
    let inner: Vec<ChainId> = inner_rack.chains.iter().map(|c| c.id).collect();
    assert_eq!(inner, [chains[1], chains[2]]);
    // Each keeps its mixer; the new chain is at 0 dB and centre.
    assert_eq!(doc.chain(chains[2]).unwrap().mixer.volume, 0.75);
    assert_eq!(doc.chain(grouped.chain).unwrap().mixer.volume, 1.0);
    assert_eq!(doc.chain(grouped.chain).unwrap().mixer.pan, 0.0);
    doc.check_invariants().unwrap();
    shell.invoke("edit.undo", json!({})).unwrap();
    assert_eq!(
        shell.editor().document().device(rack).unwrap().chains.len(),
        4
    );
}

// Spec: racks R11
#[test]
fn ungroup_puts_one_chains_devices_back_and_is_disabled_for_several() {
    let Set {
        mut shell, audio, ..
    } = set();
    let one = add_to_track(&mut shell, effect("One")).device;
    let two = add_to_track(&mut shell, effect("Two")).device;
    let three = add_to_track(&mut shell, effect("Three")).device;
    let grouped: Grouped = serde_json::from_value(
        shell
            .invoke("device.group", json!({ "devices": [one, two] }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(track_devices(&shell, audio), [grouped.rack, three]);
    select(&mut shell, grouped.rack);
    assert!(shell.can_invoke("device.ungroup", &json!({})));
    let before = steps(&shell);
    let ungrouped: Ungrouped =
        serde_json::from_value(shell.invoke("device.ungroup", json!({})).unwrap()).unwrap();
    assert_eq!(steps(&shell), before + 1);
    assert_eq!(last_step(&shell), "Ungroup");
    assert_eq!(ungrouped.devices, [one, two]);
    assert_eq!(track_devices(&shell, audio), [one, two, three]);
    assert_eq!(shell.device_view_state().selected, Some(one), "the first");
    assert!(shell.editor().document().device(grouped.rack).is_none());
    assert!(shell.editor().document().chain(grouped.chain).is_none());

    // A rack of several chains can't be ungrouped; one with none is
    // removed; a device that isn't a rack can't be either.
    let (several, _) = rack_with_chains(&mut shell, "audio_effect", 2);
    select(&mut shell, several);
    assert!(!shell.can_invoke("device.ungroup", &json!({})));
    assert!(shell.invoke("device.ungroup", json!({})).is_err());
    let (empty, _) = rack_with_chains(&mut shell, "audio_effect", 0);
    select(&mut shell, empty);
    assert!(shell.can_invoke("device.ungroup", &json!({})));
    shell.invoke("device.ungroup", json!({})).unwrap();
    assert!(shell.editor().document().device(empty).is_none());
    select(&mut shell, one);
    assert!(!shell.can_invoke("device.ungroup", &json!({})));
}

// Spec: racks R12
#[test]
fn switching_a_rack_with_its_contents_is_one_undo_step() {
    let Set { mut shell, .. } = set();
    let (rack, chains) = rack_with_chains(&mut shell, "audio_effect", 1);
    let device = add_to_chain(&mut shell, chains[0], effect("In")).device;
    let before = steps(&shell);
    shell
        .invoke(
            "device.set_enabled_all",
            json!({ "device": rack, "enabled": false }),
        )
        .unwrap();
    assert_eq!(steps(&shell), before + 1);
    let doc = shell.editor().document();
    assert!(!doc.device(rack).unwrap().enabled);
    assert!(!doc.device(device).unwrap().enabled);
    shell.invoke("edit.undo", json!({})).unwrap();
    assert!(shell.editor().document().device(device).unwrap().enabled);
}

// Spec: racks R13
#[test]
fn a_chain_and_its_devices_survive_save_and_load() {
    let Set { mut shell, .. } = set();
    let (rack, chains) = rack_with_chains(&mut shell, "audio_effect", 2);
    let device = add_to_chain(&mut shell, chains[1], effect("In")).device;
    shell
        .invoke(
            "chain.set_colour",
            json!({ "chain": chains[1], "colour": 9 }),
        )
        .unwrap();
    shell
        .invoke(
            "chain.set_gain",
            json!({ "chain": chains[1], "volume": 0.5 }),
        )
        .unwrap();
    shell
        .invoke(
            "chain.set_solo",
            json!({ "chain": chains[0], "solo": true }),
        )
        .unwrap();
    let folder = std::env::temp_dir().join(format!("bl-racks-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let saved = shell
        .run(
            "set.save_as",
            json!({ "path": folder.join("Racks.blset").to_string_lossy() }),
        )
        .unwrap();
    let mut reopened = Shell::headless();
    reopened
        .run("set.open", json!({ "path": saved["path"] }))
        .unwrap();
    std::fs::remove_dir_all(&folder).ok();
    let doc = reopened.editor().document();
    assert_eq!(
        doc.device(rack),
        shell.editor().document().device(rack),
        "the rack, its chains and everything in them"
    );
    let chain: &Chain = doc.chain(chains[1]).unwrap();
    assert_eq!(chain.colour, Some(9));
    assert_eq!(chain.mixer.volume, 0.5);
    assert!(doc.chain(chains[0]).unwrap().mixer.solo);
    assert_eq!(chain.devices[0].id, device);
    doc.check_invariants().unwrap();
}

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

#[test]
fn a_track_with_a_rack_duplicates_with_new_ids_inside_it() {
    let Set {
        mut shell, audio, ..
    } = set();
    let (rack, chains) = rack_with_chains(&mut shell, "audio_effect", 1);
    let inner = add_to_chain(&mut shell, chains[0], effect("Gain")).device;
    let devices = shell.editor().document().device_count();

    shell
        .invoke(
            "track.duplicate_without_clips",
            json!({ "tracks": [audio] }),
        )
        .unwrap();

    let doc = shell.editor().document();
    doc.check_invariants().unwrap();
    assert_eq!(doc.device_count(), devices + 2);
    assert_eq!(doc.chain_ids().len(), 2);
    assert_eq!(doc.device_track(rack), Some(audio));
    assert_eq!(doc.device_track(inner), Some(audio));
}

fn state(shell: &Shell) -> bl_shell::device_commands::DeviceViewState {
    shell.device_view_state()
}

/// The rack's frame in the Device View.
fn frame(shell: &Shell, rack: DeviceId) -> bl_shell::device_commands::RackFrame {
    state(shell)
        .devices
        .into_iter()
        .find(|d| d.id == rack)
        .and_then(|d| d.rack)
        .expect("the rack is on the shown track")
}

fn selected(shell: &Shell, rack: DeviceId) -> Vec<ChainId> {
    frame(shell, rack).selected_chains
}

fn click(shell: &mut Shell, chain: ChainId, mode: &str) {
    shell
        .invoke("chain.select", json!({ "chain": chain, "mode": mode }))
        .unwrap();
}

// Spec: racks R19
#[test]
fn showing_and_hiding_views_is_no_undo_step_and_is_kept() {
    let Set { mut shell, .. } = set();
    let (rack, _) = rack_with_chains(&mut shell, "audio_effect", 2);
    let before = steps(&shell);
    assert!(
        frame(&shell, rack).views.chain_list,
        "a new rack shows them all"
    );
    shell
        .invoke(
            "rack.set_view",
            json!({ "rack": rack, "view": "chain_list", "shown": false }),
        )
        .unwrap();
    shell
        .invoke("rack.set_view", json!({ "rack": rack, "view": "macros" }))
        .unwrap();
    let views = frame(&shell, rack).views;
    assert!(!views.chain_list && !views.macros && views.devices);
    assert_eq!(steps(&shell), before, "no undo step");
    assert!(shell.is_dirty(), "but the set has to be saved");

    // Every view hidden folds the rack into its view column.
    shell
        .invoke(
            "rack.set_view",
            json!({ "rack": rack, "view": "devices", "shown": false }),
        )
        .unwrap();
    let hidden = state(&shell)
        .devices
        .into_iter()
        .find(|d| d.id == rack)
        .unwrap();
    assert!(hidden.rack.as_ref().unwrap().is_strip());
    assert_eq!(
        bl_shell::device_commands::frame_width(&hidden),
        bl_shell::device_commands::TITLE_BAR_HEIGHT
    );
    shell
        .invoke(
            "rack.set_view",
            json!({ "rack": rack, "view": "devices", "shown": true }),
        )
        .unwrap();

    // Reopened, the rack shows the same views.
    let folder = std::env::temp_dir().join(format!("bl-rack-views-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let saved = shell
        .run(
            "set.save_as",
            json!({ "path": folder.join("Views.blset").to_string_lossy() }),
        )
        .unwrap();
    let mut reopened = Shell::headless();
    reopened
        .run("set.open", json!({ "path": saved["path"] }))
        .unwrap();
    std::fs::remove_dir_all(&folder).ok();
    assert_eq!(reopened.devices().views_of(rack), views);
    // A rack that was never changed has no entry in the file.
    assert_eq!(reopened.devices().rack_views().len(), 1);
    // A device that isn't a rack has no views.
    let plain = add_to_track(&mut shell, effect("Gain")).device;
    assert!(
        shell
            .invoke("rack.set_view", json!({ "rack": plain, "view": "macros" }))
            .is_err()
    );
}

// Spec: racks R20
#[test]
fn a_key_that_fixes_shown_acts_only_when_it_changes_the_view() {
    let Set { mut shell, .. } = set();
    let (rack, _) = rack_with_chains(&mut shell, "audio_effect", 1);
    select(&mut shell, rack);
    let hide = json!({ "view": "chain_list", "shown": false });
    let show = json!({ "view": "chain_list", "shown": true });
    assert!(shell.can_invoke("rack.set_view", &hide));
    assert!(!shell.can_invoke("rack.set_view", &show));
    shell.invoke("rack.set_view", hide.clone()).unwrap();
    assert!(
        !shell.can_invoke("rack.set_view", &hide),
        "Minus falls back to Fold"
    );
    assert!(shell.can_invoke("rack.set_view", &show));
    // Disabled while no rack is selected.
    let plain = add_to_track(&mut shell, effect("Gain")).device;
    select(&mut shell, plain);
    assert!(!shell.can_invoke("rack.set_view", &show));
}

// Spec: racks R22
#[test]
fn chains_select_by_click_range_toggle_and_step() {
    let Set { mut shell, .. } = set();
    let (rack, chains) = rack_with_chains(&mut shell, "audio_effect", 4);
    let steps_before = steps(&shell);
    assert_eq!(
        selected(&shell, rack),
        vec![chains[0]],
        "the first while none is clicked"
    );
    click(&mut shell, chains[2], "replace");
    assert_eq!(selected(&shell, rack), vec![chains[2]]);
    click(&mut shell, chains[0], "range");
    assert_eq!(selected(&shell, rack), chains[..3].to_vec());
    click(&mut shell, chains[3], "toggle");
    assert_eq!(
        selected(&shell, rack),
        vec![chains[0], chains[1], chains[2], chains[3]]
    );
    click(&mut shell, chains[1], "toggle");
    assert_eq!(
        selected(&shell, rack),
        vec![chains[0], chains[2], chains[3]]
    );
    let shown = frame(&shell, rack);
    assert!(shown.devices.is_empty(), "several chains show no devices");
    assert_eq!(shown.note.as_deref(), Some("3 chains selected"));
    assert!(shown.chain_list_focused);

    click(&mut shell, chains[1], "replace");
    shell
        .invoke("chain.select", json!({ "mode": "next" }))
        .unwrap();
    assert_eq!(selected(&shell, rack), vec![chains[2]]);
    shell
        .invoke("chain.select", json!({ "mode": "previous" }))
        .unwrap();
    shell
        .invoke("chain.select", json!({ "mode": "previous" }))
        .unwrap();
    shell
        .invoke("chain.select", json!({ "mode": "previous" }))
        .unwrap();
    assert_eq!(
        selected(&shell, rack),
        vec![chains[0]],
        "Up stops at the first"
    );
    for _ in 0..6 {
        shell
            .invoke("chain.select", json!({ "mode": "next" }))
            .unwrap();
    }
    assert_eq!(
        selected(&shell, rack),
        vec![chains[3]],
        "Down stops at the last"
    );
    assert_eq!(steps(&shell), steps_before, "selecting is no undo step");
    assert!(
        shell
            .active_contexts()
            .contains(&bl_commands::Context::RackChains)
    );

    // A device click takes the focus back.
    let inside = add_to_chain(&mut shell, chains[1], effect("In")).device;
    select(&mut shell, inside);
    assert!(
        !shell
            .active_contexts()
            .contains(&bl_commands::Context::RackChains)
    );
}

// Spec: racks R22
#[test]
fn the_selected_chains_devices_show_and_selecting_a_device_reveals_its_chain() {
    let Set { mut shell, .. } = set();
    let (outer, chains) = rack_with_chains(&mut shell, "audio_effect", 2);
    let inner = add_to_chain(&mut shell, chains[1], rack_of("audio_effect")).device;
    let inner_chains: Vec<ChainId> = (0..2)
        .map(|n| {
            let id: ChainId = shell.new_id();
            shell
                .invoke(
                    "chain.add",
                    json!({ "rack": inner, "index": n, "chain": { "id": id, "name": "", "devices": [] } }),
                )
                .unwrap();
            id
        })
        .collect();
    let deep = add_to_chain(&mut shell, inner_chains[1], effect("Deep")).device;
    // Nothing is clicked yet: the first chains show.
    assert!(frame(&shell, outer).devices.is_empty());

    shell
        .invoke(
            "rack.set_view",
            json!({ "rack": outer, "view": "devices", "shown": false }),
        )
        .unwrap();
    shell
        .invoke("device.fold", json!({ "device": inner, "folded": true }))
        .unwrap();
    select(&mut shell, deep);
    let outer_frame = frame(&shell, outer);
    assert_eq!(outer_frame.selected_chains, vec![chains[1]]);
    assert!(outer_frame.views.devices, "the devices view is shown again");
    let inner_frame = &outer_frame.devices[0];
    assert!(!inner_frame.folded && inner_frame.id == inner);
    let nested = inner_frame.rack.as_ref().unwrap();
    assert_eq!(nested.selected_chains, vec![inner_chains[1]]);
    assert_eq!(nested.devices[0].id, deep);
    assert!(nested.devices[0].selected);
}

// Spec: racks R23
#[test]
fn chain_solo_is_exclusive_and_add_toggles() {
    let Set { mut shell, .. } = set();
    let (rack, chains) = rack_with_chains(&mut shell, "audio_effect", 3);
    let (other, other_chains) = rack_with_chains(&mut shell, "audio_effect", 1);
    let solos = |shell: &Shell, rack| -> Vec<bool> {
        frame(shell, rack).chains.iter().map(|c| c.solo).collect()
    };
    shell
        .invoke(
            "chain.set_solo",
            json!({ "chain": other_chains[0], "solo": true }),
        )
        .unwrap();
    let before = steps(&shell);
    shell
        .invoke("chain.solo", json!({ "chain": chains[0] }))
        .unwrap();
    shell
        .invoke("chain.solo", json!({ "chain": chains[1] }))
        .unwrap();
    assert_eq!(
        solos(&shell, rack),
        [false, true, false],
        "a click unsolos the others"
    );
    shell
        .invoke("chain.solo", json!({ "chain": chains[2], "add": true }))
        .unwrap();
    assert_eq!(solos(&shell, rack), [false, true, true], "Cmd-click adds");
    shell
        .invoke("chain.solo", json!({ "chain": chains[1], "add": true }))
        .unwrap();
    assert_eq!(solos(&shell, rack), [false, false, true], "and removes");
    shell
        .invoke("chain.solo", json!({ "chain": chains[2] }))
        .unwrap();
    assert_eq!(
        solos(&shell, rack),
        [false, false, false],
        "a click on the only soloed chain unsolos it"
    );
    assert_eq!(solos(&shell, other), [true], "other racks are left alone");
    assert_eq!(steps(&shell), before + 5, "one undo step each");
    assert_eq!(last_step(&shell), "Solo chain");

    // S solos the selected chain while the chain list has the focus.
    click(&mut shell, chains[1], "replace");
    assert!(shell.can_invoke("chain.solo", &json!({})));
    shell.invoke("chain.solo", json!({})).unwrap();
    assert_eq!(solos(&shell, rack), [false, true, false]);
}

// Spec: racks R21
#[test]
fn the_drop_area_makes_chains_from_devices_presets_and_moved_devices() {
    let Set { mut shell, .. } = set();
    let (rack, chains) = rack_with_chains(&mut shell, "audio_effect", 1);
    let before = steps(&shell);
    let new = shell
        .invoke(
            "chain.create",
            json!({ "rack": rack, "device": effect("Reverb") }),
        )
        .unwrap();
    let new: ChainAdded = serde_json::from_value(new).unwrap();
    assert_eq!(steps(&shell), before + 1);
    let doc = shell.editor().document();
    assert_eq!(
        doc.device(rack).unwrap().chains[1].id,
        new.chain,
        "at the end of the list"
    );
    assert_eq!(
        doc.chain(new.chain).unwrap().name,
        "Reverb",
        "named after its device"
    );
    assert_eq!(selected(&shell, rack), vec![new.chain]);

    // A device from another chain moves in, keeping its identity.
    let device = add_to_chain(&mut shell, chains[0], effect("Moved")).device;
    let moved: ChainAdded = serde_json::from_value(
        shell
            .invoke(
                "chain.create",
                json!({ "rack": rack, "move_device": device }),
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(chain_devices(&shell, moved.chain), vec![device]);
    assert!(chain_devices(&shell, chains[0]).is_empty());

    // An empty chain, and a mistake changes nothing.
    shell
        .invoke("chain.create", json!({ "rack": rack }))
        .unwrap();
    let steps_now = steps(&shell);
    assert!(
        shell
            .invoke(
                "chain.create",
                json!({ "rack": rack, "device": effect("A"), "move_device": device }),
            )
            .is_err()
    );
    assert!(
        shell
            .invoke(
                "chain.create",
                json!({ "rack": rack, "device": midi_effect("Arp") })
            )
            .is_err(),
        "an audio effect rack takes audio effects only"
    );
    assert_eq!(steps(&shell), steps_now);
}

// Spec: racks R22
#[test]
fn the_edit_keys_act_on_the_selected_chains() {
    let Set { mut shell, .. } = set();
    let (rack, chains) = rack_with_chains(&mut shell, "audio_effect", 4);
    click(&mut shell, chains[1], "replace");
    click(&mut shell, chains[2], "toggle");
    // Copy, then paste at the end.
    shell.invoke("chain.copy", json!({})).unwrap();
    assert!(state(&shell).can_paste_chain);
    let pasted: ChainAdded =
        serde_json::from_value(shell.invoke("chain.paste", json!({})).unwrap()).unwrap();
    assert_eq!(pasted.chains.len(), 2);
    assert_eq!(
        shell.editor().document().device(rack).unwrap().chains.len(),
        6
    );
    assert_eq!(
        selected(&shell, rack),
        pasted.chains,
        "the pasted chains are selected"
    );
    // Duplicate puts each copy straight after its original.
    click(&mut shell, chains[0], "replace");
    click(&mut shell, chains[1], "toggle");
    let before = steps(&shell);
    let copies: ChainAdded =
        serde_json::from_value(shell.invoke("chain.duplicate", json!({})).unwrap()).unwrap();
    assert_eq!(steps(&shell), before + 1);
    let order: Vec<ChainId> = shell
        .editor()
        .document()
        .device(rack)
        .unwrap()
        .chains
        .iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(order[0], chains[0]);
    assert_eq!(order[1], copies.chains[0]);
    assert_eq!(order[2], chains[1]);
    assert_eq!(order[3], copies.chains[1]);
    // Cut, and Delete, remove the selected chains in one step each.
    let count = order.len();
    let before = steps(&shell);
    shell.invoke("chain.cut", json!({})).unwrap();
    assert_eq!(steps(&shell), before + 1);
    assert_eq!(
        shell.editor().document().device(rack).unwrap().chains.len(),
        count - 2
    );
    click(&mut shell, chains[3], "replace");
    shell.invoke("chain.delete", json!({})).unwrap();
    assert!(shell.editor().document().chain(chains[3]).is_none());
    assert_eq!(last_step(&shell), "Delete Chain");
    shell.invoke("edit.undo", json!({})).unwrap();
    assert!(shell.editor().document().chain(chains[3]).is_some());
}

// Spec: racks R10
#[test]
fn group_with_the_chain_list_in_focus_groups_the_selected_chains() {
    let Set { mut shell, .. } = set();
    let (rack, chains) = rack_with_chains(&mut shell, "audio_effect", 3);
    click(&mut shell, chains[0], "replace");
    click(&mut shell, chains[1], "range");
    assert!(shell.can_invoke("device.group", &json!({})));
    let grouped: Grouped =
        serde_json::from_value(shell.invoke("device.group", json!({})).unwrap()).unwrap();
    let doc = shell.editor().document();
    let inner = doc.device(grouped.rack).unwrap();
    assert_eq!(
        inner.chains.iter().map(|c| c.id).collect::<Vec<_>>(),
        chains[..2].to_vec()
    );
    assert_eq!(doc.device(rack).unwrap().chains.len(), 2);
}

// Spec: racks R24
#[test]
fn a_chain_moves_between_racks_and_its_devices_drop_on_a_track() {
    let Set {
        mut shell, audio, ..
    } = set();
    let (rack, chains) = rack_with_chains(&mut shell, "audio_effect", 2);
    let (other, _) = rack_with_chains(&mut shell, "audio_effect", 1);
    let device = add_to_chain(&mut shell, chains[1], effect("Gain")).device;
    let before = steps(&shell);
    shell
        .invoke(
            "chain.move",
            json!({ "chain": chains[1], "rack": other, "index": 0 }),
        )
        .unwrap();
    assert_eq!(steps(&shell), before + 1);
    assert_eq!(
        shell.editor().document().chain_place(chains[1]).unwrap(),
        (other, 0)
    );
    assert_eq!(selected(&shell, other).len(), 1);

    // Dropped on a track's own chain, only the devices go there.
    let before = steps(&shell);
    shell
        .invoke(
            "chain.drop_on_track",
            json!({ "chain": chains[1], "track": audio }),
        )
        .unwrap();
    assert_eq!(steps(&shell), before + 1);
    assert!(shell.editor().document().chain(chains[1]).is_none());
    assert!(track_devices(&shell, audio).contains(&device));
    shell.invoke("edit.undo", json!({})).unwrap();
    assert!(
        shell.editor().document().chain(chains[1]).is_some(),
        "undone as one step"
    );
    assert_eq!(chain_devices(&shell, chains[1]), vec![device]);
    let _ = rack;
}

// Spec: racks R25
#[test]
fn the_device_list_shows_racks_chain_by_chain() {
    let Set { mut shell, .. } = set();
    let (outer, chains) = rack_with_chains(&mut shell, "audio_effect", 2);
    add_to_chain(&mut shell, chains[0], effect("First"));
    let inner = add_to_chain(&mut shell, chains[1], rack_of("audio_effect")).device;
    let inner_chain: ChainId = shell.new_id();
    shell
        .invoke(
            "chain.add",
            json!({ "rack": inner, "index": 0, "chain": { "id": inner_chain, "name": "Deep chain", "devices": [] } }),
        )
        .unwrap();
    let deep = add_to_chain(&mut shell, inner_chain, effect("Deep")).device;
    let list = state(&shell).device_list;
    let shape: Vec<(usize, &str)> = list.iter().map(|e| (e.depth, e.label.as_str())).collect();
    assert_eq!(
        shape,
        [
            (0, "Audio Effect Rack"),
            (1, "Chain"),
            (2, "First"),
            (1, "Chain"),
            (2, "Audio Effect Rack"),
            (3, "Deep chain"),
            (4, "Deep"),
        ]
        .map(|(depth, label)| (depth, label))
    );
    assert_eq!(list[0].device, Some(outer));
    assert_eq!(list[1].chain, Some(chains[0]));
    assert_eq!(list[6].device, Some(deep));
}

// Spec: racks R21
#[test]
fn chain_rows_carry_what_the_list_draws() {
    let Set {
        mut shell, midi, ..
    } = set();
    select_track(&mut shell, midi);
    let (rack, chains) = rack_with_chains(&mut shell, "instrument", 2);
    shell
        .invoke(
            "chain.set_gain",
            json!({ "chain": chains[1], "volume": 0.5 }),
        )
        .unwrap();
    shell
        .invoke("chain.set_pan", json!({ "chain": chains[1], "pan": -0.25 }))
        .unwrap();
    shell
        .invoke(
            "chain.set_enabled",
            json!({ "chain": chains[0], "enabled": false }),
        )
        .unwrap();
    shell
        .invoke("chain.hot_swap", json!({ "chain": chains[0] }))
        .unwrap();
    let rows = frame(&shell, rack).chains;
    assert!(!rows[0].enabled && rows[0].hot_swap && !rows[1].hot_swap);
    assert_eq!((rows[1].volume, rows[1].pan), (0.5, -0.25));
    assert!(rows.iter().all(|r| r.has_mixer));
    // Arming a device ends the chain's, and the other way round.
    let device = add_to_chain(&mut shell, chains[1], effect("Gain")).device;
    shell
        .invoke("device.hot_swap", json!({ "device": device }))
        .unwrap();
    assert!(!frame(&shell, rack).chains[0].hot_swap);
    shell
        .invoke("chain.hot_swap", json!({ "chain": chains[1] }))
        .unwrap();
    assert!(state(&shell).hot_swap.is_none() && frame(&shell, rack).chains[1].hot_swap);
    // A MIDI effect rack's chains have no Volume and Pan.
    let midi_rack = add_to_track(
        &mut shell,
        json!({ "kind": { "rack": { "kind": "midi_effect" } }, "name": "Rack", "role": "midi_effect" }),
    )
    .device;
    let id: ChainId = shell.new_id();
    shell
        .invoke(
            "chain.add",
            json!({ "rack": midi_rack, "index": 0, "chain": { "id": id, "name": "", "devices": [] } }),
        )
        .unwrap();
    assert!(frame(&shell, midi_rack).chains.iter().all(|r| !r.has_mixer));
}

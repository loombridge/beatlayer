//! A drum rack in the Device View (WP-5.6; the drum racks spec, R11 to R13,
//! R19 to R21), headless: the pad view with its overview, the three states
//! of a pad, selecting a pad and following Receive, the Hot-Swap Sample
//! button, Auto Select, and the view column's switches. Each gesture runs
//! the registered command an agent runs.

use crate::common;
use bl_model::{ChainId, DeviceId, RackView, TrackKind};
use bl_shell::device_commands::{DeviceAdded, DeviceFrame, RackFrame};
use bl_shell::pad_commands::{PadFrame, PadKind, PadSwitch};
use bl_ui::drum_view::PadLayout;
use bl_ui::rack_view::{self, RackLayout};
use common::{Harness, Window};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2};
use serde_json::{Value, json};

/// The default set with its first MIDI track selected.
fn harness() -> Harness {
    let mut h = Harness::manual_with_title_bar(Window::Main);
    h.settle();
    let midi = h
        .shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Midi)
        .map(bl_model::Track::id)
        .unwrap();
    h.shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [midi] }))
        .unwrap();
    h.settle();
    h
}

fn gain(name: &str) -> Value {
    let t = bl_engine::testing::gain_type();
    json!({ "kind": t.kind, "name": name, "role": t.role })
}

/// A drum rack on the shown track with a pad on each of `notes`, each a
/// chain named `Pad <note>`.
fn kit(h: &mut Harness, notes: &[u8]) -> (DeviceId, Vec<ChainId>) {
    let added: DeviceAdded = serde_json::from_value(
        h.shell
            .run(
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
    let rack = added.device;
    let mut chains = Vec::new();
    for &note in notes {
        let dropped = h
            .shell
            .run(
                "pad.drop",
                json!({ "rack": rack, "note": note, "device": gain("Gain") }),
            )
            .unwrap();
        let chain: ChainId = serde_json::from_value(dropped["chains"][0].clone()).unwrap();
        h.shell
            .run(
                "chain.rename",
                json!({ "chain": chain, "name": format!("Pad {note}") }),
            )
            .unwrap();
        chains.push(chain);
    }
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
    h.settle();
    (rack, chains)
}

fn view(h: &mut Harness) -> Rect {
    let nodes = Harness::nodes(&h.settle());
    let b = Harness::node(&nodes, "Device View").bounds.unwrap();
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

fn layout_of(h: &mut Harness, rack: DeviceId) -> RackLayout {
    let rect = view(h);
    rack_view::layouts(&h.shell, rect)
        .into_iter()
        .find(|(id, _)| *id == rack)
        .unwrap_or_else(|| panic!("no layout for {rack}"))
        .1
}

fn rack_state(h: &Harness, rack: DeviceId) -> RackFrame {
    fn find(frames: &[DeviceFrame], id: DeviceId) -> Option<RackFrame> {
        frames.iter().find_map(|d| {
            if d.id == id {
                d.rack.clone()
            } else {
                d.rack.as_ref().and_then(|r| find(&r.devices, id))
            }
        })
    }
    find(&h.shell.device_view_state().devices, rack).expect("the rack is shown")
}

fn pads(h: &Harness, rack: DeviceId) -> PadFrame {
    rack_state(h, rack).pads.expect("a drum rack has pads")
}

fn pad_layout_of(h: &mut Harness, rack: DeviceId) -> PadLayout {
    layout_of(h, rack).pads.expect("the pad view shows")
}

fn press(at: Pos2, pressed: bool, modifiers: Modifiers) -> Event {
    Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers,
    }
}

fn click(h: &mut Harness, at: Pos2) {
    h.frame(vec![Event::PointerMoved(at)]);
    for pressed in [true, false] {
        h.frame(vec![press(at, pressed, Modifiers::NONE)]);
    }
    h.settle();
}

fn hover(h: &mut Harness, at: Pos2) {
    h.frame(vec![Event::PointerMoved(at)]);
    h.settle();
}

fn drag(h: &mut Harness, from: Pos2, to: Pos2) {
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![press(from, true, Modifiers::NONE)]);
    for step in 1..=6 {
        let t = step as f32 / 6.0;
        h.frame(vec![Event::PointerMoved(from + (to - from) * t)]);
    }
    h.frame(vec![press(to, false, Modifiers::NONE)]);
    h.settle();
}

fn last_step(h: &Harness) -> String {
    h.shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

// Spec: drum-racks R11
#[test]
fn the_pad_view_shows_sixteen_pads_and_moves_by_group_or_row() {
    let mut h = harness();
    let (rack, _) = kit(&mut h, &[]);
    let frame = pads(&h, rack);
    assert_eq!(frame.first, 36, "a new drum rack shows C1 to D#2");
    let at = pad_layout_of(&mut h, rack);
    assert_eq!(at.pads.len(), 16);
    assert_eq!(
        at.groups.len(),
        8,
        "the overview shows eight groups of sixteen"
    );
    // The notes rise left to right and from the bottom row up.
    for n in 0..15 {
        let (a, b) = (&at.pads[n], &at.pads[n + 1]);
        assert_eq!(b.note, a.note + 1);
        if n % 4 != 3 {
            assert!(
                b.pad.left() > a.pad.left() && b.pad.top() == a.pad.top(),
                "{n}"
            );
        }
    }
    assert!(
        at.pads[4].pad.bottom() < at.pads[0].pad.top(),
        "the next row is above"
    );
    // Dragging the overview's selector moves the view a group.
    let from = at.selector.center();
    let to = pos2(from.x, from.y - at.groups[0].height());
    drag(&mut h, from, to);
    assert_ne!(pads(&h, rack).first, 36, "the selector moved the pads");
    assert_eq!(pads(&h, rack).first % 4, 0);
    // Up and Down with a pad focused move a group, and with Cmd (Alt on
    // Windows and Linux) a row.
    let layout = pad_layout_of(&mut h, rack);
    let first = pads(&h, rack).first;
    click(&mut h, layout.pads[0].pad.center());
    h.key(Key::ArrowUp, Modifiers::NONE);
    assert_eq!(pads(&h, rack).first, (first + 16).min(112));
    let first = pads(&h, rack).first;
    let rows = if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD
    } else {
        Modifiers::ALT
    };
    h.key(Key::ArrowDown, rows);
    assert_eq!(pads(&h, rack).first, first - 4);
    // The view is no undo step, and the set keeps it.
    assert_ne!(last_step(&h), "Change \"Chain On\"");
}

// Spec: drum-racks R12
#[test]
fn empty_single_and_multi_pads_show_and_offer_what_they_should() {
    let mut h = harness();
    let (rack, chains) = kit(&mut h, &[36, 37]);
    // A second chain on 37 makes it a Multi pad.
    h.shell
        .invoke("chain.duplicate", json!({ "chain": chains[1] }))
        .unwrap();
    h.settle();
    let frame = pads(&h, rack);
    let (single, multi, empty) = (&frame.pads[0], &frame.pads[1], &frame.pads[2]);
    assert_eq!(
        (single.kind, single.label.as_str()),
        (PadKind::Single, "Pad 36")
    );
    assert_eq!(
        (multi.kind, multi.label.as_str()),
        (PadKind::Multi, "Multi")
    );
    assert_eq!((empty.kind, empty.label.as_str()), (PadKind::Empty, "D1"));
    // Hovering an empty pad names the note and its General MIDI sound in the
    // Status Bar.
    let layout = pad_layout_of(&mut h, rack);
    hover(&mut h, layout.pads[2].pad.center());
    assert_eq!(h.shell.hint(), Some("D1 (38): Acoustic Snare"));
    hover(&mut h, pos2(0.0, 0.0));
    assert_eq!(h.shell.hint(), None, "leaving the pad clears it");
    // Mute lights while a pad's chain is off; on a Multi pad it acts on both
    // chains and shows a mixed state while they differ.
    let nodes = Harness::nodes(&h.settle());
    let mute = Harness::node(&nodes, "Mute of Pad C1: Pad 36");
    assert_eq!(mute.value, "off");
    let at = layout.pads[0].mute.center();
    click(&mut h, at);
    assert_eq!(pads(&h, rack).pads[0].mute, PadSwitch::On);
    assert!(
        !h.shell
            .editor()
            .document()
            .chain(chains[0])
            .unwrap()
            .mixer
            .enabled
    );
    h.shell
        .invoke(
            "chain.set_enabled",
            json!({ "chain": chains[1], "enabled": false }),
        )
        .unwrap();
    h.settle();
    assert_eq!(pads(&h, rack).pads[1].mute, PadSwitch::Mixed);
    let at = layout.pads[1].mute.center();
    click(&mut h, at);
    assert_eq!(
        pads(&h, rack).pads[1].mute,
        PadSwitch::On,
        "a mixed pad turns on"
    );
    // A solo click solos every chain of the pad.
    let at = layout.pads[1].solo.center();
    click(&mut h, at);
    assert_eq!(pads(&h, rack).pads[1].solo, PadSwitch::On);
    // The Multi pad's Rename is off; a single pad's is on.
    let nodes = Harness::nodes(&h.settle());
    assert!(
        nodes
            .iter()
            .all(|n| !n.label.starts_with("Hot-Swap Sample"))
    );
    let single_pad = Harness::node(&nodes, "Pad C1: Pad 36");
    assert!(!single_pad.disabled);
    let multi_pad = Harness::node(&nodes, "Pad C#1: Multi");
    assert_eq!(
        multi_pad.description,
        "D#1 (37): Side Stick".replace("D#1", "C#1")
    );
}

// Spec: drum-racks R13
#[test]
fn a_pad_selects_its_chains_and_follows_receive() {
    let mut h = harness();
    let (rack, chains) = kit(&mut h, &[36, 38]);
    let layout = pad_layout_of(&mut h, rack);
    click(&mut h, layout.pads[2].pad.center());
    let state = rack_state(&h, rack);
    assert_eq!(state.selected_chains, [chains[1]]);
    assert_eq!(state.pads.as_ref().unwrap().selected, Some(38));
    // Changing the chain's Receive moves it to its new note's pad at once.
    h.shell
        .invoke(
            "chain.set_receive",
            json!({ "chain": chains[1], "receive": { "note": 41 } }),
        )
        .unwrap();
    h.settle();
    let frame = pads(&h, rack);
    assert_eq!(frame.pads[2].kind, PadKind::Empty);
    assert_eq!(frame.pads[5].chains, [chains[1]]);
    // Selecting a chain in the chain list selects its pad.
    h.shell
        .invoke("chain.select", json!({ "chain": chains[0] }))
        .unwrap();
    h.settle();
    assert_eq!(pads(&h, rack).selected, Some(36));
    // A chain on All Notes sits on no pad.
    h.shell
        .invoke(
            "chain.set_receive",
            json!({ "chain": chains[0], "receive": "all_notes" }),
        )
        .unwrap();
    h.settle();
    assert!(
        pads(&h, rack)
            .pads
            .iter()
            .all(|p| !p.chains.contains(&chains[0]))
    );
}

// Spec: drum-racks R19
#[test]
fn hot_swap_sample_shows_on_hover_and_swaps_the_sample() {
    use bl_media::BitDepth;
    let folder = std::env::temp_dir().join(format!("bl-drum-ui-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let wav = |name: &str, frames: usize| {
        let samples: Vec<f32> = (0..frames).map(|i| (i as f32 * 0.05).sin() * 0.2).collect();
        let bytes =
            bl_media::wav_bytes(&samples, 1, 48_000, BitDepth::Int16, bl_media::Dither::None)
                .unwrap();
        let path = folder.join(name);
        std::fs::write(&path, bytes).unwrap();
        path.to_string_lossy().into_owned()
    };
    let mut h = harness();
    let (rack, _) = kit(&mut h, &[]);
    let first = wav("Kick.wav", 1_000);
    let second = wav("Kick 2.wav", 1_400);
    h.shell
        .invoke(
            "pad.drop",
            json!({ "rack": rack, "note": 36, "samples": [first] }),
        )
        .unwrap();
    h.settle();
    let layout = pad_layout_of(&mut h, rack);
    // The button shows only while the pointer is over the pad.
    let nodes = Harness::nodes(&h.settle());
    assert!(
        nodes
            .iter()
            .all(|n| !n.label.starts_with("Hot-Swap Sample"))
    );
    hover(&mut h, layout.pads[0].pad.center());
    let nodes = Harness::nodes(&h.settle());
    assert!(
        nodes
            .iter()
            .any(|n| n.label.starts_with("Hot-Swap Sample of"))
    );
    let before = h.shell.editor().document().clone();
    click(&mut h, layout.pads[0].sample.center());
    assert_eq!(pads(&h, rack).hot_swap, Some(36));
    // Loading a sample from the browser now replaces the pad's.
    h.shell
        .invoke("browser.select", json!({ "paths": [second] }))
        .unwrap();
    h.shell.set_focused_area(bl_shell::Area::Browser);
    h.shell.invoke("browser.load_selected", json!({})).unwrap();
    h.settle();
    assert_ne!(h.shell.editor().document(), &before);
    assert_eq!(last_step(&h), "Load Sample");
    let _ = std::fs::remove_dir_all(folder);
}

// Spec: drum-racks R20
#[test]
fn auto_select_selects_each_chain_its_note_reaches() {
    let mut h = harness();
    let (rack, chains) = kit(&mut h, &[36, 38]);
    h.shell
        .invoke(
            "rack.set_view",
            json!({ "rack": rack, "view": "auto_select", "shown": true }),
        )
        .unwrap();
    // A clip on the rack's track plays a note for the second pad.
    let track = h.shell.editor().document().device_track(rack).unwrap();
    let clip: bl_model::ClipId = h.shell.new_id();
    let beat = bl_core::time::Ticks(bl_core::time::TICKS_PER_QUARTER);
    h.shell
        .run(
            "clip.create",
            json!({
                "track": track,
                "clip": bl_model::Clip::new(
                    clip,
                    bl_core::time::Ticks::ZERO,
                    beat * 4,
                    bl_model::ClipData::Midi(bl_model::MidiData::new([bl_model::Note::new(
                        38,
                        beat,
                        beat,
                        100,
                    )])),
                ),
            }),
        )
        .unwrap();
    h.settle();
    h.shell.invoke("transport.play", json!({})).unwrap();
    h.wait("the chain the note reached", |h| {
        rack_state(h, rack).selected_chains == [chains[1]]
    });
    let selected = rack_state(&h, rack).selected_chains;
    h.shell.invoke("transport.stop", json!({})).unwrap();
    assert_eq!(
        selected,
        [chains[1]],
        "the chain the note reached is selected"
    );
    assert_eq!(pads(&h, rack).selected, Some(38), "and so is its pad");
}

// Spec: drum-racks R21
#[test]
fn the_view_column_shows_input_output_send_and_return() {
    let mut h = harness();
    let (rack, _) = kit(&mut h, &[36]);
    let layout = layout_of(&mut h, rack);
    let order: Vec<RackView> = layout.selectors.iter().map(|(v, _)| *v).collect();
    assert_eq!(
        order,
        [
            RackView::Macros,
            RackView::Variations,
            RackView::Pads,
            RackView::ChainList,
            RackView::Devices,
            RackView::InputOutput,
            RackView::Send,
            RackView::Returns,
            RackView::AutoSelect
        ],
        "the pad view between the Macro Controls and the chain list, then the four switches"
    );
    // With every switch off a drum chain's row has the usual controls only.
    let list = layout.chain_list.as_ref().unwrap();
    assert!(list.rows[0].io.is_none() && list.returns.is_none());
    // Input/Output shows each chain's Preview, Receive, Play and Choke.
    let at = layout
        .selectors
        .iter()
        .find(|(v, _)| *v == RackView::InputOutput)
        .unwrap()
        .1;
    click(&mut h, at.center());
    let layout = layout_of(&mut h, rack);
    let io = layout.chain_list.as_ref().unwrap().rows[0]
        .io
        .expect("the columns show");
    assert!(io.receive.left() > io.preview.right() - 1.0);
    let nodes = Harness::nodes(&h.settle());
    for label in [
        "Receive of Pad 36",
        "Play of Pad 36",
        "Choke of Pad 36",
        "Preview of Pad 36",
    ] {
        Harness::node(&nodes, label);
    }
    assert_eq!(Harness::node(&nodes, "Receive of Pad 36").value, "C1");
    // Return shows the return chains' section; Send, one slider per return.
    h.shell
        .invoke(
            "chain.create",
            json!({ "rack": rack, "device": gain("Gain"), "returns": true }),
        )
        .unwrap();
    h.settle();
    for view in [RackView::Returns, RackView::Send] {
        let layout = layout_of(&mut h, rack);
        let at = layout.selectors.iter().find(|(v, _)| *v == view).unwrap().1;
        click(&mut h, at.center());
    }
    let layout = layout_of(&mut h, rack);
    let list = layout.chain_list.as_ref().unwrap();
    assert!(list.returns.is_some(), "the return section shows");
    assert_eq!(list.rows.len(), 2, "a drum chain and a return chain");
    assert!(list.rows[0].sends[0].is_some() && list.rows[0].sends[1].is_none());
    assert!(
        list.rows[1].audio_to.is_some(),
        "a return chain has Audio To"
    );
}

// Spec: drum-racks R7, racks R5
#[test]
fn dragged_drum_chains_stay_before_the_return_chains() {
    let mut h = harness();
    let (rack, chains) = kit(&mut h, &[36, 37, 38, 39]);
    h.shell
        .invoke(
            "chain.create",
            json!({ "rack": rack, "device": gain("Gain"), "returns": true }),
        )
        .unwrap();
    for (chain, mode) in [(chains[0], "replace"), (chains[1], "toggle")] {
        h.shell
            .run("chain.select", json!({ "chain": chain, "mode": mode }))
            .unwrap();
    }
    h.settle();
    let layout = layout_of(&mut h, rack);
    let rows = &layout.chain_list.as_ref().unwrap().rows;
    // The first two dragged to just above the fourth.
    drag(
        &mut h,
        rows[0].name.center(),
        pos2(rows[0].name.center().x, rows[3].row.top() + 1.0),
    );
    let order: Vec<ChainId> = h
        .shell
        .editor()
        .document()
        .device(rack)
        .unwrap()
        .chains
        .iter()
        .filter(|c| !c.is_return)
        .map(|c| c.id)
        .collect();
    assert_eq!(order, [chains[2], chains[0], chains[1], chains[3]]);
    assert!(
        h.shell.message().is_none_or(|m| !m.error),
        "{:?}",
        h.shell.message()
    );
}

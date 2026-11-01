//! Similar sample swapping, headless (WP-9.10; the sound similarity spec, R24
//! to R27 and R30; drum racks R30; one-shot drum sampler R26, R27): the
//! buttons on Simpler's display, the display's menu, the keys S-SWAP-001 to
//! S-SWAP-004 on Simpler, Drum Sampler and a Drum Rack's pad, and the arrows
//! over the pads while Option is held (S-SWAP-005, I-HOLD-004). Each control
//! runs the registered command an agent runs.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::common::{self, Harness, Node, Window};
use bl_model::{DeviceId, TrackKind};
use bl_shell::browser::places::create_user_library;
use bl_shell::device_commands::{DeviceAdded, DeviceFrame, RackFrame, SimilarState};
use bl_shell::pad_commands::PadFrame;
use bl_ui::device_view::{layout, title_rect};
use bl_ui::drum_view::PadLayout;
use bl_ui::rack_view::{self, RackLayout};
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2};
use serde_json::json;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-ui-swap-{}-{name}", std::process::id()));
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

/// A 16-bit mono WAV of a decaying tone of `hz`, `seconds` long.
fn wav(path: &Path, hz: f64, seconds: f64, rate: u32) {
    let frames = (seconds * f64::from(rate)) as u32;
    let data = frames * 2;
    let mut bytes = Vec::new();
    for chunk in [
        &b"RIFF"[..],
        &(36 + data).to_le_bytes(),
        b"WAVEfmt ",
        &16u32.to_le_bytes(),
        &1u16.to_le_bytes(),
        &1u16.to_le_bytes(),
        &rate.to_le_bytes(),
        &(rate * 2).to_le_bytes(),
        &2u16.to_le_bytes(),
        &16u16.to_le_bytes(),
        b"data",
        &data.to_le_bytes(),
    ] {
        bytes.extend_from_slice(chunk);
    }
    for i in 0..frames {
        let t = f64::from(i) / f64::from(rate);
        let sample = (t * hz * std::f64::consts::TAU).sin() * (-t * 6.0).exp() * 20_000.0;
        bytes.extend_from_slice(&(sample as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// A window on the default set with four kicks and three hi-hats, all
/// analysed, in the User Library, and its first MIDI track shown.
fn harness(scratch: &Scratch) -> (Harness, PathBuf) {
    let root = scratch.0.join("User Library");
    create_user_library(&root).unwrap();
    for (name, hz) in [
        ("Kick 50", 50.0),
        ("Kick 55", 55.0),
        ("Kick 60", 60.0),
        ("Kick 65", 65.0),
    ] {
        wav(&root.join(format!("{name}.wav")), hz, 0.4, 22_050);
    }
    for (name, hz) in [
        ("Hat 6k", 6_000.0),
        ("Hat 7k", 7_000.0),
        ("Hat 8k", 8_000.0),
    ] {
        wav(&root.join(format!("{name}.wav")), hz, 0.12, 22_050);
    }
    let mut h = Harness::manual_in(Window::Main, &scratch.0.join("config"));
    h.size += egui::vec2(0.0, bl_ui::layout::TITLE_BAR);
    h.title_bar = true;
    h.shell.set_device_types(bl_devices::device_types());
    h.shell.pace_analysis_by_hand();
    h.shell.set_user_library(Some(root.clone()));
    assert!(h.shell.wait_for_browser_index(Duration::from_secs(60)));
    h.shell.release_analysis(7);
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
    h.shell.set_focused_area(bl_shell::Area::Detail);
    h.settle();
    (h, root)
}

fn add(h: &mut Harness, kind: &bl_engine::DeviceType) -> DeviceId {
    let added: DeviceAdded = serde_json::from_value(
        h.shell
            .run(
                "device.add",
                json!({ "device": { "kind": kind.kind, "name": kind.name, "role": kind.role } }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
}

fn load(h: &mut Harness, device: DeviceId, file: &Path) {
    h.shell
        .run(
            "device.load_sample",
            json!({ "device": device, "path": file.to_string_lossy() }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": device }))
        .unwrap();
    h.settle();
}

fn similar(h: &mut Harness, device: DeviceId) -> SimilarState {
    serde_json::from_value(
        h.shell
            .run("device.similar", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap()
}

fn sample_name(h: &Harness, device: DeviceId) -> String {
    let media = h
        .shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .media_at(0)
        .unwrap();
    h.shell
        .media_store()
        .entry(bl_media::ContentHash::from_bytes(media.0))
        .unwrap()
        .path
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn nodes(h: &mut Harness) -> Vec<Node> {
    Harness::nodes(&h.settle())
}

fn button<'a>(nodes: &'a [Node], label: &str) -> &'a Node {
    nodes
        .iter()
        .find(|n| n.role == Role::Button && n.label == label)
        .unwrap_or_else(|| panic!("no button labelled {label:?}"))
}

fn centre(node: &Node) -> Pos2 {
    let b = node.bounds.unwrap();
    pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
}

fn click(h: &mut Harness, at: Pos2) {
    h.frame(vec![Event::PointerMoved(at)]);
    for pressed in [true, false] {
        h.frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }]);
    }
    h.settle();
}

fn right_click(h: &mut Harness, at: Pos2) {
    h.frame(vec![Event::PointerMoved(at)]);
    for pressed in [true, false] {
        h.frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        }]);
    }
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

/// What the keys S-SWAP-001 to S-SWAP-004 press: Cmd on macOS, Ctrl
/// elsewhere, with an arrow.
fn press(h: &mut Harness, key: Key) {
    h.key(key, common::command());
    h.settle();
}

// Spec: sound-similarity R25, R27, R30. Simpler's display has Previous
// Similar, Next Similar and Show Similar Files buttons: the first is
// disabled with its reason while the sample is the reference, and a click on
// Next swaps in one undo step.
#[test]
fn simplers_display_has_the_swap_buttons() {
    let scratch = Scratch::new("buttons");
    let (mut h, root) = harness(&scratch);
    let device = add(&mut h, &bl_devices::sampler_type());
    load(&mut h, device, &root.join("Kick 50.wav"));
    let seen = nodes(&mut h);
    let previous = button(&seen, "Previous Similar Sample");
    assert!(previous.disabled, "the sample is the reference");
    assert!(
        previous.description.contains("reference"),
        "{}",
        previous.description
    );
    let next = button(&seen, "Next Similar Sample");
    assert!(!next.disabled);
    let show = button(&seen, "Show Similar Files");
    assert!(!show.disabled);
    // At the top right of the waveform, in this order.
    let (p, n, s) = (centre(previous), centre(next), centre(show));
    assert!(p.x < n.x && n.x < s.x && (p.y - n.y).abs() < 1.0);

    h.take_events();
    click(&mut h, n);
    assert_eq!(last_step(&h), "Swap Sample");
    assert_ne!(sample_name(&h, device), "Kick 50");
    let moved = similar(&mut h, device);
    assert_eq!(moved.position, Some(1));
    let seen = nodes(&mut h);
    assert!(!button(&seen, "Previous Similar Sample").disabled);
    click(&mut h, centre(button(&seen, "Previous Similar Sample")));
    assert_eq!(sample_name(&h, device), "Kick 50");
    // A press on a button doesn't audition the waveform.
    assert!(!h.take_events().iter().any(
        |e| matches!(e, bl_ui::main_window::UiEvent::Invoke(i) if i.id == "device.audition_sample")
    ));

    // Show Similar Files lists the reference's neighbours in the browser.
    click(&mut h, centre(button(&seen, "Show Similar Files")));
    assert_eq!(
        h.shell
            .browser_view()
            .similar_reference()
            .map(|p| p.file_name().unwrap().to_owned()),
        Some("Kick 50.wav".into())
    );
}

// Spec: sound-similarity R25. The display's menu holds Show Similar Files,
// Save as Similarity Reference and Return to Reference, the last two
// disabled with the reason while they have nothing to do.
#[test]
fn simplers_menu_keeps_and_returns_to_the_reference() {
    let scratch = Scratch::new("menu");
    let (mut h, root) = harness(&scratch);
    let device = add(&mut h, &bl_devices::sampler_type());
    load(&mut h, device, &root.join("Kick 50.wav"));
    let seen = nodes(&mut h);
    let next = centre(button(&seen, "Next Similar Sample"));
    click(&mut h, next);
    let first = sample_name(&h, device);
    // Right-click on the waveform, away from the buttons.
    let show = centre(button(&seen, "Show Similar Files"));
    let waveform = pos2(show.x - 120.0, show.y + 30.0);
    right_click(&mut h, waveform);
    let open = nodes(&mut h);
    let item = |label: &str| {
        open.iter()
            .find(|n| n.label == label)
            .unwrap_or_else(|| panic!("no menu item {label:?}"))
    };
    assert!(!item("Show Similar Files").disabled);
    assert!(!item("Save as Similarity Reference").disabled);
    assert!(!item("Return to Reference").disabled);
    click(&mut h, centre(item("Save as Similarity Reference")));
    assert_eq!(last_step(&h), "Save as Similarity Reference");
    assert_eq!(
        h.shell
            .editor()
            .document()
            .device(device)
            .unwrap()
            .similarity_reference,
        h.shell
            .editor()
            .document()
            .device(device)
            .unwrap()
            .media_at(0)
    );
    // Holding the reference now: both are disabled, with the reason.
    right_click(&mut h, waveform);
    let open = nodes(&mut h);
    for label in ["Save as Similarity Reference", "Return to Reference"] {
        let row = open.iter().find(|n| n.label == label).unwrap();
        assert!(row.disabled, "{label}");
        assert!(row.description.contains("reference"), "{}", row.description);
    }
    h.key(Key::Escape, Modifiers::NONE);
    let _ = first;
}

// Spec: sound-similarity R24; S-SWAP-001 to S-SWAP-004. The four keys act on
// the selected Simpler and on the selected Drum Sampler.
#[test]
fn the_swap_keys_act_on_simpler_and_drum_sampler() {
    for kind in [bl_devices::sampler_type(), bl_devices::drum_sampler_type()] {
        let scratch = Scratch::new(&format!("keys-{}", kind.name).replace(' ', "-"));
        let (mut h, root) = harness(&scratch);
        let device = add(&mut h, &kind);
        load(&mut h, device, &root.join("Kick 50.wav"));
        let name = &kind.name;
        // Cmd-Left at the start of the list does nothing.
        press(&mut h, Key::ArrowLeft);
        assert_eq!(sample_name(&h, device), "Kick 50", "{name}");
        // S-SWAP-001
        press(&mut h, Key::ArrowRight);
        assert_eq!(last_step(&h), "Swap Sample", "{name}");
        let second = sample_name(&h, device);
        assert_ne!(second, "Kick 50", "{name}");
        // S-SWAP-003, then another swap and S-SWAP-004.
        press(&mut h, Key::ArrowUp);
        assert_eq!(last_step(&h), "Save as Similarity Reference", "{name}");
        press(&mut h, Key::ArrowRight);
        assert_ne!(sample_name(&h, device), second, "{name}");
        press(&mut h, Key::ArrowDown);
        assert_eq!(last_step(&h), "Return to Reference", "{name}");
        assert_eq!(sample_name(&h, device), second, "{name}");
        // S-SWAP-002 goes back one in the list.
        press(&mut h, Key::ArrowRight);
        press(&mut h, Key::ArrowLeft);
        assert_eq!(sample_name(&h, device), second, "{name}");
    }
}

fn rack_view_rect(h: &mut Harness) -> Rect {
    let nodes = Harness::nodes(&h.settle());
    let b = Harness::node(&nodes, "Device View").bounds.unwrap();
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

fn rack_layout(h: &mut Harness, rack: DeviceId) -> RackLayout {
    let rect = rack_view_rect(h);
    rack_view::layouts(&h.shell, rect)
        .into_iter()
        .find(|(id, _)| *id == rack)
        .unwrap()
        .1
}

fn pad_layout(h: &mut Harness, rack: DeviceId) -> PadLayout {
    rack_layout(h, rack).pads.expect("the pad view shows")
}

fn pads(h: &Harness, rack: DeviceId) -> PadFrame {
    fn find(frames: &[DeviceFrame], id: DeviceId) -> Option<RackFrame> {
        frames.iter().find_map(|d| {
            if d.id == id {
                d.rack.clone()
            } else {
                d.rack.as_ref().and_then(|r| find(&r.devices, id))
            }
        })
    }
    find(&h.shell.device_view_state().devices, rack)
        .expect("the rack is shown")
        .pads
        .expect("a drum rack has pads")
}

/// A drum rack with Kick 50 on note 36 and a pad of a gain on 37.
fn kit(h: &mut Harness, root: &Path) -> (DeviceId, DeviceId) {
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
    let dropped = h
        .shell
        .run(
            "pad.drop",
            json!({ "rack": added.device, "note": 36,
                "samples": [root.join("Kick 50.wav").to_string_lossy()] }),
        )
        .unwrap();
    let chain: bl_model::ChainId = serde_json::from_value(dropped["chains"][0].clone()).unwrap();
    let sampler = h
        .shell
        .editor()
        .document()
        .chain(chain)
        .unwrap()
        .devices
        .iter()
        .find(|d| !d.media.is_empty())
        .map(|d| d.id)
        .unwrap();
    let gain = bl_engine::testing::gain_type();
    h.shell
        .run(
            "pad.drop",
            json!({ "rack": added.device, "note": 37,
                "device": { "kind": gain.kind, "name": "Gain", "role": gain.role } }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
    h.settle();
    (added.device, sampler)
}

fn alt() -> Modifiers {
    Modifiers {
        alt: true,
        ..Modifiers::NONE
    }
}

// Spec: drum-racks R30; sound-similarity R24, R26, R30; S-SWAP-005,
// I-HOLD-004. Holding Option over a Drum Rack's pads shows Previous Similar
// and Next Similar arrows on each pad whose chain plays a sample, a click on
// one swaps that pad, releasing Option hides them, and the four keys act on
// the selected pad.
#[test]
fn option_held_over_the_pads_shows_the_swap_arrows_and_the_keys_act_on_the_pad() {
    let scratch = Scratch::new("pads");
    let (mut h, root) = harness(&scratch);
    let (rack, sampler) = kit(&mut h, &root);
    let at = pad_layout(&mut h, rack);
    let pad = at.pads.iter().find(|p| p.note == 36).unwrap().pad;
    let other = at.pads.iter().find(|p| p.note == 37).unwrap().pad;

    // No arrows without Option.
    let seen = nodes(&mut h);
    assert!(!seen.iter().any(|n| n.label == "Next Similar Sample"));
    // Option held with the pointer over the pads.
    h.frame(vec![
        Event::ModifiersChanged(alt()),
        Event::PointerMoved(pad.center()),
    ]);
    let seen = Harness::nodes(&h.frame(vec![]));
    let arrows: Vec<&Node> = seen
        .iter()
        .filter(|n| n.role == Role::Button && n.label.ends_with("Similar Sample"))
        .collect();
    assert_eq!(arrows.len(), 2, "one pad plays a sample: a pair of arrows");
    let previous = button(&seen, "Previous Similar Sample");
    let next = button(&seen, "Next Similar Sample");
    assert!(previous.disabled, "the reference is the first");
    assert!(!next.disabled);
    let (p, n) = (centre(previous), centre(next));
    assert!(
        pad.contains(p) && pad.contains(n) && p.x < pad.center().x && n.x > pad.center().x,
        "over the pad's left and right edges"
    );
    assert!(!other.contains(n));

    // A click on Next swaps that pad's sample.
    let before = sample_name(&h, sampler);
    h.frame(vec![Event::ModifiersChanged(alt()), Event::PointerMoved(n)]);
    for pressed in [true, false] {
        h.frame(vec![
            Event::ModifiersChanged(alt()),
            Event::PointerButton {
                pos: n,
                button: PointerButton::Primary,
                pressed,
                modifiers: alt(),
            },
        ]);
    }
    assert_eq!(last_step(&h), "Swap Sample");
    assert_ne!(sample_name(&h, sampler), before);

    // Releasing Option hides them.
    h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    let seen = nodes(&mut h);
    assert!(!seen.iter().any(|n| n.label == "Next Similar Sample"));

    // The keys act on the selected pad's sampler.
    let at = pad_layout(&mut h, rack);
    let pad = at.pads.iter().find(|p| p.note == 36).unwrap().pad;
    click(&mut h, pad.center());
    assert_eq!(pads(&h, rack).selected, Some(36));
    let second = sample_name(&h, sampler);
    press(&mut h, Key::ArrowRight);
    assert_ne!(sample_name(&h, sampler), second, "S-SWAP-001");
    press(&mut h, Key::ArrowLeft);
    assert_eq!(sample_name(&h, sampler), second, "S-SWAP-002");
    press(&mut h, Key::ArrowUp);
    assert_eq!(last_step(&h), "Save as Similarity Reference", "S-SWAP-003");
    press(&mut h, Key::ArrowRight);
    press(&mut h, Key::ArrowDown);
    assert_eq!(last_step(&h), "Return to Reference", "S-SWAP-004");
    assert_eq!(sample_name(&h, sampler), second);
}

// Spec: drum-racks R11, R30. With a pad that plays nothing to swap selected,
// Cmd-Up and Cmd-Down (Alt on Windows and Linux) still move the pads by a
// row.
#[test]
fn a_pad_with_nothing_to_swap_leaves_the_row_keys_to_the_pad_view() {
    let scratch = Scratch::new("rows");
    let (mut h, root) = harness(&scratch);
    let (rack, _) = kit(&mut h, &root);
    let at = pad_layout(&mut h, rack);
    let gain_pad = at.pads.iter().find(|p| p.note == 37).unwrap().pad;
    click(&mut h, gain_pad.center());
    let first = pads(&h, rack).first;
    let rows = if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD
    } else {
        Modifiers::ALT
    };
    h.key(Key::ArrowDown, rows);
    assert_eq!(pads(&h, rack).first, first - 4, "a row down");
}

/// The nodes with `device`'s context menu open.
fn open_device_menu(h: &mut Harness, device: DeviceId) -> Vec<Node> {
    let view = rack_view_rect(h);
    let frame = layout(&h.shell, view)
        .into_iter()
        .find(|(id, _)| *id == device)
        .unwrap()
        .1;
    right_click(h, title_rect(frame, false).center());
    nodes(h)
}

// Spec: one-shot-drum-sampler R27. Drum Sampler's display isn't drawn yet, so
// its menu holds Show Similar Files, Save as Similarity Reference and Return
// to Reference, which run the commands an agent runs.
#[test]
fn drum_samplers_menu_holds_the_similar_sample_items() {
    let scratch = Scratch::new("strike-menu");
    let (mut h, root) = harness(&scratch);
    let device = add(&mut h, &bl_devices::drum_sampler_type());
    load(&mut h, device, &root.join("Kick 50.wav"));
    h.shell
        .run(
            "device.swap_similar",
            json!({ "device": device, "direction": "next" }),
        )
        .unwrap();
    h.settle();
    let open = open_device_menu(&mut h, device);
    let item = |label: &str| {
        open.iter()
            .find(|n| n.label == label)
            .unwrap_or_else(|| panic!("no menu item {label:?}"))
    };
    assert!(!item("Show Similar Files").disabled);
    assert!(!item("Return to Reference").disabled);
    click(&mut h, centre(item("Return to Reference")));
    assert_eq!(last_step(&h), "Return to Reference");
    assert_eq!(sample_name(&h, device), "Kick 50");
    let open = open_device_menu(&mut h, device);
    let row = open
        .iter()
        .find(|n| n.label == "Return to Reference")
        .unwrap();
    assert!(row.disabled, "it holds the reference");
    click(
        &mut h,
        centre(
            open.iter()
                .find(|n| n.label == "Show Similar Files")
                .unwrap(),
        ),
    );
    assert!(h.shell.browser_view().similar_reference().is_some());
}

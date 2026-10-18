//! The sampler devices' sample displays (WP-5.15, IMP-28; the multisampler
//! spec, R54, R55 and R61), headless: Snapshot's display with its flags,
//! loop brace and slices; Shift-drag to select; a click that auditions and
//! changes nothing; Esc; the context menu's edits; and a sample dropped on
//! the multisampler's Sample tab.

mod common;

use bl_devices::sampler::{self, LENGTH, LOOP, MODE, SLICE_BY, SLICE_REGIONS};
use bl_model::{DeviceId, LayerId};
use common::{Harness, Node, Window};
use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use serde_json::json;

/// A file dropped into the window, as the app's drag and drop gives egui.
#[derive(Debug)]
struct DroppedFile(std::path::PathBuf);

impl egui::DroppedFile for DroppedFile {
    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|error| error.to_string())
    }
}

/// A mono 16-bit WAV of a rising ramp, so edits are easy to tell.
fn wav(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bl-sample-display-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(name);
    let samples: Vec<f32> = (0..48_000).map(|i| (i as f32 / 48_000.0) * 0.5).collect();
    std::fs::write(
        &file,
        bl_media::wav_bytes(
            &samples,
            1,
            48_000,
            bl_media::BitDepth::Int16,
            bl_media::Dither::None,
        )
        .unwrap(),
    )
    .unwrap();
    file
}

/// A window with an instrument of `device_type` on a new MIDI track, holding
/// `Ramp.wav`.
fn window(device_type: &bl_engine::DeviceType) -> (Harness, DeviceId) {
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.set_device_types(bl_devices::device_types());
    h.settle();
    h.shell
        .invoke("track.create", json!({ "kind": "midi", "name": "Samples" }))
        .unwrap();
    let midi = h
        .shell
        .editor()
        .document()
        .tracks()
        .filter(|t| t.kind() == bl_model::TrackKind::Midi)
        .last()
        .map(bl_model::Track::id)
        .unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [midi] }))
        .unwrap();
    let added = h
        .shell
        .run(
            "device.add",
            json!({ "device": { "kind": device_type.kind, "name": device_type.name, "role": device_type.role } }),
        )
        .unwrap();
    let device: DeviceId = serde_json::from_value(added["device"].clone()).unwrap();
    let file = wav("Ramp.wav");
    if device_type.kind == bl_devices::multisampler_type().kind {
        h.shell
            .run(
                "device.add_layers",
                json!({ "device": device, "paths": [file.to_string_lossy()] }),
            )
            .unwrap();
    } else {
        h.shell
            .run(
                "device.load_sample",
                json!({ "device": device, "path": file.to_string_lossy() }),
            )
            .unwrap();
    }
    h.shell
        .run("device.select", json!({ "device": device }))
        .unwrap();
    h.settle();
    (h, device)
}

fn centre(b: egui::accesskit::Rect) -> Pos2 {
    pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
}

fn nodes(h: &mut Harness) -> Vec<Node> {
    Harness::nodes(&h.settle())
}

fn bounds(nodes: &[Node], label: &str) -> egui::accesskit::Rect {
    nodes
        .iter()
        .find(|n| n.label == label)
        .unwrap_or_else(|| panic!("no node labelled {label:?}"))
        .bounds
        .unwrap()
}

fn set_param(h: &mut Harness, device: DeviceId, param: bl_model::ParamId, value: f64) {
    h.shell
        .run(
            "device.set_param",
            json!({ "device": device, "param": param, "value": value }),
        )
        .unwrap();
}

fn value_of(h: &Harness, device: DeviceId, param: bl_model::ParamId) -> f64 {
    sampler::sampler_type()
        .description
        .value(
            &h.shell.editor().document().device(device).unwrap().params,
            param,
        )
        .unwrap()
}

/// A drag with `modifiers` held, in a few steps.
fn drag(h: &mut Harness, from: Pos2, to: Pos2, modifiers: Modifiers) {
    let held = Event::ModifiersChanged(modifiers);
    h.frame(vec![held.clone(), Event::PointerMoved(from)]);
    h.frame(vec![
        held.clone(),
        Event::PointerButton {
            pos: from,
            button: PointerButton::Primary,
            pressed: true,
            modifiers,
        },
    ]);
    for step in 1..=6 {
        let t = step as f32 / 6.0;
        h.frame(vec![
            held.clone(),
            Event::PointerMoved(from + (to - from) * t),
        ]);
    }
    h.frame(vec![
        held,
        Event::PointerButton {
            pos: to,
            button: PointerButton::Primary,
            pressed: false,
            modifiers,
        },
    ]);
    h.settle();
}

// Spec: multisampler R54
#[test]
fn snapshots_display_draws_its_flags_and_slices() {
    let (mut h, device) = window(&sampler::sampler_type());
    let all = nodes(&mut h);
    assert!(
        all.iter().any(|n| n.label == "Sample Display"),
        "the display cell shows the sample"
    );
    let (start, end) = (bounds(&all, "Region Start"), bounds(&all, "Region End"));
    assert!(start.x0 < end.x0, "the flags are the region's ends");
    assert!(
        !all.iter().any(|n| n.label == "Loop Start"),
        "no loop, no loop flag"
    );
    // The loop brace shows in Classic mode once the loop is on.
    set_param(&mut h, device, LOOP, 1.0);
    set_param(&mut h, device, sampler::LOOP_LENGTH, 50.0);
    let all = nodes(&mut h);
    let brace = bounds(&all, "Loop Start");
    assert!(
        brace.x0 > start.x0 && brace.x0 < end.x0,
        "the brace starts inside the region"
    );
    // Slice mode shows the slices, none of them a flag.
    set_param(&mut h, device, MODE, 2.0);
    set_param(&mut h, device, SLICE_BY, 2.0);
    set_param(&mut h, device, SLICE_REGIONS, 4.0);
    let all = nodes(&mut h);
    let slices: Vec<&Node> = all
        .iter()
        .filter(|n| {
            n.label
                .strip_prefix("Slice ")
                .is_some_and(|rest| rest.parse::<u32>().is_ok())
        })
        .collect();
    assert_eq!(
        slices.len(),
        3,
        "four equal slices have three cuts inside the region"
    );
    assert!(slices.iter().all(|n| n.description.contains("automatic")));
    assert!(
        !all.iter().any(|n| n.label == "Loop Start"),
        "Slice mode has no loop brace"
    );

    // Dragging the End flag shortens the region as one undo step.
    let steps = h.shell.editor().undo_history().len();
    let from = centre(bounds(&all, "Region End"));
    drag(&mut h, from, from - vec2(60.0, 0.0), Modifiers::NONE);
    assert!(value_of(&h, device, LENGTH) < 100.0);
    assert_eq!(
        h.shell.editor().undo_history().len(),
        steps + 1,
        "one step however far the flag went"
    );
    // Dragging the Start flag moves it alone: the End stays where it was.
    let all = nodes(&mut h);
    let end_before = bounds(&all, "Region End").x0;
    let from = centre(bounds(&all, "Region Start"));
    drag(&mut h, from, from + vec2(40.0, 0.0), Modifiers::NONE);
    let all = nodes(&mut h);
    assert!(value_of(&h, device, sampler::START) > 0.0);
    assert!((bounds(&all, "Region End").x0 - end_before).abs() < 1.5);
}

// Spec: multisampler R54, R55
#[test]
fn shift_drag_selects_and_a_click_auditions() {
    let (mut h, device) = window(&sampler::sampler_type());
    let all = nodes(&mut h);
    let display = bounds(&all, "Sample Display");
    let before = h.shell.editor().document().clone();
    let steps = h.shell.editor().undo_history().len();
    // A click with no drag auditions while the button is down, and changes
    // nothing in the set.
    let at = pos2(
        display.x0 as f32 + 60.0,
        ((display.y0 + display.y1) / 2.0) as f32,
    );
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    assert_eq!(
        h.shell.auditioning(),
        Some((device, None)),
        "playing while the button is down"
    );
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
    assert_eq!(
        h.shell.auditioning(),
        None,
        "and stopped when it is released"
    );
    assert_eq!(h.shell.editor().undo_history().len(), steps);
    assert_eq!(
        *h.shell.editor().document(),
        before,
        "the click changed nothing in the set"
    );

    // Shift-drag selects a span of the sample; no audition starts.
    drag(&mut h, at, at + vec2(50.0, 0.0), Modifiers::SHIFT);
    assert_eq!(h.shell.auditioning(), None);
    let all = nodes(&mut h);
    let selection = all
        .iter()
        .find(|n| n.label == "Selection")
        .expect("the selection shows");
    let (from, to) = selection.value.split_once(" to ").unwrap();
    let (from, to): (u64, u64) = (from.parse().unwrap(), to.parse().unwrap());
    assert!(to > from + 1_000, "{from} to {to}");
    assert_eq!(
        *h.shell.editor().document(),
        before,
        "selecting changes nothing in the set"
    );

    // Esc clears it.
    h.frame(vec![Event::PointerMoved(at)]);
    h.key(egui::Key::Escape, Modifiers::NONE);
    assert!(!nodes(&mut h).iter().any(|n| n.label == "Selection"));

    // A drag without Shift pans and zooms instead of selecting or playing.
    drag(&mut h, at, at + vec2(0.0, 40.0), Modifiers::NONE);
    assert_eq!(h.shell.auditioning(), None);
    assert!(!nodes(&mut h).iter().any(|n| n.label == "Selection"));
    assert_eq!(*h.shell.editor().document(), before);

    // The context menu's edits act on the selection: select, then Trim.
    drag(&mut h, at, at + vec2(40.0, 0.0), Modifiers::SHIFT);
    let selection = nodes(&mut h)
        .into_iter()
        .find(|n| n.label == "Selection")
        .unwrap();
    let (from, to) = selection.value.split_once(" to ").unwrap();
    let range = (from.parse::<u64>().unwrap(), to.parse::<u64>().unwrap());
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Secondary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Secondary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    let menu = nodes(&mut h);
    for label in [
        "Crop",
        "Reverse",
        "Normalize Volumes",
        "Trim",
        "Cut",
        "Fade In",
        "Fade Out",
        "Normalize",
        "Revert Edits",
    ] {
        assert!(
            menu.iter().any(|n| n.label == label),
            "the menu has {label}"
        );
    }
    let revert = menu.iter().find(|n| n.label == "Revert Edits").unwrap();
    assert!(revert.disabled, "nothing to revert yet");
    h.click(centre(bounds(&menu, "Trim")));
    h.settle();
    let held = h.shell.editor().document().device(device).unwrap().clone();
    assert_eq!(held.sample_edits.len(), 1);
    let range_after = held.sample_edits[0].range.unwrap();
    assert_eq!((range_after.from, range_after.to), range);
}

// Spec: multisampler R61
#[test]
fn a_drop_replaces_and_an_option_drop_adds_a_layer() {
    let (mut h, device) = window(&bl_devices::multisampler_type());
    let layer = |h: &Harness| {
        h.shell
            .editor()
            .document()
            .device(device)
            .unwrap()
            .layers
            .list
            .clone()
    };
    let original: LayerId = layer(&h)[0].id;
    // Narrow the layer's key zone so a new layer can be told to share it.
    h.shell
        .run(
            "device.set_layer_zone",
            json!({ "device": device, "kind": "key", "zones": [{ "layer": original, "zone": { "low": 40, "high": 50 } }] }),
        )
        .unwrap();
    let all = nodes(&mut h);
    let display = bounds(&all, "Sample Display");
    let over = pos2(
        display.x0 as f32 + 80.0,
        ((display.y0 + display.y1) / 2.0) as f32,
    );
    let hash = layer(&h)[0].media;
    let dropped = wav("Dropped.wav");
    // Dropped plainly, the file replaces the selected layer's sample and keeps
    // its zones.
    let steps = h.shell.editor().undo_history().len();
    h.frame(vec![Event::PointerMoved(over)]);
    h.dropped
        .push(std::sync::Arc::new(DroppedFile(dropped.clone())));
    h.settle();
    let after = layer(&h);
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].name, "Ramp", "the layer keeps its name");
    assert_eq!(after[0].key.low, 40);
    assert_eq!(
        h.shell.editor().undo_history().len(),
        steps + 1,
        "one undo step"
    );
    // The same bytes are the same media, so use another file for the add.
    let second = {
        let dir = dropped.parent().unwrap();
        let file = dir.join("Second.wav");
        let samples: Vec<f32> = (0..24_000).map(|i| (i as f32 / 24_000.0) * 0.25).collect();
        std::fs::write(
            &file,
            bl_media::wav_bytes(
                &samples,
                1,
                48_000,
                bl_media::BitDepth::Int16,
                bl_media::Dither::None,
            )
            .unwrap(),
        )
        .unwrap();
        file
    };
    // With Option held it adds a new layer after the selected one, with its
    // three zones.
    let held = Event::ModifiersChanged(Modifiers::ALT);
    h.frame(vec![held.clone(), Event::PointerMoved(over)]);
    h.dropped.push(std::sync::Arc::new(DroppedFile(second)));
    h.frame(vec![held]);
    h.settle();
    let after = layer(&h);
    assert_eq!(after.len(), 2, "a layer was added");
    assert_eq!(after[0].id, original);
    assert_eq!(after[1].name, "Second");
    assert_eq!(
        (after[1].key, after[1].velocity, after[1].select),
        (after[0].key, after[0].velocity, after[0].select),
        "so the two sound together"
    );
    let _ = hash;
}

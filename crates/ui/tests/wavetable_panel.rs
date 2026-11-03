//! Wavetable's panel (WP-8.11; the wavetable spec, R35 to R41), headless:
//! each oscillator's display with its drag and its drop, the filter,
//! envelope and LFO displays, the matrix as a grid, the accessibility tree
//! and the layout at every zoom.

use crate::common;
use bl_devices::wavetable::{POSITION, osc};
use bl_model::{DeviceId, ParamId, WavetableChoice};
use bl_shell::device_commands::{DeviceFrame, TITLE_BAR_HEIGHT};
use bl_ui::device_panel::{body_rect, place_shown};
use bl_ui::device_view::layout;
use common::{Harness, Window};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use serde_json::json;

fn window() -> (Harness, DeviceId) {
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.set_device_types(bl_devices::device_types());
    h.settle();
    let midi = h
        .shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == bl_model::TrackKind::Midi)
        .map(bl_model::Track::id)
        .unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [midi] }))
        .unwrap();
    let t = bl_devices::wavetable_type();
    let added = h
        .shell
        .run(
            "device.add",
            json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
        )
        .unwrap();
    let device: DeviceId = serde_json::from_value(added["device"].clone()).unwrap();
    h.shell
        .run("device.select", json!({ "device": device }))
        .unwrap();
    h.settle();
    (h, device)
}

fn frame_state(h: &Harness, device: DeviceId) -> DeviceFrame {
    h.shell
        .device_view_state()
        .devices
        .into_iter()
        .find(|d| d.id == device)
        .unwrap()
}

/// Where the display cell called `name` is drawn.
fn cell(h: &mut Harness, device: DeviceId, name: &str) -> Rect {
    let nodes = Harness::nodes(&h.settle());
    let b = Harness::node(&nodes, "Device View").bounds.unwrap();
    let view = Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    );
    let frame = layout(&h.shell, view)
        .into_iter()
        .find(|(id, _)| *id == device)
        .unwrap()
        .1;
    let shown = frame_state(h, device);
    let mut body = body_rect(frame, TITLE_BAR_HEIGHT);
    body.min.x += bl_shell::device_commands::sidechain_width(&shown);
    place_shown(shown.panel.as_ref().unwrap(), shown.expanded, body)
        .1
        .into_iter()
        .find(|(n, _, _)| n == name)
        .unwrap_or_else(|| panic!("no cell {name}"))
        .1
}

fn param(h: &Harness, device: DeviceId, param: ParamId) -> f64 {
    h.shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .params
        .get(&param)
        .copied()
        .unwrap_or_else(|| {
            bl_devices::wavetable_type()
                .description
                .param(param)
                .unwrap()
                .default
        })
}

fn steps(h: &Harness) -> usize {
    h.shell.editor().undo_history().len()
}

fn last_step(h: &Harness) -> String {
    h.shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn press(at: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

fn drag(h: &mut Harness, from: Pos2, by: egui::Vec2) {
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![press(from, true)]);
    for i in 1..=10 {
        h.frame(vec![Event::PointerMoved(from + by * (i as f32 / 10.0))]);
    }
    h.frame(vec![press(from + by, false)]);
    h.settle();
}

fn page(h: &mut Harness, device: DeviceId, page: &str) {
    h.shell
        .run(
            "device.set_page",
            json!({ "device": device, "node": "Tabs", "page": page }),
        )
        .unwrap();
    h.settle();
}

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

fn drop_file(h: &mut Harness, at: Pos2, file: &std::path::Path) {
    h.frame(vec![Event::PointerMoved(at)]);
    h.dropped
        .push(std::sync::Arc::new(DroppedFile(file.to_path_buf())));
    h.frame(Vec::new());
    h.settle();
}

fn files(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bl-wavetable-panel-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// Spec: wavetable R36
#[test]
fn a_drag_across_an_oscillators_display_moves_position_as_one_step_named_as_the_knobs() {
    let (mut h, device) = window();
    let position = osc(1, POSITION);
    let display = cell(&mut h, device, "Osc 1");
    let before = steps(&h);
    drag(
        &mut h,
        display.center() - vec2(display.width() / 4.0, 0.0),
        vec2(display.width() / 2.0, 0.0),
    );
    let moved = param(&h, device, position);
    assert!(
        (moved - 50.0).abs() < 3.0,
        "half the cell is half the table: {moved}"
    );
    assert_eq!(steps(&h), before + 1, "one step however far it went");
    let name = last_step(&h);
    assert_eq!(name, "Change \"Osc 1 Position\"");
    // The knob's drag gives the same name.
    let nodes = Harness::nodes(&h.settle());
    let knob = Harness::node(&nodes, "Osc 1 Position").bounds.unwrap();
    let at = pos2(
        ((knob.x0 + knob.x1) / 2.0) as f32,
        ((knob.y0 + knob.y1) / 2.0) as f32,
    );
    drag(&mut h, at, vec2(0.0, -20.0));
    assert_eq!(last_step(&h), name);
    // The display's data says where Position is.
    let shown = h.shell.device_display(device).unwrap();
    assert!(shown.reading("Osc 1 Position").unwrap().value > 50.0);
}

// Spec: wavetable R37
#[test]
fn a_wav_dropped_on_an_oscillator_loads_a_user_table_and_a_midi_file_is_refused() {
    let (mut h, device) = window();
    let dir = files("drop");
    let wav = dir.join("Grains.wav");
    let samples: Vec<f32> = (0..4_096 * 3)
        .map(|i| (i as f32 * 220.0 / 48_000.0 * std::f32::consts::TAU).sin() * 0.5)
        .collect();
    std::fs::write(
        &wav,
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
    let midi = dir.join("Phrase.mid");
    std::fs::write(&midi, b"MThd\0\0\0\x06\0\0\0\x01\0\x60").unwrap();
    let second = cell(&mut h, device, "Osc 2");
    let before = steps(&h);
    // A MIDI file is refused: nothing loads and nothing is recorded.
    drop_file(&mut h, second.center(), &midi);
    let document = h.shell.editor().document();
    assert_eq!(document.device(device).unwrap().wavetable_at(1), None);
    assert_eq!(steps(&h), before);
    // A WAV on the second oscillator is its table; the first keeps its own.
    drop_file(&mut h, second.center(), &wav);
    let document = h.shell.editor().document();
    let loaded = document.device(device).unwrap();
    assert!(matches!(
        loaded.wavetable_at(1),
        Some(WavetableChoice::User { frames, .. }) if *frames >= 2
    ));
    assert_eq!(loaded.wavetable_at(0), None);
    assert_eq!(steps(&h), before + 1);
    // The display draws the table it loaded.
    let shown = h.shell.device_display(device).unwrap();
    assert!(shown.series.iter().any(|s| s.name == "Osc 2 Table 1"));
}

// Spec: wavetable R37
#[test]
fn an_oscillator_is_outlined_while_a_wav_hovers_over_it() {
    let (mut h, device) = window();
    let display = cell(&mut h, device, "Osc 1");
    fn has_outline(shape: &egui::Shape, display: Rect) -> bool {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().any(|s| has_outline(s, display)),
            egui::Shape::Rect(rect) => {
                rect.stroke.width == 2.0 && display.expand(1.0).contains_rect(rect.rect)
            }
            _ => false,
        }
    }
    let outlined = |h: &mut Harness| {
        let output = h.frame(vec![Event::PointerMoved(display.center())]);
        output
            .shapes
            .iter()
            .any(|clipped| has_outline(&clipped.shape, display))
    };
    assert!(!outlined(&mut h));
    let hover = |name: &str| egui::HoveredFile {
        path: Some(std::path::PathBuf::from(name)),
        ..Default::default()
    };
    h.hovered = vec![hover("Grains.wav")];
    assert!(outlined(&mut h), "a WAV outlines it");
    h.hovered = vec![hover("Phrase.mid")];
    assert!(!outlined(&mut h), "a MIDI file doesn't");
}

/// Where the grid's cell for `source` routed to `destination` is drawn.
fn grid_cell(h: &mut Harness, source: &str, destination: &str) -> Rect {
    let nodes = Harness::nodes(&h.settle());
    let name = format!("{source} to {destination}");
    let b = Harness::node(&nodes, &name)
        .bounds
        .unwrap_or_else(|| panic!("{name} has no place"));
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

fn routes(h: &Harness, device: DeviceId) -> Vec<(String, String, f64)> {
    let device = h.shell.editor().document().device(device).unwrap().clone();
    h.shell
        .device_matrix(&device)
        .unwrap()
        .slots
        .into_iter()
        .filter_map(|slot| {
            Some((
                slot.source?.to_owned(),
                slot.destination?.to_owned(),
                slot.amount,
            ))
        })
        .collect()
}

fn click(h: &mut Harness, at: Pos2, button: PointerButton) {
    let event = |pressed| Event::PointerButton {
        pos: at,
        button,
        pressed,
        modifiers: Modifiers::NONE,
    };
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![event(true)]);
    h.frame(vec![event(false)]);
    h.settle();
}

fn key(h: &mut Harness, key: egui::Key) {
    for pressed in [true, false] {
        h.frame(vec![Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
    }
    h.settle();
}

// Spec: wavetable R39
#[test]
fn a_drag_in_the_grid_sets_an_amount_and_in_an_empty_cell_takes_a_free_slot_in_one_step() {
    let (mut h, device) = window();
    page(&mut h, device, "Matrix");
    // The grid starts with no rows: no destination has a route.
    let panel = frame_state(&h, device).panel.unwrap();
    assert!(panel.matrix_grid.as_ref().unwrap().rows.is_empty());
    h.shell
        .run(
            "device.add_destination",
            json!({ "device": device, "destination": "Osc 1 Position" }),
        )
        .unwrap();
    h.settle();
    let cell = grid_cell(&mut h, "LFO 1", "Osc 1 Position");
    let before = steps(&h);
    drag(&mut h, cell.center(), vec2(0.0, -25.0));
    // Up 25 points is +50 %, less the few points the press takes to become
    // a drag, in a slot the drag took.
    let taken = routes(&h, device);
    assert_eq!(taken.len(), 1);
    assert_eq!(
        (taken[0].0.as_str(), taken[0].1.as_str()),
        ("LFO 1", "Osc 1 Position")
    );
    let amount = taken[0].2;
    assert!((30.0..=50.0).contains(&amount), "{amount}");
    assert_eq!(
        steps(&h),
        before + 1,
        "the slot and the amount are one step"
    );
    assert_eq!(last_step(&h), "Change Modulation Route");
    // Dragging the same cell again moves its amount, a step each time.
    drag(&mut h, cell.center(), vec2(0.0, 10.0));
    let lowered = routes(&h, device)[0].2;
    assert!(lowered < amount && lowered >= amount - 20.0, "{lowered}");
    assert_eq!(steps(&h), before + 2);
    // The route is in the readback the agent reads, in the first free slot.
    let device_state = h.shell.editor().document().device(device).unwrap().clone();
    let matrix = h.shell.device_matrix(&device_state).unwrap();
    assert_eq!(matrix.slots[0].source, Some("LFO 1"));
    // The cell says its route and amount to a screen reader.
    let nodes = Harness::nodes(&h.settle());
    let node = Harness::node(&nodes, "LFO 1 to Osc 1 Position");
    assert_eq!(node.role, egui::accesskit::Role::Slider);
    assert_eq!(node.value, format!("{lowered:.0} %"));
    assert_eq!(node.numeric, Some(lowered));
    // Delete in a cell clears its route, in one step.
    click(&mut h, cell.center(), PointerButton::Primary);
    let cleared = steps(&h);
    key(&mut h, egui::Key::Delete);
    assert!(routes(&h, device).is_empty());
    assert_eq!(steps(&h), cleared + 1);
    assert_eq!(last_step(&h), "Clear Modulation Route");
    assert_eq!(
        param(&h, device, bl_devices::wavetable::slot_amount(0)),
        0.0
    );
}

// Spec: wavetable R39
#[test]
fn a_destination_is_added_from_the_chooser_and_removed_from_its_rows_menu() {
    let (mut h, device) = window();
    page(&mut h, device, "Matrix");
    let nodes = Harness::nodes(&h.settle());
    let chooser = Harness::node(&nodes, "Add Destination").bounds.unwrap();
    let at = pos2(
        ((chooser.x0 + chooser.x1) / 2.0) as f32,
        ((chooser.y0 + chooser.y1) / 2.0) as f32,
    );
    click(&mut h, at, PointerButton::Primary);
    let nodes = Harness::nodes(&h.settle());
    let item = nodes
        .iter()
        .find(|n| n.label == "Osc 2 Position" && n.role != egui::accesskit::Role::Slider)
        .expect("the chooser lists the destinations not shown")
        .bounds
        .unwrap();
    click(
        &mut h,
        pos2(
            ((item.x0 + item.x1) / 2.0) as f32,
            ((item.y0 + item.y1) / 2.0) as f32,
        ),
        PointerButton::Primary,
    );
    let panel = frame_state(&h, device).panel.unwrap();
    let grid = panel.matrix_grid.unwrap();
    assert_eq!(grid.rows.len(), 1);
    assert_eq!(grid.rows[0].destination, "Osc 2 Position");
    assert!(!grid.addable.contains(&"Osc 2 Position".to_owned()));
    // Nothing was an undo step: a row is view state.
    // A route into it, then the row's menu removes the row and the route.
    h.shell
        .run(
            "device.set_route",
            json!({ "device": device, "source": "Velocity", "destination": "Osc 2 Position", "amount": 40.0 }),
        )
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    let label = nodes
        .iter()
        .find(|n| n.role == egui::accesskit::Role::Row && n.label == "Osc 2 Position")
        .and_then(|n| n.bounds)
        .expect("the row's name");
    click(
        &mut h,
        pos2(
            ((label.x0 + label.x1) / 2.0) as f32,
            ((label.y0 + label.y1) / 2.0) as f32,
        ),
        PointerButton::Secondary,
    );
    let nodes = Harness::nodes(&h.settle());
    let remove = nodes
        .iter()
        .find(|n| n.label == "Remove")
        .expect("the row's menu")
        .bounds
        .unwrap();
    let before = steps(&h);
    click(
        &mut h,
        pos2(
            ((remove.x0 + remove.x1) / 2.0) as f32,
            ((remove.y0 + remove.y1) / 2.0) as f32,
        ),
        PointerButton::Primary,
    );
    assert!(routes(&h, device).is_empty(), "the route went with the row");
    assert_eq!(steps(&h), before + 1);
    assert_eq!(last_step(&h), "Remove Destination");
    let grid = frame_state(&h, device).panel.unwrap().matrix_grid.unwrap();
    assert!(grid.rows.is_empty() && grid.addable.contains(&"Osc 2 Position".to_owned()));
    // Undoing brings the route back, and its row with it.
    h.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(routes(&h, device).len(), 1);
    assert_eq!(routes(&h, device)[0].2, 40.0);
}

fn set(h: &mut Harness, device: DeviceId, param: ParamId, value: f64) {
    h.shell
        .invoke(
            "device.set_param",
            json!({ "device": device, "param": param, "value": value }),
        )
        .unwrap();
    h.settle();
}

/// Where the handle on the marker called `name` sits in the cell `cell`.
fn handle(h: &mut Harness, device: DeviceId, cell_name: &str, name: &str) -> Pos2 {
    let area = cell(h, device, cell_name);
    let display = h.shell.device_display(device).unwrap();
    let marker = display.markers.iter().find(|m| m.name == name).unwrap();
    let panel = frame_state(h, device).panel.unwrap();
    bl_ui::device_display::handle_center(area, &display, marker, &panel).unwrap()
}

// Spec: wavetable R38
#[test]
fn dragging_a_filters_dot_moves_its_frequency_and_resonance_as_one_step() {
    use bl_devices::wavetable::{FILTER_FREQ, FILTER_RES, filter};
    let (mut h, device) = window();
    set(&mut h, device, filter(1, FILTER_FREQ), 1_000.0);
    let before = (
        param(&h, device, filter(1, FILTER_FREQ)),
        param(&h, device, filter(1, FILTER_RES)),
    );
    let dot = handle(&mut h, device, "Filter", "Filter 1");
    let steps_before = steps(&h);
    drag(&mut h, dot, vec2(30.0, -12.0));
    assert!(
        param(&h, device, filter(1, FILTER_FREQ)) > before.0,
        "right is higher"
    );
    assert!(
        param(&h, device, filter(1, FILTER_RES)) > before.1,
        "up is more resonant"
    );
    assert_eq!(steps(&h), steps_before + 1, "both move in one step");
    assert_eq!(last_step(&h), "Change \"Filter 1 Freq\"");
    // The dot follows: it is where the new frequency puts it.
    let moved = handle(&mut h, device, "Filter", "Filter 1");
    assert!(moved.x > dot.x);
}

// Spec: wavetable R38
#[test]
fn dragging_an_envelopes_breakpoint_moves_its_time_and_level_as_one_step() {
    use bl_devices::wavetable::{ENV_ATTACK, ENV_DECAY, ENV_SUSTAIN, envelope};
    let (mut h, device) = window();
    page(&mut h, device, "Mod Sources");
    let attack = envelope(0, ENV_ATTACK);
    let before = param(&h, device, attack);
    let at = handle(&mut h, device, "Amp Envelope", "Amp Envelope Attack");
    let steps_before = steps(&h);
    drag(&mut h, at, vec2(20.0, 0.0));
    assert!(
        param(&h, device, attack) > before,
        "right is a longer attack"
    );
    assert_eq!(steps(&h), steps_before + 1);
    assert_eq!(last_step(&h), "Change \"Amp Attack\"");
    // The decay's breakpoint sets the decay time across and Sustain up.
    let sustain = envelope(0, ENV_SUSTAIN);
    let decay = envelope(0, ENV_DECAY);
    let (sustain_before, decay_before) = (param(&h, device, sustain), param(&h, device, decay));
    let at = handle(&mut h, device, "Amp Envelope", "Amp Envelope Decay");
    let steps_before = steps(&h);
    drag(&mut h, at, vec2(15.0, 20.0));
    assert!(param(&h, device, decay) > decay_before);
    assert!(
        param(&h, device, sustain) < sustain_before,
        "down is a lower Sustain"
    );
    assert_eq!(steps(&h), steps_before + 1);
}

// Spec: wavetable R40
#[test]
fn the_accessibility_tree_names_every_display_cell_and_tab() {
    use egui::accesskit::Role;
    let (mut h, device) = window();
    let nodes = Harness::nodes(&h.settle());
    for tab in ["Oscillator", "Matrix", "Mod Sources", "MIDI"] {
        assert!(
            nodes.iter().any(|n| n.role == Role::Tab && n.label == tab),
            "tab {tab}"
        );
    }
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::TabList && n.label == "Tabs")
    );
    let image = |nodes: &[common::Node], name: &str| {
        nodes
            .iter()
            .find(|n| n.role == Role::Image && n.label == name)
            .unwrap_or_else(|| panic!("no display {name}"))
            .value
            .clone()
    };
    assert!(image(&nodes, "Osc 1").contains("Osc 1 Position 0.0 %, frame 1 of 64"));
    assert!(image(&nodes, "Osc 2").contains("Osc 2 Position"));
    assert!(
        image(&nodes, "Filter").contains("Filter 1 Low-pass 24"),
        "{}",
        image(&nodes, "Filter")
    );
    for (name, marker) in [
        ("Amp Envelope", "Amp Envelope Attack"),
        ("Env 2", "Env 2 Release"),
    ] {
        page(&mut h, device, "Mod Sources");
        let nodes = Harness::nodes(&h.settle());
        assert!(
            image(&nodes, name).contains(marker),
            "{name}: {}",
            image(&nodes, name)
        );
    }
    let nodes = Harness::nodes(&h.settle());
    assert!(image(&nodes, "LFO 1").contains("LFO 1 Wave Sine, 1.00 Hz"));
    // The handles are sliders named for their breakpoints.
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::Slider && n.label == "Env 3 Decay")
    );
    page(&mut h, device, "Matrix");
    let nodes = Harness::nodes(&h.settle());
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::Table && n.label == MATRIX_CELL_NAME)
    );
    h.shell
        .run(
            "device.set_route",
            json!({ "device": device, "source": "Mod Wheel", "destination": "Filter 1 Freq", "amount": 25.0 }),
        )
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    let cell = Harness::node(&nodes, "Mod Wheel to Filter 1 Freq");
    assert_eq!((cell.role, cell.value.as_str()), (Role::Slider, "25 %"));
}

const MATRIX_CELL_NAME: &str = bl_devices::wavetable::MATRIX_CELL;

// Spec: wavetable R41
#[test]
fn at_every_zoom_the_controls_displays_and_grid_lie_inside_the_body_with_nothing_overlapping() {
    for percent in [50, 100, 200] {
        let (mut h, device) = window();
        h.shell
            .run("view.set_zoom", json!({ "percent": percent }))
            .unwrap();
        for page_name in bl_devices::wavetable::PAGES {
            page(&mut h, device, page_name);
            let nodes = Harness::nodes(&h.settle());
            let b = Harness::node(&nodes, "Device View").bounds.unwrap();
            let view = Rect::from_min_max(
                pos2(b.x0 as f32, b.y0 as f32),
                pos2(b.x1 as f32, b.y1 as f32),
            );
            let frame = layout(&h.shell, view)
                .into_iter()
                .find(|(id, _)| *id == device)
                .unwrap()
                .1;
            let shown = frame_state(&h, device);
            let mut body = body_rect(frame, TITLE_BAR_HEIGHT);
            body.min.x += bl_shell::device_commands::sidechain_width(&shown);
            let panel = shown.panel.as_ref().unwrap();
            let (controls, cells, pages) =
                bl_ui::device_panel::place_with_pages(panel, shown.expanded, body);
            let mut rects: Vec<(String, Rect)> = controls
                .into_iter()
                .map(|(param, _, rect)| (format!("control {}", param.0), rect))
                .chain(cells.into_iter().map(|(name, rect, _)| (name, rect)))
                .collect();
            rects.extend(pages.iter().flat_map(|p| {
                p.buttons.iter().map(|r| {
                    (
                        format!("tab {}", p.name),
                        Rect::from_min_size(pos2(r.x, r.y), vec2(r.width, r.height)),
                    )
                })
            }));
            assert!(rects.len() >= 5, "{page_name} at {percent}%");
            for (name, rect) in &rects {
                assert!(
                    body.expand(0.5).contains_rect(*rect),
                    "{name} lies outside the body on {page_name} at {percent}%: {rect:?} in {body:?}"
                );
            }
            for (i, (a, ra)) in rects.iter().enumerate() {
                for (b, rb) in &rects[i + 1..] {
                    let overlap = ra.intersect(*rb);
                    assert!(
                        overlap.width() <= 0.01 || overlap.height() <= 0.01,
                        "{a} overlaps {b} on {page_name} at {percent}%: {ra:?} {rb:?}"
                    );
                }
            }
        }
    }
}

// Spec: wavetable R39
#[test]
fn removing_a_destination_with_no_route_records_no_step() {
    let (mut h, device) = window();
    h.shell
        .run(
            "device.add_destination",
            json!({ "device": device, "destination": "Osc 1 Position" }),
        )
        .unwrap();
    let before = steps(&h);
    h.shell
        .run(
            "device.remove_destination",
            json!({ "device": device, "destination": "Osc 1 Position" }),
        )
        .unwrap();
    assert_eq!(steps(&h), before);
    let grid = frame_state(&h, device).panel.unwrap().matrix_grid.unwrap();
    assert!(grid.rows.is_empty());
}

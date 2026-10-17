//! A rack in the Device View (WP-5.2; the racks spec, R12 and R18 to R26),
//! headless: its frame with the view column, chain list and the selected
//! chain's devices; selecting, soloing and dragging chains; the device list;
//! the menus. Each gesture runs the registered command an agent runs.
//! Devices are the engine's test devices.

mod common;

use bl_engine::testing;
use bl_model::{ChainId, DeviceId, RackView, TrackId};
use bl_shell::device_commands::{DeviceAdded, DeviceFrame, RackFrame, TITLE_BAR_HEIGHT};
use bl_ui::device_view::{activator_rect, layout, title_rect};
use bl_ui::layout::Element;
use bl_ui::rack_view::{self, RackLayout};
use common::{Harness, Window, command};
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2};
use serde_json::{Value, json};

fn harness() -> Harness {
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let audio: Vec<TrackId> = h.shell.editor().document().top_level().collect();
    h.shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio[2]] }))
        .unwrap();
    h.settle();
    h
}

fn gain(name: &str) -> Value {
    let t = testing::gain_type();
    json!({ "kind": t.kind, "name": name, "role": t.role })
}

fn rack_json() -> Value {
    json!({ "kind": { "rack": { "kind": "audio_effect" } }, "name": "Rack", "role": "audio_effect" })
}

fn add(h: &mut Harness, args: Value) -> DeviceId {
    let added: DeviceAdded =
        serde_json::from_value(h.shell.run("device.add", args).unwrap()).unwrap();
    h.settle();
    added.device
}

fn add_chain(h: &mut Harness, rack: DeviceId, name: &str) -> ChainId {
    let id: ChainId = h.shell.new_id();
    let end = h
        .shell
        .editor()
        .document()
        .device(rack)
        .unwrap()
        .chains
        .len();
    h.shell
        .invoke(
            "chain.add",
            json!({ "rack": rack, "index": end, "chain": { "id": id, "name": name, "devices": [] } }),
        )
        .unwrap();
    id
}

/// An audio effect rack on the shown track with chains named `names`.
fn rack(h: &mut Harness, names: &[&str]) -> (DeviceId, Vec<ChainId>) {
    let rack = add(h, json!({ "device": rack_json() }));
    let chains = names.iter().map(|n| add_chain(h, rack, n)).collect();
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

fn frame_of(h: &mut Harness, rack: DeviceId) -> Rect {
    let rect = view(h);
    layout(&h.shell, rect)
        .into_iter()
        .find(|(id, _)| *id == rack)
        .unwrap()
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

fn press(at: Pos2, pressed: bool, modifiers: Modifiers) -> Event {
    Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers,
    }
}

fn click_with(h: &mut Harness, at: Pos2, modifiers: Modifiers) {
    let held = Event::ModifiersChanged(modifiers);
    h.frame(vec![held.clone(), Event::PointerMoved(at)]);
    for pressed in [true, false] {
        h.frame(vec![held.clone(), press(at, pressed, modifiers)]);
    }
    h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    h.settle();
}

fn click(h: &mut Harness, at: Pos2) {
    click_with(h, at, Modifiers::NONE);
}

fn right_click(h: &mut Harness, at: Pos2) -> egui::FullOutput {
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
    h.settle()
}

/// A press at `from`, six steps, and a release at `to`.
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

fn selected(h: &Harness, rack: DeviceId) -> Vec<ChainId> {
    rack_state(h, rack).selected_chains
}

fn last_step(h: &Harness) -> String {
    h.shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// The menu item whose label, less the keys shown beside it, is `label`.
fn item<'a>(nodes: &'a [common::Node], label: &str) -> &'a common::Node {
    nodes
        .iter()
        .find(|n| {
            n.role == Role::Button
                && (n.label == label || n.label.starts_with(&format!("{label} ")))
        })
        .unwrap_or_else(|| panic!("no menu item {label:?}"))
}

fn row(layout: &RackLayout, n: usize) -> rack_view::ChainRowRects {
    layout
        .chain_list
        .as_ref()
        .expect("the chain list shows")
        .rows[n]
}

// Spec: racks R18
#[test]
fn a_racks_views_sit_left_to_right_in_its_brackets() {
    let mut h = harness();
    let (outer, outer_chains) = rack(&mut h, &["Dry", "Wet"]);
    let inner = add(
        &mut h,
        json!({ "device": rack_json(), "chain": outer_chains[0] }),
    );
    let inner_chains = [add_chain(&mut h, inner, "A"), add_chain(&mut h, inner, "B")];
    let effect = add(
        &mut h,
        json!({ "device": gain("Deep"), "chain": inner_chains[0] }),
    );
    h.settle();
    // Nothing is clicked: the first chain of each rack shows.
    let outer_frame = frame_of(&mut h, outer);
    let outer_layout = layout_of(&mut h, outer);

    // The view column, then the Macro Controls' place, the chain list and
    // the selected chain's devices, which the end bracket closes.
    let macros = outer_layout.macros.unwrap();
    let list = outer_layout.chain_list.as_ref().unwrap().area;
    let devices = outer_layout.devices.unwrap();
    let bracket = outer_layout.bracket.unwrap();
    let column_right = outer_layout.selectors[0].1.right();
    assert!(
        outer_layout
            .selectors
            .iter()
            .all(|(_, r)| r.top() >= outer_frame.top() + TITLE_BAR_HEIGHT)
    );
    assert!(column_right <= macros.left());
    assert_eq!(macros.right(), list.left());
    assert_eq!(list.right(), devices.left());
    assert_eq!(bracket.right(), devices.right());
    assert_eq!(
        devices.right(),
        outer_frame.right(),
        "the bracket ends the rack"
    );
    assert_eq!(
        outer_layout
            .selectors
            .iter()
            .map(|(v, _)| *v)
            .collect::<Vec<_>>(),
        [RackView::Macros, RackView::ChainList, RackView::Devices]
    );

    // The nested rack is a bracket inside its parent's.
    let inner_frame = outer_layout.device_frames[0].1;
    assert_eq!(outer_layout.device_frames[0].0, inner);
    assert!(
        devices.contains_rect(inner_frame),
        "inside the parent's devices view"
    );
    let inner_layout = layout_of(&mut h, inner);
    let inner_devices = inner_layout.devices.unwrap();
    assert!(inner_frame.contains_rect(inner_devices));
    assert!(inner_frame.contains_rect(inner_layout.bracket.unwrap()));
    assert!(inner_layout.bracket.unwrap().right() < outer_layout.bracket.unwrap().left());
    let deep = inner_layout.device_frames[0];
    assert_eq!(deep.0, effect);
    assert!(inner_devices.contains_rect(deep.1));
    assert!(
        deep.1.right() < inner_layout.bracket.unwrap().left(),
        "the bracket follows the last device"
    );

    // The rack and its chain list are named for assistive technology.
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(
        Harness::node(&nodes, "Chain List of Audio Effect Rack").role,
        Role::List
    );
    assert_eq!(Harness::node(&nodes, "Macro Controls").role, Role::Group);
    let _ = (activator_rect, title_rect);
}

// Spec: racks R19
#[test]
fn hiding_every_view_folds_the_rack() {
    let mut h = harness();
    let (rack, _) = rack(&mut h, &["One"]);
    let steps = h.shell.editor().undo_history().len();
    let wide = frame_of(&mut h, rack).width();
    assert!(wide > 3.0 * TITLE_BAR_HEIGHT);
    for view in [RackView::Macros, RackView::ChainList] {
        let at = layout_of(&mut h, rack)
            .selectors
            .iter()
            .find(|(v, _)| *v == view)
            .unwrap()
            .1;
        click(&mut h, at.center());
        assert!(!rack_state(&h, rack).views.shown(view));
    }
    let narrower = frame_of(&mut h, rack).width();
    assert!(narrower < wide, "each hidden view narrows it");
    let devices = layout_of(&mut h, rack).selectors[2].1;
    click(&mut h, devices.center());
    assert!(rack_state(&h, rack).is_strip());
    assert_eq!(
        frame_of(&mut h, rack).width(),
        TITLE_BAR_HEIGHT,
        "it folds into its view column"
    );
    assert_eq!(h.shell.editor().undo_history().len(), steps, "no undo step");

    // The selectors stay, to bring the views back.
    let chains = layout_of(&mut h, rack)
        .selectors
        .iter()
        .find(|(v, _)| *v == RackView::ChainList)
        .unwrap()
        .1;
    click(&mut h, chains.center());
    assert!(rack_state(&h, rack).views.chain_list);
    assert!(!rack_state(&h, rack).is_strip());
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "Chain List View").value, "shown");
    assert_eq!(
        Harness::node(&nodes, "Macro Controls View").role,
        Role::Button
    );
}

// Spec: racks R12
#[test]
fn option_click_on_a_racks_activator_switches_everything_inside() {
    let mut h = harness();
    let (rack, chains) = rack(&mut h, &["One"]);
    let inside = add(&mut h, json!({ "device": gain("In"), "chain": chains[0] }));
    let frame = frame_of(&mut h, rack);
    let activator = activator_rect(frame, false).center();
    let steps = h.shell.editor().undo_history().len();
    click_with(&mut h, activator, Modifiers::ALT);
    let doc = h.shell.editor().document();
    assert!(!doc.device(rack).unwrap().enabled);
    assert!(
        !doc.device(inside).unwrap().enabled,
        "and everything inside it"
    );
    assert_eq!(
        h.shell.editor().undo_history().len(),
        steps + 1,
        "one undo step"
    );
    click_with(&mut h, activator, Modifiers::ALT);
    assert!(h.shell.editor().document().device(inside).unwrap().enabled);
    // A plain click switches only the rack.
    click(&mut h, activator);
    let doc = h.shell.editor().document();
    assert!(!doc.device(rack).unwrap().enabled && doc.device(inside).unwrap().enabled);
}

// Spec: racks R21
#[test]
fn the_chain_list_shows_each_chains_controls_and_takes_drops() {
    let mut h = harness();
    let (rack, chains) = rack(&mut h, &["Dry", "Wet"]);
    let list = layout_of(&mut h, rack);
    let first = row(&list, 0);
    // A row, in order: the activator, the name, Solo and Hot-Swap, and Volume
    // and Pan under them.
    assert!(first.activator.right() < first.name.left());
    assert!(first.name.right() < first.solo.left());
    assert!(first.solo.right() < first.hot_swap.left());
    assert!(first.volume.top() > first.activator.bottom());
    assert!(first.volume.right() < first.pan.left());
    assert_eq!(
        row(&list, 1).row.top() - first.row.top(),
        bl_shell::device_commands::CHAIN_ROW_HEIGHT
    );
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "Chain Dry").role, Role::Button);
    assert_eq!(Harness::node(&nodes, "Chain Activator of Dry").value, "on");
    assert_eq!(
        Harness::node(&nodes, "Chain Volume of Wet").role,
        Role::Slider
    );

    // The activator switches the chain, one undo step.
    let steps = h.shell.editor().undo_history().len();
    click(&mut h, first.activator.center());
    assert!(!rack_state(&h, rack).chains[0].enabled);
    assert_eq!(h.shell.editor().undo_history().len(), steps + 1);
    // The Hot-Swap button arms it.
    click(&mut h, first.hot_swap.center());
    assert!(rack_state(&h, rack).chains[0].hot_swap);

    // Volume and Pan follow the pointer in one undo step per drag, and a
    // double-click resets them.
    let volume = first.volume;
    let steps = h.shell.editor().undo_history().len();
    drag(
        &mut h,
        volume.center(),
        pos2(volume.left() + volume.width() * 0.25, volume.center().y),
    );
    let set = rack_state(&h, rack).chains[0].volume;
    assert!((set - rack_view::volume_at(0.25)).abs() < 0.05, "{set}");
    assert!(set < 1.0);
    assert_eq!(h.shell.editor().undo_history().len(), steps + 1);
    drag(
        &mut h,
        first.pan.center(),
        pos2(first.pan.right(), first.pan.center().y),
    );
    assert_eq!(rack_state(&h, rack).chains[0].pan, 1.0);
    h.time += 1.0;
    for _ in 0..2 {
        h.frame(vec![Event::PointerMoved(first.pan.center())]);
        for pressed in [true, false] {
            h.frame(vec![press(first.pan.center(), pressed, Modifiers::NONE)]);
        }
    }
    h.settle();
    assert_eq!(rack_state(&h, rack).chains[0].pan, 0.0);

    // A device dragged onto the drop area makes a chain holding it.
    let outside = add(&mut h, json!({ "device": gain("Loose") }));
    let list = layout_of(&mut h, rack);
    let drop = list.chain_list.as_ref().unwrap().drop_area;
    let frame = frame_of(&mut h, outside);
    let steps = h.shell.editor().undo_history().len();
    drag(&mut h, title_rect(frame, false).center(), drop.center());
    let state = rack_state(&h, rack);
    assert_eq!(state.chains.len(), 3);
    assert_eq!(
        state.chains[2].name, "Loose",
        "at the end of the list, named after it"
    );
    assert_eq!(
        h.shell
            .editor()
            .document()
            .chain(state.chains[2].id)
            .unwrap()
            .devices[0]
            .id,
        outside
    );
    assert_eq!(h.shell.editor().undo_history().len(), steps + 1);

    // So does a preset file dropped from the desktop.
    let preset = std::env::temp_dir().join(format!("bl-ui-rack-{}.blpreset", std::process::id()));
    h.shell
        .run(
            "device.save_preset",
            json!({ "device": outside, "path": preset.to_string_lossy() }),
        )
        .unwrap();
    let drop = layout_of(&mut h, rack).chain_list.unwrap().drop_area;
    h.frame(vec![Event::PointerMoved(drop.center())]);
    h.dropped
        .push(std::sync::Arc::new(DroppedFile(preset.clone())));
    h.frame(Vec::new());
    h.settle();
    assert_eq!(
        rack_state(&h, rack).chains.len(),
        4,
        "a chain holding a new device"
    );
    let _ = std::fs::remove_file(&preset);
    let _ = chains;
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

// Spec: racks R22
#[test]
fn chains_select_by_click_arrows_and_modifiers() {
    let mut h = harness();
    let (rack, chains) = rack(&mut h, &["A", "B", "C", "D"]);
    let at = |h: &mut Harness, n: usize| row(&layout_of(h, rack), n).name.center();
    let (a, b, c) = (at(&mut h, 0), at(&mut h, 1), at(&mut h, 2));
    let steps = h.shell.editor().undo_history().len();
    click(&mut h, c);
    assert_eq!(selected(&h, rack), vec![chains[2]]);
    click_with(&mut h, a, Modifiers::SHIFT);
    assert_eq!(
        selected(&h, rack),
        chains[..3].to_vec(),
        "Shift-click selects a range"
    );
    click_with(&mut h, b, command());
    assert_eq!(
        selected(&h, rack),
        vec![chains[0], chains[2]],
        "Cmd-click removes one"
    );
    click_with(&mut h, b, command());
    assert_eq!(selected(&h, rack), chains[..3].to_vec(), "and adds one");
    // With several selected, the devices view says how many.
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "3 chains selected").role, Role::Label);
    click(&mut h, b);
    h.key(Key::ArrowDown, Modifiers::NONE);
    assert_eq!(
        selected(&h, rack),
        vec![chains[2]],
        "Down selects the chain below"
    );
    h.key(Key::ArrowUp, Modifiers::NONE);
    h.key(Key::ArrowUp, Modifiers::NONE);
    assert_eq!(
        selected(&h, rack),
        vec![chains[0]],
        "Up selects the chain above"
    );
    assert_eq!(
        h.shell.editor().undo_history().len(),
        steps,
        "selecting is no undo step"
    );
    assert!(
        h.shell.device_view_state().devices[0]
            .rack
            .as_ref()
            .unwrap()
            .chain_list_focused
    );

    // The devices view shows the selected chain's devices.
    let inside = add(
        &mut h,
        json!({ "device": gain("Inside"), "chain": chains[3] }),
    );
    let d = at(&mut h, 3);
    click(&mut h, d);
    let frames = layout_of(&mut h, rack).device_frames;
    assert_eq!(
        frames.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![inside]
    );
}

// Spec: racks R23
#[test]
fn chain_solo_is_exclusive_and_cmd_click_adds() {
    let mut h = harness();
    let (rack, chains) = rack(&mut h, &["A", "B", "C"]);
    let solo = |h: &mut Harness, n: usize| row(&layout_of(h, rack), n).solo.center();
    let soloed =
        |h: &Harness| -> Vec<bool> { rack_state(h, rack).chains.iter().map(|c| c.solo).collect() };
    let a = solo(&mut h, 0);
    click(&mut h, a);
    assert_eq!(soloed(&h), [true, false, false]);
    let b = solo(&mut h, 1);
    click(&mut h, b);
    assert_eq!(
        soloed(&h),
        [false, true, false],
        "a click unsolos the others"
    );
    let c = solo(&mut h, 2);
    click_with(&mut h, c, command());
    assert_eq!(soloed(&h), [false, true, true], "Cmd-click adds");
    click_with(&mut h, b, command());
    assert_eq!(soloed(&h), [false, false, true], "and removes");
    assert_eq!(last_step(&h), "Solo chain");
    // Taking the focus, S solos the selected chain.
    let name = row(&layout_of(&mut h, rack), 0).name.center();
    click(&mut h, name);
    h.key(Key::S, Modifiers::NONE);
    assert_eq!(soloed(&h), [true, false, false]);
    let _ = chains;
}

// Spec: racks R24
#[test]
fn a_chain_drags_between_racks_and_onto_another_track() {
    let mut h = harness();
    let (first, first_chains) = rack(&mut h, &["Keep", "Move"]);
    let (second, second_chains) = rack(&mut h, &["Target"]);
    let inside = add(
        &mut h,
        json!({ "device": gain("Carried"), "chain": first_chains[1] }),
    );
    // The second rack sits right of the first.
    let from = row(&layout_of(&mut h, first), 1).name.center();
    let target = layout_of(&mut h, second).chain_list.unwrap();
    let to = pos2(target.area.center().x, target.rows[0].row.top() + 2.0);
    let steps = h.shell.editor().undo_history().len();
    drag(&mut h, from, to);
    let doc = h.shell.editor().document();
    assert_eq!(doc.chain_place(first_chains[1]).map(|p| p.0), Some(second));
    assert_eq!(
        doc.device(second).unwrap().chains[0].id,
        first_chains[1],
        "at the pointer"
    );
    assert_eq!(doc.device(first).unwrap().chains.len(), 1);
    assert_eq!(
        h.shell.editor().undo_history().len(),
        steps + 1,
        "one undo step"
    );
    let _ = second_chains;

    // Within a rack, a chain moves up the list.
    let list = layout_of(&mut h, second);
    let (from, rows) = (row(&list, 1).name.center(), list.chain_list.unwrap().rows);
    drag(
        &mut h,
        from,
        pos2(rows[0].row.center().x, rows[0].row.top() + 1.0),
    );
    assert_eq!(
        h.shell.editor().document().device(second).unwrap().chains[0].id,
        second_chains[0]
    );

    // Dropped in the track's own chain, outside any rack, only its devices go.
    let list = layout_of(&mut h, second);
    let from = row(&list, 1).name.center();
    assert_eq!(
        h.shell.editor().document().device(second).unwrap().chains[1].id,
        first_chains[1]
    );
    let frame = frame_of(&mut h, second);
    let to = pos2(frame.right() + 40.0, frame.center().y);
    let steps = h.shell.editor().undo_history().len();
    drag(&mut h, from, to);
    let doc = h.shell.editor().document();
    assert!(doc.chain(first_chains[1]).is_none(), "the chain is gone");
    assert!(
        doc.track(h.shell.device_view_state().track.unwrap())
            .unwrap()
            .devices()
            .any(|d| d.id == inside)
    );
    assert_eq!(
        h.shell.editor().undo_history().len(),
        steps + 1,
        "one undo step"
    );
    h.shell.invoke("edit.undo", json!({})).unwrap();
    assert!(h.shell.editor().document().chain(first_chains[1]).is_some());
}

// Spec: racks R25
#[test]
fn the_device_list_shows_racks_as_a_hierarchy() {
    let mut h = harness();
    let (outer, chains) = rack(&mut h, &["Dry", "Wet"]);
    let inner = add(&mut h, json!({ "device": rack_json(), "chain": chains[1] }));
    let inner_chain = add_chain(&mut h, inner, "Deep chain");
    let deep = add(
        &mut h,
        json!({ "device": gain("Deep"), "chain": inner_chain }),
    );
    h.settle();
    let selector = h
        .main
        .drawn_layout()
        .unwrap()
        .elements
        .iter()
        .find(|(e, _)| *e == Element::DeviceSelector)
        .unwrap()
        .1;
    let nodes = Harness::nodes(&right_click(&mut h, selector.center()));
    // Every device, chain by chain, and a device two levels down.
    let level = |label: &str| {
        nodes
            .iter()
            .find(|n| n.role == Role::Button && n.label == label)
            .unwrap_or_else(|| panic!("no item {label:?}"))
            .description
            .clone()
    };
    assert_eq!(level("Audio Effect Rack"), "Level 1");
    assert_eq!(level("Wet"), "Level 2");
    assert_eq!(level("Deep chain"), "Level 4");
    assert_eq!(level("Deep"), "Level 5");
    let bounds = nodes
        .iter()
        .find(|n| n.role == Role::Button && n.label == "Deep")
        .and_then(|n| n.bounds)
        .unwrap();
    let at = pos2(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    );
    click(&mut h, at);
    // It's selected, and its chain shows in each rack.
    assert_eq!(
        h.shell.device_view_state().devices[0]
            .rack
            .as_ref()
            .unwrap()
            .selected_chains,
        vec![chains[1]]
    );
    assert_eq!(rack_state(&h, inner).selected_chains, vec![inner_chain]);
    let rect = view(&mut h);
    let layouts = rack_view::layouts(&h.shell, rect);
    assert!(
        layouts
            .iter()
            .any(|(id, l)| *id == inner && l.device_frames[0].0 == deep)
    );
    let _ = outer;
}

// Spec: racks R26
#[test]
fn a_chains_menu_renames_and_colours() {
    let mut h = harness();
    let (rack, chains) = rack(&mut h, &["Pad", "Lead"]);
    let name = row(&layout_of(&mut h, rack), 0).name.center();
    let nodes = Harness::nodes(&right_click(&mut h, name));
    assert!(!item(&nodes, "Rename").disabled);
    assert!(item(&nodes, "Edit Info Text").disabled);
    assert!(
        nodes.iter().any(|n| n.label == "Colour 70"),
        "the palette's 5 rows of 14"
    );
    let swatch = Harness::node(&nodes, "Colour 15");
    let b = swatch.bounds.unwrap();
    click(
        &mut h,
        pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32),
    );
    assert_eq!(
        h.shell.editor().document().chain(chains[0]).unwrap().colour,
        Some(14)
    );
    assert_eq!(last_step(&h), "Change Chain Color");

    // Rename types the name in the row.
    let nodes = Harness::nodes(&right_click(&mut h, name));
    let b = item(&nodes, "Rename").bounds.unwrap();
    click(
        &mut h,
        pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32),
    );
    assert!(h.main.is_typing(), "the row's name is a text field");
    h.frame(vec![Event::Text("Strings".to_owned())]);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert!(!h.main.is_typing());
    let doc = h.shell.editor().document();
    assert!(doc.chain(chains[0]).unwrap().name.ends_with("Strings"));
}

// Spec: racks R26
#[test]
fn a_racks_menu_adds_ungroup_after_group_and_disables_compare() {
    let mut h = harness();
    let (rack, _) = rack(&mut h, &["Only"]);
    let frame = frame_of(&mut h, rack);
    let nodes = Harness::nodes(&right_click(&mut h, title_rect(frame, false).center()));
    // Group, then Ungroup straight after it, then Fold.
    let labels: Vec<&str> = ["Group", "Ungroup", "Fold"]
        .into_iter()
        .map(|l| item(&nodes, l).label.split(' ').next().unwrap())
        .collect();
    assert_eq!(labels, ["Group", "Ungroup", "Fold"]);
    let at = |label: &str| item(&nodes, label).bounds.unwrap().y0;
    assert!(at("Group") < at("Ungroup") && at("Ungroup") < at("Fold"));
    assert!(
        item(&nodes, "Ungroup").bounds.unwrap().y0 - item(&nodes, "Group").bounds.unwrap().y0
            < 25.0,
        "straight after"
    );
    assert!(
        !item(&nodes, "Ungroup").disabled,
        "a rack of one chain can be ungrouped"
    );
    assert!(item(&nodes, "Compare: Switch to B").disabled);
    assert!(item(&nodes, "Compare: Copy A to B").disabled);
    assert_eq!(
        item(&nodes, "Compare: Switch to B").description,
        "A/B compare doesn't apply to racks."
    );
}

// Spec: racks R24
#[test]
fn a_chain_held_over_another_tracks_title_bar_shows_that_track_for_the_drop() {
    let mut h = harness();
    let tracks: Vec<TrackId> = h.shell.editor().document().top_level().collect();
    let (first, chains) = rack(&mut h, &["Move"]);
    // A rack on the next audio track, which the Device View isn't showing.
    h.shell
        .invoke(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[3]] }),
        )
        .unwrap();
    h.settle();
    let (second, _) = rack(&mut h, &["Target"]);
    h.shell
        .invoke(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[2]] }),
        )
        .unwrap();
    h.settle();

    let from = row(&layout_of(&mut h, first), 0).name.center();
    let nodes = Harness::nodes(&h.settle());
    let title = nodes
        .iter()
        .find(|n| n.id == bl_ui::arrangement::headers::title_id(tracks[3]).accesskit_id())
        .and_then(|n| n.bounds)
        .unwrap();
    let over = pos2(
        ((title.x0 + title.x1) / 2.0) as f32,
        ((title.y0 + title.y1) / 2.0) as f32,
    );
    // Press on the row, then hold over the other track's title bar.
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![press(from, true, Modifiers::NONE)]);
    for step in 1..=6 {
        h.frame(vec![Event::PointerMoved(
            from + (over - from) * (step as f32 / 6.0),
        )]);
    }
    h.settle();
    assert_eq!(
        h.shell.arrangement().selected_tracks().first(),
        Some(&tracks[3]),
        "the drag focused the track"
    );
    // Its rack is there to drop into.
    let list = layout_of(&mut h, second).chain_list.unwrap();
    let to = pos2(list.area.center().x, list.rows[0].row.top() + 2.0);
    for step in 1..=6 {
        h.frame(vec![Event::PointerMoved(
            over + (to - over) * (step as f32 / 6.0),
        )]);
    }
    h.frame(vec![press(to, false, Modifiers::NONE)]);
    h.settle();
    let doc = h.shell.editor().document();
    assert_eq!(doc.chain_place(chains[0]).map(|p| p.0), Some(second));
    assert_eq!(
        doc.chain_track(chains[0]),
        Some(tracks[3]),
        "on the other track now"
    );
}

// Spec: racks R20
#[test]
fn the_keys_show_and_hide_chains_and_minus_folds_when_they_are_hidden() {
    let mut h = harness();
    let (rack, _) = rack(&mut h, &["One"]);
    let frame = frame_of(&mut h, rack);
    click(&mut h, title_rect(frame, false).center());
    assert_eq!(h.shell.devices().selected(), Some(rack));
    let steps = h.shell.editor().undo_history().len();
    h.key(Key::Minus, Modifiers::NONE);
    assert!(
        !rack_state(&h, rack).views.chain_list,
        "Minus hides the chain list"
    );
    assert!(!h.shell.devices().is_folded(rack));
    h.key(Key::Minus, Modifiers::NONE);
    assert!(
        h.shell.devices().is_folded(rack),
        "and folds the rack once it's hidden"
    );
    h.key(Key::Minus, Modifiers::NONE);
    assert!(!h.shell.devices().is_folded(rack), "and unfolds it");
    h.key(Key::Plus, Modifiers::NONE);
    assert!(rack_state(&h, rack).views.chain_list, "Plus shows it");
    assert_eq!(h.shell.editor().undo_history().len(), steps, "no undo step");

    // View > Show Chains and Hide Chains are disabled with no rack selected.
    let plain = add(&mut h, json!({ "device": gain("Plain") }));
    let frame = frame_of(&mut h, plain);
    click(&mut h, title_rect(frame, false).center());
    let show = json!({ "view": "chain_list", "shown": true });
    let hide = json!({ "view": "chain_list", "shown": false });
    assert!(
        !h.shell.can_invoke("rack.set_view", &show) && !h.shell.can_invoke("rack.set_view", &hide)
    );
}

// Spec: racks R25
#[test]
fn a_device_chosen_from_the_list_scrolls_into_view() {
    let mut h = harness();
    let mut last = None;
    for n in 0..5 {
        let (_, chains) = rack(&mut h, &["Only"]);
        last = Some(add(
            &mut h,
            json!({ "device": gain(&format!("Far {n}")), "chain": chains[0] }),
        ));
    }
    let far = last.unwrap();
    let rect = view(&mut h);
    let state = h.shell.device_view_state();
    let unscrolled = rack_view::all_frames(&state, rect, 0.0)
        .into_iter()
        .find(|(id, _)| *id == far)
        .unwrap()
        .1;
    assert!(unscrolled.right() > rect.right(), "it starts out of view");
    h.shell
        .invoke("device.select", json!({ "device": far }))
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    let frame = nodes
        .iter()
        .find(|n| n.role == Role::Button && n.label == "Far 4")
        .and_then(|n| n.bounds)
        .unwrap();
    assert!(
        f64::from(rect.left()) <= frame.x0 && frame.x1 <= f64::from(rect.right()),
        "{frame:?}"
    );
}

// Spec: racks R21, R22
#[test]
fn a_focused_slider_keeps_up_down_and_delete_for_itself() {
    let mut h = harness();
    let (rack, chains) = rack(&mut h, &["A", "B"]);
    // Select B, which gives the chain list the focus, then click A's Volume.
    let name = row(&layout_of(&mut h, rack), 1).name.center();
    click(&mut h, name);
    let volume = row(&layout_of(&mut h, rack), 0).volume;
    // A press and release without moving sets it; Down then steps it.
    click(&mut h, volume.center());
    let set = rack_state(&h, rack).chains[0].volume;
    h.key(Key::ArrowDown, Modifiers::NONE);
    assert!(
        rack_state(&h, rack).chains[0].volume < set,
        "Down steps the slider"
    );
    assert_eq!(
        selected(&h, rack),
        vec![chains[1]],
        "and doesn't move the selection"
    );
    h.key(Key::Delete, Modifiers::NONE);
    assert_eq!(
        rack_state(&h, rack).chains[0].volume,
        1.0,
        "Delete resets it"
    );
    assert_eq!(rack_state(&h, rack).chains.len(), 2, "and deletes no chain");
}

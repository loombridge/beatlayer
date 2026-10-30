//! Lock to Control Surface, the hand icon and the session ring in the window,
//! headless (WP-7.6; the control surfaces spec, R23 to R27, R31 and R32, the
//! device chains spec R41 and R42, and the Session View spec R36 to R38).

use crate::common::{self, Harness, Window};
use bl_engine::testing;
use bl_midi_io::{Direction, TestBackend};
use bl_model::{DeviceId, TrackId};
use bl_shell::device_commands::DeviceAdded;
use bl_shell::mixer_view::MixerOf;
use bl_ui::device_view::{button_rects, layout, title_rect};
use bl_ui::layout::mixer::{MixerLayout, SessionLayout, TrackColumn};
use bl_ui::layout::{REFERENCE, ShellLayout, TITLE_BAR};
use bl_ui::theme::Theme;
use egui::accesskit::Role;
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use serde_json::{Value, json};

const SURFACE: &str = "Test Instant Surface";

/// The default set with an audio track selected and a gain device on it.
fn device_harness() -> (Harness, DeviceId) {
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let tracks: Vec<TrackId> = h.shell.editor().document().top_level().collect();
    h.shell
        .invoke(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[2]] }),
        )
        .unwrap();
    h.settle();
    let t = testing::gain_type();
    let added: DeviceAdded = serde_json::from_value(
        h.shell
            .run(
                "device.add",
                json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
            )
            .unwrap(),
    )
    .unwrap();
    h.settle();
    (h, added.device)
}

/// Runs `surface` on row `row` of the table, on test ports: every row has a
/// pair, so surfaces on other rows keep running.
fn run_surface(h: &mut Harness, row: u8, surface: &str) {
    let test = TestBackend::new();
    for n in 1..=6 {
        test.add_port(Direction::Input, &format!("In {n}"));
        test.add_port(Direction::Output, &format!("Out {n}"));
    }
    h.shell.use_midi_backend(Box::new(test));
    h.shell
        .run(
            "control_surface.set",
            json!({ "row": row, "surface": surface,
                    "input": format!("In {row}"), "output": format!("Out {row}") }),
        )
        .unwrap();
    h.settle();
}

fn frame_of(h: &mut Harness, device: DeviceId) -> Rect {
    let nodes = Harness::nodes(&h.settle());
    let b = Harness::node(&nodes, "Device View").bounds.unwrap();
    let view = Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    );
    layout(&h.shell, view)
        .into_iter()
        .find(|(id, _)| *id == device)
        .unwrap()
        .1
}

fn press(at: Pos2, pressed: bool, button: PointerButton) -> Event {
    Event::PointerButton {
        pos: at,
        button,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

fn right_click(h: &mut Harness, at: Pos2) -> egui::FullOutput {
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![press(at, true, PointerButton::Secondary)]);
    h.frame(vec![press(at, false, PointerButton::Secondary)]);
    h.settle()
}

fn centre(node: &common::Node) -> Pos2 {
    let b = node.bounds.unwrap();
    pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
}

fn locks(h: &Harness) -> Vec<Value> {
    serde_json::to_value(h.shell.device_view_state().devices[0].locks.clone())
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

// Spec: device-chains R41, control-surfaces R23. Lock to's items follow Save
// as Default Preset, one for each running surface that can lock, ticked while
// it is, and a disabled item with no surface; neither lock nor unlock is an
// undo step.
#[test]
fn lock_to_items_follow_save_as_default_preset_and_tick_while_locked() {
    let (mut h, device) = device_harness();
    let frame = frame_of(&mut h, device);

    // With no surface: a single disabled item.
    let out = right_click(&mut h, title_rect(frame, false).center());
    let nodes = Harness::nodes(&out);
    let item = Harness::node(&nodes, "Lock to Control Surface");
    assert!(item.disabled);
    let save = Harness::node(&nodes, "Save as Default Preset");
    assert!(
        item.bounds.unwrap().y0 > save.bounds.unwrap().y0,
        "after it"
    );
    h.key(egui::Key::Escape, Modifiers::NONE);
    h.settle();

    // A surface that can't lock isn't offered; one that can is.
    run_surface(&mut h, 2, "test");
    let out = right_click(&mut h, title_rect(frame, false).center());
    assert!(
        Harness::nodes(&out)
            .iter()
            .all(|n| !n.label.contains("Lock to Test Surface")),
        "a surface that can't lock isn't offered"
    );
    h.key(egui::Key::Escape, Modifiers::NONE);
    h.settle();
    run_surface(&mut h, 1, "test-instant");
    let steps = h.shell.editor().undo_history().count();
    let out = right_click(&mut h, title_rect(frame, false).center());
    let nodes = Harness::nodes(&out);
    let item = Harness::node(&nodes, &format!("Lock to {SURFACE}"));
    assert!(!item.disabled);
    let save = Harness::node(&nodes, "Save as Default Preset");
    assert!(
        item.bounds.unwrap().y0 > save.bounds.unwrap().y0,
        "after it"
    );
    h.click(centre(item));
    h.settle();
    assert_eq!(locks(&h).len(), 1, "locked");
    assert_eq!(
        h.shell.editor().undo_history().count(),
        steps,
        "no undo step"
    );

    // Ticked while locked; choosing it again unlocks.
    let out = right_click(&mut h, title_rect(frame, false).center());
    let nodes = Harness::nodes(&out);
    let ticked = nodes
        .iter()
        .find(|n| n.label == format!("✔ Lock to {SURFACE}"))
        .expect("ticked while locked");
    h.click(centre(ticked));
    h.settle();
    assert!(locks(&h).is_empty(), "unlocked");
    assert_eq!(h.shell.editor().undo_history().count(), steps);
}

/// Where a frame paints `text`, if it does.
fn paints(output: &egui::FullOutput, text: &str) -> bool {
    fn walk(shape: &egui::Shape, text: &str) -> bool {
        match shape {
            egui::Shape::Text(t) => t.galley.job.text == text,
            egui::Shape::Vec(shapes) => shapes.iter().any(|s| walk(s, text)),
            _ => false,
        }
    }
    output.shapes.iter().any(|c| walk(&c.shape, text))
}

// Spec: device-chains R42, control-surfaces R25. A locked device's title bar
// shows the hand icon before Hot-Swap Presets, a folded device under its
// name; hovering it says so in the Status Bar and it reaches assistive
// technology as an image.
#[test]
fn a_locked_device_shows_the_hand_icon_in_its_title_bar_and_folded() {
    let (mut h, device) = device_harness();
    run_surface(&mut h, 1, "test-instant");
    let text = format!("Locked to {SURFACE}");
    let find = |h: &mut Harness| {
        Harness::nodes(&h.settle())
            .into_iter()
            .find(|n| n.label == text)
    };
    assert!(find(&mut h).is_none(), "no icon while nothing is locked");

    h.shell
        .run(
            "control_surface.lock",
            json!({ "row": 1, "device": device }),
        )
        .unwrap();
    h.settle();
    let frame = frame_of(&mut h, device);
    let icon = find(&mut h).expect("the hand icon");
    assert_eq!(icon.role, Role::Image);
    let (hot_swap, bounds) = (button_rects(frame, false)[0], icon.bounds.unwrap());
    assert!(
        f64::from(hot_swap.left()) >= bounds.x1,
        "before Hot-Swap Presets"
    );
    assert!(
        f64::from(title_rect(frame, false).left()) <= bounds.x0,
        "after the name starts"
    );

    // The Status Bar says so while the pointer is over it.
    let at = centre(&icon);
    h.frame(vec![Event::PointerMoved(at)]);
    let output = h.frame(vec![Event::PointerMoved(at + vec2(0.5, 0.0))]);
    assert!(paints(&output, &text), "the Status Bar's text");
    h.frame(vec![Event::PointerMoved(pos2(5.0, 500.0))]);
    let away = h.frame(vec![]);
    assert!(!paints(&away, &text));

    // Folded, it sits in the strip above Hot-Swap Presets, under the name.
    h.shell
        .run("device.fold", json!({ "device": device }))
        .unwrap();
    h.settle();
    let frame = frame_of(&mut h, device);
    let icon = find(&mut h).expect("the icon in the strip");
    let (hot_swap, bounds) = (button_rects(frame, true)[0], icon.bounds.unwrap());
    assert!(
        f64::from(hot_swap.top()) >= bounds.y1,
        "above Hot-Swap Presets"
    );
    assert!(f64::from(title_rect(frame, true).top()) <= bounds.y0);
}

/// The rectangles a frame strokes, with their strokes.
fn stroked(output: &egui::FullOutput) -> Vec<(Rect, egui::Stroke)> {
    fn walk(shape: &egui::Shape, out: &mut Vec<(Rect, egui::Stroke)>) {
        match shape {
            egui::Shape::Rect(r) if r.stroke.width > 0.0 => out.push((r.rect, r.stroke.into())),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    output.shapes.iter().for_each(|c| walk(&c.shape, &mut out));
    out
}

fn session_harness() -> Harness {
    let mut h = Harness::manual_with_title_bar(Window::Main);
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    h.settle();
    h
}

fn grid(h: &Harness, scroll: f32) -> (SessionLayout, MixerLayout) {
    let view = ShellLayout::compute(REFERENCE, true, true)
        .translate(vec2(0.0, TITLE_BAR))
        .main_view;
    let session = SessionLayout::compute(view, Some(bl_ui::layout::mixer::Sections::DEFAULT));
    let columns = h.shell.mixer_columns().len();
    let returns = h.shell.editor().document().returns().len();
    let mixer = MixerLayout::compute(
        session.slots,
        &vec![TrackColumn::track(); columns],
        returns,
        bl_ui::layout::mixer::Sections::DEFAULT,
        scroll,
    );
    (session, mixer)
}

// Spec: session-view R36. Each running surface's ring is a border round its
// slots in its row's colour, two overlapping rings are each drawn with the
// lower row's on top, and a ring past the last track draws round what exists.
#[test]
fn the_rings_border_surrounds_its_slots_in_its_rows_colour() {
    let mut h = session_harness();
    run_surface(&mut h, 1, "test-instant");
    let output = h.settle();
    let theme = Theme::bundled(true);
    let (session, mixer) = grid(&h, 0.0);
    let ring: Vec<_> = stroked(&output)
        .into_iter()
        .filter(|(_, s)| s.color == theme.ring_colour(0) && s.width == 2.0)
        .collect();
    assert_eq!(ring.len(), 1, "one border in the first row's colour");
    let rect = ring[0].0;
    assert_eq!(ring[0].1.width, 2.0);
    // Two scenes tall, from the first track's column to the last return's:
    // the set has fewer than eight tracks.
    let first = &mixer.columns[0];
    let last = mixer.returns.last().unwrap();
    let want = Rect::from_min_max(
        pos2(first.rect.left(), session.scene_row(0, 0.0).top()),
        pos2(last.rect.right(), session.scene_row(1, 0.0).bottom()),
    );
    assert!(
        (rect.min - want.min).abs().max_elem() < 1.5,
        "{rect:?} {want:?}"
    );
    assert!(
        (rect.max - want.max).abs().max_elem() < 1.5,
        "{rect:?} {want:?}"
    );

    // A second ring: each is drawn, the lower row's on top (drawn later).
    run_surface(&mut h, 2, "test");
    let output = h.settle();
    let strokes = stroked(&output);
    let at = |row: usize| {
        strokes
            .iter()
            .position(|(_, s)| s.color == theme.ring_colour(row) && s.width == 2.0)
            .unwrap_or_else(|| panic!("no ring in row {}'s colour", row + 1))
    };
    assert!(at(0) < at(1), "the lower row's ring is drawn on top");

    // Stopping a surface takes its border away.
    h.shell
        .run("control_surface.set", json!({ "row": 2, "surface": null }))
        .unwrap();
    let output = h.settle();
    assert!(
        stroked(&output)
            .iter()
            .all(|(_, s)| s.color != theme.ring_colour(1) || s.width != 2.0)
    );
}

// Spec: session-view R37. A ring a surface moves scrolls the grid the least
// it needs to show its first track and scene.
#[test]
fn a_moved_ring_scrolls_the_grid_to_show_it() {
    let mut h = session_harness();
    run_surface(&mut h, 1, "test-instant");
    for n in 0..30 {
        h.shell
            .invoke(
                "track.create",
                json!({ "kind": "audio", "name": format!("T{n}") }),
            )
            .unwrap();
    }
    for _ in 0..40 {
        h.shell.invoke("session.insert_scene", json!({})).unwrap();
    }
    h.settle();
    let scroll = |h: &Harness| h.shell.mixer_view().of(MixerOf::Session).scroll;
    assert_eq!(scroll(&h), 0.0);
    assert_eq!(h.shell.session().state().scroll_y, 0.0);

    let columns = h.shell.mixer_columns();
    h.shell
        .run(
            "control_surface.move_ring",
            json!({ "row": 1, "track": columns[20] }),
        )
        .unwrap();
    h.settle();
    assert!(
        scroll(&h) > 0.0,
        "scrolled sideways to the ring's first track"
    );
    let before = scroll(&h);
    // Moving the ring within view scrolls nothing more.
    h.shell
        .run(
            "control_surface.move_ring",
            json!({ "row": 1, "step": "right" }),
        )
        .unwrap();
    h.settle();
    assert!(scroll(&h) >= before);

    let scenes: Vec<_> = h.shell.editor().document().scenes().map(|s| s.id).collect();
    h.shell
        .run(
            "control_surface.move_ring",
            json!({ "row": 1, "scene": scenes[35] }),
        )
        .unwrap();
    h.settle();
    assert!(
        h.shell.session().state().scroll_y > 0.0,
        "scrolled down to it"
    );
}

// Spec: session-view R38. A slot inside a ring says whose in its description.
#[test]
fn a_slot_in_a_ring_names_the_surface_in_its_description() {
    let mut h = session_harness();
    run_surface(&mut h, 1, "test-instant");
    let doc = h.shell.editor().document();
    let first = doc
        .track(h.shell.mixer_columns()[0])
        .unwrap()
        .name()
        .to_owned();
    let nodes = Harness::nodes(&h.settle());
    let slot = |scene: usize| {
        nodes
            .iter()
            .find(|n| n.role == Role::ListItem && n.label.starts_with(&format!("{first}: ")))
            .map(|_| ())
            .map(|()| scene)
    };
    assert!(slot(0).is_some());
    let inside: Vec<_> = nodes
        .iter()
        .filter(|n| n.role == Role::ListItem && n.description.contains("in the ring of"))
        .collect();
    // Eight scenes, a ring of two rows over six columns.
    assert!(!inside.is_empty());
    assert!(
        inside
            .iter()
            .all(|n| n.description.contains(&format!("in the ring of {SURFACE}")))
    );
    let all = nodes.iter().filter(|n| n.role == Role::ListItem).count();
    assert!(inside.len() < all, "slots outside the ring say nothing");
}

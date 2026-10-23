//! The detail views, headless (WP-6.27): the Status Bar's toggles and
//! selectors, the double-clicks that open a view, the top edge's drag, the
//! keys, the keyboard focus between the two views, and what assistive
//! technology reads.
//!
//! Spec: detail-views R1 to R18.

use crate::common;
use bl_core::time::Ticks;
use bl_model::{Clip, ClipData, ClipId, Command, MidiData, Note, SceneId, TrackId};
use bl_shell::commands::DetailView;
use bl_shell::files::factory_default_set;
use bl_shell::menus::{MenuPlatform, menu_bar};
use bl_ui::arrangement::benchmark;
use bl_ui::keymap::Platform;
use bl_ui::layout::mixer::{MixerLayout, Sections, SessionLayout, TrackColumn};
use bl_ui::layout::{Element, REFERENCE, ShellLayout, TITLE_BAR};
use bl_ui::menus::{ItemState, item_state};
use common::{Harness, Node, Window, command};
use egui::accesskit::{Action, Role};
use egui::{Event, Id, Key, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use serde_json::json;

const BAR: i64 = Ticks::WHOLE.get();

fn shows(h: &Harness) -> (bool, bool) {
    let detail = h.shell.detail();
    (detail.clip_shown(), detail.device_shown())
}

fn layout(h: &Harness) -> ShellLayout {
    h.main
        .drawn_layout()
        .expect("the last frame's layout")
        .clone()
}

fn element(h: &Harness, element: Element) -> Pos2 {
    layout(h).rect(element).center()
}

/// A click with `modifiers` held, as the pointer presses and releases.
fn click_with(h: &mut Harness, at: Pos2, modifiers: Modifiers) {
    h.frame(vec![
        Event::ModifiersChanged(modifiers),
        Event::PointerMoved(at),
    ]);
    for pressed in [true, false] {
        h.frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers,
        }]);
    }
    h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    h.settle();
}

fn double_click(h: &mut Harness, at: Pos2) {
    h.time += 1.0;
    h.frame(vec![Event::PointerMoved(at)]);
    for _ in 0..2 {
        for pressed in [true, false] {
            h.frame(vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            }]);
        }
    }
    h.settle();
}

/// A press at `from`, the pointer moving to `to` in a few frames, and a
/// release.
fn drag(h: &mut Harness, from: Pos2, to: Pos2) {
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    for step in 1..=4 {
        h.frame(vec![Event::PointerMoved(
            from + (to - from) * (step as f32 / 4.0),
        )]);
    }
    h.frame(vec![Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
}

fn node<'a>(nodes: &'a [Node], label: &str) -> &'a Node {
    Harness::node(nodes, label)
}

fn rect_of(node: &Node) -> Rect {
    let bounds = node.bounds.expect("the node has bounds");
    Rect::from_min_max(
        pos2(bounds.x0 as f32, bounds.y0 as f32),
        pos2(bounds.x1 as f32, bounds.y1 as f32),
    )
}

/// The top edge's grab point: the middle of the gap above the detail area.
fn edge(h: &Harness) -> Pos2 {
    let detail = layout(h).detail;
    pos2(detail.center().x, detail.top() - 4.0)
}

/// A MIDI clip on the first MIDI track, selected, with a note off the grid.
fn midi_clip(h: &mut Harness) -> ClipId {
    let mut editor = factory_default_set();
    let track: TrackId = editor.document().top_level().next().unwrap();
    let id: ClipId = editor.new_id();
    let step = h.shell.arrangement().grid_ticks(editor.document());
    let notes = vec![Note::new(60, step / 3, step, 100)];
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                id,
                Ticks::ZERO,
                Ticks(4 * BAR),
                ClipData::Midi(MidiData::new(notes)),
            ),
        })
        .unwrap();
    editor.clear_history().unwrap();
    h.shell = benchmark::shell_with(editor);
    h.settle();
    h.shell
        .run("arrangement.select_clips", json!({ "clips": [id] }))
        .unwrap();
    id
}

// Spec: detail-views R1, R2, R3 (I-CLICK-027). A click on a toggle shows or
// hides its view and leaves the other; Option-click shows both; a click on a
// selector shows its view alone.
#[test]
fn the_toggles_and_selectors_run_their_commands() {
    let mut h = Harness::new(Window::Main);
    h.settle();
    assert_eq!(shows(&h), (false, true), "the Device View alone");

    h.click(element(&h, Element::ClipViewToggle));
    assert_eq!(shows(&h), (true, true), "the Device View stays");
    h.click(element(&h, Element::DeviceViewToggle));
    assert_eq!(shows(&h), (true, false));
    {
        let at = element(&h, Element::DeviceViewToggle);
        click_with(&mut h, at, Modifiers::ALT);
    };
    assert_eq!(shows(&h), (true, true), "Option-click shows both");
    {
        let at = element(&h, Element::ClipViewToggle);
        click_with(&mut h, at, Modifiers::ALT);
    };
    assert_eq!(
        shows(&h),
        (true, true),
        "and with both showing, changes nothing"
    );

    h.click(element(&h, Element::ClipSelector));
    assert_eq!(shows(&h), (true, false), "a selector shows its view alone");
    h.click(element(&h, Element::DeviceSelector));
    assert_eq!(shows(&h), (false, true));
    h.click(element(&h, Element::DeviceSelector));
    assert_eq!(shows(&h), (false, true), "its own view stays");
    assert!(!h.shell.is_dirty(), "none of it touches the set");
}

// Spec: detail-views R3, R18. The toggles are switches that read on or off,
// and the selectors read the clip's and the track's names.
#[test]
fn the_toggles_and_selectors_reach_assistive_technology() {
    let mut h = Harness::new(Window::Main);
    midi_clip(&mut h);
    let nodes = Harness::nodes(&h.settle());
    let clip = node(&nodes, "Show/Hide Clip View");
    let device = node(&nodes, "Show/Hide Device View");
    assert_eq!(clip.role, Role::Button);
    assert_eq!((clip.value.as_str(), clip.toggled), ("off", Some(false)));
    assert_eq!((device.value.as_str(), device.toggled), ("on", Some(true)));
    let track = node(&nodes, "Device View Selector");
    assert_eq!(track.role, Role::Button);
    assert_eq!(track.value, "1 MIDI", "the selected track's name");

    h.click(element(&h, Element::ClipViewToggle));
    let nodes = Harness::nodes(&h.settle());
    let clip = node(&nodes, "Show/Hide Clip View");
    assert_eq!((clip.value.as_str(), clip.toggled), ("on", Some(true)));
    // The two views are regions, the Clip View above the Device View.
    let (above, below) = (
        rect_of(node(&nodes, "Clip View")),
        rect_of(node(&nodes, "Device View")),
    );
    assert!(above.bottom() <= below.top(), "{above:?} over {below:?}");
}

// Spec: detail-views R4 (I-CLICK-005, I-CLICK-006). Double-clicking a track's
// title bar selects the track and shows the Device View, and with both views
// showing hides neither.
#[test]
fn double_clicking_a_title_bar_shows_the_device_view() {
    let mut h = Harness::new(Window::Main);
    h.settle();
    h.shell.run("view.toggle_device_view", json!({})).unwrap();
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    h.settle();
    assert_eq!(shows(&h), (true, false));
    let nodes = Harness::nodes(&h.settle());
    let title = rect_of(node(&nodes, "3 Audio")).center();
    double_click(&mut h, title);
    assert_eq!(shows(&h), (false, true));
    assert_eq!(
        h.shell.arrangement().selected_tracks().len(),
        1,
        "the track is selected"
    );

    h.shell.run("view.show_detail_views", json!({})).unwrap();
    let nodes = Harness::nodes(&h.settle());
    let title = rect_of(node(&nodes, "4 Audio")).center();
    double_click(&mut h, title);
    assert_eq!(shows(&h), (true, true), "neither is hidden");
}

// Spec: detail-views R4 (I-CLICK-005). Double-clicking a clip in the
// Arrangement selects it and shows the Clip View.
#[test]
fn double_clicking_an_arrangement_clip_shows_the_clip_view() {
    let mut h = Harness::with_title_bar(Window::Main);
    let clip = midi_clip(&mut h);
    h.shell
        .run("arrangement.select_clips", json!({ "clips": [] }))
        .ok();
    h.settle();
    assert_eq!(shows(&h), (false, true));
    // The clip's title row, on the first track, a bar in.
    let nodes = Harness::nodes(&h.settle());
    let track = rect_of(node(&nodes, "1 MIDI"));
    let at = pos2(320.0 + 30.0, track.top() + 8.0);
    double_click(&mut h, at);
    assert_eq!(shows(&h), (true, false));
    assert_eq!(h.shell.arrangement().selected_clips(), [clip]);
}

fn session_grid(h: &Harness) -> (Rect, Vec<TrackId>, Vec<SceneId>, MixerLayout) {
    let view = ShellLayout::compute(REFERENCE, true, true)
        .translate(vec2(0.0, TITLE_BAR))
        .main_view;
    let session = SessionLayout::compute(view, Some(Sections::DEFAULT));
    let columns = h.shell.mixer_columns();
    let returns = h.shell.editor().document().returns().count();
    let layout = MixerLayout::compute(
        session.slots,
        &vec![TrackColumn::track(); columns.len()],
        returns,
        Sections::DEFAULT,
        0.0,
    );
    let scenes = h
        .shell
        .editor()
        .document()
        .scenes()
        .map(|scene| scene.id)
        .collect();
    let first = layout.columns[0]
        .rect
        .intersect(session.scene_row(0, 0.0))
        .shrink(1.0);
    (first, columns, scenes, layout)
}

// Spec: detail-views R4 (I-CLICK-005). Double-clicking a Session slot that
// holds a clip shows the Clip View.
#[test]
fn double_clicking_a_session_slot_shows_the_clip_view() {
    let mut h = Harness::manual_with_title_bar(Window::Main);
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    h.settle();
    let (slot, columns, scenes, _) = session_grid(&h);
    // An empty slot makes a clip (I-CLICK-004) and changes no view.
    double_click(&mut h, slot.center());
    assert!(
        h.shell
            .editor()
            .document()
            .slot(columns[0], scenes[0])
            .is_some()
    );
    assert_eq!(shows(&h), (false, true));
    // Then the clip's slot opens it.
    double_click(&mut h, slot.center());
    assert_eq!(shows(&h), (true, false));
}

// Spec: detail-views R13. A selected scene's Scene View takes the place of
// the top view showing: the Device View's alone, the Clip View's with both.
#[test]
fn the_scene_view_takes_the_top_views_place() {
    let mut h = Harness::new(Window::Main);
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    let scene = h.shell.editor().document().scenes().next().unwrap().id;
    h.shell
        .run("session.select", json!({ "scene": scene }))
        .unwrap();
    let scene_view_id = bl_ui::scene_view::scene_view_id().accesskit_id();
    let nodes = Harness::nodes(&h.settle());
    assert!(
        nodes.iter().any(|n| n.id == scene_view_id),
        "the Scene View shows"
    );
    assert!(
        !nodes.iter().any(|n| n.label == "Device View"),
        "in the Device View's place"
    );

    h.shell.run("view.show_detail_views", json!({})).unwrap();
    let nodes = Harness::nodes(&h.settle());
    assert!(
        nodes.iter().any(|n| n.id == scene_view_id),
        "in the Clip View's place"
    );
    assert!(
        nodes.iter().any(|n| n.label == "Device View"),
        "the Device View stays below"
    );
    assert!(
        !nodes.iter().any(|n| n.label == "Clip View"),
        "the Clip View waits"
    );
    assert_eq!(shows(&h), (true, true), "the switches stay as they were");
}

// Spec: detail-views R10, R18. Dragging the top edge sets the Clip View's
// height frame by frame, to the limits; dragging to the bottom closes the
// view, which opens again at its old height; none is an undo step.
#[test]
fn dragging_the_top_edge_resizes_and_closes_the_clip_view() {
    let mut h = Harness::new(Window::Main);
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    h.settle();
    let steps = h.shell.editor().undo_history().len();
    let before = layout(&h).clip_view.unwrap().height();
    assert_eq!(before, 225.0);

    // 60 points up: 60 taller.
    let grab = edge(&h);
    drag(&mut h, grab, grab - vec2(0.0, 60.0));
    assert_eq!(h.shell.detail().clip_height(), 285.0);
    assert_eq!(layout(&h).clip_view.unwrap().height(), 285.0);

    // Far up stops where the main view keeps its 80 points.
    let grab = edge(&h);
    drag(&mut h, grab, pos2(grab.x, 10.0));
    let tallest = layout(&h).largest_clip_view;
    assert_eq!(h.shell.detail().clip_height(), f64::from(tallest));
    assert_eq!(layout(&h).main_view.height(), 80.0);

    // To the bottom closes it, and it opens again at the height it had
    // before that drag.
    let had = h.shell.detail().clip_height();
    let grab = edge(&h);
    drag(&mut h, grab, pos2(grab.x, 990.0));
    assert_eq!(shows(&h), (false, true));
    assert_eq!(h.shell.detail().clip_height(), had);
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    h.settle();
    assert_eq!(layout(&h).clip_view.unwrap().height(), had as f32);
    assert!(!h.shell.is_dirty());
    assert_eq!(h.shell.editor().undo_history().len(), steps);
}

// Spec: detail-views R10. With only the Device View showing, a drag of the
// edge ending lower than half its height closes it, and a shorter one goes
// back.
#[test]
fn dragging_the_device_views_edge_closes_it_past_half_its_height() {
    let mut h = Harness::new(Window::Main);
    h.settle();
    let grab = edge(&h);
    drag(&mut h, grab, grab + vec2(0.0, 40.0));
    assert_eq!(shows(&h), (false, true), "the edge goes back");
    drag(&mut h, grab, grab + vec2(0.0, 150.0));
    assert_eq!(shows(&h), (false, false));
    let layout = layout(&h);
    assert_eq!(layout.main_view.bottom(), layout.detail.bottom());
}

// Spec: detail-views R18. The top edge is a splitter whose value is the Clip
// View's height, and a screen reader's increment and decrement move it by 10
// points.
#[test]
fn the_top_edge_is_a_splitter() {
    let mut h = Harness::new(Window::Main);
    h.settle();
    let nodes = Harness::nodes(&h.settle());
    assert!(
        !nodes.iter().any(|n| n.label == "Clip View Height"),
        "no splitter while the Clip View hides"
    );
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    let nodes = Harness::nodes(&h.settle());
    let splitter = node(&nodes, "Clip View Height");
    assert_eq!(splitter.role, Role::Splitter);
    assert_eq!(splitter.numeric, Some(225.0));
    let id = Id::new("detail-edge");
    h.accesskit(id, Action::Increment);
    h.settle();
    assert_eq!(h.shell.detail().clip_height(), 235.0);
    h.accesskit(id, Action::Decrement);
    h.accesskit(id, Action::Decrement);
    h.settle();
    assert_eq!(h.shell.detail().clip_height(), 215.0);
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(node(&nodes, "Clip View Height").numeric, Some(215.0));
}

// Spec: detail-views R11 (S-NOTE-030). Cmd+Option+E with the focus in the
// Clip View expands it, and again puts it back.
#[test]
fn the_expand_key_works_in_the_clip_view() {
    let mut h = Harness::new(Window::Main);
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    h.shell.run("focus.clip_view", json!({})).unwrap();
    h.settle();
    let keys = command() | Modifiers::ALT;
    h.key(Key::E, keys);
    h.settle();
    assert!(h.shell.detail().expanded());
    assert_eq!(layout(&h).main_view.height(), 80.0);
    h.key(Key::E, keys);
    h.settle();
    assert!(!h.shell.detail().expanded());
    assert_eq!(layout(&h).clip_view.unwrap().height(), 225.0);
}

// Spec: detail-views R1, R5, R6, R14 (S-VIEW-004, S-VIEW-016, S-VIEW-022,
// S-FOCUS-004, S-FOCUS-005): the keys.
#[test]
fn the_keys_reach_the_detail_commands() {
    let mut h = Harness::new(Window::Main);
    h.settle();
    let keys = command() | Modifiers::ALT;
    h.key(Key::Num3, keys);
    assert_eq!(shows(&h), (true, true), "Cmd+Option+3");
    h.key(Key::Num4, keys);
    assert_eq!(shows(&h), (true, false), "Cmd+Option+4");
    h.key(Key::Num4, keys);
    h.key(Key::L, keys);
    assert_eq!(shows(&h), (false, false), "Cmd+Option+L hides both");
    h.key(Key::L, keys);
    assert_eq!(shows(&h), (true, true), "and shows them again");

    h.key(Key::Num4, Modifiers::ALT);
    assert_eq!(h.shell.detail().focus(), DetailView::Device, "Option+4");
    h.key(Key::Num3, Modifiers::ALT);
    assert_eq!(h.shell.detail().focus(), DetailView::Clip, "Option+3");
    h.key(Key::F12, Modifiers::NONE);
    assert_eq!(h.shell.detail().focus(), DetailView::Device, "F12 moves it");
}

// Spec: detail-views R5. Shift+Tab switches the views only while Tab doesn't
// move the focus; F12 does whichever way the setting is.
#[test]
fn shift_tab_and_f12_switch_the_views_by_the_tab_setting() {
    let mut h = Harness::new(Window::Main);
    h.settle();
    h.key(Key::Tab, Modifiers::SHIFT);
    assert_eq!(shows(&h), (true, false), "Shift+Tab, Tab setting off");
    h.key(Key::F12, Modifiers::NONE);
    assert_eq!(shows(&h), (false, true), "F12");
    h.shell
        .run("focus.toggle_tab_navigation", json!({}))
        .unwrap();
    h.settle();
    h.key(Key::Tab, Modifiers::SHIFT);
    assert_eq!(
        shows(&h),
        (false, true),
        "Shift+Tab moves the focus instead"
    );
    h.key(Key::F12, Modifiers::NONE);
    assert_eq!(shows(&h), (true, false), "F12 whatever the setting");
}

// Spec: detail-views R15. With both views showing, a note editor key reaches
// the note editor only while the focus is in the Clip View.
#[test]
fn a_note_editor_key_reaches_only_the_view_with_the_focus() {
    let mut h = Harness::new(Window::Main);
    let clip = midi_clip(&mut h);
    h.shell.run("view.show_detail_views", json!({})).unwrap();
    h.settle();
    let start = |h: &Harness| match &h.shell.editor().document().clip(clip).unwrap().data {
        ClipData::Midi(midi) => midi.notes.iter().next().unwrap().start,
        ClipData::Audio(_) => panic!("the clip is MIDI"),
    };
    let off_grid = start(&h);
    assert_ne!(off_grid, Ticks::ZERO);

    h.shell
        .run("clip_view.select_notes", json!({ "notes": [0] }))
        .ok();
    h.shell.run("focus.device_view", json!({})).unwrap();
    h.settle();
    h.key(Key::U, command());
    assert_eq!(start(&h), off_grid, "Cmd+U doesn't reach the note editor");

    h.shell.run("focus.clip_view", json!({})).unwrap();
    h.settle();
    h.key(Key::U, command());
    assert_eq!(start(&h), Ticks::ZERO, "with the focus there, it quantises");
}

// Spec: detail-views R14 (I-FOCUS-001). A click in a view gives it the focus.
#[test]
fn a_click_in_a_view_gives_it_the_focus() {
    let mut h = Harness::new(Window::Main);
    h.shell.run("view.show_detail_views", json!({})).unwrap();
    h.settle();
    let (clip, device) = {
        let layout = layout(&h);
        (layout.clip_view.unwrap(), layout.device_view.unwrap())
    };
    h.click(clip.center());
    assert_eq!(h.shell.detail().focus(), DetailView::Clip);
    h.click(device.center());
    assert_eq!(h.shell.detail().focus(), DetailView::Device);
}

fn menu_item(h: &Harness, row: &str) -> ItemState {
    let bar = menu_bar(MenuPlatform::current());
    let item = bar
        .iter()
        .flat_map(bl_shell::menus::Menu::walk)
        .find(|item| item.row == row)
        .unwrap_or_else(|| panic!("{row} is in the menus"));
    item_state(item, &h.shell, &h.keymap, Platform::current())
}

// Spec: detail-views R1, R5, R6, R11, R14. The View menu's items and the
// Navigate menu's reach their commands, with their ticks, their enabled
// states and the changing label.
#[test]
fn the_menu_items_reach_their_commands_and_follow_the_views() {
    let mut h = Harness::new(Window::Main);
    h.settle();
    let clip = menu_item(&h, "M-VIEW-010");
    let device = menu_item(&h, "M-VIEW-011");
    assert_eq!((clip.checked, device.checked), (Some(false), Some(true)));
    assert_eq!(clip.invocation.unwrap().id, "view.toggle_clip_view");
    assert_eq!(device.invocation.unwrap().id, "view.toggle_device_view");
    assert_eq!(
        menu_item(&h, "M-VIEW-012").invocation.unwrap().id,
        "view.switch_detail_view"
    );
    assert!(menu_item(&h, "M-VIEW-012").enabled);

    // Expand Clip View is disabled in a new set, enabled while the Clip View
    // shows (whichever view has the focus) and ticked while it's expanded.
    let expand = menu_item(&h, "M-VIEW-013");
    assert!(!expand.enabled);
    h.shell.run("view.show_detail_views", json!({})).unwrap();
    h.shell.run("focus.device_view", json!({})).unwrap();
    let expand = menu_item(&h, "M-VIEW-013");
    assert!(expand.enabled);
    assert_eq!(expand.checked, Some(false));
    assert_eq!(expand.invocation.unwrap().id, "view.expand_clip_view");
    h.shell.run("view.expand_clip_view", json!({})).unwrap();
    assert_eq!(menu_item(&h, "M-VIEW-013").checked, Some(true));

    // The Hide item reads for what it would hide, then what it would bring
    // back.
    let hide = menu_item(&h, "M-VIEW-014");
    assert_eq!(hide.label, "Hide Clip and Device View");
    assert_eq!(hide.invocation.unwrap().id, "view.toggle_detail_area");
    h.shell.run("view.toggle_detail_area", json!({})).unwrap();
    assert_eq!(
        menu_item(&h, "M-VIEW-014").label,
        "Show Clip and Device View"
    );

    // The Navigate menu's two items.
    for (row, command) in [
        ("M-NAV-004", "focus.clip_view"),
        ("M-NAV-005", "focus.device_view"),
    ] {
        let item = menu_item(&h, row);
        assert!(item.enabled, "{row}");
        assert_eq!(item.invocation.unwrap().id, command);
    }
}

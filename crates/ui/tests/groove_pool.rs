//! The Groove Pool (WP-6.20): its view in the browser column, its rows, fields
//! and buttons, the Clip View's groove controls, the Control Bar's Global
//! Amount, hot-swapping, and what is dropped on it and from it. The behaviour
//! is in the grooves spec, R24 to R35.

use std::path::PathBuf;

use crate::common::{self, Harness, Node, Window};
use bl_core::time::Ticks;
use bl_model::{ClipId, GrooveId, SceneId, TrackId, TrackKind};
use bl_ui::layout::mixer::{MixerLayout, Sections, SessionLayout, TrackColumn};
use bl_ui::layout::{REFERENCE, ShellLayout, TITLE_BAR};
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2};
use serde_json::json;

const BAR: i64 = Ticks::WHOLE.get();

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-groove-pool-{}-{name}", std::process::id()));
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

fn press(at: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// A press at `from`, eight steps, and a release at `to`.
fn drag(h: &mut Harness, from: Pos2, to: Pos2) {
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![press(from, true)]);
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        h.frame(vec![Event::PointerMoved(from + (to - from) * t)]);
    }
    h.frame(vec![press(to, false)]);
    h.settle();
}

/// A drag with a pause on the way, as a hand moves a control: `frames` moves
/// of `dy` points each, then the release.
fn drag_by(h: &mut Harness, from: Pos2, dy: f32, frames: u32) {
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![press(from, true)]);
    let mut at = from;
    for _ in 0..frames {
        at += egui::vec2(0.0, dy);
        h.frame(vec![Event::PointerMoved(at)]);
    }
    h.frame(vec![press(at, false)]);
    h.settle();
}

fn centre(node: &Node) -> Pos2 {
    let b = node.bounds.expect("the node has a place");
    pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
}

fn rect(node: &Node) -> Rect {
    let b = node.bounds.expect("the node has a place");
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

fn nodes(h: &mut Harness) -> Vec<Node> {
    Harness::nodes(&h.settle())
}

/// The nodes named `label` with a place, which several controls may share.
fn named<'a>(nodes: &'a [Node], label: &str) -> Vec<&'a Node> {
    nodes
        .iter()
        .filter(|n| n.label == label && n.bounds.is_some())
        .collect()
}

/// The harness's set, whose pool holds the factory default's groove (R38),
/// with the pool showing.
fn shown() -> Harness {
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.run("view.toggle_groove_pool", json!({})).unwrap();
    h.settle();
    h
}

fn groove_ids(h: &mut Harness) -> Vec<GrooveId> {
    let pool = h.shell.groove_pool();
    pool.grooves.iter().map(|g| g.groove.id).collect()
}

fn midi_track(h: &Harness) -> TrackId {
    h.shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Midi)
        .unwrap()
        .id()
}

/// A MIDI clip of a note on the first MIDI track, from bar `start`, `bars`
/// long.
fn clip(h: &mut Harness, name: &str, start: i64, bars: i64) -> ClipId {
    let track = midi_track(h);
    let id: ClipId = h.shell.new_id();
    h.shell
        .run(
            "clip.create",
            json!({ "track": track, "clip": {
                "id": id, "name": name, "start": start * BAR, "length": bars * BAR,
                "offset": 0, "mute": false,
                "data": { "midi": { "notes": [
                    { "key": 60, "start": 0, "length": BAR / 4, "velocity": 100 },
                    { "key": 60, "start": BAR / 4 + 4000, "length": BAR / 4, "velocity": 80 }
                ] } } } }),
        )
        .unwrap();
    id
}

fn pool_group(nodes: &[Node]) -> Option<Rect> {
    nodes
        .iter()
        .find(|n| n.label == "Groove Pool" && n.role == Role::Group)
        .map(rect)
}

fn browser_group(nodes: &[Node]) -> Rect {
    rect(
        nodes
            .iter()
            .find(|n| n.label == "Browser" && n.role == Role::Group)
            .expect("the browser"),
    )
}

/// Cmd+Option+6, which shows and hides the pool.
fn cmd_alt_6() -> Modifiers {
    common::command() | Modifiers::ALT
}

// Spec: grooves R24 (L-GRV-001, L-CTRL-001, M-VIEW-022, S-VIEW-017,
// S-FOCUS-007). Cmd+Option+6, View > Groove Pool and the Browser Config Menu
// show and hide the pool in the lower half of the browser column, the browser
// above it shrinking to make room; Option+6 focuses it, showing it.
#[test]
fn the_pool_shows_in_the_browsers_lower_half_and_each_way_switches_it() {
    let mut h = Harness::with_title_bar(Window::Main);
    let before = nodes(&mut h);
    assert!(pool_group(&before).is_none(), "hidden in a new session");
    let column = browser_group(&before);
    let steps = h.shell.editor().undo_history().len();

    // The key shows it.
    h.key(Key::Num6, cmd_alt_6());
    let shown = nodes(&mut h);
    let pool = pool_group(&shown).expect("the pool shows");
    let browser = browser_group(&shown);
    assert_eq!((pool.left(), pool.right()), (column.left(), column.right()));
    assert_eq!(pool.bottom(), column.bottom(), "down to the column's foot");
    assert!(
        (pool.height() - (column.height() - 8.0) / 2.0).abs() <= 1.0,
        "about half: {pool:?} in {column:?}"
    );
    assert_eq!(browser.bottom() + 8.0, pool.top(), "8 points between them");
    assert_eq!(browser.top(), column.top());
    assert!(browser.height() < column.height(), "the browser shrank");
    // The key again hides it.
    h.key(Key::Num6, cmd_alt_6());
    assert!(pool_group(&nodes(&mut h)).is_none());
    assert_eq!(browser_group(&nodes(&mut h)), column);

    // The View menu's item.
    h.shell
        .invoke("view.toggle_groove_pool", json!({}))
        .unwrap();
    assert!(pool_group(&nodes(&mut h)).is_some());
    h.shell
        .invoke("view.toggle_groove_pool", json!({}))
        .unwrap();
    assert!(pool_group(&nodes(&mut h)).is_none());

    // The Browser Config Menu's item, ticked while it shows.
    let menu = {
        let all = nodes(&mut h);
        centre(Harness::node(&all, "Browser Config Menu"))
    };
    h.click(menu);
    let items = nodes(&mut h);
    let item = items
        .iter()
        .find(|n| {
            n.label.trim_start_matches("✔ ").starts_with("Groove Pool") && n.role == Role::Button
        })
        .expect("the Groove Pool item");
    assert!(!item.label.starts_with('✔'));
    h.click(centre(item));
    assert!(pool_group(&nodes(&mut h)).is_some());
    h.click(menu);
    let items = nodes(&mut h);
    let item = items
        .iter()
        .find(|n| {
            n.label.trim_start_matches("✔ ").starts_with("Groove Pool") && n.role == Role::Button
        })
        .unwrap();
    assert!(item.label.starts_with('✔'), "ticked while it shows");
    h.click(centre(item));
    assert!(pool_group(&nodes(&mut h)).is_none());
    assert_eq!(h.shell.editor().undo_history().len(), steps, "view state");

    // Option+6 shows it and gives it the keyboard.
    h.key(Key::Num6, Modifiers::ALT);
    let focused = nodes(&mut h);
    assert!(pool_group(&focused).is_some());
    assert_eq!(h.shell.focused_area(), bl_shell::Area::Browser);
    assert_eq!(
        h.shell.browser_view().focus(),
        bl_shell::browser::Part::GroovePool
    );
    let table = focused
        .iter()
        .find(|n| n.role == Role::Table && n.label == "Groove Pool")
        .expect("the pool's table");
    assert_eq!(
        h.ctx.memory(|m| m.focused()).map(|id| id.accesskit_id()),
        Some(table.id)
    );
    assert_eq!(h.shell.editor().undo_history().len(), steps);
}

fn groove(h: &Harness, index: usize) -> bl_model::Groove {
    h.shell.groove_pool().grooves[index].groove.clone()
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

fn field_at(h: &mut Harness, label: &str) -> Pos2 {
    let all = nodes(h);
    centre(
        all.iter()
            .find(|n| n.label == label && n.role == Role::SpinButton)
            .unwrap_or_else(|| panic!("no field {label:?}")),
    )
}

/// Types `text` into the focused field: digits, Period and Minus, then
/// Enter.
fn type_in(h: &mut Harness, text: &str) {
    for c in text.chars() {
        let key = match c {
            '.' => Key::Period,
            '-' => Key::Minus,
            c => Key::from_name(&c.to_string()).expect("a digit"),
        };
        h.key(key, Modifiers::NONE);
    }
    h.key(Key::Enter, Modifiers::NONE);
}

// Spec: grooves R26 (L-GRV-003). Each field's drag, arrow keys and typing run
// `groove.set`, one undo step for each gesture, Base from its list; a value
// out of range is brought within it.
#[test]
fn each_fields_drag_arrows_and_typing_run_groove_set_one_undo_step_per_gesture() {
    let mut h = shown();
    let first = groove(&h, 0);
    assert_eq!(
        (first.quantize, first.timing, first.velocity),
        (0.0, 100.0, 0.0)
    );
    let steps_before = steps(&h);

    // A drag up is one undo step, however far it goes.
    let at = field_at(&mut h, "Quantize");
    drag_by(&mut h, at, -7.0, 6);
    let dragged = groove(&h, 0).quantize;
    assert!(dragged > 5.0, "a step every 3 points: {dragged}");
    assert_eq!(steps(&h), steps_before + 1, "one undo step for the drag");
    assert_eq!(last_step(&h), "Change Quantize");

    // The arrow keys, with the field focused: a step each.
    let at = field_at(&mut h, "Quantize");
    h.click(at);
    h.key(Key::ArrowUp, Modifiers::NONE);
    assert_eq!(groove(&h, 0).quantize, dragged + 1.0);
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.key(Key::ArrowDown, Modifiers::NONE);
    assert_eq!(groove(&h, 0).quantize, dragged - 1.0);
    assert_eq!(steps(&h), steps_before + 4, "a step for each press");

    // Typing: digits, a decimal point, Enter; out of range is brought in.
    let at = field_at(&mut h, "Timing");
    h.click(at);
    type_in(&mut h, "62.5");
    assert_eq!(groove(&h, 0).timing, 62.5);
    assert_eq!(last_step(&h), "Change Timing");
    type_in(&mut h, "500");
    assert_eq!(groove(&h, 0).timing, 100.0);
    // Esc drops what was typed.
    let before = steps(&h);
    h.key(Key::Num7, Modifiers::NONE);
    h.key(Key::Escape, Modifiers::NONE);
    h.key(Key::Enter, Modifiers::NONE);
    assert_eq!(groove(&h, 0).timing, 100.0);
    assert_eq!(steps(&h), before);

    // Velocity runs from -100 to +100.
    let at = field_at(&mut h, "Velocity");
    h.click(at);
    type_in(&mut h, "-40");
    assert_eq!(groove(&h, 0).velocity, -40.0);
    assert_eq!(last_step(&h), "Change Velocity");
    type_in(&mut h, "-400");
    assert_eq!(groove(&h, 0).velocity, -100.0);
    let at = field_at(&mut h, "Random");
    h.click(at);
    type_in(&mut h, "30");
    assert_eq!(groove(&h, 0).random, 30.0);

    // Base is chosen from its list.
    let all = nodes(&mut h);
    let base = centre(
        all.iter()
            .find(|n| n.label == "Base" && n.role == Role::ComboBox)
            .unwrap(),
    );
    h.click(base);
    let items = nodes(&mut h);
    let eighth = items
        .iter()
        .find(|n| n.label == "1/8" && n.bounds.is_some())
        .expect("the list holds 1/8");
    h.click(centre(eighth));
    assert_eq!(groove(&h, 0).base, bl_model::GrooveBase::Eighth);
    assert_eq!(last_step(&h), "Change Base");
}

/// The path of one of the bundled grooves' files.
fn bundled(name: &str) -> String {
    bl_shell::resources::grooves()
        .expect("the bundled grooves")
        .join(format!("{name}.blgroove"))
        .to_string_lossy()
        .into_owned()
}

fn load(h: &mut Harness, name: &str) -> GrooveId {
    let loaded = h
        .shell
        .run("groove.load", json!({ "path": bundled(name) }))
        .unwrap();
    h.settle();
    serde_json::from_value(loaded["groove"].clone()).unwrap()
}

fn row_of<'a>(nodes: &'a [Node], name: &str) -> &'a Node {
    nodes
        .iter()
        .find(|n| n.role == Role::Row && n.label == name)
        .unwrap_or_else(|| panic!("no row {name:?}"))
}

// Spec: grooves R26 (L-GRV-003). Clicking a groove's name selects it, Up and
// Down move the selection, Cmd+R renames it in place and Delete deletes it,
// each one undo step.
#[test]
fn selecting_a_row_renames_with_cmd_r_and_deletes_with_delete() {
    let mut h = shown();
    let swing8 = load(&mut h, "Swing 8ths 55");
    let all = nodes(&mut h);
    assert_eq!(row_of(&all, "Swing 16ths 60").selected, Some(false));
    let name = centre(
        all.iter()
            .find(|n| n.role == Role::Cell && n.label == "Swing 8ths 55")
            .expect("its name"),
    );
    h.click(name);
    let all = nodes(&mut h);
    assert_eq!(row_of(&all, "Swing 8ths 55").selected, Some(true));
    assert_eq!(row_of(&all, "Swing 16ths 60").selected, Some(false));
    // The pool has the keyboard, so the browser's keys give way.
    assert_eq!(
        h.shell.browser_view().focus(),
        bl_shell::browser::Part::GroovePool
    );

    // Up selects the one above, Down the one below.
    h.key(Key::ArrowUp, Modifiers::NONE);
    assert_eq!(
        row_of(&nodes(&mut h), "Swing 16ths 60").selected,
        Some(true)
    );
    h.key(Key::ArrowDown, Modifiers::NONE);
    assert_eq!(row_of(&nodes(&mut h), "Swing 8ths 55").selected, Some(true));

    // Cmd+R types a new name in place; Enter keeps it.
    let before = steps(&h);
    h.key(Key::R, common::command());
    let editing = nodes(&mut h);
    let field = editing
        .iter()
        .find(|n| n.role == Role::TextInput && n.label == "Groove Name")
        .expect("the name's field opens");
    assert!(field.bounds.is_some());
    h.frame(vec![Event::Text("Shuffle".to_owned())]);
    h.key(Key::Enter, Modifiers::NONE);
    let state = h.shell.groove_pool();
    assert_eq!(state.grooves[1].groove.name, "Shuffle");
    assert_eq!(steps(&h), before + 1);
    assert_eq!(last_step(&h), "Rename Groove");
    // Esc drops a name typed.
    h.key(Key::R, common::command());
    h.frame(vec![Event::Text("Dropped".to_owned())]);
    h.key(Key::Escape, Modifiers::NONE);
    assert_eq!(h.shell.groove_pool().grooves[1].groove.name, "Shuffle");
    assert_eq!(steps(&h), before + 1);

    // Delete deletes the selected groove, and the clips that used it have
    // none.
    let clip = clip(&mut h, "Riff", 0, 2);
    h.shell
        .run(
            "clip.set_groove",
            json!({ "clips": [clip], "groove": swing8 }),
        )
        .unwrap();
    h.settle();
    h.key(Key::Delete, Modifiers::NONE);
    let ids = groove_ids(&mut h);
    assert!(!ids.contains(&swing8));
    assert_eq!(last_step(&h), "Delete Groove");
    assert_eq!(h.shell.editor().document().clip(clip).unwrap().groove, None);
    assert!(
        !nodes(&mut h)
            .iter()
            .any(|n| n.role == Role::Row && n.label == "Shuffle")
    );
}

fn button<'a>(nodes: &'a [Node], label: &str) -> &'a Node {
    named(nodes, label)
        .into_iter()
        .find(|n| n.role == Role::Button)
        .unwrap_or_else(|| panic!("no button {label:?}"))
}

fn view_hot_swap(h: &Harness) -> Option<bl_shell::groove_commands::GrooveHotSwap> {
    h.shell.groove_pool_view().hot_swap
}

// Spec: grooves R27 (L-GRV-003). A row's save button writes the groove into
// the User Library's Grooves folder, and its commit button commits it into
// every clip that uses it, in one undo step; with no clip using the groove it
// is disabled.
#[test]
fn the_save_and_commit_buttons_write_a_file_and_the_clips() {
    let scratch = Scratch::new("buttons");
    let mut h = shown();
    let library = scratch.0.join("User Library");
    bl_shell::browser::places::create_user_library(&library).unwrap();
    h.shell.set_user_library(Some(library.clone()));
    let groove = groove_ids(&mut h)[0];

    // Nothing uses the groove: Commit is disabled.
    let all = nodes(&mut h);
    assert!(button(&all, "Commit Groove").disabled);
    assert!(!button(&all, "Save Groove").disabled);
    h.click(centre(button(&all, "Save Groove")));
    let saved = library.join("Grooves").join("Swing 16ths 60.blgroove");
    assert!(saved.is_file(), "{saved:?}");
    assert!(bl_project::groove_file::GrooveFile::load(&saved).is_ok());
    // The Grooves label lists it with the bundled ones.
    h.shell
        .run("browser.open_place", json!({ "label": "grooves" }))
        .unwrap();
    let listed: bl_shell::browser_commands::BrowserState = serde_json::from_value(
        h.shell
            .run("browser.state", json!({ "limit": 100 }))
            .unwrap(),
    )
    .unwrap();
    let names = listed
        .rows
        .iter()
        .filter(|r| r.name == "Swing 16ths 60.blgroove")
        .count();
    assert_eq!(names, 2, "the bundled groove and the saved one");

    // Two clips use it; Commit writes it into both, as one step.
    let (a, b) = (clip(&mut h, "A", 0, 2), clip(&mut h, "B", 4, 2));
    h.shell
        .run(
            "clip.set_groove",
            json!({ "clips": [a, b], "groove": groove }),
        )
        .unwrap();
    let all = nodes(&mut h);
    assert!(!button(&all, "Commit Groove").disabled);
    let before = steps(&h);
    h.click(centre(button(&all, "Commit Groove")));
    assert_eq!(steps(&h), before + 1);
    assert_eq!(last_step(&h), "Commit Groove");
    for clip in [a, b] {
        assert_eq!(h.shell.editor().document().clip(clip).unwrap().groove, None);
    }
    assert!(button(&nodes(&mut h), "Commit Groove").disabled);
}

// Spec: grooves R31, R32 (L-GRV-006). The footer's switch runs
// `groove.set_auto_load`; the hot-swap buttons of a row, of a clip and of the
// footer arm the hot-swap, which tops the browser with a bar and the
// Grooves label; loading a groove from the browser replaces it, and the bar's
// close button, the button again and Esc end it.
#[test]
fn hot_swapping_from_the_pool_replaces_the_groove_and_each_way_ends_it() {
    let mut h = shown();
    let groove = groove_ids(&mut h)[0];
    let used = clip(&mut h, "Riff", 0, 2);
    h.shell
        .run(
            "clip.set_groove",
            json!({ "clips": [used], "groove": groove }),
        )
        .unwrap();
    let all = nodes(&mut h);
    assert!(
        !all.iter().any(|n| n.label == "Close Hot-Swap"),
        "no bar while nothing is armed"
    );
    // The row's button arms it: the bar tops the browser, which shows Grooves.
    h.click(centre(button(&all, "Hot-Swap Groove")));
    assert_eq!(
        view_hot_swap(&h),
        Some(bl_shell::groove_commands::GrooveHotSwap::Groove(groove))
    );
    let all = nodes(&mut h);
    let close = Harness::node(&all, "Close Hot-Swap");
    assert_eq!(close.description, "Swapping Groove");
    assert_eq!(
        h.shell.browser_view().label(),
        &bl_shell::browser_commands::Label::Grooves
    );
    assert!(
        button(&all, "Hot-Swap Groove").toggled == Some(true),
        "the button is lit"
    );

    // Loading a groove from the browser replaces it for the clip, in one step.
    let steps_before = steps(&h);
    h.shell
        .run(
            "browser.select",
            json!({ "paths": [bundled("Swing 8ths 75")] }),
        )
        .unwrap();
    h.shell.set_focused_area(bl_shell::Area::Browser);
    h.shell.invoke("browser.load_selected", json!({})).unwrap();
    h.settle();
    assert_eq!(steps(&h), steps_before + 1);
    assert_eq!(last_step(&h), "Hot-Swap Groove");
    let now = h.shell.groove_pool();
    assert_eq!(now.grooves.len(), 1);
    assert_eq!(now.grooves[0].groove.name, "Swing 8ths 75");
    assert_eq!(
        h.shell.editor().document().clip(used).unwrap().groove,
        Some(now.grooves[0].groove.id)
    );
    assert!(view_hot_swap(&h).is_some(), "it stays on");

    // The bar's close button ends it.
    let all = nodes(&mut h);
    h.click(centre(Harness::node(&all, "Close Hot-Swap")));
    assert_eq!(view_hot_swap(&h), None);
    // The footer's button arms the auto-load groove; Esc ends it.
    let all = nodes(&mut h);
    h.click(centre(button(&all, "Hot-Swap Auto Load Groove")));
    assert_eq!(
        view_hot_swap(&h),
        Some(bl_shell::groove_commands::GrooveHotSwap::AutoLoad)
    );
    h.key(Key::Escape, Modifiers::NONE);
    assert_eq!(view_hot_swap(&h), None);
    // The button again ends it.
    let all = nodes(&mut h);
    h.click(centre(button(&all, "Hot-Swap Groove")));
    assert!(view_hot_swap(&h).is_some());
    let all = nodes(&mut h);
    h.click(centre(button(&all, "Hot-Swap Groove")));
    assert_eq!(view_hot_swap(&h), None);
    assert!(!nodes(&mut h).iter().any(|n| n.label == "Close Hot-Swap"));

    // The footer's switch runs `groove.set_auto_load`.
    let all = nodes(&mut h);
    let switch = Harness::node(&all, "Auto Load Groove");
    assert_eq!(switch.value, "on");
    h.click(centre(switch));
    assert!(!h.shell.groove_pool().auto_load);
    assert_eq!(last_step(&h), "Change Auto Load Groove");
}

/// The Control Bar's Global Amount field, which the footer has a field of the
/// same name beside.
fn bar_amount(nodes: &[Node]) -> Option<&Node> {
    named(nodes, "Global Amount")
        .into_iter()
        .find(|n| n.bounds.is_some_and(|b| b.y1 <= 61.0))
}

// Spec: grooves R30 (L-CTRL-008). The Control Bar shows the Global Amount
// while the pool holds a groove, everything to its right moving 40 points
// right, and hides it while the pool is empty; it edits the amount as the
// pool's footer does.
#[test]
fn the_control_bars_global_amount_shows_with_a_groove_and_moves_its_neighbours() {
    let mut h = Harness::with_title_bar(Window::Main);
    let metronome = |nodes: &[Node]| rect(Harness::node(nodes, "Metronome")).left();
    let with = nodes(&mut h);
    let field = rect(bar_amount(&with).expect("the pool holds a groove: the field shows"));
    assert_eq!((field.left(), field.right()), (209.0, 249.0));
    assert_eq!(metronome(&with), 249.0, "40 points right of 209");
    assert_eq!(
        rect(Harness::node(&with, "Time Signature Denominator")).left(),
        189.0
    );
    let play = rect(Harness::node(&with, "Play")).left();

    // The amount reads as a percentage: 0 % in the default set.
    assert_eq!(bar_amount(&with).unwrap().value, "0 %");
    // It edits as the other fields do: Up steps, typing sets, one step each.
    let before = steps(&h);
    h.click(centre(bar_amount(&with).unwrap()));
    h.key(Key::ArrowUp, Modifiers::NONE);
    assert_eq!(h.shell.groove_pool().global_amount, 1.0);
    type_in(&mut h, "130");
    assert_eq!(h.shell.groove_pool().global_amount, 130.0);
    assert_eq!(steps(&h), before + 2);
    assert_eq!(last_step(&h), "Change Global Amount");
    // A drag is one undo step.
    let at = centre(bar_amount(&nodes(&mut h)).unwrap());
    drag_by(&mut h, at, 7.0, 5);
    assert!(h.shell.groove_pool().global_amount < 130.0);
    assert_eq!(steps(&h), before + 3);

    // The pool's footer shows the same amount, and edits it.
    h.shell.run("view.toggle_groove_pool", json!({})).unwrap();
    let both = nodes(&mut h);
    assert_eq!(named(&both, "Global Amount").len(), 2);
    h.shell.run("view.toggle_groove_pool", json!({})).unwrap();

    // With no groove in the pool the field is gone and the rest moves back.
    let ids = groove_ids(&mut h);
    h.shell
        .run("groove.delete", json!({ "groove": ids[0] }))
        .unwrap();
    let without = nodes(&mut h);
    assert!(bar_amount(&without).is_none());
    assert_eq!(metronome(&without), 209.0);
    assert_eq!(
        rect(Harness::node(&without, "Play")).left(),
        play,
        "the middle group stays where it was at this width"
    );
    // Loading a groove brings it back.
    load(&mut h, "Straight 8ths");
    assert!(bar_amount(&nodes(&mut h)).is_some());
}

/// A harness with two MIDI clips selected and the Clip View shown, and the
/// clips' IDs.
fn clip_view_on_two_clips() -> (Harness, ClipId, ClipId) {
    let mut h = Harness::with_title_bar(Window::Main);
    let (a, b) = (clip(&mut h, "A", 0, 2), clip(&mut h, "B", 4, 2));
    h.shell
        .run("arrangement.select_clips", json!({ "clips": [a, b] }))
        .unwrap();
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    h.settle();
    (h, a, b)
}

fn combo<'a>(nodes: &'a [Node], label: &str) -> &'a Node {
    named(nodes, label)
        .into_iter()
        .find(|n| n.role == Role::ComboBox)
        .unwrap_or_else(|| panic!("no chooser {label:?}"))
}

// Spec: grooves R29 (L-CLIP-004, S10). The Main Properties panel's Groove
// row: the Clip Groove chooser lists None and then the pool's grooves, shows
// the groove of the clip the Clip View shows and sets every clip it shows;
// Hot-Swap Groove makes the clip's groove the browser's target, and Commit
// writes the grooves into the clips.
#[test]
fn the_clip_views_groove_row_sets_every_shown_clip_hot_swaps_and_commits() {
    let (mut h, a, b) = clip_view_on_two_clips();
    let all = nodes(&mut h);
    let chooser = combo(&all, "Groove");
    assert!(!chooser.disabled);
    assert_eq!(chooser.value, "None");
    assert!(
        button(&all, "Hot-Swap Groove").disabled,
        "no groove to swap"
    );
    assert!(button(&all, "Commit").disabled, "no groove to commit");

    // The list: None, then the pool's grooves in pool order.
    let swing8 = load(&mut h, "Swing 8ths 55");
    let all = nodes(&mut h);
    h.click(centre(combo(&all, "Groove")));
    let list = nodes(&mut h);
    let mut items: Vec<(f64, String)> = list
        .iter()
        .filter(|n| {
            n.role != Role::ComboBox && n.bounds.is_some_and(|b| b.x0 >= 249.0 && b.x0 < 420.0)
        })
        .filter(|n| ["None", "Swing 16ths 60", "Swing 8ths 55"].contains(&n.label.as_str()))
        .map(|n| (n.bounds.unwrap().y0, n.label.clone()))
        .collect();
    items.sort_by(|x, y| x.0.total_cmp(&y.0));
    let labels: Vec<&str> = items.iter().map(|(_, l)| l.as_str()).collect();
    assert_eq!(labels, ["None", "Swing 16ths 60", "Swing 8ths 55"]);

    // Choosing one sets both clips the Clip View shows, as one undo step.
    let before = steps(&h);
    let choice = list
        .iter()
        .find(|n| n.label == "Swing 8ths 55" && n.role != Role::ComboBox && n.bounds.is_some())
        .unwrap();
    h.click(centre(choice));
    assert_eq!(steps(&h), before + 1);
    assert_eq!(last_step(&h), "Change Clip Groove");
    for clip in [a, b] {
        assert_eq!(
            h.shell.editor().document().clip(clip).unwrap().groove,
            Some(swing8)
        );
    }
    let all = nodes(&mut h);
    assert_eq!(combo(&all, "Groove").value, "Swing 8ths 55");

    // Hot-Swap Groove makes the clip's groove the browser's hot-swap target.
    assert!(!button(&all, "Hot-Swap Groove").disabled);
    h.click(centre(button(&all, "Hot-Swap Groove")));
    assert_eq!(
        view_hot_swap(&h),
        Some(bl_shell::groove_commands::GrooveHotSwap::Groove(swing8))
    );
    let all = nodes(&mut h);
    assert!(all.iter().any(|n| n.label == "Close Hot-Swap"));
    h.key(Key::Escape, Modifiers::NONE);
    assert_eq!(view_hot_swap(&h), None);

    // Commit writes the grooves into both clips and sets them to None.
    let before = steps(&h);
    let all = nodes(&mut h);
    h.click(centre(button(&all, "Commit")));
    assert_eq!(steps(&h), before + 1);
    assert_eq!(last_step(&h), "Commit Groove");
    for clip in [a, b] {
        assert_eq!(h.shell.editor().document().clip(clip).unwrap().groove, None);
    }
    assert_eq!(combo(&nodes(&mut h), "Groove").value, "None");
}

/// The name of a groove's row in the pool, where it is dragged from.
fn name_of(h: &mut Harness, groove: &str) -> Pos2 {
    let all = nodes(h);
    centre(
        all.iter()
            .find(|n| n.role == Role::Cell && n.label == groove)
            .unwrap_or_else(|| panic!("no groove {groove:?}")),
    )
}

fn pool_centre(h: &mut Harness) -> Pos2 {
    centre_of_group(&nodes(h), "Groove Pool")
}

fn centre_of_group(all: &[Node], label: &str) -> Pos2 {
    centre(
        all.iter()
            .find(|n| n.role == Role::Group && n.label == label)
            .unwrap(),
    )
}

// Spec: grooves R28 (I-DROP-010). A clip dragged from the Arrangement onto
// the pool extracts a groove from it, in one undo step, and the clip stays; a
// groove dragged from the browser onto the pool loads, without applying it.
#[test]
fn a_clip_dropped_on_the_pool_extracts_and_a_groove_file_loads() {
    let mut h = shown();
    let idea = clip(&mut h, "Idea", 2, 4);
    h.settle();
    let steps_before = steps(&h);
    let to = pool_centre(&mut h);

    // The clip's title bar, bars 2 to 6 at 30 points a bar from x 320.
    drag(&mut h, pos2(410.0, 124.0), to);
    assert_eq!(steps(&h), steps_before + 1);
    assert_eq!(last_step(&h), "Extract Groove");
    let state = h.shell.groove_pool();
    assert_eq!(state.grooves.len(), 2);
    assert_eq!(state.grooves[1].groove.name, "Idea");
    assert_eq!(state.grooves[1].groove.length, Ticks(4 * BAR));
    assert!(
        h.shell.editor().document().clip(idea).is_some(),
        "the clip stayed"
    );
    assert_eq!(
        h.shell.editor().document().clip(idea).unwrap().start,
        Ticks(2 * BAR)
    );

    // The Grooves label's file, dragged from the browser onto the pool.
    h.shell
        .run("browser.open_place", json!({ "label": "grooves" }))
        .unwrap();
    h.settle();
    let all = nodes(&mut h);
    let file = centre(
        all.iter()
            .find(|n| n.role == Role::ListItem && n.label == "Straight 16ths.blgroove")
            .or_else(|| {
                all.iter()
                    .find(|n| n.label.starts_with("Straight 16ths") && n.bounds.is_some())
            })
            .expect("the groove file's row"),
    );
    let to = pool_centre(&mut h);
    let steps_before = steps(&h);
    drag(&mut h, file, to);
    assert_eq!(steps(&h), steps_before + 1);
    let state = h.shell.groove_pool();
    assert_eq!(state.grooves.len(), 3);
    assert_eq!(state.grooves[2].groove.name, "Straight 16ths");
    assert!(!state.grooves[2].active, "loaded, not applied");
}

// Spec: grooves R28 (I-DROP-017). A groove dragged from the pool onto a clip
// in the Arrangement becomes the clip's groove; onto a MIDI track's lane it
// becomes a clip of its notes there; into the drop area below the tracks it
// becomes a clip on a new MIDI track; from the browser's file it joins the
// pool first; all in one undo step each.
#[test]
fn a_groove_dropped_on_a_clip_a_midi_lane_or_the_drop_area_applies_or_makes_a_clip() {
    let mut h = shown();
    let target = clip(&mut h, "Riff", 2, 4);
    let swing = load(&mut h, "Swing 8ths 55");
    let tracks_before = h.shell.editor().document().top_level().count();

    // On the clip (bars 2 to 6, the first MIDI track): its groove.
    let steps_before = steps(&h);
    let from = name_of(&mut h, "Swing 8ths 55");
    drag(&mut h, from, pos2(410.0, 130.0));
    assert_eq!(steps(&h), steps_before + 1);
    assert_eq!(last_step(&h), "Change Clip Groove");
    assert_eq!(
        h.shell.editor().document().clip(target).unwrap().groove,
        Some(swing)
    );

    // On the second MIDI track's lane, at bar 8: a clip of its notes.
    let steps_before = steps(&h);
    let from = name_of(&mut h, "Swing 8ths 55");
    drag(&mut h, from, pos2(560.0, 250.0));
    assert_eq!(steps(&h), steps_before + 1);
    assert_eq!(last_step(&h), "Groove to Clip");
    let doc = h.shell.editor().document();
    let second = doc
        .tracks()
        .filter(|t| t.kind() == TrackKind::Midi)
        .nth(1)
        .unwrap();
    let made = second
        .clips()
        .next()
        .expect("a clip on the second MIDI track");
    assert_eq!(made.name, "Swing 8ths 55");
    assert_eq!(made.start, Ticks(8 * BAR));
    assert_eq!(made.length, Ticks(BAR));

    // On an audio track's empty lane: nothing.
    let before = h.shell.editor().document().clone();
    let from = name_of(&mut h, "Swing 8ths 55");
    drag(&mut h, from, pos2(560.0, 340.0));
    assert_eq!(*h.shell.editor().document(), before);

    // In the drop area below the tracks: a new MIDI track holding the clip.
    let from = name_of(&mut h, "Swing 8ths 55");
    drag(&mut h, from, pos2(560.0, 600.0));
    let doc = h.shell.editor().document();
    assert_eq!(doc.top_level().count(), tracks_before + 1);
    let new = doc.top_level().last().unwrap();
    assert_eq!(doc.track(new).unwrap().kind(), TrackKind::Midi);
    assert_eq!(doc.track(new).unwrap().clips().count(), 1);

    // A groove file from the browser onto a clip joins the pool and applies,
    // as one step.
    h.shell
        .run("browser.open_place", json!({ "label": "grooves" }))
        .unwrap();
    h.settle();
    let all = nodes(&mut h);
    let file = centre(
        all.iter()
            .find(|n| n.label.starts_with("Straight 8ths") && n.bounds.is_some())
            .expect("the groove file's row"),
    );
    let pool_before = h.shell.groove_pool().grooves.len();
    let steps_before = steps(&h);
    drag(&mut h, file, pos2(410.0, 130.0));
    assert_eq!(steps(&h), steps_before + 1);
    let state = h.shell.groove_pool();
    assert_eq!(state.grooves.len(), pool_before + 1);
    let loaded = state.grooves.last().unwrap();
    assert_eq!(loaded.groove.name, "Straight 8ths");
    assert_eq!(
        h.shell.editor().document().clip(target).unwrap().groove,
        Some(loaded.groove.id)
    );
}

/// The Session grid's layout, as the view builds it.
struct Grid {
    session: SessionLayout,
    layout: MixerLayout,
    columns: Vec<TrackId>,
    scenes: Vec<SceneId>,
}

impl Grid {
    fn new(h: &Harness) -> Self {
        let view = ShellLayout::compute(REFERENCE, true, true)
            .translate(egui::vec2(0.0, TITLE_BAR))
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
        Self {
            session,
            layout,
            columns,
            scenes,
        }
    }

    fn centre(&self, track: usize, scene: usize) -> Pos2 {
        self.layout.columns[track]
            .rect
            .intersect(self.session.scene_row(scene, 0.0))
            .shrink(1.0)
            .center()
    }
}

// Spec: grooves R28. In the Session View, a clip dropped on the pool extracts a
// groove from it, and a groove dropped on a slot's clip becomes its groove.
#[test]
fn in_the_session_a_clip_extracts_a_groove_and_a_groove_applies_to_a_slot_clip() {
    let mut h = Harness::manual_with_title_bar(Window::Main);
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    h.shell.run("view.toggle_groove_pool", json!({})).unwrap();
    h.settle();
    let grid = Grid::new(&h);
    let (track, scene) = (grid.columns[0], grid.scenes[0]);
    h.shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    h.settle();
    let in_slot = h.shell.editor().document().slot(track, scene).unwrap().id;
    // The factory default set's auto-load groove: the new clip has it.
    let factory = groove_ids(&mut h)[0];
    assert_eq!(
        h.shell.editor().document().clip(in_slot).unwrap().groove,
        Some(factory)
    );

    // The clip onto the pool: a groove of it.
    let to = pool_centre(&mut h);
    let steps_before = steps(&h);
    drag(&mut h, grid.centre(0, 0), to);
    assert_eq!(steps(&h), steps_before + 1);
    assert_eq!(last_step(&h), "Extract Groove");
    assert_eq!(h.shell.groove_pool().grooves.len(), 2);

    // A groove from the pool onto the clip.
    let other = load(&mut h, "Straight 8ths");
    let from = name_of(&mut h, "Straight 8ths");
    let steps_before = steps(&h);
    drag(&mut h, from, grid.centre(0, 0));
    assert_eq!(steps(&h), steps_before + 1);
    assert_eq!(
        h.shell.editor().document().clip(in_slot).unwrap().groove,
        Some(other)
    );
}

// Spec: grooves R33. The Grooves label lists the bundled grooves; Enter and a
// double-click on one load it into the pool, without applying it.
#[test]
fn enter_and_a_double_click_on_a_groove_in_the_browser_load_it_without_applying_it() {
    let mut h = Harness::manual_with_title_bar(Window::Main);
    let used = clip(&mut h, "Riff", 0, 2);
    h.shell
        .run("browser.open_place", json!({ "label": "grooves" }))
        .unwrap();
    h.settle();
    let row = |h: &mut Harness, name: &str| {
        let all = nodes(h);
        centre(
            all.iter()
                .find(|n| n.label.starts_with(name) && n.bounds.is_some_and(|b| b.y0 > 200.0))
                .unwrap_or_else(|| panic!("no row {name:?}")),
        )
    };
    let count = h.shell.groove_pool().grooves.len();

    // Enter on the selected row.
    let at = row(&mut h, "Swing 16ths 75");
    h.click(at);
    h.key(Key::Enter, Modifiers::NONE);
    let pool = h.shell.groove_pool();
    assert_eq!(pool.grooves.len(), count + 1);
    let loaded = pool.grooves.last().unwrap();
    assert_eq!(loaded.groove.name, "Swing 16ths 75");
    assert!(!loaded.active, "loaded without applying it");
    assert_eq!(last_step(&h), "Load Groove");
    assert_eq!(h.shell.editor().document().clip(used).unwrap().groove, None);

    // A double-click on another.
    let at = row(&mut h, "Straight 8ths");
    h.frame(vec![Event::PointerMoved(at)]);
    h.time += 1.0;
    for _ in 0..2 {
        h.frame(vec![press(at, true)]);
        h.frame(vec![press(at, false)]);
    }
    h.settle();
    let pool = h.shell.groove_pool();
    assert_eq!(pool.grooves.len(), count + 2);
    assert_eq!(pool.grooves.last().unwrap().groove.name, "Straight 8ths");
}

// Spec: grooves R35. The pool reaches assistive technology as a table: each
// row a groove, each field a value control with its name and value, each
// button named for what it does, the footer's Global Amount and Auto Load
// Groove; Tab moves between a row's fields and a screen reader's increment
// and decrement step a field.
#[test]
fn the_pool_is_a_table_of_rows_with_named_fields_and_buttons_in_tab_order() {
    let mut h = shown();
    let straight = load(&mut h, "Straight 16ths");
    let used = clip(&mut h, "Riff", 0, 2);
    h.shell
        .run(
            "clip.set_groove",
            json!({ "clips": [used], "groove": straight }),
        )
        .unwrap();
    let all = nodes(&mut h);
    let table = all
        .iter()
        .find(|n| n.role == Role::Table && n.label == "Groove Pool")
        .expect("a table");
    let mut rows: Vec<&Node> = all.iter().filter(|n| n.role == Role::Row).collect();
    assert_eq!(rows.len(), 2, "a row for each groove");
    // In the order they are drawn: a list of nodes is in the order of their IDs.
    rows.sort_by(|a, b| a.bounds.unwrap().y0.total_cmp(&b.bounds.unwrap().y0));
    assert!(
        rows.iter()
            .all(|row| Harness::descends_from(&all, row.id, table.id)),
        "the rows are the table's"
    );
    for (row, name) in rows.iter().zip(["Swing 16ths 60", "Straight 16ths"]) {
        assert_eq!(row.label, name);
        for label in ["Base", "Quantize", "Timing", "Random", "Velocity"] {
            let inside: Vec<&Node> = all
                .iter()
                .filter(|n| n.label == label && Harness::descends_from(&all, n.id, row.id))
                .collect();
            assert_eq!(inside.len(), 1, "{name}: {label}");
        }
        for label in ["Hot-Swap Groove", "Save Groove", "Commit Groove"] {
            assert!(
                all.iter().any(|n| n.role == Role::Button
                    && n.label == label
                    && Harness::descends_from(&all, n.id, row.id)),
                "{name}: {label}"
            );
        }
    }
    // A value control reads its name and its value, and has a numeric value.
    let quantize = all
        .iter()
        .find(|n| n.label == "Quantize" && Harness::descends_from(&all, n.id, rows[0].id))
        .unwrap();
    assert_eq!(
        (quantize.role, quantize.value.as_str()),
        (Role::SpinButton, "0 %")
    );
    assert_eq!(quantize.numeric, Some(0.0));
    let velocity = all
        .iter()
        .find(|n| n.label == "Velocity" && Harness::descends_from(&all, n.id, rows[1].id))
        .unwrap();
    assert_eq!(velocity.value, "0 %");
    // The footer.
    let footer = Harness::node(&all, "Auto Load Groove");
    assert_eq!((footer.role, footer.value.as_str()), (Role::Switch, "on"));
    let amount: Vec<&Node> = named(&all, "Global Amount");
    assert!(amount.iter().any(|n| n.value == "0 %"));

    // Tab runs through the first row's controls, then the second's, then the
    // footer's, in the order they are drawn.
    if !h.shell.tab_navigation() {
        h.shell
            .run("focus.toggle_tab_navigation", json!({}))
            .unwrap();
    }
    h.key(Key::Num6, Modifiers::ALT);
    let mut seen = Vec::new();
    for _ in 0..30 {
        let all = nodes(&mut h);
        let focused = h.ctx.memory(|m| m.focused()).map(|id| id.accesskit_id());
        let label = all
            .iter()
            .find(|n| Some(n.id) == focused)
            .map(|n| n.label.clone())
            .unwrap_or_default();
        if seen.last() != Some(&label) {
            seen.push(label);
        }
        h.key(Key::Tab, Modifiers::NONE);
        if seen.len() > 1 && seen[0] == *seen.last().unwrap() {
            break;
        }
    }
    let at = |label: &str, from: usize| {
        seen.iter()
            .skip(from)
            .position(|l| l == label)
            .map(|p| p + from)
            .unwrap_or_else(|| panic!("{label:?} in {seen:?}"))
    };
    let first = at("Hot-Swap Groove", 0);
    let save = at("Save Groove", first);
    let base = at("Base", save);
    let quantize = at("Quantize", base);
    let timing = at("Timing", quantize);
    let random = at("Random", timing);
    let velocity = at("Velocity", random);
    let second = at("Hot-Swap Groove", velocity);
    let auto = at("Auto Load Groove", second);
    let swap = at("Hot-Swap Auto Load Groove", auto);
    let amount = at("Global Amount", swap);
    assert!(amount > swap, "{seen:?}");

    // A screen reader's increment steps a field, one undo step.
    let field = Harness::node(&all, "Timing").id;
    let before = steps(&h);
    let timing = all
        .iter()
        .find(|n| n.label == "Timing" && Harness::descends_from(&all, n.id, rows[0].id))
        .unwrap();
    let _ = field;
    let id = egui::Id::new(("groove-pool-field", groove_ids(&mut h)[0], "Timing"));
    assert_eq!(timing.id, id.accesskit_id());
    h.accesskit(id, egui::accesskit::Action::Decrement);
    assert_eq!(groove(&h, 0).timing, 99.0);
    assert_eq!(steps(&h), before + 1);
}

/// Whether the frame paints `text` somewhere.
fn painted(output: &egui::FullOutput, text: &str) -> bool {
    fn has(drawn: &egui::Shape, text: &str) -> bool {
        match drawn {
            egui::Shape::Text(t) => t.galley.job.text == text,
            egui::Shape::Vec(shapes) => shapes.iter().any(|one| has(one, text)),
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|clipped| has(&clipped.shape, text))
}

// Spec: grooves R25 (L-GRV-002, L-GRV-004, L-GRV-005). Under the column
// headers Groove Name, Base, Quantize, Timing, Random and Velocity the pool has
// a row for each groove; an empty pool says "Drop Clips or Grooves Here"; the
// footer holds the section's name and the Global Amount; an inactive groove's
// fields are drawn grey and still edit.
#[test]
fn the_pool_has_its_headers_footer_and_empty_text_and_greys_an_inactive_groove() {
    let mut h = shown();
    let output = h.settle();
    for text in [
        "Groove Name",
        "Base",
        "Quantize",
        "Timing",
        "Random",
        "Velocity",
        "Groove Pool",
        "Global Amount",
    ] {
        assert!(painted(&output, text), "{text:?} is painted");
    }
    assert!(!painted(&output, "Drop Clips or Grooves Here"));
    // An inactive groove's fields are grey, an active groove's are not, and
    // either edits.
    let ink = |h: &mut Harness| -> Vec<egui::Color32> {
        fn collect(shape: &egui::Shape, out: &mut Vec<egui::Color32>) {
            match shape {
                egui::Shape::Text(t) if t.galley.job.text == "100 %" => out.push(t.fallback_color),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|one| collect(one, out)),
                _ => {}
            }
        }
        let output = h.settle();
        let mut colours = Vec::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut colours);
        }
        colours
    };
    let inactive = ink(&mut h);
    assert!(!inactive.is_empty(), "the Timing field reads 100 %");
    let used = clip(&mut h, "Riff", 0, 2);
    let groove = groove_ids(&mut h)[0];
    h.shell
        .run(
            "clip.set_groove",
            json!({ "clips": [used], "groove": groove }),
        )
        .unwrap();
    let active = ink(&mut h);
    assert_ne!(inactive, active, "the grey is for an inactive groove");
    let at = field_at(&mut h, "Random");
    h.click(at);
    h.key(Key::ArrowUp, Modifiers::NONE);
    assert_eq!(groove_value(&h), 1.0, "an inactive groove edits");

    // With no groove the pool is empty and says where to drop.
    h.shell
        .run("groove.delete", json!({ "groove": groove }))
        .unwrap();
    let output = h.settle();
    assert!(painted(&output, "Drop Clips or Grooves Here"));
    assert!(nodes(&mut h).iter().all(|n| n.role != Role::Row));
}

fn groove_value(h: &Harness) -> f64 {
    groove(h, 0).random
}

// Spec: grooves R34 (C-CLIP-ARR-009). Extract Groove(s) in an Arrangement clip's
// context menu runs `groove.extract` for the selected clips.
#[test]
fn extract_groove_in_a_clips_context_menu_makes_a_groove_of_it() {
    let mut h = shown();
    let idea = clip(&mut h, "Idea", 2, 4);
    h.settle();
    let at = pos2(410.0, 124.0);
    h.frame(vec![Event::PointerMoved(at)]);
    for pressed in [true, false] {
        h.frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        }]);
    }
    let menu = nodes(&mut h);
    let item = menu
        .iter()
        .rfind(|n| n.label.starts_with("Extract Groove(s)") && n.bounds.is_some())
        .expect("the menu's item");
    assert!(!item.disabled, "a clip is selected");
    let steps_before = steps(&h);
    h.click(centre(item));
    assert_eq!(steps(&h), steps_before + 1);
    assert_eq!(last_step(&h), "Extract Groove");
    let state = h.shell.groove_pool();
    assert_eq!(state.grooves.last().unwrap().groove.name, "Idea");
    assert_eq!(
        h.shell.editor().document().clip(idea).unwrap().start,
        Ticks(2 * BAR)
    );
}

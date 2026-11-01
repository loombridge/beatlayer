//! Show Similar Files, headless (WP-9.9; the sound similarity spec, R11,
//! R12, R14, R18; browser R32): the icon beside a selected file, the item
//! menu's and the View menu's items and Cmd-Shift-F each show the reference
//! as a token in the search field and its neighbours below it, most similar
//! first; a file not analysed or over 60 seconds offers none of them.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::common::{self, Harness, Window};
use bl_shell::browser::places::create_user_library;
use bl_shell::browser_commands::{self as ids, BrowserState, Label};
use bl_shell::menus::{MenuPlatform, menu_bar};
use bl_ui::browser::similar_token_id;
use bl_ui::keymap::Platform;
use bl_ui::main_window::UiEvent;
use bl_ui::menus::item_state;
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2};
use serde_json::json;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-ui-similar-{}-{name}", std::process::id()));
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

/// Three kicks, three hi-hats and a sample over 60 seconds in the User
/// Library, indexed, analysis paced by hand and none of it done yet.
fn harness(scratch: &Scratch) -> (Harness, PathBuf) {
    let root = scratch.0.join("User Library");
    create_user_library(&root).unwrap();
    for (name, hz) in [("Kick 50", 50.0), ("Kick 55", 55.0), ("Kick 60", 60.0)] {
        wav(&root.join(format!("{name}.wav")), hz, 0.4, 22_050);
    }
    for (name, hz) in [
        ("Hat 6k", 6_000.0),
        ("Hat 7k", 7_000.0),
        ("Hat 8k", 8_000.0),
    ] {
        wav(&root.join(format!("{name}.wav")), hz, 0.12, 22_050);
    }
    wav(&root.join("Long.wav"), 110.0, 61.0, 8_000);
    let mut h = Harness::manual_in(Window::Main, &scratch.0.join("config"));
    h.shell.pace_analysis_by_hand();
    h.shell.set_user_library(Some(root.clone()));
    assert!(h.shell.wait_for_browser_index(Duration::from_secs(60)));
    h.shell
        .run(ids::OPEN_PLACE, json!({ "label": "samples" }))
        .unwrap();
    h.shell.set_focused_area(bl_shell::Area::Browser);
    h.settle();
    (h, root)
}

fn analyse(h: &mut Harness) {
    h.shell.release_analysis(7);
    h.settle();
}

fn select(h: &mut Harness, root: &Path, name: &str) {
    let path = root.join(name).to_string_lossy().into_owned();
    h.shell
        .run(ids::SELECT, json!({ "paths": [path] }))
        .unwrap();
    h.settle();
}

fn look(h: &mut Harness) -> Vec<common::Node> {
    Harness::nodes(&h.settle())
}

fn centre_of(nodes: &[common::Node], role: Role, label: &str) -> Pos2 {
    let node = nodes
        .iter()
        .find(|n| n.role == role && n.label == label)
        .unwrap_or_else(|| panic!("no {role:?} labelled {label:?}"));
    let b = node
        .bounds
        .unwrap_or_else(|| panic!("{label:?} has no place"));
    pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
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

/// The item menu's Show Similar Files, which carries its key in its label,
/// unlike the icon's button.
fn menu_row(nodes: &[common::Node]) -> &common::Node {
    nodes
        .iter()
        .find(|n| {
            n.role == Role::Button
                && n.label.starts_with("Show Similar Files")
                && n.label != "Show Similar Files"
        })
        .expect("the menu item")
}

fn centre(node: &common::Node) -> Pos2 {
    let b = node.bounds.unwrap();
    pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
}

fn state(h: &mut Harness) -> BrowserState {
    serde_json::from_value(h.shell.run(ids::STATE, json!({ "limit": 1000 })).unwrap()).unwrap()
}

fn names(h: &Harness) -> Vec<String> {
    let view = h.shell.browser_view();
    (0..view.row_count())
        .map(|i| view.row(i).unwrap().name)
        .collect()
}

fn invoked(h: &mut Harness, id: &str) -> bool {
    h.take_events()
        .iter()
        .any(|e| matches!(e, UiEvent::Invoke(i) if i.id == id))
}

/// What the list and the field show once similar files are shown for
/// Kick 55: the token, then the other kicks before any hi-hat.
fn shows_the_neighbours_of_kick_55(h: &mut Harness) {
    let shown = state(h);
    assert_eq!(shown.label, Label::All);
    assert!(
        shown
            .similar
            .as_deref()
            .is_some_and(|p| p.ends_with("Kick 55.wav")),
        "{:?}",
        shown.similar
    );
    let rows = names(h);
    assert_eq!(rows[0], "Kick 55.wav", "the reference first: {rows:?}");
    assert!(
        rows[1..3].contains(&"Kick 50.wav".to_owned())
            && rows[1..3].contains(&"Kick 60.wav".to_owned()),
        "the nearest kicks next: {rows:?}"
    );
    let nodes = look(h);
    let token = nodes
        .iter()
        .find(|n| n.role == Role::Button && n.label == "Similar to Kick 55.wav")
        .unwrap_or_else(|| panic!("no reference token"));
    assert!(token.bounds.is_some());
    let _ = similar_token_id();
}

// Spec: sound-similarity R11, R18; browser R32. The icon shows beside the
// selected analysed sample, is a button named Show Similar Files, and a
// click lists its neighbours under the token in the search field.
#[test]
fn the_icon_beside_a_selected_sample_shows_similar_files() {
    let scratch = Scratch::new("icon");
    let (mut h, root) = harness(&scratch);
    select(&mut h, &root, "Kick 55.wav");
    let nodes = look(&mut h);
    assert!(
        !nodes.iter().any(|n| n.label == "Show Similar Files"),
        "not analysed yet: no icon"
    );
    analyse(&mut h);
    let nodes = look(&mut h);
    let icon = centre_of(&nodes, Role::Button, "Show Similar Files");
    let row = Harness::node(&nodes, "Kick 55.wav").bounds.unwrap();
    assert!(
        icon.x > (row.x1 - 30.0) as f32,
        "at the right end of its row"
    );
    h.click(icon);
    assert!(invoked(&mut h, ids::SHOW_SIMILAR));
    shows_the_neighbours_of_kick_55(&mut h);

    // The icon stays on the reference's row, and the token ends the search.
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::Button, "Similar to Kick 55.wav"));
    let ended = state(&mut h);
    assert_eq!((ended.similar, ended.label), (None, Label::Samples));
}

// Spec: sound-similarity R11. A sample over 60 seconds and a selection of
// two offer no icon; the menu item is disabled with the reason.
#[test]
fn a_sample_over_sixty_seconds_offers_no_search() {
    let scratch = Scratch::new("long");
    let (mut h, root) = harness(&scratch);
    analyse(&mut h);
    select(&mut h, &root, "Long.wav");
    let nodes = look(&mut h);
    assert!(!nodes.iter().any(|n| n.label == "Show Similar Files"));
    right_click(&mut h, centre_of(&nodes, Role::ListItem, "Long.wav"));
    let nodes = look(&mut h);
    let item = menu_row(&nodes);
    assert!(item.disabled);
    assert!(
        item.description.contains("60 seconds"),
        "{}",
        item.description
    );
    // The View menu's item is disabled too.
    assert!(!menu_item(&h, "M-VIEW-026").enabled);

    let both = ["Kick 50.wav", "Kick 55.wav"].map(|n| root.join(n).to_string_lossy().into_owned());
    h.shell.run(ids::SELECT, json!({ "paths": both })).unwrap();
    let nodes = look(&mut h);
    assert!(!nodes.iter().any(|n| n.label == "Show Similar Files"));
}

fn menu_item(h: &Harness, row: &str) -> bl_ui::menus::ItemState {
    let menus = menu_bar(MenuPlatform::current());
    let item = menus
        .iter()
        .flat_map(bl_shell::menus::Menu::walk)
        .find(|item| item.row == row)
        .unwrap();
    item_state(item, &h.shell, &h.keymap, Platform::current())
}

// Spec: sound-similarity R11; C-BROWSER-003. The item menu's row.
#[test]
fn the_item_menus_row_shows_similar_files() {
    let scratch = Scratch::new("menu");
    let (mut h, root) = harness(&scratch);
    analyse(&mut h);
    let nodes = look(&mut h);
    right_click(&mut h, centre_of(&nodes, Role::ListItem, "Kick 55.wav"));
    let nodes = look(&mut h);
    let item = menu_row(&nodes);
    assert!(!item.disabled);
    h.click(centre(item));
    assert!(invoked(&mut h, ids::SHOW_SIMILAR));
    shows_the_neighbours_of_kick_55(&mut h);
    let _ = root;
}

// Spec: sound-similarity R11; M-VIEW-026, S-BRW-010. The View menu's item
// is live with a sample selected and Cmd-Shift-F (Ctrl-Shift-F) does the
// same.
#[test]
fn the_view_menu_and_cmd_shift_f_show_similar_files() {
    let scratch = Scratch::new("keys");
    let (mut h, root) = harness(&scratch);
    analyse(&mut h);
    assert!(
        !menu_item(&h, "M-VIEW-026").enabled,
        "nothing is selected yet"
    );
    select(&mut h, &root, "Kick 55.wav");
    let item = menu_item(&h, "M-VIEW-026");
    assert!(item.enabled);
    assert_eq!(item.invocation.as_ref().unwrap().id, ids::SHOW_SIMILAR);
    h.take_events();

    let mods = Modifiers {
        shift: true,
        ..common::command()
    };
    h.key(Key::F, mods);
    assert!(invoked(&mut h, ids::SHOW_SIMILAR));
    shows_the_neighbours_of_kick_55(&mut h);

    // Esc ends it and goes back to the browser as it was.
    h.shell
        .run(ids::OPEN_PLACE, json!({ "label": "samples" }))
        .unwrap();
    h.shell.run(ids::SHOW_SIMILAR, json!({})).unwrap();
    h.shell
        .run(ids::FOCUS_BROWSER, json!({ "part": "search" }))
        .unwrap();
    h.settle();
    h.key(Key::Escape, Modifiers::NONE);
    h.settle();
    let ended = state(&mut h);
    assert_eq!((ended.similar, ended.label), (None, Label::Samples));
}

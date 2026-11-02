//! The browser's collections in the sidebar and in the content pane, and a
//! browser item's context menu, headless (WP-6.32b). Spec: browser-tags R34
//! to R39, browser R25.

use crate::common;

use std::path::{Path, PathBuf};

use bl_shell::Effect;
use bl_shell::browser::places::create_user_library;
use bl_shell::browser_commands::{self as ids, BrowserState, Label};
use bl_ui::browser::{collection_colour, collections_edit_id};
use common::{Harness, Window, command, painted_rects};
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2};
use serde_json::json;

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-ui-collections-{}-{name}", std::process::id()));
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

/// A 16-bit mono WAV file at 48 kHz.
fn wav(path: &Path, frames: u32) {
    let mut bytes = Vec::new();
    for chunk in [
        &b"RIFF"[..],
        &(36 + frames * 2).to_le_bytes(),
        b"WAVEfmt ",
        &16u32.to_le_bytes(),
        &1u16.to_le_bytes(),
        &1u16.to_le_bytes(),
        &48_000u32.to_le_bytes(),
        &96_000u32.to_le_bytes(),
        &2u16.to_le_bytes(),
        &16u16.to_le_bytes(),
        b"data",
        &(frames * 2).to_le_bytes(),
    ] {
        bytes.extend_from_slice(chunk);
    }
    for i in 0..frames {
        bytes.extend_from_slice(&(((i % 100) as i16 - 50) * 30).to_le_bytes());
    }
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// A harness with a User Library holding Kick, Pad and Snare, the User
/// Library chosen and the browser focused.
fn harness(scratch: &Scratch) -> (Harness, PathBuf) {
    let root = scratch.0.join("User Library");
    create_user_library(&root).unwrap();
    wav(&root.join("Kick.wav"), 100);
    wav(&root.join("Pad.wav"), 2_000);
    wav(&root.join("Snare.wav"), 400);
    let mut h = Harness::manual_with_title_bar(Window::Main);
    h.shell.set_user_library(Some(root.clone()));
    assert!(
        h.shell
            .wait_for_browser_index(std::time::Duration::from_secs(20))
    );
    h.shell
        .run(ids::OPEN_PLACE, json!({ "label": "user_library" }))
        .unwrap();
    h.shell.set_focused_area(bl_shell::Area::Browser);
    h.settle();
    (h, root)
}

fn look(h: &mut Harness) -> Vec<common::Node> {
    let output = h.settle();
    Harness::nodes(&output)
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

/// The centre of the Collections heading's button, which Edit and Done share:
/// the Library's heading has an Edit button of its own with the same name.
fn collections_button(nodes: &[common::Node]) -> Pos2 {
    let node = nodes
        .iter()
        .find(|n| n.id == collections_edit_id().accesskit_id())
        .expect("the Collections heading's button");
    let b = node.bounds.expect("the button has a place");
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

fn state(h: &mut Harness) -> BrowserState {
    serde_json::from_value(h.shell.run(ids::STATE, json!({ "limit": 1000 })).unwrap()).unwrap()
}

/// The collections the row named `name` is in.
fn collections_of(h: &mut Harness, name: &str) -> Vec<u8> {
    state(h)
        .rows
        .iter()
        .find(|row| row.name == name)
        .map(|row| row.collections.clone())
        .unwrap_or_default()
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The middle of the button whose label starts with `label`: a menu entry
/// with a key shown beside it reads its key after its name.
fn button(nodes: &[common::Node], label: &str) -> Pos2 {
    let node = nodes
        .iter()
        .find(|n| n.role == Role::Button && n.label.starts_with(label))
        .unwrap_or_else(|| panic!("no Button labelled {label:?}"));
    let b = node.bounds.expect("the button's place");
    pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
}

/// The menu's buttons: their labels in the order the menu draws them, and
/// whether each is disabled.
fn menu_buttons(nodes: &[common::Node], wanted: &[&str]) -> Vec<(String, bool)> {
    wanted
        .iter()
        .filter_map(|label| {
            nodes
                .iter()
                .find(|n| n.role == Role::Button && n.label.starts_with(label))
                .map(|n| ((*label).to_owned(), n.disabled))
        })
        .collect()
}

const FINDER: &str = if cfg!(target_os = "macos") {
    "Show in Finder"
} else {
    "Show in File Manager"
};

// Spec: browser-tags R37. A row shows a small square for each collection the
// item is in, in the collections' order, three at most; assistive technology
// is told all of them.
#[test]
fn a_row_shows_three_squares_of_four_collections() {
    let scratch = Scratch::new("squares");
    let (mut h, root) = harness(&scratch);
    let pad = text(&root.join("Pad.wav"));
    for number in [4, 1, 3, 2] {
        h.shell
            .run(
                ids::SET_COLLECTION,
                json!({ "collection": number, "items": [pad], "member": true }),
            )
            .unwrap();
    }
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    let row = Harness::node(&nodes, "Pad.wav");
    let bounds = row.bounds.expect("the row's place");
    assert_eq!(
        row.description,
        "in collections Favorites, Orange, Yellow, Green"
    );
    let mut squares: Vec<(f32, egui::Color32)> = painted_rects(&output)
        .into_iter()
        .filter(|(rect, _)| {
            (rect.width() - 7.0).abs() < 0.01
                && (rect.height() - 7.0).abs() < 0.01
                && f64::from(rect.center().y) > bounds.y0
                && f64::from(rect.center().y) < bounds.y1
        })
        .map(|(rect, fill)| (rect.left(), fill))
        .collect();
    squares.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert_eq!(
        squares.iter().map(|(_, fill)| *fill).collect::<Vec<_>>(),
        [1, 2, 3].map(|n| collection_colour(&h.theme, n)),
        "three of the four, in the collections' order"
    );
    // A row in none shows none.
    let kick = Harness::node(&nodes, "Kick.wav").bounds.unwrap();
    assert!(
        !painted_rects(&output)
            .iter()
            .any(|(rect, _)| (rect.width() - 7.0).abs() < 0.01
                && f64::from(rect.center().y) > kick.y0
                && f64::from(rect.center().y) < kick.y1)
    );
}

// Spec: browser R25, browser-tags R35; C-BROWSER-001 to 011. A right-click
// opens the item's menu: Show in Places away from the item's place, Show in
// Finder, Show Similar Files disabled, Clear All Colors and the seven
// collections, acting on the selection.
#[test]
fn the_item_context_menu_has_its_items_and_acts_on_the_selection() {
    let scratch = Scratch::new("menu");
    let (mut h, root) = harness(&scratch);
    let nodes = look(&mut h);
    // A right-click selects the row it is on, and opens the menu.
    right_click(&mut h, centre_of(&nodes, Role::ListItem, "Pad.wav"));
    assert_eq!(
        h.shell.browser_view().selected(),
        [root.join("Pad.wav")],
        "the row is selected"
    );
    let nodes = look(&mut h);
    let wanted = [
        "Show in Places",
        FINDER,
        "Show Similar Files",
        "Clear All Colors",
        "Favorites",
        "Orange",
        "Yellow",
        "Green",
        "Blue",
        "Purple",
        "Gray",
    ];
    let buttons = menu_buttons(&nodes, &wanted);
    let present: Vec<&str> = buttons.iter().map(|(label, _)| label.as_str()).collect();
    // Under the place the item lies in, Show in Places isn't offered.
    assert_eq!(present, wanted[1..], "{buttons:?}");
    assert_eq!(
        buttons
            .iter()
            .filter(|(_, disabled)| *disabled)
            .map(|(label, _)| label.as_str())
            .collect::<Vec<_>>(),
        ["Show Similar Files"]
    );
    // The keys show beside the entries.
    for (name, key) in [
        ("Clear All Colors", "0"),
        ("Favorites", "1"),
        ("Yellow", "3"),
        ("Gray", "7"),
    ] {
        let node = nodes
            .iter()
            .find(|n| n.role == Role::Button && n.label.starts_with(name))
            .unwrap();
        assert!(node.label.contains(key), "{:?}", node.label);
    }
    h.click(button(&nodes, "Yellow"));
    assert_eq!(collections_of(&mut h, "Pad.wav"), [3]);
    // Several selected items: the menu acts on all of them.
    h.shell
        .run(
            ids::SELECT,
            json!({ "paths": [text(&root.join("Pad.wav")), text(&root.join("Kick.wav"))] }),
        )
        .unwrap();
    let nodes = look(&mut h);
    right_click(&mut h, centre_of(&nodes, Role::ListItem, "Pad.wav"));
    assert_eq!(h.shell.browser_view().selected().len(), 2, "kept");
    let nodes = look(&mut h);
    h.click(button(&nodes, "Gray"));
    assert_eq!(collections_of(&mut h, "Pad.wav"), [3, 7]);
    assert_eq!(collections_of(&mut h, "Kick.wav"), [7]);
    let nodes = look(&mut h);
    right_click(&mut h, centre_of(&nodes, Role::ListItem, "Kick.wav"));
    let nodes = look(&mut h);
    h.click(button(&nodes, "Clear All Colors"));
    assert!(collections_of(&mut h, "Pad.wav").is_empty());
    assert!(collections_of(&mut h, "Kick.wav").is_empty());
}

// Spec: stem-separation R22, R31. An audio file's menu offers Separate Stems
// to New Audio Tracks after Show Similar Files, which opens the dialog on
// the file; a folder's menu offers it disabled.
#[test]
fn separate_stems_in_an_audio_files_menu_opens_the_dialog_on_the_file() {
    if bl_shell::stems::packages().is_empty() {
        return;
    }
    let scratch = Scratch::new("stems");
    let (mut h, root) = harness(&scratch);
    let nodes = look(&mut h);
    right_click(&mut h, centre_of(&nodes, Role::ListItem, "Pad.wav"));
    let nodes = look(&mut h);
    let stems = "Separate Stems to New Audio Tracks";
    let top = |label: &str| {
        let node = nodes
            .iter()
            .find(|n| n.role == Role::Button && n.label.starts_with(label))
            .unwrap_or_else(|| panic!("no {label:?}"));
        node.bounds.unwrap().y0
    };
    assert!(
        top("Show Similar Files") < top(stems),
        "after Show Similar Files"
    );
    assert!(
        top(stems) < top("Clear All Colors"),
        "before the collections"
    );
    assert_eq!(menu_buttons(&nodes, &[stems]), [(stems.to_owned(), false)]);
    h.click(button(&nodes, stems));
    let dialog = h.shell.stems_dialog().expect("the dialog is open");
    assert_eq!(
        dialog.source,
        bl_shell::stems_commands::StemSource::File {
            path: text(&root.join("Pad.wav"))
        }
    );
}

// Spec: browser R25. Under a collection the menu offers Show in Places, which
// shows the item in its place, and Show in Finder asks the host to show it;
// for a device, which lies in no place and is no file, both are drawn
// disabled.
#[test]
fn show_in_places_and_show_in_finder_in_the_menu() {
    let scratch = Scratch::new("places");
    let (mut h, root) = harness(&scratch);
    let pad = text(&root.join("Pad.wav"));
    h.shell
        .run(
            ids::SET_COLLECTION,
            json!({ "collection": 5, "items": [pad, "rack:audio_effect"], "member": true }),
        )
        .unwrap();
    h.shell
        .run(ids::OPEN_PLACE, json!({ "label": { "collection": 5 } }))
        .unwrap();
    let nodes = look(&mut h);
    right_click(
        &mut h,
        centre_of(&nodes, Role::ListItem, "Audio Effect Rack"),
    );
    let nodes = look(&mut h);
    let buttons = menu_buttons(&nodes, &["Show in Places", FINDER]);
    assert_eq!(
        buttons,
        [
            ("Show in Places".to_owned(), true),
            (FINDER.to_owned(), true)
        ],
        "a device is in no place and is no file"
    );
    // Close the menu, and open the file's.
    h.key(Key::Escape, Modifiers::NONE);
    let nodes = look(&mut h);
    right_click(&mut h, centre_of(&nodes, Role::ListItem, "Pad.wav"));
    let nodes = look(&mut h);
    let buttons = menu_buttons(&nodes, &["Show in Places", FINDER]);
    assert_eq!(
        buttons,
        [
            ("Show in Places".to_owned(), false),
            (FINDER.to_owned(), false)
        ]
    );
    h.click(centre_of(&nodes, Role::Button, FINDER));
    assert!(
        h.effects
            .contains(&Effect::ShowInFileManager(vec![root.join("Pad.wav")])),
        "{:?}",
        h.effects
    );
    let nodes = look(&mut h);
    right_click(&mut h, centre_of(&nodes, Role::ListItem, "Pad.wav"));
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::Button, "Show in Places"));
    let after = state(&mut h);
    assert_eq!(after.label, Label::UserLibrary);
    assert_eq!(after.selected, [text(&root.join("Pad.wav"))]);
}

// Spec: browser-tags R35; S-BRW-008, S-BRW-009. The keys 1 to 7 put the
// selected items in a collection and take them out again, and 0 takes them out
// of every one.
#[test]
fn the_number_keys_put_the_selection_in_collections() {
    let scratch = Scratch::new("keys");
    let (mut h, root) = harness(&scratch);
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::ListItem, "Pad.wav"));
    h.key(Key::Num3, Modifiers::NONE);
    assert_eq!(collections_of(&mut h, "Pad.wav"), [3]);
    h.key(Key::Num3, Modifiers::NONE);
    assert!(
        collections_of(&mut h, "Pad.wav").is_empty(),
        "again takes it out"
    );
    for (key, number) in [
        (Key::Num1, 1),
        (Key::Num2, 2),
        (Key::Num4, 4),
        (Key::Num5, 5),
        (Key::Num6, 6),
        (Key::Num7, 7),
    ] {
        h.key(key, Modifiers::NONE);
        assert!(
            collections_of(&mut h, "Pad.wav").contains(&number),
            "{key:?} puts it in collection {number}"
        );
    }
    assert_eq!(collections_of(&mut h, "Pad.wav"), [1, 2, 4, 5, 6, 7]);
    h.key(Key::Num0, Modifiers::NONE);
    assert!(collections_of(&mut h, "Pad.wav").is_empty());
    // Three selected items go in together.
    let all = ["Kick.wav", "Pad.wav", "Snare.wav"].map(|n| text(&root.join(n)));
    h.shell.run(ids::SELECT, json!({ "paths": all })).unwrap();
    h.key(Key::Num3, Modifiers::NONE);
    for name in ["Kick.wav", "Pad.wav", "Snare.wav"] {
        assert_eq!(collections_of(&mut h, name), [3], "{name}");
    }
    // A key with a modifier is not a collection's.
    h.key(Key::Num2, command());
    assert_eq!(collections_of(&mut h, "Pad.wav"), [3]);
}

// Spec: browser-tags R39. Edit, to the right of the Collections heading, gives
// each collection a check box that shows or hides it; Done ends it; a hidden
// collection comes back when an item is put in it.
#[test]
fn edit_and_done_show_and_hide_collections() {
    let scratch = Scratch::new("edit");
    let (mut h, root) = harness(&scratch);
    let nodes = look(&mut h);
    let heading = Harness::node(&nodes, "Collections").bounds.unwrap();
    h.frame(vec![Event::PointerMoved(pos2(
        heading.x1 as f32 - 8.0,
        (heading.y0 + heading.y1) as f32 / 2.0,
    ))]);
    let nodes = look(&mut h);
    let edit = nodes
        .iter()
        .find(|n| n.id == collections_edit_id().accesskit_id())
        .expect("the Collections heading's button");
    assert_eq!((edit.role, edit.label.as_str()), (Role::Button, "Edit"));
    h.click(collections_button(&nodes));
    let nodes = look(&mut h);
    assert!(
        nodes
            .iter()
            .any(|n| n.id == collections_edit_id().accesskit_id() && n.label == "Done")
    );
    for name in [
        "Favorites",
        "Orange",
        "Yellow",
        "Green",
        "Blue",
        "Purple",
        "Gray",
    ] {
        let check = nodes
            .iter()
            .find(|n| n.role == Role::CheckBox && n.label == name)
            .unwrap_or_else(|| panic!("a check box for {name}"));
        assert_eq!(check.toggled, Some(true));
    }
    h.click(centre_of(&nodes, Role::CheckBox, "Green"));
    assert!(
        !state(&mut h)
            .labels
            .iter()
            .find(|l| l.label == Label::Collection(4))
            .unwrap()
            .shown
    );
    // Hidden, it still shows while the mode is on; Done takes it out.
    let nodes = look(&mut h);
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::CheckBox && n.label == "Green")
    );
    h.click(collections_button(&nodes));
    let nodes = look(&mut h);
    assert!(
        !nodes
            .iter()
            .any(|n| n.role == Role::ListItem && n.label == "Green"),
        "a hidden collection leaves the sidebar"
    );
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::ListItem && n.label == "Blue")
    );
    // Putting an item in it brings it back.
    h.click(centre_of(&nodes, Role::ListItem, "Pad.wav"));
    h.shell
        .run(
            ids::SET_COLLECTION,
            json!({ "collection": 4, "items": [text(&root.join("Pad.wav"))], "member": true }),
        )
        .unwrap();
    let nodes = look(&mut h);
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::ListItem && n.label == "Green")
    );
}

// Spec: browser-tags R38. A collection is renamed by its context menu or with
// Cmd-R while the sidebar has the keyboard; the name shows in the sidebar.
#[test]
fn a_collection_is_renamed_by_its_menu_and_by_cmd_r() {
    let scratch = Scratch::new("rename");
    let (mut h, _) = harness(&scratch);
    let nodes = look(&mut h);
    right_click(&mut h, centre_of(&nodes, Role::ListItem, "Yellow"));
    let nodes = look(&mut h);
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::Button && n.label == "Rename")
    );
    assert!(
        !nodes
            .iter()
            .any(|n| n.role == Role::Button && n.label.starts_with("Choose Icon")),
        "a collection has its colour, not an icon"
    );
    h.click(centre_of(&nodes, Role::Button, "Rename"));
    assert_eq!(state(&mut h).renaming_label, Some(Label::Collection(3)));
    h.settle();
    h.frame(vec![Event::Text("Keepers".to_owned())]);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    let after = state(&mut h);
    assert_eq!(after.renaming_label, None);
    assert!(
        after.labels[2].name.ends_with("Keepers"),
        "{:?}",
        after.labels[2]
    );

    // Cmd-R with the collection chosen and the sidebar focused.
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::ListItem, &after.labels[1].name));
    assert_eq!(state(&mut h).label, Label::Collection(2));
    h.key(Key::R, command());
    assert_eq!(state(&mut h).renaming_label, Some(Label::Collection(2)));
}

// Spec: browser-tags R36. Choosing a collection in the sidebar lists its items.
#[test]
fn choosing_a_collection_lists_its_items() {
    let scratch = Scratch::new("choose");
    let (mut h, root) = harness(&scratch);
    for name in ["Snare.wav", "Kick.wav"] {
        h.shell
            .run(
                ids::SET_COLLECTION,
                json!({ "collection": 2, "items": [text(&root.join(name))], "member": true }),
            )
            .unwrap();
    }
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::ListItem, "Orange"));
    let after = state(&mut h);
    assert_eq!(after.label, Label::Collection(2));
    assert_eq!(
        after
            .rows
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        ["Kick.wav", "Snare.wav"]
    );
    let nodes = look(&mut h);
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::ListItem && n.label == "Orange" && n.selected == Some(true)),
        "the chosen collection is selected in the sidebar"
    );
}

// Spec: browser-tags R41, R43; L-BRW-016. A plug-in row's menu ends with
// Hide Plug-In, after Gray (7); it hides the plug-in and the Content Options
// menu's Show Hidden Plug-Ins lists it again, where the row's menu offers
// Show Plug-In.
#[test]
fn a_plug_in_rows_menu_hides_it_and_content_options_shows_hidden_plug_ins() {
    if bl_plugin_host::scan::scan_program().is_none() {
        eprintln!("beatlayer-scan wasn't built; skipping (build the workspace's bins)");
        return;
    }
    let scratch = Scratch::new("hide");
    let (mut h, _) = harness(&scratch);
    let bundle = text(bl_test_clap::bundle());
    h.shell
        .run(
            bl_shell::plugin_commands::SCAN,
            json!({ "paths": [bundle.clone()] }),
        )
        .unwrap();
    h.shell
        .run(ids::OPEN_PLACE, json!({ "label": "plug_ins" }))
        .unwrap();
    h.shell
        .run(ids::SEARCH, json!({ "query": "gain echo" }))
        .unwrap();
    let nodes = look(&mut h);
    right_click(
        &mut h,
        centre_of(&nodes, Role::ListItem, bl_test_clap::PLUGIN_NAME),
    );
    let nodes = look(&mut h);
    let top = |label: &str| {
        nodes
            .iter()
            .find(|n| n.role == Role::Button && n.label.starts_with(label))
            .unwrap_or_else(|| panic!("no {label:?}"))
            .bounds
            .unwrap()
            .y0
    };
    assert!(top("Gray") < top("Hide Plug-In"), "after Gray");
    h.click(button(&nodes, "Hide Plug-In"));
    let hidden = h.shell.plugin_settings().hidden;
    assert_eq!(hidden, [format!("clap/{}", bl_test_clap::PLUGIN_ID)]);
    assert!(state(&mut h).rows.is_empty(), "hidden: left out");

    // Show Hidden Plug-Ins, in Content Options under the Plug-Ins label.
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::Button, "Content Options"));
    let nodes = look(&mut h);
    h.click(button(&nodes, "Show Hidden Plug-Ins"));
    assert!(h.shell.plugin_settings().show_hidden);
    let shown = state(&mut h);
    assert!(shown.rows[0].hidden, "listed again, dimmed");
    // Its menu offers Show Plug-In.
    let nodes = look(&mut h);
    right_click(
        &mut h,
        centre_of(&nodes, Role::ListItem, bl_test_clap::PLUGIN_NAME),
    );
    let nodes = look(&mut h);
    h.click(button(&nodes, "Show Plug-In"));
    assert!(h.shell.plugin_settings().hidden.is_empty());
    assert!(!state(&mut h).rows[0].hidden);
}

/// Drags from one point to another, in steps, and settles.
fn drag(h: &mut Harness, from: Pos2, to: Pos2) {
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        h.frame(vec![Event::PointerMoved(from + (to - from) * t)]);
    }
    h.frame(vec![Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
}

fn nested_numbers(h: &mut Harness, parent: u32) -> Vec<u32> {
    state(h)
        .labels
        .iter()
        .filter(|l| l.parent == Some(Label::Collection(parent)))
        .filter_map(|l| match l.label {
            Label::Collection(n) => Some(n),
            _ => None,
        })
        .collect()
}

fn has(nodes: &[common::Node], role: Role, label: &str) -> bool {
    nodes.iter().any(|n| n.role == role && n.label == label)
}

// Spec: browser-tags R45, R47; L-BRW-016. A collection's context menu has New
// Collection, which makes one inside it with its name ready to edit; the
// sidebar lists nested collections under a fold triangle, indented; a nested
// collection's menu has Rename, New Collection and Delete, and Delete asks
// first when it holds items.
#[test]
fn the_sidebar_nests_collections_with_fold_triangles_and_menus() {
    let scratch = Scratch::new("nest-sidebar");
    let (mut h, root) = harness(&scratch);
    let nodes = look(&mut h);
    assert!(!nodes.iter().any(|n| n.label.starts_with("Expand")));
    // New Collection in Orange's menu.
    right_click(&mut h, centre_of(&nodes, Role::ListItem, "Orange"));
    let nodes = look(&mut h);
    h.click(button(&nodes, "New Collection"));
    assert_eq!(nested_numbers(&mut h, 2), [8]);
    assert_eq!(
        h.shell.browser_view().renaming_label(),
        Some(&Label::Collection(8)),
        "its name is ready to edit"
    );
    h.shell
        .run(
            ids::RENAME_LABEL,
            json!({ "label": { "collection": 8 }, "name": "Kicks" }),
        )
        .unwrap();
    let nodes = look(&mut h);
    // Orange was opened by it: Kicks shows, indented past Orange.
    let orange = centre_of(&nodes, Role::ListItem, "Orange");
    let kicks = centre_of(&nodes, Role::ListItem, "Kicks");
    assert!(kicks.y > orange.y);
    let left = |label: &str| {
        nodes
            .iter()
            .find(|n| n.role == Role::ListItem && n.label == label)
            .and_then(|n| n.bounds)
            .unwrap()
            .x0
    };
    assert!(left("Kicks") >= left("Orange"), "same row start");
    // The fold triangle closes and opens Orange's list.
    h.click(centre_of(&nodes, Role::Button, "Collapse Orange"));
    let nodes = look(&mut h);
    assert!(!has(&nodes, Role::ListItem, "Kicks"));
    h.click(centre_of(&nodes, Role::Button, "Expand Orange"));
    let nodes = look(&mut h);
    assert!(has(&nodes, Role::ListItem, "Kicks"));
    // Kicks' menu: Rename, New Collection, Delete.
    right_click(&mut h, centre_of(&nodes, Role::ListItem, "Kicks"));
    let nodes = look(&mut h);
    let wanted = ["Rename", "New Collection", "Delete"];
    assert_eq!(menu_buttons(&nodes, &wanted).len(), 3);
    // Delete with items in it asks first.
    h.click(centre_of(&nodes, Role::ListItem, "Kicks"));
    h.shell
        .run(
            ids::SET_COLLECTION,
            json!({ "collection": 8, "items": [text(&root.join("Pad.wav"))], "member": true }),
        )
        .unwrap();
    let nodes = look(&mut h);
    right_click(&mut h, centre_of(&nodes, Role::ListItem, "Kicks"));
    let nodes = look(&mut h);
    h.click(button(&nodes, "Delete"));
    assert_eq!(nested_numbers(&mut h, 2), [8], "it asked first");
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::Button, "Cancel"));
    assert_eq!(nested_numbers(&mut h, 2), [8], "Cancel keeps it");
    let nodes = look(&mut h);
    right_click(&mut h, centre_of(&nodes, Role::ListItem, "Kicks"));
    let nodes = look(&mut h);
    h.click(button(&nodes, "Delete"));
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::Button, "Delete"));
    assert!(nested_numbers(&mut h, 2).is_empty());
    assert!(root.join("Pad.wav").is_file(), "no file is touched");
}

// Spec: browser-tags R46; I-DROP-024, I-DRAG-056. A collection with nested
// ones has a submenu in an item's menu, listing them; items dragged onto a
// nested collection's label go into it, and a nested label dragged onto a
// collection moves into it.
#[test]
fn items_go_into_a_nested_collection_by_its_submenu_and_by_dropping() {
    let scratch = Scratch::new("nest-items");
    let (mut h, _) = harness(&scratch);
    for (parent, name) in [(2, "Kicks"), (3, "Pads")] {
        h.shell
            .run(
                ids::ADD_COLLECTION,
                json!({ "parent": parent, "name": name }),
            )
            .unwrap();
    }
    // The item's menu: Orange is a submenu holding itself and Kicks.
    let nodes = look(&mut h);
    right_click(&mut h, centre_of(&nodes, Role::ListItem, "Pad.wav"));
    let nodes = look(&mut h);
    h.click(button(&nodes, "Orange"));
    let nodes = look(&mut h);
    h.click(button(&nodes, "Kicks"));
    let row = state(&mut h)
        .rows
        .into_iter()
        .find(|r| r.name == "Pad.wav")
        .unwrap();
    assert_eq!(row.nested, [8]);
    assert_eq!(row.collections, [2], "it shows Orange's colour");
    // Dragging an item onto Pads' label puts it in.
    let nodes = look(&mut h);
    drag(
        &mut h,
        centre_of(&nodes, Role::ListItem, "Snare.wav"),
        centre_of(&nodes, Role::ListItem, "Pads"),
    );
    let snare = state(&mut h)
        .rows
        .into_iter()
        .find(|r| r.name == "Snare.wav")
        .unwrap();
    assert_eq!(snare.nested, [9]);
    assert_eq!(snare.collections, [3]);
    // A nested label dragged onto another collection moves into it.
    let nodes = look(&mut h);
    drag(
        &mut h,
        centre_of(&nodes, Role::ListItem, "Kicks"),
        centre_of(&nodes, Role::ListItem, "Green"),
    );
    assert_eq!(nested_numbers(&mut h, 4), [8]);
    assert!(nested_numbers(&mut h, 2).is_empty());
}

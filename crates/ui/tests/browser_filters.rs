//! The browser's Filter View, its menu, the Results bar, saved labels, the
//! sidebar's Edit mode, the content pane's columns, Browse Back and
//! Forward, and Full-Height Browser, headless (WP-6.32). Spec: browser-tags
//! R18 to R33, browser R26.

mod common;

use std::path::{Path, PathBuf};

use bl_shell::Area;
use bl_shell::browser::places::create_user_library;
use bl_shell::browser_commands::{self as ids, BrowserState, Label};
use bl_ui::browser::{
    add_label_id, clear_filters_id, column_id, content_options_id, control_id, filter_group_id,
    filter_tag_id, filter_view_id, labels_edit_id, search_id,
};
use bl_ui::layout::{BrowserLayout, BrowserParts, DetailViews, REFERENCE, ShellLayout};
use bl_ui::main_window::UiEvent;
use common::{Harness, Window, command};
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use serde_json::json;

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-ui-filters-{}-{name}", std::process::id()));
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

/// A harness with three samples in the User Library, tagged Mine|A (Kick
/// and Pad), Mine|B (Snare) and Feel|Hard (Kick and Snare), the All label
/// chosen.
fn harness(scratch: &Scratch) -> Harness {
    let root = scratch.0.join("User Library");
    create_user_library(&root).unwrap();
    wav(&root.join("Kick.wav"), 100);
    wav(&root.join("Snare.wav"), 2_000);
    wav(&root.join("Pad.wav"), 40_000);
    let mut h = Harness::manual_with_title_bar(Window::Main);
    h.shell.set_user_library(Some(root.clone()));
    assert!(
        h.shell
            .wait_for_browser_index(std::time::Duration::from_secs(20))
    );
    let run = |h: &mut Harness, id: &str, args: serde_json::Value| {
        h.shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id}: {e}"));
    };
    run(&mut h, ids::ADD_TAG_GROUP, json!({ "name": "Mine" }));
    run(&mut h, ids::ADD_TAG_GROUP, json!({ "name": "Feel" }));
    for (name, group) in [("A", "Mine"), ("B", "Mine"), ("Hard", "Feel")] {
        run(
            &mut h,
            ids::ADD_TAG,
            json!({ "name": name, "group": group }),
        );
    }
    let item = |name: &str| root.join(name).to_string_lossy().into_owned();
    for (items, tag) in [
        (vec![item("Kick.wav"), item("Pad.wav")], "Mine|A"),
        (vec![item("Snare.wav")], "Mine|B"),
        (vec![item("Kick.wav"), item("Snare.wav")], "Feel|Hard"),
    ] {
        run(
            &mut h,
            ids::TAG_ITEMS,
            json!({ "items": items, "tags": [tag] }),
        );
    }
    h.shell.set_focused_area(Area::Browser);
    h.settle();
    h
}

fn names(h: &Harness) -> Vec<String> {
    let view = h.shell.browser_view();
    (0..view.row_count())
        .map(|i| view.row(i).unwrap().name)
        .collect()
}

/// The names of the audio files in the list, in order.
fn files(h: &Harness) -> Vec<String> {
    names(h)
        .into_iter()
        .filter(|name| name.ends_with(".wav"))
        .collect()
}

/// Runs a frame and returns its nodes.
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

fn click_with(h: &mut Harness, at: Pos2, modifiers: Modifiers) {
    let held = Event::ModifiersChanged(modifiers);
    h.frame(vec![held.clone(), Event::PointerMoved(at)]);
    h.frame(vec![
        held.clone(),
        Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: true,
            modifiers,
        },
    ]);
    h.frame(vec![
        held,
        Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: false,
            modifiers,
        },
    ]);
    h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    h.settle();
}

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

fn right_click(h: &mut Harness, at: Pos2) {
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
    h.settle();
}

fn invoked(h: &mut Harness, id: &str) -> bool {
    h.take_events()
        .iter()
        .any(|e| matches!(e, UiEvent::Invoke(i) if i.id == id))
}

fn state(h: &mut Harness) -> BrowserState {
    serde_json::from_value(h.shell.run(ids::STATE, json!({ "limit": 1000 })).unwrap()).unwrap()
}

// Spec: browser-tags R18, R20, R23, R33.
#[test]
fn the_filter_view_shows_groups_and_tag_buttons_and_clicks_select_them() {
    let scratch = Scratch::new("tags");
    let mut h = harness(&scratch);
    let nodes = look(&mut h);
    // Each group is a group with its name, each tag a toggle button inside
    // it naming the tag and whether it's selected.
    for (group, tag) in [
        ("Mine", "Mine|A"),
        ("Mine", "Mine|B"),
        ("Feel", "Feel|Hard"),
    ] {
        let group_node = nodes
            .iter()
            .find(|n| n.role == Role::Group && n.label == group)
            .unwrap_or_else(|| panic!("no group {group}"));
        let tag_node = nodes
            .iter()
            .find(|n| n.role == Role::Button && n.label == tag)
            .unwrap_or_else(|| panic!("no tag {tag}"));
        assert_eq!(tag_node.toggled, Some(false), "{tag} starts unselected");
        assert!(
            Harness::descends_from(&nodes, tag_node.id, group_node.id),
            "{tag} hangs from {group}"
        );
    }
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::Button && n.label == "Content|Sample"),
        "Beatlayer's own groups show their tags in use"
    );

    // A click selects a tag alone in its group.
    let a = centre_of(&nodes, Role::Button, "Mine|A");
    h.click(a);
    assert!(invoked(&mut h, ids::FILTER));
    assert_eq!(names(&h), ["Kick.wav", "Pad.wav"]);
    let nodes = look(&mut h);
    let selected = nodes
        .iter()
        .find(|n| n.role == Role::Button && n.label == "Mine|A")
        .unwrap();
    assert_eq!(selected.toggled, Some(true));
    // Cmd-click in the same group widens, a tag in another group narrows.
    click_with(&mut h, centre_of(&nodes, Role::Button, "Mine|B"), command());
    assert_eq!(names(&h), ["Kick.wav", "Pad.wav", "Snare.wav"]);
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::Button, "Feel|Hard"));
    assert_eq!(names(&h), ["Kick.wav", "Snare.wav"]);
    // Clicking the only selected tag of a group clears it.
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::Button, "Feel|Hard"));
    assert_eq!(names(&h), ["Kick.wav", "Pad.wav", "Snare.wav"]);
}

// Spec: browser-tags R23.
#[test]
fn the_filter_views_keys_move_between_tags_and_select_them() {
    let scratch = Scratch::new("keys");
    let mut h = harness(&scratch);
    let nodes = look(&mut h);
    // A click on the Filter View's background gives it the keyboard.
    let area = Harness::node(&nodes, "Filter View").bounds.unwrap();
    h.click(pos2(area.x1 as f32 - 3.0, area.y1 as f32 - 3.0));
    assert!(h.ctx.memory(|m| m.has_focus(filter_view_id())));
    // The arrows walk the tag buttons in order, Enter selects one alone.
    let order: Vec<String> = {
        let nodes = look(&mut h);
        let mut tags: Vec<(f32, f32, String)> = nodes
            .iter()
            .filter(|n| {
                n.role == Role::Button
                    && n.bounds.is_some_and(|b| b.y0 >= area.y0 && b.y1 <= area.y1)
                    && n.label.contains('|')
            })
            .map(|n| {
                let b = n.bounds.unwrap();
                (b.y0 as f32, b.x0 as f32, n.label.clone())
            })
            .collect();
        tags.sort_by(|a, b| a.partial_cmp(b).unwrap());
        tags.into_iter().map(|(_, _, label)| label).collect()
    };
    assert!(order.len() >= 4, "{order:?}");
    h.key(Key::ArrowRight, Modifiers::NONE);
    h.key(Key::Enter, Modifiers::NONE);
    assert_eq!(h.shell.browser_view().filter_tags(), [order[1].clone()]);
    // Cmd-Enter adds the next one to its group, or takes it out.
    h.key(Key::ArrowRight, Modifiers::NONE);
    h.key(Key::Enter, command());
    assert_eq!(h.shell.browser_view().filter_tags().len(), 2);
    h.key(Key::Enter, command());
    assert_eq!(h.shell.browser_view().filter_tags().len(), 1);
}

// Spec: browser-tags R21.
#[test]
fn the_filter_view_menu_lists_the_groups_and_its_items() {
    let scratch = Scratch::new("menu");
    let mut h = harness(&scratch);
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::Button, "Filter View Menu"));
    let nodes = look(&mut h);
    let item = |label: &str| {
        nodes
            .iter()
            .find(|n| {
                n.role == Role::Button && n.label.trim_start_matches('\u{2714}').trim() == label
            })
            .unwrap_or_else(|| panic!("no menu item {label:?}"))
    };
    // A group for each the chosen label can show, ticked while it shows.
    for group in ["Content", "Format", "Creator", "Devices", "Mine", "Feel"] {
        item(group);
    }
    assert!(item("Content").label.starts_with('\u{2714}'));
    item("Show All Filter Groups");
    item("Reset Filter Groups to Default");
    assert!(
        item("Include Auto Tags").disabled,
        "waits for automatic tagging"
    );
    item("Tag Editor");
    assert!(item("Show Quick Tags").label.starts_with('\u{2714}'));
    // Choosing a group hides it under the chosen label.
    let before = h.shell.browser_view().filter_view().groups.len();
    h.click(centre_of(&nodes, Role::Button, "\u{2714} Mine"));
    assert!(invoked(&mut h, ids::SET_FILTER_GROUPS));
    let after = h.shell.browser_view().filter_view();
    assert_eq!(after.groups.len(), before - 1);
    assert!(after.menu.iter().any(|g| g.name == "Mine" && !g.shown));
}

// Spec: browser-tags R21.
#[test]
fn a_groups_name_offers_hide_filter_group() {
    let scratch = Scratch::new("hide");
    let mut h = harness(&scratch);
    let nodes = look(&mut h);
    let group = Harness::node(&nodes, "Feel").bounds.unwrap();
    right_click(&mut h, pos2(group.x0 as f32 + 6.0, group.y0 as f32 + 6.0));
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::Button, "Hide Filter Group"));
    assert!(
        h.shell
            .browser_view()
            .filter_view()
            .menu
            .iter()
            .any(|g| g.name == "Feel" && !g.shown)
    );
}

// Spec: browser-tags R18, R30.
#[test]
fn the_toggle_the_keys_and_the_buttons_reach_their_commands() {
    let scratch = Scratch::new("toggle");
    let mut h = harness(&scratch);
    // The toggle at x 275 shows and hides the Filter View.
    let toggle = pos2(284.0, 71.0);
    h.click(toggle);
    assert!(!h.shell.browser_view().filter_view_shown());
    h.click(toggle);
    assert!(h.shell.browser_view().filter_view_shown());
    // Cmd-Option-G while the browser has the focus.
    h.shell.set_focused_area(Area::Browser);
    h.key(Key::G, command() | Modifiers::ALT);
    assert!(!h.shell.browser_view().filter_view_shown());
    h.key(Key::G, command() | Modifiers::ALT);
    assert!(h.shell.browser_view().filter_view_shown());

    // Browse Back and Forward are disabled in a new set.
    let nodes = look(&mut h);
    assert!(Harness::node(&nodes, "Browse Back").disabled);
    assert!(Harness::node(&nodes, "Browse Forward").disabled);
    h.shell
        .run(ids::OPEN_PLACE, json!({ "label": "samples" }))
        .unwrap();
    let nodes = look(&mut h);
    assert!(!Harness::node(&nodes, "Browse Back").disabled);
    h.click(centre_of(&nodes, Role::Button, "Browse Back"));
    assert!(invoked(&mut h, ids::BACK));
    assert_eq!(*h.shell.browser_view().label(), Label::All);
    let nodes = look(&mut h);
    assert!(Harness::node(&nodes, "Browse Back").disabled);
    h.click(centre_of(&nodes, Role::Button, "Browse Forward"));
    assert_eq!(*h.shell.browser_view().label(), Label::Samples);
    // Cmd-[ and Cmd-] while the browser has the focus.
    h.shell.set_focused_area(Area::Browser);
    h.key(Key::OpenBracket, command());
    assert_eq!(*h.shell.browser_view().label(), Label::All);
    h.key(Key::CloseBracket, command());
    assert_eq!(*h.shell.browser_view().label(), Label::Samples);
    // The mouse's back and forward buttons over the browser.
    let over = pos2(200.0, 400.0);
    let press = |button| {
        vec![Event::PointerButton {
            pos: over,
            button,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]
    };
    h.frame(vec![Event::PointerMoved(over)]);
    h.frame(press(PointerButton::Extra1));
    h.settle();
    assert_eq!(*h.shell.browser_view().label(), Label::All);
    h.frame(press(PointerButton::Extra2));
    h.settle();
    assert_eq!(*h.shell.browser_view().label(), Label::Samples);
}

// Spec: browser-tags R22.
#[test]
fn the_results_bar_clears_and_keeps_a_search() {
    let scratch = Scratch::new("results");
    let mut h = harness(&scratch);
    let nodes = look(&mut h);
    assert!(!nodes.iter().any(|n| n.label == "Results"), "no bar yet");
    h.click(centre_of(&nodes, Role::Button, "Mine|A"));
    let nodes = look(&mut h);
    let bar = Harness::node(&nodes, "Results");
    assert_eq!(bar.value, "1 filter");
    // The bar sits between the Filter View and the header row.
    let (filters, header) = (
        Harness::node(&nodes, "Filter View").bounds.unwrap(),
        Harness::node(&nodes, "Name").bounds.unwrap(),
    );
    let bar_bounds = bar.bounds.unwrap();
    assert!(filters.y1 <= bar_bounds.y0 && bar_bounds.y1 <= header.y0);
    // Add Label keeps what is shown as a label.
    h.click(centre_of(&nodes, Role::Button, "Add Label"));
    assert!(invoked(&mut h, ids::ADD_LABEL));
    assert_eq!(state(&mut h).saved_labels.len(), 1);
    // Clear clears the tags.
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::Button, "Clear"));
    assert!(invoked(&mut h, ids::CLEAR_FILTERS));
    assert!(h.shell.browser_view().filter_tags().is_empty());
    assert!(!look(&mut h).iter().any(|n| n.label == "Results"));
    // Esc in the search field clears the words and the tags.
    let nodes = look(&mut h);
    let tag = centre_of(&nodes, Role::Button, "Mine|A");
    h.click(tag);
    h.shell
        .run(ids::FOCUS_BROWSER, json!({ "part": "search" }))
        .unwrap();
    h.settle();
    h.frame(vec![Event::Text("pad".to_owned())]);
    h.settle();
    assert_eq!(h.shell.browser_view().search_text(), "pad");
    h.key(Key::Escape, Modifiers::NONE);
    h.settle();
    assert_eq!(h.shell.browser_view().search_text(), "");
    assert!(h.shell.browser_view().filter_tags().is_empty());
    let _ = (search_id(), add_label_id(), clear_filters_id());
}

// Spec: browser-tags R22.
#[test]
fn cmd_f_selects_the_search_text_and_clears_the_tags() {
    let scratch = Scratch::new("cmd-f");
    let mut h = harness(&scratch);
    h.shell
        .run(ids::SEARCH, json!({ "query": "kick" }))
        .unwrap();
    h.shell
        .run(ids::FILTER, json!({ "tags": ["Mine|A"] }))
        .unwrap();
    h.settle();
    h.key(Key::F, command());
    h.settle();
    assert!(h.shell.browser_view().filter_tags().is_empty());
    assert_eq!(h.shell.browser_view().search_text(), "kick");
    // Typing replaces the selected text.
    h.frame(vec![Event::Text("s".to_owned())]);
    h.settle();
    assert_eq!(h.shell.browser_view().search_text(), "s");
}

// Spec: browser-tags R24, R25.
#[test]
fn a_saved_label_takes_its_name_in_the_sidebar_and_has_a_menu() {
    let scratch = Scratch::new("saved");
    let mut h = harness(&scratch);
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::Button, "Mine|A"));
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::Button, "Add Label"));
    // Its name is open for editing.
    assert_eq!(
        state(&mut h).renaming_label,
        Some(Label::Saved(1)),
        "ready to edit"
    );
    h.settle();
    h.frame(vec![Event::Text(" favourites".to_owned())]);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    let after = state(&mut h);
    assert_eq!(after.renaming_label, None);
    let name = &after.saved_labels[0].name;
    assert!(name.ends_with("favourites"), "{name}");
    // Its context menu: Rename, Choose Icon, Remove.
    let nodes = look(&mut h);
    let label = centre_of(&nodes, Role::ListItem, name);
    right_click(&mut h, label);
    let nodes = look(&mut h);
    for item in ["Rename", "Choose Icon", "Remove"] {
        assert!(
            nodes
                .iter()
                .any(|n| n.role == Role::Button && n.label.starts_with(item)),
            "the menu has {item}: {:?}",
            nodes
                .iter()
                .filter(|n| n.role == Role::Button)
                .map(|n| &n.label)
                .collect::<Vec<_>>()
        );
    }
    h.click(centre_of(&nodes, Role::Button, "Remove"));
    assert!(state(&mut h).saved_labels.is_empty());
}

// Spec: browser-tags R26.
#[test]
fn the_library_sections_edit_mode_hides_labels_and_dragging_moves_them() {
    let scratch = Scratch::new("edit");
    let mut h = harness(&scratch);
    let nodes = look(&mut h);
    // Edit is at the right of the Library heading.
    let heading = Harness::node(&nodes, "Library").bounds.unwrap();
    h.frame(vec![Event::PointerMoved(pos2(
        heading.x1 as f32 - 8.0,
        (heading.y0 + heading.y1) as f32 / 2.0,
    ))]);
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::Button, "Edit"));
    let nodes = look(&mut h);
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::Button && n.label == "Done")
    );
    // A check box on each label hides or shows it.
    let tunings = nodes
        .iter()
        .find(|n| n.role == Role::CheckBox && n.label == "Tunings")
        .expect("a check box for Tunings");
    assert_eq!(tunings.toggled, Some(true));
    h.click(centre_of(&nodes, Role::CheckBox, "Tunings"));
    assert!(
        !state(&mut h)
            .labels
            .iter()
            .find(|l| l.label == Label::Tunings)
            .unwrap()
            .shown
    );
    // Dragging a label up moves it; All stays first.
    let nodes = look(&mut h);
    let samples = centre_of(&nodes, Role::ListItem, "Samples");
    let sounds = centre_of(&nodes, Role::ListItem, "Sounds");
    drag(&mut h, samples + vec2(20.0, 0.0), sounds + vec2(20.0, 0.0));
    let order: Vec<String> = state(&mut h)
        .labels
        .iter()
        .filter(|l| l.section == ids::Section::Library)
        .map(|l| l.name.clone())
        .collect();
    assert_eq!(&order[..3], ["All", "Samples", "Sounds"]);
    // Done ends the mode; a hidden label leaves the sidebar.
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::Button, "Done"));
    let nodes = look(&mut h);
    assert!(
        !nodes
            .iter()
            .any(|n| n.role == Role::ListItem && n.label == "Tunings")
    );
    assert!(
        !nodes
            .iter()
            .any(|n| n.role == Role::CheckBox && n.label == "Samples")
    );
    // Reset Label Order, in the heading's context menu.
    let heading = Harness::node(&nodes, "Library").bounds.unwrap();
    right_click(
        &mut h,
        pos2(heading.x0 as f32 + 8.0, heading.y0 as f32 + 8.0),
    );
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::Button, "Reset Label Order"));
    let order: Vec<String> = state(&mut h)
        .labels
        .iter()
        .filter(|l| l.section == ids::Section::Library)
        .map(|l| l.name.clone())
        .collect();
    assert_eq!(&order[..3], ["All", "Sounds", "Drums"]);
}

// Spec: browser-tags R25.
#[test]
fn cmd_r_renames_the_chosen_label_in_the_sidebar() {
    let scratch = Scratch::new("rename");
    let mut h = harness(&scratch);
    h.shell
        .run(ids::OPEN_PLACE, json!({ "label": "drums" }))
        .unwrap();
    h.shell
        .run(ids::FOCUS_BROWSER, json!({ "part": "sidebar" }))
        .unwrap();
    h.settle();
    h.key(Key::R, command());
    h.settle();
    assert_eq!(state(&mut h).renaming_label, Some(Label::Drums));
    h.frame(vec![Event::Text("Beats".to_owned())]);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    let after = state(&mut h);
    assert_eq!(
        after
            .labels
            .iter()
            .find(|l| l.label == Label::Drums)
            .unwrap()
            .name,
        "Beats"
    );
    assert_eq!(after.renaming_label, None);
}

// Spec: browser-tags R27, R28.
#[test]
fn the_content_options_menu_shows_columns_and_the_header_sorts_and_moves_them() {
    let scratch = Scratch::new("columns");
    let mut h = harness(&scratch);
    h.shell
        .run(ids::OPEN_PLACE, json!({ "label": "user_library" }))
        .unwrap();
    h.settle();
    // The menu's button at the header row's right.
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::Button, "Content Options"));
    let nodes = look(&mut h);
    for item in [
        "Date Modified",
        "Size",
        "Kind",
        "Length",
        "Sample Rate",
        "Channels",
        "Show File Extensions",
    ] {
        assert!(
            nodes
                .iter()
                .any(|n| n.label.trim_start_matches('\u{2714}').trim() == item
                    && n.role == Role::Button),
            "the menu has {item}"
        );
    }
    h.click(centre_of(&nodes, Role::Button, "Size"));
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::Button, "Content Options"));
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::Button, "Kind"));
    assert_eq!(
        h.shell.browser_view().columns().shown,
        [ids::Column::Size, ids::Column::Kind]
    );
    // The header row names the columns; the rows show their values.
    let nodes = look(&mut h);
    let size = centre_of(&nodes, Role::ColumnHeader, "Size");
    let kind = centre_of(&nodes, Role::ColumnHeader, "Kind");
    assert!(size.x < kind.x);
    // A click on a column's name sorts by it, again reverses.
    h.click(size);
    assert_eq!(files(&h), ["Kick.wav", "Snare.wav", "Pad.wav"]);
    assert_eq!(names(&h)[0], "Clips", "folders stay first");
    let nodes = look(&mut h);
    let header = Harness::node(&nodes, "Size");
    assert_eq!(header.role, Role::ColumnHeader);
    h.click(centre_of(&nodes, Role::ColumnHeader, "Size"));
    assert_eq!(files(&h), ["Pad.wav", "Snare.wav", "Kick.wav"]);
    // Dragging a column's name moves the column.
    let nodes = look(&mut h);
    let size = centre_of(&nodes, Role::ColumnHeader, "Size");
    let kind = centre_of(&nodes, Role::ColumnHeader, "Kind");
    drag(&mut h, size, kind + vec2(20.0, 0.0));
    assert_eq!(
        h.shell.browser_view().columns().shown,
        [ids::Column::Kind, ids::Column::Size]
    );
    // Right-clicking a column's name opens the same menu.
    let nodes = look(&mut h);
    right_click(&mut h, centre_of(&nodes, Role::ColumnHeader, "Kind"));
    let nodes = look(&mut h);
    assert!(
        nodes
            .iter()
            .any(|n| n.label.ends_with("Show File Extensions"))
    );
    // Show File Extensions off: names lose theirs.
    h.click(centre_of(
        &nodes,
        Role::Button,
        "\u{2714} Show File Extensions",
    ));
    assert!(!h.shell.browser_view().columns().file_extensions);
    let _ = (column_id(ids::Column::Size), content_options_id());
}

// Spec: browser R26.
#[test]
fn the_full_height_browser_runs_down_to_the_status_bar() {
    // The layout at 1600 x 1000 points.
    let normal =
        ShellLayout::compute_with_detail(REFERENCE, true, true, None, None, DetailViews::default());
    let full = ShellLayout::compute_with_detail(
        REFERENCE,
        true,
        true,
        None,
        None,
        DetailViews::default().with_full_height_browser(true),
    );
    let browser = full.browser.unwrap();
    assert_eq!(browser.top(), normal.browser.unwrap().top());
    assert_eq!(browser.right(), 311.0);
    // The column ends where the main view would if no detail views were
    // open: just above the Status Bar.
    assert!(browser.bottom() > normal.browser.unwrap().bottom());
    assert!(browser.bottom() <= full.status_bar.top());
    assert!(full.status_bar.top() - browser.bottom() < 8.0);
    // The detail area and the Info View sit beside it, under the main view.
    assert_eq!(full.detail.left(), browser.right() + 9.0);
    assert_eq!(full.info_view.unwrap().left(), full.detail.left());
    assert_eq!(full.main_view.left(), browser.right() + 9.0);
    assert_eq!(full.main_view.bottom(), normal.main_view.bottom());
    assert_eq!(full.detail.bottom(), normal.detail.bottom());
    // Hiding the browser gives the detail area its place back.
    let hidden = ShellLayout::compute_with_detail(
        REFERENCE,
        false,
        true,
        None,
        None,
        DetailViews::default().with_full_height_browser(true),
    );
    assert_eq!(hidden.detail.left(), 9.0);
}

// Spec: browser R26.
#[test]
fn the_full_height_browser_switch_moves_the_drawn_column() {
    let scratch = Scratch::new("full-height");
    let mut h = harness(&scratch);
    let bounds = |h: &mut Harness, label: &str| {
        let nodes = look(h);
        let b = Harness::node(&nodes, label).bounds.unwrap();
        (b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32)
    };
    let before = bounds(&mut h, "Browser");
    assert_eq!(before.3, 766.0, "to the detail area's top");
    // The Browser Config Menu beside the browser's toggle has the item.
    let nodes = look(&mut h);
    let menu = nodes
        .iter()
        .find(|n| n.label == "Browser Config Menu")
        .expect("the Browser Config Menu");
    let b = menu.bounds.unwrap();
    h.click(pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::Button, "Full-Height Browser"));
    assert!(h.shell.browser_view().full_height());
    let after = bounds(&mut h, "Browser");
    let status = bounds(&mut h, "Status Bar");
    assert!(
        after.3 > 766.0 && after.3 <= status.1,
        "{after:?} {status:?}"
    );
    // The item is ticked while it's on, and again puts the column back.
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::Button, "Browser Config Menu"));
    let nodes = look(&mut h);
    assert!(
        nodes
            .iter()
            .any(|n| n.label.starts_with('\u{2714}') && n.label.contains("Full-Height Browser"))
    );
    h.click(centre_of(
        &nodes,
        Role::Button,
        "\u{2714} Full-Height Browser",
    ));
    assert_eq!(bounds(&mut h, "Browser").3, 766.0);
    // The View menu's item runs the same command.
    h.shell
        .run("view.toggle_full_height_browser", json!({}))
        .unwrap();
    assert!(h.shell.browser_view().full_height());
}

// Spec: browser-tags R18, R23.
#[test]
fn tab_walks_the_search_field_the_filter_toggle_and_menu_then_the_sidebar_and_the_filter_view() {
    let scratch = Scratch::new("tab");
    let mut h = harness(&scratch);
    h.shell
        .run(ids::FOCUS_BROWSER, json!({ "part": "search" }))
        .unwrap();
    h.settle();
    assert!(h.ctx.memory(|m| m.has_focus(search_id())));
    let walk = [
        control_id("Show Filter View"),
        control_id("Filter View Menu"),
        bl_ui::browser::sidebar_id(),
        filter_view_id(),
    ];
    for expected in walk {
        h.shell.run("focus.next", json!({})).unwrap();
        // The move is made on the frames after the command, and a part of
        // the browser takes the focus through `focus.browser`.
        h.settle();
        h.settle();
        assert!(
            h.ctx.memory(|m| m.has_focus(expected)),
            "Tab reaches {expected:?}"
        );
    }
    let _ = (
        filter_group_id("Mine"),
        filter_tag_id("Mine|A"),
        labels_edit_id(),
    );
    // Hidden, Tab goes from the sidebar to the content pane: its header
    // row's Content Options button, then its list.
    h.shell
        .run(ids::SET_FILTER_VIEW, json!({ "show": false }))
        .unwrap();
    h.settle();
    h.shell
        .run(ids::FOCUS_BROWSER, json!({ "part": "sidebar" }))
        .unwrap();
    h.settle();
    for expected in [content_options_id(), bl_ui::browser::list_id()] {
        h.shell.run("focus.next", json!({})).unwrap();
        h.settle();
        h.settle();
        assert!(h.ctx.memory(|m| m.has_focus(expected)), "{expected:?}");
    }
}

// Spec: browser-tags R18.
#[test]
fn the_filter_views_layout_rows_follow_layout_md() {
    let column = Rect::from_min_max(pos2(9.0, 61.0), pos2(311.0, 766.0));
    let hidden = BrowserLayout::compute_parts(column, BrowserParts::default());
    assert_eq!((hidden.header.top(), hidden.list.top()), (84.0, 103.0));
    assert_eq!(hidden.filter_view.height(), 0.0);
    let shown = BrowserLayout::compute_parts(
        column,
        BrowserParts {
            filter_view: true,
            ..BrowserParts::default()
        },
    );
    // L-BRW-007, L-BRW-009, L-BRW-010.
    assert_eq!(
        (shown.filter_view.top(), shown.filter_view.bottom()),
        (84.0, 204.0)
    );
    assert_eq!((shown.header.top(), shown.header.bottom()), (208.0, 227.0));
    assert_eq!(shown.list.top(), 227.0);
    // The Results bar sits between the Filter View and the header row.
    let with_bar = BrowserLayout::compute_parts(
        column,
        BrowserParts {
            filter_view: true,
            results_bar: true,
            ..BrowserParts::default()
        },
    );
    assert!(with_bar.results_bar.top() >= with_bar.filter_view.bottom());
    assert!(with_bar.results_bar.bottom() <= with_bar.header.top());
    assert!(with_bar.header.top() > shown.header.top());
}

// Spec: browser-tags R8.
#[test]
fn deleting_a_tag_a_saved_label_uses_asks_first_and_names_the_label() {
    let scratch = Scratch::new("delete-asks");
    let mut h = harness(&scratch);
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::Button, "Mine|A"));
    h.shell.run(ids::ADD_LABEL, json!({})).unwrap();
    h.shell
        .run(
            ids::RENAME_LABEL,
            json!({ "label": { "saved": 1 }, "name": "Hearts" }),
        )
        .unwrap();
    h.shell
        .run(ids::SHOW_TAG_EDITOR, json!({ "show": true }))
        .unwrap();
    h.settle();
    let nodes = look(&mut h);
    right_click(&mut h, centre_of(&nodes, Role::CheckBox, "Mine|A"));
    let nodes = look(&mut h);
    h.click(centre_of(&nodes, Role::Button, "Delete"));
    // Nothing is deleted until the person answers.
    assert!(
        h.shell
            .browser_view()
            .tag_names()
            .contains(&"Mine|A".to_owned())
    );
    let nodes = look(&mut h);
    let ask = nodes
        .iter()
        .find(|n| n.label.contains("use it") || n.value.contains("use it"))
        .expect("the question");
    let text = format!("{}{}", ask.label, ask.value);
    assert!(text.contains("Hearts"), "names the label: {text}");
    h.click(centre_of(&nodes, Role::Button, "Delete"));
    assert!(
        !h.shell
            .browser_view()
            .tag_names()
            .contains(&"Mine|A".to_owned())
    );
    assert!(state(&mut h).saved_labels[0].tags.is_empty());
}

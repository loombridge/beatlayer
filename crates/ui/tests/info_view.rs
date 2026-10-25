//! The Info View (WP-2.24), headless: it shows at Live's place in the
//! detail area, reads whatever the pointer is over — the control's name,
//! the registered description of the command it runs, and its key on a
//! separate line — shows nothing while the pointer is over empty space,
//! and hides and shows from its Status Bar toggle and its key.
//!
//! Spec: info-view R1 to R5.

use crate::common;
use bl_shell::commands as ids;
use bl_shell::transport_commands;
use bl_ui::layout::{Element, REFERENCE, ShellLayout};
use common::{Harness, Window};
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, Pos2};
use serde_json::json;

/// Moves the pointer to `at` and lets the frame settle on it.
fn hover(h: &mut Harness, at: Pos2) {
    h.frame(vec![Event::PointerMoved(at)]);
    h.settle();
}

#[test]
fn a_new_set_shows_the_info_view_where_live_puts_it() {
    // Spec: info-view R1, R3. L-INFO-001: the detail area's left, 231
    // points wide; L-WIN-006's right half stays the Device View's.
    let mut h = Harness::new(Window::Main);
    let output = h.settle();
    assert!(h.shell.info_view_visible(), "a new set opens with it shown");
    let layout = h.main.drawn_layout().expect("the last frame's layout");
    let info = layout.info_view.expect("the Info View shows");
    assert_eq!(info.min, layout.detail.min);
    assert_eq!(info.right(), 240.0);
    assert_eq!(info.bottom(), layout.detail.bottom());
    assert_eq!(layout.device_view.unwrap().left(), 249.0);
    let nodes = Harness::nodes(&output);
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::Group && n.label == "Info View"),
        "the region is in the tree"
    );
}

#[test]
fn hovering_a_control_reads_the_command_registered_for_it() {
    // Spec: info-view R4, D17: the text comes from the command's registered
    // description.
    let mut h = Harness::new(Window::Main);
    h.settle();
    let play = ShellLayout::compute(REFERENCE, true, true).rect(Element::Play);
    hover(&mut h, play.center());
    let description = h
        .shell
        .registry()
        .get(transport_commands::PLAY)
        .expect("transport.play is registered")
        .description
        .to_string();
    let content = h.main.info_view_content();
    assert_eq!(content.title, "Play");
    assert_eq!(content.body, description);
    assert!(!content.body.is_empty(), "the description is written");
    assert_eq!(content.shortcut, None, "Play has no recorded key");
    // Spec: info-view R5. Keyboard focus moves; the pointer stays, and the
    // Info View keeps reading the pointer.
    h.shell
        .run(bl_shell::commands::FOCUS_TOGGLE_TAB_NAVIGATION, json!({}))
        .unwrap();
    h.key(Key::Tab, Modifiers::NONE);
    h.settle();
    let content = h.main.info_view_content();
    assert_eq!(content.title, "Play", "keyboard focus doesn't move it");
}

#[test]
fn the_control_under_the_pointer_is_the_toggle_itself() {
    // Spec: info-view R4. A Status Bar element is read too, and its key
    // shows on its own line (L-INFO-002).
    let mut h = Harness::new(Window::Main);
    h.settle();
    let toggle = ShellLayout::compute(REFERENCE, true, true).rect(Element::InfoToggle);
    hover(&mut h, toggle.center());
    let expected_key = {
        let keymap = bl_ui::keymap::Keymap::live12();
        let platform = bl_ui::keymap::Platform::current();
        keymap
            .chord_for_command(ids::VIEW_TOGGLE_INFO_VIEW, platform)
            .map(|chord| bl_ui::keymap::chord_text(chord, platform))
    };
    assert!(expected_key.is_some(), "S-VIEW-008 is bound in the keymap");
    let content = h.main.info_view_content();
    assert_eq!(content.title, "Show/Hide Info View");
    assert_eq!(
        content.body,
        h.shell
            .registry()
            .get(ids::VIEW_TOGGLE_INFO_VIEW)
            .unwrap()
            .description
            .to_string()
    );
    assert_eq!(content.shortcut, expected_key);
}

#[test]
fn hovering_empty_space_clears_it() {
    // Spec: info-view R4. Nothing under the pointer: nothing shown.
    let mut h = Harness::new(Window::Main);
    h.settle();
    let play = ShellLayout::compute(REFERENCE, true, true).rect(Element::Play);
    hover(&mut h, play.center());
    assert!(!h.main.info_view_content().title.is_empty());
    // The Control Bar's background, between its controls: no element.
    hover(&mut h, Pos2::new(400.0, 30.0));
    let content = h.main.info_view_content();
    assert_eq!(content.title, "", "cleared: nothing under the pointer");
    assert_eq!(content.body, "");
    assert_eq!(content.shortcut, None);
}

#[test]
fn the_status_bar_toggle_hides_it_and_the_device_view_widens() {
    // Spec: info-view R2. The Device View takes the Info View's place, as
    // the main view does when the browser hides.
    let mut h = Harness::new(Window::Main);
    h.settle();
    let toggle = ShellLayout::compute(REFERENCE, true, true).rect(Element::InfoToggle);
    h.click(toggle.center());
    assert!(!h.shell.info_view_visible());
    let nodes = Harness::nodes(&h.settle());
    assert!(
        !nodes
            .iter()
            .any(|n| n.role == Role::Group && n.label == "Info View"),
        "the region is gone"
    );
    let layout = h.main.drawn_layout().expect("the last frame's layout");
    assert!(layout.info_view.is_none());
    assert_eq!(
        layout.device_view.unwrap().left(),
        layout.detail.left(),
        "the Device View takes its place"
    );
    assert_eq!(
        h.main.info_view_content().title,
        "",
        "hidden: it shows nothing"
    );
    // Back again.
    h.click(toggle.center());
    assert!(h.shell.info_view_visible());
    let nodes = Harness::nodes(&h.settle());
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::Group && n.label == "Info View")
    );
}

#[test]
fn the_key_hides_and_shows_it() {
    // Spec: info-view R5. S-VIEW-008: Shift+Slash.
    let mut h = Harness::new(Window::Main);
    h.settle();
    h.key(Key::Slash, Modifiers::SHIFT);
    assert!(!h.shell.info_view_visible());
    h.key(Key::Slash, Modifiers::SHIFT);
    assert!(h.shell.info_view_visible());
}

// WP-6.58: the user's own info text. Spec: info-view R10, R11.

use bl_model::TrackId;
use bl_shell::menus::{Menu, MenuPlatform, menu_bar};
use bl_ui::arrangement::headers::title_id;
use bl_ui::menus::item_state;
use egui::PointerButton;

/// The first track, selected by its title bar, with the Arrangement
/// focused.
fn selected_track(h: &mut Harness) -> TrackId {
    let track = h.shell.editor().document().top_level().next().unwrap();
    h.shell
        .invoke(
            "arrangement.select_tracks",
            json!({ "tracks": [track], "title_bars": true }),
        )
        .unwrap();
    h.shell.set_focused_area(bl_shell::Area::MainView);
    h.settle();
    track
}

/// Where a node is, from the last frame's tree.
fn centre_of(h: &mut Harness, id: egui::Id) -> Pos2 {
    let nodes = Harness::nodes(&h.settle());
    let b = nodes
        .iter()
        .find(|n| n.id == id.accesskit_id())
        .and_then(|n| n.bounds)
        .expect("the node is drawn");
    Pos2::new(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
}

/// Runs Edit ▸ Edit Info Text as the menu does.
fn edit_info_text_from_the_edit_menu(h: &mut Harness) {
    let menus = menu_bar(MenuPlatform::current());
    let item = menus
        .iter()
        .flat_map(Menu::walk)
        .find(|item| item.row == "M-EDIT-025")
        .unwrap();
    let state = item_state(
        item,
        &h.shell,
        &h.keymap,
        bl_ui::keymap::Platform::current(),
    );
    assert!(
        state.enabled,
        "Edit Info Text is enabled with a track selected"
    );
    let invocation = state.invocation.expect("the item runs a command");
    h.shell.invoke(&invocation.id, invocation.args).unwrap();
    h.settle();
}

/// The info text field, if the Info View shows one.
fn field(h: &mut Harness) -> Option<egui::accesskit::NodeId> {
    let nodes = Harness::nodes(&h.settle());
    nodes
        .iter()
        .find(|n| n.role == Role::MultilineTextInput && n.label == "Info text")
        .map(|n| n.id)
}

fn type_text(h: &mut Harness, text: &str) {
    h.frame(vec![Event::Text(text.to_owned())]);
    h.settle();
}

fn track_text(h: &Harness, track: TrackId) -> String {
    h.shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .info_text()
        .to_owned()
}

#[test]
fn edit_info_text_in_the_edit_menu_opens_the_field_with_the_focus_and_return_keeps_it() {
    // Spec: info-view R10.
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let track = selected_track(&mut h);
    // The Info View is shown first if it's hidden.
    h.shell.invoke("view.toggle_info_view", json!({})).unwrap();
    h.settle();
    assert!(!h.shell.info_view_visible());
    edit_info_text_from_the_edit_menu(&mut h);
    assert!(h.shell.info_view_visible(), "shown for the field");
    let node = field(&mut h).expect("the Info View is a text field");
    assert_eq!(
        h.ctx.memory(|m| m.focused()).map(|id| id.accesskit_id()),
        Some(node),
        "with the focus"
    );
    type_text(&mut h, "Played live");
    assert_eq!(track_text(&h, track), "", "nothing kept before Return");
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert_eq!(track_text(&h, track), "Played live");
    assert_eq!(
        h.shell.editor().undo_history().next().unwrap(),
        "Change Info Text"
    );
    assert!(field(&mut h).is_none(), "the field closes");
}

#[test]
fn option_return_adds_a_line_and_escape_drops_the_change() {
    // Spec: info-view R10.
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let track = selected_track(&mut h);
    edit_info_text_from_the_edit_menu(&mut h);
    type_text(&mut h, "first");
    h.key(Key::Enter, Modifiers::ALT);
    h.settle();
    type_text(&mut h, "second");
    assert!(
        field(&mut h).is_some(),
        "Option-Return keeps the field open"
    );
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert_eq!(track_text(&h, track), "first\nsecond");
    // Esc leaves the text as it was.
    edit_info_text_from_the_edit_menu(&mut h);
    type_text(&mut h, " and more");
    h.key(Key::Escape, Modifiers::NONE);
    h.settle();
    assert_eq!(track_text(&h, track), "first\nsecond");
    assert!(field(&mut h).is_none());
}

#[test]
fn the_focus_leaving_keeps_the_text() {
    // Spec: info-view R10.
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let track = selected_track(&mut h);
    edit_info_text_from_the_edit_menu(&mut h);
    type_text(&mut h, "kept");
    // A click on empty space in the Control Bar moves the focus away.
    h.click(Pos2::new(400.0, 50.0));
    h.settle();
    assert_eq!(track_text(&h, track), "kept");
    assert!(field(&mut h).is_none());
}

#[test]
fn a_tracks_context_menu_edit_info_text_opens_the_field() {
    // Spec: info-view R10 (C-TRACK-006).
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let track = h.shell.editor().document().top_level().next().unwrap();
    let at = centre_of(&mut h, title_id(track));
    h.frame(vec![
        Event::PointerMoved(at),
        Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Secondary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    let nodes = Harness::nodes(&h.settle());
    let item = nodes
        .iter()
        .find(|n| n.label == "Edit Info Text")
        .expect("the track's menu lists Edit Info Text");
    assert!(!item.disabled, "it works now");
    let b = item.bounds.unwrap();
    h.click(Pos2::new(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    h.settle();
    assert!(field(&mut h).is_some());
    type_text(&mut h, "Bass");
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert_eq!(track_text(&h, track), "Bass");
}

#[test]
fn hovering_an_item_with_info_text_shows_its_name_and_text_and_assistive_technology_reads_it() {
    // Spec: info-view R11.
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let track = h.shell.editor().document().top_level().next().unwrap();
    let name = h
        .shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .name()
        .to_owned();
    let before = centre_of(&mut h, title_id(track));
    h.frame(vec![Event::PointerMoved(before)]);
    h.settle();
    let plain = h.main.info_view_content().clone();
    h.shell
        .invoke(
            "track.set_info_text",
            json!({ "track": track, "info_text": "Plays the bass line" }),
        )
        .unwrap();
    h.frame(vec![Event::PointerMoved(Pos2::new(5.0, 5.0))]);
    h.settle();
    let at = centre_of(&mut h, title_id(track));
    hover(&mut h, at);
    let content = h.main.info_view_content();
    assert_eq!(content.title, name);
    assert_eq!(content.body, "Plays the bass line");
    assert_ne!(*content, plain);
    // The AccessKit tree gives the text as the track's description.
    let nodes = Harness::nodes(&h.settle());
    let node = nodes
        .iter()
        .find(|n| n.id == title_id(track).accesskit_id())
        .unwrap();
    assert_eq!(node.description, "Plays the bass line");
    // The pointer leaving clears it (R4).
    let info = h.main.drawn_layout().unwrap().info_view.unwrap();
    hover(&mut h, info.center());
    assert_eq!(h.main.info_view_content().title, "");
}

#[test]
fn edit_info_text_in_the_edit_menu_acts_on_a_selected_scene() {
    // Spec: info-view R10 (M-EDIT-025; C-SCENE-006 waits on the Session
    // View's menus).
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    h.shell
        .invoke("view.toggle_session_arrangement", json!({}))
        .unwrap();
    let scene = h.shell.editor().document().scenes().next().unwrap().id;
    h.shell
        .invoke("session.select", json!({ "scene": scene }))
        .unwrap();
    h.shell.set_focused_area(bl_shell::Area::MainView);
    h.settle();
    edit_info_text_from_the_edit_menu(&mut h);
    assert!(field(&mut h).is_some());
    type_text(&mut h, "Chorus");
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    let doc = h.shell.editor().document();
    assert_eq!(doc.scene(scene).unwrap().info_text, "Chorus");
}

#[test]
fn hiding_the_info_view_closes_the_field_so_the_keys_are_free() {
    // Spec: info-view R2, R10.
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    selected_track(&mut h);
    edit_info_text_from_the_edit_menu(&mut h);
    assert!(h.main.is_typing());
    h.shell.invoke("view.toggle_info_view", json!({})).unwrap();
    h.settle();
    assert!(!h.main.is_typing(), "no field nobody sees keeps the keys");
}

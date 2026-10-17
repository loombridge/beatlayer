//! The track header's context menu (WP-4.22; menus.md C-TRACK), headless:
//! a right-click on a track's title bar in the Arrangement's headers, the
//! Session View and the mixers lists Live's items; the ones whose commands
//! exist run them on the selected tracks, and the rest are drawn disabled,
//! each naming its package. The keys (S-CTX-002) open it for the selected
//! title bar.
//!
//! Spec: track-context-menu R1 to R7; accessibility R8.

mod common;

use bl_model::{TrackId, TrackKind};
use bl_shell::mixer_view::MixerOf;
use bl_shell::mixer_view_commands::TOGGLE;
use bl_ui::layout::mixer::{MixerLayout, Sections, SessionLayout};
use bl_ui::layout::{REFERENCE, ShellLayout, TITLE_BAR};
use bl_ui::main_window::{UiEvent, main_view_id};
use bl_ui::mixer::column_id;
use common::{Harness, Node, Window};
use egui::accesskit::Role;
use egui::{Event, FullOutput, Key, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use serde_json::json;

fn arrangement() -> Harness {
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    h
}

fn session() -> Harness {
    let mut h = arrangement();
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    h.settle();
    h
}

fn tracks(h: &Harness) -> Vec<TrackId> {
    h.shell.editor().document().top_level().collect()
}

fn kind_of(h: &Harness, track: TrackId) -> TrackKind {
    h.shell.editor().document().track(track).unwrap().kind()
}

fn name_of(h: &Harness, track: TrackId) -> String {
    h.shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .name()
        .to_owned()
}

fn press(at: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos: at,
        button: PointerButton::Secondary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

fn right_click(h: &mut Harness, at: Pos2) -> FullOutput {
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![press(at, true)]);
    h.frame(vec![press(at, false)]);
    h.settle()
}

/// Where a track's title bar is in the Arrangement's headers.
fn header_title(h: &mut Harness, name: &str) -> Pos2 {
    let nodes = Harness::nodes(&h.settle());
    let b = nodes
        .iter()
        .find(|n| n.role == Role::RowHeader && n.label == name)
        .unwrap_or_else(|| panic!("no title {name:?}"))
        .bounds
        .unwrap();
    pos2(b.x0 as f32 + 60.0, b.y0 as f32 + 7.0)
}

/// Where a track's title bar is in a row of mixer title bars.
fn title_in(h: &Harness, row: Rect, track: TrackId) -> Pos2 {
    let columns = h.shell.mixer_columns();
    let returns = h.shell.editor().document().returns().count();
    let layout = MixerLayout::compute(row, columns.len(), returns, Sections::DEFAULT, 0.0);
    let at = columns.iter().position(|t| *t == track).unwrap();
    pos2(layout.columns[at].rect.center().x, row.center().y)
}

/// Where a track's title bar is in the Session View.
fn session_title(h: &Harness, track: TrackId) -> Pos2 {
    let view = ShellLayout::compute(REFERENCE, true, true)
        .translate(vec2(0.0, TITLE_BAR))
        .main_view;
    let session = SessionLayout::compute(view, Some(Sections::DEFAULT));
    title_in(h, session.title_bars, track)
}

/// The menu item `label`, which reads with the keys it shows after it
/// ("Delete Delete", "Cut ⌘X").
fn item(nodes: &[Node], label: &str) -> Node {
    nodes
        .iter()
        .find(|n| {
            n.role == Role::Button
                && (n.label == label
                    || n.label
                        .strip_prefix(label)
                        .and_then(|keys| keys.strip_prefix(' '))
                        .is_some_and(|keys| !keys.contains(' ')))
        })
        .or_else(|| nodes.iter().find(|n| n.label == label))
        .unwrap_or_else(|| panic!("no menu item {label:?}"))
        .clone()
}

fn choose(h: &mut Harness, label: &str) {
    let nodes = Harness::nodes(&h.settle());
    let at = item(&nodes, label).bounds.unwrap();
    h.click(pos2(
        ((at.x0 + at.x1) / 2.0) as f32,
        ((at.y0 + at.y1) / 2.0) as f32,
    ));
    h.settle();
}

fn invoked(h: &mut Harness, id: &str) -> bool {
    h.take_events()
        .iter()
        .any(|e| matches!(e, UiEvent::Invoke(i) if i.id == id))
}

fn last_step(h: &Harness) -> String {
    h.shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

// Spec: track-context-menu R1, R6 (C-TRACK-001 to C-TRACK-016)
#[test]
fn the_menu_lists_lives_items_and_names_later_packages() {
    let mut h = arrangement();
    let at = header_title(&mut h, "3 Audio");
    let nodes = Harness::nodes(&right_click(&mut h, at));
    // The title bar the menu opened on is selected.
    let audio = tracks(&h)[2];
    assert_eq!(h.shell.arrangement().selected_tracks(), [audio]);
    assert!(h.shell.arrangement().title_bars_selected());
    assert!(!item(&nodes, "Duplicate Track Without Clips").disabled);
    for label in [
        "Delete",
        "Rename",
        "Insert Audio Track",
        "Insert MIDI Track",
        "Insert Return Track",
        "Group Tracks",
    ] {
        let node = item(&nodes, label);
        assert!(!node.disabled, "{label} works");
        // It shows its key after the name.
        assert!(node.label.len() > label.len(), "{label} shows a key");
    }
    // No clip or time selection: the edit items have nothing to act on.
    for label in ["Cut", "Copy", "Duplicate"] {
        assert!(
            item(&nodes, label).disabled,
            "{label} waits for a selection"
        );
    }
    for (label, package) in [
        ("Edit Info Text", "WP-2.24"),
        ("Freeze Track", "WP-4.14"),
        ("Bounce Track in Place", "WP-4.15"),
        ("Select Track Content", "not planned"),
        ("Save as Default MIDI Track", "not planned"),
        ("Assign Track Color to Clips", "track colours"),
        ("Colour palette", "track colours"),
        ("Show Take Lanes", "Phase 6"),
        ("Insert Take Lane", "Phase 6"),
        ("Delete All Take Lanes", "Phase 6"),
    ] {
        let node = item(&nodes, label);
        assert!(node.disabled, "{label} waits");
        assert!(
            node.description.contains(package),
            "{label}: {}",
            node.description
        );
    }
    // The other states' items are absent from a single audio track.
    for label in ["Ungroup Tracks", "Link Tracks", "Crop Clip"] {
        assert!(!nodes.iter().any(|n| n.label.starts_with(label)), "{label}");
    }
}

// Spec: track-context-menu R3 (the keys the items show)
#[test]
fn working_items_show_their_keys_and_run_their_commands() {
    let mut h = arrangement();
    let before = tracks(&h).len();
    let at = header_title(&mut h, "3 Audio");
    right_click(&mut h, at);
    h.take_events();
    choose(&mut h, "Insert MIDI Track");
    assert!(invoked(&mut h, "track.create"));
    let after = tracks(&h);
    assert_eq!(after.len(), before + 1);
    // Directly after the selected track, selected itself.
    assert_eq!(kind_of(&h, after[3]), TrackKind::Midi);
    assert_eq!(h.shell.arrangement().selected_tracks(), [after[3]]);

    let at = header_title(&mut h, "3 Audio");
    right_click(&mut h, at);
    h.take_events();
    choose(&mut h, "Duplicate Track Without Clips");
    assert!(invoked(&mut h, "track.duplicate_without_clips"));
    assert_eq!(tracks(&h).len(), before + 2);
    assert_eq!(last_step(&h), "Duplicate Track(s) Without Clips");
}

// Spec: track-context-menu R4 (the Delete key on a selected title bar)
#[test]
fn the_delete_key_deletes_the_selected_title_bars_tracks() {
    let mut h = arrangement();
    let audio = tracks(&h)[2];
    let at = header_title(&mut h, "3 Audio");
    h.click(at);
    h.settle();
    h.key(Key::Delete, Modifiers::NONE);
    h.settle();
    assert!(h.shell.editor().document().track(audio).is_none());
    assert_eq!(last_step(&h), "Delete Track(s)");
}

// Spec: track-context-menu R4, R5 (Delete, Rename)
#[test]
fn delete_and_rename_act_on_the_selected_track() {
    let mut h = arrangement();
    let audio = tracks(&h)[2];
    let at = header_title(&mut h, "3 Audio");
    right_click(&mut h, at);
    choose(&mut h, "Rename");
    // The name is typed in the title bar; Return renames the track.
    assert!(h.main.is_typing());
    for c in "Bass".chars() {
        h.frame(vec![Event::Text(c.to_string())]);
    }
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert!(!h.main.is_typing());
    assert!(
        name_of(&h, audio).ends_with("Bass"),
        "{}",
        name_of(&h, audio)
    );

    let name = name_of(&h, audio);
    let at = header_title(&mut h, &name);
    right_click(&mut h, at);
    h.take_events();
    choose(&mut h, "Delete");
    assert!(invoked(&mut h, "track.delete"));
    assert!(h.shell.editor().document().track(audio).is_none());
    assert_eq!(last_step(&h), "Delete Track(s)");
}

// Spec: track-context-menu R5 (a title bar that isn't drawn)
#[test]
fn renaming_a_track_out_of_view_leaves_the_keys_to_shortcuts() {
    let mut h = arrangement();
    let all = tracks(&h);
    h.shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [all[2], all[3]], "title_bars": true }),
        )
        .unwrap();
    h.shell.run("track.group", json!({})).unwrap();
    h.shell
        .run("arrangement.fold_groups", json!({ "fold": true }))
        .unwrap();
    h.shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [all[2]], "title_bars": true }),
        )
        .unwrap();
    h.settle();
    // Cmd-R on a track hidden in its folded group: there's no title bar to
    // type in, so the keys stay with the shortcuts.
    h.shell.invoke("track.rename", json!({})).unwrap();
    h.settle();
    assert!(!h.main.is_typing());
}

// Spec: track-context-menu R2 (several tracks)
#[test]
fn with_several_tracks_selected_each_item_acts_on_all_of_them() {
    let mut h = arrangement();
    let all = tracks(&h);
    h.shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [all[2], all[3]], "title_bars": true }),
        )
        .unwrap();
    let at = header_title(&mut h, "4 Audio");
    let nodes = Harness::nodes(&right_click(&mut h, at));
    // The right-click kept the selection; Link Tracks is drawn for it.
    assert_eq!(h.shell.arrangement().selected_tracks(), [all[2], all[3]]);
    assert!(item(&nodes, "Link Tracks").disabled);
    assert!(item(&nodes, "Unlink Track(s)").disabled);
    let steps = h.shell.editor().undo_history().count();
    choose(&mut h, "Delete");
    let left = tracks(&h);
    assert!(!left.contains(&all[2]) && !left.contains(&all[3]));
    assert_eq!(h.shell.editor().undo_history().count(), steps + 1);
    assert_eq!(last_step(&h), "Delete Track(s)");
}

// Spec: track-context-menu R7 (a group, a time selection)
#[test]
fn a_group_and_a_time_selection_add_their_items() {
    let mut h = arrangement();
    let all = tracks(&h);
    h.shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [all[2], all[3]], "title_bars": true }),
        )
        .unwrap();
    h.shell.run("track.group", json!({})).unwrap();
    h.settle();
    let at = header_title(&mut h, "Group");
    let nodes = Harness::nodes(&right_click(&mut h, at));
    assert!(!item(&nodes, "Ungroup Tracks").disabled);
    for label in [
        "Bounce Group to New Track",
        "Assign Track Color to Grouped Tracks and Clips",
    ] {
        assert!(item(&nodes, label).disabled, "{label}");
    }
    choose(&mut h, "Ungroup Tracks");
    assert!(
        tracks(&h)
            .iter()
            .all(|t| kind_of(&h, *t) != TrackKind::Group)
    );

    // From a time selection on the track: Crop Clip waits for WP-4.13, and
    // the edit items work.
    let audio = tracks(&h)[2];
    h.shell
        .run(
            "arrangement.select_time",
            json!({ "tracks": [audio], "start": 0, "end": 2_822_400 }),
        )
        .unwrap();
    let at = header_title(&mut h, "3 Audio");
    let nodes = Harness::nodes(&right_click(&mut h, at));
    assert!(item(&nodes, "Crop Clip").disabled);
    assert!(item(&nodes, "Crop Clip").description.contains("WP-4.13"));
    assert!(!item(&nodes, "Copy").disabled);
}

// Spec: track-context-menu R1, R8 (the Session View)
#[test]
fn the_session_view_s_title_bars_open_the_same_menu() {
    let mut h = session();
    let audio = tracks(&h)[2];
    let at = session_title(&h, audio);
    let nodes = Harness::nodes(&right_click(&mut h, at));
    assert_eq!(h.shell.arrangement().selected_tracks(), [audio]);
    for label in ["Delete", "Rename", "Group Tracks", "Insert Audio Track"] {
        assert!(!item(&nodes, label).disabled, "{label} works");
    }
    // Take lanes are the Arrangement's.
    assert!(!nodes.iter().any(|n| n.label.starts_with("Show Take Lanes")));
    let before = tracks(&h).len();
    h.take_events();
    choose(&mut h, "Insert Audio Track");
    assert!(invoked(&mut h, "track.create"));
    assert_eq!(tracks(&h).len(), before + 1);

    let at = session_title(&h, audio);
    right_click(&mut h, at);
    h.take_events();
    choose(&mut h, "Delete");
    assert!(invoked(&mut h, "track.delete"));
    assert!(h.shell.editor().document().track(audio).is_none());
}

// Spec: track-context-menu R1 (the Arrangement's mixer)
#[test]
fn the_arrangement_mixer_s_title_bars_open_it_too() {
    let mut h = arrangement();
    h.shell.run(TOGGLE, json!({})).unwrap();
    let nodes = Harness::nodes(&h.settle());
    let b = Harness::node(&nodes, "Arrangement Mixer").bounds.unwrap();
    let row = Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y0 as f32 + 17.0),
    );
    let audio = tracks(&h)[2];
    let at = title_in(&h, row, audio);
    let nodes = Harness::nodes(&right_click(&mut h, at));
    assert_eq!(h.shell.arrangement().selected_tracks(), [audio]);
    assert!(!item(&nodes, "Delete").disabled);
    assert!(!nodes.iter().any(|n| n.label.starts_with("Show Take Lanes")));
}

// Spec: accessibility R8 (S-CTX-002), track-context-menu R9
#[test]
fn the_keys_open_the_selected_title_bar_s_menu() {
    let mut h = arrangement();
    let audio = tracks(&h)[2];
    let at = header_title(&mut h, "3 Audio");
    h.click(at);
    h.settle();
    assert!(h.shell.arrangement().title_bars_selected());
    h.ctx.memory_mut(|m| m.request_focus(main_view_id()));
    h.settle();
    let output = if cfg!(target_os = "macos") {
        h.key(Key::M, Modifiers::CTRL | Modifiers::ALT | Modifiers::SHIFT);
        h.settle()
    } else {
        h.key(Key::F10, Modifiers::SHIFT);
        h.settle()
    };
    assert!(egui::Popup::is_any_open(&h.ctx));
    // The first item that can be chosen is focused: Cut, Copy and Duplicate
    // have nothing to act on.
    let nodes = Harness::nodes(&output);
    assert!(!item(&nodes, "Duplicate Track Without Clips").disabled);
    let focus = output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .focus;
    let focused = nodes
        .iter()
        .find(|n| n.id == focus)
        .map(|n| n.label.clone())
        .unwrap_or_default();
    assert_eq!(focused, "Duplicate Track Without Clips");
    // Down moves to Delete; Enter runs it.
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.settle();
    h.take_events();
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert!(invoked(&mut h, "track.delete"));
    assert!(h.shell.editor().document().track(audio).is_none());
}

// Spec: accessibility R8 (S-CTX-002), track-context-menu R9 (the Session View)
#[test]
fn the_keys_open_a_session_column_s_title_menu() {
    let mut h = session();
    let audio = tracks(&h)[2];
    h.ctx
        .memory_mut(|m| m.request_focus(column_id(MixerOf::Session, audio)));
    h.settle();
    if cfg!(target_os = "macos") {
        h.key(Key::M, Modifiers::CTRL | Modifiers::ALT | Modifiers::SHIFT);
    } else {
        h.key(Key::F10, Modifiers::SHIFT);
    }
    let nodes = Harness::nodes(&h.settle());
    assert!(egui::Popup::is_any_open(&h.ctx));
    assert!(!item(&nodes, "Delete").disabled);
    assert_eq!(h.shell.arrangement().selected_tracks(), [audio]);
}

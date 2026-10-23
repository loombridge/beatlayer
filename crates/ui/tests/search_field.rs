//! The search field in the main window (WP-6.42, IMP-16 and IMP-18; the
//! command palette spec, R1 to R14): its keys from every part of the
//! window, typing, choosing a row, the Tracks scope's Return, the outlines
//! in the Arrangement, the Session View and the mixers, Show Only Matches,
//! assistive technology, and the field's place at every zoom.

use crate::common;
use bl_commands::Context;
use bl_core::time::Ticks;
use bl_model::{Clip, ClipData, ClipId, Command, MidiData, TrackId};
use bl_shell::files::factory_default_set;
use bl_shell::search_commands::SearchScope;
use bl_shell::{Area, MainView};
use bl_ui::arrangement::benchmark;
use bl_ui::arrangement::headers::title_id;
use bl_ui::browser::search_id;
use bl_ui::layout::{SEARCH_ROW, SEARCH_ROWS, search_field_rect};
use common::{Harness, Window, command};
use egui::accesskit::Role;
use egui::epaint::Shape;
use egui::{Color32, Event, FullOutput, Key, Modifiers, Rect};
use serde_json::json;

const BAR: i64 = Ticks::WHOLE.get();

fn harness() -> Harness {
    let mut h = Harness::with_title_bar(Window::Main);
    // The keys and the rows' availability follow where the keyboard was
    // before the field opened.
    h.shell.set_focused_area(Area::MainView);
    h.settle();
    h
}

fn tracks(h: &Harness) -> Vec<TrackId> {
    h.shell.editor().document().top_level().collect()
}

/// Cmd-Shift-P (Ctrl-Shift-P), then frames for the field to open and take
/// the keyboard.
fn open_commands(h: &mut Harness) {
    h.key(Key::P, command() | Modifiers::SHIFT);
    h.settle();
}

/// Cmd-Shift-O (Ctrl-Shift-O).
fn open_tracks(h: &mut Harness) {
    h.key(Key::O, command() | Modifiers::SHIFT);
    h.settle();
}

fn type_text(h: &mut Harness, text: &str) {
    h.frame(vec![Event::Text(text.to_owned())]);
    h.settle();
}

fn press(h: &mut Harness, key: Key) {
    h.key(key, Modifiers::NONE);
    h.settle();
}

fn rename(h: &mut Harness, track: TrackId, name: &str) {
    h.shell
        .run("track.rename", json!({ "track": track, "name": name }))
        .unwrap();
}

fn selected(h: &Harness) -> Vec<TrackId> {
    h.shell.arrangement().selected_tracks().to_vec()
}

/// The rectangles a frame outlined in the search colour.
fn outlines(h: &Harness, output: &FullOutput) -> Vec<Rect> {
    let colour: Color32 = h.theme.colors.search.0;
    let mut found = Vec::new();
    for clipped in &output.shapes {
        collect(&clipped.shape, colour, &mut found);
    }
    found
}

fn collect(shape: &Shape, colour: Color32, found: &mut Vec<Rect>) {
    match shape {
        Shape::Rect(rect) if rect.stroke.color == colour && rect.stroke.width > 0.0 => {
            found.push(rect.rect);
        }
        Shape::Vec(shapes) => {
            for shape in shapes {
                collect(shape, colour, found);
            }
        }
        _ => {}
    }
}

// Spec: command-palette R1, R2. Cmd-Shift-P opens the field on the Commands
// scope wherever the keyboard is: the Arrangement, the Session View, the
// note editor and a text field.
#[test]
fn the_key_opens_the_field_from_every_part_of_the_window() {
    // The Arrangement.
    let mut h = harness();
    open_commands(&mut h);
    assert!(h.shell.search_open());
    assert_eq!(h.shell.search().scope, SearchScope::Commands);
    assert!(
        h.ctx
            .memory(|m| m.has_focus(bl_ui::search_field::text_id()))
    );

    // The Session View.
    let mut h = harness();
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    h.settle();
    assert_eq!(h.shell.main_view(), MainView::Session);
    open_commands(&mut h);
    assert!(h.shell.search_open());

    // A text field: the browser's search field.
    let mut h = harness();
    h.key(Key::F, command());
    h.settle();
    assert!(h.ctx.memory(|m| m.has_focus(search_id())));
    open_commands(&mut h);
    assert!(h.shell.search_open(), "the key works in a text field");
    assert_eq!(h.shell.search().scope, SearchScope::Commands);

    // The note editor.
    let mut editor = factory_default_set();
    let first = editor.document().top_level().next().unwrap();
    let clip: ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track: first,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(4 * BAR),
                ClipData::Midi(MidiData::new(vec![])),
            ),
        })
        .unwrap();
    editor.clear_history().unwrap();
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell = benchmark::shell_with(editor);
    h.settle();
    h.shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    h.shell.set_focused_area(Area::Detail);
    h.settle();
    assert!(h.shell.active_contexts().contains(&Context::NoteEditor));
    open_commands(&mut h);
    assert!(h.shell.search_open(), "the key works in the note editor");
}

// Spec: command-palette R3, R8. Typing, then Return, runs Insert MIDI Track
// as one undo step and closes the field, and the keyboard goes back to where
// it was.
#[test]
fn typing_and_return_run_a_command_as_one_undo_step_and_close_the_field() {
    let mut h = harness();
    h.shell.set_focused_area(Area::MainView);
    let before = tracks(&h).len();
    let history = h.shell.editor().undo_history().len();
    open_commands(&mut h);
    type_text(&mut h, "insert midi track");
    let state = h.shell.search_state();
    assert_eq!(state.text, "insert midi track");
    assert_eq!(state.commands[0].label, "Insert MIDI Track");
    assert_eq!(state.highlight, Some(0));
    press(&mut h, Key::Enter);
    assert!(!h.shell.search_open(), "running a command closes the field");
    assert_eq!(tracks(&h).len(), before + 1);
    assert_eq!(h.shell.editor().undo_history().len(), history + 1);
    h.shell.invoke("edit.undo", json!({})).unwrap();
    assert_eq!(tracks(&h).len(), before);
    // The keyboard isn't left in the closed field.
    assert!(
        !h.ctx
            .memory(|m| m.has_focus(bl_ui::search_field::text_id()))
    );
}

// Spec: command-palette R3, R8. Down and Up move the highlight, wrapping; a
// click on a row runs it; Esc closes the field and changes nothing.
#[test]
fn the_arrows_a_click_and_escape() {
    let mut h = harness();
    let before = tracks(&h).len();
    open_commands(&mut h);
    type_text(&mut h, "insert");
    let rows = h.shell.search().commands.len();
    assert!(rows > 2);
    press(&mut h, Key::ArrowUp);
    assert_eq!(
        h.shell.search().highlight,
        rows - 1,
        "Up from the first wraps"
    );
    press(&mut h, Key::ArrowDown);
    assert_eq!(h.shell.search().highlight, 0, "Down from the last wraps");
    press(&mut h, Key::ArrowDown);
    assert_eq!(h.shell.search().highlight, 1);
    // Esc closes the field and runs nothing.
    press(&mut h, Key::Escape);
    assert!(!h.shell.search_open());
    assert_eq!(tracks(&h).len(), before);
    // A click on a row runs it.
    open_commands(&mut h);
    type_text(&mut h, "insert midi track");
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    let option = nodes
        .iter()
        .find(|n| n.role == Role::ListBoxOption && n.label.starts_with("Insert MIDI Track"))
        .expect("the row");
    let bounds = option.bounds.unwrap();
    h.click(egui::pos2(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    ));
    assert!(!h.shell.search_open());
    assert_eq!(tracks(&h).len(), before + 1);
}

// Spec: command-palette R7. A row that can't run now is dimmed, does nothing
// when chosen, and the Status Bar says so.
#[test]
fn an_unavailable_row_does_nothing_and_says_so() {
    let mut h = harness();
    open_commands(&mut h);
    type_text(&mut h, "redo");
    let state = h.shell.search_state();
    let at = state
        .commands
        .iter()
        .position(|row| row.command == "edit.redo")
        .expect("Redo is a row");
    assert!(!state.commands[at].available);
    for _ in 0..at {
        press(&mut h, Key::ArrowDown);
    }
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    let option = nodes
        .iter()
        .find(|n| n.role == Role::ListBoxOption && n.label.starts_with("Nothing to Redo"))
        .unwrap_or_else(|| {
            panic!(
                "the row: {:?}",
                nodes
                    .iter()
                    .filter(|n| n.role == Role::ListBoxOption)
                    .map(|n| &n.label)
                    .collect::<Vec<_>>()
            )
        });
    assert!(option.disabled, "the row reads as unavailable");
    press(&mut h, Key::Enter);
    assert!(h.shell.search_open(), "the field stays open");
    let message = h.shell.message().expect("the Status Bar says so");
    assert!(
        message.text.contains("isn't available now"),
        "{}",
        message.text
    );
}

// Spec: command-palette R2, R5. The Info View shows the highlighted row's
// description.
#[test]
fn the_info_view_describes_the_highlighted_row() {
    let mut h = harness();
    open_commands(&mut h);
    type_text(&mut h, "insert midi track");
    h.settle();
    let content = h.main.info_view_content();
    assert_eq!(content.title, "Insert MIDI Track");
    assert!(content.body.contains("track"), "{}", content.body);
}

// Spec: command-palette R1, R2. Cmd-Shift-O switches an open field to the
// Tracks scope and keeps the text; Return walks the matches in display
// order, wrapping and opening a folded group.
#[test]
fn the_tracks_scope_walks_the_matches_with_return() {
    let mut h = harness();
    h.shell
        .invoke("track.create", json!({ "kind": "group" }))
        .unwrap();
    let group = *tracks(&h).last().unwrap();
    let inside = tracks(&h)[..2].to_vec();
    for track in &inside {
        h.shell
            .run(
                "track.move",
                json!({ "track": track, "parent": group, "index": 0 }),
            )
            .unwrap();
    }
    h.shell
        .run("arrangement.fold_tracks", json!({ "tracks": [group] }))
        .unwrap();
    for (track, name) in inside.iter().zip(["Kick", "Kicker"]) {
        rename(&mut h, *track, name);
    }
    let outside = tracks(&h)[1];
    rename(&mut h, outside, "Sidekick");
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [] }))
        .unwrap();
    h.settle();

    open_commands(&mut h);
    type_text(&mut h, "kick");
    open_tracks(&mut h);
    assert_eq!(h.shell.search().scope, SearchScope::Tracks);
    assert_eq!(h.shell.search().text, "kick", "the text stays");
    let order: Vec<TrackId> = h.shell.search().tracks.iter().map(|r| r.track).collect();
    assert_eq!(order.len(), 3);
    let mut walked = Vec::new();
    for _ in 0..4 {
        press(&mut h, Key::Enter);
        walked.push(selected(&h));
    }
    assert_eq!(walked[0], [order[0]]);
    assert_eq!(walked[1], [order[1]]);
    assert_eq!(walked[2], [order[2]]);
    assert_eq!(walked[3], [order[0]], "wrapping");
    assert!(!h.shell.arrangement().is_folded(group), "the group opened");
    assert!(h.shell.search_open(), "the field stays open");
    h.key(Key::Enter, Modifiers::SHIFT);
    h.settle();
    assert_eq!(selected(&h), [order[2]], "Shift-Return goes back");
    // Choosing a row selects its track the same way.
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    let row = nodes
        .iter()
        .find(|n| n.role == Role::ListBoxOption && n.label == "Kick")
        .expect("the row");
    let bounds = row.bounds.unwrap();
    h.click(egui::pos2(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    ));
    let kick = h
        .shell
        .search()
        .tracks
        .iter()
        .find(|row| row.name == "Kick")
        .unwrap()
        .track;
    assert_eq!(selected(&h), [kick]);
}

// Spec: command-palette R10. The tracks the Tracks scope finds are outlined
// in the Arrangement's headers, the Session View's title bars and both
// mixers' title bars.
#[test]
fn the_matches_are_outlined_in_every_view() {
    let mut h = harness();
    let ids = tracks(&h);
    rename(&mut h, ids[0], "Bassline");
    h.settle();
    // Nothing is outlined before the search.
    let before = h.settle();
    assert!(outlines(&h, &before).is_empty());
    open_tracks(&mut h);
    type_text(&mut h, "bassline");
    let arrangement = h.settle();
    assert_eq!(
        outlines(&h, &arrangement).len(),
        1,
        "the Arrangement's header"
    );
    // With the Arrangement's mixer showing, its title bar is outlined too.
    h.shell
        .run(
            "mixer_view.toggle",
            json!({ "view": "arrangement", "shown": true }),
        )
        .unwrap();
    let mixer = h.settle();
    assert_eq!(
        outlines(&h, &mixer).len(),
        2,
        "the header and the mixer's title bar"
    );
    // The Session View's title bars, and its mixer.
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    let session = h.settle();
    assert_eq!(
        outlines(&h, &session).len(),
        1,
        "the Session View's title bar"
    );
    // The Commands scope outlines nothing.
    h.shell
        .run("search.set_scope", json!({ "scope": "commands" }))
        .unwrap();
    let commands = h.settle();
    assert!(outlines(&h, &commands).is_empty());
    // A rename moves the outline while the field is open.
    h.shell
        .run("search.set_scope", json!({ "scope": "tracks" }))
        .unwrap();
    rename(&mut h, ids[1], "Bassline 2");
    let both = h.settle();
    assert_eq!(outlines(&h, &both).len(), 2);
    // Closing the field removes them.
    h.shell.run("search.close", json!({})).unwrap();
    let closed = h.settle();
    assert!(outlines(&h, &closed).is_empty());
}

// Spec: command-palette R12. Show Only Matches hides the other tracks in
// the Arrangement and the Session View, and closing the field shows them
// again.
#[test]
fn show_only_matches_hides_the_others_in_both_views() {
    let mut h = harness();
    let ids = tracks(&h);
    rename(&mut h, ids[0], "Pad");
    let shown = |h: &mut Harness| {
        let output = h.settle();
        let nodes = Harness::nodes(&output);
        ids.iter()
            .map(|track| {
                nodes
                    .iter()
                    .any(|node| node.id == title_id(*track).accesskit_id())
            })
            .collect::<Vec<_>>()
    };
    assert!(shown(&mut h).iter().all(|shown| *shown));
    open_tracks(&mut h);
    type_text(&mut h, "pad");
    // The switch sits in the field, off.
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    let switch = nodes
        .iter()
        .find(|n| n.role == Role::Switch && n.label == "Show Only Matches")
        .expect("the switch");
    assert_eq!(switch.toggled, Some(false));
    let bounds = switch.bounds.unwrap();
    h.click(egui::pos2(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    ));
    assert!(h.shell.search().only_matches);
    let on = shown(&mut h);
    assert!(on[0], "the match shows");
    assert!(on[1..].iter().all(|shown| !shown), "the others are hidden");
    // The Session View hides them too.
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    h.settle();
    assert_eq!(h.shell.mixer_columns().first(), Some(&ids[0]));
    assert_eq!(h.shell.mixer_columns().len(), 1);
    // Closing the field shows every track again.
    h.shell.run("search.close", json!({})).unwrap();
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    assert!(shown(&mut h).iter().all(|shown| *shown));
}

// Spec: command-palette R13. The field is a search box, the scopes are a
// pair of toggle buttons, the list is a list box whose options read the
// row, and the number of rows is announced.
#[test]
fn assistive_technology_reads_the_field_the_scopes_and_the_rows() {
    let mut h = harness();
    open_commands(&mut h);
    type_text(&mut h, "insert midi");
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::Group && n.label == "Search Commands"),
        "the panel is named"
    );
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::SearchInput && n.label == "Search Commands"),
        "the text is a search box"
    );
    let commands = nodes
        .iter()
        .find(|n| n.role == Role::Button && n.label == "Commands")
        .expect("the Commands button");
    let tracks_button = nodes
        .iter()
        .find(|n| n.role == Role::Button && n.label == "Tracks")
        .expect("the Tracks button");
    assert_eq!(commands.toggled, Some(true));
    assert_eq!(tracks_button.toggled, Some(false));
    let list = nodes
        .iter()
        .find(|n| n.role == Role::ListBox && n.label == "Commands")
        .expect("the list box");
    let options: Vec<&common::Node> = nodes
        .iter()
        .filter(|n| n.role == Role::ListBoxOption)
        .collect();
    assert!(!options.is_empty());
    let first = options
        .iter()
        .find(|o| o.label.starts_with("Insert MIDI Track"))
        .expect("the Insert MIDI Track row");
    assert!(
        first.label.starts_with("Insert MIDI Track, Create"),
        "{}",
        first.label
    );
    assert_eq!(
        first.selected,
        Some(true),
        "the highlighted row is selected"
    );
    assert!(
        options
            .iter()
            .all(|o| Harness::descends_from(&nodes, o.id, list.id))
    );
    let count = h.shell.search().commands.len();
    let announced = nodes
        .iter()
        .find(|n| n.role == Role::Status && n.label.ends_with("results"))
        .expect("the count is announced");
    assert_eq!(announced.label, format!("{count} results"));
    // The Tracks scope's field has its own name, and the switch.
    open_tracks(&mut h);
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::SearchInput && n.label == "Find Track")
    );
    let tracks_button = nodes
        .iter()
        .find(|n| n.role == Role::Button && n.label == "Tracks")
        .unwrap();
    assert_eq!(tracks_button.toggled, Some(true));
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::Switch && n.label == "Show Only Matches")
    );
}

// Spec: command-palette R14. At every zoom from 50% to 200% the field keeps
// its place at the main view's top, scaled by the zoom.
#[test]
fn the_field_keeps_its_place_at_every_zoom() {
    for percent in [50, 75, 100, 150, 200] {
        let mut h = harness();
        h.shell
            .run("view.set_zoom", json!({ "percent": percent }))
            .unwrap();
        h.settle();
        open_commands(&mut h);
        type_text(&mut h, "insert");
        let output = h.settle();
        let nodes = Harness::nodes(&output);
        let group = nodes
            .iter()
            .find(|n| n.role == Role::Group && n.label == "Search Commands")
            .expect("the panel");
        let bounds = group.bounds.expect("the field's bounds");
        let main_view = h.main.drawn_layout().expect("a drawn layout").main_view;
        let rows = h.shell.search().commands.len().min(SEARCH_ROWS);
        let expected = search_field_rect(main_view, rows);
        let drawn = Rect::from_min_max(
            egui::pos2(bounds.x0 as f32, bounds.y0 as f32),
            egui::pos2(bounds.x1 as f32, bounds.y1 as f32),
        );
        assert!(
            (drawn.min - expected.min).length() < 0.5
                && (drawn.size() - expected.size()).length() < 0.5,
            "{percent}%: {drawn:?} against {expected:?}"
        );
        assert!(rows as f32 * SEARCH_ROW > 0.0);
    }
}

// Spec: command-palette R3. Closing the field gives the keyboard back to
// where it was: here the browser's search field.
#[test]
fn closing_the_field_gives_the_keyboard_back() {
    let mut h = harness();
    h.key(Key::F, command());
    h.settle();
    assert!(h.ctx.memory(|m| m.has_focus(search_id())));
    open_commands(&mut h);
    assert!(
        !h.ctx.memory(|m| m.has_focus(search_id())),
        "the field has it"
    );
    press(&mut h, Key::Escape);
    assert!(!h.shell.search_open());
    assert!(
        h.ctx.memory(|m| m.has_focus(search_id())),
        "and gives it back"
    );
    // A click outside the field closes it too.
    open_commands(&mut h);
    h.click(egui::pos2(40.0, 600.0));
    assert!(!h.shell.search_open(), "a click outside closes the field");
}

// Spec: command-palette R13. Tab moves between the field, the scope buttons
// and Show Only Matches.
#[test]
fn tab_reaches_the_scope_buttons_and_the_switch() {
    let mut h = harness();
    open_tracks(&mut h);
    let focused = |h: &Harness| {
        h.ctx
            .memory(egui::Memory::focused)
            .map(|id| id.accesskit_id())
    };
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    let id_of = |role, label: &str| {
        nodes
            .iter()
            .find(|n| n.role == role && n.label == label)
            .unwrap_or_else(|| panic!("no {label}"))
            .id
    };
    let text = nodes
        .iter()
        .find(|n| n.role == Role::SearchInput)
        .unwrap()
        .id;
    let switch = id_of(Role::Switch, "Show Only Matches");
    let commands = id_of(Role::Button, "Commands");
    let tracks_button = id_of(Role::Button, "Tracks");
    assert_eq!(focused(&h), Some(text), "the text has the keyboard");
    let mut reached = vec![];
    for _ in 0..4 {
        press(&mut h, Key::Tab);
        reached.push(focused(&h));
    }
    assert!(reached.contains(&Some(switch)), "Tab reaches the switch");
    // Shift-Tab walks back through the buttons.
    let mut back = vec![];
    for _ in 0..6 {
        h.key(Key::Tab, Modifiers::SHIFT);
        h.settle();
        back.push(focused(&h));
    }
    assert!(
        back.contains(&Some(commands)) && back.contains(&Some(tracks_button)),
        "Shift-Tab reaches both scope buttons"
    );
}

// Spec: command-palette R11. Return brings the selected track's column into
// view in the Session View.
#[test]
fn return_scrolls_the_session_view_to_the_track() {
    // With the Session mixer showing, and with it hidden.
    for shown in [true, false] {
        scrolls_to_the_track(shown);
    }
}

fn scrolls_to_the_track(mixer_shown: bool) {
    let mut h = harness();
    h.shell
        .run(
            "mixer_view.toggle",
            json!({ "view": "session", "shown": mixer_shown }),
        )
        .unwrap();
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    for _ in 0..30 {
        h.shell
            .invoke("track.create", json!({ "kind": "midi" }))
            .unwrap();
    }
    let last = *tracks(&h).last().unwrap();
    rename(&mut h, last, "Zed");
    let first = tracks(&h)[0];
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [first] }))
        .unwrap();
    h.settle();
    let scroll = |h: &Harness| {
        h.shell
            .mixer_view()
            .of(bl_shell::mixer_view::MixerOf::Session)
            .scroll
    };
    assert_eq!(scroll(&h), 0.0);
    open_tracks(&mut h);
    type_text(&mut h, "zed");
    press(&mut h, Key::Enter);
    h.settle();
    h.settle();
    assert_eq!(selected(&h), [last]);
    assert!(scroll(&h) > 0.0, "the Session View scrolled to the track");
}

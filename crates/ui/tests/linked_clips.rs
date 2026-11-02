//! Linked clips in the UI (WP-6.44), headless: the items in the Edit menu
//! and the clips' context menus, the link mark on a linked clip's slot and
//! its Status Bar text, a click on the mark selecting the link's clips, and
//! the names assistive technology reads. Spec: linked-clips R1, R8, R10,
//! R11.

use crate::common::{Harness, Window};
use bl_model::{ClipId, SceneId, TrackId};
use bl_shell::context_menus::{self, SessionClipMenu};
use bl_shell::menus::{MenuPlatform, menu_bar};
use bl_ui::layout::mixer::{MixerLayout, Sections, SessionLayout, TrackColumn};
use bl_ui::layout::{REFERENCE, ShellLayout, TITLE_BAR};
use bl_ui::main_window::UiEvent;
use egui::{Event, PointerButton, Pos2, Rect, vec2};
use serde_json::json;

fn labels(items: &[bl_shell::menus::MenuItem]) -> Vec<&str> {
    items
        .iter()
        .filter(|item| !item.is_separator())
        .map(|item| item.label)
        .collect()
}

// Spec: linked-clips R1, R8
#[test]
fn the_items_sit_after_duplicate_in_the_edit_menu_and_the_clips_menus() {
    let edit = menu_bar(MenuPlatform::Other)
        .into_iter()
        .find(|menu| menu.title == "Edit")
        .expect("an Edit menu");
    let edit = labels(&edit.items);
    let at = edit
        .iter()
        .position(|label| *label == "Duplicate Track Without Clips")
        .unwrap();
    assert_eq!(edit[at + 1], "Duplicate as Linked Clip");

    for menu in [
        context_menus::session_clip(SessionClipMenu::default()),
        context_menus::arrangement_clip(),
    ] {
        let menu = labels(&menu);
        let at = menu.iter().position(|label| *label == "Duplicate").unwrap();
        assert_eq!(menu[at + 1], "Duplicate as Linked Clip", "{menu:?}");
        assert_eq!(menu[at + 2], "Unlink Clip", "{menu:?}");
    }
}

fn harness() -> Harness {
    let mut h = Harness::manual_with_title_bar(Window::Main);
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    h.settle();
    h
}

/// The slot's cell, as the view draws it.
fn cell(h: &Harness, track: TrackId, scene: SceneId) -> Rect {
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
    let column = columns.iter().position(|t| *t == track).unwrap();
    let row = h.shell.editor().document().scene_position(scene).unwrap();
    layout.columns[column]
        .rect
        .intersect(session.scene_row(row, 0.0))
        .shrink(1.0)
}

/// Where the link mark is in a slot: the top right corner.
fn mark(h: &Harness, track: TrackId, scene: SceneId) -> Pos2 {
    let cell = cell(h, track, scene);
    Pos2::new(
        cell.right() - 1.0 - bl_ui::link_mark::WIDTH / 2.0,
        cell.top() + 8.0,
    )
}

/// The first MIDI track's first two slots, the first holding a clip, linked
/// with a copy in the second.
fn linked_slots(h: &mut Harness) -> (TrackId, [SceneId; 2], [ClipId; 2]) {
    let doc = h.shell.editor().document();
    let track = doc
        .tracks()
        .find(|t| t.kind() == bl_model::TrackKind::Midi)
        .unwrap()
        .id();
    let scenes: Vec<SceneId> = doc.scenes().map(|scene| scene.id).collect();
    h.shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scenes[0] }),
        )
        .unwrap();
    let clip = h
        .shell
        .editor()
        .document()
        .slot(track, scenes[0])
        .unwrap()
        .id;
    let made = h
        .shell
        .run("clip.duplicate_linked", json!({ "clips": [clip] }))
        .unwrap();
    let copy: ClipId = serde_json::from_value(made["created"][0].clone()).unwrap();
    h.settle();
    (track, [scenes[0], scenes[1]], [clip, copy])
}

// Spec: linked-clips R10, R11
#[test]
fn a_linked_clips_slot_is_named_with_its_link_and_its_mark_selects_the_link() {
    let mut h = harness();
    let (track, scenes, clips) = linked_slots(&mut h);
    assert_eq!(
        h.shell
            .editor()
            .document()
            .slot(track, scenes[1])
            .map(|c| c.id),
        Some(clips[1]),
        "the copy is in the first empty slot below"
    );

    // R11: the slot's name says it's linked and how many clips its link has.
    let nodes = Harness::nodes(&h.settle());
    assert!(
        nodes
            .iter()
            .any(|n| n.label.contains("linked, 2 clips") && n.label.contains(": 1")),
        "{:?}",
        nodes.iter().map(|n| &n.label).collect::<Vec<_>>()
    );
    let named = "Select the 2 clips linked with this one";
    let buttons = nodes.iter().filter(|n| n.label == named).count();
    assert_eq!(
        buttons, 2,
        "each linked clip's slot has the mark as a button"
    );

    // R10: pointing at the mark says how many others, in the Status Bar.
    h.take_events();
    let at = mark(&h, track, scenes[0]);
    h.frame(vec![Event::PointerMoved(at)]);
    let hints: Vec<Option<String>> = h
        .take_events()
        .into_iter()
        .filter_map(|event| match event {
            UiEvent::StatusHint(hint) => Some(hint),
            _ => None,
        })
        .collect();
    assert!(
        hints.contains(&Some("Linked to 1 other clip".to_owned())),
        "{hints:?}"
    );

    // R10: clicking it selects the slots of the link's clips.
    for pressed in [true, false] {
        h.frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }]);
    }
    h.settle();
    let selected: Vec<SceneId> = h
        .shell
        .session()
        .state()
        .slots
        .iter()
        .map(|slot| slot.scene)
        .collect();
    assert!(
        selected.len() == 2 && selected.contains(&scenes[0]) && selected.contains(&scenes[1]),
        "{selected:?}"
    );
}

// Spec: linked-clips R10, R11
#[test]
fn a_linked_clips_title_bar_in_the_arrangement_has_the_mark_and_it_selects_the_link() {
    let mut h = Harness::manual_with_title_bar(Window::Main);
    let track = h
        .shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == bl_model::TrackKind::Midi)
        .unwrap()
        .id();
    let clip: ClipId = h.shell.new_id();
    let made = bl_model::Clip::new(
        clip,
        bl_core::time::Ticks::ZERO,
        bl_core::time::Ticks::WHOLE * 2,
        bl_model::ClipData::Midi(bl_model::MidiData::default()),
    );
    h.shell
        .run("clip.create", json!({ "track": track, "clip": made }))
        .unwrap();
    h.shell
        .run("clip.duplicate_linked", json!({ "clips": [clip] }))
        .unwrap();
    h.settle();

    // R11: both clips are read as linked, with their link's size.
    let nodes = Harness::nodes(&h.settle());
    let linked: Vec<_> = nodes
        .iter()
        .filter(|n| n.label == "MIDI Clip, linked, 2 clips")
        .collect();
    assert_eq!(
        linked.len(),
        2,
        "{:?}",
        nodes.iter().map(|n| &n.label).collect::<Vec<_>>()
    );

    // R10: the mark is at the right end of the title bar; pointing at it says
    // so in the Status Bar, and clicking it selects both clips.
    let (_, rect) = h
        .main
        .arrangement()
        .shown_clips()
        .find(|(id, _)| *id == clip)
        .expect("the clip is drawn");
    let at = Pos2::new(
        rect.right() - bl_ui::link_mark::WIDTH / 2.0 - 1.0,
        rect.top() + 6.0,
    );
    h.take_events();
    h.frame(vec![Event::PointerMoved(at)]);
    let hinted = h.take_events().into_iter().any(|event| {
        matches!(event, UiEvent::StatusHint(Some(text)) if text == "Linked to 1 other clip")
    });
    assert!(hinted);
    for pressed in [true, false] {
        h.frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }]);
    }
    h.settle();
    assert_eq!(h.shell.arrangement().selected_clips().len(), 2);
}

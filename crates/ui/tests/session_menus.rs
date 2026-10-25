//! The Session View's context menus and the Clip View title bar's (WP-6.56;
//! menus.md C-SLOT, C-SCENE, C-CLIP-SES, C-CLIPTITLE), headless: a
//! right-click on an empty slot, a clip, a scene, the Scene View's title bar,
//! a scene field and the Clip View's title bar lists Live's items with the
//! right ones enabled and ticked; the working ones run on the selection, Rename
//! types in place, and the keys open the menus.
//!
//! Spec: session-context-menus R1 to R16.

use crate::common;
use bl_model::{ClipData, ClipId, SceneId, TrackId, TrackKind};
use bl_ui::layout::mixer::{MixerLayout, SceneRow, Sections, SessionLayout, TrackColumn};
use bl_ui::layout::{REFERENCE, ShellLayout, TITLE_BAR};
use bl_ui::main_window::UiEvent;
use common::{Harness, Node, Window};
use egui::accesskit::Role;
use egui::{Event, FullOutput, Key, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use serde_json::json;

fn harness() -> Harness {
    let mut h = Harness::manual_with_title_bar(Window::Main);
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    h.settle();
    h
}

fn view_rect() -> Rect {
    ShellLayout::compute(REFERENCE, true, true)
        .translate(vec2(0.0, TITLE_BAR))
        .main_view
}

/// Where the grid draws its cells.
struct Grid {
    session: SessionLayout,
    layout: MixerLayout,
    columns: Vec<TrackId>,
    scenes: Vec<SceneId>,
}

impl Grid {
    fn new(h: &Harness) -> Self {
        let session = SessionLayout::compute(view_rect(), Some(Sections::DEFAULT));
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

    /// The centre of a track's slot for a scene, away from the launch
    /// triangle at its left.
    fn slot(&self, track: TrackId, scene: usize) -> Pos2 {
        let column = self.columns.iter().position(|t| *t == track).unwrap();
        let cell = self.layout.columns[column]
            .rect
            .intersect(self.session.scene_row(scene, 0.0))
            .shrink(1.0);
        cell.center()
    }

    fn scene_row(&self, scene: usize) -> SceneRow {
        SceneRow::compute(self.session.scene_row(scene, 0.0), self.layout.main.rect)
    }
}

fn track_of(h: &Harness, kind: TrackKind) -> TrackId {
    h.shell
        .mixer_columns()
        .into_iter()
        .find(|t| h.shell.editor().document().track(*t).unwrap().kind() == kind)
        .unwrap_or_else(|| panic!("the default set has a {kind:?} track"))
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

/// The menu item whose label is `label`, alone or with its keys after it.
fn item(output: &FullOutput, label: &str) -> Node {
    let nodes = Harness::nodes(output);
    nodes
        .iter()
        .find(|n| {
            n.role == Role::Button
                && (n.label == label
                    || n.label
                        .strip_prefix(label)
                        .is_some_and(|keys| keys.starts_with(' ') && !keys.trim().contains("  "))
                    || n.label.strip_prefix("✔ ").is_some_and(|rest| rest == label))
        })
        .unwrap_or_else(|| panic!("no menu item {label:?}"))
        .clone()
}

fn has(output: &FullOutput, label: &str) -> bool {
    Harness::nodes(output).iter().any(|n| {
        n.role == Role::Button
            && (n.label == label
                || n.label.strip_prefix("✔ ").is_some_and(|rest| rest == label)
                || n.label
                    .strip_prefix(label)
                    .is_some_and(|keys| keys.starts_with(' ') && !keys.trim().contains(' ')))
    })
}

fn click_node(h: &mut Harness, node: &Node) {
    let b = node.bounds.unwrap();
    h.click(pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    h.settle();
}

fn choose(h: &mut Harness, label: &str) {
    let output = h.settle();
    click_node(h, &item(&output, label));
}

fn invoked(h: &mut Harness, id: &str) -> bool {
    h.take_events()
        .iter()
        .any(|e| matches!(e, UiEvent::Invoke(i) if i.id == id))
}

fn clip_at(h: &Harness, track: TrackId, scene: SceneId) -> Option<ClipId> {
    h.shell
        .editor()
        .document()
        .slot(track, scene)
        .map(|clip| clip.id)
}

/// Puts an empty MIDI clip in a track's slot.
fn midi_clip(h: &mut Harness, track: TrackId, scene: SceneId) -> ClipId {
    h.shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    h.settle();
    clip_at(h, track, scene).unwrap()
}

/// Puts a short audio clip in an audio track's slot.
fn audio_clip(h: &mut Harness, track: TrackId, scene: SceneId) -> ClipId {
    let file = std::env::temp_dir().join(format!("bl-ui-session-menus-{}.wav", std::process::id()));
    let samples: Vec<f32> = (0..4_800)
        .map(|i| (i as f32 * 440.0 / 48_000.0 * std::f32::consts::TAU).sin() * 0.25)
        .collect();
    let bytes = bl_media::wav_bytes(
        &samples,
        1,
        48_000,
        bl_media::BitDepth::Int16,
        bl_media::Dither::None,
    )
    .unwrap();
    std::fs::write(&file, bytes).unwrap();
    h.shell
        .run(
            "session.drop_files",
            json!({ "paths": [file.to_string_lossy()], "track": track, "scene": scene }),
        )
        .unwrap();
    h.settle();
    let clip = clip_at(h, track, scene).expect("the audio clip is in the slot");
    assert!(matches!(
        h.shell.editor().document().clip(clip).unwrap().data,
        ClipData::Audio(_)
    ));
    clip
}

fn labels(output: &FullOutput, wanted: &[&str]) -> Vec<String> {
    wanted
        .iter()
        .map(|label| item(output, label).label)
        .collect()
}

// Spec: session-context-menus R1, R6, R12; menus.md C-SLOT-001 to C-SLOT-007.
#[test]
fn an_empty_midi_slots_menu_lists_lives_items_with_crop_clip_disabled() {
    let mut h = harness();
    let g = Grid::new(&h);
    let track = track_of(&h, TrackKind::Midi);
    let output = right_click(&mut h, g.slot(track, 3));
    let rows = [
        "Freeze Track",
        "Bounce Track in Place",
        "Bounce to New Track",
        "Paste Bounced Audio",
        "Crop Clip",
        "Remove Stop Button",
        "Insert Empty MIDI Clip",
    ];
    for row in rows {
        assert!(has(&output, row), "{row}");
    }
    // The keys Live shows beside them (menus.md).
    let key = if cfg!(target_os = "macos") {
        "⌘"
    } else {
        "Ctrl"
    };
    assert!(
        item(&output, "Remove Stop Button").label.contains(key),
        "{:?}",
        item(&output, "Remove Stop Button").label
    );
    assert!(item(&output, "Crop Clip").disabled, "no clip to crop");
    assert!(
        item(&output, "Bounce to New Track").disabled,
        "nothing to bounce"
    );
    assert!(!item(&output, "Insert Empty MIDI Clip").disabled);
    assert!(!has(&output, "Paste"), "nothing to paste");
    // The right-click selected the slot it was made on (R5).
    let selected = h.shell.session().selected_slots();
    assert_eq!(selected.len(), 1);
    assert_eq!((selected[0].track, selected[0].scene), (track, g.scenes[3]));

    // Insert Empty MIDI Clip makes the clip in the slot clicked.
    choose(&mut h, "Insert Empty MIDI Clip");
    assert!(clip_at(&h, track, g.scenes[3]).is_some());
}

// Spec: session-context-menus R1, R6. An audio track's slot has no Insert
// Empty MIDI Clip, and Remove Stop Button reads Add Stop Button once removed.
#[test]
fn an_audio_slots_menu_leaves_out_the_midi_clip_and_the_stop_button_reads_add_once_removed() {
    let mut h = harness();
    let g = Grid::new(&h);
    let track = track_of(&h, TrackKind::Audio);
    let output = right_click(&mut h, g.slot(track, 2));
    assert!(!has(&output, "Insert Empty MIDI Clip"));
    choose(&mut h, "Remove Stop Button");
    assert!(
        !h.shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .has_stop_button(g.scenes[2])
    );
    let output = right_click(&mut h, g.slot(track, 2));
    assert!(has(&output, "Add Stop Button"));
    assert!(!has(&output, "Remove Stop Button"));
}

// Spec: session-context-menus R1. A group's slot adds Bounce Group to New
// Track after Bounce to New Track.
#[test]
fn a_groups_slot_adds_bounce_group_to_new_track() {
    let mut h = harness();
    let all = h.shell.mixer_columns();
    h.shell
        .run("track.group", json!({ "tracks": [all[0], all[1]] }))
        .unwrap();
    h.settle();
    let g = Grid::new(&h);
    let group = track_of(&h, TrackKind::Group);
    let output = right_click(&mut h, g.slot(group, 1));
    assert!(has(&output, "Bounce Group to New Track"));
    let order = labels(
        &output,
        &["Bounce to New Track", "Bounce Group to New Track"],
    );
    assert_eq!(order.len(), 2);
}

// Spec: session-context-menus R2, R6, R7, R12; menus.md C-CLIP-SES-001 to
// C-CLIP-SES-018.
#[test]
fn a_midi_clips_menu_lists_lives_items_and_runs_the_working_ones() {
    let mut h = harness();
    let g = Grid::new(&h);
    let track = track_of(&h, TrackKind::Midi);
    let clip = midi_clip(&mut h, track, g.scenes[0]);
    let output = right_click(&mut h, g.slot(track, 0));
    for row in [
        "Cut",
        "Copy",
        "Duplicate",
        "Delete",
        "Rename",
        "Edit Info Text",
        "Extract Groove(s)",
        "Deactivate Clip",
        "Export MIDI Clip...",
        "Freeze Track",
        "Bounce Track in Place",
        "Bounce to New Track",
        "Paste Bounced Audio",
        "Crop Clip",
        "Remove Stop Button",
        "Create Follow Action Chain",
        "Select on Launch (Pref)",
    ] {
        assert!(has(&output, row), "{row}");
    }
    assert!(!has(&output, "Paste"), "nothing copied yet");
    // The items whose packages aren't built say so (R13).
    let info = item(&output, "Edit Info Text");
    assert!(info.disabled);
    assert!(info.description.contains("WP-6.58"), "{}", info.description);
    // Select on Launch (Pref) is ticked while it's on, as in a new set.
    assert!(
        Harness::nodes(&output)
            .iter()
            .any(|n| n.label.starts_with("✔ Select on Launch (Pref)")),
        "ticked"
    );
    assert!(!item(&output, "Export MIDI Clip...").disabled);
    assert!(!item(&output, "Deactivate Clip").disabled);
    // The palette is one grid of 70 swatches.
    let grid = Harness::nodes(&output)
        .into_iter()
        .find(|n| n.role == Role::Grid && n.label == "Colour palette")
        .expect("the palette");
    assert_eq!(grid.children.len(), 70);

    // Deactivate Clip runs session.deactivate on the selection.
    choose(&mut h, "Deactivate Clip");
    assert!(h.shell.editor().document().clip(clip).unwrap().mute);

    // After Copy the menu offers Paste straight after it.
    choose_from(&mut h, g.slot(track, 0), "Copy");
    let output = right_click(&mut h, g.slot(track, 0));
    assert!(has(&output, "Paste"));

    // Delete removes the clip.
    h.take_events();
    choose(&mut h, "Delete");
    let events: Vec<String> = h
        .take_events()
        .iter()
        .filter_map(|e| match e {
            UiEvent::Invoke(i) => Some(i.id.clone()),
            _ => None,
        })
        .collect();
    assert!(clip_at(&h, track, g.scenes[0]).is_none(), "{events:?}");
}

fn choose_from(h: &mut Harness, at: Pos2, label: &str) {
    right_click(h, at);
    choose(h, label);
}

// Spec: session-context-menus R2, R16. An audio clip's menu has no MIDI
// export and adds Slice, Convert and Separate Stems after Crop Clip.
#[test]
fn an_audio_clips_menu_adds_the_audio_rows_after_crop_clip() {
    let mut h = harness();
    let g = Grid::new(&h);
    let track = track_of(&h, TrackKind::Audio);
    audio_clip(&mut h, track, g.scenes[0]);
    let output = right_click(&mut h, g.slot(track, 0));
    assert!(!has(&output, "Export MIDI Clip..."));
    for row in [
        "Slice to New MIDI Track",
        "Convert Harmony to New MIDI Track",
        "Convert Melody to New MIDI Track",
        "Convert Drums to New MIDI Track",
        "Separate Stems to New Audio Tracks",
    ] {
        assert!(has(&output, row), "{row}");
    }
    assert!(!item(&output, "Slice to New MIDI Track").disabled);
    let stems = item(&output, "Separate Stems to New Audio Tracks");
    assert!(stems.disabled);
    assert!(
        stems.description.contains("Phase 9"),
        "{}",
        stems.description
    );
}

// Spec: session-context-menus R5, R7, R12. With several clips selected an
// item acts on all of them, and the palette colours them together.
#[test]
fn an_item_acts_on_every_selected_clip_and_a_click_outside_the_selection_selects_first() {
    let mut h = harness();
    let g = Grid::new(&h);
    let (first, second) = (
        track_of(&h, TrackKind::Midi),
        track_of(&h, TrackKind::Audio),
    );
    let a = midi_clip(&mut h, first, g.scenes[0]);
    let b = audio_clip(&mut h, second, g.scenes[0]);
    h.shell
        .run(
            "session.select",
            json!({ "track": first, "scene": g.scenes[0] }),
        )
        .unwrap();
    h.shell
        .run(
            "session.select",
            json!({ "track": second, "scene": g.scenes[0], "mode": "add" }),
        )
        .unwrap();
    h.settle();

    // A right-click on a selected clip keeps the selection of both.
    let output = right_click(&mut h, g.slot(first, 0));
    assert_eq!(h.shell.session().selected_slots().len(), 2);
    let swatch = |row: usize, column: usize| {
        let name = format!("Row {row}, colour {column}");
        Harness::nodes(&output)
            .into_iter()
            .find(|n| n.role == Role::Button && n.label == name)
            .unwrap()
    };
    h.take_events();
    click_node(&mut h, &swatch(3, 4));
    let doc = h.shell.editor().document();
    assert_eq!(doc.clip(a).unwrap().colour, Some(2 * 14 + 3));
    assert_eq!(doc.clip(b).unwrap().colour, Some(2 * 14 + 3));
    assert!(
        !egui::Popup::is_any_open(&h.ctx),
        "choosing closes the menu"
    );

    // Delete takes both.
    choose_from(&mut h, g.slot(first, 0), "Delete");
    assert!(clip_at(&h, first, g.scenes[0]).is_none());
    assert!(clip_at(&h, second, g.scenes[0]).is_none());

    // A right-click on a slot outside the selection selects it alone.
    let c = midi_clip(&mut h, first, g.scenes[1]);
    h.shell
        .run(
            "session.select",
            json!({ "track": first, "scene": g.scenes[0] }),
        )
        .unwrap();
    h.settle();
    right_click(&mut h, g.slot(first, 1));
    let selected = h.shell.session().selected_slots();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].scene, g.scenes[1]);
    assert!(clip_at(&h, first, g.scenes[1]) == Some(c));
}

// Spec: session-context-menus R3, R8, R12; menus.md C-SCENE-001 to
// C-SCENE-014.
#[test]
fn a_scenes_menu_lists_lives_items_and_the_working_ones_run() {
    let mut h = harness();
    let g = Grid::new(&h);
    let row = g.scene_row(1);
    let output = right_click(&mut h, row.name.center());
    for item_label in [
        "Cut",
        "Copy",
        "Duplicate",
        "Delete",
        "Rename",
        "Edit Info Text",
        "Insert Scene",
        "Capture and Insert Scene",
        "Select All Slots in Scene",
        "Cancel Scene Launch",
        "Select Next Scene on Launch (Pref)",
        "Trigger Recording on Launch (Pref)",
        "Scene Tempo and Time Signature",
        "No Color",
    ] {
        assert!(has(&output, item_label), "{item_label}");
    }
    assert!(
        item(&output, "Cancel Scene Launch").disabled,
        "nothing queued"
    );
    // A scene's Rename shows no key (C-SCENE-005), unlike the others.
    assert_eq!(item(&output, "Rename").label, "Rename");
    assert!(item(&output, "Delete").label.len() > "Delete".len());
    assert!(item(&output, "Edit Info Text").disabled);
    assert!(
        Harness::nodes(&output)
            .iter()
            .any(|n| n.label.starts_with("✔ Select Next Scene on Launch (Pref)")),
        "ticked in a new install"
    );
    // The right-click selected the scene.
    assert_eq!(h.shell.session().state().scenes, vec![g.scenes[1]]);

    // Select All Slots in Scene selects the scene's slot on every track that
    // has slots.
    choose(&mut h, "Select All Slots in Scene");
    let state = h.shell.session().state();
    let tracks = h
        .shell
        .mixer_columns()
        .into_iter()
        .filter(|t| {
            h.shell
                .editor()
                .document()
                .track(*t)
                .unwrap()
                .kind()
                .can_be_grouped()
        })
        .count();
    assert_eq!(state.slots.len(), tracks);
    assert!(state.slots.iter().all(|s| s.scene == g.scenes[1]));

    // Insert Scene adds one below.
    let before = g.scenes.len();
    choose_from(&mut h, row.name.center(), "Insert Scene");
    assert_eq!(h.shell.editor().document().scenes().count(), before + 1);
}

// Spec: session-context-menus R3, R8. The palette and No Color colour the
// selected scenes together.
#[test]
fn the_scene_palette_colours_the_selected_scenes_and_no_color_clears_them() {
    let mut h = harness();
    let g = Grid::new(&h);
    h.shell
        .run("session.select", json!({ "scene": g.scenes[0] }))
        .unwrap();
    h.shell
        .run(
            "session.select",
            json!({ "scene": g.scenes[2], "mode": "add" }),
        )
        .unwrap();
    h.settle();
    let output = right_click(&mut h, g.scene_row(0).name.center());
    assert_eq!(h.shell.session().state().scenes.len(), 2);
    let swatch = Harness::nodes(&output)
        .into_iter()
        .find(|n| n.role == Role::Button && n.label == "Row 2, colour 5")
        .unwrap();
    click_node(&mut h, &swatch);
    let doc = h.shell.editor().document();
    assert_eq!(doc.scene(g.scenes[0]).unwrap().colour, Some(14 + 4));
    assert_eq!(doc.scene(g.scenes[2]).unwrap().colour, Some(14 + 4));
    assert_eq!(doc.scene(g.scenes[1]).unwrap().colour, None);

    choose_from(&mut h, g.scene_row(0).name.center(), "No Color");
    let doc = h.shell.editor().document();
    assert_eq!(doc.scene(g.scenes[0]).unwrap().colour, None);
    assert_eq!(doc.scene(g.scenes[2]).unwrap().colour, None);
}

// Spec: session-context-menus R7, R8, R11. Rename types the name in the
// slot and in the scene's name field: Return keeps it, Esc doesn't; Cmd+R
// does the same from the keyboard.
#[test]
fn rename_types_in_place_and_return_keeps_the_name_while_escape_does_not() {
    let mut h = harness();
    let g = Grid::new(&h);
    let track = track_of(&h, TrackKind::Midi);
    let clip = midi_clip(&mut h, track, g.scenes[0]);
    let name_of = |h: &Harness| h.shell.editor().document().clip(clip).unwrap().name.clone();

    // Return keeps the typed name.
    choose_from(&mut h, g.slot(track, 0), "Rename");
    h.frame(vec![Event::Text("Hook".to_owned())]);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert_eq!(name_of(&h), "Hook");

    // Esc leaves it as it was.
    choose_from(&mut h, g.slot(track, 0), "Rename");
    h.frame(vec![Event::Text("X".to_owned())]);
    h.key(Key::Escape, Modifiers::NONE);
    h.settle();
    assert_eq!(name_of(&h), "Hook");

    // The scene's name field.
    let row = g.scene_row(0);
    choose_from(&mut h, row.name.center(), "Rename");
    h.frame(vec![Event::Text("Verse".to_owned())]);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert_eq!(
        h.shell.editor().document().scene(g.scenes[0]).unwrap().name,
        "Verse"
    );

    // Cmd+R renames the selected clip (S-EDIT-008).
    h.shell
        .run(
            "session.select",
            json!({ "track": track, "scene": g.scenes[0] }),
        )
        .unwrap();
    h.shell.run("session.focus", json!({})).unwrap();
    h.settle();
    h.key(Key::R, common::command());
    h.settle();
    h.frame(vec![Event::Text("Fill".to_owned())]);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert_eq!(name_of(&h), "HookFill");
}

// Spec: session-context-menus R11 (S-EDIT-005). The Delete key acts on the
// Session View's selection.
#[test]
fn the_delete_key_deletes_the_selected_clip() {
    let mut h = harness();
    let g = Grid::new(&h);
    let track = track_of(&h, TrackKind::Midi);
    midi_clip(&mut h, track, g.scenes[0]);
    h.shell
        .run(
            "session.select",
            json!({ "track": track, "scene": g.scenes[0] }),
        )
        .unwrap();
    h.shell.run("session.focus", json!({})).unwrap();
    h.settle();
    h.key(Key::Delete, Modifiers::NONE);
    h.settle();
    assert!(clip_at(&h, track, g.scenes[0]).is_none());
}

// Spec: session-context-menus R3, R8. A scene field's menu is Return to
// Default, enabled while the field holds the scene's own value.
#[test]
fn a_scene_fields_menu_returns_it_to_the_default() {
    let mut h = harness();
    h.shell
        .run("mixer_view.scene_tempo", json!({ "shown": true }))
        .unwrap();
    h.settle();
    let g = Grid::new(&h);
    let scene = g.scenes[0];
    let row = g.scene_row(0);
    // The tempo field is the left half of the name area.
    let area = row.name.shrink2(vec2(1.0, 1.0));
    let tempo = pos2(area.left() + area.width() / 8.0, area.center().y);
    // No value of its own: the item is there, disabled.
    let output = right_click(&mut h, tempo);
    assert!(item(&output, "Return to Default").disabled);
    h.key(Key::Escape, Modifiers::NONE);
    h.shell
        .run("scene.set_tempo", json!({ "scene": scene, "tempo": 133.0 }))
        .unwrap();
    h.settle();
    let output = right_click(&mut h, tempo);
    assert!(!item(&output, "Return to Default").disabled);
    choose(&mut h, "Return to Default");
    assert_eq!(
        h.shell.editor().document().scene(scene).unwrap().tempo,
        None
    );
}

// Spec: session-context-menus R3. The Scene View's title bar opens the
// scene menu for the selected scenes.
#[test]
fn the_scene_views_title_bar_opens_the_scene_menu() {
    let mut h = harness();
    let g = Grid::new(&h);
    h.shell
        .run("session.select", json!({ "scene": g.scenes[0] }))
        .unwrap();
    h.shell
        .run(
            "session.select",
            json!({ "scene": g.scenes[1], "mode": "add" }),
        )
        .unwrap();
    h.shell.run("view.show_detail_views", json!({})).unwrap();
    h.settle();
    let area = h
        .main
        .drawn_layout()
        .expect("the last frame's layout")
        .clip_view
        .expect("the detail area shows the scene");
    let title = pos2(area.left() + 120.0, area.top() + 8.0);
    let output = right_click(&mut h, title);
    assert!(has(&output, "Cancel Scene Launch"));
    assert!(has(&output, "Select All Slots in Scene"));
    // Both selected scenes stay selected: the menu acts on all.
    assert_eq!(h.shell.session().state().scenes.len(), 2);
}

// Spec: session-context-menus R14. The keys open the menu of the selection
// with the first item that can be chosen focused, and Esc closes it.
#[test]
fn the_keys_open_the_selected_clips_menu_and_the_selected_scenes() {
    let mut h = harness();
    let g = Grid::new(&h);
    let track = track_of(&h, TrackKind::Midi);
    midi_clip(&mut h, track, g.scenes[0]);
    h.shell
        .run(
            "session.select",
            json!({ "track": track, "scene": g.scenes[0] }),
        )
        .unwrap();
    h.shell.run("session.focus", json!({})).unwrap();
    h.settle();
    h.key(Key::F10, Modifiers::SHIFT);
    let output = h.settle();
    assert!(has(&output, "Extract Groove(s)"), "the clip's menu opened");
    h.key(Key::Escape, Modifiers::NONE);
    h.settle();
    assert!(!egui::Popup::is_any_open(&h.ctx));

    // With a scene selected, the scene's.
    h.shell
        .run("session.select", json!({ "scene": g.scenes[2] }))
        .unwrap();
    h.settle();
    h.key(Key::F10, Modifiers::SHIFT);
    let output = h.settle();
    assert!(
        has(&output, "Cancel Scene Launch"),
        "the scene's menu opened"
    );
}

// Spec: session-context-menus R4, R9, R13, R14. The Clip View's title bar
// opens its menu: Rename, Edit Info Text, the palette, Fold and the three
// Arrange Clip View Panels items, the ones that wait naming their package.
#[test]
fn the_clip_views_title_bar_opens_its_menu_and_rename_types_in_it() {
    let mut h = harness();
    let g = Grid::new(&h);
    let track = track_of(&h, TrackKind::Audio);
    let clip = audio_clip(&mut h, track, g.scenes[0]);
    h.shell
        .run(
            "session.select",
            json!({ "track": track, "scene": g.scenes[0] }),
        )
        .unwrap();
    h.shell.run("view.show_detail_views", json!({})).unwrap();
    h.settle();
    let detail = h
        .main
        .drawn_layout()
        .expect("the last frame's layout")
        .clip_view
        .expect("the Clip View shows");
    let title = pos2(detail.left() + 60.0, detail.top() + 7.0);
    let output = right_click(&mut h, title);
    for row in [
        "Rename",
        "Edit Info Text",
        "Fold",
        "Arrange Clip View Panels Automatically",
        "Arrange Clip View Panels Horizontally",
        "Arrange Clip View Panels Vertically",
    ] {
        assert!(has(&output, row), "{row}");
    }
    let fold = item(&output, "Fold");
    assert!(fold.disabled);
    assert!(fold.description.contains("WP-6.72"), "{}", fold.description);
    assert!(
        Harness::nodes(&output)
            .iter()
            .any(|n| n.role == Role::Grid && n.label == "Colour palette"),
        "the palette"
    );

    // The palette colours the clips the Clip View shows.
    let swatch = Harness::nodes(&output)
        .into_iter()
        .find(|n| n.role == Role::Button && n.label == "Row 1, colour 3")
        .unwrap();
    click_node(&mut h, &swatch);
    assert_eq!(
        h.shell.editor().document().clip(clip).unwrap().colour,
        Some(2)
    );

    // Rename types the name in the title bar; Return renames the clip.
    right_click(&mut h, title);
    choose(&mut h, "Rename");
    h.frame(vec![Event::Text("Vox".to_owned())]);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert!(
        h.shell
            .editor()
            .document()
            .clip(clip)
            .unwrap()
            .name
            .ends_with("Vox")
    );
}

// Spec: session-context-menus R16. The Arrangement clip's Export MIDI
// Clip... runs the export on a MIDI clip and is disabled on an audio clip.
#[test]
fn the_arrangement_clips_export_midi_clip_is_enabled_on_a_midi_clip() {
    let mut h = harness();
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    h.settle();
    let track = track_of(&h, TrackKind::Midi);
    let g = Grid::new(&h);
    let clip = midi_clip(&mut h, track, g.scenes[0]);
    h.shell
        .run(
            "session.copy_to_arrangement",
            json!({ "clips": [clip], "track": track, "at": 0 }),
        )
        .ok();
    h.settle();
    let enabled = h.shell.can_invoke_in(
        "midi.export_clip",
        &json!({ "clip": clip }),
        &[bl_commands::Context::Global],
    );
    assert!(enabled, "a MIDI clip can be exported");
    let audio_track = track_of(&h, TrackKind::Audio);
    let audio = audio_clip(&mut h, audio_track, g.scenes[1]);
    assert!(!h.shell.can_invoke_in(
        "midi.export_clip",
        &json!({ "clip": audio }),
        &[bl_commands::Context::Global]
    ));
    let _ = invoked(&mut h, "midi.export_clip");
}

// Spec: session-context-menus R11. A scene field that has the keyboard keeps
// Delete: it turns the field off and deletes nothing.
#[test]
fn a_focused_scene_field_keeps_the_delete_key() {
    let mut h = harness();
    let g = Grid::new(&h);
    let track = track_of(&h, TrackKind::Midi);
    midi_clip(&mut h, track, g.scenes[1]);
    h.shell
        .run("mixer_view.scene_tempo", json!({ "shown": true }))
        .unwrap();
    let scene = g.scenes[0];
    h.shell
        .run("scene.set_tempo", json!({ "scene": scene, "tempo": 133.0 }))
        .unwrap();
    // A clip is selected, so Delete would delete it; the Session View has
    // the keyboard.
    h.shell
        .run(
            "session.select",
            json!({ "track": track, "scene": g.scenes[1] }),
        )
        .unwrap();
    h.shell.run("session.focus", json!({})).unwrap();
    h.settle();
    let nodes = Harness::nodes(&h.settle());
    let id = bl_ui::scene_fields::field_id(scene, bl_shell::scene_fields::SceneField::Tempo)
        .accesskit_id();
    let b = nodes
        .iter()
        .find(|n| n.id == id)
        .and_then(|n| n.bounds)
        .expect("the tempo field");
    h.click(pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    h.settle();
    h.key(Key::Delete, Modifiers::NONE);
    h.settle();
    assert_eq!(
        h.shell.editor().document().scene(scene).unwrap().tempo,
        None
    );
    assert!(clip_at(&h, track, g.scenes[1]).is_some(), "the clip stays");
}

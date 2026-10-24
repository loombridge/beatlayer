//! The Clip View's Transform and Generate panels, headless (WP-6.11): the
//! panel selector row, the chosen tool's controls drawn from its
//! description, and a control's drag as one undo step that Esc puts back.
//! Spec: midi-tools R1, R2, R7, R22; L-CLIP-003, L-CLIP-007.

use crate::common;

use bl_core::time::Ticks;
use bl_model::{Clip, ClipData, ClipId, Command, MidiData, Note};
use bl_shell::files::factory_default_set;
use bl_shell::midi_tool_commands::Panel;
use bl_ui::arrangement::benchmark;
use common::{Harness, Node, Window};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect};
use serde_json::json;

const QUARTER: i64 = Ticks::QUARTER.get();

/// A MIDI clip of three notes in the Arrangement, selected and open in the
/// Clip View, with its three notes selected.
fn set() -> (Harness, ClipId) {
    let mut editor = factory_default_set();
    let track = editor.document().top_level().next().unwrap();
    let clip: ClipId = editor.new_id();
    let notes = [60u8, 62, 64]
        .into_iter()
        .enumerate()
        .map(|(at, key)| Note::new(key, Ticks(at as i64 * QUARTER), Ticks(QUARTER), 100));
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(4 * Ticks::WHOLE.get()),
                ClipData::Midi(MidiData::new(notes)),
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
    h.shell
        .run("clip_view.select_notes", json!({ "notes": [0, 1, 2] }))
        .unwrap();
    h.settle();
    (h, clip)
}

fn nodes(h: &mut Harness) -> Vec<Node> {
    Harness::nodes(&h.settle())
}

/// The selector row's button named `name`, not the group the shown panel is.
fn radio<'a>(all: &'a [Node], name: &str) -> &'a Node {
    all.iter()
        .find(|n| n.label == name && n.toggled.is_some())
        .unwrap_or_else(|| panic!("no button named {name:?}"))
}

fn centre(node: &Node) -> Pos2 {
    let b = node.bounds.expect("the control has bounds");
    Rect::from_min_max(
        Pos2::new(b.x0 as f32, b.y0 as f32),
        Pos2::new(b.x1 as f32, b.y1 as f32),
    )
    .center()
}

fn keys(h: &Harness, clip: ClipId) -> Vec<u8> {
    match &h.shell.editor().document().clip(clip).unwrap().data {
        ClipData::Midi(midi) => bl_shell::clip_view::notes_in_order(midi)
            .iter()
            .map(|note| note.key)
            .collect(),
        ClipData::Audio(_) => panic!("the clip is MIDI"),
    }
}

fn steps(h: &Harness) -> usize {
    h.shell.editor().undo_history().len()
}

fn press(h: &mut Harness, at: Pos2) {
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
}

fn release(h: &mut Harness, at: Pos2) {
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
}

/// Moves the pointer up by `points` in eight frames, as a drag does.
fn drag_up(h: &mut Harness, from: Pos2, points: f32) {
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        h.frame(vec![Event::PointerMoved(
            from + egui::vec2(0.0, -points * t),
        )]);
    }
}

#[test]
fn the_selector_row_shows_the_three_panels_and_a_click_shows_one() {
    // Spec: midi-tools R1; L-CLIP-003. A new Clip View shows Pitch and Time
    // Utilities; clicking a button shows its panel.
    let (mut h, _) = set();
    let all = nodes(&mut h);
    assert_eq!(radio(&all, "Pitch and Time Utilities").toggled, Some(true));
    for name in ["MIDI Transformation Tools", "MIDI Generative Tools"] {
        assert_eq!(radio(&all, name).toggled, Some(false), "{name}");
    }
    assert!(Harness::node(&all, "Main Properties").disabled);
    let steps = steps(&h);
    h.click(centre(radio(&all, "MIDI Transformation Tools")));
    assert_eq!(h.shell.clip_view().midi_tools().panel(), Panel::Transform);
    let all = nodes(&mut h);
    assert_eq!(radio(&all, "MIDI Transformation Tools").toggled, Some(true));
    h.click(centre(radio(&all, "MIDI Generative Tools")));
    assert_eq!(h.shell.clip_view().midi_tools().panel(), Panel::Generate);
    assert_eq!(self::steps(&h), steps, "showing a panel is no undo step");
}

#[test]
fn a_tool_panel_has_the_selector_the_controls_auto_apply_and_reset() {
    // Spec: midi-tools R2, R6; L-CLIP-007.
    let (mut h, _) = set();
    h.shell
        .run("clip_view.show_panel", json!({ "panel": "transform" }))
        .unwrap();
    let all = nodes(&mut h);
    assert_eq!(
        Harness::node(&all, "Transformation Selector").value,
        "Test Transpose"
    );
    for control in [
        "Degrees",
        "Pieces",
        "Spread",
        "Test Transpose Auto",
        "Transform",
        "Reset",
    ] {
        Harness::node(&all, control);
    }
    assert!(
        Harness::node(&all, "Reset").disabled,
        "Reset is disabled at the defaults"
    );
    assert_eq!(
        Harness::node(&all, "Test Transpose Auto").toggled,
        Some(true)
    );
    h.shell
        .run("clip_view.show_panel", json!({ "panel": "generate" }))
        .unwrap();
    let all = nodes(&mut h);
    assert_eq!(Harness::node(&all, "Generator Selector").value, "Test Fill");
    for control in ["Key", "Division", "Length", "Generate"] {
        Harness::node(&all, control);
    }
    assert_eq!(Harness::node(&all, "Division").value, "1/16");
}

#[test]
fn the_apply_button_applies_the_tool_and_auto_switches_it() {
    // Spec: midi-tools R2, R18, R19. The Transform button applies once, with
    // Auto on or off.
    let (mut h, clip) = set();
    h.shell
        .run(
            "midi_tool.set_auto",
            json!({ "tool": "test_transpose", "auto": false }),
        )
        .unwrap();
    h.shell
        .run(
            "midi_tool.set",
            json!({ "tool": "test_transpose", "values": { "degrees": 12 } }),
        )
        .unwrap();
    h.shell
        .run("clip_view.show_panel", json!({ "panel": "transform" }))
        .unwrap();
    let all = nodes(&mut h);
    assert_eq!(
        Harness::node(&all, "Test Transpose Auto").toggled,
        Some(false)
    );
    assert!(
        !Harness::node(&all, "Reset").disabled,
        "the degrees are not at their default"
    );
    let before = steps(&h);
    h.click(centre(Harness::node(&all, "Transform")));
    assert_eq!(keys(&h, clip), [72, 74, 76]);
    assert_eq!(steps(&h), before + 1);
    // The Auto switch runs `midi_tool.set_auto`.
    let all = nodes(&mut h);
    h.click(centre(Harness::node(&all, "Test Transpose Auto")));
    let state: bl_shell::midi_tool_commands::MidiToolState =
        serde_json::from_value(h.shell.run("midi_tool.state", json!({})).unwrap()).unwrap();
    assert!(
        state
            .tools
            .iter()
            .find(|t| t.tool == "test_transpose")
            .unwrap()
            .auto
    );
}

#[test]
fn a_controls_drag_with_auto_on_is_one_undo_step_and_esc_puts_the_notes_back() {
    // Spec: midi-tools R22. The frames of a drag are `midi_tool.set`
    // gestures; the drag is one step named after the tool, and Esc puts the
    // notes back.
    let (mut h, clip) = set();
    h.shell
        .run("clip_view.show_panel", json!({ "panel": "transform" }))
        .unwrap();
    let all = nodes(&mut h);
    let degrees = centre(Harness::node(&all, "Degrees"));
    let before = steps(&h);
    let original = keys(&h, clip);

    // A drag released: one step, however many frames.
    press(&mut h, degrees);
    drag_up(&mut h, degrees, 8.0);
    release(&mut h, degrees + egui::vec2(0.0, -8.0));
    assert_ne!(keys(&h, clip), original, "the drag moved the notes");
    assert_eq!(steps(&h), before + 1, "however many frames, one undo step");
    assert_eq!(
        h.shell.editor().undo_history().next(),
        Some("Test Transpose"),
        "named after the tool"
    );
    let moved = keys(&h, clip);

    // A drag ended with Esc puts the notes back as they were before it.
    let all = nodes(&mut h);
    let degrees = centre(Harness::node(&all, "Degrees"));
    press(&mut h, degrees);
    drag_up(&mut h, degrees, 12.0);
    assert_ne!(keys(&h, clip), moved, "the drag is under way");
    h.frame(vec![Event::Key {
        key: Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    release(&mut h, degrees + egui::vec2(0.0, -12.0));
    assert_eq!(keys(&h, clip), moved, "Esc put the notes back");
    assert_eq!(steps(&h), before + 1, "and recorded nothing");
}

#[test]
fn the_info_view_shows_a_controls_name_and_description() {
    // Spec: midi-tools R7, info-view R4. While the pointer is over a control
    // the Info View shows the parameter's name and registered description.
    let (mut h, _) = set();
    h.shell
        .run("clip_view.show_panel", json!({ "panel": "transform" }))
        .unwrap();
    let all = nodes(&mut h);
    let degrees = centre(Harness::node(&all, "Degrees"));
    h.frame(vec![Event::PointerMoved(degrees)]);
    h.settle();
    h.frame(vec![Event::PointerMoved(degrees)]);
    let content = h.main.info_view_content();
    assert_eq!(content.title, "Degrees");
    assert_eq!(content.body, "How many scale degrees each note moves.");
}

#[test]
fn cmd_enter_and_the_edit_menu_apply_the_chosen_tool() {
    // Spec: midi-tools R19; S-NOTE-033, M-EDIT-023. Cmd+Enter in the note
    // editor and Edit ▸ Apply Transformation/Generator run `midi_tool.apply`;
    // the item is disabled while no MIDI clip shows.
    let keymap = bl_ui::keymap::Keymap::live12();
    let entry = keymap.entry_for("S-NOTE-033").unwrap();
    assert_eq!(entry.status, bl_ui::keymap::Status::Bound);
    assert_eq!(entry.commands[0].id, "midi_tool.apply");
    let item = |h: &Harness| {
        let menus = bl_shell::menus::menu_bar(bl_shell::menus::MenuPlatform::current());
        let item = menus
            .iter()
            .flat_map(bl_shell::menus::Menu::walk)
            .find(|item| item.row == "M-EDIT-023")
            .unwrap();
        bl_ui::menus::item_state(
            item,
            &h.shell,
            &h.keymap,
            bl_ui::keymap::Platform::current(),
        )
    };
    let mut none = Harness::with_title_bar(Window::Main);
    none.settle();
    assert!(!item(&none).enabled, "no MIDI clip shows");

    let (mut h, clip) = set();
    h.shell.run("focus.clip_view", json!({})).unwrap();
    h.settle();
    let state = item(&h);
    assert!(state.enabled);
    assert_eq!(
        state.invocation.as_ref().map(|i| i.id.as_str()),
        Some("midi_tool.apply")
    );
    // The key reaches it: the three notes selected move by the degrees the
    // tool keeps.
    h.shell
        .run(
            "midi_tool.set",
            json!({ "tool": "test_transpose", "values": { "degrees": 5 } }),
        )
        .unwrap();
    h.shell.run("midi_tool.close", json!({})).unwrap();
    h.shell
        .run(
            "midi_tool.set_auto",
            json!({ "tool": "test_transpose", "auto": false }),
        )
        .unwrap();
    let before = keys(&h, clip);
    h.key(Key::Enter, common::command());
    assert_ne!(keys(&h, clip), before, "Cmd+Enter applied the tool");
}

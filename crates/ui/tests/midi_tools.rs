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
use egui::{Event, FullOutput, Id, Key, Modifiers, PointerButton, Pos2, Rect, Shape};
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
    // These tests drive the framework through the test tools.
    for tool in ["test_fill", "test_transpose"] {
        h.shell
            .run("midi_tool.choose", json!({ "tool": tool }))
            .unwrap();
    }
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

/// Every shape of a frame, the nested ones flattened.
fn shapes(output: &FullOutput) -> Vec<Shape> {
    fn walk(shape: &Shape, out: &mut Vec<Shape>) {
        match shape {
            Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
            other => out.push(other.clone()),
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

/// Opens the Transformation Selector and reads which tools its list marks
/// with the accent LED, then closes it.
fn marked_in_the_list(h: &mut Harness) -> Vec<String> {
    let all = nodes(h);
    let selector = Harness::node(&all, "Transformation Selector");
    let b = selector.bounds.expect("the selector has bounds");
    let chooser = Rect::from_min_max(
        Pos2::new(b.x0 as f32, b.y0 as f32),
        Pos2::new(b.x1 as f32, b.y1 as f32),
    );
    h.accesskit(
        Id::new(("chooser", "tool-selector-transformation")),
        egui::accesskit::Action::Click,
    );
    // The list fades in.
    for _ in 0..12 {
        h.frame(vec![]);
    }
    let found = shapes(&h.frame(vec![]));
    let accent = h.theme.colors.accent.0;
    let leds: Vec<Pos2> = found
        .iter()
        .filter_map(|shape| match shape {
            Shape::Circle(c) if c.fill == accent => Some(c.center),
            _ => None,
        })
        .collect();
    let mut marked = Vec::new();
    for shape in &found {
        let Shape::Text(text) = shape else { continue };
        let row = text.galley.rect.translate(text.pos.to_vec2());
        let label = text.galley.text();
        // The list's rows lie below the chooser's box.
        if row.top() < chooser.bottom() || !label.starts_with("Test ") {
            continue;
        }
        if leds.iter().any(|led| {
            (led.y - row.center().y).abs() < 4.0
                && (chooser.left()..=chooser.right()).contains(&led.x)
        }) {
            marked.push(label.to_owned());
        }
    }
    h.key(Key::Escape, Modifiers::NONE);
    h.settle();
    marked.sort();
    marked
}

#[test]
fn the_selector_marks_the_tools_in_the_open_chain() {
    // Spec: midi-tools R29. The list marks each tool of the open chain, and
    // none once the chain is closed.
    let (mut h, _) = set();
    h.shell
        .run("clip_view.show_panel", json!({ "panel": "transform" }))
        .unwrap();
    assert!(marked_in_the_list(&mut h).is_empty(), "no chain is open");
    h.shell
        .run(
            "midi_tool.apply",
            json!({ "tool": "test_transpose", "values": { "degrees": 1 } }),
        )
        .unwrap();
    assert_eq!(marked_in_the_list(&mut h), ["Test Transpose"]);
    h.shell
        .run(
            "midi_tool.apply",
            json!({ "tool": "test_shift", "values": { "sixteenths": 1 } }),
        )
        .unwrap();
    assert_eq!(marked_in_the_list(&mut h), ["Test Shift", "Test Transpose"]);
    h.shell.run("midi_tool.close", json!({})).unwrap();
    assert!(marked_in_the_list(&mut h).is_empty(), "the chain closed");
}

/// The values of one tool, as `midi_tool.state` reports them.
fn values(h: &mut Harness, tool: &str) -> serde_json::Value {
    let state: bl_shell::midi_tool_commands::MidiToolState =
        serde_json::from_value(h.shell.run("midi_tool.state", json!({})).unwrap()).unwrap();
    serde_json::Value::Object(
        state
            .tools
            .into_iter()
            .find(|t| t.tool == tool)
            .unwrap()
            .values,
    )
}

fn choose(h: &mut Harness, tool: &str) {
    h.shell
        .run("midi_tool.choose", json!({ "tool": tool }))
        .unwrap();
    h.shell
        .run("midi_tool.set_auto", json!({ "tool": tool, "auto": false }))
        .unwrap();
    h.shell
        .run("clip_view.show_panel", json!({ "panel": "transform" }))
        .unwrap();
}

#[test]
fn chops_pattern_and_emphasis_toggles_each_click_a_set_and_gaps_shows_a_dash() {
    // Spec: midi-transformations R8, R43. Each click on a toggle is a
    // `midi_tool.set`; Gaps shows a dash once the pattern is not what it
    // writes; an element with Emphasis on has its Pattern toggle unavailable.
    let (mut h, _) = set();
    choose(&mut h, "chop");
    h.shell
        .run(
            "midi_tool.set",
            json!({ "tool": "chop", "values": { "parts": 4 } }),
        )
        .unwrap();
    let all = nodes(&mut h);
    assert_eq!(Harness::node(&all, "Gaps").value, "0");
    for at in 1..=4 {
        assert_eq!(
            Harness::node(&all, &format!("Pattern {at}")).toggled,
            Some(true)
        );
    }
    assert!(
        all.iter().all(|n| n.label != "Pattern 5"),
        "one toggle for each of the parts"
    );
    let steps = steps(&h);
    h.click(centre(Harness::node(&all, "Pattern 2")));
    assert_eq!(
        values(&mut h, "chop")["pattern"][1],
        json!(false),
        "the click set the pattern"
    );
    assert_eq!(self::steps(&h), steps, "settings are no undo step");
    let all = nodes(&mut h);
    assert_eq!(Harness::node(&all, "Pattern 2").toggled, Some(false));
    assert_eq!(Harness::node(&all, "Gaps").value, "\u{2013}");
    // Emphasis on the first element makes its Pattern toggle unavailable.
    h.click(centre(Harness::node(&all, "Emphasis 1")));
    assert_eq!(values(&mut h, "chop")["emphasis"][0], json!(true));
    let all = nodes(&mut h);
    assert!(Harness::node(&all, "Pattern 1").disabled);
    assert!(!Harness::node(&all, "Pattern 3").disabled);
    // Setting Gaps writes the pattern afresh and the number shows again.
    h.shell
        .run(
            "midi_tool.set",
            json!({ "tool": "chop", "values": { "gaps": 1 } }),
        )
        .unwrap();
    let all = nodes(&mut h);
    assert_eq!(Harness::node(&all, "Gaps").value, "1");
    assert_eq!(Harness::node(&all, "Pattern 2").toggled, Some(false));
    assert_eq!(Harness::node(&all, "Pattern 3").toggled, Some(true));
}

#[test]
fn quantize_has_its_five_controls_and_triplets_wait_for_a_grid() {
    // Spec: midi-transformations R18. Triplets are unavailable with Current
    // Grid.
    let (mut h, _) = set();
    choose(&mut h, "quantize");
    let all = nodes(&mut h);
    for control in ["Start", "End", "Grid", "Triplets", "Amount"] {
        Harness::node(&all, control);
    }
    // The Clip View's own editing grid is named Grid too.
    assert!(
        all.iter()
            .any(|n| n.label == "Grid" && n.value == "Current Grid"),
        "the tool's chooser"
    );
    assert!(Harness::node(&all, "Triplets").disabled);
    h.shell
        .run(
            "midi_tool.set",
            json!({ "tool": "quantize", "values": { "grid": "1_8" } }),
        )
        .unwrap();
    let all = nodes(&mut h);
    assert!(!Harness::node(&all, "Triplets").disabled);
    h.click(centre(Harness::node(&all, "Triplets")));
    assert_eq!(values(&mut h, "quantize")["triplets"], json!(true));
}

#[test]
fn cmd_shift_u_and_the_edit_menu_show_the_transform_panel_with_quantize_chosen() {
    // Spec: midi-transformations R21; S-NOTE-012, M-EDIT-036. Neither changes
    // notes or is an undo step.
    let keymap = bl_ui::keymap::Keymap::live12();
    let entry = keymap.entry_for("S-NOTE-012").unwrap();
    assert_eq!(entry.status, bl_ui::keymap::Status::Bound);
    assert_eq!(entry.commands[0].id, "clip_view.quantize_settings");
    let (mut h, clip) = set();
    h.shell.run("focus.clip_view", json!({})).unwrap();
    h.settle();
    let menus = bl_shell::menus::menu_bar(bl_shell::menus::MenuPlatform::current());
    let item = menus
        .iter()
        .flat_map(bl_shell::menus::Menu::walk)
        .find(|item| item.row == "M-EDIT-036")
        .unwrap();
    let state = bl_ui::menus::item_state(
        item,
        &h.shell,
        &h.keymap,
        bl_ui::keymap::Platform::current(),
    );
    assert!(state.enabled, "the item works with the note editor focused");
    assert_eq!(
        state.invocation.as_ref().map(|i| i.id.as_str()),
        Some("clip_view.quantize_settings")
    );
    let (steps, notes) = (steps(&h), keys(&h, clip));
    assert_eq!(h.shell.clip_view().midi_tools().panel(), Panel::Utilities);
    h.key(Key::U, common::command() | Modifiers::SHIFT);
    assert_eq!(h.shell.clip_view().midi_tools().panel(), Panel::Transform);
    let all = nodes(&mut h);
    assert_eq!(
        Harness::node(&all, "Transformation Selector").value,
        "Quantize"
    );
    assert_eq!((self::steps(&h), keys(&h, clip)), (steps, notes));
}

#[test]
fn lfo_draws_its_curve_and_random_shows_a_reseed_button() {
    // Spec: midi-transformations R39, R43. The curve is drawn from the
    // tool's settings; Reseed comes with the Random wave and runs
    // `midi_tool.reseed`, which gives the open application a new seed.
    let (mut h, _) = set();
    choose(&mut h, "lfo");
    let all = nodes(&mut h);
    Harness::node(&all, "LFO curve");
    assert!(all.iter().all(|n| n.label != "Reseed"));
    h.shell
        .run(
            "midi_tool.set",
            json!({ "tool": "lfo", "values": { "shape_type": "random", "amount": 12 } }),
        )
        .unwrap();
    h.shell
        .run("midi_tool.apply", json!({ "tool": "lfo", "seed": 3 }))
        .unwrap();
    let all = nodes(&mut h);
    let steps = steps(&h);
    h.click(centre(Harness::node(&all, "Reseed")));
    let state: bl_shell::midi_tool_commands::MidiToolState =
        serde_json::from_value(h.shell.run("midi_tool.state", json!({})).unwrap()).unwrap();
    assert_ne!(state.application.unwrap().seed, 3);
    assert_eq!(self::steps(&h), steps + 1, "the new run is an undo step");
}

/// Chooses a generator and shows the Generate panel, with Auto as given.
fn generator(h: &mut Harness, tool: &str, auto: bool) {
    h.shell
        .run("midi_tool.choose", json!({ "tool": tool }))
        .unwrap();
    h.shell
        .run("midi_tool.set_auto", json!({ "tool": tool, "auto": auto }))
        .unwrap();
    h.shell
        .run("clip_view.show_panel", json!({ "panel": "generate" }))
        .unwrap();
}

fn set_values(h: &mut Harness, tool: &str, values: serde_json::Value) {
    h.shell
        .run("midi_tool.set", json!({ "tool": tool, "values": values }))
        .unwrap();
}

fn type_text(h: &mut Harness, text: &str) {
    h.frame(vec![Event::Text(text.to_owned())]);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
}

#[test]
fn rhythm_draws_its_step_pattern_names_its_pitch_and_steps_accent_offset_with_arrows() {
    // Spec: midi-generators R3, R4, R5. The step pattern shows the steps that
    // hold notes, a pitch shows by name and takes a typed name or number, and
    // Accent Offset moves with two arrows that stop at its ends.
    let (mut h, _) = set();
    generator(&mut h, "rhythm", false);
    let all = nodes(&mut h);
    assert_eq!(
        Harness::node(&all, "Step pattern").value,
        "steps 1, 5, 9, 13"
    );
    set_values(&mut h, "rhythm", json!({ "steps": 8, "density": 3 }));
    let all = nodes(&mut h);
    assert_eq!(Harness::node(&all, "Step pattern").value, "steps 1, 4, 7");
    // The pitch reads C3; a typed name or number sets it.
    assert_eq!(Harness::node(&all, "Pitch").value, "C3");
    h.click(centre(Harness::node(&all, "Pitch")));
    type_text(&mut h, "d#3");
    assert_eq!(values(&mut h, "rhythm")["pitch"], 63);
    type_text(&mut h, "72");
    assert_eq!(values(&mut h, "rhythm")["pitch"], 72);
    let all = nodes(&mut h);
    assert_eq!(Harness::node(&all, "Pitch").value, "C4");
    // A name that isn't a key changes nothing, and Esc drops the entry.
    type_text(&mut h, "H9");
    assert_eq!(values(&mut h, "rhythm")["pitch"], 72);
    h.frame(vec![Event::Text("6".to_owned())]);
    h.key(Key::Escape, Modifiers::NONE);
    h.key(Key::Enter, Modifiers::NONE);
    assert_eq!(values(&mut h, "rhythm")["pitch"], 72);
    // Accent Offset: arrows stop at 0 and at Density − 1.
    let all = nodes(&mut h);
    assert!(Harness::node(&all, "<").disabled, "the offset is 0");
    for expected in [1, 2] {
        let all = nodes(&mut h);
        h.click(centre(Harness::node(&all, ">")));
        assert_eq!(values(&mut h, "rhythm")["accent_offset"], expected);
    }
    let all = nodes(&mut h);
    assert!(Harness::node(&all, ">").disabled, "Density − 1 is 2");
    assert!(!Harness::node(&all, "<").disabled);
}

/// The slider's x for `key`, and its centre line.
fn slider_at(h: &mut Harness, key: f32) -> Pos2 {
    let all = nodes(h);
    let b = Harness::node(&all, "Pitch Range slider")
        .bounds
        .expect("the slider has bounds");
    let (left, right) = (b.x0 as f32, b.x1 as f32);
    Pos2::new(
        left + (key + 0.5) / 128.0 * (right - left),
        ((b.y0 + b.y1) / 2.0) as f32,
    )
}

fn range(h: &mut Harness, tool: &str) -> serde_json::Value {
    values(h, tool)["pitch_range"].clone()
}

#[test]
fn seeds_pitch_range_slider_moves_a_handle_or_both_and_merges_handles() {
    // Spec: midi-generators R8. Dragging a handle moves that end, the bar
    // between them moves both (Key Track), the handles merge where they meet,
    // and a drag is one undo step that Esc puts back.
    let (mut h, _) = set();
    generator(&mut h, "seed", true);
    let before = steps(&h);
    let (low, to) = (slider_at(&mut h, 48.0), slider_at(&mut h, 60.0));
    press(&mut h, low);
    h.frame(vec![Event::PointerMoved(to)]);
    release(&mut h, to);
    assert_eq!(range(&mut h, "seed"), json!({ "low": 60, "high": 72 }));
    assert_eq!(steps(&h), before + 1, "one undo step");
    // The bar between the handles moves both, keeping the width.
    let (from, to) = (slider_at(&mut h, 66.0), slider_at(&mut h, 78.0));
    press(&mut h, from);
    h.frame(vec![Event::PointerMoved(to)]);
    release(&mut h, to);
    assert_eq!(range(&mut h, "seed"), json!({ "low": 72, "high": 84 }));
    // Pulling one handle to the other merges them, and they part again the
    // way the pointer pulls.
    let (high, to) = (slider_at(&mut h, 84.0), slider_at(&mut h, 40.0));
    press(&mut h, high);
    h.frame(vec![Event::PointerMoved(to)]);
    release(&mut h, to);
    assert_eq!(range(&mut h, "seed"), json!({ "low": 72, "high": 72 }));
    let (merged, to) = (slider_at(&mut h, 72.0), slider_at(&mut h, 80.0));
    press(&mut h, merged);
    h.frame(vec![Event::PointerMoved(to)]);
    release(&mut h, to);
    assert_eq!(range(&mut h, "seed"), json!({ "low": 72, "high": 80 }));
    // Esc puts a drag back.
    let kept = steps(&h);
    let (from, to) = (slider_at(&mut h, 76.0), slider_at(&mut h, 90.0));
    press(&mut h, from);
    h.frame(vec![Event::PointerMoved(to)]);
    h.frame(vec![Event::Key {
        key: Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    release(&mut h, to);
    assert_eq!(range(&mut h, "seed"), json!({ "low": 72, "high": 80 }));
    assert_eq!(steps(&h), kept, "and recorded nothing");
    // The range also reads as two pitches by name.
    let all = nodes(&mut h);
    assert_eq!(Harness::node(&all, "Pitch Range low").value, "C4");
    assert_eq!(Harness::node(&all, "Pitch Range high").value, "G#4");
}

#[test]
fn shapes_levels_display_draws_levels_and_the_preset_reads_user() {
    // Spec: midi-generators R11. Pressing and dragging across the display
    // sets the levels under the pointer, the preset chooser then reads User,
    // and the drag is one undo step that Esc puts back.
    let (mut h, _) = set();
    generator(&mut h, "shape", true);
    let all = nodes(&mut h);
    let b = Harness::node(&all, "Shape Levels").bounds.expect("bounds");
    let (left, right, top, bottom) = (b.x0 as f32, b.x1 as f32, b.y0 as f32, b.y1 as f32);
    let each = (right - left) / 16.0;
    let at = |column: f32, level: f32| {
        Pos2::new(
            left + (column + 0.5) * each,
            bottom - level * (bottom - top),
        )
    };
    assert_eq!(values(&mut h, "shape")["preset"], "rise");
    let before = steps(&h);
    press(&mut h, at(2.0, 0.5));
    h.frame(vec![Event::PointerMoved(at(4.0, 0.5))]);
    h.frame(vec![Event::PointerMoved(at(6.0, 0.5))]);
    release(&mut h, at(6.0, 0.5));
    let now = values(&mut h, "shape");
    assert_eq!(now["preset"], "user");
    let levels: Vec<f64> = now["levels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_f64().unwrap())
        .collect();
    for column in 2..=6 {
        assert!((levels[column] - 0.5).abs() < 0.02, "{column}: {levels:?}");
    }
    assert!((levels[0] - 0.0).abs() < 1e-9 && (levels[15] - 1.0).abs() < 1e-9);
    assert_eq!(steps(&h), before + 1, "one undo step");
    // Choosing a preset writes its levels again.
    set_values(&mut h, "shape", json!({ "preset": "fall" }));
    assert!((values(&mut h, "shape")["levels"][0].as_f64().unwrap() - 1.0).abs() < 1e-9);
    // Esc puts a drag back.
    let kept = (steps(&h), values(&mut h, "shape"));
    press(&mut h, at(1.0, 0.1));
    h.frame(vec![Event::PointerMoved(at(3.0, 0.1))]);
    h.frame(vec![Event::Key {
        key: Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    release(&mut h, at(3.0, 0.1));
    assert_eq!((steps(&h), values(&mut h, "shape")), kept);
}

#[test]
fn euclideans_tabs_have_the_voices_rotations_pitches_arrows_and_randomise_button() {
    // Spec: midi-generators R14, R16. The Pattern tab has a switch and a
    // Rotation for each voice and the button that sets every Rotation at
    // random; the Voices tab each voice's pitch and Velocity and the arrows
    // that move every pitch together.
    let (mut h, _) = set();
    // The tabs are taller than the Clip View is by default.
    h.shell.run("view.expand_clip_view", json!({})).unwrap();
    generator(&mut h, "euclidean", false);
    let all = nodes(&mut h);
    assert_eq!(radio(&all, "Pattern").toggled, Some(true));
    assert_eq!(Harness::node(&all, "Voice 1").toggled, Some(true));
    assert_eq!(Harness::node(&all, "Voice 2").toggled, Some(false));
    assert_eq!(
        Harness::node(&all, "Voice 1 pattern").value,
        "steps 1, 5, 9, 13"
    );
    // A voice's switch is a `midi_tool.set`.
    h.click(centre(Harness::node(&all, "Voice 2")));
    assert_eq!(values(&mut h, "euclidean")["voice_2"], json!(true));
    // The button sets every Rotation, from 0 to Steps − 1.
    let all = nodes(&mut h);
    h.click(centre(Harness::node(&all, "Randomize Rotations")));
    let now = values(&mut h, "euclidean");
    let rotations: Vec<u64> = (1..=4)
        .map(|voice| now[format!("rotation_{voice}")].as_u64().unwrap())
        .collect();
    assert!(rotations.iter().all(|r| *r < 16), "{rotations:?}");
    assert!(rotations.iter().any(|r| *r > 0), "{rotations:?}");
    let all = nodes(&mut h);
    assert_eq!(
        Harness::node(&all, "Voice 1 Rotation").value,
        rotations[0].to_string()
    );
    // The Voices tab: pitches by name, then the arrows move all four.
    h.click(centre(radio(&all, "Voices")));
    let all = nodes(&mut h);
    assert_eq!(radio(&all, "Voices").toggled, Some(true));
    for (voice, name) in [(1, "C1"), (2, "D1"), (3, "F#1"), (4, "A#1")] {
        assert_eq!(
            Harness::node(&all, &format!("Voice {voice} Pitch")).value,
            name
        );
    }
    h.click(centre(Harness::node(&all, ">")));
    let now = values(&mut h, "euclidean");
    let pitches: Vec<u64> = (1..=4)
        .map(|voice| now[format!("pitch_{voice}")].as_u64().unwrap())
        .collect();
    assert_eq!(pitches, [37, 39, 43, 47]);
    let all = nodes(&mut h);
    h.click(centre(Harness::node(&all, "<")));
    assert_eq!(values(&mut h, "euclidean")["pitch_1"], 36);
}

/// The Chord Selector Pad's diagram for bank entry `entry` of the first page
/// (three across, as the panel draws them), at its centre.
fn diagram(h: &mut Harness, entry: usize) -> Pos2 {
    let all = nodes(h);
    let b = Harness::node(&all, "Chord Selector Pad")
        .bounds
        .expect("the pad has bounds");
    let (column, row) = ((entry % 3) as f32, (entry / 3) as f32);
    Pos2::new(
        b.x0 as f32 + column * 32.0 + 15.0,
        b.y0 as f32 + row * 26.0 + 12.0,
    )
}

fn chord(h: &mut Harness) -> u64 {
    values(h, "stacks")["chord"].as_u64().unwrap()
}

#[test]
fn stacks_pad_chooses_by_click_and_drag_and_cmd_up_and_down_and_names_the_chord_on_hover() {
    // Spec: midi-generators R21; S-NOTE-052. Clicking a diagram or dragging
    // across the pad chooses its entry (a drag is one undo step that Esc puts
    // back); with the pad focused Cmd+Up and Cmd+Down choose the entry before
    // or after; hovering a diagram puts the chord's name and notes in the
    // Status Bar.
    let (mut h, _) = set();
    h.shell.run("view.expand_clip_view", json!({})).unwrap();
    generator(&mut h, "stacks", true);
    h.settle();
    assert_eq!(chord(&mut h), 0);
    // A click on the third diagram of the first row chooses entry 2, and with
    // Auto on the notes are written: the Ninth is five notes.
    let before = steps(&h);
    let at = diagram(&mut h, 2);
    h.click(at);
    h.settle();
    assert_eq!(chord(&mut h), 2);
    assert_eq!(steps(&h), before + 1);
    // Cmd+Up and Cmd+Down choose the entry before and after, stopping at the
    // ends of the bank.
    h.key(Key::ArrowDown, Modifiers::COMMAND);
    h.settle();
    assert_eq!(chord(&mut h), 3);
    h.key(Key::ArrowUp, Modifiers::COMMAND);
    h.key(Key::ArrowUp, Modifiers::COMMAND);
    h.settle();
    assert_eq!(chord(&mut h), 1);
    h.key(Key::ArrowUp, Modifiers::COMMAND);
    h.key(Key::ArrowUp, Modifiers::COMMAND);
    h.settle();
    assert_eq!(chord(&mut h), 0, "no wrap at the first entry");
    // Without Cmd, Up and Down leave the chord as it is.
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.settle();
    assert_eq!(chord(&mut h), 0);
    // A drag across the pad is one undo step and ends on the entry under the
    // pointer.
    let (from, to) = (diagram(&mut h, 0), diagram(&mut h, 5));
    let before = steps(&h);
    let (one, four) = (diagram(&mut h, 1), diagram(&mut h, 4));
    press(&mut h, from);
    h.frame(vec![Event::PointerMoved(one)]);
    h.frame(vec![Event::PointerMoved(four)]);
    h.frame(vec![Event::PointerMoved(to)]);
    release(&mut h, to);
    assert_eq!(chord(&mut h), 5);
    assert_eq!(steps(&h), before + 1, "one undo step");
    // Esc puts a drag back.
    let kept = (steps(&h), values(&mut h, "stacks"));
    let seven = diagram(&mut h, 7);
    press(&mut h, to);
    h.frame(vec![Event::PointerMoved(seven)]);
    h.frame(vec![Event::Key {
        key: Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    release(&mut h, seven);
    assert_eq!((steps(&h), values(&mut h, "stacks")), kept);
    // Hovering names the chord and its notes on the selected chord's root;
    // leaving the pad clears the hint.
    let major = diagram(&mut h, 9 - 9);
    h.shell
        .run(
            "midi_tool.set",
            json!({ "tool": "stacks", "values": { "chord_root": 60 } }),
        )
        .unwrap();
    h.frame(vec![Event::PointerMoved(major)]);
    h.settle();
    h.frame(vec![Event::PointerMoved(major)]);
    assert_eq!(h.shell.hint(), Some("Triad: C3 E3 G3"));
    h.frame(vec![Event::PointerMoved(Pos2::new(5.0, 5.0))]);
    h.settle();
    assert_eq!(h.shell.hint(), None);
}

#[test]
fn stacks_add_chord_and_delete_chord_and_the_progression_cells() {
    // Spec: midi-generators R17. Add Chord adds a chord after the selected
    // one and selects it, Delete Chord removes it (disabled for the only
    // chord), and clicking a cell of the progression selects that chord.
    let (mut h, _) = set();
    h.shell.run("view.expand_clip_view", json!({})).unwrap();
    generator(&mut h, "stacks", false);
    let all = nodes(&mut h);
    assert!(Harness::node(&all, "Delete Chord").disabled);
    assert_eq!(
        Harness::node(&all, "Chord 1: Triad on C3").toggled,
        Some(true)
    );
    h.click(centre(Harness::node(&all, "Add Chord")));
    let all = nodes(&mut h);
    h.click(centre(Harness::node(&all, "Add Chord")));
    let all = nodes(&mut h);
    assert_eq!(values(&mut h, "stacks")["selected"], 2);
    assert_eq!(
        values(&mut h, "stacks")["progression"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        Harness::node(&all, "Chord 3: Triad on C3").toggled,
        Some(true)
    );
    assert!(!Harness::node(&all, "Delete Chord").disabled);
    // A cell selects its chord and is no undo step.
    let before = steps(&h);
    h.click(centre(Harness::node(&all, "Chord 1: Triad on C3")));
    assert_eq!(values(&mut h, "stacks")["selected"], 0);
    assert_eq!(steps(&h), before);
    // Delete Chord removes the selected one.
    let all = nodes(&mut h);
    h.click(centre(Harness::node(&all, "Delete Chord")));
    assert_eq!(
        values(&mut h, "stacks")["progression"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    // The controls show the selected chord's settings and set them.
    let all = nodes(&mut h);
    assert_eq!(Harness::node(&all, "Chord Root").value, "C3");
    h.click(centre(Harness::node(&all, "Chord Root")));
    type_text(&mut h, "d3");
    assert_eq!(values(&mut h, "stacks")["chord_root"], 62);
    assert_eq!(
        values(&mut h, "stacks")["progression"][0]["root"],
        62,
        "the selected chord's root"
    );
    assert_eq!(values(&mut h, "stacks")["progression"][1]["root"], 60);
}

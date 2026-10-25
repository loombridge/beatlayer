//! The browser's Tuning section and tunings in the editor, headless (WP-6.10):
//! loading a tuning by double-click, Enter and drop; the section's controls
//! running their commands and reading their values; Delete on the name;
//! View ▸ Tuning and Navigate ▸ Tuning; the note editor's rows, their names
//! and the Status Bar's line; the scale controls a tuning hides; and
//! assistive technology reading each control. Spec: tunings R16, R26 to R30.

use crate::common;
use std::path::{Path, PathBuf};

use bl_model::{Tuning, TuningNote};
use bl_shell::browser::places::create_user_library;
use bl_shell::browser_commands::{self as ids};
use bl_shell::files::factory_default_set;
use bl_ui::arrangement::benchmark;
use bl_ui::layout::{Element, REFERENCE, ShellLayout};
use bl_ui::tuning_section::{name_id, triangle_id};
use common::{Harness, Node, Window};
use egui::accesskit::{Action, Role};
use egui::{Event, FullOutput, Id, Key, Modifiers, PointerButton, Pos2, pos2};
use serde_json::json;

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-ui-tuning-{}-{name}", std::process::id()));
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

/// A file dropped into the window, as the app's drag and drop gives egui.
#[derive(Debug)]
struct Dropped(PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|error| error.to_string())
    }
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/tunings")
}

/// The list's first row's top: the content pane starts at y 84, the Filter
/// View fills it to y 204, and the header row sits at y 208 (L-BRW-007,
/// L-BRW-009).
const LIST_TOP: f32 = 227.0;

fn row(index: usize) -> Pos2 {
    pos2(160.0, LIST_TOP + 17.0 * index as f32 + 8.5)
}

/// A harness whose User Library holds a 19-step and a 12-note tuning file,
/// the Tunings label chosen.
fn harness(scratch: &Scratch) -> Harness {
    let root = scratch.0.join("User Library");
    create_user_library(&root).unwrap();
    std::fs::copy(
        fixtures().join("equal-19.scl"),
        root.join("Tunings").join("Nineteen.scl"),
    )
    .unwrap();
    let mut h = Harness::manual_with_title_bar(Window::Main);
    h.shell.set_user_library(Some(root));
    h.shell
        .run(ids::OPEN_PLACE, json!({ "label": "tunings" }))
        .unwrap();
    h.shell
        .run(ids::SET_PREVIEW, json!({ "on": false }))
        .unwrap();
    h.wait("the index", |h| {
        !h.shell.browser_view().index_state().indexing
    });
    h.settle();
    h
}

fn names(h: &Harness) -> Vec<String> {
    let view = h.shell.browser_view();
    (0..view.row_count())
        .map(|i| view.row(i).unwrap().name)
        .collect()
}

fn index_of(h: &Harness, name: &str) -> usize {
    names(h)
        .iter()
        .position(|n| n == name)
        .unwrap_or_else(|| panic!("{name} is listed"))
}

fn tuning(h: &Harness) -> Option<Tuning> {
    h.shell.editor().document().settings().tuning.clone()
}

fn double_click(h: &mut Harness, at: Pos2) {
    // Long enough after the last click that this isn't a triple click.
    h.time += 1.0;
    h.frame(vec![Event::PointerMoved(at)]);
    for _ in 0..2 {
        for pressed in [true, false] {
            h.frame(vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            }]);
        }
    }
    h.settle();
}

fn nodes(h: &mut Harness) -> Vec<Node> {
    Harness::nodes(&h.settle())
}

fn centre(node: &Node) -> Pos2 {
    let b = node.bounds.expect("bounds");
    pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
}

fn find<'a>(nodes: &'a [Node], role: Role, label: &str) -> &'a Node {
    nodes
        .iter()
        .find(|n| n.role == role && n.label == label)
        .unwrap_or_else(|| panic!("no {role:?} labelled {label:?}"))
}

fn has(nodes: &[Node], label: &str) -> bool {
    nodes.iter().any(|n| n.label == label)
}

// Spec: tunings R27. A double-click on a tuning loads it and shows the
// Tuning section; Enter loads the selected tuning; a file dropped on the
// section, from the operating system or from the list, loads too.
#[test]
fn a_double_click_enter_and_a_drop_load_a_tuning() {
    let scratch = Scratch::new("load");
    let mut h = harness(&scratch);
    assert!(!h.shell.tuning_section_shown());
    assert!(!has(&nodes(&mut h), "Tuning"), "hidden in a new set");

    // A double-click on one of Beatlayer's own.
    let at = index_of(&h, "Pythagorean");
    double_click(&mut h, row(at));
    assert_eq!(tuning(&h).map(|t| t.name), Some("Pythagorean".to_owned()));
    assert!(h.shell.tuning_section_shown());
    assert!(has(&nodes(&mut h), "Tuning"), "the section shows");

    // Enter on the selected tuning, a file in the User Library found by a
    // search.
    h.shell
        .run(
            ids::SEARCH,
            json!({ "query": "nineteen", "label": "tunings" }),
        )
        .unwrap();
    h.settle();
    assert_eq!(names(&h), ["Nineteen"]);
    let listed = nodes(&mut h);
    let item = listed
        .iter()
        .find(|n| n.role == Role::ListItem && n.label.starts_with("Nineteen"))
        .expect("the row is listed");
    h.click(centre(item));
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    let loaded = tuning(&h).expect("a tuning");
    assert_eq!((loaded.name.as_str(), loaded.degrees()), ("Nineteen", 19));

    // A file dropped on the section from the operating system.
    h.shell.run("tuning.remove", json!({})).unwrap();
    let all = nodes(&mut h);
    let over = centre(find(&all, Role::Button, "Tuning"));
    h.frame(vec![Event::PointerMoved(over)]);
    h.dropped
        .push(std::sync::Arc::new(Dropped(fixtures().join("just-12.scl"))));
    h.settle();
    assert_eq!(tuning(&h).map(|t| t.name), Some("just-12".to_owned()));
    // A file that is not a tuning is left alone.
    h.shell.run("tuning.remove", json!({})).unwrap();
    h.frame(vec![Event::PointerMoved(over)]);
    h.dropped
        .push(std::sync::Arc::new(Dropped(PathBuf::from("/a/Kick.wav"))));
    h.settle();
    assert_eq!(tuning(&h), None);

    // A tuning dragged from the list and dropped on the section.
    h.shell
        .run(
            ids::SEARCH,
            json!({ "query": "vallotti", "label": "tunings" }),
        )
        .unwrap();
    h.settle();
    let listed = nodes(&mut h);
    let item = listed
        .iter()
        .find(|n| n.role == Role::ListItem && n.label.starts_with("Vallotti"))
        .expect("the row is listed");
    let from = centre(item);
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    for step in 1..=6 {
        let t = step as f32 / 6.0;
        h.frame(vec![Event::PointerMoved(from + (over - from) * t)]);
    }
    h.frame(vec![Event::PointerButton {
        pos: over,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
    assert_eq!(tuning(&h).map(|t| t.name), Some("Vallotti".to_owned()));
}

/// A harness with the 19-step tuning loaded and the section showing.
fn loaded(scratch: &Scratch) -> Harness {
    let mut h = harness(scratch);
    let at = index_of(&h, "19 Equal Steps");
    double_click(&mut h, row(at));
    assert_eq!(tuning(&h).map(|t| t.degrees()), Some(19));
    h
}

// Spec: tunings R26. Hovering the loaded tuning's name, and selecting a
// tuning in the list, show its description in the Info View.
#[test]
fn the_info_view_describes_a_selected_tuning_and_the_loaded_one() {
    let scratch = Scratch::new("info");
    let mut h = harness(&scratch);
    let at = index_of(&h, "Pythagorean");
    h.click(row(at));
    h.settle();
    let info = h.main.info_view_content().clone();
    assert_eq!(info.title, "Pythagorean");
    assert!(
        info.body.contains("12 notes to the pseudo-octave"),
        "{}",
        info.body
    );
    assert!(
        info.body.contains("Pseudo-octave: 1200.00 cents"),
        "{}",
        info.body
    );
    // The loaded one, under the pointer.
    let mut h = loaded(&scratch);
    let all = nodes(&mut h);
    h.frame(vec![Event::PointerMoved(centre(find(
        &all,
        Role::Button,
        "Tuning",
    )))]);
    h.settle();
    let info = h.main.info_view_content().clone();
    assert_eq!(info.title, "19 Equal Steps");
    assert!(
        info.body.contains("19 notes to the pseudo-octave"),
        "{}",
        info.body
    );
    // A file R3 refuses shows the error.
    let bad = scratch.0.join("Broken.scl");
    std::fs::copy(fixtures().join("no-pitches.scl"), &bad).unwrap();
    h.shell
        .run(ids::SELECT, json!({ "paths": [bad.to_string_lossy()] }))
        .unwrap();
    h.frame(vec![Event::PointerMoved(pos2(700.0, 400.0))]);
    h.settle();
    assert!(h.main.info_view_content().body.contains("no pitches"));
}

// Spec: tunings R28. The section's controls read their values and run R7's
// and R8's commands.
#[test]
fn the_sections_controls_run_their_commands_and_read_their_values() {
    let scratch = Scratch::new("controls");
    let mut h = loaded(&scratch);
    let read = |h: &mut Harness| nodes(h);
    let all = read(&mut h);
    assert_eq!(find(&all, Role::Button, "Tuning").value, "19 Equal Steps");
    let before = tuning(&h).unwrap();
    assert_eq!(find(&all, Role::SpinButton, "Octave").value, "3");
    assert_eq!(find(&all, Role::ComboBox, "Note").value, "0");
    assert_eq!(
        find(&all, Role::SpinButton, "Ref. Pitch").value,
        "261.626 Hz"
    );
    assert_eq!(
        find(&all, Role::SpinButton, "Lowest Note").value,
        before.note_name(before.range.lowest)
    );
    assert_eq!(
        find(&all, Role::SpinButton, "Highest Note").value,
        before.note_name(before.range.highest)
    );

    // Octave steps the reference's octave, which moves nothing.
    let step = |h: &mut Harness, salt: &str, action: Action| {
        h.accesskit(Id::new(("number-box", salt)), action);
        h.settle();
    };
    step(&mut h, "tuning-octave", Action::Increment);
    let after = tuning(&h).unwrap();
    assert_eq!(after.reference.octave, before.reference.octave + 1);
    assert!(
        (after.key_frequency(70).unwrap() - before.key_frequency(70).unwrap()).abs() < 1e-9,
        "naming another note as the reference moves nothing"
    );
    // Ref. Pitch moves every pitch by one ratio.
    step(&mut h, "tuning-ref-pitch", Action::Increment);
    let moved = tuning(&h).unwrap();
    assert!((moved.reference.frequency - after.reference.frequency - 0.1).abs() < 1e-9);
    // Note chooses the reference's degree.
    h.accesskit(Id::new(("chooser", "tuning-note")), Action::Focus);
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.settle();
    assert_eq!(tuning(&h).unwrap().reference.degree, 1);
    // Lowest Note moves both ends, keeping the count; Highest Note too.
    let count = tuning(&h).unwrap().key_count();
    step(&mut h, "tuning-lowest", Action::Increment);
    let ranged = tuning(&h).unwrap();
    assert_eq!(
        ranged.range.lowest.index(19),
        before.range.lowest.index(19) + 1
    );
    assert_eq!(ranged.key_count(), count);
    step(&mut h, "tuning-highest", Action::Decrement);
    let ranged_again = tuning(&h).unwrap();
    assert_eq!(
        ranged_again.range.highest.index(19),
        ranged.range.highest.index(19) - 1
    );
    // The values read back.
    let all = read(&mut h);
    assert_eq!(find(&all, Role::SpinButton, "Octave").value, "4");
    assert_eq!(find(&all, Role::ComboBox, "Note").value, "1");
    assert_eq!(
        find(&all, Role::SpinButton, "Lowest Note").value,
        ranged_again.note_name(ranged_again.range.lowest)
    );
    // Each ran its command: one undo step each.
    let steps = h.shell.editor().undo_history().len();
    assert!(steps >= 6, "{steps}");
}

// Spec: tunings R28. Delete on the name removes the tuning; with the focus
// elsewhere it does not.
#[test]
fn delete_on_the_name_removes_the_tuning() {
    let scratch = Scratch::new("delete");
    let mut h = loaded(&scratch);
    // The triangle has the focus: Delete does nothing to the tuning.
    h.accesskit(triangle_id(), Action::Focus);
    h.key(Key::Delete, Modifiers::NONE);
    h.settle();
    assert!(tuning(&h).is_some());
    h.accesskit(name_id(), Action::Focus);
    h.settle();
    h.key(Key::Delete, Modifiers::NONE);
    h.settle();
    assert_eq!(tuning(&h), None, "back to twelve equal steps");
    let all = nodes(&mut h);
    assert_eq!(
        find(&all, Role::Button, "Tuning").value,
        "Twelve equal steps"
    );
    assert!(!has(&all, "Octave"), "no controls without a tuning");
    // Undo puts it back.
    h.shell.run("edit.undo", json!({})).unwrap();
    assert!(tuning(&h).is_some());
}

// Spec: tunings R28 (M-VIEW-023, M-NAV-009). View ▸ Tuning shows and hides
// the section; Navigate ▸ Tuning shows it and gives its name the focus.
#[test]
fn view_tuning_and_navigate_tuning_show_and_focus_the_section() {
    let mut h = Harness::manual_with_title_bar(Window::Main);
    h.settle();
    assert!(!has(&nodes(&mut h), "Tuning"));
    h.shell.invoke("view.tuning_section", json!({})).unwrap();
    assert!(has(&nodes(&mut h), "Tuning"));
    h.shell.invoke("view.tuning_section", json!({})).unwrap();
    assert!(!has(&nodes(&mut h), "Tuning"));
    h.shell.invoke("focus.tuning", json!({})).unwrap();
    h.settle();
    h.settle();
    assert!(has(&nodes(&mut h), "Tuning"));
    assert!(
        h.ctx.memory(|m| m.has_focus(name_id())),
        "the name has the keyboard"
    );
}

// Spec: tunings R28. The save button waits for the ASCL writer, so it is
// unavailable; the link button opens the tuning's link and is unavailable
// where there is none.
#[test]
fn the_buttons_are_unavailable_without_a_link_or_a_writer() {
    let scratch = Scratch::new("buttons");
    let mut h = loaded(&scratch);
    let all = nodes(&mut h);
    assert!(find(&all, Role::Button, "Save Tuning").disabled);
    assert!(find(&all, Role::Button, "Open Tuning Link").disabled);
    // A tuning with a link.
    let mut linked = tuning(&h).unwrap();
    linked.link = Some("https://example.org/tuning".to_owned());
    h.shell
        .run("tuning.set", json!({ "tuning": linked, "retune": false }))
        .unwrap();
    let all = nodes(&mut h);
    let link = find(&all, Role::Button, "Open Tuning Link");
    assert!(!link.disabled);
    let at = centre(link);
    h.frame(vec![Event::PointerMoved(at)]);
    let mut opened = Vec::new();
    for pressed in [true, false] {
        let output = h.frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }]);
        opened.extend(urls(&output));
    }
    assert_eq!(opened, ["https://example.org/tuning"]);
    // A link that is not a web address is not opened.
    let mut odd = tuning(&h).unwrap();
    odd.link = Some("file:///etc/passwd".to_owned());
    h.shell
        .run("tuning.set", json!({ "tuning": odd, "retune": false }))
        .unwrap();
    let all = nodes(&mut h);
    assert!(find(&all, Role::Button, "Open Tuning Link").disabled);
}

fn urls(output: &FullOutput) -> Vec<String> {
    output
        .platform_output
        .commands
        .iter()
        .filter_map(|command| match command {
            egui::OutputCommand::OpenUrl(open) => Some(open.url.clone()),
            _ => None,
        })
        .collect()
}

// Spec: tunings R28. Assistive technology reads each control with its name and
// value, and the section hangs from the browser.
#[test]
fn assistive_technology_reads_the_sections_controls() {
    let scratch = Scratch::new("access");
    let mut h = loaded(&scratch);
    let all = nodes(&mut h);
    let browser = find(&all, Role::Group, "Browser");
    for (role, label) in [
        (Role::Button, "Tuning"),
        (Role::Button, "Tuning Controls"),
        (Role::SpinButton, "Octave"),
        (Role::ComboBox, "Note"),
        (Role::SpinButton, "Ref. Pitch"),
        (Role::SpinButton, "Lowest Note"),
        (Role::SpinButton, "Highest Note"),
        (Role::Button, "Save Tuning"),
        (Role::Button, "Open Tuning Link"),
    ] {
        let node = find(&all, role, label);
        assert!(
            Harness::descends_from(&all, node.id, browser.id),
            "{label} is in the browser"
        );
    }
    for label in ["Octave", "Ref. Pitch", "Lowest Note", "Highest Note"] {
        let node = find(&all, Role::SpinButton, label);
        assert!(node.numeric.is_some() && !node.value.is_empty(), "{label}");
    }
    // The triangle folds the controls away and back.
    h.accesskit(triangle_id(), Action::Click);
    h.settle();
    let folded = nodes(&mut h);
    assert!(!has(&folded, "Octave"));
    assert!(has(&folded, "Open Tuning Link"));
    h.accesskit(triangle_id(), Action::Click);
    assert!(has(&nodes(&mut h), "Octave"));
}

// Spec: tunings R16. A tuning of other than twelve notes hides the scale
// controls of the Control Bar and draws a scale-aware device's Use Current
// Scale disabled and off; a twelve-note tuning changes nothing; they come
// back with the tuning gone.
#[test]
fn a_tuning_of_other_than_twelve_notes_hides_the_scale_controls() {
    let mut h = Harness::new(Window::Main);
    h.settle();
    let control_bar = |h: &mut Harness| {
        let all = nodes(h);
        ["Scale Mode", "Root Note", "Scale Name"].map(|label| has(&all, label))
    };
    assert_eq!(control_bar(&mut h), [true; 3]);
    let layout = ShellLayout::compute(REFERENCE, true, true);
    let _ = layout.rect(Element::ScaleMode);
    for (file, hidden) in [("equal-12.scl", false), ("equal-19.scl", true)] {
        h.shell
            .run(
                "tuning.load",
                json!({ "path": fixtures().join(file).to_string_lossy() }),
            )
            .unwrap();
        assert_eq!(control_bar(&mut h), [!hidden; 3], "{file}");
    }
    h.shell.run("tuning.remove", json!({})).unwrap();
    assert_eq!(control_bar(&mut h), [true; 3], "back with the tuning gone");
}

/// A song with a MIDI track holding a clip with a note on key 36, open in
/// the Clip View, the 19-step tuning loaded.
fn editor_under_tuning() -> (Harness, bl_model::TrackId) {
    let mut editor = factory_default_set();
    let tracks: Vec<bl_model::TrackId> = editor.document().top_level().collect();
    let id: bl_model::ClipId = editor.new_id();
    let note = bl_model::Note::new(
        36,
        bl_core::time::Ticks::ZERO,
        bl_core::time::Ticks(bl_core::time::TICKS_PER_QUARTER),
        100,
    );
    editor
        .apply(bl_model::Command::CreateClip {
            track: tracks[0],
            clip: bl_model::Clip::new(
                id,
                bl_core::time::Ticks::ZERO,
                bl_core::time::Ticks(bl_core::time::TICKS_PER_QUARTER * 16),
                bl_model::ClipData::Midi(bl_model::MidiData::new([note])),
            ),
        })
        .unwrap();
    editor.clear_history().unwrap();
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell = benchmark::shell_with(editor);
    h.settle();
    h.shell
        .run("arrangement.select_clips", json!({ "clips": [id] }))
        .unwrap();
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    h.shell
        .run(
            "tuning.load",
            json!({ "path": fixtures().join("equal-19.scl").to_string_lossy() }),
        )
        .unwrap();
    h.settle();
    (h, tracks[0])
}

/// The labels of the piano ruler's keys, which are list items of the editor.
fn rows(h: &mut Harness) -> Vec<Node> {
    nodes(h)
        .into_iter()
        .filter(|n| n.role == Role::ListItem)
        .collect()
}

// Spec: tunings R29. The rows are the tuning's keys, named by their degree's
// number with the octave where the file names none; on a track that bypasses
// the tuning they are twelve-step keys and their names.
#[test]
fn the_editors_rows_are_named_as_the_tuning_names_them() {
    let (mut h, track) = editor_under_tuning();
    let tuning = tuning(&h).unwrap();
    let name = |key: u8| tuning.note_name(tuning.key_note(key).unwrap());
    let shown = rows(&mut h);
    let labels: Vec<&str> = shown.iter().map(|n| n.label.as_str()).collect();
    assert!(labels.contains(&name(36).as_str()), "{labels:?}");
    assert!(!labels.contains(&"C1"), "no twelve-step names: {labels:?}");
    // 19 steps: key 36 is degree 14 of octave 1 (key 60 is degree 0 of octave 3).
    assert_eq!(name(36), "14:1");
    assert_eq!(name(60), "0:3");

    // A file that names its degrees names its rows by them.
    let mut named = tuning.clone();
    named.names = (0..19).map(|d| format!("n{d}-")).collect();
    h.shell
        .run("tuning.set", json!({ "tuning": named, "retune": false }))
        .unwrap();
    let labels: Vec<String> = rows(&mut h).into_iter().map(|n| n.label).collect();
    assert!(labels.contains(&"n14-1".to_owned()), "{labels:?}");

    // Bypass Tuning: twelve-step keys, with their names.
    h.shell
        .run(
            "track.set_tuning_bypass",
            json!({ "track": track, "bypass": true }),
        )
        .unwrap();
    let labels: Vec<String> = rows(&mut h).into_iter().map(|n| n.label).collect();
    assert!(labels.contains(&"C1".to_owned()), "{labels:?}");
    assert!(
        !labels
            .iter()
            .any(|l| l.starts_with('n') && l.ends_with('1'))
    );
}

// Spec: tunings R29. Keys past the tuning's range are not shown.
#[test]
fn keys_past_the_tunings_range_are_not_shown() {
    let (mut h, _) = editor_under_tuning();
    let full = tuning(&h).unwrap();
    // 30 notes: keys 0 to 29 play; the note on key 36 stays in the clip.
    h.shell
        .run(
            "tuning.set_range",
            json!({ "lowest": { "octave": 0, "degree": 0 },
                    "highest": { "octave": 1, "degree": 10 } }),
        )
        .unwrap();
    let limited = tuning(&h).unwrap();
    assert_eq!(limited.key_count(), 30);
    let labels: Vec<String> = rows(&mut h).into_iter().map(|n| n.label).collect();
    let last = limited.note_name(limited.key_note(29).unwrap());
    assert!(labels.contains(&last), "key 29 shows: {labels:?}");
    let past = TuningNote::from_index(limited.range.lowest.index(19) + 30, 19).unwrap();
    let past = full.note_name(past);
    assert!(!labels.contains(&past), "key 30 does not: {labels:?}");
}

/// Whether a frame painted `text` somewhere (the Status Bar's line).
fn painted(output: &FullOutput, text: &str) -> bool {
    fn has_text(shape: &egui::Shape, text: &str) -> bool {
        match shape {
            egui::Shape::Text(t) => t.galley.job.text == text,
            egui::Shape::Vec(shapes) => shapes.iter().any(|s| has_text(s, text)),
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|clipped| has_text(&clipped.shape, text))
}

/// The note area's rectangle in the Clip View.
fn grid(h: &mut Harness) -> egui::Rect {
    let all = nodes(h);
    let b = Harness::node(&all, "Clip View").bounds.expect("bounds");
    let view = bl_ui::clip_view::editor_rect(egui::Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    ));
    let shown = h.shell.clip_view().lanes().shown();
    let bar = h.shell.clip_view().note_filters().visible;
    bl_ui::note_lanes::note_layout(
        bl_ui::note_filters::body_rect(view, bar),
        &h.main.clip_view().lane_heights(&shown),
    )
    .grid
}

// Spec: tunings R30. The pointer over a row, or a note, shows in the Status
// Bar its name, its frequency in Hz and the key of an incoming keyboard that
// plays it.
#[test]
fn the_status_bar_shows_a_rows_name_and_frequency() {
    let (mut h, _) = editor_under_tuning();
    let tuning = tuning(&h).unwrap();
    let grid = grid(&mut h);
    let top = h.main.clip_view().top();
    let key = |key: u8, x: f32| {
        pos2(
            x,
            bl_ui::clip_view::y_of_key(grid, top, key) + bl_ui::clip_view::KEY_HEIGHT / 2.0,
        )
    };
    let line = |key: u8| {
        format!(
            "{}, {:.2} Hz, key {}",
            tuning.note_name(tuning.key_note(key).unwrap()),
            tuning.key_frequency(key).unwrap(),
            bl_midi_tools::key_name(key)
        )
    };
    // Over the key track.
    let at = key(36, grid.left() - bl_ui::clip_view::KEY_WIDTH / 2.0);
    h.frame(vec![Event::PointerMoved(at)]);
    let output = h.frame(vec![Event::PointerMoved(at)]);
    assert!(painted(&output, &line(36)), "{:?}", line(36));
    // Over a row of the note area, away from a note.
    let at = key(34, grid.right() - 20.0);
    h.frame(vec![Event::PointerMoved(at)]);
    let output = h.frame(vec![Event::PointerMoved(at)]);
    assert!(painted(&output, &line(34)), "{:?}", line(34));
    // Not on a bypassing track.
    let track = h.shell.editor().document().top_level().next().unwrap();
    h.shell
        .run(
            "track.set_tuning_bypass",
            json!({ "track": track, "bypass": true }),
        )
        .unwrap();
    h.frame(vec![Event::PointerMoved(at)]);
    let output = h.frame(vec![Event::PointerMoved(at)]);
    assert!(!painted(&output, &line(34)));
}

// Spec: tunings R16. The Clip View's scale controls — the Main Properties
// panel's and the editor toolbar's Fold to Scale and Highlight Scale — are
// hidden under a tuning of other than twelve notes, and back with it gone.
#[test]
fn the_clip_views_scale_controls_are_hidden_under_a_19_step_tuning() {
    let (mut h, _) = editor_under_tuning();
    let clip = h
        .shell
        .clip_view()
        .shown_clip(h.shell.editor().document(), h.shell.arrangement())
        .expect("the editor shows the clip");
    h.shell
        .run(
            "clip.set_scale",
            json!({ "clips": [clip], "enabled": true }),
        )
        .unwrap();
    let controls = |h: &mut Harness| {
        let all = nodes(h);
        ["Clip Scale Mode", "Fold to Scale", "Highlight Scale"].map(|label| has(&all, label))
    };
    assert_eq!(controls(&mut h), [false; 3], "hidden under 19 notes");
    h.shell
        .run(
            "tuning.load",
            json!({ "path": fixtures().join("equal-12.scl").to_string_lossy() }),
        )
        .unwrap();
    assert_eq!(controls(&mut h), [true; 3], "twelve notes change nothing");
    h.shell
        .run(
            "tuning.load",
            json!({ "path": fixtures().join("bohlen-pierce.scl").to_string_lossy() }),
        )
        .unwrap();
    assert_eq!(controls(&mut h), [false; 3]);
    h.shell.run("tuning.remove", json!({})).unwrap();
    assert_eq!(controls(&mut h), [true; 3], "back with the tuning gone");
}

//! The browser, headless (WP-1.10): where its parts sit, that it builds
//! only the rows on screen, that clicks, keys, and assistive technology
//! reach its commands, that the preview strip follows the preview, and that
//! dragging files from it into the Arrangement places them. Spec: browser.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bl_commands::Context;
use bl_core::time::Ticks;
use bl_model::{TrackId, TrackKind};
use bl_shell::Area;
use bl_shell::browser::Part;
use bl_shell::browser::places::create_user_library;
use bl_shell::browser_commands::{self as ids, Label};
use bl_ui::browser::{add_folder_id, label_id, list_id, raw_toggle_id, search_id};
use bl_ui::keymap::{Keymap, Mods, Platform};
use bl_ui::main_window::UiEvent;
use common::{Harness, Window, command};
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2};
use serde_json::json;

const BAR: i64 = Ticks::WHOLE.get();

/// The list's first row's top, in window coordinates: the content pane
/// starts at y 84 and its header row is 19 points tall.
const LIST_TOP: f32 = 103.0;

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-ui-browser-{}-{name}", std::process::id()));
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

/// A harness that draws Live's title bar, so coordinates are layout.md's,
/// with a User Library holding a loop and a folder of drums.
fn harness(scratch: &Scratch) -> (Harness, PathBuf) {
    let root = scratch.0.join("User Library");
    create_user_library(&root).unwrap();
    wav(&root.join("Samples").join("Drums").join("Kick.wav"), 4_800);
    wav(&root.join("Samples").join("Drums").join("Snare.wav"), 4_800);
    wav(&root.join("Loop.wav"), 96_000);
    let mut h = Harness::manual_with_title_bar(Window::Main);
    h.shell.set_user_library(Some(root.clone()));
    h.shell
        .run(ids::OPEN_PLACE, json!({ "label": "user_library" }))
        .unwrap();
    h.settle();
    (h, root)
}

/// The middle of the list's row `index`.
fn row(index: usize) -> Pos2 {
    pos2(160.0, LIST_TOP + 17.0 * index as f32 + 8.5)
}

fn names(h: &Harness) -> Vec<String> {
    let view = h.shell.browser_view();
    (0..view.row_count())
        .map(|i| view.row(i).unwrap().name)
        .collect()
}

fn selected(h: &Harness) -> Vec<String> {
    h.shell
        .browser_view()
        .selected()
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

fn invoked(h: &mut Harness, id: &str) -> bool {
    h.take_events()
        .iter()
        .any(|e| matches!(e, UiEvent::Invoke(i) if i.id == id))
}

/// A drag from `from` to `to` with `modifiers` held throughout.
fn drag(h: &mut Harness, from: Pos2, to: Pos2, modifiers: Modifiers) {
    let held = Event::ModifiersChanged(modifiers);
    h.frame(vec![held.clone(), Event::PointerMoved(from)]);
    h.frame(vec![
        held.clone(),
        Event::PointerButton {
            pos: from,
            button: PointerButton::Primary,
            pressed: true,
            modifiers,
        },
    ]);
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        h.frame(vec![
            held.clone(),
            Event::PointerMoved(from + (to - from) * t),
        ]);
    }
    h.frame(vec![
        held,
        Event::PointerButton {
            pos: to,
            button: PointerButton::Primary,
            pressed: false,
            modifiers,
        },
    ]);
    h.settle();
}

/// A click with `modifiers` held, as the platform reports them.
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

fn top_level(h: &Harness) -> Vec<TrackId> {
    h.shell.editor().document().top_level().collect()
}

// Spec: browser R1.
#[test]
fn the_browser_sits_where_live_puts_it() {
    let scratch = Scratch::new("layout");
    let (mut h, _) = harness(&scratch);
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    let bounds = |label: &str| {
        let b = Harness::node(&nodes, label).bounds.expect(label);
        (b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32)
    };
    let near = |a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)| {
        (a.0 - b.0).abs() < 0.51
            && (a.1 - b.1).abs() < 0.51
            && (a.2 - b.2).abs() < 0.51
            && (a.3 - b.3).abs() < 0.51
    };
    // L-WIN-004, L-BRW-002, 006, 010.
    for (label, expected) in [
        ("Browser", (9.0, 61.0, 311.0, 766.0)),
        ("Browser Sidebar", (9.0, 84.0, 109.0, 766.0)),
        ("Browser Items", (110.0, 103.0, 311.0, 744.0)),
        ("Name", (110.0, 84.0, 311.0, 103.0)),
    ] {
        assert!(
            near(bounds(label), expected),
            "{label}: {:?}",
            bounds(label)
        );
    }
    // L-BRW-001: Back, Forward, the search field, the filters.
    assert!((bounds("Browse Back").0 - 9.0).abs() < 0.51);
    assert!((bounds("Browse Forward").0 - 29.0).abs() < 0.51);
    assert!((bounds("Show Filter View").0 - 275.0).abs() < 0.51);
    assert!((bounds("Filter View Menu").0 - 297.0).abs() < 0.51);
    let search = nodes
        .iter()
        .find(|n| n.role == Role::TextInput)
        .and_then(|n| n.bounds)
        .unwrap();
    assert!((search.x0 - 50.0).abs() < 0.51 && (search.x1 - 270.0).abs() < 0.51);
    // L-BRW-011: the Preview switch at x 114, y 748.
    let preview = bounds("Preview");
    assert!((preview.0 - 114.0).abs() < 0.51 && (preview.1 - 748.0).abs() < 0.51);
    // Hiding the browser gives the main view its place.
    h.shell
        .run(bl_shell::commands::VIEW_TOGGLE_BROWSER, json!({}))
        .unwrap();
    let output = h.settle();
    assert!(
        !Harness::nodes(&output)
            .iter()
            .any(|n| n.label == "Browser Sidebar")
    );
}

// Spec: browser R2, R17.
#[test]
fn every_part_reaches_assistive_technology() {
    let scratch = Scratch::new("a11y");
    let (mut h, _) = harness(&scratch);
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    let browser = Harness::node(&nodes, "Browser").id;
    let sidebar = Harness::node(&nodes, "Browser Sidebar");
    assert_eq!(sidebar.role, Role::List);
    let mut headings: Vec<&str> = nodes
        .iter()
        .filter(|n| n.role == Role::Heading && Harness::descends_from(&nodes, n.id, browser))
        .map(|n| n.label.as_str())
        .collect();
    headings.sort_unstable();
    assert_eq!(headings, ["Collections", "Library", "Places"]);
    let user_library = Harness::node(&nodes, "User Library");
    assert_eq!(user_library.role, Role::ListItem);
    assert!(user_library.description.contains("User Library"));
    // Instruments lists the instrument rack since WP-5.8, so Sounds stands
    // for a label whose content is still to come.
    let sounds = Harness::node(&nodes, "Sounds");
    assert!(sounds.description.contains("M2"));
    assert!(
        Harness::node(&nodes, "Favorites").disabled,
        "collections are M6"
    );
    // Rows name their kind.
    let samples = nodes
        .iter()
        .find(|n| n.label == "Samples" && n.value.starts_with("folder"))
        .expect("the Samples folder's row");
    assert_eq!(samples.role, Role::ListItem);
    let looped = Harness::node(&nodes, "Loop.wav");
    assert_eq!(looped.value, "audio file");
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::CheckBox && n.label == "Preview")
    );
    let strip = Harness::node(&nodes, "Preview Strip");
    assert_eq!(strip.role, Role::ProgressIndicator);
    assert_eq!(strip.value, "nothing previewed");
    assert!(Harness::node(&nodes, "Add Folder\u{2026}").role == Role::Button);
}

// Spec: browser R5.
#[test]
fn add_folder_asks_for_a_folder() {
    let scratch = Scratch::new("add-folder");
    let (mut h, _) = harness(&scratch);
    h.take_events();
    let rect = h.ctx.read_response(add_folder_id()).expect("drawn").rect;
    h.click(rect.center());
    assert!(invoked(&mut h, ids::ADD_FOLDER));
    // The shell asked for the folder dialog (the harness drains it); the
    // dialog's answer runs the same command with the path.
    let loops = scratch.0.join("Loops");
    wav(&loops.join("Break.wav"), 100);
    h.shell
        .invoke(ids::ADD_FOLDER, json!({ "path": loops.to_string_lossy() }))
        .unwrap();
    h.settle();
    assert_eq!(names(&h), ["Break.wav"]);
    let label = Label::Folder(loops.to_string_lossy().into_owned());
    assert!(h.ctx.read_response(label_id(&label)).is_some(), "in Places");
}

// Spec: browser R7.
#[test]
fn clicks_select_and_double_clicks_open() {
    let scratch = Scratch::new("clicks");
    let (mut h, _) = harness(&scratch);
    h.shell
        .run(ids::SET_PREVIEW, json!({ "on": false }))
        .unwrap();
    assert_eq!(
        names(&h),
        [
            "Clips", "Defaults", "Grooves", "Presets", "Samples", "Loop.wav"
        ]
    );
    h.click(row(5));
    assert_eq!(selected(&h), ["Loop.wav"]);
    assert_eq!(h.shell.focused_area(), Area::Browser);
    assert_eq!(h.shell.browser_view().focus(), Part::Content);
    // Shift-click selects the range; Cmd-click adds and removes.
    click_with(&mut h, row(3), Modifiers::SHIFT);
    assert_eq!(selected(&h), ["Loop.wav", "Samples", "Presets"]);
    click_with(&mut h, row(4), command());
    assert_eq!(selected(&h), ["Loop.wav", "Presets"]);
    // A double-click on a folder opens it.
    let double = |h: &mut Harness, at: Pos2| {
        // Long enough after the clicks above that this isn't a triple click.
        h.time += 1.0;
        for _ in 0..2 {
            h.frame(vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            }]);
            h.frame(vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }]);
        }
        h.settle();
    };
    h.frame(vec![Event::PointerMoved(row(4))]);
    double(&mut h, row(4));
    assert_eq!(names(&h)[5], "Drums");
    // A double-click on a file loads it onto a new track (no track is
    // selected).
    let before = top_level(&h).len();
    h.frame(vec![Event::PointerMoved(row(6))]);
    double(&mut h, row(6));
    assert_eq!(top_level(&h).len(), before + 1);
}

// Spec: browser R7.
#[test]
fn keys_move_through_the_sidebar_and_the_list() {
    let scratch = Scratch::new("keys");
    let (mut h, _) = harness(&scratch);
    h.shell
        .run(ids::SET_PREVIEW, json!({ "on": false }))
        .unwrap();
    h.click(row(0));
    assert_eq!(selected(&h), ["Clips"]);
    assert!(h.ctx.memory(|m| m.has_focus(list_id())));
    for _ in 0..4 {
        h.key(Key::ArrowDown, Modifiers::NONE);
    }
    assert_eq!(selected(&h), ["Samples"]);
    // Right opens the folder, Right again goes into it, Left goes back up,
    // Left again closes it.
    h.key(Key::ArrowRight, Modifiers::NONE);
    assert_eq!(names(&h)[5], "Drums");
    h.key(Key::ArrowRight, Modifiers::NONE);
    assert_eq!(selected(&h), ["Drums"]);
    h.key(Key::ArrowLeft, Modifiers::NONE);
    assert_eq!(selected(&h), ["Samples"]);
    h.key(Key::ArrowLeft, Modifiers::NONE);
    assert_eq!(names(&h).len(), 6, "closed");
    // Shift-Down extends the selection.
    h.key(Key::ArrowDown, Modifiers::SHIFT);
    assert_eq!(selected(&h), ["Samples", "Loop.wav"]);
    h.key(Key::ArrowUp, Modifiers::NONE);
    assert_eq!(selected(&h), ["Samples"]);
    // Left at the top level moves to the sidebar, where Up and Down choose
    // labels and Right goes back.
    h.key(Key::ArrowLeft, Modifiers::NONE);
    assert_eq!(h.shell.browser_view().focus(), Part::Sidebar);
    assert_eq!(h.shell.active_contexts()[0], Context::BrowserSidebar);
    h.key(Key::ArrowDown, Modifiers::NONE);
    assert_eq!(h.shell.browser_view().label(), &Label::CurrentProject);
    h.key(Key::ArrowUp, Modifiers::NONE);
    h.key(Key::ArrowUp, Modifiers::NONE);
    assert_eq!(h.shell.browser_view().label(), &Label::Templates);
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.key(Key::ArrowRight, Modifiers::NONE);
    assert_eq!(h.shell.browser_view().focus(), Part::Content);
    // Enter loads the selected file.
    h.click(row(5));
    let before = top_level(&h).len();
    h.key(Key::Enter, Modifiers::NONE);
    assert_eq!(top_level(&h).len(), before + 1, "Enter loaded Loop.wav");
}

// Spec: browser R8.
#[test]
fn option_5_and_cmd_f_reach_the_browser() {
    let scratch = Scratch::new("focus");
    let (mut h, _) = harness(&scratch);
    h.shell
        .run(bl_shell::commands::VIEW_TOGGLE_BROWSER, json!({}))
        .unwrap();
    h.settle();
    assert!(!h.shell.browser_visible());
    // Option-5 (Alt-5) shows the browser and focuses it.
    let platform = Platform::current();
    let alt = Mods {
        alt: true,
        ..Mods::default()
    }
    .to_egui(platform);
    h.key(Key::Num5, alt);
    assert!(h.shell.browser_visible());
    assert_eq!(h.shell.focused_area(), Area::Browser);
    h.settle();
    assert!(h.ctx.memory(|m| m.has_focus(list_id())));
    // Cmd-F (Ctrl-F) chooses All and puts the cursor in the search field.
    h.key(Key::F, command());
    h.settle();
    assert_eq!(h.shell.browser_view().label(), &Label::All);
    assert!(h.ctx.memory(|m| m.has_focus(search_id())));
    // The keys resolve as the keymap says.
    let keymap = Keymap::live12();
    let hit = |key, mods: Mods, contexts: &[Context]| {
        keymap
            .resolve(platform, key, mods, contexts)
            .map(|hit| hit.command.id.clone())
    };
    let alt5 = Mods {
        alt: true,
        ..Mods::default()
    };
    assert_eq!(
        hit(Key::Num5, alt5, &[Context::Arrangement, Context::Global]).as_deref(),
        Some(ids::FOCUS_BROWSER)
    );
    assert_eq!(
        hit(
            Key::Enter,
            Mods::default(),
            &[Context::Browser, Context::Global]
        )
        .as_deref(),
        Some(ids::LOAD_SELECTED)
    );
}

// Spec: browser R9.
#[test]
fn typing_searches_as_you_type() {
    let scratch = Scratch::new("search");
    let (mut h, _) = harness(&scratch);
    let deadline = Instant::now() + Duration::from_secs(10);
    while h.shell.browser_view().index_state().indexing {
        h.shell.poll();
        assert!(Instant::now() < deadline, "the index never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
    h.key(Key::F, command());
    h.settle();
    h.frame(vec![Event::Text("sn".to_owned())]);
    h.settle();
    assert_eq!(h.shell.browser_view().search_text(), "sn");
    assert_eq!(names(&h), ["Snare.wav"]);
    h.frame(vec![Event::Text("x".to_owned())]);
    h.settle();
    assert!(names(&h).is_empty());
    assert!(
        h.shell
            .browser_view()
            .note()
            .unwrap()
            .contains("Nothing matches")
    );
    h.key(Key::Backspace, Modifiers::NONE);
    h.settle();
    assert_eq!(names(&h), ["Snare.wav"]);
    // Down moves to the first result.
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.settle();
    assert_eq!(selected(&h), ["Snare.wav"]);
    assert_eq!(h.shell.browser_view().focus(), Part::Content);
}

// Spec: browser R11.
#[test]
fn the_preview_strip_follows_the_preview() {
    let scratch = Scratch::new("preview");
    let (mut h, _) = harness(&scratch);
    assert!(h.shell.audio().is_running(), "the null device");
    h.click(row(5));
    h.wait("the preview to play", |h| {
        h.shell.browser_preview().position_seconds > 0.0
    });
    assert!(h.shell.browser_view().preview_overview().is_some());
    // The switch turns preview off, which stops it.
    let toggle = h
        .ctx
        .read_response(bl_ui::browser::preview_toggle_id())
        .unwrap()
        .rect;
    h.click(toggle.center());
    assert!(!h.shell.browser_view().preview_on());
    assert_eq!(h.shell.browser_preview().playing, None);
}

// Spec: browser R18.
#[test]
fn the_raw_switch_sits_in_the_preview_strip() {
    let scratch = Scratch::new("raw-strip");
    let (mut h, _) = harness(&scratch);
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    let raw = nodes
        .iter()
        .find(|n| n.role == Role::CheckBox && n.label == "Raw")
        .expect("the Raw switch");
    assert_eq!(raw.selected, Some(false));
    let raw_bounds = raw.bounds.expect("Raw bounds");
    // Sits at the right end of the preview strip (y 748, right around x 307).
    assert!((raw_bounds.y0 - 748.0).abs() < 1.0);
    assert!((raw_bounds.x1 - 307.0).abs() < 1.0);

    // Clicking it toggles raw on.
    let toggle = h.ctx.read_response(raw_toggle_id()).unwrap().rect;
    h.click(toggle.center());
    assert!(h.shell.browser_preview().raw);
}

/// Where a drop in the lanes lands at the default zoom: 30 points a bar
/// from x 320, tracks 88 points tall from y 119.
fn lanes_point(bars: f32, y: f32) -> Pos2 {
    pos2(320.0 + 30.0 * bars, y)
}

// Spec: browser R13. The acceptance criterion of WP-1.10.
#[test]
fn dragging_a_file_below_the_last_track_creates_a_track() {
    let scratch = Scratch::new("drag-new");
    let (mut h, _) = harness(&scratch);
    h.shell
        .run(ids::SET_PREVIEW, json!({ "on": false }))
        .unwrap();
    let before = top_level(&h);
    assert_eq!(before.len(), 4);
    // Four tracks of 88 points end at y 471; the drop area is below.
    drag(&mut h, row(5), lanes_point(2.3, 520.0), Modifiers::NONE);
    let after = top_level(&h);
    assert_eq!(after.len(), 5, "a new track");
    let doc = h.shell.editor().document();
    let track = doc.track(after[4]).unwrap();
    assert_eq!(track.kind(), TrackKind::Audio);
    assert_eq!(track.name(), "Loop");
    let clip = track.clips().next().unwrap();
    assert_eq!(clip.start, Ticks(2 * BAR), "snapped to the bar");
    assert_eq!(clip.length, Ticks(BAR));
    assert_eq!(h.shell.editor().undo_history().len(), 1);
}

// Spec: browser R13.
#[test]
fn dragging_onto_a_track_places_a_clip_at_the_drop() {
    let scratch = Scratch::new("drag-track");
    let (mut h, root) = harness(&scratch);
    h.shell
        .run(ids::SET_PREVIEW, json!({ "on": false }))
        .unwrap();
    let audio = top_level(&h)[2];
    // "3 Audio" spans y 295 to 383; 4.3 bars snaps to the nearest bar.
    drag(&mut h, row(5), lanes_point(4.3, 340.0), Modifiers::NONE);
    let doc = h.shell.editor().document();
    assert_eq!(top_level(&h).len(), 4, "no new track");
    let clip = doc.track(audio).unwrap().clips().next().expect("a clip");
    assert_eq!(clip.start, Ticks(4 * BAR));
    assert_eq!(clip.name, "Loop");
    // Onto a MIDI track: refused, nothing changes.
    drag(&mut h, row(5), lanes_point(1.0, 160.0), Modifiers::NONE);
    assert_eq!(h.shell.editor().document().clip_count(), 1);
    // Two files with Cmd (Ctrl) held: a track each.
    h.shell
        .run(
            ids::OPEN_FOLDER,
            json!({ "path": root.join("Samples").to_string_lossy() }),
        )
        .unwrap();
    h.shell
        .run(
            ids::OPEN_FOLDER,
            json!({ "path": root.join("Samples").join("Drums").to_string_lossy() }),
        )
        .unwrap();
    h.settle();
    assert_eq!(names(&h)[6..8], ["Kick.wav", "Snare.wav"]);
    h.click(row(6));
    click_with(&mut h, row(7), Modifiers::SHIFT);
    assert_eq!(selected(&h), ["Kick.wav", "Snare.wav"]);
    drag(&mut h, row(6), lanes_point(8.0, 340.0), command());
    let tracks = top_level(&h);
    assert_eq!(tracks.len(), 5, "one new track below 3 Audio");
    assert_eq!(tracks[3], {
        let doc = h.shell.editor().document();
        doc.top_level()
            .find(|t| doc.track(*t).unwrap().name() == "Snare")
            .unwrap()
    });
}

// Spec: browser R17.
#[test]
fn only_visible_rows_are_built() {
    let scratch = Scratch::new("many");
    let (mut h, root) = harness(&scratch);
    let many = root.join("Samples").join("Many");
    std::fs::create_dir_all(&many).unwrap();
    for i in 0..2_000 {
        std::fs::write(many.join(format!("Hit {i}.wav")), b"").unwrap();
    }
    h.shell
        .run(
            ids::OPEN_FOLDER,
            json!({ "path": root.join("Samples").to_string_lossy() }),
        )
        .unwrap();
    h.shell
        .run(ids::OPEN_FOLDER, json!({ "path": many.to_string_lossy() }))
        .unwrap();
    h.settle();
    let stats = h.main.browser().stats();
    assert!(stats.rows_total > 2_000);
    // The list is 641 points tall: 38 rows and a part.
    assert!(stats.rows_built <= 39, "{stats:?}");
    // Scrolling to the end builds the same few.
    h.frame(vec![
        Event::PointerMoved(row(10)),
        Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -100_000.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        },
    ]);
    let output = h.settle();
    assert!(h.main.browser().stats().rows_built <= 39);
    let nodes = Harness::nodes(&output);
    assert!(nodes.iter().any(|n| n.label == "Loop.wav"), "the last row");
    let _ = list_id();
}

// Spec: multisampler R47.
#[test]
fn dragging_an_sfz_instrument_makes_a_multisampler_on_a_midi_track_or_a_new_one() {
    let scratch = Scratch::new("drag-sfz");
    let (mut h, root) = harness(&scratch);
    h.shell
        .run(ids::SET_PREVIEW, json!({ "on": false }))
        .unwrap();
    std::fs::write(
        root.join("Zz Bank.sfz"),
        "<region> sample=Samples/Drums/Kick.wav key=36\n<region> sample=Samples/Drums/Snare.wav key=38\n",
    )
    .unwrap();
    h.shell
        .run(ids::OPEN_PLACE, json!({ "label": "user_library" }))
        .unwrap();
    h.settle();
    let at = names(&h).iter().position(|n| n == "Zz Bank.sfz").unwrap();
    let multisamplers = |h: &Harness| -> Vec<(bl_model::TrackId, usize)> {
        let doc = h.shell.editor().document();
        doc.tracks()
            .flat_map(|track| {
                track
                    .devices()
                    .filter(|d| !d.layers.list.is_empty())
                    .map(|d| (track.id(), d.layers.list.len()))
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    assert!(multisamplers(&h).is_empty());
    // Below the last track: a new MIDI track holding the instrument.
    let tracks = top_level(&h).len();
    drag(&mut h, row(at), lanes_point(2.3, 520.0), Modifiers::NONE);
    assert_eq!(top_level(&h).len(), tracks + 1, "a new track");
    let doc = h.shell.editor().document();
    let new = doc.track(*top_level(&h).last().unwrap()).unwrap();
    assert_eq!((new.kind(), new.name()), (TrackKind::Midi, "Zz Bank"));
    assert_eq!(multisamplers(&h), [(new.id(), 2)]);
    assert_eq!(h.shell.editor().undo_history().len(), 1, "one undo step");
    // Onto a MIDI track's lane: the multisampler goes on that track.
    let midi = top_level(&h)[0];
    drag(&mut h, row(at), lanes_point(1.0, 160.0), Modifiers::NONE);
    assert_eq!(top_level(&h).len(), tracks + 1, "no further track");
    assert!(multisamplers(&h).contains(&(midi, 2)));
    // Onto an audio track's lane: refused.
    let before = multisamplers(&h).len();
    drag(&mut h, row(at), lanes_point(4.3, 340.0), Modifiers::NONE);
    assert_eq!(multisamplers(&h).len(), before);
}

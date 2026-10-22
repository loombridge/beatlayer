//! Clip files by drag and drop, headless (WP-6.34; the clip files spec, R3,
//! R4, R7): an Arrangement clip and a Session clip dragged onto a folder in
//! the browser are saved there and the new name opens for editing; a clip
//! file dropped on the Session's drop area makes a track; the Library
//! settings page's rows. Each gesture runs the registered command an agent
//! runs.

mod common;

use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_model::{Clip, ClipData, ClipId, MidiData, Note, SceneId, TrackId, TrackKind};
use bl_shell::browser::places::create_user_library;
use bl_shell::browser_commands::BrowserState;
use bl_ui::layout::mixer::{MixerLayout, Sections, SessionLayout, TrackColumn};
use bl_ui::layout::{REFERENCE, ShellLayout, TITLE_BAR};
use common::{Harness, Window};
use egui::accesskit::Action;
use egui::{Event, Id, Key, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();

/// The list's first row's top, in window coordinates (the browser's tests').
const LIST_TOP: f32 = 103.0;

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-ui-clip-files-{}-{name}", std::process::id()));
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

/// The middle of the browser list's row `index`.
fn row(index: usize) -> Pos2 {
    pos2(160.0, LIST_TOP + 17.0 * index as f32 + 8.5)
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Opens the User Library in the browser; returns the row of its `name`
/// folder.
fn open_library(h: &mut Harness, root: &Path, name: &str) -> usize {
    h.shell
        .run("browser.open_place", json!({ "label": "user_library" }))
        .unwrap();
    h.settle();
    let browser = h.shell.browser_view();
    let rows: Vec<String> = (0..browser.row_count())
        .map(|i| browser.row(i).unwrap().name)
        .collect();
    let _ = root;
    rows.iter()
        .position(|n| n == name)
        .unwrap_or_else(|| panic!("{name} in {rows:?}"))
}

fn press(at: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// A press at `from`, eight steps, and a release at `to`.
fn drag(h: &mut Harness, from: Pos2, to: Pos2) {
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![press(from, true)]);
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        h.frame(vec![Event::PointerMoved(from + (to - from) * t)]);
    }
    h.frame(vec![press(to, false)]);
    h.settle();
}

fn browser(h: &mut Harness) -> BrowserState {
    serde_json::from_value(
        h.shell
            .run("browser.state", json!({ "limit": 1000 }))
            .unwrap(),
    )
    .unwrap()
}

/// A MIDI clip of one note, `start` and `length` in bars.
fn midi_clip(h: &mut Harness, track: TrackId, name: &str, start: i64, length: i64) -> ClipId {
    let id: ClipId = h.shell.new_id();
    let mut clip = Clip::new(
        id,
        Ticks(start * BAR),
        Ticks(length * BAR),
        ClipData::Midi(MidiData::new([Note::new(
            60,
            Ticks::ZERO,
            Ticks(BAR / 4),
            100,
        )])),
    );
    clip.name = name.to_owned();
    h.shell
        .run("clip.create", json!({ "track": track, "clip": clip }))
        .unwrap();
    id
}

fn tracks(h: &Harness, kind: TrackKind) -> Vec<TrackId> {
    let doc = h.shell.editor().document();
    doc.top_level()
        .filter(|id| doc.track(*id).is_some_and(|t| t.kind() == kind))
        .collect()
}

/// A short mono WAV file that isn't silence, `seconds` long.
fn wav(path: &Path, seconds: f32) {
    let frames = (48_000.0 * seconds) as usize;
    let samples: Vec<f32> = (0..frames)
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
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

// Spec: clip files R3, R4 (I-DROP-011). An Arrangement clip dragged onto a
// folder in the browser is saved there, changes nothing in the set, and its
// new row's name opens for editing; typing and Enter rename the file.
#[test]
fn an_arrangement_clip_dragged_onto_a_folder_is_saved_and_its_name_opens() {
    let scratch = Scratch::new("arrangement");
    let root = scratch.0.join("User Library");
    create_user_library(&root).unwrap();
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.set_user_library(Some(root.clone()));
    h.settle();
    let midi = tracks(&h, TrackKind::Midi)[0];
    let clip = midi_clip(&mut h, midi, "Idea", 2, 4);
    h.settle();
    let clips = open_library(&mut h, &root, "Clips");
    let before = h.shell.editor().document().clone();
    let steps = h.shell.editor().undo_history().len();

    // The first track's clip title bar, bars 2 to 6 at 30 points a bar from
    // x 320, to the Clips folder's row.
    drag(&mut h, pos2(410.0, 124.0), row(clips));
    let saved = root.join("Clips").join("Idea.blclip");
    assert!(saved.is_file(), "{saved:?}");
    assert_eq!(*h.shell.editor().document(), before, "the clip stayed");
    assert_eq!(h.shell.editor().undo_history().len(), steps);
    assert_eq!(h.shell.editor().document().clip_track(clip), Some(midi));
    let state = browser(&mut h);
    assert_eq!(state.renaming, Some(text(&saved)));
    assert_eq!(state.selected, [text(&saved)]);

    // Typing a name and Enter rename the file.
    h.frame(vec![Event::Text("Bass".to_owned())]);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert!(root.join("Clips").join("Bass.blclip").is_file());
    assert!(!saved.exists());
    assert_eq!(browser(&mut h).renaming, None);
}

// Spec: clip files R4. Esc keeps the name the file has.
#[test]
fn esc_keeps_the_name_a_saved_clip_has() {
    let scratch = Scratch::new("esc");
    let root = scratch.0.join("User Library");
    create_user_library(&root).unwrap();
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.set_user_library(Some(root.clone()));
    h.settle();
    let midi = tracks(&h, TrackKind::Midi)[0];
    let clip = midi_clip(&mut h, midi, "Idea", 2, 4);
    h.settle();
    h.shell
        .run("clip.save_file", json!({ "clip": clip }))
        .unwrap();
    h.settle();
    h.frame(vec![Event::Text("Other".to_owned())]);
    h.key(Key::Escape, Modifiers::NONE);
    h.settle();
    assert!(root.join("Clips").join("Idea.blclip").is_file());
    assert_eq!(browser(&mut h).renaming, None);
}

// Spec: file management R8. With Ask, the drop asks, and Copy copies the
// sample; Cancel saves nothing.
#[test]
fn ask_shows_copy_dont_copy_and_cancel() {
    let scratch = Scratch::new("ask");
    let root = scratch.0.join("User Library");
    create_user_library(&root).unwrap();
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.set_user_library(Some(root.clone()));
    h.shell
        .run(
            "settings.set_collect_files_on_export",
            json!({ "mode": "ask" }),
        )
        .unwrap();
    let sample = scratch.0.join("Elsewhere").join("Pad.wav");
    wav(&sample, 4.0);
    let audio = tracks(&h, TrackKind::Audio)[0];
    h.shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [text(&sample)], "track": audio, "at": 0 }),
        )
        .unwrap();
    h.settle();
    let clips = open_library(&mut h, &root, "Clips");

    // The first audio track's clip: bars 0 to 2, its title bar at y 300.
    drag(&mut h, pos2(340.0, 300.0), row(clips));
    assert!(
        std::fs::read_dir(root.join("Clips"))
            .unwrap()
            .next()
            .is_none(),
        "nothing is saved until the person answers"
    );
    let nodes = Harness::nodes(&h.settle());
    for label in ["Copy", "Don't Copy", "Cancel"] {
        let _ = Harness::node(&nodes, label);
    }
    // Cancel saves nothing.
    let cancel = Harness::node(&nodes, "Cancel").bounds.unwrap();
    h.click(pos2(
        ((cancel.x0 + cancel.x1) / 2.0) as f32,
        ((cancel.y0 + cancel.y1) / 2.0) as f32,
    ));
    h.settle();
    assert!(
        std::fs::read_dir(root.join("Clips"))
            .unwrap()
            .next()
            .is_none()
    );

    // Copy copies the sample into the User Library and saves.
    drag(&mut h, pos2(340.0, 300.0), row(clips));
    let nodes = Harness::nodes(&h.settle());
    let copy = Harness::node(&nodes, "Copy").bounds.unwrap();
    h.click(pos2(
        ((copy.x0 + copy.x1) / 2.0) as f32,
        ((copy.y0 + copy.y1) / 2.0) as f32,
    ));
    h.settle();
    assert!(root.join("Clips").join("Pad.blclip").is_file());
    assert!(
        root.join("Samples")
            .join("Imported")
            .join("Pad.wav")
            .is_file()
    );
}

fn session_harness() -> Harness {
    let mut h = Harness::manual_with_title_bar(Window::Main);
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    h.settle();
    h
}

/// The main view's rectangle at 1600 × 1000 points, as the window lays it
/// out.
fn view_rect() -> Rect {
    ShellLayout::compute(REFERENCE, true, true)
        .translate(vec2(0.0, TITLE_BAR))
        .main_view
}

/// The Session grid's layout, as the view builds it.
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

    fn centre(&self, track: usize, scene: usize) -> Pos2 {
        self.layout.columns[track]
            .rect
            .intersect(self.session.scene_row(scene, 0.0))
            .shrink(1.0)
            .center()
    }
}

// Spec: clip files R3, R4 (I-DROP-011). A Session clip dragged onto a folder
// is saved there too.
#[test]
fn a_session_clip_dragged_onto_a_folder_is_saved_and_its_name_opens() {
    let scratch = Scratch::new("session");
    let root = scratch.0.join("User Library");
    create_user_library(&root).unwrap();
    let mut h = session_harness();
    h.shell.set_user_library(Some(root.clone()));
    h.settle();
    let g = Grid::new(&h);
    let (track, scene) = (g.columns[0], g.scenes[0]);
    let id: ClipId = h.shell.new_id();
    h.shell
        .run(
            "clip.create_in_slot",
            json!({ "track": track, "scene": scene, "clip": Clip::new(
                id, Ticks::ZERO, Ticks(BAR),
                ClipData::Midi(MidiData::new([Note::new(60, Ticks::ZERO, Ticks(BAR / 4), 100)])),
            ) }),
        )
        .unwrap();
    h.settle();
    let clips = open_library(&mut h, &root, "Clips");
    let before = h.shell.editor().document().clone();

    drag(&mut h, g.centre(0, 0), row(clips));
    let name = h
        .shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .name()
        .to_owned();
    let saved = root.join("Clips").join(format!("{name}.blclip"));
    assert!(saved.is_file(), "{saved:?}");
    assert_eq!(*h.shell.editor().document(), before, "the clip stayed");
    assert_eq!(browser(&mut h).renaming, Some(text(&saved)));
}

// Spec: clip files R7, R8 (I-DROP-002). A clip file dropped on the Session's
// drop area makes a track of its kind, holding the clip, in the scene it was
// dropped at.
#[test]
fn a_clip_file_dropped_on_the_sessions_drop_area_makes_a_track() {
    let scratch = Scratch::new("drop-area");
    let root = scratch.0.join("User Library");
    create_user_library(&root).unwrap();
    let mut h = session_harness();
    h.shell.set_user_library(Some(root.clone()));
    let midi = tracks(&h, TrackKind::Midi)[0];
    let clip = midi_clip(&mut h, midi, "Idea", 0, 1);
    let saved: Value = h
        .shell
        .run("clip.save_file", json!({ "clip": clip }))
        .unwrap();
    let file = PathBuf::from(saved["path"].as_str().unwrap());
    h.settle();
    let g = Grid::new(&h);
    let area = g
        .layout
        .drop_area
        .expect("a drop area right of the tracks")
        .intersect(g.session.slots);
    let at = pos2(area.center().x, g.session.scene_row(2, 0.0).center().y);
    let tracks_before = h.shell.editor().document().top_level().len();

    h.frame(vec![Event::PointerMoved(at)]);
    h.dropped.push(std::sync::Arc::new(Dropped(file)));
    h.frame(Vec::new());
    h.settle();

    let doc = h.shell.editor().document();
    assert_eq!(doc.top_level().len(), tracks_before + 1);
    let made = doc.top_level().last().unwrap();
    assert_eq!(doc.track(made).unwrap().kind(), TrackKind::Midi);
    assert_eq!(doc.track(made).unwrap().name(), "Idea");
    assert!(
        doc.slot(made, g.scenes[2]).is_some(),
        "in the scene dropped at"
    );
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

// Spec: file management R7, browser R24 (L-SET-016). The Library page has
// Collect Files on Export and the User Library's folder.
#[test]
fn the_library_page_has_collect_files_on_export_and_the_user_library() {
    let scratch = Scratch::new("settings");
    let root = scratch.0.join("User Library");
    create_user_library(&root).unwrap();
    let mut h = Harness::new(Window::Settings);
    h.shell.set_user_library(Some(root.clone()));
    h.shell
        .invoke("app.settings", json!({ "page": "library" }))
        .unwrap();
    h.shell.take_effects();
    h.settle();

    let nodes = Harness::nodes(&h.frame(vec![]));
    let chooser = Harness::node(&nodes, "Collect Files on Export");
    assert_eq!(chooser.value, "Always", "a new install's choice");
    h.accesskit(
        Id::new(("chooser", "collect-files-on-export")),
        Action::Focus,
    );
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.settle();
    assert_eq!(
        h.shell.collect_files_on_export(),
        bl_shell::settings::CollectFiles::Never
    );
    assert!(
        h.events.iter().any(|e| matches!(
            e,
            bl_ui::main_window::UiEvent::Invoke(i)
                if i.id == "settings.set_collect_files_on_export"
        )),
        "the chooser runs its command"
    );
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert!(
        nodes
            .iter()
            .any(|n| n.label == "Path" && n.value == text(&root)),
        "the User Library's folder is shown"
    );
}

// Spec: clip files R7 (I-DROP-005). A clip file dragged from the browser onto
// an Arrangement lane of its clip's kind is placed at the drop position; on
// a lane of the other kind nothing is highlighted and nothing changes.
#[test]
fn a_clip_file_dragged_onto_a_lane_of_its_kind_is_placed_there() {
    let scratch = Scratch::new("lane");
    let root = scratch.0.join("User Library");
    create_user_library(&root).unwrap();
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.set_user_library(Some(root.clone()));
    h.settle();
    let midis = tracks(&h, TrackKind::Midi);
    let clip = midi_clip(&mut h, midis[0], "Idea", 2, 4);
    let saved: Value = h
        .shell
        .run("clip.save_file", json!({ "clip": clip }))
        .unwrap();
    let file = saved["path"].as_str().unwrap().to_owned();
    h.shell.run("browser.rename_file", json!({})).unwrap();
    h.shell
        .run("browser.open_place", json!({ "label": "user_library" }))
        .unwrap();
    let clips_folder = text(&root.join("Clips"));
    h.shell
        .run(
            "browser.open_folder",
            json!({ "path": clips_folder, "open": true }),
        )
        .unwrap();
    h.settle();
    let rows: Vec<String> = {
        let browser = h.shell.browser_view();
        (0..browser.row_count())
            .map(|i| browser.row(i).unwrap().path)
            .collect()
    };
    let at = rows
        .iter()
        .position(|path| *path == file)
        .unwrap_or_else(|| panic!("the clip file's row {file} in {rows:?}"));

    // On the audio lane nothing happens.
    let before = h.shell.editor().document().clone();
    drag(&mut h, row(at), pos2(560.0, 340.0));
    assert_eq!(*h.shell.editor().document(), before);

    // On the second MIDI track, at bar 8 of the lanes.
    drag(&mut h, row(at), pos2(560.0, 250.0));
    let doc = h.shell.editor().document();
    let placed = doc
        .track(midis[1])
        .unwrap()
        .clips()
        .next()
        .expect("the clip landed on the second MIDI track");
    assert_eq!(placed.start, Ticks(8 * BAR));
    assert_eq!(placed.name, "Idea");
}

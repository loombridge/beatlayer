//! Merging sets and saving tracks as a set by drag and drop, headless (WP-6.35;
//! the set exchange spec, R3, R4, R8, R11, R12): a set, or a track of one
//! opened in the browser, dragged onto a track's title bar is added after it;
//! track title bars and several Session clips dragged onto a folder in the
//! browser are saved there as a new set. Each gesture runs the registered
//! command an agent runs.

use crate::common;

use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_model::{Clip, ClipData, Command, Editor, MidiData, Note, SceneId, TrackId, TrackKind};
use bl_project::{Media, Project, SetFile};
use bl_shell::browser::places::create_user_library;
use bl_shell::browser_commands::{BrowserState, ItemKind};
use bl_ui::layout::mixer::{MixerLayout, Sections, SessionLayout, TrackColumn};
use bl_ui::layout::{REFERENCE, ShellLayout, TITLE_BAR};
use common::{Harness, Window};
use egui::accesskit::Role;
use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use serde_json::json;

const BAR: i64 = Ticks::WHOLE.get();

/// The list's first row's top, in window coordinates (the browser's tests'):
/// below the Filter View and the column header row.
const LIST_TOP: f32 = 227.0;

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-ui-set-exchange-{}-{name}", std::process::id()));
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

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The middle of the browser list's row `index`.
fn row(index: usize) -> Pos2 {
    pos2(160.0, LIST_TOP + 17.0 * index as f32 + 8.5)
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

/// The index of the browser row called `name`.
fn row_of(h: &Harness, name: &str) -> usize {
    let view = h.shell.browser_view();
    let names: Vec<String> = (0..view.row_count())
        .map(|i| view.row(i).unwrap().name)
        .collect();
    names
        .iter()
        .position(|n| n == name)
        .unwrap_or_else(|| panic!("{name} in {names:?}"))
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

fn names(h: &Harness) -> Vec<String> {
    let doc = h.shell.editor().document();
    doc.track_order()
        .into_iter()
        .map(|id| doc.track(id).unwrap())
        .filter(|t| !matches!(t.kind(), TrackKind::Main | TrackKind::Return))
        .map(|t| t.name().to_owned())
        .collect()
}

fn midi_clip(editor: &mut Editor, name: &str) -> Clip {
    let mut clip = Clip::new(
        editor.new_id(),
        Ticks::ZERO,
        Ticks(BAR),
        ClipData::Midi(MidiData::new([Note::new(
            60,
            Ticks::ZERO,
            Ticks(BAR / 4),
            100,
        )])),
    );
    clip.name = name.to_owned();
    clip
}

/// A set in `folder` holding a MIDI track "Drums" with a Session clip in the
/// first scene, an audio track "Pad" and a group "Strings" with an audio
/// track "Bass" in it.
fn source_set(folder: &Path) -> PathBuf {
    let mut editor = Editor::new();
    let add = |editor: &mut Editor, kind, name: &str, parent: Option<TrackId>| {
        let id: TrackId = editor.new_id();
        let index = match parent {
            Some(group) => editor.document().track(group).unwrap().children().len(),
            None => editor.document().top_level().len(),
        };
        editor
            .apply(Command::CreateTrack {
                id,
                kind,
                name: name.to_owned(),
                parent,
                index,
            })
            .unwrap();
        id
    };
    let drums = add(&mut editor, TrackKind::Midi, "Drums", None);
    add(&mut editor, TrackKind::Audio, "Pad", None);
    let group = add(&mut editor, TrackKind::Group, "Strings", None);
    add(&mut editor, TrackKind::Audio, "Bass", Some(group));
    let clip = midi_clip(&mut editor, "Beat");
    let scene: SceneId = editor.document().scenes().next().unwrap().id;
    editor
        .apply(Command::CreateSlotClip {
            track: drums,
            scene,
            clip,
        })
        .unwrap();
    std::fs::create_dir_all(folder).unwrap();
    let path = folder.join("Song.blset");
    SetFile::new(path.clone())
        .save(&Project::from_editor(&editor, Media::new()))
        .unwrap();
    path
}

/// A harness with the User Library under `root`, on the default set (two
/// MIDI tracks, two audio tracks, two returns), showing its folders.
fn harness(scratch: &Scratch) -> (Harness, PathBuf) {
    let root = scratch.0.join("User Library");
    create_user_library(&root).unwrap();
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.set_user_library(Some(root.clone()));
    h.settle();
    (h, root)
}

// Spec: set-exchange R3 (I-DROP-021). A set file dragged from the browser onto
// a track's title bar adds all its tracks but its returns after that track.
#[test]
fn a_set_dragged_onto_a_title_bar_adds_its_tracks_after_it() {
    let scratch = Scratch::new("set-on-title");
    let (mut h, root) = harness(&scratch);
    source_set(&root.join("Songs"));
    h.shell
        .run("browser.open_place", json!({ "label": "user_library" }))
        .unwrap();
    h.shell
        .run(
            "browser.open_folder",
            json!({ "path": text(&root.join("Songs")) }),
        )
        .unwrap();
    h.settle();
    let set_row = row_of(&h, "Song.blset");
    let before = names(&h);
    let steps = h.shell.editor().undo_history().len();

    let title = header_title(&mut h, "2 MIDI");
    drag(&mut h, row(set_row), title);
    assert_eq!(
        names(&h),
        [
            "1 MIDI", "2 MIDI", "Drums", "Pad", "Strings", "Bass", "3 Audio", "4 Audio"
        ],
        "the set's tracks went in after the track it was dropped on, from {before:?}"
    );
    assert_eq!(h.shell.editor().undo_history().len(), steps + 1);
}

// Spec: set-exchange R4, R8 (I-DROP-022). A track of an opened set, and its
// Devices row, are dragged onto title bars the same way.
#[test]
fn a_track_of_an_opened_set_dragged_onto_a_title_bar_is_added() {
    let scratch = Scratch::new("track-on-title");
    let (mut h, root) = harness(&scratch);
    let song = source_set(&root.join("Songs"));
    h.shell
        .run("browser.open_place", json!({ "label": "user_library" }))
        .unwrap();
    h.shell
        .run(
            "browser.open_folder",
            json!({ "path": text(&root.join("Songs")) }),
        )
        .unwrap();
    h.shell
        .run("browser.open_folder", json!({ "path": text(&song) }))
        .unwrap();
    h.shell.wait_for_browser_sets();
    h.settle();
    let state = browser(&mut h);
    assert!(state.rows.iter().any(|r| r.kind == ItemKind::Group));
    let strings = row_of(&h, "Strings");

    let title = header_title(&mut h, "4 Audio");
    drag(&mut h, row(strings), title);
    assert_eq!(
        names(&h),
        ["1 MIDI", "2 MIDI", "3 Audio", "4 Audio", "Strings", "Bass"],
        "a group comes with the tracks inside it"
    );
}

// Spec: set-exchange R11 (I-DROP-011). Track title bars dragged onto a folder
// in the browser are saved there as a new set, with the open set's returns;
// the open set is unchanged and the new row's name opens for editing.
#[test]
fn track_title_bars_dragged_onto_a_folder_are_saved_as_a_set() {
    let scratch = Scratch::new("tracks-on-folder");
    let (mut h, root) = harness(&scratch);
    h.shell
        .run("browser.open_place", json!({ "label": "user_library" }))
        .unwrap();
    h.settle();
    let clips = row_of(&h, "Clips");
    let before = h.shell.editor().document().clone();
    let steps = h.shell.editor().undo_history().len();

    // Two title bars selected, one dragged.
    let first = header_title(&mut h, "1 MIDI");
    let doc = h.shell.editor().document();
    let ids: Vec<TrackId> = doc
        .top_level()
        .filter(|id| matches!(doc.track(*id).unwrap().name(), "1 MIDI" | "3 Audio"))
        .collect();
    h.shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": ids, "title_bars": true }),
        )
        .unwrap();
    h.settle();
    drag(&mut h, first, row(clips));

    let saved = root.join("Clips").join("Clips Project").join("Clips.blset");
    let saved = if saved.is_file() {
        saved
    } else {
        // Named after the open set, placed as Save As places a set.
        let set = std::fs::read_dir(root.join("Clips"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| p.is_dir())
            .expect("a project folder was made");
        std::fs::read_dir(set)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| bl_project::is_set(p))
            .expect("the set")
    };
    let (project, _) = bl_project::load(&saved).unwrap();
    let doc = &project.document;
    let saved_names: Vec<String> = doc
        .top_level()
        .map(|id| doc.track(id).unwrap().name().to_owned())
        .collect();
    assert_eq!(saved_names, ["1 MIDI", "3 Audio"]);
    assert_eq!(doc.returns().count(), 2, "the open set's returns came too");
    assert_eq!(*h.shell.editor().document(), before, "the open set stayed");
    assert_eq!(h.shell.editor().undo_history().len(), steps);
    let state = browser(&mut h);
    assert_eq!(state.renaming, Some(text(&saved)));
}

// Spec: set-exchange R12 (I-DROP-011). Several selected Session clips dragged
// onto a folder in the browser are saved as a new set.
#[test]
fn several_session_clips_dragged_onto_a_folder_are_saved_as_a_set() {
    let scratch = Scratch::new("clips-on-folder");
    let (mut h, root) = harness(&scratch);
    let doc = h.shell.editor().document();
    let (midi, scenes): (TrackId, Vec<SceneId>) = (
        doc.top_level().next().unwrap(),
        doc.scenes().map(|s| s.id).collect(),
    );
    let mut made = Vec::new();
    for scene in [scenes[0], scenes[1]] {
        let id: bl_model::ClipId = h.shell.new_id();
        made.push(id);
        h.shell.edit(|editor| {
            let mut clip = midi_clip(editor, "Idea");
            clip.id = id;
            editor
                .apply(Command::CreateSlotClip {
                    track: midi,
                    scene,
                    clip,
                })
                .unwrap();
        });
    }
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    h.shell
        .run("browser.open_place", json!({ "label": "user_library" }))
        .unwrap();
    h.settle();
    let clips = row_of(&h, "Clips");
    // The two clips' slots are selected: the first and second scene of the
    // first track.
    for (scene, mode) in [(scenes[0], "replace"), (scenes[1], "add")] {
        h.shell
            .run(
                "session.select",
                json!({ "track": midi, "scene": scene, "mode": mode }),
            )
            .unwrap();
    }
    h.settle();

    let view = ShellLayout::compute(REFERENCE, true, true)
        .translate(vec2(0.0, TITLE_BAR))
        .main_view;
    let session = SessionLayout::compute(view, Some(Sections::DEFAULT));
    let columns = h.shell.mixer_columns();
    let layout = MixerLayout::compute(
        session.title_bars,
        &vec![TrackColumn::track(); columns.len()],
        h.shell.editor().document().returns().count(),
        Sections::DEFAULT,
        0.0,
    );
    let slot = pos2(layout.columns[0].rect.center().x, session.slots.top() + 8.0);
    drag(&mut h, slot, row(clips));

    let sets: Vec<PathBuf> = walk(&root.join("Clips"))
        .into_iter()
        .filter(|p| bl_project::is_set(p))
        .collect();
    assert_eq!(sets.len(), 1, "{sets:?}");
    let (project, _) = bl_project::load(&sets[0]).unwrap();
    let doc = &project.document;
    assert_eq!(
        doc.top_level().count(),
        1,
        "a track for the track they are on"
    );
    assert_eq!(doc.scene_count(), 2);
    let track = doc.track(doc.top_level().next().unwrap()).unwrap();
    assert_eq!(track.slots().count(), 2);
}

fn walk(folder: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if let Ok(entries) = std::fs::read_dir(folder) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                found.extend(walk(&path));
            } else {
                found.push(path);
            }
        }
    }
    found
}

//! Linked clips in the shell (WP-6.44), driven headless through
//! `Shell::run`: Duplicate as Linked Clip in both views, Unlink Clip, a split
//! of a linked clip, the plain copies that stay independent, and what the
//! menus offer. The behaviour is in the linked clips spec.

use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_model::{
    Clip, ClipData, ClipId, Command, Document, Editor, MidiData, Note, SceneId, TrackId,
};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::editing_commands::EditResult;
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::settings::Preferences;
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();

/// Opens a set held in memory.
struct Preloaded(Option<Editor>);

impl SetStore for Preloaded {
    fn open(&mut self, path: &Path) -> Result<(Editor, PathBuf), String> {
        let editor = self.0.take().ok_or("opened already")?;
        Ok((editor, path.to_owned()))
    }

    fn save(&mut self, _: &Path, _: &Editor) -> Result<Vec<String>, String> {
        Err("not saved".to_owned())
    }

    fn save_as(&mut self, _: &Path, _: &Editor) -> Result<(PathBuf, Vec<String>), String> {
        Err("not saved".to_owned())
    }

    fn close(&mut self) {}
}

/// The factory default set with a two-note MIDI clip on its first track,
/// one in the Arrangement and one in the first slot.
struct Song {
    shell: Shell,
    track: TrackId,
    arranged: ClipId,
    slotted: ClipId,
}

fn notes() -> ClipData {
    ClipData::Midi(MidiData::new([
        Note::new(60, Ticks::ZERO, Ticks(240), 100),
        Note::new(64, Ticks(240), Ticks(240), 100),
    ]))
}

fn song() -> Song {
    let mut editor = factory_default_set();
    let track = editor.document().top_level().next().unwrap();
    let arranged: ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(arranged, Ticks(BAR), Ticks(BAR * 2), notes()),
        })
        .unwrap();
    let scene = editor.document().scenes().next().unwrap().id;
    let slotted: ClipId = editor.new_id();
    editor
        .apply(Command::CreateSlotClip {
            track,
            scene,
            clip: Clip::new(slotted, Ticks::ZERO, Ticks(BAR * 2), notes()),
        })
        .unwrap();
    editor.clear_history().unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Linked.blset" }))
        .unwrap();
    Song {
        shell,
        track,
        arranged,
        slotted,
    }
}

impl Song {
    fn doc(&self) -> &Document {
        self.shell.editor().document()
    }

    fn run(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}"))
    }

    fn keys(&self, clip: ClipId) -> Vec<u8> {
        let ClipData::Midi(midi) = &self.doc().clip(clip).unwrap().data else {
            panic!("not MIDI");
        };
        let mut keys: Vec<u8> = midi.notes.iter().map(|n| n.key).collect();
        keys.sort_unstable();
        keys
    }

    fn last_step(&self) -> String {
        self.shell
            .editor()
            .undo_history()
            .next()
            .unwrap()
            .to_owned()
    }
}

// Spec: linked-clips R1, R2
#[test]
fn duplicate_as_linked_clip_puts_a_linked_copy_after_the_clip_in_one_step() {
    let mut song = song();
    let steps = song.shell.editor().undo_history().len();
    let result: EditResult = serde_json::from_value(
        song.run("clip.duplicate_linked", json!({ "clips": [song.arranged] })),
    )
    .unwrap();
    let [copy] = result.created[..] else {
        panic!("one copy");
    };
    let (origin, made) = (
        song.doc().clip(song.arranged).unwrap(),
        song.doc().clip(copy).unwrap(),
    );
    assert_eq!(
        made.start,
        origin.end(),
        "right after the clip, as Duplicate puts it"
    );
    assert!(origin.link.is_some());
    assert_eq!(origin.link, made.link);
    assert_eq!(song.shell.editor().undo_history().len(), steps + 1);
    assert_eq!(song.last_step(), "Duplicate as Linked Clip");
    assert_eq!(result.selected_clips, [copy], "the copy is selected");
    // A note edit through the copy reaches the clip.
    let mut data = made.data.clone();
    if let ClipData::Midi(midi) = &mut data {
        midi.notes
            .push_back(Note::new(67, Ticks(480), Ticks(240), 90));
    }
    song.run("clip.set_data", json!({ "clip": copy, "data": data }));
    assert_eq!(song.keys(song.arranged), [60, 64, 67]);
    song.doc().check_invariants().unwrap();
}

// Spec: linked-clips R7
#[test]
fn duplicate_makes_an_independent_copy() {
    let mut song = song();
    let result: EditResult = serde_json::from_value(
        song.run("arrangement.duplicate", json!({ "clips": [song.arranged] })),
    )
    .unwrap();
    let copy = result.created[0];
    assert_eq!(song.doc().clip(copy).unwrap().link, None);
    assert_eq!(song.doc().clip(song.arranged).unwrap().link, None);
}

// Spec: linked-clips R1
#[test]
fn in_the_session_view_the_copy_goes_to_the_first_empty_slot_below() {
    let mut song = song();
    let scenes: Vec<SceneId> = song.doc().scenes().map(|scene| scene.id).collect();
    let result: EditResult = serde_json::from_value(
        song.run("clip.duplicate_linked", json!({ "clips": [song.slotted] })),
    )
    .unwrap();
    let copy = result.created[0];
    let held = song.doc().track(song.track).unwrap();
    assert_eq!(held.slot(scenes[1]).map(|clip| clip.id), Some(copy));
    assert_eq!(
        song.doc().clip(copy).unwrap().link,
        song.doc().clip(song.slotted).unwrap().link
    );
    assert!(song.doc().clip(copy).unwrap().link.is_some());
    // The next one skips the slot taken, and a scene is added past the last.
    let before = song.doc().scene_count();
    let mut from = copy;
    for _ in 1..before {
        let next: EditResult =
            serde_json::from_value(song.run("clip.duplicate_linked", json!({ "clips": [from] })))
                .unwrap();
        from = next.created[0];
    }
    assert_eq!(song.doc().scene_count(), before + 1);
    song.doc().check_invariants().unwrap();
}

// Spec: linked-clips R8
#[test]
fn unlink_clip_takes_a_clip_out_and_the_link_ends_with_one_left() {
    let mut song = song();
    song.run("clip.duplicate_linked", json!({ "clips": [song.arranged] }));
    song.run("clip.unlink", json!({ "clips": [song.arranged] }));
    assert_eq!(song.last_step(), "Unlink Clip");
    assert_eq!(song.doc().clip(song.arranged).unwrap().link, None);
    let copy = song
        .doc()
        .track(song.track)
        .unwrap()
        .clips()
        .find(|clip| clip.id != song.arranged)
        .unwrap();
    assert_eq!(copy.link, None, "a link left with one clip ends");
}

// Spec: linked-clips R1, R8
#[test]
fn the_menus_offer_the_items_while_there_is_something_to_act_on() {
    let mut song = song();
    assert!(
        !song
            .shell
            .can_invoke("clip.unlink", &json!({ "clips": [song.arranged] }))
    );
    assert!(song.shell.can_invoke(
        "clip.duplicate_linked",
        &json!({ "clips": [song.arranged] })
    ));
    song.run("clip.duplicate_linked", json!({ "clips": [song.arranged] }));
    assert!(
        song.shell
            .can_invoke("clip.unlink", &json!({ "clips": [song.arranged] }))
    );
}

// Spec: linked-clips R6
#[test]
fn splitting_a_linked_clip_keeps_both_parts_in_the_link() {
    let mut song = song();
    song.run("clip.duplicate_linked", json!({ "clips": [song.arranged] }));
    let link = song.doc().clip(song.arranged).unwrap().link.unwrap();
    song.run(
        "arrangement.split",
        json!({ "clips": [song.arranged], "at": [BAR * 2] }),
    );
    let members = song.doc().link_members(link);
    assert_eq!(members.len(), 3, "{members:?}");
    song.doc().check_invariants().unwrap();
}

// Spec: linked-clips R16
#[test]
fn linked_clips_and_linked_tracks_work_side_by_side() {
    let mut song = song();
    // The first two tracks are linked; the second holds a clip of its own at
    // the first clip's place.
    let tracks: Vec<TrackId> = song.doc().top_level().take(2).collect();
    let (first, second) = (tracks[0], tracks[1]);
    song.run("track.link", json!({ "tracks": [first, second] }));
    let beside: ClipId = song.shell.new_id();
    let mut clip = Clip::new(beside, Ticks(BAR), Ticks(BAR * 2), notes());
    clip.name = "Beside".to_owned();
    song.run("clip.create", json!({ "track": second, "clip": clip }));
    song.run("clip.duplicate_linked", json!({ "clips": [song.arranged] }));
    let copy = song
        .doc()
        .track(first)
        .unwrap()
        .clips()
        .find(|clip| clip.id != song.arranged)
        .unwrap()
        .id;

    // Moving the clip moves the clip on the linked track as linked-track
    // editing says, and keeps it in its link.
    song.run(
        "arrangement.move_clips",
        json!({ "clips": [song.arranged], "by": BAR }),
    );
    assert_eq!(
        song.doc().clip(song.arranged).unwrap().start,
        Ticks(2 * BAR)
    );
    assert_eq!(song.doc().clip(beside).unwrap().start, Ticks(2 * BAR));
    assert_eq!(song.doc().linked_clip_ids(song.arranged), [copy]);

    // Changing its contents changes the clips of its link, and not the one
    // on the linked track.
    let mut data = song.doc().clip(song.arranged).unwrap().data.clone();
    if let ClipData::Midi(midi) = &mut data {
        midi.notes
            .push_back(Note::new(72, Ticks(480), Ticks(240), 90));
    }
    song.run(
        "clip.set_data",
        json!({ "clip": song.arranged, "data": data }),
    );
    assert_eq!(song.keys(copy), [60, 64, 72]);
    assert_eq!(song.keys(beside), [60, 64]);
    song.doc().check_invariants().unwrap();
}

//! The Arrangement's edits, driven headless through `Shell::run`,
//! `Shell::invoke`, and `Shell::gesture`, as agents, keys and menus, and
//! the mouse drive them (WP-1.8b; D17). Every edit is one undo step, and
//! undo and redo restore the set exactly. The behaviour is in
//! `docs/specs/arrangement-editing.md`.

use std::path::{Path, PathBuf};

use bl_commands::Permission;
use bl_core::time::Ticks;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, Document, Editor, TrackId,
};
use bl_shell::arrangement_commands::{self as view_ids, ArrangementState};
use bl_shell::audio::AudioHost;
use bl_shell::editing_commands::{self as ids, ClipboardState, EditResult};
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::settings::Preferences;
use bl_shell::{GesturePhase, Invocation, Shell, ShellError};
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

    fn close(&mut self) {}
}

/// The factory default set (1 MIDI, 2 MIDI, 3 Audio, 4 Audio, two
/// returns) with audio clips on its audio tracks: `(track 2 or 3, start
/// bar from 0, bars)`.
struct Song {
    shell: Shell,
    clips: Vec<ClipId>,
    tracks: Vec<TrackId>,
}

fn song(clips: &[(usize, i64, i64)]) -> Song {
    let mut editor = factory_default_set();
    let tracks: Vec<TrackId> = editor.document().top_level().collect();
    let mut ids = Vec::new();
    for (index, (track, start, bars)) in clips.iter().enumerate() {
        let id: ClipId = editor.new_id();
        let mut clip = Clip::new(
            id,
            Ticks(start * BAR),
            Ticks(bars * BAR),
            ClipData::Audio(AudioData {
                media: ContentHash([index as u8; 32]),
            }),
        );
        clip.name = format!("Clip {index}");
        editor
            .apply(Command::CreateClip {
                track: tracks[*track],
                clip,
            })
            .unwrap();
        ids.push(id);
    }
    // The set as it opens: no history.
    editor.clear_history().unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Clips.blset" }))
        .unwrap();
    Song {
        shell,
        clips: ids,
        tracks,
    }
}

impl Song {
    fn doc(&self) -> &Document {
        self.shell.editor().document()
    }

    fn clip(&self, id: ClipId) -> Clip {
        self.doc()
            .clip(id)
            .unwrap_or_else(|| panic!("clip {id:?}"))
            .clone()
    }

    /// `(start bar, end bar, offset in bars)` of each clip on a track, by
    /// start.
    fn lane(&self, track: usize) -> Vec<(f64, f64, f64)> {
        let mut clips: Vec<(f64, f64, f64)> = self
            .doc()
            .track(self.tracks[track])
            .unwrap()
            .clips()
            .map(|c| {
                (
                    c.start.get() as f64 / BAR as f64,
                    c.end().get() as f64 / BAR as f64,
                    c.offset.get() as f64 / BAR as f64,
                )
            })
            .collect();
        clips.sort_by(|a, b| a.0.total_cmp(&b.0));
        clips
    }

    fn edit(&mut self, id: &str, args: Value) -> EditResult {
        let value = self
            .shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}"));
        serde_json::from_value(value).unwrap()
    }

    fn view(&mut self, id: &str, args: Value) -> ArrangementState {
        serde_json::from_value(
            self.shell
                .run(id, args)
                .unwrap_or_else(|e| panic!("{id} failed: {e}")),
        )
        .unwrap()
    }

    fn state(&mut self) -> ArrangementState {
        self.view(view_ids::STATE, json!({}))
    }

    fn steps(&self) -> usize {
        self.shell.editor().undo_history().len()
    }

    /// Runs an edit, checks it's exactly one undo step, and that undo and
    /// redo restore the set exactly, then leaves it done.
    fn one_step(&mut self, id: &str, args: Value) -> EditResult {
        let before = self.doc().clone();
        let steps = self.steps();
        let result = self.edit(id, args);
        let after = self.doc().clone();
        assert_ne!(after, before, "{id} changed nothing");
        assert!(result.undo_step, "{id}");
        assert_eq!(self.steps(), steps + 1, "{id} is one undo step");
        self.shell.run("edit.undo", json!({})).unwrap();
        assert_eq!(*self.doc(), before, "undoing {id} restores the set exactly");
        assert!(self.doc().check_invariants().is_ok());
        self.shell.run("edit.redo", json!({})).unwrap();
        assert_eq!(*self.doc(), after, "redoing {id} restores it exactly");
        result
    }
}

#[test]
fn every_edit_is_registered_with_its_class_and_bindings() {
    // Spec: arrangement-editing R14.
    let shell = Shell::headless();
    let registry = shell.registry();
    for id in ids::IDS {
        let spec = registry.get(id).unwrap_or_else(|| panic!("{id}"));
        assert!(spec.args.is_some() && spec.result.is_some(), "{id}");
        assert!(!spec.description.is_empty(), "{id}");
    }
    assert!(registry.problems().is_empty(), "{:?}", registry.problems());
    let class = |id: &str| registry.get(id).unwrap().permission.unwrap();
    for id in [ids::DELETE, ids::CUT] {
        assert_eq!(class(id), Permission::Destructive, "{id}");
    }
    for id in [
        ids::MOVE_CLIPS,
        ids::RESIZE_CLIPS,
        ids::SPLIT,
        ids::DUPLICATE,
        ids::PASTE,
        ids::NUDGE,
    ] {
        assert_eq!(class(id), Permission::Edit, "{id}");
    }
    for id in [ids::SELECT_CLIPS, ids::SELECT_ALL, ids::COPY] {
        assert_eq!(class(id), Permission::Read, "{id}");
    }
    let keys = |id: &str| -> Vec<String> {
        registry
            .get(id)
            .unwrap()
            .bindings
            .iter()
            .map(|b| b.shortcut.to_string())
            .collect()
    };
    assert_eq!(keys(ids::CUT), ["S-EDIT-001"]);
    assert_eq!(keys(ids::COPY), ["S-EDIT-002"]);
    assert_eq!(keys(ids::PASTE), ["S-EDIT-003"]);
    assert_eq!(keys(ids::DUPLICATE), ["S-EDIT-004"]);
    assert_eq!(keys(ids::DELETE), ["S-EDIT-005"]);
    assert_eq!(keys(ids::SELECT_ALL), ["S-EDIT-009"]);
    assert_eq!(keys(ids::SPLIT), ["S-ARR-001"]);
    assert_eq!(keys(ids::RESIZE_CLIPS), ["S-ARR-004"]);
    // `clip.delete` gave up Delete in the Arrangement to the selection.
    assert!(registry.get("clip.delete").unwrap().bindings.is_empty());
}

#[test]
fn selecting_clips_and_time_is_never_an_undo_step() {
    // Spec: arrangement-editing R1, R2, R3.
    let mut s = song(&[(2, 0, 2), (2, 4, 2), (3, 2, 2), (3, 8, 1)]);
    let [a, b, c, d] = s.clips[..] else { panic!() };
    let state = s.view(ids::SELECT_CLIPS, json!({ "clips": [b] }));
    assert_eq!(state.selected_clips, [b]);
    assert_eq!(state.selected_tracks, [s.tracks[2]]);
    let selection = state.time_selection.unwrap();
    assert_eq!(
        (selection.start, selection.end),
        (Ticks(4 * BAR), Ticks(6 * BAR))
    );
    // Cmd-click adds and removes; Shift-click takes in what lies between.
    let state = s.view(ids::SELECT_CLIPS, json!({ "clips": [d], "mode": "toggle" }));
    assert_eq!(state.selected_clips, [b, d]);
    assert_eq!(state.selected_tracks, [s.tracks[2], s.tracks[3]]);
    let state = s.view(ids::SELECT_CLIPS, json!({ "clips": [b], "mode": "toggle" }));
    assert_eq!(state.selected_clips, [d]);
    s.view(ids::SELECT_CLIPS, json!({ "clips": [a] }));
    let state = s.view(ids::SELECT_CLIPS, json!({ "clips": [c], "mode": "extend" }));
    assert_eq!(state.selected_clips, [a, c]);
    let state = s.view(ids::SELECT_CLIPS, json!({ "clips": [d], "mode": "extend" }));
    assert_eq!(state.selected_clips, [a, c, b, d]);
    let state = s.view(ids::SELECT_CLIPS, json!({ "clips": [], "mode": "replace" }));
    assert!(state.selected_clips.is_empty());

    // Select All.
    let state = s.view(ids::SELECT_ALL, json!({}));
    assert_eq!(state.selected_clips, [a, c, b, d]);
    // Placing the insert marker or selecting time deselects clips.
    let state = s.view(
        view_ids::SET_INSERT_MARKER,
        json!({ "at": BAR, "track": s.tracks[2] }),
    );
    assert!(state.selected_clips.is_empty());
    s.view(ids::SELECT_CLIPS, json!({ "clips": [a] }));
    let state = s.view(
        view_ids::SELECT_TIME,
        json!({ "start": 0, "end": BAR, "tracks": [s.tracks[3]] }),
    );
    assert!(state.selected_clips.is_empty());

    assert!(!s.shell.editor().can_undo(), "selecting isn't an undo step");
    assert!(!s.shell.is_dirty(), "nor does it change the set");
    // Unknown clips are refused.
    assert!(matches!(
        s.shell
            .run(ids::SELECT_CLIPS, json!({ "clips": [999_999] })),
        Err(ShellError::InvalidArgs { .. })
    ));
}

#[test]
fn moving_clips_is_one_step_in_time_and_between_tracks() {
    // Spec: arrangement-editing R4, R13.
    let mut s = song(&[(2, 0, 2), (3, 1, 1)]);
    let [a, b] = s.clips[..] else { panic!() };
    // An agent moves both, placing the first at bar 5 with a position in
    // bars and beats; the other keeps its distance.
    let result = s.one_step(ids::MOVE_CLIPS, json!({ "clips": [a, b], "to": "5.1.1" }));
    assert_eq!(s.clip(a).start, Ticks(4 * BAR));
    assert_eq!(s.clip(b).start, Ticks(5 * BAR));
    assert_eq!(result.selected_clips, [a, b]);
    assert!(result.created.is_empty() && result.deleted.is_empty());
    assert_eq!(s.shell.editor().undo_history().next(), Some("Move clips"));
    assert!(s.shell.is_dirty());

    // Up a track: the top clip goes to 3 Audio's neighbour above... which
    // is a MIDI track, so nothing moves.
    let before = s.doc().clone();
    assert!(matches!(
        s.shell.run(
            ids::MOVE_CLIPS,
            json!({ "clips": [a], "track": s.tracks[1] })
        ),
        Err(ShellError::Failed { .. })
    ));
    assert_eq!(*s.doc(), before);
    // Down a track, with the selection: each clip moves down one.
    s.view(ids::SELECT_CLIPS, json!({ "clips": [a] }));
    s.one_step(ids::MOVE_CLIPS, json!({ "track": s.tracks[3], "by": BAR }));
    assert_eq!(s.doc().clip_track(a), Some(s.tracks[3]));
    assert_eq!(s.clip(a).start, Ticks(5 * BAR));
    // Never before 1.1.1.
    assert!(
        s.shell
            .run(ids::MOVE_CLIPS, json!({ "clips": [b], "by": -6 * BAR }))
            .is_err()
    );
}

#[test]
fn copying_leaves_the_originals_and_selects_the_copies() {
    // Spec: arrangement-editing R5.
    let mut s = song(&[(2, 0, 2)]);
    let a = s.clips[0];
    let result = s.one_step(
        ids::MOVE_CLIPS,
        json!({ "clips": [a], "by": 4 * BAR, "track": s.tracks[3], "copy": true }),
    );
    assert_eq!(result.created.len(), 1);
    let copy = result.created[0];
    assert_eq!(result.selected_clips, [copy]);
    assert_eq!(s.clip(a).start, Ticks::ZERO, "the original stays");
    let copied = s.clip(copy);
    assert_eq!(
        (copied.start, copied.length),
        (Ticks(4 * BAR), Ticks(2 * BAR))
    );
    assert_eq!(copied.name, "Clip 0");
    assert_eq!(copied.data, s.clip(a).data);
    assert_eq!(s.shell.editor().undo_history().next(), Some("Copy clips"));
}

#[test]
fn a_clip_placed_over_another_replaces_what_it_covers() {
    // Spec: arrangement-editing R6, R16.
    // 3 Audio: [0, 8). 4 Audio: a short clip to drop over it.
    let mut s = song(&[(2, 0, 8), (3, 0, 2)]);
    let [long, short] = s.clips[..] else { panic!() };
    // In the middle: the long clip is split around it, its second part
    // playing on from where it was.
    s.one_step(
        ids::MOVE_CLIPS,
        json!({ "clips": [short], "track": s.tracks[2], "to": "4.1.1", "copy": true }),
    );
    assert_eq!(
        s.lane(2),
        [(0.0, 3.0, 0.0), (3.0, 5.0, 0.0), (5.0, 8.0, 5.0)]
    );
    let right = s
        .doc()
        .track(s.tracks[2])
        .unwrap()
        .clips()
        .find(|c| c.start == Ticks(5 * BAR))
        .unwrap()
        .clone();
    assert_eq!(
        right.content_start(),
        s.clip(long).content_start(),
        "both parts play the same media from the same place"
    );
    // Over its start: the long clip is trimmed from the left, and plays
    // on from where it was.
    let mut s = song(&[(2, 1, 8), (3, 0, 2)]);
    let [_, short] = s.clips[..] else { panic!() };
    s.one_step(
        ids::MOVE_CLIPS,
        json!({ "clips": [short], "track": s.tracks[2] }),
    );
    assert_eq!(s.lane(2), [(0.0, 2.0, 0.0), (2.0, 9.0, 1.0)]);
    // Over its end: shortened. Over all of it: deleted.
    let mut s = song(&[(2, 0, 3), (3, 2, 2), (2, 6, 1), (3, 6, 2)]);
    let [_, b, c, d] = s.clips[..] else { panic!() };
    let result = s.one_step(
        ids::MOVE_CLIPS,
        json!({ "clips": [b, d], "track": s.tracks[2] }),
    );
    assert_eq!(
        s.lane(2),
        [(0.0, 2.0, 0.0), (2.0, 4.0, 0.0), (6.0, 8.0, 0.0)]
    );
    assert_eq!(result.deleted, [c]);
    assert!(s.lane(3).is_empty());
}

#[test]
fn resizing_moves_edges_and_keeps_contents_in_place() {
    // Spec: arrangement-editing R7, R16.
    let mut s = song(&[(2, 2, 4), (2, 8, 2)]);
    let [a, b] = s.clips[..] else { panic!() };
    // The end edge, to a position.
    s.one_step(
        ids::RESIZE_CLIPS,
        json!({ "clips": [a], "edge": "end", "to": "6.1.1" }),
    );
    assert_eq!(s.lane(2), [(2.0, 5.0, 0.0), (8.0, 10.0, 0.0)]);
    // The start edge, later: the clip shows less, the rest stays put.
    s.one_step(
        ids::RESIZE_CLIPS,
        json!({ "clips": [a], "edge": "start", "by": BAR }),
    );
    assert_eq!(s.lane(2), [(3.0, 5.0, 1.0), (8.0, 10.0, 0.0)]);
    // Back out, but no further than its contents begin.
    s.one_step(
        ids::RESIZE_CLIPS,
        json!({ "clips": [a], "edge": "start", "by": -BAR }),
    );
    assert!(matches!(
        s.shell.run(
            ids::RESIZE_CLIPS,
            json!({ "clips": [a], "edge": "start", "by": -BAR })
        ),
        Err(ShellError::Failed { .. })
    ));
    // Lengthening over a neighbour replaces what it covers.
    s.one_step(
        ids::RESIZE_CLIPS,
        json!({ "clips": [a], "edge": "end", "to": "10.1.1" }),
    );
    assert_eq!(s.lane(2), [(2.0, 9.0, 0.0), (9.0, 10.0, 1.0)]);
    assert_eq!(s.clip(b).start, Ticks(9 * BAR));
    // Never shorter than a tick.
    assert!(
        s.shell
            .run(
                ids::RESIZE_CLIPS,
                json!({ "clips": [b], "edge": "end", "by": -BAR })
            )
            .is_err()
    );
}

#[test]
fn splitting_keeps_what_each_part_plays() {
    // Spec: arrangement-editing R8, R16.
    let mut s = song(&[(2, 0, 4), (3, 0, 4)]);
    let [a, b] = s.clips[..] else { panic!() };
    // At the insert marker, on the selected track (Cmd-E).
    s.view(
        view_ids::SET_INSERT_MARKER,
        json!({ "at": BAR, "track": s.tracks[2] }),
    );
    let result = s.one_step(ids::SPLIT, json!({}));
    assert_eq!(result.created.len(), 1);
    assert_eq!(s.lane(2), [(0.0, 1.0, 0.0), (1.0, 4.0, 1.0)]);
    assert_eq!(s.lane(3), [(0.0, 4.0, 0.0)], "only the selected track");
    let part = s.clip(result.created[0]);
    assert_eq!(part.content_start(), s.clip(a).content_start());
    assert_eq!(
        (part.name.as_str(), part.data.clone()),
        ("Clip 0", s.clip(a).data)
    );
    // At a time selection's edges: the part inside is selected.
    s.view(
        view_ids::SELECT_TIME,
        json!({ "start": 2 * BAR, "end": 3 * BAR, "tracks": [s.tracks[3]] }),
    );
    let result = s.one_step(ids::SPLIT, json!({}));
    assert_eq!(
        s.lane(3),
        [(0.0, 2.0, 0.0), (2.0, 3.0, 2.0), (3.0, 4.0, 3.0)]
    );
    assert_eq!(result.selected_clips.len(), 1);
    assert_eq!(s.clip(result.selected_clips[0]).start, Ticks(2 * BAR));
    // An agent splits a clip at explicit positions.
    let result = s.one_step(
        ids::SPLIT,
        json!({ "clips": [b], "at": ["1.3", 3 * BAR / 2] }),
    );
    assert_eq!(result.created.len(), 2);
    // Nowhere inside a clip: nothing to split, and no undo step.
    let result = s.edit(ids::SPLIT, json!({ "clips": [b], "at": ["9"] }));
    assert!(!result.undo_step);
}

#[test]
fn deleting_clips_or_the_time_selected() {
    // Spec: arrangement-editing R9.
    let mut s = song(&[(2, 0, 4), (3, 0, 4), (3, 6, 2)]);
    let [a, _, c] = s.clips[..] else { panic!() };
    s.view(ids::SELECT_CLIPS, json!({ "clips": [a] }));
    let result = s.one_step(ids::DELETE, json!({}));
    assert_eq!(result.deleted, [a]);
    assert!(s.lane(2).is_empty());
    assert!(result.selected_clips.is_empty());
    // Time: what it covers goes, nothing after it moves.
    let result = s.one_step(
        ids::DELETE,
        json!({ "tracks": [s.tracks[3]], "start": "2.1.1", "end": "8.1.1" }),
    );
    assert_eq!(s.lane(3), [(0.0, 1.0, 0.0), (7.0, 8.0, 1.0)]);
    assert!(result.deleted.is_empty());
    assert!(s.doc().clip(c).is_some());
    // Nothing selected: Delete can't run.
    s.view(view_ids::SET_INSERT_MARKER, json!({ "at": 0 }));
    assert!(!s.shell.can_invoke(ids::DELETE, &json!({})));
    assert!(matches!(
        s.shell.run(ids::DELETE, json!({})),
        Err(ShellError::Unavailable { .. })
    ));
}

#[test]
fn duplicating_places_a_copy_after_and_selects_it() {
    // Spec: arrangement-editing R10.
    let mut s = song(&[(2, 0, 2), (3, 1, 2), (2, 5, 1)]);
    let [a, b, _] = s.clips[..] else { panic!() };
    s.view(ids::SELECT_CLIPS, json!({ "clips": [a, b], "mode": "add" }));
    // The two span bars 1 to 4: the copies go to bars 4 to 7, and cover
    // part of the clip at bar 6.
    let result = s.one_step(ids::DUPLICATE, json!({}));
    assert_eq!(result.created.len(), 2);
    assert_eq!(result.selected_clips, result.created);
    assert_eq!(
        s.lane(2),
        [(0.0, 2.0, 0.0), (3.0, 5.0, 0.0), (5.0, 6.0, 0.0)]
    );
    assert_eq!(s.lane(3), [(1.0, 3.0, 0.0), (4.0, 6.0, 0.0)]);
    // Again: the next copy follows the first.
    s.one_step(ids::DUPLICATE, json!({}));
    assert_eq!(
        s.lane(3),
        [(1.0, 3.0, 0.0), (4.0, 6.0, 0.0), (7.0, 9.0, 0.0)]
    );
    // Time: the material in the span, copied after it, and the selection
    // moves to the copy.
    let mut s = song(&[(2, 0, 4)]);
    s.view(
        view_ids::SELECT_TIME,
        json!({ "start": BAR, "end": 2 * BAR, "tracks": [s.tracks[2]] }),
    );
    let result = s.one_step(ids::DUPLICATE, json!({}));
    assert_eq!(
        s.lane(2),
        [(0.0, 2.0, 0.0), (2.0, 3.0, 1.0), (3.0, 4.0, 3.0)]
    );
    let time = result.time_selection.unwrap();
    assert_eq!((time.start, time.end), (Ticks(2 * BAR), Ticks(3 * BAR)));
}

#[test]
fn cut_copy_and_paste_carry_material_between_tracks() {
    // Spec: arrangement-editing R11.
    let mut s = song(&[(2, 0, 2), (3, 1, 2)]);
    let [a, b] = s.clips[..] else { panic!() };
    // Copy time across both audio tracks: changes nothing in the set.
    let state: ClipboardState = serde_json::from_value(
        s.shell
            .run(
                ids::COPY,
                json!({ "tracks": [s.tracks[2], s.tracks[3]], "start": 0, "end": 4 * BAR }),
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        (state.clips, state.tracks, state.length),
        (2, 2, Ticks(4 * BAR))
    );
    assert!(!s.shell.editor().can_undo(), "copying isn't an undo step");
    // Paste at bar 9 on the first audio track: both tracks get theirs.
    let result = s.one_step(ids::PASTE, json!({ "at": "9", "track": s.tracks[2] }));
    assert_eq!(result.created.len(), 2);
    assert_eq!(s.lane(2), [(0.0, 2.0, 0.0), (8.0, 10.0, 0.0)]);
    assert_eq!(s.lane(3), [(1.0, 3.0, 0.0), (9.0, 11.0, 0.0)]);
    let time = result.time_selection.unwrap();
    assert_eq!((time.start, time.end), (Ticks(8 * BAR), Ticks(12 * BAR)));
    // Onto a MIDI track: refused, nothing changes.
    let before = s.doc().clone();
    assert!(matches!(
        s.shell
            .run(ids::PASTE, json!({ "at": 0, "track": s.tracks[0] })),
        Err(ShellError::Failed { .. })
    ));
    assert_eq!(*s.doc(), before);
    // Cut a clip: it goes, and pastes at the insert marker.
    let cut = s.clip(b).data;
    s.view(ids::SELECT_CLIPS, json!({ "clips": [b] }));
    let result = s.one_step(ids::CUT, json!({}));
    assert_eq!(result.deleted, [b]);
    s.view(
        view_ids::SET_INSERT_MARKER,
        json!({ "at": 4 * BAR, "track": s.tracks[2] }),
    );
    let result = s.one_step(ids::PASTE, json!({}));
    assert_eq!(
        s.lane(2),
        [(0.0, 2.0, 0.0), (4.0, 6.0, 0.0), (8.0, 10.0, 0.0)]
    );
    assert_eq!(s.clip(result.created[0]).data, cut);
    assert_ne!(cut, s.clip(a).data);
    assert_eq!(s.shell.editor().undo_history().next(), Some("Paste"));
}

#[test]
fn nudging_moves_by_the_grid_or_moves_the_insert_marker() {
    // Spec: arrangement-editing R12.
    let mut s = song(&[(2, 1, 1)]);
    let a = s.clips[0];
    // Fix the grid at a quarter note so the step is known.
    s.view(view_ids::SET_GRID, json!({ "fixed": "quarter" }));
    s.view(ids::SELECT_CLIPS, json!({ "clips": [a] }));
    s.one_step(ids::NUDGE, json!({ "direction": "right" }));
    assert_eq!(s.clip(a).start, Ticks(BAR) + Ticks::QUARTER);
    s.one_step(ids::NUDGE, json!({ "direction": "left" }));
    assert_eq!(s.clip(a).start, Ticks(BAR));
    // With no clip selected the insert marker moves, which isn't an edit.
    s.view(view_ids::SET_INSERT_MARKER, json!({ "at": BAR }));
    let steps = s.steps();
    s.edit(ids::NUDGE, json!({ "direction": "right" }));
    assert_eq!(s.state().insert_marker, Ticks(BAR) + Ticks::QUARTER);
    assert_eq!(s.steps(), steps);
}

#[test]
fn a_gesture_is_one_undo_step_however_many_frames_it_spans() {
    // Spec: arrangement-editing R13. Each frame runs the move measured from
    // where the drag began; the commit records one step.
    let mut s = song(&[(2, 0, 1), (2, 4, 1)]);
    let [a, b] = s.clips[..] else { panic!() };
    s.view(ids::SELECT_CLIPS, json!({ "clips": [a] }));
    let frame = |by: i64| Invocation::new(ids::MOVE_CLIPS, json!({ "by": by }));
    let steps = s.steps();
    for step in 1..=12 {
        s.shell
            .gesture(GesturePhase::Update, Some(&frame(step * BAR / 4)))
            .unwrap();
        // The set follows the drag as it goes.
        assert_eq!(s.clip(a).start, Ticks(step * BAR / 4));
        assert!(s.shell.gesture_open());
        // Keys and other edits wait for it.
        assert!(!s.shell.can_invoke("edit.undo", &json!({})));
        assert!(s.shell.run(ids::DELETE, json!({})).is_err());
    }
    // Over the second clip on the way (covering it), then past it.
    s.shell
        .gesture(GesturePhase::Update, Some(&frame(4 * BAR)))
        .unwrap();
    assert!(s.doc().clip(b).is_none(), "the clip it covers is replaced");
    // A frame that's refused (before 1.1.1) keeps the last that worked.
    assert!(
        s.shell
            .gesture(GesturePhase::Update, Some(&frame(-BAR)))
            .is_err()
    );
    assert_eq!(s.clip(a).start, Ticks(4 * BAR));
    s.shell
        .gesture(GesturePhase::Commit, Some(&frame(6 * BAR)))
        .unwrap();
    assert!(!s.shell.gesture_open());
    assert_eq!(s.clip(a).start, Ticks(6 * BAR));
    assert!(s.doc().clip(b).is_some(), "it moved on past it");
    assert_eq!(s.steps(), steps + 1, "the whole drag is one undo step");
    assert_eq!(s.shell.editor().undo_history().next(), Some("Move clips"));
    let after = s.doc().clone();
    s.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(s.clip(a).start, Ticks::ZERO);
    s.shell.run("edit.redo", json!({})).unwrap();
    assert_eq!(*s.doc(), after);

    // Esc: a cancelled drag leaves the set and the selection as they were.
    let before = s.doc().clone();
    let selected = s.state().selected_clips;
    let copy = Invocation::new(ids::MOVE_CLIPS, json!({ "by": BAR, "copy": true }));
    s.shell.gesture(GesturePhase::Update, Some(&copy)).unwrap();
    assert_ne!(*s.doc(), before);
    s.shell.gesture(GesturePhase::Cancel, None).unwrap();
    assert_eq!(*s.doc(), before);
    assert_eq!(s.state().selected_clips, selected);
    assert_eq!(s.steps(), steps + 1);
}

#[test]
fn keys_and_menus_act_on_the_selection() {
    // Spec: arrangement-editing R14. With nothing given, each edit takes
    // the selection, as a key press or the Edit menu does through
    // `Shell::invoke`.
    let mut s = song(&[(2, 0, 2)]);
    let a = s.clips[0];
    for id in [ids::DELETE, ids::DUPLICATE, ids::COPY, ids::CUT] {
        assert!(
            !s.shell.can_invoke(id, &json!({})),
            "{id} with nothing selected"
        );
    }
    assert!(
        !s.shell.can_invoke(ids::PASTE, &json!({})),
        "nothing to paste"
    );
    s.view(ids::SELECT_CLIPS, json!({ "clips": [a] }));
    for id in [ids::DELETE, ids::DUPLICATE, ids::COPY, ids::CUT, ids::SPLIT] {
        assert!(s.shell.can_invoke(id, &json!({})), "{id}");
    }
    s.shell.invoke(ids::COPY, json!({})).unwrap();
    s.shell.invoke(ids::DUPLICATE, json!({})).unwrap();
    assert_eq!(s.lane(2), [(0.0, 2.0, 0.0), (2.0, 4.0, 0.0)]);
    assert!(s.shell.can_invoke(ids::PASTE, &json!({})));
    s.shell.invoke(ids::DELETE, json!({})).unwrap();
    assert_eq!(s.lane(2), [(0.0, 2.0, 0.0)]);
    // Undo names the step in the Edit menu.
    assert_eq!(
        s.shell.dynamic_label("edit.undo").as_deref(),
        Some("Undo Delete")
    );
}

/// A small deterministic generator, so a long run of random edits is the
/// same on every machine.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, below: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % below.max(1)
    }
}

#[test]
fn undo_and_redo_restore_every_step_of_a_long_session_exactly() {
    // Spec: arrangement-editing R13. Three hundred edits of every kind with
    // random targets; then undo every one, checking each version, and redo
    // every one.
    let mut s = song(&[(2, 0, 4), (3, 2, 4), (2, 8, 2), (3, 9, 3)]);
    let mut rng = Lcg(0x5eed);
    let mut versions = vec![s.doc().clone()];
    let mut made = 0;
    for _ in 0..300 {
        let doc = s.doc().clone();
        let mut all: Vec<ClipId> = s
            .tracks
            .iter()
            .flat_map(|t| doc.track(*t).unwrap().clips().map(|c| c.id))
            .collect();
        all.sort_unstable();
        let pick = |rng: &mut Lcg| all[rng.next(all.len() as u64) as usize];
        let audio = [s.tracks[2], s.tracks[3]];
        let bar = |rng: &mut Lcg| (rng.next(48) as i64) * BAR / 4;
        // Deleting is rarer than adding, and never takes the last clips.
        let choice = match rng.next(12) {
            4 | 5 if all.len() <= 3 => 6,
            n => n,
        };
        let (id, args) = match choice {
            0 | 1 => (
                ids::MOVE_CLIPS,
                json!({ "clips": [pick(&mut rng)], "to": bar(&mut rng),
                        "track": audio[rng.next(2) as usize], "copy": rng.next(2) == 0 }),
            ),
            2 => (
                ids::RESIZE_CLIPS,
                json!({ "clips": [pick(&mut rng)], "edge": "end",
                        "by": BAR / 4 * (rng.next(5) as i64 - 2) }),
            ),
            3 => (
                ids::RESIZE_CLIPS,
                json!({ "clips": [pick(&mut rng)], "edge": "start",
                        "by": BAR / 4 * (rng.next(5) as i64 - 2) }),
            ),
            4 => (ids::DELETE, json!({ "clips": [pick(&mut rng)] })),
            5 => {
                let start = bar(&mut rng);
                (
                    ids::DELETE,
                    json!({ "tracks": [audio[rng.next(2) as usize]], "start": start,
                            "end": start + BAR / 4 * (1 + rng.next(8) as i64) }),
                )
            }
            6 | 7 => (ids::DUPLICATE, json!({ "clips": [pick(&mut rng)] })),
            8 => {
                let from = bar(&mut rng);
                s.shell
                    .run(
                        ids::COPY,
                        json!({ "tracks": audio, "start": from, "end": from + 2 * BAR }),
                    )
                    .unwrap();
                (
                    ids::PASTE,
                    json!({ "at": bar(&mut rng), "track": audio[0] }),
                )
            }
            9 => (
                ids::SPLIT,
                json!({ "tracks": audio, "at": [bar(&mut rng)] }),
            ),
            10 => (
                ids::SPLIT,
                json!({ "clips": [pick(&mut rng)], "at": [bar(&mut rng), bar(&mut rng)] }),
            ),
            _ => (
                ids::NUDGE,
                json!({ "clips": [pick(&mut rng)],
                        "direction": if rng.next(2) == 0 { "left" } else { "right" } }),
            ),
        };
        let steps = s.steps();
        let outcome = s.shell.run(id, args.clone());
        match outcome {
            Ok(_) if s.steps() == steps + 1 => {
                made += 1;
                assert!(s.doc().check_invariants().is_ok(), "{id}");
                versions.push(s.doc().clone());
            }
            Ok(_) => assert_eq!(*s.doc(), doc, "{id} changed the set without a step"),
            Err(_) => assert_eq!(*s.doc(), doc, "a refused {id} left the set as it was"),
        }
        // No two clips on a track overlap after any edit.
        for track in audio {
            let mut spans: Vec<(Ticks, Ticks)> = s
                .doc()
                .track(track)
                .unwrap()
                .clips()
                .map(|c| (c.start, c.end()))
                .collect();
            spans.sort_unstable();
            for pair in spans.windows(2) {
                assert!(
                    pair[0].1 <= pair[1].0,
                    "{id} left clips overlapping: {spans:?}"
                );
            }
        }
    }
    assert!(made > 150, "only {made} edits changed the set");
    eprintln!("{made} edits, {} clips at the end", s.doc().clip_count());
    for version in versions.iter().rev().skip(1) {
        s.shell.run("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc(), version);
    }
    assert!(!s.shell.editor().can_undo());
    for version in versions.iter().skip(1) {
        s.shell.run("edit.redo", json!({})).unwrap();
        assert_eq!(s.doc(), version);
    }
}

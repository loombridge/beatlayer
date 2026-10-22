//! Fades and crossfades in the Arrangement's edits, driven headless through
//! `Shell::run` and `Shell::gesture` (the audio clips spec, R26 to R29):
//! the 4 ms crossfade of clips an edit leaves meeting, kept while they meet
//! and ended when they part; Create Fade and Delete Fades. Each is one undo
//! step.

use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, Document, EDGE_FADE_SECONDS, Editor,
    TrackId, crossfades,
};
use bl_shell::arrangement_commands as view_ids;
use bl_shell::audio::AudioHost;
use bl_shell::editing_commands::{self as ids, EditResult};
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::settings::Preferences;
use bl_shell::{GesturePhase, Invocation, Shell};
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();
const HALF: f64 = EDGE_FADE_SECONDS / 2.0;

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

/// The factory default set with audio clips on its third track: `(start bar,
/// bars, Start marker in seconds)`, with no fades.
struct Song {
    shell: Shell,
    clips: Vec<ClipId>,
    track: TrackId,
}

fn song(clips: &[(i64, i64, f64)]) -> Song {
    let mut editor = factory_default_set();
    let track = editor.document().top_level().nth(2).unwrap();
    let mut ids = Vec::new();
    for (index, (start, bars, marker)) in clips.iter().enumerate() {
        let id: ClipId = editor.new_id();
        let audio = AudioData {
            start: *marker,
            ..AudioData::new(ContentHash([index as u8; 32]))
        };
        let clip = Clip::new(
            id,
            Ticks(start * BAR),
            Ticks(bars * BAR),
            ClipData::Audio(audio),
        );
        editor.apply(Command::CreateClip { track, clip }).unwrap();
        ids.push(id);
    }
    editor.clear_history().unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Fades.blset" }))
        .unwrap();
    Song {
        shell,
        clips: ids,
        track,
    }
}

/// A clip's fades: `(fade_in, fade_in_before, fade_out, fade_out_after)`.
type Fades = (f64, f64, f64, f64);

impl Song {
    fn doc(&self) -> &Document {
        self.shell.editor().document()
    }

    fn audio(&self, clip: ClipId) -> AudioData {
        match &self.doc().clip(clip).expect("the clip").data {
            ClipData::Audio(audio) => audio.clone(),
            ClipData::Midi(_) => panic!("an audio clip"),
        }
    }

    fn fades(&self, clip: ClipId) -> Fades {
        let a = self.audio(clip);
        (a.fade_in, a.fade_in_before, a.fade_out, a.fade_out_after)
    }

    fn crossfades(&self) -> Vec<bl_model::Crossfade> {
        crossfades(self.doc().track(self.track).unwrap().clips())
    }

    fn steps(&self) -> usize {
        self.shell.editor().undo_history().len()
    }

    /// Runs a command the document owns, such as `clip.set_fades`.
    fn run(&mut self, id: &str, args: Value) {
        self.shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}"));
    }

    fn edit(&mut self, id: &str, args: Value) -> EditResult {
        let value = self
            .shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}"));
        serde_json::from_value(value).unwrap()
    }

    /// Runs an edit, checks it's exactly one undo step named `name`, and that
    /// undo and redo restore the set exactly, then leaves it done.
    fn one_step(&mut self, id: &str, args: Value, name: &str) -> EditResult {
        let before = self.doc().clone();
        let steps = self.steps();
        let result = self.edit(id, args);
        let after = self.doc().clone();
        assert_ne!(after, before, "{id} changed nothing");
        assert_eq!(self.steps(), steps + 1, "{id} is one undo step");
        assert_eq!(self.shell.editor().undo_history().next(), Some(name));
        self.shell.run("edit.undo", json!({})).unwrap();
        assert_eq!(*self.doc(), before, "undoing {id} restores the set exactly");
        self.shell.run("edit.redo", json!({})).unwrap();
        assert_eq!(*self.doc(), after, "redoing {id} restores it exactly");
        result
    }

    fn set_edge_fades(&mut self, on: bool) {
        self.shell
            .run(
                "settings.set_create_fades_on_clip_edges",
                json!({ "on": on }),
            )
            .unwrap();
    }
}

// Spec: audio-clips R26. A split gives its parts the 4 ms crossfade, 2 ms each
// side, in the same undo step; it ends when they part, and each edge goes back
// to a fade of 4 ms.
#[test]
fn a_split_crossfades_its_parts_and_parting_them_ends_it() {
    let mut s = song(&[(2, 4, 0.0)]);
    let [whole] = s.clips[..] else { panic!() };
    let result = s.one_step(
        ids::SPLIT,
        json!({ "clips": [whole], "at": [3 * BAR] }),
        "Split clips",
    );
    let [right] = result.created[..] else { panic!() };
    assert_eq!(s.fades(whole), (0.0, 0.0, HALF, HALF));
    assert_eq!(s.fades(right), (HALF, HALF, 0.0, 0.0));
    let found = s.crossfades();
    assert_eq!(found.len(), 1);
    assert_eq!(
        (found[0].earlier, found[0].later, found[0].boundary),
        (whole, right, Ticks(3 * BAR))
    );
    assert_eq!((found[0].before, found[0].after, found[0].curve), (HALF, HALF, 0.0));

    // Moving the right part away parts them: each edge takes back a fade of
    // 4 ms.
    s.one_step(
        ids::MOVE_CLIPS,
        json!({ "clips": [right], "by": 2 * BAR }),
        "Move clips",
    );
    assert!(s.crossfades().is_empty());
    assert_eq!(s.fades(whole), (0.0, 0.0, EDGE_FADE_SECONDS, 0.0));
    assert_eq!(s.fades(right), (EDGE_FADE_SECONDS, 0.0, 0.0, 0.0));
}

// Spec: audio-clips R26. A clip moved to meet another gets the crossfade; one
// that starts at the start of its file can't be read before its start (R25),
// so its 4 ms run past the boundary instead.
#[test]
fn clips_moved_to_meet_get_the_crossfade_where_their_files_allow() {
    let mut s = song(&[(0, 2, 0.0), (3, 2, 0.0), (6, 2, 5.0)]);
    let [a, b, c] = s.clips[..] else { panic!() };
    s.one_step(
        ids::MOVE_CLIPS,
        json!({ "clips": [b], "by": -BAR }),
        "Move clips",
    );
    assert_eq!(s.fades(a), (0.0, 0.0, 0.0, EDGE_FADE_SECONDS));
    assert_eq!(s.fades(b), (EDGE_FADE_SECONDS, 0.0, 0.0, 0.0));
    // A file with room before the clip's start gets 2 ms each side.
    s.one_step(
        ids::MOVE_CLIPS,
        json!({ "clips": [c], "by": -2 * BAR }),
        "Move clips",
    );
    assert_eq!(s.fades(b), (EDGE_FADE_SECONDS, 0.0, HALF, HALF));
    assert_eq!(s.fades(c), (HALF, HALF, 0.0, 0.0));
    assert_eq!(s.crossfades().len(), 2);
}

// Spec: audio-clips R26. A crossfade keeps its span and curve while its two
// clips go on meeting, wherever their boundary moves: both moved together, or
// one resized over the other.
#[test]
fn a_crossfade_stays_while_its_clips_meet_wherever_the_boundary_moves() {
    let mut s = song(&[(2, 4, 0.0)]);
    let [whole] = s.clips[..] else { panic!() };
    let result = s.edit(
        ids::SPLIT,
        json!({ "clips": [whole], "at": [3 * BAR, 4 * BAR] }),
    );
    let mut parts = vec![whole];
    parts.extend(result.created.iter().copied());
    parts.sort_by_key(|id| s.doc().clip(*id).unwrap().start);
    let [first, second, third] = parts[..] else { panic!() };
    // A longer crossfade with a curve, made by hand.
    s.run(
        "arrangement.set_crossfade",
        json!({ "track": s.track, "boundary": 4 * BAR, "before": 0.25, "after": 0.5, "curve": 0.5 }),
    );
    let before = s.crossfades();
    assert_eq!(before.len(), 2);

    // The second and third parts move together, over the first.
    s.one_step(
        ids::MOVE_CLIPS,
        json!({ "clips": [second, third], "by": -BAR / 2 }),
        "Move clips",
    );
    let after = s.crossfades();
    assert_eq!(after.len(), 2);
    let kept = after.iter().find(|c| c.earlier == second).unwrap();
    assert_eq!((kept.before, kept.after, kept.curve), (0.25, 0.5, 0.5));
    assert_eq!(kept.boundary, Ticks(7 * BAR / 2));
    // The first was shortened by the second, so they meet at the new place.
    let end = s.doc().clip(first).unwrap().end();
    assert_eq!(end, Ticks(5 * BAR / 2));

    // The first clip resized over the second: they meet further on, and the
    // crossfade between them goes on.
    s.one_step(
        ids::RESIZE_CLIPS,
        json!({ "clips": [first], "edge": "end", "by": BAR / 4 }),
        "Resize clips",
    );
    assert_eq!(s.crossfades().len(), 2);
    assert_eq!(s.doc().clip(second).unwrap().start, Ticks(11 * BAR / 4));
}

// Spec: audio-clips R22, R26. With Create Fades on Clip Edges off, clips that
// come to meet get no crossfade, and parting them leaves no fade.
#[test]
fn with_the_setting_off_meeting_clips_get_no_crossfade() {
    let mut s = song(&[(2, 4, 0.0)]);
    s.set_edge_fades(false);
    let [whole] = s.clips[..] else { panic!() };
    let result = s.one_step(
        ids::SPLIT,
        json!({ "clips": [whole], "at": [3 * BAR] }),
        "Split clips",
    );
    let [right] = result.created[..] else { panic!() };
    assert!(s.crossfades().is_empty());
    // Made by hand, a crossfade ends when the clips part, leaving no fade.
    s.run(
        "arrangement.set_crossfade",
        json!({ "track": s.track, "boundary": 3 * BAR, "before": 0.1, "after": 0.1 }),
    );
    assert_eq!(s.crossfades().len(), 1);
    s.one_step(
        ids::MOVE_CLIPS,
        json!({ "clips": [right], "by": BAR }),
        "Move clips",
    );
    assert!(s.crossfades().is_empty());
    assert_eq!(s.fades(whole), (0.0, 0.0, 0.0, 0.0));
    assert_eq!(s.fades(right), (0.0, 0.0, 0.0, 0.0));
}

// Spec: audio-clips R28. Create Fade on a time selection: a fade-in from a
// clip's start, a fade-out up to its end, a crossfade over a boundary; the
// menu's name follows the selection.
#[test]
fn create_fade_makes_what_the_time_selection_gives() {
    let mut s = song(&[(0, 4, 0.0), (5, 2, 5.0), (7, 2, 5.0)]);
    s.set_edge_fades(false);
    let [long, left, right] = s.clips[..] else { panic!() };
    let track = s.track;
    let select = |s: &mut Song, start: i64, end: i64| {
        s.shell
            .run(
                view_ids::SELECT_TIME,
                json!({ "start": start, "end": end, "tracks": [track] }),
            )
            .unwrap();
    };
    let label = |s: &Song| s.shell.dynamic_label(ids::CREATE_FADE).unwrap();
    assert_eq!(label(&s), "Create Fade", "nothing selected gives none");
    assert!(!s.shell.can_invoke(ids::CREATE_FADE, &json!({})));

    // A selection holding a clip's start and not its end: a fade-in to its
    // end. Two seconds is a bar at 120 BPM.
    select(&mut s, 0, BAR);
    assert_eq!(label(&s), "Create Fade In/Out");
    s.one_step(ids::CREATE_FADE, json!({}), "Create Fade");
    assert_eq!(s.fades(long), (2.0, 0.0, 0.0, 0.0));
    // Its end and not its start: a fade-out from the selection's start.
    select(&mut s, 3 * BAR, 4 * BAR);
    s.one_step(ids::CREATE_FADE, json!({}), "Create Fade");
    assert_eq!(s.fades(long), (2.0, 0.0, 2.0, 0.0));
    // A selection holding the whole clip, or neither edge, gives none.
    for (start, end) in [(0, 4 * BAR), (BAR, 3 * BAR)] {
        select(&mut s, start, end);
        assert_eq!(label(&s), "Create Fade");
        assert!(s.shell.run(ids::CREATE_FADE, json!({})).is_err());
    }

    // Two clips meeting inside the selection: a crossfade over it.
    select(&mut s, 6 * BAR + BAR / 2, 7 * BAR + BAR / 2);
    assert_eq!(label(&s), "Create Crossfade");
    s.one_step(ids::CREATE_FADE, json!({}), "Create Crossfade");
    assert_eq!(s.fades(left), (0.0, 0.0, 1.0, 1.0));
    assert_eq!(s.fades(right), (1.0, 1.0, 0.0, 0.0));
    assert_eq!(s.crossfades().len(), 1);
}

// Spec: audio-clips R28. With clips selected instead, each boundary where two
// of them meet gets the 4 ms crossfade, whatever the setting.
#[test]
fn create_fade_on_selected_clips_crossfades_where_they_meet() {
    let mut s = song(&[(0, 2, 5.0), (2, 2, 5.0), (6, 2, 5.0)]);
    s.set_edge_fades(false);
    let [a, b, c] = s.clips[..] else { panic!() };
    s.shell
        .run(ids::SELECT_CLIPS, json!({ "clips": [a, b, c] }))
        .unwrap();
    assert_eq!(
        s.shell.dynamic_label(ids::CREATE_FADE).unwrap(),
        "Create Crossfade"
    );
    s.one_step(ids::CREATE_FADE, json!({}), "Create Crossfade");
    assert_eq!(s.fades(a), (0.0, 0.0, HALF, HALF));
    assert_eq!(s.fades(b), (HALF, HALF, 0.0, 0.0));
    assert_eq!(s.fades(c), (0.0, 0.0, 0.0, 0.0));
    // Clips that meet no other selected clip give nothing.
    s.shell
        .run(ids::SELECT_CLIPS, json!({ "clips": [c] }))
        .unwrap();
    assert!(!s.shell.can_invoke(ids::CREATE_FADE, &json!({})));
}

// Spec: audio-clips R29. Delete Fades puts a fade back to 4 ms and a crossfade
// to the 4 ms crossfade while Create Fades on Clip Edges is on, and removes
// them with it off.
#[test]
fn delete_fades_resets_to_the_edge_defaults_or_removes() {
    let mut s = song(&[(0, 2, 5.0), (2, 2, 5.0)]);
    let [a, b] = s.clips[..] else { panic!() };
    s.run(
        "arrangement.set_crossfade",
        json!({ "track": s.track, "boundary": 2 * BAR, "before": 0.25, "after": 0.5, "curve": -0.5 }),
    );
    s.run(
        "clip.set_fades",
        json!({ "clip": a, "fade_in": 1.0, "fade_out": 0.25, "fade_in_curve": 0.5 }),
    );
    // One edge's fade, as Delete does with a handle selected.
    s.one_step(
        ids::DELETE_FADES,
        json!({ "clips": [a], "edge": "start" }),
        "Delete Fade",
    );
    assert_eq!(s.fades(a).0, EDGE_FADE_SECONDS);
    assert_eq!(s.audio(a).fade_in_curve, 0.0);
    // Both clips selected: the crossfade goes back to 4 ms, 2 ms each side.
    s.shell
        .run(ids::SELECT_CLIPS, json!({ "clips": [a, b] }))
        .unwrap();
    s.one_step(ids::DELETE_FADES, json!({}), "Delete Fades");
    assert_eq!(s.fades(a), (EDGE_FADE_SECONDS, 0.0, HALF, HALF));
    assert_eq!(s.fades(b), (HALF, HALF, EDGE_FADE_SECONDS, 0.0));
    assert_eq!(s.audio(a).fade_out_curve, 0.0);

    // With the setting off the fades go, and the crossfade with them.
    s.set_edge_fades(false);
    s.one_step(ids::DELETE_FADES, json!({}), "Delete Fades");
    assert_eq!(s.fades(a), (0.0, 0.0, 0.0, 0.0));
    assert_eq!(s.fades(b), (0.0, 0.0, 0.0, 0.0));
    assert!(s.crossfades().is_empty());
}

// Spec: audio-clips R27. A drag of a fade handle runs `clip.set_fades` or
// `arrangement.set_crossfade` as a gesture: one undo step named Change Fade,
// and Esc puts the fade back.
#[test]
fn dragging_a_fade_handle_is_one_undo_step() {
    let mut s = song(&[(0, 2, 5.0), (2, 2, 5.0)]);
    let [a, b] = s.clips[..] else { panic!() };
    let steps = s.steps();
    let frame = |length: f64| {
        Invocation::new(
            "clip.set_fades",
            json!({ "clip": a, "fade_in": length, "fade_out": 0.0 }),
        )
    };
    for step in 1..=8 {
        s.shell
            .gesture(GesturePhase::Update, Some(&frame(f64::from(step) * 0.1)))
            .unwrap();
        assert_eq!(s.fades(a).0, f64::from(step) * 0.1);
    }
    s.shell
        .gesture(GesturePhase::Commit, Some(&frame(1.0)))
        .unwrap();
    assert_eq!(s.fades(a).0, 1.0);
    assert_eq!(s.steps(), steps + 1);
    assert_eq!(s.shell.editor().undo_history().next(), Some("Change Fade"));

    // A crossfade's outer handle dragged over the clip beside it.
    let before = s.doc().clone();
    let crossfade = |after: f64| {
        Invocation::new(
            "arrangement.set_crossfade",
            json!({ "track": s.track, "boundary": 2 * BAR, "before": 0.1, "after": after }),
        )
    };
    s.shell
        .gesture(GesturePhase::Update, Some(&crossfade(0.3)))
        .unwrap();
    assert_eq!(s.crossfades().len(), 1);
    s.shell
        .gesture(GesturePhase::Update, Some(&crossfade(0.6)))
        .unwrap();
    assert_eq!(s.fades(b).0, 0.6);
    s.shell.gesture(GesturePhase::Cancel, None).unwrap();
    assert_eq!(*s.doc(), before, "Esc puts the fades back");
    assert_eq!(s.steps(), steps + 1);
}

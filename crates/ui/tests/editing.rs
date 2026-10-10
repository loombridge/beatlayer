//! The Arrangement's editing gestures, headless (WP-1.8b): clicking,
//! dragging, and keys reach the same editing commands agents run, and each
//! drag is one undo step however many frames it spans. Spec:
//! arrangement-editing.

mod common;

use std::sync::Arc;

use bl_core::time::Ticks;
use bl_media::Peaks;
use bl_model::{AudioData, Clip, ClipData, ClipId, Command, ContentHash, Document, TrackId};
use bl_shell::arrangement_commands::{self as view_ids, ArrangementState};
use bl_shell::editing_commands as ids;
use bl_shell::files::factory_default_set;
use bl_ui::arrangement::benchmark;
use bl_ui::arrangement::gpu::{Slot, lock};
use bl_ui::arrangement::peaks::MemoryPeaks;
use common::{Harness, Window, command};
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2};
use serde_json::json;

const BAR: i64 = Ticks::WHOLE.get();

/// The lanes' left edge and the new set's zoom: 30 points to a bar.
const LEFT: f32 = 320.0;
const POINTS_PER_BAR: f32 = 30.0;

/// Where bar `bar` (counting from 0) is across the lanes.
fn x(bar: f32) -> f32 {
    LEFT + bar * POINTS_PER_BAR
}

/// Heights in the factory set: each track 88 points from y 119, so the
/// third track (3 Audio) is 295 to 383 and the fourth 383 to 471. A clip's
/// title row is its top 13 points.
const AUDIO_3: (f32, f32) = (300.0, 340.0);
const AUDIO_4: (f32, f32) = (388.0, 430.0);

/// The modifier that copies while dragging: Option on macOS, Ctrl
/// elsewhere.
fn copy_key() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::ALT
    } else {
        Modifiers::CTRL | Modifiers::COMMAND
    }
}

/// The modifier that bypasses snapping while dragging: Cmd on macOS, Alt
/// elsewhere.
fn bypass_key() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD | Modifiers::COMMAND
    } else {
        Modifiers::ALT
    }
}

struct Set {
    h: Harness,
    a: ClipId,
    b: ClipId,
    tracks: Vec<TrackId>,
}

/// The factory set with clip A on 3 Audio over bars 2 to 6 and clip B on 4
/// Audio over bars 8 to 10 (counting from 0), drawn at the reference size.
fn set() -> Set {
    set_with(&[(2, 2.0, 4.0), (3, 8.0, 2.0)], None)
}

fn set_with(clips: &[(usize, f64, f64)], peaks: Option<MemoryPeaks>) -> Set {
    let mut editor = factory_default_set();
    let tracks: Vec<TrackId> = editor.document().top_level().collect();
    let mut ids = Vec::new();
    for (track, start, bars) in clips {
        let id: ClipId = editor.new_id();
        let ticks = |bars: f64| Ticks((bars * BAR as f64) as i64);
        editor
            .apply(Command::CreateClip {
                track: tracks[*track],
                clip: Clip::new(
                    id,
                    ticks(*start),
                    ticks(*bars),
                    ClipData::Audio(AudioData {
                        media: benchmark::media_hash(0),
                    }),
                ),
            })
            .unwrap();
        ids.push(id);
    }
    editor.clear_history().unwrap();
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell = benchmark::shell_with(editor);
    if let Some(peaks) = peaks {
        h.main
            .arrangement_mut()
            .set_peak_source(Box::new(peaks), None);
    }
    h.settle();
    Set {
        h,
        a: ids[0],
        b: ids.get(1).copied().unwrap_or(ids[0]),
        tracks,
    }
}

impl Set {
    fn doc(&self) -> &Document {
        self.h.shell.editor().document()
    }

    fn clip(&self, id: ClipId) -> Clip {
        self.doc().clip(id).unwrap().clone()
    }

    fn bars(&self, id: ClipId) -> (f64, f64) {
        let clip = self.clip(id);
        (
            clip.start.get() as f64 / BAR as f64,
            clip.end().get() as f64 / BAR as f64,
        )
    }

    fn state(&mut self) -> ArrangementState {
        serde_json::from_value(self.h.shell.run(view_ids::STATE, json!({})).unwrap()).unwrap()
    }

    fn steps(&self) -> usize {
        self.h.shell.editor().undo_history().len()
    }

    fn undo_label(&self) -> Option<String> {
        self.h
            .shell
            .editor()
            .undo_history()
            .next()
            .map(str::to_owned)
    }

    /// A click with modifiers held.
    fn click(&mut self, at: Pos2, modifiers: Modifiers) {
        let held = Event::ModifiersChanged(modifiers);
        self.h.frame(vec![held.clone(), Event::PointerMoved(at)]);
        for pressed in [true, false] {
            self.h.frame(vec![
                held.clone(),
                Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers,
                },
            ]);
        }
        self.h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
        self.h.settle();
    }

    /// A press, a drag through `steps` frames, and, unless `release` is
    /// false, a release, with modifiers held throughout.
    fn drag(&mut self, from: Pos2, to: Pos2, modifiers: Modifiers, release: bool) {
        let held = Event::ModifiersChanged(modifiers);
        self.h.frame(vec![held.clone(), Event::PointerMoved(from)]);
        self.h.frame(vec![
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
            self.h.frame(vec![
                held.clone(),
                Event::PointerMoved(from + (to - from) * t),
            ]);
        }
        if release {
            self.release(to, modifiers);
        }
    }

    fn release(&mut self, at: Pos2, modifiers: Modifiers) {
        self.h.frame(vec![
            Event::ModifiersChanged(modifiers),
            Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed: false,
                modifiers,
            },
        ]);
        self.h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
        self.h.settle();
    }

    fn key(&mut self, key: Key, modifiers: Modifiers) {
        self.h.key(key, modifiers);
        self.h.settle();
    }

    /// Checks the newest step undoes to exactly `before` and redoes.
    fn undoes_to(&mut self, before: &Document) {
        let after = self.doc().clone();
        self.key(Key::Z, command());
        assert_eq!(self.doc(), before, "Cmd-Z restores the set exactly");
        self.key(Key::Z, command() | Modifiers::SHIFT);
        assert_eq!(*self.doc(), after, "Cmd-Shift-Z redoes it");
    }
}

#[test]
fn clicks_select_clips_and_time() {
    // Spec: arrangement-editing R1, R2 (S-EDIT-010, S-EDIT-011, S-ZOOM-008
    // to 010, I-CLICK-011).
    let mut s = set();
    let (a, b) = (s.a, s.b);
    s.click(pos2(x(3.0), AUDIO_3.0), Modifiers::NONE);
    let state = s.state();
    assert_eq!(state.selected_clips, [a], "clicking a title bar selects");
    assert_eq!(state.selected_tracks, [s.tracks[2]]);
    s.click(pos2(x(9.0), AUDIO_4.0), command());
    assert_eq!(s.state().selected_clips, [a, b], "Cmd-click adds");
    s.click(pos2(x(3.0), AUDIO_3.0), command());
    assert_eq!(s.state().selected_clips, [b], "and removes");
    s.click(pos2(x(3.0), AUDIO_3.0), Modifiers::SHIFT);
    assert_eq!(s.state().selected_clips, [b, a], "Shift-click extends");

    // The body places the insert marker, snapped, and deselects clips.
    s.click(pos2(x(3.6), AUDIO_3.1), Modifiers::NONE);
    let state = s.state();
    assert!(state.selected_clips.is_empty());
    assert_eq!(state.insert_marker, Ticks(4 * BAR));
    // Shift-click in the background extends time from the marker.
    s.click(pos2(x(12.0), AUDIO_4.1), Modifiers::SHIFT);
    let state = s.state();
    let time = state.time_selection.unwrap();
    assert_eq!((time.start, time.end), (Ticks(4 * BAR), Ticks(12 * BAR)));
    assert_eq!(state.selected_tracks, [s.tracks[2], s.tracks[3]]);
    assert_eq!(s.steps(), 0, "selecting is never an undo step");
}

#[test]
fn dragging_a_title_bar_moves_with_snapping_as_one_step() {
    // Spec: arrangement-editing R4, R13 (I-DRAG-011, I-DRAG-021).
    let mut s = set();
    let a = s.a;
    let before = s.doc().clone();
    // About two bars later: it snaps to the bar.
    s.drag(
        pos2(x(3.0), AUDIO_3.0),
        pos2(x(5.07), AUDIO_3.0),
        Modifiers::NONE,
        true,
    );
    assert_eq!(s.bars(a), (4.0, 8.0));
    assert_eq!(s.steps(), 1, "eight frames of drag are one undo step");
    assert_eq!(s.undo_label().as_deref(), Some("Move clips"));
    assert_eq!(s.state().selected_clips, [a]);
    s.undoes_to(&before);

    // Down onto 4 Audio.
    s.drag(
        pos2(x(5.0), AUDIO_3.0),
        pos2(x(5.0), AUDIO_4.0),
        Modifiers::NONE,
        true,
    );
    assert_eq!(s.doc().clip_track(a), Some(s.tracks[3]));
    // Cmd (Alt) held: no snapping, a bar and a half later.
    s.drag(
        pos2(x(5.0), AUDIO_4.0),
        pos2(x(6.5), AUDIO_4.0),
        bypass_key(),
        true,
    );
    assert_eq!(s.bars(a), (5.5, 9.5));
    // The clip it now covers (B, bars 8 to 10) is shortened from its start.
    assert_eq!(s.bars(s.b), (9.5, 10.0));
    assert_eq!(s.steps(), 3);
    // Never dragged before 1.1.1.
    s.drag(
        pos2(x(6.0), AUDIO_4.0),
        pos2(x(-3.0), AUDIO_4.0),
        Modifiers::NONE,
        true,
    );
    assert_eq!(s.bars(a).0, 0.0);
}

#[test]
fn the_copy_modifier_copies_and_leaves_the_original() {
    // Spec: arrangement-editing R5.
    let mut s = set();
    let a = s.a;
    let before = s.doc().clone();
    s.drag(
        pos2(x(3.0), AUDIO_3.0),
        pos2(x(13.0), AUDIO_3.0),
        copy_key(),
        true,
    );
    assert_eq!(s.bars(a), (2.0, 6.0), "the original stays");
    let state = s.state();
    assert_eq!(state.selected_clips.len(), 1);
    let copy = state.selected_clips[0];
    assert_ne!(copy, a);
    assert_eq!(s.bars(copy), (12.0, 16.0));
    assert_eq!(s.undo_label().as_deref(), Some("Copy clips"));
    assert_eq!(s.steps(), 1);
    s.undoes_to(&before);
}

#[test]
fn dragging_an_edge_resizes_and_esc_cancels_a_drag() {
    // Spec: arrangement-editing R7, R13 (I-DRAG-012).
    let mut s = set();
    let a = s.a;
    // The right edge, a bar later.
    s.drag(
        pos2(x(6.0) - 2.0, AUDIO_3.1),
        pos2(x(7.03) - 2.0, AUDIO_3.1),
        Modifiers::NONE,
        true,
    );
    assert_eq!(s.bars(a), (2.0, 7.0));
    assert_eq!(s.undo_label().as_deref(), Some("Resize clips"));
    // The left edge, a bar later: the clip shows less of its contents.
    s.drag(
        pos2(x(2.0) + 2.0, AUDIO_3.1),
        pos2(x(3.0) + 2.0, AUDIO_3.1),
        Modifiers::NONE,
        true,
    );
    assert_eq!(s.bars(a), (3.0, 7.0));
    assert_eq!(s.clip(a).offset, Ticks(BAR));
    assert_eq!(s.steps(), 2);

    // Esc during a drag: the clip goes back and nothing is recorded.
    let before = s.doc().clone();
    s.drag(
        pos2(x(4.0), AUDIO_3.0),
        pos2(x(9.0), AUDIO_3.0),
        Modifiers::NONE,
        false,
    );
    assert_eq!(s.bars(a).0, 8.0, "the clip follows the drag as it goes");
    assert!(s.h.shell.gesture_open());
    s.h.key(Key::Escape, Modifiers::NONE);
    s.release(pos2(x(9.0), AUDIO_3.0), Modifiers::NONE);
    assert_eq!(*s.doc(), before);
    assert!(!s.h.shell.gesture_open());
    assert_eq!(s.steps(), 2);
}

#[test]
fn edit_keys_act_on_the_selection_one_step_each() {
    // Spec: arrangement-editing R3, R8 to R11, R14 (S-EDIT-001 to 005,
    // S-EDIT-009, S-ARR-001).
    let mut s = set();
    let (a, b) = (s.a, s.b);
    let start = s.doc().clone();
    // Cmd-D: a copy after the clip, selected.
    s.click(pos2(x(3.0), AUDIO_3.0), Modifiers::NONE);
    s.key(Key::D, command());
    let copy = s.state().selected_clips[0];
    assert_eq!(s.bars(copy), (6.0, 10.0));
    // Delete: the copy goes.
    s.key(Key::Delete, Modifiers::NONE);
    assert!(s.doc().clip(copy).is_none());
    // Cmd-E at the insert marker, placed by clicking the clip's body.
    s.click(pos2(x(4.0), AUDIO_3.1), Modifiers::NONE);
    s.key(Key::E, command());
    assert_eq!(s.bars(a), (2.0, 4.0));
    assert_eq!(s.doc().track(s.tracks[2]).unwrap().clips().len(), 2);
    // Cmd-C, then Cmd-V at a marker on 3 Audio.
    s.click(pos2(x(9.0), AUDIO_4.0), Modifiers::NONE);
    s.key(Key::C, command());
    s.click(pos2(x(12.0), AUDIO_3.1), Modifiers::NONE);
    s.key(Key::V, command());
    let pasted = s.state().selected_clips[0];
    assert_eq!(s.bars(pasted), (12.0, 14.0));
    assert_eq!(s.doc().clip_track(pasted), Some(s.tracks[2]));
    // Cmd-X cuts the pasted clip.
    s.key(Key::X, command());
    assert!(s.doc().clip(pasted).is_none());
    // Cmd-A selects every clip.
    s.key(Key::A, command());
    assert_eq!(s.state().selected_clips.len(), 3);
    assert!(s.state().selected_clips.contains(&b));
    // Duplicate, Delete, Split, Paste, Cut: five steps; Copy and Select
    // All none. Undoing all five restores the set exactly.
    assert_eq!(s.steps(), 5);
    for _ in 0..5 {
        s.key(Key::Z, command());
    }
    assert_eq!(*s.doc(), start);
}

#[test]
fn arrows_nudge_jump_to_clip_edges_and_extend() {
    // Spec: arrangement-editing R12 (S-ARR-028, S-A11Y-001, S-A11Y-002).
    let mut s = set();
    let a = s.a;
    s.click(pos2(x(3.0), AUDIO_3.0), Modifiers::NONE);
    s.key(Key::ArrowRight, Modifiers::NONE);
    assert_eq!(s.bars(a), (3.0, 7.0), "a grid step later");
    s.key(Key::ArrowLeft, Modifiers::NONE);
    assert_eq!(s.bars(a), (2.0, 6.0));
    assert_eq!(s.steps(), 2);
    // With the marker at 1.1.1 on 3 Audio, Option-Right goes to the clip's
    // start, then its end.
    s.click(pos2(x(0.0) + 1.0, AUDIO_3.1), Modifiers::NONE);
    s.key(Key::ArrowRight, Modifiers::ALT);
    assert_eq!(s.state().insert_marker, Ticks(2 * BAR));
    s.key(Key::ArrowRight, Modifiers::ALT);
    assert_eq!(s.state().insert_marker, Ticks(6 * BAR));
    // Shift-Right selects time from the marker, a grid step at a time;
    // Shift-Down takes in the next track.
    s.key(Key::ArrowRight, Modifiers::SHIFT);
    s.key(Key::ArrowRight, Modifiers::SHIFT);
    let state = s.state();
    let time = state.time_selection.unwrap();
    assert_eq!((time.start, time.end), (Ticks(6 * BAR), Ticks(8 * BAR)));
    s.key(Key::ArrowDown, Modifiers::SHIFT);
    assert_eq!(s.state().selected_tracks, [s.tracks[2], s.tracks[3]]);
    // With nothing selected, the arrows move the insert marker.
    s.click(pos2(x(1.0), AUDIO_4.1), Modifiers::NONE);
    s.key(Key::ArrowRight, Modifiers::NONE);
    assert_eq!(s.state().insert_marker, Ticks(2 * BAR));
    assert_eq!(s.steps(), 2);
}

#[test]
fn enter_resizes_from_the_keyboard() {
    // Spec: arrangement-editing R12, R13 (S-ARR-004).
    let mut s = set();
    let a = s.a;
    s.click(pos2(x(3.0), AUDIO_3.0), Modifiers::NONE);
    s.key(Key::Enter, Modifiers::NONE);
    s.key(Key::ArrowRight, Modifiers::NONE);
    s.key(Key::ArrowRight, Modifiers::NONE);
    assert_eq!(s.bars(a), (2.0, 8.0), "the end moves as the keys go");
    s.key(Key::Enter, Modifiers::NONE);
    assert_eq!(s.bars(a), (2.0, 8.0));
    assert_eq!(s.steps(), 1, "one undo step");
    // Esc puts it back.
    s.key(Key::Enter, Modifiers::NONE);
    s.key(Key::ArrowLeft, Modifiers::NONE);
    assert_eq!(s.bars(a), (2.0, 7.0));
    s.key(Key::Escape, Modifiers::NONE);
    assert_eq!(s.bars(a), (2.0, 8.0));
    assert_eq!(s.steps(), 1);
    assert!(!s.h.shell.gesture_open());
}

#[test]
fn selected_clips_are_drawn_and_announced() {
    // Spec: arrangement-editing R15.
    let mut s = set();
    let rects = |s: &Set| {
        let shared = s.h.main.arrangement().shared();
        let frame = lock(&shared);
        frame.slots[Slot::Lanes as usize].rects.len()
    };
    let plain = rects(&s);
    s.click(pos2(x(3.0), AUDIO_3.0), Modifiers::NONE);
    assert!(rects(&s) > plain, "a selected clip draws its highlight");
    let output = s.h.settle();
    let nodes = Harness::nodes(&output);
    let clips: Vec<_> = nodes.iter().filter(|n| n.role == Role::ListItem).collect();
    assert_eq!(clips.len(), 2);
    let selected: Vec<_> = clips.iter().filter(|n| n.selected == Some(true)).collect();
    assert_eq!(selected.len(), 1);
    assert!(
        selected[0].value.starts_with("3.1.1 to 7.1.1"),
        "{}",
        selected[0].value
    );
    // The pointer says what a press would do.
    let cursor = |s: &mut Set, at: Pos2| {
        s.h.frame(vec![Event::PointerMoved(at)])
            .platform_output
            .cursor_icon
    };
    assert_eq!(
        cursor(&mut s, pos2(x(6.0) - 1.0, AUDIO_3.1)),
        egui::CursorIcon::ResizeHorizontal
    );
    assert_eq!(
        cursor(&mut s, pos2(x(4.0), AUDIO_3.0)),
        egui::CursorIcon::Grab
    );
}

#[test]
fn a_split_clip_s_waveform_carries_on_from_where_it_was() {
    // Spec: arrangement-editing R16: the second part draws the media from
    // where the first leaves off.
    let mut peaks = MemoryPeaks::new();
    peaks.insert(
        benchmark::media_hash(0),
        Arc::new(
            Peaks::from_samples(&benchmark::media_samples(
                &mut benchmark::Random::new(1),
                16.0,
            ))
            .unwrap(),
        ),
    );
    // Sixteen seconds at 120 BPM is eight bars.
    let mut s = set_with(&[(2, 2.0, 8.0)], Some(peaks));
    let waves = |s: &Set| {
        let shared = s.h.main.arrangement().shared();
        let frame = lock(&shared);
        let mut waves = frame.slots[Slot::Lanes as usize].waves.clone();
        waves.sort_by(|p, q| p.draw[0].total_cmp(&q.draw[0]));
        // The first channel's band of each clip.
        waves.dedup_by(|p, q| (p.draw[0] - q.draw[0]).abs() < 0.5);
        waves
    };
    let whole = waves(&s);
    assert_eq!(whole.len(), 1);
    let peak_at = |w: &bl_ui::arrangement::gpu::WaveInst, at: f32| {
        f64::from(w.run[2])
            + f64::from(w.band[3])
            + f64::from(at - w.draw[0]) * f64::from(w.band[2])
    };
    s.h.shell
        .run(ids::SPLIT, json!({ "clips": [s.a], "at": ["7"] }))
        .unwrap();
    s.h.settle();
    let parts = waves(&s);
    assert_eq!(parts.len(), 2);
    let split = x(6.0);
    assert!(
        (parts[1].draw[0] - split).abs() < 0.5,
        "{:?}",
        parts[1].draw
    );
    let expected = peak_at(&whole[0], split);
    let actual = peak_at(&parts[1], split);
    assert!(
        (expected - actual).abs() < 0.01,
        "the second part starts at peak {actual}, not {expected}"
    );
    assert_eq!(parts[0].run[2], whole[0].run[2]);
}

#[test]
fn edit_commands_are_bound_in_the_arrangement() {
    // Spec: arrangement-editing R14: the keymap sends the edit keys to the
    // editing commands in the Arrangement.
    let s = set();
    let contexts = [bl_commands::Context::Arrangement];
    for (shortcut, id) in [
        ("S-EDIT-001", ids::CUT),
        ("S-EDIT-002", ids::COPY),
        ("S-EDIT-003", ids::PASTE),
        ("S-EDIT-004", ids::DUPLICATE),
        ("S-EDIT-005", ids::DELETE),
        ("S-EDIT-009", ids::SELECT_ALL),
        ("S-ARR-001", ids::SPLIT),
    ] {
        assert_eq!(
            s.h.keymap
                .command_for(shortcut, &contexts)
                .map(|c| c.id.as_str()),
            Some(id),
            "{shortcut}"
        );
    }
    let _ = ContentHash([0; 32]);
}

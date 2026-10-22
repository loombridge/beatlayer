//! The fade handles on audio clips, headless (WP-6.51; audio clips R27 to
//! R29): they show on hover and while F is held, drag to set a fade's length,
//! curve, or crossfade, show their limit as a dotted line when selected, and
//! go with Delete. Create Fade and Delete Fades run from their keys. Every
//! drag is one undo step, `Change Fade`.

mod common;

use bl_core::time::Ticks;
use bl_model::{AudioData, Clip, ClipData, ClipId, Command, Document, TrackId};
use bl_shell::arrangement_commands::{self as view_ids, ArrangementState, FadeHandle};
use bl_shell::editing_commands::Edge;
use bl_shell::files::factory_default_set;
use bl_ui::arrangement::benchmark;
use bl_ui::keymap::Platform;
use common::{Harness, Window};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2};
use serde_json::json;

const BAR: i64 = Ticks::WHOLE.get();
const LEFT: f32 = 320.0;
const POINTS_PER_BAR: f32 = 30.0;

fn x(bar: f32) -> f32 {
    LEFT + bar * POINTS_PER_BAR
}

/// A point over the third track's clips, in the body under the title row.
const BODY: f32 = 340.0;

fn bypass_key() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD | Modifiers::COMMAND
    } else {
        Modifiers::ALT
    }
}

struct Set {
    h: Harness,
    clips: Vec<ClipId>,
    tracks: Vec<TrackId>,
}

/// The factory set with audio clips on the third track: `(start bar, bars)`,
/// the second starting a second into its file so that a crossfade can reach
/// back before it.
fn set(clips: &[(f64, f64)]) -> Set {
    let mut editor = factory_default_set();
    let tracks: Vec<TrackId> = editor.document().top_level().collect();
    let mut ids = Vec::new();
    for (index, (start, bars)) in clips.iter().enumerate() {
        let id: ClipId = editor.new_id();
        let ticks = |bars: f64| Ticks((bars * BAR as f64) as i64);
        let audio = AudioData {
            start: if index == 0 { 0.0 } else { 1.0 },
            ..AudioData::new(benchmark::media_hash(0))
        };
        editor
            .apply(Command::CreateClip {
                track: tracks[2],
                clip: Clip::new(id, ticks(*start), ticks(*bars), ClipData::Audio(audio)),
            })
            .unwrap();
        ids.push(id);
    }
    editor.clear_history().unwrap();
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell = benchmark::shell_with(editor);
    h.settle();
    Set {
        h,
        clips: ids,
        tracks,
    }
}

impl Set {
    fn doc(&self) -> &Document {
        self.h.shell.editor().document()
    }

    fn audio(&self, clip: ClipId) -> AudioData {
        match &self.doc().clip(clip).unwrap().data {
            ClipData::Audio(audio) => audio.clone(),
            ClipData::Midi(_) => panic!("an audio clip"),
        }
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

    fn state(&mut self) -> ArrangementState {
        serde_json::from_value(self.h.shell.run(view_ids::STATE, json!({})).unwrap()).unwrap()
    }

    fn hover(&mut self, at: Pos2) {
        self.h.frame(vec![Event::PointerMoved(at)]);
        self.h.settle();
    }

    /// The handles the last frame drew of `clip`.
    fn spots(&self, clip: ClipId) -> Vec<(Edge, FadeHandle, Pos2)> {
        self.h
            .main
            .arrangement()
            .drawn_fades()
            .spots
            .iter()
            .filter(|s| s.clip == clip)
            .map(|s| (s.edge, s.handle, s.pos))
            .collect()
    }

    fn spot(&self, clip: ClipId, edge: Edge, handle: FadeHandle) -> Pos2 {
        self.spots(clip)
            .into_iter()
            .find(|s| s.0 == edge && s.1 == handle)
            .unwrap_or_else(|| panic!("no {handle:?} handle at {edge:?} of {clip:?}"))
            .2
    }

    fn press_f(&mut self, down: bool) {
        self.h.frame(vec![Event::Key {
            key: Key::F,
            physical_key: None,
            pressed: down,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
        self.h.settle();
    }

    fn click(&mut self, at: Pos2) {
        self.hover(at);
        for pressed in [true, false] {
            self.h.frame(vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            }]);
        }
        self.h.settle();
    }

    fn drag(&mut self, from: Pos2, to: Pos2, modifiers: Modifiers, release: bool) {
        let held = Event::ModifiersChanged(modifiers);
        self.hover(from);
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
            self.h.frame(vec![
                held,
                Event::PointerButton {
                    pos: to,
                    button: PointerButton::Primary,
                    pressed: false,
                    modifiers,
                },
            ]);
            self.h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
            self.h.settle();
        }
    }

    fn key(&mut self, key: Key, modifiers: Modifiers) {
        self.h.key(key, modifiers);
        self.h.settle();
    }

    fn shortcut(&mut self, shortcut: &str) {
        let platform = Platform::current();
        let chord = self
            .h
            .keymap
            .chord_for_shortcut(shortcut, platform)
            .unwrap_or_else(|| panic!("{shortcut} has a key chord"))
            .clone();
        self.key(chord.key, bl_ui::keymap::mods_to_egui(chord.mods, platform));
    }
}

// Spec: audio-clips R27. Handles show while the pointer is over an audio clip
// on a lane at least 40 points tall, and over every clip while F is held.
#[test]
fn handles_show_on_hover_and_while_f_is_held() {
    let mut s = set(&[(2.0, 2.0), (8.0, 2.0)]);
    let [a, b] = s.clips[..] else { panic!() };
    // Nothing shows with the pointer elsewhere.
    s.hover(pos2(100.0, 50.0));
    assert!(s.spots(a).is_empty() && s.spots(b).is_empty());
    // Over the first clip: its two fades show a peak and an outer handle
    // each (the curve handle comes with a fade that has a length), and the
    // second clip's show nothing.
    s.hover(pos2(x(3.0), BODY));
    assert_eq!(s.spots(a).len(), 4);
    assert!(s.spots(b).is_empty());
    // At the clip's start: the outer handle at its foot, the peak at its top.
    let start = s.spot(a, Edge::Start, FadeHandle::Outer);
    let peak = s.spot(a, Edge::Start, FadeHandle::Peak);
    assert_eq!(start.x, x(2.0));
    assert_eq!(peak.x, x(2.0), "no fade yet");
    assert!(peak.y < start.y);
    assert!(
        !s.spots(a).iter().any(|s| s.1 == FadeHandle::Curve),
        "a fade with no length has no curve handle"
    );
    let end = s.spot(a, Edge::End, FadeHandle::Outer);
    assert_eq!(end.x, x(4.0));
    // F held shows every clip's, wherever the pointer is.
    s.hover(pos2(100.0, 50.0));
    s.press_f(true);
    assert_eq!((s.spots(a).len(), s.spots(b).len()), (4, 4));
    s.press_f(false);
    assert!(s.spots(a).is_empty());

    // A lane under 40 points shows none on hover, but still with F held.
    let track = s.tracks[2];
    s.h.shell
        .run(
            view_ids::SET_TRACK_HEIGHT,
            json!({ "tracks": [track], "height": 30 }),
        )
        .unwrap();
    s.h.settle();
    s.hover(pos2(x(3.0), 305.0));
    assert!(s.spots(a).is_empty());
    s.press_f(true);
    assert_eq!(s.spots(a).len(), 4);
    s.press_f(false);
}

// Spec: audio-clips R27, R29. Dragging a peak handle sets the fade's length,
// snapped to the grid unless Cmd (Alt) is held; the dotted line shows how far
// it can go; Esc puts it back; each drag is one undo step, Change Fade.
#[test]
fn dragging_a_peak_handle_sets_the_fades_length_in_one_step() {
    let mut s = set(&[(2.0, 4.0)]);
    let [a] = s.clips[..] else { panic!() };
    s.hover(pos2(x(4.0), BODY));
    let peak = s.spot(a, Edge::Start, FadeHandle::Peak);
    let steps = s.steps();
    // Half a bar, a second, unsnapped.
    s.drag(peak, pos2(peak.x + 15.0, peak.y), bypass_key(), true);
    assert_eq!(s.audio(a).fade_in, 1.0);
    assert_eq!(s.steps(), steps + 1, "the drag is one undo step");
    assert_eq!(s.undo_label().as_deref(), Some("Change Fade"));
    // The handle followed, and the drag selected it.
    s.hover(pos2(x(4.0), BODY));
    assert_eq!(s.spot(a, Edge::Start, FadeHandle::Peak).x, peak.x + 15.0);
    let state = s.state();
    let selected = state.selected_fade.expect("the handle is selected");
    assert_eq!(
        (selected.clip, selected.edge, selected.handle),
        (a, Edge::Start, FadeHandle::Peak)
    );
    // Its dotted line is at the furthest the fade can reach: the clip's far
    // end, four bars on, as there's no fade-out.
    let limit =
        s.h.main
            .arrangement()
            .drawn_fades()
            .limit
            .expect("a dotted line");
    assert_eq!(limit[0].x, x(6.0));
    assert_eq!(limit[0].x, limit[1].x);

    // Snapped: the length lands on the grid.
    let before = s.doc().clone();
    let peak = s.spot(a, Edge::Start, FadeHandle::Peak);
    s.drag(peak, pos2(x(2.0) + 22.0, peak.y), Modifiers::NONE, true);
    let grid = s.h.shell.arrangement();
    let snapped = grid.snap_drag(s.doc(), Ticks(2 * BAR + 22 * BAR / 30), false);
    let tempo = &s.doc().settings().tempo;
    let expected = tempo.ticks_to_seconds(snapped) - tempo.ticks_to_seconds(Ticks(2 * BAR));
    assert_eq!(s.audio(a).fade_in, expected);
    assert_ne!(*s.doc(), before);

    // Esc during a drag puts the fade back.
    let before = s.doc().clone();
    let peak = s.spot(a, Edge::Start, FadeHandle::Peak);
    let steps = s.steps();
    s.drag(peak, pos2(peak.x + 30.0, peak.y), bypass_key(), false);
    assert_ne!(s.audio(a).fade_in, s.audio_before(&before, a).fade_in);
    s.key(Key::Escape, Modifiers::NONE);
    assert_eq!(*s.doc(), before);
    assert_eq!(s.steps(), steps);
}

impl Set {
    fn audio_before(&self, doc: &Document, clip: ClipId) -> AudioData {
        match &doc.clip(clip).unwrap().data {
            ClipData::Audio(audio) => audio.clone(),
            ClipData::Midi(_) => panic!("an audio clip"),
        }
    }
}

// Spec: audio-clips R23, R27. Dragging a curve handle up bows the fade
// towards full level, down towards silence.
#[test]
fn dragging_a_curve_handle_shapes_the_fade() {
    let mut s = set(&[(2.0, 4.0)]);
    let [a] = s.clips[..] else { panic!() };
    s.edit_fade(a, 4.0, 4.0);
    s.hover(pos2(x(4.0), BODY));
    let curve = s.spot(a, Edge::Start, FadeHandle::Curve);
    s.drag(curve, pos2(curve.x, curve.y - 12.0), Modifiers::NONE, true);
    let up = s.audio(a).fade_in_curve;
    assert!(up > 0.0 && up <= 1.0, "{up}");
    assert_eq!(s.undo_label().as_deref(), Some("Change Fade"));
    s.hover(pos2(x(4.0), BODY));
    let curve = s.spot(a, Edge::Start, FadeHandle::Curve);
    s.drag(curve, pos2(curve.x, curve.y + 24.0), Modifiers::NONE, true);
    assert!(s.audio(a).fade_in_curve < 0.0);
    // The fade-out's own.
    s.hover(pos2(x(4.0), BODY));
    let curve = s.spot(a, Edge::End, FadeHandle::Curve);
    s.drag(curve, pos2(curve.x, curve.y - 12.0), Modifiers::NONE, true);
    assert!(s.audio(a).fade_out_curve > 0.0);
}

impl Set {
    /// Gives a clip fades of `fade_in` and `fade_out` seconds, as an agent
    /// does.
    fn edit_fade(&mut self, clip: ClipId, fade_in: f64, fade_out: f64) {
        self.h
            .shell
            .run(
                "clip.set_fades",
                json!({ "clip": clip, "fade_in": fade_in, "fade_out": fade_out }),
            )
            .unwrap();
        self.h.settle();
    }
}

// Spec: audio-clips R24, R27. Dragging the outer handle of an edge another
// clip meets, out over that clip, makes a crossfade with the peak where it
// was; at an edge no clip meets, the outer handle stays at the edge.
#[test]
fn dragging_an_outer_handle_over_the_clip_beside_makes_a_crossfade() {
    let mut s = set(&[(2.0, 2.0), (4.0, 2.0)]);
    let [a, b] = s.clips[..] else { panic!() };
    s.hover(pos2(x(3.0), BODY));
    // Nothing is beside the first clip's start: the outer handle can't move.
    let outer = s.spot(a, Edge::Start, FadeHandle::Outer);
    let before = s.doc().clone();
    s.drag(outer, pos2(outer.x - 15.0, outer.y), bypass_key(), true);
    assert_eq!(*s.doc(), before);
    assert_eq!(s.steps(), 0);

    // The first clip's end meets the second.
    s.hover(pos2(x(3.0), BODY));
    let outer = s.spot(a, Edge::End, FadeHandle::Outer);
    s.drag(outer, pos2(outer.x + 15.0, outer.y), bypass_key(), true);
    let (first, second) = (s.audio(a), s.audio(b));
    assert_eq!((first.fade_out, first.fade_out_after), (0.0, 1.0));
    assert_eq!((second.fade_in, second.fade_in_before), (1.0, 0.0));
    assert_eq!(s.steps(), 1);
    assert_eq!(s.undo_label().as_deref(), Some("Change Fade"));
    assert_eq!(
        bl_model::crossfades(s.doc().track(s.tracks[2]).unwrap().clips()).len(),
        1
    );
    // Both clips show the crossfade's handles from either: the first's outer
    // handle is a second into the second clip.
    s.hover(pos2(x(5.0), BODY));
    assert_eq!(s.spot(a, Edge::End, FadeHandle::Outer).x, x(4.0) + 15.0);
    // Pulling it out further reaches back before the second clip's start too.
    let outer = s.spot(b, Edge::Start, FadeHandle::Outer);
    s.drag(outer, pos2(outer.x - 15.0, outer.y), bypass_key(), true);
    let second = s.audio(b);
    assert_eq!((second.fade_in, second.fade_in_before), (1.0, 1.0));
    assert_eq!(s.audio(a).fade_out, 1.0);
}

// Spec: audio-clips R29. Clicking a handle selects it; Delete deletes that
// fade, back to 4 ms while Create Fades on Clip Edges is on, and a crossfade
// to the 4 ms crossfade.
#[test]
fn delete_with_a_handle_selected_deletes_its_fade() {
    let mut s = set(&[(2.0, 2.0), (4.0, 2.0)]);
    let [a, b] = s.clips[..] else { panic!() };
    s.edit_fade(a, 1.0, 0.0);
    s.hover(pos2(x(3.0), BODY));
    let peak = s.spot(a, Edge::Start, FadeHandle::Peak);
    let steps = s.steps();
    s.click(peak);
    assert_eq!(s.steps(), steps, "selecting is never an undo step");
    assert_eq!(s.state().selected_fade.unwrap().clip, a);
    assert!(s.h.main.arrangement().drawn_fades().limit.is_some());
    s.key(Key::Delete, Modifiers::NONE);
    assert_eq!(s.audio(a).fade_in, bl_model::EDGE_FADE_SECONDS);
    assert_eq!(s.undo_label().as_deref(), Some("Delete Fade"));
    assert!(s.doc().clip(a).is_some(), "the clip stays");
    assert_eq!(s.state().selected_fade, None, "the handle is gone with it");

    // A crossfade's handle: Delete puts back the 4 ms crossfade.
    s.h.shell
        .run(
            "arrangement.set_crossfade",
            json!({ "track": s.tracks[2], "boundary": 4 * BAR, "before": 0.0, "after": 1.0 }),
        )
        .unwrap();
    s.h.settle();
    s.hover(pos2(x(3.0), BODY));
    let outer = s.spot(a, Edge::End, FadeHandle::Outer);
    s.click(outer);
    s.key(Key::Delete, Modifiers::NONE);
    let (first, second) = (s.audio(a), s.audio(b));
    let half = bl_model::EDGE_FADE_SECONDS / 2.0;
    assert_eq!((first.fade_out, first.fade_out_after), (half, half));
    assert_eq!((second.fade_in, second.fade_in_before), (half, half));
    assert_eq!(s.undo_label().as_deref(), Some("Delete Fade"));
}

// Spec: audio-clips R28, R29. Create Fade's name follows the selection and its
// key makes the fade; Delete Fades' key deletes the selected clips' fades.
#[test]
fn the_fade_keys_make_and_delete_fades() {
    let mut s = set(&[(2.0, 4.0)]);
    let [a] = s.clips[..] else { panic!() };
    let label = |s: &Set| s.h.shell.dynamic_label("arrangement.create_fade").unwrap();
    assert_eq!(label(&s), "Create Fade");
    // A time selection holding the clip's start and not its end.
    s.h.shell
        .run(
            view_ids::SELECT_TIME,
            json!({ "start": 2 * BAR, "end": 3 * BAR, "tracks": [s.tracks[2]] }),
        )
        .unwrap();
    s.h.settle();
    assert_eq!(label(&s), "Create Fade In/Out");
    let before = s.doc().clone();
    s.shortcut("S-ARR-008");
    assert_eq!(s.audio(a).fade_in, 2.0, "a bar, from the clip's start");
    assert_eq!(s.undo_label().as_deref(), Some("Create Fade"));
    s.shortcut("S-EDIT-006");
    assert_eq!(*s.doc(), before);

    // Delete Fades on the selected clip.
    s.edit_fade(a, 1.0, 1.0);
    s.h.shell
        .run(
            bl_shell::editing_commands::SELECT_CLIPS,
            json!({ "clips": [a] }),
        )
        .unwrap();
    s.h.settle();
    s.shortcut("S-ARR-009");
    let audio = s.audio(a);
    assert_eq!(
        (audio.fade_in, audio.fade_out),
        (bl_model::EDGE_FADE_SECONDS, bl_model::EDGE_FADE_SECONDS)
    );
    assert_eq!(s.undo_label().as_deref(), Some("Delete Fades"));
}

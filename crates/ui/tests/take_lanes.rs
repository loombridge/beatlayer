//! Take lanes in the Arrangement, headless (WP-6.22): their rows and
//! headers, Show Take Lanes, selecting on them, the Arrangement's edits
//! there, the keys and the menus. Every gesture reaches the commands agents
//! run. Spec: comping R15 and R23 to R30.

mod common;

use bl_core::time::Ticks;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, Document, NewTakeLane, TakeLaneId, TrackId,
};
use bl_shell::arrangement_commands::{self as view_ids, ArrangementState, Lane};
use bl_shell::files::factory_default_set;
use bl_ui::arrangement::benchmark;
use common::{Harness, Node, Window, command};
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2, vec2};
use serde_json::json;

const BAR: i64 = Ticks::WHOLE.get();

/// The lanes' left edge and the new set's zoom: 30 points to a bar.
const LEFT: f32 = 320.0;
const POINTS_PER_BAR: f32 = 30.0;

/// Where bar `bar` (counting from 0) is across the lanes.
fn x(bar: f32) -> f32 {
    LEFT + bar * POINTS_PER_BAR
}

/// 3 Audio's main lane is 295 to 383 (88 points from y 119 after two 88
/// point tracks); its take lanes follow, 44 points each.
const MAIN: (f32, f32) = (295.0, 383.0);
const LANE_1: (f32, f32) = (383.0, 427.0);
const LANE_2: (f32, f32) = (427.0, 471.0);
const LANE_3: (f32, f32) = (471.0, 515.0);

struct Set {
    h: Harness,
    tracks: Vec<TrackId>,
    lanes: Vec<TakeLaneId>,
    /// The clip of each take lane.
    takes: Vec<ClipId>,
}

fn audio_clip(editor: &mut bl_model::Editor, file: u8, start: i64, bars: i64) -> Clip {
    Clip::new(
        editor.new_id(),
        Ticks(start * BAR),
        Ticks(bars * BAR),
        ClipData::Audio(AudioData::new(benchmark::media_hash(usize::from(file)))),
    )
}

/// The factory set with a clip on 3 Audio over bars 0 to 8 and three take
/// lanes under it: Take 1 over bars 0 to 4, Take 2 over bars 0 to 4 and Take
/// 3 over bars 2 to 6, each of a file of its own.
fn set() -> Set {
    let mut editor = factory_default_set();
    let tracks: Vec<TrackId> = editor.document().top_level().collect();
    let audio = tracks[2];
    let main = audio_clip(&mut editor, 9, 0, 8);
    editor
        .apply(Command::CreateClip {
            track: audio,
            clip: main,
        })
        .unwrap();
    let mut lanes = Vec::new();
    let mut takes = Vec::new();
    for (n, (start, bars)) in [(0, 4), (0, 4), (2, 4)].into_iter().enumerate() {
        let lane: TakeLaneId = editor.new_id();
        editor
            .apply(Command::AddTakeLane {
                track: audio,
                lane: NewTakeLane {
                    id: Some(lane),
                    name: format!("Take {}", n + 1),
                    clips: Vec::new(),
                },
                index: None,
            })
            .unwrap();
        let clip = audio_clip(&mut editor, n as u8 + 1, start, bars);
        takes.push(clip.id);
        editor
            .apply(Command::CreateTakeClip { lane, clip })
            .unwrap();
        lanes.push(lane);
    }
    editor.clear_history().unwrap();
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell = benchmark::shell_with(editor);
    h.settle();
    Set {
        h,
        tracks,
        lanes,
        takes,
    }
}

impl Set {
    fn audio(&self) -> TrackId {
        self.tracks[2]
    }

    fn doc(&self) -> &Document {
        self.h.shell.editor().document()
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

    fn nodes(&mut self) -> Vec<Node> {
        Harness::nodes(&self.h.settle())
    }

    /// Shows 3 Audio's take lanes by the toggle in its header.
    fn show_lanes(&mut self) {
        let at = self.centre(Role::Switch, "Show Take Lanes");
        self.h.click(at);
        self.h.settle();
        assert!(self.h.shell.arrangement().shows_take_lanes(self.audio()));
    }

    fn centre(&mut self, role: Role, label: &str) -> Pos2 {
        let nodes = self.nodes();
        let node = nodes
            .iter()
            .find(|n| n.role == role && n.label == label)
            .unwrap_or_else(|| panic!("no {role:?} named {label:?}"));
        let b = node.bounds.unwrap();
        pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
    }

    /// Where a take lane's header is (its name), by the lane's place.
    fn header(&mut self, n: usize) -> Pos2 {
        let label = format!("3 Audio, take lane Take {}", n + 1);
        let at = self.centre(Role::RowHeader, &label);
        // The name's left part, clear of the Audition Take Lane button.
        pos2(at.x - 30.0, at.y)
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

    fn drag(&mut self, from: Pos2, to: Pos2, release: bool) {
        self.h.frame(vec![Event::PointerMoved(from)]);
        self.h.frame(vec![Event::PointerButton {
            pos: from,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        for step in 1..=8 {
            let t = step as f32 / 8.0;
            self.h
                .frame(vec![Event::PointerMoved(from + (to - from) * t)]);
        }
        if release {
            self.release(to);
        }
    }

    fn release(&mut self, at: Pos2) {
        self.h.frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        self.h.settle();
    }

    fn key(&mut self, key: Key, modifiers: Modifiers) {
        self.h.key(key, modifiers);
        self.h.settle();
    }

    /// Selects time over the lane rows between two points.
    fn select(&mut self, from: Pos2, to: Pos2) {
        self.drag(from, to, true);
    }

    fn main_clips(&self) -> Vec<(i64, i64)> {
        let mut clips: Vec<(i64, i64)> = self
            .doc()
            .track(self.audio())
            .unwrap()
            .clips()
            .map(|c| (c.start.get() / BAR, c.end().get() / BAR))
            .collect();
        clips.sort_unstable();
        clips
    }

    fn on_lane(&self, n: usize) -> Vec<(i64, i64)> {
        let mut clips: Vec<(i64, i64)> = self
            .doc()
            .take_lane(self.lanes[n])
            .unwrap()
            .clips
            .iter()
            .map(|c| (c.start.get() / BAR, c.end().get() / BAR))
            .collect();
        clips.sort_unstable();
        clips
    }

    fn lane_ids(&self) -> Vec<TakeLaneId> {
        self.doc()
            .track(self.audio())
            .unwrap()
            .take_lanes()
            .map(|lane| lane.id)
            .collect()
    }
}

// Spec: comping R23, R24, R30 (L-ARR-012, L-ARR-020, L-ARR-021, I-CLICK-043).
#[test]
fn each_shown_take_lane_is_a_row_with_a_header_beside_it() {
    let mut s = set();
    // The toggle shows on 3 Audio, which has take lanes, and not on 4 Audio,
    // which has none; the lanes are hidden to begin with.
    let nodes = s.nodes();
    let toggles: Vec<&Node> = nodes
        .iter()
        .filter(|n| n.role == Role::Switch && n.label.ends_with("Take Lanes"))
        .collect();
    assert_eq!(toggles.len(), 1);
    assert_eq!(toggles[0].label, "Show Take Lanes");
    assert!(!nodes.iter().any(|n| n.label.contains("take lane Take")));
    s.show_lanes();
    // A header beside each, the name and the audition switch, 44 points tall
    // under the main lane (R23).
    let nodes = s.nodes();
    for (n, (top, bottom)) in [LANE_1, LANE_2, LANE_3].into_iter().enumerate() {
        let label = format!("3 Audio, take lane Take {}", n + 1);
        let header = nodes
            .iter()
            .find(|node| node.role == Role::RowHeader && node.label == label)
            .unwrap_or_else(|| panic!("no header {label:?}"));
        let b = header.bounds.unwrap();
        assert!((b.y0 as f32 - top).abs() < 1.5, "{label}: {b:?}");
        assert!((b.y1 as f32 - bottom).abs() < 2.5, "{label}: {b:?}");
        assert!(header.description.starts_with("Take lane of 3 Audio"));
        let switch = nodes
            .iter()
            .find(|node| {
                node.role == Role::Switch
                    && node.label == format!("Audition Take Lane Take {}", n + 1)
            })
            .unwrap_or_else(|| panic!("no audition switch for {label:?}"));
        assert_eq!(switch.toggled, Some(false));
    }
    // The lane's clips are drawn as the main lane's are, in their own rows.
    // Each row reaches assistive technology with its track's and its own
    // name, and whether it is auditioned (R30).
    for n in 1..=3 {
        let row = nodes
            .iter()
            .find(|node| {
                node.role == Role::Group && node.label == format!("3 Audio, take lane Take {n}")
            })
            .unwrap_or_else(|| panic!("no row node for Take {n}"));
        assert_eq!(row.value, "");
    }
    let clip_nodes = |nodes: &[Node]| {
        let mut values = nodes
            .iter()
            .filter(|n| n.role == Role::ListItem && n.label == "Audio Clip")
            .map(|n| n.value.clone())
            .collect::<Vec<_>>();
        values.sort();
        values
    };
    assert_eq!(
        clip_nodes(&nodes),
        [
            "1.1.1 to 5.1.1",
            "1.1.1 to 5.1.1",
            "1.1.1 to 9.1.1",
            "3.1.1 to 7.1.1"
        ],
        "the main lane's clip and each take lane's"
    );
    // The toggle hides them again (I-CLICK-043); it's view state.
    let at = s.centre(Role::Switch, "Hide Take Lanes");
    s.h.click(at);
    s.h.settle();
    assert!(!s.h.shell.arrangement().shows_take_lanes(s.audio()));
    assert!(!s.nodes().iter().any(|n| n.label.contains("take lane Take")));
    assert_eq!(s.steps(), 0, "showing take lanes is never an undo step");
}

// Spec: comping R24 (S-COMP-001). Cmd+Option+U shows and hides the selected
// tracks' take lanes, and Automation Mode hides them.
#[test]
fn cmd_option_u_shows_and_hides_the_selected_tracks_take_lanes() {
    let mut s = set();
    s.h.shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [s.audio()] }),
        )
        .unwrap();
    let keys = command() | Modifiers::ALT;
    s.key(Key::U, keys);
    assert!(s.h.shell.arrangement().shows_take_lanes(s.audio()));
    s.key(Key::U, keys);
    assert!(!s.h.shell.arrangement().shows_take_lanes(s.audio()));
    s.key(Key::U, keys);
    // Automation Mode hides the rows; the switch is kept.
    s.h.shell
        .run("arrangement.automation_mode", json!({ "on": true }))
        .unwrap();
    assert!(!s.nodes().iter().any(|n| n.label.contains("take lane Take")));
    assert!(!s.nodes().iter().any(|n| n.label == "Hide Take Lanes"));
    s.h.shell
        .run("arrangement.automation_mode", json!({ "on": false }))
        .unwrap();
    assert!(
        s.nodes()
            .iter()
            .any(|n| n.label.contains("take lane Take 3"))
    );
    assert_eq!(s.steps(), 0);
}

// Spec: comping R14, R30 (I-CLICK-040). The Audition Take Lane button
// auditions the lane or stops auditioning it.
#[test]
fn the_audition_button_auditions_the_lane() {
    let mut s = set();
    s.show_lanes();
    let one = s.centre(Role::Switch, "Audition Take Lane Take 1");
    s.h.click(one);
    s.h.settle();
    assert_eq!(
        s.doc().track(s.audio()).unwrap().audition(),
        Some(s.lanes[0])
    );
    assert_eq!(s.undo_label().as_deref(), Some("Audition Take Lane"));
    let nodes = s.nodes();
    assert_eq!(
        Harness::node(&nodes, "Audition Take Lane Take 1").toggled,
        Some(true),
        "lit while it's auditioned"
    );
    // Auditioning another switches the first off.
    let two = s.centre(Role::Switch, "Audition Take Lane Take 2");
    s.h.click(two);
    s.h.settle();
    assert_eq!(
        s.doc().track(s.audio()).unwrap().audition(),
        Some(s.lanes[1])
    );
    s.h.click(two);
    s.h.settle();
    assert_eq!(s.doc().track(s.audio()).unwrap().audition(), None);
    assert_eq!(s.steps(), 3);
}

// Spec: comping R26 (I-CLICK). Clicking a take lane's header selects it,
// Cmd-click adds or takes it away, and Shift-click takes in those between;
// selecting is never an undo step.
#[test]
fn clicking_headers_selects_take_lanes() {
    let mut s = set();
    s.show_lanes();
    let (one, two, three) = (s.header(0), s.header(1), s.header(2));
    s.click(one, Modifiers::NONE);
    let state = s.state();
    assert_eq!(state.selected_take_lanes, [s.lanes[0]]);
    assert_eq!(state.selected_tracks, [s.audio()]);
    s.click(three, command());
    assert_eq!(s.state().selected_take_lanes, [s.lanes[0], s.lanes[2]]);
    s.click(three, command());
    assert_eq!(s.state().selected_take_lanes, [s.lanes[0]]);
    s.click(three, Modifiers::SHIFT);
    assert_eq!(
        s.state().selected_take_lanes,
        s.lanes,
        "Shift-click takes in the lanes between"
    );
    s.click(two, Modifiers::NONE);
    assert_eq!(s.state().selected_take_lanes, [s.lanes[1]]);
    assert_eq!(s.steps(), 0);
    // The selected header is marked for assistive technology.
    let nodes = s.nodes();
    assert_eq!(
        Harness::node(&nodes, "3 Audio, take lane Take 2").selected,
        Some(true)
    );
}

// Spec: comping R26. Dragging across take lanes selects time on the lanes
// dragged across; clicking a take-lane clip's title bar selects the clip;
// Cmd-clicking inside a time selection adds or takes away the clicked lane,
// and with the only lane moves the insert marker instead.
#[test]
fn dragging_across_take_lanes_selects_time_on_them() {
    let mut s = set();
    s.show_lanes();
    s.select(pos2(x(1.0), LANE_1.0 + 20.0), pos2(x(3.0), LANE_2.0 + 20.0));
    let state = s.state();
    let time = state.time_selection.unwrap();
    assert_eq!((time.start, time.end), (Ticks(BAR), Ticks(3 * BAR)));
    assert_eq!(
        state.time_selection_lanes,
        [
            Lane::Take {
                track: s.audio(),
                lane: s.lanes[0]
            },
            Lane::Take {
                track: s.audio(),
                lane: s.lanes[1]
            },
        ],
        "the lanes dragged across, not the main lane"
    );
    assert_eq!(
        s.h.shell.arrangement().selection_take_lanes(),
        &s.lanes[..2]
    );
    // Cmd-click inside the selection on Take 3 adds it, and on Take 1 takes
    // it away.
    s.click(pos2(x(2.4), LANE_3.0 + 30.0), command());
    assert_eq!(s.state().time_selection_lanes.len(), 3);
    s.click(pos2(x(2.4), LANE_1.0 + 30.0), command());
    assert_eq!(
        s.h.shell.arrangement().selection_take_lanes(),
        [s.lanes[1], s.lanes[2]]
    );
    s.click(pos2(x(2.4), LANE_3.0 + 30.0), command());
    assert!(s.state().time_selection.is_some());
    // A click in the only lane left moves the insert marker instead.
    s.click(pos2(x(2.4), LANE_2.0 + 30.0), command());
    assert!(s.state().time_selection.is_none(), "the marker moved");
    assert_eq!(s.state().insert_marker, Ticks(2 * BAR));
    // A take-lane clip's title bar selects the clip.
    s.click(pos2(x(3.0), LANE_3.0 + 4.0), Modifiers::NONE);
    let state = s.state();
    assert_eq!(state.selected_clips, [s.takes[2]]);
    assert_eq!(s.h.shell.arrangement().selection_take_lanes(), [s.lanes[2]]);
    assert_eq!(s.steps(), 0, "selecting is never an undo step");
}

// Spec: comping R26. Clicking a take lane's background places the insert
// marker and selects the lane; Esc with no time selection moves the focus to
// the selected lanes' headers.
#[test]
fn escape_moves_to_the_selected_lanes_headers() {
    let mut s = set();
    s.show_lanes();
    s.click(pos2(x(5.0), LANE_2.0 + 30.0), Modifiers::NONE);
    assert_eq!(s.state().insert_marker, Ticks(5 * BAR));
    assert_eq!(s.h.shell.arrangement().selection_take_lanes(), [s.lanes[1]]);
    s.key(Key::Escape, Modifiers::NONE);
    assert_eq!(s.state().selected_take_lanes, [s.lanes[1]]);
}

// Spec: comping R17, R28 (S-COMP-002). Enter copies the time selected on a
// take lane to the main lane in one undo step.
#[test]
fn enter_copies_the_selected_take_lane_area_to_the_main_lane() {
    let mut s = set();
    s.show_lanes();
    s.select(pos2(x(1.0), LANE_2.0 + 20.0), pos2(x(3.0), LANE_2.0 + 20.0));
    s.key(Key::Enter, Modifiers::NONE);
    assert_eq!(s.main_clips(), [(0, 1), (1, 3), (3, 8)]);
    assert_eq!(s.steps(), 1);
    assert_eq!(
        s.undo_label().as_deref(),
        Some("Copy Selection to Main Lane")
    );
    assert_eq!(s.on_lane(1), [(0, 4)], "the take is left as it was");
    s.key(Key::Z, command());
    assert_eq!(s.main_clips(), [(0, 8)]);
    // A take-lane clip's title bar selected: Enter copies its span.
    s.click(pos2(x(3.0), LANE_3.0 + 4.0), Modifiers::NONE);
    s.key(Key::Enter, Modifiers::NONE);
    assert_eq!(s.main_clips(), [(0, 2), (2, 6), (6, 8)]);
}

// Spec: comping R15, R28 (S-COMP-003). T auditions the selected take lane,
// and again switches it off.
#[test]
fn t_auditions_the_selected_take_lane() {
    let mut s = set();
    s.show_lanes();
    let second = s.header(1);
    s.click(second, Modifiers::NONE);
    s.key(Key::T, Modifiers::NONE);
    assert_eq!(
        s.doc().track(s.audio()).unwrap().audition(),
        Some(s.lanes[1])
    );
    s.key(Key::T, Modifiers::NONE);
    assert_eq!(s.doc().track(s.audio()).unwrap().audition(), None);
    // With time selected on a take lane, too.
    s.select(pos2(x(1.0), LANE_3.0 + 20.0), pos2(x(3.0), LANE_3.0 + 20.0));
    s.key(Key::T, Modifiers::NONE);
    assert_eq!(
        s.doc().track(s.audio()).unwrap().audition(),
        Some(s.lanes[2])
    );
}

// Spec: comping R19, R28 (S-COMP-006). Cmd+Down and Cmd+Up, with time or
// clips selected, replace the main lane with the next or previous take and
// move the selection to the lane taken from.
#[test]
fn cmd_down_and_up_step_the_main_lane_through_the_takes() {
    let mut s = set();
    s.show_lanes();
    s.select(pos2(x(1.0), LANE_1.0 + 20.0), pos2(x(3.0), LANE_1.0 + 20.0));
    s.key(Key::ArrowDown, command());
    assert_eq!(s.undo_label().as_deref(), Some("Replace with Next Take"));
    assert_eq!(s.h.shell.arrangement().selection_take_lanes(), [s.lanes[1]]);
    assert_eq!(s.main_clips(), [(0, 1), (1, 3), (3, 8)]);
    s.key(Key::ArrowDown, command());
    assert_eq!(s.h.shell.arrangement().selection_take_lanes(), [s.lanes[2]]);
    s.key(Key::ArrowUp, command());
    assert_eq!(s.h.shell.arrangement().selection_take_lanes(), [s.lanes[1]]);
    assert_eq!(s.steps(), 3);
}

// Spec: comping R8, R28 (S-COMP-008). Cmd+Up and Cmd+Down with take lane
// headers selected move the lanes one place, and nothing past the ends.
#[test]
fn cmd_up_and_down_with_headers_selected_move_the_lanes() {
    let mut s = set();
    s.show_lanes();
    let first = s.header(0);
    s.click(first, Modifiers::NONE);
    s.key(Key::ArrowDown, command());
    assert_eq!(
        s.lane_ids(),
        [s.lanes[1], s.lanes[0], s.lanes[2]],
        "one place down"
    );
    assert_eq!(s.undo_label().as_deref(), Some("Move Take Lane(s)"));
    s.key(Key::ArrowDown, command());
    s.key(Key::ArrowDown, command());
    assert_eq!(s.lane_ids(), [s.lanes[1], s.lanes[2], s.lanes[0]]);
    assert_eq!(s.steps(), 2, "nothing past the last");
    s.key(Key::ArrowUp, command());
    assert_eq!(s.lane_ids(), [s.lanes[1], s.lanes[0], s.lanes[2]]);
}

// Spec: comping R5, R7, R25, R28 (S-COMP-005, S-COMP-007, S-EDIT-005).
// Cmd+D and Delete act on selected headers, Left goes to the main lane.
#[test]
fn duplicate_delete_and_left_act_on_selected_headers() {
    let mut s = set();
    s.show_lanes();
    let second = s.header(1);
    s.click(second, Modifiers::NONE);
    s.key(Key::D, command());
    assert_eq!(s.lane_ids().len(), 4);
    assert_eq!(s.lane_ids()[0], s.lanes[0]);
    assert_eq!(s.lane_ids()[1], s.lanes[1]);
    assert_eq!(s.undo_label().as_deref(), Some("Duplicate Take Lane(s)"));
    // The copy is selected: Delete deletes it.
    s.key(Key::Delete, Modifiers::NONE);
    assert_eq!(s.lane_ids(), s.lanes);
    assert_eq!(s.undo_label().as_deref(), Some("Delete Take Lane(s)"));
    // Left selects the track's title bar and hides the lanes.
    let third = s.header(2);
    s.click(third, Modifiers::NONE);
    s.key(Key::ArrowLeft, Modifiers::NONE);
    assert!(s.h.shell.arrangement().title_bars_selected());
    assert!(!s.h.shell.arrangement().shows_take_lanes(s.audio()));
    assert!(!s.nodes().iter().any(|n| n.label.contains("take lane Take")));
}

// Spec: comping R6, R28 (S-EDIT-008). Cmd+R types a new name in the first
// selected lane's header; Return keeps it, Esc leaves the name, and Tab moves
// to the next lane's name.
#[test]
fn cmd_r_types_a_name_in_the_header() {
    let mut s = set();
    s.show_lanes();
    let first = s.header(0);
    s.click(first, Modifiers::NONE);
    s.key(Key::R, command());
    s.h.settle();
    assert!(s.h.main.is_typing(), "the name is typed in the header");
    let press = |key: Key, modifiers: Modifiers| Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    };
    s.h.frame(vec![press(Key::A, command())]);
    s.h.frame(vec![Event::Text("Verse".to_owned())]);
    s.h.frame(vec![press(Key::Enter, Modifiers::NONE)]);
    s.h.settle();
    let name = |s: &Set, n: usize| s.doc().take_lane(s.lanes[n]).unwrap().name.clone();
    assert_eq!(name(&s, 0), "Verse");
    assert_eq!(s.undo_label().as_deref(), Some("Rename Take Lane"));
    assert!(!s.h.main.is_typing());
    // Esc leaves the name as it was.
    s.key(Key::R, command());
    s.h.frame(vec![press(Key::A, command())]);
    s.h.frame(vec![Event::Text("Chorus".to_owned())]);
    s.h.frame(vec![press(Key::Escape, Modifiers::NONE)]);
    s.h.settle();
    assert_eq!(name(&s, 0), "Verse");
    assert_eq!(s.steps(), 1);
    // Tab keeps the name and moves on to the next lane's.
    s.key(Key::R, command());
    s.h.frame(vec![press(Key::A, command())]);
    s.h.frame(vec![Event::Text("Intro".to_owned())]);
    s.h.frame(vec![press(Key::Tab, Modifiers::NONE)]);
    s.h.settle();
    assert_eq!(name(&s, 0), "Intro");
    assert!(s.h.main.is_typing(), "the next lane's name is typed now");
    s.h.frame(vec![press(Key::A, command())]);
    s.h.frame(vec![Event::Text("Bridge".to_owned())]);
    s.h.frame(vec![press(Key::Enter, Modifiers::NONE)]);
    s.h.settle();
    assert_eq!(name(&s, 1), "Bridge");
}

// Spec: comping R28 (S-ARR-022, I-SCROLL-006). Option with + or - makes the
// selected take lanes 16 points taller or shorter.
#[test]
fn option_plus_and_minus_resize_the_selected_lanes() {
    let mut s = set();
    s.show_lanes();
    let second = s.header(1);
    s.click(second, Modifiers::NONE);
    s.key(Key::Plus, Modifiers::ALT);
    assert_eq!(s.h.shell.arrangement().take_lane_height(s.lanes[1]), 60.0);
    assert_eq!(s.h.shell.arrangement().take_lane_height(s.lanes[0]), 44.0);
    assert_eq!(s.h.shell.arrangement().track_height(s.audio()), 88.0);
    s.key(Key::Minus, Modifiers::ALT);
    s.key(Key::Minus, Modifiers::ALT);
    assert_eq!(s.h.shell.arrangement().take_lane_height(s.lanes[1]), 28.0);
    // The wheel with Option over a take lane does the same.
    s.h.frame(vec![
        Event::PointerMoved(pos2(x(10.0), LANE_1.0 + 10.0)),
        Event::ModifiersChanged(Modifiers::ALT),
    ]);
    s.h.frame(vec![Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, 16.0),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::ALT,
    }]);
    s.h.settle();
    let heights: Vec<f32> = s
        .lanes
        .iter()
        .map(|l| s.h.shell.arrangement().take_lane_height(*l))
        .collect();
    assert!(
        heights.iter().any(|h| (*h - 28.0).abs() > 0.5),
        "the wheel resized a lane: {heights:?}"
    );
    assert_eq!(s.steps(), 0, "heights are view state");
}

// Spec: comping R27. A clip dragged by its title bar moves to the lane the
// pointer is over, a take lane or the main lane, in one undo step; Esc during
// the drag cancels it.
#[test]
fn dragging_a_clip_between_lanes_moves_it() {
    let mut s = set();
    s.show_lanes();
    let clip = s.takes[0];
    let before = s.doc().clone();
    // Down onto Take 2.
    s.drag(
        pos2(x(1.0), LANE_1.0 + 4.0),
        pos2(x(1.0), LANE_2.0 + 4.0),
        true,
    );
    assert_eq!(s.doc().clip_lane(clip), Some(s.lanes[1]));
    assert_eq!(s.steps(), 1, "the whole drag is one undo step");
    // Esc cancels one that is under way.
    s.drag(
        pos2(x(1.0), LANE_2.0 + 4.0),
        pos2(x(1.0), MAIN.0 + 4.0),
        false,
    );
    assert!(s.h.shell.gesture_open());
    s.h.key(Key::Escape, Modifiers::NONE);
    s.release(pos2(x(1.0), MAIN.0 + 4.0));
    assert_eq!(s.doc().clip_lane(clip), Some(s.lanes[1]));
    assert_eq!(s.steps(), 1);
    // Up onto the main lane: moved, not copied.
    s.drag(
        pos2(x(1.0), LANE_2.0 + 4.0),
        pos2(x(1.0), MAIN.0 + 4.0),
        true,
    );
    assert_eq!(s.doc().clip_lane(clip), None);
    assert_eq!(s.doc().clip_track(clip), Some(s.audio()));
    assert_eq!(s.on_lane(0), []);
    assert_eq!(s.steps(), 2);
    s.h.shell.run("edit.undo", json!({})).unwrap();
    s.h.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(*s.doc(), before);
}

// Spec: comping R29 (C-TAKE-001 to C-TAKE-008). A take lane header's menu
// lists its rows in order and runs them on the selected lanes.
#[test]
fn a_take_lane_headers_menu_lists_its_items() {
    let mut s = set();
    s.show_lanes();
    let at = s.header(1);
    s.h.frame(vec![Event::PointerMoved(at)]);
    for pressed in [true, false] {
        s.h.frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        }]);
    }
    let nodes = s.nodes();
    // The right-click selected the header.
    assert_eq!(s.state().selected_take_lanes, [s.lanes[1]]);
    let item = |label: &str| -> Node {
        nodes
            .iter()
            .find(|n| {
                n.role == Role::Button
                    && (n.label == label
                        || n.label
                            .strip_prefix(label)
                            .and_then(|keys| keys.strip_prefix(' '))
                            .is_some_and(|keys| !keys.contains(' ')))
            })
            .unwrap_or_else(|| panic!("no menu item {label:?}"))
            .clone()
    };
    for label in [
        "Insert Take Lane",
        "Duplicate",
        "Rename",
        "Delete",
        "Delete All Take Lanes",
        "Delete All Unused Take Lanes",
    ] {
        assert!(!item(label).disabled, "{label} works");
    }
    // Copy Selection to Main Lane needs time on the lane; Paste Bounced Audio
    // a copied source.
    assert!(item("Copy Selection to Main Lane").disabled);
    assert!(item("Paste Bounced Audio").disabled);
    // They show their keys.
    assert!(item("Duplicate").label.len() > "Duplicate".len());
    assert!(item("Insert Take Lane").label.contains('⇧') || cfg!(not(target_os = "macos")));
    // Delete All Unused Take Lanes: none of the takes is on the main lane yet
    // as a copy, so all three go.
    let b = item("Delete All Take Lanes").bounds.unwrap();
    s.h.click(pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    s.h.settle();
    assert!(s.lane_ids().is_empty());
}

// Spec: comping R29. Right-clicking a take lane's clip or background opens
// the clip's or the time selection's menu, with Copy Selection to Main Lane
// after Paste Bounced Audio.
#[test]
fn the_clip_and_time_menus_on_a_take_lane_add_copy_selection_to_main_lane() {
    let mut s = set();
    s.show_lanes();
    let right_click = |s: &mut Set, at: Pos2| -> Vec<Node> {
        s.h.frame(vec![Event::PointerMoved(at)]);
        for pressed in [true, false] {
            s.h.frame(vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Secondary,
                pressed,
                modifiers: Modifiers::NONE,
            }]);
        }
        s.nodes()
    };
    let nodes = right_click(&mut s, pos2(x(1.0), LANE_2.0 + 4.0));
    let copy = nodes
        .iter()
        .find(|n| n.role == Role::Button && n.label.starts_with("Copy Selection to Main Lane"))
        .expect("the clip's menu has Copy Selection to Main Lane");
    assert!(!copy.disabled, "a take-lane clip is selected");
    assert_eq!(s.state().selected_clips, [s.takes[1]]);
    s.key(Key::Escape, Modifiers::NONE);
    // The background of the main lane has none.
    s.h.shell
        .run(
            "arrangement.set_insert_marker",
            json!({ "at": 0, "track": s.audio() }),
        )
        .unwrap();
    let nodes = right_click(&mut s, pos2(x(20.0), MAIN.0 + 20.0));
    assert!(
        !nodes
            .iter()
            .any(|n| n.label.starts_with("Copy Selection to Main Lane")),
        "the main lane's menu has none"
    );
    s.key(Key::Escape, Modifiers::NONE);
    // The background of a take lane has it, and selects the lane.
    let nodes = right_click(&mut s, pos2(x(20.0), LANE_3.0 + 20.0));
    assert!(
        nodes
            .iter()
            .any(|n| n.label.starts_with("Copy Selection to Main Lane"))
    );
    assert_eq!(s.h.shell.arrangement().selection_take_lanes(), [s.lanes[2]]);
}

// Spec: comping R27. Cmd+D duplicates the time selected on a take lane, on
// the lane, leaving the main lane alone.
#[test]
fn the_edit_keys_act_on_time_selected_on_a_take_lane() {
    let mut s = set();
    s.show_lanes();
    s.select(pos2(x(1.0), LANE_1.0 + 20.0), pos2(x(2.0), LANE_1.0 + 20.0));
    s.key(Key::D, command());
    assert_eq!(s.on_lane(0), vec![(0, 2), (2, 3), (3, 4)]);
    assert_eq!(s.main_clips(), [(0, 8)], "the main lane was left alone");
    s.key(Key::Delete, Modifiers::NONE);
    assert_eq!(s.on_lane(0), vec![(0, 2), (3, 4)]);
    assert_eq!(s.main_clips(), [(0, 8)]);
    assert_eq!(s.steps(), 2);
}

// Spec: comping R26. Shift-click extends the time selection over the take
// lanes between, as well as the tracks.
#[test]
fn shift_click_extends_the_time_selection_over_lanes() {
    let mut s = set();
    s.show_lanes();
    s.select(pos2(x(1.0), LANE_1.0 + 20.0), pos2(x(2.0), LANE_1.0 + 20.0));
    s.click(pos2(x(5.5), LANE_3.0 + 30.0), Modifiers::SHIFT);
    let state = s.state();
    let time = state.time_selection.unwrap();
    assert_eq!((time.start, time.end), (Ticks(BAR), Ticks(5 * BAR)));
    assert_eq!(s.h.shell.arrangement().selection_take_lanes(), s.lanes);
    assert_eq!(state.time_selection_lanes.len(), 3, "not the main lane");
}

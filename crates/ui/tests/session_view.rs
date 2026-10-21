//! The Session View grid (WP-3.7), headless: the slots and the Main column's
//! scene rows sit where layout.md puts them at 1600 × 1000 points; clicks,
//! the arrows, and Shift and Cmd clicks change the selection; dragging moves
//! and copies a clip through the command layer; a dropped file lands in the
//! slot; double-clicking an empty MIDI slot makes a one-bar clip; every slot
//! reaches AccessKit with its track and scene named; and the grid registers
//! the clip slot in the strip's Tab order.
//!
//! Spec: session-view R1 to R14.

mod common;

use bl_model::{SceneId, TrackId};
use bl_ui::layout::mixer::{MixerLayout, SceneRow, Sections, SessionLayout, TrackColumn};
use bl_ui::layout::{REFERENCE, ShellLayout, TITLE_BAR};
use bl_ui::session_view::slot_id;
use common::{Harness, Node, Window, command};
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use serde_json::json;

fn harness() -> Harness {
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

/// The grid's layout, as the view builds it.
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
        let returns: Vec<TrackId> = h.shell.editor().document().returns().collect();
        let layout = MixerLayout::compute(
            session.slots,
            &vec![TrackColumn::track(); columns.len()],
            returns.len(),
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

    /// The clip slot cell for a track column and a scene, as the view draws
    /// it.
    fn cell(&self, track: usize, scene: usize) -> Rect {
        self.layout.columns[track]
            .rect
            .intersect(self.session.scene_row(scene, 0.0))
            .shrink(1.0)
    }

    fn centre(&self, track: usize, scene: usize) -> Pos2 {
        self.cell(track, scene).center()
    }

    /// The Main column's scene row.
    fn scene_row(&self, scene: usize) -> SceneRow {
        SceneRow::compute(self.session.scene_row(scene, 0.0), self.layout.main.rect)
    }

    /// The first audio track's column index.
    fn audio_column(&self, h: &Harness) -> usize {
        self.columns
            .iter()
            .position(|track| {
                h.shell
                    .editor()
                    .document()
                    .track(*track)
                    .is_some_and(|t| t.kind() == bl_model::TrackKind::Audio)
            })
            .expect("the default set has an audio track")
    }
}

fn state(h: &Harness) -> bl_shell::session_view_commands::SessionViewState {
    h.shell.session().state()
}

fn click(h: &mut Harness, at: Pos2, modifiers: Modifiers) {
    let held = Event::ModifiersChanged(modifiers);
    h.frame(vec![held.clone(), Event::PointerMoved(at)]);
    for pressed in [true, false] {
        h.frame(vec![
            held.clone(),
            Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers,
            },
        ]);
    }
    h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    h.settle();
}

fn double_click(h: &mut Harness, at: Pos2) {
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

/// A press at `from`, moves, and a release at `to`, with `modifiers` held.
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
    for step in 1..=4 {
        let at = from + (to - from) * (step as f32 / 4.0);
        h.frame(vec![held.clone(), Event::PointerMoved(at)]);
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
    h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    h.settle();
}

/// The modifier that copies while dragging: Option on macOS, Ctrl
/// elsewhere (I-DRAG-002).
fn copy_key() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::ALT
    } else {
        Modifiers::CTRL | Modifiers::COMMAND
    }
}

/// Drops `files` at `at` with `modifiers` held, as Finder or the browser
/// does.
fn drop_files(h: &mut Harness, at: Pos2, modifiers: Modifiers, files: &[std::path::PathBuf]) {
    h.frame(vec![
        Event::PointerMoved(at),
        Event::ModifiersChanged(modifiers),
    ]);
    for file in files {
        h.dropped
            .push(std::sync::Arc::new(DroppedFile(file.clone())));
    }
    h.frame(Vec::new());
    h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    h.settle();
}

/// A short audio file to drop, as one dragged from the browser or Finder.
fn wav_file(tag: &str) -> std::path::PathBuf {
    let file = std::env::temp_dir().join(format!("bl-ui-session-{}-{tag}.wav", std::process::id()));
    let samples: Vec<f32> = (0..4_800)
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
    std::fs::write(&file, bytes).unwrap();
    file
}

fn rect_of(node: &Node) -> Rect {
    let b = node.bounds.expect("a node with bounds");
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

// Spec: session-view R1, R2. The grid sits at layout.md's L-SES rows at
// 1600 × 1000 points.
#[test]
fn the_grid_sits_where_live_puts_it() {
    let mut h = harness();
    let g = Grid::new(&h);
    // L-SES-002: title bars y 61 to 78.
    assert_eq!(
        (g.session.title_bars.top(), g.session.title_bars.bottom()),
        (61.0, 78.0)
    );
    // L-SES-001: columns from x 320, 94 points each.
    assert_eq!(g.layout.columns[0].rect.left(), 320.0);
    assert_eq!(g.layout.columns[1].rect.left(), 414.0);
    assert_eq!(g.layout.columns[0].rect.width(), 94.0);
    // L-SES-003: one 18-point row per scene, under the title bars.
    let first = g.session.scene_row(0, 0.0);
    assert_eq!((first.top(), first.bottom()), (78.0, 96.0));
    let second = g.session.scene_row(1, 0.0);
    assert_eq!(second.top(), 96.0);
    // L-SES-006: the Main column at 1487 to 1591, with the launch button at
    // its left, the name between, and the number at its right edge.
    assert_eq!(
        (g.layout.main.rect.left(), g.layout.main.rect.right()),
        (1487.0, 1591.0)
    );
    let row = g.scene_row(0);
    assert_eq!((row.launch.left(), row.number.right()), (1487.0, 1591.0));
    assert!(row.name.left() >= row.launch.right() && row.name.right() <= row.number.left());
    // L-SES-005: the return columns, whose slots are blank.
    assert_eq!(g.layout.returns.len(), 2);
    assert!(g.layout.returns[0].rect.left() > g.layout.columns.last().unwrap().rect.right());
    // The first slot's node sits at its cell.
    let nodes = common::Harness::nodes(&h.settle());
    let node = nodes
        .iter()
        .find(|n| n.id == slot_id(g.columns[0], g.scenes[0]).accesskit_id())
        .expect("the first slot's node");
    assert!(rect_of(node).intersects(g.cell(0, 0)));
    assert_eq!(node.role, Role::ListItem);
}

// Spec: session-view R4, R5 (I-CLICK-002, I-CLICK-033). A click selects a
// slot; Cmd-click adds one; Shift-click extends; a scene row selects the
// scene; Esc goes to the title bar.
#[test]
fn clicks_and_modifiers_change_the_selection() {
    let mut h = harness();
    let g = Grid::new(&h);
    let (t0, t1) = (g.centre(0, 0), g.centre(1, 1));
    click(&mut h, t0, Modifiers::NONE);
    assert_eq!(state(&h).slots, [slotref(&g, 0, 0)]);
    // Cmd-click adds the second slot.
    click(&mut h, t1, Modifiers::COMMAND);
    assert_eq!(state(&h).slots.len(), 2);
    // Shift-click selects the rectangle from the anchor.
    click(&mut h, t0, Modifiers::NONE);
    let far = g.centre(1, 3);
    click(&mut h, far, Modifiers::SHIFT);
    assert_eq!(state(&h).slots.len(), 2 * 4, "two tracks by four scenes");
    // A scene row selects the scene.
    let scene = g.scene_row(2).name.center();
    click(&mut h, scene, Modifiers::NONE);
    assert_eq!(state(&h).scenes, [g.scenes[2]]);
    assert!(state(&h).slots.is_empty());
    // Esc goes to the highlighted track's title bar (S-SES-019).
    h.settle();
    let _ = h.key(Key::Escape, Modifiers::NONE);
    h.settle();
}

fn slotref(g: &Grid, track: usize, scene: usize) -> bl_shell::session_view_commands::SlotRef {
    bl_shell::session_view_commands::SlotRef {
        track: g.columns[track],
        scene: g.scenes[scene],
    }
}

// Spec: session-view R4 (I-FOCUS-005, S-SES-002, S-SES-010, S-SES-011).
#[test]
fn the_arrows_move_the_selection() {
    let mut h = harness();
    let g = Grid::new(&h);
    click(&mut h, g.centre(0, 0), Modifiers::NONE);
    let key = |h: &mut Harness, key: Key| {
        h.key(key, Modifiers::NONE);
        h.settle();
    };
    key(&mut h, Key::ArrowRight);
    assert_eq!(state(&h).slots, [slotref(&g, 1, 0)], "right moves a track");
    key(&mut h, Key::ArrowDown);
    assert_eq!(state(&h).slots, [slotref(&g, 1, 1)], "down moves a scene");
    key(&mut h, Key::PageDown);
    assert_eq!(
        state(&h).slots,
        [slotref(&g, 1, g.scenes.len() - 1)],
        "Page Down moves eight scenes, clamped to the last"
    );
    key(&mut h, Key::Home);
    assert_eq!(state(&h).slots, [slotref(&g, 0, g.scenes.len() - 1)]);
}

// Spec: session-view R9 (I-CLICK-004). Double-clicking an empty MIDI slot
// makes a one-bar clip there.
#[test]
fn double_clicking_an_empty_midi_slot_makes_a_clip() {
    let mut h = harness();
    let g = Grid::new(&h);
    let (track, scene) = (g.columns[0], g.scenes[0]);
    assert!(h.shell.editor().document().slot(track, scene).is_none());
    double_click(&mut h, g.centre(0, 0));
    let clip = h
        .shell
        .editor()
        .document()
        .slot(track, scene)
        .expect("the double-click made a clip")
        .clone();
    let bar = h
        .shell
        .editor()
        .document()
        .settings()
        .time_signatures
        .signature_at(bl_core::time::Ticks::ZERO)
        .bar_length();
    assert_eq!(clip.length, bar);
    // An audio slot is not a MIDI slot: the double-click makes nothing.
    let audio = g.audio_column(&h);
    double_click(&mut h, g.centre(audio, 1));
    assert!(
        h.shell
            .editor()
            .document()
            .slot(g.columns[audio], g.scenes[1])
            .is_none()
    );
}

// Spec: session-view R10 (I-DRAG-001, I-DRAG-002, S-SES-004). A drag moves
// a clip to another slot through the command layer; the copy modifier
// copies it.
#[test]
fn dragging_moves_a_clip_and_the_command_modifier_copies_it() {
    let mut h = harness();
    let g = Grid::new(&h);
    let (track, scene0, scene1) = (g.columns[0], g.scenes[0], g.scenes[1]);
    h.shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene0 }),
        )
        .unwrap();
    let clip = h.shell.editor().document().slot(track, scene0).unwrap().id;
    drag(&mut h, g.centre(0, 0), g.centre(0, 1), Modifiers::NONE);
    assert!(
        h.shell.editor().document().slot(track, scene0).is_none(),
        "the clip left its slot"
    );
    assert_eq!(
        h.shell.editor().document().slot(track, scene1).unwrap().id,
        clip,
        "it moved to the slot it was dropped on"
    );
    // Option-drag (Ctrl on Windows and Linux) copies it back.
    drag(&mut h, g.centre(0, 1), g.centre(0, 0), copy_key());
    assert!(
        h.shell.editor().document().slot(track, scene1).is_some(),
        "the source stays"
    );
    let copy = h.shell.editor().document().slot(track, scene0).unwrap().id;
    assert_ne!(copy, clip, "the copy is independent");
}

// Spec: session-view R9 (I-DROP-001). A file dropped on a slot becomes a
// Session clip there.
#[test]
fn dropping_a_file_places_a_clip_in_the_slot() {
    let mut h = harness();
    let g = Grid::new(&h);
    let audio = g.audio_column(&h);
    let track = g.columns[audio];
    let scene = g.scenes[0];
    // A real audio file, as one dragged from the browser or Finder.
    let file = wav_file("single");
    drop_files(
        &mut h,
        g.centre(audio, 0),
        Modifiers::NONE,
        std::slice::from_ref(&file),
    );
    assert!(
        h.shell.editor().document().slot(track, scene).is_some(),
        "the drop placed a clip in the slot"
    );
    let _ = std::fs::remove_file(&file);
}

// Spec: interactions I-DROP-003. Several files dropped on a slot lie down
// that track's scenes; with the command key held they spread across the
// tracks at the scene instead.
#[test]
fn dropping_several_files_spreads_them_across_the_tracks_with_the_command_key() {
    let mut h = harness();
    let g = Grid::new(&h);
    let audio = g.audio_column(&h);
    let (track, next) = (g.columns[audio], g.columns[audio + 1]);
    let (scene0, scene1, scene2) = (g.scenes[0], g.scenes[1], g.scenes[2]);
    let files = [wav_file("a"), wav_file("b"), wav_file("c"), wav_file("d")];
    // Without a modifier, one after another down the track's scenes.
    drop_files(&mut h, g.centre(audio, 0), Modifiers::NONE, &files[0..2]);
    {
        let doc = h.shell.editor().document();
        assert!(
            doc.slot(track, scene0).is_some() && doc.slot(track, scene1).is_some(),
            "the files lie down one track"
        );
    }
    // With the command key (Cmd; Ctrl on Windows and Linux) held: across
    // the tracks at the scene.
    drop_files(&mut h, g.centre(audio, 2), command(), &files[2..4]);
    {
        let doc = h.shell.editor().document();
        assert!(
            doc.slot(track, scene2).is_some() && doc.slot(next, scene2).is_some(),
            "the command key spread them across the tracks"
        );
    }
    for file in &files {
        let _ = std::fs::remove_file(file);
    }
}

/// A file dropped into the window, as the app's drag and drop gives egui.
#[derive(Debug)]
struct DroppedFile(std::path::PathBuf);

impl egui::DroppedFile for DroppedFile {
    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|error| error.to_string())
    }
}

// Spec: session-view R12; the accessibility spec. Every slot reaches
// AccessKit named with its track and its scene, and the grid registers the
// clip slot in the strip's Tab order.
#[test]
fn every_slot_reaches_accesskit_with_its_track_and_scene() {
    let mut h = harness();
    let nodes = common::Harness::nodes(&h.settle());
    let first = nodes
        .iter()
        .find(|n| n.label.starts_with("1 MIDI: 1"))
        .expect("track 1's first slot");
    assert_eq!(first.role, Role::ListItem);
    assert!(first.label.ends_with("(empty)"), "{}", first.label);
    assert!(
        nodes.iter().any(|n| n.label.starts_with("3 Audio: 1")),
        "an audio track's slot too"
    );
    // The grid registers a clip slot in the strip's Tab order, after the
    // title bar (the accessibility spec, R3, R4; observed in Live 12.4.6).
    assert!(
        bl_ui::focus::drawn(&h.ctx).entries().iter().any(|entry| {
            entry.control.is_some_and(|control| {
                control.place == bl_ui::mixer::Place::Mixer(bl_shell::mixer_view::MixerOf::Session)
                    && control.slot == bl_ui::focus::Slot::ClipSlot
            })
        }),
        "the clip slot is in the Tab order"
    );
}

// Launching from the grid (WP-3.8). The null device runs the engine on a
// timer, so these wait for what it publishes.

fn wait(h: &mut Harness, what: &str, done: impl FnMut(&Harness) -> bool) {
    h.wait(what, done);
}

fn states(h: &Harness) -> Vec<bl_engine::SlotStatus> {
    h.shell.session_states().to_vec()
}

fn slot_state(h: &Harness, track: TrackId, scene: SceneId) -> Option<bl_engine::SlotState> {
    h.shell
        .session_states()
        .iter()
        .find(|s| s.track == track && s.scene == scene)
        .map(|s| s.state)
}

/// A MIDI clip in the slot, made through its command.
fn clip_in(h: &mut Harness, track: TrackId, scene: SceneId) {
    h.shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
}

/// Where a slot's launch triangle or stop square is.
fn mark(g: &Grid, track: usize, scene: usize) -> Pos2 {
    let cell = g.cell(track, scene);
    pos2(cell.left() + 5.5, cell.center().y)
}

fn status(g: &Grid, track: usize) -> bl_ui::layout::mixer::StatusCells {
    bl_ui::layout::mixer::StatusCells::track(g.layout.columns[track].rect, g.session.status_row)
}

fn main_status(g: &Grid) -> bl_ui::layout::mixer::MainStatusCells {
    bl_ui::layout::mixer::MainStatusCells::compute(g.layout.main.rect, g.session.status_row)
}

fn toggled(h: &mut Harness, label: &str) -> Option<bool> {
    common::Harness::node(&common::Harness::nodes(&h.settle()), label).toggled
}

fn quantise_none(h: &mut Harness) {
    h.shell
        .run(
            "session.set_quantization",
            json!({ "quantization": "none" }),
        )
        .unwrap();
}

// Spec: session-view R31 (I-CLICK-001, I-CLICK-003). A launch triangle
// queues its clip for the grid, the slot draws what the engine publishes,
// and a track's stop button drops the queued launch.
#[test]
fn clicking_a_launch_triangle_queues_a_clip_and_a_stop_button_drops_it() {
    let mut h = harness();
    let g = Grid::new(&h);
    let (track, scene) = (g.columns[0], g.scenes[0]);
    clip_in(&mut h, track, scene);
    // Eight bars is sixteen seconds at 120 beats a minute: the launch waits.
    h.shell
        .run(
            "session.set_quantization",
            json!({ "quantization": "8_bars" }),
        )
        .unwrap();
    h.shell.run("transport.play", json!({})).unwrap();
    wait(&mut h, "the transport", |h| h.shell.playing());
    assert_eq!(toggled(&mut h, "Launch 1 MIDI: 1"), Some(false));

    click(&mut h, mark(&g, 0, 0), Modifiers::NONE);
    wait(&mut h, "the queued launch", |h| {
        slot_state(h, track, scene) == Some(bl_engine::SlotState::Queued)
    });
    assert_eq!(toggled(&mut h, "Launch 1 MIDI: 1"), Some(true));

    click(&mut h, status(&g, 0).stop.center(), Modifiers::NONE);
    wait(&mut h, "the dropped launch", |h| states(h).is_empty());
    assert_eq!(toggled(&mut h, "Launch 1 MIDI: 1"), Some(false));
    h.shell.run("transport.stop", json!({})).unwrap();
}

// Spec: session-view R31 (I-CLICK-001, I-CLICK-003, L-SES-008). With the
// transport stopped a click starts the clip at once; the stop square, a
// track's stop button and Stop All Clips each stop the grid's clips.
#[test]
fn the_stop_square_a_stop_button_and_stop_all_clips_stop_the_grid() {
    let mut h = harness();
    let g = Grid::new(&h);
    let (t0, t1, s0, s1) = (g.columns[0], g.columns[1], g.scenes[0], g.scenes[1]);
    clip_in(&mut h, t0, s0);
    clip_in(&mut h, t1, s0);
    quantise_none(&mut h);

    click(&mut h, mark(&g, 0, 0), Modifiers::NONE);
    click(&mut h, mark(&g, 1, 0), Modifiers::NONE);
    wait(&mut h, "both clips", |h| {
        slot_state(h, t0, s0) == Some(bl_engine::SlotState::Playing)
            && slot_state(h, t1, s0) == Some(bl_engine::SlotState::Playing)
    });
    assert_eq!(toggled(&mut h, "Launch 1 MIDI: 1"), Some(true));

    // The track's stop button stops its clip and leaves the other track's.
    click(&mut h, status(&g, 0).stop.center(), Modifiers::NONE);
    wait(&mut h, "track 1 to stop", |h| {
        slot_state(h, t0, s0).is_none() && slot_state(h, t1, s0).is_some()
    });
    // The stop square of an empty slot stops its track's clip.
    click(&mut h, mark(&g, 1, 1), Modifiers::NONE);
    wait(&mut h, "track 2 to stop", |h| states(h).is_empty());

    // Stop All Clips stops every track.
    click(&mut h, mark(&g, 0, 0), Modifiers::NONE);
    click(&mut h, mark(&g, 1, 0), Modifiers::NONE);
    wait(&mut h, "both clips again", |h| states(h).len() == 2);
    click(&mut h, main_status(&g).stop_all.center(), Modifiers::NONE);
    wait(&mut h, "every track to stop", |h| states(h).is_empty());
    assert!(h.shell.editor().document().slot(t0, s1).is_none());
    h.shell.run("transport.stop", json!({})).unwrap();
}

// Spec: session-view R31 (I-CLICK-009). A scene's launch button launches
// every clip in the scene.
#[test]
fn a_scene_launch_button_launches_the_scene() {
    let mut h = harness();
    let g = Grid::new(&h);
    let (t0, t1, scene) = (g.columns[0], g.columns[1], g.scenes[1]);
    clip_in(&mut h, t0, scene);
    clip_in(&mut h, t1, scene);
    quantise_none(&mut h);

    click(&mut h, g.scene_row(1).launch.center(), Modifiers::NONE);
    wait(&mut h, "the scene's clips", |h| {
        slot_state(h, t0, scene) == Some(bl_engine::SlotState::Playing)
            && slot_state(h, t1, scene) == Some(bl_engine::SlotState::Playing)
    });
    h.shell.run("transport.stop", json!({})).unwrap();
}

// Spec: session-view R31 (L-SES-012, S-TRANS-009). Back to Arrangement
// lights while a Session clip has taken a track over, a click on it and F10
// each return the tracks, and it goes dark.
#[test]
fn back_to_arrangement_lights_while_a_clip_plays_over_the_arrangement() {
    let mut h = harness();
    let g = Grid::new(&h);
    let (track, scene) = (g.columns[0], g.scenes[0]);
    clip_in(&mut h, track, scene);
    quantise_none(&mut h);
    assert_eq!(toggled(&mut h, "Back to Arrangement"), Some(false));

    for use_key in [false, true] {
        click(&mut h, mark(&g, 0, 0), Modifiers::NONE);
        wait(&mut h, "the button to light", |h| {
            !h.shell.session_over().is_empty()
        });
        assert_eq!(toggled(&mut h, "Back to Arrangement"), Some(true));
        if use_key {
            h.key(Key::F10, Modifiers::NONE);
        } else {
            click(&mut h, main_status(&g).back.center(), Modifiers::NONE);
        }
        wait(&mut h, "the button to clear", |h| {
            h.shell.session_over().is_empty()
        });
        assert_eq!(toggled(&mut h, "Back to Arrangement"), Some(false));
        assert!(states(&h).is_empty(), "the Session clip stopped");
    }
    h.shell.run("transport.stop", json!({})).unwrap();
}

// Spec: session-view R32 (S-SES-001). Enter launches the selected clip, and
// the selected scene.
#[test]
fn enter_launches_the_selected_clip_and_scene() {
    let mut h = harness();
    let g = Grid::new(&h);
    let (track, s0, s1) = (g.columns[0], g.scenes[0], g.scenes[1]);
    clip_in(&mut h, track, s0);
    clip_in(&mut h, track, s1);
    quantise_none(&mut h);

    // The clip's body, not its triangle, selects it.
    click(&mut h, g.centre(0, 0), Modifiers::NONE);
    assert_eq!(state(&h).slots, [slotref(&g, 0, 0)]);
    h.key(Key::Enter, Modifiers::NONE);
    wait(&mut h, "the selected clip", |h| {
        slot_state(h, track, s0) == Some(bl_engine::SlotState::Playing)
    });

    click(&mut h, g.scene_row(1).name.center(), Modifiers::NONE);
    assert_eq!(state(&h).scenes, [s1]);
    h.key(Key::Enter, Modifiers::NONE);
    wait(&mut h, "the selected scene", |h| {
        slot_state(h, track, s1) == Some(bl_engine::SlotState::Playing)
            && slot_state(h, track, s0).is_none()
    });
    h.shell.run("transport.stop", json!({})).unwrap();
}

// Spec: session-view R31. The launch buttons reach AccessKit named with
// their track and scene; the follow-action switch is there, on, and a click
// turns it off.
#[test]
fn the_launch_buttons_reach_accesskit_with_their_track_and_scene() {
    let mut h = harness();
    let g = Grid::new(&h);
    clip_in(&mut h, g.columns[0], g.scenes[0]);
    let nodes = common::Harness::nodes(&h.settle());
    for label in [
        "Launch 1 MIDI: 1",
        "Stop 1 MIDI",
        "Launch Scene 1",
        "Stop Clips 1 MIDI",
        "Stop All Clips",
        "Back to Arrangement",
    ] {
        let node = common::Harness::node(&nodes, label);
        assert_eq!(node.role, Role::Button, "{label}");
        assert!(!node.disabled, "{label}");
        assert!(node.bounds.is_some(), "{label}");
    }
    // Spec: follow-actions R8
    let follow = common::Harness::node(&nodes, "Enable Follow Actions Globally");
    assert!(!follow.disabled);
    assert_eq!(follow.toggled, Some(true));
    let b = follow.bounds.unwrap();
    h.click(egui::pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    assert!(!h.shell.launch_settings().follow_actions);
    let nodes = common::Harness::nodes(&h.settle());
    let follow = common::Harness::node(&nodes, "Enable Follow Actions Globally");
    assert_eq!(follow.toggled, Some(false));
}

//! The Arrangement View, headless (WP-1.8a): where its parts sit, that it
//! builds only what's on screen, that waveforms come and go with their
//! peaks, and that gestures, keys, and assistive technology reach its view
//! commands. Spec: arrangement-view.

mod common;

use std::collections::HashMap;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

use bl_commands::Context;
use bl_core::time::Ticks;
use bl_media::{MediaStatus, PeakEvent, Peaks};
use bl_model::ContentHash;
use bl_model::TrackId;
use bl_shell::arrangement::TransportView;
use bl_shell::arrangement_commands::{self as ids, ArrangementState};
use bl_shell::mixer_controls::Switch;
use bl_ui::arrangement::benchmark::{self, BenchmarkScene};
use bl_ui::arrangement::headers::{fold_id, title_id};
use bl_ui::arrangement::peaks::{PeakSource, PeakState};
use bl_ui::layout::{ArrangementLayout, Element, REFERENCE, ShellLayout};
use bl_ui::mixer::{Place, pan_id, send_id, switch_id, volume_field_id};
use common::{Harness, Node, Window, command};
use egui::accesskit::Role;
use egui::{
    Event, FullOutput, Id, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, Rect, TouchPhase,
    pos2, vec2,
};
use serde_json::json;

const BAR: i64 = Ticks::WHOLE.get();

fn state(h: &mut Harness) -> ArrangementState {
    serde_json::from_value(h.shell.run(ids::STATE, json!({})).unwrap()).unwrap()
}

/// The Arrangement's layout in a harness that draws Live's title bar, so
/// its coordinates are layout.md's window coordinates.
fn layout(h: &Harness) -> ArrangementLayout {
    let shell = ShellLayout::compute(h.size - vec2(0.0, bl_ui::layout::TITLE_BAR), true, true)
        .translate(vec2(0.0, bl_ui::layout::TITLE_BAR));
    let doc = h.shell.editor().document();
    let pinned = if h.shell.arrangement().show_return_tracks() {
        doc.returns().len() + 1
    } else {
        1
    };
    ArrangementLayout::compute(shell.main_view, h.shell.arrangement().overview(), pinned)
}

/// A harness showing the benchmark scene, with its peaks.
fn with_scene(scene: BenchmarkScene) -> Harness {
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell = benchmark::shell_with(scene.editor);
    h.main
        .arrangement_mut()
        .set_peak_source(Box::new(scene.peaks), None);
    h.settle();
    h
}

/// A drag with `modifiers` held throughout, as the platform reports them
/// (a modifiers event each frame, which egui keeps for that frame).
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
    for step in 1..=6 {
        let t = step as f32 / 6.0;
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

fn wheel(h: &mut Harness, at: Pos2, delta: egui::Vec2, modifiers: Modifiers) {
    h.frame(vec![
        Event::PointerMoved(at),
        Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta,
            phase: TouchPhase::Move,
            modifiers,
        },
    ]);
    h.settle();
}

#[test]
fn the_arrangement_sits_where_live_puts_it() {
    // Spec: arrangement-view R1, R2 (layout.md, L-ARR-001 to 017).
    let mut h = Harness::with_title_bar(Window::Main);
    let output = h.settle();
    let l = layout(&h);
    let near = |a: f32, b: f32| (a - b).abs() < 0.51;
    let rows = [
        ("overview", l.overview.unwrap(), (320.0, 61.0, 1295.0, 74.0)),
        ("beat-time ruler", l.beat_ruler, (320.0, 81.0, 1295.0, 99.0)),
        ("scrub area", l.scrub, (320.0, 99.0, 1295.0, 115.0)),
        ("lanes", l.lanes, (320.0, 119.0, 1295.0, 688.0)),
        ("headers", l.headers, (1297.0, 119.0, 1591.0, 688.0)),
        ("time ruler", l.time_ruler, (320.0, 746.0, 1295.0, 760.0)),
        (
            "grid indicator",
            l.grid_indicator,
            (1265.0, 728.0, 1293.0, 740.0),
        ),
    ];
    for (name, rect, (x0, y0, x1, y1)) in rows {
        assert!(
            near(rect.left(), x0)
                && near(rect.top(), y0)
                && near(rect.right(), x1)
                && near(rect.bottom(), y1),
            "{name}: {rect:?}"
        );
    }
    // The returns and Main, pinned under the lanes, 18 points each.
    let tops: Vec<f32> = (0..3).map(|i| l.pinned_row(i, 3).top()).collect();
    assert_eq!(tops, [688.0, 706.0, 725.0]);
    // L-ARR-010 and 017 from the header column's left.
    assert!(near(l.set_locator.left(), 1299.0) && near(l.set_locator.top(), 82.0));
    assert!(near(l.previous_locator.top(), 100.0));
    assert!(near(l.optimize_height.left(), 1372.0) && near(l.optimize_width.left(), 1390.0));
    assert!(near(l.waveform_zoom.left(), 1303.0));
    // A new track is 88 points tall, its title in the first header column.
    let nodes = Harness::nodes(&output);
    let first = nodes
        .iter()
        .find(|n| n.role == Role::RowHeader && n.label == "1 MIDI")
        .expect("the first track's title");
    let bounds = first.bounds.unwrap();
    assert!(
        near(bounds.y0 as f32, 119.0) && near(bounds.y1 as f32, 206.0),
        "{bounds:?}"
    );
    assert!(near(bounds.x0 as f32, 1297.0) && near(bounds.x1 as f32, 1400.0));
    let second = nodes
        .iter()
        .find(|n| n.role == Role::RowHeader && n.label == "2 MIDI")
        .unwrap();
    assert!(near(second.bounds.unwrap().y0 as f32, 207.0));
    // Without the Overview the rulers move up into its place.
    h.shell.run(ids::OVERVIEW, json!({ "on": false })).unwrap();
    h.settle();
    let l = layout(&h);
    assert!(l.overview.is_none());
    assert!(near(l.beat_ruler.top(), 61.0) && near(l.lanes.top(), 99.0));
}

#[test]
fn only_visible_clips_are_built_each_frame() {
    // Spec: arrangement-view R17. ADR-001: views stay virtualised.
    let mut h = with_scene(benchmark::scene(200, 25, 4));
    let count_visible = |h: &mut Harness| -> usize {
        let s = state(h);
        let doc = h.shell.editor().document();
        s.visible_tracks
            .iter()
            .flat_map(|t| doc.track(*t).unwrap().clips())
            .filter(|c| c.start < s.visible.end && c.end() > s.visible.start)
            .count()
    };
    let stats = h.main.arrangement().stats();
    assert_eq!(stats.total_clips, 5_000);
    assert_eq!(stats.clips, count_visible(&mut h));
    assert!(stats.clips > 0 && stats.clips < 100, "{stats:?}");
    assert!(stats.rows <= 8, "{stats:?}");
    // Each visible stereo clip draws two waveform bands.
    assert_eq!(stats.waveforms, 2 * stats.clips);

    // Scrolled down and along: other clips, still only those on screen.
    h.shell
        .run(ids::SCROLL, json!({ "x": 3_000.0, "y": 5_000.0 }))
        .unwrap();
    h.settle();
    let stats = h.main.arrangement().stats();
    assert_eq!(stats.clips, count_visible(&mut h));
    assert!(stats.clips < 100);

    // A frame where nothing in the lanes changed builds nothing.

    h.frame(vec![Event::PointerMoved(pos2(10.0, 10.0))]);
    assert!(!h.main.arrangement().stats().rebuilt);

    // Everything on screen: all 200 tracks and 5,000 clips are built.
    h.shell
        .run(ids::OPTIMIZE_HEIGHT, json!({ "on": true }))
        .unwrap();
    h.shell
        .run(ids::OPTIMIZE_WIDTH, json!({ "on": true }))
        .unwrap();
    h.settle();
    let stats = h.main.arrangement().stats();
    assert_eq!(stats.rows, 200);
    assert_eq!(stats.clips, 5_000);
}

/// A peak source whose peaks become ready, or go away, when the test says.
#[derive(Clone, Default)]
struct Switchable {
    ready: Arc<Mutex<HashMap<ContentHash, Arc<Peaks>>>>,
}

impl PeakSource for Switchable {
    fn request(&self, media: ContentHash) -> PeakState {
        self.ready
            .lock()
            .unwrap()
            .get(&media)
            .map_or(PeakState::Pending, |p| PeakState::Ready(Arc::clone(p)))
    }
}

fn media(hash: ContentHash) -> bl_media::ContentHash {
    bl_media::ContentHash::from_bytes(hash.0)
}

#[test]
fn waveforms_appear_with_their_peaks_and_go_when_invalidated() {
    // Spec: arrangement-view R4.
    let scene = benchmark::scene(6, 12, 1);
    let peaks = Arc::new(
        Peaks::from_samples(&benchmark::media_samples(
            &mut benchmark::Random::new(1),
            16.0,
        ))
        .unwrap(),
    );
    let hash = benchmark::media_hash(0);
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell = benchmark::shell_with(scene.editor);
    let source = Switchable::default();
    let (events, received) = channel::<PeakEvent>();
    h.main
        .arrangement_mut()
        .set_peak_source(Box::new(source.clone()), Some(received));
    h.settle();
    let stats = h.main.arrangement().stats();
    assert!(stats.clips > 0);
    assert_eq!(stats.waveforms, 0, "pending peaks draw no waveform");

    let send = |events: &Sender<PeakEvent>, event| events.send(event).unwrap();
    source
        .ready
        .lock()
        .unwrap()
        .insert(hash, Arc::clone(&peaks));
    send(&events, PeakEvent::Ready(media(hash)));
    h.settle();
    let stats = h.main.arrangement().stats();
    assert_eq!(stats.waveforms, 2 * stats.clips, "{stats:?}");

    // The file changed: the cache invalidates its peaks and rebuilds them.
    source.ready.lock().unwrap().clear();
    send(
        &events,
        PeakEvent::Invalidated {
            hash: media(hash),
            status: MediaStatus::Missing,
        },
    );
    h.settle();
    assert_eq!(h.main.arrangement().stats().waveforms, 0);
    source.ready.lock().unwrap().insert(hash, peaks);
    send(&events, PeakEvent::Ready(media(hash)));
    h.settle();
    let stats = h.main.arrangement().stats();
    assert_eq!(stats.waveforms, 2 * stats.clips);
}

#[test]
fn clicks_and_drags_place_the_marker_and_select_time() {
    // Spec: arrangement-view R12 (I-CLICK-011, I-DRAG-010).
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let l = layout(&h);
    let tracks: Vec<_> = h.shell.editor().document().top_level().collect();
    // 30 points to a bar: bar 5 is 120 points in; the second track's lane
    // is 88 to 176 points down. Snapping goes to the nearest bar.
    h.click(pos2(l.lanes.left() + 124.0, l.lanes.top() + 130.0));
    let s = state(&mut h);
    assert_eq!(s.insert_marker, Ticks(4 * BAR));
    assert_eq!(s.selected_tracks, [tracks[1]]);
    assert!(!h.shell.editor().can_undo(), "not an undo step");

    drag(
        &mut h,
        pos2(l.lanes.left() + 61.0, l.lanes.top() + 40.0),
        pos2(l.lanes.left() + 239.0, l.lanes.top() + 200.0),
        Modifiers::NONE,
    );
    let s = state(&mut h);
    let selection = s.time_selection.expect("a time selection");
    assert_eq!(
        (selection.start, selection.end),
        (Ticks(2 * BAR), Ticks(8 * BAR))
    );
    assert_eq!(s.selected_tracks, tracks[0..3]);

    // Z zooms to it.
    h.key(Key::Z, Modifiers::NONE);
    let zoomed = state(&mut h);
    assert_eq!(zoomed.visible.start, Ticks(2 * BAR));
    h.key(Key::X, Modifiers::NONE);
    assert_eq!(state(&mut h).visible.start, Ticks::ZERO);
}

#[test]
fn the_wheel_rulers_and_trackpad_scroll_and_zoom() {
    // Spec: arrangement-view R6, R7.
    let mut h = with_scene(benchmark::scene(30, 4, 1));
    let l = layout(&h);
    let middle = l.lanes.center();
    wheel(&mut h, middle, vec2(0.0, -6.0), Modifiers::NONE);
    assert!(state(&mut h).scroll_y > 0.0, "the wheel scrolls the tracks");
    wheel(&mut h, middle, vec2(0.0, -6.0), Modifiers::SHIFT);
    let s = state(&mut h);
    assert!(s.visible.start > Ticks::ZERO, "Shift turns it sideways");
    let before = s.ticks_per_point;
    wheel(&mut h, middle, vec2(0.0, 6.0), command());
    assert!(
        state(&mut h).ticks_per_point < before,
        "Cmd with the wheel zooms in"
    );
    h.frame(vec![Event::PointerMoved(middle), Event::Zoom(0.5)]);
    h.settle();
    assert!(state(&mut h).ticks_per_point > before, "a pinch zooms out");

    // Dragging down in the beat-time ruler zooms in around the press.
    let before = state(&mut h);
    let at = pos2(l.beat_ruler.left() + 300.0, l.beat_ruler.center().y);
    drag(&mut h, at, at + vec2(0.0, 60.0), Modifiers::NONE);
    let after = state(&mut h);
    assert!(after.ticks_per_point < before.ticks_per_point / 1.5);
    let x = |s: &ArrangementState, t: f64| (t - s.visible.start.get() as f64) / s.ticks_per_point;
    let anchor = before.visible.start.get() as f64 + 300.0 * before.ticks_per_point;
    assert!((x(&before, anchor) - x(&after, anchor)).abs() < 1.5);
    // Sideways, it scrolls the other way, like grabbing the lanes.
    let before = state(&mut h);
    drag(&mut h, at, at - vec2(100.0, 0.0), Modifiers::NONE);
    assert!(state(&mut h).visible.start > before.visible.start);
    // So does the time ruler.
    let before = state(&mut h);
    let t = l.time_ruler.center();
    drag(&mut h, t, t + vec2(80.0, 0.0), Modifiers::NONE);
    assert!(state(&mut h).visible.start < before.visible.start);
    // Cmd-Option (Ctrl-Alt) dragging in the lanes pans both ways.
    let before = state(&mut h);
    let pan = command() | Modifiers::ALT;
    drag(&mut h, middle, middle - vec2(50.0, 0.0), pan);
    assert!(state(&mut h).visible.start > before.visible.start);
}

#[test]
fn the_middle_button_pans_the_lanes() {
    // Spec: view-options R1, arrangement-view R19 (I-DRAG-058).
    let mut h = with_scene(benchmark::scene(30, 4, 1));
    let l = layout(&h);
    let middle = l.lanes.center();
    // Room to pan: the lanes are scrolled down and to the right first, by
    // the same drag Cmd-Option makes.
    let pan = command() | Modifiers::ALT;
    drag(&mut h, middle, middle - vec2(0.0, 200.0), pan);
    drag(&mut h, middle, middle - vec2(200.0, 0.0), pan);
    let before = state(&mut h);
    assert!(before.scroll_y > 50.0, "the lanes are scrolled down");
    assert!(
        before.visible.start > Ticks::ZERO,
        "the lanes are scrolled right"
    );
    // A middle-button drag of 100 points right and 50 down pans the lanes
    // with the pointer.
    h.middle_drag(middle, middle + vec2(100.0, 50.0));
    let after = state(&mut h);
    let right = (before.visible.start.get() as f64 - after.visible.start.get() as f64)
        / before.ticks_per_point;
    assert!(
        (right - 100.0).abs() < 0.5,
        "the lanes moved 100 points right: {right}"
    );
    assert!(
        (before.scroll_y - after.scroll_y - 50.0).abs() < 0.5,
        "the lanes moved 50 points down: {}",
        before.scroll_y - after.scroll_y
    );
    // Nothing else changed: no selection, no clip, no undo step.
    assert_eq!(before.selected_clips, after.selected_clips);
    assert_eq!(before.selected_tracks, after.selected_tracks);
    assert_eq!(before.time_selection, after.time_selection);
    assert!(!h.shell.editor().can_undo(), "not an undo step");

    // The same drag with Cmd-Option (Ctrl-Alt) held gives the same view.
    let mut h = with_scene(benchmark::scene(30, 4, 1));
    let l = layout(&h);
    let middle = l.lanes.center();
    drag(&mut h, middle, middle - vec2(0.0, 200.0), pan);
    drag(&mut h, middle, middle - vec2(200.0, 0.0), pan);
    drag(&mut h, middle, middle + vec2(100.0, 50.0), pan);
    let keys = state(&mut h);
    assert!(
        (keys.visible.start.get() as f64 - after.visible.start.get() as f64).abs() < 0.5,
        "the middle button pans as Cmd-Option does"
    );
    assert!((keys.scroll_y - after.scroll_y).abs() < 0.5);
}

#[test]
fn keys_run_the_view_commands() {
    // Spec: arrangement-view R6, R8, R9, R11, R14, R15; shortcuts S-ZOOM-003
    // to 005, S-GRID-002 to 006, S-VIEW-011, S-ARR-020 to 026, 030, 031,
    // 034, S-TRACK-010, S-A11Y-003.
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let tracks: Vec<_> = h.shell.editor().document().top_level().collect();
    let tpp = state(&mut h).ticks_per_point;
    h.key(Key::Plus, Modifiers::NONE);
    assert!((state(&mut h).ticks_per_point - tpp / 2.0).abs() < 1e-6);
    h.key(Key::Minus, Modifiers::NONE);
    assert!((state(&mut h).ticks_per_point - tpp).abs() < 1e-6);

    h.key(Key::Num1, command());
    assert_eq!(state(&mut h).grid.label, "1/2", "Cmd-1 narrows the grid");
    h.key(Key::Num2, command());
    assert_eq!(state(&mut h).grid.label, "1/1");
    h.key(Key::Num4, command());
    assert!(!state(&mut h).grid.snap, "Cmd-4 switches snapping");
    h.key(Key::Num5, command());
    assert!(state(&mut h).grid.fixed, "Cmd-5 fixes the grid");
    assert_eq!(state(&mut h).grid.label, "1/16");
    h.key(Key::Num3, command());
    assert_eq!(state(&mut h).grid.label, "1/16T");

    let option = Modifiers::ALT;
    h.key(Key::O, command() | option);
    assert!(!state(&mut h).overview, "Cmd-Option-O hides the Overview");
    h.key(Key::F, option | Modifiers::SHIFT);
    assert!(!state(&mut h).follow, "Option-Shift-F switches Follow");

    h.key(Key::W, Modifiers::NONE);
    assert!(state(&mut h).optimize_width);
    h.key(Key::H, Modifiers::NONE);
    assert!(state(&mut h).optimize_height);
    h.key(Key::H, Modifiers::NONE);
    h.key(Key::W, Modifiers::NONE);

    // Esc goes to the first track's title bar; U then folds it; Down moves
    // to the next track, Left folds, Right unfolds.
    h.key(Key::Escape, Modifiers::NONE);
    assert!(h.shell.active_contexts().contains(&Context::TrackHeaders));
    h.key(Key::U, Modifiers::NONE);
    assert_eq!(state(&mut h).folded_tracks, [tracks[0]]);
    h.key(Key::ArrowDown, Modifiers::NONE);
    assert_eq!(state(&mut h).selected_tracks, [tracks[1]]);
    h.key(Key::ArrowLeft, Modifiers::NONE);
    assert!(h.shell.arrangement().is_folded(tracks[1]));
    h.key(Key::ArrowRight, Modifiers::NONE);
    assert!(!h.shell.arrangement().is_folded(tracks[1]));
    h.key(Key::ArrowUp, Modifiers::NONE);
    assert_eq!(state(&mut h).selected_tracks, [tracks[0]]);
    h.key(Key::U, option);
    assert!(
        state(&mut h).folded_tracks.is_empty(),
        "Option-U unfolds all"
    );
    h.key(Key::Plus, option);
    assert_eq!(h.shell.arrangement().track_height(tracks[0]), 104.0);
    h.key(Key::Minus, option);
    assert_eq!(h.shell.arrangement().track_height(tracks[0]), 88.0);
}

#[test]
fn headers_fold_select_and_resize() {
    // Spec: arrangement-view R3, R14 (I-CLICK-019, I-DRAG-018).
    let mut h = Harness::with_title_bar(Window::Main);
    let output = h.settle();
    let tracks: Vec<_> = h.shell.editor().document().top_level().collect();
    let nodes = Harness::nodes(&output);
    let fold = nodes
        .iter()
        .find(|n| n.id == fold_id(tracks[0]).accesskit_id())
        .expect("a fold button");
    let b = fold.bounds.unwrap();
    h.click(pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    assert!(h.shell.arrangement().is_folded(tracks[0]));
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    let title = nodes
        .iter()
        .find(|n| n.id == title_id(tracks[1]).accesskit_id())
        .unwrap();
    let t = title.bounds.unwrap();
    // The second track now starts 18 points down, under the folded first.
    assert!((t.y0 - 137.0).abs() < 0.6, "{t:?}");
    h.click(pos2((t.x0 + 50.0) as f32, (t.y0 + 30.0) as f32));
    assert_eq!(state(&mut h).selected_tracks, [tracks[1]]);
    assert!(h.shell.arrangement().title_bars_selected());
    // Dragging the split line under the second track's header.
    let bottom = t.y1 as f32;
    drag(
        &mut h,
        pos2((t.x0 + 50.0) as f32, bottom - 1.0),
        pos2((t.x0 + 50.0) as f32, bottom + 39.0),
        Modifiers::NONE,
    );
    let height = h.shell.arrangement().track_height(tracks[1]);
    assert!((height - 128.0).abs() < 2.0, "{height}");
}

#[test]
fn the_track_controls_hide_their_parts() {
    // Spec: arrangement-view R21, R22, R23, R24.
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let doc = h.shell.editor().document();
    let tracks: Vec<TrackId> = doc.top_level().collect();
    let returns: Vec<TrackId> = doc.returns().collect();
    let midi = tracks[0];
    let audio = tracks[2];
    let near = |a: f32, b: f32| (a - b).abs() < 0.51;
    let has = |nodes: &[Node], id: Id| nodes.iter().any(|n| n.id == id.accesskit_id());

    // While they show, the In/Out choosers, the volume field, the pan, and
    // the sends are in the tree.
    let nodes = Harness::nodes(&h.settle());
    assert!(nodes.iter().any(|n| n.label == "Input Type"));
    assert!(has(&nodes, volume_field_id(Place::Header, midi)));
    assert!(has(&nodes, pan_id(Place::Header, midi)));
    assert!(has(&nodes, send_id(Place::Header, audio, returns[0])));

    // In/Out off: the title takes the In/Out column's width, and the
    // header column and the lanes keep theirs (R21).
    h.shell
        .run(ids::SHOW_IN_OUT, json!({ "on": false }))
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    assert!(
        !nodes.iter().any(|n| n.label == "Input Type"),
        "the In/Out choosers are gone"
    );
    let first = nodes
        .iter()
        .find(|n| n.role == Role::RowHeader && n.label == "1 MIDI")
        .expect("the first track's title");
    let bounds = first.bounds.unwrap();
    assert!(near(bounds.x1 as f32, 1476.0), "{bounds:?}");
    let l = layout(&h);
    assert!(near(l.lanes.right(), 1295.0), "the lanes don't move");
    assert!(
        near(l.headers.right(), 1591.0),
        "the header column keeps its width"
    );

    // Volume off: the volume field, the pan, and the sends go; the
    // activator, Solo, and Arm stay (R22).
    h.shell
        .run(ids::SHOW_VOLUME, json!({ "on": false }))
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    assert!(!has(&nodes, volume_field_id(Place::Header, midi)));
    assert!(!has(&nodes, pan_id(Place::Header, midi)));
    assert!(!has(&nodes, send_id(Place::Header, audio, returns[0])));
    assert!(has(&nodes, switch_id(Place::Header, Switch::Solo, midi)));

    // Return Tracks off: the returns' rows go, Main's stays, and the lanes
    // take the height the returns leave (R23).
    h.shell
        .run(ids::SHOW_RETURN_TRACKS, json!({ "on": false }))
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    let headers: Vec<&str> = nodes
        .iter()
        .filter(|n| n.role == Role::RowHeader)
        .map(|n| n.label.as_str())
        .collect();
    assert!(
        !headers.contains(&"A Return") && !headers.contains(&"B Return"),
        "{headers:?}"
    );
    assert!(headers.contains(&"Main"), "Main's row stays");
    let l = layout(&h);
    assert!(
        near(l.lanes.bottom(), 725.0),
        "the lanes take the returns' height"
    );

    // Switching back gives the layout of a new set exactly.
    for id in [ids::SHOW_IN_OUT, ids::SHOW_VOLUME, ids::SHOW_RETURN_TRACKS] {
        h.shell.run(id, json!({ "on": true })).unwrap();
    }
    let nodes = Harness::nodes(&h.settle());
    assert!(nodes.iter().any(|n| n.label == "Input Type"));
    assert!(has(&nodes, volume_field_id(Place::Header, midi)));
    assert!(has(&nodes, pan_id(Place::Header, midi)));
    assert!(has(&nodes, send_id(Place::Header, audio, returns[0])));
    let headers: Vec<&str> = nodes
        .iter()
        .filter(|n| n.role == Role::RowHeader)
        .map(|n| n.label.as_str())
        .collect();
    assert!(headers.contains(&"A Return") && headers.contains(&"Main"));
    let l = layout(&h);
    assert!(near(l.lanes.bottom(), 688.0), "the lanes are as they were");
    let first = nodes
        .iter()
        .find(|n| n.role == Role::RowHeader && n.label == "1 MIDI")
        .expect("the first track's title");
    let bounds = first.bounds.unwrap();
    assert!(near(bounds.x1 as f32, 1400.0), "{bounds:?}");

    // At every interface zoom, the parts that show keep their places (R24,
    // interface-zoom R1).
    h.shell
        .run(ids::SHOW_IN_OUT, json!({ "on": false }))
        .unwrap();
    for percent in [50, 75, 100, 150, 200] {
        h.shell
            .run("view.set_zoom", json!({ "percent": percent }))
            .unwrap();
        let nodes = Harness::nodes(&h.settle());
        let first = nodes
            .iter()
            .find(|n| n.role == Role::RowHeader && n.label == "1 MIDI")
            .expect("the first track's title");
        let bounds = first.bounds.unwrap();
        assert!(
            near((bounds.x1 - bounds.x0) as f32, 179.0),
            "at {percent}%: {bounds:?}"
        );
    }
}

#[test]
fn the_follow_switch_and_the_optimize_buttons_work() {
    // Spec: arrangement-view R9, R10, R13 (L-CTRL-012, L-ARR-017).
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let shell_layout =
        ShellLayout::compute(REFERENCE, true, true).translate(vec2(0.0, bl_ui::layout::TITLE_BAR));
    h.click(shell_layout.rect(Element::Follow).center());
    assert!(!state(&mut h).follow);
    h.click(shell_layout.rect(Element::Follow).center());
    assert!(state(&mut h).follow);
    let l = layout(&h);
    h.click(l.optimize_width.center());
    assert!(state(&mut h).optimize_width);
    h.click(l.optimize_height.center());
    assert!(state(&mut h).optimize_height);
    // The playhead pages the view while the song plays.
    h.shell.set_transport(TransportView {
        playing: true,
        position: Ticks(500 * BAR),
        ..TransportView::default()
    });
    h.click(l.optimize_width.center());
    h.shell.set_transport(TransportView {
        playing: true,
        position: Ticks(600 * BAR),
        ..TransportView::default()
    });
    h.settle();
    assert_eq!(state(&mut h).visible.start, Ticks(600 * BAR));
}

/// The rectangle the beat-time ruler paints the loop brace in, from the
/// view and the set's loop, as `rulers::beat_ruler` computes it.
fn brace(h: &mut Harness, ruler: Rect) -> Rect {
    let s = state(h);
    let loop_ = h.shell.editor().document().settings().arrangement_loop;
    let x_of = |at: Ticks| {
        ruler.left() + ((at.get() as f64 - s.visible.start.get() as f64) / s.ticks_per_point) as f32
    };
    let (l0, l1) = (x_of(loop_.start), x_of(loop_.end()));
    Rect::from_min_max(
        pos2(l0.max(ruler.left() - 1.0), ruler.bottom() - 6.0),
        pos2(l1.min(ruler.right() + 1.0), ruler.bottom() - 1.0),
    )
}

// Spec: arrangement-view R13.
#[test]
fn the_loop_brace_is_painted_in_its_on_and_off_colours() {
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let ruler = layout(&h).beat_ruler;
    // One colour while the loop is on, another while it's off.
    let on = h.theme.colors.loop_on.0;
    let off = h.theme.colors.loop_off.0;
    assert_ne!(on, off, "the theme gives the brace two colours");
    let near = |a: f32, b: f32| (a - b).abs() < 0.51;
    // The fill of the rectangle painted where the brace sits, found by its
    // rectangle, the way `accessibility.rs` finds the focus strokes.
    let the_brace = |output: &FullOutput, want: Rect| {
        common::painted_rects(output)
            .into_iter()
            .find(|(r, _)| {
                near(r.min.x, want.min.x)
                    && near(r.max.x, want.max.x)
                    && near(r.min.y, want.min.y)
                    && near(r.max.y, want.max.y)
            })
            .map(|(_, fill)| fill)
    };
    // Off in a new set: the brace is painted in the off colour, along the
    // ruler's lower edge.
    let output = h.settle();
    let want = brace(&mut h, ruler);
    assert_eq!(
        the_brace(&output, want),
        Some(off),
        "the brace at {want:?} is painted in the off colour"
    );
    // On: the same brace is painted in the on colour.
    h.shell
        .run("transport.set_loop_enabled", json!({ "enabled": true }))
        .unwrap();
    let output = h.settle();
    let want = brace(&mut h, ruler);
    assert_eq!(
        the_brace(&output, want),
        Some(on),
        "the brace at {want:?} is painted in the on colour"
    );
}

#[test]
fn every_part_reaches_assistive_technology() {
    // Spec: arrangement-view R3, R5; architecture, "UI": every control has
    // a role, a name, and a value.
    let mut h = with_scene(benchmark::scene(6, 12, 1));
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::Pane && n.label == "Arrangement View")
    );
    let headers: Vec<&str> = nodes
        .iter()
        .filter(|n| n.role == Role::RowHeader)
        .map(|n| n.label.as_str())
        .collect();
    // Six tracks and Main.
    assert_eq!(headers.len(), 7, "{headers:?}");
    assert!(headers.contains(&"Main"));
    // The browser's items are list items too (WP-1.10).
    let browser = Harness::node(&nodes, "Browser").id;
    let clips: Vec<_> = nodes
        .iter()
        .filter(|n| n.role == Role::ListItem && !Harness::descends_from(&nodes, n.id, browser))
        .collect();
    assert!(!clips.is_empty());
    assert!(
        clips
            .iter()
            .all(|c| !c.label.is_empty() && c.value.contains(" to "))
    );
    let ruler = Harness::node(&nodes, "Beat-Time Ruler");
    assert_eq!(ruler.value, "bars 1 to 33");
    let overview = Harness::node(&nodes, "Overview");
    assert_eq!(overview.value, "bars 1 to 33 in view, of bars 1 to 495");
    // 32 and a half bars at 120 BPM.
    let time = Harness::node(&nodes, "Time Ruler");
    assert_eq!(time.value, "0:00 to 1:05");
    let grid = Harness::node(&nodes, "Grid");
    assert_eq!(grid.value, "1/1, adaptive");
    let follow = Harness::node(&nodes, "Follow");
    assert!(!follow.disabled);
    assert_eq!(follow.value, "on");
    // The headers' mixer controls work since WP-1.9 (the mixer view spec,
    // R4), and the sends and In/Out choosers since WP-2.19.
    let solo = nodes.iter().find(|n| n.label == "Solo").unwrap();
    assert!(!solo.disabled);
    // Nothing waits for WP-2.19 any more (a MIDI track without an
    // instrument shows no sends, and the audio tracks' headers here are
    // too short for theirs).
    assert!(
        !nodes
            .iter()
            .any(|n| n.description.contains("WP-2.19") && !n.description.contains("WP-2.19b"))
    );
    let input = nodes
        .iter()
        .find(|n| n.role == Role::ComboBox && n.label == "Input Type")
        .unwrap();
    assert!(!input.disabled);
    // Spec: time-editing R4. The Set Locator button is live now.
    let locator = Harness::node(&nodes, "Set Locator");
    assert!(!locator.disabled);
    let previous = Harness::node(&nodes, "Previous Locator");
    assert!(!previous.disabled);
    let next = Harness::node(&nodes, "Next Locator");
    assert!(!next.disabled);
    for name in [
        "Optimize Height",
        "Optimize Width",
        "Overview",
        "Time Ruler",
        "Scrub Area",
    ] {
        let node = Harness::node(&nodes, name);
        assert!(!node.disabled, "{name}");
    }
}

#[test]
fn a_warped_clip_is_drawn_through_its_warp_map() {
    // Spec: sample-editor R4. A warped clip's waveform is drawn through the
    // same mapping as the Sample Editor's: each segment of the file runs at
    // the speed that segment plays, so both sides of a warp marker agree on
    // the frame under it.
    use bl_model::{AudioData, Clip, ClipData, Command};
    use bl_ui::arrangement::gpu::{Slot, WaveInst, lock};
    use bl_ui::arrangement::peaks::MemoryPeaks;

    let hash = benchmark::media_hash(0);
    let peaks = Arc::new(
        Peaks::from_samples(&benchmark::media_samples(
            &mut benchmark::Random::new(1),
            16.0,
        ))
        .unwrap(),
    );
    let mut source = MemoryPeaks::new();
    source.insert(hash, Arc::clone(&peaks));
    let mut editor = bl_shell::files::factory_default_set();
    let track = editor.document().top_level().nth(2).unwrap();
    let id: bl_model::ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                id,
                Ticks(2 * BAR),
                Ticks(8 * BAR),
                ClipData::Audio(AudioData::new(hash)),
            ),
        })
        .unwrap();
    editor.clear_history().unwrap();
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell = benchmark::shell_with(editor);
    h.main
        .arrangement_mut()
        .set_peak_source(Box::new(source), None);
    h.settle();
    // Sixteen seconds at 120 BPM is eight bars. A marker at the eighth
    // second, four bars in, moved four beats later, stretches the first half.
    let quarter = Ticks::QUARTER.get();
    h.shell
        .run("clip.set_warp", json!({ "clip": id, "warp": true }))
        .unwrap();
    h.shell
        .run(
            "clip.insert_warp_marker",
            json!({ "clips": [id], "time": 8.0 }),
        )
        .unwrap();
    h.shell
        .run(
            "clip.move_warp_markers",
            json!({ "clips": [id], "markers": [1], "by": 4 * quarter }),
        )
        .unwrap();
    h.settle();
    let lanes = layout(&h).lanes;
    let shared = h.main.arrangement().shared();
    let mut waves: Vec<WaveInst> = lock(&shared).slots[Slot::Lanes as usize].waves.clone();
    waves.sort_by(|p, q| p.draw[0].total_cmp(&q.draw[0]));
    // The first channel's band of each stretch.
    waves.dedup_by(|p, q| (p.draw[0] - q.draw[0]).abs() < 0.5);
    assert_eq!(waves.len(), 2, "one stretch each side of the marker");
    // 30 points to a bar: the marker is at 20 beats, five bars in, from the
    // clip's start at bar 2.
    let marker = lanes.left() + (2.0 + 5.0) * 30.0;
    assert!(
        (waves[1].draw[0] - marker).abs() < 1.0,
        "{:?}",
        waves[1].draw
    );
    assert!(
        (waves[0].draw[2] - marker).abs() < 1.0,
        "{:?}",
        waves[0].draw
    );
    // Both stretches reach the same frame, the eighth second, at the marker.
    let frame_at = |wave: &WaveInst, x: f32| {
        let level = (0..peaks.level_count())
            .find(|level| peaks.run(*level, 0).map(|run| run.len) == Some(wave.run[1]))
            .expect("the stretch's level");
        (f64::from(wave.run[2])
            + f64::from(wave.band[3])
            + f64::from(x - wave.draw[0]) * f64::from(wave.band[2]))
            * Peaks::frames_per_peak(level) as f64
    };
    let expected = 8.0 * f64::from(peaks.sample_rate());
    let left = frame_at(&waves[0], marker);
    let right = frame_at(&waves[1], marker);
    assert!((left - expected).abs() < 400.0, "{left} against {expected}");
    assert!(
        (right - expected).abs() < 400.0,
        "{right} against {expected}"
    );
}

// Spec: arrangement-view R18.
#[test]
fn the_return_tracks_rows_scroll_with_the_tracks_when_the_switch_is_on() {
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    h.shell
        .invoke(
            bl_shell::mixer_view_commands::RETURN_TRACKS_SCROLL,
            json!({ "on": true }),
        )
        .unwrap();
    h.settle();
    let nodes = Harness::nodes(&h.settle());
    let header = |name: &str| header_of(&nodes, name);
    // The returns' rows are in the lanes, after the drop area under the
    // last track, and Main stays pinned under them.
    let a = header("A Return").expect("the first return's row");
    let b = header("B Return").expect("the second return's row");
    let main = header("Main").expect("Main");
    assert!(a.bottom() <= b.top(), "the returns' rows follow each other");
    assert!(b.bottom() <= main.top(), "Main stays pinned under them");
    assert_eq!(a.height(), main.height(), "the height of a pinned row");
    let tracks: Vec<Rect> = nodes
        .iter()
        .filter(|n| n.role == Role::RowHeader)
        .filter(|n| !n.label.ends_with("Return") && n.label != "Main")
        .map(rect_of)
        .collect();
    let last = tracks.iter().fold(Rect::ZERO, |acc, r| acc.union(*r));
    // The drop area (44 points) under the last track; a row's header is a
    // point shorter than the row, which has a line under it.
    let gap = a.top() - last.bottom();
    assert!((44.0..=45.0).contains(&gap), "the drop area: {gap}");
    // With more tracks than fit, the lanes scroll as far as the last
    // return.
    for _ in 0..3 {
        h.shell
            .invoke("track.create", json!({ "kind": "audio" }))
            .unwrap();
    }
    h.settle();
    let nodes = Harness::nodes(&h.settle());
    assert!(
        !nodes
            .iter()
            .any(|n| n.role == Role::RowHeader && n.label == "A Return"),
        "the returns' rows are below the last track"
    );
    wheel(
        &mut h,
        pos2(600.0, 400.0),
        vec2(0.0, -400.0),
        Modifiers::NONE,
    );
    let nodes = Harness::nodes(&h.settle());
    let a = header_of(&nodes, "A Return").expect("the first return's row");
    let main = header_of(&nodes, "Main").expect("Main");
    assert!(a.bottom() <= main.top(), "Main stays pinned under them");
    assert_eq!(a.height(), main.height(), "the height of a pinned row");
    // With the return tracks hidden, their scrolling rows go too, and
    // Main stays (R23).
    h.shell
        .run(ids::SHOW_RETURN_TRACKS, json!({ "on": false }))
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    assert!(header_of(&nodes, "A Return").is_none(), "the returns hidden");
    assert!(header_of(&nodes, "Main").is_some(), "Main stays");
}

/// The row header named `name` in a frame's tree.
fn header_of(nodes: &[common::Node], name: &str) -> Option<Rect> {
    nodes
        .iter()
        .find(|n| n.role == Role::RowHeader && n.label == name)
        .map(rect_of)
}

/// The rectangle a node's bounds describe.
fn rect_of(node: &common::Node) -> Rect {
    let b = node.bounds.expect("a node with bounds");
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

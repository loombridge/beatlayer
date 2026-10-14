//! The Arrangement View's view commands, driven headless through
//! `Shell::run` and `Shell::invoke`, as agents, menus, and keys drive them
//! (WP-1.8a; D17). The behaviour is in the arrangement view spec.

use bl_commands::{Context, Permission};
use bl_core::time::Ticks;
use std::path::{Path, PathBuf};

use bl_model::{AudioData, Clip, ClipData, ClipId, Command, ContentHash, Editor, TrackId};
use bl_shell::arrangement::rows::{DEFAULT_TRACK_HEIGHT, FOLDED_HEIGHT};
use bl_shell::arrangement::{
    DEFAULT_TICKS_PER_POINT, MAX_TICKS_PER_POINT, MIN_TICKS_PER_POINT, TransportView,
};
use bl_shell::arrangement_commands::{self as ids, ArrangementState};
use bl_shell::audio::AudioHost;
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::settings::Preferences;
use bl_shell::{Shell, ShellError};
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();

fn state(shell: &mut Shell) -> ArrangementState {
    serde_json::from_value(shell.run(ids::STATE, json!({})).unwrap()).unwrap()
}

fn run(shell: &mut Shell, id: &str, args: Value) -> ArrangementState {
    serde_json::from_value(
        shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}")),
    )
    .unwrap()
}

fn tracks(shell: &Shell) -> Vec<TrackId> {
    shell.editor().document().top_level().collect()
}

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

/// A shell showing the factory default set with audio clips on its third
/// track, each `(start bar, bars)`.
fn shell_with_clips(clips: &[(i64, i64)]) -> (Shell, Vec<ClipId>) {
    let mut editor = factory_default_set();
    let track = editor.document().top_level().nth(2).unwrap();
    let mut ids = Vec::new();
    for (start, bars) in clips {
        let id: ClipId = editor.new_id();
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(
                    id,
                    Ticks(start * BAR),
                    Ticks(bars * BAR),
                    ClipData::Audio(AudioData::new(ContentHash([7; 32]))),
                ),
            })
            .unwrap();
        ids.push(id);
    }
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Clips.blset" }))
        .unwrap();
    (shell, ids)
}

fn playing(at: Ticks) -> TransportView {
    TransportView {
        playing: true,
        position: at,
        ..TransportView::default()
    }
}

#[test]
fn every_view_command_is_registered_read_only_and_reachable() {
    // Spec: arrangement-view R16.
    let mut shell = Shell::headless();
    for id in ids::IDS {
        let spec = shell.registry().get(id).unwrap_or_else(|| panic!("{id}"));
        assert_eq!(spec.permission, Some(Permission::Read), "{id}");
        assert!(spec.args.is_some() && spec.result.is_some(), "{id}");
    }
    assert!(shell.registry().problems().is_empty());
    let before = shell.editor().document().clone();
    let state = state(&mut shell);
    assert!(!shell.is_dirty(), "a view command never changes the set");
    assert_eq!(
        shell.editor().document().track_count(),
        before.track_count()
    );
    assert!(!shell.editor().can_undo(), "nor is it an undo step");
    assert!(state.follow && state.overview);
}

#[test]
fn a_new_set_opens_at_bar_one_with_live_s_defaults() {
    // Spec: arrangement-view R6, R10, R11.
    let mut shell = Shell::headless();
    let state = state(&mut shell);
    assert_eq!(state.visible.start, Ticks::ZERO);
    assert!((state.ticks_per_point - DEFAULT_TICKS_PER_POINT).abs() < 1e-6);
    assert!(state.follow && !state.follow_paused && state.overview);
    assert!(!state.grid.fixed && state.grid.snap && !state.grid.triplet);
    assert_eq!(state.grid.label, "1/1");
    assert_eq!(state.insert_marker, Ticks::ZERO);
    assert_eq!(state.visible_tracks.len(), 4);
    assert_eq!(state.zoom_back_steps, 0);
}

#[test]
fn zooming_keeps_the_anchor_still_within_limits() {
    // Spec: arrangement-view R6.
    let mut shell = Shell::headless();
    let anchor = Ticks(BAR * 8);
    let before = state(&mut shell);
    let x_before = (anchor.get() - before.visible.start.get()) as f64 / before.ticks_per_point;
    let after = run(
        &mut shell,
        ids::ZOOM,
        json!({ "factor": 2.0, "anchor": anchor }),
    );
    assert!((after.ticks_per_point - before.ticks_per_point / 2.0).abs() < 1e-6);
    let x_after = (anchor.get() - after.visible.start.get()) as f64 / after.ticks_per_point;
    assert!((x_before - x_after).abs() < 0.01, "{x_before} {x_after}");
    // Out as far as it goes, and in as far as it goes.
    for _ in 0..40 {
        run(&mut shell, ids::ZOOM, json!({ "factor": 0.5 }));
    }
    assert!((state(&mut shell).ticks_per_point - MAX_TICKS_PER_POINT).abs() < 1e-3);
    for _ in 0..60 {
        run(&mut shell, ids::ZOOM, json!({ "factor": 64.0 }));
    }
    assert!((state(&mut shell).ticks_per_point - MIN_TICKS_PER_POINT).abs() < 1e-6);
    assert!(matches!(
        shell.run(ids::ZOOM, json!({ "factor": 0.0 })),
        Err(ShellError::InvalidArgs { .. })
    ));
    // The zoom keys' anchor: the insert marker while it's in view.
    let mut shell = Shell::headless();
    run(&mut shell, ids::SET_INSERT_MARKER, json!({ "at": BAR * 4 }));
    let before = state(&mut shell);
    let after = run(&mut shell, ids::ZOOM, json!({ "factor": 2.0 }));
    let x = |s: &ArrangementState| (BAR * 4 - s.visible.start.get()) as f64 / s.ticks_per_point;
    assert!((x(&before) - x(&after)).abs() < 0.01);
}

#[test]
fn scrolling_stops_at_the_start_and_the_last_track() {
    // Spec: arrangement-view R2, R7.
    let mut shell = Shell::headless();
    let state0 = run(&mut shell, ids::SCROLL, json!({ "x": -500.0, "y": -500.0 }));
    assert_eq!(state0.visible.start, Ticks::ZERO);
    assert_eq!(state0.scroll_y, 0.0);
    let state1 = run(&mut shell, ids::SCROLL, json!({ "x": 300.0 }));
    assert_eq!(
        state1.visible.start.get(),
        (300.0 * DEFAULT_TICKS_PER_POINT).round() as i64
    );
    // Four 88-point tracks and the drop area fit in the lanes: nothing to
    // scroll down to.
    assert_eq!(
        run(&mut shell, ids::SCROLL, json!({ "y": 400.0 })).scroll_y,
        0.0
    );
    for _ in 0..20 {
        shell
            .invoke("track.create", json!({ "kind": "audio" }))
            .unwrap();
    }
    let rows_height = 24.0 * f64::from(DEFAULT_TRACK_HEIGHT) + 44.0;
    let viewport = f64::from(shell.arrangement().viewport().height);
    let scrolled = run(&mut shell, ids::SCROLL, json!({ "y": 100_000.0 }));
    assert!((scrolled.scroll_y - (rows_height - viewport)).abs() < 0.01);
    assert!(
        scrolled
            .visible_tracks
            .contains(tracks(&shell).last().unwrap())
    );
    assert!(!scrolled.visible_tracks.contains(&tracks(&shell)[0]));
    let first = tracks(&shell)[0];
    let shown = run(&mut shell, ids::SHOW_TRACK, json!({ "track": first }));
    assert_eq!(shown.scroll_y, 0.0);
}

#[test]
fn follow_turns_the_page_and_pauses_while_scrolling_by_hand() {
    // Spec: arrangement-view R10.
    let mut shell = Shell::headless();
    let width = f64::from(shell.arrangement().viewport().width);
    let end = (width * DEFAULT_TICKS_PER_POINT) as i64;
    // Inside the view: nothing moves.
    shell.set_transport(playing(Ticks(end / 2)));
    assert_eq!(state(&mut shell).visible.start, Ticks::ZERO);
    // Past the right edge: the page turns to the playhead.
    shell.set_transport(playing(Ticks(end + BAR)));
    assert_eq!(state(&mut shell).visible.start, Ticks(end + BAR));
    // Scrolling sideways by hand pauses it.
    let paused = run(&mut shell, ids::SCROLL, json!({ "x": -100.0 }));
    assert!(paused.follow && paused.follow_paused);
    let left = paused.visible.start;
    shell.set_transport(playing(Ticks(end * 3)));
    assert_eq!(
        state(&mut shell).visible.start,
        left,
        "paused: no page turn"
    );
    // Stopping resumes it.
    shell.set_transport(TransportView {
        playing: false,
        position: Ticks(end * 3),
        ..TransportView::default()
    });
    assert!(!state(&mut shell).follow_paused);
    // A click on the beat-time ruler pauses it while playing; a click in
    // the lanes resumes it.
    shell.set_transport(playing(Ticks(end * 3)));
    assert!(run(&mut shell, ids::FOLLOW, json!({ "paused": true })).follow_paused);
    assert!(!run(&mut shell, ids::SET_INSERT_MARKER, json!({ "at": 0 })).follow_paused);
    // Stopped, there's nothing to pause.
    shell.set_transport(TransportView::default());
    assert!(!run(&mut shell, ids::FOLLOW, json!({ "paused": true })).follow_paused);
    // With Follow off the view stays put.
    let off = run(&mut shell, ids::FOLLOW, json!({}));
    assert!(!off.follow);
    let left = off.visible.start;
    shell.set_transport(playing(Ticks(end * 9)));
    assert_eq!(state(&mut shell).visible.start, left);
    assert_eq!(shell.switch_state(ids::FOLLOW), Some(false));
}

#[test]
fn zoom_to_selection_and_back() {
    // Spec: arrangement-view R8.
    let mut shell = Shell::headless();
    let first = tracks(&shell)[0];
    assert!(!shell.can_invoke(ids::ZOOM_BACK, &json!({})));
    assert!(matches!(
        shell.run(ids::ZOOM_BACK, json!({})),
        Err(ShellError::Unavailable { .. })
    ));
    let selected = run(
        &mut shell,
        ids::SELECT_TIME,
        json!({ "start": BAR * 9, "end": BAR * 5, "tracks": [first] }),
    );
    assert_eq!(selected.time_selection.unwrap().start, Ticks(BAR * 5));
    let zoomed = run(&mut shell, ids::ZOOM_TO_SELECTION, json!({}));
    assert_eq!(zoomed.visible.start, Ticks(BAR * 5));
    assert!((zoomed.visible.end.get() - BAR * 9).abs() <= 1);
    assert_eq!(zoomed.zoom_back_steps, 1);
    // Straight away again: back, so a held Z zooms only while held.
    let back = run(&mut shell, ids::ZOOM_TO_SELECTION, json!({}));
    assert_eq!(back.visible.start, Ticks::ZERO);
    assert_eq!(back.zoom_back_steps, 0);
    // X steps back through the views.
    run(&mut shell, ids::ZOOM_TO_SELECTION, json!({}));
    run(
        &mut shell,
        ids::SHOW_RANGE,
        json!({ "start": BAR * 6, "end": BAR * 7 }),
    );
    assert_eq!(
        run(&mut shell, ids::ZOOM_BACK, json!({})).visible.start,
        Ticks(BAR * 5)
    );
    assert_eq!(
        run(&mut shell, ids::ZOOM_BACK, json!({})).visible.start,
        Ticks::ZERO
    );
    // Without a selection, the whole arrangement: at least 16 bars.
    run(&mut shell, ids::SET_INSERT_MARKER, json!({ "at": 0 }));
    let whole = run(&mut shell, ids::ZOOM_TO_SELECTION, json!({}));
    assert_eq!(whole.visible.start, Ticks::ZERO);
    assert!((whole.visible.end.get() - BAR * 16).abs() <= 1);
}

#[test]
fn optimize_height_fits_every_track_and_gives_heights_back() {
    // Spec: arrangement-view R9.
    let mut shell = Shell::headless();
    for _ in 0..40 {
        shell
            .invoke("track.create", json!({ "kind": "audio" }))
            .unwrap();
    }
    assert!(state(&mut shell).visible_tracks.len() < 44);
    let on = run(&mut shell, ids::OPTIMIZE_HEIGHT, json!({}));
    assert!(on.optimize_height);
    assert_eq!(on.visible_tracks.len(), 44);
    let rows = shell.arrangement().rows(shell.editor().document());
    let viewport = shell.arrangement().viewport().height;
    assert!((rows.last().unwrap().bottom() - viewport).abs() < 0.01);
    let off = run(&mut shell, ids::OPTIMIZE_HEIGHT, json!({}));
    assert!(!off.optimize_height);
    let rows = shell.arrangement().rows(shell.editor().document());
    assert!(rows.iter().all(|r| r.height == DEFAULT_TRACK_HEIGHT));
    // Changing a height by hand turns it off.
    run(&mut shell, ids::OPTIMIZE_HEIGHT, json!({ "on": true }));
    let first = tracks(&shell)[0];
    let resized = run(
        &mut shell,
        ids::SET_TRACK_HEIGHT,
        json!({ "tracks": [first], "height": 40.0 }),
    );
    assert!(!resized.optimize_height);
    assert_eq!(shell.arrangement().track_height(first), 40.0);
}

#[test]
fn optimize_width_fits_the_arrangement_until_the_view_moves() {
    // Spec: arrangement-view R9.
    let (mut shell, clips) = shell_with_clips(&[(60, 4), (10, 4)]);
    let before = state(&mut shell);
    let on = run(&mut shell, ids::OPTIMIZE_WIDTH, json!({}));
    assert!(on.optimize_width);
    assert_eq!(on.visible.start, Ticks::ZERO);
    assert!(
        (on.visible.end.get() - BAR * 64).abs() <= 1,
        "{:?}",
        on.visible
    );
    // It keeps fitting as the arrangement grows.
    shell
        .run(
            "clip.move",
            json!({ "clip": clips[1], "track": tracks(&shell)[2], "start": BAR * 100 }),
        )
        .unwrap();
    shell
        .run(
            "clip.set_length",
            json!({ "clip": clips[1], "length": BAR * 28 }),
        )
        .unwrap();
    assert!((state(&mut shell).visible.end.get() - BAR * 128).abs() <= 1);
    let off = run(&mut shell, ids::OPTIMIZE_WIDTH, json!({}));
    assert_eq!(off.visible, before.visible, "back to the view before");
    // Zooming turns it off where the view is.
    run(&mut shell, ids::OPTIMIZE_WIDTH, json!({}));
    let zoomed = run(&mut shell, ids::ZOOM, json!({ "factor": 1.0, "anchor": 0 }));
    assert!(!zoomed.optimize_width);
    assert!((zoomed.visible.end.get() - BAR * 128).abs() <= 1);
}

#[test]
fn the_grid_narrows_widens_and_switches() {
    // Spec: arrangement-view R11.
    let mut shell = Shell::headless();
    let narrow = run(&mut shell, ids::NARROW_GRID, json!({}));
    assert_eq!(narrow.grid.label, "1/2");
    assert!(!shell.can_invoke(ids::NARROW_GRID, &json!({})));
    assert!(matches!(
        shell.run(ids::NARROW_GRID, json!({})),
        Err(ShellError::Unavailable { .. })
    ));
    let wide = run(&mut shell, ids::WIDEN_GRID, json!({}));
    assert_eq!(wide.grid.label, "1/1");
    // Triplets can't go on while the grid is a bar apart.
    assert!(!shell.can_invoke(ids::TRIPLET_GRID, &json!({})));
    run(&mut shell, ids::ZOOM, json!({ "factor": 8.0 }));
    let triplet = run(&mut shell, ids::TRIPLET_GRID, json!({}));
    assert!(triplet.grid.triplet && triplet.grid.label.ends_with('T'));
    let snap = run(&mut shell, ids::SNAP_TO_GRID, json!({}));
    assert!(!snap.grid.snap);
    let fixed = run(&mut shell, ids::FIXED_GRID, json!({}));
    assert!(fixed.grid.fixed);
    assert_eq!(fixed.grid.label, "1/16T");
    let set = run(
        &mut shell,
        ids::SET_GRID,
        json!({ "fixed": "four_bars", "triplet": false, "snap": true }),
    );
    assert_eq!(set.grid.label, "4 Bars");
    assert_eq!(set.grid.step, Ticks(BAR * 4));
    let adaptive = run(&mut shell, ids::SET_GRID, json!({ "adaptive": "widest" }));
    assert!(!adaptive.grid.fixed);
    assert!(matches!(
        shell.run(
            ids::SET_GRID,
            json!({ "adaptive": "wide", "fixed": "half" })
        ),
        Err(ShellError::InvalidArgs { .. })
    ));
    assert_eq!(shell.switch_state(ids::SNAP_TO_GRID), Some(true));
}

#[test]
fn snapping_follows_the_grid() {
    // Spec: arrangement-view R12.
    let shell = Shell::headless();
    let doc = shell.editor().document();
    let view = shell.arrangement();
    // A grid of whole bars at the default zoom.
    assert_eq!(view.snap(doc, Ticks(BAR * 3 + BAR / 3)), Ticks(BAR * 3));
    assert_eq!(view.snap(doc, Ticks(BAR * 3 + BAR * 2 / 3)), Ticks(BAR * 4));
}

#[test]
fn folding_tracks_and_groups() {
    // Spec: arrangement-view R2, R14.
    let mut shell = Shell::headless();
    shell
        .invoke("track.create", json!({ "kind": "group" }))
        .unwrap();
    // A new track goes after the last one.
    let group = *tracks(&shell).last().unwrap();
    let first = tracks(&shell)[0];
    shell
        .run(
            "track.move",
            json!({ "track": first, "parent": group, "index": 0 }),
        )
        .unwrap();
    let rows = shell.arrangement().rows(shell.editor().document());
    let child = rows.iter().find(|r| r.track == first).unwrap();
    assert_eq!(child.depth, 1);
    // Folding a track leaves its title row.
    let second = tracks(&shell)[0];
    let folded = run(&mut shell, ids::FOLD_TRACKS, json!({ "tracks": [second] }));
    assert_eq!(folded.folded_tracks, [second]);
    let rows = shell.arrangement().rows(shell.editor().document());
    assert_eq!(
        rows.iter().find(|r| r.track == second).unwrap().height,
        FOLDED_HEIGHT
    );
    // A folded group hides its tracks; + and - on its title bar show and
    // hide them.
    assert!(!shell.can_invoke(ids::FOLD_GROUPS, &json!({ "fold": true })));
    run(
        &mut shell,
        ids::SELECT_TRACKS,
        json!({ "tracks": [group], "title_bars": true }),
    );
    assert!(shell.active_contexts().contains(&Context::TrackHeaders));
    assert!(shell.can_invoke(ids::FOLD_GROUPS, &json!({ "fold": true })));
    run(&mut shell, ids::FOLD_GROUPS, json!({ "fold": true }));
    let rows = shell.arrangement().rows(shell.editor().document());
    assert!(rows.iter().all(|r| r.track != first), "hidden in its group");
    run(&mut shell, ids::FOLD_GROUPS, json!({ "fold": false }));
    // U toggles the selected tracks; Option-U unfolds everything.
    run(&mut shell, ids::FOLD_TRACKS, json!({}));
    assert!(shell.arrangement().is_folded(group));
    let all = run(&mut shell, ids::UNFOLD_ALL, json!({}));
    assert!(all.folded_tracks.is_empty());
}

#[test]
fn track_heights_stay_in_range() {
    // Spec: arrangement-view R14.
    let mut shell = Shell::headless();
    let first = tracks(&shell)[0];
    run(&mut shell, ids::SELECT_TRACKS, json!({ "tracks": [first] }));
    run(&mut shell, ids::SET_TRACK_HEIGHT, json!({ "delta": 16.0 }));
    assert_eq!(
        shell.arrangement().track_height(first),
        DEFAULT_TRACK_HEIGHT + 16.0
    );
    run(&mut shell, ids::SET_TRACK_HEIGHT, json!({ "height": 1.0 }));
    assert_eq!(shell.arrangement().track_height(first), 18.0);
    run(
        &mut shell,
        ids::SET_TRACK_HEIGHT,
        json!({ "height": 10_000.0 }),
    );
    assert_eq!(shell.arrangement().track_height(first), 800.0);
    assert!(matches!(
        shell.run(
            ids::SET_TRACK_HEIGHT,
            json!({ "height": 50.0, "delta": 2.0 })
        ),
        Err(ShellError::InvalidArgs { .. })
    ));
}

#[test]
fn the_insert_marker_and_selection_are_view_state() {
    // Spec: arrangement-view R12, R15.
    let mut shell = Shell::headless();
    let [a, b, ..] = tracks(&shell)[..] else {
        panic!("four tracks")
    };
    let placed = run(
        &mut shell,
        ids::SET_INSERT_MARKER,
        json!({ "at": BAR * 2, "track": b }),
    );
    assert_eq!(placed.insert_marker, Ticks(BAR * 2));
    assert_eq!(placed.selected_tracks, [b]);
    assert!(!shell.is_dirty() && !shell.editor().can_undo());
    let both = run(
        &mut shell,
        ids::SELECT_TRACKS,
        json!({ "tracks": [a], "add": true }),
    );
    assert_eq!(both.selected_tracks, [b, a]);
    // Esc goes to the highlighted track's title bar.
    assert!(!shell.active_contexts().contains(&Context::TrackHeaders));
    let title = run(&mut shell, ids::FOCUS_TITLE_BAR, json!({}));
    assert_eq!(title.selected_tracks, [b]);
    assert_eq!(
        shell.active_contexts(),
        [Context::TrackHeaders, Context::Arrangement, Context::Global]
    );
    // A deleted track leaves the selection; a new set starts afresh.
    shell.run("track.delete", json!({ "track": b })).unwrap();
    assert!(state(&mut shell).selected_tracks.is_empty());
    run(&mut shell, ids::SCROLL, json!({ "x": 200.0 }));
    shell.run("set.new", json!({})).unwrap();
    assert_eq!(state(&mut shell).visible.start, Ticks::ZERO);
}

#[test]
fn the_overview_shows_and_hides() {
    let mut shell = Shell::headless();
    assert_eq!(shell.switch_state(ids::OVERVIEW), Some(true));
    assert!(!run(&mut shell, ids::OVERVIEW, json!({})).overview);
    assert!(run(&mut shell, ids::OVERVIEW, json!({ "on": true })).overview);
}

#[test]
fn the_viewport_follows_the_window() {
    let mut shell = Shell::headless();
    let before = state(&mut shell);
    shell.set_arrangement_viewport(500.0, 300.0);
    let after = state(&mut shell);
    assert_eq!(after.visible.start, before.visible.start);
    assert!(after.visible.end < before.visible.end);
    assert_eq!(
        after.visible_tracks.len(),
        4,
        "300 points show part of the fourth"
    );
}

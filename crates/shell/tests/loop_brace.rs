//! The loop brace in the Arrangement, driven headless through `Shell::run`
//! as its click, drags and keys drive it (WP-6.73). Selecting it makes its
//! span the time selection, and none of it is an undo step. The behaviour is
//! in the transport spec, R31 to R34.

use std::path::{Path, PathBuf};

use bl_commands::Context;
use bl_core::time::Ticks;
use bl_model::{Document, Editor};
use bl_shell::arrangement_commands::{self as view_ids, ArrangementState};
use bl_shell::audio::AudioHost;
use bl_shell::editing_commands as ids;
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::settings::Preferences;
use bl_shell::{Shell, ShellError};
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();
const BEAT: i64 = BAR / 4;

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

/// The factory default set, its brace as `(start, length)`, in the Arrangement.
fn open(start: i64, length: i64) -> Shell {
    let mut editor = factory_default_set();
    editor
        .apply(bl_model::Command::SetLoop {
            start: Ticks(start),
            length: Ticks(length),
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
        .run("set.open", json!({ "path": "Brace.blset" }))
        .unwrap();
    shell
}

fn state(shell: &mut Shell, id: &str, args: Value) -> ArrangementState {
    serde_json::from_value(
        shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}")),
    )
    .unwrap()
}

fn doc(shell: &Shell) -> &Document {
    shell.editor().document()
}

/// The brace as `transport.state` reads it back: start and length.
fn brace(shell: &mut Shell) -> (i64, i64) {
    let read = shell.run("transport.state", json!({})).unwrap();
    (
        read["loop_start"].as_i64().unwrap(),
        read["loop_length"].as_i64().unwrap(),
    )
}

fn grid(shell: &mut Shell, fixed: &str) {
    state(
        shell,
        view_ids::SET_GRID,
        json!({ "fixed": fixed, "snap": true }),
    );
}

fn press(shell: &mut Shell, by: &str) -> ArrangementState {
    state(shell, ids::NUDGE_LOOP_BRACE, json!({ "by": by }))
}

fn span(state: &ArrangementState) -> Option<(i64, i64)> {
    state
        .time_selection
        .as_ref()
        .map(|t| (t.start.get(), t.end.get()))
}

#[test]
fn selecting_the_brace_selects_its_span_on_every_track_and_leaves_the_loop_switch() {
    // Spec: transport R32 (S-LOOP, I-CLICK-017).
    let mut shell = open(2 * BAR, 4 * BAR);
    let steps = shell.editor().undo_history().len();
    let selected = state(&mut shell, ids::SELECT_LOOP_BRACE, json!({}));
    assert!(selected.loop_brace_selected);
    assert_eq!(span(&selected), Some((2 * BAR, 6 * BAR)));
    assert_eq!(selected.insert_marker.get(), 2 * BAR);
    let every: Vec<_> = doc(&shell).track_order().into_iter().collect();
    assert_eq!(selected.selected_tracks, every);
    assert!(
        every.len() > doc(&shell).top_level().count(),
        "returns and Main too"
    );
    assert!(!doc(&shell).settings().arrangement_loop.enabled);
    assert_eq!(shell.editor().undo_history().len(), steps);
    assert_eq!(shell.active_contexts()[0], Context::LoopBrace);
    // Selecting anything else deselects it.
    let after = state(&mut shell, view_ids::SET_INSERT_MARKER, json!({ "at": 0 }));
    assert!(!after.loop_brace_selected);
    assert!(!shell.active_contexts().contains(&Context::LoopBrace));
    assert!(matches!(
        shell.run(ids::NUDGE_LOOP_BRACE, json!({ "by": "right" })),
        Err(ShellError::Unavailable { .. })
    ));
}

#[test]
fn the_keys_move_and_resize_the_brace_and_place_the_insert_marker() {
    // Spec: transport R33 (S-LOOP-007 to S-LOOP-010, S-ARR-012).
    let mut shell = open(2 * BAR, 4 * BAR);
    grid(&mut shell, "quarter");
    let steps = shell.editor().undo_history().len();
    state(&mut shell, ids::SELECT_LOOP_BRACE, json!({}));
    // (key, start, length, insert marker)
    let sequence = [
        ("right", 2 * BAR + BEAT, 4 * BAR, 2 * BAR + BEAT),
        ("left", 2 * BAR, 4 * BAR, 2 * BAR),
        ("earlier", 0, 4 * BAR, 0),
        ("later", 4 * BAR, 4 * BAR, 4 * BAR),
        ("halve", 4 * BAR, 2 * BAR, 6 * BAR),
        ("double", 4 * BAR, 4 * BAR, 8 * BAR),
        ("lengthen", 4 * BAR, 4 * BAR + BEAT, 8 * BAR + BEAT),
        ("shorten", 4 * BAR, 4 * BAR, 8 * BAR),
    ];
    for (key, start, length, marker) in sequence {
        let moved = press(&mut shell, key);
        assert_eq!(brace(&mut shell), (start, length), "{key}");
        assert_eq!(moved.insert_marker.get(), marker, "{key}");
        assert_eq!(span(&moved), Some((start, start + length)), "{key}");
        assert!(moved.loop_brace_selected, "{key}");
    }
    assert_eq!(shell.editor().undo_history().len(), steps, "no undo step");
    assert!(!doc(&shell).settings().arrangement_loop.enabled);
}

#[test]
fn down_stops_at_the_songs_start() {
    // Spec: transport R33.
    let mut shell = open(2 * BAR, 4 * BAR);
    state(&mut shell, ids::SELECT_LOOP_BRACE, json!({}));
    press(&mut shell, "earlier");
    assert_eq!(brace(&mut shell), (0, 4 * BAR));
    press(&mut shell, "left");
    assert_eq!(brace(&mut shell), (0, 4 * BAR));
}

#[test]
fn left_and_right_go_to_the_next_grid_line_whatever_the_grid() {
    // Spec: transport R33.
    let mut shell = open(4 * BAR + 3 * BEAT, 4 * BAR);
    state(&mut shell, ids::SELECT_LOOP_BRACE, json!({}));
    grid(&mut shell, "eighth");
    press(&mut shell, "right");
    assert_eq!(brace(&mut shell).0, 4 * BAR + 3 * BEAT + BEAT / 2);
    grid(&mut shell, "half");
    press(&mut shell, "right");
    assert_eq!(
        brace(&mut shell).0,
        5 * BAR,
        "to the next line, not half a bar on"
    );
    press(&mut shell, "left");
    assert_eq!(brace(&mut shell).0, 4 * BAR + 2 * BEAT);
    grid(&mut shell, "one_bar");
    press(&mut shell, "left");
    assert_eq!(brace(&mut shell).0, 4 * BAR);
}

#[test]
fn a_drag_sets_the_brace_and_puts_the_insert_marker_where_asked_in_no_undo_step() {
    // Spec: transport R34 (I-DRAG-019).
    let mut shell = open(0, 4 * BAR);
    let steps = shell.editor().undo_history().len();
    let end = state(
        &mut shell,
        ids::SET_LOOP_BRACE,
        json!({ "start": 0, "length": 8 * BAR, "insert_marker": "end" }),
    );
    assert_eq!(brace(&mut shell), (0, 8 * BAR));
    assert_eq!(end.insert_marker.get(), 8 * BAR);
    assert_eq!(span(&end), Some((0, 8 * BAR)));
    assert!(end.loop_brace_selected);
    let bar = state(
        &mut shell,
        ids::SET_LOOP_BRACE,
        json!({ "start": 3 * BEAT, "length": 8 * BAR }),
    );
    assert_eq!(brace(&mut shell), (3 * BEAT, 8 * BAR));
    assert_eq!(bar.insert_marker.get(), 3 * BEAT);
    assert_eq!(shell.editor().undo_history().len(), steps);
    for bad in [
        json!({ "start": -1, "length": BAR }),
        json!({ "start": 0, "length": 0 }),
    ] {
        assert!(shell.run(ids::SET_LOOP_BRACE, bad).is_err());
    }
    assert_eq!(brace(&mut shell), (3 * BEAT, 8 * BAR));
}

#[test]
fn undoing_an_earlier_step_leaves_the_brace_where_it_was_moved() {
    // Spec: transport R31.
    let mut shell = open(0, 4 * BAR);
    shell
        .run("transport.set_loop_enabled", json!({ "enabled": true }))
        .unwrap();
    state(
        &mut shell,
        ids::SET_LOOP_BRACE,
        json!({ "start": BAR, "length": 2 * BAR }),
    );
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(brace(&mut shell), (BAR, 2 * BAR));
    assert!(!doc(&shell).settings().arrangement_loop.enabled);
}

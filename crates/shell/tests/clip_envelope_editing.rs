//! The Clip View's Envelope Editor's editing, driven headless through
//! `Shell::run` (WP-6.25c; D17): the time selection, moving several
//! breakpoints, the edit commands, the shapes, stretch and skew, and
//! Simplify Envelope on a Session clip's and an Arrangement clip's envelopes.
//! The behaviour is in the clip envelopes spec (R12, R13).

use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_model::{
    AutomationTarget, Breakpoint, Clip, ClipData, ClipId, Command, Editor, MidiData, MixerParam,
    SceneId, TrackId,
};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::clip_envelope_commands::{self as ids, ClipEnvelopeResult, EnvelopeClipboardState};
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::note_commands::ClipViewState;
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

struct Song {
    shell: Shell,
    /// The Arrangement clip on the first MIDI track.
    arranged: ClipId,
    /// The Session clip in the first scene of the same track.
    session: ClipId,
    track: TrackId,
}

fn volume() -> AutomationTarget {
    AutomationTarget::Mixer {
        param: MixerParam::Volume,
    }
}

fn pan() -> AutomationTarget {
    AutomationTarget::Mixer {
        param: MixerParam::Pan,
    }
}

/// A set with a four-bar MIDI clip in the Arrangement and a Session clip on
/// the first MIDI track, and the Clip View open.
fn song() -> Song {
    let mut editor = factory_default_set();
    let track: TrackId = editor.document().top_level().next().unwrap();
    let arranged: ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                arranged,
                Ticks::ZERO,
                Ticks(4 * BAR),
                ClipData::Midi(MidiData::new(Vec::new())),
            ),
        })
        .unwrap();
    editor.clear_history().unwrap();
    let scene: SceneId = editor.document().scenes().next().unwrap().id;
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Envelopes.blset" }))
        .unwrap();
    shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    let session = shell.editor().document().slot(track, scene).unwrap().id;
    shell.run("view.toggle_clip_view", json!({})).unwrap();
    shell
        .run("view.clip_tab", json!({ "tab": "envelopes" }))
        .unwrap();
    Song {
        shell,
        arranged,
        session,
        track,
    }
}

impl Song {
    fn steps(&self) -> usize {
        self.shell.editor().undo_history().len()
    }

    fn run(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .run(id, args)
            .unwrap_or_else(|error| panic!("{id} failed: {error}"))
    }

    /// Runs `id`, which must make exactly one undo step.
    fn step(&mut self, id: &str, args: Value) -> ClipEnvelopeResult {
        let before = self.steps();
        let result = self.run(id, args);
        assert_eq!(self.steps(), before + 1, "{id} is one undo step");
        serde_json::from_value(result).unwrap()
    }

    /// Shows `clip`'s envelope for `target`.
    fn show(&mut self, clip: ClipId, target: AutomationTarget) {
        self.shell
            .run("arrangement.select_clips", json!({ "clips": [clip] }))
            .ok();
        if clip == self.session {
            let scene: SceneId = self.shell.editor().document().scenes().next().unwrap().id;
            self.run(
                "session.select",
                json!({ "track": self.track, "scene": scene }),
            );
        }
        self.run("view.clip_envelope", json!({ "target": target }));
    }

    fn state(&mut self) -> ClipViewState {
        serde_json::from_value(self.run("view.clip_tab", json!({ "tab": "envelopes" }))).unwrap()
    }

    fn points(&self, clip: ClipId, target: AutomationTarget) -> Vec<Breakpoint> {
        self.shell
            .editor()
            .document()
            .clip(clip)
            .unwrap()
            .envelope(&target)
            .map(|envelope| envelope.envelope.points().collect())
            .unwrap_or_default()
    }

    fn draw(&mut self, clip: ClipId, target: AutomationTarget, points: &[(i64, f64)]) {
        for (at, value) in points {
            self.run(
                "clip.set_envelope_breakpoint",
                json!({ "clip": clip, "target": target, "at": at, "value": value }),
            );
        }
    }
}

fn values(points: &[Breakpoint]) -> Vec<f64> {
    points.iter().map(|point| point.value).collect()
}

// Spec: clip-envelopes R12. Selecting time in the editor is view state: the
// shown clip's state reports it, and it is no undo step.
#[test]
fn a_time_selection_is_view_state_that_the_clip_views_state_reports() {
    let mut song = song();
    let clip = song.arranged;
    song.show(clip, pan());
    let steps = song.steps();
    let selected: ClipEnvelopeResult =
        serde_json::from_value(song.run(ids::SELECT_TIME, json!({ "start": BAR, "end": 2 * BAR })))
            .unwrap();
    assert!(!selected.undo_step);
    let range = selected.time_selection.unwrap();
    assert_eq!((range.start, range.end), (Ticks(BAR), Ticks(2 * BAR)));
    let state = song.state();
    let range = state.envelope_selection.unwrap();
    assert_eq!((range.start, range.end), (Ticks(BAR), Ticks(2 * BAR)));
    // Bars, beats and sixteenths read as they do for the lanes.
    song.run(
        ids::SELECT_TIME,
        json!({ "start": "1.1.1", "end": "2.1.1" }),
    );
    let range = song.state().envelope_selection.unwrap();
    assert_eq!((range.start, range.end), (Ticks::ZERO, Ticks(BAR)));
    // A span that ends before it starts is refused, and none lets go.
    assert!(
        song.shell
            .run(ids::SELECT_TIME, json!({ "start": 2 * BAR, "end": BAR }))
            .is_err()
    );
    song.run(ids::SELECT_TIME, json!({}));
    assert!(song.state().envelope_selection.is_none());
    assert_eq!(song.steps(), steps, "selecting is not an undo step");
}

// Spec: clip-envelopes R12, R13. Every command runs by its ID on the
// envelopes of a Session clip and an Arrangement clip, and each one that
// edits is one undo step.
#[test]
fn every_command_runs_by_id_on_both_kinds_of_clip_as_one_undo_step() {
    let mut song = song();
    for clip in [song.arranged, song.session] {
        song.show(clip, pan());
        song.draw(
            clip,
            pan(),
            &[(0, -1.0), (BAR / 4, 1.0), (BAR / 2, -0.5), (BAR, 0.5)],
        );
        let span = |extra: Value| {
            let mut args = json!({ "clip": clip, "target": pan(), "start": 0, "end": BAR / 2 });
            if let (Some(args), Some(extra)) = (args.as_object_mut(), extra.as_object()) {
                args.extend(extra.clone());
            }
            args
        };
        song.step(
            ids::MOVE_BREAKPOINTS,
            json!({ "clip": clip, "target": pan(), "at": [BAR / 4], "by": BAR / 8, "share": -0.25 }),
        );
        // Copying changes nothing in the set.
        let steps = song.steps();
        let copied: EnvelopeClipboardState =
            serde_json::from_value(song.run(ids::COPY, span(json!({})))).unwrap();
        assert_eq!((copied.lanes, copied.length), (1, Ticks(BAR / 2)));
        assert_eq!(song.steps(), steps);
        song.step(
            ids::PASTE,
            json!({ "clip": clip, "target": pan(), "at": BAR }),
        );
        song.step(ids::DUPLICATE, span(json!({})));
        song.step(ids::INSERT_SHAPE, span(json!({ "shape": "sine" })));
        song.step(
            ids::STRETCH_VALUES,
            span(json!({ "bottom": 0.25, "top": 0.75 })),
        );
        song.step(
            ids::STRETCH_TIME,
            span(json!({ "to_start": 0, "to_end": BAR / 4 })),
        );
        song.step(
            ids::SKEW,
            span(json!({ "edge": "top", "at_start": 1.0, "at_end": 0.5 })),
        );
        song.step(ids::SIMPLIFY, span(json!({})));
        song.step(ids::CUT, span(json!({})));
        song.step(
            ids::DELETE_RANGE,
            span(json!({ "start": BAR, "end": 2 * BAR })),
        );
        // Undo takes the last one back, whichever clip it was on.
        let before = song.points(clip, pan());
        song.run("edit.undo", json!({}));
        assert_ne!(song.points(clip, pan()), before);
    }
}

// Spec: clip-envelopes R13. A shape on a Volume envelope spans 0 to 100% of
// the modulation scale, and on a Pan envelope -100% to 100%.
#[test]
fn a_shape_on_a_volume_envelope_spans_0_to_100_and_on_pan_minus_100_to_100() {
    let mut song = song();
    let clip = song.arranged;
    for (target, low, high) in [(volume(), 0.0, 1.0), (pan(), -1.0, 1.0)] {
        song.show(clip, target);
        song.step(
            ids::INSERT_SHAPE,
            json!({ "shape": "sawtooth", "clip": clip, "target": target,
                    "start": 0, "end": BAR }),
        );
        let points = song.points(clip, target);
        let values = values(&points);
        let (min, max) = (
            values.iter().copied().fold(f64::MAX, f64::min),
            values.iter().copied().fold(f64::MIN, f64::max),
        );
        assert!((min - low).abs() < 1e-9, "{target:?}: {min}");
        assert!((max - high).abs() < 0.01, "{target:?}: {max}");
        // The envelope was made, linked, by the shape.
        assert!(
            song.shell
                .editor()
                .document()
                .clip(clip)
                .unwrap()
                .envelope(&target)
                .unwrap()
                .linked
        );
    }
}

// Spec: clip-envelopes R12. The shape copied from one envelope pastes into
// another parameter's, each value at the same place on its travel, and
// automation lanes share the clipboard.
#[test]
fn a_copied_shape_pastes_onto_another_parameter_at_the_same_place_on_its_travel() {
    let mut song = song();
    let clip = song.arranged;
    song.show(clip, volume());
    song.draw(clip, volume(), &[(0, 0.0), (BAR / 2, 1.0), (BAR, 0.5)]);
    song.run(
        ids::COPY,
        json!({ "clip": clip, "target": volume(), "start": 0, "end": BAR }),
    );
    song.show(clip, pan());
    song.run(
        ids::SELECT_TIME,
        json!({ "start": 2 * BAR, "end": 3 * BAR }),
    );
    // With the selection, the shape lands from its start.
    song.step(ids::PASTE, json!({}));
    let pasted = song.points(clip, pan());
    assert_eq!(pasted.first().map(|point| point.at), Some(Ticks(2 * BAR)));
    assert!(
        (pasted[0].value + 1.0).abs() < 1e-9,
        "the bottom of Volume is the bottom of Pan"
    );
    // Halfway up the fader's share is halfway up the pan's.
    assert!(pasted.last().unwrap().value.abs() < 1e-9, "{pasted:?}");
    // Cut puts the shape on the clipboard, and leaves the envelope straight
    // across the span with its edges.
    song.step(ids::CUT, json!({}));
    let after = song.points(clip, pan());
    assert!(
        after
            .iter()
            .all(|point| point.at <= Ticks(2 * BAR) || point.at >= Ticks(3 * BAR))
    );
    // Duplicate writes the copy after the span and selects it.
    song.step(ids::DUPLICATE, json!({}));
    let range = song.state().envelope_selection.unwrap();
    assert_eq!((range.start, range.end), (Ticks(3 * BAR), Ticks(4 * BAR)));
}

// Spec: clip-envelopes R12. Selected breakpoints move together by the same
// time and share of the travel, and the ones they pass over go.
#[test]
fn moving_several_breakpoints_moves_them_together_and_removes_the_ones_passed() {
    let mut song = song();
    let clip = song.arranged;
    song.show(clip, pan());
    song.draw(
        clip,
        pan(),
        &[(0, 0.0), (BAR / 4, 0.5), (BAR / 2, -0.5), (BAR, 0.0)],
    );
    song.step(
        ids::MOVE_BREAKPOINTS,
        json!({ "clip": clip, "target": pan(), "at": [BAR / 4, BAR / 2], "by": BAR / 8, "share": 0.1 }),
    );
    let points = song.points(clip, pan());
    let at: Vec<i64> = points.iter().map(|point| point.at.get()).collect();
    assert_eq!(at, [0, BAR / 4 + BAR / 8, BAR / 2 + BAR / 8, BAR]);
    // A tenth of the travel is 0.2 of the pan's -1 to 1.
    assert!((points[1].value - 0.7).abs() < 1e-6, "{points:?}");
    assert!((points[2].value + 0.3).abs() < 1e-6, "{points:?}");
    // Moving onto the last one removes it.
    song.step(
        ids::MOVE_BREAKPOINTS,
        json!({ "clip": clip, "target": pan(), "at": [BAR / 2 + BAR / 8], "by": BAR / 2, "share": 0.0 }),
    );
    let at: Vec<i64> = song
        .points(clip, pan())
        .iter()
        .map(|point| point.at.get())
        .collect();
    assert_eq!(at, [0, BAR / 4 + BAR / 8, BAR / 2 + BAR / 8 + BAR / 2]);
    // A position with no breakpoint, and an envelope there is not, are
    // refused.
    assert!(
        song.shell
            .run(
                ids::MOVE_BREAKPOINTS,
                json!({ "clip": clip, "target": volume(), "at": [0], "by": 1, "share": 0.0 })
            )
            .is_err()
    );
}

// Spec: clip-envelopes R13. Stretching in time moves the time selection
// with it; stretching the values and skewing keep the breakpoints in the
// modulation scale.
#[test]
fn stretching_in_time_moves_the_selection_and_values_stay_on_the_modulation_scale() {
    let mut song = song();
    let clip = song.arranged;
    song.show(clip, pan());
    song.draw(clip, pan(), &[(0, -1.0), (BAR / 2, 1.0), (BAR, -1.0)]);
    song.run(ids::SELECT_TIME, json!({ "start": 0, "end": BAR }));
    song.step(
        ids::STRETCH_TIME,
        json!({ "to_start": 0, "to_end": 2 * BAR }),
    );
    let range = song.state().envelope_selection.unwrap();
    assert_eq!((range.start, range.end), (Ticks::ZERO, Ticks(2 * BAR)));
    let at: Vec<i64> = song
        .points(clip, pan())
        .iter()
        .map(|point| point.at.get())
        .collect();
    assert_eq!(at, [0, BAR, 2 * BAR]);
    // The top handle brought halfway down: 1 is now 0 on the pan scale.
    song.step(ids::STRETCH_VALUES, json!({ "bottom": 0.0, "top": 0.5 }));
    let stretched = values(&song.points(clip, pan()));
    assert!((stretched[1] - 0.0).abs() < 1e-9, "{stretched:?}");
    assert!((stretched[0] + 1.0).abs() < 1e-9);
    // The edge cannot pass the opposite one.
    assert!(
        song.shell
            .run(
                ids::SKEW,
                json!({ "edge": "top", "at_start": -0.5, "at_end": 1.0 })
            )
            .is_err()
    );
    song.step(
        ids::SKEW,
        json!({ "edge": "top", "at_start": 1.0, "at_end": 0.5 }),
    );
}

// Spec: clip-envelopes R13. Simplify Envelope keeps the edges and adds
// nothing.
#[test]
fn simplifying_keeps_the_edges_and_adds_nothing() {
    let mut song = song();
    let clip = song.arranged;
    song.show(clip, pan());
    let ramp: Vec<(i64, f64)> = (0..=40)
        .map(|i| (i * BAR / 40, f64::from(i as i32) / 20.0 - 1.0))
        .collect();
    song.draw(clip, pan(), &ramp);
    song.run(ids::SELECT_TIME, json!({ "start": 0, "end": BAR }));
    let before = song.points(clip, pan());
    song.step(ids::SIMPLIFY, json!({}));
    let after = song.points(clip, pan());
    assert!(after.len() < before.len(), "{} points", after.len());
    assert_eq!(after.first().map(|p| p.at), before.first().map(|p| p.at));
    assert!(
        after
            .iter()
            .all(|point| before.iter().any(|b| b.at == point.at))
    );
}

// Spec: clip-envelopes R5, R12. A Session clip's Automation envelope takes
// the edits on the parameter's own scale, and an Arrangement clip has none.
#[test]
fn a_session_clips_automation_envelope_is_edited_on_the_parameters_own_scale() {
    let mut song = song();
    let session = song.session;
    song.show(session, volume());
    song.step(
        ids::INSERT_SHAPE,
        json!({ "shape": "sawtooth", "clip": session, "target": volume(),
                "kind": "automation", "start": 0, "end": BAR }),
    );
    let automation = song
        .shell
        .editor()
        .document()
        .clip(session)
        .unwrap()
        .envelope_of(&volume(), bl_model::EnvelopeKind::Automation)
        .unwrap()
        .envelope
        .points()
        .collect::<Vec<_>>();
    // The fader's own gain runs from silence to above unity, not 0 to 1.
    assert!(automation.last().unwrap().value > 1.0, "{automation:?}");
    assert!(song.points(session, volume()).is_empty());
    let arranged = song.arranged;
    assert!(
        song.shell
            .run(
                ids::INSERT_SHAPE,
                json!({ "shape": "sine", "clip": arranged, "target": volume(),
                        "kind": "automation", "start": 0, "end": BAR })
            )
            .is_err()
    );
}

// Spec: clip-envelopes R13 (automation-editing R18). Edit ▸ Simplify
// Envelope runs on the editor's time selection while it has the keyboard.
#[test]
fn edit_simplify_envelope_acts_on_the_envelope_editors_selection() {
    let mut song = song();
    let clip = song.arranged;
    song.show(clip, pan());
    let ramp: Vec<(i64, f64)> = (0..=40)
        .map(|i| (i * BAR / 40, f64::from(i as i32) / 20.0 - 1.0))
        .collect();
    song.draw(clip, pan(), &ramp);
    song.shell.set_focused_area(bl_shell::Area::Detail);
    let simplify = bl_shell::automation_commands::SIMPLIFY;
    assert!(
        !song.shell.can_invoke(simplify, &json!({})),
        "disabled without a time selection"
    );
    song.run(ids::SELECT_TIME, json!({ "start": 0, "end": BAR }));
    assert!(song.shell.can_invoke(simplify, &json!({})));
    let before = song.points(clip, pan()).len();
    song.run(simplify, json!({}));
    assert!(song.points(clip, pan()).len() < before);
    // The edit keys reach the editor's commands in its context, and only
    // there.
    for id in [ids::COPY, ids::CUT, ids::DELETE_RANGE, ids::DUPLICATE] {
        assert!(song.shell.can_invoke(id, &json!({})), "{id}");
    }
    song.shell.set_focused_area(bl_shell::Area::MainView);
    for id in [ids::COPY, ids::CUT, ids::DELETE_RANGE, ids::DUPLICATE] {
        assert!(!song.shell.can_invoke(id, &json!({})), "{id}");
    }
}

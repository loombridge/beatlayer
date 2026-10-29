//! The time-signature lane's commands and the fragmentary-bar commands,
//! driven headless through `Shell::run` and `Shell::invoke`, as agents,
//! keys and menus drive them (WP-6.61; the time editing spec, R29 to R36).

use std::path::{Path, PathBuf};

use bl_commands::Permission;
use bl_core::time::{Ticks, TimeSignature};
use bl_model::{
    AudioData, AutomationTarget, Breakpoint, Clip, ClipData, ClipId, Command, ContentHash,
    Document, Editor, Locator, LocatorId, MixerParam, TrackId,
};
use bl_shell::arrangement_commands::ArrangementState;
use bl_shell::audio::AudioHost;
use bl_shell::editing_commands as edit_ids;
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::locator_commands as ids;
use bl_shell::settings::Preferences;
use bl_shell::transport::{self, Field, Group};
use bl_shell::{Effect, Shell};
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();

fn bars(n: i64) -> Ticks {
    Ticks(n * BAR)
}

fn quarters(n: i64) -> Ticks {
    Ticks::from_quarters(n)
}

fn sig(numerator: u8, denominator: u8) -> TimeSignature {
    TimeSignature::new(numerator, denominator).unwrap()
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

const VOLUME: AutomationTarget = AutomationTarget::Mixer {
    param: MixerParam::Volume,
};

/// The factory default set with, on its first audio track: a clip across
/// bars 1 to 3 (0 to 3 bars) and one from bar 8 to 10, a volume ramp from 0
/// at bar 1 to 1 at bar 9, and a locator at bar 8.
struct Set {
    shell: Shell,
    track: TrackId,
    locator: LocatorId,
}

fn set() -> Set {
    set_with(&[])
}

/// [`set`] with the time-signature markers `markers` already in it.
fn set_with(markers: &[(Ticks, TimeSignature)]) -> Set {
    let mut editor = factory_default_set();
    for &(at, signature) in markers {
        editor
            .apply(Command::SetTimeSignatureChange { at, signature })
            .unwrap();
    }
    let track: TrackId = editor.document().top_level().nth(2).unwrap();
    for (start, end, hash) in [(0, 3, 1), (8, 10, 2)] {
        let id: ClipId = editor.new_id();
        let clip = Clip::new(
            id,
            bars(start),
            bars(end - start),
            ClipData::Audio(AudioData::new(ContentHash([hash; 32]))),
        );
        editor.apply(Command::CreateClip { track, clip }).unwrap();
    }
    editor
        .apply(Command::SetAutomationEnvelope {
            track,
            target: VOLUME,
            points: vec![
                Breakpoint {
                    at: bars(0),
                    value: 0.0,
                    bend: 0.0,
                },
                Breakpoint {
                    at: bars(8),
                    value: 1.0,
                    bend: 0.0,
                },
            ],
        })
        .unwrap();
    let locator: LocatorId = editor.new_id();
    editor
        .apply(Command::AddLocator {
            locator: Locator::new(locator, bars(7), "L".to_owned()),
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
        .run("set.open", json!({ "path": "Lane.blset" }))
        .unwrap();
    Set {
        shell,
        track,
        locator,
    }
}

impl Set {
    fn doc(&self) -> &Document {
        self.shell.editor().document()
    }

    fn run(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}"))
    }

    fn state(&mut self) -> ArrangementState {
        serde_json::from_value(self.run("arrangement.state", json!({}))).unwrap()
    }

    fn steps(&self) -> usize {
        self.shell.editor().undo_history().len()
    }

    fn last_step(&self) -> String {
        self.shell
            .editor()
            .undo_history()
            .next()
            .unwrap_or_default()
            .to_owned()
    }

    fn signatures(&self) -> Vec<(Ticks, TimeSignature)> {
        self.doc()
            .settings()
            .time_signatures
            .changes()
            .map(|c| (c.at, c.signature))
            .collect()
    }

    /// The starts of the clips on the track's main lane.
    fn clips(&self) -> Vec<(Ticks, Ticks)> {
        let mut spans: Vec<_> = self
            .doc()
            .track(self.track)
            .unwrap()
            .clips()
            .map(|c| (c.start, c.end()))
            .collect();
        spans.sort_unstable();
        spans
    }

    fn volume(&self, at: Ticks) -> f64 {
        self.doc()
            .track(self.track)
            .unwrap()
            .automation_for(&VOLUME)
            .unwrap()
            .envelope
            .value_at(at)
            .unwrap()
    }

    /// Runs `id`, checks it is exactly one undo step named `name`, and that
    /// undo and redo give the set back exactly, then leaves it done.
    fn one_step(&mut self, id: &str, args: Value, name: &str) {
        let before = self.doc().clone();
        let steps = self.steps();
        self.run(id, args);
        let after = self.doc().clone();
        assert_ne!(after, before, "{id} changed nothing");
        assert_eq!(self.steps(), steps + 1, "{id} is one undo step");
        assert_eq!(self.last_step(), name);
        self.run("edit.undo", json!({}));
        assert_eq!(*self.doc(), before, "undoing {id} restores the set exactly");
        self.run("edit.redo", json!({}));
        assert_eq!(*self.doc(), after, "redoing {id} restores it exactly");
        assert!(self.doc().check_invariants().is_ok());
    }

    /// Runs `id` expecting a refusal that leaves the set and its history
    /// alone.
    fn refused(&mut self, id: &str, args: Value) {
        let before = self.doc().clone();
        let steps = self.steps();
        assert!(self.shell.run(id, args).is_err(), "{id} should be refused");
        assert_eq!(*self.doc(), before);
        assert_eq!(self.steps(), steps);
    }
}

#[test]
fn every_lane_command_is_registered_with_its_class() {
    // Spec: time-editing R36.
    let shell = Shell::headless();
    for (id, class) in [
        (ids::SELECT_SIGNATURE, Permission::Read),
        (ids::INSERT_SIGNATURE, Permission::Edit),
        (ids::EDIT_SIGNATURE, Permission::Edit),
        (ids::MOVE_SIGNATURE, Permission::Edit),
        (
            edit_ids::DELETE_FRAGMENTARY_BAR_TIME,
            Permission::Destructive,
        ),
        (edit_ids::COMPLETE_FRAGMENTARY_BAR, Permission::Edit),
        ("song.set_time_signature_change", Permission::Edit),
        ("song.move_time_signature_change", Permission::Edit),
        ("song.delete_time_signature_change", Permission::Edit),
    ] {
        let spec = shell
            .registry()
            .get(id)
            .unwrap_or_else(|| panic!("{id} isn't registered"));
        assert_eq!(spec.permission, Some(class), "{id}");
        assert!(spec.args.is_some() && !spec.description.is_empty(), "{id}");
    }
}

#[test]
fn inserting_a_marker_selects_it_and_is_one_undo_step() {
    // Spec: time-editing R30.
    let mut s = set();
    s.run("arrangement.set_insert_marker", json!({ "at": bars(8) }));
    s.run(ids::INSERT_SIGNATURE, json!({}));
    // It takes the signature in force there, and is the one selected.
    assert_eq!(
        s.signatures(),
        [(Ticks::ZERO, sig(4, 4)), (bars(8), sig(4, 4))]
    );
    assert_eq!(s.state().selected_time_signature, Some(bars(8)));
    assert_eq!(s.steps(), 1);
    assert_eq!(s.last_step(), "Insert Time Signature Change");
    s.run("edit.undo", json!({}));
    assert_eq!(s.signatures().len(), 1);
    s.run("edit.redo", json!({}));
    // Where a marker is already, it selects that one and adds nothing.
    let steps = s.steps();
    s.run(ids::SELECT_SIGNATURE, json!({}));
    assert_eq!(s.state().selected_time_signature, None);
    s.run(ids::INSERT_SIGNATURE, json!({ "at": bars(8) }));
    s.run(ids::INSERT_SIGNATURE, json!({ "at": "1.1.1" }));
    assert_eq!(s.steps(), steps);
    assert_eq!(s.state().selected_time_signature, Some(Ticks::ZERO));
    assert_eq!(s.signatures().len(), 2);
    // From a menu or key it also opens the value for typing.
    s.shell.take_effects();
    s.shell
        .invoke(ids::INSERT_SIGNATURE, json!({ "at": bars(12) }))
        .unwrap();
    assert!(
        s.shell
            .take_effects()
            .contains(&Effect::EditTimeSignature(bars(12)))
    );
}

#[test]
fn typing_a_signature_takes_each_separator_and_refuses_what_live_does() {
    // Spec: time-editing R31.
    let mut s = set_with(&[(bars(4), sig(4, 4))]);
    s.run(ids::SELECT_SIGNATURE, json!({ "at": bars(4) }));
    for (text, want) in [
        ("6/8", sig(6, 8)),
        ("7,16", sig(7, 16)),
        ("3.4", sig(3, 4)),
        ("5 2", sig(5, 2)),
        ("12  8", sig(12, 8)),
    ] {
        s.one_step(
            ids::EDIT_SIGNATURE,
            json!({ "signature": text }),
            "Edit Time Signature",
        );
        assert_eq!(s.signatures()[1], (bars(4), want), "{text}");
    }
    // 0 beats and a denominator of 5 are outside the ranges.
    for text in ["0/4", "3/5", "100/4", "4/32", "4", ""] {
        s.refused(ids::EDIT_SIGNATURE, json!({ "signature": text }));
    }
    // With no signature given, a person types it: the shell asks the UI.
    s.shell.take_effects();
    s.shell.invoke(ids::EDIT_SIGNATURE, json!({})).unwrap();
    assert_eq!(s.shell.take_effects(), [Effect::EditTimeSignature(bars(4))]);
    // The marker at 1.1.1 can be typed over too.
    s.run(
        ids::EDIT_SIGNATURE,
        json!({ "at": "1.1.1", "signature": "3/4" }),
    );
    assert_eq!(s.signatures()[0], (Ticks::ZERO, sig(3, 4)));
}

#[test]
fn markers_move_and_delete_with_the_first_one_fixed() {
    // Spec: time-editing R32.
    let mut s = set_with(&[(bars(4), sig(3, 4)), (bars(8), sig(5, 4))]);
    s.run(ids::SELECT_SIGNATURE, json!({ "at": bars(4) }));
    s.run(ids::MOVE_SIGNATURE, json!({ "to": bars(5) }));
    // A selected marker stays selected where it lands.
    assert_eq!(s.state().selected_time_signature, Some(bars(5)));
    assert_eq!(s.steps(), 1);
    assert_eq!(s.last_step(), "Move Time Signature Change");
    s.run("edit.undo", json!({}));
    s.run(ids::SELECT_SIGNATURE, json!({ "at": bars(4) }));
    s.one_step(
        ids::MOVE_SIGNATURE,
        json!({ "to": bars(5) }),
        "Move Time Signature Change",
    );
    assert_eq!(
        s.signatures(),
        [
            (Ticks::ZERO, sig(4, 4)),
            (bars(5), sig(3, 4)),
            (bars(8), sig(5, 4))
        ]
    );
    // The first can't move or be deleted, and a marker can't land on
    // another.
    s.refused(ids::MOVE_SIGNATURE, json!({ "at": "1.1.1", "to": bars(1) }));
    s.refused(
        "song.delete_time_signature_change",
        json!({ "at": Ticks::ZERO }),
    );
    s.refused(ids::MOVE_SIGNATURE, json!({ "to": bars(8) }));
    s.refused(ids::MOVE_SIGNATURE, json!({ "at": bars(2), "to": bars(3) }));
    // Deleting is one undo step, and drops the selection.
    s.run(ids::SELECT_SIGNATURE, json!({ "at": bars(5) }));
    s.run(
        "song.delete_time_signature_change",
        json!({ "at": bars(5) }),
    );
    assert_eq!(s.state().selected_time_signature, None);
    assert_eq!(s.last_step(), "Delete time signature change");
    s.run("edit.undo", json!({}));
    s.run(ids::SELECT_SIGNATURE, json!({ "at": bars(5) }));
    // Without a selection, Left and Right have nothing to move.
    s.run(ids::SELECT_SIGNATURE, json!({}));
    assert!(
        !s.shell
            .can_invoke(ids::MOVE_SIGNATURE, &json!({ "to": bars(3) }))
    );
}

#[test]
fn selecting_a_marker_is_view_state_and_clears_the_locators() {
    // Spec: time-editing R31, R36.
    let mut s = set_with(&[(bars(4), sig(3, 4))]);
    s.run("locator.select", json!({ "locators": [s.locator] }));
    assert_eq!(s.state().selected_locators, [s.locator]);
    let steps = s.steps();
    s.run(ids::SELECT_SIGNATURE, json!({ "at": bars(4) }));
    let state = s.state();
    assert_eq!(state.selected_time_signature, Some(bars(4)));
    assert!(state.selected_locators.is_empty());
    assert_eq!(s.steps(), steps);
    // Selecting a locator lets go of the marker.
    s.run("locator.select", json!({ "locators": [s.locator] }));
    assert_eq!(s.state().selected_time_signature, None);
    // Where there is no marker, nothing is selected.
    assert!(
        s.shell
            .run(ids::SELECT_SIGNATURE, json!({ "at": bars(5) }))
            .is_err()
    );
    // A deleted marker is forgotten.
    s.run(ids::SELECT_SIGNATURE, json!({ "at": bars(4) }));
    s.run(
        "song.delete_time_signature_change",
        json!({ "at": bars(4) }),
    );
    assert_eq!(s.state().selected_time_signature, None);
}

#[test]
fn the_control_bars_fields_change_the_marker_in_force() {
    // Spec: time-editing R33.
    let mut s = set_with(&[(bars(4), sig(3, 4))]);
    let numerator = Field::new(Group::Numerator, 0);
    assert!(transport::editable(&s.shell, Group::Numerator));
    // Stopped, the fields follow the insert marker: before the change the
    // first marker is in force, after it the second.
    s.run("arrangement.set_insert_marker", json!({ "at": bars(2) }));
    let change = transport::step(&s.shell, numerator, 1, false).unwrap();
    s.shell.invoke(&change.id, change.args).unwrap();
    assert_eq!(
        s.signatures(),
        [(Ticks::ZERO, sig(5, 4)), (bars(4), sig(3, 4))]
    );
    s.run("arrangement.set_insert_marker", json!({ "at": bars(6) }));
    let change =
        transport::typed(&s.shell, Group::Numerator, &[Some("7".into()), None, None]).unwrap();
    s.shell.invoke(&change.id, change.args).unwrap();
    let change = transport::step(&s.shell, Field::new(Group::Denominator, 0), 1, false).unwrap();
    s.shell.invoke(&change.id, change.args).unwrap();
    assert_eq!(
        s.signatures(),
        [(Ticks::ZERO, sig(5, 4)), (bars(4), sig(7, 8))]
    );
    // Each field's edit is its own undo step.
    assert_eq!(s.steps(), 3);
    // Setting the value it already has changes nothing.
    assert!(
        transport::typed(&s.shell, Group::Numerator, &[Some("7".into()), None, None]).is_none()
    );
}

/// A set with 4/4 from the start and 3/4 from `change`, which cuts the bar
/// before it short when `change` is not on a bar line of the 4/4.
fn short_bar_set(change: Ticks) -> Set {
    set_with(&[(change, sig(3, 4))])
}

#[test]
fn delete_fragmentary_bar_time_puts_the_next_marker_on_a_bar_line() {
    // Spec: time-editing R34, R15.
    // 4/4 from the start, 3/4 from a quarter note and a half into bar 2: bar 2
    // is two beats long.
    let change = bars(1) + quarters(2);
    let mut s = short_bar_set(change);
    assert_eq!(
        s.doc()
            .settings()
            .time_signatures
            .fragmentary_bars()
            .count(),
        1
    );
    // A whole bar is refused, and so is a position past the change.
    s.refused(
        edit_ids::DELETE_FRAGMENTARY_BAR_TIME,
        json!({ "at": "1.1.1" }),
    );
    s.refused(
        edit_ids::DELETE_FRAGMENTARY_BAR_TIME,
        json!({ "at": bars(3) }),
    );
    s.one_step(
        edit_ids::DELETE_FRAGMENTARY_BAR_TIME,
        json!({ "at": bars(1) }),
        "Delete Fragmentary Bar Time",
    );
    // The marker is on a bar line now, and no bar is short.
    assert_eq!(
        s.signatures(),
        [(Ticks::ZERO, sig(4, 4)), (bars(1), sig(3, 4))]
    );
    assert_eq!(
        s.doc()
            .settings()
            .time_signatures
            .fragmentary_bars()
            .count(),
        0
    );
    // Every track's clips, envelopes and locators after the bar move
    // earlier by the two beats it held.
    let gone = quarters(2);
    assert_eq!(
        s.clips(),
        [
            (Ticks::ZERO, bars(1)),
            (bars(1), bars(3) - gone),
            (bars(8) - gone, bars(10) - gone)
        ]
    );
    assert!((s.volume(bars(8) - gone) - 1.0).abs() < 1e-9);
    assert_eq!(s.doc().locators()[0].position, bars(7) - gone);
    // The same, by any position inside the short bar.
    let mut s = short_bar_set(change);
    s.run(
        edit_ids::DELETE_FRAGMENTARY_BAR_TIME,
        json!({ "at": bars(1) + quarters(1) }),
    );
    assert_eq!(s.signatures()[1].0, bars(1));
}

#[test]
fn complete_fragmentary_bar_inserts_the_time_the_bar_lacks() {
    // Spec: time-editing R34, R17.
    let change = bars(1) + quarters(2);
    let mut s = short_bar_set(change);
    s.one_step(
        edit_ids::COMPLETE_FRAGMENTARY_BAR,
        json!({ "at": bars(1) }),
        "Complete Fragmentary Bar",
    );
    // Two beats open at the bar's start: the marker follows them, so the bar
    // is whole and the marker on a bar line.
    assert_eq!(
        s.signatures(),
        [(Ticks::ZERO, sig(4, 4)), (bars(2), sig(3, 4))]
    );
    assert_eq!(
        s.doc()
            .settings()
            .time_signatures
            .fragmentary_bars()
            .count(),
        0
    );
    let added = quarters(2);
    assert_eq!(
        s.clips(),
        [
            (Ticks::ZERO, bars(1)),
            (bars(1) + added, bars(3) + added),
            (bars(8) + added, bars(10) + added)
        ]
    );
    assert_eq!(s.doc().locators()[0].position, bars(7) + added);
    // Nothing short, nothing to complete.
    s.refused(edit_ids::COMPLETE_FRAGMENTARY_BAR, json!({ "at": "1.1.1" }));
}

#[test]
fn completing_a_short_bar_that_starts_at_a_marker_keeps_that_marker() {
    // Spec: time-editing R34. 3/4 from bar 2 to a marker two beats later:
    // the short bar starts where its own marker is.
    let mut s = set_with(&[(bars(1), sig(3, 4)), (bars(1) + quarters(2), sig(4, 4))]);
    s.run(edit_ids::COMPLETE_FRAGMENTARY_BAR, json!({ "at": bars(1) }));
    assert_eq!(
        s.signatures(),
        [
            (Ticks::ZERO, sig(4, 4)),
            (bars(1), sig(3, 4)),
            (bars(1) + quarters(3), sig(4, 4))
        ]
    );
    assert!(s.doc().check_invariants().is_ok());
}

#[test]
fn moving_a_marker_leaves_fragmentary_bars_as_they_fall() {
    // Spec: time-editing R35.
    let mut s = short_bar_set(bars(2));
    assert_eq!(
        s.doc()
            .settings()
            .time_signatures
            .fragmentary_bars()
            .count(),
        0
    );
    s.run(
        "song.move_time_signature_change",
        json!({ "at": bars(2), "to": bars(2) - quarters(1) }),
    );
    // The bar is short now, and nothing completed or removed it.
    assert_eq!(
        s.signatures(),
        [(Ticks::ZERO, sig(4, 4)), (bars(2) - quarters(1), sig(3, 4))]
    );
    assert_eq!(
        s.doc()
            .settings()
            .time_signatures
            .fragmentary_bars()
            .count(),
        1
    );
    assert_eq!(s.clips().len(), 2);
}

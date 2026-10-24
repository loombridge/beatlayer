//! The …Time commands, driven headless through `Shell::run` and
//! `Shell::invoke`, as agents, keys and menus drive them (WP-6.24b; the time
//! editing spec, R14 to R24).

use std::path::{Path, PathBuf};

use bl_commands::Permission;
use bl_core::time::{Ticks, TimeSignature};
use bl_model::{
    AudioData, AutomationTarget, Breakpoint, Clip, ClipData, ClipId, Command, ContentHash,
    Document, Editor, Locator, LocatorId, MixerParam, NewTakeLane, TakeClip, TakeClipData,
    TakeLaneId, TrackId,
};
use bl_shell::Shell;
use bl_shell::arrangement_commands::ArrangementState;
use bl_shell::audio::AudioHost;
use bl_shell::editing_commands::{self as ids, EditResult, TimeClipboardState};
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::settings::Preferences;
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();

fn bars(n: i64) -> Ticks {
    Ticks(n * BAR)
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

/// The factory default set with, on its two audio tracks A and B: clips
/// across bars 2 to 4 (a1 0 to 6 and a2 8 to 10 on A, b0 0 to 1 and b1 3 to 5
/// on B), a take lane on A (1 to 3), a volume ramp on A from 0 at bar 0 to 1
/// at bar 8, a tempo change at bar 3, a 3/4 marker at bar 3, and locators at
/// bars 1, 3, 4 and 6.
struct Set {
    shell: Shell,
    a: TrackId,
    b: TrackId,
    a1: ClipId,
    b1: ClipId,
    lane: TakeLaneId,
    locators: [LocatorId; 4],
}

fn set() -> Set {
    let mut editor = factory_default_set();
    let tracks: Vec<TrackId> = editor.document().top_level().collect();
    let (a, b) = (tracks[2], tracks[3]);
    let clip = |editor: &mut Editor, track, start, end, hash| -> ClipId {
        let id: ClipId = editor.new_id();
        let clip = Clip::new(
            id,
            bars(start),
            bars(end - start),
            ClipData::Audio(AudioData::new(ContentHash([hash; 32]))),
        );
        editor.apply(Command::CreateClip { track, clip }).unwrap();
        id
    };
    let a1 = clip(&mut editor, a, 0, 6, 1);
    clip(&mut editor, a, 8, 10, 2);
    clip(&mut editor, b, 0, 1, 3);
    let b1 = clip(&mut editor, b, 3, 5, 4);
    let lane: TakeLaneId = editor.new_id();
    editor
        .apply(Command::AddTakeLane {
            track: a,
            lane: NewTakeLane {
                id: Some(lane),
                name: "Take 1".to_owned(),
                clips: vec![TakeClip {
                    id: None,
                    name: "Take".to_owned(),
                    start: bars(1),
                    length: bars(2),
                    data: TakeClipData::Audio(AudioData::new(ContentHash([5; 32]))),
                }],
            },
            index: None,
        })
        .unwrap();
    editor
        .apply(Command::SetAutomationEnvelope {
            track: a,
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
    editor
        .apply(Command::SetTempoBreakpoint {
            at: bars(3),
            bpm: bl_core::time::Bpm::new(140.0).unwrap(),
        })
        .unwrap();
    editor
        .apply(Command::SetTimeSignatureChange {
            at: bars(3),
            signature: TimeSignature::new(3, 4).unwrap(),
        })
        .unwrap();
    let locators = [1, 3, 4, 6].map(|at| {
        let id: LocatorId = editor.new_id();
        let locator = Locator::new(id, bars(at), format!("L{at}"));
        editor.apply(Command::AddLocator { locator }).unwrap();
        id
    });
    editor.clear_history().unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Time.blset" }))
        .unwrap();
    Set {
        shell,
        a,
        b,
        a1,
        b1,
        lane,
        locators,
    }
}

impl Set {
    fn doc(&self) -> &Document {
        self.shell.editor().document()
    }

    fn edit(&mut self, id: &str, args: Value) -> EditResult {
        serde_json::from_value(
            self.shell
                .run(id, args)
                .unwrap_or_else(|e| panic!("{id} failed: {e}")),
        )
        .unwrap()
    }

    fn state(&mut self) -> ArrangementState {
        serde_json::from_value(self.shell.run("arrangement.state", json!({})).unwrap()).unwrap()
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

    /// The clips of a track's main lane, as `(start, end)` in bars.
    fn main(&self, track: TrackId) -> Vec<(i64, i64)> {
        let mut spans: Vec<_> = self
            .doc()
            .track(track)
            .unwrap()
            .clips()
            .map(|c| (c.start.get() / BAR, c.end().get() / BAR))
            .collect();
        spans.sort_unstable();
        spans
    }

    fn take(&self) -> Vec<(i64, i64)> {
        self.doc()
            .track(self.a)
            .unwrap()
            .take_lane(self.lane)
            .unwrap()
            .clips
            .iter()
            .map(|c| (c.start.get() / BAR, c.end().get() / BAR))
            .collect()
    }

    fn volume(&self, at: Ticks) -> f64 {
        self.doc()
            .track(self.a)
            .unwrap()
            .automation_for(&VOLUME)
            .unwrap()
            .envelope
            .value_at(at)
            .unwrap()
    }

    fn tempo(&self, at: Ticks) -> f64 {
        self.doc().settings().tempo.bpm_at(at).get()
    }

    fn signatures(&self) -> Vec<(Ticks, TimeSignature)> {
        self.doc()
            .settings()
            .time_signatures
            .changes()
            .map(|c| (c.at, c.signature))
            .collect()
    }

    fn locator_positions(&self) -> Vec<(String, Ticks)> {
        self.doc()
            .locators()
            .iter()
            .map(|l| (l.name.clone(), l.position))
            .collect()
    }

    /// Runs `id`, checks it is exactly one undo step named `name`, and that
    /// undo and redo give the set back exactly, then leaves it done.
    fn one_step(&mut self, id: &str, args: Value, name: &str) -> EditResult {
        let before = self.doc().clone();
        let steps = self.steps();
        let result = self.edit(id, args);
        let after = self.doc().clone();
        assert_ne!(after, before, "{id} changed nothing");
        assert!(result.undo_step, "{id}");
        assert_eq!(self.steps(), steps + 1, "{id} is one undo step");
        assert_eq!(self.last_step(), name);
        self.shell.run("edit.undo", json!({})).unwrap();
        assert_eq!(*self.doc(), before, "undoing {id} restores the set exactly");
        self.shell.run("edit.redo", json!({})).unwrap();
        assert_eq!(*self.doc(), after, "redoing {id} restores it exactly");
        assert!(self.doc().check_invariants().is_ok());
        result
    }
}

fn span(start: i64, end: i64) -> Value {
    json!({ "start": bars(start), "end": bars(end) })
}

#[test]
fn every_time_command_is_registered_with_its_class_and_keys() {
    // Spec: time-editing R21, R24.
    let shell = Shell::headless();
    let registry = shell.registry();
    for (id, class, shortcut) in [
        (ids::INSERT_SILENCE, Permission::Edit, "S-ARR-014"),
        (ids::CUT_TIME, Permission::Destructive, "S-ARR-015"),
        (ids::COPY_TIME, Permission::Read, "S-ARR-016"),
        (ids::PASTE_TIME, Permission::Edit, "S-ARR-017"),
        (ids::DUPLICATE_TIME, Permission::Edit, "S-ARR-018"),
        (ids::DELETE_TIME, Permission::Destructive, "S-ARR-019"),
    ] {
        let spec = registry.get(id).unwrap_or_else(|| panic!("{id}"));
        assert_eq!(spec.permission, Some(class), "{id}");
        assert!(spec.args.is_some() && spec.result.is_some(), "{id}");
        assert!(!spec.description.is_empty(), "{id}");
        assert!(
            spec.bindings.iter().any(|b| b.shortcut == shortcut),
            "{id} binds {shortcut}"
        );
    }
    assert!(registry.problems().is_empty(), "{:?}", registry.problems());
}

#[test]
fn delete_time_closes_the_span_on_every_track() {
    // Spec: time-editing R14, R15.
    let mut s = set();
    let result = s.one_step(ids::DELETE_TIME, span(2, 4), "Delete Time");
    // A clip across the whole span is split, its parts closing up.
    assert_eq!(s.main(s.a), [(0, 2), (2, 4), (6, 8)]);
    assert_eq!(s.main(s.b), [(0, 1), (2, 3)]);
    assert_eq!(s.take(), [(1, 2)]);
    assert!(!result.created.is_empty());
    // The part after the split plays what it played: its contents start
    // where the clip's did, two bars on from its new start.
    let tempo = s.doc().settings().tempo.clone();
    let origins: Vec<i64> = {
        let mut clips: Vec<_> = s.doc().track(s.a).unwrap().clips().collect();
        clips.sort_by_key(|c| c.start);
        clips
            .iter()
            .map(|c| (c.start - c.origin(&tempo)).get() / BAR)
            .collect()
    };
    assert_eq!(origins[0], 0);
    assert!(
        origins[1] > 0,
        "the part after the split starts into the file"
    );
    // The envelope keeps its shape either side and steps over a tick.
    assert!((s.volume(bars(1)) - 0.125).abs() < 1e-9);
    assert!((s.volume(bars(2)) - 0.25).abs() < 1e-9);
    assert!((s.volume(bars(2) + Ticks(1)) - 0.5).abs() < 1e-9);
    assert!((s.volume(bars(4)) - 0.75).abs() < 1e-6);
    // The tempo jumps at the span's start, with two breakpoints there.
    assert!((s.tempo(bars(1)) - (120.0 + 20.0 / 3.0)).abs() < 1e-6);
    assert!((s.tempo(bars(2)) - 140.0).abs() < 1e-9);
    // The 3/4 marker inside the span was in force at its end, so it holds
    // from the start.
    assert_eq!(
        s.signatures(),
        [
            (Ticks::ZERO, TimeSignature::default()),
            (bars(2), TimeSignature::new(3, 4).unwrap())
        ]
    );
    // Locators inside go; the ones after move up.
    assert_eq!(
        s.locator_positions(),
        [
            ("L1".to_owned(), bars(1)),
            ("L4".to_owned(), bars(2)),
            ("L6".to_owned(), bars(4))
        ]
    );
    // The insert marker is at the start, with no time selected.
    let state = s.state();
    assert_eq!(state.time_selection, None);
    assert_eq!(state.insert_marker, bars(2));
    assert_eq!(result.time_selection, None);
}

#[test]
fn insert_silence_opens_time_on_every_track() {
    // Spec: time-editing R14, R17.
    let mut s = set();
    let result = s.one_step(ids::INSERT_SILENCE, span(2, 4), "Insert Silence");
    assert_eq!(s.main(s.a), [(0, 2), (4, 8), (10, 12)]);
    assert_eq!(s.main(s.b), [(0, 1), (5, 7)]);
    assert_eq!(s.take(), [(1, 2), (4, 5)]);
    // The envelope holds the value it had at the point across the new time.
    assert!((s.volume(bars(2)) - 0.25).abs() < 1e-9);
    assert!((s.volume(bars(3)) - 0.25).abs() < 1e-9);
    assert!((s.volume(bars(4)) - 0.25).abs() < 1e-9);
    assert!((s.volume(bars(10)) - 1.0).abs() < 1e-9);
    // So do the tempo and the signature.
    let held = 120.0 + 40.0 / 3.0;
    assert!((s.tempo(bars(2)) - held).abs() < 1e-6);
    assert!((s.tempo(bars(3)) - held).abs() < 1e-6);
    assert!((s.tempo(bars(5)) - 140.0).abs() < 1e-9);
    assert_eq!(s.signatures()[1].0, bars(5));
    assert_eq!(
        s.locator_positions(),
        [
            ("L1".to_owned(), bars(1)),
            ("L3".to_owned(), bars(5)),
            ("L4".to_owned(), bars(6)),
            ("L6".to_owned(), bars(8))
        ]
    );
    // The inserted span is the time selection.
    let selected = result.time_selection.unwrap();
    assert_eq!((selected.start, selected.end), (bars(2), bars(4)));
    assert_eq!(s.state().insert_marker, bars(2));
}

#[test]
fn copy_time_changes_nothing_and_reports_what_the_clipboard_holds() {
    // Spec: time-editing R18.
    let mut s = set();
    let before = s.doc().clone();
    let steps = s.steps();
    let held: TimeClipboardState =
        serde_json::from_value(s.shell.run(ids::COPY_TIME, span(2, 4)).unwrap()).unwrap();
    assert_eq!(*s.doc(), before);
    assert_eq!(s.steps(), steps);
    assert_eq!(held.length, bars(2));
    // a1, b1 and the take, which cross the span.
    assert_eq!(held.clips, 3);
    assert_eq!(held.tracks, 2);
    assert_eq!(held.envelopes, 1);
    assert_eq!(held.time_signatures, 1);
    assert_eq!(held.locators, 1);
}

#[test]
fn cut_time_then_paste_time_at_the_same_place_gives_the_set_back() {
    // Spec: time-editing R16, R19.
    let mut s = set();
    let before = s.doc().clone();
    let (a, b) = (s.main(s.a), s.main(s.b));
    let cut = s.one_step(ids::CUT_TIME, span(2, 4), "Cut Time");
    assert_eq!(s.main(s.a), [(0, 2), (2, 4), (6, 8)]);
    assert_eq!(cut.time_selection, None);
    let pasted = s.one_step(ids::PASTE_TIME, json!({ "at": bars(2) }), "Paste Time");
    // Every track has the clips it had, in pieces where the span cut them.
    let merged = |spans: Vec<(i64, i64)>| -> Vec<(i64, i64)> {
        let mut out: Vec<(i64, i64)> = Vec::new();
        for (start, end) in spans {
            match out.last_mut() {
                Some(last) if last.1 == start => last.1 = end,
                _ => out.push((start, end)),
            }
        }
        out
    };
    assert_eq!(merged(s.main(s.a)), a);
    assert_eq!(merged(s.main(s.b)), b);
    assert_eq!(merged(s.take()), [(1, 3)]);
    for whole in 0..=8 {
        for fraction in [0, BAR / 2] {
            let at = Ticks(whole * BAR + fraction);
            let was = before
                .track(s.a)
                .unwrap()
                .automation_for(&VOLUME)
                .unwrap()
                .envelope
                .value_at(at)
                .unwrap();
            assert!((s.volume(at) - was).abs() < 1e-6, "volume at {at:?}");
            assert!(
                (s.tempo(at) - before.settings().tempo.bpm_at(at).get()).abs() < 1e-6,
                "tempo at {at:?}"
            );
        }
    }
    let was: Vec<_> = before
        .settings()
        .time_signatures
        .changes()
        .map(|c| (c.at, c.signature))
        .collect();
    assert_eq!(s.signatures(), was);
    assert_eq!(
        s.locator_positions(),
        before
            .locators()
            .iter()
            .map(|l| (l.name.clone(), l.position))
            .collect::<Vec<_>>()
    );
    // The pasted span is selected.
    let selected = pasted.time_selection.unwrap();
    assert_eq!((selected.start, selected.end), (bars(2), bars(4)));
    assert!(s.doc().check_invariants().is_ok());
}

#[test]
fn duplicate_time_puts_a_copy_right_after_and_leaves_the_clipboard() {
    // Spec: time-editing R20.
    let mut s = set();
    s.shell.run(ids::COPY_TIME, span(0, 1)).unwrap();
    let result = s.one_step(ids::DUPLICATE_TIME, span(2, 4), "Duplicate Time");
    // The copy of bars 2 to 4 lands at 4 to 6, b1's piece a bar into it.
    assert_eq!(s.main(s.b), [(0, 1), (3, 4), (5, 6), (6, 7)]);
    let selected = result.time_selection.unwrap();
    assert_eq!((selected.start, selected.end), (bars(4), bars(6)));
    // Pressing it again lays down another.
    s.edit(ids::DUPLICATE_TIME, json!({}));
    assert_eq!(s.state().time_selection.unwrap().start, bars(6));
    // The clipboard still holds Copy Time's span of bar 0.
    s.edit(ids::PASTE_TIME, json!({ "at": bars(20) }));
    assert!(s.main(s.b).contains(&(20, 21)));
}

#[test]
fn paste_time_puts_clips_from_copy_where_paste_puts_them() {
    // Spec: time-editing R19.
    let mut s = set();
    s.shell
        .run("arrangement.copy", json!({ "clips": [s.b1] }))
        .unwrap();
    s.one_step(ids::PASTE_TIME, json!({ "at": bars(7) }), "Paste Time");
    // Two bars of time open at 7 on every track, and the clip lands in them
    // on track B, where it came from.
    assert_eq!(s.main(s.b), [(0, 1), (3, 5), (7, 9)]);
    assert_eq!(s.main(s.a), [(0, 6), (10, 12)]);
}

#[test]
fn the_time_commands_act_on_the_time_selection_and_refuse_without_one() {
    // Spec: time-editing R14, R17, R22.
    let mut s = set();
    let before = s.doc().clone();
    let all = [
        ids::INSERT_SILENCE,
        ids::CUT_TIME,
        ids::COPY_TIME,
        ids::DUPLICATE_TIME,
        ids::DELETE_TIME,
    ];
    for id in all {
        assert!(!s.shell.can_invoke(id, &json!({})), "{id} with no span");
        assert!(s.shell.run(id, json!({})).is_err(), "{id} with no span");
    }
    assert!(!s.shell.can_invoke(ids::PASTE_TIME, &json!({})));
    assert_eq!(*s.doc(), before);

    // Selected clips give the span of the clips, across every track.
    s.shell
        .run("arrangement.select_clips", json!({ "clips": [s.b1] }))
        .unwrap();
    for id in all {
        assert!(s.shell.can_invoke(id, &json!({})), "{id} with clips");
    }
    s.edit(ids::DELETE_TIME, json!({}));
    // b1 was 3 to 5, so bars 3 to 5 closed up on both tracks.
    assert_eq!(s.main(s.a), [(0, 3), (3, 4), (6, 8)]);
    assert!(s.main(s.b).iter().all(|(_, end)| *end <= 3));
    // With a time selection and no clips, it is the time.
    s.shell
        .run(
            "arrangement.select_time",
            json!({ "start": bars(0), "end": bars(1) }),
        )
        .unwrap();
    assert!(s.shell.can_invoke(ids::INSERT_SILENCE, &json!({})));
    s.edit(ids::INSERT_SILENCE, json!({}));
    assert_eq!(s.main(s.b), [(1, 2)]);
    s.shell.invoke(ids::COPY_TIME, json!({})).unwrap();
    assert!(s.shell.can_invoke(ids::PASTE_TIME, &json!({})));
}

#[test]
fn a_track_gone_since_the_copy_gets_nothing() {
    // Spec: time-editing R19.
    let mut s = set();
    s.shell.run(ids::COPY_TIME, span(2, 4)).unwrap();
    s.shell
        .run("track.delete", json!({ "track": s.b }))
        .unwrap();
    s.edit(ids::PASTE_TIME, json!({ "at": bars(10) }));
    assert!(s.doc().track(s.b).is_none());
    assert_eq!(s.main(s.a).last(), Some(&(10, 12)));
}

/// A small linear congruential generator, so the random run repeats.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, below: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % below.max(1)
    }
}

#[test]
fn random_time_commands_among_random_edits_keep_the_rules_and_undo_exactly() {
    // Spec: time-editing R23. No two clips of a track overlap, no two
    // breakpoints of an envelope or locators share a position, and undo and
    // redo give the set back exactly.
    let mut s = set();
    let mut rng = Lcg(0x71e5);
    let mut versions = vec![s.doc().clone()];
    let quarter = BAR / 4;
    for _ in 0..150 {
        let at = (rng.next(40) as i64) * quarter;
        let len = (1 + rng.next(12) as i64) * quarter;
        let (id, args) = match rng.next(9) {
            0 => (ids::INSERT_SILENCE, json!({ "start": at, "end": at + len })),
            1 => (ids::DELETE_TIME, json!({ "start": at, "end": at + len })),
            2 => (ids::CUT_TIME, json!({ "start": at, "end": at + len })),
            3 => (ids::COPY_TIME, json!({ "start": at, "end": at + len })),
            4 => (ids::PASTE_TIME, json!({ "at": at })),
            5 => (ids::DUPLICATE_TIME, json!({ "start": at, "end": at + len })),
            6 => (
                "arrangement.move_clips",
                json!({ "clips": [s.a1], "by": quarter * (rng.next(9) as i64 - 4) }),
            ),
            7 => (
                "arrangement.delete",
                json!({ "tracks": [s.b], "start": at, "end": at + len }),
            ),
            _ => ("locator.add", json!({ "at": at })),
        };
        let before = s.doc().clone();
        let steps = s.steps();
        // A refused command, such as a clip that would start before 1.1.1
        // or one whose clip is gone, changes nothing.
        match s.shell.run(id, args) {
            Ok(_) if s.steps() == steps + 1 => versions.push(s.doc().clone()),
            Ok(_) | Err(_) => assert_eq!(*s.doc(), before, "{id} changed nothing"),
        }
        let doc = s.doc();
        assert!(doc.check_invariants().is_ok(), "{id}");
        for track in doc.tracks() {
            let mut lanes: Vec<Vec<&Clip>> = vec![track.clips().collect()];
            lanes.extend(track.take_lanes().map(|l| l.clips.iter().collect()));
            for mut clips in lanes {
                clips.sort_by_key(|c| c.start);
                for pair in clips.windows(2) {
                    assert!(pair[0].end() <= pair[1].start, "{id} overlapped clips");
                }
            }
            for (_, automation) in track.automation() {
                let points: Vec<Ticks> = automation.envelope.points().map(|p| p.at).collect();
                assert!(points.windows(2).all(|w| w[0] < w[1]), "{id}");
            }
        }
        let mut at: Vec<Ticks> = doc.locators().iter().map(|l| l.position).collect();
        at.dedup();
        assert_eq!(at.len(), doc.locators().len(), "{id}");
    }
    // Undo every step, checking each version, then redo them all.
    for version in versions.iter().rev().skip(1) {
        s.shell.run("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc(), version);
    }
    for version in versions.iter().skip(1) {
        s.shell.run("edit.redo", json!({})).unwrap();
        assert_eq!(s.doc(), version);
    }
}

#[test]
fn the_locators_a_paste_makes_are_new_and_keep_their_names() {
    // Spec: time-editing R19.
    let mut s = set();
    s.shell.run(ids::COPY_TIME, span(2, 4)).unwrap();
    s.edit(ids::PASTE_TIME, json!({ "at": bars(12) }));
    let pasted = s
        .doc()
        .locators()
        .iter()
        .find(|l| l.position == bars(13))
        .expect("L3's copy is a bar into the pasted span");
    assert_eq!(pasted.name, "L3");
    assert!(!s.locators.contains(&pasted.id));
}

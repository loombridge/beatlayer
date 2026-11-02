//! The arranger track, driven headless through `Shell::run`, as agents, keys
//! and menus drive it (WP-6.45; the arranger track spec, R1 to R16).

use std::path::{Path, PathBuf};

use bl_commands::Permission;
use bl_core::time::{Ticks, TimeSignature};
use bl_model::{
    AudioData, AutomationTarget, Breakpoint, Clip, ClipData, ClipId, Command, ContentHash,
    Document, Editor, Locator, LocatorId, MixerParam, NewTakeLane, SectionId, SongSection,
    TakeClip, TakeClipData, TakeLaneId, TrackId,
};
use bl_shell::Shell;
use bl_shell::arrangement_commands::ArrangementState;
use bl_shell::audio::AudioHost;
use bl_shell::editing_commands::{self as edit_ids, EditResult};
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::settings::Preferences;
use bl_shell_spec::arranger::{self as ids, ArrangerResult};
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

/// The factory default set (the time editing tests' set, with sections) with, on its two audio tracks A and B: clips
/// across bars 2 to 4 (a1 0 to 6 and a2 8 to 10 on A, b0 0 to 1 and b1 3 to 5
/// on B), a take lane on A (1 to 3), a volume ramp on A from 0 at bar 0 to 1
/// at bar 8, a tempo change at bar 3, a 3/4 marker at bar 3, and locators at
/// bars 1, 3, 4 and 6.
struct Set {
    sections: [SectionId; 3],
    shell: Shell,
    a1: ClipId,
}

fn set(with_sections: bool) -> Set {
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
    clip(&mut editor, b, 3, 5, 4);
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
    for at in [1, 3, 4, 6] {
        let id: LocatorId = editor.new_id();
        let locator = Locator::new(id, bars(at), format!("L{at}"));
        editor.apply(Command::AddLocator { locator }).unwrap();
    }
    // Intro, Verse and Chorus over bars 0 to 2, 2 to 5 and 5 to 8.
    let sections = [(0, 2, "Intro"), (2, 5, "Verse"), (5, 8, "Chorus")].map(|(from, to, name)| {
        let id: SectionId = editor.new_id();
        if with_sections {
            let section = SongSection {
                id,
                start: bars(from),
                length: bars(to - from),
                name: name.to_owned(),
                colour: (from % 14) as u8,
            };
            editor.apply(Command::AddSection { section }).unwrap();
        }
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
        sections,
        shell,
        a1,
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

    /// The sections, as `(name, start, end)` in bars.
    fn section_spans(&self) -> Vec<(String, i64, i64)> {
        self.doc()
            .sections()
            .iter()
            .map(|s| (s.name.clone(), s.start.get() / BAR, s.end().get() / BAR))
            .collect()
    }

    /// The ID of the section with this name.
    fn id_of(&self, name: &str) -> SectionId {
        self.doc()
            .sections()
            .iter()
            .find(|s| s.name == name)
            .unwrap()
            .id
    }

    fn run(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}"))
    }

    fn arranger(&mut self, id: &str, args: Value) -> ArrangerResult {
        serde_json::from_value(
            self.shell
                .run(id, args)
                .unwrap_or_else(|e| panic!("{id} failed: {e}")),
        )
        .unwrap()
    }

    /// The document without its sections, which Cut Time and Paste Time give
    /// new IDs.
    fn without_section_ids(&self) -> Value {
        let mut saved = serde_json::to_value(self.doc()).unwrap();
        saved["settings"]
            .as_object_mut()
            .unwrap()
            .remove("sections");
        saved
    }

    /// Runs `id`, checks it is exactly one undo step named `name`, and that
    /// undo and redo give the set back exactly, then leaves it done.
    fn one_step(&mut self, id: &str, args: Value, name: &str) -> ArrangerResult {
        let before = self.doc().clone();
        let steps = self.steps();
        let result = self.arranger(id, args);
        let after = self.doc().clone();
        assert_ne!(after, before, "{id} changed nothing");
        assert!(result.undo_step, "{id}");
        assert!(after.check_invariants().is_ok(), "{id}");
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
fn every_arranger_command_is_registered_with_its_class() {
    // Spec: arranger-track R15.
    let shell = Shell::headless();
    let registry = shell.registry();
    for (id, class) in [
        (ids::ADD_SECTION, Permission::Edit),
        (ids::SECTIONS_FROM_LOCATORS, Permission::Edit),
        (ids::RENAME_SECTION, Permission::Edit),
        (ids::SET_SECTION_COLOUR, Permission::Edit),
        (ids::RESIZE_SECTION, Permission::Edit),
        (ids::MOVE_SECTION, Permission::Edit),
        (ids::COPY_SECTION, Permission::Edit),
        (ids::DUPLICATE_SECTION, Permission::Edit),
        (ids::REMOVE_SECTION, Permission::Edit),
        (ids::DELETE_SECTION, Permission::Destructive),
        (ids::SELECT_SECTION, Permission::Read),
        (ids::ARRANGER_TRACK, Permission::Read),
    ] {
        let spec = registry.get(id).unwrap_or_else(|| panic!("{id}"));
        assert_eq!(spec.permission, Some(class), "{id}");
        assert!(spec.args.is_some() && spec.result.is_some(), "{id}");
        assert!(!spec.description.is_empty(), "{id}");
    }
    assert!(registry.problems().is_empty(), "{:?}", registry.problems());
}

#[test]
fn moving_a_section_is_a_cut_time_and_a_paste_time_in_one_step() {
    // Spec: arranger-track R8. Moving Verse before Intro leaves every clip,
    // envelope breakpoint, tempo breakpoint, time-signature marker and
    // locator where Cut Time of Verse and Paste Time at Intro's start would.
    let mut moved = set(true);
    let verse = moved.sections[1];
    moved.run(edit_ids::COPY_TIME, span(8, 9));
    let result = moved.one_step(
        ids::MOVE_SECTION,
        json!({ "section": verse, "to": bars(0) }),
        "Move Section",
    );
    assert_eq!(result.selected_sections.len(), 1);
    let landed = moved.doc().section(result.selected_sections[0]).unwrap();
    assert_eq!(landed.name, "Verse", "the moved section is selected");
    let mut by_hand = set(true);
    by_hand.edit(edit_ids::CUT_TIME, span(2, 5));
    by_hand.edit(edit_ids::PASTE_TIME, json!({ "at": bars(0) }));
    assert_eq!(moved.without_section_ids(), by_hand.without_section_ids());
    assert_eq!(
        moved.section_spans(),
        [
            ("Verse".to_owned(), 0, 3),
            ("Intro".to_owned(), 3, 5),
            ("Chorus".to_owned(), 5, 8)
        ]
    );
    // The time clipboard is left as it was: the bar copied before.
    let pasted = moved.edit(edit_ids::PASTE_TIME, json!({ "at": bars(20) }));
    let selected = pasted.time_selection.unwrap();
    assert_eq!((selected.start, selected.end), (bars(20), bars(21)));
}

#[test]
fn moving_a_section_later_lands_it_after_the_one_it_passes() {
    // Spec: arranger-track R8, R12.
    let mut s = set(true);
    let intro = s.sections[0];
    s.one_step(
        ids::MOVE_SECTION,
        json!({ "section": intro, "to": bars(8) }),
        "Move Section",
    );
    assert_eq!(
        s.section_spans(),
        [
            ("Verse".to_owned(), 0, 3),
            ("Chorus".to_owned(), 3, 6),
            ("Intro".to_owned(), 6, 8)
        ]
    );
    // Move Earlier and Move Later swap with a neighbour; none is refused.
    let chorus = s.id_of("Chorus");
    s.one_step(
        ids::MOVE_SECTION,
        json!({ "section": chorus, "with": "earlier" }),
        "Move Section",
    );
    assert_eq!(s.section_spans()[0].0, "Chorus");
    let chorus = s.id_of("Chorus");
    s.one_step(
        ids::MOVE_SECTION,
        json!({ "section": chorus, "with": "later" }),
        "Move Section",
    );
    assert_eq!(s.section_spans()[0].0, "Verse");
    let last = s.doc().sections().last().unwrap().id;
    assert!(
        s.shell
            .run(
                ids::MOVE_SECTION,
                json!({ "section": last, "with": "later" })
            )
            .is_err()
    );
    assert!(
        s.shell
            .run(ids::MOVE_SECTION, json!({ "section": last }))
            .is_err(),
        "a move needs `to` or `with`"
    );
}

#[test]
fn a_drop_inside_the_sections_own_span_changes_nothing() {
    // Spec: arranger-track R8.
    let mut s = set(true);
    let verse = s.sections[1];
    let before = s.doc().clone();
    for to in [bars(2), bars(3), bars(5)] {
        let result = s.arranger(ids::MOVE_SECTION, json!({ "section": verse, "to": to }));
        assert!(!result.undo_step);
    }
    assert_eq!(*s.doc(), before);
    assert_eq!(s.steps(), 0);
}

#[test]
fn a_section_dropped_inside_another_is_cut_to_the_free_part() {
    // Spec: arranger-track R8, R16. Opening time inside Verse lengthens it,
    // and the moved section, wholly inside it now, leaves only its time.
    let mut s = set(true);
    let intro = s.sections[0];
    s.one_step(
        ids::MOVE_SECTION,
        json!({ "section": intro, "to": bars(3) }),
        "Move Section",
    );
    assert_eq!(
        s.section_spans(),
        [("Verse".to_owned(), 0, 5), ("Chorus".to_owned(), 5, 8)]
    );
}

#[test]
fn copying_a_section_is_a_copy_time_and_a_paste_time_and_leaves_the_original() {
    // Spec: arranger-track R9.
    let mut copied = set(true);
    let intro = copied.sections[0];
    let result = copied.one_step(
        ids::COPY_SECTION,
        json!({ "section": intro, "to": bars(8) }),
        "Copy Section",
    );
    let mut by_hand = set(true);
    by_hand.run(edit_ids::COPY_TIME, span(0, 2));
    by_hand.edit(edit_ids::PASTE_TIME, json!({ "at": bars(8) }));
    assert_eq!(copied.without_section_ids(), by_hand.without_section_ids());
    assert_eq!(copied.section_spans().len(), 4);
    assert_eq!(copied.section_spans()[3], ("Intro".to_owned(), 8, 10));
    assert_ne!(result.selected_sections, [intro], "the copy is selected");
    assert_eq!(result.selected_sections.len(), 1);
}

#[test]
fn duplicating_a_section_puts_a_copy_straight_after_it() {
    // Spec: arranger-track R10.
    let mut dup = set(true);
    let verse = dup.sections[1];
    let result = dup.one_step(
        ids::DUPLICATE_SECTION,
        json!({ "section": verse }),
        "Duplicate Section",
    );
    let mut by_hand = set(true);
    by_hand.edit(edit_ids::DUPLICATE_TIME, span(2, 5));
    assert_eq!(dup.without_section_ids(), by_hand.without_section_ids());
    assert_eq!(
        dup.section_spans(),
        [
            ("Intro".to_owned(), 0, 2),
            ("Verse".to_owned(), 2, 5),
            ("Verse".to_owned(), 5, 8),
            ("Chorus".to_owned(), 8, 11)
        ]
    );
    let copy = dup.doc().sections()[2].clone();
    assert_eq!(result.selected_sections, [copy.id]);
    assert_eq!(copy.colour, dup.doc().section(verse).unwrap().colour);
}

#[test]
fn deleting_a_section_closes_the_song_up_and_removing_one_leaves_the_tracks() {
    // Spec: arranger-track R11.
    let mut deleted = set(true);
    let verse = deleted.sections[1];
    deleted.one_step(
        ids::DELETE_SECTION,
        json!({ "section": verse }),
        "Delete Section",
    );
    let mut by_hand = set(true);
    by_hand.edit(edit_ids::DELETE_TIME, span(2, 5));
    assert_eq!(deleted.doc(), by_hand.doc());
    assert_eq!(
        deleted.section_spans(),
        [("Intro".to_owned(), 0, 2), ("Chorus".to_owned(), 2, 5)]
    );
    let mut removed = set(true);
    let before = removed.without_section_ids();
    let result = removed.one_step(
        ids::REMOVE_SECTION,
        json!({ "section": verse }),
        "Remove Section",
    );
    assert!(result.selected_sections.is_empty());
    assert_eq!(removed.without_section_ids(), before);
    assert_eq!(removed.section_spans().len(), 2);
}

#[test]
fn the_time_commands_lengthen_cut_move_and_carry_sections() {
    // Spec: arranger-track R13.
    let mut s = set(true);
    s.edit(edit_ids::INSERT_SILENCE, span(3, 4));
    assert_eq!(
        s.section_spans(),
        [
            ("Intro".to_owned(), 0, 2),
            ("Verse".to_owned(), 2, 6),
            ("Chorus".to_owned(), 6, 9)
        ]
    );
    let mut s = set(true);
    s.edit(edit_ids::DELETE_TIME, span(1, 3));
    assert_eq!(
        s.section_spans(),
        [
            ("Intro".to_owned(), 0, 1),
            ("Verse".to_owned(), 1, 3),
            ("Chorus".to_owned(), 3, 6)
        ]
    );
    // Cut Time of bars 3 to 6 and Paste Time at 1.1.1 carries the parts of
    // Verse and Chorus inside it, cut to it.
    let mut s = set(true);
    s.edit(edit_ids::CUT_TIME, span(3, 6));
    s.edit(edit_ids::PASTE_TIME, json!({ "at": bars(0) }));
    assert_eq!(
        s.section_spans(),
        [
            ("Verse".to_owned(), 0, 2),
            ("Chorus".to_owned(), 2, 3),
            ("Intro".to_owned(), 3, 5),
            ("Verse".to_owned(), 5, 6),
            ("Chorus".to_owned(), 6, 8)
        ]
    );
    // Edits of clips alone never move sections.
    let mut s = set(true);
    let before = s.section_spans();
    s.edit(
        "arrangement.move_clips",
        json!({ "clips": [s.a1], "by": bars(1) }),
    );
    assert_eq!(s.section_spans(), before);
}

#[test]
fn a_section_is_made_by_span_cut_short_and_never_over_another() {
    // Spec: arranger-track R5.
    let mut s = set(true);
    let before = s.doc().clone();
    assert!(
        s.shell.run(ids::ADD_SECTION, span(3, 4)).is_err(),
        "a start inside a section is refused"
    );
    assert_eq!(*s.doc(), before);
    let made = s.one_step(
        ids::ADD_SECTION,
        json!({ "start": bars(10), "end": bars(12), "name": "Outro" }),
        "Add Section",
    );
    assert_eq!(made.selected_sections.len(), 1);
    // The next section reached cuts a new one short; the colour is the
    // palette's next, the name the next whole number.
    s.one_step(ids::ADD_SECTION, span(8, 11 + 3), "Add Section");
    let sections = s.doc().sections();
    assert_eq!((sections[3].start, sections[3].end()), (bars(8), bars(10)));
    assert_eq!(sections[3].name, "Section 1");
    assert_eq!(sections[3].colour, 7, "the one after the newest section's");
    // With no span it takes the time selection.
    s.run(
        "arrangement.select_time",
        json!({ "start": bars(13), "end": bars(14) }),
    );
    s.one_step(ids::ADD_SECTION, json!({}), "Add Section");
    assert_eq!(s.doc().sections().last().unwrap().name, "Section 2");
}

#[test]
fn sections_are_made_from_the_locators_where_none_is() {
    // Spec: arranger-track R5. Locators at bars 1, 3, 4 and 6; the
    // arrangement ends at bar 10.
    let mut s = set(false);
    s.one_step(ids::SECTIONS_FROM_LOCATORS, json!({}), "Add Section");
    // The last runs to the Arrangement's end: 16 bars at least.
    let end = s.doc().settings().time_signatures.bar_start(16);
    assert_eq!(s.doc().sections().last().unwrap().end(), end);
    assert_eq!(
        s.section_spans()[..3],
        [
            ("L1".to_owned(), 1, 3),
            ("L3".to_owned(), 3, 4),
            ("L4".to_owned(), 4, 6)
        ]
    );
    // Wherever a section is already, a locator makes none.
    let mut s = set(true);
    let result = s.arranger(ids::SECTIONS_FROM_LOCATORS, json!({}));
    assert!(!result.undo_step);
    assert_eq!(s.section_spans().len(), 3);
}

#[test]
fn renaming_colouring_and_resizing_are_one_undo_step_each() {
    // Spec: arranger-track R7.
    let mut s = set(true);
    let (verse, chorus) = (s.sections[1], s.sections[2]);
    s.one_step(
        ids::RENAME_SECTION,
        json!({ "section": verse, "name": "Bridge" }),
        "Rename Section",
    );
    assert_eq!(s.doc().section(verse).unwrap().name, "Bridge");
    s.one_step(
        ids::SET_SECTION_COLOUR,
        json!({ "section": verse, "colour": 9 }),
        "Set Section Colour",
    );
    let clips = s.without_section_ids();
    s.one_step(
        ids::RESIZE_SECTION,
        json!({ "section": chorus, "start": bars(6), "end": bars(9) }),
        "Resize Section",
    );
    assert_eq!(s.section_spans()[2], ("Chorus".to_owned(), 6, 9));
    assert_eq!(
        s.without_section_ids(),
        clips,
        "nothing on the tracks moves"
    );
    // A span that would reach another section is refused.
    let before = s.doc().clone();
    assert!(
        s.shell
            .run(
                ids::RESIZE_SECTION,
                json!({ "section": verse, "end": bars(7) })
            )
            .is_err()
    );
    assert_eq!(*s.doc(), before);
}

#[test]
fn selecting_a_section_selects_its_time_and_is_no_undo_step() {
    // Spec: arranger-track R6.
    let mut s = set(true);
    let [intro, verse, chorus] = s.sections;
    s.run(ids::SELECT_SECTION, json!({ "sections": [verse] }));
    let state = s.state();
    assert_eq!(state.selected_sections, [verse]);
    let time = state.time_selection.unwrap();
    assert_eq!((time.start, time.end), (bars(2), bars(5)));
    s.run(
        ids::SELECT_SECTION,
        json!({ "sections": [chorus], "mode": "extend" }),
    );
    assert_eq!(s.state().selected_sections, [verse, chorus]);
    s.run(
        ids::SELECT_SECTION,
        json!({ "sections": [intro], "mode": "add" }),
    );
    assert_eq!(s.state().selected_sections, [verse, chorus, intro]);
    s.run(
        ids::SELECT_SECTION,
        json!({ "sections": [chorus], "mode": "toggle" }),
    );
    assert_eq!(s.state().selected_sections, [verse, intro]);
    assert_eq!(s.steps(), 0);
    // The selected section is the one the commands act on.
    s.run(ids::SELECT_SECTION, json!({ "sections": [verse] }));
    s.one_step(ids::DUPLICATE_SECTION, json!({}), "Duplicate Section");
    let unknown = SectionId(bl_core::id::Id::new(999_999).unwrap());
    assert!(
        s.shell
            .run(ids::SELECT_SECTION, json!({ "sections": [unknown] }))
            .is_err()
    );
}

#[test]
fn the_lane_shows_from_the_view_command_and_is_saved_with_no_undo_step() {
    // Spec: arranger-track R1.
    let mut s = set(false);
    assert!(!s.state().arranger_track);
    let saved = serde_json::to_value(s.doc()).unwrap();
    assert!(saved["settings"].get("arranger_track").is_none());
    let state: ArrangementState =
        serde_json::from_value(s.shell.run(ids::ARRANGER_TRACK, json!({})).unwrap()).unwrap();
    assert!(state.arranger_track);
    assert_eq!(s.shell.switch_state(ids::ARRANGER_TRACK), Some(true));
    assert_eq!(s.steps(), 0);
    // An edit and its undo leave the lane as it was.
    s.edit(edit_ids::INSERT_SILENCE, span(1, 2));
    s.shell.run("edit.undo", json!({})).unwrap();
    assert!(s.state().arranger_track);
    let saved = serde_json::to_value(s.doc()).unwrap();
    assert_eq!(saved["settings"]["arranger_track"], true);
    s.shell
        .run(ids::ARRANGER_TRACK, json!({ "on": false }))
        .unwrap();
    assert!(!s.state().arranger_track);
}

#[test]
fn random_section_edits_among_time_edits_keep_the_rules_and_undo_exactly() {
    // Spec: arranger-track R16. No two sections overlap, no two clips of a
    // track overlap, and undo and redo give the set back exactly.
    let mut s = set(true);
    let mut rng = Lcg(0x5ec7);
    let mut versions = vec![s.doc().clone()];
    let quarter = BAR / 4;
    for _ in 0..200 {
        let at = (rng.next(48) as i64) * quarter;
        let len = (1 + rng.next(12) as i64) * quarter;
        let sections: Vec<SectionId> = s.doc().sections().iter().map(|x| x.id).collect();
        let pick = sections
            .get(rng.next(sections.len() as u64) as usize)
            .copied();
        let section = json!({ "section": pick });
        let with = |extra: Value| {
            let mut args = section.clone();
            args.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            args
        };
        let (id, args) = match rng.next(14) {
            0 => (ids::ADD_SECTION, json!({ "start": at, "end": at + len })),
            1 => (ids::MOVE_SECTION, with(json!({ "to": at }))),
            2 => (ids::MOVE_SECTION, with(json!({ "with": "earlier" }))),
            3 => (ids::MOVE_SECTION, with(json!({ "with": "later" }))),
            4 => (ids::COPY_SECTION, with(json!({ "to": at }))),
            5 => (ids::DUPLICATE_SECTION, section.clone()),
            6 => (ids::DELETE_SECTION, section.clone()),
            7 => (ids::REMOVE_SECTION, section.clone()),
            8 => (
                ids::RESIZE_SECTION,
                with(json!({ "start": at, "end": at + len })),
            ),
            9 => (
                edit_ids::INSERT_SILENCE,
                json!({ "start": at, "end": at + len }),
            ),
            10 => (
                edit_ids::DELETE_TIME,
                json!({ "start": at, "end": at + len }),
            ),
            11 => (edit_ids::CUT_TIME, json!({ "start": at, "end": at + len })),
            12 => (edit_ids::PASTE_TIME, json!({ "at": at })),
            _ => (ids::SECTIONS_FROM_LOCATORS, json!({})),
        };
        let before = s.doc().clone();
        let steps = s.steps();
        match s.shell.run(id, args) {
            Ok(_) if s.steps() == steps + 1 => versions.push(s.doc().clone()),
            Ok(_) | Err(_) => assert_eq!(*s.doc(), before, "{id} changed nothing"),
        }
        let doc = s.doc();
        assert!(doc.check_invariants().is_ok(), "{id}");
        for pair in doc.sections().windows(2) {
            assert!(pair[0].end() <= pair[1].start, "{id} overlapped sections");
        }
        for track in doc.tracks() {
            let mut lanes: Vec<Vec<&Clip>> = vec![track.clips().collect()];
            lanes.extend(track.take_lanes().map(|l| l.clips.iter().collect()));
            for mut clips in lanes {
                clips.sort_by_key(|c| c.start);
                for pair in clips.windows(2) {
                    assert!(pair[0].end() <= pair[1].start, "{id} overlapped clips");
                }
            }
        }
    }
    for version in versions.iter().rev().skip(1) {
        s.shell.run("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc(), version);
    }
    for version in versions.iter().skip(1) {
        s.shell.run("edit.redo", json!({})).unwrap();
        assert_eq!(s.doc(), version);
    }
}

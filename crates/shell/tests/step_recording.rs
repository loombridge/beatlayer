//! The MIDI Note Editor's Preview switch, its previews, step recording and
//! Step Input Mode (WP-6.5; the step recording spec), driven headless
//! through `Shell::run`, the computer MIDI keyboard's keys and the test
//! clock, as the keys and the menu drive them.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bl_commands::Context;
use bl_core::clock::TestClock;
use bl_core::midi::MidiSource;
use bl_core::time::Ticks;
use bl_model::{Clip, ClipData, ClipId, Command, Document, Editor, MidiData, Note, TrackId};
use bl_shell::Shell;
use bl_shell::arrangement::TransportView;
use bl_shell::audio::AudioHost;
use bl_shell::clip_view::notes_in_order;
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::note_commands::{ClipViewState, StepRecordResult};
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

/// An empty MIDI clip on the first MIDI track, selected, with the track
/// armed, Preview on, the computer MIDI keyboard on and the insert marker
/// at the clip's start: everything step recording needs.
struct Stepper {
    shell: Shell,
    clip: ClipId,
    track: TrackId,
    step: i64,
}

fn shell_with(prefs: Preferences, config: Option<PathBuf>) -> (Shell, ClipId, TrackId) {
    let mut editor = factory_default_set();
    let track = editor.document().top_level().next().unwrap();
    let clip: ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(4 * BAR),
                ClipData::Midi(MidiData::new(Vec::<Note>::new())),
            ),
        })
        .unwrap();
    editor.clear_history().unwrap();
    let mut shell = Shell::new(
        prefs,
        config,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Steps.blset" }))
        .unwrap();
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    (shell, clip, track)
}

fn stepper() -> Stepper {
    let (mut shell, clip, track) = shell_with(Preferences::default(), None);
    shell.use_test_clock(TestClock::new());
    let step = shell
        .arrangement()
        .grid_ticks(shell.editor().document())
        .get();
    let mut stepper = Stepper {
        shell,
        clip,
        track,
        step,
    };
    stepper.run("mixer.set_arm", json!({ "track": track, "arm": true }));
    stepper.run("clip_view.set_preview", json!({ "on": true }));
    stepper.run("midi.toggle_computer_keyboard", json!({ "on": true }));
    stepper.run(
        "clip_view.set_insert_marker",
        json!({ "clip": clip, "beat": 0 }),
    );
    // The frame that turns the input's copy on.
    stepper.shell.poll();
    stepper
}

impl Stepper {
    fn run(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .run(id, args)
            .unwrap_or_else(|error| panic!("{id}: {error}"))
    }

    fn key(&mut self, key: &str, pressed: bool) {
        self.run(
            "midi.computer_keyboard_key",
            json!({ "key": key, "pressed": pressed }),
        );
    }

    fn doc(&self) -> &Document {
        self.shell.editor().document()
    }

    /// The clip's notes as `(key, start, length, velocity)`, in order.
    fn notes(&self) -> Vec<(u8, i64, i64, u8)> {
        match &self.doc().clip(self.clip).unwrap().data {
            ClipData::Midi(midi) => notes_in_order(midi)
                .iter()
                .map(|n| (n.key, n.start.get(), n.length.get(), n.velocity))
                .collect(),
            ClipData::Audio(_) => panic!("the clip is MIDI"),
        }
    }

    fn marker(&mut self) -> Option<i64> {
        let state: ClipViewState =
            serde_json::from_value(self.run("view.state", json!({}))["clip_view"].clone()).unwrap();
        state.insert_marker.map(Ticks::get)
    }

    fn right(&mut self) -> StepRecordResult {
        serde_json::from_value(self.run("note.step_record", json!({}))).unwrap()
    }

    fn left(&mut self) -> StepRecordResult {
        serde_json::from_value(self.run("note.step_record", json!({ "back": true }))).unwrap()
    }

    fn undo_steps(&self) -> Vec<String> {
        self.shell
            .editor()
            .undo_history()
            .map(str::to_owned)
            .collect()
    }
}

// Spec: step-recording R3, R4, R5, R10
#[test]
fn holding_two_keys_and_pressing_right_twice_adds_two_notes_two_steps_long() {
    let mut s = stepper();
    let step = s.step;
    let before = s.undo_steps().len();
    // A and D are C3 and E3 on the computer MIDI keyboard, at velocity 100.
    s.key("A", true);
    s.key("D", true);
    let first = s.right();
    assert_eq!((first.added, first.lengthened), (2, 0));
    assert_eq!(s.notes(), [(60, 0, step, 100), (64, 0, step, 100)]);
    assert_eq!(s.marker(), Some(step));
    let second = s.right();
    assert_eq!((second.added, second.lengthened), (0, 2));
    assert_eq!(s.notes(), [(60, 0, 2 * step, 100), (64, 0, 2 * step, 100)]);
    assert_eq!(s.marker(), Some(2 * step));
    // Each press is one undo step, named Step Record.
    let steps = s.undo_steps();
    assert_eq!(steps.len() - before, 2);
    assert_eq!(&steps[..2], ["Step Record", "Step Record"]);
}

// Spec: step-recording R5, R7
#[test]
fn letting_one_key_go_lengthens_only_the_other_and_a_new_key_is_added_at_the_marker() {
    let mut s = stepper();
    let step = s.step;
    s.key("A", true);
    s.key("D", true);
    s.right();
    s.right();
    s.key("A", false);
    s.right();
    assert_eq!(s.notes(), [(60, 0, 2 * step, 100), (64, 0, 3 * step, 100)]);
    // F joins while D is still held: it starts on the marker, with no
    // record quantisation, and D grows again.
    s.key("F", true);
    s.right();
    assert_eq!(
        s.notes(),
        [
            (60, 0, 2 * step, 100),
            (64, 0, 4 * step, 100),
            (65, 3 * step, step, 100)
        ]
    );
    assert_eq!(s.marker(), Some(4 * step));
}

// Spec: step-recording R6
#[test]
fn left_shortens_the_notes_just_recorded_and_then_removes_them() {
    let mut s = stepper();
    let step = s.step;
    s.key("D", true);
    s.right();
    s.right();
    let shorter = s.left();
    assert_eq!((shorter.shortened, shorter.removed), (1, 0));
    assert_eq!(s.notes(), [(64, 0, step, 100)]);
    assert_eq!(s.marker(), Some(step));
    let gone = s.left();
    assert_eq!((gone.shortened, gone.removed), (0, 1));
    assert_eq!(s.notes(), []);
    assert_eq!(s.marker(), Some(0));
    // With nothing left to hold, Left moves the marker alone, and it stays
    // inside the clip.
    s.key("D", false);
    s.left();
    assert_eq!(s.marker(), Some(0));
}

// Spec: step-recording R4
#[test]
fn right_with_nothing_held_leaves_a_rest() {
    let mut s = stepper();
    let step = s.step;
    let before = s.undo_steps().len();
    let rest = s.right();
    assert_eq!((rest.added, rest.undo_step), (0, false));
    assert_eq!(s.marker(), Some(step));
    assert_eq!(s.notes(), []);
    assert_eq!(s.undo_steps().len(), before, "a rest is no undo step");
    // The next notes start after the rest.
    s.key("A", true);
    s.right();
    assert_eq!(s.notes(), [(60, step, step, 100)]);
}

// Spec: step-recording R3, R7
#[test]
fn nothing_step_records_with_the_transport_running_preview_off_or_the_track_disarmed() {
    let mut s = stepper();
    s.key("A", true);
    let refused = |s: &mut Stepper| {
        assert!(s.shell.run("note.step_record", json!({})).is_err());
        assert_eq!(s.notes(), []);
        assert!(!s.shell.step_recording());
    };
    s.shell.set_transport(TransportView {
        playing: true,
        ..TransportView::default()
    });
    refused(&mut s);
    s.shell.set_transport(TransportView::default());
    assert!(s.shell.step_recording());

    s.run("clip_view.set_preview", json!({ "on": false }));
    refused(&mut s);
    s.run("clip_view.set_preview", json!({ "on": true }));

    s.run("mixer.set_arm", json!({ "track": s.track, "arm": false }));
    refused(&mut s);
    s.run("mixer.set_arm", json!({ "track": s.track, "arm": true }));
    s.right();
    assert_eq!(s.notes().len(), 1);
}

// Spec: step-recording R13
#[test]
fn step_record_by_id_takes_the_held_notes_as_keys_and_velocities() {
    let mut s = stepper();
    let step = s.step;
    let held = json!({ "held": [{ "key": 48, "velocity": 90 }, { "key": 55, "velocity": 70 }] });
    let done: StepRecordResult = serde_json::from_value(s.run("note.step_record", held)).unwrap();
    assert_eq!((done.added, done.undo_step), (2, true));
    assert_eq!(s.notes(), [(48, 0, step, 90), (55, 0, step, 70)]);
    // The same keys again make them longer; an empty list leaves a rest.
    s.run(
        "note.step_record",
        json!({ "held": [{ "key": 48, "velocity": 90 }] }),
    );
    assert_eq!(s.notes(), [(48, 0, 2 * step, 90), (55, 0, step, 70)]);
    s.run("note.step_record", json!({ "held": [] }));
    assert_eq!(s.marker(), Some(3 * step));
    // The keys must be real ones.
    assert!(
        s.shell
            .run(
                "note.step_record",
                json!({ "held": [{ "key": 200, "velocity": 90 }] })
            )
            .is_err()
    );
}

// Spec: step-recording R3, R8
#[test]
fn a_preview_is_not_a_held_key() {
    let mut s = stepper();
    s.run("note.preview", json!({ "key": 70, "on": true }));
    s.shell.poll();
    s.right();
    assert_eq!(s.notes(), [], "the preview was heard by nothing that steps");
    s.run("note.preview", json!({ "key": 70, "on": false }));
}

// Spec: step-recording R9, R10
#[test]
fn step_input_mode_arms_the_track_focuses_the_clip_view_and_switches_the_keyboard() {
    let (mut shell, _, track) = shell_with(Preferences::default(), None);
    assert!(shell.can_invoke("clip_view.step_input_mode", &json!({})));
    assert!(!shell.computer_keyboard_on());
    assert!(!shell.editor().document().track(track).unwrap().mixer().arm);
    let before = shell.editor().undo_history().len();
    shell.run("clip_view.step_input_mode", json!({})).unwrap();
    assert!(shell.editor().document().track(track).unwrap().mixer().arm);
    assert!(shell.computer_keyboard_on());
    assert!(shell.active_contexts().contains(&Context::NoteEditor));
    assert_eq!(
        shell.editor().undo_history().len() - before,
        1,
        "arming is the one undo step"
    );
    // It is a command, never ticked, and switches the keyboard to the
    // other state each time.
    assert_eq!(shell.switch_state("clip_view.step_input_mode"), None);
    shell.run("clip_view.step_input_mode", json!({})).unwrap();
    assert!(!shell.computer_keyboard_on());
    // It does not switch Preview on.
    assert!(!shell.clip_view().preview());
    // With no MIDI clip shown, it is not available.
    shell
        .run("arrangement.select_clips", json!({ "clips": [] }))
        .unwrap();
    let mut empty = Shell::headless();
    assert!(!empty.can_invoke("clip_view.step_input_mode", &json!({})));
    assert!(empty.run("clip_view.step_input_mode", json!({})).is_err());
}

// Spec: step-recording R1
#[test]
fn the_preview_switch_is_a_preference_that_survives_a_restart() {
    let config = std::env::temp_dir().join(format!("bl-step-prefs-{}", std::process::id()));
    std::fs::create_dir_all(&config).unwrap();
    let (mut shell, _, _) = shell_with(Preferences::default(), Some(config.clone()));
    assert_eq!(shell.switch_state("clip_view.set_preview"), Some(false));
    let history = shell.editor().undo_history().len();
    let state: ClipViewState =
        serde_json::from_value(shell.run("clip_view.set_preview", json!({})).unwrap()).unwrap();
    assert!(state.preview, "with no argument the switch flips");
    assert_eq!(shell.switch_state("clip_view.set_preview"), Some(true));
    assert_eq!(shell.editor().undo_history().len(), history, "no undo step");
    assert!(Preferences::load(&config).unwrap().note_preview);

    let (restarted, _, _) = shell_with(Preferences::load(&config).unwrap(), None);
    assert_eq!(restarted.switch_state("clip_view.set_preview"), Some(true));
    assert!(restarted.clip_view().preview());
    std::fs::remove_file(config.join("preferences.json")).ok();
}

// Spec: step-recording R2
#[test]
fn a_preview_plays_into_the_clips_track_until_released_and_never_hangs() {
    let (mut shell, clip, track) = shell_with(Preferences::default(), None);
    let clock = TestClock::new();
    shell.use_test_clock(clock.clone());
    let mut queue = shell.midi().sender().attach(64);
    let heard = |queue: &mut rtrb::Consumer<bl_core::midi::MidiInputEvent>| {
        std::iter::from_fn(|| queue.pop().ok())
            .map(|e| {
                (
                    e.source,
                    e.message.status(),
                    e.message.data1(),
                    e.message.data2(),
                )
            })
            .collect::<Vec<_>>()
    };
    let source = MidiSource::Preview(track.0);

    // Preview off: a press is turned away.
    assert!(
        shell
            .run("note.preview", json!({ "key": 64, "on": true }))
            .is_err()
    );
    assert_eq!(heard(&mut queue), []);

    shell
        .run("clip_view.set_preview", json!({ "on": true }))
        .unwrap();
    shell
        .run(
            "note.preview",
            json!({ "clip": clip, "key": 64, "on": true }),
        )
        .unwrap();
    assert_eq!(heard(&mut queue), [(source, 0x90, 64, 100)]);
    shell
        .run("note.preview", json!({ "key": 64, "on": false }))
        .unwrap();
    assert_eq!(heard(&mut queue), [(source, 0x80, 64, 64)]);

    // Switching Preview off ends a note still held.
    shell
        .run(
            "note.preview",
            json!({ "key": 66, "velocity": 40, "on": true }),
        )
        .unwrap();
    shell
        .run("clip_view.set_preview", json!({ "on": false }))
        .unwrap();
    assert_eq!(
        heard(&mut queue),
        [(source, 0x90, 66, 40), (source, 0x80, 66, 64)]
    );
    // A release always works, and nothing changed in the set.
    shell
        .run("note.preview", json!({ "key": 66, "on": false }))
        .unwrap();
    assert_eq!(heard(&mut queue), []);
    assert_eq!(shell.editor().undo_history().len(), 0);
}

// Spec: step-recording R2
#[test]
fn a_note_selected_from_the_keys_sounds_at_its_own_velocity_for_250_ms() {
    let (mut shell, clip, track) = shell_with(Preferences::default(), None);
    shell
        .run(
            "note.add",
            json!({ "clip": clip, "key": 62, "start": 0, "length": 480, "velocity": 77 }),
        )
        .unwrap();
    let clock = TestClock::new();
    shell.use_test_clock(clock.clone());
    let mut queue = shell.midi().sender().attach(64);
    let heard = |queue: &mut rtrb::Consumer<bl_core::midi::MidiInputEvent>| {
        std::iter::from_fn(|| queue.pop().ok())
            .map(|e| {
                (
                    e.source,
                    e.message.status(),
                    e.message.data1(),
                    e.message.data2(),
                )
            })
            .collect::<Vec<_>>()
    };
    let source = MidiSource::Preview(track.0);
    shell
        .run("clip_view.select_notes", json!({ "notes": [] }))
        .unwrap();
    // Preview off: selecting from the keys is silent.
    shell
        .run(
            "clip_view.select_notes",
            json!({ "notes": [], "select": "nearest" }),
        )
        .unwrap();
    assert_eq!(heard(&mut queue), []);

    shell
        .run("clip_view.set_preview", json!({ "on": true }))
        .unwrap();
    shell
        .run(
            "clip_view.select_notes",
            json!({ "notes": [], "select": "nearest" }),
        )
        .unwrap();
    assert_eq!(heard(&mut queue), [(source, 0x90, 62, 77)]);
    clock.advance(Duration::from_millis(249));
    shell.poll();
    assert_eq!(heard(&mut queue), []);
    clock.advance(Duration::from_millis(1));
    shell.poll();
    assert_eq!(heard(&mut queue), [(source, 0x80, 62, 64)]);
}

// Spec: step-recording R11
#[test]
fn each_step_is_announced() {
    let mut s = stepper();
    s.key("A", true);
    s.right();
    let message = s.shell.message().unwrap().text.clone();
    assert!(message.starts_with("1 note added"), "{message}");
    s.right();
    assert!(
        s.shell
            .message()
            .unwrap()
            .text
            .starts_with("1 note made longer")
    );
    s.key("A", false);
    s.right();
    assert!(s.shell.message().unwrap().text.starts_with("rest"));
    s.run("clip_view.step_input_mode", json!({}));
    assert!(
        s.shell
            .message()
            .unwrap()
            .text
            .starts_with("Step Input Mode: track armed, computer MIDI keyboard off")
    );
}

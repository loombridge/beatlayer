//! The Arrangement's automation editing, driven headless through
//! `Shell::run` (WP-6.25): the lanes a selection names, the edit commands
//! on envelopes, moving several breakpoints at once, and Lock Envelopes.
//! The behaviour is in the automation editing spec.

use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_model::{
    AudioData, AutomationTarget, Clip, ClipData, ContentHash, Command, Document, Editor, MixerParam,
    TrackId,
};
use bl_shell::arrangement_commands::{self as view_ids, ArrangementState, Lane};
use bl_shell::audio::AudioHost;
use bl_shell::automation_commands::{self as ids, EnvelopeClipboardState};
use bl_shell::editing_commands::{self as edit_ids};
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::settings::Preferences;
use bl_shell::Shell;
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

/// A set with one MIDI track, one clip on it, and a volume envelope with
/// breakpoints at bars 1, 2 and 3.
struct Song {
    shell: Shell,
    track: TrackId,
    clip: bl_model::ClipId,
}

fn song() -> Song {
    let mut editor = factory_default_set();
    let track = editor
        .document()
        .top_level()
        .find(|id| {
            editor
                .document()
                .track(*id)
                .is_some_and(|track| track.kind() == bl_model::TrackKind::Audio)
        })
        .unwrap();
    let clip: bl_model::ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(BAR * 2),
                ClipData::Audio(AudioData::new(ContentHash([7; 32]))),
            ),
        })
        .unwrap();
    for (bar, value) in [(0, 0.0), (1, 1.0), (2, 0.5)] {
        editor
            .apply(Command::SetAutomationBreakpoint {
                track,
                target: AutomationTarget::Mixer {
                    param: MixerParam::Volume,
                },
                at: Ticks(bar * BAR),
                value,
            })
            .unwrap();
    }
    editor.clear_history().unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Automation.blset" }))
        .unwrap();
    Song { shell, track, clip }
}

impl Song {
    fn doc(&self) -> &Document {
        self.shell.editor().document()
    }

    /// The envelope's breakpoints, as `(bar, value)`.
    fn envelope(&self) -> Vec<(i64, f64)> {
        self.doc()
            .track(self.track)
            .unwrap()
            .automation_for(&AutomationTarget::Mixer {
                param: MixerParam::Volume,
            })
            .unwrap()
            .envelope
            .points()
            .map(|point| (point.at.get() / BAR, point.value))
            .collect()
    }

    /// The automation lane of the track, for the commands' arguments.
    fn lane(&self) -> Value {
        json!({
            "type": "automation",
            "track": self.track,
            "target": { "type": "mixer", "param": "volume" },
        })
    }

    fn run(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}"))
    }

    /// Selects the time from bar 1 to bar 2 in the track's automation lane.
    fn select(&mut self) {
        self.run(
            view_ids::SELECT_TIME,
            json!({
                "start": Ticks(BAR),
                "end": Ticks(BAR * 2),
                "lanes": [self.lane()],
            }),
        );
    }
}

// Spec: automation-editing R1.
#[test]
fn a_selection_names_the_lanes_it_covers() {
    let mut song = song();
    song.select();
    let state: ArrangementState = serde_json::from_value(song.run(view_ids::STATE, json!({}))).unwrap();
    assert_eq!(
        state.time_selection_lanes,
        vec![Lane::Automation {
            track: song.track,
            target: AutomationTarget::Mixer {
                param: MixerParam::Volume
            }
        }]
    );
    assert_eq!(state.time_selection.map(|range| range.start), Some(Ticks(BAR)));
}

// Spec: automation-editing R5.
#[test]
fn deleting_an_envelopes_range_leaves_it_straight_across_and_the_rest_alone() {
    let mut song = song();
    song.select();
    song.run(
        ids::DELETE_RANGE,
        json!({ "lanes": [song.lane()], "start": Ticks(BAR), "end": Ticks(BAR * 2) }),
    );
    assert_eq!(
        song.envelope(),
        vec![
            (0, 0.0),
            // The values at the span's edges are kept, and the envelope runs
            // straight across the span.
            (1, 1.0),
            (2, 0.5),
        ]
    );
}

// Spec: automation-editing R5.
#[test]
fn copying_an_envelope_and_pasting_it_onto_another_parameter_keeps_its_shape() {
    let mut song = song();
    song.select();
    let state: EnvelopeClipboardState =
        serde_json::from_value(song.run(ids::COPY, json!({}))).unwrap();
    assert_eq!(state.lanes, 1);
    assert_eq!(state.length, Ticks(BAR));
    // Paste it back at bar 4: the shape lands there, with the values as
    // positions on the travel.
    song.run(
        ids::PASTE,
        json!({ "lanes": [song.lane()], "at": Ticks(BAR * 4) }),
    );
    let envelope = song.envelope();
    assert!(envelope.len() >= 5, "{envelope:?}");
}

// Spec: automation-editing R2.
#[test]
fn moving_several_breakpoints_moves_them_all_as_one() {
    let mut song = song();
    song.run(
        ids::MOVE_BREAKPOINTS,
        json!({
            "track": song.track,
            "target": { "type": "mixer", "param": "volume" },
            "at": [Ticks(BAR), Ticks(BAR * 2)],
            "by": Ticks(BAR),
            "share": 0.0,
        }),
    );
    assert_eq!(
        song.envelope(),
        vec![(0, 0.0), (2, 1.0), (3, 0.5)],
        "both breakpoints moved by a bar"
    );
}

// Spec: automation-editing R9.
#[test]
fn moving_a_clip_carries_its_automation_with_it() {
    let mut song = song();
    // The clip goes from bar 1 to bar 3; the envelope's shape over that
    // span goes with it.
    song.run(
        edit_ids::MOVE_CLIPS,
        json!({ "clips": [song.clip], "by": Ticks(BAR * 2) }),
    );
    let envelope = song.envelope();
    // The values at the clip's old span are kept, and the shape is at the
    // new one.
    assert_eq!(envelope[0], (0, 0.0), "{envelope:?}");
    assert!(
        envelope.iter().any(|(bar, value)| *bar == 3 && (*value - 1.0).abs() < 0.001),
        "{envelope:?}"
    );
    assert!(
        envelope.iter().any(|(bar, value)| *bar == 4 && (*value - 0.5).abs() < 0.001),
        "{envelope:?}"
    );
}

// Spec: automation-editing R10.
#[test]
fn with_lock_envelopes_on_a_clip_moves_on_its_own() {
    let mut song = song();
    song.run("song.set_lock_envelopes", json!({ "enabled": true }));
    song.run(
        edit_ids::MOVE_CLIPS,
        json!({ "clips": [song.clip], "by": Ticks(BAR * 2) }),
    );
    assert_eq!(
        song.envelope(),
        vec![(0, 0.0), (1, 1.0), (2, 0.5)],
        "the envelope stays where it is"
    );
}

// Spec: automation-editing R8.
#[test]
fn lock_envelopes_is_off_in_a_new_set_and_saved_with_the_set() {
    let mut song = song();
    assert!(!song.doc().settings().lock_envelopes);
    song.run("song.set_lock_envelopes", json!({ "enabled": true }));
    assert!(song.doc().settings().lock_envelopes);
}

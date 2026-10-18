//! The Clip View's tabs and the clip it shows, driven headless through
//! `Shell::run` (WP-3.16; D17): the Notes and Envelopes tabs and their
//! commands, the control the Envelopes tab shows, and the Clip View
//! following the clip most recently selected in the Arrangement or in a
//! Session slot. The behaviour is in the piano roll and clip envelopes
//! specs.

use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_model::{
    AutomationTarget, Clip, ClipData, ClipId, Command, Editor, MidiData, MixerParam, SceneId,
    TrackId,
};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::note_commands::{ClipTab, ClipViewState};
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
    track: TrackId,
    scene: SceneId,
}

/// A set with a MIDI clip in the Arrangement on the first MIDI track, and
/// the Clip View open.
fn song() -> Song {
    let mut editor = factory_default_set();
    let track: TrackId = editor.document().top_level().next().unwrap();
    let arranged: ClipId = editor.new_id();
    let clip = Clip::new(
        arranged,
        Ticks::ZERO,
        Ticks(4 * BAR),
        ClipData::Midi(MidiData::new(Vec::new())),
    );
    editor.apply(Command::CreateClip { track, clip }).unwrap();
    editor.clear_history().unwrap();
    let scene = editor.document().scenes().next().unwrap().id;
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Tabs.blset" }))
        .unwrap();
    shell.run("view.toggle_clip_view", json!({})).unwrap();
    Song {
        shell,
        arranged,
        track,
        scene,
    }
}

impl Song {
    fn view(&mut self, id: &str, args: Value) -> ClipViewState {
        serde_json::from_value(
            self.shell
                .run(id, args)
                .unwrap_or_else(|e| panic!("{id} failed: {e}")),
        )
        .unwrap()
    }

    /// The clip the Clip View shows, without changing whether it shows.
    fn clip(&mut self) -> Option<ClipId> {
        let state: bl_shell::commands::ViewState =
            serde_json::from_value(self.shell.run("view.state", json!({})).unwrap()).unwrap();
        state.clip_view.clip
    }

    fn select_slot(&mut self) {
        self.shell
            .run(
                "session.select",
                json!({ "track": self.track, "scene": self.scene }),
            )
            .unwrap();
    }

    fn select_arranged(&mut self) {
        self.shell
            .run(
                "arrangement.select_clips",
                json!({ "clips": [self.arranged] }),
            )
            .unwrap();
    }

    /// The Session clip in the slot, after `session.insert_empty_midi_clip`.
    fn session_clip(&self) -> ClipId {
        self.shell
            .editor()
            .document()
            .slot(self.track, self.scene)
            .expect("the slot holds a clip")
            .id
    }
}

// Spec: piano-roll R22. The tab commands change only the view: Notes
// and Envelopes by name, and the next tab, round to the first, when `tab` is
// left out. The state reads the tab back.
#[test]
fn the_tab_commands_show_and_cycle_the_clip_views_tabs() {
    let mut song = song();
    let steps = song.shell.editor().undo_history().len();
    assert_eq!(
        song.view("view.clip_tab", json!({ "tab": "notes" })).tab,
        ClipTab::Notes
    );
    assert_eq!(
        song.view("view.clip_tab", json!({ "tab": "envelopes" }))
            .tab,
        ClipTab::Envelopes
    );
    assert_eq!(song.view("view.clip_tab", json!({})).tab, ClipTab::Notes);
    assert_eq!(
        song.view("view.clip_tab", json!({})).tab,
        ClipTab::Envelopes
    );
    assert_eq!(
        song.shell.editor().undo_history().len(),
        steps,
        "a tab is view state, not an undo step"
    );
    assert!(
        song.shell
            .run("view.clip_tab", json!({ "tab": "mpe" }))
            .is_err(),
        "the MPE tab comes with its own package"
    );
}

// Spec: piano-roll R22. The tab decides which editor's keys apply: the
// Notes tab is the note editor, the Envelopes tab the envelope editor.
#[test]
fn the_envelopes_tab_swaps_the_note_editors_keys_for_the_envelope_editors() {
    use bl_commands::Context;
    let mut song = song();
    song.shell.set_focused_area(bl_shell::Area::Detail);
    let contexts = song.shell.active_contexts();
    assert!(contexts.contains(&Context::NoteEditor) && !contexts.contains(&Context::Envelope));
    song.view("view.clip_tab", json!({ "tab": "envelopes" }));
    let contexts = song.shell.active_contexts();
    assert!(contexts.contains(&Context::ClipView) && contexts.contains(&Context::Envelope));
    assert!(!contexts.contains(&Context::NoteEditor));
}

// Spec: clip-envelopes R8. `view.clip_envelope` chooses the parameter the
// tab shows, from the shown clip's track's; one the track doesn't have is
// refused, and `null` shows none.
#[test]
fn the_envelope_chooser_names_a_parameter_of_the_shown_clips_track() {
    let mut song = song();
    song.select_arranged();
    let pan = AutomationTarget::Mixer {
        param: MixerParam::Pan,
    };
    let state = song.view("view.clip_envelope", json!({ "target": pan }));
    assert_eq!(state.envelope, Some(pan));
    // A send must feed a return track, and this is a MIDI track.
    let missing = AutomationTarget::Send { to: song.track };
    assert!(
        song.shell
            .run("view.clip_envelope", json!({ "target": missing }))
            .is_err()
    );
    assert_eq!(song.clip(), Some(song.arranged));
    let state = song.view("view.clip_envelope", json!({ "target": null }));
    assert_eq!(state.envelope, None);
}

// Spec: piano-roll R1. The Clip View shows the clip most recently selected,
// from either view: a Session slot's replaces the Arrangement's, the
// Arrangement's replaces it again, and selecting the slot that is already
// selected shows its clip once more. The note commands then edit it.
#[test]
fn the_clip_view_follows_the_clip_most_recently_selected_in_either_view() {
    let mut song = song();
    song.select_arranged();
    assert_eq!(song.clip(), Some(song.arranged));
    song.shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": song.track, "scene": song.scene }),
        )
        .unwrap();
    let session = song.session_clip();
    song.select_slot();
    assert_eq!(song.clip(), Some(session));
    song.select_arranged();
    assert_eq!(song.clip(), Some(song.arranged));
    song.select_slot();
    assert_eq!(song.clip(), Some(session), "the slot was still selected");
    song.shell
        .run(
            "note.add",
            json!({ "key": 60, "start": 0, "length": Ticks::QUARTER }),
        )
        .unwrap();
    let clip = song.shell.editor().document().clip(session).unwrap();
    assert!(
        matches!(&clip.data, ClipData::Midi(midi) if midi.notes.len() == 1),
        "the Notes tab edits the Session clip"
    );
}

// Spec: clip-envelopes R5, R8. A Session clip takes envelope edits like an
// Arrangement clip, and the state names the control the tab shows.
#[test]
fn a_session_clip_takes_envelope_edits_by_command_id() {
    let mut song = song();
    song.shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": song.track, "scene": song.scene }),
        )
        .unwrap();
    song.select_slot();
    let session = song.session_clip();
    let pan = AutomationTarget::Mixer {
        param: MixerParam::Pan,
    };
    song.view("view.clip_envelope", json!({ "target": pan }));
    song.shell
        .run(
            "clip.set_envelope_breakpoint",
            json!({ "clip": session, "target": pan, "at": 0, "value": 0.5 }),
        )
        .unwrap();
    let clip = song.shell.editor().document().clip(session).unwrap();
    assert_eq!(clip.envelope(&pan).unwrap().envelope.points().count(), 1);
    assert_eq!(song.clip(), Some(session));
}

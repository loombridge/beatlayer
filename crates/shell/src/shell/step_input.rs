//! The MIDI Note Editor's Preview switch, its previews, step recording and
//! Step Input Mode (WP-6.5; the step recording spec, R1 to R11): the
//! shell's side of them. The notes a step adds are `clip_view`'s; what the
//! shell does is read the keys held on the MIDI input, play previews into a
//! track's chain, and arm, focus and switch the keyboard for Step Input Mode.

use std::time::Duration;

use bl_core::midi::MidiMessage;
use bl_shell_spec::notes::{
    self as note_ids, HeldKey, PreviewNoteArgs, SetPreviewArgs, StepRecordArgs, StepRecordResult,
};
use serde_json::json;

use super::*;

/// How long a note selected from the keys sounds as a preview (Spec:
/// step-recording R2).
const KEY_PREVIEW: Duration = Duration::from_millis(250);

/// The velocity a key clicked in the piano ruler plays at (R2).
const KEY_VELOCITY: u8 = 100;

/// A preview that is sounding on a track.
#[derive(Clone, Copy, Debug)]
struct Sounding {
    track: TrackId,
    key: u8,
    /// When it ends by itself; none while a button holds it.
    ends: Option<std::time::Instant>,
}

/// The keys held on the MIDI input and the previews sounding.
#[derive(Debug, Default)]
pub(super) struct StepInput {
    held: Vec<HeldKey>,
    sounding: Vec<Sounding>,
}

impl Shell {
    /// The track of the MIDI clip the editor shows.
    pub(super) fn editor_track(&self) -> Option<TrackId> {
        let doc = self.editor.document();
        let clip = self.clip_view.shown_clip(doc, &self.arrangement)?;
        doc.clip_track(clip)
    }

    /// Whether the editor step-records: the track of the clip it shows is
    /// armed, Preview is on and the transport is stopped (Spec:
    /// step-recording R3).
    #[must_use]
    pub fn step_recording(&self) -> bool {
        self.clip_view.preview()
            && !self.playing()
            && self
                .editor_track()
                .and_then(|track| self.editor.document().track(track))
                .is_some_and(|track| track.mixer().arm)
    }

    /// The keys held on the MIDI input, as the last frame heard them while
    /// step recording.
    #[must_use]
    pub fn held_keys(&self) -> &[HeldKey] {
        &self.step.held
    }

    /// Takes the note messages the MIDI input copied for the main thread
    /// into the keys held (R3, R8).
    // shortcut: the copy carries no source, so every input's keys count as
    // held, not only those the clip's track takes (its MIDI From and
    // channel).
    pub(super) fn hear_held(&mut self, heard: &[MidiMessage]) {
        for message in heard {
            let key = message.data1();
            if message.is_note_on() {
                let velocity = message.data2();
                match self.step.held.iter_mut().find(|h| h.key == key) {
                    Some(held) => held.velocity = velocity,
                    None => self.step.held.push(HeldKey { key, velocity }),
                }
            } else if message.is_note_off() {
                self.step.held.retain(|h| h.key != key);
                self.clip_view.step_release(key);
            }
        }
    }

    /// Forgets the keys held, as when step recording stops.
    pub(super) fn forget_held(&mut self) {
        self.step.held.clear();
    }

    /// Plays or ends a note on `track`'s chain, through the MIDI input's
    /// queue as a preview, which no track's input takes (R2). `hold` ends
    /// it by itself after that long.
    fn sound(&mut self, track: TrackId, key: u8, velocity: u8, on: bool, hold: Option<Duration>) {
        let source = bl_core::midi::MidiSource::Preview(track.0);
        let was = self
            .step
            .sounding
            .iter()
            .position(|s| s.track == track && s.key == key);
        if let Some(at) = was {
            self.step.sounding.remove(at);
            self.midi.send_as(source, MidiMessage::note_off(0, key));
        }
        if on {
            let ends = hold.map(|hold| self.midi.now() + hold);
            self.step.sounding.push(Sounding { track, key, ends });
            self.midi
                .send_as(source, MidiMessage::note_on(0, key, velocity));
        }
    }

    /// Ends every preview sounding.
    pub(super) fn end_previews(&mut self) {
        for s in std::mem::take(&mut self.step.sounding) {
            self.midi.send_as(
                bl_core::midi::MidiSource::Preview(s.track.0),
                MidiMessage::note_off(0, s.key),
            );
        }
    }

    /// Ends the previews whose time is up (R2).
    pub(super) fn expire_previews(&mut self) {
        let now = self.midi.now();
        let (over, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.step.sounding)
            .into_iter()
            .partition(|s| s.ends.is_some_and(|ends| ends <= now));
        self.step.sounding = keep;
        for s in over {
            self.midi.send_as(
                bl_core::midi::MidiSource::Preview(s.track.0),
                MidiMessage::note_off(0, s.key),
            );
        }
    }

    /// Plays the note a keyboard selection reached at its own velocity, for
    /// 250 ms, when Preview is on (R2).
    pub(super) fn preview_selected(&mut self) {
        if !self.clip_view.preview() {
            return;
        }
        let doc = self.editor.document();
        let Some(clip) = self.clip_view.shown_clip(doc, &self.arrangement) else {
            return;
        };
        let Some(track) = doc.clip_track(clip) else {
            return;
        };
        let Some(bl_model::ClipData::Midi(midi)) = doc.clip(clip).map(|c| &c.data) else {
            return;
        };
        let ordered = clip_view::notes_in_order(midi);
        let selected = self.clip_view.selected_notes(doc, &self.arrangement);
        let Some(note) = selected
            .iter()
            .max()
            .and_then(|place| ordered.get(*place as usize))
        else {
            return;
        };
        let (key, velocity) = (note.key, note.velocity.clamp(1, 127));
        self.sound(track, key, velocity, true, Some(KEY_PREVIEW));
    }

    /// Runs the Preview switch, a preview, step recording and Step Input
    /// Mode (R1, R2, R4 to R6, R9).
    pub(super) fn run_step_input(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let state = |shell: &Self| {
            to_value(
                &shell
                    .clip_view
                    .state(shell.editor.document(), &shell.arrangement),
            )
        };
        match id {
            note_ids::SET_PREVIEW => {
                let SetPreviewArgs { on } = Self::args(id, args)?;
                let on = on.unwrap_or(!self.clip_view.preview());
                self.clip_view.set_preview(on);
                self.prefs.note_preview = on;
                self.save_preferences();
                if !on {
                    self.end_previews();
                }
                self.touch();
                Ok(state(self))
            }
            note_ids::PREVIEW_NOTE => {
                let PreviewNoteArgs {
                    clip,
                    key,
                    velocity,
                    on,
                } = Self::args(id, args)?;
                let invalid = |message: &str| ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: message.to_owned(),
                };
                if key > 127 {
                    return Err(invalid("`key` must be 127 or less; 60 is middle C"));
                }
                let velocity = velocity.unwrap_or(KEY_VELOCITY);
                if !(1..=127).contains(&velocity) {
                    return Err(invalid("`velocity` must be from 1 to 127"));
                }
                let doc = self.editor.document();
                let clip = clip
                    .or_else(|| self.clip_view.shown_clip(doc, &self.arrangement))
                    .ok_or_else(|| ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "no MIDI clip is shown in the Clip View".to_owned(),
                    })?;
                let track = doc
                    .clip_track(clip)
                    .ok_or_else(|| invalid("no clip has that ID"))?;
                // A release always works, so no note is left sounding.
                if on && !self.clip_view.preview() {
                    return Err(ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "the MIDI Note Editor's Preview switch is off".to_owned(),
                    });
                }
                self.sound(track, key, velocity, on, None);
                Ok(state(self))
            }
            note_ids::STEP_RECORD => self.run_step_record(args),
            note_ids::STEP_INPUT_MODE => {
                Self::args::<NoArgs>(id, args)?;
                let unavailable = |reason: &str| ShellError::Unavailable {
                    command: id.to_owned(),
                    reason: reason.to_owned(),
                };
                let track = self
                    .editor_track()
                    .ok_or_else(|| unavailable("no MIDI clip is shown in the Clip View"))?;
                self.run("mixer.set_arm", json!({ "track": track, "arm": true }))?;
                let keyboard = self.run("midi.toggle_computer_keyboard", json!({}))?;
                self.clip_view.show();
                self.focused_window = WindowKind::Main;
                self.focused_area = Area::Detail;
                self.value_focused = false;
                let on = keyboard["on"].as_bool().unwrap_or(false);
                self.set_message(
                    format!(
                        "Step Input Mode: track armed, computer MIDI keyboard {}, Clip View has \
                         the focus",
                        if on { "on" } else { "off" }
                    ),
                    false,
                );
                Ok(state(self))
            }
            other => Err(ShellError::UnknownCommand(other.to_owned())),
        }
    }

    /// Steps forward or back with the keys held (R3 to R7, R11).
    fn run_step_record(&mut self, args: Value) -> Result<Value, ShellError> {
        let id = note_ids::STEP_RECORD;
        let mut args: StepRecordArgs = Self::args(id, args)?;
        if !self.clip_view.preview() || !self.step_armed() || self.playing() {
            return Err(ShellError::Unavailable {
                command: id.to_owned(),
                reason: "step recording needs the clip's track armed, the MIDI Note Editor's \
                         Preview switch on and the transport stopped"
                    .to_owned(),
            });
        }
        if args.held.is_none() {
            // The keys held now: take what the input copied since the last
            // frame first.
            self.learn_midi();
            args.held = Some(self.step.held.clone());
        }
        let value = self.run_note_edit(id, to_value(&args))?;
        if let Ok(done) = serde_json::from_value::<StepRecordResult>(value.clone()) {
            self.set_message(announce(&done, args.back), false);
        }
        Ok(value)
    }

    /// Whether the clip the editor shows is on an armed track.
    fn step_armed(&self) -> bool {
        self.editor_track()
            .and_then(|track| self.editor.document().track(track))
            .is_some_and(|track| track.mixer().arm)
    }
}

/// What a step says to a screen reader (R11): the notes added, made longer
/// or shorter or removed, and where the insert marker now stands.
fn announce(done: &StepRecordResult, back: bool) -> String {
    let plural = |n: u32| if n == 1 { "note" } else { "notes" };
    let mut parts = Vec::new();
    for (n, what) in [
        (done.added, "added"),
        (done.lengthened, "made longer"),
        (done.shortened, "made shorter"),
        (done.removed, "removed"),
    ] {
        if n > 0 {
            parts.push(format!("{n} {} {what}", plural(n)));
        }
    }
    if parts.is_empty() {
        parts.push(if back { "step back" } else { "rest" }.to_owned());
    }
    let beats = done.insert_marker.get() as f64 / bl_core::time::TICKS_PER_QUARTER as f64;
    format!(
        "{}. Insert marker at beat {:.2}",
        parts.join(", "),
        beats + 1.0
    )
}

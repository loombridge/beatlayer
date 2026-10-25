//! The Clip View's Main Properties panel through the shell (WP-6.55): which
//! panel the left group shows, scrolling the editor to a marker, and the Set
//! buttons, which move a marker of a playing clip to where it has reached.
//! The behaviour is in the clip properties spec, R1 to R12.
//!
//! The Set buttons need the engine's playhead and the launch quantisation,
//! which the document doesn't hold, so they are run here.

use bl_core::time::{SampleRate, Ticks};
use bl_engine::{SlotState, session_content_tick};
use bl_model::{AudioData, Clip, ClipData, ClipId, Command, MidiData};
use bl_shell_spec::clip_properties::{
    self as ids, ArrangePanelsArgs, FoldPanelsArgs, Marker, MarkerArgs, MarkerAt, PropertiesShown,
    ShowPropertiesArgs,
};
use serde_json::Value;

use super::*;
use crate::arrangement::editing::{EditFailure, transaction};
use crate::sample_editor::SampleView;

/// The span of a clip's contents a Set button reads: where the clip started
/// playing, and how long ago.
struct Played {
    /// The song position the clip started from: its start in the
    /// Arrangement, or the tick its Session launch fell on.
    origin: Ticks,
    /// Ticks since then.
    since: Ticks,
}

impl Shell {
    /// Runs the Main Properties panel's own commands (WP-6.55).
    pub(super) fn run_clip_properties(
        &mut self,
        id: &str,
        args: Value,
    ) -> Result<Value, ShellError> {
        match id {
            ids::SHOW_PROPERTIES => {
                let ShowPropertiesArgs { panel } = Self::args(id, args)?;
                self.clip_view.show_properties(panel);
                let panel = self
                    .clip_view
                    .shown_properties(self.editor.document(), &self.arrangement);
                Ok(to_value(&PropertiesShown { panel }))
            }
            ids::SHOW_MARKER => {
                let MarkerArgs { clip, marker } = Self::args(id, args)?;
                let clip = self.marker_clip(id, clip)?;
                let position = self.marker_value(id, clip, marker)?;
                if let Some(ClipData::Audio(audio)) =
                    self.editor.document().clip(clip).map(|c| c.data.clone())
                {
                    let lowest = if audio.warp { f64::MIN } else { 0.0 };
                    self.clip_view
                        .sample_mut(clip, &audio)
                        .reveal(position, lowest);
                }
                Ok(to_value(&MarkerAt {
                    clip,
                    marker,
                    position,
                }))
            }
            ids::SET_MARKER_AT_PLAYHEAD => {
                let MarkerArgs { clip, marker } = Self::args(id, args)?;
                let clip = self.marker_clip(id, clip)?;
                let position = self.set_marker_at_playhead(id, clip, marker)?;
                Ok(to_value(&MarkerAt {
                    clip,
                    marker,
                    position,
                }))
            }
            ids::FOCUS_PANELS | ids::FOCUS_SELECTED_PANEL => {
                Self::args::<NoArgs>(id, args)?;
                self.run(bl_shell_spec::commands::FOCUS_CLIP_VIEW, json!({}))?;
                // Spec: detail-views R24. Folded panels unfold first.
                self.clip_view.set_panels_folded(false);
                self.effects.push(if id == ids::FOCUS_PANELS {
                    Effect::FocusClipPanels
                } else {
                    Effect::FocusSelectedClipPanel
                });
                Ok(Value::Null)
            }
            ids::ARRANGE_PANELS => {
                // Spec: detail-views R20, R22. A preference, no part of the
                // set.
                let ArrangePanelsArgs { arrangement } = Self::args(id, args)?;
                self.clip_view.set_panel_arrangement(arrangement);
                self.prefs.panel_arrangement = arrangement;
                self.save_preferences();
                Ok(to_value(&self.clip_view.panels()))
            }
            ids::FOLD_PANELS => {
                // Spec: detail-views R23. View state: no undo step.
                let FoldPanelsArgs { folded } = Self::args(id, args)?;
                let folded = folded.unwrap_or(!self.clip_view.panels_folded());
                self.clip_view.set_panels_folded(folded);
                Ok(to_value(&self.clip_view.panels()))
            }
            other => Err(ShellError::UnknownCommand(other.to_owned())),
        }
    }

    /// Tells the Clip View how tall it is, for Automatically to arrange the
    /// panels by (detail views R20). An expanded Clip View is as tall as the
    /// window allows; shortcut: the window's own height (any, before the
    /// host reports one) stands in for the layout's largest, which only the UI
    /// knows, so a window too short to leave 400 points of Clip View reads
    /// vertical here where the UI draws horizontal.
    pub(super) fn sync_panels_height(&mut self) {
        let height = if self.detail.expanded() {
            self.prefs
                .window_size
                .map_or(f32::MAX, |(_, window)| window)
        } else {
            self.detail.clip_height() as f32
        };
        self.clip_view.set_panels_height(height);
    }

    /// Whether `clip` is playing now, which is when the Set buttons work.
    #[must_use]
    pub fn clip_playing(&self, clip: ClipId) -> bool {
        self.editor
            .document()
            .clip(clip)
            .is_some_and(|clip| self.clip_played(clip).is_some())
    }

    /// The clip a panel command acts on: the one given, or the one the Clip
    /// View shows.
    fn marker_clip(&self, id: &str, clip: Option<ClipId>) -> Result<ClipId, ShellError> {
        let doc = self.editor.document();
        clip.or_else(|| self.clip_view.shown_any_clip(doc, &self.arrangement))
            .filter(|&clip| doc.clip(clip).is_some())
            .ok_or_else(|| ShellError::Unavailable {
                command: id.to_owned(),
                reason: "no clip is shown in the Clip View".to_owned(),
            })
    }

    /// Where `marker` of `clip` is, in ticks of a MIDI or warped audio
    /// clip's contents or seconds in an unwarped one's file. A marker that
    /// is "the end of the file" or "of the clip" reads where that falls.
    fn marker_value(&self, id: &str, clip: ClipId, marker: Marker) -> Result<f64, ShellError> {
        let doc = self.editor.document();
        let Some(value) = doc.clip(clip) else {
            return Err(ShellError::Unavailable {
                command: id.to_owned(),
                reason: format!("clip {clip} isn't in the set"),
            });
        };
        match &value.data {
            ClipData::Midi(midi) => Ok(match marker {
                Marker::Start => midi.start,
                Marker::End => midi.end.unwrap_or(midi.start + value.length),
                Marker::LoopPosition => midi.loop_start,
                Marker::LoopLength => midi.loop_end.unwrap_or(midi.loop_start + value.length),
            }
            .get() as f64),
            ClipData::Audio(audio) => Ok(match marker {
                Marker::Start => audio.start,
                Marker::End => audio.end.map_or_else(|| self.file_end(id, audio), Ok)?,
                Marker::LoopPosition => audio.loop_start,
                Marker::LoopLength => audio
                    .loop_end
                    .map_or_else(|| self.file_end(id, audio), Ok)?,
            }),
        }
    }

    /// The end of an audio clip's file on the clip's axis: ticks of a warped
    /// clip's contents or seconds.
    fn file_end(&self, id: &str, audio: &AudioData) -> Result<f64, ShellError> {
        let seconds = self
            .file_seconds(audio)
            .ok_or_else(|| ShellError::Unavailable {
                command: id.to_owned(),
                reason: "the media store doesn't have the clip's file".to_owned(),
            })?;
        Ok(SampleView::beat_of(audio, seconds).map_or(seconds, |beat| beat.get() as f64))
    }

    /// How long `clip` has played and from where, if it plays now: a Session
    /// clip since the launch the engine reports, an Arrangement clip since
    /// the playhead entered it.
    fn clip_played(&self, clip: &Clip) -> Option<Played> {
        let transport = self.arrangement.transport();
        if !transport.playing {
            return None;
        }
        let doc = self.editor.document();
        let slot = self.session_states.iter().find(|status| {
            matches!(status.state, SlotState::Playing | SlotState::QueuedToStop)
                && doc
                    .slot(status.track, status.scene)
                    .is_some_and(|slot| slot.id == clip.id)
        });
        if let Some(status) = slot {
            let rate = self.audio.sample_rate().and_then(SampleRate::new)?;
            let origin = doc.settings().tempo.frames_to_ticks(status.started?, rate);
            return Some(Played {
                origin,
                since: (transport.position - origin).max(Ticks::ZERO),
            });
        }
        let track = doc.track_order().into_iter().find(|&track| {
            doc.track(track)
                .is_some_and(|t| t.clips().any(|c| c.id == clip.id))
        })?;
        (!self.session_over.contains(&track)
            && transport.position >= clip.start
            && transport.position < clip.end())
        .then(|| Played {
            origin: clip.start,
            since: transport.position - clip.start,
        })
    }

    /// `since`, rounded down to a step of the global launch quantisation
    /// (Spec: clip-properties R5): exactly as it is with quantisation off or
    /// None. A bar step is a bar of the clip's Signature, or the song's.
    fn on_launch_grid(&self, clip: &Clip, origin: Ticks, since: Ticks) -> Ticks {
        let launch = self.store.launch();
        let bar = match &clip.data {
            ClipData::Midi(MidiData { signature, .. })
            | ClipData::Audio(AudioData { signature, .. }) => *signature,
        }
        .unwrap_or_else(|| {
            self.editor
                .document()
                .settings()
                .time_signatures
                .signature_at(origin)
        })
        .bar_length();
        match launch.quantization.ticks(bar).filter(|_| launch.enabled) {
            Some(step) if step > Ticks::ZERO => since - Ticks(since.get().rem_euclid(step.get())),
            _ => since,
        }
    }

    /// Where `clip` has reached on its own axis (ticks of a MIDI or warped
    /// audio clip's contents, seconds in an unwarped one's file) `since`
    /// after it started from song position `origin`.
    fn reached(&self, clip: &Clip, origin: Ticks, since: Ticks) -> Option<f64> {
        match &clip.data {
            ClipData::Midi(_) => session_content_tick(clip, true, since).map(|at| at.get() as f64),
            ClipData::Audio(audio) => {
                // A warped clip's contents run in ticks, an unwarped clip's
                // file in seconds.
                let into = if audio.warp {
                    audio.start + since.get() as f64
                } else {
                    let tempo = &self.editor.document().settings().tempo;
                    tempo.ticks_to_seconds(origin + since) - tempo.ticks_to_seconds(origin)
                        + audio.start
                };
                Some(match (audio.looping, audio.loop_end) {
                    (true, Some(to)) if into >= to && to > audio.loop_start => {
                        audio.loop_start
                            + (into - audio.loop_start).rem_euclid(to - audio.loop_start)
                    }
                    _ => into,
                })
            }
        }
    }

    /// Runs `clip.set_marker_at_playhead` (R5, R8): moves `marker` to where
    /// the playing clip has reached, as one undo step, and returns where it
    /// went.
    fn set_marker_at_playhead(
        &mut self,
        id: &str,
        clip: ClipId,
        marker: Marker,
    ) -> Result<f64, ShellError> {
        let unavailable = |reason: &str| ShellError::Unavailable {
            command: id.to_owned(),
            reason: reason.to_owned(),
        };
        let doc = self.editor.document();
        let value = doc
            .clip(clip)
            .ok_or_else(|| unavailable("the clip isn't in the set"))?
            .clone();
        let played = self
            .clip_played(&value)
            .ok_or_else(|| unavailable("the clip isn't playing"))?;
        let since = self.on_launch_grid(&value, played.origin, played.since);
        let at = self
            .reached(&value, played.origin, since)
            .ok_or_else(|| unavailable("the clip isn't playing"))?;
        let loop_start = self.marker_value(id, clip, Marker::LoopPosition)?;
        let keep_end = match &value.data {
            ClipData::Midi(midi) => midi.loop_end.map(|end| end.get() as f64),
            ClipData::Audio(audio) => audio.loop_end,
        };
        let looping = match &value.data {
            ClipData::Midi(midi) => midi.looping,
            ClipData::Audio(audio) => audio.looping,
        };
        let (start, end) = (
            self.marker_value(id, clip, Marker::Start)?,
            match &value.data {
                ClipData::Midi(midi) => midi.end.map(|end| end.get() as f64),
                ClipData::Audio(audio) => audio.end,
            },
        );
        let commands: Vec<Command> = match (&value.data, marker) {
            (ClipData::Midi(_), Marker::Start) => vec![Command::SetMidiClipRegion {
                clip,
                start: Ticks(at as i64),
                end: end.map(|end| Ticks(end as i64)),
            }],
            (ClipData::Midi(_), Marker::End) => vec![Command::SetMidiClipRegion {
                clip,
                start: Ticks(start as i64),
                end: Some(Ticks(at as i64)),
            }],
            (ClipData::Audio(audio), Marker::Start) => vec![Command::SetClipRegion {
                clip,
                start: at,
                end,
                ticks: audio.warp,
            }],
            (ClipData::Audio(audio), Marker::End) => vec![Command::SetClipRegion {
                clip,
                start,
                end: Some(at),
                ticks: audio.warp,
            }],
            (_, Marker::LoopPosition) => {
                vec![self.loop_command(&value, clip, at, keep_end), loop_on(clip)]
            }
            (_, Marker::LoopLength) => {
                // With the loop off the whole brace moves to end here. A
                // brace with its end left out ends where the file or the
                // clip does.
                let from = if looping {
                    loop_start
                } else {
                    at - (self.marker_value(id, clip, Marker::LoopLength)? - loop_start)
                };
                vec![
                    self.loop_command(&value, clip, from, Some(at)),
                    loop_on(clip),
                ]
            }
        };
        transaction(&mut self.editor, "Set Clip Marker", |editor| {
            for command in commands {
                editor.apply(command)?;
            }
            Ok(())
        })
        .map_err(|error| match error {
            EditFailure::Refused(reason) | EditFailure::Unavailable(reason) => {
                ShellError::Unavailable {
                    command: id.to_owned(),
                    reason,
                }
            }
            EditFailure::InvalidArgs(message) => ShellError::InvalidArgs {
                command: id.to_owned(),
                message,
            },
        })?;
        Ok(at)
    }

    /// The command that puts `clip`'s loop brace from `start` to `end`, in
    /// the form the clip's contents use.
    fn loop_command(&self, clip: &Clip, id: ClipId, start: f64, end: Option<f64>) -> Command {
        match &clip.data {
            ClipData::Midi(_) => Command::SetMidiClipLoop {
                clip: id,
                start: Ticks(start as i64),
                end: end.map(|end| Ticks(end as i64)),
            },
            ClipData::Audio(audio) => Command::SetClipLoop {
                clip: id,
                start,
                end,
                ticks: audio.warp,
            },
        }
    }
}

/// The command that switches `clip`'s loop on.
fn loop_on(clip: ClipId) -> Command {
    Command::SetClipLoopEnabled {
        clip,
        enabled: true,
    }
}

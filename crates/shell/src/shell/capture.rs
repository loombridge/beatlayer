//! Capture MIDI (WP-3.14): the notes played on monitoring MIDI tracks, as a
//! clip on each track that was played.
//!
//! The engine logs the notes and controllers ([`bl_engine::CapturedMidi`]);
//! the shell keeps the latest of them per track in a [`Buffer`] until a
//! capture or a recording uses them. The rules for the tempo, the loop and the merge are
//! in `crate::capture_midi`; this places the result in the Session View.

use std::collections::BTreeMap;

use bl_core::midi::MidiMessage;
use bl_core::time::{Bpm, TempoMap};
use bl_engine::SessionCommand;
use bl_engine::controller_take::{Heard, give_expressions, give_pressure};
use bl_engine::mpe::MpeRole;
use bl_model::{
    AutomationTarget, Clip, ClipData, ClipId, Command, EnvelopeKind, MidiData, Note, SceneId,
};
use bl_shell_spec::capture_midi::{Captured, CapturedClip};

use super::session_recording::{Pick, pick, playing};
use super::*;
use crate::recording::write_moves;
use bl_engine::capture::{self, CapturedMidi, Fit, Played, Timing};

/// The undo step a capture is.
const UNDO_LABEL: &str = "Capture MIDI";

/// What one track's capture does, before the document is touched.
enum Work {
    /// Notes and controller moves added to the clip playing in a slot.
    Merge {
        scene: SceneId,
        clip: Box<Clip>,
        notes: Vec<Note>,
        moves: Vec<(Ticks, MidiMessage)>,
    },
    /// A new clip in the slot of `scene`, or of a scene added at the end.
    New {
        scene: Option<SceneId>,
        data: MidiData,
        length: Ticks,
        moves: Vec<(Ticks, MidiMessage)>,
    },
}

impl Shell {
    /// Keeps the notes the engine has logged since the last call. A running
    /// recording owns what is played during it, so they are let go.
    pub(super) fn poll_capture(&mut self) {
        let heard = self.audio.take_captured();
        if self.recording.is_some() {
            self.capture.clear();
            return;
        }
        for message in heard {
            self.capture.push(message);
        }
    }

    /// Whether Capture MIDI has a note to bring back, as the Control Bar's
    /// button and Create ▸ Capture MIDI enable on.
    #[must_use]
    pub fn capture_midi_ready(&self) -> bool {
        self.capture.has_notes()
    }

    /// How many played notes Capture MIDI holds, counted by their releases,
    /// as of the last [`Shell::poll_engine`]. A test waits on it, since the
    /// engine logs what the keyboard sends on its own thread.
    #[must_use]
    pub fn capture_midi_released(&self) -> usize {
        self.capture.released()
    }

    /// Runs `midi.capture`.
    pub(super) fn run_capture_midi(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        Self::args::<NoArgs>(id, args)?;
        let unavailable = |reason: &str| ShellError::Unavailable {
            command: id.to_owned(),
            reason: reason.to_owned(),
        };
        self.poll_capture();
        if !self.capture.has_notes() {
            return Err(unavailable(
                "nothing to capture: play notes on a MIDI track that monitors its input first",
            ));
        }
        let rate = self
            .audio
            .sample_rate()
            .and_then(SampleRate::new)
            .ok_or_else(|| unavailable("no audio device is open"))?;
        let running = self.audio.transport_state().playing;
        let heard = self.capture.take();
        let doc = self.editor.document();
        let tempo = doc.settings().tempo.clone();
        let signatures = doc.settings().time_signatures.clone();
        let tracks: Vec<(TrackId, Vec<Played>)> = doc
            .track_order()
            .into_iter()
            .filter(|&track| {
                doc.track(track)
                    .is_some_and(|t| t.kind() == TrackKind::Midi)
            })
            .filter_map(|track| {
                let notes = capture::pair(heard.get(&track)?);
                (!notes.is_empty()).then_some((track, notes))
            })
            .collect();
        if tracks.is_empty() {
            return Err(unavailable("the tracks that were played are gone"));
        }
        let ticks = |frames: Frames| tempo.frames_to_ticks(frames, rate);
        // Spec: midi-controller-data R15. Controllers up to the last note-off.
        let controllers: BTreeMap<TrackId, Vec<CapturedMidi>> = heard
            .iter()
            .map(|(track, messages)| (*track, capture::controllers(messages)))
            .collect();
        let stamped = tracks
            .iter()
            .flat_map(|(_, notes)| notes.iter().filter_map(|n| n.song_on))
            .min();
        let current = tempo.bpm_at(Ticks::ZERO).get();
        let seconds_of = |clock: u64, from: u64| (clock - from) as f64 / rate.as_f64();
        let first = tracks
            .iter()
            .map(|(_, notes)| notes[0].on)
            .min()
            .unwrap_or(0);
        let origin = stamped.filter(|_| running).map_or(Ticks::ZERO, |at| {
            signatures.bar_start(signatures.bar_at(ticks(at).max(Ticks::ZERO)).0)
        });
        let bar = signatures.signature_at(origin).bar_length();
        let bar_seconds = bar.as_quarters() * 60.0 / current;
        let phrases: Vec<usize> = tracks
            .iter()
            .map(|(_, notes)| capture::last_phrase(notes, rate, bar_seconds))
            .collect();
        // Spec: capture-midi R6. A stopped transport may change the tempo,
        // unless the song's tempo changes along it.
        let fit = if running || tempo.segments().len() != 1 {
            Fit {
                bpm: current,
                set: None,
            }
        } else {
            let mut onsets: Vec<f64> = tracks
                .iter()
                .zip(&phrases)
                .flat_map(|((_, notes), &phrase)| {
                    notes[phrase..].iter().map(|n| seconds_of(n.on, first))
                })
                .collect();
            onsets.sort_by(f64::total_cmp);
            capture::fit(&onsets, current)
        };
        let timing = match stamped.filter(|_| running) {
            Some(_) => Timing::Running { origin },
            None => Timing::Stopped {
                from: first,
                bpm: fit.bpm,
                rate,
            },
        };
        let from = self.session_scene();
        let mut work = Vec::new();
        for ((track, notes), phrase) in tracks.iter().zip(phrases) {
            let Some(Pick { scene, overdub, .. }) = pick(doc, *track, from, &self.session_states)
            else {
                continue;
            };
            if overdub {
                // Spec: capture-midi R4. The clip that plays takes the notes.
                let slot = scene.and_then(|scene| Some((scene, doc.slot(*track, scene)?.clone())));
                let started = scene.and_then(|scene| {
                    playing(&self.session_states, *track, scene).and_then(|s| s.started)
                });
                let (Some((scene, clip)), Some(started)) = (slot, started) else {
                    continue;
                };
                let (mut notes, ids) = launch_relative(notes, started, &ticks);
                let moves = launch_moves(&controllers[track], started, &ticks);
                give_pressure(&mut notes, &moves);
                // Spec: mpe R12. The expressions of notes an MPE port played.
                give_expressions(
                    &mut notes,
                    &ids,
                    &launch_expressions(&heard[track], started, &ticks),
                );
                work.push((
                    *track,
                    Work::Merge {
                        scene,
                        clip: Box::new(clip),
                        notes,
                        moves,
                    },
                ));
                continue;
            }
            let mut placed = capture::place(notes, timing, ticks, phrase, bar);
            let moves = capture::place_controllers(&controllers[track], timing, ticks);
            give_pressure(&mut placed.notes, &moves);
            // Spec: mpe R12. The expressions of notes an MPE port played.
            give_expressions(
                &mut placed.notes,
                &placed.ids,
                &capture::place_expressions(&heard[track], timing, ticks),
            );
            let data = MidiData {
                looping: true,
                start: placed.loop_start,
                loop_start: placed.loop_start,
                loop_end: Some(placed.loop_end),
                ..MidiData::new(placed.notes)
            };
            let length = placed.loop_end;
            work.push((
                *track,
                Work::New {
                    scene,
                    data,
                    length,
                    moves,
                },
            ));
        }
        let look = &self.prefs.look;
        let made = self
            .editor
            .transact(UNDO_LABEL, |editor| apply(editor, look, work, fit.set))
            .map_err(|error| unavailable(&error.to_string()))?;
        // The engine hears of the new clips before the launch asks for them.
        self.dirty = true;
        self.document_changed();
        for clip in made.iter().filter(|clip| !clip.merged) {
            // A new clip starts as a launch does: on the next grid line
            // while the song plays, at once while it is stopped.
            let _ = self.audio.send_session(SessionCommand::Launch {
                track: clip.track,
                scene: clip.scene,
            });
        }
        if let Some(last) = made.last() {
            let _ = self.run(
                session_ids::SELECT,
                json!({ "track": last.track, "scene": last.scene }),
            );
        }
        self.touch();
        Ok(to_value(&Captured {
            clips: made,
            tempo: fit.set.map(Bpm::get),
        }))
    }
}

/// The notes with their starts counted from the frame the clip launched on,
/// and the ID each has if an MPE port played it; those played before it, or
/// with no known song position, are left out.
fn launch_relative(
    notes: &[Played],
    started: Frames,
    ticks: &impl Fn(Frames) -> Ticks,
) -> (Vec<Note>, Vec<Option<u32>>) {
    let begin = ticks(started);
    notes
        .iter()
        .filter_map(|n| {
            let (on, off) = (ticks(n.song_on?), ticks(n.song_off.or(n.song_on)?));
            (on >= begin).then(|| {
                let mut note = Note::new(n.key, on - begin, (off - on).max(Ticks(1)), n.velocity);
                note.release_velocity = n.release;
                (note, n.id)
            })
        })
        .unzip()
}

/// What an MPE port said about its notes, with positions counted from the
/// frame the clip launched on, like [`launch_relative`]'s.
fn launch_expressions(
    messages: &[CapturedMidi],
    started: Frames,
    ticks: &impl Fn(Frames) -> Ticks,
) -> Vec<Heard> {
    let begin = ticks(started);
    messages
        .iter()
        .filter_map(|m| {
            let MpeRole::Expression {
                id,
                expression,
                value,
                ..
            } = m.mpe
            else {
                return None;
            };
            Some(Heard {
                id,
                expression,
                value: f64::from(value),
                at: ticks(m.song?) - begin,
            })
        })
        .collect()
}

/// The controller moves with their positions counted from the frame the
/// clip launched on, like [`launch_relative`]'s.
fn launch_moves(
    controllers: &[CapturedMidi],
    started: Frames,
    ticks: &impl Fn(Frames) -> Ticks,
) -> Vec<(Ticks, MidiMessage)> {
    let begin = ticks(started);
    controllers
        .iter()
        .filter_map(|c| {
            let at = ticks(c.song?);
            (at >= begin).then_some((at - begin, c.message))
        })
        .collect()
}

/// Applies a capture's work inside the open transaction: the tempo, the
/// scene a track needs at the end, the new clips and the merged ones.
fn apply(
    editor: &mut Editor,
    look: &bl_shell_spec::settings::ThemeLook,
    work: Vec<(TrackId, Work)>,
    tempo: Option<Bpm>,
) -> Result<Vec<CapturedClip>, bl_model::EditError> {
    if let Some(bpm) = tempo {
        editor.apply(Command::SetTempo {
            tempo: TempoMap::constant(bpm),
        })?;
    }
    let mut inserted: Option<SceneId> = None;
    let mut made = Vec::new();
    for (track, work) in work {
        match work {
            Work::Merge {
                scene,
                clip,
                notes,
                moves,
            } => {
                let ClipData::Midi(data) = &clip.data else {
                    continue;
                };
                // Spec: midi-controller-data R15. The moves stand where the
                // notes do, and a clip that doesn't loop takes those before
                // its end.
                let marker = data.start;
                let end = data
                    .end
                    .unwrap_or(marker + clip.length)
                    .min(marker + clip.length);
                let moves: Vec<_> = moves
                    .into_iter()
                    .map(|(at, message)| (marker + at, message))
                    .filter(|(at, _)| data.looping || *at < end)
                    .collect();
                let reach = moves.iter().map(|(at, _)| *at - marker).max();
                let (merged, length) =
                    capture::merge(data, clip.length, notes, reach.unwrap_or(Ticks::ZERO));
                // A loop that moved on takes the controllers' loop with it.
                let before = capture::loop_span(data, clip.length);
                let after = capture::loop_span(&merged, length);
                let copies = (after.1 - before.1).get() / (before.1 - before.0).get();
                editor.apply(Command::SetClipLength {
                    clip: clip.id,
                    length,
                })?;
                editor.apply(Command::SetClipData {
                    clip: clip.id,
                    data: ClipData::Midi(merged),
                })?;
                for envelope in clip.envelopes().filter(|e| e.target.is_midi()) {
                    let points: Vec<_> = envelope.envelope.points().collect();
                    repeat_loop(editor, clip.id, envelope.target, &points, before, copies)?;
                }
                write_moves(editor, clip.id, &moves, None)?;
                made.push(CapturedClip {
                    track,
                    scene,
                    clip: clip.id,
                    merged: true,
                });
            }
            Work::New {
                scene,
                data,
                length,
                moves,
            } => {
                let scene = match scene.or(inserted) {
                    Some(scene) => scene,
                    None => {
                        let scene: SceneId = editor.new_id();
                        let index = editor.document().scenes().len();
                        editor.apply(Command::CreateScene { id: scene, index })?;
                        inserted = Some(scene);
                        scene
                    }
                };
                let clip: ClipId = editor.new_id();
                editor.apply(Command::CreateSlotClip {
                    track,
                    scene,
                    // Spec: grooves R20. A clip made by recording takes the
                    // set's auto-load groove.
                    clip: Clip::new(clip, Ticks::ZERO, length, ClipData::Midi(data))
                        .with_groove(editor.document().settings().grooves.auto_loaded())
                        .with_colour(look.new_clip_colour(clip)),
                })?;
                write_moves(editor, clip, &moves, None)?;
                made.push(CapturedClip {
                    track,
                    scene,
                    clip,
                    merged: false,
                });
            }
        }
    }
    Ok(made)
}

/// Copies one controller envelope's loop into the `copies` passes a capture
/// added to the clip, as the notes are.
fn repeat_loop(
    editor: &mut Editor,
    clip: ClipId,
    target: AutomationTarget,
    points: &[bl_model::Breakpoint],
    span: (Ticks, Ticks),
    copies: i64,
) -> Result<(), bl_model::EditError> {
    for point in capture::repeat_points(points, span, copies) {
        editor.apply(Command::SetClipEnvelopeBreakpoint {
            clip,
            target,
            kind: EnvelopeKind::Modulation,
            at: point.at,
            value: point.value,
        })?;
        if point.bend != 0.0 {
            editor.apply(Command::SetClipEnvelopeCurve {
                clip,
                target,
                kind: EnvelopeKind::Modulation,
                at: point.at,
                bend: point.bend,
            })?;
        }
    }
    Ok(())
}

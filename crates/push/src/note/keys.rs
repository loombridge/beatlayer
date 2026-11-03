//! The pads of a melodic layout: the notes, the Melodic Sequencer's cells and
//! steps, and pressure (`docs/specs/push-2-notes.md`, R6, R38 and R42 to
//! R44).

use bl_control::ReadModel;
use bl_core::midi::MidiMessage;
use bl_core::time::Ticks;
use serde_json::json;

use super::steps::VELOCITY;
use super::{Down, Frame, Note, Pending, Source, clip};
use crate::content::{Ctx, HeldStep, Pad};
use crate::controls::cc;

/// The notes whose bit is set in `keys`, lowest first.
pub(super) fn keys_of(keys: u128) -> impl Iterator<Item = u8> {
    (0..128u8).filter(move |key| keys >> key & 1 == 1)
}

impl Note {
    /// A pad's pressure becomes the pressure of the note it plays: polyphonic
    /// key pressure on that note, which the engine turns into the note's own
    /// (R6).
    pub(super) fn pressure(&mut self, pad: Pad, pressure: u8, ctx: &mut Ctx<'_>) {
        let note = match self.down.get(&(pad.row, pad.column)) {
            Some(Down::Played(note)) => *note,
            Some(Down::Picked { key, played: true }) => *key,
            _ => return,
        };
        self.repeat_pressure((pad.row, pad.column), pressure);
        if let Some(message) = MidiMessage::from_bytes(&[0xA0, note, pressure.min(127)]) {
            ctx.cx.play_at(ctx.at, message);
        }
    }

    /// A pad of 64 Notes plays its note; with Delete held it deletes the
    /// pitch's notes inside the loop (R32, R38).
    pub(super) fn key_pad(
        &mut self,
        frame: &Frame,
        note: u8,
        velocity: u8,
        ctx: &mut Ctx<'_>,
        model: &ReadModel,
    ) -> Down {
        if ctx.modifiers.has(cc::DELETE) {
            if let Some(id) = frame.clip
                && let Some((c, midi)) = clip::midi(model, id)
            {
                let (from, to) = clip::region(c, midi);
                let places = clip::places(midi, note, from, to);
                if !places.is_empty() {
                    ctx.cx
                        .run("note.delete", json!({ "clip": id, "notes": places }));
                }
            }
            return Down::Done;
        }
        self.voice(frame.track, ctx, note, velocity);
        Down::Played(note)
    }

    /// One of the 32 notes: it plays and selects its note, together with the
    /// pads held with it; with Select held it only selects; while a step is
    /// held it removes the note from the step (R43, R44).
    pub(super) fn pick_pad(
        &mut self,
        frame: &Frame,
        key: u8,
        velocity: u8,
        ctx: &mut Ctx<'_>,
        model: &ReadModel,
    ) -> Down {
        let held = self.down.values().find_map(Down::step);
        if let Some(step) = held
            && let Some(id) = step.clip
            && let Some((_, midi)) = clip::midi(model, id)
        {
            let places = clip::places(midi, key, step.start, step.start + step.length);
            if !places.is_empty() {
                ctx.cx
                    .run("note.delete", json!({ "clip": id, "notes": places }));
                return Down::Done;
            }
        }
        let together = self
            .down
            .values()
            .any(|down| matches!(down, Down::Picked { .. }));
        let memory = self.memory(frame.track);
        let before = if together { memory.keys } else { 0 };
        memory.keys = before | (1u128 << key);
        if ctx.modifiers.has(cc::SELECT) {
            return Down::Picked { key, played: false };
        }
        self.voice(frame.track, ctx, key, velocity);
        Down::Picked { key, played: true }
    }

    /// A cell of the Melodic Sequencer: adds its row's note at its step, or
    /// deletes the note there, or with Mute held deactivates it (R42).
    pub(super) fn cell_pad(
        &mut self,
        frame: &Frame,
        key: u8,
        step: u8,
        ctx: &mut Ctx<'_>,
        model: &ReadModel,
    ) -> Down {
        let start = frame.pages.step_start(self.pair, step);
        self.toggle_note(frame, key, start, VELOCITY, ctx, model)
    }

    /// A step of the Melodic Sequencer + 32 Notes: adds every selected note
    /// to it; with Duplicate held copies its notes to another step, with
    /// Delete held deletes them, with Mute held deactivates them (R44).
    pub(super) fn notes_step(
        &mut self,
        frame: &Frame,
        index: u8,
        ctx: &mut Ctx<'_>,
        model: &ReadModel,
    ) -> Down {
        let start = frame.pages.step_start(self.pair, index);
        let length = frame.size.ticks();
        let end = start + length;
        let selected = frame.keys;
        let step = HeldStep {
            clip: frame.clip,
            key: keys_of(selected).next().unwrap_or(36),
            start,
            length,
        };
        let Some((id, midi)) = frame
            .clip
            .and_then(|id| clip::midi(model, id).map(|(_, midi)| (id, midi)))
        else {
            // The first step of an empty slot makes a clip with the notes.
            if selected != 0
                && let Some(scene) = frame.scene
                && model.document.slot(frame.track, scene).is_none()
                && !ctx.modifiers.has(cc::MUTE)
                && !ctx.modifiers.has(cc::DELETE)
                && !ctx.modifiers.has(cc::DUPLICATE)
            {
                self.first_step(frame, scene, selected, start, length, VELOCITY, ctx);
            }
            return Down::Step(step);
        };
        if ctx.modifiers.has(cc::DUPLICATE) {
            match self.source.take() {
                Some(Source::Step(from)) if from != start => {
                    let adds: Vec<_> = midi
                        .notes
                        .iter()
                        .filter(|n| n.start >= from && n.start < from + length)
                        .map(|n| {
                            (
                                "note.add",
                                json!({
                                    "clip": id,
                                    "key": n.key,
                                    "start": n.start - from + start,
                                    "length": n.length,
                                    "velocity": n.velocity,
                                }),
                            )
                        })
                        .collect();
                    Self::group(ctx.cx, adds);
                }
                _ => self.source = Some(Source::Step(start)),
            }
        } else if ctx.modifiers.has(cc::DELETE) {
            let places = clip::places_in(midi, start, end);
            if !places.is_empty() {
                ctx.cx
                    .run("note.delete", json!({ "clip": id, "notes": places }));
            }
        } else if ctx.modifiers.has(cc::MUTE) {
            let places = clip::places_in(midi, start, end);
            if !places.is_empty() {
                ctx.cx
                    .run("note.deactivate", json!({ "clip": id, "notes": places }));
            }
        } else {
            let adds: Vec<_> = keys_of(selected)
                .filter(|key| clip::places(midi, *key, start, end).is_empty())
                .map(|key| {
                    (
                        "note.add",
                        json!({
                            "clip": id,
                            "key": key,
                            "start": start,
                            "length": length,
                            "velocity": self.accented(VELOCITY),
                        }),
                    )
                })
                .collect();
            Self::group(ctx.cx, adds);
        }
        Down::Step(step)
    }

    /// Makes the one-bar clip of an empty slot, which the next tick fills
    /// with the notes of `keys` at `start` and launches (R20).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn first_step(
        &mut self,
        frame: &Frame,
        scene: bl_model::SceneId,
        keys: u128,
        start: Ticks,
        length: Ticks,
        velocity: u8,
        ctx: &mut Ctx<'_>,
    ) {
        if self.pending.is_some() {
            return;
        }
        ctx.cx.run_held(
            super::GESTURE,
            "session.insert_empty_midi_clip",
            json!({ "track": frame.track, "scene": scene }),
        );
        self.pending = Some(Pending {
            track: frame.track,
            scene,
            keys,
            start,
            length,
            velocity,
            waited: 0,
        });
    }
}

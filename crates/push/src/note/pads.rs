//! The drum pads and the velocity pads (`docs/specs/push-2-notes.md`, R2,
//! R10 and R14 to R19).

use bl_control::ReadModel;
use bl_core::midi::MidiMessage;
use bl_model::{Chain, DeviceId, Receive};
use serde_json::json;

use super::{Down, Frame, Kind, Note, Source, clip, grid::Role};
use crate::content::{Ctx, Pad, Target};
use crate::controls::cc;

/// The chains of `rack` that receive `note`: the pad's.
pub(super) fn pad_chains(model: &ReadModel, rack: DeviceId, note: u8) -> Vec<&Chain> {
    model.document.device(rack).map_or_else(Vec::new, |rack| {
        rack.chains
            .iter()
            .filter(|c| !c.is_return && c.receive == Receive::Note(note))
            .collect()
    })
}

/// The swatch Push 2 shows for a pad in place of its track's colour, if its
/// chains have one (R19).
pub(super) fn pad_colour(model: &ReadModel, rack: DeviceId, note: u8) -> Option<u8> {
    pad_chains(model, rack, note)
        .into_iter()
        .find_map(|chain| chain.push_colour)
}

/// A note-on for a drum pad: channel 0, as the pads' notes reach the tracks.
pub(crate) fn note_on(note: u8, velocity: u8) -> MidiMessage {
    MidiMessage::note_on(0, note, velocity)
}

impl Note {
    pub(super) fn pad_on(&mut self, pad: Pad, velocity: u8, ctx: &mut Ctx<'_>) {
        let Some(model) = ctx.model else {
            return;
        };
        let Some(frame) = self.frame(model) else {
            return;
        };
        let down = match Self::role(&frame, pad.row, pad.column) {
            Role::Drum(note) => self.drum(&frame, note, velocity, ctx, model),
            Role::Velocity(velocity) => self.velocity_pad(&frame, velocity, ctx),
            Role::Loop(page) => self.loop_pad(&frame, page, ctx, model),
            Role::Step(index) => self.step_pad(&frame, index, ctx, model),
            Role::Off => Down::Done,
        };
        self.down.insert((pad.row, pad.column), down);
    }

    /// Releasing a pad ends the note it played, whatever the layout has
    /// become since (R2).
    pub(super) fn pad_off(&mut self, pad: Pad, ctx: &mut Ctx<'_>) {
        if let Some(Down::Played(note)) = self.down.remove(&(pad.row, pad.column)) {
            ctx.cx
                .play_at(ctx.at, MidiMessage::note_off(0, note));
        }
    }

    /// Selects `note` as the pad the steps edit (R14).
    fn pick(&mut self, frame: &Frame, note: u8, ctx: &mut Ctx<'_>) {
        match frame.kind {
            Kind::Rack { rack, .. } => ctx
                .cx
                .run("pad.select", json!({ "rack": rack, "note": note })),
            Kind::Slices => {
                self.picked.insert(frame.track, note);
            }
            Kind::Melodic | Kind::Silent => {}
        }
    }

    fn drum(
        &mut self,
        frame: &Frame,
        note: u8,
        velocity: u8,
        ctx: &mut Ctx<'_>,
        model: &ReadModel,
    ) -> Down {
        let modifiers = ctx.modifiers;
        let (rack, filled) = match frame.kind {
            Kind::Rack { rack, filled, .. } => (Some(rack), filled >> note & 1 == 1),
            _ => (None, true),
        };
        if modifiers.shift() {
            // The colour picker for the pad (R19).
            if let Some(rack) = rack
                && filled
            {
                ctx.open_picker(Target::Command {
                    command: "pad.set_push_colour".to_owned(),
                    args: json!({ "rack": rack, "note": note }),
                    current: pad_colour(model, rack, note),
                });
            }
            return Down::Done;
        }
        if modifiers.has(cc::SELECT) {
            // Silent selection (R14).
            self.pick(frame, note, ctx);
            return Down::Done;
        }
        if modifiers.has(cc::MUTE) || modifiers.has(cc::SOLO) {
            if let Some(rack) = rack
                && filled
            {
                let id = if modifiers.has(cc::MUTE) {
                    "pad.set_mute"
                } else {
                    "pad.set_solo"
                };
                ctx.cx.run(id, json!({ "rack": rack, "note": note }));
            }
            return Down::Done;
        }
        if modifiers.has(cc::DELETE) {
            self.delete_pad(frame, note, filled, ctx, model);
            return Down::Done;
        }
        if modifiers.has(cc::DUPLICATE) {
            self.duplicate_pad(frame, note, filled, ctx);
            return Down::Done;
        }
        self.pick(frame, note, ctx);
        ctx.cx.play_at(ctx.at, note_on(note, velocity));
        Down::Played(note)
    }

    /// Delete and a pad: its notes in the clip the steps edit, or its chains
    /// where it has none there (R16).
    fn delete_pad(
        &mut self,
        frame: &Frame,
        note: u8,
        filled: bool,
        ctx: &mut Ctx<'_>,
        model: &ReadModel,
    ) {
        if let Some(id) = frame.clip
            && let Some((_, midi)) = clip::midi(model, id)
        {
            let places = clip::places(midi, note, bl_core::time::Ticks(0), bl_core::time::Ticks(i64::MAX));
            if !places.is_empty() {
                ctx.cx
                    .run("note.delete", json!({ "clip": id, "notes": places }));
                return;
            }
        }
        if let Kind::Rack { rack, .. } = frame.kind
            && filled
        {
            ctx.cx.run("pad.delete", json!({ "rack": rack, "note": note }));
        }
    }

    /// Duplicate and a pad, then another: the first pad's chains replace the
    /// second's (R17).
    fn duplicate_pad(&mut self, frame: &Frame, note: u8, filled: bool, ctx: &mut Ctx<'_>) {
        let Kind::Rack { rack, filled: all, .. } = frame.kind else {
            return;
        };
        match self.source.take() {
            Some(Source::Pad(from)) if from != note && all >> from & 1 == 1 => {
                ctx.cx
                    .run("pad.copy", json!({ "rack": rack, "from": from, "to": note }));
            }
            _ if filled => self.source = Some(Source::Pad(note)),
            _ => {}
        }
    }

    /// A velocity pad plays the selected drum pad at its velocity, and new
    /// steps take it; with Select held it only chooses it (R10, R14).
    fn velocity_pad(&mut self, frame: &Frame, velocity: u8, ctx: &mut Ctx<'_>) -> Down {
        self.velocity = Some(velocity);
        if ctx.modifiers.has(cc::SELECT) {
            return Down::Done;
        }
        let Some(note) = frame.selected else {
            return Down::Done;
        };
        ctx.cx.play_at(ctx.at, note_on(note, velocity));
        Down::Played(note)
    }
}

//! The drum pads and the velocity pads (`docs/specs/push-2-notes.md`, R2,
//! R10 and R14 to R19).

use bl_control::ReadModel;
use bl_core::midi::MidiMessage;
use bl_model::{Chain, DeviceId, Receive};
use serde_json::json;

use super::{Down, Frame, Kind, Note, Sounding, Source, clip, grid::Role};
use crate::content::{Ctx, Pad, Target};
use crate::controls::cc;
use crate::simpler::{self, FIRST_KEY};

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
        // Spec: push-2-notes R54. Accent overrides the velocity the pad gave.
        let velocity = self.accented(velocity);
        let down = match Self::role(&frame, pad.row, pad.column) {
            Role::Drum(note) => self.drum(&frame, note, velocity, ctx, model),
            Role::Velocity(velocity) => self.velocity_pad(&frame, velocity, ctx),
            Role::Loop(page) => self.loop_pad(&frame, page, ctx, model),
            Role::Step(index) if matches!(frame.kind, Kind::Melodic) => {
                self.notes_step(&frame, index, ctx, model)
            }
            Role::Step(index) => self.step_pad(&frame, index, ctx, model),
            Role::Key(note) => self.key_pad(&frame, note, velocity, ctx, model),
            Role::Pick(note) => self.pick_pad(&frame, note, velocity, ctx, model),
            Role::Cell { key, step } => self.cell_pad(&frame, key, step, ctx, model),
            Role::Off => Down::Done,
        };
        self.repeat_pad(frame.track, (pad.row, pad.column), down, velocity);
        self.down.insert((pad.row, pad.column), down);
    }

    /// Releasing a pad ends the note it played, whatever the layout has
    /// become since (R2).
    pub(super) fn pad_off(&mut self, pad: Pad, ctx: &mut Ctx<'_>) {
        self.repeat_release((pad.row, pad.column));
        if let Some(
            Down::Played(note)
            | Down::Picked {
                key: note,
                played: true,
            },
        ) = self.down.remove(&(pad.row, pad.column))
        {
            ctx.cx.play_at(ctx.at, MidiMessage::note_off(0, note));
        }
    }

    /// Selects `note` as the pad the steps edit (R14).
    fn pick(&mut self, frame: &Frame, note: u8, ctx: &mut Ctx<'_>) {
        match frame.kind {
            Kind::Rack { rack, .. } => ctx
                .cx
                .run("pad.select", json!({ "rack": rack, "note": note })),
            Kind::Slices { .. } => {
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
            Kind::Slices { filled, .. } => (None, filled >> note & 1 == 1),
            _ => (None, true),
        };
        if let Kind::Slices { device, .. } = frame.kind
            && !filled
        {
            // A pad past the last slice does nothing, unless Pad Slicing
            // makes a slice of it (R74).
            if ctx.pad_slicing && !ctx.modifiers.any() {
                self.slice_here(device, ctx, model);
            }
            return Down::Done;
        }
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
        if modifiers.has(cc::QUANTIZE) {
            // Spec: push-2-notes R57. Only this pad's notes.
            if let Some(clip) = frame.clip {
                let swing = (ctx.swing * 100.0).round() as u8;
                ctx.cx.run(
                    "note.quantize",
                    json!({ "clip": clip, "pitch": note, "swing": swing }),
                );
            }
            return Down::Done;
        }
        self.pick(frame, note, ctx);
        if let Kind::Slices { device, .. } = frame.kind {
            self.sounding = simpler::View::of(model, device)
                .and_then(|view| view.played(usize::from(note - FIRST_KEY)))
                .map(|played| Sounding { at: ctx.at, played });
        }
        self.voice(frame.track, ctx, note, velocity);
        Down::Played(note)
    }

    /// Pad Slicing: a slice where the sample plays now, as far on as the
    /// pad that started it has played (R74).
    fn slice_here(&mut self, device: DeviceId, ctx: &mut Ctx<'_>, model: &ReadModel) {
        let Some(Sounding { at, played }) = self.sounding else {
            return;
        };
        if !simpler::View::of(model, device).is_some_and(|view| view.manual()) {
            return;
        }
        let elapsed = ctx.at.saturating_duration_since(at).as_secs_f64();
        let frame = played.start + (elapsed * played.per_second) as u64;
        if frame > played.start && frame < played.end {
            ctx.cx.run(
                "device.add_slice",
                json!({ "device": device, "frame": frame }),
            );
        }
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
            let places = clip::places(
                midi,
                note,
                bl_core::time::Ticks(0),
                bl_core::time::Ticks(i64::MAX),
            );
            if !places.is_empty() {
                ctx.cx
                    .run("note.delete", json!({ "clip": id, "notes": places }));
                return;
            }
        }
        if let Kind::Rack { rack, .. } = frame.kind
            && filled
        {
            ctx.cx
                .run("pad.delete", json!({ "rack": rack, "note": note }));
        }
        // A slice placed by hand goes with its pad (R73).
        if let Kind::Slices { device, .. } = frame.kind
            && filled
            && let Some(frame) = simpler::View::of(model, device)
                .and_then(|view| view.manual_start(usize::from(note - FIRST_KEY)))
        {
            ctx.cx.run(
                "device.delete_slice",
                json!({ "device": device, "frame": frame }),
            );
        }
    }

    /// Duplicate and a pad, then another: the first pad's chains replace the
    /// second's (R17).
    fn duplicate_pad(&mut self, frame: &Frame, note: u8, filled: bool, ctx: &mut Ctx<'_>) {
        let Kind::Rack {
            rack, filled: all, ..
        } = frame.kind
        else {
            return;
        };
        match self.source.take() {
            Some(Source::Pad(from)) if from != note && all >> from & 1 == 1 => {
                ctx.cx.run(
                    "pad.copy",
                    json!({ "rack": rack, "from": from, "to": note }),
                );
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
        self.voice(frame.track, ctx, note, self.accented(velocity));
        Down::Played(note)
    }
}

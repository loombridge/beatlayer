//! The touch strip in Note Mode, and Octave's range in a melodic layout
//! (`docs/specs/push-2-notes.md`, R7, R13, R33, R41 and R43).

use bl_core::midi::MidiMessage;
use serde_json::json;

use super::{Frame, Kind, Melodic, Note, grid, melodic};
use crate::content::{Ctx, StripEvent, StripUse};
use crate::controls::cc;

impl Note {
    pub(super) fn strip_event(&mut self, event: StripEvent, ctx: &mut Ctx<'_>) {
        let Some(model) = ctx.model else {
            return;
        };
        let Some(frame) = self.frame(model) else {
            return;
        };
        match frame.kind {
            Kind::Rack { rack, .. } => {
                let StripEvent::Position(position) = event else {
                    return;
                };
                // The strip chooses the bank by sixteen notes, or by a row of
                // four with Shift held (R13).
                let (parts, per) = if ctx.modifiers.shift() {
                    (32.0, 4)
                } else {
                    (8.0, 16)
                };
                let place = (position.clamp(0.0, 0.999) * parts) as u8;
                let first = (place * per).min(grid::LAST_FIRST_PAD);
                ctx.cx
                    .run("rack.move_pads", json!({ "rack": rack, "first": first }));
            }
            Kind::Melodic if frame.melodic == Melodic::Notes => self.bend(&frame, event, ctx),
            Kind::Melodic => {
                if let StripEvent::Position(position) = event {
                    self.range_strip(&frame, position, ctx);
                }
            }
            _ => {}
        }
    }

    /// 64 Notes' strip: pitch bend, or with a Select tap the modulation
    /// wheel, and back (R7).
    fn bend(&mut self, frame: &Frame, event: StripEvent, ctx: &mut Ctx<'_>) {
        match event {
            StripEvent::Touch if ctx.modifiers.has(cc::SELECT) => {
                self.strip_taken = true;
                let modulation = !frame.modulation;
                self.memory(frame.track).modulation = modulation;
                self.strip = Some(if modulation {
                    StripUse::Modulation
                } else {
                    StripUse::PitchBend
                });
                ctx.notify(if modulation {
                    "Touch Strip: Modulation Wheel"
                } else {
                    "Touch Strip: Pitch Bend"
                });
            }
            StripEvent::Release => self.strip_taken = false,
            StripEvent::Position(position) if !self.strip_taken => {
                let message = if frame.modulation {
                    let value = (position.clamp(0.0, 1.0) * 127.0).round() as u8;
                    MidiMessage::from_bytes(&[0xB0, 1, value])
                } else {
                    // The strip sends the top 8 of the 14 bits; the bend is
                    // the 14-bit value they make, 8192 at the centre (R7).
                    let raw = (position.clamp(0.0, 1.0) * 255.0).round() as u16 * 64;
                    MidiMessage::from_bytes(&[0xE0, (raw & 0x7F) as u8, (raw >> 7) as u8])
                };
                if let Some(message) = message {
                    ctx.cx.play_at(ctx.at, message);
                }
            }
            _ => {}
        }
    }

    /// The Melodic Sequencer's strip moves its rows to the position touched,
    /// by octaves with Shift held (R41, R43).
    fn range_strip(&mut self, frame: &Frame, position: f32, ctx: &mut Ctx<'_>) {
        let position = position.clamp(0.0, 1.0);
        let span = if frame.melodic == Melodic::Sequencer {
            if frame.overlay.is_some() { 7 } else { 8 }
        } else {
            32
        };
        let low = if ctx.modifiers.shift() {
            // The octave touched, from 0 to 10, with the scale's root in it.
            let octave = (position * 10.999) as i32;
            let mut low = 12 * octave + i32::from(frame.scale.root);
            while low > 127 {
                low -= 12;
            }
            low as u8
        } else {
            let notes: Vec<u8> = if frame.pads.in_key {
                frame.scale.notes()
            } else {
                (0..128).collect()
            };
            let last = notes.len().saturating_sub(span);
            notes
                .get((position * last as f32).round() as usize)
                .copied()
                .unwrap_or(frame.low)
        };
        self.set_low(frame, low, ctx);
    }

    /// Moves the lowest row or note, and notifies the range (R41).
    fn set_low(&mut self, frame: &Frame, low: u8, ctx: &mut Ctx<'_>) {
        if low == frame.low {
            return;
        }
        self.memory(frame.track).low = Some(low);
        let view = melodic::View {
            low,
            ..frame.view()
        };
        if let Some((low, high)) = view.range() {
            ctx.notify(format!(
                "{} to {}",
                melodic::note_name(low),
                melodic::note_name(high)
            ));
        }
    }

    /// Octave Up and Down in a melodic layout (R33, R41, R43).
    pub(super) fn octave_melodic(
        &mut self,
        frame: &Frame,
        up: bool,
        shift: bool,
        ctx: &mut Ctx<'_>,
    ) {
        if frame.melodic == Melodic::Notes {
            if melodic::can_octave(frame.scale, frame.pads, frame.octave, up) {
                self.memory(frame.track).octave += if up { 1 } else { -1 };
            }
        } else if let Some(low) =
            melodic::shifted_low(frame.scale, frame.pads.in_key, frame.low, up, shift)
        {
            self.set_low(frame, low, ctx);
        }
    }

    /// What the strip is for in `frame` (R7, R13, R41).
    pub(super) fn strip_for(&self, frame: &Frame) -> Option<StripUse> {
        match frame.kind {
            Kind::Rack { .. } => Some(StripUse::Range),
            Kind::Melodic if frame.melodic == Melodic::Notes => Some(if frame.modulation {
                StripUse::Modulation
            } else {
                StripUse::PitchBend
            }),
            Kind::Melodic => Some(StripUse::Range),
            Kind::Slices | Kind::Silent => None,
        }
    }
}

//! Pitch, the transposing MIDI effect (WP-5.30; the MIDI transpose spec): moves
//! every note by a number of semitones, and keeps the result inside a window
//! of keys by blocking, folding or limiting what falls outside it.
//!
//! Its panel is laid out as Live draws it (layout L-DEV-064): the Pitch knob
//! over its stepper, then the window's Lowest and Range, and Mode.

use bl_engine::{DeviceType, NoteBuffer, NoteEvent, NoteKind, Registry};
use bl_model::device_layout::{BODY_HEIGHT, Control, ControlBand, LayoutNode, Widget};
use bl_model::{DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::midi_fx::{Key, Router, key_choice, scale_in_use, use_current_scale, value};

/// The Pitch device type's id.
pub const LIFT: &str = "beatlayer.lift";

/// The transposition, in semitones.
pub const PITCH: ParamId = ParamId(0);
/// How far Step Up and Step Down move the transposition, in semitones.
pub const STEP_WIDTH: ParamId = ParamId(1);
/// The lowest key of the window.
pub const LOWEST: ParamId = ParamId(2);
/// How many keys the window spans above its lowest.
pub const RANGE: ParamId = ParamId(3);
/// What happens to a note outside the window.
pub const MODE: ParamId = ParamId(4);
/// Reads Pitch and Step Width in degrees of the device's current scale
/// (the transposing spec, R12); the title bar draws it.
pub const USE_SCALE: ParamId = ParamId(5);

/// The most degrees Pitch and Step Width count with Use Current Scale on.
const MOST_DEGREES: i32 = 30;

/// The Mode choices.
pub const MODES: [&str; 3] = ["Block", "Fold", "Limit"];

/// Pitch's panel as Live draws it: the knob, the stepper under it that moves
/// Pitch by Step Width, the window's two boxes and Mode.
fn layout() -> DeviceLayout {
    let at = |param, widget, x, y| Control::new(param, widget, x, y);
    DeviceLayout::new(LayoutNode::Band(ControlBand::free(
        111.0,
        BODY_HEIGHT,
        vec![
            at(PITCH, Widget::Knob, 35.0, 10.0),
            at(STEP_WIDTH, Widget::Stepper, 22.0, 81.0).stepping(PITCH),
            at(LOWEST, Widget::Number, 5.0, 103.0).sized(32.0, 21.0),
            at(RANGE, Widget::Slider, 58.0, 103.0).sized(48.0, 21.0),
            at(MODE, Widget::Chooser, 5.0, 131.0).sized(101.0, 21.0),
        ],
    )))
}

/// Pitch's device type.
#[must_use]
pub fn lift_type() -> DeviceType {
    DeviceType::native(LIFT, "Pitch", DeviceRole::MidiEffect).with_description(
        DeviceDescription::new(vec![
            ParamDescriptor::integer(PITCH, "Pitch", (-128, 128), 0, ParamUnit::Semitones)
                .describe("Moves every note by this many semitones."),
            ParamDescriptor::integer(STEP_WIDTH, "Step Width", (1, 48), 12, ParamUnit::Semitones)
                .describe("How far the buttons beside it, − and +, move Pitch."),
            key_choice(LOWEST, "Lowest", 0)
                .describe("The lowest key a note may sound on after it is moved."),
            ParamDescriptor::integer(RANGE, "Range", (0, 127), 127, ParamUnit::Semitones)
                .describe("How many keys above Lowest a note may sound on."),
            ParamDescriptor::choice(MODE, "Mode", &MODES, 0).describe(
                "What happens to a note outside the window: Block drops it, Fold moves it by \
                 octaves into the window, Limit moves it to the window's nearest edge.",
            ),
            ParamDescriptor::toggle(USE_SCALE, "Use Current Scale", false).describe(
                "Reads Pitch and Step Width in degrees of the current scale, from -30 to 30, \
                 instead of semitones.",
            ),
        ])
        .with_title_switch(USE_SCALE)
        .with_layout(layout())
        .with_banks(&[("Pitch", &["Pitch", "Step Width", "Lowest", "Range", "Mode"])]),
    )
}

/// Adds Pitch to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, lift_type(), |_, _| Lift {
        router: Router::new(),
    });
}

/// Where Pitch sends a note whose move gives `moved`, into the window from
/// `lowest` up to `highest`, or `None` if the note is blocked.
fn lifted(moved: i32, lowest: i32, highest: i32, mode: usize) -> Option<u8> {
    if (lowest..=highest).contains(&moved) {
        return Some(moved as u8);
    }
    match mode {
        1 => {
            // Fold by octaves; a window narrower than an octave may have no
            // place for the note.
            let mut at = moved;
            while at > highest {
                at -= 12;
            }
            while at < lowest {
                at += 12;
            }
            (at <= highest).then_some(at as u8)
        }
        2 => Some(moved.clamp(lowest, highest) as u8),
        _ => None,
    }
}

struct Lift {
    router: Router,
}

impl NativeDevice for Lift {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let degrees = use_current_scale(audio, params, USE_SCALE);
        let pitch = value(params, PITCH) as i32;
        let pitch = if degrees {
            pitch.clamp(-MOST_DEGREES, MOST_DEGREES)
        } else {
            pitch
        };
        let lowest = value(params, LOWEST) as i32;
        let highest = (lowest + value(params, RANGE) as i32).min(127);
        let mode = value(params, MODE) as usize;
        for note in audio.notes() {
            match note.kind {
                NoteKind::On => {
                    // Spec: midi-transpose R12, R13. The move counts degrees
                    // of the current scale, and the window takes the key it
                    // gives.
                    let moved = scale_in_use(audio, params, USE_SCALE, note.offset)
                        .move_off(note.key, pitch);
                    let made = lifted(moved, lowest, highest, mode).map(|key| Key {
                        key,
                        ..Key::of(&note)
                    });
                    if self.router.track(&note, made)
                        && let Some(made) = made
                    {
                        let moved = NoteEvent {
                            key: made.key,
                            ..note
                        };
                        self.router.send_now(audio, moved, Key::of(&note));
                    }
                }
                NoteKind::Off | NoteKind::Choke => self.router.end(audio, &note, note.velocity),
                // Spec: midi-transpose R14
                NoteKind::Expression { .. } => {
                    let scale = scale_in_use(audio, params, USE_SCALE, note.offset);
                    self.router
                        .expression_by(audio, &note, |from, to, bend| scale.bend(from, to, bend));
                }
                // Spec: device-framework R39. A channel message passes through.
                _ => audio.send(note),
            }
        }
        self.router.finish(audio);
    }

    fn release(&mut self, out: &mut NoteBuffer) {
        self.router.release(out);
    }

    fn dropped_notes(&self) -> u64 {
        self.router.dropped()
    }
}

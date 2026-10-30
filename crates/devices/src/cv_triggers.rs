//! CV Triggers (WP-7.17; the CV tools spec, R16, R17, R18): an instrument that
//! turns a MIDI track's notes into pulses and gates for drum modules. It has
//! eight outputs, each answering one key with a Mode (a pulse of a fixed
//! Length, or a gate held as long as the key), a Level and a Velocity switch
//! that scales the level by the note's velocity. Its sound comes back through
//! the External Instrument's Audio From, Gain and Hardware Latency, as CV
//! Instrument's does.

use bl_engine::{DeviceType, NoteBuffer, NoteEvent, NoteKind, Registry};
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};

use crate::cv::{self, GAIN, frames_of, gap_frames, sample, volts_display};
use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::hardware::play_return;

/// CV Triggers' type id.
pub const CV_TRIGGERS: &str = "beatlayer.cv_triggers";

/// How many outputs it has.
pub const OUTPUTS: usize = 8;

/// The outputs' names, in the order the choosers list them.
pub const OUTPUT_NAMES: [&str; OUTPUTS] = [
    "Trigger 1",
    "Trigger 2",
    "Trigger 3",
    "Trigger 4",
    "Trigger 5",
    "Trigger 6",
    "Trigger 7",
    "Trigger 8",
];

/// The key the first output answers on a new device: C1 (36).
const FIRST_KEY: i64 = 36;

/// What an output does when its key is played.
pub const MODES: [&str; 2] = ["Trigger", "Gate"];

/// One of an output's parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// The key it answers.
    Note = 0,
    /// Trigger or Gate.
    Mode = 1,
    /// How long a Trigger lasts.
    Length = 2,
    /// The voltage it writes.
    Level = 3,
    /// Whether the note's velocity scales the level.
    Velocity = 4,
}

/// Output `output`'s (0 to 7) parameter `field`.
#[must_use]
pub const fn param(output: usize, field: Field) -> ParamId {
    ParamId(1 + output as u32 * 5 + field as u32)
}

/// CV Triggers' type.
#[must_use]
pub fn cv_triggers_type() -> DeviceType {
    let mut params = vec![cv::gain()];
    for output in 0..OUTPUTS {
        let name = |what: &str| format!("Trigger {} {what}", output + 1);
        params.extend([
            ParamDescriptor::integer(
                param(output, Field::Note),
                &name("Note"),
                (0, 127),
                FIRST_KEY + output as i64,
                ParamUnit::None,
            )
            .describe("The key the output answers."),
            ParamDescriptor::choice(param(output, Field::Mode), &name("Mode"), &MODES, 0)
                .describe("Trigger: a pulse of a fixed length. Gate: high while the key is held."),
            ParamDescriptor::continuous(
                param(output, Field::Length),
                &name("Length"),
                (1.0, 100.0),
                10.0,
                ParamUnit::Milliseconds,
            )
            .decimals(0)
            .describe("How long a Trigger pulse lasts."),
            ParamDescriptor::continuous(
                param(output, Field::Level),
                &name("Level"),
                (0.0, 10.0),
                5.0,
                ParamUnit::None,
            )
            .decimals(2)
            .describe("The voltage the output writes, in volts."),
            ParamDescriptor::toggle(param(output, Field::Velocity), &name("Velocity"), false)
                .describe("Scales the level by the velocity of the note."),
        ]);
    }
    // A bank per field, the eight outputs across it; Gain on its own.
    let fields = ["Level", "Note", "Mode", "Length", "Velocity"];
    let names: Vec<Vec<String>> = fields
        .iter()
        .map(|field| {
            (1..=OUTPUTS)
                .map(|n| format!("Trigger {n} {field}"))
                .collect()
        })
        .collect();
    let refs: Vec<Vec<&str>> = names
        .iter()
        .map(|bank| bank.iter().map(String::as_str).collect())
        .collect();
    let mut banks: Vec<(&str, &[&str])> = fields
        .iter()
        .zip(&refs)
        .map(|(field, bank)| (*field, bank.as_slice()))
        .collect();
    banks.push(("Gain", &["Gain"]));
    DeviceType::native(CV_TRIGGERS, "CV Triggers", DeviceRole::Instrument)
        .with_cv(&OUTPUT_NAMES, &[])
        .with_description(DeviceDescription::new(params).with_banks(&banks))
}

/// Adds CV Triggers to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, cv_triggers_type(), |_, config| {
        CvTriggers::new(config.sample_rate.as_f64())
    });
}

/// CV Triggers' display (R9): the voltage each output last wrote.
#[must_use]
pub fn display(device: DeviceId, data: Option<&[f32]>) -> DeviceDisplay {
    volts_display(device, OUTPUT_NAMES.map(str::to_owned), data)
}

/// One output's state.
#[derive(Clone, Copy, Default)]
struct Out {
    /// Frames of the 1 ms gap left.
    gap: u32,
    /// Frames of a Trigger's pulse left.
    pulse: u32,
    /// Whether a Gate is held.
    held: bool,
    /// The voltage the pulse or gate writes, fixed at its note on.
    level: f64,
}

impl Out {
    fn high(&self) -> bool {
        self.pulse > 0 || self.held
    }
}

struct CvTriggers {
    rate: f64,
    outs: [Out; OUTPUTS],
    was_playing: bool,
    shown: [f32; OUTPUTS],
}

impl CvTriggers {
    fn new(rate: f64) -> Self {
        Self {
            rate,
            outs: [Out::default(); OUTPUTS],
            was_playing: false,
            shown: [0.0; OUTPUTS],
        }
    }

    fn silence(&mut self) {
        self.outs = [Out::default(); OUTPUTS];
    }
}

impl NativeDevice for CvTriggers {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let ramp = |output, field| {
            params.ramp(
                params
                    .index(param(output, field))
                    .expect("the description holds it"),
            )
        };
        play_return(
            audio,
            params.ramp(params.index(GAIN).expect("the description holds it")),
        );
        let ramps: [[&[f32]; 5]; OUTPUTS] = std::array::from_fn(|output| {
            [
                ramp(output, Field::Note),
                ramp(output, Field::Mode),
                ramp(output, Field::Length),
                ramp(output, Field::Level),
                ramp(output, Field::Velocity),
            ]
        });
        let playing = audio.playing();
        // Spec: cv-tools R18. The transport stopping takes every output to 0 V.
        if self.was_playing && !playing {
            self.silence();
        }
        self.was_playing = playing;
        let gap = gap_frames(self.rate);
        let mut notes = audio.notes().peekable();
        for f in 0..audio.frames() {
            while let Some(note) = notes.next_if(|n| n.offset as usize == f) {
                let on = match note.kind {
                    NoteKind::On => true,
                    NoteKind::Off | NoteKind::Choke => false,
                    _ => continue,
                };
                self.take(&note, on, f, &ramps, gap);
            }
            for (output, out) in self.outs.iter_mut().enumerate() {
                let volts = if out.gap > 0 {
                    out.gap -= 1;
                    0.0
                } else if out.high() {
                    if out.pulse > 0 {
                        out.pulse -= 1;
                    }
                    out.level
                } else {
                    0.0
                };
                if let Some(buffer) = audio.cv_out(output) {
                    buffer[f] = sample(volts);
                }
                self.shown[output] = volts.clamp(-10.0, 10.0) as f32;
            }
        }
    }

    fn release(&mut self, _: &mut NoteBuffer) {
        // Spec: cv-tools R18. Switched off, removed or moved, every output
        // falls.
        self.silence();
    }

    fn display_len(&self) -> usize {
        OUTPUTS
    }

    fn display(&mut self, out: &mut [f32]) {
        out.copy_from_slice(&self.shown);
    }
}

impl CvTriggers {
    /// Takes a note on or off at frame `f` (R17): every output whose key it
    /// is answers.
    fn take(
        &mut self,
        note: &NoteEvent,
        on: bool,
        f: usize,
        ramps: &[[&[f32]; 5]; OUTPUTS],
        gap: u32,
    ) {
        for (out, ramp) in self.outs.iter_mut().zip(ramps) {
            if ramp[Field::Note as usize][f] as u8 != note.key {
                continue;
            }
            let gate = ramp[Field::Mode as usize][f] >= 0.5;
            if !on {
                out.held = false;
                continue;
            }
            // A note on while the output is high takes it down for 1 ms.
            if out.high() {
                out.gap = gap;
            }
            let scale = if ramp[Field::Velocity as usize][f] >= 0.5 {
                f64::from(note.velocity)
            } else {
                1.0
            };
            out.level = f64::from(ramp[Field::Level as usize][f]) * scale;
            if gate {
                out.held = true;
                out.pulse = 0;
            } else {
                out.held = false;
                out.pulse = frames_of(self.rate, f64::from(ramp[Field::Length as usize][f])).max(1);
            }
        }
    }
}

//! CV Instrument (WP-7.17; the CV tools spec, R10 to R15, R18, R19): an
//! instrument that plays one modular voice from a MIDI track's notes. Pitch
//! writes the key's pitch under the set's tuning, one volt to the octave, Gate
//! is high while a key is held, and Mod 1 and Mod 2 follow a source between
//! two voltages. Its sound comes back through the External Instrument's
//! Audio From, Gain and Hardware Latency.
//!
//! The engine does the routing: it adds each output to the interface channel
//! the device's settings name ([`bl_model::External`]) and maps the return into
//! the block. The node writes the outputs in volts divided by ten (R1),
//! calibrates the oscillator on Pitch against its returning sound with the
//! Tuner's detector (R14), and plays its pitches through the calibration
//! it holds (R12).

use bl_engine::{DeviceType, KeyPitches, NodeConfig, NodeData, NoteBuffer, NoteEvent, NoteKind};
use bl_engine::{NoteExpression, Registry};
use bl_model::{
    CvCalibration, Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceRole, ParamDescriptor,
    ParamId, ParamUnit,
};

use crate::cv::{self, GAIN, frames_of, gap_frames, sample, volts_display};
use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::hardware::play_return;
use crate::midi_fx::Key;
use crate::play_mod::{SOURCES, Sources};
use crate::signal_mod::Held;
use crate::tuner::Tuner;

/// CV Instrument's type id.
pub const CV_INSTRUMENT: &str = "beatlayer.cv_instrument";

/// The CV outputs, in the order the choosers list them (R10).
pub const OUTPUTS: [&str; 4] = ["Pitch", "Gate", "Mod 1", "Mod 2"];

/// Which key sounds of those held (R11): Last, Low or High.
pub const NOTE_PRIORITY: ParamId = ParamId(1);
/// Transpose, −48 to +48 semitones.
pub const TRANSPOSE: ParamId = ParamId(2);
/// Fine, −100 to +100 cents.
pub const FINE: ParamId = ParamId(3);
/// Bend Range, 0 to 24 semitones.
pub const BEND_RANGE: ParamId = ParamId(4);
/// Glide, 0 to 5,000 ms.
pub const GLIDE: ParamId = ParamId(5);
/// Gate Level, 0 to 10 V.
pub const GATE_LEVEL: ParamId = ParamId(6);
/// Retrigger: a new key takes the gate down for 1 ms first.
pub const RETRIGGER: ParamId = ParamId(7);
/// The first of the six parameters of Mod 1 (Source, Min, Max) and, after
/// them, Mod 2's.
const MOD: u32 = 8;

/// The ids of Mod `n`'s (0 or 1) Source, Min and Max.
#[must_use]
pub const fn mod_params(n: u32) -> [ParamId; 3] {
    [
        ParamId(MOD + n * 3),
        ParamId(MOD + n * 3 + 1),
        ParamId(MOD + n * 3 + 2),
    ]
}

/// The Note Priority choices.
pub const PRIORITIES: [&str; 3] = ["Last", "Low", "High"];

/// The steps a calibration takes (R14): the whole volts from −2 V to +5 V.
pub const STEPS: usize = 8;
/// The first step's voltage.
pub const FIRST_STEP_VOLTS: f64 = -2.0;
/// How long each step is held, in milliseconds.
const STEP_MS: f64 = 250.0;
/// How far into a step the measuring starts: the last 150 ms of 250 ms.
const MEASURE_FROM_MS: f64 = 100.0;
/// The pitches the detector reads (the Tuner spec, R2).
const READABLE: (f64, f64) = (41.0, 2000.0);
/// How far the detections of a step may spread and still be steady, as a
/// frequency ratio (25 cents).
const STEADY: f64 = 1.0145;

/// What the shell sends the node as node data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CvCommand {
    /// Starts a calibration (R14).
    Calibrate,
}

/// Where the node's display data holds what the shell reads: the four
/// outputs' voltages, then the calibration's state, its run number and the
/// step it is on, then each step's pitch in hertz (0 for none).
pub const DISPLAY_LEN: usize = 7 + STEPS;
/// The display slot of the calibration's state.
pub const STATE_AT: usize = 4;
/// The display slot of the calibration's run number, which counts up as each
/// calibration starts.
pub const RUN_AT: usize = 5;
/// The display slot of the step the calibration is on.
pub const STEP_AT: usize = 6;
/// The display slot of the first step's pitch.
pub const PITCHES_AT: usize = 7;
/// No calibration has run.
pub const IDLE: f32 = 0.0;
/// A calibration is running.
pub const RUNNING: f32 = 1.0;
/// The last calibration has finished.
pub const FINISHED: f32 = 2.0;

/// The keys it follows at once.
const VOICES: usize = 32;

/// CV Instrument's type.
#[must_use]
pub fn cv_instrument_type() -> DeviceType {
    let sources: Vec<&str> = (0..SOURCES.len())
        .filter(|&at| at != 6 && at != 7)
        .map(|at| SOURCES[at])
        .collect();
    let volts = |id, name: &str, range: (f64, f64), default: f64, text: &str| {
        ParamDescriptor::continuous(id, name, range, default, ParamUnit::None)
            .decimals(2)
            .describe(text)
    };
    let mut params = vec![
        cv::gain(),
        ParamDescriptor::choice(NOTE_PRIORITY, "Note Priority", &PRIORITIES, 0)
            .describe("Which of the keys held sounds: the last, the lowest or the highest."),
        ParamDescriptor::integer(TRANSPOSE, "Transpose", (-48, 48), 0, ParamUnit::Semitones)
            .describe("Moves the pitch by whole semitones."),
        ParamDescriptor::continuous(FINE, "Fine", (-100.0, 100.0), 0.0, ParamUnit::Cents)
            .decimals(0)
            .describe("Moves the pitch by cents."),
        ParamDescriptor::continuous(
            BEND_RANGE,
            "Bend Range",
            (0.0, 24.0),
            2.0,
            ParamUnit::Semitones,
        )
        .decimals(0)
        .describe("How far the pitch bend moves the pitch, in semitones."),
        ParamDescriptor::continuous(GLIDE, "Glide", (0.0, 5_000.0), 0.0, ParamUnit::Milliseconds)
            .decimals(0)
            .describe("How long the pitch takes to glide to a new key."),
        volts(
            GATE_LEVEL,
            "Gate Level",
            (0.0, 10.0),
            5.0,
            "The voltage Gate writes while a key is held, in volts.",
        ),
        ParamDescriptor::toggle(RETRIGGER, "Retrigger", true)
            .describe("A new key takes the gate down for 1 ms so the envelope starts again."),
    ];
    for n in 0..2u32 {
        let [source, min, max] = mod_params(n);
        let label = n + 1;
        params.push(
            ParamDescriptor::choice(source, &format!("Mod {label} Source"), &sources, n as usize)
                .describe("What Mod follows: a source of the Expression Control."),
        );
        params.push(volts(
            min,
            &format!("Mod {label} Min"),
            (-10.0, 10.0),
            0.0,
            "The voltage at the source's lowest share, in volts.",
        ));
        params.push(volts(
            max,
            &format!("Mod {label} Max"),
            (-10.0, 10.0),
            5.0,
            "The voltage at the source's highest share, in volts.",
        ));
    }
    DeviceType::native(CV_INSTRUMENT, "CV Instrument", DeviceRole::Instrument)
        .with_cv(&OUTPUTS, &[])
        .with_description(DeviceDescription::new(params))
}

/// Adds CV Instrument to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, cv_instrument_type(), |device, config| {
        CvInstrument::new(device, config)
    });
}

/// CV Instrument's display (R9): the voltage each output last wrote, and
/// the calibration's pitches.
#[must_use]
pub fn display(device: DeviceId, data: Option<&[f32]>) -> DeviceDisplay {
    volts_display(device, OUTPUTS.map(str::to_owned), data)
}

/// A key held, with the pitch bend of its own (R19).
#[derive(Clone, Copy)]
struct Voice {
    key: Key,
    bend: f64,
}

/// A calibration at work (R14).
struct Run {
    running: bool,
    finished: bool,
    step: usize,
    /// Frames into the step.
    into: u32,
    detections: [f64; 8],
    heard: usize,
    pitches: [f32; STEPS],
}

impl Run {
    const fn new() -> Self {
        Self {
            running: false,
            finished: false,
            step: 0,
            into: 0,
            detections: [0.0; 8],
            heard: 0,
            pitches: [0.0; STEPS],
        }
    }

    /// The pitch a step gave: the median of what the detector heard in the
    /// measured part, or 0 if it wasn't steady or lies outside what the
    /// detector reads.
    fn result(&self) -> f32 {
        let mut seen = self.detections;
        let seen = &mut seen[..self.heard];
        seen.sort_unstable_by(f64::total_cmp);
        let (Some(&low), Some(&high)) = (seen.first(), seen.last()) else {
            return 0.0;
        };
        let median = seen[seen.len() / 2];
        let steady = seen.len() >= 2
            && low > 0.0
            && high / low < STEADY
            && (READABLE.0..=READABLE.1).contains(&median);
        if steady { median as f32 } else { 0.0 }
    }
}

struct CvInstrument {
    rate: f64,
    keys: KeyPitches,
    calibration: Option<CvCalibration>,
    held: Held,
    sources: Sources,
    voices: Vec<Voice>,
    sounding: Option<Key>,
    gate: bool,
    /// Frames of a retrigger's gap left.
    gap: u32,
    /// The channel's pitch bend, −1 to +1.
    bend: f64,
    /// The pitch written, in volts, and whether any has been.
    volts: f64,
    started: bool,
    /// Frames of a glide left.
    glide: u32,
    was_playing: bool,
    tuner: Tuner,
    run: Run,
    /// The last value written to each output, in volts.
    shown: [f32; 4],
    runs: u32,
}

impl CvInstrument {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        Self {
            rate: config.sample_rate.as_f64(),
            keys: KeyPitches::twelve(),
            calibration: device
                .external
                .as_ref()
                .and_then(|external| external.cv_calibration.clone()),
            held: Held::new(),
            sources: Sources::START,
            voices: Vec::with_capacity(VOICES),
            sounding: None,
            gate: false,
            gap: 0,
            bend: 0.0,
            volts: 0.0,
            started: false,
            glide: 0,
            was_playing: false,
            tuner: Tuner::new(config),
            run: Run::new(),
            shown: [0.0; 4],
            runs: 0,
        }
    }

    /// Every key lets go and the gate falls.
    fn silence(&mut self) {
        self.voices.clear();
        self.held.clear();
        self.sounding = None;
        self.gate = false;
        self.gap = 0;
    }

    /// The key that sounds of those held, by `priority` (R11).
    fn pick(&self, priority: usize) -> Option<Key> {
        let voices = self.voices.iter();
        match priority {
            1 => voices.min_by_key(|v| v.key.key),
            2 => voices.max_by_key(|v| v.key.key),
            _ => voices.last(),
        }
        .map(|v| v.key)
    }

    /// The pitch in volts for the sounding key now (R11, R12).
    fn target(&self, fine: f64, transpose: f64, range: f64) -> Option<f64> {
        let sounding = self.sounding?;
        let own = self
            .voices
            .iter()
            .find(|v| v.key == sounding)
            .map_or(0.0, |v| v.bend);
        let pitch =
            self.keys.pitch(sounding.key)? + transpose + fine / 100.0 + (self.bend + own) * range;
        Some(match &self.calibration {
            Some(calibration) => calibration.volts_for(440.0 * ((pitch - 69.0) / 12.0).exp2()),
            None => (pitch - 60.0) / 12.0,
        })
    }

    /// Takes a note or message at its frame (R11, R13, R19).
    fn take(&mut self, note: &NoteEvent, priority: usize, retrigger: bool, glide: u32) {
        match note.kind {
            NoteKind::On => {
                // A key past the tuning's range plays nothing.
                if self.keys.pitch(note.key).is_none() || self.voices.len() >= VOICES {
                    return;
                }
                self.held.on(note);
                self.sources.take(note, &self.held);
                self.voices.retain(|v| !v.key.names(note));
                self.voices.push(Voice {
                    key: Key::of(note),
                    bend: 0.0,
                });
                let before = self.sounding;
                self.sounding = self.pick(priority);
                if self.sounding != before {
                    if self.gate && retrigger {
                        self.gap = gap_frames(self.rate);
                    }
                    self.glide = glide;
                }
                self.gate = true;
            }
            NoteKind::Off | NoteKind::Choke => {
                self.held.off(note);
                let before = self.sounding;
                self.voices.retain(|v| !v.key.names(note));
                self.sounding = self.pick(priority);
                if self.voices.is_empty() {
                    self.gate = false;
                    self.gap = 0;
                } else if self.sounding != before {
                    self.glide = glide;
                }
            }
            NoteKind::PitchBend { value } => {
                self.bend = f64::from(value);
                self.sources.take(note, &self.held);
            }
            NoteKind::Expression {
                expression: NoteExpression::Bend,
                value,
            } => {
                if let Some(voice) = self.voices.iter_mut().find(|v| v.key.names(note)) {
                    voice.bend = f64::from(value);
                }
            }
            _ => self.sources.take(note, &self.held),
        }
    }

    /// Feeds one frame of the returning sound to the calibration, and moves
    /// it on a frame (R14). Returns the pitch voltage of the step it is on.
    fn calibrate(&mut self, heard: f32) -> f64 {
        let (steps, measure) = (
            frames_of(self.rate, STEP_MS),
            frames_of(self.rate, MEASURE_FROM_MS),
        );
        let volts = FIRST_STEP_VOLTS + self.run.step as f64;
        if let Some(found) = self.tuner.listen(&[heard])
            && self.run.into >= measure
            && self.run.heard < self.run.detections.len()
        {
            self.run.detections[self.run.heard] = found.unwrap_or(0.0);
            self.run.heard += 1;
        }
        self.run.into += 1;
        if self.run.into >= steps {
            self.run.pitches[self.run.step] = self.run.result();
            self.run.step += 1;
            self.run.into = 0;
            self.run.heard = 0;
            if self.run.step == STEPS {
                self.run.running = false;
                self.run.finished = true;
            }
        }
        volts
    }
}

impl NativeDevice for CvInstrument {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let at = |id: ParamId| params.index(id).expect("the description holds it");
        let ramp = |id: ParamId| params.ramp(at(id));
        play_return(audio, ramp(GAIN));
        let (priority, transpose, fine, range) = (
            ramp(NOTE_PRIORITY),
            ramp(TRANSPOSE),
            ramp(FINE),
            ramp(BEND_RANGE),
        );
        let (glide, level, retrigger) = (ramp(GLIDE), ramp(GATE_LEVEL), ramp(RETRIGGER));
        let mods = [mod_params(0), mod_params(1)].map(|ids| ids.map(ramp));
        let heard = audio.external(0);
        let playing = audio.playing();
        // Spec: cv-tools R18. The transport stopping takes the gate to 0 V.
        if self.was_playing && !playing {
            self.silence();
        }
        self.was_playing = playing;
        let mut notes = audio.notes().peekable();
        for f in 0..audio.frames() {
            while let Some(note) = notes.next_if(|n| n.offset as usize == f) {
                // Notes reaching the device while it calibrates are ignored.
                if !self.run.running {
                    let frames = frames_of(self.rate, f64::from(glide[f]));
                    self.take(&note, priority[f] as usize, retrigger[f] >= 0.5, frames);
                }
            }
            let calibrating = self.run.running;
            let pitch = if calibrating {
                self.glide = 0;
                let volts = self.calibrate(heard.map_or(0.0, |heard| heard[f]));
                self.started = true;
                Some(volts)
            } else {
                self.target(
                    f64::from(fine[f]),
                    f64::from(transpose[f]),
                    f64::from(range[f]),
                )
            };
            // A straight line to the target in volts over the glide: the
            // step comes after the frame is written, so the line starts on
            // the frame the key arrives, where the pitch was.
            let gliding = self.started && self.glide > 0;
            let target = pitch.unwrap_or(self.volts);
            if !gliding {
                self.volts = target;
                self.started |= pitch.is_some();
            }
            let line = (target - self.volts) / f64::from(self.glide.max(1));
            // The gate is high while a key is held, or the calibration runs.
            let high = (self.gate || calibrating) && self.gap == 0;
            self.gap = self.gap.saturating_sub(1);
            let gate = if high { f64::from(level[f]) } else { 0.0 };
            let mut volts = [self.volts, gate, 0.0, 0.0];
            for (out, [source, min, max]) in volts[2..].iter_mut().zip(&mods) {
                let kind = source[f] as usize;
                let kind = if kind < 6 { kind } else { kind + 2 };
                let (min, max) = (f64::from(min[f]), f64::from(max[f]));
                *out = min + self.sources.share(kind) * (max - min);
            }
            for (output, volts) in volts.into_iter().enumerate() {
                if let Some(out) = audio.cv_out(output) {
                    out[f] = sample(volts);
                }
                self.shown[output] = volts.clamp(-10.0, 10.0) as f32;
            }
            if gliding {
                self.volts += line;
                self.glide -= 1;
            }
        }
    }

    fn release(&mut self, _: &mut NoteBuffer) {
        // Spec: cv-tools R18. Switched off, removed or moved, the voice lets
        // go and the gate falls.
        self.silence();
    }

    fn receive_data(&mut self, data: NodeData) {
        if let Some(keys) = data.downcast_ref::<KeyPitches>() {
            self.keys = *keys;
        } else if data.downcast_ref::<CvCommand>() == Some(&CvCommand::Calibrate) {
            self.silence();
            self.run = Run::new();
            self.run.running = true;
            self.runs += 1;
        }
    }

    fn display_len(&self) -> usize {
        DISPLAY_LEN
    }

    fn display(&mut self, out: &mut [f32]) {
        out[..4].copy_from_slice(&self.shown);
        out[STATE_AT] = if self.run.running {
            RUNNING
        } else if self.run.finished {
            FINISHED
        } else {
            IDLE
        };
        out[RUN_AT] = self.runs as f32;
        out[STEP_AT] = self.run.step as f32;
        out[PITCHES_AT..].copy_from_slice(&self.run.pitches);
    }
}

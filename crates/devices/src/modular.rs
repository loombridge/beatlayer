//! Modular, Beatlayer's native modular device (WP-9.36; the modular device
//! spec): a patch of blocks joined by cables, compiled off the audio thread
//! into a schedule and swapped in at a block boundary with the kept blocks'
//! state carried over. An instrument patch plays polyphonically, each voice
//! running its own copy of the blocks that depend on Note In.
//!
//! The device's patch parameters are 64 numbered parameters: the descriptors
//! of a device type are the same for every device of the type, so the name,
//! range, unit and default of the setting each one stands for are the
//! patch's (`modular.patch` reports them), and each parameter's value runs
//! from 0 to 1 across its setting's range.

use std::sync::Arc;

use bl_engine::{
    DeviceData, DeviceType, NodeConfig, NodeData, NoteBuffer, NoteEvent, NoteExpression, NoteKind,
    Registry,
};
use bl_model::modular::{self, DEFAULT_VOICES, MAX_PARAMS, MAX_VOICES, VOICES, slot_param};
use bl_model::{Device, DeviceDescription, DeviceKind, DeviceRole, ParamDescriptor, ParamUnit};

use crate::framework::{Audio, NativeDevice, ParamBank, add_with_data};
use crate::midi_fx::{Key, Router};

mod blocks;
mod schedule;

use blocks::Frame;
use schedule::{Handoff, Run, Schedule, at};

/// How long a stolen voice fades out before the new note takes its place
/// (R5), in seconds.
const STEAL_FADE: f64 = 0.005;
/// How long a released voice must be silent before it is free, in seconds.
const QUIET: f64 = 0.1;
/// A voice below this level to the rest of the patch is silent.
const SILENT: f32 = 1e-5;
/// A per-note bend's range, in semitones, as MPE's default.
const BEND_SEMITONES: f64 = 48.0;

/// The three device types (R1).
fn device_type(id: &str, name: &str, role: DeviceRole) -> DeviceType {
    let mut params: Vec<ParamDescriptor> = (1..=MAX_PARAMS as u8)
        .map(|slot| {
            ParamDescriptor::continuous(
                slot_param(slot),
                &format!("Patch Parameter {slot}"),
                (0.0, 1.0),
                0.0,
                ParamUnit::None,
            )
            .describe(
                "A setting of a block the patch made a patch parameter: its value runs from 0 \
                 to 1 across the setting's range. `modular.patch` says which setting it is.",
            )
        })
        .collect();
    if role == DeviceRole::Instrument {
        params.push(
            ParamDescriptor::integer(
                VOICES,
                "Voices",
                (1, i64::from(MAX_VOICES)),
                i64::from(DEFAULT_VOICES),
                ParamUnit::None,
            )
            .describe(
                "How many voices play. A new note past it takes the oldest voice, which \
                     fades out over 5 ms.",
            ),
        );
    }
    let device_type =
        DeviceType::native(id, name, role).with_description(DeviceDescription::new(params));
    if role == DeviceRole::AudioEffect {
        device_type.with_sidechain()
    } else {
        device_type
    }
}

/// Adds the three Modular device types to `registry`.
pub fn register(registry: &mut Registry) {
    for (id, name, role) in [
        (
            modular::INSTRUMENT,
            "Modular Instrument",
            DeviceRole::Instrument,
        ),
        (modular::EFFECT, "Modular Effect", DeviceRole::AudioEffect),
        (
            modular::MIDI_EFFECT,
            "Modular MIDI Effect",
            DeviceRole::MidiEffect,
        ),
    ] {
        add_with_data(
            registry,
            device_type(id, name, role),
            Modular::new,
            Some(Arc::new(device_data)),
        );
        // The voices a schedule holds are made with it (R5).
        if role == DeviceRole::Instrument {
            registry.rebuilds_data_on(id, &[VOICES]);
        }
    }
}

/// The schedule `device`'s patch compiles to, for `config`'s rate (R4).
fn compile(device: &Device, config: &NodeConfig) -> Schedule {
    let instrument = matches!(&device.kind,
        DeviceKind::Native { device_type } if device_type == modular::INSTRUMENT);
    let voices = device
        .params
        .get(&VOICES)
        .map_or(f64::from(DEFAULT_VOICES), |v| *v)
        .round() as usize;
    Schedule::build(
        device.patch.as_ref().unwrap_or(&modular::Patch::default()),
        config.sample_rate.as_f64(),
        voices,
        instrument,
    )
}

/// The schedule of `device`'s patch, built off the audio thread for its node
/// to swap in when the patch changes (R4).
#[must_use]
pub fn device_data(
    device: &Device,
    config: &NodeConfig,
    _file: bl_engine::FileOf<'_>,
) -> DeviceData {
    let schedule = compile(device, config);
    let mut data = DeviceData::default();
    data.items
        .push(Box::new(move |gc| gc.data(Handoff::new(schedule))));
    data
}

/// One voice.
#[derive(Clone, Copy, Default)]
struct Voice {
    key: Key,
    age: u64,
    released: bool,
    /// Frames the released voice has been silent.
    quiet: u32,
    /// Frames of its fade left while it is being stolen, and the note that
    /// takes its place.
    fade: u32,
    next: Option<NoteEvent>,
}

/// Modular's audio-thread half.
pub struct Modular {
    role: DeviceRole,
    rate: f64,
    schedule: Box<Schedule>,
    voices: [Voice; MAX_VOICES as usize],
    active: u32,
    gain: [f32; MAX_VOICES as usize],
    heard: [f32; MAX_VOICES as usize],
    clock: u64,
    router: Router,
}

impl Modular {
    /// Makes a Modular for `device`, compiling its patch. Every state and
    /// buffer is made here, off the audio thread.
    #[must_use]
    pub fn new(device: &Device, config: &NodeConfig) -> Self {
        Self {
            role: device.role,
            rate: config.sample_rate.as_f64(),
            schedule: Box::new(compile(device, config)),
            voices: [Voice::default(); MAX_VOICES as usize],
            active: 0,
            gain: [1.0; MAX_VOICES as usize],
            heard: [0.0; MAX_VOICES as usize],
            clock: 0,
            router: Router::new(),
        }
    }

    /// Starts `note` on `lane`: its blocks begin at rest and Note In shows
    /// the note (R13).
    fn start(&mut self, lane: usize, note: NoteEvent) {
        let sched = &mut *self.schedule;
        for step in sched.steps.iter_mut().filter(|s| s.voice) {
            step.states[lane].reset();
        }
        let lanes = sched.lanes;
        for &base in &sched.note_ins {
            let mut put = |k: usize, v: f32| sched.values[at(base + k, lanes, lane)] = v;
            put(0, (f32::from(note.key) - 60.0) / 12.0);
            put(1, note.velocity);
            put(2, 1.0);
            put(3, 0.0);
            put(4, 0.0);
        }
        self.voices[lane] = Voice {
            key: Key::of(&note),
            age: self.clock,
            ..Voice::default()
        };
        self.clock += 1;
        self.active |= 1 << lane;
        self.gain[lane] = 1.0;
    }

    /// Plays `note` in a free voice, or takes the oldest one (R5).
    fn note_on(&mut self, note: NoteEvent, limit: usize) {
        let lanes = self.schedule.lanes;
        if self.schedule.note_ins.is_empty() {
            return;
        }
        let limit = limit.min(lanes);
        if let Some(lane) = (0..limit).find(|lane| self.active & (1 << lane) == 0) {
            self.start(lane, note);
            return;
        }
        // The oldest voice that isn't already being stolen fades out and
        // starts the note when it is gone.
        let oldest = (0..limit)
            .filter(|&lane| self.voices[lane].fade == 0)
            .min_by_key(|&lane| self.voices[lane].age);
        if let Some(lane) = oldest {
            self.voices[lane].fade = ((self.rate * STEAL_FADE) as u32).max(1);
            self.voices[lane].next = Some(note);
        }
    }

    /// The voice playing `note`'s key, if any.
    fn find(&self, note: &NoteEvent) -> Option<usize> {
        (0..self.schedule.lanes).find(|&lane| {
            self.active & (1 << lane) != 0
                && !self.voices[lane].released
                && self.voices[lane].fade == 0
                && self.voices[lane].key.names(note)
        })
    }

    fn note_off(&mut self, note: &NoteEvent) {
        // A note stolen before it started never plays.
        for voice in &mut self.voices {
            if voice.next.is_some_and(|n| Key::of(&n).names(note)) {
                voice.next = None;
            }
        }
        if let Some(lane) = self.find(note) {
            let sched = &mut *self.schedule;
            let lanes = sched.lanes;
            for &base in &sched.note_ins {
                sched.values[at(base + 2, lanes, lane)] = 0.0;
            }
            self.voices[lane].released = true;
        }
    }

    fn expression(&mut self, note: &NoteEvent, expression: NoteExpression, value: f32) {
        let Some(lane) = self.find(note) else { return };
        let sched = &mut *self.schedule;
        let lanes = sched.lanes;
        for &base in &sched.note_ins {
            match expression {
                NoteExpression::Pressure => sched.values[at(base + 3, lanes, lane)] = value,
                NoteExpression::Slide => sched.values[at(base + 4, lanes, lane)] = value,
                NoteExpression::Bend => {
                    let key = f64::from(self.voices[lane].key.key);
                    sched.values[at(base, lanes, lane)] =
                        ((key - 60.0) / 12.0 + f64::from(value) * BEND_SEMITONES / 12.0) as f32;
                }
            }
        }
    }

    /// After a frame: fades run down, and released voices that have been
    /// quiet long enough are freed.
    fn after_frame(&mut self) {
        let quiet = (self.rate * QUIET) as u32;
        let mut bits = self.active;
        while bits != 0 {
            let lane = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let voice = &mut self.voices[lane];
            if voice.fade > 0 {
                voice.fade -= 1;
                let steal = ((self.rate * STEAL_FADE) as u32).max(1);
                self.gain[lane] = voice.fade as f32 / steal as f32;
                if voice.fade == 0 {
                    let next = voice.next.take();
                    self.active &= !(1 << lane);
                    if let Some(note) = next {
                        self.start(lane, note);
                    }
                }
            } else if voice.released {
                voice.quiet = if self.heard[lane] < SILENT {
                    voice.quiet + 1
                } else {
                    0
                };
                if voice.quiet >= quiet {
                    self.active &= !(1 << lane);
                }
            }
        }
    }
}

impl NativeDevice for Modular {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        if self.role == DeviceRole::MidiEffect {
            self.pass_notes(audio);
            return;
        }
        let frames = audio.frames();
        let limit = if self.role == DeviceRole::Instrument {
            params.value(MAX_PARAMS).round().max(1.0) as usize
        } else {
            1
        };
        let sample_rate = self.rate;
        let bpm = audio.bpm();
        let channels = audio.channels();
        let mut notes = audio.notes().peekable();
        for f in 0..frames {
            while let Some(note) = notes.next_if(|n| n.offset as usize <= f) {
                match note.kind {
                    NoteKind::On => self.note_on(note, limit),
                    NoteKind::Off | NoteKind::Choke => self.note_off(&note),
                    NoteKind::Expression { expression, value } => {
                        self.expression(&note, expression, value);
                    }
                    _ => {}
                }
            }
            let sched = &mut *self.schedule;
            if !sched.exposed.is_empty() {
                sched.refresh(|index| f64::from(params.ramp(index)[f]));
            }
            let lanes = sched.lanes;
            for &n in &sched.audio_ins {
                let step = &sched.steps[n];
                let out = step.out;
                for side in 0..2 {
                    let sample = if matches!(step.states[0], blocks::State::SidechainIn) {
                        audio
                            .sidechain(side.min(channels - 1))
                            .map_or(0.0, |s| s[f])
                    } else if self.role == DeviceRole::AudioEffect {
                        audio.channel(side.min(channels - 1))[f]
                    } else {
                        0.0
                    };
                    sched.values[at(out + side, lanes, 0)] = sample;
                }
            }
            self.heard = [0.0; MAX_VOICES as usize];
            let mut run = Run {
                frame: Frame {
                    rate: sample_rate,
                    bpm,
                },
                active: self.active,
                gain: &self.gain,
                heard: &mut self.heard,
            };
            let out = sched.frame(&mut run);
            for (side, sample) in out.iter().enumerate().take(channels) {
                audio.channel_mut(side)[f] = *sample;
            }
            self.after_frame();
        }
    }

    fn release(&mut self, out: &mut NoteBuffer) {
        self.router.release(out);
    }

    fn dropped_notes(&self) -> u64 {
        self.router.dropped()
    }

    fn receive_data(&mut self, data: NodeData) {
        // Spec: modular-device R4. A new schedule takes the place of the
        // running one at the start of the block, with the state of the
        // blocks both hold; the schedule it replaces goes back into the data
        // for the drop thread to free.
        if let Some(handoff) = data.downcast_ref::<Handoff>()
            && let Some(mut next) = handoff.take()
        {
            next.take_state_from(&mut self.schedule);
            let lanes = next.lanes;
            let previous = std::mem::replace(&mut self.schedule, next);
            self.active &= if lanes >= 32 {
                u32::MAX
            } else {
                (1 << lanes) - 1
            };
            handoff.give(previous);
        }
    }

    fn latency(&self) -> u32 {
        self.schedule.latency()
    }
}

impl Modular {
    /// A MIDI effect: the notes of a Note In joined to a Note Out go on,
    /// and no others (R13).
    fn pass_notes(&mut self, audio: &mut Audio<'_, '_>) {
        for note in audio.notes() {
            match note.kind {
                NoteKind::On => {
                    if self.schedule.notes_through && self.router.track(&note, Some(Key::of(&note)))
                    {
                        self.router.send_now(audio, note, Key::of(&note));
                    }
                }
                NoteKind::Off | NoteKind::Choke => self.router.end(audio, &note, note.velocity),
                NoteKind::Expression { .. } => {
                    if self.schedule.notes_through {
                        self.router.expression(audio, &note);
                    }
                }
                _ => {
                    if self.schedule.notes_through {
                        audio.send(note);
                    }
                }
            }
        }
        self.router.finish(audio);
    }
}

//! Note Length, the note-length MIDI effect (WP-5.30; the MIDI note length spec): sets
//! how long each note sounds, in milliseconds or beat divisions, or starts
//! notes when keys are released instead of when they are pressed, or
//! latches them.
//!
//! Its look is the framework's generic layout: Beatlayer's own.

use bl_engine::{DeviceType, NoteBuffer, NoteEvent, NoteKind, Registry};
use bl_model::{DeviceDescription, DeviceRole, ParamDescriptor, ParamId, ParamUnit};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::midi_fx::{Key, Router, value};

/// The Note Length device type's id.
pub const HOLD: &str = "beatlayer.hold";

/// Whether notes start when keys go down or when they come up.
pub const TRIGGER: ParamId = ParamId(0);
/// Length in beat divisions rather than milliseconds.
pub const SYNC: ParamId = ParamId(1);
/// The length in milliseconds.
pub const LENGTH_MS: ParamId = ParamId(2);
/// The length in beat divisions.
pub const LENGTH_DIVISION: ParamId = ParamId(3);
/// How much of the length a note sounds for.
pub const GATE: ParamId = ParamId(4);
/// How much the output's velocity follows the key's release velocity.
pub const RELEASE_VELOCITY: ParamId = ParamId(5);
/// How long a held key's velocity takes to fall to nothing.
pub const DECAY_TIME: ParamId = ParamId(6);
/// Makes notes shorter or longer by pitch around middle C.
pub const KEY_SCALE: ParamId = ParamId(7);
/// Keeps a note sounding until the next one starts.
pub const LATCH: ParamId = ParamId(8);

/// The Trigger choices.
pub const TRIGGERS: [&str; 2] = ["Note On", "Note Off"];
/// The beat divisions Length offers, in beats (quarter notes).
pub const DIVISIONS: [(&str, f64); 7] = [
    ("1/32", 0.125),
    ("1/16", 0.25),
    ("1/8", 0.5),
    ("1/4", 1.0),
    ("1/2", 2.0),
    ("1 bar", 4.0),
    ("2 bars", 8.0),
];

/// The velocity of a note-off a note that came in didn't end.
const DEFAULT_RELEASE: f32 = 64.0 / 127.0;
/// How many held keys Note Off mode follows.
const HELD_ROOM: usize = 128;

/// Note Length's device type.
#[must_use]
pub fn hold_type() -> DeviceType {
    let names: Vec<&str> = DIVISIONS.iter().map(|d| d.0).collect();
    DeviceType::native(HOLD, "Note Length", DeviceRole::MidiEffect).with_description(
        DeviceDescription::new(vec![
            ParamDescriptor::choice(TRIGGER, "Trigger", &TRIGGERS, 0).describe(
                "Starts notes when a key goes down (Note On) or when it comes up (Note Off).",
            ),
            ParamDescriptor::toggle(SYNC, "Sync", false)
                .describe("Sets the length in beat divisions rather than milliseconds."),
            ParamDescriptor::continuous(
                LENGTH_MS,
                "Length",
                (1.0, 10_000.0),
                200.0,
                ParamUnit::Milliseconds,
            )
            .logarithmic()
            .decimals(0)
            .describe("How long a note lasts, before Gate."),
            ParamDescriptor::choice(LENGTH_DIVISION, "Length Division", &names, 2)
                .describe("How long a note lasts, in beat divisions, before Gate."),
            ParamDescriptor::continuous(GATE, "Gate", (1.0, 200.0), 100.0, ParamUnit::Percent)
                .decimals(0)
                .describe("Scales Length: 100 % is exactly Length, 200 % twice as long."),
            ParamDescriptor::continuous(
                RELEASE_VELOCITY,
                "Release Velocity",
                (0.0, 100.0),
                0.0,
                ParamUnit::Percent,
            )
            .decimals(0)
            .describe(
                "In Note Off mode, mixes the key's release velocity into the note's velocity. \
                 Leave at 0 % if the keyboard sends none.",
            ),
            ParamDescriptor::continuous(
                DECAY_TIME,
                "Decay Time",
                (0.0, 10_000.0),
                0.0,
                ParamUnit::Milliseconds,
            )
            .decimals(0)
            .describe(
                "In Note Off mode, how long a key's velocity takes to fall to nothing while it \
                 is held; 0 turns the decay off.",
            ),
            ParamDescriptor::continuous(
                KEY_SCALE,
                "Key Scale",
                (-100.0, 100.0),
                0.0,
                ParamUnit::Percent,
            )
            .decimals(0)
            .describe(
                "Makes notes below middle C longer and notes above it shorter (positive), or \
                 the opposite (negative).",
            ),
            ParamDescriptor::toggle(LATCH, "Latch", false).describe(
                "Keeps each note sounding until the next one starts; Length and Gate then do \
                 nothing.",
            ),
        ]),
    )
}

/// Adds Note Length to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, hold_type(), |_, _| Hold {
        router: Router::new(),
        held: Vec::with_capacity(HELD_ROOM),
        latched: false,
    });
}

/// A key held down.
#[derive(Clone, Copy)]
struct Held {
    key: Key,
    /// The device's clock when it went down.
    since: i64,
    velocity: f32,
}

struct Hold {
    router: Router,
    held: Vec<Held>,
    /// Whether Latch was on in the last stretch.
    latched: bool,
}

/// One block's settings.
struct Settings {
    note_off: bool,
    latch: bool,
    /// Frames of Length, before Gate and Key Scale.
    length: f64,
    gate: f64,
    release: f64,
    /// Frames the velocity takes to fall to nothing, or 0.
    decay: f64,
    key_scale: f64,
}

impl Settings {
    /// How many frames a note on `key` sounds for.
    fn frames(&self, key: u8) -> i64 {
        let scale = 2f64.powf(-self.key_scale * (f64::from(key) - 60.0) / 12.0);
        ((self.length * self.gate * scale).round() as i64).max(1)
    }
}

impl Hold {
    /// Takes the key `note` lifts off the keys held down. A key Note Length never
    /// saw go down, such as one that went down while it was switched off,
    /// passes its note-off through, so the instrument doesn't hang on it
    /// (spec: device-framework R31).
    fn take_held(&mut self, note: &NoteEvent) -> Option<Held> {
        let at = self.held.iter().position(|h| {
            h.key.channel == note.channel
                && h.key.key == note.key
                && (h.key.id.is_none() || note.note_id.is_none() || h.key.id == note.note_id)
        })?;
        Some(self.held.remove(at))
    }

    /// Starts a note for the key that went `key`, which sounds for
    /// `settings`' length unless latched.
    fn start(
        &mut self,
        audio: &mut Audio<'_, '_>,
        settings: &Settings,
        source: &NoteEvent,
        velocity: f32,
    ) {
        let key = Key::of(source);
        self.router.cut(audio, source.offset, |k| {
            if settings.latch {
                true
            } else {
                k.channel == key.channel && k.key == key.key
            }
        });
        if !self.router.track(source, Some(key)) {
            return;
        }
        let on = NoteEvent {
            kind: NoteKind::On,
            velocity,
            ..*source
        };
        self.router.send_now(audio, on, key);
        if !settings.latch {
            let off = NoteEvent {
                kind: NoteKind::Off,
                velocity: DEFAULT_RELEASE,
                ..on
            };
            let at = self.router.now() + i64::from(source.offset) + settings.frames(key.key);
            self.router.hold(at, off, key);
        }
    }
}

impl NativeDevice for Hold {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let rate = audio.sample_rate().as_f64();
        let tempo = audio.tempo();
        let bpm = tempo
            .bpm_at(tempo.frames_to_ticks(audio.song_position(), audio.sample_rate()))
            .get();
        let length = if value(params, SYNC) >= 0.5 {
            let (_, beats) = DIVISIONS[value(params, LENGTH_DIVISION) as usize];
            beats * 60.0 / bpm * rate
        } else {
            value(params, LENGTH_MS) * rate / 1000.0
        };
        let settings = Settings {
            note_off: value(params, TRIGGER) as usize == 1,
            latch: value(params, LATCH) >= 0.5,
            length,
            gate: value(params, GATE) / 100.0,
            release: value(params, RELEASE_VELOCITY) / 100.0,
            decay: value(params, DECAY_TIME) * rate / 1000.0,
            key_scale: value(params, KEY_SCALE) / 100.0,
        };
        // Spec: device-framework R31. Latched notes have no end of their own,
        // so turning Latch off ends them.
        if self.latched && !settings.latch {
            self.router.cut(audio, 0, |_| true);
        }
        self.latched = settings.latch;
        for note in audio.notes() {
            self.router.advance(audio, note.offset);
            match (note.kind, settings.note_off) {
                (NoteKind::On, note_off) => {
                    if self.held.len() < HELD_ROOM {
                        self.held.push(Held {
                            key: Key::of(&note),
                            since: self.router.now() + i64::from(note.offset),
                            velocity: note.velocity,
                        });
                    }
                    if !note_off {
                        self.start(audio, &settings, &note, note.velocity);
                    }
                }
                (NoteKind::Off, true) => {
                    let Some(held) = self.take_held(&note) else {
                        audio.send(note);
                        continue;
                    };
                    let elapsed = self.router.now() + i64::from(note.offset) - held.since;
                    let pressed = if settings.decay > 0.0 {
                        held.velocity * (1.0 - elapsed as f64 / settings.decay).max(0.0) as f32
                    } else {
                        held.velocity
                    };
                    let mixed = pressed * (1.0 - settings.release as f32)
                        + note.velocity * settings.release as f32;
                    // Latched, a note starts once every key is up.
                    if !settings.latch || self.held.is_empty() {
                        let source = NoteEvent {
                            note_id: held.key.id,
                            ..note
                        };
                        self.start(audio, &settings, &source, mixed.clamp(1.0 / 127.0, 1.0));
                    }
                }
                (NoteKind::Off, false) => {
                    if self.take_held(&note).is_none() {
                        audio.send(note);
                    }
                }
                (NoteKind::Choke, _) => self.router.end(audio, &note, note.velocity),
                (NoteKind::Expression { .. }, _) => self.router.expression(audio, &note),
                // Spec: device-framework R39. A channel message passes through.
                (_, _) => audio.send(note),
            }
        }
        self.router.finish(audio);
    }

    fn release(&mut self, out: &mut NoteBuffer) {
        self.router.release(out);
        self.held.clear();
    }

    fn dropped_notes(&self) -> u64 {
        self.router.dropped()
    }
}

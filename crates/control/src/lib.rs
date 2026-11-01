//! MIDI and key mapping: which messages a set's mappings take, and what a
//! message does to the control it maps (`docs/specs/remote-mapping.md`, R7
//! to R13 and R15 to R32).
//!
//! Everything here is a pure function of the mapping, the message and the
//! little state a mapping keeps between messages, so the same message on the
//! same state gives the same result on every platform and the window is not
//! needed to test it. The window's side (running the control's command) is
//! `bl-shell`'s.
//!
//! The control-surface host is [`Surfaces`] (`docs/specs/control-surfaces.md`,
//! R1 to R16), with the [`ControlSurface`] trait a surface implements.
//!
//! [`act`] holds the value laws of R15 to R31: a note or a key, an absolute
//! controller (with Takeover Mode) and a relative one, on each kind of
//! control, a chromatic slot's (R33 to R36) included. [`guess`] is the mode
//! guess of R27.

use bl_core::midi::MidiMessage;
use bl_midi_io::RemoteBits;
use bl_model::{
    ControlKind, RelativeEncoding, RemoteMapping, RemoteMode, RemoteSource, TakeoverMode,
};

pub mod browse;
pub mod grid;
mod host;
pub mod launchkey;
pub mod mix;
mod surface;
pub mod template;
mod template_surface;
#[cfg(feature = "testing")]
pub mod testing;

pub use bl_engine::{SlotState, SlotStatus};
pub use host::{CommandRequest, ROWS, RowConfig, RowReport, RunState, Surfaces, Traffic};
pub use surface::{
    Ask, Control, ControlSurface, Cx, DeviceViews, DrumView, Focus, Gesture, Kind, Lock, Moves,
    Out, ParamView, ReadModel, Recognise, Request, Ring, RingStep, RingView, Role, RowView,
    Selection, SurfaceState, TICK, Transport, Waveform, kind, kinds,
};

/// The messages `mappings` take, for the MIDI system's thread to judge by
/// (R9). Key mappings take no MIDI message.
#[must_use]
pub fn bits(mappings: &[RemoteMapping]) -> RemoteBits {
    let mut bits = RemoteBits::default();
    for mapping in mappings {
        match mapping.source {
            RemoteSource::Note { channel, key } => bits.note(channel - 1, key),
            RemoteSource::NoteRange {
                channel, low, high, ..
            } => {
                (low..=high).for_each(|key| bits.note(channel - 1, key));
            }
            RemoteSource::Cc {
                channel,
                controller,
            } => bits.controller(channel - 1, controller),
            RemoteSource::Cc14 {
                channel,
                controller,
            } => {
                bits.controller(channel - 1, controller);
                bits.controller(channel - 1, controller + 32);
            }
            RemoteSource::PitchBend { channel } => bits.pitch_bend(channel - 1),
            RemoteSource::Key { .. } => {}
        }
    }
    bits
}

/// What one message or key is to a mapping that takes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    /// A note-on of `key` at `velocity`.
    NoteOn { key: u8, velocity: u8 },
    /// A note-off of `key`.
    NoteOff { key: u8 },
    /// An absolute value, `value` of `full` (127, or 16,383 for a 14-bit
    /// controller and pitch bend).
    Value { value: u16, full: u16 },
    /// The key went down.
    KeyDown,
    /// The key came up.
    KeyUp,
}

/// How near two positions on a control's travel are to be the same one.
const SAME: f64 = 1e-9;

/// What a mapping remembers between messages: the high part of a 14-bit
/// pair, whether a slot's or a button's press is held (R20, R22), and where
/// an absolute controller and its control were (R29 to R31). None of it is
/// saved.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Memory {
    high: u8,
    pressed: Option<bool>,
    /// Where the controller last was, as the position in the mapping's range
    /// its value gives (R21).
    previous: Option<f64>,
    /// Where the control was after the mapping's last message moved it.
    given: Option<f64>,
    /// Whether the control follows the controller, as with Takeover Mode
    /// None.
    following: bool,
    /// The key of a chromatic slot's latest launch, whose release is the
    /// only one that acts (R36).
    latest: Option<u8>,
}

impl Memory {
    /// The memory of a controller known to rest at `position`, as the
    /// position in the mapping's range its value gives, before it has sent a
    /// message (supported controllers R35).
    #[must_use]
    pub fn resting(position: f64) -> Self {
        Self {
            previous: Some(position),
            ..Self::default()
        }
    }

    /// Whether the control follows the controller (R29).
    #[must_use]
    pub fn following(&self) -> bool {
        self.following
    }

    /// Where the controller last was, as a position in the mapping's range.
    #[must_use]
    pub fn last(&self) -> Option<f64> {
        self.previous
    }

    /// Tells the memory where the control really is after the mapping's last
    /// message moved it, which a stepped control rounds (R29). Without it
    /// the memory takes the control to be where the law asked.
    pub fn gave(&mut self, position: f64) {
        self.given = Some(position);
    }
}

/// What `message` is to `mapping`, if the mapping takes it (R8), updating
/// the mapping's `memory`. Channels count from 1, as the mapping has them.
#[must_use]
pub fn hit(mapping: &RemoteMapping, message: &MidiMessage, memory: &mut Memory) -> Option<Hit> {
    let channel = message.channel()? + 1;
    let (status, a, b) = (message.status() & 0xF0, message.data1(), message.data2());
    let note = |on: bool, key: u8| {
        if on {
            Hit::NoteOn { key, velocity: b }
        } else {
            Hit::NoteOff { key }
        }
    };
    match (mapping.source.clone(), status) {
        (RemoteSource::Note { channel: c, key }, 0x80 | 0x90) if c == channel && key == a => {
            Some(note(message.is_note_on(), a))
        }
        (
            RemoteSource::NoteRange {
                channel: c,
                low,
                high,
                ..
            },
            0x80 | 0x90,
        ) if c == channel && (low..=high).contains(&a) => Some(note(message.is_note_on(), a)),
        (
            RemoteSource::Cc {
                channel: c,
                controller,
            },
            0xB0,
        ) if c == channel && controller == a => Some(Hit::Value {
            value: u16::from(b),
            full: 127,
        }),
        (
            RemoteSource::Cc14 {
                channel: c,
                controller,
            },
            0xB0,
        ) if c == channel => {
            // Spec: remote-mapping R22. The high part applies at once with
            // the low part taken as 0; its partner applies again with the
            // low part it carries.
            let (high, low) = if a == controller {
                memory.high = b;
                (b, 0)
            } else if a == controller + 32 {
                (memory.high, b)
            } else {
                return None;
            };
            Some(Hit::Value {
                value: u16::from(high) << 7 | u16::from(low),
                full: 16_383,
            })
        }
        (RemoteSource::PitchBend { channel: c }, 0xE0) if c == channel => Some(Hit::Value {
            value: u16::from(b) << 7 | u16::from(a),
            full: 16_383,
        }),
        _ => None,
    }
}

/// What a control does with a hit (R15 to R26).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    /// A slot's or a button's press, at a velocity from 1 to 127.
    Press(u8),
    /// A chromatic slot's press at a velocity from 1 to 127, with the
    /// semitones its key is from the root (R34).
    PressTransposed(u8, i8),
    /// A slot's or a button's release.
    Release,
    /// A switch turns over.
    Toggle,
    /// A switch is set.
    Set(bool),
    /// A radio button selects its next option, after the last the first.
    Next,
    /// A radio button selects this option.
    Option(u32),
    /// A continuous control goes to this position on its travel, from 0 to
    /// 1.
    Position(f64),
    /// A continuous control goes to Max, or to Min when it is at Max.
    MaxOrMin,
}

/// The mapping's range, as the positions the laws work in: a continuous
/// control's Min and Max are positions on its travel (R4), the rest are
/// plain numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Law {
    /// The kind of control.
    pub kind: ControlKind,
    /// The bottom of the range.
    pub min: f64,
    /// The top of the range.
    pub max: f64,
    /// How a controller is read.
    pub mode: RemoteMode,
    /// How many keys the mapping's range has; 1 for a single note or key.
    pub keys: u32,
    /// The root key of a slot's note range (R33).
    pub root: Option<u8>,
    /// What an absolute controller does when it and its control disagree
    /// (R29 to R31).
    pub takeover: TakeoverMode,
    /// One step of a stepped continuous control, as a share of its travel;
    /// `None` for any other, whose step is a 127th of the range (R26).
    pub step: Option<f64>,
}

impl Law {
    /// The law of a mapping, with `min` and `max` as the positions the
    /// caller worked out for a continuous control, Takeover Mode None and
    /// no stepped control.
    #[must_use]
    pub fn of(mapping: &RemoteMapping, min: f64, max: f64) -> Self {
        let (keys, root) = match mapping.source {
            RemoteSource::NoteRange {
                low, high, root, ..
            } => (u32::from(high - low) + 1, root),
            _ => (1, None),
        };
        Self {
            kind: mapping.kind,
            min,
            max,
            mode: mapping.mode,
            keys,
            root,
            takeover: TakeoverMode::None,
            step: None,
        }
    }

    /// One step of the range, for a relative controller and Value Scaling
    /// (R26).
    fn step(&self) -> f64 {
        self.step
            .unwrap_or_else(|| (self.max - self.min).abs() / 127.0)
    }

    /// The range's lower and upper ends.
    fn ends(&self) -> (f64, f64) {
        (self.min.min(self.max), self.min.max(self.max))
    }
}

/// What `hit` does to a control under `law`, updating the mapping's
/// `memory`; `key` is the offset of the hit note from the range's lowest and
/// `at` the control's value as the laws read it: a continuous control's
/// position on its travel, a radio button's option, `None` when the control
/// can't be read.
#[must_use]
pub fn act(law: &Law, hit: Hit, key: u32, at: Option<f64>, memory: &mut Memory) -> Option<Action> {
    let range = |share: f64| law.min + share * (law.max - law.min);
    match (hit, law.kind) {
        // Spec: remote-mapping R34, R36. A key of a slot's range launches
        // its clip transposed by the key less the root, and only the latest
        // key's release lets go.
        (Hit::NoteOn { key, velocity }, ControlKind::Slot) if law.root.is_some() => {
            let transpose = i16::from(key) - i16::from(law.root?);
            memory.latest = Some(key);
            Some(Action::PressTransposed(
                velocity,
                i8::try_from(transpose).ok()?,
            ))
        }
        (Hit::NoteOff { key }, ControlKind::Slot) if law.root.is_some() => memory
            .latest
            .take_if(|latest| *latest == key)
            .map(|_| Action::Release),
        (Hit::NoteOn { velocity, .. }, ControlKind::Slot | ControlKind::Button) => {
            Some(Action::Press(velocity))
        }
        (Hit::KeyDown, ControlKind::Slot | ControlKind::Button) => Some(Action::Press(127)),
        (Hit::NoteOff { .. } | Hit::KeyUp, ControlKind::Slot | ControlKind::Button) => {
            Some(Action::Release)
        }
        (Hit::NoteOn { .. } | Hit::KeyDown, ControlKind::Switch | ControlKind::RangedSwitch) => {
            Some(Action::Toggle)
        }
        (Hit::NoteOn { .. } | Hit::KeyDown, ControlKind::Radio { .. }) if law.keys == 1 => {
            Some(Action::Next)
        }
        (Hit::NoteOn { .. }, ControlKind::Radio { options }) => {
            let share = f64::from(key) / f64::from(law.keys - 1);
            Some(Action::Option(nearest(range(share), options)))
        }
        (Hit::NoteOn { .. }, ControlKind::Continuous) if law.keys == 1 => Some(Action::MaxOrMin),
        (Hit::NoteOn { .. }, ControlKind::Continuous) => Some(Action::Position(range(
            f64::from(key) / f64::from(law.keys - 1),
        ))),
        (Hit::Value { value, full }, kind) => match law.mode {
            RemoteMode::Absolute | RemoteMode::Absolute14 => {
                absolute(law, kind, value, full, at, memory)
            }
            RemoteMode::Relative {
                encoding,
                accelerated,
            } => relative(
                law,
                size(encoding, u8::try_from(value).ok()?),
                accelerated,
                at,
            ),
        },
        _ => None,
    }
}

/// What an absolute controller's `value` of `full` does to a control of
/// `kind` (R20 to R22, R29 to R31).
fn absolute(
    law: &Law,
    kind: ControlKind,
    value: u16,
    full: u16,
    at: Option<f64>,
    memory: &mut Memory,
) -> Option<Action> {
    let share = f64::from(value) / f64::from(full);
    let range = |share: f64| law.min + share * (law.max - law.min);
    let half = full / 2 + 1;
    match kind {
        ControlKind::Slot | ControlKind::Button => {
            let down = value >= half;
            let was = memory.pressed.replace(down);
            match (down, was) {
                (true, Some(true)) | (false, None | Some(false)) => None,
                (true, _) => Some(Action::Press(127)),
                (false, Some(true)) => Some(Action::Release),
            }
        }
        ControlKind::RangedSwitch => {
            // Min and Max are controller values; scale a 14-bit one.
            let c = share * 127.0;
            let (low, high) = law.ends();
            let inside = (low..=high).contains(&c.round());
            Some(Action::Set(inside == (law.min <= law.max)))
        }
        ControlKind::Switch => Some(Action::Set(value >= half)),
        ControlKind::Radio { options } => Some(Action::Option(nearest(range(share), options))),
        ControlKind::Continuous => takeover(law, range(share), at, memory),
    }
}

/// Where an absolute controller, now at `target` in the mapping's range,
/// takes a continuous control that is at `at`, by the law's Takeover Mode
/// (R29 to R31).
fn takeover(law: &Law, target: f64, at: Option<f64>, memory: &mut Memory) -> Option<Action> {
    let previous = memory.previous.replace(target);
    let follow = |memory: &mut Memory| {
        memory.following = true;
        memory.given = Some(target);
        Some(Action::Position(target))
    };
    let Some(control) = at else {
        return follow(memory);
    };
    let in_step = memory.following
        && memory
            .given
            .is_some_and(|given| (given - control).abs() <= SAME);
    if in_step {
        return follow(memory);
    }
    memory.following = law.takeover == TakeoverMode::None;
    match law.takeover {
        TakeoverMode::None => follow(memory),
        // R30
        TakeoverMode::PickUp => {
            let reached = (target - control).abs() <= SAME
                || previous.is_some_and(|before| {
                    (before.min(target)..=before.max(target)).contains(&control)
                });
            if reached { follow(memory) } else { None }
        }
        // R31
        TakeoverMode::ValueScaling => {
            let before = previous?;
            if (target - control).abs() <= law.step() {
                return follow(memory);
            }
            if (target - before).abs() <= SAME {
                return None;
            }
            let (low, high) = law.ends();
            let end = if target > before { high } else { low };
            let moved = control + (target - before) * (end - control) / (end - before);
            let moved = moved.clamp(0.0, 1.0);
            if (target - moved).abs() <= law.step() {
                return follow(memory);
            }
            memory.given = Some(moved);
            Some(Action::Position(moved))
        }
    }
}

/// What a relative controller's message of `size` (R24) does to a control
/// (R23, R25, R26); `size` is positive for an increment, negative for a
/// decrement.
fn relative(law: &Law, size: i32, accelerated: bool, at: Option<f64>) -> Option<Action> {
    if size == 0 {
        return None;
    }
    let up = size > 0;
    match law.kind {
        ControlKind::Slot | ControlKind::Button => Some(if up {
            Action::Press(127)
        } else {
            Action::Release
        }),
        ControlKind::Switch | ControlKind::RangedSwitch => Some(Action::Set(up)),
        ControlKind::Radio { options } => {
            let (low, high) = law.ends();
            let high = (high.round().max(0.0) as u32).min(options.saturating_sub(1));
            let low = (low.round().max(0.0) as u32).min(high);
            let now = at?.round().max(0.0) as u32;
            let next = if up {
                now.saturating_add(1)
            } else {
                now.saturating_sub(1)
            };
            let next = next.clamp(low, high);
            (next != now).then_some(Action::Option(next))
        }
        ControlKind::Continuous => {
            let now = at?;
            let (low, high) = law.ends();
            let steps = if accelerated { size.unsigned_abs() } else { 1 };
            let towards_max = if law.max >= law.min { up } else { !up };
            let by = f64::from(steps) * law.step();
            let next = if towards_max { now + by } else { now - by }.clamp(low, high);
            ((next - now).abs() > SAME).then_some(Action::Position(next))
        }
    }
}

/// The size of the increment (positive) or decrement (negative) a relative
/// controller's value `c` is in `encoding`; 0 changes nothing (R24).
// Spec: remote-mapping R24. The one place the four encodings are read, so
// an observation of Live changes this function alone.
#[must_use]
pub fn size(encoding: RelativeEncoding, c: u8) -> i32 {
    let c = i32::from(c.min(127));
    match encoding {
        RelativeEncoding::SignedBit => match c {
            65..=127 => c - 64,
            1..=63 => -c,
            _ => 0,
        },
        RelativeEncoding::SignedBit2 => match c {
            1..=63 => c,
            65..=127 => -(c - 64),
            _ => 0,
        },
        RelativeEncoding::BinOffset => match c {
            65..=127 => c - 64,
            0..=63 => -(64 - c),
            _ => 0,
        },
        RelativeEncoding::TwosComplement => match c {
            1..=63 => c,
            64..=127 => -(128 - c),
            _ => 0,
        },
    }
}

/// The encoding whose decrement of one `c` is (R27).
fn decrement_of_one(c: u8) -> Option<RelativeEncoding> {
    Some(match c {
        1 => RelativeEncoding::SignedBit,
        65 => RelativeEncoding::SignedBit2,
        63 => RelativeEncoding::BinOffset,
        127 => RelativeEncoding::TwosComplement,
        _ => return None,
    })
}

/// The mode of the controller `messages` were learned from, guessed from
/// the messages that arrived while it was learned, oldest first (R27); the
/// guess after each new message is this function of all of them. Anything
/// but control changes is ignored, and the controller is the first control
/// change's.
// Spec: remote-mapping R27. The one place the guess is made.
#[must_use]
pub fn guess(messages: &[MidiMessage]) -> RemoteMode {
    let changes: Vec<(u8, u8, u8)> = messages
        .iter()
        .filter(|m| m.status() & 0xF0 == 0xB0)
        .filter_map(|m| Some((m.channel()?, m.data1(), m.data2())))
        .collect();
    let Some(&(channel, controller, _)) = changes.first() else {
        return RemoteMode::Absolute;
    };
    if changes
        .windows(2)
        .any(|w| w[0].0 == w[1].0 && w[0].1 < 32 && w[1].1 == w[0].1 + 32)
    {
        return RemoteMode::Absolute14;
    }
    let values: Vec<u8> = changes
        .iter()
        .filter(|&&(c, n, _)| c == channel && n == controller)
        .map(|&(_, _, v)| v)
        .collect();
    let named = values
        .windows(2)
        .rev()
        .find(|w| w[0] == w[1])
        .and_then(|w| decrement_of_one(w[0]));
    match named {
        Some(encoding) => RemoteMode::Relative {
            encoding,
            accelerated: values.iter().any(|&v| size(encoding, v).abs() > 1),
        },
        None => RemoteMode::Absolute,
    }
}

/// What a mapped control is now, for the controller's lamps and faders
/// (R38).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    /// A switch, a slot that plays or is queued: on or off.
    On(bool),
    /// A continuous control's position on its travel, or a radio button's
    /// option.
    Value(f64),
}

/// The messages that tell the controller `mapping` is mapped to what its
/// control is now, `state`, with `min` and `max` as [`Law::of`] takes them;
/// none where the mapping has nothing to show (R38). Each is the inverse of
/// the law that reads the controller (R20 to R22).
// Spec: remote-mapping R38. The one place feedback is worked out.
#[must_use]
pub fn feedback(mapping: &RemoteMapping, min: f64, max: f64, state: State) -> Vec<MidiMessage> {
    let kind = mapping.kind;
    let (channel, full, controller) = match mapping.source {
        RemoteSource::Note { channel, key } => {
            let velocity = match (kind, state) {
                (
                    ControlKind::Slot | ControlKind::Switch | ControlKind::RangedSwitch,
                    State::On(on),
                ) => 127 * u8::from(on),
                _ => return Vec::new(),
            };
            return vec![MidiMessage::note_on(channel - 1, key, velocity)];
        }
        RemoteSource::Cc {
            channel,
            controller,
        } => (channel, 127.0, Some(controller)),
        RemoteSource::Cc14 {
            channel,
            controller,
        } => (channel, 16_383.0, Some(controller)),
        RemoteSource::PitchBend { channel } => (channel, 16_383.0, None),
        RemoteSource::NoteRange { .. } | RemoteSource::Key { .. } => return Vec::new(),
    };
    let share = |value: f64| {
        if max == min {
            0.0
        } else {
            ((value - min) / (max - min)).clamp(0.0, 1.0)
        }
    };
    let value = match (kind, state) {
        (ControlKind::Slot | ControlKind::Switch, State::On(on)) => full * f64::from(u8::from(on)),
        (ControlKind::RangedSwitch, State::On(on)) => {
            // Min and Max are controller values; a value inside Min to Max is
            // on unless Min is above Max (R20).
            let (low, high) = (min.min(max), min.max(max));
            let inside = on == (min <= max);
            let c = if inside {
                max
            } else if low > 0.0 {
                0.0
            } else {
                (high + 1.0).min(127.0)
            };
            (c / 127.0 * full).round()
        }
        (ControlKind::Continuous | ControlKind::Radio { .. }, State::Value(value)) => {
            (share(value) * full).round()
        }
        _ => return Vec::new(),
    } as u16;
    // A relative mapping shows the position as 0 to 127 on its controller
    // (R38).
    let relative = matches!(mapping.mode, RemoteMode::Relative { .. });
    let (value, full) = if relative {
        (((f64::from(value) / full) * 127.0).round() as u16, 127)
    } else {
        (value, full as u16)
    };
    let (high, low) = ((value >> 7) as u8, (value & 127) as u8);
    let cc = |controller: u8, value: u8| {
        MidiMessage::from_bytes(&[0xB0 | (channel - 1), controller, value])
    };
    let sent = match controller {
        Some(controller) if full == 127 => vec![cc(controller, value as u8)],
        Some(controller) => vec![cc(controller, high), cc(controller + 32, low)],
        None if relative => Vec::new(),
        None => vec![MidiMessage::from_bytes(&[0xE0 | (channel - 1), low, high])],
    };
    sent.into_iter().flatten().collect()
}

/// The option of a radio button of `options` nearest `position`.
fn nearest(position: f64, options: u32) -> u32 {
    (position.round().max(0.0) as u32).min(options.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use bl_core::id::Id;
    use bl_model::{DeviceId, MixerParam, ParamId, RemoteTarget, TrackId};

    use super::*;

    fn track() -> TrackId {
        TrackId(Id::new(2).unwrap())
    }

    fn mapping(
        source: RemoteSource,
        target: RemoteTarget,
        range: Option<(f64, f64)>,
    ) -> RemoteMapping {
        let kind = target.natural_kind().unwrap();
        RemoteMapping::new(source, target, kind, range, None).unwrap()
    }

    fn message(bytes: &[u8]) -> MidiMessage {
        MidiMessage::from_bytes(bytes).unwrap()
    }

    /// What a controller value does to a control of `kind`, in turn.
    fn values(mapping: &RemoteMapping, min: f64, max: f64, values: &[u8]) -> Vec<Option<Action>> {
        let law = Law::of(mapping, min, max);
        let mut memory = Memory::default();
        values
            .iter()
            .map(|&v| {
                let hit = Hit::Value {
                    value: u16::from(v),
                    full: 127,
                };
                act(&law, hit, 0, None, &mut memory)
            })
            .collect()
    }

    // Spec: remote-mapping R8, R9
    #[test]
    fn the_table_takes_what_the_mappings_name_by_zero_based_channel() {
        let list = [
            mapping(
                RemoteSource::NoteRange {
                    channel: 2,
                    low: 10,
                    high: 12,
                    root: None,
                },
                RemoteTarget::Crossfader,
                Some((0.0, 1.0)),
            ),
            mapping(
                RemoteSource::Cc14 {
                    channel: 1,
                    controller: 7,
                },
                RemoteTarget::Mixer {
                    track: track(),
                    param: MixerParam::Volume,
                },
                Some((0.0, 2.0)),
            ),
        ];
        let taken = bits(&list);
        let remote = bl_midi_io::input_queue(4).0;
        remote.remote().publish(&taken);
        let takes = |bytes: &[u8]| remote.remote().takes(&message(bytes));
        assert!(takes(&[0x91, 11, 1]) && takes(&[0x81, 12, 0]) && !takes(&[0x91, 13, 1]));
        assert!(takes(&[0xB0, 7, 1]) && takes(&[0xB0, 39, 1]) && !takes(&[0xB0, 8, 1]));
    }

    // Spec: remote-mapping R22
    #[test]
    fn a_14_bit_pair_applies_the_high_part_at_once_and_again_with_the_low_part() {
        let m = RemoteMapping::new(
            RemoteSource::Cc14 {
                channel: 1,
                controller: 7,
            },
            RemoteTarget::Crossfader,
            ControlKind::Continuous,
            Some((0.0, 1.0)),
            None,
        )
        .unwrap();
        let mut memory = Memory::default();
        let value = |bytes: &[u8], memory: &mut Memory| match hit(&m, &message(bytes), memory) {
            Some(Hit::Value { value, full }) => (value, full),
            other => panic!("{other:?}"),
        };
        assert_eq!(value(&[0xB0, 7, 64], &mut memory), (8192, 16_383));
        assert_eq!(value(&[0xB0, 39, 5], &mut memory), (8197, 16_383));
        assert_eq!(
            hit(&m, &message(&[0xB1, 7, 64]), &mut memory),
            None,
            "another channel"
        );
    }

    // Spec: remote-mapping R33 to R36. A key of a slot's range presses it
    // transposed by the key less the root, at the key's velocity; the root
    // is not transposed; and only the release of the latest key acts.
    #[test]
    fn a_slots_range_presses_transposed_and_only_the_latest_keys_release_acts() {
        let slot = mapping(
            RemoteSource::NoteRange {
                channel: 1,
                low: 36,
                high: 60,
                root: Some(48),
            },
            RemoteTarget::Slot {
                track: track(),
                scene: bl_model::SceneId(Id::new(3).unwrap()),
            },
            None,
        );
        let law = Law::of(&slot, 0.0, 1.0);
        let mut memory = Memory::default();
        let mut act_on = |hit| act(&law, hit, 0, None, &mut memory);
        let on = |key, velocity| Hit::NoteOn { key, velocity };
        let off = |key| Hit::NoteOff { key };
        assert_eq!(
            act_on(on(52, 90)),
            Some(Action::PressTransposed(90, 4)),
            "four above the root"
        );
        assert_eq!(act_on(on(48, 100)), Some(Action::PressTransposed(100, 0)));
        assert_eq!(act_on(on(36, 1)), Some(Action::PressTransposed(1, -12)));
        // Three keys are down; the latest is 36, so the earlier ones' releases
        // do nothing, and 36's lets go once.
        assert_eq!(act_on(off(52)), None);
        assert_eq!(act_on(off(48)), None);
        assert_eq!(act_on(off(36)), Some(Action::Release));
        assert_eq!(act_on(off(36)), None);
    }

    // Spec: remote-mapping R20
    #[test]
    fn an_absolute_controller_presses_a_slot_on_the_way_up_and_releases_on_the_way_down() {
        let slot = mapping(
            RemoteSource::Cc {
                channel: 1,
                controller: 20,
            },
            RemoteTarget::Slot {
                track: track(),
                scene: bl_model::SceneId(Id::new(3).unwrap()),
            },
            None,
        );
        assert_eq!(
            values(&slot, 0.0, 1.0, &[70, 80, 63, 10, 64]),
            [
                Some(Action::Press(127)),
                None,
                Some(Action::Release),
                None,
                Some(Action::Press(127))
            ]
        );
    }

    // Spec: remote-mapping R20
    #[test]
    fn a_ranged_switch_is_on_inside_min_to_max_and_off_outside_and_the_other_way_round_when_inverted()
     {
        let activator = mapping(
            RemoteSource::Cc {
                channel: 1,
                controller: 23,
            },
            RemoteTarget::Mixer {
                track: track(),
                param: MixerParam::Mute,
            },
            Some((64.0, 127.0)),
        );
        let on = |set: bool| Some(Action::Set(set));
        assert_eq!(
            values(&activator, 64.0, 127.0, &[0, 63, 64, 127]),
            [on(false), on(false), on(true), on(true)]
        );
        assert_eq!(
            values(&activator, 100.0, 20.0, &[0, 20, 60, 100, 127]),
            [on(true), on(false), on(false), on(false), on(true)]
        );
    }

    // Spec: remote-mapping R17 to R19, R21
    #[test]
    fn notes_toggle_step_and_spread_and_a_controller_sets_a_position_in_the_range() {
        let note = RemoteSource::Note {
            channel: 1,
            key: 36,
        };
        let solo = mapping(note.clone(), RemoteTarget::Solo { track: track() }, None);
        let law = Law::of(&solo, 0.0, 1.0);
        let mut memory = Memory::default();
        let on = Hit::NoteOn {
            key: 36,
            velocity: 90,
        };
        assert_eq!(act(&law, on, 0, None, &mut memory), Some(Action::Toggle));
        assert_eq!(
            act(&law, Hit::NoteOff { key: 36 }, 0, None, &mut memory),
            None
        );

        let assign = mapping(
            note.clone(),
            RemoteTarget::CrossfadeAssign { track: track() },
            None,
        );
        let law = Law::of(&assign, 0.0, 2.0);
        assert_eq!(act(&law, on, 0, None, &mut memory), Some(Action::Next));

        let fader = mapping(
            RemoteSource::NoteRange {
                channel: 1,
                low: 36,
                high: 40,
                root: None,
            },
            RemoteTarget::Crossfader,
            Some((0.0, 1.0)),
        );
        let law = Law::of(&fader, 0.2, 0.8);
        assert_eq!(
            act(&law, on, 0, None, &mut memory),
            Some(Action::Position(0.2))
        );
        assert_eq!(
            act(&law, on, 4, None, &mut memory),
            Some(Action::Position(0.8))
        );

        let single = Law::of(
            &mapping(note, RemoteTarget::Crossfader, Some((0.0, 1.0))),
            0.0,
            1.0,
        );
        assert_eq!(
            act(&single, on, 0, None, &mut memory),
            Some(Action::MaxOrMin)
        );

        let fader = mapping(
            RemoteSource::Cc {
                channel: 1,
                controller: 1,
            },
            RemoteTarget::Crossfader,
            Some((0.0, 1.0)),
        );
        assert_eq!(
            values(&fader, 0.0, 1.0, &[0, 127]),
            [Some(Action::Position(0.0)), Some(Action::Position(1.0))]
        );
        assert_eq!(
            values(&fader, 1.0, 0.0, &[127]),
            [Some(Action::Position(0.0))],
            "Min above Max turns it round"
        );
    }

    use bl_model::TakeoverMode;
    use proptest::prelude::*;

    /// One mapping's law for a control of `kind` with `mode`, over `min` to
    /// `max`.
    fn law(kind: ControlKind, mode: RemoteMode, min: f64, max: f64) -> Law {
        Law {
            kind,
            min,
            max,
            mode,
            keys: 1,
            root: None,
            takeover: TakeoverMode::None,
            step: None,
        }
    }

    fn turn(law: &Law, c: u16, at: Option<f64>) -> Option<Action> {
        act(
            law,
            Hit::Value {
                value: c,
                full: 127,
            },
            0,
            at,
            &mut Memory::default(),
        )
    }

    // Spec: remote-mapping R24
    #[test]
    fn each_encoding_reads_one_63_64_65_and_127_as_the_rule_says() {
        use RelativeEncoding::{BinOffset, SignedBit, SignedBit2, TwosComplement};
        let table = [
            (SignedBit, [-1, -63, 0, 1, 63]),
            (SignedBit2, [1, 63, 0, -1, -63]),
            (BinOffset, [-63, -1, 0, 1, 63]),
            (TwosComplement, [1, 63, -64, -63, -1]),
        ];
        for (encoding, sizes) in table {
            let read: Vec<i32> = [1, 63, 64, 65, 127].map(|c| size(encoding, c)).into();
            assert_eq!(read, sizes, "{encoding:?}");
        }
        assert_eq!(size(RelativeEncoding::BinOffset, 0), -64);
        assert_eq!(size(RelativeEncoding::SignedBit, 0), 0);
        assert_eq!(size(RelativeEncoding::TwosComplement, 0), 0);
    }

    // Spec: remote-mapping R23, R25, R26
    #[test]
    fn a_relative_controller_moves_a_fader_by_its_size_or_by_one_step_and_stays_in_the_range() {
        let step = 1.0 / 127.0;
        let mode = |accelerated| RemoteMode::Relative {
            encoding: RelativeEncoding::BinOffset,
            accelerated,
        };
        let fader =
            |accelerated, min, max| law(ControlKind::Continuous, mode(accelerated), min, max);
        let near = |a: Option<Action>, to: f64| match a {
            Some(Action::Position(p)) => assert!((p - to).abs() < 1e-12, "{p} vs {to}"),
            other => panic!("{other:?}"),
        };
        near(
            turn(&fader(true, 0.0, 1.0), 67, Some(0.5)),
            0.5 + 3.0 * step,
        );
        near(turn(&fader(false, 0.0, 1.0), 67, Some(0.5)), 0.5 + step);
        near(
            turn(&fader(true, 0.0, 1.0), 61, Some(0.5)),
            0.5 - 3.0 * step,
        );
        near(turn(&fader(true, 0.0, 1.0), 127, Some(0.99)), 1.0);
        near(turn(&fader(true, 0.2, 0.6), 127, Some(0.5)), 0.6);
        // A step is a 127th of the range, and Min above Max turns it round.
        near(
            turn(&fader(false, 0.2, 0.6), 65, Some(0.3)),
            0.3 + 0.4 / 127.0,
        );
        near(
            turn(&fader(false, 0.6, 0.2), 65, Some(0.3)),
            0.3 - 0.4 / 127.0,
        );
        assert_eq!(turn(&fader(true, 0.0, 1.0), 127, Some(1.0)), None, "at Max");
        assert_eq!(turn(&fader(true, 0.0, 1.0), 64, Some(0.5)), None, "size 0");
        assert_eq!(turn(&fader(true, 0.0, 1.0), 65, None), None, "unreadable");
        // A stepped control's own step.
        let stepped = Law {
            step: Some(0.25),
            ..fader(false, 0.0, 1.0)
        };
        near(turn(&stepped, 127, Some(0.25)), 0.5);
    }

    // Spec: remote-mapping R23
    #[test]
    fn a_relative_controller_presses_and_releases_a_slot_sets_a_switch_and_steps_a_radio_button() {
        let mode = RemoteMode::Relative {
            encoding: RelativeEncoding::TwosComplement,
            accelerated: false,
        };
        let slot = law(ControlKind::Slot, mode, 0.0, 1.0);
        assert_eq!(turn(&slot, 1, None), Some(Action::Press(127)));
        assert_eq!(turn(&slot, 127, None), Some(Action::Release));
        for kind in [ControlKind::Switch, ControlKind::RangedSwitch] {
            let switch = law(kind, mode, 64.0, 127.0);
            assert_eq!(turn(&switch, 3, None), Some(Action::Set(true)));
            assert_eq!(turn(&switch, 126, None), Some(Action::Set(false)));
        }
        let radio = law(ControlKind::Radio { options: 3 }, mode, 0.0, 2.0);
        assert_eq!(turn(&radio, 1, Some(0.0)), Some(Action::Option(1)));
        assert_eq!(turn(&radio, 1, Some(1.0)), Some(Action::Option(2)));
        assert_eq!(turn(&radio, 1, Some(2.0)), None, "stops at the last");
        assert_eq!(turn(&radio, 127, Some(2.0)), Some(Action::Option(1)));
        assert_eq!(turn(&radio, 127, Some(0.0)), None, "stops at the first");
        let narrowed = law(ControlKind::Radio { options: 5 }, mode, 1.0, 3.0);
        assert_eq!(turn(&narrowed, 1, Some(3.0)), None, "stops at Max");
    }

    // Spec: remote-mapping R19
    #[test]
    fn a_range_of_five_keys_on_a_five_option_radio_button_selects_each_option() {
        let mapping = RemoteMapping::new(
            RemoteSource::NoteRange {
                channel: 1,
                low: 60,
                high: 64,
                root: None,
            },
            RemoteTarget::Device {
                device: DeviceId(Id::new(5).unwrap()),
                param: ParamId(2),
            },
            ControlKind::Radio { options: 5 },
            None,
            None,
        )
        .unwrap();
        let law = Law::of(&mapping, mapping.min, mapping.max);
        assert_eq!(law.keys, 5);
        for k in 0..5u32 {
            let hit = Hit::NoteOn {
                key: 60 + k as u8,
                velocity: 100,
            };
            assert_eq!(
                act(&law, hit, k, None, &mut Memory::default()),
                Some(Action::Option(k))
            );
        }
    }

    fn cc(controller: u8, value: u8) -> MidiMessage {
        message(&[0xB0, controller, value])
    }

    // Spec: remote-mapping R27
    #[test]
    fn the_guess_names_the_encoding_of_a_repeated_decrement_and_a_14_bit_pair() {
        use RelativeEncoding::{BinOffset, SignedBit, SignedBit2, TwosComplement};
        let relative = |encoding, accelerated| RemoteMode::Relative {
            encoding,
            accelerated,
        };
        for (value, encoding) in [
            (1, SignedBit),
            (65, SignedBit2),
            (63, BinOffset),
            (127, TwosComplement),
        ] {
            assert_eq!(
                guess(&[cc(20, value), cc(20, value)]),
                relative(encoding, false),
                "{value}"
            );
        }
        // A larger step in the run makes it accelerated, and the latest
        // pair of equal values is the one that counts.
        assert_eq!(
            guess(&[cc(20, 125), cc(20, 127), cc(20, 127)]),
            relative(TwosComplement, true)
        );
        assert_eq!(
            guess(&[cc(20, 10), cc(20, 20), cc(20, 127), cc(20, 127)]),
            relative(TwosComplement, true)
        );
        assert_eq!(
            guess(&[cc(20, 127), cc(20, 126), cc(20, 127)]),
            RemoteMode::Absolute
        );
        assert_eq!(guess(&[cc(20, 40), cc(20, 40)]), RemoteMode::Absolute);
        assert_eq!(guess(&[cc(20, 40)]), RemoteMode::Absolute);
        assert_eq!(guess(&[]), RemoteMode::Absolute);
        assert_eq!(guess(&[cc(7, 64), cc(39, 5)]), RemoteMode::Absolute14);
        assert_eq!(
            guess(&[cc(7, 64), message(&[0xB1, 39, 5])]),
            RemoteMode::Absolute,
            "another channel"
        );
        assert_eq!(
            guess(&[cc(40, 1), cc(40, 1), cc(72, 1)]),
            relative(SignedBit, false),
            "only controllers below 32 have a partner"
        );
        assert_eq!(
            guess(&[message(&[0x90, 60, 1]), message(&[0x90, 60, 1])]),
            RemoteMode::Absolute,
            "notes aren't controllers"
        );
    }

    /// A controller's values driving a control that starts at `start`, as
    /// the shell does: each position asked for is where the control is next.
    fn drive(takeover: TakeoverMode, start: f64, values: &[u8]) -> Vec<(Option<Action>, f64)> {
        let mapping = mapping(
            RemoteSource::Cc {
                channel: 1,
                controller: 1,
            },
            RemoteTarget::Crossfader,
            Some((0.0, 1.0)),
        );
        let law = Law {
            takeover,
            ..Law::of(&mapping, 0.0, 1.0)
        };
        let (mut memory, mut at) = (Memory::default(), start);
        values
            .iter()
            .map(|&v| {
                let hit = Hit::Value {
                    value: u16::from(v),
                    full: 127,
                };
                let action = act(&law, hit, 0, Some(at), &mut memory);
                if let Some(Action::Position(p)) = action {
                    at = p;
                }
                (action, at)
            })
            .collect()
    }

    fn share(v: u8) -> f64 {
        f64::from(v) / 127.0
    }

    // Spec: remote-mapping R29
    #[test]
    fn takeover_none_jumps_to_the_controller_on_every_message() {
        let run = drive(TakeoverMode::None, 0.9, &[10, 20]);
        assert_eq!(run[0], (Some(Action::Position(share(10))), share(10)));
        assert_eq!(run[1], (Some(Action::Position(share(20))), share(20)));
    }

    // Spec: remote-mapping R30
    #[test]
    fn pick_up_waits_until_the_controller_crosses_the_control_and_then_follows() {
        // The control is at 0.5 (63.5 of 127): nothing until the controller
        // goes from 63 to 64, then it follows, up and down.
        let run = drive(TakeoverMode::PickUp, 0.5, &[10, 40, 63, 64, 90, 70]);
        let moved: Vec<bool> = run.iter().map(|(a, _)| a.is_some()).collect();
        assert_eq!(moved, [false, false, false, true, true, true]);
        assert_eq!(run[3].1, share(64));
        assert_eq!(run[5].1, share(70));
        // The first message does nothing unless it is the control's own.
        assert!(drive(TakeoverMode::PickUp, share(33), &[33])[0].0.is_some());
        assert_eq!(drive(TakeoverMode::PickUp, share(33), &[34])[0].0, None);
        // From above, too.
        let run = drive(TakeoverMode::PickUp, 0.2, &[120, 80, 30, 20, 10]);
        let moved: Vec<bool> = run.iter().map(|(a, _)| a.is_some()).collect();
        assert_eq!(moved, [false, false, false, true, true]);
    }

    // Spec: remote-mapping R29, R30
    #[test]
    fn something_else_moving_the_control_makes_pick_up_wait_again() {
        let mapping = mapping(
            RemoteSource::Cc {
                channel: 1,
                controller: 1,
            },
            RemoteTarget::Crossfader,
            Some((0.0, 1.0)),
        );
        let law = Law {
            takeover: TakeoverMode::PickUp,
            ..Law::of(&mapping, 0.0, 1.0)
        };
        let mut memory = Memory::default();
        let mut send = |v: u16, at: f64| {
            act(
                &law,
                Hit::Value {
                    value: v,
                    full: 127,
                },
                0,
                Some(at),
                &mut memory,
            )
        };
        assert!(send(50, 50.0 / 127.0).is_some(), "the control's own value");
        assert!(send(60, 50.0 / 127.0).is_some(), "it followed");
        assert_eq!(send(70, 0.0), None, "a click moved it to 0");
        assert_eq!(send(80, 0.0), None);
        assert_eq!(send(90, 1.0), None, "and again to 1");
        assert!(
            send(127, 1.0).is_some(),
            "the controller reaches the control"
        );
    }

    // Spec: remote-mapping R31
    #[test]
    fn value_scaling_brings_the_control_and_the_controller_to_the_end_together() {
        let run = drive(TakeoverMode::ValueScaling, 0.8, &[20, 40, 80, 127]);
        assert_eq!(run[0].0, None, "the first message changes nothing");
        // From 20 to 40 of 127 the control goes from 0.8 by (0.157 · 0.2 / 0.843).
        let (c, c2) = (share(20), share(40));
        let expected = 0.8 + (c2 - c) * (1.0 - 0.8) / (1.0 - c);
        assert!(
            (run[1].1 - expected).abs() < 1e-12,
            "{} vs {expected}",
            run[1].1
        );
        assert!(run[1].1 < 0.85 && run[2].1 > run[1].1);
        assert!((run[3].1 - 1.0).abs() < 1e-12, "both reach the end");
        // Downwards, to Min.
        let run = drive(TakeoverMode::ValueScaling, 0.3, &[100, 60, 30, 0]);
        assert!(run.iter().skip(1).all(|(_, at)| *at <= 0.3));
        assert!(run[3].1.abs() < 1e-12);
        // Within a step of each other it follows as None does.
        let run = drive(TakeoverMode::ValueScaling, share(41), &[40, 41, 42]);
        assert_eq!(run[1], (Some(Action::Position(share(41))), share(41)));
        assert_eq!(run[2], (Some(Action::Position(share(42))), share(42)));
    }

    proptest! {
        // Spec: remote-mapping R32
        #[test]
        fn the_same_inputs_give_the_same_outputs(
            mode in 0..3usize,
            start in 0.0..=1.0f64,
            values in proptest::collection::vec(0..=127u8, 1..40),
        ) {
            let takeover = TakeoverMode::ALL[mode];
            let (a, b) = (drive(takeover, start, &values), drive(takeover, start, &values));
            prop_assert_eq!(&a, &b);
            for (_, at) in &a {
                prop_assert!((0.0..=1.0).contains(at));
            }
        }

        // Spec: remote-mapping R31
        #[test]
        fn value_scaling_ends_on_the_controllers_end(
            start in 0.0..=1.0f64,
            low in 0..120u8,
            up in any::<bool>(),
        ) {
            let values: Vec<u8> = if up {
                (low..=127).collect()
            } else {
                (0..=127 - low).rev().collect()
            };
            let run = drive(TakeoverMode::ValueScaling, start, &values);
            let last = run.last().unwrap().1;
            prop_assert!((last - if up { 1.0 } else { 0.0 }).abs() < 1e-9, "{last}");
        }

        // Spec: remote-mapping R24
        #[test]
        fn a_relative_value_is_never_larger_than_its_encoding_allows(c in 0..=127u8) {
            for encoding in [
                RelativeEncoding::SignedBit,
                RelativeEncoding::SignedBit2,
                RelativeEncoding::BinOffset,
                RelativeEncoding::TwosComplement,
            ] {
                prop_assert!((-64..=63).contains(&size(encoding, c)));
            }
        }
    }

    // Spec: remote-mapping R20, R22
    #[test]
    fn pitch_bend_is_14_bits_with_its_centre_at_8192_the_half_scale() {
        let m = RemoteMapping::new(
            RemoteSource::PitchBend { channel: 1 },
            RemoteTarget::Solo { track: track() },
            ControlKind::Switch,
            None,
            None,
        )
        .unwrap();
        let law = Law::of(&m, 0.0, 1.0);
        let bend = |lsb: u8, msb: u8| {
            let hit = hit(&m, &message(&[0xE0, lsb, msb]), &mut Memory::default()).unwrap();
            assert!(matches!(hit, Hit::Value { full: 16_383, .. }));
            act(&law, hit, 0, None, &mut Memory::default())
        };
        assert_eq!(bend(0x00, 0x40), Some(Action::Set(true)), "centre, 8192");
        assert_eq!(bend(0x7F, 0x3F), Some(Action::Set(false)), "8191");
        assert_eq!(bend(0x7F, 0x7F), Some(Action::Set(true)), "top");
        assert_eq!(bend(0x00, 0x00), Some(Action::Set(false)), "bottom");
    }
    fn sent(m: &RemoteMapping, min: f64, max: f64, state: State) -> Vec<Vec<u8>> {
        feedback(m, min, max, state)
            .iter()
            .map(|m| m.bytes().to_vec())
            .collect()
    }

    // Spec: remote-mapping R21, R22, R38
    #[test]
    fn a_continuous_control_sends_the_inverse_of_its_law_on_each_encoding() {
        let on = |source| mapping(source, RemoteTarget::Crossfader, Some((0.0, 1.0)));
        let cc = on(RemoteSource::Cc {
            channel: 1,
            controller: 28,
        });
        assert_eq!(
            sent(&cc, 0.0, 1.0, State::Value(0.5)),
            [[0xB0, 28, 64]],
            "0.5 of 127"
        );
        assert_eq!(
            sent(&cc, 1.0, 0.0, State::Value(0.25)),
            [[0xB0, 28, 95]],
            "Min above Max sends the inverted value"
        );
        assert_eq!(
            sent(&cc, 0.25, 0.75, State::Value(0.0)),
            [[0xB0, 28, 0]],
            "outside the range clamps"
        );
        let wide = on(RemoteSource::Cc14 {
            channel: 2,
            controller: 7,
        });
        assert_eq!(
            sent(&wide, 0.0, 1.0, State::Value(0.5)),
            [[0xB1, 7, 64], [0xB1, 39, 0]],
            "a 14-bit pair, the high part then the low"
        );
        let bend = on(RemoteSource::PitchBend { channel: 3 });
        assert_eq!(
            sent(&bend, 0.0, 1.0, State::Value(0.5)),
            [[0xE2, 0, 64]],
            "8192, low then high"
        );
    }

    // Spec: remote-mapping R38
    #[test]
    fn a_note_shows_a_switch_or_a_slot_with_velocity_127_or_0_and_nothing_else() {
        let note = RemoteSource::Note {
            channel: 1,
            key: 36,
        };
        let solo = mapping(note.clone(), RemoteTarget::Solo { track: track() }, None);
        assert_eq!(sent(&solo, 0.0, 1.0, State::On(true)), [[0x90, 36, 127]]);
        assert_eq!(sent(&solo, 0.0, 1.0, State::On(false)), [[0x90, 36, 0]]);
        let scene = bl_model::SceneId(Id::new(3).unwrap());
        let slot = mapping(
            note.clone(),
            RemoteTarget::Slot {
                track: track(),
                scene,
            },
            None,
        );
        assert_eq!(sent(&slot, 0.0, 1.0, State::On(true)), [[0x90, 36, 127]]);
        let fader = mapping(note, RemoteTarget::Crossfader, Some((0.0, 1.0)));
        assert!(sent(&fader, 0.0, 1.0, State::Value(0.5)).is_empty());
        let range = mapping(
            RemoteSource::NoteRange {
                channel: 1,
                low: 36,
                high: 40,
                root: None,
            },
            RemoteTarget::Slot {
                track: track(),
                scene,
            },
            None,
        );
        assert!(sent(&range, 0.0, 1.0, State::On(true)).is_empty());
        let key = mapping(
            RemoteSource::Key { key: "q".into() },
            RemoteTarget::Solo { track: track() },
            None,
        );
        assert!(sent(&key, 0.0, 1.0, State::On(true)).is_empty());
    }

    // Spec: remote-mapping R20, R38
    #[test]
    fn a_controller_shows_a_switch_by_the_value_that_turns_it_on_or_off() {
        let source = RemoteSource::Cc {
            channel: 1,
            controller: 23,
        };
        let activator = mapping(
            source.clone(),
            RemoteTarget::Mixer {
                track: track(),
                param: MixerParam::Mute,
            },
            Some((64.0, 100.0)),
        );
        assert_eq!(
            sent(&activator, 64.0, 100.0, State::On(true)),
            [[0xB0, 23, 100]]
        );
        assert_eq!(
            sent(&activator, 64.0, 100.0, State::On(false)),
            [[0xB0, 23, 0]]
        );
        assert_eq!(
            sent(&activator, 100.0, 20.0, State::On(true)),
            [[0xB0, 23, 0]],
            "Min above Max: on is outside"
        );
        assert_eq!(
            sent(&activator, 100.0, 20.0, State::On(false)),
            [[0xB0, 23, 20]]
        );
        let solo = mapping(source, RemoteTarget::Solo { track: track() }, None);
        assert_eq!(sent(&solo, 0.0, 1.0, State::On(true)), [[0xB0, 23, 127]]);
        assert_eq!(sent(&solo, 0.0, 1.0, State::On(false)), [[0xB0, 23, 0]]);
    }

    // Spec: remote-mapping R38
    #[test]
    fn a_relative_mapping_shows_the_position_as_0_to_127_on_its_controller() {
        let relative = RemoteMapping::new(
            RemoteSource::Cc {
                channel: 1,
                controller: 29,
            },
            RemoteTarget::Crossfader,
            ControlKind::Continuous,
            Some((0.0, 1.0)),
            Some(RemoteMode::Relative {
                encoding: RelativeEncoding::SignedBit,
                accelerated: false,
            }),
        )
        .unwrap();
        assert_eq!(
            sent(&relative, 0.0, 1.0, State::Value(1.0)),
            [[0xB0, 29, 127]]
        );
        assert_eq!(
            sent(&relative, 0.0, 1.0, State::Value(0.0)),
            [[0xB0, 29, 0]]
        );
    }
}

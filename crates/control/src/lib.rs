//! MIDI and key mapping: which messages a set's mappings take, and what a
//! message does to the control it maps (`docs/specs/remote-mapping.md`, R7
//! to R13 and R32).
//!
//! Everything here is a pure function of the mapping, the message and the
//! little state a mapping keeps between messages, so the same message on the
//! same state gives the same result on every platform and the window is not
//! needed to test it. The window's side (running the control's command) is
//! `bl-shell`'s.
//!
//! The laws are those of WP-7.1's tests: a note or a key on a slot, a
//! switch, a button, a radio button or a continuous control, and an absolute
//! controller. WP-7.2 (relative controllers, Takeover Mode, chromatic slots)
//! replaces [`act`] and builds on [`Hit`].

use bl_core::midi::MidiMessage;
use bl_midi_io::RemoteBits;
use bl_model::{ControlKind, RemoteMapping, RemoteMode, RemoteSource};

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

/// What a mapping remembers between messages: the high part of a 14-bit
/// pair, and whether a slot's or a button's press is held (R20, R22).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    high: u8,
    pressed: Option<bool>,
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

/// What a control does with a hit (R15 to R21).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    /// A slot's or a button's press, at a velocity from 1 to 127.
    Press(u8),
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
}

impl Law {
    /// The law of a mapping, with `min` and `max` as the positions the
    /// caller worked out for a continuous control.
    #[must_use]
    pub fn of(mapping: &RemoteMapping, min: f64, max: f64) -> Self {
        let keys = match mapping.source {
            RemoteSource::NoteRange { low, high, .. } => u32::from(high - low) + 1,
            _ => 1,
        };
        Self {
            kind: mapping.kind,
            min,
            max,
            mode: mapping.mode,
            keys,
        }
    }
}

/// What `hit` does to a control under `law`, updating the mapping's
/// `memory`; `key` is the offset of the hit note from the range's lowest.
/// A hit the laws of this work package don't cover, such as a relative
/// controller's, does nothing.
// shortcut: relative controllers, Takeover Mode and a slot's chromatic
// transposition belong to WP-7.2 and WP-7.2b, which replace this function
#[must_use]
pub fn act(law: &Law, hit: Hit, key: u32, memory: &mut Memory) -> Option<Action> {
    let range = |share: f64| law.min + share * (law.max - law.min);
    match (hit, law.kind) {
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
        (Hit::Value { value, full }, kind)
            if matches!(law.mode, RemoteMode::Absolute | RemoteMode::Absolute14) =>
        {
            let share = f64::from(value) / f64::from(full);
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
                    let (low, high) = (law.min.min(law.max), law.min.max(law.max));
                    let inside = (low..=high).contains(&c.round());
                    Some(Action::Set(inside == (law.min <= law.max)))
                }
                ControlKind::Switch => Some(Action::Set(value >= half)),
                ControlKind::Radio { options } => {
                    Some(Action::Option(nearest(range(share), options)))
                }
                ControlKind::Continuous => Some(Action::Position(range(share))),
            }
        }
        _ => None,
    }
}

/// The option of a radio button of `options` nearest `position`.
fn nearest(position: f64, options: u32) -> u32 {
    (position.round().max(0.0) as u32).min(options.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use bl_core::id::Id;
    use bl_model::{MixerParam, RemoteTarget, TrackId};

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
                act(&law, hit, 0, &mut memory)
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
        assert_eq!(act(&law, on, 0, &mut memory), Some(Action::Toggle));
        assert_eq!(act(&law, Hit::NoteOff { key: 36 }, 0, &mut memory), None);

        let assign = mapping(
            note.clone(),
            RemoteTarget::CrossfadeAssign { track: track() },
            None,
        );
        let law = Law::of(&assign, 0.0, 2.0);
        assert_eq!(act(&law, on, 0, &mut memory), Some(Action::Next));

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
        assert_eq!(act(&law, on, 0, &mut memory), Some(Action::Position(0.2)));
        assert_eq!(act(&law, on, 4, &mut memory), Some(Action::Position(0.8)));

        let single = Law::of(
            &mapping(note, RemoteTarget::Crossfader, Some((0.0, 1.0))),
            0.0,
            1.0,
        );
        assert_eq!(act(&single, on, 0, &mut memory), Some(Action::MaxOrMin));

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
}

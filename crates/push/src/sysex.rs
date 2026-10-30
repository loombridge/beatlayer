//! Push 2's system exclusive messages: the commands Beatlayer sends, the
//! replies it reads, and the allowlist of what it may ever send
//! (`docs/specs/push-2.md`, R4 and R5).

/// Ableton's sysex ID, the device ID and the model ID that open every command.
const HEADER: [u8; 6] = [0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01];

/// The MIDI Device Inquiry, which has a format of its own.
pub const DEVICE_INQUIRY: [u8; 6] = [0xF0, 0x7E, 0x01, 0x06, 0x01, 0xF7];

/// The commands Beatlayer sends, by the interface manual's IDs. A command not
/// listed is never sent (R5).
pub mod id {
    /// Set LED Color Palette Entry.
    pub const SET_PALETTE_ENTRY: u8 = 0x03;
    /// Reapply Color Palette.
    pub const REAPPLY_PALETTE: u8 = 0x05;
    /// Set LED Brightness.
    pub const SET_LED_BRIGHTNESS: u8 = 0x06;
    /// Set Display Brightness.
    pub const SET_DISPLAY_BRIGHTNESS: u8 = 0x08;
    /// Set MIDI Mode.
    pub const SET_MIDI_MODE: u8 = 0x0A;
    /// Set Touch Strip Configuration.
    pub const SET_TOUCH_STRIP_CONFIG: u8 = 0x17;
    /// Set Touch Strip LEDs.
    pub const SET_TOUCH_STRIP_LEDS: u8 = 0x19;
    /// Request Statistics.
    pub const REQUEST_STATISTICS: u8 = 0x1A;
    /// Set Pad Parameters.
    pub const SET_PAD_PARAMETERS: u8 = 0x1B;
    /// Set Aftertouch Mode.
    pub const SET_AFTERTOUCH_MODE: u8 = 0x1E;
    /// Set Pad Velocity Curve Entry.
    pub const SET_VELOCITY_CURVE: u8 = 0x20;

    /// Every command the surface may send.
    pub const ALLOWED: [u8; 11] = [
        SET_PALETTE_ENTRY,
        REAPPLY_PALETTE,
        SET_LED_BRIGHTNESS,
        SET_DISPLAY_BRIGHTNESS,
        SET_MIDI_MODE,
        SET_TOUCH_STRIP_CONFIG,
        SET_TOUCH_STRIP_LEDS,
        REQUEST_STATISTICS,
        SET_PAD_PARAMETERS,
        SET_AFTERTOUCH_MODE,
        SET_VELOCITY_CURVE,
    ];

    /// Flash LED White Balance, Set Temporary 400g Pad Values and Set LED
    /// White Balance: changes that outlast a restart of the device, which
    /// Beatlayer never sends (R5).
    pub const NEVER: [u8; 3] = [0x23, 0x22, 0x14];
}

/// The command `id` with its arguments, as a whole message.
#[must_use]
pub fn command(id: u8, args: &[u8]) -> Vec<u8> {
    let mut bytes = HEADER.to_vec();
    bytes.push(id);
    bytes.extend_from_slice(args);
    bytes.push(0xF7);
    bytes
}

/// Whether `bytes` is a system exclusive message the surface may send: the
/// Device Inquiry, or one of [`id::ALLOWED`] with at most 17 arguments, all
/// seven-bit (R5).
#[must_use]
pub fn allowed(bytes: &[u8]) -> bool {
    if bytes == DEVICE_INQUIRY {
        return true;
    }
    let Some(rest) = bytes.strip_prefix(&HEADER) else {
        return false;
    };
    let Some((&end, body)) = rest.split_last() else {
        return false;
    };
    let Some((&command, args)) = body.split_first() else {
        return false;
    };
    end == 0xF7
        && id::ALLOWED.contains(&command)
        && args.len() <= 17
        && args.iter().all(|b| *b < 0x80)
}

/// Whether `bytes` is a system exclusive message at all.
#[must_use]
pub fn is_sysex(bytes: &[u8]) -> bool {
    bytes.first() == Some(&0xF0)
}

/// Set LED Color Palette Entry: entry `index` is `rgb`, and `white` for the
/// white LEDs (R14).
#[must_use]
pub fn palette_entry(index: u8, rgb: [u8; 3], white: u8) -> Vec<u8> {
    let mut args = vec![index & 0x7F];
    for value in [rgb[0], rgb[1], rgb[2], white] {
        args.extend([value & 0x7F, value >> 7]);
    }
    command(id::SET_PALETTE_ENTRY, &args)
}

/// Reapply Color Palette.
#[must_use]
pub fn reapply_palette() -> Vec<u8> {
    command(id::REAPPLY_PALETTE, &[])
}

/// Set LED Brightness, 0 to 127.
#[must_use]
pub fn led_brightness(value: u8) -> Vec<u8> {
    command(id::SET_LED_BRIGHTNESS, &[value.min(127)])
}

/// Set Display Brightness, 0 to 255.
#[must_use]
pub fn display_brightness(value: u8) -> Vec<u8> {
    command(id::SET_DISPLAY_BRIGHTNESS, &[value & 0x7F, value >> 7])
}

/// The MIDI modes Set MIDI Mode takes (R4, R33).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiMode {
    /// Controls on the Live port, which the surface reads.
    Live = 0,
    /// Controls on the User port.
    User = 1,
}

/// Set MIDI Mode.
#[must_use]
pub fn midi_mode(mode: MidiMode) -> Vec<u8> {
    command(id::SET_MIDI_MODE, &[mode as u8])
}

/// Request Statistics, leaving the run ID as it is.
#[must_use]
pub fn request_statistics() -> Vec<u8> {
    command(id::REQUEST_STATISTICS, &[0])
}

/// Set Touch Strip Configuration with the flags `flags` (R11).
#[must_use]
pub fn touch_strip_config(flags: u8) -> Vec<u8> {
    command(id::SET_TOUCH_STRIP_CONFIG, &[flags & 0x7F])
}

/// Set Touch Strip LEDs: 31 levels from 0 to 7, the bottom LED first (R11).
#[must_use]
pub fn touch_strip_leds(levels: &[u8; 31]) -> Vec<u8> {
    let mut args = [0u8; 16];
    for (n, level) in levels.iter().enumerate() {
        args[n / 2] |= (level & 7) << (3 * (n % 2));
    }
    command(id::SET_TOUCH_STRIP_LEDS, &args)
}

/// Set Pad Parameters with the aftertouch thresholds `low` and `high` (R30).
#[must_use]
pub fn pad_parameters(low: u16, high: u16) -> Vec<u8> {
    let split = |value: u16| [(value & 0x7F) as u8, (value >> 7) as u8];
    let (low, high) = (split(low), split(high));
    command(
        id::SET_PAD_PARAMETERS,
        &[0, 0, 0, 0, low[0], low[1], high[0], high[1]],
    )
}

/// Set Aftertouch Mode: channel pressure (false) or polyphonic key pressure.
#[must_use]
pub fn aftertouch_mode(polyphonic: bool) -> Vec<u8> {
    command(id::SET_AFTERTOUCH_MODE, &[u8::from(polyphonic)])
}

/// Set Pad Velocity Curve Entry for the sixteen entries from `start`.
#[must_use]
pub fn velocity_curve_entries(start: u8, entries: &[u8]) -> Vec<u8> {
    let mut args = vec![start];
    args.extend_from_slice(entries);
    command(id::SET_VELOCITY_CURVE, &args)
}

/// What a reply from Push 2 says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    /// The Device Inquiry reply of a Push 2, with its firmware as
    /// `major.minor.build`.
    Identity {
        /// The firmware version.
        firmware: String,
    },
    /// A Device Inquiry reply of some other device.
    OtherDevice,
    /// Request Statistics: whether Push 2 has its own power supply.
    Statistics {
        /// True with the external power supply.
        external_power: bool,
    },
    /// Set MIDI Mode: the mode now in force.
    MidiMode(u8),
    /// Any other sysex message.
    Other,
}

/// Reads a sysex message from Push 2.
#[must_use]
pub fn parse(bytes: &[u8]) -> Reply {
    // F0 7E dev 06 02, Ableton's ID, family 0x1967 and member 2 (R1).
    if let [0xF0, 0x7E, _, 0x06, 0x02, rest @ ..] = bytes {
        return match rest {
            [0x00, 0x21, 0x1D, 0x67, 0x32, 0x02, 0x00, major, minor, build_low, build_high, ..] => {
                let build = u16::from(*build_low) | u16::from(*build_high) << 7;
                Reply::Identity {
                    firmware: format!("{major}.{minor}.{build}"),
                }
            }
            _ => Reply::OtherDevice,
        };
    }
    match bytes.strip_prefix(&HEADER) {
        Some([id::REQUEST_STATISTICS, power, ..]) => Reply::Statistics {
            external_power: *power == 1,
        },
        Some([id::SET_MIDI_MODE, mode, ..]) => Reply::MidiMode(*mode),
        _ => Reply::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: push-2 R5. The interface manual's examples, byte for byte.
    #[test]
    fn commands_are_the_interface_manuals_examples() {
        assert_eq!(
            palette_entry(125, [0, 0, 255], 126),
            [0xF0, 0, 0x21, 0x1D, 1, 1, 3, 0x7D, 0, 0, 0, 0, 0x7F, 1, 0x7E, 0, 0xF7]
        );
        assert_eq!(
            led_brightness(0x40),
            [0xF0, 0, 0x21, 0x1D, 1, 1, 6, 0x40, 0xF7]
        );
        assert_eq!(
            display_brightness(255),
            [0xF0, 0, 0x21, 0x1D, 1, 1, 8, 0x7F, 1, 0xF7]
        );
        assert_eq!(
            touch_strip_config(0x68),
            [0xF0, 0, 0x21, 0x1D, 1, 1, 0x17, 0x68, 0xF7]
        );
        assert_eq!(
            pad_parameters(1220, 1570),
            [0xF0, 0, 0x21, 0x1D, 1, 1, 0x1B, 0, 0, 0, 0, 0x44, 9, 0x22, 0x0C, 0xF7]
        );
        let mut levels = [4u8; 31];
        for n in (0..31).step_by(4) {
            levels[n] = 7;
        }
        let sent = touch_strip_leds(&levels);
        assert_eq!(sent[7], 0x27, "LED 0 full, LED 1 half");
        assert_eq!(sent.len(), 6 + 1 + 16 + 1);
    }

    // Spec: push-2 R5. Only the documented commands, never the three that
    // outlast a restart.
    #[test]
    fn the_allowlist_takes_what_beatlayer_sends_and_none_of_the_three() {
        for command in [
            led_brightness(10),
            midi_mode(MidiMode::Live),
            request_statistics(),
            reapply_palette(),
            DEVICE_INQUIRY.to_vec(),
        ] {
            assert!(allowed(&command), "{command:02X?}");
        }
        for never in id::NEVER {
            assert!(!allowed(&command(never, &[0, 0])), "{never:02X}");
        }
        assert!(!allowed(&[0xF0, 0x00, 0x21, 0x1D, 1, 1, 0x7F, 0xF7]));
        assert!(!allowed(&[0xF0, 0x7D, 1, 0xF7]));
    }

    #[test]
    fn replies_are_read_as_the_interface_manual_gives_them() {
        let identity = [
            0xF0, 0x7E, 0x01, 0x06, 0x02, 0x00, 0x21, 0x1D, 0x67, 0x32, 0x02, 0x00, 0x01, 0x00,
            0x2F, 0x00, 0x73, 0x4D, 0x1F, 0x08, 0x00, 0x01, 0xF7,
        ];
        assert_eq!(
            parse(&identity),
            Reply::Identity {
                firmware: "1.0.47".to_owned()
            }
        );
        let mut other = identity;
        other[8] = 0x10;
        assert_eq!(parse(&other), Reply::OtherDevice);
        assert_eq!(
            parse(&command(id::REQUEST_STATISTICS, &[1, 0, 0x3F, 7, 0, 0, 0])),
            Reply::Statistics {
                external_power: true
            }
        );
        assert_eq!(parse(&midi_mode(MidiMode::User)), Reply::MidiMode(1));
    }
}

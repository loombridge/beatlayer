//! MIDI ports and the computer MIDI keyboard (WP-2.2; the MIDI I/O spec,
//! R6 and R24): `midi.ports`, `midi.set_port`, and the four computer
//! keyboard commands, with their arguments and results.
//!
//! [`register`] adds their specs, which are pure data, so `cargo xtask` can
//! check them and list them in the command reference; `bl-shell` runs them by
//! ID. A track's MIDI From and channel are the routing commands'
//! (`routing.set_input`), not repeated here.

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `midi.ports`: list the MIDI ports and the computer keyboard's state.
pub const PORTS: &str = "midi.ports";
/// `midi.set_port`: change a port's Track, Sync, Remote and MPE switches.
pub const SET_PORT: &str = "midi.set_port";
/// `midi.set_show_resync_button`: Show Resync Button in Settings ▸ Tempo &
/// MIDI (the MIDI clock spec, R10).
pub const SET_SHOW_RESYNC_BUTTON: &str = "midi.set_show_resync_button";
/// `midi.toggle_computer_keyboard`: switch the computer MIDI keyboard.
pub const TOGGLE_COMPUTER_KEYBOARD: &str = "midi.toggle_computer_keyboard";
/// `midi.computer_keyboard_octave`: move the computer keyboard's octave.
pub const COMPUTER_KEYBOARD_OCTAVE: &str = "midi.computer_keyboard_octave";
/// `midi.computer_keyboard_velocity`: step the computer keyboard's velocity.
pub const COMPUTER_KEYBOARD_VELOCITY: &str = "midi.computer_keyboard_velocity";
/// `midi.computer_keyboard_key`: press or release one of its note keys.
pub const COMPUTER_KEYBOARD_KEY: &str = "midi.computer_keyboard_key";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    PORTS,
    SET_PORT,
    SET_SHOW_RESYNC_BUTTON,
    TOGGLE_COMPUTER_KEYBOARD,
    COMPUTER_KEYBOARD_OCTAVE,
    COMPUTER_KEYBOARD_VELOCITY,
    COMPUTER_KEYBOARD_KEY,
];

/// Whether `id` is one of the MIDI I/O commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Whether a MIDI port takes messages in or sends them out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortDirection {
    /// An input: it brings messages into Beatlayer.
    Input,
    /// An output: tracks can send messages to it.
    Output,
}

impl PortDirection {
    /// How a port's row in the Settings window starts: `In` or `Out`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Input => "In",
            Self::Output => "Out",
        }
    }
}

/// An output's MIDI Clock Type (the MIDI clock spec, R1, R5, R6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClockTypeChoice {
    /// Song Position Pointer and Continue each time the play position
    /// changes.
    #[default]
    Song,
    /// Only Start, at the start of the next bar.
    Pattern,
}

impl ClockTypeChoice {
    /// Every choice, in the order the chooser lists them.
    pub const ALL: [Self; 2] = [Self::Song, Self::Pattern];

    /// The chooser's text.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Song => "Song",
            Self::Pattern => "Pattern",
        }
    }
}

impl From<ClockTypeChoice> for bl_core::midi::ClockType {
    fn from(choice: ClockTypeChoice) -> Self {
        match choice {
            ClockTypeChoice::Song => Self::Song,
            ClockTypeChoice::Pattern => Self::Pattern,
        }
    }
}

impl From<bl_core::midi::ClockType> for ClockTypeChoice {
    fn from(clock_type: bl_core::midi::ClockType) -> Self {
        match clock_type {
            bl_core::midi::ClockType::Song => Self::Song,
            bl_core::midi::ClockType::Pattern => Self::Pattern,
        }
    }
}

/// An output's Hardware Resync (the MIDI clock spec, R1, R11).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HardwareResyncChoice {
    /// Stop, then Start.
    #[default]
    StopAndStart,
    /// Start.
    StartOnly,
    /// Nothing.
    DontResync,
}

impl HardwareResyncChoice {
    /// Every choice, in the order the chooser lists them.
    pub const ALL: [Self; 3] = [Self::StopAndStart, Self::StartOnly, Self::DontResync];

    /// The chooser's text.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::StopAndStart => "Stop and Start",
            Self::StartOnly => "Start Only",
            Self::DontResync => "Don't Resync",
        }
    }
}

impl From<HardwareResyncChoice> for bl_core::midi::HardwareResync {
    fn from(choice: HardwareResyncChoice) -> Self {
        match choice {
            HardwareResyncChoice::StopAndStart => Self::StopAndStart,
            HardwareResyncChoice::StartOnly => Self::StartOnly,
            HardwareResyncChoice::DontResync => Self::Off,
        }
    }
}

impl From<bl_core::midi::HardwareResync> for HardwareResyncChoice {
    fn from(resync: bl_core::midi::HardwareResync) -> Self {
        match resync {
            bl_core::midi::HardwareResync::StopAndStart => Self::StopAndStart,
            bl_core::midi::HardwareResync::StartOnly => Self::StartOnly,
            bl_core::midi::HardwareResync::Off => Self::DontResync,
        }
    }
}

/// The range of a port's MIDI Clock Sync Delay in milliseconds (the MIDI
/// clock spec, R2).
pub const SYNC_DELAY_RANGE: core::ops::RangeInclusive<i16> = -100..=100;

/// One MIDI port as the Settings window's table shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortInfo {
    /// Input or output.
    pub direction: PortDirection,
    /// The name the MIDI system gives it.
    pub name: String,
    /// Track: an input's notes and controllers reach tracks; an output takes
    /// tracks' MIDI.
    pub track: bool,
    /// Sync: an input's clock and transport messages are followed; an output
    /// sends clock. Kept now, acted on when sync is built.
    pub sync: bool,
    /// Remote: an input controls Beatlayer through the set's MIDI mappings
    /// (the remote mapping spec, R7 and R8); an output gets the values of
    /// the mapped controls as feedback (R38 to R40).
    pub remote: bool,
    /// MPE: an input's messages reach tracks as MPE, each note on a channel of
    /// its own; an output has no MPE switch, and reads false.
    #[serde(default)]
    pub mpe: bool,
    /// MIDI Clock Sync Delay in milliseconds, from -100 to 100, added to
    /// the clock an output sends; kept while Sync is off.
    #[serde(default)]
    pub sync_delay_ms: i16,
    /// An output's MIDI Clock Type: Song or Pattern.
    #[serde(default)]
    pub clock_type: ClockTypeChoice,
    /// An output's Hardware Resync: what Resync External Hardware sends it.
    #[serde(default)]
    pub hardware_resync: HardwareResyncChoice,
    /// Whether the MIDI system offers the port now.
    pub present: bool,
    /// Whether it is open: any of its switches is on.
    pub open: bool,
    /// Why it couldn't be opened, if it couldn't.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The computer MIDI keyboard's state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KeyboardInfo {
    /// Whether it is on.
    pub on: bool,
    /// Its octave: 0 puts the A key on C3, -5 on C-2, 5 on C8.
    pub octave: i8,
    /// The velocity its notes play at.
    pub velocity: u8,
    /// The MIDI note the A key plays.
    pub lowest_note: u8,
    /// The notes its keys reach, as `C3 to D#4`.
    pub range: String,
}

/// What `midi.ports` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MidiPortsResult {
    /// The MIDI system in use: Core MIDI, ALSA, Windows Multimedia MIDI, or
    /// none.
    pub system: String,
    /// Every port the system offers, inputs first.
    pub ports: Vec<PortInfo>,
    /// The computer MIDI keyboard.
    pub keyboard: KeyboardInfo,
    /// Whether Show Resync Button is on, so the Control Bar shows the
    /// Resync External Hardware button.
    #[serde(default)]
    pub show_resync_button: bool,
}

/// Arguments of `midi.set_port`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetPortArgs {
    /// Input or output.
    pub direction: PortDirection,
    /// The port's name, as `midi.ports` lists it.
    pub name: String,
    /// Track on or off. Left out, it stays as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<bool>,
    /// Sync on or off. Left out, it stays as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<bool>,
    /// Remote on or off. Left out, it stays as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<bool>,
    /// MPE on or off, for an input. Left out, it stays as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mpe: Option<bool>,
    /// MIDI Clock Sync Delay in milliseconds, from -100 to 100. Left out, it
    /// stays as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = -100, max = 100))]
    pub sync_delay_ms: Option<i16>,
    /// An output's MIDI Clock Type. Left out, it stays as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_type: Option<ClockTypeChoice>,
    /// An output's Hardware Resync. Left out, it stays as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardware_resync: Option<HardwareResyncChoice>,
}

/// What `midi.set_show_resync_button` returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SetShowResyncButtonResult {
    /// Whether Show Resync Button is on now.
    pub on: bool,
}

/// Arguments of `midi.set_show_resync_button`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetShowResyncButtonArgs {
    /// Show the button or hide it. Left out, it switches to the other state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
}

/// Arguments of `midi.toggle_computer_keyboard`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToggleKeyboardArgs {
    /// Switch it on or off. Left out, it switches to the other state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
}

/// Arguments of `midi.computer_keyboard_octave` and
/// `midi.computer_keyboard_velocity`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StepArgs {
    /// The steps to move: -1 for down (Z, C), 1 for up (X, V). Any number
    /// of steps is allowed; it stops at the ends of the range.
    #[schemars(range(min = -16, max = 16))]
    pub steps: i8,
}

/// Arguments of `midi.computer_keyboard_key`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyArgs {
    /// The note key's letter: A W S E D F T G Y H U J K O L P, from the
    /// lowest note up.
    pub key: String,
    /// Whether the key goes down (the note starts) or comes up (it ends).
    pub pressed: bool,
}

/// What `midi.computer_keyboard_key` returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KeyboardKeyResult {
    /// Whether a note message went to the tracks: false for a press whose
    /// note would pass 127, a press of a key already down, or a release of a
    /// key that held no note.
    pub sent: bool,
    /// The MIDI note it started or ended, if a message went.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<u8>,
}

/// Adds the MIDI I/O commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(PORTS, "MIDI Ports")
            .description(
                "Lists every MIDI input and output port the computer offers, each with its \
                 Track, Sync, Remote and MPE switches as the Settings window's Tempo & MIDI page \
                 shows them, with each port's MIDI Clock Sync Delay, MIDI Clock Type and \
                 Hardware Resync, the computer MIDI keyboard's state, and whether Show Resync \
                 Button is on. Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<MidiPortsResult>(),
    )?;
    registry.register(
        CommandSpec::new(SET_PORT, "Set MIDI Port")
            .description(
                "Changes a MIDI port's Track, Sync, Remote and MPE switches, as the Settings \
                 window's MIDI ports table does; switches left out stay as they are. A port is \
                 open while its Track, Sync or Remote switch is on and closed when all three \
                 are off; MPE opens nothing by itself. An input's notes and controllers reach \
                 tracks only while its Track switch is on, and as MPE, each note on a channel \
                 of its own with its own pitch, slide and pressure, while its MPE switch is on \
                 too; an input with Remote on feeds the set's MIDI mappings, and a message one takes \
                 never reaches a track (`remote.mappings`). An output with Sync on gets MIDI \
                 clock, Start, Continue, Stop and Song \
                 Position Pointer while the song plays, whatever its Track switch, stamped for \
                 the moment each is heard: `sync_delay_ms` (-100 to 100) moves them earlier or \
                 later, `clock_type` is Song (Song Position Pointer and Continue whenever the \
                 play position changes) or Pattern (only Start, at the next bar line), and \
                 `hardware_resync` is what Resync External Hardware sends it: Stop and Start, \
                 Start Only or Don't Resync. Those three are kept while Sync is off; \
                 `clock_type` and `hardware_resync` are refused on an input. An output's Remote is \
                 kept and acts when it's built. Output ports have no MPE switch, and `mpe` on \
                 one is refused. The port's \
                 switches are kept for the next start, and aren't part of the set or an undo \
                 step. Opens or closes a device outside Beatlayer. Fails if the computer \
                 doesn't offer the port now.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<SetPortArgs>()
            .result::<PortInfo>(),
    )?;
    registry.register(
        CommandSpec::new(SET_SHOW_RESYNC_BUTTON, "Show Resync Button")
            .description(
                "Sets Show Resync Button, the switch on the second section of Settings ▸ Tempo & \
                 MIDI, to `on`, or to the other state if it's left out. While it's on, the \
                 Control Bar shows the Resync External Hardware button. It starts off, is kept \
                 for the next start, and isn't part of the set or an undo step.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<SetShowResyncButtonArgs>()
            .result::<SetShowResyncButtonResult>(),
    )?;
    registry.register(
        CommandSpec::new(TOGGLE_COMPUTER_KEYBOARD, "Computer MIDI Keyboard")
            .description(
                "Switches the computer MIDI keyboard, as the Control Bar's switch and M do, to \
                 `on`, or to the other state if it's left out. While it's on, the letter keys \
                 play notes (A W S E D F T G Y H U J K O L P) to the tracks whose MIDI From is \
                 Computer Keyboard or All Ins; Z and X move its octave, C and V its velocity, \
                 and the letters it uses don't run their shortcuts unless Shift is held. \
                 Switching it off ends every note it holds. Whether it's on is kept for the \
                 next start, and isn't part of the set or an undo step.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .binding("S-MAP-003")
            .args::<ToggleKeyboardArgs>()
            .result::<KeyboardInfo>(),
    )?;
    registry.register(
        CommandSpec::new(COMPUTER_KEYBOARD_OCTAVE, "Computer Keyboard Octave")
            .description(
                "Moves the computer MIDI keyboard's octave by `steps`, as Z (down) and X (up) \
                 do: 12 semitones a step, from C-2 to C8 for the A key, stopping at the ends. \
                 Notes already held keep sounding until their keys come up. Works whether or \
                 not the keyboard is on. Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<StepArgs>()
            .result::<KeyboardInfo>(),
    )?;
    registry.register(
        CommandSpec::new(COMPUTER_KEYBOARD_VELOCITY, "Computer Keyboard Velocity")
            .description(
                "Steps the computer MIDI keyboard's velocity by `steps` through 1, 20, 40, 60, \
                 80, 100, 120, and 127, as C (lower) and V (raise) do, stopping at the ends. \
                 It starts at 100. Works whether or not the keyboard is on. Changes nothing in \
                 the set.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<StepArgs>()
            .result::<KeyboardInfo>(),
    )?;
    registry.register(
        CommandSpec::new(COMPUTER_KEYBOARD_KEY, "Computer Keyboard Key")
            .description(
                "Presses or releases one of the computer MIDI keyboard's note keys, as typing \
                 it does: a press sends a note-on at the keyboard's velocity on MIDI channel 1, \
                 a release the note-off of the note that key started, even if the octave moved \
                 meanwhile. The notes reach the tracks whose MIDI From is Computer Keyboard or \
                 All Ins. A press fails while the keyboard is off. Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<KeyArgs>()
            .result::<KeyboardKeyResult>(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_listed_id_is_registered_and_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        let problems: Vec<String> = registry
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(problems.is_empty(), "{problems:#?}");

        let schema = registry.get(SET_PORT).unwrap().args.as_ref().unwrap();
        assert!(
            bl_commands::validate(
                schema,
                &json!({ "direction": "input", "name": "IAC", "track": false })
            )
            .is_ok()
        );
        assert!(bl_commands::validate(schema, &json!({ "name": "IAC" })).is_err());
        let schema = registry
            .get(COMPUTER_KEYBOARD_KEY)
            .unwrap()
            .args
            .as_ref()
            .unwrap();
        assert!(bl_commands::validate(schema, &json!({ "key": "A", "pressed": true })).is_ok());
        assert!(bl_commands::validate(schema, &json!({ "key": "A" })).is_err());
    }
}

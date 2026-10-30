//! Push 2's controls: the table of R6, written from the interface manual's
//! MIDI mapping (`docs/specs/push-2.md`, R6).

/// What kind of LED a button has (R6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedKind {
    /// Colours, by palette index.
    Rgb,
    /// Levels of white, by palette index.
    White,
}

/// A button: its name, its controller number and its LED.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Button {
    /// The name `push.state` uses.
    pub name: &'static str,
    /// The controller number it sends and its LED answers to.
    pub number: u8,
    /// Its LED.
    pub led: LedKind,
}

/// The buttons' controller numbers, for the code that names one.
pub mod cc {
    /// Tap Tempo.
    pub const TAP_TEMPO: u8 = 3;
    /// Metronome.
    pub const METRONOME: u8 = 9;
    /// Lower display button 1; the others follow.
    pub const LOWER_1: u8 = 20;
    /// Stop Clip.
    pub const STOP_CLIP: u8 = 29;
    /// Setup.
    pub const SETUP: u8 = 30;
    /// Layout.
    pub const LAYOUT: u8 = 31;
    /// Convert.
    pub const CONVERT: u8 = 35;
    /// The scene button at the bottom, 8; the others count up from it.
    pub const SCENE_8: u8 = 36;
    /// The scene button at the top, 1.
    pub const SCENE_1: u8 = 43;
    /// Master.
    pub const MASTER: u8 = 28;
    /// Select.
    pub const SELECT: u8 = 48;
    /// Shift.
    pub const SHIFT: u8 = 49;
    /// Note.
    pub const NOTE: u8 = 50;
    /// Session.
    pub const SESSION: u8 = 51;
    /// Scale.
    pub const SCALE: u8 = 58;
    /// User.
    pub const USER: u8 = 59;
    /// Mute.
    pub const MUTE: u8 = 60;
    /// Solo.
    pub const SOLO: u8 = 61;
    /// Foot switch 1, the sustain pedal.
    pub const FOOTSWITCH_1: u8 = 64;
    /// Foot switch 2.
    pub const FOOTSWITCH_2: u8 = 69;
    /// Play.
    pub const PLAY: u8 = 85;
    /// Record.
    pub const RECORD: u8 = 86;
    /// New.
    pub const NEW: u8 = 87;
    /// Duplicate.
    pub const DUPLICATE: u8 = 88;
    /// Automate.
    pub const AUTOMATE: u8 = 89;
    /// Fixed Length.
    pub const FIXED_LENGTH: u8 = 90;
    /// Upper display button 1; the others follow.
    pub const UPPER_1: u8 = 102;
    /// Device.
    pub const DEVICE: u8 = 110;
    /// Browse.
    pub const BROWSE: u8 = 111;
    /// Mix.
    pub const MIX: u8 = 112;
    /// Clip.
    pub const CLIP: u8 = 113;
    /// Quantize.
    pub const QUANTIZE: u8 = 116;
    /// Double Loop.
    pub const DOUBLE_LOOP: u8 = 117;
    /// Delete.
    pub const DELETE: u8 = 118;
    /// Undo.
    pub const UNDO: u8 = 119;
}

const fn rgb(name: &'static str, number: u8) -> Button {
    Button {
        name,
        number,
        led: LedKind::Rgb,
    }
}

const fn white(name: &'static str, number: u8) -> Button {
    Button {
        name,
        number,
        led: LedKind::White,
    }
}

/// Every button with an LED, by the table in R6.
pub const BUTTONS: [Button; 65] = [
    rgb("upper_1", 102),
    rgb("upper_2", 103),
    rgb("upper_3", 104),
    rgb("upper_4", 105),
    rgb("upper_5", 106),
    rgb("upper_6", 107),
    rgb("upper_7", 108),
    rgb("upper_8", 109),
    rgb("lower_1", 20),
    rgb("lower_2", 21),
    rgb("lower_3", 22),
    rgb("lower_4", 23),
    rgb("lower_5", 24),
    rgb("lower_6", 25),
    rgb("lower_7", 26),
    rgb("lower_8", 27),
    rgb("scene_1", 43),
    rgb("scene_2", 42),
    rgb("scene_3", 41),
    rgb("scene_4", 40),
    rgb("scene_5", 39),
    rgb("scene_6", 38),
    rgb("scene_7", 37),
    rgb("scene_8", 36),
    white("tap_tempo", cc::TAP_TEMPO),
    white("metronome", cc::METRONOME),
    white("delete", cc::DELETE),
    white("undo", cc::UNDO),
    rgb("mute", cc::MUTE),
    rgb("solo", cc::SOLO),
    rgb("stop_clip", cc::STOP_CLIP),
    white("convert", cc::CONVERT),
    white("double_loop", cc::DOUBLE_LOOP),
    white("quantize", cc::QUANTIZE),
    white("duplicate", cc::DUPLICATE),
    white("new", cc::NEW),
    white("fixed_length", cc::FIXED_LENGTH),
    rgb("automate", cc::AUTOMATE),
    rgb("record", cc::RECORD),
    rgb("play", cc::PLAY),
    white("setup", cc::SETUP),
    white("user", cc::USER),
    white("add_device", 52),
    white("add_track", 53),
    white("device", cc::DEVICE),
    white("browse", cc::BROWSE),
    white("mix", cc::MIX),
    white("clip", cc::CLIP),
    white("master", cc::MASTER),
    white("up", 46),
    white("down", 47),
    white("left", 44),
    white("right", 45),
    white("repeat", 56),
    white("accent", 57),
    white("scale", cc::SCALE),
    white("layout", cc::LAYOUT),
    white("note", cc::NOTE),
    white("session", cc::SESSION),
    white("octave_up", 55),
    white("octave_down", 54),
    white("page_left", 62),
    white("page_right", 63),
    white("shift", cc::SHIFT),
    white("select", cc::SELECT),
];

/// The button with controller number `number`, if it has an LED.
#[must_use]
pub fn button(number: u8) -> Option<&'static Button> {
    BUTTONS.iter().find(|button| button.number == number)
}

/// The button called `name`.
#[must_use]
pub fn button_named(name: &str) -> Option<&'static Button> {
    BUTTONS.iter().find(|button| button.name == name)
}

/// The lower display button's number for the track at `column`, from 0.
#[must_use]
pub const fn lower(column: u8) -> u8 {
    cc::LOWER_1 + column
}

/// The upper display button's number for `column`, from 0.
#[must_use]
pub const fn upper(column: u8) -> u8 {
    cc::UPPER_1 + column
}

/// The scene button's number for ring row `row`, from 0 at the top.
#[must_use]
pub const fn scene(row: u8) -> u8 {
    cc::SCENE_1 - row
}

/// The column of a lower display button, from 0.
#[must_use]
pub const fn lower_column(number: u8) -> Option<u8> {
    if number >= cc::LOWER_1 && number < cc::LOWER_1 + 8 {
        Some(number - cc::LOWER_1)
    } else {
        None
    }
}

/// The column of an upper display button, from 0.
#[must_use]
pub const fn upper_column(number: u8) -> Option<u8> {
    if number >= cc::UPPER_1 && number < cc::UPPER_1 + 8 {
        Some(number - cc::UPPER_1)
    } else {
        None
    }
}

/// An encoder: the eight above the display, then the master, tempo and swing
/// encoders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Encoder {
    /// Its place: 0 to 7 for the track encoders, then [`MASTER`], [`TEMPO`]
    /// and [`SWING`].
    pub index: u8,
    /// The controller it turns on.
    pub controller: u8,
    /// The note it touches on.
    pub touch: u8,
}

/// The master encoder's index.
pub const MASTER: u8 = 8;
/// The tempo encoder's index.
pub const TEMPO: u8 = 9;
/// The swing encoder's index.
pub const SWING: u8 = 10;

/// Every encoder, as R6 gives them.
pub const ENCODERS: [Encoder; 11] = [
    Encoder { index: 0, controller: 71, touch: 0 },
    Encoder { index: 1, controller: 72, touch: 1 },
    Encoder { index: 2, controller: 73, touch: 2 },
    Encoder { index: 3, controller: 74, touch: 3 },
    Encoder { index: 4, controller: 75, touch: 4 },
    Encoder { index: 5, controller: 76, touch: 5 },
    Encoder { index: 6, controller: 77, touch: 6 },
    Encoder { index: 7, controller: 78, touch: 7 },
    Encoder { index: MASTER, controller: 79, touch: 8 },
    Encoder { index: TEMPO, controller: 14, touch: 10 },
    Encoder { index: SWING, controller: 15, touch: 9 },
];

/// The encoder that turns on controller `number`.
#[must_use]
pub fn encoder_turned(number: u8) -> Option<Encoder> {
    ENCODERS.iter().find(|e| e.controller == number).copied()
}

/// The encoder touched on note `note`.
#[must_use]
pub fn encoder_touched(note: u8) -> Option<Encoder> {
    ENCODERS.iter().find(|e| e.touch == note).copied()
}

/// The touch strip's note (R11).
pub const STRIP_TOUCH: u8 = 12;

/// The note of the pad in row `row` from the bottom and column `column` from
/// the left, both from 0 (R6).
#[must_use]
pub const fn pad_note(row: u8, column: u8) -> u8 {
    36 + 8 * row + column
}

/// The row and column of the pad that sends note `note`.
#[must_use]
pub const fn pad_of(note: u8) -> Option<(u8, u8)> {
    if note >= 36 && note < 100 {
        Some(((note - 36) / 8, (note - 36) % 8))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_has_no_two_buttons_on_one_number_or_name() {
        for (n, a) in BUTTONS.iter().enumerate() {
            for b in &BUTTONS[n + 1..] {
                assert_ne!(a.number, b.number, "{} and {}", a.name, b.name);
                assert_ne!(a.name, b.name);
            }
        }
    }

    #[test]
    fn pads_and_the_numbered_rows_follow_the_map() {
        assert_eq!(pad_note(0, 0), 36);
        assert_eq!(pad_note(7, 7), 99);
        assert_eq!(pad_of(43), Some((0, 7)));
        assert_eq!(pad_of(100), None);
        assert_eq!(scene(0), 43);
        assert_eq!(scene(7), 36);
        assert_eq!(button_named("scene_1").map(|b| b.number), Some(43));
        assert_eq!(lower_column(27), Some(7));
    }
}

//! The colours of tracks, clips and scenes (the themes spec, R14 to R21).

use bl_core::id::Id;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// How many swatches the palette has: 5 rows of 14, numbered 0 to 69 from
/// the top row's left.
pub const SWATCHES: u8 = 70;

/// How many swatches are in the palette's top row: the theme's track colours.
pub const TRACK_COLOURS: u8 = 14;

/// The swatch a track made without a choice takes, and the one a set saved
/// before colours gives each track: `id mod 14`, in the top row (R21).
#[must_use]
pub fn id_swatch(id: Id) -> u8 {
    (id.get() % u64::from(TRACK_COLOURS)) as u8
}

/// The 14 track colours of the bundled dark theme, which the palette is
/// drawn from where no theme is at hand (a headless shell, an import with
/// no window). `bl-ui`'s tests keep it equal to the theme file.
pub const DEFAULT_TRACK_PALETTE: [[u8; 3]; TRACK_COLOURS as usize] = [
    [0xe0, 0x67, 0x5a],
    [0xe8, 0x94, 0x5a],
    [0xe6, 0xc3, 0x5c],
    [0xa8, 0xc9, 0x5a],
    [0x5f, 0xbf, 0x6e],
    [0x4f, 0xbf, 0xa3],
    [0x4f, 0xb0, 0xd6],
    [0x5b, 0x8c, 0xe0],
    [0x7f, 0x74, 0xdb],
    [0xa4, 0x6b, 0xd6],
    [0xd4, 0x6b, 0xb8],
    [0xd9, 0x70, 0x8a],
    [0x9a, 0xa6, 0xb2],
    [0xc2, 0xa3, 0x82],
];

/// The colour of swatch `index` (0 to 69) of the palette drawn from a
/// theme's 14 track colours (the themes spec, R15): the top row as the
/// theme gives it, the second and third rows 30 % and 60 % of the way to
/// white, and the fourth and fifth 30 % and 50 % of the way to black, each
/// channel mixed in gamma space.
#[must_use]
pub fn swatch_rgb(track_colours: &[[u8; 3]], index: u8) -> [u8; 3] {
    let column = usize::from(index) % usize::from(TRACK_COLOURS);
    let row = usize::from(index) / usize::from(TRACK_COLOURS);
    let base = track_colours
        .get(column % track_colours.len().max(1))
        .copied()
        .unwrap_or_default();
    let mix: f32 = [0.0, 0.3, 0.6, -0.3, -0.5][row.min(4)];
    let towards = if mix < 0.0 { 0.0 } else { 255.0 };
    let t = mix.abs();
    base.map(|channel| {
        let channel = f32::from(channel);
        ((1.0 - t) * channel + t * towards + 0.5) as u8
    })
}

/// A colour in a set: one of the palette's 70 swatches, or a colour of the
/// person's own, `#RRGGBB`. A swatch follows the theme's appearance; a
/// custom colour is drawn as saved (the themes spec, R29). Saved and sent
/// as the swatch's number or as the `#RRGGBB` string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Wire", into = "Wire")]
pub enum Colour {
    /// A swatch of the palette, from 0 to 69.
    Swatch(u8),
    /// A colour of the person's own: its red, green and blue.
    Custom([u8; 3]),
}

impl Default for Colour {
    fn default() -> Self {
        Self::Swatch(0)
    }
}

impl From<u8> for Colour {
    fn from(swatch: u8) -> Self {
        Self::Swatch(swatch)
    }
}

impl Colour {
    /// Checks that a swatch is one of the palette's 70 (a custom colour is
    /// always whole).
    ///
    /// # Errors
    /// A swatch from 70 up.
    pub fn check(self, what: &'static str) -> Result<(), crate::error::ModelError> {
        match self {
            Self::Swatch(swatch) => {
                crate::command::check_value(what, f64::from(swatch), swatch < SWATCHES)
            }
            Self::Custom(_) => Ok(()),
        }
    }

    /// The colour drawn from a theme's 14 track colours: a swatch as the
    /// palette gives it (R15), a custom colour as saved (R29).
    #[must_use]
    pub fn rgb(self, track_colours: &[[u8; 3]]) -> [u8; 3] {
        match self {
            Self::Swatch(swatch) => swatch_rgb(track_colours, swatch),
            Self::Custom(rgb) => rgb,
        }
    }

    /// The swatch's number; none for a custom colour.
    #[must_use]
    pub fn swatch(self) -> Option<u8> {
        match self {
            Self::Swatch(swatch) => Some(swatch),
            Self::Custom(_) => None,
        }
    }

    /// A custom colour as `#RRGGBB`.
    #[must_use]
    pub fn hex(rgb: [u8; 3]) -> String {
        format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
    }

    /// Reads `#RRGGBB`, in either case.
    #[must_use]
    pub fn parse_hex(text: &str) -> Option<[u8; 3]> {
        let digits = text.strip_prefix('#')?;
        if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let channel = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
        Some([channel(0)?, channel(2)?, channel(4)?])
    }
}

/// What a colour is written as.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Wire {
    Swatch(u8),
    Hex(String),
}

impl From<Colour> for Wire {
    fn from(colour: Colour) -> Self {
        match colour {
            Colour::Swatch(swatch) => Self::Swatch(swatch),
            Colour::Custom(rgb) => Self::Hex(Colour::hex(rgb)),
        }
    }
}

impl TryFrom<Wire> for Colour {
    type Error = String;

    fn try_from(wire: Wire) -> Result<Self, String> {
        match wire {
            Wire::Swatch(swatch) => Ok(Self::Swatch(swatch)),
            Wire::Hex(text) => Self::parse_hex(&text)
                .map(Self::Custom)
                .ok_or_else(|| format!("`{text}` is neither a swatch number nor a #RRGGBB colour")),
        }
    }
}

impl JsonSchema for Colour {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("Colour")
    }

    fn schema_id() -> Cow<'static, str> {
        Cow::Borrowed("bl_model::colour::Colour")
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A swatch of the palette of 5 rows of 14 (0 to 69, numbered from the top row's left, row by row), or a colour of one's own as `#RRGGBB`.",
            "oneOf": [
                { "type": "integer", "minimum": 0, "maximum": 69 },
                { "type": "string", "pattern": "^#[0-9A-Fa-f]{6}$" }
            ]
        })
    }
}

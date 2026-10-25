//! The colours of tracks, clips and scenes (the themes spec, R14 to R21).

use bl_core::id::Id;

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

/// Checks a swatch number.
pub(crate) fn check_swatch(what: &'static str, swatch: u8) -> Result<(), crate::error::ModelError> {
    crate::command::check_value(what, f64::from(swatch), swatch < SWATCHES)
}

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

/// Checks a swatch number.
pub(crate) fn check_swatch(what: &'static str, swatch: u8) -> Result<(), crate::error::ModelError> {
    crate::command::check_value(what, f64::from(swatch), swatch < SWATCHES)
}

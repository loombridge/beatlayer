//! The Launchpads' 128-colour palette (`docs/specs/supported-controllers.md`,
//! R10): the RGB values measured from the palette image the vendors publish
//! in their programmer's references (the Launchpad X's, Mini MK3's and Pro
//! MK3's are one palette), and the entry nearest to a colour.

/// The palette's RGB values, by palette index. Index 0 is off, shown here as
/// the grey the vendors' image draws it in.
pub const PALETTE: [[u8; 3]; 128] = [
    [97, 97, 97], [179, 179, 179], [221, 221, 221], [255, 255, 255],
    [255, 179, 179], [255, 97, 97], [221, 97, 97], [179, 97, 97],
    [255, 243, 213], [255, 179, 97], [221, 140, 97], [179, 118, 97],
    [255, 238, 161], [255, 255, 97], [221, 221, 97], [179, 179, 97],
    [221, 255, 161], [194, 255, 97], [161, 221, 97], [129, 179, 97],
    [194, 255, 179], [97, 255, 97], [97, 221, 97], [97, 179, 97],
    [194, 255, 194], [97, 255, 140], [97, 221, 118], [97, 179, 107],
    [194, 255, 204], [97, 255, 204], [97, 221, 161], [97, 179, 129],
    [194, 255, 243], [97, 255, 233], [97, 221, 194], [97, 179, 150],
    [194, 243, 255], [97, 238, 255], [97, 199, 221], [97, 161, 179],
    [194, 221, 255], [97, 199, 255], [97, 161, 221], [97, 129, 179],
    [161, 140, 255], [97, 97, 255], [97, 97, 221], [97, 97, 179],
    [204, 179, 255], [161, 97, 255], [129, 97, 221], [118, 97, 179],
    [255, 179, 255], [255, 97, 255], [221, 97, 221], [179, 97, 179],
    [255, 179, 213], [255, 97, 194], [221, 97, 161], [179, 97, 140],
    [255, 118, 97], [233, 179, 97], [221, 194, 97], [161, 161, 97],
    [97, 179, 97], [97, 179, 140], [97, 140, 213], [97, 97, 255],
    [97, 179, 179], [140, 97, 243], [204, 179, 194], [140, 118, 129],
    [255, 97, 97], [243, 255, 161], [238, 252, 97], [204, 255, 97],
    [118, 221, 97], [97, 255, 204], [97, 233, 255], [97, 161, 255],
    [140, 97, 255], [204, 97, 252], [238, 140, 221], [161, 118, 97],
    [255, 161, 97], [221, 249, 97], [213, 255, 140], [97, 255, 97],
    [179, 255, 161], [204, 252, 213], [179, 255, 246], [204, 228, 255],
    [161, 194, 246], [213, 194, 249], [249, 140, 255], [255, 97, 204],
    [255, 194, 97], [243, 238, 97], [228, 255, 97], [221, 204, 97],
    [179, 161, 97], [97, 186, 118], [118, 194, 140], [129, 129, 161],
    [129, 140, 204], [204, 170, 129], [221, 97, 97], [249, 179, 161],
    [249, 186, 118], [255, 243, 140], [233, 249, 161], [213, 238, 118],
    [129, 129, 161], [249, 249, 213], [221, 252, 228], [233, 233, 255],
    [228, 213, 255], [179, 179, 179], [213, 213, 213], [249, 255, 255],
    [233, 97, 97], [170, 97, 97], [129, 246, 97], [97, 179, 97],
    [243, 238, 97], [179, 161, 97], [238, 194, 97], [194, 118, 97],
];

/// Static, bright and dim colours the surfaces use by name, as palette
/// indices.
pub mod colour {
    /// White, and its dim form.
    pub const WHITE: u8 = 3;
    /// A dim white.
    pub const DIM_WHITE: u8 = 1;
    /// Red, and its dim form.
    pub const RED: u8 = 5;
    /// A dim red.
    pub const DIM_RED: u8 = 7;
    /// Yellow, and its dim form.
    pub const YELLOW: u8 = 13;
    /// A dim yellow.
    pub const DIM_YELLOW: u8 = 15;
    /// Green, and its dim form.
    pub const GREEN: u8 = 21;
    /// A dim green.
    pub const DIM_GREEN: u8 = 23;
    /// Blue, and its dim form.
    pub const BLUE: u8 = 45;
    /// A dim blue.
    pub const DIM_BLUE: u8 = 47;
    /// Purple.
    pub const PURPLE: u8 = 49;
    /// Gold, which Shift lights the buttons it changes in.
    pub const GOLD: u8 = 61;
    /// Light brown, the ring's block in Session Overview.
    pub const BROWN: u8 = 105;
    /// The hues the device faders and the sends beyond the second take, by
    /// the bank's or the send's place.
    pub const RAINBOW: [u8; 8] = [5, 9, 13, 21, 29, 37, 45, 53];
}

/// The palette entry nearest to `rgb` by distance in RGB, off excluded; the
/// lowest index of those equally near (R10).
#[must_use]
pub fn nearest(rgb: [u8; 3]) -> u8 {
    let distance = |entry: &[u8; 3]| -> i32 {
        entry
            .iter()
            .zip(rgb)
            .map(|(a, b)| (i32::from(*a) - i32::from(b)).pow(2))
            .sum()
    };
    let mut best = 1;
    for (index, entry) in PALETTE.iter().enumerate().skip(1) {
        if distance(entry) < distance(&PALETTE[best]) {
            best = index;
        }
    }
    best as u8
}

/// The entry nearest to a colour at half its brightness: the dim form of a
/// colour the palette has no named dim entry for (R13).
#[must_use]
pub fn dimmed(rgb: [u8; 3]) -> u8 {
    nearest(rgb.map(|c| c / 2))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: supported-controllers R10. A colour takes the palette entry
    // nearest to it, and a palette entry's own colour takes itself.
    #[test]
    fn a_colour_takes_the_nearest_entry_and_an_entry_takes_itself() {
        assert_eq!(nearest([255, 0, 0]), colour::RED);
        assert_eq!(nearest([97, 255, 97]), colour::GREEN);
        for (index, entry) in PALETTE.iter().enumerate().skip(1) {
            let found = nearest(*entry);
            assert_eq!(PALETTE[usize::from(found)], *entry, "entry {index}");
            assert!(found as usize <= index, "the lowest of equals: {index}");
        }
    }
}

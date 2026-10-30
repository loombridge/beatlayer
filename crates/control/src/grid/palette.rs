//! The grid controllers' 128-colour palettes (`docs/specs/supported-controllers.md`,
//! R10): the Launchpads', whose RGB values were measured from the palette
//! image the vendors publish in their programmer's references (the Launchpad
//! X's, Mini MK3's and Pro MK3's are one palette), and the APCs', whose
//! values the protocol documents give (the APC mini mk2's and the APC40
//! mkII's are one palette); and the entry nearest to a colour in each.

/// The palette's RGB values, by palette index. Index 0 is off, shown here as
/// the grey the vendors' image draws it in.
pub const PALETTE: [[u8; 3]; 128] = [
    [97, 97, 97],
    [179, 179, 179],
    [221, 221, 221],
    [255, 255, 255],
    [255, 179, 179],
    [255, 97, 97],
    [221, 97, 97],
    [179, 97, 97],
    [255, 243, 213],
    [255, 179, 97],
    [221, 140, 97],
    [179, 118, 97],
    [255, 238, 161],
    [255, 255, 97],
    [221, 221, 97],
    [179, 179, 97],
    [221, 255, 161],
    [194, 255, 97],
    [161, 221, 97],
    [129, 179, 97],
    [194, 255, 179],
    [97, 255, 97],
    [97, 221, 97],
    [97, 179, 97],
    [194, 255, 194],
    [97, 255, 140],
    [97, 221, 118],
    [97, 179, 107],
    [194, 255, 204],
    [97, 255, 204],
    [97, 221, 161],
    [97, 179, 129],
    [194, 255, 243],
    [97, 255, 233],
    [97, 221, 194],
    [97, 179, 150],
    [194, 243, 255],
    [97, 238, 255],
    [97, 199, 221],
    [97, 161, 179],
    [194, 221, 255],
    [97, 199, 255],
    [97, 161, 221],
    [97, 129, 179],
    [161, 140, 255],
    [97, 97, 255],
    [97, 97, 221],
    [97, 97, 179],
    [204, 179, 255],
    [161, 97, 255],
    [129, 97, 221],
    [118, 97, 179],
    [255, 179, 255],
    [255, 97, 255],
    [221, 97, 221],
    [179, 97, 179],
    [255, 179, 213],
    [255, 97, 194],
    [221, 97, 161],
    [179, 97, 140],
    [255, 118, 97],
    [233, 179, 97],
    [221, 194, 97],
    [161, 161, 97],
    [97, 179, 97],
    [97, 179, 140],
    [97, 140, 213],
    [97, 97, 255],
    [97, 179, 179],
    [140, 97, 243],
    [204, 179, 194],
    [140, 118, 129],
    [255, 97, 97],
    [243, 255, 161],
    [238, 252, 97],
    [204, 255, 97],
    [118, 221, 97],
    [97, 255, 204],
    [97, 233, 255],
    [97, 161, 255],
    [140, 97, 255],
    [204, 97, 252],
    [238, 140, 221],
    [161, 118, 97],
    [255, 161, 97],
    [221, 249, 97],
    [213, 255, 140],
    [97, 255, 97],
    [179, 255, 161],
    [204, 252, 213],
    [179, 255, 246],
    [204, 228, 255],
    [161, 194, 246],
    [213, 194, 249],
    [249, 140, 255],
    [255, 97, 204],
    [255, 194, 97],
    [243, 238, 97],
    [228, 255, 97],
    [221, 204, 97],
    [179, 161, 97],
    [97, 186, 118],
    [118, 194, 140],
    [129, 129, 161],
    [129, 140, 204],
    [204, 170, 129],
    [221, 97, 97],
    [249, 179, 161],
    [249, 186, 118],
    [255, 243, 140],
    [233, 249, 161],
    [213, 238, 118],
    [129, 129, 161],
    [249, 249, 213],
    [221, 252, 228],
    [233, 233, 255],
    [228, 213, 255],
    [179, 179, 179],
    [213, 213, 213],
    [249, 255, 255],
    [233, 97, 97],
    [170, 97, 97],
    [129, 246, 97],
    [97, 179, 97],
    [243, 238, 97],
    [179, 161, 97],
    [238, 194, 97],
    [194, 118, 97],
];

/// The APC mini mk2's and the APC40 mkII's palette, by velocity: the RGB
/// values their protocol documents publish, which are the same for both. Entry
/// 0 is off.
pub const APC: [[u8; 3]; 128] = [
    [0, 0, 0],
    [30, 30, 30],
    [127, 127, 127],
    [255, 255, 255],
    [255, 76, 76],
    [255, 0, 0],
    [89, 0, 0],
    [25, 0, 0],
    [255, 189, 108],
    [255, 84, 0],
    [89, 29, 0],
    [39, 27, 0],
    [255, 255, 76],
    [255, 255, 0],
    [89, 89, 0],
    [25, 25, 0],
    [136, 255, 76],
    [84, 255, 0],
    [29, 89, 0],
    [20, 43, 0],
    [76, 255, 76],
    [0, 255, 0],
    [0, 89, 0],
    [0, 25, 0],
    [76, 255, 94],
    [0, 255, 25],
    [0, 89, 13],
    [0, 25, 2],
    [76, 255, 136],
    [0, 255, 85],
    [0, 89, 29],
    [0, 31, 18],
    [76, 255, 183],
    [0, 255, 153],
    [0, 89, 53],
    [0, 25, 18],
    [76, 195, 255],
    [0, 169, 255],
    [0, 65, 82],
    [0, 16, 25],
    [76, 136, 255],
    [0, 85, 255],
    [0, 29, 89],
    [0, 8, 25],
    [76, 76, 255],
    [0, 0, 255],
    [0, 0, 89],
    [0, 0, 25],
    [135, 76, 255],
    [84, 0, 255],
    [25, 0, 100],
    [15, 0, 48],
    [255, 76, 255],
    [255, 0, 255],
    [89, 0, 89],
    [25, 0, 25],
    [255, 76, 135],
    [255, 0, 84],
    [89, 0, 29],
    [34, 0, 19],
    [255, 21, 0],
    [153, 53, 0],
    [121, 81, 0],
    [67, 100, 0],
    [3, 57, 0],
    [0, 87, 53],
    [0, 84, 127],
    [0, 0, 255],
    [0, 69, 79],
    [37, 0, 204],
    [127, 127, 127],
    [32, 32, 32],
    [255, 0, 0],
    [189, 255, 45],
    [175, 237, 6],
    [100, 255, 9],
    [16, 139, 0],
    [0, 255, 135],
    [0, 169, 255],
    [0, 42, 255],
    [63, 0, 255],
    [122, 0, 255],
    [178, 26, 125],
    [64, 33, 0],
    [255, 74, 0],
    [136, 225, 6],
    [114, 255, 21],
    [0, 255, 0],
    [59, 255, 38],
    [89, 255, 113],
    [56, 255, 204],
    [91, 138, 255],
    [49, 81, 198],
    [135, 127, 233],
    [211, 29, 255],
    [255, 0, 93],
    [255, 127, 0],
    [185, 176, 0],
    [144, 255, 0],
    [131, 93, 7],
    [57, 43, 0],
    [20, 76, 16],
    [13, 80, 56],
    [21, 21, 42],
    [22, 32, 90],
    [105, 60, 28],
    [168, 0, 10],
    [222, 81, 61],
    [216, 106, 28],
    [255, 225, 38],
    [158, 225, 47],
    [103, 181, 15],
    [30, 30, 48],
    [220, 255, 107],
    [128, 255, 189],
    [154, 153, 255],
    [142, 102, 255],
    [64, 64, 64],
    [117, 117, 117],
    [224, 255, 255],
    [160, 0, 0],
    [53, 0, 0],
    [26, 208, 0],
    [7, 66, 0],
    [185, 176, 0],
    [63, 49, 0],
    [179, 95, 0],
    [75, 21, 2],
];

/// Entries of the APC palette the surfaces name.
pub mod apc {
    /// Red.
    pub const RED: u8 = 5;
    /// Yellow.
    pub const YELLOW: u8 = 13;
    /// Green.
    pub const GREEN: u8 = 21;
    /// Blue.
    pub const BLUE: u8 = 45;
    /// A dim white.
    pub const DIM_WHITE: u8 = 2;
    /// A dim red.
    pub const DIM_RED: u8 = 6;
    /// The vendor's own amber, red and green, which Session Overview takes.
    pub const AMBER: u8 = 126;
    /// The vendor's red.
    pub const OVERVIEW_RED: u8 = 120;
    /// The vendor's green.
    pub const OVERVIEW_GREEN: u8 = 122;
}

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

/// The entry of `table` nearest to `rgb` by distance in RGB, off excluded;
/// the lowest index of those equally near (R10).
fn nearest_in(table: &[[u8; 3]; 128], rgb: [u8; 3]) -> u8 {
    let distance = |entry: &[u8; 3]| -> i32 {
        entry
            .iter()
            .zip(rgb)
            .map(|(a, b)| (i32::from(*a) - i32::from(b)).pow(2))
            .sum()
    };
    let mut best = 1;
    for (index, entry) in table.iter().enumerate().skip(1) {
        if distance(entry) < distance(&table[best]) {
            best = index;
        }
    }
    best as u8
}

/// The Launchpad palette entry nearest to `rgb` (R10).
#[must_use]
pub fn nearest(rgb: [u8; 3]) -> u8 {
    nearest_in(&PALETTE, rgb)
}

/// The APC palette entry nearest to `rgb` (R10).
#[must_use]
pub fn apc_nearest(rgb: [u8; 3]) -> u8 {
    nearest_in(&APC, rgb)
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
    // Spec: supported-controllers R10. The APC palette's entries are the
    // protocol documents' own, and a colour takes the nearest.
    #[test]
    fn an_apc_colour_takes_the_nearest_entry_and_an_entry_takes_itself() {
        assert_eq!(APC[5], [255, 0, 0], "5 is red");
        assert_eq!(APC[21], [0, 255, 0], "21 is green");
        assert_eq!(APC[126], [0xB3, 0x5F, 0x00], "the vendor's amber");
        assert_eq!(apc_nearest([255, 10, 10]), apc::RED);
        assert_eq!(apc_nearest([0, 250, 0]), apc::GREEN);
        for (index, entry) in APC.iter().enumerate().skip(1) {
            let found = apc_nearest(*entry);
            assert_eq!(APC[usize::from(found)], *entry, "entry {index}");
            assert!(found as usize <= index, "the lowest of equals: {index}");
        }
    }

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

//! Colours (the DAWproject spec, R23): a `color` becomes the nearest swatch
//! of the palette as the theme in use draws it, by the distance between the
//! two in the CIELAB colour space.

use bl_model::{SWATCHES, swatch_rgb};

use super::*;

/// A colour given as `#rrggbb`.
fn parse(text: &str) -> Option<[u8; 3]> {
    let hex = text.trim().strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some([(value >> 16) as u8, (value >> 8) as u8, value as u8])
}

/// A colour in CIELAB, from sRGB under the D65 white point.
fn lab([r, g, b]: [u8; 3]) -> [f64; 3] {
    let linear = |channel: u8| {
        let c = f64::from(channel) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (linear(r), linear(g), linear(b));
    let x = (0.4124_564 * r + 0.3575_761 * g + 0.1804_375 * b) / 0.950_47;
    let y = 0.2126_729 * r + 0.7151_522 * g + 0.0721_750 * b;
    let z = (0.0193_339 * r + 0.1191_920 * g + 0.9503_041 * b) / 1.088_83;
    let f = |t: f64| {
        if t > 216.0 / 24_389.0 {
            t.cbrt()
        } else {
            (24_389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// The swatch of the palette drawn from `track_colours` nearest `rgb`; of
/// two as near, the lower.
#[must_use]
pub fn nearest_swatch(track_colours: &[[u8; 3]], rgb: [u8; 3]) -> u8 {
    let wanted = lab(rgb);
    let distance = |swatch: u8| {
        let have = lab(swatch_rgb(track_colours, swatch));
        (0..3).map(|i| (have[i] - wanted[i]).powi(2)).sum::<f64>()
    };
    (0..SWATCHES)
        .min_by(|a, b| distance(*a).total_cmp(&distance(*b)))
        .unwrap_or(0)
}

impl Importer<'_, '_> {
    /// The swatch a `color` names, or `None` with a report for one that
    /// isn't `#rrggbb` (R23).
    pub(super) fn swatch_of(&mut self, color: &str, place: &str) -> Option<u8> {
        let Some(rgb) = parse(color) else {
            self.report
                .add("a colour that isn't #rrggbb", place, "left out");
            return None;
        };
        Some(nearest_swatch(self.context.track_colours, rgb))
    }

    /// A new track's colour: the file's, or the one a new track takes (R23).
    pub(super) fn colour_track(&mut self, id: TrackId, color: Option<&str>, place: &str) {
        let swatch = match color {
            Some(color) => self.swatch_of(color, place),
            None => Some(self.context.new_colours.track(id)),
        };
        let held = self.editor.document().track(id).map(|track| track.colour());
        if let Some(swatch) = swatch
            && held != Some(swatch)
        {
            self.apply(bl_model::Command::SetTrackColour {
                tracks: vec![id],
                colour: swatch,
            });
        }
    }

    /// A scene's colour: the file's, or none (R23).
    pub(super) fn colour_scene(
        &mut self,
        scene: bl_model::SceneId,
        color: Option<&str>,
        place: &str,
    ) {
        if let Some(swatch) = color.and_then(|color| self.swatch_of(color, place)) {
            self.apply(bl_model::Command::SetSceneColour {
                scenes: vec![scene],
                colour: Some(swatch),
            });
        }
    }

    /// A clip's colour: the file's, else the one a new clip takes, which
    /// may be none, the track's (R23).
    pub(super) fn colour_clip(&mut self, clip: &mut Clip, color: Option<&str>, place: &str) {
        clip.colour = match color {
            Some(color) => self.swatch_of(color, place),
            None => self.context.new_colours.clip(clip.id),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: dawproject R23. A swatch's own colour is nearest itself, and a
    // colour between two takes the nearer.
    #[test]
    fn the_nearest_swatch_is_found_by_lab_distance() {
        let palette = bl_model::DEFAULT_TRACK_PALETTE;
        for swatch in 0..SWATCHES {
            let rgb = swatch_rgb(&palette, swatch);
            let found = nearest_swatch(&palette, rgb);
            assert_eq!(swatch_rgb(&palette, found), rgb);
        }
        assert_eq!(nearest_swatch(&palette, [0xe1, 0x66, 0x5a]), 0);
        assert_eq!(parse("#ff8800"), Some([0xff, 0x88, 0x00]));
        assert_eq!(parse("ff8800"), None);
        assert_eq!(parse("#ff88"), None);
    }
}

//! The colour pipeline: a theme file's tokens, toned, brightened, recoloured
//! and made high-contrast by the Theme & Colors page's settings (spec:
//! themes R4 to R8).
//!
//! The steps run in R8's order and each one reaches only the tokens of its
//! kind (R6), so Brightness never touches a chromatic colour and Color
//! Intensity and Color Hue never touch a background; the track palette is
//! of neither kind and no step touches it.

use egui::Color32;

use bl_shell::settings::{ThemeLook, Tone};

use crate::theme::{Colour, Colours, Theme};

/// What a colour token is for (themes R6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A neutral colour that text and lines are drawn on: toned, and moved
    /// by Brightness.
    Background,
    /// Any other neutral colour, such as a separator, a grid line or text:
    /// toned.
    Neutral,
    /// A chromatic colour: moved by Color Intensity and Color Hue.
    Chromatic,
    /// Drawn over the track palette, which no setting changes.
    Fixed,
}

impl Colours {
    /// Every colour token with its name and kind, to change in place. R6
    /// names the kinds; tokens it doesn't name are placed with the colours
    /// they're drawn like: the greys (`pending`, `inactive`, `clip_muted`,
    /// `ruler_tick`, `display_line`, `overview_outline`) are neutral, the
    /// lit number box's `value_fill` is chromatic, and the clip text and
    /// waveform, drawn over the track palette, are fixed.
    pub fn tokens_mut(&mut self) -> Vec<(&'static str, Kind, &mut Colour)> {
        use Kind::{Background, Chromatic, Fixed, Neutral};
        // Naming every field, so a token added to the theme files has to be
        // given its kind here (R6).
        let Self {
            window,
            panel,
            bar,
            control,
            control_hover,
            control_pressed,
            lane,
            lane_selected,
            ruler,
            display,
            separator,
            lane_line,
            grid_bar,
            grid_beat,
            text,
            text_dim,
            pending,
            inactive,
            clip_muted,
            ruler_tick,
            display_line,
            overview_outline,
            accent,
            accent_text,
            record,
            arm_selection,
            error_text,
            focus,
            meter,
            meter_hot,
            meter_expression,
            playhead,
            insert_marker,
            loop_on,
            loop_off,
            song_start,
            time_selection,
            automation,
            modulation,
            mapping,
            midi_map,
            key_map,
            search,
            scale,
            value_fill,
            clip_text,
            waveform,
            out_of_scale,
            pitch_hues: _,
            track_palette: _,
        } = self;
        vec![
            ("window", Background, window),
            ("panel", Background, panel),
            ("bar", Background, bar),
            ("control", Background, control),
            ("control_hover", Background, control_hover),
            ("control_pressed", Background, control_pressed),
            ("lane", Background, lane),
            ("lane_selected", Background, lane_selected),
            ("ruler", Background, ruler),
            ("display", Background, display),
            ("separator", Neutral, separator),
            ("lane_line", Neutral, lane_line),
            ("grid_bar", Neutral, grid_bar),
            ("grid_beat", Neutral, grid_beat),
            ("text", Neutral, text),
            ("text_dim", Neutral, text_dim),
            ("pending", Neutral, pending),
            ("inactive", Neutral, inactive),
            ("clip_muted", Neutral, clip_muted),
            ("ruler_tick", Neutral, ruler_tick),
            ("display_line", Neutral, display_line),
            ("overview_outline", Neutral, overview_outline),
            ("accent", Chromatic, accent),
            ("accent_text", Chromatic, accent_text),
            ("record", Chromatic, record),
            ("arm_selection", Chromatic, arm_selection),
            ("error_text", Chromatic, error_text),
            ("focus", Chromatic, focus),
            ("meter", Chromatic, meter),
            ("meter_hot", Chromatic, meter_hot),
            ("meter_expression", Chromatic, meter_expression),
            ("playhead", Chromatic, playhead),
            ("insert_marker", Chromatic, insert_marker),
            ("loop_on", Chromatic, loop_on),
            ("loop_off", Chromatic, loop_off),
            ("song_start", Chromatic, song_start),
            ("time_selection", Chromatic, time_selection),
            ("automation", Chromatic, automation),
            ("modulation", Chromatic, modulation),
            ("mapping", Chromatic, mapping),
            ("midi_map", Chromatic, midi_map),
            ("key_map", Chromatic, key_map),
            ("search", Chromatic, search),
            ("scale", Chromatic, scale),
            ("value_fill", Chromatic, value_fill),
            ("clip_text", Fixed, clip_text),
            ("waveform", Fixed, waveform),
            ("out_of_scale", Fixed, out_of_scale),
        ]
    }

    /// A token's colour by name.
    #[must_use]
    pub fn token(&self, name: &str) -> Option<Color32> {
        let mut copy = self.clone();
        copy.tokens_mut()
            .into_iter()
            .find(|(n, ..)| *n == name)
            .map(|(.., c)| c.0)
    }
}

/// How much a pairing's contrast ratio has to be (themes R5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Text: 7:1 with High Contrast, 4.5:1 in the bundled theme without.
    Text,
    /// Dimmed text: 4.5:1 with High Contrast, 3:1 without.
    Dim,
    /// A separator, a control's outline or the focus outline: 3:1 with High
    /// Contrast, no level without.
    Line,
}

impl Level {
    /// The ratio the level asks for, with High Contrast or without.
    #[must_use]
    pub fn ratio(self, high_contrast: bool) -> Option<f64> {
        match (self, high_contrast) {
            (Self::Text, true) => Some(7.0),
            (Self::Text, false) | (Self::Dim, true) => Some(4.5),
            (Self::Dim, false) | (Self::Line, true) => Some(3.0),
            (Self::Line, false) => None,
        }
    }
}

/// The backgrounds text is drawn on.
const TEXT_BACKGROUNDS: &[&str] = &[
    "window",
    "panel",
    "bar",
    "control",
    "control_hover",
    "control_pressed",
    "lane",
    "lane_selected",
    "ruler",
];
/// The backgrounds lines are drawn on.
const LINE_BACKGROUNDS: &[&str] = &["window", "panel", "bar", "control"];

/// Every pairing the theme draws that R5 sets a level for: the foreground
/// token, the tokens it's drawn on, and the level. Text pairings come first:
/// High Contrast settles them before the lines.
pub const PAIRINGS: &[(&str, &[&str], Level)] = &[
    ("text", TEXT_BACKGROUNDS, Level::Text),
    ("text_dim", TEXT_BACKGROUNDS, Level::Dim),
    ("error_text", &["window", "panel", "bar"], Level::Text),
    ("accent_text", &["accent"], Level::Text),
    ("separator", LINE_BACKGROUNDS, Level::Line),
    ("focus", LINE_BACKGROUNDS, Level::Line),
    ("lane_line", &["lane", "lane_selected"], Level::Line),
    ("ruler_tick", &["ruler"], Level::Line),
];

/// The contrast ratio of two colours, as WCAG 2.2 defines it.
#[must_use]
pub fn contrast(a: Color32, b: Color32) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn linear(channel: u8) -> f64 {
    let c = f64::from(channel) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn luminance(colour: Color32) -> f64 {
    0.2126 * linear(colour.r()) + 0.7152 * linear(colour.g()) + 0.0722 * linear(colour.b())
}

/// A colour in CIE LCh(ab) (D65): lightness 0 to 100, chroma, hue in
/// degrees.
#[derive(Clone, Copy, Debug)]
pub struct Lch {
    /// Lightness, CIELAB L*.
    pub l: f64,
    /// Chroma.
    pub c: f64,
    /// Hue, in degrees.
    pub h: f64,
}

const WHITE: [f64; 3] = [0.950_47, 1.0, 1.088_83];
const DELTA: f64 = 6.0 / 29.0;

fn lab_f(t: f64) -> f64 {
    if t > DELTA.powi(3) {
        t.cbrt()
    } else {
        t / (3.0 * DELTA * DELTA) + 4.0 / 29.0
    }
}

fn lab_f_inverse(t: f64) -> f64 {
    if t > DELTA {
        t.powi(3)
    } else {
        3.0 * DELTA * DELTA * (t - 4.0 / 29.0)
    }
}

impl Lch {
    /// A colour's LCh.
    #[must_use]
    pub fn of(colour: Color32) -> Self {
        let (r, g, b) = (linear(colour.r()), linear(colour.g()), linear(colour.b()));
        let x = 0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b;
        let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b;
        let z = 0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b;
        let (fx, fy, fz) = (
            lab_f(x / WHITE[0]),
            lab_f(y / WHITE[1]),
            lab_f(z / WHITE[2]),
        );
        let (a, b) = (500.0 * (fx - fy), 200.0 * (fy - fz));
        Self {
            l: 116.0 * fy - 16.0,
            c: a.hypot(b),
            h: b.atan2(a).to_degrees().rem_euclid(360.0),
        }
    }

    /// The linear sRGB channels, which may fall outside 0 to 1.
    fn linear_rgb(self) -> [f64; 3] {
        let (sin, cos) = self.h.to_radians().sin_cos();
        let fy = (self.l + 16.0) / 116.0;
        let fx = fy + self.c * cos / 500.0;
        let fz = fy - self.c * sin / 200.0;
        let x = WHITE[0] * lab_f_inverse(fx);
        let y = WHITE[1] * lab_f_inverse(fy);
        let z = WHITE[2] * lab_f_inverse(fz);
        [
            3.240_454_2 * x - 1.537_138_5 * y - 0.498_531_4 * z,
            -0.969_266 * x + 1.876_010_8 * y + 0.041_556 * z,
            0.055_643_4 * x - 0.204_025_9 * y + 1.057_225_2 * z,
        ]
    }

    /// The colour the screen shows: this lightness and hue, with the
    /// chroma reduced until it's within the screen's colours.
    #[must_use]
    pub fn to_rgb(self) -> Color32 {
        let l = self.l.clamp(0.0, 100.0);
        let within = |c: f64| {
            Self { l, c, h: self.h }
                .linear_rgb()
                .iter()
                .all(|v| (-0.000_5..=1.000_5).contains(v))
        };
        let mut chroma = self.c.max(0.0);
        if !within(chroma) {
            let (mut lo, mut hi) = (0.0, chroma);
            for _ in 0..24 {
                let mid = (lo + hi) / 2.0;
                if within(mid) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            chroma = lo;
        }
        let [r, g, b] = Self {
            l,
            c: chroma,
            h: self.h,
        }
        .linear_rgb();
        let encode = |v: f64| {
            let v = v.clamp(0.0, 1.0);
            let c = if v <= 0.003_130_8 {
                12.92 * v
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            };
            (c * 255.0).round() as u8
        };
        Color32::from_rgb(encode(r), encode(g), encode(b))
    }
}

impl Theme {
    /// This theme's colours with the Theme & Colors settings applied, in
    /// R8's order: Tone, then Brightness, Color Intensity and Color Hue,
    /// then Grid Line Intensity, and High Contrast last.
    #[must_use]
    pub fn styled(&self, look: &ThemeLook) -> Self {
        let mut theme = self.clone();
        let mut tokens = theme.colors.tokens_mut();
        for (_, kind, colour) in &mut tokens {
            let mut lch = Lch::of(colour.0);
            match kind {
                Kind::Background | Kind::Neutral => match look.tone {
                    Tone::Neutral => {}
                    Tone::Warm => (lch.c, lch.h) = (8.0, 60.0),
                    Tone::Cool => (lch.c, lch.h) = (8.0, 250.0),
                },
                Kind::Chromatic | Kind::Fixed => {}
            }
            match kind {
                Kind::Background => lch.l = (lch.l + f64::from(look.brightness)).clamp(0.0, 100.0),
                Kind::Chromatic => {
                    lch.c *= f64::from(look.color_intensity) / 100.0;
                    lch.h = (lch.h + f64::from(look.color_hue)).rem_euclid(360.0);
                }
                Kind::Neutral | Kind::Fixed => {}
            }
            // A token no step changes keeps its exact file value.
            let unchanged = match kind {
                Kind::Background => look.tone == Tone::Neutral && look.brightness == 0,
                Kind::Neutral => look.tone == Tone::Neutral,
                Kind::Chromatic => look.color_intensity == 100 && look.color_hue == 0,
                Kind::Fixed => true,
            };
            if !unchanged {
                colour.0 = lch.to_rgb();
            }
        }
        // Grid Line Intensity: the lane's colour mixed towards the grid's by
        // that much, going on past it at more than 100 %.
        let find = |tokens: &[(&str, Kind, &mut Colour)], name: &str| {
            tokens.iter().find(|(n, ..)| *n == name).map(|(.., c)| c.0)
        };
        if let Some(lane) = find(&tokens, "lane") {
            let amount = f32::from(look.grid_line_intensity) / 100.0;
            for (name, _, colour) in &mut tokens {
                if *name == "grid_bar" || *name == "grid_beat" {
                    let mix = |from: u8, to: u8| {
                        let (from, to) = (f32::from(from), f32::from(to));
                        (from + (to - from) * amount).round().clamp(0.0, 255.0) as u8
                    };
                    let grid = colour.0;
                    colour.0 = Color32::from_rgb(
                        mix(lane.r(), grid.r()),
                        mix(lane.g(), grid.g()),
                        mix(lane.b(), grid.b()),
                    );
                }
            }
        }
        if look.high_contrast {
            high_contrast(&mut tokens);
        }
        drop(tokens);
        theme
    }
}

/// High Contrast (themes R5): for each pairing in [`PAIRINGS`], the
/// backgrounds are brought within the lightness a text or line colour can
/// reach the level against (a no-op unless Brightness or the theme puts
/// them mid-grey), then the foreground moves in lightness, towards white on
/// a dark background and black on a light one, until the level holds.
fn high_contrast(tokens: &mut [(&str, Kind, &mut Colour)]) {
    for &(foreground, backgrounds, level) in PAIRINGS {
        let ratio = level
            .ratio(true)
            .expect("every level has a ratio with High Contrast");
        let get = |tokens: &[(&str, Kind, &mut Colour)], name: &str| {
            tokens
                .iter()
                .find(|(n, ..)| *n == name)
                .map(|(.., c)| c.0)
                .expect("a pairing names tokens")
        };
        let mean = backgrounds
            .iter()
            .map(|b| luminance(get(tokens, b)))
            .sum::<f64>()
            / (backgrounds.len() as f64);
        let towards_white = mean < 0.179;
        // The luminance a background has to stay within for white or black
        // to reach the ratio, with a margin for rounding to 8 bits.
        let margin = ratio * 1.03;
        for background in backgrounds {
            let index = tokens
                .iter()
                .position(|(n, ..)| n == background)
                .expect("a pairing names tokens");
            let colour = &mut tokens[index].2.0;
            let mut lch = Lch::of(*colour);
            let step = if towards_white { -0.25 } else { 0.25 };
            let outside = |c: Color32| {
                if towards_white {
                    luminance(c) > 1.05 / margin - 0.05
                } else {
                    luminance(c) < 0.05 * margin - 0.05
                }
            };
            while outside(*colour) && (0.0..=100.0).contains(&(lch.l + step)) {
                lch.l += step;
                *colour = lch.to_rgb();
            }
        }
        let bgs: Vec<Color32> = backgrounds.iter().map(|b| get(tokens, b)).collect();
        let index = tokens
            .iter()
            .position(|(n, ..)| *n == foreground)
            .expect("a pairing names tokens");
        let colour = &mut tokens[index].2.0;
        let mut lch = Lch::of(*colour);
        let step = if towards_white { 0.25 } else { -0.25 };
        while bgs.iter().any(|&b| contrast(*colour, b) < ratio)
            && (0.0..=100.0).contains(&(lch.l + step))
        {
            lch.l += step;
            *colour = lch.to_rgb();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn looks() -> Vec<ThemeLook> {
        let base = ThemeLook::default();
        let mut looks = Vec::new();
        for tone in Tone::ALL {
            for (brightness, intensity, hue, grid) in [
                (0, 100, 0, 100),
                (-50, 100, 0, 100),
                (50, 100, 0, 100),
                (0, 0, 180, 0),
                (0, 200, -180, 200),
                (-50, 200, 90, 0),
                (50, 0, -90, 200),
                (-25, 100, 0, 100),
                (25, 100, 0, 100),
            ] {
                looks.push(ThemeLook {
                    tone,
                    brightness,
                    color_intensity: intensity,
                    color_hue: hue,
                    grid_line_intensity: grid,
                    ..base.clone()
                });
            }
        }
        looks
    }

    fn bundled() -> [Theme; 2] {
        [Theme::bundled(true), Theme::bundled(false)]
    }

    fn tokens(theme: &Theme) -> Vec<(&'static str, Kind, Color32)> {
        theme
            .colors
            .clone()
            .tokens_mut()
            .into_iter()
            .map(|(n, k, c)| (n, k, c.0))
            .collect()
    }

    /// Every pairing holds its level, or says which does not.
    fn failing(theme: &Theme, high_contrast: bool) -> Vec<String> {
        let mut failures = Vec::new();
        for &(foreground, backgrounds, level) in PAIRINGS {
            let Some(ratio) = level.ratio(high_contrast) else {
                continue;
            };
            for background in backgrounds {
                let (f, b) = (
                    theme.colors.token(foreground).unwrap(),
                    theme.colors.token(background).unwrap(),
                );
                let got = contrast(f, b);
                if got < ratio {
                    failures.push(format!("{foreground} on {background}: {got:.2} < {ratio}"));
                }
            }
        }
        failures
    }

    // Spec: themes R6, R7, R8.
    #[test]
    fn each_step_leaves_the_tokens_of_the_other_kinds_alone() {
        for theme in bundled() {
            let before = tokens(&theme);
            for look in looks() {
                let after = tokens(&theme.styled(&look));
                assert_eq!(
                    theme.styled(&look).colors.track_palette,
                    theme.colors.track_palette,
                    "no setting touches the track palette"
                );
                for ((name, kind, was), (_, _, is)) in before.iter().zip(&after) {
                    let moved = was != is;
                    let by_tone = look.tone != Tone::Neutral
                        && matches!(kind, Kind::Background | Kind::Neutral);
                    let by_brightness = look.brightness != 0 && *kind == Kind::Background;
                    let by_colour = (look.color_intensity != 100 || look.color_hue != 0)
                        && *kind == Kind::Chromatic;
                    let by_grid = look.grid_line_intensity != 100
                        && matches!(*name, "grid_bar" | "grid_beat");
                    assert!(
                        !moved || by_tone || by_brightness || by_colour || by_grid,
                        "{name} moved with {look:?}"
                    );
                }
            }
        }
    }

    // Spec: themes R7.
    #[test]
    fn brightness_moves_only_backgrounds_and_colour_only_chromatic_colours() {
        let theme = Theme::bundled(true);
        let bright = theme.styled(&ThemeLook {
            brightness: 20,
            ..ThemeLook::default()
        });
        let (was, is) = (
            Lch::of(theme.colors.panel.0),
            Lch::of(bright.colors.panel.0),
        );
        assert!((is.l - was.l - 20.0).abs() < 1.0, "{} to {}", was.l, is.l);
        let tinted = theme.styled(&ThemeLook {
            color_intensity: 0,
            ..ThemeLook::default()
        });
        assert!(Lch::of(tinted.colors.accent.0).c < 1.5, "no chroma left");
        assert_eq!(tinted.colors.panel, theme.colors.panel);
        let turned = theme.styled(&ThemeLook {
            color_hue: 90,
            ..ThemeLook::default()
        });
        let (was, is) = (
            Lch::of(theme.colors.accent.0),
            Lch::of(turned.colors.accent.0),
        );
        assert!(
            ((is.h - was.h - 90.0).rem_euclid(360.0)).min((was.h + 90.0 - is.h).rem_euclid(360.0))
                < 3.0
        );
    }

    // Spec: themes R7.
    #[test]
    fn grid_line_intensity_mixes_the_lane_towards_the_grid_colour() {
        let theme = Theme::bundled(true);
        let at = |percent| {
            theme
                .styled(&ThemeLook {
                    grid_line_intensity: percent,
                    ..ThemeLook::default()
                })
                .colors
        };
        assert_eq!(at(0).grid_bar, theme.colors.lane, "0 % shows no line");
        assert_eq!(at(100).grid_bar, theme.colors.grid_bar);
        let (lane, grid, double) = (
            theme.colors.lane.0,
            theme.colors.grid_bar.0,
            at(200).grid_bar.0,
        );
        assert!(
            i32::from(double.r()) - i32::from(grid.r())
                >= i32::from(grid.r()) - i32::from(lane.r()) - 1
        );
    }

    // Spec: themes R4.
    #[test]
    fn warm_and_cool_land_at_their_hue_and_chroma() {
        for theme in bundled() {
            for (tone, hue) in [(Tone::Warm, 60.0), (Tone::Cool, 250.0)] {
                let toned = theme.styled(&ThemeLook {
                    tone,
                    ..ThemeLook::default()
                });
                for name in ["panel", "bar", "control", "lane", "separator", "text_dim"] {
                    let was = Lch::of(theme.colors.token(name).unwrap());
                    let is = Lch::of(toned.colors.token(name).unwrap());
                    assert!((is.l - was.l).abs() < 1.5, "{name} keeps its lightness");
                    assert!((is.c - 8.0).abs() < 1.5, "{name}: chroma {}", is.c);
                    assert!((is.h - hue).abs() < 8.0, "{name}: hue {}", is.h);
                }
                assert_eq!(
                    toned.colors.accent, theme.colors.accent,
                    "chromatic colours aren't toned"
                );
            }
        }
    }

    // Spec: themes R5.
    #[test]
    fn high_contrast_reaches_its_levels_at_every_extreme() {
        for theme in bundled() {
            for look in looks() {
                let look = ThemeLook {
                    high_contrast: true,
                    ..look
                };
                let failures = failing(&theme.styled(&look), true);
                assert!(failures.is_empty(), "{look:?}: {failures:?}");
            }
        }
    }

    // Spec: themes R5.
    #[test]
    fn the_bundled_theme_meets_its_levels_without_high_contrast() {
        for theme in bundled() {
            for tone in Tone::ALL {
                let styled = theme.styled(&ThemeLook {
                    tone,
                    ..ThemeLook::default()
                });
                let failures = failing(&styled, false);
                assert!(failures.is_empty(), "{} {tone:?}: {failures:?}", theme.dark);
            }
        }
    }

    // Spec: themes R5.
    #[test]
    fn high_contrast_changes_nothing_that_already_holds() {
        // A pairing that already meets its level stays as the file gives it.
        let theme = Theme::bundled(true);
        let styled = theme.styled(&ThemeLook {
            high_contrast: true,
            ..ThemeLook::default()
        });
        assert_eq!(styled.colors.window, theme.colors.window);
        assert_eq!(styled.colors.track_palette, theme.colors.track_palette);
    }
}

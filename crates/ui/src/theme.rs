//! Themes: Beatlayer's colours, sizes, and typeface, as tokens read from
//! theme files (`themes/dark.toml`, `themes/light.toml`).
//!
//! Every colour the UI draws is a token here, so a theme file changes the
//! whole look. The colours and the typeface are Beatlayer's own, never
//! Ableton's (D16, clean-room policy).

use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, Visuals};
use serde::Deserialize;

use bl_shell::settings::ThemeChoice;

/// The bundled UI typeface: Inter Regular 3.019, under OFL-1.1
/// (`fonts/README.md`).
pub const UI_FONT: &[u8] = include_bytes!("../fonts/inter/Inter-Regular.ttf");

const DARK: &str = include_str!("../themes/dark.toml");
const LIGHT: &str = include_str!("../themes/light.toml");

/// A colour token, read from `#RRGGBB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Colour(pub Color32);

impl<'de> Deserialize<'de> for Colour {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        parse_hex(&text)
            .map(Colour)
            .ok_or_else(|| serde::de::Error::custom(format!("`{text}` isn't a #RRGGBB colour")))
    }
}

fn parse_hex(text: &str) -> Option<Color32> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some(Color32::from_rgb(
        (value >> 16) as u8,
        (value >> 8) as u8,
        value as u8,
    ))
}

/// The colour tokens. Each is described in the theme files.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Colours {
    pub window: Colour,
    pub panel: Colour,
    pub bar: Colour,
    pub control: Colour,
    pub control_hover: Colour,
    pub control_pressed: Colour,
    pub accent: Colour,
    pub accent_text: Colour,
    pub record: Colour,
    pub error_text: Colour,
    pub text: Colour,
    pub text_dim: Colour,
    pub pending: Colour,
    pub separator: Colour,
    pub focus: Colour,
    pub meter: Colour,
    pub meter_hot: Colour,
    pub lane: Colour,
    pub lane_selected: Colour,
    pub lane_line: Colour,
    pub grid_bar: Colour,
    pub grid_beat: Colour,
    pub ruler: Colour,
    pub ruler_tick: Colour,
    pub playhead: Colour,
    pub insert_marker: Colour,
    pub loop_on: Colour,
    pub loop_off: Colour,
    /// The locator that holds the song start time (WP-6.24).
    pub song_start: Colour,
    pub time_selection: Colour,
    pub clip_text: Colour,
    pub waveform: Colour,
    pub clip_muted: Colour,
    pub overview_outline: Colour,
    pub display: Colour,
    pub display_line: Colour,
    pub value_fill: Colour,
    pub inactive: Colour,
    /// An automation envelope, and the LED of a parameter that has one
    /// (WP-5.9).
    pub automation: Colour,
    /// A parameter's modulation: its ring segment, its fader mark, its
    /// envelope in a clip, and the LED of a modulated parameter (WP-5.9).
    pub modulation: Colour,
    /// The tint of every parameter a lit Map button can map, and of the lit
    /// button (WP-5.10).
    pub mapping: Colour,
    /// Track and clip colours, assigned to new tracks in turn.
    pub track_palette: Vec<Colour>,
}

/// The size tokens, in points.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Sizes {
    /// Body text.
    pub text: f32,
    /// Small text, such as the Status Bar's.
    pub small_text: f32,
    /// Headings, such as the Settings window's sections.
    pub heading: f32,
    /// Controls' corner radius.
    pub corner: f32,
}

/// A theme: its name, whether it's dark, and its tokens.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    /// The theme's name.
    pub name: String,
    /// Whether it's a dark theme.
    pub dark: bool,
    /// Colours.
    pub colors: Colours,
    /// Sizes.
    pub sizes: Sizes,
}

impl Theme {
    /// Reads a theme file.
    ///
    /// # Errors
    ///
    /// A message if the file isn't a valid theme.
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| format!("not a valid theme: {e}"))
    }

    /// The built-in theme for a choice.
    #[must_use]
    pub fn builtin(choice: ThemeChoice) -> Self {
        let text = match choice {
            ThemeChoice::Dark => DARK,
            ThemeChoice::Light => LIGHT,
        };
        Self::parse(text).expect("the built-in themes are valid")
    }

    /// A token's colour.
    #[must_use]
    pub fn c(colour: Colour) -> Color32 {
        colour.0
    }

    /// The body font.
    #[must_use]
    pub fn font(&self) -> FontId {
        FontId::proportional(self.sizes.text)
    }

    /// The small font.
    #[must_use]
    pub fn small_font(&self) -> FontId {
        FontId::proportional(self.sizes.small_text)
    }

    /// egui's own widgets' look, from the tokens.
    #[must_use]
    pub fn visuals(&self) -> Visuals {
        let c = &self.colors;
        let mut visuals = if self.dark {
            Visuals::dark()
        } else {
            Visuals::light()
        };
        visuals.panel_fill = c.panel.0;
        visuals.window_fill = c.panel.0;
        visuals.extreme_bg_color = c.window.0;
        visuals.faint_bg_color = c.bar.0;
        visuals.override_text_color = Some(c.text.0);
        visuals.selection.bg_fill = c.accent.0;
        visuals.selection.stroke.color = c.accent_text.0;
        visuals.hyperlink_color = c.focus.0;
        visuals.error_fg_color = c.error_text.0;
        let widgets = &mut visuals.widgets;
        widgets.noninteractive.bg_fill = c.panel.0;
        widgets.noninteractive.weak_bg_fill = c.panel.0;
        widgets.noninteractive.fg_stroke.color = c.text.0;
        widgets.noninteractive.bg_stroke.color = c.separator.0;
        widgets.inactive.bg_fill = c.control.0;
        widgets.inactive.weak_bg_fill = c.control.0;
        widgets.inactive.fg_stroke.color = c.text.0;
        widgets.hovered.bg_fill = c.control_hover.0;
        widgets.hovered.weak_bg_fill = c.control_hover.0;
        widgets.hovered.fg_stroke.color = c.text.0;
        widgets.active.bg_fill = c.control_pressed.0;
        widgets.active.weak_bg_fill = c.control_pressed.0;
        widgets.active.fg_stroke.color = c.text.0;
        widgets.open.bg_fill = c.control_pressed.0;
        widgets.open.weak_bg_fill = c.control_pressed.0;
        visuals
    }

    /// Applies the theme and the bundled typeface to an egui context.
    pub fn apply(&self, ctx: &egui::Context) {
        ctx.set_visuals(self.visuals());
        ctx.all_styles_mut(|style| {
            for (text_style, font) in &mut style.text_styles {
                font.size = match text_style {
                    egui::TextStyle::Small => self.sizes.small_text,
                    egui::TextStyle::Heading => self.sizes.heading,
                    _ => self.sizes.text,
                };
            }
        });
    }
}

/// Font definitions with only the bundled typeface, for every family.
#[must_use]
pub fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::empty();
    fonts
        .font_data
        .insert("inter".into(), FontData::from_static(UI_FONT).into());
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("inter".into());
    }
    fonts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_themes_parse_and_differ() {
        let dark = Theme::builtin(ThemeChoice::Dark);
        let light = Theme::builtin(ThemeChoice::Light);
        assert!(dark.dark && !light.dark);
        assert_ne!(dark.colors.panel, light.colors.panel);
    }

    #[test]
    fn a_bad_colour_is_reported() {
        let text = DARK.replace("#22262c", "#22262");
        assert!(Theme::parse(&text).unwrap_err().contains("#22262"));
        let text = DARK.replace("panel = ", "panle = ");
        assert!(Theme::parse(&text).is_err(), "unknown tokens are refused");
    }

    #[test]
    fn the_typeface_is_inter() {
        // The font's name table holds "Inter" in UTF-16.
        let name: Vec<u8> = "Inter".encode_utf16().flat_map(u16::to_be_bytes).collect();
        assert!(UI_FONT.windows(name.len()).any(|w| w == name));
    }
}

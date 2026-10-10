//! Fonts from the operating system.
//!
//! egui's bundled fonts (`epaint_default_fonts`) are licensed OFL-1.1 and
//! Ubuntu-font-1.0, which `deny.toml` doesn't allow, so the spike builds egui
//! without them and loads a system UI font at start-up instead.

use egui::{FontData, FontDefinitions, FontFamily};

/// Candidate font files per platform: (path, face index in a collection).
fn candidates() -> &'static [(&'static str, u32)] {
    if cfg!(target_os = "macos") {
        &[
            ("/System/Library/Fonts/Helvetica.ttc", 0),
            ("/System/Library/Fonts/Supplemental/Arial.ttf", 0),
            ("/System/Library/Fonts/Geneva.ttf", 0),
        ]
    } else if cfg!(target_os = "windows") {
        &[
            ("C:\\Windows\\Fonts\\segoeui.ttf", 0),
            ("C:\\Windows\\Fonts\\arial.ttf", 0),
        ]
    } else {
        &[
            ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 0),
            ("/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf", 0),
            ("/usr/share/fonts/TTF/DejaVuSans.ttf", 0),
            (
                "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
                0,
            ),
            ("/usr/share/fonts/noto/NotoSans-Regular.ttf", 0),
        ]
    }
}

/// Font definitions with the first system font found, used for both the
/// proportional and the monospace family.
pub fn system_fonts() -> Result<(FontDefinitions, String), String> {
    for &(path, index) in candidates() {
        if let Ok(bytes) = std::fs::read(path) {
            let mut data = FontData::from_owned(bytes);
            data.index = index;
            let mut fonts = FontDefinitions::empty();
            fonts.font_data.insert("system".into(), data.into());
            for family in [FontFamily::Proportional, FontFamily::Monospace] {
                fonts
                    .families
                    .entry(family)
                    .or_default()
                    .push("system".into());
            }
            return Ok((fonts, path.to_owned()));
        }
    }
    Err(format!(
        "no system font found; tried {:?}",
        candidates().iter().map(|c| c.0).collect::<Vec<_>>()
    ))
}

//! Draws a [`Screen`] into a 960 × 160 image with a pure-Rust 2D rasteriser
//! and font rasteriser (`docs/specs/push-2.md`, R17 to R20).
//!
//! The look is Beatlayer's own: the app's typeface, a black background, a
//! 24-pixel band above and below the content, eight columns of 120 pixels.

use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, MetadataProvider};
use tiny_skia::{FillRule, LineCap, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, Transform};

use crate::frame::{HEIGHT, RGB_BYTES, WIDTH};
use crate::screen::{Column, Dot, Graphic, Label, List, Screen};

/// The columns' width.
const COLUMN: f32 = 120.0;
/// The bands' height.
const BAND: f32 = 24.0;
/// The app's typeface (OFL-1.1): the UI crate's own file, which this crate
/// reads without depending on it.
const FONT: &[u8] = include_bytes!("../../ui/fonts/inter/Inter-Regular.ttf");

const TEXT: [u8; 3] = [235, 235, 235];
const DIM: [u8; 3] = [140, 140, 140];

/// Draws `screen` and returns the image: RGB, 3 bytes a pixel, `WIDTH` ×
/// `HEIGHT`.
///
/// # Panics
///
/// Never in practice: the pixmap's size is a constant.
#[must_use]
pub fn render(screen: &Screen) -> Vec<u8> {
    let mut pixmap =
        Pixmap::new(WIDTH as u32, HEIGHT as u32).expect("a pixmap of the display's size");
    pixmap.fill(tiny_skia::Color::BLACK);
    let mut painter = Painter {
        pixmap: &mut pixmap,
        font: FontRef::new(FONT).ok(),
    };
    painter.draw(screen);
    let mut rgb = Vec::with_capacity(RGB_BYTES);
    for pixel in pixmap.pixels() {
        rgb.extend_from_slice(&[pixel.red(), pixel.green(), pixel.blue()]);
    }
    rgb
}

/// The image as a PNG, for `push.display_frame` and the golden frames.
///
/// # Errors
///
/// A message if the encoder fails.
pub fn png(rgb: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, WIDTH as u32, HEIGHT as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(rgb))
        .map_err(|e| e.to_string())?;
    Ok(out)
}

struct Painter<'a> {
    pixmap: &'a mut Pixmap,
    font: Option<FontRef<'static>>,
}

fn paint(rgb: [u8; 3], alpha: u8) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(rgb[0], rgb[1], rgb[2], alpha);
    paint.anti_alias = true;
    paint
}

/// A path builder that takes a glyph's outline, y up, and puts it at `x`,
/// `baseline` on the display, y down.
struct Pen {
    path: PathBuilder,
    x: f32,
    baseline: f32,
}

impl Pen {
    fn point(&self, x: f32, y: f32) -> (f32, f32) {
        (self.x + x, self.baseline - y)
    }
}

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.point(x, y);
        self.path.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.point(x, y);
        self.path.line_to(x, y);
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let ((cx, cy), (x, y)) = (self.point(cx, cy), self.point(x, y));
        self.path.quad_to(cx, cy, x, y);
    }
    fn curve_to(&mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) {
        let (c1, c2, to) = (self.point(c1x, c1y), self.point(c2x, c2y), self.point(x, y));
        self.path.cubic_to(c1.0, c1.1, c2.0, c2.1, to.0, to.1);
    }
    fn close(&mut self) {
        self.path.close();
    }
}

impl Painter<'_> {
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, colour: [u8; 3], alpha: u8) {
        if let Some(rect) = Rect::from_xywh(x, y, w, h) {
            self.pixmap
                .fill_rect(rect, &paint(colour, alpha), Transform::identity(), None);
        }
    }

    fn fill(&mut self, path: &Path, colour: [u8; 3]) {
        self.pixmap.fill_path(
            path,
            &paint(colour, 255),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    fn stroke(&mut self, path: &Path, colour: [u8; 3], width: f32) {
        let stroke = Stroke {
            width,
            line_cap: LineCap::Round,
            ..Stroke::default()
        };
        self.pixmap.stroke_path(
            path,
            &paint(colour, 255),
            &stroke,
            Transform::identity(),
            None,
        );
    }

    /// The width of `text` at `size` pixels.
    fn width(&self, text: &str, size: f32) -> f32 {
        let Some(font) = &self.font else {
            return 0.0;
        };
        let metrics = font.glyph_metrics(Size::new(size), LocationRef::default());
        let charmap = font.charmap();
        text.chars()
            .filter_map(|c| metrics.advance_width(charmap.map(c)?))
            .sum()
    }

    /// `text` cut to fit `max` pixels, with an ellipsis where it was cut.
    fn fit(&self, text: &str, size: f32, max: f32) -> String {
        if self.width(text, size) <= max {
            return text.to_owned();
        }
        let mut cut: String = text.to_owned();
        while !cut.is_empty() && self.width(&format!("{cut}…"), size) > max {
            cut.pop();
        }
        format!("{cut}…")
    }

    /// Draws `text` with its baseline at `baseline`, left edge at `x`.
    fn text(&mut self, text: &str, x: f32, baseline: f32, size: f32, colour: [u8; 3]) {
        let Some(font) = self.font.clone() else {
            return;
        };
        let metrics = font.glyph_metrics(Size::new(size), LocationRef::default());
        let charmap = font.charmap();
        let outlines = font.outline_glyphs();
        let mut pen = Pen {
            path: PathBuilder::new(),
            x,
            baseline,
        };
        for c in text.chars() {
            let Some(glyph) = charmap.map(c) else {
                continue;
            };
            if let Some(outline) = outlines.get(glyph) {
                let _ = outline.draw(
                    DrawSettings::unhinted(Size::new(size), LocationRef::default()),
                    &mut pen,
                );
            }
            pen.x += metrics.advance_width(glyph).unwrap_or(size / 2.0);
        }
        if let Some(path) = pen.path.finish() {
            self.fill(&path, colour);
        }
    }

    /// Draws `text` centred on `centre`, cut to `max` pixels wide.
    fn centred(
        &mut self,
        text: &str,
        centre: f32,
        baseline: f32,
        size: f32,
        max: f32,
        colour: [u8; 3],
    ) {
        let text = self.fit(text, size, max);
        let width = self.width(&text, size);
        self.text(&text, centre - width / 2.0, baseline, size, colour);
    }

    fn draw(&mut self, screen: &Screen) {
        let lower = HEIGHT as f32 - BAND;
        for (n, label) in screen.upper.iter().enumerate() {
            self.label(label, n, 0.0);
        }
        for (n, label) in screen.lower.iter().enumerate() {
            self.label(label, n, lower);
        }
        for (n, column) in screen.columns.iter().enumerate() {
            self.column(column, n);
        }
        for (n, list) in screen.lists.iter().enumerate() {
            if let Some(list) = list {
                self.list(list, n);
            }
        }
        for (n, progress) in screen.progress.iter().enumerate() {
            if let Some(progress) = progress {
                let colour = screen.lower[n].colour;
                let x = n as f32 * COLUMN + 8.0;
                self.rect(x, HEIGHT as f32 - 4.0, COLUMN - 16.0, 3.0, colour, 70);
                self.rect(
                    x,
                    HEIGHT as f32 - 4.0,
                    (COLUMN - 16.0) * progress.clamp(0.0, 1.0),
                    3.0,
                    colour,
                    255,
                );
            }
        }
        if let Some(curve) = &screen.curve {
            self.curve(curve);
        }
        if let Some(title) = &screen.title {
            self.centred(title, WIDTH as f32 / 2.0, 90.0, 26.0, 820.0, TEXT);
        }
        if let Some(lit) = screen.count_in {
            let alpha = if lit { 255 } else { 60 };
            self.rect(0.0, 0.0, WIDTH as f32, 5.0, [255, 255, 255], alpha);
        }
        if let Some(message) = &screen.notification {
            self.rect(250.0, 52.0, 460.0, 56.0, [38, 38, 38], 255);
            self.rect(250.0, 52.0, 460.0, 2.0, TEXT, 255);
            self.centred(message, WIDTH as f32 / 2.0, 90.0, 22.0, 430.0, TEXT);
        }
    }

    /// A label in its band: the selected one on a band of its colour.
    fn label(&mut self, label: &Label, column: usize, top: f32) {
        if label.is_empty() {
            return;
        }
        let x = column as f32 * COLUMN;
        let centre = x + COLUMN / 2.0;
        if label.selected {
            self.rect(
                x + 3.0,
                top + 2.0,
                COLUMN - 6.0,
                BAND - 4.0,
                label.colour,
                255,
            );
            self.centred(
                &label.text,
                centre,
                top + 17.0,
                14.0,
                COLUMN - 14.0,
                [0, 0, 0],
            );
        } else {
            self.centred(
                &label.text,
                centre,
                top + 17.0,
                14.0,
                COLUMN - 14.0,
                label.colour,
            );
        }
    }

    /// A column of the content band (R20).
    fn column(&mut self, column: &Column, n: usize) {
        if column.is_empty() {
            return;
        }
        let x = n as f32 * COLUMN;
        let centre = x + COLUMN / 2.0;
        if column.touched {
            self.rect(
                x + 2.0,
                BAND + 2.0,
                COLUMN - 4.0,
                HEIGHT as f32 - 2.0 * BAND - 4.0,
                [50, 50, 50],
                255,
            );
        }
        match column.dot {
            Dot::None => {}
            Dot::Automated => self.dot(x + 10.0, BAND + 14.0, [255, 255, 255]),
            Dot::Overridden => self.dot(x + 10.0, BAND + 14.0, [120, 120, 120]),
        }
        self.centred(&column.name, centre, BAND + 19.0, 15.0, COLUMN - 28.0, TEXT);
        self.centred(&column.value, centre, BAND + 40.0, 13.0, COLUMN - 10.0, DIM);
        match &column.graphic {
            Graphic::None => {}
            Graphic::Arc(value) => self.arc(centre, BAND + 76.0, value.clamp(0.0, 1.0)),
            Graphic::Choice(entry) => {
                self.centred(entry, centre, BAND + 76.0, 16.0, COLUMN - 12.0, TEXT);
            }
        }
    }

    /// A browser column: eight rows, scrolled to keep the selected one in
    /// view, which is on a band of its own (push-2-tracks R6).
    fn list(&mut self, list: &List, n: usize) {
        const ROW: f32 = 14.0;
        const ROWS: usize = 8;
        let x = n as f32 * COLUMN;
        let top = list
            .selected
            .map_or(0, |at| at.saturating_sub(ROWS - 1))
            .min(list.rows.len().saturating_sub(ROWS));
        for (slot, (name, folder)) in list.rows.iter().skip(top).take(ROWS).enumerate() {
            let y = BAND + 2.0 + slot as f32 * ROW;
            let selected = list.selected == Some(top + slot);
            if selected {
                let band = if list.in_use {
                    [255, 255, 255]
                } else {
                    [90, 90, 90]
                };
                self.rect(x + 2.0, y, COLUMN - 4.0, ROW, band, 255);
            }
            let colour = match (selected, list.in_use) {
                (true, true) => [0, 0, 0],
                (true, false) => TEXT,
                _ => DIM,
            };
            let text = if *folder {
                format!("{name} ›")
            } else {
                name.clone()
            };
            let text = self.fit(&text, 11.0, COLUMN - 10.0);
            self.text(&text, x + 6.0, y + 11.0, 11.0, colour);
        }
    }

    fn dot(&mut self, x: f32, y: f32, colour: [u8; 3]) {
        if let Some(path) = PathBuilder::from_circle(x, y, 3.5) {
            self.fill(&path, colour);
        }
    }

    /// An arc of 270 degrees, open at the bottom, with its value in light.
    fn arc(&mut self, cx: f32, cy: f32, value: f32) {
        let (radius, start, sweep) = (22.0f32, 135.0f32.to_radians(), 270.0f32.to_radians());
        let arc = |to: f32| {
            let mut path = PathBuilder::new();
            let steps = 48;
            for step in 0..=steps {
                let angle = start + sweep * to * step as f32 / steps as f32;
                let (x, y) = (cx + radius * angle.cos(), cy + radius * angle.sin());
                if step == 0 {
                    path.move_to(x, y);
                } else {
                    path.line_to(x, y);
                }
            }
            path.finish()
        };
        if let Some(path) = arc(1.0) {
            self.stroke(&path, [70, 70, 70], 4.0);
        }
        if value > 0.0
            && let Some(path) = arc(value)
        {
            self.stroke(&path, [255, 255, 255], 4.0);
        }
    }

    /// The velocity curve over columns 4 and 5: force across, velocity up
    /// (R31).
    fn curve(&mut self, curve: &[u8]) {
        let (left, top, width, height) =
            (3.0 * COLUMN + 10.0, BAND + 10.0, 2.0 * COLUMN - 20.0, 90.0);
        let mut frame = PathBuilder::new();
        frame.move_to(left, top);
        frame.line_to(left, top + height);
        frame.line_to(left + width, top + height);
        if let Some(path) = frame.finish() {
            self.stroke(&path, [90, 90, 90], 1.5);
        }
        let mut line = PathBuilder::new();
        for (n, velocity) in curve.iter().enumerate() {
            let x = left + width * n as f32 / (curve.len().max(2) - 1) as f32;
            let y = top + height - height * f32::from(*velocity) / 127.0;
            if n == 0 {
                line.move_to(x, y);
            } else {
                line.line_to(x, y);
            }
        }
        if let Some(path) = line.finish() {
            self.stroke(&path, [255, 255, 255], 2.0);
        }
    }
}

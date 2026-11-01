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
use crate::screen::{ClipBody, ClipScreen, Column, Dot, Graphic, Icon, Label, List, Screen, Shade};

/// The columns' width.
const COLUMN: f32 = 120.0;
/// The bands' height.
const BAND: f32 = 24.0;
/// Where a clip's graphic sits in the content band: its left edge, and its
/// top and bottom, over the two lines of the columns' names and values.
const CLIP_LEFT: f32 = 14.0;
const CLIP_TOP: f32 = BAND + 22.0;
const CLIP_BOTTOM: f32 = BAND + 72.0;
/// The app's typeface (OFL-1.1): the UI crate's own file, which this crate
/// reads without depending on it.
const FONT: &[u8] = include_bytes!("../../ui/fonts/inter/Inter-Regular.ttf");

const TEXT: [u8; 3] = [235, 235, 235];
const DIM: [u8; 3] = [140, 140, 140];
/// The arm icon's red (Beatlayer's own).
const ARM: [u8; 3] = [255, 64, 64];

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

    /// A rectangle on whole pixels, not smoothed: what a clip's notes, markers
    /// and boxes are drawn with, since the rasteriser's hairline path fails
    /// on thin smoothed ones.
    fn crisp(&mut self, x: f32, y: f32, w: f32, h: f32, colour: [u8; 3], alpha: u8) {
        let (x, y) = (x.round(), y.round());
        let (w, h) = (w.round().max(1.0), h.round().max(1.0));
        if let Some(rect) = Rect::from_xywh(x, y, w, h) {
            let mut paint = paint(colour, alpha);
            paint.anti_alias = false;
            self.pixmap
                .fill_rect(rect, &paint, Transform::identity(), None);
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
        if let Some(clip) = &screen.clip {
            self.clip(clip);
        }
        for (n, column) in screen.columns.iter().enumerate() {
            if screen.clip.is_some() {
                self.compact(column, n);
            } else {
                self.column(column, n, screen.shades[n]);
            }
        }
        for (n, list) in screen.lists.iter().enumerate() {
            if let Some(list) = list {
                self.list(list, n);
            }
        }
        for (depth, (from, to)) in screen.folds.iter().enumerate() {
            let (left, right) = (
                *from as f32 * COLUMN + 6.0,
                (*to as f32 + 1.0) * COLUMN - 6.0,
            );
            let colour = screen.upper[*from].colour;
            self.rect(
                left,
                BAND - 2.0 + depth as f32 * 2.0,
                right - left,
                1.5,
                colour,
                255,
            );
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
        for &(from, to, colour) in &screen.underlines {
            let x = f32::from(from) * COLUMN + 6.0;
            let width = f32::from(to - from + 1) * COLUMN - 12.0;
            self.rect(x, HEIGHT as f32 - 8.0, width, 2.0, colour, 255);
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
        if label.armed
            && let Some(dot) = PathBuilder::from_circle(x + 9.0, top + 11.0, 4.0)
        {
            self.fill(&dot, ARM);
        }
        let ink = if label.selected {
            self.rect(
                x + 3.0,
                top + 2.0,
                COLUMN - 6.0,
                BAND - 4.0,
                label.colour,
                255,
            );
            [0, 0, 0]
        } else {
            label.colour
        };
        match label.icon {
            Icon::None => self.centred(&label.text, centre, top + 17.0, 14.0, COLUMN - 14.0, ink),
            Icon::Group | Icon::Rack => {
                self.centred(&label.text, centre, top + 17.0, 14.0, COLUMN - 14.0, ink);
                self.fold_icon(label.icon, x + 6.0, top, ink);
            }
            icon if label.text.is_empty() => {
                self.icon(icon, centre, top + BAND / 2.0, ink);
            }
            icon => {
                self.icon(icon, x + 14.0, top + BAND / 2.0, ink);
                self.centred(
                    &label.text,
                    centre + 8.0,
                    top + 17.0,
                    14.0,
                    COLUMN - 36.0,
                    ink,
                );
            }
        }
    }

    /// An icon about 12 pixels across, centred on `x`, `y` (push-2-tracks
    /// R22): a rack is a frame with a mark for what it holds.
    fn icon(&mut self, icon: Icon, x: f32, y: f32, colour: [u8; 3]) {
        let mut path = PathBuilder::new();
        match icon {
            Icon::Left | Icon::Right => {
                let d = if icon == Icon::Left { -1.0 } else { 1.0 };
                path.move_to(x + d * 5.0, y);
                path.line_to(x - d * 4.0, y - 6.0);
                path.line_to(x - d * 4.0, y + 6.0);
                path.close();
                if let Some(path) = path.finish() {
                    self.fill(&path, colour);
                }
                return;
            }
            _ => {}
        }
        let mut frame = PathBuilder::new();
        frame.move_to(x - 6.0, y - 6.0);
        frame.line_to(x + 6.0, y - 6.0);
        frame.line_to(x + 6.0, y + 6.0);
        frame.line_to(x - 6.0, y + 6.0);
        frame.close();
        if let Some(frame) = frame.finish() {
            self.stroke(&frame, colour, 1.5);
        }
        match icon {
            // Keys: two bars.
            Icon::InstrumentRack => {
                self.rect(x - 2.5, y - 3.0, 1.5, 6.0, colour, 255);
                self.rect(x + 1.0, y - 3.0, 1.5, 6.0, colour, 255);
            }
            // A wave.
            Icon::AudioEffectRack => {
                path.move_to(x - 4.0, y + 1.0);
                path.line_to(x - 2.0, y - 3.0);
                path.line_to(x + 1.0, y + 3.0);
                path.line_to(x + 4.0, y - 1.0);
                if let Some(path) = path.finish() {
                    self.stroke(&path, colour, 1.2);
                }
            }
            // A dot and a stem: a note.
            Icon::MidiEffectRack => {
                self.rect(x + 0.5, y - 4.0, 1.5, 7.0, colour, 255);
                if let Some(dot) = PathBuilder::from_circle(x - 0.5, y + 3.0, 2.0) {
                    self.fill(&dot, colour);
                }
            }
            // A grid of pads.
            Icon::DrumRack => {
                for (dx, dy) in [(-3.5, -3.5), (0.5, -3.5), (-3.5, 0.5), (0.5, 0.5)] {
                    self.rect(x + dx, y + dy, 3.0, 3.0, colour, 255);
                }
            }
            Icon::None | Icon::Group | Icon::Rack | Icon::Left | Icon::Right => {}
        }
    }

    /// A column of the content band (R20).
    fn column(&mut self, column: &Column, n: usize, shade: Shade) {
        if column.is_empty() {
            return;
        }
        let x = n as f32 * COLUMN;
        let centre = x + COLUMN / 2.0;
        let (text, value) = match shade {
            Shade::Normal => (TEXT, DIM),
            Shade::Dimmed => ([95, 95, 95], [70, 70, 70]),
            Shade::Grey => ([120, 120, 120], [90, 90, 90]),
        };
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
        self.centred(&column.name, centre, BAND + 19.0, 15.0, COLUMN - 28.0, text);
        self.centred(
            &column.value,
            centre,
            BAND + 40.0,
            13.0,
            COLUMN - 10.0,
            value,
        );
        match &column.graphic {
            Graphic::None => {}
            Graphic::Arc(position) => {
                let lit = if shade == Shade::Normal {
                    [255, 255, 255]
                } else {
                    text
                };
                self.arc(centre, BAND + 76.0, position.clamp(0.0, 1.0), lit);
            }
            Graphic::Choice(entry) => {
                self.centred(entry, centre, BAND + 76.0, 16.0, COLUMN - 12.0, text);
            }
            Graphic::Fader(value) => self.fader(centre, value.clamp(0.0, 1.0)),
        }
    }

    /// A column under a clip's graphic: its name and its value in two lines
    /// (push-2-tracks R44).
    fn compact(&mut self, column: &Column, n: usize) {
        if column.is_empty() {
            return;
        }
        let centre = n as f32 * COLUMN + COLUMN / 2.0;
        if column.touched {
            self.rect(
                n as f32 * COLUMN + 2.0,
                CLIP_BOTTOM + 2.0,
                COLUMN - 4.0,
                HEIGHT as f32 - BAND - CLIP_BOTTOM - 4.0,
                [50, 50, 50],
                255,
            );
        }
        self.centred(
            &column.name,
            centre,
            CLIP_BOTTOM + 20.0,
            13.0,
            COLUMN - 10.0,
            TEXT,
        );
        self.centred(
            &column.value,
            centre,
            CLIP_BOTTOM + 38.0,
            12.0,
            COLUMN - 8.0,
            DIM,
        );
    }

    /// A clip over the content band: its name on a band of its colour, its
    /// notes or waveform with the markers, the playhead and the sequencers'
    /// boxes (push-2-tracks R44, R50 to R55).
    fn clip(&mut self, clip: &ClipScreen) {
        let (left, right) = (CLIP_LEFT, WIDTH as f32 - 12.0);
        let (top, bottom) = (CLIP_TOP, CLIP_BOTTOM - 4.0);
        let name = self.fit(&clip.name, 14.0, 360.0);
        let width = self.width(&name, 14.0) + 16.0;
        self.rect(left, BAND + 2.0, width, 17.0, clip.colour, 255);
        self.text(&name, left + 8.0, BAND + 15.0, 14.0, [0, 0, 0]);
        let (from, to) = clip.view;
        let across = (to - from).max(1e-9);
        let x = |beat: f64| left + (right - left) * ((beat - from) / across).clamp(0.0, 1.0) as f32;
        match &clip.body {
            ClipBody::Prompt(message) => {
                self.centred(
                    message,
                    (left + right) / 2.0,
                    (top + bottom) / 2.0 + 5.0,
                    18.0,
                    right - left,
                    DIM,
                );
            }
            ClipBody::Notes { rows, bars } => {
                let height = (bottom - top) / rows.len().max(1) as f32;
                if let Some(area) = &clip.area {
                    let (first, last) = area.rows.unwrap_or((0, rows.len().saturating_sub(1)));
                    let y = top + height * first as f32;
                    let h = height * (last + 1 - first.min(last + 1)) as f32;
                    self.crisp(x(area.from), y, x(area.to) - x(area.from), h, TEXT, 40);
                }
                for &(held_from, held_to) in &clip.held {
                    self.crisp(
                        x(held_from),
                        top,
                        x(held_to) - x(held_from),
                        bottom - top,
                        TEXT,
                        60,
                    );
                }
                for bar in bars {
                    let y = top + height * bar.row as f32;
                    let w = (x(bar.to) - x(bar.from)).max(2.0);
                    self.crisp(
                        x(bar.from),
                        y + 0.5,
                        w,
                        (height - 1.0).max(1.0),
                        bar.colour,
                        (bar.opacity.clamp(0.0, 1.0) * 255.0).round() as u8,
                    );
                }
            }
            ClipBody::Waveform(peaks) => {
                let mid = (top + bottom) / 2.0;
                let half = (bottom - top) / 2.0;
                let step = (right - left) / peaks.len().max(1) as f32;
                for (n, [low, high]) in peaks.iter().enumerate() {
                    let (a, b) = (mid - high * half, mid - low * half);
                    self.crisp(
                        left + step * n as f32,
                        a,
                        step.max(1.0),
                        (b - a).max(1.0),
                        clip.colour,
                        255,
                    );
                }
            }
        }
        for (name, at) in &clip.markers {
            if *at >= from && *at <= to {
                let loop_edge = name.starts_with("loop");
                self.crisp(
                    x(*at),
                    top,
                    1.5,
                    bottom - top,
                    TEXT,
                    if loop_edge { 230 } else { 120 },
                );
            }
        }
        if let Some(at) = clip.playhead
            && at >= from
            && at <= to
        {
            self.crisp(x(at), top, 2.0, bottom - top, [255, 255, 255], 255);
        }
        // The pitch-range box and the note lines at the left edge.
        for &(at, thickness) in &clip.lines {
            let y = bottom - (bottom - top) * at;
            self.crisp(
                0.0,
                y - 0.5,
                2.0 * f32::from(thickness),
                1.5,
                clip.colour,
                255,
            );
        }
        if let Some((low, high)) = clip.range {
            let (y_low, y_high) = (
                bottom - (bottom - top) * low,
                bottom - (bottom - top) * high,
            );
            self.crisp(1.0, y_high, 9.0, (y_low - y_high).max(2.0), TEXT, 90);
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

    /// A vertical fader: a track and the handle at its value.
    fn fader(&mut self, cx: f32, value: f32) {
        let (top, height) = (BAND + 50.0, 60.0);
        self.rect(cx - 2.0, top, 4.0, height, [70, 70, 70], 255);
        let y = top + height * (1.0 - value);
        self.rect(cx - 2.0, y, 4.0, top + height - y, TEXT, 255);
        self.rect(cx - 12.0, y - 2.0, 24.0, 4.0, TEXT, 255);
    }

    /// The icon of a label that unfolds, at its left in its band.
    fn fold_icon(&mut self, icon: Icon, x: f32, top: f32, colour: [u8; 3]) {
        match icon {
            // A folder: a tab above a body.
            Icon::Group => {
                self.rect(x, top + 6.0, 5.0, 3.0, colour, 255);
                self.rect(x, top + 9.0, 11.0, 7.0, colour, 255);
            }
            // A rack: three bars side by side.
            Icon::Rack => {
                for bar in 0..3 {
                    self.rect(x + 4.0 * bar as f32, top + 6.0, 3.0, 10.0, colour, 255);
                }
            }
            _ => {}
        }
    }

    fn dot(&mut self, x: f32, y: f32, colour: [u8; 3]) {
        if let Some(path) = PathBuilder::from_circle(x, y, 3.5) {
            self.fill(&path, colour);
        }
    }

    /// An arc of 270 degrees, open at the bottom, with its value in light.
    fn arc(&mut self, cx: f32, cy: f32, value: f32, lit: [u8; 3]) {
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
            self.stroke(&path, lit, 4.0);
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

//! The dense views as plain instance lists, drawn by either backend.
//!
//! The UI builds one [`DenseList`] per dense region each frame (the timeline,
//! and the meters in the track-header column). [`crate::paint_cpu`] turns a
//! list into an egui mesh; [`crate::paint_gpu`] uploads it as instance buffers
//! and draws it in a wgpu paint callback. Coordinates are in egui points.

use bytemuck::{Pod, Zeroable};
use egui::{Color32, Rect};

use crate::scene::{MIP_LEVELS, PeakRun};

/// A filled rectangle.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct RectInst {
    /// min x, min y, max x, max y.
    pub rect: [f32; 4],
    /// Premultiplied, gamma-space RGBA, as egui's `Color32`.
    pub color: [u8; 4],
}

/// One clip's waveform.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct WaveInst {
    /// The part to draw, already clipped to the view.
    pub draw: [f32; 4],
    /// The whole waveform area, which maps the peaks to x and y.
    pub full: [f32; 4],
    /// First peak and number of peaks at the chosen mip level.
    pub offset: u32,
    pub count: u32,
    pub color: [u8; 4],
    pub _pad: u32,
}

/// Rectangles and waveforms in two layers: everything before the split, then
/// everything after it (the clip being dragged), so a dragged clip sits on top.
#[derive(Clone, Debug, Default)]
pub struct DenseList {
    pub rects: Vec<RectInst>,
    pub waves: Vec<WaveInst>,
    pub rect_split: Option<usize>,
    pub wave_split: Option<usize>,
}

impl DenseList {
    pub fn clear(&mut self) {
        self.rects.clear();
        self.waves.clear();
        self.rect_split = None;
        self.wave_split = None;
    }

    pub fn rect(&mut self, rect: Rect, color: Color32) {
        self.rects.push(RectInst {
            rect: [rect.min.x, rect.min.y, rect.max.x, rect.max.y],
            color: color.to_array(),
        });
    }

    /// Adds a waveform; `full` is the clip's waveform area and `clip` the
    /// visible region it's cut to.
    pub fn wave(&mut self, full: Rect, clip: Rect, run: PeakRun, color: Color32) {
        let draw = full.intersect(clip);
        if !draw.is_positive() || run.len == 0 {
            return;
        }
        self.waves.push(WaveInst {
            draw: [draw.min.x, draw.min.y, draw.max.x, draw.max.y],
            full: [full.min.x, full.min.y, full.max.x, full.max.y],
            offset: run.offset,
            count: run.len,
            color: color.to_array(),
            _pad: 0,
        });
    }

    /// Starts the top layer.
    pub fn begin_top_layer(&mut self) {
        self.rect_split = Some(self.rects.len());
        self.wave_split = Some(self.waves.len());
    }

    /// The two layers as (rects, waves) ranges.
    pub fn layers(&self) -> [(std::ops::Range<usize>, std::ops::Range<usize>); 2] {
        let r = self.rect_split.unwrap_or(self.rects.len());
        let w = self.wave_split.unwrap_or(self.waves.len());
        [(0..r, 0..w), (r..self.rects.len(), w..self.waves.len())]
    }
}

/// Picks the coarsest mip level that still has at least one peak per pixel,
/// so each pixel column reads one to four peaks.
pub fn pick_level(levels: &[PeakRun; MIP_LEVELS], width_px: f32) -> usize {
    let mut k = 0;
    while k + 1 < MIP_LEVELS && levels[k + 1].len as f32 >= width_px {
        k += 1;
    }
    k
}

/// The range of peaks under pixel column `col`, counted from the waveform's
/// left edge in physical pixels. The GPU shader computes the same range.
pub fn column_peaks(col: f32, count: u32, width_px: f32) -> std::ops::Range<u32> {
    let r = count as f32 / width_px.max(1e-3);
    let p0 = ((col * r).floor().max(0.0) as u32).min(count.saturating_sub(1));
    let p1 = ((((col + 1.0) * r).ceil()).clamp(0.0, count as f32) as u32).max(p0 + 1);
    p0..p1.min(p0 + 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn levels(len0: u32) -> [PeakRun; MIP_LEVELS] {
        let mut out = [PeakRun::default(); MIP_LEVELS];
        let mut len = len0;
        for run in &mut out {
            run.len = len;
            len = len.div_ceil(4);
        }
        out
    }

    #[test]
    fn level_choice_keeps_one_to_four_peaks_per_pixel() {
        let l = levels(3_000);
        assert_eq!(pick_level(&l, 5_000.0), 0); // zoomed in: level 0
        assert_eq!(pick_level(&l, 3_000.0), 0);
        assert_eq!(pick_level(&l, 750.0), 1);
        assert_eq!(pick_level(&l, 100.0), 2);
        assert_eq!(pick_level(&l, 1.0), 5);
        for width in [10.0, 37.0, 200.0, 700.0, 2_999.0] {
            let k = pick_level(&l, width);
            let ppp = l[k].len as f32 / width;
            assert!((1.0..4.0 + 1e-3).contains(&ppp) || k == 0, "{width}: {ppp}");
        }
    }

    #[test]
    fn columns_cover_every_peak() {
        let count = 375;
        let width = 160.0;
        let mut next = 0;
        for col in 0..160 {
            let range = column_peaks(col as f32, count, width);
            assert!(range.start <= next, "gap before column {col}");
            next = next.max(range.end);
        }
        assert_eq!(next, count);
    }

    #[test]
    fn layers_split_where_asked() {
        let mut list = DenseList::default();
        let r = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(10.0, 10.0));
        list.rect(r, Color32::RED);
        list.begin_top_layer();
        list.rect(r, Color32::BLUE);
        let [bottom, top] = list.layers();
        assert_eq!(bottom.0, 0..1);
        assert_eq!(top.0, 1..2);
    }
}

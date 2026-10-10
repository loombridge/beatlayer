//! Dense views through egui's own painting: one `epaint::Mesh` per region,
//! rebuilt every frame and tessellated and uploaded by egui like any shape.

use egui::{Color32, Mesh, Painter, Pos2, Rect, Shape, pos2};

use crate::dense::{DenseList, WaveInst, column_peaks};
use crate::scene::Scene;

/// Paints `list` with `painter` as a single mesh.
pub fn paint(painter: &Painter, list: &DenseList, scene: &Scene, pixels_per_point: f32) {
    let mut mesh = Mesh::default();
    build_mesh(&mut mesh, list, scene, pixels_per_point);
    if !mesh.is_empty() {
        painter.add(Shape::mesh(mesh));
    }
}

/// Fills `mesh` with the rectangles and waveforms in `list`, layer by layer.
pub fn build_mesh(mesh: &mut Mesh, list: &DenseList, scene: &Scene, pixels_per_point: f32) {
    for (rects, waves) in list.layers() {
        for r in &list.rects[rects] {
            let color =
                Color32::from_rgba_premultiplied(r.color[0], r.color[1], r.color[2], r.color[3]);
            let rect = Rect::from_min_max(pos2(r.rect[0], r.rect[1]), pos2(r.rect[2], r.rect[3]));
            mesh.add_colored_rect(rect, color);
        }
        for w in &list.waves[waves] {
            add_wave(mesh, w, scene, pixels_per_point);
        }
    }
}

/// Adds one waveform as a strip: a top and a bottom vertex per physical pixel
/// column, joined by two triangles per column.
fn add_wave(mesh: &mut Mesh, w: &WaveInst, scene: &Scene, ppp: f32) {
    let color = Color32::from_rgba_premultiplied(w.color[0], w.color[1], w.color[2], w.color[3]);
    let full_left_px = w.full[0] * ppp;
    let width_px = (w.full[2] - w.full[0]) * ppp;
    let mid = 0.5 * (w.full[1] + w.full[3]);
    let half = 0.5 * (w.full[3] - w.full[1]) * 0.9;
    let min_height = 1.0 / ppp;

    let first_col = (w.draw[0] * ppp).floor() as i64;
    let last_col = (w.draw[2] * ppp).ceil() as i64;
    let mut previous: Option<u32> = None;
    for col in first_col..last_col {
        let rel = col as f32 - full_left_px;
        let mut lo = 1.0f32;
        let mut hi = -1.0f32;
        for p in column_peaks(rel, w.count, width_px) {
            let (a, b) = scene.peak(w.offset + p);
            lo = lo.min(a);
            hi = hi.max(b);
        }
        let mut top = mid - hi * half;
        let mut bottom = mid - lo * half;
        if bottom - top < min_height {
            let c = 0.5 * (top + bottom);
            top = c - 0.5 * min_height;
            bottom = c + 0.5 * min_height;
        }
        let x = (col as f32 + 0.5) / ppp;
        let x = x.clamp(w.draw[0], w.draw[2]);
        let i = mesh.vertices.len() as u32;
        mesh.colored_vertex(Pos2::new(x, top), color);
        mesh.colored_vertex(Pos2::new(x, bottom), color);
        if let Some(p) = previous {
            mesh.add_triangle(p, p + 1, i);
            mesh.add_triangle(i, p + 1, i + 1);
        }
        previous = Some(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Config, Scene};

    #[test]
    fn a_waveform_has_two_vertices_per_pixel_column() {
        let scene = Scene::generate(Config {
            tracks: 1,
            clips_per_track: 1,
            bars: 8,
            files: 1,
        });
        let clip = &scene.clips[0];
        let full = Rect::from_min_max(pos2(10.0, 0.0), pos2(110.0, 40.0));
        let mut list = DenseList::default();
        list.wave(full, Rect::EVERYTHING, clip.levels[0], Color32::WHITE);
        let mut mesh = Mesh::default();
        build_mesh(&mut mesh, &list, &scene, 2.0);
        assert_eq!(mesh.vertices.len(), 200 * 2);
        assert_eq!(mesh.indices.len(), 199 * 6);
        for v in &mesh.vertices {
            assert!((0.0..=40.0).contains(&v.pos.y));
        }
    }
}

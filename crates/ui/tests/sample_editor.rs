//! The Sample Editor, headless (WP-4.10): the waveform through the warp map,
//! the markers, and making, moving and deleting them by mouse and keys reach
//! the same commands agents run, and each drag is one undo step however many
//! frames it spans. Spec: sample-editor.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bl_core::time::Ticks;
use bl_media::Peaks;
use bl_model::{AudioData, ClipData, ClipId, Document};
use bl_ui::arrangement::peaks::MemoryPeaks;
use bl_ui::sample_editor::{Geometry, at_height};
use common::{Harness, Window, command};
use egui::epaint::Shape;
use egui::{Event, FullOutput, Key, Modifiers, PointerButton, Pos2, Rect, pos2};
use serde_json::{Value, json};

const SECONDS: u32 = 8;

/// Where the clicks in the test file are, in seconds.
const CLICKS: [f64; 4] = [0.5, 1.5, 2.25, 3.5];

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-sample-editor-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a 16-bit mono WAV file of `SECONDS` at 48 kHz that is silent but
/// for a short loud burst at each of `CLICKS`.
fn wav(path: &Path) {
    let rate = 48_000u32;
    let frames = SECONDS * rate;
    let data = frames * 2;
    let mut bytes = Vec::with_capacity(44 + data as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let t = f64::from(i) / f64::from(rate);
        let value: f64 = CLICKS
            .iter()
            .filter(|&&c| t >= c && t < c + 0.05)
            .map(|&c| {
                let age = t - c;
                (age * 880.0 * std::f64::consts::TAU).sin() * 0.8 * (-age * 60.0).exp()
            })
            .sum();
        bytes.extend_from_slice(&((value * 30_000.0) as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// What the tests change: a set with one audio clip, the file's peaks and
/// analysis ready, and the Clip View showing it.
struct Sample {
    h: Harness,
    clip: ClipId,
    _scratch: Scratch,
}

/// The test file as a clip four bars long on the first audio track, warped
/// or not, with the Clip View on it, at the reference size.
fn sample(name: &str, warp: bool) -> Sample {
    let scratch = Scratch::new(name);
    let path = scratch.0.join("Clicks.wav");
    wav(&path);
    let mut h = Harness::with_title_bar(Window::Main);
    let track = h
        .shell
        .editor()
        .document()
        .top_level()
        .find(|t| {
            h.shell
                .editor()
                .document()
                .track(*t)
                .is_some_and(|t| t.kind() == bl_model::TrackKind::Audio)
        })
        .unwrap();
    h.shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [path.display().to_string()], "track": track, "at": 0 }),
        )
        .unwrap();
    let (clip, media) = {
        let doc = h.shell.editor().document();
        let clip = doc.track(track).unwrap().clips().next().unwrap();
        let ClipData::Audio(audio) = &clip.data else {
            unreachable!("an audio clip")
        };
        (clip.id, audio.media)
    };
    h.shell.wait_analysis(media).unwrap();
    let mut peaks = MemoryPeaks::new();
    // The same file, read again for its peaks.
    let store = bl_media::MediaStore::new();
    let data = store.load(store.import(&path).unwrap()).unwrap();
    peaks.insert(media, Arc::new(Peaks::from_samples(&data).unwrap()));
    h.main
        .arrangement_mut()
        .set_peak_source(Box::new(peaks), None);
    if warp {
        h.shell
            .run("clip.set_warp", json!({ "clip": clip, "warp": true }))
            .unwrap();
    }
    h.shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    h.settle();
    Sample {
        h,
        clip,
        _scratch: scratch,
    }
}

impl Sample {
    fn doc(&self) -> &Document {
        self.h.shell.editor().document()
    }

    fn audio(&self) -> AudioData {
        match &self.doc().clip(self.clip).unwrap().data {
            ClipData::Audio(audio) => audio.clone(),
            ClipData::Midi(_) => unreachable!("an audio clip"),
        }
    }

    fn steps(&self) -> usize {
        self.h.shell.editor().undo_history().len()
    }

    /// The Clip View's rectangle, from its accessible node.
    fn view(&mut self) -> Rect {
        let output = self.h.settle();
        let nodes = Harness::nodes(&output);
        let node = Harness::node(&nodes, "Clip View");
        let bounds = node.bounds.expect("the Clip View has bounds");
        Rect::from_min_max(
            pos2(bounds.x0 as f32, bounds.y0 as f32),
            pos2(bounds.x1 as f32, bounds.y1 as f32),
        )
    }

    /// Where the editor puts things now.
    fn geometry(&mut self) -> Geometry {
        let view = self.view();
        let audio = self.audio();
        let span = self.h.shell.clip_view().sample().span();
        Geometry::new(view, &audio, f64::from(SECONDS), span)
    }

    /// A call to the shell, as an agent makes it.
    fn run(&mut self, id: &str, args: Value) -> Value {
        let result = self.h.shell.run(id, args).unwrap();
        self.h.settle();
        result
    }

    fn markers(&self) -> Vec<(f64, i64)> {
        self.audio()
            .warp_markers
            .iter()
            .map(|m| (m.time, m.beat.get()))
            .collect()
    }

    /// A press, `moves` and a release, one frame each.
    fn drag(&mut self, from: Pos2, to: &[Pos2], modifiers: Modifiers) {
        self.h.frame(vec![
            Event::ModifiersChanged(modifiers),
            Event::PointerMoved(from),
        ]);
        self.h.frame(vec![Event::PointerButton {
            pos: from,
            button: PointerButton::Primary,
            pressed: true,
            modifiers,
        }]);
        for &at in to {
            self.h.frame(vec![Event::PointerMoved(at)]);
        }
        let end = *to.last().unwrap_or(&from);
        self.h.frame(vec![Event::PointerButton {
            pos: end,
            button: PointerButton::Primary,
            pressed: false,
            modifiers,
        }]);
        self.h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
        self.h.settle();
    }

    /// A click with `modifiers` at a point.
    fn click_with(&mut self, at: Pos2, modifiers: Modifiers) {
        self.h.frame(vec![
            Event::ModifiersChanged(modifiers),
            Event::PointerMoved(at),
        ]);
        for pressed in [true, false] {
            self.h.frame(vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers,
            }]);
        }
        self.h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
        self.h.settle();
    }

    /// Two clicks in quick succession with `modifiers` held.
    fn double_click_with(&mut self, at: Pos2, modifiers: Modifiers) {
        self.h.frame(vec![Event::ModifiersChanged(modifiers)]);
        self.double_click_holding(at, modifiers);
        self.h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
        self.h.settle();
    }

    /// Two clicks in quick succession.
    fn double_click(&mut self, at: Pos2) {
        self.double_click_holding(at, Modifiers::NONE);
    }

    fn double_click_holding(&mut self, at: Pos2, modifiers: Modifiers) {
        // egui counts a click after a double-click within 0.6 s as a triple.
        self.h.time += 1.0;
        self.h.frame(vec![Event::PointerMoved(at)]);
        for pressed in [true, false, true, false] {
            self.h.frame(vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers,
            }]);
        }
        self.h.settle();
    }

    /// The pointer moves part of the way, frame by frame.
    fn path(from: Pos2, to: Pos2, frames: usize) -> Vec<Pos2> {
        (1..=frames)
            .map(|i| from + (to - from) * (i as f32 / frames as f32))
            .collect()
    }
}

#[test]
fn the_sample_editor_takes_the_note_editors_place() {
    // Spec: sample-editor R1.
    let mut s = sample("place", true);
    let output = s.h.settle();
    let nodes = Harness::nodes(&output);
    Harness::node(&nodes, "Sample");
    Harness::node(&nodes, "Envelopes");
    assert!(nodes.iter().all(|n| n.label != "Notes" && n.label != "MPE"));
    let wave = nodes
        .iter()
        .find(|n| n.label.starts_with("Waveform of"))
        .expect("the waveform is a region");
    assert!(wave.label.contains("Clicks.wav"), "{}", wave.label);
    let view = s.view();
    let geometry = s.geometry();
    assert!(view.contains_rect(geometry.wave));
    assert!(s.h.main.clip_view().sample_stats().columns > 0);
}

/// The columns of the waveform that carry signal, as runs `(left, right)` in
/// points, found in the meshes a frame drew.
fn bursts(output: &FullOutput, wave: Rect) -> Vec<(f32, f32)> {
    fn meshes(shape: &Shape, out: &mut Vec<Vec<(f32, f32)>>) {
        match shape {
            Shape::Mesh(mesh) if mesh.vertices.len() >= 400 => {
                out.push(
                    mesh.vertices
                        .chunks(4)
                        .map(|quad| {
                            let top = quad.iter().map(|v| v.pos.y).fold(f32::MAX, f32::min);
                            let bottom = quad.iter().map(|v| v.pos.y).fold(f32::MIN, f32::max);
                            (quad[0].pos.x, bottom - top)
                        })
                        .collect(),
                );
            }
            Shape::Vec(shapes) => shapes.iter().for_each(|s| meshes(s, out)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    for clipped in &output.shapes {
        meshes(&clipped.shape, &mut found);
    }
    let columns = found.into_iter().next().unwrap_or_default();
    let _ = wave;
    let mut runs: Vec<(f32, f32)> = Vec::new();
    for (x, height) in columns {
        if height < 8.0 {
            continue;
        }
        match runs.last_mut() {
            Some(run) if x - run.1 <= 2.0 => run.1 = x,
            _ => runs.push((x, x)),
        }
    }
    runs
}

#[test]
fn a_warped_clips_waveform_lines_up_with_its_beats() {
    // Spec: sample-editor R3, R4. A marker that puts the last click two
    // beats later draws it there, stretching the audio before it.
    let mut s = sample("warped", true);
    let clip = s.clip;
    s.run(
        "clip.insert_warp_marker",
        json!({ "clips": [clip], "time": 3.5 }),
    );
    s.run(
        "clip.move_warp_markers",
        json!({ "clips": [clip], "markers": [1], "by": Ticks::QUARTER.get() * 2 }),
    );
    let geometry = s.geometry();
    let output = s.h.settle();
    let runs = bursts(&output, geometry.wave);
    let expected: Vec<f32> = CLICKS.iter().map(|&t| geometry.x_of_time(t)).collect();
    assert_eq!(runs.len(), CLICKS.len(), "{runs:?}");
    for ((left, _), x) in runs.iter().zip(&expected) {
        assert!((left - x).abs() <= 2.0, "a burst starts at {left}, not {x}");
    }
    // The last click sits two beats later than the unwarped file puts it.
    let unwarped = geometry.axis.x_of(3.5 * 2.0 * Ticks::QUARTER.get() as f64);
    assert!(
        (expected[3]
            - unwarped
            - 2.0 * Ticks::QUARTER.get() as f32 * geometry.axis.width
                / geometry.axis.length as f32)
            .abs()
            < 1.0
    );
}

#[test]
fn an_unwarped_clip_runs_along_time_in_the_file() {
    // Spec: sample-editor R3.
    let mut s = sample("unwarped", false);
    let geometry = s.geometry();
    assert!(!geometry.warped);
    assert_eq!(
        (geometry.axis.start, geometry.axis.end()),
        (0.0, f64::from(SECONDS))
    );
    let output = s.h.settle();
    let runs = bursts(&output, geometry.wave);
    assert_eq!(runs.len(), CLICKS.len(), "{runs:?}");
    for ((left, _), &t) in runs.iter().zip(&CLICKS) {
        let x = geometry.wave.left() + (t / f64::from(SECONDS)) as f32 * geometry.wave.width();
        assert!((left - x).abs() <= 2.0, "a burst starts at {left}, not {x}");
    }
}

/// The warped sample with the editing grid on quarter notes, so a click lands
/// on a beat, and a marker on the second bar's first beat.
fn marked(name: &str) -> Sample {
    let mut s = sample(name, true);
    s.run("arrangement.set_grid", json!({ "fixed": "quarter" }));
    let clip = s.clip;
    s.run(
        "clip.insert_warp_marker",
        json!({ "clips": [clip], "time": 2.0 }),
    );
    s
}

/// Where beat `beat` (counting quarter notes from 1.1.1) is, in the upper
/// half of the waveform (or the lower half with `fraction` near 1).
fn at_beat(g: &Geometry, beat: i64, fraction: f32) -> Pos2 {
    at_height(
        g.wave,
        g.axis.x_of((beat * Ticks::QUARTER.get()) as f64),
        fraction,
    )
}

#[test]
fn double_clicks_make_and_delete_warp_markers() {
    // Spec: sample-editor R6.
    let mut s = sample("double", true);
    s.run("arrangement.set_grid", json!({ "fixed": "quarter" }));
    let g = s.geometry();
    let steps = s.steps();
    let quarter = Ticks::QUARTER.get();
    assert_eq!(s.markers().len(), 1);
    // A double-click in the upper half makes a marker on the nearest beat.
    s.double_click(at_beat(&g, 4, 0.2) + egui::vec2(9.0, 0.0));
    assert_eq!(s.markers(), [(0.0, 0), (2.0, 4 * quarter)]);
    assert_eq!(s.steps(), steps + 1, "one undo step");
    s.double_click(at_beat(&g, 8, 0.2));
    assert_eq!(s.markers().len(), 3);
    // The lower half makes none.
    s.double_click(at_beat(&g, 12, 0.8));
    assert_eq!(s.markers().len(), 3);
    assert_eq!(s.steps(), steps + 2);
    // A click on a marker selects it, Cmd-click adds another to the
    // selection, and a double-click deletes it.
    let on = at_beat(&g, 4, 0.5);
    s.click_with(on, Modifiers::NONE);
    assert_eq!(s.h.shell.clip_view().sample().markers(), [1]);
    s.click_with(at_beat(&g, 8, 0.5), command());
    assert_eq!(s.h.shell.clip_view().sample().markers(), [1, 2]);
    s.double_click(on);
    assert_eq!(s.markers().len(), 2);
    assert_eq!(s.markers()[1].1, 8 * quarter);
    assert_eq!(s.steps(), steps + 3);
    s.h.shell.invoke("edit.undo", json!({})).unwrap();
    assert_eq!(s.markers().len(), 3);
}

#[test]
fn dragging_a_marker_stretches_the_audio_in_one_step() {
    // Spec: sample-editor R7, warping R7.
    let mut s = marked("drag");
    let g = s.geometry();
    let steps = s.steps();
    let from = at_beat(&g, 4, 0.7);
    // The grabbed marker lands on the nearest beat, and the audio it was
    // pinned to stays where it was in the file.
    let to = at_beat(&g, 6, 0.7) + egui::vec2(11.0, 0.0);
    let path = Sample::path(from, to, 12);
    s.drag(from, &path, Modifiers::NONE);
    assert_eq!(s.markers(), [(0.0, 0), (2.0, 6 * Ticks::QUARTER.get())]);
    assert_eq!(s.steps(), steps + 1, "however many frames the drag lasted");
    s.h.shell.invoke("edit.undo", json!({})).unwrap();
    assert_eq!(s.markers(), [(0.0, 0), (2.0, 4 * Ticks::QUARTER.get())]);
}

#[test]
fn a_drag_stops_short_of_a_neighbour_and_moves_every_selected_marker() {
    // Spec: sample-editor R7.
    let mut s = marked("neighbour");
    let clip = s.clip;
    s.run(
        "clip.insert_warp_marker",
        json!({ "clips": [clip], "time": 4.0 }),
    );
    s.run(
        "clip_view.select_warp_markers",
        json!({ "markers": [0, 1] }),
    );
    let g = s.geometry();
    // Dragging the second marker far right takes the first with it, and
    // stops a tick short of the third.
    let from = at_beat(&g, 4, 0.7);
    let to = at_beat(&g, 14, 0.7);
    let path = Sample::path(from, to, 10);
    s.drag(from, &path, Modifiers::NONE);
    let markers = s.markers();
    assert_eq!(markers.len(), 3);
    assert_eq!(
        markers[2].1,
        8 * Ticks::QUARTER.get(),
        "the third stays put"
    );
    assert_eq!(markers[1].1, 8 * Ticks::QUARTER.get() - 1);
    assert_eq!(markers[0].1, 4 * Ticks::QUARTER.get() - 1);
}

#[test]
fn shift_dragging_moves_the_waveform_under_a_marker() {
    // Spec: sample-editor R7. The marker stays on its beat; its time in the
    // file moves the other way.
    let mut s = marked("shift");
    let g = s.geometry();
    let steps = s.steps();
    let from = at_beat(&g, 4, 0.7);
    let to = at_beat(&g, 5, 0.7);
    let path = Sample::path(from, to, 8);
    s.drag(from, &path, Modifiers::SHIFT);
    let markers = s.markers();
    assert_eq!(markers[1].1, 4 * Ticks::QUARTER.get(), "the beat stays");
    assert!((markers[1].0 - 1.5).abs() < 0.01, "{markers:?}");
    assert_eq!(s.steps(), steps + 1);
}

#[test]
fn esc_cancels_a_marker_drag() {
    // Spec: sample-editor R7.
    let mut s = marked("esc");
    let g = s.geometry();
    let steps = s.steps();
    let before = s.markers();
    let from = at_beat(&g, 4, 0.7);
    let to = at_beat(&g, 8, 0.7);
    s.h.frame(vec![Event::PointerMoved(from)]);
    s.h.frame(vec![Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    for at in Sample::path(from, to, 8) {
        s.h.frame(vec![Event::PointerMoved(at)]);
    }
    assert_ne!(s.markers(), before, "the drag moved the marker");
    s.h.key(Key::Escape, Modifiers::NONE);
    s.h.frame(vec![Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    s.h.settle();
    assert_eq!(s.markers(), before);
    assert_eq!(s.steps(), steps, "nothing is recorded");
}

/// The clip's transients, as times in the file.
fn transients(s: &mut Sample) -> Vec<f64> {
    let clip = s.clip;
    let result = s.run("clip.transients", json!({ "clip": clip }));
    result["transients"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["time"].as_f64().unwrap())
        .collect()
}

/// Every rectangle a frame paints with a fill, as `(rect, fill)`.
fn painted_rects(output: &FullOutput) -> Vec<(Rect, egui::Color32)> {
    fn walk(shape: &Shape, out: &mut Vec<(Rect, egui::Color32)>) {
        match shape {
            Shape::Rect(rect) => out.push((rect.rect, rect.fill)),
            Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

#[test]
fn markers_transients_and_the_pseudo_warp_marker_are_drawn() {
    // Spec: sample-editor R5.
    let mut s = marked("drawn");
    let clip = s.clip;
    let times = transients(&mut s);
    assert!(
        times.len() >= CLICKS.len(),
        "the analysis finds the clicks: {times:?}"
    );
    let g = s.geometry();
    let output = s.h.settle();
    let nodes = Harness::nodes(&output);
    // Each warp marker, transient, and the Start and End markers and the
    // loop brace is an item with its name and value.
    let marker = Harness::node(&nodes, "Warp Marker 2");
    assert!(marker.value.contains("2.000 s"), "{}", marker.value);
    assert!(marker.value.contains("beat 2.1.1"), "{}", marker.value);
    assert_eq!(marker.selected, Some(false));
    assert_eq!(
        nodes
            .iter()
            .filter(|n| n.label.starts_with("Transient "))
            .count(),
        times.len()
    );
    for name in ["Start Marker", "End Marker", "Loop Brace"] {
        Harness::node(&nodes, name);
    }
    s.run(
        "clip_view.select_warp_markers",
        json!({ "clip": clip, "markers": [1] }),
    );
    let output = s.h.settle();
    let nodes = Harness::nodes(&output);
    assert_eq!(Harness::node(&nodes, "Warp Marker 2").selected, Some(true));
    // A transient with no warp marker offers a pseudo-warp marker while the
    // pointer is over it, in a quieter colour than a warp marker's.
    let accent = s.h.theme.colors.accent.0;
    let quiet = accent.gamma_multiply(0.5);
    let transient = times
        .iter()
        .copied()
        .find(|t| (t - 2.0).abs() > 0.2)
        .unwrap();
    let x = g.x_of_time(transient);
    let line = |rects: &[(Rect, egui::Color32)], fill| {
        rects.iter().any(|(r, f)| {
            *f == fill && r.height() >= g.wave.height() - 1.0 && (r.center().x - x).abs() <= 1.0
        })
    };
    let before = painted_rects(&s.h.settle());
    assert!(!line(&before, quiet));
    s.h.frame(vec![Event::PointerMoved(at_height(g.wave, x, 0.2))]);
    let hovering = painted_rects(&s.h.frame(vec![]));
    assert!(
        line(&hovering, quiet),
        "the pseudo-warp marker shows under the pointer"
    );
    s.h.frame(vec![Event::PointerMoved(at_height(g.wave, x, 0.9))]);
    assert!(!line(&painted_rects(&s.h.frame(vec![])), quiet));
}

#[test]
fn a_pseudo_warp_marker_becomes_real_and_pins_its_neighbours_with_cmd() {
    // Spec: sample-editor R8, warping R26.
    let mut s = sample("pseudo", true);
    s.run("arrangement.set_grid", json!({ "fixed": "quarter" }));
    let times = transients(&mut s);
    let g = s.geometry();
    let steps = s.steps();
    let t = times[1];
    let x = g.x_of_time(t);
    // A double-click makes it a warp marker at the transient's time.
    s.double_click(at_height(g.wave, x, 0.2));
    let markers = s.markers();
    assert_eq!(markers.len(), 2);
    assert!((markers[1].0 - t).abs() < 1e-9, "{markers:?}");
    assert_eq!(s.steps(), steps + 1);
    s.h.shell.invoke("edit.undo", json!({})).unwrap();
    // Dragging it is that insertion and a move, in one undo step.
    let beat = g.axis_of(t);
    let from = at_height(g.wave, x, 0.2);
    let to = from
        + egui::vec2(
            g.axis.width / g.axis.length as f32 * Ticks::QUARTER.get() as f32 * 2.0,
            0.0,
        );
    let path = Sample::path(from, to, 10);
    let before = s.steps();
    s.drag(from, &path, Modifiers::NONE);
    let markers = s.markers();
    assert_eq!(markers.len(), 2, "{markers:?}");
    assert!((markers[1].0 - t).abs() < 1e-9);
    let moved = markers[1].1 as f64 - beat;
    assert!(
        (moved - 2.0 * Ticks::QUARTER.get() as f64).abs() < Ticks::QUARTER.get() as f64,
        "moved {moved}"
    );
    assert_eq!(s.markers()[1].1 % Ticks::QUARTER.get(), 0, "on a beat");
    assert_eq!(s.steps(), before + 1, "one step for the whole drag");
    s.h.shell.invoke("edit.undo", json!({})).unwrap();
    assert_eq!(
        s.markers().len(),
        1,
        "the insertion and the move undo together"
    );
    // With Cmd (Ctrl) held, the transients either side get markers too.
    s.double_click_with(at_height(g.wave, x, 0.2), command());
    let markers = s.markers();
    assert_eq!(markers.len(), 4, "{markers:?}");
    assert!((markers[1].0 - times[0]).abs() < 1e-9);
    assert!((markers[3].0 - times[2]).abs() < 1e-9);
}

/// The point in the middle of the node labelled `label`, in the nodes.
fn centre_of(nodes: &[common::Node], label: &str) -> Pos2 {
    let node = nodes
        .iter()
        .rfind(|n| n.label == label && n.bounds.is_some())
        .unwrap_or_else(|| panic!("no node labelled {label:?}"));
    let b = node.bounds.unwrap();
    pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
}

#[test]
fn clicks_set_the_insert_marker_and_drags_select_time() {
    // Spec: sample-editor R9. None of it is an undo step.
    let mut s = sample("clicks", true);
    s.run("arrangement.set_grid", json!({ "fixed": "quarter" }));
    let g = s.geometry();
    let steps = s.steps();
    // A click away from a marker puts the insert marker on the nearest beat.
    s.click_with(at_beat(&g, 3, 0.8) + egui::vec2(8.0, 0.0), Modifiers::NONE);
    let state = s.run("clip_view.sample_editor", json!({}));
    assert_eq!(state["insert_marker"]["time"], 1.5);
    assert_eq!(state["insert_marker"]["beat"], 3 * Ticks::QUARTER.get());
    // Dragging selects a span, on the grid.
    let from = at_beat(&g, 4, 0.8) + egui::vec2(-6.0, 0.0);
    let to = at_beat(&g, 8, 0.8) + egui::vec2(5.0, 0.0);
    s.drag(from, &Sample::path(from, to, 8), Modifiers::NONE);
    let state = s.run("clip_view.sample_editor", json!({}));
    assert_eq!(
        state["insert_marker"],
        Value::Null,
        "a selection replaces the marker"
    );
    assert_eq!(state["time_selection"][0]["time"], 2.0);
    assert_eq!(state["time_selection"][1]["time"], 4.0);
    // Shift-click extends it to the click.
    s.click_with(at_beat(&g, 12, 0.8), Modifiers::SHIFT);
    let state = s.run("clip_view.sample_editor", json!({}));
    assert_eq!(state["time_selection"][0]["time"], 2.0);
    assert_eq!(state["time_selection"][1]["time"], 6.0);
    assert_eq!(s.steps(), steps, "never an undo step");
}

#[test]
fn the_scrub_area_moves_the_song_to_the_clips_point() {
    // Spec: sample-editor R9.
    let mut s = sample("scrub", true);
    let g = s.geometry();
    s.h.take_events();
    let at = pos2(
        g.axis.x_of(8.0 * Ticks::QUARTER.get() as f64),
        g.strip.center().y - 3.0,
    );
    s.click_with(at, Modifiers::NONE);
    let located: Vec<_> = s
        .h
        .take_events()
        .into_iter()
        .filter_map(|e| match e {
            bl_ui::main_window::UiEvent::Invoke(i) if i.id == "transport.locate" => Some(i.args),
            _ => None,
        })
        .collect();
    // The clip starts at 1.1.1, so the song goes to the same tick.
    assert_eq!(located, [json!({ "position": 8 * Ticks::QUARTER.get() })]);
}

#[test]
fn the_sample_editor_keys_run_their_commands() {
    // Spec: sample-editor R10.
    let mut s = sample("keys", true);
    s.run("arrangement.set_grid", json!({ "fixed": "quarter" }));
    let g = s.geometry();
    let quarter = Ticks::QUARTER.get();
    let steps = s.steps();
    // Click the waveform: it takes the focus and puts the insert marker.
    s.click_with(at_beat(&g, 4, 0.8), Modifiers::NONE);
    assert!(
        s.h.shell
            .active_contexts()
            .contains(&bl_commands::Context::SampleEditor)
    );
    // Cmd+I inserts a warp marker at the insert marker.
    s.h.key(Key::I, command());
    assert_eq!(s.markers(), [(0.0, 0), (2.0, 4 * quarter)]);
    // Delete deletes the selected marker.
    s.click_with(at_beat(&g, 4, 0.5), Modifiers::NONE);
    s.h.key(Key::Delete, Modifiers::NONE);
    assert_eq!(s.markers().len(), 1);
    s.run("clip_view.set_insert_marker", json!({}));
    s.h.key(Key::I, command());
    assert_eq!(s.markers().len(), 1, "no insert marker, no marker");
    // Left and Right move the selected marker a grid step.
    s.click_with(at_beat(&g, 6, 0.8), Modifiers::NONE);
    s.h.key(Key::I, command());
    s.run("clip_view.select_warp_markers", json!({ "markers": [1] }));
    s.h.key(Key::ArrowRight, Modifiers::NONE);
    assert_eq!(s.markers()[1].1, 7 * quarter);
    s.h.key(Key::ArrowLeft, Modifiers::NONE);
    s.h.key(Key::ArrowLeft, Modifiers::NONE);
    assert_eq!(s.markers()[1].1, 5 * quarter);
    // Cmd+Left and Cmd+Right select the previous or next marker.
    s.h.key(Key::ArrowLeft, command());
    assert_eq!(s.h.shell.clip_view().sample().markers(), [0]);
    s.h.key(Key::ArrowRight, command());
    assert_eq!(s.h.shell.clip_view().sample().markers(), [1]);
    // Cmd+Shift+I puts a transient at the insert marker; selecting it and
    // pressing Cmd+Shift+Delete takes it out.
    let transients_before = transients(&mut s).len();
    s.click_with(at_beat(&g, 14, 0.8), Modifiers::NONE);
    let marker = s.run("clip_view.sample_editor", json!({}))["insert_marker"]["time"]
        .as_f64()
        .unwrap();
    s.h.key(Key::I, command() | Modifiers::SHIFT);
    let now = transients(&mut s);
    assert_eq!(now.len(), transients_before + 1);
    assert!(now.iter().any(|t| (t - marker).abs() < 1e-9));
    s.run("clip_view.select_transients", json!({ "times": [marker] }));
    s.h.key(Key::Delete, command() | Modifiers::SHIFT);
    assert_eq!(transients(&mut s).len(), transients_before);
    assert!(s.steps() > steps);
}

#[test]
fn z_x_w_and_h_zoom_and_follow_pauses_on_an_edit() {
    // Spec: sample-editor R11.
    let mut s = sample("zoom", true);
    let g = s.geometry();
    let whole = g.axis.length;
    s.click_with(at_beat(&g, 4, 0.8), Modifiers::NONE);
    s.run("clip_view.select_time", json!({ "start": 2.0, "end": 4.0 }));
    // Z zooms to the time selection, X goes back, W fits the whole file.
    s.h.key(Key::Z, Modifiers::NONE);
    let zoomed = s.geometry();
    assert_eq!(zoomed.axis.start, 4.0 * Ticks::QUARTER.get() as f64);
    assert_eq!(zoomed.axis.length, 4.0 * Ticks::QUARTER.get() as f64);
    s.h.key(Key::X, Modifiers::NONE);
    assert_eq!(s.geometry().axis.length, whole);
    s.h.key(Key::Z, Modifiers::NONE);
    s.h.key(Key::W, Modifiers::NONE);
    assert_eq!(s.geometry().axis.length, whole);
    // H fits the waveform to the editor's height, and again turns it off.
    s.h.key(Key::H, Modifiers::NONE);
    assert!(s.h.shell.clip_view().sample().fit_height());
    s.h.key(Key::H, Modifiers::NONE);
    assert!(!s.h.shell.clip_view().sample().fit_height());
    // Scrolling by hand is a span, not a zoom: X doesn't remember it.
    s.h.shell.run("clip_view.sample_view", json!({ "show": "span", "start": 0.0, "end": 4.0 * Ticks::QUARTER.get() as f64, "ticks": true })).unwrap();
    s.h.shell.run("clip_view.sample_view", json!({ "show": "scroll", "start": 100.0, "end": 100.0 + 4.0 * Ticks::QUARTER.get() as f64, "ticks": true })).unwrap();
    assert!(s.h.shell.clip_view().sample().follow_paused());
    s.h.key(Key::X, Modifiers::NONE);
    assert_eq!(s.geometry().axis.length, whole);
    // An edit pauses Follow too, and starting the song lets it follow again.
    s.run("clip_view.sample_view", json!({ "show": "whole" }));
    let clip = s.clip;
    s.run(
        "clip.insert_warp_marker",
        json!({ "clips": [clip], "time": 2.0 }),
    );
    assert!(s.h.shell.clip_view().sample().follow_paused());
}

impl Sample {
    /// A right-click at a point, and the frame's output with the menu open.
    fn right_click(&mut self, at: Pos2) -> FullOutput {
        self.h.frame(vec![Event::PointerMoved(at)]);
        for pressed in [true, false] {
            self.h.frame(vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Secondary,
                pressed,
                modifiers: Modifiers::NONE,
            }]);
        }
        self.h.settle()
    }
}

#[test]
fn the_context_menu_runs_the_warp_commands_and_disables_refused_ones() {
    // Spec: sample-editor R12.
    let mut s = sample("menu", true);
    s.run("arrangement.set_grid", json!({ "fixed": "quarter" }));
    let g = s.geometry();
    let output = s.right_click(at_beat(&g, 4, 0.8));
    let nodes = Harness::nodes(&output);
    let item = |label: &str| {
        nodes
            .iter()
            .rfind(|n| n.label == label && n.bounds.is_some())
            .unwrap_or_else(|| panic!("no menu item {label:?}"))
    };
    // The commands of packages that haven't landed are there, disabled.
    for pending in [
        "Set 1.1.1 Here",
        "Warp From Here",
        "Warp From Here (Start At …)",
        "Warp From Here (Straight)",
        "Warp … BPM From Here",
        "Warp as x-Bar Loop",
        "Quantize",
        "Crop Clip Sample",
    ] {
        assert!(item(pending).disabled, "{pending}");
    }
    // Without a time selection there are no selection items.
    assert!(
        nodes
            .iter()
            .all(|n| n.label != "Warp Selection as x-Bar Loop")
    );
    // The right-click put the insert marker there, so Insert Warp Marker
    // runs; nothing is selected, so Delete Warp Marker is refused.
    assert!(!item("Insert Warp Marker").disabled);
    assert!(item("Delete Warp Marker").disabled);
    assert!(!item("Insert Transient").disabled);
    assert!(!item("Reset Transients").disabled);
    assert!(!item("Show in Browser").disabled);
    s.h.click(centre_of(&nodes, "Insert Warp Marker"));
    assert_eq!(s.markers(), [(0.0, 0), (2.0, 4 * Ticks::QUARTER.get())]);
    // With a time selection the menu adds its selection items.
    s.run("clip_view.select_time", json!({ "start": 1.0, "end": 3.0 }));
    let output = s.right_click(at_beat(&g, 5, 0.8));
    let nodes = Harness::nodes(&output);
    for label in [
        "Warp Selection as x-Bar Loop",
        "Crop Clip Sample to Time Selection",
    ] {
        assert!(nodes.iter().any(|n| n.label == label), "{label}");
    }
    // Show in Browser runs the browser's command.
    s.h.take_events();
    s.h.click(centre_of(&nodes, "Show in Browser"));
    let shown: Vec<_> = s
        .h
        .take_events()
        .into_iter()
        .filter_map(|e| match e {
            bl_ui::main_window::UiEvent::Invoke(i) if i.id == "browser.show_file" => Some(i.args),
            _ => None,
        })
        .collect();
    assert_eq!(shown, [json!({ "clip": s.clip })]);
}

#[test]
fn only_the_markers_in_view_are_built() {
    // Spec: sample-editor R22. A file with a warp marker and a transient
    // every few milliseconds draws what is in view, not what the file holds.
    let mut s = sample("many", true);
    let clip = s.clip;
    let mut audio = s.audio();
    audio.warp_markers = (0..2000)
        .map(|i| bl_model::WarpMarker {
            time: f64::from(i) * 0.004,
            beat: Ticks(i64::from(i) * 1_000),
        })
        .collect();
    audio.transient_edits.added = (0..2000).map(|i| 0.002 + f64::from(i) * 0.004).collect();
    s.run(
        "clip.set_data",
        json!({ "clip": clip, "data": ClipData::Audio(audio) }),
    );
    let whole = s.h.main.clip_view().sample_stats();
    assert!(whole.markers >= 1_900, "{whole:?}");
    let span = 100_000.0;
    s.run(
        "clip_view.sample_view",
        json!({ "show": "span", "start": 500_000.0, "end": 500_000.0 + span, "ticks": true }),
    );
    let zoomed = s.h.main.clip_view().sample_stats();
    assert!(zoomed.markers <= 110, "{zoomed:?}");
    assert!(zoomed.transients <= 110, "{zoomed:?}");
    assert!(zoomed.columns > 0);
}

#[test]
fn the_create_menu_holds_the_marker_items_while_the_sample_editor_has_focus() {
    // Spec: sample-editor R13 (menus.md's note under the Create menu).
    use bl_shell::menus::{MenuPlatform, menu_bar};
    use bl_ui::keymap::{Keymap, Platform};
    use bl_ui::menus::item_state;
    let mut s = marked("menus");
    let keymap = Keymap::live12();
    let bar = menu_bar(MenuPlatform::current());
    let create = bar.iter().find(|m| m.title == "Create").unwrap();
    let edit = bar.iter().find(|m| m.title == "Edit").unwrap();
    let state = |s: &Sample, menu: &bl_shell::menus::Menu, label: &str| {
        let item = menu
            .walk()
            .find(|i| i.label == label)
            .unwrap_or_else(|| panic!("no menu item {label:?}"));
        item_state(item, &s.h.shell, &keymap, Platform::current())
    };
    // The keyboard focus is outside the editor: the items can't run.
    s.h.shell.set_focused_area(bl_shell::Area::MainView);
    for label in ["Insert Warp Marker", "Insert Transient", "Delete Transient"] {
        assert!(!state(&s, create, label).enabled, "{label}");
    }
    // In the editor with an insert marker, the first two run; Delete
    // Transient waits for a selected transient.
    let g = s.geometry();
    s.click_with(at_beat(&g, 6, 0.8), Modifiers::NONE);
    assert!(state(&s, create, "Insert Warp Marker").enabled);
    assert!(state(&s, create, "Insert Transient").enabled);
    assert!(!state(&s, create, "Delete Transient").enabled);
    let insert = state(&s, create, "Insert Transient").invocation.unwrap();
    let before = transients(&mut s).len();
    s.h.shell.invoke(&insert.id, insert.args).unwrap();
    assert_eq!(transients(&mut s).len(), before + 1);
    let added = s.run("clip_view.sample_editor", json!({}))["insert_marker"]["time"].clone();
    s.run("clip_view.select_transients", json!({ "times": [added] }));
    let delete = state(&s, create, "Delete Transient");
    assert!(delete.enabled);
    let invocation = delete.invocation.unwrap();
    s.h.shell.invoke(&invocation.id, invocation.args).unwrap();
    assert_eq!(transients(&mut s).len(), before);
    // Edit ▸ Quantize waits for audio quantise.
    let quantize = state(&s, edit, "Quantize");
    assert!(!quantize.enabled);
}

#[test]
fn follow_scrolls_the_editor_to_the_playhead_until_an_edit_pauses_it() {
    // Spec: sample-editor R11. The null device plays in real time, so the
    // test waits for the song to start.
    let mut s = sample("follow", true);
    let quarter = Ticks::QUARTER.get() as f64;
    let scrolls = |s: &mut Sample| -> Vec<Value> {
        s.h.take_events()
            .into_iter()
            .filter_map(|e| match e {
                bl_ui::main_window::UiEvent::Invoke(i)
                    if i.id == "clip_view.sample_view" && i.args["show"] == "page" =>
                {
                    Some(i.args)
                }
                _ => None,
            })
            .collect()
    };
    // Zoomed to the fourth bar, with the song about to play from the start.
    s.run(
        "clip_view.sample_view",
        json!({ "show": "span", "start": 12.0 * quarter, "end": 16.0 * quarter, "ticks": true }),
    );
    s.h.take_events();
    s.h.shell.run("transport.play", json!({})).unwrap();
    let begun = std::time::Instant::now();
    let mut turned = Vec::new();
    while turned.is_empty() && begun.elapsed() < std::time::Duration::from_secs(5) {
        std::thread::sleep(std::time::Duration::from_millis(10));
        s.h.frame(vec![]);
        turned.extend(scrolls(&mut s));
    }
    assert!(s.h.shell.playing(), "the song starts");
    let page = turned
        .first()
        .expect("the view turns the page to the playhead");
    assert!(page["start"].as_f64().unwrap() < 12.0 * quarter);
    // An edit in the editor pauses Follow: the view stays where it is.
    let clip = s.clip;
    s.run(
        "clip_view.sample_view",
        json!({ "show": "span", "start": 12.0 * quarter, "end": 16.0 * quarter, "ticks": true }),
    );
    s.run(
        "clip.insert_warp_marker",
        json!({ "clips": [clip], "time": 6.0 }),
    );
    assert!(s.h.shell.clip_view().sample().follow_paused());
    s.h.take_events();
    s.h.frame(vec![]);
    assert!(scrolls(&mut s).is_empty(), "paused");
    // Stopping the song lets it follow again.
    s.h.shell.run("transport.stop", json!({})).unwrap();
    let begun = std::time::Instant::now();
    while s.h.shell.playing() && begun.elapsed() < std::time::Duration::from_secs(5) {
        std::thread::sleep(std::time::Duration::from_millis(10));
        s.h.frame(vec![]);
    }
    assert!(!s.h.shell.playing());
    assert!(!s.h.shell.clip_view().sample().follow_paused());
}

#[test]
fn the_start_and_end_markers_and_the_loop_brace_drag_in_one_step() {
    // Spec: sample-editor R5 (piano-roll R21): the strip's drags run
    // `clip.set_region` and `clip.set_loop`.
    let mut s = marked("strip");
    let g = s.geometry();
    let quarter = Ticks::QUARTER.get() as f64;
    let steps = s.steps();
    let marker_y = g.strip.top() + 3.0;
    // The Start marker moves along the grid, in ticks of the contents.
    let from = pos2(g.axis.x_of(0.0) + 1.0, marker_y);
    let to = pos2(g.axis.x_of(4.0 * quarter) + 6.0, marker_y);
    s.drag(from, &Sample::path(from, to, 8), Modifiers::NONE);
    assert_eq!(s.audio().start, 4.0 * quarter);
    assert_eq!(s.steps(), steps + 1);
    // The End marker.
    let end = g.axis_of(f64::from(SECONDS));
    let from = pos2(g.axis.x_of(end) - 1.0, marker_y);
    let to = pos2(g.axis.x_of(12.0 * quarter), marker_y);
    s.drag(from, &Sample::path(from, to, 8), Modifiers::NONE);
    assert_eq!(s.audio().end, Some(12.0 * quarter));
    assert_eq!(s.steps(), steps + 2);
    // The brace's end changes its length, and its body moves it whole.
    let brace_y = g.strip.bottom() - 2.0;
    let from = pos2(g.axis.x_of(end) - 1.0, brace_y);
    let to = pos2(g.axis.x_of(10.0 * quarter), brace_y);
    s.drag(from, &Sample::path(from, to, 6), Modifiers::NONE);
    assert_eq!(s.audio().loop_end, Some(10.0 * quarter));
    let from = pos2(g.axis.x_of(5.0 * quarter), brace_y);
    let to = pos2(g.axis.x_of(7.0 * quarter), brace_y);
    s.drag(from, &Sample::path(from, to, 6), Modifiers::NONE);
    let audio = s.audio();
    assert_eq!(audio.loop_start, 2.0 * quarter);
    assert_eq!(audio.loop_end, Some(12.0 * quarter));
    assert_eq!(s.steps(), steps + 4);
    // Esc puts the clip back.
    let before = s.audio();
    let from = pos2(g.axis.x_of(4.0 * quarter), marker_y);
    s.h.frame(vec![Event::PointerMoved(from)]);
    s.h.frame(vec![Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    for at in Sample::path(from, from + egui::vec2(120.0, 0.0), 6) {
        s.h.frame(vec![Event::PointerMoved(at)]);
    }
    assert_ne!(s.audio().start, before.start);
    s.h.key(Key::Escape, Modifiers::NONE);
    s.h.frame(vec![Event::PointerButton {
        pos: from + egui::vec2(120.0, 0.0),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    s.h.settle();
    assert_eq!(s.audio(), before);
    assert_eq!(s.steps(), steps + 4);
}

#[test]
fn the_end_marker_drags_when_the_start_lies_past_the_file() {
    // Spec: sample-editor R5: a Start past the file's end, which an agent
    // can set, leaves the End marker draggable rather than crashing.
    let mut s = sample("past-end", false);
    let clip = s.clip;
    let past = f64::from(SECONDS) + 1.0;
    s.run(
        "clip.set_region",
        json!({ "clip": clip, "start": past, "end": null, "ticks": false }),
    );
    let g = s.geometry();
    let marker_y = g.strip.top() + 3.0;
    let from = pos2(g.axis.x_of(f64::from(SECONDS)) - 1.0, marker_y);
    let to = pos2(g.axis.x_of(f64::from(SECONDS) / 2.0), marker_y);
    s.drag(from, &Sample::path(from, to, 4), Modifiers::NONE);
    assert_eq!(s.audio().start, past);
}

#[test]
fn shift_dragging_a_pseudo_warp_marker_moves_its_transient() {
    // Spec: sample-editor R8.
    let mut s = sample("transient", true);
    let times = transients(&mut s);
    let g = s.geometry();
    let t = times[1];
    let steps = s.steps();
    let from = at_height(g.wave, g.x_of_time(t), 0.2);
    let to = from + egui::vec2(60.0, 0.0);
    s.drag(from, &Sample::path(from, to, 8), Modifiers::SHIFT);
    let now = transients(&mut s);
    assert_eq!(now.len(), times.len());
    assert!(
        now.iter().all(|x| (x - t).abs() > 1e-6),
        "the transient moved"
    );
    assert!(
        now.iter().any(|x| (x - g.time_at(to.x)).abs() < 0.02),
        "{now:?}"
    );
    assert_eq!(s.markers().len(), 1, "no audio changes");
    assert_eq!(s.steps(), steps + 1);
}

#[test]
fn the_other_arrow_keys_move_the_insert_marker_the_region_and_the_selection() {
    // Spec: sample-editor R10 (S-SAMPLE-006, S-A11Y-012, S-A11Y-013).
    let mut s = sample("arrows", true);
    s.run("arrangement.set_grid", json!({ "fixed": "quarter" }));
    let g = s.geometry();
    let quarter = Ticks::QUARTER.get();
    let alt = if cfg!(target_os = "macos") {
        Modifiers::ALT
    } else {
        Modifiers::CTRL | Modifiers::COMMAND
    };
    s.click_with(at_beat(&g, 4, 0.8), Modifiers::NONE);
    let insert = |s: &mut Sample| {
        s.run("clip_view.sample_editor", json!({}))["insert_marker"]["beat"].clone()
    };
    assert_eq!(insert(&mut s), 4 * quarter);
    // Option+Right and Option+Left (Ctrl on Windows) move the insert marker.
    s.h.key(Key::ArrowRight, alt);
    assert_eq!(insert(&mut s), 5 * quarter);
    s.h.key(Key::ArrowLeft, alt);
    s.h.key(Key::ArrowLeft, alt);
    assert_eq!(insert(&mut s), 3 * quarter);
    // Shift+Right starts a time selection at the insert marker, and again
    // extends it.
    s.h.key(Key::ArrowRight, Modifiers::SHIFT);
    s.h.key(Key::ArrowRight, Modifiers::SHIFT);
    let state = s.run("clip_view.sample_editor", json!({}));
    assert_eq!(state["time_selection"][0]["beat"], 3 * quarter);
    assert_eq!(state["time_selection"][1]["beat"], 5 * quarter);
    // With the Start marker selected, Shift+Right moves the region by a step.
    let strip_y = g.strip.top() + 3.0;
    s.click_with(pos2(g.axis.x_of(0.0) + 2.0, strip_y), Modifiers::NONE);
    assert!(s.h.shell.clip_view().sample().start_marker());
    let before = s.audio();
    s.h.key(Key::ArrowRight, Modifiers::SHIFT);
    let after = s.audio();
    assert_eq!(after.start, before.start + quarter as f64);
    assert_eq!(
        after.end, None,
        "an End marker at the file's end stays there"
    );
}

#[test]
fn the_wheel_scrolls_and_the_ruler_zooms() {
    // Spec: sample-editor R11.
    let mut s = sample("wheel", true);
    let quarter = Ticks::QUARTER.get() as f64;
    s.run(
        "clip_view.sample_view",
        json!({ "show": "span", "start": 4.0 * quarter, "end": 8.0 * quarter, "ticks": true }),
    );
    let g = s.geometry();
    let steps = s.steps();
    // The wheel moves the view and is not a zoom: X has nothing to go back to
    // but the span before.
    s.h.frame(vec![Event::PointerMoved(g.wave.center())]);
    s.h.frame(vec![Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(-100.0, 0.0),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    }]);
    s.h.settle();
    let scrolled = s.geometry();
    assert!(scrolled.axis.start > 4.0 * quarter, "{:?}", scrolled.axis);
    assert!((scrolled.axis.length - g.axis.length).abs() < 1.0);
    // Dragging down in the ruler zooms in about the pointer.
    let from = pos2(g.wave.center().x, g.ruler.center().y);
    let to = from + egui::vec2(0.0, 40.0);
    s.drag(from, &Sample::path(from, to, 8), Modifiers::NONE);
    let zoomed = s.geometry();
    assert!(
        zoomed.axis.length < scrolled.axis.length,
        "{:?}",
        zoomed.axis
    );
    assert_eq!(s.steps(), steps, "never an undo step");
}

#[test]
fn the_envelopes_tab_shows_an_audio_clips_envelopes() {
    // Spec: sample-editor R1. The Option+Tab key goes from the Sample tab to
    // the Envelopes tab and back.
    let mut s = sample("envelopes", true);
    let output = s.h.settle();
    let nodes = Harness::nodes(&output);
    assert!(nodes.iter().any(|n| n.label.starts_with("Waveform of")));
    assert!(nodes.iter().all(|n| n.label != "Clip Envelope"));
    s.run("view.clip_tab", json!({ "tab": "envelopes" }));
    let output = s.h.settle();
    let nodes = Harness::nodes(&output);
    Harness::node(&nodes, "Clip Envelope");
    assert!(nodes.iter().all(|n| !n.label.starts_with("Waveform of")));
    // The editor draws an envelope the clip has, from the clip's start.
    let clip = s.clip;
    s.run(
        "clip.set_envelope_breakpoint",
        json!({ "clip": clip, "target": { "type": "mixer", "param": "pan" }, "at": 0, "value": 0.25 }),
    );
    s.run(
        "view.clip_envelope",
        json!({ "target": { "type": "mixer", "param": "pan" } }),
    );
    s.h.settle();
    s.run("view.clip_tab", json!({}));
    let output = s.h.settle();
    assert!(
        Harness::nodes(&output)
            .iter()
            .any(|n| n.label.starts_with("Waveform of"))
    );
}

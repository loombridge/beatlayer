//! The Sample Editor (WP-4.10): the Clip View's editor for an audio clip, in
//! the MIDI Note Editor's place.
//!
//! It draws the clip's file as a waveform, one band per channel, along the
//! clip's beats when the clip is warped (each segment of the file stretched
//! between its two warp markers) and along time in the file when it isn't,
//! with the warp markers, the transients, a pseudo-warp marker under the
//! pointer, the insert marker and time selection, the Start and End markers
//! and the loop brace. People make, move and delete markers with the mouse
//! and with Live's Sample Editor keys; every one of those runs a registered
//! command ([`bl_shell::sample_editor_commands`] and the document's own
//! `clip.*` warp commands), so an agent does the same by ID. A drag is one
//! undo step however long it lasts, and Esc puts the clip back.
//!
//! Nothing here owns the state: the selections, the insert marker, the time
//! selection and the span in view are the shell's
//! ([`bl_shell::sample_editor::SampleView`]). Only the drag under way lives
//! here. A frame builds only what's in view (R22): the peaks of the visible
//! span, and the markers and transients inside it, found by binary search.

use std::sync::Arc;

use bl_commands::Context;
use bl_core::time::Ticks;
use bl_media::Peaks;
use bl_model::{AudioData, Clip, ClipData, ClipId, Document, TRANSIENT_MATCH, WarpMap};
use bl_shell::sample_editor_commands as ids;
use bl_shell::{GesturePhase, Invocation, Shell};
use egui::{
    Align2, Color32, CursorIcon, Id, Key, Mesh, Modifiers, Pos2, Rect, Sense, Shape, Stroke, Ui,
    pos2,
};
use serde_json::{Value, json};

use crate::arrangement::editing::EditKeys;
use crate::main_window::UiEvent;
use crate::theme::Theme;

/// How tall the time ruler is, in points (L-CLIP-011).
pub const RULER_HEIGHT: f32 = 14.0;
/// How tall the marker strip under the ruler is: the Start and End markers
/// and the loop brace.
pub const STRIP_HEIGHT: f32 = 16.0;
/// How tall the row of transient markers along the waveform's top is.
pub const TRANSIENT_ROW: f32 = 12.0;
/// How close to a marker's line, in points, a press grabs it.
pub const MARKER_GRAB: f32 = 4.0;
/// The shortest span the editor zooms to, in seconds of the file and in
/// ticks of a warped clip's contents.
const MIN_SPAN_SECONDS: f64 = 0.005;
const MIN_SPAN_TICKS: f64 = 1_000.0;
/// How wide a warp marker's handle is, in points.
const HANDLE: f32 = 7.0;
/// The ticks in a quarter note, as the document counts them.
const QUARTER: f64 = 705_600.0;
/// The fewest points between two grid lines the editor draws.
const GRID_GAP: f32 = 6.0;

/// The ruler under the Clip View's toolbar.
#[must_use]
pub fn ruler_rect(view: Rect) -> Rect {
    let top = view.top() + crate::clip_view::TOOLBAR_HEIGHT;
    Rect::from_min_max(
        pos2(view.left(), top),
        pos2(view.right(), top + RULER_HEIGHT),
    )
}

/// The marker strip under the ruler.
#[must_use]
pub fn strip_rect(view: Rect) -> Rect {
    let top = ruler_rect(view).bottom();
    Rect::from_min_max(
        pos2(view.left(), top),
        pos2(view.right(), top + STRIP_HEIGHT),
    )
}

/// The waveform's area, under the marker strip.
#[must_use]
pub fn wave_rect(view: Rect) -> Rect {
    let top = strip_rect(view).bottom();
    Rect::from_min_max(
        pos2(view.left(), top),
        pos2(view.right(), view.bottom().max(top)),
    )
}

/// The row of transient markers along the waveform's top.
#[must_use]
pub fn transient_rect(wave: Rect) -> Rect {
    Rect::from_min_max(
        wave.min,
        pos2(
            wave.right(),
            (wave.top() + TRANSIENT_ROW).min(wave.bottom()),
        ),
    )
}

/// The editor's axis: which value of the clip's axis sits at the waveform's
/// left edge, and how much of it the width shows. The value is a tick of a
/// warped clip's contents, or seconds in an unwarped clip's file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axis {
    /// The waveform's left edge, in points.
    pub left: f32,
    /// Its width, in points.
    pub width: f32,
    /// The value at the left edge.
    pub start: f64,
    /// How much of the axis the width shows.
    pub length: f64,
}

impl Axis {
    /// Where `value` falls, x.
    #[must_use]
    pub fn x_of(&self, value: f64) -> f32 {
        self.left + ((value - self.start) / self.length.max(f64::EPSILON)) as f32 * self.width
    }

    /// The value under `x`.
    #[must_use]
    pub fn value_at(&self, x: f32) -> f64 {
        self.start + f64::from(x - self.left) / f64::from(self.width.max(1.0)) * self.length
    }

    /// The value at the right edge.
    #[must_use]
    pub fn end(&self) -> f64 {
        self.start + self.length
    }
}

/// Where the editor puts things for one clip: its rectangles, its axis, and
/// the clip's warp map.
#[derive(Clone, Debug)]
pub struct Geometry {
    /// The time ruler.
    pub ruler: Rect,
    /// The marker strip.
    pub strip: Rect,
    /// The waveform's area.
    pub wave: Rect,
    /// The axis the waveform runs along.
    pub axis: Axis,
    /// Whether the clip is warped, so the axis is ticks of its contents.
    pub warped: bool,
    /// The length of the file, in seconds.
    pub file: f64,
    map: Option<WarpMap>,
}

impl Geometry {
    /// The geometry for `audio`, shown in the Clip View's `view`, with the
    /// span `span` in view (`None` for the whole file) and a file of `file`
    /// seconds.
    #[must_use]
    pub fn new(view: Rect, audio: &AudioData, file: f64, span: Option<(f64, f64)>) -> Self {
        let map = audio.warp.then(|| audio.warp_map().ok()).flatten();
        let wave = wave_rect(view);
        let mut geometry = Self {
            ruler: ruler_rect(view),
            strip: strip_rect(view),
            wave,
            axis: Axis {
                left: wave.left(),
                width: wave.width(),
                start: 0.0,
                length: 1.0,
            },
            warped: map.is_some(),
            file,
            map,
        };
        let (start, end) = span
            .filter(|(from, to)| to > from)
            .unwrap_or_else(|| geometry.whole());
        geometry.axis.start = start;
        geometry.axis.length = (end - start).max(f64::EPSILON);
        geometry
    }

    /// The whole file as an axis span.
    #[must_use]
    pub fn whole(&self) -> (f64, f64) {
        (self.axis_of(0.0), self.axis_of(self.file))
    }

    /// The axis value of a time in the file.
    #[must_use]
    pub fn axis_of(&self, time: f64) -> f64 {
        self.map.as_ref().map_or(time, |map| map.time_to_beat(time))
    }

    /// The time in the file at an axis value.
    #[must_use]
    pub fn time_of(&self, value: f64) -> f64 {
        self.map
            .as_ref()
            .map_or(value, |map| map.beat_to_time(value))
    }

    /// Where a time in the file falls, x.
    #[must_use]
    pub fn x_of_time(&self, time: f64) -> f32 {
        self.axis.x_of(self.axis_of(time))
    }

    /// The time in the file under `x`.
    #[must_use]
    pub fn time_at(&self, x: f32) -> f64 {
        self.time_of(self.axis.value_at(x))
    }

    /// The warp map, if the clip is warped.
    #[must_use]
    pub fn map(&self) -> Option<&WarpMap> {
        self.map.as_ref()
    }

    /// The smallest span the axis may be zoomed to.
    #[must_use]
    pub fn min_span(&self) -> f64 {
        if self.warped {
            MIN_SPAN_TICKS
        } else {
            MIN_SPAN_SECONDS
        }
    }
}

/// What a frame built, for the test of R22: the work a frame does follows
/// what's in view, not the size of the file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// The warp markers drawn.
    pub markers: usize,
    /// The transients drawn.
    pub transients: usize,
    /// The columns of the waveform read from the peaks.
    pub columns: usize,
}

/// The part of the marker strip a press grabbed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Start,
    End,
    LoopStart,
    LoopEnd,
    LoopBody,
}

/// The drag under way.
#[derive(Clone, Debug, PartialEq)]
enum Drag {
    /// A Start or End marker, or the loop brace, in the marker strip.
    Strip {
        clip: ClipId,
        part: Part,
        grab: f64,
        start: f64,
        end: Option<f64>,
        loop_start: f64,
        loop_end: Option<f64>,
        looping: bool,
    },
    /// Warp markers along the beats (R7).
    Markers {
        clip: ClipId,
        clips: Vec<ClipId>,
        places: Vec<u32>,
        /// Every marker's beat when the drag began.
        beats: Vec<i64>,
        grabbed: u32,
        last: Option<Invocation>,
    },
    /// The waveform under a warp marker (R7).
    Shift {
        clip: ClipId,
        clips: Vec<ClipId>,
        place: u32,
        grab: f64,
        map: WarpMap,
        beat: f64,
        last: Option<Invocation>,
    },
    /// A pseudo-warp marker made real by the drag (R8).
    Pseudo {
        clip: ClipId,
        time: f64,
        pin: bool,
        /// The new marker's place, once the frame that inserted it has run.
        place: Option<u32>,
        beats: Vec<i64>,
        last: Option<Invocation>,
    },
    /// A transient moved with Shift (R8).
    Transient {
        clip: ClipId,
        from: f64,
        last: Option<Invocation>,
    },
    /// A time selection (R9).
    Select { clip: ClipId, anchor: f64 },
    /// The ruler: dragging zooms and scrolls (R11).
    Ruler {
        clip: ClipId,
        anchor: f64,
        press: Pos2,
        span: (f64, f64),
    },
}

/// The Sample Editor's own state: the drag under way, and what the last
/// frame built.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SampleEditorUi {
    drag: Option<Drag>,
    stats: std::cell::Cell<Stats>,
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

fn gesture(events: &mut Vec<UiEvent>, phase: GesturePhase, invocation: Option<Invocation>) {
    events.push(UiEvent::Gesture { phase, invocation });
}

impl SampleEditorUi {
    /// What the last frame built.
    #[must_use]
    pub fn stats(&self) -> Stats {
        self.stats.get()
    }

    /// Forgets the drag, as showing another clip or the Envelopes tab does.
    pub fn reset(&mut self) {
        self.drag = None;
    }

    /// Whether a drag is under way.
    #[must_use]
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Draws the Sample Editor for `clip` in the Clip View's `view`, with the
    /// peaks of its file if they are ready, and turns the pointer's gestures
    /// and the keys into commands.
    #[expect(clippy::too_many_arguments, reason = "the editor's whole frame")]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        view: Rect,
        clip: ClipId,
        peaks: Option<Arc<Peaks>>,
        events: &mut Vec<UiEvent>,
    ) {
        let doc = shell.editor().document();
        let Some(value) = doc.clip(clip) else {
            return;
        };
        let ClipData::Audio(audio) = &value.data else {
            return;
        };
        let sample = shell.clip_view().sample();
        let own = sample.clip() == Some(clip);
        let file = shell
            .clip_file_seconds(clip)
            .or_else(|| {
                peaks
                    .as_ref()
                    .map(|p| p.frames() as f64 / f64::from(p.sample_rate().max(1)))
            })
            .unwrap_or(0.0);
        let span = own.then(|| sample.span()).flatten();
        let geometry = Geometry::new(view, audio, file, span);
        let colours = &theme.colors;
        ui.painter().rect_filled(geometry.wave, 0.0, colours.lane.0);
        self.stats.set(Stats::default());
        if geometry.wave.width() < 8.0 || geometry.wave.height() < 8.0 {
            return;
        }
        if file <= 0.0 {
            ui.painter().text(
                geometry.wave.center(),
                Align2::CENTER_CENTER,
                "The waveform isn't ready yet",
                theme.font(),
                colours.pending.0,
            );
        }
        let frame = Frame {
            shell,
            doc,
            theme,
            clip: value,
            audio,
            geometry: &geometry,
            transients: if file > 0.0 {
                shell.clip_transients(clip)
            } else {
                bl_shell::TransientList::default()
            },
            selected_markers: if own { sample.markers() } else { &[] },
            selected_transients: if own { sample.transients() } else { &[] },
            span_chosen: span.is_some(),
            start_selected: own && sample.start_marker(),
            insert: own.then(|| sample.insert_marker()).flatten(),
            selection: own.then(|| sample.time_selection()).flatten(),
        };
        // Follow: the view turns the page when the playhead leaves it.
        if let Some(scroll) = frame.follow_scroll(own && sample.follow_paused()) {
            invoke(
                events,
                ids::SAMPLE_VIEW,
                json!({
                    "clip": clip,
                    "show": "page",
                    "start": scroll.0,
                    "end": scroll.1,
                    "ticks": geometry.warped,
                }),
            );
        }
        self.paint(ui, &frame, peaks.as_deref());
        if file > 0.0 {
            self.input(ui, &frame, events);
        }
        self.accessibility(ui, &frame);
    }
}

/// Everything a frame of the editor reads.
struct Frame<'a> {
    shell: &'a Shell,
    doc: &'a Document,
    theme: &'a Theme,
    clip: &'a Clip,
    audio: &'a AudioData,
    geometry: &'a Geometry,
    transients: bl_shell::TransientList,
    selected_markers: &'a [u32],
    selected_transients: &'a [f64],
    /// Whether the span in view was chosen, rather than fitted to the whole
    /// file, which follows the warp map as it is edited.
    span_chosen: bool,
    start_selected: bool,
    insert: Option<f64>,
    selection: Option<(f64, f64)>,
}

impl Frame<'_> {
    fn g(&self) -> &Geometry {
        self.geometry
    }

    /// The editing grid's step in axis units: ticks of a warped clip's
    /// contents, and seconds at the song's tempo where the clip isn't warped.
    fn step(&self) -> f64 {
        let ticks = self.shell.arrangement().grid_ticks(self.doc).get() as f64;
        if self.g().warped {
            ticks
        } else {
            let bpm = self.doc.settings().tempo.bpm_at(self.clip.start).get();
            ticks / QUARTER * 60.0 / bpm
        }
    }

    /// Where something dragged to axis `value` lands: on the nearest grid
    /// line of a warped clip while snapping is on, and freely while it's off
    /// (Cmd — Alt on Windows — does the opposite). An unwarped clip has no
    /// grid to snap to.
    fn snap(&self, value: f64, bypass: bool) -> f64 {
        if !self.g().warped {
            return value;
        }
        let at = Ticks(value.round() as i64);
        self.shell
            .arrangement()
            .snap_drag(self.doc, at, bypass)
            .get() as f64
    }

    fn markers(&self) -> &[bl_model::WarpMarker] {
        if self.g().warped {
            &self.audio.warp_markers
        } else {
            &[]
        }
    }

    /// The warp markers in view, by place.
    fn visible_markers(&self) -> std::ops::Range<usize> {
        let markers = self.markers();
        if markers.is_empty() {
            return 0..0;
        }
        let margin =
            f64::from(HANDLE) / f64::from(self.g().axis.width.max(1.0)) * self.g().axis.length;
        let (from, to) = (self.g().axis.start - margin, self.g().axis.end() + margin);
        let first = markers.partition_point(|m| (m.beat.get() as f64) < from);
        let last = markers.partition_point(|m| (m.beat.get() as f64) <= to);
        first..last.max(first)
    }

    /// The transients in view, as indexes into the sorted list.
    fn visible_transients(&self) -> std::ops::Range<usize> {
        let g = self.g();
        let (from, to) = (
            g.time_at(g.wave.left() - 8.0),
            g.time_at(g.wave.right() + 8.0),
        );
        let first = self.transients.partition_point(|&t| t < from);
        let last = self.transients.partition_point(|&t| t <= to);
        first..last.max(first)
    }

    /// Whether a warp marker already sits on `time`.
    fn marked(&self, time: f64) -> bool {
        let markers = self.markers();
        let at = markers.partition_point(|m| m.time < time - TRANSIENT_MATCH);
        markers
            .get(at)
            .is_some_and(|m| (m.time - time).abs() <= TRANSIENT_MATCH)
    }

    /// The warp marker whose line is within the grab distance of `x`.
    fn marker_at(&self, x: f32) -> Option<usize> {
        self.visible_markers()
            .map(|place| {
                (
                    place,
                    (self.g().axis.x_of(self.markers()[place].beat.get() as f64) - x).abs(),
                )
            })
            .filter(|(_, distance)| *distance <= MARKER_GRAB)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(place, _)| place)
    }

    /// The transient without a warp marker whose line is within the grab
    /// distance of `x`: the one a pseudo-warp marker stands on.
    fn transient_at(&self, x: f32) -> Option<f64> {
        self.transients[self.visible_transients()]
            .iter()
            .copied()
            .filter(|&t| !self.marked(t))
            .map(|t| (t, (self.g().x_of_time(t) - x).abs()))
            .filter(|(_, distance)| *distance <= MARKER_GRAB)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(t, _)| t)
    }

    /// Which part of the marker strip is at `pos`.
    fn strip_part(&self, pos: Pos2) -> Option<Part> {
        let g = self.g();
        if !g.strip.contains(pos) {
            return None;
        }
        let x = |value: f64| g.axis.x_of(value);
        let near = |value: f64| (x(value) - pos.x).abs() <= MARKER_GRAB;
        let (loop_start, loop_end) = self.brace();
        if near(self.audio.start) {
            return Some(Part::Start);
        }
        if near(self.end()) {
            return Some(Part::End);
        }
        if near(loop_start) {
            return Some(Part::LoopStart);
        }
        if near(loop_end) {
            return Some(Part::LoopEnd);
        }
        (pos.x > x(loop_start) && pos.x < x(loop_end) && pos.y > g.strip.center().y)
            .then_some(Part::LoopBody)
    }

    /// The End marker, as an axis value.
    fn end(&self) -> f64 {
        self.audio
            .end
            .unwrap_or_else(|| self.g().axis_of(self.g().file))
    }

    /// The loop brace's ends, as axis values.
    fn brace(&self) -> (f64, f64) {
        (
            self.audio.loop_start,
            self.audio
                .loop_end
                .unwrap_or_else(|| self.g().axis_of(self.g().file)),
        )
    }

    /// Where the song's playhead falls in the clip's axis, if it is inside the
    /// clip: the beat of its contents for a warped clip, and the time in the
    /// file for an unwarped one, wrapped into the loop brace.
    fn playhead(&self) -> Option<f64> {
        let position = self.shell.arrangement().transport().position;
        if position < self.clip.start || position >= self.clip.end() {
            return None;
        }
        let into = if self.g().warped {
            (position - self.clip.start).get() as f64 + self.audio.start
        } else {
            let tempo = &self.doc.settings().tempo;
            tempo.ticks_to_seconds(position) - tempo.ticks_to_seconds(self.clip.start)
                + self.audio.start
        };
        let (from, to) = self.brace();
        Some(if self.audio.looping && into >= to && to > from {
            from + (into - from).rem_euclid(to - from)
        } else {
            into
        })
    }

    /// The span to scroll to so the playhead stays in view while the song
    /// plays and Follow is on, unless an edit or a scroll paused it (R11).
    fn follow_scroll(&self, paused: bool) -> Option<(f64, f64)> {
        let transport = self.shell.arrangement().transport();
        if !transport.playing || !self.shell.arrangement().follow() || paused {
            return None;
        }
        let at = self.playhead()?;
        let g = self.g();
        if at >= g.axis.start && at < g.axis.end() {
            return None;
        }
        Some((at, at + g.axis.length))
    }
}

/// The modifier keys the editor's gestures read.
#[derive(Clone, Copy, Debug)]
struct Keys {
    /// Cmd (Alt on Windows) does the opposite of the grid's snapping.
    bypass: bool,
    /// Cmd (Ctrl on Windows and Linux) adds to the selection.
    toggle: bool,
    /// Shift.
    shift: bool,
}

impl Keys {
    fn of(ui: &Ui) -> Self {
        let keys = EditKeys::from(ui.input(|i| i.modifiers));
        Self {
            bypass: keys.bypass,
            toggle: keys.toggle,
            shift: ui.input(|i| i.modifiers.shift),
        }
    }
}

/// A rectangle one pixel wide at `x` between `top` and `bottom`.
fn line(x: f32, top: f32, bottom: f32, width: f32) -> Rect {
    Rect::from_min_max(pos2(x - width / 2.0, top), pos2(x + width / 2.0, bottom))
}

/// Whole-number labels' spacing for the unwarped ruler, in seconds.
const SECOND_STEPS: [f64; 17] = [
    0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 300.0,
    600.0,
];

/// A time as the unwarped ruler reads it: minutes, seconds, and a fraction
/// where the zoom needs one.
fn time_label(seconds: f64, step: f64) -> String {
    let minutes = (seconds / 60.0).floor();
    let rest = seconds - minutes * 60.0;
    let places = if step < 0.01 {
        3
    } else if step < 0.1 {
        2
    } else if step < 1.0 {
        1
    } else {
        0
    };
    format!(
        "{}:{:0width$.places$}",
        minutes as i64,
        rest,
        width = if places > 0 { 3 + places } else { 2 }
    )
}

impl SampleEditorUi {
    fn paint(&self, ui: &Ui, frame: &Frame<'_>, peaks: Option<&Peaks>) {
        let g = frame.g();
        let colours = &frame.theme.colors;
        let painter = ui.painter();
        self.paint_ruler(ui, frame);
        self.paint_strip(ui, frame);
        let wave = painter.with_clip_rect(g.wave);
        if frame.g().warped {
            self.paint_grid(ui, frame);
        }
        let mut stats = self.stats.get();
        if let Some(peaks) = peaks {
            stats.columns = paint_waveform(&wave, frame, peaks);
        }
        // The time selection and the insert marker.
        if let Some((from, to)) = frame.selection {
            let rect = Rect::from_min_max(
                pos2(g.x_of_time(from), g.wave.top()),
                pos2(g.x_of_time(to), g.wave.bottom()),
            )
            .intersect(g.wave);
            wave.rect_filled(rect, 0.0, colours.time_selection.0.gamma_multiply(0.25));
        }
        // Transients along the top.
        let row = transient_rect(g.wave);
        let visible = frame.visible_transients();
        stats.transients = visible.len();
        for &time in &frame.transients[visible] {
            let x = g.x_of_time(time);
            let picked = frame
                .selected_transients
                .iter()
                .any(|&t| (t - time).abs() <= TRANSIENT_MATCH);
            wave.rect_filled(
                line(x, row.top(), row.bottom() - 2.0, 1.0),
                0.0,
                if picked {
                    colours.time_selection.0
                } else {
                    colours.text_dim.0
                },
            );
        }
        // A pseudo-warp marker under the pointer.
        if self.drag.is_none()
            && let Some(hover) = ui.input(|i| i.pointer.hover_pos())
            && g.wave.contains(hover)
            && hover.y < g.wave.center().y
            && frame.marker_at(hover.x).is_none()
            && let Some(time) = frame.transient_at(hover.x)
        {
            wave.rect_filled(
                line(g.x_of_time(time), g.wave.top(), g.wave.bottom(), 1.0),
                0.0,
                colours.accent.0.gamma_multiply(0.5),
            );
        }
        // The warp markers: handles reaching down from the top.
        stats.markers = frame.visible_markers().len();
        self.stats.set(stats);
        for place in frame.visible_markers() {
            let marker = &frame.markers()[place];
            let x = g.axis.x_of(marker.beat.get() as f64);
            let colour = if frame.selected_markers.contains(&(place as u32)) {
                colours.time_selection.0
            } else {
                colours.accent.0
            };
            wave.rect_filled(line(x, g.wave.top(), g.wave.bottom(), 1.0), 0.0, colour);
            wave.add(Shape::convex_polygon(
                vec![
                    pos2(x - HANDLE / 2.0, g.wave.top()),
                    pos2(x + HANDLE / 2.0, g.wave.top()),
                    pos2(x, g.wave.top() + HANDLE),
                ],
                colour,
                Stroke::NONE,
            ));
        }
        // The insert marker, and the playhead.
        if let Some(time) = frame.insert {
            wave.rect_filled(
                line(g.x_of_time(time), g.wave.top(), g.wave.bottom(), 1.0),
                0.0,
                colours.insert_marker.0,
            );
        }
        if frame.shell.arrangement().transport().playing
            && let Some(at) = frame.playhead()
        {
            wave.rect_filled(
                line(g.axis.x_of(at), g.wave.top(), g.wave.bottom(), 1.0),
                0.0,
                colours.playhead.0,
            );
        }
    }

    /// The editing grid's lines over a warped clip's waveform.
    fn paint_grid(&self, ui: &Ui, frame: &Frame<'_>) {
        let g = frame.g();
        let painter = ui.painter().with_clip_rect(g.wave);
        let colours = &frame.theme.colors;
        let per_tick = f64::from(g.axis.width) / g.axis.length;
        let mut step = frame.shell.arrangement().grid_step(frame.doc);
        if (step.ticks.get() as f64 * per_tick) < f64::from(GRID_GAP) {
            // Too close to tell apart: bars only.
            step.ticks = frame
                .doc
                .settings()
                .time_signatures
                .signature_at(frame.clip.start)
                .bar_length();
            step.bars = Some(1);
        }
        let from = Ticks(g.axis.start.floor() as i64);
        let to = Ticks(g.axis.end().ceil() as i64);
        let px = (ui.ctx().pixels_per_point() * 0.5).max(0.5);
        bl_shell::arrangement::grid::lines(
            &frame.doc.settings().time_signatures,
            step,
            from,
            to,
            4_000,
            |at, bar| {
                let x = g.axis.x_of(at.get() as f64);
                painter.rect_filled(
                    Rect::from_min_max(pos2(x, g.wave.top()), pos2(x + px, g.wave.bottom())),
                    0.0,
                    if bar {
                        colours.grid_bar.0
                    } else {
                        colours.grid_beat.0
                    },
                );
            },
        );
    }

    fn paint_ruler(&self, ui: &Ui, frame: &Frame<'_>) {
        let g = frame.g();
        let colours = &frame.theme.colors;
        let painter = ui.painter().with_clip_rect(g.ruler);
        painter.rect_filled(g.ruler, 0.0, colours.ruler.0);
        let font = frame.theme.small_font();
        if g.warped {
            // Bars and beats, labelled where they have room (L-CLIP-011).
            let signature = frame
                .doc
                .settings()
                .time_signatures
                .signature_at(frame.clip.start);
            let bar = signature.bar_length().get() as f64;
            let per_bar = f64::from(g.axis.width) / g.axis.length * bar;
            let every = (60.0 / per_bar.max(1e-6)).ceil().max(1.0) as i64;
            let first = (g.axis.start / bar).floor() as i64;
            let last = (g.axis.end() / bar).ceil() as i64;
            let mut index = first.div_euclid(every) * every;
            while index <= last {
                let x = g.axis.x_of(index as f64 * bar);
                painter.rect_filled(
                    line(x, g.ruler.top() + 8.0, g.ruler.bottom(), 1.0),
                    0.0,
                    colours.ruler_tick.0,
                );
                // There is no bar 0: the bar before 1 is -1.
                let number = if index >= 0 { index + 1 } else { index };
                painter.text(
                    pos2(x + 3.0, g.ruler.top() + 1.0),
                    Align2::LEFT_TOP,
                    number.to_string(),
                    font.clone(),
                    colours.text_dim.0,
                );
                index += every;
            }
        } else {
            let per_second = f64::from(g.axis.width) / g.axis.length;
            let step = SECOND_STEPS
                .iter()
                .copied()
                .find(|s| s * per_second >= 60.0)
                .unwrap_or(600.0);
            let mut at = (g.axis.start / step).floor() * step;
            while at <= g.axis.end() {
                let x = g.axis.x_of(at);
                painter.rect_filled(
                    line(x, g.ruler.top() + 8.0, g.ruler.bottom(), 1.0),
                    0.0,
                    colours.ruler_tick.0,
                );
                painter.text(
                    pos2(x + 3.0, g.ruler.top() + 1.0),
                    Align2::LEFT_TOP,
                    time_label(at.max(0.0), step),
                    font.clone(),
                    colours.text_dim.0,
                );
                at += step;
            }
        }
        painter.line_segment(
            [g.ruler.left_bottom(), g.ruler.right_bottom()],
            Stroke::new(1.0, colours.separator.0),
        );
    }

    fn paint_strip(&self, ui: &Ui, frame: &Frame<'_>) {
        let g = frame.g();
        let colours = &frame.theme.colors;
        let painter = ui.painter().with_clip_rect(g.strip);
        painter.rect_filled(g.strip, 0.0, colours.ruler.0);
        let flag = |value: f64, selected: bool| {
            let x = g.axis.x_of(value);
            let colour = if selected {
                colours.time_selection.0
            } else {
                colours.insert_marker.0
            };
            painter.line_segment(
                [pos2(x, g.strip.top()), pos2(x, g.strip.bottom())],
                Stroke::new(1.0, colour),
            );
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(x - 2.5, g.strip.top()),
                    pos2(x + 2.5, g.strip.top() + 5.0),
                ),
                0.0,
                colour,
            );
        };
        flag(frame.audio.start, frame.start_selected);
        flag(frame.end(), false);
        let (from, to) = frame.brace();
        let brace = Rect::from_min_max(
            pos2(g.axis.x_of(from), g.strip.bottom() - 6.0),
            pos2(g.axis.x_of(to), g.strip.bottom()),
        );
        painter.rect_filled(
            brace,
            1.0,
            if frame.audio.looping {
                colours.loop_on.0
            } else {
                colours.loop_off.0
            },
        );
        painter.line_segment(
            [g.strip.left_bottom(), g.strip.right_bottom()],
            Stroke::new(1.0, colours.separator.0),
        );
    }
}

/// Draws the waveform: for each column of points, the lowest and highest
/// sample of the frames it covers, read from the peaks, one band per channel;
/// the file outside the clip's region is drawn dimmed (R4).
fn paint_waveform(painter: &egui::Painter, frame: &Frame<'_>, peaks: &Peaks) -> usize {
    let g = frame.g();
    let colours = &frame.theme.colors;
    let channels = usize::from(peaks.channels().max(1));
    let rate = f64::from(peaks.sample_rate());
    let band = g.wave.height() / channels as f32;
    let top_pad = TRANSIENT_ROW.min(band / 4.0);
    // The region that plays is bright, the rest of the file dimmed.
    let region_start = if frame.audio.looping {
        frame.audio.start.min(frame.audio.loop_start)
    } else {
        frame.audio.start
    };
    let region_end = if frame.audio.looping {
        frame.brace().1
    } else {
        frame.end()
    };
    let (lit_from, lit_to) = (g.time_of(region_start), g.time_of(region_end));
    let bright = colours.waveform.0;
    let dim = colours.waveform.0.gamma_multiply(0.35);
    let width = g.wave.width().ceil() as usize;
    // Columns first, to fit the height to what is in view if asked.
    let mut columns: Vec<(Option<(i8, i8)>, bool)> = Vec::new();
    let mut loudest = 0i8;
    for channel in 0..channels {
        for column in 0..width {
            let x = g.wave.left() + column as f32;
            let (t0, t1) = (g.time_at(x), g.time_at(x + 1.0));
            let (t0, t1) = (t0.min(t1), t0.max(t1));
            let range = if t1 <= 0.0 || t0 >= g.file {
                None
            } else {
                let from = (t0.max(0.0) * rate).floor() as u64;
                let to = ((t1 * rate).ceil() as u64).max(from + 1);
                peaks.min_max(channel, from..to)
            };
            let lit = (t0 + t1) / 2.0 >= lit_from && (t0 + t1) / 2.0 <= lit_to;
            if let Some([lo, hi]) = range {
                loudest = loudest.max(hi).max(lo.saturating_neg());
            }
            columns.push((range.map(|[lo, hi]| (lo, hi)), lit));
        }
    }
    let read = columns.iter().filter(|c| c.0.is_some()).count();
    let sample = frame.shell.clip_view().sample();
    let scale = if sample.clip() == Some(frame.clip.id) && sample.fit_height() && loudest > 0 {
        127.0 / f32::from(loudest)
    } else {
        1.0
    };
    let mut mesh = Mesh::default();
    for channel in 0..channels {
        let mid = g.wave.top() + band * channel as f32 + band / 2.0 + top_pad / 2.0;
        let half = (band / 2.0 - top_pad / 2.0).max(1.0);
        for column in 0..width {
            let Some(((lo, hi), lit)) = columns[channel * width + column]
                .0
                .map(|range| (range, columns[channel * width + column].1))
            else {
                continue;
            };
            let x = g.wave.left() + column as f32;
            let y0 = mid - (f32::from(hi) / 127.0 * scale).min(1.0) * half;
            let y1 = mid - (f32::from(lo) / 127.0 * scale).max(-1.0) * half;
            let colour: Color32 = if lit { bright } else { dim };
            mesh.add_colored_rect(
                Rect::from_min_max(pos2(x, y0), pos2(x + 1.0, y1.max(y0 + 1.0))),
                colour,
            );
        }
    }
    painter.add(Shape::mesh(mesh));
    read
}

impl SampleEditorUi {
    /// The pointer's gestures and the editor's keys.
    #[expect(clippy::too_many_lines, reason = "one pass over the editor's input")]
    fn input(&mut self, ui: &mut Ui, frame: &Frame<'_>, events: &mut Vec<UiEvent>) {
        let g = frame.g();
        let clip = frame.clip.id;
        let keys = Keys::of(ui);
        let wave_id = Id::new(("sample-editor-wave", clip));
        let response = ui.interact(g.wave, wave_id, Sense::click_and_drag());
        let strip = ui.interact(
            g.strip,
            Id::new(("sample-editor-strip", clip)),
            Sense::click_and_drag(),
        );
        let ruler = ui.interact(
            g.ruler,
            Id::new(("sample-editor-ruler", clip)),
            Sense::click_and_drag(),
        );
        let pointer = response
            .interact_pointer_pos()
            .or_else(|| strip.interact_pointer_pos())
            .or_else(|| ruler.interact_pointer_pos())
            .or_else(|| ui.input(|i| i.pointer.interact_pos()));
        let press = ui.input(|i| i.pointer.press_origin());

        // Esc puts everything back.
        if self.drag.is_some() && ui.input(|i| i.key_pressed(Key::Escape)) {
            if !matches!(self.drag, Some(Drag::Select { .. } | Drag::Ruler { .. })) {
                gesture(events, GesturePhase::Cancel, None);
            }
            self.drag = None;
        }

        // The ruler: dragging up and down zooms about the pointer, and
        // left and right scrolls (R11).
        if ruler.drag_started()
            && let Some(press) = press
        {
            self.drag = Some(Drag::Ruler {
                clip,
                anchor: g.axis.value_at(press.x),
                press,
                span: (g.axis.start, g.axis.end()),
            });
        }
        // The marker strip: the Start and End markers and the loop brace.
        if strip.drag_started()
            && let Some(press) = press
            && let Some(part) = frame.strip_part(press)
        {
            Self::keep_view(frame, events);
            let (loop_start, _) = frame.brace();
            self.drag = Some(Drag::Strip {
                clip,
                part,
                grab: g.axis.value_at(press.x),
                start: frame.audio.start,
                end: frame.audio.end,
                loop_start,
                loop_end: frame.audio.loop_end,
                looping: frame.audio.looping,
            });
        }
        if strip.clicked()
            && let Some(pos) = strip.interact_pointer_pos()
        {
            match frame.strip_part(pos) {
                // Clicking the Start marker selects it (R10).
                Some(Part::Start) => invoke(
                    events,
                    ids::SELECT_WARP_MARKERS,
                    json!({ "clip": clip, "markers": [], "start_marker": true }),
                ),
                Some(_) => {}
                // The scrub area: the song goes to that point (R9).
                None => self.scrub(frame, pos, events),
            }
        }
        // The waveform.
        if response.drag_started()
            && let Some(press) = press
        {
            self.begin_wave_drag(frame, press, keys, events);
        }
        if let Some(pos) = pointer {
            self.update_drag(frame, pos, keys, ui, events);
        }
        let released = response.drag_stopped() || strip.drag_stopped() || ruler.drag_stopped();
        if released && let Some(drag) = self.drag.take() {
            Self::finish_drag(&drag, events);
        }
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            self.wave_click(frame, pos, keys, events);
        }
        if response.double_clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            self.wave_double_click(frame, pos, keys, events);
        }
        // Cursors.
        if let Some(hover) = response.hover_pos() {
            if frame.marker_at(hover.x).is_some() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            } else if hover.y < g.wave.center().y && frame.transient_at(hover.x).is_some() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
        }
        if let Some(hover) = strip.hover_pos()
            && frame.strip_part(hover).is_some()
        {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }
        // The wheel scrolls (R11).
        if ui.rect_contains_pointer(g.wave) || ui.rect_contains_pointer(g.ruler) {
            let scroll = ui.input(|i| i.smooth_scroll_delta);
            let by = if scroll.x != 0.0 { scroll.x } else { scroll.y };
            if by != 0.0 {
                let delta = -f64::from(by) / f64::from(g.axis.width.max(1.0)) * g.axis.length;
                self.show_span(
                    frame,
                    g.axis.start + delta,
                    g.axis.end() + delta,
                    true,
                    events,
                );
            }
        }
        self.context_menu(ui, &response, frame, events);
        self.keys(ui, frame, events);
    }

    /// Asks for the span `start` to `end`: scrolling, or a zoom.
    fn show_span(
        &self,
        frame: &Frame<'_>,
        start: f64,
        end: f64,
        scroll: bool,
        events: &mut Vec<UiEvent>,
    ) {
        let g = frame.g();
        let (whole_start, whole_end) = g.whole();
        // The view may not leave the file's span by more than its own width.
        let length = (end - start).clamp(
            g.min_span(),
            (16.0 * (whole_end - whole_start)).max(g.min_span()),
        );
        let start = start.clamp(whole_start - length, whole_end);
        invoke(
            events,
            ids::SAMPLE_VIEW,
            json!({
                "clip": frame.clip.id,
                "show": if scroll { "scroll" } else { "span" },
                "start": start,
                "end": start + length,
                "ticks": g.warped,
            }),
        );
    }

    /// A click in the scrub area: the song goes to where that point of the
    /// clip plays, by steps of the global launch quantisation while it plays
    /// (R9).
    fn scrub(&self, frame: &Frame<'_>, pos: Pos2, events: &mut Vec<UiEvent>) {
        let g = frame.g();
        let value = g.axis.value_at(pos.x);
        let clip = frame.clip;
        let tempo = &frame.doc.settings().tempo;
        let mut at = if g.warped {
            clip.start + Ticks((value - frame.audio.start).round() as i64)
        } else {
            tempo.seconds_to_ticks(tempo.ticks_to_seconds(clip.start) + (value - frame.audio.start))
        };
        let settings = frame.shell.launch_settings();
        if frame.shell.arrangement().transport().playing && settings.enabled {
            let bar = frame
                .doc
                .settings()
                .time_signatures
                .signature_at(at)
                .bar_length();
            if let Some(step) = settings.quantization.ticks(bar)
                && step.get() > 0
            {
                at = Ticks((at.get() as f64 / step.get() as f64).round() as i64 * step.get());
            }
        }
        // Reaching a point before the clip's start isn't a position.
        let at = at.max(Ticks::ZERO);
        invoke(events, "transport.locate", json!({ "position": at }));
        // Spec: sample-editor R11. A click in the scrub area resumes Follow.
        invoke(events, ids::SAMPLE_VIEW, json!({ "clip": clip.id }));
    }

    /// Keeps the span in view where it is while a drag edits the clip: a
    /// view fitted to the whole file would follow the warp map under the
    /// pointer as the drag goes.
    fn keep_view(frame: &Frame<'_>, events: &mut Vec<UiEvent>) {
        if !frame.span_chosen {
            let g = frame.g();
            invoke(
                events,
                ids::SAMPLE_VIEW,
                json!({
                    "clip": frame.clip.id,
                    "show": "scroll",
                    "start": g.axis.start,
                    "end": g.axis.end(),
                    "ticks": g.warped,
                }),
            );
        }
    }

    /// A press that turns into a drag on the waveform.
    fn begin_wave_drag(
        &mut self,
        frame: &Frame<'_>,
        press: Pos2,
        keys: Keys,
        events: &mut Vec<UiEvent>,
    ) {
        let g = frame.g();
        let clip = frame.clip.id;
        let upper = press.y < g.wave.center().y;
        if let Some(place) = frame.marker_at(press.x) {
            Self::keep_view(frame, events);
            let place_u = place as u32;
            let shift = keys.shift;
            // Dragging a marker that isn't selected selects it alone.
            let mut places: Vec<u32> = frame.selected_markers.to_vec();
            if !places.contains(&place_u) {
                places = vec![place_u];
                invoke(
                    events,
                    ids::SELECT_WARP_MARKERS,
                    json!({ "clip": clip, "markers": places }),
                );
            }
            let clips = frame.shell.sample_edit_clips(clip, &places);
            if shift {
                // Shift-dragging moves the waveform under the marker (R7).
                let Some(map) = g.map().cloned() else {
                    return;
                };
                self.drag = Some(Drag::Shift {
                    clip,
                    clips: frame.shell.sample_edit_clips(clip, &[place_u]),
                    place: place_u,
                    grab: g.axis.value_at(press.x),
                    beat: frame.markers()[place].beat.get() as f64,
                    map,
                    last: None,
                });
            } else {
                self.drag = Some(Drag::Markers {
                    clip,
                    clips,
                    places,
                    beats: frame.markers().iter().map(|m| m.beat.get()).collect(),
                    grabbed: place_u,
                    last: None,
                });
            }
            return;
        }
        if upper && let Some(time) = frame.transient_at(press.x) {
            Self::keep_view(frame, events);
            if keys.shift {
                self.drag = Some(Drag::Transient {
                    clip,
                    from: time,
                    last: None,
                });
            } else if g.warped {
                self.drag = Some(Drag::Pseudo {
                    clip,
                    time,
                    pin: keys.toggle,
                    place: None,
                    beats: Vec::new(),
                    last: None,
                });
            }
            return;
        }
        // Anywhere else a drag selects a span of the clip (R9).
        self.drag = Some(Drag::Select {
            clip,
            anchor: g.time_at(press.x),
        });
    }

    /// Runs a frame of the drag under way with the pointer at `pos`.
    fn update_drag(
        &mut self,
        frame: &Frame<'_>,
        pos: Pos2,
        keys: Keys,
        ui: &Ui,
        events: &mut Vec<UiEvent>,
    ) {
        if !ui.input(|i| i.pointer.is_decidedly_dragging())
            && !ui.input(|i| i.pointer.primary_down())
        {
            return;
        }
        let g = frame.g();
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        match drag {
            Drag::Strip {
                clip,
                part,
                grab,
                start,
                end,
                loop_start,
                loop_end,
                looping,
            } => {
                let value = frame.snap(g.axis.value_at(pos.x), keys.bypass);
                let delta = value - frame.snap(*grab, keys.bypass);
                let file_end = g.axis_of(g.file);
                let ceiling = file_end.max(*grab);
                let floor = g.axis_of(0.0);
                let end_now = end.unwrap_or(file_end);
                let loop_end_now = loop_end.unwrap_or(file_end);
                let ticks = g.warped;
                let min = if ticks { 1.0 } else { 0.001 };
                let (id, args) = match part {
                    Part::Start => {
                        let mut upper = end_now;
                        if *looping {
                            upper = upper.min(loop_end_now);
                        }
                        let start = (*start + delta).clamp(floor, (upper - min).max(floor));
                        (
                            "clip.set_region",
                            json!({ "clip": clip, "start": start, "end": end, "ticks": ticks }),
                        )
                    }
                    Part::End => {
                        let end = (end_now + delta).clamp(*start + min, ceiling.max(*start + min));
                        (
                            "clip.set_region",
                            json!({ "clip": clip, "start": start, "end": end, "ticks": ticks }),
                        )
                    }
                    Part::LoopStart => {
                        let from =
                            (*loop_start + delta).clamp(floor, (loop_end_now - min).max(floor));
                        (
                            "clip.set_loop",
                            json!({ "clip": clip, "start": from, "end": loop_end, "ticks": ticks }),
                        )
                    }
                    Part::LoopEnd => {
                        let to = (loop_end_now + delta)
                            .clamp(*loop_start + min, ceiling.max(*loop_start + min));
                        (
                            "clip.set_loop",
                            json!({ "clip": clip, "start": loop_start, "end": to, "ticks": ticks }),
                        )
                    }
                    Part::LoopBody => {
                        let shift = delta.max(floor - *loop_start).min(file_end - loop_end_now);
                        (
                            "clip.set_loop",
                            json!({
                                "clip": clip,
                                "start": *loop_start + shift,
                                "end": loop_end_now + shift,
                                "ticks": ticks,
                            }),
                        )
                    }
                };
                gesture(
                    events,
                    GesturePhase::Update,
                    Some(Invocation::new(id, args)),
                );
            }
            Drag::Markers {
                clips,
                places,
                beats,
                grabbed,
                last,
                ..
            } => {
                let target = frame.snap(g.axis.value_at(pos.x), keys.bypass);
                let Some(&grabbed_beat) = beats.get(*grabbed as usize) else {
                    return;
                };
                let by = bounded_move(beats, places, target.round() as i64 - grabbed_beat);
                let invocation = Invocation::new(
                    "clip.move_warp_markers",
                    json!({ "clips": clips, "markers": places, "by": by }),
                );
                gesture(events, GesturePhase::Update, Some(invocation.clone()));
                *last = Some(invocation);
            }
            Drag::Shift {
                clips,
                place,
                grab,
                map,
                beat,
                last,
                ..
            } => {
                // The waveform follows the pointer, so the marker's time in
                // the file moves the other way (R7).
                let moved = g.axis.value_at(pos.x) - *grab;
                let by = -(map.beat_to_time(*beat + moved) - map.beat_to_time(*beat));
                let invocation = Invocation::new(
                    "clip.shift_warp_marker",
                    json!({ "clips": clips, "marker": place, "by": by }),
                );
                gesture(events, GesturePhase::Update, Some(invocation.clone()));
                *last = Some(invocation);
            }
            Drag::Pseudo {
                clip,
                time,
                pin,
                place,
                beats,
                last,
            } => {
                match *place {
                    None => {
                        if last.is_none() {
                            // The first frame makes the marker real.
                            let invocation = Invocation::new(
                                "clip.insert_warp_marker",
                                json!({
                                    "clips": [clip],
                                    "time": time,
                                    "pin_neighbours": pin,
                                }),
                            );
                            gesture(events, GesturePhase::Update, Some(invocation.clone()));
                            *last = Some(invocation);
                        } else if let Some(at) = frame
                            .audio
                            .warp_markers
                            .iter()
                            .position(|m| (m.time - *time).abs() <= 1e-9)
                        {
                            // The marker is in the set now: take its place
                            // and every marker's beat, and move it.
                            *place = Some(at as u32);
                            *beats = frame
                                .audio
                                .warp_markers
                                .iter()
                                .map(|m| m.beat.get())
                                .collect();
                        }
                    }
                    Some(at) => {
                        let target = frame.snap(g.axis.value_at(pos.x), keys.bypass);
                        let grabbed = beats.get(at as usize).copied().unwrap_or_default();
                        let by = bounded_move(beats, &[at], target.round() as i64 - grabbed);
                        let invocation = Invocation::new(
                            "clip.move_warp_markers",
                            json!({ "clips": [clip], "markers": [at], "by": by }),
                        );
                        gesture(events, GesturePhase::Update, Some(invocation.clone()));
                        *last = Some(invocation);
                    }
                }
            }
            Drag::Transient { clip, from, last } => {
                let to = g.time_at(pos.x).max(0.0).min(g.file);
                let invocation = Invocation::new(
                    "clip.move_transient",
                    json!({ "clip": clip, "from": from, "to": to }),
                );
                gesture(events, GesturePhase::Update, Some(invocation.clone()));
                *last = Some(invocation);
            }
            Drag::Select { clip, anchor } => {
                let here = g.time_at(pos.x);
                let (from, to) = (anchor.min(here), anchor.max(here));
                let (from, to) = if g.warped {
                    let snap = |t: f64| g.time_of(frame.snap(g.axis_of(t), keys.bypass));
                    (snap(from), snap(to))
                } else {
                    (from, to)
                };
                if to > from {
                    invoke(
                        events,
                        ids::SELECT_TIME,
                        json!({ "clip": clip, "start": from.max(0.0), "end": to.min(g.file) }),
                    );
                }
            }
            Drag::Ruler {
                anchor,
                press,
                span,
                ..
            } => {
                let dx = f64::from(pos.x - press.x) / f64::from(g.axis.width.max(1.0));
                let dy = f64::from(pos.y - press.y);
                // Dragging down zooms in, and up zooms out, about the point
                // that was pressed.
                let length = ((span.1 - span.0) * (-dy * 0.01).exp()).max(g.min_span());
                let fraction = (*anchor - span.0) / (span.1 - span.0).max(f64::EPSILON);
                let start = *anchor - fraction * length - dx * length;
                self.show_span(frame, start, start + length, true, events);
            }
        }
    }

    /// The drag ends: the last frame of one that edits the clip stands as
    /// one undo step. Selecting and zooming only changed the view.
    fn finish_drag(drag: &Drag, events: &mut Vec<UiEvent>) {
        let last = match drag {
            Drag::Select { .. } | Drag::Ruler { .. } => return,
            Drag::Strip { .. } => return gesture(events, GesturePhase::Commit, None),
            Drag::Markers { last, .. }
            | Drag::Shift { last, .. }
            | Drag::Pseudo { last, .. }
            | Drag::Transient { last, .. } => last.clone(),
        };
        if last.is_some() {
            gesture(events, GesturePhase::Commit, last);
        } else {
            gesture(events, GesturePhase::Cancel, None);
        }
    }

    /// A click on the waveform: it selects a marker or a transient, or puts
    /// the insert marker where it fell (R6, R9).
    fn wave_click(&self, frame: &Frame<'_>, pos: Pos2, keys: Keys, events: &mut Vec<UiEvent>) {
        let g = frame.g();
        let clip = frame.clip.id;
        let mode = if keys.toggle { "toggle" } else { "replace" };
        if let Some(place) = frame.marker_at(pos.x) {
            invoke(
                events,
                ids::SELECT_WARP_MARKERS,
                json!({ "clip": clip, "markers": [place], "mode": mode }),
            );
            return;
        }
        if pos.y < g.wave.center().y
            && let Some(time) = frame.transient_at(pos.x)
        {
            invoke(
                events,
                ids::SELECT_TRANSIENTS,
                json!({ "clip": clip, "times": [time], "mode": mode }),
            );
            return;
        }
        let value = frame.snap(g.axis.value_at(pos.x), keys.bypass);
        let time = g.time_of(value).clamp(0.0, g.file);
        if keys.shift {
            // Shift-click extends the span from the insert marker or the
            // selection's far end (R9).
            let anchor = frame
                .insert
                .or(frame.selection.map(|(from, _)| from))
                .unwrap_or(0.0);
            let (from, to) = (anchor.min(time), anchor.max(time));
            if to > from {
                invoke(
                    events,
                    ids::SELECT_TIME,
                    json!({ "clip": clip, "start": from, "end": to }),
                );
                return;
            }
        }
        // A click away from a marker clears the marker and transient
        // selections, and puts the insert marker there.
        invoke(
            events,
            ids::SELECT_WARP_MARKERS,
            json!({ "clip": clip, "markers": [] }),
        );
        invoke(
            events,
            ids::SELECT_TRANSIENTS,
            json!({ "clip": clip, "times": [] }),
        );
        invoke(
            events,
            ids::SET_INSERT_MARKER,
            json!({ "clip": clip, "time": time }),
        );
    }

    /// A double-click: on a warp marker it deletes it, on a pseudo-warp
    /// marker it makes it real, and in the upper half elsewhere it makes a
    /// warp marker there (R6, R8).
    fn wave_double_click(
        &self,
        frame: &Frame<'_>,
        pos: Pos2,
        keys: Keys,
        events: &mut Vec<UiEvent>,
    ) {
        let g = frame.g();
        let clip = frame.clip.id;
        if !g.warped {
            return;
        }
        if let Some(place) = frame.marker_at(pos.x) {
            invoke(
                events,
                "clip.delete_warp_markers",
                json!({
                    "clips": frame.shell.sample_edit_clips(clip, &[place as u32]),
                    "markers": [place],
                }),
            );
            return;
        }
        if pos.y >= g.wave.center().y {
            return;
        }
        if let Some(time) = frame.transient_at(pos.x) {
            invoke(
                events,
                "clip.insert_warp_marker",
                json!({ "clips": [clip], "time": time, "pin_neighbours": keys.toggle }),
            );
            return;
        }
        let beat = frame.snap(g.axis.value_at(pos.x), keys.bypass).round() as i64;
        invoke(
            events,
            "clip.insert_warp_marker",
            json!({ "clips": frame.shell.sample_edit_clips(clip, &[]), "beat": beat }),
        );
    }
}

/// How far a drag may move the markers at `places`, in ticks, from `beats`
/// as they were: it stops short of a neighbour that isn't moving with them
/// (R7), so the edit is not refused.
fn bounded_move(beats: &[i64], places: &[u32], by: i64) -> i64 {
    let moving = |place: usize| places.contains(&(place as u32));
    let mut low = i64::MIN;
    let mut high = i64::MAX;
    for &place in places {
        let place = place as usize;
        let Some(&beat) = beats.get(place) else {
            continue;
        };
        if place + 1 < beats.len() && !moving(place + 1) {
            high = high.min(beats[place + 1] - beat - 1);
        }
        if place > 0 && !moving(place - 1) {
            low = low.max(beats[place - 1] - beat + 1);
        }
    }
    if low > high {
        return 0;
    }
    by.clamp(low, high)
}

impl SampleEditorUi {
    /// The editor's context menu (R12): the warp commands, the marker and
    /// transient items, and Show in Browser. An item whose command a later
    /// package brings, or that the clip would refuse, is drawn disabled.
    fn context_menu(
        &self,
        ui: &Ui,
        response: &egui::Response,
        frame: &Frame<'_>,
        events: &mut Vec<UiEvent>,
    ) {
        let clip = frame.clip.id;
        let g = frame.g();
        // A right-click puts the insert marker where it fell, or selects the
        // marker it fell on, so the menu acts there.
        if response.secondary_clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            if let Some(place) = frame.marker_at(pos.x) {
                if !frame.selected_markers.contains(&(place as u32)) {
                    invoke(
                        events,
                        ids::SELECT_WARP_MARKERS,
                        json!({ "clip": clip, "markers": [place] }),
                    );
                }
            } else if !frame.selection.is_some_and(|(from, to)| {
                let t = g.time_at(pos.x);
                t >= from && t <= to
            }) {
                let time = g
                    .time_of(frame.snap(g.axis.value_at(pos.x), false))
                    .clamp(0.0, g.file);
                invoke(
                    events,
                    ids::SET_INSERT_MARKER,
                    json!({ "clip": clip, "time": time }),
                );
            }
        }
        let contexts = [Context::ClipView, Context::SampleEditor];
        let selection = frame.selection.is_some();
        // The editor's selection, as the span of song time the crop keeps.
        let crop_selection = frame.selection.map(|(from, to)| {
            let tempo = &frame.doc.settings().tempo;
            let at = |value: f64| {
                if g.warped {
                    frame.clip.start
                        + bl_core::time::Ticks((value - frame.audio.start).round() as i64)
                } else {
                    tempo.seconds_to_ticks(
                        tempo.ticks_to_seconds(frame.clip.start) + (value - frame.audio.start),
                    )
                }
            };
            json!({ "clip": clip, "start": at(from), "end": at(to) })
        });
        let build = |ui: &mut Ui, events: &mut Vec<UiEvent>| {
            let pending = |ui: &mut Ui, label: &str, package: &str| {
                ui.add_enabled(false, egui::Button::new(label))
                    .on_disabled_hover_text(format!("Not built yet: arrives with {package}."));
            };
            let mut run = |ui: &mut Ui, label: &str, id: &str, args: Value| {
                let enabled = frame.shell.can_invoke_in(id, &args, &contexts);
                let response = ui.add_enabled(enabled, egui::Button::new(label));
                crate::menus::register_item(ui.ctx(), &response);
                if response.clicked() {
                    events.push(UiEvent::Invoke(Invocation::new(id, args)));
                    ui.close();
                }
            };
            pending(
                ui,
                "Set 1.1.1 Here",
                "WP-4.9 (tempo detection and auto-warp)",
            );
            pending(
                ui,
                "Warp From Here",
                "WP-4.9 (tempo detection and auto-warp)",
            );
            pending(
                ui,
                "Warp From Here (Start At …)",
                "WP-4.9 (tempo detection and auto-warp)",
            );
            pending(
                ui,
                "Warp From Here (Straight)",
                "WP-4.9 (tempo detection and auto-warp)",
            );
            pending(
                ui,
                "Warp … BPM From Here",
                "WP-4.9 (tempo detection and auto-warp)",
            );
            pending(
                ui,
                "Warp as x-Bar Loop",
                "WP-4.9 (tempo detection and auto-warp)",
            );
            if selection {
                pending(
                    ui,
                    "Warp Selection as x-Bar Loop",
                    "WP-4.9 (tempo detection and auto-warp)",
                );
            }
            ui.separator();
            run(
                ui,
                "Insert Warp Marker",
                "clip.insert_warp_marker",
                json!({}),
            );
            run(
                ui,
                "Delete Warp Marker",
                "clip.delete_warp_markers",
                json!({}),
            );
            run(ui, "Insert Transient", "clip.insert_transient", json!({}));
            run(ui, "Delete Transient", "clip.delete_transients", json!({}));
            run(
                ui,
                "Reset Transients",
                "clip.reset_transients",
                json!({ "clip": clip }),
            );
            ui.separator();
            run(ui, "Quantize", "clip.quantize_audio", json!({}));
            ui.separator();
            // Spec: audio-clip-editing R14, R16. The Sample Editor's own
            // names for Crop Clip.
            run(ui, "Crop Clip Sample", "clip.crop", json!({ "clip": clip }));
            if let Some(span) = crop_selection.clone() {
                run(ui, "Crop Clip Sample to Time Selection", "clip.crop", span);
            }
            ui.separator();
            run(
                ui,
                "Show in Browser",
                "browser.show_file",
                json!({ "clip": clip }),
            );
        };
        response.context_menu(|ui| build(ui, events));
        // The keyboard opens it too (S-CTX-002): at the insert marker, or the
        // editor's left edge.
        if frame
            .shell
            .active_contexts()
            .contains(&Context::SampleEditor)
            && crate::menus::context_menu_keys(ui)
        {
            let x = frame.insert.map_or(g.wave.left(), |time| g.x_of_time(time));
            crate::menus::open_keyboard_context_menu(
                ui.ctx(),
                response.id,
                pos2(x, g.wave.center().y),
            );
        }
        crate::menus::keyboard_context_menu(response, |ui| build(ui, events));
    }

    /// The editor's arrow keys (R10). The other keys — Cmd+I, Delete, Z, X and
    /// the rest — are the keymap's, and reach the commands that fill in what
    /// the editor has selected.
    fn keys(&self, ui: &Ui, frame: &Frame<'_>, events: &mut Vec<UiEvent>) {
        if !frame
            .shell
            .active_contexts()
            .contains(&Context::SampleEditor)
            || ui.ctx().text_edit_focused()
        {
            return;
        }
        let g = frame.g();
        let clip = frame.clip.id;
        let presses: Vec<(Key, Modifiers)> = ui.input(|i| {
            i.events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::Key {
                        key: key @ (Key::ArrowLeft | Key::ArrowRight),
                        pressed: true,
                        modifiers,
                        ..
                    } => Some((*key, *modifiers)),
                    _ => None,
                })
                .collect()
        });
        let step = frame.step();
        for (key, modifiers) in presses {
            let sign = if key == Key::ArrowRight { 1.0 } else { -1.0 };
            let markers = frame.selected_markers;
            let mac = cfg!(target_os = "macos");
            // Cmd (Ctrl) with an arrow selects the previous or next warp
            // marker; Option (Alt) moves the insert marker. On Windows and
            // Linux Ctrl does both, so it selects markers while there are
            // any and moves the insert marker otherwise.
            let select = modifiers.command && !modifiers.shift && !modifiers.alt;
            let insert = (modifiers.alt && !modifiers.command && !modifiers.shift)
                || (!mac && select && frame.markers().is_empty());
            if select && !insert {
                let count = frame.markers().len() as i64;
                if count == 0 {
                    continue;
                }
                let at = if sign > 0.0 {
                    markers.iter().max().map_or(0, |p| i64::from(*p) + 1)
                } else {
                    markers
                        .iter()
                        .min()
                        .map_or(count - 1, |p| i64::from(*p) - 1)
                };
                invoke(
                    events,
                    ids::SELECT_WARP_MARKERS,
                    json!({ "clip": clip, "markers": [at.clamp(0, count - 1)] }),
                );
            } else if insert {
                let from = frame
                    .insert
                    .map_or_else(|| g.axis.start, |time| g.axis_of(time));
                let value = g.axis_of(g.time_of(from + sign * step).clamp(0.0, g.file));
                invoke(
                    events,
                    ids::SET_INSERT_MARKER,
                    json!({ "clip": clip, "time": g.time_of(value).clamp(0.0, g.file) }),
                );
            } else if modifiers.shift && !modifiers.command && !modifiers.alt {
                if frame.start_selected {
                    // Shift with the Start marker selected moves the region.
                    // Stopping at the file's start keeps the region's length.
                    let shift = (sign * step).max(g.axis_of(0.0) - frame.audio.start);
                    invoke(
                        events,
                        "clip.set_region",
                        json!({
                            "clip": clip,
                            "start": frame.audio.start + shift,
                            "end": frame.audio.end.map(|e| e + shift),
                            "ticks": g.warped,
                        }),
                    );
                } else {
                    // Start or extend a time selection (S-A11Y-013).
                    let (from, to) = frame
                        .selection
                        .or_else(|| frame.insert.map(|t| (t, t)))
                        .unwrap_or((g.time_at(g.wave.left()), g.time_at(g.wave.left())));
                    let (from_v, to_v) = (g.axis_of(from), g.axis_of(to));
                    let (from_v, to_v) = if sign > 0.0 {
                        (from_v, to_v + step)
                    } else {
                        (from_v - step, to_v)
                    };
                    let (a, b) = (
                        g.time_of(from_v).clamp(0.0, g.file),
                        g.time_of(to_v).clamp(0.0, g.file),
                    );
                    if b > a {
                        invoke(
                            events,
                            ids::SELECT_TIME,
                            json!({ "clip": clip, "start": a, "end": b }),
                        );
                    }
                }
            } else if !modifiers.any() && g.warped && !markers.is_empty() {
                // Left and Right move the selected warp markers by a grid
                // step (S-SAMPLE-003).
                let places: Vec<u32> = markers.to_vec();
                let beats: Vec<i64> = frame.markers().iter().map(|m| m.beat.get()).collect();
                let by = bounded_move(&beats, &places, (sign * step).round() as i64);
                if by != 0 {
                    invoke(
                        events,
                        "clip.move_warp_markers",
                        json!({
                            "clips": frame.shell.sample_edit_clips(clip, &places),
                            "markers": places,
                            "by": by,
                        }),
                    );
                }
            }
        }
    }

    /// The editor's parts for assistive technology (R20): the waveform as a
    /// region naming the clip and its file, and each warp marker, transient,
    /// Start and End marker and the loop brace as an item with its role, name
    /// and value. Only what is in view is described.
    fn accessibility(&self, ui: &Ui, frame: &Frame<'_>) {
        let g = frame.g();
        let clip = frame.clip.id;
        let file = frame
            .shell
            .clip_file_path(clip)
            .and_then(|path| path.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default();
        let signature = frame.audio.signature.unwrap_or_else(|| {
            frame
                .doc
                .settings()
                .time_signatures
                .signature_at(frame.clip.start)
        });
        let describe = |time: f64| match beat_label(frame.audio, signature, time) {
            Some(beat) => format!("{beat}, {time:.3} s in the file"),
            None => format!("{time:.3} s in the file"),
        };
        let editor = Rect::from_min_max(g.ruler.min, g.wave.max);
        let node = |id: Id,
                    rect: Rect,
                    role: egui::accesskit::Role,
                    label: String,
                    value: String,
                    selected: Option<bool>| {
            // A marker at the editor's edge is described inside it.
            let rect = rect.intersect(editor);
            let response = ui.interact(rect, id, Sense::hover());
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_role(role);
                node.set_label(label);
                node.set_value(value);
                if let Some(selected) = selected {
                    node.set_selected(selected);
                }
                node.set_bounds(egui::accesskit::Rect {
                    x0: rect.min.x.into(),
                    y0: rect.min.y.into(),
                    x1: rect.max.x.into(),
                    y1: rect.max.y.into(),
                });
            });
        };
        let region = if file.is_empty() {
            frame.clip.name.clone()
        } else {
            format!("{} ({file})", frame.clip.name)
        };
        node(
            Id::new(("sample-waveform", clip)),
            g.wave,
            egui::accesskit::Role::Group,
            format!("Waveform of {region}"),
            String::new(),
            None,
        );
        for place in frame.visible_markers() {
            let marker = &frame.markers()[place];
            let x = g.axis.x_of(marker.beat.get() as f64);
            node(
                Id::new(("sample-warp-marker", clip, place)),
                Rect::from_min_max(
                    pos2(x - HANDLE / 2.0, g.wave.top()),
                    pos2(x + HANDLE / 2.0, g.wave.bottom()),
                ),
                egui::accesskit::Role::ListItem,
                format!("Warp Marker {}", place + 1),
                describe(marker.time),
                Some(frame.selected_markers.contains(&(place as u32))),
            );
        }
        let row = transient_rect(g.wave);
        for index in frame.visible_transients() {
            let time = frame.transients[index];
            let x = g.x_of_time(time);
            node(
                Id::new(("sample-transient", clip, index)),
                Rect::from_min_max(pos2(x - 1.0, row.top()), pos2(x + 1.0, row.bottom())),
                egui::accesskit::Role::ListItem,
                format!("Transient {}", index + 1),
                describe(time),
                Some(
                    frame
                        .selected_transients
                        .iter()
                        .any(|&t| (t - time).abs() <= TRANSIENT_MATCH),
                ),
            );
        }
        let at = |value: f64| g.axis.x_of(value);
        let flag = |x: f32| {
            Rect::from_min_max(
                pos2(x - 2.5, g.strip.top()),
                pos2(x + 2.5, g.strip.bottom()),
            )
        };
        let time_of = |value: f64| g.time_of(value);
        node(
            Id::new(("sample-start", clip)),
            flag(at(frame.audio.start)),
            egui::accesskit::Role::ListItem,
            "Start Marker".to_owned(),
            describe(time_of(frame.audio.start)),
            Some(frame.start_selected),
        );
        node(
            Id::new(("sample-end", clip)),
            flag(at(frame.end())),
            egui::accesskit::Role::ListItem,
            "End Marker".to_owned(),
            describe(time_of(frame.end())),
            Some(false),
        );
        let (from, to) = frame.brace();
        node(
            Id::new(("sample-loop-brace", clip)),
            Rect::from_min_max(
                pos2(at(from), g.strip.bottom() - 6.0),
                pos2(at(to), g.strip.bottom()),
            ),
            egui::accesskit::Role::ListItem,
            "Loop Brace".to_owned(),
            format!(
                "{} to {}, {}",
                describe(time_of(from)),
                describe(time_of(to)),
                if frame.audio.looping { "on" } else { "off" }
            ),
            Some(false),
        );
    }
}

/// The beat a time in the file plays on, as bars, beats and sixteenths, for a
/// warped clip, as the accessible description gives it.
fn beat_label(
    audio: &AudioData,
    signature: bl_core::time::TimeSignature,
    time: f64,
) -> Option<String> {
    let ticks = bl_shell::sample_editor::SampleView::beat_of(audio, time)?.get();
    let (bar, beat) = (signature.bar_length().get(), signature.beat_length().get());
    let (bars, into) = (ticks.div_euclid(bar), ticks.rem_euclid(bar));
    let sixteenths = (into % beat) / Ticks::SIXTEENTH.get();
    // There is no bar 0: the bar before 1.1.1 is -1.
    let number = if bars >= 0 { bars + 1 } else { bars };
    Some(format!(
        "beat {number}.{}.{}",
        into / beat + 1,
        sixteenths + 1
    ))
}

/// The point halfway down `rect`'s height at `x`.
#[must_use]
pub fn at_height(rect: Rect, x: f32, fraction: f32) -> Pos2 {
    pos2(x, rect.top() + rect.height() * fraction)
}

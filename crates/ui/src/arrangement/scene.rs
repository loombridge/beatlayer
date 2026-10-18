//! What the Arrangement View draws in its track lanes, built for the GPU:
//! lane backgrounds, grid lines, clips, and waveforms (arrangement-view R4,
//! R17).
//!
//! Only what's on screen is built. The rows on screen are found by binary
//! search over their tops, and each track's clips are kept sorted by start
//! in a [`ClipIndex`], which re-sorts only the tracks an edit changed, so a
//! frame touches the visible clips and what changed, and nothing else (a
//! drag that moves a few clips doesn't re-sort the set). The instance lists
//! live in
//! the block shared with the GPU callback and keep their capacity, so a
//! steady frame allocates nothing here.

use std::collections::{HashMap, HashSet};

use bl_core::time::{TempoMap, Ticks};
use bl_media::Peaks;
use bl_model::{ClipData, ClipId, ContentHash, Document, Track, TrackId, WarpMap};
use bl_shell::arrangement::Horizontal;
use bl_shell::arrangement::grid::{self, Step};
use bl_shell::arrangement::rows::{Row, rows_between};
use egui::{Color32, Rect, pos2};

use super::gpu::{RectInst, SlotData, WaveInst};
use super::peaks::{Atlas, Known, PeakLibrary};
use crate::theme::Theme;

/// One clip as the view needs it each frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipSpan {
    /// Where it starts.
    pub start: i64,
    /// Where it ends.
    pub end: i64,
    /// Where its contents begin: for an audio clip, where the start of its
    /// file falls, its Start marker's time before its start (WP-1.4b); for a
    /// MIDI clip, its start less its offset (WP-1.8b). A split clip's
    /// waveform carries on from where it was.
    pub origin: i64,
    /// The clip.
    pub id: ClipId,
    /// Whether it's deactivated.
    pub muted: bool,
    /// Its media, for an audio clip.
    pub media: Option<ContentHash>,
}

/// One track's clips, sorted by start.
#[derive(Clone, Debug, Default)]
pub struct TrackClips {
    /// The clips, by start.
    pub clips: Vec<ClipSpan>,
    /// The longest clip's length, so a search can start early enough to
    /// find a long clip that began before the view.
    pub longest: i64,
}

impl TrackClips {
    /// The clips that overlap `from..to`.
    pub fn overlapping(&self, from: i64, to: i64) -> impl Iterator<Item = &ClipSpan> {
        let first = self
            .clips
            .partition_point(|clip| clip.start < from.saturating_sub(self.longest));
        self.clips[first..]
            .iter()
            .take_while(move |clip| clip.start < to)
            .filter(move |clip| clip.end > from)
    }
}

impl TrackClips {
    /// A track's clips, sorted by start, placed under `tempo`.
    fn of(track: &Track, tempo: &TempoMap) -> Self {
        let mut clips: Vec<ClipSpan> = track
            .clips()
            .map(|clip| ClipSpan {
                start: clip.start.get(),
                end: clip.end().get(),
                origin: clip.origin(tempo).get(),
                id: clip.id,
                muted: clip.mute,
                media: match &clip.data {
                    ClipData::Audio(audio) => Some(audio.media),
                    ClipData::Midi(_) => None,
                },
            })
            .collect();
        clips.sort_by_key(|clip| (clip.start, clip.end));
        let longest = clips.iter().map(|c| c.end - c.start).max().unwrap_or(0);
        Self { clips, longest }
    }
}

/// Every track's clips, sorted for the view, brought up to date when the
/// document changes.
///
/// The document shares every track an edit didn't touch with the version
/// before it, so a track that is the same node in both needs nothing done:
/// an update re-sorts only the tracks that changed. A drag (WP-1.8b), which
/// changes the set on every frame, costs what it moves rather than the size
/// of the set.
#[derive(Clone, Debug, Default)]
pub struct ClipIndex {
    revision: Option<u64>,
    /// The version of the document the index holds.
    doc: Option<Document>,
    tracks: HashMap<TrackId, TrackClips>,
    total: usize,
}

impl ClipIndex {
    /// Brings the index up to date if the document's revision isn't the one
    /// it was built from. Returns whether it did.
    pub fn update(&mut self, doc: &Document, revision: u64) -> bool {
        if self.revision == Some(revision) {
            return false;
        }
        self.revision = Some(revision);
        let old = self.doc.replace(doc.clone());
        self.tracks.retain(|id, _| doc.track(*id).is_some());
        self.total = 0;
        // A new tempo map moves where every audio clip's file starts.
        let tempo = &doc.settings().tempo;
        let same_tempo = old
            .as_ref()
            .is_some_and(|old| old.settings().tempo == *tempo);
        for track in doc.tracks() {
            let id = track.id();
            // The same node in both versions: the same clips.
            let same = same_tempo
                && old
                    .as_ref()
                    .and_then(|old| old.track(id))
                    .is_some_and(|before| std::ptr::eq(before, track));
            if !same {
                if track.clips().len() == 0 {
                    self.tracks.remove(&id);
                } else {
                    self.tracks.insert(id, TrackClips::of(track, tempo));
                }
            }
            self.total += self.tracks.get(&id).map_or(0, |t| t.clips.len());
        }
        true
    }

    /// A track's clips.
    #[must_use]
    pub fn track(&self, track: TrackId) -> Option<&TrackClips> {
        self.tracks.get(&track)
    }

    /// Every clip in the document.
    #[must_use]
    pub fn total(&self) -> usize {
        self.total
    }
}

/// What one frame built, for tests and the benchmark.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameStats {
    /// Track rows on screen.
    pub rows: usize,
    /// Clips built: those overlapping the lanes on screen.
    pub clips: usize,
    /// Waveform bands built (one per channel of each audio clip with its
    /// peaks on the GPU).
    pub waveforms: usize,
    /// Rectangles built.
    pub rects: usize,
    /// Clips in the document.
    pub total_clips: usize,
    /// Whether the lanes were built this frame, rather than reused from the
    /// last because nothing they show changed.
    pub rebuilt: bool,
}

/// A clip name to draw over the GPU's clips.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Label {
    /// Where the text may go: the visible part of the clip's title row.
    pub rect: Rect,
    /// The clip.
    pub clip: ClipId,
}

/// A visible clip, for assistive technology.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShownClip {
    /// Its rectangle on screen.
    pub rect: Rect,
    /// The clip.
    pub clip: ClipId,
    /// Its track.
    pub track: TrackId,
}

/// Everything a frame's lanes need.
pub struct LanesInput<'a> {
    /// The set.
    pub doc: &'a Document,
    /// The rows, top to bottom.
    pub rows: &'a [Row],
    /// The horizontal view.
    pub horizontal: Horizontal,
    /// The lanes' rectangle on screen.
    pub lanes: Rect,
    /// How far the rows are scrolled.
    pub scroll_y: f32,
    /// Pixels per point.
    pub pixels_per_point: f32,
    /// The theme.
    pub theme: &'a Theme,
    /// The selected tracks.
    pub selected: &'a [TrackId],
    /// The selected clips (WP-1.8b).
    pub selected_clips: &'a [ClipId],
    /// The grid's spacing.
    pub grid_step: Step,
}

/// The colour a track and its clips take: Beatlayer's palette, in turn by
/// the track's ID, so a new track takes the next colour and keeps it. The
/// document has no colours yet (arrangement-view R3).
#[must_use]
pub fn track_colour(theme: &Theme, track: TrackId) -> Color32 {
    let palette = &theme.colors.track_palette;
    if palette.is_empty() {
        return theme.colors.control.0;
    }
    palette[(track.0.get() % palette.len() as u64) as usize].0
}

fn rgba(colour: Color32) -> [u8; 4] {
    colour.to_array()
}

/// A clip's body: its colour taken part of the way to the lane's, so the
/// waveform reads over it.
fn body_colour(clip: Color32, lane: Color32, dark: bool) -> Color32 {
    let t = if dark { 0.18 } else { 0.35 };
    let mix = |a: u8, b: u8| (f32::from(a) * (1.0 - t) + f32::from(b) * t).round() as u8;
    Color32::from_rgb(
        mix(clip.r(), lane.r()),
        mix(clip.g(), lane.g()),
        mix(clip.b(), lane.b()),
    )
}

/// A waveform's ink: the theme's on lanes tall enough to read it, and a
/// darker shade of the clip's own colour on short ones, so a view of many
/// short tracks still shows each clip's colour.
fn ink(clip: Color32, waveform: Color32, h: f32) -> Color32 {
    if h >= 20.0 {
        return waveform;
    }
    let shade = |c: u8| (f32::from(c) * 0.55).round() as u8;
    Color32::from_rgb(shade(clip.r()), shade(clip.g()), shade(clip.b()))
}

/// The height of a clip's title row, where its name goes, for a lane `h`
/// points tall: none on short lanes, where the name sits over the body.
#[must_use]
pub fn title_height(h: f32) -> f32 {
    if h >= 28.0 { 13.0 } else { 0.0 }
}

/// Builds the lanes' instances into `out`, the clip names into `labels`,
/// and the clips on screen into `shown`, asking `library` for peaks.
#[expect(
    clippy::too_many_lines,
    reason = "one pass over the visible rows, in drawing order"
)]
pub fn build_lanes(
    input: &LanesInput<'_>,
    index: &ClipIndex,
    library: &mut PeakLibrary,
    atlas: &mut Atlas,
    out: &mut SlotData,
    labels: &mut Vec<Label>,
    shown: &mut Vec<ShownClip>,
) -> FrameStats {
    let LanesInput {
        doc,
        rows,
        horizontal,
        lanes,
        scroll_y,
        pixels_per_point,
        theme,
        selected,
        selected_clips,
        grid_step,
    } = *input;
    let colors = &theme.colors;
    let px = 1.0 / pixels_per_point.max(0.1);
    out.rects.clear();
    out.waves.clear();
    labels.clear();
    shown.clear();
    let mut stats = FrameStats {
        total_clips: index.total(),
        rebuilt: true,
        ..FrameStats::default()
    };
    let rect = |out: &mut SlotData, r: [f32; 4], colour: Color32| {
        out.rects.push(RectInst {
            rect: r,
            color: rgba(colour),
        });
    };

    // The lanes' background.
    rect(
        out,
        [lanes.left(), lanes.top(), lanes.right(), lanes.bottom()],
        colors.lane.0,
    );

    let visible = rows_between(rows, scroll_y, scroll_y + lanes.height());
    let row_y = |row: &Row| lanes.top() + row.top - scroll_y;
    stats.rows = visible.len();

    // Selected tracks' lanes.
    for row in &rows[visible.clone()] {
        if selected.contains(&row.track) {
            let y = row_y(row);
            rect(
                out,
                [
                    lanes.left(),
                    y,
                    lanes.right(),
                    y + row.height + row.automation,
                ],
                colors.lane_selected.0,
            );
        }
    }

    // Grid lines, at the grid's spacing; bars only when that's too dense.
    let range = horizontal.range(lanes.width());
    let step_points = grid_step.ticks.get() as f64 / horizontal.ticks_per_point;
    let signatures = &doc.settings().time_signatures;
    let draw_step = if step_points >= 4.0 {
        grid_step
    } else {
        let bar = signatures.signature_at(range.start).bar_length();
        let mut bars = 1;
        while (bar.get() * bars) as f64 / horizontal.ticks_per_point < 4.0 && bars < 1 << 20 {
            bars *= 2;
        }
        Step {
            ticks: Ticks(bar.get() * bars),
            bars: Some(bars),
            note: None,
            triplet: false,
        }
    };
    grid::lines(
        signatures,
        draw_step,
        range.start,
        range.end,
        4096,
        |at, bar| {
            let x = lanes.left() + horizontal.x(at) as f32;
            let colour = if bar {
                colors.grid_bar.0
            } else {
                colors.grid_beat.0
            };
            rect(out, [x, lanes.top(), x + px, lanes.bottom()], colour);
        },
    );

    // Lines between tracks.
    for row in &rows[visible.clone()] {
        let bottom = row_y(row) + row.height + row.automation;
        if row.height >= 3.0 {
            rect(
                out,
                [lanes.left(), bottom - px, lanes.right(), bottom],
                colors.lane_line.0,
            );
        }
    }

    // Clips. A big selection is looked up through a set (WP-1.8b).
    let selected_set: Option<HashSet<ClipId>> =
        (selected_clips.len() > 16).then(|| selected_clips.iter().copied().collect());
    let is_selected = |clip: ClipId| match &selected_set {
        Some(set) => set.contains(&clip),
        None => selected_clips.contains(&clip),
    };
    let tempo = &doc.settings().tempo;
    let from = range.start.get();
    let to = range.end.get();
    let left_limit = lanes.left() - 2.0;
    let right_limit = lanes.right() + 2.0;
    for row in &rows[visible] {
        let Some(clips) = index.track(row.track) else {
            continue;
        };
        let y0 = row_y(row);
        let h = row.height;
        let track_colour = track_colour(theme, row.track);
        for span in clips.overlapping(from, to) {
            stats.clips += 1;
            let x0 = lanes.left() + horizontal.x(Ticks(span.start)) as f32;
            let x1 = lanes.left() + horizontal.x(Ticks(span.end)) as f32;
            let (dx0, dx1) = (x0.max(left_limit), x1.min(right_limit).max(x0 + px));
            let colour = if span.muted {
                colors.clip_muted.0
            } else {
                track_colour
            };
            let inset = if h >= 6.0 { px } else { 0.0 };
            let (top, bottom) = (y0 + inset, y0 + h - inset.max(px));
            let clip_rect = Rect::from_min_max(pos2(x0, top), pos2(x1, bottom));
            shown.push(ShownClip {
                rect: clip_rect,
                clip: span.id,
                track: row.track,
            });
            if row.folded || h < 2.0 {
                // Folded: a bar of the clip's colour.
                rect(out, [dx0, top, dx1, bottom], colour);
            } else {
                let title = title_height(h);
                rect(
                    out,
                    [dx0, top, dx1, bottom],
                    if h >= 20.0 {
                        body_colour(colour, colors.lane.0, theme.dark)
                    } else {
                        colour
                    },
                );
                if title > 0.0 {
                    rect(out, [dx0, top, dx1, top + title], colour);
                }
                // The clip's left edge, on lanes tall enough to show it.
                let edge = if h >= 6.0 { px } else { 0.0 };
                if edge > 0.0 && x0 >= left_limit {
                    rect(out, [x0, top, x0 + px, bottom], colors.lane_line.0);
                }
                if let Some(media) = span.media {
                    // Down to a line on the shortest lanes, as Optimize
                    // Height makes them with 200 tracks.
                    let band = (top + title + edge, bottom - edge);
                    let warp = warp_of(doc, span.id);
                    if band.1 - band.0 >= 1.0
                        && let Known::Ready { peaks, base } = library.get(media, atlas)
                    {
                        stats.waveforms += waveform(
                            out,
                            &WaveClip {
                                peaks,
                                base: *base,
                                start: Ticks(span.origin),
                                draw: (dx0.max(x0), dx1.min(x1)),
                                band,
                                warp: warp.as_ref().map(|(map, shift)| (map, *shift)),
                            },
                            tempo,
                            horizontal,
                            lanes,
                            pixels_per_point,
                            if span.muted {
                                colors.lane.0
                            } else {
                                ink(colour, colors.waveform.0, h)
                            },
                        );
                    }
                }
            }
            if is_selected(span.id) && !row.folded && h >= 2.0 {
                // Spec: arrangement-editing R15. The title row, or a band
                // along the top of a short clip, and an outline.
                let band = title_height(h).max((h * 0.25).clamp(px, 4.0));
                let highlight = colors.time_selection.0;
                rect(out, [dx0, top, dx1, top + band], highlight);
                let line = (2.0 * px).min((bottom - top) / 2.0);
                rect(out, [dx0, bottom - line, dx1, bottom], highlight);
                if x0 >= left_limit {
                    rect(out, [x0, top, x0 + line, bottom], highlight);
                }
                if x1 <= right_limit {
                    rect(out, [x1 - line, top, x1, bottom], highlight);
                }
            }
            if x1 - x0 >= 20.0 && h >= 12.0 {
                let title = title_height(h).max(h.min(14.0));
                labels.push(Label {
                    rect: Rect::from_min_max(
                        pos2(x0.max(lanes.left()), top),
                        pos2(x1.min(lanes.right()), (top + title).min(bottom)),
                    ),
                    clip: span.id,
                });
            }
        }
    }
    stats.rects = out.rects.len();
    stats
}

/// A clip's waveform to draw: its peaks, where they are in the peak
/// buffer, where the clip's contents begin, and what part of it to draw.
struct WaveClip<'a> {
    peaks: &'a Peaks,
    base: u64,
    /// Where the media's first frame plays: the clip's start less its
    /// offset.
    start: Ticks,
    /// The left and right edges to draw, on screen.
    draw: (f32, f32),
    /// The top and bottom of the waveform's area.
    band: (f32, f32),
    /// A warped clip's map, and the tick of the arrangement that its beat 0
    /// falls on: the file is drawn through it, each segment between two
    /// warp markers stretched to the beats it plays on (sample editor R4).
    warp: Option<(&'a WarpMap, f64)>,
}

/// A warped audio clip's map and the tick its beat 0 falls on.
fn warp_of(doc: &Document, clip: ClipId) -> Option<(WarpMap, f64)> {
    let value = doc.clip(clip)?;
    match &value.data {
        ClipData::Audio(audio) if audio.warp => {
            let map = audio.warp_map().ok()?;
            Some((map, value.start.get() as f64 - audio.start))
        }
        _ => None,
    }
}

/// Adds one waveform band per channel and returns how many. An unwarped
/// clip's media plays from its first frame where the clip's contents begin,
/// at its own speed, so the frame under a point is worked out through the
/// tempo map at the drawn part's left edge. A warped clip's is drawn through
/// its warp map instead (see [`WaveClip::warp`]).
fn waveform(
    out: &mut SlotData,
    clip: &WaveClip<'_>,
    tempo: &TempoMap,
    horizontal: Horizontal,
    lanes: Rect,
    pixels_per_point: f32,
    colour: Color32,
) -> usize {
    let peaks = clip.peaks;
    let rate = f64::from(peaks.sample_rate());
    let (left, right) = clip.draw;
    if right <= left || peaks.frames() == 0 {
        return 0;
    }
    if let Some((map, shift)) = clip.warp {
        return warped_waveform(
            out,
            clip,
            map,
            shift,
            horizontal,
            lanes,
            pixels_per_point,
            colour,
        );
    }
    let at_left = horizontal.at(f64::from(left - lanes.left()));
    let seconds = tempo.ticks_to_seconds(at_left) - tempo.ticks_to_seconds(clip.start);
    let frame_left = (seconds * rate).max(0.0);
    // Seconds per tick at the left edge, from the tempo there.
    let bpm = tempo.bpm_at(at_left).get();
    let seconds_per_tick = 60.0 / (bpm * Ticks::QUARTER.get() as f64);
    let frames_per_point = horizontal.ticks_per_point * seconds_per_tick * rate;
    // Where the media ends, on screen.
    let media_right = f64::from(left) + (peaks.frames() as f64 - frame_left) / frames_per_point;
    let right = right.min(media_right as f32);
    if right <= left {
        return 0;
    }
    band(
        out,
        clip,
        frame_left,
        frames_per_point,
        (left, right),
        pixels_per_point,
        colour,
    )
}

/// Adds the waveform's bands for the part of the media that plays between
/// the points `span`, starting at `frame_left` and running at
/// `frames_per_point` frames to the point.
fn band(
    out: &mut SlotData,
    clip: &WaveClip<'_>,
    frame_left: f64,
    frames_per_point: f64,
    span: (f32, f32),
    pixels_per_point: f32,
    colour: Color32,
) -> usize {
    let peaks = clip.peaks;
    let (left, right) = span;
    if right <= left {
        return 0;
    }
    let frames_per_pixel = frames_per_point / f64::from(pixels_per_point);
    let level = peaks.level_for(frames_per_pixel);
    let frames_per_peak = Peaks::frames_per_peak(level) as f64;
    let peak_left = frame_left / frames_per_peak;
    let whole = peak_left.floor();
    let channels = usize::from(peaks.channels());
    let (top, bottom) = clip.band;
    let band_height = (bottom - top) / channels as f32;
    let mut count = 0;
    for channel in 0..channels {
        let Some(run) = peaks.run(level, channel) else {
            continue;
        };
        let Ok(first) = u32::try_from(clip.base + u64::from(run.offset)) else {
            continue;
        };
        let band_top = top + band_height * channel as f32;
        out.waves.push(WaveInst {
            draw: [left, band_top, right, band_top + band_height],
            band: [
                band_top,
                band_top + band_height,
                (frames_per_pixel / frames_per_peak) as f32,
                (peak_left - whole) as f32,
            ],
            run: [first, run.len, whole.min(f64::from(u32::MAX)) as u32, 0],
            color: rgba(colour),
        });
        count += 1;
    }
    count
}

/// Draws a warped clip's waveform: one stretch of bands for each segment of
/// its warp map that falls in view, each running at the speed that segment
/// plays the file, so the waveform lines up with the beats it plays on. The
/// markers in view are found by binary search.
#[expect(clippy::too_many_arguments, reason = "the waveform's whole geometry")]
fn warped_waveform(
    out: &mut SlotData,
    clip: &WaveClip<'_>,
    map: &WarpMap,
    shift: f64,
    horizontal: Horizontal,
    lanes: Rect,
    pixels_per_point: f32,
    colour: Color32,
) -> usize {
    let rate = f64::from(clip.peaks.sample_rate());
    let file = clip.peaks.frames() as f64 / rate;
    let (left, right) = clip.draw;
    // The beat of the contents under a point, and the point of a beat.
    let beat_at = |x: f32| horizontal.at(f64::from(x - lanes.left())).get() as f64 - shift;
    let x_of = |beat: f64| lanes.left() + horizontal.x(Ticks((beat + shift).round() as i64)) as f32;
    // The file plays from time 0 to its end: draw only the beats inside it.
    let (first, last) = (map.time_to_beat(0.0), map.time_to_beat(file));
    let (from, to) = (beat_at(left).max(first), beat_at(right).min(last));
    if to <= from {
        return 0;
    }
    let markers = map.markers();
    let mut next = markers.partition_point(|m| (m.beat as f64) <= from);
    let mut start = from;
    let mut count = 0;
    while start < to {
        // Each stretch runs to the next marker, or to the end of the view.
        let end = markers.get(next).map_or(to, |m| (m.beat as f64).min(to));
        if end > start {
            let (t0, t1) = (map.beat_to_time(start), map.beat_to_time(end));
            let (x0, x1) = (x_of(start), x_of(end));
            if x1 > x0 && t1 > t0 {
                let frames_per_point = (t1 - t0) * rate / f64::from(x1 - x0);
                count += band(
                    out,
                    clip,
                    t0 * rate,
                    frames_per_point,
                    (x0, x1.min(right)),
                    pixels_per_point,
                    colour,
                );
            }
        }
        start = end;
        next += 1;
    }
    count
}

/// Builds the Overview's instances: every clip, track by track, squeezed
/// into the strip from 1.1.1 to `end`.
pub fn build_overview(
    rows: &[Row],
    index: &ClipIndex,
    strip: Rect,
    end: Ticks,
    theme: &Theme,
    out: &mut SlotData,
) {
    out.rects.clear();
    out.waves.clear();
    out.rects.push(RectInst {
        rect: [strip.left(), strip.top(), strip.right(), strip.bottom()],
        color: rgba(theme.colors.ruler.0),
    });
    if rows.is_empty() || end.get() <= 0 {
        return;
    }
    let row_height = strip.height() / rows.len() as f32;
    let ticks_per_point = end.get() as f64 / f64::from(strip.width().max(1.0));
    for (i, row) in rows.iter().enumerate() {
        let Some(clips) = index.track(row.track) else {
            continue;
        };
        let y0 = strip.top() + row_height * i as f32;
        let y1 = (y0 + row_height).max(y0 + 0.5);
        let colour = track_colour(theme, row.track);
        for clip in &clips.clips {
            let x0 = strip.left() + (clip.start as f64 / ticks_per_point) as f32;
            let x1 = (strip.left() + (clip.end as f64 / ticks_per_point) as f32).max(x0 + 0.5);
            out.rects.push(RectInst {
                rect: [x0, y0, x1.min(strip.right()), y1],
                color: rgba(if clip.muted {
                    theme.colors.clip_muted.0
                } else {
                    colour
                }),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use bl_core::time::Ticks;
    use bl_model::{AudioData, Clip, Command, TrackKind};

    use super::super::benchmark::{Random, media_hash, scene};
    use super::*;

    /// Each track's clips as the view reads them, and how many there are.
    fn contents(index: &ClipIndex) -> (Vec<(TrackId, Vec<ClipSpan>, i64)>, usize) {
        let mut tracks: Vec<_> = index
            .tracks
            .iter()
            .map(|(track, clips)| (*track, clips.clips.clone(), clips.longest))
            .collect();
        tracks.sort_by_key(|(track, _, _)| *track);
        (tracks, index.total())
    }

    /// The index built from nothing.
    fn rebuilt(doc: &Document) -> ClipIndex {
        let mut index = ClipIndex::default();
        index.update(doc, 0);
        index
    }

    #[test]
    fn re_sorting_only_changed_tracks_matches_a_rebuild() {
        // WP-1.8b: an update re-sorts only the tracks an edit changed; the
        // index must hold what building it from nothing would.
        let set = scene(6, 30, 2);
        let mut editor = set.editor;
        let mut tracks = set.tracks;
        let mut random = Random::new(7);
        let mut index = ClipIndex::default();
        let mut revision = 0;
        index.update(editor.document(), revision);
        let bar = Ticks::WHOLE.get();
        let below = |random: &mut Random, n: usize| random.below(n as u64) as usize;
        for step in 0..400 {
            let clips: Vec<ClipId> = editor.document().clip_ids().collect();
            let command = match random.below(8) {
                // Many clips at once, as dragging a big selection does.
                0 => Command::MoveClips {
                    clips: clips
                        .iter()
                        .copied()
                        .filter(|_| random.below(3) == 0)
                        .collect(),
                    by: Ticks(bar * (random.below(5) as i64 + 1)),
                },
                1 | 2 => Command::MoveClip {
                    clip: clips[below(&mut random, clips.len())],
                    track: tracks[below(&mut random, tracks.len())],
                    start: Ticks(bar * random.below(512) as i64),
                },
                3 => Command::SetClipLength {
                    clip: clips[below(&mut random, clips.len())],
                    length: Ticks(bar * (random.below(16) as i64 + 1)),
                },
                4 => Command::SetClipMute {
                    clip: clips[below(&mut random, clips.len())],
                    mute: random.below(2) == 0,
                },
                5 => Command::DeleteClip {
                    clip: clips[below(&mut random, clips.len())],
                },
                6 => {
                    // A new, empty track: the track list changes.
                    let id: TrackId = editor.new_id();
                    tracks.push(id);
                    Command::CreateTrack {
                        id,
                        kind: TrackKind::Audio,
                        name: format!("New {step}"),
                        parent: None,
                        index: 0,
                    }
                }
                _ => Command::CreateClip {
                    track: tracks[below(&mut random, tracks.len())],
                    clip: Clip::new(
                        editor.new_id(),
                        Ticks(bar * random.below(512) as i64),
                        Ticks(bar * (random.below(8) as i64 + 1)),
                        ClipData::Audio(AudioData::new(media_hash(1))),
                    ),
                },
            };
            if editor.apply(command).is_err() {
                continue;
            }
            revision += 1;
            assert!(index.update(editor.document(), revision));
            assert_eq!(
                contents(&index),
                contents(&rebuilt(editor.document())),
                "step {step}"
            );
        }
        assert!(!index.update(editor.document(), revision));
    }
}

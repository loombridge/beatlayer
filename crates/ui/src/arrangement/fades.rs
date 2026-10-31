//! The fade handles on audio clips in the Arrangement (WP-6.51, the audio
//! clips spec R27; I-DRAG-017, I-HOLD-002).
//!
//! While the pointer is over an audio clip on a lane at least 40 points tall,
//! and over every clip while F is held, each of the clip's fades shows three
//! handles: one at its peak, where it reaches full level; one at its outer
//! end, where it reaches silence, at the clip's edge or a crossfade's far
//! end; and a curve handle between them. Dragging a peak handle sets the
//! fade's length; dragging the outer handle of an edge another audio clip
//! meets, out over that clip, makes or changes a crossfade; dragging the curve
//! handle up or down shapes the fade. Each drag is the document's own
//! `clip.set_fades` or `arrangement.set_crossfade`, run as one gesture.

use bl_core::time::Ticks;
use bl_model::{ClipData, ClipId, Crossfade, FadeLimits, Track, TrackId, crossfades, fade_power};
use bl_shell::arrangement_commands::{FadeHandle, FadeHandleState};
use bl_shell::editing_commands::Edge;
use egui::{Painter, Pos2, Rect, Shape, Stroke, pos2};
use serde_json::{Value, json};
use std::collections::HashMap;

use super::editing::Lanes;
use super::scene::{ShownClip, title_height};
use crate::theme::Theme;

/// The shortest lane that shows a hovered clip's handles, in points
/// (Beatlayer's own, R27).
pub const MIN_LANE_HEIGHT: f32 = 40.0;

/// How close, in points, the pointer must be to a handle to take it.
const GRAB: f32 = 6.0;

/// How big a handle is drawn, in points across.
const SIZE: f32 = 7.0;

/// One handle on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    /// The clip.
    pub clip: ClipId,
    /// Its fade-in (`Start`) or fade-out (`End`).
    pub edge: Edge,
    /// Which of the fade's handles.
    pub handle: FadeHandle,
    /// Where it is.
    pub pos: Pos2,
}

/// The vertical band a clip's fades are drawn in: from just under its title
/// row to its bottom edge.
fn band(shown: &ShownClip, height: f32) -> (f32, f32) {
    let top = shown.rect.top() + title_height(height) + 2.0;
    (top, (shown.rect.bottom() - 2.0).max(top + 1.0))
}

/// The row height of a clip's lane.
fn lane_height(_lanes: &Lanes<'_>, shown: &ShownClip) -> f32 {
    shown.lane_height
}

/// The song position `seconds` after `from` under the tempo map.
fn after(lanes: &Lanes<'_>, from: Ticks, seconds: f64) -> Ticks {
    let tempo = &lanes.doc.settings().tempo;
    tempo.seconds_to_ticks(tempo.ticks_to_seconds(from) + seconds)
}

/// The seconds from `a` to `b` under the tempo map.
fn seconds_between(lanes: &Lanes<'_>, a: Ticks, b: Ticks) -> f64 {
    let tempo = &lanes.doc.settings().tempo;
    tempo.ticks_to_seconds(b) - tempo.ticks_to_seconds(a)
}

/// The x of a song position.
fn x_of(lanes: &Lanes<'_>, at: Ticks) -> f32 {
    lanes.rect.left() + lanes.horizontal.x(at) as f32
}

/// The shown clips whose handles show now: those under `pointer`, the clips
/// they crossfade with, and, with F held, every audio clip on screen.
pub(super) fn visible<'a>(
    lanes: &'a Lanes<'_>,
    pointer: Option<Pos2>,
    held: bool,
) -> Vec<&'a ShownClip> {
    let audio = |shown: &&ShownClip| {
        lanes
            .doc
            .clip(shown.clip)
            .is_some_and(|clip| clip.data.is_audio())
    };
    if held {
        return lanes.shown.iter().filter(audio).collect();
    }
    let Some(pointer) = pointer.filter(|p| lanes.rect.contains(*p)) else {
        return Vec::new();
    };
    let Some(under) = lanes.shown.iter().rev().find(|s| s.rect.contains(pointer)) else {
        return Vec::new();
    };
    if lane_height(lanes, under) < MIN_LANE_HEIGHT || !audio(&under) {
        return Vec::new();
    }
    let mut out = vec![under];
    // A handle at the edge two clips share belongs to both, and the pointer
    // is over only the later.
    if let Some(value) = lanes.doc.clip(under.clip) {
        out.extend(lanes.shown.iter().filter(|other| {
            other.track == under.track
                && other.clip != under.clip
                && lanes.doc.clip(other.clip).is_some_and(|c| {
                    c.data.is_audio() && (c.end() == value.start || c.start == value.end())
                })
        }));
    }
    if let Some(track) = lanes.doc.track(under.track) {
        for crossfade in track_crossfades(track) {
            let partner = if crossfade.earlier == under.clip {
                crossfade.later
            } else if crossfade.later == under.clip {
                crossfade.earlier
            } else {
                continue;
            };
            out.extend(lanes.shown.iter().filter(|s| s.clip == partner));
        }
    }
    out
}

/// The crossfades of a track's clips.
fn track_crossfades(track: &Track) -> Vec<Crossfade> {
    #[cfg(test)]
    tests::CALLS.with(|calls| calls.set(calls.get() + 1));
    crossfades(track.clips())
}

/// The crossfades of each track the `showing` clips sit on, found once per
/// track: with F held, every clip on screen shows, and each used to find its
/// whole track's again.
fn joined_by_track(lanes: &Lanes<'_>, showing: &[&ShownClip]) -> HashMap<TrackId, Vec<Crossfade>> {
    let mut joined = HashMap::new();
    for shown in showing {
        if let Some(track) = lanes.doc.track(shown.track) {
            joined
                .entry(shown.track)
                .or_insert_with(|| track_crossfades(track));
        }
    }
    joined
}

/// The handles of one shown clip's two fades.
pub(super) fn spots(
    lanes: &Lanes<'_>,
    shown: &ShownClip,
    joined: &[bl_model::Crossfade],
) -> Vec<Spot> {
    let Some(clip) = lanes.doc.clip(shown.clip) else {
        return Vec::new();
    };
    let ClipData::Audio(audio) = &clip.data else {
        return Vec::new();
    };
    let (top, bottom) = band(shown, lane_height(lanes, shown));
    let before = if joined.iter().any(|c| c.later == clip.id) {
        audio.fade_in_before
    } else {
        0.0
    };
    let tail = if joined.iter().any(|c| c.earlier == clip.id) {
        audio.fade_out_after
    } else {
        0.0
    };
    let mut out = Vec::with_capacity(6);
    for (edge, outer, peak, curve) in [
        (
            Edge::Start,
            x_of(lanes, after(lanes, clip.start, -before)),
            x_of(lanes, after(lanes, clip.start, audio.fade_in)),
            audio.fade_in_curve,
        ),
        (
            Edge::End,
            x_of(lanes, after(lanes, clip.end(), tail)),
            x_of(lanes, after(lanes, clip.end(), -audio.fade_out)),
            audio.fade_out_curve,
        ),
    ] {
        // Half way along the fade, at the gain the curve gives there.
        let gain = 0.5f64.powf(fade_power(curve)) as f32;
        out.push(Spot {
            clip: clip.id,
            edge,
            handle: FadeHandle::Peak,
            pos: pos2(peak, top),
        });
        // A fade with no length has no shape to give.
        if (peak - outer).abs() > f32::EPSILON {
            out.push(Spot {
                clip: clip.id,
                edge,
                handle: FadeHandle::Curve,
                pos: pos2((outer + peak) / 2.0, bottom - gain * (bottom - top)),
            });
        }
        out.push(Spot {
            clip: clip.id,
            edge,
            handle: FadeHandle::Outer,
            pos: pos2(outer, bottom),
        });
    }
    out
}

/// The handle under `pos`, if one is within reach and showing.
pub(super) fn spot_at(lanes: &Lanes<'_>, pos: Pos2, held: bool) -> Option<Spot> {
    let showing = visible(lanes, Some(pos), held);
    let joined = joined_by_track(lanes, &showing);
    let mut best: Option<(f32, Spot)> = None;
    for shown in showing {
        let joined = joined.get(&shown.track).map_or(&[][..], Vec::as_slice);
        for spot in spots(lanes, shown, joined) {
            let distance = spot.pos.distance(pos);
            if distance <= GRAB && best.is_none_or(|(d, _)| distance < d) {
                best = Some((distance, spot));
            }
        }
    }
    best.map(|(_, spot)| spot)
}

/// Where the clip whose outer handle is dragged meets another audio clip.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Meeting {
    boundary: Ticks,
    /// Whether the two clips already crossfade, with these spans.
    crossfade: Option<(f64, f64)>,
    /// The earlier clip's fade-out and the later clip's fade-in, inside
    /// their clips: where a new crossfade's span starts and ends.
    earlier_fade: f64,
    later_fade: f64,
    /// The furthest the crossfade may reach before the boundary, into the
    /// earlier clip, and after it, into the later one.
    before_room: f64,
    after_room: f64,
}

/// A drag of one handle.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct FadeDrag {
    spot: Spot,
    track: TrackId,
    /// The clip's fades when the drag began: `(fade_in, fade_out)`.
    lengths: (f64, f64),
    /// The curve of the fade being dragged.
    curve: f64,
    limits: FadeLimits,
    /// The band the clip's fades are drawn in, top and bottom.
    band: (f32, f32),
    /// The clip the dragged edge meets, if one does.
    meeting: Option<Meeting>,
}

impl FadeDrag {
    /// A drag starting on `spot`.
    pub(super) fn begin(lanes: &Lanes<'_>, spot: Spot) -> Option<Self> {
        let clip = lanes.doc.clip(spot.clip)?;
        let ClipData::Audio(audio) = &clip.data else {
            return None;
        };
        let shown = lanes.shown.iter().find(|s| s.clip == spot.clip)?;
        let track = lanes.doc.track(shown.track)?;
        let boundary = match spot.edge {
            Edge::Start => clip.start,
            Edge::End => clip.end(),
        };
        let neighbour = track.clips().find(|other| {
            other.id != clip.id
                && other.data.is_audio()
                && match spot.edge {
                    Edge::Start => other.end() == boundary,
                    Edge::End => other.start == boundary,
                }
        });
        let meeting = neighbour.and_then(|other| {
            let (earlier, later) = match spot.edge {
                Edge::Start => (other, clip),
                Edge::End => (clip, other),
            };
            let (ClipData::Audio(first), ClipData::Audio(second), Some(before), Some(after)) = (
                &earlier.data,
                &later.data,
                lanes.doc.fade_limits(later.id),
                lanes.doc.fade_limits(earlier.id),
            ) else {
                return None;
            };
            Some(Meeting {
                boundary,
                crossfade: track_crossfades(track)
                    .iter()
                    .find(|c| c.earlier == earlier.id && c.later == later.id)
                    .map(|c| (c.before, c.after)),
                earlier_fade: first.fade_out,
                later_fade: second.fade_in,
                before_room: before.start.outside,
                after_room: after.end.outside,
            })
        });
        Some(Self {
            spot,
            track: shown.track,
            lengths: (audio.fade_in, audio.fade_out),
            curve: match spot.edge {
                Edge::Start => audio.fade_in_curve,
                Edge::End => audio.fade_out_curve,
            },
            limits: lanes.doc.fade_limits(spot.clip)?,
            band: band(shown, lane_height(lanes, shown)),
            meeting,
        })
    }

    /// The handle being dragged.
    pub(super) fn state(&self) -> FadeHandleState {
        FadeHandleState {
            clip: self.spot.clip,
            edge: self.spot.edge,
            handle: self.spot.handle,
        }
    }

    /// The command for the pointer at `pointer`, with the position snapped as
    /// `snap` says.
    pub(super) fn frame(
        &self,
        lanes: &Lanes<'_>,
        pointer: Pos2,
        snap: impl Fn(Ticks) -> Ticks,
    ) -> Option<(&'static str, Value)> {
        let clip = lanes.doc.clip(self.spot.clip)?;
        let at = snap(lanes.at(pointer));
        let (fade_in, fade_out) = self.lengths;
        let room = match self.spot.edge {
            Edge::Start => self.limits.start,
            Edge::End => self.limits.end,
        };
        // How far inside the clip, or past its edge, the pointer is: seconds
        // from the edge, positive towards the clip's middle.
        let inward = match self.spot.edge {
            Edge::Start => seconds_between(lanes, clip.start, at),
            Edge::End => seconds_between(lanes, at, clip.end()),
        };
        match self.spot.handle {
            FadeHandle::Peak => {
                let length = inward.clamp(0.0, room.inside);
                let (fade_in, fade_out) = match self.spot.edge {
                    Edge::Start => (length, fade_out),
                    Edge::End => (fade_in, length),
                };
                Some((
                    "clip.set_fades",
                    json!({ "clip": clip.id, "fade_in": fade_in, "fade_out": fade_out }),
                ))
            }
            FadeHandle::Outer => {
                // Only an edge another clip meets has a crossfade to make.
                let meeting = self.meeting?;
                let delta = seconds_between(lanes, meeting.boundary, at);
                let (before, after) = match (meeting.crossfade, self.spot.edge) {
                    // The end of the earlier clip moves the span's far end.
                    (Some((before, _)), Edge::End) => {
                        (before, delta.clamp(0.0, meeting.after_room))
                    }
                    (Some((_, after)), Edge::Start) => {
                        ((-delta).clamp(0.0, meeting.before_room), after)
                    }
                    // Where the clips' handles coincide, the way the pointer
                    // goes chooses whose it is: out over the later clip, or
                    // back over the earlier.
                    (None, _) if delta >= 0.0 => {
                        (meeting.earlier_fade, delta.min(meeting.after_room))
                    }
                    (None, _) => ((-delta).min(meeting.before_room), meeting.later_fade),
                };
                (before + after > 0.0).then(|| {
                    (
                        "arrangement.set_crossfade",
                        json!({
                            "track": self.track,
                            "boundary": meeting.boundary,
                            "before": before,
                            "after": after,
                            "curve": self.curve,
                        }),
                    )
                })
            }
            FadeHandle::Curve => {
                // The gain half way along the fade is where the pointer is
                // between the band's bottom (0) and top (1).
                let (top, bottom) = self.band;
                let gain = f64::from(((bottom - pointer.y) / (bottom - top)).clamp(0.02, 0.98));
                let power = gain.ln() / 0.5f64.ln();
                let curve = (-power.ln() / 4f64.ln()).clamp(-1.0, 1.0);
                let key = match self.spot.edge {
                    Edge::Start => "fade_in_curve",
                    Edge::End => "fade_out_curve",
                };
                Some((
                    "clip.set_fades",
                    json!({
                        "clip": clip.id,
                        "fade_in": fade_in,
                        "fade_out": fade_out,
                        key: curve,
                    }),
                ))
            }
        }
    }
}

/// What a frame drew of the fade handles, for tests.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drawn {
    /// The handles that showed.
    pub spots: Vec<Spot>,
    /// The dotted line of the selected handle's limit: its two ends.
    pub limit: Option<[Pos2; 2]>,
}

/// Draws the handles that show, each fade's outline, and, for the selected
/// or dragged handle, a dotted line across the clip at the furthest it can go
/// (R25). Returns what it drew.
pub(super) fn paint(
    painter: &Painter,
    lanes: &Lanes<'_>,
    theme: &Theme,
    pointer: Option<Pos2>,
    held: bool,
    selected: Option<FadeHandleState>,
) -> Drawn {
    let colors = &theme.colors;
    let mut drawn = Drawn::default();
    let showing = visible(lanes, pointer, held);
    let joined = joined_by_track(lanes, &showing);
    for shown in showing {
        let joined = joined.get(&shown.track).map_or(&[][..], Vec::as_slice);
        let spots = spots(lanes, shown, joined);
        let (top, bottom) = band(shown, lane_height(lanes, shown));
        // Each fade's outline, from the outer handle through the curve
        // handle, where it has one, to the peak.
        for edge in [Edge::Start, Edge::End] {
            let of = |handle: FadeHandle| {
                spots
                    .iter()
                    .find(|s| s.edge == edge && s.handle == handle)
                    .map(|s| s.pos)
            };
            let line: Vec<Pos2> = [FadeHandle::Outer, FadeHandle::Curve, FadeHandle::Peak]
                .into_iter()
                .filter_map(of)
                .collect();
            painter.add(Shape::line(line, Stroke::new(1.0, colors.clip_text.0)));
        }
        drawn.spots.extend(spots.iter().copied());
        for spot in &spots {
            let on = selected.is_some_and(|s| {
                s.clip == spot.clip && s.edge == spot.edge && s.handle == spot.handle
            });
            let fill = if on {
                colors.accent.0
            } else {
                colors.clip_text.0
            };
            let square = Rect::from_center_size(spot.pos, egui::vec2(SIZE, SIZE));
            let outline = Stroke::new(1.0, colors.lane_line.0);
            if spot.handle == FadeHandle::Curve {
                painter.circle(spot.pos, SIZE / 2.0, fill, outline);
            } else {
                painter.rect(square, 1.0, fill, outline, egui::StrokeKind::Inside);
            }
        }
        // The limit of the selected handle.
        let Some(on) = selected.filter(|s| s.clip == shown.clip) else {
            continue;
        };
        let (Some(clip), Some(limits)) = (lanes.doc.clip(on.clip), lanes.doc.fade_limits(on.clip))
        else {
            continue;
        };
        let mut dotted = |points: [Pos2; 2]| {
            painter.add(Shape::dotted_line(&points, colors.accent.0, 4.0, 0.8));
            drawn.limit.get_or_insert(points);
        };
        let (edge_at, room, sign) = match on.edge {
            Edge::Start => (clip.start, limits.start, 1.0),
            Edge::End => (clip.end(), limits.end, -1.0),
        };
        match on.handle {
            FadeHandle::Peak | FadeHandle::Outer => {
                let reach = if on.handle == FadeHandle::Peak {
                    room.inside * sign
                } else {
                    -room.outside * sign
                };
                let x = x_of(lanes, after(lanes, edge_at, reach));
                dotted([pos2(x, top), pos2(x, bottom)]);
            }
            FadeHandle::Curve => {
                let (left, right) = (shown.rect.left(), shown.rect.right());
                for curve in [-1.0, 1.0] {
                    let gain = 0.5f64.powf(fade_power(curve)) as f32;
                    let y = bottom - gain * (bottom - top);
                    dotted([pos2(left, y), pos2(right, y)]);
                }
            }
        }
    }
    drawn
}

#[cfg(test)]
pub(super) mod tests {
    use std::cell::Cell;

    use bl_model::{AudioData, Clip, Command, Editor, TrackKind};
    use bl_shell::arrangement::{ArrangementView, Horizontal};
    use egui::{Context, LayerId};

    use super::super::benchmark::media_hash;
    use super::*;

    thread_local! {
        /// How many times `track_crossfades` has run on this thread.
        pub(in super::super) static CALLS: Cell<usize> = const { Cell::new(0) };
    }

    const CLIPS: usize = 2000;

    #[test]
    fn showing_every_fade_handle_finds_each_tracks_crossfades_once() {
        // Chopped break: one track of abutting clips, every second pair
        // crossfaded. With F held every clip shows, and each used to find the
        // whole track's crossfades again.
        let beat = Ticks::WHOLE.get() / 4;
        let mut editor = Editor::new();
        let track: TrackId = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Audio,
                name: "Break".into(),
                parent: None,
                index: 0,
            })
            .unwrap();
        let mut shown = Vec::new();
        for index in 0..CLIPS {
            let id: ClipId = editor.new_id();
            let mut audio = AudioData::new(media_hash(0));
            if index % 2 == 0 {
                audio.fade_out = 0.1;
                audio.fade_out_after = 0.1;
            } else {
                audio.fade_in = 0.1;
                audio.fade_in_before = 0.1;
            }
            editor
                .apply(Command::CreateClip {
                    track,
                    clip: Clip::new(
                        id,
                        Ticks(beat * index as i64),
                        Ticks(beat),
                        ClipData::Audio(audio),
                    ),
                })
                .unwrap();
            shown.push(ShownClip {
                rect: Rect::from_min_size(pos2(index as f32 * 10.0, 0.0), egui::vec2(10.0, 80.0)),
                clip: id,
                track,
                lane: None,
                lane_height: 80.0,
            });
        }
        let view = ArrangementView::default();
        let lanes = Lanes {
            view: &view,
            doc: editor.document(),
            rect: Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(10.0 * CLIPS as f32, 80.0)),
            horizontal: Horizontal {
                left: 0.0,
                ticks_per_point: beat as f64 / 10.0,
            },
            shown: &shown,
            rows: &[],
            scroll_y: 0.0,
            draw_mode: false,
        };
        let theme = Theme::bundled(true);
        let ctx = Context::default();
        let painter = ctx.layer_painter(LayerId::background());

        CALLS.with(|calls| calls.set(0));
        let drawn = paint(&painter, &lanes, &theme, None, true, None);
        assert_eq!(CALLS.with(Cell::get), 1);
        assert!(!drawn.spots.is_empty());

        CALLS.with(|calls| calls.set(0));
        let found = spot_at(&lanes, drawn.spots[0].pos, true);
        assert!(CALLS.with(Cell::get) <= 2);
        assert!(found.is_some());
    }
}

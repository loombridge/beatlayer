//! The Arrangement's editing gestures (WP-1.8b): clicking a clip's title bar
//! selects it, dragging it moves or copies the selection, dragging an edge
//! resizes, and keys nudge, move the insert marker between clip edges,
//! extend the selection, and resize from the keyboard.
//!
//! Every gesture becomes one of the Arrangement's editing commands
//! (`bl_shell::editing_commands`), the same ones agents run. A drag sends
//! its command on each frame as a [`UiEvent::Gesture`], measured from where
//! the drag began, and the host runs the frames as one transaction, so the
//! whole drag is one undo step (arrangement-editing R13).
//!
//! The behaviour is in the arrangement editing spec.

use bl_core::time::Ticks;
use bl_model::{ClipData, ClipId, Document, TrackId};
use bl_shell::arrangement::editing::{earliest_move, resize_limits};
use bl_shell::arrangement::rows::{Row, first_row_at};
use bl_shell::arrangement::{ArrangementView, Horizontal};
use bl_shell::arrangement_commands as view_ids;
use bl_shell::editing_commands::{self as ids, Edge};
use bl_shell::locator_commands as locator_ids;
use bl_shell::{GesturePhase, Invocation};
use egui::{CursorIcon, Key, Modifiers, Pos2, Rect};
use serde_json::{Value, json};

use super::scene::{ShownClip, title_height};
use crate::main_window::UiEvent;

/// How close to a clip's edge, in points, a press resizes it rather than
/// moving it. Narrow clips keep their middle for moving.
pub const EDGE_GRAB: f32 = 4.0;

/// `clip.stretch`: stretch a warped clip (the warping spec, R47).
const STRETCH: &str = "clip.stretch";
/// `clip.slide_contents`: slide a clip's contents inside it (R54).
const SLIDE_CONTENTS: &str = "clip.slide_contents";

/// The `edge` argument [`STRETCH`] takes for an Arrangement edge.
fn stretch_edge(edge: Edge) -> &'static str {
    match edge {
        Edge::Start => "left",
        Edge::End => "right",
    }
}

/// Where on a clip the pointer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    /// The title bar, which moves the clip; the whole clip on a lane too
    /// short for one.
    Title,
    /// The body, under the title bar, where clicks and drags select time.
    Body,
    /// Its left edge, which resizes it.
    Start,
    /// Its right edge, which resizes it.
    End,
}

/// A clip under the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    /// The clip.
    pub clip: ClipId,
    /// Its track.
    pub track: TrackId,
    /// Where on it.
    pub zone: Zone,
}

/// What a frame's lanes look like, for hit-testing and positions.
pub(super) struct Lanes<'a> {
    pub view: &'a ArrangementView,
    pub doc: &'a Document,
    pub rect: Rect,
    pub horizontal: Horizontal,
    pub shown: &'a [ShownClip],
    pub rows: &'a [Row],
    pub scroll_y: f32,
}

impl Lanes<'_> {
    /// The position under a point.
    pub(super) fn at(&self, pos: Pos2) -> Ticks {
        self.horizontal.at(f64::from(pos.x - self.rect.left()))
    }

    /// The row under a point, if any.
    fn row_at(&self, pos: Pos2) -> Option<usize> {
        let y = pos.y - self.rect.top() + self.scroll_y;
        let index = first_row_at(self.rows, y);
        (index < self.rows.len() && self.rows[index].top <= y).then_some(index)
    }

    /// The track whose lane is under a point, if any.
    pub(super) fn track_at(&self, pos: Pos2) -> Option<TrackId> {
        self.row_at(pos).map(|row| self.rows[row].track)
    }

    /// The clip under a point, and where on it.
    #[must_use]
    pub fn hit(&self, pos: Pos2) -> Option<Hit> {
        if !self.rect.contains(pos) {
            return None;
        }
        // Later clips draw over earlier ones, so the last one wins.
        let shown = self.shown.iter().rev().find(|s| s.rect.contains(pos))?;
        let row = self.rows.iter().find(|r| r.track == shown.track)?;
        let rect = shown.rect;
        let grab = EDGE_GRAB.min(rect.width() / 4.0);
        let title = if row.folded {
            rect.height()
        } else {
            title_height(row.height)
        };
        let zone = if pos.x - rect.left() <= grab {
            Zone::Start
        } else if rect.right() - pos.x <= grab {
            Zone::End
        } else if title <= 0.0 || pos.y - rect.top() <= title {
            Zone::Title
        } else {
            Zone::Body
        };
        Some(Hit {
            clip: shown.clip,
            track: shown.track,
            zone,
        })
    }
}

/// The modifier keys that change a gesture, by what they do on this
/// platform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditKeys {
    /// Copy while dragging: Option on macOS, Ctrl on Windows and Linux
    /// (as S-SES-004 and S-NOTE-002 copy).
    pub copy: bool,
    /// Ignore snapping while dragging, or snap with it off: Cmd on macOS,
    /// Alt elsewhere (S-EDIT-013, S-GRID-007).
    pub bypass: bool,
    /// Add or remove one clip: Cmd-click, Ctrl-click (S-EDIT-010).
    pub toggle: bool,
    /// Extend the selection: Shift (S-EDIT-011, S-ZOOM-008).
    pub extend: bool,
    /// Select time inside a clip from its title bar: Shift with Option
    /// (Alt) (S-ARR-007).
    pub time_in_clip: bool,
    /// Slide a clip's contents inside the clip: Shift with Option (Alt), on
    /// the clip's body (S-ARR-005, I-DRAG-013); with `slide_bypass` as well,
    /// it doesn't snap (S-ARR-036).
    pub slide: bool,
    /// Slide a clip's contents without snapping: Cmd with Shift and Option on
    /// macOS, Ctrl with Shift and Alt on Windows and Linux (S-ARR-036,
    /// I-DRAG-014). The general `bypass` is Alt there, which is this
    /// gesture's slide key.
    pub slide_bypass: bool,
}

impl EditKeys {
    /// The keys as egui reports them.
    #[must_use]
    pub fn from(modifiers: Modifiers) -> Self {
        let mac = cfg!(target_os = "macos");
        Self {
            copy: if mac { modifiers.alt } else { modifiers.ctrl },
            bypass: if mac {
                modifiers.mac_cmd
            } else {
                modifiers.alt
            },
            toggle: modifiers.command,
            extend: modifiers.shift,
            time_in_clip: modifiers.shift && modifiers.alt,
            slide: modifiers.shift && modifiers.alt,
            slide_bypass: modifiers.command,
        }
    }
}

/// A drag that edits clips.
#[derive(Clone, Debug, PartialEq)]
enum ClipDrag {
    /// Moving or copying clips by a title bar.
    Move {
        anchor: ClipId,
        clips: Vec<ClipId>,
        /// From the anchor's start to where it was grabbed.
        grab: Ticks,
        /// The anchor's start when the drag began.
        start: Ticks,
        /// The anchor's track when the drag began.
        track: TrackId,
        /// The furthest the clips may go earlier.
        earliest: Ticks,
    },
    /// Moving clips' edges.
    Resize {
        anchor: ClipId,
        clips: Vec<ClipId>,
        edge: Edge,
        /// From the anchor's edge to where it was grabbed.
        grab: Ticks,
        /// The anchor's edge when the drag began.
        at: Ticks,
        /// How far the edges may move, earlier and later.
        limits: (Ticks, Ticks),
    },
    /// Shift-dragging a warped clip's edge, which stretches it (the warping
    /// spec, R47): the other edge stays put and the clip's contents scale.
    Stretch {
        clip: ClipId,
        /// The edge being dragged.
        edge: Edge,
    },
    /// Shift+Option-dragging a clip's contents, which slide inside it
    /// (R54): the clip stays put.
    Slide {
        clip: ClipId,
        /// The tick under the pointer when the drag began.
        from: Ticks,
    },
}

/// A resize from the keyboard (S-ARR-004): Enter, then Left and Right, then
/// Enter, or Esc to put it back.
#[derive(Clone, Debug, PartialEq)]
struct KeyResize {
    clips: Vec<ClipId>,
    by: Ticks,
    limits: (Ticks, Ticks),
}

/// The editing gestures under way between frames.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ClipGestures {
    drag: Option<ClipDrag>,
    key_resize: Option<KeyResize>,
}

fn gesture(events: &mut Vec<UiEvent>, phase: GesturePhase, id: &str, args: Value) {
    events.push(UiEvent::Gesture {
        phase,
        invocation: Some(Invocation::new(id, args)),
    });
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

impl ClipGestures {
    /// The clips a move drag is carrying, with the one grabbed first, if
    /// one is under way: released over the browser, one clip is saved as a
    /// clip file and several save nothing (clip files R3).
    #[must_use]
    pub fn moving(&self) -> Option<(ClipId, usize)> {
        match &self.drag {
            Some(ClipDrag::Move { anchor, clips, .. }) => Some((*anchor, clips.len())),
            _ => None,
        }
    }

    /// Whether a drag or a keyboard resize is under way.
    #[must_use]
    pub fn active(&self) -> bool {
        self.drag.is_some() || self.key_resize.is_some()
    }

    /// A drag starting at `press`: on a clip's title bar it moves the
    /// selection, the clip included, and on an edge it resizes. Returns
    /// whether it took the drag; anywhere else the lanes select time.
    pub fn begin(
        &mut self,
        lanes: &Lanes<'_>,
        press: Pos2,
        keys: EditKeys,
        events: &mut Vec<UiEvent>,
    ) -> bool {
        let Some(hit) = lanes.hit(press) else {
            return false;
        };
        let Some(clip) = lanes.doc.clip(hit.clip) else {
            return false;
        };
        let at = lanes.at(press);
        let selected = lanes.view.is_clip_selected(hit.clip);
        let clips = if selected {
            lanes.view.selected_clips().to_vec()
        } else {
            vec![hit.clip]
        };
        match hit.zone {
            // Spec: warping R54. Shift+Option on a clip's body slides its
            // contents inside it.
            Zone::Body if keys.slide => {
                if !selected {
                    invoke(events, ids::SELECT_CLIPS, json!({ "clips": [hit.clip] }));
                }
                self.drag = Some(ClipDrag::Slide {
                    clip: hit.clip,
                    from: at,
                });
            }
            Zone::Body => return false,
            Zone::Title if keys.time_in_clip => return false,
            Zone::Title => {
                if !selected {
                    // Spec: arrangement-editing R4: grabbing a clip that
                    // isn't selected selects it.
                    invoke(events, ids::SELECT_CLIPS, json!({ "clips": [hit.clip] }));
                }
                self.drag = Some(ClipDrag::Move {
                    anchor: hit.clip,
                    earliest: earliest_move(lanes.doc, &clips),
                    clips,
                    grab: at - clip.start,
                    start: clip.start,
                    track: hit.track,
                });
            }
            Zone::Start | Zone::End => {
                let edge = if hit.zone == Zone::Start {
                    Edge::Start
                } else {
                    Edge::End
                };
                if !selected {
                    invoke(events, ids::SELECT_CLIPS, json!({ "clips": [hit.clip] }));
                }
                // Spec: warping R47. Shift-dragging a warped clip's edge
                // stretches it instead of resizing it.
                let warped = matches!(&clip.data, ClipData::Audio(audio) if audio.warp);
                if keys.extend && warped {
                    self.drag = Some(ClipDrag::Stretch {
                        clip: hit.clip,
                        edge,
                    });
                    return true;
                }
                let position = match edge {
                    Edge::Start => clip.start,
                    Edge::End => clip.end(),
                };
                // A clip keeps at least one grid step, or a tick with
                // snapping off.
                let shortest = if lanes.view.grid().snap {
                    lanes.view.grid_ticks(lanes.doc)
                } else {
                    Ticks(1)
                };
                self.drag = Some(ClipDrag::Resize {
                    anchor: hit.clip,
                    limits: resize_limits(lanes.doc, &clips, edge, shortest),
                    clips,
                    edge,
                    grab: at - position,
                    at: position,
                });
            }
        }
        true
    }

    /// The drag's command for the pointer at `pointer`.
    fn frame(
        &self,
        lanes: &Lanes<'_>,
        pointer: Pos2,
        keys: EditKeys,
    ) -> Option<(&'static str, Value)> {
        let at = lanes.at(pointer);
        match self.drag.as_ref()? {
            ClipDrag::Move {
                anchor,
                clips,
                grab,
                start,
                track,
                earliest,
            } => {
                // Spec: arrangement-editing R4: the grabbed clip's start
                // lands on the grid.
                let snapped = lanes.view.snap_drag(lanes.doc, at - *grab, keys.bypass);
                let by = (snapped - *start).max(*earliest);
                let to_track = lanes
                    .row_at(pointer)
                    .map_or(*track, |row| lanes.rows[row].track);
                Some((
                    ids::MOVE_CLIPS,
                    json!({
                        "clips": clips,
                        "anchor": anchor,
                        "by": by,
                        "track": to_track,
                        "copy": keys.copy,
                    }),
                ))
            }
            ClipDrag::Resize {
                anchor,
                clips,
                edge,
                grab,
                at: from,
                limits,
            } => {
                let snapped = lanes.view.snap_drag(lanes.doc, at - *grab, keys.bypass);
                let by = (snapped - *from).clamp(limits.0, limits.1);
                Some((
                    ids::RESIZE_CLIPS,
                    json!({ "clips": clips, "anchor": anchor, "edge": edge, "by": by }),
                ))
            }
            // Spec: warping R47. The dragged edge goes where the pointer is,
            // snapped as a resize is.
            ClipDrag::Stretch { clip, edge } => {
                let at = lanes.view.snap_drag(lanes.doc, at, keys.bypass);
                Some((
                    STRETCH,
                    json!({ "clip": clip, "edge": stretch_edge(*edge), "at": at }),
                ))
            }
            // Spec: warping R54. The contents move by the drag, snapped to
            // the grid unless Cmd is held as well.
            ClipDrag::Slide { clip, from } => {
                let to = lanes.view.snap_drag(lanes.doc, at, keys.slide_bypass);
                let from = lanes.view.snap_drag(lanes.doc, *from, keys.slide_bypass);
                Some((SLIDE_CONTENTS, json!({ "clip": clip, "by": to - from })))
            }
        }
    }

    /// The drag went on to `pointer`.
    pub fn update(
        &self,
        lanes: &Lanes<'_>,
        pointer: Pos2,
        keys: EditKeys,
        events: &mut Vec<UiEvent>,
    ) {
        if let Some((id, args)) = self.frame(lanes, pointer, keys) {
            gesture(events, GesturePhase::Update, id, args);
        }
    }

    /// The drag ended at `pointer`: one undo step.
    pub fn end(
        &mut self,
        lanes: &Lanes<'_>,
        pointer: Option<Pos2>,
        keys: EditKeys,
        events: &mut Vec<UiEvent>,
    ) {
        let last = pointer.and_then(|pointer| self.frame(lanes, pointer, keys));
        if self.drag.take().is_some() {
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Commit,
                invocation: last.map(|(id, args)| Invocation::new(id, args)),
            });
        }
    }

    /// Esc during a drag or a keyboard resize: the set goes back.
    pub fn cancel(&mut self, events: &mut Vec<UiEvent>) {
        if self.drag.take().is_some() || self.key_resize.take().is_some() {
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Cancel,
                invocation: None,
            });
        }
    }

    /// A click at `pos`: on a title bar it selects the clip, with Cmd
    /// (Ctrl) adding or removing it and Shift extending the selection; with
    /// Shift elsewhere it extends the time selection. Returns whether it
    /// took the click; otherwise the lanes place the insert marker.
    pub fn click(
        &self,
        lanes: &Lanes<'_>,
        pos: Pos2,
        keys: EditKeys,
        events: &mut Vec<UiEvent>,
    ) -> bool {
        if let Some(hit) = lanes.hit(pos)
            && hit.zone != Zone::Body
            && !keys.time_in_clip
        {
            // Spec: arrangement-editing R1.
            let mode = if keys.toggle {
                "toggle"
            } else if keys.extend {
                "extend"
            } else {
                "replace"
            };
            invoke(
                events,
                ids::SELECT_CLIPS,
                json!({ "clips": [hit.clip], "mode": mode }),
            );
            return true;
        }
        if keys.extend {
            // Spec: arrangement-editing R2: Shift-click extends the time
            // selection to the click.
            let at = lanes.view.snap(lanes.doc, lanes.at(pos));
            let (from, row) = extend_from(lanes, at);
            let here = lanes.row_at(pos).or(row);
            invoke(
                events,
                view_ids::SELECT_TIME,
                json!({ "start": from, "end": at, "tracks": tracks_between(lanes, row, here) }),
            );
            return true;
        }
        false
    }

    /// The pointer's cursor over the lanes.
    #[must_use]
    pub fn cursor(&self, lanes: &Lanes<'_>, hover: Pos2, keys: EditKeys) -> Option<CursorIcon> {
        match &self.drag {
            Some(ClipDrag::Move { .. }) if keys.copy => return Some(CursorIcon::Copy),
            Some(ClipDrag::Move { .. }) => return Some(CursorIcon::Grabbing),
            Some(ClipDrag::Resize { .. }) => return Some(CursorIcon::ResizeHorizontal),
            Some(ClipDrag::Stretch { .. }) => return Some(CursorIcon::ResizeHorizontal),
            Some(ClipDrag::Slide { .. }) => return Some(CursorIcon::Grabbing),
            None => {}
        }
        match lanes.hit(hover)?.zone {
            Zone::Start | Zone::End => Some(CursorIcon::ResizeHorizontal),
            Zone::Title if !keys.time_in_clip => Some(CursorIcon::Grab),
            _ => None,
        }
    }

    /// The keys the Arrangement's lanes read themselves (each has two
    /// directions, which the keymap's one-command-per-shortcut can't say):
    /// Left and Right nudge the selected clips or move the insert marker
    /// (S-ARR-028), Option-Left and Option-Right (Alt) move the insert
    /// marker to the previous or next clip edge or locator (S-A11Y-001),
    /// Shift with an
    /// arrow extends the selection (S-A11Y-002), and Enter starts a
    /// keyboard resize of the selected clips' ends (S-ARR-004). Esc
    /// cancels a drag or a keyboard resize.
    pub fn keys(
        &mut self,
        lanes: &Lanes<'_>,
        input: &egui::InputState,
        title_bars: bool,
        events: &mut Vec<UiEvent>,
    ) {
        let pressed = |key: Key, shift: bool, alt: bool| {
            input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key { key: k, pressed: true, modifiers, .. }
                        if *k == key
                            && modifiers.shift == shift
                            && modifiers.alt == alt
                            && !modifiers.command
                            && !modifiers.ctrl
                )
            })
        };
        if pressed(Key::Escape, false, false) && self.active() {
            self.cancel(events);
            return;
        }
        if self.drag.is_some() {
            return;
        }
        let view = lanes.view;
        let doc = lanes.doc;
        let step = view.grid_ticks(doc);

        // A keyboard resize under way: arrows move the ends, Enter keeps it.
        if let Some(mut resize) = self.key_resize.take() {
            let mut by = resize.by;
            if pressed(Key::ArrowLeft, false, false) {
                by -= step;
            }
            if pressed(Key::ArrowRight, false, false) {
                by += step;
            }
            by = by.clamp(resize.limits.0, resize.limits.1);
            let args = json!({ "clips": resize.clips, "edge": "end", "by": by });
            if pressed(Key::Enter, false, false) {
                events.push(UiEvent::Gesture {
                    phase: GesturePhase::Commit,
                    invocation: Some(Invocation::new(ids::RESIZE_CLIPS, args)),
                });
            } else {
                if by != resize.by {
                    resize.by = by;
                    gesture(events, GesturePhase::Update, ids::RESIZE_CLIPS, args);
                }
                self.key_resize = Some(resize);
            }
            return;
        }
        if pressed(Key::Enter, false, false) && !view.selected_clips().is_empty() {
            let clips = view.selected_clips().to_vec();
            // Open the gesture straight away, so Esc reaches it rather than
            // its other binding.
            gesture(
                events,
                GesturePhase::Update,
                ids::RESIZE_CLIPS,
                json!({ "clips": clips, "edge": "end", "by": 0 }),
            );
            self.key_resize = Some(KeyResize {
                limits: resize_limits(doc, &clips, Edge::End, step),
                clips,
                by: Ticks::ZERO,
            });
            return;
        }

        // Spec: time-editing R7. While locators are selected, Left and
        // Right move them one grid step.
        let locators = lanes.view.selected_locators().to_vec();
        if !locators.is_empty() {
            let step = lanes.view.grid_ticks(lanes.doc);
            let mut moved = false;
            for (key, later) in [(Key::ArrowLeft, false), (Key::ArrowRight, true)] {
                if pressed(key, false, false) {
                    let first = locators[0];
                    let at = lanes
                        .doc
                        .locator(first)
                        .map_or(Ticks::ZERO, |locator| locator.position);
                    let to = if later { at + step } else { at - step };
                    invoke(
                        events,
                        locator_ids::MOVE,
                        json!({ "locator": first, "to": to.max(Ticks::ZERO) }),
                    );
                    moved = true;
                }
            }
            // Option-Left and Option-Right still move the insert marker
            // (the time editing spec, R12).
            if moved {
                return;
            }
        }
        if !title_bars {
            for (key, direction) in [(Key::ArrowLeft, "left"), (Key::ArrowRight, "right")] {
                if pressed(key, false, false) {
                    // Spec: arrangement-editing R12.
                    invoke(events, ids::NUDGE, json!({ "direction": direction }));
                }
            }
        }
        for (key, later) in [(Key::ArrowLeft, false), (Key::ArrowRight, true)] {
            if pressed(key, false, true)
                && let Some(at) = next_edge(lanes, view.insert_marker(), later)
            {
                let mut args = json!({ "at": at });
                if let Some(track) = view.selected_tracks().first() {
                    args["track"] = json!(track);
                }
                invoke(events, view_ids::SET_INSERT_MARKER, args);
            }
        }
        // Shift with Left or Right moves the selection's far end a grid
        // step; with Up or Down it takes in the track above or below.
        let (start, end) = view
            .time_selection()
            .unwrap_or((view.insert_marker(), view.insert_marker()));
        for (key, by) in [(Key::ArrowLeft, -step), (Key::ArrowRight, step)] {
            if pressed(key, true, false) {
                let tracks: Vec<TrackId> = view.selected_tracks().to_vec();
                invoke(
                    events,
                    view_ids::SELECT_TIME,
                    json!({ "start": start, "end": (end + by).max(Ticks::ZERO), "tracks": tracks }),
                );
            }
        }
        for (key, down) in [(Key::ArrowUp, false), (Key::ArrowDown, true)] {
            if pressed(key, true, false) {
                let rows: Vec<usize> = view
                    .selected_tracks()
                    .iter()
                    .filter_map(|t| lanes.rows.iter().position(|r| r.track == *t))
                    .collect();
                let next = if down {
                    rows.iter().max().map(|r| r + 1)
                } else {
                    rows.iter().min().and_then(|r| r.checked_sub(1))
                };
                if let Some(row) = next.filter(|r| *r < lanes.rows.len()) {
                    let mut tracks = view.selected_tracks().to_vec();
                    tracks.push(lanes.rows[row].track);
                    if end > start {
                        invoke(
                            events,
                            view_ids::SELECT_TIME,
                            json!({ "start": start, "end": end, "tracks": tracks }),
                        );
                    } else {
                        invoke(events, view_ids::SELECT_TRACKS, json!({ "tracks": tracks }));
                    }
                }
            }
        }
    }
}

/// Where a Shift-click or Shift-drag extends the time selection from: its
/// far end from `at`, or the insert marker, and the first selected track's
/// row.
pub(super) fn extend_from(lanes: &Lanes<'_>, at: Ticks) -> (Ticks, Option<usize>) {
    let view = lanes.view;
    let from = match view.time_selection() {
        Some((start, _)) if at >= start => start,
        Some((_, end)) => end,
        None => view.insert_marker(),
    };
    let row = view
        .selected_tracks()
        .iter()
        .filter_map(|t| lanes.rows.iter().position(|r| r.track == *t))
        .min();
    (from, row)
}

/// The tracks of the rows from `a` to `b`.
fn tracks_between(lanes: &Lanes<'_>, a: Option<usize>, b: Option<usize>) -> Vec<TrackId> {
    match (a, b) {
        (Some(a), Some(b)) => {
            let (a, b) = (a.min(b), a.max(b));
            lanes.rows[a..=b].iter().map(|r| r.track).collect()
        }
        (Some(a), None) | (None, Some(a)) => vec![lanes.rows[a].track],
        (None, None) => Vec::new(),
    }
}

/// The nearest clip edge before (or after, with `later`) `from` on the
/// selected tracks, or on every track if none is selected.
fn next_edge(lanes: &Lanes<'_>, from: Ticks, later: bool) -> Option<Ticks> {
    let view = lanes.view;
    let doc = lanes.doc;
    let tracks: Vec<TrackId> = if view.selected_tracks().is_empty() {
        lanes.rows.iter().map(|r| r.track).collect()
    } else {
        view.selected_tracks().to_vec()
    };
    let edges = tracks
        .iter()
        .filter_map(|t| doc.track(*t))
        .flat_map(|t| t.clips().flat_map(|c| [c.start, c.end()]));
    // Spec: time-editing R12. Option-Left and Option-Right stop at the
    // locators as well as at the clip edges.
    let edges = edges.chain(doc.locators().iter().map(|locator| locator.position));
    if later {
        edges.filter(|at| *at > from).min()
    } else {
        edges.filter(|at| *at < from).max()
    }
}

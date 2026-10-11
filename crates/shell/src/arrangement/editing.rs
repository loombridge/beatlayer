//! The Arrangement's edits (WP-1.8b): selecting clips, moving and copying
//! them, resizing, splitting, deleting, duplicating, and the clipboard.
//!
//! Every edit runs as one transaction of the document's own commands, so it
//! is one undo step, and a track never ends up playing two clips at once:
//! whatever an edit places over a clip replaces the part it covers
//! ([`super::placing`]). The selection and the clipboard aren't in the document, so
//! selecting and copying are never undo steps.
//!
//! People reach these through keys, menus, and gestures, which leave the
//! targets to the selection; agents and tests give clips, tracks, and
//! positions explicitly. A drag runs the same command again on each frame
//! from where the drag began, inside one transaction (see
//! `Shell::gesture`). The behaviour is in the arrangement editing spec.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use bl_core::time::Ticks;
use bl_model::{
    Change, ChangeSet, Clip, ClipData, ClipId, Command, Document, Editor, TrackId, TrackKind,
};
use bl_shell_spec::arrangement::TimeRange;
use bl_shell_spec::editing::{
    self as ids, ClipboardState, Direction, Edge, EditResult, LoopSelectionArgs, MoveClipsArgs,
    NudgeArgs, PasteArgs, Position, ResizeClipsArgs, SelectClipsArgs, SelectMode, SplitArgs,
    TargetArgs,
};
use serde::Serialize;
use serde_json::Value;

use super::placing::{clear, clear_spans, piece, split_off};
use super::{ArrangementView, parse};

/// Why an edit didn't happen. The document is as it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditFailure {
    /// The arguments don't fit the command.
    InvalidArgs(String),
    /// The command can't run now, for the reason given (nothing selected,
    /// nothing on the clipboard).
    Unavailable(String),
    /// The edit was refused, for the reason given, and changed nothing.
    Refused(String),
}

impl From<super::ViewError> for EditFailure {
    fn from(error: super::ViewError) -> Self {
        match error {
            super::ViewError::InvalidArgs(message) => Self::InvalidArgs(message),
            super::ViewError::Unavailable(reason) => Self::Unavailable(reason),
        }
    }
}

impl From<bl_model::EditError> for EditFailure {
    fn from(error: bl_model::EditError) -> Self {
        Self::Refused(error.to_string())
    }
}

/// Material copied from the Arrangement, for pasting: each track's clips,
/// placed from the material's start.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Clipboard {
    /// Each track's clips, with the track's distance from the first in the
    /// track list. A clip's `start` counts from the material's start.
    pub tracks: Vec<(usize, Vec<Clip>)>,
    /// How long the material is.
    pub length: Ticks,
    /// The tracks it was copied from, in order, to paste onto when no
    /// track is selected.
    pub source: Vec<TrackId>,
}

impl Clipboard {
    /// What it holds, as `arrangement.copy` reports it.
    #[must_use]
    pub fn state(&self) -> ClipboardState {
        ClipboardState {
            clips: self.tracks.iter().map(|(_, clips)| clips.len()).sum(),
            tracks: self.tracks.len(),
            length: self.length,
        }
    }
}

/// What an edit acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Target {
    /// Whole clips.
    Clips(Vec<ClipId>),
    /// The material in a span of time on tracks.
    Time {
        tracks: Vec<TrackId>,
        start: Ticks,
        end: Ticks,
    },
}

/// The tracks of the scrolling lanes, in display order: every track but the
/// returns and Main. Track distances count along this list.
#[must_use]
pub fn lanes(doc: &Document) -> Vec<TrackId> {
    doc.track_order()
        .into_iter()
        .filter(|id| {
            doc.track(*id)
                .is_some_and(|t| !matches!(t.kind(), TrackKind::Return | TrackKind::Main))
        })
        .collect()
}

/// Each lane's place in [`lanes`], for looking many up.
fn lane_positions(doc: &Document) -> HashMap<TrackId, usize> {
    lanes(doc)
        .into_iter()
        .enumerate()
        .map(|(index, track)| (track, index))
        .collect()
}

fn lane_index(lanes: &[TrackId], track: TrackId) -> Option<usize> {
    lanes.iter().position(|id| *id == track)
}

/// Whether a clip with this data can go on this track.
fn fits(doc: &Document, track: TrackId, data: &ClipData) -> bool {
    doc.track(track).is_some_and(|t| match t.kind() {
        TrackKind::Audio => data.is_audio(),
        TrackKind::Midi => !data.is_audio(),
        _ => false,
    })
}

fn invalid(message: impl Into<String>) -> EditFailure {
    EditFailure::InvalidArgs(message.into())
}

fn unavailable(reason: impl Into<String>) -> EditFailure {
    EditFailure::Unavailable(reason.into())
}

fn refused(reason: impl Into<String>) -> EditFailure {
    EditFailure::Refused(reason.into())
}

fn apply(editor: &mut Editor, command: Command) -> Result<(), EditFailure> {
    editor.apply(command).map(|_| ())?;
    Ok(())
}

/// Clears where moved clips landed, leaving the moved clips alone: each
/// destination track is looked through once, however many clips land on it.
fn clear_moved(
    editor: &mut Editor,
    clips: &[ClipId],
    moves: &[(ClipId, TrackId, Ticks, Ticks)],
) -> Result<(), EditFailure> {
    let keep: BTreeSet<ClipId> = clips.iter().copied().collect();
    let mut spans: BTreeMap<TrackId, Vec<(Ticks, Ticks)>> = BTreeMap::new();
    for (_, track, start, end) in moves {
        spans.entry(*track).or_default().push((*start, *end));
    }
    for (track, spans) in spans {
        clear_spans(editor, track, &spans, &keep)?;
    }
    Ok(())
}

/// Puts new clips on tracks, each replacing what it covers, and returns
/// their IDs. Each clip's own ID is replaced with a new one.
fn place(
    editor: &mut Editor,
    placements: Vec<(TrackId, Vec<Clip>)>,
) -> Result<Vec<ClipId>, EditFailure> {
    let doc = editor.document();
    for (track, clips) in &placements {
        if let Some(clip) = clips.iter().find(|c| !fits(doc, *track, &c.data)) {
            let what = if clip.data.is_audio() {
                "an audio clip"
            } else {
                "a MIDI clip"
            };
            let name = doc.track(*track).map_or("", |t| t.name());
            return Err(refused(format!(
                "{what} can't go on track \"{name}\" (ID {}): audio clips go on audio tracks \
                 and MIDI clips on MIDI tracks",
                track.0
            )));
        }
        if let Some(clip) = clips.iter().find(|c| c.start.is_negative()) {
            return Err(refused(format!(
                "a clip would start at {}, before 1.1.1",
                clip.start
            )));
        }
    }
    let mut created = Vec::new();
    let mut placed = BTreeSet::new();
    for (track, clips) in placements {
        for mut clip in clips {
            clip.id = editor.new_id();
            clear(editor, track, clip.start, clip.end(), &placed)?;
            placed.insert(clip.id);
            created.push(clip.id);
            apply(editor, Command::CreateClip { track, clip })?;
        }
    }
    Ok(created)
}

/// Runs `edit` as one transaction labelled `label`, returning its result
/// and whether it recorded an undo step. A failure puts the document back.
fn transaction<T>(
    editor: &mut Editor,
    label: &'static str,
    edit: impl FnOnce(&mut Editor) -> Result<T, EditFailure>,
) -> Result<(T, bool), EditFailure> {
    editor.begin(label);
    match edit(editor) {
        Ok(value) => {
            let recorded = editor.commit()?;
            Ok((value, recorded))
        }
        Err(error) => {
            editor.cancel().ok();
            Err(error)
        }
    }
}

/// The clips to act on: those given, or the selected ones. Each must exist;
/// a clip listed twice counts once.
fn clips_or_selected(
    doc: &Document,
    view: &ArrangementView,
    clips: Option<Vec<ClipId>>,
) -> Result<Vec<ClipId>, EditFailure> {
    let given = clips.is_some();
    let clips = clips.unwrap_or_else(|| view.selected_clips.clone());
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(clips.len());
    for clip in clips {
        if doc.clip(clip).is_none() {
            return Err(invalid(format!("no clip has the ID {}", clip.0)));
        }
        if seen.insert(clip) {
            out.push(clip);
        }
    }
    if out.is_empty() {
        return Err(if given {
            invalid("`clips` is empty")
        } else {
            unavailable("no clip is selected")
        });
    }
    Ok(out)
}

fn tracks_exist(doc: &Document, tracks: &[TrackId]) -> Result<(), EditFailure> {
    match tracks.iter().find(|id| doc.track(**id).is_none()) {
        Some(missing) => Err(invalid(format!("no track has the ID {}", missing.0))),
        None => Ok(()),
    }
}

/// What a command's target arguments, or the selection, name.
fn target(doc: &Document, view: &ArrangementView, args: TargetArgs) -> Result<Target, EditFailure> {
    let TargetArgs {
        clips,
        tracks,
        start,
        end,
    } = args;
    if let Some(clips) = clips {
        if tracks.is_some() || start.is_some() || end.is_some() {
            return Err(invalid(
                "give `clips`, or a span of time (`tracks`, `start`, `end`), not both",
            ));
        }
        return Ok(Target::Clips(clips_or_selected(doc, view, Some(clips))?));
    }
    if tracks.is_none() && start.is_none() && end.is_none() && !view.selected_clips.is_empty() {
        return Ok(Target::Clips(clips_or_selected(doc, view, None)?));
    }
    let signatures = &doc.settings().time_signatures;
    let selection = view.time_selection;
    let start = match start {
        Some(start) => start.resolve(signatures).map_err(invalid)?,
        None => selection
            .map(|(start, _)| start)
            .ok_or_else(|| unavailable("nothing is selected"))?,
    };
    let end = match end {
        Some(end) => end.resolve(signatures).map_err(invalid)?,
        None => selection
            .map(|(_, end)| end)
            .ok_or_else(|| unavailable("no time is selected"))?,
    };
    if end <= start {
        return Err(invalid("the span's `end` must be after its `start`"));
    }
    let tracks = tracks.unwrap_or_else(|| view.selected.clone());
    tracks_exist(doc, &tracks)?;
    if tracks.is_empty() {
        return Err(unavailable("no track is selected"));
    }
    Ok(Target::Time { tracks, start, end })
}

/// The material a target names, by track in display order, with absolute
/// positions, and the span it covers.
fn material(doc: &Document, target: &Target) -> (Vec<(TrackId, Vec<Clip>)>, Ticks, Ticks) {
    let lanes = lanes(doc);
    let order = |track: &TrackId| lane_index(&lanes, *track).unwrap_or(usize::MAX);
    match target {
        Target::Clips(ids) => {
            let mut by_track: Vec<(TrackId, Vec<Clip>)> = Vec::new();
            let (mut start, mut end) = (Ticks::MAX, Ticks::MIN);
            for id in ids {
                let (Some(clip), Some(track)) = (doc.clip(*id), doc.clip_track(*id)) else {
                    continue;
                };
                start = start.min(clip.start);
                end = end.max(clip.end());
                match by_track.iter_mut().find(|(t, _)| *t == track) {
                    Some((_, clips)) => clips.push(clip.clone()),
                    None => by_track.push((track, vec![clip.clone()])),
                }
            }
            by_track.sort_by_key(|(track, _)| order(track));
            for (_, clips) in &mut by_track {
                clips.sort_by_key(|c| (c.start, c.id));
            }
            (by_track, start, end)
        }
        Target::Time { tracks, start, end } => {
            let mut tracks = tracks.clone();
            tracks.sort_by_key(order);
            tracks.dedup();
            let by_track = tracks
                .into_iter()
                .map(|track| {
                    let mut clips: Vec<Clip> = doc.track(track).map_or_else(Vec::new, |t| {
                        t.clips()
                            .filter(|c| c.start < *end && c.end() > *start)
                            .map(|c| piece(c, c.start.max(*start), c.end().min(*end)))
                            .collect()
                    });
                    clips.sort_by_key(|c| (c.start, c.id));
                    (track, clips)
                })
                .collect();
            (by_track, *start, *end)
        }
    }
}

/// The clipboard holding a target's material.
fn clipboard_of(doc: &Document, target: &Target) -> Clipboard {
    let lanes = lanes(doc);
    let (by_track, start, end) = material(doc, target);
    let first = by_track
        .first()
        .and_then(|(track, _)| lane_index(&lanes, *track))
        .unwrap_or(0);
    let tracks = by_track
        .iter()
        .map(|(track, clips)| {
            let distance = lane_index(&lanes, *track).unwrap_or(first) - first;
            let clips = clips
                .iter()
                .map(|clip| Clip {
                    start: clip.start - start,
                    ..clip.clone()
                })
                .collect();
            (distance, clips)
        })
        .collect();
    Clipboard {
        tracks,
        length: end - start,
        source: by_track.iter().map(|(track, _)| *track).collect(),
    }
}

/// Deletes a target's material.
fn delete_target(editor: &mut Editor, target: &Target) -> Result<(), EditFailure> {
    match target {
        Target::Clips(clips) => {
            for clip in clips {
                apply(editor, Command::DeleteClip { clip: *clip })?;
            }
            Ok(())
        }
        Target::Time { tracks, start, end } => {
            for track in tracks {
                clear(editor, *track, *start, *end, &BTreeSet::new())?;
            }
            Ok(())
        }
    }
}

/// The span the clips cover and their tracks in display order.
fn span_of(doc: &Document, clips: &[ClipId]) -> Option<(Ticks, Ticks, Vec<TrackId>)> {
    let positions = lane_positions(doc);
    let tracks: BTreeSet<TrackId> = clips.iter().filter_map(|c| doc.clip_track(*c)).collect();
    let mut tracks: Vec<TrackId> = tracks.into_iter().collect();
    tracks.sort_by_key(|t| positions.get(t).copied().unwrap_or(usize::MAX));
    let (mut start, mut end) = (Ticks::MAX, Ticks::MIN);
    for clip in clips.iter().filter_map(|c| doc.clip(*c)) {
        start = start.min(clip.start);
        end = end.max(clip.end());
    }
    (start <= end).then_some((start, end, tracks))
}

impl ArrangementView {
    /// The selected clips, in the order they were selected (WP-1.8b).
    #[must_use]
    pub fn selected_clips(&self) -> &[ClipId] {
        &self.selected_clips
    }

    /// Whether a clip is selected.
    #[must_use]
    pub fn is_clip_selected(&self, clip: ClipId) -> bool {
        self.selected_clips.contains(&clip)
    }

    /// Selects clips, their tracks, and the time they span, as clicking
    /// their title bars does. An empty list clears the clip selection and
    /// leaves the rest.
    pub(crate) fn select_clips(&mut self, doc: &Document, clips: Vec<ClipId>) {
        self.selected_clips = clips;
        if let Some((start, end, tracks)) = span_of(doc, &self.selected_clips) {
            self.time_selection = Some((start, end));
            self.insert_marker = start;
            self.selected = tracks;
            self.title_bars = false;
            self.follow_paused = false;
        }
    }

    /// Selects time on tracks, with no clips, as dragging across the lanes
    /// does.
    pub(crate) fn select_span(&mut self, tracks: Vec<TrackId>, start: Ticks, end: Ticks) {
        self.selected_clips.clear();
        self.selected = tracks;
        self.time_selection = (end > start).then_some((start, end));
        self.insert_marker = start;
        self.title_bars = false;
    }

    /// Takes the selection from another view: the clips, tracks, time, and
    /// insert marker, leaving zoom and scrolling as they are. A gesture
    /// starts each frame from the selection it began with.
    pub(crate) fn restore_selection(&mut self, from: &ArrangementView) {
        self.selected_clips.clone_from(&from.selected_clips);
        self.selected.clone_from(&from.selected);
        self.time_selection = from.time_selection;
        self.insert_marker = from.insert_marker;
        self.title_bars = from.title_bars;
    }

    /// Drops selected clips the document no longer has.
    pub(crate) fn forget_missing_clips(&mut self, doc: &Document) {
        self.selected_clips.retain(|id| doc.clip(*id).is_some());
    }

    /// The editing grid's spacing at the current zoom, in ticks: how far
    /// Left and Right nudge.
    #[must_use]
    pub fn grid_ticks(&self, doc: &Document) -> Ticks {
        self.grid_step(doc).ticks.max(Ticks(1))
    }

    /// Where something dragged to `at` lands: on the nearest grid line while
    /// snapping is on, and freely while it's off. Holding Cmd (Alt on
    /// Windows) during the drag, `invert`, does the opposite (I-DRAG-021;
    /// arrangement-editing R4).
    #[must_use]
    pub fn snap_drag(&self, doc: &Document, at: Ticks, invert: bool) -> Ticks {
        if self.grid.snap == invert {
            at
        } else {
            super::grid::snap(&doc.settings().time_signatures, self.grid_step(doc), at)
        }
    }
}

/// The furthest a drag may move clips earlier: as far as the earliest of
/// them is from 1.1.1, so none starts before it. For the UI, which clamps
/// its drags rather than have the edit refused.
#[must_use]
pub fn earliest_move(doc: &Document, clips: &[ClipId]) -> Ticks {
    -clips
        .iter()
        .filter_map(|c| doc.clip(*c))
        .map(|c| c.start)
        .min()
        .unwrap_or(Ticks::ZERO)
}

/// How far a drag may move clips' `edge`, earlier and later, so each keeps
/// at least `shortest` of length and none starts before 1.1.1 or before its
/// contents begin. For the UI, which clamps its drags.
#[must_use]
pub fn resize_limits(
    doc: &Document,
    clips: &[ClipId],
    edge: Edge,
    shortest: Ticks,
) -> (Ticks, Ticks) {
    let mut earliest = Ticks::MIN;
    let mut latest = Ticks::MAX;
    for clip in clips.iter().filter_map(|c| doc.clip(*c)) {
        let room = (clip.length - shortest).max(Ticks::ZERO);
        match edge {
            Edge::End => earliest = earliest.max(-room),
            Edge::Start => {
                earliest = earliest.max(-clip.offset.min(clip.start));
                latest = latest.min(room);
            }
        }
    }
    (earliest.min(Ticks::ZERO), latest.max(Ticks::ZERO))
}

fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// What a finished edit reports: what changed between `before` and the
/// document now, and the selection.
fn result(
    before: &Document,
    editor: &Editor,
    view: &ArrangementView,
    undo_step: bool,
) -> EditResult {
    let changes = ChangeSet::between(before, editor.document());
    let of = |kind: Change| {
        changes
            .clips
            .iter()
            .filter(|(_, change)| **change == kind)
            .map(|(id, _)| *id)
            .collect()
    };
    EditResult {
        created: of(Change::Added),
        changed: of(Change::Modified),
        deleted: of(Change::Removed),
        selected_clips: view.selected_clips.clone(),
        time_selection: view
            .time_selection
            .map(|(start, end)| TimeRange { start, end }),
        selected_tracks: view.selected.clone(),
        undo_step,
    }
}

/// The name of the undo step an editing command makes, such as "Copy
/// clips".
#[must_use]
pub fn label(id: &str, args: &Value) -> &'static str {
    match id {
        ids::MOVE_CLIPS if args.get("copy").and_then(Value::as_bool) == Some(true) => "Copy clips",
        ids::MOVE_CLIPS | ids::NUDGE => "Move clips",
        ids::RESIZE_CLIPS => "Resize clips",
        ids::SPLIT => "Split clips",
        ids::DELETE => "Delete",
        ids::DUPLICATE => "Duplicate",
        ids::CUT => "Cut",
        ids::PASTE => "Paste",
        ids::LOOP_SELECTION => "Loop selection",
        _ => "Select",
    }
}

/// Whether a person can run editing command `id` with `args` now: there's
/// something selected to act on, or on the clipboard to paste.
#[must_use]
pub fn can_run(
    id: &str,
    args: &Value,
    doc: &Document,
    view: &ArrangementView,
    clipboard: Option<&Clipboard>,
) -> bool {
    let has_selection = !view.selected_clips.is_empty()
        || (view.time_selection.is_some() && !view.selected.is_empty());
    let given = |key: &str| args.get(key).is_some();
    match id {
        ids::SELECT_CLIPS | ids::SELECT_ALL | ids::LOOP_SELECTION | ids::SELECT_LOOP => true,
        ids::MOVE_CLIPS | ids::RESIZE_CLIPS => given("clips") || !view.selected_clips.is_empty(),
        ids::NUDGE => true,
        ids::SPLIT => {
            given("clips")
                || given("tracks")
                || !view.selected_clips.is_empty()
                || !view.selected.is_empty()
        }
        ids::DELETE | ids::DUPLICATE | ids::COPY | ids::CUT => {
            given("clips") || given("start") || has_selection
        }
        ids::PASTE => {
            clipboard.is_some_and(|c| !c.tracks.is_empty())
                && (given("track") || !view.selected.is_empty() || !lanes(doc).is_empty())
        }
        _ => false,
    }
}

/// Runs editing command `id` with `args`, as `Shell::run` does: an edit
/// returns an [`EditResult`], a selection the view's state, and a copy what
/// the clipboard holds.
///
/// # Errors
///
/// [`EditFailure`] saying why nothing changed.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per command keeps each one's behaviour in sight"
)]
pub fn run(
    id: &str,
    args: Value,
    editor: &mut Editor,
    view: &mut ArrangementView,
    clipboard: &mut Option<Clipboard>,
) -> Result<Value, EditFailure> {
    let before = editor.document().clone();
    let doc = &before;
    match id {
        ids::SELECT_CLIPS => {
            let SelectClipsArgs { clips, mode } = parse(args)?;
            let clips = if clips.is_empty() {
                Vec::new()
            } else {
                clips_or_selected(doc, view, Some(clips))?
            };
            let mut selected = view.selected_clips.clone();
            match mode {
                SelectMode::Replace => selected = clips,
                SelectMode::Add => {
                    for clip in clips {
                        if !selected.contains(&clip) {
                            selected.push(clip);
                        }
                    }
                }
                SelectMode::Toggle => {
                    for clip in clips {
                        if let Some(at) = selected.iter().position(|c| *c == clip) {
                            selected.remove(at);
                        } else {
                            selected.push(clip);
                        }
                    }
                }
                SelectMode::Extend => match selected.first().copied() {
                    None => selected = clips,
                    Some(first) => {
                        let mut ends = clips.clone();
                        ends.push(first);
                        if let Some((start, end, tracks)) = span_of(doc, &ends) {
                            let lanes = lanes(doc);
                            let rows: Vec<usize> = tracks
                                .iter()
                                .filter_map(|t| lane_index(&lanes, *t))
                                .collect();
                            let (top, bottom) = (
                                rows.iter().copied().min().unwrap_or(0),
                                rows.iter().copied().max().unwrap_or(0),
                            );
                            for track in lanes.get(top..=bottom).unwrap_or_default() {
                                let Some(lane) = doc.track(*track) else {
                                    continue;
                                };
                                let mut inside: Vec<&Clip> = lane
                                    .clips()
                                    .filter(|c| c.start >= start && c.end() <= end)
                                    .collect();
                                inside.sort_by_key(|c| c.start);
                                for clip in inside {
                                    if !selected.contains(&clip.id) {
                                        selected.push(clip.id);
                                    }
                                }
                            }
                        }
                    }
                },
            }
            if selected.is_empty() {
                view.selected_clips.clear();
            } else {
                view.select_clips(doc, selected);
            }
            Ok(to_value(&view.state(doc)))
        }
        ids::SELECT_ALL => {
            parse::<bl_commands::NoArgs>(args)?;
            let lanes = lanes(doc);
            let mut all: Vec<&Clip> = lanes
                .iter()
                .filter_map(|t| doc.track(*t))
                .flat_map(bl_model::Track::clips)
                .collect();
            all.sort_by_key(|c| (c.start, c.id));
            let clips: Vec<ClipId> = all.iter().map(|c| c.id).collect();
            if clips.is_empty() {
                let holders: Vec<TrackId> = lanes
                    .into_iter()
                    .filter(|t| doc.track(*t).is_some_and(|t| t.kind().holds_clips()))
                    .collect();
                view.select_span(holders, Ticks::ZERO, view.arrangement_end(doc));
            } else {
                view.select_clips(doc, clips);
            }
            Ok(to_value(&view.state(doc)))
        }
        ids::MOVE_CLIPS => {
            let MoveClipsArgs {
                clips,
                by,
                to,
                anchor,
                track,
                copy,
            } = parse(args)?;
            let clips = clips_or_selected(doc, view, clips)?;
            let lanes = lanes(doc);
            let anchor = match anchor {
                Some(anchor) if clips.contains(&anchor) => anchor,
                Some(anchor) => {
                    return Err(invalid(format!(
                        "the anchor, clip {}, isn't one of the clips",
                        anchor.0
                    )));
                }
                None => earliest(doc, &lanes, &clips),
            };
            let anchor_clip = doc.clip(anchor).expect("the clips exist");
            let by = match (by, to) {
                (Some(_), Some(_)) => return Err(invalid("give `by` or `to`, not both")),
                (Some(by), None) => by,
                (None, Some(to)) => {
                    to.resolve(&doc.settings().time_signatures)
                        .map_err(invalid)?
                        - anchor_clip.start
                }
                (None, None) => Ticks::ZERO,
            };
            let shift = match track {
                None => 0,
                Some(track) => {
                    tracks_exist(doc, &[track])?;
                    let to = lane_index(&lanes, track).ok_or_else(|| {
                        refused("clips can't go on a return track or the Main track")
                    })?;
                    let from = doc
                        .clip_track(anchor)
                        .and_then(|t| lane_index(&lanes, t))
                        .unwrap_or(to);
                    to as isize - from as isize
                }
            };
            // Where each clip goes, checked before anything changes.
            let positions = lane_positions(doc);
            let mut moves = Vec::with_capacity(clips.len());
            for clip in &clips {
                let value = doc.clip(*clip).expect("the clips exist");
                let from = doc
                    .clip_track(*clip)
                    .and_then(|t| positions.get(&t).copied())
                    .unwrap_or(0);
                let destination = from
                    .checked_add_signed(shift)
                    .and_then(|i| lanes.get(i).copied())
                    .ok_or_else(|| refused("there's no track that far up or down"))?;
                if !fits(doc, destination, &value.data) {
                    return Err(refused(
                        "a clip would land on a track of another kind: audio clips go on audio \
                         tracks and MIDI clips on MIDI tracks",
                    ));
                }
                let start = value
                    .start
                    .checked_add(by)
                    .filter(|start| !start.is_negative())
                    .ok_or_else(|| refused("a clip would start before 1.1.1"))?;
                moves.push((*clip, destination, start, start + value.length));
            }
            let label = if copy { "Copy clips" } else { "Move clips" };
            let (moved, recorded) = transaction(editor, label, |editor| {
                if copy {
                    let placements = moves
                        .iter()
                        .map(|(clip, track, start, _)| {
                            let value = editor.document().clip(*clip).expect("the clips exist");
                            (
                                *track,
                                vec![Clip {
                                    start: *start,
                                    ..value.clone()
                                }],
                            )
                        })
                        .collect();
                    place(editor, placements)
                } else if shift == 0 {
                    // In time only: one command, however many clips.
                    if by != Ticks::ZERO {
                        apply(
                            editor,
                            Command::MoveClips {
                                clips: clips.clone(),
                                by,
                            },
                        )?;
                    }
                    clear_moved(editor, &clips, &moves)?;
                    Ok(clips.clone())
                } else {
                    for (clip, track, start, _) in &moves {
                        apply(
                            editor,
                            Command::MoveClip {
                                clip: *clip,
                                track: *track,
                                start: *start,
                            },
                        )?;
                    }
                    clear_moved(editor, &clips, &moves)?;
                    Ok(clips.clone())
                }
            })?;
            view.select_clips(editor.document(), moved);
            Ok(to_value(&result(doc, editor, view, recorded)))
        }
        ids::RESIZE_CLIPS => {
            let ResizeClipsArgs {
                clips,
                edge,
                by,
                to,
                anchor,
            } = parse(args)?;
            let clips = clips_or_selected(doc, view, clips)?;
            let lanes = lanes(doc);
            let anchor = match anchor {
                Some(anchor) if clips.contains(&anchor) => anchor,
                Some(anchor) => {
                    return Err(invalid(format!(
                        "the anchor, clip {}, isn't one of the clips",
                        anchor.0
                    )));
                }
                None => earliest(doc, &lanes, &clips),
            };
            let anchor_clip = doc.clip(anchor).expect("the clips exist");
            let by = match (by, to) {
                (Some(by), None) => by,
                (None, Some(to)) => {
                    let at = to
                        .resolve(&doc.settings().time_signatures)
                        .map_err(invalid)?;
                    at - match edge {
                        Edge::Start => anchor_clip.start,
                        Edge::End => anchor_clip.end(),
                    }
                }
                _ => return Err(invalid("give `by` or `to`")),
            };
            for clip in clips.iter().filter_map(|c| doc.clip(*c)) {
                match edge {
                    Edge::End if clip.length + by <= Ticks::ZERO => {
                        return Err(refused("a clip would be shorter than a tick"));
                    }
                    Edge::Start if by >= clip.length => {
                        return Err(refused("a clip would be shorter than a tick"));
                    }
                    Edge::Start if (clip.start + by).is_negative() => {
                        return Err(refused("a clip would start before 1.1.1"));
                    }
                    Edge::Start if (clip.offset + by).is_negative() => {
                        return Err(refused(
                            "a clip would start before its contents begin; it can't show more \
                             of them than they hold",
                        ));
                    }
                    _ => {}
                }
            }
            let (_, recorded) = transaction(editor, "Resize clips", |editor| {
                let keep: BTreeSet<ClipId> = clips.iter().copied().collect();
                for clip in &clips {
                    let value = editor
                        .document()
                        .clip(*clip)
                        .expect("the clips exist")
                        .clone();
                    let track = editor.document().clip_track(*clip).expect("on a track");
                    match edge {
                        Edge::End => {
                            apply(
                                editor,
                                Command::SetClipLength {
                                    clip: *clip,
                                    length: value.length + by,
                                },
                            )?;
                            clear(editor, track, value.end(), value.end() + by, &keep)?;
                        }
                        Edge::Start => {
                            apply(
                                editor,
                                Command::TrimClipStart {
                                    clip: *clip,
                                    start: value.start + by,
                                },
                            )?;
                            clear(editor, track, value.start + by, value.start, &keep)?;
                        }
                    }
                }
                Ok(())
            })?;
            view.select_clips(editor.document(), clips);
            Ok(to_value(&result(doc, editor, view, recorded)))
        }
        ids::SPLIT => {
            let SplitArgs { clips, tracks, at } = parse(args)?;
            let signatures = &doc.settings().time_signatures;
            let mut positions: Vec<Ticks> = match at {
                Some(at) => at
                    .iter()
                    .map(|p| p.resolve(signatures))
                    .collect::<Result<_, _>>()
                    .map_err(invalid)?,
                None => match view.time_selection {
                    Some((start, end)) if view.selected_clips.is_empty() => vec![start, end],
                    _ => vec![view.insert_marker],
                },
            };
            positions.sort_unstable();
            positions.dedup();
            let targets: Vec<ClipId> = match (clips, tracks) {
                (Some(_), Some(_)) => return Err(invalid("give `clips` or `tracks`, not both")),
                (Some(clips), None) => clips_or_selected(doc, view, Some(clips))?,
                (None, Some(tracks)) => {
                    tracks_exist(doc, &tracks)?;
                    clips_on(doc, &tracks)
                }
                (None, None) if !view.selected_clips.is_empty() => {
                    clips_or_selected(doc, view, None)?
                }
                (None, None) => {
                    if view.selected.is_empty() {
                        return Err(unavailable("no track or clip is selected"));
                    }
                    clips_on(doc, &view.selected)
                }
            };
            let (created, recorded) = transaction(editor, "Split clips", |editor| {
                let mut created = Vec::new();
                for clip in &targets {
                    // Right to left, so the clip keeps its first part.
                    for at in positions.iter().rev() {
                        let Some(value) = editor.document().clip(*clip) else {
                            continue;
                        };
                        if value.start < *at && *at < value.end() {
                            created.push(split_off(editor, *clip, *at)?);
                        }
                    }
                }
                Ok(created)
            })?;
            // With time selected, the parts inside it become the selection.
            if let (Some((start, end)), true) =
                (view.time_selection, view.selected_clips.is_empty())
            {
                let doc = editor.document();
                let inside: Vec<ClipId> = targets
                    .iter()
                    .chain(&created)
                    .filter(|c| {
                        doc.clip(**c)
                            .is_some_and(|c| c.start >= start && c.end() <= end)
                    })
                    .copied()
                    .collect();
                if !inside.is_empty() {
                    view.select_clips(doc, inside);
                }
            }
            Ok(to_value(&result(doc, editor, view, recorded)))
        }
        ids::DELETE => {
            let target = target(doc, view, parse(args)?)?;
            let (_, recorded) =
                transaction(editor, "Delete", |editor| delete_target(editor, &target))?;
            view.selected_clips.clear();
            Ok(to_value(&result(doc, editor, view, recorded)))
        }
        ids::DUPLICATE => {
            let target = target(doc, view, parse(args)?)?;
            let (by_track, start, end) = material(doc, &target);
            let length = end - start;
            let placements = by_track
                .into_iter()
                .map(|(track, clips)| {
                    let clips = clips
                        .into_iter()
                        .map(|c| Clip {
                            start: c.start + length,
                            ..c
                        })
                        .collect();
                    (track, clips)
                })
                .collect();
            let (created, recorded) =
                transaction(editor, "Duplicate", |editor| place(editor, placements))?;
            match target {
                Target::Clips(_) => view.select_clips(editor.document(), created),
                Target::Time { tracks, .. } => view.select_span(tracks, end, end + length),
            }
            Ok(to_value(&result(doc, editor, view, recorded)))
        }
        ids::COPY => {
            let target = target(doc, view, parse(args)?)?;
            let copied = clipboard_of(doc, &target);
            let state = copied.state();
            *clipboard = Some(copied);
            Ok(to_value(&state))
        }
        ids::CUT => {
            let target = target(doc, view, parse(args)?)?;
            let copied = clipboard_of(doc, &target);
            let (_, recorded) =
                transaction(editor, "Cut", |editor| delete_target(editor, &target))?;
            *clipboard = Some(copied);
            view.selected_clips.clear();
            Ok(to_value(&result(doc, editor, view, recorded)))
        }
        ids::PASTE => {
            let PasteArgs { at, track } = parse(args)?;
            let Some(copied) = clipboard.as_ref().filter(|c| !c.tracks.is_empty()) else {
                return Err(unavailable("there's nothing on the clipboard to paste"));
            };
            let at = match at {
                Some(at) => at
                    .resolve(&doc.settings().time_signatures)
                    .map_err(invalid)?,
                None => view
                    .time_selection
                    .map_or(view.insert_marker, |(start, _)| start),
            };
            let lanes = lanes(doc);
            let first = match track {
                Some(track) => {
                    tracks_exist(doc, &[track])?;
                    track
                }
                None => view
                    .selected
                    .iter()
                    .copied()
                    .filter(|t| lane_index(&lanes, *t).is_some())
                    .min_by_key(|t| lane_index(&lanes, *t))
                    .or_else(|| {
                        copied
                            .source
                            .first()
                            .copied()
                            .filter(|t| doc.track(*t).is_some())
                    })
                    .ok_or_else(|| unavailable("no track is selected to paste onto"))?,
            };
            let base = lane_index(&lanes, first)
                .ok_or_else(|| refused("clips can't go on a return track or the Main track"))?;
            let mut placements = Vec::with_capacity(copied.tracks.len());
            let mut destinations = Vec::with_capacity(copied.tracks.len());
            for (distance, clips) in &copied.tracks {
                let track = lanes
                    .get(base + distance)
                    .copied()
                    .ok_or_else(|| refused("there aren't enough tracks below to paste onto"))?;
                destinations.push(track);
                let clips = clips
                    .iter()
                    .map(|c| Clip {
                        start: c.start + at,
                        ..c.clone()
                    })
                    .collect();
                placements.push((track, clips));
            }
            let length = copied.length;
            let (created, recorded) =
                transaction(editor, "Paste", |editor| place(editor, placements))?;
            view.select_span(destinations, at, at + length);
            if !created.is_empty() {
                view.selected_clips = created;
            }
            Ok(to_value(&result(doc, editor, view, recorded)))
        }
        ids::NUDGE => {
            let NudgeArgs { direction, clips } = parse(args)?;
            let step = view.grid_ticks(doc);
            let by = match direction {
                Direction::Left => -step,
                Direction::Right => step,
            };
            if clips.is_none() && view.selected_clips.is_empty() {
                // Nothing selected: the insert marker moves instead.
                let at = (view.insert_marker + by).max(Ticks::ZERO);
                view.insert_marker = at;
                view.time_selection = None;
                return Ok(to_value(&result(doc, editor, view, false)));
            }
            let clips = clips_or_selected(doc, view, clips)?;
            let args = to_value(&MoveClipsArgs {
                clips: Some(clips),
                by: Some(by),
                ..MoveClipsArgs::default()
            });
            run(ids::MOVE_CLIPS, args, editor, view, clipboard)
        }
        ids::LOOP_SELECTION => {
            let LoopSelectionArgs { start, end } = parse(args)?;
            let signatures = &doc.settings().time_signatures;
            let span = match (start, end) {
                (Some(start), Some(end)) => Some((
                    start.resolve(signatures).map_err(invalid)?,
                    end.resolve(signatures).map_err(invalid)?,
                )),
                (None, None) if !view.selected_clips.is_empty() => {
                    span_of(doc, &view.selected_clips).map(|(start, end, _)| (start, end))
                }
                (None, None) => view.time_selection,
                _ => return Err(invalid("give `start` and `end`, or neither")),
            };
            let (_, recorded) = match span {
                Some((start, end)) => {
                    if end <= start || start.is_negative() {
                        return Err(invalid(
                            "the loop must start at 1.1.1 or later and end after it starts",
                        ));
                    }
                    transaction(editor, "Loop selection", |editor| {
                        apply(
                            editor,
                            Command::SetLoop {
                                start,
                                length: end - start,
                            },
                        )?;
                        apply(editor, Command::SetLoopEnabled { enabled: true })
                    })?
                }
                None => {
                    // Nothing selected: the shortcut switches the loop.
                    let enabled = !doc.settings().arrangement_loop.enabled;
                    transaction(editor, "Switch loop", |editor| {
                        apply(editor, Command::SetLoopEnabled { enabled })
                    })?
                }
            };
            Ok(to_value(&result(doc, editor, view, recorded)))
        }
        ids::SELECT_LOOP => {
            parse::<bl_commands::NoArgs>(args)?;
            let brace = doc.settings().arrangement_loop;
            view.select_span(lanes(doc), brace.start, brace.start + brace.length);
            Ok(to_value(&view.state(doc)))
        }
        other => Err(invalid(format!("`{other}` isn't an editing command"))),
    }
}

/// The earliest of the clips, the topmost of those that start together.
fn earliest(doc: &Document, lanes: &[TrackId], clips: &[ClipId]) -> ClipId {
    // One pass: the earliest start, and among clips starting then, the
    // highest lane, looked up only for those.
    let lane = |c: ClipId| {
        doc.clip_track(c)
            .and_then(|t| lane_index(lanes, t))
            .unwrap_or(usize::MAX)
    };
    let mut best: Option<(Ticks, ClipId, Option<usize>)> = None;
    for &clip in clips {
        let start = doc.clip(clip).map_or(Ticks::MAX, |c| c.start);
        best = match best {
            None => Some((start, clip, None)),
            Some((first, _, _)) if start < first => Some((start, clip, None)),
            Some((first, holder, holder_lane)) if start == first => {
                let holder_lane = holder_lane.unwrap_or_else(|| lane(holder));
                let this = lane(clip);
                Some(if this < holder_lane {
                    (start, clip, Some(this))
                } else {
                    (first, holder, Some(holder_lane))
                })
            }
            keep => keep,
        };
    }
    best.map_or(clips[0], |(_, clip, _)| clip)
}

/// Every clip on the tracks.
fn clips_on(doc: &Document, tracks: &[TrackId]) -> Vec<ClipId> {
    tracks
        .iter()
        .filter_map(|t| doc.track(*t))
        .flat_map(|t| t.clips().map(|c| c.id))
        .collect()
}

/// Positions for the UI and tests: `ticks` as an editing command's
/// [`Position`].
#[must_use]
pub fn position(ticks: Ticks) -> Position {
    Position::Ticks(ticks)
}

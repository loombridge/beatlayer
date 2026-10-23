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
    AutomationTarget, Breakpoint, Change, ChangeSet, Clip, ClipData, ClipId, Command, Document,
    Editor, TakeLaneId, TrackId, TrackKind,
};
use bl_shell_spec::arrangement::{Lane, TimeRange};
use bl_shell_spec::editing::{
    self as ids, ClipboardState, DeleteFadesArgs, Direction, Edge, EditResult, LoopSelectionArgs,
    MoveClipsArgs, NudgeArgs, PasteArgs, Position, ResizeClipsArgs, SelectClipsArgs, SelectMode,
    SplitArgs, TargetArgs,
};
use serde::Serialize;
use serde_json::Value;

use super::automation;
use super::fades;
use super::placing::{clear_on, clear_spans, piece, split_off};
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

/// The envelopes over one piece of copied material: a clip's span, or a
/// track's span of a copied time selection.
#[derive(Clone, Debug, PartialEq)]
pub struct EnvelopeMaterial {
    /// The track the piece was on.
    pub source: TrackId,
    /// The track's distance from the first track of the material.
    pub distance: usize,
    /// Where the piece starts, counted from the material's start.
    pub start: Ticks,
    /// How long it is.
    pub length: Ticks,
    /// Each parameter's shape over the piece, with its positions from the
    /// piece's start.
    pub shapes: Vec<(AutomationTarget, Vec<Breakpoint>)>,
}

/// One lane's clips in copied material.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialLane {
    /// The track's distance from the first track of the material in the
    /// track list.
    pub distance: usize,
    /// The take lane's place among its track's take lanes, or `None` for the
    /// track's main lane.
    pub slot: Option<usize>,
    /// The lane's clips; a clip's `start` counts from the material's start.
    pub clips: Vec<Clip>,
}

/// Material copied from the Arrangement, for pasting: each lane's clips,
/// placed from the material's start.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Material {
    /// Each lane's clips, top to bottom.
    pub lanes: Vec<MaterialLane>,
    /// How long the material is.
    pub length: Ticks,
    /// Where the material starts in the song, so a bounce can render the
    /// source as it is when the material is pasted (the freeze and bounce
    /// spec, R25).
    pub start: Ticks,
    /// The tracks it was copied from, in order, to paste onto when no
    /// track is selected.
    pub source: Vec<TrackId>,
    /// The envelopes over the material's pieces, so a paste carries them
    /// (the automation editing spec, R9).
    pub envelopes: Vec<EnvelopeMaterial>,
}

impl Material {
    /// What it holds, as `arrangement.copy` reports it.
    #[must_use]
    pub fn state(&self) -> ClipboardState {
        ClipboardState {
            clips: self.lanes.iter().map(|lane| lane.clips.len()).sum(),
            tracks: self
                .lanes
                .iter()
                .map(|lane| lane.distance)
                .collect::<BTreeSet<_>>()
                .len(),
            length: self.length,
        }
    }
}

/// What the clipboard holds: the Arrangement's material, or whole tracks
/// copied from the track headers (the track context menu spec, R14).
/// Whichever was copied last is what it holds.
#[derive(Clone, Debug, PartialEq)]
pub enum Clipboard {
    /// The Arrangement's material: each track's clips, placed from the
    /// material's start.
    Material(Material),
    /// Whole tracks, with everything they hold.
    Tracks(crate::tracks::CopiedTracks),
}

impl Clipboard {
    /// The Arrangement's material, if the clipboard holds it.
    #[must_use]
    pub fn material(&self) -> Option<&Material> {
        match self {
            Self::Material(material) => Some(material),
            Self::Tracks(_) => None,
        }
    }
}

/// A lane an edit acts on: a track's main lane, or one of its take lanes
/// (the comping spec, R27).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Place {
    /// The track.
    pub track: TrackId,
    /// The take lane, or `None` for the track's main lane.
    pub lane: Option<TakeLaneId>,
}

impl Place {
    /// A track's main lane.
    #[must_use]
    pub const fn main(track: TrackId) -> Self {
        Self { track, lane: None }
    }

    /// The lane a clip sits on.
    fn of_clip(doc: &Document, clip: ClipId) -> Option<Self> {
        Some(Self {
            track: doc.clip_track(clip)?,
            lane: doc.clip_lane(clip),
        })
    }

    /// The place a selection's lane names; an automation lane isn't one.
    fn of_lane(lane: &Lane) -> Option<Self> {
        match *lane {
            Lane::Track { track } => Some(Self::main(track)),
            Lane::Take { track, lane } => Some(Self {
                track,
                lane: Some(lane),
            }),
            Lane::Automation { .. } => None,
        }
    }

    /// The place as a selection's lane.
    #[must_use]
    pub const fn as_lane(self) -> Lane {
        match self.lane {
            None => Lane::Track { track: self.track },
            Some(lane) => Lane::Take {
                track: self.track,
                lane,
            },
        }
    }

    /// Where the place's row is among the Arrangement's: its track's place
    /// in the track list, then the main lane before the take lanes in order.
    fn row(self, doc: &Document, positions: &HashMap<TrackId, usize>) -> (usize, usize) {
        let track = positions.get(&self.track).copied().unwrap_or(usize::MAX);
        let lane = self.lane.map_or(0, |lane| {
            1 + doc
                .track(self.track)
                .and_then(|held| held.take_lanes().position(|l| l.id == lane))
                .unwrap_or(usize::MAX - 1)
        });
        (track, lane)
    }

    /// The lane's clips.
    pub(crate) fn clips(self, doc: &Document) -> Vec<&Clip> {
        match self.lane {
            None => doc
                .track(self.track)
                .map_or_else(Vec::new, |t| t.clips().collect()),
            Some(lane) => doc
                .take_lane(lane)
                .map_or_else(Vec::new, |l| l.clips.iter().collect()),
        }
    }

    /// The lane's place among its track's take lanes, `None` for the main
    /// lane.
    fn slot(self, doc: &Document) -> Option<usize> {
        let lane = self.lane?;
        doc.track(self.track)?
            .take_lanes()
            .position(|l| l.id == lane)
    }
}

/// `places` in the order of the Arrangement's rows.
pub(crate) fn in_row_order(doc: &Document, mut places: Vec<Place>) -> Vec<Place> {
    let positions = lane_positions(doc);
    places.sort_by_key(|place| place.row(doc, &positions));
    places.dedup();
    places
}

/// The tracks of `places`' main lanes, in order and once each.
pub(crate) fn main_tracks(places: &[Place]) -> Vec<TrackId> {
    let mut tracks: Vec<TrackId> = Vec::new();
    for place in places.iter().filter(|place| place.lane.is_none()) {
        if !tracks.contains(&place.track) {
            tracks.push(place.track);
        }
    }
    tracks
}

/// What an edit acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// Whole clips.
    Clips(Vec<ClipId>),
    /// The material in a span of time on lanes.
    Time {
        places: Vec<Place>,
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

pub(crate) fn invalid(message: impl Into<String>) -> EditFailure {
    EditFailure::InvalidArgs(message.into())
}

pub(crate) fn unavailable(reason: impl Into<String>) -> EditFailure {
    EditFailure::Unavailable(reason.into())
}

pub(crate) fn refused(reason: impl Into<String>) -> EditFailure {
    EditFailure::Refused(reason.into())
}

pub(crate) fn apply(editor: &mut Editor, command: Command) -> Result<(), EditFailure> {
    editor.apply(command).map(|_| ())?;
    Ok(())
}

/// Clears where moved clips landed, leaving the moved clips alone: each
/// destination lane is looked through once, however many clips land on it.
fn clear_moved(
    editor: &mut Editor,
    clips: &[ClipId],
    moves: &[(ClipId, Place, Ticks, Ticks)],
) -> Result<(), EditFailure> {
    let keep: BTreeSet<ClipId> = clips.iter().copied().collect();
    let mut spans: BTreeMap<Place, Vec<(Ticks, Ticks)>> = BTreeMap::new();
    for (_, place, start, end) in moves {
        spans.entry(*place).or_default().push((*start, *end));
    }
    for (place, spans) in spans {
        clear_spans(editor, place.track, place.lane, &spans, &keep)?;
    }
    Ok(())
}

/// Puts new clips on lanes, each replacing what it covers on its lane, and
/// returns their IDs. Each clip's own ID is replaced with a new one.
pub(crate) fn place(
    editor: &mut Editor,
    placements: Vec<(Place, Vec<Clip>)>,
) -> Result<Vec<ClipId>, EditFailure> {
    let doc = editor.document();
    for (place, clips) in &placements {
        let track = place.track;
        if let Some(clip) = clips.iter().find(|c| !fits(doc, track, &c.data)) {
            let what = if clip.data.is_audio() {
                "an audio clip"
            } else {
                "a MIDI clip"
            };
            let name = doc.track(track).map_or("", |t| t.name());
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
    for (place, clips) in placements {
        for clip in clips {
            created.push(place_clip(
                editor,
                place.track,
                place.lane,
                clip,
                &mut placed,
            )?);
        }
    }
    Ok(created)
}

/// As [`place`], with every clip on take lane `lane` of `track`.
pub(crate) fn place_on_lane(
    editor: &mut Editor,
    track: TrackId,
    lane: TakeLaneId,
    clips: Vec<Clip>,
) -> Result<Vec<ClipId>, EditFailure> {
    place(
        editor,
        vec![(
            Place {
                track,
                lane: Some(lane),
            },
            clips,
        )],
    )
}

/// As [`place`], with every clip on its track's main lane.
pub(crate) fn place_on_main(
    editor: &mut Editor,
    placements: Vec<(TrackId, Vec<Clip>)>,
) -> Result<Vec<ClipId>, EditFailure> {
    place(
        editor,
        placements
            .into_iter()
            .map(|(track, clips)| (Place::main(track), clips))
            .collect(),
    )
}

/// Puts one new clip, under a new ID, on `track`'s main lane or its take
/// lane `lane`, replacing what it covers there.
fn place_clip(
    editor: &mut Editor,
    track: TrackId,
    lane: Option<TakeLaneId>,
    mut clip: Clip,
    placed: &mut BTreeSet<ClipId>,
) -> Result<ClipId, EditFailure> {
    clip.id = editor.new_id();
    // Spec: grooves R3, R5. A copy keeps its groove while the pool still
    // holds it; one deleted since the copy leaves it none.
    clip.groove = clip
        .groove
        .filter(|groove| editor.document().settings().grooves.get(*groove).is_some());
    clear_on(editor, track, lane, clip.start, clip.end(), placed)?;
    placed.insert(clip.id);
    // Spec: scales R7. A copy keeps its source's scale, which the clip takes
    // the set's current scale when it is made.
    let scale = clip.scale;
    let id = clip.id;
    apply(
        editor,
        match lane {
            Some(lane) => Command::CreateTakeClip { lane, clip },
            None => Command::CreateClip { track, clip },
        },
    )?;
    if scale != editor.document().settings().scale {
        apply(editor, scale.command(vec![id]))?;
    }
    Ok(id)
}

/// Runs `edit` as one transaction labelled `label`, returning its result
/// and whether it recorded an undo step. A failure puts the document back.
pub(crate) fn transaction<T>(
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

/// Runs `edit` as [`transaction`] does, then settles the fades the edit left
/// (R26) in the same undo step: `before` is the document the edit began from.
fn settled<T>(
    editor: &mut Editor,
    label: &'static str,
    (before, edge_fades): (&Document, bool),
    edit: impl FnOnce(&mut Editor) -> Result<T, EditFailure>,
) -> Result<(T, bool), EditFailure> {
    transaction(editor, label, |editor| {
        let value = edit(editor)?;
        fades::settle(editor, before, edge_fades)?;
        Ok(value)
    })
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

/// The lanes a command's `tracks` and `lanes` name, or the selection's.
fn places_of(
    doc: &Document,
    view: &ArrangementView,
    tracks: Option<Vec<TrackId>>,
    lanes: Option<Vec<Lane>>,
) -> Result<Vec<Place>, EditFailure> {
    match (tracks, lanes) {
        (Some(_), Some(_)) => Err(invalid("give `tracks` or `lanes`, not both")),
        (Some(tracks), None) => {
            tracks_exist(doc, &tracks)?;
            Ok(tracks.into_iter().map(Place::main).collect())
        }
        (None, Some(lanes)) => {
            let mut places = Vec::with_capacity(lanes.len());
            for lane in &lanes {
                let place = Place::of_lane(lane)
                    .ok_or_else(|| invalid("an automation lane isn't a lane of clips"))?;
                tracks_exist(doc, &[place.track])?;
                if let Some(held) = place.lane
                    && doc.take_lane_track(held) != Some(place.track)
                {
                    return Err(invalid(format!(
                        "take lane {} isn't one of track {}'s",
                        held.0, place.track.0
                    )));
                }
                places.push(place);
            }
            Ok(places)
        }
        (None, None) => {
            let places = view.selected_places();
            tracks_exist(doc, &places.iter().map(|p| p.track).collect::<Vec<_>>())?;
            Ok(places)
        }
    }
}

/// What a command's target arguments, or the selection, name.
pub(crate) fn target(
    doc: &Document,
    view: &ArrangementView,
    args: TargetArgs,
) -> Result<Target, EditFailure> {
    let TargetArgs {
        clips,
        tracks,
        lanes,
        start,
        end,
    } = args;
    if let Some(clips) = clips {
        if tracks.is_some() || lanes.is_some() || start.is_some() || end.is_some() {
            return Err(invalid(
                "give `clips`, or a span of time (`tracks` or `lanes`, `start`, `end`), not both",
            ));
        }
        return Ok(Target::Clips(clips_or_selected(doc, view, Some(clips))?));
    }
    if tracks.is_none()
        && lanes.is_none()
        && start.is_none()
        && end.is_none()
        && !view.selected_clips.is_empty()
    {
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
    let places = places_of(doc, view, tracks, lanes)?;
    if places.is_empty() {
        return Err(unavailable("no track is selected"));
    }
    Ok(Target::Time { places, start, end })
}

/// The material a target names, by lane in display order, with absolute
/// positions, and the span it covers.
pub(crate) fn lane_material(
    doc: &Document,
    target: &Target,
) -> (Vec<(Place, Vec<Clip>)>, Ticks, Ticks) {
    let positions = lane_positions(doc);
    match target {
        Target::Clips(ids) => {
            let mut by_place: Vec<(Place, Vec<Clip>)> = Vec::new();
            let (mut start, mut end) = (Ticks::MAX, Ticks::MIN);
            for id in ids {
                let (Some(clip), Some(place)) = (doc.clip(*id), Place::of_clip(doc, *id)) else {
                    continue;
                };
                start = start.min(clip.start);
                end = end.max(clip.end());
                match by_place.iter_mut().find(|(p, _)| *p == place) {
                    Some((_, clips)) => clips.push(clip.clone()),
                    None => by_place.push((place, vec![clip.clone()])),
                }
            }
            by_place.sort_by_key(|(place, _)| place.row(doc, &positions));
            for (_, clips) in &mut by_place {
                clips.sort_by_key(|c| (c.start, c.id));
            }
            (by_place, start, end)
        }
        Target::Time { places, start, end } => {
            let mut places = places.clone();
            places.sort_by_key(|place| place.row(doc, &positions));
            places.dedup();
            let by_place = places
                .into_iter()
                .map(|place| {
                    let mut clips: Vec<Clip> = place
                        .clips(doc)
                        .into_iter()
                        .filter(|c| c.start < *end && c.end() > *start)
                        .map(|c| {
                            piece(
                                c,
                                c.start.max(*start),
                                c.end().min(*end),
                                &doc.settings().tempo,
                            )
                        })
                        .collect();
                    clips.sort_by_key(|c| (c.start, c.id));
                    (place, clips)
                })
                .collect();
            (by_place, *start, *end)
        }
    }
}

/// The material a target names on tracks' main lanes, by track in display
/// order, with absolute positions, and the span it covers: take lanes hold
/// no material that bounces, reverses or plays.
pub(crate) fn material(
    doc: &Document,
    target: &Target,
) -> (Vec<(TrackId, Vec<Clip>)>, Ticks, Ticks) {
    let (by_place, start, end) = lane_material(doc, target);
    let by_track = by_place
        .into_iter()
        .filter(|(place, _)| place.lane.is_none())
        .map(|(place, clips)| (place.track, clips))
        .collect();
    (by_track, start, end)
}

/// The tracks of a lane list, in order and once each.
fn tracks_of(by_place: &[(Place, Vec<Clip>)]) -> Vec<TrackId> {
    let mut tracks: Vec<TrackId> = Vec::new();
    for (place, _) in by_place {
        if !tracks.contains(&place.track) {
            tracks.push(place.track);
        }
    }
    tracks
}

/// The clipboard holding a target's material.
fn clipboard_of(doc: &Document, target: &Target) -> Material {
    let lanes = lanes(doc);
    let (by_place, start, end) = lane_material(doc, target);
    let first = by_place
        .first()
        .and_then(|(place, _)| lane_index(&lanes, place.track))
        .unwrap_or(0);
    let material = by_place
        .iter()
        .map(|(place, clips)| MaterialLane {
            distance: lane_index(&lanes, place.track).unwrap_or(first) - first,
            slot: place.slot(doc),
            clips: clips
                .iter()
                .map(|clip| Clip {
                    start: clip.start - start,
                    ..clip.clone()
                })
                .collect(),
        })
        .collect();
    // Spec: automation-editing R9. The material's envelopes go with it.
    let envelopes = envelope_material(doc, target, &by_place, start, end);
    Material {
        lanes: material,
        length: end - start,
        start,
        source: tracks_of(&by_place),
        envelopes,
    }
}

/// The envelopes over a track's span: each parameter's shape, with its
/// positions from the span's start.
fn envelopes_over(
    doc: &Document,
    track: TrackId,
    start: Ticks,
    end: Ticks,
) -> Vec<(AutomationTarget, Vec<Breakpoint>)> {
    doc.track(track)
        .map(|track| {
            track
                .automation()
                .map(|(target, automation)| (*target, automation.envelope.shape_in(start, end)))
                .filter(|(_, shape)| !shape.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// The material's tracks, by their distance from the first of them.
fn tracks_by_distance(doc: &Document, by_place: &[(Place, Vec<Clip>)]) -> BTreeMap<usize, TrackId> {
    let lanes = lanes(doc);
    let first = by_place
        .first()
        .and_then(|(place, _)| lane_index(&lanes, place.track))
        .unwrap_or(0);
    by_place
        .iter()
        .map(|(place, _)| {
            (
                lane_index(&lanes, place.track).unwrap_or(first) - first,
                place.track,
            )
        })
        .collect()
}

/// The envelopes over the pieces of a target's material, so a copy and a
/// paste carry them (the automation editing spec, R9). A take lane's
/// material carries none: the tracks' envelopes follow their main lanes.
fn envelope_material(
    doc: &Document,
    target: &Target,
    by_place: &[(Place, Vec<Clip>)],
    start: Ticks,
    end: Ticks,
) -> Vec<EnvelopeMaterial> {
    let lanes = lanes(doc);
    let first = by_place
        .first()
        .and_then(|(place, _)| lane_index(&lanes, place.track))
        .unwrap_or(0);
    let distance = |track: &TrackId| lane_index(&lanes, *track).unwrap_or(first) - first;
    let mains = by_place.iter().filter(|(place, _)| place.lane.is_none());
    match target {
        Target::Clips(_) => mains
            .flat_map(|(place, clips)| {
                clips.iter().map(|clip| EnvelopeMaterial {
                    source: place.track,
                    distance: distance(&place.track),
                    start: clip.start - start,
                    length: clip.length,
                    shapes: envelopes_over(doc, place.track, clip.start, clip.end()),
                })
            })
            .collect(),
        Target::Time { .. } => mains
            .map(|(place, _)| EnvelopeMaterial {
                source: place.track,
                distance: distance(&place.track),
                start: Ticks::ZERO,
                length: end - start,
                shapes: envelopes_over(doc, place.track, start, end),
            })
            .collect(),
    }
}

/// Writes the envelopes of `materials` into the tracks they name, each
/// piece at its own place from `at`.
///
/// A piece that lands on another track keeps only the envelopes that go
/// with a clip to another track: the mixer controls and the sends that
/// track also has. A device's parameters stay on the track they're on (the
/// automation editing spec, R9).
fn write_envelopes(
    editor: &mut Editor,
    materials: &[EnvelopeMaterial],
    tracks: &BTreeMap<usize, TrackId>,
    at: Ticks,
) -> Result<(), EditFailure> {
    for material in materials {
        let Some(track) = tracks.get(&material.distance) else {
            continue;
        };
        let start = at + material.start;
        let end = start + material.length;
        for (target, shape) in &material.shapes {
            if material.source != *track {
                let doc = editor.document();
                if !travels_with(target, doc, *track) {
                    continue;
                }
            }
            let mut envelope = automation::envelope_of(editor.document(), *track, *target)
                .cloned()
                .unwrap_or_default();
            envelope.write_span(start, end, shape);
            apply(
                editor,
                Command::SetAutomationEnvelope {
                    track: *track,
                    target: *target,
                    points: envelope.points().collect(),
                },
            )?;
        }
    }
    Ok(())
}

/// Removes the envelopes of `materials` over their spans, as Delete leaves
/// a span (the automation editing spec, R9).
fn remove_envelopes(
    editor: &mut Editor,
    materials: &[EnvelopeMaterial],
    tracks: &BTreeMap<usize, TrackId>,
    at: Ticks,
) -> Result<(), EditFailure> {
    for material in materials {
        let Some(track) = tracks.get(&material.distance) else {
            continue;
        };
        let start = at + material.start;
        let end = start + material.length;
        for (target, _) in &material.shapes {
            automation::remove_shape(editor, *track, *target, start, end)?;
        }
    }
    Ok(())
}

/// Whether a parameter's envelope goes with a clip to another track: the
/// mixer controls and the sends that track also has. A device's parameters
/// stay on the track they're on (the automation editing spec, R9).
fn travels_with(target: &AutomationTarget, doc: &Document, destination: TrackId) -> bool {
    match target {
        AutomationTarget::Mixer { .. } => true,
        AutomationTarget::Send { to: return_track } => doc
            .track(destination)
            .is_some_and(|track| track.sends().any(|(to, _)| to == *return_track)),
        _ => false,
    }
}

/// Moves the envelopes over a moved clip's span with it: the shape over
/// the old span goes to the new one, and the old span is left as Delete
/// leaves a span. A copy leaves the old span as it is.
fn carry_envelopes(
    editor: &mut Editor,
    from: TrackId,
    to: TrackId,
    old: (Ticks, Ticks),
    new: (Ticks, Ticks),
    copy: bool,
) -> Result<(), EditFailure> {
    let doc = editor.document().clone();
    let Some(source) = doc.track(from) else {
        return Ok(());
    };
    let targets: Vec<AutomationTarget> = source.automation().map(|(target, _)| *target).collect();
    for target in targets {
        if from != to && !travels_with(&target, &doc, to) {
            continue;
        }
        let Some(automation) = source.automation_for(&target) else {
            continue;
        };
        let shape = automation.envelope.shape_in(old.0, old.1);
        if shape.is_empty() {
            continue;
        }
        if !copy {
            automation::remove_shape(editor, from, target, old.0, old.1)?;
        }
        let mut envelope = automation::envelope_of(editor.document(), to, target)
            .cloned()
            .unwrap_or_default();
        envelope.write_span(new.0, new.1, &shape);
        apply(
            editor,
            Command::SetAutomationEnvelope {
                track: to,
                target,
                points: envelope.points().collect(),
            },
        )?;
    }
    Ok(())
}

/// Deletes a target's material, and the envelopes over it when Lock
/// Envelopes is off (the automation editing spec, R9).
fn delete_target(editor: &mut Editor, target: &Target) -> Result<(), EditFailure> {
    let doc = editor.document().clone();
    let locked = doc.settings().lock_envelopes;
    let (by_place, start, end) = lane_material(&doc, target);
    match target {
        Target::Clips(clips) => {
            for clip in clips {
                apply(editor, Command::DeleteClip { clip: *clip })?;
            }
        }
        Target::Time { places, start, end } => {
            for place in places {
                clear_on(
                    editor,
                    place.track,
                    place.lane,
                    *start,
                    *end,
                    &BTreeSet::new(),
                )?;
            }
        }
    }
    if !locked {
        let materials = envelope_material(&doc, target, &by_place, start, end);
        let tracks = tracks_by_distance(&doc, &by_place);
        remove_envelopes(editor, &materials, &tracks, start)?;
    }
    Ok(())
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

/// The lanes the clips sit on, in display order.
fn places_of_clips(doc: &Document, clips: &[ClipId]) -> Vec<Place> {
    let positions = lane_positions(doc);
    let mut places: Vec<Place> = clips
        .iter()
        .filter_map(|clip| Place::of_clip(doc, *clip))
        .collect();
    places.sort_by_key(|place| place.row(doc, &positions));
    places.dedup();
    places
}

impl ArrangementView {
    /// The lanes the selection's time is on: for each selected track, the
    /// lanes the time selection names there, or its main lane when it names
    /// none (the comping spec, R27).
    pub(crate) fn selected_places(&self) -> Vec<Place> {
        let mut places = Vec::new();
        for track in &self.selected {
            let named: Vec<Place> = self
                .time_selection_lanes
                .iter()
                .filter_map(Place::of_lane)
                .filter(|place| place.track == *track)
                .collect();
            if named.is_empty() {
                places.push(Place::main(*track));
            } else {
                places.extend(named);
            }
        }
        places
    }

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
        self.selected_fade = None;
        self.selected_take_lanes.clear();
        if let Some((start, end, tracks)) = span_of(doc, &self.selected_clips) {
            self.time_selection = Some((start, end));
            self.insert_marker = start;
            self.selected = tracks;
            self.title_bars = false;
            self.follow_paused = false;
            // Spec: automation-editing R1, comping R26. The selection is on
            // the lanes the clips sit on.
            self.time_selection_lanes = places_of_clips(doc, &self.selected_clips)
                .into_iter()
                .map(Place::as_lane)
                .collect();
            self.select_lane_of_selection();
        }
    }

    /// Gives the keyboard to the automation lane the time selection names,
    /// when it names one lane and no track's own (the automation editing
    /// spec, R1, R20), and takes it from the lane otherwise.
    pub(crate) fn select_lane_of_selection(&mut self) {
        let lane = match self.time_selection_lanes.as_slice() {
            [Lane::Automation { track, .. }] => Some(*track),
            _ => None,
        };
        self.select_lane(lane);
        self.selected_breakpoint = None;
    }

    /// Selects time on tracks, with no clips, as dragging across the lanes
    /// does.
    pub(crate) fn select_span(&mut self, tracks: Vec<TrackId>, start: Ticks, end: Ticks) {
        let lanes = tracks
            .iter()
            .copied()
            .map(|track| Lane::Track { track })
            .collect();
        self.select_span_on(lanes, start, end);
    }

    /// Selects time on lanes, with no clips: the lanes' tracks become the
    /// selected tracks.
    pub(crate) fn select_span_on(&mut self, lanes: Vec<Lane>, start: Ticks, end: Ticks) {
        self.selected_clips.clear();
        self.selected_take_lanes.clear();
        let mut tracks: Vec<TrackId> = Vec::new();
        for track in lanes.iter().map(Lane::track) {
            if !tracks.contains(&track) {
                tracks.push(track);
            }
        }
        self.selected = tracks;
        self.time_selection = (end > start).then_some((start, end));
        self.insert_marker = start;
        self.title_bars = false;
        // Spec: automation-editing R1. The selection names the lanes it
        // covers.
        self.time_selection_lanes = lanes;
        self.select_lane_of_selection();
    }

    /// Takes the selection from another view: the clips, tracks, time, and
    /// insert marker, leaving zoom and scrolling as they are. A gesture
    /// starts each frame from the selection it began with.
    pub(crate) fn restore_selection(&mut self, from: &ArrangementView) {
        self.selected_clips.clone_from(&from.selected_clips);
        self.selected_take_lanes
            .clone_from(&from.selected_take_lanes);
        self.selected.clone_from(&from.selected);
        self.time_selection = from.time_selection;
        self.time_selection_lanes
            .clone_from(&from.time_selection_lanes);
        self.insert_marker = from.insert_marker;
        self.title_bars = from.title_bars;
        self.selected_lane = from.selected_lane;
        self.selected_breakpoint = from.selected_breakpoint;
    }

    /// Drops selected clips the document no longer has.
    pub(crate) fn forget_missing_clips(&mut self, doc: &Document) {
        self.selected_clips.retain(|id| doc.clip(*id).is_some());
        if self
            .selected_fade
            .is_some_and(|fade| doc.clip(fade.clip).is_none())
        {
            self.selected_fade = None;
        }
    }

    /// The editing grid's spacing at the current zoom, in ticks: how far
    /// Left and Right nudge.
    #[must_use]
    pub fn grid_ticks(&self, doc: &Document) -> Ticks {
        self.grid_step(doc).ticks.max(Ticks(1))
    }

    /// `at` on the nearest grid line, whether snapping is on or not: where
    /// holding Cmd (Ctrl on Windows) puts a split (note-editing R33).
    #[must_use]
    pub fn nearest_line(&self, doc: &Document, at: Ticks) -> Ticks {
        super::grid::snap(&doc.settings().time_signatures, self.grid_step(doc), at)
    }

    /// The grid lines either side of `at`: the last one before it and the
    /// first one after it, or `at` itself where there is none. A stretch
    /// marker moves freely between them before it snaps (note-editing R43).
    #[must_use]
    pub fn lines_around(&self, doc: &Document, at: Ticks) -> (Ticks, Ticks) {
        let step = self.grid_step(doc);
        let reach = step.ticks.max(Ticks(1)) * 2;
        let (mut before, mut after) = (at, at);
        super::grid::lines(
            &doc.settings().time_signatures,
            step,
            (at - reach).max(Ticks::ZERO),
            at + reach,
            64,
            |line, _| {
                if line < at {
                    before = line;
                } else if line > at && after == at {
                    after = line;
                }
            },
        );
        (before, after)
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
                let tempo = &doc.settings().tempo;
                earliest = earliest.max(clip.earliest_start(tempo) - clip.start);
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
        ids::CREATE_FADE => "Create Fade",
        ids::DELETE_FADES => "Delete Fades",
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
    let headers = !view.selected_take_lanes.is_empty();
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
        ids::DELETE | ids::DUPLICATE => {
            given("clips") || given("start") || has_selection || headers
        }
        ids::COPY | ids::CUT => given("clips") || given("start") || has_selection,
        ids::CREATE_FADE => create_fade_plan(doc, view, args).is_some_and(|plan| !plan.is_empty()),
        ids::DELETE_FADES => given("clips") || !view.selected_clips.is_empty(),
        ids::PASTE => {
            clipboard.is_some_and(|c| c.material().is_some_and(|m| !m.lanes.is_empty()))
                && (given("track")
                    || given("lane")
                    || !view.selected.is_empty()
                    || !lanes(doc).is_empty())
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
    edge_fades: bool,
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
                lane,
                copy,
            } = parse(args)?;
            if track.is_some() && lane.is_some() {
                return Err(invalid("give `track` or `lane`, not both"));
            }
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
            // Where the anchor goes: its track's main lane for `track`, the
            // lane `lane` names (the comping spec, R27). The other clips keep
            // their distance from it in the track list and among each track's
            // lanes, the main lane above the take lanes.
            let slot = |place: Place| place.slot(doc).map_or(-1isize, |at| at as isize);
            let anchor_place = Place::of_clip(doc, anchor).expect("on a track");
            let destination = match (track, lane) {
                (Some(track), None) => {
                    tracks_exist(doc, &[track])?;
                    Some(Place::main(track))
                }
                (None, Some(lane)) => places_of(doc, view, None, Some(vec![lane]))?
                    .first()
                    .copied(),
                _ => None,
            };
            let (shift, slot_shift) = match destination {
                None => (0, 0),
                Some(destination) => {
                    let to = lane_index(&lanes, destination.track).ok_or_else(|| {
                        refused("clips can't go on a return track or the Main track")
                    })?;
                    let from = lane_index(&lanes, anchor_place.track).unwrap_or(to);
                    (
                        to as isize - from as isize,
                        slot(destination) - slot(anchor_place),
                    )
                }
            };
            // Where each clip goes, checked before anything changes, and
            // where it was, so its envelopes can go with it (the automation
            // editing spec, R9).
            let sources: Vec<Option<(Place, Ticks, Ticks)>> = clips
                .iter()
                .map(|clip| {
                    let value = doc.clip(*clip).expect("the clips exist");
                    Some((Place::of_clip(doc, *clip)?, value.start, value.end()))
                })
                .collect();
            let positions = lane_positions(doc);
            let mut moves = Vec::with_capacity(clips.len());
            for clip in &clips {
                let value = doc.clip(*clip).expect("the clips exist");
                let source = Place::of_clip(doc, *clip).expect("on a track");
                let from = positions.get(&source.track).copied().unwrap_or(0);
                let track = from
                    .checked_add_signed(shift)
                    .and_then(|i| lanes.get(i).copied())
                    .ok_or_else(|| refused("there's no track that far up or down"))?;
                if !fits(doc, track, &value.data) {
                    return Err(refused(
                        "a clip would land on a track of another kind: audio clips go on audio \
                         tracks and MIDI clips on MIDI tracks",
                    ));
                }
                let at = slot(source) + slot_shift;
                let take = match usize::try_from(at) {
                    Ok(at) => Some(
                        doc.track(track)
                            .and_then(|held| held.take_lanes().nth(at))
                            .map(|held| held.id)
                            .ok_or_else(|| refused("there's no take lane that far down"))?,
                    ),
                    Err(_) if at == -1 => None,
                    Err(_) => return Err(refused("there's no lane that far up")),
                };
                let start = value
                    .start
                    .checked_add(by)
                    .filter(|start| !start.is_negative())
                    .ok_or_else(|| refused("a clip would start before 1.1.1"))?;
                moves.push((
                    *clip,
                    Place { track, lane: take },
                    start,
                    start + value.length,
                ));
            }
            let in_place = clips
                .iter()
                .zip(&moves)
                .all(|(clip, (_, place, ..))| Place::of_clip(doc, *clip) == Some(*place));
            let label = if copy { "Copy clips" } else { "Move clips" };
            let locked = doc.settings().lock_envelopes;
            let (moved, recorded) = settled(editor, label, (doc, edge_fades), |editor| {
                let moved = if copy {
                    let placements = moves
                        .iter()
                        .map(|(clip, place, start, _)| {
                            let value = editor.document().clip(*clip).expect("the clips exist");
                            (
                                *place,
                                vec![Clip {
                                    start: *start,
                                    ..value.clone()
                                }],
                            )
                        })
                        .collect();
                    place(editor, placements)?
                } else if in_place {
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
                    clips.clone()
                } else {
                    for (clip, place, start, _) in &moves {
                        apply(
                            editor,
                            Command::MoveClipToLane {
                                clip: *clip,
                                track: place.track,
                                lane: place.lane,
                                start: *start,
                            },
                        )?;
                    }
                    clear_moved(editor, &clips, &moves)?;
                    clips.clone()
                };
                // Spec: automation-editing R9. The tracks' envelopes go
                // over the clips' spans, in the same undo step. They follow
                // the main lanes: a take lane's clips carry none.
                if !locked {
                    for ((_, to, start, end), source) in moves.iter().zip(&sources) {
                        let Some((from, old_start, old_end)) = source else {
                            continue;
                        };
                        if from.lane.is_some() || to.lane.is_some() {
                            continue;
                        }
                        carry_envelopes(
                            editor,
                            from.track,
                            to.track,
                            (*old_start, *old_end),
                            (*start, *end),
                            copy,
                        )?;
                    }
                }
                Ok(moved)
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
                    Edge::Start
                        if clip
                            .with_start(clip.start + by, &doc.settings().tempo)
                            .is_err() =>
                    {
                        return Err(refused(
                            "a clip would start before its contents begin (an audio clip's \
                             Start marker before the start of its file); it can't show more of \
                             them than they hold",
                        ));
                    }
                    _ => {}
                }
            }
            let (_, recorded) = settled(editor, "Resize clips", (doc, edge_fades), |editor| {
                let keep: BTreeSet<ClipId> = clips.iter().copied().collect();
                for clip in &clips {
                    let value = editor
                        .document()
                        .clip(*clip)
                        .expect("the clips exist")
                        .clone();
                    let track = editor.document().clip_track(*clip).expect("on a track");
                    let lane = editor.document().clip_lane(*clip);
                    match edge {
                        Edge::End => {
                            apply(
                                editor,
                                Command::SetClipLength {
                                    clip: *clip,
                                    length: value.length + by,
                                },
                            )?;
                            clear_on(editor, track, lane, value.end(), value.end() + by, &keep)?;
                        }
                        Edge::Start => {
                            apply(
                                editor,
                                Command::TrimClipStart {
                                    clip: *clip,
                                    start: value.start + by,
                                },
                            )?;
                            clear_on(editor, track, lane, value.start + by, value.start, &keep)?;
                        }
                    }
                }
                Ok(())
            })?;
            view.select_clips(editor.document(), clips);
            Ok(to_value(&result(doc, editor, view, recorded)))
        }
        ids::SPLIT => {
            let SplitArgs {
                clips,
                tracks,
                lanes,
                at,
            } = parse(args)?;
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
            let targets: Vec<ClipId> = match (clips, tracks, lanes) {
                (Some(clips), None, None) => clips_or_selected(doc, view, Some(clips))?,
                (None, None, None) if !view.selected_clips.is_empty() => {
                    clips_or_selected(doc, view, None)?
                }
                (None, None, None) => {
                    if view.selected.is_empty() {
                        return Err(unavailable("no track or clip is selected"));
                    }
                    clips_in(doc, &view.selected_places())
                }
                (None, tracks, lanes) => clips_in(doc, &places_of(doc, view, tracks, lanes)?),
                _ => {
                    return Err(invalid(
                        "give `clips`, `tracks` or `lanes`, not more than one",
                    ));
                }
            };
            let (created, recorded) =
                settled(editor, "Split clips", (doc, edge_fades), |editor| {
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
            let args: TargetArgs = parse(args)?;
            // Spec: comping R7, R28. With take lane headers selected, Delete
            // deletes the lanes.
            if args == TargetArgs::default() && !view.selected_take_lanes.is_empty() {
                let lanes = view.selected_take_lanes.clone();
                let (_, recorded) = transaction(editor, "Delete Take Lane(s)", |editor| {
                    apply(editor, Command::DeleteTakeLanes { lanes })
                })?;
                view.selected_take_lanes.clear();
                return Ok(to_value(&result(doc, editor, view, recorded)));
            }
            let target = target(doc, view, args)?;
            let (_, recorded) = settled(editor, "Delete", (doc, edge_fades), |editor| {
                delete_target(editor, &target)
            })?;
            view.selected_clips.clear();
            Ok(to_value(&result(doc, editor, view, recorded)))
        }
        ids::DUPLICATE => {
            let args: TargetArgs = parse(args)?;
            // Spec: comping R5, R28. With take lane headers selected, it
            // duplicates the lanes.
            if args == TargetArgs::default() && !view.selected_take_lanes.is_empty() {
                super::comping::run(
                    bl_shell_spec::comping::DUPLICATE,
                    serde_json::json!({}),
                    editor,
                    view,
                )?;
                let recorded = editor.document() != doc;
                return Ok(to_value(&result(doc, editor, view, recorded)));
            }
            let target = target(doc, view, args)?;
            let (by_place, start, end) = lane_material(doc, &target);
            let length = end - start;
            // Spec: automation-editing R9. The copy carries the envelopes
            // over the same spans.
            let materials = envelope_material(doc, &target, &by_place, start, end);
            let tracks = tracks_by_distance(doc, &by_place);
            let locked = doc.settings().lock_envelopes;
            let placements = by_place
                .into_iter()
                .map(|(place, clips)| {
                    let clips = clips
                        .into_iter()
                        .map(|c| Clip {
                            start: c.start + length,
                            ..c
                        })
                        .collect();
                    (place, clips)
                })
                .collect();
            let (created, recorded) = settled(editor, "Duplicate", (doc, edge_fades), |editor| {
                let created = place(editor, placements)?;
                if !locked {
                    write_envelopes(editor, &materials, &tracks, start + length)?;
                }
                Ok(created)
            })?;
            match target {
                Target::Clips(_) => view.select_clips(editor.document(), created),
                Target::Time { places, .. } => view.select_span_on(
                    places.into_iter().map(Place::as_lane).collect(),
                    end,
                    end + length,
                ),
            }
            Ok(to_value(&result(doc, editor, view, recorded)))
        }
        ids::COPY => {
            let target = target(doc, view, parse(args)?)?;
            let copied = clipboard_of(doc, &target);
            let state = copied.state();
            *clipboard = Some(Clipboard::Material(copied));
            Ok(to_value(&state))
        }
        ids::CUT => {
            let target = target(doc, view, parse(args)?)?;
            let copied = clipboard_of(doc, &target);
            let (_, recorded) = settled(editor, "Cut", (doc, edge_fades), |editor| {
                delete_target(editor, &target)
            })?;
            *clipboard = Some(Clipboard::Material(copied));
            view.selected_clips.clear();
            Ok(to_value(&result(doc, editor, view, recorded)))
        }
        ids::PASTE => {
            let PasteArgs { at, track, lane } = parse(args)?;
            if track.is_some() && lane.is_some() {
                return Err(invalid("give `track` or `lane`, not both"));
            }
            let Some(copied) = clipboard
                .as_ref()
                .and_then(Clipboard::material)
                .filter(|c| !c.lanes.is_empty())
            else {
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
            let positions = lane_positions(doc);
            // The lane the material's first lane goes to.
            let first: Place = match (track, lane) {
                (Some(track), None) => {
                    tracks_exist(doc, &[track])?;
                    Place::main(track)
                }
                (None, Some(lane)) => places_of(doc, view, None, Some(vec![lane]))?
                    .first()
                    .copied()
                    .ok_or_else(|| invalid("`lane` names no lane"))?,
                _ => view
                    .selected_places()
                    .into_iter()
                    .filter(|place| lane_index(&lanes, place.track).is_some())
                    .min_by_key(|place| place.row(doc, &positions))
                    .or_else(|| {
                        copied
                            .source
                            .first()
                            .copied()
                            .filter(|t| doc.track(*t).is_some())
                            .map(Place::main)
                    })
                    .ok_or_else(|| unavailable("no track is selected to paste onto"))?,
            };
            let base = lane_index(&lanes, first.track)
                .ok_or_else(|| refused("clips can't go on a return track or the Main track"))?;
            // The material's lanes keep their distance from its first, among
            // the tracks and among a track's lanes, the main lane above the
            // take lanes.
            let slot_pos = |slot: Option<usize>| slot.map_or(-1isize, |at| at as isize);
            let slot_shift = first.slot(doc).map_or(-1isize, |at| at as isize)
                - slot_pos(copied.lanes.first().and_then(|lane| lane.slot));
            let mut placements = Vec::with_capacity(copied.lanes.len());
            let mut destinations = Vec::with_capacity(copied.lanes.len());
            let mut landed: BTreeMap<usize, TrackId> = BTreeMap::new();
            for held in &copied.lanes {
                let track = lanes
                    .get(base + held.distance)
                    .copied()
                    .ok_or_else(|| refused("there aren't enough tracks below to paste onto"))?;
                let at_slot = slot_pos(held.slot) + slot_shift;
                let take = match usize::try_from(at_slot) {
                    Ok(at_slot) => Some(
                        doc.track(track)
                            .and_then(|t| t.take_lanes().nth(at_slot))
                            .map(|t| t.id)
                            .ok_or_else(|| {
                                refused("there aren't enough take lanes below to paste onto")
                            })?,
                    ),
                    Err(_) if at_slot == -1 => None,
                    Err(_) => return Err(refused("there isn't a lane that far up to paste onto")),
                };
                let place = Place { track, lane: take };
                destinations.push(place);
                landed.insert(held.distance, track);
                let clips = held
                    .clips
                    .iter()
                    .map(|c| Clip {
                        start: c.start + at,
                        ..c.clone()
                    })
                    .collect();
                placements.push((place, clips));
            }
            let length = copied.length;
            // Spec: automation-editing R9. The pasted material carries the
            // envelopes over the same spans.
            let materials = copied.envelopes.clone();
            let locked = doc.settings().lock_envelopes;
            let (created, recorded) = settled(editor, "Paste", (doc, edge_fades), |editor| {
                let created = place(editor, placements)?;
                if !locked {
                    write_envelopes(editor, &materials, &landed, at)?;
                }
                Ok(created)
            })?;
            view.select_span_on(
                destinations.into_iter().map(Place::as_lane).collect(),
                at,
                at + length,
            );
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
                view.time_selection_lanes.clear();
                return Ok(to_value(&result(doc, editor, view, false)));
            }
            let clips = clips_or_selected(doc, view, clips)?;
            let args = to_value(&MoveClipsArgs {
                clips: Some(clips),
                by: Some(by),
                ..MoveClipsArgs::default()
            });
            run(ids::MOVE_CLIPS, args, editor, view, clipboard, edge_fades)
        }
        ids::CREATE_FADE => {
            let target = target(doc, view, parse(args)?)?;
            let plan = fades::plan(doc, &target);
            if plan.is_empty() {
                return Err(unavailable("the selection gives no fade"));
            }
            let label = if plan.has_crossfade() {
                "Create Crossfade"
            } else {
                "Create Fade"
            };
            let (_, recorded) = transaction(editor, label, |editor| fades::create(editor, &plan))?;
            Ok(to_value(&result(doc, editor, view, recorded)))
        }
        ids::DELETE_FADES => {
            let DeleteFadesArgs { clips, edge } = parse(args)?;
            let clips = clips_or_selected(doc, view, clips)?;
            let label = if clips.len() == 1 && edge.is_some() {
                "Delete Fade"
            } else {
                "Delete Fades"
            };
            let (_, recorded) = transaction(editor, label, |editor| {
                fades::delete_fades(editor, &clips, edge, edge_fades)
            })?;
            view.selected_fade = None;
            Ok(to_value(&result(doc, editor, view, recorded)))
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

/// What Create Fade would make from the selection or the arguments, if they
/// name something to act on.
fn create_fade_plan(doc: &Document, view: &ArrangementView, args: &Value) -> Option<fades::Plan> {
    let args: TargetArgs = serde_json::from_value(args.clone()).ok()?;
    let target = target(doc, view, args).ok()?;
    Some(fades::plan(doc, &target))
}

/// The name Edit ▸ Create ▸ Create Fade shows now (M-CREATE-010): Create
/// Fade In/Out when the selection gives a fade, Create Crossfade when it
/// gives a crossfade, and Create Fade when it gives neither.
#[must_use]
pub fn create_fade_label(doc: &Document, view: &ArrangementView) -> &'static str {
    match create_fade_plan(doc, view, &Value::Object(serde_json::Map::new())) {
        Some(plan) if plan.has_crossfade() => "Create Crossfade",
        Some(plan) if !plan.is_empty() => "Create Fade In/Out",
        _ => "Create Fade",
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

/// Every clip on the lanes.
fn clips_in(doc: &Document, places: &[Place]) -> Vec<ClipId> {
    places
        .iter()
        .flat_map(|place| place.clips(doc).into_iter().map(|clip| clip.id))
        .collect()
}

/// Positions for the UI and tests: `ticks` as an editing command's
/// [`Position`].
#[must_use]
pub fn position(ticks: Ticks) -> Position {
    Position::Ticks(ticks)
}

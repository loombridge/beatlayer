//! Clips between the Session and the Arrangement, and the Session View's
//! clipboard (WP-6.29b).
//!
//! `session.copy_to_arrangement` and `session.copy_to_session` are what
//! dragging clips between the views, over the view selector or between the
//! two windows, and pasting them across the views run; Cut, Copy, Paste and
//! Duplicate in the Session View carry slot clips and scenes on the same
//! clipboard the Arrangement uses. Each is one undo step, and what lands on a
//! slot or a lane replaces what was there. The selection and the clipboard
//! aren't in the document. The behaviour is in the second window spec, R20 to
//! R23, and the session view spec, R39 to R43.

use std::collections::{BTreeMap, BTreeSet};

use bl_core::time::Ticks;
use bl_model::{
    ChangeSet, Clip, ClipId, Command, Document, Editor, EnvelopeKind, FollowActions, FollowPatch,
    Scene, SceneId, TrackId,
};
use bl_shell_spec::session_view::{
    self as ids, ClipboardArgs, CopiedClips, CopyToArrangementArgs, CopyToSessionArgs,
    SessionClipboardState, SlotRef,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::arrangement::ArrangementView;
use crate::arrangement::editing::{
    self, Clipboard, EditFailure, Material, Place, apply, first_selected_track, fits, invalid,
    lanes, paste_position, refused, settled, transaction, unavailable,
};
use crate::session_view::SessionView;

/// The undo step of a drop or a paste in the Arrangement (second-window R23).
pub const COPY_TO_ARRANGEMENT_LABEL: &str = "Copy Clips to Arrangement";
/// The undo step of a drop or a paste of Arrangement clips in the Session View
/// (second-window R23).
pub const COPY_TO_SESSION_LABEL: &str = "Copy Clips to Session";

/// One copied Session clip, with its place counted from the material's first
/// track and first scene.
#[derive(Clone, Debug, PartialEq)]
pub struct SlotClip {
    /// How many tracks it is from the first, in the track list.
    pub column: usize,
    /// How many scenes it is from the first.
    pub row: usize,
    /// The clip.
    pub clip: Clip,
}

/// Clips copied from Session slots, or made from Arrangement clips to go in
/// slots, for pasting.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlotMaterial {
    /// The clips, by track then scene.
    pub clips: Vec<SlotClip>,
}

impl SlotMaterial {
    /// What it holds, as `session.copy` reports it.
    #[must_use]
    pub fn state(&self) -> SessionClipboardState {
        SessionClipboardState {
            clips: self.clips.len(),
            scenes: 0,
        }
    }
}

/// A copied scene: its own settings and the clip in each track's slot.
#[derive(Clone, Debug, PartialEq)]
pub struct CopiedScene {
    /// The scene as it was copied, its ID the source's.
    pub scene: Scene,
    /// The clips it held, with the column of the track each was in, counted
    /// from the first track of the set, so another set can take them.
    pub clips: Vec<(usize, Clip)>,
}

/// Scenes copied from the Session View, for pasting.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneMaterial {
    /// The scenes, in the order they were in.
    pub scenes: Vec<CopiedScene>,
}

/// What a copy between the views, or a paste, placed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Placed {
    /// The new clips in the Arrangement.
    pub created: Vec<ClipId>,
    /// The slots that took a clip.
    pub slots: Vec<SlotRef>,
    /// How many clips weren't placed: they would land on a track of the
    /// other kind or past the last track.
    pub skipped: usize,
}

/// What a command did, and what the Status Bar says about it.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    /// What the command returns.
    pub value: Value,
    /// The Status Bar's line, if any.
    pub message: Option<String>,
}

/// Whether `id` is one of the commands in this module.
#[must_use]
pub fn handles(id: &str) -> bool {
    matches!(
        id,
        ids::COPY_TO_ARRANGEMENT
            | ids::COPY_TO_SESSION
            | ids::CUT
            | ids::COPY
            | ids::PASTE
            | ids::DUPLICATE
    )
}

/// What the Status Bar says when `count` clips weren't placed.
#[must_use]
pub fn skipped_message(count: usize) -> Option<String> {
    match count {
        0 => None,
        1 => Some(
            "1 clip wasn't placed: it would land on a track of another kind or past the last \
             track"
                .to_owned(),
        ),
        n => Some(format!(
            "{n} clips weren't placed: they would land on tracks of another kind or past the \
             last track"
        )),
    }
}

fn parse<T: DeserializeOwned>(args: Value) -> Result<T, EditFailure> {
    let args = if args.is_null() {
        Value::Object(serde_json::Map::new())
    } else {
        args
    };
    serde_json::from_value(args).map_err(|error| invalid(error.to_string()))
}

fn position(list: &[TrackId], track: TrackId) -> Option<usize> {
    list.iter().position(|t| *t == track)
}

/// Where a slot clip sits: its track's place in the track list, and its
/// scene's among the scenes.
fn slot_of(doc: &Document, tracks: &[TrackId], clip: ClipId) -> Option<(usize, usize)> {
    let track = doc.clip_track(clip)?;
    let (scene, _) = doc
        .track(track)?
        .slots()
        .find(|(_, held)| held.id == clip)?;
    Some((position(tracks, track)?, doc.scene_position(scene)?))
}

/// The clips of the selected slots.
fn selected_clips(doc: &Document, session: &SessionView) -> Vec<ClipId> {
    session
        .selected_slots()
        .iter()
        .filter_map(|slot| doc.slot(slot.track, slot.scene))
        .map(|clip| clip.id)
        .collect()
}

/// Session clips as material: their places counted from the first track and
/// the first scene among them.
///
/// # Errors
///
/// [`EditFailure`] if a clip doesn't exist or isn't in a slot.
pub fn material_of_slot_clips(
    doc: &Document,
    clips: &[ClipId],
) -> Result<SlotMaterial, EditFailure> {
    let tracks = lanes(doc);
    let mut found = Vec::with_capacity(clips.len());
    let mut seen = BTreeSet::new();
    for id in clips {
        if !seen.insert(*id) {
            continue;
        }
        let clip = doc
            .clip(*id)
            .ok_or_else(|| invalid(format!("no clip has the ID {}", id.0)))?;
        let (column, row) = slot_of(doc, &tracks, *id)
            .ok_or_else(|| invalid(format!("clip {} isn't in a Session slot", id.0)))?;
        found.push((column, row, clip.clone()));
    }
    if found.is_empty() {
        return Err(invalid("`clips` is empty"));
    }
    Ok(normalised(found))
}

/// Places counted from the first column and the first row among them.
fn normalised(mut found: Vec<(usize, usize, Clip)>) -> SlotMaterial {
    let column = found
        .iter()
        .map(|(column, _, _)| *column)
        .min()
        .unwrap_or(0);
    let row = found.iter().map(|(_, row, _)| *row).min().unwrap_or(0);
    found.sort_by_key(|(c, r, _)| (*c, *r));
    SlotMaterial {
        clips: found
            .into_iter()
            .map(|(c, r, clip)| SlotClip {
                column: c - column,
                row: r - row,
                clip,
            })
            .collect(),
    }
}

/// Arrangement clips as material for slots: each track's clips down the
/// scenes in the order they play, one a slot, the tracks across the grid as
/// far apart as they are in the track list (second-window R21).
///
/// # Errors
///
/// [`EditFailure`] if a clip doesn't exist or is a Session clip.
pub fn material_of_arrangement_clips(
    doc: &Document,
    clips: &[ClipId],
) -> Result<SlotMaterial, EditFailure> {
    let tracks = lanes(doc);
    let mut by_track: BTreeMap<usize, Vec<&Clip>> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for id in clips {
        if !seen.insert(*id) {
            continue;
        }
        let clip = doc
            .clip(*id)
            .ok_or_else(|| invalid(format!("no clip has the ID {}", id.0)))?;
        if slot_of(doc, &tracks, *id).is_some() {
            return Err(invalid(format!(
                "clip {} is a Session clip: use `session.copy_clip_to_slot`",
                id.0
            )));
        }
        let column = doc
            .clip_track(*id)
            .and_then(|track| position(&tracks, track))
            .ok_or_else(|| invalid(format!("clip {} isn't on a track's lane", id.0)))?;
        by_track.entry(column).or_default().push(clip);
    }
    if by_track.is_empty() {
        return Err(invalid("`clips` is empty"));
    }
    let first = *by_track.keys().next().expect("not empty");
    let mut found = Vec::new();
    for (column, mut held) in by_track {
        held.sort_by_key(|clip| (clip.start, clip.id));
        found.extend(
            held.into_iter()
                .enumerate()
                .map(|(row, clip)| (column - first, row, clip.clone())),
        );
    }
    Ok(normalised(found))
}

/// The Arrangement's copied material as material for slots, as
/// [`material_of_arrangement_clips`] makes it.
#[must_use]
pub fn material_of_clipboard(material: &Material) -> SlotMaterial {
    let mut by_distance: BTreeMap<usize, Vec<&Clip>> = BTreeMap::new();
    for lane in &material.lanes {
        by_distance
            .entry(lane.distance)
            .or_default()
            .extend(lane.clips.iter());
    }
    let mut found = Vec::new();
    for (distance, mut held) in by_distance {
        held.sort_by_key(|clip| (clip.start, clip.id));
        found.extend(
            held.into_iter()
                .enumerate()
                .map(|(row, clip)| (distance, row, clip.clone())),
        );
    }
    normalised(found)
}

/// `clip` as it goes on `track`, in a slot or on a lane: its envelopes that
/// the track can't take are left behind (an Arrangement clip's only modulate,
/// and a parameter of another track isn't this track's), and a groove the pool
/// no longer holds.
fn carried(doc: &Document, track: TrackId, clip: &Clip, in_slot: bool) -> Clip {
    let mut carried = clip.clone();
    carried.envelopes.retain(|envelope| {
        (in_slot || envelope.kind != EnvelopeKind::Automation)
            && doc.check_clip_target(track, clip, envelope.target).is_ok()
    });
    carried.groove = carried
        .groove
        .filter(|groove| doc.settings().grooves.get(*groove).is_some());
    carried
}

/// Copies Session clips into the Arrangement (second-window R20): `base` is
/// the track the material's first track goes to, the others follow in the
/// track list, and each track's clips lie one after another from `at`. What
/// they cover is replaced. One undo step.
///
/// # Errors
///
/// [`EditFailure`] if `base` isn't a track that holds clips, or the set
/// refuses the edit.
pub fn copy_to_arrangement(
    editor: &mut Editor,
    material: &SlotMaterial,
    base: TrackId,
    at: Ticks,
    edge_fades: bool,
) -> Result<(Placed, bool), EditFailure> {
    let doc = editor.document().clone();
    let tracks = lanes(&doc);
    let first = position(&tracks, base)
        .ok_or_else(|| refused("clips can't go on a return track or the Main track"))?;
    let mut by_column: BTreeMap<usize, Vec<&SlotClip>> = BTreeMap::new();
    for held in &material.clips {
        by_column.entry(held.column).or_default().push(held);
    }
    let mut placements = Vec::new();
    let mut skipped = 0;
    for (column, mut held) in by_column {
        held.sort_by_key(|held| held.row);
        let Some(&track) = tracks.get(first + column) else {
            skipped += held.len();
            continue;
        };
        let mut start = at;
        let mut clips = Vec::new();
        for slot in held {
            if !fits(&doc, track, &slot.clip.data) {
                skipped += 1;
                continue;
            }
            let clip = carried(&doc, track, &slot.clip, false);
            let length = clip.length;
            clips.push(Clip { start, ..clip });
            start += length;
        }
        if !clips.is_empty() {
            placements.push((Place::main(track), clips));
        }
    }
    let (created, recorded) = settled(
        editor,
        COPY_TO_ARRANGEMENT_LABEL,
        (&doc, edge_fades),
        |editor| editing::place(editor, placements),
    )?;
    Ok((
        Placed {
            created,
            skipped,
            ..Placed::default()
        },
        recorded,
    ))
}

/// Copies clips into Session slots (second-window R21): the material's first
/// track goes to track number `column` of the track list and its first scene
/// to scene number `row`, the rest keeping their places, and scenes are added
/// at the end when the clips run past the last. A clip landing on a full slot
/// replaces its clip. One undo step.
fn place_in_session(
    editor: &mut Editor,
    material: &SlotMaterial,
    (column, row): (usize, usize),
    label: &'static str,
) -> Result<(Placed, bool), EditFailure> {
    let doc = editor.document().clone();
    let tracks = lanes(&doc);
    let mut targets = Vec::with_capacity(material.clips.len());
    let mut skipped = 0;
    for slot in &material.clips {
        match tracks.get(column + slot.column) {
            Some(&track) if fits(&doc, track, &slot.clip.data) => {
                targets.push((track, row + slot.row, slot));
            }
            _ => skipped += 1,
        }
    }
    let needed = targets.iter().map(|(_, row, _)| row + 1).max().unwrap_or(0);
    let existing = doc.scenes().count();
    let (slots, recorded) = transaction(editor, label, |editor| {
        for index in existing..needed {
            let id: SceneId = editor.new_id();
            apply(editor, Command::CreateScene { id, index })?;
        }
        let scenes: Vec<SceneId> = editor.document().scenes().map(|scene| scene.id).collect();
        let mut slots = Vec::with_capacity(targets.len());
        for (track, row, slot) in &targets {
            let scene = scenes[*row];
            if let Some(held) = editor.document().slot(*track, scene) {
                let clip = held.id;
                apply(editor, Command::DeleteClip { clip })?;
            }
            let mut clip = carried(editor.document(), *track, &slot.clip, true);
            clip.id = editor.new_id();
            clip.start = Ticks::ZERO;
            let (made, scale) = (clip.id, clip.scale);
            apply(
                editor,
                Command::CreateSlotClip {
                    track: *track,
                    scene,
                    clip,
                },
            )?;
            crate::tracks::keep_scale(editor, made, scale)?;
            slots.push(SlotRef {
                track: *track,
                scene,
            });
        }
        Ok(slots)
    })?;
    Ok((
        Placed {
            slots,
            skipped,
            ..Placed::default()
        },
        recorded,
    ))
}

/// The slot the Session View's selection starts at: its first track and
/// first scene, or the first scene selected with the first track that holds
/// clips.
fn selection_start(doc: &Document, session: &SessionView) -> Option<(usize, usize)> {
    let tracks = lanes(doc);
    let state = session.state();
    let column = state
        .slots
        .iter()
        .filter_map(|slot| position(&tracks, slot.track))
        .min();
    let row = state
        .slots
        .iter()
        .filter_map(|slot| doc.scene_position(slot.scene))
        .min();
    if let (Some(column), Some(row)) = (column, row) {
        return Some((column, row));
    }
    let row = state
        .scenes
        .iter()
        .filter_map(|scene| doc.scene_position(*scene))
        .min()?;
    let column = tracks.iter().position(|track| {
        doc.track(*track).is_some_and(|t| {
            matches!(
                t.kind(),
                bl_model::TrackKind::Audio | bl_model::TrackKind::Midi
            )
        })
    })?;
    Some((column, row))
}

/// The selected slots' clips with their places in the track list and among the
/// scenes: only the selected slots', or, with `all_tracks`, those of every
/// track in the selected slots' scenes.
fn chosen(doc: &Document, session: &SessionView, all_tracks: bool) -> Vec<(usize, usize, Clip)> {
    let tracks = lanes(doc);
    let slots: Vec<SlotRef> = if all_tracks {
        let scenes: BTreeSet<SceneId> = session.selected_slots().iter().map(|s| s.scene).collect();
        let scenes: Vec<SceneId> = doc
            .scenes()
            .map(|scene| scene.id)
            .filter(|scene| scenes.contains(scene))
            .collect();
        scenes
            .iter()
            .flat_map(|scene| {
                tracks.iter().map(move |track| SlotRef {
                    track: *track,
                    scene: *scene,
                })
            })
            .collect()
    } else {
        session.selected_slots().to_vec()
    };
    slots
        .iter()
        .filter_map(|slot| {
            let clip = doc.slot(slot.track, slot.scene)?;
            Some((
                position(&tracks, slot.track)?,
                doc.scene_position(slot.scene)?,
                clip.clone(),
            ))
        })
        .collect()
}

/// The selected scenes as material, in their order.
fn scene_material(doc: &Document, scenes: &[SceneId]) -> SceneMaterial {
    let tracks = lanes(doc);
    SceneMaterial {
        scenes: doc
            .scenes()
            .filter(|scene| scenes.contains(&scene.id))
            .map(|scene| CopiedScene {
                scene: scene.clone(),
                clips: tracks
                    .iter()
                    .enumerate()
                    .filter_map(|(column, track)| {
                        Some((column, doc.slot(*track, scene.id)?.clone()))
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// What a copy of the selection puts on the clipboard: the selected scenes
/// with their clips, or the clips of the selected slots (S-EDIT-014).
fn copied(
    doc: &Document,
    session: &SessionView,
    all_tracks: bool,
) -> Result<Clipboard, EditFailure> {
    let scenes = session.state().scenes;
    if !scenes.is_empty() {
        return Ok(Clipboard::Scenes(scene_material(doc, &scenes)));
    }
    let found = chosen(doc, session, all_tracks);
    if found.is_empty() {
        return Err(unavailable("no clip or scene is selected"));
    }
    Ok(Clipboard::Slots(normalised(found)))
}

fn state_of(clipboard: &Clipboard) -> SessionClipboardState {
    match clipboard {
        Clipboard::Slots(slots) => slots.state(),
        Clipboard::Scenes(scenes) => SessionClipboardState {
            clips: scenes.scenes.iter().map(|scene| scene.clips.len()).sum(),
            scenes: scenes.scenes.len(),
        },
        _ => SessionClipboardState::default(),
    }
}

/// The follow actions of a copied scene as the patch that gives them to a new
/// one.
fn follow_patch(follow: &FollowActions) -> FollowPatch {
    FollowPatch {
        enabled: Some(follow.enabled),
        action_a: Some(follow.action_a),
        action_b: Some(follow.action_b),
        jump_a: Some(follow.jump_a),
        jump_b: Some(follow.jump_b),
        chance_a: Some(follow.chance_a),
        linked: Some(follow.linked),
        multiplier: Some(follow.multiplier),
        time: Some(follow.time),
    }
}

/// Inserts copies of scenes after position `after` (the end when `None`), each
/// with its name, tempo, signature, follow actions and clips, as new scenes
/// with new clips. One undo step.
fn paste_scenes(
    editor: &mut Editor,
    material: &SceneMaterial,
    after: Option<usize>,
    label: &'static str,
) -> Result<(Vec<SceneId>, bool), EditFailure> {
    let first = after.map_or_else(|| editor.document().scenes().count(), |at| at + 1);
    // Clips go by the column they came from, which another set may not have.
    let tracks = lanes(editor.document());
    transaction(editor, label, |editor| {
        let mut made = Vec::with_capacity(material.scenes.len());
        for (offset, copied) in material.scenes.iter().enumerate() {
            let id: SceneId = editor.new_id();
            apply(
                editor,
                Command::CreateScene {
                    id,
                    index: first + offset,
                },
            )?;
            if !copied.scene.name.is_empty() {
                apply(
                    editor,
                    Command::RenameScene {
                        scene: id,
                        name: copied.scene.name.clone(),
                    },
                )?;
            }
            if copied.scene.tempo.is_some() {
                apply(
                    editor,
                    Command::SetSceneTempo {
                        scene: id,
                        tempo: copied.scene.tempo,
                    },
                )?;
            }
            if copied.scene.signature.is_some() {
                apply(
                    editor,
                    Command::SetSceneTimeSignature {
                        scene: id,
                        signature: copied.scene.signature,
                    },
                )?;
            }
            if copied.scene.follow != FollowActions::default() {
                apply(
                    editor,
                    Command::SetSceneFollowActions {
                        scene: id,
                        patch: follow_patch(&copied.scene.follow),
                    },
                )?;
            }
            for (column, clip) in &copied.clips {
                let doc = editor.document();
                let Some(track) = tracks.get(*column).copied() else {
                    continue;
                };
                if !fits(doc, track, &clip.data) {
                    continue;
                }
                let mut clip = carried(doc, track, clip, true);
                clip.id = editor.new_id();
                clip.start = Ticks::ZERO;
                let (made, scale) = (clip.id, clip.scale);
                apply(
                    editor,
                    Command::CreateSlotClip {
                        track,
                        scene: id,
                        clip,
                    },
                )?;
                crate::tracks::keep_scale(editor, made, scale)?;
            }
            made.push(id);
        }
        Ok(made)
    })
}

fn copied_clips(before: &Document, editor: &Editor, skipped: usize) -> Outcome {
    Outcome {
        value: serde_json::to_value(CopiedClips {
            changes: ChangeSet::between(before, editor.document()),
            skipped,
        })
        .unwrap_or(Value::Null),
        message: skipped_message(skipped),
    }
}

fn changed(before: &Document, editor: &Editor) -> Outcome {
    Outcome {
        value: serde_json::to_value(ChangeSet::between(before, editor.document()))
            .unwrap_or(Value::Null),
        message: None,
    }
}

/// Whether a person can run command `id` with `args` now: there's something
/// selected to act on, or on the clipboard to paste.
#[must_use]
pub fn can_run(
    id: &str,
    args: &Value,
    doc: &Document,
    session: &SessionView,
    view: &ArrangementView,
    clipboard: Option<&Clipboard>,
) -> bool {
    let given = |key: &str| args.get(key).is_some();
    let all_tracks = args
        .get("all_tracks")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let has_scenes = !session.state().scenes.is_empty();
    match id {
        ids::COPY_TO_ARRANGEMENT => {
            (given("clips") || !selected_clips(doc, session).is_empty())
                && (given("track") || first_selected_track(doc, view).is_some())
        }
        ids::COPY_TO_SESSION => {
            (given("clips") || !view.selected_clips().is_empty())
                && ((given("track") && given("scene")) || selection_start(doc, session).is_some())
        }
        ids::COPY | ids::CUT | ids::DUPLICATE => {
            has_scenes || !chosen(doc, session, all_tracks).is_empty()
        }
        ids::PASTE => {
            selection_start(doc, session).is_some()
                && match clipboard {
                    Some(Clipboard::Slots(slots)) => !slots.clips.is_empty(),
                    Some(Clipboard::Scenes(scenes)) => !scenes.scenes.is_empty(),
                    Some(Clipboard::Material(material)) => !material.lanes.is_empty(),
                    _ => false,
                }
        }
        _ => false,
    }
}

/// Runs command `id` with `args`, as `Shell::run` does for the Session View's
/// clipboard and the copies between the views.
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
    session: &mut SessionView,
    view: &mut ArrangementView,
    clipboard: &mut Option<Clipboard>,
    edge_fades: bool,
) -> Result<Outcome, EditFailure> {
    let before = editor.document().clone();
    let doc = &before;
    match id {
        ids::COPY_TO_ARRANGEMENT => {
            let CopyToArrangementArgs { clips, track, at } = parse(args)?;
            let clips = match clips {
                Some(clips) => clips,
                None => selected_clips(doc, session),
            };
            if clips.is_empty() {
                return Err(unavailable("no Session clip is selected"));
            }
            let material = material_of_slot_clips(doc, &clips)?;
            let base = match track {
                Some(track) => {
                    if doc.track(track).is_none() {
                        return Err(invalid(format!("no track has the ID {}", track.0)));
                    }
                    track
                }
                None => first_selected_track(doc, view)
                    .ok_or_else(|| unavailable("no track is selected to copy the clips to"))?,
            };
            let at = match at {
                Some(at) => at
                    .resolve(&doc.settings().time_signatures)
                    .map_err(invalid)?,
                None => paste_position(view),
            };
            let (placed, _) = copy_to_arrangement(editor, &material, base, at, edge_fades)?;
            let skipped = placed.skipped;
            if !placed.created.is_empty() {
                view.select_clips(editor.document(), placed.created);
            }
            Ok(copied_clips(doc, editor, skipped))
        }
        ids::COPY_TO_SESSION => {
            let CopyToSessionArgs {
                clips,
                track,
                scene,
            } = parse(args)?;
            let clips = clips.unwrap_or_else(|| view.selected_clips().to_vec());
            if clips.is_empty() {
                return Err(unavailable("no Arrangement clip is selected"));
            }
            let material = material_of_arrangement_clips(doc, &clips)?;
            let start = destination(doc, session, track, scene)?;
            let (placed, _) = place_in_session(editor, &material, start, COPY_TO_SESSION_LABEL)?;
            let skipped = placed.skipped;
            if !placed.slots.is_empty() {
                session.select_slots(placed.slots);
            }
            Ok(copied_clips(doc, editor, skipped))
        }
        ids::COPY => {
            let ClipboardArgs { all_tracks } = parse(args)?;
            let copy = copied(doc, session, all_tracks)?;
            let state = state_of(&copy);
            *clipboard = Some(copy);
            Ok(Outcome {
                value: serde_json::to_value(state).unwrap_or(Value::Null),
                message: None,
            })
        }
        ids::CUT => {
            let ClipboardArgs { all_tracks } = parse(args)?;
            let copy = copied(doc, session, all_tracks)?;
            transaction(editor, "Cut", |editor| {
                match &copy {
                    Clipboard::Scenes(scenes) => {
                        for held in scenes.scenes.iter().rev() {
                            apply(
                                editor,
                                Command::DeleteScene {
                                    scene: held.scene.id,
                                },
                            )?;
                        }
                    }
                    _ => {
                        for (_, _, clip) in chosen(doc, session, all_tracks) {
                            apply(editor, Command::DeleteClip { clip: clip.id })?;
                        }
                    }
                }
                Ok(())
            })?;
            *clipboard = Some(copy);
            session.forget_missing(editor.document());
            Ok(changed(doc, editor))
        }
        ids::PASTE => {
            let ClipboardArgs { all_tracks } = parse(args)?;
            let Some(held) = clipboard.clone() else {
                return Err(unavailable("there's nothing on the clipboard to paste"));
            };
            let start = selection_start(doc, session)
                .ok_or_else(|| unavailable("select a slot or a scene to paste into"))?;
            let column = if all_tracks {
                lanes(doc)
                    .iter()
                    .position(|track| {
                        doc.track(*track).is_some_and(|t| {
                            matches!(
                                t.kind(),
                                bl_model::TrackKind::Audio | bl_model::TrackKind::Midi
                            )
                        })
                    })
                    .unwrap_or(start.0)
            } else {
                start.0
            };
            match held {
                Clipboard::Slots(slots) => {
                    let (placed, _) = place_in_session(editor, &slots, (column, start.1), "Paste")?;
                    let skipped = placed.skipped;
                    if !placed.slots.is_empty() {
                        session.select_slots(placed.slots);
                    }
                    Ok(copied_clips(doc, editor, skipped))
                }
                Clipboard::Material(material) if !material.lanes.is_empty() => {
                    // Spec: second-window R22. The Arrangement's clips go down
                    // the scenes from the selected slot.
                    let material = material_of_clipboard(&material);
                    let (placed, _) = place_in_session(
                        editor,
                        &material,
                        (column, start.1),
                        COPY_TO_SESSION_LABEL,
                    )?;
                    let skipped = placed.skipped;
                    if !placed.slots.is_empty() {
                        session.select_slots(placed.slots);
                    }
                    Ok(copied_clips(doc, editor, skipped))
                }
                Clipboard::Scenes(scenes) => {
                    let state = session.state();
                    let after = state
                        .scenes
                        .iter()
                        .chain(state.slots.iter().map(|slot| &slot.scene))
                        .filter_map(|scene| doc.scene_position(*scene))
                        .max();
                    let (made, _) = paste_scenes(editor, &scenes, after, "Paste")?;
                    session.select_scenes(made);
                    Ok(changed(doc, editor))
                }
                Clipboard::Material(_) => {
                    Err(unavailable("there's nothing on the clipboard to paste"))
                }
                Clipboard::Tracks(_) | Clipboard::Time(_) => Err(unavailable(
                    "the clipboard holds tracks or time from the Arrangement; paste it there",
                )),
            }
        }
        ids::DUPLICATE => {
            let ClipboardArgs { all_tracks } = parse(args)?;
            let scenes = session.state().scenes;
            if !scenes.is_empty() {
                // A selected scene is copied with its clips, straight after
                // it, as `session.duplicate_scene` does.
                let tracks = lanes(doc);
                let (made, _) = transaction(editor, "Duplicate Scene", |editor| {
                    let mut made = Vec::new();
                    for scene in doc.scenes().filter(|s| scenes.contains(&s.id)) {
                        let clips: Vec<(TrackId, ClipId)> = tracks
                            .iter()
                            .filter(|track| doc.slot(**track, scene.id).is_some())
                            .map(|track| (*track, editor.new_id()))
                            .collect();
                        let copy: SceneId = editor.new_id();
                        apply(
                            editor,
                            Command::DuplicateScene {
                                scene: scene.id,
                                id: copy,
                                clips,
                            },
                        )?;
                        made.push(copy);
                    }
                    Ok(made)
                })?;
                session.select_scenes(made);
                return Ok(changed(doc, editor));
            }
            let found = chosen(doc, session, all_tracks);
            if found.is_empty() {
                return Err(unavailable("no clip is selected"));
            }
            // The copies go straight below the selection's rows.
            let column = found.iter().map(|(c, _, _)| *c).min().unwrap_or(0);
            let bottom = found.iter().map(|(_, r, _)| *r).max().unwrap_or(0);
            let material = normalised(found);
            let (placed, _) =
                place_in_session(editor, &material, (column, bottom + 1), "Duplicate")?;
            let skipped = placed.skipped;
            if !placed.slots.is_empty() {
                session.select_slots(placed.slots);
            }
            Ok(copied_clips(doc, editor, skipped))
        }
        other => Err(invalid(format!(
            "`{other}` isn't a Session clipboard command"
        ))),
    }
}

/// The track and scene a copy into the Session View starts at: those given,
/// or the selected slot's.
fn destination(
    doc: &Document,
    session: &SessionView,
    track: Option<TrackId>,
    scene: Option<SceneId>,
) -> Result<(usize, usize), EditFailure> {
    let tracks = lanes(doc);
    let selected = selection_start(doc, session);
    let column = match track {
        Some(track) => position(&tracks, track)
            .ok_or_else(|| refused("clips can't go on a return track or the Main track"))?,
        None => selected
            .map(|(column, _)| column)
            .ok_or_else(|| unavailable("select a slot first, or name its track"))?,
    };
    let row = match scene {
        Some(scene) => doc
            .scene_position(scene)
            .ok_or_else(|| invalid(format!("no scene has the ID {}", scene.0)))?,
        None => selected.map_or(0, |(_, row)| row),
    };
    Ok((column, row))
}

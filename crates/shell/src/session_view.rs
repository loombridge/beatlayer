//! The Session View grid (WP-3.7): what it has selected, how far the scenes
//! are scrolled, and the `session.*` commands that change it and edit the
//! set through `bl-model`'s scene and slot commands.
//!
//! A slot is a track's column and a scene's row; clicking one selects it
//! (I-CLICK-002), Shift-click selects from the last click to it, and
//! Cmd-click (Ctrl on Windows and Linux) adds it or takes it out
//! (I-CLICK-033). The selection and the scroll are view state, never part of
//! the set; what a scene or a slot holds is `bl-model`'s. The commands that
//! edit the set fill in the new IDs a gesture can't name and run the model
//! commands as one undo step. The behaviour they follow is in the session
//! view spec.

use std::path::Path;

use bl_core::time::{Bpm, Ticks, TimeSignature};
use bl_media::MediaStore;
use bl_model::{
    ChangeSet, Clip, ClipData, ClipId, Editor, LaunchMode, LaunchQuantization, MidiData, SceneId,
    TrackId,
};
use bl_shell_spec::session_view::{
    self as ids, CopyClipArgs, DropFilesArgs, DuplicateSceneArgs, InsertClipArgs, InsertSceneArgs,
    ScrollArgs, SelectArgs, SelectEdge, SelectEdgeArgs, SessionSelectMode, SessionViewState,
    SlotRef,
};
use serde_json::Value;

use crate::browser::arrange::MediaFile;

/// How tall a clip slot row is, in points (layout.md, L-SES-003): the
/// scenes the scroll is kept within are counted in these.
pub const SLOT_ROW: f64 = 18.0;

/// Why a Session View command didn't run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionViewError {
    /// The arguments don't fit.
    InvalidArgs(String),
    /// The set doesn't allow it now.
    Unavailable(String),
    /// The set refused the edit.
    Failed(String),
}

/// What a Session View command had from the set and the media.
pub struct Context<'a> {
    /// The set's document and its undo history.
    pub editor: &'a mut Editor,
    /// The set's media, which a dropped file is read into.
    pub media: &'a MediaStore,
    /// The tracks with a column, left to right.
    pub columns: &'a [TrackId],
    /// The scenes, top to bottom.
    pub scenes: &'a [SceneId],
    /// Whether a new clip's edges fade (Settings ▸ Create Fades on Clip
    /// Edges).
    pub edge_fades: bool,
    /// What a new clip's warp settings are made from (the warping spec,
    /// R33).
    pub warping: crate::auto_warp::Warping<'a>,
    /// The launch mode a new clip starts with (Settings ▸ Default Launch
    /// Mode).
    pub launch_mode: LaunchMode,
    /// The launch quantisation a new clip starts with (Settings ▸ Default
    /// Launch Quantization); `None` follows the global one.
    pub launch_quantization: Option<LaunchQuantization>,
    /// What Capture and Insert Scene needs from the engine.
    pub capture: Capture,
}

/// What the engine knows that Capture and Insert Scene captures (WP-3.5).
#[derive(Clone, Debug)]
pub struct Capture {
    /// The slots whose clips play, as `(track, scene)`.
    pub playing: Vec<(TrackId, SceneId)>,
    /// The scene launched last since the set opened, if any.
    pub last_launched: Option<SceneId>,
    /// The tempo in force now.
    pub tempo: Bpm,
    /// The time signature in force now.
    pub signature: TimeSignature,
}

/// What a Session View command changed: its result, the files a drop read
/// into the set's media, and whether the set changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionOutcome {
    /// What the command returns.
    pub value: Value,
    /// The files a drop added, for the set's media table.
    pub media: Vec<MediaFile>,
    /// Whether the set changed (the command was an edit).
    pub document_changed: bool,
    /// The scene Capture and Insert Scene made, when the playing clips move
    /// to its copies.
    pub captured: Option<SceneId>,
}

/// The Session View grid's view state. See the [module docs](self).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionView {
    slots: Vec<SlotRef>,
    scenes: Vec<SceneId>,
    tracks: Vec<TrackId>,
    title_tracks: bool,
    /// The slot a Shift-click extends from.
    anchor: Option<SlotRef>,
    /// The scene a Shift-click extends from.
    scene_anchor: Option<SceneId>,
    /// The track a Shift-click extends from.
    track_anchor: Option<TrackId>,
    scroll_y: f64,
    /// Counts `session.focus` runs, so the window moves the focus once for
    /// each.
    focus_requests: u64,
}

impl SessionView {
    /// A grid with nothing selected.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Back to a new set's grid, keeping the focus count.
    pub fn reset(&mut self) {
        *self = Self {
            focus_requests: self.focus_requests,
            ..Self::new()
        };
    }

    /// What the grid has selected.
    #[must_use]
    pub fn state(&self) -> SessionViewState {
        SessionViewState {
            slots: self.slots.clone(),
            scenes: self.scenes.clone(),
            tracks: self.tracks.clone(),
            title_tracks: self.title_tracks,
            scroll_y: self.scroll_y,
        }
    }

    /// How many times `session.focus` has run.
    #[must_use]
    pub fn focus_requests(&self) -> u64 {
        self.focus_requests
    }

    /// The selected slots, in track then scene order.
    #[must_use]
    pub fn selected_slots(&self) -> &[SlotRef] {
        &self.slots
    }

    /// The first selected slot, which a single-slot command acts on.
    #[must_use]
    pub fn first_slot(&self) -> Option<SlotRef> {
        self.slots.first().copied()
    }

    /// Runs a Session View command.
    ///
    /// # Errors
    ///
    /// [`SessionViewError`] if the arguments don't fit or the set refuses
    /// the edit.
    pub fn run(
        &mut self,
        id: &str,
        args: Value,
        ctx: Context<'_>,
    ) -> Result<SessionOutcome, SessionViewError> {
        let args = if args.is_null() {
            Value::Object(serde_json::Map::new())
        } else {
            args
        };
        let parse = |e: serde_json::Error| SessionViewError::InvalidArgs(e.to_string());
        match id {
            ids::STATE => {
                no_args(args)?;
                Ok(self.state_outcome())
            }
            ids::SELECT => {
                let args: SelectArgs = serde_json::from_value(args).map_err(parse)?;
                self.select(args, ctx.columns, ctx.scenes)?;
                Ok(self.state_outcome())
            }
            ids::SELECT_ALL => {
                no_args(args)?;
                self.slots = every_slot(ctx.columns, ctx.scenes);
                self.scenes.clear();
                self.tracks.clear();
                self.title_tracks = false;
                Ok(self.state_outcome())
            }
            ids::SELECT_EDGE => {
                let args: SelectEdgeArgs = serde_json::from_value(args).map_err(parse)?;
                self.select_edge(args.edge, ctx.columns, ctx.scenes);
                Ok(self.state_outcome())
            }
            ids::SCROLL => {
                let ScrollArgs { y } = serde_json::from_value(args).map_err(parse)?;
                if !y.is_finite() {
                    return Err(SessionViewError::InvalidArgs(
                        "`y` must be a finite number".to_owned(),
                    ));
                }
                let max = ((ctx.scenes.len() as f64) - 1.0).max(0.0) * SLOT_ROW;
                self.scroll_y = (self.scroll_y + y).clamp(0.0, max);
                Ok(self.state_outcome())
            }
            ids::FOCUS => {
                no_args(args)?;
                self.focus_requests = self.focus_requests.wrapping_add(1);
                Ok(self.state_outcome())
            }
            ids::INSERT_SCENE => {
                let insert: InsertSceneArgs = serde_json::from_value(args).map_err(parse)?;
                let index = insert
                    .index
                    .unwrap_or_else(|| self.insert_index(ctx.scenes));
                let id: SceneId = ctx.editor.new_id();
                let changes = self.edit(ctx.editor, "Insert Scene", |editor| {
                    model(
                        editor,
                        "scene.create",
                        &serde_json::json!({ "id": id, "index": index }),
                    )
                })?;
                Ok(edit_outcome(changes))
            }
            ids::DUPLICATE_SCENE => {
                let duplicate: DuplicateSceneArgs = serde_json::from_value(args).map_err(parse)?;
                let scene = duplicate
                    .scene
                    .or_else(|| self.scenes.first().copied())
                    .or_else(|| self.slots.first().map(|slot| slot.scene))
                    .ok_or_else(|| {
                        SessionViewError::Unavailable(
                            "select a scene to duplicate first".to_owned(),
                        )
                    })?;
                let clips = self.slot_clip_ids(ctx.editor, ctx.columns, scene);
                let id: SceneId = ctx.editor.new_id();
                let changes = self.edit(ctx.editor, "Duplicate Scene", |editor| {
                    model(
                        editor,
                        "scene.duplicate",
                        &serde_json::json!({ "scene": scene, "id": id, "clips": clips }),
                    )
                })?;
                Ok(edit_outcome(changes))
            }
            ids::CAPTURE_AND_INSERT_SCENE => {
                no_args(args)?;
                // Spec: session-view R24. After the scene launched last, or
                // else the selected one; a set with no scenes left gets it
                // first.
                let after = ctx
                    .capture
                    .last_launched
                    .filter(|scene| ctx.scenes.contains(scene))
                    .or_else(|| self.scenes.first().copied())
                    .or_else(|| self.slots.first().map(|slot| slot.scene));
                let index = after
                    .and_then(|scene| ctx.scenes.iter().position(|s| *s == scene))
                    .map_or(ctx.scenes.len(), |at| at + 1);
                let id: SceneId = ctx.editor.new_id();
                // A copy of each playing clip, in track order.
                let mut copies = Vec::new();
                for &track in ctx.columns {
                    let playing = ctx
                        .capture
                        .playing
                        .iter()
                        .find(|(playing, _)| *playing == track);
                    let source = playing
                        .and_then(|&(_, scene)| ctx.editor.document().slot(track, scene))
                        .cloned();
                    if let Some(source) = source {
                        let clip = Clip {
                            id: ctx.editor.new_id(),
                            start: Ticks::ZERO,
                            offset: Ticks::ZERO,
                            ..source
                        };
                        copies.push((track, clip));
                    }
                }
                let Capture {
                    tempo, signature, ..
                } = ctx.capture;
                let changes = self.edit(ctx.editor, "Capture and Insert Scene", |editor| {
                    model(
                        editor,
                        "scene.create",
                        &serde_json::json!({ "id": id, "index": index }),
                    )?;
                    model(
                        editor,
                        "scene.set_tempo",
                        &serde_json::json!({ "scene": id, "tempo": tempo }),
                    )?;
                    model(
                        editor,
                        "scene.set_time_signature",
                        &serde_json::json!({ "scene": id, "signature": signature }),
                    )?;
                    for (track, clip) in &copies {
                        model(
                            editor,
                            "clip.create_in_slot",
                            &serde_json::json!({ "track": track, "scene": id, "clip": clip }),
                        )?;
                    }
                    Ok(())
                })?;
                let mut outcome = edit_outcome(changes);
                outcome.captured = Some(id);
                Ok(outcome)
            }
            ids::INSERT_EMPTY_MIDI_CLIP => {
                let insert: InsertClipArgs = serde_json::from_value(args).map_err(parse)?;
                let (track, scene) = self.target_slot(insert.track, insert.scene)?;
                let bar = ctx
                    .editor
                    .document()
                    .settings()
                    .time_signatures
                    .signature_at(Ticks::ZERO)
                    .bar_length();
                let mut clip = Clip::new(
                    ctx.editor.new_id(),
                    Ticks::ZERO,
                    bar,
                    ClipData::Midi(MidiData::new(std::iter::empty())),
                );
                clip.launch.mode = ctx.launch_mode;
                clip.launch.quantization = ctx.launch_quantization;
                let changes = self.edit(ctx.editor, "Insert Empty MIDI Clip", |editor| {
                    model(
                        editor,
                        "clip.create_in_slot",
                        &serde_json::json!({ "track": track, "scene": scene, "clip": clip }),
                    )
                })?;
                Ok(edit_outcome(changes))
            }
            ids::COPY_CLIP_TO_SLOT => {
                let copy: CopyClipArgs = serde_json::from_value(args).map_err(parse)?;
                let source = ctx
                    .editor
                    .document()
                    .clip(copy.clip)
                    .ok_or_else(|| {
                        SessionViewError::InvalidArgs(format!("no clip has the ID {}", copy.clip.0))
                    })?
                    .clone();
                let clip = Clip {
                    id: ctx.editor.new_id(),
                    start: Ticks::ZERO,
                    offset: Ticks::ZERO,
                    ..source
                };
                let changes = self.edit(ctx.editor, "Copy Clip", |editor| {
                    model(
                        editor,
                        "clip.create_in_slot",
                        &serde_json::json!({
                            "track": copy.track,
                            "scene": copy.scene,
                            "clip": clip,
                        }),
                    )
                })?;
                Ok(edit_outcome(changes))
            }
            ids::DELETE => {
                no_args(args)?;
                let scenes = self.scenes.clone();
                let slots = self.slots.clone();
                if scenes.is_empty() && slots.is_empty() {
                    return Ok(self.state_outcome());
                }
                let changes = self.edit(ctx.editor, "Delete", |editor| {
                    for scene in scenes.iter().rev() {
                        model(
                            editor,
                            "scene.delete",
                            &serde_json::json!({ "scene": scene }),
                        )?;
                    }
                    for slot in &slots {
                        if let Some(clip) = editor.document().slot(slot.track, slot.scene) {
                            let clip = clip.id;
                            model(editor, "clip.delete", &serde_json::json!({ "clip": clip }))?;
                        }
                    }
                    Ok(())
                })?;
                self.slots
                    .retain(|slot| ctx.editor.document().slot(slot.track, slot.scene).is_some());
                self.scenes
                    .retain(|scene| ctx.editor.document().scene(*scene).is_some());
                Ok(edit_outcome(changes))
            }
            ids::DEACTIVATE => {
                no_args(args)?;
                let selected: Vec<_> = self
                    .slots
                    .iter()
                    .filter_map(|slot| ctx.editor.document().slot(slot.track, slot.scene))
                    .map(|clip| (clip.id, clip.mute))
                    .collect();
                if selected.is_empty() {
                    return Ok(self.state_outcome());
                }
                // A press switches them off unless they're all off already,
                // which switches them back on.
                let mute = selected.iter().any(|(_id, mute)| !mute);
                let changes = self.edit(ctx.editor, "Deactivate Clip", |editor| {
                    for (clip, _) in &selected {
                        model(
                            editor,
                            "clip.set_mute",
                            &serde_json::json!({ "clip": clip, "mute": mute }),
                        )?;
                    }
                    Ok(())
                })?;
                Ok(edit_outcome(changes))
            }
            ids::TOGGLE_FOLLOW_ACTIONS => {
                no_args(args)?;
                let selected: Vec<_> = self
                    .slots
                    .iter()
                    .filter_map(|slot| ctx.editor.document().slot(slot.track, slot.scene))
                    .map(|clip| (clip.id, clip.launch.follow.enabled))
                    .collect();
                if selected.is_empty() {
                    return Ok(self.state_outcome());
                }
                // Spec: follow-actions R15. Mixed states go all on, and all
                // on goes all off.
                let enabled = selected.iter().any(|(_id, on)| !on);
                let changes = self.edit(ctx.editor, "Toggle Follow Actions", |editor| {
                    for (clip, _) in &selected {
                        model(
                            editor,
                            "clip.set_follow_actions",
                            &serde_json::json!({ "clip": clip, "enabled": enabled }),
                        )?;
                    }
                    Ok(())
                })?;
                Ok(edit_outcome(changes))
            }
            ids::CREATE_FOLLOW_ACTION_CHAIN => {
                no_args(args)?;
                let chain = self.follow_chain(ctx.editor, ctx.columns, ctx.scenes);
                if chain.is_empty() {
                    return Ok(self.state_outcome());
                }
                let changes = self.edit(ctx.editor, "Create Follow Action Chain", |editor| {
                    for patch in &chain {
                        model(editor, "clip.set_follow_actions", patch)?;
                    }
                    Ok(())
                })?;
                Ok(edit_outcome(changes))
            }
            ids::DROP_FILES => {
                let args: DropFilesArgs = serde_json::from_value(args).map_err(parse)?;
                self.drop(ctx, args)
            }
            _ => Err(SessionViewError::InvalidArgs(format!(
                "`{id}` isn't a Session View command"
            ))),
        }
    }

    fn state_outcome(&self) -> SessionOutcome {
        SessionOutcome {
            value: serde_json::to_value(self.state()).unwrap_or(Value::Null),
            ..SessionOutcome::default()
        }
    }

    /// The `clip.set_follow_actions` arguments that chain the selected
    /// clips (Spec: follow-actions R16): a chain for each track, in scene
    /// order, each clip on to Next at 100 % unless the next selected clip
    /// isn't the next slot, when it jumps there; the last jumps to the
    /// first.
    fn follow_chain(
        &self,
        editor: &Editor,
        columns: &[TrackId],
        doc_scenes: &[SceneId],
    ) -> Vec<Value> {
        let mut patches = Vec::new();
        for &track in columns {
            let held: Vec<(usize, ClipId)> = doc_scenes
                .iter()
                .enumerate()
                .filter(|(_, scene)| {
                    self.slots
                        .iter()
                        .any(|s| s.track == track && s.scene == **scene)
                })
                .filter_map(|(at, scene)| Some((at, editor.document().slot(track, *scene)?.id)))
                .collect();
            for (n, &(at, clip)) in held.iter().enumerate() {
                let (next, wraps) = held
                    .get(n + 1)
                    .map_or((held[0].0, true), |&(next, _)| (next, false));
                let jump = wraps || next != at + 1;
                let mut patch = serde_json::json!({
                    "clip": clip,
                    "enabled": true,
                    "action_a": if jump { "jump" } else { "next" },
                    "action_b": "no_action",
                    "chance_a": 100,
                    "linked": true,
                    "multiplier": 1,
                });
                if jump {
                    patch["jump_a"] = (next + 1).into();
                }
                patches.push(patch);
            }
        }
        patches
    }

    /// Runs `edit` as one undo step and returns what it changed.
    fn edit(
        &self,
        editor: &mut Editor,
        label: &'static str,
        edit: impl FnOnce(&mut Editor) -> Result<(), SessionViewError>,
    ) -> Result<ChangeSet, SessionViewError> {
        let before = editor.document().clone();
        editor.transact(label, edit)?;
        Ok(ChangeSet::between(&before, editor.document()))
    }

    /// Changes the selection, as a click with `args.mode` does.
    fn select(
        &mut self,
        args: SelectArgs,
        columns: &[TrackId],
        doc_scenes: &[SceneId],
    ) -> Result<(), SessionViewError> {
        match (args.track, args.scene) {
            (Some(track), Some(scene)) => {
                if !columns.contains(&track) || !doc_scenes.contains(&scene) {
                    return Err(SessionViewError::InvalidArgs(
                        "that track or scene has no slot".to_owned(),
                    ));
                }
                let item = SlotRef { track, scene };
                match args.mode {
                    SessionSelectMode::Replace => {
                        self.slots = vec![item];
                        self.scenes.clear();
                        self.tracks.clear();
                        self.anchor = Some(item);
                    }
                    SessionSelectMode::Add => {
                        if let Some(at) = self.slots.iter().position(|s| *s == item) {
                            self.slots.remove(at);
                        } else {
                            self.slots.push(item);
                        }
                        self.anchor = Some(item);
                    }
                    SessionSelectMode::Range => {
                        let from = self.anchor.unwrap_or(item);
                        self.slots = slots_between(from, item, columns, doc_scenes);
                        self.scenes.clear();
                        self.tracks.clear();
                    }
                }
                self.title_tracks = false;
            }
            (None, Some(scene)) => {
                if !doc_scenes.contains(&scene) {
                    return Err(SessionViewError::InvalidArgs(
                        "no scene has that ID".to_owned(),
                    ));
                }
                match args.mode {
                    SessionSelectMode::Replace => {
                        self.scenes = vec![scene];
                        self.slots.clear();
                        self.tracks.clear();
                        self.scene_anchor = Some(scene);
                    }
                    SessionSelectMode::Add => {
                        if let Some(at) = self.scenes.iter().position(|s| *s == scene) {
                            self.scenes.remove(at);
                        } else {
                            self.scenes.push(scene);
                        }
                        self.scene_anchor = Some(scene);
                    }
                    SessionSelectMode::Range => {
                        let from = self.scene_anchor.unwrap_or(scene);
                        self.scenes = scenes_between(from, scene, doc_scenes);
                        self.slots.clear();
                        self.tracks.clear();
                    }
                }
                self.title_tracks = false;
            }
            (Some(track), None) => {
                if !columns.contains(&track) {
                    return Err(SessionViewError::InvalidArgs(
                        "no track has that column".to_owned(),
                    ));
                }
                match args.mode {
                    SessionSelectMode::Replace => {
                        self.tracks = vec![track];
                        self.slots.clear();
                        self.scenes.clear();
                    }
                    SessionSelectMode::Add => {
                        if let Some(at) = self.tracks.iter().position(|t| *t == track) {
                            self.tracks.remove(at);
                        } else {
                            self.tracks.push(track);
                        }
                    }
                    SessionSelectMode::Range => {
                        let from = self.track_anchor.unwrap_or(track);
                        self.tracks = tracks_between(from, track, columns);
                        self.slots.clear();
                        self.scenes.clear();
                    }
                }
                self.track_anchor = Some(track);
                self.title_tracks = !self.tracks.is_empty();
            }
            (None, None) => {
                return Err(SessionViewError::InvalidArgs(
                    "name a track, a scene, or both".to_owned(),
                ));
            }
        }
        Ok(())
    }

    /// Selects the first or last track's slot in the current scene
    /// (S-SES-020 to S-SES-022): on the last track, a second End selects the
    /// scene itself.
    fn select_edge(&mut self, edge: SelectEdge, columns: &[TrackId], doc_scenes: &[SceneId]) {
        if columns.is_empty() || doc_scenes.is_empty() {
            return;
        }
        let scene_at = self
            .slots
            .first()
            .and_then(|slot| doc_scenes.iter().position(|s| *s == slot.scene))
            .or_else(|| {
                self.scenes
                    .first()
                    .and_then(|scene| doc_scenes.iter().position(|s| s == scene))
            })
            .unwrap_or(0);
        let scene = doc_scenes[scene_at];
        if edge == SelectEdge::Last {
            let last = *columns.last().expect("not empty");
            if self
                .slots
                .first()
                .is_some_and(|slot| slot.track == last && slot.scene == scene)
            {
                // A second End on the last track selects the scene.
                self.scenes = vec![scene];
                self.slots.clear();
                self.tracks.clear();
                self.title_tracks = false;
                self.scene_anchor = Some(scene);
                return;
            }
        }
        let track = if edge == SelectEdge::First {
            columns[0]
        } else {
            columns[columns.len() - 1]
        };
        self.slots = vec![SlotRef { track, scene }];
        self.scenes.clear();
        self.tracks.clear();
        self.title_tracks = false;
        self.anchor = self.slots.first().copied();
    }

    /// Where an insert goes with nothing named: below the last selected
    /// scene, or after every scene.
    fn insert_index(&self, doc_scenes: &[SceneId]) -> usize {
        self.scenes
            .iter()
            .filter_map(|scene| doc_scenes.iter().position(|s| s == scene))
            .max()
            .map_or(doc_scenes.len(), |at| at + 1)
    }

    /// The slot a single-slot command acts on: what was named, or the single
    /// selected slot.
    fn target_slot(
        &self,
        track: Option<TrackId>,
        scene: Option<SceneId>,
    ) -> Result<(TrackId, SceneId), SessionViewError> {
        match (track, scene) {
            (Some(track), Some(scene)) => Ok((track, scene)),
            _ => self
                .first_slot()
                .map(|slot| (slot.track, slot.scene))
                .ok_or_else(|| {
                    SessionViewError::Unavailable(
                        "select a slot first, or name its track and scene".to_owned(),
                    )
                }),
        }
    }

    /// The tracks that hold a clip in `scene`, each with the new ID its copy
    /// takes.
    fn slot_clip_ids(
        &self,
        editor: &mut Editor,
        columns: &[TrackId],
        scene: SceneId,
    ) -> Vec<(TrackId, ClipId)> {
        let holding: Vec<TrackId> = columns
            .iter()
            .copied()
            .filter(|track| editor.document().slot(*track, scene).is_some())
            .collect();
        holding
            .into_iter()
            .map(|track| (track, editor.new_id()))
            .collect()
    }

    /// Drops audio files into slots, reading each into the set's media.
    fn drop(
        &mut self,
        ctx: Context<'_>,
        args: DropFilesArgs,
    ) -> Result<SessionOutcome, SessionViewError> {
        use crate::browser::arrange;
        if args.paths.is_empty() {
            return Err(SessionViewError::InvalidArgs(
                "`paths` lists no files".to_owned(),
            ));
        }
        let start = ctx
            .scenes
            .iter()
            .position(|scene| *scene == args.scene)
            .ok_or_else(|| SessionViewError::InvalidArgs("no scene has that ID".to_owned()))?;
        let track_at = ctx
            .columns
            .iter()
            .position(|track| *track == args.track)
            .ok_or_else(|| SessionViewError::InvalidArgs("no track has that column".to_owned()))?;
        // Each file is read before anything changes, so a file that can't be
        // read changes nothing.
        let mut prepared = Vec::with_capacity(args.paths.len());
        for path in &args.paths {
            if !arrange::is_audio(path) {
                return Err(SessionViewError::InvalidArgs(format!(
                    "{path} isn't an audio file"
                )));
            }
            let (media, name, seconds) = arrange::prepare_for_slot(ctx.media, Path::new(path))
                .map_err(|error| match error {
                    arrange::AddError::InvalidArgs(message) => {
                        SessionViewError::InvalidArgs(message)
                    }
                    arrange::AddError::Failed(message) => SessionViewError::Failed(message),
                })?;
            prepared.push((media, name, seconds));
        }
        // The slot each file goes in: down the scenes of `track`, or, with
        // `spread` (the command modifier held), across the tracks at
        // `scene`.
        let targets: Vec<(TrackId, SceneId)> = (0..prepared.len())
            .filter_map(|index| {
                if args.spread {
                    ctx.columns
                        .get(track_at + index)
                        .map(|track| (*track, args.scene))
                } else {
                    ctx.scenes
                        .get(start + index)
                        .map(|scene| (args.track, *scene))
                }
            })
            .collect();
        let mut outcome = SessionOutcome {
            document_changed: true,
            ..SessionOutcome::default()
        };
        let before = ctx.editor.document().clone();
        ctx.editor.transact("Insert Clip", |editor| {
            for ((media, name, seconds), (track, scene)) in prepared.iter().zip(&targets) {
                let new = crate::auto_warp::new_clip(
                    media.hash,
                    *seconds,
                    editor.document(),
                    Ticks::ZERO,
                    ctx.warping,
                );
                let (length, audio) = (new.length, new.audio);
                let audio = if ctx.edge_fades {
                    audio.with_edge_fades()
                } else {
                    audio
                };
                let mut clip =
                    Clip::new(editor.new_id(), Ticks::ZERO, length, ClipData::Audio(audio));
                clip.name.clone_from(name);
                clip.launch.mode = ctx.launch_mode;
                clip.launch.quantization = ctx.launch_quantization;
                model(
                    editor,
                    "clip.create_in_slot",
                    &serde_json::json!({ "track": track, "scene": scene, "clip": clip }),
                )?;
            }
            Ok(())
        })?;
        outcome.value = serde_json::to_value(ChangeSet::between(&before, ctx.editor.document()))
            .unwrap_or(Value::Null);
        outcome.media = prepared.into_iter().map(|(media, _, _)| media).collect();
        Ok(outcome)
    }
}

/// A `bl-commands` `NoArgs` argument: the command takes none.
fn no_args(args: Value) -> Result<(), SessionViewError> {
    serde_json::from_value::<bl_commands::NoArgs>(args)
        .map(|_| ())
        .map_err(|e| SessionViewError::InvalidArgs(e.to_string()))
}

/// Every slot of `columns` across `doc_scenes`, in order.
fn every_slot(columns: &[TrackId], doc_scenes: &[SceneId]) -> Vec<SlotRef> {
    columns
        .iter()
        .flat_map(|track| {
            doc_scenes.iter().map(move |scene| SlotRef {
                track: *track,
                scene: *scene,
            })
        })
        .collect()
}

/// The slots in the rectangle between two slots.
fn slots_between(
    a: SlotRef,
    b: SlotRef,
    columns: &[TrackId],
    doc_scenes: &[SceneId],
) -> Vec<SlotRef> {
    let (Some(ta), Some(tb)) = (
        columns.iter().position(|t| *t == a.track),
        columns.iter().position(|t| *t == b.track),
    ) else {
        return vec![b];
    };
    let (Some(sa), Some(sb)) = (
        doc_scenes.iter().position(|s| *s == a.scene),
        doc_scenes.iter().position(|s| *s == b.scene),
    ) else {
        return vec![b];
    };
    let (t0, t1) = (ta.min(tb), ta.max(tb));
    let (s0, s1) = (sa.min(sb), sa.max(sb));
    columns[t0..=t1]
        .iter()
        .flat_map(|track| {
            doc_scenes[s0..=s1].iter().map(move |scene| SlotRef {
                track: *track,
                scene: *scene,
            })
        })
        .collect()
}

/// The scenes from `a` to `b`, inclusive, in order.
fn scenes_between(a: SceneId, b: SceneId, doc_scenes: &[SceneId]) -> Vec<SceneId> {
    let (Some(ia), Some(ib)) = (
        doc_scenes.iter().position(|s| *s == a),
        doc_scenes.iter().position(|s| *s == b),
    ) else {
        return vec![b];
    };
    let (i0, i1) = (ia.min(ib), ia.max(ib));
    doc_scenes[i0..=i1].to_vec()
}

/// The tracks from `a` to `b`, inclusive, in order.
fn tracks_between(a: TrackId, b: TrackId, columns: &[TrackId]) -> Vec<TrackId> {
    let (Some(ia), Some(ib)) = (
        columns.iter().position(|t| *t == a),
        columns.iter().position(|t| *t == b),
    ) else {
        return vec![b];
    };
    let (i0, i1) = (ia.min(ib), ia.max(ib));
    columns[i0..=i1].to_vec()
}

/// Runs a `bl-model` command by ID.
fn model(editor: &mut Editor, id: &str, args: &Value) -> Result<(), SessionViewError> {
    bl_model::registry::run(editor, id, args.clone())
        .map(|_| ())
        .map_err(|error| SessionViewError::Failed(error.to_string()))
}

fn edit_outcome(changes: ChangeSet) -> SessionOutcome {
    SessionOutcome {
        value: serde_json::to_value(changes).unwrap_or(Value::Null),
        document_changed: true,
        ..SessionOutcome::default()
    }
}

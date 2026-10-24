//! The Session View grid's view and editing commands (WP-3.7; decisions D16
//! and D17): selecting slots, scenes, and track title bars, scrolling the
//! scenes, inserting and duplicating scenes, making an empty MIDI clip in a
//! slot, moving and copying a clip between slots, deactivating the selected
//! clips, dropping files into a slot, and copying clips between the Session
//! and the Arrangement (WP-6.29b), with Cut, Copy, Paste and Duplicate.
//!
//! The selection and scroll commands change only what the view shows, so
//! they are in the read class. The rest edit the set: each is one undo step,
//! and each runs `bl-model`'s scene and slot commands, filling in the new
//! IDs a person's gesture can't name. [`register`] adds the specs, which are
//! pure data, so `cargo xtask` can check them and list them in the command
//! reference; `bl-shell` runs them by ID. The behaviour they follow is in
//! the session view spec.

use bl_commands::Context::{Global, Session};
use bl_commands::{Binding, CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_model::{ChangeSet, ClipId, SceneId, TrackId};

use crate::editing::Position;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `session.state`: what the grid shows.
pub const STATE: &str = "session.state";
/// `session.select`: select a slot, a scene, or a track's title bar.
pub const SELECT: &str = "session.select";
/// `session.select_all`: select every slot (Cmd/Ctrl+A).
pub const SELECT_ALL: &str = "session.select_all";
/// `session.select_edge`: select the first or last track of the current
/// scene (S-SES-020 to S-SES-022).
pub const SELECT_EDGE: &str = "session.select_edge";
/// `session.scroll`: scroll the scenes and the track columns.
pub const SCROLL: &str = "session.scroll";
/// `session.insert_scene`: insert an empty scene (Create ▸ Insert Scene).
pub const INSERT_SCENE: &str = "session.insert_scene";
/// `session.duplicate_scene`: duplicate the selected scene.
pub const DUPLICATE_SCENE: &str = "session.duplicate_scene";
/// `session.capture_and_insert_scene`: Capture and Insert Scene (Cmd+Shift+I).
pub const CAPTURE_AND_INSERT_SCENE: &str = "session.capture_and_insert_scene";
/// `session.insert_empty_midi_clip`: Insert Empty MIDI Clip (Cmd+Shift+M).
pub const INSERT_EMPTY_MIDI_CLIP: &str = "session.insert_empty_midi_clip";
/// `session.copy_clip_to_slot`: copy a clip into another slot.
pub const COPY_CLIP_TO_SLOT: &str = "session.copy_clip_to_slot";
/// `session.delete`: delete the selected slot clips or scenes.
pub const DELETE: &str = "session.delete";
/// `session.deactivate`: deactivate (or reactivate) the selected clips (0).
pub const DEACTIVATE: &str = "session.deactivate";
/// `session.toggle_follow_actions`: switch the selected clips' follow actions
/// on or off (S-SES-013).
pub const TOGGLE_FOLLOW_ACTIONS: &str = "session.toggle_follow_actions";
/// `session.create_follow_action_chain`: chain the selected clips (S-SES-014).
pub const CREATE_FOLLOW_ACTION_CHAIN: &str = "session.create_follow_action_chain";
/// `session.drop_files`: drop audio files into a slot.
pub const DROP_FILES: &str = "session.drop_files";
/// `session.focus`: move the keyboard focus to the Session View.
pub const FOCUS: &str = "session.focus";
/// `session.copy_to_arrangement`: copy Session clips into the Arrangement.
pub const COPY_TO_ARRANGEMENT: &str = "session.copy_to_arrangement";
/// `session.copy_to_session`: copy Arrangement clips into Session slots.
pub const COPY_TO_SESSION: &str = "session.copy_to_session";
/// `session.cut`: cut the selected slot clips or scenes.
pub const CUT: &str = "session.cut";
/// `session.copy`: copy the selected slot clips or scenes.
pub const COPY: &str = "session.copy";
/// `session.paste`: paste what the clipboard holds into the Session View.
pub const PASTE: &str = "session.paste";
/// `session.duplicate`: duplicate the selected slot clips.
pub const DUPLICATE: &str = "session.duplicate";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    STATE,
    SELECT,
    SELECT_ALL,
    SELECT_EDGE,
    SCROLL,
    INSERT_SCENE,
    DUPLICATE_SCENE,
    CAPTURE_AND_INSERT_SCENE,
    INSERT_EMPTY_MIDI_CLIP,
    COPY_CLIP_TO_SLOT,
    DELETE,
    DEACTIVATE,
    TOGGLE_FOLLOW_ACTIONS,
    CREATE_FOLLOW_ACTION_CHAIN,
    DROP_FILES,
    FOCUS,
    COPY_TO_ARRANGEMENT,
    COPY_TO_SESSION,
    CUT,
    COPY,
    PASTE,
    DUPLICATE,
];

/// Whether `id` is one of the Session View's commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// One slot: a track's column and a scene's row.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
pub struct SlotRef {
    /// The track.
    pub track: TrackId,
    /// The scene.
    pub scene: SceneId,
}

/// How a click changes the selection (the Session View's own modifier
/// click rules).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionSelectMode {
    /// Replaces the selection with what was clicked.
    #[default]
    Replace,
    /// Adds the item to the selection, or removes it when it's already in
    /// it (Cmd/Ctrl-click).
    Add,
    /// Selects from the last clicked item to this one (Shift-click).
    Range,
}

/// What `session.state` and its commands return: what the grid has selected.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionViewState {
    /// The selected slots, in track order then scene order.
    pub slots: Vec<SlotRef>,
    /// The selected scenes (their launch buttons or names).
    pub scenes: Vec<SceneId>,
    /// The selected track title bars.
    pub tracks: Vec<TrackId>,
    /// Whether the selection is track title bars, so Esc has already gone
    /// up as far as it can.
    pub title_tracks: bool,
    /// How far the scenes are scrolled down, in points at 100%.
    pub scroll_y: f64,
}

/// Arguments of `session.select`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectArgs {
    /// The track, for a slot or a title bar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    /// The scene, for a slot or a scene.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<SceneId>,
    /// How the click changes the selection.
    #[serde(default)]
    pub mode: SessionSelectMode,
}

/// Which edge of the current scene to select (S-SES-020 to S-SES-022).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SelectEdge {
    /// The first track of the current scene (Home).
    #[default]
    First,
    /// The last track of the current scene; pressed again on the last track,
    /// the scene itself (End).
    Last,
}

/// Arguments of `session.select_edge`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectEdgeArgs {
    /// Which edge to select.
    #[serde(default)]
    pub edge: SelectEdge,
}

/// Arguments of `session.scroll`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScrollArgs {
    /// Points to scroll the scenes down: positive towards later scenes.
    #[serde(default)]
    pub y: f64,
}

/// Arguments of `session.insert_scene`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InsertSceneArgs {
    /// Where it goes among the scenes, from 0 to the number of scenes; below
    /// the selection when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

/// Arguments of `session.duplicate_scene`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DuplicateSceneArgs {
    /// The scene; the selected one when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<SceneId>,
}

/// Arguments of `session.insert_empty_midi_clip`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InsertClipArgs {
    /// The track; the selected slot's when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    /// The scene; the selected slot's when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<SceneId>,
}

/// Arguments of `session.copy_clip_to_slot`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CopyClipArgs {
    /// The clip to copy.
    pub clip: ClipId,
    /// The track whose slot it goes in.
    pub track: TrackId,
    /// The scene whose slot it goes in.
    pub scene: SceneId,
}

/// Arguments of `session.drop_files`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DropFilesArgs {
    /// The files' paths, in the order dropped.
    pub paths: Vec<String>,
    /// The track whose slot they go in. Left out, the Session's drop area:
    /// clip files make a new track of their kind each, in the scene (audio
    /// files need a track).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    /// The scene whose slot the first one goes in.
    pub scene: SceneId,
    /// With the command modifier (Cmd, Ctrl on Windows and Linux) held,
    /// spread them across the tracks instead of down the scenes.
    #[serde(default)]
    pub spread: bool,
}

/// Arguments of `session.copy_to_arrangement`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CopyToArrangementArgs {
    /// The Session clips to copy, by the IDs they have in their slots. Left
    /// out, the clips in the selected slots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipId>>,
    /// The track the first clip goes to; the clips of other tracks go to the
    /// tracks after it, as far down as they were across the grid. Left out,
    /// the first selected track of the Arrangement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    /// Where the first clip of each track starts. Left out, the time
    /// selection's start, or the insert marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Position>,
}

/// Arguments of `session.copy_to_session`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CopyToSessionArgs {
    /// The Arrangement clips to copy. Left out, the selected clips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipId>>,
    /// The track whose slot takes the first clip; the clips of other tracks
    /// go to the tracks after it. Left out, the selected slot's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    /// The scene whose slot takes the first clip of each track; its next
    /// clips go down the scenes after it. Left out, the selected slot's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<SceneId>,
}

/// Arguments of `session.cut`, `session.copy`, `session.paste` and
/// `session.duplicate`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClipboardArgs {
    /// With Shift held (S-EDIT-014): act on the clips and slots of every
    /// track in the selected scenes, or paste the copied clips from the first
    /// track, instead of the selected slots' own.
    #[serde(default)]
    pub all_tracks: bool,
}

/// What `session.copy` and `session.cut` put on the clipboard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionClipboardState {
    /// How many slot clips it holds.
    pub clips: usize,
    /// How many scenes it holds.
    pub scenes: usize,
}

/// What copying clips between the views did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CopiedClips {
    /// What the edit changed in the set.
    pub changes: ChangeSet,
    /// How many clips weren't placed, because they would land on a track of
    /// another kind or past the last track.
    pub skipped: usize,
}

/// Adds the Session View's commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    for spec in specs() {
        registry.register(spec)?;
    }
    Ok(())
}

/// A view command: read class, returning the grid's state.
fn view(id: &'static str, label: &'static str, description: &'static str) -> CommandSpec {
    CommandSpec::new(id, label)
        .description(description)
        .permission(Permission::Read)
        .result::<SessionViewState>()
}

fn specs() -> Vec<CommandSpec> {
    vec![
        view(
            STATE,
            "Session View State",
            "Reports what the Session View grid has selected: the slots, the scenes (their \
             launch buttons or names), and the track title bars, and how far the scenes are \
             scrolled. Changes nothing.",
        )
        .context(Global)
        .args::<NoArgs>(),
        view(
            SELECT,
            "Select in the Session View",
            "Selects a slot, a scene, or a track's title bar, as clicking the grid does. \
             `track` and `scene` together name a slot, `scene` alone a scene, and `track` \
             alone a title bar. `mode` is `replace` for a plain click, `add` for Cmd-click \
             (Ctrl on Windows and Linux), which adds the item or removes it when it's already \
             selected, and `range` for Shift-click, which selects from the last item clicked \
             to this one. Changes only the view.",
        )
        .context(Session)
        .args::<SelectArgs>(),
        view(
            SELECT_ALL,
            "Select All Slots",
            "Selects every clip slot in the Session View, as Edit > Select All (Cmd-A, Ctrl-A) \
             does in it. Changes only the view.",
        )
        .context(Session)
        .binding("S-SES-003")
        .args::<NoArgs>(),
        view(
            SELECT_EDGE,
            "Select the Scene's First or Last Track",
            "Selects the first (Home) or last (End) track of the current scene, as Home and End \
             do in the Session View. On the last track, End selects the scene itself. Changes \
             only the view.",
        )
        .context(Session)
        .binding(Binding::new("S-SES-020").arg("edge", "first"))
        .binding(Binding::new("S-SES-021").arg("edge", "last"))
        .binding(Binding::new("S-SES-022").arg("edge", "last"))
        .args::<SelectEdgeArgs>(),
        view(
            SCROLL,
            "Scroll the Session View",
            "Scrolls the Session View's scenes by `y` points (positive towards later scenes), \
             as the wheel over the grid does. The returns and Main stay pinned. It can't \
             scroll before the first scene or past the last. Changes only the view.",
        )
        .context(Session)
        .args::<ScrollArgs>(),
        CommandSpec::new(INSERT_SCENE, "Insert Scene")
            .description(
                "Inserts a new, empty scene at `index` among the scenes, as Create > Insert Scene \
             (Cmd-I, Ctrl-I) does; below the selection when `index` is left out. It holds an \
             empty clip slot in every audio and MIDI track. One undo step.",
            )
            .permission(Permission::Edit)
            .context(Session)
            .binding("S-SES-008")
            .args::<InsertSceneArgs>()
            .result::<ChangeSet>(),
        CommandSpec::new(DUPLICATE_SCENE, "Duplicate Scene")
            .description(
                "Copies the selected scene and the clips in its slots into a new scene directly \
             after it, as Edit > Duplicate does. Each copied clip is independent. One undo \
             step.",
            )
            .permission(Permission::Edit)
            .context(Session)
            .args::<DuplicateSceneArgs>()
            .result::<ChangeSet>(),
        CommandSpec::new(CAPTURE_AND_INSERT_SCENE, "Capture and Insert Scene")
            .description(
                "Inserts a new scene holding copies of the clips that are playing, each in its \
             own track's slot, as Create > Capture and Insert Scene (Cmd-Shift-I, Ctrl-Shift-I) \
             does, and the playing clips carry on in the copies without a break. The scene goes \
             after the scene launched last, or after the selected scene when none has been \
             launched since the set opened, and its tempo and time signature are set to the \
             ones in force now. With nothing playing it inserts an empty scene with the same \
             tempo and signature, and launches nothing. One undo step. The playing clips are \
             the engine's, so it needs an engine that runs.",
            )
            .permission(Permission::Edit)
            .context(Session)
            .binding("S-SES-009")
            .args::<NoArgs>()
            .result::<ChangeSet>(),
        CommandSpec::new(INSERT_EMPTY_MIDI_CLIP, "Insert Empty MIDI Clip")
            .description(
                "Makes an empty one-bar MIDI clip in an empty MIDI slot, as Insert Empty MIDI Clip \
             (Cmd-Shift-M, Ctrl-Shift-M) and double-clicking an empty MIDI slot do. The slot \
             is the selected one, or the one `track` and `scene` name. The clip is one bar in \
             the time signature in force at its start. One undo step.",
            )
            .permission(Permission::Edit)
            .context(Session)
            .binding("S-SES-007")
            .args::<InsertClipArgs>()
            .result::<ChangeSet>(),
        CommandSpec::new(COPY_CLIP_TO_SLOT, "Copy Clip to Slot")
            .description(
                "Copies a clip into a track's Session slot at `scene`, as Option-dragging (Ctrl on \
             Windows and Linux) a clip between slots does. The target slot must be empty and \
             the clip's data must match the track. The copy gets its own ID. One undo step.",
            )
            .permission(Permission::Edit)
            .context(Session)
            .args::<CopyClipArgs>()
            .result::<ChangeSet>(),
        CommandSpec::new(DELETE, "Delete in the Session View")
            .description(
                "Deletes the Session View's selection: the clips in the selected slots, or the \
             selected scenes with the clips in their slots. Destructive, so an agent asks \
             first. One undo step.",
            )
            .permission(Permission::Destructive)
            .context(Session)
            .args::<NoArgs>()
            .result::<ChangeSet>(),
        CommandSpec::new(DEACTIVATE, "Deactivate Clips")
            .description(
                "Switches the selected slots' clips off, or back on when they're all off, as \
             pressing 0 does: a deactivated clip stays in place but doesn't play. One undo \
             step.",
            )
            .permission(Permission::Edit)
            .context(Session)
            .binding("S-SES-018")
            .args::<NoArgs>()
            .result::<ChangeSet>(),
        CommandSpec::new(TOGGLE_FOLLOW_ACTIONS, "Toggle Follow Actions")
            .description(
                "Switches the Follow Action switch of the selected slots' clips on, or off when \
             they are all on already, as Shift+Enter does: with some on and some off, they all \
             go on. One undo step.",
            )
            .permission(Permission::Edit)
            .context(Session)
            .binding("S-SES-013")
            .args::<NoArgs>()
            .result::<ChangeSet>(),
        CommandSpec::new(CREATE_FOLLOW_ACTION_CHAIN, "Create Follow Action Chain")
            .description(
                "Chains the selected slots' clips, as the clip menu's Create Follow Action Chain \
             (Cmd+Shift+Enter) does, a chain for each track in scene order: every clip gets \
             Follow Action on, A Next at 100 %, B No Action and Linked 1x, so each launches the \
             one after it when it ends. Where the next clip of the selection isn't the next \
             slot, the clip gets A Jump to its slot instead, and the last clip jumps to the \
             first, so the chain loops within the selection. Loop switches don't change. One \
             undo step.",
            )
            .permission(Permission::Edit)
            .context(Session)
            .binding("S-SES-014")
            .args::<NoArgs>()
            .result::<ChangeSet>(),
        CommandSpec::new(DROP_FILES, "Drop Files into a Slot")
            .description(
                "Puts audio files dropped from the browser, Finder, or Explorer into the Session \
             slot at `track` and `scene`, as dropping them on a slot does: one after another \
             down that track's scenes, or, with `spread` (Cmd, Ctrl on Windows and Linux, held \
             while dropping), across the tracks at `scene`. Each target slot must be empty. \
             Each file becomes a Session clip of its length, starting at the clip's beginning. \
             One undo step. Reads the files, outside the project, and registers them so the set \
             keeps them. Clip files (`.blclip`) are placed as clips: in an empty slot on a track \
             of their kind, bringing the track's devices when it has no clips and no devices; \
             with no `track` (the Session's drop area), each on a new track of its kind in the \
             scene, named after the file, with its devices; with `spread`, each on a track of \
             its own across the tracks at `scene`. A track of the other kind, a group, return \
             or Main track, and a slot holding a clip take none, and the command is refused.",
            )
            .permission(Permission::External)
            .context(Session)
            .args::<DropFilesArgs>()
            .result::<ChangeSet>(),
        CommandSpec::new(COPY_TO_ARRANGEMENT, "Copy Clips to Arrangement")
            .description(
                "Copies Session clips (`clips`, or the clips in the selected slots) into the \
                 Arrangement, as dragging them onto the Arrangement View, between the windows, \
                 or pasting them there does, and leaves the slots as they were. The first \
                 track's first clip starts at `at` (left out, the time selection's start or the \
                 insert marker) on `track` (left out, the first selected track); the clips of \
                 other tracks go to the tracks as far down as they were across the grid, and the \
                 clips of one track from several scenes follow one another in scene order, each \
                 as long as it is. Each keeps its contents, loop and warp settings, and its \
                 modulation envelopes where the new track has what they modulate. The copies \
                 replace what they cover (destructive, so an agent asks first, when that's part \
                 of another clip). A clip that would land on a track of the other kind, or past \
                 the last track, isn't placed, and the result counts them. The copies are \
                 selected afterwards. One undo step, `Copy Clips to Arrangement`.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Session])
            .args::<CopyToArrangementArgs>()
            .result::<CopiedClips>(),
        CommandSpec::new(COPY_TO_SESSION, "Copy Clips to Session")
            .description(
                "Copies Arrangement clips (`clips`, or the selected clips) into Session slots, \
                 as dragging them onto the Session View, between the windows, or pasting them \
                 there does, and leaves the Arrangement as it was. The clips of one track go \
                 down the scenes from `scene` in the order they play, one clip a slot, and the \
                 clips of other tracks go across the tracks from `track`, so playing the scenes \
                 in turn plays them back in order; `track` and `scene` left out, the selected \
                 slot's. Scenes are added at the end when the clips run past the last. A clip \
                 landing on a full slot replaces its clip (destructive, so an agent asks \
                 first); a clip that would land on a track of the other kind, a group, a return \
                 or Main isn't placed, and the result counts them. The copies' slots are \
                 selected afterwards. One undo step, `Copy Clips to Session`.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Session])
            .args::<CopyToSessionArgs>()
            .result::<CopiedClips>(),
        CommandSpec::new(CUT, "Cut in the Session View")
            .description(
                "Copies the Session View's selection to the clipboard, as `session.copy` does, \
                 then deletes it, as `session.delete` does: the clips in the selected slots, or \
                 the selected scenes with their clips. Cmd-X (Ctrl-X) runs this. Because it \
                 removes the person's work an agent asks first. One undo step.",
            )
            .permission(Permission::Destructive)
            .context(Session)
            .binding("S-EDIT-001")
            .binding(Binding::new("S-SES-029").arg("all_tracks", true))
            .args::<ClipboardArgs>()
            .result::<ChangeSet>(),
        CommandSpec::new(COPY, "Copy in the Session View")
            .description(
                "Copies the clips in the selected slots, or the selected scenes with their \
                 clips, to the clipboard, for `session.paste` here or `arrangement.paste` in the \
                 Arrangement. With `all_tracks`, the clips of every track in the selected slots' \
                 scenes. The clipboard isn't part of the set, and copying changes nothing in it. \
                 Cmd-C (Ctrl-C) runs this.",
            )
            .permission(Permission::Read)
            .context(Session)
            .binding("S-EDIT-002")
            .binding(Binding::new("S-SES-030").arg("all_tracks", true))
            .args::<ClipboardArgs>()
            .result::<SessionClipboardState>(),
        CommandSpec::new(PASTE, "Paste in the Session View")
            .description(
                "Pastes the clipboard into the Session View. Slot clips go from the selected \
                 slot, keeping their places across the tracks and down the scenes; Arrangement \
                 clips copied with `arrangement.copy` go down the scenes in the order they play, \
                 one clip a slot, the clips of several tracks across the tracks, as \
                 `session.copy_to_session` places them. Scenes on the clipboard are inserted as \
                 new scenes after the selected scene. A clip landing on a full slot replaces its \
                 clip (destructive, so an agent asks first), and scenes are added at the end \
                 when the clips run past the last. With `all_tracks`, the first copied track \
                 goes to the first track. Cmd-V (Ctrl-V) runs this. One undo step.",
            )
            .permission(Permission::Edit)
            .context(Session)
            .binding("S-EDIT-003")
            .binding(Binding::new("S-SES-031").arg("all_tracks", true))
            .args::<ClipboardArgs>()
            .result::<CopiedClips>(),
        CommandSpec::new(DUPLICATE, "Duplicate in the Session View")
            .description(
                "Puts a copy of the clips in the selected slots in the slots just below the \
                 selection, keeping their places, and selects the copies, so running it again \
                 repeats them. With `all_tracks`, the clips of every track in the selected \
                 slots' scenes. A copy landing on a full slot replaces its clip (destructive, so \
                 an agent asks first), and scenes are added at the end when the copies run past \
                 the last. Cmd-D (Ctrl-D) runs this; a selected scene is duplicated by \
                 `session.duplicate_scene`. One undo step.",
            )
            .permission(Permission::Edit)
            .context(Session)
            .binding("S-EDIT-004")
            .binding(Binding::new("S-SES-032").arg("all_tracks", true))
            .args::<ClipboardArgs>()
            .result::<CopiedClips>(),
        view(
            FOCUS,
            "Move Focus to the Session View",
            "Shows the Session View if the Arrangement View is in front, and moves the \
             keyboard focus to it, as Navigate > Session View (Option-1, Alt-1) does, so its \
             shortcuts apply. With the second window open, moves the focus to the window that \
             shows the Session View and brings it to the front, without changing either \
             window's view. Changes only the view.",
        )
        .context(Global)
        .binding("S-FOCUS-002")
        .args::<NoArgs>(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_id_is_registered_and_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        for id in IDS {
            let spec = registry.get(id).unwrap_or_else(|| panic!("{id}"));
            assert!(spec.permission.is_some(), "{id}");
            assert!(spec.args.is_some() && spec.result.is_some(), "{id}");
            assert!(!spec.description.is_empty(), "{id}");
        }
        assert!(registry.problems().is_empty(), "{:?}", registry.problems());
    }

    #[test]
    fn the_selection_commands_are_read_class() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        for id in [STATE, SELECT, SELECT_ALL, SCROLL, FOCUS] {
            assert_eq!(
                registry.get(id).unwrap().permission,
                Some(Permission::Read),
                "{id}"
            );
        }
    }
}

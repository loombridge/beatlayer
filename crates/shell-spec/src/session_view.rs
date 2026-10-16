//! The Session View grid's view and editing commands (WP-3.7; decisions D16
//! and D17): selecting slots, scenes, and track title bars, scrolling the
//! scenes, inserting and duplicating scenes, making an empty MIDI clip in a
//! slot, moving and copying a clip between slots, deactivating the selected
//! clips, and dropping files into a slot.
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
/// `session.insert_empty_midi_clip`: Insert Empty MIDI Clip (Cmd+Shift+M).
pub const INSERT_EMPTY_MIDI_CLIP: &str = "session.insert_empty_midi_clip";
/// `session.copy_clip_to_slot`: copy a clip into another slot.
pub const COPY_CLIP_TO_SLOT: &str = "session.copy_clip_to_slot";
/// `session.delete`: delete the selected slot clips or scenes.
pub const DELETE: &str = "session.delete";
/// `session.deactivate`: deactivate (or reactivate) the selected clips (0).
pub const DEACTIVATE: &str = "session.deactivate";
/// `session.drop_files`: drop audio files into a slot.
pub const DROP_FILES: &str = "session.drop_files";
/// `session.focus`: move the keyboard focus to the Session View.
pub const FOCUS: &str = "session.focus";

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
    INSERT_EMPTY_MIDI_CLIP,
    COPY_CLIP_TO_SLOT,
    DELETE,
    DEACTIVATE,
    DROP_FILES,
    FOCUS,
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
    /// The track whose slot they go in.
    pub track: TrackId,
    /// The scene whose slot the first one goes in.
    pub scene: SceneId,
    /// With the command modifier (Cmd, Ctrl on Windows and Linux) held,
    /// spread them across the tracks instead of down the scenes.
    #[serde(default)]
    pub spread: bool,
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
        CommandSpec::new(DROP_FILES, "Drop Files into a Slot")
            .description(
                "Puts audio files dropped from the browser, Finder, or Explorer into the Session \
             slot at `track` and `scene`, as dropping them on a slot does: one after another \
             down that track's scenes, or, with `spread` (Cmd, Ctrl on Windows and Linux, held \
             while dropping), across the tracks at `scene`. Each target slot must be empty. \
             Each file becomes a Session clip of its length, starting at the clip's beginning. \
             One undo step. Reads the files, outside the project, and registers them so the set \
             keeps them.",
            )
            .permission(Permission::External)
            .context(Session)
            .args::<DropFilesArgs>()
            .result::<ChangeSet>(),
        view(
            FOCUS,
            "Move Focus to the Session View",
            "Shows the Session View if the Arrangement View is in front, and moves the \
             keyboard focus to it, as Navigate > Session View (Option-1, Alt-1) does, so its \
             shortcuts apply. Changes only the view.",
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

//! The Arrangement's editing commands in the command registry (WP-1.8b;
//! decisions D16 and D17): selecting clips, moving and copying them,
//! resizing, splitting, deleting, duplicating, and the clipboard.
//!
//! Each acts on clips, or on a span of time on tracks, given explicitly in
//! its arguments, or, when they're left out, on what's selected in the
//! Arrangement, as a person's key press or menu choice does. Every edit is
//! one undo step, and a drag that edits is one step however long it lasts:
//! the mouse runs these same commands inside one transaction. Positions are
//! ticks or bars, beats, and sixteenths ([`Position`]).
//!
//! [`register`] adds their specs, which are pure data, so `cargo xtask` can
//! check them and list them in the command reference; `bl-shell` runs
//! them by ID. The behaviour they follow is in the arrangement editing spec.

use std::borrow::Cow;

use bl_commands::Context::{Arrangement, LoopBrace, TakeLanes};
use bl_commands::{Binding, CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_core::time::{Ticks, TimeSignatureMap};
use bl_model::{ClipId, TrackId};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

use crate::arrangement::{ArrangementState, Lane, TimeRange};

/// `arrangement.select_clips`: select clips, as clicking their title bars
/// does.
pub const SELECT_CLIPS: &str = "arrangement.select_clips";
/// `arrangement.select_all`: select every clip (Cmd-A).
pub const SELECT_ALL: &str = "arrangement.select_all";
/// `arrangement.move_clips`: move or copy clips in time and between tracks.
pub const MOVE_CLIPS: &str = "arrangement.move_clips";
/// `arrangement.resize_clips`: move clips' left or right edges.
pub const RESIZE_CLIPS: &str = "arrangement.resize_clips";
/// `arrangement.split`: split clips at positions (Cmd-E).
pub const SPLIT: &str = "arrangement.split";
/// `arrangement.delete`: delete clips, or the material in a span of time
/// (Delete).
pub const DELETE: &str = "arrangement.delete";
/// `arrangement.duplicate`: put a copy right after the selection (Cmd-D).
pub const DUPLICATE: &str = "arrangement.duplicate";
/// `arrangement.copy`: copy to the clipboard (Cmd-C).
pub const COPY: &str = "arrangement.copy";
/// `arrangement.cut`: copy to the clipboard, then delete (Cmd-X).
pub const CUT: &str = "arrangement.cut";
/// `arrangement.paste`: place the clipboard's material (Cmd-V).
pub const PASTE: &str = "arrangement.paste";
/// `arrangement.create_fade`: make fades and crossfades from the selection
/// (Cmd-Option-F).
pub const CREATE_FADE: &str = "arrangement.create_fade";
/// `arrangement.delete_fades`: delete the fades and crossfades of clips
/// (Cmd-Option-Delete, and Delete with a fade handle selected).
pub const DELETE_FADES: &str = "arrangement.delete_fades";
/// `arrangement.insert_silence`: open empty time on every track (Cmd-I).
pub const INSERT_SILENCE: &str = "arrangement.insert_silence";
/// `arrangement.cut_time`: copy a span of every track, then delete it
/// (Cmd-Shift-X).
pub const CUT_TIME: &str = "arrangement.cut_time";
/// `arrangement.copy_time`: copy a span of every track (Cmd-Shift-C).
pub const COPY_TIME: &str = "arrangement.copy_time";
/// `arrangement.paste_time`: open time on every track and paste into it
/// (Cmd-Shift-V).
pub const PASTE_TIME: &str = "arrangement.paste_time";
/// `arrangement.duplicate_time`: put a copy of a span of every track right
/// after it (Cmd-Shift-D).
pub const DUPLICATE_TIME: &str = "arrangement.duplicate_time";
/// `arrangement.delete_time`: close a span on every track (Cmd-Shift-Delete).
pub const DELETE_TIME: &str = "arrangement.delete_time";
/// `arrangement.nudge`: move the selection by one grid step (Left, Right).
pub const NUDGE: &str = "arrangement.nudge";
/// `arrangement.loop_selection`: set the loop brace to the selection and
/// switch the loop on (Cmd-L).
pub const LOOP_SELECTION: &str = "arrangement.loop_selection";
/// `arrangement.select_loop`: select the time inside the loop brace
/// (Cmd-Shift-L).
pub const SELECT_LOOP: &str = "arrangement.select_loop";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    SELECT_CLIPS,
    SELECT_ALL,
    MOVE_CLIPS,
    RESIZE_CLIPS,
    SPLIT,
    DELETE,
    DUPLICATE,
    COPY,
    CUT,
    PASTE,
    NUDGE,
    LOOP_SELECTION,
    SELECT_LOOP,
    CREATE_FADE,
    DELETE_FADES,
    INSERT_SILENCE,
    CUT_TIME,
    COPY_TIME,
    PASTE_TIME,
    DUPLICATE_TIME,
    DELETE_TIME,
];

/// Whether `id` is one of the Arrangement's editing commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// A position in the song: ticks, or bars, beats, and sixteenths as text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Position {
    /// Ticks from 1.1.1.
    Ticks(Ticks),
    /// Bars, beats, and sixteenths, such as `5.1.1`, under the song's time
    /// signatures.
    Bars(String),
}

impl Position {
    /// The position in ticks, reading text under `signatures`.
    ///
    /// # Errors
    ///
    /// A message if the text isn't a position in the song.
    pub fn resolve(&self, signatures: &TimeSignatureMap) -> Result<Ticks, String> {
        match self {
            Self::Ticks(ticks) => Ok(*ticks),
            Self::Bars(text) => signatures
                .parse_position(text)
                .map_err(|error| format!("`{text}` isn't a position: {error}")),
        }
    }
}

impl From<Ticks> for Position {
    fn from(ticks: Ticks) -> Self {
        Self::Ticks(ticks)
    }
}

impl Default for Position {
    /// 1.1.1.
    fn default() -> Self {
        Self::Ticks(Ticks::ZERO)
    }
}

impl JsonSchema for Position {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("Position")
    }

    fn schema_id() -> Cow<'static, str> {
        Cow::Borrowed("bl_shell_spec::editing::Position")
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let ticks = generator.subschema_for::<Ticks>();
        json_schema!({
            "description": "A position in the song: ticks (705,600 to the quarter note; 0 is \
                            1.1.1, the start of the arrangement), or bars, beats, and sixteenths \
                            as text under the song's time signatures, such as \"5\", \"5.3\", or \
                            \"5.3.2\" (missing fields are 1). There's no bar 0: the bar before 1 \
                            is -1.",
            "anyOf": [
                ticks,
                {
                    "type": "string",
                    "pattern": "^\\s*-?[0-9]+(\\.[0-9]+){0,2}\\s*$"
                }
            ]
        })
    }
}

/// How `arrangement.select_clips` changes the selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "How to change the clip selection: `replace` it (a click), `add` to it, \
                   `toggle` each clip in or out (Cmd-click, Ctrl-click on Windows), or `extend` \
                   it to every clip between the first selected clip and these (Shift-click)."
)]
pub enum SelectMode {
    /// Select only these clips.
    #[default]
    Replace,
    /// Add these clips.
    Add,
    /// Select each clip that isn't selected and deselect each that is.
    Toggle,
    /// Add these clips and every clip between them and the first selected
    /// clip, in time and in the track list.
    Extend,
}

/// Arguments of `arrangement.select_clips`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectClipsArgs {
    /// The clips. Empty with `replace` clears the clip selection.
    pub clips: Vec<ClipId>,
    /// How to change the selection; `replace` if left out.
    #[serde(default)]
    pub mode: SelectMode,
    /// Whether it reaches the tracks linked with the ones it names (the
    /// linked-track editing spec, R15). `true` if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked: Option<bool>,
}

/// What an edit acts on: clips, or a span of time on tracks. Left out, the
/// Arrangement's selection: its selected clips if there are any, otherwise
/// its time selection on the selected tracks.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetArgs {
    /// Whole clips to act on. Give either `clips` or a span of time, not
    /// both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipId>>,
    /// The tracks a span of time covers. Left out, the selected tracks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<TrackId>>,
    /// The lanes a span of time covers, each a track's own lane or one of
    /// its take lanes (the comping spec, R27). Left out, the lanes of the
    /// time selection, or `tracks`' own lanes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lanes: Option<Vec<Lane>>,
    /// Where the span starts. Left out, the time selection's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// Where the span ends, after `start`. Left out, the time selection's
    /// end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Position>,
    /// Whether it reaches the tracks linked with the ones it names (the
    /// linked-track editing spec, R15). `true` if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked: Option<bool>,
}

/// A span of time on every track, for the …Time commands (time editing R14).
/// Left out, the Arrangement's time selection (the span of the selected
/// clips when clips are selected).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimeSpanArgs {
    /// Where the span starts. Left out, the time selection's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// Where the span ends, after `start`. Left out, the time selection's
    /// end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Position>,
}

/// Arguments of `arrangement.paste_time`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PasteTimeArgs {
    /// Where the time opens. Left out, the time selection's start, or the
    /// insert marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Position>,
}

/// What `arrangement.copy_time` put on the clipboard.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TimeClipboardState {
    /// How many clips it holds, on the main lanes and the take lanes.
    pub clips: usize,
    /// How many tracks it holds clips or envelopes of.
    pub tracks: usize,
    /// How many envelopes it holds, over every track.
    pub envelopes: usize,
    /// How many tempo breakpoints it holds.
    pub tempo_breakpoints: usize,
    /// How many time-signature markers it holds.
    pub time_signatures: usize,
    /// How many locators it holds.
    pub locators: usize,
    /// How long the span is.
    pub length: Ticks,
}

/// Arguments of `arrangement.move_clips`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveClipsArgs {
    /// The clips. Left out, the selected clips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipId>>,
    /// How far to move them in time, in ticks; negative is earlier. Give
    /// `by` or `to`, or neither to move them only between tracks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Ticks>,
    /// Where `anchor` starts after the move; the other clips keep their
    /// distance from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Position>,
    /// The clip that `to` and `track` place. Left out, the earliest clip
    /// (the topmost of those that start together).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<ClipId>,
    /// The track `anchor` moves to; the other clips keep their distance from
    /// it in the track list. Left out, every clip stays on its track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    /// The lane `anchor` moves to, a track's own lane or one of its take
    /// lanes (the comping spec, R27); the other clips keep their distance
    /// from it among the track's lanes. Give `track` or `lane`, not both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<Lane>,
    /// Copy the clips instead, leaving the originals where they are.
    #[serde(default)]
    pub copy: bool,
    /// Whether it reaches the tracks linked with the ones it names (the
    /// linked-track editing spec, R15). `true` if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked: Option<bool>,
}

/// Which edge of a clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "A clip's `start` (left) edge, which shows more or less of its beginning, or \
                   its `end` (right) edge."
)]
pub enum Edge {
    /// The left edge.
    Start,
    /// The right edge.
    End,
}

/// Arguments of `arrangement.resize_clips`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResizeClipsArgs {
    /// The clips. Left out, the selected clips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipId>>,
    /// Which edge moves.
    pub edge: Edge,
    /// How far each clip's edge moves, in ticks; negative is earlier. Give
    /// `by` or `to`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Ticks>,
    /// Where `anchor`'s edge goes; the other clips' edges move as far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Position>,
    /// The clip whose edge `to` places. Left out, the earliest clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<ClipId>,
    /// Whether it reaches the tracks linked with the ones it names (the
    /// linked-track editing spec, R15). `true` if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked: Option<bool>,
}

/// Arguments of `arrangement.split`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SplitArgs {
    /// The clips to split. Left out, the clips on `tracks`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipId>>,
    /// The tracks whose clips are split. Left out, the selected clips, or
    /// the clips on the selected tracks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<TrackId>>,
    /// The lanes whose clips are split, each a track's own lane or one of its
    /// take lanes (the comping spec, R27). Left out, `tracks`' own lanes, or
    /// the lanes the time selection covers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lanes: Option<Vec<Lane>>,
    /// Where to split. Left out, the time selection's start and end, or the
    /// insert marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Vec<Position>>,
    /// Whether it reaches the tracks linked with the ones it names (the
    /// linked-track editing spec, R15). `true` if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked: Option<bool>,
}

/// Arguments of `arrangement.paste`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PasteArgs {
    /// Where the material starts. Left out, the time selection's start, or
    /// the insert marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Position>,
    /// The track the clipboard's first track goes to; its other tracks go
    /// to the tracks after it, keeping their spacing. Left out, the first
    /// selected track, or the track the material was copied from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    /// The lane the clipboard's first lane goes to, a track's own lane or
    /// one of its take lanes (the comping spec, R27); its other lanes keep
    /// their distance from it, among the tracks and among a track's lanes.
    /// Left out, the first lane selected, or the first selected track's main
    /// lane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<Lane>,
}

/// Arguments of `arrangement.delete_fades`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteFadesArgs {
    /// The audio clips. Left out, the selected clips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipId>>,
    /// Only the fade at this edge of each clip, as Delete does with one fade
    /// handle selected. Left out, both edges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge: Option<Edge>,
}

/// Which way `arrangement.nudge` moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(description = "Earlier (`left`) or later (`right`).")]
pub enum Direction {
    /// Earlier.
    Left,
    /// Later.
    Right,
}

/// Arguments of `arrangement.nudge`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NudgeArgs {
    /// Which way.
    pub direction: Direction,
    /// The clips. Left out, the selected clips; with none selected, the
    /// insert marker moves instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipId>>,
}

/// Arguments of `arrangement.loop_selection`. Give both or neither.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoopSelectionArgs {
    /// Where the loop brace starts. Left out with `end`, the selection's
    /// start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// Where it ends, after `start`. Left out with `start`, the selection's
    /// end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Position>,
}

/// What an edit did, and what's selected afterwards.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EditResult {
    /// Clips the edit created: copies, pasted clips, and parts split off.
    pub created: Vec<ClipId>,
    /// Clips it changed: moved, resized, or shortened where something was
    /// placed over them.
    pub changed: Vec<ClipId>,
    /// Clips it deleted, including any that something was placed over
    /// entirely.
    pub deleted: Vec<ClipId>,
    /// The clips selected afterwards.
    pub selected_clips: Vec<ClipId>,
    /// The time selected afterwards, if any.
    pub time_selection: Option<TimeRange>,
    /// The tracks selected afterwards.
    pub selected_tracks: Vec<TrackId>,
    /// Whether the edit was recorded as an undo step: false when it changed
    /// nothing.
    pub undo_step: bool,
}

/// What `arrangement.copy` put on the clipboard.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClipboardState {
    /// How many clips it holds.
    pub clips: usize,
    /// How many tracks they came from, in order.
    pub tracks: usize,
    /// How long the material is, from its start to its end.
    pub length: Ticks,
}

/// Adds the Arrangement's editing commands to a registry.
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

fn edit(
    id: &'static str,
    label: &'static str,
    permission: Permission,
    description: &'static str,
) -> CommandSpec {
    CommandSpec::new(id, label)
        .description(description)
        .permission(permission)
        .context(Arrangement)
        .result::<EditResult>()
}

#[expect(
    clippy::too_many_lines,
    reason = "one spec per command, written out in full"
)]
fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(SELECT_CLIPS, "Select Clips")
            .description(
                "Selects clips in the Arrangement, as clicking a clip's title bar does: \
                 `replace` selects only these, `toggle` adds or removes each (Cmd-click, \
                 Ctrl-click on Windows), `add` adds them, and `extend` adds them and every clip \
                 between them and the first selected clip (Shift-click). Their tracks become \
                 the selected tracks and the time they span the time selection, which Z zooms \
                 to. Selecting is never an undo step and changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(Arrangement)
            .binding(Binding::new("S-EDIT-010").arg("mode", "toggle"))
            .binding(Binding::new("S-ZOOM-010").arg("mode", "toggle"))
            .binding(Binding::new("S-EDIT-011").arg("mode", "extend"))
            .binding(Binding::new("S-ZOOM-009").arg("mode", "extend"))
            .binding(Binding::new("S-ZOOM-008").arg("mode", "add"))
            .args::<SelectClipsArgs>()
            .result::<ArrangementState>(),
        CommandSpec::new(SELECT_ALL, "Select All")
            .description(
                "Selects every clip in the Arrangement, and the time from 1.1.1 to the end of \
                 the last clip on every track that holds clips, as Cmd-A (Ctrl-A) does. \
                 Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(Arrangement)
            .binding("S-EDIT-009")
            .args::<NoArgs>()
            .result::<ArrangementState>(),
        edit(
            MOVE_CLIPS,
            "Move Clips",
            Permission::Edit,
            "Moves clips (`clips`, or the selected clips) in time by `by` ticks, or so that \
             `anchor` starts at `to`, and to other tracks: `anchor` goes to `track` and each \
             other clip keeps its distance from it in the track list. With `copy`, copies go \
             there and the originals stay. A clip placed over another clip on its new track \
             replaces the part it covers: a clip covered entirely is deleted, one covered at an \
             end is shortened, and one covered in the middle is split around it. Audio clips go \
             only to audio tracks and MIDI clips to MIDI tracks, and no clip can start before \
             1.1.1; if any would, nothing moves. The moved clips (or the copies) are selected \
             afterwards. Dragging a clip's title bar runs this, with Option (Ctrl on Windows) \
             held to copy; the whole drag is one undo step. A move or copy that would replace \
             part of another clip is destructive, so an agent asks first.",
        )
        .args::<MoveClipsArgs>(),
        edit(
            RESIZE_CLIPS,
            "Resize Clips",
            Permission::Edit,
            "Moves the `start` or `end` edge of clips (`clips`, or the selected clips) by `by` \
             ticks, or so that `anchor`'s edge lands at `to`, every clip's edge moving as far. \
             Moving the start edge keeps the clip's contents where they play, so the clip shows \
             more or less of them; it can't go before where the contents begin or before \
             1.1.1, and a clip can't become shorter than a tick. Lengthening a clip over its \
             neighbour replaces the part it covers. Dragging a clip's edge runs this, and so \
             does Enter, then Left or Right, then Enter, from the keyboard; each is one undo \
             step. Lengthening a clip over another is destructive, so an agent asks first.",
        )
        .binding("S-ARR-004")
        .args::<ResizeClipsArgs>(),
        edit(
            SPLIT,
            "Split",
            Permission::Edit,
            "Splits clips at positions: `clips`, or every clip on `tracks`, at each of `at` \
             that falls inside them. Left out, it splits at the time selection's start and \
             end, or at the insert marker, the selected clips or the clips on the selected \
             tracks, as Cmd-E (Ctrl-E) does. Each part plays exactly what it played before, so \
             the song sounds the same; the parts can then be moved, deleted, or copied on their \
             own. The parts split off are listed in `created`.",
        )
        .binding("S-ARR-001")
        .args::<SplitArgs>(),
        edit(
            DELETE,
            "Delete",
            Permission::Destructive,
            "Deletes clips (`clips`, or the selected clips), or the material in a span of time \
             on tracks (`tracks`, `start`, `end`, or the time selection on the selected \
             tracks): clips inside the span are deleted and clips that cross its edges are \
             shortened or split, leaving the time empty. Nothing after the span moves. Delete \
             runs this on the Arrangement's selection. Undo restores what it deleted, but \
             because it removes the person's work an agent asks first.",
        )
        .binding("S-EDIT-005")
        .args::<TargetArgs>(),
        edit(
            DUPLICATE,
            "Duplicate",
            Permission::Edit,
            "Places a copy of clips (`clips`, or the selected clips) right after the span they \
             cover, on the same tracks, or a copy of the material in a span of time on tracks \
             right after the span, and selects the copy, so running it again repeats the \
             material. The copy replaces whatever it covers; when that's part of another clip, \
             it's destructive, so an agent asks first. Cmd-D (Ctrl-D) runs this on the \
             Arrangement's selection.",
        )
        .binding("S-EDIT-004")
        // Cmd-D also reaches it with a time selection or clips on a take
        // lane, where S-COMP-005 is the take-lane context's own Cmd-D.
        .context(TakeLanes)
        .args::<TargetArgs>(),
        CommandSpec::new(COPY, "Copy")
            .description(
                "Copies clips (`clips`, or the selected clips), or the material in a span of \
                 time on tracks, to the Arrangement's clipboard with the tracks they're on, for \
                 `arrangement.paste`. Cmd-C (Ctrl-C) runs this on the selection. The clipboard \
                 isn't part of the set, and copying changes nothing in it.",
            )
            .permission(Permission::Read)
            .context(Arrangement)
            .binding("S-EDIT-002")
            .args::<TargetArgs>()
            .result::<ClipboardState>(),
        edit(
            CUT,
            "Cut",
            Permission::Destructive,
            "Copies clips or the material in a span of time to the clipboard, as \
             `arrangement.copy` does, then deletes it, as `arrangement.delete` does. Cmd-X \
             (Ctrl-X) runs this on the selection. Because it removes the person's work, an \
             agent asks first.",
        )
        .binding("S-EDIT-001")
        .args::<TargetArgs>(),
        edit(
            PASTE,
            "Paste",
            Permission::Edit,
            "Places the clipboard's material starting at `at` (left out, the time selection's \
             start or the insert marker), its first track's clips on `track` (left out, the \
             first selected track) and its other tracks' clips on the tracks after it, keeping \
             their spacing. The material replaces what it covers (destructive, so an agent \
             asks first, when that's part of another clip) and is selected afterwards. \
             It fails, changing nothing, if a clip would land on a track of another kind or \
             there's nothing on the clipboard. Cmd-V (Ctrl-V) runs this.",
        )
        .binding("S-EDIT-003")
        .args::<PasteArgs>(),
        edit(
            INSERT_SILENCE,
            "Insert Silence",
            Permission::Edit,
            "Opens as much empty time as the time selection (or `start` to `end`) is long, at \
             its start, on every track, whichever tracks are selected: a clip across that point \
             is split there, and every clip (main lane and take lanes), automation breakpoint, \
             tempo breakpoint, time-signature marker and locator at or after it moves later. \
             Each envelope, the tempo and the signature hold across the new time the value they \
             had at the point. The Session View's slots and the loop brace are left as they \
             are. The new time becomes the time selection. One undo step, `Insert Silence`. \
             Cmd-I (Ctrl-I) runs this with the Arrangement focused. To give the position and \
             length in ticks, use `song.insert_time`.",
        )
        .binding("S-ARR-014")
        .args::<TimeSpanArgs>(),
        edit(
            CUT_TIME,
            "Cut Time",
            Permission::Destructive,
            "Copies the time selection (or `start` to `end`) from every track to the clipboard, \
             as `arrangement.copy_time` does, then deletes it, as `arrangement.delete_time` \
             does. One undo step, `Cut Time`. Cmd-Shift-X (Ctrl-Shift-X) runs this, and so does \
             Shift with Cut. Because it removes the person's work, an agent asks first.",
        )
        .binding("S-ARR-015")
        .args::<TimeSpanArgs>(),
        CommandSpec::new(COPY_TIME, "Copy Time")
            .description(
                "Copies the time selection (or `start` to `end`) from every track, whichever \
                 tracks are selected, to the clipboard in place of what it held: each track's \
                 clips and parts of clips on its main lane and take lanes, each track's \
                 envelopes over the span, the tempo over it, the time-signature markers and \
                 locators inside it, and its length. Cmd-Shift-C (Ctrl-Shift-C) runs this, and \
                 so does Shift with Copy. The clipboard isn't part of the set, and copying \
                 changes nothing in it. The result says what the clipboard holds.",
            )
            .permission(Permission::Read)
            .context(Arrangement)
            .binding("S-ARR-016")
            .args::<TimeSpanArgs>()
            .result::<TimeClipboardState>(),
        edit(
            PASTE_TIME,
            "Paste Time",
            Permission::Edit,
            "Opens as much time as the clipboard holds at `at` (left out, the time selection's \
             start or the insert marker) on every track, as `arrangement.insert_silence` does, \
             and puts the clipboard's material in it. Material copied with \
             `arrangement.copy_time` or `arrangement.cut_time` goes back on the tracks it came \
             from (a track deleted since gets nothing), with its envelopes, the tempo, the \
             time-signature markers and the locators; clips copied with `arrangement.copy` or \
             `arrangement.cut` go where `arrangement.paste` puts them. The pasted time becomes \
             the time selection. Refused, changing nothing, with nothing on the clipboard. One \
             undo step, `Paste Time`. Cmd-Shift-V (Ctrl-Shift-V) runs this, and so does Shift \
             with Paste.",
        )
        .binding("S-ARR-017")
        .args::<PasteTimeArgs>(),
        edit(
            DUPLICATE_TIME,
            "Duplicate Time",
            Permission::Edit,
            "Puts a copy of the time selection (or `start` to `end`) from every track straight \
             after it, as `arrangement.copy_time` and `arrangement.paste_time` at its end \
             would, leaving the clipboard as it was. The copy becomes the time selection, so \
             running it again lays down another. One undo step, `Duplicate Time`. \
             Cmd-Shift-D (Ctrl-Shift-D) runs this, and so does Shift with Duplicate.",
        )
        .binding("S-ARR-018")
        .args::<TimeSpanArgs>(),
        edit(
            DELETE_TIME,
            "Delete Time",
            Permission::Destructive,
            "Removes the time selection (or `start` to `end`) from every track, whichever \
             tracks are selected, so what follows closes up: clips inside it are deleted, a \
             clip across an edge is cut or split there, and everything after it moves earlier. \
             An envelope keeps its shape up to the span's start and from there takes the shape \
             it had from the span's end, stepping between the two values over one tick where \
             they differ; the tempo steps at the span's start. Time-signature markers and \
             locators inside the span are deleted. The insert marker goes to the span's start \
             with no time selected. One undo step, `Delete Time`. Cmd-Shift-Delete \
             (Ctrl-Shift-Delete) runs this, and so does Shift with Delete. Undo restores \
             it, but because it removes the person's work an agent asks first.",
        )
        .binding("S-ARR-019")
        .args::<TimeSpanArgs>(),
        edit(
            NUDGE,
            "Nudge",
            Permission::Edit,
            "Moves clips (`clips`, or the selected clips) one step of the editing grid earlier \
             (`left`) or later (`right`), as Left and Right do in the Arrangement; with no \
             clips selected it moves the insert marker one grid step instead, which changes \
             nothing in the set. A clip can't move before 1.1.1. A clip nudged over part of \
             another replaces it, which is destructive, so an agent asks first.",
        )
        .args::<NudgeArgs>(),
        edit(
            LOOP_SELECTION,
            "Loop Selection",
            Permission::Edit,
            "Sets the Arrangement's loop brace to span from `start` to `end`, or the time \
             selection (or the selected clips) when they're left out, and switches the loop on, \
             as one undo step, as Cmd-L (Ctrl-L) does. With nothing selected and no span given, \
             it switches the loop on or off instead. Playback that reaches the brace's end then \
             goes on from its start.",
        )
        .binding("S-ARR-011")
        .args::<LoopSelectionArgs>(),
        edit(
            CREATE_FADE,
            "Create Fade",
            Permission::Edit,
            "Makes fades and crossfades from the Arrangement's selection, as Create Fade \
             (Cmd-Option-F) does, on the selected audio clips (`clips`) or the time selection \
             on its audio tracks (`tracks`, `start`, `end`). With a span of time: a clip whose \
             start the span holds, and not its end, gets a fade-in from its start to the span's \
             end; a clip whose end it holds, and not its start, gets a fade-out from the span's \
             start to its end; two audio clips that meet inside the span get a crossfade over \
             it; a clip the span holds whole, or holds neither edge of, is left as it is. \
             With clips, each boundary where two of them meet gets a 4 ms crossfade, 2 ms each \
             side, whatever the Create Fades on Clip Edges setting. Each fade is kept within \
             its clip's limits, so a span longer than a clip allows makes the longest fade \
             there is. One undo step, named Create Fade or Create Crossfade. Refused where the \
             selection gives no fade. Read the result back from the clips' `data` with the \
             query tool: `fade_in`, `fade_out`, their curves, `fade_in_before` and \
             `fade_out_after`.",
        )
        .binding("S-ARR-008")
        .args::<TargetArgs>(),
        edit(
            DELETE_FADES,
            "Delete Fades",
            Permission::Edit,
            "Deletes the fades and crossfades of audio clips (`clips`, or the selected clips), \
             or only those at one `edge` of each, as Delete does with a fade handle selected. \
             With Create Fades on Clip Edges on, a fade goes back to 4 ms and a crossfade to \
             the 4 ms crossfade of two clips that meet; with it off, fades go, and a crossfade \
             with them, leaving the clips meeting with no fade. One undo step, named Delete \
             Fade or Delete Fades. Cmd-Option-Delete (Ctrl-Alt-Backspace) runs this.",
        )
        .binding("S-ARR-009")
        .args::<DeleteFadesArgs>(),
        CommandSpec::new(SELECT_LOOP, "Select Loop")
            .description(
                "Selects the time inside the Arrangement's loop brace on every track, as \
                 Cmd-Shift-L (Ctrl-Shift-L) and clicking the brace do, so the editing commands \
                 act on the looped material. Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .contexts([Arrangement, LoopBrace])
            .binding("S-ARR-013")
            .binding("S-LOOP-011")
            .args::<NoArgs>()
            .result::<ArrangementState>(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_id_is_registered_and_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        for id in IDS {
            assert!(registry.contains(id), "{id}");
        }
        let problems: Vec<String> = registry
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(problems.is_empty(), "{problems:#?}");
        // Deleting work is destructive; selecting and copying change nothing.
        let class = |id: &str| registry.get(id).unwrap().permission;
        assert_eq!(class(DELETE), Some(Permission::Destructive));
        assert_eq!(class(CUT), Some(Permission::Destructive));
        assert_eq!(class(SELECT_CLIPS), Some(Permission::Read));
        assert_eq!(class(COPY), Some(Permission::Read));
        // The …Time commands: removing time asks first, copying changes
        // nothing.
        assert_eq!(class(DELETE_TIME), Some(Permission::Destructive));
        assert_eq!(class(CUT_TIME), Some(Permission::Destructive));
        assert_eq!(class(COPY_TIME), Some(Permission::Read));
        assert_eq!(class(INSERT_SILENCE), Some(Permission::Edit));
        assert_eq!(class(MOVE_CLIPS), Some(Permission::Edit));
    }

    #[test]
    fn positions_read_as_ticks_or_bars() {
        let signatures = TimeSignatureMap::default();
        let read = |value: serde_json::Value| {
            serde_json::from_value::<Position>(value)
                .unwrap()
                .resolve(&signatures)
        };
        let bar = Ticks::WHOLE.get();
        assert_eq!(read(serde_json::json!(bar)), Ok(Ticks::WHOLE));
        assert_eq!(read(serde_json::json!("2.1.1")), Ok(Ticks::WHOLE));
        assert_eq!(read(serde_json::json!("3")), Ok(Ticks::WHOLE * 2));
        assert_eq!(read(serde_json::json!("-1.3")), Ok(-Ticks::QUARTER * 2));
        assert!(read(serde_json::json!("bar five")).is_err());
    }
}

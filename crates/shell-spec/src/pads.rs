//! A drum rack in the Device View (WP-5.6; the drum racks spec): its pad
//! view, the pads' mute, solo, preview and hot-swap, dropping on pads,
//! swapping pads, Save as Default Pad, previewing chains, extracting chains,
//! and Group to Drum Rack. A drum chain's Receive, Play, Choke and sends and
//! a return chain's Audio To are the document's own commands (`chain.set_*`,
//! `bl-model`'s); the commands here are the ones that act on the view or on
//! several chains at once, which run on the shell.

use bl_commands::Context::{DeviceView, Global};
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_model::{ChainId, DeviceId, Receive, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::devices::{DeviceViewState, NewDevice};

/// `rack.move_pads`: move the notes a drum rack's pad view shows.
pub const MOVE_PADS: &str = "rack.move_pads";
/// `pad.select`: select a pad.
pub const PAD_SELECT: &str = "pad.select";
/// `pad.set_mute`: mute a pad's chains.
pub const PAD_SET_MUTE: &str = "pad.set_mute";
/// `pad.set_solo`: solo a pad's chains.
pub const PAD_SET_SOLO: &str = "pad.set_solo";
/// `pad.preview`: hold a pad's preview button.
pub const PAD_PREVIEW: &str = "pad.preview";
/// `chain.preview`: hold a drum chain's preview button.
pub const CHAIN_PREVIEW: &str = "chain.preview";
/// `pad.drop`: drop samples, a device or a preset on a pad.
pub const PAD_DROP: &str = "pad.drop";
/// `pad.swap`: drag a pad onto another.
pub const PAD_SWAP: &str = "pad.swap";
/// `pad.save_default`: Save as Default Pad.
pub const PAD_SAVE_DEFAULT: &str = "pad.save_default";
/// `pad.delete`: delete a pad's chains.
pub const PAD_DELETE: &str = "pad.delete";
/// `pad.rename`: rename a pad's chain.
pub const PAD_RENAME: &str = "pad.rename";
/// `pad.hot_swap`: make a pad the target of hot-swap.
pub const PAD_HOT_SWAP: &str = "pad.hot_swap";
/// `pad.hot_swap_sample`: the pad's Hot-Swap Sample button.
pub const PAD_HOT_SWAP_SAMPLE: &str = "pad.hot_swap_sample";
/// `rack.switch_hot_swap_target`: D, between a drum rack and its last
/// selected pad.
pub const SWITCH_HOT_SWAP_TARGET: &str = "rack.switch_hot_swap_target";
/// `device.group_to_drum_rack`: Group to Drum Rack.
pub const GROUP_TO_DRUM_RACK: &str = "device.group_to_drum_rack";
/// `chain.extract`: pull a drum chain out of its rack onto a new track.
pub const CHAIN_EXTRACT: &str = "chain.extract";
/// `chain.extract_return`: pull a return chain out onto a new return track.
pub const CHAIN_EXTRACT_RETURN: &str = "chain.extract_return";

/// Every command [`specs`] describes.
pub const IDS: &[&str] = &[
    MOVE_PADS,
    PAD_SELECT,
    PAD_SET_MUTE,
    PAD_SET_SOLO,
    PAD_PREVIEW,
    CHAIN_PREVIEW,
    PAD_DROP,
    PAD_SWAP,
    PAD_SAVE_DEFAULT,
    PAD_DELETE,
    PAD_RENAME,
    PAD_HOT_SWAP,
    PAD_HOT_SWAP_SAMPLE,
    SWITCH_HOT_SWAP_TARGET,
    GROUP_TO_DRUM_RACK,
    CHAIN_EXTRACT,
    CHAIN_EXTRACT_RETURN,
];

/// How many pads the pad view shows at a time: four rows of four.
pub const PADS_SHOWN: u8 = 16;
/// How many notes one row of the pad view holds.
pub const PADS_PER_ROW: u8 = 4;
/// A pad's width at 100%, in points.
pub const PAD_WIDTH: f32 = 56.0;
/// A pad's height at 100%, in points.
pub const PAD_HEIGHT: f32 = 34.0;
/// The gap between pads at 100%.
pub const PAD_GAP: f32 = 3.0;
/// The overview's width at 100%: the strip at the pad view's left that shows
/// all 128 notes as eight groups of sixteen.
pub const PAD_OVERVIEW_WIDTH: f32 = 30.0;
/// The width at 100% of a drum rack's pad view: the overview, then the grid.
pub const RACK_PAD_WIDTH: f32 = PAD_OVERVIEW_WIDTH + 4.0 + 4.0 * PAD_WIDTH + 3.0 * PAD_GAP + 4.0;
/// The width at 100% the chain list gains with Input/Output on: the
/// Preview button, Receive, Play and Choke.
pub const DRUM_IO_WIDTH: f32 = 150.0;
/// The width at 100% of one return chain's send column with Send on.
pub const DRUM_SEND_WIDTH: f32 = 34.0;
/// The height at 100% of the heading of a chain list's return section.
pub const RETURN_HEADING_HEIGHT: f32 = 14.0;

/// Whether `id` is one of the commands here.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// A switch that may be on for some of a pad's chains: Multi pads show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PadSwitch {
    /// Off for every chain of the pad.
    Off,
    /// On for every chain of the pad.
    On,
    /// On for some of them.
    Mixed,
}

/// What a pad holds (drum racks spec, R12).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PadKind {
    /// No chain receives its note: it shows only the note's name.
    Empty,
    /// One chain: it shows the chain's name.
    Single,
    /// Several chains on its note: it reads `Multi`.
    Multi,
}

/// One pad of the pad view.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PadState {
    /// Its MIDI note.
    pub note: u8,
    /// Its place among the sixteen pads shown, from 0 for the lowest: what
    /// keeps the keyboard's place while the view moves.
    pub slot: u8,
    /// The note's name in Live's numbering (`C1`).
    pub name: String,
    /// What hovering it shows in the Status Bar: the note and its General
    /// MIDI sound.
    pub hover: String,
    /// What it holds.
    pub kind: PadKind,
    /// What it shows: the chain's name, `Multi`, or the note's name.
    pub label: String,
    /// The chains that receive its note.
    pub chains: Vec<ChainId>,
    /// Its mute: the chains' activators off, lit while they are.
    pub mute: PadSwitch,
    /// Its solo: the chains' Solo.
    pub solo: PadSwitch,
    /// Whether any of its chains' Preview buttons is held.
    pub previewing: bool,
    /// Whether its one chain plays a sample, so it has a Hot-Swap Sample
    /// button.
    pub has_sample: bool,
    /// Whether it's the selected pad.
    pub selected: bool,
}

/// The pad view (drum racks spec, R11 to R13).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PadFrame {
    /// The lowest note shown: a multiple of 4, at most 112.
    pub first: u8,
    /// The sixteen pads shown, from `first` up: the grid reads rising left
    /// to right and from the bottom row up.
    pub pads: Vec<PadState>,
    /// The notes that have a chain, for the overview.
    pub occupied: Vec<u8>,
    /// The selected pad.
    pub selected: Option<u8>,
    /// The pad hot-swap targets, while it does.
    pub hot_swap: Option<u8>,
}

/// A drum chain's row in the chain list (drum racks spec, R21).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DrumRow {
    /// What it answers.
    pub receive: Receive,
    /// How the chooser reads it.
    pub receive_label: String,
    /// The note it passes on.
    pub play: u8,
    /// How the chooser reads it.
    pub play_label: String,
    /// Its choke group.
    pub choke: Option<u8>,
    /// Its send to each return chain, in the rack's order, as linear gain.
    pub sends: Vec<f64>,
    /// Whether its Preview button is held.
    pub previewing: bool,
    /// Whether Play and Choke are disabled: Receive is All Notes.
    pub all_notes: bool,
}

/// A return chain's output (drum racks spec, R8).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReturnRow {
    /// The return track it sends to, or `null` for the rack's own output.
    pub audio_to: Option<TrackId>,
    /// How its Audio To chooser reads it.
    pub audio_to_label: String,
}

/// An entry of a return chain's Audio To chooser.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReturnChoice {
    /// The return track.
    pub track: TrackId,
    /// Its label: its letter and name.
    pub label: String,
}

/// Arguments of the commands that act on one pad of a drum rack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PadArgs {
    /// The drum rack; the selected device if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rack: Option<DeviceId>,
    /// The pad's note, 0 to 127.
    pub note: u8,
}

/// Arguments of `rack.move_pads`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MovePadsArgs {
    /// The drum rack; the selected device if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rack: Option<DeviceId>,
    /// Show the sixteen notes from here (rounded down to a row); the view
    /// moves by `groups` and `rows` from where it is if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first: Option<u8>,
    /// How many groups of sixteen notes to move up (positive) or down.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groups: Option<i32>,
    /// How many rows of four notes to move up or down.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<i32>,
}

/// Arguments of `pad.set_mute` and `pad.set_solo`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PadSwitchArgs {
    /// The drum rack; the selected device if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rack: Option<DeviceId>,
    /// The pad's note.
    pub note: u8,
    /// Turn it on (true) or off (false); switches if left out, a mixed
    /// pad turning on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
}

/// Arguments of `pad.preview`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PadPreviewArgs {
    /// The drum rack; the selected device if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rack: Option<DeviceId>,
    /// The pad's note.
    pub note: u8,
    /// Press (true) or release (false) the Preview button.
    pub on: bool,
}

/// Arguments of `chain.preview`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChainPreviewArgs {
    /// The drum chain.
    pub chain: ChainId,
    /// Press (true) or release (false) the Preview button.
    pub on: bool,
}

/// Arguments of `pad.drop`: one of `samples`, `device`, `preset` and
/// `move_device`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PadDropArgs {
    /// The drum rack; the selected device if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rack: Option<DeviceId>,
    /// The pad's note; with several samples, the first of the pads they
    /// rise from.
    pub note: u8,
    /// Audio files: one goes on the pad, several on pads rising from it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<String>,
    /// With several samples, layer them on this one pad (Cmd, or Alt on
    /// Windows and Linux) rather than spread them over pads.
    #[serde(default, skip_serializing_if = "is_false")]
    pub layer: bool,
    /// A device to drop, as `device.add` describes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<NewDevice>,
    /// A preset file to drop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// A device to move onto the pad from where it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub move_device: Option<DeviceId>,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_false(value: &bool) -> bool {
    !*value
}

/// What `pad.drop` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PadDropped {
    /// The chains the drop made or changed, in order.
    pub chains: Vec<ChainId>,
    /// The pads they are on, in order.
    pub notes: Vec<u8>,
}

/// Arguments of `pad.swap`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PadSwapArgs {
    /// The drum rack; the selected device if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rack: Option<DeviceId>,
    /// The pad dragged.
    pub from: u8,
    /// The pad it's dropped on.
    pub to: u8,
    /// Layer both pads' chains on `to` instead (Cmd, or Alt on Windows and
    /// Linux).
    #[serde(default, skip_serializing_if = "is_false")]
    pub layer: bool,
}

/// Arguments of `pad.rename`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PadRenameArgs {
    /// The drum rack; the selected device if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rack: Option<DeviceId>,
    /// The pad's note.
    pub note: u8,
    /// The new name; empty gives the name a chain has without one.
    pub name: String,
}

/// Arguments of `pad.hot_swap`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PadHotSwapArgs {
    /// The drum rack; the selected device if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rack: Option<DeviceId>,
    /// The pad's note.
    pub note: u8,
    /// Start (true) or end (false) hot-swapping the pad; switches if left
    /// out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
}

/// Arguments of `device.group_to_drum_rack`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GroupToDrumRackArgs {
    /// The devices, all in one chain of a track; the selected device if
    /// left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub devices: Option<Vec<DeviceId>>,
}

/// Arguments of `chain.extract`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtractChainArgs {
    /// The drum chain.
    pub chain: ChainId,
}

/// What `chain.extract` and `chain.extract_return` return.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Extracted {
    /// The new track.
    pub track: TrackId,
    /// The devices that moved to it, in order.
    pub devices: Vec<DeviceId>,
    /// How many clips on the new track hold notes taken from the old.
    pub clips: usize,
}

/// Adds the drum rack commands to a registry.
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

#[expect(clippy::too_many_lines, reason = "one spec per command, in order")]
pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(MOVE_PADS, "Move the Pads")
            .description(
                "Moves the sixteen notes a drum rack's pad view shows (the rack in focus, or \
                 `rack`): to `first` (a note, rounded down to a row), or by `groups` of sixteen \
                 notes and `rows` of four from where it is, as dragging the overview's \
                 selector, or Up and Down with Cmd (Alt on Windows and Linux) for a row, do. \
                 The view stays within notes 0 to 127. It changes only the view: it isn't an \
                 undo step, and the set keeps which notes show (`device_view.state`, the \
                 rack's `pads.first`).",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<MovePadsArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(PAD_SELECT, "Select Pad")
            .description(
                "Selects a pad, as clicking it does, and its chains in the chain list, so the \
                 devices view shows them. The selected pad is the one D switches hot-swap to. \
                 It changes only the view.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<PadArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(PAD_SET_MUTE, "Mute Pad")
            .description(
                "Mutes a pad, or unmutes it: it switches off the Chain Activator of each of \
                 the pad's chains (a pad of several chains mutes them all, and a mixed pad \
                 mutes the rest). One undo step.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<PadSwitchArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(PAD_SET_SOLO, "Solo Pad")
            .description(
                "Solos a pad, or unsolos it: it sets the Solo of each of the pad's chains, \
                 and leaves every other chain's as it is. While any drum chain is soloed the \
                 rack's return chains stay audible carrying only the soloed chains' sends. \
                 One undo step.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<PadSwitchArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(PAD_PREVIEW, "Preview Pad")
            .description(
                "Holds a pad's Preview button, or lets it go: while it is held each of the \
                 pad's chains plays its Receive note into itself alone at velocity 100 (C3 \
                 for a chain on All Notes), through the rest of the track's chain and its \
                 mixer, whether or not the song plays. A press with no release plays until \
                 the release, so an agent that presses must release. It changes nothing in \
                 the set and isn't an undo step.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<PadPreviewArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(CHAIN_PREVIEW, "Preview Chain")
            .description(
                "Holds a drum chain's Preview button, or lets it go: while it is held the \
                 chain plays its Receive note into itself alone, as `pad.preview` does for a \
                 pad. It changes nothing in the set and isn't an undo step.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<ChainPreviewArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(PAD_DROP, "Drop on Pad")
            .description(
                "Drops what the browser drags onto a pad. One audio file in `samples` on an \
                 empty pad makes a chain on its note holding the sampler with the file \
                 loaded (or a chain made from the pad default that Save as Default Pad \
                 saved), named after the file; on a pad with a chain it replaces the chain's \
                 sampler and sample and keeps its effects. Several files go one to a pad, \
                 rising a note each from `note` (those past note 127 are left out), or with \
                 `layer` onto that one pad as an instrument rack with a chain per file. A \
                 `device` or `preset` goes at the end of the pad's chain if it is an audio \
                 effect, at the end of its MIDI effects if it is a MIDI effect, and in place \
                 of its instrument if it is an instrument; on an empty pad each makes a \
                 chain holding it. A pad of several chains takes no drop. One undo step.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<PadDropArgs>()
            .result::<PadDropped>(),
        CommandSpec::new(PAD_SWAP, "Swap Pads")
            .description(
                "Drags one pad onto another: each pad's chains take the other pad's note as \
                 their Receive, so the set's clips play the other sound. With `layer` (Cmd, \
                 or Alt on Windows and Linux) both pads' chains are layered on `to` as one \
                 chain holding an instrument rack of them, and `from` is left empty. One \
                 undo step.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<PadSwapArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(PAD_SAVE_DEFAULT, "Save as Default Pad")
            .description(
                "Saves the pad's chain as the default pad: a sample dropped on an empty pad \
                 afterwards makes its chain from that default, with the sample in the \
                 first sampler. The one saved last is used. A pad of one chain only. It \
                 changes nothing in the set. Writes a file outside the project.",
            )
            .permission(Permission::External)
            .context(DeviceView)
            .args::<PadArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(PAD_DELETE, "Delete Pad")
            .description(
                "Deletes the chains on a pad, with their devices and automation: one undo \
                 step (Delete Chain). Undo restores them, but because it removes the \
                 person's work an agent asks first.",
            )
            .permission(Permission::Destructive)
            .context(DeviceView)
            .args::<PadArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(PAD_RENAME, "Rename Pad")
            .description(
                "Renames the chain of a pad that holds one chain: an empty name gives it \
                 the name a chain has without one. A pad of several chains can't be \
                 renamed. One undo step.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<PadRenameArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(PAD_HOT_SWAP, "Hot-Swap Pad")
            .description(
                "Makes a pad the target of hot-swap, as its Hot-Swap button does, or ends \
                 it: loading an item from the browser then lands on the pad as dropping it \
                 would (`pad.drop`). It ends the hot-swap of a device or a chain. Only a \
                 pad of one chain, or an empty one, can be the target. It changes only the \
                 view.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<PadHotSwapArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(PAD_HOT_SWAP_SAMPLE, "Hot-Swap Sample")
            .description(
                "A pad's Hot-Swap Sample button: it makes the pad the hot-swap target and \
                 opens the folder of the sample its chain plays in the browser, so loading \
                 a sample replaces the pad's (one undo step each). Pressing it again, the \
                 hot-swap bar's close button, Esc, Q, or going from the browser to another \
                 view ends it. Only a pad whose chain plays a sample has one.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<PadHotSwapArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(SWITCH_HOT_SWAP_TARGET, "Switch Hot-Swap Target")
            .description(
                "While a drum rack is being hot-swapped, switches the target between the \
                 rack and the rack's last selected pad, as D does; with a pad the target \
                 an item loaded from the browser lands on the pad. It changes only the \
                 view.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-VIEW-007")
            .args::<bl_commands::NoArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(GROUP_TO_DRUM_RACK, "Group to Drum Rack")
            .description(
                "Wraps the selected devices (or `devices`, all in one chain of a MIDI \
                 track) in a new drum rack, which takes the first device's place: the rack \
                 holds one chain, named after the first device, that receives note 36 (C1, \
                 the first pad a new drum rack shows) and plays C3, and holds the devices \
                 in their order, each keeping its settings and running state. They must \
                 fit one chain of an instrument rack: MIDI effects, then at most one \
                 instrument, then audio effects. One undo step (Group).",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<GroupToDrumRackArgs>()
            .result::<crate::devices::Grouped>(),
        CommandSpec::new(CHAIN_EXTRACT, "Extract Chain")
            .description(
                "Pulls a drum chain out of its rack onto a new MIDI track, as dragging it \
                 from the Session mixer onto the drop area right of the tracks does: the \
                 track holds the chain's devices, and the notes the chain received leave \
                 the clips of the rack's track (Arrangement and Session) for clips on the \
                 new track at the same places and in the same slots, moved to the chain's \
                 Play note so its devices hear what they heard in the rack. The chain \
                 leaves the rack. One undo step (Move Chain). Dragged from the chain list \
                 onto another track or rack it takes only its devices: \
                 `chain.drop_on_track`.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<ExtractChainArgs>()
            .result::<Extracted>(),
        CommandSpec::new(CHAIN_EXTRACT_RETURN, "Extract Return Chain")
            .description(
                "Pulls a return chain out of its drum rack onto a new return track after the \
                 last one, as dragging it onto the Session mixer's returns does: the track \
                 holds the chain's audio effects, and the drum chains' sends to it leave \
                 the rack with it. One undo step (Move Chain).",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<ExtractChainArgs>()
            .result::<Extracted>(),
    ]
}

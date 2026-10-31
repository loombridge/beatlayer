//! What a surface that mixes reads of the window's view state and of the
//! In/Out section (`docs/specs/push-2-tracks.md`, R31, R37 and R38): which
//! groups and racks are folded out, and the choosers of the selected
//! track's routing, each with the command every entry runs.

use bl_model::{DeviceId, TrackId};
use serde_json::Value;

/// One line of a chooser: a choice and the command that makes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// What it reads.
    pub label: String,
    /// Whether it is the chosen one.
    pub chosen: bool,
    /// The command choosing it runs.
    pub command: String,
    /// Its arguments.
    pub args: Value,
}

/// One of the selected track's routing choosers.
#[derive(Clone, Debug, PartialEq)]
pub struct Chooser {
    /// Its name.
    pub name: String,
    /// What it shows now.
    pub text: String,
    /// What it lists.
    pub entries: Vec<Entry>,
}

/// A track's In/Out section in the order of R31: Input Type, Input Channel,
/// Monitor, Output Type and Output Channel; a track without one leaves its
/// place empty.
#[derive(Clone, Debug, PartialEq)]
pub struct InOut {
    /// The track.
    pub track: TrackId,
    /// The five choosers.
    pub choosers: [Option<Chooser>; 5],
}

/// The window's own state that Mix Mode acts on and shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mix {
    /// The group tracks that are folded: the tracks inside them are hidden
    /// (`arrangement.fold_groups`).
    pub folded_groups: Vec<TrackId>,
    /// The racks whose chains are folded out as strips in the mixers
    /// (`mixer_view.fold_chains`).
    pub open_racks: Vec<DeviceId>,
    /// The In/Out section of the first selected track.
    pub in_out: Option<InOut>,
}

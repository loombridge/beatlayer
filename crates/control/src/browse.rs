//! What a surface that browses reads of the browser (`docs/specs/push-2-tracks.md`,
//! R5, R6 and R13): lists of entries it can fetch on its own thread without
//! waiting for the index or the disk.

use std::fmt::Debug;
use std::sync::Arc;

use bl_model::DeviceRole;
use serde_json::Value;

/// What Load does with an entry.
#[derive(Clone, Debug, PartialEq)]
pub enum Load {
    /// A device or an empty rack, as `device.add` takes it (its `device`
    /// argument).
    Device {
        /// The new device's JSON.
        new_device: Value,
        /// Where it goes in a chain.
        role: DeviceRole,
    },
    /// A preset file, by `device.add_preset` or `device.replace`.
    Preset {
        /// The file.
        path: String,
        /// Its category's role, when the folder it is in says it.
        role: Option<DeviceRole>,
    },
    /// A plug-in, by `plugin.load`.
    Plugin {
        /// Its file or bundle.
        path: String,
        /// Its identifier within the file.
        plugin: String,
        /// What it is in a chain.
        role: DeviceRole,
    },
    /// An audio file, which `session.drop_files` puts in an empty slot
    /// (push-2-tracks R43).
    Sample {
        /// The file.
        path: String,
    },
}

impl Load {
    /// The role the loaded device takes in a chain, if it is known.
    #[must_use]
    pub const fn role(&self) -> Option<DeviceRole> {
        match self {
            Self::Device { role, .. } | Self::Plugin { role, .. } => Some(*role),
            Self::Preset { role, .. } => *role,
            Self::Sample { .. } => None,
        }
    }
}

/// One entry of a column.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// What [`Catalogue::children`] is asked for: unique among the entries
    /// of one offer.
    pub key: String,
    /// The name it shows.
    pub name: String,
    /// What Load does with it; none for a label or a folder.
    pub load: Option<Load>,
    /// Whether it can hold entries: a label or a folder.
    pub folder: bool,
}

/// The browser's lists, as of one moment. Each call answers from what is
/// indexed now and never waits.
pub trait Catalogue: Send + Sync + Debug {
    /// The leftmost column for an offer: the labels, with the collections
    /// after them. `role` is the role of what Load replaces or adds, or none
    /// for everything a MIDI track can hold.
    fn labels(&self, role: Option<DeviceRole>) -> Vec<Entry>;

    /// What the entry with `key` holds, folders first.
    fn children(&self, key: &str, role: Option<DeviceRole>) -> Vec<Entry>;

    /// The audio files Clip Mode offers an empty audio slot, by name
    /// (push-2-tracks R43).
    fn samples(&self) -> Vec<Entry> {
        Vec::new()
    }
}

/// The browser as a surface sees it each tick.
#[derive(Clone, Debug)]
pub struct Browse {
    /// Changes whenever the lists change, so a surface knows when to read
    /// them again.
    pub revision: u64,
    /// Whether the browser's Preview switch is on.
    pub preview: bool,
    /// The lists.
    pub catalogue: Arc<dyn Catalogue>,
}

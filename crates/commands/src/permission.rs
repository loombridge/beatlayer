//! Permission classes: what a command may touch, and so whether an agent
//! must ask before running it.

use core::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What a command may touch (decision D17).
///
/// The agent API runs `read` and `edit` commands straight away, and asks the
/// user before a `destructive` or `external` one unless the user has turned
/// on autonomous mode for that agent.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Reads the project or the app's state and changes nothing.
    Read,
    /// Changes the project in a way undo reverses. Deleting a track, clip,
    /// or device is an edit, because undo brings it back.
    Edit,
    /// Loses something undo can't bring back: deletes or overwrites a file,
    /// discards unsaved changes, clears the undo history, or quits.
    Destructive,
    /// Reaches outside the project: files elsewhere on disk, the network,
    /// audio and MIDI devices, or other applications.
    External,
}

impl Permission {
    /// Every class, from the least to the most reach.
    pub const ALL: [Self; 4] = [Self::Read, Self::Edit, Self::Destructive, Self::External];

    /// The class's name as it's serialised, such as `edit`.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Edit => "edit",
            Self::Destructive => "destructive",
            Self::External => "external",
        }
    }

    /// What a command in this class may do, in a sentence.
    #[must_use]
    pub const fn meaning(self) -> &'static str {
        match self {
            Self::Read => "Reads the project or the app's state and changes nothing.",
            Self::Edit => {
                "Changes the project in a way undo reverses, including deleting tracks, clips, \
                 and devices."
            }
            Self::Destructive => {
                "Loses something undo can't bring back: deletes or overwrites a file, discards \
                 unsaved changes, clears the undo history, or quits."
            }
            Self::External => {
                "Reaches outside the project: files elsewhere on disk, the network, audio and \
                 MIDI devices, or other applications."
            }
        }
    }

    /// Whether an agent must ask the user before running a command in this
    /// class (unless the user has turned on autonomous mode for it).
    #[must_use]
    pub const fn asks_first(self) -> bool {
        matches!(self, Self::Destructive | Self::External)
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_match_serde() {
        for permission in Permission::ALL {
            assert_eq!(
                serde_json::to_value(permission).unwrap(),
                permission.id(),
                "{permission:?}"
            );
        }
    }

    #[test]
    fn only_destructive_and_external_ask_first() {
        let asking: Vec<_> = Permission::ALL
            .into_iter()
            .filter(|p| p.asks_first())
            .collect();
        assert_eq!(asking, [Permission::Destructive, Permission::External]);
    }
}

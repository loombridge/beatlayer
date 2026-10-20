//! The Clip View's Audio Utilities panel commands (WP-4.10b): opening a
//! clip's file in the external sample editor, and replacing a clip's file
//! with another. The panel's other controls run the document's own `clip.*`
//! commands, which agents run too. The behaviour is in the sample editor
//! spec, R16 and R18.
//!
//! These need the media store or the desktop, so they belong to the shell
//! rather than to the document. [`register`] adds their specs, which are
//! pure data, so `cargo xtask` can check them and list them in the command
//! reference; `bl-shell` runs them by ID.

use bl_commands::Context::ClipView;
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_model::{ClipId, ContentHash};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `clip.edit_sample`: open an audio clip's file in the external sample
/// editor.
pub const EDIT_SAMPLE: &str = "clip.edit_sample";

/// `clip.replace_sample`: replace an audio clip's file with another.
pub const REPLACE_SAMPLE: &str = "clip.replace_sample";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[EDIT_SAMPLE, REPLACE_SAMPLE];

/// Whether `id` is one of the Audio Utilities panel's commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `clip.edit_sample`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditSampleArgs {
    /// The audio clip. Left out, the one the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
}

/// Arguments of `clip.replace_sample`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplaceSampleArgs {
    /// The audio clip. Left out, the one the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The file the clip plays instead.
    pub path: String,
}

/// What `clip.edit_sample` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EditedSample {
    /// The clip whose file was opened.
    pub clip: ClipId,
    /// The file that was opened.
    pub file: String,
    /// The editor the file was opened in, or the Settings page that was
    /// opened instead, when the File & Folder page names no editor.
    pub editor: String,
    /// Whether the editor was found and started. `false` when the File &
    /// Folder page names no editor, so the Settings page was opened instead.
    pub opened: bool,
}

/// What `clip.replace_sample` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReplacedSample {
    /// The clip.
    pub clip: ClipId,
    /// The file the clip plays now.
    pub file: String,
    /// The media it plays, by its content hash.
    pub media: ContentHash,
}

/// Adds the Audio Utilities panel's commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(EDIT_SAMPLE, "Edit Sample")
            .description(
                "Opens an audio clip's file in the external sample editor, as the Edit button in \
                 the Clip View's Audio Utilities panel does: the editor the File & Folder page's \
                 Sample Editor row names, or that page itself when it names none. The file's \
                 content is checked when Beatlayer comes to the front again: if it has changed, \
                 every clip of the set that played the old file plays the new one, in one undo \
                 step, each keeping its warp markers only if the file kept its length. Opens a \
                 program outside the set.",
            )
            .permission(Permission::External)
            .context(ClipView)
            .args::<EditSampleArgs>()
            .result::<EditedSample>(),
    )?;
    registry.register(
        CommandSpec::new(REPLACE_SAMPLE, "Replace Sample")
            .description(
                "Replaces an audio clip's file with the file at `path`, as dropping a file from \
                 the browser on the Clip View does: the clip keeps its gain and pitch, and its \
                 warp markers only if the new file is exactly as long as the old one. One undo \
                 step.",
            )
            .permission(Permission::Edit)
            .context(ClipView)
            .args::<ReplaceSampleArgs>()
            .result::<ReplacedSample>(),
    )?;
    Ok(())
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
        let class = |id: &str| registry.get(id).unwrap().permission;
        assert_eq!(class(EDIT_SAMPLE), Some(Permission::External));
        assert_eq!(class(REPLACE_SAMPLE), Some(Permission::Edit));
    }
}

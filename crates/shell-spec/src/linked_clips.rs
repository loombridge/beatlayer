//! Linked clips on the shell (WP-6.44; the linked clips spec): Duplicate as
//! Linked Clip (IMP-21), which needs the shell's selection and the two views'
//! rules for where a copy goes.
//!
//! `clip.link` and `clip.unlink` are `bl-model`'s: they act on the clips
//! they name.

use bl_commands::Context::{Arrangement, Session};
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_model::ClipId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::editing::EditResult;

/// `clip.duplicate_linked`: put a linked copy of each clip where Duplicate
/// puts a copy (IMP-21).
pub const DUPLICATE_LINKED: &str = "clip.duplicate_linked";

/// The undo step's name (R1).
pub const DUPLICATE_LINKED_STEP: &str = "Duplicate as Linked Clip";

/// The model's `clip.unlink`, which the clips' context menus run (R8).
pub const UNLINK: &str = "clip.unlink";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[DUPLICATE_LINKED];

/// Whether `id` is one of these commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `clip.duplicate_linked`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DuplicateLinkedArgs {
    /// The clips to copy, all in the Arrangement or all in Session slots;
    /// the selected clips if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipId>>,
}

/// Adds the linked clip commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(DUPLICATE_LINKED, "Duplicate as Linked Clip")
            .description(
                "Puts a copy of each clip (`clips`, or the selected clips) that shares its \
                 contents with the clip: its notes and MIDI controller envelopes, or its audio \
                 file with its warping, gain, HiQ and RAM, so an edit to one reaches every \
                 clip of the link in one undo step (the clips keep their own place, length, \
                 markers, loop and name). In the Arrangement the copies go where Duplicate puts \
                 them, right after the span the clips cover; in the Session View, in the first \
                 empty slot below each clip on its track, a scene being added where there is \
                 none. A clip that was independent joins a new link with its copy; a linked \
                 clip's copy joins its link. The copies become the selection. One undo step, \
                 named Duplicate as Linked Clip. `created` lists the copies. Read a clip's \
                 link back through the query tool's `clip.link`; `clip.unlink` takes a clip \
                 out of its link.",
            )
            .permission(Permission::Edit)
            .contexts([Arrangement, Session])
            .args::<DuplicateLinkedArgs>()
            .result::<EditResult>(),
    )
}

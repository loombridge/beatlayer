//! Scenes: the Session View's rows.
//!
//! A scene is one row of the Session grid. It holds one clip slot in every
//! audio and MIDI track; the clips themselves live in the tracks, keyed by
//! scene, so a scene is its ID and its name and moving the scenes moves
//! their clips with them. The slots and the commands that edit them are in
//! the session view spec.

use bl_core::id::Id;
use serde::{Deserialize, Serialize};

use crate::error::ModelError;
use crate::ids::SceneId;

/// How many scenes a new set opens with (the layout reference's default
/// state: eight).
pub const DEFAULT_SCENE_COUNT: usize = 8;

/// A scene: one row of the Session grid, holding a clip slot in every audio
/// and MIDI track.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scene {
    /// The scene's ID.
    pub id: SceneId,
    /// The scene's name, which may be empty: a scene without a name shows
    /// its number, which is its position.
    pub name: String,
}

impl Scene {
    /// An unnamed scene.
    #[must_use]
    pub fn new(id: SceneId) -> Self {
        Self {
            id,
            name: String::new(),
        }
    }
}

/// The default scenes: [`DEFAULT_SCENE_COUNT`] unnamed scenes with IDs
/// straight after `after`, the largest ID in the document so far. A new set
/// and a set saved before scenes existed both get these, and no ID is
/// reused, because a loader moves the project's allocator past the
/// document's largest ID.
///
/// # Errors
///
/// [`ModelError::IdsExhausted`] if `after` leaves no room for them: a loaded
/// document may hold any IDs, so this isn't a panic.
pub(crate) fn default_scenes(after: Id) -> Result<Vec<Scene>, ModelError> {
    (1..=DEFAULT_SCENE_COUNT as u64)
        .map(|step| {
            let raw = after
                .get()
                .checked_add(step)
                .ok_or(ModelError::IdsExhausted)?;
            Ok(Scene::new(SceneId(
                Id::new(raw).expect("object IDs are never zero"),
            )))
        })
        .collect()
}

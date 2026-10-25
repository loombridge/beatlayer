//! Scenes: the Session View's rows.
//!
//! A scene is one row of the Session grid. It holds one clip slot in every
//! audio and MIDI track; the clips themselves live in the tracks, keyed by
//! scene, so a scene is its ID and its name and moving the scenes moves
//! their clips with them. The slots and the commands that edit them are in
//! the session view spec.

use bl_core::id::Id;
use bl_core::time::{Bpm, TimeSignature};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ModelError;
use crate::follow::FollowActions;
use crate::ids::SceneId;

/// How many scenes a new set opens with (the layout reference's default
/// state: eight).
pub const DEFAULT_SCENE_COUNT: usize = 8;

/// A scene: one row of the Session grid, holding a clip slot in every audio
/// and MIDI track.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Scene {
    /// The scene's ID.
    pub id: SceneId,
    /// The scene's name, which may be empty: a scene without a name shows
    /// its number, which is its position.
    pub name: String,
    /// The tempo the scene sets when it is launched, if it has its own
    /// (20 to 999 BPM). `None` leaves the set's tempo as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tempo: Option<Bpm>,
    /// The time signature the scene sets when it is launched, if it has its
    /// own. `None` leaves the set's signature as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<TimeSignature>,
    /// What the scene does by itself after it has been launched for a while
    /// (WP-3.10): on a scene, Linked is the Longest button.
    #[serde(default, skip_serializing_if = "FollowActions::is_default")]
    pub follow: FollowActions,
    /// The scene's colour, a swatch from 0 to 69, or none (the themes spec,
    /// R14): a new scene has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<u8>,
    /// The user's own info text (Edit Info Text), which the Info View shows while the pointer is over the scene; empty for none, and left out of the saved set then (the Info View spec, R9).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub info_text: String,
}

/// The lowest tempo a scene can set (Live's scene tempo range).
pub const SCENE_TEMPO_MIN: f64 = 20.0;
/// The highest tempo a scene can set.
pub const SCENE_TEMPO_MAX: f64 = 999.0;

impl Scene {
    /// The scene as the engine hears it: its colour is never heard (the
    /// themes spec, R27), so a change set leaves it out.
    pub(crate) fn without_colour(&self) -> Self {
        Self {
            colour: None,
            ..self.clone()
        }
    }

    /// An unnamed scene.
    #[must_use]
    pub fn new(id: SceneId) -> Self {
        Self {
            id,
            name: String::new(),
            tempo: None,
            signature: None,
            follow: FollowActions::default(),
            colour: None,
            info_text: String::new(),
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

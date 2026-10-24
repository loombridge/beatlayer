//! A drag of clips between the Session and the Arrangement (WP-6.29b).
//!
//! The drag starts in one view's grid or lanes, may carry on over the view
//! selector, which switches the window to the other view, or into the other
//! window, and ends where the pointer is let go. Nothing in the set changes
//! until then: each window reports the place under its pointer, and the drop
//! runs `session.copy_to_arrangement` or `session.copy_to_session` there, as
//! one undo step (the second window spec, R18 to R23). The drag is the
//! person's hand on the set, so an agent's calls wait until it ends.

use bl_core::time::Ticks;
use bl_model::{ClipId, SceneId, TrackId};
use serde_json::json;

use crate::WindowKind;

/// What is being dragged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DragSource {
    /// Session clips, by the IDs they have in their slots.
    Slots(Vec<ClipId>),
    /// Arrangement clips.
    Arrangement(Vec<ClipId>),
}

/// A place the clips can be dropped, in the other view from the one they
/// came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropSpot {
    /// On this track's lane, starting at this position (already snapped).
    Arrangement {
        /// The track under the pointer.
        track: TrackId,
        /// Where the first clip starts.
        at: Ticks,
    },
    /// On this slot.
    Session {
        /// The track under the pointer.
        track: TrackId,
        /// The scene under the pointer.
        scene: SceneId,
    },
}

/// A step of the drag, as a window tells the shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipDragEvent {
    /// The drag began: clips were picked up.
    Begin(DragSource),
    /// The window's pointer is over this place, or over no place that takes
    /// the clips (`None`).
    Over(Option<DropSpot>),
    /// The drag ended: dropped where the pointer last was (`drop`), or
    /// abandoned with Esc.
    End {
        /// Whether the clips are dropped.
        drop: bool,
    },
}

/// A drag under way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipDrag {
    /// What's being dragged.
    pub source: DragSource,
    /// The window it began in.
    pub window: WindowKind,
    /// The window the pointer is over and the place it is over there.
    pub over: Option<(WindowKind, DropSpot)>,
}

impl ClipDrag {
    /// The command a drop at `spot` runs, with its arguments: Session clips
    /// go to the Arrangement and Arrangement clips to the Session View; a
    /// place in the view the clips came from takes them by the grid's own
    /// drag instead.
    #[must_use]
    pub fn drop_command(&self) -> Option<(&'static str, serde_json::Value)> {
        let (_, spot) = self.over.as_ref()?;
        match (&self.source, spot) {
            (DragSource::Slots(clips), DropSpot::Arrangement { track, at }) => Some((
                bl_shell_spec::session_view::COPY_TO_ARRANGEMENT,
                json!({ "clips": clips, "track": track, "at": at }),
            )),
            (DragSource::Arrangement(clips), DropSpot::Session { track, scene }) => Some((
                bl_shell_spec::session_view::COPY_TO_SESSION,
                json!({ "clips": clips, "track": track, "scene": scene }),
            )),
            _ => None,
        }
    }
}

//! Arm by selection (`docs/specs/push-2-notes.md`, R3): while a Push 2
//! surface runs, selecting a MIDI track arms it, and the track armed that
//! way before is disarmed. It follows the selection and isn't an undo step.

use bl_model::{Editor, TrackId, TrackKind};
use serde_json::Value;

use super::{Shell, snapshot};

impl Shell {
    /// The editor a save writes instead of `self.editor` (R3): the set
    /// without the arms a surface made by selection, which aren't saved.
    /// `None` where there are none.
    pub(super) fn without_selection_arms(&self) -> Option<Editor> {
        if self.surface.selection_armed.is_empty() {
            return None;
        }
        let mut copy = snapshot(&self.editor);
        for track in &self.surface.selection_armed {
            copy.follow_arm(*track, false);
        }
        Some(copy)
    }

    /// Spec: push-2-notes R3. Arms the newly selected MIDI track while a Push
    /// 2 runs, disarms the one armed that way before, and lets go of every
    /// selection arm when the last Push 2 stops. Returns whether the set
    /// changed.
    pub(super) fn poll_selection_arm(&mut self) -> bool {
        let running = !self.pushes().is_empty();
        let doc = self.editor.document();
        // An undone arm, or one the person ended, is no longer ours.
        self.surface
            .selection_armed
            .retain(|track| doc.track(*track).is_some_and(|t| t.mixer().arm));
        let mut changed = false;
        if !running {
            self.surface.armed_for = None;
            for track in std::mem::take(&mut self.surface.selection_armed) {
                changed |= !self.editor.follow_arm(track, false).is_empty();
            }
        } else if let Some(track) = self.selected_midi_track()
            && self.surface.armed_for != Some(track)
        {
            self.surface.armed_for = Some(track);
            let before: Vec<TrackId> = std::mem::take(&mut self.surface.selection_armed)
                .into_iter()
                .collect();
            for old in before {
                changed |= !self.editor.follow_arm(old, false).is_empty();
            }
            let by_hand = self
                .editor
                .document()
                .track(track)
                .is_some_and(|t| t.mixer().arm);
            if !by_hand {
                changed |= !self.editor.follow_arm(track, true).is_empty();
                self.surface.selection_armed.insert(track);
            }
        }
        if changed {
            self.document_changed();
            self.touch();
        }
        changed
    }

    /// Whether `track` is armed by a surface's selection, not by hand (R3).
    #[must_use]
    pub fn armed_by_selection(&self, track: TrackId) -> bool {
        self.surface.selection_armed.contains(&track)
    }

    /// Marks `track`, which is armed, as armed by selection, as a Push 2's
    /// selection would: for the tests of what draws it.
    pub(crate) fn mark_armed_by_selection(&mut self, track: TrackId) {
        self.surface.selection_armed.insert(track);
    }

    /// The first selected track, if it is a MIDI track.
    fn selected_midi_track(&self) -> Option<TrackId> {
        let doc = self.editor.document();
        let track = crate::devices::shown_track(&self.arrangement, doc)?;
        (doc.track(track)?.kind() == TrackKind::Midi).then_some(track)
    }

    /// Spec: push-2-notes R3. Arming a track by hand, as its Arm switch or
    /// `mixer.set_arm` does, makes it a manual arm that stays when another
    /// track is selected.
    pub(super) fn arm_taken_by_hand(&mut self, args: &Value) {
        if let Some(track) = args
            .get("track")
            .and_then(|t| serde_json::from_value(t.clone()).ok())
        {
            self.surface.selection_armed.remove(&track);
        }
    }
}

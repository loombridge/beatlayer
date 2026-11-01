//! The clip the Clip View shows and the scale that follows it.

use std::collections::HashMap;

use super::Shell;
use bl_shell_spec::editing as edit_ids;
use bl_shell_spec::scales as scales_ids;
use bl_shell_spec::session_view as session_ids;

impl Shell {
    /// The clip the Clip View shows: the Session clip most recently
    /// selected, or else the Arrangement's first selected clip.
    pub(super) fn shown_clip(&self) -> Option<bl_model::ClipId> {
        self.clip_view
            .shown_any_clip(self.editor.document(), &self.arrangement)
    }

    /// Points the set's current scale at the clip the Clip View shows, when
    /// the selection changed or the clip's own scale did, by this command or
    /// by undoing or redoing one (the scales spec, R10). `shown` and its
    /// `scale` are what they were before the command. It is no undo step,
    /// though the set is marked as changed.
    pub(super) fn follow_shown_scale(
        &mut self,
        id: &str,
        shown: Option<bl_model::ClipId>,
        scale: Option<bl_model::Scale>,
    ) {
        if id == scales_ids::SET_CLIP_SCALE {
            self.clip_view.forget_scale_fold_rows();
        }
        let now = self.shown_clip();
        let now_scale = crate::scales::shown_scale(self.editor.document(), now);
        if (now, now_scale) == (shown, scale) && id != scales_ids::SET_CLIP_SCALE {
            return;
        }
        if let Some(scale) = now_scale {
            self.follow_scale(scale);
        }
    }

    /// Points the set's current scale at `scale` (the scales spec, R10, R11
    /// and R12): no undo step, though the set is marked as changed.
    pub(super) fn follow_scale(&mut self, scale: bl_model::Scale) {
        if self.editor.document().settings().scale == scale {
            return;
        }
        self.editor.follow_scale(scale);
        self.dirty = true;
        self.document_changed();
        self.touch();
    }

    /// The clip in the Session slot selected first, if the slot holds one.
    pub(super) fn selected_slot_clip(&self) -> Option<bl_model::ClipId> {
        let slot = self.session.first_slot()?;
        self.editor
            .document()
            .slot(slot.track, slot.scene)
            .map(|clip| clip.id)
    }

    /// The clips in the selected Session slots, in track then scene order.
    pub(super) fn selected_slot_clips(&self) -> Vec<bl_model::ClipId> {
        let doc = self.editor.document();
        let track_at: HashMap<_, _> = doc
            .track_order()
            .into_iter()
            .enumerate()
            .map(|(at, track)| (track, at))
            .collect();
        let scene_at: HashMap<_, _> = doc
            .scenes()
            .enumerate()
            .map(|(at, scene)| (scene.id, at))
            .collect();
        // Spec: multi-clip-editing R2. By track, then scene.
        let mut keyed: Vec<_> = self
            .session
            .selected_slots()
            .iter()
            .filter_map(|slot| {
                let clip = doc.slot(slot.track, slot.scene)?;
                Some((
                    (track_at.get(&slot.track), scene_at.get(&slot.scene)),
                    clip.id,
                ))
            })
            .collect();
        keyed.sort_unstable_by_key(|(key, _)| *key);
        keyed.into_iter().map(|(_, clip)| clip).collect()
    }

    /// The clips the Control Bar's scale controls change (the scales spec,
    /// R12): the Arrangement's selected clips, or the Session slot's clip.
    #[must_use]
    pub fn scale_targets(&self) -> Vec<bl_model::ClipId> {
        // Spec: multi-clip-editing R9, R15. Every clip the note editor
        // shows, or with Focus Mode on the active clip alone.
        let active = self
            .clip_view
            .active_clips(self.editor.document(), &self.arrangement);
        if active.len() > 1
            || self
                .clip_view
                .focus_active(self.editor.document(), &self.arrangement)
        {
            return active;
        }
        let arranged = self.arrangement.selected_clips();
        if !arranged.is_empty() {
            return arranged.to_vec();
        }
        self.selected_slot_clip().into_iter().collect()
    }

    /// What the Control Bar's scale controls show (the scales spec, R6): the
    /// selected clips' scale, where every one of them has it, and the set's
    /// current scale when no clip is selected.
    #[must_use]
    pub fn scale_display(&self) -> crate::scales::Display {
        let targets = self.scale_targets();
        crate::scales::display(self.editor.document(), &targets)
    }

    /// The name of the pitch class `class` (0 is C) as the Root Note
    /// choosers show it: spelled as the note spelling chooses, and for Auto
    /// by the scale they show (the scales spec, R24).
    #[must_use]
    pub fn root_name(&self, class: u8) -> String {
        let shown = self.scale_display();
        let scale = bl_model::Scale {
            enabled: shown.enabled == Some(true),
            root: shown.root.unwrap_or(0),
            name: shown.name.unwrap_or(bl_model::ScaleName::Major),
        };
        self.clip_view.note_spelling().class_name(class, &scale)
    }

    /// Points the Clip View at the clip most recently selected, in the
    /// Session View or the Arrangement, after the command `id` ran with the
    /// selections `arranged` and `slot` before it. Selecting again counts:
    /// clicking the slot that is already selected shows its clip again.
    pub(super) fn follow_clip_selection(
        &mut self,
        id: &str,
        arranged: &[bl_model::ClipId],
        slot: Option<bl_model::ClipId>,
        slots: &[bl_model::ClipId],
    ) {
        let now = self.selected_slot_clip();
        let all = self.selected_slot_clips();
        let picked = matches!(
            id,
            session_ids::SELECT | session_ids::SELECT_ALL | session_ids::SELECT_EDGE
        ) || now != slot
            || all != slots;
        if now.is_some() && picked {
            // Spec: multi-clip-editing R1, R3. Every selected slot's clip
            // shows, the one clicked last being the foreground clip.
            let doc = self.editor.document();
            let last = self
                .session
                .anchor()
                .and_then(|anchor| doc.slot(anchor.track, anchor.scene))
                .map(|clip| clip.id);
            self.clip_view.show_session_clips(all, last);
        } else if (matches!(id, edit_ids::SELECT_CLIPS | edit_ids::SELECT_ALL)
            || self.arrangement.selected_clips() != arranged)
            && !self.arrangement.selected_clips().is_empty()
        {
            self.clip_view.show_arranged_clips();
        }
    }
}

//! Linked clips in the document (WP-6.44; the linked clips spec): the clips
//! of one link hold the same contents, and one pass after every command
//! makes an edit to one of them reach the rest (R4).

use std::collections::{BTreeMap, BTreeSet};

use bl_core::id::Id;

use crate::clip::{Clip, ClipData};
use crate::document::Document;
use crate::error::ModelError;
use crate::ids::{ClipId, LinkId};
use crate::shared::Shared;

impl Document {
    /// The clips of a link, in ID order.
    #[must_use]
    pub fn link_members(&self, link: LinkId) -> Vec<ClipId> {
        let mut members: Vec<ClipId> = self
            .tracks
            .values()
            .flat_map(|track| track.all_clips())
            .filter(|clip| clip.link == Some(link))
            .map(|clip| clip.id)
            .collect();
        members.sort();
        members
    }

    /// The clips linked with `clip`, not counting it: empty for an
    /// independent clip (R14).
    #[must_use]
    pub fn linked_clip_ids(&self, clip: ClipId) -> Vec<ClipId> {
        let Some(link) = self.clip(clip).and_then(|clip| clip.link) else {
            return Vec::new();
        };
        let mut members = self.link_members(link);
        members.retain(|member| *member != clip);
        members
    }

    /// Links clips that already hold the same contents (R2, R14): into the
    /// link one of them is in, or a new one.
    pub(crate) fn link_clips(&mut self, clips: &[ClipId]) -> Result<(), ModelError> {
        let mut seen = BTreeSet::new();
        if let Some(twice) = clips.iter().find(|clip| !seen.insert(**clip)) {
            return Err(ModelError::DuplicateId(twice.0));
        }
        let [first, _, ..] = clips else {
            return Err(ModelError::InvalidInput("link at least two clips"));
        };
        let first = self.clip(*first).ok_or(ModelError::NoSuchClip(*first))?;
        let mut links = BTreeSet::new();
        for id in clips {
            let clip = self.clip(*id).ok_or(ModelError::NoSuchClip(*id))?;
            if clip.data.is_audio() != first.data.is_audio() {
                return Err(ModelError::InvalidInput(
                    "a link holds clips of one kind, audio or MIDI",
                ));
            }
            if !clip.shares_contents_with(first) {
                return Err(ModelError::InvalidInput(
                    "clips that hold different contents can't be linked",
                ));
            }
            links.extend(clip.link);
        }
        let link = match (links.pop_first(), links.is_empty()) {
            (Some(link), true) => link,
            (None, _) => self.next_link_id(),
            (Some(_), false) => {
                return Err(ModelError::InvalidInput(
                    "the clips belong to different links: unlink them first",
                ));
            }
        };
        for id in clips {
            if let Some(clip) = self.clip_in_place(*id) {
                clip.link = Some(link);
            }
        }
        Ok(())
    }

    /// Takes clips out of their links (R8); the pass after the command ends
    /// a link that is left with one clip.
    pub(crate) fn unlink_clips(&mut self, clips: &[ClipId]) -> Result<(), ModelError> {
        let mut seen = BTreeSet::new();
        for id in clips {
            if !seen.insert(*id) {
                return Err(ModelError::DuplicateId(id.0));
            }
            self.clip(*id).ok_or(ModelError::NoSuchClip(*id))?;
        }
        for id in clips {
            if let Some(clip) = self.clip_in_place(*id).filter(|clip| clip.link.is_some()) {
                clip.link = None;
            }
        }
        Ok(())
    }

    /// A link ID no clip uses.
    fn next_link_id(&self) -> LinkId {
        let last = (self.tracks.values().flat_map(|track| track.all_clips()))
            .filter_map(|clip| clip.link)
            .max()
            .map_or(0, |link| link.0.get());
        LinkId(Id::new(last + 1).expect("one more than a raw ID is not zero"))
    }

    /// The clip with this ID, to change in place.
    pub(crate) fn clip_in_place(&mut self, id: ClipId) -> Option<&mut Clip> {
        let track = *self.index.clip_track.get(&id)?;
        Shared::make_mut(self.tracks.get_mut(&track)?).clip_mut(id)
    }

    /// Makes every link as R4 says after a command that turned `before`
    /// into this document: the clips whose contents the command changed give
    /// them to the rest of their link, a link left with fewer than two clips
    /// of one kind ends (R8, R9), and the rest of the document is left
    /// alone. Only the links of clips the command touched are looked at.
    pub(crate) fn sync_links(&mut self, before: &Self) {
        let touched = self.touched_links(before);
        if touched.is_empty() {
            return;
        }
        let mut members: BTreeMap<LinkId, Vec<ClipId>> = BTreeMap::new();
        for clip in self.tracks.values().flat_map(|track| track.all_clips()) {
            if let Some(link) = clip.link.filter(|link| touched.contains(link)) {
                members.entry(link).or_default().push(clip.id);
            }
        }
        for link in touched {
            let mut ids = members.remove(&link).unwrap_or_default();
            ids.sort();
            self.sync_link(before, link, &ids);
        }
    }

    /// The links of the clips a command added, removed or changed.
    fn touched_links(&self, before: &Self) -> BTreeSet<LinkId> {
        let mut touched = BTreeSet::new();
        for (id, track) in &self.tracks {
            if before
                .tracks
                .get(id)
                .is_some_and(|old| Shared::ptr_eq(old, track))
            {
                continue;
            }
            for clip in track.all_clips() {
                let old = before.clip(clip.id);
                if old != Some(clip) {
                    touched.extend(clip.link);
                    touched.extend(old.and_then(|old| old.link));
                }
            }
        }
        for (id, track) in &before.tracks {
            if self
                .tracks
                .get(id)
                .is_some_and(|new| Shared::ptr_eq(new, track))
            {
                continue;
            }
            for clip in track.all_clips() {
                if self.clip(clip.id).is_none() {
                    touched.extend(clip.link);
                }
            }
        }
        touched
    }

    fn sync_link(&mut self, before: &Self, link: LinkId, ids: &[ClipId]) {
        // A link holds clips of one kind: the first clip's says which.
        let audio = ids
            .first()
            .and_then(|id| self.clip(*id))
            .map(|clip| clip.data.is_audio());
        let ids: Vec<ClipId> = ids
            .iter()
            .copied()
            .filter(|id| self.clip(*id).map(|clip| clip.data.is_audio()) == audio)
            .collect();
        if ids.len() < 2 {
            for id in self.link_members(link) {
                if let Some(clip) = self.clip_in_place(id) {
                    clip.link = None;
                }
            }
            return;
        }
        // The clip whose contents the others take: one the command changed,
        // else one that was in the link already, else the first.
        let was_linked = |id: ClipId| before.clip(id).filter(|old| old.link == Some(link));
        let source = ids
            .iter()
            .copied()
            .find(|id| {
                was_linked(*id).is_some_and(|old| {
                    self.clip(*id)
                        .is_some_and(|clip| !clip.shares_contents_with(old))
                })
            })
            .or_else(|| ids.iter().copied().find(|id| was_linked(*id).is_some()))
            .unwrap_or(ids[0]);
        let Some(source) = self.clip(source).cloned() else {
            return;
        };
        let contents = source.contents();
        for id in ids {
            let Some(clip) = self.clip(id) else { continue };
            let tempo = self.settings.tempo.bpm_at(clip.start);
            let mut next = clip.clone();
            // An audio clip's markers are seconds in its file or ticks of
            // its contents, as its Warp switch says: a switch that changed
            // takes the clip's own markers across the warp map they were
            // in: the shared one when Warp goes on, its own when it goes off.
            if let (ClipData::Audio(own), ClipData::Audio(shared)) =
                (&mut next.data, &contents.data)
                && own.warp != shared.warp
            {
                let mut mapped = own.clone();
                if shared.warp {
                    mapped.warp_markers = shared.warp_markers.clone();
                    mapped.tempo = shared.tempo;
                }
                if let Ok(converted) = mapped.with_warp(shared.warp, tempo) {
                    *own = converted;
                }
            }
            // The source takes its own contents back too, which puts its
            // envelopes in the order a saved set gives them.
            let next = next.with_contents(&contents);
            if next != *clip
                && let Some(clip) = self.clip_in_place(id)
            {
                *clip = next;
            }
        }
    }
}

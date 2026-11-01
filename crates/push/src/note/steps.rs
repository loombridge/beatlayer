//! Step sequencing: the step pads, the loop length pads and their pages
//! (`docs/specs/push-2-notes.md`, R20 to R30).

use bl_control::ReadModel;
use bl_model::ClipId;
use serde_json::json;

use super::{DOUBLE_TAP, Down, Frame, GESTURE, Note, Pending, Source, WAIT, clip, grid};
use crate::content::{Ctx, HeldStep};
use crate::controls::cc;

/// The velocity a new step takes when none was chosen (R22).
pub(super) const VELOCITY: u8 = 100;

impl Note {
    /// A step pad: adds a note at the step, or deletes the one there, or with
    /// Mute held deactivates it (R20, R22).
    pub(super) fn step_pad(
        &mut self,
        frame: &Frame,
        index: u8,
        ctx: &mut Ctx<'_>,
        model: &ReadModel,
    ) -> Down {
        let Some(key) = frame.selected else {
            return Down::Done;
        };
        let start = frame.pages.step_start(self.pair, index);
        // Spec: push-2-notes R54. Accent overrides the velocity pads' choice.
        let velocity = self.accented(self.velocity.unwrap_or(VELOCITY));
        self.toggle_note(frame, key, start, velocity, ctx, model)
    }

    /// Adds a note of `key` at the step starting at `start`, or deletes the
    /// one there, or with Mute held deactivates it (R20, R22, R42).
    pub(super) fn toggle_note(
        &mut self,
        frame: &Frame,
        key: u8,
        start: bl_core::time::Ticks,
        velocity: u8,
        ctx: &mut Ctx<'_>,
        model: &ReadModel,
    ) -> Down {
        let length = frame.size.ticks();
        let step = HeldStep {
            clip: frame.clip,
            key,
            start,
            length,
        };
        match frame
            .clip
            .and_then(|id| clip::midi(model, id).map(|(_, midi)| (id, midi)))
        {
            Some((id, midi)) => {
                let places = clip::places(midi, key, start, start + length);
                if ctx.modifiers.has(cc::MUTE) {
                    if !places.is_empty() {
                        ctx.cx
                            .run("note.deactivate", json!({ "clip": id, "notes": places }));
                    }
                } else if places.is_empty() {
                    ctx.cx.run(
                        "note.add",
                        json!({
                            "clip": id,
                            "key": key,
                            "start": start,
                            "length": length,
                            "velocity": velocity,
                        }),
                    );
                } else {
                    ctx.cx
                        .run("note.delete", json!({ "clip": id, "notes": places }));
                }
            }
            None => {
                // The first step of an empty slot makes a one-bar clip with
                // the note and launches it, as one undo step (R20). The clip
                // exists from the next tick, which adds the note.
                if let Some(scene) = frame.scene
                    && model.document.slot(frame.track, scene).is_none()
                    && !ctx.modifiers.has(cc::MUTE)
                {
                    self.first_step(frame, scene, 1u128 << key, start, length, velocity, ctx);
                }
            }
        }
        Down::Step(step)
    }

    /// A loop length pad (R26 to R30).
    pub(super) fn loop_pad(
        &mut self,
        frame: &Frame,
        page: u8,
        ctx: &mut Ctx<'_>,
        model: &ReadModel,
    ) -> Down {
        let Some((id, clip, midi)) = frame
            .clip
            .and_then(|id| clip::midi(model, id).map(|(clip, midi)| (id, clip, midi)))
        else {
            return Down::Done;
        };
        let pages = frame.pages;
        let (from, to) = (pages.page_start(page), pages.page_start(page + 1));
        if ctx.modifiers.has(cc::DUPLICATE) {
            match self.source.take() {
                Some(Source::Page(first)) if first != page => {
                    let before = pages.page_start(first);
                    let adds: Vec<_> = midi
                        .notes
                        .iter()
                        .filter(|n| n.start >= before && n.start < pages.page_start(first + 1))
                        .map(|n| {
                            (
                                "note.add",
                                json!({
                                    "clip": id,
                                    "key": n.key,
                                    "start": n.start - before + from,
                                    "length": n.length,
                                    "velocity": n.velocity,
                                }),
                            )
                        })
                        .collect();
                    Self::group(ctx.cx, adds);
                }
                _ => self.source = Some(Source::Page(page)),
            }
            return Down::Done;
        }
        if ctx.modifiers.has(cc::DELETE) {
            let places = clip::places_in(midi, from, to);
            if !places.is_empty() {
                ctx.cx
                    .run("note.delete", json!({ "clip": id, "notes": places }));
            }
            return Down::Done;
        }
        let held = self.down.values().find_map(|down| match down {
            Down::Loop(first) => Some(*first),
            _ => None,
        });
        let double = self.last_loop_tap.is_some_and(|(last, at)| {
            last == page && ctx.at.saturating_duration_since(at) <= DOUBLE_TAP
        });
        self.last_loop_tap = Some((page, ctx.at));
        let (low, high) = clip::region(clip, midi);
        let outside = from >= high || to <= low;
        if let Some(first) = held {
            self.set_loop(id, pages, first.min(page), first.max(page), ctx);
        } else if double || outside {
            self.set_loop(id, pages, page, page, ctx);
        } else {
            // A tap on a page of the loop shows it and stops following
            // (R28).
            self.pair = pages.view_of(page);
            self.following = false;
        }
        Down::Loop(page)
    }

    /// Sets the loop from page `first`'s start to page `last`'s end, and
    /// turns it on, as one undo step (R27).
    fn set_loop(
        &mut self,
        clip: ClipId,
        pages: grid::Pages,
        first: u8,
        last: u8,
        ctx: &mut Ctx<'_>,
    ) {
        Self::group(
            ctx.cx,
            vec![
                (
                    "clip.set_midi_loop",
                    json!({
                        "clip": clip,
                        "start": pages.page_start(first),
                        "end": pages.page_start(last + 1),
                    }),
                ),
                (
                    "clip.set_loop_enabled",
                    json!({ "clip": clip, "enabled": true }),
                ),
            ],
        );
        self.pair = pages.view_of(first);
        self.following = true;
    }

    /// What each tick does: finishes the first step of an empty slot once
    /// its clip exists, and keeps the step area on the page that plays (R20,
    /// R28).
    pub(super) fn tick_steps(&mut self, ctx: &mut Ctx<'_>) {
        // The pads' pressure comes as polyphonic key pressure (R6).
        ctx.set_aftertouch(true);
        let Some(model) = ctx.model else {
            return;
        };
        let Some(frame) = self.frame(model) else {
            self.pending = None;
            self.summary = super::State::default();
            self.reduced = 0;
            return;
        };
        if let Some(mut pending) = self.pending.take() {
            if let Some(clip) = clip::midi_clip(model, pending.track, pending.scene) {
                self.finish_first_step(pending, clip, ctx);
            } else if pending.waited >= WAIT {
                ctx.cx.end_gesture(GESTURE);
            } else {
                pending.waited += 1;
                self.pending = Some(pending);
            }
        }
        if self.following
            && let Some(id) = frame.clip
            && let Some(scene) = frame.scene
            && let Some((c, midi)) = clip::midi(model, id)
            && let Some(at) = clip::playhead(model, frame.track, scene, c, midi)
        {
            let pages = frame.pages;
            let page =
                (at.get() / pages.page().get().max(1)).clamp(0, i64::from(pages.count()) - 1);
            self.pair = pages.view_of(u8::try_from(page).unwrap_or(0));
        }
        self.summarise(&frame);
    }

    fn finish_first_step(&mut self, pending: Pending, clip: ClipId, ctx: &mut Ctx<'_>) {
        let Pending {
            track,
            scene,
            keys,
            start,
            length,
            velocity,
            ..
        } = pending;
        let mut commands = vec![("session.select", json!({ "track": track, "scene": scene }))];
        commands.extend(super::keys::keys_of(keys).map(|key| {
            (
                "note.add",
                json!({
                    "clip": clip,
                    "key": key,
                    "start": start,
                    "length": length,
                    "velocity": velocity,
                }),
            )
        }));
        commands.push((
            "clip.set_loop_enabled",
            json!({ "clip": clip, "enabled": true }),
        ));
        commands.push(("session.launch", json!({ "track": track, "scene": scene })));
        Self::group(ctx.cx, commands);
    }
}

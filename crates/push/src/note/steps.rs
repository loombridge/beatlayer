//! Step sequencing: the step pads, the loop length pads and their pages
//! (`docs/specs/push-2-notes.md`, R20 to R30).

use bl_control::ReadModel;
use bl_model::ClipId;
use serde_json::json;

use super::{DOUBLE_TAP, Down, Frame, GESTURE, Note, Pending, Source, WAIT, clip, grid};
use crate::content::{Ctx, HeldStep};
use crate::controls::cc;

/// The velocity a new step takes when none was chosen (R22).
const VELOCITY: u8 = 100;

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
        let start = frame.size.step_start(self.pair, index);
        let length = frame.size.ticks();
        let step = HeldStep {
            clip: frame.clip,
            key,
            start,
            length,
        };
        let velocity = self.velocity.unwrap_or(VELOCITY);
        match frame.clip.and_then(|id| clip::midi(model, id).map(|(_, midi)| (id, midi))) {
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
                    && self.pending.is_none()
                {
                    ctx.cx.run_held(
                        GESTURE,
                        "session.insert_empty_midi_clip",
                        json!({ "track": frame.track, "scene": scene }),
                    );
                    self.pending = Some(Pending {
                        track: frame.track,
                        scene,
                        key,
                        start,
                        length,
                        velocity,
                        waited: 0,
                    });
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
        let size = frame.size;
        let (from, to) = (size.page_start(page), size.page_start(page + 1));
        if ctx.modifiers.has(cc::DUPLICATE) {
            match self.source.take() {
                Some(Source::Page(first)) if first != page => {
                    let before = size.page_start(first);
                    let adds: Vec<_> = midi
                        .notes
                        .iter()
                        .filter(|n| n.start >= before && n.start < size.page_start(first + 1))
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
        let double = self
            .last_loop_tap
            .is_some_and(|(last, at)| last == page && ctx.at.saturating_duration_since(at) <= DOUBLE_TAP);
        self.last_loop_tap = Some((page, ctx.at));
        let (low, high) = clip::region(clip, midi);
        let outside = from >= high || to <= low;
        if let Some(first) = held {
            self.set_loop(id, size, first.min(page), first.max(page), ctx);
        } else if double || outside {
            self.set_loop(id, size, page, page, ctx);
        } else {
            // A tap on a page of the loop shows it and stops following
            // (R28).
            self.pair = page / 2;
            self.following = false;
        }
        Down::Loop(page)
    }

    /// Sets the loop from page `first`'s start to page `last`'s end, and
    /// turns it on, as one undo step (R27).
    fn set_loop(
        &mut self,
        clip: ClipId,
        size: grid::Size,
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
                        "start": size.page_start(first),
                        "end": size.page_start(last + 1),
                    }),
                ),
                ("clip.set_loop_enabled", json!({ "clip": clip, "enabled": true })),
            ],
        );
        self.pair = first / 2;
        self.following = true;
    }

    /// What each tick does: finishes the first step of an empty slot once
    /// its clip exists, and keeps the step area on the page that plays (R20,
    /// R28).
    pub(super) fn tick_steps(&mut self, ctx: &mut Ctx<'_>) {
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
            let page = (at.get() / frame.size.page().get().max(1)).clamp(0, i64::from(grid::PAGES) - 1);
            self.pair = u8::try_from(page / 2).unwrap_or(0);
        }
        self.summarise(&frame);
    }

    fn finish_first_step(&mut self, pending: Pending, clip: ClipId, ctx: &mut Ctx<'_>) {
        let Pending {
            track,
            scene,
            key,
            start,
            length,
            velocity,
            ..
        } = pending;
        Self::group(
            ctx.cx,
            vec![
                ("session.select", json!({ "track": track, "scene": scene })),
                (
                    "note.add",
                    json!({
                        "clip": clip,
                        "key": key,
                        "start": start,
                        "length": length,
                        "velocity": velocity,
                    }),
                ),
                ("clip.set_loop_enabled", json!({ "clip": clip, "enabled": true })),
                ("session.launch", json!({ "track": track, "scene": scene })),
            ],
        );
    }
}

/// Whether `page` of the loop length pads is in view: one of the two pages
/// the step area shows (R29).
pub(super) const fn in_view(pair: u8, page: u8) -> bool {
    page / 2 == pair
}

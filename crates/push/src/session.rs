//! Session Mode: the pads and the Scene/Grid buttons as the session ring's
//! slots and scenes, and Session Overview
//! (`docs/specs/push-2-tracks.md`, R59 to R73).
//!
//! Stop Clip works in every mode, so the surface itself handles it; the
//! pad mode's content here does the rest.

use std::collections::{BTreeMap, BTreeSet};

use bl_control::{Moves, ReadModel, RingView, SlotState};
use bl_model::{ClipId, SceneId, TrackId};
use serde_json::json;

use crate::content::{Content, Ctx, Gesture, Pad, PadEvent, Paint, Target};
use crate::controls::{self, cc};
use crate::leds::{Led, fixed, white};
use crate::surface::dim;

/// How many tracks and scenes the ring covers.
const SIDE: usize = 8;
/// How many scenes one of the Scene/Grid buttons stands for in Session
/// Overview (R70).
const PART: usize = SIDE * SIDE;

/// What a pad or a Scene/Grid button shows, for `push.state` (R73).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Look {
    /// In the slots: `none` (no track or scene), `empty`, `stopped`,
    /// `queued`, `playing`, `stopping` or `recording`. In Session Overview:
    /// `none`, `ring`, `playing` or `block`. On a scene button: `none`,
    /// `empty` or `clip`; in the overview, `none`, `part` or `shown`.
    pub what: &'static str,
    /// The clip in the slot.
    pub clip: Option<ClipId>,
    /// The swatch of the clip's colour, or of the scene's.
    pub colour: Option<u8>,
}

impl Look {
    fn of(what: &'static str) -> Self {
        Self {
            what,
            ..Self::default()
        }
    }
}

/// What Session Mode shows, for `push.state` (R73).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// The ring's first track.
    pub first_track: Option<TrackId>,
    /// The ring's first scene.
    pub first_scene: Option<SceneId>,
    /// How many tracks the ring covers now.
    pub tracks: usize,
    /// How many scenes it covers now.
    pub scenes: usize,
    /// Whether Session Overview is shown.
    pub overview: bool,
    /// Whether Session Overview is locked on.
    pub locked: bool,
    /// The part of 64 scenes the overview shows, from 0.
    pub part: usize,
    /// The 64 pads, the top row first and each row from the left.
    pub pads: Vec<Look>,
    /// The eight Scene/Grid buttons, top to bottom.
    pub scene_buttons: Vec<Look>,
}

/// The slot at `pad` of `ring`, if the ring covers one there: its track in
/// the pad's column and its scene in the pad's row from the top (R60).
fn slot_at(ring: &RingView, pad: Pad) -> Option<(TrackId, SceneId)> {
    Some((
        *ring.tracks.get(usize::from(pad.column))?,
        *ring.scenes.get(SIDE - 1 - usize::from(pad.row))?,
    ))
}

/// The ring row, from the top, of Scene/Grid button `button`.
const fn scene_row(button: u8) -> Option<usize> {
    if button >= cc::SCENE_8 && button <= cc::SCENE_1 {
        Some((cc::SCENE_1 - button) as usize)
    } else {
        None
    }
}

/// Session Mode's content.
#[derive(Default)]
pub struct Session {
    /// Layout is held and Session Overview shows (R69).
    held: bool,
    /// Session Overview is locked on (R69).
    locked: bool,
    /// The press of Layout that unlocked the overview doesn't show it again
    /// while it is held.
    unlocked: bool,
    /// The part of 64 scenes the overview shows (R70).
    part: usize,
    /// The slots whose pad is down and launched, for the release (R61).
    down: BTreeMap<(u8, u8), (TrackId, SceneId)>,
    /// The clip Duplicate copies, once its first pad was pressed (R66).
    copy: Option<ClipId>,
    state: Option<State>,
}

impl Session {
    /// Session Mode before the first pad.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn overview(&self) -> bool {
        self.held || self.locked
    }

    fn pressed(&mut self, pad: Pad, ctx: &mut Ctx<'_>) {
        let Some(model) = ctx.model else {
            return;
        };
        let Some(ring) = model.row(ctx.cx.row()).and_then(|row| row.ring.as_ref()) else {
            return;
        };
        if self.overview() {
            Self::block(model, self.part, pad, ctx);
            return;
        }
        let Some((track, scene)) = slot_at(ring, pad) else {
            return;
        };
        let doc = &model.document;
        let clip = doc.slot(track, scene);
        let modifiers = ctx.modifiers;
        if modifiers.shift() {
            // The colour picker for the clip (R64).
            if let Some(clip) = clip {
                let own = clip
                    .colour
                    .or_else(|| doc.track(track).map(bl_model::Track::colour));
                ctx.open_picker(Target::Command {
                    command: "clip.set_colour".to_owned(),
                    args: json!({ "clips": [clip.id] }),
                    current: own,
                });
            }
        } else if modifiers.has(cc::SELECT) {
            // Selects the slot and shows the clip's name (R65).
            ctx.cx
                .run("session.select", json!({ "track": track, "scene": scene }));
            if let Some(clip) = clip.filter(|clip| !clip.name.is_empty()) {
                ctx.notify(clip.name.clone());
            }
        } else if modifiers.has(cc::DELETE) {
            if clip.is_some() {
                ctx.cx
                    .run("session.select", json!({ "track": track, "scene": scene }));
                ctx.cx.run("session.delete", json!({}));
            }
        } else if modifiers.has(cc::DUPLICATE) {
            // The first pad with a clip is the one copied, each pad after it
            // a place to copy it to (R66).
            match (self.copy, clip) {
                (None, Some(clip)) => self.copy = Some(clip.id),
                (Some(from), clip) if clip.map(|c| c.id) != Some(from) => ctx.cx.run(
                    "session.copy_clip_to_slot",
                    json!({ "clip": from, "track": track, "scene": scene }),
                ),
                _ => {}
            }
        } else if clip.is_some() {
            ctx.cx
                .run("session.launch", json!({ "track": track, "scene": scene }));
            self.down.insert((pad.row, pad.column), (track, scene));
        } else if let Some(t) = doc.track(track) {
            // An empty slot records on an armed track, and stops the track
            // where it has a stop button (R61).
            if t.mixer().arm {
                ctx.cx.run(
                    "session.record_slot",
                    json!({ "track": track, "scene": scene }),
                );
            } else if t.has_stop_button(scene) {
                ctx.cx.run("session.stop_track", json!({ "track": track }));
            }
        }
    }

    /// A pad of Session Overview moves the ring to its block (R70).
    fn block(model: &ReadModel, part: usize, pad: Pad, ctx: &mut Ctx<'_>) {
        let (r, c) = (SIDE - 1 - usize::from(pad.row), usize::from(pad.column));
        let track = model.columns.get(c * SIDE).copied();
        let scene = model
            .document
            .scenes()
            .nth(part * PART + r * SIDE)
            .map(|s| s.id);
        if let (Some(track), Some(scene)) = (track, scene) {
            ctx.cx.run(
                "control_surface.move_ring",
                json!({ "row": ctx.cx.row() + 1, "track": track, "scene": scene }),
            );
        }
    }

    fn scene_button(&mut self, row: usize, ctx: &mut Ctx<'_>) {
        let Some(model) = ctx.model else {
            return;
        };
        if self.overview() {
            // The k-th button shows the k-th part of 64 scenes, where the
            // set has it (R70).
            if row * PART < model.document.scenes().len() || row == 0 {
                self.part = row;
            }
            return;
        }
        let scene = model
            .row(ctx.cx.row())
            .and_then(|r| r.ring.as_ref())
            .and_then(|ring| ring.scenes.get(row))
            .copied();
        let Some(scene) = scene else {
            return;
        };
        let id = if ctx.modifiers.has(cc::SELECT) {
            "session.select"
        } else {
            "session.launch_scene"
        };
        ctx.cx.run(id, json!({ "scene": scene }));
    }

    /// An arrow, Octave or Page button moves the ring (R67): one step, or
    /// eight with Shift or on Octave and Page.
    fn arrow(button: u8, ctx: &mut Ctx<'_>) {
        let Some(moves) = ctx
            .model
            .and_then(|m| m.row(ctx.cx.row()))
            .and_then(|r| r.ring.as_ref())
            .map(|ring| ring.moves)
        else {
            return;
        };
        let shift = ctx.modifiers.shift();
        let (step, can) = match button {
            cc::UP => (if shift { "page_up" } else { "up" }, moves.up),
            cc::DOWN => (if shift { "page_down" } else { "down" }, moves.down),
            cc::LEFT => (if shift { "page_left" } else { "left" }, moves.left),
            cc::RIGHT => (if shift { "page_right" } else { "right" }, moves.right),
            cc::OCTAVE_UP => ("page_up", moves.up),
            cc::OCTAVE_DOWN => ("page_down", moves.down),
            cc::PAGE_LEFT => ("page_left", moves.left),
            _ => ("page_right", moves.right),
        };
        if can {
            ctx.cx.run(
                "control_surface.move_ring",
                json!({ "row": ctx.cx.row() + 1, "step": step }),
            );
        }
    }

    fn layout(&mut self, gesture: Gesture, ctx: &Ctx<'_>) {
        match gesture {
            // Shift with Layout locks the overview on; Layout again unlocks
            // it (R69).
            Gesture::Press if ctx.modifiers.shift() => self.locked = true,
            Gesture::Press if self.locked => {
                self.locked = false;
                self.unlocked = true;
            }
            Gesture::Hold if !self.unlocked => self.held = true,
            Gesture::Release => {
                self.held = false;
                self.unlocked = false;
            }
            _ => {}
        }
    }

    fn paint_slots(ring: &RingView, paint: &mut Paint<'_>, state: &mut State) {
        let model = paint.model;
        let doc = &model.document;
        for row in 0..SIDE {
            for column in 0..SIDE {
                let slot = ring.tracks.get(column).zip(ring.scenes.get(row));
                let mut look = Look::of("none");
                let mut led = Led::default();
                if let Some((&track, &scene)) = slot {
                    let clip = doc.slot(track, scene);
                    let status = model
                        .slots
                        .iter()
                        .find(|s| s.track == track && s.scene == scene)
                        .map(|s| s.state);
                    if clip.is_none() && status != Some(SlotState::Recording) {
                        // Dark, an empty slot on an armed track included
                        // (R63).
                        look = Look::of("empty");
                    } else {
                        let swatch = clip
                            .and_then(|c| c.colour)
                            .or_else(|| doc.track(track).map(bl_model::Track::colour))
                            .unwrap_or(0);
                        let rgb = bl_model::swatch_rgb(paint.colours, swatch);
                        let full = paint.lights.colour(rgb);
                        let (what, shown) = match status {
                            Some(SlotState::Playing) => {
                                let dimmed = paint.lights.colour(dim(rgb));
                                ("playing", Led::pulsing(full, dimmed))
                            }
                            Some(SlotState::Recording) => {
                                ("recording", Led::pulsing(full, fixed::RED))
                            }
                            Some(SlotState::Queued) => ("queued", Led::steady(full)),
                            Some(SlotState::QueuedToStop) => ("stopping", Led::steady(full)),
                            None => ("stopped", Led::steady(full)),
                        };
                        look = Look {
                            what,
                            clip: clip.map(|c| c.id),
                            colour: Some(swatch),
                        };
                        led = shown;
                    }
                }
                paint.lights.pad((SIDE - 1 - row) as u8, column as u8, led);
                state.pads.push(look);
            }
        }
        // A Scene/Grid button is lit while its scene holds a clip: in the
        // scene's colour, white when it has none (R63).
        for row in 0..SIDE {
            let scene = ring.scenes.get(row).copied();
            let mut look = Look::of("none");
            let mut led = Led::default();
            if let Some(scene) = scene {
                look = Look::of("empty");
                if model.columns.iter().any(|&t| doc.slot(t, scene).is_some()) {
                    let own = doc.scenes().find(|s| s.id == scene).and_then(|s| s.colour);
                    look = Look {
                        what: "clip",
                        clip: None,
                        colour: own,
                    };
                    led = Led::steady(match own {
                        Some(swatch) => paint
                            .lights
                            .colour(bl_model::swatch_rgb(paint.colours, swatch)),
                        None => fixed::WHITE,
                    });
                }
            }
            paint.lights.button(controls::scene(row as u8), led);
            state.scene_buttons.push(look);
        }
    }

    fn paint_overview(&mut self, ring: &RingView, paint: &mut Paint<'_>, state: &mut State) {
        let model = paint.model;
        let doc = &model.document;
        let scenes: Vec<SceneId> = doc.scenes().map(|s| s.id).collect();
        let parts = scenes.len().div_ceil(PART).max(1);
        self.part = self.part.min(parts - 1);
        let part = self.part;
        let track_at = |track| model.columns.iter().position(|&t| t == track);
        let scene_at = |scene| scenes.iter().position(|&s| s == scene);
        // The block of a place, if it is in this part.
        let block_of = |track: Option<usize>, scene: Option<usize>| match (track, scene) {
            (Some(t), Some(s)) if s / PART == part => Some(((s % PART) / SIDE, t / SIDE)),
            _ => None,
        };
        let first = block_of(
            ring.first_track.and_then(track_at),
            ring.first_scene.and_then(scene_at),
        );
        let playing: BTreeSet<(usize, usize)> = model
            .slots
            .iter()
            .filter(|s| matches!(s.state, SlotState::Playing | SlotState::QueuedToStop))
            .filter_map(|s| block_of(track_at(s.track), scene_at(s.scene)))
            .collect();
        for row in 0..SIDE {
            for column in 0..SIDE {
                let exists =
                    column * SIDE < model.columns.len() && part * PART + row * SIDE < scenes.len();
                let (what, led) = if !exists {
                    ("none", Led::default())
                } else if first == Some((row, column)) {
                    ("ring", Led::steady(fixed::WHITE))
                } else if playing.contains(&(row, column)) {
                    ("playing", Led::steady(fixed::GREEN))
                } else {
                    ("block", Led::steady(white::DIM))
                };
                paint.lights.pad((SIDE - 1 - row) as u8, column as u8, led);
                state.pads.push(Look::of(what));
            }
        }
        // The buttons are the parts of 64 scenes (R70).
        for row in 0..SIDE {
            let (what, led) = if row >= parts {
                ("none", Led::default())
            } else if row == part {
                ("shown", Led::steady(fixed::WHITE))
            } else {
                ("part", Led::steady(white::DIM))
            };
            paint.lights.button(controls::scene(row as u8), led);
            state.scene_buttons.push(Look::of(what));
        }
        state.part = part;
    }
}

impl Content for Session {
    fn name(&self) -> &str {
        "Session"
    }

    fn button(&mut self, button: u8, gesture: Gesture, ctx: &mut Ctx<'_>) {
        if button == cc::LAYOUT {
            self.layout(gesture, ctx);
        } else if gesture == Gesture::Press {
            if let Some(row) = scene_row(button) {
                self.scene_button(row, ctx);
            } else if matches!(
                button,
                cc::UP
                    | cc::DOWN
                    | cc::LEFT
                    | cc::RIGHT
                    | cc::OCTAVE_UP
                    | cc::OCTAVE_DOWN
                    | cc::PAGE_LEFT
                    | cc::PAGE_RIGHT
            ) {
                Self::arrow(button, ctx);
            }
        }
    }

    fn pad(&mut self, pad: Pad, event: PadEvent, ctx: &mut Ctx<'_>) {
        match event {
            PadEvent::On(_) => self.pressed(pad, ctx),
            // The release a Gate or Repeat clip needs (R61), of the slot the
            // press launched even if the ring has moved since.
            PadEvent::Off => {
                if let Some((track, scene)) = self.down.remove(&(pad.row, pad.column)) {
                    ctx.cx
                        .run("session.release", json!({ "track": track, "scene": scene }));
                }
            }
            PadEvent::Pressure(_) | PadEvent::ChannelPressure(_) => {}
        }
    }

    fn paint(&mut self, paint: &mut Paint<'_>) {
        if !paint.modifiers.has(cc::DUPLICATE) {
            self.copy = None;
        }
        let Some(ring) = paint
            .model
            .row(paint.row)
            .and_then(|row| row.ring.as_ref())
            .cloned()
        else {
            self.state = None;
            return;
        };
        let mut state = State {
            first_track: ring.first_track,
            first_scene: ring.first_scene,
            tracks: ring.tracks.len(),
            scenes: ring.scenes.len(),
            overview: self.overview(),
            locked: self.locked,
            ..State::default()
        };
        if self.overview() {
            self.paint_overview(&ring, paint, &mut state);
        } else {
            Self::paint_slots(&ring, paint, &mut state);
        }
        let Moves {
            left,
            right,
            up,
            down,
        } = ring.moves;
        for (button, can) in [
            (cc::LEFT, left),
            (cc::RIGHT, right),
            (cc::UP, up),
            (cc::DOWN, down),
            (cc::PAGE_LEFT, left),
            (cc::PAGE_RIGHT, right),
            (cc::OCTAVE_UP, up),
            (cc::OCTAVE_DOWN, down),
        ] {
            paint.lights.white(button, can, false);
        }
        paint.lights.white(cc::LAYOUT, true, self.overview());
        self.state = Some(state);
    }

    fn session_state(&self) -> Option<State> {
        self.state.clone()
    }
}

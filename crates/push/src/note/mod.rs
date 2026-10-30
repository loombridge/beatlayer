//! Note Mode on a track: the drum layouts, the pads of a drum rack and step
//! sequencing beats (`docs/specs/push-2-notes.md`, R1 to R5 and R8 to R31).
//!
//! [`Note`] is the content of the Note pad mode. Which layout the pads take
//! comes from the selected track's first instrument every time it is read, so
//! changing the instrument changes the layout at once (R1). What the surface
//! keeps for each track (R5) is its [`Memory`]; everything the pads show or
//! do reads the set from the read model and asks for the registered commands
//! a person's editors run.

mod clip;
pub mod grid;
mod pads;
mod paint;
mod steps;

use std::collections::BTreeMap;
use std::time::Instant;

use bl_control::{Cx, ReadModel};
use bl_core::time::Ticks;
use bl_model::{ClipId, DeviceId, SceneId, TrackId};
use serde_json::{Value, json};

use crate::content::{Content, Ctx, Gesture, HeldStep, Pad, PadEvent, Paint, StripEvent, StripUse};
use crate::controls::cc;

pub use grid::{Layout, Overlay, Role, Size};

/// The key of the gesture every step, pad and page edit runs in, so its
/// commands are one undo step (control surfaces R7).
const GESTURE: u32 = 0x6000;
/// How long a second tap on a loop length pad may follow the first and make a
/// double tap (R27).
const DOUBLE_TAP: std::time::Duration = std::time::Duration::from_millis(300);
/// How many ticks the surface waits for a clip it asked for to appear before
/// it lets go of the first step.
const WAIT: u8 = 25;

/// What the pads are for on the selected track (R1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A drum rack, and the notes its pad view shows.
    Rack {
        rack: DeviceId,
        first: u8,
        selected: Option<u8>,
        /// The notes whose pad holds a chain: bit n is note n.
        filled: u128,
    },
    /// A Simpler slicing a sample: its slices are the pads (R1, R75).
    Slices,
    /// Any other MIDI track: its layouts are WP-7.22's.
    Melodic,
    /// An audio track, a return or the Main track: the pads are dark (R1).
    Silent,
}

/// What the surface keeps for each track (R5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Memory {
    /// The layout, or the track's own default until Layout steps it.
    layout: Option<Layout>,
    /// The loop length pads Shift and Layout keep in view.
    lock: Option<Overlay>,
    /// The step size.
    size: Size,
}

/// What a pad that is down does until it is released.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Down {
    /// It plays a note, which its release ends whatever the layout has
    /// become (R2).
    Played(u8),
    /// It holds a step.
    Step(HeldStep),
    /// It holds a loop length pad.
    Loop(u8),
    /// It did something that ends with the press.
    Done,
}

/// What Duplicate copies from, after its first tap (R17, R30).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Pad(u8),
    Page(u8),
}

/// The first step on an empty slot, waiting for its clip (R20).
#[derive(Clone, Copy, Debug)]
struct Pending {
    track: TrackId,
    scene: SceneId,
    key: u8,
    start: Ticks,
    length: Ticks,
    velocity: u8,
    waited: u8,
}

/// What Note Mode shows, for `push.state` (R5 to R31).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// The track the pads play.
    pub track: Option<TrackId>,
    /// The layout: a drum layout's name, `Melodic` or `None`.
    pub layout: String,
    /// What holding Layout, or Shift and Layout, shows in the lower-right
    /// pads, if anything.
    pub overlay: Option<String>,
    /// Whether that stays after Layout is released.
    pub overlay_locked: bool,
    /// The first note of the drum bank.
    pub bank: Option<u8>,
    /// The pad the steps edit.
    pub selected_pad: Option<u8>,
    /// The step size, such as `1/16`.
    pub step_size: String,
    /// The clip the steps edit.
    pub edited_clip: Option<ClipId>,
    /// The scene whose slot holds it.
    pub edited_scene: Option<SceneId>,
    /// The first page of the two the step area shows, from 1.
    pub page: u8,
    /// Whether the step area follows the page that plays.
    pub following: bool,
    /// The velocity new steps take, if a velocity pad chose one.
    pub velocity: Option<u8>,
    /// The steps held now.
    pub held_steps: Vec<HeldStep>,
}

/// The content of Note Mode.
pub struct Note {
    memory: BTreeMap<TrackId, Memory>,
    /// The track the pads played last.
    track: Option<TrackId>,
    /// The scene the steps edit, kept while no slot or scene is selected.
    scene: Option<SceneId>,
    /// Which pair of pages the step area shows, from 0 (R26).
    pair: u8,
    /// Whether the step area follows the page that plays (R28).
    following: bool,
    /// The velocity new steps take (R10).
    velocity: Option<u8>,
    /// What holding Layout shows (R9).
    held: Option<Overlay>,
    /// Whether the press of Layout now down kept a view, so its release
    /// doesn't step.
    layout_used: bool,
    down: BTreeMap<(u8, u8), Down>,
    /// The loop length pad tapped last, for a double tap (R27).
    last_loop_tap: Option<(u8, Instant)>,
    /// Duplicate's first tap (R17, R30).
    source: Option<Source>,
    /// Whether Shift was held when an Octave button went down.
    octave_shift: bool,
    pending: Option<Pending>,
    /// The pad each track's slicing Simpler's steps edit (R75).
    picked: BTreeMap<TrackId, u8>,
    summary: State,
    reduced: u64,
}

impl Default for Note {
    fn default() -> Self {
        Self::new()
    }
}

impl Note {
    /// The content of Note Mode, before any track is selected.
    #[must_use]
    pub fn new() -> Self {
        Self {
            memory: BTreeMap::new(),
            track: None,
            scene: None,
            pair: 0,
            following: true,
            velocity: None,
            held: None,
            layout_used: false,
            down: BTreeMap::new(),
            last_loop_tap: None,
            source: None,
            octave_shift: false,
            pending: None,
            picked: BTreeMap::new(),
            summary: State::default(),
            reduced: 0,
        }
    }

    fn memory(&mut self, track: TrackId) -> &mut Memory {
        self.memory.entry(track).or_default()
    }

    /// What the pads are now: the selected track, what it is, which scene's
    /// slot the steps edit (R1, R20).
    fn frame(&mut self, model: &ReadModel) -> Option<Frame> {
        let track = clip::selected_track(model)?;
        if self.track != Some(track) {
            // Selecting a track brings back its own memory (R5); the page
            // and following start afresh.
            self.track = Some(track);
            self.pair = 0;
            self.following = true;
            self.source = None;
            self.last_loop_tap = None;
        }
        let kind = clip::classify(model, track);
        if let Some(scene) = clip::selected_scene(model) {
            self.scene = Some(scene);
        }
        let scene = self
            .scene
            .filter(|scene| model.document.scene(*scene).is_some())
            .or_else(|| model.document.scenes().next().map(|scene| scene.id));
        let clip = scene.and_then(|scene| clip::midi_clip(model, track, scene));
        let memory = *self.memory(track);
        let layout = memory.layout.unwrap_or(match kind {
            Kind::Slices => Layout::Pads,
            _ => Layout::LoopSelector,
        });
        let selected = match &kind {
            Kind::Rack {
                first, selected, ..
            } => Some(selected.unwrap_or(*first)),
            Kind::Slices => Some(self.picked.get(&track).copied().unwrap_or(36)),
            _ => None,
        };
        Some(Frame {
            track,
            kind,
            scene,
            clip,
            layout,
            overlay: self.held.or(memory.lock),
            size: memory.size,
            selected,
        })
    }

    /// What the pad at `pad` is in `frame` (R8 to R11).
    fn role(frame: &Frame, row: u8, column: u8) -> Role {
        let first = match frame.kind {
            Kind::Rack { first, .. } => first,
            Kind::Slices => 36,
            Kind::Melodic | Kind::Silent => return Role::Off,
        };
        grid::role(frame.layout, frame.overlay, first, frame.size, row, column)
    }

    /// Runs `commands` as one undo step (control surfaces R7).
    fn group(cx: &mut Cx, commands: Vec<(&str, Value)>) {
        let count = commands.len();
        for (n, (id, args)) in commands.into_iter().enumerate() {
            if count == 1 {
                cx.run(id, args);
            } else if n + 1 == count {
                cx.run_released(GESTURE, id, args);
            } else {
                cx.run_held(GESTURE, id, args);
            }
        }
    }
}

/// What the pads and steps act on at one moment.
pub(crate) struct Frame {
    pub track: TrackId,
    pub kind: Kind,
    pub scene: Option<SceneId>,
    pub clip: Option<ClipId>,
    pub layout: Layout,
    pub overlay: Option<Overlay>,
    pub size: Size,
    /// The drum pad the steps edit.
    pub selected: Option<u8>,
}

impl Content for Note {
    fn name(&self) -> &str {
        "Note"
    }

    fn pad(&mut self, pad: Pad, event: PadEvent, ctx: &mut Ctx<'_>) {
        match event {
            PadEvent::On(velocity) => self.pad_on(pad, velocity, ctx),
            PadEvent::Off => self.pad_off(pad, ctx),
            // Pressure is WP-7.22's (R6).
            PadEvent::Pressure(_) | PadEvent::ChannelPressure(_) => {}
        }
    }

    fn button(&mut self, button: u8, gesture: Gesture, ctx: &mut Ctx<'_>) {
        self.button_event(button, gesture, ctx);
    }

    fn strip(&mut self, event: StripEvent, ctx: &mut Ctx<'_>) {
        self.strip_event(event, ctx);
    }

    fn tick(&mut self, ctx: &mut Ctx<'_>) {
        self.tick_steps(ctx);
    }

    fn paint(&mut self, paint: &mut Paint<'_>) {
        self.paint_pads(paint);
    }

    fn held_steps(&self) -> Vec<HeldStep> {
        self.down
            .values()
            .filter_map(|down| match down {
                Down::Step(step) => Some(*step),
                _ => None,
            })
            .collect()
    }

    fn strip_use(&self) -> Option<StripUse> {
        self.summary.bank.map(|_| StripUse::Range)
    }

    fn note_state(&self) -> Option<State> {
        Some(self.summary.clone())
    }

    fn reduced_pads(&self) -> u64 {
        self.reduced
    }
}

/// The pad's place as one number, row from the bottom times 8 plus column.
const fn bit(row: u8, column: u8) -> u64 {
    1 << (row * 8 + column)
}

impl Note {
    fn strip_event(&mut self, event: StripEvent, ctx: &mut Ctx<'_>) {
        let StripEvent::Position(position) = event else {
            return;
        };
        let Some(model) = ctx.model else {
            return;
        };
        let Some(frame) = self.frame(model) else {
            return;
        };
        let Kind::Rack { rack, .. } = frame.kind else {
            return;
        };
        // The strip chooses the bank by sixteen notes, or by a row of four
        // with Shift held (R13).
        let (parts, per) = if ctx.modifiers.shift() {
            (32.0, 4)
        } else {
            (8.0, 16)
        };
        let place = (position.clamp(0.0, 0.999) * parts) as u8;
        let first = (place * per).min(grid::LAST_FIRST_PAD);
        ctx.cx
            .run("rack.move_pads", json!({ "rack": rack, "first": first }));
    }

    fn button_event(&mut self, button: u8, gesture: Gesture, ctx: &mut Ctx<'_>) {
        let Some(model) = ctx.model else {
            return;
        };
        match (button, gesture) {
            (cc::LAYOUT, _) => self.layout_button(gesture, ctx),
            (cc::DELETE, Gesture::Tap) => {
                if let Some(frame) = self.frame(model)
                    && let Some(clip) = frame.clip
                {
                    ctx.cx.run("clip.delete", json!({ "clip": clip }));
                }
            }
            (55 | 54, Gesture::Press) => self.octave_shift = ctx.modifiers.shift(),
            // Shift makes it a combination, which has no tap.
            (55 | 54, Gesture::Tap) if !self.octave_shift => self.octave(button == 55, ctx),
            (55 | 54, Gesture::Release) if self.octave_shift => self.octave(button == 55, ctx),
            (62 | 63, Gesture::Tap) => self.page_button(button == 63, false),
            (62 | 63, Gesture::Hold) => self.page_button(button == 63, true),
            (number, Gesture::Press) => {
                // The scene buttons choose the step size (R21).
                if let Some(row) = (cc::SCENE_8..=cc::SCENE_1)
                    .contains(&number)
                    .then(|| cc::SCENE_1 - number)
                    && let Some(size) = Size::at(row)
                    && let Some(track) = self.frame(model).map(|f| f.track)
                {
                    self.memory(track).size = size;
                }
            }
            _ => {}
        }
    }

    fn layout_button(&mut self, gesture: Gesture, ctx: &mut Ctx<'_>) {
        let Some(model) = ctx.model else {
            return;
        };
        let Some(frame) = self.frame(model) else {
            return;
        };
        if matches!(frame.kind, Kind::Melodic | Kind::Silent) {
            return;
        }
        let track = frame.track;
        match gesture {
            Gesture::Press => {
                self.layout_used = false;
                // Shift and Layout keep the view Layout shows while held (R9).
                if ctx.modifiers.shift() {
                    let overlay = frame.layout.held();
                    let memory = self.memory(track);
                    memory.lock = if memory.lock.is_some() {
                        None
                    } else {
                        Some(overlay)
                    };
                    self.layout_used = true;
                }
            }
            Gesture::Hold => self.held = Some(frame.layout.held()),
            Gesture::Release => self.held = None,
            Gesture::Tap if !self.layout_used => {
                let memory = self.memory(track);
                // Layout alone leaves a kept view without stepping.
                if memory.lock.take().is_none() {
                    memory.layout = Some(frame.layout.next());
                }
            }
            _ => {}
        }
    }

    /// Octave Up and Down move the drum bank sixteen notes, or a row of four
    /// with Shift held (R13).
    fn octave(&mut self, up: bool, ctx: &mut Ctx<'_>) {
        let Some(model) = ctx.model else {
            return;
        };
        let Some(frame) = self.frame(model) else {
            return;
        };
        let Kind::Rack { rack, first, .. } = frame.kind else {
            return;
        };
        let (key, by) = if self.octave_shift {
            ("rows", 1)
        } else {
            ("groups", 1)
        };
        let by = if up { by } else { -by };
        if Self::can_move(first, up) {
            ctx.cx
                .run("rack.move_pads", json!({ "rack": rack, key: by }));
        }
    }

    /// Whether the bank can move that way: it stays within notes 0 to 127.
    const fn can_move(first: u8, up: bool) -> bool {
        if up {
            first < grid::LAST_FIRST_PAD
        } else {
            first > 0
        }
    }

    fn page_button(&mut self, right: bool, hold: bool) {
        if hold {
            self.following = true;
            return;
        }
        // Page Left and Right show the pair of pages before or after (R28).
        self.following = false;
        self.pair = if right {
            (self.pair + 1).min(grid::PAGES / 2 - 1)
        } else {
            self.pair.saturating_sub(1)
        };
    }
}

//! Note Mode on a track: the drum layouts, the pads of a drum rack, step
//! sequencing beats, and the melodic layouts with the Scale menu and the
//! arrows (`docs/specs/push-2-notes.md`, R1 to R47).
//!
//! [`Note`] is the content of the Note pad mode. Which layout the pads take
//! comes from the selected track's first instrument every time it is read, so
//! changing the instrument changes the layout at once (R1). What the surface
//! keeps for each track (R5) is its [`Memory`]; everything the pads show or
//! do reads the set from the read model and asks for the registered commands
//! a person's editors run.

mod clip;
pub mod grid;
mod keys;
mod melodic;
mod menu;
mod navigate;
mod pads;
mod paint;
mod steps;
mod strip;

use std::collections::BTreeMap;
use std::time::Instant;

use bl_control::{Cx, ReadModel};
use bl_core::time::Ticks;
use bl_model::{ClipId, DeviceId, PadLayout, Scale, SceneId, TrackId};
use serde_json::{Value, json};

use crate::content::{Content, Ctx, Gesture, HeldStep, Pad, PadEvent, Paint, StripEvent, StripUse};
use crate::controls::cc;
use crate::encoder::Counting;

pub use grid::{Layout, Melodic, Overlay, Pages, Role, Size};

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
    /// Any other MIDI track: it plays notes of the scale (R32 to R44).
    Melodic,
    /// An audio track, a return or the Main track: the pads are dark (R1).
    Silent,
}

/// What the surface keeps for each track (R5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Memory {
    /// The drum layout, or the track's own default until Layout steps it.
    layout: Option<Layout>,
    /// The melodic layout (R39).
    melodic: Melodic,
    /// The loop length pads Shift and Layout keep in view.
    lock: Option<Overlay>,
    /// The step size.
    size: Size,
    /// 64 Notes' shift in octaves (R33).
    octave: i32,
    /// The Melodic Sequencer's lowest row, which is also where its 32 notes
    /// start, until Octave or the strip moves it (R40, R43).
    low: Option<u8>,
    /// The selected notes of the Melodic Sequencer + 32 Notes, a bit for
    /// each of the 128 (R43).
    keys: u128,
    /// Whether the touch strip is the modulation wheel (R7).
    modulation: bool,
}

/// What a pad that is down does until it is released.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Down {
    /// It plays a note, which its release ends whatever the layout has
    /// become (R2).
    Played(u8),
    /// It plays and selects one of the 32 notes, or only selects it (R43).
    Picked { key: u8, played: bool },
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
    /// A step of the Melodic Sequencer + 32 Notes, by where it starts.
    Step(Ticks),
}

/// The first step on an empty slot, waiting for its clip (R20).
#[derive(Clone, Copy, Debug)]
struct Pending {
    track: TrackId,
    scene: SceneId,
    /// The notes the step takes, a bit for each of the 128.
    keys: u128,
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
    /// What a melodic layout shows, on a melodic track.
    pub melody: Option<Melody>,
}

/// What Note Mode shows of a melodic track's layouts and scale options
/// (R32 to R44).
#[derive(Clone, Debug, PartialEq)]
pub struct Melody {
    /// In Key (true) or Chromatic.
    pub in_key: bool,
    /// Whether the bottom-left pad stays on C.
    pub fixed: bool,
    /// The interval between rows: `4ths`, `3rds` or `Sequent`.
    pub spacing: String,
    /// `Vertical` or `Horizontal`.
    pub direction: String,
    /// 64 Notes' shift in octaves.
    pub octave: i32,
    /// The lowest note the pads, rows or 32 notes hold.
    pub low: u8,
    /// The highest.
    pub high: u8,
    /// The range's notes as the display notifies them, such as `C1 to C3`.
    pub range: String,
    /// The notes selected in the Melodic Sequencer + 32 Notes.
    pub selected_notes: Vec<u8>,
    /// Whether the touch strip is the modulation wheel rather than pitch
    /// bend, in 64 Notes.
    pub modulation: bool,
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
    /// The Scale menu's encoders' leftover steps.
    menu_counts: [Counting; 8],
    /// Whether the touch strip's press was a Select tap, whose position
    /// goes nowhere (R7).
    strip_taken: bool,
    /// What the strip is for.
    strip: Option<StripUse>,
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
            menu_counts: Default::default(),
            strip_taken: false,
            strip: None,
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
        let settings = model.document.settings();
        let (scale, pads) = (settings.scale, settings.pad_layout);
        let pages = if matches!(kind, Kind::Melodic) {
            Pages::melodic(memory.size, memory.melodic)
        } else {
            Pages::drum(memory.size)
        };
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
            melodic: memory.melodic,
            scale,
            pads,
            octave: memory.octave,
            low: memory.low.unwrap_or_else(|| melodic::default_low(scale)),
            keys: memory.keys,
            modulation: memory.modulation,
            pages,
        })
    }

    /// What the pad at `pad` is in `frame` (R8 to R11).
    fn role(frame: &Frame, row: u8, column: u8) -> Role {
        let first = match frame.kind {
            Kind::Rack { first, .. } => first,
            Kind::Slices => 36,
            Kind::Melodic => return frame.view().role(row, column),
            Kind::Silent => return Role::Off,
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
    /// The melodic layout.
    pub melodic: Melodic,
    /// The set's current scale.
    pub scale: Scale,
    /// The set's pad layout options.
    pub pads: PadLayout,
    /// 64 Notes' shift in octaves.
    pub octave: i32,
    /// The Melodic Sequencer's lowest row, and where its 32 notes start.
    pub low: u8,
    /// The selected notes of the Melodic Sequencer + 32 Notes.
    pub keys: u128,
    /// Whether the strip is the modulation wheel.
    pub modulation: bool,
    /// How the layout counts pages.
    pub pages: Pages,
}

impl Frame {
    /// What the melodic layout's pads are.
    fn view(&self) -> melodic::View {
        melodic::View {
            layout: self.melodic,
            scale: self.scale,
            pads: self.pads,
            octave: self.octave,
            low: self.low,
            loops: self.overlay.is_some(),
            per_row: self.size.per_row(),
        }
    }
}

impl Content for Note {
    fn name(&self) -> &str {
        "Note"
    }

    fn pad(&mut self, pad: Pad, event: PadEvent, ctx: &mut Ctx<'_>) {
        match event {
            PadEvent::On(velocity) => self.pad_on(pad, velocity, ctx),
            PadEvent::Off => self.pad_off(pad, ctx),
            PadEvent::Pressure(pressure) => self.pressure(pad, pressure, ctx),
            // Polyphonic key pressure is what Note Mode sets (R6).
            PadEvent::ChannelPressure(_) => {}
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
        self.strip
    }

    fn scale_available(&self) -> bool {
        self.summary.melody.is_some()
    }

    fn scale_button(&mut self, button: u8, ctx: &mut Ctx<'_>) {
        self.scale_press(button, ctx);
    }

    fn scale_turn(&mut self, column: u8, steps: i32, ctx: &mut Ctx<'_>) {
        self.scale_steps(column, steps, ctx);
    }

    fn paint_scale(&mut self, paint: &mut Paint<'_>) {
        self.paint_scale_menu(paint);
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
            (62 | 63, Gesture::Tap) => self.page_button(button == 63, false, model),
            (62 | 63, Gesture::Hold) => self.page_button(button == 63, true, model),
            (cc::LEFT | cc::RIGHT | cc::UP | cc::DOWN, Gesture::Press) => {
                self.arrow(button, ctx);
            }
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
        if matches!(frame.kind, Kind::Silent) {
            return;
        }
        let track = frame.track;
        let melodic = matches!(frame.kind, Kind::Melodic);
        // A melodic layout shows the loop length pads (R39).
        let held = if melodic {
            Overlay::LoopPads
        } else {
            frame.layout.held()
        };
        match gesture {
            Gesture::Press => {
                self.layout_used = false;
                // Shift and Layout keep the view Layout shows while held (R9).
                if ctx.modifiers.shift() {
                    let overlay = held;
                    let memory = self.memory(track);
                    memory.lock = if memory.lock.is_some() {
                        None
                    } else {
                        Some(overlay)
                    };
                    self.layout_used = true;
                }
            }
            Gesture::Hold => self.held = Some(held),
            Gesture::Release => self.held = None,
            Gesture::Tap if !self.layout_used => {
                let memory = self.memory(track);
                // Layout alone leaves a kept view without stepping.
                if memory.lock.take().is_none() {
                    if melodic {
                        memory.melodic = frame.melodic.next();
                    } else {
                        memory.layout = Some(frame.layout.next());
                    }
                }
            }
            _ => {}
        }
    }

    /// Octave Up and Down move the drum bank sixteen notes, or a row of four
    /// with Shift held (R13), the melodic pads an octave (R33), and the
    /// Melodic Sequencer's rows an octave, or one scale note (R41, R43).
    fn octave(&mut self, up: bool, ctx: &mut Ctx<'_>) {
        let Some(model) = ctx.model else {
            return;
        };
        let Some(frame) = self.frame(model) else {
            return;
        };
        if matches!(frame.kind, Kind::Melodic) {
            self.octave_melodic(&frame, up, self.octave_shift, ctx);
            return;
        }
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

    fn page_button(&mut self, right: bool, hold: bool, model: &ReadModel) {
        if hold {
            self.following = true;
            return;
        }
        let Some(views) = self.frame(model).map(|frame| frame.pages.views()) else {
            return;
        };
        // Page Left and Right show the pages before or after (R28).
        self.following = false;
        self.pair = if right {
            (self.pair + 1).min(views - 1)
        } else {
            self.pair.saturating_sub(1)
        };
    }
}

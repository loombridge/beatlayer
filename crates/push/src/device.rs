//! Device Mode: the selected track's devices on the upper display buttons,
//! the device in focus on the encoders, Edit Mode's banks, Delete, Mute and
//! moves, and racks (`docs/specs/push-2-tracks.md`, R15 to R27).
//!
//! What the mode shows is read from the set and the Device View as the read
//! model has them, so Push 2 follows the window (R3, R26); what it keeps
//! itself (the window of devices, Edit Mode, the device button held) is
//! surface state: no part of the set, no undo step (R2).

use std::time::Instant;

use bl_control::{Focus, ParamView, ReadModel};
use bl_model::{
    AutomationTarget, ChainId, DeviceId, DeviceRole, Document, ParamKind, RackKind, TrackId,
};
use serde_json::json;

use crate::content::{Content, Ctx, Gesture, HeldStep, Paint, Turn};
use crate::controls::{self, cc};
use crate::encoder::{Counting, LIST_STEPS, Moving};
use crate::screen::{Column, Dot, Graphic, Icon, Label, Shade};
use crate::simpler::{self, Simpler};

/// The gesture key of encoder `n`'s changes is this plus `n`, so a touch is
/// one undo step (R1).
const PARAM_GESTURE: u32 = 0x5200;
/// The gesture key of the moves made while a device's button is held: one
/// undo step for the hold (R21).
const MOVE_GESTURE: u32 = 0x5300;
/// The window of devices when its first and last buttons are arrows.
const WINDOW_WITH_ARROWS: usize = 6;
/// How many devices the upper buttons hold at most without arrows.
const BUTTONS: usize = 8;
/// What a device that is switched off is drawn in.
const GREY: [u8; 3] = [110, 110, 110];
/// The colour of an arrow.
const WHITE: [u8; 3] = [230, 230, 230];
/// How many chains the lower buttons show at once (R24).
const CHAINS_SHOWN: usize = 8;
/// A turn that rests this long starts again from where the document has the
/// parameter.
const REST: std::time::Duration = std::time::Duration::from_millis(500);

/// A device on the upper buttons' list (R15).
#[derive(Clone, Debug, PartialEq)]
struct Slot {
    id: DeviceId,
    name: String,
    depth: usize,
    role: DeviceRole,
    rack: Option<RackKind>,
    enabled: bool,
    /// The chain it is in, or none for the track's own chain.
    chain: Option<ChainId>,
    /// Whether it is a rack whose devices view shows (R22).
    unfolded: bool,
}

/// The track Device Mode shows: the first selected track the set has.
fn shown_track(model: &ReadModel) -> Option<TrackId> {
    model
        .selection
        .tracks
        .iter()
        .copied()
        .find(|track| model.document.track(*track).is_some())
}

/// The devices in signal order, with the devices of an unfolded rack's
/// selected chain after it (R15, R22).
fn list(model: &ReadModel) -> Vec<Slot> {
    let mut slots = Vec::new();
    if let Some(track) = shown_track(model).and_then(|t| model.document.track(t)) {
        for device in track.devices() {
            push(model, None, device, 0, &mut slots);
        }
    }
    slots
}

fn push(
    model: &ReadModel,
    chain: Option<ChainId>,
    device: &bl_model::Device,
    depth: usize,
    slots: &mut Vec<Slot>,
) {
    let rack = device.rack_kind();
    let views = model
        .devices
        .racks
        .get(&device.id)
        .copied()
        .unwrap_or_default();
    let unfolded = rack.is_some() && views.devices;
    slots.push(Slot {
        id: device.id,
        name: device.title().to_owned(),
        depth,
        role: device.role,
        rack,
        enabled: device.enabled,
        chain,
        unfolded,
    });
    if unfolded
        && let Some(selected) = model.devices.chains.get(&device.id)
        && let Some(inside) = device.chains.iter().find(|c| c.id == *selected)
    {
        for inner in &inside.devices {
            push(model, Some(inside.id), inner, depth + 1, slots);
        }
    }
}

/// What a device's button shows, as the readback names it (R27).
fn kind_name(slot: &Slot) -> &'static str {
    match (slot.rack, slot.role) {
        (Some(RackKind::Instrument), _) => "instrument_rack",
        (Some(RackKind::AudioEffect), _) => "audio_effect_rack",
        (Some(RackKind::MidiEffect), _) => "midi_effect_rack",
        (Some(RackKind::Drum), _) => "drum_rack",
        (None, DeviceRole::Instrument) => "instrument",
        (None, DeviceRole::AudioEffect) => "audio_effect",
        (None, DeviceRole::MidiEffect) => "midi_effect",
    }
}

const fn rack_icon(kind: RackKind) -> Icon {
    match kind {
        RackKind::Instrument => Icon::InstrumentRack,
        RackKind::AudioEffect => Icon::AudioEffectRack,
        RackKind::MidiEffect => Icon::MidiEffectRack,
        RackKind::Drum => Icon::DrumRack,
    }
}

/// What an upper button stands for (R15).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    /// An arrow that moves the window towards the first device.
    Left,
    /// An arrow that moves it towards the last.
    Right,
    /// The device at this place in the list.
    Slot(usize),
    /// Nothing.
    Empty,
}

/// The devices the upper buttons show: all of them up to eight, else a window
/// of six between two arrows that moves by one (R15).
#[derive(Clone, Copy, Debug)]
struct Window {
    first: usize,
    len: usize,
}

impl Window {
    fn new(first: usize, len: usize) -> Self {
        let mut window = Self { first, len };
        window.first = first.min(len.saturating_sub(window.cap()));
        window
    }

    const fn arrows(self) -> bool {
        self.len > BUTTONS
    }

    const fn cap(self) -> usize {
        if self.arrows() {
            WINDOW_WITH_ARROWS
        } else {
            BUTTONS
        }
    }

    fn can_left(self) -> bool {
        self.arrows() && self.first > 0
    }

    fn can_right(self) -> bool {
        self.arrows() && self.first + self.cap() < self.len
    }

    fn at(self, column: usize) -> Place {
        let place = if self.arrows() {
            match column {
                0 if self.can_left() => return Place::Left,
                7 if self.can_right() => return Place::Right,
                1..=6 => column - 1,
                _ => return Place::Empty,
            }
        } else {
            column
        };
        let at = self.first + place;
        if at < self.len {
            Place::Slot(at)
        } else {
            Place::Empty
        }
    }

    /// The column of the device at `index`, if it is in the window.
    fn column_of(self, index: usize) -> Option<usize> {
        let offset = index.checked_sub(self.first)?;
        (offset < self.cap()).then_some(offset + usize::from(self.arrows()))
    }

    /// The first position that shows `index`, keeping the window's.
    fn showing(self, index: usize) -> usize {
        if index < self.first {
            index
        } else if index >= self.first + self.cap() {
            index + 1 - self.cap()
        } else {
            self.first
        }
    }
}

/// How the lower buttons divide the banks in Edit Mode into pages (R17): a
/// page is the first bank it shows and how many it shows. With more than
/// eight banks the first page holds seven and an arrow on the rightmost
/// button; a later page has a back arrow on the leftmost.
fn pages(count: usize) -> Vec<(usize, usize)> {
    let mut pages = Vec::new();
    let mut at = 0;
    loop {
        let room = 8 - usize::from(!pages.is_empty());
        let rest = count - at;
        if rest <= room {
            pages.push((at, rest));
            return pages;
        }
        pages.push((at, room - 1));
        at += room - 1;
    }
}

/// What `push.state` reads of Device Mode (R27).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// The devices on the upper buttons, with the button each is on from 1.
    pub devices: Vec<DeviceState>,
    /// Whether the first and the last upper buttons are arrows, shown only
    /// where the window can move that way.
    pub arrows: (bool, bool),
    /// Whether the track has no devices.
    pub empty: bool,
    /// Whether Edit Mode is on.
    pub edit: bool,
    /// The bank on the encoders, from 0.
    pub bank: usize,
    /// Every bank's name.
    pub banks: Vec<String>,
    /// The page of banks the lower buttons show in Edit Mode, from 0.
    pub page: usize,
    /// How many pages the banks make.
    pub pages: usize,
    /// The eight parameters on the encoders.
    pub params: Vec<ParamState>,
    /// The chains the lower buttons show while a rack's button is held.
    pub chains: Vec<ChainState>,
    /// A Simpler's main bank and Edit Mode, when it is the device in focus
    /// (push-2-notes R67 to R76).
    pub simpler: Option<simpler::State>,
}

/// One device on an upper button, as `push.state` reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceState {
    /// The button, from 1.
    pub button: usize,
    /// The device.
    pub device: DeviceId,
    /// Its name.
    pub name: String,
    /// How deep it is inside racks: 0 on the track's own chain.
    pub depth: usize,
    /// What it is: `instrument`, `audio_effect`, `midi_effect`, or a rack's
    /// kind.
    pub kind: &'static str,
    /// Whether it is switched on.
    pub enabled: bool,
    /// Whether it is the device in focus.
    pub selected: bool,
    /// For a rack, whether its devices follow it.
    pub unfolded: bool,
}

/// One encoder's parameter, as `push.state` reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamState {
    /// The parameter's name.
    pub name: String,
    /// Its value as the device shows it.
    pub value: String,
    /// Whether a macro holds it.
    pub held: bool,
}

/// One chain on a lower button, as `push.state` reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct ChainState {
    /// The chain.
    pub chain: ChainId,
    /// Its name.
    pub name: String,
    /// Whether it is the selected chain.
    pub selected: bool,
}

/// The device button held now (R21, R24).
#[derive(Clone, Copy, Debug)]
struct Held {
    device: DeviceId,
    rack: bool,
    /// The upper button, from 0.
    column: usize,
}

/// Device Mode's content.
pub struct DeviceMode {
    /// The first device the window shows.
    first: usize,
    /// The device in focus the window last made room for.
    seen: Option<DeviceId>,
    /// Edit Mode, with the device it is on (R16).
    edit: Option<DeviceId>,
    /// The page of banks shown in Edit Mode.
    page: usize,
    held: Option<Held>,
    /// The place a held device was moved to, until the set shows it.
    moved_to: Option<(DeviceId, usize)>,
    moved: bool,
    moves: Counting,
    counts: [Counting; 8],
    moving: [Moving; 8],
    /// The value each stepped parameter was last set to, until the set shows
    /// it.
    sent: [Option<(f64, Instant)>; 8],
    /// What Simpler adds to its banks (push-2-notes R67 to R74).
    simpler: Simpler,
    state: State,
}

impl Default for DeviceMode {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceMode {
    /// Device Mode before it is shown.
    #[must_use]
    pub fn new() -> Self {
        Self {
            first: 0,
            seen: None,
            edit: None,
            page: 0,
            held: None,
            moved_to: None,
            moved: false,
            moves: Counting::default(),
            counts: Default::default(),
            moving: Default::default(),
            sent: [None; 8],
            simpler: Simpler::new(),
            state: State::default(),
        }
    }

    fn focus(model: &ReadModel, row: usize) -> Option<&Focus> {
        model.row(row)?.focus.as_ref()
    }

    /// Where the first bank button of the page the focus is on sits.
    fn page_of(banks: usize, bank: usize) -> usize {
        pages(banks)
            .iter()
            .position(|(first, count)| bank >= *first && bank < first + count)
            .unwrap_or(0)
    }

    fn upper(&mut self, column: usize, gesture: Gesture, model: &ReadModel, ctx: &mut Ctx<'_>) {
        let slots = list(model);
        let window = Window::new(self.first, slots.len());
        let focus = Self::focus(model, ctx.cx.row()).map(|f| f.device);
        if self.edit.is_some() {
            // The leftmost button goes back to the top level (R16).
            if column == 0 && gesture == Gesture::Tap {
                self.edit = None;
            } else if column > 0
                && gesture == Gesture::Press
                && let Some(focus) = focus
                && let Some(view) = simpler::View::of(model, focus)
            {
                // Simpler's Edit Mode runs its own buttons (push-2-notes R70).
                let bpm = model.tempo.get();
                self.simpler.press(column, &view, ctx.slice, bpm, ctx);
            }
            return;
        }
        let place = window.at(column);
        let locked_or_held = |button| ctx.modifiers.has(button);
        match (gesture, place) {
            (Gesture::Press, Place::Left) => self.first = window.first.saturating_sub(1),
            (Gesture::Press, Place::Right) => self.first = window.first + 1,
            (Gesture::Press, Place::Slot(at)) => {
                let slot = &slots[at];
                if locked_or_held(cc::DELETE) {
                    Self::delete(slot, &slots, ctx);
                } else if locked_or_held(cc::MUTE) {
                    ctx.cx.run(
                        "device.set_enabled",
                        json!({ "device": slot.id, "enabled": !slot.enabled }),
                    );
                } else if ctx.modifiers.shift() && slot.rack.is_some() && focus == Some(slot.id) {
                    // Shift with a tap of the selected rack's button enters
                    // its Edit Mode, since a second tap unfolds it (R23).
                    self.enter_edit(slot.id, model, ctx);
                }
            }
            (Gesture::Tap, Place::Slot(at))
                if !locked_or_held(cc::DELETE) && !locked_or_held(cc::MUTE) =>
            {
                let slot = &slots[at];
                if focus != Some(slot.id) {
                    ctx.cx.run("device.select", json!({ "device": slot.id }));
                } else if let Some(kind) = slot.rack {
                    // A rack's second tap folds or unfolds it, as the
                    // window's devices view does; a drum rack stays as the
                    // window has it (R22).
                    if kind != RackKind::Drum {
                        ctx.cx.run(
                            "rack.set_view",
                            json!({
                                "rack": slot.id,
                                "view": "devices",
                                "shown": !slot.unfolded,
                            }),
                        );
                    }
                } else {
                    self.enter_edit(slot.id, model, ctx);
                }
            }
            (Gesture::Hold, Place::Slot(at))
                if !locked_or_held(cc::DELETE) && !locked_or_held(cc::MUTE) =>
            {
                let slot = &slots[at];
                self.held = Some(Held {
                    device: slot.id,
                    rack: slot.rack.is_some(),
                    column,
                });
                self.moves.reset();
                self.moved = false;
                self.moved_to = None;
            }
            (Gesture::Release, _) if self.held.is_some_and(|h| h.column == column) => {
                self.let_go(ctx);
            }
            _ => {}
        }
    }

    /// The device button comes up: the moves made while it was held are one
    /// undo step (R21).
    fn let_go(&mut self, ctx: &mut Ctx<'_>) {
        if self.held.take().is_some() && std::mem::take(&mut self.moved) {
            ctx.cx.end_gesture(MOVE_GESTURE);
        }
        self.moved_to = None;
    }

    fn enter_edit(&mut self, device: DeviceId, model: &ReadModel, ctx: &Ctx<'_>) {
        self.edit = Some(device);
        self.page = Self::focus(model, ctx.cx.row())
            .map_or(0, |focus| Self::page_of(focus.banks.len(), focus.bank));
    }

    /// Deletes `slot`'s device and selects the one after it in its chain, or
    /// the one before when it was the last (R19).
    fn delete(slot: &Slot, slots: &[Slot], ctx: &mut Ctx<'_>) {
        let siblings: Vec<&Slot> = slots
            .iter()
            .filter(|s| s.chain == slot.chain && s.depth == slot.depth)
            .collect();
        let at = siblings.iter().position(|s| s.id == slot.id);
        let next = at.and_then(|at| {
            siblings
                .get(at + 1)
                .or_else(|| at.checked_sub(1).and_then(|b| siblings.get(b)))
        });
        ctx.cx.run("device.remove", json!({ "device": slot.id }));
        if let Some(next) = next {
            ctx.cx.run("device.select", json!({ "device": next.id }));
        }
    }

    fn lower(&mut self, column: usize, gesture: Gesture, model: &ReadModel, ctx: &mut Ctx<'_>) {
        if gesture != Gesture::Press {
            return;
        }
        if let Some(held) = self.held.filter(|h| h.rack) {
            // The held rack's chains (R24).
            if let Some(chain) = model
                .document
                .device(held.device)
                .and_then(|rack| rack.chains.get(column))
            {
                ctx.cx.run("chain.select", json!({ "chain": chain.id }));
            }
            return;
        }
        let Some(focus) = Self::focus(model, ctx.cx.row()) else {
            return;
        };
        let pages = pages(focus.banks.len());
        let page = self.page.min(pages.len() - 1);
        let (first, count) = pages[page];
        let back = page > 0;
        let forward = page + 1 < pages.len();
        if back && column == 0 {
            self.page = page - 1;
        } else if forward && column == 7 {
            self.page = page + 1;
        } else if let Some(n) = column.checked_sub(usize::from(back))
            && n < count
        {
            ctx.cx.run(
                "control_surface.set_bank",
                json!({ "row": ctx.cx.row() + 1, "bank": first + n }),
            );
        }
    }

    /// A finger came on encoder `column`: Simpler's main bank remembers it
    /// for Zoom (push-2-notes R69).
    fn touch_simpler(&mut self, column: u8, ctx: &Ctx<'_>) {
        let Some(model) = ctx.model else {
            return;
        };
        if let Some(focus) = Self::focus(model, ctx.cx.row())
            && focus.bank == 0
            && let Some(view) = simpler::View::of(model, focus.device)
        {
            self.simpler.touch(column, &view);
        }
    }

    /// Turns encoder `column` by `steps`: Simpler's main bank, or the
    /// parameter the bank puts there (R18).
    fn turn(&mut self, column: u8, steps: i32, model: &ReadModel, ctx: &mut Ctx<'_>) {
        let Some(focus) = Self::focus(model, ctx.cx.row()) else {
            return;
        };
        if focus.bank == 0
            && let Some(view) = simpler::View::of(model, focus.device)
        {
            if !view.has_sample() {
                return;
            }
            match simpler::control_param(&view, column) {
                Some(Some(id)) => {
                    if let Some(param) = view.param(id) {
                        self.set_param(column, steps, &param, focus.device, ctx);
                    }
                }
                Some(None) => {}
                None => self.simpler.turn(column, steps, &view, ctx.slice, ctx),
            }
            return;
        }
        if let Some(param) = focus.params.get(usize::from(column)) {
            self.set_param(column, steps, param, focus.device, ctx);
        }
    }

    /// Sets `param` of `device` on encoder `column` by `steps` (R18).
    fn set_param(
        &mut self,
        column: u8,
        steps: i32,
        param: &ParamView,
        device: DeviceId,
        ctx: &mut Ctx<'_>,
    ) {
        // A parameter a macro holds doesn't move (R18).
        if param.held {
            return;
        }
        let descriptor = &param.descriptor;
        let n = usize::from(column);
        let now = ctx.cx.now();
        // Spec: push-2-notes R66. With steps held, the turn is the value at
        // the first held step.
        let target = AutomationTarget::Device {
            device,
            param: param.param,
        };
        let steps_value = ctx
            .model
            .filter(|_| !ctx.held_steps.is_empty())
            .and_then(|m| crate::automation::value_at_first_step(m, ctx.held_steps, &target));
        let current = steps_value.unwrap_or(param.value);
        let value = if descriptor.is_stepped() {
            let by = self.counts[n].take(steps, LIST_STEPS);
            if by == 0 {
                return;
            }
            let base = match self.sent[n] {
                Some((value, at)) if now.saturating_duration_since(at) < REST => value,
                _ => current,
            };
            let value = descriptor.clamp(base + f64::from(by));
            self.sent[n] = Some((value, now));
            value
        } else {
            let position = descriptor.normalize(current);
            let to = self.moving[n].turn(now, position, steps, ctx.modifiers.shift());
            descriptor.denormalize(to)
        };
        if !ctx.held_steps.is_empty() {
            if let Some(clip) = ctx
                .model
                .and_then(crate::automation::edited_clip)
                .map(|c| c.id)
            {
                crate::automation::write_steps(
                    ctx,
                    PARAM_GESTURE + u32::from(column),
                    clip,
                    &target,
                    value,
                );
            }
            return;
        }
        ctx.cx.run_held(
            PARAM_GESTURE + u32::from(column),
            "device.set_param",
            json!({ "device": device, "param": param.param, "value": value }),
        );
    }

    /// Delete and a touch: the parameter's automation goes, or the parameter
    /// goes back to its default where it has none (R62).
    fn delete_automation(&mut self, column: u8, ctx: &mut Ctx<'_>) {
        let Some(model) = ctx.model else {
            return;
        };
        let Some(focus) = Self::focus(model, ctx.cx.row()) else {
            return;
        };
        let Some(param) = focus.params.get(usize::from(column)) else {
            return;
        };
        let Some(track) = model.document.device_track(focus.device) else {
            return;
        };
        let target = AutomationTarget::Device {
            device: focus.device,
            param: param.param,
        };
        if !crate::automation::delete(ctx, model, track, &target) && !param.held {
            ctx.cx.run(
                "device.set_param",
                json!({
                    "device": focus.device,
                    "param": param.param,
                    "value": param.descriptor.default,
                }),
            );
        }
    }

    /// Moves the held effect one place for each list step, only to places
    /// that keep the chain's order (R21).
    fn move_held(&mut self, held: Held, steps: i32, ctx: &mut Ctx<'_>, doc: &Document) {
        let Some(place) = doc.device_place(held.device) else {
            return;
        };
        let Some(device) = doc.device(held.device) else {
            return;
        };
        let by = self.moves.take(steps, LIST_STEPS);
        // An instrument doesn't move (R21).
        if by == 0 || device.role == DeviceRole::Instrument {
            return;
        }
        let siblings: Vec<DeviceRole> = match place.chain {
            None => doc
                .track(place.track)
                .map(|t| t.devices().map(|d| (d.id, d.role)).collect::<Vec<_>>()),
            Some(chain) => doc
                .chain(chain)
                .map(|c| c.devices.iter().map(|d| (d.id, d.role)).collect()),
        }
        .unwrap_or_default()
        .into_iter()
        .filter(|(id, _)| *id != held.device)
        .map(|(_, role)| role)
        .collect();
        let mut at = match self.moved_to {
            Some((id, to)) if id == held.device => to,
            _ => place.index,
        };
        let start = at;
        for _ in 0..by.unsigned_abs() {
            if by > 0 && siblings.get(at) == Some(&device.role) {
                at += 1;
            } else if by < 0 && at > 0 && siblings.get(at - 1) == Some(&device.role) {
                at -= 1;
            } else {
                break;
            }
        }
        if at == start {
            return;
        }
        self.moved_to = Some((held.device, at));
        self.moved = true;
        let (id, args) = match place.chain {
            None => (
                "device.move",
                json!({ "device": held.device, "track": place.track, "index": at }),
            ),
            Some(chain) => (
                "device.move_to_chain",
                json!({ "device": held.device, "chain": chain, "index": at }),
            ),
        };
        ctx.cx.run_held(MOVE_GESTURE, id, args);
    }

    /// A parameter as its column shows it (R20).
    fn column(model: &ReadModel, focus: &Focus, param: &ParamView, held: &[HeldStep]) -> Column {
        let doc = &model.document;
        let descriptor = &param.descriptor;
        let graphic = match descriptor.kind {
            ParamKind::Toggle | ParamKind::Choice { .. } => Graphic::Choice(param.text.clone()),
            _ => Graphic::Arc(descriptor.normalize(param.value) as f32),
        };
        let target = AutomationTarget::Device {
            device: focus.device,
            param: param.param,
        };
        let dot = doc.device_track(focus.device).map_or(Dot::None, |track| {
            crate::automation::dot(model, track, &target)
        });
        // Spec: push-2-notes R66. Holding steps shows the value at the first.
        let graphic = match crate::automation::value_at_first_step(model, held, &target) {
            Some(value) if !matches!(graphic, Graphic::Choice(_)) => {
                Graphic::Arc(descriptor.normalize(value) as f32)
            }
            _ => graphic,
        };
        Column {
            name: param.name.clone(),
            value: param.text.clone(),
            graphic,
            dot,
            touched: false,
        }
    }
}

impl Content for DeviceMode {
    fn name(&self) -> &str {
        "Device"
    }

    fn button(&mut self, button: u8, gesture: Gesture, ctx: &mut Ctx<'_>) {
        let Some(model) = ctx.model else {
            return;
        };
        if let Some(column) = controls::upper_column(button) {
            self.upper(usize::from(column), gesture, model, ctx);
        } else if let Some(column) = controls::lower_column(button) {
            self.lower(usize::from(column), gesture, model, ctx);
        }
    }

    fn encoder(&mut self, column: u8, turn: Turn, ctx: &mut Ctx<'_>) {
        let n = usize::from(column);
        match turn {
            Turn::Touch => {
                self.counts[n].reset();
                self.moving[n].touch();
                self.sent[n] = None;
                if ctx.modifiers.has(cc::DELETE) {
                    self.delete_automation(column, ctx);
                }
                self.touch_simpler(column, ctx);
            }
            // From touch to release is one undo step (R1).
            Turn::Release => ctx.cx.end_gesture(PARAM_GESTURE + u32::from(column)),
            Turn::Steps(steps) => {
                let Some(model) = ctx.model else {
                    return;
                };
                match self.held {
                    Some(held) => self.move_held(held, steps, ctx, &model.document),
                    None => self.turn(column, steps, model, ctx),
                }
            }
        }
    }

    fn paint(&mut self, paint: &mut Paint<'_>) {
        let model = paint.model;
        let doc = &model.document;
        let focus = Self::focus(model, paint.row);
        let focus_id = focus.map(|f| f.device);
        let slots = list(model);
        // The window makes room for the device in focus when that changes,
        // whichever side changed it (R15).
        if self.seen != focus_id {
            self.seen = focus_id;
            if let Some(index) = slots.iter().position(|s| Some(s.id) == focus_id) {
                self.first = Window::new(self.first, slots.len()).showing(index);
            }
        }
        let window = Window::new(self.first, slots.len());
        self.first = window.first;
        // Edit Mode ends when another device comes into focus, whose banks
        // are others (R16).
        if self.edit.is_some() && self.edit != focus_id {
            self.edit = None;
        }
        if self.held.is_some_and(|h| doc.device(h.device).is_none()) {
            self.held = None;
        }
        let colour = shown_track(model)
            .and_then(|t| doc.track(t))
            .map_or(WHITE, |t| bl_model::swatch_rgb(paint.colours, t.colour()));

        let mut state = State {
            empty: slots.is_empty(),
            arrows: (window.can_left(), window.can_right()),
            ..State::default()
        };

        // The upper buttons.
        let simpler = focus_id.and_then(|device| simpler::View::of(model, device));
        if self.edit.is_some() {
            paint.screen.upper[0] = Label::new("Back", colour, false).with_icon(Icon::Left);
            state.edit = true;
            // Simpler's Edit Mode has buttons of its own after Back (R70).
            if let Some(view) = &simpler {
                let buttons = self.simpler.paint_edit(
                    view,
                    model.tempo.get(),
                    colour,
                    &mut paint.screen.upper,
                );
                state.simpler = Some(simpler::State {
                    buttons,
                    ..simpler::State::default()
                });
            }
        } else {
            for column in 0..8 {
                match window.at(column) {
                    Place::Left => {
                        paint.screen.upper[column] =
                            Label::new("", WHITE, false).with_icon(Icon::Left);
                    }
                    Place::Right => {
                        paint.screen.upper[column] =
                            Label::new("", WHITE, false).with_icon(Icon::Right);
                    }
                    Place::Slot(at) => {
                        let slot = &slots[at];
                        let selected = Some(slot.id) == focus_id;
                        let ink = if slot.enabled { colour } else { GREY };
                        let mut label = Label::new(slot.name.clone(), ink, selected);
                        if let Some(kind) = slot.rack {
                            label = label.with_icon(rack_icon(kind));
                        }
                        paint.screen.upper[column] = label;
                        state.devices.push(DeviceState {
                            button: column + 1,
                            device: slot.id,
                            name: slot.name.clone(),
                            depth: slot.depth,
                            kind: kind_name(slot),
                            enabled: slot.enabled,
                            selected,
                            unfolded: slot.unfolded,
                        });
                    }
                    Place::Empty => {}
                }
            }
            // A line under an unfolded rack runs to the last of its devices.
            for (at, slot) in slots.iter().enumerate() {
                let Some(from) = window.column_of(at).filter(|_| slot.unfolded) else {
                    continue;
                };
                let last = slots[at + 1..]
                    .iter()
                    .take_while(|s| s.depth > slot.depth)
                    .count();
                let to = (at + 1..=at + last).rev().find_map(|i| window.column_of(i));
                if let Some(to) = to {
                    paint.screen.folds.push((from, to));
                }
            }
        }

        // The encoders: the bank in focus, drawn grey while the device is off.
        let enabled = focus_id
            .and_then(|d| doc.device(d))
            .is_none_or(|d| d.enabled);
        if let Some(focus) = focus {
            let shade = if enabled { Shade::Normal } else { Shade::Grey };
            let main = focus.bank == 0;
            if let Some(view) = simpler.as_ref().filter(|_| main) {
                // Simpler's main bank is drawn over its waveform (R67).
                self.simpler.settle(view);
                let buttons = state.simpler.take().map(|s| s.buttons);
                let mut drawn = self.simpler.paint(view, paint.slice, paint);
                if let Some(buttons) = buttons {
                    drawn.buttons = buttons;
                }
                state.simpler = Some(drawn);
            } else {
                for (column, param) in focus.params.iter().take(8).enumerate() {
                    paint.screen.columns[column] =
                        Self::column(model, focus, param, paint.held_steps);
                    paint.screen.shades[column] = match (shade, param.held) {
                        (Shade::Normal, true) => Shade::Dimmed,
                        (shade, _) => shade,
                    };
                    state.params.push(ParamState {
                        name: param.name.clone(),
                        value: param.text.clone(),
                        held: param.held,
                    });
                }
            }
            state.bank = focus.bank;
            state.banks.clone_from(&focus.banks);
            state.pages = pages(focus.banks.len()).len();
            state.page = Self::page_of(focus.banks.len(), focus.bank);
        }

        // The lower buttons: Edit Mode's banks, or a held rack's chains.
        if self.edit.is_some()
            && let Some(focus) = focus
        {
            let pages = pages(focus.banks.len());
            let page = self.page.min(pages.len() - 1);
            self.page = page;
            let (first, count) = pages[page];
            let back = page > 0;
            if back {
                paint.screen.lower[0] = Label::new("", WHITE, false).with_icon(Icon::Left);
            }
            for n in 0..count {
                let bank = first + n;
                paint.screen.lower[n + usize::from(back)] =
                    Label::new(focus.banks[bank].clone(), colour, bank == focus.bank);
            }
            if page + 1 < pages.len() {
                paint.screen.lower[7] = Label::new("", WHITE, false).with_icon(Icon::Right);
            }
            state.page = page;
        } else if let Some(held) = self.held.filter(|h| h.rack)
            && let Some(rack) = doc.device(held.device)
        {
            let selected = model.devices.chains.get(&held.device);
            for (n, chain) in rack.chains.iter().take(CHAINS_SHOWN).enumerate() {
                let ink = chain
                    .colour
                    .map_or(colour, |c| bl_model::swatch_rgb(paint.colours, c));
                let is_selected = selected == Some(&chain.id);
                paint.screen.lower[n] = Label::new(chain.name.clone(), ink, is_selected);
                state.chains.push(ChainState {
                    chain: chain.id,
                    name: chain.name.clone(),
                    selected: is_selected,
                });
            }
        }

        // A track with no devices says so and offers Add Device (R26).
        if slots.is_empty() {
            let text = if shown_track(model).is_none() {
                "No track is selected"
            } else {
                "This track has no devices: press Add Device"
            };
            paint.screen.title = Some(text.to_owned());
        }
        paint.lights.white(cc::ADD_DEVICE, true, slots.is_empty());
        self.state = state;
    }

    fn claims_lower(&self) -> bool {
        self.edit.is_some() || self.held.is_some_and(|h| h.rack)
    }

    fn pad_slicing(&self) -> bool {
        self.simpler.pad_slicing()
    }

    fn device_state(&self) -> Option<State> {
        Some(self.state.clone())
    }
}

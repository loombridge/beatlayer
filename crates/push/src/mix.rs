//! Mix Mode: Track Mix and Global Mix, Input & Output, unfolding groups and
//! racks, and Mute, Solo, Delete and Duplicate on tracks
//! (`docs/specs/push-2-tracks.md`, R28 to R42).
//!
//! The mode keeps only what Push 2 shows: which of Track Mix's two views is
//! chosen, which parameter Global Mix sets, how far the sends are shifted and,
//! while chains are unfolded, where the eight places start (R2). The folds
//! themselves are the window's own, so they are read from the model and
//! changed by the commands the window runs (R37).

use std::time::{Duration, Instant};

use bl_control::ReadModel;
use bl_control::mix::Chooser as Route;
use bl_model::fader::{
    fader_gain, fader_position, pan_text, send_level, send_position, volume_text,
};
use bl_model::{
    ChainId, Device, DeviceId, Document, PanMode, RackKind, Receive, TrackId, TrackKind,
    return_letter,
};
use serde_json::{Value, json};

use crate::content::{Content, Ctx, Gesture, MixPage, Pad, Paint, Target, Turn};
use crate::controls::{self, cc};
use crate::encoder::{Counting, LIST_STEPS, Moving};
use crate::leds::Led;
use crate::screen::{Column, Graphic, Icon, Label};

/// The gesture key of the commands one lower-button action runs, so they are
/// one undo step (R1, R39, R40).
const ACTION_GESTURE: u32 = 0x5200;
/// The colour of the mode's labels.
const WHITE: [u8; 3] = [230, 230, 230];
/// The Mute button's light while the selected track is muted.
const MUTED: [u8; 3] = [255, 150, 0];
/// The Solo button's light while the selected track is soloed.
const SOLOED: [u8; 3] = [60, 140, 255];
/// A list that rests this long starts again from the document's choice.
const REST: Duration = Duration::from_millis(500);
/// The eight places.
const PLACES: usize = 8;

/// One of the eight places: a track, or a chain of an unfolded rack (R37,
/// R38).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// A track.
    Track(TrackId),
    /// A chain of the rack on a track.
    Chain {
        /// The track the rack is on.
        track: TrackId,
        /// The rack.
        rack: DeviceId,
        /// The chain.
        chain: ChainId,
    },
}

impl Place {
    const fn track(self) -> TrackId {
        match self {
            Self::Track(track) | Self::Chain { track, .. } => track,
        }
    }
}

/// Track Mix's two views (R29).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    /// Volume, pan and sends.
    #[default]
    Mix,
    /// The routing and monitoring.
    InOut,
}

/// What Global Mix sets on the eight places (R32).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Global {
    /// Volumes.
    #[default]
    Volumes,
    /// Pans.
    Pans,
    /// The send to the return at this place among the returns, from 0.
    Send(usize),
}

/// What an encoder sets, for `push.state` (R42).
#[derive(Clone, Debug, PartialEq)]
pub struct ControlState {
    /// Its name.
    pub name: String,
    /// Its value as text.
    pub value: String,
    /// Where it sits from 0 to 1, for a continuous control.
    pub position: Option<f64>,
    /// Whether turning it does nothing.
    pub inert: bool,
}

/// One of the eight places, for `push.state` (R42).
#[derive(Clone, Debug, PartialEq)]
pub struct PlaceState {
    /// The track's or chain's name.
    pub name: String,
    /// The track, or the track the chain's rack is on.
    pub track: TrackId,
    /// The chain, if the place is one.
    pub chain: Option<ChainId>,
    /// Whether it is the selected place.
    pub selected: bool,
}

/// What `push.state` reads of Mix Mode (R42).
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    /// Track Mix's chosen view.
    pub view: View,
    /// What Global Mix sets.
    pub global: Global,
    /// How far the sends are shifted on the page shown.
    pub shift: usize,
    /// The eight places.
    pub places: Vec<PlaceState>,
    /// What each encoder sets, by column.
    pub encoders: Vec<Option<ControlState>>,
    /// The group tracks that are unfolded.
    pub unfolded_groups: Vec<TrackId>,
    /// The racks whose chains are unfolded.
    pub unfolded_racks: Vec<DeviceId>,
}

/// What an encoder turns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Param {
    Volume(TrackId),
    Pan(TrackId),
    SplitLeft(TrackId),
    SplitRight(TrackId),
    Send(TrackId, TrackId),
    ChainVolume(ChainId),
    ChainPan(ChainId),
    ChainSend(ChainId, ChainId),
}

impl Param {
    /// The command that moves the control to `position`, from 0 to 1 on its
    /// travel (R9).
    fn command(self, position: f64) -> (&'static str, Value) {
        let pan = (position * 2.0 - 1.0).clamp(-1.0, 1.0);
        match self {
            Self::Volume(track) => (
                "mixer.set_volume",
                json!({ "track": track, "volume": fader_gain(position) }),
            ),
            Self::Pan(track) => ("mixer.set_pan", json!({ "track": track, "pan": pan })),
            Self::SplitLeft(track) => (
                "mixer.set_split_pan_left",
                json!({ "track": track, "pan": pan }),
            ),
            Self::SplitRight(track) => (
                "mixer.set_split_pan_right",
                json!({ "track": track, "pan": pan }),
            ),
            Self::Send(track, to) => (
                "mixer.set_send",
                json!({ "track": track, "to": to, "level": send_level(position) }),
            ),
            Self::ChainVolume(chain) => (
                "chain.set_gain",
                json!({ "chain": chain, "volume": fader_gain(position) }),
            ),
            Self::ChainPan(chain) => ("chain.set_pan", json!({ "chain": chain, "pan": pan })),
            Self::ChainSend(chain, to) => (
                "chain.set_send",
                json!({ "chain": chain, "to": to, "level": send_level(position) }),
            ),
        }
    }
}

impl Param {
    /// The track and the automation target of a track's control; a rack
    /// chain's has none here.
    fn automation(self) -> Option<(TrackId, bl_model::AutomationTarget)> {
        use bl_model::{
            AutomationTarget::{Mixer, Send},
            MixerParam,
        };
        Some(match self {
            Self::Volume(t) => (
                t,
                Mixer {
                    param: MixerParam::Volume,
                },
            ),
            Self::Pan(t) => (
                t,
                Mixer {
                    param: MixerParam::Pan,
                },
            ),
            Self::SplitLeft(t) => (
                t,
                Mixer {
                    param: MixerParam::SplitLeft,
                },
            ),
            Self::SplitRight(t) => (
                t,
                Mixer {
                    param: MixerParam::SplitRight,
                },
            ),
            Self::Send(t, to) => (t, Send { to }),
            _ => return None,
        })
    }

    /// The control's value at `position` on its travel, as an envelope holds
    /// it.
    fn value(self, position: f64) -> f64 {
        let pan = (position * 2.0 - 1.0).clamp(-1.0, 1.0);
        match self {
            Self::Volume(_) | Self::ChainVolume(_) => fader_gain(position),
            Self::Send(..) | Self::ChainSend(..) => send_level(position),
            _ => pan,
        }
    }

    /// Where an envelope's `value` sits on the control's travel.
    fn position_of(self, value: f64) -> f64 {
        match self {
            Self::Volume(_) | Self::ChainVolume(_) => fader_position(value),
            Self::Send(..) | Self::ChainSend(..) => send_position(value),
            _ => (value + 1.0) / 2.0,
        }
    }

    /// Where the control rests with nothing set: 0 dB, centre, or no send.
    fn default_position(self) -> f64 {
        match self {
            Self::Volume(_) | Self::ChainVolume(_) => fader_position(1.0),
            Self::Send(..) | Self::ChainSend(..) => 0.0,
            _ => 0.5,
        }
    }
}

/// What an encoder does.
enum Kind {
    /// A continuous control; none when it is shown but inert (R33).
    Move(Option<Param>),
    /// A list stepped through its entries (R31).
    List(Route),
}

/// One encoder's control as the display shows it (R41).
struct Knob {
    name: String,
    value: String,
    graphic: Graphic,
    /// Where the control sits from 0 to 1.
    position: f64,
    kind: Kind,
}

fn db(gain: f64) -> String {
    format!("{} dB", volume_text(gain))
}

fn volume(name: &str, gain: f64, target: Param) -> Knob {
    let position = fader_position(gain);
    Knob {
        name: name.to_owned(),
        value: db(gain),
        graphic: Graphic::Fader(position as f32),
        position,
        kind: Kind::Move(Some(target)),
    }
}

fn pan(name: &str, pan: f64, target: Option<Param>) -> Knob {
    let position = (pan + 1.0) / 2.0;
    Knob {
        name: name.to_owned(),
        value: pan_text(pan),
        graphic: Graphic::Arc(position as f32),
        position,
        kind: Kind::Move(target),
    }
}

fn send(name: String, level: f64, target: Param) -> Knob {
    let position = send_position(level);
    Knob {
        name,
        value: db(level),
        graphic: Graphic::Arc(position as f32),
        position,
        kind: Kind::Move(Some(target)),
    }
}

/// The rack on a track whose chains unfold: an instrument or drum rack of
/// more than one chain (R37).
fn rack_of(doc: &Document, track: TrackId) -> Option<&Device> {
    doc.track(track)?.devices().find(|d| {
        matches!(d.rack_kind(), Some(RackKind::Instrument | RackKind::Drum)) && d.chains.len() > 1
    })
}

/// The tracks inside a group, at any depth.
fn inside(doc: &Document, group: TrackId, found: &mut Vec<TrackId>) {
    for child in doc.track(group).into_iter().flat_map(|t| t.children()) {
        found.push(child);
        inside(doc, child, found);
    }
}

/// Every place there is, in order: the tracks of the ring's list (the
/// returns end it, Main is never in it, R34), each followed by its rack's
/// chains when they are unfolded (R37).
fn places(model: &ReadModel) -> Vec<Place> {
    let doc = &model.document;
    let mut out = Vec::new();
    for track in model.columns.iter().copied() {
        out.push(Place::Track(track));
        if let Some(rack) = rack_of(doc, track)
            && model.mix.open_racks.contains(&rack.id)
        {
            out.extend(rack.chains.iter().map(|chain| Place::Chain {
                track,
                rack: rack.id,
                chain: chain.id,
            }));
        }
    }
    out
}

/// The eight places shown.
struct Layout {
    places: Vec<Place>,
    window: Vec<Place>,
}

/// The lowest solo/mute commands one click on a switch runs: every selected
/// track if the clicked one is selected (mixer view R8).
fn reach(model: &ReadModel, track: TrackId, has: impl Fn(TrackId) -> bool) -> Vec<TrackId> {
    let mut tracks = vec![track];
    if model.selection.tracks.contains(&track) {
        tracks.extend(
            model
                .selection
                .tracks
                .iter()
                .copied()
                .filter(|&t| t != track && has(t)),
        );
    }
    tracks
}

/// Mix Mode's content.
pub struct Mix {
    view: View,
    global: Global,
    track_shift: usize,
    global_shift: usize,
    /// Where the eight places start while chains are unfolded; the ring's
    /// first track otherwise.
    start: Option<usize>,
    /// The ring's first track when `start` was set.
    seen: Option<TrackId>,
    /// The chain Track Mix mixes, if one was chosen.
    chain: Option<Place>,
    moving: [Moving; 8],
    counts: [Counting; 8],
    /// The entry a list was last stepped to, until the document shows it.
    stepped: [Option<(usize, Instant)>; 8],
}

impl Default for Mix {
    fn default() -> Self {
        Self::new()
    }
}

impl Mix {
    /// Mix Mode before it is entered.
    #[must_use]
    pub fn new() -> Self {
        Self {
            view: View::Mix,
            global: Global::Volumes,
            track_shift: 0,
            global_shift: 0,
            start: None,
            seen: None,
            chain: None,
            moving: Default::default(),
            counts: Default::default(),
            stepped: Default::default(),
        }
    }

    /// The ring's first track, and where it is among the places.
    fn ring(model: &ReadModel, row: usize, places: &[Place]) -> (Option<TrackId>, usize) {
        let first = model
            .row(row)
            .and_then(|r| r.ring.as_ref())
            .and_then(|ring| ring.first_track);
        let at = first
            .and_then(|track| places.iter().position(|p| *p == Place::Track(track)))
            .unwrap_or(0);
        (first, at)
    }

    /// Keeps `start` where the ring says while no chain is unfolded, and
    /// starts it afresh when the ring moves under it.
    fn sync(&mut self, model: &ReadModel, row: usize) {
        let places = places(model);
        let (first, at) = Self::ring(model, row, &places);
        if !places.iter().any(|p| matches!(p, Place::Chain { .. })) {
            self.start = None;
            self.seen = first;
        } else if self.start.is_none() || self.seen != first {
            self.start = Some(at);
            self.seen = first;
        }
    }

    fn layout(&self, model: &ReadModel, row: usize) -> Layout {
        let places = places(model);
        let (_, at) = Self::ring(model, row, &places);
        let start = self
            .start
            .unwrap_or(at)
            .min(places.len().saturating_sub(PLACES));
        let window = places.iter().skip(start).take(PLACES).copied().collect();
        Layout { places, window }
    }

    /// What the page shows: the selected place, a chain if one was chosen
    /// and its track is still the selected one (R38).
    fn subject(&self, model: &ReadModel) -> Option<Place> {
        let track = *model.selection.tracks.first()?;
        let doc = &model.document;
        match self.chain {
            // A chain folded back in is no longer a place to mix.
            Some(Place::Chain {
                track: t,
                rack,
                chain,
            }) if t == track
                && doc.chain(chain).is_some()
                && model.mix.open_racks.contains(&rack) =>
            {
                self.chain
            }
            _ => doc.track(track).map(|_| Place::Track(track)),
        }
    }

    fn selected(&self, model: &ReadModel, place: Place) -> bool {
        let subject = self.subject(model);
        match place {
            Place::Track(track) => {
                model.selection.tracks.contains(&track)
                    && !matches!(subject, Some(Place::Chain { track: t, .. }) if t == track)
            }
            chain => subject == Some(chain),
        }
    }

    /// The controls the selected place has in Track Mix: the front ones
    /// (volume and pan) and every send (R29, R30, R38).
    fn controls(model: &ReadModel, place: Place) -> (Vec<Knob>, Vec<Knob>) {
        let doc = &model.document;
        let mut front = Vec::new();
        let mut sends = Vec::new();
        match place {
            Place::Track(id) => {
                let Some(track) = doc.track(id) else {
                    return (front, sends);
                };
                let mixer = track.mixer();
                front.push(volume("Volume", mixer.volume, Param::Volume(id)));
                if mixer.pan_mode == PanMode::Split {
                    front.push(pan("Pan L", mixer.split_left, Some(Param::SplitLeft(id))));
                    front.push(pan("Pan R", mixer.split_right, Some(Param::SplitRight(id))));
                } else {
                    front.push(pan("Pan", mixer.pan, Some(Param::Pan(id))));
                }
                if track.kind().has_sends() {
                    for (n, to) in doc.returns().enumerate() {
                        sends.push(send(
                            format!("Send {}", return_letter(n)),
                            track.send(to),
                            Param::Send(id, to),
                        ));
                    }
                }
            }
            Place::Chain { rack, chain, .. } => {
                let (Some(rack), Some(chain)) = (doc.device(rack), doc.chain(chain)) else {
                    return (front, sends);
                };
                if rack
                    .rack_kind()
                    .is_some_and(RackKind::chains_have_volume_and_pan)
                {
                    front.push(volume(
                        "Volume",
                        chain.mixer.volume,
                        Param::ChainVolume(chain.id),
                    ));
                    front.push(pan("Pan", chain.mixer.pan, Some(Param::ChainPan(chain.id))));
                }
                if rack.rack_kind() == Some(RackKind::Drum) && !chain.is_return {
                    for (n, ret) in rack.chains.iter().filter(|c| c.is_return).enumerate() {
                        sends.push(send(
                            format!("Send {}", return_letter(n)),
                            chain.send(ret.id),
                            Param::ChainSend(chain.id, ret.id),
                        ));
                    }
                }
            }
        }
        (front, sends)
    }

    /// How many sends fit among the encoders after the front controls.
    const fn slots(front: usize) -> usize {
        PLACES - front
    }

    /// The control of `place` that Global Mix's `global` sets (R32, R33,
    /// R38).
    fn global_knob(model: &ReadModel, place: Place, global: Global) -> Option<Knob> {
        let doc = &model.document;
        match place {
            Place::Track(id) => {
                let track = doc.track(id)?;
                let mixer = track.mixer();
                match global {
                    Global::Volumes => Some(volume("Volume", mixer.volume, Param::Volume(id))),
                    // A track in Split Stereo Pan Mode shows its pan but its
                    // encoder does nothing.
                    Global::Pans if mixer.pan_mode == PanMode::Split => {
                        Some(pan("Pan", mixer.pan, None))
                    }
                    Global::Pans => Some(pan("Pan", mixer.pan, Some(Param::Pan(id)))),
                    Global::Send(n) if track.kind().has_sends() => {
                        let to = doc.returns().nth(n)?;
                        Some(send(
                            format!("Send {}", return_letter(n)),
                            track.send(to),
                            Param::Send(id, to),
                        ))
                    }
                    Global::Send(_) => None,
                }
            }
            Place::Chain { rack, chain, .. } => {
                let (rack, chain) = (doc.device(rack)?, doc.chain(chain)?);
                let mixes = rack
                    .rack_kind()
                    .is_some_and(RackKind::chains_have_volume_and_pan);
                match global {
                    Global::Volumes if mixes => Some(volume(
                        "Volume",
                        chain.mixer.volume,
                        Param::ChainVolume(chain.id),
                    )),
                    Global::Pans if mixes => {
                        Some(pan("Pan", chain.mixer.pan, Some(Param::ChainPan(chain.id))))
                    }
                    Global::Send(n)
                        if rack.rack_kind() == Some(RackKind::Drum) && !chain.is_return =>
                    {
                        let ret = rack.chains.iter().filter(|c| c.is_return).nth(n)?;
                        Some(send(
                            format!("Send {}", return_letter(n)),
                            chain.send(ret.id),
                            Param::ChainSend(chain.id, ret.id),
                        ))
                    }
                    _ => None,
                }
            }
        }
    }

    /// A list's knob (R31).
    fn list(route: &Route) -> Option<Knob> {
        (!route.entries.is_empty()).then(|| Knob {
            name: route.name.clone(),
            value: route.text.clone(),
            graphic: Graphic::Choice(route.text.clone()),
            position: 0.0,
            kind: Kind::List(route.clone()),
        })
    }

    /// What each of the eight encoders sets on the page shown.
    fn knobs(&self, model: &ReadModel, layout: &Layout, page: MixPage) -> Vec<Option<Knob>> {
        let mut knobs: Vec<Option<Knob>> = (0..PLACES).map(|_| None).collect();
        match page {
            MixPage::Global => {
                for (column, place) in layout.window.iter().enumerate() {
                    knobs[column] = Self::global_knob(model, *place, self.global);
                }
            }
            MixPage::Track => match (self.view, self.subject(model)) {
                (View::Mix, Some(place)) => {
                    let (front, sends) = Self::controls(model, place);
                    let slots = Self::slots(front.len());
                    let shift = self.track_shift.min(sends.len().saturating_sub(slots));
                    let shown = front.into_iter().chain(sends.into_iter().skip(shift));
                    for (slot, knob) in knobs.iter_mut().zip(shown) {
                        *slot = Some(knob);
                    }
                }
                (View::InOut, Some(Place::Track(track))) => {
                    if let Some(in_out) = model.mix.in_out.as_ref().filter(|io| io.track == track) {
                        for (slot, route) in knobs.iter_mut().zip(&in_out.choosers) {
                            *slot = route.as_ref().and_then(Self::list);
                        }
                    }
                }
                _ => {}
            },
        }
        knobs
    }

    /// How many returns the sends of the page shown run along, and how many
    /// places there are for them (R29, R32).
    fn send_range(&self, model: &ReadModel, page: MixPage) -> (usize, usize) {
        let returns = model.document.returns().len();
        match page {
            MixPage::Global => {
                // The buttons also reach the return chains of an unfolded
                // drum rack (R38).
                let sends = places(model)
                    .into_iter()
                    .filter_map(|place| match place {
                        Place::Chain { rack, .. } => model.document.device(rack),
                        Place::Track(_) => None,
                    })
                    .filter(|rack| rack.rack_kind() == Some(RackKind::Drum))
                    .map(|rack| rack.chains.iter().filter(|c| c.is_return).count())
                    .fold(returns, usize::max);
                (sends, if sends > 6 { 5 } else { sends })
            }
            MixPage::Track => {
                let (front, sends) = self.subject(model).map_or((2, 0), |place| {
                    let (front, sends) = Self::controls(model, place);
                    (front.len(), sends.len())
                });
                (sends, Self::slots(front))
            }
        }
    }

    /// The shift the page shown has, kept within the sends there are.
    fn shift(&self, model: &ReadModel, page: MixPage) -> usize {
        let (sends, slots) = self.send_range(model, page);
        match page {
            MixPage::Track => self.track_shift,
            MixPage::Global => self.global_shift,
        }
        .min(sends.saturating_sub(slots))
    }

    // ----- the buttons -----

    fn upper(&mut self, column: u8, model: &ReadModel, page: MixPage) {
        let (sends, slots) = self.send_range(model, page);
        let arrows = sends > slots;
        match page {
            MixPage::Track => match column {
                0 => self.view = View::Mix,
                1 => self.view = View::InOut,
                6 if arrows && self.view == View::Mix => {
                    self.track_shift = self.shift(model, page).saturating_sub(1);
                }
                7 if arrows && self.view == View::Mix => {
                    self.track_shift = (self.shift(model, page) + 1).min(sends - slots);
                }
                _ => {}
            },
            MixPage::Global => {
                let shift = self.shift(model, page);
                match column {
                    0 => self.global = Global::Volumes,
                    1 => self.global = Global::Pans,
                    7 if arrows => {
                        // Round to Send A after the last (R32).
                        self.global_shift = if shift + 1 > sends - slots {
                            0
                        } else {
                            shift + 1
                        };
                    }
                    c if usize::from(c) < 2 + slots => {
                        self.global = Global::Send(shift + usize::from(c) - 2);
                    }
                    _ => {}
                }
            }
        }
    }

    /// The commands one action runs, as one undo step.
    fn run_all(ctx: &mut Ctx<'_>, commands: Vec<(&str, Value)>) {
        let last = commands.len().saturating_sub(1);
        for (n, (id, args)) in commands.into_iter().enumerate() {
            if last == 0 {
                ctx.cx.run(id, args);
            } else if n < last {
                ctx.cx.run_held(ACTION_GESTURE, id, args);
            } else {
                ctx.cx.run_released(ACTION_GESTURE, id, args);
            }
        }
    }

    /// A click on a track's Track Activator: every track it reaches goes to
    /// the clicked one's other state (R39).
    fn mute(model: &ReadModel, track: TrackId) -> Vec<(&'static str, Value)> {
        let doc = &model.document;
        let has = |t: TrackId| doc.track(t).is_some_and(|t| t.kind().has_mute_and_solo());
        let Some(clicked) = doc.track(track).filter(|t| t.kind().has_mute_and_solo()) else {
            return Vec::new();
        };
        let mute = !clicked.mixer().mute;
        reach(model, track, has)
            .into_iter()
            .map(|t| ("mixer.set_mute", json!({ "track": t, "mute": mute })))
            .collect()
    }

    /// A click on a track's Solo, as the window's: it takes the others off,
    /// unless it is the only one on, when it goes off (R39, mixer R15).
    fn solo(model: &ReadModel, track: TrackId) -> Vec<(&'static str, Value)> {
        let doc = &model.document;
        let has = |t: TrackId| doc.track(t).is_some_and(|t| t.kind().has_mute_and_solo());
        let Some(clicked) = doc.track(track).filter(|t| t.kind().has_mute_and_solo()) else {
            return Vec::new();
        };
        let tracks = reach(model, track, has);
        let others = doc
            .tracks()
            .any(|t| !tracks.contains(&t.id()) && t.mixer().solo);
        if clicked.mixer().solo && !others {
            tracks
                .into_iter()
                .map(|t| ("mixer.set_solo", json!({ "track": t, "solo": false })))
                .collect()
        } else {
            let mut commands = vec![("mixer.solo_exclusive", json!({ "track": track }))];
            commands.extend(
                tracks
                    .into_iter()
                    .skip(1)
                    .map(|t| ("mixer.set_solo", json!({ "track": t, "solo": true }))),
            );
            commands
        }
    }

    /// A lower display button pressed with a modifier held (R39, R40, R27).
    fn combine(&mut self, place: Place, ctx: &mut Ctx<'_>) {
        let Some(model) = ctx.model else {
            return;
        };
        let m = ctx.modifiers;
        let commands = match place {
            Place::Track(track) if m.has(cc::DELETE) => {
                if model.document.main_id() == track {
                    return;
                }
                vec![("track.delete", json!({ "track": track }))]
            }
            Place::Track(track) if m.has(cc::DUPLICATE) => {
                vec![
                    ("arrangement.select_tracks", json!({ "tracks": [track] })),
                    ("track.duplicate", json!({})),
                ]
            }
            Place::Track(track) if m.has(cc::MUTE) => Self::mute(model, track),
            Place::Track(track) if m.has(cc::SOLO) => Self::solo(model, track),
            Place::Track(track) if m.shift() => {
                ctx.open_picker(Target::Track(track));
                return;
            }
            Place::Chain { chain, .. } if m.shift() => {
                let colour = model.document.chain(chain).and_then(|c| c.colour);
                ctx.open_picker(Target::Command {
                    command: "chain.set_colour".to_owned(),
                    args: json!({ "chain": chain }),
                    current: colour,
                });
                return;
            }
            _ => return,
        };
        Self::run_all(ctx, commands);
    }

    /// A lower display button tapped: selects its place; a selected track's
    /// folds or unfolds (R37).
    fn tap(&mut self, place: Place, ctx: &mut Ctx<'_>) {
        let Some(model) = ctx.model else {
            return;
        };
        match place {
            Place::Track(track) => {
                let selected = model.selection.tracks.contains(&track);
                self.chain = None;
                if !selected {
                    ctx.select_track(track);
                } else if model.document.track(track).map(bl_model::Track::kind)
                    == Some(TrackKind::Group)
                {
                    let folded = model.mix.folded_groups.contains(&track);
                    ctx.cx
                        .run("arrangement.fold_groups", json!({ "fold": !folded }));
                } else if let Some(rack) = rack_of(&model.document, track) {
                    let open = model.mix.open_racks.contains(&rack.id);
                    ctx.cx.run(
                        "mixer_view.fold_chains",
                        json!({ "rack": rack.id, "shown": !open }),
                    );
                }
            }
            Place::Chain { track, chain, .. } => {
                if !model.selection.tracks.contains(&track) {
                    ctx.select_track(track);
                }
                ctx.cx.run("chain.select", json!({ "chain": chain }));
                self.chain = Some(place);
            }
        }
    }

    // ----- painting -----

    fn paint_lower(&self, paint: &mut Paint<'_>, layout: &Layout) {
        let model = paint.model;
        let doc = &model.document;
        let swatch = |index: u8| bl_model::swatch_rgb(paint.colours, index);
        for (column, place) in layout.window.iter().enumerate() {
            let Some(track) = doc.track(place.track()) else {
                continue;
            };
            let (name, colour, icon) = match *place {
                Place::Track(_) => {
                    let icon = if track.kind() == TrackKind::Group {
                        Icon::Group
                    } else if rack_of(doc, place.track()).is_some() {
                        Icon::Rack
                    } else {
                        Icon::None
                    };
                    (track.name().to_owned(), swatch(track.colour()), icon)
                }
                Place::Chain { chain, .. } => {
                    let Some(chain) = doc.chain(chain) else {
                        continue;
                    };
                    (
                        chain.name.clone(),
                        swatch(chain.colour.unwrap_or_else(|| track.colour())),
                        Icon::None,
                    )
                }
            };
            paint.screen.lower[column] =
                Label::new(name, colour, self.selected(model, *place)).with_icon(icon);
            if let Place::Track(id) = place
                && let Some((_, fraction)) = model.progress.iter().find(|(t, _)| t == id)
            {
                paint.screen.progress[column] = Some(*fraction);
            }
            // A line under an unfolded group or rack runs to the last of what
            // it holds (R37).
            let held: Vec<TrackId> = match *place {
                Place::Track(id) if track.kind() == TrackKind::Group => {
                    let mut found = Vec::new();
                    inside(doc, id, &mut found);
                    found
                }
                _ => Vec::new(),
            };
            let members = layout.window[column + 1..]
                .iter()
                .take_while(|p| match (*place, **p) {
                    (Place::Track(id), Place::Chain { track, .. }) => track == id,
                    (Place::Track(_), Place::Track(t)) => held.contains(&t),
                    _ => false,
                })
                .count();
            if members > 0 && matches!(place, Place::Track(_)) {
                let last = u8::try_from(column + members).unwrap_or(7);
                paint
                    .screen
                    .underlines
                    .push((u8::try_from(column).unwrap_or(0), last, colour));
            }
        }
    }

    fn paint_upper(&self, paint: &mut Paint<'_>) {
        let model = paint.model;
        let label = |text: &str, selected: bool| Label::new(text, WHITE, selected);
        let (sends, slots) = self.send_range(model, paint.mix_page);
        let arrows = sends > slots;
        let letter = |n: usize| format!("Send {}", return_letter(n));
        match paint.mix_page {
            MixPage::Track => {
                paint.screen.upper[0] = label("Mix", self.view == View::Mix);
                paint.screen.upper[1] = label("Input & Output", self.view == View::InOut);
                if arrows && self.view == View::Mix {
                    paint.screen.upper[6] = label("<", false);
                    paint.screen.upper[7] = label(">", false);
                }
            }
            MixPage::Global => {
                let shift = self.shift(model, paint.mix_page);
                paint.screen.upper[0] = label("Volumes", self.global == Global::Volumes);
                paint.screen.upper[1] = label("Pans", self.global == Global::Pans);
                for slot in 0..slots {
                    let n = shift + slot;
                    paint.screen.upper[2 + slot] =
                        label(&letter(n), self.global == Global::Send(n));
                }
                if arrows {
                    paint.screen.upper[7] = label(">", false);
                }
            }
        }
    }

    fn paint_buttons(paint: &mut Paint<'_>) {
        let model = paint.model;
        let doc = &model.document;
        let first = model.selection.tracks.first().and_then(|t| doc.track(*t));
        let mut light = |button: u8, on: bool, rgb: [u8; 3]| {
            if on {
                let index = paint.lights.colour(rgb);
                paint.lights.button(button, Led::steady(index));
            }
        };
        light(cc::MUTE, first.is_some_and(|t| t.mixer().mute), MUTED);
        light(cc::SOLO, first.is_some_and(|t| t.mixer().solo), SOLOED);
        paint.lights.white(cc::DELETE, true, false);
        paint.lights.white(cc::DUPLICATE, true, false);
    }
}

impl Mix {
    /// Delete and a touch: the control's automation goes, or the control
    /// goes back to where it rests where it has none (R62).
    fn delete_automation(&mut self, column: u8, ctx: &mut Ctx<'_>) {
        let Some(model) = ctx.model else {
            return;
        };
        let row = ctx.cx.row();
        self.sync(model, row);
        let layout = self.layout(model, row);
        let mut knobs = self.knobs(model, &layout, ctx.mix_page);
        let Some(Kind::Move(Some(param))) = knobs
            .get_mut(usize::from(column))
            .and_then(Option::take)
            .map(|knob| knob.kind)
        else {
            return;
        };
        if let Some((track, target)) = param.automation()
            && crate::automation::delete(ctx, model, track, &target)
        {
            return;
        }
        let (id, args) = param.command(param.default_position());
        ctx.run_turn(column, id, args);
    }
}

impl Content for Mix {
    fn name(&self) -> &str {
        "Mix"
    }

    fn button(&mut self, button: u8, gesture: Gesture, ctx: &mut Ctx<'_>) {
        let Some(model) = ctx.model else {
            return;
        };
        let row = ctx.cx.row();
        self.sync(model, row);
        if let Some(column) = controls::lower_column(button) {
            let layout = self.layout(model, row);
            let Some(&place) = layout.window.get(usize::from(column)) else {
                return;
            };
            match gesture {
                Gesture::Press if ctx.modifiers.any() => self.combine(place, ctx),
                Gesture::Tap if !ctx.modifiers.any() => self.tap(place, ctx),
                _ => {}
            }
            return;
        }
        if gesture == Gesture::Press
            && let Some(column) = controls::upper_column(button)
        {
            self.upper(column, model, ctx.mix_page);
            return;
        }
        match (button, gesture) {
            (cc::MUTE, Gesture::Tap) => {
                if let Some(&track) = model.selection.tracks.first() {
                    Self::run_all(ctx, Self::mute(model, track));
                }
            }
            (cc::SOLO, Gesture::Tap) => {
                if let Some(&track) = model.selection.tracks.first() {
                    Self::run_all(ctx, Self::solo(model, track));
                }
            }
            (cc::LEFT | cc::RIGHT, Gesture::Press) if ctx.mix_page == MixPage::Global => {
                let layout = self.layout(model, row);
                if layout.places.len() <= PLACES {
                    return;
                }
                let left = button == cc::LEFT;
                if let Some(start) = self.start {
                    let last = layout.places.len() - PLACES;
                    self.start = Some(if left {
                        start.saturating_sub(1)
                    } else {
                        (start + 1).min(last)
                    });
                } else {
                    let step = if left { "left" } else { "right" };
                    ctx.cx.run(
                        "control_surface.move_ring",
                        json!({ "row": row + 1, "step": step }),
                    );
                }
            }
            _ => {}
        }
    }

    fn pad_select(&mut self, pad: Pad, ctx: &mut Ctx<'_>) -> bool {
        let Some(model) = ctx.model else {
            return false;
        };
        let (Some(drum), Some(&track)) = (model.drum, model.selection.tracks.first()) else {
            return false;
        };
        if !model.mix.open_racks.contains(&drum.rack) || pad.row >= 4 || pad.column >= 4 {
            return false;
        }
        let note = drum.first.saturating_add(4 * pad.row + pad.column);
        ctx.cx
            .run("pad.select", json!({ "rack": drum.rack, "note": note }));
        let row = ctx.cx.row();
        self.sync(model, row);
        let chain = model.document.device(drum.rack).and_then(|rack| {
            rack.chains
                .iter()
                .find(|c| !c.is_return && c.receive == Receive::Note(note))
        });
        if let Some(chain) = chain {
            let place = Place::Chain {
                track,
                rack: drum.rack,
                chain: chain.id,
            };
            self.chain = Some(place);
            // The eight shown move to it if they don't hold it.
            let layout = self.layout(model, row);
            if !layout.window.contains(&place)
                && let Some(at) = layout.places.iter().position(|p| *p == place)
            {
                self.start = Some(at.min(layout.places.len().saturating_sub(PLACES)));
            }
        }
        true
    }

    fn encoder(&mut self, column: u8, turn: Turn, ctx: &mut Ctx<'_>) {
        let col = usize::from(column);
        let Turn::Steps(steps) = turn else {
            self.moving[col].touch();
            self.counts[col].reset();
            self.stepped[col] = None;
            if turn == Turn::Touch && ctx.modifiers.has(cc::DELETE) {
                self.delete_automation(column, ctx);
            }
            return;
        };
        let Some(model) = ctx.model else {
            return;
        };
        let row = ctx.cx.row();
        self.sync(model, row);
        let layout = self.layout(model, row);
        let mut knobs = self.knobs(model, &layout, ctx.mix_page);
        let Some(knob) = knobs.get_mut(col).and_then(Option::take) else {
            return;
        };
        let now = ctx.cx.now();
        match knob.kind {
            Kind::Move(Some(target)) => {
                // Spec: push-2-notes R66. With steps held, the turn is the
                // value at the first held step, written into their span.
                let held = target.automation().filter(|_| !ctx.held_steps.is_empty());
                let from = held
                    .as_ref()
                    .and_then(|(_, at)| {
                        crate::automation::value_at_first_step(model, ctx.held_steps, at)
                    })
                    .map_or(knob.position, |value| target.position_of(value));
                let position = self.moving[col].turn(now, from, steps, ctx.modifiers.shift());
                if let Some((_, at)) = held {
                    if let Some(clip) = crate::automation::edited_clip(model).map(|c| c.id) {
                        let gesture = crate::surface::ENCODER_GESTURE + u32::from(column);
                        crate::automation::write_steps(
                            ctx,
                            gesture,
                            clip,
                            &at,
                            target.value(position),
                        );
                        ctx.gesture(column);
                    }
                    return;
                }
                let (id, args) = target.command(position);
                ctx.run_turn(column, id, args);
            }
            Kind::Move(None) => {}
            Kind::List(route) => {
                let by = self.counts[col].take(steps, LIST_STEPS);
                if by == 0 {
                    return;
                }
                let chosen = route.entries.iter().position(|e| e.chosen);
                let base = match self.stepped[col] {
                    Some((at, when)) if now.saturating_duration_since(when) < REST => Some(at),
                    _ => chosen,
                };
                let last = isize::try_from(route.entries.len() - 1).unwrap_or(0);
                // Nothing chosen counts as the place before the first entry.
                let from = base.map_or(-1, |at| isize::try_from(at).unwrap_or(0));
                let at = usize::try_from((from + isize::try_from(by).unwrap_or(0)).clamp(0, last))
                    .unwrap_or(0);
                self.stepped[col] = Some((at, now));
                if Some(at) != chosen {
                    let entry = &route.entries[at];
                    ctx.run_turn(column, &entry.command, entry.args.clone());
                }
            }
        }
    }

    fn paint(&mut self, paint: &mut Paint<'_>) {
        let model = paint.model;
        self.sync(model, paint.row);
        let layout = self.layout(model, paint.row);
        self.paint_lower(paint, &layout);
        let held_steps = paint.held_steps;
        self.paint_upper(paint);
        Self::paint_buttons(paint);
        for (column, knob) in self
            .knobs(model, &layout, paint.mix_page)
            .into_iter()
            .enumerate()
        {
            if let Some(knob) = knob {
                // Spec: push-2-notes R64. The automation's dot; R66, with
                // steps held the value at the first.
                let automated = match knob.kind {
                    Kind::Move(Some(param)) => param.automation().map(|a| (param, a)),
                    _ => None,
                };
                let dot = automated
                    .as_ref()
                    .map_or(crate::screen::Dot::None, |(_, (track, at))| {
                        crate::automation::dot(model, *track, at)
                    });
                let graphic = automated
                    .as_ref()
                    .and_then(|(param, (_, at))| {
                        crate::automation::value_at_first_step(model, held_steps, at)
                            .map(|value| Graphic::Arc(param.position_of(value) as f32))
                    })
                    .unwrap_or(knob.graphic);
                paint.screen.columns[column] = Column {
                    name: knob.name,
                    value: knob.value,
                    graphic,
                    dot,
                    ..Column::default()
                };
            }
        }
    }

    fn claims_lower(&self) -> bool {
        true
    }

    fn mix_state(&self, model: &ReadModel, page: MixPage, row: usize) -> Option<State> {
        let layout = self.layout(model, row);
        let encoders = self
            .knobs(model, &layout, page)
            .into_iter()
            .map(|knob| {
                knob.map(|knob| ControlState {
                    inert: matches!(knob.kind, Kind::Move(None)),
                    position: matches!(knob.kind, Kind::Move(_)).then_some(knob.position),
                    name: knob.name,
                    value: knob.value,
                })
            })
            .collect();
        let places = layout
            .window
            .iter()
            .filter_map(|&place| {
                let doc = &model.document;
                let name = match place {
                    Place::Track(track) => doc.track(track)?.name().to_owned(),
                    Place::Chain { chain, .. } => doc.chain(chain)?.name.clone(),
                };
                Some(PlaceState {
                    name,
                    track: place.track(),
                    chain: match place {
                        Place::Chain { chain, .. } => Some(chain),
                        Place::Track(_) => None,
                    },
                    selected: self.selected(model, place),
                })
            })
            .collect();
        Some(State {
            view: self.view,
            global: self.global,
            shift: self.shift(model, page),
            places,
            encoders,
            unfolded_groups: model
                .document
                .tracks()
                .filter(|t| {
                    t.kind() == TrackKind::Group
                        && t.children().len() > 0
                        && !model.mix.folded_groups.contains(&t.id())
                })
                .map(bl_model::Track::id)
                .collect(),
            unfolded_racks: model.mix.open_racks.clone(),
        })
    }
}

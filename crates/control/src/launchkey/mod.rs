//! The Launchkey MK3 and Launchkey MK4 surfaces
//! (`docs/specs/supported-controllers.md`, R51 to R71).
//!
//! One surface plays both generations: sixteen pads that are a ring eight
//! tracks wide and two scenes tall, pots (MK3) or encoders (MK4) that reach
//! the device in focus or the ring's tracks, faders and fader buttons, a
//! transport and workflow row of buttons, and the controller's display. What
//! a control reaches is the shell's to work out when a message arrives
//! ([`Cx::control`]); what the surface shows comes from the read model on
//! each tick, and only what changed is sent.

use std::collections::BTreeMap;
use std::time::Instant;

use bl_model::fader::gain_db;
use bl_model::{DeviceId, Document, Scale, ScaleName, TrackId};

use crate::grid::palette::{colour, nearest};
use crate::grid::{Grid, Light, TrackFn};
use crate::surface::{
    ControlSurface, Cx, Kind, ReadModel, Recognise, Ring, RingView, Role, SurfaceState,
};

mod events;
#[cfg(feature = "testing")]
pub mod sim;

/// The ring: eight tracks wide, two scenes tall (R52, R62).
const RING: Ring = Ring {
    tracks: 8,
    scenes: 2,
};

/// The maker's bytes in the device inquiry's reply.
const MAKER: [u8; 3] = [0x00, 0x20, 0x29];

/// The first Session pad's note: the top row is 60h to 67h and the bottom
/// 70h to 77h (R52).
const PAD_TOP: u8 = 0x60;

/// The drum pads' notes, 36 to 51 (R59, R70).
const DRUM: std::ops::Range<u8> = 36..52;

/// Which generation of the controller a surface drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gen {
    Mk3,
    Mk4,
}

/// The Launchkey MK3 as the chooser offers it (R1, R51).
#[must_use]
pub fn mk3() -> Kind {
    kind("launchkey-mk3", "Launchkey MK3", make_mk3, |port| {
        port.contains("lkmk3 daw") || (port.contains("launchkey mk3") && port.contains("daw"))
    })
}

/// The Launchkey MK4 as the chooser offers it (R1, R61).
#[must_use]
pub fn mk4() -> Kind {
    kind("launchkey-mk4", "Launchkey MK4", make_mk4, |port| {
        port.contains("launchkey")
            && port.contains("daw")
            && !port.contains("mk3")
            && !port.contains("lkmk3")
    })
}

fn kind(
    id: &'static str,
    name: &'static str,
    make: fn() -> Box<dyn ControlSurface>,
    test: fn(&str) -> bool,
) -> Kind {
    Kind::new(id, name, true, true, Some(RING), Vec::new(), make).recognised(Recognise {
        port: "daw",
        test: Some(test),
        maker: &MAKER,
        pair: false,
    })
}

fn make_mk3() -> Box<dyn ControlSurface> {
    Box::new(Keys::new(Gen::Mk3))
}

fn make_mk4() -> Box<dyn ControlSurface> {
    Box::new(Keys::new(Gen::Mk4))
}

/// Where a light is: a Session pad (channels 1 to 3), a drum pad (channels
/// 10 to 12) or a button's controller (channels 1 to 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layer {
    /// A Session pad's note.
    Pad,
    /// A drum pad's light index.
    Drum,
    /// A button's controller number.
    Button,
}

/// The messages that give a pad or button a light, or put it out (R52).
fn light_bytes(layer: Layer, number: u8, light: Option<Light>) -> Vec<Vec<u8>> {
    let base = match layer {
        Layer::Pad => 0x90,
        Layer::Drum => 0x99,
        Layer::Button => 0xB0,
    };
    match light {
        None => vec![vec![base, number, 0]],
        Some(Light::Static(c)) => vec![vec![base, number, c]],
        Some(Light::Flash { a, b }) => vec![vec![base, number, a], vec![base + 1, number, b]],
        Some(Light::Pulse(c)) => vec![vec![base + 2, number, c]],
    }
}

/// What the pads show, as the controller last reported (R52, R56, R59, R60).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pads {
    Session,
    Drum,
    DeviceSelect,
    /// One of the controller's own modes.
    Own,
}

/// What the pots or encoders reach, as the controller last reported (R54,
/// R64, R65).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pots {
    Device,
    Volume,
    Pan,
    /// A send, from 0.
    Send(u8),
    /// The MK4's relative Transport encoders.
    Transport,
    Own,
}

/// What the faders reach (R55, R66).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Faders {
    Volume,
    Device,
    Send(u8),
    Own,
}

/// The track function of the bottom row, stepped by the third button (R53,
/// R63).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cycle {
    Clips,
    Stop,
    Solo,
    Mute,
}

impl Cycle {
    /// The next function: the MK3 goes Stop, Solo, Mute, the MK4 Stop, Mute,
    /// Solo.
    const fn next(self, family: Gen) -> Self {
        match (self, family) {
            (Self::Clips, _) => Self::Stop,
            (Self::Stop, Gen::Mk3) | (Self::Mute, Gen::Mk4) => Self::Solo,
            (Self::Stop, Gen::Mk4) | (Self::Solo, Gen::Mk3) => Self::Mute,
            _ => Self::Clips,
        }
    }

    const fn track_fn(self) -> Option<TrackFn> {
        match self {
            Self::Clips => None,
            Self::Stop => Some(TrackFn::Stop),
            Self::Solo => Some(TrackFn::Solo),
            Self::Mute => Some(TrackFn::Mute),
        }
    }

    const fn colour(self) -> u8 {
        match self {
            Self::Clips => colour::WHITE,
            Self::Stop => colour::RED,
            Self::Solo => colour::BLUE,
            Self::Mute => colour::YELLOW,
        }
    }
}

/// The MK4's encoder pages that hold more than one thing (R64).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Volume,
    Pan,
}

/// What the surface knows of the controller's state, and what it has told
/// it.
pub(crate) struct Keys {
    family: Gen,
    /// The byte after the maker's in the SysEx header (R51, R61).
    header: u8,
    /// Whether the controller has faders (R51, R66).
    faders: bool,
    row: usize,
    pads: Pads,
    pots: Pots,
    fader_mode: Faders,
    cycle: Cycle,
    /// What the fader buttons do: arm, or select (R55).
    fader_fn: TrackFn,
    /// The MK4's Mixer page (R64).
    page: Page,
    /// The send the MK4's Sends mode reaches, from 0 (R64).
    send: u8,
    shift: bool,
    /// Pads whose press went to a slot, so their release does.
    slots_down: Vec<u8>,
    /// Drum pads held, with the note they play.
    drums_down: BTreeMap<u8, u8>,
    /// Whether the MK4 was told the DAW has its drum pads (R70).
    claimed: bool,
    /// What each light was last sent as.
    lit: BTreeMap<(Layer, u8), Light>,
    /// The text last sent to each field of the display.
    shown: BTreeMap<(u8, u8), String>,
    /// The scale last sent to the MK4 (R69).
    scale: Option<Scale>,
    /// Where a Transport encoder last asked for a value to go, until the
    /// model shows it: turns that come faster than the model does add up
    /// (R65).
    pending: BTreeMap<u8, (f64, Instant)>,
    last: Option<ReadModel>,
}

impl Keys {
    pub(crate) fn new(family: Gen) -> Self {
        Self {
            family,
            header: match family {
                Gen::Mk3 => 0x0F,
                Gen::Mk4 => 0x14,
            },
            faders: true,
            row: 0,
            pads: Pads::Session,
            pots: Pots::Device,
            fader_mode: Faders::Volume,
            cycle: Cycle::Clips,
            fader_fn: TrackFn::Arm,
            page: Page::Volume,
            send: 0,
            shift: false,
            slots_down: Vec::new(),
            drums_down: BTreeMap::new(),
            claimed: false,
            lit: BTreeMap::new(),
            shown: BTreeMap::new(),
            scale: None,
            pending: BTreeMap::new(),
            last: None,
        }
    }

    /// Reads the controller's size from its input's name: the 88's header
    /// and the Mini's differ, and the 25 and 37 and the Minis have no faders
    /// (R51, R61, R66).
    fn size_from(&mut self, input: &str) {
        let name = input.to_lowercase();
        let small = name.contains("25") || name.contains("37");
        self.header = match self.family {
            Gen::Mk3 if name.contains("88") => 0x12,
            Gen::Mk3 => 0x0F,
            Gen::Mk4 if name.contains("mini") => 0x13,
            Gen::Mk4 => 0x14,
        };
        self.faders = !small && !name.contains("mini");
    }

    fn sysex(&self, body: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0xF0, 0x00, 0x20, 0x29, 0x02, self.header];
        bytes.extend_from_slice(body);
        bytes.push(0xF7);
        bytes
    }

    /// The channel of the mode reports and feature controls: 16 on the MK3,
    /// 7 on the MK4 (R51, R61).
    const fn mode_status(&self) -> u8 {
        match self.family {
            Gen::Mk3 => 0xBF,
            Gen::Mk4 => 0xB6,
        }
    }

    /// The controller numbers of the pad, pot and fader mode (R51, R61).
    const fn mode_numbers(&self) -> [u8; 3] {
        match self.family {
            Gen::Mk3 => [0x03, 0x09, 0x0A],
            Gen::Mk4 => [0x1D, 0x1E, 0x1F],
        }
    }

    /// The mode `control_surface.list` reports (R4).
    fn mode(&self) -> String {
        let pads = match self.pads {
            Pads::Session => "Session",
            Pads::Drum if self.last.as_ref().is_some_and(|m| m.drum.is_some()) => "Drum",
            Pads::Drum => "Drum (the controller's own)",
            Pads::DeviceSelect => "Device Select",
            Pads::Own => "Custom",
        };
        let mut parts = vec![pads.to_owned()];
        parts.push(format!(
            "track buttons: {}",
            self.cycle.track_fn().map_or("Clips", |f| f.name())
        ));
        let (name, pots) = match (self.family, self.pots) {
            (Gen::Mk3, Pots::Device) => ("pots", "Device".to_owned()),
            (Gen::Mk4, Pots::Device) => ("encoders", "Plugin".to_owned()),
            (_, Pots::Volume) => (self.pots_name(), "Volume".to_owned()),
            (_, Pots::Pan) => (self.pots_name(), "Pan".to_owned()),
            (_, Pots::Send(n)) => (self.pots_name(), send_name(n)),
            (_, Pots::Transport) => ("encoders", "Transport".to_owned()),
            (_, Pots::Own) => (self.pots_name(), "Custom".to_owned()),
        };
        parts.push(format!("{name}: {pots}"));
        if self.faders {
            let faders = match self.fader_mode {
                Faders::Volume => "Volume".to_owned(),
                Faders::Device => "Device".to_owned(),
                Faders::Send(n) => send_name(n),
                Faders::Own => "Custom".to_owned(),
            };
            parts.push(format!("faders: {faders}"));
            parts.push(format!("fader buttons: {}", self.fader_fn.name()));
        }
        parts.join("; ")
    }

    const fn pots_name(&self) -> &'static str {
        match self.family {
            Gen::Mk3 => "pots",
            Gen::Mk4 => "encoders",
        }
    }

    // What the pots and faders reach ------------------------------------

    /// The role of pot or encoder `i`, if it reaches anything (R54, R64).
    fn pot_role(&self, i: u8) -> Option<Role> {
        Some(match self.pots {
            Pots::Device => Role::Device(i),
            Pots::Volume => Role::Volume(i),
            Pots::Pan => Role::Pan(i),
            Pots::Send(send) => Role::Send { column: i, send },
            Pots::Transport | Pots::Own => return None,
        })
    }

    /// The role of fader `i`; the ninth is the Main track's (R55, R66).
    fn fader_role(&self, i: u8) -> Option<Role> {
        if i == 8 {
            return matches!(self.fader_mode, Faders::Volume).then_some(Role::MainVolume);
        }
        Some(match self.fader_mode {
            Faders::Volume => Role::Volume(i),
            Faders::Device => Role::Device(i),
            Faders::Send(send) => Role::Send { column: i, send },
            Faders::Own => return None,
        })
    }

    // What the surface shows -------------------------------------------

    fn ring<'a>(&self, model: &'a ReadModel) -> Option<&'a RingView> {
        model.row(self.row)?.ring.as_ref()
    }

    /// The Session pad at a column and a row from the top.
    const fn pad_note(column: u8, row: u8) -> u8 {
        PAD_TOP + 0x10 * row + column
    }

    /// Every light the surface wants on now.
    fn render(&self, model: &ReadModel) -> BTreeMap<(Layer, u8), Light> {
        let mut out = BTreeMap::new();
        let ring = self.ring(model);
        let doc = &model.document;
        match self.pads {
            Pads::Session => {
                if let Some(ring) = ring {
                    for r in 0..2u8 {
                        for c in 0..8u8 {
                            let Some(&track) = ring.tracks.get(usize::from(c)) else {
                                continue;
                            };
                            let light =
                                match (self.cycle.track_fn(), ring.scenes.get(usize::from(r))) {
                                    (Some(f), _) if r == 1 => Grid::track_fn_light(model, f, track),
                                    (_, Some(&scene)) => Grid::slot_light(model, track, scene),
                                    _ => None,
                                };
                            if let Some(light) = light {
                                out.insert((Layer::Pad, Self::pad_note(c, r)), light);
                            }
                        }
                    }
                }
            }
            Pads::DeviceSelect => {
                let focus = model.row(self.row).and_then(|r| r.focus.as_ref());
                for i in 0..16u8 {
                    if let Some(focus) = focus.filter(|f| usize::from(i) < f.banks.len()) {
                        let on = usize::from(i) == focus.bank;
                        let c = if on { colour::WHITE } else { colour::PURPLE };
                        let note = (if i < 8 { 0x40 } else { 0x50 }) + i % 8;
                        out.insert((Layer::Pad, note), Light::Static(c));
                    }
                }
            }
            Pads::Drum | Pads::Own => {}
        }
        if let Some(drum) = model.drum.filter(|_| self.pads == Pads::Drum) {
            let own = doc
                .device_track(drum.rack)
                .map_or(colour::WHITE, |t| nearest(Grid::track_colour(doc, t)));
            for n in DRUM {
                // The MK3's pads play notes 36 to 51 themselves; the MK4's
                // play the pads its pad view shows (R59, R70).
                let note = match self.family {
                    Gen::Mk3 => Some(n),
                    Gen::Mk4 => drum.first.checked_add(n - DRUM.start).filter(|k| *k < 128),
                };
                let Some(note) = note else {
                    continue;
                };
                let light = if drum.selected == Some(note) {
                    Some(Light::Static(colour::WHITE))
                } else if drum.filled >> note & 1 == 1 {
                    Some(Light::Static(own))
                } else {
                    None
                };
                if let Some(light) = light {
                    out.insert((Layer::Drum, n), light);
                }
            }
        }
        self.button_lights(model, ring, &mut out);
        out
    }

    fn button_lights(
        &self,
        model: &ReadModel,
        ring: Option<&RingView>,
        out: &mut BTreeMap<(Layer, u8), Light>,
    ) {
        let mut put = |cc: u8, c: Option<u8>| {
            if let Some(c) = c {
                out.insert((Layer::Button, cc), Light::Static(c));
            }
        };
        let moves = ring.map(|r| r.moves).unwrap_or_default();
        let drum = model.drum.filter(|_| self.claimed);
        // The pad arrows: the ring, or the drum rack's shown pads (R62, R70).
        let (up, down) = match drum {
            Some(d) => (d.first < 112, d.first > 0),
            None => (moves.up, moves.down),
        };
        put(0x6A, up.then_some(colour::WHITE));
        put(0x6B, down.then_some(colour::WHITE));
        let first = ring.and_then(|r| r.scenes.first().copied());
        put(
            0x68,
            first
                .and_then(|s| Grid::scene_light(model, s))
                .and_then(static_colour),
        );
        put(0x69, Some(self.cycle.colour()));
        if self.family == Gen::Mk4 {
            let (left, right) = self.track_moves(model, ring);
            put(0x66, left.then_some(colour::WHITE));
            put(0x67, right.then_some(colour::WHITE));
        } else {
            let locked = model
                .row(self.row)
                .and_then(|r| r.focus.as_ref())
                .is_some_and(|f| f.lock.is_some());
            put(0x34, locked.then_some(colour::WHITE));
        }
        if self.faders {
            for c in 0..8u8 {
                let light = ring
                    .and_then(|r| r.tracks.get(usize::from(c)))
                    .and_then(|t| Grid::track_fn_light(model, self.fader_fn, *t))
                    .and_then(static_colour);
                put(0x25 + c, light);
            }
            let switch = if self.fader_fn == TrackFn::Arm {
                colour::WHITE
            } else {
                colour::DIM_WHITE
            };
            put(0x2D, Some(switch));
        }
    }

    /// Whether the track buttons can move: the selection (or, with Shift,
    /// the ring) to the left and to the right (R62).
    fn track_moves(&self, model: &ReadModel, ring: Option<&RingView>) -> (bool, bool) {
        if self.shift {
            let moves = ring.map(|r| r.moves).unwrap_or_default();
            return (moves.left, moves.right);
        }
        match selected_index(model) {
            Some(at) => (at > 0, at + 1 < model.columns.len()),
            None => (false, !model.columns.is_empty()),
        }
    }

    /// Sends the lights that changed, and puts out those no longer wanted.
    fn show(&mut self, want: BTreeMap<(Layer, u8), Light>, cx: &mut Cx) {
        for (&(layer, number), &light) in &want {
            if self.lit.get(&(layer, number)) != Some(&light) {
                for bytes in light_bytes(layer, number, Some(light)) {
                    cx.send_for(control_id(layer, number), bytes);
                }
            }
        }
        for &(layer, number) in self.lit.keys() {
            if !want.contains_key(&(layer, number)) {
                for bytes in light_bytes(layer, number, None) {
                    cx.send_for(control_id(layer, number), bytes);
                }
            }
        }
        self.lit = want;
    }

    // The display ----------------------------------------------------------

    /// What a control shows: a title, its parameter's name and its value
    /// (R58, R68).
    fn describe(&self, model: &ReadModel, role: Role) -> Option<(String, String, String)> {
        let doc = &model.document;
        let focus = model.row(self.row)?.focus.as_ref();
        let ring = self.ring(model);
        let track = |column: u8| -> Option<(TrackId, &str)> {
            let id = *ring?.tracks.get(usize::from(column))?;
            Some((id, doc.track(id)?.name()))
        };
        let device = focus
            .and_then(|f| doc.device(f.device))
            .map_or("", |d| d.name.as_str());
        Some(match role {
            Role::Device(i) => {
                let param = focus?.params.get(usize::from(i))?;
                (device.to_owned(), param.name.clone(), param.text.clone())
            }
            Role::Volume(c) => {
                let (id, name) = track(c)?;
                let volume = doc.track(id)?.mixer().volume;
                ("Volume".to_owned(), name.to_owned(), db_text(volume))
            }
            Role::Pan(c) => {
                let (id, name) = track(c)?;
                let pan = doc.track(id)?.mixer().pan;
                ("Pan".to_owned(), name.to_owned(), pan_text(pan))
            }
            Role::Send { column, send } => {
                let (id, name) = track(column)?;
                let to = doc.returns().nth(usize::from(send))?;
                let level = doc.track(id)?.send(to);
                (send_name(send), name.to_owned(), db_text(level))
            }
            Role::MainVolume => {
                let volume = doc.track(doc.main_id())?.mixer().volume;
                ("Volume".to_owned(), "Main".to_owned(), db_text(volume))
            }
            _ => return None,
        })
    }

    /// Sends the display text that changed (R58, R68). `field` is a place in
    /// `shown`; `send` makes the message for a changed text.
    fn text(
        &mut self,
        cx: &mut Cx,
        key: (u8, u8),
        text: &str,
        send: impl FnOnce(&Self, &str) -> Vec<u8>,
    ) {
        let text = ascii(text, 16);
        if self.shown.get(&key) != Some(&text) {
            cx.send(send(self, &text));
            self.shown.insert(key, text);
        }
    }

    /// The MK3's display: each pot's and fader's name and value, and the two
    /// rows of the default display (R58).
    fn display_mk3(&mut self, model: &ReadModel, cx: &mut Cx) {
        let controls = (0..8u8)
            .map(|i| (0x38 + i, self.pot_role(i)))
            .chain((0..9u8).map(|i| (0x50 + i, self.fader_role(i).filter(|_| self.faders))));
        for (index, role) in controls.collect::<Vec<_>>() {
            let device = matches!(role, Some(Role::Device(_)));
            let (name, value) = match role.and_then(|r| self.describe(model, r)) {
                Some((_, name, value)) if device => (name, value),
                Some((title, name, value)) => (format!("{title} {name}"), value),
                None => (String::new(), String::new()),
            };
            for (kind, text) in [(0x07u8, name), (0x08, value)] {
                self.text(cx, (kind, index), &text, |s, t| {
                    let mut body = vec![kind, index];
                    body.extend_from_slice(t.as_bytes());
                    s.sysex(&body)
                });
            }
        }
        let focus = model.row(self.row).and_then(|r| r.focus.as_ref());
        let device = focus
            .and_then(|f| model.document.device(f.device))
            .map_or(String::new(), |d| d.name.clone());
        let bank = focus
            .and_then(|f| f.banks.get(f.bank))
            .cloned()
            .unwrap_or_default();
        for (row, text) in [device, bank].into_iter().enumerate() {
            self.text(cx, (0x04, row as u8), &text, |s, t| {
                let mut body = vec![0x04, row as u8];
                body.extend_from_slice(t.as_bytes());
                s.sysex(&body)
            });
        }
    }

    /// Sets the text of one field of one of the MK4's displays (R68).
    fn screen(&mut self, cx: &mut Cx, target: u8, field: u8, text: &str) {
        self.text(cx, (target, field), text, |s, t| {
            let mut body = vec![0x06, target, field];
            body.extend_from_slice(t.as_bytes());
            s.sysex(&body)
        });
    }

    /// Sets up one of the MK4's displays: its arrangement and whether the
    /// controller shows it by itself (R68).
    fn configure(&mut self, cx: &mut Cx, target: u8, config: u8) {
        if self
            .shown
            .insert((0x04, target), config.to_string())
            .is_none()
        {
            cx.send(self.sysex(&[0x04, target, config]));
        }
    }

    /// The MK4's screen: a temporary display for each encoder and fader,
    /// the stationary display and the modes' names (R68).
    fn display_mk4(&mut self, model: &ReadModel, cx: &mut Cx) {
        // Three lines, shown by the controller on a change or a touch.
        const THREE_LINES: u8 = 0x40 | 0x20 | 2;
        const TWO_LINES: u8 = 1;
        let controls = (0..8u8)
            .map(|i| (0x15 + i, self.pot_role(i)))
            .chain((0..9u8).map(|i| (0x05 + i, self.fader_role(i).filter(|_| self.faders))))
            .collect::<Vec<_>>();
        for (target, role) in controls {
            let Some((title, name, value)) = role.and_then(|r| self.describe(model, r)) else {
                continue;
            };
            self.configure(cx, target, THREE_LINES);
            self.screen(cx, target, 0, &title);
            self.screen(cx, target, 1, &name);
            self.screen(cx, target, 2, &value);
        }
        // The stationary display: the selected track and the device in focus.
        let doc = &model.document;
        let selected = model
            .selection
            .tracks
            .last()
            .and_then(|t| doc.track(*t))
            .map_or("", |t| t.name());
        let device = model
            .row(self.row)
            .and_then(|r| r.focus.as_ref())
            .and_then(|f| doc.device(f.device))
            .map_or("", |d| d.name.as_str());
        self.configure(cx, 0x20, TWO_LINES);
        self.screen(cx, 0x20, 0, selected);
        self.screen(cx, 0x20, 1, device);
        // The names the controller shows for its modes.
        let mixer = match self.page {
            Page::Volume => "Volume",
            Page::Pan => "Pan",
        };
        let sends = send_name(self.send);
        for (target, name) in [
            (0x22, "Session"),
            (0x23, "Drum"),
            (0x24, mixer),
            (0x25, "Device"),
            (0x26, sends.as_str()),
            (0x27, "Transport"),
            (0x28, "Volume"),
        ] {
            self.configure(cx, target, 1);
            self.screen(cx, target, 0, name);
        }
    }

    /// Shows a name and a value on the MK4's global temporary display (R62,
    /// R65, R68).
    fn flash(&mut self, cx: &mut Cx, name: &str, value: &str) {
        if self.family != Gen::Mk4 {
            return;
        }
        self.shown.remove(&(0x21, 0));
        self.shown.remove(&(0x21, 1));
        self.configure(cx, 0x21, 1);
        self.screen(cx, 0x21, 0, name);
        self.screen(cx, 0x21, 1, value);
        cx.send(self.sysex(&[0x04, 0x21, 0x7F]));
    }

    /// Sends the MK4 the set's scale when it changed (R69).
    fn follow_scale(&mut self, model: &ReadModel, cx: &mut Cx) {
        let scale = model.document.settings().scale;
        if self.scale == Some(scale) {
            return;
        }
        self.scale = Some(scale);
        let status = 0xB6;
        cx.send(vec![status, 0x4A, u8::from(scale.enabled)]);
        cx.send(vec![status, 0x3D, scale.root % 12]);
        cx.send(vec![status, 0x3E, mode_index(scale.name)]);
    }
}

/// The position of a scale in the controller's own list, read down the
/// user guide's two columns (R69).
pub(crate) const fn mode_index(name: ScaleName) -> u8 {
    match name {
        ScaleName::Major => 0,
        ScaleName::Minor => 1,
        ScaleName::Dorian => 2,
        ScaleName::Mixolydian => 3,
        ScaleName::Lydian => 4,
        ScaleName::Phrygian => 5,
        ScaleName::Locrian => 6,
        ScaleName::WholeTone => 7,
        ScaleName::HalfWholeDiminished => 8,
        ScaleName::WholeHalfDiminished => 9,
        ScaleName::MinorBlues => 10,
        ScaleName::MinorPentatonic => 11,
        ScaleName::MajorPentatonic => 12,
        ScaleName::HarmonicMinor => 13,
        ScaleName::HarmonicMajor => 14,
        ScaleName::MelodicMinor => 17,
    }
}

/// The index of the selected track among the ring's columns.
fn selected_index(model: &ReadModel) -> Option<usize> {
    let selected = model.selection.tracks.last()?;
    model.columns.iter().position(|t| t == selected)
}

fn static_colour(light: Light) -> Option<u8> {
    match light {
        Light::Static(c) | Light::Pulse(c) | Light::Flash { a: c, .. } => Some(c),
    }
}

fn send_name(send: u8) -> String {
    format!("Send {}", char::from(b'A' + send % 26))
}

fn db_text(gain: f64) -> String {
    let db = gain_db(gain);
    if db.is_finite() {
        format!("{db:.1} dB")
    } else {
        "-inf dB".to_owned()
    }
}

fn pan_text(pan: f64) -> String {
    let steps = (pan.abs() * 50.0).round() as i32;
    match steps {
        0 => "C".to_owned(),
        n if pan < 0.0 => format!("{n}L"),
        n => format!("{n}R"),
    }
}

/// `text` as the controller's screen takes it: ASCII, at most `max` long.
fn ascii(text: &str, max: usize) -> String {
    text.chars()
        .map(|c| if (' '..='~').contains(&c) { c } else { '?' })
        .take(max)
        .collect()
}

fn control_id(layer: Layer, number: u8) -> u32 {
    ((layer as u32) << 8) | u32::from(number)
}

impl ControlSurface for Keys {
    fn start(&mut self, cx: &mut Cx) {
        self.row = cx.row();
        self.size_from(cx.input());
        // DAW mode, then the pads, pots and faders in the modes the surface
        // drives (R51, R61).
        cx.send(vec![0x9F, 0x0C, 0x7F]);
        let [pads, pots, faders] = self.mode_numbers();
        let status = self.mode_status();
        // Session (DAW) on the pads, Device (Plugin) on the pots.
        cx.send(vec![status, pads, 0x02]);
        cx.send(vec![status, pots, 0x02]);
        if self.faders {
            cx.send(vec![status, faders, 0x01]);
        }
    }

    fn stop(&mut self, cx: &mut Cx) {
        // Every light out, the display given back, then the controller's own
        // state (R17, R51, R61).
        for ((layer, number), _) in std::mem::take(&mut self.lit) {
            for bytes in light_bytes(layer, number, None) {
                cx.send(bytes);
            }
        }
        if self.family == Gen::Mk3 {
            cx.send(self.sysex(&[0x06]));
        }
        cx.send(vec![0x9F, 0x0C, 0x00]);
    }

    fn message(&mut self, at: Instant, bytes: &[u8], cx: &mut Cx) {
        self.incoming(at, bytes, cx);
    }

    fn tick(&mut self, model: &ReadModel, cx: &mut Cx) {
        self.row = cx.row();
        // The messages of this tick see the model before it.
        self.last = Some(model.clone());
        self.claim_drum(model, cx);
        let want = self.render(model);
        self.show(want, cx);
        match self.family {
            Gen::Mk3 => self.display_mk3(model, cx),
            Gen::Mk4 => {
                self.display_mk4(model, cx);
                self.follow_scale(model, cx);
            }
        }
    }

    fn state(&self) -> SurfaceState {
        SurfaceState {
            ring: Some(RING),
            mode: Some(self.mode()),
            watch: Vec::new(),
        }
    }
}

/// The devices on the track of the device in focus, in chain order.
fn devices(doc: &Document, focus: DeviceId) -> Vec<DeviceId> {
    doc.device_track(focus)
        .and_then(|t| doc.track(t))
        .map(|t| t.devices().map(|d| d.id).collect())
        .unwrap_or_default()
}

//! The APC mini mk2 profile of the grid surface
//! (`docs/specs/supported-controllers.md`, R35 to R40): its eight by eight
//! pads are the session ring, its track buttons run a track function, its
//! scene buttons launch scenes, and Shift gives the buttons their other
//! uses.

use std::collections::{BTreeMap, HashSet};
use std::time::Instant;

use bl_core::midi::MidiMessage;
use bl_model::RemoteMode;
use serde_json::json;

use super::apc::{Key, Leds, Lit, Rests, channel, reach, version};
use super::events::pulses;
use super::palette::{apc, apc_nearest};
use super::{Faders, Grid, Look, RING, TrackFn, drum_note};
use crate::Hit;
use crate::surface::{
    ControlSurface, Cx, Kind, ReadModel, Recognise, RingStep, Role, SurfaceState,
};

/// The maker's byte and the product's in the reply to the device inquiry
/// (R2).
const MAKER: [u8; 2] = [0x47, 0x4F];

/// The first track button's note, and the first scene button's (R36).
const TRACK: u8 = 0x64;
const SCENE: u8 = 0x70;
const SHIFT: u8 = 0x7A;
/// The first fader's controller number (R36).
const FADER: u8 = 0x30;

/// The APC mini mk2 as the chooser offers it (R1, R35).
#[must_use]
pub fn apc_mini() -> Kind {
    Kind::new(
        "apc-mini-mk2",
        "APC mini mk2",
        true,
        true,
        Some(RING),
        Vec::new(),
        || Box::new(ApcMini::new()),
    )
    .recognised(Recognise {
        port: "apc mini mk2",
        test: None,
        maker: &MAKER,
        pair: true,
    })
}

/// What the pads are: the Session's, the drum layout's or the controller's
/// own Note mode (R38, R40).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pads {
    Session,
    Drum,
    Note,
}

/// What Shift with the scene buttons 1 to 5 choose for the track buttons
/// (R38).
const FUNCTIONS: [TrackFn; 5] = [
    TrackFn::Stop,
    TrackFn::Solo,
    TrackFn::Mute,
    TrackFn::Arm,
    TrackFn::Select,
];

/// The pad of a column and a row from the top: notes rise left to right and
/// then up, so the top row is the last (R36).
const fn pad_note(column: u8, row: u8) -> u8 {
    (7 - row) * 8 + column
}

struct ApcMini {
    shift: bool,
    func: TrackFn,
    faders: Faders,
    pads: Pads,
    leds: Leds,
    rests: Rests<9>,
    /// Pads whose press went to a slot, so their release does.
    slots_down: HashSet<u8>,
    /// Drum pads held, with the note they play.
    drums_down: BTreeMap<u8, u8>,
    clock: Option<Instant>,
    last: Option<ReadModel>,
    row: usize,
}

impl ApcMini {
    fn new() -> Self {
        Self {
            shift: false,
            func: TrackFn::Stop,
            faders: Faders::Volume,
            pads: Pads::Session,
            leds: Leds::default(),
            rests: Rests::new(),
            slots_down: HashSet::new(),
            drums_down: BTreeMap::new(),
            clock: None,
            last: None,
            row: 0,
        }
    }

    fn press(cx: &mut Cx, role: Role, key: u8) {
        cx.control(
            role,
            RemoteMode::Absolute,
            Hit::NoteOn { key, velocity: 127 },
        );
    }

    /// The role fader `index` reaches: the ring's tracks for the first eight
    /// and the Main track for the ninth, in every mode (R39).
    fn fader_role(&self, index: u8) -> Role {
        if index == 8 {
            Role::MainVolume
        } else {
            self.faders.role(index)
        }
    }

    // The controller's messages ------------------------------------------

    /// The introduction's reply gives the faders' positions (R35).
    fn sysex(&mut self, bytes: &[u8]) {
        if bytes.len() == 17 && bytes[..5] == [0xF0, 0x47, 0x7F, 0x4F, 0x61] {
            self.rests.replied(&bytes[7..16]);
        }
    }

    fn pad(&mut self, note: u8, velocity: u8, down: bool, cx: &mut Cx) {
        let (column, row) = (note % 8, 7 - note / 8);
        if !down {
            if self.slots_down.remove(&note) {
                cx.control(
                    Role::Slot { column, row },
                    RemoteMode::Absolute,
                    Hit::NoteOff { key: note },
                );
            }
            return;
        }
        // A pad that arrives here is a Session pad, whatever was thought.
        self.pads = Pads::Session;
        self.slots_down.insert(note);
        cx.control(
            Role::Slot { column, row },
            RemoteMode::Absolute,
            Hit::NoteOn {
                key: note,
                velocity,
            },
        );
    }

    /// A drum pad plays its note into the selected track and selects the
    /// drum rack pad (R40).
    fn drum(&mut self, note: u8, velocity: u8, down: bool, cx: &mut Cx) {
        if !down {
            if let Some(key) = self.drums_down.remove(&note) {
                cx.play(MidiMessage::note_off(0, key));
            }
            return;
        }
        self.pads = Pads::Drum;
        let drum = self.last.as_ref().and_then(|m| m.drum);
        let Some(drum) = drum else {
            return;
        };
        let Some(key) = drum_note(&drum, note % 8, note / 8) else {
            return;
        };
        self.drums_down.insert(note, key);
        cx.play(MidiMessage::note_on(0, key, velocity));
        cx.run("pad.select", json!({ "rack": drum.rack, "note": key }));
    }

    fn track_button(&mut self, column: u8, cx: &mut Cx) {
        let key = TRACK + column;
        if !self.shift {
            Self::press(cx, self.func.role(column), key);
            return;
        }
        match column {
            0 => self.faders = Faders::Volume,
            1 => self.faders = Faders::Pan,
            // Pressing Send again steps to the next send (R39).
            2 => {
                let returns = self
                    .last
                    .as_ref()
                    .map_or(0, |m| m.document.returns().len())
                    .max(1);
                self.faders = match self.faders {
                    Faders::Send(n) => Faders::Send(((usize::from(n) + 1) % returns) as u8),
                    _ => Faders::Send(0),
                };
            }
            3 => self.faders = Faders::Device,
            _ => {
                let step = [
                    RingStep::Up,
                    RingStep::Down,
                    RingStep::Left,
                    RingStep::Right,
                ][usize::from(column - 4)];
                Self::press(cx, Role::Ring(step), key);
            }
        }
    }

    fn scene_button(&mut self, row: u8, cx: &mut Cx) {
        if !self.shift {
            Self::press(cx, Role::SceneLaunch(row), SCENE + row);
            return;
        }
        match row {
            0..=4 => {
                self.func = FUNCTIONS[usize::from(row)];
                self.pads = Pads::Session;
            }
            5 => self.pads = Pads::Drum,
            6 => self.pads = Pads::Note,
            _ => Self::press(cx, Role::StopAll, SCENE + row),
        }
    }

    fn fader(&mut self, index: u8, value: u8, cx: &mut Cx) {
        self.rests.set(usize::from(index), value);
        cx.control(
            self.fader_role(index),
            RemoteMode::Absolute,
            Hit::Value {
                value: u16::from(value),
                full: 127,
            },
        );
    }

    // What the surface shows ---------------------------------------------

    /// Every light the surface wants on now.
    fn render(&self, model: &ReadModel) -> BTreeMap<Key, Lit> {
        let mut out = BTreeMap::new();
        let ring = model.row(self.row).and_then(|r| r.ring.as_ref());
        match self.pads {
            Pads::Session => {
                for row in 0..8usize {
                    for column in 0..8usize {
                        if let Some(lit) = Self::slot_light(model, ring, column, row) {
                            out.insert((pad_note(column as u8, row as u8), 0), lit);
                        }
                    }
                }
            }
            Pads::Drum => {
                if let Some(drum) = model.drum {
                    for note in 0..64u8 {
                        let Some(key) = drum_note(&drum, note % 8, note / 8) else {
                            continue;
                        };
                        let colour = if drum.selected == Some(key) {
                            apc::BLUE
                        } else if drum.filled >> key & 1 == 1 {
                            apc::YELLOW
                        } else {
                            continue;
                        };
                        out.insert((note, 0), vec![[0x90 | channel::FULL, note, colour]]);
                    }
                }
            }
            Pads::Note => {}
        }
        let single = |out: &mut BTreeMap<Key, Lit>, note: u8, on: bool| {
            if on {
                out.insert((note, 0), vec![[0x90, note, 1]]);
            }
        };
        let moves = ring.map(|r| r.moves).unwrap_or_default();
        for column in 0..8u8 {
            let on = if !self.shift {
                ring.and_then(|r| r.tracks.get(usize::from(column)))
                    .and_then(|&track| Grid::track_fn_on(model, self.func, track))
                    .unwrap_or(false)
            } else {
                match column {
                    0 => self.faders == Faders::Volume,
                    1 => self.faders == Faders::Pan,
                    2 => matches!(self.faders, Faders::Send(_)),
                    3 => self.faders == Faders::Device,
                    4 => moves.up,
                    5 => moves.down,
                    6 => moves.left,
                    _ => moves.right,
                }
            };
            single(&mut out, TRACK + column, on);
        }
        for row in 0..8u8 {
            let on = if self.shift {
                match row {
                    0..=4 => self.func == FUNCTIONS[usize::from(row)],
                    5 => self.pads == Pads::Drum,
                    6 => self.pads == Pads::Note,
                    _ => false,
                }
            } else {
                ring.and_then(|r| r.scenes.get(usize::from(row)))
                    .is_some_and(|&scene| Grid::scene_look(model, scene) == Some(true))
            };
            single(&mut out, SCENE + row, on);
        }
        out
    }

    /// A pad's light for its slot (R37).
    fn slot_light(
        model: &ReadModel,
        ring: Option<&crate::RingView>,
        column: usize,
        row: usize,
    ) -> Option<Lit> {
        let ring = ring?;
        let (&track, &scene) = (ring.tracks.get(column)?, ring.scenes.get(row)?);
        let note = pad_note(column as u8, row as u8);
        let rgb = Grid::clip_rgb(&model.document, track, scene);
        let on = |channel: u8, colour: u8| Some(vec![[0x90 | channel, note, colour]]);
        match Grid::slot_look(model, track, scene) {
            Look::Stopped => on(channel::HALF, apc_nearest(rgb)),
            Look::Playing => on(channel::FULL, apc_nearest(rgb)),
            Look::Queued => on(channel::BLINK_EIGHTH, apc_nearest(rgb)),
            Look::Recording => on(channel::PULSE_QUARTER, apc::RED),
            Look::QueuedToRecord { .. } => on(channel::BLINK_EIGHTH, apc::RED),
            Look::Armed => on(channel::QUARTER, apc::RED),
            Look::Empty => None,
        }
    }

    /// The mode `control_surface.list` reports (R4).
    fn mode(&self) -> String {
        let pads = match self.pads {
            Pads::Session => "Session",
            Pads::Drum => "Drum",
            Pads::Note => "Note",
        };
        format!(
            "{pads}; track buttons: {}; faders: {}",
            self.func.name(),
            self.faders.name()
        )
    }
}

impl ControlSurface for ApcMini {
    fn start(&mut self, cx: &mut Cx) {
        self.row = cx.row();
        let [major, minor, fix] = version();
        cx.send(vec![
            0xF0, 0x47, 0x7F, 0x4F, 0x60, 0x00, 0x04, 0x00, major, minor, fix, 0xF7,
        ]);
        self.clock = Some(cx.now());
    }

    fn stop(&mut self, cx: &mut Cx) {
        self.leds.out(cx);
    }

    fn message(&mut self, _at: Instant, bytes: &[u8], cx: &mut Cx) {
        let Some(&status) = bytes.first() else {
            return;
        };
        if status == 0xF0 {
            self.sysex(bytes);
            return;
        }
        let (kind, channel) = (status & 0xF0, status & 0x0F);
        let first = bytes.get(1).copied().unwrap_or(0);
        let second = bytes.get(2).copied().unwrap_or(0);
        let down = kind == 0x90 && second > 0;
        match (kind, channel, first) {
            (0x90 | 0x80, 0, 0..=0x3F) => self.pad(first, second, down, cx),
            (0x90 | 0x80, 9, 0..=0x3F) => self.drum(first, second, down, cx),
            (0x90 | 0x80, 0, 0x64..=0x6B) if down => self.track_button(first - TRACK, cx),
            (0x90 | 0x80, 0, 0x70..=0x77) if down => self.scene_button(first - SCENE, cx),
            (0x90 | 0x80, 0, SHIFT) => self.shift = down,
            (0xB0, 0, 0x30..=0x38) => self.fader(first - FADER, second, cx),
            _ => {}
        }
    }

    fn tick(&mut self, model: &ReadModel, cx: &mut Cx) {
        self.row = cx.row();
        // The messages of this tick see the model before it.
        self.last = Some(model.clone());
        let want = self.render(model);
        self.leds.show(want, cx);
        let roles = std::array::from_fn(|i| Some(self.fader_role(i as u8)));
        self.rests.tell(roles, reach(model, self.row), cx);
        pulses(&mut self.clock, model, cx);
    }

    fn state(&self) -> SurfaceState {
        SurfaceState {
            ring: Some(RING),
            mode: Some(self.mode()),
            watch: Vec::new(),
        }
    }
}

//! The APC40 mkII profile of the grid surface
//! (`docs/specs/supported-controllers.md`, R41 to R50): its eight by five
//! pads are the session ring, each track strip has its buttons and fader,
//! and its knobs reach the tracks' pan and sends and the device in focus.

use std::collections::{BTreeMap, HashSet};
use std::time::Instant;

use bl_model::{
    CrossfadeAssign, DeviceId, RelativeEncoding, RemoteMode, SceneId, TrackId, TrackKind,
};
use serde_json::json;

use super::apc::{Key, Leds, Lit, Rests, channel, reach, version};
use super::events::pulses;
use super::palette::{apc, apc_nearest};
use super::{Grid, Look, TrackFn};
use crate::Hit;
use crate::surface::{
    ControlSurface, Cx, Kind, ReadModel, Recognise, Ring, RingStep, Role, SurfaceState,
};

/// The maker's byte and the product's in the reply to the device inquiry
/// (R2).
const MAKER: [u8; 2] = [0x47, 0x29];

/// The pads are eight tracks wide and five scenes tall (R42, R49).
const RING: Ring = Ring {
    tracks: 8,
    scenes: 5,
};

/// The control that moves the tempo, the one that moves the cue volume, and
/// the controls that take the tempo field's range (R46).
const TEMPO: u8 = 0x0D;
const CUE: u8 = 0x2F;
const TEMPO_MIN: f64 = 20.0;
const TEMPO_MAX: f64 = 999.0;

/// The global launch quantisation Shift with Track Select 1 to 8 sets (R45).
const QUANTISATIONS: [&str; 8] = [
    "none",
    "8_bars",
    "4_bars",
    "2_bars",
    "1_bar",
    "quarter",
    "eighth",
    "sixteenth",
];

/// How long a tempo set that hasn't shown yet is remembered, in ticks.
const PENDING: u8 = 25;

/// The APC40 mkII as the chooser offers it (R1, R41).
#[must_use]
pub fn apc40() -> Kind {
    Kind::new(
        "apc40-mkii",
        "APC40 mkII",
        true,
        true,
        Some(RING),
        Vec::new(),
        || Box::new(Apc40::new()),
    )
    .recognised(Recognise {
        port: "apc40 mkii",
        test: None,
        maker: &MAKER,
        pair: false,
    })
}

/// What the eight track knobs reach (R47).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Knobs {
    Pan,
    Sends,
    User,
}

/// The pad at a column and a row from the top (R42): notes rise left to
/// right and then up, so the top row, the ring's first scene, is the last.
const fn pad_note(column: u8, row: u8) -> u8 {
    (4 - row) * 8 + column
}

struct Apc40 {
    shift: bool,
    bank: bool,
    knobs: Knobs,
    /// The send the Sends knobs reach, from 0.
    send: u8,
    /// Whether Sends is held, so Track Select chooses a send (R47).
    sends_held: bool,
    leds: Leds,
    rests: Rests<10>,
    /// Pads whose press went to a slot, so their release does.
    slots_down: HashSet<u8>,
    /// The values the knobs' rings were last sent, by controller number.
    rings: BTreeMap<u8, u8>,
    /// The tempo the knob last asked for, which the model doesn't show yet,
    /// and for how many ticks more it is remembered.
    pending: Option<(f64, u8)>,
    clock: Option<Instant>,
    last: Option<ReadModel>,
    row: usize,
}

impl Apc40 {
    fn new() -> Self {
        Self {
            shift: false,
            bank: false,
            knobs: Knobs::Pan,
            send: 0,
            sends_held: false,
            leds: Leds::default(),
            rests: Rests::new(),
            slots_down: HashSet::new(),
            rings: BTreeMap::new(),
            pending: None,
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

    fn model(&self) -> Option<&ReadModel> {
        self.last.as_ref()
    }

    fn returns(&self) -> usize {
        self.model().map_or(0, |m| m.document.returns().len())
    }

    /// The role a track knob reaches (R47): a return has no send to a return.
    fn knob_role(&self, column: u8) -> Option<Role> {
        match self.knobs {
            Knobs::Pan => Some(Role::Pan(column)),
            Knobs::Sends => {
                let model = self.model()?;
                let ring = model.row(self.row)?.ring.as_ref()?;
                let track = model
                    .document
                    .track(*ring.tracks.get(usize::from(column))?)?;
                (track.kind() != TrackKind::Return).then_some(Role::Send {
                    column,
                    send: self.send,
                })
            }
            Knobs::User => None,
        }
    }

    /// The device in focus on this row.
    fn focus_device(&self) -> Option<DeviceId> {
        Some(self.model()?.row(self.row)?.focus.as_ref()?.device)
    }

    /// The device before or after the one in focus on its track (R48).
    fn neighbour(&self, forward: bool) -> Option<DeviceId> {
        let doc = &self.model()?.document;
        let device = self.focus_device()?;
        let track = doc.track(doc.device_track(device)?)?;
        let ids: Vec<DeviceId> = track.devices().map(|d| d.id).collect();
        let at = ids.iter().position(|d| *d == device)?;
        if forward {
            ids.get(at + 1).copied()
        } else {
            at.checked_sub(1)
                .and_then(|before| ids.get(before))
                .copied()
        }
    }

    // The controller's messages ------------------------------------------

    fn pad(&mut self, note: u8, velocity: u8, down: bool, cx: &mut Cx) {
        let (column, row) = (note % 8, 4 - note / 8);
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
        if self.shift {
            self.overview_press(column, row, cx);
            return;
        }
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

    /// A pad of Session Overview moves the ring to its block (R43).
    fn overview_press(&mut self, column: u8, row: u8, cx: &mut Cx) {
        let Some(model) = self.model() else {
            return;
        };
        let track = model.columns.get(usize::from(column) * 8).copied();
        let scene = model
            .document
            .scenes()
            .nth(usize::from(row) * usize::from(RING.scenes))
            .map(|s| s.id);
        if let (Some(track), Some(scene)) = (track, scene) {
            cx.run(
                "control_surface.move_ring",
                json!({ "row": self.row + 1, "track": track, "scene": scene }),
            );
        }
    }

    /// A button of a track's strip, on the track's channel (R42, R44, R45,
    /// R47).
    fn strip_button(&mut self, column: u8, note: u8, cx: &mut Cx) {
        let role = match note {
            0x30 => Role::Arm(column),
            0x31 => Role::Solo(column),
            0x32 => Role::Mute(column),
            0x34 => Role::Stop(column),
            0x42 => Role::Crossfade(column),
            _ if self.shift => {
                if let Some(quantization) = QUANTISATIONS.get(usize::from(column)) {
                    cx.run(
                        "session.set_quantization",
                        json!({ "quantization": quantization }),
                    );
                }
                return;
            }
            _ if self.sends_held => {
                if usize::from(column) < self.returns() {
                    self.send = column;
                    self.knobs = Knobs::Sends;
                }
                return;
            }
            _ => Role::Select(column),
        };
        Self::press(cx, role, note);
    }

    /// The device section's eight buttons, in the order of R48.
    fn device_button(&mut self, index: u8, cx: &mut Cx) {
        let Some(model) = self.model() else {
            return;
        };
        let focus = model.row(self.row).and_then(|r| r.focus.as_ref());
        if self.shift {
            if let Some(focus) = focus
                && usize::from(index) < focus.banks.len()
            {
                cx.run(
                    "control_surface.set_bank",
                    json!({ "row": self.row + 1, "bank": index }),
                );
            }
            return;
        }
        let key = 0x3A + index;
        match index {
            0 | 1 => {
                if let Some(device) = self.neighbour(index == 1) {
                    cx.run("device.select", json!({ "device": device }));
                }
            }
            2 => Self::press(cx, Role::PreviousBank, key),
            3 => Self::press(cx, Role::NextBank, key),
            4 => {
                if let Some(focus) = focus
                    && let Some(device) = model.document.device(focus.device)
                {
                    cx.run(
                        "device.set_enabled",
                        json!({ "device": focus.device, "enabled": !device.enabled }),
                    );
                }
            }
            5 => {
                if let Some(focus) = focus {
                    cx.run(
                        "control_surface.lock",
                        json!({ "row": self.row + 1, "device": focus.device }),
                    );
                }
            }
            6 => cx.run("view.switch_detail_view", json!({})),
            _ => cx.run("view.toggle_detail_area", json!({})),
        }
    }

    /// The arrows move the ring a scene or a track, or with Bank on a page of
    /// them (R43).
    fn arrow(&mut self, note: u8, cx: &mut Cx) {
        let step = match (note, self.bank) {
            (0x5E, false) => RingStep::Up,
            (0x5E, true) => RingStep::PageUp,
            (0x5F, false) => RingStep::Down,
            (0x5F, true) => RingStep::PageDown,
            (0x61, false) => RingStep::Left,
            (0x61, true) => RingStep::PageLeft,
            (_, false) => RingStep::Right,
            (_, true) => RingStep::PageRight,
        };
        Self::press(cx, Role::Ring(step), note);
    }

    fn note(&mut self, channel: u8, note: u8, velocity: u8, down: bool, cx: &mut Cx) {
        match note {
            0x00..=0x27 => self.pad(note, velocity, down, cx),
            0x30..=0x34 | 0x42 if down && channel < 8 => self.strip_button(channel, note, cx),
            0x3A..=0x41 if down => self.device_button(note - 0x3A, cx),
            0x50 if down => {
                if let Some(main) = self.model().map(|m| m.document.main_id()) {
                    cx.run("session.select", json!({ "track": main }));
                    cx.run("arrangement.select_tracks", json!({ "tracks": [main] }));
                }
            }
            0x51 if down => Self::press(cx, Role::StopAll, note),
            0x52..=0x56 if down => Self::press(cx, Role::SceneLaunch(note - 0x52), note),
            0x57 if down => self.knobs = Knobs::Pan,
            0x58 => {
                self.sends_held = down;
                if down {
                    self.knobs = Knobs::Sends;
                }
            }
            0x59 if down => self.knobs = Knobs::User,
            0x5A if down => {
                let on = self.model().is_some_and(|m| m.metronome);
                cx.run("transport.set_metronome_enabled", json!({ "enabled": !on }));
            }
            0x5B if down => cx.run("transport.play", json!({})),
            0x5D if down => {
                let recording = self.model().is_some_and(|m| m.transport.recording);
                if recording {
                    cx.run("transport.stop_recording", json!({}));
                } else {
                    cx.run("transport.set_recording", json!({ "recording": true }));
                }
            }
            0x5E..=0x61 if down => self.arrow(note, cx),
            0x62 => self.shift = down,
            0x63 if down => cx.run("transport.tap_tempo", json!({})),
            0x64 | 0x65 => cx.run(
                "transport.phase_nudge",
                json!({
                    "direction": if note == 0x65 { "up" } else { "down" },
                    "held": down,
                }),
            ),
            0x66 if down => cx.run("session.record", json!({})),
            0x67 if down => self.bank = !self.bank,
            _ => {}
        }
    }

    fn controller(&mut self, channel: u8, cc: u8, value: u8, cx: &mut Cx) {
        let absolute = |cx: &mut Cx, role| {
            cx.control(
                role,
                RemoteMode::Absolute,
                Hit::Value {
                    value: u16::from(value),
                    full: 127,
                },
            );
        };
        match cc {
            0x07 if channel < 8 => {
                self.rests.set(usize::from(channel), value);
                absolute(cx, Role::Volume(channel));
            }
            0x0E => {
                self.rests.set(8, value);
                absolute(cx, Role::MainVolume);
            }
            0x0F => {
                self.rests.set(9, value);
                absolute(cx, Role::Crossfader);
            }
            CUE => cx.control(
                Role::CueVolume,
                RemoteMode::Relative {
                    encoding: RelativeEncoding::TwosComplement,
                    accelerated: true,
                },
                Hit::Value {
                    value: u16::from(value),
                    full: 127,
                },
            ),
            TEMPO => self.tempo(value, cx),
            0x10..=0x17 => absolute(cx, Role::Device(cc - 0x10)),
            0x30..=0x37 => {
                if let Some(role) = self.knob_role(cc - 0x30) {
                    absolute(cx, role);
                }
            }
            _ => {}
        }
    }

    /// The tempo knob steps the tempo a BPM, or a tenth with Shift held
    /// (R46).
    fn tempo(&mut self, value: u8, cx: &mut Cx) {
        let steps = crate::size(RelativeEncoding::TwosComplement, value);
        let Some(shown) = self.model().map(|m| m.tempo.get()) else {
            return;
        };
        if steps == 0 {
            return;
        }
        let from = self.pending.map_or(shown, |(bpm, _)| bpm);
        let step = if self.shift { 0.1 } else { 1.0 };
        let bpm = ((from + f64::from(steps) * step) * 10.0)
            .round()
            .clamp(TEMPO_MIN * 10.0, TEMPO_MAX * 10.0)
            / 10.0;
        self.pending = Some((bpm, PENDING));
        cx.run_held(
            0x2_0000 | u32::from(TEMPO),
            "song.set_tempo",
            json!({ "tempo": [{ "kind": "constant", "start": 0, "bpm": bpm }] }),
        );
    }

    // What the surface shows ---------------------------------------------

    /// A pad's light for its slot (R9, R42).
    fn slot_light(model: &ReadModel, track: TrackId, scene: SceneId, note: u8) -> Option<Lit> {
        let rgb = Grid::clip_rgb(&model.document, track, scene);
        let colour = apc_nearest(rgb);
        let with = |primary: u8, motion: u8, secondary: u8| {
            Some(vec![
                [0x90, note, primary],
                [0x90 | motion, note, secondary],
            ])
        };
        match Grid::slot_look(model, track, scene) {
            Look::Stopped => Some(vec![[0x90, note, colour]]),
            Look::Queued => with(colour, channel::BLINK_EIGHTH, apc::GREEN),
            Look::QueuedToRecord { armed } => with(
                if armed { apc::DIM_RED } else { 0 },
                channel::BLINK_EIGHTH,
                apc::RED,
            ),
            Look::Playing => with(colour, channel::PULSE_QUARTER, apc::GREEN),
            Look::Recording => with(apc::DIM_RED, channel::PULSE_QUARTER, apc::RED),
            Look::Armed => Some(vec![[0x90, note, apc::DIM_RED]]),
            Look::Empty => None,
        }
    }

    /// The pads while Shift shows Session Overview (R43): each stands for a
    /// block of eight tracks and five scenes, amber where the ring is, green
    /// while a clip plays in it, red with clips none of which play.
    fn overview(&self, model: &ReadModel, out: &mut BTreeMap<Key, Lit>) {
        let doc = &model.document;
        let scenes: Vec<SceneId> = doc.scenes().map(|s| s.id).collect();
        let (wide, tall) = (usize::from(RING.tracks), usize::from(RING.scenes));
        let ring = model.row(self.row).and_then(|r| r.ring.as_ref());
        let first_track = ring
            .and_then(|r| r.first_track)
            .and_then(|t| model.columns.iter().position(|x| *x == t))
            .unwrap_or(0);
        let first_scene = ring
            .and_then(|r| r.first_scene)
            .and_then(|s| scenes.iter().position(|x| *x == s))
            .unwrap_or(0);
        for r in 0..tall {
            for c in 0..wide {
                let tracks: Vec<TrackId> = model
                    .columns
                    .iter()
                    .skip(c * wide)
                    .take(wide)
                    .copied()
                    .collect();
                let block: Vec<SceneId> =
                    scenes.iter().skip(r * tall).take(tall).copied().collect();
                if tracks.is_empty() || block.is_empty() {
                    continue;
                }
                let holds = tracks
                    .iter()
                    .any(|t| block.iter().any(|s| doc.slot(*t, *s).is_some()));
                let playing = model.slots.iter().any(|s| {
                    s.state == bl_engine::SlotState::Playing
                        && tracks.contains(&s.track)
                        && block.contains(&s.scene)
                });
                // The ring's block, both of two when it straddles them.
                let ring_here = first_track < (c + 1) * wide
                    && c * wide < first_track + wide
                    && first_scene < (r + 1) * tall
                    && r * tall < first_scene + tall;
                let colour = if ring_here {
                    apc::AMBER
                } else if playing {
                    apc::OVERVIEW_GREEN
                } else if holds {
                    apc::OVERVIEW_RED
                } else {
                    continue;
                };
                let note = pad_note(c as u8, r as u8);
                out.insert((note, 0), vec![[0x90, note, colour]]);
            }
        }
    }

    /// A track's Clip Stop light: on while it plays, blinking while its stop
    /// waits for the launch quantisation (R44).
    fn stop_light(model: &ReadModel, track: TrackId) -> u8 {
        let state = |wanted: &[bl_engine::SlotState]| {
            model
                .slots
                .iter()
                .any(|s| s.track == track && wanted.contains(&s.state))
        };
        if state(&[bl_engine::SlotState::QueuedToStop]) {
            2
        } else {
            u8::from(Grid::track_fn_on(model, TrackFn::Stop, track) == Some(true))
        }
    }

    /// Every note-on light the surface wants on now.
    fn render(&self, model: &ReadModel) -> BTreeMap<Key, Lit> {
        let mut out: BTreeMap<Key, Lit> = BTreeMap::new();
        let doc = &model.document;
        let ring = model.row(self.row).and_then(|r| r.ring.as_ref());
        let single = |out: &mut BTreeMap<Key, Lit>, note: u8, channel: u8, value: u8| {
            if value != 0 {
                out.insert((note, channel), vec![[0x90 | channel, note, value]]);
            }
        };
        if self.shift {
            self.overview(model, &mut out);
        } else if let Some(ring) = ring {
            for (r, &scene) in ring.scenes.iter().enumerate() {
                for (c, &track) in ring.tracks.iter().enumerate() {
                    let note = pad_note(c as u8, r as u8);
                    if let Some(lit) = Self::slot_light(model, track, scene, note) {
                        out.insert((note, 0), lit);
                    }
                }
            }
        }
        if let Some(ring) = ring {
            for (r, &scene) in ring.scenes.iter().enumerate() {
                let note = 0x52 + r as u8;
                match Grid::scene_look(model, scene) {
                    Some(true) => {
                        out.insert((note, 0), vec![[0x90, note, apc::GREEN]]);
                    }
                    Some(false) => {
                        out.insert((note, 0), vec![[0x90, note, apc::DIM_WHITE]]);
                    }
                    None => {}
                }
            }
            for (c, &track) in ring.tracks.iter().enumerate() {
                let c = c as u8;
                let on = |f| u8::from(Grid::track_fn_on(model, f, track) == Some(true));
                single(&mut out, 0x30, c, on(TrackFn::Arm));
                single(&mut out, 0x31, c, on(TrackFn::Solo));
                single(&mut out, 0x32, c, on(TrackFn::Mute));
                single(&mut out, 0x33, c, on(TrackFn::Select));
                single(&mut out, 0x34, c, Self::stop_light(model, track));
                let assign = doc.track(track).map(|t| t.mixer().crossfade);
                single(
                    &mut out,
                    0x42,
                    c,
                    match assign {
                        Some(CrossfadeAssign::A) => 1,
                        Some(CrossfadeAssign::B) => 2,
                        _ => 0,
                    },
                );
            }
        }
        let main_selected = model.selection.tracks.contains(&doc.main_id());
        single(&mut out, 0x50, 0, u8::from(main_selected));
        // The device section (R48).
        let focus = model.row(self.row).and_then(|r| r.focus.as_ref());
        single(&mut out, 0x3A, 0, u8::from(self.neighbour(false).is_some()));
        single(&mut out, 0x3B, 0, u8::from(self.neighbour(true).is_some()));
        if let Some(focus) = focus {
            single(&mut out, 0x3C, 0, u8::from(focus.bank > 0));
            single(
                &mut out,
                0x3D,
                0,
                u8::from(focus.bank + 1 < focus.banks.len()),
            );
            let enabled = doc.device(focus.device).is_some_and(|d| d.enabled);
            single(&mut out, 0x3E, 0, u8::from(enabled));
            single(&mut out, 0x3F, 0, u8::from(focus.lock.is_some()));
        }
        // The knob modes and the transport (R47, R49).
        single(&mut out, 0x57, 0, u8::from(self.knobs == Knobs::Pan));
        single(&mut out, 0x58, 0, u8::from(self.knobs == Knobs::Sends));
        single(&mut out, 0x59, 0, u8::from(self.knobs == Knobs::User));
        single(&mut out, 0x5A, 0, u8::from(model.metronome));
        single(&mut out, 0x5B, 0, u8::from(model.transport.playing));
        single(&mut out, 0x5D, 0, u8::from(model.transport.recording));
        single(&mut out, 0x66, 0, u8::from(model.session_record));
        single(&mut out, 0x67, 0, u8::from(self.bank));
        out
    }

    /// Sends a controller message, if the value isn't the one it last sent.
    fn send_cc(&mut self, cc: u8, value: u8, cx: &mut Cx) {
        if self.rings.insert(cc, value) != Some(value) {
            cx.send_for(0x3_0000 | u32::from(cc), vec![0xB0, cc, value]);
        }
    }

    /// The knob rings: the type of each and the value it shows (R47, R48).
    fn rings(&mut self, model: &ReadModel, cx: &mut Cx) {
        let row = model.row(self.row);
        let at = |role: Role| -> Option<u8> {
            row?.positions
                .iter()
                .find(|(r, _)| *r == role)
                .map(|(_, p)| (p.clamp(0.0, 1.0) * 127.0).round() as u8)
        };
        for i in 0..8u8 {
            // Device knobs: Single rings, dark where the bank has nothing.
            let param = row
                .and_then(|r| r.focus.as_ref())
                .is_some_and(|f| f.params.get(usize::from(i)).is_some());
            self.send_cc(0x18 + i, u8::from(param), cx);
            if let Some(value) = at(Role::Device(i)).filter(|_| param) {
                self.send_cc(0x10 + i, value, cx);
            }
            // Track knobs: Pan or Volume rings, dark in User.
            let role = self.knob_role(i);
            let value = role.and_then(at);
            let kind = match (self.knobs, value) {
                (Knobs::Pan, Some(_)) => 3,
                (Knobs::Sends, Some(_)) => 2,
                _ => 0,
            };
            self.send_cc(0x38 + i, kind, cx);
            if let Some(value) = value {
                self.send_cc(0x30 + i, value, cx);
            }
        }
    }

    /// The mode `control_surface.list` reports (R4).
    fn mode(&self) -> String {
        let knobs = match self.knobs {
            Knobs::Pan => "Pan".to_owned(),
            Knobs::Sends => format!("Send {}", char::from(b'A' + self.send.min(25))),
            Knobs::User => "User".to_owned(),
        };
        format!(
            "{}; faders: Volume; track knobs: {knobs}; device knobs: Device",
            if self.shift {
                "Session Overview"
            } else {
                "Session"
            }
        )
    }

    /// What the host is told to introduce Beatlayer with, and to leave in:
    /// the mode, then Beatlayer's version (R41).
    fn introduction(mode: u8) -> Vec<u8> {
        let [major, minor, fix] = version();
        vec![
            0xF0, 0x47, 0x7F, 0x29, 0x60, 0x00, 0x04, mode, major, minor, fix, 0xF7,
        ]
    }
}

impl ControlSurface for Apc40 {
    fn start(&mut self, cx: &mut Cx) {
        self.row = cx.row();
        // The mode where the host lights everything, rings included (R41).
        cx.send(Self::introduction(0x42));
        self.clock = Some(cx.now());
    }

    fn stop(&mut self, cx: &mut Cx) {
        // Every light out, the rings included, then Generic (R41).
        self.leds.out(cx);
        for ring in std::mem::take(&mut self.rings).into_keys() {
            if matches!(ring, 0x18..=0x1F | 0x38..=0x3F) {
                cx.send(vec![0xB0, ring, 0]);
            }
        }
        cx.send(Self::introduction(0x40));
    }

    fn message(&mut self, _at: Instant, bytes: &[u8], cx: &mut Cx) {
        let Some(&status) = bytes.first() else {
            return;
        };
        let (kind, channel) = (status & 0xF0, status & 0x0F);
        let first = bytes.get(1).copied().unwrap_or(0);
        let second = bytes.get(2).copied().unwrap_or(0);
        match kind {
            0x90 | 0x80 => self.note(channel, first, second, kind == 0x90 && second > 0, cx),
            0xB0 => self.controller(channel, first, second, cx),
            _ => {}
        }
    }

    fn tick(&mut self, model: &ReadModel, cx: &mut Cx) {
        self.row = cx.row();
        // The messages of this tick see the model before it.
        self.last = Some(model.clone());
        if let Some((bpm, left)) = self.pending {
            self.pending =
                (left > 1 && (model.tempo.get() - bpm).abs() > 0.05).then_some((bpm, left - 1));
        }
        let want = self.render(model);
        self.leds.show(want, cx);
        self.rings(model, cx);
        let roles = std::array::from_fn(|i| {
            Some(match i {
                0..=7 => Role::Volume(i as u8),
                8 => Role::MainVolume,
                _ => Role::Crossfader,
            })
        });
        self.rests.tell(roles, reach(model, self.row), cx);
        pulses(&mut self.clock, model, cx);
    }

    fn state(&self) -> SurfaceState {
        let mut watch: Vec<Role> = (0..8).map(Role::Device).collect();
        watch.extend((0..8).filter_map(|i| self.knob_role(i)));
        SurfaceState {
            ring: Some(RING),
            mode: Some(self.mode()),
            watch,
        }
    }
}

//! The surface a controller template makes (`docs/specs/control-surfaces.md`,
//! R42): it reads its controls' messages as the template says, asks the
//! shell for the roles and the commands they are, and lights each control's
//! lamp or fader from the read model.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use bl_core::midi::MidiMessage;
use bl_core::time::{Bpm, TempoMap};
use bl_engine::SlotState;
use bl_model::fader::{TOP_DB, db_gain};
use bl_model::{DeviceId, Document, RemoteMode, TrackId};
use serde_json::{Value, json};

use crate::Hit;
use crate::surface::{ControlSurface, Cx, ReadModel, Ring, Role, SurfaceState, Transport};
use crate::template::{Act, Control, Feedback, Reads, Source, Template};

/// The tempo a control's whole travel covers, in beats per minute: the
/// Tempo field's own range.
const TEMPO: (f64, f64) = (20.0, 999.0);

/// Which half of a 14-bit pair a controller message is.
#[derive(Clone, Copy)]
enum Half {
    Whole,
    High,
    Low,
}

/// How a lamp shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lamp {
    Off,
    On,
    Flash,
}

/// What the last tick showed, for what a message needs to know.
struct Seen {
    document: Arc<Document>,
    transport: Transport,
    focus: Option<DeviceId>,
}

/// The surface of one template.
pub struct TemplateSurface {
    template: Arc<Template>,
    /// The control each message of the template's controls belongs to.
    by_source: HashMap<Source, (usize, Half)>,
    /// The high half of each 14-bit control's pair, as it arrived.
    high: HashMap<usize, u8>,
    /// What each control's lamp last showed, as the bytes sent, so a lamp
    /// is sent only when it changes.
    shown: HashMap<usize, Vec<u8>>,
    seen: Option<Seen>,
}

impl TemplateSurface {
    /// A surface for `template`.
    #[must_use]
    pub fn new(template: Arc<Template>) -> Self {
        let mut by_source = HashMap::new();
        for (at, control) in template.controls.iter().enumerate() {
            match control.source {
                Source::Cc14 {
                    channel,
                    controller,
                } => {
                    let cc = |controller| Source::Cc {
                        channel,
                        controller,
                    };
                    by_source.insert(cc(controller), (at, Half::High));
                    by_source.insert(cc(controller + 32), (at, Half::Low));
                }
                source => {
                    by_source.insert(source, (at, Half::Whole));
                }
            }
        }
        Self {
            template,
            by_source,
            high: HashMap::new(),
            shown: HashMap::new(),
            seen: None,
        }
    }

    /// What a message is to the control it belongs to: how its value reads
    /// and what it was. `None` for a message that says nothing yet, such as
    /// the high half of a pair.
    fn read(&mut self, at: usize, half: Half, message: &MidiMessage) -> Option<(Hit, RemoteMode)> {
        let template = Arc::clone(&self.template);
        let control = &template.controls[at];
        let (a, b) = (message.data1(), message.data2());
        let value = |full: u16, value: u16| Hit::Value { value, full };
        let (hit, mode) = match (control.kind, control.source) {
            (Reads::Button { .. }, Source::Note { .. }) => {
                let hit = if message.is_note_on() {
                    Hit::NoteOn {
                        key: a,
                        velocity: b,
                    }
                } else {
                    Hit::NoteOff { key: a }
                };
                (hit, RemoteMode::Absolute)
            }
            (Reads::Button { press, release }, _) => {
                // A controller button says press or release by its value;
                // another value is neither.
                let hit = if b == release {
                    Hit::NoteOff { key: a }
                } else if b == press {
                    Hit::NoteOn {
                        key: a,
                        velocity: b,
                    }
                } else {
                    return None;
                };
                (hit, RemoteMode::Absolute)
            }
            (Reads::Toggle, Source::Note { .. }) => (
                value(127, u16::from(if message.is_note_on() { b } else { 0 })),
                RemoteMode::Absolute,
            ),
            (
                Reads::Relative {
                    encoding,
                    accelerated,
                },
                _,
            ) => (
                value(127, u16::from(b)),
                RemoteMode::Relative {
                    encoding,
                    accelerated,
                },
            ),
            (_, Source::Cc14 { .. }) => match half {
                Half::High => {
                    self.high.insert(at, b);
                    return None;
                }
                _ => {
                    let high = self.high.get(&at).copied().unwrap_or(0);
                    (
                        value(16_383, u16::from(high) << 7 | u16::from(b)),
                        RemoteMode::Absolute14,
                    )
                }
            },
            (_, Source::PitchBend { .. }) => (
                value(16_383, u16::from(b) << 7 | u16::from(a)),
                RemoteMode::Absolute14,
            ),
            _ => (value(127, u16::from(b)), RemoteMode::Absolute),
        };
        Some((hit, mode))
    }

    /// What a control's message asks for.
    fn act(&mut self, at: usize, hit: Hit, mode: RemoteMode, cx: &mut Cx) {
        let template = Arc::clone(&self.template);
        let control = &template.controls[at];
        if let Act::Role(role) = control.act {
            cx.control(role, mode, hit);
            return;
        }
        // A button acts on its press; a toggle says whether it is on.
        let (pressed, on) = match hit {
            Hit::NoteOn { .. } => (true, true),
            Hit::Value { value, full } => (value > 0, u32::from(value) * 2 >= u32::from(full)),
            _ => (false, false),
        };
        let toggle = matches!(control.kind, Reads::Toggle);
        // A button flips a switch from how the last tick left it; a toggle
        // says which way. Before the first tick a button doesn't know.
        let flip = |now: Option<bool>| if toggle { Some(on) } else { now.map(|n| !n) };
        let seen = self.seen.as_ref();
        match control.act {
            Act::Tempo => {
                if let Hit::Value { value, full } = hit {
                    let share = f64::from(value) / f64::from(full);
                    let bpm = (TEMPO.0 + share * (TEMPO.1 - TEMPO.0)) * 100.0;
                    if let Ok(bpm) = Bpm::new(bpm.round() / 100.0) {
                        let tempo =
                            serde_json::to_value(TempoMap::constant(bpm)).unwrap_or(Value::Null);
                        cx.run_held(at as u32, "song.set_tempo", json!({ "tempo": tempo }));
                    }
                }
            }
            Act::Record if pressed => {
                if let Some(on) = flip(seen.map(|s| s.transport.recording)) {
                    cx.run("transport.set_recording", json!({ "recording": on }));
                }
            }
            Act::Loop if pressed => {
                let now = seen.map(|s| s.document.settings().arrangement_loop.enabled);
                if let Some(on) = flip(now) {
                    cx.run("transport.set_loop_enabled", json!({ "enabled": on }));
                }
            }
            Act::Metronome if pressed => {
                let now = seen.map(|s| s.document.settings().metronome.enabled);
                if let Some(on) = flip(now) {
                    cx.run("transport.set_metronome_enabled", json!({ "enabled": on }));
                }
            }
            Act::Play if pressed => cx.run("transport.play", json!({})),
            Act::Stop if pressed => cx.run("transport.stop", json!({})),
            Act::SessionRecord if pressed => cx.run("session.record", json!({})),
            Act::TapTempo if pressed => cx.run("transport.tap_tempo", json!({})),
            Act::Undo if pressed => cx.run("edit.undo", json!({})),
            Act::Redo if pressed => cx.run("edit.redo", json!({})),
            Act::BackToArrangement if pressed => cx.run("session.back_to_arrangement", json!({})),
            Act::NextDevice if pressed => self.step_device(1, cx),
            Act::PreviousDevice if pressed => self.step_device(-1, cx),
            _ => {}
        }
    }

    /// Selects the device `delta` places along the chain from the one in
    /// focus; nothing at the ends, or for a device inside a rack.
    fn step_device(&self, delta: isize, cx: &mut Cx) {
        let Some(Seen {
            document,
            focus: Some(focus),
            ..
        }) = &self.seen
        else {
            return;
        };
        for track in document.tracks() {
            let chain: Vec<DeviceId> = track.devices().map(|d| d.id).collect();
            if let Some(at) = chain.iter().position(|d| d == focus) {
                let next = at.checked_add_signed(delta).and_then(|n| chain.get(n));
                if let Some(next) = next {
                    cx.run("device.select", json!({ "device": next }));
                }
                return;
            }
        }
    }

    /// How a control's lamp shows now, if it has a state to show.
    fn lamp_of(&self, control: &Control, model: &ReadModel, row: usize) -> Lamp {
        let doc = &model.document;
        let ring = model.row(row).and_then(|r| r.ring.as_ref());
        let track =
            |column: u8| -> Option<TrackId> { ring?.tracks.get(usize::from(column)).copied() };
        let on = |lit: Option<bool>| {
            if lit == Some(true) {
                Lamp::On
            } else {
                Lamp::Off
            }
        };
        let mixer = |column, f: fn(&bl_model::Mixer) -> bool| {
            on(track(column)
                .and_then(|t| doc.track(t))
                .map(|t| f(t.mixer())))
        };
        match control.act {
            Act::Role(Role::Slot { column, row: r }) => {
                let slot = track(column).zip(ring.and_then(|r_| r_.scenes.get(usize::from(r))));
                match slot
                    .and_then(|(t, s)| model.slots.iter().find(|x| x.track == t && x.scene == *s))
                {
                    Some(s) if matches!(s.state, SlotState::Playing | SlotState::Recording) => {
                        Lamp::On
                    }
                    Some(_) => Lamp::Flash,
                    None => Lamp::Off,
                }
            }
            Act::Role(Role::Mute(c)) => mixer(c, |m| m.mute),
            Act::Role(Role::Solo(c)) => mixer(c, |m| m.solo),
            Act::Role(Role::Arm(c)) => mixer(c, |m| m.arm),
            Act::Role(Role::Select(c)) => on(track(c).map(|t| model.selection.tracks.contains(&t))),
            Act::Play => on(Some(model.transport.playing)),
            Act::Loop => on(Some(model.transport.loop_on)),
            Act::Record => on(Some(model.transport.recording)),
            Act::Metronome => on(Some(doc.settings().metronome.enabled)),
            _ => Lamp::Off,
        }
    }

    /// The position on its travel, 0 to 1, a control with a value to show
    /// shows.
    fn position_of(control: &Control, model: &ReadModel, row: usize) -> Option<f64> {
        let doc = &model.document;
        let ring = model.row(row).and_then(|r| r.ring.as_ref());
        let mixer = |role| match role {
            Role::MainVolume => Some(doc.main()),
            Role::Volume(c) | Role::Pan(c) => doc.track(*ring?.tracks.get(usize::from(c))?),
            _ => None,
        };
        let Act::Role(role) = control.act else {
            return None;
        };
        let mixer = mixer(role)?.mixer();
        match role {
            Role::Pan(_) => Some((mixer.pan + 1.0) / 2.0),
            _ => Some(mixer.volume / db_gain(TOP_DB)),
        }
        .map(|position| position.clamp(0.0, 1.0))
    }

    /// The messages that put a control's lamp or fader at `lamp` or
    /// `position`.
    fn lights(control: &Control, lamp: Lamp, position: Option<f64>) -> Vec<Vec<u8>> {
        let (Source::Note { channel, .. }
        | Source::Cc { channel, .. }
        | Source::Cc14 { channel, .. }
        | Source::PitchBend { channel }) = control.source;
        match (control.feedback, control.source) {
            (Some(Feedback::State { off, on, flash }), source) => {
                let value = match lamp {
                    Lamp::Off => off,
                    Lamp::On => on,
                    Lamp::Flash => flash.unwrap_or(on),
                };
                match source {
                    Source::Note { key, .. } => vec![vec![0x90 | channel, key, value]],
                    Source::Cc { controller, .. } => vec![vec![0xB0 | channel, controller, value]],
                    _ => Vec::new(),
                }
            }
            (Some(Feedback::Value), source) => {
                let Some(position) = position else {
                    return Vec::new();
                };
                match source {
                    Source::Cc { controller, .. } => {
                        vec![vec![
                            0xB0 | channel,
                            controller,
                            (position * 127.0).round() as u8,
                        ]]
                    }
                    Source::Cc14 { controller, .. } => {
                        let v = (position * 16_383.0).round() as u16;
                        vec![
                            vec![0xB0 | channel, controller, (v >> 7) as u8],
                            vec![0xB0 | channel, controller + 32, (v & 127) as u8],
                        ]
                    }
                    Source::PitchBend { .. } => {
                        let v = (position * 16_383.0).round() as u16;
                        vec![vec![0xE0 | channel, (v & 127) as u8, (v >> 7) as u8]]
                    }
                    Source::Note { .. } => Vec::new(),
                }
            }
            (None, _) => Vec::new(),
        }
    }

    /// Sends what a control shows when it changes.
    fn show(&mut self, at: usize, messages: Vec<Vec<u8>>, cx: &mut Cx) {
        let flat: Vec<u8> = messages.iter().flatten().copied().collect();
        if messages.is_empty() || self.shown.get(&at) == Some(&flat) {
            return;
        }
        for message in messages {
            cx.send_for(at as u32, message);
        }
        self.shown.insert(at, flat);
    }

    /// Puts every lamp out.
    fn lamps_out(&mut self, cx: &mut Cx) {
        let template = Arc::clone(&self.template);
        for (at, control) in template.controls.iter().enumerate() {
            if matches!(control.feedback, Some(Feedback::State { .. })) {
                self.show(at, Self::lights(control, Lamp::Off, None), cx);
            }
        }
    }
}

impl ControlSurface for TemplateSurface {
    fn start(&mut self, cx: &mut Cx) {
        for bytes in &self.template.setup {
            cx.send(bytes.clone());
        }
        self.lamps_out(cx);
    }

    fn stop(&mut self, cx: &mut Cx) {
        self.lamps_out(cx);
        for bytes in &self.template.goodbye {
            cx.send(bytes.clone());
        }
    }

    fn message(&mut self, at: Instant, bytes: &[u8], cx: &mut Cx) {
        let Some(message) = MidiMessage::from_bytes(bytes) else {
            return;
        };
        let Some(channel) = message.channel() else {
            return;
        };
        let source = match message.status() & 0xF0 {
            0x80 | 0x90 => Source::Note {
                channel,
                key: message.data1(),
            },
            0xB0 => Source::Cc {
                channel,
                controller: message.data1(),
            },
            0xE0 => Source::PitchBend { channel },
            _ => return,
        };
        // A message that isn't one of the template's controls reaches the
        // tracks as it would without a surface.
        let Some(&(control, half)) = self.by_source.get(&source) else {
            cx.hand_back(at, message);
            return;
        };
        if let Some((hit, mode)) = self.read(control, half, &message) {
            self.act(control, hit, mode, cx);
        }
    }

    fn tick(&mut self, model: &ReadModel, cx: &mut Cx) {
        let row = cx.row();
        self.seen = Some(Seen {
            document: Arc::clone(&model.document),
            transport: model.transport,
            focus: model
                .row(row)
                .and_then(|r| r.focus.as_ref())
                .map(|f| f.device),
        });
        let template = Arc::clone(&self.template);
        for (at, control) in template.controls.iter().enumerate() {
            let messages = match control.feedback {
                Some(Feedback::State { .. }) => {
                    Self::lights(control, self.lamp_of(control, model, row), None)
                }
                Some(Feedback::Value) => {
                    Self::lights(control, Lamp::Off, Self::position_of(control, model, row))
                }
                None => continue,
            };
            self.show(at, messages, cx);
        }
    }

    fn state(&self) -> SurfaceState {
        SurfaceState {
            ring: (self.template.ring_width > 0).then_some(Ring {
                tracks: self.template.ring_width,
                scenes: self.template.ring_height,
            }),
            mode: None,
        }
    }
}

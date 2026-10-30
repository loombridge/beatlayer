//! What the Launchkey surface does with the controller's messages
//! (`docs/specs/supported-controllers.md`, R51 to R71): pads, buttons, pots
//! and encoders, faders, and the modes the controller reports.

use std::time::Duration;

use bl_core::midi::MidiMessage;
use bl_core::time::{Bpm, TICKS_PER_QUARTER, TempoMap, Ticks};
use bl_engine::SlotState;
use bl_model::{ClipData, RemoteMode};
use serde_json::json;

use super::*;
use crate::Hit;
use crate::surface::RingStep;

/// How long a Transport encoder trusts where it asked for a value to go
/// before the model shows it.
const PENDING: Duration = Duration::from_millis(300);

/// How far a Transport encoder's step moves a position or a loop brace: a
/// beat (Beatlayer's own, R65).
const BEAT: i64 = TICKS_PER_QUARTER;

impl Keys {
    pub(super) fn incoming(&mut self, _at: Instant, bytes: &[u8], cx: &mut Cx) {
        let Some(&status) = bytes.first() else {
            return;
        };
        let (kind, channel) = (status & 0xF0, status & 0x0F);
        let (a, b) = (
            bytes.get(1).copied().unwrap_or(0),
            bytes.get(2).copied().unwrap_or(0),
        );
        let on = kind == 0x90 && b > 0;
        match (kind, channel) {
            (0x90 | 0x80, 0) => self.pad(a, b, on, cx),
            (0x90 | 0x80, 9) => self.drum(a, b, on, cx),
            (0xB0, _) => self.controller(channel, a, b, cx),
            _ => {}
        }
    }

    fn controller(&mut self, channel: u8, cc: u8, value: u8, cx: &mut Cx) {
        let [pads, pots, faders] = self.mode_numbers();
        match cc {
            _ if cc == pads => self.pad_mode(value),
            _ if cc == pots => self.pot_mode(value),
            _ if cc == faders => self.fader_mode_reported(value),
            0x15..=0x1C => self.pot(cc - 0x15, value, cx),
            0x55..=0x5C if self.family == Gen::Mk4 => self.encoder(cc - 0x55, value, cx),
            0x35..=0x3D if self.family == Gen::Mk3 && self.faders => {
                self.fader(cc - 0x35, value, cx)
            }
            0x05..=0x0D if self.family == Gen::Mk4 && self.faders => {
                self.fader(cc - 0x05, value, cx)
            }
            _ => self.button(channel, cc, value > 0, cx),
        }
    }

    // The modes the controller reports (R51, R54, R55, R61, R64, R66) -----

    fn pad_mode(&mut self, value: u8) {
        self.pads = match (self.family, value) {
            (_, 2) => Pads::Session,
            (Gen::Mk3, 1) | (Gen::Mk4, 1 | 0x0F) => Pads::Drum,
            (Gen::Mk3, 9) => Pads::DeviceSelect,
            _ => Pads::Own,
        };
    }

    fn pot_mode(&mut self, value: u8) {
        self.pots = match (self.family, value) {
            (Gen::Mk3, 1) => Pots::Volume,
            (Gen::Mk3, 3) => Pots::Pan,
            (Gen::Mk3, 4) => Pots::Send(0),
            (Gen::Mk3, 5) => Pots::Send(1),
            (Gen::Mk4, 1) => self.mixer_pots(),
            (Gen::Mk4, 4) => Pots::Send(self.send),
            (Gen::Mk4, 5) => Pots::Transport,
            (_, 2) => Pots::Device,
            _ => Pots::Own,
        };
    }

    const fn mixer_pots(&self) -> Pots {
        match self.page {
            Page::Volume => Pots::Volume,
            Page::Pan => Pots::Pan,
        }
    }

    fn fader_mode_reported(&mut self, value: u8) {
        self.fader_mode = match (self.family, value) {
            (_, 1) => Faders::Volume,
            (Gen::Mk3, 2) => Faders::Device,
            (Gen::Mk3, 4) => Faders::Send(0),
            (Gen::Mk3, 5) => Faders::Send(1),
            _ => Faders::Own,
        };
    }

    // Pads -----------------------------------------------------------------

    fn pad(&mut self, note: u8, velocity: u8, press: bool, cx: &mut Cx) {
        if !press {
            if let Some(at) = self.slots_down.iter().position(|n| *n == note) {
                self.slots_down.swap_remove(at);
                let (column, row) = (note & 0x0F, (note >> 4) - 6);
                cx.control(
                    Role::Slot { column, row },
                    RemoteMode::Absolute,
                    Hit::NoteOff { key: note },
                );
            }
            return;
        }
        match self.pads {
            Pads::Session => {
                let (column, row) = match note {
                    0x60..=0x67 => (note - 0x60, 0),
                    0x70..=0x77 => (note - 0x70, 1),
                    _ => return,
                };
                let hit = Hit::NoteOn {
                    key: note,
                    velocity,
                };
                if let (Some(f), 1) = (self.cycle.track_fn(), row) {
                    cx.control(f.role(column), RemoteMode::Absolute, hit);
                    return;
                }
                self.slots_down.push(note);
                cx.control(Role::Slot { column, row }, RemoteMode::Absolute, hit);
            }
            Pads::DeviceSelect => {
                let bank = match note {
                    0x40..=0x47 => note - 0x40,
                    0x50..=0x57 => note - 0x50 + 8,
                    _ => return,
                };
                cx.run(
                    "control_surface.set_bank",
                    json!({ "row": self.row + 1, "bank": bank }),
                );
            }
            Pads::Drum | Pads::Own => {}
        }
    }

    /// A drum pad the DAW claimed (R70): it plays the pad its place on the
    /// pad view gives it, and selects it.
    fn drum(&mut self, note: u8, velocity: u8, press: bool, cx: &mut Cx) {
        if !press {
            if let Some(key) = self.drums_down.remove(&note) {
                cx.play(MidiMessage::note_off(0, key));
            }
            return;
        }
        let Some(drum) = self
            .last
            .as_ref()
            .and_then(|m| m.drum)
            .filter(|_| self.claimed)
        else {
            return;
        };
        let Some(key) = DRUM
            .contains(&note)
            .then(|| drum.first.checked_add(note - DRUM.start))
            .flatten()
            .filter(|k| *k < 128)
        else {
            return;
        };
        self.drums_down.insert(note, key);
        cx.play(MidiMessage::note_on(0, key, velocity));
        cx.run("pad.select", json!({ "rack": drum.rack, "note": key }));
    }

    /// Takes the drum pads from the controller while the pads show Drum on a
    /// track with a drum rack, and gives them back otherwise (R70).
    pub(super) fn claim_drum(&mut self, model: &ReadModel, cx: &mut Cx) {
        if self.family != Gen::Mk4 {
            return;
        }
        let want = self.pads == Pads::Drum && model.drum.is_some();
        if want != self.claimed {
            self.claimed = want;
            cx.send(vec![0xB6, 0x54, u8::from(want)]);
        }
    }

    // Pots, encoders and faders -------------------------------------------

    fn absolute(role: Role, value: u8, cx: &mut Cx) {
        cx.control(
            role,
            RemoteMode::Absolute,
            Hit::Value {
                value: u16::from(value),
                full: 127,
            },
        );
    }

    fn pot(&mut self, i: u8, value: u8, cx: &mut Cx) {
        if let Some(role) = self.pot_role(i) {
            Self::absolute(role, value, cx);
        }
    }

    fn fader(&mut self, i: u8, value: u8, cx: &mut Cx) {
        if let Some(role) = self.fader_role(i) {
            Self::absolute(role, value, cx);
        }
    }

    /// A Transport encoder's step, from 40h (R65): the play position, the
    /// horizontal zoom (the selection across tracks in the Session View),
    /// the track height (across scenes), the loop's start, its end, the
    /// locators, nothing, and the tempo.
    fn encoder(&mut self, i: u8, value: u8, cx: &mut Cx) {
        let steps = i32::from(value) - 64;
        let Some(model) = self.last.clone().filter(|_| steps != 0) else {
            return;
        };
        let doc = &model.document;
        let brace = doc.settings().arrangement_loop;
        let now = cx.now();
        let beats = i64::from(steps) * BEAT;
        let held = 0x100 + u32::from(i);
        match i {
            0 => {
                let from = if model.transport.playing {
                    model.transport.position
                } else {
                    model.transport.insert_marker
                };
                let from = self.pending(0, from.get() as f64, now) as i64;
                let at = (from + beats).max(0);
                self.pending.insert(0, (at as f64, now));
                cx.run("transport.locate", json!({ "position": Ticks(at) }));
                self.flash(cx, "Position", &format!("beat {}", at / BEAT + 1));
            }
            1 if model.session_view => self.step_selection(steps, cx),
            1 => {
                let factor = 1.25f64.powi(steps);
                cx.run("arrangement.zoom", json!({ "factor": factor }));
                self.flash(cx, "Zoom", if steps > 0 { "in" } else { "out" });
            }
            2 if model.session_view => self.step_scene(steps, cx),
            2 => {
                let delta = f64::from(steps) * 8.0;
                cx.run("arrangement.set_track_height", json!({ "delta": delta }));
                self.flash(
                    cx,
                    "Track height",
                    if steps > 0 { "taller" } else { "shorter" },
                );
            }
            3 => {
                let (start, length) = self.brace(brace, now);
                let end = start + length;
                let new = (start + beats).clamp(0, end - BEAT);
                self.pending.insert(3, (new as f64, now));
                self.pending.insert(4, ((end - new) as f64, now));
                let args = json!({ "start": Ticks(new), "length": Ticks(end - new) });
                cx.run_held(held, "transport.set_loop", args);
                self.flash(cx, "Loop start", &format!("beat {}", new / BEAT + 1));
            }
            4 => {
                let (start, length) = self.brace(brace, now);
                let new = (length + beats).max(BEAT);
                self.pending.insert(4, (new as f64, now));
                let args = json!({ "start": Ticks(start), "length": Ticks(new) });
                cx.run_held(held, "transport.set_loop", args);
                self.flash(
                    cx,
                    "Loop end",
                    &format!("beat {}", (start + new) / BEAT + 1),
                );
            }
            5 => {
                let id = if steps > 0 {
                    "locator.jump_next"
                } else {
                    "locator.jump_previous"
                };
                cx.run(id, json!({}));
                self.flash(cx, "Locators", if steps > 0 { "next" } else { "previous" });
            }
            7 => {
                let from = self.pending(7, model.tempo.get(), now);
                let bpm = (from + f64::from(steps)).clamp(20.0, 999.0);
                self.pending.insert(7, (bpm, now));
                if let Ok(bpm) = Bpm::new(bpm) {
                    let tempo = serde_json::to_value(TempoMap::constant(bpm)).unwrap_or_default();
                    cx.run_held(held, "song.set_tempo", json!({ "tempo": tempo }));
                    self.flash(cx, "Tempo", &format!("{bpm:.0} BPM"));
                }
            }
            _ => {}
        }
    }

    /// Where an encoder's last turn left `key`, if the model hasn't caught up
    /// with it yet, else `model`.
    fn pending(&self, key: u8, model: f64, now: Instant) -> f64 {
        match self.pending.get(&key) {
            Some(&(want, at)) if want != model && now.saturating_duration_since(at) < PENDING => {
                want
            }
            _ => model,
        }
    }

    /// The loop brace's start and length as the next turn finds them.
    fn brace(&self, brace: bl_model::ArrangementLoop, now: Instant) -> (i64, i64) {
        let start = self.pending(3, brace.start.get() as f64, now) as i64;
        let length = self.pending(4, brace.length.get() as f64, now) as i64;
        (start, length)
    }

    // Buttons --------------------------------------------------------------

    fn control_button(cx: &mut Cx, role: Role, cc: u8) {
        cx.control(
            role,
            RemoteMode::Absolute,
            Hit::NoteOn {
                key: cc,
                velocity: 127,
            },
        );
    }

    fn button(&mut self, channel: u8, cc: u8, press: bool, cx: &mut Cx) {
        // The MK3 repeats ▲ and ▼ as 68h and 69h on channel 16, for scripts
        // written for the Mini (R52, R53).
        if self.family == Gen::Mk3 && matches!(cc, 0x68 | 0x69) && channel != 0 {
            return;
        }
        if self.family == Gen::Mk4 && cc == 0x3F {
            self.shift = press;
            return;
        }
        if !press {
            return;
        }
        let Some(model) = self.last.clone() else {
            return;
        };
        match cc {
            0x6A | 0x6B => self.arrow(cc == 0x6A, &model, cx),
            0x66 | 0x67 => self.track_button(if cc == 0x66 { -1 } else { 1 }, &model, cx),
            0x68 => Self::control_button(cx, Role::SceneLaunch(0), cc),
            0x69 => self.cycle = self.cycle.next(self.family),
            0x33 | 0x34 if self.family == Gen::Mk4 => self.bank_button(cc == 0x34, &model, cx),
            0x34 => self.lock(&model, cx),
            0x25..=0x2C if self.faders => {
                Self::control_button(cx, self.fader_fn.role(cc - 0x25), cc);
            }
            0x2D if self.faders => {
                self.fader_fn = if self.fader_fn == TrackFn::Arm {
                    TrackFn::Select
                } else {
                    TrackFn::Arm
                };
            }
            0x4A => cx.run("midi.capture", json!({})),
            0x4B => Self::quantise(&model, cx),
            0x4C => {
                let enabled = !model.metronome;
                cx.run(
                    "transport.set_metronome_enabled",
                    json!({ "enabled": enabled }),
                );
            }
            0x4D if self.shift => cx.run("edit.redo", json!({})),
            0x4D => cx.run("edit.undo", json!({})),
            0x73 => self.play(&model, cx),
            0x74 => cx.run("transport.stop", json!({})),
            0x75 => self.record(&model, cx),
            0x76 => {
                let enabled = !model.document.settings().arrangement_loop.enabled;
                cx.run("transport.set_loop_enabled", json!({ "enabled": enabled }));
            }
            _ => {}
        }
    }

    /// Play starts the transport; with Shift it pauses it and continues it
    /// (R57, R67). The Minis' Play starts and stops it.
    fn play(&mut self, model: &ReadModel, cx: &mut Cx) {
        let id = match (self.shift, model.transport.playing) {
            (true, true) => "transport.stop",
            (true, false) => "transport.continue",
            (false, _) if self.header == 0x13 => "transport.play_stop",
            (false, _) => "transport.play",
        };
        cx.run(id, json!({}));
    }

    /// Record switches Session Record, on the MK4 while the Session View
    /// shows; there it switches Arrangement recording while the Arrangement
    /// does (R57, R67).
    fn record(&mut self, model: &ReadModel, cx: &mut Cx) {
        if self.header == 0x13 && self.shift {
            cx.run("midi.capture", json!({}));
        } else if self.family == Gen::Mk3 || model.session_view {
            cx.run("session.record", json!({}));
        } else {
            let recording = !model.transport.recording;
            cx.run("transport.set_recording", json!({ "recording": recording }));
        }
    }

    /// Quantise quantises the notes of the clip playing on the selected
    /// track to sixteenths (R57).
    fn quantise(model: &ReadModel, cx: &mut Cx) {
        let doc = &model.document;
        let clip = model.selection.tracks.last().and_then(|track| {
            let slot = model
                .slots
                .iter()
                .find(|s| s.track == *track && s.state == SlotState::Playing)?;
            doc.slot(*track, slot.scene)
        });
        if let Some(clip) = clip
            && let ClipData::Midi(midi) = &clip.data
        {
            let notes: Vec<usize> = (0..midi.notes.len()).collect();
            let grid = Ticks(TICKS_PER_QUARTER / 4);
            cx.run(
                "note.quantize",
                json!({ "clip": clip.id, "notes": notes, "grid": grid }),
            );
        }
    }

    /// ▲ and ▼: the ring a scene, the drum rack's pads sixteen notes while
    /// the DAW has them, or, while Device Select is held, the previous and
    /// next device (R52, R56, R70).
    fn arrow(&mut self, up: bool, model: &ReadModel, cx: &mut Cx) {
        if self.pads == Pads::DeviceSelect {
            self.step_device(if up { -1 } else { 1 }, model, cx);
        } else if let Some(drum) = model.drum.filter(|_| self.claimed) {
            let groups = if up { 1 } else { -1 };
            cx.run(
                "rack.move_pads",
                json!({ "rack": drum.rack, "groups": groups }),
            );
        } else if self.pads == Pads::Session {
            let step = if up { RingStep::Up } else { RingStep::Down };
            Self::control_button(cx, Role::Ring(step), if up { 0x6A } else { 0x6B });
        }
    }

    /// The track buttons: the selection a track along, moving the ring when
    /// it would leave it; with Shift on the MK4, the ring eight tracks
    /// (R52, R62).
    fn track_button(&mut self, dir: i32, model: &ReadModel, cx: &mut Cx) {
        if !(self.family == Gen::Mk4 && self.shift) {
            self.step_selection(dir, cx);
            return;
        }
        let Some(ring) = self.ring(model) else {
            return;
        };
        let len = model.columns.len();
        let first = ring
            .first_track
            .and_then(|t| model.columns.iter().position(|c| *c == t))
            .unwrap_or(0);
        let width = usize::from(RING.tracks);
        let new = if dir < 0 {
            first.saturating_sub(width)
        } else {
            (first + width).min(len.saturating_sub(width))
        };
        let (step, cc) = if dir < 0 {
            (RingStep::PageLeft, 0x66)
        } else {
            (RingStep::PageRight, 0x67)
        };
        Self::control_button(cx, Role::Ring(step), cc);
        let last = (new + width).min(len);
        self.flash(cx, "Tracks", &format!("{}-{last}", new + 1));
    }

    /// Moves the track selection `steps` along the Session View's columns,
    /// moving the ring just far enough to keep it in view (R52, R62).
    fn step_selection(&mut self, steps: i32, cx: &mut Cx) {
        let Some(model) = self.last.clone() else {
            return;
        };
        let Some(ring) = self.ring(&model) else {
            return;
        };
        let len = model.columns.len();
        if len == 0 {
            return;
        }
        let first = ring
            .first_track
            .and_then(|t| model.columns.iter().position(|c| *c == t))
            .unwrap_or(0);
        let last = len as i32 - 1;
        let target = match selected_index(&model) {
            Some(at) => (at as i32 + steps).clamp(0, last),
            None => first as i32,
        } as usize;
        let width = usize::from(RING.tracks);
        let new_first = if target < first {
            target
        } else if target >= first + width {
            target + 1 - width
        } else {
            first
        };
        if new_first != first {
            cx.run(
                "control_surface.move_ring",
                json!({ "row": self.row + 1, "track": model.columns[new_first] }),
            );
        }
        Self::control_button(cx, Role::Select((target - new_first) as u8), 0x66);
        let name = model
            .columns
            .get(target)
            .and_then(|t| model.document.track(*t))
            .map_or("", |t| t.name());
        self.flash(cx, "Track", name);
    }

    /// Moves the scene selection `steps` along the scenes.
    fn step_scene(&mut self, steps: i32, cx: &mut Cx) {
        let Some(model) = self.last.as_ref() else {
            return;
        };
        let scenes: Vec<_> = model.document.scenes().map(|s| s.id).collect();
        let Some(last) = scenes.len().checked_sub(1) else {
            return;
        };
        let at = model
            .selection
            .scenes
            .last()
            .and_then(|s| scenes.iter().position(|c| c == s));
        let target = match at {
            Some(at) => (at as i32 + steps).clamp(0, last as i32) as usize,
            None => 0,
        };
        cx.run("session.select", json!({ "scene": scenes[target] }));
        let value = format!("{}", target + 1);
        self.flash(cx, "Scene", &value);
    }

    /// Selects the device `steps` along the chain of the device in focus.
    fn step_device(&mut self, steps: i32, model: &ReadModel, cx: &mut Cx) {
        let Some(focus) = model.row(self.row).and_then(|r| r.focus.as_ref()) else {
            return;
        };
        let chain = devices(&model.document, focus.device);
        let Some(at) = chain.iter().position(|d| *d == focus.device) else {
            return;
        };
        let target = (at as i32 + steps).clamp(0, chain.len() as i32 - 1) as usize;
        cx.run("device.select", json!({ "device": chain[target] }));
    }

    /// Device Lock locks the device in focus to the surface, or gives it
    /// back (R56).
    fn lock(&mut self, model: &ReadModel, cx: &mut Cx) {
        let Some(focus) = model.row(self.row).and_then(|r| r.focus.as_ref()) else {
            return;
        };
        let row = self.row + 1;
        if focus.lock.is_some() {
            cx.run("control_surface.unlock", json!({ "row": row }));
        } else {
            cx.run(
                "control_surface.lock",
                json!({ "row": row, "device": focus.device }),
            );
        }
    }

    /// The MK4's encoder bank buttons: in Plugin the device's banks, or with
    /// Shift its neighbours on the track; in Mixer the page; in Sends the
    /// send (R64).
    fn bank_button(&mut self, next: bool, model: &ReadModel, cx: &mut Cx) {
        let cc = if next { 0x34 } else { 0x33 };
        match self.pots {
            Pots::Device if self.shift => self.step_device(if next { 1 } else { -1 }, model, cx),
            Pots::Device => {
                let role = if next {
                    Role::NextBank
                } else {
                    Role::PreviousBank
                };
                Self::control_button(cx, role, cc);
            }
            Pots::Volume | Pots::Pan => {
                self.page = if next { Page::Pan } else { Page::Volume };
                self.pots = self.mixer_pots();
            }
            Pots::Send(_) => {
                let returns = model.document.returns().len().max(1);
                let send = i32::from(self.send) + if next { 1 } else { -1 };
                self.send = send.clamp(0, returns as i32 - 1) as u8;
                self.pots = Pots::Send(self.send);
            }
            Pots::Transport | Pots::Own => {}
        }
    }
}

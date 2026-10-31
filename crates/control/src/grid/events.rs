//! What the grid surface does with the controller's messages, and on each
//! tick: pads, buttons, faders, the layout it tells the controller to show,
//! the lights, the faders' positions and the clock (supported controllers
//! R7 to R34).

use super::*;

impl Grid {
    // The controller's messages ------------------------------------------

    fn sysex_in(&mut self, bytes: &[u8]) {
        // The Pro reports each layout it shows (R29).
        if self.model == Model::Pro
            && bytes.len() == 11
            && bytes[..6] == [0xF0, 0x00, 0x20, 0x29, 0x02, 0x0E]
            && bytes[6] == 0x00
        {
            self.layout = match bytes[7] {
                0 => Layout::Session,
                1 => Layout::Fader(bytes[8].min(3)),
                4 => Layout::Note,
                _ => Layout::Own,
            };
        }
    }

    /// Anything pressed besides a Mixer button held makes its release go
    /// back to the view before it (R20).
    fn used(&mut self) {
        if let Some(peek) = &mut self.peek {
            peek.used = true;
        }
    }

    /// The track and scene at a column and a row of the ring.
    fn slot_at(&self, column: u8, row: u8) -> Option<(TrackId, SceneId)> {
        let ring = self.ring(self.last.as_ref()?)?;
        Some((
            *ring.tracks.get(usize::from(column))?,
            *ring.scenes.get(usize::from(row))?,
        ))
    }

    fn pad(&mut self, note: u8, velocity: u8, press: bool, cx: &mut Cx) {
        let Some((column, row)) = pad_at(note) else {
            return;
        };
        if !press {
            if self.slots_down.remove(&note) {
                cx.control(
                    Role::Slot { column, row },
                    RemoteMode::Absolute,
                    Hit::NoteOff { key: note },
                );
            }
            return;
        }
        self.used();
        if self.overview {
            self.overview_press(column, row, cx);
            return;
        }
        if let Some(f) = self.bottom_row()
            && row == 7
        {
            cx.control(
                f.role(column),
                RemoteMode::Absolute,
                Hit::NoteOn {
                    key: note,
                    velocity,
                },
            );
            return;
        }
        if self.model == Model::Pro && self.modified_pad(column, row, cx) {
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

    /// A pad pressed while one of the Pro's modifiers is held (R32); whether
    /// one was.
    fn modified_pad(&mut self, column: u8, row: u8, cx: &mut Cx) -> bool {
        let held = self.shift || self.clear || self.duplicate || self.quantise;
        let Some((track, scene)) = self.slot_at(column, row) else {
            return held;
        };
        let Some(model) = &self.last else {
            return held;
        };
        let doc = &model.document;
        let clip = doc.slot(track, scene);
        if self.shift && self.duplicate {
            if let Some(clip) = clip {
                cx.run("clip.duplicate_loop", json!({ "clips": [clip.id] }));
            }
        } else if self.clear {
            cx.run("session.select", json!({ "track": track, "scene": scene }));
            cx.run("session.delete", json!({}));
        } else if self.duplicate {
            // The slot below, replacing what is there.
            let below = doc
                .scene_position(scene)
                .and_then(|at| doc.scenes().nth(at + 1))
                .map(|s| s.id);
            if let (Some(clip), Some(below)) = (clip, below) {
                cx.run(
                    "session.copy_clip_to_slot",
                    json!({ "clip": clip.id, "track": track, "scene": below }),
                );
            }
        } else if self.quantise {
            if let Some(clip) = clip
                && let ClipData::Midi(midi) = &clip.data
            {
                let notes: Vec<usize> = (0..midi.notes.len()).collect();
                cx.run(
                    "note.quantize",
                    json!({
                        "clip": clip.id,
                        "notes": notes,
                        "grid": Ticks(TICKS_PER_QUARTER / 4),
                    }),
                );
            }
        } else if self.shift {
            cx.run("session.select", json!({ "track": track, "scene": scene }));
        }
        held
    }

    /// A pad of Session Overview moves the ring to its block (R33).
    fn overview_press(&mut self, column: u8, row: u8, cx: &mut Cx) {
        let Some(model) = &self.last else {
            return;
        };
        let track = model.columns.get(usize::from(column) * 8).copied();
        let scene = model
            .document
            .scenes()
            .nth(usize::from(row) * 8)
            .map(|s| s.id);
        if let (Some(track), Some(scene)) = (track, scene) {
            cx.run(
                "control_surface.move_ring",
                json!({ "row": self.row + 1, "track": track, "scene": scene }),
            );
        }
    }

    fn drum(&mut self, note: u8, velocity: u8, press: bool, cx: &mut Cx) {
        if !press {
            if let Some(key) = self.drums_down.remove(&note) {
                cx.play(MidiMessage::note_off(0, key));
            }
            return;
        }
        let drum = self.last.as_ref().and_then(|m| m.drum);
        let (Some(drum), Some((column, row))) = (drum, pad_at(note)) else {
            return;
        };
        let Some(key) = drum_note(&drum, column, 7 - row) else {
            return;
        };
        self.drums_down.insert(note, key);
        cx.play(MidiMessage::note_on(0, key, velocity));
        cx.run("pad.select", json!({ "rack": drum.rack, "note": key }));
    }

    fn arrow(&mut self, dir: Dir, cx: &mut Cx) {
        let drum = self
            .last
            .as_ref()
            .filter(|m| self.drum_visible(m))
            .and_then(|m| m.drum);
        if let Some(drum) = drum {
            let args = match dir {
                Dir::Up => json!({ "rack": drum.rack, "groups": 1 }),
                Dir::Down => json!({ "rack": drum.rack, "groups": -1 }),
                Dir::Left => json!({ "rack": drum.rack, "rows": -1 }),
                Dir::Right => json!({ "rack": drum.rack, "rows": 1 }),
            };
            cx.run("rack.move_pads", args);
        } else if matches!(self.layout, Layout::Session | Layout::Fader(_)) {
            let step = match dir {
                Dir::Up => RingStep::Up,
                Dir::Down => RingStep::Down,
                Dir::Left => RingStep::Left,
                Dir::Right => RingStep::Right,
            };
            self.control_button(cx, Role::Ring(step), self.model.arrow(dir));
        }
    }

    fn control_button(&self, cx: &mut Cx, role: Role, cc: u8) {
        cx.control(
            role,
            RemoteMode::Absolute,
            Hit::NoteOn {
                key: cc,
                velocity: 127,
            },
        );
    }

    fn button(&mut self, at: Instant, cc: u8, press: bool, cx: &mut Cx) {
        let Some(button) = self.model.button(cc) else {
            return;
        };
        if !press {
            self.released(button, cc, at, cx);
            return;
        }
        let mixer_button =
            self.model == Model::X && self.mixer.is_some() && matches!(button, Button::Scene(_));
        if !mixer_button {
            self.used();
        }
        match button {
            Button::Arrow(dir) => self.arrow(dir, cx),
            Button::Scene(row) if mixer_button => self.choose_mix(MIXES[usize::from(row)], cc),
            Button::Scene(row) => self.control_button(cx, Role::SceneLaunch(row), cc),
            Button::Session => self.session_pressed(),
            Button::Note => {
                self.layout = Layout::Note;
                self.overview = false;
            }
            Button::Own => self.layout = Layout::Own,
            Button::Capture => self.capture_down = Some(at),
            Button::StopSoloMute => self.cycle = self.cycle.next(),
            Button::TrackSelect(column) => self.track_select(column, cx),
            Button::Control(n) => self.track_control(n, cx),
            Button::Shift => self.shift = true,
            Button::Clear => self.clear = true,
            Button::Duplicate => self.duplicate = true,
            Button::Quantise => self.quantise = true,
            Button::Play => cx.run("transport.play_stop", json!({})),
            Button::Record if self.shift => cx.run("midi.capture", json!({})),
            Button::Record => cx.run("session.record", json!({})),
        }
    }

    fn released(&mut self, button: Button, cc: u8, at: Instant, cx: &mut Cx) {
        match button {
            Button::Shift => self.shift = false,
            Button::Clear => self.clear = false,
            Button::Duplicate => self.duplicate = false,
            Button::Quantise => self.quantise = false,
            Button::Session if self.model == Model::Pro => self.overview = false,
            Button::Capture => {
                // A tap is Session Record; held longer, Capture MIDI (R22).
                if let Some(down) = self.capture_down.take() {
                    let id = if at.saturating_duration_since(down) < TAP {
                        "session.record"
                    } else {
                        "midi.capture"
                    };
                    cx.run(id, json!({}));
                }
            }
            Button::Scene(_) if self.model == Model::X => {
                // A Mixer button held while other controls were pressed
                // gives the view before it back (R20).
                if self.peek_button == Some(cc)
                    && let Some(peek) = self.peek.take()
                {
                    self.peek_button = None;
                    if peek.used {
                        self.mixer = peek.before;
                        self.layout_dirty = true;
                    }
                }
            }
            _ => {}
        }
    }

    fn choose_mix(&mut self, mix: Mix, cc: u8) {
        if self.mixer == Some(mix) {
            self.mixer = None;
            self.peek = None;
            self.peek_button = None;
        } else {
            self.peek = Some(Peek {
                before: self.mixer,
                used: false,
            });
            self.peek_button = Some(cc);
            self.mixer = Some(mix);
        }
        self.layout_dirty = true;
    }

    fn session_pressed(&mut self) {
        match self.model {
            Model::X => {
                if self.layout == Layout::Session {
                    self.mixer = self.mixer.is_none().then_some(Mix::Volume);
                } else {
                    self.layout = Layout::Session;
                }
                self.layout_dirty = true;
            }
            Model::Mini => {
                self.layout = Layout::Session;
                self.layout_dirty = true;
            }
            Model::Pro => self.overview = true,
        }
    }

    /// A Track Select button of the Pro: a device on the Device page, else
    /// the track function (R31).
    fn track_select(&mut self, column: u8, cx: &mut Cx) {
        if self.layout == Layout::Fader(3) {
            let device = self
                .last
                .as_ref()
                .and_then(|m| self.devices(m).get(usize::from(column)).copied());
            if let Some(device) = device {
                cx.run("device.select", json!({ "device": device }));
            }
            return;
        }
        let f = self.row_fn.unwrap_or(TrackFn::Select);
        self.control_button(cx, f.role(column), 101 + column);
    }

    /// The Pro's track controls (R31, R32).
    fn track_control(&mut self, n: u8, cx: &mut Cx) {
        if self.shift {
            match n {
                1 => cx.run("edit.undo", json!({})),
                2 => cx.run("edit.redo", json!({})),
                3 => {
                    let on = self.last.as_ref().is_some_and(|m| m.metronome);
                    cx.run("transport.set_metronome_enabled", json!({ "enabled": !on }));
                }
                6 => cx.run("transport.tap_tempo", json!({})),
                _ => {}
            }
            return;
        }
        let toggle = |current: &mut Option<TrackFn>, f| {
            *current = (*current != Some(f)).then_some(f);
        };
        match n {
            1 => toggle(&mut self.row_fn, TrackFn::Arm),
            2 => toggle(&mut self.row_fn, TrackFn::Mute),
            3 => toggle(&mut self.row_fn, TrackFn::Solo),
            8 => toggle(&mut self.row_fn, TrackFn::Stop),
            4 | 5 | 7 => self.show_page(match n {
                4 => 0,
                5 => 1,
                _ => 3,
            }),
            _ => {
                // Pressing Sends again steps to the next send.
                if self.layout == Layout::Fader(2) {
                    let returns = self
                        .last
                        .as_ref()
                        .map_or(0, |m| m.document.returns().len())
                        .max(1);
                    self.send = ((usize::from(self.send) + 1) % returns) as u8;
                } else {
                    self.show_page(2);
                }
            }
        }
    }

    /// Shows a page of the Pro's Fader layout, or the Session layout when it
    /// shows already.
    fn show_page(&mut self, page: u8) {
        self.layout = if self.layout == Layout::Fader(page) {
            Layout::Session
        } else {
            Layout::Fader(page)
        };
        self.layout_dirty = true;
    }

    fn fader(&mut self, cc: u8, value: u8, cx: &mut Cx) {
        let Some(faders) = self.faders() else {
            return;
        };
        let base = FADER_CC + self.fader_page() * 8;
        let Some(index) = cc.checked_sub(base).filter(|i| *i < 8) else {
            return;
        };
        self.used();
        self.fader_at[usize::from(index)] = Some(value);
        cx.control(
            faders.role(index),
            RemoteMode::Absolute,
            Hit::Value {
                value: u16::from(value),
                full: 127,
            },
        );
    }

    /// The Pro's fader bank, whose controller numbers sit above the last's.
    fn fader_page(&self) -> u8 {
        match (self.model, self.layout) {
            (Model::Pro, Layout::Fader(page)) => page,
            _ => 0,
        }
    }

    // The tick -----------------------------------------------------------

    /// The message that sets the grid faders up for what they reach (R21,
    /// R31): all eight, the ones past the last track hidden.
    fn fader_message(&self, model: &ReadModel, faders: Faders) -> Vec<u8> {
        let page = self.fader_page();
        let horizontal = u8::from(faders == Faders::Pan);
        let mut body = match self.model {
            Model::Pro => vec![0x01, page, horizontal],
            _ => vec![0x01, 0x00, horizontal],
        };
        for i in 0..8u8 {
            body.extend([
                i,
                u8::from(faders == Faders::Pan),
                FADER_CC + page * 8 + i,
                self.fader_colour(model, faders, i).unwrap_or(0),
            ]);
        }
        self.sysex(&body)
    }

    /// A fader's colour, or `None` where it is hidden: past the last track,
    /// or past the bank's last parameter (R14).
    fn fader_colour(&self, model: &ReadModel, faders: Faders, index: u8) -> Option<u8> {
        let doc = &model.document;
        let track = self
            .ring(model)
            .and_then(|r| r.tracks.get(usize::from(index)).copied());
        match faders {
            Faders::Volume => track.map(|_| colour::GREEN),
            Faders::Pan => track.map(|t| nearest(Self::track_colour(doc, t))),
            Faders::Send(send) => {
                let t = doc.track(track?)?;
                (t.kind() != TrackKind::Return && usize::from(send) < doc.returns().len()).then(
                    || match send {
                        0 => colour::PURPLE,
                        1 => colour::BLUE,
                        n => colour::RAINBOW[usize::from(n) % colour::RAINBOW.len()],
                    },
                )
            }
            Faders::Device => {
                let focus = model.row(self.row)?.focus.as_ref()?;
                focus.params.get(usize::from(index))?;
                Some(colour::RAINBOW[focus.bank % colour::RAINBOW.len()])
            }
        }
    }

    /// Tells the controller what changed in the modes it keeps: its drum
    /// layout, its faders and the layout it shows (R21, R23, R31, R34).
    fn sync(&mut self, model: &ReadModel, cx: &mut Cx) {
        if matches!(self.model, Model::X | Model::Pro) {
            let drum = model.drum.is_some();
            if self.drum_mode != Some(drum) {
                self.drum_mode = Some(drum);
                cx.send(self.sysex(&[0x0F, u8::from(drum)]));
            }
        }
        let faders = self.faders();
        match faders {
            Some(faders) => {
                let message = self.fader_message(model, faders);
                if self.fader_config.as_ref() != Some(&message) {
                    cx.send(message.clone());
                    self.fader_config = Some(message);
                    self.fader_at = [None; 8];
                }
            }
            None => self.fader_config = None,
        }
        if std::mem::take(&mut self.layout_dirty) {
            match (self.model, self.layout) {
                (Model::X, Layout::Session) => {
                    let layout = if faders.is_some() { 0x0D } else { 0x00 };
                    cx.send(self.sysex(&[0x00, layout]));
                }
                (Model::Mini, Layout::Session) => cx.send(self.session_layout()),
                (Model::Pro, Layout::Session) => cx.send(self.session_layout()),
                (Model::Pro, Layout::Fader(page)) => {
                    cx.send(self.sysex(&[0x00, 0x01, page, 0x00]));
                }
                _ => {}
            }
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

    /// Sends the faders' positions that moved since they were last sent,
    /// whatever moved them (R14).
    fn positions(&mut self, model: &ReadModel, cx: &mut Cx) {
        let Some(faders) = self.faders() else {
            return;
        };
        let Some(row) = model.row(self.row) else {
            return;
        };
        let page = self.fader_page();
        for index in 0..8u8 {
            let Some(&(_, position)) = row.positions.iter().find(|(r, _)| *r == faders.role(index))
            else {
                continue;
            };
            let value = (position.clamp(0.0, 1.0) * 127.0).round() as u8;
            let slot = &mut self.fader_at[usize::from(index)];
            if slot.is_none_or(|sent| sent.abs_diff(value) > 1) {
                *slot = Some(value);
                let cc = FADER_CC + page * 8 + index;
                cx.send_for(0x1_0000 | u32::from(cc), vec![0xB4, cc, value]);
            }
        }
    }

    fn pulses(&mut self, model: &ReadModel, cx: &mut Cx) {
        pulses(&mut self.clock, model, cx);
    }
}

/// Sends the clock the controller's flashing and pulsing keep time with: 24
/// pulses a quarter note at the set's tempo, each stamped with its moment;
/// `next` is when the following one is due (R11).
pub(super) fn pulses(next: &mut Option<Instant>, model: &ReadModel, cx: &mut Cx) {
    let now = cx.now();
    let period = Duration::from_secs_f64(60.0 / (24.0 * model.tempo.get().max(1.0)));
    let mut due = next.unwrap_or(now);
    if due + Duration::from_millis(200) < now {
        due = now;
    }
    for _ in 0..64 {
        if due >= now + AHEAD {
            break;
        }
        cx.send_at(due, vec![0xF8]);
        due += period;
    }
    *next = Some(due);
}

fn control_id(layer: Layer, number: u8) -> u32 {
    ((layer as u32) << 8) | u32::from(number)
}

impl ControlSurface for Grid {
    fn start(&mut self, cx: &mut Cx) {
        self.row = cx.row();
        // DAW mode, then the Session layout (R18, R25, R29).
        cx.send(self.sysex(&[0x10, 0x01]));
        cx.send(self.session_layout());
        self.clock = Some(cx.now());
    }

    fn stop(&mut self, cx: &mut Cx) {
        // The pads still down can no longer be released: let their notes and
        // slots go.
        for (_, key) in std::mem::take(&mut self.drums_down) {
            cx.play(MidiMessage::note_off(0, key));
        }
        for note in std::mem::take(&mut self.slots_down) {
            if let Some((column, row)) = pad_at(note) {
                cx.control(
                    Role::Slot { column, row },
                    RemoteMode::Absolute,
                    Hit::NoteOff { key: note },
                );
            }
        }
        // Every light out, then the controller's own state back (R17).
        for ((layer, number), _) in std::mem::take(&mut self.lit) {
            for bytes in light_bytes(layer, number, None) {
                cx.send(bytes);
            }
        }
        cx.send(self.sysex(&[0x10, 0x00]));
    }

    fn message(&mut self, at: Instant, bytes: &[u8], cx: &mut Cx) {
        let Some(&status) = bytes.first() else {
            return;
        };
        if status == 0xF0 {
            self.sysex_in(bytes);
            return;
        }
        let (kind, channel) = (status & 0xF0, status & 0x0F);
        let first = bytes.get(1).copied().unwrap_or(0);
        let second = bytes.get(2).copied().unwrap_or(0);
        let press = kind == 0x90 && second > 0;
        match (kind, channel) {
            (0x90 | 0x80, 0) => self.pad(first, second, press, cx),
            (0x90 | 0x80, 8) => self.drum(first, second, press, cx),
            (0xB0, 0) => self.button(at, first, second > 0, cx),
            (0xB0, 4) => self.fader(first, second, cx),
            _ => {}
        }
    }

    fn tick(&mut self, model: &ReadModel, cx: &mut Cx) {
        self.row = cx.row();
        // The messages of this tick see the model before it.
        self.last = Some(model.clone());
        self.sync(model, cx);
        let want = self.render(model);
        self.show(want, cx);
        self.positions(model, cx);
        self.pulses(model, cx);
    }

    fn state(&self) -> SurfaceState {
        SurfaceState {
            ring: Some(RING),
            mode: Some(self.mode()),
            watch: self
                .faders()
                .map(|f| (0..8).map(|i| f.role(i)).collect())
                .unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bl_core::clock::TestClock;
    use bl_core::id::Id;
    use bl_model::{DeviceId, Editor};

    use super::*;
    use crate::surface::Ask;

    // Spec: supported-controllers R4. A surface stopped with pads down lets
    // their notes and slots go, since the releases can no longer arrive.
    #[test]
    fn stopping_with_pads_down_ends_their_notes_and_slots() {
        let mut grid = Grid::new(Model::X);
        let mut model = ReadModel::at_rest(Arc::new(Editor::new().document().clone()));
        model.drum = Some(DrumView {
            rack: DeviceId(Id::new(5).unwrap()),
            first: 36,
            selected: None,
            filled: 0,
        });
        let now = TestClock::new().now();
        let mut cx = Cx::new(0, now);
        grid.tick(&model, &mut cx);
        let note = |column, row| {
            (0..128)
                .find(|n| pad_at(*n) == Some((column, row)))
                .unwrap()
        };
        grid.message(now, &[0x98, note(0, 7), 100], &mut cx);
        grid.message(now, &[0x90, note(0, 0), 100], &mut cx);
        assert_eq!(cx.played, [MidiMessage::note_on(0, 36, 100)]);

        let mut cx = Cx::new(0, now);
        grid.stop(&mut cx);
        assert_eq!(cx.played, [MidiMessage::note_off(0, 36)]);
        assert!(cx.requests.iter().any(|ask| matches!(
            ask,
            Ask::Control(c) if c.role == (Role::Slot { column: 0, row: 0 })
                && c.hit == (Hit::NoteOff { key: note(0, 0) })
        )));
    }
}

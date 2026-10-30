//! MIDI input, MIDI learn, MIDI I/O and MIDI files.

use super::{Effect, MIDI_LAMP, SYNC_LAMP, Shell, ShellError, midi_file_error, to_value};
use crate::midi::MidiHost;
use bl_commands::NoArgs;
use bl_shell_spec::midi_files as midi_ids;
use bl_shell_spec::midi_io as midi_io_ids;
use bl_shell_spec::transport as transport_ids;
use serde_json::Value;
use std::path::PathBuf;

impl Shell {
    /// Lights the Control Bar's MIDI indicators for a moment when messages
    /// have reached tracks' inputs or left for ports since the last call;
    /// returns whether either changed (midi-io R25; external-devices R7).
    pub(super) fn track_midi_activity(&mut self) -> bool {
        let now = std::time::Instant::now();
        let seen = (
            self.midi.sender().stats().snapshot().to_tracks,
            self.midi.output().stats().snapshot().sent,
        );
        let counts = [(seen.0, self.midi_seen.0), (seen.1, self.midi_seen.1)];
        let mut changed = false;
        for (lit, (new, old)) in self.midi_lit.iter_mut().zip(counts) {
            if new != old {
                *lit = Some(now);
                changed = true;
            } else if lit.is_some_and(|since| now.duration_since(since) >= MIDI_LAMP) {
                *lit = None;
                changed = true;
            }
        }
        self.midi_seen = seen;
        // The lamp follows the test clock when a test paces the engine by
        // hand.
        changed |= self.track_link(self.midi.now());
        changed | self.track_sync_activity(self.midi.now())
    }

    /// Lights the lower sync lamp when the engine has sent a quarter note of
    /// MIDI clock since the last call, and the upper when one has arrived,
    /// and darkens each 100 ms later (midi-clock R12, R16); keeps what the
    /// engine follows. Returns whether the lamps or the followed tempo
    /// changed.
    fn track_sync_activity(&mut self, now: std::time::Instant) -> bool {
        let quarters = self.audio.sync_out_state().quarters;
        let before = self.sync_lit.is_some();
        if quarters != self.sync_seen {
            self.sync_lit = Some(now);
        } else if self
            .sync_lit
            .is_some_and(|since| now.duration_since(since) >= SYNC_LAMP)
        {
            self.sync_lit = None;
        }
        self.sync_seen = quarters;
        let moved = self.hear_sync_in();
        let state = self.sync_in;
        let upper = self.sync_in_lit.is_some();
        if state.quarters != self.sync_in_seen {
            self.sync_in_lit = Some(now);
        } else if self
            .sync_in_lit
            .is_some_and(|since| now.duration_since(since) >= SYNC_LAMP)
        {
            self.sync_in_lit = None;
        }
        self.sync_in_seen = state.quarters;
        before != self.sync_lit.is_some() || upper != self.sync_in_lit.is_some() || moved
    }

    /// Reads what the engine reports of the clock it follows. Returns
    /// whether what the Tempo field shows changed: that the song follows, or
    /// the tempo to hundredths.
    pub(super) fn hear_sync_in(&mut self) -> bool {
        let state = self.audio.sync_in_state();
        let shown =
            |s: &bl_engine::SyncInState| (s.following, s.tempo.map(|t| (t * 100.0).round()));
        let moved = shown(&self.sync_in) != shown(&state);
        self.sync_in = state;
        moved
    }

    /// Whether the lower sync lamp is lit: a quarter note of MIDI clock left
    /// for a port a moment ago (L-CTRL-028, the MIDI clock spec, R12).
    #[must_use]
    pub fn sync_output_lit(&self) -> bool {
        self.sync_lit.is_some()
    }

    /// Whether the upper sync lamp is lit: a quarter note of MIDI clock
    /// arrived at a sync input a moment ago (the MIDI clock spec, R16).
    #[must_use]
    pub fn sync_input_lit(&self) -> bool {
        self.sync_in_lit.is_some()
    }

    /// Whether EXT is on: the song follows the incoming MIDI clock. It is
    /// kept in the preferences, and off while no input has Sync on (the MIDI
    /// clock spec, R15).
    #[must_use]
    pub fn external_sync(&self) -> bool {
        self.midi.external_sync() && self.sync_lamps_shown()
    }

    /// The tempo the song follows, in BPM, while EXT is on and a tempo has
    /// been found: what the Tempo field shows, and why it can't be changed
    /// (the MIDI clock spec, R17).
    #[must_use]
    pub fn external_tempo(&self) -> Option<f64> {
        self.sync_in
            .following
            .then_some(self.sync_in.tempo)
            .flatten()
    }

    /// Whether the Control Bar shows the sync lamp pair: some input port has
    /// Sync on (midi-clock R15).
    #[must_use]
    pub fn sync_lamps_shown(&self) -> bool {
        self.midi.ports().iter().any(|p| {
            p.direction == bl_shell_spec::midi_io::PortDirection::Input && p.sync && p.open
        })
    }

    /// Whether the Control Bar shows the Resync External Hardware button:
    /// Show Resync Button is on (midi-clock R10).
    #[must_use]
    pub fn resync_button_shown(&self) -> bool {
        self.midi.show_resync_button()
    }

    /// Runs `sync.state`.
    pub(super) fn run_sync(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        Self::args::<NoArgs>(id, args)?;
        let engine = self.audio.sync_out_state();
        let incoming = self.audio.sync_in_state();
        let leading_input = incoming.lead.and_then(|lead| {
            self.midi
                .ports()
                .into_iter()
                .find(|p| bl_core::midi::PortId::from_name(&p.name) == lead)
                .map(|p| p.name)
        });
        Ok(to_value(&bl_shell_spec::sync::SyncState {
            output_ports: self.midi.sync_outputs().len() as u32,
            sending: engine.sending,
            output_lamp: self.sync_output_lit(),
            clocks_sent: engine.clocks,
            clocks_late: engine.late,
            clocks_dropped: engine.dropped,
            input_ports: self.midi.sync_inputs().len() as u32,
            external_sync_available: self.sync_lamps_shown(),
            external_sync: self.external_sync(),
            leading_input,
            receiving: incoming.receiving,
            input_lamp: self.sync_input_lit(),
            tempo: incoming.tempo,
            clocks_received: incoming.clocks,
            clocks_set_aside: incoming.glitches,
            sync_dropped: incoming.dropped,
            timecode: incoming
                .timecode
                .map(|t| bl_shell_spec::sync::TimecodeReport {
                    time: t.time.to_string(),
                    rate: t.rate.into(),
                    running: t.running,
                    position_ticks: t.position.get(),
                }),
        }))
    }

    /// Runs `transport.set_external_sync`: EXT, kept in the preferences
    /// (the MIDI clock spec, R15).
    pub(super) fn run_set_external_sync(&mut self, args: Value) -> Result<Value, ShellError> {
        let id = transport_ids::SET_EXTERNAL_SYNC;
        let transport_ids::SetExternalSyncArgs { on } = Self::args(id, args)?;
        let on = on.unwrap_or(!self.external_sync());
        if on && !self.sync_lamps_shown() {
            return Err(ShellError::Failed {
                command: id.to_owned(),
                message: "no input port has Sync on: switch one on with midi.set_port".to_owned(),
            });
        }
        self.midi.set_external_sync(on);
        self.prefs.midi = self.midi.prefs();
        self.save_preferences();
        self.send_sync();
        Ok(to_value(&transport_ids::ExternalSync { on }))
    }

    /// Whether the Control Bar's MIDI track input and output indicators are
    /// lit: a message reached a track's input, or left for a port, a moment
    /// ago (L-CTRL-026).
    #[must_use]
    pub fn midi_indicators(&self) -> (bool, bool) {
        (self.midi_lit[0].is_some(), self.midi_lit[1].is_some())
    }

    /// Plays MIDI through `backend` from now on: the platform's MIDI system
    /// in the app, the test backend in tests. The ports keep the switches
    /// the preferences hold.
    pub fn use_midi_backend(&mut self, backend: Box<dyn bl_midi_io::MidiBackend>) {
        let sender = self.midi.sender().clone();
        let output = self.midi.output().clone();
        let clock = self.midi.test_clock();
        self.midi = MidiHost::new(
            backend,
            sender,
            output,
            &self.prefs.midi,
            std::sync::Arc::clone(&self.push_display),
        );
        self.midi
            .set_surface_templates(bl_control::template::kinds(&self.surface.templates));
        if let Some(clock) = clock {
            self.midi.set_test_clock(clock);
        }
        self.send_sync();
    }

    /// Drives the shell's clocks from `clock`: the computer keyboard stamps
    /// its notes with it, and the next audio stream times its blocks from it
    /// (T199). Call it after [`Shell::use_midi_backend`] and before
    /// [`Shell::start_audio`].
    pub fn use_test_clock(&mut self, clock: bl_core::clock::TestClock) {
        self.midi.set_test_clock(clock.clone());
        self.audio.set_test_clock(clock);
    }

    /// Claims Push 2's display interface with `claim` from now on, in place
    /// of `nusb`'s: a test's sink.
    pub fn use_push_display(&mut self, claim: bl_push::Claim) {
        self.push_display = std::sync::Arc::clone(&claim);
        self.midi.set_push_display(claim);
    }

    /// The MIDI ports and the computer MIDI keyboard.
    #[must_use]
    pub fn midi(&self) -> &MidiHost {
        &self.midi
    }

    /// Whether the computer MIDI keyboard is on, so its letter keys play
    /// notes instead of running shortcuts.
    #[must_use]
    pub fn computer_keyboard_on(&self) -> bool {
        self.midi.keyboard_info().on
    }

    /// Lists the MIDI ports again, so a device plugged in or removed shows.
    /// Returns whether the list changed.
    pub fn refresh_midi(&mut self) -> bool {
        let before = self.midi.ports();
        self.midi.refresh();
        self.midi_listed = std::time::Instant::now();
        let changed = before != self.midi.ports();
        if changed {
            self.send_sync();
        }
        // Spec: push-2 R2. A Push 2 that took a row keeps it for the next start.
        let prefs = self.midi.prefs();
        if prefs != self.prefs.midi {
            self.prefs.midi = prefs;
            self.save_preferences();
        }
        changed
    }

    /// Tells the audio engine which output ports have Sync on and their
    /// options, which input ports have Sync on and their delay, and whether
    /// EXT is on (the MIDI clock spec, R3, R14, R15).
    pub(super) fn send_sync(&mut self) {
        self.audio.set_sync_outputs(self.midi.sync_outputs());
        self.audio.set_sync_inputs(self.midi.sync_inputs());
        self.audio.set_external_sync(self.external_sync());
    }

    /// Spec: midi-chord R15 and midi-cc-sender R7. While a Chord device's
    /// Learn is on, the keys played set its shifts; while a CC Control
    /// device's Learn is on, an incoming controller is handed to its next
    /// Custom dial without one. Each change is one undo step. Returns
    /// whether the set changed.
    pub(super) fn learn_midi(&mut self) -> bool {
        use bl_devices::midi_cc::{
            CC_CONTROL, DIALS, LEARN as CC_LEARN, cc_is_listed, cc_type_for, dial_cc, dial_value,
        };
        use bl_devices::stack::{
            LEARN as CHORD_LEARN, STACK, USE_SCALE as CHORD_USE_SCALE, learn, shift,
        };
        use bl_model::{Command, DeviceId, DeviceKind, ParamId};
        let devices: Vec<(DeviceId, String)> = self
            .editor
            .document()
            .tracks()
            .flat_map(|track| track.devices_deep())
            .filter_map(|device| match &device.kind {
                DeviceKind::Native { device_type } => Some((device.id, device_type.clone())),
                _ => None,
            })
            .collect();
        let learning = |device_type: &str, param: ParamId| -> Vec<DeviceId> {
            devices
                .iter()
                .filter(|(id, kind)| {
                    kind == device_type
                        && self
                            .editor
                            .document()
                            .device(*id)
                            .and_then(|device| device.params.get(&param).copied())
                            .is_some_and(|on| on >= 0.5)
                })
                .map(|(id, _)| *id)
                .collect()
        };
        let chords = learning(STACK, CHORD_LEARN);
        let senders = learning(CC_CONTROL, CC_LEARN);
        let sender = self.midi.sender().clone();
        // Spec: step-recording R3, R8. Step recording reads the keys held
        // through the same copy of the input.
        let stepping = self.step_recording();
        if !stepping {
            self.forget_held();
        }
        if chords.is_empty() && senders.is_empty() && !stepping {
            if self.chord_learn.tapping {
                sender.set_tap(false);
                self.chord_learn.tapping = false;
                self.chord_learn.clear();
            }
            return false;
        }
        if !self.chord_learn.tapping {
            sender.set_tap(true);
            self.chord_learn.tapping = true;
        }
        let mut heard = Vec::new();
        sender.take_notes(&mut heard);
        if stepping {
            self.hear_held(&heard);
        }
        let mut changed = false;
        if !chords.is_empty()
            && let Some(held) = self.chord_learn.listen_from(&heard)
        {
            // Spec: midi-chord R17. A Chord counting degrees learns the
            // degrees from the root to each key, in the current scale.
            let set_scale = self.editor.document().settings().scale;
            let shifts: Vec<(DeviceId, [i32; 6])> = chords
                .iter()
                .map(|device| {
                    let degrees = self
                        .editor
                        .document()
                        .device(*device)
                        .and_then(|device| device.params.get(&CHORD_USE_SCALE).copied())
                        .is_some_and(|on| on >= 0.5)
                        && !self.scales_hidden();
                    let scale = if degrees {
                        set_scale
                    } else {
                        bl_model::Scale::default()
                    };
                    (*device, learn(&held, &scale))
                })
                .collect();
            self.edit(|editor| {
                editor.begin("Learn Chord");
                for (device, shifts) in &shifts {
                    for (n, value) in shifts.iter().enumerate() {
                        editor
                            .apply(Command::SetDeviceParam {
                                device: *device,
                                param: shift(n as u32),
                                value: f64::from(*value),
                            })
                            .expect("a shift takes any whole number in range");
                    }
                }
                editor.commit().expect("the learned shifts commit");
            });
            changed = true;
        }
        // Spec: midi-cc-sender R6. A controller the CC Type list leaves out
        // (bank select and the channel mode messages) isn't learned.
        let learned: Vec<(u8, f64)> = heard
            .iter()
            .filter(|message| message.status() & 0xF0 == 0xB0 && cc_is_listed(message.data1()))
            .map(|message| (message.data1(), f64::from(message.data2())))
            .collect();
        if !senders.is_empty() && !learned.is_empty() {
            let free: Vec<(DeviceId, Vec<u32>)> = senders
                .iter()
                .map(|device| {
                    let params = &self
                        .editor
                        .document()
                        .device(*device)
                        .expect("the device is in the set")
                        .params;
                    let free = (0..DIALS)
                        .filter(|k| params.get(&dial_cc(*k)).is_none_or(|cc| *cc == 0.0))
                        .collect();
                    (*device, free)
                })
                .collect();
            self.edit(|editor| {
                editor.begin("Learn CC Control");
                for (device, mut free) in free.clone() {
                    for &(controller, value) in &learned {
                        let Some(k) = free.first().copied() else {
                            break;
                        };
                        free.remove(0);
                        editor
                            .apply(Command::SetDeviceParam {
                                device,
                                param: dial_cc(k),
                                value: cc_type_for(controller),
                            })
                            .expect("a controller is in the CC Type list");
                        editor
                            .apply(Command::SetDeviceParam {
                                device,
                                param: dial_value(k),
                                value,
                            })
                            .expect("a controller value is in range");
                    }
                }
                editor.commit().expect("the learned controllers commit");
            });
            changed = true;
        }
        changed
    }

    /// Runs a MIDI port or computer MIDI keyboard command (WP-2.2).
    pub(super) fn run_midi_io(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let failed = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        let result = match id {
            midi_io_ids::PORTS => {
                Self::args::<NoArgs>(id, args)?;
                self.refresh_midi();
                to_value(&self.midi.report())
            }
            midi_io_ids::SET_PORT => {
                let args = Self::args::<midi_io_ids::SetPortArgs>(id, args)?;
                self.refresh_midi();
                let port = self.midi.set_port(&args).map_err(failed)?;
                // Spec: midi-clock R15, R24. With no input on Sync, EXT is
                // off.
                if !self.sync_lamps_shown() {
                    self.midi.set_external_sync(false);
                }
                self.prefs.midi = self.midi.prefs();
                self.save_preferences();
                self.send_sync();
                to_value(&port)
            }
            midi_io_ids::SET_SHOW_RESYNC_BUTTON => {
                let args = Self::args::<midi_io_ids::SetShowResyncButtonArgs>(id, args)?;
                let on = self.midi.set_show_resync_button(args.on);
                self.prefs.midi = self.midi.prefs();
                self.save_preferences();
                to_value(&midi_io_ids::SetShowResyncButtonResult { on })
            }
            midi_io_ids::TOGGLE_COMPUTER_KEYBOARD => {
                let args = Self::args::<midi_io_ids::ToggleKeyboardArgs>(id, args)?;
                self.midi.set_keyboard_on(args.on);
                self.prefs.midi = self.midi.prefs();
                self.save_preferences();
                to_value(&self.midi.keyboard_info())
            }
            midi_io_ids::COMPUTER_KEYBOARD_OCTAVE => {
                let args = Self::args::<midi_io_ids::StepArgs>(id, args)?;
                self.midi.shift_octave(args.steps);
                to_value(&self.midi.keyboard_info())
            }
            midi_io_ids::COMPUTER_KEYBOARD_VELOCITY => {
                let args = Self::args::<midi_io_ids::StepArgs>(id, args)?;
                self.midi.step_velocity(args.steps);
                to_value(&self.midi.keyboard_info())
            }
            midi_io_ids::COMPUTER_KEYBOARD_KEY => {
                let args = Self::args::<midi_io_ids::KeyArgs>(id, args)?;
                let result = self.midi.key(&args.key, args.pressed).map_err(failed)?;
                // Spec: multisampler R7. Auto Select follows the note.
                if let Some(note) = result.note {
                    let velocity = self.midi.keyboard_info().velocity;
                    self.follow_note(note, velocity, args.pressed);
                }
                to_value(&result)
            }
            _ => return Err(ShellError::UnknownCommand(id.to_owned())),
        };
        self.touch();
        Ok(result)
    }

    /// Runs a Standard MIDI File command (WP-2.21): importing a file into the
    /// Arrangement, or writing a MIDI clip out as one.
    pub(super) fn run_midi_files(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            midi_ids::IMPORT => {
                let args = Self::args::<midi_ids::MidiImportArgs>(id, args)?;
                let at = args.at.unwrap_or_else(|| self.arrangement.insert_marker());
                let path = PathBuf::from(&args.path);
                let look = &self.prefs.look;
                let result = self
                    .editor
                    .transact(crate::midi_files::UNDO_LABEL, |editor| {
                        let result = crate::midi_files::import(
                            editor,
                            &path,
                            args.track,
                            at,
                            args.import_tempo,
                        )?;
                        let clips: Vec<_> = result.clips.iter().map(|clip| clip.clip).collect();
                        crate::colours::colour_new(editor, look, &result.created_tracks, &clips)
                            .map_err(|error| {
                                crate::midi_files::MidiFileError::Failed(error.to_string())
                            })?;
                        Ok(result)
                    })
                    .map_err(|error| midi_file_error(id, error))?;
                self.dirty = true;
                self.document_changed();
                self.touch();
                Ok(to_value(&result))
            }
            midi_ids::EXPORT => {
                let args = Self::args::<midi_ids::MidiExportArgs>(id, args)?;
                if args.path.is_none() {
                    // Spec: session-context-menus R7, R16. The clip menus'
                    // Export MIDI Clip... asks where to save the file; the
                    // app then runs this again with the path.
                    let clip = self.editor.document().clip(args.clip).ok_or_else(|| {
                        ShellError::InvalidArgs {
                            command: id.to_owned(),
                            message: format!("no clip has the ID {}", args.clip.0),
                        }
                    })?;
                    let name = if clip.name.is_empty() {
                        "Clip"
                    } else {
                        &clip.name
                    };
                    self.effects.push(Effect::PickMidiExportPath {
                        clip: args.clip,
                        default_name: format!("{}.mid", bl_project::preset::file_safe(name)),
                    });
                    return Ok(Value::Null);
                }
                let result = crate::midi_files::export(self.editor.document(), &args)
                    .map_err(|error| midi_file_error(id, error))?;
                self.touch();
                Ok(to_value(&result))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }
}

//! MIDI input, MIDI learn, MIDI I/O and MIDI files.

use super::{MIDI_LAMP, Shell, ShellError, midi_file_error, to_value};
use crate::midi::MidiHost;
use bl_commands::NoArgs;
use bl_shell_spec::midi_files as midi_ids;
use bl_shell_spec::midi_io as midi_io_ids;
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
        changed
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
        self.midi = MidiHost::new(backend, sender, output, &self.prefs.midi);
    }

    /// Drives the shell's clocks from `clock`: the computer keyboard stamps
    /// its notes with it, and the next audio stream times its blocks from it
    /// (T199). Call it after [`Shell::use_midi_backend`] and before
    /// [`Shell::start_audio`].
    pub fn use_test_clock(&mut self, clock: bl_core::clock::TestClock) {
        self.midi.set_test_clock(clock.clone());
        self.audio.set_test_clock(clock);
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
        before != self.midi.ports()
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
                        .is_some_and(|on| on >= 0.5);
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
                let port = self
                    .midi
                    .set_port(
                        args.direction,
                        &args.name,
                        args.track,
                        args.sync,
                        args.remote,
                        args.mpe,
                    )
                    .map_err(failed)?;
                self.prefs.midi = self.midi.prefs();
                self.save_preferences();
                to_value(&port)
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
                let result = crate::midi_files::import(
                    &mut self.editor,
                    &path,
                    args.track,
                    at,
                    args.import_tempo,
                )
                .map_err(|error| midi_file_error(id, error))?;
                self.dirty = true;
                self.document_changed();
                self.touch();
                Ok(to_value(&result))
            }
            midi_ids::EXPORT => {
                let args = Self::args::<midi_ids::MidiExportArgs>(id, args)?;
                let result = crate::midi_files::export(self.editor.document(), &args)
                    .map_err(|error| midi_file_error(id, error))?;
                self.touch();
                Ok(to_value(&result))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }
}

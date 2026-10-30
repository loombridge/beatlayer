//! The control surfaces in the shell (WP-7.5; the control surfaces spec, R6,
//! R7, R13 to R15): the commands that set the Control Surfaces table, the
//! read model each surface gets on its tick, and the commands a surface asks
//! for, run on this thread as a person's own.
//!
//! The host is `bl-control`'s, on a thread of its own. This hands it the set
//! and what the engine published, takes what it asks for, and runs it in
//! order through [`Shell::run`], so a surface's command has the same
//! checks, undo step and refusal as the same command from the window or an
//! agent.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bl_commands::{NoArgs, Permission};
use bl_control::{
    Ask, CommandRequest, ReadModel, RowConfig, RowReport, RunState, Selection, Transport,
};
use bl_shell_spec::control_surface as ids;
use serde_json::Value;

use super::{MIDI_LAMP, Shell, ShellError, to_value};

/// How long a control with no release may rest and its gesture stay one
/// undo step (control surfaces R7).
const REST: Duration = Duration::from_millis(500);

/// The control surfaces' state in the shell.
#[derive(Default)]
pub(super) struct SurfaceHost {
    /// The document last handed over, and the revision it was.
    document: Option<(u64, Arc<bl_model::Document>)>,
    /// The control whose gesture is one undo step, its row, and when it last
    /// moved.
    gesture: Option<(usize, u32, Instant)>,
    /// Set while a surface's command runs, so it doesn't end its own
    /// gesture.
    applying: bool,
    /// The counts of messages remote control took and sent, and of keys
    /// taken, when [`Shell::poll`] last looked, and when each last changed
    /// while the lamp still lit.
    seen: (u64, u64, u64),
    lit: [Option<Instant>; 3],
    /// When each lamp last lit, for `remote.state`.
    last_lit: [Option<Instant>; 3],
    applied: u64,
    pub(super) refused: u64,
}

fn info(report: RowReport, row: usize) -> ids::RowInfo {
    let port = |name: Option<String>, present: Option<bool>| {
        name.map(|name| ids::PortRef {
            name,
            present: present.unwrap_or(false),
        })
    };
    ids::RowInfo {
        row: u8::try_from(row + 1).unwrap_or(u8::MAX),
        surface: report.config.surface.map(|id| ids::SurfaceRef {
            id,
            name: report.surface_name,
        }),
        input: port(report.config.input, report.input_present),
        output: port(report.config.output, report.output_present),
        state: match report.run {
            RunState::Empty => ids::RowRun::Empty,
            RunState::Waiting => ids::RowRun::Waiting,
            RunState::Running => ids::RowRun::Running,
            RunState::Stopped => ids::RowRun::Stopped,
        },
        error: report.error,
        can_dump: report.can_dump,
        surface_state: None,
    }
}

impl Shell {
    /// The surfaces the table's chooser offers (R1), as
    /// `control_surface.surfaces` reports them.
    #[must_use]
    pub fn control_surface_choices() -> Vec<ids::SurfaceInfo> {
        bl_control::kinds()
            .iter()
            .map(|kind| ids::SurfaceInfo {
                id: kind.id.to_owned(),
                name: kind.name.to_owned(),
                needs_output: kind.needs_output,
                can_lock: kind.can_lock,
                ring: kind.ring.map(|ring| ids::RingInfo {
                    tracks: ring.tracks,
                    scenes: ring.scenes,
                }),
                has_setup: !kind.dump.is_empty(),
            })
            .collect()
    }

    /// The Control Bar's remote indicators: the upper lamp is lit while a
    /// message a mapping or a surface took, or one taken for learning,
    /// arrived a moment ago, the lower while feedback or a surface's message
    /// left (remote mapping R41; control surfaces R15; L-CTRL-026).
    #[must_use]
    pub fn remote_indicators(&self) -> (bool, bool) {
        (self.surface.lit[0].is_some(), self.surface.lit[1].is_some())
    }

    /// Each remote lamp as `remote.state` reports it: the remote pair's two
    /// and the key indicators' upper one.
    pub(super) fn remote_lamps(&self) -> [bl_shell_spec::remote::Lamp; 3] {
        let now = Instant::now();
        std::array::from_fn(|n| bl_shell_spec::remote::Lamp {
            lit: self.surface.lit[n].is_some(),
            last_lit_ms_ago: self.surface.last_lit[n]
                .map(|at| now.saturating_duration_since(at).as_millis() as u64),
        })
    }

    /// The key indicators beside the Key switch: the upper lamp is lit while
    /// a key a key mapping took was pressed a moment ago; the lower stays
    /// dark (remote mapping R41; L-CTRL-021).
    #[must_use]
    pub fn key_indicators(&self) -> (bool, bool) {
        (self.surface.lit[2].is_some(), false)
    }

    /// The six rows of the Control Surfaces table as `control_surface.list`
    /// reports them, for the Settings window to draw.
    #[must_use]
    pub fn control_surface_rows(&self) -> Vec<ids::RowInfo> {
        (0..bl_control::ROWS)
            .map(|row| self.row_info(row))
            .collect()
    }

    /// One row of the table as `control_surface.list` reports it, with the
    /// state a running surface holds: its ring, the device it reaches, its
    /// own mode (R13, R35).
    pub(super) fn row_info(&self, row: usize) -> ids::RowInfo {
        let report = self.midi.surface_rows().swap_remove(row);
        let mode = report.state.as_ref().and_then(|state| state.mode.clone());
        let running = report.run == RunState::Running;
        let mut info = info(report, row);
        if running {
            info.surface_state = Some(ids::SurfaceStateInfo {
                ring: self.ring_state(row),
                device: self.device_focus(row),
                mode,
            });
        }
        info
    }

    /// How many commands a surface asked for ran, and how many were
    /// refused.
    #[must_use]
    pub fn surface_counts(&self) -> (u64, u64) {
        (self.surface.applied, self.surface.refused)
    }

    /// Hands the surfaces the read model, runs the commands they asked for
    /// in the order they asked, and ends a gesture that has rested 500 ms
    /// (R6, R7). Returns whether the set changed.
    pub(super) fn poll_surfaces(&mut self) -> bool {
        let revision = self.document_revision;
        self.sync_instant();
        if self.midi.surfaces().running() {
            let model = self.surface_model();
            self.midi.surfaces().publish(model);
        }
        let mut requests = Vec::new();
        self.midi.surfaces().take_requests(&mut requests);
        for request in requests {
            match request.ask {
                Ask::Command(command) => {
                    self.run_surface_request(CommandRequest {
                        row: request.row,
                        ask: Ask::Command(command),
                    });
                }
                Ask::Control(control) => self.run_control(request.row, control),
            }
        }
        if let Some((_, _, at)) = self.surface.gesture
            && self.midi.now().saturating_duration_since(at) >= REST
        {
            self.end_surface_gesture();
        }
        self.light_remote_indicators() | (revision != self.document_revision)
    }

    /// What the host gives each surface on its tick (R6).
    fn surface_model(&mut self) -> ReadModel {
        let document = match &self.surface.document {
            Some((revision, document)) if *revision == self.document_revision => {
                Arc::clone(document)
            }
            _ => {
                let document = Arc::new(self.editor.document().clone());
                self.surface.document = Some((self.document_revision, Arc::clone(&document)));
                document
            }
        };
        let view = self.arrangement.transport();
        let session = self.session.state();
        ReadModel {
            document,
            slots: self.session_states().to_vec(),
            transport: Transport {
                playing: view.playing,
                position: view.position,
                loop_on: view.loop_on,
                loop_start: view.loop_start,
                loop_length: view.loop_length,
            },
            tempo: self.current_tempo(),
            meters: self.meters().to_vec(),
            selection: Selection {
                tracks: self.arrangement.selected_tracks().to_vec(),
                slots: session.slots.iter().map(|s| (s.track, s.scene)).collect(),
                scenes: session.scenes,
            },
            rows: self.row_views(),
        }
    }

    /// Lights the remote indicators for a moment when surfaces took or sent
    /// messages since the last call; returns whether either changed (R15).
    fn light_remote_indicators(&mut self) -> bool {
        let now = Instant::now();
        let (surface_in, surface_out) = self.midi.surfaces().traffic();
        let (sent, keys) = self.remote_traffic();
        let taken = self.midi.sender().stats().snapshot().remote_taken;
        let seen = (surface_in + taken, surface_out + sent, keys);
        let counts = [
            (seen.0, self.surface.seen.0),
            (seen.1, self.surface.seen.1),
            (seen.2, self.surface.seen.2),
        ];
        let mut changed = false;
        for ((lit, last), (new, old)) in self
            .surface
            .lit
            .iter_mut()
            .zip(&mut self.surface.last_lit)
            .zip(counts)
        {
            if new != old {
                *lit = Some(now);
                *last = Some(now);
                changed = true;
            } else if lit.is_some_and(|since| now.duration_since(since) >= MIDI_LAMP) {
                *lit = None;
                changed = true;
            }
        }
        self.surface.seen = seen;
        changed
    }

    /// Runs one command a surface asked for, inside its gesture (R7);
    /// whether it ran.
    pub(super) fn run_surface_request(&mut self, request: CommandRequest) -> bool {
        let CommandRequest { row, ask } = request;
        let Ask::Command(request) = ask else {
            return false;
        };
        match request.gesture {
            Some(gesture) => {
                let mut held = self
                    .surface
                    .gesture
                    .is_some_and(|(r, c, _)| r == row && c == gesture.control);
                if !held {
                    self.end_surface_gesture();
                    // A drag of the person's is under way: the command joins
                    // it, and ending it is not the surface's to do.
                    if !self.gesture {
                        // The step is named as the window names the
                        // command's.
                        let label = self
                            .registry
                            .get(&request.id)
                            .map_or_else(|| request.id.clone(), |spec| spec.label.to_string());
                        self.begin_gesture(&label);
                        held = true;
                    }
                }
                if held {
                    self.surface.gesture = Some((row, gesture.control, self.midi.now()));
                }
            }
            None => self.end_surface_gesture(),
        }
        self.surface.applying = true;
        let result = self.run(&request.id, request.args);
        self.surface.applying = false;
        let ran = result.is_ok();
        if ran {
            self.surface.applied += 1;
        } else {
            self.surface.refused += 1;
        }
        if request.gesture.is_some_and(|gesture| gesture.released) {
            self.end_surface_gesture();
        }
        ran
    }

    /// Ends the gesture that is one undo step, if one is open.
    pub(super) fn end_surface_gesture(&mut self) {
        if self.surface.gesture.take().is_some() {
            self.end_gesture();
        }
    }

    /// Ends the gesture for any command but the read ones and a surface's
    /// own (R7: any other edit ends it).
    pub(super) fn surface_gesture_ends_for(&mut self, id: &str) {
        let reads = self
            .registry
            .get(id)
            .is_some_and(|spec| spec.permission == Some(Permission::Read));
        if self.surface.gesture.is_some() && !self.surface.applying && !reads {
            self.end_surface_gesture();
        }
    }

    /// Runs a control surface command (R13, R14).
    pub(super) fn run_control_surface(
        &mut self,
        id: &str,
        args: Value,
    ) -> Result<Value, ShellError> {
        let failed = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        let row_of = |row: u8| -> Result<usize, ShellError> {
            (1..=bl_control::ROWS)
                .contains(&usize::from(row))
                .then(|| usize::from(row) - 1)
                .ok_or_else(|| ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: format!(
                        "the row is {row}; the table has rows 1 to {}",
                        bl_control::ROWS
                    ),
                })
        };
        match id {
            ids::LIST => {
                Self::args::<NoArgs>(id, args)?;
                self.refresh_midi();
                self.sync_instant();
                Ok(to_value(&ids::ListResult {
                    rows: self.control_surface_rows(),
                }))
            }
            ids::SET => {
                let args = Self::args::<ids::SetArgs>(id, args)?;
                let row = row_of(args.row)?;
                self.refresh_midi();
                let now = self.midi.surfaces().rows()[row].clone();
                let config = RowConfig {
                    surface: args.surface.unwrap_or(now.surface),
                    input: args.input.unwrap_or(now.input),
                    output: args.output.unwrap_or(now.output),
                };
                self.midi.set_surface_row(row, config).map_err(failed)?;
                self.sync_instant();
                self.prefs.midi = self.midi.prefs();
                self.save_preferences();
                self.touch();
                Ok(to_value(&self.row_info(row)))
            }
            ids::DUMP => {
                let args = Self::args::<ids::DumpArgs>(id, args)?;
                let row = row_of(args.row)?;
                self.refresh_midi();
                let sent = self.midi.dump_surface(row).map_err(failed)?;
                Ok(to_value(&ids::DumpResult { sent }))
            }
            ids::SURFACES => {
                Self::args::<NoArgs>(id, args)?;
                Ok(to_value(&ids::SurfacesResult {
                    surfaces: Self::control_surface_choices(),
                }))
            }
            ids::LOCK | ids::UNLOCK | ids::MOVE_RING | ids::SET_BANK | ids::BANKS => {
                self.run_instant(id, args)
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }
}

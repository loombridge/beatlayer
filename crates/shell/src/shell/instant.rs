//! Instant mappings in the shell (WP-7.6; the control surfaces spec, R17 to
//! R38): the device a surface's device controls follow, its banks, the lock
//! to a device, the session ring and the controls that reach them.
//!
//! All of it is surface state (R12): kept here, per row of the Control
//! Surfaces table, for as long as the row's surface runs, never saved with
//! the set and never an undo step. A surface only says which control moved
//! (`bl_control::Role`); what the role reaches is worked out here as the
//! message is run, so it is what the person sees, and the value laws are
//! the remote mappings' own ([`Shell::apply_hit`]'s).

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use bl_control::{
    Ask, CommandRequest, Control, Focus, Gesture, Hit, Kind, Lock, Memory, Moves, ParamView, ROWS,
    Request, RingStep, RingView, Role, RowView, RunState,
};
use bl_model::{
    ControlKind, Device, DeviceId, DeviceKind, DeviceRole, Document, MACROS, MixerParam, ParamId,
    RemoteMapping, RemoteSource, RemoteTarget, SHOWN_MACROS, SceneId, TrackId, TrackKind,
    macro_number, macro_param, numbered,
};
use bl_shell_spec::control_surface as ids;
use bl_shell_spec::control_surface::{
    BankInfo, BankParam, BanksResult, DeviceFocus, DeviceLock, LockKind, RingBox, RingMoves,
    RingState,
};
use serde_json::{Value, json};

use super::{Shell, ShellError, to_value};

/// What a row's takeover remembers for a role: the parameter it reached,
/// and where the controller and the control were (remote mapping R29).
struct Held {
    target: RemoteTarget,
    memory: Memory,
}

/// Where a ring is: its first track and scene by ID, and the places they
/// last had, which stand in for a track or scene that is gone (R30).
#[derive(Clone, Copy)]
struct Place {
    track: Option<TrackId>,
    scene: Option<SceneId>,
    track_at: usize,
    scene_at: usize,
}

/// What one row holds while its surface runs.
#[derive(Default)]
struct Row {
    place: Option<Place>,
    /// The device locked by hand (R23).
    hand: Option<DeviceId>,
    /// The device arming locked (R26).
    arm: Option<DeviceId>,
    /// The bank last shown of each device (R18).
    banks: HashMap<DeviceId, usize>,
    held: HashMap<Role, Held>,
    /// The device and bank the device controls reached when a control last
    /// moved.
    reached: Option<(DeviceId, usize)>,
}

/// The instant mappings' state in the shell.
#[derive(Default)]
pub(super) struct Instant {
    rows: [Row; ROWS],
    /// The armed MIDI tracks, the one armed first at the front (R26).
    armed: Vec<TrackId>,
    /// A new set is open: every ring starts again (R30).
    fresh: bool,
    /// How many times each ring was moved, so the Session View scrolls to
    /// show it when it changes (session view R37).
    moved: [u64; ROWS],
}

/// A device's banks: a name and the parameters it takes.
type Banks = Vec<(String, Vec<ParamId>)>;

/// The device a row's device controls reach, and where it is on its banks.
struct Reach {
    device: DeviceId,
    lock: Option<Lock>,
    bank: usize,
    banks: Banks,
}

/// The tracks a ring runs along: the Session View's columns, a folded
/// group's tracks left out, then the returns (R28).
fn ring_tracks(shell: &Shell) -> Vec<TrackId> {
    let doc = shell.editor.document();
    let mut tracks = shell.mixer_columns();
    tracks.extend(doc.returns());
    tracks
}

/// The first instrument or instrument rack at the top of a track's chain
/// (R26).
fn instrument_of(doc: &Document, track: TrackId) -> Option<DeviceId> {
    doc.track(track)?
        .devices()
        .find(|d| d.role == DeviceRole::Instrument)
        .map(|d| d.id)
}

/// `at` kept where a ring can start: its last column holds the last track
/// at the furthest (R29).
const fn furthest(count: usize, size: usize) -> usize {
    count.saturating_sub(size)
}

/// A stable number for a role, which names the gesture of a control (R7).
fn role_id(role: Role) -> u32 {
    let mut hasher = DefaultHasher::new();
    role.hash(&mut hasher);
    // A surface's own controls number from 0 up; these sit above.
    0x8000_0000 | (hasher.finish() as u32 >> 1)
}

impl Shell {
    /// The kind of surface each row runs, or `None` for a row that doesn't
    /// run one.
    fn running_kinds(&self) -> [Option<&'static Kind>; ROWS] {
        let mut kinds = [None; ROWS];
        for (row, report) in self.midi.surface_rows().into_iter().enumerate() {
            if report.run == RunState::Running {
                kinds[row] = report.config.surface.as_deref().and_then(bl_control::kind);
            }
        }
        kinds
    }

    /// Starts every ring again, as a newly opened set does (R9, R30).
    pub(super) fn instant_reset(&mut self) {
        self.instant.fresh = true;
    }

    /// Brings the rows' state in line with the set and with which surfaces
    /// run: locks end with their devices, arming locks, the rings keep
    /// their places by ID (R24, R26, R30). Runs after every change to the
    /// set.
    pub(super) fn sync_instant(&mut self) {
        let kinds = self.running_kinds();
        let tracks = ring_tracks(self);
        let doc = self.editor.document();
        let scenes: Vec<SceneId> = doc.scenes().map(|s| s.id).collect();
        let armed_now: Vec<TrackId> = doc
            .tracks()
            .filter(|t| t.kind() == TrackKind::Midi && t.mixer().arm)
            .map(bl_model::Track::id)
            .collect();
        let fresh = std::mem::take(&mut self.instant.fresh);
        let (mut newest, mut disarmed) = (None, false);
        if fresh {
            self.instant.armed = armed_now;
        } else {
            let before = self.instant.armed.len();
            self.instant.armed.retain(|t| armed_now.contains(t));
            disarmed = self.instant.armed.len() != before;
            for track in armed_now {
                if !self.instant.armed.contains(&track) {
                    self.instant.armed.push(track);
                    newest = Some(track);
                }
            }
        }
        let fallback = self
            .instant
            .armed
            .iter()
            .rev()
            .find_map(|track| instrument_of(doc, *track));
        for (row, kind) in kinds.iter().enumerate() {
            let state = &mut self.instant.rows[row];
            let Some(kind) = kind else {
                *state = Row::default();
                continue;
            };
            if fresh {
                *state = Row::default();
            }
            let exists = |d: &DeviceId| doc.device(*d).is_some();
            state.hand = state.hand.filter(exists);
            let had = state.arm.is_some();
            state.arm = state.arm.filter(exists);
            if kind.can_lock && state.hand.is_none() {
                if let Some(track) = newest {
                    state.arm = instrument_of(doc, track);
                } else if disarmed || (had && state.arm.is_none()) {
                    state.arm = fallback;
                }
            }
            state.banks.retain(|d, _| doc.device(*d).is_some());
            state.held.retain(|_, held| match held.target {
                RemoteTarget::Device { device, .. } => doc.device(device).is_some(),
                _ => true,
            });
            if kind.ring.is_none() {
                state.place = None;
                continue;
            }
            let place = state.place.get_or_insert(Place {
                track: tracks.first().copied(),
                scene: scenes.first().copied(),
                track_at: 0,
                scene_at: 0,
            });
            // Spec: control-surfaces R30. A track or scene the set still has
            // keeps the ring by ID; one that is gone leaves it at the place
            // it had, on whatever is there now, or the last.
            place.track_at = match place.track.and_then(|t| tracks.iter().position(|x| *x == t)) {
                Some(at) => at,
                None => {
                    let hidden = place.track.and_then(|t| {
                        // Folded away: the ring goes to the group that
                        // holds it.
                        let mut up = doc.parent(t);
                        while let Some(group) = up {
                            if let Some(at) = tracks.iter().position(|x| *x == group) {
                                return Some(at);
                            }
                            up = doc.parent(group);
                        }
                        None
                    });
                    hidden.unwrap_or_else(|| place.track_at.min(tracks.len().saturating_sub(1)))
                }
            };
            place.track = tracks.get(place.track_at).copied();
            place.scene_at = match place.scene.and_then(|s| scenes.iter().position(|x| *x == s)) {
                Some(at) => at,
                None => place.scene_at.min(scenes.len().saturating_sub(1)),
            };
            place.scene = scenes.get(place.scene_at).copied();
        }
    }

    /// Starts takeover afresh for every device control of a row whose
    /// device or bank has changed since a control last moved: they reach
    /// other parameters now (R22).
    fn track_reach(&mut self) {
        for row in 0..ROWS {
            let now = self.reach(row).map(|r| (r.device, r.bank));
            let state = &mut self.instant.rows[row];
            if state.reached != now {
                state.reached = now;
                state.held.retain(|role, _| !matches!(role, Role::Device(_)));
            }
        }
    }

    /// A row's ring as `control_surface.list` reports it.
    pub(super) fn ring_state(&self, row: usize) -> Option<RingState> {
        let size = self.running_kinds()[row]?.ring?;
        let place = self.instant.rows[row].place?;
        let (tracks, scenes) = (ring_tracks(self).len(), self.editor.document().scenes().len());
        let (width, height) = (usize::from(size.tracks), usize::from(size.scenes));
        Some(RingState {
            tracks: size.tracks,
            scenes: size.scenes,
            first_track: place.track,
            first_scene: place.scene,
            track_index: place.track_at,
            scene_index: place.scene_at,
            can_move: RingMoves {
                left: place.track_at > 0,
                right: place.track_at < furthest(tracks, width),
                up: height > 0 && place.scene_at > 0,
                down: height > 0 && place.scene_at < furthest(scenes, height),
            },
        })
    }

    /// The ring's tracks and scenes: the ones its block covers.
    fn ring_cover(&self, row: usize) -> Option<(Vec<TrackId>, Vec<SceneId>)> {
        let size = self.running_kinds()[row]?.ring?;
        let place = self.instant.rows[row].place?;
        let tracks = ring_tracks(self);
        let scenes: Vec<SceneId> = self.editor.document().scenes().map(|s| s.id).collect();
        fn cover<T: Copy>(all: &[T], at: usize, size: u8) -> Vec<T> {
            all.iter().skip(at).take(usize::from(size)).copied().collect()
        }
        Some((
            cover(&tracks, place.track_at, size.tracks),
            cover(&scenes, place.scene_at, size.scenes),
        ))
    }

    /// Moves a row's ring to a first track and scene by ID, or by a step
    /// (R29).
    fn move_ring(
        &mut self,
        row: usize,
        track: Option<TrackId>,
        scene: Option<SceneId>,
        step: Option<ids::RingStep>,
    ) -> Result<(), String> {
        let Some(size) = self.running_kinds()[row].and_then(|k| k.ring) else {
            return Err("the row's surface has no session ring".to_owned());
        };
        let tracks = ring_tracks(self);
        let scenes: Vec<SceneId> = self.editor.document().scenes().map(|s| s.id).collect();
        let (width, height) = (usize::from(size.tracks), usize::from(size.scenes));
        let Some(place) = self.instant.rows[row].place else {
            return Err("the row's ring isn't there yet".to_owned());
        };
        let (mut track_at, mut scene_at) = (place.track_at, place.scene_at);
        if let Some(track) = track {
            track_at = tracks
                .iter()
                .position(|t| *t == track)
                .ok_or("that track isn't one the ring runs along")?;
        }
        if let Some(scene) = scene {
            scene_at = scenes
                .iter()
                .position(|s| *s == scene)
                .ok_or("the set has no such scene")?;
        }
        let (across, down) = (isize::try_from(width).unwrap_or(0), isize::try_from(height).unwrap_or(0));
        let (dx, dy) = match step {
            None => (0, 0),
            Some(ids::RingStep::Left) => (-1, 0),
            Some(ids::RingStep::Right) => (1, 0),
            Some(ids::RingStep::Up) => (0, -1),
            Some(ids::RingStep::Down) => (0, 1),
            Some(ids::RingStep::PageLeft) => (-across, 0),
            Some(ids::RingStep::PageRight) => (across, 0),
            Some(ids::RingStep::PageUp) => (0, -down),
            Some(ids::RingStep::PageDown) => (0, down),
        };
        let shift = |at: usize, by: isize| at.saturating_add_signed(by);
        // A ring with no scenes has nothing to move up or down.
        track_at = shift(track_at, dx).min(furthest(tracks.len(), width));
        scene_at = if height == 0 {
            0
        } else {
            shift(scene_at, dy).min(furthest(scenes.len(), height))
        };
        self.instant.rows[row].place = Some(Place {
            track: tracks.get(track_at).copied(),
            scene: scenes.get(scene_at).copied(),
            track_at,
            scene_at,
        });
        self.instant.moved[row] += 1;
        Ok(())
    }

    /// How many times each ring has been moved by a surface or a command:
    /// the Session View scrolls to show a ring when its count changes
    /// (session view R37).
    #[must_use]
    pub fn ring_moves(&self) -> [u64; ROWS] {
        self.instant.moved
    }

    /// The device a row's device controls reach: the locked one, or the
    /// device in focus (R17, R23).
    fn reach(&self, row: usize) -> Option<Reach> {
        self.running_kinds()[row]?;
        let doc = self.editor.document();
        let state = &self.instant.rows[row];
        let (device, lock) = if let Some(device) = state.hand {
            (device, Some(Lock::Hand))
        } else if let Some(device) = state.arm {
            (device, Some(Lock::Arming))
        } else {
            let shown = crate::devices::shown_track(&self.arrangement, doc)?;
            let selected = self
                .devices
                .selected()
                .filter(|d| doc.device_track(*d) == Some(shown));
            let first = || doc.track(shown)?.devices().next().map(|d| d.id);
            (selected.or_else(first)?, None)
        };
        let banks = self.banks_of(doc.device(device)?);
        let bank = state
            .banks
            .get(&device)
            .copied()
            .unwrap_or(0)
            .min(banks.len().saturating_sub(1));
        Some(Reach {
            device,
            lock,
            bank,
            banks,
        })
    }

    /// A device's banks as a surface offers them (R19 to R21).
    fn banks_of(&self, device: &Device) -> Banks {
        if matches!(device.kind, DeviceKind::Rack { .. }) {
            let shown = self
                .devices
                .views_of(device.id)
                .macro_count
                .clamp(SHOWN_MACROS, MACROS);
            let macros = |from: u8, to: u8| (from..=to).map(macro_param).collect();
            let mut banks = vec![("Macros 1–8".to_owned(), macros(1, shown.min(8)))];
            if shown > 8 {
                banks.push(("Macros 9–16".to_owned(), macros(9, shown)));
            }
            return banks;
        }
        let Some(description) = self.devices.description(&device.kind) else {
            return Vec::new();
        };
        if matches!(device.kind, DeviceKind::Plugin { .. }) {
            // Spec: control-surfaces R21. The configured list in eights, or
            // every parameter when it is empty.
            let mut list = self.devices.plugin_configured(device, description);
            if list.is_empty() {
                list = description.params.iter().map(|p| p.id).collect();
            }
            return numbered(&list)
                .into_iter()
                .map(|b| (b.name, b.params))
                .collect();
        }
        description
            .banks()
            .into_iter()
            .map(|b| (b.name, b.params))
            .collect()
    }

    /// One parameter as a surface reads it.
    fn param_view(&self, device: &Device, param: ParamId) -> Option<ParamView> {
        let doc = self.editor.document();
        let description = self.devices.description(&device.kind)?;
        let descriptor = description.param(param)?;
        let value = description.value(&device.params, param)?;
        let name = match macro_number(param) {
            Some(number) if matches!(device.kind, DeviceKind::Rack { .. }) => {
                self.devices.macro_name(doc, device, number)
            }
            _ => descriptor.name.clone(),
        };
        Some(ParamView {
            param,
            name,
            value,
            text: descriptor.format(value),
            held: self.devices.macro_holding(doc, device.id, param).is_some(),
        })
    }

    /// What each row holds, for the surfaces' read model (R6).
    pub(super) fn row_views(&self) -> Vec<RowView> {
        let kinds = self.running_kinds();
        let doc = self.editor.document();
        (0..ROWS)
            .map(|row| {
                let Some(kind) = kinds[row] else {
                    return RowView::default();
                };
                let ring = kind.ring.and_then(|size| {
                    let place = self.instant.rows[row].place?;
                    let (tracks, scenes) = self.ring_cover(row)?;
                    let state = self.ring_state(row)?;
                    Some(RingView {
                        size,
                        first_track: place.track,
                        first_scene: place.scene,
                        tracks,
                        scenes,
                        moves: Moves {
                            left: state.can_move.left,
                            right: state.can_move.right,
                            up: state.can_move.up,
                            down: state.can_move.down,
                        },
                    })
                });
                let focus = self.reach(row).and_then(|reach| {
                    let device = doc.device(reach.device)?;
                    let params = reach.banks.get(reach.bank).map_or_else(Vec::new, |(_, ids)| {
                        ids.iter()
                            .filter_map(|id| self.param_view(device, *id))
                            .collect()
                    });
                    Some(Focus {
                        device: reach.device,
                        lock: reach.lock,
                        bank: reach.bank,
                        banks: reach.banks.iter().map(|(name, _)| name.clone()).collect(),
                        params,
                    })
                });
                RowView { ring, focus }
            })
            .collect()
    }

    /// The device a row's surface is on, as `control_surface.list` reports
    /// it (R35).
    pub(super) fn device_focus(&self, row: usize) -> Option<DeviceFocus> {
        let reach = self.reach(row)?;
        Some(DeviceFocus {
            device: reach.device,
            locked: reach.lock.map(|lock| match lock {
                Lock::Hand => LockKind::Hand,
                Lock::Arming => LockKind::Arming,
            }),
            bank: reach.bank,
            bank_name: reach
                .banks
                .get(reach.bank)
                .map(|(name, _)| name.clone())
                .unwrap_or_default(),
            banks: reach.banks.into_iter().map(|(name, _)| name).collect(),
        })
    }

    /// The surfaces locked to `device`, in the table's row order (R25).
    pub(super) fn device_locks(&self, device: DeviceId) -> Vec<DeviceLock> {
        let names = self.midi.surface_rows();
        self.instant
            .rows
            .iter()
            .enumerate()
            .filter_map(|(row, state)| {
                let by = if state.hand == Some(device) {
                    LockKind::Hand
                } else if state.arm == Some(device) {
                    LockKind::Arming
                } else {
                    return None;
                };
                Some(DeviceLock {
                    row: u8::try_from(row + 1).ok()?,
                    surface: names.get(row)?.surface_name.clone()?,
                    by,
                })
            })
            .collect()
    }

    /// The running surfaces' rings, by track and scene ID, for
    /// `session.state` (session view R38).
    pub(super) fn ring_boxes(&self) -> Vec<RingBox> {
        let names = self.midi.surface_rows();
        (0..ROWS)
            .filter_map(|row| {
                let (tracks, scenes) = self.ring_cover(row)?;
                Some(RingBox {
                    row: u8::try_from(row + 1).ok()?,
                    surface: names.get(row)?.surface_name.clone()?,
                    tracks,
                    scenes,
                })
            })
            .collect()
    }

    /// Steps a row's surface to the next or previous bank of its device
    /// (R18): it stops at the first and the last.
    fn step_bank(&mut self, row: usize, forward: bool) {
        let Some(reach) = self.reach(row) else {
            return;
        };
        let next = if forward {
            (reach.bank + 1).min(reach.banks.len().saturating_sub(1))
        } else {
            reach.bank.saturating_sub(1)
        };
        self.instant.rows[row].banks.insert(reach.device, next);
    }

    /// Runs a control a surface said moved (R22, R33): the place its role
    /// reaches now, the value laws, and the command that sets it.
    pub(super) fn run_control(&mut self, row: usize, control: Control) {
        self.track_reach();
        let pressed = matches!(control.hit, Hit::NoteOn { .. });
        match control.role {
            Role::NextBank => {
                if pressed {
                    self.step_bank(row, true);
                }
            }
            Role::PreviousBank => {
                if pressed {
                    self.step_bank(row, false);
                }
            }
            Role::Ring(step) => {
                if pressed {
                    let step = match step {
                        RingStep::Left => ids::RingStep::Left,
                        RingStep::Right => ids::RingStep::Right,
                        RingStep::Up => ids::RingStep::Up,
                        RingStep::Down => ids::RingStep::Down,
                        RingStep::PageLeft => ids::RingStep::PageLeft,
                        RingStep::PageRight => ids::RingStep::PageRight,
                        RingStep::PageUp => ids::RingStep::PageUp,
                        RingStep::PageDown => ids::RingStep::PageDown,
                    };
                    let _ = self.move_ring(row, None, None, Some(step));
                }
            }
            Role::Select(column) => {
                if pressed && let Some(track) = self.ring_track(row, column) {
                    let request = Request {
                        id: "session.select".to_owned(),
                        args: json!({ "track": track }),
                        gesture: None,
                    };
                    self.run_surface_request(CommandRequest {
                        row,
                        ask: Ask::Command(request),
                    });
                }
            }
            role => match self.target_of(row, role) {
                Some(target) => self.drive(row, role, target, control),
                None => {}
            },
        }
    }

    /// The track at a column of a row's ring; `None` past the last (R33).
    fn ring_track(&self, row: usize, column: u8) -> Option<TrackId> {
        self.ring_cover(row)?.0.get(usize::from(column)).copied()
    }

    /// The control a role reaches now, if there is one: a column past the
    /// last track, a row past the last scene and a device control past the
    /// bank do nothing (R32, R33).
    fn target_of(&mut self, row: usize, role: Role) -> Option<RemoteTarget> {
        let track = |s: &Self, column| s.ring_track(row, column);
        let doc = self.editor.document();
        Some(match role {
            Role::Device(n) => {
                let reach = self.reach(row)?;
                let param = *reach.banks.get(reach.bank)?.1.get(usize::from(n))?;
                // Spec: control-surfaces R22. A parameter a macro holds
                // isn't moved.
                if self.devices.macro_holding(doc, reach.device, param).is_some() {
                    self.surface.refused += 1;
                    return None;
                }
                RemoteTarget::Device {
                    device: reach.device,
                    param,
                }
            }
            Role::Slot { column, row: r } => RemoteTarget::Slot {
                track: track(self, column)?,
                scene: *self.ring_cover(row)?.1.get(usize::from(r))?,
            },
            Role::SceneLaunch(r) => RemoteTarget::SceneLaunch {
                scene: *self.ring_cover(row)?.1.get(usize::from(r))?,
            },
            Role::StopAll => RemoteTarget::StopAll,
            Role::Stop(c) => RemoteTarget::TrackStop {
                track: track(self, c)?,
            },
            Role::Volume(c) => RemoteTarget::Mixer {
                track: track(self, c)?,
                param: MixerParam::Volume,
            },
            Role::Pan(c) => RemoteTarget::Mixer {
                track: track(self, c)?,
                param: MixerParam::Pan,
            },
            Role::Send { column, send } => RemoteTarget::Send {
                track: track(self, column)?,
                to: doc.returns().nth(usize::from(send))?,
            },
            Role::Mute(c) => RemoteTarget::Mixer {
                track: track(self, c)?,
                param: MixerParam::Mute,
            },
            Role::Solo(c) => RemoteTarget::Solo {
                track: track(self, c)?,
            },
            Role::Arm(c) => RemoteTarget::Arm {
                track: track(self, c)?,
            },
            Role::Crossfade(c) => RemoteTarget::CrossfadeAssign {
                track: track(self, c)?,
            },
            Role::NextBank | Role::PreviousBank | Role::Ring(_) | Role::Select(_) => return None,
        })
    }

    /// Applies a control's message to the control `target` its role reaches,
    /// by the value laws and Takeover Mode as a mapping does, and runs the
    /// command that sets it (R22).
    fn drive(&mut self, row: usize, role: Role, target: RemoteTarget, control: Control) {
        let (kind, range) = match target.natural_kind() {
            Some(kind) => (kind, target.natural_range()),
            None => match self.param_control(&target) {
                Some(found) => found,
                None => {
                    self.surface.refused += 1;
                    return;
                }
            },
        };
        let source = RemoteSource::Cc {
            channel: 1,
            controller: 0,
        };
        let Ok(mapping) = RemoteMapping::new(source, target, kind, range, Some(control.mode))
        else {
            self.surface.refused += 1;
            return;
        };
        let (min, max) = self.law_range(&mapping);
        let law = bl_control::Law {
            takeover: self.prefs.takeover_mode,
            step: self.step_of(&target),
            ..bl_control::Law::of(&mapping, min, max)
        };
        let at = self.value_of(&mapping);
        // Spec: control-surfaces R22. Takeover starts afresh whenever the
        // parameter a control reaches changes.
        let mut memory = match self.instant.rows[row].held.get(&role) {
            Some(held) if held.target == target => held.memory,
            _ => Memory::default(),
        };
        let action = bl_control::act(&law, control.hit, 0, at, &mut memory);
        self.instant.rows[row]
            .held
            .insert(role, Held { target, memory });
        let Some(action) = action else {
            return;
        };
        if action == bl_control::Action::Release && kind == ControlKind::Button {
            return;
        }
        let Some(run) = self.command_for(&mapping, action) else {
            self.surface.refused += 1;
            return;
        };
        let request = Request {
            id: run.id.to_owned(),
            args: run.args,
            gesture: run.gesture.map(|_| Gesture {
                control: role_id(role),
                released: false,
            }),
        };
        let ran = self.run_surface_request(CommandRequest {
            row,
            ask: Ask::Command(request),
        });
        // Spec: remote-mapping R29. Where a stepped control really is after
        // the command is where the control was left.
        if ran
            && let bl_control::Action::Position(_) = action
            && let Some(now) = self.value_of(&mapping)
            && let Some(held) = self.instant.rows[row].held.get_mut(&role)
        {
            held.memory.gave(now);
        }
    }

    /// The running row `row` as the commands take it: from 1 to 6.
    fn running_row(&self, id: &str, row: u8) -> Result<usize, ShellError> {
        let index = usize::from(row)
            .checked_sub(1)
            .filter(|i| *i < ROWS)
            .ok_or_else(|| ShellError::InvalidArgs {
                command: id.to_owned(),
                message: format!("the row is {row}; the table has rows 1 to {ROWS}"),
            })?;
        if self.running_kinds()[index].is_none() {
            return Err(ShellError::Failed {
                command: id.to_owned(),
                message: format!("row {row} has no running surface"),
            });
        }
        Ok(index)
    }

    /// Runs one of the instant mappings' commands (R23, R29, R36, R37).
    pub(super) fn run_instant(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        self.sync_instant();
        let failed = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        let row_info = |shell: &Self, row: usize| {
            to_value(&shell.row_info(row))
        };
        match id {
            ids::LOCK => {
                let args = Self::args::<ids::LockArgs>(id, args)?;
                let row = self.running_row(id, args.row)?;
                if !self.running_kinds()[row].is_some_and(|k| k.can_lock) {
                    return Err(failed(
                        "that surface can't be locked to a device".to_owned(),
                    ));
                }
                if self.editor.document().device(args.device).is_none() {
                    return Err(failed("the set has no such device".to_owned()));
                }
                let state = &mut self.instant.rows[row];
                // Locking the locked device again gives it back (R23).
                if state.hand == Some(args.device) {
                    state.hand = None;
                    state.arm = None;
                } else {
                    state.hand = Some(args.device);
                }
                Ok(row_info(self, row))
            }
            ids::UNLOCK => {
                let args = Self::args::<ids::UnlockArgs>(id, args)?;
                let row = self.running_row(id, args.row)?;
                let state = &mut self.instant.rows[row];
                state.hand = None;
                state.arm = None;
                Ok(row_info(self, row))
            }
            ids::MOVE_RING => {
                let args = Self::args::<ids::MoveRingArgs>(id, args)?;
                let row = self.running_row(id, args.row)?;
                self.move_ring(row, args.track, args.scene, args.step)
                    .map_err(failed)?;
                Ok(row_info(self, row))
            }
            ids::SET_BANK => {
                let args = Self::args::<ids::SetBankArgs>(id, args)?;
                let row = self.running_row(id, args.row)?;
                let reach = self
                    .reach(row)
                    .ok_or_else(|| failed("the surface's device controls reach no device".to_owned()))?;
                if args.bank >= reach.banks.len() {
                    return Err(failed(format!(
                        "the device has {} banks, so {} is out of range",
                        reach.banks.len(),
                        args.bank
                    )));
                }
                self.instant.rows[row].banks.insert(reach.device, args.bank);
                Ok(row_info(self, row))
            }
            ids::BANKS => {
                let args = Self::args::<ids::BanksArgs>(id, args)?;
                let device = self
                    .editor
                    .document()
                    .device(args.device)
                    .ok_or_else(|| failed("the set has no such device".to_owned()))?;
                let banks = self
                    .banks_of(device)
                    .into_iter()
                    .map(|(name, params)| BankInfo {
                        name,
                        params: params
                            .into_iter()
                            .filter_map(|param| self.param_view(device, param))
                            .map(|p| BankParam {
                                param: p.param,
                                name: p.name,
                                value: p.value,
                                text: p.text,
                                held: p.held,
                            })
                            .collect(),
                    })
                    .collect();
                Ok(to_value(&BanksResult {
                    device: args.device,
                    banks,
                }))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }
}

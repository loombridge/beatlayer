//! MIDI Map Mode and Key Map Mode in the shell (WP-7.3; the map modes spec,
//! R1 to R10): which mode is on, the control selected in it, and learning,
//! which turns the next message on the remote input into a mapping of the
//! selected control.
//!
//! The modes and the selection are view state: they aren't saved with the
//! set and aren't undo steps. What learning makes are the mappings of
//! `remote.map` and `remote.set_mode`, one undo step for the run of them
//! that belongs to one control (R5).

use bl_control::guess;
use bl_core::midi::MidiMessage;
use bl_model::{ControlKind, RemoteMapping, RemoteMode, RemoteSource, RemoteTarget};
use bl_shell_spec::midi_io::PortDirection;
use bl_shell_spec::remote as ids;
use bl_shell_spec::remote::{MapMode, MapModeArgs, MappingInfo, SelectArgs, StateResult};
use serde_json::{Value, json};

use super::remote::RunKey;
use super::{Shell, ShellError, to_value};

/// How many of a controller's messages the guess looks back over.
const GUESS_WINDOW: usize = 32;

/// The map mode that is on and what it has learned (R2 to R7, R10).
#[derive(Default)]
pub(super) struct MapState {
    pub(super) mode: MapMode,
    /// The control the next message or key maps (R4).
    selected: Option<RemoteTarget>,
    /// The keys held, in the order they went down: channel and key.
    held: Vec<(u8, u8)>,
    /// A slot's root key while it is held, and the lowest key that went down
    /// below it and the highest above it meanwhile (R6).
    root: Option<(u8, u8)>,
    below: Option<u8>,
    above: Option<u8>,
    /// The controller being learned (a 14-bit pair's first), and its
    /// messages since it was first heard.
    controller: Option<(u8, u8)>,
    messages: Vec<MidiMessage>,
    /// The mode the messages so far suggest, while they do (R10).
    guess: Option<RemoteMode>,
    /// Whether the mode was chosen by hand, so the messages stop guessing.
    chosen: bool,
}

impl MapState {
    /// Forgets what learning has heard, as a new selection does.
    fn forget_heard(&mut self) {
        *self = Self {
            mode: self.mode,
            selected: self.selected,
            ..Self::default()
        };
    }
}

impl Shell {
    /// The map mode that is on.
    #[must_use]
    pub fn map_mode(&self) -> MapMode {
        self.remote.map.mode
    }

    /// The control selected in a map mode.
    #[must_use]
    pub fn map_selection(&self) -> Option<RemoteTarget> {
        self.remote.map.selected
    }

    /// The mode guessed for the controller being learned, while it is
    /// learned.
    #[must_use]
    pub fn map_guess(&self) -> Option<RemoteMode> {
        self.remote.map.guess
    }

    /// The selected control's controller mapping in MIDI Map Mode, whose
    /// mode the Status Bar's chooser sets (R10).
    #[must_use]
    pub fn map_controller_mapping(&self) -> Option<RemoteMapping> {
        let target = self.remote.map.selected?;
        if self.remote.map.mode != MapMode::Midi {
            return None;
        }
        self.editor
            .document()
            .settings()
            .remote
            .iter()
            .find(|m| {
                m.target == target
                    && matches!(
                        m.source,
                        RemoteSource::Cc { .. } | RemoteSource::Cc14 { .. }
                    )
            })
            .cloned()
    }

    /// Whether an input has its Remote switch on, so messages can reach
    /// learning (R5).
    #[must_use]
    pub fn remote_input_on(&self) -> bool {
        self.midi
            .ports()
            .iter()
            .any(|port| port.direction == PortDirection::Input && port.remote)
    }

    /// The Mapping Browser's rows: the mode's mappings in order of their
    /// source (R8); none while no map mode is on.
    #[must_use]
    pub fn map_rows(&self) -> Vec<MappingInfo> {
        let keys = match self.remote.map.mode {
            MapMode::Off => return Vec::new(),
            MapMode::Midi => false,
            MapMode::Key => true,
        };
        let mut rows: Vec<MappingInfo> = self
            .editor
            .document()
            .settings()
            .remote
            .iter()
            .filter(|m| m.source.is_key() == keys)
            .map(|m| self.mapping_info(m))
            .collect();
        rows.sort_by(|a, b| a.source.sort_key().cmp(&b.source.sort_key()));
        rows
    }

    /// Turns a map mode on or off (R1, R2): one at a time, and none with a
    /// rack's Map mode, which a map mode turns off.
    pub(super) fn set_map_mode(&mut self, mode: MapMode) {
        self.end_remote_run();
        if mode != MapMode::Off
            && let Some(rack) = self.devices.map_mode()
        {
            let _ = self.run("rack.map_mode", json!({ "rack": rack, "on": false }));
        }
        self.remote.map = MapState {
            mode,
            ..MapState::default()
        };
        self.midi
            .sender()
            .remote()
            .set_learning(mode == MapMode::Midi);
    }

    /// A rack's Map mode turning on turns a map mode off (R2).
    pub(super) fn rack_map_mode_ends_map_mode(&mut self, id: &str) {
        if id == bl_shell_spec::devices::RACK_MAP_MODE
            && self.devices.map_mode().is_some()
            && self.remote.map.mode != MapMode::Off
        {
            self.set_map_mode(MapMode::Off);
        }
    }

    /// Drops a selection whose control the set no longer has.
    pub(super) fn keep_selection(&mut self) {
        if let Some(target) = self.remote.map.selected
            && !self.editor.document().has_remote_target(&target)
        {
            self.remote.map.selected = None;
            self.remote.map.forget_heard();
        }
    }

    /// The kind of control `target` is, or `None` if the set has no such
    /// control.
    pub(super) fn kind_of(&self, target: &RemoteTarget) -> Option<ControlKind> {
        if !self.editor.document().has_remote_target(target) {
            return None;
        }
        target
            .natural_kind()
            .or_else(|| self.param_control(target).map(|(kind, _)| kind))
    }

    fn map_state(&self) -> StateResult {
        let map = &self.remote.map;
        StateResult {
            mode: map.mode,
            selected: map.selected,
            guess: map.guess,
            remote_input: self.remote_input_on(),
            rows: self.map_rows(),
        }
    }

    /// Runs `remote.map_mode`, `remote.select` and `remote.state` (R2, R4,
    /// R10).
    pub(super) fn run_map_mode(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let failed = |message: &str| ShellError::Failed {
            command: id.to_owned(),
            message: message.to_owned(),
        };
        match id {
            ids::MAP_MODE => {
                let MapModeArgs { mode, toggle } = Self::args(id, args)?;
                let mode = if toggle && mode == self.remote.map.mode {
                    MapMode::Off
                } else {
                    mode
                };
                self.set_map_mode(mode);
            }
            ids::SELECT => {
                let SelectArgs { target } = Self::args(id, args)?;
                if let Some(target) = target {
                    if self.remote.map.mode == MapMode::Off {
                        return Err(failed("turn a map mode on first: `remote.map_mode`"));
                    }
                    let kind = self
                        .kind_of(&target)
                        .ok_or_else(|| failed("the set has no such control"))?;
                    if self.remote.map.mode == MapMode::Key && kind == ControlKind::Continuous {
                        return Err(failed("Key Map Mode takes no continuous control"));
                    }
                }
                if self.remote.map.selected != target {
                    self.end_remote_run();
                    self.remote.map.selected = target;
                    self.remote.map.forget_heard();
                }
            }
            _ => Self::args::<bl_commands::NoArgs>(id, args).map(drop)?,
        }
        Ok(to_value(&self.map_state()))
    }

    /// Settles the mode of the selected controller's mapping by hand, so
    /// later messages stop guessing (R10).
    pub(super) fn mode_chosen(&mut self, source: &RemoteSource) {
        let map = &mut self.remote.map;
        if let RemoteSource::Cc {
            channel,
            controller,
        }
        | RemoteSource::Cc14 {
            channel,
            controller,
        } = *source
        {
            map.controller = Some((channel, controller));
            map.chosen = true;
            map.guess = None;
        }
    }

    /// Learns from a message on the remote input while a map mode is on
    /// (R5, R6): in MIDI Map Mode it maps the selected control, and in Key
    /// Map Mode it is only dropped.
    pub(super) fn learn(&mut self, message: &MidiMessage) {
        if self.remote.map.mode != MapMode::Midi {
            return;
        }
        let Some(target) = self.remote.map.selected else {
            return;
        };
        let Some(kind) = self.kind_of(&target) else {
            return;
        };
        let Some(channel) = message.channel().map(|c| c + 1) else {
            return;
        };
        let (a, b) = (message.data1(), message.data2());
        match message.status() & 0xF0 {
            0x90 if b > 0 => self.learn_note(target, kind, channel, a),
            0x80 | 0x90 => {
                let map = &mut self.remote.map;
                map.held.retain(|&held| held != (channel, a));
                if map.root == Some((channel, a)) {
                    map.root = None;
                    map.below = None;
                    map.above = None;
                }
            }
            0xB0 => self.learn_controller(target, channel, a, message),
            0xE0 => self.learn_source(RemoteSource::PitchBend { channel }, target, None),
            _ => {}
        }
    }

    fn learn_note(&mut self, target: RemoteTarget, kind: ControlKind, channel: u8, key: u8) {
        let map = &mut self.remote.map;
        let first = map
            .held
            .iter()
            .find(|&&(c, k)| c == channel && k != key)
            .map(|&(_, k)| k);
        map.held.push((channel, key));
        let single = RemoteSource::Note { channel, key };
        let source = match kind {
            // Spec: map-modes R6. The first key is the root; a key below and
            // a key above it, while it is held, make the range.
            ControlKind::Slot => match map.root {
                Some((c, root)) if c == channel && key != root => {
                    if key < root {
                        map.below = Some(map.below.map_or(key, |b| b.min(key)));
                    } else {
                        map.above = Some(map.above.map_or(key, |a| a.max(key)));
                    }
                    let (Some(low), Some(high)) = (map.below, map.above) else {
                        return;
                    };
                    RemoteSource::NoteRange {
                        channel,
                        low,
                        high,
                        root: Some(root),
                    }
                }
                _ => {
                    map.root = Some((channel, key));
                    map.below = None;
                    map.above = None;
                    single
                }
            },
            // Spec: map-modes R5. A key held while a second goes down maps
            // the range between them.
            ControlKind::Continuous | ControlKind::Radio { .. } => match first {
                Some(first) => RemoteSource::NoteRange {
                    channel,
                    low: first.min(key),
                    high: first.max(key),
                    root: None,
                },
                None => single,
            },
            _ => single,
        };
        self.learn_source(source, target, None);
    }

    fn learn_controller(
        &mut self,
        target: RemoteTarget,
        channel: u8,
        controller: u8,
        message: &MidiMessage,
    ) {
        let map = &mut self.remote.map;
        let partner = matches!(map.controller,
            Some((c, n)) if c == channel && n < 32 && controller == n + 32);
        if !partner && map.controller != Some((channel, controller)) {
            map.controller = Some((channel, controller));
            map.messages.clear();
            map.chosen = false;
        }
        if map.chosen {
            return;
        }
        map.messages.push(*message);
        if map.messages.len() > GUESS_WINDOW {
            map.messages.remove(0);
        }
        let mode = guess(&map.messages);
        map.guess = Some(mode);
        let (channel, controller) = map.controller.unwrap_or((channel, controller));
        let source = if mode == RemoteMode::Absolute14 {
            RemoteSource::Cc14 {
                channel,
                controller,
            }
        } else {
            RemoteSource::Cc {
                channel,
                controller,
            }
        };
        self.learn_source(source, target, Some(mode));
    }

    /// Maps `source` to the selected control, or changes the mode of the
    /// mapping it already has; the whole learning of one control is one
    /// undo step (R5).
    fn learn_source(
        &mut self,
        source: RemoteSource,
        target: RemoteTarget,
        mode: Option<RemoteMode>,
    ) {
        let held: Option<RemoteMapping> = self
            .editor
            .document()
            .settings()
            .remote
            .iter()
            .find(|m| m.source == source && m.target == target)
            .cloned();
        if let Some(held) = &held
            && mode.is_none_or(|mode| mode == held.mode)
        {
            return;
        }
        let now = self.midi.now();
        let key = RunKey::Learning(target);
        if self.remote.run.as_ref().map(|(key, _)| key) != Some(&key) {
            self.end_remote_run();
            self.begin_gesture("Map");
        }
        self.remote.run = Some((key, now));
        self.remote.applying = true;
        let result = match (held, mode) {
            (Some(_), Some(mode)) => {
                self.run(ids::SET_MODE, json!({ "source": source, "mode": mode }))
            }
            _ => self.run(
                ids::MAP,
                json!({ "source": source, "target": target, "mode": mode }),
            ),
        };
        self.remote.applying = false;
        if result.is_err() {
            self.remote.refused += 1;
        }
    }
}

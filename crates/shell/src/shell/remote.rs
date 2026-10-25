//! MIDI and key mappings in the shell (WP-7.1; the remote mapping spec, R5
//! to R14): the commands that change the set's mappings, the remote input
//! drained on each poll, and running a mapped control's command.
//!
//! The mappings are the set's (`Settings::remote`); the table the MIDI
//! thread judges messages by is published from them whenever they change
//! (R9). What a message does to a control is `bl-control`'s; this turns its
//! result into the command a click or a drag on that control runs (R11).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use bl_commands::{NoArgs, Permission};
use bl_control::{Action, Hit, Law, Memory};
use bl_core::midi::{MidiInputEvent, MidiMessage};
use bl_model::modulation::Travel;
use bl_model::{
    Command, ControlKind, CrossfadeAssign, MixerParam, ParamKind, RemoteMapping, RemoteSource,
    RemoteTarget,
};
use bl_shell_spec::remote as ids;
use bl_shell_spec::remote::{MappingInfo, MappingsResult};
use serde_json::{Value, json};

use super::{Shell, ShellError, to_value};

/// How long a run of messages to one continuous control may pause and stay
/// one undo step (R12).
const RUN_PAUSE: Duration = Duration::from_secs(1);

/// The remote input's state in the shell.
#[derive(Default)]
pub(super) struct RemoteState {
    /// The mappings the MIDI thread's table was published from.
    published: Vec<RemoteMapping>,
    /// What each mapping remembers between messages.
    memory: HashMap<RemoteSource, Memory>,
    /// The mapping whose run of messages to a continuous control is one
    /// undo step, and when its latest message came.
    run: Option<(RemoteSource, Instant)>,
    /// Set while a mapped message's command runs, so it doesn't end its own
    /// run.
    applying: bool,
    /// Whether a map mode is on, so no mapping acts (R13).
    map_mode: bool,
    /// The messages that crossed for learning while a map mode was on.
    learned: Vec<MidiInputEvent>,
    applied: u64,
    refused: u64,
}

/// A command a mapped message runs.
struct Run {
    id: &'static str,
    args: Value,
    /// The undo step's name, for a command that moves a continuous control.
    gesture: Option<&'static str>,
}

fn cmd(id: &'static str, args: Value) -> Option<Run> {
    Some(Run {
        id,
        args,
        gesture: None,
    })
}

fn drag(id: &'static str, args: Value, name: &'static str) -> Option<Run> {
    Some(Run {
        id,
        args,
        gesture: Some(name),
    })
}

impl Shell {
    /// Turns a map mode's flag on or off: while it is on no mapping acts,
    /// and every message on the remote input crosses to
    /// [`Shell::take_learned_messages`] (R10, R13).
    pub fn set_map_mode(&mut self, on: bool) {
        self.end_remote_run();
        self.remote.map_mode = on;
        self.remote.learned.clear();
        self.midi.sender().remote().set_learning(on);
    }

    /// Whether a map mode is on.
    #[must_use]
    pub fn map_mode(&self) -> bool {
        self.remote.map_mode
    }

    /// The messages that crossed from the remote input since the last call
    /// while a map mode was on, for learning.
    pub fn take_learned_messages(&mut self) -> Vec<MidiInputEvent> {
        std::mem::take(&mut self.remote.learned)
    }

    /// How many mapped messages ran their control's command, and how many
    /// were refused: a control the set no longer has, or a command that
    /// failed (R11).
    #[must_use]
    pub fn remote_counts(&self) -> (u64, u64) {
        (self.remote.applied, self.remote.refused)
    }

    /// Whether a key mapping would act on the key `name` now (R13): no map
    /// mode is on and the set maps it. A window asks before it takes a key
    /// from the shortcuts.
    #[must_use]
    pub fn key_is_mapped(&self, name: &str) -> bool {
        !self.remote.map_mode
            && self
                .editor
                .document()
                .settings()
                .remote
                .iter()
                .any(|m| matches!(&m.source, RemoteSource::Key { key } if key == name))
    }

    /// Publishes the table of mapped messages if the mappings changed (R9).
    pub(super) fn publish_remote(&mut self) {
        let mappings = &self.editor.document().settings().remote;
        if *mappings == self.remote.published {
            return;
        }
        self.midi
            .sender()
            .remote()
            .publish(&bl_control::bits(mappings));
        self.remote.published.clone_from(mappings);
        let held: Vec<&RemoteSource> = mappings.iter().map(|m| &m.source).collect();
        self.remote
            .memory
            .retain(|source, _| held.contains(&source));
    }

    /// Applies the messages mappings took since the last poll, in the order
    /// they arrived (R10, R11), and ends a run of messages that has paused
    /// for a second (R12). Returns whether the set changed.
    pub(super) fn poll_remote(&mut self) -> bool {
        self.publish_remote();
        let revision = self.document_revision;
        let mut events = Vec::new();
        self.midi.sender().remote().drain(&mut events);
        if self.remote.map_mode {
            self.remote.learned.append(&mut events);
        }
        for event in events {
            self.apply_message(&event.message);
        }
        if let Some((_, at)) = self.remote.run
            && self.midi.now().saturating_duration_since(at) >= RUN_PAUSE
        {
            self.end_remote_run();
        }
        revision != self.document_revision
    }

    /// Ends the run of messages that is one undo step, if one is open.
    pub(super) fn end_remote_run(&mut self) {
        if self.remote.run.take().is_some() {
            self.end_gesture();
        }
    }

    /// Ends the run for any command but the read ones and the mapped
    /// message's own (R12: any other edit ends it).
    pub(super) fn remote_run_ends_for(&mut self, id: &str) {
        let reads = self
            .registry
            .get(id)
            .is_some_and(|spec| spec.permission == Some(Permission::Read));
        if self.remote.run.is_some() && !self.remote.applying && !reads {
            self.end_remote_run();
        }
    }

    /// Applies a MIDI message to the mappings that take it; whether one did.
    fn apply_message(&mut self, message: &MidiMessage) -> bool {
        if self.remote.map_mode {
            return false;
        }
        let mut taken = false;
        for mapping in self.editor.document().settings().remote.clone() {
            let memory = self
                .remote
                .memory
                .entry(mapping.source.clone())
                .or_default();
            if let Some(hit) = bl_control::hit(&mapping, message, memory) {
                taken = true;
                self.apply_hit(&mapping, hit);
            }
        }
        taken
    }

    /// Applies a computer key to the mapping of it; whether there is one.
    fn apply_key(&mut self, key: &str, pressed: bool) -> bool {
        if self.remote.map_mode {
            return false;
        }
        let mut taken = false;
        for mapping in self.editor.document().settings().remote.clone() {
            if matches!(&mapping.source, RemoteSource::Key { key: k } if k == key) {
                taken = true;
                self.apply_hit(&mapping, if pressed { Hit::KeyDown } else { Hit::KeyUp });
            }
        }
        taken
    }

    /// Runs the command a hit on `mapping` asks of its control.
    fn apply_hit(&mut self, mapping: &RemoteMapping, hit: Hit) {
        let (min, max) = self.law_range(mapping);
        let law = Law::of(mapping, min, max);
        let key = match (&mapping.source, hit) {
            (RemoteSource::NoteRange { low, .. }, Hit::NoteOn { key, .. }) => u32::from(key - low),
            _ => 0,
        };
        let memory = self
            .remote
            .memory
            .entry(mapping.source.clone())
            .or_default();
        let Some(action) = bl_control::act(&law, hit, key, memory) else {
            return;
        };
        // A button's release has nothing to do, and isn't a refusal.
        if action == Action::Release && mapping.kind == ControlKind::Button {
            return;
        }
        let Some(run) = self.command_for(mapping, action) else {
            self.remote.refused += 1;
            return;
        };
        let now = self.midi.now();
        match run.gesture {
            Some(name) => {
                if self.remote.run.as_ref().map(|(source, _)| source) != Some(&mapping.source) {
                    self.end_remote_run();
                    self.begin_gesture(name);
                }
                self.remote.run = Some((mapping.source.clone(), now));
            }
            None => self.end_remote_run(),
        }
        self.remote.applying = true;
        let result = self.run(run.id, run.args);
        self.remote.applying = false;
        if result.is_ok() {
            self.remote.applied += 1;
        } else {
            self.remote.refused += 1;
        }
    }

    /// A mapping's Min and Max as the laws take them: positions on the
    /// control's travel for a continuous control, as they are otherwise.
    fn law_range(&self, mapping: &RemoteMapping) -> (f64, f64) {
        if mapping.kind != ControlKind::Continuous {
            return (mapping.min, mapping.max);
        }
        (
            self.position_of(&mapping.target, mapping.min),
            self.position_of(&mapping.target, mapping.max),
        )
    }

    /// The control's travel; `None` for the crossfader, which is straight.
    fn travel(&self, target: &RemoteTarget) -> Option<Travel<'_>> {
        match *target {
            RemoteTarget::Mixer { param, .. } | RemoteTarget::Chain { param, .. } => {
                Some(match param {
                    MixerParam::Volume => Travel::Fader,
                    MixerParam::Mute => Travel::Switch,
                    _ => Travel::Pan,
                })
            }
            RemoteTarget::Send { .. } => Some(Travel::Send),
            RemoteTarget::Device { device, param } => self
                .editor
                .document()
                .device(device)
                .and_then(|d| self.devices.description(&d.kind))
                .and_then(|description| description.param(param))
                .map(Travel::Param),
            _ => None,
        }
    }

    fn position_of(&self, target: &RemoteTarget, value: f64) -> f64 {
        self.travel(target)
            .map_or(value.clamp(0.0, 1.0), |t| t.position(value))
    }

    fn value_at(&self, target: &RemoteTarget, position: f64) -> f64 {
        self.travel(target)
            .map_or(position.clamp(0.0, 1.0), |t| t.value(position))
    }

    /// The control's value now, for the actions that turn it over.
    fn current(&self, target: &RemoteTarget) -> Option<f64> {
        let doc = self.editor.document();
        let flag = |on: bool| f64::from(u8::from(on));
        Some(match *target {
            RemoteTarget::Solo { track } => flag(doc.track(track)?.mixer().solo),
            RemoteTarget::Arm { track } => flag(doc.track(track)?.mixer().arm),
            RemoteTarget::Mixer {
                track,
                param: MixerParam::Mute,
            } => flag(doc.track(track)?.mixer().mute),
            RemoteTarget::Mixer { track, param } => {
                let mixer = doc.track(track)?.mixer();
                match param {
                    MixerParam::Volume => mixer.volume,
                    MixerParam::Pan => mixer.pan,
                    MixerParam::SplitLeft => mixer.split_left,
                    _ => mixer.split_right,
                }
            }
            RemoteTarget::CrossfadeAssign { track } => match doc.track(track)?.mixer().crossfade {
                CrossfadeAssign::A => 0.0,
                CrossfadeAssign::None => 1.0,
                CrossfadeAssign::B => 2.0,
            },
            RemoteTarget::Crossfader => doc.settings().crossfader,
            RemoteTarget::DeviceOn { device } => flag(doc.device(device)?.enabled),
            RemoteTarget::Device { device, param } => {
                doc.device(device)?.params.get(&param).copied()?
            }
            _ => return None,
        })
    }

    /// The command an action on a mapping's control runs, or `None` if the
    /// set no longer has the control or the action means nothing to it
    /// (R11).
    fn command_for(&self, mapping: &RemoteMapping, action: Action) -> Option<Run> {
        let target = mapping.target;
        let current = self.current(&target);
        let on = |current: Option<f64>| match action {
            Action::Set(on) => Some(on),
            Action::Toggle => current.map(|v| v < 0.5),
            _ => None,
        };
        match (target, action) {
            (RemoteTarget::Slot { track, scene }, Action::Press(_)) => {
                cmd("session.launch", json!({ "track": track, "scene": scene }))
            }
            (RemoteTarget::Slot { track, scene }, Action::Release) => {
                cmd("session.release", json!({ "track": track, "scene": scene }))
            }
            (RemoteTarget::SceneLaunch { scene }, Action::Press(_)) => {
                cmd("session.launch_scene", json!({ "scene": scene }))
            }
            (RemoteTarget::TrackStop { track }, Action::Press(_)) => {
                cmd("session.stop_track", json!({ "track": track }))
            }
            (RemoteTarget::StopAll, Action::Press(_)) => cmd("session.stop_all", json!({})),
            (RemoteTarget::Solo { track }, _) => cmd(
                "mixer.set_solo",
                json!({ "track": track, "solo": on(current)? }),
            ),
            (RemoteTarget::Arm { track }, _) => cmd(
                "mixer.set_arm",
                json!({ "track": track, "arm": on(current)? }),
            ),
            // The activator is on while the track isn't muted.
            (
                RemoteTarget::Mixer {
                    track,
                    param: MixerParam::Mute,
                },
                _,
            ) => {
                let mute = match action {
                    Action::Set(on) => !on,
                    _ => current? < 0.5,
                };
                cmd("mixer.set_mute", json!({ "track": track, "mute": mute }))
            }
            (RemoteTarget::DeviceOn { device }, _) => cmd(
                "device.set_enabled",
                json!({ "device": device, "enabled": on(current)? }),
            ),
            (RemoteTarget::CrossfadeAssign { track }, Action::Next | Action::Option(_)) => {
                let option = match action {
                    Action::Option(option) => option,
                    _ => (current? as u32 + 1) % 3,
                };
                let assign = ["a", "none", "b"][option as usize % 3];
                cmd(
                    "mixer.set_crossfade_assign",
                    json!({ "track": track, "assign": assign }),
                )
            }
            (_, Action::Position(position)) => self.drag_to(&target, position),
            (_, Action::MaxOrMin) => {
                let (min, max) = self.law_range(mapping);
                let at = self.position_of(&target, current?);
                self.drag_to(&target, if (at - max).abs() < 1e-9 { min } else { max })
            }
            (RemoteTarget::Device { device, param }, _) => {
                let value = match action {
                    Action::Option(option) => f64::from(option),
                    Action::Next => {
                        let ControlKind::Radio { options } = mapping.kind else {
                            return None;
                        };
                        f64::from((current? as u32 + 1) % options)
                    }
                    _ => f64::from(u8::from(on(current)?)),
                };
                cmd(
                    "device.set_param",
                    json!({ "device": device, "param": param, "value": value }),
                )
            }
            _ => None,
        }
    }

    /// The command that moves a continuous control to `position` on its
    /// travel.
    fn drag_to(&self, target: &RemoteTarget, position: f64) -> Option<Run> {
        let value = self.value_at(target, position);
        match *target {
            RemoteTarget::Mixer { track, param } => match param {
                MixerParam::Volume => drag(
                    "mixer.set_volume",
                    json!({ "track": track, "volume": value }),
                    "Change volume",
                ),
                MixerParam::Pan => drag(
                    "mixer.set_pan",
                    json!({ "track": track, "pan": value }),
                    "Change pan",
                ),
                MixerParam::SplitLeft => drag(
                    "mixer.set_split_pan_left",
                    json!({ "track": track, "pan": value }),
                    "Change split pan",
                ),
                MixerParam::SplitRight => drag(
                    "mixer.set_split_pan_right",
                    json!({ "track": track, "pan": value }),
                    "Change split pan",
                ),
                MixerParam::Mute => None,
            },
            RemoteTarget::Send { track, to } => drag(
                "mixer.set_send",
                json!({ "track": track, "to": to, "level": value }),
                "Change send",
            ),
            RemoteTarget::Crossfader => drag(
                "mixer.set_crossfader",
                json!({ "position": value }),
                "Change crossfader",
            ),
            RemoteTarget::Device { device, param } => drag(
                "device.set_param",
                json!({ "device": device, "param": param, "value": value }),
                "Change parameter",
            ),
            RemoteTarget::Chain { chain, param } => match param {
                MixerParam::Volume => drag(
                    "chain.set_gain",
                    json!({ "chain": chain, "volume": value }),
                    "Change chain volume",
                ),
                MixerParam::Pan => drag(
                    "chain.set_pan",
                    json!({ "chain": chain, "pan": value }),
                    "Change chain pan",
                ),
                _ => None,
            },
            _ => None,
        }
    }

    /// The path to a control and its name, for `remote.mappings` (R14).
    fn describe(&self, target: &RemoteTarget) -> (String, String) {
        let doc = self.editor.document();
        let track = |id| {
            doc.track(id)
                .map_or_else(String::new, |t| t.name().to_owned())
        };
        let scene = |id| doc.scene_position(id).map_or(0, |at| at + 1);
        let device = |id| {
            doc.device(id).map_or_else(String::new, |d| {
                format!(
                    "{} / {}",
                    track(doc.device_track(id).unwrap_or(doc.main_id())),
                    d.name
                )
            })
        };
        let mixer = |param| match param {
            MixerParam::Volume => "Volume",
            MixerParam::Pan => "Pan",
            MixerParam::Mute => "Track Activator",
            MixerParam::SplitLeft => "Split Pan Left",
            MixerParam::SplitRight => "Split Pan Right",
        };
        match *target {
            RemoteTarget::Slot { track: t, scene: s } => {
                (track(t), format!("Clip Slot {}", scene(s)))
            }
            RemoteTarget::SceneLaunch { scene: s } => {
                ("Session".to_owned(), format!("Scene {} Launch", scene(s)))
            }
            RemoteTarget::TrackStop { track: t } => (track(t), "Stop".to_owned()),
            RemoteTarget::StopAll => ("Session".to_owned(), "Stop All Clips".to_owned()),
            RemoteTarget::Mixer { track: t, param } => (track(t), mixer(param).to_owned()),
            RemoteTarget::Send { track: t, to } => (track(t), format!("Send to {}", track(to))),
            RemoteTarget::Solo { track: t } => (track(t), "Solo".to_owned()),
            RemoteTarget::Arm { track: t } => (track(t), "Arm".to_owned()),
            RemoteTarget::CrossfadeAssign { track: t } => (track(t), "Crossfade Assign".to_owned()),
            RemoteTarget::Crossfader => ("Main".to_owned(), "Crossfader".to_owned()),
            RemoteTarget::DeviceOn { device: d } => (device(d), "Device On".to_owned()),
            RemoteTarget::Device { device: d, param } => {
                let name = doc
                    .device(d)
                    .and_then(|dev| self.devices.description(&dev.kind))
                    .and_then(|description| description.param(param))
                    .map_or_else(|| format!("Parameter {}", param.0), |p| p.name.clone());
                (device(d), name)
            }
            RemoteTarget::Chain { chain, param } => (
                doc.chain(chain)
                    .map_or_else(String::new, |c| c.name.clone()),
                mixer(param).to_owned(),
            ),
        }
    }

    fn mapping_info(&self, mapping: &RemoteMapping) -> MappingInfo {
        let (path, name) = self.describe(&mapping.target);
        MappingInfo {
            source: mapping.source.clone(),
            target: mapping.target,
            path,
            name,
            kind: mapping.kind,
            min: mapping.min,
            max: mapping.max,
            mode: mapping.mode,
        }
    }

    /// The kind and the range a device parameter's control has, from its
    /// description: a toggle is a ranged switch, a choice a radio button.
    fn param_control(&self, target: &RemoteTarget) -> Option<(ControlKind, Option<(f64, f64)>)> {
        let RemoteTarget::Device { device, param } = *target else {
            return None;
        };
        let doc = self.editor.document();
        let descriptor = self
            .devices
            .description(&doc.device(device)?.kind)?
            .param(param)?;
        Some(match &descriptor.kind {
            ParamKind::Toggle => (ControlKind::RangedSwitch, None),
            ParamKind::Choice { choices } => (
                ControlKind::Radio {
                    options: choices.len() as u32,
                },
                None,
            ),
            _ => (
                ControlKind::Continuous,
                Some((descriptor.min(), descriptor.max())),
            ),
        })
    }

    /// Sets the set's mappings to `mappings` as one undo step named `label`.
    fn set_mappings(
        &mut self,
        id: &str,
        label: &'static str,
        mappings: Vec<RemoteMapping>,
    ) -> Result<(), ShellError> {
        self.end_remote_run();
        self.edit(|editor| {
            editor.transact(label, |editor| {
                editor
                    .apply(Command::SetRemoteMappings { mappings })
                    .map(drop)
            })
        })
        .map_err(|error| ShellError::Failed {
            command: id.to_owned(),
            message: error.to_string(),
        })?;
        self.publish_remote();
        Ok(())
    }

    /// The mapping with `source`.
    fn mapping_of(&self, id: &str, source: &RemoteSource) -> Result<RemoteMapping, ShellError> {
        self.editor
            .document()
            .settings()
            .remote
            .iter()
            .find(|m| &m.source == source)
            .cloned()
            .ok_or_else(|| ShellError::Failed {
                command: id.to_owned(),
                message: "no mapping has that source; `remote.mappings` lists them".to_owned(),
            })
    }

    /// Replaces the mapping with `source` by `change` of it.
    fn change_mapping(
        &mut self,
        id: &str,
        label: &'static str,
        source: &RemoteSource,
        change: impl FnOnce(&mut RemoteMapping),
    ) -> Result<Value, ShellError> {
        let mut mapping = self.mapping_of(id, source)?;
        change(&mut mapping);
        mapping.check().map_err(|error| ShellError::Failed {
            command: id.to_owned(),
            message: error.to_string(),
        })?;
        let mappings = self
            .editor
            .document()
            .settings()
            .remote
            .iter()
            .map(|m| {
                if &m.source == source {
                    mapping.clone()
                } else {
                    m.clone()
                }
            })
            .collect();
        self.set_mappings(id, label, mappings)?;
        Ok(to_value(&self.mapping_info(&mapping)))
    }

    /// Runs one of the remote mapping commands (R14).
    pub(super) fn run_remote(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let failed = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        match id {
            ids::MAPPINGS => {
                Self::args::<NoArgs>(id, args)?;
                let mut mappings: Vec<MappingInfo> = self
                    .editor
                    .document()
                    .settings()
                    .remote
                    .iter()
                    .map(|m| self.mapping_info(m))
                    .collect();
                mappings.sort_by_key(|m| m.source.is_key());
                Ok(to_value(&MappingsResult {
                    mappings,
                    dropped_on_load: self.editor.document().remote_dropped(),
                }))
            }
            ids::MAP => {
                let args = Self::args::<ids::MapArgs>(id, args)?;
                let (kind, range) = match args.kind.or(args.target.natural_kind()) {
                    Some(kind) => (kind, args.target.natural_range()),
                    None => self
                        .param_control(&args.target)
                        .ok_or_else(|| failed("the set has no such device parameter".to_owned()))?,
                };
                let range = match (args.min, args.max, range) {
                    (None, None, range) => range,
                    (min, max, range) => {
                        let (low, high) = range.unwrap_or((0.0, 1.0));
                        Some((min.unwrap_or(low), max.unwrap_or(high)))
                    }
                };
                let mapping = RemoteMapping::new(args.source, args.target, kind, range, args.mode)
                    .map_err(|error| failed(error.to_string()))?;
                let mappings = bl_model::remote::with_mapping(
                    &self.editor.document().settings().remote,
                    mapping.clone(),
                );
                self.set_mappings(id, "Map", mappings)?;
                Ok(to_value(&self.mapping_info(&mapping)))
            }
            ids::UNMAP => {
                let args = Self::args::<ids::UnmapArgs>(id, args)?;
                let key = args.via == ids::Via::Key;
                let (source, target) = (args.source, args.target);
                if source.is_some() == target.is_some() {
                    return Err(failed(
                        "name the mapping by `source` or by `target`".to_owned(),
                    ));
                }
                let all = self.editor.document().settings().remote.clone();
                let kept: Vec<RemoteMapping> = all
                    .iter()
                    .filter(|m| match (&source, &target) {
                        (Some(source), _) => &m.source != source,
                        (_, Some(target)) => !(m.target == *target && m.source.is_key() == key),
                        _ => true,
                    })
                    .cloned()
                    .collect();
                let removed = all.len() - kept.len();
                if removed > 0 {
                    self.set_mappings(id, "Delete Mapping", kept)?;
                }
                Ok(to_value(&ids::UnmapResult { removed }))
            }
            ids::SET_RANGE => {
                let args = Self::args::<ids::SetRangeArgs>(id, args)?;
                self.change_mapping(id, "Change Mapping Range", &args.source, |m| {
                    m.min = args.min;
                    m.max = args.max;
                })
            }
            ids::INVERT_RANGE => {
                let args = Self::args::<ids::SourceArgs>(id, args)?;
                self.change_mapping(id, "Invert Range", &args.source, |m| {
                    std::mem::swap(&mut m.min, &mut m.max);
                })
            }
            ids::SET_MODE => {
                let args = Self::args::<ids::SetModeArgs>(id, args)?;
                self.change_mapping(id, "Change Mapping Mode", &args.source, |m| {
                    m.mode = args.mode;
                })
            }
            ids::INPUT => {
                let args = Self::args::<ids::InputArgs>(id, args)?;
                let taken = match (args.bytes, args.key) {
                    (Some(bytes), None) => {
                        let message = MidiMessage::from_bytes(&bytes).ok_or_else(|| {
                            failed("those bytes aren't one short MIDI message".to_owned())
                        })?;
                        self.apply_message(&message)
                    }
                    (None, Some(key)) => match args.pressed {
                        Some(pressed) => self.apply_key(&key, pressed),
                        None => {
                            let pressed = self.apply_key(&key, true);
                            self.apply_key(&key, false) | pressed
                        }
                    },
                    _ => return Err(failed("give `bytes` or `key`".to_owned())),
                };
                // A call is one undo step; only a controller's messages
                // run on (R12), and an agent mustn't wait for a second.
                self.end_remote_run();
                Ok(to_value(&ids::InputResult { taken }))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }
}

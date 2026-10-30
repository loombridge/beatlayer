//! What the map modes can map beyond the mixer's and the devices' controls
//! (WP-7.3b; the map modes spec, R11 to R24): the relative Session mapping
//! strip, the Control Bar, the crossfader's fields, the step buttons, the
//! Clip View's controls and Monitor, what each does when a message reaches
//! it, and the list `remote.targets` gives.

use bl_control::Action;
use bl_midi_tools::{ParamKind, ParamSpec, ParamValue, Registry, ToolDescriptor, ToolKind};
use bl_model::{
    BarControl, ClipControl, ClipData, ClipId, ControlKind, CrossfaderField, LaunchMode,
    LaunchQuantization, Monitor, RemoteMapping, RemoteTarget, StripControl, ToolControl, ToolId,
    TrackKind, WarpMode,
};
use bl_shell_spec::remote::{MapMode, TargetInfo, TargetsResult};
use serde_json::{Value, json};

use super::Shell;
use super::remote::{Run, cmd, drag, nothing};

/// How the monitor options are written, in the order of the radio button.
const MONITORS: [(&str, Monitor); 3] = [
    ("in", Monitor::In),
    ("auto", Monitor::Auto),
    ("off", Monitor::Off),
];

/// The warp modes in the order of the Warp Mode radio button.
const WARP_MODES: [WarpMode; 6] = [
    WarpMode::Beats,
    WarpMode::Tones,
    WarpMode::Texture,
    WarpMode::RePitch,
    WarpMode::Complex,
    WarpMode::ComplexPro,
];

/// The lowest gain a clip's Gain field shows, in decibels: silence below it.
const GAIN_FLOOR: f64 = bl_model::remote::GAIN_RANGE.0;

fn flag(on: bool) -> f64 {
    f64::from(u8::from(on))
}

/// The name of a control of the Control Bar.
const fn bar_name(control: BarControl) -> &'static str {
    match control {
        BarControl::TapTempo => "Tap Tempo",
        BarControl::Play => "Play",
        BarControl::Stop => "Stop",
        BarControl::ReEnableAutomation => "Re-Enable Automation",
        BarControl::CaptureMidi => "Capture MIDI",
        BarControl::New => "New",
        BarControl::Tempo => "Tempo",
        BarControl::Metronome => "Metronome",
        BarControl::Follow => "Follow",
        BarControl::ArrangementRecord => "Arrangement Record",
        BarControl::MidiOverdub => "MIDI Arrangement Overdub",
        BarControl::AutomationArm => "Automation Arm",
        BarControl::SessionRecord => "Session Record",
        BarControl::PunchIn => "Punch-In",
        BarControl::ArrangementLoop => "Arrangement Loop",
        BarControl::PunchOut => "Punch-Out",
        BarControl::DrawMode => "Draw Mode",
        BarControl::Quantization => "Quantization Menu",
    }
}

/// The name of a control of the Clip View.
const fn clip_name(control: ClipControl) -> &'static str {
    match control {
        ClipControl::Warp => "Warp",
        ClipControl::WarpMode => "Warp Mode",
        ClipControl::HighQuality => "HiQ",
        ClipControl::Ram => "RAM",
        ClipControl::Gain => "Gain",
        ClipControl::Transpose => "Transpose",
        ClipControl::Detune => "Detune",
        ClipControl::LaunchMode => "Launch Mode",
        ClipControl::Legato => "Legato",
        ClipControl::LaunchQuantization => "Quantize",
        ClipControl::Velocity => "Velocity",
        ClipControl::NudgeDown => "Nudge Backward",
        ClipControl::NudgeUp => "Nudge Forward",
        ClipControl::Scrub => "Scrub",
    }
}

/// Whether a Clip View control acts on the audio clips the panel shows,
/// rather than on the Session clip the Launch Properties panel shows.
const fn on_audio(control: ClipControl) -> bool {
    matches!(
        control,
        ClipControl::Warp
            | ClipControl::WarpMode
            | ClipControl::HighQuality
            | ClipControl::Ram
            | ClipControl::Gain
            | ClipControl::Transpose
            | ClipControl::Detune
    )
}

/// The IDs of the parameter `spec` of `tool` as a target names them.
fn tool_ids(tool: &ToolDescriptor, spec: &ParamSpec) -> Option<(ToolId, ToolId)> {
    Some((ToolId::new(&tool.id)?, ToolId::new(&spec.id)?))
}

impl Shell {
    /// The clips the Audio Utilities panel acts on: the Arrangement's
    /// selected audio clips, or the Session clip the Clip View shows.
    #[must_use]
    pub fn audio_panel_clips(&self) -> Vec<ClipId> {
        let doc = self.editor.document();
        let Some(shown) = self.clip_view.shown_audio_clip(doc, &self.arrangement) else {
            return Vec::new();
        };
        if self
            .clip_view
            .shown_session_clip(doc, &self.arrangement)
            .is_some()
        {
            return vec![shown];
        }
        let selected: Vec<ClipId> = self
            .arrangement
            .selected_clips()
            .iter()
            .copied()
            .filter(|id| {
                matches!(
                    doc.clip(*id).map(|clip| &clip.data),
                    Some(ClipData::Audio(_))
                )
            })
            .collect();
        if selected.is_empty() {
            vec![shown]
        } else {
            selected
        }
    }

    /// The clips a Clip View control acts on now (map modes R17): none while
    /// the Clip View shows none that it fits.
    fn clip_targets(&self, control: ClipControl) -> Vec<ClipId> {
        if on_audio(control) {
            return self.audio_panel_clips();
        }
        self.clip_view
            .shown_session_clip(self.editor.document(), &self.arrangement)
            .into_iter()
            .collect()
    }

    /// The highlighted scene's number, counting from 1.
    fn highlighted_number(&self) -> Option<usize> {
        let scene = self.highlighted_scene()?;
        self.editor
            .document()
            .scene_position(scene)
            .map(|at| at + 1)
    }

    /// The macro a device parameter is, when its device is a rack.
    fn macro_of(&self, target: &RemoteTarget) -> Option<&bl_model::Macro> {
        let RemoteTarget::Device { device, param } = *target else {
            return None;
        };
        let device = self.editor.document().device(device)?;
        let number = bl_model::macro_number(param)?;
        matches!(device.kind, bl_model::DeviceKind::Rack { .. })
            .then(|| device.macros.setting(number))
    }

    /// How many buttons a radio-button macro has (macros R34); `None` for
    /// every other parameter.
    pub(super) fn macro_steps(&self, target: &RemoteTarget) -> Option<u32> {
        self.macro_of(target)
            .filter(|m| m.style == bl_model::MacroStyle::RadioButtons)
            .map(|m| u32::from(m.steps))
    }

    /// A radio button's current option: a radio-button macro's value is a
    /// position on its travel, whose button is the nearest step.
    pub(super) fn option_of(&self, target: &RemoteTarget, value: f64) -> f64 {
        self.macro_steps(target).map_or(value, |steps| {
            (value.clamp(0.0, 1.0) * f64::from(steps.saturating_sub(1))).round()
        })
    }

    /// One step of a continuous control that steps, as a share of its travel
    /// (remote mapping R26): a stepped macro's, one scene of the number box,
    /// one semitone of Transpose.
    pub(super) fn step_of_other(&self, target: &RemoteTarget) -> Option<f64> {
        let whole = |(low, high): (f64, f64)| 1.0 / (high - low);
        match *target {
            RemoteTarget::Strip {
                control: StripControl::SceneNumber,
            }
            | RemoteTarget::Clip {
                control: ClipControl::Transpose,
            } => target.natural_range().map(whole),
            RemoteTarget::Device { .. } => self
                .macro_of(target)
                .filter(|m| m.style == bl_model::MacroStyle::Stepped)
                .map(|m| 1.0 / f64::from(m.steps.saturating_sub(1).max(1))),
            // A whole number of a MIDI Tool steps by one.
            RemoteTarget::MidiTool {
                control: ToolControl::Param { .. },
                ..
            } => match self.tool_param(target)?.2.kind {
                ParamKind::Whole { min, max } if max > min => Some(1.0 / (max - min) as f64),
                _ => None,
            },
            _ => None,
        }
    }

    /// The value of a control this module knows, in its own units.
    pub(super) fn current_other(&self, target: &RemoteTarget) -> Option<f64> {
        let doc = self.editor.document();
        let settings = doc.settings();
        match *target {
            RemoteTarget::Monitor { track } => {
                let monitor = doc.track(track)?.routing().monitor;
                MONITORS
                    .iter()
                    .position(|(_, m)| *m == monitor)
                    .map(|at| at as f64)
            }
            RemoteTarget::CueVolume => Some(settings.cue_volume),
            RemoteTarget::Strip {
                control: StripControl::SceneNumber,
            } => Some(self.highlighted_number().map_or(1.0, |n| n as f64)),
            RemoteTarget::Bar { control } => Some(match control {
                BarControl::Tempo => crate::transport::tempo(self),
                BarControl::Metronome => flag(settings.metronome.enabled),
                BarControl::Follow => flag(self.arrangement.follow()),
                BarControl::ArrangementRecord => flag(self.audio.recording()),
                BarControl::MidiOverdub => flag(settings.midi_overdub),
                BarControl::AutomationArm => flag(self.arrangement.automation_arm()),
                BarControl::SessionRecord => flag(self.session_run.is_some()),
                BarControl::PunchIn => flag(settings.punch_in),
                BarControl::PunchOut => flag(settings.punch_out),
                BarControl::ArrangementLoop => flag(settings.arrangement_loop.enabled),
                BarControl::DrawMode => flag(self.clip_view.draw_mode()),
                BarControl::Quantization => LaunchQuantization::ALL
                    .iter()
                    .position(|q| *q == self.store.launch().quantization)?
                    as f64,
                _ => return None,
            }),
            RemoteTarget::MidiTool { tool, control } => {
                let view = self.clip_view.midi_tools().view_by_id(tool.as_str())?;
                match control {
                    ToolControl::Auto => Some(flag(view.auto)),
                    ToolControl::Param { .. } => {
                        let (_, at, _) = self.tool_param(target)?;
                        match view.values.get(at)? {
                            ParamValue::Number(n) => Some(*n),
                            ParamValue::Whole(n) => Some(*n as f64),
                            ParamValue::Switch(on) => Some(flag(*on)),
                            ParamValue::Choice(n) => Some(*n as f64),
                            _ => None,
                        }
                    }
                    ToolControl::Apply | ToolControl::Reset => None,
                }
            }
            RemoteTarget::Clip { control } => {
                if control == ClipControl::Scrub {
                    return Some(0.5);
                }
                let clip = doc.clip(*self.clip_targets(control).first()?)?;
                let audio = match &clip.data {
                    ClipData::Audio(audio) => Some(audio),
                    ClipData::Midi(_) => None,
                };
                Some(match control {
                    ClipControl::Warp => flag(audio?.warp),
                    ClipControl::WarpMode => {
                        let mode = audio?.warp_mode;
                        WARP_MODES.iter().position(|m| *m == mode)? as f64
                    }
                    ClipControl::HighQuality => flag(audio?.high_quality),
                    ClipControl::Ram => flag(audio?.ram),
                    ClipControl::Gain => {
                        let gain = audio?.gain;
                        if gain <= 0.0 {
                            GAIN_FLOOR
                        } else {
                            (20.0 * gain.log10()).clamp(GAIN_FLOOR, bl_model::remote::GAIN_RANGE.1)
                        }
                    }
                    ClipControl::Transpose => f64::from(audio?.transpose),
                    ClipControl::Detune => audio?.detune,
                    ClipControl::LaunchMode => LaunchMode::ALL
                        .iter()
                        .position(|m| *m == clip.launch.mode)?
                        as f64,
                    ClipControl::Legato => flag(clip.launch.legato),
                    ClipControl::LaunchQuantization => clip.launch.quantization.map_or(0.0, |q| {
                        1.0 + LaunchQuantization::ALL
                            .iter()
                            .position(|s| *s == q)
                            .unwrap_or(0) as f64
                    }),
                    ClipControl::Velocity => clip.launch.velocity_amount,
                    _ => return None,
                })
            }
            _ => None,
        }
    }

    /// The scene and track of the slot that holds `clip`.
    fn slot_of(&self, clip: ClipId) -> Option<(bl_model::TrackId, bl_model::SceneId)> {
        let doc = self.editor.document();
        doc.track_order().into_iter().find_map(|track| {
            let scene = doc
                .track(track)?
                .slots()
                .find_map(|(scene, held)| (held.id == clip).then_some(scene))?;
            Some((track, scene))
        })
    }

    /// The command a message runs on one of the controls of this module, or
    /// `None` if the control isn't one of them. `Some(None)` is a control
    /// the message means nothing to, and `Some(Some(nothing))` one that is
    /// in the state it is asked for.
    pub(super) fn command_other(
        &self,
        mapping: &RemoteMapping,
        action: Action,
    ) -> Option<Option<Run>> {
        let target = mapping.target;
        let current = self.current_other(&target);
        let pressed = matches!(action, Action::Press(_) | Action::PressTransposed(..));
        // The state a switch is asked for.
        let want = |current: Option<f64>| match action {
            Action::Set(on) => Some(on),
            Action::Toggle => current.map(|v| v < 0.5),
            _ => None,
        };
        // The option a radio button is asked for, `options` in all.
        let option = |options: u32| match action {
            Action::Option(chosen) => Some(chosen),
            Action::Next => current.map(|v| (v as u32 + 1) % options),
            _ => None,
        };
        // A continuous control's value for the position the message gives.
        let value = |this: &Self| match action {
            Action::Position(position) => Some(this.value_at(&target, position)),
            Action::MaxOrMin => {
                let (min, max) = this.law_range(mapping);
                let at = this.position_of(&target, current?);
                let to = if (at - max).abs() < 1e-9 { min } else { max };
                Some(this.value_at(&target, to))
            }
            _ => None,
        };
        let press = |run: Option<Run>| Some(if pressed { run } else { None });
        match target {
            RemoteTarget::Monitor { track } => Some(option(3).and_then(|at| {
                cmd(
                    "routing.set_monitor",
                    json!({ "track": track, "monitor": MONITORS[at as usize % 3].0 }),
                )
            })),
            RemoteTarget::CueVolume => Some(value(self).and_then(|volume| {
                drag(
                    "mixer.set_cue_volume",
                    json!({ "volume": volume }),
                    "Change cue volume",
                )
            })),
            RemoteTarget::TrackLaunch { track } => {
                let scene = self.highlighted_scene();
                Some(match action {
                    Action::Press(velocity) => cmd(
                        "session.launch_highlighted",
                        json!({ "track": track, "velocity": velocity }),
                    ),
                    Action::PressTransposed(velocity, transpose) => cmd(
                        "session.launch_highlighted",
                        json!({ "track": track, "velocity": velocity, "transpose": transpose }),
                    ),
                    // shortcut: the release goes to the scene highlighted
                    // now, not the one pressed, if the highlight moved
                    // while the key was held
                    Action::Release => scene.map_or_else(nothing, |s| {
                        cmd("session.release", json!({ "track": track, "scene": s }))
                    }),
                    _ => None,
                })
            }
            RemoteTarget::Strip { control } => Some(match control {
                StripControl::SceneUp | StripControl::SceneDown => {
                    let step = if control == StripControl::SceneUp {
                        -1
                    } else {
                        1
                    };
                    return press(cmd("session.highlight_scene", json!({ "step": step })));
                }
                StripControl::LaunchScene => {
                    return press(cmd("session.launch_highlighted_scene", json!({})));
                }
                StripControl::CancelLaunch => {
                    return press(self.last_launched.and_then(|scene| {
                        cmd("session.cancel_scene_launch", json!({ "scene": scene }))
                    }));
                }
                StripControl::SceneNumber => {
                    let scenes = self.editor.document().scenes().len();
                    value(self).and_then(|number| {
                        let number = (number.round() as usize).clamp(1, scenes.max(1));
                        cmd("session.highlight_scene", json!({ "number": number }))
                    })
                }
            }),
            RemoteTarget::CrossfaderField { field } => Some(match action {
                Action::Press(_) | Action::Release => self.field_command(field, action),
                _ => None,
            }),
            RemoteTarget::StepBack => press(cmd("note.step_record", json!({ "back": true }))),
            RemoteTarget::StepForward => press(cmd("note.step_record", json!({ "back": false }))),
            RemoteTarget::Bar { control } => {
                Some(self.bar_command(control, current, action, &want, &option, &value))
            }
            RemoteTarget::Clip { control } => {
                Some(self.clip_command(control, current, action, &want, &option, &value))
            }
            RemoteTarget::MidiTool { tool, control } => {
                let id = tool.as_str();
                Some(match control {
                    ToolControl::Auto => want(current).and_then(|on| {
                        cmd("midi_tool.set_auto", json!({ "tool": id, "auto": on }))
                    }),
                    ToolControl::Apply => {
                        return press(cmd("midi_tool.apply", json!({ "tool": id })));
                    }
                    ToolControl::Reset => {
                        return press(cmd("midi_tool.reset", json!({ "tool": id })));
                    }
                    ToolControl::Param { param } => {
                        self.tool_command(target, tool, param, &want, &option, &value, current)
                    }
                })
            }
            _ => None,
        }
    }

    /// The MIDI Tool a target names, its parameter's place and the
    /// parameter.
    fn tool_param(
        &self,
        target: &RemoteTarget,
    ) -> Option<(&'static ToolDescriptor, usize, &'static ParamSpec)> {
        let RemoteTarget::MidiTool {
            tool,
            control: ToolControl::Param { param },
        } = target
        else {
            return None;
        };
        let descriptor = Registry::native().find(tool.as_str())?.descriptor();
        let at = descriptor
            .parameters
            .iter()
            .position(|spec| spec.id == param.as_str())?;
        Some((descriptor, at, &descriptor.parameters[at]))
    }

    /// The kind and range of a MIDI Tool's parameter as a control (map modes
    /// R23): a number a continuous control, a choice a radio button, a switch
    /// a ranged switch; the pitches, divisions and displays aren't mappable.
    pub(super) fn tool_param_control(
        &self,
        target: &RemoteTarget,
    ) -> Option<(ControlKind, Option<(f64, f64)>)> {
        Some(match &self.tool_param(target)?.2.kind {
            ParamKind::Number { min, max, .. } => (ControlKind::Continuous, Some((*min, *max))),
            ParamKind::Whole { min, max } => {
                (ControlKind::Continuous, Some((*min as f64, *max as f64)))
            }
            ParamKind::Choice { options } => (
                ControlKind::Radio {
                    options: u32::try_from(options.len()).ok()?,
                },
                None,
            ),
            ParamKind::Switch => (ControlKind::RangedSwitch, None),
            _ => return None,
        })
    }

    /// What a message runs on a MIDI Tool's parameter: `midi_tool.set` with
    /// its value, a run of messages one undo step named after the tool (map
    /// modes R23, MIDI Tools R35).
    #[expect(clippy::too_many_arguments, reason = "the message's whole context")]
    fn tool_command(
        &self,
        target: RemoteTarget,
        tool: ToolId,
        param: ToolId,
        want: &dyn Fn(Option<f64>) -> Option<bool>,
        option: &dyn Fn(u32) -> Option<u32>,
        value: &dyn Fn(&Self) -> Option<f64>,
        current: Option<f64>,
    ) -> Option<Run> {
        let (descriptor, _, spec) = self.tool_param(&target)?;
        let set =
            |json: Value| json!({ "tool": tool.as_str(), "values": { param.as_str(): json } });
        match &spec.kind {
            ParamKind::Number { .. } => drag(
                "midi_tool.set",
                set(json!(value(self)?)),
                descriptor.name.clone(),
            ),
            ParamKind::Whole { .. } => drag(
                "midi_tool.set",
                set(json!(value(self)?.round() as i64)),
                descriptor.name.clone(),
            ),
            ParamKind::Switch => cmd("midi_tool.set", set(json!(want(current)?))),
            ParamKind::Choice { options } => {
                let at = option(u32::try_from(options.len()).ok()?)? as usize;
                cmd("midi_tool.set", set(json!(options.get(at)?.id)))
            }
            _ => None,
        }
    }

    /// What a press or a release of one of the crossfader's fields runs
    /// (map modes R19).
    fn field_command(&self, field: CrossfaderField, action: Action) -> Option<Run> {
        let doc = self.editor.document();
        let mapped = |of: CrossfaderField| {
            doc.settings()
                .remote
                .iter()
                .any(|m| m.target == RemoteTarget::CrossfaderField { field: of })
        };
        let fields = [
            CrossfaderField::Left,
            CrossfaderField::Centre,
            CrossfaderField::Right,
        ];
        let several = fields.iter().filter(|f| mapped(**f)).count() > 1;
        let to = match action {
            Action::Press(_) if !several => {
                // One mapped field toggles between the two ends.
                if doc.settings().crossfader <= 0.0 {
                    1.0
                } else {
                    0.0
                }
            }
            Action::Press(_) => field.position(),
            // A release moves the crossfader back to the field still held.
            _ => match self.remote.fields_held.last() {
                Some(held) if several => held.position(),
                _ => return nothing(),
            },
        };
        cmd("mixer.set_crossfader", json!({ "position": to }))
    }

    /// Notes a press or a release of a crossfader field, for the fields that
    /// are held.
    pub(super) fn hold_field(&mut self, field: CrossfaderField, action: Action) {
        self.remote.fields_held.retain(|held| *held != field);
        if matches!(action, Action::Press(_)) {
            self.remote.fields_held.push(field);
        }
    }

    /// What a message runs on a Control Bar control (map modes R11).
    fn bar_command(
        &self,
        control: BarControl,
        current: Option<f64>,
        action: Action,
        want: &dyn Fn(Option<f64>) -> Option<bool>,
        option: &dyn Fn(u32) -> Option<u32>,
        value: &dyn Fn(&Self) -> Option<f64>,
    ) -> Option<Run> {
        let pressed = matches!(action, Action::Press(_));
        let button = |id: &'static str| if pressed { cmd(id, json!({})) } else { None };
        // A switch whose command takes the state.
        let set = |id: &'static str, key: &str| {
            let on = want(current)?;
            cmd(id, json!({ key: on }))
        };
        // A switch whose command turns it over, run only to change it.
        let turn = |id: &'static str| {
            let on = want(current)?;
            if on == (current? >= 0.5) {
                nothing()
            } else {
                cmd(id, json!({}))
            }
        };
        match control {
            BarControl::TapTempo => button("transport.tap_tempo"),
            BarControl::Play => button("transport.play"),
            BarControl::Stop => button("transport.stop"),
            BarControl::ReEnableAutomation => button(bl_model::registry::RE_ENABLE_AUTOMATION),
            BarControl::CaptureMidi => button(bl_shell_spec::capture_midi::CAPTURE),
            BarControl::New => button("session.new_take"),
            BarControl::Tempo => {
                let invocation = crate::transport::set_tempo(self, value(self)?)?;
                drag("song.set_tempo", invocation.args, "Change tempo")
            }
            BarControl::Metronome => set("transport.set_metronome_enabled", "enabled"),
            BarControl::Follow => set("arrangement.follow", "on"),
            BarControl::ArrangementRecord => set("transport.set_recording", "recording"),
            BarControl::MidiOverdub => set("transport.set_midi_overdub", "enabled"),
            BarControl::AutomationArm => set("arrangement.automation_arm", "on"),
            BarControl::SessionRecord => turn("session.record"),
            BarControl::PunchIn => set("transport.set_punch_in", "enabled"),
            BarControl::PunchOut => set("transport.set_punch_out", "enabled"),
            BarControl::ArrangementLoop => set("transport.set_loop_enabled", "enabled"),
            BarControl::DrawMode => turn("view.draw_mode"),
            BarControl::Quantization => {
                let step = LaunchQuantization::ALL[option(bl_model::remote::QUANTIZATION_OPTIONS)?
                    as usize
                    % LaunchQuantization::ALL.len()];
                cmd("session.set_quantization", json!({ "quantization": step }))
            }
        }
    }

    /// What a message runs on a Clip View control (map modes R17): the
    /// control's command for each clip it acts on.
    fn clip_command(
        &self,
        control: ClipControl,
        current: Option<f64>,
        action: Action,
        want: &dyn Fn(Option<f64>) -> Option<bool>,
        option: &dyn Fn(u32) -> Option<u32>,
        value: &dyn Fn(&Self) -> Option<f64>,
    ) -> Option<Run> {
        let clips = self.clip_targets(control);
        let (first, rest) = clips.split_first()?;
        let clip_args = |clip: &ClipId, args: Value| {
            let mut args = args;
            args["clip"] = json!(clip);
            args
        };
        // The command with `args` for every clip, the gesture named for a
        // control that is dragged.
        let each = |id: &'static str, args: Value, gesture: Option<&'static str>| {
            let mut run = cmd(id, clip_args(first, args.clone()))?;
            run.gesture = gesture.map(str::to_owned);
            run.more = rest
                .iter()
                .map(|clip| clip_args(clip, args.clone()))
                .collect();
            Some(run)
        };
        let set = |id: &'static str, key: &str| each(id, json!({ key: want(current)? }), None);
        match control {
            ClipControl::Warp => set("clip.set_warp", "warp"),
            ClipControl::HighQuality => set("clip.set_high_quality", "high_quality"),
            ClipControl::Ram => set("clip.set_ram_mode", "ram"),
            ClipControl::Legato => set("clip.set_legato", "legato"),
            ClipControl::WarpMode => {
                let mode = WARP_MODES[option(bl_model::remote::WARP_MODE_OPTIONS)? as usize % 6];
                each("clip.set_warp_mode", json!({ "mode": mode }), None)
            }
            ClipControl::LaunchMode => {
                let mode =
                    LaunchMode::ALL[option(bl_model::remote::LAUNCH_MODE_OPTIONS)? as usize % 4];
                each("clip.set_launch_mode", json!({ "mode": mode }), None)
            }
            ClipControl::LaunchQuantization => {
                let at = option(bl_model::remote::QUANTIZATION_OPTIONS + 1)? as usize;
                let step = at
                    .checked_sub(1)
                    .map(|i| LaunchQuantization::ALL[i % LaunchQuantization::ALL.len()]);
                each(
                    "clip.set_launch_quantization",
                    json!({ "quantization": step }),
                    None,
                )
            }
            ClipControl::Gain => {
                let db = value(self)?;
                let gain = if db <= GAIN_FLOOR {
                    0.0
                } else {
                    10.0_f64.powf(db / 20.0)
                };
                each(
                    "clip.set_gain",
                    json!({ "gain": gain }),
                    Some("Change gain"),
                )
            }
            ClipControl::Transpose => each(
                "clip.set_pitch",
                json!({ "transpose": value(self)?.round() as i32 }),
                Some("Change transpose"),
            ),
            ClipControl::Detune => each(
                "clip.set_pitch",
                json!({ "detune": value(self)? }),
                Some("Change detune"),
            ),
            ClipControl::Velocity => each(
                "clip.set_velocity_amount",
                json!({ "amount": value(self)? }),
                Some("Change velocity"),
            ),
            ClipControl::NudgeDown | ClipControl::NudgeUp => {
                if !matches!(action, Action::Press(_)) {
                    return None;
                }
                let (track, scene) = self.slot_of(*first)?;
                let steps = if control == ClipControl::NudgeUp {
                    1
                } else {
                    -1
                };
                cmd(
                    "session.nudge",
                    json!({ "track": track, "scene": scene, "steps": steps }),
                )
            }
            ClipControl::Scrub => None,
        }
    }

    /// Nudges the playing Session clip by what the Scrub control's message
    /// says (map modes R22, launch quantisation R35): a relative
    /// controller's increment of `n` steps, `n` steps in an accelerated mode
    /// and one in a linear mode, an absolute controller's change from its
    /// last value, its first message nudging nothing. Whether it nudged.
    pub(super) fn nudge_by_scrub(&mut self, mapping: &RemoteMapping, action: Action) -> bool {
        let Action::Position(position) = action else {
            return false;
        };
        let relative = matches!(mapping.mode, bl_model::RemoteMode::Relative { .. });
        let share = 1.0 / 127.0;
        let steps = if relative {
            // The law moved the virtual position off the middle.
            ((position - 0.5) / share).round()
        } else {
            let last = self.remote.scrub_last.replace(position);
            last.map_or(0.0, |last| ((position - last) / share).round())
        };
        let steps = steps.clamp(-64.0, 64.0) as i32;
        let Some(clip) = self.clip_targets(ClipControl::NudgeUp).first().copied() else {
            return false;
        };
        let Some((track, scene)) = self.slot_of(clip) else {
            return false;
        };
        steps != 0
            && self
                .run(
                    "session.nudge",
                    json!({ "track": track, "scene": scene, "steps": steps }),
                )
                .is_ok()
    }

    /// The path to a control of this module and its name.
    pub(super) fn describe_other(&self, target: &RemoteTarget) -> (String, String) {
        let track = |id| {
            self.editor
                .document()
                .track(id)
                .map_or_else(String::new, |t| t.name().to_owned())
        };
        let session = |name: &str| ("Session".to_owned(), name.to_owned());
        match *target {
            RemoteTarget::Monitor { track: t } => (track(t), "Monitor".to_owned()),
            RemoteTarget::CueVolume => ("Main".to_owned(), "Cue Volume".to_owned()),
            RemoteTarget::TrackLaunch { track: t } => (track(t), "Launch".to_owned()),
            RemoteTarget::Strip { control } => session(match control {
                StripControl::SceneUp => "Scene Up",
                StripControl::SceneDown => "Scene Down",
                StripControl::SceneNumber => "Scene Number",
                StripControl::LaunchScene => "Launch Scene",
                StripControl::CancelLaunch => "Cancel Launch",
            }),
            RemoteTarget::CrossfaderField { field } => (
                "Main".to_owned(),
                match field {
                    CrossfaderField::Left => "Crossfader Left",
                    CrossfaderField::Centre => "Crossfader Centre",
                    CrossfaderField::Right => "Crossfader Right",
                }
                .to_owned(),
            ),
            RemoteTarget::Bar { control } => {
                ("Control Bar".to_owned(), bar_name(control).to_owned())
            }
            RemoteTarget::StepBack => ("Note Editor".to_owned(), "Step Back".to_owned()),
            RemoteTarget::StepForward => ("Note Editor".to_owned(), "Step Forward".to_owned()),
            RemoteTarget::Clip { control } => {
                ("Clip View".to_owned(), clip_name(control).to_owned())
            }
            RemoteTarget::MidiTool { tool, control } => {
                let descriptor = Registry::native()
                    .find(tool.as_str())
                    .map(bl_midi_tools::MidiTool::descriptor);
                let named = descriptor.map_or_else(|| tool.as_str().to_owned(), |d| d.name.clone());
                let what = match control {
                    ToolControl::Auto => "Auto".to_owned(),
                    ToolControl::Reset => "Reset".to_owned(),
                    ToolControl::Apply => match descriptor.map(|d| d.kind) {
                        Some(ToolKind::Generator) => "Generate".to_owned(),
                        _ => "Transform".to_owned(),
                    },
                    ToolControl::Param { param } => self.tool_param(target).map_or_else(
                        || param.as_str().to_owned(),
                        |(_, _, spec)| spec.name.clone(),
                    ),
                };
                ("MIDI Tools".to_owned(), format!("{named}: {what}"))
            }
            _ => (String::new(), String::new()),
        }
    }

    /// Every control the set has that `mode` maps (map modes R24): what each
    /// view shows, a control that shows in several places once.
    pub(super) fn remote_targets(&self, mode: Option<MapMode>) -> TargetsResult {
        let mode = match mode.unwrap_or(self.remote.map.mode) {
            MapMode::Off => MapMode::Midi,
            mode => mode,
        };
        let doc = self.editor.document();
        let mut targets = Vec::new();
        let mut add = |this: &Self, target: RemoteTarget| {
            let Some(kind) = this.kind_of(&target) else {
                return;
            };
            if mode == MapMode::Key && kind == ControlKind::Continuous {
                return;
            }
            let (path, name) = this.describe(&target);
            targets.push(TargetInfo {
                target,
                kind,
                path,
                name,
                range: this.units_for(&target, kind),
            });
        };
        let scenes: Vec<_> = doc.scenes().map(|scene| scene.id).collect();
        for scene in &scenes {
            add(self, RemoteTarget::SceneLaunch { scene: *scene });
        }
        add(self, RemoteTarget::StopAll);
        for control in [
            StripControl::SceneUp,
            StripControl::SceneDown,
            StripControl::SceneNumber,
            StripControl::LaunchScene,
            StripControl::CancelLaunch,
        ] {
            add(self, RemoteTarget::Strip { control });
        }
        for track in doc.tracks() {
            let id = track.id();
            let kind = track.kind();
            if kind.holds_clips() {
                for scene in &scenes {
                    add(
                        self,
                        RemoteTarget::Slot {
                            track: id,
                            scene: *scene,
                        },
                    );
                }
                add(self, RemoteTarget::TrackStop { track: id });
                add(self, RemoteTarget::TrackLaunch { track: id });
            }
            for param in [bl_model::MixerParam::Volume, bl_model::MixerParam::Pan] {
                add(self, RemoteTarget::Mixer { track: id, param });
            }
            if track.mixer().pan_mode == bl_model::PanMode::Split {
                for param in [
                    bl_model::MixerParam::SplitLeft,
                    bl_model::MixerParam::SplitRight,
                ] {
                    add(self, RemoteTarget::Mixer { track: id, param });
                }
            }
            if kind.has_mute_and_solo() {
                add(
                    self,
                    RemoteTarget::Mixer {
                        track: id,
                        param: bl_model::MixerParam::Mute,
                    },
                );
                add(self, RemoteTarget::Solo { track: id });
            }
            if kind.has_arm() {
                add(self, RemoteTarget::Arm { track: id });
                add(self, RemoteTarget::Monitor { track: id });
            }
            if kind.has_crossfade_assign() {
                add(self, RemoteTarget::CrossfadeAssign { track: id });
            }
            if kind.has_sends() {
                for to in doc.returns() {
                    add(self, RemoteTarget::Send { track: id, to });
                }
            }
            if kind == TrackKind::Main {
                add(self, RemoteTarget::CueVolume);
                add(self, RemoteTarget::Crossfader);
                for field in [
                    CrossfaderField::Left,
                    CrossfaderField::Centre,
                    CrossfaderField::Right,
                ] {
                    add(self, RemoteTarget::CrossfaderField { field });
                }
            }
            for device in track.devices_deep() {
                add(self, RemoteTarget::DeviceOn { device: device.id });
                if let Some(description) = self.devices.description(&device.kind) {
                    for param in &description.params {
                        // Spec: macros R35. A parameter a macro holds has
                        // no control to map.
                        if self
                            .devices
                            .macro_holding(doc, device.id, param.id)
                            .is_none()
                        {
                            add(
                                self,
                                RemoteTarget::Device {
                                    device: device.id,
                                    param: param.id,
                                },
                            );
                        }
                    }
                }
                for chain in &device.chains {
                    for param in [bl_model::MixerParam::Volume, bl_model::MixerParam::Pan] {
                        add(
                            self,
                            RemoteTarget::Chain {
                                chain: chain.id,
                                param,
                            },
                        );
                    }
                }
            }
        }
        for control in ALL_BAR {
            add(self, RemoteTarget::Bar { control });
        }
        add(self, RemoteTarget::StepBack);
        add(self, RemoteTarget::StepForward);
        for control in ALL_CLIP {
            // The scrub shows in MIDI Map Mode only (map modes R22).
            if control != ClipControl::Scrub || mode == MapMode::Midi {
                add(self, RemoteTarget::Clip { control });
            }
        }
        // Spec: map-modes R23. Every native tool's Auto switch, apply button,
        // Reset button and the parameters that have a kind to map.
        for tool in Registry::native()
            .tools(ToolKind::Transformation)
            .chain(Registry::native().tools(ToolKind::Generator))
        {
            let descriptor = tool.descriptor();
            let Some(id) = ToolId::new(&descriptor.id) else {
                continue;
            };
            for control in [ToolControl::Auto, ToolControl::Apply, ToolControl::Reset] {
                add(self, RemoteTarget::MidiTool { tool: id, control });
            }
            for spec in &descriptor.parameters {
                if let Some((tool, param)) = tool_ids(descriptor, spec) {
                    let control = ToolControl::Param { param };
                    add(self, RemoteTarget::MidiTool { tool, control });
                }
            }
        }
        TargetsResult { mode, targets }
    }
}

/// Every Control Bar control a map mode maps.
const ALL_BAR: [BarControl; 18] = [
    BarControl::TapTempo,
    BarControl::Tempo,
    BarControl::Metronome,
    BarControl::Quantization,
    BarControl::Follow,
    BarControl::Play,
    BarControl::Stop,
    BarControl::ArrangementRecord,
    BarControl::MidiOverdub,
    BarControl::AutomationArm,
    BarControl::ReEnableAutomation,
    BarControl::CaptureMidi,
    BarControl::New,
    BarControl::SessionRecord,
    BarControl::PunchIn,
    BarControl::ArrangementLoop,
    BarControl::PunchOut,
    BarControl::DrawMode,
];

/// Every Clip View control a map mode maps.
const ALL_CLIP: [ClipControl; 14] = [
    ClipControl::Warp,
    ClipControl::WarpMode,
    ClipControl::HighQuality,
    ClipControl::Ram,
    ClipControl::Gain,
    ClipControl::Transpose,
    ClipControl::Detune,
    ClipControl::LaunchMode,
    ClipControl::Legato,
    ClipControl::LaunchQuantization,
    ClipControl::Velocity,
    ClipControl::NudgeDown,
    ClipControl::NudgeUp,
    ClipControl::Scrub,
];

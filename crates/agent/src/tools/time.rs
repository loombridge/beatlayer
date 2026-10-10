//! Transport and time: the transport's commands, and commands scheduled at
//! a bar and beat.
//!
//! Agents can't hit musical timing over a network, so they schedule
//! instead. A scheduled command reaches the engine as a
//! [`ScheduledCommand`] (WP-0.9): the engine runs its action on the exact
//! frame the song reaches its position (a fader or device parameter change
//! there and then; for any other command, nothing but the report) and
//! reports it, and the main thread then runs the command on the document
//! as an undo step labelled with the agent's name, so the document matches
//! what played.
//!
//! The agent API doesn't own an engine. It queues what the engine must
//! hear as [`EngineMessage`]s, which the host takes with
//! [`Agent::take_engine_messages`](crate::Agent::take_engine_messages)
//! and sends on, and the host hands back the engine's reports with
//! [`Agent::on_report`](crate::Agent::on_report). Until the app plays the
//! song through the engine (the transport, WP-1.5), nothing takes them, and
//! scheduled commands wait.

use bl_commands::Permission;
use bl_core::time::Ticks;
use bl_engine::nodes::fader;
use bl_engine::{Action, NodeKey, ParamValue, Report, ScheduledCommand};
use bl_model::registry as model;
use bl_shell::Shell;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::run::resolve_placeholders;
use super::{Ctx, ToolOutcome, args, describe_args, done, failed, step, tool_definition};

pub(crate) const TRANSPORT: &str = "transport";
pub(crate) const SCHEDULE: &str = "schedule";

/// The most scheduled commands waiting at once, as the engine holds
/// (`EngineConfig::max_scheduled`).
pub(crate) const MAX_WAITING: usize = 256;

/// What the engine must hear about scheduled commands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EngineMessage {
    /// Schedule this command (send it with `Sender::schedule`).
    Schedule(ScheduledCommand),
    /// Cancel the scheduled command with this ID (`Sender::cancel`).
    Cancel(u64),
}

/// What `transport` does.
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TransportAction {
    /// Report whether the song can play here and which transport commands
    /// this build has.
    Status,
    /// Start playing (`transport.play`).
    Play,
    /// Stop (`transport.stop`).
    Stop,
    /// Carry on from where it stopped (`transport.continue`).
    Continue,
    /// Move the song position (`transport.locate`).
    Locate,
    /// Set or switch the loop (`transport.loop`).
    Loop,
}

impl TransportAction {
    fn command(self) -> Option<&'static str> {
        Some(match self {
            Self::Status => return None,
            Self::Play => "transport.play",
            Self::Stop => "transport.stop",
            Self::Continue => "transport.continue",
            Self::Locate => "transport.locate",
            Self::Loop => "transport.loop",
        })
    }
}

/// Arguments of `transport`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TransportArgs {
    /// What to do.
    action: TransportAction,
    /// The transport command's arguments, as `describe_command` gives them.
    #[serde(default)]
    #[schemars(with = "Option<Map<String, Value>>")]
    args: Option<Value>,
}

/// What `schedule` does.
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ScheduleAction {
    /// Schedule `command` with `args` at `at`.
    Add,
    /// List scheduled commands: waiting, fired, and cancelled.
    List,
    /// Cancel the waiting command with ID `id`.
    Cancel,
}

/// Arguments of `schedule`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScheduleArgs {
    /// What to do.
    action: ScheduleAction,
    /// For `add`: where in the song it runs, as bar.beat.sixteenth by the
    /// song's time signatures, such as `5.1.1` (the downbeat of bar 5) or
    /// `2.3` (bar 2, beat 3).
    #[serde(default)]
    at: Option<String>,
    /// For `add`: the command's ID, such as `mixer.set_mute`.
    #[serde(default)]
    command: Option<String>,
    /// For `add`: the command's arguments. `{"$ref": "name"}` refers to an
    /// ID a `run` call issued.
    #[serde(default)]
    #[schemars(with = "Option<Map<String, Value>>")]
    args: Option<Value>,
    /// For `cancel`: the scheduled command's ID, from `add` or `list`.
    #[serde(default)]
    id: Option<u64>,
}

pub(crate) fn transport_definition() -> Value {
    tool_definition::<TransportArgs>(
        TRANSPORT,
        "Transport",
        "Plays, stops, continues, locates, and loops the song through the transport's \
         commands, and reports with `status` whether the song plays here. To make something \
         happen at an exact musical moment, use `schedule` rather than timing calls yourself.",
        false,
    )
}

pub(crate) fn schedule_definition() -> Value {
    tool_definition::<ScheduleArgs>(
        SCHEDULE,
        "Schedule commands",
        "Runs a command when the playing song reaches a bar and beat, on the exact sample: \
         `add` with `at` (bar.beat.sixteenth, such as \"9.1.1\"), `command`, and `args`; \
         `list` to see them; `cancel` with `id`. Fader and device parameter changes \
         (`mixer.set_volume`, `mixer.set_pan`, `mixer.set_mute`, `mixer.set_solo`, \
         `device.set_param`) take effect in the audio on that sample; any other command runs \
         as the song gets there. Each runs as an undo step labelled with your name. The \
         command and its arguments are checked when you schedule it, and it asks the person \
         then if it's destructive or external. A command fires once, when the song plays \
         through its position.",
        false,
    )
}

pub(crate) fn transport(ctx: &mut Ctx<'_>, raw: Value) -> ToolOutcome {
    let TransportArgs {
        action,
        args: given,
    } = step!(args(raw));
    let available: Vec<String> = ctx
        .shell
        .registry()
        .iter()
        .filter(|spec| spec.area() == "transport")
        .map(|spec| spec.id.to_string())
        .collect();
    let Some(command) = action.command() else {
        return done(json!({
            "engine_plays_here": ctx.state.engine_attached,
            "transport_commands": available,
            "scheduled_waiting": ctx.state.schedule.waiting(),
            "note": if available.is_empty() {
                "This build has no transport commands yet: play, stop, continue, locate, and \
                 the loop come with the transport (WP-1.5). Scheduled commands wait until the \
                 song plays."
            } else {
                "Run the transport's commands through this tool or `run`."
            },
        }));
    };
    if !available.iter().any(|id| id == command) {
        return failed(format!(
            "this build has no `{command}` command yet: play, stop, continue, locate, and the \
             loop come with the transport (WP-1.5). Until then, `schedule` commands at bars \
             and beats; they fire when the song plays."
        ));
    }
    super::run::run(
        ctx,
        json!({ "commands": [{ "command": command, "args": given.unwrap_or_else(|| json!({})) }] }),
    )
}

/// One scheduled command.
#[derive(Clone, Debug)]
struct Entry {
    id: u64,
    at: Ticks,
    position: String,
    command: String,
    args: Value,
    label: String,
    state: State,
}

#[derive(Clone, Debug, PartialEq)]
enum State {
    Waiting,
    Fired { frame: i64, error: Option<String> },
    Cancelled,
    Rejected,
}

/// The scheduled commands, and what the engine has yet to hear.
#[derive(Debug, Default)]
pub(crate) struct Schedule {
    entries: Vec<Entry>,
    next_id: u64,
    outbox: Vec<EngineMessage>,
}

impl Schedule {
    /// How many are waiting to fire.
    pub(crate) fn waiting(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.state == State::Waiting)
            .count()
    }

    pub(crate) fn take_messages(&mut self) -> Vec<EngineMessage> {
        std::mem::take(&mut self.outbox)
    }

    fn list(&self) -> Value {
        let entries: Vec<Value> = self
            .entries
            .iter()
            .map(|e| {
                let (state, detail) = match &e.state {
                    State::Waiting => ("waiting", Value::Null),
                    State::Fired { frame, error } => (
                        if error.is_some() { "failed" } else { "fired" },
                        json!({ "song_frame": frame, "error": error }),
                    ),
                    State::Cancelled => ("cancelled", Value::Null),
                    State::Rejected => (
                        "rejected",
                        json!("the engine held as many scheduled commands as it can"),
                    ),
                };
                json!({
                    "id": e.id,
                    "at": e.position,
                    "ticks": e.at.get(),
                    "command": e.command,
                    "args": e.args,
                    "state": state,
                    "detail": detail,
                })
            })
            .collect();
        json!({ "scheduled": entries })
    }

    /// Runs the command whose scheduled ID fired, on the document, as the
    /// agent's undo step.
    pub(crate) fn on_report(&mut self, shell: &mut Shell, report: Report) {
        match report {
            Report::Fired { id, position, .. } => {
                let Some(entry) = self
                    .entries
                    .iter_mut()
                    .find(|e| e.id == id && e.state == State::Waiting)
                else {
                    return;
                };
                let error = run_now(shell, entry).err();
                entry.state = State::Fired {
                    frame: position.get(),
                    error,
                };
            }
            Report::Rejected { id } => {
                if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
                    entry.state = State::Rejected;
                }
            }
        }
    }
}

/// Runs a fired command.
fn run_now(shell: &mut Shell, entry: &Entry) -> Result<(), String> {
    if is_edit(&entry.command) {
        let label = entry.label.clone();
        shell.edit(|editor| {
            editor
                .transact(label, |editor| {
                    model::run(editor, &entry.command, entry.args.clone())
                })
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    } else {
        shell
            .run(&entry.command, entry.args.clone())
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

fn is_edit(id: &str) -> bool {
    model::handles(id) && id != model::UNDO && id != model::REDO
}

/// What the engine does on the frame: a fader or device parameter change,
/// or only the report.
fn engine_action(command: &str, args: &Value) -> Action {
    let id = |key: &str| {
        args.get(key)
            .and_then(Value::as_u64)
            .and_then(bl_core::id::Id::new)
    };
    let number = |key: &str| args.get(key).and_then(Value::as_f64);
    let switch = |key: &str| {
        args.get(key)
            .and_then(Value::as_bool)
            .map(|on| if on { 1.0 } else { 0.0 })
    };
    let fader = |param, value: Option<f64>| match (id("track"), value) {
        (Some(track), Some(value)) => Action::Param {
            key: NodeKey::Fader(bl_model::TrackId(track)),
            param,
            value: ParamValue::Set(value),
        },
        _ => Action::Notify,
    };
    match command {
        "mixer.set_volume" => fader(fader::VOLUME, number("volume")),
        "mixer.set_pan" => fader(fader::PAN, number("pan")),
        "mixer.set_mute" => fader(fader::MUTE, switch("mute")),
        "mixer.set_solo" => fader(fader::SOLO, switch("solo")),
        "device.set_param" => match (
            id("device"),
            args.get("param")
                .and_then(Value::as_u64)
                .and_then(|p| u32::try_from(p).ok()),
            number("value"),
        ) {
            (Some(device), Some(param), Some(value)) => Action::Param {
                key: NodeKey::Device(bl_model::DeviceId(device)),
                param: bl_model::ParamId(param),
                value: ParamValue::Set(value),
            },
            _ => Action::Notify,
        },
        _ => Action::Notify,
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "each check before scheduling reads best in order"
)]
pub(crate) fn schedule(ctx: &mut Ctx<'_>, raw: Value) -> ToolOutcome {
    let ScheduleArgs {
        action,
        at,
        command,
        args: given,
        id,
    } = step!(args(raw));
    match action {
        ScheduleAction::List => done(ctx.state.schedule.list()),
        ScheduleAction::Cancel => {
            let Some(id) = id else {
                return failed("give the `id` of the scheduled command to cancel");
            };
            let schedule = &mut ctx.state.schedule;
            let Some(entry) = schedule
                .entries
                .iter_mut()
                .find(|e| e.id == id && e.state == State::Waiting)
            else {
                return failed(format!("no scheduled command {id} is waiting"));
            };
            entry.state = State::Cancelled;
            schedule.outbox.push(EngineMessage::Cancel(id));
            done(json!({ "cancelled": id }))
        }
        ScheduleAction::Add => {
            let (Some(at), Some(command)) = (at, command) else {
                return failed("`add` needs `at` (such as \"5.1.1\") and `command`");
            };
            let document = ctx.shell.editor().document();
            let signatures = &document.settings().time_signatures;
            let ticks = match signatures.parse_position(&at) {
                Ok(ticks) => ticks,
                Err(error) => {
                    return failed(format!(
                        "`{at}` isn't a position in this song: {error}. Write bar.beat.sixteenth, \
                         such as 5.1.1"
                    ));
                }
            };
            if ticks < Ticks::ZERO {
                return failed(format!(
                    "`{at}` is before the song starts; scheduled commands fire from 1.1.1 on"
                ));
            }
            let position = signatures.to_bbs(ticks).to_string();
            let Some(spec) = ctx.shell.registry().get(&command) else {
                return failed(format!(
                    "no command is called `{command}`; use list_commands to find one"
                ));
            };
            let class = spec.permission.unwrap_or(Permission::External);
            let schema = spec.args.clone();
            let label = spec.label.to_string();
            if ctx.state.schedule.waiting() >= MAX_WAITING {
                return failed(format!(
                    "{MAX_WAITING} scheduled commands are waiting, as many as the engine holds; \
                     cancel some first"
                ));
            }
            let mut args = given.unwrap_or_else(|| json!({}));
            let mut names = ctx.session.refs.clone();
            let shell = &mut *ctx.shell;
            let mut issue = || shell.new_id::<bl_core::id::Id>().get();
            if let Err(message) = resolve_placeholders(&mut args, &mut names, &mut issue) {
                return failed(message);
            }
            if let Some(schema) = &schema
                && let Err(message) = bl_commands::validate(schema, &args)
            {
                return failed(format!("invalid arguments for `{command}`: {message}"));
            }
            // Try it on the song as it is now, so a mistake shows today
            // rather than when the song gets there.
            if is_edit(&command) {
                let tried = ctx.shell.edit(|editor| {
                    editor.begin("Try a scheduled command");
                    let result = model::run(editor, &command, args.clone());
                    editor.cancel().ok();
                    result
                });
                if let Err(error) = tried {
                    return failed(format!(
                        "`{command}` fails on the song as it is now: {error}"
                    ));
                }
            }
            let shell = &*ctx.shell;
            step!(ctx.permit(class, &[command.as_str()], || format!(
                "At {position}: {label} ({class}): {}",
                describe_args(shell, &args)
            )));
            ctx.session.refs = names;
            let schedule = &mut ctx.state.schedule;
            schedule.next_id += 1;
            let id = schedule.next_id;
            let entry = Entry {
                id,
                at: ticks,
                position: position.clone(),
                command: command.clone(),
                args: args.clone(),
                label: format!("{}: {label} at {position}", ctx.session.client.label()),
                state: State::Waiting,
            };
            let action = engine_action(&command, &args);
            schedule
                .outbox
                .push(EngineMessage::Schedule(ScheduledCommand {
                    id,
                    at: ticks,
                    action,
                }));
            schedule.entries.push(entry);
            done(json!({
                "id": id,
                "at": position,
                "ticks": ticks.get(),
                "command": command,
                "sample_accurate": !matches!(action, Action::Notify),
                "note": if ctx.state.engine_attached {
                    "It fires when the playing song reaches that position."
                } else {
                    "Nothing plays the song here yet (the transport comes with WP-1.5), so it \
                     waits until the song plays through that position."
                },
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fader_and_device_changes_become_engine_actions() {
        let action = engine_action("mixer.set_mute", &json!({ "track": 4, "mute": true }));
        assert_eq!(
            action,
            Action::Param {
                key: NodeKey::Fader(bl_model::TrackId(bl_core::id::Id::new(4).unwrap())),
                param: fader::MUTE,
                value: ParamValue::Set(1.0),
            }
        );
        let action = engine_action(
            "device.set_param",
            &json!({ "device": 9, "param": 1, "value": -36.0 }),
        );
        assert!(matches!(action, Action::Param { value: ParamValue::Set(v), .. } if v == -36.0));
        assert_eq!(
            engine_action("track.rename", &json!({ "track": 4, "name": "A" })),
            Action::Notify
        );
    }
}

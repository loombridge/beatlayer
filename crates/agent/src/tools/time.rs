//! Transport and time: the transport's commands, and commands scheduled at
//! a bar and beat.
//!
//! The transport's playback commands (`transport.play`, `transport.stop`,
//! `transport.continue`, `transport.locate`, and the rest that
//! `bl_engine::transport` registers, WP-1.5) act in the engine, not on the
//! document. The agent API turns each into the engine's
//! [`TransportCommand`] and queues it as an [`EngineMessage::Transport`];
//! the host puts it on the engine's one queue, the queue the UI's transport
//! commands take, in order with programs and parameter changes. So agents
//! never touch the audio thread, and the engine acts at the start of its
//! next block. The loop brace and its switch (`transport.set_loop`,
//! `transport.set_loop_enabled`) belong to the set: they're document edits
//! that run on the shell like any other, and reach the engine when the host
//! syncs the document.
//!
//! Agents can't hit musical timing over a network, so they schedule
//! instead. A scheduled command reaches the engine as a
//! [`ScheduledCommand`] (WP-0.9): the engine runs its action on the exact
//! frame the song reaches its position (a track's volume, pan, or mute, a
//! device parameter, or a transport stop or jump, there and then; for any
//! other command, solo among them, nothing but the report) and reports it, and the main thread then runs a
//! document command on the document as an undo step labelled with the
//! agent's name, so the document matches what played.
//!
//! The agent API doesn't own an engine. It queues what the engine must
//! hear as [`EngineMessage`]s, which the host takes with
//! [`Agent::take_engine_messages`](crate::Agent::take_engine_messages)
//! and sends on, and the host hands back the engine's reports with
//! [`Agent::on_report`](crate::Agent::on_report) and its state with
//! [`Agent::on_engine_state`](crate::Agent::on_engine_state). A host that
//! plays the song through an engine says so with
//! [`Agent::set_engine_attached`](crate::Agent::set_engine_attached).
//! Without one (`bl-cli mcp` has none), the playback commands are refused,
//! saying why, and scheduled commands wait.

use bl_commands::Permission;
use bl_core::time::Ticks;
use bl_engine::{
    Action, EngineState, NodeKey, ParamValue, Report, ScheduledCommand, TransportCommand, mixer,
    transport,
};
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

/// What the engine must hear from the agent API. The host sends each on
/// the engine's queue, in order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EngineMessage {
    /// Run this transport command at the start of the next block (send it
    /// with `Sender::transport` or `Engine::transport`, as the UI's
    /// transport commands are sent).
    Transport(TransportCommand),
    /// Schedule this command (send it with `Sender::schedule`).
    Schedule(ScheduledCommand),
    /// Cancel the scheduled command with this ID (`Sender::cancel`).
    Cancel(u64),
}

/// Where the engine last said the transport was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Playhead {
    playing: bool,
    position: Ticks,
    insert_marker: Ticks,
}

impl Playhead {
    pub(crate) fn of(state: &EngineState) -> Self {
        Self {
            playing: state.playing,
            position: state.position_ticks,
            insert_marker: state.insert_marker,
        }
    }
}

/// What `transport` does. Each action but `status` runs the transport
/// command of the same name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TransportAction {
    /// Report whether the song plays here, where the playhead and the
    /// insert marker are, the loop, and the transport's commands.
    Status,
    /// Start playing from the insert marker (`transport.play`).
    Play,
    /// Stop, keeping the position; while stopped, return to 1.1.1
    /// (`transport.stop`).
    Stop,
    /// Play on from where playback stopped (`transport.continue`).
    Continue,
    /// Move the insert marker, and while playing the playhead, to `at`
    /// (bar.beat.sixteenth) or `args.position` in ticks
    /// (`transport.locate`).
    Locate,
    /// Move the insert marker to the playhead
    /// (`transport.insert_marker_to_playhead`).
    InsertMarkerToPlayhead,
    /// Move the loop brace: `args.start` and `args.length`, in ticks
    /// (`transport.set_loop`). An undo step.
    SetLoop,
    /// Switch the loop on or off: `args.enabled` (`transport.set_loop_enabled`).
    /// An undo step.
    SetLoopEnabled,
}

impl TransportAction {
    fn command(self) -> Option<&'static str> {
        Some(match self {
            Self::Status => return None,
            Self::Play => "transport.play",
            Self::Stop => "transport.stop",
            Self::Continue => "transport.continue",
            Self::Locate => "transport.locate",
            Self::InsertMarkerToPlayhead => "transport.insert_marker_to_playhead",
            Self::SetLoop => "transport.set_loop",
            Self::SetLoopEnabled => "transport.set_loop_enabled",
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
    /// For `locate`: where to, as bar.beat.sixteenth by the song's time
    /// signatures, such as `5.1.1`, instead of `args.position` in ticks.
    #[serde(default)]
    at: Option<String>,
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
        "Plays, stops, continues, and locates the song through the transport's commands, and \
         moves and switches the Arrangement loop. `play` starts from the insert marker; \
         `locate` moves it (with `at`, such as \"5.1.1\"), and jumps there while playing; \
         `stop` stops, and a second `stop` returns to 1.1.1; `continue` plays on from where \
         playback stopped. `set_loop` (`args.start`, `args.length` in ticks) and \
         `set_loop_enabled` (`args.enabled`) change the set, as undo steps. `status` says \
         whether the song plays here, where the playhead and the insert marker are, and the \
         loop. The engine acts at the start of its next block; to make something happen at \
         an exact musical moment, use `schedule` rather than timing calls yourself.",
        false,
    )
}

pub(crate) fn schedule_definition() -> Value {
    tool_definition::<ScheduleArgs>(
        SCHEDULE,
        "Schedule commands",
        "Runs a command when the playing song reaches a bar and beat, on the exact sample: \
         `add` with `at` (bar.beat.sixteenth, such as \"9.1.1\"), `command`, and `args`; \
         `list` to see them; `cancel` with `id`. Volume, pan, mute, and device parameter \
         changes (`mixer.set_volume`, `mixer.set_pan`, `mixer.set_mute`, `device.set_param`) \
         take effect in the audio on that sample, and `transport.stop` and `transport.locate` \
         stop or jump there; any other command, `mixer.set_solo` among them, runs as the song \
         gets there. Each document change runs as an undo step labelled with your name. The \
         command and its arguments are checked when you schedule it, and it asks the person \
         then if it's destructive or external. A command fires once, when the song plays \
         through its position.",
        false,
    )
}

/// `text`, a bar.beat.sixteenth position, in ticks by the song's time
/// signatures.
fn position(shell: &Shell, text: &str) -> Result<Ticks, String> {
    shell
        .editor()
        .document()
        .settings()
        .time_signatures
        .parse_position(text)
        .map_err(|error| {
            format!(
                "`{text}` isn't a position in this song: {error}. Write bar.beat.sixteenth, \
                 such as 5.1.1"
            )
        })
}

pub(crate) fn transport(ctx: &mut Ctx<'_>, raw: Value) -> ToolOutcome {
    let TransportArgs {
        action,
        args: given,
        at,
    } = step!(args(raw));
    let Some(command) = action.command() else {
        return done(status(ctx));
    };
    let mut given = given.unwrap_or_else(|| json!({}));
    if let Some(at) = at {
        if action != TransportAction::Locate {
            return failed("`at` is for `locate`; give other actions their `args`");
        }
        if given.as_object().is_none_or(|args| !args.is_empty()) {
            return failed("give `locate` either `at` or `args.position`, not both");
        }
        let ticks = match position(ctx.shell, &at) {
            Ok(ticks) => ticks,
            Err(message) => return failed(message),
        };
        given = json!({ "position": ticks.get() });
    }
    super::run::run(
        ctx,
        json!({ "commands": [{ "command": command, "args": given }] }),
    )
}

/// What `transport`'s `status` reports.
fn status(ctx: &Ctx<'_>) -> Value {
    let document = ctx.shell.editor().document();
    let settings = document.settings();
    let signatures = &settings.time_signatures;
    let at =
        |ticks: Ticks| json!({ "bbs": signatures.to_bbs(ticks).to_string(), "ticks": ticks.get() });
    let brace = settings.arrangement_loop;
    let mut available: Vec<&str> = super::commands(ctx.shell)
        .filter(|spec| spec.area() == "transport")
        .map(|spec| spec.id.as_ref())
        .collect();
    available.sort_unstable();
    let attached = ctx.state.engine_attached;
    let playhead = ctx.state.playhead.filter(|_| attached);
    json!({
        "engine_plays_here": attached,
        "playing": playhead.map(|p| p.playing),
        "position": playhead.map(|p| at(p.position)),
        "insert_marker": playhead.map(|p| at(p.insert_marker)),
        "loop": {
            "enabled": brace.enabled,
            "start": at(brace.start),
            "end": at(brace.end()),
            "length": brace.length.get(),
        },
        "transport_commands": available,
        "scheduled_waiting": ctx.state.schedule.waiting(),
        "note": if attached {
            "Play, stop, continue, and locate go to the engine, which acts on them at the \
             start of its next block; `position` is where it last said the playhead was. The \
             loop belongs to the set: `set_loop` and `set_loop_enabled` change it as undo \
             steps. To act on an exact beat, `schedule` the command."
        } else {
            "Nothing plays the song here: no engine is attached to this agent server \
             (`bl-cli mcp` runs without one), so play, stop, continue, and locate are \
             refused. The loop belongs to the set and can be changed here. Scheduled commands \
             wait until the song plays."
        },
    })
}

/// Sends one of the transport's playback commands to the engine, through
/// the host, which puts it on the engine's queue as the UI's transport
/// commands go: the agent never touches the audio thread.
pub(crate) fn send_transport(ctx: &mut Ctx<'_>, id: &str, args: Value) -> Result<(), String> {
    if !ctx.state.engine_attached {
        return Err(format!(
            "nothing plays the song here: no engine is attached to this agent server \
             (`bl-cli mcp` runs without one), so `{id}` can't move the transport. The loop \
             (`transport.set_loop`, `transport.set_loop_enabled`) belongs to the set and can \
             be changed anywhere, and `schedule` commands wait until the song plays"
        ));
    }
    let command = transport::command_from_args(id, args).map_err(|error| error.to_string())?;
    ctx.state.outbox.push(EngineMessage::Transport(command));
    Ok(())
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

/// The scheduled commands.
#[derive(Debug, Default)]
pub(crate) struct Schedule {
    entries: Vec<Entry>,
    next_id: u64,
}

impl Schedule {
    /// How many are waiting to fire.
    pub(crate) fn waiting(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.state == State::Waiting)
            .count()
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
    /// agent's undo step. A transport command already ran in the engine,
    /// on its frame.
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
    if transport::handles(&entry.command) {
        // The engine ran it on its frame, and it changes nothing in the
        // document.
        return Ok(());
    }
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

/// What the engine does on the frame: a track mixer or device parameter
/// change, a transport command, or only the report. Solo isn't one parameter
/// (it decides which routes every track is heard through), so a scheduled
/// solo is only reported, and the main thread applies it as the song gets
/// there.
fn engine_action(command: &str, args: &Value) -> Action {
    if transport::handles(command) {
        return transport::command_from_args(command, args.clone())
            .map_or(Action::Notify, Action::Transport);
    }
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
    let track_mixer = |param, value: Option<f64>| match (id("track"), value) {
        (Some(track), Some(value)) => Action::Param {
            key: NodeKey::Mixer(bl_model::TrackId(track)),
            param,
            value: ParamValue::Set(value),
        },
        _ => Action::Notify,
    };
    match command {
        "mixer.set_volume" => track_mixer(mixer::track::VOLUME, number("volume")),
        "mixer.set_pan" => track_mixer(mixer::track::PAN, number("pan")),
        "mixer.set_mute" => track_mixer(mixer::track::MUTE, switch("mute")),
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
            let Some(entry) = ctx
                .state
                .schedule
                .entries
                .iter_mut()
                .find(|e| e.id == id && e.state == State::Waiting)
            else {
                return failed(format!("no scheduled command {id} is waiting"));
            };
            entry.state = State::Cancelled;
            ctx.state.outbox.push(EngineMessage::Cancel(id));
            done(json!({ "cancelled": id }))
        }
        ScheduleAction::Add => {
            let (Some(at), Some(command)) = (at, command) else {
                return failed("`add` needs `at` (such as \"5.1.1\") and `command`");
            };
            let ticks = match position(ctx.shell, &at) {
                Ok(ticks) => ticks,
                Err(message) => return failed(message),
            };
            if ticks < Ticks::ZERO {
                return failed(format!(
                    "`{at}` is before the song starts; scheduled commands fire from 1.1.1 on"
                ));
            }
            let position = ctx
                .shell
                .editor()
                .document()
                .settings()
                .time_signatures
                .to_bbs(ticks)
                .to_string();
            let Some(spec) = super::command(ctx.shell, &command) else {
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
            let action = engine_action(&command, &args);
            if let Action::Transport(TransportCommand::Play | TransportCommand::Continue) = action {
                return failed(format!(
                    "a scheduled command fires only while the song plays, when `{command}` \
                     would do nothing; run it now with `transport`, or schedule \
                     `transport.locate` to jump"
                ));
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
            schedule.entries.push(entry);
            ctx.state
                .outbox
                .push(EngineMessage::Schedule(ScheduledCommand {
                    id,
                    at: ticks,
                    action,
                }));
            done(json!({
                "id": id,
                "at": position,
                "ticks": ticks.get(),
                "command": command,
                "sample_accurate": !matches!(action, Action::Notify),
                "note": if ctx.state.engine_attached {
                    "It fires when the playing song reaches that position."
                } else {
                    "Nothing plays the song here (no engine is attached to this agent server), \
                     so it waits until the song plays through that position."
                },
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixer_and_device_changes_become_engine_actions() {
        let action = engine_action("mixer.set_mute", &json!({ "track": 4, "mute": true }));
        assert_eq!(
            action,
            Action::Param {
                key: NodeKey::Mixer(bl_model::TrackId(bl_core::id::Id::new(4).unwrap())),
                param: mixer::track::MUTE,
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
        assert_eq!(
            engine_action("mixer.set_solo", &json!({ "track": 4, "solo": true })),
            Action::Notify
        );
    }

    #[test]
    fn transport_commands_become_the_engines_transport_actions() {
        assert_eq!(
            engine_action("transport.stop", &json!({})),
            Action::Transport(TransportCommand::Stop)
        );
        assert_eq!(
            engine_action("transport.locate", &json!({ "position": 2_822_400 })),
            Action::Transport(TransportCommand::Locate(Ticks(2_822_400)))
        );
        // The loop is the document's: the engine follows it when the
        // document changes.
        assert_eq!(
            engine_action("transport.set_loop_enabled", &json!({ "enabled": true })),
            Action::Notify
        );
    }

    #[test]
    fn every_transport_action_runs_a_registered_transport_command() {
        let shell = Shell::headless();
        for action in [
            TransportAction::Play,
            TransportAction::Stop,
            TransportAction::Continue,
            TransportAction::Locate,
            TransportAction::InsertMarkerToPlayhead,
            TransportAction::SetLoop,
            TransportAction::SetLoopEnabled,
        ] {
            let id = action.command().unwrap();
            let spec = super::super::command(&shell, id).unwrap_or_else(|| panic!("{id}"));
            assert_eq!(spec.area(), "transport");
            // The playback commands go to the engine; the loop's run on the
            // document.
            assert_eq!(
                super::super::is_engine_command(&shell, id),
                !model::handles(id),
                "{id}"
            );
        }
        assert_eq!(TransportAction::Status.command(), None);
    }
}

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
//!
//! The Session View's launch and stop commands (WP-3.2) act in the engine
//! too: [`send_session`] queues one as an [`EngineMessage::Session`], and a
//! launch or a stop lands on the launch quantisation grid the same way the
//! UI's do.

use bl_commands::Permission;
use bl_core::time::Ticks;
use bl_engine::{
    Action, EngineState, NodeKey, ParamValue, Report, ScheduledCommand, SessionCommand,
    TransportCommand, mixer, session, transport,
};
use bl_model::registry as model;
use bl_shell::Shell;
use lspk_agent::Session;
use lspk_agent::permission::Decision;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::run::resolve_placeholders;
use super::{Ctx, ToolOutcome, args, done, failed, question_line, step, tool_definition};
use crate::permission::{Mode, decide};
use bl_shell::permission::{CallPermission, raised_by_arguments};

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
    /// Run this Session View command at the start of the next block (send
    /// it with `Sender::session`), landing on the launch quantisation grid
    /// (WP-3.2).
    Session(SessionCommand),
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
    pub(crate) fn new(playing: bool, position: Ticks, insert_marker: Ticks) -> Self {
        Self {
            playing,
            position,
            insert_marker,
        }
    }

    pub(crate) fn of(state: &EngineState) -> Self {
        Self::new(state.playing, state.position_ticks, state.insert_marker)
    }
}

/// Where a session keeps its number in [`Session::refs`], which every call
/// carries and which a call that names IDs writes back whole, so the number
/// survives.
const SESSION_NUMBER: &str = "$session";

/// Gives `session` a number of its own the first time it calls, so what it
/// holds in the engine (a Phase Nudge) can end when the session does. The kit
/// keeps sessions by ID and closes them itself, and a call isn't told which
/// session it is on.
pub(crate) fn number_session(state: &mut super::State, session: &mut Session) {
    if !session.refs.contains_key(SESSION_NUMBER) {
        state.sessions_numbered += 1;
        session
            .refs
            .insert(SESSION_NUMBER.to_owned(), state.sessions_numbered);
    }
}

/// Where a session keeps the set its `$ref` names were issued in.
const SESSION_SET: &str = "$set";

/// Drops the scheduled commands, here and in the engine, of a set that was
/// replaced (a new or opened one): they would act on other objects in the
/// new set.
pub(crate) fn follow_set_schedule(state: &mut super::State, generation: u64) {
    if state.set_generation != Some(generation) {
        state.set_generation = Some(generation);
        let cancelled = state.schedule.cancel_waiting(None);
        state
            .outbox
            .extend(cancelled.into_iter().map(EngineMessage::Cancel));
    }
}

/// [`follow_set_schedule`], and the session's `$ref` names, which would name
/// other objects in the new set.
pub(crate) fn follow_set(state: &mut super::State, session: &mut Session, generation: u64) {
    follow_set_schedule(state, generation);
    if session.refs.get(SESSION_SET) != Some(&generation) {
        session.refs.retain(|name, _| name == SESSION_NUMBER);
        session.refs.insert(SESSION_SET.to_owned(), generation);
    }
}

/// The numbers of the sessions that have one.
pub(crate) fn session_numbers<'a>(sessions: impl Iterator<Item = &'a Session>) -> Vec<u64> {
    sessions
        .filter_map(|session| session.refs.get(SESSION_NUMBER).copied())
        .collect()
}

/// Releases the Phase Nudge buttons held by sessions that are not among
/// `open` any more (spec: transport R28), by queueing the releases they never
/// sent.
pub(crate) fn release_orphaned_nudges(state: &mut super::State, open: &[u64]) {
    let outbox = &mut state.outbox;
    state.nudges.retain(|&(owner, direction)| {
        let kept = open.contains(&owner);
        if !kept {
            outbox.push(EngineMessage::Transport(TransportCommand::PhaseNudge {
                direction,
                held: false,
            }));
        }
        kept
    });
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
    // What the shell would refuse when the host sends it, said now rather
    // than dropped then.
    if let Some(reason) = ctx.shell.transport_refusal(command) {
        return Err(format!("`{id}` is refused: {reason}"));
    }
    if let TransportCommand::PhaseNudge { direction, held } = command {
        // Spec: transport R28. The session that pressed a button holds it.
        let owner = ctx.session.refs.get(SESSION_NUMBER).copied().unwrap_or(0);
        ctx.state.nudges.retain(|&(_, button)| button != direction);
        if held {
            ctx.state.nudges.push((owner, direction));
        }
    }
    ctx.state.outbox.push(EngineMessage::Transport(command));
    Ok(())
}

/// Sends one of the Session View's launch or stop commands to the engine,
/// through the host, which puts it on the engine's queue as the UI's
/// launching goes: the agent never touches the audio thread.
pub(crate) fn send_session(ctx: &mut Ctx<'_>, id: &str, args: Value) -> Result<(), String> {
    if !ctx.state.engine_attached {
        return Err(format!(
            "nothing plays the song here: no engine is attached to this agent server \
             (`bl-cli mcp` runs without one), so `{id}` can't launch or stop a clip. Attach an \
             engine, or run the command where the set plays"
        ));
    }
    let command = session::command_from_args(id, args).map_err(|error| error.to_string())?;
    ctx.state.outbox.push(EngineMessage::Session(command));
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
    /// Who made the step the command records, so the history says so
    /// (undo history R9).
    maker: bl_model::Maker,
    /// What the engine does on the frame.
    action: Action,
    /// The classes the agent was cleared to run when this was scheduled: the
    /// ones the person said yes to, and those the agent needs no answer for.
    cleared: Vec<Permission>,
    /// The classes the call had when it was scheduled, for a command whose
    /// class its arguments don't change.
    classes: Vec<Permission>,
    /// Whether the agent's autonomy cleared it, so it is decided again when
    /// it fires, against the agent's rights then.
    by_autonomy: bool,
    /// The agent whose rights the session had, and where the agent API runs.
    agent: Option<String>,
    mode: Mode,
    /// The set it was scheduled in ([`Shell::set_generation`]).
    set: u64,
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
    /// The engine's commands for those waiting to fire, for an engine that
    /// has just started.
    pub(crate) fn waiting_commands(&self) -> Vec<ScheduledCommand> {
        self.entries
            .iter()
            .filter(|e| e.state == State::Waiting)
            .map(|e| ScheduledCommand {
                id: e.id,
                at: e.at,
                action: e.action,
            })
            .collect()
    }

    /// Cancels every waiting command, or those of `agent` only, and gives
    /// their IDs, for the engine to drop.
    pub(crate) fn cancel_waiting(&mut self, agent: Option<&str>) -> Vec<u64> {
        self.entries
            .iter_mut()
            .filter(|e| {
                e.state == State::Waiting && agent.is_none_or(|a| e.agent.as_deref() == Some(a))
            })
            .map(|e| {
                e.state = State::Cancelled;
                e.id
            })
            .collect()
    }

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
    if entry.set != shell.set_generation() {
        return Err(
            "not run: the set was replaced since this was scheduled. Nothing changed".to_owned(),
        );
    }
    // Spec: agent-permissions R1. What a command acts on can change between
    // scheduling and firing (the clipboard, the selection, the insert
    // marker), so it is judged again, and left alone when it now reaches
    // further than the agent was cleared to.
    let now = if raised_by_arguments(&entry.command) {
        shell.call_permission(&entry.command, &entry.args).classes()
    } else {
        entry.classes.clone()
    };
    // A clearance that autonomy gave goes with the autonomy, or with the
    // agent being forgotten.
    if entry.by_autonomy
        && now.iter().any(|class| {
            class.asks_first()
                && decide(
                    entry.mode,
                    shell,
                    entry.agent.as_deref(),
                    *class,
                    &super::judged_ids(&entry.command, &entry.args),
                ) != Decision::Run
        })
    {
        return Err(
            "not run: the agent is no longer autonomous (or was forgotten), and it needs \
                    the person's say. Nothing changed"
                .to_owned(),
        );
    }
    let missing: Vec<Permission> = now
        .into_iter()
        .filter(|class| !entry.cleared.contains(class))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "not run: on the song as it is now it needs the person's say ({missing:?}), which \
             was not asked when it was scheduled. Nothing changed"
        ));
    }
    if is_edit(&entry.command) {
        // Spec: macros R11, R13. Through the shell, as `run` goes, so a macro
        // holds its parameters and moves its mapped ones.
        shell
            .prepare_edit(&entry.command, entry.args.clone())
            .and_then(|prepared| {
                shell.run_document_edits(
                    entry.maker.clone(),
                    &entry.label,
                    &[(entry.command.clone(), prepared)],
                )
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    } else {
        // The Arrangement's edits and the browser's drops are the agent's
        // undo steps too.
        shell
            .run_labelled(
                entry.maker.clone(),
                entry.label.clone(),
                &entry.command,
                entry.args.clone(),
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

fn is_edit(id: &str) -> bool {
    model::handles(id) && id != model::UNDO && id != model::REDO && id != model::GO_TO_STEP
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
            // Spec: agent-permissions R10. The engine would record on the
            // frame, but only `Shell::send_transport` sets up the files it
            // writes, and a fired command doesn't pass through it.
            if action == Action::Transport(TransportCommand::SetRecording(true)) {
                return failed(format!(
                    "a scheduled `{command}` can't start a recording, which needs files set up \
                     first; schedule `transport.locate` to jump, and start the recording with \
                     `transport` when the song is there"
                ));
            }
            // Spec: transport R28. A held Phase Nudge ends with the session
            // that pressed it, which a scheduled press has none of.
            if let Action::Transport(TransportCommand::PhaseNudge { held: true, .. }) = action {
                return failed(format!(
                    "a scheduled `{command}` can't hold a Phase Nudge, which ends with the \
                     session that pressed it; press it with `run` when the song is there"
                ));
            }
            // Try it on the song as it is now, so a mistake shows today
            // rather than when the song gets there.
            if is_edit(&command) {
                let tried = ctx
                    .shell
                    .prepare_edit(&command, args.clone())
                    .and_then(|prepared| {
                        ctx.shell
                            .try_document_edits(&[(command.clone(), prepared)])
                            .map_err(|(_, error)| error)
                    });
                if let Err(error) = tried {
                    return failed(format!(
                        "`{command}` fails on the song as it is now: {error}"
                    ));
                }
            }
            // Spec: agent-permissions R1. Decided from its arguments against
            // the song as it is now, when the person can be asked.
            let call = if raised_by_arguments(&command) {
                ctx.shell.call_permission(&command, &args)
            } else {
                CallPermission::registered(class)
            };
            let classes = call.classes();
            let shell = &*ctx.shell;
            let judged = super::judged_ids(&command, &args);
            step!(ctx.permit(&classes, &judged, || format!(
                "At {position}: {}",
                question_line(shell, &command, &label, &call, &args)
            )));
            // What the person just said yes to is cleared, and so is what the
            // agent needs no answer for.
            let by_autonomy = ctx.decide(Permission::Destructive, &judged) == Decision::Run;
            let cleared: Vec<Permission> = Permission::ALL
                .into_iter()
                .filter(|class| {
                    !class.asks_first()
                        || classes.contains(class)
                        || ctx.decide(*class, &judged) == Decision::Run
                })
                .collect();
            ctx.session.refs = names;
            let maker = ctx.maker();
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
                maker,
                action,
                cleared,
                classes,
                by_autonomy,
                agent: ctx.session.agent.clone(),
                mode: ctx.mode,
                set: ctx.shell.set_generation(),
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

//! The tools: a few that serve the whole command registry, rather than one
//! per command (the architecture's "Agent control").
//!
//! | Tool | Group | What it does |
//! |---|---|---|
//! | `list_commands` | Discovery | Lists commands by area, or finds them by words |
//! | `describe_command` | Discovery | One command's description, permission, and schemas |
//! | `run` | Run | One command or a batch as one undo step, or a dry run |
//! | `query` | Query | Reads the project by ID or readable path, with summaries |
//! | `transport` | Transport and time | Plays, stops, continues, locates, and loops the song, and says where it is |
//! | `schedule` | Transport and time | Runs a command at a bar and beat, on its exact sample |
//! | `checkpoint` | Checkpoints | Saves and restores named versions of the document |
//! | `preview` | Perception | Renders a range or a track as audio the client can play |
//! | `analyze` | Perception | Measures a preview or a range: loudness, levels, spectrum, tempo, key, transients |
//! | `capture_view` | Perception | Captures one of the app's views as an image, while the app runs |
//!
//! The guide is a resource, not a tool (see [`crate::guide`]).
//!
//! The commands the tools serve are the shell's registry and the
//! transport's playback commands, which the engine runs (see [`command`]
//! and [`time`]).

mod checkpoint;
mod discovery;
mod perception;
mod query;
mod run;
mod time;

use std::sync::OnceLock;

use bl_commands::{CommandSpec, Permission, Registry};
use bl_shell::Shell;
use bl_shell::permission::CallPermission;
use lspk_agent::Session;
use lspk_agent::permission::and_list;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

pub(crate) use checkpoint::Checkpoints;
pub(crate) use lspk_agent::permission::class_names;
pub(crate) use lspk_agent::tool_definition;
pub use time::EngineMessage;
pub(crate) use time::{Playhead, Schedule};

use crate::permission::{Decision, Mode, decide};
use crate::server::Capture;

/// What the tools share across sessions: checkpoints and scheduled
/// commands belong to the project, not to one agent.
#[derive(Debug, Default)]
pub(crate) struct State {
    pub(crate) checkpoints: Checkpoints,
    pub(crate) schedule: Schedule,
    /// What the engine must hear, in the order the agents asked, until the
    /// host takes it.
    pub(crate) outbox: Vec<EngineMessage>,
    /// Whether an engine plays the song, so the transport moves and
    /// scheduled commands fire.
    pub(crate) engine_attached: bool,
    /// Where the engine last said the transport was.
    pub(crate) playhead: Option<Playhead>,
}

/// The transport's playback commands (WP-1.5): play, stop, continue,
/// locate, and the rest that `bl_engine::transport` registers. They act in
/// the engine rather than on the document, so the shell doesn't run them;
/// the agent API sends them to the engine through the host
/// ([`EngineMessage::Transport`]).
fn engine_commands() -> &'static Registry {
    static COMMANDS: OnceLock<Registry> = OnceLock::new();
    COMMANDS.get_or_init(|| {
        let mut registry = Registry::new();
        bl_engine::transport::register(&mut registry)
            .expect("the transport's command IDs are unique");
        registry
    })
}

/// Whether `id` is a transport command the engine runs (a playback command,
/// not the loop, which belongs to the set), which the agent API sends to the
/// engine its host attached rather than to the shell. The shell registers
/// the same commands for its menus and keys (WP-1.18), so the registry
/// can't tell them apart.
pub(crate) fn is_engine_command(_shell: &Shell, id: &str) -> bool {
    bl_engine::transport::handles(id) && !bl_model::registry::handles(id)
}

/// The command called `id`: the shell's, or one of the transport's.
pub(crate) fn command<'a>(shell: &'a Shell, id: &str) -> Option<&'a CommandSpec> {
    shell
        .registry()
        .get(id)
        .or_else(|| engine_commands().get(id))
}

/// Every command agents can run: the shell's, then the transport's
/// playback commands.
pub(crate) fn commands(shell: &Shell) -> impl Iterator<Item = &CommandSpec> {
    shell.registry().iter().chain(
        engine_commands()
            .iter()
            .filter(|spec| !shell.registry().contains(&spec.id)),
    )
}

/// Everything a tool call works with.
pub(crate) struct Ctx<'a> {
    pub(crate) mode: Mode,
    pub(crate) shell: &'a mut Shell,
    pub(crate) session: &'a mut Session,
    pub(crate) state: &'a mut State,
    /// The person already allowed this call.
    pub(crate) confirmed: bool,
}

/// How a tool call ended: the kit's outcome, where a call that waits for
/// the host is a view capture (WP-1.16), which the app's window draws on
/// the main thread's next frame.
pub(crate) type ToolOutcome = lspk_agent::ToolOutcome<Capture>;

/// A line of the person's question: the command's label, the classes it
/// asks for, its arguments, and the clips it would replace or shorten, by
/// name (spec: agent-permissions R4).
pub(crate) fn question_line(
    shell: &Shell,
    label: &str,
    call: &CallPermission,
    args: &Value,
) -> String {
    let asking: Vec<Permission> = call
        .classes()
        .into_iter()
        .filter(|class| class.asks_first())
        .collect();
    let classes = class_names(&asking);
    let args = describe_args(shell, args);
    let mut line = if args.is_empty() {
        format!("{label} ({classes})")
    } else {
        format!("{label} ({classes}): {args}")
    };
    let replaced = replaced_clips(shell, call);
    if !replaced.is_empty() {
        line.push_str(". ");
        line.push_str(&replaced);
    }
    line
}

/// The clips a call would replace or shorten, named as the question names
/// clips: the dry run's `replaces`, as a sentence. Empty if there are none.
fn replaced_clips(shell: &Shell, call: &CallPermission) -> String {
    if call.replaces.is_empty() {
        return String::new();
    }
    let document = shell.editor().document();
    let names: Vec<String> = call
        .replaces
        .iter()
        .map(|clip| match document.clip(*clip) {
            Some(found) if !found.name.is_empty() => {
                format!("clip \u{201c}{}\u{201d}", found.name)
            }
            _ => format!("clip {}", clip.0),
        })
        .collect();
    let names = and_list(&names);
    if call.may {
        format!("It may replace or shorten {names}.")
    } else {
        format!("It replaces or shortens {names}.")
    }
}

impl Ctx<'_> {
    /// Whether the agent may go ahead with commands whose most far-reaching
    /// class is `class`: `Ok` to go ahead, or how the call ends instead.
    pub(crate) fn permit(
        &self,
        classes: &[Permission],
        commands: &[&str],
        summary: impl FnOnce() -> String,
    ) -> Result<(), ToolOutcome> {
        lspk_agent::permission::permit(
            classes,
            self.confirmed,
            |class| self.decide(class, commands),
            summary,
        )
    }

    /// Whether the agent may go ahead with commands whose most far-reaching
    /// class is `class`, with the rights its session has.
    pub(crate) fn decide(&self, class: Permission, commands: &[&str]) -> Decision {
        decide(
            self.mode,
            self.shell,
            self.session.agent.as_deref(),
            class,
            commands,
        )
    }

    /// The agent's name, as undo steps and the person see it.
    pub(crate) fn agent(&self) -> &str {
        self.session.client.label()
    }

    /// An undo step's label for something this agent did.
    pub(crate) fn step_label(&self, what: &str) -> String {
        format!("{}: {what}", self.agent())
    }
}

/// Every tool's definition, for `tools/list`.
pub(crate) fn list() -> Vec<Value> {
    vec![
        discovery::list_definition(),
        discovery::describe_definition(),
        run::definition(),
        query::definition(),
        time::transport_definition(),
        time::schedule_definition(),
        checkpoint::definition(),
        perception::preview_definition(),
        perception::analyze_definition(),
        perception::capture_definition(),
    ]
}

/// Runs the tool called `name`.
///
/// # Errors
///
/// A message if no tool has that name, which is a protocol error rather
/// than the tool's own failure.
pub(crate) fn call(ctx: &mut Ctx<'_>, name: &str, args: Value) -> Result<ToolOutcome, String> {
    Ok(match name {
        discovery::LIST => discovery::list(ctx, args),
        discovery::DESCRIBE => discovery::describe(ctx, args),
        run::NAME => run::run(ctx, args),
        query::NAME => query::query(ctx, args),
        time::TRANSPORT => time::transport(ctx, args),
        time::SCHEDULE => time::schedule(ctx, args),
        checkpoint::NAME => checkpoint::checkpoint(ctx, args),
        perception::PREVIEW => perception::preview(ctx, args),
        perception::ANALYZE => perception::analyze(ctx, args),
        perception::CAPTURE => perception::capture(ctx, args),
        other => return Err(format!("no tool is called `{other}`")),
    })
}

/// A tool's arguments, read into `T`. Missing arguments read as `{}`.
fn args<T: DeserializeOwned>(args: Value) -> Result<T, ToolOutcome> {
    let args = if args.is_null() { json!({}) } else { args };
    serde_json::from_value(args)
        .map_err(|error| ToolOutcome::Done(Err(format!("invalid arguments: {error}"))))
}

/// Ends a tool call with its result.
fn done(result: Value) -> ToolOutcome {
    ToolOutcome::Done(Ok(result))
}

/// Ends a tool call with a failure the agent reads.
fn failed(message: impl Into<String>) -> ToolOutcome {
    ToolOutcome::Done(Err(message.into()))
}

/// Unwraps a step that may end the call early.
macro_rules! step {
    ($result:expr) => {
        match $result {
            Ok(value) => value,
            Err(outcome) => return outcome,
        }
    };
}
pub(crate) use step;

/// A command's arguments described for the person: IDs of tracks, clips,
/// and devices with their names, and other values as they are.
pub(crate) fn describe_args(shell: &Shell, args: &Value) -> String {
    let Value::Object(fields) = args else {
        return String::new();
    };
    let document = shell.editor().document();
    let name_of = |key: &str, value: &Value| -> Option<String> {
        let id = bl_core::id::Id::new(value.as_u64()?)?;
        let name = match key {
            "track" => document.track(bl_model::TrackId(id))?.name().to_owned(),
            "clip" => document.clip(bl_model::ClipId(id))?.name.clone(),
            "device" => document.device(bl_model::DeviceId(id))?.name.clone(),
            _ => return None,
        };
        Some(if name.is_empty() {
            format!("{key} {id}")
        } else {
            format!("{key} \u{201c}{name}\u{201d}")
        })
    };
    fields
        .iter()
        .map(|(key, value)| {
            name_of(key, value).unwrap_or_else(|| match value {
                Value::String(text) => format!("{key} \u{201c}{text}\u{201d}"),
                Value::Object(_) | Value::Array(_) => format!("{key} (…)"),
                other => format!("{key} {other}"),
            })
        })
        .collect::<Vec<_>>()
        .join(", ")
}

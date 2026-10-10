//! The tools: a few that serve the whole command registry, rather than one
//! per command (the architecture's "Agent control").
//!
//! | Tool | Group | What it does |
//! |---|---|---|
//! | `list_commands` | Discovery | Lists commands by area, or finds them by words |
//! | `describe_command` | Discovery | One command's description, permission, and schemas |
//! | `run` | Run | One command or a batch as one undo step, or a dry run |
//! | `query` | Query | Reads the project by ID or readable path, with summaries |
//! | `transport` | Transport and time | The transport's commands, when the build has them |
//! | `schedule` | Transport and time | Runs a command at a bar and beat, on its exact sample |
//! | `checkpoint` | Checkpoints | Saves and restores named versions of the document |
//!
//! The guide is a resource, not a tool (see [`crate::guide`]).

mod checkpoint;
mod discovery;
mod query;
mod run;
mod time;

use bl_commands::Permission;
use bl_shell::Shell;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

pub(crate) use checkpoint::Checkpoints;
pub use time::EngineMessage;
pub(crate) use time::Schedule;

use crate::permission::{Decision, Mode, decide};
use crate::server::Session;

/// What the tools share across sessions: checkpoints and scheduled
/// commands belong to the project, not to one agent.
#[derive(Debug, Default)]
pub(crate) struct State {
    pub(crate) checkpoints: Checkpoints,
    pub(crate) schedule: Schedule,
    /// Whether an engine plays the song, so scheduled commands fire.
    pub(crate) engine_attached: bool,
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

/// How a tool call ended.
#[derive(Debug)]
pub(crate) enum ToolOutcome {
    /// It finished: its result, or why it failed (which the agent reads).
    Done(Result<Value, String>),
    /// It needs the person's confirmation first.
    Ask {
        /// The most far-reaching class among what it would run.
        class: Permission,
        /// What it would do, for the person to read.
        summary: String,
    },
}

impl Ctx<'_> {
    /// Whether the agent may go ahead with commands whose most far-reaching
    /// class is `class`: `Ok` to go ahead, or how the call ends instead.
    pub(crate) fn permit(
        &self,
        class: Permission,
        commands: &[&str],
        summary: impl FnOnce() -> String,
    ) -> Result<(), ToolOutcome> {
        match decide(
            self.mode,
            self.shell,
            &self.session.client.name,
            class,
            commands,
        ) {
            Decision::Run => Ok(()),
            Decision::Ask if self.confirmed => Ok(()),
            Decision::Ask => Err(ToolOutcome::Ask {
                class,
                summary: summary(),
            }),
            Decision::Refuse(message) => Err(ToolOutcome::Done(Err(message))),
        }
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

/// A tool's definition, as `tools/list` gives it.
fn tool_definition<T: JsonSchema>(
    name: &str,
    title: &str,
    description: &str,
    read_only: bool,
) -> Value {
    let mut schema = bl_commands::args_schema::<T>().to_value();
    if let Some(object) = schema.as_object_mut() {
        // The tool's description says what the arguments are for.
        object.remove("title");
        object.remove("description");
    }
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": schema,
        "annotations": {
            "title": title,
            "readOnlyHint": read_only,
            "destructiveHint": !read_only,
            "idempotentHint": read_only,
            "openWorldHint": false,
        },
    })
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

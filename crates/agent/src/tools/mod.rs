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
pub(crate) use time::{
    Playhead, Schedule, number_session, release_orphaned_nudges, session_numbers,
};

use crate::permission::{Decision, Mode, decide};
use crate::server::Deferred;

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
    /// The Phase Nudge buttons agents hold, each with the number of the
    /// session that pressed it, so the nudge ends with the session (the
    /// transport spec, R28).
    pub(crate) nudges: Vec<(u64, bl_engine::NudgeDirection)>,
    /// How many sessions have been given a number.
    pub(crate) sessions_numbered: u64,
}

/// The transport's playback commands (WP-1.5) and the Session View's
/// launching and stopping (WP-3.2): the commands that act in the engine
/// rather than on the document, so the shell doesn't run them; the agent API
/// sends them to the engine through the host ([`EngineMessage::Transport`]
/// and [`EngineMessage::Session`]).
fn engine_commands() -> &'static Registry {
    static COMMANDS: OnceLock<Registry> = OnceLock::new();
    COMMANDS.get_or_init(|| {
        let mut registry = Registry::new();
        bl_engine::transport::register(&mut registry)
            .expect("the transport's command IDs are unique");
        bl_engine::session::register(&mut registry)
            .expect("the Session View's command IDs are unique");
        registry
    })
}

/// Whether `id` is a command the engine runs (a playback command, not the
/// loop, which belongs to the set; or one of the Session View's launch and
/// stop commands), which the agent API sends to the engine its host
/// attached rather than to the shell. The shell registers the same commands
/// for its menus and keys (WP-1.18), so the registry can't tell them apart.
pub(crate) fn is_engine_command(_shell: &Shell, id: &str) -> bool {
    (bl_engine::transport::handles(id) || bl_engine::session::handles(id))
        && !bl_model::registry::handles(id)
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
    /// Whether a window runs that can capture views and defer renders (WP-1.16, WP-2.28).
    pub(crate) view_capture: bool,
}

/// How a tool call ended: the kit's outcome, where a call that waits for
/// the host is a view capture (WP-1.16) or an audio render (WP-2.28).
pub(crate) type ToolOutcome = lspk_agent::ToolOutcome<Deferred>;

/// A line of the person's question: the command's label, the classes it
/// asks for, its arguments, and the clips it would replace or shorten, by
/// name (spec: agent-permissions R4). The commands that open the sample
/// editor say which command line they start.
pub(crate) fn question_line(
    shell: &Shell,
    id: &str,
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
    let described = describe_args(shell, args);
    let mut line = if described.is_empty() {
        format!("{label} ({classes})")
    } else {
        format!("{label} ({classes}): {described}")
    };
    let replaced = replaced_clips(shell, call);
    if !replaced.is_empty() {
        line.push_str(". ");
        line.push_str(&replaced);
    }
    // A MIDI Tool's call that names no clip acts on the one the Clip View
    // shows, which the question then names (the MIDI Tools spec, R24).
    if let Some(fewer) = call.fewer_notes
        && args.get("clip").is_none()
    {
        line.push_str(&format!(
            ". It leaves {} {} of its {} notes.",
            clip_name(shell, fewer.clip),
            fewer.leaves,
            fewer.has
        ));
    }
    if let Some(track) = call.deletes_track {
        let name = shell
            .editor()
            .document()
            .track(track)
            .map(|track| track.name().to_owned())
            .filter(|name| !name.is_empty())
            .map_or_else(
                || format!("track {}", track.0),
                |name| format!("track \u{201c}{name}\u{201d}"),
            );
        line.push_str(&format!(
            ". It deletes {name} with the devices, automation and settings on it."
        ));
    }
    if matches!(
        id,
        bl_shell::audio_panel_commands::EDIT_SAMPLE | bl_shell::file_manager_commands::EDIT_FILE
    ) {
        match shell.sample_editor() {
            Some(editor) => line.push_str(&format!(". It starts \u{201c}{editor}\u{201d}.")),
            None => line.push_str(". It opens the File & Folder page."),
        }
    }
    if let Some(clip) = call.cropped {
        line.push_str(". ");
        line.push_str(&format!(
            "It crops {}, dropping what the region doesn't keep.",
            clip_name(shell, clip)
        ));
    }
    line
}

/// A clip as the question names it: its name, or its ID when it has none.
fn clip_name(shell: &Shell, clip: bl_model::ClipId) -> String {
    match shell.editor().document().clip(clip) {
        Some(found) if !found.name.is_empty() => format!("clip \u{201c}{}\u{201d}", found.name),
        _ => format!("clip {}", clip.0),
    }
}

/// The clips a call would replace or shorten, named as the question names
/// clips: the dry run's `replaces`, as a sentence. Empty if there are none.
fn replaced_clips(shell: &Shell, call: &CallPermission) -> String {
    if call.replaces.is_empty() {
        return String::new();
    }
    let names: Vec<String> = call
        .replaces
        .iter()
        .map(|clip| clip_name(shell, *clip))
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

    /// Who made the steps this agent's calls record: the agent, by the name
    /// the agent API knows it by (undo history R9).
    pub(crate) fn maker(&self) -> bl_model::Maker {
        bl_model::Maker::Agent(self.agent().to_owned())
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

/// The `query` tool for the built-in assistant, which reads the project as
/// an agent does. It has no checkpoints or scheduled commands of its own to
/// report: those belong to the agent server.
pub fn assistant_query(shell: &mut Shell, args: Value) -> Result<Value, String> {
    let mut ctx = Ctx {
        mode: Mode::App,
        shell,
        session: &mut Session::default(),
        state: &mut State::default(),
        confirmed: false,
        view_capture: false,
    };
    match query::query(&mut ctx, args) {
        ToolOutcome::Done(result) => result,
        _ => Err("the query couldn't be answered".to_owned()),
    }
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

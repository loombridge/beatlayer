//! The built-in assistant at work (WP-2.23): the settings the Agents page
//! changes, the agent loop's host, its permission prompts, and the activity
//! log.
//!
//! The loop itself is `bl-assistant`'s ([`bl_assistant::agent`]): it sends
//! the conversation and its few tools to a provider and runs each `run`
//! call as the registry command it names, through a [`Commands`] host. This
//! module is that host, backed by the [`Shell`]: each turn's changes are one
//! undo step, each call's permission class is decided from its arguments
//! against the set as it is, and a destructive or external call the person
//! hasn't already allowed asks them first ([`Prompter`]). A denial is
//! returned to the model as that call's result, and every command is
//! recorded in the activity log ([`ActivityEntry`]).
//!
//! The panel (part 3) shows the conversation this module keeps in the shell
//! ([`Row`]): what the person sent, what the model said as it streamed,
//! and each tool call with its result. [`run_turn`] drives one turn of the
//! loop and fills the rows as the turn goes; the panel's Stop is
//! `assistant.stop`, which the loop sees between two calls.

pub use bl_shell_spec::assistant::*;

use std::collections::BTreeMap;
use std::ops::RangeInclusive;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use bl_assistant::agent::{Agent, Commands, Turn};
use bl_assistant::keychain::Keychain;
use bl_assistant::message::{Event, Message, Request};
use bl_assistant::provider::{Anthropic, Error, Gemini, OpenAi, Provider};
use bl_assistant::tools::{DESCRIBE_COMMAND, LIST_COMMANDS, QUERY};
use bl_commands::{CommandSpec, Permission, Registry};
use bl_core::id::Id;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Shell;
use crate::placeholders::resolve_placeholders;

/// The widths the docked panel may take, in points.
pub const WIDTH_RANGE: RangeInclusive<f32> = PANEL_MIN_WIDTH..=PANEL_MAX_WIDTH;

/// What the model is told about the app it is in.
const SYSTEM_PROMPT: &str = "You are the assistant built into Beatlayer, a digital audio \
    workstation. Use the tools to look at and change the person's set, and say briefly what you \
    did. Everything you change in one turn is one undo step. A command that creates something \
    needs a new ID: write {\"$new\": \"name\"} where the ID goes, and {\"$ref\": \"name\"} wherever a \
    later call in the turn refers to it. Positions and lengths are in ticks, 705600 to the \
    quarter note.";

/// One line of the panel's conversation.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    /// What the person sent.
    You(String),
    /// What the model said, as far as it has streamed.
    Assistant(String),
    /// A tool call the model made, shown while it runs and then with its
    /// result.
    Call {
        /// The call's ID, which its result answers.
        id: String,
        /// The tool the model named, such as `track_create`.
        name: String,
        /// The registry command the tool is, such as `track.create`.
        command: Option<String>,
        /// The arguments the model gave.
        arguments: Value,
        /// What came of it: `None` while it waits or runs, then the
        /// command's result, or why it didn't run.
        result: Option<Result<Value, String>>,
    },
    /// Something the person should know that isn't the model's: why a turn
    /// ended early or failed.
    Notice(String),
}

/// The conversation the panel shows, and the state of the turn running.
#[derive(Debug, Default)]
pub struct Conversation {
    pub(crate) rows: Vec<Row>,
    /// The conversation in the providers' terms, which the next turn
    /// continues.
    history: Vec<Message>,
    pub(crate) running: bool,
    /// What the person sent that no turn has taken yet.
    pub(crate) pending: Option<String>,
}

impl Conversation {
    fn push_text(&mut self, text: &str) {
        match self.rows.last_mut() {
            Some(Row::Assistant(said)) => said.push_str(text),
            _ => self.rows.push(Row::Assistant(text.to_owned())),
        }
    }

    /// Gives the first call that waits for `command` its result.
    pub(crate) fn finish_call(&mut self, command: &str, outcome: Result<Value, String>) {
        let waiting = self.rows.iter_mut().find_map(|row| match row {
            Row::Call {
                command: Some(called),
                result: result @ None,
                ..
            } if called == command => Some(result),
            _ => None,
        });
        if let Some(result) = waiting {
            *result = Some(outcome);
        }
    }
}

/// The conversation, shared between the shell, which the panel reads it
/// from, and the turn that fills it while the shell runs the turn's calls.
pub(crate) type SharedConversation = Arc<Mutex<Conversation>>;

/// The shell's end of a turn's Stop button.
pub(crate) type StopFlag = Arc<AtomicBool>;

pub(crate) fn lock(conversation: &SharedConversation) -> MutexGuard<'_, Conversation> {
    conversation.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Runs the assistant's turn for `text` on `shell`: one turn of the loop
/// with `provider`, asking `prompter` before a command that needs consent.
/// The panel's rows fill as the turn goes, and Stop (`assistant.stop`) ends
/// the turn between two calls.
///
/// The person's message is `text`; when it came from
/// [`Shell::take_assistant_prompt`] its row is already there, otherwise it
/// is added.
///
/// # Errors
///
/// [`Error`] if the provider failed. The turn's changes so far are still
/// one undo step, and the panel shows why the turn ended.
pub fn run_turn(
    shell: &mut Shell,
    provider: &dyn Provider,
    prompter: &mut dyn Prompter,
    text: &str,
) -> Result<Turn, Error> {
    let (conversation, stop) = shell.assistant_handles();
    let agent = Agent::new();
    let mut request = Request::new(shell.assistant_settings().model.clone());
    SYSTEM_PROMPT.clone_into(&mut request.system);
    {
        let mut conversation = lock(&conversation);
        if !matches!(conversation.rows.last(), Some(Row::You(said)) if said == text) {
            conversation.rows.push(Row::You(text.to_owned()));
        }
        conversation.running = true;
        request.messages.clone_from(&conversation.history);
    }
    request.messages.push(Message::user(text));
    stop.store(false, Ordering::SeqCst);
    shell.assistant_touch();

    let outcome = {
        let mut commands = AssistantCommands::new(shell, prompter);
        agent.run(
            provider,
            &mut request,
            &mut commands,
            &|| stop.load(Ordering::SeqCst),
            &mut |event| {
                let mut conversation = lock(&conversation);
                match event {
                    Event::Text(text) => conversation.push_text(&text),
                    Event::ToolCall(call) => {
                        // A `run` call shows the command it runs and that
                        // command's arguments.
                        let run = call.command().ok();
                        conversation.rows.push(Row::Call {
                            arguments: run
                                .as_ref()
                                .map_or_else(|| call.arguments.clone(), |run| run.args.clone()),
                            command: run.map(|run| run.command),
                            id: call.id,
                            name: call.name,
                            result: None,
                        });
                    }
                    Event::Usage(_) => {}
                }
            },
        )
    };

    {
        let mut conversation = lock(&conversation);
        conversation.running = false;
        match &outcome {
            Ok(turn) => {
                // A call the loop answered without running it (an unknown
                // tool, or the turn stopped before it) hasn't a result yet.
                for step in &turn.steps {
                    for row in &mut conversation.rows {
                        if let Row::Call { id, result, .. } = row
                            && result.is_none()
                            && *id == step.call.id
                        {
                            *result = Some(step.result.clone());
                        }
                    }
                }
                if let Some(stopped) = turn.stopped {
                    let why = match stopped {
                        bl_assistant::agent::Stopped::Cancelled => "Stopped.",
                        bl_assistant::agent::Stopped::Cap => {
                            "Stopped: the turn reached its limit on commands."
                        }
                    };
                    conversation.rows.push(Row::Notice(why.to_owned()));
                }
                conversation.history = request.messages;
            }
            Err(error) => conversation.rows.push(Row::Notice(format!(
                "The assistant couldn't answer: {error}"
            ))),
        }
        // Whatever ended the turn, no call it left behind is still running.
        for row in &mut conversation.rows {
            if let Row::Call { result, .. } = row
                && result.is_none()
            {
                *result = Some(Err("Not run.".to_owned()));
            }
        }
    }
    shell.assistant_touch();
    outcome
}

/// One command the assistant ran, or tried to: its ID, when, and what came
/// of it.
#[derive(Clone, Debug, PartialEq)]
pub struct ActivityEntry {
    /// The command's ID, such as `track.create`.
    pub command: String,
    /// When the call was made.
    pub at: SystemTime,
    /// What came of it.
    pub outcome: Outcome,
}

/// What came of one command the assistant called.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// It ran, with the command's result.
    Ran(Value),
    /// It was refused before running: the person declined it, or it's a
    /// command only a person runs.
    Denied(String),
    /// It ran and failed, with why.
    Failed(String),
}

/// What the person chose when the assistant asked to run a command whose
/// class needs consent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Allow this call only.
    Once,
    /// Allow this call and every later call of the same command, for this
    /// and later sessions.
    Always,
    /// Refuse the call.
    Deny,
}

/// One command the assistant wants to run that needs the person's consent,
/// shown to them as a [`PromptRequest::question`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptRequest {
    /// The command's ID.
    pub command: String,
    /// The class it needs, its most far-reaching.
    pub class: Permission,
}

impl PromptRequest {
    /// The question shown to the person.
    #[must_use]
    pub fn question(&self) -> String {
        format!(
            "The assistant wants to run \u{201c}{}\u{201d}, which is {}. Allow it?",
            self.command, self.class
        )
    }

    /// What the model is told when the person refuses.
    #[must_use]
    pub fn denial(&self) -> String {
        format!(
            "`{}` wasn't run: the person declined it, so nothing changed.",
            self.command
        )
    }
}

/// How the person is asked to allow a command the assistant wants to run.
/// The app shows a dialog; tests answer from a script.
pub trait Prompter {
    /// Asks the person about `request` and returns their choice.
    fn ask(&mut self, request: &PromptRequest) -> Decision;
}

/// The assistant's host over a [`Shell`]: the [`Commands`] the agent loop
/// runs its tool calls through, with the person's permission prompts and
/// the activity log.
pub struct AssistantCommands<'a> {
    shell: &'a mut Shell,
    prompter: &'a mut dyn Prompter,
    /// The IDs the turn's `$new` placeholders have been given.
    names: BTreeMap<String, u64>,
    /// The call about to run, with its placeholders resolved by `permit`.
    resolved: Option<Value>,
}

impl<'a> AssistantCommands<'a> {
    /// A host running the assistant's calls on `shell`, asking `prompter`
    /// before a destructive or external call the person hasn't allowed.
    pub fn new(shell: &'a mut Shell, prompter: &'a mut dyn Prompter) -> Self {
        Self {
            shell,
            prompter,
            names: BTreeMap::new(),
            resolved: None,
        }
    }
}

impl Commands for AssistantCommands<'_> {
    fn begin_turn(&mut self, label: &str) {
        self.shell.begin_assistant_turn(label);
    }

    fn end_turn(&mut self) {
        self.shell.end_assistant_turn();
    }

    fn permit(&mut self, command: &str, args: &Value) -> Result<(), String> {
        self.resolved = None;
        if self.shell.registry().get(command).is_none() {
            return Err(format!(
                "there is no command `{command}`; `{LIST_COMMANDS}` finds the right ID"
            ));
        }
        // The assistant is the person's own, but it never changes what the
        // person allows, any more than an agent may (agent-permissions R9).
        // Binding keys to such a command would make the person change it.
        let bound = args
            .get("command")
            .and_then(Value::as_str)
            .filter(|_| command == bl_shell_spec::keymap::BIND);
        if let Some(own) = [Some(command), bound]
            .into_iter()
            .flatten()
            .find(|id| bl_shell_spec::agent::handles(id) || handles(id))
        {
            let message =
                format!("`{own}` is the person's own setting; the assistant never changes it.");
            self.shell.assistant_record_denied(command, message.clone());
            self.shell
                .assistant_finish_call(command, Err(message.clone()));
            return Err(message);
        }
        // New IDs are issued here, once, so the class is decided from the
        // arguments that will run.
        let mut args = args.clone();
        let shell = &mut *self.shell;
        resolve_placeholders(&mut args, &mut self.names, &mut || {
            shell.new_id::<Id>().get()
        })?;
        // The class is decided from the arguments against the set as it is,
        // as the agent API decides it (agent-permissions R1).
        let class = self.shell.call_permission(command, &args).class();
        self.resolved = Some(args);
        if !class.asks_first() || self.shell.assistant_always_allows(command) {
            return Ok(());
        }
        let request = PromptRequest {
            command: command.to_owned(),
            class,
        };
        match self.prompter.ask(&request) {
            Decision::Once => Ok(()),
            Decision::Always => {
                self.shell.assistant_remember_command(command);
                Ok(())
            }
            Decision::Deny => {
                let denial = request.denial();
                self.shell.assistant_record_denied(command, denial.clone());
                self.shell
                    .assistant_finish_call(command, Err(denial.clone()));
                Err(denial)
            }
        }
    }

    fn run(&mut self, command: &str, args: Value) -> Result<Value, String> {
        let args = self.resolved.take().unwrap_or(args);
        let at = SystemTime::now();
        let result = self
            .shell
            .run(command, args)
            .map_err(|error| error.to_string());
        let outcome = match &result {
            Ok(value) => Outcome::Ran(value.clone()),
            Err(message) => Outcome::Failed(message.clone()),
        };
        self.shell.assistant_record(ActivityEntry {
            command: command.to_owned(),
            at,
            outcome,
        });
        self.shell.assistant_finish_call(command, result.clone());
        result
    }

    fn read(&mut self, tool: &str, args: Value) -> Result<Value, String> {
        match tool {
            LIST_COMMANDS => list_commands(self.shell.registry(), args),
            DESCRIBE_COMMAND => describe_command(self.shell.registry(), args),
            QUERY => match self.shell.assistant_query() {
                Some(query) => query(self.shell, args),
                None => Err("this app can't read the project for the assistant".to_owned()),
            },
            other => Err(format!("`{other}` isn't a tool that reads")),
        }
    }
}

/// How the host answers the assistant's `query` tool, which reads the
/// project: the agent API's own query, which lives in a crate above the
/// shell, so the host (the app, `bl-cli`) hands it over
/// ([`Shell::set_assistant_query`]).
pub type QueryFn = fn(&mut Shell, Value) -> Result<Value, String>;

/// Arguments of `list_commands`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    area: Option<String>,
    search: Option<String>,
}

/// Arguments of `describe_command`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DescribeArgs {
    command: String,
}

/// `list_commands`: the registry's areas with their counts, or the commands
/// of an area or matching words, best matches first.
fn list_commands(registry: &Registry, args: Value) -> Result<Value, String> {
    let ListArgs { area, search } =
        serde_json::from_value(args).map_err(|error| error.to_string())?;
    if area.is_none() && search.is_none() {
        let mut areas: BTreeMap<&str, usize> = BTreeMap::new();
        for spec in registry.iter() {
            *areas.entry(spec.area()).or_default() += 1;
        }
        return Ok(json!({
            "commands": registry.len(),
            "areas": areas,
            "next": "Call list_commands with an `area` or `search` to list commands.",
        }));
    }
    let words: Vec<String> = search
        .as_deref()
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    let mut found: Vec<(usize, &CommandSpec)> = registry
        .iter()
        .filter(|spec| area.as_deref().is_none_or(|area| spec.area() == area))
        .filter_map(|spec| {
            let named = format!("{} {}", spec.id, spec.label).to_lowercase();
            let description = spec.description.to_lowercase();
            let mut score = 0;
            for word in &words {
                if named.contains(word.as_str()) {
                    score += 2;
                } else if description.contains(word.as_str()) {
                    score += 1;
                } else {
                    return None;
                }
            }
            Some((score, spec))
        })
        .collect();
    found.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    let commands: Vec<Value> = found
        .into_iter()
        .map(|(_, spec)| {
            let description = &spec.description;
            json!({
                "command": spec.id,
                "label": spec.label,
                "permission": spec.permission.unwrap_or(Permission::External).id(),
                "summary": description.find(". ").map_or(&**description, |end| &description[..=end]),
            })
        })
        .collect();
    Ok(json!({ "commands": commands }))
}

/// `describe_command`: one command's description, permission class and
/// schemas.
fn describe_command(registry: &Registry, args: Value) -> Result<Value, String> {
    let DescribeArgs { command } =
        serde_json::from_value(args).map_err(|error| error.to_string())?;
    let spec = registry.get(&command).ok_or_else(|| {
        format!("there is no command `{command}`; `{LIST_COMMANDS}` finds the right ID")
    })?;
    Ok(json!({
        "command": spec.id,
        "label": spec.label,
        "description": spec.description,
        "permission": spec.permission.unwrap_or(Permission::External).id(),
        "arguments": spec.args.as_ref().and_then(|schema| serde_json::to_value(schema).ok()),
        "result": spec.result.as_ref().and_then(|schema| serde_json::to_value(schema).ok()),
    }))
}

/// The provider `kind` names, with its key read from `keychain` when a
/// request is made. `base_url` points it elsewhere than the provider's own
/// API unless it is empty.
#[must_use]
pub fn provider(
    kind: AssistantProvider,
    base_url: &str,
    keychain: Arc<dyn Keychain>,
) -> Box<dyn Provider> {
    match kind {
        AssistantProvider::OpenAi => {
            let provider = OpenAi::new(keychain);
            Box::new(if base_url.is_empty() {
                provider
            } else {
                provider.base_url(base_url)
            })
        }
        AssistantProvider::Anthropic => {
            let provider = Anthropic::new(keychain);
            Box::new(if base_url.is_empty() {
                provider
            } else {
                provider.base_url(base_url)
            })
        }
        AssistantProvider::Gemini => {
            let provider = Gemini::new(keychain);
            Box::new(if base_url.is_empty() {
                provider
            } else {
                provider.base_url(base_url)
            })
        }
    }
}

/// The provider's keychain account, as `bl-assistant` stores it: the same
/// names the providers use.
#[must_use]
pub fn keychain_account(provider: AssistantProvider) -> &'static str {
    match provider {
        AssistantProvider::OpenAi => OpenAi::ACCOUNT,
        AssistantProvider::Anthropic => Anthropic::ACCOUNT,
        AssistantProvider::Gemini => Gemini::ACCOUNT,
    }
}

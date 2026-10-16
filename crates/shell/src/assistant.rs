//! The built-in assistant at work (WP-2.23): the settings the Agents page
//! changes, the agent loop's host, its permission prompts, and the activity
//! log.
//!
//! The loop itself is `bl-assistant`'s ([`bl_assistant::agent`]): it sends
//! the conversation and the commands to a provider and runs each tool call
//! as the registry command it names, through a [`Commands`] host. This
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

use std::ops::RangeInclusive;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use bl_assistant::agent::{Agent, Commands, Turn};
use bl_assistant::message::{Event, Message, Request};
use bl_assistant::provider::{Error, Provider};
use bl_assistant::tools::ToolSet;
use bl_commands::Permission;
use serde_json::Value;

use crate::Shell;

/// The widths the docked panel may take, in points.
pub const WIDTH_RANGE: RangeInclusive<f32> = PANEL_MIN_WIDTH..=PANEL_MAX_WIDTH;

/// What the model is told about the app it is in.
const SYSTEM_PROMPT: &str = "You are the assistant built into Beatlayer, a digital audio \
    workstation. Use the tools to look at and change the person's set, and say briefly what you \
    did. Everything you change in one turn is one undo step.";

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
    let tools = ToolSet::new(shell.registry().iter());
    let agent = Agent::new(shell.registry().iter());
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
                    Event::ToolCall(call) => conversation.rows.push(Row::Call {
                        command: call.command(&tools).ok().map(|command| command.command),
                        id: call.id,
                        name: call.name,
                        arguments: call.arguments,
                        result: None,
                    }),
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
}

impl<'a> AssistantCommands<'a> {
    /// A host running the assistant's calls on `shell`, asking `prompter`
    /// before a destructive or external call the person hasn't allowed.
    pub fn new(shell: &'a mut Shell, prompter: &'a mut dyn Prompter) -> Self {
        Self { shell, prompter }
    }
}

impl Commands for AssistantCommands<'_> {
    fn begin_turn(&mut self, label: &str) {
        self.shell.begin_assistant_turn(label);
    }

    fn end_turn(&mut self) {
        self.shell.end_assistant_turn();
    }

    fn permit(&mut self, command: &str, args: &Value, _class: Permission) -> Result<(), String> {
        // The assistant is the person's own, but it never changes what the
        // person allows, any more than an agent may (agent-permissions R9).
        if bl_shell_spec::agent::handles(command) || handles(command) {
            let message =
                format!("`{command}` is the person's own setting; the assistant never changes it.");
            self.shell.assistant_record_denied(command, message.clone());
            self.shell
                .assistant_finish_call(command, Err(message.clone()));
            return Err(message);
        }
        // The class is decided from the arguments against the set as it is,
        // as the agent API decides it (agent-permissions R1).
        let class = self.shell.call_permission(command, args).class();
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
}

/// The provider's keychain account, as `bl-assistant` stores it: the same
/// names the providers use.
#[must_use]
pub fn keychain_account(provider: AssistantProvider) -> &'static str {
    match provider {
        AssistantProvider::OpenAi => bl_assistant::provider::OpenAi::ACCOUNT,
        AssistantProvider::Anthropic => bl_assistant::provider::Anthropic::ACCOUNT,
        AssistantProvider::Gemini => bl_assistant::provider::Gemini::ACCOUNT,
    }
}

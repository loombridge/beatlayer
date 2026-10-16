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
//! The panel (a later package) drives a turn by making an
//! [`AssistantCommands`] host with its prompter and calling
//! [`bl_assistant::agent::Agent::run`].

pub use bl_shell_spec::assistant::*;

use std::time::SystemTime;

use bl_assistant::agent::Commands;
use bl_commands::Permission;
use serde_json::Value;

use crate::Shell;

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

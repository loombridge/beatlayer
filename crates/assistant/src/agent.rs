//! The agent loop: one turn of the conversation, run to its end
//! (WP-2.23, part 2). It sends the conversation and the command tools to a
//! [`Provider`], runs each tool call as the registry command it names, feeds
//! every result back, and repeats until the model answers without calling a
//! tool.
//!
//! Two things the loop owes the person, the same the agent API gives theirs:
//!
//! - **One undo step per turn.** Every change a turn makes joins one undo
//!   step named `label`, so the person undoes a turn at once however many
//!   commands it ran.
//! - **The same permission classes.** Each call's registered class goes to
//!   [`Commands::permit`], whose rules decide run or refuse; a refusal is
//!   returned to the model as that call's result rather than ending the
//!   turn. In the app the person is asked; headless and in tests,
//!   destructive and external calls are refused.
//!
//! A turn also ends early when the person cancels it (between calls,
//! keeping what ran) or when it has run its cap on calls, so a model that
//! keeps calling tools can't run away.

use bl_commands::{CommandSpec, Permission};
use serde_json::Value;

use crate::message::{Event, Message, Part, Reply, Request, Role, ToolResult};
use crate::provider::{Error, Provider};
use crate::tools::{ToolCall, ToolSet};

/// The most tool calls one turn runs unless the caller asks for another
/// count: a runaway model is stopped rather than left calling forever.
pub const DEFAULT_MAX_CALLS: usize = 32;

/// Where the loop's tool calls run, and who allows them.
///
/// The app backs it with the shell, inside the shell edit that opens the
/// turn's undo step; tests with an editor. The loop calls
/// [`Commands::begin_turn`] and [`Commands::end_turn`] once around a turn,
/// so everything it runs joins one undo step.
pub trait Commands {
    /// Opens the turn's changes as one undo step named `label`. Called once
    /// before any of the turn's calls.
    fn begin_turn(&mut self, label: &str);
    /// Closes the turn's undo step, keeping what ran, whatever ended the
    /// turn. Called once after the last of the turn's calls.
    fn end_turn(&mut self);
    /// Whether `command`, called with `args` and carrying the registered
    /// class `class`, may run now: `Ok` to run it, or why it's refused. A
    /// refusal is the call's result, which goes back to the model.
    fn permit(&mut self, command: &str, args: &Value, class: Permission) -> Result<(), String>;
    /// Runs `command` with `args`, returning its result, or why it failed.
    fn run(&mut self, command: &str, args: Value) -> Result<Value, String>;
}

/// One tool call the loop ran, and what came back: the command's result, or
/// the refusal or failure the model was told about.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    /// The call the model made.
    pub call: ToolCall,
    /// Its result, or why it didn't run.
    pub result: Result<Value, String>,
}

/// Why the loop ended a turn before the model answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stopped {
    /// The person cancelled the turn: it stopped between calls, keeping
    /// what ran.
    Cancelled,
    /// The turn reached its call cap.
    Cap,
}

/// One turn's answer and what the loop did to get it.
#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    /// The model's last reply. When [`Turn::stopped`] is set, this is the
    /// reply whose calls were cut short rather than a final answer.
    pub reply: Reply,
    /// Every tool call the loop ran, in order, with its result.
    pub steps: Vec<Step>,
    /// Why the loop ended early, or `None` when the model answered.
    pub stopped: Option<Stopped>,
}

/// The agent loop: the tools it may offer, the undo label it groups a turn
/// under, and the call cap.
pub struct Agent {
    tools: ToolSet,
    max_calls: usize,
    label: String,
}

impl Agent {
    /// The loop offering `commands` as the model's tools, with the default
    /// call cap and undo label.
    #[must_use]
    pub fn new<'a>(commands: impl IntoIterator<Item = &'a CommandSpec>) -> Self {
        Self {
            tools: ToolSet::new(commands),
            max_calls: DEFAULT_MAX_CALLS,
            label: DEFAULT_LABEL.to_owned(),
        }
    }

    /// The loop with a different cap on the tool calls one turn may run.
    #[must_use]
    pub fn with_max_calls(mut self, max_calls: usize) -> Self {
        self.max_calls = max_calls;
        self
    }

    /// The loop grouping its turns under a different undo-step label.
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Runs one turn to its end: sets `request`'s tools to the loop's, sends
    /// the conversation, runs each tool call through `commands`, appends the
    /// model's messages and their results to `request.messages`, and returns
    /// the turn once the model answers without a tool call, the person
    /// cancels (`cancel`), or the call cap is reached.
    ///
    /// `on_event` sees each text delta and finished tool call as the
    /// provider streams it, for a panel that shows the turn while it runs.
    ///
    /// # Errors
    ///
    /// [`Error`] if a provider turn failed. The turn's changes so far are
    /// still closed as one undo step.
    pub fn run(
        &self,
        provider: &dyn Provider,
        request: &mut Request,
        commands: &mut dyn Commands,
        cancel: &dyn Fn() -> bool,
        on_event: &mut dyn FnMut(Event),
    ) -> Result<Turn, Error> {
        request.tools = self.tools.tools().to_vec();
        commands.begin_turn(&self.label);
        let turn = self.turn(provider, request, commands, cancel, on_event);
        commands.end_turn();
        turn
    }

    fn turn(
        &self,
        provider: &dyn Provider,
        request: &mut Request,
        commands: &mut dyn Commands,
        cancel: &dyn Fn() -> bool,
        on_event: &mut dyn FnMut(Event),
    ) -> Result<Turn, Error> {
        let mut steps = Vec::new();
        loop {
            let reply = provider.stream(request, on_event)?;
            request.messages.push(assistant_message(&reply));
            if reply.tool_calls.is_empty() {
                return Ok(Turn {
                    reply,
                    steps,
                    stopped: None,
                });
            }
            let mut stopped = None;
            let mut results = Vec::new();
            for call in &reply.tool_calls {
                let content = if let Some(reason) = stopped {
                    // A call the loop stopped before: it still gets a result,
                    // so the conversation holds one for every call the model
                    // made and the next turn is well formed.
                    not_run(reason)
                } else if cancel() {
                    stopped = Some(Stopped::Cancelled);
                    not_run(Stopped::Cancelled)
                } else if steps.len() >= self.max_calls {
                    stopped = Some(Stopped::Cap);
                    not_run(Stopped::Cap)
                } else {
                    let result = self.call(commands, call);
                    let content = match &result {
                        Ok(value) => value.to_string(),
                        Err(message) => message.clone(),
                    };
                    steps.push(Step {
                        call: call.clone(),
                        result,
                    });
                    content
                };
                results.push(ToolResult {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    content,
                });
            }
            request.messages.push(Message::results(results));
            if let Some(stopped) = stopped {
                return Ok(Turn {
                    reply,
                    steps,
                    stopped: Some(stopped),
                });
            }
        }
    }

    /// Runs one tool call as the command it names, or reports why it didn't.
    fn call(&self, commands: &mut dyn Commands, call: &ToolCall) -> Result<Value, String> {
        let command = call
            .command(&self.tools)
            .map_err(|error| error.to_string())?;
        let class = self
            .tools
            .get(&call.name)
            .map_or(Permission::External, |tool| tool.permission);
        commands.permit(&command.command, &command.args, class)?;
        commands.run(&command.command, command.args)
    }
}

/// The undo-step label a turn gets unless the caller names another.
pub const DEFAULT_LABEL: &str = "Assistant";

/// The result of a call the loop stopped before running.
fn not_run(reason: Stopped) -> String {
    match reason {
        Stopped::Cancelled => "Not run: the person cancelled the turn.".to_owned(),
        Stopped::Cap => "Not run: the turn reached its call cap.".to_owned(),
    }
}

/// The model's reply as one assistant message: its text, then its calls, as
/// the conversation holds them.
fn assistant_message(reply: &Reply) -> Message {
    let mut parts = Vec::with_capacity(reply.tool_calls.len() + 1);
    if !reply.text.is_empty() {
        parts.push(Part::Text(reply.text.clone()));
    }
    parts.extend(reply.tool_calls.iter().cloned().map(Part::Call));
    Message {
        role: Role::Assistant,
        parts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_becomes_one_assistant_message() {
        let reply = Reply {
            text: "Adding a track.".to_owned(),
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "track_create".into(),
                arguments: serde_json::json!({}),
            }],
            stop: crate::message::Stop::End,
        };
        let message = assistant_message(&reply);
        assert_eq!(message.role, Role::Assistant);
        assert_eq!(message.text(), "Adding a track.");
        assert_eq!(message.parts.len(), 2);
        assert!(matches!(message.parts[1], Part::Call(_)));
    }

    #[test]
    fn a_reply_with_no_text_is_calls_only() {
        let reply = Reply {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "track_create".into(),
                arguments: serde_json::json!({}),
            }],
            stop: crate::message::Stop::End,
        };
        let message = assistant_message(&reply);
        assert_eq!(message.parts.len(), 1);
    }
}

//! The model's tools: a small fixed set that serves the whole command
//! registry, the same shape the agent API gives agent clients (the
//! architecture's "Agent control").
//!
//! One tool per command would give the model about a thousand tools, more
//! than a provider accepts (OpenAI answers a request with more than 128
//! functions with a 400) and a very large prompt on every request. The
//! model gets four: [`LIST_COMMANDS`] and [`DESCRIBE_COMMAND`] to find a
//! command and read its argument schema, [`RUN`] to run one by its ID, and
//! [`QUERY`] to read the project. A `run` call comes back as a [`Command`]
//! ([`ToolCall::command`]), which the agent loop runs through the same
//! permission classes as the agent API; the others it answers from the
//! host ([`Commands::read`](crate::agent::Commands::read)).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::provider::Error;

/// Lists the registry's areas, or the commands of one.
pub const LIST_COMMANDS: &str = "list_commands";
/// Describes one command: its description, permission class and argument
/// schema.
pub const DESCRIBE_COMMAND: &str = "describe_command";
/// Runs one command by its ID.
pub const RUN: &str = "run";
/// Reads the project by ID or readable path.
pub const QUERY: &str = "query";

/// What a tool call carries when its arguments were not JSON: the text the
/// model sent.
const UNPARSED: &str = "__unparsed_arguments";
/// Why the arguments did not parse, beside [`UNPARSED`].
const UNPARSED_ERROR: &str = "__error";

/// One tool, in the shape every provider's tool format is made from.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    /// The tool's name, which every provider's naming rules allow.
    pub name: String,
    /// What the tool does and when to use it, written for a model.
    pub description: String,
    /// The JSON Schema of the tool's arguments.
    pub input: Value,
}

impl ToolSpec {
    fn new(name: &str, description: &str, input: Value) -> Self {
        Self {
            name: name.to_owned(),
            description: description.to_owned(),
            input,
        }
    }
}

/// The tools the model is offered, in the order they are listed.
#[must_use]
pub fn tools() -> Vec<ToolSpec> {
    vec![
        ToolSpec::new(
            LIST_COMMANDS,
            "Lists Beatlayer's commands: every action a person can take in the app, which you \
             run with `run`. With no arguments it lists the areas (such as `track`, `clip`, \
             `mixer`, `song`, `set`) and how many commands each holds. With `area` or `search` \
             it lists the matching commands: each one's ID, the label a person sees, its \
             permission class (read, edit, destructive, external) and the first sentence of its \
             description. Use `describe_command` for a command's full description and argument \
             schema before running it.",
            json!({
                "type": "object",
                "properties": {
                    "area": {
                        "type": "string",
                        "description": "Only commands in this area: the part of a command's ID before the dot, such as `track` or `clip`."
                    },
                    "search": {
                        "type": "string",
                        "description": "Only commands whose ID, label or description holds every one of these words."
                    }
                }
            }),
        ),
        ToolSpec::new(
            DESCRIBE_COMMAND,
            "Describes one command: what it does and when to use it, its permission class, and \
             the JSON Schema of its arguments. Read it before running a command for the first \
             time.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The command's ID, such as `clip.create`." }
                },
                "required": ["command"]
            }),
        ),
        ToolSpec::new(
            RUN,
            "Runs one command by its ID, with the arguments its schema describes (see \
             `describe_command`). Everything you run in one turn is one undo step. A destructive \
             or external command asks the person first.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The command's ID, such as `clip.create`." },
                    "args": { "type": "object", "description": "The command's arguments. Leave it out for a command that takes none." }
                },
                "required": ["command"]
            }),
        ),
        ToolSpec::new(
            QUERY,
            "Reads the person's project by ID or by a readable path such as `tracks`, `tempo`, \
             `track \"Bass\"`, `track \"Bass\" / clip #1` or `device \"EQ\" / param 0`. With no \
             arguments it gives an overview of the whole song. Long lists come in pages.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "What to read, as a readable path." },
                    "id": { "type": "integer", "description": "What to read, by its ID: any track, clip, device or rack chain." },
                    "detail": { "type": "string", "enum": ["summary", "full"], "description": "How much to return. `summary` is the default." },
                    "offset": { "type": "integer", "description": "Where a long list starts, counting from 0." },
                    "limit": { "type": "integer", "description": "The most entries a list returns, from 1 to 500; 50 unless given." }
                }
            }),
        ),
    ]
}

/// A tool call the model made: a whole one, with its arguments parsed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// The call's ID, which its result names. Gemini's API gives its calls
    /// no ID, so the crate makes one.
    pub id: String,
    /// The tool's name: one of the four above.
    pub name: String,
    /// The arguments, as the model sent them.
    pub arguments: Value,
}

impl ToolCall {
    /// A call whose arguments weren't JSON. The call is kept rather than
    /// failing the stream, so the loop can tell the model what was wrong
    /// and run the reply's other calls.
    #[must_use]
    pub fn unparsed(id: String, name: String, raw: &str, error: &str) -> Self {
        Self {
            id,
            name,
            arguments: json!({ UNPARSED: raw, UNPARSED_ERROR: error }),
        }
    }

    /// Why the call's arguments weren't JSON, if they weren't.
    #[must_use]
    pub fn unparsed_because(&self) -> Option<&str> {
        self.arguments.get(UNPARSED)?;
        self.arguments.get(UNPARSED_ERROR)?.as_str()
    }

    /// The registry command a `run` call names, with the arguments it sent.
    ///
    /// # Errors
    ///
    /// If this isn't a `run` call, or it names no command: the loop reports
    /// it back to the model rather than running it.
    pub fn command(&self) -> Result<Command, Error> {
        if self.name != RUN {
            return Err(Error::Protocol(format!(
                "`{}` doesn't run a command; only `{RUN}` does",
                self.name
            )));
        }
        let command = self
            .arguments
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Error::Protocol(format!(
                    "`{RUN}` needs a `command`: the ID of the command to run"
                ))
            })?;
        Ok(Command {
            command: command.to_owned(),
            args: self
                .arguments
                .get("args")
                .cloned()
                .unwrap_or_else(|| json!({})),
        })
    }
}

/// A registry command named by a `run` call, with its arguments: what the
/// agent loop runs.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    /// The command's ID, such as `clip.create`.
    pub command: String,
    /// Its arguments, as the model sent them.
    pub args: Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixed_few_tools_serve_every_command() {
        let names: Vec<_> = tools().into_iter().map(|tool| tool.name).collect();
        assert_eq!(names, [LIST_COMMANDS, DESCRIBE_COMMAND, RUN, QUERY]);
    }

    #[test]
    fn a_run_call_resolves_to_its_command() {
        let call = ToolCall {
            id: "call_1".into(),
            name: RUN.into(),
            arguments: json!({ "command": "clip.create", "args": { "track": 1 } }),
        };
        let command = call.command().unwrap();
        assert_eq!(command.command, "clip.create");
        assert_eq!(command.args, json!({ "track": 1 }));
    }

    #[test]
    fn a_run_call_without_args_runs_with_none() {
        let call = ToolCall {
            id: "call_1".into(),
            name: RUN.into(),
            arguments: json!({ "command": "transport.play" }),
        };
        assert_eq!(call.command().unwrap().args, json!({}));
    }

    #[test]
    fn only_a_run_call_naming_a_command_is_a_command() {
        let no_command = ToolCall {
            id: "call_1".into(),
            name: RUN.into(),
            arguments: json!({}),
        };
        assert!(
            no_command
                .command()
                .unwrap_err()
                .to_string()
                .contains("command")
        );
        let other = ToolCall {
            id: "call_2".into(),
            name: QUERY.into(),
            arguments: json!({}),
        };
        assert!(other.command().unwrap_err().to_string().contains("query"));
    }

    #[test]
    fn an_unparsed_call_says_why() {
        let call = ToolCall::unparsed("c".into(), RUN.into(), "{,}", "expected value");
        assert_eq!(call.unparsed_because(), Some("expected value"));
        let whole = ToolCall {
            id: "c".into(),
            name: RUN.into(),
            arguments: json!({ "command": "a.b" }),
        };
        assert_eq!(whole.unparsed_because(), None);
    }
}

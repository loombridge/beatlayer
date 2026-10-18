//! The registry's commands as the model's tools.
//!
//! A feature that exists for a person exists for an agent (the
//! architecture's "Agent control"), so the assistant doesn't get a tool set
//! of its own: every command's ID, description, and argument schema is a
//! tool the model may call. A call comes back as a [`ToolCall`], and
//! [`ToolCall::command`] resolves it to the registry command it names,
//! which the agent loop then runs.
//!
//! Every provider's naming rules are narrower than a command's ID
//! (`clip.create`): a dot isn't allowed. The tool's name is the command's ID
//! with dots as underscores, and a command whose ID would give the same
//! tool name as another is a registry error.

use std::collections::BTreeMap;

use bl_commands::{CommandSpec, Permission};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::provider::Error;

/// One command, in the shape every provider's tool format is made from.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    /// The tool's name: the command's ID with dots as underscores, which
    /// every provider's naming rules allow.
    pub name: String,
    /// The command's ID, such as `clip.create`.
    pub command: String,
    /// What the command does, when to use it, and what it changes: the
    /// registry's description, written for a model.
    pub description: String,
    /// The command's registered permission class, which the agent loop
    /// passes to its [`Commands`](crate::agent::Commands) when it asks
    /// whether the call may run. A command that names none is external,
    /// the most a command can need, as the agent API reads it.
    pub permission: Permission,
    /// The JSON Schema of the command's arguments.
    pub input: Value,
}

impl ToolSpec {
    /// The tool for one command.
    #[must_use]
    pub fn new(spec: &CommandSpec) -> Self {
        Self {
            name: tool_name(&spec.id),
            command: spec.id.to_string(),
            description: spec.description.to_string(),
            permission: spec.permission.unwrap_or(Permission::External),
            input: spec
                .args
                .as_ref()
                .and_then(|args| serde_json::to_value(args).ok())
                .unwrap_or_else(|| json!({ "type": "object" })),
        }
    }
}

/// The tool name for a command's ID.
#[must_use]
pub fn tool_name(id: &str) -> String {
    id.replace('.', "_")
}

/// The model's tools for a set of commands, and the command each tool name
/// runs.
#[derive(Clone, Debug, Default)]
pub struct ToolSet {
    tools: Vec<ToolSpec>,
    by_name: BTreeMap<String, usize>,
}

impl ToolSet {
    /// The tools for `commands`: one tool each, in the order given.
    ///
    /// # Panics
    ///
    /// If two commands share a tool name: their IDs differ only in a dot
    /// where the other has an underscore, which the command registry's own
    /// rules make a mistake to fix, not something to carry on with.
    #[must_use]
    pub fn new<'a>(commands: impl IntoIterator<Item = &'a CommandSpec>) -> Self {
        let mut tools = Vec::new();
        let mut by_name = BTreeMap::new();
        for spec in commands {
            let tool = ToolSpec::new(spec);
            if let Some(seen) = by_name.get(&tool.name) {
                let other: &ToolSpec = &tools[*seen];
                panic!(
                    "the commands `{}` and `{}` share the tool name `{}`",
                    other.command, tool.command, tool.name
                );
            }
            by_name.insert(tool.name.clone(), tools.len());
            tools.push(tool);
        }
        Self { tools, by_name }
    }

    /// The tools, in the order their commands came.
    #[must_use]
    pub fn tools(&self) -> &[ToolSpec] {
        &self.tools
    }

    /// The tool a name refers to, if one does.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ToolSpec> {
        self.by_name.get(name).map(|index| &self.tools[*index])
    }

    /// Whether there are no tools.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

/// A tool call the model made: a whole one, with its arguments parsed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// The call's ID, which its result names. Gemini's API gives its calls
    /// no ID, so the crate makes one.
    pub id: String,
    /// The tool's name ([`ToolSpec::name`]).
    pub name: String,
    /// The arguments, as the model sent them.
    pub arguments: Value,
}

impl ToolCall {
    /// The registry command this call names, with the arguments it sent.
    ///
    /// # Errors
    ///
    /// If the tool set has no tool called [`ToolCall::name`]: the model
    /// named something it wasn't offered, which the loop reports back to it
    /// rather than running.
    pub fn command(&self, tools: &ToolSet) -> Result<Command, Error> {
        let tool = tools.get(&self.name).ok_or_else(|| {
            Error::Protocol(format!(
                "the model called a tool called `{}`, which it wasn't offered",
                self.name
            ))
        })?;
        Ok(Command {
            command: tool.command.clone(),
            args: self.arguments.clone(),
        })
    }
}

/// A registry command named by a tool call, with its arguments: what the
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
    use bl_commands::{NoArgs, Registry};

    fn set() -> ToolSet {
        let mut registry = Registry::new();
        registry
            .register(
                CommandSpec::new("clip.create", "Create Clip")
                    .description("Adds a clip. Undo removes it.")
                    .args::<NoArgs>(),
            )
            .unwrap();
        ToolSet::new(registry.iter())
    }

    #[test]
    fn a_command_becomes_a_tool() {
        let set = set();
        let tool = &set.tools()[0];
        assert_eq!(tool.name, "clip_create");
        assert_eq!(tool.command, "clip.create");
        assert_eq!(tool.description, "Adds a clip. Undo removes it.");
        assert_eq!(tool.input["type"], "object");
    }

    #[test]
    fn a_call_resolves_to_its_command() {
        let set = set();
        let call = ToolCall {
            id: "call_1".into(),
            name: "clip_create".into(),
            arguments: json!({ "track": 1 }),
        };
        let command = call.command(&set).unwrap();
        assert_eq!(command.command, "clip.create");
        assert_eq!(command.args, json!({ "track": 1 }));
    }

    #[test]
    fn a_call_to_an_offered_tool_only() {
        let set = set();
        let call = ToolCall {
            id: "call_1".into(),
            name: "clip_duplicate".into(),
            arguments: json!({}),
        };
        let error = call.command(&set).unwrap_err().to_string();
        assert!(error.contains("clip_duplicate"), "{error}");
    }

    #[test]
    #[should_panic(expected = "share the tool name")]
    fn two_commands_with_one_tool_name_are_a_registry_error() {
        let mut registry = Registry::new();
        registry
            .register(CommandSpec::new("clip.create", "Create Clip").args::<NoArgs>())
            .unwrap();
        registry
            .register(CommandSpec::new("clip_create", "Trap").args::<NoArgs>())
            .unwrap();
        let _ = ToolSet::new(registry.iter());
    }
}

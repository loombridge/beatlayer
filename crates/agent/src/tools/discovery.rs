//! Discovery: list commands by area or find them by words, and describe one
//! with its schemas.

use std::collections::BTreeMap;

use bl_commands::CommandSpec;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Ctx, ToolOutcome, args, done, failed, step, tool_definition};
use crate::permission::{Decision, decide};

pub(crate) const LIST: &str = "list_commands";
pub(crate) const DESCRIBE: &str = "describe_command";

/// Arguments of `list_commands`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListArgs {
    /// Only commands in this area: the part of a command's ID before the
    /// dot, such as `track`, `clip`, `mixer`, `device`, `song`, or `set`.
    #[serde(default)]
    area: Option<String>,
    /// Only commands whose ID, label, or description holds every one of
    /// these words, best matches first.
    #[serde(default)]
    search: Option<String>,
}

/// Arguments of `describe_command`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DescribeArgs {
    /// The command's ID, such as `clip.create`.
    command: String,
}

pub(crate) fn list_definition() -> Value {
    tool_definition::<ListArgs>(
        LIST,
        "List commands",
        "Lists Beatlayer's commands: every action a person can take in the app, which agents \
         run through `run`. With no arguments it lists the areas (such as `track`, `clip`, \
         `mixer`, `song`, `set`) and how many commands each holds. With `area` or `search` it \
         lists the matching commands: each one's ID, the label a person sees, its permission \
         class (read, edit, destructive, external), whether running it asks the person first, \
         and the first sentence of its description. Use `describe_command` for a command's \
         full description and argument schema before running it.",
        true,
    )
}

pub(crate) fn describe_definition() -> Value {
    tool_definition::<DescribeArgs>(
        DESCRIBE,
        "Describe a command",
        "Describes one command: what it does and when to use it, its permission class and \
         whether it asks the person first, the parts of the interface it belongs to, and the \
         JSON Schemas of its arguments and its result. Read it before running a command for \
         the first time.",
        true,
    )
}

/// Whether running a command would ask the person first, or be refused.
fn access(ctx: &Ctx<'_>, spec: &CommandSpec) -> &'static str {
    let class = spec.permission.unwrap_or(bl_commands::Permission::External);
    match decide(
        ctx.mode,
        ctx.shell,
        &ctx.session.client.name,
        class,
        &[spec.id.as_ref()],
    ) {
        Decision::Run => "runs",
        Decision::Ask => "asks the person first",
        Decision::Refuse(_) => "refused to agents",
    }
}

/// The first sentence of a description.
fn summary(description: &str) -> &str {
    description
        .find(". ")
        .map_or(description, |end| &description[..=end])
}

pub(crate) fn list(ctx: &mut Ctx<'_>, raw: Value) -> ToolOutcome {
    let ListArgs { area, search } = step!(args(raw));
    let registry = ctx.shell.registry();
    if area.is_none() && search.is_none() {
        let mut areas: BTreeMap<&str, usize> = BTreeMap::new();
        for spec in registry {
            *areas.entry(spec.area()).or_default() += 1;
        }
        return done(json!({
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
            let id = spec.id.to_lowercase();
            let label = spec.label.to_lowercase();
            let description = spec.description.to_lowercase();
            let mut score = 0;
            for word in &words {
                if id.contains(word.as_str()) || label.contains(word.as_str()) {
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
    found.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.id.cmp(&b.1.id)));
    if found.is_empty() {
        return failed(match (&area, &search) {
            (Some(area), None) => format!(
                "no commands are in the area `{area}`; call list_commands with no arguments \
                 for the areas"
            ),
            _ => "no commands match; try fewer or other words".to_owned(),
        });
    }
    let commands: Vec<Value> = found
        .iter()
        .map(|(_, spec)| {
            json!({
                "command": spec.id,
                "label": spec.label,
                "permission": spec.permission,
                "access": access(ctx, spec),
                "summary": summary(&spec.description),
            })
        })
        .collect();
    done(json!({ "commands": commands }))
}

pub(crate) fn describe(ctx: &mut Ctx<'_>, raw: Value) -> ToolOutcome {
    let DescribeArgs { command } = step!(args(raw));
    let Some(spec) = ctx.shell.registry().get(&command) else {
        return failed(format!(
            "no command is called `{command}`; use list_commands to find one"
        ));
    };
    let mut described = serde_json::to_value(spec).unwrap_or(Value::Null);
    if let Some(object) = described.as_object_mut() {
        object.insert("access".to_owned(), json!(access(ctx, spec)));
        if let Some(permission) = spec.permission {
            object.insert("permission_meaning".to_owned(), json!(permission.meaning()));
        }
        // Bindings name the shortcuts that run it, which an agent can't press.
        object.remove("bindings");
    }
    done(described)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_is_the_first_sentence() {
        assert_eq!(summary("Does a thing. Then more."), "Does a thing.");
        assert_eq!(summary("Only one."), "Only one.");
    }
}

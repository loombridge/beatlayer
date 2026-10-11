//! Run: one command, or a batch of document edits as a single undo step
//! labelled with the agent's name, or a dry run that checks them and
//! changes nothing.
//!
//! A command that creates an object needs a new ID. Rather than ask for one
//! first, an agent writes `{"$new": "name"}` where the ID goes, and
//! `{"$ref": "name"}` wherever a later command (in the same call or a later
//! one) refers to it. The result maps each name to the ID it got.

use std::collections::BTreeMap;

use bl_commands::{CommandSpec, Permission};
use bl_core::id::Id;
use bl_model::ChangeSet;
use bl_model::registry::{self as model, REDO, UNDO};
use bl_shell::permission::{CallPermission, raised_by_arguments};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{
    Ctx, ToolOutcome, args, class_names, done, failed, question_line, step, tool_definition,
};

pub(crate) const NAME: &str = "run";

/// Arguments of `run`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunArgs {
    /// The commands to run, in order. More than one must all be document
    /// edits (commands in the `song`, `track`, `mixer`, `clip`, and `device`
    /// areas, and the loop's `transport.set_loop` and
    /// `transport.set_loop_enabled`); they run as one undo step, all or
    /// none.
    #[schemars(length(min = 1))]
    commands: Vec<Step>,
    /// What the commands do together, in a few words, such as "Add a drum
    /// track". It names the undo step after the agent's name. Without it the
    /// step is named after the commands.
    #[serde(default)]
    label: Option<String>,
    /// Check the commands without changing anything: their arguments, the
    /// edits applied in turn to the current document and then undone, and
    /// whether running them would ask the person first. Commands that
    /// aren't document edits are checked against their schemas only.
    #[serde(default)]
    dry_run: bool,
}

/// One command in a `run` call.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Step {
    /// The command's ID, such as `track.create`.
    command: String,
    /// Its arguments, as `describe_command` gives their schema. Where a new
    /// object's ID goes, write `{"$new": "name"}` and the server issues one;
    /// refer to it in later commands, in this call or later ones, with
    /// `{"$ref": "name"}`.
    #[serde(default = "empty_object")]
    #[schemars(with = "Map<String, Value>")]
    args: Value,
}

fn empty_object() -> Value {
    json!({})
}

pub(crate) fn definition() -> Value {
    tool_definition::<RunArgs>(
        NAME,
        "Run commands",
        "Runs one command, or a batch of document edits as a single undo step that the \
         person can undo, labelled with your name. Each command is named by its ID with its \
         arguments as its schema describes them (see describe_command). New objects need new \
         IDs: write {\"$new\": \"name\"} where the ID goes and {\"$ref\": \"name\"} to refer to \
         it later; the result maps each name to its ID. A batch runs all or nothing: if a \
         command fails, nothing changes and the error names it. Set `dry_run` to check the \
         commands against the current project without changing anything. Destructive and \
         external commands ask the person first unless they've turned on autonomous mode for \
         you, and wait for the answer.",
        false,
    )
}

/// Whether a command is a document edit, which can run in a batch.
fn is_edit(id: &str) -> bool {
    model::handles(id) && id != UNDO && id != REDO
}

/// Replaces `{"$new": name}` and `{"$ref": name}` in `value`, issuing new
/// IDs with `issue`. Every `$new` is replaced first, so a `$ref` may name
/// an ID made anywhere in the same arguments.
pub(crate) fn resolve_placeholders(
    value: &mut Value,
    names: &mut BTreeMap<String, u64>,
    issue: &mut dyn FnMut() -> u64,
) -> Result<(), String> {
    replace(value, names, issue, "$new")?;
    replace(value, names, issue, "$ref")
}

/// Replaces the placeholders with the key `kind` (`$new` or `$ref`).
fn replace(
    value: &mut Value,
    names: &mut BTreeMap<String, u64>,
    issue: &mut dyn FnMut() -> u64,
    kind: &str,
) -> Result<(), String> {
    match value {
        Value::Object(object) if object.len() == 1 && object.contains_key(kind) => {
            let Some(Value::String(name)) = object.get(kind) else {
                return Err(format!("`{kind}` takes a name, as a string"));
            };
            let id = if kind == "$new" {
                if names.contains_key(name) {
                    return Err(format!(
                        "`{name}` already names an ID; give each new object its own name"
                    ));
                }
                let id = issue();
                names.insert(name.clone(), id);
                id
            } else {
                *names.get(name).ok_or_else(|| {
                    format!("`{name}` names no ID; make one with {{\"$new\": \"{name}\"}} first")
                })?
            };
            *value = json!(id);
            Ok(())
        }
        Value::Object(object) => object
            .values_mut()
            .try_for_each(|inner| replace(inner, names, issue, kind)),
        Value::Array(items) => items
            .iter_mut()
            .try_for_each(|inner| replace(inner, names, issue, kind)),
        _ => Ok(()),
    }
}

/// A command looked up and its arguments made ready.
struct Ready {
    id: String,
    args: Value,
    /// What this call needs, decided from its arguments.
    call: CallPermission,
}

/// The name of a batch's undo step when the agent gave none.
fn default_label(specs: &[&CommandSpec]) -> String {
    match specs {
        [] => String::new(),
        [one] => one.label.to_string(),
        [first, second] => format!("{} and {}", first.label, second.label),
        [first, second, rest @ ..] => {
            format!("{}, {}, and {} more", first.label, second.label, rest.len())
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the checks, the permission, and the three ways to run read best in order"
)]
pub(crate) fn run(ctx: &mut Ctx<'_>, raw: Value) -> ToolOutcome {
    let RunArgs {
        commands,
        label,
        dry_run,
    } = step!(args(raw));
    if commands.is_empty() {
        return failed("`commands` is empty: give at least one command to run");
    }

    // Look every command up first, so nothing runs if one is wrong.
    let mut specs: Vec<&CommandSpec> = Vec::with_capacity(commands.len());
    for (index, step) in commands.iter().enumerate() {
        let Some(spec) = super::command(ctx.shell, &step.command) else {
            return failed(format!(
                "command {} (`{}`): no command has that ID; use list_commands to find one",
                index + 1,
                step.command
            ));
        };
        specs.push(spec);
    }
    if commands.len() > 1
        && let Some(other) = commands.iter().find(|step| !is_edit(&step.command))
    {
        return failed(format!(
            "a batch can hold only document edits (the `song`, `track`, `mixer`, `clip`, and \
             `device` areas, and the loop); run `{}` in a call of its own",
            other.command
        ));
    }
    let label = label
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| default_label(&specs));
    let schemas: Vec<Option<bl_commands::schemars::Schema>> =
        specs.iter().map(|spec| spec.args.clone()).collect();
    let permissions: Vec<Permission> = specs
        .iter()
        .map(|spec| spec.permission.unwrap_or(Permission::External))
        .collect();
    let labels: Vec<String> = specs.iter().map(|spec| spec.label.to_string()).collect();

    // Issue IDs and check every command's arguments against its schema.
    let mut names = ctx.session.refs.clone();
    let mut ready = Vec::with_capacity(commands.len());
    for (index, (step, schema)) in commands.into_iter().zip(&schemas).enumerate() {
        let mut args = step.args;
        let shell = &mut *ctx.shell;
        let mut issue = || shell.new_id::<Id>().get();
        if let Err(message) = resolve_placeholders(&mut args, &mut names, &mut issue) {
            return failed(format!(
                "command {} (`{}`): {message}",
                index + 1,
                step.command
            ));
        }
        if let Some(schema) = schema
            && let Err(message) = bl_commands::validate(schema, &args)
        {
            return failed(format!(
                "command {} (`{}`): invalid arguments: {message}",
                index + 1,
                step.command
            ));
        }
        // Spec: agent-permissions R1 to R3. Each call's class is decided
        // from its arguments against the set as it is now.
        let call = if raised_by_arguments(&step.command) {
            ctx.shell.call_permission(&step.command, &args)
        } else {
            CallPermission::registered(permissions[index])
        };
        ready.push(Ready {
            id: step.command,
            args,
            call,
        });
    }
    let new_ids: BTreeMap<&String, &u64> = names
        .iter()
        .filter(|(name, _)| !ctx.session.refs.contains_key(*name))
        .collect();
    let new_ids = json!(new_ids);

    // Who decides: the person may need to be asked, or it may be refused.
    let mut classes: Vec<Permission> = ready.iter().flat_map(|step| step.call.classes()).collect();
    classes.sort_unstable();
    classes.dedup();
    let class = classes
        .iter()
        .copied()
        .max()
        .unwrap_or(Permission::External);
    let ids: Vec<&str> = ready.iter().map(|step| step.id.as_str()).collect();
    let decision = ctx.decide(class, &ids);
    if !dry_run {
        let shell = &*ctx.shell;
        step!(ctx.permit(&classes, &ids, || {
            let lines: Vec<String> = ready
                .iter()
                .zip(&labels)
                .filter(|(step, _)| step.call.class().asks_first())
                .map(|(step, label)| question_line(shell, label, &step.call, &step.args))
                .collect();
            lines.join("\n")
        }));
    }

    if dry_run {
        let would = match &decision {
            crate::permission::Decision::Run => "run",
            crate::permission::Decision::Ask => "ask the person first",
            crate::permission::Decision::Refuse(_) => "be refused",
        };
        let refused = match decision {
            crate::permission::Decision::Refuse(message) => Some(message),
            _ => None,
        };
        let results: Result<Vec<Value>, String> = if ready.iter().all(|r| is_edit(&r.id)) {
            ctx.shell.edit(|editor| {
                editor.begin("Dry run");
                let mut results = Vec::with_capacity(ready.len());
                let mut failure = None;
                for (index, step) in ready.iter().enumerate() {
                    match model::run(editor, &step.id, step.args.clone()) {
                        Ok(changes) => {
                            let mut checked = json!({ "command": step.id, "changes": changes });
                            // Spec: agent-permissions R11. `clip.set_data`
                            // says whether it would delete notes.
                            if raised_by_arguments(&step.id) {
                                checked["permission"] = json!(class_names(&step.call.classes()));
                                if let Some(fewer) = step.call.fewer_notes {
                                    checked["fewer_notes"] = json!({
                                        "clip": fewer.clip,
                                        "has": fewer.has,
                                        "leaves": fewer.leaves,
                                    });
                                }
                            }
                            results.push(checked);
                        }
                        Err(error) => {
                            failure =
                                Some(format!("command {} (`{}`): {error}", index + 1, step.id));
                            break;
                        }
                    }
                }
                editor.cancel().ok();
                failure.map_or(Ok(results), Err)
            })
        } else {
            Ok(ready
                .iter()
                .map(|step| {
                    let mut checked = json!({
                        "command": step.id,
                        "note": "Its arguments fit its schema. It isn't a document edit, so it \
                                 wasn't tried.",
                    });
                    if raised_by_arguments(&step.id) {
                        checked["permission"] = json!(class_names(&step.call.classes()));
                        checked["replaces"] = json!(step.call.replaces);
                        if step.call.may && !step.call.replaces.is_empty() {
                            checked["note"] = json!(
                                "Its arguments fit its schema. It wasn't tried. It may replace \
                                 part of the clips in `replaces`: how much depends on its files' \
                                 lengths, which are read only when it runs."
                            );
                        } else {
                            checked["note"] = json!(
                                "Its arguments fit its schema, and it was tried on the song as \
                                 it is and undone. `replaces` lists the clips it would delete or \
                                 shorten by placing clips over them."
                            );
                        }
                    }
                    checked
                })
                .collect())
        };
        return done(json!({
            "dry_run": true,
            "valid": results.is_ok(),
            "error": results.as_ref().err(),
            "would": would,
            "permission": class_names(&classes),
            "refused_because": refused,
            "results": results.unwrap_or_default(),
            "ids": new_ids,
            "note": "Nothing changed. IDs shown for $new names are examples; a real run issues \
                     its own.",
        }));
    }

    // Run them.
    let outcome: Result<(Vec<Value>, Option<String>), String> =
        if ready.len() == 1 && super::is_engine_command(ctx.shell, &ready[0].id) {
            // A transport command goes to the engine, through the host.
            let step = &ready[0];
            super::time::send_transport(ctx, &step.id, step.args.clone())
                .map(|()| {
                    let sent = json!({
                        "command": step.id,
                        "result": null,
                        "note": "Sent to the engine, which acts on it at the start of its next \
                                 block.",
                    });
                    (vec![sent], None)
                })
                .map_err(|error| format!("command 1 (`{}`): {error}", step.id))
        } else if ready.len() == 1 && !is_edit(&ready[0].id) {
            // Spec: agent-permissions R6. An edit the shell runs (the
            // Arrangement's, a drop from the browser) is an undo step
            // labelled with the agent's name too.
            let step = &ready[0];
            let step_label = ctx.step_label(&label);
            let before = ctx.shell.editor().document().clone();
            ctx.shell
                .run_labelled(step_label.clone(), &step.id, step.args.clone())
                .map(|result| {
                    // An undo step only if this call recorded one: a
                    // command that replaces the set (a new or opened one)
                    // records none.
                    let editor = ctx.shell.editor();
                    let recorded = !ChangeSet::between(&before, editor.document()).is_empty()
                        && editor.undo_history().next() == Some(step_label.as_str());
                    (
                        vec![json!({ "command": step.id, "result": result })],
                        recorded.then_some(step_label),
                    )
                })
                .map_err(|error| format!("command 1 (`{}`): {error}", step.id))
        } else {
            let step_label = ctx.step_label(&label);
            let undo_label = step_label.clone();
            ctx.shell.edit(|editor| {
                editor.transact(step_label, |editor| {
                    let start = editor.document().clone();
                    let mut results = Vec::with_capacity(ready.len());
                    for (index, step) in ready.iter().enumerate() {
                        let changes =
                            model::run(editor, &step.id, step.args.clone()).map_err(|error| {
                                format!("command {} (`{}`): {error}", index + 1, step.id)
                            })?;
                        results.push(json!({ "command": step.id, "result": changes }));
                    }
                    // A batch that leaves the song as it was records no undo
                    // step.
                    let changed = !ChangeSet::between(&start, editor.document()).is_empty();
                    Ok((results, changed.then_some(undo_label)))
                })
            })
        };
    match outcome {
        Ok((results, undo_step)) => {
            ctx.session.refs = names;
            let asked = ready.iter().any(|step| step.call.class().asks_first());
            done(json!({
                "ran": results.len(),
                "undo_step": undo_step,
                "asked_first": asked && ctx.confirmed,
                "results": results,
                "ids": new_ids,
            }))
        }
        Err(message) => failed(format!("{message}. Nothing changed.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_issue_ids_and_refer_back() {
        let mut next = 100;
        let mut issue = || {
            next += 1;
            next
        };
        let mut names = BTreeMap::new();
        // A reference may come before the ID it names in the arguments.
        let mut args = json!({
            "after": {"$ref": "bass"},
            "id": {"$new": "bass"},
            "list": [{"$new": "eq"}],
            "name": "$new",
        });
        resolve_placeholders(&mut args, &mut names, &mut issue).unwrap();
        assert_eq!(
            args,
            json!({
                "after": 101,
                "id": 101,
                "list": [102],
                "name": "$new",
            })
        );
        let mut again = json!({"id": {"$new": "bass"}});
        assert!(resolve_placeholders(&mut again, &mut names, &mut issue).is_err());
        let mut unknown = json!([{"$ref": "drums"}]);
        assert!(resolve_placeholders(&mut unknown, &mut names, &mut issue).is_err());
        let mut wrong = json!({"$new": 3});
        assert!(resolve_placeholders(&mut wrong, &mut names, &mut issue).is_err());
    }

    #[test]
    fn batches_are_named_after_their_commands() {
        let a = CommandSpec::new("a.b", "Insert Track");
        let b = CommandSpec::new("a.c", "Create Clip");
        let c = CommandSpec::new("a.d", "Set Volume");
        assert_eq!(default_label(&[&a]), "Insert Track");
        assert_eq!(default_label(&[&a, &b]), "Insert Track and Create Clip");
        assert_eq!(
            default_label(&[&a, &b, &c, &c]),
            "Insert Track, Create Clip, and 2 more"
        );
    }
}

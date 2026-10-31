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
use bl_model::registry::{self as model, GO_TO_STEP, REDO, UNDO};
use bl_shell::permission::{CallPermission, raised_by_arguments};
pub(crate) use bl_shell::placeholders::resolve_placeholders;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{
    Ctx, ToolOutcome, args, class_names, done, failed, question_line, step, tool_definition,
};
use crate::server::{Deferred, ExportCall};

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
    model::handles(id) && id != UNDO && id != REDO && id != GO_TO_STEP
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
        // Spec: warping R7, R11, R23 to R27, R39 to R41. An edit is checked
        // against the media and filled in from them, as the shell does when
        // it runs one.
        if is_edit(&step.command) {
            match ctx.shell.prepare_edit(&step.command, args) {
                Ok(prepared) => args = prepared,
                Err(error) => {
                    return failed(format!(
                        "command {} (`{}`): {error}",
                        index + 1,
                        step.command
                    ));
                }
            }
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
                .map(|(step, label)| question_line(shell, &step.id, label, &step.call, &step.args))
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
                            // says whether it would delete notes, and
                            // `clip.crop` which clip it would take audio
                            // from (audio-clip-editing R18).
                            if raised_by_arguments(&step.id) {
                                checked["permission"] = json!(class_names(&step.call.classes()));
                                if let Some(fewer) = step.call.fewer_notes {
                                    checked["fewer_notes"] = json!({
                                        "clip": fewer.clip,
                                        "has": fewer.has,
                                        "leaves": fewer.leaves,
                                    });
                                }
                                if let Some(clip) = step.call.cropped {
                                    checked["crop"] = json!({ "clip": clip });
                                }
                                if let Some(track) = step.call.deletes_track {
                                    checked["deletes_track"] = json!(track);
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
                        // The MIDI Tools' calls are the shell's, not the
                        // document's, and say it the same way
                        // `clip.set_data` does (the MIDI Tools spec, R24).
                        if let Some(fewer) = step.call.fewer_notes {
                            checked["fewer_notes"] = json!({
                                "clip": fewer.clip,
                                "has": fewer.has,
                                "leaves": fewer.leaves,
                            });
                        }
                        if let Some(clip) = step.call.cropped {
                            checked["crop"] = json!({ "clip": clip });
                        }
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

    // Spec: export R44. In the app an export runs on its own thread and the
    // call waits for it, so the progress window draws and `export.status`
    // and `export.cancel` answer meanwhile; headless, it runs to its end.
    if ctx.view_capture && ready.len() == 1 && ready[0].id == bl_shell::export_commands::AUDIO {
        let step = &ready[0];
        return match ctx.shell.start_export(step.args.clone()) {
            Ok(()) => {
                ctx.session.refs = names;
                ToolOutcome::Defer(Deferred::Export(ExportCall {
                    asked_first: step.call.class().asks_first() && ctx.confirmed,
                    ids: new_ids,
                }))
            }
            Err(error) => failed(format!(
                "command 1 (`{}`): {error}. Nothing changed.",
                step.id
            )),
        };
    }

    // Run them.
    let outcome: Result<(Vec<Value>, Option<String>), String> =
        if ready.len() == 1 && super::is_engine_command(ctx.shell, &ready[0].id) {
            // A transport or Session View command goes to the engine,
            // through the host.
            let step = &ready[0];
            let result = if bl_engine::transport::handles(&step.id) {
                super::time::send_transport(ctx, &step.id, step.args.clone())
            } else {
                super::time::send_session(ctx, &step.id, step.args.clone())
            };
            result
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
                .run_labelled(ctx.maker(), step_label.clone(), &step.id, step.args.clone())
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
            // Spec: macros R6, R7, R11 and R13. Edits go through the shell,
            // as [`Shell::run`] does, so a macro's follow-ups are applied
            // through the agent API too.
            let step_label = ctx.step_label(&label);
            let undo_label = step_label.clone();
            let steps: Vec<(String, Value)> = ready
                .iter()
                .map(|step| (step.id.clone(), step.args.clone()))
                .collect();
            ctx.shell
                .run_document_edits(ctx.maker(), &step_label, &steps)
                .map(|(results, changed)| (results, changed.then_some(undo_label)))
                .map_err(|error| error.to_string())
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

//! Checkpoints: named versions of the document an agent can go back to.
//!
//! The document is an immutable tree, so a checkpoint is a cheap snapshot
//! that shares every node with the live document. Restoring one is a single
//! undo step labelled with the agent's name, so the person can undo it. A
//! restore that would remove tracks, clips, or devices that exist now is
//! destructive and asks the person first; one that only adds or changes is
//! an edit. Checkpoints belong to the project for as long as the agent API
//! runs; they aren't saved with the set.

use std::time::{SystemTime, UNIX_EPOCH};

use bl_commands::Permission;
use bl_model::{Change, ChangeSet, Document};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Ctx, ToolOutcome, args, done, failed, step, tool_definition};

pub(crate) const NAME: &str = "checkpoint";

/// The most checkpoints kept at once.
pub(crate) const MAX: usize = 100;

/// What `checkpoint` does.
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Action {
    /// Save the document as it is now, under `name`, replacing any
    /// checkpoint with that name.
    Save,
    /// Put the document back as checkpoint `name` has it, as one undo step.
    Restore,
    /// List the checkpoints.
    List,
    /// Forget checkpoint `name`.
    Delete,
}

/// Arguments of `checkpoint`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CheckpointArgs {
    /// What to do.
    action: Action,
    /// The checkpoint's name, for `save`, `restore`, and `delete`.
    #[serde(default)]
    name: Option<String>,
}

pub(crate) fn definition() -> Value {
    tool_definition::<CheckpointArgs>(
        NAME,
        "Checkpoints",
        "Saves and restores named checkpoints of the whole song, so you can try a variation \
         and go back. A checkpoint costs almost nothing. `save` keeps the song as it is now \
         under `name`; `restore` puts it back as one undo step, which the person can undo; \
         `list` shows them; `delete` forgets one. A restore that would remove tracks, clips, \
         or devices that exist now asks the person first, unless you're autonomous. \
         Checkpoints last while Beatlayer runs, aren't saved in the set, and restore only in \
         the set they were saved in.",
        false,
    )
}

/// One checkpoint.
#[derive(Clone, Debug)]
struct Checkpoint {
    name: String,
    document: Document,
    saved_at: u64,
    set: String,
    /// The set it was saved in ([`bl_shell::Shell::set_generation`]): a
    /// restore replaces the song, which is only right in that set.
    generation: u64,
    by: String,
}

/// The checkpoints, in the order they were saved.
#[derive(Debug, Default)]
pub(crate) struct Checkpoints {
    items: Vec<Checkpoint>,
}

impl Checkpoints {
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    fn get(&self, name: &str) -> Option<&Checkpoint> {
        self.items.iter().find(|c| c.name == name)
    }

    pub(crate) fn list(&self) -> Value {
        let items: Vec<Value> = self
            .items
            .iter()
            .map(|c| {
                json!({
                    "name": c.name,
                    "saved_at_unix": c.saved_at,
                    "set": c.set,
                    "by": c.by,
                    "tracks": c.document.track_count(),
                    "clips": c.document.clip_count(),
                    "devices": c.document.device_count(),
                })
            })
            .collect();
        json!({ "checkpoints": items })
    }
}

/// Counts what a change set adds, removes, and changes.
fn tally(changes: &ChangeSet) -> Value {
    let count = |change: Change| {
        changes.tracks.values().filter(|c| **c == change).count()
            + changes.clips.values().filter(|c| **c == change).count()
            + changes.devices.values().filter(|c| **c == change).count()
    };
    json!({
        "added": count(Change::Added),
        "removed": count(Change::Removed),
        "modified": count(Change::Modified),
        "song_settings": changes.settings,
    })
}

pub(crate) fn checkpoint(ctx: &mut Ctx<'_>, raw: Value) -> ToolOutcome {
    let CheckpointArgs { action, name } = step!(args(raw));
    let name = name.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty());
    let need_name = || failed("give the checkpoint a `name`");
    match action {
        Action::List => done(ctx.state.checkpoints.list()),
        Action::Save => {
            let Some(name) = name else { return need_name() };
            let items = &mut ctx.state.checkpoints.items;
            let replaced = items.iter().position(|c| c.name == name);
            if replaced.is_none() && items.len() >= MAX {
                return failed(format!(
                    "{MAX} checkpoints are kept at most; delete one first"
                ));
            }
            let checkpoint = Checkpoint {
                name: name.clone(),
                document: ctx.shell.editor().document().clone(),
                saved_at: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs()),
                set: ctx.shell.set_name(),
                generation: ctx.shell.set_generation(),
                by: ctx.session.client.label().to_owned(),
            };
            match replaced {
                Some(index) => items[index] = checkpoint,
                None => items.push(checkpoint),
            }
            done(json!({ "saved": name, "replaced": replaced.is_some() }))
        }
        Action::Delete => {
            let Some(name) = name else { return need_name() };
            let items = &mut ctx.state.checkpoints.items;
            let before = items.len();
            items.retain(|c| c.name != name);
            if items.len() == before {
                return failed(format!("no checkpoint is called \u{201c}{name}\u{201d}"));
            }
            done(json!({ "deleted": name }))
        }
        Action::Restore => {
            let Some(name) = name else { return need_name() };
            let Some(checkpoint) = ctx.state.checkpoints.get(&name).cloned() else {
                return failed(format!(
                    "no checkpoint is called \u{201c}{name}\u{201d}; `list` shows them"
                ));
            };
            if checkpoint.generation != ctx.shell.set_generation() {
                return failed(format!(
                    "checkpoint \u{201c}{name}\u{201d} was saved in set {}, not the open set; \
                     nothing changed",
                    checkpoint.set
                ));
            }
            let current = ctx.shell.editor().document();
            let changes = ChangeSet::between(current, &checkpoint.document);
            if changes.is_empty() {
                return done(json!({
                    "restored": name,
                    "changes": tally(&changes),
                    "note": "The song already matches it; nothing changed.",
                }));
            }
            let removes = changes.tracks.values().any(|c| *c == Change::Removed)
                || changes.clips.values().any(|c| *c == Change::Removed)
                || changes.devices.values().any(|c| *c == Change::Removed);
            let class = if removes {
                Permission::Destructive
            } else {
                Permission::Edit
            };
            let tally = tally(&changes);
            step!(ctx.permit(&[class], &["checkpoint.restore"], || format!(
                "Restore checkpoint \u{201c}{name}\u{201d} ({class}): it removes {} and changes \
                 {} of the tracks, clips, and devices.",
                tally["removed"], tally["modified"]
            )));
            let label = ctx.step_label(&format!("Restore checkpoint \u{201c}{name}\u{201d}"));
            let maker = ctx.maker();
            ctx.shell
                .edit(|editor| editor.replace_by(maker, label.clone(), checkpoint.document));
            done(json!({
                "restored": name,
                "undo_step": label,
                "changes": tally,
            }))
        }
    }
}

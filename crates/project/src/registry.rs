//! Crash recovery and Save Continuously in the command registry (WP-1.11,
//! decisions D16 and D17).
//!
//! [`register`] adds the commands' specs to a host's registry, and [`run`]
//! runs one by ID with JSON arguments against the app's [`Autosave`], as a
//! person's choice in the recovery prompt or an agent does:
//!
//! | ID | Does | Permission |
//! |---|---|---|
//! | `set.list_recoverable` | Lists the sets a crash left unsaved | read |
//! | `set.recover` | Opens one, as it was after its last edit | destructive |
//! | `set.discard_recovery` | Declines one | destructive |
//! | `settings.set_continuous_save` | Turns Save Continuously on or off | edit |
//!
//! ```
//! use bl_commands::Registry;
//! use bl_project::autosave::Autosave;
//! use bl_project::registry;
//! use serde_json::json;
//!
//! let mut commands = Registry::new();
//! registry::register(&mut commands)?;
//! assert!(commands.get("set.recover").is_some());
//!
//! # let dir = std::env::temp_dir().join(format!("bl-registry-doc-{}", std::process::id()));
//! let mut autosave = Autosave::start(&dir)?;
//! let ran = registry::run(&mut autosave, "set.list_recoverable", json!({}))?;
//! assert_eq!(ran.result, json!({ "sets": [] }));
//! # std::fs::remove_dir_all(&dir)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use core::fmt;

use bl_commands::Context::{Global, Settings};
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::autosave::{Autosave, AutosaveError, Recoverable, Recovered, set_name};
use crate::save::{SetFile, utc_rfc3339};

/// `set.list_recoverable`: the sets a crash left unsaved.
pub const LIST_RECOVERABLE: &str = "set.list_recoverable";
/// `set.recover`: open a set a crash left unsaved.
pub const RECOVER: &str = "set.recover";
/// `set.discard_recovery`: decline to recover a set.
pub const DISCARD_RECOVERY: &str = "set.discard_recovery";
/// `settings.set_continuous_save`: turn Save Continuously on or off.
pub const SET_CONTINUOUS_SAVE: &str = "settings.set_continuous_save";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// [`run`].
pub const IDS: &[&str] = &[
    LIST_RECOVERABLE,
    RECOVER,
    DISCARD_RECOVERY,
    SET_CONTINUOUS_SAVE,
];

/// Whether `id` is one of the commands [`register`] adds.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `set.recover` and `set.discard_recovery`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecoveryArgs {
    /// The recoverable set's `id`, as `set.list_recoverable` gives it. Leave
    /// it out for the newest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// Arguments of `settings.set_continuous_save`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContinuousSaveArgs {
    /// Whether Save Continuously is on.
    pub on: bool,
}

/// A set a crash left unsaved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecoverableSet {
    /// The ID that `set.recover` and `set.discard_recovery` take.
    pub id: String,
    /// The set's file, or null for a set that had never been saved.
    pub path: Option<String>,
    /// The set's name, as the title bar showed it.
    pub name: String,
    /// How many journalled edits follow the set's last save. Edits made
    /// faster than the disk could keep up count as one.
    pub edits: u64,
    /// When the last edit was journalled, in UTC (RFC 3339).
    pub last_edit: String,
    /// The last edit's name, as the undo history showed it.
    pub last_change: String,
    /// Whether the set file has been saved since, by another session or
    /// program, so recovering and saving would replace that save.
    pub saved_since: bool,
}

impl From<&Recoverable> for RecoverableSet {
    fn from(offer: &Recoverable) -> Self {
        Self {
            id: offer.id.clone(),
            path: offer.set.as_ref().map(|path| path.display().to_string()),
            name: offer.name(),
            edits: u64::try_from(offer.edits).unwrap_or(u64::MAX),
            last_edit: utc_rfc3339(offer.last_edit),
            last_change: offer.last_label.clone(),
            saved_since: offer.saved_since,
        }
    }
}

/// What `set.list_recoverable` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecoverableSets {
    /// The sets, newest first.
    pub sets: Vec<RecoverableSet>,
}

/// What `set.recover` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecoveredSet {
    /// The ID it was recovered from.
    pub id: String,
    /// The set's file, or null for a set that had never been saved.
    pub path: Option<String>,
    /// The set's name, as the title bar shows it.
    pub name: String,
    /// How many journalled edits it brought back.
    pub edits: u64,
}

/// What `set.discard_recovery` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiscardedRecovery {
    /// The ID that was declined.
    pub id: String,
    /// Where a copy of its journal is kept for seven days.
    pub kept_at: String,
}

/// What `settings.set_continuous_save` returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContinuousSave {
    /// Whether Save Continuously is now on.
    pub on: bool,
}

/// Adds the commands to a registry, with the save history's
/// ([`crate::history`]).
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    for spec in specs() {
        registry.register(spec)?;
    }
    // The save history's commands (WP-6.39) go where these go.
    crate::history::register(registry)
}

fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(LIST_RECOVERABLE, "List Recoverable Sets")
            .description(
                "Lists the sets that Beatlayer, here or in an earlier run, stopped without \
                 closing (a crash, a forced quit, a power cut) while they had unsaved edits. \
                 Each has an `id` for `set.recover` or `set.discard_recovery`, its file (null \
                 if it had never been saved), how many edits followed its last save, and when \
                 the last was made. Sets open in another running instance aren't listed. \
                 Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<RecoverableSets>(),
        CommandSpec::new(RECOVER, "Recover Set")
            .description(
                "Opens a set that `set.list_recoverable` lists, the one with `id` or the \
                 newest, exactly as it was after its last journalled edit. It replaces the \
                 open set, whose undo history is cleared; if the open set had unsaved edits \
                 they're kept for recovery in turn. The recovered set is unsaved: use \
                 `set.save` to keep it in its file, or `set.save_as`. Use it when the user \
                 wants back the work a crash interrupted.",
            )
            .permission(Permission::Destructive)
            .context(Global)
            .args::<RecoveryArgs>()
            .result::<RecoveredSet>(),
        CommandSpec::new(DISCARD_RECOVERY, "Discard Recovery")
            .description(
                "Declines to recover a set that `set.list_recoverable` lists, the one with `id` \
                 or the newest: its unsaved edits are removed from beside the set, so it isn't \
                 offered again, and a copy of its journal is kept in the recovery folder's \
                 Discarded folder for seven days. Its set file is untouched.",
            )
            .permission(Permission::Destructive)
            .context(Global)
            .args::<RecoveryArgs>()
            .result::<DiscardedRecovery>(),
        CommandSpec::new(SET_CONTINUOUS_SAVE, "Save Continuously")
            .description(
                "Turns Save Continuously on or off (it starts off). When on, every finished \
                 edit of a set that has a file is saved within a second, so reopening the set \
                 shows it without asking, and each edit is a save point in the set's history \
                 since its last normal save. Turning it on saves the open set's unsaved edits \
                 that way. When off, edits are still journalled for crash recovery, and only \
                 Save keeps them. Save still writes the set file either way.",
            )
            .permission(Permission::Edit)
            .context(Settings)
            .args::<ContinuousSaveArgs>()
            .result::<ContinuousSave>(),
    ]
}

/// What running a command gave.
#[derive(Debug)]
pub struct Ran {
    /// The result, as the command's result schema describes it.
    pub result: Value,
    /// For `set.recover`, the recovered set, which the host opens in place
    /// of the open one: an editor for `project`, saving to `set`, marked
    /// as having unsaved changes.
    pub recovered: Option<Recovered>,
}

/// Runs a command by its registry ID with JSON arguments.
///
/// # Errors
///
/// [`RunError::UnknownCommand`] if `id` isn't one of these commands,
/// [`RunError::InvalidArgs`] if the arguments don't fit its schema, and
/// [`RunError::Failed`] if the service refused it.
pub fn run(autosave: &mut Autosave, id: &str, args: Value) -> Result<Ran, RunError> {
    let command = IDS
        .iter()
        .copied()
        .find(|known| *known == id)
        .ok_or_else(|| RunError::UnknownCommand(id.to_owned()))?;
    let failed = |error| RunError::Failed { command, error };
    let value = |result: Value| {
        Ok(Ran {
            result,
            recovered: None,
        })
    };
    match command {
        LIST_RECOVERABLE => {
            parse::<NoArgs>(command, args)?;
            let sets = autosave.offers().iter().map(RecoverableSet::from).collect();
            value(json(&RecoverableSets { sets }))
        }
        RECOVER => {
            let args: RecoveryArgs = parse(command, args)?;
            let recovered = autosave.recover(args.id.as_deref()).map_err(failed)?;
            let path = recovered.set.as_ref().map(SetFile::path);
            let result = RecoveredSet {
                id: recovered.id.clone(),
                path: path.map(|path| path.display().to_string()),
                name: set_name(path),
                edits: u64::try_from(recovered.edits).unwrap_or(u64::MAX),
            };
            Ok(Ran {
                result: json(&result),
                recovered: Some(recovered),
            })
        }
        DISCARD_RECOVERY => {
            let args: RecoveryArgs = parse(command, args)?;
            let id = match args.id {
                Some(id) => id,
                None => autosave
                    .offers()
                    .first()
                    .map(|offer| offer.id.clone())
                    .ok_or(failed(AutosaveError::NoOffer { id: None }))?,
            };
            let kept = autosave.discard(Some(&id)).map_err(failed)?;
            value(json(&DiscardedRecovery {
                id,
                kept_at: kept.display().to_string(),
            }))
        }
        _ => {
            let args: ContinuousSaveArgs = parse(command, args)?;
            autosave.set_continuous(args.on);
            value(json(&ContinuousSave {
                on: autosave.continuous(),
            }))
        }
    }
}

/// A result as JSON. The result types are plain data, which always
/// serialises.
fn json(result: &impl Serialize) -> Value {
    serde_json::to_value(result).unwrap_or(Value::Null)
}

fn parse<T: for<'de> Deserialize<'de>>(command: &'static str, args: Value) -> Result<T, RunError> {
    serde_json::from_value(args).map_err(|error| RunError::InvalidArgs {
        command,
        message: error.to_string(),
    })
}

/// Why [`run`] refused a command.
#[derive(Debug)]
#[non_exhaustive]
pub enum RunError {
    /// No command here has this ID.
    UnknownCommand(String),
    /// The arguments don't fit the command's schema.
    InvalidArgs {
        /// The command.
        command: &'static str,
        /// What's wrong, from the deserialiser.
        message: String,
    },
    /// The autosave service refused the command.
    Failed {
        /// The command.
        command: &'static str,
        /// Why.
        error: AutosaveError,
    },
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCommand(id) => write!(f, "no recovery command is called `{id}`"),
            Self::InvalidArgs { command, message } => {
                write!(f, "invalid arguments for `{command}`: {message}")
            }
            Self::Failed { command, error } => write!(f, "`{command}` failed: {error}"),
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Failed { error, .. } => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use bl_commands::Registry;
    use serde_json::json;

    use super::*;

    #[test]
    fn every_command_is_registered_and_complete() {
        // Spec: crash-recovery R11 (the command)
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len() + crate::history::IDS.len());
        assert_eq!(registry.problems(), Vec::new());
        for id in IDS {
            assert!(handles(id));
            assert!(registry.contains(id), "{id}");
        }
        assert!(!handles("set.save"));
        // They sit beside the document's commands.
        bl_model::registry::register(&mut registry).unwrap();
    }

    #[test]
    fn arguments_are_checked() {
        let dir = std::env::temp_dir().join(format!("bl-registry-args-{}", std::process::id()));
        let mut autosave = Autosave::start(&dir).unwrap();
        assert!(matches!(
            run(&mut autosave, "set.nope", json!({})),
            Err(RunError::UnknownCommand(_))
        ));
        assert!(matches!(
            run(&mut autosave, LIST_RECOVERABLE, json!({ "extra": 1 })),
            Err(RunError::InvalidArgs { .. })
        ));
        assert!(matches!(
            run(&mut autosave, SET_CONTINUOUS_SAVE, json!({ "on": "yes" })),
            Err(RunError::InvalidArgs { .. })
        ));
        assert!(matches!(
            run(&mut autosave, RECOVER, json!({})),
            Err(RunError::Failed {
                error: AutosaveError::NoOffer { id: None },
                ..
            })
        ));
        assert!(matches!(
            run(&mut autosave, DISCARD_RECOVERY, json!({ "id": "1-2-3" })),
            Err(RunError::Failed {
                error: AutosaveError::NoOffer { .. },
                ..
            })
        ));
        let ran = run(&mut autosave, SET_CONTINUOUS_SAVE, json!({ "on": true })).unwrap();
        assert_eq!(ran.result, json!({ "on": true }));
        assert!(autosave.continuous());
        drop(autosave);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

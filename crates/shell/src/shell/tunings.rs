//! Tuning systems through the shell (WP-6.9; the tunings spec, R2, R15 and
//! R19): reading a `.scl` or `.ascl` file into the set, and the set's tuning
//! read back.
//!
//! The edits themselves are `bl-model`'s commands. What the shell adds is
//! the file, and the setting Retune Set On Loading Tuning Systems, which a
//! command left without `retune` follows.

use bl_interop::scala;
use bl_model::Command;
use bl_shell_spec::tunings::{self as ids, LoadArgs, TuningState};
use serde_json::{Value, json};

use super::*;

impl Shell {
    /// Runs one of the shell's tuning commands.
    pub(super) fn run_tunings(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            ids::LOAD => {
                let LoadArgs { path, retune } = Self::args(id, args)?;
                let tuning = scala::read(Path::new(&path)).map_err(|error| ShellError::Failed {
                    command: id.to_owned(),
                    message: format!("{path}: {error}"),
                })?;
                let retune = retune.unwrap_or(self.prefs.retune_on_tuning_load);
                self.editor
                    .apply(Command::SetTuning { tuning, retune })
                    .map_err(|error| ShellError::Failed {
                        command: id.to_owned(),
                        message: error.to_string(),
                    })?;
                self.changed();
                Ok(to_value(&self.tuning_state()))
            }
            ids::STATE => {
                Self::args::<NoArgs>(id, args)?;
                Ok(to_value(&self.tuning_state()))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }

    /// The set's tuning as `tuning.state` reports it (R19).
    #[must_use]
    pub fn tuning_state(&self) -> TuningState {
        TuningState {
            tuning: self
                .editor
                .document()
                .settings()
                .tuning
                .as_ref()
                .map(bl_model::Tuning::readout),
        }
    }

    /// Whether a person loading or removing a tuning is asked first: Retune
    /// Set On Loading Tuning Systems is on (R15).
    #[must_use]
    pub fn tuning_change_asks(&self) -> bool {
        self.prefs.retune_on_tuning_load
    }

    /// `args` for `tuning.set` or `tuning.remove` with `retune` filled in
    /// from the setting where the caller left it out (R15).
    pub(super) fn with_retune(&self, id: &str, args: Value) -> Value {
        if !matches!(id, "tuning.set" | ids::REMOVE) {
            return args;
        }
        let mut fields = match args {
            Value::Object(fields) => fields,
            Value::Null => serde_json::Map::new(),
            other => return other,
        };
        fields
            .entry("retune")
            .or_insert_with(|| json!(self.prefs.retune_on_tuning_load));
        Value::Object(fields)
    }
}

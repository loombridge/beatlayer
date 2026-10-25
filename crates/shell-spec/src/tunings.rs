//! Tuning systems (WP-6.9; the tunings spec, R2, R15 and R19): the commands
//! that need the shell, `tuning.load`, which reads a tuning file, and
//! `tuning.state`, which reads the set's tuning back.
//!
//! The commands that change the tuning inside the set, `tuning.set`,
//! `tuning.remove`, `tuning.set_reference` and `tuning.set_range`, and
//! `track.set_tuning_bypass`, are `bl-model`'s; Retune Set On Loading Tuning
//! Systems is `settings.set_retune_on_tuning_load`.

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_model::TuningReadout;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `tuning.load`: read a `.scl` or `.ascl` file into the set (R2).
pub const LOAD: &str = "tuning.load";
/// `tuning.state`: the set's tuning (R19).
pub const STATE: &str = "tuning.state";

/// `tuning.remove` (`bl-model`): the set's tuning goes (R9).
pub const REMOVE: &str = "tuning.remove";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[LOAD, STATE];

/// Whether `id` is one of the commands [`register`] adds.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `tuning.load`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoadArgs {
    /// The `.scl` or `.ascl` file to read, absolute or relative to the
    /// app's working directory, or the path of one of Beatlayer's own
    /// tunings, `tunings:/` and its name. The tuning is named after the
    /// file, without its extension.
    pub path: String,
    /// Whether every note of the MIDI clips on tracks that play the tuning
    /// moves to the key whose new pitch is nearest the pitch it had. Left
    /// out, it follows Retune Set On Loading Tuning Systems.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retune: Option<bool>,
}

/// What `tuning.load` and `tuning.state` return.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TuningState {
    /// The set's tuning, or null when it plays twelve equal steps.
    pub tuning: Option<TuningReadout>,
}

/// Adds the tuning commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(LOAD, "Load Tuning")
            .description(
                "Reads a Scala `.scl` file, or an `.ascl` file read as one, and makes it the \
                 set's tuning in place of any other, as one undo step named \"Load Tuning\". \
                 The set keeps the tuning inside it. A file that holds no pitches, a count that \
                 doesn't match its lines, a ratio of 0 or less, or a pseudo-octave at or below \
                 the unison is refused with an error naming the problem, and the set's tuning \
                 stays as it was. The tuning is named after the file, without its extension, \
                 and takes the default reference pitch (octave 3, degree 0, 261.626 Hz) and \
                 the 128 notes that put degree 0 of that octave on key 60. With `retune` true, \
                 every note of the MIDI clips on tracks that play the tuning moves to the key \
                 whose new pitch is nearest the pitch it had; left out, `retune` follows \
                 Retune Set On Loading Tuning Systems. The result is the tuning, as \
                 `tuning.state` reads it. Reads a file outside the project.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<LoadArgs>()
            .result::<TuningState>(),
    )?;
    registry.register(
        CommandSpec::new(STATE, "Tuning State")
            .description(
                "Reports the set's tuning: its name, description, how many notes it holds to the \
                 pseudo-octave, the pitch of each in cents, the pseudo-octave's size, the note \
                 names and link where it has them, the reference pitch (octave, degree and \
                 frequency in Hz), the note range (the lowest note, which key 0 plays, and the \
                 highest) and the frequency of each key 0 to 127 (null for a key above the \
                 range, which plays nothing). `tuning` is null when the set plays twelve equal \
                 steps. Changes nothing. The commands that change the tuning read back through \
                 it.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<TuningState>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_commands_register_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        assert!(registry.problems().is_empty(), "{:?}", registry.problems());
        assert!(handles(LOAD) && handles(STATE) && !handles(REMOVE));
    }
}

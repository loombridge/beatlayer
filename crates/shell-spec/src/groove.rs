//! Grooves through the shell (WP-6.19, the grooves spec R5 and R16 to R19,
//! R23): the commands that read or write a groove file, make a groove of a
//! clip, write one into a clip, make a clip of one, and read the pool.
//!
//! The Groove Pool's own edits (`groove.add`, `groove.rename`, `groove.set`,
//! `groove.set_global_amount`, `groove.set_auto_load`, `groove.delete`) and
//! `clip.set_groove` are `bl-model`'s. These need the shell: the User
//! Library's folder, a file's bytes, a clip's transients and audio, or the
//! selected clips.

use bl_commands::Context::{ClipView, Global};
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use bl_model::{ClipId, Groove, GrooveId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `groove.load`: read a groove file into the pool.
pub const LOAD: &str = "groove.load";
/// `groove.save`: write a groove to a groove file.
pub const SAVE: &str = "groove.save";
/// `groove.extract`: make a groove of a clip's timing and loudness.
pub const EXTRACT: &str = "groove.extract";
/// `groove.commit`: write a clip's groove into the clip.
pub const COMMIT: &str = "groove.commit";
/// `groove.to_clip`: make a MIDI clip of a groove.
pub const TO_CLIP: &str = "groove.to_clip";
/// `groove.pool`: read the set's grooves.
pub const POOL: &str = "groove.pool";

/// `clip.set_groove` (`bl-model`): a clip's groove.
pub const SET_CLIP_GROOVE: &str = "clip.set_groove";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[LOAD, SAVE, EXTRACT, COMMIT, TO_CLIP, POOL];

/// Whether `id` is one of the commands [`register`] adds.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// The undo step `groove.load` makes.
pub const LOAD_STEP: &str = "Load Groove";
/// The undo step `groove.extract` makes.
pub const EXTRACT_STEP: &str = "Extract Groove";
/// The undo step `groove.commit` makes.
pub const COMMIT_STEP: &str = "Commit Groove";
/// The undo step `groove.to_clip` makes.
pub const TO_CLIP_STEP: &str = "Groove to Clip";

/// Arguments of `groove.load`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoadGrooveArgs {
    /// The groove file, a `.blgroove`, absolute or relative to the app's
    /// working directory.
    pub path: String,
    /// Clips to give the groove to as well, in the same undo step.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<ClipId>,
}

/// What `groove.load` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GrooveLoaded {
    /// The groove it added to the end of the pool.
    pub groove: GrooveId,
    /// Its name.
    pub name: String,
}

/// Arguments of `groove.save`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveGrooveArgs {
    /// The groove, one of the pool's.
    pub groove: GrooveId,
    /// The file to write. Left out, `<name>.blgroove` in the User Library's
    /// Grooves folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Replace a file already there. Without it, an existing file is left
    /// alone and the save refused.
    #[serde(default)]
    pub replace: bool,
}

/// What `groove.save` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GrooveSaved {
    /// The file written.
    pub path: String,
}

/// Arguments of `groove.extract`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtractGrooveArgs {
    /// The clips to make grooves of. Left out, the selected ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipId>>,
}

/// What `groove.extract` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GrooveExtracted {
    /// The grooves it added to the end of the pool, one for each clip, in
    /// the clips' order.
    pub grooves: Vec<GrooveId>,
}

/// Arguments of `groove.commit`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommitGrooveArgs {
    /// The clips to commit. Left out, the selected ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipId>>,
    /// A groove: with it and no `clips`, every clip that uses the groove is
    /// committed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groove: Option<GrooveId>,
    /// The seed Random's one draw comes from. Left out, a new one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
}

/// What `groove.commit` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GrooveCommitted {
    /// The clips whose groove was written in.
    pub clips: Vec<ClipId>,
}

/// Arguments of `groove.to_clip`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GrooveToClipArgs {
    /// The groove, one of the pool's.
    pub groove: GrooveId,
    /// The MIDI track to put the clip on.
    pub track: TrackId,
    /// Where the clip starts, in ticks.
    pub at: Ticks,
}

/// What `groove.to_clip` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GrooveClip {
    /// The clip it made.
    pub clip: ClipId,
}

/// One groove of the pool, as `groove.pool` reports it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GroovePoolEntry {
    /// The groove: its ID, name, length, notes and five parameters.
    #[serde(flatten)]
    pub groove: Groove,
    /// Whether any clip uses it: an inactive groove's parameters are greyed.
    pub active: bool,
    /// The clips that use it.
    pub clips: Vec<ClipId>,
}

/// What `groove.pool` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GroovePoolState {
    /// The grooves, in the order they were added.
    pub grooves: Vec<GroovePoolEntry>,
    /// The Global Amount, from 0 to 130 percent.
    pub global_amount: f64,
    /// The Auto Load Groove switch.
    pub auto_load: bool,
    /// The groove new MIDI clips take while the switch is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_load_groove: Option<GrooveId>,
}

/// Adds the groove commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(LOAD, "Load Groove")
            .description(
                "Reads a groove file (`.blgroove`, Beatlayer's own JSON format) and adds the \
                 groove it holds to the end of the Groove Pool, with the parameters it holds, \
                 as double-clicking a groove in the browser does. With `clips`, those clips are \
                 given the groove too. One undo step, `Load Groove`. A file that is damaged, \
                 was saved by a newer Beatlayer or breaks a groove's limits is refused and \
                 changes nothing; loading the same file twice adds two grooves. Reads a file \
                 outside the project. Returns the groove's ID and name.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<LoadGrooveArgs>()
            .result::<GrooveLoaded>(),
    )?;
    registry.register(
        CommandSpec::new(SAVE, "Save Groove")
            .description(
                "Writes a groove of the pool, with its current parameters, to a groove file: \
                 at `path`, or as `<name>.blgroove` in the User Library's Grooves folder, \
                 where the browser's Grooves label finds it. The file is written whole or not \
                 at all, and a file already there is left alone and the save refused unless \
                 `replace` is true. Changes nothing in the set and is no undo step; writes a \
                 file outside the project. Returns the path written.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<SaveGrooveArgs>()
            .result::<GrooveSaved>(),
    )?;
    registry.register(
        CommandSpec::new(EXTRACT, "Extract Groove")
            .description(
                "Makes a groove of each clip it names (`clips`, or the selected clips) at the \
                 end of the Groove Pool, from the part of the clip that plays: its loop brace \
                 while its loop is on, otherwise its Start to its End marker, the groove as \
                 long as that part. From a MIDI clip, a groove note at each note's start with \
                 its velocity, the loudest where notes start together. From an audio clip, a \
                 groove note at each of its transients, in its beats through its warp map (an \
                 unwarped clip's at the set's tempo where it starts), with a velocity from the \
                 audio's peak level in the 20 ms after it: the loudest peak 127, and 48 dB \
                 below it 1. The groove is named after the clip (`Groove` if it has no name) \
                 and has Base 1/16, Quantize 0, Timing 100, Random 0 and Velocity 0. An audio \
                 clip needs its file's analysis to be ready. One undo step, `Extract Groove`. \
                 Returns the grooves' IDs.",
            )
            .permission(Permission::Edit)
            .contexts([Global, ClipView])
            .args::<ExtractGrooveArgs>()
            .result::<GrooveExtracted>(),
    )?;
    registry.register(
        CommandSpec::new(COMMIT, "Commit Groove")
            .description(
                "Writes a clip's groove into the clip and sets its groove to none. A MIDI \
                 clip's notes move to where they play with the groove and take the velocities \
                 they play at, Random drawn once from `seed` (a new seed if left out). A \
                 warped audio clip gets a warp marker at each grid line and moved transient \
                 where the groove puts it, over the part of its contents that plays, and, \
                 while the groove's Velocity is above 0, a Volume clip envelope stepping \
                 through the gains it gives, replacing any Volume envelope the clip had (a \
                 Volume envelope only lowers a level, so the steps are shares of the loudest \
                 and the clip's gain rises by that gain). `clips` names the clips; left out, \
                 the selected ones; with `groove` and no clips, every clip that uses the \
                 groove. A clip with no groove is left as it is. One undo step, `Commit \
                 Groove`, however many clips it commits. An audio clip needs its file's \
                 analysis to be ready. Returns the clips committed.",
            )
            .permission(Permission::Edit)
            .contexts([Global, ClipView])
            .args::<CommitGrooveArgs>()
            .result::<GrooveCommitted>(),
    )?;
    registry.register(
        CommandSpec::new(TO_CLIP, "Groove to Clip")
            .description(
                "Makes a MIDI clip of a groove of the pool on a MIDI track at a position, as \
                 dragging the groove onto the track does: a note on C3 (key 60) at each \
                 groove note with its velocity, each a sixteenth long or as long as the gap to \
                 the next note, whichever is shorter; the clip one groove long, looping, and \
                 named after the groove. A clip made so takes no auto-load groove. A clip \
                 already there is replaced where the new one covers it. One undo step, `Groove \
                 to Clip`. Returns the clip's ID.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<GrooveToClipArgs>()
            .result::<GrooveClip>(),
    )?;
    registry.register(
        CommandSpec::new(POOL, "Groove Pool")
            .description(
                "Reports the set's grooves: each with its ID, name, length, notes and its \
                 Base, Quantize, Timing, Random and Velocity, whether it is active (some clip \
                 uses it) and which clips use it; the Global Amount; and Auto Load Groove \
                 with its groove. Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<GroovePoolState>(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_id_is_registered_and_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        let problems: Vec<String> = registry
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(problems.is_empty(), "{problems:#?}");
        let class = |id: &str| registry.get(id).unwrap().permission;
        assert_eq!(class(LOAD), Some(Permission::External));
        assert_eq!(class(SAVE), Some(Permission::External));
        assert_eq!(class(EXTRACT), Some(Permission::Edit));
        assert_eq!(class(COMMIT), Some(Permission::Edit));
        assert_eq!(class(TO_CLIP), Some(Permission::Edit));
        assert_eq!(class(POOL), Some(Permission::Read));
    }
}

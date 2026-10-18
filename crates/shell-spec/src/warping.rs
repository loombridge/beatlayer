//! Transients as an agent reads them (WP-4.3): the `clip.transients` query,
//! its arguments and its result. The warping spec, R23 and R24, holds the
//! behaviour; the edits themselves are `bl-model`'s `clip.insert_transient`
//! and its siblings.

use bl_commands::Context::ClipView;
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use bl_model::ClipId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `clip.transients`: a clip's transients, with its edits applied.
pub const TRANSIENTS: &str = "clip.transients";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[TRANSIENTS];

/// Whether `id` is one of the warping query commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Which clip `clip.transients` reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "The audio clip whose transients to report.")]
pub struct ClipTransientsArgs {
    /// The audio clip.
    pub clip: ClipId,
}

/// One transient of a clip.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ClipTransient {
    /// Where it is in the file, in seconds.
    pub time: f64,
    /// The beat of the clip's contents it plays on, in ticks from the clip's
    /// 1.1.1; `null` for a clip that isn't warped, which has no beats.
    pub beat: Option<Ticks>,
    /// Whether it was put in by hand rather than detected.
    pub by_hand: bool,
}

/// What `clip.transients` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ClipTransients {
    /// The clip.
    pub clip: ClipId,
    /// Whether the file's analysis is ready. Until it is, only the
    /// transients put in by hand are listed.
    pub analysis_ready: bool,
    /// Why the analysis failed, if it did: the file is missing, changed, or
    /// can't be decoded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analysis_error: Option<String>,
    /// The clip's transients in order of their time in the file: those
    /// detected, less the ones taken away, with the ones put in by hand.
    pub transients: Vec<ClipTransient>,
}

/// Adds `clip.transients` to a registry.
///
/// # Errors
///
/// [`RegisterError`] if its ID is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(TRANSIENTS, "Clip Transients")
            .description(
                "Lists an audio clip's transients: where its file starts notes and hits, found \
                 once in the background and kept by the file's content hash, with the clip's own \
                 edits applied (`clip.insert_transient`, `clip.delete_transients`, \
                 `clip.move_transient`, `clip.reset_transients`). Each has its `time` in the \
                 file in seconds, its `beat` in ticks from the clip's 1.1.1 when the clip is \
                 warped, and whether it was put in `by_hand`. `analysis_ready` says whether the \
                 file's analysis is done; until it is, only the transients put in by hand are \
                 listed, and asking starts the analysis. Changes nothing.",
            )
            .permission(Permission::Read)
            .context(ClipView)
            .args::<ClipTransientsArgs>()
            .result::<ClipTransients>(),
    )
}

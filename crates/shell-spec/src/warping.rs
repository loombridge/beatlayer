//! Transients as an agent reads them (WP-4.3): the `clip.transients` query,
//! its arguments and its result. The warping spec, R23 and R24, holds the
//! behaviour; the edits themselves are `bl-model`'s `clip.insert_transient`
//! and its siblings. Also a file's saved default clip settings (WP-4.9, R42):
//! `clip.save_default` and `clip.defaults`.

use bl_commands::Context::{ClipView, SampleEditor};
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use bl_model::{ClipDefaults, ClipId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `clip.transients`: a clip's transients, with its edits applied.
pub const TRANSIENTS: &str = "clip.transients";

/// `clip.save_default`: save an audio clip's settings as its file's defaults.
pub const SAVE_DEFAULT: &str = "clip.save_default";

/// `clip.defaults`: read back the defaults saved for an audio clip's file.
pub const DEFAULTS: &str = "clip.defaults";

/// `clip.quantize_audio`: quantise a warped audio clip's transients to the grid.
pub const QUANTIZE_AUDIO: &str = "clip.quantize_audio";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[TRANSIENTS, SAVE_DEFAULT, DEFAULTS, QUANTIZE_AUDIO];

/// Whether `id` is one of the warping commands.
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

/// Which clip's file `clip.save_default` and `clip.defaults` act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "The audio clip whose file's saved defaults to write or read.")]
pub struct ClipDefaultsArgs {
    /// The audio clip.
    pub clip: ClipId,
}

/// What `clip.save_default` and `clip.defaults` return.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SavedDefaults {
    /// The clip.
    pub clip: ClipId,
    /// The defaults file beside the clip's sample, where the defaults are or
    /// would be kept.
    pub file: String,
    /// The file's saved defaults; `null` when none are saved, or the file
    /// beside the sample is not whole, not of the current format, or was
    /// saved for other content.
    pub defaults: Option<ClipDefaults>,
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

/// Arguments of `clip.quantize_audio`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QuantizeAudioArgs {
    /// The audio clip. Left out, the one the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The start of the time selection, in ticks from the clip's 1.1.1. Left out, the clip's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Ticks>,
    /// The end of the time selection, in ticks from the clip's 1.1.1. Left out, the clip's end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Ticks>,
    /// The grid line spacing in ticks, over the Quantize settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<Ticks>,
    /// How far each warp marker moves toward its grid line, from 0 to 100 percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<u8>,
    /// The transient times in the file in seconds to use over the clip's analysis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transients: Option<Vec<f64>>,
}

/// Adds `clip.transients` and `clip.quantize_audio` to a registry.
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
    )?;
    registry.register(
        CommandSpec::new(SAVE_DEFAULT, "Save Default Clip")
            .description(
                "Saves an audio clip's Warp switch, warp markers and tempo, warp mode and mode \
                 controls (which hold its Transpose and Detune), and gain as its file's \
                 defaults, as the Save button in the clip's title bar does: into a small file \
                 of Beatlayer's own, `<sample file name>.beatlayer-clip`, beside the sample, so \
                 the defaults travel with it, replacing any saved before. A clip made from the \
                 file afterwards (dropped from the browser, loaded with Enter or imported) \
                 starts with them and is not auto-warped; clips already in the set don't \
                 change. Fails with the reason, and writes nothing, if the folder can't be \
                 written or the clip's file isn't known. Changes nothing in the set and is no \
                 undo step; writes a file outside the project.",
            )
            .permission(Permission::External)
            .context(ClipView)
            .args::<ClipDefaultsArgs>()
            .result::<SavedDefaults>(),
    )?;
    registry.register(
        CommandSpec::new(DEFAULTS, "Clip Defaults")
            .description(
                "Reads the defaults saved for an audio clip's file by `clip.save_default`: its \
                 `file`, the `<sample file name>.beatlayer-clip` beside the sample, and the \
                 `defaults` in it, or `null` when none are saved or the file does not hold \
                 settings for this content. Changes nothing.",
            )
            .permission(Permission::Read)
            .context(ClipView)
            .args::<ClipDefaultsArgs>()
            .result::<SavedDefaults>(),
    )?;
    registry.register(
        CommandSpec::new(QUANTIZE_AUDIO, "Quantize")
            .description(
                "Quantises a warped audio clip: for each line of the quantise grid inside the \
                 time selection (`from` to `to` in ticks from 1.1.1, or the whole clip with none), \
                 the clip's transient nearest that line, if one lies within half a grid step of it, \
                 gets a warp marker at its time in the file (made if there is none) and moves `amount` \
                 percent of the way to the line, keeping its time in the file. Transients nearest no \
                 line get no marker. `grid` is the spacing between grid lines in ticks; left out, it \
                 uses the Quantize tool's settings (`clip_view.quantize_settings`), whose grid follows \
                 the editing grid until a note division is chosen and is a triplet of it with Triplets \
                 on. `amount` is from 0 to 100 percent (left out, the settings' amount). The audio \
                 clip's Transform panel and Cmd+Enter run it with those settings. 100 percent lands each on its line; 0 changes nothing and is \
                 no undo step; otherwise it is one undo step. Refuses an unwarped clip.",
            )
            .permission(Permission::Edit)
            .contexts([ClipView, SampleEditor])
            .binding("S-SAMPLE-001")
            .args::<QuantizeAudioArgs>()
            .result::<()>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_id_is_registered_and_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        for id in IDS {
            assert!(registry.contains(id), "{id}");
        }
        let problems: Vec<String> = registry
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(problems.is_empty(), "{problems:#?}");
        let class = |id: &str| registry.get(id).unwrap().permission;
        assert_eq!(class(TRANSIENTS), Some(Permission::Read));
        assert_eq!(class(DEFAULTS), Some(Permission::Read));
        assert_eq!(class(SAVE_DEFAULT), Some(Permission::External));
        assert_eq!(class(QUANTIZE_AUDIO), Some(Permission::Edit));
        let bindings: Vec<&str> = registry
            .get(QUANTIZE_AUDIO)
            .unwrap()
            .bindings
            .iter()
            .map(|binding| binding.shortcut.as_ref())
            .collect();
        assert_eq!(bindings, ["S-SAMPLE-001"]);
    }
}

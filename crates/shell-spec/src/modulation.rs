//! Modulation as an agent reads it (WP-5.9): the `modulation.state` query,
//! its arguments and its result. The modulation spec holds the behaviour.

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_model::{AutomationTarget, ClipId, DeviceId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `modulation.state`: the parameters a track's clips and the modulators act
/// on, with what each played in the last block and what modulates it.
pub const STATE: &str = "modulation.state";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[STATE];

/// Whether `id` is one of the modulation commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Which parameters `modulation.state` reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "The track whose parameters to report; every track's when left out.")]
pub struct StateArgs {
    /// The track, or every track when left out.
    #[serde(default)]
    pub track: Option<TrackId>,
}

/// One source modulating a parameter: a clip's envelope.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModulationSource {
    /// The clip whose modulation envelope it is.
    pub clip: ClipId,
    /// The envelope's value at the play position: a share, from 0 to 1 for a
    /// volume or a send's level and from −1 to 1 for the rest; `null` while
    /// the clip doesn't play there.
    pub share: Option<f64>,
    /// What it contributes at the play position, as a share of the
    /// parameter's travel; 0 while the clip doesn't play there.
    pub contribution: f64,
}

/// One modulator slot acting on a parameter (WP-5.10).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModulatorSource {
    /// The modulator device.
    pub device: DeviceId,
    /// Its slot, from 0 to 7.
    pub slot: u8,
    /// Whether the modulator is in Remote Control, holding the parameter
    /// rather than modulating it.
    pub remote_control: bool,
    /// What it contributes now, as a share of the parameter's travel, when
    /// it is the only source and the engine's published values tell: `null`
    /// when several sources act together (the engine publishes their sum) or
    /// when it holds the parameter.
    pub contribution: Option<f64>,
}

/// A parameter a track's clips or modulators act on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModulatedParameter {
    /// The track whose parameter it is.
    pub track: TrackId,
    /// The parameter: a mixer control, a send, or a device's parameter.
    pub target: AutomationTarget,
    /// Its base, in the parameter's own units: its automation envelope's
    /// value or its control's, as the last block played it while the song
    /// plays under modulation, and its control's otherwise.
    pub base: f64,
    /// The value it played in the last block; the base while nothing
    /// modulates it now.
    pub played: f64,
    /// The clip envelopes modulating it.
    pub sources: Vec<ModulationSource>,
    /// The modulator slots mapping it.
    #[serde(default)]
    pub modulators: Vec<ModulatorSource>,
    /// The modulator in Remote Control that holds it, if one does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_by: Option<DeviceId>,
}

/// What `modulation.state` reports.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModulationState {
    /// Every parameter a clip's envelope modulates, track by track.
    pub parameters: Vec<ModulatedParameter>,
}

/// Registers the modulation commands.
///
/// # Errors
///
/// [`RegisterError`] if an ID is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(STATE, "Modulation State")
            .description(
                "Reports the parameters a track's clips and the modulators act on (all tracks' \
                 when `track` is left out): for each, the track and the parameter (a mixer \
                 control, a send, or a device's parameter), its base (what its automation, a \
                 modulator in Remote Control or its control sets), the value it played in the \
                 last block, the clip envelopes modulating it, each with its value at the play \
                 position and what it contributes, as a share of the parameter's travel, the \
                 modulator slots mapping it (`modulators`), and the modulator that holds it in \
                 Remote Control (`held_by`). A clip envelope modulates by a share: 0 to 1 for a \
                 volume or a send, which brings its level down to that share, and −1 to 1 for \
                 the rest. It reads back `clip.set_envelope_breakpoint` and the other clip \
                 envelope commands, and `modulator.map`. Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<StateArgs>()
            .result::<ModulationState>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_registers_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        assert!(registry.problems().is_empty(), "{:?}", registry.problems());
        assert!(handles(STATE));
    }
}

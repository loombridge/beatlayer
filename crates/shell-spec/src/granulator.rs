//! Granulator III's capture command (WP-8.27; the Granulator III spec, R19):
//! starting and stopping a capture of the device's sidechain source.

use bl_commands::Context::DeviceView;
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_model::DeviceId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `granulator.capture`: start or stop a Granulator III's capture (R19).
pub const CAPTURE: &str = "granulator.capture";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[CAPTURE];

/// Whether `id` is one of these commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// What `granulator.capture` does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GranulatorCaptureAction {
    /// Starts a capture.
    Start,
    /// Ends the capture running now.
    Stop,
}

/// Arguments of `granulator.capture`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GranulatorCaptureArgs {
    /// The Granulator III. The one selected in the Device View when it is
    /// left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// `start` or `stop`.
    pub action: GranulatorCaptureAction,
}

/// What `granulator.capture` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GranulatorCaptureResult {
    /// The device.
    pub device: DeviceId,
    /// Whether a capture runs now.
    pub capturing: bool,
}

/// Registers Granulator III's capture command.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(CAPTURE, "Capture")
            .description(
                "Starts or stops a Granulator III's capture (`action`: `start` or `stop`). A \
                 capture writes the audio of the device's sidechain Source into the device, \
                 sample for sample from the block it starts in and aligned for latency as the \
                 sidechain is, and stops by itself after Capture Length (1 to 30 s). While it \
                 fills, the device plays grains from what has been captured so far. When it \
                 ends, the captured audio becomes the device's sample as one undo step, \
                 Capture Audio, and a worker writes it to a WAV file in the project's \
                 Samples/Recorded folder, which the set references as a loaded sample. It is \
                 refused with the Source at No Input, the sidechain's External off, a set \
                 never saved (it has no project folder), no audio engine running or a \
                 capture already running. Read the capture back in the query tool's `device` \
                 result, under `capture`.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<GranulatorCaptureArgs>()
            .result::<GranulatorCaptureResult>(),
    )?;
    Ok(())
}

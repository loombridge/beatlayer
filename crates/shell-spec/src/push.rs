//! Push 2 for agents (`docs/specs/push-2.md`, R34): `push.state`,
//! `push.set_mode`, `push.set_setup` and `push.display_frame`, with their
//! arguments and results.
//!
//! [`register`] adds their specs, which are pure data; `bl-shell` runs them
//! by ID against the Push 2 surfaces `bl-push` makes.

use std::collections::BTreeMap;

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `push.state`: what each connected Push 2 shows and is set to.
pub const STATE: &str = "push.state";
/// `push.set_mode`: the pad mode and the display mode.
pub const SET_MODE: &str = "push.set_mode";
/// `push.set_setup`: the Setup menu's values.
pub const SET_SETUP: &str = "push.set_setup";
/// `push.display_frame`: the latest display frame as a PNG.
pub const DISPLAY_FRAME: &str = "push.display_frame";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[STATE, SET_MODE, SET_SETUP, DISPLAY_FRAME];

/// Whether `id` is one of the commands [`register`] adds.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// The pad mode (R21).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PushPadMode {
    /// Note Mode.
    Note,
    /// Session Mode.
    Session,
}

/// The display mode (R22).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PushDisplayMode {
    /// Device.
    Device,
    /// Browse.
    Browse,
    /// Mix.
    Mix,
    /// Clip.
    Clip,
}

/// Mix's page (R22).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PushMixPage {
    /// Track Mix.
    Track,
    /// Global Mix.
    Global,
}

/// A menu over the display mode (R23).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PushMenu {
    /// Setup.
    Setup,
    /// Scale.
    Scale,
}

/// Workflow (R31).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PushWorkflow {
    /// Scene, the default.
    Scene,
    /// Clip.
    Clip,
}

/// The MIDI mode (R4, R33).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PushMidiMode {
    /// Live mode: the surface reads the controls.
    Live,
    /// User mode: the controls go to the User port.
    User,
}

/// What an LED does besides showing its colour (R13).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushLedAnimation {
    /// The colour index it moves to.
    pub colour: u8,
    /// The animation's channel, 1 to 15: one-shot on 1 to 5, pulsing on 6 to
    /// 10 and blinking on 11 to 15.
    pub channel: u8,
    /// `one_shot`, `pulsing` or `blinking`.
    pub kind: String,
}

/// One LED (R13): the palette index it shows, which is a level of white for
/// a white LED, and its animation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushLed {
    /// The colour index, 0 for off.
    pub colour: u8,
    /// Its animation, if it has one.
    pub animation: Option<PushLedAnimation>,
}

/// How the touch strip is configured and lit (R11).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushTouchStrip {
    /// `pitch_bend`, `modulation` or `range`.
    pub configuration: String,
    /// The configuration flags Push 2 was given.
    pub flags: u8,
    /// The 31 LEDs' levels, 0 to 7, the bottom one first; all 0 unless
    /// Beatlayer lights it (`range`).
    pub leds: Vec<u8>,
}

/// The Setup menu's values (R31).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushSetup {
    /// Pad Sensitivity, 0 to 10.
    pub pad_sensitivity: u8,
    /// Pad Gain, 0 to 10.
    pub pad_gain: u8,
    /// Pad Dynamics, 0 to 10.
    pub pad_dynamics: u8,
    /// Display Light, 0 to 100 %.
    pub display_light: u8,
    /// LED Brightness, 0 to 100 %.
    pub led_brightness: u8,
    /// Workflow.
    pub workflow: PushWorkflow,
}

/// What the display shows (R17 to R20).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushDisplay {
    /// The content as text, by band and column: the labels, the parameters'
    /// names and values with their dots, the notification, the count-in and
    /// the progress bars.
    pub text: Vec<String>,
    /// Whether the display interface is claimed.
    pub claimed: bool,
    /// Why it isn't.
    pub error: Option<String>,
    /// Frames sent so far.
    pub frames: u64,
}

/// One connected Push 2.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushInfo {
    /// The Control Surfaces table's row it runs in, from 1.
    pub row: u8,
    /// Its Live port.
    pub input: String,
    /// The output it is sent to.
    pub output: String,
    /// The firmware version its Device Inquiry reported.
    pub firmware: Option<String>,
    /// Whether it has its own power supply, once Request Statistics was
    /// answered.
    pub external_power: Option<bool>,
    /// Why it isn't driven, if it isn't: not a Push 2.
    pub error: Option<String>,
    /// The MIDI mode.
    pub midi_mode: PushMidiMode,
    /// The pad mode.
    pub pad_mode: PushPadMode,
    /// The display mode.
    pub display_mode: PushDisplayMode,
    /// Mix's page, while Mix is the display mode.
    pub mix: Option<PushMixPage>,
    /// The menu that is open.
    pub menu: Option<PushMenu>,
    /// The buttons held, by name.
    pub held: Vec<String>,
    /// The buttons locked on (Mute, Solo, Stop Clip), by name.
    pub locked: Vec<String>,
    /// Whether the colour picker is open.
    pub colour_picker: bool,
    /// Every LED, by control name: a button by its name (`play`, `lower_1`),
    /// a pad as `pad_<row>_<column>`, row from the bottom and column from
    /// the left, both from 0.
    pub leds: BTreeMap<String, PushLed>,
    /// The touch strip.
    pub touch_strip: PushTouchStrip,
    /// The Setup menu's values.
    pub setup: PushSetup,
    /// The display.
    pub display: PushDisplay,
}

/// What `push.state` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushState {
    /// Each Push 2 that runs, by row.
    pub pushes: Vec<PushInfo>,
}

/// Arguments of `push.set_mode`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PushSetModeArgs {
    /// The row, from 1 to 6. Left out, every Push 2.
    #[serde(default)]
    pub row: Option<u8>,
    /// The pad mode to switch to.
    #[serde(default)]
    pub pad: Option<PushPadMode>,
    /// The display mode to switch to.
    #[serde(default)]
    pub display: Option<PushDisplayMode>,
    /// Mix's page, with `display` `mix`; Track Mix if left out.
    #[serde(default)]
    pub mix: Option<PushMixPage>,
}

/// What `push.set_mode` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushSetModeResult {
    /// The rows whose Push 2 was asked to switch, from 1.
    pub rows: Vec<u8>,
}

/// Arguments of `push.set_setup`: any of the Setup menu's values; what is left
/// out stays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PushSetSetupArgs {
    /// Pad Sensitivity, 0 to 10.
    #[serde(default)]
    #[schemars(range(min = 0, max = 10))]
    pub pad_sensitivity: Option<u8>,
    /// Pad Gain, 0 to 10.
    #[serde(default)]
    #[schemars(range(min = 0, max = 10))]
    pub pad_gain: Option<u8>,
    /// Pad Dynamics, 0 to 10.
    #[serde(default)]
    #[schemars(range(min = 0, max = 10))]
    pub pad_dynamics: Option<u8>,
    /// Display Light, 0 to 100 %.
    #[serde(default)]
    #[schemars(range(min = 0, max = 100))]
    pub display_light: Option<u8>,
    /// LED Brightness, 0 to 100 %.
    #[serde(default)]
    #[schemars(range(min = 0, max = 100))]
    pub led_brightness: Option<u8>,
    /// Workflow.
    #[serde(default)]
    pub workflow: Option<PushWorkflow>,
}

/// Arguments of `push.display_frame`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PushDisplayFrameArgs {
    /// The row, from 1 to 6. Left out, the first Push 2.
    #[serde(default)]
    pub row: Option<u8>,
}

/// What `push.display_frame` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushDisplayFrame {
    /// The row.
    pub row: u8,
    /// The image's width in pixels.
    pub width: u32,
    /// The image's height in pixels.
    pub height: u32,
    /// The frame as a PNG, base64.
    pub png: String,
}

/// Adds the Push 2 commands' specs to `registry`.
///
/// # Errors
///
/// [`RegisterError`] if an ID is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(STATE, "Push 2 State")
            .description(
                "Reads each Push 2 that runs in the Control Surfaces table: its ports, \
                 firmware version and power supply; its MIDI mode; the pad mode, the display \
                 mode (with Track Mix or Global Mix), any open menu and the buttons held or \
                 locked; each LED's colour index and animation by control name; the touch \
                 strip's configuration and LEDs; the Setup menu's values; and the display's \
                 content as text, by band and column: labels, parameter names and values, \
                 dots, notifications. Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<PushState>(),
    )?;
    registry.register(
        CommandSpec::new(SET_MODE, "Set Push 2 Mode")
            .description(
                "Sets the pad mode (`note` or `session`) and the display mode (`device`, \
                 `browse`, `mix` or `clip`, with Mix's page) of one Push 2 or all of them, as \
                 a tap on their buttons does; an open menu closes. The surface switches on its next \
                 tick. Changes only what Push 2 shows, never the set. Read back by \
                 `push.state`.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<PushSetModeArgs>()
            .result::<PushSetModeResult>(),
    )?;
    registry.register(
        CommandSpec::new(SET_SETUP, "Set Push 2 Setup")
            .description(
                "Sets Push 2's Setup menu: Pad Sensitivity, Pad Gain and Pad Dynamics (0 to \
                 10, which shape the velocity curve), Display Light and LED Brightness (0 to \
                 100 %) and PushWorkflow (`scene` or `clip`), any of them. The values are \
                 preferences, kept between launches and the same for every set, and reach \
                 every Push 2 that runs. Read back by `push.state` (`setup`).",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<PushSetSetupArgs>()
            .result::<PushSetup>(),
    )?;
    registry.register(
        CommandSpec::new(DISPLAY_FRAME, "Push 2 Display Frame")
            .description(
                "Returns what Push 2's display shows as a 960 × 160 PNG, base64, drawn from \
                 the same content `push.state` reports as text. Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<PushDisplayFrameArgs>()
            .result::<PushDisplayFrame>(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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

        let schema = registry.get(SET_SETUP).unwrap().args.as_ref().unwrap();
        assert!(
            bl_commands::validate(schema, &json!({ "pad_gain": 4, "workflow": "clip" })).is_ok()
        );
        assert!(bl_commands::validate(schema, &json!({ "pad_gain": 11 })).is_err());
        assert!(bl_commands::validate(schema, &json!({ "led_brightness": 101 })).is_err());
    }
}

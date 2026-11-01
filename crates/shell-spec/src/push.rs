//! Push 2 for agents (`docs/specs/push-2.md`, R34): `push.state`,
//! `push.set_mode`, `push.set_setup` and `push.display_frame`, with their
//! arguments and results.
//!
//! [`register`] adds their specs, which are pure data; `bl-shell` runs them
//! by ID against the Push 2 surfaces `bl-push` makes.

use std::collections::BTreeMap;

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_model::{ChainId, DeviceId, TrackId};
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

/// One column of Browse Mode (push-2-tracks R14).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushBrowseColumn {
    /// The names of its entries, in order.
    pub entries: Vec<String>,
    /// The selected entry, from 0.
    pub selected: Option<usize>,
}

/// What Browse Mode shows (push-2-tracks R14).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushBrowse {
    /// What Load does: `add`, `replace` or `add_track`.
    pub load: String,
    /// With `replace`, the device Load replaces.
    pub replaces: Option<bl_model::DeviceId>,
    /// The columns, from the leftmost.
    pub columns: Vec<PushBrowseColumn>,
    /// The column in use, from 0.
    pub in_use: usize,
}

/// A device on one of the upper buttons in Device Mode (push-2-tracks R27).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushDevice {
    /// The upper display button, from 1.
    pub button: usize,
    /// The device.
    pub device: bl_model::DeviceId,
    /// Its name.
    pub name: String,
    /// How deep it is inside racks: 0 on the track's own chain.
    pub depth: usize,
    /// What it is: `instrument`, `audio_effect`, `midi_effect`,
    /// `instrument_rack`, `audio_effect_rack`, `midi_effect_rack` or
    /// `drum_rack`.
    pub kind: String,
    /// Whether it is switched on.
    pub enabled: bool,
    /// Whether it is the device in focus, which is the window's selected one.
    pub selected: bool,
    /// For a rack, whether its devices follow it on the buttons.
    pub unfolded: bool,
}

/// A parameter on one of the encoders in Device Mode.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushParam {
    /// Its name.
    pub name: String,
    /// Its value as the device shows it.
    pub value: String,
    /// Whether a macro holds it, so the encoder doesn't move it.
    pub held: bool,
}

/// A chain on one of the lower buttons while a rack's button is held.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushChain {
    /// The chain.
    pub chain: bl_model::ChainId,
    /// Its name.
    pub name: String,
    /// Whether it is the selected chain.
    pub selected: bool,
}

/// What Device Mode shows (push-2-tracks R27).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushDeviceMode {
    /// The devices on the upper buttons, in order.
    pub devices: Vec<PushDevice>,
    /// Whether the first upper button is an arrow that moves the devices
    /// shown back.
    pub left_arrow: bool,
    /// Whether the last upper button is an arrow that moves them on.
    pub right_arrow: bool,
    /// Whether the track has no devices.
    pub empty: bool,
    /// Whether Edit Mode is on.
    pub edit: bool,
    /// The bank on the encoders, from 0.
    pub bank: usize,
    /// Every bank's name.
    pub banks: Vec<String>,
    /// The page of banks the lower buttons show in Edit Mode, from 0.
    pub bank_page: usize,
    /// How many pages the banks make.
    pub bank_pages: usize,
    /// The parameters on the eight encoders.
    pub params: Vec<PushParam>,
    /// The chains the lower buttons show while a rack's button is held.
    pub chains: Vec<PushChain>,
    /// Whether Mute is locked, so each device button switches its device.
    pub mute_locked: bool,
}

/// One control on an encoder in Clip Mode (push-2-tracks R58).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushClipControl {
    /// The encoder, from 1.
    pub encoder: u8,
    /// The control's name.
    pub name: String,
    /// Its value as the display shows it.
    pub value: String,
}

/// The sequenceable area the pads edit in a step sequencing layout
/// (push-2-tracks R51).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushClipArea {
    /// How the display fits the clip: `folded`, `sequencer`,
    /// `sequencer_notes` or `pad`.
    pub fit: String,
    /// Where the page of steps starts, in ticks.
    pub from: bl_core::time::Ticks,
    /// Where it ends.
    pub to: bl_core::time::Ticks,
    /// The lowest pitch the pads edit.
    pub low: u8,
    /// The highest.
    pub high: u8,
}

/// What Clip Mode shows (push-2-tracks R58).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PushClipMode {
    /// The clip shown.
    pub clip: Option<bl_model::ClipId>,
    /// What the mode shows: `midi`, `audio`, `empty_midi`, `empty_audio` or
    /// `nothing`.
    pub shows: String,
    /// The clip's name.
    pub name: String,
    /// Whether the clip's Loop is on.
    pub looping: Option<bool>,
    /// The controls on the encoders, left to right.
    pub controls: Vec<PushClipControl>,
    /// The zoom: 1 shows the whole clip.
    pub zoom: f64,
    /// The span of beats of the clip's contents the display shows.
    pub view: [f64; 2],
    /// Whether the display is folded to the pitches that hold notes.
    pub folded: bool,
    /// The sequenceable area the pads edit.
    pub area: Option<PushClipArea>,
    /// The steps held, whose notes the encoders set.
    pub held_steps: Vec<PushHeldStep>,
    /// The drum pad whose every note the encoders set while Select holds it,
    /// as a MIDI note.
    pub select_pad: Option<u8>,
}

/// What a pad or a Scene/Grid button shows in Session Mode (push-2-tracks
/// R73).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushSessionLook {
    /// Of a pad in the slots: `none` (no track or scene), `empty`,
    /// `stopped`, `queued`, `playing`, `stopping` or `recording`. Of a pad
    /// in Session Overview: `none`, `ring`, `playing` or `block`. Of a
    /// Scene/Grid button: `none`, `empty` or `clip`; in the overview,
    /// `none`, `part` or `shown`.
    pub shows: String,
    /// The clip in the pad's slot.
    pub clip: Option<bl_model::ClipId>,
    /// The swatch of the clip's colour, or of the scene's.
    pub colour: Option<u8>,
}

/// What Session Mode shows (push-2-tracks R73).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushSession {
    /// The ring's first track.
    pub first_track: Option<bl_model::TrackId>,
    /// The ring's first scene.
    pub first_scene: Option<bl_model::SceneId>,
    /// How many tracks the ring covers.
    pub tracks: usize,
    /// How many scenes the ring covers.
    pub scenes: usize,
    /// Whether Session Overview is shown.
    pub overview: bool,
    /// Whether Session Overview is locked on.
    pub overview_locked: bool,
    /// The part of 64 scenes the overview shows, from 0.
    pub overview_part: usize,
    /// Whether Stop Clip is locked.
    pub stop_clip_locked: bool,
    /// The 64 pads, the top row first and each row from the left.
    pub pads: Vec<PushSessionLook>,
    /// The eight Scene/Grid buttons, top to bottom.
    pub scene_buttons: Vec<PushSessionLook>,
}

/// What one encoder sets in Mix Mode (push-2-tracks R42).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PushMixControl {
    /// What it sets: `Volume`, `Pan`, `Pan L`, `Pan R`, `Send A`, an In/Out
    /// chooser's name.
    pub name: String,
    /// Its value as the display shows it.
    pub value: String,
    /// Where a continuous control sits, from 0 to 1; none for a list.
    pub position: Option<f64>,
    /// Whether turning it does nothing: a split-pan track's pan in Global Mix.
    pub inert: bool,
}

/// One of the eight places Mix Mode shows: a track, or a chain of an unfolded
/// rack (push-2-tracks R42).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushMixPlace {
    /// Its name, as the lower display button shows it.
    pub name: String,
    /// The track, or for a chain the track its rack is on.
    pub track: TrackId,
    /// The chain, if the place is one.
    pub chain: Option<ChainId>,
    /// Whether it is the selected place.
    pub selected: bool,
}

/// What Mix Mode shows (push-2-tracks R42).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PushMix {
    /// Track Mix's chosen view: `mix` or `in_out`.
    pub view: String,
    /// What Global Mix sets: `volumes`, `pans` or `send_a`, `send_b` and on.
    pub global: String,
    /// How many places the sends are shifted on the page shown.
    pub sends_shift: usize,
    /// The eight places, left to right.
    pub places: Vec<PushMixPlace>,
    /// What each of the eight encoders sets, left to right; none where it
    /// sets nothing.
    pub encoders: Vec<Option<PushMixControl>>,
    /// The group tracks that are unfolded.
    pub unfolded_groups: Vec<TrackId>,
    /// The racks whose chains are unfolded.
    pub unfolded_racks: Vec<DeviceId>,
}

/// One connected Push 2.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
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
    /// What Browse Mode shows, while it is the display mode.
    pub browse: Option<PushBrowse>,
    /// What Device Mode shows, while it is the display mode.
    pub device: Option<PushDeviceMode>,
    /// What Clip Mode shows, while it is the display mode.
    pub clip: Option<PushClipMode>,
    /// What Session Mode shows, while it is the pad mode.
    pub session: Option<PushSession>,
    /// What Mix Mode shows, while it is the display mode.
    pub mix_mode: Option<PushMix>,
    /// What Note Mode shows, while it is the pad mode.
    pub note: Option<PushNote>,
}

/// A step held down in Note Mode.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushHeldStep {
    /// The clip the step belongs to, if the slot holds one.
    pub clip: Option<bl_model::ClipId>,
    /// The step's drum pad, as a MIDI note.
    pub pad: u8,
    /// Where the step starts in the clip's contents.
    pub start: bl_core::time::Ticks,
    /// How long the step is.
    pub length: bl_core::time::Ticks,
}

/// What a melodic track's layouts show (the notes on Push 2 spec, R32 to
/// R44).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushMelody {
    /// In Key (true) or Chromatic.
    pub in_key: bool,
    /// Whether the bottom-left pad stays on C.
    pub fixed: bool,
    /// The interval between rows: `4ths`, `3rds` or `Sequent`.
    pub layout: String,
    /// `Vertical` or `Horizontal`.
    pub direction: String,
    /// 64 Notes' shift in octaves from where the pads start.
    pub octave: i32,
    /// The lowest note the pads, rows or 32 notes hold, as a MIDI note.
    pub low: u8,
    /// The highest.
    pub high: u8,
    /// The range as the display notifies it, such as `C1 to C3`.
    pub range: String,
    /// The notes selected in the Melodic Sequencer + 32 Notes.
    pub selected_notes: Vec<u8>,
    /// Whether the touch strip is the modulation wheel rather than pitch
    /// bend, in 64 Notes.
    pub strip_modulation: bool,
}

/// What Note Mode shows (the notes on Push 2 spec, R1 to R47).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushNote {
    /// The track the pads play: the first selected one.
    pub track: Option<bl_model::TrackId>,
    /// The layout: `Loop Selector`, `16 Velocities` or `64 Pads` on a drum
    /// track, `64 Notes`, `Melodic Sequencer` or `Melodic Sequencer + 32
    /// Notes` on another MIDI track, `None` on any other track.
    pub layout: String,
    /// What holding Layout, or Shift and Layout, shows in the lower-right
    /// pads, if anything: `16 Velocities` or `Loop Length Pads`.
    pub overlay: Option<String>,
    /// Whether that stays in view after Layout is released.
    pub overlay_locked: bool,
    /// The first note of the drum rack's pad view, which the pads show.
    pub bank: Option<u8>,
    /// The drum pad the steps edit, as a MIDI note.
    pub selected_pad: Option<u8>,
    /// The step size, such as `1/16` or `1/8T`.
    pub step_size: String,
    /// The clip the steps edit, if the slot holds one.
    pub edited_clip: Option<bl_model::ClipId>,
    /// The scene whose slot holds it.
    pub edited_scene: Option<bl_model::SceneId>,
    /// The first of the two pages the step area shows, counting from 1.
    pub page: u8,
    /// Whether the step area follows the page that plays.
    pub following: bool,
    /// The velocity new steps take, once a velocity pad chose one.
    pub velocity: Option<u8>,
    /// The steps held down now.
    pub held_steps: Vec<PushHeldStep>,
    /// What a melodic layout shows, on a melodic track.
    pub melody: Option<PushMelody>,
}

/// What `push.state` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
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
                 dots, notifications; and, in Browse Mode, what Load does, its columns with their \
                 entries and selection and the column in use; in Device Mode, the devices on \
                 the upper buttons with their depth and which is selected, whether Edit Mode is \
                 on, the bank and its page of banks, the parameters on the encoders, the chains \
                 shown while a rack's button is held, and whether Mute is locked; in Mix Mode, \
                 Track Mix's view, what Global Mix sets, the sends' shift, the eight tracks or \
                 chains shown with what each encoder sets and its value, and the groups and \
                 racks that are unfolded; and, in Session Mode, the ring, what each pad and \
                 Scene/Grid button shows, whether Session Overview is shown or locked and the \
                 Stop Clip lock. Changes nothing.",
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

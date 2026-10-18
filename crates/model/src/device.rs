//! Devices in a track's device chain (WP-2.4).
//!
//! A device is a native instrument or effect, or a plug-in, in a track's
//! chain: its kind, its role (MIDI effect, instrument, or audio effect),
//! its names, its activator, and the parameter values the user has set.
//! Each kind of device describes its parameters ([`crate::param`], WP-2.5),
//! which give each value its range.

use imbl::OrdMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ids::{ContentHash, DeviceId, ParamId};
use crate::modulator::{Mapping, ShapePoint};
use crate::rack::{Chain, RackKind};
use crate::track::Input;

/// The most sample slots a device's media can fill.
pub const MAX_MEDIA_SLOTS: usize = 16;

/// A device in a track's chain: a native instrument or effect, or a plug-in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Device {
    /// The device's ID.
    pub id: DeviceId,
    /// The name the title bar shows while Show Preset Name is on: the
    /// device's own name until a preset or Rename gives it another.
    pub name: String,
    /// The device's own name, such as `Auto Filter`, which the title bar
    /// shows while Show Preset Name is off. Empty while it's the same as
    /// [`name`](Self::name), and left out of the saved set then.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub own_name: String,
    /// What the device is.
    pub kind: DeviceKind,
    /// Where it goes in a chain: MIDI effects, then the instrument, then
    /// audio effects. Audio effects aren't marked in the saved set.
    #[serde(default, skip_serializing_if = "DeviceRole::is_audio_effect")]
    pub role: DeviceRole,
    /// Whether the device is switched on. A device that's off passes its
    /// input through unchanged.
    pub enabled: bool,
    /// Whether the title bar shows the preset's or the given name (on, as a
    /// new device is) or the device's own name: the context menu's Show
    /// Preset Name. Left out of the saved set while on.
    #[serde(default = "on", skip_serializing_if = "is_on")]
    pub show_preset_name: bool,
    /// Parameter values the user has set, in the parameter's own units.
    /// Every value is finite. Parameters not listed keep their defaults.
    ///
    /// They serialise as a list of `[parameter, value]` pairs, since some
    /// formats can't key a map by number.
    #[serde(with = "param_pairs")]
    #[schemars(with = "Vec<(ParamId, f64)>")]
    pub params: OrdMap<ParamId, f64>,
    /// Whether the device's B state is the active one, whose values
    /// [`params`](Self::params) holds (device compare R1). Left out of the
    /// saved set while A is active.
    #[serde(default, skip_serializing_if = "is_off")]
    pub b_active: bool,
    /// The values of the state that isn't active, in the same form as
    /// [`params`](Self::params) (device compare R1, R10). `None` while the
    /// two states are equal, as a new device's are, and left out of the
    /// saved set then. Use [`other_params`](Self::other_params) to read it.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "other_pairs")]
    #[schemars(with = "Option<Vec<(ParamId, f64)>>")]
    pub other_params: Option<OrdMap<ParamId, f64>>,
    /// The device's sidechain input, if its type declares one (WP-2.19b):
    /// where its detector can listen other than its own input. Left out
    /// of the saved set for a device without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidechain: Option<Sidechain>,
    /// Whether the device floats in a window of its own rather than
    /// showing in the Device View (IMP-14, WP-2.5b), where its place in the
    /// chain shows as a strip. Only native devices float. Left out of the
    /// saved set while docked. Where its window sits isn't part of the
    /// document: moving or resizing a window isn't an undo step
    /// ([`WindowFrame`]).
    #[serde(default, skip_serializing_if = "is_off")]
    pub floating: bool,
    /// A plug-in's own state and where it was loaded from (WP-2.6), as
    /// last saved with the set. Left out of the saved set for a native
    /// device, and for a plug-in until the set is first saved with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<PluginState>,
    /// The media the device plays, by content hash, one entry per sample
    /// slot (WP-2.16, WP-5.17): the sampler's sample is slot 0, the
    /// eight-slot drum sampler's slots are 0 to 7. The media store holds
    /// each like a clip's audio (the sampler spec, R11). A slot with no
    /// sample is `None`, and the list never ends in one. Left out of the
    /// saved set for a device that plays none; a set saved before slots
    /// existed holds one hash, which loads as slot 0.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "media_slots"
    )]
    pub media: Vec<Option<ContentHash>>,
    /// The parameters a modulator maps, one per slot (the modulation spec,
    /// R19). Left out of the saved set for a device that maps none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mappings: Vec<Mapping>,
    /// The breakpoints of a Curve Mod's shape (R23); `None` plays the
    /// default shape. Left out of the saved set while it is the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<Vec<ShapePoint>>,
    /// A rack's chains, in order (WP-5.1). Empty for every device that
    /// isn't a rack, and left out of the saved set then, so sets saved
    /// before racks load unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chains: Vec<Chain>,
}

/// A plug-in device's own state, as the set file keeps it (WP-2.6).
///
/// The running plug-in's state moves ahead of this as it plays and its
/// editor changes it; the host takes it again (an amendment, never an undo
/// step) each time the set is saved, and when the device is copied.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "A plug-in's own state: `path`, the file it was loaded from, and `data`, the \
                   state it saved, in base64."
)]
pub struct PluginState {
    /// The plug-in file or bundle it was loaded from, so a reopened set
    /// finds it before the plug-in scanner (WP-2.9) can by its ID.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    /// The state the plug-in saved, in the plug-in's own format; empty for
    /// none. Base64 in the set file.
    #[serde(default, with = "base64_bytes", skip_serializing_if = "Vec::is_empty")]
    #[schemars(with = "String")]
    pub data: Vec<u8>,
}

/// Serialises bytes as base64 text.
mod base64_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&crate::base64::encode(bytes))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        crate::base64::decode(&text).ok_or_else(|| serde::de::Error::custom("not base64"))
    }
}

/// Where a floating device's window sits and how big it is (IMP-14,
/// WP-2.5b), in points on the screen: the top-left corner of its outer
/// frame, and the size of what it shows inside. It's saved with the set
/// beside the document, so moving or resizing a window is never an undo
/// step.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WindowFrame {
    /// The left edge of the window's outer frame, in points from the left
    /// of the main screen.
    pub x: f64,
    /// The top of the window's outer frame, in points from the top of the
    /// main screen.
    pub y: f64,
    /// The width of the window's contents, in points.
    pub width: f64,
    /// The height of the window's contents, in points.
    pub height: f64,
}

impl WindowFrame {
    /// Whether every number is finite and the size is above 0.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|v| v.is_finite())
            && self.width > 0.0
            && self.height > 0.0
    }
}

/// Each floating device's window frame, as a set saves them beside its
/// document (WP-2.5b).
pub type DeviceWindows = std::collections::BTreeMap<DeviceId, WindowFrame>;

/// A device's sidechain input and the settings of its sidechain section
/// (the routing spec, WP-2.19b): External, Source with its channel, Mix,
/// SC Gain, and Sidechain Listen.
///
/// What feeds it is the track its Source names, tapped where its channel
/// says; in Live 12 tracks aren't routed into a sidechain from their Audio
/// To (observed). While External is on, the device's detector hears that
/// signal scaled by SC Gain, mixed with the device's own input by Mix;
/// while it's off, its own input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "A device's sidechain input: `enabled` (the External switch), `input` \
                   (Source: `none`, or `track` with a track and a `tap`), `gain` (SC Gain, \
                   linear, 1 is 0 dB), `mix` (0 the device's own input to 1 the sidechain \
                   alone), and `listen` (Sidechain Listen: hear what the detector hears)."
)]
pub struct Sidechain {
    /// External: whether the device's detector listens to the sidechain
    /// rather than its own input. Off on a new device.
    #[serde(default, skip_serializing_if = "is_off")]
    pub enabled: bool,
    /// Source: No Input on a new device, or a track tapped Pre FX, Post
    /// FX (what choosing a track picks), or Post Mixer.
    #[serde(default = "no_input", skip_serializing_if = "is_no_input")]
    pub input: Input,
    /// SC Gain, linear: 1 (0 dB) on a new device.
    #[serde(default = "unity", skip_serializing_if = "is_unity")]
    pub gain: f64,
    /// Mix: how much of the detector's signal is the sidechain, from 0
    /// (the device's own input) to 1 (the sidechain alone, 100 %, as on a
    /// new device).
    #[serde(default = "unity", skip_serializing_if = "is_unity")]
    pub mix: f64,
    /// Sidechain Listen: the device's output is what its detector hears.
    #[serde(default, skip_serializing_if = "is_off")]
    pub listen: bool,
}

impl Default for Sidechain {
    /// A new device's (observed): External off, No Input, Mix 100 %, SC
    /// Gain 0 dB, not listening.
    fn default() -> Self {
        Self {
            enabled: false,
            input: Input::None,
            gain: 1.0,
            mix: 1.0,
            listen: false,
        }
    }
}

/// Reads a device's media: a list of slots, or the one hash (or `null`) a
/// set saved before sample slots held, which is slot 0.
fn media_slots<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<Option<ContentHash>>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Saved {
        Slots(Vec<Option<ContentHash>>),
        One(Option<ContentHash>),
    }
    Ok(match Saved::deserialize(deserializer)? {
        Saved::Slots(slots) => slots,
        Saved::One(one) => one.into_iter().map(Some).collect(),
    })
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_off(value: &bool) -> bool {
    !*value
}

fn no_input() -> Input {
    Input::None
}

fn is_no_input(input: &Input) -> bool {
    *input == Input::None
}

fn unity() -> f64 {
    1.0
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_unity(value: &f64) -> bool {
    *value == 1.0
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_on(value: &bool) -> bool {
    *value
}

fn on() -> bool {
    true
}

impl Device {
    /// A switched-on audio effect with default parameters, named `name`.
    #[must_use]
    pub fn new(id: DeviceId, name: impl Into<String>, kind: DeviceKind) -> Self {
        Self {
            id,
            name: name.into(),
            own_name: String::new(),
            kind,
            role: DeviceRole::AudioEffect,
            enabled: true,
            show_preset_name: true,
            params: OrdMap::new(),
            b_active: false,
            other_params: None,
            sidechain: None,
            floating: false,
            plugin: None,
            media: Vec::new(),
            mappings: Vec::new(),
            shape: None,
            chains: Vec::new(),
        }
    }

    /// The media in sample slot `slot`, if it holds any.
    #[must_use]
    pub fn media_at(&self, slot: usize) -> Option<ContentHash> {
        self.media.get(slot).copied().flatten()
    }

    /// Puts `media` in sample slot `slot`, growing the list to reach it and
    /// trimming empty slots off its end.
    pub fn set_media(&mut self, slot: usize, media: Option<ContentHash>) {
        if self.media.len() <= slot {
            if media.is_none() {
                return;
            }
            self.media.resize(slot + 1, None);
        }
        self.media[slot] = media;
        while self.media.last() == Some(&None) {
            self.media.pop();
        }
    }

    /// Whether the device can float in a window of its own: native devices
    /// can; plug-ins open their own editor windows, as in Live (IMP-14).
    #[must_use]
    pub fn can_float(&self) -> bool {
        matches!(self.kind, DeviceKind::Native { .. })
    }

    /// Whether the device has A and B states: native devices do; racks
    /// and plug-ins don't, as in Live (device compare R6).
    #[must_use]
    pub fn can_compare(&self) -> bool {
        matches!(self.kind, DeviceKind::Native { .. })
    }

    /// The values of the state that isn't active.
    #[must_use]
    pub fn other_params(&self) -> &OrdMap<ParamId, f64> {
        self.other_params.as_ref().unwrap_or(&self.params)
    }

    /// Sets one parameter in the active state only (device compare R2).
    pub(crate) fn set_param(&mut self, param: ParamId, value: f64) {
        let before = self.params.clone();
        self.params.insert(param, value);
        if self.can_compare() {
            self.other_params.get_or_insert(before);
            self.forget_equal_other();
        }
    }

    /// Sets both states to `params`, with A active, as a device starts,
    /// loading a preset does (device compare R9) and a copy does (R8).
    pub fn set_both(&mut self, params: OrdMap<ParamId, f64>) {
        self.params = params;
        self.b_active = false;
        self.other_params = None;
    }

    /// Makes the other state active (device compare R3).
    pub(crate) fn compare_switch(&mut self) {
        let other = self
            .other_params
            .take()
            .unwrap_or_else(|| self.params.clone());
        self.other_params = Some(std::mem::replace(&mut self.params, other));
        self.b_active = !self.b_active;
        self.forget_equal_other();
    }

    /// Copies the active state over the other (device compare R4).
    pub(crate) fn compare_copy(&mut self) {
        self.other_params = None;
    }

    fn forget_equal_other(&mut self) {
        if self.other_params.as_ref() == Some(&self.params) {
            self.other_params = None;
        }
    }

    /// The same device with a sidechain input, as a new device has.
    #[must_use]
    pub fn with_sidechain(mut self) -> Self {
        self.sidechain = Some(Sidechain::default());
        self
    }

    /// The same device with another role.
    #[must_use]
    pub fn with_role(mut self, role: DeviceRole) -> Self {
        self.role = role;
        self
    }

    /// The device's own name, such as `Auto Filter`, whatever a preset or
    /// Rename called it.
    #[must_use]
    pub fn own_name(&self) -> &str {
        if self.own_name.is_empty() {
            &self.name
        } else {
            &self.own_name
        }
    }

    /// What its title bar shows: its preset's or given name while Show
    /// Preset Name is on, and its own name while it's off.
    #[must_use]
    pub fn title(&self) -> &str {
        if self.show_preset_name {
            &self.name
        } else {
            self.own_name()
        }
    }

    /// Gives the device a new title name (a preset's, or Rename's),
    /// remembering its own name. An empty name gives it its own name back.
    pub(crate) fn set_name(&mut self, name: &str) {
        if self.own_name.is_empty() {
            self.own_name.clone_from(&self.name);
        }
        if name.is_empty() {
            self.name = std::mem::take(&mut self.own_name);
        } else {
            name.clone_into(&mut self.name);
        }
        if self.own_name == self.name {
            self.own_name.clear();
        }
    }
}

/// Where a device goes in a chain. A chain holds its MIDI effects first,
/// then its instrument, then its audio effects, as Live's does; only MIDI
/// tracks hold MIDI effects and instruments, and a chain holds at most one
/// instrument (a rack holds several, one to a chain).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DeviceRole {
    /// Takes MIDI and gives MIDI, before the instrument.
    MidiEffect,
    /// Takes MIDI and makes audio.
    Instrument,
    /// Takes audio and gives audio, after the instrument.
    #[default]
    AudioEffect,
}

impl DeviceRole {
    /// Whether it's an audio effect.
    #[must_use]
    pub fn is_audio_effect(&self) -> bool {
        *self == Self::AudioEffect
    }

    /// The name of its kind of device, as the browser's labels and the
    /// hot-swap bar name it: `Audio Effect`, `Instrument`, `MIDI Effect`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::MidiEffect => "MIDI Effect",
            Self::Instrument => "Instrument",
            Self::AudioEffect => "Audio Effect",
        }
    }

    /// Whether a track of `kind` can hold a device with this role: MIDI
    /// effects and instruments only go on MIDI tracks.
    #[must_use]
    pub fn fits(self, kind: crate::TrackKind) -> bool {
        self == Self::AudioEffect || kind == crate::TrackKind::Midi
    }
}

/// What a device is.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    /// A device built into Beatlayer.
    Native {
        /// The device type's stable identifier, such as `beatlayer.utility`.
        device_type: String,
    },
    /// A plug-in.
    Plugin {
        /// The plug-in's format.
        format: PluginFormat,
        /// The plug-in's identifier in its format: a CLAP ID, a VST3 class
        /// ID, an Audio Unit's type, subtype, and manufacturer, or an LV2
        /// URI.
        plugin_id: String,
    },
    /// A rack (WP-5.1): a device that holds [`Device::chains`].
    Rack {
        /// Which of the four kinds of rack it is.
        kind: RackKind,
    },
}

/// A plug-in format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PluginFormat {
    /// CLAP.
    Clap,
    /// VST3.
    Vst3,
    /// Audio Unit (macOS).
    AudioUnit,
    /// LV2 (Linux).
    Lv2,
}

/// Serialises an optional set of parameter values as `param_pairs` does.
mod other_pairs {
    use imbl::OrdMap;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::param_pairs;
    use crate::ids::ParamId;

    struct Pairs(OrdMap<ParamId, f64>);

    impl Serialize for Pairs {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            param_pairs::serialize(&self.0, serializer)
        }
    }

    impl<'de> Deserialize<'de> for Pairs {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            param_pairs::deserialize(deserializer).map(Self)
        }
    }

    #[expect(clippy::ref_option, reason = "serde passes a reference")]
    pub(super) fn serialize<S: Serializer>(
        params: &Option<OrdMap<ParamId, f64>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        params.clone().map(Pairs).serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<OrdMap<ParamId, f64>>, D::Error> {
        Ok(Option::<Pairs>::deserialize(deserializer)?.map(|pairs| pairs.0))
    }
}

/// Serialises parameter values as a list of `[parameter, value]` pairs.
pub(crate) mod param_pairs {
    use core::fmt;

    use imbl::OrdMap;
    use serde::de::{Error, SeqAccess, Visitor};
    use serde::ser::SerializeSeq;
    use serde::{Deserializer, Serializer};

    use crate::ids::ParamId;

    pub(crate) fn serialize<S: Serializer>(
        params: &OrdMap<ParamId, f64>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(params.len()))?;
        for pair in params {
            seq.serialize_element(&pair)?;
        }
        seq.end()
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<OrdMap<ParamId, f64>, D::Error> {
        deserializer.deserialize_seq(Pairs)
    }

    struct Pairs;

    impl<'de> Visitor<'de> for Pairs {
        type Value = OrdMap<ParamId, f64>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a list of [parameter, value] pairs")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut params = OrdMap::new();
            while let Some((param, value)) = seq.next_element::<(ParamId, f64)>()? {
                if params.insert(param, value).is_some() {
                    return Err(A::Error::custom(format_args!(
                        "parameter {} is listed twice",
                        param.0
                    )));
                }
            }
            Ok(params)
        }
    }
}

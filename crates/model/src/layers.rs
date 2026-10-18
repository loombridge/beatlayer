//! A multisample's layers (WP-5.13; the multisampler spec, R2 to R5, R12,
//! R20): the ordered list a multisampler holds, each layer a sample with
//! three zones and playback settings of its own.
//!
//! Layer IDs belong to their device: a copied device keeps its layers' IDs,
//! and each device numbers its own from a counter that never reuses one.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ModelError;
use crate::ids::ContentHash;

/// The lowest gain a layer's Volume reaches: silence.
pub const LAYER_GAIN_MIN: f64 = 0.0;

/// The highest gain a layer's Volume reaches, +24 dB as a linear gain.
pub const LAYER_GAIN_MAX: f64 = 15.848_931_924_611_135;

/// A layer's pan runs from −50 (left) to 50 (right).
pub const LAYER_PAN_RANGE: f64 = 50.0;

/// A layer's Detune and loop Detune run from −50 to 50 cents.
pub const LAYER_DETUNE_RANGE: f64 = 50.0;

/// A layer's ID, which is unique within its device.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
#[schemars(description = "The ID of a layer: a positive integer, unique within its device.")]
pub struct LayerId(pub u32);

/// Which of a layer's three zones.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ZoneKind {
    /// The notes it plays over, 0 to 127.
    #[default]
    Key,
    /// The note-on velocities it plays at, 1 to 127.
    Velocity,
    /// The Sample Selector values it plays at, 0 to 127.
    Select,
}

impl ZoneKind {
    /// The lowest and highest value of the zone's scale.
    #[must_use]
    pub const fn range(self) -> (u8, u8) {
        match self {
            Self::Key | Self::Select => (0, 127),
            Self::Velocity => (1, 127),
        }
    }
}

/// How a zone's fade turns its share into a gain (R5): the device's one
/// Fade Mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FadeMode {
    /// The gain is the share.
    Lin,
    /// The gain is sin(share · π/2): constant power across a crossfade.
    Pow,
}

/// A range of values with a fade at each end (R5): the layer is silent at
/// the lowest and highest value when it has a fade there, and reaches full
/// level `fade_low` values above the lowest and `fade_high` below the
/// highest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Zone {
    /// The lowest value in the zone.
    pub low: u8,
    /// The highest value in the zone.
    pub high: u8,
    /// How many values the fade at the low end spans; 0 for none.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub fade_low: u8,
    /// How many values the fade at the high end spans; 0 for none.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub fade_high: u8,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_zero(value: &u8) -> bool {
    *value == 0
}

impl Zone {
    /// A zone over `low` to `high` with no fades.
    #[must_use]
    pub const fn new(low: u8, high: u8) -> Self {
        Self {
            low,
            high,
            fade_low: 0,
            fade_high: 0,
        }
    }

    /// The zone over a kind's whole scale, as a new layer has.
    #[must_use]
    pub const fn whole(kind: ZoneKind) -> Self {
        let (low, high) = kind.range();
        Self::new(low, high)
    }

    /// Whether the zone sits inside `kind`'s scale, holds at least one
    /// value, and its fades stay inside it (R10).
    #[must_use]
    pub fn is_valid(&self, kind: ZoneKind) -> bool {
        let (min, max) = kind.range();
        min <= self.low
            && self.low <= self.high
            && self.high <= max
            && u16::from(self.fade_low) + u16::from(self.fade_high)
                <= u16::from(self.high - self.low)
    }

    /// Whether `value` is inside the zone, fades included.
    #[must_use]
    pub fn contains(&self, value: u8) -> bool {
        self.low <= value && value <= self.high
    }

    /// The gain the zone gives `value` (R5): 0 outside it, 1 between its
    /// fades, and in a fade the share of the way from the silent edge to
    /// the full one, as `mode` turns it into a gain.
    #[must_use]
    pub fn gain(&self, value: u8, mode: FadeMode) -> f32 {
        if !self.contains(value) {
            return 0.0;
        }
        let rise = |from_edge: u8, span: u8| {
            if span == 0 || from_edge >= span {
                1.0
            } else {
                let share = f32::from(from_edge) / f32::from(span);
                match mode {
                    FadeMode::Lin => share,
                    FadeMode::Pow => (share * core::f32::consts::FRAC_PI_2).sin(),
                }
            }
        };
        rise(value - self.low, self.fade_low) * rise(self.high - value, self.fade_high)
    }
}

/// What a voice does between Sample Start and Sample End while a note is
/// held (R17).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SustainMode {
    /// Plays straight through.
    #[default]
    Off,
    /// Plays to Loop End, goes on from Loop Start, and repeats.
    Loop,
    /// Plays to Loop End, then backwards to Loop Start, and repeats.
    BackAndForth,
}

/// What a note's end does while a sustain loop is on (R18).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseMode {
    /// Keeps the voice in the sustain loop until the release ends.
    #[default]
    Off,
    /// Plays on through Loop End to Sample End.
    Release,
    /// Plays on, then loops from the release loop point to Sample End.
    Loop,
    /// As `loop`, back and forth.
    BackAndForth,
}

/// One sample of a multisample (R2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    /// The layer's ID, unique within its device.
    pub id: LayerId,
    /// Its name: the file's name until renamed.
    pub name: String,
    /// Its sample, which the media store holds.
    pub media: ContentHash,
    /// The sample's length in frames at its own rate.
    pub length: u64,
    /// The sample's own rate, in hertz: the unit its markers count in.
    pub sample_rate: u32,
    /// How many channels the sample has.
    pub channels: u8,
    /// The notes it plays over.
    pub key: Zone,
    /// The velocities it plays at.
    pub velocity: Zone,
    /// The Sample Selector values it plays at.
    pub select: Zone,
    /// The key that plays the sample at its own pitch, 0 to 127 (R15).
    pub root_key: u8,
    /// Its tuning, in cents from −50 to 50.
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub detune: f64,
    /// Its level as a linear gain from 0 (−∞ dB) to +24 dB (rule G1).
    pub volume: f64,
    /// Its place in the stereo field, −50 to 50.
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub pan: f64,
    /// Sample Start, in frames of the sample (R16).
    pub start: u64,
    /// Sample End, in frames of the sample.
    pub end: u64,
    /// Link: the voice starts at Loop Start instead of Sample Start. The
    /// Sample Start it keeps comes back when Link goes off.
    #[serde(default, skip_serializing_if = "is_false")]
    pub link: bool,
    /// The sustain loop's kind (R17).
    #[serde(default)]
    pub sustain: SustainMode,
    /// What the note's end does inside a sustain loop (R18).
    #[serde(default)]
    pub release: ReleaseMode,
    /// Loop Start, in frames.
    pub loop_start: u64,
    /// Loop End, in frames.
    pub loop_end: u64,
    /// Where the release loop begins, between Loop End and Sample End.
    pub release_loop: u64,
    /// The loop's crossfade in milliseconds (R19).
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub crossfade_ms: f64,
    /// How the voice is retuned inside a loop, in cents (R19).
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub loop_detune: f64,
    /// Whether the layer's voices play on to Sample End whatever the note
    /// does (R50's `one_shot`). No control in the Sample tab sets it: an SFZ
    /// import does, and `device.update_layers`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub one_shot: bool,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_zero_f64(value: &f64) -> bool {
    *value == 0.0
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_false(value: &bool) -> bool {
    !*value
}

/// C3, the key a new layer is rooted on when its file names none.
pub const DEFAULT_ROOT_KEY: u8 = 60;

impl Layer {
    /// A layer for a sample of `length` frames at `sample_rate`, as R3 makes
    /// one: every zone whole with no fades, rooted on C3, the whole sample
    /// played, no sustain loop.
    #[must_use]
    pub fn new(
        id: LayerId,
        name: impl Into<String>,
        media: ContentHash,
        length: u64,
        sample_rate: u32,
        channels: u8,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            media,
            length,
            sample_rate,
            channels,
            key: Zone::whole(ZoneKind::Key),
            velocity: Zone::whole(ZoneKind::Velocity),
            select: Zone::whole(ZoneKind::Select),
            root_key: DEFAULT_ROOT_KEY,
            detune: 0.0,
            volume: 1.0,
            pan: 0.0,
            start: 0,
            end: length,
            link: false,
            sustain: SustainMode::Off,
            release: ReleaseMode::Off,
            loop_start: 0,
            loop_end: length,
            release_loop: length,
            crossfade_ms: 0.0,
            loop_detune: 0.0,
            one_shot: false,
        }
    }

    /// The zone of `kind`.
    #[must_use]
    pub fn zone(&self, kind: ZoneKind) -> Zone {
        match kind {
            ZoneKind::Key => self.key,
            ZoneKind::Velocity => self.velocity,
            ZoneKind::Select => self.select,
        }
    }

    fn zone_mut(&mut self, kind: ZoneKind) -> &mut Zone {
        match kind {
            ZoneKind::Key => &mut self.key,
            ZoneKind::Velocity => &mut self.velocity,
            ZoneKind::Select => &mut self.select,
        }
    }

    /// The longest crossfade the loop's length allows: half of it, in
    /// milliseconds.
    #[must_use]
    pub fn crossfade_limit_ms(&self) -> f64 {
        let frames = self.loop_end.saturating_sub(self.loop_start) as f64;
        frames / f64::from(self.sample_rate.max(1)) * 500.0
    }

    /// Checks every setting against its range and the markers' order
    /// (R2, R15 to R19): Sample Start before Sample End inside the sample,
    /// Sample Start ≤ Loop Start < Loop End ≤ the release loop ≤ Sample End.
    ///
    /// # Errors
    ///
    /// [`ModelError::InvalidValue`] naming the first setting out of range.
    pub fn check(&self) -> Result<(), ModelError> {
        let bad = |what: &'static str, value: f64| Err(ModelError::InvalidValue { what, value });
        for kind in [ZoneKind::Key, ZoneKind::Velocity, ZoneKind::Select] {
            if !self.zone(kind).is_valid(kind) {
                return bad("zone", f64::from(self.zone(kind).low));
            }
        }
        if self.sample_rate == 0 {
            return bad("sample rate", 0.0);
        }
        if self.length == 0 {
            return bad("layer length", 0.0);
        }
        if self.root_key > 127 {
            return bad("root key", f64::from(self.root_key));
        }
        let in_range = |v: f64, limit: f64| v.is_finite() && v.abs() <= limit;
        if !in_range(self.detune, LAYER_DETUNE_RANGE) {
            return bad("layer detune", self.detune);
        }
        if !in_range(self.loop_detune, LAYER_DETUNE_RANGE) {
            return bad("loop detune", self.loop_detune);
        }
        if !in_range(self.pan, LAYER_PAN_RANGE) {
            return bad("layer pan", self.pan);
        }
        if !(self.volume.is_finite() && (LAYER_GAIN_MIN..=LAYER_GAIN_MAX).contains(&self.volume)) {
            return bad("layer volume", self.volume);
        }
        if !(self.crossfade_ms.is_finite()
            && (0.0..=self.crossfade_limit_ms() + 1e-9).contains(&self.crossfade_ms))
        {
            return bad("loop crossfade", self.crossfade_ms);
        }
        if !(self.start < self.end && self.end <= self.length) {
            return bad("sample start", self.start as f64);
        }
        if !(self.start <= self.loop_start && self.loop_start < self.loop_end) {
            return bad("loop start", self.loop_start as f64);
        }
        if !(self.loop_end <= self.release_loop && self.release_loop <= self.end) {
            return bad("loop end", self.loop_end as f64);
        }
        Ok(())
    }
}

/// Settings of a layer to change, each left as it was when `None`
/// (`device.update_layers`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayerSettings {
    /// Root Key, 0 to 127.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_key: Option<u8>,
    /// Detune, −50 to 50 cents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detune: Option<f64>,
    /// Volume, a linear gain from 0 to +24 dB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<f64>,
    /// Pan, −50 to 50.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pan: Option<f64>,
    /// Sample Start, in frames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<u64>,
    /// Sample End, in frames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<u64>,
    /// Link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<bool>,
    /// Sustain Mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sustain: Option<SustainMode>,
    /// Release Mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<ReleaseMode>,
    /// Loop Start, in frames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_start: Option<u64>,
    /// Loop End, in frames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_end: Option<u64>,
    /// The Release Loop point, in frames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_loop: Option<u64>,
    /// The loop's crossfade in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crossfade_ms: Option<f64>,
    /// The loop's Detune, −50 to 50 cents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_detune: Option<f64>,
    /// Whether the layer's voices play on to Sample End whatever the note
    /// does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub one_shot: Option<bool>,
}

impl LayerSettings {
    /// Sets the markers: for the shell's Snap, which settles where they land.
    #[must_use]
    pub fn markers(
        start: Option<u64>,
        end: Option<u64>,
        loop_start: Option<u64>,
        loop_end: Option<u64>,
        release_loop: Option<u64>,
    ) -> Self {
        Self {
            start,
            end,
            loop_start,
            loop_end,
            release_loop,
            ..Self::default()
        }
    }

    /// Applies the settings to `layer`: a crossfade the loop's new length no
    /// longer allows is shortened to what it allows, unless the settings set
    /// the crossfade themselves.
    ///
    /// # Errors
    ///
    /// The layer's own check; the layer is left as it was when it fails.
    pub fn apply(&self, layer: &mut Layer) -> Result<(), ModelError> {
        let mut next = layer.clone();
        macro_rules! take {
            ($($field:ident),*) => { $(if let Some(v) = self.$field { next.$field = v; })* };
        }
        take!(
            root_key,
            detune,
            volume,
            pan,
            start,
            end,
            link,
            sustain,
            release,
            loop_start,
            loop_end,
            release_loop,
            crossfade_ms,
            loop_detune,
            one_shot
        );
        // Markers the settings leave alone follow the ones they move: a loop
        // that no longer fits inside the sample's new span is held to it.
        if self.loop_end.is_none() {
            next.loop_end = next
                .loop_end
                .max(next.start.saturating_add(1))
                .min(next.end.max(1));
        }
        if self.loop_start.is_none() {
            next.loop_start = next
                .loop_start
                .max(next.start)
                .min(next.loop_end.saturating_sub(1).max(next.start));
        }
        if self.release_loop.is_none() {
            next.release_loop = next
                .release_loop
                .max(next.loop_end)
                .min(next.end.max(next.loop_end));
        }
        if self.crossfade_ms.is_none() {
            next.crossfade_ms = next.crossfade_ms.min(next.crossfade_limit_ms());
        }
        next.check()?;
        *layer = next;
        Ok(())
    }
}

/// One layer's zone of a kind (`device.set_layer_zone`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayerZone {
    /// The layer.
    pub layer: LayerId,
    /// Its new zone.
    pub zone: Zone,
}

/// How the layer list is sorted (R11).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LayerSort {
    /// Alphabetically by name, ignoring case.
    Name,
    /// By the key zone's lowest key.
    Key,
    /// By the velocity zone's lowest velocity.
    Velocity,
    /// By the sample select zone's lowest value.
    Selector,
}

/// How Distribute Ranges spreads layers (R12).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Distribution {
    /// Equal shares of the range of the editor in view.
    Equally,
    /// Key zones split where neighbouring root keys meet.
    AroundRootKey,
}

/// The tab a multisampler shows in its body (R1); Zone isn't one, since it
/// opens the zone editor above the Device View instead.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SamplerTab {
    /// The selected layer's waveform and playback.
    #[default]
    Sample,
    /// Pitch and the modulation oscillator.
    PitchOsc,
    /// The filter and the global controls.
    FilterGlobal,
    /// The auxiliary envelope and the LFOs.
    Modulation,
    /// The MIDI sources.
    Midi,
}

/// How tall the zone editor's rows are (R10): a right-click offers Small,
/// Medium and Large, which change only the view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EditorSize {
    /// The smallest rows.
    Small,
    /// The default rows.
    #[default]
    Medium,
    /// The tallest rows.
    Large,
}

/// What a multisampler's views show, saved with the set but no part of the
/// document, so changing it is no undo step (R1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MultisamplerView {
    /// The tab in the body.
    #[serde(default)]
    pub tab: SamplerTab,
    /// Whether the zone editor shows above the Device View.
    #[serde(default)]
    pub zone_editor: bool,
    /// Which zone editor the Key, Vel and Sel chooser shows (R10).
    #[serde(default = "key_editor")]
    pub editor: ZoneKind,
    /// The zone editor's row size.
    #[serde(default)]
    pub size: EditorSize,
    /// Auto Select (R7).
    #[serde(default)]
    pub auto_select: bool,
}

fn key_editor() -> ZoneKind {
    ZoneKind::Key
}

/// Each multisampler's view, as a set saves them beside its document.
pub type MultisamplerViews = std::collections::BTreeMap<crate::ids::DeviceId, MultisamplerView>;

/// The layers a multisampler holds and the settings of the layer list that
/// aren't parameters (R2, R19, R20, R23).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Layers {
    /// The layers, in list order.
    pub list: Vec<Layer>,
    /// The last layer ID given out: the next is one more.
    pub last_id: u32,
    /// RAM Mode: every layer decoded whole into memory (R23).
    #[serde(default, skip_serializing_if = "is_false")]
    pub ram_mode: bool,
    /// Snap: markers land on zero crossings (R20).
    #[serde(default, skip_serializing_if = "is_false")]
    pub snap: bool,
    /// The title bar's Use Constant Power Fade for Loops being off (R19,
    /// R29); constant power is the default.
    #[serde(default, skip_serializing_if = "is_false")]
    pub linear_loop_fades: bool,
}

impl Layers {
    /// Whether nothing is set: no layers, and every option as a new
    /// device has it. Such a device saves none.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The layer with `id`.
    #[must_use]
    pub fn get(&self, id: LayerId) -> Option<&Layer> {
        self.list.iter().find(|l| l.id == id)
    }

    /// The place of the layer with `id` in the list.
    #[must_use]
    pub fn position(&self, id: LayerId) -> Option<usize> {
        self.list.iter().position(|l| l.id == id)
    }

    fn require(&self, id: LayerId) -> Result<usize, ModelError> {
        self.position(id).ok_or(ModelError::NoSuchLayer(id))
    }

    fn next_id(&mut self) -> LayerId {
        self.last_id += 1;
        LayerId(self.last_id)
    }

    /// The layers a note plays (R4): those whose key, velocity and select
    /// zones hold `key`, `velocity` (1 to 127) and `selector`.
    #[must_use]
    pub fn matching(&self, key: u8, velocity: u8, selector: u8) -> Vec<LayerId> {
        self.list
            .iter()
            .filter(|l| {
                l.key.contains(key) && l.velocity.contains(velocity) && l.select.contains(selector)
            })
            .map(|l| l.id)
            .collect()
    }

    /// Checks the list: valid layers with unique IDs no larger than
    /// `last_id`.
    ///
    /// # Errors
    ///
    /// The first problem found.
    pub fn check(&self) -> Result<(), ModelError> {
        let mut seen = std::collections::BTreeSet::new();
        for layer in &self.list {
            if layer.id.0 == 0 || layer.id.0 > self.last_id || !seen.insert(layer.id) {
                return Err(ModelError::InvalidValue {
                    what: "layer id",
                    value: f64::from(layer.id.0),
                });
            }
            layer.check()?;
        }
        Ok(())
    }

    /// Adds `layers` at `index` (the end if `None`), giving each the next
    /// of the device's IDs, and returns the IDs.
    ///
    /// # Errors
    ///
    /// An index past the end, or a layer out of range; nothing is added.
    pub fn insert(
        &mut self,
        index: Option<usize>,
        layers: &[Layer],
    ) -> Result<Vec<LayerId>, ModelError> {
        let at = index.unwrap_or(self.list.len());
        if at > self.list.len() {
            return Err(ModelError::IndexOutOfRange {
                index: at,
                len: self.list.len(),
            });
        }
        layers.iter().try_for_each(Layer::check)?;
        let mut ids = Vec::with_capacity(layers.len());
        for (i, layer) in layers.iter().enumerate() {
            let mut layer = layer.clone();
            layer.id = self.next_id();
            ids.push(layer.id);
            self.list.insert(at + i, layer);
        }
        Ok(ids)
    }

    /// Removes the listed layers.
    ///
    /// # Errors
    ///
    /// A layer that isn't there; nothing is removed.
    pub fn remove(&mut self, ids: &[LayerId]) -> Result<(), ModelError> {
        ids.iter().try_for_each(|&id| self.require(id).map(drop))?;
        self.list.retain(|l| !ids.contains(&l.id));
        Ok(())
    }

    /// Copies each listed layer, the copy straight after its original.
    /// Returns the copies' IDs, in list order.
    ///
    /// # Errors
    ///
    /// A layer that isn't there; nothing is copied.
    pub fn duplicate(&mut self, ids: &[LayerId]) -> Result<Vec<LayerId>, ModelError> {
        ids.iter().try_for_each(|&id| self.require(id).map(drop))?;
        let mut copies = Vec::new();
        let mut list = Vec::with_capacity(self.list.len() + ids.len());
        for layer in std::mem::take(&mut self.list) {
            let copy = ids.contains(&layer.id).then(|| layer.clone());
            list.push(layer);
            if let Some(mut copy) = copy {
                copy.id = self.next_id();
                copies.push(copy.id);
                list.push(copy);
            }
        }
        self.list = list;
        Ok(copies)
    }

    /// Renames a layer.
    ///
    /// # Errors
    ///
    /// No such layer.
    pub fn rename(&mut self, id: LayerId, name: &str) -> Result<(), ModelError> {
        let at = self.require(id)?;
        name.clone_into(&mut self.list[at].name);
        Ok(())
    }

    /// Moves the listed layers, keeping their order, so the first of them
    /// sits at `index` among the layers left when they're taken out.
    ///
    /// # Errors
    ///
    /// A layer that isn't there, or an index past the layers left.
    pub fn moved(&mut self, ids: &[LayerId], index: usize) -> Result<(), ModelError> {
        ids.iter().try_for_each(|&id| self.require(id).map(drop))?;
        let (moving, mut rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.list)
            .into_iter()
            .partition(|l| ids.contains(&l.id));
        if index > rest.len() {
            let len = rest.len();
            rest.extend(moving);
            self.list = rest;
            return Err(ModelError::IndexOutOfRange { index, len });
        }
        rest.splice(index..index, moving);
        self.list = rest;
        Ok(())
    }

    /// Sorts the list by `by`, stably, ascending or descending.
    pub fn sort(&mut self, by: LayerSort, descending: bool) {
        let order = |a: &Layer, b: &Layer| match by {
            LayerSort::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            LayerSort::Key => (a.key.low, a.key.high).cmp(&(b.key.low, b.key.high)),
            LayerSort::Velocity => {
                (a.velocity.low, a.velocity.high).cmp(&(b.velocity.low, b.velocity.high))
            }
            LayerSort::Selector => {
                (a.select.low, a.select.high).cmp(&(b.select.low, b.select.high))
            }
        };
        // Comparing the other way round, rather than reversing, keeps ties in order.
        if descending {
            self.list.sort_by(|a, b| order(b, a));
        } else {
            self.list.sort_by(order);
        }
    }

    /// Replaces a layer's sample, keeping its zones and settings and its
    /// markers at the same share of the new length.
    ///
    /// # Errors
    ///
    /// No such layer, or a sample with no frames, rate or channel.
    pub fn replace_media(
        &mut self,
        id: LayerId,
        media: ContentHash,
        length: u64,
        sample_rate: u32,
        channels: u8,
    ) -> Result<(), ModelError> {
        let at = self.require(id)?;
        if length == 0 {
            return Err(ModelError::InvalidValue {
                what: "layer length",
                value: 0.0,
            });
        }
        let mut next = self.list[at].clone();
        let (old, new) = (next.length.max(1) as f64, length as f64);
        let scale = |frame: u64| ((frame as f64 * new / old).round() as u64).min(length);
        next.media = media;
        next.length = length;
        next.sample_rate = sample_rate;
        next.channels = channels;
        next.start = scale(next.start).min(length - 1);
        next.end = scale(next.end).max(next.start + 1).min(length);
        next.loop_start = scale(next.loop_start).clamp(next.start, next.end - 1);
        next.loop_end = scale(next.loop_end).clamp(next.loop_start + 1, next.end);
        next.release_loop = scale(next.release_loop).clamp(next.loop_end, next.end);
        next.crossfade_ms = next.crossfade_ms.min(next.crossfade_limit_ms());
        next.check()?;
        self.list[at] = next;
        Ok(())
    }

    /// Sets `kind`'s zone of each listed layer.
    ///
    /// # Errors
    ///
    /// A layer that isn't there or a zone out of range; nothing changes.
    pub fn set_zones(
        &mut self,
        kind: ZoneKind,
        zones: &[(LayerId, Zone)],
    ) -> Result<(), ModelError> {
        for &(id, zone) in zones {
            self.require(id)?;
            if !zone.is_valid(kind) {
                return Err(ModelError::InvalidValue {
                    what: "zone",
                    value: f64::from(zone.low),
                });
            }
        }
        for &(id, zone) in zones {
            let at = self.require(id)?;
            *self.list[at].zone_mut(kind) = zone;
        }
        Ok(())
    }

    /// Changes the settings of each listed layer, as one: none changes if
    /// any is refused.
    ///
    /// # Errors
    ///
    /// A layer that isn't there or a setting out of range.
    pub fn update(&mut self, ids: &[LayerId], settings: &LayerSettings) -> Result<(), ModelError> {
        let mut changed = self.list.clone();
        for &id in ids {
            let at = self.require(id)?;
            settings.apply(&mut changed[at])?;
        }
        self.list = changed;
        Ok(())
    }

    /// Distributes ranges over the listed layers, or over all of them when
    /// one or none is listed (R12), in list order, without fades: `how`
    /// spreads them equally over the range of `kind`'s editor, or (for key
    /// zones) around their root keys.
    ///
    /// # Errors
    ///
    /// A layer that isn't there.
    pub fn distribute(
        &mut self,
        ids: &[LayerId],
        how: Distribution,
        kind: ZoneKind,
    ) -> Result<(), ModelError> {
        ids.iter().try_for_each(|&id| self.require(id).map(drop))?;
        let chosen: Vec<usize> = (0..self.list.len())
            .filter(|&i| ids.len() <= 1 || ids.contains(&self.list[i].id))
            .collect();
        match how {
            Distribution::Equally => {
                let (low, high) = kind.range();
                for (n, &i) in chosen.iter().enumerate() {
                    *self.list[i].zone_mut(kind) = equal_share(low, high, n, chosen.len());
                }
            }
            Distribution::AroundRootKey => {
                let mut roots: Vec<u8> = chosen.iter().map(|&i| self.list[i].root_key).collect();
                roots.sort_unstable();
                roots.dedup();
                for (r, &root) in roots.iter().enumerate() {
                    let low = if r == 0 {
                        0
                    } else {
                        (roots[r - 1] + root) / 2 + 1
                    };
                    let high = roots.get(r + 1).map_or(127, |&next| (root + next) / 2);
                    let sharing: Vec<usize> = chosen
                        .iter()
                        .copied()
                        .filter(|&i| self.list[i].root_key == root)
                        .collect();
                    for (n, &i) in sharing.iter().enumerate() {
                        self.list[i].key = equal_share(low, high, n, sharing.len());
                    }
                }
            }
        }
        Ok(())
    }
}

/// Layer `n` of `count`'s equal share of the values `low` to `high` (R12):
/// from `low + ⌊m·n/count⌋` to `low + ⌊m·(n+1)/count⌋ − 1` over `m` values,
/// a zone keeping at least its first value when there are more layers than
/// values.
fn equal_share(low: u8, high: u8, n: usize, count: usize) -> Zone {
    let m = usize::from(high - low) + 1;
    let first = usize::from(low) + m * n / count;
    let last = (usize::from(low) + m * (n + 1) / count)
        .saturating_sub(1)
        .max(first);
    Zone::new(
        u8::try_from(first).unwrap_or(high),
        u8::try_from(last).unwrap_or(high),
    )
}

/// The frame nearest to `frame` where `left` is zero or changes sign (R20),
/// the earlier one on a tie; `frame` itself where the sample has none.
#[must_use]
pub fn snap_to_zero_crossing(left: &[f32], frame: u64) -> u64 {
    let crossing = |i: usize| {
        left[i] == 0.0 || (i > 0 && (left[i - 1] < 0.0) != (left[i] < 0.0) && left[i - 1] != 0.0)
    };
    let Ok(at) = usize::try_from(frame) else {
        return frame;
    };
    let at = at.min(left.len().saturating_sub(1));
    if left.is_empty() {
        return frame;
    }
    (0..left.len())
        .find_map(|distance| {
            let before = at.checked_sub(distance).filter(|&i| crossing(i));
            let after = Some(at + distance).filter(|&i| i < left.len() && crossing(i));
            before.or(after)
        })
        .map_or(frame, |i| i as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(root: u8) -> Layer {
        let mut l = Layer::new(LayerId(1), "a", ContentHash([0; 32]), 48_000, 48_000, 1);
        l.root_key = root;
        l
    }

    fn layers(roots: &[u8]) -> Layers {
        let mut l = Layers::default();
        for &root in roots {
            l.insert(None, &[layer(root)]).unwrap();
        }
        l
    }

    // Spec: multisampler R2
    #[test]
    fn layers_save_and_reopen_and_a_set_without_them_has_none() {
        assert!(Layers::default().is_default());
        let mut l = layers(&[60, 64]);
        l.list[1].key = Zone {
            low: 10,
            high: 90,
            fade_low: 4,
            fade_high: 6,
        };
        l.snap = true;
        let json = serde_json::to_string(&l).unwrap();
        assert_eq!(serde_json::from_str::<Layers>(&json).unwrap(), l);
        l.check().unwrap();
    }

    // Spec: multisampler R5
    #[test]
    fn a_fade_runs_from_the_silent_edge_to_full_in_either_law() {
        let zone = Zone {
            low: 10,
            high: 50,
            fade_low: 4,
            fade_high: 8,
        };
        assert_eq!(zone.gain(9, FadeMode::Lin), 0.0);
        assert_eq!(zone.gain(10, FadeMode::Pow), 0.0);
        assert_eq!(zone.gain(12, FadeMode::Lin), 0.5);
        assert!((zone.gain(12, FadeMode::Pow) - core::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert_eq!(zone.gain(14, FadeMode::Lin), 1.0);
        assert_eq!(zone.gain(42, FadeMode::Lin), 1.0);
        assert_eq!(zone.gain(46, FadeMode::Lin), 0.5);
        assert_eq!(zone.gain(50, FadeMode::Lin), 0.0);
        assert_eq!(zone.gain(51, FadeMode::Lin), 0.0);
        assert!(Zone::new(5, 5).is_valid(ZoneKind::Key));
        assert!(!Zone::new(0, 5).is_valid(ZoneKind::Velocity));
        assert!(
            !Zone {
                fade_low: 3,
                fade_high: 3,
                ..Zone::new(1, 5)
            }
            .is_valid(ZoneKind::Key)
        );
    }

    // Spec: multisampler R12
    #[test]
    fn distributing_equally_and_around_root_keys() {
        let mut l = layers(&[60; 4]);
        l.distribute(&[], Distribution::Equally, ZoneKind::Key)
            .unwrap();
        let keys: Vec<_> = l.list.iter().map(|l| (l.key.low, l.key.high)).collect();
        assert_eq!(keys, [(0, 31), (32, 63), (64, 95), (96, 127)]);
        l.distribute(&[], Distribution::Equally, ZoneKind::Velocity)
            .unwrap();
        let velocities: Vec<_> = l
            .list
            .iter()
            .map(|l| (l.velocity.low, l.velocity.high))
            .collect();
        assert_eq!(velocities, [(1, 31), (32, 63), (64, 95), (96, 127)]);

        // Selected layers only, when several are.
        let mut l = layers(&[60; 4]);
        let ids = [l.list[1].id, l.list[3].id];
        l.distribute(&ids, Distribution::Equally, ZoneKind::Select)
            .unwrap();
        assert_eq!((l.list[0].select.low, l.list[0].select.high), (0, 127));
        assert_eq!((l.list[1].select.low, l.list[1].select.high), (0, 63));
        assert_eq!((l.list[3].select.low, l.list[3].select.high), (64, 127));

        // Roots C3, E3, G3 split at ⌊(a+b)/2⌋; a shared root divides its zone.
        let mut l = layers(&[60, 64, 67, 64]);
        l.distribute(&[], Distribution::AroundRootKey, ZoneKind::Key)
            .unwrap();
        let keys: Vec<_> = l.list.iter().map(|l| (l.key.low, l.key.high)).collect();
        assert_eq!(keys, [(0, 62), (63, 63), (66, 127), (64, 65)]);
    }

    // Spec: multisampler R20
    #[test]
    fn snap_puts_markers_on_left_channel_zero_crossings() {
        let left = [0.5, 0.4, -0.1, -0.5, -0.2, 0.3, 0.5, 0.0, 0.2];
        assert_eq!(snap_to_zero_crossing(&left, 0), 2);
        assert_eq!(snap_to_zero_crossing(&left, 3), 2);
        assert_eq!(snap_to_zero_crossing(&left, 4), 5);
        assert_eq!(snap_to_zero_crossing(&left, 6), 5);
        assert_eq!(snap_to_zero_crossing(&left, 7), 7);
        assert_eq!(snap_to_zero_crossing(&[0.5, 0.5], 1), 1);
    }

    // Spec: multisampler R16
    #[test]
    fn markers_left_alone_follow_the_ones_that_move() {
        let mut l = layers(&[60]);
        let id = l.list[0].id;
        // Sample End comes in: the loop that filled the sample follows it.
        let settings = LayerSettings {
            end: Some(30_000),
            ..LayerSettings::default()
        };
        l.update(&[id], &settings).unwrap();
        let layer = &l.list[0];
        assert_eq!(
            (layer.loop_start, layer.loop_end, layer.release_loop),
            (0, 30_000, 30_000)
        );
        // Markers set together must be in order, or nothing changes.
        let out_of_order = LayerSettings {
            loop_start: Some(20_000),
            loop_end: Some(10_000),
            ..LayerSettings::default()
        };
        assert!(l.update(&[id], &out_of_order).is_err());
        assert_eq!(l.list[0].loop_end, 30_000);
        // Sample Start past the loop's start takes it along.
        let settings = LayerSettings {
            start: Some(5_000),
            ..LayerSettings::default()
        };
        l.update(&[id], &settings).unwrap();
        assert_eq!(l.list[0].loop_start, 5_000);
    }

    #[test]
    fn list_edits_keep_ids_and_order() {
        let mut l = layers(&[60, 62, 64]);
        let [a, b, c] = [l.list[0].id, l.list[1].id, l.list[2].id];
        let copies = l.duplicate(&[b]).unwrap();
        assert_eq!(
            l.list.iter().map(|l| l.id).collect::<Vec<_>>(),
            [a, b, copies[0], c]
        );
        l.moved(&[c, a], 1).unwrap();
        assert_eq!(
            l.list.iter().map(|l| l.id).collect::<Vec<_>>(),
            [b, a, c, copies[0]]
        );
        l.remove(&[a]).unwrap();
        let again = l.insert(None, &[layer(70)]).unwrap();
        assert_ne!(again[0], a, "an ID is never reused");
        assert!(l.remove(&[a]).is_err());
        l.check().unwrap();
    }

    #[test]
    fn markers_past_the_end_are_refused_not_a_panic() {
        let mut l = layers(&[60]);
        let id = l.list[0].id;
        let at_end = LayerSettings {
            start: Some(48_000),
            ..LayerSettings::default()
        };
        assert!(l.update(&[id], &at_end).is_err());
        let near_end = LayerSettings {
            start: Some(47_990),
            ..LayerSettings::default()
        };
        l.update(&[id], &near_end).unwrap();
        l.replace_media(id, ContentHash([1; 32]), 100, 48_000, 1)
            .unwrap();
        l.check().unwrap();
        assert!(
            l.replace_media(id, ContentHash([1; 32]), 0, 48_000, 1)
                .is_err()
        );
    }

    #[test]
    fn a_descending_sort_keeps_ties_in_order() {
        let mut l = layers(&[60, 60, 62]);
        l.list[2].key = Zone::new(62, 127);
        let [a, b, c] = [l.list[0].id, l.list[1].id, l.list[2].id];
        l.sort(LayerSort::Key, true);
        assert_eq!(l.list.iter().map(|l| l.id).collect::<Vec<_>>(), [c, a, b]);
    }
}

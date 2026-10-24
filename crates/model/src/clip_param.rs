//! An audio clip's sound settings as parameters (WP-4.8, the warp modes spec
//! R28 to R33): each has a descriptor, can be automated on the track and
//! drawn as a clip envelope, and is read by the warp modes frame by frame.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::clip::AudioData;
use crate::ids::ParamId;
use crate::param::{ParamDescriptor, ParamUnit, db_to_gain};
use crate::warp_settings::{
    DETUNE_MAX, DETUNE_MIN, Preserve, TRANSPOSE_MAX, TRANSPOSE_MIN, TransientLoopMode,
};

/// The top of the clip's Gain, in dB (R28, Beatlayer's own).
const GAIN_MAX_DB: f64 = 24.0;

/// One of an audio clip's sound settings (R28), in the order the Envelopes
/// tab lists them (R29): the three the manual names first, then Beatlayer's
/// additions.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "A sound setting of an audio clip: `gain` (linear, 1 is 0 dB), `transpose` \
                   (semitones), `sample_offset` (Beats only, in sixteenth notes, an envelope \
                   only), `detune` (cents), `high_quality` (the HiQ switch), `preserve`, \
                   `transient_loop_mode` and `transient_envelope` (Beats), `tones_grain_size`, \
                   `texture_grain_size` and `texture_flux`, `formants` and \
                   `complex_pro_envelope` (Complex Pro)."
)]
pub enum ClipParam {
    /// The clip's gain: a linear gain from silence (−∞ dB) to +24 dB.
    Gain,
    /// Transpose, in semitones, −48 to 48.
    Transpose,
    /// Beats' Sample Offset (R30), in sixteenth notes, −8 to 8. It has no
    /// setting of its own: only an envelope or automation moves it.
    SampleOffset,
    /// Detune, in cents, −50 to 50.
    Detune,
    /// The HiQ switch.
    HighQuality,
    /// Beats' Preserve, by its place in the list.
    Preserve,
    /// Beats' Transient Loop Mode, by its place in the list.
    TransientLoopMode,
    /// Beats' Transient Envelope, 0 to 100.
    TransientEnvelope,
    /// Tones' Grain Size, in milliseconds.
    TonesGrainSize,
    /// Texture's Grain Size, in milliseconds.
    TextureGrainSize,
    /// Texture's Flux, in per cent.
    TextureFlux,
    /// Complex Pro's Formants, in per cent.
    Formants,
    /// Complex Pro's Envelope, 8 to 512.
    ComplexProEnvelope,
}

impl ClipParam {
    /// Every setting, in the order the Envelopes tab lists them.
    pub const ALL: [Self; 13] = [
        Self::Gain,
        Self::Transpose,
        Self::SampleOffset,
        Self::Detune,
        Self::HighQuality,
        Self::Preserve,
        Self::TransientLoopMode,
        Self::TransientEnvelope,
        Self::TonesGrainSize,
        Self::TextureGrainSize,
        Self::TextureFlux,
        Self::Formants,
        Self::ComplexProEnvelope,
    ];

    /// The setting's lowest and highest value, as its descriptor gives them,
    /// without building the descriptor: what the engine reads on the audio
    /// thread.
    #[must_use]
    pub fn limits(self) -> (f64, f64) {
        match self {
            Self::Gain => (0.0, db_to_gain(GAIN_MAX_DB)),
            Self::Transpose => (f64::from(TRANSPOSE_MIN), f64::from(TRANSPOSE_MAX)),
            Self::SampleOffset => (-8.0, 8.0),
            Self::Detune => (DETUNE_MIN, DETUNE_MAX),
            Self::HighQuality => (0.0, 1.0),
            Self::Preserve => (0.0, 6.0),
            Self::TransientLoopMode => (0.0, 2.0),
            Self::TransientEnvelope => (0.0, 100.0),
            Self::TonesGrainSize => (10.0, 100.0),
            Self::TextureGrainSize => (10.0, 500.0),
            Self::TextureFlux | Self::Formants => (0.0, 100.0),
            Self::ComplexProEnvelope => (8.0, 512.0),
        }
    }

    /// Whether the setting takes whole steps: a switch, a choice or a whole
    /// number.
    #[must_use]
    pub fn is_stepped(self) -> bool {
        matches!(
            self,
            Self::Transpose
                | Self::HighQuality
                | Self::Preserve
                | Self::TransientLoopMode
                | Self::TransientEnvelope
                | Self::ComplexProEnvelope
        )
    }

    /// `value` held to the setting's range, on a step for a stepped one.
    fn held(self, value: f64) -> f64 {
        // A clip's own Gain may be above the automation's top (+24 dB); only
        // silence is its floor.
        if self == Self::Gain {
            return value.max(0.0);
        }
        let (low, high) = self.limits();
        let value = value.clamp(low, high);
        if self.is_stepped() {
            value.round()
        } else {
            value
        }
    }

    /// The setting's descriptor (R28), numbered by its place in
    /// [`ALL`](Self::ALL): its range, default and unit.
    #[must_use]
    pub fn descriptor(self) -> ParamDescriptor {
        let id = ParamId(self as u32);
        let preserve = ["Transients", "1 Bar", "1/2", "1/4", "1/8", "1/16", "1/32"];
        match self {
            Self::Gain => ParamDescriptor::gain(id, "Gain", GAIN_MAX_DB, 0.0),
            Self::Transpose => ParamDescriptor::integer(
                id,
                "Transpose",
                (i64::from(TRANSPOSE_MIN), i64::from(TRANSPOSE_MAX)),
                0,
                ParamUnit::Semitones,
            ),
            Self::SampleOffset => {
                ParamDescriptor::continuous(id, "Sample Offset", (-8.0, 8.0), 0.0, ParamUnit::None)
            }
            Self::Detune => ParamDescriptor::continuous(
                id,
                "Detune",
                (DETUNE_MIN, DETUNE_MAX),
                0.0,
                ParamUnit::Cents,
            ),
            Self::HighQuality => ParamDescriptor::toggle(id, "HiQ", true),
            Self::Preserve => ParamDescriptor::choice(id, "Preserve", &preserve, 0),
            Self::TransientLoopMode => ParamDescriptor::choice(
                id,
                "Transient Loop Mode",
                &["Loop Off", "Loop Forward", "Loop Back-and-Forth"],
                2,
            ),
            Self::TransientEnvelope => ParamDescriptor::integer(
                id,
                "Transient Envelope",
                (0, 100),
                100,
                ParamUnit::Percent,
            ),
            Self::TonesGrainSize => ParamDescriptor::continuous(
                id,
                "Tones Grain Size",
                (10.0, 100.0),
                30.0,
                ParamUnit::Milliseconds,
            ),
            Self::TextureGrainSize => ParamDescriptor::continuous(
                id,
                "Texture Grain Size",
                (10.0, 500.0),
                65.0,
                ParamUnit::Milliseconds,
            ),
            Self::TextureFlux => {
                ParamDescriptor::continuous(id, "Flux", (0.0, 100.0), 25.0, ParamUnit::Percent)
            }
            Self::Formants => {
                ParamDescriptor::continuous(id, "Formants", (0.0, 100.0), 100.0, ParamUnit::Percent)
            }
            Self::ComplexProEnvelope => {
                ParamDescriptor::integer(id, "Envelope", (8, 512), 128, ParamUnit::None)
            }
        }
    }

    /// The range of values a clip envelope for the setting holds (R29): a
    /// share from 0 to 100 % for Gain; for Transpose, Detune and Sample
    /// Offset the amount they add, in their own units, over their own range;
    /// and for each of the rest an amount of up to the setting's whole travel
    /// either way, so an envelope can lower a setting as well as raise it
    /// (Beatlayer's own).
    #[must_use]
    pub fn envelope_range(self) -> (f64, f64) {
        match self {
            Self::Gain => (0.0, 100.0),
            Self::Transpose | Self::Detune | Self::SampleOffset => self.limits(),
            _ => {
                let (low, high) = self.limits();
                (low - high, high - low)
            }
        }
    }

    /// The setting's value on `audio`, in the descriptor's units. Sample
    /// Offset has no setting, so it is 0.
    #[must_use]
    pub fn base(self, audio: &AudioData) -> f64 {
        let settings = &audio.warp_settings;
        match self {
            Self::Gain => audio.gain,
            Self::Transpose => f64::from(audio.transpose),
            Self::SampleOffset => 0.0,
            Self::Detune => audio.detune,
            Self::HighQuality => f64::from(u8::from(audio.high_quality)),
            Self::Preserve => f64::from(settings.beats.preserve as u8),
            Self::TransientLoopMode => f64::from(settings.beats.transient_loop_mode as u8),
            Self::TransientEnvelope => f64::from(settings.beats.transient_envelope),
            Self::TonesGrainSize => settings.tones.grain_size,
            Self::TextureGrainSize => settings.texture.grain_size,
            Self::TextureFlux => settings.texture.flux,
            Self::Formants => settings.complex_pro.formants,
            Self::ComplexProEnvelope => f64::from(settings.complex_pro.envelope),
        }
    }

    /// The value the setting plays when its `base` (the setting's own value,
    /// or the track automation's) is moved by its clip envelope's value
    /// `envelope`, if the clip has one at that point (R29): Gain's is a
    /// share of the base, Sample Offset is the envelope's value, and the rest
    /// add it, clamped to the setting's range, a choice or a switch taking
    /// the step nearest the sum. Transpose, whose sum R25 holds to its limits
    /// and leaves fractional, is not rounded.
    #[must_use]
    pub fn moved(self, base: f64, envelope: Option<f64>) -> f64 {
        let base = self.held(base);
        let Some(envelope) = envelope else {
            return base;
        };
        match self {
            Self::Gain => base * envelope.clamp(0.0, 100.0) / 100.0,
            Self::SampleOffset => self.held(envelope),
            Self::Transpose => {
                (base + envelope).clamp(f64::from(TRANSPOSE_MIN), f64::from(TRANSPOSE_MAX))
            }
            _ => self.held(base + envelope),
        }
    }
}

// The enums run in their chooser's order, so `as u8` is a choice's place.
impl Preserve {
    /// The choice numbered `index` in Beats' Preserve list, the nearest one
    /// for a number out of range.
    #[must_use]
    pub fn from_index(index: f64) -> Self {
        match index.round().clamp(0.0, 6.0) as u8 {
            0 => Self::Transients,
            1 => Self::Bar,
            2 => Self::Half,
            3 => Self::Quarter,
            4 => Self::Eighth,
            5 => Self::Sixteenth,
            _ => Self::ThirtySecond,
        }
    }
}

impl TransientLoopMode {
    /// The choice numbered `index` in Transient Loop Mode's list, the nearest
    /// one for a number out of range.
    #[must_use]
    pub fn from_index(index: f64) -> Self {
        match index.round().clamp(0.0, 2.0) as u8 {
            0 => Self::LoopOff,
            1 => Self::LoopForward,
            _ => Self::LoopBackAndForth,
        }
    }
}

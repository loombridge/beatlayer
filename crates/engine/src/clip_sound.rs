//! What moves an audio clip's sound settings while it plays (WP-4.8, the warp
//! modes spec R29 to R33): the track's automation of each setting, and the
//! clip's own envelope for it.
//!
//! The compiler places both in song frames, off the audio thread, and the clip
//! list carries them. A setting's value at a frame is the same whichever block
//! the frame lands in: the continuous modes read their settings once for each
//! [`STEP`] frames, on a grid of song frames, so a render never depends on how
//! the song was split into blocks (R33). Reading allocates nothing.

use bl_core::time::{SampleRate, TempoMap};
use bl_model::{AudioData, AutomationTarget, Clip, ClipParam, EnvelopeKind, Track};

use crate::automation::{PlacedClipEnvelope, PlacedEnvelope};
use crate::warped::Playing;

/// How many song frames a mode's settings are held for: the grid they are
/// read on, a little over a millisecond at 48 kHz.
pub(crate) const STEP: i64 = 64;

/// How many settings there are.
const SETTINGS: usize = ClipParam::ALL.len();

/// What moves one setting.
#[derive(Default)]
struct Drive {
    /// The track's automation of the setting, where it is on.
    automation: Option<PlacedEnvelope>,
    /// The clip's envelope for it, while the clip plays.
    envelope: Option<PlacedClipEnvelope>,
}

/// An Arrangement clip's settings and what moves them. A clip that nothing
/// moves has none, and plays from its [`Playing`] as it always did.
pub(crate) struct ClipSound {
    /// Each setting's own value, by place in [`ClipParam::ALL`].
    bases: [f64; SETTINGS],
    drives: [Drive; SETTINGS],
}

impl ClipSound {
    /// What moves `audio`'s settings, which sits on `track` as `clip`: `None`
    /// where nothing does. A track's automation that is turned off (a hand
    /// move overrode it, R31) moves nothing.
    pub(crate) fn of(
        track: &Track,
        clip: &Clip,
        audio: &AudioData,
        tempo: &TempoMap,
        rate: SampleRate,
    ) -> Option<Self> {
        let mut sound = Self {
            bases: ClipParam::ALL.map(|param| param.base(audio)),
            drives: Default::default(),
        };
        let mut driven = false;
        for (target, automation) in track.automation() {
            if let AutomationTarget::Clip { param } = *target
                && automation.enabled
                && !automation.envelope.is_empty()
            {
                sound.drives[param as usize].automation =
                    Some(PlacedEnvelope::placed(&automation.envelope, tempo, rate, 0));
                driven = true;
            }
        }
        for envelope in clip
            .envelopes()
            .filter(|envelope| envelope.kind == EnvelopeKind::Modulation)
        {
            if let AutomationTarget::Clip { param } = envelope.target
                && let Some(placed) = PlacedClipEnvelope::placed(clip, envelope, tempo, rate, 0)
            {
                sound.drives[param as usize].envelope = Some(placed);
                driven = true;
            }
        }
        driven.then_some(sound)
    }

    /// What the setting `param` plays at song `frame` (R29, R31): its own
    /// value, or the track automation's where that drives it, moved by the
    /// clip's envelope where the clip plays one there.
    pub(crate) fn value(&self, param: ClipParam, frame: i64) -> f64 {
        let drive = &self.drives[param as usize];
        let base = drive
            .automation
            .as_ref()
            .map_or(self.bases[param as usize], |automation| {
                automation.value_at(frame)
            });
        let envelope = drive
            .envelope
            .as_ref()
            .and_then(|envelope| envelope.value_at(frame));
        param.moved(base, envelope)
    }

    /// The clip's pitch ratio at `frame` (R25): 2 raised to its Transpose and
    /// its Detune in semitones, each as moved there, over 12.
    pub(crate) fn pitch_at(&self, frame: i64) -> f64 {
        let change =
            self.value(ClipParam::Transpose, frame) + self.value(ClipParam::Detune, frame) / 100.0;
        2f64.powf(change / 12.0)
    }

    /// The clip's Gain at `frame`: its own, or the automation's, taken down
    /// by the envelope's share (R29).
    pub(crate) fn gain_at(&self, frame: i64) -> f64 {
        self.value(ClipParam::Gain, frame)
    }

    /// `base` with every setting a mode reads set to what it plays on the
    /// step of the grid that `frame` falls in (R33). A step that begins
    /// before the clip does is read where the clip begins.
    pub(crate) fn playing(&self, base: Playing, frame: i64, start: i64) -> Playing {
        let at = (frame - frame.rem_euclid(STEP)).max(start);
        let value = |param| self.value(param, at);
        let mut playing = base;
        // The envelopes act only while Warp is on (R27).
        if base.warp {
            playing.pitch = self.pitch_at(at);
        }
        playing.high_quality = value(ClipParam::HighQuality) >= 0.5;
        playing.transient_envelope = value(ClipParam::TransientEnvelope);
        playing.tones.grain_size = value(ClipParam::TonesGrainSize);
        playing.texture.grain_size = value(ClipParam::TextureGrainSize);
        playing.texture.flux = value(ClipParam::TextureFlux);
        playing.complex_pro.formants = value(ClipParam::Formants);
        playing.complex_pro.envelope = value(ClipParam::ComplexProEnvelope) as u32;
        playing
    }

    /// How many frames from `frame` lie on the same step of the grid.
    pub(crate) fn step_left(frame: i64) -> usize {
        (STEP - frame.rem_euclid(STEP)) as usize
    }
}

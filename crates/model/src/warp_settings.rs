//! A warped clip's mode and the controls of each mode (WP-4.2, the warp modes
//! spec, R1 to R3), and the clip's RAM and HiQ switches.
//!
//! A clip keeps every mode's controls at once, so changing its mode and
//! changing it back restores them. Each field is saved only where it differs
//! from a new clip's, so a set saved before these settings existed loads with
//! every clip at the table's defaults.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ModelError;

/// How a warped clip's audio is fitted to the speed its warp map sets: the
/// six warp modes, in the order the Audio panel offers them (R1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WarpMode {
    /// Segments cut at transients or on a grid, for rhythmic material. The
    /// default of a new clip.
    #[default]
    Beats,
    /// Grains that keep the pitch of pitched material.
    Tones,
    /// Overlapping grains with some randomness, for material without a clear
    /// pitch.
    Texture,
    /// Plays the file faster or slower, so pitch follows speed.
    RePitch,
    /// Frequency-domain stretching, for whole mixes.
    Complex,
    /// Complex, with finer settings and Formants.
    ComplexPro,
}

/// What Beats keeps (R2): its transients, or the lines of a grid in the
/// clip's own beats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Preserve {
    /// A segment at each transient; the default.
    #[default]
    Transients,
    /// A segment at each bar.
    #[serde(rename = "1_bar")]
    Bar,
    /// At each half note.
    Half,
    /// At each quarter note.
    Quarter,
    /// At each eighth note.
    Eighth,
    /// At each sixteenth note.
    Sixteenth,
    /// At each thirty-second note.
    ThirtySecond,
}

/// How Beats fills the time a segment's own audio doesn't (R2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransientLoopMode {
    /// Silence until the next segment.
    LoopOff,
    /// Back to the segment's middle, forwards again.
    LoopForward,
    /// Backwards to the middle, then forwards; the default.
    #[default]
    LoopBackAndForth,
}

/// Beats' controls.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BeatsSettings {
    /// What it preserves: transients unless given.
    #[serde(default, skip_serializing_if = "is_default")]
    pub preserve: Preserve,
    /// What it does when a segment runs out: Loop Back-and-Forth unless given.
    #[serde(default, skip_serializing_if = "is_default")]
    pub transient_loop_mode: TransientLoopMode,
    /// How far each segment fades, 0 to 100: 100 (no fade) unless given.
    #[serde(default = "full", skip_serializing_if = "is_full")]
    #[schemars(range(min = 0, max = 100))]
    pub transient_envelope: u32,
}

/// Tones' controls.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TonesSettings {
    /// The grains' average length in milliseconds, 10 to 100: 30 unless given.
    #[serde(default = "tones_grain", skip_serializing_if = "is_tones_grain")]
    #[schemars(range(min = 10.0, max = 100.0))]
    pub grain_size: f64,
}

/// Texture's controls.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TextureSettings {
    /// The grains' length in milliseconds, 10 to 500: 65 unless given.
    #[serde(default = "texture_grain", skip_serializing_if = "is_texture_grain")]
    #[schemars(range(min = 10.0, max = 500.0))]
    pub grain_size: f64,
    /// The randomness, 0 to 100 percent (Live's Fluctuation): 25 unless given.
    #[serde(default = "texture_flux", skip_serializing_if = "is_texture_flux")]
    #[schemars(range(min = 0.0, max = 100.0))]
    pub flux: f64,
}

/// Complex Pro's controls.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ComplexProSettings {
    /// How far the formants stay put when the pitch moves, 0 to 100 percent:
    /// 100 unless given.
    #[serde(default = "formants", skip_serializing_if = "is_formants")]
    #[schemars(range(min = 0.0, max = 100.0))]
    pub formants: f64,
    /// The detail of the spectral envelope, 8 to 512: 128 unless given.
    #[serde(default = "envelope", skip_serializing_if = "is_envelope")]
    #[schemars(range(min = 8, max = 512))]
    pub envelope: u32,
}

/// The lowest a clip's Transpose may be, in whole semitones (R2): the limit
/// the manual gives for Transpose with its envelope.
pub const TRANSPOSE_MIN: i32 = -48;

/// The highest a clip's Transpose may be, in whole semitones (R2).
pub const TRANSPOSE_MAX: i32 = 48;

/// The lowest a clip's Detune may be, in cents (R2, Beatlayer's own range).
pub const DETUNE_MIN: f64 = -50.0;

/// The highest a clip's Detune may be, in cents (R2, Beatlayer's own range).
pub const DETUNE_MAX: f64 = 50.0;

/// Every mode's controls (R2). Re-Pitch and Complex have none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WarpSettings {
    /// Beats' controls.
    #[serde(default, skip_serializing_if = "is_default")]
    pub beats: BeatsSettings,
    /// Tones' controls.
    #[serde(default, skip_serializing_if = "is_default")]
    pub tones: TonesSettings,
    /// Texture's controls.
    #[serde(default, skip_serializing_if = "is_default")]
    pub texture: TextureSettings,
    /// Complex Pro's controls.
    #[serde(default, skip_serializing_if = "is_default")]
    pub complex_pro: ComplexProSettings,
}

/// Changes to Beats' controls: only the ones given change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BeatsUpdate {
    /// What it preserves.
    #[serde(default)]
    pub preserve: Option<Preserve>,
    /// What it does when a segment runs out.
    #[serde(default)]
    pub transient_loop_mode: Option<TransientLoopMode>,
    /// How far each segment fades, 0 to 100.
    #[serde(default)]
    #[schemars(range(min = 0, max = 100))]
    pub transient_envelope: Option<u32>,
}

/// Changes to Tones' controls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TonesUpdate {
    /// The grains' average length in milliseconds, 10 to 100.
    #[serde(default)]
    #[schemars(range(min = 10.0, max = 100.0))]
    pub grain_size: Option<f64>,
}

/// Changes to Texture's controls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TextureUpdate {
    /// The grains' length in milliseconds, 10 to 500.
    #[serde(default)]
    #[schemars(range(min = 10.0, max = 500.0))]
    pub grain_size: Option<f64>,
    /// The randomness, 0 to 100 percent.
    #[serde(default)]
    #[schemars(range(min = 0.0, max = 100.0))]
    pub flux: Option<f64>,
}

/// Changes to Complex Pro's controls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ComplexProUpdate {
    /// How far the formants stay put, 0 to 100 percent.
    #[serde(default)]
    #[schemars(range(min = 0.0, max = 100.0))]
    pub formants: Option<f64>,
    /// The detail of the spectral envelope, 8 to 512.
    #[serde(default)]
    #[schemars(range(min = 8, max = 512))]
    pub envelope: Option<u32>,
}

impl Default for BeatsSettings {
    fn default() -> Self {
        Self {
            preserve: Preserve::default(),
            transient_loop_mode: TransientLoopMode::default(),
            transient_envelope: full(),
        }
    }
}

impl Default for TonesSettings {
    fn default() -> Self {
        Self {
            grain_size: tones_grain(),
        }
    }
}

impl Default for TextureSettings {
    fn default() -> Self {
        Self {
            grain_size: texture_grain(),
            flux: texture_flux(),
        }
    }
}

impl Default for ComplexProSettings {
    fn default() -> Self {
        Self {
            formants: formants(),
            envelope: envelope(),
        }
    }
}

impl WarpSettings {
    /// These settings with the given controls changed.
    ///
    /// # Errors
    ///
    /// If a value is outside its range (R2), or nothing is given.
    pub fn updated(
        mut self,
        beats: Option<BeatsUpdate>,
        tones: Option<TonesUpdate>,
        texture: Option<TextureUpdate>,
        complex_pro: Option<ComplexProUpdate>,
    ) -> Result<Self, ModelError> {
        let given =
            beats.is_some() || tones.is_some() || texture.is_some() || complex_pro.is_some();
        if !given {
            return Err(ModelError::InvalidInput(
                "name at least one mode's controls: `beats`, `tones`, `texture` or `complex_pro`",
            ));
        }
        if let Some(update) = beats {
            let b = &mut self.beats;
            b.preserve = update.preserve.unwrap_or(b.preserve);
            b.transient_loop_mode = update.transient_loop_mode.unwrap_or(b.transient_loop_mode);
            b.transient_envelope = update.transient_envelope.unwrap_or(b.transient_envelope);
        }
        if let Some(update) = tones {
            self.tones.grain_size = update.grain_size.unwrap_or(self.tones.grain_size);
        }
        if let Some(update) = texture {
            self.texture.grain_size = update.grain_size.unwrap_or(self.texture.grain_size);
            self.texture.flux = update.flux.unwrap_or(self.texture.flux);
        }
        if let Some(update) = complex_pro {
            self.complex_pro.formants = update.formants.unwrap_or(self.complex_pro.formants);
            self.complex_pro.envelope = update.envelope.unwrap_or(self.complex_pro.envelope);
        }
        self.check()?;
        Ok(self)
    }

    /// Checks every control against R2's range.
    ///
    /// # Errors
    ///
    /// Names the first control out of range.
    pub fn check(&self) -> Result<(), ModelError> {
        let within = |what, value: f64, low: f64, high: f64| {
            if (low..=high).contains(&value) {
                Ok(())
            } else {
                Err(ModelError::InvalidValue { what, value })
            }
        };
        within(
            "Transient Envelope",
            f64::from(self.beats.transient_envelope),
            0.0,
            100.0,
        )?;
        within("Tones Grain Size", self.tones.grain_size, 10.0, 100.0)?;
        within("Texture Grain Size", self.texture.grain_size, 10.0, 500.0)?;
        within("Texture Flux", self.texture.flux, 0.0, 100.0)?;
        within("Formants", self.complex_pro.formants, 0.0, 100.0)?;
        within(
            "Complex Pro Envelope",
            f64::from(self.complex_pro.envelope),
            8.0,
            512.0,
        )
    }
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

fn full() -> u32 {
    100
}
fn is_full(value: &u32) -> bool {
    *value == 100
}
fn tones_grain() -> f64 {
    30.0
}
fn is_tones_grain(value: &f64) -> bool {
    *value == 30.0
}
fn texture_grain() -> f64 {
    65.0
}
fn is_texture_grain(value: &f64) -> bool {
    *value == 65.0
}
fn texture_flux() -> f64 {
    25.0
}
fn is_texture_flux(value: &f64) -> bool {
    *value == 25.0
}
fn formants() -> f64 {
    100.0
}
fn is_formants(value: &f64) -> bool {
    *value == 100.0
}
fn envelope() -> u32 {
    128
}
fn is_envelope(value: &u32) -> bool {
    *value == 128
}

#[cfg(test)]
mod tests {
    use bl_core::time::Ticks;
    use serde_json::json;

    use super::*;
    use crate::{
        AudioData, Clip, ClipData, ClipId, Command, ContentHash, Editor, ModelError, TrackKind,
    };

    /// An editor with one audio clip.
    fn editor_with_clip() -> (Editor, ClipId) {
        let mut editor = Editor::new();
        let track = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Audio,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        let id = editor.new_id();
        let data = ClipData::Audio(AudioData::new(ContentHash([1; 32])));
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(id, Ticks::ZERO, Ticks::WHOLE * 2, data),
            })
            .unwrap();
        (editor, id)
    }

    fn audio(editor: &Editor, clip: ClipId) -> AudioData {
        match &editor.document().clip(clip).unwrap().data {
            ClipData::Audio(audio) => audio.clone(),
            ClipData::Midi(_) => unreachable!("an audio clip"),
        }
    }

    // Spec: warp-modes R2
    #[test]
    fn new_clips_have_the_tables_defaults() {
        let audio = AudioData::new(ContentHash([1; 32]));
        assert_eq!(audio.warp_mode, WarpMode::Beats);
        let settings = audio.warp_settings;
        assert_eq!(
            (
                settings.beats.preserve,
                settings.beats.transient_loop_mode,
                settings.beats.transient_envelope
            ),
            (
                Preserve::Transients,
                TransientLoopMode::LoopBackAndForth,
                100
            )
        );
        assert_eq!(settings.tones.grain_size, 30.0);
        assert_eq!(
            (settings.texture.grain_size, settings.texture.flux),
            (65.0, 25.0)
        );
        assert_eq!(
            (settings.complex_pro.formants, settings.complex_pro.envelope),
            (100.0, 128)
        );
        assert!(!audio.ram && audio.high_quality);
    }

    // Spec: warp-modes R2, R3
    #[test]
    fn each_mode_keeps_its_own_settings() {
        let (mut editor, clip) = editor_with_clip();
        editor
            .apply(Command::SetClipWarpSettings {
                clip,
                beats: Some(BeatsUpdate {
                    preserve: Some(Preserve::Eighth),
                    transient_loop_mode: Some(TransientLoopMode::LoopBackAndForth),
                    transient_envelope: Some(40),
                }),
                tones: Some(TonesUpdate {
                    grain_size: Some(55.0),
                }),
                texture: Some(TextureUpdate {
                    grain_size: Some(200.0),
                    flux: Some(0.0),
                }),
                complex_pro: Some(ComplexProUpdate {
                    formants: Some(30.0),
                    envelope: Some(300),
                }),
            })
            .unwrap();
        let set = audio(&editor, clip).warp_settings;
        // Changing the mode and changing it back restores every control.
        for mode in [WarpMode::RePitch, WarpMode::Tones, WarpMode::Beats] {
            editor
                .apply(Command::SetClipWarpMode { clip, mode })
                .unwrap();
            assert_eq!(audio(&editor, clip).warp_mode, mode);
            assert_eq!(audio(&editor, clip).warp_settings, set);
        }
        // One control changes alone.
        editor
            .apply(Command::SetClipWarpSettings {
                clip,
                beats: None,
                tones: None,
                texture: Some(TextureUpdate {
                    grain_size: None,
                    flux: Some(60.0),
                }),
                complex_pro: None,
            })
            .unwrap();
        let changed = audio(&editor, clip).warp_settings;
        assert_eq!(
            (changed.texture.flux, changed.texture.grain_size),
            (60.0, 200.0)
        );
        assert_eq!(changed.beats, set.beats);
    }

    // Spec: warp-modes R3
    #[test]
    fn a_setting_out_of_range_or_none_at_all_is_refused_and_changes_nothing() {
        let (mut editor, clip) = editor_with_clip();
        let before = audio(&editor, clip);
        let update = |beats, tones, texture, complex_pro| Command::SetClipWarpSettings {
            clip,
            beats,
            tones,
            texture,
            complex_pro,
        };
        for command in [
            update(None, None, None, None),
            update(
                Some(BeatsUpdate {
                    transient_envelope: Some(101),
                    ..BeatsUpdate::default()
                }),
                None,
                None,
                None,
            ),
            update(
                None,
                Some(TonesUpdate {
                    grain_size: Some(9.0),
                }),
                None,
                None,
            ),
            update(
                None,
                Some(TonesUpdate {
                    grain_size: Some(f64::NAN),
                }),
                None,
                None,
            ),
            update(
                None,
                None,
                Some(TextureUpdate {
                    grain_size: Some(501.0),
                    flux: None,
                }),
                None,
            ),
            update(
                None,
                None,
                Some(TextureUpdate {
                    grain_size: None,
                    flux: Some(-1.0),
                }),
                None,
            ),
            update(
                None,
                None,
                None,
                Some(ComplexProUpdate {
                    formants: Some(100.5),
                    envelope: None,
                }),
            ),
            update(
                None,
                None,
                None,
                Some(ComplexProUpdate {
                    formants: None,
                    envelope: Some(7),
                }),
            ),
        ] {
            assert!(editor.apply(command.clone()).is_err(), "{command:?}");
            assert_eq!(audio(&editor, clip), before);
        }
        // A MIDI clip has none of them.
        let track = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Midi,
                name: String::new(),
                parent: None,
                index: 1,
            })
            .unwrap();
        let midi = editor.new_id();
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(
                    midi,
                    Ticks::ZERO,
                    Ticks::WHOLE,
                    ClipData::Midi(crate::MidiData::default()),
                ),
            })
            .unwrap();
        for command in [
            Command::SetClipWarpMode {
                clip: midi,
                mode: WarpMode::Tones,
            },
            Command::SetClipRamMode {
                clip: midi,
                ram: true,
            },
            Command::SetClipHighQuality {
                clip: midi,
                high_quality: false,
            },
            Command::SetClipWarpSettings {
                clip: midi,
                beats: Some(BeatsUpdate::default()),
                tones: None,
                texture: None,
                complex_pro: None,
            },
        ] {
            assert!(matches!(
                editor.apply(command).unwrap_err(),
                crate::EditError::Command(ModelError::NotAudioClip(_))
            ));
        }
    }

    // Spec: warp-modes R3. Each is one undo step, and a setting is saved only
    // where it differs from a new clip's.
    #[test]
    fn warp_settings_are_one_step_each_and_saved_only_where_they_differ() {
        let (mut editor, clip) = editor_with_clip();
        let saved = |editor: &Editor| serde_json::to_value(audio(editor, clip)).unwrap();
        for key in ["warp_mode", "warp_settings", "ram", "high_quality"] {
            assert!(saved(&editor).get(key).is_none(), "{key} in a new clip");
        }
        editor
            .apply(Command::SetClipWarpMode {
                clip,
                mode: WarpMode::ComplexPro,
            })
            .unwrap();
        editor
            .apply(Command::SetClipWarpSettings {
                clip,
                beats: None,
                tones: None,
                texture: None,
                complex_pro: Some(ComplexProUpdate {
                    formants: Some(50.0),
                    envelope: None,
                }),
            })
            .unwrap();
        editor
            .apply(Command::SetClipRamMode { clip, ram: true })
            .unwrap();
        editor
            .apply(Command::SetClipHighQuality {
                clip,
                high_quality: false,
            })
            .unwrap();
        let value = saved(&editor);
        assert_eq!(value["warp_mode"], json!("complex_pro"));
        assert_eq!(
            value["warp_settings"],
            json!({ "complex_pro": { "formants": 50.0 } })
        );
        assert_eq!(
            (value["ram"].clone(), value["high_quality"].clone()),
            (json!(true), json!(false))
        );
        let back: AudioData = serde_json::from_value(value).unwrap();
        assert_eq!(back, audio(&editor, clip));
        // Four edits, four steps.
        for _ in 0..4 {
            editor.undo().unwrap();
        }
        assert_eq!(audio(&editor, clip), AudioData::new(ContentHash([1; 32])));
    }

    // Spec: warping R14
    #[test]
    fn an_unwarped_clip_cannot_loop_and_unwarping_stops_the_loop() {
        let (mut editor, clip) = editor_with_clip();
        let refused = editor
            .apply(Command::SetClipLoopEnabled {
                clip,
                enabled: true,
            })
            .unwrap_err();
        assert!(matches!(
            refused,
            crate::EditError::Command(ModelError::InvalidInput(_))
        ));
        assert!(!audio(&editor, clip).looping);
        // Warped, it loops; unwarped again, in one step, it doesn't.
        editor
            .apply(Command::SetClipWarp { clip, warp: true })
            .unwrap();
        editor
            .apply(Command::SetClipLoopEnabled {
                clip,
                enabled: true,
            })
            .unwrap();
        assert!(audio(&editor, clip).looping);
        editor
            .apply(Command::SetClipWarp { clip, warp: false })
            .unwrap();
        assert!(!audio(&editor, clip).looping);
        editor.undo().unwrap();
        assert!(audio(&editor, clip).looping && audio(&editor, clip).warp);
        // An older set's looped unwarped clip can switch its loop off.
        let mut older = AudioData::new(ContentHash([1; 32]));
        older.looping = true;
        editor
            .apply(Command::SetClipWarp { clip, warp: false })
            .unwrap();
        editor
            .apply(Command::SetClipData {
                clip,
                data: ClipData::Audio(older),
            })
            .unwrap();
        editor
            .apply(Command::SetClipLoopEnabled {
                clip,
                enabled: false,
            })
            .unwrap();
        assert!(!audio(&editor, clip).looping);
    }

    // Spec: audio-clips R15, warping R14. A warped clip's Start marker must be
    // before its brace's end for the loop to come on.
    #[test]
    fn warped_clips_loop_only_inside_their_brace() {
        let (mut editor, clip) = editor_with_clip();
        editor
            .apply(Command::SetClipWarp { clip, warp: true })
            .unwrap();
        editor
            .apply(Command::SetClipLoop {
                clip,
                start: 0.0,
                end: Some(Ticks::QUARTER.get() as f64),
                ticks: true,
            })
            .unwrap();
        editor
            .apply(Command::SetClipRegion {
                clip,
                start: (Ticks::QUARTER * 2).get() as f64,
                end: None,
                ticks: true,
            })
            .unwrap();
        let refused = editor
            .apply(Command::SetClipLoopEnabled {
                clip,
                enabled: true,
            })
            .unwrap_err();
        assert!(matches!(
            refused,
            crate::EditError::Command(ModelError::InvalidValue { .. })
        ));
    }
}

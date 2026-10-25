//! Remote mappings: the MIDI and key mappings a set holds (WP-7.1; the
//! remote mapping spec, R1 to R6).
//!
//! A mapping joins one source (a note, a range of notes, a controller, a
//! 14-bit controller, pitch bend, or a computer key) to one control, named
//! by the IDs of what it belongs to. What a message does to the control is
//! `bl-control`'s; this module holds what is saved.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::automation::MixerParam;
use crate::document::Document;
use crate::error::ModelError;
use crate::fader::{TOP_DB, db_gain};
use crate::ids::{ChainId, DeviceId, ParamId, SceneId, TrackId};
use crate::track::TrackKind;

/// Where a mapping's messages come from. Channels count from 1 to 16, as
/// the window shows them.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[schemars(
    description = "A mapping's source: `note` (a `channel` 1 to 16 and a `key` 0 to 127), \
                   `note_range` (a `channel`, a `low` and a `high` key, and for a slot a \
                   `root` key between them), `cc` (a `channel` and a `controller` 0 to 127), \
                   `cc14` (a `channel` and a `controller` 0 to 31, whose partner 32 higher \
                   carries the low seven bits), `pitch_bend` (a `channel`), or `key` (one \
                   key of the computer keyboard, named as the keymap names it)."
)]
pub enum RemoteSource {
    /// One key of one channel.
    Note {
        /// The channel, 1 to 16.
        channel: u8,
        /// The key, 0 to 127.
        key: u8,
    },
    /// A range of keys of one channel.
    NoteRange {
        /// The channel, 1 to 16.
        channel: u8,
        /// The lowest key.
        low: u8,
        /// The highest key.
        high: u8,
        /// For a slot, the key that plays its clip as it is.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        root: Option<u8>,
    },
    /// A control change.
    Cc {
        /// The channel, 1 to 16.
        channel: u8,
        /// The controller, 0 to 127.
        controller: u8,
    },
    /// A 14-bit control change: the controller carries the high seven bits
    /// and the one 32 higher the low seven.
    Cc14 {
        /// The channel, 1 to 16.
        channel: u8,
        /// The controller, 0 to 31.
        controller: u8,
    },
    /// Pitch bend.
    PitchBend {
        /// The channel, 1 to 16.
        channel: u8,
    },
    /// A key of the computer keyboard.
    Key {
        /// Its name, as the keymap names keys.
        key: String,
    },
}

impl RemoteSource {
    /// Whether it is a computer key rather than a MIDI message.
    #[must_use]
    pub const fn is_key(&self) -> bool {
        matches!(self, Self::Key { .. })
    }

    /// Whether both take a message in common (R5): the same note, a note
    /// inside a range, a controller and the pair a 14-bit mapping takes,
    /// the same key.
    #[must_use]
    pub fn collides(&self, other: &Self) -> bool {
        use RemoteSource::{Cc, Cc14, Key, Note, NoteRange, PitchBend};
        let keys = |s: &Self| match *s {
            Note { channel, key } => Some((channel, key, key)),
            NoteRange { channel, low, high, .. } => Some((channel, low, high)),
            _ => None,
        };
        if let (Some((c, low, high)), Some((d, other_low, other_high))) = (keys(self), keys(other))
        {
            return c == d && low <= other_high && other_low <= high;
        }
        let controllers = |s: &Self| match *s {
            Cc { channel, controller } => Some((channel, controller, controller)),
            Cc14 { channel, controller } => Some((channel, controller, controller + 32)),
            _ => None,
        };
        if let (Some((c, a, b)), Some((d, x, y))) = (controllers(self), controllers(other)) {
            return c == d && (a == x || a == y || b == x || b == y);
        }
        match (self, other) {
            (PitchBend { channel: a }, PitchBend { channel: b }) => a == b,
            (Key { key: a }, Key { key: b }) => a == b,
            _ => false,
        }
    }

    fn check(&self) -> Result<(), ModelError> {
        let channel = |c: u8| {
            if (1..=16).contains(&c) {
                Ok(())
            } else {
                Err(ModelError::InvalidInput("a MIDI channel is 1 to 16"))
            }
        };
        let data = |v: u8| {
            if v < 128 {
                Ok(())
            } else {
                Err(ModelError::InvalidInput("a key or controller is 0 to 127"))
            }
        };
        match self {
            Self::Note { channel: c, key } => channel(*c).and(data(*key)),
            Self::NoteRange { channel: c, low, high, root } => {
                channel(*c).and(data(*high))?;
                if low > high || root.is_some_and(|r| r < *low || r > *high) {
                    return Err(ModelError::InvalidInput(
                        "a note range runs from its lowest key to its highest, with its root \
                         between",
                    ));
                }
                Ok(())
            }
            Self::Cc { channel: c, controller } => channel(*c).and(data(*controller)),
            Self::Cc14 { channel: c, controller } => {
                channel(*c)?;
                if *controller < 32 {
                    Ok(())
                } else {
                    Err(ModelError::InvalidInput(
                        "a 14-bit controller is 0 to 31; its partner is 32 higher",
                    ))
                }
            }
            Self::PitchBend { channel: c } => channel(*c),
            Self::Key { key } if key.is_empty() => {
                Err(ModelError::InvalidInput("a key mapping names its key"))
            }
            Self::Key { .. } => Ok(()),
        }
    }
}

/// What a message does to a control (R3): the control's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[schemars(
    description = "A control's kind: `slot`, `switch`, `ranged_switch` (a track activator or a \
                   device's on/off switch, on for controller values from Min to Max), \
                   `button`, `radio` (with its number of `options`), or `continuous`."
)]
pub enum ControlKind {
    /// A Session slot: a press and a release launch it.
    Slot,
    /// A switch: on or off.
    Switch,
    /// A switch that is on for controller values from Min to Max.
    RangedSwitch,
    /// A button: pressed, and released.
    Button,
    /// A radio button with this many options, in order.
    Radio {
        /// How many options it has, at least 2.
        options: u32,
    },
    /// A continuous control.
    Continuous,
}

/// The four relative encodings an encoder sends (R2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelativeEncoding {
    /// Signed Bit.
    SignedBit,
    /// Signed Bit 2.
    SignedBit2,
    /// Bin Offset.
    BinOffset,
    /// Twos Complement.
    TwosComplement,
}

/// How a controller's values are read (R2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[schemars(
    description = "How a controller's values are read: `absolute`, `absolute14` for a 14-bit \
                   source, or `relative` with its `encoding` (`signed_bit`, `signed_bit2`, \
                   `bin_offset` or `twos_complement`) and whether it is `accelerated`."
)]
pub enum RemoteMode {
    /// The value is a position: 0 to 127.
    #[default]
    Absolute,
    /// The value is a position: 0 to 16,383.
    Absolute14,
    /// The value is an increment or a decrement.
    Relative {
        /// The encoding.
        encoding: RelativeEncoding,
        /// Whether a larger value moves further; otherwise it is linear.
        accelerated: bool,
    },
}

/// The control a mapping drives, named by the IDs of what it belongs to
/// (R3). The kinds the window's other controls take are added with the map
/// modes that show them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[schemars(
    description = "A mappable control: `slot` (a `track` and a `scene`), `scene_launch` (a \
                   `scene`), `track_stop` (a `track`), `stop_all`, `mixer` (a `track` and a \
                   mixer `param`: volume, pan, mute for the track activator), `send` (a \
                   `track` and the return `to`), `solo` or `arm` (a `track`), \
                   `crossfade_assign` (a `track`), `crossfader`, `device` (a `device` and its \
                   `param`), `device_on` (a `device`), or `chain` (a `chain` and its mixer \
                   `param`)."
)]
pub enum RemoteTarget {
    /// A Session slot of a track at a scene, not its clip.
    Slot {
        /// The track.
        track: TrackId,
        /// The scene.
        scene: SceneId,
    },
    /// A scene's launch button.
    SceneLaunch {
        /// The scene.
        scene: SceneId,
    },
    /// A track's stop button.
    TrackStop {
        /// The track.
        track: TrackId,
    },
    /// Stop All Clips.
    StopAll,
    /// A control of a track's mixer.
    Mixer {
        /// The track.
        track: TrackId,
        /// The control.
        param: MixerParam,
    },
    /// A track's send to a return track.
    Send {
        /// The track.
        track: TrackId,
        /// The return track.
        to: TrackId,
    },
    /// A track's Solo switch.
    Solo {
        /// The track.
        track: TrackId,
    },
    /// A track's Arm button.
    Arm {
        /// The track.
        track: TrackId,
    },
    /// A track's crossfade assignment: A, off, B.
    CrossfadeAssign {
        /// The track.
        track: TrackId,
    },
    /// The crossfader.
    Crossfader,
    /// A parameter of a device, whose kind its description gives.
    Device {
        /// The device.
        device: DeviceId,
        /// Its parameter.
        param: ParamId,
    },
    /// A device's on/off switch.
    DeviceOn {
        /// The device.
        device: DeviceId,
    },
    /// A control of a rack chain's mixer.
    Chain {
        /// The chain.
        chain: ChainId,
        /// The control.
        param: MixerParam,
    },
}

/// The values a crossfade assignment's radio button takes: A, off, B.
pub const CROSSFADE_OPTIONS: u32 = 3;

impl RemoteTarget {
    /// The kind of control it is, or `None` for a device parameter, whose
    /// kind its description gives.
    #[must_use]
    pub const fn natural_kind(&self) -> Option<ControlKind> {
        Some(match self {
            Self::Slot { .. } => ControlKind::Slot,
            Self::SceneLaunch { .. } | Self::TrackStop { .. } | Self::StopAll => ControlKind::Button,
            Self::Mixer { param: MixerParam::Mute, .. }
            | Self::Chain { param: MixerParam::Mute, .. }
            | Self::DeviceOn { .. } => ControlKind::RangedSwitch,
            Self::Mixer { .. }
            | Self::Chain { .. }
            | Self::Send { .. }
            | Self::Crossfader => ControlKind::Continuous,
            Self::Solo { .. } | Self::Arm { .. } => ControlKind::Switch,
            Self::CrossfadeAssign { .. } => ControlKind::Radio {
                options: CROSSFADE_OPTIONS,
            },
            Self::Device { .. } => return None,
        })
    }

    /// The range of a continuous control's values, or `None` for a device
    /// parameter, whose range its description gives.
    #[must_use]
    pub fn natural_range(&self) -> Option<(f64, f64)> {
        match self {
            Self::Mixer { param, .. } | Self::Chain { param, .. } => match param {
                MixerParam::Volume => Some((0.0, db_gain(TOP_DB))),
                MixerParam::Pan | MixerParam::SplitLeft | MixerParam::SplitRight => {
                    Some((-1.0, 1.0))
                }
                MixerParam::Mute => None,
            },
            Self::Send { .. } | Self::Crossfader => Some((0.0, 1.0)),
            _ => None,
        }
    }
}

/// The top of a ranged switch's controller values, and the Min and Max a
/// new ranged switch mapping starts with (R4).
pub const RANGED_SWITCH_DEFAULT: (f64, f64) = (64.0, 127.0);

/// One mapping: a source drives a control over a range (R1 to R4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteMapping {
    /// Where the messages come from.
    pub source: RemoteSource,
    /// The control they drive.
    pub target: RemoteTarget,
    /// What kind of control it is, which decides what a message does.
    pub kind: ControlKind,
    /// The bottom of the range, in the control's own units: a continuous
    /// control's value, a radio button's first option, or a ranged
    /// switch's controller value (R4).
    pub min: f64,
    /// The top of the range, in the same units.
    pub max: f64,
    /// How a controller's values are read.
    #[serde(default)]
    pub mode: RemoteMode,
}

impl RemoteMapping {
    /// A mapping of `source` to `target` as `kind`, with the Min and Max a
    /// new mapping starts with (R4): `range` for a continuous control.
    ///
    /// # Errors
    ///
    /// [`ModelError`] if a continuous control has no `range`, or the
    /// mapping breaks a rule of [`RemoteMapping::check`].
    pub fn new(
        source: RemoteSource,
        target: RemoteTarget,
        kind: ControlKind,
        range: Option<(f64, f64)>,
        mode: Option<RemoteMode>,
    ) -> Result<Self, ModelError> {
        let (min, max) = match (kind, range) {
            (_, Some(range)) => range,
            (ControlKind::Radio { options }, None) => (0.0, f64::from(options.saturating_sub(1))),
            (ControlKind::RangedSwitch, None) => RANGED_SWITCH_DEFAULT,
            (ControlKind::Continuous, None) => {
                return Err(ModelError::InvalidInput(
                    "a continuous control needs its Min and Max",
                ));
            }
            _ => (0.0, 1.0),
        };
        let mode = mode.unwrap_or(match source {
            RemoteSource::Cc14 { .. } => RemoteMode::Absolute14,
            _ => RemoteMode::Absolute,
        });
        let mapping = Self {
            source,
            target,
            kind,
            min,
            max,
            mode,
        };
        mapping.check()?;
        Ok(mapping)
    }

    /// Checks the mapping against the rules of R1 to R4.
    ///
    /// # Errors
    ///
    /// [`ModelError`] naming the first rule it breaks.
    pub fn check(&self) -> Result<(), ModelError> {
        self.source.check()?;
        let on_target = self.target.natural_kind();
        if on_target.is_some_and(|natural| natural != self.kind) {
            return Err(ModelError::InvalidInput(
                "the control's kind isn't the one that control has",
            ));
        }
        if !self.min.is_finite() || !self.max.is_finite() {
            return Err(ModelError::InvalidInput("Min and Max are numbers"));
        }
        let whole = |v: f64, top: f64| v.fract() == 0.0 && (0.0..=top).contains(&v);
        match self.kind {
            ControlKind::Radio { options } => {
                if options < 2 || !whole(self.min, f64::from(options - 1)) || !whole(self.max, f64::from(options - 1)) {
                    return Err(ModelError::InvalidInput(
                        "a radio button's Min and Max are options of its own",
                    ));
                }
            }
            ControlKind::RangedSwitch if !whole(self.min, 127.0) || !whole(self.max, 127.0) => {
                return Err(ModelError::InvalidInput(
                    "a ranged switch's Min and Max are controller values, 0 to 127",
                ));
            }
            ControlKind::Continuous if self.source.is_key() => {
                return Err(ModelError::InvalidInput(
                    "a continuous control can't be mapped to a key",
                ));
            }
            _ => {}
        }
        if matches!(self.source, RemoteSource::NoteRange { .. })
            && !matches!(
                self.kind,
                ControlKind::Slot | ControlKind::Continuous | ControlKind::Radio { .. }
            )
        {
            return Err(ModelError::InvalidInput(
                "only a slot, a radio button or a continuous control maps to a range of notes",
            ));
        }
        let mode_fits = match (&self.source, self.mode) {
            (RemoteSource::Cc14 { .. }, RemoteMode::Absolute14)
            | (RemoteSource::Cc { .. }, RemoteMode::Absolute | RemoteMode::Relative { .. }) => true,
            (RemoteSource::Cc14 { .. } | RemoteSource::Cc { .. }, _) => false,
            (_, mode) => mode == RemoteMode::Absolute,
        };
        if mode_fits {
            Ok(())
        } else {
            Err(ModelError::InvalidInput(
                "that mode doesn't fit the source: a 14-bit controller reads Absolute (14-bit), \
                 only a controller reads relative, the rest read Absolute",
            ))
        }
    }
}

/// Checks a set's mappings: each is well formed, and no two share a source
/// or share a control of a kind (R5).
///
/// # Errors
///
/// [`ModelError`] naming the first rule a mapping breaks.
pub fn check_all(mappings: &[RemoteMapping]) -> Result<(), ModelError> {
    for (at, mapping) in mappings.iter().enumerate() {
        mapping.check()?;
        for other in &mappings[..at] {
            if mapping.source.collides(&other.source) {
                return Err(ModelError::InvalidInput("one source maps one control"));
            }
            if mapping.target == other.target && mapping.source.is_key() == other.source.is_key() {
                return Err(ModelError::InvalidInput(
                    "a control has at most one MIDI mapping and one key mapping",
                ));
            }
        }
    }
    Ok(())
}

/// `mappings` with `new` made (R5): a mapping that took the new source or
/// the new control's place of its kind is gone.
// shortcut: a mapping whose note range the new source overlaps is dropped
// whole; trimming the range to the keys left is the upgrade
#[must_use]
pub fn with_mapping(mappings: &[RemoteMapping], new: RemoteMapping) -> Vec<RemoteMapping> {
    let mut kept: Vec<RemoteMapping> = mappings
        .iter()
        .filter(|m| {
            !m.source.collides(&new.source)
                && !(m.target == new.target && m.source.is_key() == new.source.is_key())
        })
        .cloned()
        .collect();
    kept.push(new);
    kept
}

impl Document {
    /// How many mappings the load that made this document dropped because
    /// their control isn't in the set (R6).
    #[must_use]
    pub fn remote_dropped(&self) -> usize {
        self.remote_dropped
    }

    /// Whether the set holds the control `target` names.
    #[must_use]
    pub fn has_remote_target(&self, target: &RemoteTarget) -> bool {
        let sends = |track: TrackId| self.track(track).is_some_and(|t| t.kind().has_sends());
        match *target {
            RemoteTarget::Slot { track, scene } => {
                self.scene(scene).is_some()
                    && self
                        .track(track)
                        .is_some_and(|t| matches!(t.kind(), TrackKind::Audio | TrackKind::Midi))
            }
            RemoteTarget::SceneLaunch { scene } => self.scene(scene).is_some(),
            RemoteTarget::TrackStop { track }
            | RemoteTarget::Solo { track }
            | RemoteTarget::Arm { track }
            | RemoteTarget::CrossfadeAssign { track }
            | RemoteTarget::Mixer { track, .. } => self.track(track).is_some(),
            RemoteTarget::Send { track, to } => sends(track) && self.returns().any(|r| r == to),
            RemoteTarget::StopAll | RemoteTarget::Crossfader => true,
            RemoteTarget::Device { device, .. } | RemoteTarget::DeviceOn { device } => {
                self.device(device).is_some()
            }
            RemoteTarget::Chain { chain, .. } => self.chain(chain).is_some(),
        }
    }

    /// Takes the mappings whose control the set no longer has out of it.
    pub(crate) fn prune_remote(&mut self) {
        if self.settings.remote.iter().any(|m| !self.has_remote_target(&m.target)) {
            let kept = self
                .settings
                .remote
                .iter()
                .filter(|m| self.has_remote_target(&m.target))
                .cloned()
                .collect();
            self.settings.remote = kept;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(channel: u8, key: u8) -> RemoteSource {
        RemoteSource::Note { channel, key }
    }

    fn solo(track: u64) -> RemoteTarget {
        RemoteTarget::Solo { track: TrackId(bl_core::id::Id::new(track).unwrap()) }
    }

    fn map(source: RemoteSource, target: RemoteTarget) -> RemoteMapping {
        let kind = target.natural_kind().unwrap();
        RemoteMapping::new(source, target, kind, None, None).unwrap()
    }

    // Spec: remote-mapping R5
    #[test]
    fn a_note_range_counts_as_each_of_its_keys_and_a_pair_as_both_controllers() {
        let range = RemoteSource::NoteRange { channel: 1, low: 48, high: 60, root: Some(55) };
        assert!(range.collides(&note(1, 50)) && !range.collides(&note(2, 50)));
        let pair = RemoteSource::Cc14 { channel: 1, controller: 7 };
        let low = RemoteSource::Cc { channel: 1, controller: 39 };
        assert!(pair.collides(&low) && !pair.collides(&RemoteSource::Cc { channel: 1, controller: 8 }));
    }

    // Spec: remote-mapping R5
    #[test]
    fn mapping_a_source_again_moves_it_and_mapping_a_control_again_replaces_its_mapping() {
        let a = map(note(1, 60), solo(2));
        let b = map(note(1, 60), RemoteTarget::Arm { track: TrackId(bl_core::id::Id::new(3).unwrap()) });
        let moved = with_mapping(&[a.clone()], b.clone());
        assert_eq!(moved, vec![b]);
        let c = map(note(1, 61), solo(2));
        assert_eq!(with_mapping(&[a], c.clone()), vec![c]);
    }

    // Spec: remote-mapping R2, R4
    #[test]
    fn a_new_mappings_range_and_mode_come_from_its_kind_and_source() {
        let m = RemoteMapping::new(
            RemoteSource::Cc14 { channel: 1, controller: 7 },
            RemoteTarget::Mixer {
                track: TrackId(bl_core::id::Id::new(2).unwrap()),
                param: MixerParam::Volume,
            },
            ControlKind::Continuous,
            Some((0.0, 2.0)),
            None,
        )
        .unwrap();
        assert_eq!(m.mode, RemoteMode::Absolute14);
        let activator = map(
            note(1, 1),
            RemoteTarget::Mixer {
                track: TrackId(bl_core::id::Id::new(2).unwrap()),
                param: MixerParam::Mute,
            },
        );
        assert_eq!((activator.min, activator.max), RANGED_SWITCH_DEFAULT);
        assert!(RemoteMapping::new(note(1, 1), RemoteTarget::Crossfader, ControlKind::Continuous, None, None).is_err());
    }
}

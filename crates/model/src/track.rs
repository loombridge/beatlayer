//! Tracks and their mixer settings.

use core::fmt;
use std::collections::BTreeSet;

use bl_core::time::Ticks;
use imbl::{OrdMap, Vector};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::automation::{Automation, AutomationTarget, TrackAutomation};
use crate::clip::{AudioData, Clip, ClipData, EnvelopeKind, TakeLane};
use crate::device::Device;
use crate::freeze::FrozenTrack;
use crate::ids::{ChainId, ClipId, DeviceId, SceneId, TakeLaneId, TrackId};
use crate::rack::Chain;
use crate::shared::Shared;

/// A frozen group or return track's whole-Arrangement output as the clip
/// its player runs (R14, R15).
fn frozen_output_clip(output: crate::freeze::FrozenOutput) -> Clip {
    let seconds = output.length.as_quarters() * 60.0 / output.tempo;
    let mut audio = AudioData::new(output.media);
    audio.end = Some(seconds);
    audio.warp = false;
    audio.tempo = Some(output.tempo);
    audio.warp_markers = vec![
        crate::warp::WarpMarker {
            time: 0.0,
            beat: Ticks::ZERO,
        },
        crate::warp::WarpMarker {
            time: seconds,
            beat: output.length,
        },
    ];
    Clip {
        id: output.id,
        name: String::new(),
        start: output.start,
        length: output.length,
        offset: Ticks::ZERO,
        mute: false,
        data: ClipData::Audio(audio),
        envelopes: Vec::new(),
        launch: crate::launch::ClipLaunch::default(),
        scale: crate::scale::Scale::default(),
        groove: None,
        colour: None,
        info_text: String::new(),
    }
}

/// A track's pan mode (the mixer spec, manual 18.1): the default Stereo Pan
/// Mode, where one control balances both channels, or Split Stereo Pan Mode,
/// where a control places each input channel on its own.
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
#[schemars(
    description = "A track's pan mode: `stereo` (the default) balances both channels with one \
                   control, `split` places the left and right channels with two controls."
)]
pub enum PanMode {
    /// One control balances both channels (the default).
    #[default]
    Stereo,
    /// Two controls place each input channel on its own.
    Split,
}

impl PanMode {
    /// Whether this is [`PanMode::Stereo`], the default, so a set leaves it
    /// out.
    #[must_use]
    pub fn is_stereo(&self) -> bool {
        matches!(self, Self::Stereo)
    }
}

/// A track delay's unit is part of its value, like Hardware Latency's: the
/// value is kept in milliseconds or in samples (frames) and the unit switch
/// converts it at the current sample rate.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "unit", rename_all = "snake_case")]
#[schemars(
    description = "A track's delay (the mixer spec): `{ \"unit\": \"milliseconds\", \"value\": \
                   10.0 }` (−1000 to 1000, two decimals; kept in milliseconds when the sample \
                   rate changes) or `{ \"unit\": \"samples\", \"value\": 480 }` (−192000 to \
                   192000; kept in samples). Positive delays the track, negative plays it \
                   early."
)]
pub enum TrackDelay {
    /// Milliseconds, −1000 to 1000.
    Milliseconds {
        /// The delay: positive delays the track, negative plays it early.
        value: f64,
    },
    /// Samples (frames of the current sample rate), −192000 to 192000.
    Samples {
        /// The delay: positive delays the track, negative plays it early.
        value: i64,
    },
}

impl Default for TrackDelay {
    /// A new track's: 0 ms.
    fn default() -> Self {
        Self::Milliseconds { value: 0.0 }
    }
}

/// The most track delay in milliseconds: Beatlayer's own bound.
pub const MAX_TRACK_DELAY_MS: f64 = 1000.0;

/// The most track delay in samples: Beatlayer's own bound, the same as
/// Hardware Latency's.
pub const MAX_TRACK_DELAY_SAMPLES: i64 = 192_000;

impl TrackDelay {
    /// Whether it is the default, 0 ms.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Whether the value is in range (and finite).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        match *self {
            Self::Milliseconds { value } => {
                (-MAX_TRACK_DELAY_MS..=MAX_TRACK_DELAY_MS).contains(&value)
            }
            Self::Samples { value } => {
                (-MAX_TRACK_DELAY_SAMPLES..=MAX_TRACK_DELAY_SAMPLES).contains(&value)
            }
        }
    }

    /// The delay in whole frames at `sample_rate` hertz: positive delays the
    /// track, negative plays it early.
    #[must_use]
    pub fn frames(&self, sample_rate: u32) -> i64 {
        match *self {
            Self::Milliseconds { value } => {
                (value * f64::from(sample_rate) / 1000.0).round() as i64
            }
            Self::Samples { value } => value,
        }
    }

    /// The same delay in the other unit at `sample_rate`, to whole samples
    /// when it becomes samples and to two decimals when it becomes
    /// milliseconds, as Hardware Latency's unit switch converts.
    #[must_use]
    pub fn switched(&self, sample_rate: u32) -> Self {
        match *self {
            Self::Milliseconds { .. } => Self::Samples {
                value: self.frames(sample_rate),
            },
            Self::Samples { value } => Self::Milliseconds {
                value: (value as f64 * 1000.0 / f64::from(sample_rate.max(1)) * 100.0).round()
                    / 100.0,
            },
        }
    }
}

/// Whether an `f64` is 0.0, where a set leaves a split pan value out.
fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

/// The kind of a track, fixed when the track is created.
///
/// The rules for each kind are in the tracks and undo spec.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[schemars(description = "The kind of a track, fixed when the track is created.")]
pub enum TrackKind {
    /// Holds audio clips.
    Audio,
    /// Holds MIDI clips.
    Midi,
    /// Holds other tracks and sums their output.
    Group,
    /// Receives sends from other tracks.
    Return,
    /// The song's output. There is exactly one.
    Main,
}

impl TrackKind {
    /// Whether tracks of this kind hold clips: audio and MIDI tracks do.
    #[must_use]
    pub const fn holds_clips(self) -> bool {
        matches!(self, Self::Audio | Self::Midi)
    }

    /// Whether tracks of this kind can go inside a group: audio, MIDI, and
    /// group tracks can.
    #[must_use]
    pub const fn can_be_grouped(self) -> bool {
        matches!(self, Self::Audio | Self::Midi | Self::Group)
    }

    /// Whether tracks of this kind have mute and solo switches: every kind
    /// but Main does.
    #[must_use]
    pub const fn has_mute_and_solo(self) -> bool {
        !matches!(self, Self::Main)
    }

    /// Whether tracks of this kind have an arm switch: audio and MIDI tracks
    /// do.
    #[must_use]
    pub const fn has_arm(self) -> bool {
        matches!(self, Self::Audio | Self::Midi)
    }

    /// Whether tracks of this kind have a send to each return track: audio,
    /// MIDI, and group tracks do. (Live gives return tracks sends too,
    /// switched off until the user enables them; Beatlayer adds those with
    /// the track context menu, whose Enable Send turns them on.)
    #[must_use]
    pub const fn has_sends(self) -> bool {
        matches!(self, Self::Audio | Self::Midi | Self::Group)
    }

    /// Whether tracks of this kind have the crossfader's A/B assign
    /// buttons: every kind but Main, which carries the crossfader itself
    /// (the mixer spec, R23).
    #[must_use]
    pub const fn has_crossfade_assign(self) -> bool {
        !matches!(self, Self::Main)
    }
}

/// Which side of the crossfader a track is assigned to, through its A and B
/// assign buttons (the mixer spec, R23). The two sides are exclusive: a
/// track is on A, on B, or on neither, and one assigned to neither plays
/// through at every crossfader position.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "Which side of the crossfader a track is assigned to: `a`, `b`, or `none` \
                   (the default), where the track plays through at every crossfader position."
)]
pub enum CrossfadeAssign {
    /// On neither side: the crossfader doesn't change the track's level.
    #[default]
    None,
    /// On the crossfader's A side.
    A,
    /// On the crossfader's B side.
    B,
}

impl CrossfadeAssign {
    /// Whether this is [`CrossfadeAssign::None`], the default.
    #[must_use]
    pub fn is_none(&self) -> bool {
        *self == Self::None
    }
}

impl fmt::Display for TrackKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Audio => "audio",
            Self::Midi => "MIDI",
            Self::Group => "group",
            Self::Return => "return",
            Self::Main => "Main",
        })
    }
}

/// Where a return track's sends take their signal from the tracks that send
/// to it: the return's Pre/Post switch (the mixer spec).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "Where a return track's sends take their signal from each sending track: \
                   `post` (the default) after the sending track's pan, volume, and activator, \
                   or `pre` before them, straight after its device chain."
)]
pub enum SendTap {
    /// After the sending track's pan, volume, and activator: the default.
    #[default]
    Post,
    /// Before them, straight after the sending track's device chain.
    Pre,
}

impl SendTap {
    /// Whether this is [`SendTap::Post`], the default.
    #[must_use]
    pub fn is_post(&self) -> bool {
        *self == Self::Post
    }
}

/// One of the audio interface's channels: a single channel or a stereo
/// pair, counted from 1 as Live's In/Out choosers show them (the routing
/// spec, WP-2.13).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "channel", rename_all = "snake_case")]
#[schemars(
    description = "One of the audio interface's channels: `mono` a single channel, or `pair` a \
                   stereo pair starting at it, counted from 1 (`{ \"type\": \"pair\", \
                   \"channel\": 3 }` is the pair 3/4)."
)]
pub enum Channel {
    /// A single channel, counted from 1.
    Mono(u16),
    /// A stereo pair: its first channel, counted from 1; the second is the
    /// next one.
    Pair(u16),
}

impl Default for Channel {
    /// The first stereo pair, `1/2`: what an interface's first channels are
    /// chosen as.
    fn default() -> Self {
        Self::Pair(1)
    }
}

impl Channel {
    /// Whether this is the default, `pair 1`.
    #[must_use]
    pub fn is_first_pair(&self) -> bool {
        *self == Self::Pair(1)
    }

    /// Whether the two choices share a channel: `1/2` overlaps `2` and
    /// `2/3`, not `3/4`.
    #[must_use]
    pub fn overlaps(self, other: Self) -> bool {
        let span = |c: Self| match c {
            Self::Mono(c) => (c, c),
            Self::Pair(c) => (c, c.saturating_add(1)),
        };
        let (a, b) = (span(self), span(other));
        a.0 <= b.1 && b.0 <= a.1
    }

    /// What Live's choosers show: `1` for one channel, `1/2` for a pair.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Mono(channel) => channel.to_string(),
            Self::Pair(channel) => format!("{channel}/{}", u32::from(channel) + 1),
        }
    }
}

/// Where a track takes its input from: the In/Out section's input type
/// chooser, Audio From on an audio track and MIDI From on a MIDI track, with
/// its channel (the routing spec).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[schemars(
    description = "Where a track takes its input from. Audio tracks: `external` (Ext. In, the \
                   audio interface's inputs, with its `channel`; a new audio track's), \
                   `resampling` (the Main output), `track` (another audio track, a MIDI track with \
                   an instrument, a return, or Main, tapped at `tap`; with a `chain` of an \
                   instrument or audio effect rack on that track, that chain's signal at \
                   `tap`; or with an `output`, one of an instrument's extra outputs on that \
                   track), or `none` (No Input). MIDI tracks: `all_ins` (every \
                   MIDI input; a new MIDI track's), `computer_keyboard`, `port` (one MIDI input, \
                   by its name as the MIDI settings list it), `track` (another MIDI track), or \
                   `none`."
)]
pub enum Input {
    /// No Input.
    None,
    /// Ext. In: the audio interface's inputs, one channel or a stereo pair.
    /// Audio tracks only.
    External {
        /// Which of the interface's inputs, counted from 1: a single channel
        /// or a stereo pair. The first pair (1/2) by default.
        #[serde(default, skip_serializing_if = "Channel::is_first_pair")]
        channel: Channel,
    },
    /// All Ins: every MIDI input port whose Track switch is on, and the
    /// computer MIDI keyboard. MIDI tracks only.
    AllIns,
    /// The computer MIDI keyboard. MIDI tracks only.
    ComputerKeyboard,
    /// One MIDI input port, by the name the MIDI system gives it. MIDI
    /// tracks only. The track keeps the name while the port is away and
    /// receives again when a port of that name returns.
    Port {
        /// The port's name, as the MIDI settings list it.
        name: String,
    },
    /// Resampling: the Main output. Audio tracks only.
    Resampling,
    /// Another track's signal.
    Track {
        /// The track: for an audio track, another audio track, a return
        /// track, or Main; for a MIDI track, another MIDI track.
        track: TrackId,
        /// Where in that track the signal is taken: Post Mixer if it's left
        /// out. An instrument output is taken whatever this says.
        #[serde(default, skip_serializing_if = "Tap::is_post_mixer")]
        tap: Tap,
        /// A chain of an instrument or audio effect rack on that track, at
        /// any depth, whose signal is taken at `tap` instead of the
        /// track's own (the racks spec, R38 to R41).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chain: Option<ChainId>,
        /// An instrument on that track, at any depth, whose extra output is
        /// taken instead of the track's own signal (the routing spec).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<DeviceOutput>,
    },
}

/// One of a device's extra audio outputs, past its main one: an
/// instrument's second stereo output, one of the eight-slot drum sampler's
/// individual outputs (the routing spec, R12, R16).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "One of a device's extra audio outputs, past its main one: `device` (an \
                   instrument on the tapped track, at any depth in its racks) and `output`, \
                   which of that device's extra outputs it is, counted from 0 in the order the \
                   device declares them."
)]
pub struct DeviceOutput {
    /// The device.
    pub device: DeviceId,
    /// Which of its extra outputs, counted from 0 past the main one.
    pub output: u32,
}

/// Where a track's signal is taken when another track takes it as its
/// input: the input channel chooser's Pre FX, Post FX, and Post Mixer.
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
#[schemars(
    description = "Where a track's signal is taken: `pre_fx` before its devices, `post_fx` \
                   after its devices and before its mixer, or `post_mixer` (the default) after \
                   its pan, volume, and activator."
)]
pub enum Tap {
    /// Before the track's devices and mixer.
    PreFx,
    /// After the track's devices, before its mixer.
    PostFx,
    /// After the track's mixer: its output. The default.
    #[default]
    PostMixer,
}

impl Tap {
    /// Whether this is [`Tap::PostMixer`], the default.
    #[must_use]
    pub fn is_post_mixer(&self) -> bool {
        *self == Self::PostMixer
    }
}

/// A track's Monitor switch: whether its input is heard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "Whether a track's input is heard: `in` always (its clips are silent), \
                   `auto` while the track is armed, or `off` never."
)]
pub enum Monitor {
    /// The input is always heard, and the track's clips are silent.
    In,
    /// The input is heard while the track is armed.
    Auto,
    /// The input is never heard.
    Off,
}

/// Where a track's audio goes: the In/Out section's Audio To chooser (the
/// routing spec).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[schemars(
    description = "Where a track's audio goes: `default` (Main, or the group the track is \
                   in), `main` (Main, even from inside a group), `track` (another audio \
                   track's input, or the group the track is in), `sends_only` (only through \
                   its sends), or `external` (Ext. Out, the audio interface's outputs, with its \
                   `channel`: a single channel or a stereo pair, the first pair by default)."
)]
pub enum Output {
    /// Main, or the group the track is in: where a new track's audio goes.
    #[default]
    Default,
    /// Main, even from a track inside a group.
    Main,
    /// Ext. Out: the audio interface's outputs. The track plays on those
    /// device channels and not into Main (WP-2.13).
    External {
        /// Which of the interface's outputs, counted from 1: a single
        /// channel or a stereo pair. The first pair (1/2) by default.
        #[serde(default, skip_serializing_if = "Channel::is_first_pair")]
        channel: Channel,
    },
    /// Nowhere but the track's sends.
    SendsOnly,
    /// Another track: an audio track's input (Track In), or the group the
    /// track is in.
    Track {
        /// The track.
        track: TrackId,
    },
}

/// Where a MIDI track's MIDI goes: the In/Out section's MIDI To chooser and
/// its channel (the routing spec, R4; the external devices spec, R1, R2).
///
/// A track is saved as its ID alone, as MIDI To has always been; a port as
/// its name and channel.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(
    description = "Where a MIDI track's MIDI goes: another MIDI track (its ID), or an output \
                   port, `{ \"name\": \"Synth\", \"channel\": 1 }`: the MIDI output port's \
                   name as the MIDI settings list it, and the channel to send on, 1 to 16; \
                   with `mpe` (the MPE/Multi-channel Settings, `{}` for the new ones) the \
                   track sends MPE MIDI in place of one channel."
)]
pub enum MidiTo {
    /// Another MIDI track's Track In.
    Track(TrackId),
    /// A MIDI output port, by the name the MIDI system gives it, and the
    /// channel everything is sent on. The track keeps the name while the
    /// port is away and sends again when a port of that name returns.
    Port {
        /// The port's name, as the MIDI settings list it.
        name: String,
        /// The channel, 1 to 16.
        channel: u8,
        /// MPE, chosen after the 16 channels in the channel chooser, with
        /// the MPE/Multi-channel Settings the notes go out on (the MPE
        /// spec, R19); `None` sends on `channel`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mpe: Option<crate::MpeSettings>,
    },
}

impl MidiTo {
    /// A port with Beatlayer's first channel, 1: what choosing a port in
    /// the chooser takes.
    #[must_use]
    pub fn port(name: impl Into<String>) -> Self {
        Self::Port {
            name: name.into(),
            channel: 1,
            mpe: None,
        }
    }

    /// The track it names, if it names one.
    #[must_use]
    pub const fn track(&self) -> Option<TrackId> {
        match self {
            Self::Track(track) => Some(*track),
            Self::Port { .. } => None,
        }
    }
}

impl Output {
    /// Whether this is [`Output::Default`].
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::Default
    }
}

/// A track's In/Out settings: its input and its channel, Monitor, and its
/// output (the routing spec).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Routing {
    /// Audio From or MIDI From.
    pub input: Input,
    /// The input channel: the MIDI channel to take, 1 to 16, or `None` for
    /// All Channels. Only a MIDI port input has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u8>,
    /// The Monitor switch.
    pub monitor: Monitor,
    /// Audio To.
    #[serde(default, skip_serializing_if = "Output::is_default")]
    pub output: Output,
    /// MIDI To, on a MIDI track: another MIDI track, an output port with a
    /// channel, or none (No Output, a new MIDI track's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub midi_output: Option<MidiTo>,
    /// Main Out, on Main: the interface's output channels the Main mix goes
    /// to, as a stereo pair. Saved only where it differs from the first
    /// pair.
    #[serde(default, skip_serializing_if = "Channel::is_first_pair")]
    pub main_out: Channel,
    /// Cue Out, on Main: the interface's output channels the cue mix goes
    /// to, as a stereo pair. It carries the cue mix once cue mode is built
    /// (a later package); the choice is kept and saved with the set.
    #[serde(default, skip_serializing_if = "Channel::is_first_pair")]
    pub cue_out: Channel,
}

impl Routing {
    /// A new track's routing: an audio track takes Ext. In, a MIDI track
    /// All Ins on All Channels, both with Monitor Auto, and both go to Main
    /// (or their group); MIDI To is No Output. Other kinds take no input.
    /// Main Out and Cue Out are the interface's first pair.
    #[must_use]
    pub const fn new(kind: TrackKind) -> Self {
        let (input, monitor) = match kind {
            TrackKind::Audio => (
                Input::External {
                    channel: Channel::Pair(1),
                },
                Monitor::Auto,
            ),
            TrackKind::Midi => (Input::AllIns, Monitor::Auto),
            _ => (Input::None, Monitor::Off),
        };
        Self {
            input,
            channel: None,
            monitor,
            output: Output::Default,
            midi_output: None,
            main_out: Channel::Pair(1),
            cue_out: Channel::Pair(1),
        }
    }

    /// Whether the input is heard, given the Arm switch.
    #[must_use]
    pub const fn monitoring(&self, armed: bool) -> bool {
        match self.monitor {
            Monitor::In => true,
            Monitor::Auto => armed,
            Monitor::Off => false,
        }
    }
}

/// A track's mixer settings: its volume, pan, and switches. Its sends are
/// [`Track::sends`]. The rules are in the mixer spec.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Mixer {
    /// Linear gain: 1.0 is unity (0 dB) and 0.0 is silence (−∞ dB). Finite
    /// and never negative.
    pub volume: f64,
    /// Stereo position from −1.0 (left) to 1.0 (right).
    pub pan: f64,
    /// Whether the track is muted: its Track Activator is off. Always off on
    /// the Main track.
    pub mute: bool,
    /// Whether the track is soloed, or, in Cue mode, cue-switched. Always off
    /// on the Main track.
    pub solo: bool,
    /// Whether the track is armed for recording. Only audio and MIDI tracks
    /// can be armed.
    pub arm: bool,
    /// Which side of the crossfader the track plays on, set by its A and B
    /// assign buttons; [`CrossfadeAssign::None`] (the default) plays through
    /// at every position. Always `None` on the Main track, which carries
    /// the crossfader itself (the mixer spec, R23). Saved only when it's
    /// assigned, so a set saved before WP-3.17 loads unchanged.
    #[serde(default, skip_serializing_if = "CrossfadeAssign::is_none")]
    pub crossfade: CrossfadeAssign,
    /// Where sends to this track take their signal from: only a return
    /// track's can be [`SendTap::Pre`]. Saved only when it's `Pre`, so a set
    /// saved before WP-1.6 loads unchanged.
    #[serde(default, skip_serializing_if = "SendTap::is_post")]
    pub tap: SendTap,
    /// The pan mode (the mixer spec, manual 18.1). Saved only where it isn't
    /// the default Stereo Pan Mode.
    #[serde(default, skip_serializing_if = "PanMode::is_stereo")]
    pub pan_mode: PanMode,
    /// In Split Stereo Pan Mode, the left input channel's position, from −1
    /// (left) to 1 (right), on the constant-power law. Saved only where it
    /// differs from the default (centre).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub split_left: f64,
    /// In Split Stereo Pan Mode, the right input channel's position, as
    /// [`Mixer::split_left`]. Saved only where it differs from the default.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub split_right: f64,
}

impl Default for Mixer {
    /// Unity gain, centred, every switch off, unassigned on the crossfader,
    /// and sends taken after the mixer.
    fn default() -> Self {
        Self {
            volume: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            arm: false,
            crossfade: CrossfadeAssign::None,
            tap: SendTap::Post,
            pan_mode: PanMode::Stereo,
            split_left: 0.0,
            split_right: 0.0,
        }
    }
}

/// A track: its mixer, sends, and automation, its device chain, and either
/// its clips (audio and MIDI tracks) or the tracks inside it (groups).
///
/// Tracks change only through commands, so this type has no setters.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub(crate) id: TrackId,
    pub(crate) kind: TrackKind,
    pub(crate) name: String,
    pub(crate) mixer: Mixer,
    /// The level of each send that isn't at −∞ dB, by return track: a linear
    /// gain above 0 and at most 1. A send that isn't listed sends nothing.
    pub(crate) sends: OrdMap<TrackId, f64>,
    pub(crate) routing: Routing,
    pub(crate) devices: Vector<Shared<Device>>,
    pub(crate) clips: OrdMap<ClipId, Shared<Clip>>,
    /// The track's delay (the mixer spec, manual 18.7): the frames its output
    /// is shifted by, positive to delay it and negative to play it early.
    pub(crate) delay: TrackDelay,
    /// Keep Monitoring Latency (the delay-compensation spec, manual 18.8):
    /// whether a monitored track keeps its delay compensation so the
    /// recording matches what is heard. On in a new track; saved only when
    /// off.
    pub(crate) keep_monitoring_latency: bool,
    /// The clips in the track's Session slots, by scene. A slot that holds
    /// nothing isn't listed: every audio and MIDI track has a slot in every
    /// scene, and a slot with no entry is empty. Return, group, and Main
    /// tracks have none.
    pub(crate) slots: OrdMap<SceneId, Shared<Clip>>,
    /// The scenes whose slot's Clip Stop button was removed (WP-3.2, C-SLOT-006,
    /// S-SES-005): every audio and MIDI slot has one until then. Return,
    /// group, and Main tracks have none.
    pub(crate) removed_stop_buttons: BTreeSet<SceneId>,
    /// The take lanes under the main lane, in order (WP-6.21): audio tracks'
    /// hold audio clips and MIDI tracks' MIDI clips.
    pub(crate) take_lanes: Vector<TakeLane>,
    /// The take lane the track plays in place of its main lane, if one is
    /// auditioned (the comping spec, R14).
    pub(crate) audition: Option<TakeLaneId>,
    pub(crate) children: Vector<TrackId>,
    /// One envelope per automated parameter of this track (WP-2.12): a
    /// control of its mixer, one of its sends, or a parameter of a device in
    /// its chain.
    pub(crate) automation: TrackAutomation,
    /// The track's frozen audio (WP-4.14): the files that play in place of
    /// its clips and devices. A frozen track keeps its devices and clip
    /// settings, so unfreezing plays them again (the freeze and bounce spec,
    /// R10).
    pub(crate) frozen: Option<FrozenTrack>,
    /// Bypass Tuning (WP-6.9; the tunings spec, R14): whether the track
    /// plays twelve equal steps whatever tuning the set holds. Off in a new
    /// track, and saved only when on.
    pub(crate) tuning_bypass: bool,
    /// The MIDI Controller Layout (the tunings spec, R22, R23).
    pub(crate) tuning_layout: crate::TuningLayout,
    /// The group of linked tracks the track is in, if any (WP-6.23; the
    /// linked-track editing spec, R1). A group always has two tracks or
    /// more.
    pub(crate) link: Option<crate::ids::LinkGroupId>,
    /// The track's colour, a swatch from 0 to 69 (the themes spec, R14). A
    /// new track takes swatch `id mod 14`.
    pub(crate) colour: u8,
    /// The user's own info text (Edit Info Text); empty for none (the Info
    /// View spec, R9).
    pub(crate) info_text: String,
}

impl Track {
    pub(crate) fn new(id: TrackId, kind: TrackKind, name: String) -> Self {
        Self {
            id,
            kind,
            name,
            colour: crate::colour::id_swatch(id.0),
            mixer: Mixer::default(),
            sends: OrdMap::new(),
            routing: Routing::new(kind),
            devices: Vector::new(),
            clips: OrdMap::new(),
            delay: TrackDelay::default(),
            keep_monitoring_latency: true,
            slots: OrdMap::new(),
            removed_stop_buttons: BTreeSet::new(),
            take_lanes: Vector::new(),
            audition: None,
            children: Vector::new(),
            automation: TrackAutomation::new(),
            frozen: None,
            tuning_bypass: false,
            tuning_layout: crate::TuningLayout::default(),
            link: None,
            info_text: String::new(),
        }
    }

    /// The track's frozen audio, if it's frozen (WP-4.14).
    #[must_use]
    pub fn frozen(&self) -> Option<&FrozenTrack> {
        self.frozen.as_ref()
    }

    /// Whether the track is frozen.
    #[must_use]
    pub fn is_frozen(&self) -> bool {
        self.frozen.is_some()
    }

    /// The clips the engine plays: a frozen track's clips are the stretches
    /// of its freeze files, warped in Beats mode to the beats they were
    /// rendered at (R6); otherwise its own clips.
    #[must_use]
    pub fn playback_clips(&self) -> Vec<Clip> {
        let Some(frozen) = &self.frozen else {
            return self.played_clips().cloned().collect();
        };
        if let Some(output) = frozen.output {
            return vec![frozen_output_clip(output)];
        }
        // The freeze's audio already sits in the clips' own data, put there
        // by the freeze command.
        self.clips.values().map(|clip| (**clip).clone()).collect()
    }

    /// The Session slot clips the engine plays, as [`Track::playback_clips`].
    #[must_use]
    pub fn playback_slots(&self) -> Vec<(SceneId, Clip)> {
        let Some(frozen) = &self.frozen else {
            return self
                .slots
                .iter()
                .map(|(scene, clip)| (*scene, (**clip).clone()))
                .collect();
        };
        let _ = frozen;
        self.slots
            .iter()
            .map(|(scene, clip)| (*scene, (**clip).clone()))
            .collect()
    }

    /// The track's ID.
    #[must_use]
    pub fn id(&self) -> TrackId {
        self.id
    }

    /// The track's own info text, empty for none.
    #[must_use]
    pub fn info_text(&self) -> &str {
        &self.info_text
    }

    /// The track's colour: a swatch from 0 to 69.
    #[must_use]
    pub fn colour(&self) -> u8 {
        self.colour
    }

    /// The track's kind.
    #[must_use]
    pub fn kind(&self) -> TrackKind {
        self.kind
    }

    /// The track's name, which may be empty.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The track's mixer settings.
    #[must_use]
    pub fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    /// The track's delay (the mixer spec).
    #[must_use]
    pub const fn delay(&self) -> TrackDelay {
        self.delay
    }

    /// Whether a monitored track keeps its delay compensation (the
    /// delay-compensation spec, manual 18.8).
    #[must_use]
    pub const fn keep_monitoring_latency(&self) -> bool {
        self.keep_monitoring_latency
    }

    /// Whether the track's Bypass Tuning switch is on (the tunings spec,
    /// R14). A track that holds a Drum Rack bypasses whatever it says; see
    /// [`plays_tuning`](Self::plays_tuning).
    #[must_use]
    pub const fn tuning_bypass(&self) -> bool {
        self.tuning_bypass
    }

    /// The track's MIDI Controller Layout (the tunings spec, R22).
    #[must_use]
    pub const fn tuning_layout(&self) -> &crate::TuningLayout {
        &self.tuning_layout
    }

    /// The group of linked tracks the track is in, if any (the linked-track
    /// editing spec, R1).
    #[must_use]
    pub const fn link(&self) -> Option<crate::ids::LinkGroupId> {
        self.link
    }

    /// Whether the track holds a Drum Rack, at any depth, which makes it
    /// bypass the tuning (the tunings spec, R14).
    #[must_use]
    pub fn holds_drum_rack(&self) -> bool {
        self.devices_deep()
            .any(|device| device.rack_kind() == Some(crate::RackKind::Drum))
    }

    /// Whether the track plays the set's tuning (the tunings spec, R14): a
    /// MIDI track whose Bypass Tuning is off and that holds no Drum Rack.
    #[must_use]
    pub fn plays_tuning(&self) -> bool {
        self.kind == TrackKind::Midi && !self.tuning_bypass && !self.holds_drum_rack()
    }

    /// The track's sends that aren't at −∞ dB, as (return track, level) in
    /// the order of the returns' IDs. Each level is a linear gain above 0
    /// and at most 1 (0 dB). Audio, MIDI, and group tracks have a send to
    /// every return track; one that isn't listed is at −∞ dB, where every
    /// send starts.
    pub fn sends(&self) -> impl ExactSizeIterator<Item = (TrackId, f64)> + '_ {
        self.sends.iter().map(|(&to, &level)| (to, level))
    }

    /// The level of the track's send to the return track `to`, as a linear
    /// gain: 0 (−∞ dB) unless it has been turned up.
    #[must_use]
    pub fn send(&self, to: TrackId) -> f64 {
        self.sends.get(&to).copied().unwrap_or(0.0)
    }

    /// The track's In/Out settings.
    #[must_use]
    pub fn routing(&self) -> &Routing {
        &self.routing
    }

    /// Whether the track's meter is a MIDI meter: a MIDI track whose chain
    /// holds no instrument and no audio effect puts out MIDI, not audio (the
    /// MIDI meters spec, R1). A frozen track plays its freeze file.
    #[must_use]
    pub fn shows_midi_meter(&self) -> bool {
        self.kind == TrackKind::Midi
            && !self.is_frozen()
            && self
                .devices()
                .all(|device| device.role == crate::DeviceRole::MidiEffect)
    }

    /// The devices in the track's chain, in signal order.
    pub fn devices(&self) -> impl ExactSizeIterator<Item = &Device> + DoubleEndedIterator {
        self.devices.iter().map(|device| &**device)
    }

    /// Where a device sits in the chain, counting from 0. Only devices in
    /// the chain itself, not those inside its racks (see
    /// [`find_device`](Self::find_device)).
    #[must_use]
    pub fn device_position(&self, device: DeviceId) -> Option<usize> {
        self.devices.iter().position(|d| d.id == device)
    }

    /// Every device of the track, those inside its racks included at any
    /// depth, in signal order.
    pub fn devices_deep(&self) -> impl Iterator<Item = &Device> {
        self.devices.iter().flat_map(|device| device.walk())
    }

    /// The device with this ID, in the chain or inside one of its racks.
    #[must_use]
    pub fn find_device(&self, id: DeviceId) -> Option<&Device> {
        self.devices.iter().find_map(|device| device.find(id))
    }

    /// The rack chain with this ID, at any depth.
    #[must_use]
    pub fn find_chain(&self, id: ChainId) -> Option<&Chain> {
        self.devices.iter().find_map(|device| device.find_chain(id))
    }

    /// Whether the track's devices, racks and chains are laid out as
    /// another version's are (see [`Device::same_shape`]).
    #[must_use]
    pub fn same_chain_shape(&self, other: &Self) -> bool {
        self.devices.len() == other.devices.len()
            && self
                .devices
                .iter()
                .zip(&other.devices)
                .all(|(a, b)| Shared::ptr_eq(a, b) || a.same_shape(b))
    }

    /// The track's clips, ordered by ID: those on its Arrangement lane.
    /// Only audio and MIDI tracks have clips.
    pub fn clips(&self) -> impl ExactSizeIterator<Item = &Clip> + DoubleEndedIterator {
        self.clips.values().map(|clip| &**clip)
    }

    /// Every clip the track holds: its main lane's, its Session slots' and
    /// its take lanes'.
    pub fn all_clips(&self) -> impl Iterator<Item = &Clip> + '_ {
        self.clips()
            .chain(self.slots().map(|(_, clip)| clip))
            .chain(self.take_lanes().flat_map(|lane| lane.clips.iter()))
    }

    /// The Arrangement clips the track plays: its main lane's, or those of
    /// the take lane that is auditioned in their place (the comping spec,
    /// R14).
    pub fn played_clips(&self) -> Box<dyn Iterator<Item = &Clip> + '_> {
        match self.audition.and_then(|id| self.take_lane(id)) {
            Some(lane) => Box::new(lane.clips.iter()),
            None => Box::new(self.clips()),
        }
    }

    /// The clips in the track's Session slots, as `(scene, clip)` pairs
    /// ordered by scene ID. Only audio and MIDI tracks hold them.
    pub fn slots(&self) -> impl ExactSizeIterator<Item = (SceneId, &Clip)> + '_ {
        self.slots.iter().map(|(&scene, clip)| (scene, &**clip))
    }

    /// The clip in the track's slot at `scene`, if the slot holds one.
    #[must_use]
    pub fn slot(&self, scene: SceneId) -> Option<&Clip> {
        self.slots.get(&scene).map(|clip| &**clip)
    }

    /// Whether the slot at `scene` has a Clip Stop button (WP-3.2,
    /// S-SES-005): every audio and MIDI slot has one until it's removed.
    #[must_use]
    pub fn has_stop_button(&self, scene: SceneId) -> bool {
        self.kind.holds_clips() && !self.removed_stop_buttons.contains(&scene)
    }

    /// The scenes whose slots' Clip Stop buttons were removed, in order.
    pub fn removed_stop_buttons(&self) -> impl ExactSizeIterator<Item = SceneId> + '_ {
        self.removed_stop_buttons.iter().copied()
    }

    /// The clip with this ID, whether it sits on the main lane, on a take
    /// lane or in a Session slot.
    #[must_use]
    pub fn clip(&self, id: ClipId) -> Option<&Clip> {
        self.clips
            .get(&id)
            .or_else(|| self.slots.values().find(|clip| clip.id == id))
            .map(|clip| &**clip)
            .or_else(|| self.take_lanes.iter().find_map(|lane| lane.clip(id)))
    }

    /// The take lane the clip with this ID sits on, if it sits on one.
    #[must_use]
    pub fn clip_lane(&self, id: ClipId) -> Option<TakeLaneId> {
        self.take_lanes
            .iter()
            .find(|lane| lane.clip(id).is_some())
            .map(|lane| lane.id)
    }

    /// The clip with this ID, to change in place, whether it sits on the
    /// Arrangement lane or in a Session slot. The track's share of the clip
    /// is copied out of the document it came from first, so editing it
    /// leaves that version as it was.
    pub(crate) fn clip_mut(&mut self, id: ClipId) -> Option<&mut Clip> {
        if let Some(clip) = self.clips.get_mut(&id) {
            return Some(Shared::make_mut(clip));
        }
        if let Some(lane) = self
            .take_lanes
            .iter_mut()
            .find(|lane| lane.clip(id).is_some())
        {
            return lane.clips.iter_mut().find(|clip| clip.id == id);
        }
        let scene = self
            .slots
            .iter()
            .find(|(_, clip)| clip.id == id)
            .map(|(&scene, _)| scene)?;
        self.slots.get_mut(&scene).map(Shared::make_mut)
    }

    /// The take lanes under the track's main lane, nearest first.
    pub fn take_lanes(&self) -> impl ExactSizeIterator<Item = &TakeLane> + '_ {
        self.take_lanes.iter()
    }

    /// The take lane with this ID.
    #[must_use]
    pub fn take_lane(&self, id: TakeLaneId) -> Option<&TakeLane> {
        self.take_lanes.iter().find(|lane| lane.id == id)
    }

    /// The name a take lane added now gets (the comping spec, R4): `Take N`,
    /// N one more than the highest number of a lane named `Take` and a
    /// number, 1 on a track with none.
    #[must_use]
    pub fn next_take_name(&self) -> String {
        let highest = self
            .take_lanes
            .iter()
            .filter_map(|lane| lane.name.strip_prefix("Take ")?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        format!("Take {}", highest + 1)
    }

    /// The take lane that is auditioned: the track plays it in place of its
    /// main lane (the comping spec, R14).
    #[must_use]
    pub fn audition(&self) -> Option<TakeLaneId> {
        self.audition
    }

    /// The tracks directly inside this one, in order. Only groups have them.
    pub fn children(&self) -> impl ExactSizeIterator<Item = TrackId> + DoubleEndedIterator + '_ {
        self.children.iter().copied()
    }

    /// The track's automation: one entry per automated parameter, in target
    /// order.
    pub fn automation(
        &self,
    ) -> impl ExactSizeIterator<Item = (&AutomationTarget, &Automation)> + '_ {
        self.automation.iter()
    }

    /// The track's automation, keyed by the parameter it drives.
    #[must_use]
    pub fn automation_map(&self) -> &TrackAutomation {
        &self.automation
    }

    /// The automation of one parameter of the track, if it has any.
    #[must_use]
    pub fn automation_for(&self, target: &AutomationTarget) -> Option<&Automation> {
        self.automation.get(target)
    }

    /// Whether any of the track's envelopes is turned off: a control was
    /// moved by hand and Re-Enable Automation hasn't run since. A Session
    /// clip's automation envelope counts too (WP-3.12).
    #[must_use]
    pub fn automation_overridden(&self) -> bool {
        self.automation
            .values()
            .any(|automation| !automation.enabled)
            || self.slots.values().any(|clip| {
                clip.envelopes
                    .iter()
                    .any(|envelope| envelope.kind == EnvelopeKind::Automation && !envelope.enabled)
            })
    }

    /// Turns `target`'s envelope off, in the track's automation and in each
    /// Session clip's automation envelopes: what moving the control by
    /// hand does (automation R5, WP-3.12). A clip that isn't playing is
    /// turned off too, and its launch turns it on again.
    pub(crate) fn override_target(&mut self, target: &AutomationTarget) {
        crate::automation::override_target(&mut self.automation, target);
        self.set_clip_envelopes_enabled(|t| t == target, false);
    }

    /// Turns the Session clips' automation envelopes for the targets
    /// `which` accepts on or off, copying only the clips that change.
    pub(crate) fn set_clip_envelopes_enabled(
        &mut self,
        which: impl Fn(&AutomationTarget) -> bool,
        enabled: bool,
    ) {
        let changing = |envelope: &crate::clip::ClipEnvelope| {
            envelope.kind == EnvelopeKind::Automation
                && envelope.enabled != enabled
                && which(&envelope.target)
        };
        let scenes: Vec<SceneId> = self
            .slots
            .iter()
            .filter(|(_, clip)| clip.envelopes.iter().any(changing))
            .map(|(&scene, _)| scene)
            .collect();
        for scene in scenes {
            if let Some(clip) = self.slots.get_mut(&scene) {
                for envelope in &mut Shared::make_mut(clip).envelopes {
                    if changing(envelope) {
                        envelope.enabled = enabled;
                    }
                }
            }
        }
    }

    /// Whether the take lanes differ in more than the colours of their clips.
    fn take_lanes_differ(&self, other: &Self) -> bool {
        if self.take_lanes.ptr_eq(&other.take_lanes) || self.take_lanes == other.take_lanes {
            return false;
        }
        self.take_lanes.len() != other.take_lanes.len()
            || self.take_lanes.iter().zip(&other.take_lanes).any(|(a, b)| {
                a.id != b.id
                    || a.name != b.name
                    || a.clips.len() != b.clips.len()
                    || a.clips
                        .iter()
                        .zip(&b.clips)
                        .any(|(x, y)| x != y && !x.differs_only_in_colour(y))
            })
    }

    /// Whether the track's own settings differ from another version's:
    /// its name, mixer, sends, routing, children, automation, its frozen
    /// audio, or the order of its device chain, but not the clips or devices
    /// themselves.
    pub(crate) fn own_settings_differ(&self, other: &Self) -> bool {
        self.kind != other.kind
            || self.name != other.name
            || self.mixer != other.mixer
            || self.routing != other.routing
            || self.delay != other.delay
            || self.keep_monitoring_latency != other.keep_monitoring_latency
            || self.tuning_bypass != other.tuning_bypass
            || self.tuning_layout != other.tuning_layout
            || self.link != other.link
            || self.removed_stop_buttons != other.removed_stop_buttons
            || self.frozen != other.frozen
            || self.audition != other.audition
            || self.take_lanes_differ(other)
            || !(self.sends.ptr_eq(&other.sends) || self.sends == other.sends)
            || !(self.children.ptr_eq(&other.children) || self.children == other.children)
            || !(self.automation.ptr_eq(&other.automation) || self.automation == other.automation)
            || !(self.devices.ptr_eq(&other.devices) || self.same_chain_shape(other))
    }
}

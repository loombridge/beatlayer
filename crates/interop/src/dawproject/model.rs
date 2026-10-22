//! Beatlayer's own model of the DAWproject schema's elements (the DAWproject
//! spec, R1 to R8): every element the schema has, with the attributes
//! Beatlayer reads.
//!
//! The model is what the reader makes of a file. It keeps what the schema
//! allows and nothing more: an element or attribute the schema doesn't have
//! is skipped and counted (R4), and a value that doesn't read is missing
//! (R8), so what the importer sees is the file as far as it could be read.

use std::collections::BTreeMap;

/// The unit a parameter's value is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Linear,
    Normalized,
    Percent,
    Decibel,
    Hertz,
    Semitones,
    Seconds,
    Beats,
    Bpm,
}

impl Unit {
    /// The unit the schema names, or `None` for a name it doesn't have.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "linear" => Self::Linear,
            "normalized" => Self::Normalized,
            "percent" => Self::Percent,
            "decibel" => Self::Decibel,
            "hertz" => Self::Hertz,
            "semitones" => Self::Semitones,
            "seconds" => Self::Seconds,
            "beats" => Self::Beats,
            "bpm" => Self::Bpm,
            _ => return None,
        })
    }

    /// The unit's name in the schema.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::Normalized => "normalized",
            Self::Percent => "percent",
            Self::Decibel => "decibel",
            Self::Hertz => "hertz",
            Self::Semitones => "semitones",
            Self::Seconds => "seconds",
            Self::Beats => "beats",
            Self::Bpm => "bpm",
        }
    }
}

/// The unit a timeline's times are in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeUnit {
    Beats,
    Seconds,
}

impl TimeUnit {
    /// The unit the schema names, or `None` for a name it doesn't have.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "beats" => Self::Beats,
            "seconds" => Self::Seconds,
            _ => return None,
        })
    }
}

/// A channel's role in the mixer (R16).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixerRole {
    Regular,
    Master,
    Effect,
    Submix,
    Vca,
}

impl MixerRole {
    /// The role the schema names, or `None` for a name it doesn't have.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "regular" => Self::Regular,
            "master" => Self::Master,
            "effect" => Self::Effect,
            "submix" => Self::Submix,
            "vca" => Self::Vca,
            _ => return None,
        })
    }
}

/// A device's role in a chain (R44).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceRole {
    Instrument,
    NoteFx,
    AudioFx,
    Analyzer,
}

impl DeviceRole {
    /// The role the schema names, or `None` for a name it doesn't have.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "instrument" => Self::Instrument,
            "noteFX" => Self::NoteFx,
            "audioFX" => Self::AudioFx,
            "analyzer" => Self::Analyzer,
            _ => return None,
        })
    }
}

/// How a point's value runs to the next point (R58).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    Hold,
    Linear,
}

impl Interpolation {
    /// The interpolation the schema names, or `None` for a name it doesn't
    /// have.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "hold" => Self::Hold,
            "linear" => Self::Linear,
            _ => return None,
        })
    }
}

/// What an automation or expression drives (R56 to R62).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expression {
    Gain,
    Pan,
    Transpose,
    Timbre,
    Formant,
    Pressure,
    ChannelController,
    ChannelPressure,
    PolyPressure,
    PitchBend,
    ProgramChange,
}

impl Expression {
    /// The expression the schema names, or `None` for a name it doesn't have.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "gain" => Self::Gain,
            "pan" => Self::Pan,
            "transpose" => Self::Transpose,
            "timbre" => Self::Timbre,
            "formant" => Self::Formant,
            "pressure" => Self::Pressure,
            "channelController" => Self::ChannelController,
            "channelPressure" => Self::ChannelPressure,
            "polyPressure" => Self::PolyPressure,
            "pitchBend" => Self::PitchBend,
            "programChange" => Self::ProgramChange,
            _ => return None,
        })
    }
}

/// What a track holds, from its `contentType` (R15).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentKind {
    Audio,
    Automation,
    Notes,
    Video,
    Markers,
    Tracks,
}

impl ContentKind {
    /// The kind the schema names, or `None` for a name it doesn't have.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "audio" => Self::Audio,
            "automation" => Self::Automation,
            "notes" => Self::Notes,
            "video" => Self::Video,
            "markers" => Self::Markers,
            "tracks" => Self::Tracks,
            _ => return None,
        })
    }
}

/// What every timeline element carries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimelineInfo {
    /// The element's `id`, when it has one.
    pub id: Option<String>,
    /// Its `name`.
    pub name: Option<String>,
    /// Its `color`.
    pub color: Option<String>,
    /// Its `comment`.
    pub comment: Option<String>,
    /// The unit its times are in (R9).
    pub time_unit: Option<TimeUnit>,
    /// The track the timeline is local to (R24).
    pub track: Option<String>,
}

/// A real-valued parameter (the schema's `realParameter`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RealParameter {
    pub id: Option<String>,
    pub name: Option<String>,
    /// The unit the value is in (R8).
    pub unit: Option<Unit>,
    /// The value, when it reads (R8).
    pub value: Option<f64>,
    /// The lowest value the parameter can have.
    pub min: Option<f64>,
    /// The highest value the parameter can have.
    pub max: Option<f64>,
}

/// A boolean parameter (the schema's `boolParameter`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BoolParameter {
    pub id: Option<String>,
    pub name: Option<String>,
    /// The value, when it reads (R8).
    pub value: Option<bool>,
}

/// A time signature parameter (the schema's `timeSignatureParameter`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimeSignatureParameter {
    pub id: Option<String>,
    pub name: Option<String>,
    pub numerator: Option<u8>,
    pub denominator: Option<u8>,
}

/// The application's name and version (R12).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Application {
    pub name: Option<String>,
    pub version: Option<String>,
}

impl Application {
    /// The application and version, as the report names it.
    #[must_use]
    pub fn label(&self) -> Option<String> {
        match (&self.name, &self.version) {
            (Some(name), Some(version)) => Some(format!("{name} {version}")),
            (Some(name), None) => Some(name.clone()),
            (None, Some(version)) => Some(version.clone()),
            (None, None) => None,
        }
    }
}

/// The Transport's tempo and time signature (R13, R14).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transport {
    pub tempo: Option<RealParameter>,
    pub time_signature: Option<TimeSignatureParameter>,
}

/// A file the project holds, inside the container or on disk (R38).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileReference {
    /// The path, as the file spells it.
    pub path: String,
    /// Whether the path is on disk rather than in the container.
    pub external: bool,
}

/// A send from a channel to a return (R22).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Send {
    pub id: Option<String>,
    /// The channel the send feeds.
    pub destination: Option<String>,
    /// Whether the send takes its signal before or after the mixer.
    pub send_type: Option<String>,
    /// The send's switch.
    pub enable: Option<BoolParameter>,
    /// The send's pan, which Beatlayer's sends don't have (R22).
    pub pan: Option<RealParameter>,
    /// The send's level.
    pub volume: Option<RealParameter>,
}

/// A device in a channel's chain (R44 to R55).
#[derive(Clone, Debug, PartialEq)]
pub enum Device {
    /// A plug-in: VST2, VST3, CLAP or an Audio Unit.
    Plugin {
        info: TimelineInfo,
        /// The plug-in's format.
        format: PluginFormat,
        /// The plug-in's ID.
        device_id: Option<String>,
        /// The plug-in's name.
        device_name: Option<String>,
        /// The plug-in's vendor.
        device_vendor: Option<String>,
        /// The plug-in's role in the chain.
        role: Option<DeviceRole>,
        /// Whether the plug-in is loaded.
        loaded: Option<bool>,
        /// The plug-in's state file.
        state: Option<FileReference>,
        /// The parameters the plug-in's automation names.
        parameters: Vec<Parameter>,
    },
    /// A generic built-in device (R50 to R54).
    Builtin {
        info: TimelineInfo,
        /// The device's ID.
        device_id: Option<String>,
        /// The device's name.
        device_name: Option<String>,
        /// The device's vendor.
        device_vendor: Option<String>,
        /// The device's role in the chain.
        role: Option<DeviceRole>,
        /// Whether the device is loaded.
        loaded: Option<bool>,
        /// The device's state file.
        state: Option<FileReference>,
        /// The parameters the device's automation names.
        parameters: Vec<Parameter>,
    },
    /// The generic equaliser (R54).
    Equalizer {
        info: TimelineInfo,
        bands: Vec<EqBand>,
        input_gain: Option<RealParameter>,
        output_gain: Option<RealParameter>,
    },
    /// The generic compressor (R51).
    Compressor {
        info: TimelineInfo,
        attack: Option<RealParameter>,
        auto_makeup: Option<BoolParameter>,
        input_gain: Option<RealParameter>,
        output_gain: Option<RealParameter>,
        ratio: Option<RealParameter>,
        release: Option<RealParameter>,
        threshold: Option<RealParameter>,
    },
    /// The generic gate (R52).
    NoiseGate {
        info: TimelineInfo,
        attack: Option<RealParameter>,
        range: Option<RealParameter>,
        ratio: Option<RealParameter>,
        release: Option<RealParameter>,
        threshold: Option<RealParameter>,
    },
    /// The generic limiter (R53).
    Limiter {
        info: TimelineInfo,
        attack: Option<RealParameter>,
        input_gain: Option<RealParameter>,
        output_gain: Option<RealParameter>,
        release: Option<RealParameter>,
        threshold: Option<RealParameter>,
    },
}

/// A plug-in's format (R47 to R50).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginFormat {
    Vst2,
    Vst3,
    Clap,
    Au,
}

impl PluginFormat {
    /// The format's name in the schema.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Vst2 => "Vst2Plugin",
            Self::Vst3 => "Vst3Plugin",
            Self::Clap => "ClapPlugin",
            Self::Au => "AuPlugin",
        }
    }
}

/// One band of the generic equaliser (R54).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EqBand {
    /// The band's type: `highPass`, `lowPass`, `bandPass`, `highShelf`,
    /// `lowShelf`, `bell` or `notch`.
    pub band_type: Option<String>,
    /// The band's place in the chain.
    pub order: Option<i64>,
    pub frequency: Option<RealParameter>,
    pub gain: Option<RealParameter>,
    pub q: Option<RealParameter>,
    pub enabled: Option<BoolParameter>,
}

/// A channel: a mixer strip (R19 to R22).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Channel {
    pub info: TimelineInfo,
    /// The channel's role in the mixer (R16).
    pub role: Option<MixerRole>,
    /// The channel another channel routes into (R19).
    pub destination: Option<String>,
    /// How many channels the audio has; Beatlayer's tracks are stereo (R20).
    pub audio_channels: Option<i64>,
    /// Whether the channel is soloed (R21).
    pub solo: Option<bool>,
    pub mute: Option<BoolParameter>,
    pub pan: Option<RealParameter>,
    pub volume: Option<RealParameter>,
    /// The channel's sends (R22).
    pub sends: Vec<Send>,
    /// The channel's devices (R44).
    pub devices: Vec<Device>,
}

impl Channel {
    /// Whether the channel holds a device with the given role (R15).
    #[must_use]
    pub fn holds_role(&self, role: DeviceRole) -> bool {
        self.devices.iter().any(|device| match device {
            Device::Plugin { role: r, .. } | Device::Builtin { role: r, .. } => *r == Some(role),
            _ => false,
        })
    }
}

/// A track: a lane in the structure (R15).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub info: TimelineInfo,
    /// What the track holds (R15).
    pub content_types: Vec<ContentKind>,
    /// Whether the track is loaded (R20).
    pub loaded: Option<bool>,
    /// The track's channel.
    pub channel: Option<Channel>,
    /// The tracks inside a group (R15).
    pub children: Vec<Track>,
}

/// What sits directly in the structure: a track or a bare channel (R15).
#[derive(Clone, Debug, PartialEq)]
pub enum Structure {
    Track(Track),
    Channel(Channel),
}

/// A note (R28).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Note {
    pub time: Option<f64>,
    pub duration: Option<f64>,
    pub channel: Option<i64>,
    pub key: Option<i64>,
    pub vel: Option<f64>,
    pub rel: Option<f64>,
    /// The note's own expressions, which the importer leaves out (R61).
    pub content: Option<Box<Content>>,
}

/// A parameter a device holds (R55).
#[derive(Clone, Debug, PartialEq)]
pub enum Parameter {
    Real(RealParameter),
    Bool(BoolParameter),
    Integer {
        id: Option<String>,
        name: Option<String>,
        value: Option<i64>,
        min: Option<i64>,
        max: Option<i64>,
    },
    Enumerated {
        id: Option<String>,
        name: Option<String>,
        value: Option<i64>,
        count: Option<i64>,
        labels: Vec<String>,
    },
    Signature(TimeSignatureParameter),
}

/// A marker (R63).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Marker {
    pub info: TimelineInfo,
    pub time: Option<f64>,
}

/// An audio or video file (R38, R41).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MediaFile {
    pub info: TimelineInfo,
    /// The file's length in seconds.
    pub duration: Option<f64>,
    /// The file's channel count.
    pub channels: Option<i64>,
    /// The file's sample rate.
    pub sample_rate: Option<i64>,
    /// The warp algorithm the file is played with (R31).
    pub algorithm: Option<String>,
    /// The file itself.
    pub file: Option<FileReference>,
}

/// A warp event (R30).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Warp {
    /// The beat in the clip's contents the event is at.
    pub time: Option<f64>,
    /// The time in the file the event is at.
    pub content_time: Option<f64>,
}

/// A clip (R27, R34, R35).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Clip {
    pub info: TimelineInfo,
    /// Where the clip starts on its parent's timeline.
    pub time: Option<f64>,
    /// How long the clip is.
    pub duration: Option<f64>,
    /// The unit the clip's play and loop markers are in (R9).
    pub content_time_unit: Option<TimeUnit>,
    pub play_start: Option<f64>,
    pub play_stop: Option<f64>,
    pub loop_start: Option<f64>,
    pub loop_end: Option<f64>,
    /// The unit the clip's fades are in.
    pub fade_time_unit: Option<TimeUnit>,
    pub fade_in: Option<f64>,
    pub fade_out: Option<f64>,
    /// Whether the clip is switched off (R27).
    pub enable: Option<bool>,
    /// The timeline the clip plays a copy of (R35).
    pub reference: Option<String>,
    /// What the clip plays.
    pub content: Option<Box<Content>>,
}

/// What a clip plays: a timeline, or a copy of one (R34, R35, R36).
#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    /// A timeline of the file's own.
    Timeline(Box<Timeline>),
    /// A copy of another timeline (R35).
    Reference(String),
}

impl Content {
    /// The timeline the content holds, when it holds one.
    #[must_use]
    pub fn timeline(&self) -> Option<&Timeline> {
        match self {
            Self::Timeline(timeline) => Some(timeline),
            Self::Reference(_) => None,
        }
    }
}

/// A timeline: one of the schema's timeline elements (R9).
#[derive(Clone, Debug, PartialEq)]
pub enum Timeline {
    Clips(Clips),
    Notes(Notes),
    ClipSlot(ClipSlot),
    Warps(Warps),
    Audio(MediaFile),
    Video(MediaFile),
    Markers(Markers),
    Points(Points),
    Lanes(Lanes),
}

impl Timeline {
    /// What every timeline carries.
    #[must_use]
    pub fn info(&self) -> &TimelineInfo {
        match self {
            Self::Clips(t) => &t.info,
            Self::Notes(t) => &t.info,
            Self::ClipSlot(t) => &t.info,
            Self::Warps(t) => &t.info,
            Self::Audio(t) => &t.info,
            Self::Video(t) => &t.info,
            Self::Markers(t) => &t.info,
            Self::Points(t) => &t.info,
            Self::Lanes(t) => &t.info,
        }
    }
}

/// A timeline of clips (R24, R25).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Clips {
    pub info: TimelineInfo,
    pub clips: Vec<Clip>,
}

/// A timeline of notes (R28).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Notes {
    pub info: TimelineInfo,
    pub notes: Vec<Note>,
}

/// A slot in the Session View (R42).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipSlot {
    pub info: TimelineInfo,
    /// Whether the slot has a Stop button (R42).
    pub has_stop: Option<bool>,
    pub clip: Option<Box<Clip>>,
}

/// A timeline of warp events around a media file (R30).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Warps {
    pub info: TimelineInfo,
    /// The unit the content's times are in (R30).
    pub content_time_unit: Option<TimeUnit>,
    /// The file the warps play.
    pub audio: Option<MediaFile>,
    /// The warp events.
    pub warps: Vec<Warp>,
}

/// A timeline of markers (R63).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Markers {
    pub info: TimelineInfo,
    pub markers: Vec<Marker>,
}

/// What a set of points drives (R56 to R62).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Target {
    /// The parameter the points drive.
    pub parameter: Option<String>,
    /// The expression the points drive, when they drive an expression.
    pub expression: Option<Expression>,
    /// The MIDI channel, for a channel expression.
    pub channel: Option<i64>,
    /// The MIDI key, for a polyphonic expression.
    pub key: Option<i64>,
    /// The controller number, for a channel controller.
    pub controller: Option<i64>,
}

/// One automation point's value (R58).
#[derive(Clone, Debug, PartialEq)]
pub enum PointValue {
    /// A real point, with how it runs to the next.
    Real {
        time: Option<f64>,
        value: Option<f64>,
        interpolation: Option<Interpolation>,
    },
    /// A boolean point.
    Bool {
        time: Option<f64>,
        value: Option<bool>,
    },
    /// An integer point.
    Integer {
        time: Option<f64>,
        value: Option<i64>,
    },
    /// An enumerated point.
    Enumerated {
        time: Option<f64>,
        value: Option<i64>,
    },
    /// A time signature point.
    Signature {
        time: Option<f64>,
        numerator: Option<i64>,
        denominator: Option<i64>,
    },
}

impl PointValue {
    /// The point's time.
    #[must_use]
    pub const fn time(&self) -> Option<f64> {
        match self {
            Self::Real { time, .. }
            | Self::Bool { time, .. }
            | Self::Integer { time, .. }
            | Self::Enumerated { time, .. }
            | Self::Signature { time, .. } => *time,
        }
    }
}

/// A timeline of automation points (R56 to R62).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Points {
    pub info: TimelineInfo,
    /// The unit the points' values are in (R57).
    pub unit: Option<Unit>,
    /// What the points drive.
    pub target: Target,
    /// The points, in the file's order.
    pub points: Vec<PointValue>,
}

/// A set of lanes (R24).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lanes {
    pub info: TimelineInfo,
    /// What the lanes hold, in the file's order.
    pub children: Vec<Timeline>,
}

/// The Arrangement: the tracks' lanes, the markers and the tempo (R13, R24).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Arrangement {
    pub info: TimelineInfo,
    /// The Arrangement's lanes, at any depth.
    pub lanes: Vec<Timeline>,
    /// The tempo automation (R13).
    pub tempo_automation: Option<Points>,
    /// The time-signature automation (R14).
    pub signature_automation: Option<Points>,
}

/// A scene of the Session View (R42).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub info: TimelineInfo,
    /// What the scene holds.
    pub content: Option<Box<Content>>,
}

/// The whole project (R3).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Project {
    /// The format's version (R3).
    pub version: Option<String>,
    /// The application that made the file (R12).
    pub application: Option<Application>,
    /// The Transport's tempo and signature.
    pub transport: Option<Transport>,
    /// The tracks and channels, in the file's order.
    pub structure: Vec<Structure>,
    /// The Arrangement.
    pub arrangement: Option<Arrangement>,
    /// The Session View's scenes.
    pub scenes: Vec<Scene>,
}

impl Project {
    /// The version's major number (R3).
    #[must_use]
    pub fn major_version(&self) -> Option<u32> {
        self.version
            .as_deref()
            .and_then(|version| version.split('.').next())
            .and_then(|major| major.trim().parse().ok())
    }
}

/// Every element in a file, by its `id` (R7).
#[derive(Clone, Debug, Default)]
pub struct Ids {
    /// The element each `id` names.
    pub by_id: BTreeMap<String, Element>,
}

/// What an `id` names (R7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Element {
    Track,
    Channel,
    Clip,
    Timeline,
    Note,
    Scene,
    Device,
    Parameter,
    Other,
}

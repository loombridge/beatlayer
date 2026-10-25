//! Writing a set as a DAWproject file (the DAWproject spec, R65 to R82): the
//! tracks, mixer and routing, the clips, the media, the Session View's
//! scenes, the tempo and time signature, the automation, the devices and
//! plug-ins with their states, the locators, and the report of what the
//! format can't carry.
//!
//! The export is pure: it reads the document and the files the plug-ins and
//! the media sit in, and makes the file's bytes. The shell takes the
//! plug-ins' states and runs it on a worker thread.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use bl_core::time::{Bpm, Ticks};
use bl_engine::DeviceType;
use bl_model::{
    AutomationTarget, ContentHash, Document, MixerParam, Output, ParamDescriptor, PluginFormat,
    SendTap, Track, TrackId, TrackKind, swatch_rgb,
};

use super::container::{Data, Entry, Error, METADATA_PART, PROJECT_PART, write_container};
use super::model::{
    Application, BoolParameter, Channel, ContentKind, Project, RealParameter, Send, Structure,
    TimeSignatureParameter, TimelineInfo, Unit,
};
use super::plugins::Plugins;
use super::report::Report;
use super::write;

mod automation;
mod clips;
mod devices;

/// The format version a file is written in (R67).
const VERSION: &str = "1.0";

/// Where the files a set's media are, which the export embeds or refers to
/// (R73).
pub trait MediaFiles: Sync {
    /// The file with this hash, where it is on disk, or `None` when it is
    /// missing.
    fn path(&self, hash: ContentHash) -> Option<PathBuf>;
}

/// What an export needs beyond the document.
pub struct ExportContext<'a> {
    /// Beatlayer's version, which the file names (R67).
    pub application_version: &'a str,
    /// The set's name, which `metadata.xml` holds as its title (R79).
    pub title: &'a str,
    /// The 14 track colours of the theme in use, from which the palette is
    /// drawn (R23).
    pub track_colours: &'a [[u8; 3]],
    /// Whether the media are embedded; if not, each is referred to by its
    /// absolute path (R73).
    pub embed_media: bool,
    /// Where the media are.
    pub media: &'a dyn MediaFiles,
    /// The plug-ins installed on this machine, for their vendors and
    /// versions and the parameters their automation names (R76, R77).
    pub plugins: &'a dyn Plugins,
}

/// What an export made.
#[derive(Debug)]
pub struct Exported {
    /// The file's bytes.
    pub bytes: Vec<u8>,
    /// What the format can't carry, which the file leaves out (R80).
    pub report: Report,
}

/// Writes the set `document` holds as a DAWproject file to `out`, giving
/// `out` back with the report. The media are copied in as the file is
/// written, not held in memory.
///
/// # Errors
///
/// [`Error::Failed`] if the archive can't be written or a medium can't be
/// read.
pub fn export_to<W: std::io::Write + std::io::Seek>(
    document: &Document,
    context: &ExportContext<'_>,
    out: W,
) -> Result<(W, Report), Error> {
    let mut exporter = Exporter {
        document,
        context,
        report: Report::default(),
        ids: Ids::default(),
        states: Vec::new(),
        audio: Vec::new(),
        taken: BTreeSet::new(),
        media: HashMap::new(),
        types: bl_devices::device_types(),
        described: HashMap::new(),
        conversions: BTreeMap::new(),
        dropped: BTreeSet::new(),
        tempo: document.tempo_map(),
        signatures: document.settings().time_signatures.clone(),
    };
    exporter.report.application = Some(format!("Beatlayer {}", context.application_version));
    let project = exporter.project();
    let mut entries = vec![
        Entry {
            name: METADATA_PART.to_owned(),
            data: Data::Bytes(write::metadata_xml(context.title).into_bytes()),
            deflate: true,
        },
        Entry {
            name: PROJECT_PART.to_owned(),
            data: Data::Bytes(write::project_xml(&project).into_bytes()),
            deflate: true,
        },
    ];
    entries.append(&mut exporter.states);
    entries.append(&mut exporter.audio);
    Ok((write_container(out, &entries)?, exporter.report))
}

/// Writes the set `document` holds as a DAWproject file, in memory.
///
/// # Errors
///
/// As [`export_to`].
pub fn export(document: &Document, context: &ExportContext<'_>) -> Result<Exported, Error> {
    let (bytes, report) = export_to(document, context, std::io::Cursor::new(Vec::new()))?;
    Ok(Exported {
        bytes: bytes.into_inner(),
        report,
    })
}

/// The `id`s the file's referenced elements have, numbered in the order the
/// elements are written (R66).
#[derive(Default)]
struct Ids {
    next: usize,
    tempo: String,
    signature: String,
    tracks: BTreeMap<TrackId, String>,
    channels: BTreeMap<TrackId, String>,
    parameters: BTreeMap<(TrackId, AutomationTarget), String>,
}

impl Ids {
    fn fresh(&mut self) -> String {
        let id = format!("id{}", self.next);
        self.next += 1;
        id
    }
}

/// A medium the file refers to or holds.
#[derive(Clone)]
struct Medium {
    path: String,
    external: bool,
    /// Its length in seconds.
    seconds: f64,
    channels: i64,
    sample_rate: i64,
}

/// A plug-in's parameters, or why it has none.
type Described = Option<Arc<Vec<ParamDescriptor>>>;

struct Exporter<'a> {
    document: &'a Document,
    context: &'a ExportContext<'a>,
    report: Report,
    ids: Ids,
    /// The plug-ins' states and Beatlayer's presets, to write after the XML.
    states: Vec<Entry>,
    /// The media, to write after them.
    audio: Vec<Entry>,
    /// The names the entries already have.
    taken: BTreeSet<String>,
    /// Each medium the set uses, found once.
    media: HashMap<ContentHash, Option<Medium>>,
    /// The device types Beatlayer ships.
    types: Vec<DeviceType>,
    /// The plug-ins loaded for their parameters, by format and ID.
    described: HashMap<(PluginFormat, String), Described>,
    /// How each automated parameter's value is written (R76).
    conversions: BTreeMap<(TrackId, AutomationTarget), automation::Conversion>,
    /// The automated parameters the file can't carry, already listed.
    dropped: BTreeSet<(TrackId, AutomationTarget)>,
    tempo: bl_core::time::TempoMap,
    signatures: bl_core::time::TimeSignatureMap,
}

impl Exporter<'_> {
    fn project(&mut self) -> Project {
        self.allocate();
        let structure = self.structure();
        let arrangement = self.arrangement();
        let scenes = self.scenes();
        Project {
            version: Some(VERSION.to_owned()),
            application: Some(Application {
                name: Some("Beatlayer".to_owned()),
                version: Some(self.context.application_version.to_owned()),
            }),
            transport: Some(self.transport()),
            structure,
            arrangement,
            scenes,
        }
    }

    /// The tracks in the order they are written: the top level with each
    /// group's tracks after it, then the returns, then Main (R67).
    fn order(&self) -> Vec<TrackId> {
        fn walk(document: &Document, id: TrackId, out: &mut Vec<TrackId>) {
            out.push(id);
            if let Some(track) = document.track(id) {
                for child in track.children() {
                    walk(document, child, out);
                }
            }
        }
        let mut out = Vec::new();
        for id in self.document.top_level() {
            walk(self.document, id, &mut out);
        }
        out.extend(self.document.returns());
        out.push(self.document.main_id());
        out
    }

    /// The `id`s of the Transport's parameters, the tracks and the channels,
    /// which other elements refer to.
    fn allocate(&mut self) {
        self.ids.tempo = self.ids.fresh();
        self.ids.signature = self.ids.fresh();
        for id in self.order() {
            let track = self.ids.fresh();
            let channel = self.ids.fresh();
            self.ids.tracks.insert(id, track);
            self.ids.channels.insert(id, channel);
        }
    }

    /// A swatch as the `#rrggbb` the theme in use draws it (R23).
    fn colour(&self, swatch: u8) -> String {
        let [r, g, b] = swatch_rgb(self.context.track_colours, swatch);
        format!("#{r:02x}{g:02x}{b:02x}")
    }

    /// Where a track is, for the report (R81).
    fn place(track: &Track) -> String {
        format!("the track \"{}\"", track.name())
    }

    /// The Transport's tempo and time signature at 1.1.1 (R75).
    fn transport(&self) -> super::model::Transport {
        let tempo = self.tempo.bpm_at(Ticks::ZERO).get();
        let signature = self.signatures.signature_at(Ticks::ZERO);
        super::model::Transport {
            tempo: Some(RealParameter {
                id: Some(self.ids.tempo.clone()),
                unit: Some(Unit::Bpm),
                value: Some(tempo),
                min: Some(Bpm::MIN.get()),
                max: Some(Bpm::MAX.get()),
                ..RealParameter::default()
            }),
            time_signature: Some(TimeSignatureParameter {
                id: Some(self.ids.signature.clone()),
                numerator: Some(signature.numerator()),
                denominator: Some(signature.denominator()),
                ..TimeSignatureParameter::default()
            }),
        }
    }

    /// The tracks, groups and returns, then Main (R67).
    fn structure(&mut self) -> Vec<Structure> {
        let document = self.document;
        let mut out = Vec::new();
        for id in document.top_level().chain(document.returns()) {
            out.push(Structure::Track(self.track(id)));
        }
        out.push(Structure::Track(self.track(document.main_id())));
        out
    }

    fn track(&mut self, id: TrackId) -> super::model::Track {
        let Some(track) = self.document.track(id) else {
            return super::model::Track::default();
        };
        let kind = match track.kind() {
            TrackKind::Midi => ContentKind::Notes,
            TrackKind::Group => ContentKind::Tracks,
            _ => ContentKind::Audio,
        };
        let channel = self.channel(track);
        let children: Vec<TrackId> = track.children().collect();
        super::model::Track {
            info: TimelineInfo {
                id: self.ids.tracks.get(&id).cloned(),
                name: Some(track.name().to_owned()),
                color: Some(self.colour(track.colour())),
                ..TimelineInfo::default()
            },
            content_types: vec![kind],
            loaded: None,
            channel: Some(channel),
            children: children
                .into_iter()
                .map(|child| self.track(child))
                .collect(),
        }
    }

    /// The automated parameters of a track, and those its Session clips'
    /// automation envelopes drive (R76).
    fn wanted(track: &Track) -> BTreeSet<AutomationTarget> {
        let mut wanted: BTreeSet<AutomationTarget> =
            track.automation().map(|(target, _)| *target).collect();
        for (_, clip) in track.slots() {
            wanted.extend(
                clip.envelopes
                    .iter()
                    .filter(|held| held.kind == bl_model::EnvelopeKind::Automation)
                    .map(|held| held.target),
            );
        }
        wanted
    }

    /// The `id` of a parameter of a track the file's points name, made on
    /// first use, or `None` when nothing automates it.
    fn parameter(
        &mut self,
        track: TrackId,
        target: AutomationTarget,
        wanted: &BTreeSet<AutomationTarget>,
        conversion: automation::Conversion,
    ) -> Option<String> {
        if !wanted.contains(&target) {
            return None;
        }
        let key = (track, target);
        if let Some(id) = self.ids.parameters.get(&key) {
            return Some(id.clone());
        }
        let id = self.ids.fresh();
        self.ids.parameters.insert(key, id.clone());
        self.conversions.insert(key, conversion);
        Some(id)
    }

    /// A track's channel: its mixer, sends, routing and devices (R68, R69).
    fn channel(&mut self, track: &Track) -> Channel {
        let id = track.id();
        let wanted = Self::wanted(track);
        let devices = self.devices(track, &wanted);
        let mixer = track.mixer();
        let parameter =
            |this: &mut Self, target, conversion| this.parameter(id, target, &wanted, conversion);
        let mute = BoolParameter {
            id: parameter(
                self,
                AutomationTarget::Mixer {
                    param: MixerParam::Mute,
                },
                automation::Conversion::Mute,
            ),
            value: Some(mixer.mute),
            ..BoolParameter::default()
        };
        let pan = RealParameter {
            id: parameter(
                self,
                AutomationTarget::Mixer {
                    param: MixerParam::Pan,
                },
                automation::Conversion::Pan,
            ),
            unit: Some(Unit::Normalized),
            min: Some(0.0),
            max: Some(1.0),
            value: Some((mixer.pan + 1.0) / 2.0),
            ..RealParameter::default()
        };
        let mut sends = Vec::new();
        for (to, level) in track.sends() {
            let (Some(destination), Some(target)) =
                (self.ids.channels.get(&to).cloned(), self.document.track(to))
            else {
                continue;
            };
            let tap = if target.mixer().tap == SendTap::Pre {
                "pre"
            } else {
                "post"
            };
            let volume = RealParameter {
                id: parameter(
                    self,
                    AutomationTarget::Send { to },
                    automation::Conversion::Send,
                ),
                unit: Some(Unit::Linear),
                min: Some(0.0),
                max: Some(1.0),
                value: Some(level),
                ..RealParameter::default()
            };
            sends.push(Send {
                id: None,
                destination: Some(destination),
                send_type: Some(tap.to_owned()),
                enable: Some(BoolParameter {
                    value: Some(true),
                    ..BoolParameter::default()
                }),
                pan: None,
                volume: Some(volume),
            });
        }
        let volume = RealParameter {
            id: parameter(
                self,
                AutomationTarget::Mixer {
                    param: MixerParam::Volume,
                },
                automation::Conversion::Volume,
            ),
            unit: Some(Unit::Linear),
            min: Some(0.0),
            max: Some(bl_model::fader::db_gain(bl_model::fader::TOP_DB)),
            value: Some(mixer.volume),
            ..RealParameter::default()
        };
        self.unwritten(track);
        Channel {
            info: TimelineInfo {
                id: self.ids.channels.get(&id).cloned(),
                ..TimelineInfo::default()
            },
            role: Some(match track.kind() {
                TrackKind::Return => super::model::MixerRole::Effect,
                // A group with no tracks in it would read back as an audio
                // track if it were a submix (R16).
                TrackKind::Group if track.children().next().is_some() => {
                    super::model::MixerRole::Submix
                }
                TrackKind::Main => super::model::MixerRole::Master,
                _ => super::model::MixerRole::Regular,
            }),
            destination: self.destination(track),
            audio_channels: Some(2),
            solo: Some(mixer.solo),
            mute: Some(mute),
            pan: Some(pan),
            volume: Some(volume),
            sends,
            devices,
        }
    }

    /// The channel a track's output feeds: its group's, Main's, or the
    /// track whose Track In it feeds (R68).
    fn destination(&mut self, track: &Track) -> Option<String> {
        if track.kind() == TrackKind::Main {
            return None;
        }
        let document = self.document;
        let fallback = document
            .parent(track.id())
            .unwrap_or_else(|| document.main_id());
        let target = match track.routing().output {
            Output::Default => fallback,
            Output::Main => document.main_id(),
            Output::Track { track: other } if document.track(other).is_some() => other,
            Output::Track { .. } => fallback,
            Output::SendsOnly | Output::External { .. } => {
                self.report.add(
                    "an output to Sends Only or Ext. Out",
                    &Self::place(track),
                    "not written; the file routes the track to its default output",
                );
                fallback
            }
        };
        self.ids.channels.get(&target).cloned()
    }

    /// What of a track the format can't carry, in the report (R80).
    fn unwritten(&mut self, track: &Track) {
        let place = Self::place(track);
        let fresh = bl_model::Routing::new(track.kind());
        let routing = track.routing();
        let mixer = track.mixer();
        let mut note = |what: &str, held: bool| {
            if held {
                self.report.add(what, &place, "not written");
            }
        };
        note("the crossfader assignment", !mixer.crossfade.is_none());
        note("the track delay", !track.delay().is_default());
        note(
            "the input",
            routing.input != fresh.input || routing.channel.is_some(),
        );
        note("the monitoring", routing.monitor != fresh.monitor);
        note("the Arm switch", mixer.arm);
        note("MIDI To", routing.midi_output.is_some());
    }
}

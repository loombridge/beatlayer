//! Turning a DAWproject file into a set (the DAWproject spec, R9 to R43):
//! the tracks, mixer and routing, the clips, the media, the Session View's
//! scenes, the tempo and time signature, and the report of what was left out
//! or changed.
//!
//! The import is pure: it reads the file's model and makes a set, with no
//! shell and no window. What the file's media holds is the shell's business.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bl_core::time::{
    Bpm, TempoMap, TempoSegment, Ticks, TimeSignature, TimeSignatureChange, TimeSignatureMap,
};
use bl_model::{
    AudioData, Clip, ClipData, ContentHash, Editor, MidiData, Monitor, Note, Output, TrackId,
    TrackKind, WarpMarker, WarpMode,
};
use bl_project::{Media, MediaPath, MediaRef, Project};

use super::container::{Container, Error};
use super::model::{
    self, Channel, Content, ContentKind, Device, DeviceRole, Interpolation, MediaFile, MixerRole,
    PointValue, Project as DawProject, TimeUnit, Timeline, Unit,
};
use super::report::Report;

/// What an import needs beyond the file itself.
pub struct Context<'a> {
    /// The folder that holds the `.dawproject` file, where the files it
    /// refers to are looked for (R40).
    pub folder: &'a Path,
    /// The folder the container's files are written into (R39).
    pub extract_to: &'a Path,
}

/// What an import made.
#[derive(Debug)]
pub struct Imported {
    /// The new set: its document, its IDs and its media.
    pub project: Project,
    /// The files the container holds that the set uses, to write out.
    pub files: Vec<Extracted>,
    /// What the import left out or changed.
    pub report: Report,
}

/// A file the set uses: one the container holds, or one the file refers to.
#[derive(Clone, Debug)]
pub struct Extracted {
    /// Its path in the container, with `/` between the parts.
    pub path: String,
    /// Its bytes, for a file the container holds.
    pub bytes: Option<Vec<u8>>,
    /// Where it is on disk, for a file the file refers to (R40).
    pub on_disk: Option<PathBuf>,
    /// The hash of its bytes.
    pub hash: ContentHash,
    /// Its size in bytes.
    pub size: u64,
}

/// Turns a DAWproject file into a set.
///
/// # Errors
///
/// [`Error`] if the file's version isn't one Beatlayer reads, or the set
/// can't be built.
pub fn import(container: &Container, context: &Context<'_>) -> Result<Imported, Error> {
    let mut importer = Importer {
        container,
        context,
        editor: Editor::new(),
        report: Report::default(),
        channels: HashMap::new(),
        returns: HashMap::new(),
        tracks: HashMap::new(),
        timelines: HashMap::new(),
        media: Media::new(),
        files: Vec::new(),
        resolved: HashMap::new(),
        made: Vec::new(),
        audio_siblings: Vec::new(),
        master_seen: false,
        track_number: 0,
        return_number: 0,
        tempo: TempoMap::default(),
        signatures: TimeSignatureMap::default(),
        failed: None,
    };
    importer.run()
}

/// A time-signature point of the file: its time, its numerator and its
/// denominator, and the unit its time is in.
type SignaturePoint = (Option<f64>, Option<i64>, Option<i64>, TimeUnit);

/// A track the import is making.
#[derive(Clone, Debug)]
struct Made {
    /// The Beatlayer track.
    id: TrackId,
    /// Its kind.
    kind: TrackKind,
    /// The name it shows.
    name: String,
    /// The DAWproject channel it holds, when it has one.
    channel: Option<Channel>,
}

/// The import itself.
struct Importer<'a, 'c> {
    container: &'a Container,
    context: &'c Context<'c>,
    editor: Editor,
    report: Report,
    /// The DAWproject channel id → the Beatlayer track.
    channels: HashMap<String, TrackId>,
    /// The DAWproject channel id → the return track it names.
    returns: HashMap<String, TrackId>,
    /// The DAWproject track id → the Beatlayer track.
    tracks: HashMap<String, TrackId>,
    /// The DAWproject timeline id → the timeline it names, for an alias
    /// clip (R35).
    timelines: HashMap<String, Timeline>,
    /// The set's media.
    media: Media,
    /// The container's files the set uses, to write out.
    files: Vec<Extracted>,
    /// The DAWproject file path → the media it names.
    resolved: HashMap<String, Option<ContentHash>>,
    /// The tracks the import made, in the order it made them.
    made: Vec<Made>,
    /// The MIDI track and the audio track beside it, for a hybrid track
    /// (R17).
    audio_siblings: Vec<(TrackId, TrackId)>,
    /// Whether a master channel has been read (R18).
    master_seen: bool,
    /// How many tracks have been made, for their default names (R20).
    track_number: usize,
    /// How many returns have been made, for their default names (R20).
    return_number: usize,
    /// The song's tempo map (R13).
    tempo: TempoMap,
    /// The song's time-signature map (R14).
    signatures: TimeSignatureMap,
    /// The first thing that went wrong.
    failed: Option<Error>,
}

impl Importer<'_, '_> {
    /// Reads the file and makes the set.
    fn run(&mut self) -> Result<Imported, Error> {
        let project = self.container.project.clone();
        self.report.application = project.application.as_ref().and_then(|a| a.label());
        self.read_report();
        self.check_version(&project)?;
        self.collect_timelines(&project);
        self.build_tempo(&project);
        self.build_signatures(&project);
        self.apply(bl_model::Command::SetTempo {
            tempo: self.tempo.clone(),
        });
        self.apply(bl_model::Command::SetTimeSignatures {
            time_signatures: self.signatures.clone(),
        });
        self.build_tracks(&project);
        self.build_clips(&project);
        self.build_scenes(&project);
        if let Some(error) = self.failed.take() {
            return Err(error);
        }
        let project = Project {
            document: self.editor.document().clone(),
            ids: self.editor.ids(),
            media: self.media.clone(),
            windows: bl_model::DeviceWindows::new(),
            rack_views: bl_model::RackViewMap::new(),
            folds: bl_model::ClipFolds::new(),
            launch: bl_model::LaunchSettings::default(),
            multisamplers: bl_model::MultisamplerViews::new(),
        };
        Ok(Imported {
            project,
            files: std::mem::take(&mut self.files),
            report: std::mem::take(&mut self.report),
        })
    }

    /// What the reader found it couldn't use, in the report (R4, R6, R7).
    fn read_report(&mut self) {
        let report = self.container.report.clone();
        for (name, count) in &report.unknown_elements {
            self.report.add_count(
                &format!("the element `{name}`"),
                "in the project",
                "left out",
                *count,
            );
        }
        for (name, count) in &report.unknown_attributes {
            self.report.add_count(
                &format!("the attribute `{name}`"),
                "in the project",
                "left out",
                *count,
            );
        }
        for (name, count) in &report.unknown_entities {
            self.report.add_count(
                &format!("the entity `&{name};`"),
                "in the project",
                "not expanded",
                *count,
            );
        }
        for name in &report.duplicate_ids {
            self.report.add(
                &format!("a repeated `id` (`{name}`)"),
                "in the project",
                "left out",
            );
        }
        for name in &report.too_deep {
            self.report.add(
                &format!("the element `{name}`, nested deeper than 256"),
                "in the project",
                "left out",
            );
        }
        for name in &report.left_out {
            self.report.add(name, "in the container", "left out");
        }
    }

    /// The file's version (R3).
    fn check_version(&mut self, project: &DawProject) -> Result<(), Error> {
        let version = project.version.clone().unwrap_or_default();
        match project.major_version() {
            Some(1) => Ok(()),
            _ => Err(Error::Version { version }),
        }
    }

    /// The timelines the file names by their `id`, for an alias clip (R35).
    fn collect_timelines(&mut self, project: &DawProject) {
        let mut found: Vec<(String, Timeline)> = Vec::new();
        if let Some(arrangement) = &project.arrangement {
            for lane in &arrangement.lanes {
                collect_timelines(lane, &mut found);
            }
        }
        for scene in &project.scenes {
            if let Some(content) = &scene.content
                && let Content::Timeline(timeline) = content.as_ref()
            {
                collect_timelines(timeline, &mut found);
            }
        }
        for (id, timeline) in found {
            self.timelines.entry(id).or_insert(timeline);
        }
    }

    /// The song's tempo (R13).
    fn build_tempo(&mut self, project: &DawProject) {
        let transport = project
            .transport
            .as_ref()
            .and_then(|transport| transport.tempo.as_ref())
            .and_then(|tempo| tempo.value)
            .unwrap_or(Bpm::DEFAULT.get());
        let transport = self.clamp_bpm(transport, "the Transport's tempo");
        let transport_id = project
            .transport
            .as_ref()
            .and_then(|transport| transport.tempo.as_ref())
            .and_then(|tempo| tempo.id.clone());

        let points = self.tempo_points(project, transport_id.as_deref());
        if points.is_empty() {
            self.tempo = TempoMap::constant(Bpm::new(transport).unwrap_or(Bpm::DEFAULT));
            return;
        }
        // Each point's place: a point in beats is its own ticks, and a point
        // in seconds is placed through the map the points before it make.
        let mut placed: Vec<(Ticks, f64, Interpolation)> = Vec::new();
        for (time, bpm, interpolation, unit) in &points {
            let Some(time) = *time else { continue };
            let at = match unit {
                TimeUnit::Seconds => {
                    let partial = build_tempo_map(&placed);
                    self.seconds_to_ticks(partial, time)
                }
                TimeUnit::Beats => beats_to_ticks(time),
            };
            let bpm = self.clamp_bpm(*bpm, "a tempo point");
            placed.push((at, bpm, *interpolation));
        }
        // The points in the order the map holds them: by their place, and
        // of two at one tick the later in the file wins.
        placed.sort_by_key(|point| point.0);
        let mut unique: Vec<(Ticks, f64, Interpolation)> = Vec::new();
        for point in placed {
            if let Some(last) = unique.last_mut()
                && last.0 == point.0
            {
                *last = point;
            } else {
                unique.push(point);
            }
        }
        self.tempo = build_tempo_map(&unique);
    }

    /// The tempo points of the Arrangement (R13).
    fn tempo_points(
        &self,
        project: &DawProject,
        transport_id: Option<&str>,
    ) -> Vec<(Option<f64>, f64, Interpolation, TimeUnit)> {
        let mut points = Vec::new();
        let Some(arrangement) = &project.arrangement else {
            return points;
        };
        if let Some(automation) = &arrangement.tempo_automation {
            let unit = automation.info.time_unit.unwrap_or(TimeUnit::Beats);
            let value_unit = automation.unit.unwrap_or(Unit::Bpm);
            for point in &automation.points {
                if let PointValue::Real {
                    time,
                    value,
                    interpolation,
                } = point
                {
                    points.push((
                        *time,
                        value.unwrap_or(Bpm::DEFAULT.get()),
                        interpolation.unwrap_or(Interpolation::Hold),
                        unit,
                    ));
                    let _ = value_unit;
                }
            }
        }
        if let Some(arrangement) = &project.arrangement {
            let mut lanes = Vec::new();
            for lane in &arrangement.lanes {
                collect_points(lane, transport_id, TimeUnit::Beats, &mut lanes);
            }
            for (points_timeline, unit) in lanes {
                let value_unit = points_timeline.unit.unwrap_or(Unit::Bpm);
                for point in &points_timeline.points {
                    if let PointValue::Real {
                        time,
                        value,
                        interpolation,
                    } = point
                    {
                        let _ = value_unit;
                        points.push((
                            *time,
                            value.unwrap_or(Bpm::DEFAULT.get()),
                            interpolation.unwrap_or(Interpolation::Hold),
                            unit,
                        ));
                    }
                }
            }
        }
        points
    }

    /// A tempo in the range the song holds, or the nearest one in it (R13).
    fn clamp_bpm(&mut self, bpm: f64, what: &str) -> f64 {
        if !bpm.is_finite() {
            return Bpm::DEFAULT.get();
        }
        if bpm < Bpm::MIN.get() || bpm > Bpm::MAX.get() {
            let clamped = bpm.clamp(Bpm::MIN.get(), Bpm::MAX.get());
            self.report.add(
                what,
                "in the song's tempo",
                &format!("changed to {clamped} BPM"),
            );
            return clamped;
        }
        bpm
    }

    /// The song's time signature (R14).
    fn build_signatures(&mut self, project: &DawProject) {
        let transport = project
            .transport
            .as_ref()
            .and_then(|transport| transport.time_signature.as_ref());
        let base = transport
            .and_then(|signature| self.signature(signature.numerator, signature.denominator))
            .unwrap_or(TimeSignature::FOUR_FOUR);
        let transport_id = transport.and_then(|signature| signature.id.clone());

        let mut changes: Vec<TimeSignatureChange> = vec![TimeSignatureChange {
            at: Ticks::ZERO,
            signature: base,
        }];
        let mut points: Vec<SignaturePoint> = Vec::new();
        if let Some(arrangement) = &project.arrangement {
            if let Some(automation) = &arrangement.signature_automation {
                let unit = automation.info.time_unit.unwrap_or(TimeUnit::Beats);
                for point in &automation.points {
                    if let PointValue::Signature {
                        time,
                        numerator,
                        denominator,
                    } = point
                    {
                        points.push((*time, *numerator, *denominator, unit));
                    }
                }
            }
            let mut lanes = Vec::new();
            for lane in &arrangement.lanes {
                collect_points(lane, transport_id.as_deref(), TimeUnit::Beats, &mut lanes);
            }
            for (points_timeline, unit) in lanes {
                for point in &points_timeline.points {
                    if let PointValue::Signature {
                        time,
                        numerator,
                        denominator,
                    } = point
                    {
                        points.push((*time, *numerator, *denominator, unit));
                    }
                }
            }
        }
        let mut placed: Vec<(Ticks, TimeSignature)> = Vec::new();
        for (time, numerator, denominator, unit) in points {
            let Some(time) = time else { continue };
            let Some(signature) = self.signature(
                numerator.and_then(|n| u8::try_from(n).ok()),
                denominator.and_then(|d| u8::try_from(d).ok()),
            ) else {
                self.report.add(
                    "a time signature Beatlayer can't hold",
                    "in the song's time signature",
                    "left out",
                );
                continue;
            };
            let at = match unit {
                TimeUnit::Seconds => self.seconds_to_ticks(self.tempo.clone(), time),
                TimeUnit::Beats => beats_to_ticks(time),
            };
            if let Some(last) = placed.last_mut()
                && last.0 == at
            {
                *last = (at, signature);
            } else {
                placed.push((at, signature));
            }
        }
        for (at, signature) in placed {
            if at == Ticks::ZERO {
                changes[0].signature = signature;
            } else {
                changes.push(TimeSignatureChange { at, signature });
            }
        }
        self.signatures = TimeSignatureMap::new(changes).unwrap_or_default();
    }

    /// A signature Beatlayer holds, or `None` (R14).
    fn signature(&self, numerator: Option<u8>, denominator: Option<u8>) -> Option<TimeSignature> {
        TimeSignature::new(numerator?, denominator?).ok()
    }

    /// The tracks, the mixer and the routing (R15 to R22).
    fn build_tracks(&mut self, project: &DawProject) {
        for item in &project.structure {
            match item {
                model::Structure::Track(track) => self.make_track(track, None),
                model::Structure::Channel(channel) => {
                    self.make_bare_channel(channel, None);
                }
            }
        }
        self.apply_mixer();
    }

    /// A track in the structure (R15 to R20).
    fn make_track(&mut self, track: &model::Track, parent: Option<TrackId>) {
        let channel = track.channel.clone();
        let role = channel
            .as_ref()
            .and_then(|c| c.role)
            .unwrap_or(MixerRole::Regular);
        let name = track
            .info
            .name
            .clone()
            .or_else(|| channel.as_ref().and_then(|c| c.info.name.clone()));
        let place = self.track_place(track, &name);

        if role == MixerRole::Master {
            if self.master_seen {
                // A second master channel is read as a submix one (R18).
                self.report
                    .add("a second master channel", &place, "read as a group channel");
                self.make_track_kind(
                    track,
                    channel,
                    parent,
                    Some(MixerRole::Submix),
                    name,
                    &place,
                );
                return;
            }
            self.master_seen = true;
            self.make_main(track, channel, &place);
            return;
        }
        if role == MixerRole::Vca {
            // Beatlayer has no VCA (R16).
            self.report.add("a VCA channel", &place, "left out");
            for child in &track.children {
                let child_place = self.track_place(child, &child.info.name);
                self.report
                    .add("a track inside a VCA channel", &child_place, "left out");
            }
            return;
        }
        self.make_track_kind(track, channel, parent, Some(role), name, &place);
    }

    /// A track that isn't a master or a VCA (R15 to R17, R20).
    fn make_track_kind(
        &mut self,
        track: &model::Track,
        channel: Option<Channel>,
        parent: Option<TrackId>,
        role: Option<MixerRole>,
        name: Option<String>,
        place: &str,
    ) {
        let types = &track.content_types;
        let has_children = !track.children.is_empty();
        let kind = match role.unwrap_or(MixerRole::Regular) {
            MixerRole::Effect => TrackKind::Return,
            MixerRole::Submix => {
                if has_children {
                    TrackKind::Group
                } else {
                    TrackKind::Audio
                }
            }
            _ => {
                if has_children || types.contains(&ContentKind::Tracks) {
                    TrackKind::Group
                } else if types.contains(&ContentKind::Audio) && types.contains(&ContentKind::Notes)
                {
                    TrackKind::Midi
                } else if types.contains(&ContentKind::Notes) {
                    TrackKind::Midi
                } else if types.contains(&ContentKind::Audio) {
                    TrackKind::Audio
                } else if channel
                    .as_ref()
                    .is_some_and(|channel| channel.holds_role(DeviceRole::Instrument))
                {
                    TrackKind::Midi
                } else {
                    TrackKind::Audio
                }
            }
        };
        if kind == TrackKind::Group && role != Some(MixerRole::Submix) && !has_children {
            // A group with no tracks holds nothing; it stays an audio track.
        }
        let name = name.unwrap_or_else(|| self.default_name(kind));
        let id = self.create_track(kind, name.clone(), parent);
        let made = Made {
            id,
            kind,
            name,
            channel: channel.clone(),
        };
        self.made.push(made.clone());
        self.register(track, channel.as_ref(), &made);
        if track.loaded == Some(false) {
            // A track that isn't loaded has its Track Activator off (R20).
            self.apply(bl_model::Command::SetMute {
                track: id,
                mute: true,
            });
            self.report.add(
                "a track that isn't loaded",
                place,
                "its Track Activator is off",
            );
        }

        // A track holding both audio and notes becomes a MIDI track and an
        // audio track beside it (R17).
        if kind == TrackKind::Midi
            && types.contains(&ContentKind::Audio)
            && types.contains(&ContentKind::Notes)
        {
            let audio_name = format!("{} Audio", made.name);
            let audio_id = self.create_track(TrackKind::Audio, audio_name.clone(), parent);
            let audio = Made {
                id: audio_id,
                kind: TrackKind::Audio,
                name: audio_name,
                channel: None,
            };
            self.report.add(
                "a track holding both audio and notes",
                place,
                "split into a MIDI track and an audio track",
            );
            self.audio_siblings.push((made.id, audio.id));
        }

        // The group's tracks.
        let group = if kind == TrackKind::Group {
            Some(id)
        } else {
            None
        };
        for child in &track.children {
            self.make_track(child, group);
        }
    }

    /// The master channel becomes Main (R18).
    fn make_main(&mut self, track: &model::Track, channel: Option<Channel>, place: &str) {
        let main = self.editor.document().main_id();
        if let Some(channel) = &channel {
            if channel.mute.as_ref().and_then(|mute| mute.value) == Some(true) {
                // Main has no activator (R18).
                self.report.add("a muted master channel", place, "left out");
            }
            self.report_devices(channel, place);
        }
        let made = Made {
            id: main,
            kind: TrackKind::Main,
            name: "Main".to_owned(),
            channel: channel.clone(),
        };
        self.made.push(made.clone());
        self.register(track, channel.as_ref(), &made);
    }

    /// A channel that sits directly in the structure (R15).
    fn make_bare_channel(&mut self, channel: &Channel, parent: Option<TrackId>) {
        let role = channel.role.unwrap_or(MixerRole::Regular);
        let place = channel
            .info
            .name
            .clone()
            .map(|name| format!("the channel \"{name}\""))
            .unwrap_or_else(|| "a channel in the structure".to_owned());
        if role == MixerRole::Master {
            if self.master_seen {
                self.report
                    .add("a second master channel", &place, "read as a group channel");
            } else {
                self.master_seen = true;
                self.make_main(
                    &model::Track {
                        info: channel.info.clone(),
                        content_types: Vec::new(),
                        loaded: None,
                        channel: Some(channel.clone()),
                        children: Vec::new(),
                    },
                    Some(channel.clone()),
                    &place,
                );
                return;
            }
        }
        if role == MixerRole::Vca {
            self.report.add("a VCA channel", &place, "left out");
            return;
        }
        let kind = if role == MixerRole::Effect {
            TrackKind::Return
        } else {
            TrackKind::Audio
        };
        let name = channel
            .info
            .name
            .clone()
            .unwrap_or_else(|| self.default_name(kind));
        let id = self.create_track(kind, name.clone(), parent);
        let made = Made {
            id,
            kind,
            name,
            channel: Some(channel.clone()),
        };
        self.made.push(made.clone());
        self.register_channel(channel, &made);
    }

    /// Records the file's ids against the tracks they name (R7, R19).
    fn register(&mut self, track: &model::Track, channel: Option<&Channel>, made: &Made) {
        if let Some(id) = &track.info.id {
            self.tracks.insert(id.clone(), made.id);
        }
        if let Some(channel) = channel {
            self.register_channel(channel, made);
        }
    }

    /// Records a channel's id against the track it names.
    fn register_channel(&mut self, channel: &Channel, made: &Made) {
        if let Some(id) = &channel.info.id {
            self.channels.insert(id.clone(), made.id);
            if made.kind == TrackKind::Return {
                self.returns.insert(id.clone(), made.id);
            }
        }
    }

    /// Where a track is, for the report (R12).
    fn track_place(&self, track: &model::Track, name: &Option<String>) -> String {
        let name = name.clone().or_else(|| track.info.name.clone());
        match name {
            Some(name) => format!("the track \"{name}\""),
            None => "a track in the file".to_owned(),
        }
    }

    /// A track's default name (R20).
    fn default_name(&mut self, kind: TrackKind) -> String {
        match kind {
            TrackKind::Return => {
                self.return_number += 1;
                format!("{} Return", letter(self.return_number))
            }
            TrackKind::Main => "Main".to_owned(),
            _ => {
                self.track_number += 1;
                let word = match kind {
                    TrackKind::Audio => "Audio",
                    TrackKind::Midi => "MIDI",
                    TrackKind::Group => "Group",
                    _ => "Track",
                };
                format!("{} {word}", self.track_number)
            }
        }
    }

    /// Makes a track in the set.
    fn create_track(&mut self, kind: TrackKind, name: String, parent: Option<TrackId>) -> TrackId {
        let id: TrackId = self.editor.new_id();
        let index = match kind {
            TrackKind::Return => self.editor.document().returns().len(),
            _ => match parent {
                Some(group) => self
                    .editor
                    .document()
                    .track(group)
                    .map_or(0, |group| group.children().len()),
                None => self.editor.document().top_level().len(),
            },
        };
        if let Err(error) = self.editor.apply(bl_model::Command::CreateTrack {
            id,
            kind,
            name,
            parent,
            index,
        }) {
            self.failed = Some(Error::Failed(error.to_string()));
        }
        id
    }

    /// The mixer and the routing of every track (R19 to R22).
    fn apply_mixer(&mut self) {
        let made = self.make_list();
        for track in &made {
            let Some(channel) = &track.channel else {
                continue;
            };
            let place = format!("the track \"{}\"", track.name);
            self.report_devices(channel, &place);
            if let Some(volume) = &channel.volume {
                let gain = self.volume_gain(volume, &place);
                self.apply(bl_model::Command::SetVolume {
                    track: track.id,
                    volume: gain,
                });
            }
            if let Some(pan) = &channel.pan {
                let pan = self.pan_value(pan);
                self.apply(bl_model::Command::SetPan {
                    track: track.id,
                    pan,
                });
            }
            if channel.mute.as_ref().and_then(|mute| mute.value) == Some(true) {
                self.apply(bl_model::Command::SetMute {
                    track: track.id,
                    mute: true,
                });
            }
            if channel.solo == Some(true) {
                self.apply(bl_model::Command::SetSolo {
                    track: track.id,
                    solo: true,
                });
            }
            if track.kind == TrackKind::Audio && channel.audio_channels.is_some_and(|n| n != 2) {
                // Beatlayer's tracks are stereo (R20).
                self.report.add(
                    "a channel count that isn't two",
                    &place,
                    "left out; the track plays in stereo",
                );
            }
        }
        self.apply_sends(&made);
        self.apply_outputs(&made);
    }

    /// The devices of a channel, which WP-6.36b reads (R44).
    fn report_devices(&mut self, channel: &Channel, place: &str) {
        for device in &channel.devices {
            let what = match device {
                Device::Plugin { format, .. } => match format {
                    model::PluginFormat::Vst2 => "a VST2 plug-in",
                    model::PluginFormat::Vst3 => "a VST3 plug-in",
                    model::PluginFormat::Clap => "a CLAP plug-in",
                    model::PluginFormat::Au => "an Audio Unit plug-in",
                },
                Device::Builtin { .. } => "a built-in device",
                Device::Equalizer { .. } => "a generic equaliser",
                Device::Compressor { .. } => "a generic compressor",
                Device::NoiseGate { .. } => "a generic gate",
                Device::Limiter { .. } => "a generic limiter",
            };
            self.report.add(what, place, "left out");
        }
    }

    /// A volume as a linear gain (R21).
    fn volume_gain(&mut self, parameter: &model::RealParameter, place: &str) -> f64 {
        let Some(value) = parameter.value else {
            return 1.0;
        };
        let gain = match parameter.unit {
            Some(Unit::Linear) => value,
            Some(Unit::Decibel) => {
                if value == f64::NEG_INFINITY {
                    0.0
                } else {
                    10f64.powf(value / 20.0)
                }
            }
            Some(Unit::Normalized) => {
                let min = parameter.min.unwrap_or(0.0);
                let max = parameter.max.unwrap_or(1.0);
                min + value * (max - min)
            }
            _ => {
                self.report.add(
                    "a volume in a unit Beatlayer can't read",
                    place,
                    "left at its default",
                );
                return 1.0;
            }
        };
        if !gain.is_finite() || gain < 0.0 {
            return 1.0;
        }
        if gain > 2.0 {
            // The fader's top is +6 dB (R21).
            self.report
                .add("a volume past the fader's top", place, "changed to +6 dB");
            return 2.0;
        }
        gain
    }

    /// A pan as a position from −1 to 1 (R21).
    fn pan_value(&mut self, parameter: &model::RealParameter) -> f64 {
        let Some(value) = parameter.value else {
            return 0.0;
        };
        let pan = match parameter.unit {
            Some(Unit::Percent) => {
                let min = parameter.min.unwrap_or(0.0);
                let max = parameter.max.unwrap_or(100.0);
                (value - min) / (max - min) * 2.0 - 1.0
            }
            Some(Unit::Normalized) | Some(Unit::Linear) | None => {
                let min = parameter.min.unwrap_or(0.0);
                let max = parameter.max.unwrap_or(1.0);
                (value - min) / (max - min) * 2.0 - 1.0
            }
            Some(_) => return 0.0,
        };
        pan.clamp(-1.0, 1.0)
    }

    /// The sends of every track (R22).
    fn apply_sends(&mut self, made: &[Made]) {
        // The sends to each return, and whether they are pre or post.
        let mut taps: HashMap<TrackId, (usize, usize)> = HashMap::new();
        for track in made {
            let Some(channel) = &track.channel else {
                continue;
            };
            let place = format!("the track \"{}\"", track.name);
            if track.kind == TrackKind::Return || track.kind == TrackKind::Main {
                for _send in &channel.sends {
                    self.report
                        .add("a send from a return or Main", &place, "left out");
                }
                continue;
            }
            for send in &channel.sends {
                let destination = send
                    .destination
                    .clone()
                    .and_then(|id| self.returns.get(&id).copied());
                let Some(return_track) = destination else {
                    self.report.add(
                        "a send to a channel that isn't a return",
                        &place,
                        "left out",
                    );
                    continue;
                };
                let level = send
                    .volume
                    .as_ref()
                    .map(|volume| self.volume_gain(volume, &place))
                    .unwrap_or(1.0);
                if send.enable.as_ref().and_then(|enable| enable.value) == Some(false) {
                    self.report.add(
                        &format!(
                            "a send at {} dB",
                            (20.0 * level.max(f64::MIN_POSITIVE).log10()).round()
                        ),
                        &place,
                        "left out; the send is switched off",
                    );
                    continue;
                }
                if send
                    .pan
                    .as_ref()
                    .and_then(|pan| pan.value)
                    .is_some_and(|pan| pan != 0.5)
                {
                    self.report.add(
                        "a send's pan",
                        &place,
                        "left out; Beatlayer's sends have no pan",
                    );
                }
                if level > 1.0 {
                    self.report
                        .add("a send above 0 dB", &place, "changed to 0 dB");
                }
                self.apply(bl_model::Command::SetSend {
                    track: track.id,
                    to: return_track,
                    level: level.min(1.0),
                });
                let entry = taps.entry(return_track).or_default();
                if send.send_type.as_deref() == Some("pre") {
                    entry.0 += 1;
                } else {
                    entry.1 += 1;
                }
            }
        }
        for (return_track, (pre, post)) in taps {
            let tap = if pre > post {
                bl_model::SendTap::Pre
            } else {
                bl_model::SendTap::Post
            };
            if pre > 0 && post > 0 {
                let name = self
                    .editor
                    .document()
                    .track(return_track)
                    .map_or_else(|| "a return".to_owned(), |track| track.name().to_owned());
                self.report.add(
                    "sends that disagree about pre or post",
                    &format!("the return \"{name}\""),
                    "the return takes them post",
                );
            }
            self.apply(bl_model::Command::SetSendTap {
                track: return_track,
                tap,
            });
        }
    }

    /// Where every track's output goes (R19).
    fn apply_outputs(&mut self, made: &[Made]) {
        let main = self.editor.document().main_id();
        for track in made {
            let Some(channel) = &track.channel else {
                continue;
            };
            let place = format!("the track \"{}\"", track.name);
            let Some(destination) = &channel.destination else {
                continue;
            };
            let Some(target) = self.channels.get(destination).copied() else {
                self.report.add(
                    "a destination the file doesn't hold",
                    &place,
                    "left out; the track plays to Main",
                );
                continue;
            };
            if target == main {
                // Main is the default output for a track at the top, and a
                // MIDI track has no other output (R19).
                if track.kind != TrackKind::Midi {
                    self.apply(bl_model::Command::SetOutput {
                        track: track.id,
                        output: Output::Main,
                    });
                }
                continue;
            }
            if self.editor.document().parent(track.id) == Some(target) {
                // The track's own group's channel is the default output (R19).
                continue;
            }
            let Some(target_track) = self.editor.document().track(target) else {
                continue;
            };
            let target_kind = target_track.kind();
            let target_clips = target_track.clips().count();
            match target_kind {
                TrackKind::Audio if track.kind == TrackKind::Midi => {
                    // A MIDI track has no output but its default (R19).
                    self.report.add(
                        "a destination that isn't a track's input",
                        &place,
                        "left out; the track plays to Main",
                    );
                }
                TrackKind::Audio => {
                    self.apply(bl_model::Command::SetOutput {
                        track: track.id,
                        output: Output::Track { track: target },
                    });
                    // The receiving track hears what is routed to it (R19).
                    self.apply(bl_model::Command::SetMonitor {
                        track: target,
                        monitor: Monitor::In,
                    });
                    self.apply(bl_model::Command::SetInput {
                        track: target,
                        input: bl_model::Input::None,
                        channel: None,
                    });
                    if target_clips > 0 {
                        self.report.add(
                            "a track that holds clips and monitors an input",
                            &place,
                            "the clips are silent while it monitors",
                        );
                    }
                }
                _ => {
                    self.report.add(
                        "a destination that isn't a track's input",
                        &place,
                        "left out; the track plays to Main",
                    );
                }
            }
        }
    }

    /// The clips of every track (R24 to R37).
    fn build_clips(&mut self, project: &DawProject) {
        let Some(arrangement) = &project.arrangement else {
            return;
        };
        // The lanes of each track, in the file's order.
        let mut lanes: HashMap<String, Vec<(model::Clips, TimeUnit)>> = HashMap::new();
        let mut loose: HashMap<String, Vec<(Timeline, TimeUnit)>> = HashMap::new();
        for lane in &arrangement.lanes {
            collect_lanes(lane, None, TimeUnit::Beats, &mut lanes, &mut loose);
        }
        for (track_id, track_lanes) in lanes {
            let Some(track) = self.tracks.get(&track_id).copied() else {
                continue;
            };
            let kind = self
                .editor
                .document()
                .track(track)
                .map_or(TrackKind::Audio, |track| track.kind());
            let name = self
                .editor
                .document()
                .track(track)
                .map_or_else(String::new, |track| track.name().to_owned());
            let place = format!("the track \"{name}\"");
            let mut clips: Vec<Clip> = Vec::new();
            // The clips that belong on another track: an audio clip on a
            // MIDI track goes to the audio track beside it (R37).
            let mut elsewhere: HashMap<TrackId, Vec<Clip>> = HashMap::new();
            let mut take_lanes: Vec<(String, Vec<Clip>)> = Vec::new();
            for (index, (timeline, unit)) in track_lanes.iter().enumerate() {
                if index == 0 {
                    for clip in &timeline.clips {
                        let (target, target_kind, target_place) =
                            self.clip_target(clip, track, kind, &place);
                        let made = self.build_clip(clip, *unit, target, target_kind, &target_place);
                        if target == track {
                            clips.extend(made);
                        } else {
                            elsewhere.entry(target).or_default().extend(made);
                        }
                    }
                } else if kind == TrackKind::Audio {
                    let lane_name = timeline
                        .info
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("Take {}", take_lanes.len() + 1));
                    let mut lane_clips = Vec::new();
                    for clip in &timeline.clips {
                        lane_clips.extend(self.build_clip(clip, *unit, track, kind, &place));
                    }
                    take_lanes.push((lane_name, lane_clips));
                } else {
                    for _clip in &timeline.clips {
                        self.report
                            .add("a lane of clips on a MIDI track", &place, "left out");
                    }
                }
            }
            for (timeline, unit) in loose.get(&track_id).cloned().unwrap_or_default() {
                // A timeline outside any clip becomes one clip (R24).
                let made = self.build_loose_clip(&timeline, unit, track, kind, &place);
                clips.extend(made);
            }
            self.place_clips(track, clips);
            for (target, made) in elsewhere {
                self.place_clips(target, made);
            }
            for (name, lane_clips) in take_lanes {
                if lane_clips.is_empty() {
                    continue;
                }
                let lane = bl_model::TakeLane {
                    name,
                    clips: lane_clips
                        .into_iter()
                        .map(|clip| bl_model::TakeClip {
                            name: clip.name.clone(),
                            start: clip.start,
                            length: clip.length,
                            data: match clip.data {
                                ClipData::Audio(data) => data,
                                ClipData::Midi(_) => AudioData::new(ContentHash([0; 32])),
                            },
                        })
                        .collect(),
                };
                self.apply(bl_model::Command::AddTakeLane { track, lane });
            }
        }
    }

    /// Places the clips of a track, each replacing what it covers of the
    /// clips before it (R26).
    fn place_clips(&mut self, track: TrackId, clips: Vec<Clip>) {
        let mut placed: Vec<Clip> = Vec::new();
        for clip in clips {
            let start = clip.start;
            let end = clip.end();
            let mut kept: Vec<Clip> = Vec::new();
            for earlier in placed {
                if earlier.end() <= start || earlier.start >= end {
                    kept.push(earlier);
                    continue;
                }
                if earlier.start >= start && earlier.end() <= end {
                    self.report
                        .add("a clip that another clip covers", "the track", "removed");
                    continue;
                }
                if earlier.start < start {
                    if earlier.end() > end {
                        // The new clip splits it.
                        let left = earlier.part(earlier.start, start, &self.tempo);
                        let right = earlier.part(end, earlier.end(), &self.tempo);
                        kept.push(left);
                        kept.push(right);
                        self.report
                            .add("a clip another clip covers", "the track", "split");
                    } else {
                        kept.push(earlier.part(earlier.start, start, &self.tempo));
                        self.report
                            .add("a clip another clip covers", "the track", "shortened");
                    }
                } else {
                    kept.push(earlier.part(end, earlier.end(), &self.tempo));
                    self.report
                        .add("a clip another clip covers", "the track", "shortened");
                }
            }
            kept.push(clip);
            placed = kept;
        }
        for clip in placed {
            let id = clip.id;
            self.apply(bl_model::Command::CreateClip { track, clip });
            let _ = id;
        }
    }

    /// A timeline outside any clip becomes one clip (R24).
    fn build_loose_clip(
        &mut self,
        timeline: &Timeline,
        unit: TimeUnit,
        track: TrackId,
        kind: TrackKind,
        place: &str,
    ) -> Vec<Clip> {
        let mut clip = model::Clip {
            info: model::TimelineInfo {
                id: None,
                name: timeline.info().name.clone(),
                color: None,
                comment: None,
                time_unit: Some(unit),
                track: None,
            },
            time: Some(0.0),
            duration: None,
            content_time_unit: Some(unit),
            play_start: None,
            play_stop: None,
            loop_start: None,
            loop_end: None,
            fade_time_unit: None,
            fade_in: None,
            fade_out: None,
            enable: None,
            reference: None,
            content: Some(Box::new(Content::Timeline(Box::new(timeline.clone())))),
        };
        match timeline {
            Timeline::Notes(notes) => {
                // A MIDI clip lasts as long as its notes, rounded up to the
                // next bar (R24, the MIDI clips spec R17).
                let length = notes
                    .notes
                    .iter()
                    .filter_map(|note| note.time.zip(note.duration))
                    .filter(|(_time, duration)| *duration > 0.0)
                    .map(|(time, duration)| time + duration)
                    .fold(0.0f64, f64::max);
                let bar = self.signatures.signature_at(Ticks::ZERO).bar_length();
                let length =
                    beats_to_ticks(round_up(length, bar.as_quarters())).max(Ticks::QUARTER);
                clip.duration = Some(length.as_quarters());
            }
            Timeline::Audio(file) | Timeline::Video(file) => {
                let duration = file.duration.unwrap_or(0.0);
                clip.duration = Some(duration);
            }
            Timeline::Warps(warps) => {
                let duration = warps
                    .audio
                    .as_ref()
                    .and_then(|audio| audio.duration)
                    .unwrap_or(0.0);
                clip.duration = Some(duration);
            }
            _ => {
                self.report
                    .add("a timeline that holds no clip", place, "left out");
                return Vec::new();
            }
        }
        self.build_clip(&clip, unit, track, kind, place)
    }

    /// The track a clip belongs on: an audio clip on a MIDI track goes to
    /// the audio track beside it (R37).
    fn clip_target(
        &self,
        source: &model::Clip,
        track: TrackId,
        kind: TrackKind,
        place: &str,
    ) -> (TrackId, TrackKind, String) {
        if kind == TrackKind::Midi
            && self.content_is_audio(source)
            && let Some(sibling) = self.audio_sibling(track)
        {
            let name = self
                .editor
                .document()
                .track(sibling)
                .map_or_else(String::new, |track| track.name().to_owned());
            return (sibling, TrackKind::Audio, format!("the track \"{name}\""));
        }
        (track, kind, place.to_owned())
    }

    /// The clips a DAWproject clip becomes (R27 to R37).
    fn build_clip(
        &mut self,
        source: &model::Clip,
        parent_unit: TimeUnit,
        track: TrackId,
        kind: TrackKind,
        place: &str,
    ) -> Vec<Clip> {
        let Some(time) = source.time else {
            // A clip without its time is left out (R4).
            self.report
                .add("a clip without its time", place, "left out");
            return Vec::new();
        };
        let content_unit = source.content_time_unit.unwrap_or(parent_unit);
        let start = match parent_unit {
            TimeUnit::Beats => beats_to_ticks(time),
            TimeUnit::Seconds => self.seconds_to_ticks(self.tempo.clone(), time),
        };
        let content = self.resolve_content(source);
        let Some(content) = content else {
            return Vec::new();
        };
        self.clip_from_content(source, &content, content_unit, start, track, kind, place, 0)
    }

    /// What a clip plays: its own content, or a copy of the timeline it
    /// names (R35).
    fn resolve_content(&mut self, source: &model::Clip) -> Option<Timeline> {
        if let Some(reference) = &source.reference {
            let Some(timeline) = self.timelines.get(reference) else {
                // A reference to something that isn't a timeline leaves the
                // clip out (R7, R35).
                self.report.add(
                    "a clip that names a timeline the file doesn't hold",
                    "the clip",
                    "left out",
                );
                return None;
            };
            self.report
                .add("an alias clip", "the clip", "read as a copy");
            return Some(timeline.clone());
        }
        source
            .content
            .as_ref()
            .and_then(|content| content.timeline())
            .cloned()
    }

    /// The clips a clip's content makes (R27 to R37).
    #[allow(clippy::too_many_arguments)]
    fn clip_from_content(
        &mut self,
        source: &model::Clip,
        content: &Timeline,
        content_unit: TimeUnit,
        start: Ticks,
        track: TrackId,
        kind: TrackKind,
        place: &str,
        depth: usize,
    ) -> Vec<Clip> {
        match content {
            Timeline::Notes(notes) => {
                self.midi_clip(source, notes, content_unit, start, kind, place)
            }
            Timeline::Audio(file) => {
                self.audio_clip(source, file, content_unit, start, kind, place, false)
            }
            Timeline::Video(_) => {
                // Video isn't carried (R36).
                self.report.add("a video clip", place, "left out");
                Vec::new()
            }
            Timeline::Warps(warps) => {
                self.warped_clip(source, warps, content_unit, start, kind, place)
            }
            Timeline::Clips(clips) => {
                if depth > 8 {
                    self.report
                        .add("a clip nested too deeply", place, "left out");
                    return Vec::new();
                }
                // A clip whose content is a timeline of clips is flattened
                // (R34).
                let mut made = Vec::new();
                for inner in &clips.clips {
                    let inner_content = self.resolve_content(inner);
                    let Some(inner_content) = inner_content else {
                        continue;
                    };
                    let inner_unit = inner.content_time_unit.unwrap_or(content_unit);
                    made.extend(self.clip_from_content(
                        inner,
                        &inner_content,
                        inner_unit,
                        start,
                        track,
                        kind,
                        place,
                        depth + 1,
                    ));
                }
                for _clip in &made {
                    self.report.add("a nested clip", place, "flattened");
                }
                made
            }
            Timeline::Lanes(lanes) => {
                // A clip whose content is a set of lanes takes its notes or
                // audio from the first of them (R36).
                let mut found = false;
                let mut made = Vec::new();
                for child in &lanes.children {
                    match child {
                        Timeline::Notes(_) | Timeline::Audio(_) | Timeline::Warps(_) => {
                            if found {
                                self.report.add(
                                    "a further timeline in a clip's lanes",
                                    place,
                                    "left out",
                                );
                                continue;
                            }
                            found = true;
                            made.extend(self.clip_from_content(
                                source,
                                child,
                                content_unit,
                                start,
                                track,
                                kind,
                                place,
                                depth + 1,
                            ));
                        }
                        Timeline::Video(_) => {
                            self.report.add("a video timeline", place, "left out");
                        }
                        Timeline::Points(_) => {
                            self.report
                                .add("automation in a clip's lanes", place, "left out");
                        }
                        _ => {}
                    }
                }
                made
            }
            Timeline::ClipSlot(slot) => {
                let Some(clip) = &slot.clip else {
                    return Vec::new();
                };
                self.build_clip(clip, content_unit, track, kind, place)
            }
            Timeline::Points(_) => {
                self.report.add("automation", place, "left out");
                Vec::new()
            }
            Timeline::Markers(_) => {
                self.report.add("a timeline of markers", place, "left out");
                Vec::new()
            }
        }
    }

    /// A MIDI clip (R28).
    fn midi_clip(
        &mut self,
        source: &model::Clip,
        notes: &model::Notes,
        unit: TimeUnit,
        start: Ticks,
        kind: TrackKind,
        place: &str,
    ) -> Vec<Clip> {
        if kind != TrackKind::Midi {
            // Notes on an audio track are left out (R37).
            self.report
                .add("notes on an audio track", place, "left out");
            return Vec::new();
        }
        let mut made_notes: Vec<Note> = Vec::new();
        let mut channels: Vec<i64> = Vec::new();
        let mut left_out = 0usize;
        for note in &notes.notes {
            let Some(key) = note.key else {
                self.report.add("a note without its key", place, "left out");
                left_out += 1;
                continue;
            };
            if !(0..=127).contains(&key) {
                left_out += 1;
                continue;
            }
            let Some(time) = note.time else {
                left_out += 1;
                continue;
            };
            let Some(duration) = note.duration else {
                left_out += 1;
                continue;
            };
            if duration <= 0.0 {
                left_out += 1;
                continue;
            }
            let start_ticks = match unit {
                TimeUnit::Beats => beats_to_ticks(time),
                TimeUnit::Seconds => self.seconds_to_ticks(self.tempo.clone(), time),
            };
            let length_ticks = match unit {
                TimeUnit::Beats => beats_to_ticks(duration),
                TimeUnit::Seconds => self.seconds_to_ticks(self.tempo.clone(), duration),
            };
            let velocity = note
                .vel
                .map(|vel| (vel * 127.0).round().clamp(1.0, 127.0) as u8)
                .unwrap_or(100);
            let release = note
                .rel
                .map(|rel| (rel * 127.0).round().clamp(1.0, 127.0) as u8)
                .unwrap_or(64);
            let mut built = Note::new(key as u8, start_ticks, length_ticks.max(Ticks(1)), velocity);
            built.release_velocity = release;
            made_notes.push(built);
            if let Some(channel) = note.channel {
                channels.push(channel);
            }
        }
        if left_out > 0 {
            self.report
                .add_count("a note that can't be played", place, "left out", left_out);
        }
        channels.sort_unstable();
        channels.dedup();
        if channels.len() > 1 {
            // A note's channel isn't kept (R28).
            self.report.add(
                "notes on more than one channel",
                place,
                "left out; the notes play on one channel",
            );
        }
        let length = self.clip_length(source, start, unit, Some(notes_length(&made_notes)));
        let mut data = MidiData::new(made_notes);
        data.start = source
            .play_start
            .map(|value| self.markers_to_ticks(value, unit))
            .unwrap_or(Ticks::ZERO);
        data.end = source
            .play_stop
            .map(|value| self.markers_to_ticks(value, unit));
        let looping = source
            .loop_start
            .zip(source.loop_end)
            .is_some_and(|(start, end)| end > start);
        data.looping = looping;
        if looping {
            data.loop_start = source
                .loop_start
                .map(|value| self.markers_to_ticks(value, unit))
                .unwrap_or(Ticks::ZERO);
            data.loop_end = source
                .loop_end
                .map(|value| self.markers_to_ticks(value, unit));
        }
        let mut clip = Clip::new(self.editor.new_id(), start, length, ClipData::Midi(data));
        self.finish_clip(&mut clip, source);
        vec![clip]
    }

    /// An audio clip that isn't warped (R29).
    #[allow(clippy::too_many_arguments)]
    fn audio_clip(
        &mut self,
        source: &model::Clip,
        file: &MediaFile,
        unit: TimeUnit,
        start: Ticks,
        kind: TrackKind,
        place: &str,
        warped: bool,
    ) -> Vec<Clip> {
        let _ = warped;
        if kind != TrackKind::Audio {
            self.report
                .add("an audio clip on a MIDI track", place, "left out");
            return Vec::new();
        }
        let Some(hash) = self.media_for(file, place) else {
            return Vec::new();
        };
        let mut data = AudioData::new(hash);
        data.start = source.play_start.unwrap_or(0.0);
        data.end = source.play_stop;
        let looping = source
            .loop_start
            .zip(source.loop_end)
            .is_some_and(|(start, end)| end > start);
        data.looping = looping;
        data.loop_start = source.loop_start.unwrap_or(0.0);
        data.loop_end = source.loop_end;
        let length = self.clip_length(source, start, unit, file.duration);
        let mut clip = Clip::new(self.editor.new_id(), start, length, ClipData::Audio(data));
        self.finish_clip(&mut clip, source);
        vec![clip]
    }

    /// An audio clip whose content is a set of warps (R30, R31).
    #[allow(clippy::too_many_arguments)]
    fn warped_clip(
        &mut self,
        source: &model::Clip,
        warps: &model::Warps,
        unit: TimeUnit,
        start: Ticks,
        kind: TrackKind,
        place: &str,
    ) -> Vec<Clip> {
        if kind != TrackKind::Audio {
            self.report
                .add("an audio clip on a MIDI track", place, "left out");
            return Vec::new();
        }
        let Some(file) = &warps.audio else {
            self.report
                .add("a warped clip with no file", place, "left out");
            return Vec::new();
        };
        let rising = warps.warps.len() >= 2
            && warps
                .warps
                .windows(2)
                .all(|pair| match (pair[0].time, pair[1].time) {
                    (Some(first), Some(second)) => second > first,
                    _ => false,
                });
        if !rising {
            // A set of warps that doesn't rise is read as an unwarped clip
            // (R30).
            self.report.add(
                "a warped clip whose warps don't rise",
                place,
                "read as an unwarped clip",
            );
            return self.audio_clip(source, file, unit, start, kind, place, false);
        }
        let Some(hash) = self.media_for(file, place) else {
            return Vec::new();
        };
        let mut data = AudioData::new(hash);
        data.warp = true;
        let mut markers = Vec::new();
        for warp in &warps.warps {
            let (Some(time), Some(content_time)) = (warp.time, warp.content_time) else {
                continue;
            };
            markers.push(WarpMarker {
                time: content_time,
                beat: self.markers_to_ticks(time, unit),
            });
        }
        markers.sort_by(|a, b| a.beat.cmp(&b.beat).then(a.time.total_cmp(&b.time)));
        let tempo = markers
            .windows(2)
            .last()
            .map(|pair| {
                let beats = (pair[1].beat - pair[0].beat).as_quarters();
                let seconds = pair[1].time - pair[0].time;
                if seconds > 0.0 {
                    (beats / seconds * 60.0).clamp(Bpm::MIN.get(), Bpm::MAX.get())
                } else {
                    Bpm::DEFAULT.get()
                }
            })
            .unwrap_or(Bpm::DEFAULT.get());
        data.tempo = Some(tempo);
        data.warp_mode = self.warp_mode(file.algorithm.as_deref(), place);
        data.warp_markers = markers;
        // A warped clip's markers are beats of its contents (R30).
        data.start = source
            .play_start
            .map(|value| self.markers_to_ticks(value, unit).as_quarters())
            .unwrap_or(0.0);
        data.end = source
            .play_stop
            .map(|value| self.markers_to_ticks(value, unit).as_quarters());
        let looping = source
            .loop_start
            .zip(source.loop_end)
            .is_some_and(|(start, end)| end > start);
        data.looping = looping;
        if looping {
            data.loop_start = source
                .loop_start
                .map(|value| self.markers_to_ticks(value, unit).as_quarters())
                .unwrap_or(0.0);
            data.loop_end = source
                .loop_end
                .map(|value| self.markers_to_ticks(value, unit).as_quarters());
        }
        let length = self.clip_length(source, start, unit, None);
        let mut clip = Clip::new(self.editor.new_id(), start, length, ClipData::Audio(data));
        self.finish_clip(&mut clip, source);
        vec![clip]
    }

    /// A warped clip's mode (R31).
    fn warp_mode(&mut self, algorithm: Option<&str>, place: &str) -> WarpMode {
        let Some(algorithm) = algorithm else {
            return WarpMode::default();
        };
        let name: String = algorithm
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
            .flat_map(char::to_lowercase)
            .collect();
        let mode = match name.as_str() {
            "repitch" => Some(WarpMode::RePitch),
            "beats" => Some(WarpMode::Beats),
            "tones" => Some(WarpMode::Tones),
            "texture" => Some(WarpMode::Texture),
            "complex" => Some(WarpMode::Complex),
            "complexpro" => Some(WarpMode::ComplexPro),
            _ => None,
        };
        match mode {
            Some(mode) => mode,
            None => {
                // Any other name takes the Default Warp Mode (R31).
                self.report.add(
                    &format!("the warp mode `{algorithm}`"),
                    place,
                    "changed to the default warp mode",
                );
                WarpMode::default()
            }
        }
    }

    /// A clip's name and switch (R27).
    fn finish_clip(&mut self, clip: &mut Clip, source: &model::Clip) {
        if let Some(name) = &source.info.name {
            clip.name = name.clone();
        }
        if source.enable == Some(false) {
            clip.mute = true;
        }
    }

    /// How long a clip is (R27).
    fn clip_length(
        &mut self,
        source: &model::Clip,
        start: Ticks,
        unit: TimeUnit,
        content_length: Option<f64>,
    ) -> Ticks {
        if let Some(duration) = source.duration {
            return match unit {
                TimeUnit::Beats => beats_to_ticks(duration),
                TimeUnit::Seconds => {
                    let end = self.seconds_to_ticks(self.tempo.clone(), duration);
                    end.saturating_sub(start).max(Ticks(1))
                }
            };
        }
        if let (Some(play_start), Some(play_stop)) = (source.play_start, source.play_stop) {
            let length = play_stop - play_start;
            return match unit {
                TimeUnit::Beats => beats_to_ticks(length).max(Ticks(1)),
                TimeUnit::Seconds => {
                    let end = self.seconds_to_ticks(self.tempo.clone(), play_stop);
                    let from = self.seconds_to_ticks(self.tempo.clone(), play_start);
                    end.saturating_sub(from).max(Ticks(1))
                }
            };
        }
        match content_length {
            Some(length) if length > 0.0 => match unit {
                TimeUnit::Beats => beats_to_ticks(length).max(Ticks(1)),
                TimeUnit::Seconds => {
                    let end = self.seconds_to_ticks(self.tempo.clone(), length);
                    end.saturating_sub(start).max(Ticks(1))
                }
            },
            _ => Ticks::QUARTER,
        }
    }

    /// A clip's play or loop marker, in ticks (R9).
    fn markers_to_ticks(&self, value: f64, unit: TimeUnit) -> Ticks {
        match unit {
            TimeUnit::Beats => beats_to_ticks(value),
            TimeUnit::Seconds => self.seconds_to_ticks(self.tempo.clone(), value),
        }
    }

    /// The media a file names, or `None` when it isn't there (R38 to R41).
    fn media_for(&mut self, file: &MediaFile, place: &str) -> Option<ContentHash> {
        let reference = file.file.as_ref()?;
        let path = reference.path.replace('\\', "/");
        if let Some(hash) = self.resolved.get(&path) {
            return *hash;
        }
        let resolved = self.resolve_media(&path, reference.external, file, place);
        self.resolved.insert(path, resolved);
        resolved
    }

    /// The media a file names, read from the container or from the disk
    /// (R38 to R41).
    fn resolve_media(
        &mut self,
        path: &str,
        external: bool,
        file: &MediaFile,
        place: &str,
    ) -> Option<ContentHash> {
        let mut found = PathBuf::new();
        let bytes = if external {
            match self.find_external(path) {
                Some((at, moved)) => {
                    found = at.clone();
                    if moved {
                        self.report.add(
                            &format!("the file `{path}`"),
                            place,
                            &format!("found at {}", at.display()),
                        );
                    }
                    match std::fs::read(&at) {
                        Ok(bytes) => bytes,
                        Err(_) => {
                            self.report.add(
                                &format!("the file `{path}`"),
                                place,
                                "left out; it couldn't be read",
                            );
                            return None;
                        }
                    }
                }
                None => {
                    self.report.add(
                        &format!("the file `{path}`"),
                        place,
                        "left out; it wasn't found",
                    );
                    return None;
                }
            }
        } else {
            match self.container.files.get(path) {
                Some(bytes) => bytes.clone(),
                None => {
                    self.report.add(
                        &format!("the file `{path}`"),
                        place,
                        "left out; the container doesn't hold it",
                    );
                    return None;
                }
            }
        };
        let hash = hash_of(&bytes);
        let size = bytes.len() as u64;
        // The file has to decode to be played (R41).
        let extension = Path::new(path)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("");
        let Some((sample_rate, channels, frames)) = probe(&bytes, extension) else {
            self.report.add(
                &format!("the file `{path}`"),
                place,
                "left out; it isn't a file Beatlayer can play",
            );
            return None;
        };
        self.check_media_agreement(file, sample_rate, channels, frames, path, place);
        self.files.push(Extracted {
            path: path.to_owned(),
            bytes: if external { None } else { Some(bytes) },
            on_disk: if external { Some(found) } else { None },
            hash,
            size,
        });
        let mut media = self.media.clone();
        if let Ok(relative) = MediaPath::new(path) {
            media.insert(MediaRef {
                hash,
                path: relative,
                size: 0,
            });
        }
        self.media = media;
        Some(hash)
    }

    /// Whether the file's own rate, channels and length agree with the
    /// `Audio` element's (R41).
    fn check_media_agreement(
        &mut self,
        file: &MediaFile,
        sample_rate: u32,
        channels: u16,
        frames: Option<u64>,
        path: &str,
        place: &str,
    ) {
        let mut differs = false;
        if let Some(rate) = file.sample_rate
            && u32::try_from(rate).ok() != Some(sample_rate)
        {
            differs = true;
        }
        if let Some(count) = file.channels
            && u16::try_from(count).ok() != Some(channels)
        {
            differs = true;
        }
        if let (Some(duration), Some(frames)) = (file.duration, frames)
            && (duration - frames as f64 / f64::from(sample_rate)).abs() > 0.001
        {
            differs = true;
        }
        if differs {
            self.report.add(
                &format!("the file `{path}`"),
                place,
                "the file's own rate, channels and length were used",
            );
        }
    }

    /// The file a path names on disk, looked for by name when it isn't there
    /// (R40).
    fn find_external(&self, path: &str) -> Option<(PathBuf, bool)> {
        let direct = if is_absolute(path) {
            PathBuf::from(path)
        } else {
            self.context.folder.join(path)
        };
        if direct.is_file() {
            return Some((direct, false));
        }
        let name = Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())?;
        for found in walk(self.context.folder, 0) {
            if found.is_file()
                && found
                    .file_name()
                    .is_some_and(|found| found.to_string_lossy() == name)
            {
                return Some((found, true));
            }
        }
        None
    }

    /// The scenes of the Session View (R42, R43).
    fn build_scenes(&mut self, project: &DawProject) {
        if project.scenes.is_empty() {
            // A file with no scenes gets the eight a new set has (R42).
            return;
        }
        let scenes = self.editor.document().scene_count();
        for (index, scene) in project.scenes.iter().enumerate() {
            if index >= scenes {
                self.report.add("a scene", "the Session View", "left out");
                break;
            }
            let Some(scene_id) = self.editor.document().scenes().nth(index).map(|s| s.id) else {
                break;
            };
            if let Some(name) = &scene.info.name {
                self.apply(bl_model::Command::RenameScene {
                    scene: scene_id,
                    name: name.clone(),
                });
            }
            let mut slots: Vec<model::ClipSlot> = Vec::new();
            if let Some(content) = &scene.content
                && let Some(timeline) = content.timeline()
            {
                collect_slots(timeline, &mut slots);
            }
            let mut seen: Vec<String> = Vec::new();
            for slot in slots {
                let Some(track_id) = &slot.info.track else {
                    continue;
                };
                let Some(track) = self.tracks.get(track_id).copied() else {
                    self.report.add(
                        "a slot for a track the file doesn't hold",
                        "the Session View",
                        "left out",
                    );
                    continue;
                };
                let kind = self
                    .editor
                    .document()
                    .track(track)
                    .map_or(TrackKind::Audio, |track| track.kind());
                if !kind.holds_clips() {
                    self.report.add(
                        "a slot for a track that can't hold clips",
                        "the Session View",
                        "left out",
                    );
                    continue;
                }
                if seen.contains(track_id) {
                    self.report.add(
                        "a second slot for one track in one scene",
                        "the Session View",
                        "left out",
                    );
                    continue;
                }
                seen.push(track_id.clone());
                let Some(clip) = &slot.clip else {
                    if slot.has_stop == Some(false) {
                        self.apply(bl_model::Command::SetSlotStopButton {
                            track,
                            scene: scene_id,
                            stop_button: false,
                        });
                    }
                    continue;
                };
                let name = self
                    .editor
                    .document()
                    .track(track)
                    .map_or_else(String::new, |track| track.name().to_owned());
                let place = format!("the track \"{name}\"");
                let clips = self.build_clip(clip, TimeUnit::Beats, track, kind, &place);
                let Some(mut clip) = clips.into_iter().next() else {
                    continue;
                };
                clip.start = Ticks::ZERO;
                self.apply(bl_model::Command::CreateSlotClip {
                    track,
                    scene: scene_id,
                    clip,
                });
                if slot.has_stop == Some(false) {
                    self.apply(bl_model::Command::SetSlotStopButton {
                        track,
                        scene: scene_id,
                        stop_button: false,
                    });
                }
            }
        }
    }

    /// The tracks the import made.
    fn make_list(&self) -> Vec<Made> {
        self.made.clone()
    }

    /// The audio track beside a MIDI track, for a hybrid track (R17).
    fn audio_sibling(&self, midi: TrackId) -> Option<TrackId> {
        self.audio_siblings
            .iter()
            .find(|(midi_track, _)| *midi_track == midi)
            .map(|(_, audio)| *audio)
    }

    /// Whether a clip plays audio, so that it belongs on an audio track
    /// (R37).
    fn content_is_audio(&self, source: &model::Clip) -> bool {
        if let Some(reference) = &source.reference {
            return self
                .timelines
                .get(reference)
                .is_some_and(|timeline| timeline_is_audio(timeline, 0));
        }
        source
            .content
            .as_ref()
            .and_then(|content| content.timeline())
            .is_some_and(|timeline| timeline_is_audio(timeline, 0))
    }

    /// Applies a command, keeping the first failure.
    fn apply(&mut self, command: bl_model::Command) {
        if let Err(error) = self.editor.apply(command) {
            self.failed = Some(Error::Failed(error.to_string()));
        }
    }

    /// A time in seconds as a tick, through a tempo map (R9).
    fn seconds_to_ticks(&self, tempo: TempoMap, seconds: f64) -> Ticks {
        tempo.seconds_to_ticks(seconds)
    }
}

/// The beat a quarter note is, as ticks.
fn beats_to_ticks(beats: f64) -> Ticks {
    if !beats.is_finite() {
        return Ticks::ZERO;
    }
    // A beat is a quarter note (R9).
    Ticks::from_quarters(beats.round() as i64)
}

/// A length rounded up to a whole bar.
fn round_up(length: f64, bar: f64) -> f64 {
    if bar <= 0.0 {
        return length.max(0.0);
    }
    (length / bar).ceil() * bar
}

/// How long a set of notes runs.
fn notes_length(notes: &[Note]) -> f64 {
    notes
        .iter()
        .map(|note| note.start.get() as f64 + note.length.get() as f64)
        .fold(0.0f64, f64::max)
}

/// What a file holds, when it decodes: its rate, its channels and its
/// length in frames.
fn probe(bytes: &[u8], extension: &str) -> Option<(u32, u16, Option<u64>)> {
    bl_media::Decoder::from_reader(std::io::Cursor::new(bytes.to_vec()), Some(extension))
        .ok()
        .map(|decoder| {
            let info = decoder.info();
            (info.sample_rate, info.channels, info.frames)
        })
}

/// Whether a timeline plays audio, looking through a timeline of clips for
/// the audio inside (R34, R37).
fn timeline_is_audio(timeline: &Timeline, depth: usize) -> bool {
    match timeline {
        Timeline::Audio(_) | Timeline::Warps(_) => true,
        Timeline::Clips(clips) if depth < 8 => clips.clips.iter().any(|clip| {
            clip.content
                .as_ref()
                .and_then(|content| content.timeline())
                .is_some_and(|timeline| timeline_is_audio(timeline, depth + 1))
        }),
        _ => false,
    }
}

/// The hash of a file's bytes.
fn hash_of(bytes: &[u8]) -> ContentHash {
    ContentHash(*bl_media::ContentHash::of_bytes(bytes).as_bytes())
}

/// Whether a path is absolute, on Windows or otherwise.
fn is_absolute(path: &str) -> bool {
    if path.starts_with('/') || path.starts_with('\\') {
        return true;
    }
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()
}

/// The folders inside a folder, a few levels deep.
fn walk(folder: &Path, depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if depth > 4 {
        return found;
    }
    let Ok(entries) = std::fs::read_dir(folder) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.push(path.clone());
            found.extend(walk(&path, depth + 1));
        }
    }
    found
}

/// The letter a return's number has.
fn letter(number: usize) -> String {
    let mut name = String::new();
    let mut number = number;
    while number > 0 {
        let rest = (number - 1) % 26;
        name.insert(0, (b'A' + u8::try_from(rest).unwrap_or(0)) as char);
        number = (number - 1) / 26;
    }
    name
}

/// The timelines a timeline holds, by their ids.
fn collect_timelines(timeline: &Timeline, found: &mut Vec<(String, Timeline)>) {
    if let Some(id) = &timeline.info().id {
        found.push((id.clone(), timeline.clone()));
    }
    match timeline {
        Timeline::Lanes(lanes) => {
            for child in &lanes.children {
                collect_timelines(child, found);
            }
        }
        Timeline::Clips(clips) => {
            for clip in &clips.clips {
                if let Some(content) = clip.content.as_ref()
                    && let Some(timeline) = content.timeline()
                {
                    collect_timelines(timeline, found);
                }
            }
        }
        Timeline::ClipSlot(slot) => {
            if let Some(clip) = &slot.clip
                && let Some(content) = clip.content.as_ref()
                && let Some(timeline) = content.timeline()
            {
                collect_timelines(timeline, found);
            }
        }
        Timeline::Warps(warps) => {
            if let Some(audio) = &warps.audio {
                found.push((
                    audio.info.id.clone().unwrap_or_default(),
                    Timeline::Audio(audio.clone()),
                ));
            }
        }
        _ => {}
    }
}

/// The sets of points in a timeline that target `parameter`.
fn collect_points(
    timeline: &Timeline,
    parameter: Option<&str>,
    unit: TimeUnit,
    found: &mut Vec<(model::Points, TimeUnit)>,
) {
    match timeline {
        Timeline::Points(points) => {
            let target = points.target.parameter.as_deref();
            if target.is_some() && target == parameter {
                found.push((points.clone(), unit));
            }
        }
        Timeline::Lanes(lanes) => {
            for child in &lanes.children {
                let unit = child.info().time_unit.unwrap_or(unit);
                collect_points(child, parameter, unit, found);
            }
        }
        _ => {}
    }
}

/// The lanes of each track, and the timelines outside any clip.
fn collect_lanes(
    timeline: &Timeline,
    scope: Option<String>,
    unit: TimeUnit,
    lanes: &mut HashMap<String, Vec<(model::Clips, TimeUnit)>>,
    loose: &mut HashMap<String, Vec<(Timeline, TimeUnit)>>,
) {
    let unit = timeline.info().time_unit.unwrap_or(unit);
    let scope = timeline.info().track.clone().or(scope);
    match timeline {
        Timeline::Lanes(children) => {
            for child in &children.children {
                collect_lanes(child, scope.clone(), unit, lanes, loose);
            }
        }
        Timeline::Clips(clips) => {
            if let Some(scope) = &scope {
                lanes
                    .entry(scope.clone())
                    .or_default()
                    .push((clips.clone(), unit));
            }
        }
        Timeline::Notes(_) | Timeline::Audio(_) | Timeline::Warps(_) => {
            if let Some(scope) = &scope {
                loose
                    .entry(scope.clone())
                    .or_default()
                    .push((timeline.clone(), unit));
            }
        }
        _ => {}
    }
}

/// The slots of a scene.
fn collect_slots(timeline: &Timeline, found: &mut Vec<model::ClipSlot>) {
    match timeline {
        Timeline::ClipSlot(slot) => found.push(slot.clone()),
        Timeline::Lanes(lanes) => {
            for child in &lanes.children {
                collect_slots(child, found);
            }
        }
        _ => {}
    }
}

/// The tempo map a set of points makes (R13).
fn build_tempo_map(points: &[(Ticks, f64, Interpolation)]) -> TempoMap {
    if points.is_empty() {
        return TempoMap::default();
    }
    let mut segments: Vec<TempoSegment> = Vec::new();
    if points[0].0 > Ticks::ZERO {
        // Before the first point the first point's value holds (R13).
        segments.push(TempoSegment::Constant {
            start: Ticks::ZERO,
            bpm: to_bpm(points[0].1),
        });
    }
    for (index, (start, bpm, interpolation)) in points.iter().enumerate() {
        let bpm = to_bpm(*bpm);
        let last = index + 1 == points.len();
        match interpolation {
            Interpolation::Linear if !last => {
                let to = to_bpm(points[index + 1].1);
                segments.push(TempoSegment::Ramp {
                    start: *start,
                    from: bpm,
                    to,
                });
            }
            _ => segments.push(TempoSegment::Constant { start: *start, bpm }),
        }
    }
    TempoMap::new(segments).unwrap_or_default()
}

/// A tempo in the range the song holds.
fn to_bpm(value: f64) -> Bpm {
    Bpm::new(value.clamp(Bpm::MIN.get(), Bpm::MAX.get())).unwrap_or(Bpm::DEFAULT)
}

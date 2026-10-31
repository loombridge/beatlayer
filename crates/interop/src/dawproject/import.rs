//! Turning a DAWproject file into a set (the DAWproject spec, R9 to R43):
//! the tracks, mixer and routing, the clips, the media, the Session View's
//! scenes, the tempo and time signature, and the report of what was left out
//! or changed.
//!
//! The import is pure: it reads the file's model and makes a set, with no
//! shell and no window. What the file's media holds is the shell's business.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bl_core::time::{
    Bpm, TempoMap, TempoSegment, Ticks, TimeSignature, TimeSignatureChange, TimeSignatureMap,
};
use bl_engine::DeviceType;
use bl_model::{
    AudioData, Clip, ClipData, ContentHash, Editor, MidiData, Monitor, Note, Output,
    ParamDescriptor, TrackId, TrackKind, WarpMarker, WarpMode,
};
use bl_project::{Media, MediaPath, MediaRef, Project};

use super::container::{Container, Error};
use super::model::{
    self, Channel, Content, ContentKind, DeviceRole, Interpolation, MediaFile, MixerRole,
    PointValue, Project as DawProject, TimeUnit, Timeline, Unit,
};
use super::plugins::{NoPlugins, Plugins};
use super::report::Report;

mod automation;
mod clips;
mod colours;
mod devices;
mod markers;

use automation::{Driven, Expressions, Pending, pan_from, volume_raw};
use clips::{Lane, Piece};

/// What an import needs beyond the file itself.
pub struct Context<'a> {
    /// The folder that holds the `.dawproject` file, where the files it
    /// refers to are looked for (R40).
    pub folder: &'a Path,
    /// The folder the container's files are written into (R39).
    pub extract_to: &'a Path,
    /// The warp mode a clip takes when the file names none Beatlayer has
    /// (R31): the Default Warp Mode setting.
    pub default_warp_mode: WarpMode,
    /// The 14 track colours of the theme in use, from which the palette is
    /// drawn (R23).
    pub track_colours: &'a [[u8; 3]],
    /// The colours a new track and a new clip take (R23).
    pub new_colours: &'a dyn NewColours,
    /// The plug-ins installed on this machine (R46).
    pub plugins: &'a dyn Plugins,
}

/// The swatches a track and a clip made without a colour take (R23).
pub trait NewColours: Sync {
    /// The swatch a new track takes.
    fn track(&self, id: TrackId) -> u8;

    /// The swatch a new clip takes, or `None` for its track's.
    fn clip(&self, id: bl_model::ClipId) -> Option<u8>;
}

/// The colours a new set gives: a track `id mod 14` in the top row, a clip
/// its track's.
pub struct DefaultColours;

impl NewColours for DefaultColours {
    fn track(&self, id: TrackId) -> u8 {
        bl_model::id_swatch(id.0)
    }

    fn clip(&self, _id: bl_model::ClipId) -> Option<u8> {
        None
    }
}

impl<'a> Context<'a> {
    /// A context that takes Beats for a clip's warp mode by default, the
    /// bundled theme's palette and a machine with no plug-ins.
    #[must_use]
    pub fn new(folder: &'a Path, extract_to: &'a Path) -> Self {
        Self {
            folder,
            extract_to,
            default_warp_mode: WarpMode::default(),
            track_colours: &bl_model::DEFAULT_TRACK_PALETTE,
            new_colours: &DefaultColours,
            plugins: &NoPlugins,
        }
    }
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
    /// Its bytes, for a small file made in memory (a preset's sample). A file
    /// of the container has neither this nor `on_disk`: the shell unpacks it
    /// from the container at `path`, so a medium is never held in memory.
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
        search: None,
        made: Vec::new(),
        audio_siblings: Vec::new(),
        master_seen: false,
        track_number: 0,
        return_number: 0,
        tempo: TempoMap::default(),
        signatures: TimeSignatureMap::default(),
        failed: None,
        default_warp_mode: context.default_warp_mode,
        scene: None,
        channels_seen: Vec::new(),
        received: Vec::new(),
        left_out: Vec::new(),
        types: None,
        targets: HashMap::new(),
        described: HashMap::new(),
        pending: Pending::new(),
        held_ends: std::collections::BTreeSet::new(),
        track_expressions: HashMap::new(),
        controller_channels: HashMap::new(),
        crossfades: Vec::new(),
    };
    importer.run()
}

/// What loading a plug-in gave: its parameters, or why it failed.
type Described = Result<Arc<Vec<ParamDescriptor>>, String>;

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
    /// clip (R35). Borrowed from the file's project.
    timelines: HashMap<&'a str, Cow<'a, Timeline>>,
    /// The set's media.
    media: Media,
    /// The container's files the set uses, to write out.
    files: Vec<Extracted>,
    /// The DAWproject file path → the media it names.
    resolved: HashMap<String, Result<ContentHash, String>>,
    /// The first file of each name in the `.dawproject` file's folder and the
    /// folders inside it, made when the first file is missing (R40).
    search: Option<HashMap<String, PathBuf>>,
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
    /// The warp mode a clip takes by default (R31).
    default_warp_mode: WarpMode,
    /// The scene whose clips are being read, from 1, when it's a scene's.
    scene: Option<usize>,
    /// The channels the notes of the track being read use (R28).
    channels_seen: Vec<i64>,
    /// The tracks other tracks are routed into (R19).
    received: Vec<TrackId>,
    /// The ids of tracks the import left out and listed, whose lanes it
    /// leaves out without listing them again.
    left_out: Vec<String>,
    /// The device types Beatlayer ships, made when first needed (R51).
    types: Option<Arc<Vec<DeviceType>>>,
    /// What each DAWproject parameter drives in the set (R56).
    targets: HashMap<String, Driven>,
    /// The parameters of the plug-ins loaded for their automation (R57).
    described: HashMap<(bl_model::PluginFormat, String), Described>,
    /// The Arrangement's envelopes, written when the import is done (R58).
    pending: Pending,
    /// Where an Arrangement clip's automation ends, by track and parameter,
    /// so its last value holds after the clip (R59).
    held_ends: std::collections::BTreeSet<(TrackId, bl_model::AutomationTarget, Ticks)>,
    /// The MIDI controllers outside clips, by the track they are for (R60).
    track_expressions: HashMap<TrackId, Vec<Expressions>>,
    /// The channel each controller of a clip was first given on (R60).
    controller_channels: HashMap<(bl_model::ClipId, u16), i64>,
    /// The crossfades the clips ask for: the track, where the later clip
    /// starts and how long they last in seconds (R33).
    crossfades: Vec<(TrackId, Ticks, f64)>,
}

impl<'a> Importer<'a, '_> {
    /// Reads the file and makes the set.
    fn run(&mut self) -> Result<Imported, Error> {
        let container: &'a Container = self.container;
        let project = &container.project;
        self.report.application = project.application.as_ref().and_then(|a| a.label());
        self.read_report();
        self.check_version(project)?;
        self.collect_timelines(project);
        self.build_tempo(project);
        self.build_signatures(project);
        self.apply(bl_model::Command::SetTempo {
            tempo: self.tempo.clone(),
        });
        self.apply(bl_model::Command::SetTimeSignatures {
            time_signatures: self.signatures.clone(),
        });
        self.build_tracks(project);
        self.collect_lane_points(project);
        self.build_clips(project);
        self.build_scenes(project);
        self.write_envelopes();
        self.build_markers(project);
        self.report_received();
        self.report_metadata();
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
            scale_folds: bl_model::ClipFolds::new(),
            midi_tools: bl_project::midi_tools::MidiToolSettings::default(),
            launch: bl_model::LaunchSettings::default(),
            multisamplers: bl_model::MultisamplerViews::new(),
            window_views: bl_model::WindowViews::default(),
            mpe_view: bl_model::MpeView::default(),
            lane_views: bl_model::LaneViews::new(),
            column_widths: bl_model::ColumnWidths::new(),
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
    fn collect_timelines(&mut self, project: &'a DawProject) {
        let mut found: Vec<(&'a str, Cow<'a, Timeline>)> = Vec::new();
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
                }
            }
        }
        if let Some(arrangement) = &project.arrangement {
            let mut lanes = Vec::new();
            for lane in &arrangement.lanes {
                collect_points(lane, transport_id, TimeUnit::Beats, &mut lanes);
            }
            for (points_timeline, unit) in lanes {
                for point in &points_timeline.points {
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
        // Each point, its place and the order it came in the file.
        let mut placed: Vec<(Ticks, TimeSignature, usize)> = Vec::new();
        for (points_index, (time, numerator, denominator, unit)) in points.into_iter().enumerate() {
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
            placed.push((at, signature, points_index));
        }
        // The points in the order the map holds them: by their place, and of
        // two at one tick the later in the file wins (R14).
        placed.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.cmp(&b.2)));
        let mut unique: Vec<(Ticks, TimeSignature)> = Vec::new();
        for (at, signature, _) in placed {
            if let Some(last) = unique.last_mut()
                && last.0 == at
            {
                *last = (at, signature);
            } else {
                unique.push((at, signature));
            }
        }
        for (at, signature) in unique {
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

        // A track of video or markers is no Beatlayer track (R36); its
        // markers are read as locators (R63).
        let types = &track.content_types;
        if !types.is_empty()
            && track.children.is_empty()
            && types
                .iter()
                .all(|kind| matches!(kind, ContentKind::Video | ContentKind::Markers))
        {
            let what = if types.contains(&ContentKind::Video) {
                "a video track"
            } else {
                "a track of markers"
            };
            self.report.add(what, &place, "left out");
            self.left_out.extend(track.info.id.clone());
            return;
        }

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
            self.make_main(track, channel);
            return;
        }
        if role == MixerRole::Vca {
            // Beatlayer has no VCA (R16).
            self.report.add("a VCA channel", &place, "left out");
            self.left_out.extend(track.info.id.clone());
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
                } else if types.contains(&ContentKind::Notes) {
                    // A track of audio and notes is split in two (R17).
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
        let color = track
            .info
            .color
            .as_deref()
            .or(channel.as_ref().and_then(|c| c.info.color.as_deref()));
        self.colour_track(id, color, place);
        if track.info.comment.is_some() {
            // Comments aren't kept, and are counted in the report (R20).
            self.report.add("a comment", place, "left out");
        }
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
            self.colour_track(audio_id, color, place);
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
    fn make_main(&mut self, track: &model::Track, channel: Option<Channel>) {
        let main = self.editor.document().main_id();
        // The channel's mute, solo, volume, pan and devices are handled by
        // the mixer pass, which lists what Main can't hold (R18).
        let made = Made {
            id: main,
            kind: TrackKind::Main,
            name: "Main".to_owned(),
            channel: channel.clone(),
        };
        self.made.push(made.clone());
        self.register(track, channel.as_ref(), &made);
        if let Some(color) = track.info.color.as_deref() {
            self.colour_track(main, Some(color), "the track \"Main\"");
        }
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
        self.colour_track(id, channel.info.color.as_deref(), &place);
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
        self.create_track_at(kind, name, parent, index)
    }

    /// Makes a track at `index` among its group's tracks, the top level's
    /// or the returns'.
    fn create_track_at(
        &mut self,
        kind: TrackKind,
        name: String,
        parent: Option<TrackId>,
        index: usize,
    ) -> TrackId {
        let id: TrackId = self.editor.new_id();
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
            self.register_mixer(track.id, channel);
            self.build_devices(track, channel, &place);
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
                if track.kind == TrackKind::Main {
                    // Main has no activator (R18).
                    self.report
                        .add("a muted master channel", &place, "left out");
                } else {
                    self.apply(bl_model::Command::SetMute {
                        track: track.id,
                        mute: true,
                    });
                }
            }
            if channel.solo == Some(true) {
                if track.kind == TrackKind::Main {
                    // Main can't be soloed (R18).
                    self.report
                        .add("a soloed master channel", &place, "left out");
                } else {
                    self.apply(bl_model::Command::SetSolo {
                        track: track.id,
                        solo: true,
                    });
                }
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

    /// Records what the channel's controls and sends drive (R56).
    fn register_mixer(&mut self, track: TrackId, channel: &Channel) {
        let mut controls: Vec<(Option<String>, Driven)> = Vec::new();
        let real = |parameter: &Option<model::RealParameter>, param| {
            parameter.as_ref().map(|p| {
                (
                    p.id.clone(),
                    Driven::Mixer {
                        track,
                        param,
                        unit: p.unit,
                        min: p.min,
                        max: p.max,
                    },
                )
            })
        };
        controls.extend(real(&channel.volume, bl_model::MixerParam::Volume));
        controls.extend(real(&channel.pan, bl_model::MixerParam::Pan));
        if let Some(mute) = &channel.mute {
            controls.push((
                mute.id.clone(),
                Driven::Mixer {
                    track,
                    param: bl_model::MixerParam::Mute,
                    unit: None,
                    min: None,
                    max: None,
                },
            ));
        }
        for (id, driven) in controls {
            if let Some(id) = id {
                self.targets.insert(id, driven);
            }
        }
    }

    /// A volume as a linear gain (R21).
    fn volume_gain(&mut self, parameter: &model::RealParameter, place: &str) -> f64 {
        let Some(value) = parameter.value else {
            return 1.0;
        };
        let Some(gain) = volume_raw(parameter.unit, value, parameter.min, parameter.max) else {
            self.report.add(
                "a volume in a unit Beatlayer can't read",
                place,
                "left at its default",
            );
            return 1.0;
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
        pan_from(parameter.unit, value, parameter.min, parameter.max).unwrap_or(0.0)
    }

    /// The sends of every track (R22).
    fn apply_sends(&mut self, made: &[Made]) {
        // The sends to each return, and whether they are pre or post.
        let mut taps: BTreeMap<TrackId, (usize, usize)> = BTreeMap::new();
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
                if let Some(volume) = &send.volume
                    && let Some(id) = &volume.id
                {
                    self.targets.insert(
                        id.clone(),
                        Driven::Send {
                            track: track.id,
                            to: return_track,
                            unit: volume.unit,
                            min: volume.min,
                            max: volume.max,
                        },
                    );
                }
                for (parameter, what) in [
                    (send.pan.as_ref().and_then(|p| p.id.clone()), "a send's pan"),
                    (
                        send.enable.as_ref().and_then(|p| p.id.clone()),
                        "a send's Enable",
                    ),
                ] {
                    if let Some(parameter) = parameter {
                        self.targets.insert(parameter, Driven::Unsupported(what));
                    }
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
                    if tap == bl_model::SendTap::Pre {
                        "the return takes them pre"
                    } else {
                        "the return takes them post"
                    },
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
                    "left out; the track plays to its default output",
                );
                continue;
            };
            // A track holding audio and notes sends its audio track to the
            // same place (R17).
            let mut sources = vec![(track.id, track.kind)];
            if let Some(sibling) = self.audio_sibling(track.id) {
                sources.push((sibling, TrackKind::Audio));
            }
            for (source, kind) in sources {
                self.route(source, kind, target, main, &place);
            }
        }
    }

    /// Sends one track to a channel's track, or leaves it at its default
    /// output (R19).
    fn route(
        &mut self,
        source: TrackId,
        kind: TrackKind,
        target: TrackId,
        main: TrackId,
        place: &str,
    ) {
        if target == main {
            // Main is the default output for a track at the top, and a MIDI
            // track has no other output.
            if kind != TrackKind::Midi {
                self.apply(bl_model::Command::SetOutput {
                    track: source,
                    output: Output::Main,
                });
            }
            return;
        }
        if self.editor.document().parent(source) == Some(target) {
            // The track's own group's channel is the default output.
            return;
        }
        let target_kind = self
            .editor
            .document()
            .track(target)
            .map(|track| track.kind());
        if target_kind != Some(TrackKind::Audio) || kind == TrackKind::Midi {
            // A MIDI track has no output but its default.
            if kind != TrackKind::Midi || target_kind != Some(TrackKind::Audio) {
                self.report.add(
                    "a destination that isn't a track's input",
                    place,
                    "left out; the track plays to its default output",
                );
            }
            return;
        }
        self.apply(bl_model::Command::SetOutput {
            track: source,
            output: Output::Track { track: target },
        });
        // The receiving track hears what is routed to it.
        self.apply(bl_model::Command::SetMonitor {
            track: target,
            monitor: Monitor::In,
        });
        self.apply(bl_model::Command::SetInput {
            track: target,
            input: bl_model::Input::None,
            channel: None,
        });
        if !self.received.contains(&target) {
            self.received.push(target);
        }
    }

    /// The clips of a track that monitors what is routed to it are silent
    /// (R19).
    fn report_received(&mut self) {
        for track in std::mem::take(&mut self.received) {
            let Some(track) = self.editor.document().track(track) else {
                continue;
            };
            if track.clips().next().is_some() {
                let place = format!("the track \"{}\"", track.name());
                self.report.add(
                    "a track that holds clips and monitors an input",
                    &place,
                    "the clips are silent while it monitors",
                );
            }
        }
    }

    /// The media a file names, or `None` when it isn't there (R38 to R41).
    pub(super) fn media_for(&mut self, file: &MediaFile, place: &str) -> Option<ContentHash> {
        let Some(reference) = file.file.as_ref() else {
            self.report
                .add("an audio element with no file", place, "left out");
            return None;
        };
        let path = reference.path.replace('\\', "/");
        let resolved = match self.resolved.get(&path) {
            Some(resolved) => resolved.clone(),
            None => {
                let resolved = self.resolve_media(&path, reference.external, file, place);
                self.resolved.insert(path.clone(), resolved.clone());
                resolved
            }
        };
        // Every clip that plays a file nobody has is listed (R40).
        resolved
            .map_err(|reason| {
                self.report
                    .add(&format!("the file `{path}`"), place, &reason);
            })
            .ok()
    }

    /// The media a file names, read from the container or from the disk
    /// (R38 to R41).
    fn resolve_media(
        &mut self,
        path: &str,
        external: bool,
        file: &MediaFile,
        place: &str,
    ) -> Result<ContentHash, String> {
        // A file on disk is read where it is; a file of the container is
        // unpacked once into a spool file, which is hashed and decoded and
        // then dropped, so no file is ever held in memory (R5).
        let mut found = PathBuf::new();
        let spool;
        let at: &Path = if external {
            match self.find_external(path) {
                Some((at, moved)) => {
                    if moved {
                        let shown = at.strip_prefix(self.context.folder).unwrap_or(&at);
                        self.report.add(
                            &format!("the file `{path}`"),
                            place,
                            &format!("found at {}", shown.to_string_lossy().replace('\\', "/")),
                        );
                    }
                    found = at;
                    &found
                }
                None => {
                    return Err("left out; it wasn't found".to_owned());
                }
            }
        } else {
            if self.container.size_of(path).is_none() {
                return Err("left out; the container doesn't hold it".to_owned());
            }
            spool = Spool::unpack(self.container, path)
                .map_err(|_| "left out; it couldn't be read".to_owned())?;
            spool.path()
        };
        let Ok(hash) = bl_media::ContentHash::of_file(at).map(|hash| ContentHash(*hash.as_bytes()))
        else {
            return Err("left out; it couldn't be read".to_owned());
        };
        let size = std::fs::metadata(at).map_or(0, |meta| meta.len());
        // The file has to decode to be played (R41).
        let Some((sample_rate, channels, frames)) = probe(at) else {
            return Err("left out; it isn't a file Beatlayer can play".to_owned());
        };
        self.check_media_agreement(file, sample_rate, channels, frames, path, place);
        // Files with the same bytes are one medium (R39).
        if !self.files.iter().any(|file| file.hash == hash) {
            self.files.push(Extracted {
                path: path.to_owned(),
                bytes: None,
                on_disk: external.then_some(found),
                hash,
                size,
            });
            if let Ok(relative) = MediaPath::new(path) {
                self.media.insert(MediaRef {
                    hash,
                    path: relative,
                    size: 0,
                    reversed_from: None,
                });
            }
        }
        Ok(hash)
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
    /// (R40): in the `.dawproject` file's folder and the folders inside it,
    /// the first match by name in path order being used.
    fn find_external(&mut self, path: &str) -> Option<(PathBuf, bool)> {
        let direct = if is_absolute(path) {
            PathBuf::from(path)
        } else {
            self.context.folder.join(path)
        };
        if direct.is_file() {
            return Some((direct, false));
        }
        let name = path
            .rsplit(['/', '\\'])
            .next()
            .filter(|name| !name.is_empty())?;
        let folder = self.context.folder;
        self.search
            .get_or_insert_with(|| index_by_name(folder))
            .get(&super::folded(name))
            .map(|found| (found.clone(), true))
    }

    /// The scenes of the Session View (R42, R43).
    fn build_scenes(&mut self, project: &DawProject) {
        if project.scenes.is_empty() {
            // A file with no scenes gets the eight a new set has (R42).
            return;
        }
        // The set holds the file's scenes, no more and no fewer.
        while self.editor.document().scene_count() > project.scenes.len() {
            let Some(last) = self.editor.document().scenes().next_back().map(|s| s.id) else {
                break;
            };
            self.apply(bl_model::Command::DeleteScene { scene: last });
        }
        while self.editor.document().scene_count() < project.scenes.len() {
            let id = self.editor.new_id();
            let index = self.editor.document().scene_count();
            self.apply(bl_model::Command::CreateScene { id, index });
        }
        for (index, scene) in project.scenes.iter().enumerate() {
            let Some(scene_id) = self.editor.document().scenes().nth(index).map(|s| s.id) else {
                break;
            };
            self.colour_scene(
                scene_id,
                scene.info.color.as_deref(),
                &scene_place_of(scene, index),
            );
            if scene.info.comment.is_some() {
                // Comments aren't kept, and are counted in the report (R20).
                self.report
                    .add("a comment", &scene_place_of(scene, index), "left out");
            }
            if let Some(name) = &scene.info.name {
                self.apply(bl_model::Command::RenameScene {
                    scene: scene_id,
                    name: name.clone(),
                });
            }
            let scene_place = scene_place_of(scene, index);
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
                        &scene_place,
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
                        &scene_place,
                        "left out",
                    );
                    continue;
                }
                if seen.contains(track_id) {
                    self.report.add(
                        "a second slot for one track in one scene",
                        &scene_place,
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
                let lane = Lane {
                    id: track,
                    kind,
                    place: format!("the track \"{name}\""),
                };
                let lane = self.lane_for(clip, &lane);
                self.scene = Some(index + 1);
                let mut pieces = self.clips_in(clip, TimeUnit::Beats, Ticks::ZERO, &lane, 0);
                self.scene = None;
                if pieces.len() > 1 {
                    self.report.add_count(
                        "a clip beyond the first a slot's clip flattens to",
                        &scene_place,
                        "left out",
                        pieces.len() - 1,
                    );
                }
                if pieces.is_empty() {
                    continue;
                }
                let piece = pieces.swap_remove(0);
                let mut clip = piece.clip;
                clip.start = Ticks::ZERO;
                // The clip's automation envelopes are set once it is in
                // its slot, where the document accepts them (R59).
                let (automation, modulation): (Vec<_>, Vec<_>) = clip
                    .envelopes
                    .drain(..)
                    .partition(|held| held.kind == bl_model::EnvelopeKind::Automation);
                clip.envelopes = modulation;
                let clip_id = clip.id;
                let command = bl_model::Command::CreateSlotClip {
                    track: lane.id,
                    scene: scene_id,
                    clip,
                };
                match self.editor.apply(command) {
                    Ok(_) => {
                        for held in automation {
                            self.try_apply(
                                bl_model::Command::SetClipEnvelope {
                                    clip: clip_id,
                                    target: held.target,
                                    kind: held.kind,
                                    points: held.envelope.points().collect(),
                                },
                                "a clip's automation",
                                &piece.place,
                            );
                        }
                    }
                    Err(error) => self.report.add(
                        "a clip Beatlayer can't hold",
                        &piece.place,
                        &format!("left out; {error}"),
                    ),
                }
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
                .get(reference.as_str())
                .is_some_and(|timeline| timeline_is_audio(timeline, 0));
        }
        source
            .content
            .as_ref()
            .and_then(|content| content.timeline())
            .is_some_and(|timeline| timeline_is_audio(timeline, 0))
    }

    /// Applies a command the file may ask for something the set can't hold
    /// in: a refusal is listed, and the import goes on.
    fn try_apply(&mut self, command: bl_model::Command, what: &str, place: &str) {
        if let Err(error) = self.editor.apply(command) {
            self.report.add(what, place, &format!("left out; {error}"));
        }
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
    // A beat is a quarter note (R9), and a time becomes the nearest tick.
    Ticks((beats * Ticks::QUARTER.get() as f64).round() as i64)
}

/// What a file holds, when it decodes: its rate, its channels and its
/// length in frames.
fn probe(at: &Path) -> Option<(u32, u16, Option<u64>)> {
    bl_media::Decoder::open(at).ok().map(|decoder| {
        let info = decoder.info();
        (info.sample_rate, info.channels, info.frames)
    })
}

/// A file of the container unpacked into the system's temporary folder,
/// taken back when dropped.
struct Spool(PathBuf);

impl Spool {
    /// Unpacks the container's file at `path`, keeping its extension, which
    /// the decoder uses to tell its format.
    fn unpack(container: &Container, path: &str) -> std::io::Result<Self> {
        static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let extension = Path::new(path)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("bin");
        let number = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let at = std::env::temp_dir().join(format!(
            "bl-dawproject-{}-{number}.{extension}",
            std::process::id()
        ));
        let mut file = std::fs::File::create_new(&at)?;
        let spool = Self(at);
        container.copy_to(path, &mut file)?;
        Ok(spool)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Spool {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
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

/// Whether a folder's entry is a folder, following a link as `Path::is_dir`
/// does; the entry's type costs no further look at the disk.
fn is_folder(entry: &std::fs::DirEntry) -> bool {
    entry
        .file_type()
        .is_ok_and(|kind| kind.is_dir() || kind.is_symlink() && entry.path().is_dir())
}

/// The folders inside a folder, a few levels deep and at most `budget` of
/// them, so a file opened from a huge folder doesn't search for long.
fn walk(folder: &Path, depth: usize, budget: &mut usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if depth > 4 {
        return found;
    }
    let Ok(entries) = std::fs::read_dir(folder) else {
        return found;
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .filter(is_folder)
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    for path in paths {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        found.push(path.clone());
        found.extend(walk(&path, depth + 1, budget));
    }
    found
}

/// The first file of each name in a folder and the folders inside it, taking
/// the folders in path order (R40). Names are folded, as the probe for a
/// name was on macOS and Windows, whose file systems ignore case.
fn index_by_name(root: &Path) -> HashMap<String, PathBuf> {
    #[cfg(test)]
    WALKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut folders = vec![root.to_path_buf()];
    folders.extend(walk(root, 0, &mut 10_000));
    folders.sort();
    let mut index = HashMap::new();
    for folder in folders {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        let mut files: Vec<_> = entries
            .flatten()
            .filter(|entry| {
                entry
                    .file_type()
                    .is_ok_and(|kind| kind.is_file() || kind.is_symlink() && entry.path().is_file())
            })
            .map(|entry| (entry.file_name(), entry.path()))
            .collect();
        files.sort();
        for (name, path) in files {
            index
                .entry(super::folded(&name.to_string_lossy()))
                .or_insert(path);
        }
    }
    index
}

/// How many times a folder has been searched, for the test.
#[cfg(test)]
static WALKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

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
fn collect_timelines<'a>(timeline: &'a Timeline, found: &mut Vec<(&'a str, Cow<'a, Timeline>)>) {
    if let Some(id) = &timeline.info().id {
        found.push((id, Cow::Borrowed(timeline)));
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
                    audio.info.id.as_deref().unwrap_or_default(),
                    Cow::Owned(Timeline::Audio(audio.clone())),
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

/// The lanes of each track in the order the file gives them, and the
/// timelines outside any clip.
fn collect_lanes(
    timeline: &Timeline,
    scope: Option<String>,
    unit: TimeUnit,
    lanes: &mut Vec<(String, Vec<(model::Clips, TimeUnit)>)>,
    loose: &mut Vec<(String, Timeline, TimeUnit)>,
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
            if let Some(scope) = scope {
                match lanes.iter_mut().find(|(known, _)| *known == scope) {
                    Some((_, found)) => found.push((clips.clone(), unit)),
                    None => lanes.push((scope, vec![(clips.clone(), unit)])),
                }
            }
        }
        Timeline::Notes(_) | Timeline::Audio(_) | Timeline::Warps(_) => {
            if let Some(scope) = scope {
                loose.push((scope, timeline.clone(), unit));
            }
        }
        _ => {}
    }
}

/// Where a scene is, for the report.
fn scene_place_of(scene: &model::Scene, index: usize) -> String {
    match scene.info.name.as_deref() {
        Some(name) => format!("the scene \"{name}\""),
        None => format!("scene {}", index + 1),
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

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::atomic::Ordering;

    use super::*;

    /// A 16-bit mono WAV file of a tenth of a second of silence.
    fn wav() -> Vec<u8> {
        let frames = 4_410u32;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + frames * 2).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&44_100u32.to_le_bytes());
        out.extend_from_slice(&88_200u32.to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(frames * 2).to_le_bytes());
        out.resize(out.len() + frames as usize * 2, 0);
        out
    }

    /// Spec: dawproject R40. Every file the project refers to that isn't
    /// where it says is looked for in one search of the folders, not one
    /// search each, and the first match in path order is still the one used.
    #[test]
    fn missing_files_walk_the_folder_once() {
        let folder = std::env::temp_dir().join(format!("bl-interop-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        for number in 0..50 {
            std::fs::create_dir_all(folder.join(format!("sub/{number:02}"))).unwrap();
        }
        std::fs::write(folder.join("sub/07/hit.wav"), wav()).unwrap();

        let mut clips = String::new();
        let names = (0..20)
            .map(|number| format!("/nowhere/a{number}.wav"))
            .chain(["/nowhere/hit.wav".to_owned()]);
        for (index, name) in names.enumerate() {
            clips.push_str(&format!(
                r#"<Clip time="{index}" duration="1" contentTimeUnit="seconds"><Audio><File path="{name}" external="true"/></Audio></Clip>"#
            ));
        }
        let xml = format!(
            r#"<?xml version="1.0"?><Project version="1.0"><Application name="t" version="1"/>
<Transport><Tempo unit="bpm" value="120" id="tempo"/></Transport>
<Structure><Track contentType="audio" id="a" name="A"><Channel role="regular" id="ca"/></Track></Structure>
<Arrangement id="arr"><Lanes id="all"><Lanes track="a" timeUnit="seconds" id="la"><Clips id="cl">{clips}</Clips></Lanes></Lanes></Arrangement></Project>"#
        );
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file("project.xml", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(xml.as_bytes()).unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let container = super::super::read_container(&bytes).unwrap();

        let before = WALKS.load(Ordering::Relaxed);
        let imported = import(&container, &Context::new(&folder, &folder)).unwrap();
        assert_eq!(WALKS.load(Ordering::Relaxed) - before, 1);

        let did = |what: &str| -> Vec<String> {
            imported
                .report
                .entries
                .iter()
                .filter(|entry| entry.what == format!("the file `{what}`"))
                .map(|entry| entry.did.clone())
                .collect()
        };
        assert_eq!(did("/nowhere/hit.wav"), ["found at sub/07/hit.wav"]);
        for number in 0..20 {
            assert_eq!(
                did(&format!("/nowhere/a{number}.wav")),
                ["left out; it wasn't found"]
            );
        }
        assert_eq!(imported.files.len(), 1);
        let _ = std::fs::remove_dir_all(&folder);
    }
}

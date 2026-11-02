//! The document: the whole song as an immutable, persistent tree.

use bl_core::id::Id;
use bl_core::time::{TempoMap, TempoSegment, Ticks, TimeSignatureMap};
use imbl::{OrdMap, Vector};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::clip::{Clip, ClipData};
use crate::device::Device;
use crate::groove::GroovePool;
use crate::ids::{ChainId, ClipId, DeviceId, GrooveId, LocatorId, SceneId, TakeLaneId, TrackId};
use crate::locator::{Locator, SongStart};
use crate::metronome::MetronomeSettings;
use crate::pad_layout::PadLayout;
use crate::rack::Chain;
use crate::record_quantization::RecordQuantization;
use crate::scale::Scale;
use crate::scene::Scene;
use crate::shared::Shared;
use crate::track::{Track, TrackKind};
use crate::tuning::Tuning;

/// Song-wide settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Settings {
    /// The tempo map. The default is 120 BPM throughout.
    pub tempo: TempoMap,
    /// The time-signature map. The default is 4/4 throughout.
    pub time_signatures: TimeSignatureMap,
    /// The Main track's crossfader, from 0 (the A end) through 0.5 (the
    /// centre, where it starts) to 1 (the B end), as the Crossfader section
    /// shows it (the mixer spec, R23). A set at the centre doesn't write it,
    /// so a set saved before WP-3.17 loads unchanged.
    #[serde(default = "centre", skip_serializing_if = "is_centre")]
    pub crossfader: f64,
    /// The Arrangement loop: the loop brace and the Loop switch. A set saved
    /// before WP-1.5 has none and loads with the default; a set whose loop is
    /// the default doesn't write it, so the saved form of such a set is
    /// unchanged.
    #[serde(default, skip_serializing_if = "ArrangementLoop::is_default")]
    pub arrangement_loop: ArrangementLoop,
    /// Solo in Place: whether soloing an audio, MIDI, or group track keeps
    /// the return tracks audible, so the soloed track is heard with its
    /// sends (the mixer spec). On in a new set. A set saved before
    /// WP-1.6 has none and loads with it on; a set with it on doesn't write
    /// it, so the saved form of such a set is unchanged.
    #[serde(default = "on", skip_serializing_if = "is_on")]
    pub solo_in_place: bool,
    /// Cue mode: whether the tracks' Solo switches are Cue switches that
    /// feed the Cue Out instead of silencing the other tracks (the mixer
    /// spec). Off in a new set, and not written when off. It plays only
    /// while Main's Cue Out and Main Out don't overlap
    /// ([`Document::cue_active`]).
    #[serde(default, skip_serializing_if = "is_off")]
    pub cue_mode: bool,
    /// The Preview/Cue Volume knob on Main's strip: linear gain from 0 to
    /// [`MAX_CUE_VOLUME`], 1 (0 dB) in a new set, where the knob's reset
    /// goes. A set at the default doesn't write it.
    #[serde(default = "unity", skip_serializing_if = "is_unity")]
    pub cue_volume: f64,
    /// Metronome settings: enabled, count-in, sound, rhythm, and only while
    /// recording (WP-2.15). A set whose metronome is default doesn't write it.
    #[serde(default, skip_serializing_if = "MetronomeSettings::is_default")]
    pub metronome: MetronomeSettings,
    /// Punch-In: whether recording into the Arrangement starts at the loop
    /// brace's start (WP-2.13). Off in a new set; a set with it off doesn't
    /// write it.
    #[serde(default, skip_serializing_if = "is_off")]
    pub punch_in: bool,
    /// Punch-Out: whether recording into the Arrangement ends at the loop
    /// brace's end (WP-2.13). Off in a new set; a set with it off doesn't
    /// write it.
    #[serde(default, skip_serializing_if = "is_off")]
    pub punch_out: bool,
    /// MIDI Arrangement Overdub: whether notes recorded into the
    /// Arrangement merge into the MIDI clips already there instead of
    /// replacing them (WP-2.14). Off in a new set, and not written when off.
    #[serde(default, skip_serializing_if = "is_off")]
    pub midi_overdub: bool,
    /// Record Quantization: the grid the notes a recording makes are moved
    /// onto (the recording spec, R17). No Quantization in a new set and in a
    /// set saved before it existed, and not written then.
    #[serde(default, skip_serializing_if = "RecordQuantization::is_none")]
    pub record_quantization: RecordQuantization,
    /// The last Record Quantization value other than No Quantization that
    /// was chosen, which switching record quantising back on returns to.
    /// Not written while it is the first one a switch picks.
    #[serde(
        default = "last_record_quantization",
        skip_serializing_if = "is_first_record_quantization"
    )]
    pub record_quantization_last: RecordQuantization,
    /// Delay Compensation: whether the engine holds every path back so
    /// signals through high-latency devices reach the Main output aligned
    /// (the delay-compensation spec, manual 23.3). On in a new set; a set
    /// with it on doesn't write it.
    #[serde(default = "on", skip_serializing_if = "is_on")]
    pub delay_compensation: bool,
    /// Reduced Latency When Monitoring: whether a track with input monitoring
    /// on is given the lowest possible latency at the cost of lining up with
    /// the rest of the set (the delay-compensation spec, manual 23.3). Off in
    /// a new set; a set with it off doesn't write it. Only meaningful while
    /// delay compensation is on.
    #[serde(default, skip_serializing_if = "is_off")]
    pub reduced_latency_monitoring: bool,
    /// Lock Envelopes: whether the tracks' envelopes stay where they are in
    /// the song when their clips are edited (the automation editing spec,
    /// R8). Off in a new set, and in a set saved before it.
    #[serde(default, skip_serializing_if = "is_off")]
    pub lock_envelopes: bool,
    /// The song's locators: the markers in the Arrangement's scrub area
    /// (WP-6.24). A set saved before locators has none and loads with none;
    /// a set with none doesn't write them, so its saved form is unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locators: Vec<Locator>,
    /// Where the song starts: a locator or the loop brace's start (WP-6.24).
    /// A set saved before the song start time has none and loads with none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub song_start: Option<SongStart>,
    /// The set's grooves (WP-6.19): the Groove Pool, the Global Amount, and
    /// Auto Load Groove with its groove. A set saved before grooves loads
    /// with an empty pool, a Global Amount of 100 % and Auto Load Groove off,
    /// and a set holding none doesn't write them.
    #[serde(default, skip_serializing_if = "GroovePool::is_default")]
    pub grooves: GroovePool,
    /// The set's current scale (WP-6.7): its Scale Mode switch, its root and
    /// its scale name, which the Control Bar's scale controls show and which
    /// a new clip takes. Off, at C, in Major, in a new set, and not written
    /// then, so a set saved before the scales loads unchanged.
    #[serde(default, skip_serializing_if = "Scale::is_default")]
    pub scale: Scale,
    /// How a note-playing controller lays the scale out on its pads (the
    /// notes on Push 2 spec, R32 and R37). In Key, 4ths, Vertical and Fixed
    /// off in a new set, and not written then. Changing it is no undo step.
    #[serde(default, skip_serializing_if = "PadLayout::is_default")]
    pub pad_layout: PadLayout,
    /// The set's tuning system (WP-6.9; the tunings spec, R1 and R11), saved
    /// whole inside the set so it opens without the file it came from. None
    /// plays twelve equal steps, and a set saved before tunings has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tuning: Option<Tuning>,
    /// The archives of the set's ARA plug-ins (IMP-20), taken from the
    /// running plug-ins when the set is saved and given back when it opens.
    /// A set with no ARA plug-in doesn't write them.
    ///
    /// Shared, so the copies of the document that every command and undo
    /// step make don't each copy the archives' bytes.
    #[serde(default, skip_serializing_if = "no_archives")]
    pub ara_archives: std::sync::Arc<Vec<crate::ara::AraArchive>>,
    /// The set's MIDI and key mappings (WP-7.1; the remote mapping spec, R1
    /// to R6). A set saved before them loads with none, and a set with none
    /// doesn't write them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remote: Vec<crate::remote::RemoteMapping>,
}

impl Default for Settings {
    /// 120 BPM and 4/4 throughout, the crossfader centred, the default loop
    /// brace with the loop off, Solo in Place on, and default metronome
    /// settings.
    fn default() -> Self {
        Self {
            tempo: TempoMap::default(),
            time_signatures: TimeSignatureMap::default(),
            crossfader: centre(),
            arrangement_loop: ArrangementLoop::default(),
            solo_in_place: true,
            cue_mode: false,
            cue_volume: 1.0,
            metronome: MetronomeSettings::default(),
            punch_in: false,
            punch_out: false,
            midi_overdub: false,
            record_quantization: RecordQuantization::None,
            record_quantization_last: RecordQuantization::FIRST_ON,
            delay_compensation: true,
            reduced_latency_monitoring: false,
            lock_envelopes: false,
            locators: Vec::new(),
            song_start: None,
            grooves: GroovePool::default(),
            scale: Scale::default(),
            pad_layout: PadLayout::default(),
            tuning: None,
            ara_archives: std::sync::Arc::default(),
            remote: Vec::new(),
        }
    }
}

/// The crossfader's centre, in fader positions.
fn centre() -> f64 {
    0.5
}

/// Whether the crossfader is at the centre, where a set's saved form leaves
/// it out.
fn is_centre(position: &f64) -> bool {
    *position == centre()
}

fn on() -> bool {
    true
}

/// The Cue Volume's top, +6 dB as linear gain.
pub const MAX_CUE_VOLUME: f64 = 1.995_262_314_968_879_5;

fn unity() -> f64 {
    1.0
}

fn is_unity(value: &f64) -> bool {
    *value == 1.0
}

/// For serde's `skip_serializing_if`, which passes a reference.
fn is_on(value: &bool) -> bool {
    *value
}

fn last_record_quantization() -> RecordQuantization {
    RecordQuantization::FIRST_ON
}

fn is_first_record_quantization(value: &RecordQuantization) -> bool {
    *value == RecordQuantization::FIRST_ON
}

fn is_off(value: &bool) -> bool {
    !*value
}

fn no_archives(archives: &std::sync::Arc<Vec<crate::ara::AraArchive>>) -> bool {
    archives.is_empty()
}

/// The Arrangement loop: where the loop brace sits, and whether the Loop
/// switch is on. With it on, playback that reaches the brace's end jumps
/// back to its start (the transport spec).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ArrangementLoop {
    /// Where the brace starts: 0 (1.1.1) or later.
    pub start: Ticks,
    /// How long it is: more than 0.
    pub length: Ticks,
    /// Whether the Loop switch is on.
    pub enabled: bool,
}

impl ArrangementLoop {
    /// A new set's loop: four 4/4 bars from 1.1.1, switched off.
    pub const DEFAULT: Self = Self {
        start: Ticks::ZERO,
        length: Ticks(bl_core::time::TICKS_PER_QUARTER * 16),
        enabled: false,
    };

    /// Where the brace ends: its start plus its length.
    #[must_use]
    pub fn end(&self) -> Ticks {
        self.start.saturating_add(self.length)
    }

    /// Whether this is [`ArrangementLoop::DEFAULT`].
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::DEFAULT
    }
}

impl Default for ArrangementLoop {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A song: its settings, its tracks, and everything on them.
///
/// A `Document` is a value. Cloning it is cheap (a few reference-count
/// increments) and the clone never changes, so the compiler, autosave, and
/// analysis can each hold their own snapshot on other threads while the main
/// thread edits. An edit builds a new version that shares every node it
/// didn't touch with the old one, which is how the undo history stays small.
///
/// The only way to change a document is [`Document::apply`] with a
/// [`Command`](crate::Command), usually through an [`Editor`](crate::Editor).
///
/// The tracks form a tree:
///
/// - the top-level tracks, in order, where each group holds its own ordered
///   list of audio, MIDI, and group tracks;
/// - the return tracks, in order;
/// - the Main track.
///
/// That is also the display order: [`Document::track_order`].
///
/// A document serialises as that tree, with each group's tracks inline.
/// Deserialising checks every rule a command would, and fails with a
/// [`ModelError`](crate::ModelError) message if the input breaks one.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    into = "crate::repr::DocumentRepr",
    try_from = "crate::repr::DocumentRepr"
)]
pub struct Document {
    pub(crate) settings: Settings,
    pub(crate) main: TrackId,
    pub(crate) top: Vector<TrackId>,
    pub(crate) returns: Vector<TrackId>,
    pub(crate) tracks: OrdMap<TrackId, Shared<Track>>,
    /// The scenes, in order, top to bottom. Each audio and MIDI track has a
    /// clip slot in every scene; the clips live in the tracks.
    pub(crate) scenes: Vector<Scene>,
    pub(crate) index: Index,
    /// How many mappings the load that made this document dropped because
    /// their control isn't in the set (remote mapping R6). Not saved, and
    /// not part of what makes two documents equal.
    pub(crate) remote_dropped: usize,
}

/// Lookups derived from the tree, kept in step with it by every command and
/// rebuilt when a document is loaded. They're never saved.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Index {
    /// The track that holds each clip: on its main lane, on one of its take
    /// lanes or in one of its Session slots.
    pub(crate) clip_track: OrdMap<ClipId, TrackId>,
    /// The track that holds each take lane (WP-6.21).
    pub(crate) lane_track: OrdMap<TakeLaneId, TrackId>,
    /// The track whose chain holds each device, at any depth: a device in a
    /// rack's chain belongs to the rack's track.
    pub(crate) device_track: OrdMap<DeviceId, TrackId>,
    /// The track that holds each rack chain, at any depth (WP-5.1).
    pub(crate) chain_track: OrdMap<ChainId, TrackId>,
    /// The group that holds each grouped track.
    pub(crate) parent: OrdMap<TrackId, TrackId>,
}

impl Index {
    /// Records a device and everything inside it as belonging to `track`.
    pub(crate) fn add_device(&mut self, device: &Device, track: TrackId) {
        for device in device.walk() {
            self.device_track.insert(device.id, track);
            for chain in &device.chains {
                self.chain_track.insert(chain.id, track);
            }
        }
    }

    /// Records a rack chain and everything inside it as belonging to
    /// `track`.
    pub(crate) fn add_chain(&mut self, chain: &Chain, track: TrackId) {
        self.chain_track.insert(chain.id, track);
        for device in &chain.devices {
            self.add_device(device, track);
        }
    }

    /// Forgets a rack chain and everything inside it.
    pub(crate) fn remove_chain(&mut self, chain: &Chain) {
        self.chain_track.remove(&chain.id);
        for device in &chain.devices {
            self.remove_device(device);
        }
    }

    /// Forgets a device and everything inside it.
    pub(crate) fn remove_device(&mut self, device: &Device) {
        for device in device.walk() {
            self.device_track.remove(&device.id);
            for chain in &device.chains {
                self.chain_track.remove(&chain.id);
            }
        }
    }
}

impl Document {
    /// An empty song: default settings, a Main track with this ID, and one
    /// unnamed scene per given ID, in order. A new set's scenes are the
    /// eight a project's allocator hands out ([`Editor::new`](crate::Editor::new));
    /// a document may also hold none.
    #[must_use]
    pub fn new(main: TrackId, scenes: impl IntoIterator<Item = SceneId>) -> Self {
        let mut tracks = OrdMap::new();
        tracks.insert(
            main,
            Shared::new(Track::new(main, TrackKind::Main, "Main".to_owned())),
        );
        Self {
            settings: Settings::default(),
            main,
            top: Vector::new(),
            returns: Vector::new(),
            tracks,
            scenes: scenes.into_iter().map(Scene::new).collect(),
            index: Index::default(),
            remote_dropped: 0,
        }
    }

    /// The song-wide settings.
    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Whether Cue mode can be on: Main's Cue Out and Main Out share no
    /// output channel (the mixer spec, S10). Live greys out the Cue
    /// Switches and the Main strip's Solo/Cue button otherwise.
    #[must_use]
    pub fn cue_available(&self) -> bool {
        let routing = self.main().routing();
        !routing.cue_out.overlaps(routing.main_out)
    }

    /// Whether the tracks' Solo switches are Cue switches now: the set is in
    /// Cue mode and it can be.
    #[must_use]
    pub fn cue_active(&self) -> bool {
        self.settings.cue_mode && self.cue_available()
    }

    /// A copy of the document whose tempo map or time-signature map is
    /// replaced, for what a launched scene's tempo or signature overrides
    /// while it holds (WP-3.5). The set itself keeps its maps: only the
    /// engine's compiler plays the copy.
    #[must_use]
    pub fn with_maps(
        &self,
        tempo: Option<TempoMap>,
        time_signatures: Option<TimeSignatureMap>,
    ) -> Self {
        let mut copy = self.clone();
        if let Some(tempo) = tempo {
            copy.settings.tempo = tempo;
        }
        if let Some(time_signatures) = time_signatures {
            copy.settings.time_signatures = time_signatures;
        }
        copy
    }

    /// The Arrangement clips that lead the tempo (the warping spec, R43): a
    /// warped audio clip whose Lead/Follow switch is Lead, in display order,
    /// the one lowest in the track list last. A deactivated (muted) clip
    /// leads nothing. Each is `(clip, start, end)`.
    pub(crate) fn tempo_leaders(&self) -> Vec<(&Clip, Ticks, Ticks)> {
        self.track_order()
            .into_iter()
            .filter_map(|id| self.tracks.get(&id))
            .flat_map(|track| track.clips())
            .filter(|clip| !clip.mute)
            .filter(|clip| match &clip.data {
                ClipData::Audio(audio) => audio.warp && audio.tempo_leader,
                ClipData::Midi(_) => false,
            })
            .map(|clip| (clip, clip.start, clip.end()))
            .collect()
    }

    /// The video clip whose picture shows at `at` (the video spec, R10, R18):
    /// of the active video clips playing there, the one on the track lowest
    /// in the track list. `None` where none plays: black.
    #[must_use]
    pub fn video_clip_at(&self, at: Ticks) -> Option<&Clip> {
        self.track_order()
            .into_iter()
            .filter_map(|id| self.tracks.get(&id))
            .flat_map(|track| track.clips())
            .filter(|clip| !clip.mute && clip.start <= at && at < clip.end())
            .rfind(|clip| matches!(&clip.data, ClipData::Audio(audio) if audio.video))
    }

    /// The spans of the Arrangement clips that lead the tempo (R43), in
    /// display order: what the Main track's lane shows and where the tempo
    /// commands refuse.
    #[must_use]
    pub fn tempo_leader_spans(&self) -> Vec<(Ticks, Ticks)> {
        self.tempo_leaders()
            .into_iter()
            .map(|(_, start, end)| (start, end))
            .collect()
    }

    /// Whether any Arrangement clip leads the set's tempo (R45): while one
    /// does, the set's tempo is the clip's and `song.set_tempo` is refused.
    #[must_use]
    pub fn any_tempo_leader(&self) -> bool {
        !self.tempo_leaders().is_empty()
    }

    /// Whether `at` falls inside a leading clip's span (R45), where the
    /// tempo breakpoint commands are refused.
    #[must_use]
    pub fn tempo_leader_covers(&self, at: Ticks) -> bool {
        self.tempo_leaders()
            .into_iter()
            .any(|(_, start, end)| start <= at && at < end)
    }

    /// The set's effective tempo map (R44, R48): the Main track's Song Tempo
    /// envelope with each leading clip's tempo over its span, so a leader
    /// plays as if unwarped and every follower follows it. Over a leader the
    /// tempo is the clip's, stepping at each of its warp segments; where
    /// active leaders overlap, the one on the track lowest in the track list
    /// sets the tempo; outside every leader the Song Tempo envelope applies,
    /// its own shape and all. This is what the engine plays, what exports and
    /// previews use, and what the Main track's lane shows (R45).
    #[must_use]
    pub fn tempo_map(&self) -> TempoMap {
        let song = &self.settings.tempo;
        let leaders = self.tempo_leaders();
        if leaders.is_empty() {
            return song.clone();
        }
        // The ticks where the tempo can change: the Song Tempo envelope's own
        // segment starts, and each leader's span and segment boundaries.
        let mut ticks: Vec<Ticks> = song.segments().map(|segment| segment.start()).collect();
        for (_, start, end) in &leaders {
            ticks.push(*start);
            ticks.push(*end);
        }
        for (clip, start, end) in &leaders {
            let ClipData::Audio(audio) = &clip.data else {
                continue;
            };
            let start_offset = (audio.start.round() as i64) - clip.start.get();
            for marker in &audio.warp_markers {
                let at = Ticks(marker.beat.get() + start_offset);
                if at > *start && at < *end {
                    ticks.push(at);
                }
            }
        }
        ticks.retain(|&at| at >= Ticks::ZERO);
        ticks.sort_unstable();
        ticks.dedup();
        let mut segments = Vec::with_capacity(ticks.len() + 1);
        for (index, &at) in ticks.iter().enumerate() {
            // The last matching leader wins: the one lowest in the track list.
            let leading = leaders
                .iter()
                .filter(|(_, start, end)| *start <= at && at < *end)
                .filter_map(|(clip, start, _)| match &clip.data {
                    ClipData::Audio(audio) => {
                        let beat = audio.start + (at - *start).get() as f64;
                        audio.segment_bpm_at(beat)
                    }
                    ClipData::Midi(_) => None,
                })
                .next_back();
            let next = ticks.get(index + 1).copied();
            match (leading, next) {
                // A leader's tempo is steady between its warp segments.
                (Some(bpm), _) => segments.push(TempoSegment::Constant { start: at, bpm }),
                // Outside a leader the Song Tempo envelope's own shape holds.
                (None, Some(next)) => {
                    let (from, to) = (song.bpm_at(at), song.bpm_at(next));
                    segments.push(if from == to {
                        TempoSegment::Constant {
                            start: at,
                            bpm: from,
                        }
                    } else {
                        TempoSegment::Ramp {
                            start: at,
                            from,
                            to,
                        }
                    });
                }
                (None, None) => segments.push(TempoSegment::Constant {
                    start: at,
                    bpm: song.bpm_at(at),
                }),
            }
        }
        TempoMap::new(segments).unwrap_or_else(|_| song.clone())
    }

    /// The Main track's ID.
    #[must_use]
    pub fn main_id(&self) -> TrackId {
        self.main
    }

    /// The Main track.
    #[must_use]
    pub fn main(&self) -> &Track {
        self.tracks
            .get(&self.main)
            .expect("a document always has its Main track")
    }

    /// The scenes, in order, top to bottom.
    pub fn scenes(&self) -> impl ExactSizeIterator<Item = &Scene> + DoubleEndedIterator + '_ {
        self.scenes.iter()
    }

    /// The scene with this ID.
    #[must_use]
    pub fn scene(&self, id: SceneId) -> Option<&Scene> {
        self.scenes.iter().find(|scene| scene.id == id)
    }

    /// Where a scene sits among the scenes, counting from 0: its number
    /// less 1.
    #[must_use]
    pub fn scene_position(&self, id: SceneId) -> Option<usize> {
        self.scenes.iter().position(|scene| scene.id == id)
    }

    /// The song's locators, in time order.
    #[must_use]
    pub fn locators(&self) -> &[Locator] {
        &self.settings.locators
    }

    /// The locator with this ID.
    #[must_use]
    pub fn locator(&self, id: LocatorId) -> Option<&Locator> {
        self.settings
            .locators
            .iter()
            .find(|locator| locator.id == id)
    }

    /// Where the song starts, in ticks: its locator's position, or the loop
    /// brace's start. `None` when the song has no song start time, or when
    /// the locator it named is gone.
    #[must_use]
    pub fn song_start_ticks(&self) -> Option<Ticks> {
        match self.settings.song_start? {
            SongStart::Locator(id) => self.locator(id).map(|locator| locator.position),
            SongStart::LoopBrace => Some(self.settings.arrangement_loop.start),
        }
    }

    /// The clips, on any track's Arrangement lane or in a Session slot, whose
    /// groove is `groove`.
    #[must_use]
    pub fn clips_using_groove(&self, groove: GrooveId) -> Vec<ClipId> {
        self.tracks
            .values()
            .flat_map(|track| track.clips().chain(track.slots().map(|(_, clip)| clip)))
            .filter(|clip| clip.groove == Some(groove))
            .map(|clip| clip.id)
            .collect()
    }

    /// Sets to none the groove of every clip whose groove `gone` picks.
    pub(crate) fn forget_grooves(&mut self, gone: impl Fn(GrooveId) -> bool) {
        let owners: Vec<(TrackId, ClipId)> = self
            .tracks
            .iter()
            .flat_map(|(id, track)| {
                track
                    .clips()
                    .chain(track.slots().map(|(_, clip)| clip))
                    .filter(|clip| clip.groove.is_some_and(&gone))
                    .map(|clip| (*id, clip.id))
                    .collect::<Vec<_>>()
            })
            .collect();
        for (track, clip) in owners {
            if let Some(clip) = self
                .tracks
                .get_mut(&track)
                .map(crate::shared::Shared::make_mut)
                .and_then(|track| track.clip_mut(clip))
            {
                clip.groove = None;
            }
        }
    }

    /// How many scenes there are.
    #[must_use]
    pub fn scene_count(&self) -> usize {
        self.scenes.len()
    }

    /// The clip in a track's slot at `scene`, if the slot holds one.
    #[must_use]
    pub fn slot(&self, track: TrackId, scene: SceneId) -> Option<&Clip> {
        self.track(track)?.slot(scene)
    }

    /// The track with this ID.
    #[must_use]
    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.get(&id).map(|track| &**track)
    }

    /// The tracks linked with this one, itself included, in display order:
    /// empty for a track that isn't in a link group (the linked-track editing
    /// spec, R1).
    #[must_use]
    pub fn linked_with(&self, id: TrackId) -> Vec<TrackId> {
        let Some(group) = self.track(id).and_then(Track::link) else {
            return Vec::new();
        };
        self.track_order()
            .into_iter()
            .filter(|t| self.track(*t).and_then(Track::link) == Some(group))
            .collect()
    }

    /// The tracks that aren't in a group, other than the returns and Main, in
    /// order.
    pub fn top_level(&self) -> impl ExactSizeIterator<Item = TrackId> + DoubleEndedIterator + '_ {
        self.top.iter().copied()
    }

    /// The return tracks, in order.
    pub fn returns(&self) -> impl ExactSizeIterator<Item = TrackId> + DoubleEndedIterator + '_ {
        self.returns.iter().copied()
    }

    /// The group a track is in, if it's in one.
    #[must_use]
    pub fn parent(&self, track: TrackId) -> Option<TrackId> {
        self.index.parent.get(&track).copied()
    }

    /// Every track ID in display order: the top-level tracks with each
    /// group's contents straight after the group, then the returns, then
    /// Main.
    #[must_use]
    pub fn track_order(&self) -> Vec<TrackId> {
        let mut order = Vec::with_capacity(self.tracks.len());
        let mut pending: Vec<TrackId> = self.top.iter().rev().copied().collect();
        while let Some(id) = pending.pop() {
            order.push(id);
            if let Some(track) = self.tracks.get(&id) {
                pending.extend(track.children.iter().rev().copied());
            }
        }
        order.extend(self.returns.iter().copied());
        order.push(self.main);
        order
    }

    /// Every track in display order (see [`Document::track_order`]).
    pub fn tracks(&self) -> impl Iterator<Item = &Track> {
        self.track_order()
            .into_iter()
            .filter_map(|id| self.track(id))
    }

    /// Whether a track is empty (the view options spec, R7): it holds no
    /// clip anywhere, on its lanes, take lanes or Session slots, and no
    /// device; a group is empty when it has no device and every track in it
    /// is empty. Main never is.
    #[must_use]
    pub fn is_track_empty(&self, id: TrackId) -> bool {
        self.track(id).is_some_and(|track| {
            track.kind() != TrackKind::Main
                && track.devices().len() == 0
                && track.all_clips().next().is_none()
                && track.children().all(|child| self.is_track_empty(child))
        })
    }

    /// The empty tracks (see [`Document::is_track_empty`]), in display order.
    #[must_use]
    pub fn empty_tracks(&self) -> Vec<TrackId> {
        self.track_order()
            .into_iter()
            .filter(|&id| self.is_track_empty(id))
            .collect()
    }

    /// How many tracks there are, counting the returns and Main.
    #[must_use]
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// The clip with this ID, whether it sits in the Arrangement or in a
    /// track's slot.
    #[must_use]
    pub fn clip(&self, id: ClipId) -> Option<&Clip> {
        let track = self.index.clip_track.get(&id)?;
        self.tracks.get(track)?.clip(id)
    }

    /// The track that holds a clip.
    #[must_use]
    pub fn clip_track(&self, id: ClipId) -> Option<TrackId> {
        self.index.clip_track.get(&id).copied()
    }

    /// The track that holds a take lane (the comping spec, R1).
    #[must_use]
    pub fn take_lane_track(&self, id: TakeLaneId) -> Option<TrackId> {
        self.index.lane_track.get(&id).copied()
    }

    /// The document with no take lane auditioned, as a freeze or a bounce
    /// renders it: they take the main lanes only (the freeze and bounce
    /// spec, R30).
    #[must_use]
    pub fn without_auditions(&self) -> Self {
        let mut plain = self.clone();
        let auditioned: Vec<TrackId> = plain
            .tracks
            .iter()
            .filter(|(_, track)| track.audition.is_some())
            .map(|(&id, _)| id)
            .collect();
        for id in auditioned {
            if let Some(track) = plain.tracks.get_mut(&id) {
                Shared::make_mut(track).audition = None;
            }
        }
        plain
    }

    /// The take lane with this ID.
    #[must_use]
    pub fn take_lane(&self, id: TakeLaneId) -> Option<&crate::clip::TakeLane> {
        self.track(self.take_lane_track(id)?)?.take_lane(id)
    }

    /// The take lane a clip sits on, if it sits on one rather than on its
    /// track's main lane or in a Session slot (the comping spec, R2).
    #[must_use]
    pub fn clip_lane(&self, id: ClipId) -> Option<TakeLaneId> {
        self.track(self.clip_track(id)?)?.clip_lane(id)
    }

    /// Every clip ID, in ID order.
    pub fn clip_ids(&self) -> impl ExactSizeIterator<Item = ClipId> + '_ {
        self.index.clip_track.keys().copied()
    }

    /// How many clips there are.
    #[must_use]
    pub fn clip_count(&self) -> usize {
        self.index.clip_track.len()
    }

    /// The device with this ID.
    #[must_use]
    pub fn device(&self, id: DeviceId) -> Option<&Device> {
        let track = self.tracks.get(self.index.device_track.get(&id)?)?;
        track.find_device(id)
    }

    /// The chain with this ID, in a rack at any depth.
    #[must_use]
    pub fn chain(&self, id: ChainId) -> Option<&Chain> {
        let track = self.tracks.get(self.index.chain_track.get(&id)?)?;
        track.find_chain(id)
    }

    /// The track that holds a rack's chain.
    #[must_use]
    pub fn chain_track(&self, id: ChainId) -> Option<TrackId> {
        self.index.chain_track.get(&id).copied()
    }

    /// The rack a chain belongs to.
    #[must_use]
    pub fn chain_rack(&self, id: ChainId) -> Option<&Device> {
        let track = self.tracks.get(self.index.chain_track.get(&id)?)?;
        track
            .devices
            .iter()
            .find_map(|device| device.chain_rack(id))
    }

    /// Where a device sits: its track, the rack chain it is in (none for the
    /// track's own chain), and its position there.
    #[must_use]
    pub fn device_place(&self, id: DeviceId) -> Option<crate::rack::DevicePlace> {
        let track_id = self.device_track(id)?;
        let track = self.track(track_id)?;
        if let Some(index) = track.device_position(id) {
            return Some(crate::rack::DevicePlace {
                track: track_id,
                chain: None,
                index,
            });
        }
        track
            .devices_deep()
            .flat_map(|rack| rack.chains.iter())
            .find_map(|chain| {
                let index = chain.devices.iter().position(|d| d.id == id)?;
                Some(crate::rack::DevicePlace {
                    track: track_id,
                    chain: Some(chain.id),
                    index,
                })
            })
    }

    /// The rack a chain belongs to and the chain's position in the rack's
    /// list.
    #[must_use]
    pub fn chain_place(&self, id: ChainId) -> Option<(DeviceId, usize)> {
        let rack = self.chain_rack(id)?;
        let index = rack.chains.iter().position(|c| c.id == id)?;
        Some((rack.id, index))
    }

    /// Every chain ID, in ID order.
    pub fn chain_ids(&self) -> impl ExactSizeIterator<Item = ChainId> + '_ {
        self.index.chain_track.keys().copied()
    }

    /// The track whose chain holds a device.
    #[must_use]
    pub fn device_track(&self, id: DeviceId) -> Option<TrackId> {
        self.index.device_track.get(&id).copied()
    }

    /// Every device ID, in ID order.
    pub fn device_ids(&self) -> impl ExactSizeIterator<Item = DeviceId> + '_ {
        self.index.device_track.keys().copied()
    }

    /// How many devices there are.
    #[must_use]
    pub fn device_count(&self) -> usize {
        self.index.device_track.len()
    }

    /// Whether any track, clip, device, or scene has this ID.
    #[must_use]
    pub fn contains(&self, id: Id) -> bool {
        self.tracks.contains_key(&TrackId(id))
            || self.index.clip_track.contains_key(&ClipId(id))
            || self.index.lane_track.contains_key(&TakeLaneId(id))
            || self.index.device_track.contains_key(&DeviceId(id))
            || self.index.chain_track.contains_key(&ChainId(id))
            || self.scenes.iter().any(|scene| scene.id.0 == id)
            || self
                .settings
                .locators
                .iter()
                .any(|locator| locator.id.0 == id)
            || self
                .settings
                .grooves
                .pool
                .iter()
                .any(|groove| groove.id.0 == id)
    }

    /// The largest ID of any track, clip, device, or scene.
    #[must_use]
    pub fn max_id(&self) -> Id {
        let track = self.tracks.get_max().map(|(id, _)| id.0);
        let clip = self.index.clip_track.get_max().map(|(id, _)| id.0);
        let lane = self.index.lane_track.get_max().map(|(id, _)| id.0);
        let device = self.index.device_track.get_max().map(|(id, _)| id.0);
        let chain = self.index.chain_track.get_max().map(|(id, _)| id.0);
        let scene = self.scenes.iter().map(|scene| scene.id.0).max();
        let locator = self
            .settings
            .locators
            .iter()
            .map(|locator| locator.id.0)
            .max();
        let groove = self
            .settings
            .grooves
            .pool
            .iter()
            .map(|groove| groove.id.0)
            .max();
        [track, clip, lane, device, chain, scene, locator, groove]
            .into_iter()
            .flatten()
            .max()
            .unwrap_or(self.main.0)
    }

    /// Checks that the document's derived lookups match its tree and that the
    /// tree follows every rule a loaded document must follow.
    ///
    /// A document built through commands or deserialisation always passes;
    /// this is for tests.
    ///
    /// # Errors
    ///
    /// A description of the first inconsistency found.
    pub fn check_invariants(&self) -> Result<(), String> {
        let rebuilt = Self::try_from(crate::repr::DocumentRepr::from(self))
            .map_err(|error| format!("the tree breaks a rule: {error}"))?;
        if rebuilt != *self {
            return Err("the track map holds tracks that aren't in the tree".to_owned());
        }
        if rebuilt.index != self.index {
            return Err("the derived lookups don't match the tree".to_owned());
        }
        Ok(())
    }
}

impl PartialEq for Document {
    /// Compares the songs, skipping any part both versions share.
    fn eq(&self, other: &Self) -> bool {
        self.main == other.main
            && self.settings == other.settings
            && (self.top.ptr_eq(&other.top) || self.top == other.top)
            && (self.returns.ptr_eq(&other.returns) || self.returns == other.returns)
            && self.tracks == other.tracks
            && (self.scenes.ptr_eq(&other.scenes) || self.scenes == other.scenes)
    }
}

#[cfg(test)]
mod tests {
    use bl_core::id::IdAllocator;

    use super::*;
    use crate::DEFAULT_SCENE_COUNT;
    use crate::command::Command;
    use crate::track::CrossfadeAssign;

    /// A new, empty song and the allocator that made it: its Main track and
    /// its default scenes have IDs, and the returned `n` track IDs are free
    /// to build tracks with.
    fn song(n: usize) -> (Document, IdAllocator, TrackId, Vec<TrackId>) {
        let mut ids = IdAllocator::new();
        let main = TrackId(ids.allocate());
        let scenes = (0..DEFAULT_SCENE_COUNT).map(|_| SceneId(ids.allocate()));
        let doc = Document::new(main, scenes);
        let tracks = (0..n).map(|_| TrackId(ids.allocate())).collect();
        (doc, ids, main, tracks)
    }

    fn create(id: TrackId, kind: TrackKind, parent: Option<TrackId>, index: usize) -> Command {
        Command::CreateTrack {
            id,
            kind,
            name: String::new(),
            parent,
            index,
        }
    }

    // Spec: session-view R1
    #[test]
    fn a_new_document_has_main_and_the_default_scenes() {
        let (doc, _, main, _) = song(0);
        assert_eq!(doc.track_count(), 1);
        assert_eq!(doc.main().kind(), TrackKind::Main);
        assert_eq!(doc.main().name(), "Main");
        assert_eq!(doc.track_order(), vec![main]);
        assert_eq!(doc.scene_count(), DEFAULT_SCENE_COUNT);
        assert!(doc.scenes().all(|scene| scene.name.is_empty()));
        assert_eq!(
            doc.max_id(),
            doc.scenes().last().expect("eight scenes").id.0,
            "the default scenes take the IDs after the Main track's"
        );
        assert!(doc.contains(main.0));
        assert_eq!(doc.settings(), &Settings::default());
        doc.check_invariants().unwrap();
    }

    #[test]
    fn display_order() {
        let (mut doc, _, main, t) = song(6);
        // A return created first still comes after every other track.
        doc.apply(&create(t[0], TrackKind::Return, None, 0))
            .unwrap();
        doc.apply(&create(t[1], TrackKind::Audio, None, 0)).unwrap();
        doc.apply(&create(t[2], TrackKind::Group, None, 1)).unwrap();
        doc.apply(&create(t[3], TrackKind::Midi, Some(t[2]), 0))
            .unwrap();
        doc.apply(&create(t[4], TrackKind::Group, Some(t[2]), 1))
            .unwrap();
        doc.apply(&create(t[5], TrackKind::Audio, Some(t[4]), 0))
            .unwrap();
        assert_eq!(
            doc.track_order(),
            vec![t[1], t[2], t[3], t[4], t[5], t[0], main]
        );
        assert_eq!(doc.top_level().collect::<Vec<_>>(), vec![t[1], t[2]]);
        assert_eq!(doc.returns().collect::<Vec<_>>(), vec![t[0]]);
        assert_eq!(doc.parent(t[5]), Some(t[4]));
        assert_eq!(doc.parent(t[4]), Some(t[2]));
        assert_eq!(doc.parent(t[2]), None);
        assert_eq!(
            doc.tracks().map(Track::id).collect::<Vec<_>>(),
            doc.track_order()
        );
        assert_eq!(doc.max_id(), t[5].0);
        doc.check_invariants().unwrap();
    }

    #[test]
    fn clones_share_and_stay_put() {
        let (mut doc, _, main, t) = song(1);
        let before = doc.clone();
        doc.apply(&create(t[0], TrackKind::Audio, None, 0)).unwrap();
        assert_eq!(before.track_count(), 1);
        assert_eq!(doc.track_count(), 2);
        assert_ne!(before, doc);
        // The untouched Main track is the same node in both versions.
        assert!(Shared::ptr_eq(
            before.tracks.get(&main).unwrap(),
            doc.tracks.get(&main).unwrap()
        ));
    }

    #[test]
    fn documents_cross_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Document>();
    }

    // Spec: mixer R24, R27
    #[test]
    fn cue_mode_and_the_cue_volume_save_and_load_back() {
        let (mut doc, ..) = song(0);
        doc.apply(&Command::SetCueOut {
            channel: crate::Channel::Pair(3),
        })
        .unwrap();
        doc.apply(&Command::SetCueMode { enabled: true }).unwrap();
        doc.apply(&Command::SetCueVolume { volume: 0.5 }).unwrap();
        let saved = serde_json::to_value(&doc).unwrap();
        assert_eq!(saved["settings"]["cue_mode"], serde_json::json!(true));
        assert_eq!(saved["settings"]["cue_volume"], serde_json::json!(0.5));
        let loaded: Document = serde_json::from_value(saved).unwrap();
        assert_eq!(loaded, doc);
        assert!(loaded.cue_active());
    }

    // Spec: push-2-notes R37. The pad layout saves with the set and loads back,
    // and a set at the default doesn't write it.
    #[test]
    fn the_pad_layout_saves_and_loads_back() {
        let (mut doc, ..) = song(0);
        assert!(
            serde_json::to_value(&doc).unwrap()["settings"]
                .get("pad_layout")
                .is_none()
        );
        doc.apply(&Command::SetPadLayout {
            in_key: Some(false),
            fixed: Some(true),
            spacing: Some(crate::PadSpacing::Thirds),
            direction: Some(crate::PadDirection::Horizontal),
        })
        .unwrap();
        let saved = serde_json::to_value(&doc).unwrap();
        assert_eq!(
            saved["settings"]["pad_layout"],
            serde_json::json!({
                "in_key": false,
                "fixed": true,
                "spacing": "thirds",
                "direction": "horizontal",
            })
        );
        let loaded: Document = serde_json::from_value(saved).unwrap();
        assert_eq!(loaded, doc);
    }

    // Spec: mixer R7, R8, R16
    #[test]
    fn the_mixer_is_saved_only_where_it_differs_from_a_new_set() {
        let (mut doc, _, _, t) = song(3);
        doc.apply(&create(t[0], TrackKind::Return, None, 0))
            .unwrap();
        doc.apply(&create(t[1], TrackKind::Audio, None, 0)).unwrap();
        doc.apply(&create(t[2], TrackKind::Group, None, 1)).unwrap();
        assert!(doc.settings().solo_in_place, "on in a new set");
        let saved = serde_json::to_value(&doc).unwrap();
        assert!(saved["settings"].get("solo_in_place").is_none());
        assert!(saved["returns"][0]["mixer"].get("tap").is_none());
        assert!(saved["tracks"][0].get("sends").is_none());

        doc.apply(&Command::SetSoloInPlace { enabled: false })
            .unwrap();
        doc.apply(&Command::SetSendTap {
            track: t[0],
            tap: crate::SendTap::Pre,
        })
        .unwrap();
        doc.apply(&Command::SetSend {
            track: t[1],
            to: t[0],
            level: 0.5,
        })
        .unwrap();
        let saved = serde_json::to_value(&doc).unwrap();
        assert_eq!(saved["settings"]["solo_in_place"], serde_json::json!(false));
        assert_eq!(
            saved["returns"][0]["mixer"]["tap"],
            serde_json::json!("pre")
        );
        assert_eq!(
            saved["tracks"][0]["sends"],
            serde_json::json!([[t[0].0.get(), 0.5]])
        );
        let loaded: Document = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(loaded, doc);
        loaded.check_invariants().unwrap();

        // A send to a track that isn't a return, or louder than 0 dB, doesn't
        // load.
        for (field, value) in [
            ("sends", serde_json::json!([[t[2].0.get(), 0.5]])),
            ("sends", serde_json::json!([[t[0].0.get(), 1.5]])),
            ("sends", serde_json::json!([[t[0].0.get(), 0.0]])),
        ] {
            let mut broken = saved.clone();
            broken["tracks"][0][field] = value;
            assert!(serde_json::from_value::<Document>(broken).is_err());
        }
        let mut broken = saved;
        broken["tracks"][0]["mixer"]["tap"] = serde_json::json!("pre");
        assert!(serde_json::from_value::<Document>(broken).is_err());
    }

    // Spec: mixer R23
    #[test]
    fn the_crossfader_and_the_assignments_save_only_where_they_differ() {
        let (mut doc, _, _, t) = song(2);
        doc.apply(&create(t[0], TrackKind::Audio, None, 0)).unwrap();
        doc.apply(&create(t[1], TrackKind::Midi, None, 1)).unwrap();
        // A new set: the fader is centred and no track is assigned, and
        // neither is written, so sets saved before WP-3.17 look the same.
        let saved = serde_json::to_value(&doc).unwrap();
        assert!(saved["settings"].get("crossfader").is_none());
        assert!(saved["tracks"][0]["mixer"].get("crossfade").is_none());
        assert!(saved["main"]["mixer"].get("crossfade").is_none());

        doc.apply(&Command::SetCrossfader { position: 0.25 })
            .unwrap();
        doc.apply(&Command::SetCrossfadeAssign {
            track: t[0],
            assign: CrossfadeAssign::B,
        })
        .unwrap();
        let saved = serde_json::to_value(&doc).unwrap();
        assert_eq!(saved["settings"]["crossfader"], serde_json::json!(0.25));
        assert_eq!(
            saved["tracks"][0]["mixer"]["crossfade"],
            serde_json::json!("b")
        );
        let loaded: Document = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(loaded, doc);
        loaded.check_invariants().unwrap();

        // A position outside 0 to 1, and an assignment on Main, don't load.
        let mut broken = saved.clone();
        broken["settings"]["crossfader"] = serde_json::json!(-1.0);
        assert!(serde_json::from_value::<Document>(broken).is_err());
        let mut broken = saved;
        broken["main"]["mixer"]["crossfade"] = serde_json::json!("a");
        assert!(serde_json::from_value::<Document>(broken).is_err());
    }

    // Spec: transport R18
    #[test]
    fn a_new_sets_loop_is_four_bars_from_the_start_and_off() {
        let (doc, _, _, _) = song(0);
        let brace = doc.settings().arrangement_loop;
        assert_eq!(brace.start, Ticks::ZERO);
        assert_eq!(brace.length, Ticks::WHOLE * 4);
        assert_eq!(brace.end(), Ticks::WHOLE * 4);
        assert!(!brace.enabled);
        assert!(brace.is_default());
    }

    // Spec: transport R16, R17
    #[test]
    fn the_loop_is_part_of_the_set_and_saved_when_it_isnt_the_default() {
        let (mut doc, _, _, _) = song(0);
        let saved = serde_json::to_value(&doc).unwrap();
        assert!(
            saved["settings"].get("arrangement_loop").is_none(),
            "a default loop isn't written, so sets saved before it look the same"
        );
        doc.apply(&Command::SetLoop {
            start: Ticks::WHOLE,
            length: Ticks::QUARTER * 3,
        })
        .unwrap();
        doc.apply(&Command::SetLoopEnabled { enabled: true })
            .unwrap();
        let saved = serde_json::to_value(&doc).unwrap();
        assert_eq!(
            saved["settings"]["arrangement_loop"],
            serde_json::json!({ "start": 2_822_400, "length": 2_116_800, "enabled": true })
        );
        let loaded: Document = serde_json::from_value(saved).unwrap();
        assert_eq!(loaded, doc);
        // A refused change leaves the brace as it was.
        let before = doc.clone();
        assert!(
            doc.apply(&Command::SetLoop {
                start: Ticks::ZERO,
                length: Ticks::ZERO,
            })
            .is_err()
        );
        assert_eq!(doc, before);
    }
}

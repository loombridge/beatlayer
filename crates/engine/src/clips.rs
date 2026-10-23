//! Audio clip playback (WP-1.4, WP-1.4b): each audio track's clip player,
//! the clips it plays, and where their audio comes from. The rules, and the
//! Live sources behind them, are in the audio clips spec.
//!
//! Every audio track has a [`ClipPlayer`] at the head of its device chain
//! ([`NodeKey::Player`](crate::NodeKey::Player)). The compiler works out the
//! track's clips off the audio thread: where each starts and ends in song
//! frames, through the tempo map; which frames of its file it plays, from
//! its Start and End markers and its loop; and its gain and fades in frames.
//! That list reaches the player as node data
//! ([`Sender::send_data`](crate::Sender::send_data)), so making, moving, or
//! editing a clip never builds a new program, and the player swaps the new
//! list in at a block boundary. The list it lets go of is freed on the drop
//! thread, never on the audio thread.
//!
//! Each clip reads its audio through a source of its own, a
//! [`bl_media::AudioSource`] that the registry's [`MediaLibrary`] opens:
//! [`MediaStreams`] streams long files from disk and holds short ones in
//! memory (WP-1.3's disk streaming), so memory use follows the streams'
//! buffers rather than the files' lengths. Opening a source reads the disk,
//! so a running engine's compiler never does it: a loader thread does, and
//! the clip joins its track's list when its source is ready (see
//! `sources.rs`). A source stays with its clip from list to list while the
//! clip stays on its track.
//!
//! An unwarped clip plays one frame of its file, at the engine's rate, to each
//! frame of the song, whatever the tempo, with its Start marker at the clip's
//! start in time. A warped clip (WP-4.2, `warped.rs`) plays the file position
//! its warp map and the tempo map put at each song frame, through its warp
//! mode.

use std::collections::BTreeMap;
use std::sync::Arc;

use bl_core::time::{SampleRate, TempoMap, Ticks};
use bl_media::stream::{AudioSource, MemorySource, PlanarMut, Source, StreamConfig, Transition};
use bl_media::{MediaStore, SampleData, StreamId, Streamer, StreamerStats, convert_sample_rate};
use bl_model::{
    AudioData, Clip, ClipData, ClipId, ContentHash, SampleEdit, SampleEditKind, Scale, SceneId,
    Track, TrackId, WarpMode,
};

use crate::clip_source::ClipSource;
use crate::mixer::{Ramp, scale, smoothing_frames};
use crate::node::{Block, Node, NodeConfig, NodeData, NodeEvent, ParamValue, segments};
use crate::scales::{ScaleRuns, Span, TrackScale};
use crate::sources::{Bank, ClipSources, Handoff, Supply};
use crate::warped::{
    BeatsMedia, CUE_BACK, Flat, Histories, History, Past, Playing, PoolSpec, Positions, Stretchers,
    WarpScratch, Warped, WindowBuf, window_of,
};

/// A clip's source, as a [`MediaLibrary`] opens it.
pub struct OpenedSource {
    /// What the clip player reads, on the audio thread.
    pub source: Box<dyn AudioSource>,
    /// How to preload points of a streamed source that a jump will land on;
    /// `None` for a source in memory, which has every point ready.
    pub cues: Option<Arc<dyn SourceCues>>,
}

impl std::fmt::Debug for OpenedSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenedSource")
            .field("channels", &self.source.channels())
            .field("frames", &self.source.frames())
            .field("streams", &self.cues.is_some())
            .finish()
    }
}

/// Preloads points of a streamed source, from the compiler thread, while
/// the audio thread reads it.
pub trait SourceCues: Send + Sync {
    /// Asks for the source's audio from frame `frame` (at the engine's rate)
    /// to be preloaded into cue slot `slot`, so a jump there plays at once:
    /// slot 0 holds the clip's start, 1 its loop's start, and 2 where the
    /// Arrangement loop starts. It returns at once; the disk thread reads.
    fn cue(&self, slot: usize, frame: u64);
}

/// Where the engine finds the audio that clips name by content hash.
///
/// [`open`](Self::open) may read and decode files, so a running engine
/// calls it on its loader thread, never on the compiler's or the audio
/// thread; an offline render calls it on its own thread. [`MediaStreams`]
/// is what the app uses: it checks each file still holds its media, streams
/// long files, and holds short ones in memory. The [`MediaStore`] alone
/// decodes each file whole into memory, and [`MediaMap`] holds audio made
/// in memory, for tests and previews.
pub trait MediaLibrary: Send + Sync {
    /// A source of `media` at the engine's `rate` for a clip that starts
    /// playing at file frame `start` (at `rate`), with that point ready to
    /// play; `None` if the media is missing, has changed, or doesn't decode.
    /// A clip without its audio plays silence.
    fn open(&self, media: ContentHash, rate: SampleRate, start: u64) -> Option<OpenedSource>;

    /// A source of `media` at `rate` that holds the whole file in memory,
    /// for a clip whose RAM switch is on (the warp modes spec, R11): the
    /// same audio as [`open`](Self::open), with nothing left to stream. The
    /// default is `open`'s, which is right for a library that holds every
    /// file in memory already.
    fn open_in_memory(&self, media: ContentHash, rate: SampleRate) -> Option<OpenedSource> {
        self.open(media, rate, 0)
    }

    /// The file's transients in seconds from its start, in order, as the
    /// analysis found them (the warping spec, R20 to R23): what a Beats clip
    /// cuts its segments at (the warp modes spec, R14). `None` where the
    /// library doesn't analyse, or its analysis isn't ready, so a clip whose
    /// transients aren't known segments on its warp markers alone. It reads
    /// nothing, so the compiler calls it on its own thread.
    fn transients(&self, media: ContentHash) -> Option<Arc<Vec<f64>>> {
        let _ = media;
        None
    }

    /// The media decoded into memory at `rate`, whole, for a device that
    /// reads it at positions of its own (a sampler's sample, WP-2.16):
    /// `None` if the media is missing, has changed, or doesn't decode, and
    /// the default has none, so a device given none plays silence. Decoding
    /// reads and converts, so call it off the audio thread; the audio is
    /// shared with every reader of the same file at that rate while it
    /// lives.
    fn sample(&self, media: ContentHash, rate: SampleRate) -> Option<Arc<SampleData>> {
        let _ = (media, rate);
        None
    }

    /// The media with a sampler's `edits` applied, at `rate` (the
    /// multisampler spec, R57): the edit list rendered over the decoded
    /// sample at its own rate, off the audio thread, and shared by source
    /// and list. `None` where the library can't (the default, for a list
    /// that isn't empty), or the media is missing or an edit doesn't fit.
    fn edited_sample(
        &self,
        media: ContentHash,
        rate: SampleRate,
        edits: &[SampleEdit],
    ) -> Option<Arc<SampleData>> {
        if edits.is_empty() {
            return self.sample(media, rate);
        }
        None
    }

    /// A file at the engine's own rate that a sampler reads by frame, its
    /// first `head` frames decoded now and the rest by a thread of its own
    /// (WP-5.13, the multisampler spec, R23); `None` where the library can't
    /// (the file's rate isn't the engine's, its length isn't known, or it
    /// decodes whole for a render that must repeat), when the caller reads
    /// [`sample`](Self::sample) instead.
    fn progressive(
        &self,
        media: ContentHash,
        rate: SampleRate,
        head: usize,
    ) -> Option<Arc<bl_media::ProgressiveSample>> {
        let _ = (media, rate, head);
        None
    }
}

/// Opens `media` as a progressive sample if it's at `rate`, and starts the
/// thread that fills it.
fn progressive_of(
    store: &MediaStore,
    media: ContentHash,
    rate: SampleRate,
    head: usize,
) -> Option<Arc<bl_media::ProgressiveSample>> {
    let hash = bl_media::ContentHash::from_bytes(media.0);
    let decoder = store.open(hash).ok()?;
    if decoder.info().sample_rate != rate.hz() {
        return None;
    }
    let (sample, fill) = bl_media::ProgressiveSample::start(decoder, head).ok()?;
    fill.spawn().ok()?;
    Some(sample)
}

/// A sampler's edit list as `bl-media` renders it.
#[must_use]
pub fn media_edits(edits: &[SampleEdit]) -> Vec<bl_media::Edit> {
    edits
        .iter()
        .map(|edit| bl_media::Edit {
            kind: match edit.kind {
                SampleEditKind::Trim => bl_media::EditKind::Trim,
                SampleEditKind::Cut => bl_media::EditKind::Cut,
                SampleEditKind::Reverse => bl_media::EditKind::Reverse,
                SampleEditKind::FadeIn => bl_media::EditKind::FadeIn,
                SampleEditKind::FadeOut => bl_media::EditKind::FadeOut,
                SampleEditKind::Normalize => bl_media::EditKind::Normalize,
            },
            range: edit.range.map(|range| (range.from, range.to)),
        })
        .collect()
}

/// A decoded sample for one of a device's sample slots: what the compiler
/// sends a node that plays media, once per filled slot (WP-2.16, WP-5.17).
/// The sampler has the one slot, 0.
#[derive(Clone)]
pub struct SlotSample {
    /// The slot, as the device's [`media`](bl_model::Device::media) list
    /// numbers them.
    pub slot: usize,
    /// The audio, at the engine's rate.
    pub sample: Arc<SampleData>,
    /// The device's warp configuration.
    pub warp: bl_model::DeviceWarp,
    /// Slices in frames from the sample's start.
    pub slices: Arc<[u64]>,
}

impl SlotSample {
    /// Makes a new slot sample with default warp and no slices.
    #[must_use]
    pub fn new(slot: usize, sample: Arc<SampleData>) -> Self {
        Self {
            slot,
            sample,
            warp: bl_model::DeviceWarp::default(),
            slices: Arc::from([]),
        }
    }
}

/// A request to a sampler's node to play a sample from a point, as a click in
/// its display does (WP-5.15; the multisampler spec, R54): the node starts a
/// voice at the sample's root pitch, through its settings, and ends it when
/// the button is released. It reaches the node as node data and changes
/// nothing in the set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Audition {
    /// The layer a multisampler plays; `None` for a device's own sample.
    pub layer: Option<bl_model::LayerId>,
    /// Where in the sample the voice starts, as a share of its length from
    /// 0 to 1.
    pub from: f64,
    /// Whether the button went down (`true`) or came up.
    pub down: bool,
}

/// A source in memory over `data`, which is at the engine's rate.
fn in_memory(data: Arc<SampleData>) -> OpenedSource {
    OpenedSource {
        source: Box::new(MemorySource::new(data)),
        cues: None,
    }
}

impl MediaLibrary for MediaStore {
    /// Decodes the whole file into memory at the engine's rate (shared with
    /// every other clip that plays it): for tools that want no disk thread.
    fn open(&self, media: ContentHash, rate: SampleRate, _start: u64) -> Option<OpenedSource> {
        let hash = bl_media::ContentHash::from_bytes(media.0);
        self.load_at_rate(hash, rate.hz()).ok().map(in_memory)
    }

    fn sample(&self, media: ContentHash, rate: SampleRate) -> Option<Arc<SampleData>> {
        let hash = bl_media::ContentHash::from_bytes(media.0);
        self.load_at_rate(hash, rate.hz()).ok()
    }

    fn edited_sample(
        &self,
        media: ContentHash,
        rate: SampleRate,
        edits: &[SampleEdit],
    ) -> Option<Arc<SampleData>> {
        let hash = bl_media::ContentHash::from_bytes(media.0);
        self.load_edited_at_rate(hash, rate.hz(), &media_edits(edits))
            .ok()
    }

    fn progressive(
        &self,
        media: ContentHash,
        rate: SampleRate,
        head: usize,
    ) -> Option<Arc<bl_media::ProgressiveSample>> {
        progressive_of(self, media, rate, head)
    }
}

/// The project's media, streamed from disk: files shorter than ten seconds
/// in memory, longer ones read by a disk thread a little ahead of where
/// each clip plays (the disk streaming spec), so memory use follows the
/// streams' buffers rather than the files' lengths.
///
/// Its reads never wait for the disk: audio that isn't there yet plays as
/// silence and counts as an underrun ([`MediaStreams::stats`]). An offline
/// render makes its own with [`MediaStreams::blocking`], whose reads wait
/// instead, so a render is the same every time.
pub struct MediaStreams {
    store: Arc<MediaStore>,
    streamer: Arc<Streamer>,
    blocking: bool,
}

impl MediaStreams {
    /// Streams `store`'s media with the default sizes, for playing in real
    /// time. Starts the disk thread.
    ///
    /// # Errors
    ///
    /// If the disk thread can't start.
    pub fn new(store: Arc<MediaStore>) -> std::io::Result<Self> {
        Self::with_config(store, StreamConfig::default())
    }

    /// Streams `store`'s media as `config` says.
    ///
    /// # Errors
    ///
    /// If the disk thread can't start.
    pub fn with_config(store: Arc<MediaStore>, config: StreamConfig) -> std::io::Result<Self> {
        Ok(Self {
            store,
            streamer: Arc::new(Streamer::start(config)?),
            blocking: false,
        })
    }

    /// Streams `store`'s media for an offline render: a read waits for the
    /// disk thread rather than play silence (the disk streaming spec, R8).
    /// Never use it for an engine on an audio device.
    ///
    /// # Errors
    ///
    /// If the disk thread can't start.
    pub fn blocking(store: Arc<MediaStore>) -> std::io::Result<Self> {
        Ok(Self {
            blocking: true,
            ..Self::new(store)?
        })
    }

    /// The media store it reads.
    #[must_use]
    pub fn store(&self) -> &Arc<MediaStore> {
        &self.store
    }

    /// The disk thread's figures: open streams, underruns (what the disk
    /// overload indicator shows), and how busy it is.
    #[must_use]
    pub fn stats(&self) -> StreamerStats {
        self.streamer.stats()
    }
}

impl std::fmt::Debug for MediaStreams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaStreams")
            .field("blocking", &self.blocking)
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

impl MediaLibrary for MediaStreams {
    fn open(&self, media: ContentHash, rate: SampleRate, start: u64) -> Option<OpenedSource> {
        let hash = bl_media::ContentHash::from_bytes(media.0);
        match self
            .streamer
            .source(&self.store, hash, rate.hz(), start)
            .ok()?
        {
            Source::Memory(source) => Some(OpenedSource {
                source: Box::new(source),
                cues: None,
            }),
            Source::Stream(mut source) => {
                source.set_blocking(self.blocking);
                let cues = StreamCues {
                    streamer: Arc::clone(&self.streamer),
                    stream: source.id(),
                };
                Some(OpenedSource {
                    source: Box::new(source),
                    cues: Some(Arc::new(cues)),
                })
            }
        }
    }

    fn open_in_memory(&self, media: ContentHash, rate: SampleRate) -> Option<OpenedSource> {
        self.sample(media, rate).map(in_memory)
    }

    /// Decodes the whole file into memory at the engine's rate, through the
    /// store it streams from: a device that reads the audio at positions of
    /// its own needs all of it.
    fn sample(&self, media: ContentHash, rate: SampleRate) -> Option<Arc<SampleData>> {
        let hash = bl_media::ContentHash::from_bytes(media.0);
        self.store.load_at_rate(hash, rate.hz()).ok()
    }

    fn edited_sample(
        &self,
        media: ContentHash,
        rate: SampleRate,
        edits: &[SampleEdit],
    ) -> Option<Arc<SampleData>> {
        let hash = bl_media::ContentHash::from_bytes(media.0);
        self.store
            .load_edited_at_rate(hash, rate.hz(), &media_edits(edits))
            .ok()
    }

    /// Only for playing in real time: a render that must repeat decodes
    /// whole.
    fn progressive(
        &self,
        media: ContentHash,
        rate: SampleRate,
        head: usize,
    ) -> Option<Arc<bl_media::ProgressiveSample>> {
        if self.blocking {
            return None;
        }
        progressive_of(&self.store, media, rate, head)
    }
}

/// Cues on one stream.
struct StreamCues {
    streamer: Arc<Streamer>,
    stream: StreamId,
}

impl SourceCues for StreamCues {
    fn cue(&self, slot: usize, frame: u64) {
        // Fails only if the slot is out of range or the disk thread has
        // stopped; the jump then waits for the disk, as one nothing was
        // cued for does.
        let _ = self.streamer.cue(self.stream, slot, frame);
    }
}

/// Audio held in memory, by content hash: a [`MediaLibrary`] for tests,
/// golden renders, and previews, which converts each to the engine's rate
/// when asked.
#[derive(Clone, Debug, Default)]
pub struct MediaMap {
    files: BTreeMap<ContentHash, Arc<SampleData>>,
    /// The files' transients, for the tests and golden renders that generate
    /// their media (the warp modes spec, R14).
    transients: BTreeMap<ContentHash, Arc<Vec<f64>>>,
}

impl MediaMap {
    /// No audio.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `audio` under `media`, replacing whatever was there.
    pub fn insert(&mut self, media: ContentHash, audio: Arc<SampleData>) {
        self.files.insert(media, audio);
    }

    /// Adds `media`'s transients, in seconds from its start: what a Beats
    /// clip cuts its segments at.
    pub fn set_transients(&mut self, media: ContentHash, transients: Vec<f64>) {
        self.transients.insert(media, Arc::new(transients));
    }

    /// The audio under `media`, at its own rate.
    #[must_use]
    pub fn get(&self, media: ContentHash) -> Option<&Arc<SampleData>> {
        self.files.get(&media)
    }

    /// How many files it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether it holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

impl MediaLibrary for MediaMap {
    fn transients(&self, media: ContentHash) -> Option<Arc<Vec<f64>>> {
        self.transients.get(&media).cloned()
    }

    fn open(&self, media: ContentHash, rate: SampleRate, _start: u64) -> Option<OpenedSource> {
        let audio = self.files.get(&media)?;
        if audio.sample_rate() == rate.hz() {
            return Some(in_memory(Arc::clone(audio)));
        }
        let converted = convert_sample_rate(audio, rate.hz()).ok()?;
        Some(in_memory(Arc::new(converted)))
    }

    fn sample(&self, media: ContentHash, rate: SampleRate) -> Option<Arc<SampleData>> {
        let audio = self.files.get(&media)?;
        if audio.sample_rate() == rate.hz() {
            return Some(Arc::clone(audio));
        }
        convert_sample_rate(audio, rate.hz()).ok().map(Arc::new)
    }

    /// Renders the edits each time it's asked, with no sharing.
    fn edited_sample(
        &self,
        media: ContentHash,
        rate: SampleRate,
        edits: &[SampleEdit],
    ) -> Option<Arc<SampleData>> {
        let audio = self.files.get(&media)?;
        let edited = bl_media::edits::apply(audio, &media_edits(edits)).ok()?;
        if edited.sample_rate() == rate.hz() {
            return Some(Arc::new(edited));
        }
        convert_sample_rate(&edited, rate.hz()).ok().map(Arc::new)
    }
}

/// Where a clip sits in song frames, and which frames of its file it
/// plays, at the engine's rate.
#[derive(Clone, Debug)]
pub(crate) struct Geometry {
    /// The song frame of the clip's start, which it plays.
    pub(crate) start: i64,
    /// The song frame of its end, which it doesn't.
    pub(crate) end: i64,
    /// The file frame at the clip's start: its Start marker, at the frame
    /// nearest where the file falls when the marker is at the clip's start
    /// in time.
    pub(crate) region_start: u64,
    /// With the loop off, the file frame it stops at: its End marker.
    pub(crate) region_end: u64,
    pub(crate) looping: bool,
    pub(crate) loop_start: u64,
    pub(crate) loop_end: u64,
    /// A warped clip's playback, which the frames above only locate (the
    /// warping spec, R12): it plays the file positions of its table, not
    /// `region_start` and on, one file frame to a song frame.
    pub(crate) warped: Option<Arc<Warped>>,
    /// The clip's pitch ratio (the warp modes spec, R25): 1 where it isn't
    /// transposed. An unwarped clip reads its file at this ratio (R27).
    pub(crate) pitch: f64,
}

impl Geometry {
    /// Places `clip`, which plays `audio`, under `tempo` at `rate`, for a
    /// file `length` frames long (at `rate`) if that's known. A warped
    /// Arrangement clip gets its table once the length is known; a Session
    /// slot clip follows the tempo from its launch instead.
    #[allow(clippy::too_many_arguments)]
    fn of(
        clip: &Clip,
        audio: &AudioData,
        tempo: &TempoMap,
        rate: SampleRate,
        length: Option<u64>,
        arrangement: bool,
        media: &BeatsMedia<'_>,
    ) -> Self {
        let start = tempo.ticks_to_frames(clip.start, rate).get();
        let end = tempo.ticks_to_frames(clip.end(), rate).get();
        // Spec: warping R12. A warped clip follows its table, which the
        // compiler makes once the file's length is known.
        if audio.warp
            && let Some(warped) = Warped::new(
                clip,
                audio,
                tempo,
                rate,
                length,
                arrangement && length.is_some(),
                media,
            )
        {
            return Self {
                start,
                end,
                region_start: warped.first_frame(start),
                region_end: u64::MAX,
                looping: audio.looping,
                loop_start: warped.loop_start_frame(),
                loop_end: u64::MAX,
                warped: Some(Arc::new(warped)),
                pitch: audio.pitch_ratio(),
            };
        }
        let frames =
            |seconds: f64| u64::try_from(rate.seconds_to_frames(seconds).get()).unwrap_or(0);
        // A warped clip whose map is damaged, which the commands never
        // leave, plays nothing.
        let silent = audio.warp;
        // Spec: audio-clips R3. One file frame to each frame of the song,
        // with the Start marker where the clip starts in time: the file
        // frame less the song frame is the same for every part of a split
        // clip, since a part's marker moved by as long as its start did
        // (R16).
        let r = rate.as_f64();
        let along = (audio.start * r - tempo.ticks_to_seconds(clip.start) * r).round() as i64;
        let length = length.unwrap_or(u64::MAX);
        Self {
            start,
            end,
            region_start: u64::try_from(start.saturating_add(along)).unwrap_or(0),
            region_end: if silent {
                0
            } else {
                audio.end.map_or(length, frames)
            },
            looping: audio.looping && !silent,
            loop_start: frames(audio.loop_start),
            loop_end: audio.loop_end.map_or(length, frames),
            warped: None,
            pitch: audio.pitch_ratio(),
        }
    }

    /// How many frames before a point a jump lands on the clip may read, so
    /// the point is cued that far early.
    pub(crate) fn reach_back(&self) -> u64 {
        if self.warped.is_some() || self.pitch != 1.0 {
            CUE_BACK
        } else {
            0
        }
    }

    /// Where an unwarped clip transposed reads (the warp modes spec, R27):
    /// its file at the pitch ratio, from the song frame `origin` it starts
    /// playing at. `None` where it isn't transposed, and then it plays the
    /// file frame for frame.
    pub(crate) fn transposed(&self, origin: i64) -> Option<Positions<'_>> {
        (self.pitch != 1.0).then(|| {
            Positions::Flat(Flat {
                origin,
                start: self.region_start as f64,
                end: self.region_end as f64,
                loop_brace: self
                    .looping
                    .then_some((self.loop_start as f64, self.loop_end as f64)),
                ratio: self.pitch,
            })
        })
    }

    /// Where in the file the clip plays `elapsed` frames after its start,
    /// and for how many of the next `want` frames the file runs on from
    /// there unbroken; `None` for silence over that many.
    fn run_at(&self, elapsed: u64, want: usize) -> (Option<u64>, usize) {
        let linear = self.region_start.saturating_add(elapsed);
        let (position, until) = if self.looping {
            let length = self.loop_end.saturating_sub(self.loop_start);
            if length == 0 {
                return (None, want);
            }
            let position = if linear < self.loop_end {
                linear
            } else {
                self.loop_start + (linear - self.loop_end) % length
            };
            (position, self.loop_end)
        } else {
            if linear >= self.region_end {
                return (None, want);
            }
            (linear, self.region_end)
        };
        let run = usize::try_from(until - position).map_or(want, |run| run.min(want));
        (Some(position), run)
    }

    /// The file frame the clip plays at song frame `at`, inside it.
    pub(crate) fn file_frame(&self, at: i64) -> Option<u64> {
        let elapsed = u64::try_from(at - self.start).ok()?;
        match &self.warped {
            Some(warped) => warped.file_frame(at),
            None => self.run_at(elapsed, 1).0,
        }
    }
}

/// One clip, placed in song frames, with everything in frames at the
/// engine's rate.
pub(crate) struct PlacedClip {
    id: ClipId,
    /// The Session slot the clip sits in, or `None` for an Arrangement clip
    /// (WP-3.2): a slot clip plays only when it is launched, from its launch
    /// frame.
    scene: Option<SceneId>,
    geometry: Geometry,
    /// Whether the player carries this clip's source over from its last
    /// list, rather than find it in this list's bank.
    carry: bool,
    gain: f64,
    fade_in: u64,
    fade_out: u64,
    /// The fades' shape and reach past the clip's own edges (the audio clips
    /// spec, R23, R24, R31): all of the clip's own fades, and the straight
    /// ones of a clip with no crossfade.
    shape: FadeShape,
    /// The clip's warp mode and HiQ switch (the warp modes spec, R8 and R12).
    playing: Playing,
    /// What the clip was playing before a mode change, where the new list
    /// says it changed: the crossfade starts from it (R8).
    past: Option<Past>,
    /// Which of the track's stretcher lanes the clip plays through (R5): two
    /// of the track's clips that can sound at once have different lanes, so
    /// they never share a stretcher.
    lane: u16,
    /// The clip's scale, which the devices on its track read while it plays
    /// (the scales spec, R15). It changes none of the clip's audio.
    scale: Scale,
}

/// How a clip's fades are shaped and where they reach (R23, R24). A clip with
/// a crossfade plays on past an edge of its own, so its geometry runs `lead`
/// frames before its start and `tail` frames past its end, and its fades are
/// measured from the edges it has in the song.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FadeShape {
    /// How many frames the geometry starts before the clip's own start.
    lead: u64,
    /// How many frames it runs past the clip's own end.
    tail: u64,
    /// How many frames before the clip's start its fade-in begins: 0 unless
    /// the clip crossfades with the one before it.
    before: u64,
    /// How many frames past the clip's end its fade-out ends: 0 unless the
    /// clip crossfades with the one after it.
    after: u64,
    /// The exponents of the curves (R23), 1 for a straight line.
    in_power: f64,
    out_power: f64,
}

impl FadeShape {
    /// A clip with straight fades that reach nowhere past its edges.
    pub(crate) fn straight() -> Self {
        Self {
            lead: 0,
            tail: 0,
            before: 0,
            after: 0,
            in_power: 1.0,
            out_power: 1.0,
        }
    }
}

/// A fade's gain `x` of the way through it, along a curve of exponent `power`.
fn shaped(x: f64, power: f64) -> f64 {
    if power == 1.0 { x } else { x.powf(power) }
}

impl PlacedClip {
    fn run_at(&self, elapsed: u64, want: usize) -> (Option<u64>, usize) {
        self.geometry.run_at(elapsed, want)
    }

    /// The clip's gain with its fade-in, `elapsed` frames after its launch
    /// (WP-3.2): a launched clip has no end in time, so only its start
    /// fades. (What a stop does is to observe; Beatlayer cuts.)
    fn launch_level(&self, elapsed: u64) -> f64 {
        let fade = if elapsed < self.fade_in && self.fade_in > 0 {
            elapsed as f64 / self.fade_in as f64
        } else {
            1.0
        };
        self.gain * fade
    }

    /// The clip's gain with its fades, `elapsed` frames after its start.
    fn level(&self, elapsed: u64) -> f64 {
        let shape = &self.shape;
        // Frames from the clip's own start, before it for a crossfade's lead.
        let at = elapsed as i64 - shape.lead as i64;
        let mut fade = 1.0f64;
        // Spec: audio-clips R6, R23, R24. The fade-in rises from its start
        // (a crossfade's, `before` the clip's) to its peak, `fade_in` after
        // the clip's start.
        let rising = self.fade_in + shape.before;
        let into = at + shape.before as i64;
        if rising > 0 && into < rising as i64 {
            fade = shaped(into.max(0) as f64 / rising as f64, shape.in_power);
        }
        let falling = self.fade_out + shape.after;
        let length = self.geometry.end - self.geometry.start - (shape.lead + shape.tail) as i64;
        let left = length + shape.after as i64 - at;
        if falling > 0 && left < falling as i64 {
            let out = shaped(left.max(0) as f64 / falling as f64, shape.out_power);
            fade = fade.min(out);
        }
        self.gain * fade
    }
}

/// A track's clips, as its player plays them: sorted by start, with each
/// one's reach (the latest end among it and the clips before it), so the
/// player finds the clips under a block by binary search; and their
/// sources, which the player takes out of the list when it arrives.
pub(crate) struct ClipList {
    /// The track the list is for.
    track: TrackId,
    /// The track's Arrangement clips, sorted by start, and its Session slot
    /// clips, which have no position in the Arrangement.
    clips: Vec<PlacedClip>,
    reach: Vec<i64>,
    /// Each clip's index, by clip ID, so a new list finds a clip's source
    /// in the old one.
    ids: Vec<(ClipId, u32)>,
    /// The sources opened for this list, one slot per clip (empty where
    /// the player carries the source over); the player takes it when the
    /// list arrives, and puts it back when the list leaves, so it's freed
    /// with the list, off the audio thread.
    bank: Handoff<Bank>,
    /// Each clip's loop read-back room, one slot per clip as `bank` has
    /// (WP-4.4, R15): made here, off the audio thread, and moved between
    /// lists the same way.
    histories: Handoff<Histories>,
    /// The room the player reads the track's clips' files into, as wide as
    /// the widest window they need (WP-4.5, R5, R18, R19): made here, off the
    /// audio thread, and moved between lists the same way.
    window: Handoff<WindowBuf>,
    /// The track's Complex and Complex Pro stretchers, one pool per mode, made
    /// here off the audio thread and moved between lists the same way (WP-4.6,
    /// R5, R21): a track with no clip in the frequency-domain modes has none.
    stretchers: Handoff<Stretchers>,
}

/// What `clip` was playing in the set's last document, where its warp mode or
/// HiQ switch changed: the mode it played, and its playback under the song's
/// current tempo (R8). Made here, off the audio thread, so the player
/// crossfades from it without owning anything.
#[allow(clippy::too_many_arguments)]
fn past_of(
    previous: Option<&Track>,
    clip: &Clip,
    audio: &AudioData,
    tempo: &TempoMap,
    rate: SampleRate,
    length: Option<u64>,
    arrangement: bool,
    media: &BeatsMedia<'_>,
) -> Option<Past> {
    let old = previous?.clip(clip.id)?;
    let ClipData::Audio(before) = &old.data else {
        return None;
    };
    let playing = Playing {
        mode: before.warp_mode,
        high_quality: before.high_quality,
        tones: before.warp_settings.tones,
        texture: before.warp_settings.texture,
        complex_pro: before.warp_settings.complex_pro,
        clip_id: clip.id.0.get(),
        pitch: before.pitch_ratio(),
        warp: before.warp,
    };
    if before.warp_mode == audio.warp_mode || !before.warp {
        return None;
    }
    let warped = Geometry::of(old, before, tempo, rate, length, arrangement, media).warped?;
    Some(Past { playing, warped })
}

/// A clip's loop read-back room, where its mode keeps one (R15): nothing for
/// a clip whose mode holds no history.
fn history_of(geometry: &Geometry, channels: usize) -> Option<Box<History>> {
    let frames = geometry.warped.as_ref()?.history_frames();
    (frames > 0).then(|| Box::new(History::new(channels, frames)))
}

/// How wide the room a track's clips are read through must be: the widest
/// window any of them needs, the mode a clip changed from included, since the
/// block of the change renders both (R8, R18, R19).
fn window_width(track: &Track, previous: Option<&Track>, rate: SampleRate) -> usize {
    let width_of = |clip: &Clip, audio: &AudioData| {
        let was = previous
            .and_then(|last| last.clip(clip.id))
            .and_then(|old| match &old.data {
                ClipData::Audio(before) => Some(before.warp_mode),
                _ => None,
            });
        [Some(audio.warp_mode), was]
            .into_iter()
            .flatten()
            .map(|mode| window_of(mode, rate.as_f64()))
            .max()
            .unwrap_or(0)
    };
    let audio = |clip: &Clip| match &clip.data {
        ClipData::Audio(audio) => Some(width_of(clip, audio)),
        _ => None,
    };
    track
        .clips()
        .filter_map(&audio)
        .chain(track.slots().filter_map(|(_, clip)| audio(clip)))
        .max()
        .unwrap_or(0)
}

/// Whether the clip sounds through the library at some point: it is in a
/// frequency-domain mode now, or was before a mode change, so its crossfade
/// renders through the stretcher too (R8, R21).
fn uses_stretcher(clip: &PlacedClip) -> bool {
    let frequency = |mode: WarpMode| matches!(mode, WarpMode::Complex | WarpMode::ComplexPro);
    frequency(clip.playing.mode)
        || clip
            .past
            .as_ref()
            .is_some_and(|past| frequency(past.playing.mode))
}

/// The clip's lane in a track's stretcher pool (R5): the clips that can sound
/// at once through a stretcher take different lanes, so each plays through its
/// own Complex or Complex Pro stretcher, and the fewest lanes such an
/// assignment needs is the pool's size. Only a clip that sounds through the
/// library is counted — one in a frequency-domain mode, or one that was, for
/// its crossfade (R8, R21) — so a track of Beats clips beside one Complex clip
/// asks for one lane, not the two its Beats clips would. The track's Session
/// slot clips share one lane, since a slot clip is launched alone and the
/// track's Arrangement clips stay silent while it plays (R20), so they never
/// sound at once with each other or with an Arrangement clip. A greedy walk
/// over the clips in song order, each taking the first lane its start has
/// cleared. Returns how many lanes the clips need.
fn assign_lanes(clips: &mut [PlacedClip]) -> usize {
    // The song frame each lane's last clip ends at.
    let mut busy: Vec<i64> = Vec::new();
    for clip in clips.iter_mut() {
        if !uses_stretcher(clip) {
            continue;
        }
        // Every slot clip of the track plays alone, so they share a lane.
        if clip.scene.is_some() {
            if busy.is_empty() {
                busy.push(i64::MIN);
            }
            clip.lane = 0;
            continue;
        }
        let lane = match busy.iter().position(|&end| end <= clip.geometry.start) {
            Some(lane) => lane,
            None => {
                busy.push(i64::MIN);
                busy.len() - 1
            }
        };
        busy[lane] = clip.geometry.end;
        clip.lane = lane as u16;
    }
    busy.len()
}

/// Whether the pool a player holds serves a list that needs `needs`, so the
/// list carries none and the player keeps the one it has. A list that needs no
/// stretcher at all never keeps one: it sends an empty pool, to free the
/// player's.
fn keeps_pool(held: Option<PoolSpec>, needs: PoolSpec) -> bool {
    needs.lanes > 0 && held.is_some_and(|held| held.covers(needs))
}

/// What [`ClipList::build`] made: the list, and the media it couldn't find.
pub(crate) struct Built {
    pub(crate) list: ClipList,
    pub(crate) missing: Vec<ContentHash>,
    /// The shape of the pool the player now holds: the one this list carries,
    /// or the one it kept because it still serves the list (R5, R21). The
    /// compiler remembers it, so the next list knows what the player has.
    pub(crate) pool: PoolSpec,
}

/// How far a crossfade has a clip play past its edges (the audio clips spec,
/// R24): seconds before its start and after its end, 0 for a clip with no
/// crossfade there.
#[derive(Clone, Copy, Debug)]
struct Reach {
    before: f64,
    after: f64,
}

/// How many ticks past a crossfade's far end a clip is read, so that the
/// frame the span ends on is inside it whichever way the tick rounds.
const REACH_MARGIN: i64 = 2;

impl Reach {
    /// What the crossfades among a track's clips (`joined`) make clip `id`
    /// play past its edges.
    fn of(id: ClipId, joined: &[bl_model::Crossfade]) -> Self {
        Self {
            before: joined
                .iter()
                .find(|c| c.later == id)
                .map_or(0.0, |c| c.before),
            after: joined
                .iter()
                .find(|c| c.earlier == id)
                .map_or(0.0, |c| c.after),
        }
    }

    /// The clip with its edges moved out to the ends of its crossfades,
    /// still playing what it plays (R24): resizing it would; `None` for a
    /// clip with no crossfade, or one that can't read that far before its
    /// start.
    fn extend(self, clip: &Clip, tempo: &TempoMap) -> Option<Clip> {
        if self.before <= 0.0 && self.after <= 0.0 {
            return None;
        }
        let mut extended = clip.clone();
        if self.before > 0.0 {
            let at = tempo.seconds_to_ticks(tempo.ticks_to_seconds(clip.start) - self.before);
            // A span that reaches back to the start of the file has no room
            // for the margin: take the earliest start the file allows.
            if let Some(earlier) = (-REACH_MARGIN..=REACH_MARGIN)
                .find_map(|margin| clip.with_start(Ticks(at.get() + margin), tempo).ok())
            {
                extended = earlier;
            }
        }
        if self.after > 0.0 {
            let at = tempo.seconds_to_ticks(tempo.ticks_to_seconds(clip.end()) + self.after);
            extended.length = Ticks(at.get() + REACH_MARGIN) - extended.start;
        }
        Some(extended)
    }
}

impl ClipList {
    /// The clips of `track` under `tempo` at `rate`, with their sources
    /// from `sources` (opened from `media`), and the Arrangement loop's
    /// start at song frame `loop_start` for the sources to preload. A
    /// deactivated clip, a clip shorter than a frame, a clip whose source
    /// isn't open yet, and a clip whose audio `media` can't supply (reported
    /// as missing) aren't in it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build(
        track: &Track,
        previous: Option<&Track>,
        tempo: &TempoMap,
        rate: SampleRate,
        channels: usize,
        loop_start: Option<i64>,
        pool: Option<PoolSpec>,
        sources: &mut ClipSources,
        media: Option<&dyn MediaLibrary>,
        grooves: &bl_model::GroovePool,
    ) -> Built {
        let mut placed = Vec::new();
        let mut missing = Vec::new();
        sources.begin();
        // A frozen track plays its freeze files instead of its clips (WP-4.14,
        // R6).
        let clips = track.playback_clips();
        // Spec: audio-clips R24, R31. A crossfaded clip plays over its span
        // past its edge, from its source.
        let joined = bl_model::crossfades(clips.iter());
        for own in &clips {
            let ClipData::Audio(_) = &own.data else {
                continue;
            };
            if own.mute {
                continue;
            }
            let reach = Reach::of(own.id, &joined);
            let extended = reach.extend(own, tempo);
            let clip = extended.as_ref().unwrap_or(own);
            let ClipData::Audio(audio) = &clip.data else {
                continue;
            };
            // A Beats clip's plan asks for the file's transients, and for
            // its audio where a loop turn must be found (R14, R15).
            let transients = media
                .and_then(|library| library.transients(audio.media))
                .map_or_else(Vec::new, |detected| audio.transient_edits.apply(&detected));
            let samples = || media.and_then(|library| library.sample(audio.media, rate));
            // Spec: grooves R12. The clip's groove at the Global Amount, if
            // it changes anything.
            let groove = clip
                .groove
                .and_then(|id| grooves.play(id))
                .filter(|play| !play.is_inert());
            let beats = BeatsMedia {
                transients: &transients,
                samples: &samples,
                groove: groove.as_ref(),
            };
            // Where the clip starts reading, before its length is known.
            let at = Geometry::of(clip, audio, tempo, rate, None, true, &beats);
            if at.end <= at.start {
                continue;
            }
            let Some(sourced) = sources.source(
                track.id(),
                clip.id,
                audio.media,
                rate,
                audio.ram,
                at.region_start.saturating_sub(at.reach_back()),
                media,
                &mut missing,
            ) else {
                continue;
            };
            let geometry = Geometry::of(clip, audio, tempo, rate, sourced.frames, true, &beats);
            sources.cue(clip.id, &geometry, loop_start);
            let history = history_of(&geometry, channels);
            let past = past_of(
                previous,
                clip,
                audio,
                tempo,
                rate,
                sourced.frames,
                true,
                &beats,
            );
            let frames =
                |seconds: f64| u64::try_from(rate.seconds_to_frames(seconds).get()).unwrap_or(0);
            // Spec: audio-clips R23, R24. The clip's own edges in the song,
            // inside the geometry a crossfade extends.
            let own_start = tempo.ticks_to_frames(own.start, rate).get();
            let own_end = tempo.ticks_to_frames(own.end(), rate).get();
            let lead = u64::try_from(own_start - geometry.start).unwrap_or(0);
            let shape = FadeShape {
                lead,
                tail: u64::try_from(geometry.end - own_end).unwrap_or(0),
                before: frames(reach.before).min(lead),
                after: frames(reach.after),
                in_power: bl_model::fade_power(audio.fade_in_curve),
                out_power: bl_model::fade_power(audio.fade_out_curve),
            };
            let (carry, source) = match sourced.supply {
                Supply::Carry => (true, None),
                Supply::Fresh(source) => (false, Some(source)),
                Supply::Switch(source) => (true, Some(source)),
            };
            placed.push((
                PlacedClip {
                    id: clip.id,
                    scene: None,
                    shape,
                    geometry,
                    carry,
                    gain: audio.gain,
                    fade_in: frames(audio.fade_in),
                    fade_out: frames(audio.fade_out),
                    playing: Playing {
                        mode: audio.warp_mode,
                        high_quality: audio.high_quality,
                        tones: audio.warp_settings.tones,
                        texture: audio.warp_settings.texture,
                        complex_pro: audio.warp_settings.complex_pro,
                        clip_id: clip.id.0.get(),
                        pitch: audio.pitch_ratio(),
                        warp: audio.warp,
                    },
                    past,
                    lane: 0,
                    scale: clip.scale,
                },
                source,
                history,
            ));
        }
        // The track's Session slot clips (WP-3.2): they play only when the
        // engine launches them, so they carry no Arrangement position.
        let slots = track.playback_slots();
        for (scene, clip) in &slots {
            let ClipData::Audio(audio) = &clip.data else {
                continue;
            };
            if clip.mute {
                continue;
            }
            // A Beats clip's plan asks for the file's transients, and for
            // its audio where a loop turn must be found (R14, R15).
            let transients = media
                .and_then(|library| library.transients(audio.media))
                .map_or_else(Vec::new, |detected| audio.transient_edits.apply(&detected));
            let samples = || media.and_then(|library| library.sample(audio.media, rate));
            // Spec: grooves R12. The clip's groove at the Global Amount, if
            // it changes anything.
            let groove = clip
                .groove
                .and_then(|id| grooves.play(id))
                .filter(|play| !play.is_inert());
            let beats = BeatsMedia {
                transients: &transients,
                samples: &samples,
                groove: groove.as_ref(),
            };
            let at = Geometry::of(clip, audio, tempo, rate, None, false, &beats);
            if at.end <= at.start {
                continue;
            }
            let Some(sourced) = sources.source(
                track.id(),
                clip.id,
                audio.media,
                rate,
                audio.ram,
                at.region_start.saturating_sub(at.reach_back()),
                media,
                &mut missing,
            ) else {
                continue;
            };
            let geometry = Geometry::of(clip, audio, tempo, rate, sourced.frames, false, &beats);
            sources.cue(clip.id, &geometry, loop_start);
            let history = history_of(&geometry, channels);
            let past = past_of(
                previous,
                clip,
                audio,
                tempo,
                rate,
                sourced.frames,
                false,
                &beats,
            );
            let frames =
                |seconds: f64| u64::try_from(rate.seconds_to_frames(seconds).get()).unwrap_or(0);
            let (carry, source) = match sourced.supply {
                Supply::Carry => (true, None),
                Supply::Fresh(source) => (false, Some(source)),
                Supply::Switch(source) => (true, Some(source)),
            };
            placed.push((
                PlacedClip {
                    id: clip.id,
                    scene: Some(*scene),
                    shape: FadeShape::straight(),
                    geometry,
                    carry,
                    gain: audio.gain,
                    fade_in: frames(audio.fade_in),
                    fade_out: frames(audio.fade_out),
                    playing: Playing {
                        mode: audio.warp_mode,
                        high_quality: audio.high_quality,
                        tones: audio.warp_settings.tones,
                        texture: audio.warp_settings.texture,
                        complex_pro: audio.warp_settings.complex_pro,
                        clip_id: clip.id.0.get(),
                        pitch: audio.pitch_ratio(),
                        warp: audio.warp,
                    },
                    past,
                    lane: 0,
                    scale: clip.scale,
                },
                source,
                history,
            ));
        }
        sources.finish(track.id());
        // Only the Arrangement's clips are ordered: the session path finds
        // a slot by its scene.
        placed.sort_by_key(|(clip, _, _)| (clip.geometry.start, clip.geometry.end, clip.id));
        let (mut clips, mut bank, mut histories) = (Vec::new(), Vec::new(), Vec::new());
        for (clip, source, history) in placed {
            clips.push(clip);
            bank.push(source);
            histories.push(history);
        }
        let mut reach = Vec::with_capacity(clips.len());
        let mut furthest = i64::MIN;
        for clip in &clips {
            furthest = furthest.max(clip.geometry.end);
            reach.push(furthest);
        }
        let mut ids: Vec<(ClipId, u32)> = clips
            .iter()
            .enumerate()
            .map(|(i, clip)| (clip.id, i as u32))
            .collect();
        ids.sort_unstable();
        missing.sort_unstable();
        missing.dedup();
        let lanes = assign_lanes(&mut clips);
        let uses = |mode: WarpMode| {
            clips.iter().any(|clip| {
                clip.playing.mode == mode
                    || clip
                        .past
                        .as_ref()
                        .is_some_and(|past| past.playing.mode == mode)
            })
        };
        let rate_hz = rate.as_f64();
        let needs = PoolSpec {
            lanes,
            complex: uses(WarpMode::Complex),
            pro: uses(WarpMode::ComplexPro),
        };
        // Keep the pool the player already has while it serves this list: a
        // change that leaves the pool's shape alone — a clip's Formants, its
        // gain, a move that keeps the lanes — then carries no pool, and the
        // player keeps the one it has, so the change doesn't make the
        // stretchers again (R21). A list that needs none sends an empty pool,
        // to free the player's.
        let kept = keeps_pool(pool, needs);
        let held = if kept { pool.unwrap() } else { needs };
        let stretchers = if kept {
            Handoff::empty()
        } else {
            Handoff::new(Box::new(Stretchers::new(
                channels,
                rate_hz,
                needs.lanes,
                needs.complex,
                needs.pro,
            )))
        };
        let window = WindowBuf::new(channels, window_width(track, previous, rate));
        Built {
            list: Self {
                track: track.id(),
                clips,
                reach,
                ids,
                bank: Handoff::new(Box::new(bank.into_boxed_slice())),
                histories: Handoff::new(Box::new(histories.into_boxed_slice())),
                window: Handoff::new(Box::new(window)),
                stretchers,
            },
            missing,
            pool: held,
        }
    }

    /// Whether it plays nothing.
    pub(crate) fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }

    /// How many clips it plays.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.clips.len()
    }
}

/// An audio track's clip player, at the head of its device chain: while the
/// song plays, it adds the track's clips to the track's buffer, each where
/// the song position says (the audio clips spec), read from its source.
///
/// Its clips arrive as node data, a list the compiler made, with the
/// sources the list brings; it carries over from its last list the sources
/// of the clips the new one keeps. Until the first list arrives, and while
/// the song is stopped, it adds nothing. Clips that overlap each play,
/// summed (R8).
pub struct ClipPlayer {
    clips: Option<NodeData>,
    /// The sources of `clips`, one slot per clip.
    bank: Option<Box<Bank>>,
    /// Each clip's loop read-back room, one slot per clip (WP-4.4, R15).
    histories: Option<Box<Histories>>,
    /// The room the clips' files are read through (WP-4.5, R5): as wide as
    /// the list's clips need, and taken from the list like their sources.
    window: Option<Box<WindowBuf>>,
    /// The track's Complex and Complex Pro stretchers (WP-4.6, R5, R21), taken
    /// from the list like its window: a track with no frequency-domain clip
    /// has none.
    stretchers: Option<Box<Stretchers>>,
    /// Planar scratch for one source read: `channels` channels of
    /// `max_frames` frames.
    scratch: Vec<f32>,
    max_frames: usize,
    /// Room for warped clips' playback (WP-4.2).
    warp: WarpScratch,
    /// Whether the clips sounded in the last block, so a stop stops their
    /// sources.
    sounding: bool,
    /// Whether a clip of the track played in the last block (WP-2.13):
    /// the track's input routes close under Monitor Auto while one does
    /// (the routing spec, R11).
    playing_clip: bool,
    /// The song position the sources were made ready to play from while
    /// the song was stopped.
    readied: Option<i64>,
    /// Whether the clips are heard: not while the track's Monitor is In
    /// (the routing spec). The gain glides to each change.
    audible: bool,
    gate: Ramp,
    smoothing: u32,
    /// What the player keeps of its track's scale between blocks, and the
    /// scale it worked out for the last block (the scales spec, R15).
    scale: TrackScale,
    runs: Option<ScaleRuns>,
}

/// A clip player's parameters.
pub mod player {
    use bl_model::ParamId;

    /// Whether the track's clips are heard: silent when 0, as while the
    /// track's Monitor is In.
    pub const AUDIBLE: ParamId = ParamId(0);
}

impl ClipPlayer {
    /// A player with no clips, for blocks of up to `config.max_frames`.
    #[must_use]
    pub fn new(config: &NodeConfig) -> Self {
        Self {
            clips: None,
            bank: None,
            histories: None,
            window: None,
            stretchers: None,
            scratch: vec![0.0; config.channels * config.max_frames],
            max_frames: config.max_frames,
            warp: WarpScratch::new(config.channels, config.max_frames, config.sample_rate),
            sounding: false,
            playing_clip: false,
            readied: None,
            audible: true,
            gate: Ramp::at(1.0),
            smoothing: smoothing_frames(config.sample_rate),
            scale: TrackScale::default(),
            runs: None,
        }
    }

    /// Works out the track's scale over `block` from the clips it plays.
    fn work_out_scale(&mut self, block: &Block<'_>) {
        let Self {
            clips, scale, runs, ..
        } = self;
        *runs = Some(
            match clips.as_ref().and_then(|d| d.downcast_ref::<ClipList>()) {
                Some(list) => {
                    let spans = list.clips.iter().map(|clip| Span {
                        scene: clip.scene,
                        start: clip.geometry.start + clip.shape.lead as i64,
                        end: clip.geometry.end - clip.shape.tail as i64,
                        scale: clip.scale,
                    });
                    scale.runs(list.track, spans, block)
                }
                None => ScaleRuns::of(block.set_scale()),
            },
        );
    }

    /// The same player, its clips heard or not, resting there.
    #[must_use]
    pub fn audible(mut self, audible: bool) -> Self {
        self.audible = audible;
        self.gate = Ramp::at(if audible { 1.0 } else { 0.0 });
        self
    }

    /// Takes the gate's parameter changes, gliding toward the new state.
    fn set(&mut self, events: &[NodeEvent]) {
        for event in events {
            if event.param == player::AUDIBLE {
                self.audible = match event.value {
                    ParamValue::Set(value) => value != 0.0,
                    ParamValue::Default => true,
                };
            }
        }
        if !events.is_empty() {
            let target = if self.audible { 1.0 } else { 0.0 };
            self.gate.toward(target, self.smoothing);
        }
    }

    /// Scales what the clips put into the block by the gate, which glides
    /// to each change from its frame. The player runs first in its track,
    /// so the block holds only the clips.
    fn apply_gate(&mut self, block: &mut Block<'_>) {
        let events: &[NodeEvent] = block.events;
        if events.is_empty() && self.gate.is_settled() && self.gate.value() == 1.0 {
            return;
        }
        let channels = block.channels();
        for segment in segments(events, block.frames()) {
            self.set(segment.events);
            let mut after = self.gate;
            for c in 0..channels {
                let mut ramp = self.gate;
                scale(&mut block.channel_mut(c)[segment.range.clone()], &mut ramp);
                after = ramp;
            }
            self.gate = after;
        }
    }

    /// The song stopped: every source stops sounding, so the next read
    /// starts afresh.
    fn stop(&mut self) {
        if let Some(bank) = self.bank.as_mut() {
            for source in bank.iter_mut().flatten() {
                source.stop();
            }
        }
        self.sounding = false;
    }

    /// While the song is stopped at `position`, readies the source of each
    /// clip there to play from where it would, so Play starts it at once.
    fn ready_at(&mut self, position: i64) {
        if self.readied == Some(position) {
            return;
        }
        self.readied = Some(position);
        let (Some(list), Some(bank)) = (
            self.clips
                .as_ref()
                .and_then(|d| d.downcast_ref::<ClipList>()),
            self.bank.as_mut(),
        ) else {
            return;
        };
        let first = list.reach.partition_point(|&reach| reach <= position);
        for (clip, slot) in list.clips[first..].iter().zip(&mut bank[first..]) {
            if clip.scene.is_some() {
                continue;
            }
            if clip.geometry.start > position {
                break;
            }
            if clip.geometry.end <= position {
                continue;
            }
            if let (Some(source), Some(at)) = (slot, clip.geometry.file_frame(position)) {
                // A warped clip reads a little before the position.
                source.seek(at.saturating_sub(clip.geometry.reach_back()));
            }
        }
    }
}

impl std::fmt::Debug for ClipPlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipPlayer")
            .field("clips", &self.clips.is_some())
            .finish_non_exhaustive()
    }
}

/// Adds the frames `range` of the block, which are `elapsed` frames into
/// `clip`, read from `source`, to the block, through `level`, which says the
/// clip's gain there.
#[allow(clippy::too_many_arguments)]
fn render(
    clip: &PlacedClip,
    source: &mut dyn AudioSource,
    block: &mut Block<'_>,
    range: core::ops::Range<usize>,
    elapsed: u64,
    scratch: &mut [f32],
    stride: usize,
    level: impl Fn(&PlacedClip, u64) -> f64,
) {
    let channels = block.channels();
    let source_channels = source.channels().clamp(1, channels.max(1));
    let mut at = range.start;
    let mut elapsed = elapsed;
    while at < range.end {
        let (position, run) = clip.run_at(elapsed, range.end - at);
        if let Some(position) = position {
            let mut read = PlanarMut::new(scratch, channels, run, stride);
            // Spec: audio-clips R4. A loop's wrap, a locate, and a clip's
            // start are cuts on their exact frame; the clip's own fades
            // smooth its edges.
            source.read(position, &mut read, Transition::Cut);
            // Spec: audio-clips R9. A mono file plays on every channel.
            for c in source_channels..channels {
                let (from, to) = scratch.split_at_mut(c * stride);
                let last = (source_channels - 1) * stride;
                to[..run].copy_from_slice(&from[last..last + run]);
            }
            for c in 0..channels {
                let read = &scratch[c * stride..c * stride + run];
                let out = &mut block.channel_mut(c)[at..at + run];
                for (i, (sum, &sample)) in out.iter_mut().zip(read).enumerate() {
                    *sum += (f64::from(sample) * level(clip, elapsed + i as u64)) as f32;
                }
            }
        }
        at += run;
        elapsed += run as u64;
    }
}

/// A clip's level with the gain its groove's Velocity gives it (the grooves
/// spec, R13): `level` is the clip's own, `elapsed` frames after the song
/// frame `origin` where its contents began.
fn with_groove<'a>(
    warped: &'a Warped,
    tempo: &'a TempoMap,
    rate: f64,
    origin: i64,
    level: fn(&PlacedClip, u64) -> f64,
) -> impl Fn(&PlacedClip, u64) -> f64 + 'a {
    let ticks = tempo.seconds_to_ticks_exact(origin as f64 / rate);
    move |clip, elapsed| {
        level(clip, elapsed) * warped.groove_gain(tempo, rate, ticks, origin + elapsed as i64)
    }
}

/// Adds the frames `range` of the block, which are `elapsed` frames into the
/// warped `clip` and start at song frame `frame`, read from `source` through
/// the clip's positions and warp mode, to the block, through `level`.
#[allow(clippy::too_many_arguments)]
fn render_warped(
    clip: &PlacedClip,
    positions: &Positions<'_>,
    launched: Option<i64>,
    source: &mut dyn AudioSource,
    window: &mut WindowBuf,
    block: &mut Block<'_>,
    range: core::ops::Range<usize>,
    (elapsed, frame): (u64, i64),
    warp: &mut WarpScratch,
    mut history: Option<&mut History>,
    level: impl Fn(&PlacedClip, u64) -> f64,
    stretchers: &mut Stretchers,
    lane: usize,
) {
    let channels = block.channels();
    // A plan's segments lie on the Arrangement's song frames, so a launched
    // Session clip, which follows the tempo from its launch, has none to
    // shape its level by.
    let plan = clip
        .geometry
        .warped
        .as_ref()
        .filter(|_| launched.is_none())
        .map(|warped| warped.plan());
    let (mut at, mut elapsed, mut frame) = (range.start, elapsed, frame);
    while at < range.end {
        // Spec: warping R12, warp-modes R9, R14
        let (n, sound) = warp.render(
            clip.id,
            source,
            window,
            positions,
            launched,
            frame,
            range.end - at,
            clip.playing,
            plan,
            clip.past.as_ref(),
            history.as_deref_mut(),
            stretchers,
            lane,
        );
        if sound {
            for c in 0..channels {
                let out = &mut block.channel_mut(c)[at..at + n];
                for (i, (sum, &sample)) in out.iter_mut().zip(warp.rendered(c, n)).enumerate() {
                    *sum += (f64::from(sample) * level(clip, elapsed + i as u64)) as f32;
                }
            }
        }
        at += n;
        elapsed += n as u64;
        frame += n as i64;
    }
}

impl Node for ClipPlayer {
    fn process(&mut self, block: &mut Block<'_>) {
        self.work_out_scale(block);
        let transport = block.transport();
        // Spec: audio-clips R1
        if !transport.playing {
            // Nothing sounds, so the gate lands at once.
            let events: &[NodeEvent] = block.events;
            self.set(events);
            self.gate = Ramp::at(if self.audible { 1.0 } else { 0.0 });
            if self.sounding {
                self.stop();
            }
            self.playing_clip = false;
            self.ready_at(transport.position.get());
            return;
        }
        self.sounding = true;
        self.readied = None;
        // Spec: launch-quantisation R9. The launched slot clip plays from
        // its launch frame. Spec: session-view R20. While
        // the track is taken over, the Arrangement's clips below stay silent.
        if let (Some(list), Some(bank), Some(histories), Some(window), Some(stretchers)) = (
            self.clips
                .as_ref()
                .and_then(|d| d.downcast_ref::<ClipList>()),
            self.bank.as_mut(),
            self.histories.as_mut(),
            self.window.as_mut(),
            self.stretchers.as_mut(),
        ) && let Some(entry) = block
            .session()
            .iter()
            .find(|entry| entry.track == list.track)
            && let Some(scene) = entry.scene
            && let Some(at) = list.clips.iter().position(|clip| clip.scene == Some(scene))
        {
            let launch = entry.frame.get();
            let from = transport.position.get();
            let elapsed = u64::try_from(from - launch).unwrap_or(0);
            let clip = &list.clips[at];
            if let (Some(source), Some(warped)) = (bank[at].as_mut(), clip.geometry.warped.as_ref())
            {
                // Spec: warping R15. A launched warped clip follows the
                // tempo from its launch frame.
                let tempo = block.tempo();
                let rate = block.sample_rate().as_f64();
                let positions = warped.launched(tempo, launch, rate);
                let frames = block.frames();
                render_warped(
                    clip,
                    &positions,
                    Some(launch),
                    source.as_mut(),
                    window,
                    block,
                    0..frames,
                    (elapsed, from),
                    &mut self.warp,
                    histories[at].as_deref_mut(),
                    with_groove(warped, tempo, rate, launch, PlacedClip::launch_level),
                    stretchers,
                    clip.lane as usize,
                );
            } else if let Some(source) = bank[at].as_mut() {
                // Spec: warp-modes R27. An unwarped clip transposed reads
                // its file at the pitch ratio, from its launch frame.
                if let Some(positions) = clip.geometry.transposed(launch) {
                    render_warped(
                        clip,
                        &positions,
                        Some(launch),
                        source.as_mut(),
                        window,
                        block,
                        0..block.frames(),
                        (elapsed, from),
                        &mut self.warp,
                        histories[at].as_deref_mut(),
                        PlacedClip::launch_level,
                        stretchers,
                        clip.lane as usize,
                    );
                } else {
                    render(
                        clip,
                        source.as_mut(),
                        block,
                        0..block.frames(),
                        elapsed,
                        &mut self.scratch,
                        self.max_frames,
                        PlacedClip::launch_level,
                    );
                }
            }
        }
        let Self {
            clips,
            bank,
            histories,
            window,
            stretchers,
            scratch,
            max_frames,
            warp,
            ..
        } = self;
        let (Some(list), Some(bank), Some(histories), Some(window), Some(stretchers)) = (
            clips.as_ref().and_then(|d| d.downcast_ref::<ClipList>()),
            bank.as_mut(),
            histories.as_mut(),
            window.as_mut(),
            stretchers.as_mut(),
        ) else {
            self.playing_clip = false;
            self.apply_gate(block);
            return;
        };
        if block
            .session()
            .iter()
            .any(|entry| entry.track == list.track)
        {
            self.playing_clip = false;
            self.apply_gate(block);
            return;
        }
        let from = transport.position.get();
        let to = from + block.frames() as i64;
        let first = list.reach.partition_point(|&reach| reach <= from);
        // Spec: routing R11. A clip that covers this block is playing, so
        // the track's input closes under Monitor Auto.
        self.playing_clip = list.clips[first..]
            .iter()
            .take_while(|clip| clip.geometry.start < to)
            .any(|clip| clip.geometry.end > from);
        let played = list.clips[first..]
            .iter()
            .zip(&mut bank[first..])
            .zip(histories[first..].iter_mut());
        for ((clip, slot), history) in played {
            if clip.scene.is_some() {
                continue;
            }
            if clip.geometry.start >= to {
                break;
            }
            if clip.geometry.end <= from {
                continue;
            }
            let Some(source) = slot else {
                continue;
            };
            let start = clip.geometry.start.max(from);
            let end = clip.geometry.end.min(to);
            let range = (start - from) as usize..(end - from) as usize;
            let elapsed = (start - clip.geometry.start) as u64;
            if let Some(warped) = &clip.geometry.warped {
                let (tempo, rate) = (block.tempo(), block.sample_rate().as_f64());
                render_warped(
                    clip,
                    &warped.positions(),
                    None,
                    source.as_mut(),
                    window,
                    block,
                    range,
                    (elapsed, start),
                    warp,
                    history.as_deref_mut(),
                    with_groove(warped, tempo, rate, clip.geometry.start, PlacedClip::level),
                    stretchers,
                    clip.lane as usize,
                );
                continue;
            }
            // Spec: warp-modes R27. An unwarped clip transposed reads its
            // file at the pitch ratio.
            if let Some(positions) = clip.geometry.transposed(clip.geometry.start) {
                render_warped(
                    clip,
                    &positions,
                    None,
                    source.as_mut(),
                    window,
                    block,
                    range,
                    (elapsed, start),
                    warp,
                    history.as_deref_mut(),
                    PlacedClip::level,
                    stretchers,
                    clip.lane as usize,
                );
                continue;
            }
            render(
                clip,
                source.as_mut(),
                block,
                range,
                elapsed,
                scratch,
                *max_frames,
                PlacedClip::level,
            );
        }
        self.apply_gate(block);
    }

    fn flush(&mut self, events: &[NodeEvent]) {
        self.set(events);
    }

    fn receive_data(&mut self, data: NodeData) {
        self.readied = None;
        let Some(list) = data.downcast_ref::<ClipList>() else {
            return;
        };
        let mut bank = list.bank.take();
        let mut histories = list.histories.take();
        let window = list.window.take();
        let stretchers = list.stretchers.take();
        // Spec: warp-modes R8. The list says which clips changed their mode:
        // each crossfades from what it was playing, from this list's first
        // frame.
        if list.clips.iter().any(|clip| clip.past.is_some()) {
            self.warp.forget_change();
        }
        // Spec: audio-clips R18, warp-modes R8 and R15. The sources and the
        // loop read-back rooms of the clips the new list keeps move across
        // from the old one.
        {
            let Self {
                clips,
                bank: old_bank,
                histories: old_histories,
                ..
            } = self;
            if let (Some(old), Some(old_bank), Some(new_bank), Some(old_histories), Some(new)) = (
                clips.as_ref().and_then(|d| d.downcast_ref::<ClipList>()),
                old_bank.as_mut(),
                bank.as_mut(),
                old_histories.as_mut(),
                histories.as_mut(),
            ) {
                let kept = list
                    .clips
                    .iter()
                    .zip(new_bank.iter_mut())
                    .zip(new.iter_mut());
                for ((clip, slot), room) in kept {
                    if !clip.carry {
                        continue;
                    }
                    let Ok(i) = old.ids.binary_search_by_key(&clip.id, |&(id, _)| id) else {
                        continue;
                    };
                    let was = old.ids[i].1 as usize;
                    let playing = old_bank[was].take();
                    match (slot.take(), playing) {
                        // Spec: warp-modes R11. The RAM switch changed: the
                        // clip goes on playing its source until the new one
                        // is ready. What the source had retired goes back to
                        // the old list's bank, to be freed with it.
                        (Some(new), Some(mut playing)) => {
                            old_bank[was] = ClipSource::hand_over(&mut playing, new);
                            *slot = Some(playing);
                        }
                        (Some(new), None) => *slot = Some(new),
                        (None, playing) => *slot = playing,
                    }
                    // The read-back room carries over where it still fits
                    // the new plan; the room the new list brought goes back
                    // to the old list, whose drop thread frees it (R13).
                    if let (Some(held), Some(fresh)) = (old_histories[was].as_mut(), room.as_mut())
                        && held.capacity() == fresh.capacity()
                    {
                        std::mem::swap(held, fresh);
                    }
                }
            }
        }
        // The old list takes its bank back, with the sources the new one
        // didn't keep, and the drop thread frees them with it (R13).
        if let Some(old_histories) = self.histories.take()
            && let Some(old) = self
                .clips
                .as_ref()
                .and_then(|d| d.downcast_ref::<ClipList>())
            && let Err(old_histories) = old.histories.put(old_histories)
        {
            std::mem::forget(old_histories);
        }
        if let Some(old_bank) = self.bank.take() {
            match self
                .clips
                .as_ref()
                .and_then(|d| d.downcast_ref::<ClipList>())
            {
                Some(old) => {
                    if let Err(old_bank) = old.bank.put(old_bank) {
                        // Can't happen: the player took the bank out. Better
                        // a leak than a free here.
                        std::mem::forget(old_bank);
                    }
                }
                None => std::mem::forget(old_bank),
            }
        }
        if let Some(old_window) = self.window.take() {
            match self
                .clips
                .as_ref()
                .and_then(|d| d.downcast_ref::<ClipList>())
            {
                Some(old) => {
                    if let Err(old_window) = old.window.put(old_window) {
                        // Can't happen: the player took the window out. Better
                        // a leak than a free here.
                        std::mem::forget(old_window);
                    }
                }
                None => std::mem::forget(old_window),
            }
        }
        if stretchers.is_some()
            && let Some(old_stretchers) = self.stretchers.take()
        {
            match self
                .clips
                .as_ref()
                .and_then(|d| d.downcast_ref::<ClipList>())
            {
                Some(old) => {
                    if let Err(old_stretchers) = old.stretchers.put(old_stretchers) {
                        // Can't happen: the player took the pool out. Better a
                        // leak than a free here.
                        std::mem::forget(old_stretchers);
                    }
                }
                None => std::mem::forget(old_stretchers),
            }
        }
        if list.is_empty() {
            if let Some(bank) = bank
                && let Err(bank) = list.bank.put(bank)
            {
                std::mem::forget(bank);
            }
            if let Some(histories) = histories
                && let Err(histories) = list.histories.put(histories)
            {
                std::mem::forget(histories);
            }
            if let Some(window) = window
                && let Err(window) = list.window.put(window)
            {
                std::mem::forget(window);
            }
            if let Some(stretchers) = stretchers
                && let Err(stretchers) = list.stretchers.put(stretchers)
            {
                std::mem::forget(stretchers);
            }
            // Dropping the data, the old list's or the empty new one, only
            // lets go of it: the drop thread frees it, with its sources.
            drop(self.clips.take());
            drop(data);
            return;
        }
        self.bank = bank;
        self.histories = histories;
        self.window = window;
        // A list that brought no pool leaves the player's in place: the
        // compiler kept it, since it still serves the list (R5, R21).
        if let Some(stretchers) = stretchers {
            self.stretchers = Some(stretchers);
        }
        drop(self.clips.replace(data));
    }

    fn playing_clip(&self) -> bool {
        self.playing_clip
    }

    fn track_scale(&self) -> Option<ScaleRuns> {
        self.runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a Beats clip's plan asks of the media: nothing in these tests.
    fn no_media() -> BeatsMedia<'static> {
        BeatsMedia {
            groove: None,
            transients: &[],
            samples: &|| None,
        }
    }

    fn clip(looping: bool) -> PlacedClip {
        PlacedClip {
            id: ClipId(bl_core::id::Id::new(1).unwrap()),
            scene: None,
            geometry: Geometry {
                start: 100,
                end: 1100,
                region_start: 50,
                region_end: 300,
                looping,
                loop_start: 100,
                loop_end: 200,
                warped: None,
                pitch: 1.0,
            },
            carry: false,
            gain: 0.5,
            fade_in: 10,
            fade_out: 20,
            shape: FadeShape::straight(),
            playing: Playing {
                mode: bl_model::WarpMode::RePitch,
                high_quality: true,
                tones: Default::default(),
                texture: Default::default(),
                complex_pro: Default::default(),
                clip_id: 1,
                pitch: 1.0,
                warp: true,
            },
            past: None,
            lane: 0,
            scale: Scale::default(),
        }
    }

    /// A clip that sounds through the library, for the lane tests.
    fn complex_clip() -> PlacedClip {
        let mut clip = clip(false);
        clip.playing.mode = bl_model::WarpMode::Complex;
        clip
    }

    // Spec: warp-modes R5, R21. Clips that can sound at once take different
    // stretcher lanes; clips that can't share one, so the pool stays as small
    // as it can be.
    #[test]
    fn clips_that_can_sound_at_once_take_different_lanes() {
        let mut clips: Vec<PlacedClip> = (0..3).map(|_| complex_clip()).collect();
        // Three in a row: one lane between them.
        for (i, clip) in clips.iter_mut().enumerate() {
            let at = i as i64 * 1_000;
            clip.geometry.start = at;
            clip.geometry.end = at + 500;
        }
        assert_eq!(assign_lanes(&mut clips), 1);
        // The first two overlap: two lanes, and the third reuses the first.
        clips[1].geometry.start = 200;
        clips[1].geometry.end = 900;
        assert_eq!(assign_lanes(&mut clips), 2);
        assert_eq!([clips[0].lane, clips[1].lane, clips[2].lane], [0, 1, 0]);
        // A clip that starts where the last one ends shares its lane.
        clips[1].geometry.start = 600;
        clips[1].geometry.end = 1_000;
        assert_eq!(assign_lanes(&mut clips), 1);
    }

    // Spec: warp-modes R5, R21. Only the clips that sound through a stretcher
    // are counted, so a track of Beats clips beside one Complex clip asks for
    // one lane rather than the two its Beats clips alone would.
    #[test]
    fn a_track_counts_only_the_clips_that_use_a_stretcher() {
        let mut clips = vec![clip(false), clip(false), complex_clip()];
        for (i, clip) in clips.iter_mut().enumerate() {
            let at = i as i64 * 1_000;
            clip.geometry.start = at;
            clip.geometry.end = at + 900;
        }
        // The two Beats clips overlap each other but never a stretcher, so the
        // one Complex clip is the only lane the pool needs.
        clips[1].geometry.start = 200;
        clips[1].geometry.end = 1_100;
        assert_eq!(assign_lanes(&mut clips), 1);
        assert_eq!(clips[2].lane, 0);
    }

    // Spec: warp-modes R5, R20. The Session slot clips of a track share one
    // lane, since a slot clip is launched alone and the Arrangement is silent
    // while it plays: four scene slots ask for one lane, not four.
    #[test]
    fn session_slot_clips_share_a_lane() {
        let mut clips: Vec<PlacedClip> = (0..4).map(|_| complex_clip()).collect();
        for clip in &mut clips {
            clip.scene = Some(SceneId(bl_core::id::Id::new(1).unwrap()));
            clip.geometry.start = 0;
            clip.geometry.end = 4_800;
        }
        assert_eq!(assign_lanes(&mut clips), 1);
        assert!(clips.iter().all(|clip| clip.lane == 0));
        // A slot clip beside an Arrangement clip that starts at the same frame
        // shares its lane too: they never sound together.
        let mut arrangement = complex_clip();
        arrangement.geometry.start = 0;
        arrangement.geometry.end = 4_800;
        clips.push(arrangement);
        assert_eq!(assign_lanes(&mut clips), 1);
    }

    // Spec: warp-modes R5, R21. A pool the player holds serves a list while it
    // has the lanes and the modes the list asks for, so the list carries none
    // and the player keeps it; a list that needs no stretcher frees it.
    #[test]
    fn a_pool_is_kept_while_it_serves_the_list() {
        let two = PoolSpec {
            lanes: 2,
            complex: true,
            pro: true,
        };
        let one = PoolSpec {
            lanes: 1,
            complex: true,
            pro: false,
        };
        assert!(keeps_pool(Some(two), one));
        assert!(keeps_pool(Some(two), two));
        // Fewer lanes, or a mode the pool lacks, makes the list build one.
        assert!(!keeps_pool(Some(one), two));
        assert!(!keeps_pool(None, one));
        // A list with no stretcher never keeps the pool.
        assert!(!keeps_pool(
            Some(two),
            PoolSpec {
                lanes: 0,
                complex: false,
                pro: false,
            }
        ));
    }

    // Spec: audio-clips R3
    #[test]
    fn an_unlooped_clip_plays_from_its_start_marker_to_its_end_marker() {
        let plain = clip(false);
        assert_eq!(plain.run_at(0, 64), (Some(50), 64));
        assert_eq!(plain.run_at(200, 64), (Some(250), 50));
        assert_eq!(plain.run_at(250, 64), (None, 64));
        assert_eq!(plain.run_at(u64::MAX, 64), (None, 64));
        // Past its End marker, as a trimmed clip's Start marker can be, it
        // plays nothing.
        let mut late = clip(false);
        late.geometry.region_start = 400;
        assert_eq!(late.run_at(0, 64), (None, 64));
    }

    // Spec: audio-clips R4
    #[test]
    fn a_looped_clip_plays_into_its_loop_and_repeats_it() {
        let looped = clip(true);
        // From the Start marker (50) through the brace's start (100) to its
        // end (200), then from 100 again.
        assert_eq!(looped.run_at(0, 1000), (Some(50), 150));
        assert_eq!(looped.run_at(149, 10), (Some(199), 1));
        assert_eq!(looped.run_at(150, 1000), (Some(100), 100));
        assert_eq!(looped.run_at(275, 1000), (Some(125), 75));
        // A Start marker inside the brace starts there.
        let mut inside = clip(true);
        inside.geometry.region_start = 150;
        assert_eq!(inside.run_at(0, 1000), (Some(150), 50));
        assert_eq!(inside.run_at(50, 1000), (Some(100), 100));
        // Past the brace's end, as trimming a looped clip whose brace ends
        // at the file's end leaves it, it starts as far into the
        // repetitions (R16).
        let mut past = clip(true);
        past.geometry.region_start = 425;
        assert_eq!(past.run_at(0, 1000), (Some(125), 75));
        assert_eq!(past.run_at(0, 1000), looped.run_at(375, 1000));
    }

    // Spec: audio-clips R5, R6
    #[test]
    fn fades_rise_and_fall_in_straight_lines_from_the_clips_edges() {
        let faded = clip(false);
        // In from silence over 10 frames, out to silence over 20, at half
        // gain between.
        assert_eq!(faded.level(0), 0.0);
        assert_eq!(faded.level(5), 0.25);
        assert_eq!(faded.level(10), 0.5);
        assert_eq!(faded.level(980), 0.5);
        assert_eq!(faded.level(990), 0.25);
        assert_eq!(faded.level(999), 0.5 / 20.0);
        // Overlapping fades: the lower applies.
        let mut short = faded;
        short.geometry.end = short.geometry.start + 16;
        short.fade_in = 12;
        short.fade_out = 12;
        assert_eq!(short.level(6), 0.5 * 6.0 / 12.0);
        assert_eq!(short.level(8), 0.5 * 8.0 / 12.0);
        assert_eq!(short.level(10), 0.5 * 6.0 / 12.0);
    }

    // Spec: audio-clips R23, R24. A curve bows a fade by the law x^p with
    // p = 4^(-curve), and a crossfade's fade runs from before the clip's start
    // to after its end.
    #[test]
    fn curves_bow_the_fades_and_a_crossfade_reaches_past_the_edges() {
        let mut bowed = clip(false);
        bowed.shape.in_power = bl_model::fade_power(0.5);
        bowed.shape.out_power = bl_model::fade_power(-0.5);
        assert_eq!(bowed.shape.in_power, 0.5);
        assert_eq!(bowed.level(0), 0.0);
        assert_eq!(bowed.level(5), 0.5 * 0.5f64.powf(0.5));
        assert_eq!(bowed.level(10), 0.5);
        assert_eq!(bowed.level(990), 0.5 * 0.5f64.powf(2.0));
        // A later clip's fade-in: 30 frames before its start and 50 after.
        let mut later = clip(false);
        later.fade_in = 50;
        later.fade_out = 0;
        later.shape.lead = 30;
        later.shape.before = 30;
        later.geometry.start -= 30;
        assert_eq!(later.level(0), 0.0);
        assert_eq!(later.level(30), 0.5 * 30.0 / 80.0);
        assert_eq!(later.level(70), 0.5 * 70.0 / 80.0);
        assert_eq!(later.level(80), 0.5, "the peak, 50 frames after the start");
        // An earlier clip's fade-out: 40 frames before its end and 25 after.
        let mut earlier = clip(false);
        earlier.fade_in = 0;
        earlier.fade_out = 40;
        earlier.shape.after = 25;
        earlier.shape.tail = 27;
        earlier.geometry.end += 27;
        assert_eq!(earlier.level(959), 0.5);
        assert_eq!(earlier.level(960), 0.5);
        assert_eq!(earlier.level(980), 0.5 * 45.0 / 65.0);
        assert_eq!(earlier.level(1000), 0.5 * 25.0 / 65.0);
        assert_eq!(earlier.level(1024), 0.5 * 1.0 / 65.0);
        assert_eq!(earlier.level(1025), 0.0, "the span's far end");
    }

    // Spec: audio-clips R3, R16
    #[test]
    fn the_start_marker_falls_at_the_clips_start_in_time() {
        use bl_core::time::{Bpm, Ticks};
        let rate = SampleRate::HZ_44100;
        // A ramp, so the clip's start falls between frames.
        let tempo = TempoMap::from_breakpoints([
            (Ticks::ZERO, Bpm::new(97.0).unwrap()),
            (Ticks::WHOLE * 4, Bpm::new(131.0).unwrap()),
        ])
        .unwrap();
        let audio = AudioData {
            start: 0.123_45,
            ..AudioData::new(ContentHash([1; 32]))
        };
        let id = ClipId(bl_core::id::Id::new(1).unwrap());
        let whole = Clip::new(
            id,
            Ticks::QUARTER + Ticks(777),
            Ticks::WHOLE * 3,
            ClipData::Audio(audio),
        );
        let geometry = |clip: &Clip| {
            let ClipData::Audio(audio) = &clip.data else {
                unreachable!("an audio clip")
            };
            Geometry::of(
                clip,
                audio,
                &tempo,
                rate,
                Some(1_000_000),
                true,
                &no_media(),
            )
        };
        let g = geometry(&whole);
        let r = rate.as_f64();
        let exact = 0.123_45 * r;
        assert!((g.region_start as f64 - exact).abs() <= 1.0, "{g:?}");
        // Every part of the clip, cut anywhere, plays the same file frame at
        // each song frame as the whole clip did.
        for cut in (1..400).map(|i| whole.start + Ticks(i * 7_919)) {
            let part = whole.part(cut, whole.end(), &tempo);
            let p = geometry(&part);
            for at in [p.start, p.start + 1, p.end - 1] {
                assert_eq!(
                    p.file_frame(at),
                    g.file_frame(at),
                    "cut at {cut:?}, frame {at}"
                );
            }
        }
    }

    // Spec: audio-clips R16. A looped clip's part has its Start marker
    // wrapped back into the brace by whole lengths of it, in seconds. Where
    // the brace's ends fall on frames, the part plays exactly what the whole
    // clip did; where they don't, a wrap in seconds and the player's wrap in
    // whole frames differ by the brace's fraction of a frame each time
    // round, so the part drifts from the whole by at most that much per
    // wrap, plus a frame of rounding.
    #[test]
    fn a_looped_parts_start_marker_wraps_exactly_where_the_brace_falls_on_frames() {
        use bl_core::time::Ticks;
        let rate = SampleRate::HZ_44100;
        let tempo = TempoMap::default();
        let id = ClipId(bl_core::id::Id::new(1).unwrap());
        let r = rate.as_f64();
        for (loop_end, exact) in [(0.81, true), (0.812_3, false)] {
            let audio = AudioData {
                start: 0.0123,
                looping: true,
                loop_start: 0.5,
                loop_end: Some(loop_end),
                ..AudioData::new(ContentHash([1; 32]))
            };
            let whole = Clip::new(id, Ticks(3_141), Ticks::WHOLE * 8, ClipData::Audio(audio));
            let geometry = |clip: &Clip| {
                let ClipData::Audio(audio) = &clip.data else {
                    unreachable!("an audio clip")
                };
                Geometry::of(
                    clip,
                    audio,
                    &tempo,
                    rate,
                    Some(1_000_000),
                    true,
                    &no_media(),
                )
            };
            let g = geometry(&whole);
            let len = (g.loop_end - g.loop_start) as i64;
            let fraction = ((loop_end - 0.5) * r - len as f64).abs();
            for i in 1..200 {
                let cut = whole.start + Ticks(i * 56_789);
                let part = whole.part(cut, whole.end(), &tempo);
                let p = geometry(&part);
                let ClipData::Audio(moved) = part.data else {
                    unreachable!("an audio clip")
                };
                assert!(
                    moved.start < loop_end,
                    "the marker wrapped: {}",
                    moved.start
                );
                // How many times round the brace the whole clip has gone.
                let wraps = ((0.0123 + tempo.ticks_to_seconds(cut)
                    - tempo.ticks_to_seconds(whole.start)
                    - 0.5)
                    / (loop_end - 0.5))
                    .floor()
                    .max(0.0);
                for at in [p.start, p.start + 1, p.end - 1] {
                    let (a, b) = (p.file_frame(at).unwrap(), g.file_frame(at).unwrap());
                    let apart = (a as i64 - b as i64).rem_euclid(len);
                    let apart = apart.min(len - apart);
                    if exact {
                        assert_eq!(apart, 0, "cut at {cut:?}");
                    } else {
                        assert!(
                            apart as f64 <= wraps * fraction + 1.0,
                            "cut at {cut:?}: {apart} frames after {wraps} wraps"
                        );
                    }
                }
            }
        }
    }
}

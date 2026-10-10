//! Audio clip playback (WP-1.4): each audio track's clip player, the clips it
//! plays, and where their audio comes from. The rules, and the Live sources
//! behind them, are in `docs/specs/audio-clips.md`.
//!
//! Every audio track has a [`ClipPlayer`] at the head of its device chain
//! ([`NodeKey::Player`](crate::NodeKey::Player)). The compiler works out the
//! track's clips off the audio thread: where each starts and ends in song
//! frames, through the tempo map; which frames of its file it plays, from
//! its Start and End markers and its loop; its gain and fades in frames; and
//! its audio, at the engine's rate, from the [`MediaLibrary`] in the
//! [`Registry`](crate::Registry). That list reaches the player as node data
//! ([`Sender::send_data`](crate::Sender::send_data)), so making, moving, or
//! editing a clip never builds a new program, and the player swaps the new
//! list in at a block boundary. The list it lets go of (with its references
//! to the media) is freed on the drop thread, never on the audio thread.
//!
//! A clip plays unwarped (until Phase 4): one frame of its file, at the
//! engine's rate, to each frame of the song, whatever the tempo. The audio
//! comes through [`ClipSource`], which media decoded into memory implements
//! ([`SampleData`]); disk streaming (WP-1.3) supplies sources of its own
//! through the same seam (see [`ClipSource`]).

use std::collections::BTreeMap;
use std::sync::Arc;

use bl_core::time::{SampleRate, TempoMap};
use bl_media::{MediaStore, SampleData, convert_sample_rate};
use bl_model::{ClipData, ContentHash, Track};

use crate::node::{Block, Node, NodeConfig, NodeData};

/// Audio a clip plays, at the engine's sample rate, read on the audio
/// thread.
///
/// The clip player works out, for each stretch of a block, which frames of
/// the file it plays (from the song position, the clip's markers, and its
/// loop) and reads them with [`read`](Self::read), a channel at a time. Every
/// method must be real-time safe: no allocation, lock, or blocking call.
///
/// A source is shared and read through `&self`, which fits audio decoded
/// into memory. Disk streaming (WP-1.3) reads through a source with state of
/// its own (`bl_media::AudioSource`, which takes `&mut self`); when it lands,
/// the player keeps one such source per clip, handed over with the clip
/// list, and reads it where it now calls this trait. The positions it asks
/// for, which are the clip's behaviour, stay as they are.
pub trait ClipSource: Send + Sync + 'static {
    /// The number of channels, at least 1.
    fn channels(&self) -> usize;

    /// The length in frames at the engine's rate.
    fn frames(&self) -> u64;

    /// Writes `out.len()` frames of channel `channel` (less than
    /// [`channels`](Self::channels)), from frame `position`, into `out`,
    /// with silence past the end.
    fn read(&self, channel: usize, position: u64, out: &mut [f32]);
}

impl ClipSource for SampleData {
    fn channels(&self) -> usize {
        self.channel_count()
    }

    fn frames(&self) -> u64 {
        SampleData::frames(self) as u64
    }

    fn read(&self, channel: usize, position: u64, out: &mut [f32]) {
        let samples = self.channel(channel).unwrap_or(&[]);
        let start = usize::try_from(position)
            .unwrap_or(usize::MAX)
            .min(samples.len());
        let have = (samples.len() - start).min(out.len());
        out[..have].copy_from_slice(&samples[start..start + have]);
        out[have..].fill(0.0);
    }
}

/// Where the compiler finds the audio that clips name by content hash, ready
/// for the audio thread at the engine's rate. It's asked off the audio
/// thread, so it may decode, convert, and allocate.
///
/// The [`MediaStore`] is one: it checks the file still holds the media,
/// decodes it, converts it to the rate asked for, and shares what it has
/// already loaded. [`MediaMap`] holds audio made in memory, for tests and
/// previews.
pub trait MediaLibrary: Send + Sync {
    /// The audio of `media` at `rate`, or `None` if it's missing, has
    /// changed, or doesn't decode. A clip without its audio plays silence.
    fn audio(&self, media: ContentHash, rate: SampleRate) -> Option<Arc<dyn ClipSource>>;
}

impl MediaLibrary for MediaStore {
    fn audio(&self, media: ContentHash, rate: SampleRate) -> Option<Arc<dyn ClipSource>> {
        let hash = bl_media::ContentHash::from_bytes(media.0);
        let data: Arc<SampleData> = self.load_at_rate(hash, rate.hz()).ok()?;
        Some(data)
    }
}

/// Audio held in memory, by content hash: a [`MediaLibrary`] for tests,
/// golden renders, and previews, which converts each to the engine's rate
/// when asked.
#[derive(Clone, Debug, Default)]
pub struct MediaMap {
    files: BTreeMap<ContentHash, Arc<SampleData>>,
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
    fn audio(&self, media: ContentHash, rate: SampleRate) -> Option<Arc<dyn ClipSource>> {
        let audio = self.files.get(&media)?;
        if audio.sample_rate() == rate.hz() {
            return Some(Arc::clone(audio) as Arc<dyn ClipSource>);
        }
        let converted = convert_sample_rate(audio, rate.hz()).ok()?;
        Some(Arc::new(converted))
    }
}

/// One clip, placed in song frames, with everything in frames at the
/// engine's rate.
pub(crate) struct PlacedClip {
    /// The song frame of the clip's start, which it plays.
    start: i64,
    /// The song frame of its end, which it doesn't.
    end: i64,
    source: Arc<dyn ClipSource>,
    /// The file frame at the clip's start: its Start marker.
    region_start: u64,
    /// With the loop off, the file frame it stops at: its End marker.
    region_end: u64,
    looping: bool,
    loop_start: u64,
    loop_end: u64,
    gain: f64,
    fade_in: u64,
    fade_out: u64,
}

impl PlacedClip {
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

    /// The clip's gain with its fades, `elapsed` frames after its start.
    fn level(&self, elapsed: u64) -> f64 {
        let mut fade = 1.0f64;
        if elapsed < self.fade_in {
            fade = elapsed as f64 / self.fade_in as f64;
        }
        let length = (self.end - self.start) as u64;
        let left = length.saturating_sub(elapsed);
        if left < self.fade_out {
            fade = fade.min(left as f64 / self.fade_out as f64);
        }
        self.gain * fade
    }
}

/// A track's clips, as its player plays them: sorted by start, with each
/// one's reach (the latest end among it and the clips before it), so the
/// player finds the clips under a block by binary search.
pub(crate) struct ClipList {
    clips: Vec<PlacedClip>,
    reach: Vec<i64>,
}

/// What [`ClipList::build`] made: the list, and the media it couldn't find.
pub(crate) struct Built {
    pub(crate) list: ClipList,
    pub(crate) missing: Vec<ContentHash>,
}

impl ClipList {
    /// The clips of `track` under `tempo` at `rate`, with their audio from
    /// `media`. A deactivated clip, a clip shorter than a frame, and a clip
    /// whose audio `media` can't supply (reported as missing) aren't in it.
    pub(crate) fn build(
        track: &Track,
        tempo: &TempoMap,
        rate: SampleRate,
        media: Option<&dyn MediaLibrary>,
    ) -> Built {
        let mut clips = Vec::new();
        let mut missing = Vec::new();
        let mut sources: BTreeMap<ContentHash, Option<Arc<dyn ClipSource>>> = BTreeMap::new();
        let frames =
            |seconds: f64| u64::try_from(rate.seconds_to_frames(seconds).get()).unwrap_or(0);
        for clip in track.clips() {
            let ClipData::Audio(audio) = &clip.data else {
                continue;
            };
            if clip.mute {
                continue;
            }
            let start = tempo.ticks_to_frames(clip.start, rate).get();
            let end = tempo.ticks_to_frames(clip.end(), rate).get();
            if end <= start {
                continue;
            }
            let source = sources
                .entry(audio.media)
                .or_insert_with(|| media.and_then(|media| media.audio(audio.media, rate)));
            let Some(source) = source.clone().filter(|source| source.channels() > 0) else {
                missing.push(audio.media);
                continue;
            };
            let length = source.frames();
            clips.push(PlacedClip {
                start,
                end,
                region_start: frames(audio.start),
                region_end: audio.end.map_or(length, frames),
                looping: audio.looping,
                loop_start: frames(audio.loop_start),
                loop_end: audio.loop_end.map_or(length, frames),
                gain: audio.gain,
                fade_in: frames(audio.fade_in),
                fade_out: frames(audio.fade_out),
                source,
            });
        }
        clips.sort_by_key(|clip| (clip.start, clip.end));
        let mut reach = Vec::with_capacity(clips.len());
        let mut furthest = i64::MIN;
        for clip in &clips {
            furthest = furthest.max(clip.end);
            reach.push(furthest);
        }
        missing.sort_unstable();
        missing.dedup();
        Built {
            list: Self { clips, reach },
            missing,
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
/// the song position says (`docs/specs/audio-clips.md`).
///
/// Its clips arrive as node data, a list the compiler made; until the first
/// one does, and while the song is stopped, it adds nothing. Clips that
/// overlap each play, summed (R8).
pub struct ClipPlayer {
    clips: Option<NodeData>,
    /// One channel of a block, read from a source.
    scratch: Vec<f32>,
}

impl ClipPlayer {
    /// A player with no clips, for blocks of up to `config.max_frames`.
    #[must_use]
    pub fn new(config: &NodeConfig) -> Self {
        Self {
            clips: None,
            scratch: vec![0.0; config.max_frames],
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
/// `clip`, to the block.
fn render(
    clip: &PlacedClip,
    block: &mut Block<'_>,
    range: core::ops::Range<usize>,
    elapsed: u64,
    scratch: &mut [f32],
) {
    let channels = block.channels();
    let source_channels = clip.source.channels();
    let mut at = range.start;
    let mut elapsed = elapsed;
    while at < range.end {
        let (position, run) = clip.run_at(elapsed, range.end - at);
        if let Some(position) = position {
            let read = &mut scratch[..run];
            for c in 0..channels {
                // Spec: audio-clips R9
                clip.source.read(c.min(source_channels - 1), position, read);
                let out = &mut block.channel_mut(c)[at..at + run];
                for (i, (sum, &sample)) in out.iter_mut().zip(read.iter()).enumerate() {
                    *sum += (f64::from(sample) * clip.level(elapsed + i as u64)) as f32;
                }
            }
        }
        at += run;
        elapsed += run as u64;
    }
}

impl Node for ClipPlayer {
    fn process(&mut self, block: &mut Block<'_>) {
        let transport = block.transport();
        // Spec: audio-clips R1
        if !transport.playing {
            return;
        }
        let Self { clips, scratch } = self;
        let Some(list) = clips.as_ref().and_then(|d| d.downcast_ref::<ClipList>()) else {
            return;
        };
        let from = transport.position.get();
        let to = from + block.frames() as i64;
        let first = list.reach.partition_point(|&reach| reach <= from);
        for clip in &list.clips[first..] {
            if clip.start >= to {
                break;
            }
            if clip.end <= from {
                continue;
            }
            let start = clip.start.max(from);
            let end = clip.end.min(to);
            let range = (start - from) as usize..(end - from) as usize;
            render(clip, block, range, (start - clip.start) as u64, scratch);
        }
    }

    fn receive_data(&mut self, data: NodeData) {
        let empty = data
            .downcast_ref::<ClipList>()
            .is_none_or(ClipList::is_empty);
        // Dropping the list it had, or an empty new one, only lets go of it:
        // the drop thread frees it, with the media it refers to.
        let old = if empty {
            drop(data);
            self.clips.take()
        } else {
            self.clips.replace(data)
        };
        drop(old);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(looping: bool) -> PlacedClip {
        PlacedClip {
            start: 100,
            end: 1100,
            source: Arc::new(SampleData::new(48_000, vec![vec![0.0; 500]]).unwrap()),
            region_start: 50,
            region_end: 300,
            looping,
            loop_start: 100,
            loop_end: 200,
            gain: 0.5,
            fade_in: 10,
            fade_out: 20,
        }
    }

    // Spec: audio-clips R3
    #[test]
    fn an_unlooped_clip_plays_from_its_start_marker_to_its_end_marker() {
        let clip = clip(false);
        assert_eq!(clip.run_at(0, 64), (Some(50), 64));
        assert_eq!(clip.run_at(200, 64), (Some(250), 50));
        assert_eq!(clip.run_at(250, 64), (None, 64));
        assert_eq!(clip.run_at(u64::MAX, 64), (None, 64));
    }

    // Spec: audio-clips R4
    #[test]
    fn a_looped_clip_plays_into_its_loop_and_repeats_it() {
        let clip = clip(true);
        // From the Start marker (50) through the brace's start (100) to its
        // end (200), then from 100 again.
        assert_eq!(clip.run_at(0, 1000), (Some(50), 150));
        assert_eq!(clip.run_at(149, 10), (Some(199), 1));
        assert_eq!(clip.run_at(150, 1000), (Some(100), 100));
        assert_eq!(clip.run_at(275, 1000), (Some(125), 75));
        // A Start marker inside the brace starts there.
        let mut inside = clip;
        inside.region_start = 150;
        assert_eq!(inside.run_at(0, 1000), (Some(150), 50));
        assert_eq!(inside.run_at(50, 1000), (Some(100), 100));
    }

    // Spec: audio-clips R5, R6
    #[test]
    fn fades_rise_and_fall_in_straight_lines_from_the_clips_edges() {
        let clip = clip(false);
        // In from silence over 10 frames, out to silence over 20, at half
        // gain between.
        assert_eq!(clip.level(0), 0.0);
        assert_eq!(clip.level(5), 0.25);
        assert_eq!(clip.level(10), 0.5);
        assert_eq!(clip.level(980), 0.5);
        assert_eq!(clip.level(990), 0.25);
        assert_eq!(clip.level(999), 0.5 / 20.0);
        // Overlapping fades: the lower applies.
        let mut short = clip;
        short.end = short.start + 16;
        short.fade_in = 12;
        short.fade_out = 12;
        assert_eq!(short.level(6), 0.5 * 6.0 / 12.0);
        assert_eq!(short.level(8), 0.5 * 8.0 / 12.0);
        assert_eq!(short.level(10), 0.5 * 6.0 / 12.0);
    }

    #[test]
    fn sample_data_reads_with_silence_past_its_end() {
        let data = SampleData::new(48_000, vec![vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]]).unwrap();
        let mut out = [9.0f32; 4];
        data.read(1, 1, &mut out);
        assert_eq!(out, [5.0, 6.0, 0.0, 0.0]);
        data.read(0, 7, &mut out);
        assert_eq!(out, [0.0; 4]);
        assert_eq!(ClipSource::frames(&data), 3);
        assert_eq!(ClipSource::channels(&data), 2);
    }
}

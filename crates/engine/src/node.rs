//! The node: the unit of audio processing in an engine program.

use core::fmt;
use core::ops::Range;
use std::any::Any;
use std::sync::LazyLock;

use bl_core::time::{Frames, SampleRate, TempoMap};
use bl_model::{DeviceId, ParamId, TrackId};
use rtgc::{ArcGc, LocalRtGc};

/// What a node is in the document, which stays the same from one compile to
/// the next. The compiler moves a node instance into the new program when
/// its key survives, so the node keeps its state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum NodeKey {
    /// A device in a track's chain.
    Device(DeviceId),
    /// An audio track's clip player, at the head of its chain: it plays the
    /// track's clips, which reach it as node data
    /// ([`clips`](crate::clips)).
    Player(TrackId),
    /// A track's mixer, after its device chain: pan, volume, the Track
    /// Activator, and the track's meters. Its parameters are
    /// [`mixer::track`](crate::mixer::track).
    Mixer(TrackId),
    /// A track's output into its group or Main, which solo silences. Its
    /// parameters are [`mixer::route`](crate::mixer::route).
    Output(TrackId),
    /// A track's send to a return track. Its parameters are
    /// [`mixer::route`](crate::mixer::route).
    Send {
        /// The sending track.
        track: TrackId,
        /// The return track.
        to: TrackId,
    },
    /// A track's own input taken from another track, or from Main for
    /// Resampling (the routing spec). Its parameters are
    /// [`mixer::route`](crate::mixer::route).
    Input(TrackId),
}

impl fmt::Display for NodeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Device(device) => write!(f, "{device}"),
            Self::Player(track) => write!(f, "the clip player of {track}"),
            Self::Mixer(track) => write!(f, "the mixer of {track}"),
            Self::Output(track) => write!(f, "the output of {track}"),
            Self::Send { track, to } => write!(f, "the send from {track} to {to}"),
            Self::Input(track) => write!(f, "the input of {track}"),
        }
    }
}

/// A parameter's new value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamValue {
    /// A value in the parameter's own units.
    Set(f64),
    /// The parameter's default: the document no longer sets it.
    Default,
}

/// A parameter change for a node, at a frame within the block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeEvent {
    /// The frame in the block where the change takes effect: the first frame
    /// with the new value. Always less than the block's length.
    pub offset: u32,
    /// The parameter.
    pub param: ParamId,
    /// The new value.
    pub value: ParamValue,
}

/// What every node in a program is made for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeConfig {
    /// The sample rate.
    pub sample_rate: SampleRate,
    /// The most frames a block can have.
    pub max_frames: usize,
    /// Channels in every buffer.
    pub channels: usize,
}

/// The transport as a block starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransportInfo {
    /// Whether the song is playing.
    pub playing: bool,
    /// The song position of the block's first frame.
    pub position: Frames,
}

/// Data the compiler made off the audio thread for one node, such as a clip's
/// notes or decoded audio, shared and read-only.
///
/// Dropping it is real-time safe: the drop thread frees it.
pub struct NodeData(pub(crate) ArcGc<dyn Any + Send + Sync, LocalRtGc>);

impl NodeData {
    /// The data, if it's a `T`.
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        (*self.0).downcast_ref()
    }
}

impl Clone for NodeData {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl fmt::Debug for NodeData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NodeData")
    }
}

/// A node: a device, a track's mixer, or anything else that processes audio
/// in a program.
///
/// A node is made off the audio thread, where it may allocate, and from then
/// on runs on the audio thread, where it must follow the real-time rules in
/// the architecture notes: no allocation, no locks, no blocking calls, and
/// bounded work in every method. The engine drops nodes only on the drop
/// thread.
pub trait Node: Send + 'static {
    /// Processes one block in place: the block's audio holds the node's input
    /// on entry and must hold its output on return. Parameter changes arrive
    /// as the block's [events](Block::events), each at the frame it takes
    /// effect; [`segments`] walks a block between them.
    fn process(&mut self, block: &mut Block<'_>);

    /// Takes parameter changes while the node is switched off and doesn't
    /// process, as CLAP's `flush` does; their offsets mean nothing here. A
    /// node that keeps parameter values should apply them, or it comes back
    /// on with stale ones. The default ignores them.
    fn flush(&mut self, events: &[NodeEvent]) {
        let _ = events;
    }

    /// Takes new data for the node, such as a clip's notes after an edit. The
    /// default drops it, which is real-time safe.
    fn receive_data(&mut self, data: NodeData) {
        drop(data);
    }

    /// How many frames the node delays its input by, for delay compensation
    /// (WP-2.11).
    fn latency(&self) -> u32 {
        0
    }
}

/// One block of audio for a node to process, with what it needs to know.
///
/// A block is one unbroken stretch of the song: the engine splits its
/// device blocks wherever the transport starts, stops, jumps, or loops, so
/// the song position of frame `i` is the transport's position plus `i`.
pub struct Block<'a> {
    pub(crate) data: &'a mut [f32],
    pub(crate) channels: usize,
    pub(crate) frames: usize,
    pub(crate) stride: usize,
    pub(crate) events: &'a [NodeEvent],
    pub(crate) transport: TransportInfo,
    pub(crate) sample_rate: SampleRate,
    pub(crate) tempo: &'a TempoMap,
}

/// The tempo map of a block made outside a program: 120 BPM throughout.
static DEFAULT_TEMPO: LazyLock<TempoMap> = LazyLock::new(TempoMap::default);

impl<'a> Block<'a> {
    /// A block over planar audio: `channels` channels of `frames` frames,
    /// where channel `c` starts at `data[c * stride]`. For tests of nodes
    /// outside a program. Its tempo map is 120 BPM throughout unless
    /// [`Block::with_tempo`] gives another.
    ///
    /// # Panics
    ///
    /// If `frames` is larger than `stride`, `data` is too short, or an event
    /// isn't inside the block.
    pub fn new(
        data: &'a mut [f32],
        channels: usize,
        frames: usize,
        stride: usize,
        events: &'a [NodeEvent],
        transport: TransportInfo,
        sample_rate: SampleRate,
    ) -> Self {
        assert!(
            frames <= stride,
            "{frames} frames don't fit a stride of {stride}"
        );
        assert!(
            channels == 0 || data.len() >= (channels - 1) * stride + frames,
            "{channels} channels of {frames} frames don't fit {} samples",
            data.len()
        );
        assert!(
            events.iter().all(|e| (e.offset as usize) < frames),
            "an event is outside the block"
        );
        Self {
            data,
            channels,
            frames,
            stride,
            events,
            transport,
            sample_rate,
            tempo: &DEFAULT_TEMPO,
        }
    }

    /// The same block under another tempo map.
    #[must_use]
    pub fn with_tempo(self, tempo: &'a TempoMap) -> Self {
        Self { tempo, ..self }
    }

    /// Frames in the block.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Channels in the block.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// The sample rate.
    pub fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    /// The transport as the block starts.
    pub fn transport(&self) -> TransportInfo {
        self.transport
    }

    /// The song's tempo map, which places ticks on frames: the song
    /// position of frame `i` in ticks is
    /// `tempo().frames_to_ticks(transport().position + i, sample_rate())`.
    /// Its conversions allocate nothing, so a node may call them.
    pub fn tempo(&self) -> &'a TempoMap {
        self.tempo
    }

    /// The parameter changes in this block, in the order they take effect.
    /// The slice outlives the borrow of the block, so a node can walk it
    /// while it writes audio.
    pub fn events(&self) -> &'a [NodeEvent] {
        self.events
    }

    /// One channel's samples.
    ///
    /// # Panics
    ///
    /// If `index` isn't less than [`channels`](Self::channels).
    pub fn channel(&self, index: usize) -> &[f32] {
        assert!(index < self.channels, "channel {index} out of range");
        let start = index * self.stride;
        &self.data[start..start + self.frames]
    }

    /// One channel's samples, writable.
    ///
    /// # Panics
    ///
    /// If `index` isn't less than [`channels`](Self::channels).
    pub fn channel_mut(&mut self, index: usize) -> &mut [f32] {
        assert!(index < self.channels, "channel {index} out of range");
        let start = index * self.stride;
        &mut self.data[start..start + self.frames]
    }

    /// Every channel's samples, writable, in order.
    pub fn channels_mut(&mut self) -> impl Iterator<Item = &mut [f32]> {
        let frames = self.frames;
        self.data
            .chunks_mut(self.stride.max(1))
            .take(self.channels)
            .map(move |channel| &mut channel[..frames])
    }
}

/// A stretch of a block between parameter changes, with the changes that
/// take effect at its first frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment<'a> {
    /// The frames, within the block.
    pub range: Range<usize>,
    /// The changes to apply before rendering `range`.
    pub events: &'a [NodeEvent],
}

/// Splits a block of `frames` frames at its events, so a node can apply each
/// change on its exact frame:
///
/// ```ignore
/// for segment in segments(block.events(), block.frames()) {
///     for event in segment.events {
///         self.apply(event);
///     }
///     self.render(block, segment.range);
/// }
/// ```
///
/// `events` must be in order of offset, as [`Block::events`] is.
pub fn segments(events: &[NodeEvent], frames: usize) -> Segments<'_> {
    Segments {
        events,
        frames,
        position: 0,
        next: 0,
    }
}

/// The iterator [`segments`] returns.
#[derive(Clone, Debug)]
pub struct Segments<'a> {
    events: &'a [NodeEvent],
    frames: usize,
    position: usize,
    next: usize,
}

impl<'a> Iterator for Segments<'a> {
    type Item = Segment<'a>;

    fn next(&mut self) -> Option<Segment<'a>> {
        if self.position >= self.frames {
            return None;
        }
        let start = self.position;
        let first = self.next;
        while self
            .events
            .get(self.next)
            .is_some_and(|event| event.offset as usize <= start)
        {
            self.next += 1;
        }
        let end = self.events.get(self.next).map_or(self.frames, |event| {
            (event.offset as usize).min(self.frames)
        });
        self.position = end;
        Some(Segment {
            range: start..end,
            events: &self.events[first..self.next],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(offset: u32, value: f64) -> NodeEvent {
        NodeEvent {
            offset,
            param: ParamId(0),
            value: ParamValue::Set(value),
        }
    }

    #[test]
    fn a_block_without_events_is_one_segment() {
        let all: Vec<_> = segments(&[], 64).collect();
        assert_eq!(
            all,
            vec![Segment {
                range: 0..64,
                events: &[]
            }]
        );
        assert_eq!(segments(&[], 0).count(), 0);
    }

    #[test]
    fn events_split_the_block_where_they_land() {
        let events = [
            event(0, 1.0),
            event(10, 2.0),
            event(10, 3.0),
            event(63, 4.0),
        ];
        let all: Vec<_> = segments(&events, 64).collect();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].range, 0..10);
        assert_eq!(all[0].events, &events[..1]);
        assert_eq!(all[1].range, 10..63);
        assert_eq!(all[1].events, &events[1..3]);
        assert_eq!(all[2].range, 63..64);
        assert_eq!(all[2].events, &events[3..]);
    }

    #[test]
    fn a_block_shows_its_channels() {
        let mut data = vec![0.0f32; 2 * 8];
        let mut block = Block::new(
            &mut data,
            2,
            5,
            8,
            &[],
            TransportInfo::default(),
            SampleRate::HZ_48000,
        );
        block.channel_mut(1).fill(1.0);
        assert_eq!(block.channel(0), &[0.0; 5]);
        assert_eq!(block.channel(1), &[1.0; 5]);
        assert_eq!(block.channels_mut().count(), 2);
        assert_eq!(&data[8..13], &[1.0; 5]);
        assert_eq!(
            &data[13..],
            &[0.0; 3],
            "frames past the block are untouched"
        );
    }
}

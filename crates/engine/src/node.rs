//! The node: the unit of audio processing in an engine program.

use core::fmt;
use core::ops::Range;
use std::any::Any;
use std::sync::LazyLock;

use bl_core::time::{Frames, SampleRate, TempoMap};
use bl_model::{DeviceId, ParamId, TrackId};
use rtgc::{ArcGc, LocalRtGc};

use crate::session::SessionPlayback;

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
    /// A track's own input taken from the audio interface, Ext. In
    /// (WP-2.13). Its parameters are
    /// [`mixer::route`](crate::mixer::route); its source is the device
    /// input the processor was given.
    DeviceInput(TrackId),
    /// The track a device's sidechain takes as its Source (WP-2.19b). Its
    /// parameters are [`mixer::route`](crate::mixer::route).
    SidechainInput(DeviceId),
    /// A device's sidechain settings: External, SC Gain, Mix, and Listen,
    /// which make what the device's detector hears. Its parameters are
    /// [`sidechain`](crate::sidechain).
    Sidechain(DeviceId),
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
            Self::DeviceInput(track) => write!(f, "the interface input of {track}"),
            Self::SidechainInput(device) => write!(f, "the sidechain input of {device}"),
            Self::Sidechain(device) => write!(f, "the sidechain of {device}"),
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

/// What a note event does (WP-2.6).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NoteKind {
    /// A key goes down.
    On,
    /// A key comes up.
    Off,
    /// A voice stops at once, as a hi-hat's choke group stops it.
    Choke,
    /// A per-note expression takes a new value (WP-2.17): the note the
    /// event names bends, presses or slides while it sounds, and no other
    /// note moves.
    Expression {
        /// Which expression.
        expression: NoteExpression,
        /// Its new value, in the expression's own units.
        value: f32,
    },
}

/// A per-note expression an MPE controller changes, on the note its
/// [`NoteEvent`] names; the note's own [`NoteEvent::note_id`] tells notes
/// on one key apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NoteExpression {
    /// Per-note pitch bend, from −1 to +1: the note moves by that fraction
    /// of the instrument's pitch-bend range.
    Bend,
    /// Per-note pressure, from 0 to 1.
    Pressure,
    /// Per-note slide (MPE's timbre, controller 74), from 0 to 1.
    Slide,
}

/// A note for a node, at a frame within the block (WP-2.6): what an
/// instrument or MIDI effect plays, and what a plug-in sends back out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteEvent {
    /// The frame in the block where it happens. Always less than the
    /// block's length.
    pub offset: u32,
    /// What it does.
    pub kind: NoteKind,
    /// The MIDI channel, from 0 to 15.
    pub channel: u8,
    /// The key, from 0 to 127.
    pub key: u8,
    /// The velocity, from 0 to 1.
    pub velocity: f32,
    /// The note's own ID, to tell notes on one key apart (MPE and CLAP's
    /// per-note expressions), or `None`.
    pub note_id: Option<u32>,
}

/// Where a node writes the notes it sends on (WP-2.6), with room for a
/// fixed number made before the audio thread uses it: a note past that is
/// dropped and counted (real-time rule 4).
#[derive(Clone, Debug)]
pub struct NoteBuffer {
    notes: Vec<NoteEvent>,
    dropped: u64,
}

impl NoteBuffer {
    /// A buffer with room for `capacity` notes.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            notes: Vec::with_capacity(capacity),
            dropped: 0,
        }
    }

    /// Adds a note, or drops and counts it if the buffer is full. Never
    /// allocates.
    pub fn push(&mut self, note: NoteEvent) {
        if self.notes.len() < self.notes.capacity() {
            self.notes.push(note);
        } else {
            self.dropped += 1;
        }
    }

    /// The notes, in the order they were written.
    #[must_use]
    pub fn notes(&self) -> &[NoteEvent] {
        &self.notes
    }

    /// Empties the buffer, keeping its room.
    pub fn clear(&mut self) {
        self.notes.clear();
    }

    /// How many notes it has dropped since it was made.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
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

    /// Whether the node played a clip in the block it last processed
    /// (WP-2.13): a track's clip player says so while a clip of its track
    /// sounds, so the track's input routes close under Monitor Auto
    /// (the routing spec, R11). The default is no.
    fn playing_clip(&self) -> bool {
        false
    }

    /// Calls `report` for each parameter the node's last block played under
    /// modulation (WP-5.9), with its base and the value it played, so the
    /// engine can publish them. The default reports none.
    fn modulated(&self, report: &mut dyn FnMut(ParamId, f64, f64)) {
        let _ = report;
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
    /// What the device's detector hears from its sidechain input, planar
    /// with the block's channels and stride, while its sidechain is on.
    pub(crate) sidechain: Option<&'a [f32]>,
    /// The notes the device plays in this block, in order of offset
    /// (WP-2.6). The engine sends none yet: MIDI clips and inputs reach
    /// devices with WP-2.1 and 2.2.
    pub(crate) notes: &'a [NoteEvent],
    /// Where the device writes the notes it sends on, if anything takes
    /// them (WP-2.6).
    pub(crate) notes_out: Option<&'a mut NoteBuffer>,
    /// The Session slots playing on the tracks, with the frame each clip
    /// launched on (WP-3.2). Empty for a block outside a program's run.
    pub(crate) session: &'a [SessionPlayback],
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
            sidechain: None,
            notes: &[],
            notes_out: None,
            session: &[],
        }
    }

    /// The same block with notes to play, in order of offset, and a buffer
    /// for the notes the node sends on (WP-2.6).
    ///
    /// # Panics
    ///
    /// If a note isn't inside the block.
    #[must_use]
    pub fn with_notes(self, notes: &'a [NoteEvent], out: &'a mut NoteBuffer) -> Self {
        assert!(
            notes.iter().all(|n| (n.offset as usize) < self.frames),
            "a note is outside the block"
        );
        Self {
            notes,
            notes_out: Some(out),
            ..self
        }
    }

    /// The same block with the Session slots playing on the tracks
    /// (WP-3.2). For tests of nodes outside a program.
    #[must_use]
    pub fn with_session(self, session: &'a [SessionPlayback]) -> Self {
        Self { session, ..self }
    }

    /// The same block with a sidechain signal: planar, with the block's
    /// channels and stride. For tests of nodes outside a program.
    ///
    /// # Panics
    ///
    /// If `sidechain` is too short.
    #[must_use]
    pub fn with_sidechain(self, sidechain: &'a [f32]) -> Self {
        assert!(
            self.channels == 0
                || sidechain.len() >= (self.channels - 1) * self.stride + self.frames,
            "the sidechain is shorter than the block"
        );
        Self {
            sidechain: Some(sidechain),
            ..self
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

    /// The Session slots playing on the tracks, each with the song frame
    /// its clip launched on (WP-3.2): a track's player sounds the slot clip
    /// whose track is its own. Empty outside a program's run.
    pub fn session(&self) -> &'a [SessionPlayback] {
        self.session
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

    /// The notes the node plays in this block, in order of offset
    /// (WP-2.6). Like [`events`](Self::events), the slice outlives the
    /// borrow of the block.
    pub fn notes(&self) -> &'a [NoteEvent] {
        self.notes
    }

    /// Sends a note on from the node, if anything takes the block's notes;
    /// otherwise it's dropped. Never allocates.
    pub fn send_note(&mut self, note: NoteEvent) {
        if let Some(out) = self.notes_out.as_deref_mut() {
            out.push(note);
        }
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

    /// What the device's detector should hear on one channel, if the
    /// device has a sidechain input and its sidechain is switched on
    /// (WP-2.19b): the sidechain's signal after its gain, mixed with the
    /// device's own input by its mix. `None` while the sidechain is off, or
    /// for a device without one: the detector hears the block's own audio.
    /// A device that declares a sidechain input
    /// ([`DeviceType::sidechain`](crate::DeviceType::sidechain)) reads it
    /// here; its output is still the block's audio, processed.
    ///
    /// # Panics
    ///
    /// If `index` isn't less than [`channels`](Self::channels).
    pub fn sidechain(&self, index: usize) -> Option<&[f32]> {
        assert!(index < self.channels, "channel {index} out of range");
        let start = index * self.stride;
        self.sidechain.map(|data| &data[start..start + self.frames])
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

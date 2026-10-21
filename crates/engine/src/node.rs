//! The node: the unit of audio processing in an engine program.

use core::fmt;
use core::ops::Range;
use std::any::Any;
use std::sync::LazyLock;

use bl_core::midi::MidiMessage;
use bl_core::time::{Frames, SampleRate, TempoMap};
use bl_model::{ChainId, DeviceId, ParamId, TrackId};
use rtgc::{ArcGc, LocalRtGc};

use crate::display::DisplayReader;
use crate::modbus::{Bus, Control, ROWS};
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
    /// A track's cue switch in Cue mode (WP-3.18): the route that carries its
    /// signal after its devices into the Cue Out's task. Its parameters are
    /// [`mixer::route`](crate::mixer::route); solo mute is the switch off.
    Cue(TrackId),
    /// The mixer of the Cue Out's task, whose volume is the Cue Volume. Its
    /// parameters are [`mixer::track`](crate::mixer::track).
    CueMixer,
    /// The mixer of a rack's chain, after the chain's devices (WP-5.1): its
    /// Volume, Pan, and Chain Activator, which solo mutes. Its parameters are
    /// [`mixer::track`](crate::mixer::track).
    ChainMixer(ChainId),
    /// The filter at a rack chain's input (WP-5.3): its key, velocity and
    /// chain select zones, which reach it as parameter changes numbered by
    /// [`zones`](crate::zones).
    ChainZones(ChainId),
    /// The note routing of a drum rack's chain, which passes on the notes
    /// its Receive answers at its Play note and remembers what sounds for
    /// its choke group (the drum racks spec, R2, R4). Its parameters are
    /// [`drum::param`](crate::drum::param).
    ChainNotes(ChainId),
    /// A drum chain's send to one of its rack's return chains, taken after
    /// the chain's mixer (R6). Its parameters are
    /// [`mixer::route`](crate::mixer::route).
    ChainSend {
        /// The drum chain.
        chain: ChainId,
        /// The return chain.
        to: ChainId,
    },
    /// A drum rack's return chain's output into a return track (R8). Its
    /// parameters are [`mixer::route`](crate::mixer::route).
    ChainOutput(ChainId),
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
            Self::Cue(track) => write!(f, "the cue switch of {track}"),
            Self::CueMixer => f.write_str("the mixer of the Cue Out"),
            Self::ChainMixer(chain) => write!(f, "the mixer of {chain}"),
            Self::ChainZones(chain) => write!(f, "the zones of {chain}"),
            Self::ChainNotes(chain) => write!(f, "the notes of {chain}"),
            Self::ChainSend { chain, to } => write!(f, "the send from {chain} to {to}"),
            Self::ChainOutput(chain) => write!(f, "the output of {chain}"),
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
    /// A control change (WP-5.44): the controller takes a new value for the
    /// whole channel. The event's key, velocity and note ID mean nothing.
    Control {
        /// The controller number, from 0 to 127 (1 is the mod wheel).
        controller: u8,
        /// Its new value, from 0 to 1.
        value: f32,
    },
    /// The channel's pitch bend (WP-5.44), as the pitch wheel sends it: the
    /// channel's notes move by that fraction of the instrument's pitch-bend
    /// range. The event's key, velocity and note ID mean nothing.
    PitchBend {
        /// From −1 to +1, 0 at the wheel's centre.
        value: f32,
    },
    /// The channel's pressure (WP-5.44), one value for all its notes. The
    /// event's key, velocity and note ID mean nothing.
    ChannelPressure {
        /// From 0 to 1.
        value: f32,
    },
}

impl NoteKind {
    /// Whether it's a control change, pitch bend or channel pressure: a
    /// message for the channel, not for a note.
    #[must_use]
    pub fn is_message(&self) -> bool {
        matches!(
            self,
            Self::Control { .. } | Self::PitchBend { .. } | Self::ChannelPressure { .. }
        )
    }
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

impl NoteEvent {
    /// A channel message at `offset`, from a MIDI control change, channel
    /// pressure or pitch bend; `None` for any other message.
    #[must_use]
    pub fn from_message(offset: u32, message: &MidiMessage) -> Option<Self> {
        let unit = |byte: u8| f32::from(byte) / 127.0;
        let kind = match message.status() & 0xF0 {
            0xB0 => NoteKind::Control {
                controller: message.data1(),
                value: unit(message.data2()),
            },
            0xD0 => NoteKind::ChannelPressure {
                value: unit(message.data1()),
            },
            0xE0 => {
                let raw = i32::from(message.data1()) | i32::from(message.data2()) << 7;
                NoteKind::PitchBend {
                    value: if raw >= 8192 {
                        (raw - 8192) as f32 / 8191.0
                    } else {
                        (raw - 8192) as f32 / 8192.0
                    },
                }
            }
            _ => return None,
        };
        Some(Self {
            offset,
            kind,
            channel: message.channel()?,
            key: 0,
            velocity: 0.0,
            note_id: None,
        })
    }

    /// A note or a channel message at `offset`, from a MIDI message: a note
    /// on or off, or a control change, channel pressure or pitch bend;
    /// `None` for any other message (IMP-24; au-hosting R4). A note-on at
    /// velocity 0 is a note-off.
    #[must_use]
    pub fn from_midi(offset: u32, message: &MidiMessage) -> Option<Self> {
        if message.is_note_on() || message.is_note_off() {
            return Some(Self {
                offset,
                kind: if message.is_note_on() {
                    NoteKind::On
                } else {
                    NoteKind::Off
                },
                channel: message.channel()?,
                key: message.data1(),
                velocity: f32::from(message.data2()) / 127.0,
                note_id: None,
            });
        }
        Self::from_message(offset, message)
    }

    /// The MIDI message of a control change, channel pressure or pitch
    /// bend; `None` for a note or an expression. The inverse of
    /// [`from_message`](Self::from_message) for the values it makes.
    #[must_use]
    pub fn message(&self) -> Option<MidiMessage> {
        let seven = |value: f32| (value.clamp(0.0, 1.0) * 127.0).round() as u8;
        let status = |high: u8| high | (self.channel & 0x0F);
        match self.kind {
            NoteKind::Control { controller, value } => {
                MidiMessage::from_bytes(&[status(0xB0), controller & 0x7F, seven(value)])
            }
            NoteKind::ChannelPressure { value } => {
                MidiMessage::from_bytes(&[status(0xD0), seven(value)])
            }
            NoteKind::PitchBend { value } => {
                let value = value.clamp(-1.0, 1.0);
                let raw = if value >= 0.0 {
                    8192 + (value * 8191.0).round() as i32
                } else {
                    8192 + (value * 8192.0).round() as i32
                };
                MidiMessage::from_bytes(&[status(0xE0), (raw & 0x7F) as u8, (raw >> 7) as u8])
            }
            _ => None,
        }
    }
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

    /// Adds a note in order of offset, after the notes at its offset, but
    /// before any note-on there if it ends a note, or drops and counts it if
    /// the buffer is full. A channel message stays after the notes sent
    /// before it. Never allocates.
    pub fn insert(&mut self, note: NoteEvent) {
        if self.notes.len() == self.notes.capacity() {
            self.dropped += 1;
            return;
        }
        let ons = |n: &NoteEvent| n.kind == NoteKind::On;
        let mut at = self
            .notes
            .iter()
            .rposition(|n| n.offset <= note.offset)
            .map_or(0, |i| i + 1);
        // Anything but a note-on, a channel message or a note's expression
        // goes before the note-ons on its frame.
        let expression = matches!(note.kind, NoteKind::Expression { .. });
        if !(ons(&note) || note.kind.is_message() || expression) {
            let frame = self.notes[..at]
                .iter()
                .rposition(|n| n.offset < note.offset)
                .map_or(0, |i| i + 1);
            at = self.notes[frame..at]
                .iter()
                .position(ons)
                .map_or(at, |i| frame + i);
        }
        self.notes.insert(at, note);
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

    /// Works out a modulator's output for a block part, before any track
    /// runs (WP-5.10; the modulation spec, R20): the node writes its rows
    /// into [`Control`]'s output and the rest of the program reads them as
    /// it runs. A node that is a modulator takes its parameter changes
    /// here, not in [`process`](Self::process), which then leaves the block
    /// as it is. Returns whether the node is a modulator; the default is
    /// no, and does nothing.
    fn control(&mut self, ctl: &mut Control<'_>) -> bool {
        let _ = ctl;
        false
    }

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

    /// Ends every note the node started and still holds, by writing a
    /// note-off for each at the block's first frame into `out`. The engine
    /// calls it while the node is switched off, and on the first block after
    /// the node was removed or moved in its chain, so no instrument further
    /// down hangs on a note the node would have ended. The node forgets the
    /// notes: calling it again writes nothing. The default writes nothing,
    /// for a node that starts no notes.
    fn release(&mut self, out: &mut NoteBuffer) {
        let _ = out;
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

    /// The reader of the node's display data, if it has a display (the
    /// device framework spec, R26): the compiler asks once, when it makes
    /// the node, and the shell reads the newest values the node publishes
    /// after each block. The default is no display.
    fn display(&mut self) -> Option<DisplayReader> {
        None
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
    /// What an external device brings back from the audio interface: the
    /// input channels it is set to, planar with the block's channels and
    /// stride (WP-5.34). `None` for every other device.
    pub(crate) external: Option<&'a [f32]>,
    /// Where a device with extra audio outputs past its main one writes
    /// them (WP-5.45): one buffer per output, in the order the device
    /// declares them, each planar with the block's channels and stride, at
    /// `output * channels * stride`. `None` for a device that has none, or
    /// one no route reads.
    pub(crate) outputs: Option<&'a mut [f32]>,
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
    /// What the modulators wrote for the frames of this block (WP-5.10).
    pub(crate) modulation: Bus<'a>,
    /// Where a modulator driven by audio, notes or MIDI writes its rows
    /// while its track runs (WP-5.11): [`ROWS`] rows, `stride` apart. `None`
    /// for any other node.
    pub(crate) rows: Option<&'a mut [f32]>,
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
            external: None,
            outputs: None,
            notes: &[],
            notes_out: None,
            session: &[],
            modulation: Bus::EMPTY,
            rows: None,
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

    /// The same block with what the modulators wrote for its frames
    /// (WP-5.10). For tests of nodes outside a program.
    #[must_use]
    pub fn with_modulation(self, modulation: Bus<'a>) -> Self {
        Self { modulation, ..self }
    }

    /// The same block with rows for a modulator driven by audio, notes or
    /// MIDI to write ([`ROWS`] of them, `stride` apart). For tests of nodes
    /// outside a program.
    ///
    /// # Panics
    ///
    /// If `rows` is too short.
    #[must_use]
    pub fn with_rows(self, rows: &'a mut [f32]) -> Self {
        assert!(
            rows.len() >= ROWS * self.stride,
            "{ROWS} rows of {} don't fit {} values",
            self.stride,
            rows.len()
        );
        Self {
            rows: Some(rows),
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

    /// The same block with what an external device brings back from the
    /// interface: planar, with the block's channels and stride. For tests of
    /// nodes outside a program.
    ///
    /// # Panics
    ///
    /// If `external` is too short.
    #[must_use]
    pub fn with_external(self, external: &'a [f32]) -> Self {
        assert!(
            self.channels == 0 || external.len() >= (self.channels - 1) * self.stride + self.frames,
            "the external input is shorter than the block"
        );
        Self {
            external: Some(external),
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

    /// What the modulators wrote for the frames of this block (WP-5.10):
    /// the contributions and holds of the slots that map the node's
    /// parameters.
    pub fn modulation(&self) -> Bus<'a> {
        self.modulation
    }

    /// Row `index` of the rows a modulator driven by audio, notes or MIDI
    /// writes (WP-5.11), over the block's frames; empty for a node that
    /// writes none.
    pub fn row_mut(&mut self, index: usize) -> &mut [f32] {
        let (stride, frames) = (self.stride, self.frames);
        match self.rows.as_deref_mut() {
            Some(rows) if index < ROWS => &mut rows[index * stride..index * stride + frames],
            _ => &mut [],
        }
    }

    /// The Session slots playing on the tracks, each with the song frame
    /// its clip launched on (WP-3.2): a track's player sounds the slot clip
    /// whose track is its own. Empty outside a program's run.
    pub fn session(&self) -> &'a [SessionPlayback] {
        self.session
    }

    /// The Session clip that sets `list`'s parameters in this block
    /// (WP-3.12): the one playing on the list's track, or holding where it
    /// stopped on a track that stays taken over.
    #[must_use]
    pub fn session_now(&self, list: &crate::AutomationList) -> crate::SessionNow<'a> {
        list.now(self.session, self.tempo, self.sample_rate)
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

    /// Sends a note on in order of offset, after the notes already at its
    /// offset, but before any note-on there if it ends a note; otherwise as
    /// [`send_note`](Self::send_note). Never allocates.
    pub fn insert_note(&mut self, note: NoteEvent) {
        if let Some(out) = self.notes_out.as_deref_mut() {
            out.insert(note);
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

    /// What an external device brings back on one channel (WP-5.34): the
    /// audio interface's input channels it is set to, a pair as left and
    /// right and a single channel on both sides, silence where the interface
    /// lacks the channel. `None` for a device that isn't an external one.
    ///
    /// # Panics
    ///
    /// If `index` isn't less than [`channels`](Self::channels).
    pub fn external(&self, index: usize) -> Option<&'a [f32]> {
        assert!(index < self.channels, "channel {index} out of range");
        let start = index * self.stride;
        self.external.map(|data| &data[start..start + self.frames])
    }

    /// Where a device with extra audio outputs writes one of them
    /// (WP-5.45): output `output`, channel `channel`, planar. `None` for a
    /// device that has no extra outputs, or one no route reads.
    ///
    /// # Panics
    ///
    /// If `channel` isn't less than [`channels`](Self::channels).
    pub fn output_mut(&mut self, output: usize, channel: usize) -> Option<&mut [f32]> {
        assert!(channel < self.channels, "channel {channel} out of range");
        let channels = self.channels;
        let stride = self.stride;
        let frames = self.frames;
        self.outputs.as_mut().and_then(|data| {
            let start = (output * channels + channel) * stride;
            data.get_mut(start..start + frames)
        })
    }

    /// Every extra output's buffers, writable, as [`output_mut`](Self::output_mut)
    /// describes them. `None` for a device that has no extra outputs, or one
    /// no route reads.
    pub fn outputs_mut(&mut self) -> Option<&mut [f32]> {
        self.outputs.as_deref_mut()
    }

    /// Output `output`'s lanes, writable, in order. `None` for a device that
    /// has no extra outputs, or one no route reads.
    pub fn output_channels_mut(
        &mut self,
        output: usize,
    ) -> Option<impl ExactSizeIterator<Item = &mut [f32]>> {
        let (channels, stride, frames) = (self.channels, self.stride, self.frames);
        let data = self.outputs.as_deref_mut()?;
        let size = channels * stride.max(1);
        let at = output * size;
        data.get_mut(at..at + size).map(|data| {
            data.chunks_exact_mut(stride.max(1))
                .take(channels)
                .map(move |channel| &mut channel[..frames])
        })
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

    /// The first two channels' samples, writable, when the block has two or
    /// more: what a device that mixes the channels into each other, such as
    /// Split Stereo Pan Mode, needs.
    pub fn channels_pair_mut(&mut self) -> Option<(&mut [f32], &mut [f32])> {
        if self.channels < 2 {
            return None;
        }
        let frames = self.frames;
        let (first, rest) = self.data.split_at_mut(self.stride.max(1));
        Some((&mut first[..frames], &mut rest[..frames]))
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

    fn message(offset: u32, bytes: &[u8]) -> NoteEvent {
        NoteEvent::from_message(offset, &MidiMessage::from_bytes(bytes).unwrap()).unwrap()
    }

    // Spec: device-framework R39
    #[test]
    fn channel_messages_round_trip_through_their_midi_bytes() {
        for bytes in [
            &[0xB2, 1, 64][..],
            &[0xB0, 127, 0],
            &[0xD5, 99],
            &[0xE3, 0, 0],
            &[0xE3, 0, 64],
            &[0xE3, 127, 127],
            &[0xE0, 1, 100],
        ] {
            let event = message(0, bytes);
            assert!(event.kind.is_message());
            assert_eq!(event.message().unwrap().bytes(), bytes, "{event:?}");
        }
        assert_eq!(
            message(7, &[0xE1, 0, 64]).kind,
            NoteKind::PitchBend { value: 0.0 }
        );
        assert_eq!(
            message(7, &[0xE1, 127, 127]).kind,
            NoteKind::PitchBend { value: 1.0 }
        );
        assert_eq!(
            message(7, &[0xE1, 0, 0]).kind,
            NoteKind::PitchBend { value: -1.0 }
        );
        assert_eq!(message(7, &[0xB4, 1, 127]).channel, 4);
        // Notes, polyphonic pressure and program changes are not channel
        // messages in this sense.
        for bytes in [&[0x90, 60, 100][..], &[0xA0, 60, 10], &[0xC0, 5]] {
            let message = MidiMessage::from_bytes(bytes).unwrap();
            assert_eq!(NoteEvent::from_message(0, &message), None);
        }
    }

    // Spec: device-framework R39; au-hosting R4 (IMP-24)
    #[test]
    fn notes_and_channel_messages_become_note_events() {
        let on =
            NoteEvent::from_midi(5, &MidiMessage::from_bytes(&[0x92, 60, 100]).unwrap()).unwrap();
        assert_eq!(on.offset, 5);
        assert_eq!(on.kind, NoteKind::On);
        assert_eq!(on.channel, 2);
        assert_eq!(on.key, 60);
        assert_eq!(on.velocity, 100.0_f32 / 127.0);
        // A note-on at velocity 0 is a note-off, as a note-off is.
        for bytes in [&[0x92, 60, 0][..], &[0x82, 60, 64]] {
            let off = NoteEvent::from_midi(6, &MidiMessage::from_bytes(bytes).unwrap()).unwrap();
            assert_eq!(off.kind, NoteKind::Off, "{bytes:?}");
            assert_eq!(off.key, 60);
        }
        // A channel message is a message.
        let control =
            NoteEvent::from_midi(7, &MidiMessage::from_bytes(&[0xB2, 1, 64]).unwrap()).unwrap();
        assert_eq!(
            control.kind,
            NoteKind::Control {
                controller: 1,
                value: 64.0_f32 / 127.0
            }
        );
        // A program change is neither a note nor a message.
        assert!(NoteEvent::from_midi(8, &MidiMessage::from_bytes(&[0xC0, 5]).unwrap()).is_none());
    }

    // Spec: device-framework R39
    #[test]
    fn a_message_keeps_its_place_among_the_notes_on_its_frame() {
        let on = NoteEvent {
            offset: 4,
            kind: NoteKind::On,
            channel: 0,
            key: 60,
            velocity: 1.0,
            note_id: None,
        };
        let off = NoteEvent {
            kind: NoteKind::Off,
            key: 62,
            ..on
        };
        let bend = message(4, &[0xE0, 0, 100]);
        let control = message(4, &[0xB0, 1, 10]);
        let mut buffer = NoteBuffer::with_capacity(8);
        // A note-off goes before a note-on on its frame; messages go after
        // whatever was sent before them, a note-on included.
        for event in [control, on, bend, off, message(2, &[0xD0, 9])] {
            buffer.insert(event);
        }
        let order: Vec<_> = buffer.notes().iter().map(|n| (n.offset, n.kind)).collect();
        assert_eq!(
            order,
            [
                (2, NoteKind::ChannelPressure { value: 9.0 / 127.0 }),
                (4, control.kind),
                (4, off.kind),
                (4, on.kind),
                (4, bend.kind),
            ]
        );
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

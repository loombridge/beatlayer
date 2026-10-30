//! MIDI input on the audio thread (WP-2.2): messages from the MIDI threads
//! placed on samples and handed to each MIDI track's input.
//!
//! The MIDI device layer (`bl-midi-io`) stamps each message with the time it
//! arrived and pushes it, as a [`MidiInputEvent`], onto a lock-free queue
//! whose consumer the [`Processor`](crate::Processor) holds
//! ([`Processor::set_midi_input`](crate::Processor::set_midi_input)). At the
//! start of each block the processor:
//!
//! 1. estimates when the block starts with a delay-locked loop over the
//!    callback's time and the engine's frame count ([`BlockClock`]), so
//!    jitter in when the callback runs doesn't move messages;
//! 2. takes every message stamped before that time, and places it at the
//!    sample its stamp gives, counted from when the previous block started:
//!    messages keep their spacing, one block later than they arrived;
//! 3. hands each message to every MIDI track whose input it matches (its
//!    source and channel), in the running program's routes, with whether the
//!    track monitors in this block.
//!
//! Nothing here allocates, locks, or blocks: the routes' buffers are
//! reserved when the program is compiled, and what doesn't fit is dropped
//! and counted. The rules are in `docs/specs/midi-io.md`, R12 to R16.
//!
//! MIDI output (WP-5.34) leaves the same way it arrived. A track's MIDI To
//! names a port, and an external instrument names one too: what reaches the
//! end of the track's chain, or the device, is rewritten onto the channel
//! and handed, stamped with the moment its frame is heard at the audio
//! interface's output, to a lock-free queue whose other end the MIDI output
//! thread reads (`docs/specs/external-devices.md`, R3 to R6, R10).

use std::time::{Duration, Instant};

use bl_core::midi::{MidiInputEvent, MidiMessage, MidiOutputEvent, MidiSource, PortId};
use bl_core::time::SampleRate;
use bl_model::{DeviceId, Document, Expression, Input, MidiTo, MpeSettings, TrackId, TrackKind};
use rtrb::{Consumer, Producer};

use crate::clock_in::{ClockIn, Timing as ClockTiming};
use crate::disjoint::Shared;
use crate::mpe::{MpeReader, MpeRole};
use crate::mpe_layout::MpeLayout;
use crate::node::{NoteBuffer, NoteEvent, NoteExpression, NoteKind};
use crate::program::Task;
use crate::tuned::{KeyMap, KeyPitches};

/// The most messages a block takes from the queue. More wait for the next
/// block, so the work per block is bounded (real-time rule 4).
pub const MIDI_EVENTS_PER_BLOCK: usize = 1024;

/// The most previews one track's input holds in a block. More are dropped
/// and counted ([`MidiCounters::dropped`]).
pub const MIDI_PREVIEWS_PER_TRACK: usize = 32;

/// The most messages one track's input holds in a block. More are dropped
/// and counted ([`MidiCounters::dropped`]).
pub const MIDI_EVENTS_PER_TRACK: usize = 256;

/// How fast the block clock follows the callback's times, in hertz: low
/// enough to smooth callback jitter to a fifth or less, high enough to lock
/// within a few seconds.
const CLOCK_BANDWIDTH_HZ: f64 = 0.5;

/// A callback this far from where the clock expects it, in nanoseconds,
/// means a gap (the device stopped, the machine slept, or the engine
/// skipped processing): the clock locks again from scratch.
const RELOCK_NS: f64 = 50_000_000.0;

/// The most messages one route sends to its port in a block. More are
/// dropped and counted ([`MidiCounters::out_dropped`]).
pub const MIDI_OUT_PER_ROUTE: usize = 1024;

/// The most held notes a program swap can end at once, for routes the new
/// program no longer has or sends elsewhere.
const ORPHANS: usize = 256;

/// A MIDI message at a frame of a block, on a track's input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MidiEvent {
    /// The frame in the block where it lands. Always less than the block's
    /// length.
    pub offset: u32,
    /// The message.
    pub message: MidiMessage,
    /// Where it came from.
    pub source: MidiSource,
    /// What it is on the track's input: from an MPE port, a note of its own,
    /// or one note's expression (the MPE spec, R11); otherwise
    /// [`MpeRole::Plain`].
    pub mpe: MpeRole,
}

/// What a MIDI track's input received in the last block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackMidiInput<'a> {
    /// The messages that matched the track's input, in order of offset.
    pub events: &'a [MidiEvent],
    /// Whether the track monitors in this block (its Monitor switch and its
    /// arm, `docs/specs/midi-io.md` R10), so its devices play what arrives.
    pub monitored: bool,
}

/// Counts the MIDI input stage keeps, since the engine started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MidiCounters {
    /// Messages taken from the queue.
    pub received: u64,
    /// Messages handed to a track's input (once per track).
    pub delivered: u64,
    /// Messages that matched no track's input.
    pub unrouted: u64,
    /// Messages that missed the block they belonged in: stamped before the
    /// previous block's callback ran, but not on the queue when it did. They
    /// land on the first frame of the block that takes them.
    pub late: u64,
    /// Messages dropped because a track's input was full.
    pub dropped: u64,
    /// Times the block clock locked again after a gap.
    pub relocks: u64,
    /// Note messages Capture MIDI's ring had no room for (WP-3.14).
    pub capture_dropped: u64,
    /// Messages handed to the queue to the MIDI output ports (WP-5.34).
    pub sent: u64,
    /// Messages for a port that a route's buffer or the queue had no room
    /// for, dropped.
    pub out_dropped: u64,
}

/// Which source a route takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Filter {
    /// Every source that reaches the engine: input ports whose Track switch
    /// is on, and the computer keyboard.
    All,
    /// The computer keyboard.
    Keyboard,
    /// One port.
    Port(PortId),
    /// Nothing.
    Nothing,
}

/// One MIDI track's input in a program.
#[derive(Debug)]
struct Route {
    track: TrackId,
    filter: Filter,
    /// The channel to take, 0 to 15, or every channel.
    channel: Option<u8>,
    monitored: bool,
    /// This block's messages, reserved when the program is compiled.
    events: Vec<MidiEvent>,
    /// This block's previews of the MIDI Note Editor (Spec: step-recording
    /// R2): played whether or not the track monitors, and kept apart from
    /// `events` so recording and Capture MIDI never see them.
    previews: Vec<MidiEvent>,
    /// The `(channel, key)` and note ID of every note the track's devices
    /// were sent an on for and no off, room made for all 2,048.
    held: Vec<(u8, u8, Option<u32>)>,
    /// Whether the held notes must end: the program replaced one whose
    /// route took something else or didn't monitor.
    release: bool,
    /// The key of the tuning each incoming key plays, for a track whose MIDI
    /// Controller Layout isn't All Keys and that plays the tuning (Spec:
    /// tunings R22).
    keymap: Option<Box<KeyMap>>,
}

impl Route {
    fn takes(&self, event: &MidiInputEvent) -> bool {
        // Spec: midi-io R9
        if event.message.is_system() {
            return false;
        }
        let source = match self.filter {
            // A preview is for one track alone (see `Route::previews`).
            _ if matches!(event.source, MidiSource::Preview(_)) => false,
            Filter::All => true,
            Filter::Keyboard => event.source == MidiSource::ComputerKeyboard,
            Filter::Port(port) => event.source.port() == Some(port),
            Filter::Nothing => false,
        };
        // Spec: midi-io R28. An MPE port's messages come on every channel.
        source
            && (matches!(event.source, MidiSource::MpePort(_))
                || self
                    .channel
                    .is_none_or(|channel| event.message.channel() == Some(channel)))
    }

    /// The event with its key mapped by the track's MIDI Controller Layout,
    /// or none if the layout plays nothing for it (Spec: tunings R22).
    /// Only notes and polyphonic key pressure carry a key.
    fn mapped(&self, mut event: MidiEvent) -> Option<MidiEvent> {
        let Some(map) = &self.keymap else {
            return Some(event);
        };
        let message = event.message;
        if matches!(message.status() & 0xF0, 0x80 | 0x90 | 0xA0) {
            let to = map[usize::from(message.data1() & 0x7F)]?;
            event.message = MidiMessage::from_bytes(&[message.status(), to, message.data2()])
                .unwrap_or(message);
        }
        if let MpeRole::Expression { key, .. } = &mut event.mpe {
            *key = map[usize::from(*key & 0x7F)]?;
        }
        Some(event)
    }

    /// Adds a message in order of offset, after any at the same offset.
    fn push(&mut self, event: MidiEvent) -> bool {
        if self.events.len() == self.events.capacity() {
            return false;
        }
        let at = self
            .events
            .iter()
            .rposition(|e| e.offset <= event.offset)
            .map_or(0, |i| i + 1);
        self.events.insert(at, event);
        true
    }
}

/// What a route to a port belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutOwner {
    /// A MIDI track's MIDI To.
    Track(TrackId),
    /// An external instrument, wherever it sits in a chain.
    Device(DeviceId),
}

/// A message on its way to a port, with the frame of the block it is heard
/// on, compensation included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Outgoing {
    offset: u32,
    message: MidiMessage,
}

/// One route to a MIDI output port in a program: the notes and messages of
/// its owner leave on the port's channel (`docs/specs/external-devices.md`,
/// R3). The task that owns the owner is the only one that reaches it in a
/// block, so each route keeps its messages itself.
#[derive(Debug)]
struct OutRoute {
    port: PortId,
    /// The channel everything is rewritten onto, 0 to 15.
    channel: u8,
    /// The frames the route's messages are sent after their own frame,
    /// which delay compensation holds the track's path into Main back by
    /// (R4). Zero for an external instrument, whose own latency is what
    /// compensation lines up (R10).
    compensation: u32,
    /// Which keys were sent a note-on and no note-off, one bit each.
    held: u128,
    /// MPE, when MIDI To is MPE: the notes and expressions leave as MPE MIDI
    /// laid out in its zone, in place of the rewrite onto `channel` (the
    /// MPE spec, R19). It holds the notes sent, so `held` stays empty.
    mpe: Option<MpeLayout>,
    /// Whether the held notes end in the next block (the transport stopped).
    release: bool,
    /// This block's messages, reserved when the program is compiled.
    outgoing: Vec<Outgoing>,
    /// Messages `outgoing` had no room for, since they were last counted.
    dropped: u64,
}

impl OutRoute {
    /// A route to the port called `name` on `channel` (1 to 16), or as MPE
    /// MIDI in the zone `mpe` names.
    fn new(name: &str, channel: u8, mpe: Option<MpeSettings>) -> Self {
        Self {
            port: PortId::from_name(name),
            channel: channel.saturating_sub(1).min(15),
            compensation: 0,
            held: 0,
            mpe: mpe.map(MpeLayout::new),
            release: false,
            outgoing: Vec::with_capacity(MIDI_OUT_PER_ROUTE),
            dropped: 0,
        }
    }
}

/// Held notes of a route that went or moved in a program swap, waiting to be
/// ended on the port and channel they were sent on (R6).
#[derive(Clone, Copy, Debug)]
struct Orphan {
    port: PortId,
    channel: u8,
    compensation: u32,
    held: u128,
    /// The key held on each channel of an MPE route, `NONE` for none.
    mpe: [u8; 16],
}

/// Marks a channel of an [`Orphan`] holding no key.
const NONE: u8 = u8::MAX;

/// The MIDI tracks' inputs in a program: what each takes, and what it took
/// in the last block; and the routes to output ports with what they send.
#[derive(Debug, Default)]
pub(crate) struct MidiRoutes {
    routes: Shared<Route>,
    /// Who each of `outs` belongs to, in the same order; fixed once the
    /// program is made, so a task finds its route without reading the
    /// routes others are writing.
    owners: Vec<OutOwner>,
    outs: Shared<OutRoute>,
    orphans: Vec<Orphan>,
}

/// The MIDI message of a note event on `channel` (0 to 15), if the kind has
/// one: notes, per-note pressure, and the channel messages of WP-5.44.
/// Per-note bend and slide are MPE's, which a hardware synthesiser takes on
/// channels of its own; they don't leave.
fn out_message(note: &NoteEvent, channel: u8) -> Option<MidiMessage> {
    let seven = |value: f32| (value.clamp(0.0, 1.0) * 127.0).round() as u8;
    let key = note.key & 0x7F;
    match note.kind {
        NoteKind::On => Some(MidiMessage::note_on(
            channel,
            key,
            seven(note.velocity).max(1),
        )),
        NoteKind::Off | NoteKind::Choke => {
            MidiMessage::from_bytes(&[0x80 | channel, key, seven(note.velocity)])
        }
        NoteKind::Expression {
            expression: NoteExpression::Pressure,
            value,
        } => MidiMessage::from_bytes(&[0xA0 | channel, key, seven(value)]),
        NoteKind::Expression { .. } => None,
        NoteKind::Control { .. }
        | NoteKind::PitchBend { .. }
        | NoteKind::ChannelPressure { .. } => NoteEvent { channel, ..*note }.message(),
    }
}

/// The note-off of every key in `held` on `channel`.
fn offs(channel: u8, mut held: u128) -> impl Iterator<Item = MidiMessage> {
    std::iter::from_fn(move || {
        (held != 0).then(|| {
            let key = held.trailing_zeros() as u8;
            held &= held - 1;
            MidiMessage::note_off(channel, key)
        })
    })
}

/// The note-off of every key in `keys`, one per channel: an MPE route's
/// held notes, `NONE` for a channel holding none.
fn mpe_offs(keys: [u8; 16]) -> impl Iterator<Item = MidiMessage> {
    keys.into_iter()
        .enumerate()
        .filter(|&(_, key)| key != NONE)
        .map(|(channel, key)| MidiMessage::note_off(channel as u8, key))
}

/// The key map of `track`'s MIDI Controller Layout under the set's tuning
/// (Spec: tunings R22), or none while the track doesn't play the tuning or
/// its layout is All Keys.
pub(crate) fn key_map(document: &Document, track: TrackId) -> Option<Box<KeyMap>> {
    let track = document.track(track)?;
    let tuning = document.settings().tuning.as_ref()?;
    if !track.plays_tuning() {
        return None;
    }
    KeyPitches::of(Some(tuning)).layout_map(track.tuning_layout())
}

impl MidiRoutes {
    /// The routes for `document`'s MIDI tracks, whose tasks are among
    /// `tasks`. Off the audio thread: it reserves every buffer.
    pub(crate) fn compile(document: &Document, tasks: &[Task]) -> Self {
        let routes = tasks
            .iter()
            .filter_map(|entry| {
                let track = document.track(entry.track)?;
                if track.kind() != TrackKind::Midi {
                    return None;
                }
                let routing = track.routing();
                // Spec: routing R2, R16
                let filter = match &routing.input {
                    Input::AllIns => Filter::All,
                    Input::ComputerKeyboard => Filter::Keyboard,
                    Input::Port { name } => Filter::Port(PortId::from_name(name)),
                    _ => Filter::Nothing,
                };
                Some(Route {
                    track: entry.track,
                    filter,
                    channel: routing.channel.map(|c| c.saturating_sub(1)),
                    monitored: routing.monitoring(track.mixer().arm),
                    events: Vec::with_capacity(MIDI_EVENTS_PER_TRACK),
                    previews: Vec::with_capacity(MIDI_PREVIEWS_PER_TRACK),
                    held: Vec::with_capacity(16 * 128),
                    release: false,
                    keymap: key_map(document, entry.track),
                })
            })
            .collect();
        let mut owners = Vec::new();
        let mut outs = Vec::new();
        for entry in tasks {
            let Some(track) = document.track(entry.track) else {
                continue;
            };
            // Spec: external-devices R2
            if track.kind() == TrackKind::Midi
                && let Some(MidiTo::Port { name, channel, mpe }) = &track.routing().midi_output
            {
                owners.push(OutOwner::Track(entry.track));
                outs.push(OutRoute::new(name, *channel, *mpe));
            }
            // Spec: external-devices R10
            for device in track.devices_deep() {
                if let Some(external) = &device.external
                    && let Some(port) = &external.port
                {
                    owners.push(OutOwner::Device(device.id));
                    outs.push(OutRoute::new(port, external.channel, external.mpe));
                }
            }
        }
        Self {
            routes: Shared::new(routes),
            owners,
            outs: Shared::new(outs),
            orphans: Vec::with_capacity(ORPHANS),
        }
    }

    /// Sets the frames the messages of `track`'s MIDI To are sent after
    /// their own frames: the delay compensation of its path into Main (R4).
    pub(crate) fn set_compensation(&mut self, track: TrackId, frames: u32) {
        if let Some(at) = self
            .owners
            .iter()
            .position(|&o| o == OutOwner::Track(track))
        {
            self.outs[at].compensation = frames;
        }
    }

    /// Queues what reached the end of `track`'s chain, or what `device`
    /// was given, for the port it names, if it names one: `notes` are at
    /// offsets counted from frame `from` of the block (R3).
    ///
    /// The tracks' tasks run at once on a pool of workers; a route belongs
    /// to its owner's task, which is the one thread sending to it in a
    /// block.
    pub(crate) fn send(&self, owner: OutOwner, notes: &[NoteEvent], from: u32) {
        let Some(at) = self.owners.iter().position(|&o| o == owner) else {
            return;
        };
        // SAFETY: a route belongs to its owner's task, which is the one
        // thread sending to it in this block.
        let route = unsafe { self.outs.element(at) };
        if let Some(layout) = route.mpe.as_mut() {
            // Spec: mpe R17, R19, R21. The notes leave laid out as MPE MIDI.
            let (outgoing, dropped) = (&mut route.outgoing, &mut route.dropped);
            let base = from + route.compensation;
            let mut sink = |offset: u32, message: MidiMessage| {
                if outgoing.len() == outgoing.capacity() {
                    *dropped += 1;
                } else {
                    outgoing.push(Outgoing {
                        offset: base + offset,
                        message,
                    });
                }
            };
            layout.open(&mut sink);
            layout.lay_out(notes, &mut sink);
            return;
        }
        for note in notes {
            let Some(message) = out_message(note, route.channel) else {
                continue;
            };
            if route.outgoing.len() == route.outgoing.capacity() {
                route.dropped += 1;
                continue;
            }
            match note.kind {
                NoteKind::On => route.held |= 1 << (note.key & 0x7F),
                NoteKind::Off | NoteKind::Choke => route.held &= !(1 << (note.key & 0x7F)),
                _ => {}
            }
            route.outgoing.push(Outgoing {
                offset: from + note.offset + route.compensation,
                message,
            });
        }
    }

    /// Ends the notes `owner` holds in the next block (R6): its device was
    /// switched off, so nothing it is sent reaches the port.
    pub(crate) fn release(&self, owner: OutOwner) {
        let Some(at) = self.owners.iter().position(|&o| o == owner) else {
            return;
        };
        // SAFETY: a route belongs to its owner's task, which is the one
        // thread sending to it in this block.
        let route = unsafe { self.outs.element(at) };
        route.release |= route.held != 0 || route.mpe.as_ref().is_some_and(MpeLayout::is_holding);
    }

    /// Ends every held note in the next block (R6): the transport stopped.
    pub(crate) fn release_all(&mut self) {
        for route in self.outs.iter_mut() {
            route.release =
                route.held != 0 || route.mpe.as_ref().is_some_and(MpeLayout::is_holding);
        }
    }

    /// Every note the routes have sent and not ended, as a note-off each,
    /// for a stream that stops (R6). Allocates: off the audio thread.
    pub(crate) fn drain_held(&mut self) -> Vec<(PortId, MidiMessage)> {
        let mut messages = Vec::new();
        for o in self.orphans.drain(..) {
            messages.extend(offs(o.channel, o.held).map(|m| (o.port, m)));
            messages.extend(mpe_offs(o.mpe).map(|m| (o.port, m)));
        }
        for route in self.outs.iter_mut() {
            let port = route.port;
            messages
                .extend(offs(route.channel, std::mem::take(&mut route.held)).map(|m| (port, m)));
            if let Some(layout) = route.mpe.as_mut() {
                layout.release_all(0, &mut |_, message| messages.push((port, message)));
            }
        }
        messages
    }

    /// What `track`'s input received in the last block, if it's a MIDI
    /// track in this program.
    pub(crate) fn input(&self, track: TrackId) -> Option<TrackMidiInput<'_>> {
        self.routes
            .iter()
            .find(|route| route.track == track)
            .map(|route| TrackMidiInput {
                events: &route.events,
                monitored: route.monitored,
            })
    }

    /// What each monitoring track's input received in the last block.
    pub(crate) fn monitored_inputs(&self) -> impl Iterator<Item = (TrackId, &[MidiEvent])> {
        self.routes
            .iter()
            .filter(|route| route.monitored)
            .map(|route| (route.track, route.events.as_slice()))
    }

    fn clear(&mut self) {
        for route in self.routes.iter_mut() {
            route.events.clear();
            route.previews.clear();
        }
    }

    /// Takes over the held notes of `old`'s routes, for a program that
    /// replaces it with the same devices. A route that no longer takes what
    /// it took, or no longer monitors, ends them in its first block. Real-time
    /// safe: it swaps buffers.
    pub(crate) fn inherit(&mut self, old: &mut Self) {
        for route in self.routes.iter_mut() {
            let Some(was) = old.routes.iter_mut().find(|r| r.track == route.track) else {
                continue;
            };
            std::mem::swap(&mut route.held, &mut was.held);
            route.release = !route.monitored
                || route.filter != was.filter
                || route.channel != was.channel
                || route.keymap != was.keymap;
        }
        // Spec: external-devices R6. A route that sends where it sent keeps
        // the notes it holds; one that was removed or sends elsewhere ends
        // them where they started.
        for (owner, route) in self.owners.iter().zip(self.outs.iter_mut()) {
            if let Some(at) = old.owners.iter().position(|o| o == owner) {
                let was = &mut old.outs[at];
                if was.port == route.port && was.channel == route.channel {
                    if route.mpe.is_none() && was.mpe.is_none() {
                        route.held = std::mem::take(&mut was.held);
                        route.release = was.release;
                    } else if route.mpe.as_ref().map(MpeLayout::settings)
                        == was.mpe.as_ref().map(MpeLayout::settings)
                    {
                        // Spec: mpe R19. A route that lays out as it did
                        // keeps its channel table and the notes on it.
                        std::mem::swap(&mut route.mpe, &mut was.mpe);
                        route.release = was.release;
                    }
                }
            }
        }
        for was in old.outs.iter_mut() {
            let mpe = was.mpe.as_ref().map_or([NONE; 16], MpeLayout::keys);
            if was.held != 0 || mpe != [NONE; 16] {
                let orphan = Orphan {
                    port: was.port,
                    channel: was.channel,
                    compensation: was.compensation,
                    held: std::mem::take(&mut was.held),
                    mpe,
                };
                self.orphan(orphan);
            }
        }
        for orphan in old.orphans.drain(..) {
            self.orphan(orphan);
        }
    }

    fn orphan(&mut self, orphan: Orphan) {
        // The orphans of a swap that comes too soon after another beyond the
        // room made are dropped; 256 routes ending notes at once is more than
        // a set has.
        if self.orphans.len() < self.orphans.capacity() {
            self.orphans.push(orphan);
        }
    }

    /// Puts what `track`'s input received in the frames `from..until` of
    /// the block into `notes`, merged with the notes already there, at
    /// offsets counted from `from`; ends the held notes first if the route
    /// changed. A track that doesn't monitor adds nothing.
    ///
    /// The tracks' tasks run at once on a pool of workers, each playing its
    /// own track's route, which no other task reaches.
    pub(crate) fn play(&self, track: TrackId, from: u32, until: u32, notes: &mut NoteBuffer) {
        let Some(at) = self.routes.iter().position(|r| r.track == track) else {
            return;
        };
        // SAFETY: a route belongs to its track's task, which is the one
        // thread playing it in this block.
        let route = unsafe { self.routes.element(at) };
        if route.release {
            // Spec: midi-io R26
            route.release = false;
            for (channel, key, id) in route.held.drain(..) {
                let mut off = note(0, NoteKind::Off, channel, key, 0.0);
                off.note_id = id;
                notes.insert(off);
            }
        }
        // Spec: step-recording R2. A preview plays on its own frames,
        // monitored or not; the note-off the shell sends ends it.
        for event in &route.previews {
            if event.offset < from || event.offset >= until {
                continue;
            }
            let (offset, message) = (event.offset - from, &event.message);
            let (channel, key) = (message.channel().unwrap_or(0), message.data1());
            let velocity = f32::from(message.data2()) / 127.0;
            if message.is_note_on() {
                notes.insert(note(offset, NoteKind::On, channel, key, velocity));
            } else if message.is_note_off() {
                notes.insert(note(offset, NoteKind::Off, channel, key, velocity));
            }
        }
        if !route.monitored {
            return;
        }
        // Spec: midi-io R12
        for event in &route.events {
            if event.offset < from || event.offset >= until {
                continue;
            }
            let offset = event.offset - from;
            let (channel, key, value) = (
                event.message.channel().unwrap_or(0),
                event.message.data1(),
                f32::from(event.message.data2()) / 127.0,
            );
            match event.mpe {
                // Spec: mpe R11. An MPE port's note has an ID of its own, and
                // its expressions name it.
                MpeRole::Note { id } => {
                    let id = Some(id);
                    if event.message.is_note_on() {
                        route.held.push((channel, key, id));
                        let mut on = note(offset, NoteKind::On, channel, key, value);
                        on.note_id = id;
                        notes.insert(on);
                    } else {
                        route.held.retain(|held| held.2 != id);
                        let mut off = note(offset, NoteKind::Off, channel, key, value);
                        off.note_id = id;
                        notes.insert(off);
                    }
                }
                MpeRole::Expression {
                    id,
                    key,
                    expression,
                    value,
                } => {
                    // Spec: mpe R6. A bend is a share of 48 semitones.
                    let (expression, scale) = match expression {
                        Expression::Pitch => (NoteExpression::Bend, 1.0 / 48.0),
                        Expression::Slide => (NoteExpression::Slide, 1.0 / 127.0),
                        Expression::Pressure => (NoteExpression::Pressure, 1.0 / 127.0),
                    };
                    let kind = NoteKind::Expression {
                        expression,
                        value: (value * scale).clamp(-1.0, 1.0),
                    };
                    let mut expressed = note(offset, kind, channel, key, 0.0);
                    expressed.note_id = Some(id);
                    notes.insert(expressed);
                }
                MpeRole::Plain => {
                    if event.message.is_note_on() {
                        if !route.held.iter().any(|h| (h.0, h.1) == (channel, key)) {
                            route.held.push((channel, key, None));
                        }
                        notes.insert(note(offset, NoteKind::On, channel, key, value));
                    } else if event.message.is_note_off() {
                        route.held.retain(|held| (held.0, held.1) != (channel, key));
                        notes.insert(note(offset, NoteKind::Off, channel, key, value));
                    } else if event.message.status() & 0xF0 == 0xA0 {
                        let pressure = NoteKind::Expression {
                            expression: NoteExpression::Pressure,
                            value,
                        };
                        notes.insert(note(offset, pressure, channel, key, 0.0));
                    } else if let Some(message) = NoteEvent::from_message(offset, &event.message) {
                        // Spec: device-framework R39
                        notes.insert(message);
                    }
                }
            }
        }
    }
}

fn note(offset: u32, kind: NoteKind, channel: u8, key: u8, velocity: f32) -> NoteEvent {
    NoteEvent {
        offset,
        kind,
        channel,
        key,
        velocity,
        note_id: None,
    }
}

/// When each block starts, estimated from the callback's times and the
/// engine's frame count by a second-order delay-locked loop (Adriaensen,
/// "Using a DLL to filter time", 2005): it follows the device's real rate,
/// and smooths out jitter in when the callback runs.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BlockClock {
    locked: bool,
    /// The filtered time of the block that started at `frame`, in
    /// nanoseconds after the clock's epoch.
    time: f64,
    frame: u64,
    /// The device's rate as the loop sees it.
    ns_per_frame: f64,
    nominal: f64,
}

impl BlockClock {
    pub(crate) fn new(rate: SampleRate) -> Self {
        let nominal = 1e9 / rate.as_f64();
        Self {
            locked: false,
            time: 0.0,
            frame: 0,
            ns_per_frame: nominal,
            nominal,
        }
    }

    /// The filtered start of the block that starts at engine frame `clock`,
    /// called at `now`. Returns it, and whether the clock locked again.
    pub(crate) fn advance(&mut self, now: f64, clock: u64) -> (f64, bool) {
        if !self.locked || clock < self.frame {
            return (self.lock(now, clock), false);
        }
        let frames = (clock - self.frame) as f64;
        if frames == 0.0 {
            return (self.time, false);
        }
        let predicted = self.time + frames * self.ns_per_frame;
        let error = now - predicted;
        if error.abs() > RELOCK_NS {
            return (self.lock(now, clock), true);
        }
        let omega = core::f64::consts::TAU * CLOCK_BANDWIDTH_HZ * frames * self.ns_per_frame / 1e9;
        let omega = omega.min(1.0);
        self.time = predicted + core::f64::consts::SQRT_2 * omega * error;
        self.ns_per_frame += omega * omega * error / frames;
        // A real device runs within a fraction of a percent of its rate.
        self.ns_per_frame = self
            .ns_per_frame
            .clamp(self.nominal * 0.99, self.nominal * 1.01);
        self.frame = clock;
        (self.time, false)
    }

    fn lock(&mut self, now: f64, clock: u64) -> f64 {
        self.locked = true;
        self.time = now;
        self.frame = clock;
        self.ns_per_frame = self.nominal;
        now
    }

    /// Frames per nanosecond, as the loop measures the device.
    pub(crate) fn frames_per_ns(&self) -> f64 {
        1.0 / self.ns_per_frame
    }
}

/// When the current block starts: what the block clock estimated.
#[derive(Clone, Copy, Debug)]
struct Timing {
    epoch: Instant,
    /// The block's start, in nanoseconds after `epoch`.
    start: f64,
    /// The engine clock's frame the block starts on.
    frame: u64,
    ns_per_frame: f64,
}

/// Where the engine clock stood on the monotonic clock when the last block
/// started, as the block clock estimated it (R16): what puts a moment on the
/// main thread, such as a click, onto a frame of the engine clock as MIDI
/// input is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimeBase {
    epoch: Instant,
    /// The block's start, in nanoseconds after `epoch`.
    start_ns: f64,
    /// The engine clock's frame the block starts on.
    frame: u64,
    ns_per_frame: f64,
}

impl TimeBase {
    /// The frame of the engine clock that `time` falls on: before the block
    /// it names, a frame before its start.
    #[must_use]
    pub fn frame_at(&self, time: Instant) -> u64 {
        let frames = (signed_ns(self.epoch, time) - self.start_ns) / self.ns_per_frame;
        (self.frame as f64 + frames).round().max(0.0) as u64
    }
}

impl Timing {
    /// The moment `ns` nanoseconds after the epoch.
    fn at(&self, ns: f64) -> Instant {
        if ns >= 0.0 {
            self.epoch + Duration::from_nanos(ns as u64)
        } else {
            self.epoch
                .checked_sub(Duration::from_nanos((-ns) as u64))
                .unwrap_or(self.epoch)
        }
    }
}

/// What became of a sync message handed to [`MidiIn::emit_sync`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stamped {
    /// There is no queue to the output ports (an offline render), or the
    /// block has no timing: nothing is sent (external-devices R8).
    Offline,
    /// The queue was full: dropped and counted.
    Dropped,
    /// On the queue; `past` when its moment is before the block's start.
    Queued { past: bool },
}

/// The audio thread's MIDI input: the queue's consuming end, the block
/// clock, and the counts; and the producing end of the queue to the MIDI
/// output thread.
pub(crate) struct MidiIn {
    queue: Option<Consumer<MidiInputEvent>>,
    /// The queue to the MIDI output thread (WP-5.34), if there is one.
    out: Option<Producer<MidiOutputEvent>>,
    /// Where the current block sits on the monotonic clock.
    timing: Option<Timing>,
    clock: BlockClock,
    /// What times are measured from: the first block's time.
    epoch: Option<Instant>,
    /// Reads the messages of ports whose MPE switch is on (WP-6.17).
    mpe: MpeReader,
    /// The previous block's filtered start, and when its callback ran.
    previous: Option<(f64, f64)>,
    /// The clock follower the input's sync messages go to (R14).
    pub(crate) sync_in: ClockIn,
    pub(crate) counters: MidiCounters,
}

impl MidiIn {
    pub(crate) fn new(rate: SampleRate) -> Self {
        Self {
            queue: None,
            out: None,
            timing: None,
            clock: BlockClock::new(rate),
            epoch: None,
            mpe: MpeReader::new(),
            previous: None,
            sync_in: ClockIn::new(),
            counters: MidiCounters::default(),
        }
    }

    /// Takes over a queue, letting go of the one before (off the audio
    /// thread: the caller sets it before the stream starts).
    pub(crate) fn set_queue(
        &mut self,
        queue: Consumer<MidiInputEvent>,
    ) -> Option<Consumer<MidiInputEvent>> {
        self.previous = None;
        self.queue.replace(queue)
    }

    /// Takes over the queue to the MIDI output thread, letting go of the one
    /// before (off the audio thread, as [`set_queue`](Self::set_queue)).
    pub(crate) fn set_out(
        &mut self,
        queue: Producer<MidiOutputEvent>,
    ) -> Option<Producer<MidiOutputEvent>> {
        self.out.replace(queue)
    }

    /// Takes the messages due in the block that starts at `now`, at engine
    /// frame `clock`, `frames` long, and hands each to the routes it
    /// matches. Real-time safe.
    pub(crate) fn take(
        &mut self,
        now: Instant,
        clock: u64,
        frames: usize,
        mut routes: Option<&mut MidiRoutes>,
    ) {
        if let Some(routes) = routes.as_deref_mut() {
            routes.clear();
        }
        if frames == 0 {
            return;
        }
        let epoch = *self.epoch.get_or_insert(now);
        let called = signed_ns(epoch, now);
        let (start, relocked) = self.clock.advance(called, clock);
        if relocked {
            self.counters.relocks += 1;
            self.previous = None;
        }
        let per_ns = self.clock.frames_per_ns();
        // Spec: external-devices R4. The same estimate places the messages
        // that leave for ports.
        self.timing = Some(Timing {
            epoch,
            start,
            frame: clock,
            ns_per_frame: 1.0 / per_ns,
        });
        let Some(queue) = self.queue.as_mut() else {
            return;
        };
        // Spec: midi-io R15. The window is the previous block's span; the
        // first block takes what arrived in the block's length before it.
        let (from, previous_call) = self.previous.unwrap_or_else(|| {
            let from = start - frames as f64 / per_ns;
            (from, from)
        });
        self.previous = Some((start, called));
        let last = frames as u32 - 1;
        for _ in 0..MIDI_EVENTS_PER_BLOCK {
            let Ok(head) = queue.peek() else {
                break;
            };
            let time = signed_ns(epoch, head.time);
            if time >= start {
                // It arrived after this block's start: next block.
                break;
            }
            let Ok(event) = queue.pop() else {
                break;
            };
            self.counters.received += 1;
            // Spec: midi-clock R14. Sync messages go to the clock follower,
            // never to a track.
            if let MidiSource::Sync(port) = event.source {
                self.sync_in.push(port, time, &event.message);
                continue;
            }
            let ahead = (time - from) * per_ns;
            let offset = if ahead < 0.0 {
                // Stamped before the previous block's start. If the
                // previous callback ran after the stamp, the message should
                // have been there to take: it's late. If not, the block clock
                // was a little ahead of the callback, and the message is off
                // by less than that.
                if time < previous_call {
                    self.counters.late += 1;
                }
                0
            } else {
                (ahead.round() as u32).min(last)
            };
            // Spec: mpe R10, R11. An MPE port's message is the notes and
            // expressions it plays, which may be none.
            let roles: &[MpeRole] = match event.source {
                MidiSource::MpePort(port) if !event.message.is_system() => {
                    self.mpe.read(port, &event.message)
                }
                _ => &[MpeRole::Plain],
            };
            let mut matched = roles.is_empty();
            if let Some(routes) = routes.as_deref_mut() {
                if let MidiSource::Preview(track) = event.source {
                    for route in routes.routes.iter_mut().filter(|r| r.track.0 == track) {
                        matched = true;
                        if route.previews.len() < route.previews.capacity() {
                            route.previews.push(MidiEvent {
                                offset,
                                message: event.message,
                                source: event.source,
                                mpe: MpeRole::Plain,
                            });
                            self.counters.delivered += 1;
                        } else {
                            self.counters.dropped += 1;
                        }
                    }
                }
                for route in routes.routes.iter_mut() {
                    if !route.takes(&event) {
                        continue;
                    }
                    matched = true;
                    for &mpe in roles {
                        let placed = MidiEvent {
                            offset,
                            message: event.message,
                            source: event.source,
                            mpe,
                        };
                        let Some(placed) = route.mapped(placed) else {
                            continue;
                        };
                        if route.push(placed) {
                            self.counters.delivered += 1;
                        } else {
                            self.counters.dropped += 1;
                        }
                    }
                }
            }
            if !matched {
                self.counters.unrouted += 1;
            }
        }
    }
}

impl MidiIn {
    /// Lets the clock follower read this block's sync messages and answer
    /// for it (R17 to R24). Real-time safe.
    pub(crate) fn follow_clock(
        &mut self,
        ask: &crate::follower::Ask,
        output_latency: u32,
        ext: bool,
        looped: Option<(f64, f64)>,
        answer: &mut crate::follower::Follow,
    ) {
        let timing = self.timing.map(|t| ClockTiming {
            start: t.start,
            ns_per_frame: t.ns_per_frame,
            latency: output_latency,
        });
        self.sync_in.block(ask, timing, ext, looped, answer);
    }

    /// Where the last block started on the engine clock and on the
    /// monotonic clock, for [`TimeBase::frame_at`]; `None` before the first
    /// block. Real-time safe.
    pub(crate) fn time_base(&self) -> Option<TimeBase> {
        self.timing.map(|t| TimeBase {
            epoch: t.epoch,
            start_ns: t.start,
            frame: t.frame,
            ns_per_frame: t.ns_per_frame,
        })
    }

    /// Hands the messages the program made for ports in this part of the
    /// block to the MIDI output thread, each stamped with the moment its
    /// frame is heard at the audio interface's output: the block's start,
    /// plus its frame, plus `output_latency` (R4, R5). A full queue drops
    /// the message and counts it. Without a queue (an offline render) they
    /// go nowhere (R8). Real-time safe.
    pub(crate) fn emit(&mut self, routes: &mut MidiRoutes, output_latency: u32) {
        let Self {
            out,
            timing,
            counters,
            ..
        } = self;
        let mut push = |port: PortId, offset: u32, message: MidiMessage| {
            let (Some(out), Some(timing)) = (out.as_mut(), *timing) else {
                return;
            };
            let frames = f64::from(offset) + f64::from(output_latency);
            let time = timing.at(timing.start + frames * timing.ns_per_frame);
            let event = MidiOutputEvent {
                port,
                time,
                message,
            };
            if out.push(event).is_ok() {
                counters.sent += 1;
            } else {
                counters.out_dropped += 1;
            }
        };
        // Spec: external-devices R6. The notes of routes that went or moved
        // in a swap, and of routes whose transport stopped, end first.
        for orphan in routes.orphans.drain(..) {
            for message in offs(orphan.channel, orphan.held).chain(mpe_offs(orphan.mpe)) {
                push(orphan.port, orphan.compensation, message);
            }
        }
        let mut dropped = 0;
        for route in routes.outs.iter_mut() {
            if std::mem::take(&mut route.release) {
                for message in offs(route.channel, std::mem::take(&mut route.held)) {
                    push(route.port, route.compensation, message);
                }
                if let Some(layout) = route.mpe.as_mut() {
                    let (port, compensation) = (route.port, route.compensation);
                    layout.release_all(0, &mut |offset, message| {
                        push(port, compensation + offset, message);
                    });
                }
            }
            for event in route.outgoing.drain(..) {
                push(route.port, event.offset, event.message);
            }
            dropped += std::mem::take(&mut route.dropped);
        }
        self.counters.out_dropped += dropped;
    }

    /// Hands one sync message for `port` to the MIDI output thread, stamped
    /// for the moment the frame `frames` after the block's start (the output
    /// latency included) is heard, plus the port's sync delay of `delay_ms`
    /// (midi-clock R4). Real-time safe.
    pub(crate) fn emit_sync(
        &mut self,
        port: PortId,
        frames: i64,
        delay_ms: i16,
        message: MidiMessage,
    ) -> Stamped {
        let (Some(out), Some(timing)) = (self.out.as_mut(), self.timing) else {
            return Stamped::Offline;
        };
        let ns = timing.start + frames as f64 * timing.ns_per_frame + f64::from(delay_ms) * 1e6;
        let event = MidiOutputEvent {
            port,
            time: timing.at(ns),
            message,
        };
        if out.push(event).is_ok() {
            self.counters.sent += 1;
            Stamped::Queued {
                // A nanosecond of float rounding isn't late.
                past: ns < timing.start - 1.0,
            }
        } else {
            self.counters.out_dropped += 1;
            Stamped::Dropped
        }
    }

    /// Sends `message` to `port` at once. Allocates nothing, but is for
    /// off the audio thread, as [`release`](Self::release) is.
    pub(crate) fn send_now(&mut self, port: PortId, message: MidiMessage) {
        if let Some(out) = self.out.as_mut() {
            let _ = out.push(MidiOutputEvent {
                port,
                time: Instant::now(),
                message,
            });
        }
    }

    /// Sends the note-offs for everything `routes` still holds, at once:
    /// a stream that stops or restarts leaves no note hanging on the
    /// hardware (R6). Allocates: off the audio thread.
    pub(crate) fn release(&mut self, routes: &mut MidiRoutes) {
        let held = routes.drain_held();
        let Some(out) = self.out.as_mut() else {
            return;
        };
        let time = Instant::now();
        for (port, message) in held {
            let _ = out.push(MidiOutputEvent {
                port,
                time,
                message,
            });
        }
    }
}

/// Nanoseconds from `epoch` to `time`, negative if `time` is earlier.
fn signed_ns(epoch: Instant, time: Instant) -> f64 {
    match time.checked_duration_since(epoch) {
        Some(after) => after.as_nanos() as f64,
        None => -(epoch.duration_since(time).as_nanos() as f64),
    }
}

impl std::fmt::Debug for MidiIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MidiIn")
            .field("connected", &self.queue.is_some())
            .field("counters", &self.counters)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny random sequence for jitter: deterministic, no dependency.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
        }
    }

    // Spec: midi-io R16
    #[test]
    fn the_block_clock_smooths_callback_jitter_and_follows_the_device() {
        // A device 0.05% fast, its callbacks up to 1 ms late or early.
        let rate = SampleRate::HZ_48000;
        let mut clock = BlockClock::new(rate);
        let true_ns_per_frame = 1e9 / 48_000.0 / 1.0005;
        let mut jitter = Lcg(7);
        let block = 256u64;
        let (mut worst_raw, mut worst_filtered): (f64, f64) = (0.0, 0.0);
        let (mut squares_raw, mut squares_filtered, mut count) = (0.0, 0.0, 0.0);
        for n in 0..3_000u64 {
            let frame = n * block;
            let truth = 5e8 + frame as f64 * true_ns_per_frame;
            let raw = truth + jitter.next() * 1e6;
            let (filtered, relocked) = clock.advance(raw, frame);
            assert!(!relocked);
            // After a few seconds to lock (n = 1,000 is 5.3 s).
            if n > 1_000 {
                let (raw_error, filtered_error) = (raw - truth, filtered - truth);
                worst_raw = worst_raw.max(raw_error.abs());
                worst_filtered = worst_filtered.max(filtered_error.abs());
                squares_raw += raw_error * raw_error;
                squares_filtered += filtered_error * filtered_error;
                count += 1.0;
            }
        }
        let (rms_raw, rms_filtered) = (
            (squares_raw / count).sqrt(),
            (squares_filtered / count).sqrt(),
        );
        assert!(worst_raw > 9e5, "{worst_raw}");
        assert!(
            worst_filtered < 2.5e5,
            "the filtered start is within 0.25 ms of the truth: {worst_filtered} ns"
        );
        assert!(
            rms_filtered < rms_raw / 5.0,
            "the loop removes most of the jitter: {rms_filtered} ns RMS from {rms_raw}"
        );
        let rate_error = (clock.ns_per_frame / true_ns_per_frame - 1.0).abs();
        assert!(rate_error < 1e-4, "{rate_error}");

        // A gap makes it lock again.
        let (_, relocked) = clock.advance(1e12, 3_000 * block);
        assert!(relocked);
    }
}

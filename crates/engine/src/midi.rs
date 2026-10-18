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
//! names a port, and an external instrument names one too: what reaches
//! the end of the track's chain, or the device, is rewritten onto the
//! channel and handed, stamped with the moment its frame is heard at the
//! audio interface's output, to a lock-free queue whose other end the MIDI
//! output thread reads (`docs/specs/external-devices.md`, R3 to R6, R10).

use std::time::{Duration, Instant};

use bl_core::midi::{MidiInputEvent, MidiMessage, MidiOutputEvent, MidiSource, PortId};
use bl_core::time::SampleRate;
use bl_model::{DeviceId, Document, Input, MidiTo, TrackId, TrackKind};
use rtrb::{Consumer, Producer};

use crate::node::{NoteBuffer, NoteEvent, NoteExpression, NoteKind};
use crate::program::Task;

/// The most messages a block takes from the queue. More wait for the next
/// block, so the work per block is bounded (real-time rule 4).
pub const MIDI_EVENTS_PER_BLOCK: usize = 1024;

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

/// The most messages one block sends to ports. More are dropped and counted
/// ([`MidiCounters::out_dropped`]).
pub const MIDI_OUT_PER_BLOCK: usize = 4096;

/// The most held notes a program swap can end at once, for routes the new
/// program no longer has or sends elsewhere.
const ORPHANS: usize = 256;

/// A MIDI message at a frame of a block, on a track's input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiEvent {
    /// The frame in the block where it lands. Always less than the block's
    /// length.
    pub offset: u32,
    /// The message.
    pub message: MidiMessage,
    /// Where it came from.
    pub source: MidiSource,
}

/// What a MIDI track's input received in the last block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    /// Messages for a port that the block's buffer or the queue had no room
    /// for, or that the queue couldn't take, dropped.
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
    /// The `(channel, key)` of every note the track's devices were sent an
    /// on for and no off, room made for all 2,048.
    held: Vec<(u8, u8)>,
    /// Whether the held notes must end: the program replaced one whose
    /// route took something else or didn't monitor.
    release: bool,
}

impl OutRoute {
    /// A route to the port called `name` on `channel` (1 to 16).
    fn new(owner: OutOwner, name: &str, channel: u8) -> Self {
        Self {
            owner,
            port: PortId::from_name(name),
            channel: channel.saturating_sub(1).min(15),
            compensation: 0,
            held: 0,
            release: false,
        }
    }
}

impl Route {
    fn takes(&self, event: &MidiInputEvent) -> bool {
        // Spec: midi-io R9
        if event.message.is_system() {
            return false;
        }
        let source = match self.filter {
            Filter::All => true,
            Filter::Keyboard => event.source == MidiSource::ComputerKeyboard,
            Filter::Port(port) => event.source == MidiSource::Port(port),
            Filter::Nothing => false,
        };
        source
            && self
                .channel
                .is_none_or(|channel| event.message.channel() == Some(channel))
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

/// One route to a MIDI output port in a program: the notes and messages of
/// its owner leave on the port's channel (`docs/specs/external-devices.md`,
/// R3).
#[derive(Debug)]
struct OutRoute {
    owner: OutOwner,
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
    /// Whether the held notes end in the next block (the transport stopped).
    release: bool,
}

/// Held notes of a route that went or moved in a program swap, waiting to be
/// ended on the port and channel they were sent on (R6).
#[derive(Clone, Copy, Debug)]
struct Orphan {
    port: PortId,
    channel: u8,
    compensation: u32,
    held: u128,
}

/// A message on its way to a port, with the frame of the block it is heard
/// on, compensation included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Outgoing {
    pub(crate) port: PortId,
    pub(crate) offset: u32,
    pub(crate) message: MidiMessage,
}

/// The MIDI tracks' inputs in a program: what each takes, and what it took
/// in the last block; and the routes to output ports with what they send.
#[derive(Debug, Default)]
pub(crate) struct MidiRoutes {
    routes: Vec<Route>,
    outs: Vec<OutRoute>,
    /// This block's messages for ports, reserved when the program is
    /// compiled.
    outgoing: Vec<Outgoing>,
    orphans: Vec<Orphan>,
    /// Messages for ports that `outgoing` had no room for, since the last
    /// time they were counted.
    out_dropped: u64,
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

/// Puts a note-off for every key in `held` on `outgoing`, or counts it
/// dropped.
fn push_offs(
    outgoing: &mut Vec<Outgoing>,
    dropped: &mut u64,
    (port, channel, offset): (PortId, u8, u32),
    mut held: u128,
) {
    while held != 0 {
        let key = held.trailing_zeros() as u8;
        held &= held - 1;
        if outgoing.len() < outgoing.capacity() {
            outgoing.push(Outgoing {
                port,
                offset,
                message: MidiMessage::note_off(channel, key),
            });
        } else {
            *dropped += 1;
        }
    }
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
                    held: Vec::with_capacity(16 * 128),
                    release: false,
                })
            })
            .collect();
        let mut outs = Vec::new();
        for entry in tasks {
            let Some(track) = document.track(entry.track) else {
                continue;
            };
            // Spec: external-devices R2
            if track.kind() == TrackKind::Midi
                && let Some(MidiTo::Port { name, channel }) = &track.routing().midi_output
            {
                outs.push(OutRoute::new(OutOwner::Track(entry.track), name, *channel));
            }
            // Spec: external-devices R10
            for device in track.devices_deep() {
                if let Some(external) = &device.external
                    && let Some(port) = &external.port
                {
                    outs.push(OutRoute::new(
                        OutOwner::Device(device.id),
                        port,
                        external.channel,
                    ));
                }
            }
        }
        // Room even with no route of its own: the note-offs of routes this
        // program removes leave through it.
        let outgoing = Vec::with_capacity(MIDI_OUT_PER_BLOCK);
        let orphans = Vec::with_capacity(ORPHANS);
        Self {
            routes,
            outs,
            outgoing,
            orphans,
            out_dropped: 0,
        }
    }

    /// Sets the frames the messages of `track`'s MIDI To are sent after
    /// their own frames: the delay compensation of its path into Main (R4).
    pub(crate) fn set_compensation(&mut self, track: TrackId, frames: u32) {
        if let Some(route) = self
            .outs
            .iter_mut()
            .find(|r| r.owner == OutOwner::Track(track))
        {
            route.compensation = frames;
        }
    }

    /// Queues what reached the end of `track`'s chain, or what `device`
    /// was given, for the port it names, if it names one: `notes` are at
    /// offsets counted from frame `from` of the block (R3).
    pub(crate) fn send(&mut self, owner: OutOwner, notes: &[NoteEvent], from: u32) {
        let Some(route) = self.outs.iter_mut().find(|r| r.owner == owner) else {
            return;
        };
        for note in notes {
            let Some(message) = out_message(note, route.channel) else {
                continue;
            };
            if self.outgoing.len() == self.outgoing.capacity() {
                self.out_dropped += 1;
                continue;
            }
            match note.kind {
                NoteKind::On => route.held |= 1 << (note.key & 0x7F),
                NoteKind::Off | NoteKind::Choke => route.held &= !(1 << (note.key & 0x7F)),
                _ => {}
            }
            self.outgoing.push(Outgoing {
                port: route.port,
                offset: from + note.offset + route.compensation,
                message,
            });
        }
    }

    /// Ends every held note in the next block (R6): the transport stopped.
    pub(crate) fn release_all(&mut self) {
        for route in &mut self.outs {
            route.release = route.held != 0;
        }
    }

    /// Queues a note-off for every note a route sent that has no end:
    /// those of routes the swap that made this program removed or moved, and
    /// those a stop ended (R6).
    fn flush_releases(&mut self) {
        for orphan in self.orphans.drain(..) {
            push_offs(
                &mut self.outgoing,
                &mut self.out_dropped,
                (orphan.port, orphan.channel, orphan.compensation),
                orphan.held,
            );
        }
        for route in &mut self.outs {
            if std::mem::take(&mut route.release) {
                let held = std::mem::take(&mut route.held);
                push_offs(
                    &mut self.outgoing,
                    &mut self.out_dropped,
                    (route.port, route.channel, route.compensation),
                    held,
                );
            }
        }
    }

    /// Every note the routes have sent and not ended, as a note-off each,
    /// for a stream that stops (R6). Allocates: off the audio thread.
    pub(crate) fn drain_held(&mut self) -> Vec<Outgoing> {
        let mut held: Vec<(PortId, u8, u128)> = self
            .orphans
            .drain(..)
            .map(|o| (o.port, o.channel, o.held))
            .collect();
        held.extend(
            self.outs
                .iter_mut()
                .map(|r| (r.port, r.channel, std::mem::take(&mut r.held))),
        );
        let total = held
            .iter()
            .map(|&(.., keys)| keys.count_ones() as usize)
            .sum();
        let mut offs = Vec::with_capacity(total);
        for (port, channel, keys) in held {
            push_offs(&mut offs, &mut 0, (port, channel, 0), keys);
        }
        offs
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
        for route in &mut self.routes {
            route.events.clear();
        }
    }

    /// Takes over the held notes of `old`'s routes, for a program that
    /// replaces it with the same devices. A route that no longer takes what
    /// it took, or no longer monitors, ends them in its first block. Real-time
    /// safe: it swaps buffers.
    pub(crate) fn inherit(&mut self, old: &mut Self) {
        for route in &mut self.routes {
            let Some(was) = old.routes.iter_mut().find(|r| r.track == route.track) else {
                continue;
            };
            std::mem::swap(&mut route.held, &mut was.held);
            route.release =
                !route.monitored || route.filter != was.filter || route.channel != was.channel;
        }
        // Spec: external-devices R6. A route that sends where it sent keeps
        // the notes it holds; one that was removed or sends elsewhere ends
        // them where they started.
        for route in &mut self.outs {
            if let Some(was) = old.outs.iter_mut().find(|r| r.owner == route.owner)
                && was.port == route.port
                && was.channel == route.channel
            {
                route.held = std::mem::take(&mut was.held);
                route.release = was.release;
            }
        }
        for was in &mut old.outs {
            if was.held != 0 {
                let orphan = Orphan {
                    port: was.port,
                    channel: was.channel,
                    compensation: was.compensation,
                    held: std::mem::take(&mut was.held),
                };
                self.orphan(orphan);
            }
        }
        for orphan in old.orphans.drain(..) {
            self.orphan(orphan);
        }
    }

    fn orphan(&mut self, orphan: Orphan) {
        if self.orphans.len() < self.orphans.capacity() {
            self.orphans.push(orphan);
        } else {
            self.out_dropped += 1;
        }
    }

    /// Puts what `track`'s input received in the frames `from..until` of
    /// the block into `notes`, merged with the notes already there, at
    /// offsets counted from `from`; ends the held notes first if the route
    /// changed. A track that doesn't monitor adds nothing.
    pub(crate) fn play(&mut self, track: TrackId, from: u32, until: u32, notes: &mut NoteBuffer) {
        let Some(route) = self.routes.iter_mut().find(|r| r.track == track) else {
            return;
        };
        if route.release {
            // Spec: midi-io R26
            route.release = false;
            for (channel, key) in route.held.drain(..) {
                notes.insert(note(0, NoteKind::Off, channel, key, 0.0));
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
            if event.message.is_note_on() {
                if !route.held.contains(&(channel, key)) {
                    route.held.push((channel, key));
                }
                notes.insert(note(offset, NoteKind::On, channel, key, value));
            } else if event.message.is_note_off() {
                route.held.retain(|&held| held != (channel, key));
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
    ns_per_frame: f64,
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
    /// The previous block's filtered start, and when its callback ran.
    previous: Option<(f64, f64)>,
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
            previous: None,
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
        if frames == 0 || (self.queue.is_none() && self.out.is_none()) {
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
            let placed = MidiEvent {
                offset,
                message: event.message,
                source: event.source,
            };
            let mut matched = false;
            if let Some(routes) = routes.as_deref_mut() {
                for route in &mut routes.routes {
                    if route.takes(&event) {
                        matched = true;
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
    /// Hands the messages the program made for ports in this part of the
    /// block to the MIDI output thread, each stamped with the moment its
    /// frame is heard at the audio interface's output: the block's start,
    /// plus its frame, plus `output_latency` (R4, R5). A full queue drops
    /// the message and counts it. Without a queue (an offline render) they
    /// go nowhere (R8). Real-time safe.
    pub(crate) fn emit(&mut self, routes: &mut MidiRoutes, output_latency: u32) {
        routes.flush_releases();
        self.counters.out_dropped += std::mem::take(&mut routes.out_dropped);
        let (Some(out), Some(timing)) = (self.out.as_mut(), self.timing) else {
            routes.outgoing.clear();
            return;
        };
        for event in routes.outgoing.drain(..) {
            let frames = f64::from(event.offset) + f64::from(output_latency);
            let ns = timing.start + frames * timing.ns_per_frame;
            let time = if ns >= 0.0 {
                timing.epoch + Duration::from_nanos(ns as u64)
            } else {
                timing
                    .epoch
                    .checked_sub(Duration::from_nanos((-ns) as u64))
                    .unwrap_or(timing.epoch)
            };
            let pushed = out.push(MidiOutputEvent {
                port: event.port,
                time,
                message: event.message,
            });
            if pushed.is_ok() {
                self.counters.sent += 1;
            } else {
                self.counters.out_dropped += 1;
            }
        }
    }

    /// Sends the note-offs for everything `routes` still holds, at once:
    /// a stream that stops or restarts leaves no note hanging on the
    /// hardware (R6). Allocates: off the audio thread.
    pub(crate) fn release(&mut self, routes: &mut MidiRoutes) {
        let offs = routes.drain_held();
        let Some(out) = self.out.as_mut() else {
            return;
        };
        let time = Instant::now();
        for event in offs {
            let _ = out.push(MidiOutputEvent {
                port: event.port,
                time,
                message: event.message,
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

//! MIDI output: the queue from the audio thread, the thread that hands its
//! messages to the MIDI system, and the table of open output ports it reads
//! (`docs/specs/external-devices.md`, R5).

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bl_core::midi::{MidiOutputEvent, PortId};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::backend::OutputConnection;

/// How often the output thread looks at its queue when it's empty.
const POLL: Duration = Duration::from_millis(1);

/// What MIDI output has done since the ports were made, counted on the MIDI
/// output thread.
#[derive(Debug, Default)]
pub struct OutputStats {
    /// Messages handed to a port: on their stamp's moment where the port's
    /// backend schedules, or at once.
    pub sent: AtomicU64,
    /// Messages whose moment had gone by when the thread took them, sent at
    /// once.
    pub late: AtomicU64,
    /// Messages for a port that isn't open, or whose Track switch is off.
    pub unrouted: AtomicU64,
    /// Messages a port refused.
    pub refused: AtomicU64,
}

/// [`OutputStats`] read at one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OutputSnapshot {
    /// See [`OutputStats::sent`].
    pub sent: u64,
    /// See [`OutputStats::late`].
    pub late: u64,
    /// See [`OutputStats::unrouted`].
    pub unrouted: u64,
    /// See [`OutputStats::refused`].
    pub refused: u64,
}

impl OutputStats {
    /// The counts now.
    #[must_use]
    pub fn snapshot(&self) -> OutputSnapshot {
        OutputSnapshot {
            sent: self.sent.load(Relaxed),
            late: self.late.load(Relaxed),
            unrouted: self.unrouted.load(Relaxed),
            refused: self.refused.load(Relaxed),
        }
    }
}

/// An open output port in the table the output thread reads.
struct Slot {
    /// The set of ports that opened it ([`OutputBus::owner`]), so a set of
    /// ports that closes takes only its own.
    owner: u64,
    port: PortId,
    /// The port's Track switch: whether tracks' MIDI reaches it.
    track: bool,
    connection: Box<dyn OutputConnection>,
}

/// The MIDI output side that outlives the ports: the queue from the engine,
/// the thread that hands its messages to the open output ports, and the
/// counts. Cloning it shares it.
///
/// An engine takes its end of the queue with [`attach`](Self::attach); the
/// ports ([`MidiPorts::with_output`](crate::MidiPorts::with_output)) put
/// their open outputs in the table the thread reads, so the MIDI system can
/// change underneath without the engine's side noticing. The thread ends
/// when the last clone goes.
#[derive(Clone)]
pub struct OutputBus {
    shared: Arc<Shared>,
}

impl Default for OutputBus {
    fn default() -> Self {
        Self::new()
    }
}

/// What the main thread and the output thread share.
struct Shared {
    owners: AtomicU64,
    table: Mutex<Vec<Slot>>,
    queue: Mutex<Option<Consumer<MidiOutputEvent>>>,
    stats: Arc<OutputStats>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl OutputBus {
    /// A bus with no ports and no engine.
    #[must_use]
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                owners: AtomicU64::new(0),
                table: Mutex::new(Vec::new()),
                queue: Mutex::new(None),
                stats: Arc::new(OutputStats::default()),
                thread: Mutex::new(None),
            }),
        }
    }

    /// The counts for everything the output thread has handled.
    #[must_use]
    pub fn stats(&self) -> &Arc<OutputStats> {
        &self.shared.stats
    }

    /// Starts a fresh queue of `capacity` messages and returns its
    /// producing end, for the next engine's processor
    /// (`bl_engine::Processor::set_midi_output`), starting the output thread
    /// if it isn't running. What the queue before held, the note-offs of an
    /// engine that stopped included, goes out first.
    #[must_use]
    pub fn attach(&self, capacity: usize) -> Producer<MidiOutputEvent> {
        self.shared.drain();
        let (producer, consumer) = RingBuffer::new(capacity);
        *lock(&self.shared.queue) = Some(consumer);
        let mut thread = lock(&self.shared.thread);
        if thread.is_none() {
            let shared = Arc::downgrade(&self.shared);
            *thread = thread::Builder::new()
                .name("bl-midi-out".to_owned())
                .spawn(move || run(&shared))
                .ok();
        }
        producer
    }

    /// A number for a set of ports that opens outputs on this bus.
    pub(crate) fn owner(&self) -> u64 {
        self.shared.owners.fetch_add(1, Relaxed)
    }

    /// Puts an open port in the table, or replaces the one there.
    pub(crate) fn insert(
        &self,
        owner: u64,
        port: PortId,
        track: bool,
        connection: Box<dyn OutputConnection>,
    ) {
        let mut table = lock(&self.shared.table);
        table.retain(|slot| slot.port != port);
        table.push(Slot {
            owner,
            port,
            track,
            connection,
        });
    }

    pub(crate) fn remove(&self, owner: u64, port: PortId) {
        lock(&self.shared.table).retain(|slot| !(slot.owner == owner && slot.port == port));
    }

    pub(crate) fn set_track(&self, owner: u64, port: PortId, track: bool) {
        if let Some(slot) = lock(&self.shared.table)
            .iter_mut()
            .find(|s| s.owner == owner && s.port == port)
        {
            slot.track = track;
        }
    }

    /// Closes every port `owner` opened.
    pub(crate) fn clear(&self, owner: u64) {
        lock(&self.shared.table).retain(|slot| slot.owner != owner);
    }
}

impl Shared {
    fn drain(&self) -> usize {
        let mut queue = lock(&self.queue);
        let Some(queue) = queue.as_mut() else {
            return 0;
        };
        let mut taken = 0;
        while let Ok(event) = queue.pop() {
            taken += 1;
            self.dispatch(event);
        }
        taken
    }

    fn dispatch(&self, event: MidiOutputEvent) {
        let mut table = lock(&self.table);
        // Spec: external-devices R6. A note off still leaves on a port whose
        // Track switch went off, so the notes sent before it don't hang.
        let Some(slot) = table
            .iter_mut()
            .find(|s| s.port == event.port && (s.track || event.message.is_note_off()))
        else {
            self.stats.unrouted.fetch_add(1, Relaxed);
            return;
        };
        // Spec: external-devices R5. A moment already past is sent at once
        // and counted.
        let sent = if event.time <= Instant::now() {
            self.stats.late.fetch_add(1, Relaxed);
            slot.connection.send(event.message.bytes())
        } else {
            slot.connection.send_at(event.message.bytes(), event.time)
        };
        match sent {
            Ok(()) => self.stats.sent.fetch_add(1, Relaxed),
            Err(_) => self.stats.refused.fetch_add(1, Relaxed),
        };
    }
}

/// The output thread: it looks at the queue every millisecond while it is
/// empty, and ends when the bus is gone.
fn run(shared: &Weak<Shared>) {
    loop {
        let Some(shared) = shared.upgrade() else {
            return;
        };
        if shared.drain() == 0 {
            drop(shared);
            thread::park_timeout(POLL);
        }
    }
}

//! MIDI output: the queue from the audio thread, the thread that hands its
//! messages to the MIDI system, and the table of open output ports it reads
//! (`docs/specs/external-devices.md`, R5).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
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
    port: PortId,
    /// The port's Track switch: whether tracks' MIDI reaches it.
    track: bool,
    connection: Box<dyn OutputConnection>,
}

/// What the main thread and the output thread share.
pub(crate) struct Output {
    table: Mutex<Vec<Slot>>,
    queue: Mutex<Option<Consumer<MidiOutputEvent>>>,
    stats: Arc<OutputStats>,
    stop: AtomicBool,
    thread: Mutex<Option<JoinHandle<()>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Output {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            table: Mutex::new(Vec::new()),
            queue: Mutex::new(None),
            stats: Arc::new(OutputStats::default()),
            stop: AtomicBool::new(false),
            thread: Mutex::new(None),
        })
    }

    pub(crate) fn stats(&self) -> &Arc<OutputStats> {
        &self.stats
    }

    /// Puts an open port in the table, or replaces the one there.
    pub(crate) fn insert(&self, port: PortId, track: bool, connection: Box<dyn OutputConnection>) {
        let mut table = lock(&self.table);
        table.retain(|slot| slot.port != port);
        table.push(Slot {
            port,
            track,
            connection,
        });
    }

    pub(crate) fn remove(&self, port: PortId) {
        lock(&self.table).retain(|slot| slot.port != port);
    }

    pub(crate) fn set_track(&self, port: PortId, track: bool) {
        if let Some(slot) = lock(&self.table).iter_mut().find(|s| s.port == port) {
            slot.track = track;
        }
    }

    /// Starts a fresh queue of `capacity` messages and returns its
    /// producing end, for the next engine's processor, starting the output
    /// thread if it isn't running. What the old queue held goes with it.
    pub(crate) fn attach(self: &Arc<Self>, capacity: usize) -> Producer<MidiOutputEvent> {
        let (producer, consumer) = RingBuffer::new(capacity);
        *lock(&self.queue) = Some(consumer);
        let mut thread = lock(&self.thread);
        if thread.is_none() {
            let output = Arc::clone(self);
            *thread = thread::Builder::new()
                .name("bl-midi-out".to_owned())
                .spawn(move || output.run())
                .ok();
        }
        producer
    }

    /// Hands every queued message to its port; returns how many it took.
    pub(crate) fn drain(&self) -> usize {
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
        let Some(slot) = table.iter_mut().find(|s| s.port == event.port && s.track) else {
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

    fn run(&self) {
        while !self.stop.load(Relaxed) {
            if self.drain() == 0 {
                thread::park_timeout(POLL);
            }
        }
        self.drain();
    }

    /// Stops the thread, after it sends what is queued.
    pub(crate) fn close(&self) {
        self.stop.store(true, Relaxed);
        if let Some(thread) = lock(&self.thread).take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

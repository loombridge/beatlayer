//! The queue from the MIDI threads to the audio thread.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, PoisonError};

use bl_core::midi::{MidiInputEvent, MidiMessage};
use rtrb::{Consumer, Producer, RingBuffer};

/// Makes the queue that carries MIDI input to the engine: the sending end,
/// for every input's handler and the computer keyboard, and the consuming
/// end, for the engine's processor
/// (`bl_engine::Processor::set_midi_input`). It holds `capacity` messages.
///
/// ```
/// use std::time::Instant;
/// use bl_core::midi::{MidiInputEvent, MidiMessage, MidiSource};
///
/// let (sender, mut engine_end) = bl_midi_io::input_queue(16);
/// let event = MidiInputEvent {
///     source: MidiSource::ComputerKeyboard,
///     time: Instant::now(),
///     message: MidiMessage::note_on(0, 60, 100),
/// };
/// assert!(sender.send(event));
/// assert_eq!(engine_end.pop(), Ok(event));
/// ```
#[must_use]
pub fn input_queue(capacity: usize) -> (InputSender, Consumer<MidiInputEvent>) {
    let (producer, consumer) = RingBuffer::new(capacity);
    let sender = InputSender {
        producer: Arc::new(Mutex::new(producer)),
        stats: Arc::new(MidiStats::default()),
        tap: Arc::new(Tap::default()),
    };
    (sender, consumer)
}

/// The sending end of the MIDI input queue, shared by every MIDI-side thread.
///
/// The queue is single-producer, so the producing end sits behind a mutex
/// that only senders take: each port's handler and the computer keyboard.
/// The consuming end on the audio thread is lock-free and never waits on it
/// (`docs/specs/midi-io.md`, R14).
#[derive(Clone)]
pub struct InputSender {
    producer: Arc<Mutex<Producer<MidiInputEvent>>>,
    stats: Arc<MidiStats>,
    tap: Arc<Tap>,
}

/// How many note messages the tap holds between two reads.
const TAP_ROOM: usize = 256;

/// A copy of the note messages for the main thread, made while someone asks
/// for it: Chord's Learn listens to what is played (the chord spec).
struct Tap {
    on: AtomicBool,
    notes: Mutex<Vec<MidiMessage>>,
}

impl Default for Tap {
    fn default() -> Self {
        Self {
            on: AtomicBool::new(false),
            notes: Mutex::new(Vec::with_capacity(TAP_ROOM)),
        }
    }
}

impl InputSender {
    /// Queues a message for the engine. Returns `false`, counting it as
    /// dropped, if the queue is full.
    pub fn send(&self, event: MidiInputEvent) -> bool {
        if self.tap.on.load(Relaxed) && (event.message.is_note_on() || event.message.is_note_off())
        {
            let mut notes = self
                .tap
                .notes
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if notes.len() < TAP_ROOM {
                notes.push(event.message);
            }
        }
        let pushed = self
            .producer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event)
            .is_ok();
        if pushed {
            self.stats.to_tracks.fetch_add(1, Relaxed);
        } else {
            self.stats.dropped.fetch_add(1, Relaxed);
        }
        pushed
    }

    /// Starts or stops copying note messages for the main thread to read
    /// with [`take_notes`](Self::take_notes). Stopping forgets what was
    /// copied.
    pub fn set_tap(&self, on: bool) {
        self.tap.on.store(on, Relaxed);
        if !on {
            self.tap
                .notes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clear();
        }
    }

    /// Moves the note messages copied since the last call into `into`, in
    /// the order they came.
    pub fn take_notes(&self, into: &mut Vec<MidiMessage>) {
        into.append(
            &mut self
                .tap
                .notes
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
    }

    /// Starts a fresh queue of `capacity` messages and returns its consuming
    /// end, for the next engine's processor. What the old queue held, and
    /// anything sent before the engine takes this end, goes with the old
    /// processor: a new engine starts with an empty queue.
    #[must_use]
    pub fn attach(&self, capacity: usize) -> Consumer<MidiInputEvent> {
        let (producer, consumer) = RingBuffer::new(capacity);
        *self.producer.lock().unwrap_or_else(PoisonError::into_inner) = producer;
        consumer
    }

    /// The counts for everything that came through.
    #[must_use]
    pub fn stats(&self) -> &Arc<MidiStats> {
        &self.stats
    }
}

impl std::fmt::Debug for InputSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputSender")
            .field("stats", &self.stats.snapshot())
            .finish_non_exhaustive()
    }
}

/// What MIDI input has done since the queue was made, counted on the MIDI
/// threads.
#[derive(Debug, Default)]
pub struct MidiStats {
    /// Messages queued for the engine: those that reach tracks' inputs.
    pub to_tracks: AtomicU64,
    /// Messages dropped because the queue was full.
    pub dropped: AtomicU64,
    /// Messages from inputs with Remote on, for MIDI mapping (Phase 7).
    pub to_remote: AtomicU64,
    /// Clock and transport messages from inputs with Sync on (Phase 7).
    pub to_sync: AtomicU64,
    /// Messages nothing takes: system exclusive, or from an input whose
    /// switches don't take them.
    pub ignored: AtomicU64,
}

/// [`MidiStats`] read at one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatsSnapshot {
    /// See [`MidiStats::to_tracks`].
    pub to_tracks: u64,
    /// See [`MidiStats::dropped`].
    pub dropped: u64,
    /// See [`MidiStats::to_remote`].
    pub to_remote: u64,
    /// See [`MidiStats::to_sync`].
    pub to_sync: u64,
    /// See [`MidiStats::ignored`].
    pub ignored: u64,
}

impl MidiStats {
    /// The counts now.
    #[must_use]
    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            to_tracks: self.to_tracks.load(Relaxed),
            dropped: self.dropped.load(Relaxed),
            to_remote: self.to_remote.load(Relaxed),
            to_sync: self.to_sync.load(Relaxed),
            ignored: self.ignored.load(Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bl_core::midi::{MidiMessage, MidiSource};
    use std::time::Instant;

    #[test]
    fn attaching_again_sends_to_the_new_end_only() {
        let (sender, mut first) = input_queue(4);
        let mut second = sender.attach(4);
        let event = MidiInputEvent {
            source: MidiSource::ComputerKeyboard,
            time: Instant::now(),
            message: MidiMessage::note_on(0, 60, 100),
        };
        assert!(sender.send(event));
        assert!(first.pop().is_err());
        assert_eq!(second.pop(), Ok(event));
    }
}

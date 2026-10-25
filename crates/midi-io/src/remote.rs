//! The remote input's share of the MIDI system's thread: the table of
//! mapped messages and the queue of messages to the main thread
//! (`docs/specs/remote-mapping.md`, R7 to R10).
//!
//! The main thread publishes a [`RemoteBits`] whenever the mappings change.
//! Each input's handler asks [`RemoteInput::takes`] whether a mapping takes
//! a message before it sends it towards the tracks, and sends the messages
//! that are taken, or all of them while a map mode is on, to the main
//! thread. Nothing here is on the audio thread.

use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Acquire, Ordering::Relaxed, Ordering::Release};

use bl_core::midi::{MidiInputEvent, MidiMessage};
use rtrb::{Consumer, Producer, RingBuffer};

/// How many messages wait for the main thread.
const QUEUE: usize = 1024;

/// Words a channel takes in the table: 128 notes, 128 controllers, and pitch
/// bend.
const PER_CHANNEL: usize = 5;
const WORDS: usize = 16 * PER_CHANNEL;

/// The messages the mappings take: one bit for each channel and each note,
/// controller and pitch bend (R9). Channels count from 0 to 15.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteBits([u64; WORDS]);

impl Default for RemoteBits {
    fn default() -> Self {
        Self([0; WORDS])
    }
}

impl RemoteBits {
    fn set(&mut self, channel: u8, offset: usize, bit: u8) {
        let word = usize::from(channel & 15) * PER_CHANNEL + offset + usize::from(bit / 64);
        self.0[word] |= 1 << (bit % 64);
    }

    /// Takes the note `key`.
    pub fn note(&mut self, channel: u8, key: u8) {
        self.set(channel, 0, key & 127);
    }

    /// Takes the controller `controller`.
    pub fn controller(&mut self, channel: u8, controller: u8) {
        self.set(channel, 2, controller & 127);
    }

    /// Takes pitch bend.
    pub fn pitch_bend(&mut self, channel: u8) {
        self.set(channel, 4, 0);
    }
}

/// The remote input as every input's handler shares it.
pub struct RemoteInput {
    table: [AtomicU64; WORDS],
    learning: AtomicBool,
    producer: Mutex<Producer<MidiInputEvent>>,
    consumer: Mutex<Consumer<MidiInputEvent>>,
}

impl Default for RemoteInput {
    fn default() -> Self {
        let (producer, consumer) = RingBuffer::new(QUEUE);
        Self {
            table: std::array::from_fn(|_| AtomicU64::new(0)),
            learning: AtomicBool::new(false),
            producer: Mutex::new(producer),
            consumer: Mutex::new(consumer),
        }
    }
}

impl RemoteInput {
    /// Publishes the messages the mappings take: every message stamped from
    /// now on is judged by them (R9).
    pub fn publish(&self, bits: &RemoteBits) {
        for (word, bits) in self.table.iter().zip(&bits.0) {
            word.store(*bits, Release);
        }
    }

    /// Whether a mapping takes `message`: a channel message's note,
    /// controller or pitch bend. It allocates nothing and takes no lock.
    #[must_use]
    pub fn takes(&self, message: &MidiMessage) -> bool {
        let Some(channel) = message.channel() else {
            return false;
        };
        let (offset, bit) = match message.status() & 0xF0 {
            0x80 | 0x90 => (0, message.data1()),
            0xB0 => (2, message.data1()),
            0xE0 => (4, 0),
            _ => return false,
        };
        let word = usize::from(channel) * PER_CHANNEL + offset + usize::from(bit / 64);
        self.table[word].load(Acquire) & (1 << (bit % 64)) != 0
    }

    /// Whether a map mode is on, so every message also crosses to the main
    /// thread for learning (R10).
    #[must_use]
    pub fn learning(&self) -> bool {
        self.learning.load(Relaxed)
    }

    /// Turns the learning crossing on or off: a map mode sets it.
    pub fn set_learning(&self, on: bool) {
        self.learning.store(on, Relaxed);
    }

    /// Queues `event` for the main thread; `false` if the queue is full and
    /// the event was dropped.
    pub(crate) fn push(&self, event: MidiInputEvent) -> bool {
        self.producer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event)
            .is_ok()
    }

    /// Moves the queued events into `into`, in the order they arrived.
    pub fn drain(&self, into: &mut Vec<MidiInputEvent>) {
        let mut consumer = self.consumer.lock().unwrap_or_else(PoisonError::into_inner);
        into.extend(std::iter::from_fn(|| consumer.pop().ok()));
    }
}

impl std::fmt::Debug for RemoteInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteInput")
            .field("learning", &self.learning())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: remote-mapping R8, R9
    #[test]
    fn the_table_takes_the_messages_published_and_no_others() {
        let remote = RemoteInput::default();
        let mut bits = RemoteBits::default();
        bits.note(0, 60);
        bits.note(15, 127);
        bits.controller(1, 7);
        bits.pitch_bend(2);
        remote.publish(&bits);
        let takes = |bytes: &[u8]| remote.takes(&MidiMessage::from_bytes(bytes).unwrap());
        // A note's note-on and note-off.
        assert!(takes(&[0x90, 60, 100]) && takes(&[0x80, 60, 64]) && takes(&[0x90, 60, 0]));
        assert!(takes(&[0x9F, 127, 1]));
        assert!(!takes(&[0x90, 61, 100]), "another key");
        assert!(!takes(&[0x91, 60, 100]), "another channel");
        assert!(takes(&[0xB1, 7, 3]) && !takes(&[0xB1, 8, 3]) && !takes(&[0xB0, 7, 3]));
        assert!(takes(&[0xE2, 0, 64]) && takes(&[0xE2, 1, 0]) && !takes(&[0xE3, 0, 64]));
        // Pressure, program changes and the like are never mapped.
        assert!(!takes(&[0xA0, 60, 1]) && !takes(&[0xC0, 3]) && !takes(&[0xD0, 3]));
        assert!(!takes(&[0xF8]));

        remote.publish(&RemoteBits::default());
        assert!(!takes(&[0x90, 60, 100]), "a new table replaces the old");
    }
}

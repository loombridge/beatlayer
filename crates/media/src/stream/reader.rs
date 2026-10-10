//! The audio thread's end of a disk stream.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::Ordering::{Relaxed, Release};
use std::time::{Duration, Instant};

use rtrb::{Consumer, Producer, PushError};

use super::planar::{PlanarMut, copy_planar};
use super::player::{Got, Player, Supply};
use super::shared::{Block, Cue, Shared, StreamStats, ToDisk, ToReader};
use super::source::{AudioSource, Filled, Transition};

/// How long a blocking read waits for the disk before it gives up and plays
/// silence.
const BLOCKING_TIMEOUT: Duration = Duration::from_secs(10);

/// Identifies a stream to its [`Streamer`](super::Streamer), for
/// [`Streamer::cue`](super::Streamer::cue).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StreamId(pub(crate) u64);

impl fmt::Display for StreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "stream {}", self.0)
    }
}

/// A file streamed from disk: the audio thread's end of a stream that a
/// [`Streamer`](super::Streamer)'s disk thread fills ahead of it.
///
/// It reads from lock-free queues and never waits: audio the disk thread
/// hasn't delivered plays as silence, counted as an underrun ([`Filled`],
/// [`StreamStats`]). The start of the clip is decoded before the source
/// exists, so a launch or a locate to it never waits; so is any point
/// [cued](super::Streamer::cue) since.
///
/// Dropping it frees its buffers, so drop it off the audio thread (the
/// engine drops nodes on its drop thread); the disk thread notices and
/// closes its end.
pub struct StreamSource {
    player: Player<Streamed>,
    rate: u32,
}

impl StreamSource {
    pub(crate) fn new(streamed: Streamed, rate: u32, position: u64, fade: usize) -> Self {
        let channels = streamed.channels;
        Self {
            player: Player::new(streamed, channels, position, fade),
            rate,
        }
    }

    /// The stream's ID, for cueing it.
    #[must_use]
    pub fn id(&self) -> StreamId {
        StreamId(self.player.supply.shared.id)
    }

    /// The length of a jump's fades, in frames.
    #[must_use]
    pub fn fade_frames(&self) -> usize {
        self.player.fade_frames()
    }

    /// Whether a fade from the last jump (or from silence) is still running.
    #[must_use]
    pub fn fading(&self) -> bool {
        self.player.fading()
    }

    /// Makes reads wait for the disk thread when their audio isn't ready,
    /// rather than play silence, so an offline render never underruns and
    /// renders the same every time. A blocking read sleeps: never set this
    /// on a source an audio thread reads.
    pub fn set_blocking(&mut self, blocking: bool) {
        self.player.supply.blocking = blocking;
    }

    /// Where the audio in cue slot `slot` starts, once it has arrived.
    #[must_use]
    pub fn cued(&mut self, slot: usize) -> Option<u64> {
        self.player.supply.poll();
        self.player
            .supply
            .cues
            .get(slot)
            .and_then(Option::as_ref)
            .map(|cue| cue.start)
    }

    /// Waits until `frames` frames from the current position are ready, or
    /// the end of the source is, for up to `timeout`. Returns whether they
    /// are. A stream holds only so much ahead of where it's read, so it waits
    /// for at most all but one of its blocks' worth. It sleeps, so it's for
    /// set-up and tests, not an audio thread.
    pub fn wait_until_ready(&mut self, frames: u64, timeout: Duration) -> bool {
        let began = Instant::now();
        let supply = &mut self.player.supply;
        let holds = (supply.ring.capacity().saturating_sub(1) * supply.block_frames).max(1);
        let frames = frames.min(holds as u64);
        supply.activate();
        loop {
            let position = self.player.position();
            let ready = self.buffered();
            let at_end = self
                .player
                .supply
                .length
                .is_some_and(|length| position + ready >= length);
            if ready >= frames || at_end {
                return true;
            }
            if began.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(Duration::from_micros(200));
        }
    }
}

impl AudioSource for StreamSource {
    fn channels(&self) -> usize {
        self.player.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn frames(&self) -> Option<u64> {
        self.player.supply.length
    }

    fn position(&self) -> u64 {
        self.player.position()
    }

    fn read(&mut self, position: u64, out: &mut PlanarMut<'_>, transition: Transition) -> Filled {
        self.player.read(position, out, transition)
    }

    fn seek(&mut self, position: u64) {
        self.player.seek(position);
    }

    fn stop(&mut self) {
        self.player.stop();
    }

    fn buffered(&mut self) -> u64 {
        self.player.supply.poll();
        let start = self.player.position();
        self.player.supply.ready_from(start) - start
    }

    fn stats(&self) -> StreamStats {
        self.player.supply.shared.counters.snapshot()
    }
}

impl fmt::Debug for StreamSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamSource")
            .field("id", &self.id())
            .field("rate", &self.rate)
            .field("channels", &self.player.channels())
            .field("position", &self.player.position())
            .field("epoch", &self.player.supply.epoch)
            .field("buffered_blocks", &self.player.supply.ring.len)
            .finish_non_exhaustive()
    }
}

/// The blocks of the current sequence the reader holds, in order, in a ring
/// of fixed capacity: as many as the stream has.
pub(crate) struct Ring {
    slots: Box<[Option<Block>]>,
    head: usize,
    len: usize,
}

impl Ring {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            slots: (0..capacity.max(1)).map(|_| None).collect(),
            head: 0,
            len: 0,
        }
    }

    pub(crate) fn capacity(&self) -> usize {
        self.slots.len()
    }

    fn get(&self, i: usize) -> Option<&Block> {
        if i >= self.len {
            return None;
        }
        self.slots[(self.head + i) % self.slots.len()].as_ref()
    }

    fn front(&self) -> Option<&Block> {
        self.get(0)
    }

    fn push_back(&mut self, block: Block) -> Result<(), Block> {
        if self.len == self.slots.len() {
            return Err(block);
        }
        let i = (self.head + self.len) % self.slots.len();
        self.slots[i] = Some(block);
        self.len += 1;
        Ok(())
    }

    fn pop_front(&mut self) -> Option<Block> {
        if self.len == 0 {
            return None;
        }
        let block = self.slots[self.head].take();
        self.head = (self.head + 1) % self.slots.len();
        self.len -= 1;
        block
    }
}

/// A stream's supply on the audio thread: the blocks and cues the disk
/// thread has sent.
pub(crate) struct Streamed {
    pub(crate) shared: Arc<Shared>,
    pub(crate) from_disk: Consumer<ToReader>,
    pub(crate) to_disk: Producer<ToDisk>,
    pub(crate) channels: usize,
    pub(crate) block_frames: usize,
    pub(crate) cue_frames: usize,
    /// The length at the engine's rate, once known.
    pub(crate) length: Option<u64>,
    pub(crate) ring: Ring,
    /// Cue slots; slot 0 holds the clip's start.
    pub(crate) cues: Box<[Option<Cue>]>,
    /// The block sequence wanted from the disk thread.
    pub(crate) epoch: u64,
    /// Where the next block of the sequence starts.
    pub(crate) expect: u64,
    pub(crate) armed: bool,
    pub(crate) blocking: bool,
}

impl Streamed {
    /// Hands a buffer back to the disk thread, to reuse or free there.
    fn give_back(&mut self, message: ToDisk) {
        if let Err(PushError::Full(message)) = self.to_disk.push(message) {
            // Can't happen: the queue has room for every buffer the stream
            // has. Leaking one is still better than freeing it here.
            std::mem::forget(message);
        }
    }

    fn receive(&mut self, message: ToReader) {
        match message {
            ToReader::Block(block) => {
                if block.epoch != self.epoch || block.start != self.expect {
                    // A block of a sequence this reader has left.
                    return self.give_back(ToDisk::Block(block.samples));
                }
                if block.ends_at.is_some() {
                    self.length = block.ends_at;
                }
                self.expect = block.end_frame();
                if block.len == 0 {
                    return self.give_back(ToDisk::Block(block.samples));
                }
                if let Err(block) = self.ring.push_back(block) {
                    self.give_back(ToDisk::Block(block.samples));
                }
            }
            ToReader::Cue(cue) => {
                if cue.ends_at.is_some() {
                    self.length = cue.ends_at;
                }
                let Some(slot) = self.cues.get_mut(cue.slot) else {
                    return self.give_back(ToDisk::Cue(cue.samples));
                };
                if let Some(old) = slot.replace(cue) {
                    self.give_back(ToDisk::Cue(old.samples));
                }
            }
        }
    }

    /// Hands back the blocks that end at or before `at`.
    fn discard_before(&mut self, at: u64) {
        while self.ring.front().is_some_and(|b| b.end_frame() <= at) {
            if let Some(block) = self.ring.pop_front() {
                self.give_back(ToDisk::Block(block.samples));
            }
        }
    }

    /// Asks the disk thread for a new sequence of blocks from `target`, and
    /// hands back the blocks of the old one.
    fn restart(&mut self, target: u64) {
        while let Some(block) = self.ring.pop_front() {
            self.give_back(ToDisk::Block(block.samples));
        }
        self.epoch += 1;
        self.expect = target;
        self.shared.target.store(target, Relaxed);
        self.shared.epoch.store(self.epoch, Release);
    }

    fn cue_covering(&self, at: u64) -> Option<&Cue> {
        self.cues.iter().flatten().find(|cue| cue.covers(at))
    }

    fn past_end(&self, at: u64) -> bool {
        self.length.is_some_and(|length| at >= length)
    }

    /// Whether audio at `at` is here.
    fn covers(&self, at: u64) -> bool {
        (0..self.ring.len).any(|i| {
            self.ring
                .get(i)
                .is_some_and(|b| b.start <= at && at < b.end_frame())
        }) || self.cue_covering(at).is_some()
    }

    /// The first frame from `at` on that isn't here: how far audio is ready.
    fn ready_from(&self, mut at: u64) -> u64 {
        loop {
            if self.past_end(at) {
                return at;
            }
            let block = (0..self.ring.len)
                .filter_map(|i| self.ring.get(i))
                .find(|b| b.start <= at && at < b.end_frame())
                .map(Block::end_frame);
            match block.or_else(|| self.cue_covering(at).map(Cue::end_frame)) {
                Some(end) => at = end,
                None => return at,
            }
        }
    }

    /// For a blocking read: waits until audio at `at` arrives, the end is
    /// known to be before it, or the wait times out.
    fn wait_for(&mut self, at: u64) {
        let began = Instant::now();
        while !self.covers(at) && !self.past_end(at) && began.elapsed() < BLOCKING_TIMEOUT {
            std::thread::sleep(Duration::from_micros(50));
            self.poll();
        }
    }
}

impl Supply for Streamed {
    fn poll(&mut self) {
        // Bounded by the queue's capacity, whatever the disk thread sends.
        for _ in 0..self.from_disk.buffer().capacity() {
            let Ok(message) = self.from_disk.pop() else {
                break;
            };
            self.receive(message);
        }
    }

    fn activate(&mut self) {
        if !self.armed {
            self.armed = true;
            self.shared.armed.store(true, Release);
        }
    }

    fn fetch(
        &mut self,
        at: u64,
        out: &mut PlanarMut<'_>,
        offset: usize,
        frames: usize,
        channels: usize,
        consume: bool,
    ) -> Got {
        if consume {
            self.discard_before(at);
        }
        let mut next_block = None;
        for i in 0..self.ring.len {
            let Some(block) = self.ring.get(i) else {
                break;
            };
            if block.end_frame() <= at {
                continue;
            }
            if block.start > at {
                next_block = Some(block.start);
                break;
            }
            let from = (at - block.start) as usize;
            let n = frames.min(block.len - from);
            copy_planar(
                &block.samples,
                self.block_frames,
                from,
                out,
                offset,
                n,
                channels,
            );
            return Got::Data(n);
        }
        if let Some(cue) = self.cue_covering(at) {
            let from = (at - cue.start) as usize;
            let n = frames.min(cue.len - from);
            copy_planar(
                &cue.samples,
                self.cue_frames,
                from,
                out,
                offset,
                n,
                channels,
            );
            return Got::Data(n);
        }
        if self.past_end(at) {
            return Got::End(frames);
        }
        if consume && self.blocking {
            self.wait_for(at);
            if self.covers(at) || self.past_end(at) {
                return self.fetch(at, out, offset, frames, channels, consume);
            }
        }
        let mut missing = frames as u64;
        if let Some(start) = next_block {
            missing = missing.min(start - at);
        }
        if let Some(length) = self.length {
            missing = missing.min(length.saturating_sub(at));
        }
        Got::Missing(missing.max(1) as usize)
    }

    fn relocate(&mut self, to: u64) -> bool {
        let ring_start = self.ring.front().map_or(self.expect, |b| b.start);
        // Already here in this sequence's blocks: skip forward to it.
        if (ring_start..self.expect).contains(&to) {
            self.discard_before(to);
            return true;
        }
        // Past the end, there's nothing to read.
        if self.past_end(to) {
            return true;
        }
        // Preloaded or cued: play from the cue while the disk thread reads
        // on from its end, unless this sequence already follows it.
        if let Some(cue_end) = self.cue_covering(to).map(Cue::end_frame) {
            if ring_start != cue_end {
                self.restart(cue_end);
            }
            return true;
        }
        // Just ahead of what's here, so arriving next: wait for it.
        if to >= ring_start && to < self.expect + self.block_frames as u64 {
            self.discard_before(to);
            return false;
        }
        self.restart(to);
        false
    }

    fn fell_behind(&mut self, at: u64) {
        // More than a block behind the playhead, the disk thread would only
        // deliver audio that's already been skipped: move it on to a block
        // ahead of where playing will be by the time it can deliver.
        if !self.past_end(at)
            && at > self.expect + self.block_frames as u64
            && self.cue_covering(at).is_none()
        {
            self.restart(at + self.block_frames as u64);
        }
    }

    fn count(&mut self, delta: &StreamStats) {
        self.shared.count(delta);
    }
}

impl fmt::Debug for Streamed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Streamed")
            .field("id", &self.shared.id)
            .field("epoch", &self.epoch)
            .field("expect", &self.expect)
            .finish_non_exhaustive()
    }
}

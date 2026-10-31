//! Link Audio in the engine (the Link Audio spec, R4 to R6, R11): the audio
//! a peer's channel delivers, placed on the frames it plays on and read by
//! the audio thread without allocating, locking or blocking, and the tracks
//! the engine offers back.
//!
//! The engine builds no Link: whoever runs it on a device implements
//! [`LinkAudio`] over `bl-link`'s sinks and sources, and tests stand in for it
//! ([`testing::link_audio`](crate::testing)). A [`Channel`] is the one
//! structure both sides share: the thread Link delivers buffers on
//! [`push`](Channel::push)es them, the audio thread [`read`](Channel::read)s
//! the block it is playing.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use bl_model::TrackId;

/// The frame a time falls on at `rate`: the engine's way of putting what
/// arrives on Link's clock (microseconds) on a frame of its own. Integer
/// arithmetic, since a clock this large has no spare precision in an `f64`
/// at audio rates.
#[must_use]
pub fn frame_at(micros: i64, rate: u32) -> i64 {
    let scaled = i128::from(micros) * i128::from(rate);
    i64::try_from((scaled + 500_000).div_euclid(1_000_000)).unwrap_or(i64::MAX)
}

/// No frame read yet.
const UNSET: u64 = u64::MAX;

/// A read cursor this far from where the block says it should be, in
/// thousandths of a second, is a jump rather than jitter: it resynchronises.
const RESYNC_MS: i64 = 1;

/// A peer's channel as the audio thread plays it: a ring of stereo frames
/// indexed by the frame each plays on, the frame count of Link's clock at the
/// engine's rate, so a late buffer is one whose frames the reader already
/// passed.
pub struct Channel {
    /// Left and right, interleaved: the bits of an `f32` each. Only the
    /// writer writes a cell ahead of the reader, and the reader never writes.
    cells: Box<[AtomicU32]>,
    /// The ring's length in frames, a power of two.
    capacity: usize,
    rate: u32,
    /// The next frame the audio thread reads, or [`UNSET`].
    cursor: AtomicU64,
    /// One past the last frame written.
    end: AtomicU64,
    /// Frames that came too late, or too early for the ring, and were not
    /// kept.
    dropped: AtomicU64,
    /// Frames that were kept, ever.
    received: AtomicU64,
}

impl Channel {
    /// A channel at `rate` holding a second of audio.
    #[must_use]
    pub fn new(rate: u32) -> Self {
        let capacity = (rate.max(1) as usize).next_power_of_two();
        Self {
            cells: (0..capacity * 2).map(|_| AtomicU32::new(0)).collect(),
            capacity,
            rate,
            cursor: AtomicU64::new(UNSET),
            end: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            received: AtomicU64::new(0),
        }
    }

    /// The frame rate it holds.
    #[must_use]
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Puts `left` and `right`, which play from `start` on (a [`frame_at`]
    /// of the moment plus the Latency), where they play. Frames before the
    /// one the audio thread has reached are late: they are dropped and
    /// counted. Called from one thread, Link's, which may allocate: it does
    /// not here either.
    pub fn push(&self, start: i64, left: &[f32], right: &[f32]) {
        let len = left.len().min(right.len());
        let mut at = start.max(0) as u64;
        let mut from = 0;
        let cursor = self.cursor.load(Ordering::Acquire);
        let mut late = 0;
        if cursor != UNSET && at < cursor {
            late = ((cursor - at) as usize).min(len);
            from = late;
            at += late as u64;
        }
        let mut keep = len - from;
        // Too far ahead of what is read for the ring to hold.
        let room = if cursor == UNSET {
            self.capacity
        } else {
            (cursor + self.capacity as u64).saturating_sub(at) as usize
        };
        let early = keep.saturating_sub(room);
        keep -= early;
        self.dropped
            .fetch_add((late + early) as u64, Ordering::Relaxed);
        if keep == 0 {
            return;
        }
        let end = self.end.load(Ordering::Relaxed);
        // A gap is silence, not what the ring held a lap ago.
        if at > end {
            for frame in end.max(at.saturating_sub(self.capacity as u64))..at {
                self.store(frame, 0.0, 0.0);
            }
        }
        for i in 0..keep {
            self.store(at + i as u64, left[from + i], right[from + i]);
        }
        self.received.fetch_add(keep as u64, Ordering::Relaxed);
        self.end.store(end.max(at + keep as u64), Ordering::Release);
    }

    fn store(&self, frame: u64, left: f32, right: f32) {
        let cell = (frame as usize & (self.capacity - 1)) * 2;
        self.cells[cell].store(left.to_bits(), Ordering::Relaxed);
        self.cells[cell + 1].store(right.to_bits(), Ordering::Relaxed);
    }

    /// Reads the block that plays from frame `at`, as many frames as `left`
    /// and `right` hold, silence where nothing arrived. The cursor goes on
    /// from where the last block ended while `at` is within a millisecond of
    /// it, so the rounding of each block's moment makes no click; a block
    /// further off starts over at `at`. Audio thread: no allocation, lock or
    /// blocking call.
    pub fn read(&self, at: i64, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let at = at.max(0) as u64;
        let near = (i64::from(self.rate) * RESYNC_MS / 1000).max(1);
        let cursor = match self.cursor.load(Ordering::Relaxed) {
            UNSET => at,
            cursor if (cursor as i64 - at as i64).abs() <= near => cursor,
            _ => at,
        };
        let end = self.end.load(Ordering::Acquire);
        let oldest = end.saturating_sub(self.capacity as u64);
        for i in 0..len {
            let frame = cursor + i as u64;
            if frame >= oldest && frame < end {
                let cell = (frame as usize & (self.capacity - 1)) * 2;
                left[i] = f32::from_bits(self.cells[cell].load(Ordering::Relaxed));
                right[i] = f32::from_bits(self.cells[cell + 1].load(Ordering::Relaxed));
            } else {
                left[i] = 0.0;
                right[i] = 0.0;
            }
        }
        self.cursor.store(cursor + len as u64, Ordering::Release);
    }

    /// Frames dropped for coming too late (or too early), ever.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Frames kept, ever: the count a poll compares to see audio arrive.
    #[must_use]
    pub fn received(&self) -> u64 {
        self.received.load(Ordering::Relaxed)
    }

    /// How many frames are waiting ahead of the audio thread: the audio
    /// buffered from the peer (R9).
    #[must_use]
    pub fn buffered(&self) -> u64 {
        let cursor = self.cursor.load(Ordering::Acquire);
        let end = self.end.load(Ordering::Acquire);
        if cursor == UNSET {
            0
        } else {
            end.saturating_sub(cursor)
        }
    }
}

impl std::fmt::Debug for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Channel")
            .field("rate", &self.rate)
            .field("buffered", &self.buffered())
            .field("dropped", &self.dropped())
            .finish_non_exhaustive()
    }
}

/// Link Audio on the audio thread: what the engine reads from the peers'
/// channels and offers of its own tracks. Whoever runs the engine on a device
/// implements it over a Link instance and hands it to
/// [`Engine::set_link_audio`](crate::Engine::set_link_audio). Every call is
/// made on the audio thread: no allocation, lock or blocking call.
pub trait LinkAudio: Send {
    /// Whether Link and Link Audio are both on. While they aren't, nothing is
    /// read or offered, and the tracks that take a channel hear silence.
    fn active(&self) -> bool;

    /// The channel a peer offers, by the names the track's input keeps, if
    /// the peer is in the session and the channel is taken.
    fn channel(&self, peer: &str, channel: &str) -> Option<&Channel>;

    /// The tracks offered to the peers, sorted by id.
    fn offered(&self) -> &[TrackId];

    /// Takes the frames `at..at + left.len()` of the block of `track`'s
    /// output as it leaves its mixer, to send when the block ends.
    fn stage(&mut self, track: TrackId, at: usize, left: &[f32], right: &[f32]);

    /// Ends a block of `frames` frames at `rate` whose first frame Link's
    /// clock gives as heard at `moment` microseconds: sends what was staged.
    fn send(&mut self, moment: i64, frames: usize, rate: u32);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(from: f32, len: usize) -> Vec<f32> {
        (0..len).map(|i| from + i as f32).collect()
    }

    #[test]
    fn frames_land_where_they_play_and_a_gap_is_silent() {
        let channel = Channel::new(48_000);
        channel.push(1_000, &ramp(1.0, 4), &ramp(101.0, 4));
        channel.push(1_008, &ramp(9.0, 2), &ramp(109.0, 2));
        let (mut left, mut right) = ([9.0; 12], [9.0; 12]);
        channel.read(998, &mut left, &mut right);
        assert_eq!(
            left,
            [0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 0.0, 0.0, 0.0, 0.0, 9.0, 10.0]
        );
        assert_eq!(right[2], 101.0);
        assert_eq!(right[11], 110.0);
        assert_eq!(channel.dropped(), 0);
        assert_eq!(channel.received(), 6);
    }

    #[test]
    fn what_arrives_after_its_frames_were_read_is_dropped_and_counted() {
        let channel = Channel::new(48_000);
        let (mut left, mut right) = ([0.0; 100], [0.0; 100]);
        channel.read(5_000, &mut left, &mut right);
        // Wholly late: 50 frames before 5,100.
        channel.push(4_900, &ramp(1.0, 50), &ramp(1.0, 50));
        assert_eq!(channel.dropped(), 50);
        // Late by its first 40 frames, the rest kept.
        channel.push(5_060, &ramp(1.0, 100), &ramp(1.0, 100));
        assert_eq!(channel.dropped(), 90);
        assert_eq!(channel.received(), 60);
        channel.read(5_100, &mut left, &mut right);
        assert_eq!(left[0], 41.0);
        assert_eq!(left[59], 100.0);
        assert_eq!(left[60], 0.0);
    }

    #[test]
    fn the_cursor_goes_on_through_a_frame_of_jitter_and_jumps_past_a_millisecond() {
        let channel = Channel::new(48_000);
        channel.push(0, &ramp(0.0, 2_000), &ramp(0.0, 2_000));
        let (mut left, mut right) = ([0.0; 100], [0.0; 100]);
        channel.read(100, &mut left, &mut right);
        // The next block says 101 instead of 200: the cursor kept on at 200.
        channel.read(201, &mut left, &mut right);
        assert_eq!(left[0], 200.0);
        // 1,000 frames off is a jump.
        channel.read(1_500, &mut left, &mut right);
        assert_eq!(left[0], 1_500.0);
    }

    #[test]
    fn a_ring_a_lap_behind_is_not_replayed() {
        let channel = Channel::new(1_000);
        channel.push(0, &vec![1.0; 1_000], &vec![1.0; 1_000]);
        let (mut left, mut right) = ([0.0; 10], [0.0; 10]);
        channel.read(2_000, &mut left, &mut right);
        // A buffer 2,020 frames on leaves the gap before it silent, though
        // those cells held 1.0 a lap ago.
        channel.push(2_020, &[2.0; 10], &[2.0; 10]);
        let (mut left, mut right) = ([9.0; 20], [9.0; 20]);
        channel.read(2_010, &mut left, &mut right);
        assert!(left[..10].iter().all(|&s| s == 0.0));
        assert!(left[10..].iter().all(|&s| s == 2.0));
    }

    #[test]
    fn micros_become_frames_without_losing_the_low_bits() {
        // A clock of seven days of microseconds, at 48 kHz, is past 2^53.
        let micros = 604_800_000_000i64 + 1;
        assert_eq!(frame_at(micros, 48_000), 29_030_400_000);
        assert_eq!(frame_at(micros + 11, 48_000), 29_030_400_001);
        assert_eq!(frame_at(0, 48_000), 0);
        assert_eq!(frame_at(10_417, 48_000), 500);
    }
}

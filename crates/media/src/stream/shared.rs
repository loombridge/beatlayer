//! What a stream's two ends share: the blocks and cues that travel between
//! them, the request the reader makes of the disk thread, and the counters
//! both keep.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::time::Duration;

/// What a stream counted, or what every stream of a [`Streamer`] counted.
///
/// [`Streamer`]: super::Streamer
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamStats {
    /// Times the audio ran out while playing on: each stretch of silence the
    /// disk thread didn't fill in time counts once. Any is a fault, which
    /// the disk overload indicator shows.
    pub underruns: u64,
    /// Frames of silence those underruns played.
    pub underrun_frames: u64,
    /// Jumps while reading: a read that didn't start where the last one
    /// ended.
    pub seeks: u64,
    /// Jumps whose new position was ready: buffered already, preloaded at
    /// the clip's start, or cued.
    pub seek_hits: u64,
    /// Jumps whose new position had to be read from disk first.
    pub seek_misses: u64,
    /// Frames of silence played after a miss while the disk thread read the
    /// new position. A jump nothing was cued for costs a short wait; it's
    /// not an underrun.
    pub waited_frames: u64,
    /// Blocks the disk thread decoded.
    pub blocks: u64,
    /// Files that failed to read or decode part-way; a failed stream plays
    /// silence from the failure on.
    pub errors: u64,
}

impl StreamStats {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The disk thread's figures, from [`Streamer::stats`].
///
/// [`Streamer::stats`]: super::Streamer::stats
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamerStats {
    /// Streams open now.
    pub streams: usize,
    /// What every stream has counted since the streamer started, closed
    /// streams included.
    pub totals: StreamStats,
    /// How long the disk thread has spent reading, decoding, and converting.
    pub busy: Duration,
}

/// Counters, one per figure in [`StreamStats`], that any thread adds to.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    underruns: AtomicU64,
    underrun_frames: AtomicU64,
    seeks: AtomicU64,
    seek_hits: AtomicU64,
    seek_misses: AtomicU64,
    waited_frames: AtomicU64,
    blocks: AtomicU64,
    errors: AtomicU64,
}

impl Counters {
    /// Adds `delta`. Real-time safe: a few relaxed atomic additions.
    pub(crate) fn add(&self, delta: &StreamStats) {
        if delta.is_empty() {
            return;
        }
        let add = |counter: &AtomicU64, value: u64| {
            if value > 0 {
                counter.fetch_add(value, Relaxed);
            }
        };
        add(&self.underruns, delta.underruns);
        add(&self.underrun_frames, delta.underrun_frames);
        add(&self.seeks, delta.seeks);
        add(&self.seek_hits, delta.seek_hits);
        add(&self.seek_misses, delta.seek_misses);
        add(&self.waited_frames, delta.waited_frames);
        add(&self.blocks, delta.blocks);
        add(&self.errors, delta.errors);
    }

    pub(crate) fn snapshot(&self) -> StreamStats {
        StreamStats {
            underruns: self.underruns.load(Relaxed),
            underrun_frames: self.underrun_frames.load(Relaxed),
            seeks: self.seeks.load(Relaxed),
            seek_hits: self.seek_hits.load(Relaxed),
            seek_misses: self.seek_misses.load(Relaxed),
            waited_frames: self.waited_frames.load(Relaxed),
            blocks: self.blocks.load(Relaxed),
            errors: self.errors.load(Relaxed),
        }
    }
}

/// A streamer's figures across all its streams.
#[derive(Debug, Default)]
pub(crate) struct Totals {
    pub(crate) counters: Counters,
    pub(crate) streams: AtomicUsize,
    pub(crate) busy_nanos: AtomicU64,
    /// Whether the disk thread has paused, once it has.
    pub(crate) paused: AtomicBool,
}

/// What one stream's reader and the disk thread share.
#[derive(Debug)]
pub(crate) struct Shared {
    pub(crate) id: u64,
    /// The block sequence the reader wants. The reader raises it on every
    /// jump the blocks it has can't serve; blocks of an older one are stale.
    pub(crate) epoch: AtomicU64,
    /// The frame the wanted sequence starts at. Written before `epoch`, so
    /// the disk thread, which reads `epoch` first, sees this target or a
    /// later one.
    pub(crate) target: AtomicU64,
    /// Whether the reader has started, so the disk thread should read ahead.
    /// Until then a stream holds only its preloaded start.
    pub(crate) armed: AtomicBool,
    pub(crate) counters: Counters,
    pub(crate) totals: Arc<Totals>,
}

impl Shared {
    pub(crate) fn new(id: u64, epoch: u64, target: u64, totals: Arc<Totals>) -> Self {
        Self {
            id,
            epoch: AtomicU64::new(epoch),
            target: AtomicU64::new(target),
            armed: AtomicBool::new(false),
            counters: Counters::default(),
            totals,
        }
    }

    /// Adds to this stream's counters and the streamer's totals. Real-time
    /// safe.
    pub(crate) fn count(&self, delta: &StreamStats) {
        self.counters.add(delta);
        self.totals.counters.add(delta);
    }
}

/// Decoded audio at the engine's rate, from the disk thread: `len` frames
/// from `start`, planar, channel `c` at `samples[c * capacity]`.
#[derive(Debug)]
pub(crate) struct Block {
    /// The sequence it belongs to.
    pub(crate) epoch: u64,
    pub(crate) start: u64,
    pub(crate) len: usize,
    /// Where the source ends, if this block reached the end: after its last
    /// frame, or, for an empty block, wherever the end is known to be (a
    /// sequence asked for past the end, or a file that failed to read).
    pub(crate) ends_at: Option<u64>,
    pub(crate) samples: Box<[f32]>,
}

impl Block {
    pub(crate) fn end_frame(&self) -> u64 {
        self.start + self.len as u64
    }
}

/// A preloaded stretch of a stream, kept until it's replaced: the clip's
/// start in slot 0, and points a jump is expected to land on (a loop's
/// start) in the others.
#[derive(Debug)]
pub(crate) struct Cue {
    pub(crate) slot: usize,
    pub(crate) start: u64,
    pub(crate) len: usize,
    /// Where the source ends, if this cue reached the end.
    pub(crate) ends_at: Option<u64>,
    pub(crate) samples: Box<[f32]>,
}

impl Cue {
    pub(crate) fn end_frame(&self) -> u64 {
        self.start + self.len as u64
    }

    pub(crate) fn covers(&self, frame: u64) -> bool {
        (self.start..self.end_frame()).contains(&frame)
    }
}

/// From the disk thread to a reader.
#[derive(Debug)]
pub(crate) enum ToReader {
    Block(Block),
    Cue(Cue),
}

/// From a reader back to the disk thread: buffers it has finished with, to
/// reuse or free there, never on the audio thread.
#[derive(Debug)]
pub(crate) enum ToDisk {
    Block(Box<[f32]>),
    Cue(Box<[f32]>),
}

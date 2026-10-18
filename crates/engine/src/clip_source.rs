//! What a clip player holds for each clip's audio (WP-4.2): its source, with
//! two things a source alone doesn't give a warped clip.
//!
//! - **A tail.** A clip read through an interpolator reads a window of the
//!   file around each position, so a block's window overlaps the last one's
//!   by a few dozen frames. A streamed source discards what it has played
//!   and treats a read behind its position as a jump, so a streamed clip
//!   keeps the last frames it read here and serves an overlap from them; the
//!   source itself is only ever read forwards.
//! - **A handover.** Switching a clip's RAM mode gives it a source of the
//!   other kind. The old source goes on playing until the new one has the
//!   audio from where the clip has reached, and the next read comes from the
//!   new one, on the same frame: no gap and no step (the warp modes spec,
//!   R11).
//!
//! Making one allocates, so the compiler does; the player only moves them.

use bl_media::stream::{AudioSource, Filled, PlanarMut, StreamStats, Transition};

/// The most frames a single read may ask for: a warped clip's window.
pub(crate) const WINDOW_FRAMES: usize = 10_240;

/// How many frames of what it has read a streamed clip keeps: the most a
/// window reaches back, with room to spare.
const TAIL_FRAMES: usize = 1_024;

/// The most frames a read may skip ahead and still go on forwards, reading
/// the gap through rather than jumping over it.
const GAP_FRAMES: u64 = 4_096;

/// How many frames the new source must hold ready before a handover takes
/// over from the old one.
const READY_FRAMES: u64 = 2_048;

/// How far a new source's preload may be behind the clip before the handover
/// asks it to preload again, nearer.
const STALE_FRAMES: u64 = 8_192;

/// The last frames a streamed source produced, planar.
struct Tail {
    data: Vec<f32>,
    channels: usize,
    /// How many frames it holds, ending at file frame `end`.
    len: usize,
    end: u64,
}

impl Tail {
    fn new(channels: usize) -> Self {
        Self {
            data: vec![0.0; channels * TAIL_FRAMES],
            channels,
            len: 0,
            end: 0,
        }
    }

    /// Adds `frames` frames just read, which end at file frame `end`: the
    /// first `channels` channels, each given by `channel`.
    fn push<'a>(
        &mut self,
        frames: usize,
        end: u64,
        channels: usize,
        channel: impl Fn(usize) -> &'a [f32],
    ) {
        let keep = frames.min(TAIL_FRAMES);
        let forget = (self.len + keep).saturating_sub(TAIL_FRAMES);
        for c in 0..channels.min(self.channels) {
            let held = &mut self.data[c * TAIL_FRAMES..(c + 1) * TAIL_FRAMES];
            if forget > 0 {
                held.copy_within(forget..self.len, 0);
            }
            let at = self.len - forget;
            held[at..at + keep].copy_from_slice(&channel(c)[frames - keep..frames]);
        }
        self.len = self.len - forget + keep;
        self.end = end;
    }
}

/// A clip's audio, ready for the player to read: see the module.
pub(crate) struct ClipSource {
    current: Option<Box<dyn AudioSource>>,
    /// Present for a streamed source.
    tail: Option<Tail>,
    /// Scratch for the part of a read that comes from the source when the
    /// rest comes from the tail, or a gap before it is read through.
    scratch: Vec<f32>,
    /// The source a handover is waiting on, and, after one, the one it
    /// retired, which the next list's drop frees.
    pending: Option<Box<ClipSource>>,
    handing_over: bool,
    /// Where the pending source was last asked to preload.
    armed: Option<u64>,
}

impl ClipSource {
    /// A source for a clip: a `streamed` one keeps a tail.
    pub(crate) fn new(source: Box<dyn AudioSource>, streamed: bool) -> Self {
        let channels = source.channels();
        Self {
            current: Some(source),
            tail: streamed.then(|| Tail::new(channels)),
            scratch: if streamed {
                vec![0.0; channels * WINDOW_FRAMES]
            } else {
                Vec::new()
            },
            pending: None,
            handing_over: false,
            armed: None,
        }
    }

    /// Begins a handover from `old`, the source a clip has been reading, to
    /// `new`: `old` goes on playing until `new` holds the audio from where
    /// the clip has reached. Returns what `old` had retired before, to go
    /// to the drop thread. Moves pointers and allocates and frees nothing.
    pub(crate) fn hand_over(old: &mut Box<Self>, new: Box<Self>) -> Option<Box<Self>> {
        old.handing_over = true;
        old.armed = None;
        old.pending.replace(new)
    }

    fn source(&self) -> &dyn AudioSource {
        self.current.as_deref().expect("a clip source has a source")
    }

    fn source_mut(&mut self) -> &mut dyn AudioSource {
        self.current
            .as_deref_mut()
            .expect("a clip source has a source")
    }

    /// Reads from the current source, forwards only if it streams.
    fn read_current(
        &mut self,
        position: u64,
        out: &mut PlanarMut<'_>,
        transition: Transition,
    ) -> Filled {
        let n = out.frames();
        let (Some(source), Some(tail)) = (self.current.as_deref_mut(), self.tail.as_mut()) else {
            return self.source_mut().read(position, out, transition);
        };
        let channels = tail.channels.min(out.channels());
        let usable = tail.len > 0 && tail.end == source.position();
        let behind = tail.end.saturating_sub(position);
        if usable && position < tail.end && behind <= tail.len as u64 {
            // Part of this read is behind the source: the tail has it.
            let behind = behind as usize;
            let overlap = behind.min(n);
            for c in 0..channels {
                let from = c * TAIL_FRAMES + tail.len - behind;
                out.channel_mut(c)[..overlap].copy_from_slice(&tail.data[from..from + overlap]);
            }
            if overlap == n {
                return Filled {
                    frames: n,
                    ..Filled::default()
                };
            }
            let (rest, end) = (n - overlap, tail.end);
            let mut view = PlanarMut::new(&mut self.scratch, tail.channels, rest, WINDOW_FRAMES);
            let filled = source.read(end, &mut view, transition);
            for c in 0..channels {
                out.channel_mut(c)[overlap..].copy_from_slice(view.channel(c));
            }
            tail.push(rest, end + rest as u64, channels, |c| view.channel(c));
            return filled;
        }
        let gap = position.saturating_sub(tail.end);
        if usable && position > tail.end && gap <= GAP_FRAMES && gap as usize + n <= WINDOW_FRAMES {
            // The read skips a few frames ahead: read through them.
            let (gap, end) = (gap as usize, tail.end);
            let mut view = PlanarMut::new(&mut self.scratch, tail.channels, gap + n, WINDOW_FRAMES);
            let filled = source.read(end, &mut view, transition);
            for c in 0..channels {
                out.channel_mut(c).copy_from_slice(&view.channel(c)[gap..]);
            }
            tail.push(gap + n, end + (gap + n) as u64, channels, |c| {
                view.channel(c)
            });
            return filled;
        }
        if position != source.position() {
            tail.len = 0;
        }
        let filled = source.read(position, out, transition);
        tail.push(n, position + n as u64, channels, |c| out.channel(c));
        filled
    }

    /// While a handover waits: asks the new source to preload where the clip
    /// will read next, and takes over once it holds the audio from there.
    fn advance_handover(&mut self, upcoming: u64) {
        let Some(pending) = self.pending.as_mut() else {
            self.handing_over = false;
            return;
        };
        let ready = |pending: &mut Box<Self>, armed: u64| {
            let buffered = pending.buffered();
            upcoming >= armed
                && (armed + buffered >= upcoming + READY_FRAMES
                    || pending
                        .frames()
                        .is_some_and(|total| armed + buffered >= total))
        };
        match self.armed {
            Some(armed) if ready(pending, armed) => {
                // The new source's audio is ready from `armed`, and the clip
                // is inside it: from now on the clip reads the new source.
                let new = self.pending.as_mut().expect("a pending source");
                std::mem::swap(&mut self.current, &mut new.current);
                std::mem::swap(&mut self.tail, &mut new.tail);
                std::mem::swap(&mut self.scratch, &mut new.scratch);
                self.handing_over = false;
                self.armed = None;
            }
            Some(armed) if upcoming >= armed && upcoming < armed + STALE_FRAMES => {}
            _ => {
                pending.seek(upcoming);
                self.armed = Some(upcoming);
            }
        }
    }
}

impl AudioSource for ClipSource {
    fn channels(&self) -> usize {
        self.source().channels()
    }

    fn sample_rate(&self) -> u32 {
        self.source().sample_rate()
    }

    fn frames(&self) -> Option<u64> {
        self.source().frames()
    }

    fn position(&self) -> u64 {
        self.source().position()
    }

    fn read(&mut self, position: u64, out: &mut PlanarMut<'_>, transition: Transition) -> Filled {
        let upcoming = position + out.frames() as u64;
        let filled = self.read_current(position, out, transition);
        if self.handing_over {
            self.advance_handover(upcoming);
        }
        filled
    }

    fn seek(&mut self, position: u64) {
        if let Some(tail) = self.tail.as_mut() {
            tail.len = 0;
        }
        self.source_mut().seek(position);
    }

    fn stop(&mut self) {
        self.source_mut().stop();
        if let Some(pending) = self.pending.as_mut().filter(|_| self.handing_over) {
            pending.stop();
        }
    }

    fn buffered(&mut self) -> u64 {
        self.source_mut().buffered()
    }

    fn stats(&self) -> StreamStats {
        self.source().stats()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

    use super::*;

    /// A source whose frame `i` is `offset + i`, that counts the jumps it is
    /// asked to make and has `ready` frames buffered.
    struct Counting {
        offset: f32,
        position: u64,
        ready: Arc<AtomicU64>,
        jumps: Arc<AtomicU64>,
    }

    impl Counting {
        fn new(offset: f32) -> (Self, Arc<AtomicU64>, Arc<AtomicU64>) {
            let ready = Arc::new(AtomicU64::new(0));
            let jumps = Arc::new(AtomicU64::new(0));
            let source = Self {
                offset,
                position: 0,
                ready: Arc::clone(&ready),
                jumps: Arc::clone(&jumps),
            };
            (source, ready, jumps)
        }
    }

    impl AudioSource for Counting {
        fn channels(&self) -> usize {
            1
        }
        fn sample_rate(&self) -> u32 {
            48_000
        }
        fn frames(&self) -> Option<u64> {
            Some(1_000_000)
        }
        fn position(&self) -> u64 {
            self.position
        }
        fn read(&mut self, position: u64, out: &mut PlanarMut<'_>, _: Transition) -> Filled {
            if position != self.position {
                self.jumps.fetch_add(1, Relaxed);
            }
            for (i, sample) in out.channel_mut(0).iter_mut().enumerate() {
                *sample = self.offset + (position + i as u64) as f32;
            }
            self.position = position + out.frames() as u64;
            Filled {
                frames: out.frames(),
                ..Filled::default()
            }
        }
        fn seek(&mut self, position: u64) {
            self.position = position;
        }
        fn stop(&mut self) {}
        fn buffered(&mut self) -> u64 {
            self.ready.load(Relaxed)
        }
        fn stats(&self) -> StreamStats {
            StreamStats::default()
        }
    }

    fn read(source: &mut ClipSource, position: u64, frames: usize) -> Vec<f32> {
        let mut data = vec![0.0; frames];
        let mut view = PlanarMut::new(&mut data, 1, frames, frames);
        source.read(position, &mut view, Transition::Cut);
        data
    }

    fn ramp(offset: f32, from: u64, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| offset + (from + i as u64) as f32)
            .collect()
    }

    // Spec: warping R18. A streamed clip reads its source forwards only, even
    // where a window reaches back over the last one or skips a little ahead.
    #[test]
    fn a_streamed_clip_serves_overlapping_and_skipping_reads_from_its_tail() {
        let (source, _, jumps) = Counting::new(0.0);
        let mut clip = ClipSource::new(Box::new(source), true);
        assert_eq!(read(&mut clip, 1_000, 256), ramp(0.0, 1_000, 256));
        let first = jumps.load(Relaxed);
        // 40 frames back over the last read, 300 on.
        assert_eq!(read(&mut clip, 1_216, 300), ramp(0.0, 1_216, 300));
        // Wholly inside the tail.
        assert_eq!(read(&mut clip, 1_500, 30), ramp(0.0, 1_500, 30));
        // 20 frames ahead of where the source is.
        assert_eq!(read(&mut clip, 1_576, 100), ramp(0.0, 1_576, 100));
        assert_eq!(
            jumps.load(Relaxed),
            first,
            "the source was only read forwards"
        );
        // A real jump is the source's.
        assert_eq!(read(&mut clip, 50_000, 64), ramp(0.0, 50_000, 64));
        assert_eq!(jumps.load(Relaxed), first + 1);
    }

    // Spec: warp-modes R11. A handover plays the old source until the new one
    // holds the audio from where the clip has reached, then the new one,
    // without a step.
    #[test]
    fn a_handover_waits_for_the_new_source_and_then_takes_over() {
        let (old, _, _) = Counting::new(0.0);
        let (new, ready, _) = Counting::new(0.5);
        let mut playing = Box::new(ClipSource::new(Box::new(old), false));
        let displaced = ClipSource::hand_over(
            &mut playing,
            Box::new(ClipSource::new(Box::new(new), false)),
        );
        assert!(displaced.is_none());
        // The new source has nothing ready: the old plays on.
        for block in 0..4 {
            assert_eq!(
                read(&mut playing, 256 * block, 256),
                ramp(0.0, 256 * block, 256)
            );
        }
        assert!(playing.handing_over);
        // Once it holds the audio, the read that notices is the old
        // source's last, and the next is the new one's.
        ready.store(100_000, Relaxed);
        assert_eq!(read(&mut playing, 1_024, 256), ramp(0.0, 1_024, 256));
        assert!(!playing.handing_over);
        assert_eq!(read(&mut playing, 1_280, 256), ramp(0.5, 1_280, 256));
        assert!(
            playing.pending.is_some(),
            "the retired source waits to be freed"
        );
    }
}

//! What every source does on the audio thread, whatever holds its audio:
//! follow the reads, notice jumps, fade across them, and count what went
//! wrong.

use core::mem;

use super::planar::PlanarMut;
use super::shared::StreamStats;
use super::source::{Filled, Transition};
use super::splice::Splice;

/// What a supply has at a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Got {
    /// It copied this many frames.
    Data(usize),
    /// It has nothing for this many frames yet: the disk hasn't read them.
    Missing(usize),
    /// This many frames are past the end of the source.
    End(usize),
}

/// Where a source's audio comes from: memory, or blocks from the disk
/// thread. Every method runs on the audio thread and must follow the
/// real-time rules.
pub(crate) trait Supply {
    /// Takes in whatever has arrived from elsewhere.
    fn poll(&mut self) {}

    /// The source is about to play: start reading ahead.
    fn activate(&mut self) {}

    /// Copies up to `frames` frames from source frame `at` into `out` at
    /// frame `offset`, in its first `channels` channels, or says why it
    /// can't. With `consume`, data before `at` is finished with.
    fn fetch(
        &mut self,
        at: u64,
        out: &mut PlanarMut<'_>,
        offset: usize,
        frames: usize,
        channels: usize,
        consume: bool,
    ) -> Got;

    /// Reading jumps to `to`. Returns whether audio there is ready now.
    fn relocate(&mut self, to: u64) -> bool;

    /// Reading ran out of data while playing on, and is now at `at`: if the
    /// supply has fallen far behind, it should skip ahead.
    fn fell_behind(&mut self, at: u64) {
        let _ = at;
    }

    /// Records what a read counted.
    fn count(&mut self, delta: &StreamStats);
}

/// A source's reading state over a supply.
#[derive(Debug)]
pub(crate) struct Player<S> {
    pub(crate) supply: S,
    channels: usize,
    /// The frame the next read continues from.
    position: u64,
    /// Whether the last call was a read, so audio is sounding and a jump
    /// has something to fade out.
    sounding: bool,
    /// Whether reading waits on the disk after a jump nothing was ready
    /// for: silence until then counts as waiting, not as an underrun.
    waiting: bool,
    /// Whether the last frame read was an underrun's silence.
    underrunning: bool,
    splice: Splice,
}

impl<S: Supply> Player<S> {
    pub(crate) fn new(supply: S, channels: usize, position: u64, fade: usize) -> Self {
        Self {
            supply,
            channels,
            position,
            sounding: false,
            waiting: false,
            underrunning: false,
            splice: Splice::new(channels, fade),
        }
    }

    pub(crate) fn channels(&self) -> usize {
        self.channels
    }

    pub(crate) fn position(&self) -> u64 {
        self.position
    }

    pub(crate) fn fade_frames(&self) -> usize {
        self.splice.len()
    }

    pub(crate) fn fading(&self) -> bool {
        self.splice.fading()
    }

    /// Reads `out.frames()` frames from `position` (see
    /// [`AudioSource::read`](super::AudioSource::read)).
    pub(crate) fn read(
        &mut self,
        position: u64,
        out: &mut PlanarMut<'_>,
        transition: Transition,
    ) -> Filled {
        self.supply.poll();
        self.supply.activate();
        let mut delta = StreamStats::default();
        if position != self.position {
            delta.seeks += 1;
            if transition == Transition::Crossfade && self.sounding {
                self.capture_outgoing();
                self.splice.begin_crossfade();
            } else {
                self.splice.reset();
            }
            self.underrunning = false;
            if self.supply.relocate(position) {
                delta.seek_hits += 1;
                self.waiting = false;
            } else {
                delta.seek_misses += 1;
                self.waiting = true;
            }
        }
        let filled = self.render(position, out, true, &mut delta);
        self.position = position + out.frames() as u64;
        self.sounding = true;
        if filled.missing > 0 {
            self.supply.fell_behind(self.position);
        }
        self.supply.count(&delta);
        filled
    }

    /// Moves to `position` without reading, so the next read from there
    /// finds it ready (see [`AudioSource::seek`](super::AudioSource::seek)).
    pub(crate) fn seek(&mut self, position: u64) {
        self.supply.poll();
        self.supply.activate();
        self.stop();
        if position != self.position {
            self.waiting = !self.supply.relocate(position);
            self.position = position;
        }
    }

    /// Audio stops sounding: the next read starts afresh, with nothing to
    /// fade out.
    pub(crate) fn stop(&mut self) {
        self.sounding = false;
        self.underrunning = false;
        self.splice.reset();
    }

    /// Renders the audio that would have followed the last read, with any
    /// fades running, into the splice's capture buffer: what a crossfade
    /// fades out.
    fn capture_outgoing(&mut self) {
        let len = self.splice.len();
        let mut capture = mem::take(&mut self.splice.capture);
        {
            let mut view = PlanarMut::new(&mut capture, self.channels, len, len);
            self.render(self.position, &mut view, false, &mut StreamStats::default());
        }
        self.splice.capture = capture;
    }

    /// Fills `out` from source frame `at`: audio where the supply has it,
    /// faded in after any silence; silence where it hasn't; and then any
    /// fade-out mixed in. Channels past the source's are silenced.
    fn render(
        &mut self,
        mut at: u64,
        out: &mut PlanarMut<'_>,
        consume: bool,
        delta: &mut StreamStats,
    ) -> Filled {
        let frames = out.frames();
        let channels = self.channels.min(out.channels());
        let mut filled = Filled::default();
        let mut done = 0;
        while done < frames {
            let left = frames - done;
            match self.supply.fetch(at, out, done, left, channels, consume) {
                Got::Data(n) => {
                    let n = n.clamp(1, left);
                    self.splice.fade_in(out, channels, done..done + n);
                    if consume {
                        self.waiting = false;
                        self.underrunning = false;
                    }
                    filled.frames += n;
                    done += n;
                    at += n as u64;
                }
                Got::Missing(n) => {
                    let n = n.clamp(1, left);
                    out.silence(0..channels, done..done + n);
                    self.splice.restart_fade_in();
                    if consume {
                        if self.waiting {
                            delta.waited_frames += n as u64;
                        } else {
                            if !self.underrunning {
                                delta.underruns += 1;
                                self.underrunning = true;
                            }
                            delta.underrun_frames += n as u64;
                        }
                    }
                    filled.missing += n;
                    done += n;
                    at += n as u64;
                }
                Got::End(n) => {
                    let n = n.clamp(1, left);
                    out.silence(0..channels, done..done + n);
                    filled.past_end += n;
                    done += n;
                    at += n as u64;
                }
            }
        }
        out.silence(channels..out.channels(), 0..frames);
        self.splice.fade_out(out, channels);
        filled
    }
}

//! A long file decoded into memory in the background (WP-5.13; the
//! multisampler spec, R23): a sampler voice reads it at positions of its
//! own, so it needs its audio by frame, which a stream's sequential blocks
//! don't give. The start is decoded before the sample exists, so a note
//! never waits for the disk, and a thread fills in the rest while it plays;
//! a read past what's there plays silence, which the caller counts as an
//! underrun.
//!
//! shortcut: the rest stays in memory once read, so the memory use of a
//! long layer is its whole length, as RAM Mode's is, and only the loading
//! is spread over time; upgrade to evicting what no voice is near.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::decode::Decoder;
use crate::error::MediaError;
use crate::limits::MAX_PROGRESSIVE_SAMPLES;

/// Frames the filling thread decodes at a time.
const BLOCK: usize = 16_384;

/// Audio at its own rate, made ahead as a whole and filled as the disk
/// delivers it.
#[derive(Debug)]
pub struct ProgressiveSample {
    sample_rate: u32,
    frames: usize,
    /// One run of bit-patterns of `f32`s per channel, written once by the
    /// filling thread before [`ready`](Self::ready) passes them.
    channels: Box<[Box<[AtomicU32]>]>,
    ready: AtomicUsize,
}

/// The thread's side of a [`ProgressiveSample`]: what's left to decode.
pub struct Fill {
    sample: Arc<ProgressiveSample>,
    decoder: Decoder,
    at: usize,
}

impl ProgressiveSample {
    /// Opens `decoder`'s file, decodes its first `head` frames here, and
    /// gives the sample with the [`Fill`] that decodes the rest: run it on a
    /// thread of its own ([`Fill::spawn`]).
    ///
    /// # Errors
    ///
    /// If the file's header doesn't give its length, the length is more
    /// than [`MAX_PROGRESSIVE_SAMPLES`], or it can't be read.
    pub fn start(decoder: Decoder, head: usize) -> Result<(Arc<Self>, Fill), MediaError> {
        let info = *decoder.info();
        let frames = info
            .frames
            .and_then(|f| usize::try_from(f).ok())
            .filter(|f| *f > 0)
            .ok_or_else(|| MediaError::Malformed("a file whose length isn't known".into()))?;
        let channels = usize::from(info.channels);
        if frames.saturating_mul(channels) > MAX_PROGRESSIVE_SAMPLES {
            return Err(MediaError::Unsupported(format!(
                "{frames} frames of {channels} channels, more than the {MAX_PROGRESSIVE_SAMPLES} samples \
                 loaded progressively"
            )));
        }
        let sample = Arc::new(Self {
            sample_rate: info.sample_rate,
            frames,
            channels: (0..channels)
                .map(|_| (0..frames).map(|_| AtomicU32::new(0)).collect())
                .collect(),
            ready: AtomicUsize::new(0),
        });
        let mut fill = Fill {
            sample: Arc::clone(&sample),
            decoder,
            at: 0,
        };
        fill.read_until(head.min(frames))?;
        Ok((sample, fill))
    }

    /// Frames per second.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The number of frames the file holds, decoded or not.
    #[must_use]
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// The number of channels.
    #[must_use]
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// How many frames from the start are decoded.
    #[must_use]
    pub fn ready(&self) -> usize {
        self.ready.load(Ordering::Acquire)
    }

    /// The sample of `channel` at `frame`: silence for a frame past the
    /// file, and for one the disk hasn't delivered yet. Real-time safe.
    #[must_use]
    pub fn get(&self, channel: usize, frame: usize) -> f32 {
        if frame < self.ready() {
            self.channels
                .get(channel)
                .map_or(0.0, |c| f32::from_bits(c[frame].load(Ordering::Relaxed)))
        } else {
            0.0
        }
    }
}

impl Fill {
    /// Decodes up to frame `end`.
    fn read_until(&mut self, end: usize) -> Result<(), MediaError> {
        let channels = self.sample.channels.len();
        let mut block: Vec<Vec<f32>> = vec![vec![0.0; BLOCK]; channels];
        while self.at < end {
            // Nothing else holds the sample any more: stop reading for it.
            if Arc::strong_count(&self.sample) == 1 {
                break;
            }
            let want = (end - self.at).min(BLOCK);
            let mut outs: Vec<&mut [f32]> = block.iter_mut().map(|c| &mut c[..want]).collect();
            let got = self.decoder.read(&mut outs)?;
            if got == 0 {
                break;
            }
            for (channel, data) in self.sample.channels.iter().zip(&block) {
                for (slot, value) in channel[self.at..self.at + got].iter().zip(data) {
                    slot.store(value.to_bits(), Ordering::Relaxed);
                }
            }
            self.at += got;
            self.sample.ready.store(self.at, Ordering::Release);
        }
        Ok(())
    }

    /// Decodes the rest of the file, on this thread. A read error stops
    /// it: what's past stays silent.
    pub fn run(mut self) {
        let end = self.sample.frames;
        let _ = self.read_until(end);
    }

    /// Runs [`Fill::run`] on a thread of its own, which ends with the file.
    ///
    /// # Errors
    ///
    /// If the thread can't start.
    pub fn spawn(self) -> std::io::Result<()> {
        std::thread::Builder::new()
            .name("bl-progressive-sample".to_owned())
            .spawn(move || self.run())
            .map(drop)
    }
}

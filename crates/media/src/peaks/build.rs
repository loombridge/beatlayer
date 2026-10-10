//! Building peaks: decoding media and finding each window's extremes.
//!
//! A long file is cut into segments, about four per thread, that decode in
//! parallel, each worker with its own decoder that seeks to its segment's
//! first frame. Seeks are sample-accurate in every format (the decoder
//! pre-rolls lossy codecs), so the peaks are bit-identical to decoding the
//! file in order, which is what a one-thread build does. A segment that finds
//! the file shorter or longer than its header said makes the build start
//! again in order.
//!
//! Few segments matter for MP3, where symphonia's accurate seek scans the
//! file's frames from the start: 660 segments of an hour took 1.3 seconds on
//! an M5 Max, against 0.19 seconds for 42.

use std::fmt;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rayon::prelude::*;

use super::{FRAMES_PER_PEAK, Peaks, WINDOW};
use crate::decode::Decoder;
use crate::error::MediaError;
use crate::hash::ContentHash;
use crate::sample_data::SampleData;
use crate::store::{MediaStatus, MediaStore};

/// Each channel's level-0 peaks, as `[min, max]` byte pairs.
type Level0Peaks = Vec<Vec<[u8; 2]>>;

/// Segments per thread: enough that a fast core can take a slow core's
/// share at the end, few enough that seeking stays cheap.
const SEGMENTS_PER_THREAD: u64 = 4;

/// The most frames a header's length reserves room for: a damaged header can
/// claim any length. 2^28 frames is over an hour and a half at 48 kHz.
const MAX_RESERVE: u64 = 1 << 28;

/// Frames decoded per read.
const BLOCK: usize = 64 * WINDOW;

/// How a peak build runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildOptions {
    /// Decoding threads. `None` uses every thread of the current rayon pool
    /// (all cores, outside one); `Some(1)` decodes the file in order on the
    /// calling thread.
    pub threads: Option<NonZeroUsize>,
    /// The fewest frames a parallel task decodes, rounded up to whole
    /// peaks. A build cuts a file into about four segments per thread, but
    /// none shorter than this, and a file no longer than this decodes in
    /// order. Each segment costs one seek.
    pub segment_frames: u64,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            threads: None,
            // About 22 seconds at 48 kHz. Shorter segments cost more in
            // seeking and opening decoders than parallel decoding gains.
            segment_frames: 1 << 20,
        }
    }
}

impl BuildOptions {
    /// Options that decode in order on the calling thread.
    #[must_use]
    pub fn single_threaded() -> Self {
        Self {
            threads: NonZeroUsize::new(1),
            ..Self::default()
        }
    }
}

/// A build's progress, which another thread can watch, and its cancel switch.
#[derive(Default)]
pub struct BuildProgress {
    done: AtomicU64,
    /// The frames to build, plus one; zero while unknown.
    total: AtomicU64,
    cancelled: AtomicBool,
    observer: Option<Box<dyn Fn(u64) + Send + Sync>>,
}

impl fmt::Debug for BuildProgress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BuildProgress")
            .field("frames_done", &self.frames_done())
            .field("frames_total", &self.frames_total())
            .field("cancelled", &self.is_cancelled())
            .finish_non_exhaustive()
    }
}

impl BuildProgress {
    /// Progress at nothing done.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Progress that also calls `observer` with the frames done so far after
    /// every block, from whichever thread decoded it.
    #[must_use]
    pub fn with_observer(observer: impl Fn(u64) + Send + Sync + 'static) -> Self {
        Self {
            observer: Some(Box::new(observer)),
            ..Self::default()
        }
    }

    /// Frames decoded so far.
    #[must_use]
    pub fn frames_done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }

    /// Frames to decode in all, once the header has been read, if it says.
    #[must_use]
    pub fn frames_total(&self) -> Option<u64> {
        self.total.load(Ordering::Relaxed).checked_sub(1)
    }

    /// Asks the build to stop. It stops within one block and fails with
    /// [`MediaError::Cancelled`].
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// Whether [`BuildProgress::cancel`] was called.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    fn start(&self, total: Option<u64>) {
        self.done.store(0, Ordering::Relaxed);
        self.total
            .store(total.map_or(0, |t| t.saturating_add(1)), Ordering::Relaxed);
    }

    fn advance(&self, frames: usize) -> Result<(), MediaError> {
        let done = self.done.fetch_add(frames as u64, Ordering::Relaxed) + frames as u64;
        if let Some(observer) = &self.observer {
            observer(done);
        }
        if self.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        Ok(())
    }
}

impl Peaks {
    /// Builds the peaks of media in the store. The store checks the file
    /// still holds the media before it's read, and again after, so peaks are
    /// never built from a file that changed part-way through.
    ///
    /// # Errors
    ///
    /// Fails if the media is unknown, missing, or changed (before or during
    /// the build), can't be decoded, or the build is cancelled.
    pub fn build(
        store: &MediaStore,
        hash: ContentHash,
        options: &BuildOptions,
        progress: &BuildProgress,
    ) -> Result<Self, MediaError> {
        let first = store.open(hash)?;
        let path = store
            .entry(hash)
            .ok_or(MediaError::UnknownMedia(hash))?
            .path;
        let peaks = Self::build_from(first, || Decoder::open(&path), options, progress)?;
        match store.check(hash)? {
            MediaStatus::Available => Ok(peaks),
            MediaStatus::Missing => Err(MediaError::Missing { hash, path }),
            MediaStatus::Changed { now } => Err(MediaError::Changed { hash, path, now }),
        }
    }

    /// Builds the peaks of a file, outside the media store.
    ///
    /// # Errors
    ///
    /// Fails if the file can't be decoded or the build is cancelled.
    pub fn build_file(
        path: impl AsRef<Path>,
        options: &BuildOptions,
        progress: &BuildProgress,
    ) -> Result<Self, MediaError> {
        let path = path.as_ref();
        Self::build_with(|| Decoder::open(path), options, progress)
    }

    /// Builds peaks from any media `open` can open, once per parallel task.
    /// Each call must give a decoder on the same media.
    ///
    /// # Errors
    ///
    /// Fails if `open` fails, the media can't be decoded, or the build is
    /// cancelled.
    pub fn build_with(
        open: impl Fn() -> Result<Decoder, MediaError> + Sync,
        options: &BuildOptions,
        progress: &BuildProgress,
    ) -> Result<Self, MediaError> {
        Self::build_from(open()?, open, options, progress)
    }

    /// The peaks of audio already in memory.
    ///
    /// # Errors
    ///
    /// Fails only for audio too long for a peak file (over 2^32 peaks).
    pub fn from_samples(data: &SampleData) -> Result<Self, MediaError> {
        let level0: Level0Peaks = data
            .channels()
            .map(|samples| {
                let mut acc = Level0::with_capacity(samples.len());
                acc.push(samples);
                acc.finish()
            })
            .collect();
        Self::assemble(data.sample_rate(), data.frames() as u64, &level0)
    }

    fn build_from(
        first: Decoder,
        open: impl Fn() -> Result<Decoder, MediaError> + Sync,
        options: &BuildOptions,
        progress: &BuildProgress,
    ) -> Result<Self, MediaError> {
        let info = *first.info();
        progress.start(info.frames);
        let threads = options.threads.map(NonZeroUsize::get);
        let workers = threads.unwrap_or_else(rayon::current_num_threads);
        let segment = info
            .frames
            .unwrap_or(0)
            .div_ceil(SEGMENTS_PER_THREAD * workers as u64)
            .max(options.segment_frames)
            .max(1)
            .div_ceil(FRAMES_PER_PEAK)
            .saturating_mul(FRAMES_PER_PEAK);
        if let Some(frames) = info.frames
            && workers > 1
            && frames > segment
        {
            drop(first);
            let segments = in_pool(threads, || {
                build_segments(&open, info.channels, frames, segment, progress)
            })??;
            if let Some(level0) = segments {
                return Self::assemble(info.sample_rate, frames, &level0);
            }
            // The header's length was wrong. Decode in order instead.
            progress.start(None);
            let (frames, level0) = build_in_order(open()?, progress)?;
            return Self::assemble(info.sample_rate, frames, &level0);
        }
        let (frames, level0) = build_in_order(first, progress)?;
        Self::assemble(info.sample_rate, frames, &level0)
    }
}

/// Runs `f` on a pool of `threads` threads, or on the current pool.
fn in_pool<T: Send>(threads: Option<usize>, f: impl FnOnce() -> T + Send) -> Result<T, MediaError> {
    match threads {
        None => Ok(f()),
        Some(n) => {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .thread_name(|i| format!("bl-peaks-build-{i}"))
                .build()
                .map_err(|e| MediaError::Io(std::io::Error::other(e)))?;
            Ok(pool.install(f))
        }
    }
}

/// Decodes from the decoder's position to the end of the file. Returns the
/// frames decoded and each channel's level-0 peaks.
fn build_in_order(
    mut decoder: Decoder,
    progress: &BuildProgress,
) -> Result<(u64, Level0Peaks), MediaError> {
    let channels = usize::from(decoder.info().channels);
    let expected = decoder.info().frames.unwrap_or(0).min(MAX_RESERVE) as usize;
    let mut acc: Vec<Level0> = (0..channels)
        .map(|_| Level0::with_capacity(expected))
        .collect();
    let mut buffers = vec![vec![0.0f32; BLOCK]; channels];
    let mut outs: Vec<&mut [f32]> = buffers.iter_mut().map(|b| &mut b[..]).collect();
    let mut frames: u64 = 0;
    loop {
        let n = decoder.read(&mut outs)?;
        if n == 0 {
            break;
        }
        for (a, out) in acc.iter_mut().zip(&outs) {
            a.push(&out[..n]);
        }
        frames += n as u64;
        progress.advance(n)?;
    }
    Ok((frames, acc.into_iter().map(Level0::finish).collect()))
}

/// Decodes `frames` frames in segments, in parallel on the current pool.
/// Returns each channel's level-0 peaks, or `None` if a segment found the
/// file a different length from `frames`.
fn build_segments(
    open: &(impl Fn() -> Result<Decoder, MediaError> + Sync),
    channels: u16,
    frames: u64,
    segment: u64,
    progress: &BuildProgress,
) -> Result<Option<Level0Peaks>, MediaError> {
    let channels = usize::from(channels);
    let starts: Vec<u64> = (0..frames.div_ceil(segment)).map(|i| i * segment).collect();
    let results: Vec<Option<Level0Peaks>> = starts
        .par_iter()
        .map_init(
            || (None, vec![vec![0.0f32; BLOCK]; channels]),
            |(slot, buffers), &start| {
                // Each worker opens one decoder and seeks it from segment to
                // segment.
                let mut decoder = match slot.take() {
                    Some(decoder) => decoder,
                    None => open()?,
                };
                let len = segment.min(frames - start);
                let last = start + len == frames;
                let result = build_segment(&mut decoder, buffers, start, len, last, progress);
                *slot = Some(decoder);
                result
            },
        )
        .collect::<Result<_, MediaError>>()?;

    let capacity = frames.min(MAX_RESERVE).div_ceil(FRAMES_PER_PEAK) as usize;
    let mut level0: Level0Peaks = (0..channels)
        .map(|_| Vec::with_capacity(capacity))
        .collect();
    for result in results {
        let Some(peaks) = result else {
            return Ok(None);
        };
        for (all, part) in level0.iter_mut().zip(peaks) {
            all.extend_from_slice(&part);
        }
    }
    Ok(Some(level0))
}

/// Decodes `len` frames from `start`, which is on a peak boundary. Returns
/// `None` if the file ends before `len` frames, or, for the last segment, if
/// it goes on after them.
fn build_segment(
    decoder: &mut Decoder,
    buffers: &mut [Vec<f32>],
    start: u64,
    len: u64,
    last: bool,
    progress: &BuildProgress,
) -> Result<Option<Level0Peaks>, MediaError> {
    if progress.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    // A worker usually takes the segment after its last one, and then it's
    // already there. Not seeking matters for MP3, where a seek scans the
    // file's frames from the start.
    if decoder.position() != start {
        decoder.seek(start)?;
        if decoder.position() != start {
            return Ok(None);
        }
    }
    let mut acc: Vec<Level0> = buffers
        .iter()
        .map(|_| Level0::with_capacity(len.min(MAX_RESERVE) as usize))
        .collect();
    let mut remaining = len;
    while remaining > 0 {
        let want = (remaining.min(BLOCK as u64)) as usize;
        let mut outs: Vec<&mut [f32]> = buffers.iter_mut().map(|b| &mut b[..want]).collect();
        let n = decoder.read(&mut outs)?;
        if n == 0 {
            return Ok(None);
        }
        for (a, out) in acc.iter_mut().zip(&outs) {
            a.push(&out[..n]);
        }
        remaining -= n as u64;
        progress.advance(n)?;
    }
    if last {
        let mut outs: Vec<&mut [f32]> = buffers.iter_mut().map(|b| &mut b[..1]).collect();
        if decoder.read(&mut outs)? != 0 {
            return Ok(None);
        }
    }
    Ok(Some(acc.into_iter().map(Level0::finish).collect()))
}

/// One channel's level-0 peaks, from samples given in blocks of any size.
struct Level0 {
    peaks: Vec<[u8; 2]>,
    /// The window in progress: its extremes so far and how full it is.
    lo: f32,
    hi: f32,
    filled: usize,
}

impl Level0 {
    fn with_capacity(frames: usize) -> Self {
        Self {
            peaks: Vec::with_capacity(frames.div_ceil(WINDOW)),
            lo: f32::INFINITY,
            hi: f32::NEG_INFINITY,
            filled: 0,
        }
    }

    fn push(&mut self, mut samples: &[f32]) {
        if self.filled > 0 {
            let take = (WINDOW - self.filled).min(samples.len());
            let (head, rest) = samples.split_at(take);
            let (lo, hi) = extremes(head);
            self.lo = self.lo.min(lo);
            self.hi = self.hi.max(hi);
            self.filled += take;
            samples = rest;
            if self.filled < WINDOW {
                return;
            }
            self.peaks.push(quantise(self.lo, self.hi));
            self.filled = 0;
        }
        let (windows, rest) = samples.as_chunks::<WINDOW>();
        self.peaks.extend(windows.iter().map(|w| {
            let (lo, hi) = extremes(w);
            quantise(lo, hi)
        }));
        if !rest.is_empty() {
            (self.lo, self.hi) = extremes(rest);
            self.filled = rest.len();
        }
    }

    fn finish(mut self) -> Vec<[u8; 2]> {
        if self.filled > 0 {
            self.peaks.push(quantise(self.lo, self.hi));
        }
        self.peaks
    }
}

/// Lanes for the extremes search: enough for the compiler to keep two
/// 128-bit vectors (NEON, SSE) or one 256-bit vector busy.
const LANES: usize = 8;

/// The lowest and highest sample. NaNs are ignored; for no samples (or only
/// NaNs), the result is (+∞, −∞).
#[inline]
fn extremes(samples: &[f32]) -> (f32, f32) {
    let mut lo = [f32::INFINITY; LANES];
    let mut hi = [f32::NEG_INFINITY; LANES];
    let (chunks, rest) = samples.as_chunks::<LANES>();
    for chunk in chunks {
        for lane in 0..LANES {
            lo[lane] = lo[lane].min(chunk[lane]);
            hi[lane] = hi[lane].max(chunk[lane]);
        }
    }
    for &s in rest {
        lo[0] = lo[0].min(s);
        hi[0] = hi[0].max(s);
    }
    (
        lo.into_iter().fold(f32::INFINITY, f32::min),
        hi.into_iter().fold(f32::NEG_INFINITY, f32::max),
    )
}

/// A window's extremes as a `[min, max]` peak in 1/127 steps: the minimum
/// rounded down and the maximum up, clamped to ±127. A window with nothing
/// in it, or only NaNs, is `[0, 0]`.
fn quantise(lo: f32, hi: f32) -> [u8; 2] {
    if lo > hi || lo.is_nan() || hi.is_nan() {
        return [0, 0];
    }
    let lo = (lo * 127.0).floor().clamp(-127.0, 127.0) as i8;
    let hi = (hi * 127.0).ceil().clamp(-127.0, 127.0) as i8;
    [lo as u8, hi as u8]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn as_i8(peaks: &[[u8; 2]]) -> Vec<[i8; 2]> {
        peaks.iter().map(|p| [p[0] as i8, p[1] as i8]).collect()
    }

    #[test]
    fn quantising_rounds_outwards() {
        // Spec: waveform-peaks R2
        assert_eq!(quantise(0.0, 0.0), [0, 0]);
        assert_eq!(as_i8(&[quantise(-1.0, 1.0)]), [[-127, 127]]);
        // Over full scale clamps.
        assert_eq!(as_i8(&[quantise(-3.0, 2.0)]), [[-127, 127]]);
        // A quiet signal still shows a step either side of zero.
        assert_eq!(as_i8(&[quantise(-1e-4, 1e-4)]), [[-1, 1]]);
        // A positive DC offset keeps its sign.
        assert_eq!(as_i8(&[quantise(0.5, 0.5)]), [[63, 64]]);
        assert_eq!(quantise(f32::INFINITY, f32::NEG_INFINITY), [0, 0]);
        assert_eq!(quantise(f32::NAN, 0.5), [0, 0]);
        assert_eq!(
            as_i8(&[quantise(f32::NEG_INFINITY, f32::INFINITY)]),
            [[-127, 127]]
        );
    }

    #[test]
    fn extremes_find_every_sample_and_skip_nans() {
        let mut samples: Vec<f32> = (0..300).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();
        samples[123] = -0.9;
        samples[299] = 0.8;
        samples[7] = f32::NAN;
        assert_eq!(extremes(&samples), (-0.9, 0.8));
        assert_eq!(extremes(&[]), (f32::INFINITY, f32::NEG_INFINITY));
        assert_eq!(extremes(&[0.25]), (0.25, 0.25));
    }

    #[test]
    fn block_size_doesnt_change_level_0() {
        let samples: Vec<f32> = (0..5_000)
            .map(|i| ((i * 7_919) % 1_000) as f32 / 500.0 - 1.0)
            .collect();
        let mut whole = Level0::with_capacity(samples.len());
        whole.push(&samples);
        let whole = whole.finish();
        assert_eq!(whole.len(), 5_000usize.div_ceil(256));
        for block in [1, 3, 255, 256, 257, 1_000] {
            let mut acc = Level0::with_capacity(0);
            for chunk in samples.chunks(block) {
                acc.push(chunk);
            }
            assert_eq!(acc.finish(), whole, "block size {block}");
        }
    }
}

//! Peaks: multi-resolution min/max overviews of media, for drawing waveforms.
//!
//! A media file's [`Peaks`] hold, for each channel, the lowest and highest
//! sample in every window of [`FRAMES_PER_PEAK`] frames (level 0), and then
//! coarser levels, each merging [`MIP_FACTOR`] peaks of the level below, down
//! to a level of one peak. The UI draws a waveform at any zoom from the level
//! with about one peak per pixel, on the GPU, straight from [`Peaks::data`].
//!
//! - [`Peaks::build`] reads media from the [`MediaStore`] and builds its peaks,
//!   decoding segments of the file in parallel (each segment seeks, which is
//!   sample-accurate, so the result is identical to decoding in order).
//! - [`PeakCache`] builds peaks in the background, keyed by content hash,
//!   keeps them in memory, saves them as peak files in the platform's cache
//!   folder, and drops them when their source changes.
//! - [`Peaks::to_file_bytes`] and [`Peaks::from_file_bytes`] read and write
//!   the peak file format.
//!
//! # The data layout
//!
//! [`Peaks::data`] is the block the waveform shader reads as
//! `array<u32>` in a storage buffer, with no conversion:
//!
//! - A peak is two bytes, `[min, max]`, each an `i8` in -127 to 127 that
//!   stands for `value / 127` of full scale. Two peaks share a `u32` word,
//!   little-endian, so peak `i` is in word `i / 2` at bit `16 * (i % 2)`.
//! - The block is a sequence of runs, one per level and channel, in level
//!   order and, within a level, channel order: level 0 channel 0, level 0
//!   channel 1, ..., level 1 channel 0, and so on. [`Peaks::run`] gives each
//!   run's offset and length, counted in peaks from the start of the block.
//! - Every run starts on a word boundary: a run of odd length is followed by
//!   one zero peak. So the block's length is a multiple of four bytes, and
//!   blocks from several files can be put one after another in one buffer.
//!   Uploaded at byte offset `b` (a multiple of 4), a run's first peak is at
//!   peak index `b / 2 + run.offset`.
//! - Peak `i` of level `k` covers frames `i * 256 * 4^k` up to, not
//!   including, `(i + 1) * 256 * 4^k` (the last peak of a level covers what's
//!   left). Coarse levels sit at the end of the block, so a UI short of GPU
//!   memory can upload the levels from `k` up as one contiguous range.
//!
//! Minimums are rounded down and maximums up to the next 1/127 step, so a
//! window with any signal in it never draws as digital silence, and only a
//! window of digital silence draws as a flat line at zero. The levels above 0
//! are exact: a peak at level `k + 1` is the minimum and maximum of the four
//! peaks under it, which equals rounding the window's own extremes.
//!
//! The format, its rules, and its tests are written up in the waveform peaks
//! spec, the waveform peaks spec.
//!
//! Everything here runs on worker threads: building decodes and allocates,
//! and the cache reads and writes files.
//!
//! [`MediaStore`]: crate::MediaStore

mod build;
mod cache;
mod file;

use std::fmt;
use std::ops::Range;

use crate::error::MediaError;

pub use build::{BuildOptions, BuildProgress};
pub use cache::{
    PeakCache, PeakCacheOptions, PeakEvent, PeakStatus, PruneReport, default_cache_dir,
};
pub use file::{FILE_EXTENSION, FORMAT_VERSION};

/// Frames of the source under one peak at level 0.
pub const FRAMES_PER_PEAK: u64 = WINDOW as u64;

/// How many peaks of one level merge into one peak of the next.
pub const MIP_FACTOR: u64 = 4;

/// [`FRAMES_PER_PEAK`] as a `usize`, for slicing.
const WINDOW: usize = 256;

/// Where one channel's peaks at one level sit in [`Peaks::data`], counted in
/// peaks (two bytes each) from the start of the block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PeakRun {
    /// The first peak.
    pub offset: u32,
    /// The number of peaks.
    pub len: u32,
}

impl PeakRun {
    /// The run's bytes in [`Peaks::data`].
    #[must_use]
    pub fn byte_range(self) -> Range<usize> {
        let start = self.offset as usize * 2;
        start..start + self.len as usize * 2
    }
}

/// The peaks of one piece of media: every channel at every level, in the
/// layout the waveform shader reads (see the [module docs](self)).
#[derive(Clone, PartialEq, Eq)]
pub struct Peaks {
    sample_rate: u32,
    channels: u16,
    frames: u64,
    /// One run per level and channel, level-major.
    runs: Box<[PeakRun]>,
    data: Box<[u8]>,
}

impl fmt::Debug for Peaks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Peaks")
            .field("sample_rate", &self.sample_rate)
            .field("channels", &self.channels)
            .field("frames", &self.frames)
            .field("levels", &self.level_count())
            .field("bytes", &self.data.len())
            .finish_non_exhaustive()
    }
}

impl Peaks {
    /// The source's sample rate. Peaks count frames at this rate.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The number of channels.
    #[must_use]
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// The source's length in frames.
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// The number of levels: level 0 at [`FRAMES_PER_PEAK`] frames per peak,
    /// up to a level with one peak (or none, for an empty source).
    #[must_use]
    pub fn level_count(&self) -> usize {
        self.runs.len() / usize::from(self.channels)
    }

    /// Frames of the source under one peak at a level: 256 × 4^level.
    #[must_use]
    pub fn frames_per_peak(level: usize) -> u64 {
        u32::try_from(level)
            .ok()
            .and_then(|level| MIP_FACTOR.checked_pow(level))
            .and_then(|factor| factor.checked_mul(FRAMES_PER_PEAK))
            .unwrap_or(u64::MAX)
    }

    /// Where one channel's peaks at one level sit in [`Peaks::data`].
    #[must_use]
    pub fn run(&self, level: usize, channel: usize) -> Option<PeakRun> {
        if channel >= usize::from(self.channels) {
            return None;
        }
        self.runs
            .get(level.checked_mul(usize::from(self.channels))? + channel)
            .copied()
    }

    /// Every run, level by level and channel by channel within a level.
    #[must_use]
    pub fn runs(&self) -> &[PeakRun] {
        &self.runs
    }

    /// The peak data, ready to upload to the GPU as is.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// One peak as `[min, max]`, in 1/127 steps of full scale.
    #[must_use]
    pub fn peak(&self, level: usize, channel: usize, index: usize) -> Option<[i8; 2]> {
        let run = self.run(level, channel)?;
        if index >= run.len as usize {
            return None;
        }
        let at = (run.offset as usize + index) * 2;
        Some([self.data[at] as i8, self.data[at + 1] as i8])
    }

    /// One channel's peaks at one level, as `[min, max]` pairs.
    pub fn level(
        &self,
        level: usize,
        channel: usize,
    ) -> Option<impl ExactSizeIterator<Item = [i8; 2]> + '_> {
        let run = self.run(level, channel)?;
        let (pairs, _) = self.data[run.byte_range()].as_chunks::<2>();
        Some(pairs.iter().map(|&[lo, hi]| [lo as i8, hi as i8]))
    }

    /// The level to draw from when one pixel shows `frames_per_pixel` frames
    /// of the source: the coarsest level that still has at least one peak per
    /// pixel, so a pixel column reads one to four peaks. Below
    /// [`FRAMES_PER_PEAK`] frames per pixel, it's level 0, and a view zoomed
    /// in that far draws from the samples instead.
    #[must_use]
    pub fn level_for(&self, frames_per_pixel: f64) -> usize {
        let mut level = 0;
        while level + 1 < self.level_count()
            && Self::frames_per_peak(level + 1) as f64 <= frames_per_pixel
        {
            level += 1;
        }
        level
    }

    /// The lowest and highest peak values over a range of frames, as
    /// `[min, max]`: every level-0 window the range touches counts in full.
    /// Reads at most six peaks per level. `None` for an empty range, or a
    /// channel or range outside the source.
    #[must_use]
    pub fn min_max(&self, channel: usize, frames: Range<u64>) -> Option<[i8; 2]> {
        let run0 = self.run(0, channel)?;
        let end = frames.end.min(self.frames);
        if frames.start >= end {
            return None;
        }
        let first = frames.start / FRAMES_PER_PEAK;
        let end = end.div_ceil(FRAMES_PER_PEAK).min(u64::from(run0.len));
        let (mut a, mut b) = (first as usize, end as usize);
        let mut out = [i8::MAX, i8::MIN];
        let mut take = |level: usize, index: usize| {
            if let Some([lo, hi]) = self.peak(level, channel, index) {
                out = [out[0].min(lo), out[1].max(hi)];
            }
        };
        // Peaks at the ends that don't fill a group of four are read at this
        // level; whole groups are read from the level above.
        let factor = MIP_FACTOR as usize;
        let mut level = 0;
        while a < b {
            while a < b && a % factor != 0 {
                take(level, a);
                a += 1;
            }
            while a < b && b % factor != 0 {
                b -= 1;
                take(level, b);
            }
            a /= factor;
            b /= factor;
            level += 1;
        }
        Some(out)
    }

    /// The size of [`Peaks::data`] in bytes.
    #[must_use]
    pub fn byte_size(&self) -> usize {
        self.data.len()
    }

    /// Peaks from level-0 pairs, one list per channel, by computing the levels
    /// above and laying everything out.
    fn assemble(
        sample_rate: u32,
        frames: u64,
        level0: &[Vec<[u8; 2]>],
    ) -> Result<Self, MediaError> {
        let channels = u16::try_from(level0.len())
            .ok()
            .filter(|&c| c > 0)
            .ok_or_else(|| MediaError::Malformed(format!("{} channels", level0.len())))?;
        let layout = Layout::new(frames, channels)?;
        let mut data = vec![0u8; layout.data_len].into_boxed_slice();

        for (peaks, run) in level0.iter().zip(&layout.runs) {
            if peaks.len() != run.len as usize {
                return Err(MediaError::Malformed(format!(
                    "{} peaks for {frames} frames",
                    peaks.len()
                )));
            }
            data[run.byte_range()].copy_from_slice(peaks.as_flattened());
        }
        let per_level = usize::from(channels);
        for level in 1..layout.levels {
            for channel in 0..per_level {
                let below = layout.runs[(level - 1) * per_level + channel];
                let here = layout.runs[level * per_level + channel];
                // Runs are in level order, so every run of the level below
                // ends before this one starts.
                let (front, back) = data.split_at_mut(here.byte_range().start);
                reduce(
                    &front[below.byte_range()],
                    &mut back[..here.len as usize * 2],
                );
            }
        }
        Ok(Self {
            sample_rate,
            channels,
            frames,
            runs: layout.runs.into_boxed_slice(),
            data,
        })
    }
}

/// Where everything goes in the data block, for a length and channel count.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Layout {
    levels: usize,
    runs: Vec<PeakRun>,
    data_len: usize,
}

impl Layout {
    fn new(frames: u64, channels: u16) -> Result<Self, MediaError> {
        let too_long = || {
            MediaError::Unsupported(format!(
                "{frames} frames of {channels} channels is too long for a peak file"
            ))
        };
        let mut lengths = vec![frames.div_ceil(FRAMES_PER_PEAK)];
        while let Some(&last) = lengths.last()
            && last > 1
        {
            lengths.push(last.div_ceil(MIP_FACTOR));
        }
        let mut runs = Vec::with_capacity(lengths.len() * usize::from(channels));
        let mut offset: u64 = 0;
        for &len in &lengths {
            for _ in 0..channels {
                let len32 = u32::try_from(len).map_err(|_| too_long())?;
                let offset32 = u32::try_from(offset).map_err(|_| too_long())?;
                runs.push(PeakRun {
                    offset: offset32,
                    len: len32,
                });
                // Each run starts on a four-byte word.
                offset += len + (len & 1);
            }
        }
        if offset > u64::from(u32::MAX) {
            return Err(too_long());
        }
        Ok(Self {
            levels: lengths.len(),
            runs,
            data_len: usize::try_from(offset * 2).map_err(|_| too_long())?,
        })
    }
}

/// Merges groups of [`MIP_FACTOR`] peaks into one: the lowest minimum and the
/// highest maximum. Both slices hold `[min, max]` byte pairs.
fn reduce(below: &[u8], here: &mut [u8]) {
    let (below, _) = below.as_chunks::<2>();
    let (here, _) = here.as_chunks_mut::<2>();
    for (group, out) in below.chunks(MIP_FACTOR as usize).zip(here) {
        let mut lo = i8::MAX;
        let mut hi = i8::MIN;
        for &[min, max] in group {
            lo = lo.min(min as i8);
            hi = hi.max(max as i8);
        }
        *out = [lo as u8, hi as u8];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(values: &[[i8; 2]]) -> Vec<[u8; 2]> {
        values
            .iter()
            .map(|&[lo, hi]| [lo as u8, hi as u8])
            .collect()
    }

    #[test]
    fn levels_run_down_to_one_peak() {
        assert_eq!(Layout::new(0, 1).unwrap().levels, 1);
        assert_eq!(Layout::new(1, 1).unwrap().levels, 1);
        assert_eq!(Layout::new(256, 1).unwrap().levels, 1);
        assert_eq!(Layout::new(257, 1).unwrap().levels, 2);
        // 2 peaks, then 1.
        assert_eq!(Layout::new(512, 1).unwrap().levels, 2);
        // 5 peaks, 2, 1.
        assert_eq!(Layout::new(1_280, 1).unwrap().levels, 3);
        // An hour at 48 kHz: 675,000 peaks, then ten more levels.
        assert_eq!(Layout::new(172_800_000, 2).unwrap().levels, 11);
    }

    #[test]
    fn runs_are_level_major_and_word_aligned() {
        // Spec: waveform-peaks R8
        // 5 peaks per channel at level 0, then 2, then 1.
        let layout = Layout::new(1_280, 2).unwrap();
        let expected = [(0, 5), (6, 5), (12, 2), (14, 2), (16, 1), (18, 1)]
            .map(|(offset, len)| PeakRun { offset, len });
        assert_eq!(layout.runs, expected);
        assert_eq!(layout.data_len, 40);
        for run in &layout.runs {
            assert_eq!(run.offset % 2, 0, "a run starts mid-word");
        }
    }

    #[test]
    fn a_file_too_long_for_u32_offsets_is_refused() {
        // 2^32 peaks at level 0 alone.
        let frames = (1u64 << 32) * FRAMES_PER_PEAK;
        assert!(matches!(
            Layout::new(frames, 1),
            Err(MediaError::Unsupported(_))
        ));
    }

    #[test]
    fn levels_above_zero_merge_four_peaks() {
        // Spec: waveform-peaks R2
        let level0 = pairs(&[[-1, 1], [-5, 2], [0, 0], [-2, 9], [-127, -100]]);
        let peaks = Peaks::assemble(48_000, 1_280, &[level0]).unwrap();
        assert_eq!(peaks.level_count(), 3);
        assert_eq!(
            peaks.level(1, 0).unwrap().collect::<Vec<_>>(),
            [[-5, 9], [-127, -100]]
        );
        assert_eq!(peaks.level(2, 0).unwrap().collect::<Vec<_>>(), [[-127, 9]]);
        // The padding peak after each odd run is zero.
        assert_eq!(&peaks.data()[10..12], &[0, 0]);
    }

    #[test]
    fn min_max_reads_whole_groups_from_coarser_levels() {
        // 64 level-0 peaks: peak i is [-i, i].
        let level0: Vec<[i8; 2]> = (0..64).map(|i| [-(i as i8), i as i8]).collect();
        let peaks = Peaks::assemble(48_000, 64 * FRAMES_PER_PEAK, &[pairs(&level0)]).unwrap();
        let brute = |a: u64, b: u64| {
            let first = a / FRAMES_PER_PEAK;
            let end = b.div_ceil(FRAMES_PER_PEAK);
            level0[first as usize..end as usize]
                .iter()
                .fold([i8::MAX, i8::MIN], |m, p| [m[0].min(p[0]), m[1].max(p[1])])
        };
        let total = 64 * FRAMES_PER_PEAK;
        for a in (0..total).step_by(97) {
            for b in (a + 1..=total).step_by(331) {
                assert_eq!(peaks.min_max(0, a..b), Some(brute(a, b)), "{a}..{b}");
            }
        }
        assert_eq!(peaks.min_max(0, 10..10), None);
        assert_eq!(peaks.min_max(1, 0..10), None);
        assert_eq!(peaks.min_max(0, total..total + 5), None);
    }

    #[test]
    fn level_for_keeps_at_least_one_peak_per_pixel() {
        let level0 = vec![[0u8, 0]; 4_096];
        let peaks = Peaks::assemble(48_000, 4_096 * FRAMES_PER_PEAK, &[level0]).unwrap();
        assert_eq!(peaks.level_count(), 7);
        assert_eq!(peaks.level_for(1.0), 0);
        assert_eq!(peaks.level_for(255.0), 0);
        assert_eq!(peaks.level_for(1_023.9), 0);
        assert_eq!(peaks.level_for(1_024.0), 1);
        assert_eq!(peaks.level_for(4_096.0), 2);
        assert_eq!(peaks.level_for(1e12), 6);
        for k in 0..peaks.level_count() {
            assert_eq!(peaks.level_for(Peaks::frames_per_peak(k) as f64), k);
        }
    }
}

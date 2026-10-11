//! A planar audio buffer that sources write into.

use core::ops::Range;

/// Planar audio to write into: `channels` channels of `frames` frames, where
/// channel `c` starts at `data[c * stride]`. It borrows the samples, so
/// making one allocates nothing, and it works over an engine block's buffer
/// or a node's own scratch buffer alike.
#[derive(Debug)]
pub struct PlanarMut<'a> {
    data: &'a mut [f32],
    channels: usize,
    frames: usize,
    stride: usize,
}

impl<'a> PlanarMut<'a> {
    /// A view of `channels` channels of `frames` frames in `data`, channel
    /// `c` starting at `data[c * stride]`.
    ///
    /// # Panics
    ///
    /// If `frames` is larger than `stride`, or `data` is too short.
    #[must_use]
    pub fn new(data: &'a mut [f32], channels: usize, frames: usize, stride: usize) -> Self {
        assert!(
            frames <= stride,
            "{frames} frames don't fit a stride of {stride}"
        );
        assert!(
            channels == 0 || data.len() >= (channels - 1) * stride + frames,
            "{channels} channels of {frames} frames don't fit {} samples",
            data.len()
        );
        Self {
            data,
            channels,
            frames,
            stride,
        }
    }

    /// The number of channels.
    #[must_use]
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// The number of frames.
    #[must_use]
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// One channel's samples.
    ///
    /// # Panics
    ///
    /// If `index` isn't less than [`channels`](Self::channels).
    #[must_use]
    pub fn channel(&self, index: usize) -> &[f32] {
        assert!(index < self.channels, "channel {index} out of range");
        let start = index * self.stride;
        &self.data[start..start + self.frames]
    }

    /// One channel's samples, writable.
    ///
    /// # Panics
    ///
    /// If `index` isn't less than [`channels`](Self::channels).
    pub fn channel_mut(&mut self, index: usize) -> &mut [f32] {
        assert!(index < self.channels, "channel {index} out of range");
        let start = index * self.stride;
        &mut self.data[start..start + self.frames]
    }

    /// Sets every sample to `value`.
    pub fn fill(&mut self, value: f32) {
        for c in 0..self.channels {
            self.channel_mut(c).fill(value);
        }
    }

    /// Part of one channel.
    pub(crate) fn span_mut(&mut self, channel: usize, frames: Range<usize>) -> &mut [f32] {
        &mut self.channel_mut(channel)[frames]
    }

    /// Silences `frames` in each of `channels`.
    pub(crate) fn silence(&mut self, channels: Range<usize>, frames: Range<usize>) {
        for c in channels {
            self.span_mut(c, frames.clone()).fill(0.0);
        }
    }
}

/// Copies `frames` frames from planar `src` (channel `c` at
/// `src[c * stride + from]`) into `out` at frame `at`, for the first
/// `channels` channels.
pub(crate) fn copy_planar(
    src: &[f32],
    stride: usize,
    from: usize,
    out: &mut PlanarMut<'_>,
    at: usize,
    frames: usize,
    channels: usize,
) {
    for c in 0..channels {
        let start = c * stride + from;
        out.span_mut(c, at..at + frames)
            .copy_from_slice(&src[start..start + frames]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_shows_its_channels_and_leaves_the_rest_alone() {
        let mut data = vec![0.0f32; 2 * 8];
        let mut view = PlanarMut::new(&mut data, 2, 5, 8);
        view.channel_mut(1).fill(1.0);
        assert_eq!(view.channel(0), &[0.0; 5]);
        assert_eq!(view.channel(1), &[1.0; 5]);
        view.silence(1..2, 1..3);
        assert_eq!(view.channel(1), &[1.0, 0.0, 0.0, 1.0, 1.0]);
        assert_eq!(&data[13..], &[0.0; 3], "frames past the view are untouched");
    }

    #[test]
    #[should_panic(expected = "don't fit")]
    fn a_view_needs_room_for_its_frames() {
        let mut data = vec![0.0f32; 9];
        let _ = PlanarMut::new(&mut data, 2, 5, 5);
    }
}

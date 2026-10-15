//! Planar `f32` buffers: one contiguous slice per channel.
//!
//! The views borrow storage that a backend allocates when a stream opens, so
//! making one on the audio thread never allocates.

/// Read-only planar audio for one block.
#[derive(Debug)]
pub struct AudioBuffer<'a> {
    data: &'a [f32],
    channels: usize,
    frames: usize,
    stride: usize,
}

impl<'a> AudioBuffer<'a> {
    /// A view of `channels` channels of `frames` frames, where channel `c`
    /// starts at `data[c * stride]`.
    ///
    /// # Panics
    ///
    /// If `frames` is larger than `stride`, or `data` is too short.
    pub fn new(data: &'a [f32], channels: usize, frames: usize, stride: usize) -> Self {
        check_layout(data.len(), channels, frames, stride);
        Self {
            data,
            channels,
            frames,
            stride,
        }
    }

    /// A buffer with no channels, for streams without input.
    pub fn empty(frames: usize) -> AudioBuffer<'static> {
        AudioBuffer {
            data: &[],
            channels: 0,
            frames,
            stride: frames,
        }
    }

    /// The number of channels.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// The number of frames in each channel.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// One channel's samples.
    ///
    /// # Panics
    ///
    /// If `index` isn't less than [`channels`](Self::channels).
    pub fn channel(&self, index: usize) -> &[f32] {
        assert!(index < self.channels, "channel {index} out of range");
        let start = index * self.stride;
        &self.data[start..start + self.frames]
    }

    /// Every channel's samples, in order.
    pub fn iter(&self) -> impl Iterator<Item = &[f32]> {
        (0..self.channels).map(|index| self.channel(index))
    }
}

/// Writable planar audio for one block.
#[derive(Debug)]
pub struct AudioBufferMut<'a> {
    data: &'a mut [f32],
    channels: usize,
    frames: usize,
    stride: usize,
}

impl<'a> AudioBufferMut<'a> {
    /// A view of `channels` channels of `frames` frames, where channel `c`
    /// starts at `data[c * stride]`.
    ///
    /// # Panics
    ///
    /// If `frames` is larger than `stride`, or `data` is too short.
    pub fn new(data: &'a mut [f32], channels: usize, frames: usize, stride: usize) -> Self {
        check_layout(data.len(), channels, frames, stride);
        Self {
            data,
            channels,
            frames,
            stride,
        }
    }

    /// The number of channels.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// The number of frames in each channel.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// One channel's samples.
    ///
    /// # Panics
    ///
    /// If `index` isn't less than [`channels`](Self::channels).
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

    /// Every channel's samples, writable, in order.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut [f32]> {
        let frames = self.frames;
        self.data
            .chunks_mut(self.stride.max(1))
            .take(self.channels)
            .map(move |channel| &mut channel[..frames])
    }

    /// Sets every sample to `value`.
    pub fn fill(&mut self, value: f32) {
        for channel in self.iter_mut() {
            channel.fill(value);
        }
    }

    /// A read-only view of the same samples.
    pub fn as_buffer(&self) -> AudioBuffer<'_> {
        AudioBuffer {
            data: &*self.data,
            channels: self.channels,
            frames: self.frames,
            stride: self.stride,
        }
    }
}

fn check_layout(len: usize, channels: usize, frames: usize, stride: usize) {
    assert!(
        frames <= stride,
        "{frames} frames don't fit a stride of {stride}"
    );
    if channels > 0 {
        let needed = (channels - 1) * stride + frames;
        assert!(
            len >= needed,
            "{channels} channels of {frames} frames need {needed} samples, not {len}"
        );
    }
}

/// Owned planar storage for one direction of a stream, sized when the stream
/// opens.
#[derive(Debug)]
pub(crate) struct PlanarScratch {
    data: Vec<f32>,
    channels: usize,
    capacity: usize,
}

impl PlanarScratch {
    /// Storage for `channels` channels of up to `capacity` frames.
    pub(crate) fn new(channels: usize, capacity: usize) -> Self {
        Self {
            data: vec![0.0; channels * capacity],
            channels,
            capacity,
        }
    }

    /// The most frames a view can have.
    pub(crate) fn capacity(&self) -> usize {
        self.capacity
    }

    /// How many channels each view has.
    pub(crate) fn channels(&self) -> usize {
        self.channels
    }

    /// The first `frames` frames of every channel.
    ///
    /// # Panics
    ///
    /// If `frames` is larger than [`capacity`](Self::capacity).
    pub(crate) fn view(&self, frames: usize) -> AudioBuffer<'_> {
        AudioBuffer::new(&self.data, self.channels, frames, self.capacity)
    }

    /// The first `frames` frames of every channel, writable.
    ///
    /// # Panics
    ///
    /// If `frames` is larger than [`capacity`](Self::capacity).
    pub(crate) fn view_mut(&mut self, frames: usize) -> AudioBufferMut<'_> {
        AudioBufferMut::new(&mut self.data, self.channels, frames, self.capacity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_are_strided_slices() {
        let mut scratch = PlanarScratch::new(2, 8);
        {
            let mut view = scratch.view_mut(3);
            assert_eq!((view.channels(), view.frames()), (2, 3));
            view.channel_mut(0).copy_from_slice(&[1.0, 2.0, 3.0]);
            view.channel_mut(1).copy_from_slice(&[4.0, 5.0, 6.0]);
        }
        let view = scratch.view(3);
        assert_eq!(view.channel(0), &[1.0, 2.0, 3.0]);
        assert_eq!(view.channel(1), &[4.0, 5.0, 6.0]);
        assert_eq!(view.iter().count(), 2);
        assert_eq!(scratch.data[8..11], [4.0, 5.0, 6.0]);
    }

    #[test]
    fn iter_mut_and_fill_cover_every_channel() {
        let mut scratch = PlanarScratch::new(3, 4);
        let mut view = scratch.view_mut(2);
        for (index, channel) in view.iter_mut().enumerate() {
            channel.fill(index as f32);
        }
        assert_eq!(view.as_buffer().channel(2), &[2.0, 2.0]);
        view.fill(0.5);
        assert!(view.as_buffer().iter().all(|c| c == [0.5, 0.5]));
        // Frames past the view are untouched.
        assert_eq!(scratch.data[2..4], [0.0, 0.0]);
    }

    #[test]
    fn empty_buffers_have_frames_but_no_channels() {
        let empty = AudioBuffer::empty(128);
        assert_eq!((empty.channels(), empty.frames()), (0, 128));
        assert_eq!(empty.iter().count(), 0);

        let mut none = PlanarScratch::new(0, 64);
        let mut view = none.view_mut(64);
        assert_eq!(view.iter_mut().count(), 0);
        view.fill(1.0);
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn a_missing_channel_panics() {
        let scratch = PlanarScratch::new(1, 4);
        let _ = scratch.view(4).channel(1);
    }

    #[test]
    #[should_panic(expected = "don't fit")]
    fn a_view_larger_than_the_capacity_panics() {
        let scratch = PlanarScratch::new(1, 4);
        let _ = scratch.view(5);
    }
}

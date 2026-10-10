//! Decoded audio held in memory.

use std::fmt;

use crate::error::MediaError;

/// Decoded audio: one slice of 32-bit float samples per channel (planar), at
/// one sample rate.
///
/// Integer sources map to [-1, 1) exactly: a 16-bit sample `s` decodes to
/// `s / 32768` and a 24-bit one to `s / 8388608`, both exact in `f32`. Lossy
/// codecs can decode past ±1.0, and those samples are kept, not clipped.
///
/// The engine receives this through an `Arc` from the media store. Reading it
/// is real-time safe; building one isn't, so that happens off the audio thread.
#[derive(Clone, PartialEq)]
pub struct SampleData {
    sample_rate: u32,
    frames: usize,
    channels: Box<[Box<[f32]>]>,
}

impl SampleData {
    /// Audio from planar channels at a sample rate.
    ///
    /// # Errors
    ///
    /// Fails if the rate is zero, there are no channels, or the channels
    /// differ in length.
    pub fn new(sample_rate: u32, channels: Vec<Vec<f32>>) -> Result<Self, MediaError> {
        if sample_rate == 0 {
            return Err(MediaError::Malformed("a sample rate of zero".into()));
        }
        let Some(first) = channels.first() else {
            return Err(MediaError::Malformed("audio with no channels".into()));
        };
        let frames = first.len();
        if channels.iter().any(|c| c.len() != frames) {
            return Err(MediaError::Malformed(
                "channels of different lengths".into(),
            ));
        }
        Ok(Self {
            sample_rate,
            frames,
            channels: channels.into_iter().map(Vec::into_boxed_slice).collect(),
        })
    }

    /// Frames per second.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The length in frames (samples per channel).
    #[must_use]
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// The number of channels.
    #[must_use]
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// One channel's samples, or `None` past the last channel.
    #[must_use]
    pub fn channel(&self, index: usize) -> Option<&[f32]> {
        self.channels.get(index).map(|c| &c[..])
    }

    /// Every channel's samples, in channel order.
    pub fn channels(&self) -> impl ExactSizeIterator<Item = &[f32]> + '_ {
        self.channels.iter().map(|c| &c[..])
    }

    /// The length in seconds.
    #[must_use]
    pub fn duration_seconds(&self) -> f64 {
        self.frames as f64 / f64::from(self.sample_rate)
    }

    /// The memory the samples take, in bytes.
    #[must_use]
    pub fn byte_size(&self) -> usize {
        self.frames * self.channels.len() * size_of::<f32>()
    }

    /// The channels, consumed.
    #[must_use]
    pub fn into_channels(self) -> Vec<Vec<f32>> {
        self.channels.into_iter().map(Vec::from).collect()
    }
}

impl fmt::Debug for SampleData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SampleData")
            .field("sample_rate", &self.sample_rate)
            .field("frames", &self.frames)
            .field("channels", &self.channels.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_shapes() {
        assert!(SampleData::new(0, vec![vec![0.0]]).is_err());
        assert!(SampleData::new(48_000, vec![]).is_err());
        assert!(SampleData::new(48_000, vec![vec![0.0; 3], vec![0.0; 2]]).is_err());
    }

    #[test]
    fn reports_its_shape() {
        let data = SampleData::new(48_000, vec![vec![0.5; 4_800], vec![-0.5; 4_800]]).unwrap();
        assert_eq!(data.frames(), 4_800);
        assert_eq!(data.channel_count(), 2);
        assert_eq!(data.channel(1).unwrap()[0], -0.5);
        assert!(data.channel(2).is_none());
        assert!((data.duration_seconds() - 0.1).abs() < 1e-12);
        assert_eq!(data.byte_size(), 4_800 * 2 * 4);
    }
}

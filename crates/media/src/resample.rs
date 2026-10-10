//! Sample-rate conversion.
//!
//! Built on rubato, pinned to an exact version. For the common rates, whose
//! ratio reduces to small integers (44.1 to 48 kHz is 147 to 160), the
//! synchronous FFT resampler converts with a long anti-aliasing filter
//! (Blackman-Harris squared, about 1,024 taps per sub-block). Rates whose
//! ratio doesn't reduce that far would need an impractically large FFT, so
//! they use rubato's asynchronous sinc resampler with a 512-tap filter.
//!
//! Aliasing and imaging stay below -100 dB in the passband, which this module
//! defines as 0 Hz to 90% of the lower Nyquist frequency (19.8 kHz between
//! 44.1 and 48 kHz). `crates/media/tests/resample.rs` measures it.
//!
//! [`RateConverter::process`] converts a chunk at a time into buffers it owns,
//! with no allocation after construction, for disk streaming (WP-1.3) on its
//! worker thread. [`convert_sample_rate`] converts a whole buffer.

use rubato::audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{
    Async, Fft, FixedAsync, FixedSync, Indexing, Resampler, SincInterpolationParameters,
    SincInterpolationType, WindowFunction,
};

use crate::error::MediaError;
use crate::sample_data::SampleData;

/// Input frames per chunk.
const CHUNK: usize = 4096;
/// FFT sub-blocks per chunk: about 1,024 frames each, which sets the filter's
/// length and so its transition width.
const SUB_CHUNKS: usize = 4;
/// The largest reduced rate (rate / gcd) on either side that the FFT
/// resampler takes; past it, the sinc resampler converts instead.
const MAX_FFT_BLOCK: u32 = 8192;
/// The sinc resampler's filter length, for the rates the FFT can't take.
const SINC_LEN: usize = 512;

/// Converts audio from one sample rate to another, a chunk at a time.
///
/// Building one allocates and plans FFTs, so build it before the audio
/// starts and reuse it; [`RateConverter::process`] then neither allocates
/// nor locks.
pub struct RateConverter {
    inner: Box<dyn Resampler<f32>>,
    from: u32,
    to: u32,
    channels: usize,
    /// Owned staging buffers, so a chunk needs no allocation.
    input: Vec<Vec<f32>>,
    output: Vec<Vec<f32>>,
}

impl RateConverter {
    /// A converter from one rate to another for this many channels.
    ///
    /// # Errors
    ///
    /// Fails if a rate or the channel count is zero.
    pub fn new(from: u32, to: u32, channels: usize) -> Result<Self, MediaError> {
        if from == 0 || to == 0 || channels == 0 {
            return Err(MediaError::Resample(format!(
                "can't convert {channels} channels from {from} Hz to {to} Hz"
            )));
        }
        let divisor = gcd(from, to);
        let inner: Box<dyn Resampler<f32>> =
            if from / divisor <= MAX_FFT_BLOCK && to / divisor <= MAX_FFT_BLOCK {
                Box::new(
                    Fft::<f32>::new_custom(
                        from as usize,
                        to as usize,
                        CHUNK,
                        SUB_CHUNKS,
                        channels,
                        WindowFunction::BlackmanHarris2,
                        FixedSync::Input,
                    )
                    .map_err(|e| MediaError::Resample(e.to_string()))?,
                )
            } else {
                let parameters =
                    SincInterpolationParameters::new(SINC_LEN, WindowFunction::BlackmanHarris2)
                        .oversampling_factor(512)
                        .interpolation(SincInterpolationType::Cubic);
                Box::new(
                    Async::<f32>::new_sinc(
                        f64::from(to) / f64::from(from),
                        1.0,
                        &parameters,
                        CHUNK,
                        channels,
                        FixedAsync::Input,
                    )
                    .map_err(|e| MediaError::Resample(e.to_string()))?,
                )
            };
        let input = vec![vec![0.0; inner.input_frames_max()]; channels];
        let output = vec![vec![0.0; inner.output_frames_max()]; channels];
        Ok(Self {
            inner,
            from,
            to,
            channels,
            input,
            output,
        })
    }

    /// The input rate.
    #[must_use]
    pub fn from_rate(&self) -> u32 {
        self.from
    }

    /// The output rate.
    #[must_use]
    pub fn to_rate(&self) -> u32 {
        self.to
    }

    /// The number of channels.
    #[must_use]
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// The input frames the next [`RateConverter::process`] call consumes.
    #[must_use]
    pub fn input_frames_next(&self) -> usize {
        self.inner.input_frames_next()
    }

    /// The most output frames one call can write.
    #[must_use]
    pub fn output_frames_max(&self) -> usize {
        self.inner.output_frames_max()
    }

    /// How many output frames late the converter's output is: an input event
    /// at frame 0 appears at this output frame.
    #[must_use]
    pub fn output_delay(&self) -> usize {
        self.inner.output_delay()
    }

    /// Clears the converter's history, to start a new stream.
    pub fn reset(&mut self) {
        self.inner.reset();
    }

    /// Converts one chunk without allocating. `input` is one slice per
    /// channel holding [`RateConverter::input_frames_next`] frames, or fewer
    /// at the end of the audio, which the converter pads with silence.
    /// `output` is one slice per channel with room for
    /// [`RateConverter::output_frames_max`] frames. Returns the input frames
    /// consumed and the output frames written.
    ///
    /// # Errors
    ///
    /// Fails if the channel counts don't match or the output is too short.
    pub fn process(
        &mut self,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
    ) -> Result<(usize, usize), MediaError> {
        if input.len() != self.channels || output.len() != self.channels {
            return Err(MediaError::Resample(format!(
                "{} input and {} output channels for a {}-channel converter",
                input.len(),
                output.len(),
                self.channels
            )));
        }
        let needed = self.inner.input_frames_next();
        let available = input.iter().map(|c| c.len()).min().unwrap_or(0).min(needed);
        let room = output.iter().map(|c| c.len()).min().unwrap_or(0);
        if room < self.inner.output_frames_next() {
            return Err(MediaError::Resample(format!(
                "output room for {room} frames; the chunk needs {}",
                self.inner.output_frames_next()
            )));
        }
        for (staged, src) in self.input.iter_mut().zip(input) {
            staged[..available].copy_from_slice(&src[..available]);
        }
        let indexing = Indexing {
            input_offset: 0,
            output_offset: 0,
            partial_len: (available < needed).then_some(available),
            active_channels_mask: None,
        };
        let frames_in = self.input[0].len();
        let frames_out = self.output[0].len();
        let adapter_in = SequentialSliceOfVecs::new(&self.input[..], self.channels, frames_in)
            .map_err(|e| MediaError::Resample(e.to_string()))?;
        let mut adapter_out =
            SequentialSliceOfVecs::new_mut(&mut self.output[..], self.channels, frames_out)
                .map_err(|e| MediaError::Resample(e.to_string()))?;
        let (_, written) = self
            .inner
            .process_into_buffer(&adapter_in, &mut adapter_out, Some(&indexing))
            .map_err(|e| MediaError::Resample(e.to_string()))?;
        for (dst, staged) in output.iter_mut().zip(&self.output) {
            dst[..written].copy_from_slice(&staged[..written]);
        }
        Ok((available, written))
    }

    /// Converts a whole buffer, from the start of a fresh stream. The output
    /// lines up with the input (the converter's delay is removed) and has
    /// `ceil(frames × to ÷ from)` frames.
    ///
    /// # Errors
    ///
    /// Fails if the buffer's rate or channel count isn't the converter's.
    pub fn convert(&mut self, data: &SampleData) -> Result<SampleData, MediaError> {
        if data.sample_rate() != self.from || data.channel_count() != self.channels {
            return Err(MediaError::Resample(format!(
                "a {}-channel {} Hz buffer for a {}-channel {} Hz converter",
                data.channel_count(),
                data.sample_rate(),
                self.channels,
                self.from
            )));
        }
        self.reset();
        let frames = data.frames();
        let expected = converted_len(frames, self.from, self.to);
        let mut skip = self.output_delay();
        let mut planes: Vec<Vec<f32>> = (0..self.channels)
            .map(|_| Vec::with_capacity(expected))
            .collect();
        let mut chunk: Vec<Vec<f32>> = vec![vec![0.0; self.output_frames_max()]; self.channels];
        let mut position = 0;

        while planes[0].len() < expected {
            let take = self.input_frames_next().min(frames - position);
            let input: Vec<&[f32]> = data
                .channels()
                .map(|c| &c[position..position + take])
                .collect();
            let mut output: Vec<&mut [f32]> = chunk.iter_mut().map(|c| &mut c[..]).collect();
            let (_, written) = self.process(&input, &mut output)?;
            position += take;

            let dropped = skip.min(written);
            skip -= dropped;
            let keep = (written - dropped).min(expected - planes[0].len());
            for (plane, src) in planes.iter_mut().zip(&chunk) {
                plane.extend_from_slice(&src[dropped..dropped + keep]);
            }
        }
        SampleData::new(self.to, planes)
    }
}

impl std::fmt::Debug for RateConverter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RateConverter")
            .field("from", &self.from)
            .field("to", &self.to)
            .field("channels", &self.channels)
            .finish_non_exhaustive()
    }
}

/// Converts a whole buffer to another sample rate. A buffer already at the
/// rate comes back as a copy.
///
/// # Errors
///
/// Fails if the rate is zero.
pub fn convert_sample_rate(data: &SampleData, rate: u32) -> Result<SampleData, MediaError> {
    if data.sample_rate() == rate {
        return Ok(data.clone());
    }
    RateConverter::new(data.sample_rate(), rate, data.channel_count())?.convert(data)
}

/// The length of `frames` converted from `from` to `to`, rounded up.
#[must_use]
pub fn converted_len(frames: usize, from: u32, to: u32) -> usize {
    let scaled = frames as u128 * u128::from(to);
    scaled.div_ceil(u128::from(from)) as usize
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_round_up() {
        assert_eq!(converted_len(44_100, 44_100, 48_000), 48_000);
        assert_eq!(converted_len(1, 48_000, 44_100), 1);
        assert_eq!(converted_len(0, 48_000, 44_100), 0);
        assert_eq!(converted_len(147, 48_000, 44_100), 136);
    }

    #[test]
    fn rejects_zero_rates_and_channels() {
        assert!(RateConverter::new(0, 48_000, 2).is_err());
        assert!(RateConverter::new(48_000, 0, 2).is_err());
        assert!(RateConverter::new(44_100, 48_000, 0).is_err());
    }

    #[test]
    fn same_rate_is_a_copy() {
        let data = SampleData::new(48_000, vec![vec![0.25; 10]]).unwrap();
        assert_eq!(convert_sample_rate(&data, 48_000).unwrap(), data);
    }

    #[test]
    fn converts_lengths_and_keeps_dc() {
        for (from, to) in [(44_100, 48_000), (48_000, 44_100), (44_100, 44_101)] {
            let data = SampleData::new(from, vec![vec![0.5; 20_000], vec![-0.25; 20_000]]).unwrap();
            let out = convert_sample_rate(&data, to).unwrap();
            assert_eq!(out.sample_rate(), to);
            assert_eq!(out.frames(), converted_len(20_000, from, to));
            // Away from the edges, a constant stays constant.
            let middle = out.frames() / 2;
            assert!((out.channel(0).unwrap()[middle] - 0.5).abs() < 1e-4);
            assert!((out.channel(1).unwrap()[middle] + 0.25).abs() < 1e-4);
        }
    }

    #[test]
    fn chunks_of_any_size_match_the_whole_buffer_conversion() {
        let frames = 30_000;
        let signal: Vec<f32> = (0..frames).map(|n| (n as f32 * 0.01).sin() * 0.5).collect();
        let data = SampleData::new(48_000, vec![signal.clone()]).unwrap();
        let whole = convert_sample_rate(&data, 44_100).unwrap();

        let mut converter = RateConverter::new(48_000, 44_100, 1).unwrap();
        let mut out = Vec::new();
        let mut buffer = vec![0.0; converter.output_frames_max()];
        let mut position = 0;
        while position < frames {
            let take = converter.input_frames_next().min(frames - position);
            let (used, written) = converter
                .process(
                    &[&signal[position..position + take]],
                    &mut [&mut buffer[..]],
                )
                .unwrap();
            assert_eq!(used, take);
            out.extend_from_slice(&buffer[..written]);
            position += take;
        }
        let delay = converter.output_delay();
        for (a, b) in whole.channel(0).unwrap().iter().zip(&out[delay..]) {
            assert!((a - b).abs() < 1e-6);
        }
    }
}

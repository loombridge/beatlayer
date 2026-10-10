//! A file's audio at the engine's rate from any frame: the decoder, and a
//! rate converter when the file's rate differs. Runs on the disk thread,
//! and on the opening thread for a stream's preloaded start.

use crate::decode::Decoder;
use crate::error::MediaError;
use crate::resample::RateConverter;

/// Decoded, converted audio, read forward in chunks, with seeking.
pub(crate) struct Feed {
    decoder: Decoder,
    channels: usize,
    /// The file's rate.
    from: u32,
    /// The engine's rate.
    to: u32,
    /// The length at the engine's rate, once known: from the header, or when
    /// decoding reaches the end.
    length: Option<u64>,
    /// The engine-rate frame the next fill starts at.
    position: u64,
    /// Whether the audio has ended.
    finished: bool,
    /// The decoder's output at the file's rate, one buffer per channel.
    planes: Vec<Vec<f32>>,
    conversion: Option<Conversion>,
}

/// The converter and its pending output.
struct Conversion {
    converter: RateConverter,
    /// Converted frames not yet handed out, one buffer per channel.
    pending: Vec<Vec<f32>>,
    pending_at: usize,
    pending_len: usize,
    /// Converted frames to drop before the next one handed out: the
    /// converter's delay, and the frames between where the input started
    /// and the frame asked for.
    skip: u64,
}

impl Feed {
    /// A feed over `decoder` at `rate`, from the file's first frame.
    pub(crate) fn new(decoder: Decoder, rate: u32) -> Result<Self, MediaError> {
        let info = *decoder.info();
        let channels = usize::from(info.channels);
        let from = info.sample_rate;
        let conversion = if from == rate {
            None
        } else {
            let converter = RateConverter::new(from, rate, channels)?;
            let pending = vec![vec![0.0; converter.output_frames_max()]; channels];
            let skip = converter.output_delay() as u64;
            Some(Conversion {
                converter,
                pending,
                pending_at: 0,
                pending_len: 0,
                skip,
            })
        };
        let chunk = conversion
            .as_ref()
            .map_or(4096, |c| c.converter.input_frames_next().max(1));
        Ok(Self {
            decoder,
            channels,
            from,
            to: rate,
            length: info.frames.map(|f| converted_len(f, from, rate)),
            position: 0,
            finished: false,
            planes: vec![vec![0.0; chunk]; channels],
            conversion,
        })
    }

    pub(crate) fn channels(&self) -> usize {
        self.channels
    }

    /// The length at the engine's rate, if known.
    pub(crate) fn length(&self) -> Option<u64> {
        self.length
    }

    /// Where the source ends, given a read of `len` frames from `start` that
    /// reached the end: after them, or, for an empty read that started past
    /// the end, at the length the feed knows.
    pub(crate) fn end_from(&self, start: u64, len: usize) -> u64 {
        let end = start + len as u64;
        match self.length {
            Some(length) if len == 0 => length.min(end),
            _ => end,
        }
    }

    /// The engine-rate frame the next fill starts at.
    pub(crate) fn position(&self) -> u64 {
        self.position
    }

    /// Moves to engine-rate frame `target`.
    ///
    /// Without conversion, this is the decoder's seek, which is
    /// sample-accurate. With it, the decoder starts early enough for the
    /// converter's filter to fill, and the frames before `target` are
    /// dropped. When the FFT resampler converts, the input starts on one of
    /// its block boundaries, a block before the one `target` falls in, so
    /// the output matches a conversion of the whole file. The sinc resampler
    /// (for rates whose ratio doesn't reduce to small numbers) can start
    /// anywhere, and lands within half a frame.
    pub(crate) fn seek(&mut self, target: u64) -> Result<(), MediaError> {
        self.position = target;
        self.finished = self.length.is_some_and(|length| target >= length);
        let Some(conversion) = &mut self.conversion else {
            return self.decoder.seek(target);
        };
        let converter = &mut conversion.converter;
        let delay = converter.output_delay() as u64;
        let (input, skip) = match converter.block_sizes() {
            Some((block_in, block_out)) => {
                let (block_in, block_out) = (block_in as u64, block_out as u64);
                let block = ((target + delay) / block_out).saturating_sub(1);
                (block * block_in, target + delay - block * block_out)
            }
            None => {
                let margin = 2 * delay + 64;
                let input = (u128::from(target.saturating_sub(margin)) * u128::from(self.from)
                    / u128::from(self.to)) as u64;
                let lands = input as f64 * f64::from(self.to) / f64::from(self.from);
                (input, (target as f64 - lands).round() as u64 + delay)
            }
        };
        self.decoder.seek(input)?;
        converter.reset();
        conversion.pending_at = 0;
        conversion.pending_len = 0;
        conversion.skip = skip;
        Ok(())
    }

    /// Fills up to `frames` frames of planar `out`, channel `c` at
    /// `out[c * stride]`, from the current position. Returns the frames
    /// written, fewer only at the end.
    pub(crate) fn fill(
        &mut self,
        out: &mut [f32],
        stride: usize,
        frames: usize,
    ) -> Result<usize, MediaError> {
        let frames = frames.min(stride);
        let mut done = 0;
        while done < frames && !self.finished {
            let room = match self.length {
                Some(length) => (frames - done).min(length.saturating_sub(self.position) as usize),
                None => frames - done,
            };
            if room == 0 {
                self.finished = true;
                break;
            }
            let got = if self.conversion.is_some() {
                self.convert_into(out, stride, done, room)?
            } else {
                self.decode_into(out, stride, done, room)?
            };
            if got == 0 {
                self.finished = true;
                self.length.get_or_insert(self.position);
                break;
            }
            done += got;
            self.position += got as u64;
        }
        Ok(done)
    }

    /// Decodes straight into `out` at frame `at`.
    fn decode_into(
        &mut self,
        out: &mut [f32],
        stride: usize,
        at: usize,
        frames: usize,
    ) -> Result<usize, MediaError> {
        let mut channels: Vec<&mut [f32]> = out
            .chunks_mut(stride)
            .take(self.channels)
            .map(|c| &mut c[at..at + frames])
            .collect();
        self.decoder.read(&mut channels)
    }

    /// Hands out converted frames into `out` at frame `at`, converting
    /// another chunk when none are pending. Returns 0 at the end.
    fn convert_into(
        &mut self,
        out: &mut [f32],
        stride: usize,
        at: usize,
        frames: usize,
    ) -> Result<usize, MediaError> {
        loop {
            let Some(conversion) = &mut self.conversion else {
                return Ok(0);
            };
            let pending = conversion.pending_len - conversion.pending_at;
            if pending > 0 {
                if conversion.skip > 0 {
                    let drop = pending.min(usize::try_from(conversion.skip).unwrap_or(usize::MAX));
                    conversion.pending_at += drop;
                    conversion.skip -= drop as u64;
                    continue;
                }
                let n = pending.min(frames);
                for (c, plane) in conversion.pending.iter().enumerate() {
                    out[c * stride + at..c * stride + at + n]
                        .copy_from_slice(&plane[conversion.pending_at..conversion.pending_at + n]);
                }
                conversion.pending_at += n;
                return Ok(n);
            }
            // The end of the input, once known, ends the output: a file of
            // `n` frames converts to `converted_len(n)`.
            if self.length.is_some_and(|length| self.position >= length) {
                return Ok(0);
            }
            self.convert_chunk()?;
        }
    }

    /// Reads a chunk of input and converts it into the pending buffers.
    /// Past the input's end the chunk is silence, which flushes the
    /// converter.
    fn convert_chunk(&mut self) -> Result<(), MediaError> {
        let Some(conversion) = &mut self.conversion else {
            return Ok(());
        };
        let wanted = conversion.converter.input_frames_next();
        for plane in &mut self.planes {
            if plane.len() < wanted {
                plane.resize(wanted, 0.0);
            }
        }
        let mut got = 0;
        while got < wanted {
            let mut channels: Vec<&mut [f32]> = self
                .planes
                .iter_mut()
                .map(|p| &mut p[got..wanted])
                .collect();
            let n = self.decoder.read(&mut channels)?;
            if n == 0 {
                if self.length.is_none() {
                    // The end: what's left to hand out is the converted
                    // length of everything decoded.
                    let end = self.decoder.position();
                    self.length = Some(converted_len(end, self.from, self.to));
                }
                break;
            }
            got += n;
        }
        let input: Vec<&[f32]> = self.planes.iter().map(|p| &p[..got]).collect();
        let mut output: Vec<&mut [f32]> =
            conversion.pending.iter_mut().map(|p| &mut p[..]).collect();
        let (_, written) = conversion.converter.process(&input, &mut output)?;
        conversion.pending_at = 0;
        conversion.pending_len = written;
        Ok(())
    }
}

/// The length of `frames` converted from `from` to `to`, rounded up, as
/// [`crate::converted_len`] gives it, in 64 bits.
pub(crate) fn converted_len(frames: u64, from: u32, to: u32) -> u64 {
    if from == to {
        return frames;
    }
    (u128::from(frames) * u128::from(to)).div_ceil(u128::from(from)) as u64
}

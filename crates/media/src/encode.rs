//! Encoding: writing audio files (WP-1.12, the export spec).
//!
//! [`AudioWriter`] writes WAV, AIFF, or FLAC at 16 or 24 bits (integers) or
//! 32 bits (IEEE floating point, in WAV and AIFF-C), a block of interleaved
//! 32-bit float samples at a time, so a file of any length is written
//! without holding it in memory. Below 32 bits a [`Quantiser`] dithers (or
//! not) and rounds each sample to the nearest step.
//!
//! [`Dither::NoiseShaped`] is Beatlayer's own noise-shaped dither, not a
//! reproduction of any licensed one. It uses error-feedback noise shaping
//! (R. W. Cutler, "Transmission systems employing quantization", U.S.
//! Patent 2,927,962, 1960) with the shaping filter chosen for where hearing
//! is least sensitive, the idea Lipshitz, Wannamaker and Vanderkooy set out
//! in "Minimally audible noise shaping" (J. Audio Eng. Soc. 39(11), 1991).
//! A second-order high-pass filter (1 − z⁻¹)² shapes both a triangular
//! dither and the rounding error, so the noise it leaves falls in the 2–5
//! kHz region hearing is most sensitive to and rises above 15 kHz. Its
//! filter state is kept per channel, so one channel's noise never feeds the
//! next.
//!
//! - WAV goes through `hound`.
//! - AIFF is written here: `FORM`, `COMM`, and `SSND` chunks, big-endian;
//!   32-bit float as AIFF-C with `fl32` samples.
//! - FLAC goes through `flacenc`, one frame at a time, straight to the
//!   file; the stream's header, with its MD5 and length, is written again
//!   when the stream is complete.
//!
//! Every writer patches its header when it finishes, so a file isn't
//! complete until [`AudioWriter::finish`] returns. Writing it under a
//! temporary name and moving it into place afterwards is the caller's job.
//!
//! [`wav_bytes`] writes a short WAV file into memory instead, with the same
//! quantiser, for the agent API's audio previews (WP-1.16).

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

use crate::error::MediaError;

/// A file type an export can write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileType {
    /// RIFF WAVE.
    Wav,
    /// AIFF, or AIFF-C for 32-bit float.
    Aiff,
    /// FLAC: lossless, integers only.
    Flac,
}

impl FileType {
    /// The usual extension, without the dot.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Aiff => "aif",
            Self::Flac => "flac",
        }
    }

    /// The file type a path's extension names: `.wav`, `.aif` or `.aiff`,
    /// or `.flac`, in any case.
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "wav" | "wave" => Some(Self::Wav),
            "aif" | "aiff" | "aifc" => Some(Self::Aiff),
            "flac" => Some(Self::Flac),
            _ => None,
        }
    }

    /// Whether this file type can hold samples of `depth`. FLAC holds
    /// integers only, so not 32-bit float.
    #[must_use]
    pub fn holds(self, depth: BitDepth) -> bool {
        !(self == Self::Flac && depth == BitDepth::Float32)
    }
}

/// The bits in each sample of a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BitDepth {
    /// 16-bit integers.
    Int16,
    /// 24-bit integers.
    Int24,
    /// 32-bit IEEE floating point.
    Float32,
}

impl BitDepth {
    /// The depth with this many bits: 16, 24, or 32.
    #[must_use]
    pub fn from_bits(bits: u32) -> Option<Self> {
        match bits {
            16 => Some(Self::Int16),
            24 => Some(Self::Int24),
            32 => Some(Self::Float32),
            _ => None,
        }
    }

    /// Bits per sample.
    #[must_use]
    pub fn bits(self) -> u32 {
        match self {
            Self::Int16 => 16,
            Self::Int24 => 24,
            Self::Float32 => 32,
        }
    }

    /// Whether samples are integers, which are rounded (and dithered).
    #[must_use]
    pub fn is_integer(self) -> bool {
        self != Self::Float32
    }
}

/// Noise added before rounding to an integer depth (the export spec, R7).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Dither {
    /// Nothing: samples are rounded as they are.
    None,
    /// Noise spread evenly over one step, ±½ step.
    Rectangular,
    /// The sum of two rectangular noises: ±1 step, with a triangular
    /// distribution.
    #[default]
    Triangular,
    /// Beatlayer's noise-shaped dither: a triangular dither and the
    /// rounding error, both passed through a second-order high-pass filter,
    /// so their noise leaves the frequencies hearing is most sensitive to
    /// and rises above 15 kHz. See the module's notes for the method and
    /// its sources. Only 16- and 24-bit exports use it.
    NoiseShaped,
}

/// The seed every [`Quantiser`] starts from unless given another, so an
/// export repeats bit for bit.
pub const DITHER_SEED: u64 = 0x6265_6174_6c61_7972;

/// Rounds samples to an integer depth, with dither.
///
/// A sample is scaled so 1.0 is 2^(bits−1), the dither noise added, rounded
/// to the nearest step (halves away from zero), and clipped to the depth's
/// range, so 1.0 becomes the largest positive step. NaN becomes 0. Without
/// dither, this is how the golden references round the engine's output.
///
/// [`Dither::NoiseShaped`] feeds the rounding error, and a shaped dither,
/// back through a second-order high-pass filter, one filter per channel.
#[derive(Clone, Debug)]
pub struct Quantiser {
    scale: f64,
    dither: Dither,
    state: u64,
    /// Channels in a frame: the shaper's state is kept apart for each, so a
    /// channel's noise doesn't feed the next one.
    channels: usize,
    /// Which channel the next sample belongs to.
    channel: usize,
    /// The noise shaper's state, one per channel (only used by
    /// [`Dither::NoiseShaped`]).
    shaper: Vec<Shaper>,
}

/// One channel's noise-shaping filter state: the last two rounding errors
/// and the last two dither values, so (1 − z⁻¹)² can be applied to both.
#[derive(Clone, Copy, Debug, Default)]
struct Shaper {
    errors: [f64; 2],
    dithers: [f64; 2],
}

impl Quantiser {
    /// A quantiser to `bits` (from 2 to 32) with `dither`, its noise
    /// generator started from `seed`, for interleaved frames of `channels`
    /// channels.
    ///
    /// # Panics
    ///
    /// If `bits` is outside 2 to 32, or `channels` is zero.
    #[must_use]
    pub fn new(bits: u32, dither: Dither, channels: usize, seed: u64) -> Self {
        assert!((2..=32).contains(&bits), "{bits} bits");
        assert!(channels >= 1, "{channels} channels");
        Self {
            scale: 2f64.powi(bits as i32 - 1),
            dither,
            state: seed,
            channels,
            channel: 0,
            shaper: vec![Shaper::default(); channels],
        }
    }

    /// The next uniform number in [0, 1), from a SplitMix64 generator.
    fn uniform(&mut self) -> f64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        (z >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Two uniform numbers, for a triangular distribution over ±1 step.
    fn triangular(&mut self) -> f64 {
        self.uniform() + self.uniform() - 1.0
    }

    /// The dither noise for the next sample, in steps.
    fn noise(&mut self) -> f64 {
        match self.dither {
            Dither::None => 0.0,
            Dither::Rectangular => self.uniform() - 0.5,
            Dither::Triangular => self.triangular(),
            Dither::NoiseShaped => 0.0,
        }
    }

    /// `sample` as an integer step.
    pub fn quantise(&mut self, sample: f32) -> i32 {
        let channel = self.channel;
        self.channel = (self.channel + 1) % self.channels;
        let scaled = f64::from(sample) * self.scale;
        if self.dither == Dither::NoiseShaped {
            // The dither and the rounding error both pass through
            // (1 − z⁻¹)²: `t` is the triangular noise, `d` its shaped
            // version, and `2e₁ − e₂` feeds back the shaped error.
            let t = self.triangular();
            let shaper = &mut self.shaper[channel];
            let d = t - 2.0 * shaper.dithers[0] + shaper.dithers[1];
            shaper.dithers = [t, shaper.dithers[0]];
            let v = scaled + d + 2.0 * shaper.errors[0] - shaper.errors[1];
            let y = v.round().clamp(-self.scale, self.scale - 1.0);
            shaper.errors = [v - y, shaper.errors[0]];
            return y as i32;
        }
        let noise = self.noise();
        // Scaling by a power of two is exact, so the rounding is the one
        // step that loses anything.
        (scaled + noise)
            .round()
            .clamp(-self.scale, self.scale - 1.0) as i32
    }
}

/// What an [`AudioWriter`] writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncodeSpec {
    /// The file type.
    pub file_type: FileType,
    /// The bits in each sample.
    pub bit_depth: BitDepth,
    /// The dither below 32 bits; ignored at 32.
    pub dither: Dither,
    /// Frames per second.
    pub sample_rate: u32,
    /// Channels in each frame, from 1 to 8.
    pub channels: u16,
}

enum Sink {
    Wav(hound::WavWriter<BufWriter<File>>),
    Aiff(AiffWriter),
    Flac(Box<FlacWriter>),
}

/// Writes an audio file a block at a time. See the module's notes.
pub struct AudioWriter {
    sink: Sink,
    spec: EncodeSpec,
    quantiser: Quantiser,
    frames: u64,
    /// Integer samples for the block being written, reused.
    ints: Vec<i32>,
}

impl std::fmt::Debug for AudioWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioWriter")
            .field("spec", &self.spec)
            .field("frames", &self.frames)
            .finish_non_exhaustive()
    }
}

fn encode_error(path: &Path, what: impl std::fmt::Display) -> MediaError {
    MediaError::Encode(format!("{}: {what}", path.display()))
}

impl AudioWriter {
    /// Creates the file at `path`, replacing any file there, to be written
    /// as `spec` says. The dither noise starts from [`DITHER_SEED`].
    ///
    /// # Errors
    ///
    /// [`MediaError::Encode`] if the file type can't hold the depth (32-bit
    /// FLAC), the rate or channel count is out of range, or the file can't
    /// be created.
    pub fn create(path: &Path, spec: EncodeSpec) -> Result<Self, MediaError> {
        Self::create_seeded(path, spec, DITHER_SEED)
    }

    /// [`AudioWriter::create`], with the dither noise started from `seed`.
    ///
    /// # Errors
    ///
    /// As [`AudioWriter::create`].
    pub fn create_seeded(path: &Path, spec: EncodeSpec, seed: u64) -> Result<Self, MediaError> {
        if !spec.file_type.holds(spec.bit_depth) {
            return Err(encode_error(
                path,
                "FLAC holds integer samples only; write 32-bit float as WAV or AIFF",
            ));
        }
        if !(1..=8).contains(&spec.channels) {
            return Err(encode_error(
                path,
                format!("{} channels; 1 to 8 can be written", spec.channels),
            ));
        }
        if !(1_000..=768_000).contains(&spec.sample_rate) {
            return Err(encode_error(
                path,
                format!("a sample rate of {} Hz", spec.sample_rate),
            ));
        }
        let file = File::create(path).map_err(|error| encode_error(path, error))?;
        let file = BufWriter::with_capacity(1 << 16, file);
        let sink = match spec.file_type {
            FileType::Wav => {
                let wav = hound::WavSpec {
                    channels: spec.channels,
                    sample_rate: spec.sample_rate,
                    bits_per_sample: spec.bit_depth.bits() as u16,
                    sample_format: if spec.bit_depth.is_integer() {
                        hound::SampleFormat::Int
                    } else {
                        hound::SampleFormat::Float
                    },
                };
                Sink::Wav(hound::WavWriter::new(file, wav).map_err(|e| encode_error(path, e))?)
            }
            FileType::Aiff => {
                Sink::Aiff(AiffWriter::new(file, &spec).map_err(|error| encode_error(path, error))?)
            }
            FileType::Flac => Sink::Flac(Box::new(
                FlacWriter::new(file, &spec).map_err(|error| encode_error(path, error))?,
            )),
        };
        Ok(Self {
            sink,
            spec,
            quantiser: Quantiser::new(
                spec.bit_depth.bits().min(24),
                spec.dither,
                usize::from(spec.channels),
                seed,
            ),
            frames: 0,
            ints: Vec::new(),
        })
    }

    /// What it writes.
    #[must_use]
    pub fn spec(&self) -> &EncodeSpec {
        &self.spec
    }

    /// Frames written so far.
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Writes whole frames of interleaved samples: each frame's channels in
    /// turn.
    ///
    /// # Errors
    ///
    /// [`MediaError::Encode`] if `samples` isn't whole frames or the file
    /// can't be written.
    pub fn write(&mut self, samples: &[f32]) -> Result<(), MediaError> {
        let channels = usize::from(self.spec.channels);
        if !samples.len().is_multiple_of(channels) {
            return Err(MediaError::Encode(format!(
                "{} samples aren't whole frames of {channels} channels",
                samples.len()
            )));
        }
        let failed = |what: String| MediaError::Encode(what);
        if self.spec.bit_depth.is_integer() {
            self.ints.clear();
            self.ints
                .extend(samples.iter().map(|&s| self.quantiser.quantise(s)));
            match &mut self.sink {
                Sink::Wav(wav) => {
                    for &sample in &self.ints {
                        let written = if self.spec.bit_depth == BitDepth::Int16 {
                            wav.write_sample(sample as i16)
                        } else {
                            wav.write_sample(sample)
                        };
                        written.map_err(|e| failed(e.to_string()))?;
                    }
                }
                Sink::Aiff(aiff) => aiff
                    .write_ints(&self.ints)
                    .map_err(|e| failed(e.to_string()))?,
                Sink::Flac(flac) => flac.write(&self.ints)?,
            }
        } else {
            match &mut self.sink {
                Sink::Wav(wav) => {
                    for &sample in samples {
                        wav.write_sample(sample)
                            .map_err(|e| failed(e.to_string()))?;
                    }
                }
                Sink::Aiff(aiff) => aiff
                    .write_floats(samples)
                    .map_err(|e| failed(e.to_string()))?,
                Sink::Flac(_) => unreachable!("FLAC refuses 32-bit float when it's created"),
            }
        }
        self.frames += (samples.len() / channels) as u64;
        Ok(())
    }

    /// Completes the file: writes what's buffered and patches the header
    /// with the length. Returns the frames written.
    ///
    /// # Errors
    ///
    /// [`MediaError::Encode`] if the file can't be written.
    pub fn finish(self) -> Result<u64, MediaError> {
        let failed = |what: String| MediaError::Encode(what);
        match self.sink {
            Sink::Wav(wav) => wav.finalize().map_err(|e| failed(e.to_string()))?,
            Sink::Aiff(aiff) => aiff.finish().map_err(|e| failed(e.to_string()))?,
            Sink::Flac(flac) => flac.finish()?,
        }
        Ok(self.frames)
    }
}

/// A whole WAV file in memory: interleaved `samples` in frames of
/// `channels`, at `sample_rate`, as `bit_depth` (16 or 24 bits rounded as
/// [`Quantiser`] rounds, with `dither` from [`DITHER_SEED`]; 32 bits as
/// IEEE floating point). The same samples make the same bytes every time.
///
/// # Errors
///
/// [`MediaError::Encode`] if `samples` isn't whole frames, or the rate or
/// channel count is out of range.
pub fn wav_bytes(
    samples: &[f32],
    channels: u16,
    sample_rate: u32,
    bit_depth: BitDepth,
    dither: Dither,
) -> Result<Vec<u8>, MediaError> {
    let failed = |what: String| MediaError::Encode(format!("an in-memory WAV file: {what}"));
    if !(1..=8).contains(&channels) {
        return Err(failed(format!(
            "{channels} channels; 1 to 8 can be written"
        )));
    }
    if !(1_000..=768_000).contains(&sample_rate) {
        return Err(failed(format!("a sample rate of {sample_rate} Hz")));
    }
    if !samples.len().is_multiple_of(usize::from(channels)) {
        return Err(failed(format!(
            "{} samples aren't whole frames of {channels} channels",
            samples.len()
        )));
    }
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: bit_depth.bits() as u16,
        sample_format: if bit_depth.is_integer() {
            hound::SampleFormat::Int
        } else {
            hound::SampleFormat::Float
        },
    };
    let mut bytes = std::io::Cursor::new(Vec::with_capacity(
        44 + samples.len() * bit_depth.bits() as usize / 8,
    ));
    {
        let mut wav = hound::WavWriter::new(&mut bytes, spec).map_err(|e| failed(e.to_string()))?;
        if bit_depth.is_integer() {
            let mut quantiser =
                Quantiser::new(bit_depth.bits(), dither, usize::from(channels), DITHER_SEED);
            for &sample in samples {
                let step = quantiser.quantise(sample);
                let written = if bit_depth == BitDepth::Int16 {
                    wav.write_sample(step as i16)
                } else {
                    wav.write_sample(step)
                };
                written.map_err(|e| failed(e.to_string()))?;
            }
        } else {
            for &sample in samples {
                wav.write_sample(sample)
                    .map_err(|e| failed(e.to_string()))?;
            }
        }
        wav.finalize().map_err(|e| failed(e.to_string()))?;
    }
    Ok(bytes.into_inner())
}

/// A sample rate as the 80-bit IEEE extended number AIFF's `COMM` chunk
/// holds.
fn extended(rate: u32) -> [u8; 10] {
    let mut bytes = [0; 10];
    if rate == 0 {
        return bytes;
    }
    let shift = 31 - rate.leading_zeros();
    let exponent = 16_383 + shift as u16;
    let mantissa = u64::from(rate) << (63 - shift);
    bytes[..2].copy_from_slice(&exponent.to_be_bytes());
    bytes[2..].copy_from_slice(&mantissa.to_be_bytes());
    bytes
}

/// AIFF, or AIFF-C for 32-bit float, written as it goes and patched at the
/// end: the `FORM` size, the frame count in `COMM`, and the `SSND` size.
struct AiffWriter {
    file: BufWriter<File>,
    bytes_per_sample: usize,
    channels: usize,
    /// Where `COMM`'s frame count is.
    frames_at: u64,
    /// Where `SSND`'s size is.
    ssnd_at: u64,
    data_bytes: u64,
    frame_bytes: Vec<u8>,
}

impl AiffWriter {
    fn new(mut file: BufWriter<File>, spec: &EncodeSpec) -> std::io::Result<Self> {
        let float = !spec.bit_depth.is_integer();
        // The FORM chunk's size is patched when the file is finished.
        file.write_all(b"FORM")?;
        file.write_all(&0u32.to_be_bytes())?;
        file.write_all(if float { b"AIFC" } else { b"AIFF" })?;
        let mut at = 12u64;
        if float {
            // The AIFF-C version chunk: the one version there is.
            file.write_all(b"FVER")?;
            file.write_all(&4u32.to_be_bytes())?;
            file.write_all(&0xA280_5140u32.to_be_bytes())?;
            at += 12;
        }
        // The compression name, as a Pascal string padded to an even
        // length.
        let name: &[u8] = b"32-bit floating point";
        let comm_size: u32 = if float {
            18 + 4 + 1 + name.len() as u32 + u32::from(name.len().is_multiple_of(2))
        } else {
            18
        };
        file.write_all(b"COMM")?;
        file.write_all(&comm_size.to_be_bytes())?;
        file.write_all(&(spec.channels as i16).to_be_bytes())?;
        let frames_at = at + 8 + 2;
        file.write_all(&0u32.to_be_bytes())?;
        file.write_all(&(spec.bit_depth.bits() as i16).to_be_bytes())?;
        file.write_all(&extended(spec.sample_rate))?;
        if float {
            file.write_all(b"fl32")?;
            file.write_all(&[name.len() as u8])?;
            file.write_all(name)?;
            if name.len().is_multiple_of(2) {
                file.write_all(&[0])?;
            }
        }
        at += 8 + u64::from(comm_size);
        file.write_all(b"SSND")?;
        let ssnd_at = at + 4;
        file.write_all(&0u32.to_be_bytes())?;
        // Offset and block size: no alignment.
        file.write_all(&0u32.to_be_bytes())?;
        file.write_all(&0u32.to_be_bytes())?;
        Ok(Self {
            file,
            bytes_per_sample: (spec.bit_depth.bits() / 8) as usize,
            channels: usize::from(spec.channels),
            frames_at,
            ssnd_at,
            data_bytes: 0,
            frame_bytes: Vec::new(),
        })
    }

    fn write_ints(&mut self, samples: &[i32]) -> std::io::Result<()> {
        self.frame_bytes.clear();
        let width = self.bytes_per_sample;
        for &sample in samples {
            self.frame_bytes
                .extend_from_slice(&sample.to_be_bytes()[4 - width..]);
        }
        self.data_bytes += self.frame_bytes.len() as u64;
        self.file.write_all(&self.frame_bytes)
    }

    fn write_floats(&mut self, samples: &[f32]) -> std::io::Result<()> {
        self.frame_bytes.clear();
        for &sample in samples {
            self.frame_bytes.extend_from_slice(&sample.to_be_bytes());
        }
        self.data_bytes += self.frame_bytes.len() as u64;
        self.file.write_all(&self.frame_bytes)
    }

    fn finish(mut self) -> std::io::Result<()> {
        let too_long = || std::io::Error::other("the audio is too long for an AIFF file");
        if self.data_bytes % 2 == 1 {
            // Chunks are padded to an even length; the pad isn't counted.
            self.file.write_all(&[0])?;
        }
        let end = self.file.stream_position()?;
        let frames = self.data_bytes / (self.bytes_per_sample * self.channels) as u64;
        let ssnd = u32::try_from(self.data_bytes + 8).map_err(|_| too_long())?;
        let form = u32::try_from(end - 8).map_err(|_| too_long())?;
        let frames = u32::try_from(frames).map_err(|_| too_long())?;
        self.file.seek(SeekFrom::Start(4))?;
        self.file.write_all(&form.to_be_bytes())?;
        self.file.seek(SeekFrom::Start(self.frames_at))?;
        self.file.write_all(&frames.to_be_bytes())?;
        self.file.seek(SeekFrom::Start(self.ssnd_at))?;
        self.file.write_all(&ssnd.to_be_bytes())?;
        self.file.flush()?;
        self.file.get_ref().sync_all()
    }
}

/// FLAC written a frame at a time. The stream's header goes first with
/// placeholders, and is written again at the end with the MD5 of the
/// samples, their count, and the frame sizes: its length doesn't change.
struct FlacWriter {
    file: BufWriter<File>,
    config: flacenc::error::Verified<flacenc::config::Encoder>,
    stream: flacenc::component::Stream,
    buffer: (flacenc::source::FrameBuf, flacenc::source::Context),
    block_size: usize,
    channels: usize,
    /// Interleaved samples waiting for a whole block.
    pending: Vec<i32>,
    sink: flacenc::bitsink::ByteSink,
}

fn flac_error(what: impl std::fmt::Debug) -> MediaError {
    MediaError::Encode(format!("FLAC: {what:?}"))
}

impl FlacWriter {
    fn new(mut file: BufWriter<File>, spec: &EncodeSpec) -> Result<Self, MediaError> {
        use flacenc::component::BitRepr;
        use flacenc::error::Verify;

        let config = flacenc::config::Encoder::default()
            .into_verified()
            .map_err(|(_, error)| flac_error(error))?;
        let channels = usize::from(spec.channels);
        let bits = spec.bit_depth.bits() as usize;
        let block_size = config.block_size;
        let mut stream = flacenc::component::Stream::new(spec.sample_rate as usize, channels, bits)
            .map_err(flac_error)?;
        stream
            .stream_info_mut()
            .set_block_sizes(block_size, block_size)
            .map_err(flac_error)?;
        let buffer = (
            flacenc::source::FrameBuf::with_size(channels, block_size).map_err(flac_error)?,
            flacenc::source::Context::new(bits, channels),
        );
        let mut sink = flacenc::bitsink::ByteSink::new();
        stream.write(&mut sink).map_err(flac_error)?;
        file.write_all(sink.as_slice())?;
        sink.clear();
        Ok(Self {
            file,
            config,
            stream,
            buffer,
            block_size,
            channels,
            pending: Vec::new(),
            sink,
        })
    }

    fn write(&mut self, samples: &[i32]) -> Result<(), MediaError> {
        self.pending.extend_from_slice(samples);
        let block = self.block_size * self.channels;
        let mut taken = 0;
        while self.pending.len() - taken >= block {
            self.encode(taken..taken + block)?;
            taken += block;
        }
        self.pending.drain(..taken);
        Ok(())
    }

    fn encode(&mut self, range: std::ops::Range<usize>) -> Result<(), MediaError> {
        use flacenc::component::BitRepr;
        use flacenc::source::Fill;

        self.buffer
            .fill_interleaved(&self.pending[range])
            .map_err(flac_error)?;
        let number = self
            .buffer
            .1
            .current_frame_number()
            .ok_or_else(|| flac_error("no frame to encode"))?;
        let frame = flacenc::encode_fixed_size_frame(
            &self.config,
            &self.buffer.0,
            number,
            self.stream.stream_info(),
        )
        .map_err(flac_error)?;
        self.stream.stream_info_mut().update_frame_info(&frame);
        self.sink.clear();
        frame.write(&mut self.sink).map_err(flac_error)?;
        self.file.write_all(self.sink.as_slice())?;
        Ok(())
    }

    fn finish(mut self) -> Result<(), MediaError> {
        use flacenc::component::BitRepr;

        if !self.pending.is_empty() {
            self.encode(0..self.pending.len())?;
        }
        let digest = self.buffer.1.md5_digest();
        let total = self.buffer.1.total_samples();
        let info = self.stream.stream_info_mut();
        info.set_md5_digest(&digest);
        info.set_total_samples(total);
        // A fixed-blocksize stream's minimum block size leaves out the last
        // block, which may be shorter. Counting it, as the frames' update
        // does, makes the stream look variable-blocksize to decoders that
        // check (symphonia, which Beatlayer imports with, among them).
        info.set_block_sizes(self.block_size, self.block_size)
            .map_err(flac_error)?;
        self.sink.clear();
        self.stream.write(&mut self.sink).map_err(flac_error)?;
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(self.sink.as_slice())?;
        self.file.flush()?;
        self.file.get_ref().sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::Decoder;

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            // A number, not the time's debug form, which has characters a
            // Windows path can't hold.
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos());
            let path = std::env::temp_dir().join(format!(
                "bl-media-encode-{name}-{}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    // Spec: export R4, R7.
    #[test]
    fn samples_round_to_the_nearest_step_and_clip() {
        let mut q = Quantiser::new(24, Dither::None, 1, 0);
        assert_eq!(q.quantise(0.0), 0);
        assert_eq!(q.quantise(0.5), 1 << 22);
        assert_eq!(q.quantise(-1.0), -(1 << 23));
        assert_eq!(q.quantise(1.0), (1 << 23) - 1);
        assert_eq!(q.quantise(2.0), (1 << 23) - 1);
        assert_eq!(
            q.quantise(1.5 / 8_388_608.0),
            2,
            "halves round away from zero"
        );
        assert_eq!(q.quantise(-1.5 / 8_388_608.0), -2);
        assert_eq!(q.quantise(f32::NAN), 0);
        let mut q = Quantiser::new(16, Dither::None, 1, 0);
        assert_eq!(q.quantise(1.0), i32::from(i16::MAX));
        assert_eq!(q.quantise(-1.0), i32::from(i16::MIN));
        assert_eq!(q.quantise(0.25), 8192);
    }

    // Spec: export R7, R14.
    #[test]
    fn dither_stays_within_its_steps_and_repeats() {
        for dither in [Dither::Rectangular, Dither::Triangular] {
            let mut q = Quantiser::new(16, dither, 1, DITHER_SEED);
            let mut again = Quantiser::new(16, dither, 1, DITHER_SEED);
            let mut sum = 0i64;
            let mut squares = 0f64;
            let n = 200_000;
            for i in 0..n {
                // A value between steps: 10.25 steps.
                let x = 10.25 / 32_768.0;
                let a = q.quantise(x);
                assert_eq!(a, again.quantise(x), "the same seed repeats, at {i}");
                assert!((9..=11).contains(&a), "{dither:?}: {a}");
                sum += i64::from(a);
                squares += (f64::from(a) - 10.25).powi(2);
            }
            // Dither keeps the average: 10.25 steps, within sampling error.
            let mean = sum as f64 / f64::from(n);
            assert!((mean - 10.25).abs() < 0.01, "{dither:?}: mean {mean}");
            // The error's variance, in steps squared: 3/16 for rectangular
            // noise at a quarter step, and 1/4 for triangular noise, whose
            // error doesn't depend on the value.
            let variance = squares / f64::from(n);
            match dither {
                Dither::Rectangular => assert!((variance - 0.1875).abs() < 0.01, "{variance}"),
                _ => assert!((variance - 0.25).abs() < 0.01, "{variance}"),
            }
        }
        // With no dither, a value on a step stays there.
        let mut q = Quantiser::new(16, Dither::None, 1, 7);
        assert!((0..1000).all(|_| q.quantise(3.0 / 32_768.0) == 3));
    }

    /// |X|² at `freq` hertz in `samples` (steps), by Goertzel's method, for
    /// a real signal at `rate` hertz.
    fn goertzel(samples: &[f64], rate: f64, freq: f64) -> f64 {
        let coeff = 2.0 * (std::f64::consts::TAU * freq / rate).cos();
        let (mut s1, mut s2) = (0.0, 0.0);
        for &x in samples {
            let s0 = x + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        s1 * s1 + s2 * s2 - coeff * s1 * s2
    }

    /// The mean one-sided noise power spectral density across `low..high`
    /// hertz, sampled at `count` frequencies: steps² per hertz.
    fn band_density(samples: &[f64], rate: f64, low: f64, high: f64, count: usize) -> f64 {
        let n = samples.len() as f64;
        (0..count)
            .map(|k| {
                let freq = low + (high - low) * (k as f64 + 0.5) / count as f64;
                2.0 * goertzel(samples, rate, freq) / (n * rate)
            })
            .sum::<f64>()
            / count as f64
    }

    /// Spec: export R7 (WP-2.27). With digital silence in, the shaped
    /// dither's noise is lower in the 2–5 kHz band hearing is most
    /// sensitive to than triangular dither's flat noise, and higher above
    /// 15 kHz, where the shaping moves it.
    #[test]
    fn the_noise_shaped_dither_moves_its_noise_out_of_the_ear() {
        let rate = 48_000.0f64;
        let n = 1 << 17;
        let noise = |dither| {
            let mut q = Quantiser::new(16, dither, 1, DITHER_SEED);
            (0..n)
                .map(|_| f64::from(q.quantise(0.0)))
                .collect::<Vec<f64>>()
        };
        let shaped = noise(Dither::NoiseShaped);
        let triangular = noise(Dither::Triangular);
        let shaped_ear = band_density(&shaped, rate, 2_000.0, 5_000.0, 64);
        let triangular_ear = band_density(&triangular, rate, 2_000.0, 5_000.0, 64);
        let shaped_top = band_density(&shaped, rate, 15_000.0, 23_000.0, 64);
        let triangular_top = band_density(&triangular, rate, 15_000.0, 23_000.0, 64);
        assert!(
            shaped_ear < triangular_ear / 2.0,
            "2–5 kHz: shaped {shaped_ear:e}, triangular {triangular_ear:e}"
        );
        assert!(
            shaped_top > triangular_top * 2.0,
            "above 15 kHz: shaped {shaped_top:e}, triangular {triangular_top:e}"
        );
        // The shaping moves the noise rather than multiplying it: the whole
        // band holds no more than about 12 dB more than triangular's.
        let shaped_all = band_density(&shaped, rate, 100.0, 23_000.0, 256);
        let triangular_all = band_density(&triangular, rate, 100.0, 23_000.0, 256);
        assert!(
            shaped_all < triangular_all * 16.0,
            "{shaped_all:e} against {triangular_all:e}"
        );
    }

    /// Spec: export R7 (WP-2.27). A −90 dBFS sine, about one step at 16
    /// bits, still stands well above the noise the shaped dither leaves.
    #[test]
    fn a_minus_90_dbfs_sine_stays_above_the_noise_shaped_dither() {
        let rate = 48_000.0f64;
        let freq = 1_000.0f64;
        let n = 1 << 17;
        let amplitude = 10f64.powf(-90.0 / 20.0);
        let mut q = Quantiser::new(16, Dither::NoiseShaped, 1, DITHER_SEED);
        let out: Vec<f64> = (0..n)
            .map(|i| {
                let x = amplitude * (std::f64::consts::TAU * freq * i as f64 / rate).sin();
                f64::from(q.quantise(x as f32))
            })
            .collect();
        // The tone's amplitude, from its spectral line: |X| ≈ n·A/2.
        let tone = 2.0 * goertzel(&out, rate, freq).sqrt() / n as f64;
        // The noise the shaping leaves where hearing is most sensitive.
        let noise = (band_density(&out, rate, 2_000.0, 5_000.0, 64) * 3_000.0).sqrt();
        assert!(tone > 0.8, "the sine is {} steps", tone);
        assert!(
            tone > noise * 5.0,
            "the sine {tone} against {noise} of noise"
        );
    }

    /// Spec: export R7, R14 (WP-2.27). The shaped dither repeats from the
    /// same seed, and each sample stays within the shaping noise of the
    /// plain rounding, so an export with it nulls within its own noise.
    #[test]
    fn the_noise_shaped_dither_repeats_and_stays_within_its_noise() {
        let mut a = Quantiser::new(16, Dither::NoiseShaped, 2, DITHER_SEED);
        let mut b = Quantiser::new(16, Dither::NoiseShaped, 2, DITHER_SEED);
        let mut plain = Quantiser::new(16, Dither::None, 2, DITHER_SEED);
        for i in 0..100_000 {
            let t = i as f64 / 48_000.0;
            let x = (0.5 * (std::f64::consts::TAU * 997.0 * t).sin()) as f32;
            let shaped = a.quantise(x);
            assert_eq!(shaped, b.quantise(x), "the same seed repeats, at {i}");
            let rounded = plain.quantise(x);
            assert!(
                (f64::from(shaped) - f64::from(rounded)).abs() <= 6.5,
                "at {i}: {shaped} against {rounded}"
            );
        }
    }

    #[test]
    fn the_file_type_follows_the_extension() {
        assert_eq!(FileType::from_path(Path::new("a.WAV")), Some(FileType::Wav));
        assert_eq!(
            FileType::from_path(Path::new("a.aiff")),
            Some(FileType::Aiff)
        );
        assert_eq!(
            FileType::from_path(Path::new("a.aif")),
            Some(FileType::Aiff)
        );
        assert_eq!(
            FileType::from_path(Path::new("a.flac")),
            Some(FileType::Flac)
        );
        assert_eq!(FileType::from_path(Path::new("a.mp3")), None);
        assert_eq!(FileType::from_path(Path::new("a")), None);
        assert!(!FileType::Flac.holds(BitDepth::Float32));
        assert!(FileType::Flac.holds(BitDepth::Int24));
        assert_eq!(BitDepth::from_bits(24), Some(BitDepth::Int24));
        assert_eq!(BitDepth::from_bits(8), None);
    }

    #[test]
    fn the_sample_rate_is_an_80_bit_extended_number() {
        assert_eq!(extended(44_100), [0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]);
        assert_eq!(extended(48_000), [0x40, 0x0E, 0xBB, 0x80, 0, 0, 0, 0, 0, 0]);
    }

    /// A stereo test signal: a sine on the left and a quieter, faster one on
    /// the right, `frames` long, odd so blocks don't divide it.
    fn signal(frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|n| {
                let t = n as f64 / 48_000.0;
                [
                    (0.9 * (std::f64::consts::TAU * 441.0 * t).sin()) as f32,
                    (-0.3 * (std::f64::consts::TAU * 1234.5 * t).sin()) as f32,
                ]
            })
            .collect()
    }

    // Spec: export R5.
    #[test]
    fn every_file_type_and_bit_depth_reads_back_as_written() {
        let temp = TempDir::new("round-trip");
        let samples = signal(10_001);
        for file_type in [FileType::Wav, FileType::Aiff, FileType::Flac] {
            for depth in [BitDepth::Int16, BitDepth::Int24, BitDepth::Float32] {
                let path = temp
                    .0
                    .join(format!("x{}.{}", depth.bits(), file_type.extension()));
                let spec = EncodeSpec {
                    file_type,
                    bit_depth: depth,
                    dither: Dither::None,
                    sample_rate: 44_100,
                    channels: 2,
                };
                if !file_type.holds(depth) {
                    assert!(matches!(
                        AudioWriter::create(&path, spec),
                        Err(MediaError::Encode(_))
                    ));
                    assert!(!path.exists());
                    continue;
                }
                let mut writer = AudioWriter::create(&path, spec).unwrap();
                // Blocks of uneven sizes, as an export writes them.
                for chunk in samples.chunks(2 * 777) {
                    writer.write(chunk).unwrap();
                }
                assert_eq!(writer.finish().unwrap(), 10_001);

                let decoder = Decoder::open(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
                let info = *decoder.info();
                assert_eq!(info.sample_rate, 44_100, "{path:?}");
                assert_eq!(info.channels, 2, "{path:?}");
                let data = decoder.decode_to_end().unwrap();
                assert_eq!(data.frames(), 10_001, "{path:?}");
                let mut q = Quantiser::new(depth.bits().min(24), Dither::None, 2, 0);
                let scale = f64::from(1u32 << (depth.bits().min(24) - 1));
                for (n, frame) in samples.chunks(2).enumerate() {
                    for (c, &written) in frame.iter().enumerate() {
                        let read = data.channel(c).unwrap()[n];
                        let expected = if depth.is_integer() {
                            (f64::from(q.quantise(written)) / scale) as f32
                        } else {
                            written
                        };
                        assert_eq!(
                            read.to_bits(),
                            expected.to_bits(),
                            "{path:?} frame {n} channel {c}: {read} against {expected}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_wav_in_memory_reads_back_as_written() {
        // WP-1.16: the agent API's previews.
        let samples: Vec<f32> = (0..2_000)
            .map(|n| ((n as f32) * 0.01).sin() * 0.5)
            .collect();
        for depth in [BitDepth::Int16, BitDepth::Int24, BitDepth::Float32] {
            let bytes = wav_bytes(&samples, 2, 48_000, depth, Dither::None).unwrap();
            let mut reader = hound::WavReader::new(std::io::Cursor::new(&bytes)).unwrap();
            assert_eq!(reader.spec().channels, 2);
            assert_eq!(reader.spec().sample_rate, 48_000);
            assert_eq!(reader.len() as usize, samples.len());
            let scale = 2f32.powi(depth.bits() as i32 - 1);
            let read: Vec<f32> = if depth.is_integer() {
                reader
                    .samples::<i32>()
                    .map(|s| s.unwrap() as f32 / scale)
                    .collect()
            } else {
                reader.samples::<f32>().map(Result::unwrap).collect()
            };
            for (a, b) in read.iter().zip(&samples) {
                assert!((a - b).abs() <= 1.0 / scale, "{depth:?}");
            }
        }
        assert_eq!(
            wav_bytes(&samples, 2, 48_000, BitDepth::Int16, Dither::Triangular).unwrap(),
            wav_bytes(&samples, 2, 48_000, BitDepth::Int16, Dither::Triangular).unwrap()
        );
        assert!(wav_bytes(&samples[..3], 2, 48_000, BitDepth::Int16, Dither::None).is_err());
    }
}

//! The largest audio Beatlayer reads, checked against a file's header before
//! anything is allocated for it.
//!
//! The limits are generous for real studio material and refuse the headers
//! that only a damaged or crafted file declares. A file past one is refused
//! with [`MediaError::Unsupported`], naming the value and the limit.

use crate::error::MediaError;

/// The most channels a file may declare: room for a large immersive or
/// multitrack recording.
pub const MAX_CHANNELS: u16 = 256;

/// The highest sample rate a file may declare, in hertz: four times 192 kHz.
pub const MAX_SAMPLE_RATE: u32 = 768_000;

/// The most frames a file may declare: every RIFF WAV fits, and it's over
/// 24 hours at 48 kHz.
pub const MAX_FRAMES: u64 = 1 << 32;

/// The most frames one decoded packet may span: more than any codec's frame
/// (AAC 1,024 or 2,048, MP3 1,152, Vorbis 8,192, ALAC 4,096 by default, FLAC
/// 65,535). A damaged packet's silence is cut to it, as its duration comes
/// from the container unchecked.
pub const MAX_PACKET_FRAMES: u64 = 1 << 16;

/// The most samples, over every channel, that a [`ProgressiveSample`]
/// makes ahead for one file: 1 GiB, about 46 minutes of stereo at 48 kHz.
/// Its whole length is allocated from what the header declares, before a
/// frame is decoded, so a longer file is refused there and the caller
/// decodes it whole, which allocates only as the audio arrives.
///
/// [`ProgressiveSample`]: crate::ProgressiveSample
pub const MAX_PROGRESSIVE_SAMPLES: usize = 1 << 28;

/// Refuses a header that declares more channels, a higher sample rate, or
/// more frames than Beatlayer reads.
pub(crate) fn check_header(
    channels: u64,
    sample_rate: u32,
    frames: Option<u64>,
) -> Result<(), MediaError> {
    if channels > u64::from(MAX_CHANNELS) {
        return Err(MediaError::Unsupported(format!(
            "{channels} channels, more than the {MAX_CHANNELS} Beatlayer reads"
        )));
    }
    if sample_rate > MAX_SAMPLE_RATE {
        return Err(MediaError::Unsupported(format!(
            "a sample rate of {sample_rate} Hz, above the {MAX_SAMPLE_RATE} Hz Beatlayer reads"
        )));
    }
    if let Some(frames) = frames.filter(|&f| f > MAX_FRAMES) {
        return Err(MediaError::Unsupported(format!(
            "{frames} frames, more than the {MAX_FRAMES} Beatlayer reads"
        )));
    }
    Ok(())
}

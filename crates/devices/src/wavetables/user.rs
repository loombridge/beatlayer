//! User wavetables (the wavetable spec, R6): a WAV file with a `clm ` chunk
//! is cut into the chunk's frame size; any other WAV or AIFF is cut into
//! frames of 2,048 samples, and a file shorter than a frame is stretched to
//! fill one. Each frame is then resampled to 2,048 samples by the kit.

use std::path::Path;

use bl_media::{BitDepth, Dither};
use lspk_wavetable::{FRAME_SIZE, Interpolation, MAX_FRAMES, Wavetable, wav};

/// The samples over which a frame's end is made to meet its start when
/// Raw is off.
const JOIN: usize = 32;

/// Reads the table `path` holds: frames cut as R6 says, user tables
/// crossfading between frames (R9), and with `raw` off each frame cleaned
/// so it loops without a click.
///
/// # Errors
///
/// Why the file can't be a table: it can't be read or decoded, or has no
/// audio.
pub fn read(path: &Path, raw: bool) -> Result<Wavetable, String> {
    let named = |why: &dyn std::fmt::Display| format!("{}: {why}", path.display());
    let bytes = std::fs::read(path).map_err(|e| named(&e))?;
    let table = if has_clm(&bytes) {
        wav::read(&bytes, None).map_err(|e| named(&e))?
    } else {
        // Decoded with the media crate, which reads AIFF as well, and cut
        // from the first channel.
        let decoded = bl_media::decode_file(path).map_err(|e| named(&e))?;
        let first = decoded.channel(0).filter(|c| c.len() >= 2);
        let first = first.ok_or_else(|| named(&"the file has no audio"))?;
        let samples = &first[..first.len().min(MAX_FRAMES * FRAME_SIZE)];
        let plain = bl_media::wav_bytes(samples, 1, 48_000, BitDepth::Float32, Dither::None)
            .map_err(|e| named(&e))?;
        // A file shorter than a frame is one frame of its own length.
        wav::read(&plain, Some(samples.len().min(FRAME_SIZE))).map_err(|e| named(&e))?
    };
    let mut frames: Vec<Vec<f32>> = table.frames().iter().map(|f| f.to_vec()).collect();
    if !raw {
        frames.iter_mut().for_each(|frame| clean(frame));
    }
    Ok(Wavetable::new(&frames)
        .map_err(|e| named(&e))?
        .with_interpolation(Interpolation::Crossfade))
}

/// Takes the DC offset out of `frame` and blends its last [`JOIN`] samples
/// into its first, so the sample after the last is the first (R6).
fn clean(frame: &mut [f32]) {
    let mean = frame.iter().sum::<f32>() / frame.len() as f32;
    frame.iter_mut().for_each(|x| *x -= mean);
    let first = frame[0];
    let from = frame.len() - JOIN;
    for (i, x) in frame[from..].iter_mut().enumerate() {
        let weight = (i + 1) as f32 / JOIN as f32;
        *x += (first - *x) * weight;
    }
}

/// Whether a WAV file has a `clm ` chunk, by walking its chunks.
fn has_clm(bytes: &[u8]) -> bool {
    if bytes.get(..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return false;
    }
    let mut at = 12;
    while let Some(header) = bytes.get(at..at + 8) {
        if &header[..4] == b"clm " {
            return true;
        }
        let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        // Chunks are padded to an even size.
        at = at.saturating_add(8 + size + (size & 1));
    }
    false
}

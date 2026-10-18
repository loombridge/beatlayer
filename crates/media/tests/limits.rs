//! Headers that declare more than Beatlayer reads are refused before
//! anything is allocated for them (`bl_media::limits`).
//!
//! Each file here is small: a header that claims a huge channel count,
//! sample rate, or length, with little or no audio behind it.

use std::io::Cursor;

use bl_media::limits::{MAX_CHANNELS, MAX_FRAMES, MAX_PROGRESSIVE_SAMPLES, MAX_SAMPLE_RATE};
use bl_media::{
    AudioWriter, BitDepth, Decoder, Dither, EncodeSpec, FileType, MediaError, ProgressiveSample,
};

/// A RIFF WAV header for 16-bit PCM, with `data_bytes` of silence.
fn wav(channels: u16, rate: u32, data_bytes: u32) -> Vec<u8> {
    let block = channels.wrapping_mul(2);
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&rate.wrapping_mul(u32::from(block)).to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    out.resize(out.len() + data_bytes as usize, 0);
    out
}

/// An RF64 header for 16-bit PCM whose `ds64` chunk declares `data_bytes`,
/// followed by `actual_bytes` of silence.
fn rf64(channels: u16, rate: u32, data_bytes: u64, actual_bytes: usize) -> Vec<u8> {
    let block = channels.wrapping_mul(2);
    let mut out = Vec::new();
    out.extend_from_slice(b"RF64");
    out.extend_from_slice(&u32::MAX.to_le_bytes());
    out.extend_from_slice(b"WAVEds64");
    out.extend_from_slice(&28u32.to_le_bytes());
    out.extend_from_slice(&(data_bytes + 72).to_le_bytes());
    out.extend_from_slice(&data_bytes.to_le_bytes());
    out.extend_from_slice(&(data_bytes / u64::from(block.max(1))).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&rate.wrapping_mul(u32::from(block)).to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&u32::MAX.to_le_bytes());
    out.resize(out.len() + actual_bytes, 0);
    out
}

/// A short real FLAC file whose STREAMINFO is changed to declare `frames`
/// frames: 4096 frames of silence at 48 kHz, written by the crate's own
/// encoder.
fn flac(channels: u16, frames: u64) -> Vec<u8> {
    let dir = std::env::temp_dir().join(format!("bl-media-limits-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{channels}-{frames}.flac"));
    let spec = EncodeSpec {
        file_type: FileType::Flac,
        bit_depth: BitDepth::Int16,
        dither: Dither::None,
        sample_rate: 48_000,
        channels,
    };
    let mut writer = AudioWriter::create(&path, spec).unwrap();
    writer
        .write(&vec![0.0; 4096 * usize::from(channels)])
        .unwrap();
    writer.finish().unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    // STREAMINFO follows "fLaC" and its block header; its total frames are
    // the low 36 bits of the big-endian u64 at offset 18.
    assert_eq!(&bytes[0..4], b"fLaC");
    let field = &mut bytes[18..26];
    let packed = u64::from_be_bytes(field.try_into().unwrap());
    let packed = (packed & !((1 << 36) - 1)) | (frames & ((1 << 36) - 1));
    field.copy_from_slice(&packed.to_be_bytes());
    bytes
}

fn open(bytes: Vec<u8>, extension: &str) -> Result<Decoder, MediaError> {
    Decoder::from_reader(Cursor::new(bytes), Some(extension))
}

/// The refusal, which must name what the file declared.
fn refused(result: Result<Decoder, MediaError>, what: &str) {
    match result {
        Err(MediaError::Unsupported(message)) => {
            assert!(
                message.contains(what),
                "{message:?} doesn't mention {what:?}"
            );
        }
        Err(other) => panic!("refused for another reason: {other}"),
        Ok(decoder) => panic!("accepted {:?}", decoder.info()),
    }
}

#[test]
fn wav_at_the_limits_opens() {
    let decoder = open(wav(8, MAX_SAMPLE_RATE, 64), "wav").unwrap();
    assert_eq!(decoder.info().sample_rate, MAX_SAMPLE_RATE);
    assert_eq!(decoder.info().channels, 8);
}

#[test]
fn wav_with_too_high_a_rate_is_refused() {
    refused(open(wav(2, MAX_SAMPLE_RATE + 1, 4), "wav"), "768001 Hz");
}

#[test]
fn rf64_at_the_channel_limit_opens() {
    let decoder = open(rf64(MAX_CHANNELS, 48_000, 512, 512), "wav").unwrap();
    assert_eq!(decoder.info().channels, MAX_CHANNELS);
    assert_eq!(decoder.info().frames, Some(1));
}

#[test]
fn rf64_with_too_many_channels_is_refused() {
    refused(
        open(rf64(MAX_CHANNELS + 1, 48_000, 514, 514), "wav"),
        "257 channels",
    );
}

#[test]
fn rf64_with_too_high_a_rate_is_refused() {
    refused(open(rf64(2, 1_000_000, 4, 4), "wav"), "1000000 Hz");
}

#[test]
fn rf64_declaring_more_data_than_it_has_keeps_what_it_has() {
    // 2^40 bytes declared, 16 there: the length comes from the file.
    let decoder = open(rf64(2, 48_000, 1 << 40, 16), "wav").unwrap();
    assert_eq!(decoder.info().frames, Some(4));
}

#[test]
fn flac_declaring_too_many_frames_is_refused() {
    refused(
        open(flac(2, MAX_FRAMES + 1), "flac"),
        &format!("{} frames", MAX_FRAMES + 1),
    );
}

#[test]
fn flac_declaring_the_frame_limit_opens() {
    let decoder = open(flac(2, MAX_FRAMES), "flac").unwrap();
    assert_eq!(decoder.info().frames, Some(MAX_FRAMES));
}

#[test]
fn a_long_declared_length_isnt_made_ahead() {
    // Within the header limits, but past what a progressive sample makes
    // ahead: refused there, before the planes are allocated.
    let frames = (MAX_PROGRESSIVE_SAMPLES / 2 + 1) as u64;
    let decoder = open(flac(2, frames), "flac").unwrap();
    match ProgressiveSample::start(decoder, 1024) {
        Err(MediaError::Unsupported(message)) => assert!(message.contains("progressively")),
        Err(other) => panic!("refused for another reason: {other}"),
        Ok(_) => panic!("made a progressive sample ahead of {frames} frames"),
    }
}

#[test]
fn a_long_declared_length_decodes_to_what_is_there() {
    // A header claiming 2^32 frames of eight channels over 4096: the
    // whole decode reserves a hint, not the declared length, and ends with
    // what the file holds.
    let decoder = open(flac(8, MAX_FRAMES), "flac").unwrap();
    if let Ok(data) = decoder.decode_to_end() {
        assert!(data.frames() <= 4096);
    }
}

#[test]
fn an_mp4_box_reaching_the_end_of_u64_is_refused() {
    // Found by the audio_header fuzz target: a box whose 64-bit size takes
    // the next box's position to u64::MAX, past which looking for one
    // overflowed.
    let mut bytes = vec![0, 0, 0, 1];
    bytes.extend_from_slice(b"ftyp");
    bytes.extend_from_slice(&u64::MAX.to_be_bytes());
    bytes.extend_from_slice(&[0; 8]);
    assert!(open(bytes, "m4a").is_err());
}

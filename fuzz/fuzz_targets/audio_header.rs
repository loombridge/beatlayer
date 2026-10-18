//! Fuzzes opening an audio file: bytes as read from a WAV, RF64, AIFF, FLAC,
//! MP3, Ogg or MPEG-4 file, through the format probe, the header readers and
//! the limits on what a header may declare, then a short decode.

#![no_main]

use std::io::Cursor;

use bl_media::Decoder;
use bl_media::limits::{MAX_CHANNELS, MAX_FRAMES, MAX_SAMPLE_RATE};
use libfuzzer_sys::fuzz_target;

/// Frames decoded after the header: enough to reach the first packets
/// without letting one input run for long.
const FRAMES: usize = 4096;

fuzz_target!(|data: &[u8]| {
    let Ok(mut decoder) = Decoder::from_reader(Cursor::new(data.to_vec()), None) else {
        return;
    };
    // What the decoder accepts stays inside the limits.
    let info = *decoder.info();
    assert!(info.channels >= 1 && info.channels <= MAX_CHANNELS);
    assert!(info.sample_rate >= 1 && info.sample_rate <= MAX_SAMPLE_RATE);
    assert!(info.frames.is_none_or(|f| f <= MAX_FRAMES));

    let channels = usize::from(info.channels);
    let mut planes = vec![vec![0.0f32; 512]; channels];
    let mut read = 0;
    while read < FRAMES {
        let mut out: Vec<&mut [f32]> = planes.iter_mut().map(|p| &mut p[..]).collect();
        match decoder.read(&mut out) {
            Ok(0) | Err(_) => break,
            Ok(n) => read += n,
        }
    }
    let _ = decoder.seek(0);
});

//! R3, R4: the movie's sound through `bl-media`.

use bl_media::{SampleData, decode_file};
use bl_video::MovieAudio;

use crate::common::*;

fn wav() -> SampleData {
    decode_file(fixture("mjpeg-24f.wav")).expect("the wav")
}

#[test]
fn a_movies_audio_decodes_as_an_audio_file_of_the_same_samples_does() {
    let mut audio = MovieAudio::open(&movie("mjpeg-24f.mov")).expect("audio");
    assert_eq!((audio.sample_rate(), audio.channels()), (RATE, 1));
    assert_eq!(audio.len(), u64::from(RATE));
    assert!(audio.decode_all().expect("decoded") == wav());
}

/// The audio under `mjpeg-gaps.mov`'s edit list: 0.1 s of silence, the first
/// 0.2 s of the wav, 0.1 s of silence, then 0.3 s of it from 0.2 s.
fn gapped(wav: &SampleData) -> Vec<f32> {
    let samples = wav.channel(0).expect("a channel");
    let tenth = RATE as usize / 10;
    let mut out = vec![0.0; tenth];
    out.extend_from_slice(&samples[..2 * tenth]);
    out.extend(vec![0.0; tenth]);
    out.extend_from_slice(&samples[2 * tenth..5 * tenth]);
    out
}

#[test]
fn audio_gaps_play_silence() {
    let wav = wav();
    let expected = gapped(&wav);
    let mut audio = MovieAudio::open(&movie("mjpeg-gaps.mov")).expect("audio");
    assert_eq!(audio.len(), expected.len() as u64);
    let all = audio.decode_all().expect("decoded");
    assert!(all.channel(0).expect("a channel") == expected.as_slice());
    // The gaps are exactly zero.
    let tenth = RATE as usize / 10;
    assert!(expected[..tenth].iter().all(|&s| s == 0.0));
    assert!(expected[3 * tenth..4 * tenth].iter().all(|&s| s == 0.0));
}

#[test]
fn blocks_read_anywhere_match_the_whole() {
    let expected = gapped(&wav());
    let mut audio = MovieAudio::open(&movie("mjpeg-gaps.mov")).expect("audio");
    // Blocks of 1,000 frames in a scattered order, across every edge, and one
    // that runs past the end.
    let total = expected.len();
    for start in [
        0,
        5000,
        1500,
        9000,
        2205,
        4410,
        6615,
        15000,
        100,
        total - 500,
    ] {
        let mut block = vec![0.0f32; 1000];
        let within = audio
            .read_at(start as u64, &mut [&mut block])
            .expect("read");
        let end = (start + 1000).min(total);
        assert_eq!(within, end - start, "frames within the track at {start}");
        assert_eq!(&block[..within], &expected[start..end], "block at {start}");
        assert!(block[within..].iter().all(|&s| s == 0.0));
    }
}

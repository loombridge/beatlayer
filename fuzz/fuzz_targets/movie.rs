//! Fuzzes reading a QuickTime movie: bytes as read from a `.mov` file,
//! through the box reader, the sample tables, the edit lists, the frame
//! times, and the movie's audio track on its timeline (R28).

#![no_main]

use bl_video::{Movie, MovieAudio, Rational};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(movie) = Movie::from_bytes(data.to_vec()) else {
        return;
    };
    if let Some(video) = movie.video() {
        // The presentation order is a permutation of the frames.
        let mut seen = vec![false; video.frame_count()];
        for &i in video.presentation_order() {
            assert!(!std::mem::replace(&mut seen[i as usize], true));
        }
        assert!(seen.iter().all(|&s| s));
        // Every question about a time or a frame has an answer.
        for i in (0..video.frame_count()).step_by(video.frame_count().div_ceil(64).max(1)) {
            let _ = video.frame_start(i);
            assert!(video.sync_before(i) <= i);
        }
        for n in [-1, 0, 1, 7, 1_000, i64::MAX / 3] {
            for d in [1, 3, 30_000, 44_100] {
                let t = Rational::new(i128::from(n), d);
                if let Some(frame) = movie.frame_at(t) {
                    assert!(frame < video.frame_count());
                }
            }
        }
    }
    if let Ok(mut audio) = MovieAudio::open(&movie) {
        let mut block = vec![vec![0.0f32; 256]; usize::from(audio.channels())];
        for start in [0, 1, 4_410, 1 << 20, u64::MAX / 4] {
            let mut planes: Vec<&mut [f32]> = block.iter_mut().map(|c| &mut c[..]).collect();
            let _ = audio.read_at(start, &mut planes);
        }
    }
});

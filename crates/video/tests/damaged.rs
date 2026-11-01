//! R28: a damaged movie is refused, or its damaged frames come out black,
//! without a crash.

use std::sync::Arc;

use bl_video::{FrameSource, Movie, Picture};

use crate::common::*;

#[test]
fn a_damaged_frame_is_black_and_the_others_still_decode() {
    let good = bytes("mjpeg-24f.mov");
    let movie = Movie::from_bytes(good.clone()).expect("a movie");
    let sample = *movie.video().expect("video").sample(3).expect("frame 3");
    let mut bad = good;
    let start = sample.offset as usize;
    bad[start..start + sample.size as usize].fill(0x5a);
    let mut source =
        FrameSource::new(Arc::new(Movie::from_bytes(bad).expect("a movie"))).expect("a decoder");
    assert!(matches!(source.picture_at(at(3)), Picture::Black));
    assert_eq!(index_of(&frame_at(&mut source, at(4))), 4);
    assert_eq!(index_of(&frame_at(&mut source, at(2))), 2);
}

#[test]
fn frames_the_file_cant_give_are_black_not_a_crash() {
    // The video's chunk offset points past the end of the file, as it does in
    // a movie cut short.
    let mut bad = bytes("mjpeg-24f.mov");
    let stco = bad.windows(4).position(|w| w == b"stco").expect("stco");
    // Kind, version and flags, count, then the first offset.
    bad[stco + 12..stco + 16].copy_from_slice(&0x7fff_0000u32.to_be_bytes());
    let mut source =
        FrameSource::new(Arc::new(Movie::from_bytes(bad).expect("a movie"))).expect("a decoder");
    for i in [0, 7, 23] {
        assert!(
            matches!(source.picture_at(at(i)), Picture::Black),
            "frame {i}"
        );
    }
}

#[test]
fn every_truncation_and_byte_flip_of_a_movie_parses_or_refuses() {
    let good = bytes("mjpeg-reordered.mov");
    for len in (0..good.len()).step_by(7) {
        let _ = Movie::from_bytes(good[..len].to_vec());
    }
    let moov = good.windows(4).position(|w| w == b"moov").expect("moov") - 4;
    for at in moov..good.len() {
        for flip in [0x00, 0xff, 0x80] {
            let mut bad = good.clone();
            bad[at] ^= flip;
            if let Ok(movie) = Movie::from_bytes(bad) {
                // Whatever parsed answers questions about any time.
                for n in [-1, 0, 5, 11, 12, 1000] {
                    let _ = movie.frame_at(at_time(n));
                }
            }
        }
    }
}

fn at_time(n: i128) -> bl_video::Rational {
    at(n)
}

#[test]
fn a_chapter_track_that_isnt_there_costs_the_movie_its_markers_only() {
    // R12, R28: the video's chapter reference names track 9.
    let mut bad = bytes("mjpeg-chapters.mov");
    let chap = bad.windows(4).position(|w| w == b"chap").expect("chap");
    bad[chap + 4..chap + 8].copy_from_slice(&9u32.to_be_bytes());
    let movie = Movie::from_bytes(bad).expect("a movie");
    assert!(movie.markers().is_empty());
    assert_eq!(movie.video().expect("video").frame_count(), 24);
}

//! R1, R2, R4, R26: frame times, seeks and gaps on Motion JPEG movies.

use bl_video::{FrameSource, Movie, Picture, Rational, VideoError, open_decoder};

use crate::common::*;

#[test]
fn frame_times_are_exact_rationals() {
    let movie = movie("mjpeg-24f.mov");
    let video = movie.video().expect("video");
    assert_eq!(video.frame_count(), 24);
    assert_eq!((video.width(), video.height()), (WIDTH, HEIGHT));
    for i in 0..24 {
        assert_eq!(video.frame_start(i), Some(at(i as i128)), "frame {i}");
    }
    assert_eq!(video.frame_start(1), Some(Rational::new(1001, 30000)));
    assert_eq!(movie.duration(), Rational::new(30000, 30000));
}

#[test]
fn every_frame_decodes_exactly_at_its_frame_time() {
    let movie = movie("mjpeg-24f.mov");
    let mut source = FrameSource::new(movie).expect("a decoder");
    for i in 0..24 {
        let frame = frame_at(&mut source, at(i));
        assert_eq!(index_of(&frame), i as u32, "frame {i}");
        assert_colours(&frame);
    }
}

#[test]
fn the_frame_shown_is_the_last_one_at_or_before_the_time() {
    let movie = movie("mjpeg-24f.mov");
    for i in 0..24usize {
        let start = at(i as i128);
        assert_eq!(movie.frame_at(start), Some(i));
        // One tick of the time scale before the next frame, and halfway.
        let next = at(i as i128 + 1);
        let before_next = next.minus(Rational::new(1, TIME_SCALE));
        let halfway = start.plus(Rational::new(FRAME, 2 * TIME_SCALE));
        if i < 23 {
            assert_eq!(
                movie.frame_at(before_next),
                Some(i),
                "just before {}",
                i + 1
            );
        }
        assert_eq!(movie.frame_at(halfway), Some(i));
    }
    // Before the movie, and at its end: nothing.
    assert_eq!(movie.frame_at(Rational::new(-1, 1000)), None);
    assert_eq!(movie.frame_at(at(24)), None);
}

#[test]
fn a_seek_lands_on_the_frame_the_rule_gives() {
    let movie = movie("mjpeg-24f.mov");
    let mut source = FrameSource::new(movie).expect("a decoder");
    for i in [20, 3, 23, 0, 11, 12, 10, 1] {
        let t = at(i).plus(Rational::new(1, 1000));
        assert_eq!(index_of(&frame_at(&mut source, t)), i as u32, "seek to {i}");
    }
}

#[test]
fn frames_stored_out_of_display_order_show_in_display_order() {
    let movie = movie("mjpeg-reordered.mov");
    let video = movie.video().expect("video");
    let order = [0u32, 2, 1, 4, 3, 6, 5, 8, 7, 10, 9, 11];
    for (decode, &display) in order.iter().enumerate() {
        assert_eq!(
            video.frame_start(decode),
            Some(at(i128::from(display))),
            "stored {decode}"
        );
    }
    let mut source = FrameSource::new(movie.clone()).expect("a decoder");
    for p in 0..12 {
        assert_eq!(
            index_of(&frame_at(&mut source, at(p))),
            p as u32,
            "display {p}"
        );
    }
}

#[test]
fn where_the_video_has_gaps_the_picture_is_black() {
    let movie = movie("mjpeg-gaps.mov");
    let mut source = FrameSource::new(movie.clone()).expect("a decoder");
    // Frames 0 to 7 show, four frames of nothing, then frames 8 to 15 from the
    // twelfth frame time on.
    for i in 0..8 {
        assert_eq!(index_of(&frame_at(&mut source, at(i))), i as u32);
    }
    for gap in 8..12 {
        assert!(
            matches!(source.picture_at(at(gap)), Picture::Black),
            "gap {gap}"
        );
        assert_eq!(movie.frame_at(at(gap)), None);
    }
    for i in 8..16 {
        assert_eq!(
            index_of(&frame_at(&mut source, at(i + 4))),
            i as u32,
            "after the gap"
        );
        assert_eq!(
            movie.video().expect("video").frame_start(i as usize),
            Some(at(i + 4))
        );
    }
    // The video ends before the audio does.
    assert!(matches!(source.picture_at(at(20)), Picture::Black));
}

#[test]
fn a_codec_no_decoder_takes_is_refused_with_the_reason() {
    let mut bytes = bytes("mjpeg-24f.mov");
    let at = bytes
        .windows(4)
        .position(|w| w == b"jpeg")
        .expect("the sample entry");
    bytes[at..at + 4].copy_from_slice(b"zzzz");
    let movie = Movie::from_bytes(bytes).expect("a movie");
    let Err(VideoError::NoDecoder { codec, reason }) = open_decoder(movie.video().expect("video"))
    else {
        panic!("expected a refusal");
    };
    assert_eq!(codec, "zzzz");
    assert!(reason.contains("Motion JPEG"), "{reason}");
}

#[test]
fn a_file_that_isnt_a_movie_is_refused() {
    for (name, data) in [
        ("a wav", bytes("mjpeg-24f.wav")),
        ("nothing", Vec::new()),
        (
            "noise",
            (0..4096u32).map(|i| ((i * 7919) >> 3) as u8).collect(),
        ),
    ] {
        assert!(
            matches!(
                Movie::from_bytes(data),
                Err(VideoError::Unsupported(_) | VideoError::Malformed(_) | VideoError::Io(_))
            ),
            "{name}"
        );
    }
}

#[test]
fn a_movie_that_isnt_quicktime_is_refused() {
    let mut bytes = bytes("mjpeg-24f.mov");
    // The ftyp box's major and compatible brands.
    for brand in bytes[8..24].as_chunks_mut::<4>().0 {
        if brand == b"qt  " {
            brand.copy_from_slice(b"isom");
        }
    }
    let Err(VideoError::Unsupported(why)) = Movie::from_bytes(bytes) else {
        panic!("expected a refusal");
    };
    assert!(why.contains("QuickTime"), "{why}");
}

#[test]
fn decoding_a_leading_picture_starts_before_its_sync_frame() {
    // HEVC's open groups of pictures: frames 11 and 12 are stored after the
    // sync frame 10 but show before it, and can lean on the group before.
    let movie = Movie::from_bytes(bytes("hevc.mov")).expect("a movie");
    let video = movie.video().expect("video");
    assert!(video.sample(10).expect("frame 10").sync);
    assert!(video.sample(11).expect("frame 11").pts < video.sample(10).expect("frame 10").pts);
    assert_eq!(video.sync_before(10), 10);
    assert_eq!(video.sync_before(11), 0);
    assert_eq!(video.sync_before(12), 0);
    assert_eq!(video.sync_before(13), 10);
}

#[test]
fn damaged_times_saturate_instead_of_overflowing() {
    let huge = Rational::new(i128::from(i64::MAX), 1);
    assert_eq!(huge.plus(huge), Rational::new(i128::from(i64::MAX), 1));
    let tiny = Rational::new(1, i128::from(u64::MAX));
    let _ = huge.plus(tiny);
    let _ = tiny.minus(huge);
}

#[test]
fn a_movies_chapter_track_gives_its_markers() {
    // R12: the text track the video names as its chapters, by time.
    let movie = movie("mjpeg-chapters.mov");
    let markers: Vec<_> = movie
        .markers()
        .iter()
        .map(|marker| (marker.time, marker.name.as_str()))
        .collect();
    assert_eq!(
        markers,
        [
            (Rational::new(0, 1), "Intro"),
            (Rational::new(15000, 30000), "Drop")
        ]
    );
    // A movie without one has none, and the chapter track isn't a picture.
    assert!(self::movie("mjpeg-24f.mov").markers().is_empty());
    assert_eq!(movie.video().expect("video").frame_count(), 24);
}

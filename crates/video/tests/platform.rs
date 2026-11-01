//! R26: H.264, HEVC and ProRes through the platform's decoders, and through
//! FFmpeg's libraries where they're installed.

use std::path::Path;
use std::sync::Arc;

use bl_video::{CodedSample, Frame, FrameSource, Movie, VideoDecoder, VideoError};

use crate::common::*;

/// Where each platform must decode each codec. ProRes and HEVC need a codec
/// pack on some Windows installations, and FFmpeg's libraries aren't always on
/// Linux: there a refusal with its reason is the right answer.
fn required(codec: &str) -> bool {
    cfg!(target_os = "macos") || (cfg!(windows) && codec == "h264")
}

/// Checks the frames decoded from stored frame `from` on; a leading picture
/// of the group decoding started at may be missing, but none may be wrong.
fn assert_frames(frames: &[Option<Frame>], movie: &Movie, from: usize) {
    let video = movie.video().expect("video");
    for (decode, frame) in frames.iter().enumerate().skip(from) {
        let display = video
            .presentation_order()
            .iter()
            .position(|&i| i as usize == decode)
            .expect("in the order");
        let start = video.sample(from).expect("sample").pts;
        let Some(frame) = frame else {
            assert!(
                from > 0 && video.sample(decode).expect("sample").pts < start,
                "frame {decode} wasn't decoded from {from}"
            );
            continue;
        };
        assert_eq!(index_of(frame), display as u32, "stored frame {decode}");
        assert_colours(frame);
    }
}

#[test]
fn the_platforms_decoders_decode_every_frame_exactly() {
    for codec in ["h264", "hevc", "prores"] {
        let name = format!("{codec}.mov");
        let movie = match Movie::open(fixture(&name)) {
            Ok(movie) => Arc::new(movie),
            Err(VideoError::NoDecoder {
                codec: fourcc,
                reason,
            }) if !required(codec) => {
                eprintln!("{name}: no decoder for {fourcc} here: {reason}");
                continue;
            }
            Err(e) => panic!("{name}: {e}"),
        };
        let mut source = FrameSource::new(movie.clone()).expect("a decoder");
        // The frame at each frame time, through the cache and the GOPs.
        for i in 0..24 {
            let frame = frame_at(&mut source, at(i));
            assert_eq!(index_of(&frame), i as u32, "{name} frame {i}");
            assert_colours(&frame);
        }
        // A seek back into the middle of a GOP lands on the same frame.
        for i in [17, 5, 13, 0, 22] {
            assert_eq!(
                index_of(&frame_at(&mut source, at(i))),
                i as u32,
                "{name} seek to {i}"
            );
        }
    }
}

/// Decodes a movie's frames with `decoder`, in one call from stored frame
/// `from` to the end, and returns them by stored index.
fn decode_from(decoder: &mut dyn VideoDecoder, movie: &Movie, from: usize) -> Vec<Option<Frame>> {
    let video = movie.video().expect("video");
    let mut reader = movie.reader().expect("a reader");
    let coded: Vec<Vec<u8>> = (0..video.frame_count())
        .map(|i| video.read_sample(&mut *reader, i).expect("a sample"))
        .collect();
    let samples: Vec<CodedSample<'_>> = coded
        .iter()
        .enumerate()
        .skip(from)
        .map(|(index, data)| CodedSample {
            index,
            pts: video.sample(index).expect("sample").pts,
            data,
        })
        .collect();
    let mut frames = vec![None; coded.len()];
    decoder
        .decode(&samples, &mut |index, frame| frames[index] = Some(frame))
        .expect("decoded");
    frames
}

#[cfg(unix)]
#[test]
fn ffmpegs_libraries_decode_every_frame_where_installed() {
    let dirs = [
        Some(Path::new("/opt/homebrew/lib")),
        Some(Path::new("/usr/local/lib")),
        None,
    ];
    let mut decoded = 0;
    for codec in ["h264", "hevc", "prores"] {
        // Parsed, not opened: on macOS `Movie::open` would ask VideoToolbox.
        let movie = Movie::from_bytes(bytes(&format!("{codec}.mov"))).expect("a movie");
        let video = movie.video().expect("video");
        let Some(mut decoder) = dirs
            .iter()
            .find_map(|dir| bl_video::ffmpeg::open(video, *dir).ok())
        else {
            eprintln!("FFmpeg's libraries aren't installed: {codec} skipped");
            continue;
        };
        // From each sync frame, as the video thread decodes: an open group's
        // leading pictures that are left out mustn't shift the others.
        for from in (0..video.frame_count()).filter(|&i| video.sample(i).expect("sample").sync) {
            assert_frames(&decode_from(&mut *decoder, &movie, from), &movie, from);
        }
        decoded += 1;
    }
    eprintln!("{decoded} of 3 codecs decoded through FFmpeg");
}

#[cfg(unix)]
#[test]
fn without_ffmpegs_libraries_the_codec_is_refused_with_the_reason() {
    let movie = Movie::from_bytes(bytes("h264.mov")).expect("a movie");
    let Err(reason) = bl_video::ffmpeg::open(
        movie.video().expect("video"),
        Some(Path::new("/nonexistent")),
    ) else {
        panic!("expected a refusal");
    };
    assert!(reason.contains("FFmpeg"), "{reason}");
}

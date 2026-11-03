//! The export spec R46 to R49, R51: Motion JPEG frames and 24-bit sound
//! written into a QuickTime movie that reads back as it was written.

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;

use bl_video::{
    AudioFormat, CodedFrame, EncoderKind, EncoderSettings, Frame, FrameSource, Movie, MovieAudio,
    MovieWriter, Rational, VideoEncoder, VideoFormat, available_encoders, encoded_size,
    open_encoder, scale,
};

use crate::common::*;

/// A path for a file of one test, removed with the guard.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        Self(std::env::temp_dir().join(format!("bl-video-{name}-{}.mov", std::process::id())))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn decoded(name: &str) -> Vec<Frame> {
    let movie = movie(name);
    let count = movie.video().expect("video").frame_count();
    let mut source = FrameSource::new(movie).expect("a decoder");
    (0..count as i128)
        .map(|i| (*frame_at(&mut source, at(i))).clone())
        .collect()
}

/// Writes `frames` through the Motion JPEG encoder with `samples` as 48 kHz
/// stereo sound.
fn write(kind: EncoderKind, path: &PathBuf, frames: &[Frame], samples: &[f32]) {
    let rate = Rational::new(TIME_SCALE, FRAME);
    let encoder =
        open_encoder(kind, WIDTH, HEIGHT, rate, &EncoderSettings::default()).expect("an encoder");
    write_with(encoder, path, frames, samples);
}

fn write_with(
    mut encoder: Box<dyn VideoEncoder>,
    path: &PathBuf,
    frames: &[Frame],
    samples: &[f32],
) {
    let rate = Rational::new(TIME_SCALE, FRAME);
    let mut writer = MovieWriter::new(
        BufWriter::new(File::create(path).expect("a file")),
        VideoFormat {
            width: WIDTH,
            height: HEIGHT,
            frame_rate: rate,
        },
        AudioFormat {
            sample_rate: 48_000,
            channels: 2,
        },
    )
    .expect("a writer");
    let mut coded: Vec<CodedFrame> = Vec::new();
    for frame in frames {
        encoder.encode(frame, &mut coded).expect("encoded");
    }
    encoder.finish(&mut coded).expect("finished");
    for frame in &coded {
        writer.write_video(frame).expect("written");
    }
    // The sound in chunks of a thousand frames, the last one short.
    for chunk in samples.chunks(2000) {
        writer.write_audio(chunk).expect("written");
    }
    writer
        .finish(&encoder.entry().expect("an entry"))
        .expect("finished");
}

#[test]
fn frames_and_sound_read_back_as_they_were_written() {
    let frames = decoded("mjpeg-24f.mov");
    let samples: Vec<f32> = (0..48_000 * 2 + 7)
        .map(|i| ((i / 2) as f32 * 0.01).sin() * 0.7 * if i % 2 == 0 { 1.0 } else { -1.0 })
        .collect();
    let file = Scratch::new("round-trip");
    write(EncoderKind::MotionJpeg, &file.0, &frames, &samples);

    let movie = std::sync::Arc::new(Movie::open(&file.0).expect("the movie reads"));
    let video = movie.video().expect("video");
    assert_eq!(video.frame_count(), 24);
    assert_eq!((video.width(), video.height()), (WIDTH, HEIGHT));
    for i in 0..24 {
        assert_eq!(video.frame_start(i), Some(at(i as i128)), "frame {i}");
    }
    let mut source = FrameSource::new(movie.clone()).expect("a decoder");
    for i in 0..24 {
        let frame = frame_at(&mut source, at(i));
        assert_eq!(index_of(&frame), i as u32, "frame {i}");
        assert_colours(&frame);
    }

    let mut audio = MovieAudio::open(&movie).expect("sound");
    assert_eq!((audio.sample_rate(), audio.channels()), (48_000, 2));
    assert_eq!(audio.len(), samples.len() as u64 / 2);
    let all = audio.decode_all().expect("decoded");
    for (c, plane) in [all.channel(0), all.channel(1)].into_iter().enumerate() {
        let plane = plane.expect("a channel");
        for (n, &sample) in plane.iter().enumerate() {
            let wrote = samples[n * 2 + c];
            assert!(
                (sample - wrote).abs() <= 1.0 / 8_388_608.0,
                "channel {c} frame {n}: {sample} against {wrote}"
            );
        }
    }
}

#[test]
fn a_busy_picture_survives_the_encoder_and_the_decoder() {
    // Every kind of block: gradients, edges and noise.
    let (w, h) = (101u32, 77u32);
    let mut rgba = Vec::new();
    let mut seed = 12345u32;
    for y in 0..h {
        for x in 0..w {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (seed >> 24) as f32 / 255.0 * 24.0;
            let edge = if (x / 20 + y / 20) % 2 == 0 {
                0.0
            } else {
                90.0
            };
            rgba.extend([
                (x as f32 * 2.0 + noise).min(255.0) as u8,
                (y as f32 * 3.0 + edge).min(255.0) as u8,
                (128.0 + edge + noise).min(255.0) as u8,
                255,
            ]);
        }
    }
    let frame = Frame::new(w, h, rgba).expect("a frame");
    let mut encoder = open_encoder(
        EncoderKind::MotionJpeg,
        w,
        h,
        Rational::new(25, 1),
        &EncoderSettings {
            quality: 95,
            ..EncoderSettings::default()
        },
    )
    .expect("an encoder");
    let mut coded = Vec::new();
    encoder.encode(&frame, &mut coded).expect("encoded");
    assert!(coded[0].sync);
    let file = Scratch::new("busy");
    let mut writer = MovieWriter::new(
        File::create(&file.0).expect("a file"),
        VideoFormat {
            width: w,
            height: h,
            frame_rate: Rational::new(25, 1),
        },
        AudioFormat {
            sample_rate: 44_100,
            channels: 1,
        },
    )
    .expect("a writer");
    writer.write_video(&coded[0]).expect("written");
    writer.write_audio(&[0.0; 4410]).expect("written");
    writer
        .finish(&encoder.entry().expect("an entry"))
        .expect("finished");
    let mut source = FrameSource::new(std::sync::Arc::new(Movie::open(&file.0).expect("reads")))
        .expect("a decoder");
    let back = frame_at(&mut source, Rational::new(0, 1));
    assert_eq!((back.width(), back.height()), (w, h));
    let mut squared = 0.0f64;
    for (a, b) in frame.rgba().chunks(4).zip(back.rgba().chunks(4)) {
        for c in 0..3 {
            squared += (f64::from(a[c]) - f64::from(b[c])).powi(2);
        }
    }
    let mse = squared / (f64::from(w) * f64::from(h) * 3.0);
    let psnr = 10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10();
    assert!(psnr > 35.0, "{psnr} dB");
}

#[test]
fn motion_jpeg_is_always_listed_and_sizes_are_even() {
    assert!(available_encoders().contains(&EncoderKind::MotionJpeg));
    assert_eq!(encoded_size(65, 47), (64, 46));
    assert_eq!(encoded_size(1, 1), (2, 2));
}

#[test]
fn scaling_keeps_the_picture() {
    let frame = decoded("mjpeg-24f.mov").remove(5);
    assert_eq!(scale(&frame, WIDTH, HEIGHT), frame);
    let big = scale(&frame, WIDTH * 2, HEIGHT * 2);
    assert_eq!((big.width(), big.height()), (WIDTH * 2, HEIGHT * 2));
    // The cells of the index are 16 pixels wide now.
    assert_eq!(index_of_scaled(&big), 5);
}

fn index_of_scaled(frame: &Frame) -> u32 {
    (0..8)
        .map(|bit| {
            let [r, ..] = frame.pixel(16 * bit + 8, 8).expect("inside");
            u32::from(r > 128) << bit
        })
        .sum()
}

#[test]
fn every_encoder_the_computer_lists_writes_a_movie_that_reads_back() {
    let frames = decoded("mjpeg-24f.mov");
    for kind in available_encoders() {
        let file = Scratch::new(&format!("kind-{kind:?}"));
        write(kind, &file.0, &frames, &[0.25; 48_000 * 2]);
        let movie =
            std::sync::Arc::new(Movie::open(&file.0).unwrap_or_else(|e| panic!("{kind}: {e}")));
        assert_eq!(movie.video().expect("video").frame_count(), 24, "{kind}");
        let mut source = FrameSource::new(movie).expect("a decoder");
        for i in 0..24 {
            let frame = frame_at(&mut source, at(i));
            assert_eq!(index_of(&frame), i as u32, "{kind} frame {i}");
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
fn macos_lists_the_encoders_avfoundation_writes_through() {
    let listed = available_encoders();
    for kind in [
        EncoderKind::H264,
        EncoderKind::ProRes422,
        EncoderKind::MotionJpeg,
    ] {
        assert!(listed.contains(&kind), "{kind} in {listed:?}");
    }
}

#[cfg(unix)]
#[test]
fn ffmpegs_h264_encoder_writes_a_movie_that_reads_back_where_installed() {
    let frames = decoded("mjpeg-24f.mov");
    let mut written = 0;
    for dir in [
        Some(std::path::Path::new("/opt/homebrew/lib")),
        Some(std::path::Path::new("/usr/local/lib")),
        None,
    ] {
        let Ok(encoder) = bl_video::ffmpeg::open_h264_encoder(
            dir,
            WIDTH,
            HEIGHT,
            Rational::new(TIME_SCALE, FRAME),
            &EncoderSettings::default(),
        ) else {
            continue;
        };
        let file = Scratch::new("ffmpeg-h264");
        write_with(encoder, &file.0, &frames, &[0.0; 48_000 * 2]);
        let movie = std::sync::Arc::new(Movie::open(&file.0).expect("the movie reads"));
        assert_eq!(movie.video().expect("video").frame_count(), 24);
        let mut source = FrameSource::new(movie).expect("a decoder");
        for i in 0..24 {
            assert_eq!(
                index_of(&frame_at(&mut source, at(i))),
                i as u32,
                "frame {i}"
            );
        }
        written += 1;
        break;
    }
    eprintln!("{written} FFmpeg encoders ran");
}

#[cfg(unix)]
#[test]
fn without_ffmpegs_libraries_the_encoder_is_refused_with_the_reason() {
    let Err(error) = bl_video::ffmpeg::open_h264_encoder(
        Some(std::path::Path::new("/nonexistent")),
        WIDTH,
        HEIGHT,
        Rational::new(30, 1),
        &EncoderSettings::default(),
    ) else {
        panic!("expected a refusal");
    };
    assert!(error.to_string().contains("FFmpeg"), "{error}");
}

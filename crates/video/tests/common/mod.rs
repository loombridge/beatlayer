//! Fixtures and helpers shared by the tests.

use std::path::PathBuf;
use std::sync::Arc;

use bl_video::{Frame, FrameSource, Movie, Picture, Rational};

/// The generator's picture size and frame timing.
pub const WIDTH: u32 = 64;
pub const HEIGHT: u32 = 48;
pub const TIME_SCALE: i128 = 30000;
pub const FRAME: i128 = 1001;
pub const RATE: u32 = 22050;

pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/video")
        .join(name)
}

pub fn bytes(name: &str) -> Vec<u8> {
    std::fs::read(fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// A movie that needs no platform decoder: Motion JPEG decodes in Rust.
pub fn movie(name: &str) -> Arc<Movie> {
    Arc::new(Movie::open(fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}")))
}

/// The time frame `n` starts to show if the frames are one `FRAME` apart.
pub fn at(n: i128) -> Rational {
    Rational::new(n * FRAME, TIME_SCALE)
}

/// The index a decoded frame shows in its top row of cells.
pub fn index_of(frame: &Frame) -> u32 {
    (0..8)
        .map(|bit| {
            let [r, g, b, a] = frame.pixel(8 * bit + 4, 4).expect("inside the picture");
            assert_eq!(a, 255, "alpha");
            let luma = (u32::from(r) + u32::from(g) + u32::from(b)) / 3;
            assert!(
                !(70..=185).contains(&luma),
                "a cell neither black nor white: {luma}"
            );
            u32::from(luma > 128) << bit
        })
        .sum()
}

/// A red square at the bottom right and a green one at the bottom left.
pub fn assert_colours(frame: &Frame) {
    assert_eq!((frame.width(), frame.height()), (WIDTH, HEIGHT));
    let [r, g, b, _] = frame.pixel(WIDTH - 8, HEIGHT - 8).expect("pixel");
    assert!(r > 180 && g < 90 && b < 90, "red square: {r} {g} {b}");
    let [r, g, b, _] = frame.pixel(8, HEIGHT - 8).expect("pixel");
    assert!(g > 180 && r < 90 && b < 90, "green square: {r} {g} {b}");
}

/// The frame shown at `t`, which must be a decoded picture.
pub fn frame_at(source: &mut FrameSource, t: Rational) -> Arc<Frame> {
    match source.picture_at(t) {
        Picture::Frame(frame) => frame,
        other => panic!("expected a frame at {t}, got {other:?}"),
    }
}

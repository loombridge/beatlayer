//! Transients on a generated drum loop (the warping spec, R22).

use crate::common;
use bl_analysis::cache::Analysis;
use bl_media::decode_file;
use common::{TempDir, drum_hits, drum_loop};

fn found() -> Vec<f64> {
    let dir = TempDir::new("drums");
    let path = dir.path().join("drum-loop.wav");
    std::fs::write(&path, drum_loop()).unwrap();
    Analysis::of(&decode_file(&path).unwrap())
        .unwrap()
        .transients
}

// Spec: warping R22
#[test]
fn a_generated_drum_loops_hits_are_found_within_a_millisecond() {
    let hits = drum_hits();
    let found = found();
    assert_eq!(found.len(), hits.len(), "missed or doubled: {found:?}");
    for (got, want) in found.iter().zip(&hits) {
        assert!((got - want).abs() < 0.001, "{got} vs {want}: {found:?}");
    }
}

// Spec: warping R22
#[test]
fn the_same_file_gives_the_same_transients() {
    let first = found();
    for _ in 0..3 {
        assert_eq!(found(), first);
    }
}

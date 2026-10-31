use bl_stretch::beats::Plan;
use bl_stretch::tones::diff_ops;
use bl_stretch::{Request, Texture, WarpMode, Window};

use crate::tones::{bass_waveform, sine};

#[test]
fn texture_grains_follow_grain_size_and_flux() {
    let sample_rate = 48_000.0;
    let mut texture = Texture::new(2, sample_rate, 42);

    // At Flux 0, grain length is constant and jitter is 0.0
    texture.set_grain_size(65.0);
    texture.set_flux(0.0);
    let nominal = (0.065 * sample_rate).round() as usize;

    for _ in 0..10 {
        let (len, jitter) = texture.grain_length_and_jitter();
        assert_eq!(len, nominal);
        assert_eq!(jitter, 0.0);
    }

    // At Flux 50%, grain lengths vary within 50% of nominal and jitter is bounded
    texture.set_flux(50.0);
    let mut lengths = Vec::new();
    let max_delta = (nominal as f64 * 0.5).round() as usize + 2;

    for _ in 0..20 {
        let (len, jitter) = texture.grain_length_and_jitter();
        lengths.push(len);
        let diff = (len as isize - nominal as isize).unsigned_abs();
        assert!(diff <= max_delta, "len {} vs nominal {}", len, nominal);
        assert!(
            jitter.abs() <= nominal as f64 * 0.5 + 1.0,
            "jitter {} vs bound",
            jitter
        );
    }

    // Verify there is variation
    let has_variation = lengths.iter().any(|&l| l != lengths[0]);
    assert!(has_variation, "flux should produce varying grain lengths");
}

#[test]
fn flux_is_seeded_from_the_clip() {
    let sample_rate = 48_000.0;
    let clip_a = 12345;
    let clip_b = 67890;

    let mut tex1 = Texture::new(2, sample_rate, clip_a);
    tex1.set_flux(50.0);

    let mut tex2 = Texture::new(2, sample_rate, clip_a);
    tex2.set_flux(50.0);

    let mut tex3 = Texture::new(2, sample_rate, clip_b);
    tex3.set_flux(50.0);

    let seq1: Vec<_> = (0..10).map(|_| tex1.grain_length_and_jitter()).collect();
    let seq2: Vec<_> = (0..10).map(|_| tex2.grain_length_and_jitter()).collect();
    let seq3: Vec<_> = (0..10).map(|_| tex3.grain_length_and_jitter()).collect();

    // Same seed gives identical sequences
    assert_eq!(seq1, seq2);
    // Different seed gives different sequences
    assert_ne!(seq1, seq3);
}

#[test]
fn grain_size_keeps_its_range_at_every_rate() {
    let mut texture = Texture::new(2, 192_000.0, 0);
    texture.set_flux(0.0);
    texture.set_grain_size(500.0);
    assert_eq!(texture.grain_length_and_jitter().0, 96_000);
    texture.set_grain_size(300.0);
    assert_eq!(texture.grain_length_and_jitter().0, 57_600);

    let mut texture = Texture::new(2, 48_000.0, 0);
    texture.set_flux(0.0);
    texture.set_grain_size(500.0);
    assert_eq!(texture.grain_length_and_jitter().0, 24_000);
}

#[test]
fn texture_keeps_a_bass_tones_waveform_at_96_khz() {
    for rate in [48_000.0, 96_000.0] {
        let mut texture = Texture::new(1, rate, 0);
        texture.set_flux(0.0);
        let (cents, spread) = bass_waveform(&mut texture, rate);
        assert!(cents.abs() < 5.0, "{rate} Hz: {cents} cents off");
        assert!(spread < 1.0, "{rate} Hz: level moves {spread} dB");
    }
}

#[test]
fn texture_pitch_search_work_is_bounded_in_every_block() {
    let rate = 48_000.0;
    let source = sine(220.0, rate, 4 * 48_000);
    let window = Window {
        data: &source,
        stride: source.len(),
        channels: 1,
        frames: source.len(),
        first: 0,
    };
    let positions: Vec<f64> = (0..48_000).map(f64::from).collect();
    let mut texture = Texture::new(1, rate, 0);
    texture.set_grain_size(10.0);
    let mut out = [0.0f32; 64];
    let (start, mut worst) = (diff_ops(), 0);
    for first in (0..positions.len()).step_by(64) {
        let before = diff_ops();
        let request = Request {
            positions: &positions[first..first + 64],
            first: first as i64,
            speed: 1.0,
            neutral: false,
            high_quality: false,
            beats: &Plan::EMPTY,
            transient_envelope: 100.0,
            pitch: 1.0,
            clip: 1,
            step: 1.0,
            song: f64::NAN,
        };
        texture.render(&request, &window, &mut out, 64);
        worst = worst.max(diff_ops() - before);
    }
    assert!(worst < 150_000, "one block did {worst} operations");
    assert!(
        diff_ops() - start < 5_000_000,
        "a second did {} operations",
        diff_ops() - start
    );
}

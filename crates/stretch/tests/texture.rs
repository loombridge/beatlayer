use bl_stretch::Texture;

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

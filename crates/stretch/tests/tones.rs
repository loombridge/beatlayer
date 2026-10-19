use bl_stretch::tones::{detect_period, grain_for_period};

#[test]
fn tones_grains_span_whole_periods_near_grain_size() {
    let sample_rate: f64 = 48_000.0;
    let freq: f64 = 440.0;
    let expected_period = (sample_rate / freq).round() as usize; // 109 samples

    // 440 Hz sine wave
    let frames = 2048;
    let sine: Vec<f32> = (0..frames)
        .map(|i| (2.0 * std::f32::consts::PI * freq as f32 * i as f32 / sample_rate as f32).sin())
        .collect();

    let detected = detect_period(&sine, sample_rate);
    assert_eq!(detected, Some(expected_period));

    // Nominal grain size 30 ms at 48 kHz = 1440 frames
    let nominal = (0.030 * sample_rate).round() as usize;
    let (len, hop) = grain_for_period(nominal, detected);

    // Grain spans an exact multiple of the detected period
    assert_eq!(len % expected_period, 0);
    assert_eq!(hop % expected_period, 0);

    // Grains span whole periods near grain_size
    let diff = (len as isize - nominal as isize).unsigned_abs();
    assert!(diff <= expected_period);
}

#[test]
fn unpitched_audio_takes_grain_size_grains() {
    let sample_rate = 48_000.0;
    // Flat or empty audio has no pitch
    let flat = vec![0.0f32; 2048];
    assert_eq!(detect_period(&flat, sample_rate), None);

    let nominal = (0.030 * sample_rate).round() as usize;
    let (len, hop) = grain_for_period(nominal, None);

    assert_eq!(len, nominal);
    assert_eq!(hop, nominal / 2);
}

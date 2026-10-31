use bl_stretch::beats::Plan;
use bl_stretch::tones::detect_period;
use bl_stretch::{Request, Tones, WarpMode, Window};

#[test]
fn a_pitch_change_resamples_the_next_grain() {
    let sample_rate: f64 = 48_000.0;
    let freq: f64 = 440.0;
    let input_period = (sample_rate / freq).round() as usize; // ~109 samples

    // Generate 440 Hz sine window
    let frames = 48_000;
    let sine: Vec<f32> = (0..frames)
        .map(|i| (2.0 * std::f32::consts::PI * freq as f32 * i as f32 / sample_rate as f32).sin())
        .collect();

    let window = Window {
        data: &sine,
        stride: frames,
        channels: 1,
        frames,
        first: 0,
    };

    let empty_plan = Plan::EMPTY;
    let positions: Vec<f64> = (0..4096).map(|i| i as f64).collect();

    // 1. Render at pitch = 1.0
    let mut tones1 = Tones::new(1, sample_rate);
    tones1.set_grain_size(30.0);
    let req1 = Request {
        positions: &positions,
        first: 0,
        speed: 1.0,
        neutral: false,
        high_quality: true,
        beats: &empty_plan,
        transient_envelope: 100.0,
        pitch: 1.0,
        clip: 1,
        step: 1.0,
        song: f64::NAN,
    };
    let mut out1 = vec![0.0f32; 4096];
    tones1.render(&req1, &window, &mut out1, 4096);

    let detected1 = detect_period(&out1[1000..3000], sample_rate);
    assert_eq!(detected1, Some(input_period));

    // 2. Render at pitch = 2.0 (one octave up, 880 Hz)
    let mut tones2 = Tones::new(1, sample_rate);
    tones2.set_grain_size(30.0);
    let req2 = Request {
        positions: &positions,
        first: 0,
        speed: 1.0,
        neutral: false,
        high_quality: true,
        beats: &empty_plan,
        transient_envelope: 100.0,
        pitch: 2.0,
        clip: 1,
        step: 1.0,
        song: f64::NAN,
    };
    let mut out2 = vec![0.0f32; 4096];
    tones2.render(&req2, &window, &mut out2, 4096);

    let expected_octave_period = (sample_rate / (freq * 2.0)).round() as usize; // ~55 samples
    let detected2 = detect_period(&out2[1000..3000], sample_rate);
    assert_eq!(detected2, Some(expected_octave_period));
}

#[test]
fn tones_starts_afresh_when_another_clip_takes_it_over_at_a_continuous_frame() {
    let mut tones = Tones::new(1, 48_000.0);
    let render = |tones: &mut Tones, level: f32, file_first: i64, first: i64, clip: u64| {
        let data = vec![level; 8192];
        let window = Window {
            data: &data,
            stride: 8192,
            channels: 1,
            frames: 8192,
            first: file_first,
        };
        let start = file_first as f64 + 4096.0;
        let positions: Vec<f64> = (0..2048).map(|i| start + i as f64).collect();
        let request = Request {
            positions: &positions,
            first,
            speed: 1.0,
            neutral: false,
            high_quality: false,
            beats: &Plan::EMPTY,
            transient_envelope: 100.0,
            pitch: 1.0,
            clip,
            step: 1.0,
            song: f64::NAN,
        };
        let mut out = vec![0.0f32; 2048];
        tones.render(&request, &window, &mut out, 2048);
        out
    };

    // Clip 1 plays far into its file; clip 2 then starts where its run ended.
    render(&mut tones, 1.0, 480_000, 0, 1);
    let out = render(&mut tones, 0.5, 0, 2048, 2);
    for (i, &s) in out.iter().enumerate().take(1441).skip(1) {
        assert!((s - 0.5).abs() < 1e-3, "frame {i} is {s}");
    }
}

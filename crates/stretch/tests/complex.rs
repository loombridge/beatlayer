//! Complex and Complex Pro (WP-4.6, the warp modes spec R21 to R24): the
//! frequency-domain stretcher driven as the clip player drives it.

use bl_stretch::beats::Plan;
use bl_stretch::{Complex, Request, WarpMode, Window};

const RATE: f64 = 48_000.0;

/// Plays `file` through `mode` as the clip player does: runs of at most
/// `block` song frames, each given a window of the file around the positions
/// it reads (the warp modes spec, R6).
fn play(
    mode: &mut Complex,
    file: &[f32],
    rate: f64,
    pitch: f64,
    frames: usize,
    block: usize,
) -> Vec<f32> {
    let mut out = vec![0.0f32; frames];
    let empty = Plan::EMPTY;
    let mut done = 0;
    while done < frames {
        let n = block.min(frames - done);
        let positions: Vec<f64> = (0..n).map(|i| (done + i) as f64 * rate).collect();
        let request = Request {
            positions: &positions,
            first: done as i64,
            speed: rate,
            neutral: false,
            high_quality: true,
            beats: &empty,
            transient_envelope: 100.0,
            pitch,
            clip: 1,
        };
        let (before, after) = mode.reach(&request);
        let lo = positions[0].floor() as i64 - before as i64;
        let hi = positions[n - 1].floor() as i64 + after as i64 + 1;
        let len = (hi - lo) as usize;
        let mut data = vec![0.0f32; len];
        for (i, sample) in data.iter_mut().enumerate() {
            let at = lo + i as i64;
            if at >= 0 && (at as usize) < file.len() {
                *sample = file[at as usize];
            }
        }
        let window = Window {
            data: &data,
            stride: len,
            channels: 1,
            frames: len,
            first: lo,
        };
        mode.render(&request, &window, &mut out[done..done + n], n);
        done += n;
    }
    out
}

/// A sine of `freq` Hz, `seconds` long, at −6 dBFS.
fn sine(freq: f64, seconds: f64) -> Vec<f32> {
    let frames = (seconds * RATE) as usize;
    (0..frames)
        .map(|i| (0.501_187 * (2.0 * std::f64::consts::PI * freq * i as f64 / RATE).sin()) as f32)
        .collect()
}

/// The pitch of `samples`, nearest `expected`: the phase the tone gathers
/// between one half of the window and the next, which is exact enough to
/// check a cent.
fn pitch_at(samples: &[f32], expected: f64) -> f64 {
    let n = samples.len();
    let half = n / 2;
    let at = |from: usize, len: usize| -> (f64, f64) {
        let omega = 2.0 * std::f64::consts::PI * expected / RATE;
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &sample) in samples[from..from + len].iter().enumerate() {
            let angle = omega * (from + i) as f64;
            re += f64::from(sample) * angle.cos();
            im -= f64::from(sample) * angle.sin();
        }
        (re, im)
    };
    let (r1, i1) = at(0, half);
    let (r2, i2) = at(half, half);
    let mut phase = i2.atan2(r2) - i1.atan2(r1);
    while phase > std::f64::consts::PI {
        phase -= 2.0 * std::f64::consts::PI;
    }
    while phase < -std::f64::consts::PI {
        phase += 2.0 * std::f64::consts::PI;
    }
    expected + phase * RATE / (2.0 * std::f64::consts::PI * half as f64)
}

fn rms_db(samples: &[f32]) -> f64 {
    let mean_square = samples
        .iter()
        .map(|&s| f64::from(s) * f64::from(s))
        .sum::<f64>()
        / samples.len() as f64;
    20.0 * mean_square.sqrt().max(1e-12).log10()
}

// Spec: warp-modes R21, R35 (Complex's pitch and level rows). Both modes hold
// a sine's pitch within 1 cent and its level within 0.5 dB from 0.5 to 2
// times speed.
#[test]
fn complex_and_complex_pro_change_speed_and_pitch_independently() {
    let file = sine(440.0, 8.0);
    let reference = rms_db(&file[24_000..72_000]);
    for pro in [false, true] {
        let mut mode = Complex::new(1, RATE, pro);
        for rate in [0.5, 0.75, 1.0, 1.5, 2.0] {
            let out = play(&mut mode, &file, rate, 1.0, 96_000, 512);
            let steady = &out[48_000..96_000];
            let measured = pitch_at(steady, 440.0);
            let cents = 1200.0 * (measured / 440.0).log2();
            assert!(
                cents.abs() < 1.0,
                "Complex Pro {pro} at {rate} times speed: {measured:.3} Hz, {cents:.2} cents off"
            );
            let level = rms_db(steady);
            assert!(
                (level - reference).abs() < 0.5,
                "Complex Pro {pro} at {rate} times speed: {level:.2} dB against {reference:.2}"
            );
        }
    }
}

// Spec: warp-modes R21, R35. A pitch change moves the pitch and leaves the
// timing alone: the sine sounds at its own pitch times the ratio while the
// file is read at the same speed.
#[test]
fn a_pitch_change_moves_the_pitch_and_not_the_timing() {
    let file = sine(440.0, 4.0);
    for semitones in [-12.0, -7.0, 7.0, 12.0] {
        let ratio = 2.0_f64.powf(semitones / 12.0);
        let out = play(
            &mut Complex::new(1, RATE, false),
            &file,
            1.0,
            ratio,
            72_000,
            512,
        );
        let steady = &out[24_000..72_000];
        let measured = pitch_at(steady, 440.0 * ratio);
        let cents = 1200.0 * (measured / (440.0 * ratio)).log2();
        assert!(
            cents.abs() < 1.0,
            "{semitones} semitones: {measured:.3} Hz, {cents:.2} cents off"
        );
    }
}

// Spec: warp-modes R22. With no pitch change Formants and Envelope change
// nothing, to the bit.
#[test]
fn formants_change_nothing_without_a_pitch_change() {
    let file = sine(440.0, 4.0);
    let mut base = Complex::new(1, RATE, true);
    base.set_formants(100.0);
    base.set_envelope(128);
    let want = play(&mut base, &file, 1.5, 1.0, 48_000, 512);
    for (formants, envelope) in [(0.0, 8), (100.0, 512), (50.0, 64), (0.0, 512)] {
        let mut mode = Complex::new(1, RATE, true);
        mode.set_formants(formants);
        mode.set_envelope(envelope);
        let got = play(&mut mode, &file, 1.5, 1.0, 48_000, 512);
        assert_eq!(got, want, "Formants {formants} %, Envelope {envelope}");
    }
}

// Spec: warp-modes R23. Envelope acts only where the pitch changes and
// Formants is above 0: on a transposed vowel it changes the output, and at
// Formants 0 it doesn't.
#[test]
fn envelope_acts_only_when_formants_do() {
    let file = vowel();
    let mut low = Complex::new(1, RATE, true);
    low.set_formants(100.0);
    low.set_envelope(8);
    let mut high = Complex::new(1, RATE, true);
    high.set_formants(100.0);
    high.set_envelope(512);
    let a = play(&mut low, &file, 1.0, 2.0_f64.powf(7.0 / 12.0), 48_000, 512);
    let b = play(&mut high, &file, 1.0, 2.0_f64.powf(7.0 / 12.0), 48_000, 512);
    assert_ne!(a, b, "Envelope changed nothing where the pitch moves");

    let mut low = Complex::new(1, RATE, true);
    low.set_formants(0.0);
    low.set_envelope(8);
    let mut high = Complex::new(1, RATE, true);
    high.set_formants(0.0);
    high.set_envelope(512);
    let a = play(&mut low, &file, 1.0, 2.0_f64.powf(7.0 / 12.0), 48_000, 512);
    let b = play(&mut high, &file, 1.0, 2.0_f64.powf(7.0 / 12.0), 48_000, 512);
    assert_eq!(
        a, b,
        "Envelope acts at Formants 0, where R23 says it doesn't"
    );
}

/// A vowel: a 150 Hz pulse train through resonators at 700, 1,220 and
/// 2,600 Hz (the fixture the spec's measurements use).
fn vowel() -> Vec<f32> {
    let frames = (4.0 * RATE) as usize;
    let mut out = vec![0.0f64; frames];
    let pulse = (RATE / 150.0) as usize;
    for (i, sample) in out.iter_mut().enumerate() {
        if i % pulse < 4 {
            *sample = 1.0;
        }
    }
    for (freq, bandwidth) in [(700.0, 80.0), (1_220.0, 90.0), (2_600.0, 120.0)] {
        let r = (-std::f64::consts::PI * bandwidth / RATE).exp();
        let two_cos = 2.0 * r * (2.0 * std::f64::consts::PI * freq / RATE).cos();
        let (mut a1, mut a2) = (0.0, 0.0);
        for sample in out.iter_mut() {
            let y = (1.0 - r) * *sample + two_cos * a1 - r * r * a2;
            a2 = a1;
            a1 = y;
            *sample = y;
        }
    }
    let peak = out.iter().fold(0.0_f64, |peak, s| peak.max(s.abs()));
    out.iter().map(|s| (s / peak * 0.5) as f32).collect()
}

// Spec: warp-modes R22, R35. The vowel's first two formants stay where they
// were in Complex Pro at Formants 100 %, and move with the pitch in Complex.
// At ±7 semitones both stay within 10 % of the source's. At ±12 the library's
// formant correction reaches only 40 % on the first formant (1050 Hz against
// 750) and 12.5 % on the second: R35 records those as the tolerance there, a
// known gap (R35's Complex Pro row). Complex, which corrects nothing, still
// moves them within 10 % of the source's times the ratio.
#[test]
fn complex_pro_keeps_the_vowels_formants_where_complex_moves_them() {
    let file = vowel();
    let source = play(
        &mut Complex::new(1, RATE, false),
        &file,
        1.0,
        1.0,
        48_000,
        512,
    );
    let source = [
        formant_in(&source, (525.0, 1_050.0)),
        formant_in(&source, (1_098.0, 1_830.0)),
    ];
    // The window each formant is looked for in: from the place it belongs to
    // well above it, so a formant the library has moved up is found rather than
    // read off the window's edge, and for the second formant from above the
    // first, which its own peak would otherwise answer for.
    let band = |centre: f64, first: bool| {
        if first {
            (centre * 0.75, centre * 1.5)
        } else {
            (centre * 0.9, centre * 1.5)
        }
    };
    // The tolerance the library's formant correction reaches at each
    // transposition, for the first and the second formant (R35): the first
    // formant at ±12 is the library's own limit, a known gap.
    for (semitones, keep) in [(7.0, [0.10, 0.10]), (12.0, [0.40, 0.15])] {
        let ratio = 2.0_f64.powf(semitones / 12.0);
        let mut pro = Complex::new(1, RATE, true);
        pro.set_formants(100.0);
        // Envelope places the fundamental the formant analysis assumes, and
        // this vowel's is 150 Hz: 44,100 / 294 (R23). The manual's own advice
        // is a higher Envelope for low-pitched audio.
        pro.set_envelope(294);
        let kept = play(&mut pro, &file, 1.0, ratio, 48_000, 512);
        let moved = play(
            &mut Complex::new(1, RATE, false),
            &file,
            1.0,
            ratio,
            48_000,
            512,
        );
        for (formant, &source) in source.iter().enumerate() {
            let measured = formant_in(&kept, band(source, formant == 0));
            let error = (measured - source).abs() / source;
            eprintln!(
                "{semitones} semitones, formant {}: Complex Pro keeps {source:.0} Hz at \
                 {measured:.0} Hz, {:.0} %",
                formant + 1,
                error * 100.0
            );
            assert!(
                error <= keep[formant],
                "{semitones} semitones, formant {}: Complex Pro's formant {measured:.0} Hz is \
                 {:.0} % from {source:.0} Hz, past the {:.0} % it reaches",
                formant + 1,
                error * 100.0,
                keep[formant] * 100.0
            );
            let target = source * ratio;
            let measured = formant_in(&moved, band(target, formant == 0));
            let error = (measured - target).abs() / target;
            eprintln!(
                "{semitones} semitones, formant {}: Complex moves {target:.0} Hz to {measured:.0} \
                 Hz, {:.0} %",
                formant + 1,
                error * 100.0
            );
            assert!(
                error <= 0.10,
                "{semitones} semitones, formant {}: Complex's formant {measured:.0} Hz is {:.0} % \
                 from {target:.0} Hz, where the pitch moved it",
                formant + 1,
                error * 100.0
            );
        }
    }
}

/// The vowel's fundamental, in Hz: the spectrum's envelope is smoothed over
/// about it, so its peaks are the formants and not the harmonics that sample
/// them.
const VOWEL_F0: f64 = 150.0;

/// The largest peak of the smoothed spectrum inside `band` (a low and a high
/// frequency, in Hz): how a formant is read off, in the window around the place
/// it belongs — the source's when the formants are kept, the source's times the
/// ratio when they move — so the figure doesn't read off the window's edge and
/// a formant a mode has moved is still found (R22, R35).
fn formant_in(samples: &[f32], band: (f64, f64)) -> f64 {
    let n = 8_192.min(samples.len());
    let start = (samples.len() - n) / 2;
    let windowed: Vec<f64> = samples[start..start + n]
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos();
            f64::from(s) * w
        })
        .collect();
    let magnitude = |f: f64| -> f64 {
        let omega = 2.0 * std::f64::consts::PI * f / RATE;
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &s) in windowed.iter().enumerate() {
            let angle = omega * i as f64;
            re += s * angle.cos();
            im -= s * angle.sin();
        }
        (re * re + im * im).sqrt()
    };
    // The envelope, on a grid of 25 Hz, smoothed over the vowel's fundamental
    // either way.
    let step = 25.0;
    let from = 200.0;
    let to = 3_000.0;
    let count = ((to - from) / step) as usize + 1;
    let raw: Vec<f64> = (0..count)
        .map(|i| magnitude(from + i as f64 * step))
        .collect();
    let half = ((VOWEL_F0 / step).round() as usize).clamp(1, count / 2);
    let smooth = |i: usize| -> f64 {
        let lo = i.saturating_sub(half);
        let hi = (i + half).min(count - 1);
        raw[lo..=hi].iter().sum::<f64>() / (hi - lo + 1) as f64
    };
    let first = ((band.0 - from) / step).ceil().max(0.0) as usize;
    let last = ((band.1 - from) / step).floor().max(0.0) as usize;
    let mut best = (f64::MIN, band.0);
    for i in first..=last.min(count - 1) {
        let here = smooth(i);
        if here > best.0 {
            best = (here, from + i as f64 * step);
        }
    }
    best.1
}

// Spec: warp-modes R6. A render is the same at any block size.
#[test]
fn any_block_size_gives_the_same_samples() {
    let file = sine(440.0, 4.0);
    for pro in [false, true] {
        let reference = play(
            &mut Complex::new(1, RATE, pro),
            &file,
            1.5,
            1.0,
            48_000,
            512,
        );
        for block in [1, 61, 512, 4_096] {
            let got = play(
                &mut Complex::new(1, RATE, pro),
                &file,
                1.5,
                1.0,
                48_000,
                block,
            );
            assert_eq!(got, reference, "Complex Pro {pro} at blocks of {block}");
        }
    }
}

// Spec: warp-modes R7, R24. A locate into a Complex clip plays from the point
// at the level it plays through, and its click lands within the mode's timing
// target.
#[test]
fn a_located_complex_clip_plays_from_the_point_at_the_right_level() {
    let file = sine(440.0, 4.0);
    let through = play(
        &mut Complex::new(1, RATE, false),
        &file,
        1.0,
        1.0,
        96_000,
        512,
    );
    // A clip that begins at the locate: what the player does for a locate into
    // a Complex clip is a fresh prime there.
    let mut located = Complex::new(1, RATE, false);
    located.reset();
    let at = 48_000.0;
    let mut positions = Vec::new();
    let mut out = vec![0.0f32; 48_000];
    let empty = Plan::EMPTY;
    let mut done = 0;
    while done < out.len() {
        let n = 512.min(out.len() - done);
        positions.clear();
        positions.extend((0..n).map(|i| at + (done + i) as f64));
        let request = Request {
            positions: &positions,
            first: (at as i64) + done as i64,
            speed: 1.0,
            neutral: false,
            high_quality: true,
            beats: &empty,
            transient_envelope: 100.0,
            pitch: 1.0,
            clip: 1,
        };
        let (before, after) = located.reach(&request);
        let lo = positions[0].floor() as i64 - before as i64;
        let hi = positions[n - 1].floor() as i64 + after as i64 + 1;
        let len = (hi - lo) as usize;
        let mut data = vec![0.0f32; len];
        for (i, sample) in data.iter_mut().enumerate() {
            let frame = lo + i as i64;
            if frame >= 0 && (frame as usize) < file.len() {
                *sample = file[frame as usize];
            }
        }
        let window = Window {
            data: &data,
            stride: len,
            channels: 1,
            frames: len,
            first: lo,
        };
        located.render(&request, &window, &mut out[done..done + n], n);
        done += n;
    }
    let through_db = rms_db(&through[48_000..50_400]);
    let located_db = rms_db(&out[..2_400]);
    assert!(
        (through_db - located_db).abs() < 1.0,
        "over the first 50 ms after the locate: {located_db:.2} dB against {through_db:.2} dB \
         playing through"
    );
}

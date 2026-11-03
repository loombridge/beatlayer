//! Resonators (WP-8.43; the resonators spec): the input filter, the five
//! combs' pitches and ring, Const, Color, Mode, Width, the scale and the
//! tuning, and the renders at every block size. Each block runs as a
//! real-time section under the allocation trap.

use std::f64::consts::TAU;

use crate::common;
use crate::common::response::{Kind, biquad_response, db as complex_db, rbj, response_at};
use bl_core::time::SampleRate;
use bl_devices::resonators::{
    self, COLOR, CONST, DECAY, DRY_WET, FILTER_FREQ, FILTER_TYPE, FINE, GAIN, LEVEL, MODE, ON,
    PITCH, USE_SCALE, WIDTH, resonators_type, voice_param,
};
use bl_engine::scales::ScaleRuns;
use bl_engine::{Block, Collector, KeyPitches, Node, NodeConfig, NodeEvent, TransportInfo};
use bl_model::{ParamId, Scale, ScaleName, Tuning};

/// The block sizes every render must agree at (R12).
const BLOCKS: [usize; 4] = [1, 64, 512, 4_096];

/// The frequency of key `key` of twelve equal steps, A3 = 440 Hz at 69.
fn hz(key: f64) -> f64 {
    440.0 * ((key - 69.0) / 12.0).exp2()
}

fn cents(hz: f64, from: f64) -> f64 {
    1200.0 * (hz / from).log2()
}

/// A render of the device: its node and the rate it runs at.
struct Rig {
    node: Box<dyn Node>,
    rate: SampleRate,
    collector: Collector,
}

impl Rig {
    fn new(rate: SampleRate, params: &[(ParamId, f64)]) -> Self {
        let device_type = resonators_type();
        let config = NodeConfig {
            round_trip_latency: 0,
            sample_rate: rate,
            max_frames: 4_096,
            channels: 2,
        };
        let node = common::registry()
            .create(&common::device(&device_type, params), &config)
            .expect("the registry makes Resonators");
        Self {
            node,
            rate,
            collector: Collector::new(),
        }
    }

    fn tune(&mut self, tuning: Option<&Tuning>) {
        self.node
            .receive_data(self.collector.handle().data(KeyPitches::of(tuning)));
    }

    /// Runs `input` (both channels) in blocks of `block` frames, with
    /// `events` (frames from the start), `scale_at` giving the scale at a
    /// frame (for a device that counts degrees), and `hidden` for a set whose
    /// tuning hides the scales. Returns each channel.
    fn render(
        &mut self,
        input: &[f32],
        block: usize,
        events: &[(usize, ParamId, f64)],
        scale_at: impl Fn(usize) -> Scale,
        hidden: bool,
    ) -> Vec<Vec<f32>> {
        let mut out = vec![Vec::new(); 2];
        for start in (0..input.len()).step_by(block) {
            let end = (start + block).min(input.len());
            let len = end - start;
            let here: Vec<NodeEvent> = events
                .iter()
                .filter(|e| e.0 >= start && e.0 < end)
                .map(|&(at, param, value)| common::set((at - start) as u32, param, value))
                .collect();
            let mut runs = ScaleRuns::of(scale_at(start));
            for offset in 1..len {
                runs.push(offset as u32, scale_at(start + offset));
            }
            let mut data: Vec<f32> = (0..2).flat_map(|_| input[start..end].to_vec()).collect();
            {
                let mut block = Block::new(
                    &mut data,
                    2,
                    len,
                    len,
                    &here,
                    TransportInfo::default(),
                    self.rate,
                )
                .with_scale(runs)
                .with_scales_hidden(hidden);
                bl_rt::rt_test!({ self.node.process(&mut block) });
            }
            for (channel, values) in out.iter_mut().zip(data.chunks(len)) {
                channel.extend_from_slice(values);
            }
        }
        out
    }

    /// The left channel's answer to an impulse, `frames` long.
    fn impulse(&mut self, frames: usize) -> Vec<f32> {
        let mut input = vec![0.0f32; frames];
        input[0] = 1.0;
        self.render(&input, 4_096, &[], |_| Scale::default(), false)
            .swap_remove(0)
    }
}

/// Resonator I alone, at `note`, Color 0, the filter open at the top: the
/// rest of a test's parameters follow.
fn alone(note: u32, decay: f64, rest: &[(ParamId, f64)]) -> Vec<(ParamId, f64)> {
    let mut params = vec![
        (FILTER_TYPE, 0.0),
        (FILTER_FREQ, 18_000.0),
        (DECAY, decay),
        (voice_param(0, PITCH), f64::from(note)),
        (voice_param(1, ON), 0.0),
        (voice_param(2, ON), 0.0),
    ];
    params.extend_from_slice(rest);
    params
}

/// `params` with only resonator `voice` on.
fn only(voice: usize, params: &[(ParamId, f64)]) -> Vec<(ParamId, f64)> {
    let mut params = params.to_vec();
    params.extend((0..5).map(|v| (voice_param(v, ON), f64::from(u8::from(v == voice)))));
    params
}

/// The amplitude of `ir` at `at` hertz.
fn amplitude(ir: &[f32], at: f64, rate: SampleRate) -> f64 {
    let w = TAU * at / rate.as_f64();
    let (mut re, mut im) = (0.0, 0.0);
    for (n, sample) in ir.iter().enumerate() {
        let angle = w * n as f64;
        re += f64::from(*sample) * angle.cos();
        im -= f64::from(*sample) * angle.sin();
    }
    re.hypot(im)
}

/// The frequency near `near` (within `span` cents either side) where `ir`'s
/// spectrum peaks, by golden-section search.
fn peak(ir: &[f32], near: f64, span: f64, rate: SampleRate) -> f64 {
    let (mut low, mut high) = (
        near * (-span / 1200.0).exp2(),
        near * (span / 1200.0).exp2(),
    );
    let ratio = (5f64.sqrt() - 1.0) / 2.0;
    let (mut a, mut b) = (high - ratio * (high - low), low + ratio * (high - low));
    let (mut fa, mut fb) = (amplitude(ir, a, rate), amplitude(ir, b, rate));
    for _ in 0..40 {
        if fa < fb {
            low = a;
            (a, fa) = (b, fb);
            b = low + ratio * (high - low);
            fb = amplitude(ir, b, rate);
        } else {
            high = b;
            (b, fb) = (a, fa);
            a = high - ratio * (high - low);
            fa = amplitude(ir, a, rate);
        }
    }
    (low + high) / 2.0
}

/// How long the ring of `ir` takes to fall 60 dB, from the slope of its
/// level over windows of `window` frames, between −10 and −40 dB of its
/// first window.
fn ring_time(ir: &[f32], window: usize, rate: SampleRate) -> f64 {
    let level: Vec<f64> = ir
        .chunks_exact(window)
        .map(|w| {
            let power = w.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / window as f64;
            10.0 * power.max(1e-30).log10()
        })
        .collect();
    let top = level[0..4].iter().copied().fold(f64::MIN, f64::max);
    let at = |target: f64| level.iter().position(|l| *l < top + target).unwrap();
    let (a, b) = (at(-10.0), at(-40.0));
    let slope = (level[a] - level[b]) / ((b - a) * window) as f64 * rate.as_f64();
    60.0 / slope
}

// Spec: resonators R3, R6. Each resonator's strongest resonance is within 1
// cent of its pitch, from C-1 to C5, at three sample rates.
#[test]
fn a_resonators_peak_is_within_a_cent_of_its_pitch_from_c_minus_1_to_c5() {
    for rate in [
        SampleRate::HZ_44100,
        SampleRate::HZ_48000,
        SampleRate::HZ_96000,
    ] {
        for key in [0u32, 12, 24, 36, 48, 60, 72, 84] {
            let mut rig = Rig::new(rate, &alone(key, 1.0, &[]));
            let ir = rig.impulse(6 * rate.hz() as usize);
            let expected = hz(f64::from(key));
            let found = peak(&ir, expected, 100.0, rate);
            assert!(
                cents(found, expected).abs() < 1.0,
                "key {key} at {rate:?}: {} cents off",
                cents(found, expected)
            );
        }
    }
}

// Spec: resonators R3. Fine moves the pitch in cents, and Pitch moves II to V
// by semitones from I.
#[test]
fn fine_and_pitch_set_the_pitches() {
    let rate = SampleRate::HZ_48000;
    let params = [
        (FILTER_TYPE, 0.0),
        (FILTER_FREQ, 18_000.0),
        (DECAY, 2.0),
        (voice_param(0, PITCH), 48.0),
        (voice_param(0, FINE), 30.0),
        (voice_param(1, PITCH), 7.0),
        (voice_param(1, FINE), -20.0),
        (voice_param(2, PITCH), 19.0),
        (voice_param(3, PITCH), -12.0),
        (voice_param(4, PITCH), 24.0),
    ];
    for (voice, expected) in [
        (0, hz(48.3)),
        (1, hz(55.0 - 0.2)),
        (2, hz(67.0)),
        (3, hz(36.0)),
        (4, hz(72.0)),
    ] {
        let ir = Rig::new(rate, &only(voice, &params)).impulse(6 * 48_000);
        let found = peak(&ir, expected, 40.0, rate);
        assert!(cents(found, expected).abs() < 1.0, "resonator {voice}");
    }
}

// Spec: resonators R7. Decay sets the ring's fall of 60 dB, within 10 %.
#[test]
fn decay_sets_the_ring_time_within_ten_per_cent() {
    let rate = SampleRate::HZ_48000;
    for decay in [0.3, 1.0, 3.0] {
        let mut rig = Rig::new(rate, &alone(48, decay, &[]));
        let ir = rig.impulse((decay * 3.0 * 48_000.0) as usize + 48_000);
        let window = (rate.as_f64() / hz(48.0)).round() as usize;
        let ring = ring_time(&ir, window, rate);
        assert!(
            (ring / decay - 1.0).abs() < 0.1,
            "Decay {decay} s rings {ring} s"
        );
    }
}

// Spec: resonators R7. Without Const the higher resonators ring shorter; with
// it every resonator rings for Decay.
#[test]
fn const_keeps_the_ring_time_the_same_at_every_pitch() {
    let rate = SampleRate::HZ_48000;
    let ring_of_ii = |constant: f64, pitch: f64| {
        // Resonator II alone, `pitch` semitones above I.
        let params = [
            (FILTER_TYPE, 0.0),
            (FILTER_FREQ, 18_000.0),
            (DECAY, 1.0),
            (CONST, constant),
            (voice_param(0, PITCH), 48.0),
            (voice_param(0, ON), 0.0),
            (voice_param(1, PITCH), pitch),
            (voice_param(2, ON), 0.0),
        ];
        let ir = Rig::new(rate, &params).impulse(4 * 48_000);
        let window = (rate.as_f64() / hz(48.0 + pitch)).round() as usize;
        ring_time(&ir, window, rate)
    };
    // An octave above I rings half as long without Const, as long with it.
    let (off, on) = (ring_of_ii(0.0, 12.0), ring_of_ii(1.0, 12.0));
    assert!((off / 0.5 - 1.0).abs() < 0.1, "without Const: {off} s");
    assert!((on - 1.0).abs() < 0.1, "with Const: {on} s");
    // Two octaves above: a quarter.
    let off = ring_of_ii(0.0, 24.0);
    assert!(
        (off / 0.25 - 1.0).abs() < 0.1,
        "two octaves, no Const: {off} s"
    );
}

// Spec: resonators R8. Color above zero lifts the highs by up to 6 dB at 10
// times the pitch, within 0.5 dB, and leaves the pitch where it is.
#[test]
fn color_tilts_the_loops_within_half_a_decibel() {
    let rate = SampleRate::HZ_48000;
    let f0 = hz(48.0);
    let level = |color: f64, at: f64| {
        let mut rig = Rig::new(rate, &alone(48, 2.0, &[(COLOR, color)]));
        let ir = rig.impulse(8 * 48_000);
        let near = peak(&ir, at, 30.0, rate);
        (complex_db(response_at(&ir, near)), near)
    };
    for (color, boost) in [(100.0, 6.0), (50.0, 3.0), (25.0, 1.5)] {
        let gained = level(color, 10.0 * f0).0 - level(0.0, 10.0 * f0).0;
        assert!((gained - boost).abs() < 0.5, "Color {color}: {gained} dB");
        let (_, pitch) = level(color, f0);
        assert!(
            cents(pitch, f0).abs() < 1.0,
            "Color {color} moved the pitch"
        );
    }
    // Dark: the loop's low-pass takes the highs down and the ring shorter.
    for color in [-100.0, -50.0] {
        let mut rig = Rig::new(rate, &alone(48, 1.0, &[(COLOR, color)]));
        let ir = rig.impulse(4 * 48_000);
        let window = (rate.as_f64() / f0).round() as usize;
        assert!(ring_time(&ir, window, rate) < 1.0, "Color {color}");
        let (fundamental, third) = (
            complex_db(response_at(&ir, peak(&ir, f0, 30.0, rate))),
            complex_db(response_at(&ir, peak(&ir, 3.0 * f0, 30.0, rate))),
        );
        // Written out from the design: the comb's peak is its input scale
        // over one less its loop gain at the harmonic, which the low-pass
        // lowers away from the pitch.
        let dark = -color / 100.0;
        let pole = (-TAU * 2.0 * f0 / dark / 48_000.0).exp();
        let loss = |at: f64| {
            let w = TAU * at / 48_000.0;
            (1.0 - pole) / (1.0 - 2.0 * pole * w.cos() + pole * pole).sqrt()
        };
        let feedback = (-6.907_755_278_982_137 / (1.0 * f0)).exp();
        let expected =
            20.0 * ((1.0 - feedback * loss(f0)) / (1.0 - feedback * loss(3.0 * f0))).log10();
        assert!(
            ((third - fundamental) - expected).abs() < 0.5,
            "Color {color}: third harmonic {} dB below, design says {}",
            fundamental - third,
            -expected
        );
    }
}

/// The textbook band-pass and notch of the cookbook at Q = 1/√2.
fn textbook(kind: usize, f0: f64, hz: f64) -> f64 {
    let w0 = TAU * f0 / 48_000.0;
    let (sin, cos) = w0.sin_cos();
    let alpha = sin / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
    let filter = match kind {
        0 => rbj(Kind::LowPass, f0, 0.0, std::f64::consts::FRAC_1_SQRT_2),
        2 => rbj(Kind::HighPass, f0, 0.0, std::f64::consts::FRAC_1_SQRT_2),
        1 => ([alpha, 0.0, -alpha], [1.0 + alpha, -2.0 * cos, 1.0 - alpha]),
        _ => (
            [1.0, -2.0 * cos, 1.0],
            [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
        ),
    };
    complex_db(biquad_response(filter, hz))
}

// Spec: resonators R1, R2. The input filter's four types measure within 0.5
// dB of their design. With Decay at its least and Resonator I at C-1 the
// comb passes the filter's output as it is, at the centre's level less 3 dB.
#[test]
fn the_input_filter_measures_within_half_a_decibel_of_its_design() {
    let rate = SampleRate::HZ_48000;
    for kind in 0..4 {
        for f0 in [200.0, 1_000.0, 5_000.0] {
            let params = [
                (FILTER_TYPE, kind as f64),
                (FILTER_FREQ, f0),
                (DECAY, 0.01),
                (voice_param(0, PITCH), 0.0),
                (voice_param(1, ON), 0.0),
                (voice_param(2, ON), 0.0),
            ];
            let ir = Rig::new(rate, &params).impulse(8_192);
            for at in [f0 / 4.0, f0 / 1.5, f0 * 1.5, f0 * 4.0] {
                let measured = complex_db(response_at(&ir, at));
                // Resonator I sits in the centre: 3 dB down in each channel.
                let designed = textbook(kind, f0, at) - 3.0103;
                assert!(
                    (measured - designed).abs() < 0.5,
                    "type {kind} at {f0} Hz, read at {at}: {measured} against {designed}"
                );
            }
        }
    }
}

// Spec: resonators R9. Mode B adds a second comb a fifth above, 12 dB lower;
// Mode A has nothing there.
#[test]
fn mode_b_adds_a_fifth_twelve_decibels_down() {
    let rate = SampleRate::HZ_48000;
    let f0 = hz(36.0);
    let at = |mode: f64| {
        let mut rig = Rig::new(rate, &alone(36, 10.0, &[(MODE, mode)]));
        let ir = rig.impulse(12 * 48_000);
        let (root, fifth) = (peak(&ir, f0, 5.0, rate), peak(&ir, 1.5 * f0, 40.0, rate));
        (
            complex_db(response_at(&ir, root)),
            complex_db(response_at(&ir, fifth)),
            fifth,
        )
    };
    let (root_b, fifth_b, found) = at(1.0);
    assert!(cents(found, 1.5 * f0).abs() < 2.0, "the fifth's pitch");
    assert!(
        ((root_b - fifth_b) - 12.0).abs() < 0.5,
        "{} dB below",
        root_b - fifth_b
    );
    let (root_a, fifth_a, _) = at(0.0);
    assert!(root_a - fifth_a > 20.0, "Mode A has no fifth");
}

/// The level of `channel` of a render of resonator `voice` alone at Width
/// `width`, for noise.
fn placed(voice: usize, width: f64) -> (f64, f64) {
    let mut params = vec![
        (FILTER_TYPE, 1.0),
        (FILTER_FREQ, 1_000.0),
        (DECAY, 0.5),
        (WIDTH, width),
    ];
    for other in 0..5 {
        params.push((voice_param(other, ON), f64::from(u8::from(other == voice))));
    }
    let mut rig = Rig::new(SampleRate::HZ_48000, &params);
    let out = rig.render(
        &common::noise(48_000),
        512,
        &[],
        |_| Scale::default(),
        false,
    );
    let rms =
        |c: &[f32]| (c.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / c.len() as f64).sqrt();
    (rms(&out[0]), rms(&out[1]))
}

// Spec: resonators R10. At Width 100 % I is in the centre, II and IV left, III
// and V right; at 0 all are in the centre; between, in proportion.
#[test]
fn width_places_the_resonators() {
    let (l, r) = placed(0, 100.0);
    assert!((l / r - 1.0).abs() < 0.01, "I in the centre");
    for voice in [1, 3] {
        let (l, r) = placed(voice, 100.0);
        assert!(l > 100.0 * r, "{voice} is on the left");
    }
    for voice in [2, 4] {
        let (l, r) = placed(voice, 100.0);
        assert!(r > 100.0 * l, "{voice} is on the right");
    }
    for voice in 0..5 {
        let (l, r) = placed(voice, 0.0);
        assert!((l / r - 1.0).abs() < 0.01, "{voice} in the centre at 0 %");
    }
    // Half way, II is a quarter turn of the way to the left: the channels
    // are in the ratio of the cosine and the sine of 3/8 of a quarter circle.
    let (l, r) = placed(1, 50.0);
    let angle = 0.25 * std::f64::consts::FRAC_PI_2;
    assert!((r / l - angle.tan()).abs() < 0.01, "{}", r / l);
}

// Spec: resonators R3, R10. Dry/Wet balances the resonators against the dry
// signal, and Gain scales the resonators.
#[test]
fn dry_wet_and_gain_set_the_output() {
    let input = common::noise(4_096);
    let go = |params: &[(ParamId, f64)]| {
        Rig::new(SampleRate::HZ_48000, params).render(&input, 512, &[], |_| Scale::default(), false)
    };
    let dry = go(&[(DRY_WET, 0.0)]);
    assert_eq!(dry[0], input, "0 % is the dry signal");
    let wet = go(&[(DRY_WET, 100.0), (GAIN, 1.0)]);
    let quiet = go(&[(DRY_WET, 100.0), (GAIN, 0.5)]);
    for (a, b) in wet[0].iter().zip(&quiet[0]) {
        assert!((a - 2.0 * b).abs() < 1e-5);
    }
    let silent = go(&[(DRY_WET, 100.0), (GAIN, 0.0)]);
    assert!(silent[0].iter().all(|s| *s == 0.0));
}

/// D minor, with Scale Mode on.
fn d_minor() -> Scale {
    Scale::new(2, ScaleName::Minor)
}

// Spec: resonators R4. With Use Current Scale on, Note counts degrees of the
// scale from its root in the octave of C-1, and Pitch degrees from I's note.
#[test]
fn use_current_scale_counts_degrees_of_a_d_minor_clip() {
    let rate = SampleRate::HZ_48000;
    // Note 23 is the third degree (F) three octaves above the root: key
    // 2 + 36 + 3 = 41; Pitch +2 is two degrees further: A, key 45.
    let params = [
        (FILTER_TYPE, 0.0),
        (FILTER_FREQ, 18_000.0),
        (DECAY, 2.0),
        (USE_SCALE, 1.0),
        (voice_param(0, PITCH), 23.0),
        (voice_param(1, PITCH), 2.0),
    ];
    let mut input = vec![0.0f32; 6 * 48_000];
    input[0] = 1.0;
    let render = |params: &[(ParamId, f64)], voice| {
        Rig::new(rate, &only(voice, params))
            .render(&input, 512, &[], |_| d_minor(), false)
            .swap_remove(0)
    };
    for (voice, key) in [(0, 41.0), (1, 45.0)] {
        let found = peak(&render(&params, voice), hz(key), 40.0, rate);
        assert!(cents(found, hz(key)).abs() < 1.0, "resonator {voice}");
    }
    // Off, the same numbers are semitones: key 23, and 25 above it.
    let off: Vec<_> = params
        .iter()
        .map(|&p| {
            if p.0 == USE_SCALE {
                (USE_SCALE, 0.0)
            } else {
                p
            }
        })
        .collect();
    for (voice, key) in [(0, 23.0), (1, 25.0)] {
        let found = peak(&render(&off, voice), hz(key), 40.0, rate);
        assert!(cents(found, hz(key)).abs() < 1.0, "resonator {voice}, off");
    }
}

// Spec: resonators R4. A change of the clip's scale retunes the resonators
// from the frame it happens, gliding over 20 ms, at every block size.
#[test]
fn a_scale_change_retunes_from_its_frame() {
    let rate = SampleRate::HZ_48000;
    let params = [
        (FILTER_TYPE, 0.0),
        (FILTER_FREQ, 18_000.0),
        (DECAY, 2.0),
        (USE_SCALE, 1.0),
        (voice_param(0, PITCH), 23.0),
        (voice_param(1, ON), 0.0),
        (voice_param(2, ON), 0.0),
    ];
    let mut input = vec![0.0f32; 48_000];
    input[0] = 1.0;
    let change = 9_000;
    let changing = |at: usize| {
        if at < change {
            d_minor()
        } else {
            Scale::new(0, ScaleName::Major)
        }
    };
    let steady = |_: usize| d_minor();
    let kept = Rig::new(rate, &params).render(&input, 512, &[], steady, false);
    let render = |block: usize| Rig::new(rate, &params).render(&input, block, &[], changing, false);
    let first = render(512);
    assert_eq!(
        first[0][..change],
        kept[0][..change],
        "nothing moves before the frame"
    );
    assert_ne!(
        first[0][change..change + 400],
        kept[0][change..change + 400]
    );
    // Major from C: degree 23 is key 3 octaves up + 2 degrees = 36 + 4.
    let after = &first[0][change + 2_400..];
    let found = peak(after, hz(40.0), 100.0, rate);
    assert!(
        cents(found, hz(40.0)).abs() < 5.0,
        "{} cents",
        cents(found, hz(40.0))
    );
    for block in BLOCKS {
        assert_eq!(render(block), first, "blocks of {block}");
    }
}

/// 19 equal steps to the octave, with the default reference and range.
fn nineteen() -> Tuning {
    let cents = (1..=19).map(|i| 1200.0 * f64::from(i) / 19.0).collect();
    Tuning::new("19 equal".into(), String::new(), cents).unwrap()
}

// Spec: resonators R5. Under a tuning Note counts the tuning's notes and Pitch
// its steps, each pitch the frequency the tuning gives, plus Fine.
#[test]
fn under_a_19_note_tuning_the_pitches_come_from_the_tuning() {
    let rate = SampleRate::HZ_48000;
    let tuning = nineteen();
    let params = [
        (FILTER_TYPE, 0.0),
        (FILTER_FREQ, 18_000.0),
        (DECAY, 2.0),
        (voice_param(0, PITCH), 60.0),
        (voice_param(1, PITCH), 5.0),
        (voice_param(1, FINE), 10.0),
    ];
    let mut input = vec![0.0f32; 6 * 48_000];
    input[0] = 1.0;
    for (voice, key, fine) in [(0, 60, 0.0), (1, 65, 10.0)] {
        let mut rig = Rig::new(rate, &only(voice, &params));
        rig.tune(Some(&tuning));
        let ir = rig
            .render(&input, 512, &[], |_| d_minor(), true)
            .swap_remove(0);
        let semitones = tuning.key_pitch(key).expect("the tuning has the key");
        let expected = hz(semitones + fine / 100.0);
        let found = peak(&ir, expected, 40.0, rate);
        assert!(cents(found, expected).abs() < 1.0, "resonator {voice}");
    }
    // It isn't twelve equal steps: 19 notes put key 65 elsewhere.
    assert!(cents(hz(tuning.key_pitch(65).unwrap()), hz(65.0)).abs() > 10.0);
    // Its scales are hidden: Use Current Scale on counts the tuning's notes.
    let hidden = [
        (USE_SCALE, 1.0),
        (voice_param(0, PITCH), 60.0),
        (FILTER_TYPE, 0.0),
        (FILTER_FREQ, 18_000.0),
        (DECAY, 2.0),
    ];
    let mut rig = Rig::new(rate, &only(0, &hidden));
    rig.tune(Some(&tuning));
    let again = rig
        .render(&input, 512, &[], |_| d_minor(), true)
        .swap_remove(0);
    let expected = hz(tuning.key_pitch(60).unwrap());
    assert!(cents(peak(&again, expected, 40.0, rate), expected).abs() < 1.0);
}

/// A scenario that moves every kind of parameter: a noise signal through a
/// device whose settings change along the way.
fn scenario(block: usize) -> Vec<Vec<f32>> {
    let params = [
        (voice_param(3, ON), 1.0),
        (MODE, 1.0),
        (COLOR, 40.0),
        (USE_SCALE, 1.0),
    ];
    let events: Vec<(usize, ParamId, f64)> = vec![
        (1_000, DECAY, 0.2),
        (3_000, FILTER_FREQ, 3_000.0),
        (5_000, voice_param(0, PITCH), 55.0),
        (7_001, CONST, 1.0),
        (9_000, voice_param(2, ON), 0.0),
        (11_000, voice_param(2, ON), 1.0),
        (12_000, COLOR, -60.0),
        (13_000, WIDTH, 40.0),
        (14_000, MODE, 0.0),
        (15_000, voice_param(1, FINE), 33.0),
        (16_000, FILTER_TYPE, 3.0),
    ];
    let scale = |at: usize| {
        if at < 10_000 {
            d_minor()
        } else {
            Scale::new(5, ScaleName::Major)
        }
    };
    Rig::new(SampleRate::HZ_44100, &params).render(
        &common::noise(20_000),
        block,
        &events,
        scale,
        false,
    )
}

// Spec: resonators R11, R12. A render is the same at every block size and
// from run to run.
#[test]
fn renders_are_bit_identical_at_every_block_size_and_run_after_run() {
    let golden = scenario(512);
    assert!(golden[0].iter().any(|s| s.abs() > 1e-3), "it sounds");
    assert!(golden.iter().flatten().all(|s| s.is_finite()));
    for block in BLOCKS {
        assert_eq!(scenario(block), golden, "blocks of {block}");
    }
    assert_eq!(scenario(512), golden, "run after run");
}

// Spec: resonators R7. The loop's feedback stays below one at the longest
// Decay: a long ring dies away rather than growing.
#[test]
fn the_longest_decay_never_grows() {
    let rate = SampleRate::HZ_48000;
    let params = [
        (DECAY, 30.0),
        (CONST, 1.0),
        (MODE, 1.0),
        (voice_param(3, ON), 1.0),
        (voice_param(4, ON), 1.0),
        (voice_param(0, PITCH), 84.0),
        (voice_param(4, PITCH), 24.0),
    ];
    let ir = Rig::new(rate, &params).impulse(20 * 48_000);
    let peak_of = |w: &[f32]| w.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let early = peak_of(&ir[..48_000]);
    let late = peak_of(&ir[19 * 48_000..]);
    assert!(early.is_finite() && late <= early, "{early} then {late}");
}

// Spec: resonators R1, R13, R14, R15. The description is Beatlayer's own with
// Use Current Scale in the title bar, off in a new device, and a set that
// holds the device saves and reopens it with every parameter.
#[test]
fn the_description_has_the_switch_in_the_title_bar_and_every_parameter_is_in_a_bank() {
    let description = resonators_type().description;
    description.check().unwrap();
    assert_eq!(description.title_switch, Some(resonators::USE_SCALE));
    let switch = description.param(USE_SCALE).unwrap();
    assert_eq!(
        (switch.name.as_str(), switch.default),
        ("Use Current Scale", 0.0)
    );
    // The banks hold every parameter.
    let banked: Vec<ParamId> = description
        .banks()
        .iter()
        .flat_map(|b| b.params.clone())
        .collect();
    for param in &description.params {
        assert!(banked.contains(&param.id), "{}", param.name);
    }
    // Every gain reaches minus infinity.
    for param in description.params.iter().filter(|p| p.is_gain()) {
        assert_eq!(param.min(), 0.0, "{}", param.name);
    }
    assert_eq!(
        description.param(voice_param(0, LEVEL)).unwrap().name,
        "I Gain"
    );
}

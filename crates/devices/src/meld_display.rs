//! Meld's displays (WP-8.20; spec: meld R67, device framework R35 to R38):
//! each engine's oscillator wave at its type and macros, its filter's
//! response, each envelope with its loop and each LFO's shape after its
//! effects, as data a cell draws, a screen reader says and an agent reads.
//!
//! They draw from the device's parameters alone. The envelopes and the LFOs
//! run the device's own laws ([`Envelope`], [`meld_mod`]) at a rate that
//! puts the picture's points one to a frame; the oscillator is a drawing of
//! the type, not a render of it.

use std::f32::consts::TAU;

use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DisplayReading, DisplaySeries, OrdMap, ParamId,
};

use crate::meld::{self, LOOP_MODES, param};
use crate::meld_mod::{self, FX_NAMES, Fx, LFO1_TYPES, LFO2_TYPES};
use crate::meld_osc::{MACROS, TYPES};
use crate::wavetable_mod::{DIVISIONS, EnvParams, Envelope, Lfo, Loop, segment_step, shape_power};

/// How many points an oscillator, an envelope or an LFO is drawn in.
pub const POINTS: usize = 128;
/// How many points a filter's response is drawn in.
pub const FILTER_POINTS: usize = 96;
/// A filter response's range, in decibels.
pub const FILTER_RANGE_DB: [f32; 2] = [-36.0, 24.0];

/// The series' names for engine `engine` ("A" or "B").
#[must_use]
pub fn oscillator_name(letter: &str) -> String {
    format!("{letter} Oscillator")
}

#[must_use]
pub fn filter_name(letter: &str) -> String {
    format!("{letter} Filter")
}

#[must_use]
pub fn envelope_name(letter: &str, modulation: bool) -> String {
    format!(
        "{letter} {} Envelope",
        if modulation { "Mod" } else { "Amp" }
    )
}

#[must_use]
pub fn lfo_name(letter: &str, which: &str) -> String {
    format!("{letter} {which}")
}

/// The LFOs' series' second names: LFO 1, LFO 1 FX and LFO 2.
pub const LFO_SERIES: [&str; 3] = ["LFO 1", "LFO 1 FX", "LFO 2"];

fn series(
    name: String,
    (x_unit, x_range): (&str, [f32; 2]),
    y_range: [f32; 2],
    unit: &str,
    values: Vec<f32>,
) -> DisplaySeries {
    DisplaySeries {
        name,
        unit: unit.to_owned(),
        x_unit: x_unit.to_owned(),
        x_range,
        log_x: false,
        y_range,
        detail: false,
        values,
    }
}

/// Meld's display from `values`.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
) -> DeviceDisplay {
    let mut out = DeviceDisplay {
        device,
        live: false,
        view: None,
        series: Vec::new(),
        spectrograms: Vec::new(),
        markers: Vec::new(),
        readings: Vec::new(),
    };
    let value = |id: ParamId| description.value(values, id).unwrap_or(0.0);
    for (engine, letter) in ["A", "B"].into_iter().enumerate() {
        let at = |offset: u32| value(param(engine, offset));
        let reading = |name: String, text: String| DisplayReading {
            name,
            value: 0.0,
            unit: String::new(),
            text,
        };
        // The oscillator.
        let kind = (at(meld::TYPE) as usize).min(TYPES.len() - 1);
        let (m1, m2) = (
            (at(meld::MACRO_1) / 100.0) as f32,
            (at(meld::MACRO_2) / 100.0) as f32,
        );
        let wave = (0..POINTS)
            .map(|i| oscillator(kind, m1, m2, i as f32 / POINTS as f32))
            .collect();
        out.series.push(series(
            oscillator_name(letter),
            ("cycle", [0.0, 1.0]),
            [-1.1, 1.1],
            "",
            wave,
        ));
        out.readings.push(reading(
            oscillator_name(letter),
            format!(
                "{}, {} {:.0} %, {} {:.0} %",
                TYPES[kind],
                MACROS[kind].0,
                at(meld::MACRO_1),
                MACROS[kind].1,
                at(meld::MACRO_2)
            ),
        ));
        // The filter.
        let filter_type = at(meld::FILTER_TYPE) as usize;
        let on = at(meld::FILTER_ON) >= 0.5;
        let cutoff = at(meld::FILTER_FREQ);
        let response = filter_response(
            filter_type,
            on,
            cutoff,
            (at(meld::FILTER_MACRO_1) / 100.0) as f32,
            (at(meld::FILTER_MACRO_2) / 100.0) as f32,
        );
        out.series.push(DisplaySeries {
            name: filter_name(letter),
            unit: "dB".to_owned(),
            x_unit: "Hz".to_owned(),
            x_range: [20.0, 20_000.0],
            log_x: true,
            y_range: FILTER_RANGE_DB,
            detail: false,
            values: response,
        });
        out.readings.push(reading(
            filter_name(letter),
            if on {
                format!("{} at {cutoff:.0} Hz", meld::filter_type_name(filter_type))
            } else {
                "Off".to_owned()
            },
        ));
        // The envelopes.
        let rate = 48_000.0;
        for modulation in [false, true] {
            let (params, name) = envelope_params(engine, modulation, &value, rate);
            let (points, span) = envelope_curve(&params);
            out.series.push(series(
                envelope_name(letter, modulation),
                ("ms", [0.0, span]),
                if modulation {
                    [-1.05, 1.05]
                } else {
                    [0.0, 1.05]
                },
                "",
                points,
            ));
            out.readings.push(reading(
                envelope_name(letter, modulation),
                format!(
                    "{name}, loop {}",
                    LOOP_MODES[at(if modulation {
                        meld::MOD_MODE
                    } else {
                        meld::ENV_MODE
                    }) as usize
                        % LOOP_MODES.len()]
                ),
            ));
        }
        // LFO 1, LFO 1 FX and LFO 2, two cycles each.
        let lfos = lfo_curves(engine, &value);
        for (which, points) in LFO_SERIES.into_iter().zip(lfos) {
            out.series.push(series(
                lfo_name(letter, which),
                ("cycle", [0.0, 2.0]),
                [-1.1, 1.1],
                "",
                points,
            ));
        }
        let kind1 = (at(meld::LFO1_TYPE) as usize).min(LFO1_TYPES.len() - 1);
        let kind2 = (at(meld::LFO2_TYPE) as usize).min(LFO2_TYPES.len() - 1);
        let fx = |t: u32| FX_NAMES[(at(t) as usize).min(FX_NAMES.len() - 1)];
        out.readings.push(reading(
            lfo_name(letter, "LFO 1"),
            format!(
                "{} at {}",
                LFO1_TYPES[kind1],
                lfo_rate_text(&value, engine, false)
            ),
        ));
        out.readings.push(reading(
            lfo_name(letter, "LFO 1 FX"),
            format!("{}, then {}", fx(meld::FX1_TYPE), fx(meld::FX2_TYPE)),
        ));
        out.readings.push(reading(
            lfo_name(letter, "LFO 2"),
            format!(
                "{} at {}",
                LFO2_TYPES[kind2],
                lfo_rate_text(&value, engine, true)
            ),
        ));
    }
    out
}

/// An LFO's rate as it reads: hertz, or a division with sync on.
fn lfo_rate_text(value: &dyn Fn(ParamId) -> f64, engine: usize, second: bool) -> String {
    let at = |offset: u32| value(param(engine, offset));
    let (sync, rate, division) = if second {
        (meld::LFO2_SYNC, meld::LFO2_RATE, meld::LFO2_DIVISION)
    } else {
        (meld::LFO1_SYNC, meld::LFO1_RATE, meld::LFO1_DIVISION)
    };
    if at(sync) >= 0.5 {
        DIVISIONS[(at(division) as usize).min(DIVISIONS.len() - 1)]
            .0
            .to_owned()
    } else {
        format!("{:.2} Hz", at(rate))
    }
}

/// One cycle of oscillator type `kind` at `phase` (0 to 1) for macros `m1`
/// and `m2`: a drawing of what the type sounds like (R67), from −1 to 1.
#[must_use]
pub fn oscillator(kind: usize, m1: f32, m2: f32, phase: f32) -> f32 {
    let sine = |p: f32| (TAU * p).sin();
    let triangle = |p: f32| 1.0 - 4.0 * ((p + 0.25).fract() - 0.5).abs();
    let saw = |p: f32| 2.0 * (p + 0.5).fract() - 1.0;
    let square = |p: f32, width: f32| if p.fract() < width { 1.0 } else { -1.0 };
    // Shape's morph through the four waves.
    let morph = |shape: f32, p: f32, tone: f32| {
        let at = shape.clamp(0.0, 1.0) * 3.0;
        let low = (at as usize).min(2);
        let t = at - low as f32;
        let wave = |i: usize| match i {
            0 => sine(p),
            1 => triangle(p),
            2 => saw(p * (1.0 + tone)),
            _ => square(p, 0.5 - 0.45 * tone),
        };
        wave(low) * (1.0 - t) + wave(low + 1) * t
    };
    let noise = |seed: f32| {
        // A repeatable scatter: the draw of the point's place.
        let n = (phase * POINTS as f32).floor() + seed;
        ((n * 12.9898).sin() * 43_758.545).fract() * 2.0 - 1.0
    };
    let x = match kind {
        0 | 2 => {
            let tone = if kind == 0 { m2 } else { 0.0 };
            let rough = if kind == 2 {
                0.1 * m2 * noise(3.0)
            } else {
                0.0
            };
            morph(m1, phase + rough, tone)
        }
        1 => {
            let up = (m2 * 0.5).min(0.5) / 12.0;
            0.5 * (morph(m1, phase, 0.0) + morph(m1, phase * (1.0 + up), 0.0))
        }
        3 => 0.5 * (square(phase * (1.0 + 7.0 * m1), 0.5) + square(phase * (1.0 + 7.0 * m2), 0.5)),
        4 => {
            (square(phase, 0.1 + 0.8 * m2) + m1 * square(phase * 1.5, 0.1 + 0.8 * m2)) / (1.0 + m1)
        }
        5 => {
            let level = (1.0 + m2 * m2).sqrt().recip();
            level * (sine(phase) * (1.0 - m1) + square(phase, 0.5) * m1 + m2 * sine(phase * 0.5))
        }
        6..=9 => {
            let wave = |p: f32| match kind {
                6 => sine(p),
                7 => triangle(p),
                8 => saw(p),
                _ => square(p, 0.5),
            };
            let spread = 0.02 + 0.2 * m2;
            (wave(phase) + wave(phase * (1.0 - spread)) + wave(phase * (1.0 + spread))) / 3.0
        }
        10..=13 => {
            let ratio = match kind {
                10 => 1.0 + (m2 * 16.0).floor().min(15.0),
                11 | 12 => 2.0,
                _ => 0.5 * 32f32.powf(m2),
            };
            let beta = 8.0 * m1;
            if kind == 11 {
                // Fold FM: the sine folded back on itself by Amount.
                ((beta + 0.3) * sine(phase)).sin()
            } else {
                sine(phase + beta * 0.2 * sine(phase * ratio))
            }
        }
        14 => square(
            phase * (1.0 + (m1 * 3.0).floor().min(2.0)),
            0.5 - 0.375 * m1,
        ),
        15 => {
            (0..5)
                .map(|i| sine(phase * 2f32.powi(i)) * (1.0 - i as f32 * 0.18))
                .sum::<f32>()
                / 3.0
        }
        16 => (1.0 - phase).powf(1.0 + 4.0 * (1.0 - m1)) * noise(1.0),
        17 => sine(phase * (1.0 + 6.0 * (1.0 - phase) * m2 + m1)),
        18 => noise(5.0 + (m1 * 8.0).floor()),
        19 => 0.7 * noise(7.0) * sine(phase * (2.0 + 6.0 * m1)),
        20 => square(phase * (1.0 + 3.0 * m1), 0.5) * square(phase * 0.5, 0.5) * (1.0 - 0.5 * m2),
        21 => {
            let click = if noise(11.0) > 1.0 - 1.4 * m1 {
                1.0
            } else {
                0.0
            };
            click * noise(13.0) * (0.3 + 0.7 * m2)
        }
        22 => 0.6 * noise(17.0) * (0.3 + 0.7 * m1),
        23 => sine(phase * (6.0 + 20.0 * phase * (0.5 + m1))) * (1.0 - phase),
        _ => {
            let shape = m1.clamp(0.0, 1.0);
            let notes = [0.0, 4.0 + 3.0 * shape, 7.0, 12.0 - 4.0 * shape];
            notes
                .iter()
                .map(|n| saw(phase * 2f32.powf(n / 12.0)))
                .sum::<f32>()
                / 4.0
        }
    };
    x.clamp(-1.0, 1.0)
}

/// The envelope parameters of engine `engine`'s amplitude or modulation
/// envelope, as `value` has them, at the picture's rate; and a sentence for
/// a screen reader.
fn envelope_params(
    engine: usize,
    modulation: bool,
    value: &dyn Fn(ParamId) -> f64,
    rate: f64,
) -> (EnvParams, String) {
    let at = |offset: u32| value(param(engine, offset));
    let (a, d, s, r, sa, sd, sr, mode) = if modulation {
        (
            meld::MOD_ATTACK,
            meld::MOD_DECAY,
            meld::MOD_SUSTAIN,
            meld::MOD_RELEASE,
            meld::MOD_ATTACK_SLOPE,
            meld::MOD_DECAY_SLOPE,
            meld::MOD_RELEASE_SLOPE,
            meld::MOD_MODE,
        )
    } else {
        (
            meld::ATTACK,
            meld::DECAY,
            meld::SUSTAIN,
            meld::RELEASE,
            meld::ATTACK_SLOPE,
            meld::DECAY_SLOPE,
            meld::RELEASE_SLOPE,
            meld::ENV_MODE,
        )
    };
    let own = !modulation || at(meld::LINK) < 0.5;
    let (ta, td, tr, ka, kd, kr) = if own {
        (at(a), at(d), at(r), at(sa), at(sd), at(sr))
    } else {
        // Linked, the modulation envelope takes the amplitude envelope's.
        (
            at(meld::ATTACK),
            at(meld::DECAY),
            at(meld::RELEASE),
            at(meld::ATTACK_SLOPE),
            at(meld::DECAY_SLOPE),
            at(meld::RELEASE_SLOPE),
        )
    };
    let params = EnvParams {
        step: [ta, td, tr].map(|ms| segment_step(ms, rate, 1.0)),
        power: [ka, kd, kr].map(|k| shape_power((k / 100.0) as f32)),
        sustain: (at(s) / 100.0) as f32,
        initial: if modulation {
            (at(meld::MOD_INITIAL) / 100.0) as f32
        } else {
            0.0
        },
        peak: if modulation {
            (at(meld::MOD_PEAK) / 100.0) as f32
        } else {
            1.0
        },
        fin: if modulation {
            (at(meld::MOD_FINAL) / 100.0) as f32
        } else {
            0.0
        },
        mode: Loop::of(at(mode) as usize),
    };
    let text = format!(
        "Attack {ta:.0} ms, Decay {td:.0} ms, Sustain {:.0} %, Release {tr:.0} ms",
        at(s)
    );
    (params, text)
}

/// An envelope's points, and the length they cover in milliseconds: the
/// note held for as long as its Attack and Decay together or 100 ms, then
/// released; twice over for the looping modes.
fn envelope_curve(params: &EnvParams) -> (Vec<f32>, f32) {
    let rate = 48_000.0;
    let ms = |step: f32| 1_000.0 / (f64::from(step.max(1e-9)) * rate);
    let [a, d, r] = [params.step[0], params.step[1], params.step[2]].map(|s| ms(s) as f32);
    let hold = (a + d).max(100.0);
    let cycle = match params.mode {
        Loop::None => a + d + hold + r,
        Loop::Trigger => a + d + r + hold * 0.25,
        Loop::Loop => 2.0 * (a + d + r),
        Loop::AdLoop => 2.0 * (a + d) + hold * 0.5 + r,
    };
    let span = (cycle * 1.05).max(1.0);
    let held_until = match params.mode {
        Loop::None => a + d + hold,
        Loop::Trigger => f32::MAX,
        Loop::Loop => 2.0 * (a + d + r) * 0.999,
        Loop::AdLoop => 2.0 * (a + d) + hold * 0.5,
    };
    // Run the device's envelope with one frame to a point.
    let per_point = span / POINTS as f32;
    let scale = |step: f32| (f64::from(step) * f64::from(per_point) * rate / 1_000.0) as f32;
    let drawn = EnvParams {
        step: params.step.map(scale),
        ..*params
    };
    let mut envelope = Envelope::IDLE;
    envelope.start(drawn.initial);
    let mut released = false;
    let points = (0..POINTS)
        .map(|i| {
            let t = i as f32 * per_point;
            if !released && t >= held_until {
                envelope.release(drawn.mode);
                released = true;
            }
            envelope.next(&drawn)
        })
        .collect();
    (points, span)
}

/// LFO 1, LFO 1 FX and LFO 2 over two cycles, in [`POINTS`] points, with
/// the device's own laws.
fn lfo_curves(engine: usize, value: &dyn Fn(ParamId) -> f64) -> [Vec<f32>; 3] {
    let at = |offset: u32| value(param(engine, offset));
    let kind1 = (at(meld::LFO1_TYPE) as usize).min(LFO1_TYPES.len() - 1);
    let kind2 = (at(meld::LFO2_TYPE) as usize).min(LFO2_TYPES.len() - 1);
    let macros = [
        (at(meld::LFO1_MACRO_1) / 100.0) as f32,
        (at(meld::LFO1_MACRO_2) / 100.0) as f32,
    ];
    let offset = |o: u32| (at(o) / 360.0) as f32;
    let fx = [
        (
            (at(meld::FX1_TYPE) as usize).min(FX_NAMES.len() - 1),
            (at(meld::FX1_AMOUNT) / 100.0) as f32,
        ),
        (
            (at(meld::FX2_TYPE) as usize).min(FX_NAMES.len() - 1),
            (at(meld::FX2_AMOUNT) / 100.0) as f32,
        ),
    ];
    let mut effects = [Fx::IDLE; 2];
    for (n, effect) in effects.iter_mut().enumerate() {
        effect.start(0x000D_15C0 + n as u64);
    }
    let salt = 0x4D45_4C44;
    let mut lfo = Lfo::IDLE;
    lfo.start(0.0, salt);
    let (mut one, mut after, mut two) = (
        Vec::with_capacity(POINTS),
        Vec::with_capacity(POINTS),
        Vec::with_capacity(POINTS),
    );
    let step = 2.0 / POINTS as f64;
    let mut second = Lfo::IDLE;
    second.start(0.0, salt ^ 1);
    for _ in 0..POINTS {
        let x = meld_mod::lfo1(
            kind1,
            ((f64::from(lfo.phase as f32) + f64::from(offset(meld::LFO1_OFFSET))).fract()) as f32,
            lfo.cycles,
            salt,
            macros,
        );
        one.push(x);
        let mut y = x;
        for (effect, (kind, amount)) in effects.iter_mut().zip(fx) {
            if kind != 0 {
                y = effect.step(kind, amount, y);
            }
        }
        after.push(y);
        two.push(meld_mod::lfo2(
            kind2,
            ((f64::from(second.phase as f32) + f64::from(offset(meld::LFO2_OFFSET))).fract())
                as f32,
            second.cycles,
            salt ^ 1,
        ));
        lfo.run(step);
        second.run(step);
    }
    [one, after, two]
}

/// A filter's response over 20 Hz to 20 kHz, in decibels: SVF 12dB and
/// SVF 24dB's L-B-H-N morph, flat for the rest and for a filter that's off.
#[must_use]
pub fn filter_response(kind: usize, on: bool, cutoff: f64, q: f32, morph: f32) -> Vec<f32> {
    (0..FILTER_POINTS)
        .map(|i| {
            if !on || kind > 1 {
                return 0.0;
            }
            let t = i as f32 / (FILTER_POINTS - 1) as f32;
            let hz = 20.0 * 1_000f32.powf(t);
            let w = hz / cutoff as f32;
            // A second-order state variable: Q from 0.7 to 10.
            let quality = 0.7 + 9.3 * q * q;
            let (re, im) = (1.0 - w * w, w / quality);
            let denominator = (re * re + im * im).sqrt();
            let low = 1.0 / denominator;
            let band = (w / quality) / denominator;
            let high = w * w / denominator;
            let notch = (1.0 - w * w).abs() / denominator;
            let at = morph.clamp(0.0, 1.0) * 3.0;
            let (a, b, t) = match at as usize {
                0 => (low, band, at),
                1 => (band, high, at - 1.0),
                _ => (high, notch, at - 2.0),
            };
            let mut magnitude = a * (1.0 - t) + b * t;
            if kind == 1 {
                magnitude *= magnitude;
            }
            (20.0 * magnitude.max(1e-6).log10()).clamp(FILTER_RANGE_DB[0], FILTER_RANGE_DB[1])
        })
        .collect()
}

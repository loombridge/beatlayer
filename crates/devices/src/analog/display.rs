//! Analog's displays (WP-8.15; spec: Analog R26): each envelope's shape,
//! each filter's response with its cutoff and resonance, and each LFO's wave,
//! worked out from the parameters alone and reported by `device.display`.
//! A cell of the panel is named like the series it draws, so a section's page
//! shows its own curve.

use core::f64::consts::PI;

use bl_dsp::random::Random;
use bl_model::{DeviceDescription, DeviceDisplay, DeviceId, DisplaySeries, OrdMap, ParamId};

use super::waves;
use super::{
    EnvSettings, Envelope, FIRST_FORMANT, FORMANT_RESONANCE, SEED, env, filter, formants, kit_type,
    lfo,
};

/// The series and cells of each envelope: Filter 1, Filter 2, Amplifier 1
/// and Amplifier 2.
pub const ENVELOPES: [&str; 4] = [
    "Filter 1 Envelope",
    "Filter 2 Envelope",
    "Amp 1 Envelope",
    "Amp 2 Envelope",
];

/// The series and cells of each filter's response.
pub const RESPONSES: [&str; 2] = ["Filter 1 Response", "Filter 2 Response"];

/// The series and cells of each LFO's wave.
pub const WAVES: [&str; 2] = ["LFO 1 Wave", "LFO 2 Wave"];

/// How many points a curve has.
const POINTS: usize = 96;

/// The rate the response is worked out at, for the prototype's warped axis.
const RATE: f64 = 48_000.0;

/// What the response's scale shows, in decibels.
const RESPONSE_DB: [f32; 2] = [-48.0, 24.0];

/// How many cycles an LFO's wave shows.
const CYCLES: f32 = 2.0;

/// Spec: analog R26. Analog's display for a device whose parameters hold
/// `values`: the envelopes, the filters' responses and the LFOs' waves.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
) -> DeviceDisplay {
    let value = |id: ParamId| description.value(values, id).unwrap_or(0.0);
    let mut series = Vec::new();
    for (n, name) in ENVELOPES.iter().enumerate() {
        series.push(envelope(name, n, &value));
    }
    for (k, name) in RESPONSES.iter().enumerate() {
        series.push(response(name, k, &value));
    }
    for (k, name) in WAVES.iter().enumerate() {
        series.push(wave(name, k, &value));
    }
    DeviceDisplay {
        device,
        live: false,
        view: None,
        series,
        spectrograms: Vec::new(),
        markers: Vec::new(),
        readings: Vec::new(),
    }
}

/// Envelope `n`'s shape: its stages for a note held long enough to reach
/// its sustain (or to loop twice) and then let go, on the device's own
/// envelope worked out at a rate that gives the curve its points.
fn envelope(name: &str, n: usize, value: &dyn Fn(ParamId) -> f64) -> DisplaySeries {
    let get = |offset| value(env::id(n, offset));
    let (attack, decay, release) = (get(env::ATTACK), get(env::DECAY), get(env::RELEASE));
    let looping = get(env::LOOP) as usize;
    let free = get(env::FREE) >= 0.5;
    let stages = attack + decay + release;
    let held = if free {
        0.0
    } else if looping == 0 {
        (0.2 * stages).max(50.0)
    } else {
        2.0 * (stages + 20.0)
    };
    let total = (stages + held).max(1.0);
    // Frames of 1 ms scaled so that the curve has `POINTS` frames.
    let per_ms = (POINTS - 1) as f32 / total as f32;
    let frames = |ms: f64| ms as f32 * per_ms;
    let fall = frames(get(env::S_TIME));
    let sustain = get(env::SUSTAIN) as f32 / 100.0;
    let set = EnvSettings {
        attack: frames(attack),
        decay: frames(decay),
        sustain,
        release: frames(release),
        fall: if fall > 1.0 { sustain / fall } else { 0.0 },
        exponential: get(env::SLOPE) >= 0.5,
        free,
        legato: false,
        looping,
        attack_velocity: 0.0,
        level_velocity: 0.0,
    };
    let mut envelope = Envelope::new();
    envelope.trigger(0, &set, false, 0.0);
    let release_at = frames(stages - release + held) as usize;
    let values = (0..POINTS)
        .map(|frame| {
            if frame == release_at {
                envelope.release(0, &set);
            }
            envelope.step(0, &set)
        })
        .collect();
    DisplaySeries {
        name: name.to_owned(),
        unit: String::new(),
        x_unit: "ms".to_owned(),
        x_range: [0.0, total as f32],
        log_x: false,
        y_range: [0.0, 1.0],
        detail: false,
        values,
    }
}

/// A complex number, for the formant's sum of two resonances.
#[derive(Clone, Copy)]
struct Complex(f64, f64);

impl Complex {
    fn mul(self, other: Self) -> Self {
        Self(
            self.0 * other.0 - self.1 * other.1,
            self.0 * other.1 + self.1 * other.0,
        )
    }

    fn recip(self) -> Self {
        let power = self.0 * self.0 + self.1 * self.1;
        Self(self.0 / power, -self.1 / power)
    }

    fn norm(self) -> f64 {
        self.0.hypot(self.1)
    }
}

/// Spec: analog R7, R8. The gain of filter mode `mode` at `hz` for a
/// cutoff of `cutoff` and resonance `reso`: the analogue prototype on the
/// bilinear-warped axis the kit's filters follow, with the resonant stage's
/// damping from the kit's own law. A formant mode is two band-pass
/// resonators at the first two formants of the vowel `reso` and `cutoff`
/// give, summed.
fn gain(mode: usize, cutoff: f64, reso: f64, hz: f64) -> f64 {
    let warp = |hz: f64, centre: f64| (PI * hz / RATE).tan() / (PI * centre / RATE).tan();
    let kind = kit_type(mode);
    let second = |s: f64, k: f64| ((1.0 - s * s).powi(2) + (k * s).powi(2)).sqrt();
    let k = f64::from(kind.feedback(reso as f32));
    let s = warp(hz, cutoff);
    match mode {
        0 => 1.0 / second(s, k),
        1 => 1.0 / (second(s, 1.847_759) * second(s, k)),
        2 => s / second(s, k),
        3 => s / second(s, 1.0) * (s / second(s, k)),
        4 => (1.0 - s * s).abs() / second(s, k),
        5 => (1.0 - s * s).abs() / second(s, 2.0) * ((1.0 - s * s).abs() / second(s, k)),
        6 => s * s / second(s, k),
        7 => s.powi(4) / (second(s, 1.847_759) * second(s, k)),
        _ => {
            let k = f64::from(kit_type(mode).feedback(FORMANT_RESONANCE));
            let (f1, f2) = formants(reso as f32, cutoff as f32);
            let resonator = |centre: f64| {
                let x = warp(hz, centre);
                let one = |k: f64| Complex(0.0, x).mul(Complex(1.0 - x * x, k * x).recip());
                if mode == FIRST_FORMANT {
                    one(k)
                } else {
                    one(1.0).mul(one(k))
                }
            };
            let (a, b) = (resonator(f64::from(f1)), resonator(f64::from(f2)));
            Complex(a.0 + b.0, a.1 + b.1).norm()
        }
    }
}

/// Filter `k`'s response over 20 Hz to 20 kHz, in decibels.
fn response(name: &str, k: usize, value: &dyn Fn(ParamId) -> f64) -> DisplaySeries {
    let get = |offset| value(filter::id(k, offset));
    let (mode, cutoff, reso) = (
        (get(filter::MODE) as usize).min(super::MODES.len() - 1),
        get(filter::FREQ),
        get(filter::RESO) / 100.0,
    );
    let (low, high) = (20f64, 20_000f64);
    let values = (0..POINTS)
        .map(|n| {
            let hz = low * (high / low).powf(n as f64 / (POINTS - 1) as f64);
            let db = 20.0 * gain(mode, cutoff, reso, hz).max(1e-6).log10();
            (db as f32).clamp(RESPONSE_DB[0], RESPONSE_DB[1])
        })
        .collect();
    DisplaySeries {
        name: name.to_owned(),
        unit: "dB".to_owned(),
        x_unit: "Hz".to_owned(),
        x_range: [low as f32, high as f32],
        log_x: true,
        y_range: RESPONSE_DB,
        detail: false,
        values,
    }
}

/// LFO `k`'s wave over two cycles. The random waves show values from the
/// device's own seed, as every note's LFO draws.
fn wave(name: &str, k: usize, value: &dyn Fn(ParamId) -> f64) -> DisplaySeries {
    let get = |offset| value(lfo::id(k, offset));
    let kind = get(lfo::WAVE) as usize;
    let width = (get(lfo::WIDTH) / 100.0) as f32;
    let mut random = Random::seeded(SEED ^ (k as u64 + 1));
    let draws: [f32; 3] = core::array::from_fn(|_| random.next_bipolar() as f32);
    let values = (0..POINTS)
        .map(|n| {
            let cycles = CYCLES * n as f32 / (POINTS - 1) as f32;
            let (cycle, phase) = (cycles.floor().min(CYCLES - 1.0), cycles.fract());
            let phase = if cycles >= CYCLES { 1.0 } else { phase };
            let at = cycle as usize;
            waves::wave(kind, phase, width, draws[at], draws[at + 1])
        })
        .collect();
    DisplaySeries {
        name: name.to_owned(),
        unit: String::new(),
        x_unit: "cycles".to_owned(),
        x_range: [0.0, CYCLES],
        log_x: false,
        y_range: [-1.0, 1.0],
        detail: false,
        values,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analog::{analog_type, filter as filter_ids};

    fn values(changes: &[(ParamId, f64)]) -> (DeviceDescription, OrdMap<ParamId, f64>) {
        let description = analog_type().description;
        let mut values = OrdMap::new();
        for param in &description.params {
            values.insert(param.id, param.default);
        }
        for &(id, value) in changes {
            values.insert(id, value);
        }
        (description, values)
    }

    fn shown(changes: &[(ParamId, f64)]) -> DeviceDisplay {
        let (description, values) = values(changes);
        display(
            DeviceId(bl_core::id::Id::new(1).unwrap()),
            &description,
            &values,
        )
    }

    // Spec: analog R26
    #[test]
    fn the_display_draws_every_envelope_filter_and_lfo() {
        let shown = shown(&[]);
        for name in ENVELOPES.iter().chain(&RESPONSES).chain(&WAVES) {
            let series = shown.series(name).expect("a series");
            assert_eq!(series.values.len(), POINTS, "{name}");
        }
    }

    // Spec: analog R26
    #[test]
    fn an_envelope_rises_to_full_falls_to_its_sustain_and_to_silence() {
        let shown = shown(&[
            (env::id(2, env::ATTACK), 200.0),
            (env::id(2, env::DECAY), 300.0),
            (env::id(2, env::SUSTAIN), 40.0),
            (env::id(2, env::RELEASE), 400.0),
        ]);
        let series = shown.series(ENVELOPES[2]).unwrap();
        let peak = series.values.iter().copied().fold(0.0, f32::max);
        assert!(
            (peak - 1.0).abs() < 0.01,
            "full at the end of the attack: {peak}"
        );
        let held = series
            .values
            .iter()
            .copied()
            .find(|&x| (x - 0.4).abs() < 1e-4);
        assert!(held.is_some(), "it holds its sustain");
        assert!(
            series.values.last().unwrap().abs() < 0.05,
            "and ends at silence"
        );
        assert!(
            series.x_range[1] > 900.0,
            "it spans the stages and a held note"
        );
    }

    // Spec: analog R26
    #[test]
    fn a_filters_response_follows_its_mode_cutoff_and_resonance() {
        let at = |series: &DisplaySeries, hz: f32| {
            let n = (POINTS - 1) as f32 * (hz / 20.0).ln() / 1_000f32.ln();
            series.values[n.round() as usize]
        };
        let plain = shown(&[
            (filter_ids::id(0, filter::FREQ), 1_000.0),
            (filter_ids::id(0, filter::RESO), 0.0),
        ]);
        let plain = plain.series(RESPONSES[0]).unwrap();
        assert!(
            at(plain, 100.0).abs() < 0.3,
            "a low-pass passes below its cutoff"
        );
        assert!((at(plain, 1_000.0) + 3.0).abs() < 1.5, "3 dB down at it");
        assert!(at(plain, 8_000.0) < -30.0, "and falls above it");
        let resonant = shown(&[
            (filter_ids::id(0, filter::FREQ), 1_000.0),
            (filter_ids::id(0, filter::RESO), 90.0),
        ]);
        let resonant = resonant.series(RESPONSES[0]).unwrap();
        assert!(
            at(resonant, 1_000.0) > at(plain, 1_000.0) + 6.0,
            "resonance peaks it"
        );
        let high = shown(&[
            (filter_ids::id(0, filter::MODE), 6.0),
            (filter_ids::id(0, filter::FREQ), 1_000.0),
        ]);
        let high = high.series(RESPONSES[0]).unwrap();
        assert!(
            at(high, 100.0) < -30.0 && at(high, 8_000.0).abs() < 0.5,
            "a high-pass"
        );
    }

    // Spec: analog R26
    #[test]
    fn a_formant_responds_at_the_vowels_first_two_formants() {
        let shown = shown(&[
            (filter_ids::id(0, filter::MODE), 8.0),
            (filter_ids::id(0, filter::FREQ), 1_000.0),
            (filter_ids::id(0, filter::RESO), 0.0),
        ]);
        let series = shown.series(RESPONSES[0]).unwrap();
        let (x, _) = series.peak().unwrap();
        assert!(
            (500.0..1_300.0).contains(&x),
            "the strongest part sits at the first formants of A: {x}"
        );
    }

    // Spec: analog R18, R26
    #[test]
    fn an_lfos_wave_draws_two_cycles_of_its_shape() {
        let shown = shown(&[(lfo::id(0, lfo::WAVE), 2.0), (lfo::id(0, lfo::WIDTH), 50.0)]);
        let series = shown.series(WAVES[0]).unwrap();
        assert_eq!(series.x_range, [0.0, 2.0]);
        assert_eq!(series.values[0], 1.0, "a rectangle starts high");
        assert!(series.values.iter().any(|&x| x == -1.0), "and falls low");
    }
}

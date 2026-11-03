//! Operator's displays (WP-8.13; spec: operator R29): an oscillator's
//! envelope or its partials, the filter's envelope or response, the LFO's
//! envelope, the pitch envelope and the algorithm graph, each worked out
//! from the device's parameters and drawn partials, so `device.display`
//! reports them with no engine running.

use bl_model::{
    DeviceDisplay, DeviceId, DisplayReading, DisplaySeries, OrdMap, ParamId, Partials,
};

use crate::fm::{self, OPERATORS};
use crate::operator::{
    ALGORITHM, FILTER_CIRCUIT, FILTER_ENV, FILTER_FREQ, FILTER_RES, FILTER_TYPE, LFO_ENV,
    OSC_REPEAT, OSC_REPEAT_ON, OSC_WAVE, PITCH_ENV, osc, osc_shape, shaped_shape,
};
use crate::operator_envelope::{Shape, State};
use crate::sampler::FILTER_TYPE_CHOICES;

/// The views, in the order the display offers them (R29): the global
/// display's algorithm graph, then each oscillator's envelope and partials,
/// the LFO's, the filter's and the pitch envelope.
pub const VIEWS: [&str; 13] = [
    "Algorithm",
    "Osc A Envelope",
    "Osc A Oscillator",
    "Osc B Envelope",
    "Osc B Oscillator",
    "Osc C Envelope",
    "Osc C Oscillator",
    "Osc D Envelope",
    "Osc D Oscillator",
    "LFO Envelope",
    "Filter Envelope",
    "Filter Response",
    "Pitch Envelope",
];

/// The view a section's page starts on (R29): Global's algorithm graph, an
/// oscillator's envelope, and the LFO's, filter's and pitch's envelopes.
#[must_use]
pub fn view_of_page(page: &str) -> Option<&'static str> {
    Some(match page {
        "Global" => VIEWS[0],
        "Osc A" => VIEWS[1],
        "Osc B" => VIEWS[3],
        "Osc C" => VIEWS[5],
        "Osc D" => VIEWS[7],
        "LFO" => VIEWS[9],
        "Filter" => VIEWS[10],
        "Pitch" => VIEWS[12],
        _ => return None,
    })
}

/// Points in an envelope's curve and the filter's response.
const POINTS: usize = 96;

/// The seconds an envelope's curve holds its note after the attack and the
/// decay, before it lets go (Beatlayer's own).
const HOLD: f32 = 0.25;

/// The longest an envelope's curve runs, in seconds.
const LONGEST: f32 = 60.0;

/// An envelope's curve over time, from the note's start through its
/// release (R12 to R14): the note is held for [`HOLD`] seconds past the
/// decay, and the song counts at 120 beats a minute for Beat and Sync.
fn envelope_series(name: &str, shape: &Shape, range: [f32; 2]) -> DisplaySeries {
    let total = (shape.attack + shape.decay + HOLD + shape.release).clamp(0.1, LONGEST);
    let release_at = (total - shape.release).max(0.0);
    let tick = total / POINTS as f32;
    let mut state = State::DONE;
    state.start(shape, 0.0, None);
    let mut values = Vec::with_capacity(POINTS);
    let mut released = false;
    for i in 0..POINTS {
        let at = i as f32 * tick;
        if !released && at >= release_at {
            state.release(shape);
            released = true;
        }
        values.push(state.step(shape, tick, f64::from(at) * 2.0));
    }
    DisplaySeries {
        name: name.to_owned(),
        unit: String::new(),
        x_unit: "s".to_owned(),
        x_range: [0.0, total],
        log_x: false,
        y_range: range,
        detail: false,
        values,
    }
}

/// The filter's magnitude response in dB from 20 Hz to 20 kHz: a second-order
/// section of the chosen type at the cutoff, with Res raising its peak, and
/// twice as steep for the 24 dB slopes. Morph's is its band-pass.
fn response_series(entry: usize, freq: f64, res: f64) -> DisplaySeries {
    let kind = entry / 2;
    let slope = if entry % 2 == 1 { 2.0 } else { 1.0 };
    let q = 0.5 + res * 9.5;
    let values = (0..POINTS)
        .map(|i| {
            let hz = 20.0 * (1_000.0f64).powf(i as f64 / (POINTS - 1) as f64);
            let x = hz / freq;
            let (re, im) = (1.0 - x * x, x / q);
            let denominator = re.hypot(im);
            let magnitude = match kind {
                0 => 1.0 / denominator,
                1 => x * x / denominator,
                2 | 4 => (x / q) / denominator,
                _ => re.abs() / denominator,
            };
            (20.0 * (magnitude.max(1e-6)).log10() * slope) as f32
        })
        .collect();
    DisplaySeries {
        name: "Filter Response".to_owned(),
        unit: "dB".to_owned(),
        x_unit: "Hz".to_owned(),
        x_range: [20.0, 20_000.0],
        log_x: true,
        y_range: [-48.0, 24.0],
        detail: false,
        values,
    }
}

/// The algorithm graph as readings: the algorithm's number, and for each
/// operator what modulates it or that it is a carrier (R2).
fn algorithm_readings(algorithm: usize) -> Vec<DisplayReading> {
    let letter = |op: usize| ["A", "B", "C", "D"][op];
    let modulators = fm::modulators(algorithm);
    let carriers = fm::carriers(algorithm);
    let mut readings = vec![DisplayReading {
        name: "Algorithm".to_owned(),
        value: (algorithm + 1) as f32,
        unit: String::new(),
        text: format!("{}", algorithm + 1),
    }];
    for op in 0..OPERATORS {
        let by: Vec<&str> = (0..OPERATORS)
            .filter(|m| modulators[op] >> m & 1 == 1)
            .map(letter)
            .collect();
        let role = if carriers >> op & 1 == 1 {
            "carrier"
        } else {
            "modulator"
        };
        readings.push(DisplayReading {
            name: format!("Oscillator {}", letter(op)),
            value: by.len() as f32,
            unit: String::new(),
            text: if by.is_empty() {
                format!("{role}")
            } else {
                format!("{role}, modulated by {}", by.join(" and "))
            },
        });
    }
    readings
}

/// Operator's display for `view` (R29), from `values` and the drawn
/// `partials`; the first view without one.
#[must_use]
pub fn display(
    device: DeviceId,
    values: &OrdMap<ParamId, f64>,
    partials: &[Partials],
    view: Option<&str>,
) -> DeviceDisplay {
    let view = view
        .and_then(|view| VIEWS.iter().find(|v| **v == view))
        .copied()
        .unwrap_or(VIEWS[0]);
    let value = |id: ParamId| values.get(&id).copied().unwrap_or(0.0);
    let mut out = DeviceDisplay {
        device,
        live: false,
        view: Some(view.to_owned()),
        series: Vec::new(),
        spectrograms: Vec::new(),
        markers: Vec::new(),
        readings: Vec::new(),
    };
    match view {
        "Algorithm" => {
            out.readings = algorithm_readings((value(ALGORITHM) as usize).clamp(1, 11) - 1);
        }
        "LFO Envelope" => out.series.push(envelope_series(
            "LFO Envelope",
            &shaped_shape(&value, LFO_ENV),
            [-1.0, 1.0],
        )),
        "Filter Envelope" => out.series.push(envelope_series(
            "Filter Envelope",
            &shaped_shape(&value, FILTER_ENV),
            [-1.0, 1.0],
        )),
        "Pitch Envelope" => out.series.push(envelope_series(
            "Pitch Envelope",
            &shaped_shape(&value, PITCH_ENV),
            [-1.0, 1.0],
        )),
        "Filter Response" => {
            let entry = (value(FILTER_TYPE) as usize).min(FILTER_TYPE_CHOICES.len() - 1);
            out.series.push(response_series(
                entry,
                value(FILTER_FREQ),
                value(FILTER_RES) / 100.0,
            ));
            out.readings.push(DisplayReading {
                name: "Filter".to_owned(),
                value: value(FILTER_FREQ) as f32,
                unit: "Hz".to_owned(),
                text: format!(
                    "{} {:.0} Hz, {}",
                    FILTER_TYPE_CHOICES[entry],
                    value(FILTER_FREQ),
                    crate::instrument_filter::CIRCUITS
                        [(value(FILTER_CIRCUIT) as usize).min(4)]
                ),
            });
        }
        _ => {
            // "Osc A Envelope" and "Osc A Oscillator", and so on.
            let op = usize::from(view.as_bytes()[4] - b'A');
            if view.ends_with("Envelope") {
                let mut shape = osc_shape(&value, op);
                // An oscillator's levels are amplitudes, 0 to 1.
                shape.end = 0.0;
                out.series
                    .push(envelope_series(view, &shape, [0.0, 1.0]));
            } else {
                let wave = value(osc(op, OSC_WAVE)) as usize;
                out.readings.push(DisplayReading {
                    name: "Wave".to_owned(),
                    value: wave as f32,
                    unit: String::new(),
                    text: fm::WAVES[wave.min(fm::WAVES.len() - 1)].to_owned(),
                });
                let drawn = partials.iter().find(|p| usize::from(p.osc) == op);
                if wave == fm::USER {
                    let levels = drawn.map_or_else(
                        || {
                            let mut first = vec![0.0; bl_model::partials::COUNTS[0]];
                            first[0] = 1.0;
                            first
                        },
                        |p| p.levels.clone(),
                    );
                    out.readings.push(DisplayReading {
                        name: "Partials".to_owned(),
                        value: levels.len() as f32,
                        unit: String::new(),
                        text: format!("{} partials", levels.len()),
                    });
                    let repeat = if value(osc(op, OSC_REPEAT_ON)) >= 0.5 {
                        format!("{:.0} %", value(osc(op, OSC_REPEAT)))
                    } else {
                        "Off".to_owned()
                    };
                    out.readings.push(DisplayReading {
                        name: "Repeat".to_owned(),
                        value: value(osc(op, OSC_REPEAT)) as f32,
                        unit: "%".to_owned(),
                        text: repeat,
                    });
                    out.series.push(DisplaySeries {
                        name: "Partials".to_owned(),
                        unit: String::new(),
                        x_unit: "partial".to_owned(),
                        x_range: [1.0, levels.len() as f32],
                        log_x: false,
                        y_range: [0.0, 1.0],
                        detail: false,
                        values: levels,
                    });
                }
            }
        }
    }
    out
}

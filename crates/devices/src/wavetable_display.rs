//! What Wavetable's panel draws (WP-8.11; the wavetable spec, R35 to R38):
//! each oscillator's table decimated to at most [`TABLE_FRAMES`] frames of
//! [`POINTS`] points with the frame at Position, the two filters' combined
//! response with a dot for each, each envelope's shape with a handle on its
//! breakpoints, and each LFO's wave. A fixed size whatever the table (device
//! framework R35): the tables decimate once, off the audio thread, and the
//! rest comes from the parameters.

use lspk_wavetable::Wavetable;

use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DisplayHandle, DisplayMarker, DisplayReading,
    DisplaySeries, OrdMap, ParamId,
};

use crate::instrument_filter::Kind;
use crate::sampler::FILTER_TYPE_CHOICES;
use crate::wavetable::{
    ENV_ATTACK, ENV_ATTACK_SLOPE, ENV_DECAY, ENV_DECAY_SLOPE, ENV_FINAL, ENV_INITIAL, ENV_PEAK,
    ENV_RELEASE, ENV_RELEASE_SLOPE, ENV_SUSTAIN, FILTER_FREQ, FILTER_MORPH, FILTER_ON, FILTER_RES,
    FILTER_TYPE, LFO_AMOUNT, LFO_DIVISION, LFO_RATE, LFO_SHAPE, LFO_SYNC, LFO_WAVE, OSC_ON,
    POSITION, ROUTING, envelope, filter, lfo, osc,
};
use crate::wavetable_mod::{DIVISIONS, LFO_WAVES, Lfo, LfoParams, shape_power};

/// The frames of an oscillator's table the display draws, at most.
pub const TABLE_FRAMES: usize = 32;
/// The points each drawn frame has.
pub const POINTS: usize = 128;
/// The points of a filter, envelope or LFO curve.
const CURVE_POINTS: usize = 128;

/// The oscillator display's views: the first is the one it starts in.
pub const VIEWS: [&str; 2] = ["Perspective", "Polar"];

/// The names of the display cells.
pub const OSC_CELLS: [&str; 2] = ["Osc 1", "Osc 2"];
pub const FILTER_CELL: &str = "Filter";
pub const ENVELOPE_CELLS: [&str; 3] = ["Amp Envelope", "Env 2", "Env 3"];
pub const LFO_CELLS: [&str; 2] = ["LFO 1", "LFO 2"];

/// The frequencies a filter display runs between, in Hz.
const FILTER_RANGE: [f32; 2] = [20.0, 20_000.0];
/// What a filter display shows between its bottom and its top, in dB.
const FILTER_DB: [f32; 2] = [-48.0, 24.0];
/// The most Q a filter's section reaches (the instruments' filter, R4).
const Q_MAX: f64 = 10.0;

/// The shares of an envelope display's width its stages' times travel
/// across: Attack, Decay and Release (Sustain holds between).
pub const ATTACK_SPAN: [f32; 2] = [0.0, 0.28];
pub const DECAY_SPAN: [f32; 2] = [0.28, 0.56];
pub const RELEASE_SPAN: [f32; 2] = [0.72, 1.0];

/// A table drawn small: every frame at [`POINTS`] points, so the frame at any
/// Position can be drawn.
pub struct Preview {
    rows: Vec<[f32; POINTS]>,
}

impl Preview {
    /// The preview of `table`. Not for the audio thread.
    #[must_use]
    pub fn of(table: &Wavetable) -> Self {
        // shortcut: every 16th sample, not a low-passed one, so a bit-crushed
        // frame draws its steps; upgrade if a frame looks aliased.
        let rows = table
            .frames()
            .iter()
            .map(|frame| {
                let step = frame.len() / POINTS;
                std::array::from_fn(|i| frame[i * step])
            })
            .collect();
        Self { rows }
    }

    /// How many frames the table has.
    #[must_use]
    pub fn frames(&self) -> usize {
        self.rows.len()
    }

    /// The frame number (from 0) `position` (0 to 1 of the table) reads.
    fn frame_of(&self, position: f64) -> f64 {
        position.clamp(0.0, 1.0) * (self.rows.len() - 1) as f64
    }

    /// The frame at `position`, its neighbours blended.
    #[must_use]
    pub fn at(&self, position: f64) -> [f32; POINTS] {
        let at = self.frame_of(position);
        let (low, toward) = (at.floor() as usize, (at - at.floor()) as f32);
        let high = (low + 1).min(self.rows.len() - 1);
        std::array::from_fn(|i| self.rows[low][i] * (1.0 - toward) + self.rows[high][i] * toward)
    }

    /// The frames the display draws, back to front: all of a table of
    /// [`TABLE_FRAMES`] or fewer, else evenly picked.
    fn drawn(&self) -> Vec<&[f32; POINTS]> {
        let n = self.rows.len().min(TABLE_FRAMES);
        (0..n)
            .map(|k| {
                let at = if n == 1 {
                    0
                } else {
                    (k * (self.rows.len() - 1) + (n - 1) / 2) / (n - 1)
                };
                &self.rows[at]
            })
            .collect()
    }
}

fn series(
    name: String,
    unit: &str,
    (x_unit, x_range, log_x): (&str, [f32; 2], bool),
    y_range: [f32; 2],
    detail: bool,
    values: Vec<f32>,
) -> DisplaySeries {
    DisplaySeries {
        name,
        unit: unit.to_owned(),
        x_unit: x_unit.to_owned(),
        x_range,
        log_x,
        y_range,
        detail,
        values,
    }
}

fn hertz(hz: f64) -> String {
    if hz >= 1_000.0 {
        format!("{:.2} kHz", hz / 1_000.0)
    } else {
        format!("{hz:.0} Hz")
    }
}

type C = (f64, f64);

fn mul(a: C, b: C) -> C {
    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
}

fn div(a: C, b: C) -> C {
    let d = b.0 * b.0 + b.1 * b.1;
    ((a.0 * b.0 + a.1 * b.1) / d, (a.1 * b.0 - a.0 * b.1) / d)
}

fn q_of(q0: f64, res: f64) -> f64 {
    q0 * (Q_MAX / q0).powf(res)
}

/// One filter's response at `x` times its cutoff: the state-variable
/// prototype of the instruments' filter (R4), whatever the circuit, each
/// section's low-pass, band-pass, high-pass and notch tap, Morph a
/// crossfade between neighbours (R10).
fn response(entry: usize, morph: f64, res: f64, x: f64) -> C {
    let kind = Kind::of(entry / 2);
    let qs = if entry % 2 == 1 {
        vec![0.541_196_1, q_of(1.306_563, res)]
    } else {
        vec![q_of(std::f64::consts::FRAC_1_SQRT_2, res)]
    };
    qs.into_iter().fold((1.0, 0.0), |acc, q| {
        let den = (1.0 - x * x, x / q);
        let taps = [
            div((1.0, 0.0), den),
            div((0.0, x / q), den),
            div((-x * x, 0.0), den),
            div((1.0 - x * x, 0.0), den),
        ];
        let tap = match kind {
            Kind::LowPass => taps[0],
            Kind::BandPass => taps[1],
            Kind::HighPass => taps[2],
            Kind::Notch => taps[3],
            Kind::Morph => {
                // Low-pass at 0, band-pass at 25 %, high-pass at 50 %, notch
                // at 75 %, back to low-pass at 100 %.
                let place = morph.clamp(0.0, 1.0) * 4.0;
                let (a, b) = (place.floor() as usize % 4, (place.floor() as usize + 1) % 4);
                let t = place - place.floor();
                (
                    taps[a].0 * (1.0 - t) + taps[b].0 * t,
                    taps[a].1 * (1.0 - t) + taps[b].1 * t,
                )
            }
        };
        mul(acc, tap)
    })
}

fn magnitude(z: C) -> f64 {
    z.0.hypot(z.1)
}

fn db(linear: f64) -> f32 {
    (20.0 * linear.max(1e-6).log10()) as f32
}

/// A filter's settings as the panel has them.
struct Filter {
    on: bool,
    entry: usize,
    morph: f64,
    res: f64,
    freq: f64,
}

impl Filter {
    fn at(&self, hz: f64) -> C {
        if self.on {
            response(self.entry, self.morph, self.res, hz / self.freq)
        } else {
            (1.0, 0.0)
        }
    }
}

/// The filters' combined response at `hz` as the routing makes it (R16):
/// one after the other in Serial, the two paths averaged in Parallel and
/// Split.
fn combined(routing: usize, filters: &[Filter; 2], hz: f64) -> f64 {
    let (a, b) = (filters[0].at(hz), filters[1].at(hz));
    if routing == 0 {
        magnitude(mul(a, b))
    } else {
        magnitude(((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0))
    }
}

/// An envelope's levels at the display's `x` from 0 to 1: Attack rising from
/// Initial to Peak over its share of the width, Decay falling to Sustain
/// over its, Sustain holding, Release falling to Final.
fn envelope_level(edge: &EnvShape, x: f32) -> f32 {
    let at = |span: [f32; 2], travel: f32| span[0] + (span[1] - span[0]) * travel;
    let (attack, decay, release) = (
        at(ATTACK_SPAN, edge.times[0]),
        at(DECAY_SPAN, edge.times[1]),
        at(RELEASE_SPAN, edge.times[2]),
    );
    let sustain = edge.peak * edge.sustain;
    let stage = |from: f32, to: f32, start: f32, end: f32, power: f32| {
        let t = if end > start {
            ((x - start) / (end - start)).clamp(0.0, 1.0)
        } else {
            1.0
        };
        from + (to - from) * t.powf(power)
    };
    if x < attack {
        stage(edge.initial, edge.peak, 0.0, attack, edge.powers[0])
    } else if x < decay {
        stage(edge.peak, sustain, attack, decay, edge.powers[1])
    } else if x < RELEASE_SPAN[0] {
        sustain
    } else {
        stage(sustain, edge.fin, RELEASE_SPAN[0], release, edge.powers[2])
    }
}

/// An envelope as the display draws it.
struct EnvShape {
    /// Attack, Decay and Release's places on their travel, 0 to 1.
    times: [f32; 3],
    powers: [f32; 3],
    sustain: f32,
    initial: f32,
    peak: f32,
    fin: f32,
}

/// Wavetable's display for `device` from its parameters' `values`, and the
/// tables its oscillators play (`tables`, one for each if known): the
/// oscillators' tables with their Positions, the filters, the envelopes
/// and the LFOs.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    view: Option<&str>,
    tables: [Option<&Preview>; 2],
) -> DeviceDisplay {
    let value = |id: ParamId| description.value(values, id).unwrap_or(0.0);
    let travel = |id: ParamId| {
        description
            .param(id)
            .map_or(0.0, |p| p.normalize(value(id)) as f32)
    };
    let mut all = Vec::new();
    let mut markers = Vec::new();
    let mut readings = Vec::new();

    // The oscillators (R35).
    for (n, preview) in tables.into_iter().enumerate() {
        let name = OSC_CELLS[n];
        let position = value(osc(n + 1, POSITION)) / 100.0;
        let mut text = format!("{:.1} %", position * 100.0);
        if let Some(preview) = preview {
            let frame = preview.frame_of(position);
            text = format!(
                "{text}, frame {} of {}",
                frame.round() as usize + 1,
                preview.frames()
            );
            let axis = ("cycle", [0.0, 1.0], false);
            for (k, frame) in preview.drawn().into_iter().enumerate() {
                all.push(series(
                    format!("{name} Table {}", k + 1),
                    "",
                    axis,
                    [-1.0, 1.0],
                    true,
                    frame.to_vec(),
                ));
            }
            // An oscillator that is off draws no frame at its Position.
            if value(osc(n + 1, OSC_ON)) >= 0.5 {
                all.push(series(
                    format!("{name} Frame"),
                    "",
                    axis,
                    [-1.0, 1.0],
                    false,
                    preview.at(position).to_vec(),
                ));
            }
        }
        readings.push(DisplayReading {
            name: format!("{name} Position"),
            value: (position * 100.0) as f32,
            unit: "%".to_owned(),
            text,
        });
    }

    // The filters (R38).
    let filters: [Filter; 2] = std::array::from_fn(|i| {
        let n = i + 1;
        Filter {
            on: value(filter(n, FILTER_ON)) >= 0.5,
            entry: (value(filter(n, FILTER_TYPE)) as usize).min(FILTER_TYPE_CHOICES.len() - 1),
            morph: value(filter(n, FILTER_MORPH)) / 100.0,
            res: value(filter(n, FILTER_RES)) / 100.0,
            freq: value(filter(n, FILTER_FREQ)),
        }
    });
    let routing = value(ROUTING) as usize;
    let axis = ("Hz", FILTER_RANGE, true);
    let [low, high] = FILTER_RANGE.map(f64::from);
    let sweep = |f: &dyn Fn(f64) -> f64| -> Vec<f32> {
        (0..CURVE_POINTS)
            .map(|i| {
                let hz = low * (high / low).powf(i as f64 / (CURVE_POINTS - 1) as f64);
                db(f(hz))
            })
            .collect()
    };
    all.push(series(
        FILTER_CELL.to_owned(),
        "dB",
        axis,
        FILTER_DB,
        false,
        sweep(&|hz| combined(routing, &filters, hz)),
    ));
    for (i, f) in filters.iter().enumerate().filter(|(_, f)| f.on) {
        all.push(series(
            format!("{FILTER_CELL} {}", i + 1),
            "dB",
            axis,
            FILTER_DB,
            true,
            sweep(&|hz| magnitude(f.at(hz))),
        ));
    }
    for (i, f) in filters.iter().enumerate().filter(|(_, f)| f.on) {
        let n = i + 1;
        markers.push(DisplayMarker {
            name: format!("{FILTER_CELL} {n}"),
            series: FILTER_CELL.to_owned(),
            x: f.freq as f32,
            y: db(combined(routing, &filters, f.freq)),
            text: format!(
                "{FILTER_CELL} {n} {} {}, Resonance {:.0} %",
                FILTER_TYPE_CHOICES[f.entry],
                hertz(f.freq),
                f.res * 100.0
            ),
            handle: Some(DisplayHandle {
                x: filter(n, FILTER_FREQ),
                y: Some(filter(n, FILTER_RES)),
                span: None,
            }),
        });
    }

    // The envelopes (R38).
    for (e, name) in ENVELOPE_CELLS.into_iter().enumerate() {
        let level = |what, default: f32| {
            if e == 0 {
                default
            } else {
                (value(envelope(e, what)) / 100.0) as f32
            }
        };
        let shape = EnvShape {
            times: [
                travel(envelope(e, ENV_ATTACK)),
                travel(envelope(e, ENV_DECAY)),
                travel(envelope(e, ENV_RELEASE)),
            ],
            powers: [
                shape_power(value(envelope(e, ENV_ATTACK_SLOPE)) as f32 / 100.0),
                shape_power(value(envelope(e, ENV_DECAY_SLOPE)) as f32 / 100.0),
                shape_power(value(envelope(e, ENV_RELEASE_SLOPE)) as f32 / 100.0),
            ],
            sustain: (value(envelope(e, ENV_SUSTAIN)) / 100.0) as f32,
            initial: level(ENV_INITIAL, 0.0),
            peak: level(ENV_PEAK, 1.0),
            fin: level(ENV_FINAL, 0.0),
        };
        // A margin keeps the handles on the extremes whole inside the cell.
        let y_range = if e == 0 { [-0.08, 1.08] } else { [-1.08, 1.08] };
        all.push(series(
            name.to_owned(),
            "",
            ("stage", [0.0, 1.0], false),
            y_range,
            false,
            (0..CURVE_POINTS)
                .map(|i| envelope_level(&shape, i as f32 / (CURVE_POINTS - 1) as f32))
                .collect(),
        ));
        let at = |span: [f32; 2], travel: f32| span[0] + (span[1] - span[0]) * travel;
        let sustain = shape.peak * shape.sustain;
        // Each handle's text reads its horizontal parameter's value.
        let handle = |what: &str, x: f32, y: f32, params: (u32, Option<u32>), span| {
            let (across, up) = (
                envelope(e, params.0),
                params.1.map(|what| envelope(e, what)),
            );
            let read = description
                .param(across)
                .map_or_else(String::new, |p| p.format(value(across)));
            DisplayMarker {
                name: format!("{name} {what}"),
                series: name.to_owned(),
                x,
                y,
                text: format!("{name} {what} {read}"),
                handle: Some(DisplayHandle {
                    x: across,
                    y: up,
                    span: Some(span),
                }),
            }
        };
        markers.extend([
            handle(
                "Attack",
                at(ATTACK_SPAN, shape.times[0]),
                shape.peak,
                (ENV_ATTACK, (e > 0).then_some(ENV_PEAK)),
                ATTACK_SPAN,
            ),
            handle(
                "Decay",
                at(DECAY_SPAN, shape.times[1]),
                sustain,
                (ENV_DECAY, Some(ENV_SUSTAIN)),
                DECAY_SPAN,
            ),
            handle(
                "Release",
                at(RELEASE_SPAN, shape.times[2]),
                shape.fin,
                (ENV_RELEASE, (e > 0).then_some(ENV_FINAL)),
                RELEASE_SPAN,
            ),
        ]);
    }

    // The LFOs (R38): one cycle, or eight held values of a Random wave.
    for (i, name) in LFO_CELLS.into_iter().enumerate() {
        let n = i + 1;
        let params = LfoParams {
            wave: (value(lfo(n, LFO_WAVE)) as usize).min(LFO_WAVES.len() - 1),
            shape: (value(lfo(n, LFO_SHAPE)) / 100.0) as f32,
            amount: (value(lfo(n, LFO_AMOUNT)) / 100.0) as f32,
        };
        let random = params.wave == 4;
        let wave: Vec<f32> = (0..CURVE_POINTS)
            .map(|k| {
                let x = k as f64 / CURVE_POINTS as f64;
                let mut lfo = Lfo::IDLE;
                if random {
                    lfo.cycles = (x * 8.0) as u64;
                    lfo.salt = n as u64;
                } else {
                    lfo.phase = x;
                }
                lfo.wave(&params) * params.amount
            })
            .collect();
        all.push(series(
            name.to_owned(),
            "",
            ("cycle", [0.0, 1.0], false),
            [-1.1, 1.1],
            false,
            wave,
        ));
        let rate = if value(lfo(n, LFO_SYNC)) >= 0.5 {
            DIVISIONS[(value(lfo(n, LFO_DIVISION)) as usize).min(DIVISIONS.len() - 1)]
                .0
                .to_owned()
        } else {
            format!("{:.2} Hz", value(lfo(n, LFO_RATE)))
        };
        readings.push(DisplayReading {
            name: format!("{name} Wave"),
            value: params.wave as f32,
            unit: String::new(),
            text: format!("{}, {rate}", LFO_WAVES[params.wave]),
        });
    }

    DeviceDisplay {
        device,
        live: false,
        view: view.map(str::to_owned),
        series: all,
        spectrograms: Vec::new(),
        markers,
        readings,
    }
}

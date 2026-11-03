//! What the tests of the synthesisers share (Bass and Poli): notes, a render
//! in blocks of any size under the allocation trap, and measurements of what
//! it played.

use core::f64::consts::TAU;

use bl_engine::{
    Block, Node, NoteBuffer, NoteEvent, NoteExpression, NoteKind, Registry, TransportInfo,
};
use realfft::RealFftPlanner;

use super::{CHANNELS, RATE};

/// The sample rate, in hertz.
pub const HZ: f64 = 48_000.0;

/// The block sizes every render is checked at.
pub const BLOCKS: [usize; 4] = [1, 64, 512, 4096];

pub fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::register(&mut registry);
    registry
}

fn event(offset: usize, kind: NoteKind, key: u8, velocity: f32) -> NoteEvent {
    NoteEvent {
        offset: offset as u32,
        kind,
        channel: 0,
        key,
        velocity,
        note_id: None,
    }
}

pub fn on(frame: usize, key: u8) -> NoteEvent {
    event(frame, NoteKind::On, key, 1.0)
}

pub fn on_at(frame: usize, key: u8, velocity: f32) -> NoteEvent {
    event(frame, NoteKind::On, key, velocity)
}

pub fn off(frame: usize, key: u8) -> NoteEvent {
    event(frame, NoteKind::Off, key, 0.0)
}

/// A per-note bend of `value` (−1 to 1) on `key`.
pub fn bend(frame: usize, key: u8, value: f32) -> NoteEvent {
    event(
        frame,
        NoteKind::Expression {
            expression: NoteExpression::Bend,
            value,
        },
        key,
        0.0,
    )
}

/// The pitch wheel at `value` (−1 to 1).
pub fn wheel(frame: usize, value: f32) -> NoteEvent {
    event(frame, NoteKind::PitchBend { value }, 0, 0.0)
}

/// The mod wheel at `value` (0 to 1).
pub fn mod_wheel(frame: usize, value: f32) -> NoteEvent {
    event(
        frame,
        NoteKind::Control {
            controller: 1,
            value,
        },
        0,
        0.0,
    )
}

/// Plays `notes` (frames counted from the start, in any order) through
/// `node` for `frames`, in blocks of `block`, each a real-time section, and
/// returns both channels.
pub fn render(
    node: &mut dyn Node,
    block: usize,
    frames: usize,
    notes: &[NoteEvent],
) -> [Vec<f32>; 2] {
    let mut sorted = notes.to_vec();
    sorted.sort_by_key(|n| n.offset);
    let notes = &sorted[..];
    let mut out = [Vec::with_capacity(frames), Vec::with_capacity(frames)];
    let mut start = 0;
    while start < frames {
        let len = block.min(frames - start);
        let here: Vec<NoteEvent> = notes
            .iter()
            .filter(|n| (n.offset as usize) >= start && (n.offset as usize) < start + len)
            .map(|n| NoteEvent {
                offset: n.offset - start as u32,
                ..*n
            })
            .collect();
        let mut data = vec![0.0f32; CHANNELS * len];
        let mut sink = NoteBuffer::with_capacity(here.len().max(1));
        {
            let mut audio = Block::new(
                &mut data,
                CHANNELS,
                len,
                len,
                &[],
                TransportInfo::default(),
                RATE,
            )
            .with_notes(&here, &mut sink);
            bl_rt::rt_test!({ node.process(&mut audio) });
        }
        out[0].extend_from_slice(&data[..len]);
        out[1].extend_from_slice(&data[len..]);
        start += len;
    }
    out
}

/// The frequency of `key` with A4 at 440 Hz.
pub fn frequency(key: u8) -> f64 {
    440.0 * 2f64.powf((f64::from(key) - 69.0) / 12.0)
}

/// The root mean square of `signal`.
pub fn rms(signal: &[f32]) -> f64 {
    (signal
        .iter()
        .map(|&x| f64::from(x) * f64::from(x))
        .sum::<f64>()
        / signal.len() as f64)
        .sqrt()
}

/// The loudest frequency in `signal` between `low` and `high`, in hertz.
pub fn peak_frequency(signal: &[f32], low: f64, high: f64) -> f64 {
    const N: usize = 16_384;
    let frame = N.min(signal.len());
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(frame);
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    for ((slot, &x), n) in input.iter_mut().zip(&signal[..frame]).zip(0..frame) {
        let w = 0.5 - 0.5 * (TAU * n as f64 / frame as f64).cos();
        *slot = f64::from(x) * w;
    }
    fft.process(&mut input, &mut spectrum).unwrap();
    let bin = HZ / frame as f64;
    let mut best = (0.0, 0.0);
    for (k, value) in spectrum.iter().enumerate() {
        let hz = k as f64 * bin;
        if (low..high).contains(&hz) {
            let energy = value.norm_sqr();
            if energy > best.1 {
                best = (hz, energy);
            }
        }
    }
    best.0
}

/// The amplitude of `hz` in `signal`, by a Hann-windowed DFT over the whole
/// slice.
pub fn amplitude_at(signal: &[f32], hz: f64) -> f64 {
    let n = signal.len() as f64;
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, &x) in signal.iter().enumerate() {
        let w = 0.5 - 0.5 * (TAU * i as f64 / n).cos();
        let phase = TAU * hz * i as f64 / HZ;
        re += f64::from(x) * w * phase.cos();
        im -= f64::from(x) * w * phase.sin();
    }
    2.0 * re.hypot(im) / n
}

/// The energy outside a note's harmonics over the energy in them, in dB: the
/// aliasing level. A 7-term Blackman–Harris window, 8,192-frame spectra
/// overlapping by half, and 9 bins either side of each harmonic, as the
/// kit's own measurement does.
pub fn aliasing_db(signal: &[f32], fundamental: f64) -> f64 {
    const N: usize = 8_192;
    const A: [f64; 7] = [
        0.271_051_400_693_42,
        0.433_297_939_234_48,
        0.218_122_999_543_11,
        0.065_925_446_388_03,
        0.010_811_742_098_37,
        0.000_776_584_825_22,
        0.000_013_887_217_35,
    ];
    let window: Vec<f64> = (0..N)
        .map(|i| {
            let x = TAU * i as f64 / (N - 1) as f64;
            A.iter()
                .enumerate()
                .map(|(k, a)| {
                    let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
                    sign * a * (k as f64 * x).cos()
                })
                .sum()
        })
        .collect();
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(N);
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    let bin = HZ / N as f64;
    let margin = 9.0 * bin;
    let (mut harmonic, mut other) = (0.0, 0.0);
    for start in (0..=signal.len() - N).step_by(N / 2) {
        for ((slot, &x), &w) in input.iter_mut().zip(&signal[start..start + N]).zip(&window) {
            *slot = f64::from(x) * w;
        }
        fft.process(&mut input, &mut spectrum).unwrap();
        for (k, value) in spectrum.iter().enumerate() {
            let hz = k as f64 * bin;
            if hz < margin || hz >= HZ / 2.0 {
                continue;
            }
            let energy = value.norm_sqr();
            let lowest = ((hz - margin) / fundamental).ceil().max(1.0);
            let highest = ((hz + margin) / fundamental).floor();
            if lowest <= highest && lowest * fundamental < HZ / 2.0 {
                harmonic += energy;
            } else {
                other += energy;
            }
        }
    }
    10.0 * (other / harmonic).log10()
}

/// `signal`'s envelope: the peak of every 128-frame window.
pub fn envelope(signal: &[f32]) -> Vec<f32> {
    signal
        .chunks(128)
        .map(|window| window.iter().fold(0.0f32, |peak, x| peak.max(x.abs())))
        .collect()
}

/// Asserts `actual` is `expected` to within `tolerance` at every sample.
pub fn assert_near(actual: &[f32], expected: &[f32], tolerance: f32, what: &str) {
    assert_eq!(actual.len(), expected.len(), "{what}: length");
    let worst = (0..actual.len())
        .max_by(|&a, &b| {
            (actual[a] - expected[a])
                .abs()
                .total_cmp(&(actual[b] - expected[b]).abs())
        })
        .unwrap_or(0);
    assert!(
        (actual[worst] - expected[worst]).abs() <= tolerance,
        "{what}: sample {worst} is {} against {}",
        actual[worst],
        expected[worst]
    );
}

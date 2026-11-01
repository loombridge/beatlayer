//! The physical models against the device budget (physical-models spec,
//! R15): sixteen resonators of 32 partials, and sixteen waveguides, rendered
//! at 48 kHz in blocks of 128 frames, the architecture's capacity scenario.
//! Run with `cargo bench -p bl-dsp --bench physical`; it fails if a case takes
//! more than [`BUDGET`] of real time.

use std::hint::black_box;
use std::process::ExitCode;
use std::time::Instant;

use bl_dsp::physical::{
    Kind, ModalBank, ModalParams, ModalTable, Quality, Shape, WaveguideBank, WaveguideParams,
};

const RATE: f32 = 48_000.0;
const BLOCK: usize = 128;
const VOICES: usize = 16;
/// Seconds of audio rendered for each timing.
const SECONDS: usize = 2;

/// The share of one core's real time a device may take: the 128-track
/// capacity scenario gives each track under 1 %, and an instrument's sixteen
/// voices may take ten times that.
const BUDGET: f64 = 0.10;

/// Real seconds to render a second of audio: the best of five runs.
fn cost(mut render: impl FnMut(&[f32], &mut [f32])) -> f64 {
    let stride = VOICES;
    let mut input = vec![0.0; BLOCK * stride];
    let mut output = vec![0.0; BLOCK * stride * 2];
    let blocks = SECONDS * RATE as usize / BLOCK;
    (0..5)
        .map(|_| {
            let start = Instant::now();
            for block in 0..blocks {
                // A hit on every voice twice a second.
                input.fill(if block % (blocks / (2 * SECONDS)) == 0 {
                    0.5
                } else {
                    0.0
                });
                render(black_box(&input), &mut output);
            }
            black_box(&output);
            start.elapsed().as_secs_f64() / SECONDS as f64
        })
        .fold(f64::MAX, f64::min)
}

fn main() -> ExitCode {
    let mut over = false;
    let mut report = |name: &str, seconds: f64| {
        println!("{name}: {:.1} % of real time", seconds * 100.0);
        over |= seconds > BUDGET;
    };
    for (name, quality) in [
        ("8", Quality::Eco),
        ("32", Quality::Medium),
        ("64", Quality::High),
    ] {
        let mut bank = ModalBank::new(RATE, VOICES);
        drop(bank.set_table(ModalTable::new(Shape::Beam, quality, 1.0, 0.0)));
        for voice in 0..VOICES {
            let hz = 40.0 * (1.0 + voice as f32 / 8.0);
            bank.reset_voice(
                voice,
                &ModalParams {
                    hz,
                    decay: 2.0,
                    hit: 0.3,
                    pos_l: 0.6,
                    pos_r: 0.8,
                    ..ModalParams::default()
                },
            );
        }
        report(
            &format!("16 resonators of {name} partials"),
            cost(|input, output| bank.process(input, output)),
        );
        // Every control moving: the refresh work a sweep costs.
        let mut sweep = 0.0_f32;
        report(
            &format!("16 resonators of {name} partials, controls moving"),
            cost(|input, output| {
                sweep = (sweep + 0.01) % 1.0;
                for voice in 0..VOICES {
                    bank.set_params(
                        voice,
                        &ModalParams {
                            hz: 40.0 * (1.0 + voice as f32 / 8.0),
                            decay: 1.0 + sweep,
                            material: sweep,
                            brightness: sweep - 0.5,
                            hit: sweep,
                            pos_l: sweep,
                            pos_r: 1.0 - sweep,
                        },
                    );
                }
                bank.process(input, output);
            }),
        );
    }
    for kind in [Kind::String, Kind::Tube, Kind::Pipe] {
        let mut bank = WaveguideBank::new(RATE, VOICES, kind, 30.0);
        for voice in 0..VOICES {
            let hz = 40.0 * (1.0 + voice as f32 / 8.0);
            bank.reset_voice(
                voice,
                &WaveguideParams {
                    hz,
                    decay: 2.0,
                    inharm: 0.3,
                    opening: 0.8,
                    radius: 0.2,
                    ..WaveguideParams::default()
                },
            );
        }
        report(
            &format!("16 {kind:?} waveguides"),
            cost(|input, output| bank.process(input, output)),
        );
    }
    if over {
        eprintln!("over the budget of {:.0} % of real time", BUDGET * 100.0);
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

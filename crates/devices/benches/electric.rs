//! Electric against the device budget (electric spec, R12): sixteen voices
//! at 48 kHz in blocks of 128 frames, the architecture's capacity scenario.
//! Run with `cargo bench -p bl-devices --bench electric`; it fails if the
//! render takes more than [`BUDGET`] of real time.

use std::hint::black_box;
use std::process::ExitCode;
use std::time::Instant;

use bl_core::id::Id;
use bl_core::time::SampleRate;
use bl_devices::electric::{VOICES, electric_type};
use bl_engine::{Block, NodeConfig, NoteBuffer, NoteEvent, NoteKind, Registry, TransportInfo};
use bl_model::DeviceId;

const RATE: SampleRate = SampleRate::HZ_48000;
const BLOCK: usize = 128;
/// Seconds of audio rendered for each timing.
const SECONDS: usize = 4;

/// The share of one core's real time a device may take: the 128-track
/// capacity scenario gives each track under 1 %, and an instrument's sixteen
/// voices may take ten times that.
const BUDGET: f64 = 0.10;

/// Real seconds to render a second of audio, the notes held: the best of
/// three runs.
fn cost() -> f64 {
    let mut registry = Registry::empty();
    bl_devices::register(&mut registry);
    let mut device = electric_type().device(DeviceId(Id::new(1).expect("1 is an ID")));
    device.params.insert(VOICES, 16.0);
    let chord: Vec<NoteEvent> = (0..16)
        .map(|i| NoteEvent {
            offset: 0,
            kind: NoteKind::On,
            channel: 0,
            key: 36 + i * 3,
            velocity: 0.8,
            note_id: None,
        })
        .collect();
    (0..3)
        .map(|_| {
            let mut node = registry
                .create(
                    &device,
                    &NodeConfig {
                        round_trip_latency: 0,
                        sample_rate: RATE,
                        max_frames: BLOCK,
                        channels: 2,
                    },
                )
                .expect("the registry makes Electric");
            let mut out = NoteBuffer::with_capacity(16);
            let mut data = vec![0.0f32; 2 * BLOCK];
            let start = Instant::now();
            for block in 0..SECONDS * RATE.hz() as usize / BLOCK {
                data.fill(0.0);
                let notes: &[NoteEvent] = if block == 0 { &chord } else { &[] };
                let mut b = Block::new(
                    &mut data,
                    2,
                    BLOCK,
                    BLOCK,
                    &[],
                    TransportInfo::default(),
                    RATE,
                )
                .with_notes(notes, &mut out);
                node.process(&mut b);
                black_box(&data);
            }
            start.elapsed().as_secs_f64() / SECONDS as f64
        })
        .fold(f64::MAX, f64::min)
}

fn main() -> ExitCode {
    let seconds = cost();
    println!(
        "Electric, sixteen voices: {:.1} % of real time",
        seconds * 100.0
    );
    if seconds > BUDGET {
        eprintln!("over the budget of {:.0} %", BUDGET * 100.0);
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

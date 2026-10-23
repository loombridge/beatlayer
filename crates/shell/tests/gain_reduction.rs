//! The gain-reduction meter (IMP-25; the mixer view spec, R40): a track's
//! compressors and limiters report the gain they took off the last block, and
//! the engine publishes the most any of them reported with the track's
//! meters. Every block runs under the allocation trap.

use bl_core::time::SampleRate;
use bl_devices::bus_compressor::{
    ATTACK as BUS_ATTACK, BUS_COMPRESSOR, THRESHOLD as BUS_THRESHOLD,
};
use bl_devices::compressor::{ATTACK, COMPRESSOR, THRESHOLD};
use bl_devices::limiter::{CEILING, LIMITER};
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_engine::{Collector, Compiler, EngineConfig, Monitor, Processor, Registry, channel};
use bl_model::{Command, Device, DeviceId, DeviceKind, Editor, ParamId, TrackId, TrackKind};

const RATE: SampleRate = SampleRate::HZ_48000;
const MAX_FRAMES: usize = 4096;

/// A test tone at -6 dBFS, the compressor's and limiter's reference level.
fn tone() -> &'static [(ParamId, f64)] {
    &[(test_tone::FREQUENCY, 1000.0), (test_tone::LEVEL, -6.0)]
}

fn insert(editor: &mut Editor, track: TrackId, kind: &str, params: &[(ParamId, f64)]) -> DeviceId {
    let id = editor.new_id();
    let mut device = Device::new(
        id,
        "",
        DeviceKind::Native {
            device_type: kind.to_owned(),
        },
    );
    for &(param, value) in params {
        device.params.insert(param, value);
    }
    let index = editor.document().track(track).unwrap().devices().len();
    editor
        .apply(Command::InsertDevice {
            track,
            index,
            device,
        })
        .unwrap();
    id
}

fn track(editor: &mut Editor) -> TrackId {
    let id = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id,
            kind: TrackKind::Audio,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    id
}

/// A running engine with `devices` on one audio track, in order.
fn running(devices: &[(&str, &[(ParamId, f64)])]) -> (Processor, Monitor, TrackId) {
    let config = EngineConfig::new(RATE, MAX_FRAMES);
    let mut registry = Registry::new();
    bl_devices::register(&mut registry);
    let collector = Collector::new();
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, processor, monitor) = channel(&config);

    let mut editor = Editor::new();
    let id = track(&mut editor);
    insert(&mut editor, id, TEST_TONE, tone());
    for (kind, params) in devices {
        insert(&mut editor, id, kind, params);
    }
    let update = compiler.sync(editor.document());
    sender.send_update(update);
    assert!(sender.flush());
    (processor, monitor, id)
}

/// The most reduction the track's meter published while `blocks` blocks of
/// `block` frames ran, after the envelope has settled.
fn most_reduction(
    processor: &mut Processor,
    monitor: &mut Monitor,
    track: TrackId,
    block: usize,
) -> f32 {
    let mut most = 0.0f32;
    for _ in 0..(2 * 48_000 / block).max(2) {
        bl_rt::rt_test!({ processor.process(block) });
        most = most.min(monitor.state().gain_reduction(track).unwrap());
    }
    most
}

// Spec: mixer-view R40
#[test]
fn a_compressor_publishes_its_reduction_on_its_track_at_every_block_size() {
    for block in [1usize, 64, 512, 4096] {
        // A 4:1 ratio over a -14 dB threshold turns a -6 dBFS tone down 6 dB.
        let (mut processor, mut monitor, track) =
            running(&[(COMPRESSOR, &[(THRESHOLD, -14.0), (ATTACK, 0.01)])]);
        let most = most_reduction(&mut processor, &mut monitor, track, block);
        assert!(
            (most + 6.0).abs() < 0.5,
            "block size {block}: a 6 dB reduction reads as {most} dB"
        );
    }
}

// Spec: mixer-view R40
#[test]
fn a_track_without_a_compressor_reports_no_reduction() {
    let (mut processor, mut monitor, track) = running(&[]);
    assert_eq!(
        most_reduction(&mut processor, &mut monitor, track, 512),
        0.0,
        "a tone with no compressor or limiter reduces nothing"
    );
}

// Spec: mixer-view R40
#[test]
fn two_reducing_devices_combine_as_the_most_reduction() {
    // The compressor turns the tone down 1.5 dB, to a -7.5 dBFS peak; the
    // limiter's -18 dB ceiling then takes 10.5 dB off its input. The track
    // reads the most of the two, not their sum.
    let (mut processor, mut monitor, track) = running(&[
        (COMPRESSOR, &[(THRESHOLD, -8.0), (ATTACK, 0.01)]),
        (LIMITER, &[(CEILING, -18.0)]),
    ]);
    let most = most_reduction(&mut processor, &mut monitor, track, 512);
    assert!(
        (most + 10.5).abs() < 1.0,
        "the limiter's 10.5 dB reduction is the most: {most} dB"
    );
}

// Spec: mixer-view R40. Every native compressor and limiter reports, not only
// the Compressor and the Limiter: the Glue Compressor's reading reaches the
// track's meter too (Multiband Dynamics' is the device's own test).
#[test]
fn the_glue_compressor_reports_its_reduction() {
    // A 4:1 ratio over a -14 dB threshold turns a -6 dBFS tone down 6 dB.
    let (mut processor, mut monitor, track) = running(&[(
        BUS_COMPRESSOR,
        &[(BUS_THRESHOLD, -14.0), (BUS_ATTACK, 0.01)],
    )]);
    let most = most_reduction(&mut processor, &mut monitor, track, 512);
    assert!(
        (most + 6.0).abs() < 0.5,
        "the Glue Compressor's 6 dB reduction reads as {most} dB"
    );
}

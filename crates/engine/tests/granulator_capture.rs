//! WP-8.27's capture in the engine (the Granulator III spec R18 to R20, R22,
//! R26): a Granulator III keyed from a track captures that track's audio
//! sample for sample from the block it starts in, stops by itself after
//! Length, and plays under the allocation trap throughout. An offline render
//! of the same set captures the same audio.

use std::sync::Arc;

use crate::common::{Rig, device, key, track};
use bl_devices::granulator_iii::{
    CAPTURE_LENGTH, CaptureBuffer, CaptureCommand, granulator_iii_type,
};
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_model::{Command, DeviceId, Input, Output, Tap, TrackKind};

/// A second of audio at 48 kHz, Capture Length's least.
const SECOND: usize = 48_000;

/// What `track` plays to Main over `frames` frames: a test tone, alone.
fn tone(rig_blocks: usize) -> Vec<f32> {
    let mut rig = Rig::new(512);
    let t = track(&mut rig.editor, TrackKind::Audio, None);
    device(
        &mut rig.editor,
        t,
        TEST_TONE,
        &[(test_tone::FREQUENCY, 440.0), (test_tone::LEVEL, -6.0)],
    );
    rig.sync();
    rig.render(rig_blocks, 512)
}

/// A rig whose Granulator III is keyed from a tone track that is heard only
/// by it, and the granulator's device.
fn keyed() -> (Rig, DeviceId) {
    let mut rig = Rig::with_registry(512, bl_devices::granulator_iii::register);
    let ed = &mut rig.editor;
    let source = track(ed, TrackKind::Audio, None);
    device(
        ed,
        source,
        TEST_TONE,
        &[(test_tone::FREQUENCY, 440.0), (test_tone::LEVEL, -6.0)],
    );
    let host = track(ed, TrackKind::Midi, None);
    let id: DeviceId = ed.new_id();
    let mut granulator = granulator_iii_type().device(id);
    granulator.params.insert(CAPTURE_LENGTH, 1.0);
    ed.apply(Command::InsertDevice {
        track: host,
        index: 0,
        device: granulator,
    })
    .unwrap();
    ed.apply(Command::SetSidechainInput {
        device: id,
        input: Input::Track {
            track: source,
            tap: Tap::PostFx,
            chain: None,
            output: None,
        },
    })
    .unwrap();
    ed.apply(Command::SetOutput {
        track: source,
        output: Output::SendsOnly,
    })
    .unwrap();
    rig.sync();
    (rig, id)
}

fn start(rig: &mut Rig, device: DeviceId) -> Arc<CaptureBuffer> {
    let buffer = Arc::new(CaptureBuffer::new(SECOND));
    let data = rig
        .collector
        .handle()
        .data(CaptureCommand::Start(Arc::clone(&buffer)));
    rig.sender.send_data(key(device), data);
    assert!(rig.sender.flush(), "the queue is full");
    buffer
}

// Spec: granulator iii R19, R20
#[test]
fn a_capture_holds_the_sources_audio_sample_for_sample_and_stops_at_length() {
    let expected = tone(SECOND + 1_024);
    assert!(expected.iter().any(|s| s.abs() > 0.1), "the tone sounds");
    let (mut rig, device) = keyed();
    let buffer = start(&mut rig, device);
    // A block size that doesn't divide Length, so the capture ends inside one.
    rig.render(SECOND + 1_024, 500);
    assert!(buffer.finished());
    assert_eq!(buffer.filled(), SECOND);
    let [left, right] = buffer.read();
    assert_eq!(left, expected[..SECOND]);
    assert_eq!(right, expected[..SECOND]);
}

// Spec: granulator iii R26
#[test]
fn an_offline_render_captures_the_same_audio_at_any_block_size() {
    let (mut a, device_a) = keyed();
    let (mut b, device_b) = keyed();
    let (one, two) = (start(&mut a, device_a), start(&mut b, device_b));
    a.render(SECOND, 64);
    b.render(SECOND, 512);
    assert_eq!(one.read(), two.read());
}

// Spec: granulator iii R19
#[test]
fn stopping_ends_a_capture_where_it_is() {
    let (mut rig, device) = keyed();
    let buffer = start(&mut rig, device);
    rig.render(4_096, 512);
    let data = rig.collector.handle().data(CaptureCommand::Stop);
    rig.sender.send_data(key(device), data);
    assert!(rig.sender.flush());
    rig.render(4_096, 512);
    assert!(buffer.finished());
    assert_eq!(buffer.filled(), 4_096);
}

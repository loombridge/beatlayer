//! CV Triggers (WP-7.17; the CV tools spec, R16, R17, R18): the voltages its
//! eight outputs write for a phrase, read off the CV buffers a program would
//! give it, each block a real-time section under the allocation trap.

use crate::common;
use bl_core::time::Frames;
use bl_devices::cv_triggers::{self, Field, OUTPUTS, param};
use bl_devices::cv_triggers_type;
use bl_engine::{Block, Node, NoteBuffer, NoteEvent, NoteKind, Registry, TransportInfo};
use bl_model::ParamId;
use common::{CHANNELS, RATE, device, node};

/// The block sizes every render is checked at.
const BLOCKS: [usize; 4] = [1, 64, 512, 4096];

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::register(&mut registry);
    registry
}

/// A note: the frame, what it does, its key, its velocity.
#[derive(Clone, Copy)]
struct Played(usize, NoteKind, u8, f32);

fn on(frame: usize, key: u8) -> Played {
    Played(frame, NoteKind::On, key, 1.0)
}

fn off(frame: usize, key: u8) -> Played {
    Played(frame, NoteKind::Off, key, 0.0)
}

/// Renders `frames` frames of `notes` through a new device with `params`
/// set, at `block` frames a block, and returns what each output wrote.
fn render(
    params: &[(ParamId, f64)],
    block: usize,
    frames: usize,
    notes: &[Played],
) -> Vec<Vec<f32>> {
    let mut node = node(&registry(), &device(&cv_triggers_type(), params), 4096);
    let mut written = vec![Vec::new(); OUTPUTS];
    let mut start = 0;
    while start < frames {
        let len = block.min(frames - start);
        let block_notes: Vec<NoteEvent> = notes
            .iter()
            .filter(|n| n.0 >= start && n.0 < start + len)
            .map(|&Played(at, kind, key, velocity)| NoteEvent {
                offset: (at - start) as u32,
                kind,
                channel: 0,
                key,
                velocity,
                note_id: None,
            })
            .collect();
        let pitch = CHANNELS * len;
        let mut data = vec![0.0f32; CHANNELS * len];
        let mut cv = vec![0.0f32; OUTPUTS * pitch];
        let mut sink = NoteBuffer::with_capacity(block_notes.len().max(1));
        {
            let transport = TransportInfo {
                playing: true,
                position: Frames(start as i64),
            };
            let mut block = Block::new(&mut data, CHANNELS, len, len, &[], transport, RATE)
                .with_notes(&block_notes, &mut sink)
                .with_cv_out(&mut cv);
            let node: &mut dyn Node = &mut *node;
            bl_rt::rt_test!({ node.process(&mut block) });
        }
        for (output, out) in written.iter_mut().enumerate() {
            out.extend_from_slice(&cv[output * pitch..output * pitch + len]);
        }
        start += len;
    }
    written
}

// Spec: cv-tools R16, R17
#[test]
fn a_trigger_is_a_pulse_of_its_length_on_its_key_and_nothing_on_others() {
    for block in BLOCKS {
        // Key 36 answers output 1, a 10 ms pulse is 480 frames at 5 V.
        let out = render(
            &[],
            block,
            2000,
            &[on(100, 36), off(150, 36), on(900, 37), on(1200, 90)],
        );
        assert_eq!(out[0][99], 0.0);
        assert!(
            out[0][100..580].iter().all(|&v| v == 0.5),
            "block size {block}"
        );
        assert_eq!(out[0][580], 0.0, "the pulse ends after its length");
        assert!(
            out[0][580..].iter().all(|&v| v == 0.0),
            "the note off changes nothing"
        );
        // Key 37 answers output 2, a key no output answers does nothing.
        assert!(out[1][900..1380].iter().all(|&v| v == 0.5));
        assert_eq!(out[1][1380], 0.0);
        for other in 2..OUTPUTS {
            assert!(out[other].iter().all(|&v| v == 0.0), "output {other}");
        }
    }
}

// Spec: cv-tools R17
#[test]
fn a_note_during_a_pulse_restarts_it_after_a_millisecond_at_zero() {
    let out = render(&[], 64, 2000, &[on(100, 36), on(300, 36)]);
    assert_eq!(out[0][299], 0.5);
    assert!(
        out[0][300..348].iter().all(|&v| v == 0.0),
        "48 frames at 48 kHz"
    );
    assert!(
        out[0][348..828].iter().all(|&v| v == 0.5),
        "a new pulse of 480 frames"
    );
    assert_eq!(out[0][828], 0.0);
}

// Spec: cv-tools R17
#[test]
fn a_gate_is_held_from_the_note_on_to_the_note_off() {
    let mode = param(0, Field::Mode);
    for block in BLOCKS {
        let out = render(&[(mode, 1.0)], block, 2000, &[on(100, 36), off(1000, 36)]);
        assert_eq!(out[0][99], 0.0);
        assert!(out[0][100..1000].iter().all(|&v| v == 0.5));
        assert_eq!(out[0][1000], 0.0);
        // A second note on while it is high takes it down for 1 ms first.
        let out = render(
            &[(mode, 1.0)],
            block,
            2000,
            &[on(100, 36), on(500, 36), off(1000, 36)],
        );
        assert!(out[0][500..548].iter().all(|&v| v == 0.0));
        assert!(out[0][548..1000].iter().all(|&v| v == 0.5));
    }
}

// Spec: cv-tools R16
#[test]
fn velocity_scales_the_level_and_outputs_may_share_a_key() {
    let note = param(1, Field::Note);
    let velocity = param(0, Field::Velocity);
    let level = param(0, Field::Level);
    let out = render(
        &[(note, 36.0), (velocity, 1.0), (level, 8.0)],
        64,
        1000,
        &[Played(100, NoteKind::On, 36, 0.5)],
    );
    // Output 1 scales 8 V by the velocity of half, output 2 shares the key.
    assert!((out[0][200] - 0.4).abs() < 1e-6);
    assert_eq!(out[1][200], 0.5);
}

// Spec: cv-tools R18
#[test]
fn the_outputs_fall_when_the_device_lets_go() {
    let mode = param(0, Field::Mode);
    let mut node = node(
        &registry(),
        &device(&cv_triggers_type(), &[(mode, 1.0)]),
        4096,
    );
    let note = NoteEvent {
        offset: 0,
        kind: NoteKind::On,
        channel: 0,
        key: 36,
        velocity: 1.0,
        note_id: None,
    };
    let mut run = |notes: &[NoteEvent], node: &mut Box<dyn Node>| {
        let mut data = vec![0.0f32; CHANNELS * 64];
        let mut cv = vec![0.0f32; OUTPUTS * CHANNELS * 64];
        let mut sink = NoteBuffer::with_capacity(4);
        let mut block = Block::new(
            &mut data,
            CHANNELS,
            64,
            64,
            &[],
            TransportInfo::default(),
            RATE,
        )
        .with_notes(notes, &mut sink)
        .with_cv_out(&mut cv);
        bl_rt::rt_test!({ node.process(&mut block) });
        cv[0]
    };
    assert_eq!(run(&[note], &mut node), 0.5);
    let mut sink = NoteBuffer::with_capacity(4);
    node.release(&mut sink);
    assert_eq!(run(&[], &mut node), 0.0);
    let _ = cv_triggers::OUTPUT_NAMES;
}

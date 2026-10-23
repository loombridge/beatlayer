//! Velocity, the velocity MIDI effect (WP-5.30; the MIDI velocity spec): the curve
//! from the input window to the output range, the operation, the three modes,
//! Random, Drive and Compand. Every block is a real-time section under the
//! allocation trap.

use crate::common;
use bl_devices::touch::{
    COMPAND, DRIVE, LOWEST, MODE, OPERATION, OUT_HIGH, OUT_LOW, RANDOM, RANGE, TOUCH, shape,
};
use bl_engine::NoteKind;
use common::chain::{BEAT_FRAMES, Chain, note};

const SIZES: [usize; 4] = [1, 100, 512, 4096];

fn velocities(params: &[(bl_model::ParamId, f64)], size: usize) -> Vec<(NoteKind, f32)> {
    let mut chain = Chain::new(4096);
    chain.effect(TOUCH, params);
    chain.clip(4, [note(60, 0, 1), note(64, 2, 1)]);
    chain.sync();
    chain.play();
    chain.run(4 * BEAT_FRAMES as usize, size);
    chain.heard().iter().map(|h| (h.1, h.3)).collect()
}

// Spec: midi-velocity R1. A velocity lands where the curve puts it between
// Out Low and Out High, on the note's own frame.
#[test]
fn the_curve_maps_the_input_window_to_the_output_range() {
    for size in SIZES {
        // 100 of 127 is 0.787 of the way up the window; 40 + 0.787 × 40 = 71.
        let heard = velocities(&[(OUT_LOW, 40.0), (OUT_HIGH, 80.0)], size);
        assert_eq!(heard[0], (NoteKind::On, 71.0 / 127.0), "size {size}");
        assert_eq!(heard[2], (NoteKind::On, 71.0 / 127.0));
        // The release velocity isn't touched by Note On.
        assert_eq!(heard[1], (NoteKind::Off, 64.0 / 127.0));
    }
}

// Spec: midi-velocity R2. Operation picks note-on velocity, release velocity,
// or both.
#[test]
fn operation_picks_what_the_curve_is_applied_to() {
    let release = [(OUT_LOW, 10.0), (OUT_HIGH, 20.0)];
    // 64 of 127 is 0.504 up: 10 + 0.504 × 10 = 15.
    let only = |operation| {
        let mut params = release.to_vec();
        params.push((OPERATION, operation));
        velocities(&params, 512)
    };
    let release_only = only(1.0);
    assert_eq!(release_only[0], (NoteKind::On, 100.0 / 127.0));
    assert_eq!(release_only[1], (NoteKind::Off, 15.0 / 127.0));
    let both = only(2.0);
    assert_eq!(
        both[0].1,
        18.0 / 127.0,
        "100 of 127 is 0.787 up: 10 + 7.9 = 18"
    );
    assert_eq!(both[1], (NoteKind::Off, 15.0 / 127.0));
    let note_on = only(0.0);
    assert_eq!(note_on[1], (NoteKind::Off, 64.0 / 127.0));
}

// Spec: midi-velocity R3. Clip holds a velocity outside the window at its edge.
#[test]
fn clip_holds_velocities_at_the_windows_edges() {
    let heard = velocities(
        &[
            (LOWEST, 110.0),
            (RANGE, 10.0),
            (OUT_LOW, 30.0),
            (OUT_HIGH, 90.0),
            (MODE, 0.0),
        ],
        512,
    );
    assert_eq!(heard[0], (NoteKind::On, 30.0 / 127.0));
}

// Spec: midi-velocity R4. Gate drops notes outside the window, and their
// note-offs with them.
#[test]
fn gate_drops_notes_outside_the_window() {
    let heard = velocities(&[(LOWEST, 110.0), (RANGE, 10.0), (MODE, 1.0)], 512);
    assert!(heard.is_empty(), "{heard:?}");
    let inside = velocities(&[(LOWEST, 90.0), (RANGE, 30.0), (MODE, 1.0)], 512);
    assert_eq!(inside.len(), 4);
}

// Spec: midi-velocity R5. Fixed gives every note the Out High velocity.
#[test]
fn fixed_gives_every_note_out_high() {
    let heard = velocities(&[(OUT_HIGH, 90.0), (MODE, 2.0)], 512);
    assert_eq!(heard[0], (NoteKind::On, 90.0 / 127.0));
    assert_eq!(heard[2], (NoteKind::On, 90.0 / 127.0));
}

// Spec: midi-velocity R6. Drive bends the curve toward loud or soft, and
// Compand toward the extremes or the middle.
#[test]
fn drive_and_compand_bend_the_curve() {
    assert_eq!(shape(0.5, 0.0, 0.0), 0.5);
    assert!(shape(0.5, 1.0, 0.0) > 0.8, "positive Drive is louder");
    assert!(shape(0.5, -1.0, 0.0) < 0.2, "negative Drive is softer");
    assert!(
        shape(0.25, 0.0, 1.0) < 0.25,
        "positive Compand pushes down low values"
    );
    assert!(shape(0.75, 0.0, 1.0) > 0.75, "and up high ones");
    assert!(
        shape(0.25, 0.0, -1.0) > 0.25,
        "negative Compand squeezes to the middle"
    );
    assert!(shape(0.75, 0.0, -1.0) < 0.75);
    for x in [0.0, 0.3, 1.0] {
        for d in [-1.0, 0.0, 1.0] {
            for c in [-1.0, 0.0, 1.0] {
                assert!((0.0..=1.0).contains(&shape(x, d, c)));
            }
        }
    }
    // Through the device: Drive makes the same note louder.
    let plain = velocities(&[], 512)[0].1;
    let driven = velocities(&[(DRIVE, 100.0)], 512)[0].1;
    assert!(driven > plain);
    let companded = velocities(&[(COMPAND, 100.0)], 512)[0].1;
    assert!(companded > plain, "100 of 127 is above the middle");
}

// Spec: midi-velocity R7. Random moves velocities inside the output range,
// and two renders agree to the bit.
#[test]
fn random_stays_in_the_output_range_and_repeats() {
    let params = [(RANDOM, 60.0), (OUT_LOW, 50.0), (OUT_HIGH, 110.0)];
    let render = || {
        let mut chain = Chain::new(4096);
        chain.effect(TOUCH, &params);
        chain.clip(8, (0..8).map(|b| note(60 + b as u8, b, 1)));
        chain.sync();
        chain.play();
        chain.run(8 * BEAT_FRAMES as usize, 512);
        chain.heard()
    };
    let (first, second) = (render(), render());
    assert_eq!(first, second, "two renders are bit for bit the same");
    let ons: Vec<f32> = first
        .iter()
        .filter(|h| h.1 == NoteKind::On)
        .map(|h| h.3 * 127.0)
        .collect();
    assert!(ons.iter().all(|v| (49.9..=110.1).contains(v)), "{ons:?}");
    assert!(
        ons.windows(2).any(|w| (w[0] - w[1]).abs() > 0.5),
        "they vary: {ons:?}"
    );
}

// Spec: midi-velocity R4. A velocity on the window's edge is inside it.
#[test]
fn gate_keeps_a_velocity_on_the_window_edge() {
    let heard = velocities(&[(LOWEST, 100.0), (RANGE, 27.0), (MODE, 1.0)], 512);
    assert_eq!(heard.len(), 4, "{heard:?}");
}

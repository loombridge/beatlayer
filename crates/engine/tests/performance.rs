//! The Performance Impact indicator (the mixer spec, R31): the engine
//! times each task on the thread that ran it, and publishes each track's
//! share of the set's processing time with the meters.
//!
//! Every block runs as a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_engine::testing;
use bl_model::TrackKind;
use common::{Rig, device, track};

// Spec: the mixer spec, R31.
#[test]
fn a_track_with_more_devices_has_the_bigger_share_of_the_processing_time() {
    // The pool runs the tasks on workers of their own, and the engine
    // times each task on the thread that ran it (WP-5.37).
    let mut rig = Rig::with_workers(64, 2);
    let busy = track(&mut rig.editor, TrackKind::Audio, None);
    let light = track(&mut rig.editor, TrackKind::Audio, None);
    for _ in 0..20 {
        device(
            &mut rig.editor,
            busy,
            testing::SPIN,
            &[(testing::SPIN_MICROS, 100.0)],
        );
    }
    device(
        &mut rig.editor,
        light,
        testing::SPIN,
        &[(testing::SPIN_MICROS, 100.0)],
    );
    rig.sync();
    // The times are smoothed, so the shares settle over the blocks.
    for _ in 0..24 {
        rig.block(64);
    }
    let state = rig.monitor.state();
    let share = |track| {
        state
            .meters
            .iter()
            .find(|meter| meter.track == track)
            .unwrap_or_else(|| panic!("a meter for the track"))
            .impact
    };
    let (busy, light) = (share(busy), share(light));
    assert!(
        busy > light,
        "the track with twenty devices has the bigger share: {busy} against {light}"
    );
    assert!(busy > 0.8, "and the bigger one: {busy}");
    assert!(light < 0.2, "and the smaller one: {light}");
}

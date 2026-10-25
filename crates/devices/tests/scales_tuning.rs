//! Scales under a tuning (WP-6.10; the tunings spec, R16): while the set's
//! tuning holds other than twelve notes to its pseudo-octave, a scale-aware
//! device's Use Current Scale acts as off; a tuning of twelve notes changes
//! nothing. Every block runs as a real-time section under the allocation trap.

use crate::common;
use bl_devices::lift::{LIFT, PITCH, USE_SCALE};
use bl_model::{Command, Scale, ScaleName, Tuning};
use common::chain::{BEAT_FRAMES, Chain, note};

fn equal(steps: u32) -> Tuning {
    let cents = (1..=steps)
        .map(|i| 1200.0 * f64::from(i) / f64::from(steps))
        .collect();
    Tuning::new(format!("{steps} equal"), String::new(), cents).unwrap()
}

/// The key Pitch, moving C3 by one degree of C Major with Use Current Scale
/// on, sends it to while the set has `tuning`; it is D3 (62) counting
/// degrees, and C#3 (61) counting semitones.
fn plays(tuning: Option<Tuning>) -> u8 {
    let mut chain = Chain::new(512);
    chain.effect(LIFT, &[(PITCH, 1.0), (USE_SCALE, 1.0)]);
    chain.clip_at(0, 2, Scale::new(0, ScaleName::Major), [note(60, 0, 1)]);
    if let Some(tuning) = tuning {
        chain
            .editor
            .apply(Command::SetTuning {
                tuning,
                retune: false,
            })
            .unwrap();
    }
    chain.sync();
    chain.play();
    chain.run(2 * BEAT_FRAMES as usize, 512);
    chain.heard()[0].2
}

// Spec: tunings R16
#[test]
fn use_current_scale_acts_as_off_under_a_tuning_of_other_than_twelve_notes() {
    assert_eq!(plays(None), 62, "no tuning: degrees of the scale");
    assert_eq!(plays(Some(equal(12))), 62, "twelve notes change nothing");
    assert_eq!(plays(Some(equal(19))), 61, "19 notes: semitones, as off");
    assert_eq!(plays(Some(equal(5))), 61, "5 notes: semitones, as off");
}

// Spec: tunings R16. The scales come back with the tuning's going.
#[test]
fn the_scale_is_followed_again_once_the_tuning_goes() {
    let mut chain = Chain::new(512);
    chain.effect(LIFT, &[(PITCH, 1.0), (USE_SCALE, 1.0)]);
    chain.clip_at(
        0,
        4,
        Scale::new(0, ScaleName::Major),
        [note(60, 0, 1), note(60, 2, 1)],
    );
    chain
        .editor
        .apply(Command::SetTuning {
            tuning: equal(19),
            retune: false,
        })
        .unwrap();
    chain.sync();
    chain.play();
    chain.run(BEAT_FRAMES as usize + 1, 512);
    chain
        .editor
        .apply(Command::RemoveTuning { retune: false })
        .unwrap();
    chain.sync();
    chain.run(3 * BEAT_FRAMES as usize, 512);
    let ons: Vec<u8> = chain
        .heard()
        .iter()
        .filter(|h| h.1 == bl_engine::NoteKind::On)
        .map(|h| h.2)
        .collect();
    assert_eq!(ons, [61, 62]);
}

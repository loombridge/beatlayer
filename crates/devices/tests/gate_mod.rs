//! The Envelope MIDI (WP-5.11; the modulation spec, R25): each note starts its
//! envelope from the level it is at, the last held key's end starts the
//! release, Sustain on holds the level while a key is held and off releases
//! after the decay, and the loop modes start it again as set. The notes pass
//! through untouched. Every block runs as a real-time section under the
//! allocation trap.

bl_rt::install_trap!();

mod common;

use bl_core::time::{Frames, TempoMap};
use bl_devices::gate_mod::{
    ATTACK, DECAY, ECHO_FEEDBACK, ECHO_TIME, LOOP_MODE, RELEASE, SUSTAIN_LEVEL, SUSTAIN_ON,
    SYNC_RATE, VELOCITY, gate_mod_type,
};
use bl_engine::{Node, NoteEvent, Registry, TransportInfo};
use bl_model::ParamId;
use bl_model::modulator::{self, POLARITY};
use common::{Signal, device, node, note_off, note_on, signal};

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::gate_mod::register(&mut registry);
    registry
}

fn playing(at: usize) -> TransportInfo {
    TransportInfo {
        playing: true,
        position: Frames(at as i64),
    }
}

/// An Envelope MIDI whose slot 1 gives its envelope itself, with an attack of
/// 20 ms (960 frames), a decay of 40 ms (1,920), a sustain of 50 % and a
/// release of 100 ms (4,800); `params` over those.
fn gate(params: &[(ParamId, f64)]) -> Box<dyn Node> {
    let mut all = vec![
        (modulator::amount(0), 100.0),
        (POLARITY, 1.0),
        (ATTACK, 20.0),
        (DECAY, 40.0),
        (SUSTAIN_LEVEL, 50.0),
        (RELEASE, 100.0),
    ];
    all.extend_from_slice(params);
    node(&registry(), &device(&gate_mod_type(), &all), 512)
}

fn run(node: &mut dyn Node, frames: usize, block: usize, notes: &[NoteEvent]) -> Signal {
    signal(
        node,
        frames,
        block,
        &[],
        None,
        notes,
        &[],
        &TempoMap::default(),
        playing,
    )
}

/// How many times `level` climbs through `threshold`.
fn rises(level: &[f32], threshold: f32) -> usize {
    level
        .windows(2)
        .filter(|w| w[0] < threshold && w[1] >= threshold)
        .count()
}

// Spec: modulation R25
#[test]
fn each_note_starts_the_envelope_and_the_last_release_ends_it() {
    let one = note_on(1_000, 60, 1.0, None);
    let two = note_on(5_000, 64, 1.0, None);
    let notes = [
        one,
        two,
        note_off(6_000, &one, 0.5),
        note_off(8_000, &two, 0.5),
    ];
    let mut node = gate(&[]);
    let out = run(node.as_mut(), 18_000, 512, &notes);
    let level = out.rows.contribution(0);
    assert!(
        level[..1_000].iter().all(|&v| v == 0.0),
        "nothing before the first note"
    );
    assert!(
        (level[4_800] - 0.5).abs() < 0.03,
        "held at Sustain: {}",
        level[4_800]
    );

    // The second note starts from where the envelope is, which is the
    // sustain, not from nothing: it rises to the peak from there.
    assert!(
        level[5_000..5_900].iter().all(|&v| v >= 0.45),
        "no fall to nothing"
    );
    assert!(level[5_600] > level[5_100], "rising");
    assert!(level[5_900..6_100].iter().any(|&v| v > 0.97), "to the peak");
    // The first key's end, with the second still held, releases nothing.
    assert!((level[7_950] - 0.5).abs() < 0.03, "{}", level[7_950]);
    // The last key's end starts the release from the sustain.
    assert!(
        level[8_200..].windows(2).all(|w| w[1] <= w[0] + 1e-6),
        "falling"
    );
    assert!(level[8_000 + 5_000] < 0.01, "and gone: {}", level[13_000]);

    // The notes pass through untouched.
    assert_eq!(out.notes, notes.to_vec());
}

// Spec: modulation R25
#[test]
fn sustain_off_releases_after_the_decay() {
    let on = note_on(1_000, 60, 1.0, None);
    // The key stays down for the whole run.
    let mut held = gate(&[]);
    let sustained = run(held.as_mut(), 16_000, 512, &[on]);
    assert!((sustained.rows.contribution(0)[15_000] - 0.5).abs() < 0.03);

    let mut node = gate(&[(SUSTAIN_ON, 0.0)]);
    let out = run(node.as_mut(), 16_000, 512, &[on]);
    let level = out.rows.contribution(0);
    assert!(level[1_900] > 0.9, "the attack runs: {}", level[1_900]);
    // 960 + 1,920 + 4,800 frames after the note, with the key still down.
    assert!(level[1_000 + 960 + 1_920 + 4_800 + 200] < 0.01);
    assert!(level[15_000] < 0.01);
}

// Spec: modulation R25
#[test]
fn velocity_scales_the_peak_when_it_is_on() {
    let on = note_on(500, 60, 0.5, None);
    let peak = |params: &[(ParamId, f64)]| {
        let mut node = gate(params);
        let out = run(node.as_mut(), 6_000, 512, &[on]);
        out.rows.contribution(0).iter().copied().fold(0.0, f32::max)
    };
    assert!((peak(&[]) - 1.0).abs() < 0.02, "off: the peak is 1");
    assert!(
        (peak(&[(VELOCITY, 1.0)]) - 0.5).abs() < 0.02,
        "on: the velocity"
    );
}

// Spec: modulation R25
#[test]
fn loop_modes_retrigger_as_set() {
    let on = note_on(0, 60, 1.0, None);
    // A held key through 80,000 frames, 1⅔ s.
    let count = |params: &[(ParamId, f64)]| {
        let mut node = gate(params);
        let out = run(node.as_mut(), 80_000, 512, &[on]);
        rises(out.rows.contribution(0), 0.9)
    };
    assert_eq!(
        count(&[(LOOP_MODE, 0.0)]),
        1,
        "Free: the note starts it once"
    );
    // Sync every quarter note, 24,000 frames at 120 BPM (index 3 of the
    // divisions): at 0, 24,000, 48,000 and 72,000.
    assert_eq!(count(&[(LOOP_MODE, 1.0), (SYNC_RATE, 3.0)]), 4, "Sync");
    // Loop: attack, decay and release over and over, 7,680 frames a cycle:
    // at 0, 7,680, … 76,800.
    assert_eq!(count(&[(LOOP_MODE, 2.0)]), 11, "Loop");

    // Echo: a note that is over in a moment, 100 ms between echoes, each a
    // half of the last.
    let short = [on, note_off(100, &on, 0.5)];
    let mut node = gate(&[
        (LOOP_MODE, 3.0),
        (ECHO_TIME, 100.0),
        (ECHO_FEEDBACK, 50.0),
        (SUSTAIN_ON, 0.0),
        (ATTACK, 10.0),
        (DECAY, 20.0),
        (RELEASE, 1.0),
    ]);
    let out = run(node.as_mut(), 4_800 * 4, 512, &short);
    let level = out.rows.contribution(0);
    for (echo, expected) in [1.0f32, 0.5, 0.25, 0.125].into_iter().enumerate() {
        let window = &level[echo * 4_800..(echo + 1) * 4_800];
        let peak = window.iter().copied().fold(0.0, f32::max);
        assert!(
            (peak - expected).abs() < 0.03,
            "echo {echo}: {peak} for {expected}"
        );
    }
}

// Spec: modulation R25
#[test]
fn the_envelope_is_the_same_at_any_block_size() {
    let one = note_on(1_000, 60, 0.8, None);
    let notes = [one, note_off(7_000, &one, 0.5)];
    let params = [(VELOCITY, 1.0), (ECHO_FEEDBACK, 40.0)];
    let mut node = gate(&params);
    let reference = run(node.as_mut(), 16_000, 512, &notes);
    for block in [1, 32, 64] {
        let mut again = gate(&params);
        let out = run(again.as_mut(), 16_000, block, &notes);
        assert_eq!(out.rows.0, reference.rows.0, "blocks of {block} frames");
    }
}

// Spec: modulation R25
#[test]
fn a_repeated_note_on_is_one_key_and_its_note_off_lifts_it() {
    let one = note_on(1_000, 60, 1.0, None);
    let again = note_on(2_000, 60, 1.0, None);
    let notes = [one, again, note_off(3_000, &again, 0.5)];
    let mut node = gate(&[]);
    let out = run(node.as_mut(), 16_000, 512, &notes);
    let level = out.rows.contribution(0);
    assert!(level[2_900] > 0.4, "held: {}", level[2_900]);
    assert!(
        level[15_000..].iter().all(|&v| v < 0.01),
        "released: {}",
        level[15_000]
    );
}

// Spec: modulation R25
#[test]
fn echoes_end_when_loop_mode_moves_away_from_echo() {
    let one = note_on(1_000, 60, 1.0, None);
    let notes = [one, note_off(1_500, &one, 0.5)];
    let params = [
        (LOOP_MODE, 3.0),
        (ECHO_TIME, 100.0),
        (ECHO_FEEDBACK, 90.0),
        (ATTACK, 10.0),
        (DECAY, 20.0),
        (RELEASE, 1.0),
    ];
    let mut node = gate(&params);
    let free = [common::set(6_000, LOOP_MODE, 0.0)];
    let out = signal(
        node.as_mut(),
        30_000,
        512,
        &[],
        None,
        &notes,
        &free,
        &TempoMap::default(),
        playing,
    );
    let level = out.rows.contribution(0);
    assert!(rises(&level[..6_000], 0.3) >= 2, "echoes run in Echo mode");
    assert_eq!(rises(&level[7_000..], 0.1), 0, "none after the change");
}

// Spec: modulation R20, R28
#[test]
fn switched_on_again_it_leaves_no_stale_late_rows() {
    let one = note_on(100, 60, 1.0, None);
    let mut node = gate(&[]);
    let out = run(node.as_mut(), 1_000, 32, &[one]);
    assert!(
        out.rows.contribution(0)[900] > 0.4,
        "the first stretch writes"
    );
    // Switched off for 1,000 frames of the song, then on.
    node.release(&mut bl_engine::NoteBuffer::with_capacity(8));
    let out = signal(
        node.as_mut(),
        64,
        32,
        &[],
        None,
        &[],
        &[],
        &TempoMap::default(),
        |at| playing(2_000 + at),
    );
    assert!(
        out.late.contribution(0)[..32].iter().all(|&v| v == 0.0),
        "the late rows hold nothing written before the gap"
    );
}

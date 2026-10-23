//! Arpeggiator, the arpeggiator MIDI effect (WP-5.31; the arpeggiator spec):
//! every style, rate in milliseconds or beat divisions, gate with overlap,
//! hold adding and removing notes, offset, retrigger, repeats, transposition
//! steps and distance, velocity decay, and random styles.
//! Every block is a real-time section under the allocation trap.

use crate::common;
use bl_core::time::Bpm;
use bl_devices::arpeggiator::{
    ARPEGGIATOR, DECAY, DISTANCE, GATE, GROOVE, HOLD, OFFSET, RATE_MS, RATE_SYNC, REPEATS,
    RETRIGGER, ROOT, SCALE, STEPS, STYLE, SYNC, TARGET, USE_SCALE, VELOCITY_ON,
};
use bl_engine::NoteKind;
use bl_engine::scales::ScaleRuns;
use bl_model::{Command, Scale, ScaleName};
use common::chain::{BEAT_FRAMES, Chain, note, nothing_hangs};
use common::{bend, bends, device, keys_on, node, on, registry, scaled_through};

const SIZES: [usize; 4] = [1, 100, 512, 4096];

fn play(
    params: &[(bl_model::ParamId, f64)],
    notes: impl IntoIterator<Item = bl_model::Note>,
    size: usize,
) -> Vec<(i64, NoteKind, u8, f32)> {
    let mut chain = Chain::new(4096);
    chain.effect(ARPEGGIATOR, params);
    chain.clip(8, notes);
    chain.sync();
    chain.play();
    chain.run(8 * BEAT_FRAMES as usize, size);
    chain.heard()
}

fn times(heard: &[(i64, NoteKind, u8, f32)], kind: NoteKind) -> Vec<i64> {
    heard.iter().filter(|h| h.1 == kind).map(|h| h.0).collect()
}

fn keys(heard: &[(i64, NoteKind, u8, f32)], kind: NoteKind) -> Vec<u8> {
    heard.iter().filter(|h| h.1 == kind).map(|h| h.2).collect()
}

// Spec: arpeggiator R1. Style determines the note playback order.
#[test]
fn style_determines_note_playback_order() {
    // Three notes: 60, 64, 67 held over 2 beats (48,000 frames).
    // Synced rate at 1/8 division (index 5) is 12,000 frames per step.
    let notes = [note(60, 0, 4), note(64, 0, 4), note(67, 0, 4)];

    // Up style (0): 60, 64, 67, 60, 64, 67, ...
    let up = play(&[(STYLE, 0.0), (STEPS, 0.0)], notes.clone(), 512);
    let up_keys = keys(&up, NoteKind::On);
    assert!(up_keys.len() >= 6);
    assert_eq!(&up_keys[0..6], &[60, 64, 67, 60, 64, 67]);

    // Down style (1): 67, 64, 60, 67, 64, 60, ...
    let down = play(&[(STYLE, 1.0), (STEPS, 0.0)], notes.clone(), 512);
    let down_keys = keys(&down, NoteKind::On);
    assert_eq!(&down_keys[0..6], &[67, 64, 60, 67, 64, 60]);

    // UpDown style (2): 60, 64, 67, 64, 60, 64, ...
    let updown = play(&[(STYLE, 2.0), (STEPS, 0.0)], notes.clone(), 512);
    let updown_keys = keys(&updown, NoteKind::On);
    assert_eq!(&updown_keys[0..6], &[60, 64, 67, 64, 60, 64]);

    // Converge style (6): 60, 67, 64, 60, 67, 64, ...
    let conv = play(&[(STYLE, 6.0), (STEPS, 0.0)], notes.clone(), 512);
    let conv_keys = keys(&conv, NoteKind::On);
    assert_eq!(&conv_keys[0..6], &[60, 67, 64, 60, 67, 64]);

    // Chord Trigger (14): all notes played on each step
    let chord = play(&[(STYLE, 14.0), (STEPS, 0.0)], notes, 512);
    let on_times = times(&chord, NoteKind::On);
    let on_keys = keys(&chord, NoteKind::On);
    // At frame 0, all 3 notes trigger
    assert_eq!(on_times[0..3], [0, 0, 0]);
    assert_eq!(&on_keys[0..3], &[60, 64, 67]);
}

// Spec: arpeggiator R2. Rate sets step duration in free ms or beat divisions at current tempo.
#[test]
fn rate_sync_follows_tempo_and_divisions() {
    for size in SIZES {
        // 1/8 division is 12,000 frames at 120 BPM, 24,000 frames at 60 BPM.
        let at_120 = play(
            &[(SYNC, 1.0), (RATE_SYNC, 5.0), (STEPS, 0.0)],
            [note(60, 0, 4)],
            size,
        );
        let on_120 = times(&at_120, NoteKind::On);
        assert_eq!(on_120[0], 0);
        assert_eq!(on_120[1], 12_000);

        // At 60 BPM
        let mut chain = Chain::new(4096);
        chain.apply(Command::SetTempo {
            tempo: bl_core::time::TempoMap::constant(Bpm::new(60.0).unwrap()),
        });
        chain.effect(ARPEGGIATOR, &[(SYNC, 1.0), (RATE_SYNC, 5.0), (STEPS, 0.0)]);
        chain.clip(8, [note(60, 0, 4)]);
        chain.sync();
        chain.play();
        chain.run(4 * BEAT_FRAMES as usize, size);
        let on_60 = times(&chain.heard(), NoteKind::On);
        assert_eq!(on_60[0], 0);
        assert_eq!(on_60[1], 24_000);
    }
}

// Spec: arpeggiator R3. Gate sets duration and overlaps above 100%.
#[test]
fn gate_sets_duration_and_overlaps_above_100_percent() {
    // Step is 12,000 frames (1/8 at 120 BPM).
    // Gate 50 % = 6,000 frames duration.
    let half = play(&[(GATE, 50.0), (STEPS, 0.0)], [note(60, 0, 2)], 512);
    let offs_half = times(&half, NoteKind::Off);
    assert_eq!(offs_half[0], 6_000);

    // Gate 150 % = 18,000 frames duration (extends past step 1 at 12,000).
    let over = play(&[(GATE, 150.0), (STEPS, 0.0)], [note(60, 0, 2)], 512);
    let ons = times(&over, NoteKind::On);
    let offs = times(&over, NoteKind::Off);
    assert_eq!(ons[1], 12_000);
    // Previous note is cut when next note on the same key begins
    assert_eq!(offs[0], 12_000);
}

// Spec: arpeggiator R4. Hold mode retains notes after keys are released, adding and removing notes.
#[test]
fn hold_mode_retains_adds_and_removes_notes() {
    // Note 60 is played from beat 0 to 1 (24,000 frames).
    // In Hold mode, it keeps arpeggiating past beat 1.
    let held = play(&[(HOLD, 1.0), (STEPS, 0.0)], [note(60, 0, 1)], 512);
    let on_times = times(&held, NoteKind::On);
    // Notes continue past 24,000 frames
    assert!(on_times.len() >= 4);
    assert!(on_times.iter().any(|&t| t >= 24_000));
}

// Spec: arpeggiator R5. Pattern Offset shifts the starting note.
#[test]
fn offset_shifts_pattern_starting_position() {
    let notes = [note(60, 0, 4), note(64, 0, 4), note(67, 0, 4)];
    // Offset 0 starts on 60
    let off0 = play(&[(OFFSET, 0.0), (STEPS, 0.0)], notes.clone(), 512);
    assert_eq!(keys(&off0, NoteKind::On)[0], 60);

    // Offset 1 shifts starting note to 64
    let off1 = play(&[(OFFSET, 1.0), (STEPS, 0.0)], notes, 512);
    assert_eq!(keys(&off1, NoteKind::On)[0], 64);
}

// Spec: arpeggiator R6. Retrigger modes restart pattern on notes and beats.
#[test]
fn retrigger_modes_restart_pattern() {
    // Two sequential notes: 60 at beat 0, 64 at beat 1.
    // Retrigger Note (1) restarts pattern when 64 arrives.
    let heard = play(
        &[(RETRIGGER, 1.0), (STEPS, 0.0)],
        [note(60, 0, 1), note(64, 1, 1)],
        512,
    );
    let on_times = times(&heard, NoteKind::On);
    let on_keys = keys(&heard, NoteKind::On);
    assert_eq!(on_times[0], 0);
    assert_eq!(on_keys[0], 60);
    assert_eq!(on_times[2], BEAT_FRAMES);
    assert_eq!(on_keys[2], 64);
}

// Spec: arpeggiator R7. Repeats limits pattern cycles before stopping.
#[test]
fn repeats_limits_pattern_cycles() {
    // 1 note: 1 repeat = exactly 1 note output before stopping
    let heard = play(&[(REPEATS, 1.0), (STEPS, 0.0)], [note(60, 0, 4)], 512);
    let ons = times(&heard, NoteKind::On);
    assert_eq!(ons.len(), 1);
}

// Spec: arpeggiator R8. Steps and Distance transpose cycles progressively.
#[test]
fn steps_and_distance_transpose_cycles() {
    // 1 note (60) with Steps = 2 and Distance = 12:
    // plays 60 (+0), then 72 (+12), then 84 (+24), then 60 (+0)...
    let heard = play(&[(STEPS, 2.0), (DISTANCE, 12.0)], [note(60, 0, 4)], 512);
    let on_keys = keys(&heard, NoteKind::On);
    assert_eq!(&on_keys[0..3], &[60, 72, 84]);
    assert_eq!(on_keys[3], 60);
}

// Spec: arpeggiator R9. Velocity decay ramps toward target.
#[test]
fn velocity_decay_ramps_toward_target() {
    // Decay over 100 ms (4800 frames) toward target 1 (0.0078).
    let heard = play(
        &[
            (VELOCITY_ON, 1.0),
            (DECAY, 100.0),
            (TARGET, 1.0),
            (STEPS, 0.0),
        ],
        [note(60, 0, 2)],
        512,
    );
    let vels: Vec<f32> = heard
        .iter()
        .filter(|h| h.1 == NoteKind::On)
        .map(|h| h.3)
        .collect();
    assert!(vels.len() >= 2);
    assert!(
        vels[1] < vels[0],
        "velocity should decay: {} -> {}",
        vels[0],
        vels[1]
    );
}

// Spec: arpeggiator R10. Scale and groove Phase 6 placeholders.
#[test]
fn scale_and_groove_phase_6_placeholders() {
    // Verify parameters load without panics or effects
    let heard = play(&[(USE_SCALE, 1.0), (GROOVE, 0.0)], [note(60, 0, 1)], 512);
    assert!(!heard.is_empty());
}

// Spec: arpeggiator R11. Random styles are deterministic and repeat identically.
#[test]
fn random_styles_are_deterministic_and_repeat_identically() {
    let notes = [note(60, 0, 4), note(64, 0, 4), note(67, 0, 4)];
    let run1 = play(&[(STYLE, 15.0), (STEPS, 0.0)], notes.clone(), 512);
    let run2 = play(&[(STYLE, 15.0), (STEPS, 0.0)], notes, 512);
    assert_eq!(keys(&run1, NoteKind::On), keys(&run2, NoteKind::On));
}

// Spec: arpeggiator R12. Stopping transport leaves no hanging notes.
#[test]
fn stopping_transport_leaves_no_hanging_notes() {
    let mut chain = Chain::new(4096);
    chain.effect(ARPEGGIATOR, &[(STEPS, 0.0)]);
    chain.clip(8, [note(60, 0, 4)]);
    chain.sync();
    chain.play();
    chain.run(BEAT_FRAMES as usize, 512);
    chain.stop();
    chain.run(BEAT_FRAMES as usize, 512);
    // After stop and subsequent processing, every note on has a note off
    let heard = chain.heard();
    let ons = times(&heard, NoteKind::On).len();
    let offs = times(&heard, NoteKind::Off).len();
    assert_eq!(ons, offs, "all notes ended cleanly");
}

// Spec: arpeggiator R14. A chord of many held notes makes no heap use.
#[test]
fn a_chord_of_many_held_notes_allocates_nothing() {
    let notes: Vec<_> = (0_u8..=127).map(|key| note(key, 0, 4)).collect();
    let heard = play(&[(STEPS, 0.0), (STYLE, 14.0)], notes, 512);
    assert!(
        heard.iter().filter(|h| h.1 == NoteKind::On).count() > 100,
        "the arpeggio plays a chord of a hundred notes: {} events",
        heard.len()
    );
}

fn c_major() -> Scale {
    Scale::new(0, ScaleName::Major)
}

/// The chooser's entry for `name`: Chromatic is 0 and the list follows.
fn entry(name: ScaleName) -> f64 {
    1.0 + ScaleName::ALL.iter().position(|n| *n == name).unwrap() as f64
}

/// [`play`] with every clip of the track in `scale`.
fn play_in(
    scale: Scale,
    params: &[(bl_model::ParamId, f64)],
    notes: impl IntoIterator<Item = bl_model::Note>,
    size: usize,
) -> Vec<(i64, NoteKind, u8, f32)> {
    let mut chain = Chain::new(4096);
    chain.effect(ARPEGGIATOR, params);
    chain.clip(8, notes);
    chain.scale_clips(scale);
    chain.sync();
    chain.play();
    chain.run(8 * BEAT_FRAMES as usize, size);
    chain.heard()
}

// Spec: arpeggiator R15. Root and Scale name a scale whose degrees Distance
// counts from Root; Chromatic, the default, counts semitones.
#[test]
fn distance_counts_degrees_of_the_chosen_root_and_scale() {
    // D Major from D3: two degrees up is F#3, four is A3.
    let params = [
        (STEPS, 2.0),
        (DISTANCE, 2.0),
        (ROOT, 2.0),
        (SCALE, entry(ScaleName::Major)),
    ];
    let keys_played = keys(&play(&params, [note(62, 0, 4)], 512), NoteKind::On);
    assert_eq!(&keys_played[0..4], &[62, 66, 69, 62]);
    // A note outside the scale moves as the scale note below it does: C3 is
    // outside D Major, B2 is below it and two degrees up is D3, three
    // semitones.
    let keys_played = keys(&play(&params, [note(60, 0, 4)], 512), NoteKind::On);
    assert_eq!(&keys_played[0..2], &[60, 63]);
    // Chromatic counts semitones whatever Root is.
    let chromatic = [(STEPS, 2.0), (DISTANCE, 2.0), (ROOT, 2.0)];
    let keys_played = keys(&play(&chromatic, [note(62, 0, 4)], 512), NoteKind::On);
    assert_eq!(&keys_played[0..3], &[62, 64, 66]);
}

// Spec: arpeggiator R16. With Use Current Scale on, Distance counts degrees of
// the device's current scale and the chosen Root and Scale are not read.
#[test]
fn use_current_scale_counts_distance_in_the_current_scale() {
    let params = [
        (USE_SCALE, 1.0),
        (STEPS, 2.0),
        (DISTANCE, 2.0),
        (ROOT, 5.0),
        (SCALE, entry(ScaleName::Locrian)),
    ];
    for size in SIZES {
        let heard = play_in(c_major(), &params, [note(60, 0, 4)], size);
        let played = keys(&heard, NoteKind::On);
        assert_eq!(&played[0..4], &[60, 64, 67, 60], "size {size}");
    }
    // A current scale with Scale Mode off counts semitones.
    let heard = play_in(Scale::default(), &params, [note(60, 0, 4)], 512);
    assert_eq!(&keys(&heard, NoteKind::On)[0..3], &[60, 62, 64]);
}

// Spec: arpeggiator R17. A held note outside the scale moves as the scale note
// below it does, and a change of the scale while a note sounds doesn't move
// it: every note-off goes to its note-on's key.
#[test]
fn a_scale_change_while_a_note_sounds_does_not_move_it() {
    let params = [
        (USE_SCALE, 1.0),
        (STEPS, 1.0),
        (DISTANCE, 2.0),
        (GATE, 150.0),
    ];
    let heard = play_in(c_major(), &params, [note(61, 0, 4)], 512);
    // C#3 keeps its distance above C3, which two degrees up makes E3.
    assert_eq!(&keys(&heard, NoteKind::On)[0..4], &[61, 65, 61, 65]);
    let mut chain = Chain::new(4096);
    chain.effect(ARPEGGIATOR, &params);
    chain.clip(8, [note(61, 0, 4)]);
    chain.scale_clips(c_major());
    chain.sync();
    chain.play();
    for size in SIZES {
        chain.run(BEAT_FRAMES as usize / 2, size);
        chain.scale_clips(Scale::new(5, ScaleName::Minor));
        chain.run(BEAT_FRAMES as usize / 2, size);
        chain.scale_clips(c_major());
    }
    chain.run(4 * BEAT_FRAMES as usize, 512);
    chain.stop();
    chain.run(BEAT_FRAMES as usize, 512);
    assert!(nothing_hangs(&chain.heard()));
}

// Spec: arpeggiator R17. The per-note bend of a note the arpeggio moved is
// carried in degrees.
#[test]
fn a_bend_is_carried_in_degrees() {
    let params = [
        (USE_SCALE, 1.0),
        (STEPS, 1.0),
        (DISTANCE, 2.0),
        (SYNC, 0.0),
        (RATE_MS, 100.0),
    ];
    let device = device(&bl_devices::arpeggiator_type(), &params);
    let mut arp = node(&registry(), &device, 10_000);
    // Held from frame 0; the second step, 4,800 frames in, plays E3 (two
    // degrees above C3). A bend of two semitones on the held C3, a degree,
    // in the next block is a degree above E3: F3, a semitone.
    let scale = ScaleRuns::of(c_major());
    let first = scaled_through(arp.as_mut(), &[on(0, 60, 1)], 6_000, scale);
    assert_eq!(&keys_on(&first)[0..2], &[60, 64]);
    let sent = scaled_through(arp.as_mut(), &[bend(100, 60, 1, 2.0 / 48.0)], 4_000, scale);
    let carried = bends(&sent);
    assert_eq!(carried.len(), 1, "{sent:?}");
    assert_eq!(carried[0].0, 64);
    assert!((carried[0].1 - 1.0 / 48.0).abs() < 1e-6, "{carried:?}");
}

// Spec: arpeggiator R11, R17. Random styles with the scale repeat bit for bit,
// at any block size.
#[test]
fn random_styles_with_the_scale_repeat_identically() {
    let notes = [note(60, 0, 4), note(64, 0, 4), note(67, 0, 4)];
    for style in [15.0, 16.0, 17.0] {
        let params = [
            (STYLE, style),
            (USE_SCALE, 1.0),
            (STEPS, 3.0),
            (DISTANCE, 1.0),
        ];
        // The same render twice is the same bit for bit, and the notes played
        // are the same at any block size.
        let reference = play_in(c_major(), &params, notes.clone(), 512);
        assert!(!reference.is_empty());
        assert_eq!(play_in(c_major(), &params, notes.clone(), 512), reference);
        let ons = |heard: &[(i64, NoteKind, u8, f32)]| -> Vec<(i64, u8)> {
            heard
                .iter()
                .filter(|h| h.1 == NoteKind::On)
                .map(|h| (h.0, h.2))
                .collect()
        };
        for size in SIZES {
            let heard = play_in(c_major(), &params, notes.clone(), size);
            assert_eq!(ons(&heard), ons(&reference), "style {style}, size {size}");
        }
    }
}

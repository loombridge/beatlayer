//! WP-1.6's acceptance: the mixer (the mixer spec).
//!
//! - Gain accuracy: a fader at −6.02 dB halves the amplitude to within 0.1%,
//!   and every fader level is the gain it names; a fader at 0 is silence.
//! - The pan law, sends before and after the fader, and the activator.
//! - A solo truth table including groups and returns: each track is a DC
//!   level of its own power of two, and each return scales what it gets
//!   into a band of bits of its own, so Main's output says exactly which
//!   tracks are heard directly and which through each return.
//! - Meters, and that every mixer edit reaches the engine without a new
//!   program.
//!
//! Mixer commands run by ID with JSON arguments, as an agent runs them
//! (D17). Every block runs as a real-time section under the allocation
//! trap.

bl_rt::install_trap!();

mod common;

use bl_commands::Registry as Commands;
use bl_engine::mixer::{METER_RELEASE_DB_PER_SECOND, Solo, db_from_gain, gain_from_db, pan_gains};
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_model::{Command, Editor, TrackId, TrackKind, registry};
use common::{Rig, dc, device, track};
use serde_json::{Value, json};

/// Runs a document command by ID, as an agent does, checking its arguments
/// against the registered schema first.
fn run(editor: &mut Editor, id: &str, args: Value) {
    let mut commands = Commands::new();
    registry::register(&mut commands).unwrap();
    let spec = commands.get(id).unwrap();
    bl_commands::validate(spec.args.as_ref().unwrap(), &args).unwrap();
    registry::run(editor, id, args).unwrap_or_else(|e| panic!("{id}: {e}"));
}

/// Frames to render after a change, so a glide (20 ms at 48 kHz, 960
/// frames, from the first frame of the next block) has finished before the
/// last block of 256 frames.
const SETTLE: usize = 1280;

/// The peak of a run of samples.
fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |p, s| p.max(s.abs()))
}

/// Both channels of the last block.
fn last_stereo(rig: &Rig) -> (Vec<f32>, Vec<f32>) {
    let output = rig.processor.output();
    (output.channel(0).to_vec(), output.channel(1).to_vec())
}

// Spec: mixer R2
#[test]
fn a_fader_at_minus_6_02_db_halves_the_amplitude_to_within_a_tenth_of_a_percent() {
    let mut rig = Rig::new(512);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    // A 1 kHz sine at -6 dBFS: 48 samples a cycle, so a block holds its peaks.
    device(
        &mut rig.editor,
        a,
        TEST_TONE,
        &[(test_tone::FREQUENCY, 1000.0), (test_tone::LEVEL, -6.0)],
    );
    rig.sync();
    let unity = peak(&rig.render(4800, 512)[2400..]);

    println!("fader dB   gain asked   gain measured   error");
    for db in [6.0, 0.0, -3.0, -6.02, -12.0, -24.0, -48.0, -70.0] {
        let gain = gain_from_db(db);
        run(
            &mut rig.editor,
            "mixer.set_volume",
            json!({ "track": a, "volume": gain }),
        );
        assert!(!rig.sync().program);
        let out = rig.render(SETTLE + 4800, 512);
        let measured = f64::from(peak(&out[SETTLE..])) / f64::from(unity);
        let error = (measured - gain).abs() / gain;
        println!(
            "{db:>8}   {gain:>10.6}   {measured:>13.6}   {:.5}%",
            error * 100.0
        );
        assert!(error < 0.001, "{db} dB: {measured} for {gain}");
        if db == -6.02 {
            assert!(
                (measured - 0.5).abs() / 0.5 < 0.001,
                "-6.02 dB halves the amplitude: {measured}"
            );
        }
    }
}

// Spec: mixer R2 (and rule G1: a gain reaches -inf dB)
#[test]
fn a_fader_at_zero_is_silence() {
    let mut rig = Rig::new(256);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, a, 1.0);
    rig.sync();
    run(
        &mut rig.editor,
        "mixer.set_volume",
        json!({ "track": a, "volume": 0.0 }),
    );
    rig.sync();
    let out = rig.render(SETTLE + 2048, 256);
    assert!(out[SETTLE..].iter().all(|&s| s == 0.0));
    assert_eq!(db_from_gain(0.0), f64::NEG_INFINITY);
}

// Spec: mixer R3, R4
#[test]
fn pan_follows_the_constant_power_law_and_the_centre_passes_through() {
    let mut rig = Rig::new(256);
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    dc(&mut rig.editor, a, 0.5);
    rig.sync();
    rig.block(256);
    let (left, right) = last_stereo(&rig);
    assert!(left.iter().chain(&right).all(|&s| s == 0.5), "unchanged");

    for pan in [-1.0, -0.5, 0.25, 1.0] {
        run(
            &mut rig.editor,
            "mixer.set_pan",
            json!({ "track": a, "pan": pan }),
        );
        rig.sync();
        rig.render(SETTLE, 256);
        let (left, right) = last_stereo(&rig);
        let [l, r] = pan_gains(pan);
        assert!(left.iter().all(|&s| s == (0.5 * l) as f32), "{pan}");
        assert!(right.iter().all(|&s| s == (0.5 * r) as f32), "{pan}");
        let power = f64::from(left[0]).powi(2) + f64::from(right[0]).powi(2);
        assert!((power - 0.5).abs() < 1e-6, "constant power at {pan}");
    }
    // Hard left: +3 dB on the left and nothing on the right.
    let [l, r] = pan_gains(-1.0);
    assert!((db_from_gain(l) - 3.0103).abs() < 1e-3);
    assert_eq!(r, 0.0);
}

// Spec: mixer R5, R7, R8
#[test]
fn sends_take_the_signal_before_or_after_the_fader_and_the_activator() {
    let mut rig = Rig::new(256);
    let ed = &mut rig.editor;
    let post = track(ed, TrackKind::Return, None);
    let pre = track(ed, TrackKind::Return, None);
    let a = track(ed, TrackKind::Audio, None);
    dc(ed, a, 0.5);
    run(ed, "mixer.set_volume", json!({ "track": a, "volume": 0.5 }));
    // The returns scale what they get, so each lands in bits of its own.
    run(
        ed,
        "mixer.set_volume",
        json!({ "track": post, "volume": 1.0 / 64.0 }),
    );
    run(
        ed,
        "mixer.set_volume",
        json!({ "track": pre, "volume": 1.0 / 4096.0 }),
    );
    run(
        ed,
        "mixer.set_send_tap",
        json!({ "track": pre, "tap": "pre" }),
    );
    rig.sync();
    rig.render(512, 256);
    // Every send starts at -inf dB.
    assert!(rig.last().iter().all(|&s| s == 0.25));

    run(
        &mut rig.editor,
        "mixer.set_send",
        json!({ "track": a, "to": post, "level": 1.0 }),
    );
    run(
        &mut rig.editor,
        "mixer.set_send",
        json!({ "track": a, "to": pre, "level": 0.5 }),
    );
    assert!(!rig.sync().program, "send levels need no program");
    rig.render(SETTLE, 256);
    // Direct 0.25; post-fader send 0.25 / 64; pre-fader send 0.5 * 0.5 /
    // 4096, before the volume.
    let expected: f32 = 0.25 + 0.25 / 64.0 + 0.25 / 4096.0;
    assert!(
        rig.last().iter().all(|&s| s == expected),
        "{}",
        rig.last()[0]
    );

    // Deactivated: the output and the post-fader send go quiet, the
    // pre-fader send plays on.
    run(
        &mut rig.editor,
        "mixer.set_mute",
        json!({ "track": a, "mute": true }),
    );
    rig.sync();
    rig.render(SETTLE, 256);
    assert!(rig.last().iter().all(|&s| s == 0.25 / 4096.0));

    // The Pre/Post switch moves the tap: a new program, the same nodes.
    run(
        &mut rig.editor,
        "mixer.set_send_tap",
        json!({ "track": pre, "tap": "post" }),
    );
    assert!(rig.sync().program);
    rig.render(SETTLE, 256);
    assert!(rig.last().iter().all(|&s| s == 0.0));
}

/// The truth table's song:
///
/// ```text
/// A (audio, 1/2)            sends 1 to R1
/// G1 (group)
///   B (MIDI, 1/4)           sends 1 to R2
///   G2 (group)
///     C (audio, 1/8)        sends 1 to R1
/// D (MIDI, 1/16)            sends 1 to R2
/// R1 (return, volume 1/64)
/// R2 (return, volume 1/4096)
/// ```
///
/// Each track's level is a power of two, and each return shifts what it
/// gets down by six or twelve bits, so the sum on Main is exact and says
/// who was heard where.
struct Song {
    rig: Rig,
    tracks: [TrackId; 8],
}

const A: usize = 0;
const G1: usize = 1;
const B: usize = 2;
const G2: usize = 3;
const C: usize = 4;
const D: usize = 5;
const R1: usize = 6;
const R2: usize = 7;
const NAMES: [&str; 8] = ["A", "G1", "B", "G2", "C", "D", "R1", "R2"];
/// The DC level of each source track.
const LEVEL: [f32; 8] = [0.5, 0.0, 0.25, 0.0, 0.125, 0.0625, 0.0, 0.0];

impl Song {
    fn new() -> Self {
        let mut rig = Rig::new(256);
        let ed = &mut rig.editor;
        let r2 = track(ed, TrackKind::Return, None);
        let r1 = track(ed, TrackKind::Return, None);
        let d = track(ed, TrackKind::Midi, None);
        let g1 = track(ed, TrackKind::Group, None);
        let a = track(ed, TrackKind::Audio, None);
        let g2 = track(ed, TrackKind::Group, Some(g1));
        let b = track(ed, TrackKind::Midi, Some(g1));
        let c = track(ed, TrackKind::Audio, Some(g2));
        let tracks = [a, g1, b, g2, c, d, r1, r2];
        assert_eq!(rig.editor.document().track_order()[..8], tracks);
        for i in [A, B, C, D] {
            dc(&mut rig.editor, tracks[i], f64::from(LEVEL[i]));
        }
        let ed = &mut rig.editor;
        run(
            ed,
            "mixer.set_volume",
            json!({ "track": r1, "volume": 1.0 / 64.0 }),
        );
        run(
            ed,
            "mixer.set_volume",
            json!({ "track": r2, "volume": 1.0 / 4096.0 }),
        );
        for (from, to) in [(a, r1), (c, r1), (b, r2), (d, r2)] {
            run(
                ed,
                "mixer.set_send",
                json!({ "track": from, "to": to, "level": 1.0 }),
            );
        }
        Self { rig, tracks }
    }

    /// Main's output for the tracks heard directly, those heard through
    /// R1, and those heard through R2.
    fn expected(direct: &[usize], r1: &[usize], r2: &[usize]) -> f32 {
        let sum = |set: &[usize]| set.iter().fold(0.0f32, |sum, &i| sum + LEVEL[i]);
        sum(direct) + sum(r1) / 64.0 + sum(r2) / 4096.0
    }

    /// Main's output after the engine has settled, both channels the same.
    fn heard(&mut self) -> f32 {
        self.rig.render(SETTLE, 256);
        let (left, right) = last_stereo(&self.rig);
        assert!(left.iter().chain(&right).all(|&s| s == left[0]));
        left[0]
    }
}

/// One row of the truth table: the solo switches and Solo in Place, and
/// who is heard directly, through R1, and through R2.
struct Row {
    soloed: &'static [usize],
    deactivated: &'static [usize],
    in_place: bool,
    direct: &'static [usize],
    r1: &'static [usize],
    r2: &'static [usize],
}

const fn row(
    soloed: &'static [usize],
    in_place: bool,
    direct: &'static [usize],
    r1: &'static [usize],
    r2: &'static [usize],
) -> Row {
    Row {
        soloed,
        deactivated: &[],
        in_place,
        direct,
        r1,
        r2,
    }
}

/// Written from the spec's rules, not from the code.
const TABLE: &[Row] = &[
    // Spec: mixer R10. Nothing soloed: everything is heard.
    row(&[], true, &[A, B, C, D], &[A, C], &[B, D]),
    // R11: a track soloed in place keeps the returns, which carry only the
    // sends of what's soloed.
    row(&[A], true, &[A], &[A], &[]),
    // R11: without Solo in Place, the returns go quiet.
    row(&[A], false, &[A], &[], &[]),
    // R10: a track in a group plays through its group; its neighbours in
    // the group don't.
    row(&[B], true, &[B], &[], &[B]),
    // R10: a soloed group plays everything inside it, nested groups too.
    row(&[G1], true, &[B, C], &[C], &[B]),
    // R10: a track two groups deep plays through both.
    row(&[C], true, &[C], &[C], &[]),
    row(&[G2], true, &[C], &[C], &[]),
    // R12: a soloed return plays every send it gets, and nothing else
    // is heard, with or without Solo in Place.
    row(&[R1], true, &[], &[A, C], &[]),
    row(&[R1], false, &[], &[A, C], &[]),
    // R10: several soloed tracks (Cmd-click).
    row(&[A, D], true, &[A, D], &[A], &[D]),
    // R11, R12: a soloed track and a soloed return: the other return is
    // quiet even in place.
    row(&[A, R2], true, &[A], &[], &[B, D]),
    // R13: a soloed track whose activator is off stays silent, and its
    // post-fader send with it.
    Row {
        soloed: &[B],
        deactivated: &[B],
        in_place: true,
        direct: &[],
        r1: &[],
        r2: &[],
    },
    Row {
        soloed: &[G1],
        deactivated: &[B],
        in_place: true,
        direct: &[C],
        r1: &[C],
        r2: &[],
    },
];

fn describe(row: &Row) -> String {
    let names = |set: &[usize]| set.iter().map(|&i| NAMES[i]).collect::<Vec<_>>().join(" ");
    format!(
        "solo [{}]{}{}",
        names(row.soloed),
        if row.in_place { "" } else { ", not in place" },
        if row.deactivated.is_empty() {
            String::new()
        } else {
            format!(", [{}] off", names(row.deactivated))
        }
    )
}

fn set_row(song: &mut Song, row: &Row) {
    let ed = &mut song.rig.editor;
    for (i, &id) in song.tracks.iter().enumerate() {
        let solo = row.soloed.contains(&i);
        run(ed, "mixer.set_solo", json!({ "track": id, "solo": solo }));
        let mute = row.deactivated.contains(&i);
        run(ed, "mixer.set_mute", json!({ "track": id, "mute": mute }));
    }
    run(
        ed,
        "mixer.set_solo_in_place",
        json!({ "enabled": row.in_place }),
    );
}

// Spec: mixer R10, R11, R12, R13
#[test]
fn the_solo_truth_table_with_groups_and_returns() {
    // Each row compiled from scratch, so routes start in their state...
    for row in TABLE {
        let mut song = Song::new();
        set_row(&mut song, row);
        song.rig.sync();
        let expected = Song::expected(row.direct, row.r1, row.r2);
        assert_eq!(song.heard(), expected, "{}", describe(row));
    }

    // ...and every row in turn on one engine, so routes glide between
    // states on parameter changes, with no new program.
    let mut song = Song::new();
    song.rig.sync();
    println!("{:<34} {:>12} {:>12}", "row", "expected", "heard");
    for row in TABLE.iter().chain(TABLE.iter().rev()) {
        set_row(&mut song, row);
        assert!(!song.rig.sync().program, "{}", describe(row));
        let expected = Song::expected(row.direct, row.r1, row.r2);
        let heard = song.heard();
        println!("{:<34} {expected:>12.10} {heard:>12.10}", describe(row));
        assert_eq!(heard, expected, "{}", describe(row));
    }
    assert_eq!(song.rig.monitor.state().swaps, 1);
}

// Spec: mixer R14, R15, R17
#[test]
fn exclusive_solo_releases_the_others_and_groups_show_a_held_solo() {
    let mut song = Song::new();
    song.rig.sync();
    let [a, g1, b, g2, c, d, ..] = song.tracks;
    for track in [a, d] {
        run(
            &mut song.rig.editor,
            "mixer.set_solo",
            json!({ "track": track, "solo": true }),
        );
    }
    run(
        &mut song.rig.editor,
        "mixer.solo_exclusive",
        json!({ "track": c }),
    );
    song.rig.sync();
    assert_eq!(song.heard(), Song::expected(&[C], &[C], &[]));

    let solo = Solo::of(song.rig.editor.document());
    assert!(solo.is_soloed(c) && !solo.is_soloed(a) && !solo.is_soloed(d));
    assert!(solo.holds_solo(g2) && solo.holds_solo(g1), "half lit");
    assert!(!solo.holds_solo(b) && !solo.holds_solo(a));
    // What the UI draws as heard directly.
    let document = song.rig.editor.document();
    assert!(solo.heard_directly(document, c));
    assert!(!solo.heard_directly(document, b));
    // Spec: mixer R14. Main is never silenced by solo.
    assert!(!solo.output_muted(document.main_id()));
}

// Spec: mixer R19
#[test]
fn every_track_has_peak_and_rms_meters_after_its_mixer() {
    let mut rig = Rig::new(480);
    let ed = &mut rig.editor;
    let ret = track(ed, TrackKind::Return, None);
    let a = track(ed, TrackKind::Audio, None);
    device(
        ed,
        a,
        TEST_TONE,
        &[(test_tone::FREQUENCY, 1000.0), (test_tone::LEVEL, -6.0)],
    );
    run(
        ed,
        "mixer.set_send",
        json!({ "track": a, "to": ret, "level": 0.5 }),
    );
    let b = track(ed, TrackKind::Audio, None);
    dc(ed, b, 0.25);
    run(ed, "mixer.set_pan", json!({ "track": b, "pan": -1.0 }));
    let main = ed.document().main_id();
    rig.sync();
    // Three seconds, for the RMS average to settle.
    rig.render(144_000, 480);
    let state = rig.monitor.state();
    let db = |x: f32| db_from_gain(f64::from(x));

    // A sine at -6 dBFS: its peak, and an RMS 3 dB below it.
    let [peak, _] = state.meter(a).unwrap();
    let [rms, _] = state.rms(a).unwrap();
    assert!((db(peak) + 6.0).abs() < 0.01, "{}", db(peak));
    assert!((db(rms) + 9.0103).abs() < 0.01, "{}", db(rms));
    // After the mixer: pan moves B's level to the left and up 3 dB.
    let [left, right] = state.meter(b).unwrap();
    assert_eq!((left, right), ((0.25 * pan_gains(-1.0)[0]) as f32, 0.0));
    // Returns and Main have meters too.
    let [send, _] = state.meter(ret).unwrap();
    assert!((db(send) + 12.02).abs() < 0.01, "{}", db(send));
    assert!(state.meter(main).unwrap()[0] > 0.0);
    assert_eq!(state.meters.len(), 4);

    // A track silenced by another's solo still shows its level, which is
    // measured before solo.
    rig.apply(Command::SetSolo {
        track: b,
        solo: true,
    });
    rig.render(SETTLE, 480);
    let [still, _] = rig.monitor.state().meter(a).unwrap();
    assert!((db(still) + 6.0).abs() < 0.01);

    // A deactivated track's meter falls at the release rate.
    rig.apply(Command::SetMute {
        track: a,
        mute: true,
    });
    rig.render(48_000, 480);
    let [fallen, _] = rig.monitor.state().meter(a).unwrap();
    let fell = db(fallen) + 6.0;
    assert!(
        (fell + METER_RELEASE_DB_PER_SECOND).abs() < 0.05,
        "fell {fell} dB in a second"
    );
}

// Spec: mixer R22
#[test]
fn mixer_edits_need_no_program_and_keep_every_node() {
    let mut song = Song::new();
    song.rig.sync();
    let [a, g1, _, _, c, _, r1, _] = song.tracks;
    let made = song
        .rig
        .probe
        .made
        .load(std::sync::atomic::Ordering::Relaxed);
    for (id, args) in [
        ("mixer.set_volume", json!({ "track": a, "volume": 0.25 })),
        ("mixer.set_pan", json!({ "track": g1, "pan": 0.5 })),
        ("mixer.set_mute", json!({ "track": c, "mute": true })),
        ("mixer.set_solo", json!({ "track": a, "solo": true })),
        ("mixer.solo_exclusive", json!({ "track": r1 })),
        (
            "mixer.set_send",
            json!({ "track": a, "to": r1, "level": 0.5 }),
        ),
        ("mixer.set_solo_in_place", json!({ "enabled": false })),
        ("mixer.set_arm", json!({ "track": a, "arm": true })),
    ] {
        run(&mut song.rig.editor, id, args);
        assert!(!song.rig.sync().program, "{id}");
        song.rig.render(256, 256);
    }
    assert_eq!(song.rig.monitor.state().swaps, 1);
    assert_eq!(
        song.rig
            .probe
            .made
            .load(std::sync::atomic::Ordering::Relaxed),
        made
    );
}

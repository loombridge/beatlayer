//! A mix: the drum loop and a held Cascade bass, the bass compressed from
//! the drums' sidechain so it ducks while they hit, both grouped, a reverb
//! on a return fed by a send, a pad track frozen, and mixer moves that are
//! each one undo step; then undone back to the set it started from, redone,
//! saved and reopened.

use bl_devices::auto_filter::{AUTO_FILTER, FREQ as FILTER_FREQ};
use bl_devices::compressor::{ATTACK, COMPRESSOR, RATIO, RELEASE, THRESHOLD};
use bl_devices::reverb::REVERB;
use bl_e2e::audio::{aligned_residual_db, assert_finite, level_difference_db};
use bl_e2e::fixture::{self, POLYSYNTH};
use bl_e2e::{Audio, BAR, Driver, ExportSpec, Host, Known, SIXTEENTH, full_depth};
use serde_json::{Value, json};

use super::{
    drum_copy, ensure, frame, new_id, redo_to, rms_db, save_and_reopen, tone_db, track_named,
    undo_to,
};

/// The bass note held for two bars: A2, 110 Hz.
const BASS_KEY: u8 = 45;

/// The bass note's frequency in hertz.
const BASS_HZ: f64 = 110.0;

/// The pad's chord: A minor from A3.
const PAD_CHORD: [u8; 3] = [57, 60, 64];

/// How far the frozen track may stray from the unfrozen one: the residual,
/// relative to the unfrozen render.
const FREEZE_TOLERANCE_DB: f64 = -60.0;

/// An export of one track alone, its own devices and mixer, without the
/// returns.
fn solo(track: u64) -> ExportSpec {
    ExportSpec {
        extra: json!({ "rendered_track": { "track": track } }),
        ..ExportSpec::default()
    }
}

/// A MIDI track with Cascade at a quarter of unity and a two-bar clip of
/// `keys` held, made in one batch, named `name`.
fn synth_track(driver: &mut impl Driver, name: &str, keys: &[u8]) -> u64 {
    let notes: Vec<(u8, i64, i64, u8)> = keys
        .iter()
        .map(|&key| (key, 0, 2 * BAR - SIXTEENTH, 100))
        .collect();
    let new = format!("mix {name}");
    let made = driver.batch_ok(
        name,
        vec![
            json!({ "command": "track.create", "args": { "id": { "$new": new },
                    "kind": "midi", "name": name, "index": 0 } }),
            json!({ "command": "device.insert", "args": { "track": { "$ref": new }, "index": 0,
                    "device": fixture::native(&format!("{new} synth"), POLYSYNTH,
                                              "instrument") } }),
            json!({ "command": "mixer.set_volume",
                    "args": { "track": { "$ref": new }, "volume": 0.25 } }),
            json!({ "command": "clip.create", "args": { "track": { "$ref": new },
                    "clip": fixture::midi_clip(&format!("{new} clip"), 0, 2 * BAR,
                                               fixture::midi_data(notes)) } }),
        ],
    );
    new_id(&made, &new)
}

/// Runs `command` through the `run` tool, then checks it was one undo
/// step: one `edit.undo` gives back the document from before it, and one
/// `edit.redo` the one after.
fn one_undo_step(host: &mut Host, command: &str, args: Value) -> Result<(), String> {
    let before = host.document().clone();
    host.run(command, args)
        .map_err(|error| format!("{command}: {error}"))?;
    let after = host.document().clone();
    if after == before {
        return Err(format!("{command} changed nothing"));
    }
    host.run("edit.undo", json!({}))?;
    let undone = host.document() == &before;
    host.run("edit.redo", json!({}))?;
    let redone = host.document() == &after;
    ensure(undone && redone, || {
        format!(
            "{command}: one undo gave back the set before it: {undone}; one redo the set \
             after: {redone}"
        )
    })
}

/// The bass's level while the drums hit: the level of its note in the
/// 60 ms after each of the loop's first four beats, in dBFS, averaged.
fn level_on_hits(audio: &Audio) -> f64 {
    let hits = [0.0, 0.5, 1.0, 1.5];
    hits.iter()
        .map(|&at| tone_db(audio, BASS_HZ, frame(at + 0.005), frame(at + 0.065)))
        .sum::<f64>()
        / hits.len() as f64
}

#[test]
fn a_mix_ducks_groups_sends_freezes_and_undoes_to_the_start() {
    let mut host = Host::new();
    let mut ratchet = Known::load().ratchet("scenarios");
    let start = host.document().clone();

    // The drums on the default set's first audio track, and the bass.
    let drums = track_named(&mut host, "3 Audio");
    host.run_ok(
        "browser.add_to_arrangement",
        json!({ "paths": [drum_copy(&host, "drum-loop.wav"), drum_copy(&host, "drum-loop.wav")],
                "track": drums, "at": 0 }),
    );
    let bass = synth_track(&mut host, "Bass", &[BASS_KEY]);
    host.settle();
    let dry_bass = host.export(0, 2 * BAR, &solo(bass));
    assert_finite(&dry_bass, "the bass");

    // A compressor on the bass, listening to the drums: hard and fast, so
    // the bass ducks under every hit.
    let compressor = host.run_ok(
        "device.add",
        json!({ "track": bass, "device": { "kind": { "native": { "device_type": COMPRESSOR } },
                "name": "Compressor", "role": "audio_effect", "sidechain": true } }),
    )["device"]
        .as_u64()
        .expect("a compressor");
    for (param, value) in [
        (THRESHOLD, -40.0),
        (RATIO, 20.0),
        (ATTACK, 0.1),
        (RELEASE, 100.0),
    ] {
        host.run_ok(
            "device.set_param",
            json!({ "device": compressor, "param": param.0, "value": value }),
        );
    }
    host.run_ok(
        "device.set_sidechain_input",
        json!({ "device": compressor,
                "input": { "type": "track", "track": drums, "tap": "post_fx" } }),
    );
    host.run_ok(
        "device.set_sidechain_enabled",
        json!({ "device": compressor, "enabled": true }),
    );
    host.settle();
    let ducked = host.export(0, 2 * BAR, &solo(bass));
    let (dry, wet) = (level_on_hits(&dry_bass), level_on_hits(&ducked));
    // Spec: compressor R12. With External on, the detector hears the
    // sidechain's key, here the drums alone, so the bass turns down while
    // they hit.
    ratchet.check(
        "mix/sidechain compression",
        "the bass ducks at least 6 dB while the drums hit",
        ensure(dry - wet >= 6.0, || {
            format!("the bass after each hit: {dry:.1} dBFS dry, {wet:.1} dBFS sidechained")
        }),
    );

    // Grouped: the group's fader moves both tracks together.
    let grouped = host.run_ok("track.group", json!({ "tracks": [drums, bass] }));
    let group = grouped["group"]
        .as_u64()
        .or_else(|| grouped["track"].as_u64())
        .unwrap_or_else(|| panic!("no group in {grouped}"));
    host.settle();
    let unity = host.export(0, 2 * BAR, &ExportSpec::default());
    host.run_ok("mixer.set_volume", json!({ "track": group, "volume": 0.5 }));
    host.settle();
    let halved = host.export(0, 2 * BAR, &ExportSpec::default());
    let moved = level_difference_db(&unity, &halved);
    // Spec: mixer R6. A group sums its tracks, then runs its own mixer.
    ratchet.check(
        "mix/group",
        "the group's fader at half lowers the whole mix by 6 dB",
        ensure((moved + 6.02).abs() < 0.5, || {
            format!("the mix moved {moved:+.2} dB")
        }),
    );

    // A reverb on the first return, fed by a send from the bass: the
    // return sounds with the send and is silent without it.
    let verb = track_named(&mut host, "A Return");
    host.run_ok(
        "device.insert",
        json!({ "track": verb, "index": 0,
                "device": fixture::native("mix reverb", REVERB, "audio_effect") }),
    );
    host.settle();
    let silent_return = host.export(0, 2 * BAR, &solo(verb));
    host.run_ok(
        "mixer.set_send",
        json!({ "track": bass, "to": verb, "level": 0.5 }),
    );
    host.settle();
    let fed_return = host.export(0, 2 * BAR, &solo(verb));
    // Spec: mixer R7, R9. A send feeds the return track at its level.
    ratchet.check(
        "mix/return",
        "the return's reverb sounds once the send feeds it, and not before",
        ensure(
            fed_return.rms_dbfs() > -50.0 && silent_return.is_silent(-100.0),
            || {
                format!(
                    "the return at {:.1} dBFS fed and {:.1} dBFS unfed",
                    fed_return.rms_dbfs(),
                    silent_return.rms_dbfs()
                )
            },
        ),
    );

    // A pad, frozen: it sounds as it did through its devices.
    let pad = synth_track(&mut host, "Pad", &PAD_CHORD);
    let filter = host.run_ok(
        "device.add",
        json!({ "track": pad, "device": { "kind": { "native": { "device_type": AUTO_FILTER } },
                "name": "Auto Filter", "role": "audio_effect" } }),
    )["device"]
        .as_u64()
        .expect("an Auto Filter");
    host.run_ok(
        "device.set_param",
        json!({ "device": filter, "param": FILTER_FREQ.0, "value": 1_200.0 }),
    );
    host.settle();
    let live_pad = host.export(0, 3 * BAR, &solo(pad));
    host.run_ok("track.freeze", json!({ "tracks": [pad] }));
    host.wait_until("the pad to freeze", |host| {
        !host.query(json!({ "id": pad }))["frozen"].is_null()
    });
    host.settle();
    let frozen_pad = host.export(0, 3 * BAR, &solo(pad));
    let residual = aligned_residual_db(&live_pad, &frozen_pad, 0);
    // Spec: freeze-and-bounce R2, R6. The freeze renders through the code
    // that plays the track, and the frozen track mixes as it did.
    ratchet.check(
        "mix/freeze",
        "the frozen pad sounds as it did unfrozen, within -60 dB",
        ensure(residual < FREEZE_TOLERANCE_DB, || {
            format!(
                "residual {residual:.1} dB; unfrozen {:.1} dBFS, frozen {:.1} dBFS",
                live_pad.rms_dbfs(),
                frozen_pad.rms_dbfs()
            )
        }),
    );
    // Unfrozen again; the frozen set is saved and reopened at the end, since
    // reopening ends the undo history the mix steps back through.
    host.run_ok("track.unfreeze", json!({ "tracks": [pad] }));
    host.settle();
    let thawed = host.export(0, 3 * BAR, &solo(pad));
    let residual = aligned_residual_db(&live_pad, &thawed, 0);
    ratchet.check(
        "mix/freeze",
        "unfrozen, the pad plays through its devices as before",
        ensure(
            host.query(json!({ "id": pad }))["frozen"].is_null() && residual < FREEZE_TOLERANCE_DB,
            || format!("residual {residual:.1} dB against the pad before freezing"),
        ),
    );

    // Mixer moves: each one undo step.
    let mut moves = vec![
        ("mixer.set_volume", json!({ "track": bass, "volume": 0.4 })),
        ("mixer.set_pan", json!({ "track": bass, "pan": -0.3 })),
        (
            "mixer.set_send",
            json!({ "track": drums, "to": verb, "level": 0.2 }),
        ),
        ("mixer.set_mute", json!({ "track": pad, "mute": true })),
    ];
    if full_depth() {
        moves.extend([
            ("mixer.set_solo", json!({ "track": drums, "solo": true })),
            ("mixer.set_solo", json!({ "track": drums, "solo": false })),
            ("mixer.set_mute", json!({ "track": pad, "mute": false })),
            ("mixer.set_volume", json!({ "track": group, "volume": 0.7 })),
        ]);
    }
    for (command, args) in moves {
        // Spec: mixer R21. Every mixer action is one undo step.
        ratchet.check(
            &format!("mix/mixer moves/{command}"),
            "the move is one undo step",
            one_undo_step(&mut host, command, args),
        );
    }
    let level = rms_db(
        &host.export(0, 2 * BAR, &ExportSpec::default()),
        0,
        frame(3.9),
    );
    ratchet.check(
        "mix/mixer moves",
        "the mix still sounds after the moves",
        ensure(level > -40.0, || format!("the mix at {level:.1} dBFS")),
    );

    // Undone to the start and redone; saved and reopened, the same.
    let finished = host.export(0, 2 * BAR, &ExportSpec::default());
    host.check();
    let end = host.document().clone();
    let steps = undo_to(&mut host, &start);
    ratchet.check(
        "mix/undo to the start",
        "undo steps back to the set the mix started from",
        steps.as_ref().map(drop).map_err(Clone::clone),
    );
    if let Ok(steps) = steps {
        ratchet.check(
            "mix/undo to the start",
            "redo brings every edit back",
            redo_to(&mut host, steps, &end),
        );
    }
    if host.document() == &end {
        ratchet.check(
            "mix/save and reopen",
            "the reopened set is the one saved",
            save_and_reopen(&mut host),
        );
        let reopened = host.export(0, 2 * BAR, &ExportSpec::default());
        let residual = aligned_residual_db(&finished, &reopened, 0);
        ratchet.check(
            "mix/save and reopen",
            "the reopened set sounds as it did",
            ensure(residual < -60.0, || format!("residual {residual:.1} dB")),
        );
    }
    // Spec: freeze-and-bounce R6, R10. A frozen track is part of the set,
    // which saves and opens like any other.
    host.run_ok("track.freeze", json!({ "tracks": [pad] }));
    host.settle();
    ratchet.check(
        "mix/freeze",
        "the set with a frozen MIDI track keeps the model's rules",
        host.document().check_invariants(),
    );
    ratchet.check(
        "mix/freeze",
        "the set with a frozen MIDI track saves and reopens",
        save_and_reopen(&mut host),
    );
    ratchet.finish();
}

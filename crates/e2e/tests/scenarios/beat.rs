//! A beat: a drum rack built from the drum fixtures, a bar of kick, snare
//! and hats played a little loose, quantised, swung with a groove from the
//! pool, its hats' velocities humanised, a kick pattern written by the
//! Euclidean generator, then undone back to the set it started from, redone,
//! saved and reopened.

use bl_e2e::audio::{aligned_residual_db, assert_finite};
use bl_e2e::fixture::{self, SAMPLER};
use bl_e2e::{Audio, BAR, BEAT, Driver, ExportSpec, Host, Known, SIXTEENTH, full_depth};
use serde_json::json;

use super::{drum_copy, ensure, frame, new_id, onset, redo_to, save_and_reopen, undo_to};

/// The pads, from C1 up, and the fixture each plays.
const PADS: [(u8, &str); 4] = [
    (36, "kick.wav"),
    (37, "snare.wav"),
    (38, "hat-closed.wav"),
    (39, "hat-open.wav"),
];

/// How far off the grid the loose bar's notes are played, in ticks: a
/// 64th note, about 31 ms at 120 BPM.
const LOOSE: i64 = SIXTEENTH / 4;

/// The swing the groove gives the off-beat eighths: half a sixteenth late,
/// 62.5 ms at 120 BPM.
const SWING: i64 = SIXTEENTH / 2;

/// How long a tick lasts at the default set's 120 BPM, in seconds.
const SECONDS_PER_TICK: f64 = 0.5 / BEAT as f64;

/// The bar: kick on 1 and 3, snare on 2 and 4, a closed hat on every
/// eighth; the off-beats `LOOSE` late, the beats after the first `LOOSE`
/// early.
fn loose_bar() -> Vec<(u8, i64, i64, u8)> {
    let mut notes = Vec::new();
    for eighth in 0..8i64 {
        let drift = match eighth {
            0 => 0,
            odd if odd % 2 == 1 => LOOSE,
            _ => -LOOSE,
        };
        let at = eighth * BEAT / 2 + drift;
        notes.push((38, at, SIXTEENTH, 100));
        if eighth % 2 == 0 {
            let drum = if eighth % 4 == 0 { 36 } else { 37 };
            notes.push((drum, at, SIXTEENTH, 110));
        }
    }
    notes
}

/// The frame a tick lands on at 120 BPM.
fn at_tick(ticks: i64) -> usize {
    frame(ticks as f64 * SECONDS_PER_TICK)
}

/// Whether every off-beat eighth's hat starts `shift` ticks after its grid
/// line, rising at least 6 dB there, and, when `shift` moves it away, not
/// on the line itself.
fn off_beats_at(audio: &Audio, shift: i64) -> Result<(), String> {
    let found: Vec<((f64, isize), (f64, isize))> = (0..4i64)
        .map(|beat| {
            let line = beat * BEAT + BEAT / 2;
            (
                onset(audio, at_tick(line + shift), 96),
                onset(audio, at_tick(line), 96),
            )
        })
        .collect();
    let moved = shift != 0;
    ensure(
        found
            .iter()
            .all(|&((rise, _), (straight, _))| rise >= 6.0 && (!moved || straight < 3.0)),
        || {
            format!(
                "each off-beat's rise in dB and offset in frames, where it should start and on \
                 the straight line: {found:.1?}"
            )
        },
    )
}

/// The notes of `clip`, as (key, start, velocity), in order.
fn notes(driver: &mut impl Driver, clip: u64) -> Vec<(u64, i64, u64)> {
    let read = driver.query(json!({ "id": clip, "detail": "full", "limit": 500 }));
    read["content"]["notes"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("no notes in {read}"))
        .iter()
        .map(|note| {
            (
                note["key"].as_u64().unwrap_or_default(),
                note["start"].as_i64().unwrap_or_default(),
                note["velocity"].as_u64().unwrap_or_default(),
            )
        })
        .collect()
}

/// The pads of the drum racks on `track`: each chain's received note, its
/// name and its first device's type.
fn pads(driver: &mut impl Driver, track: u64) -> Vec<(u64, String, String)> {
    let read = driver.query(json!({ "id": track }));
    read["devices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|device| device["kind"]["rack"]["kind"] == json!("drum"))
        .flat_map(|rack| rack["chains"].as_array().cloned().unwrap_or_default())
        .map(|chain| {
            (
                chain["receive"]["note"].as_u64().unwrap_or_default(),
                chain["name"].as_str().unwrap_or_default().to_owned(),
                chain["devices"][0]["kind"]["native"]["device_type"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )
        })
        .collect()
}

#[test]
fn a_beat_is_built_quantised_swung_humanised_and_undone_to_the_start() {
    let mut host = Host::new();
    let mut ratchet = Known::load().ratchet("scenarios");
    let start = host.document().clone();

    // A MIDI track with an empty drum rack, and the four drum sounds
    // dropped on it from C1 up, a pad each.
    let made = host.batch_ok(
        "Beat track",
        vec![
            json!({ "command": "track.create", "args": { "id": { "$new": "beat track" },
                "kind": "midi", "name": "Beat", "index": 0 } }),
        ],
    );
    let track = new_id(&made, "beat track");
    let rack = host.run_ok(
        "device.add",
        json!({ "track": track, "device": { "kind": { "rack": { "kind": "drum" } },
                "name": "Drum Rack", "role": "instrument" } }),
    )["device"]
        .as_u64()
        .expect("a drum rack");
    let samples: Vec<String> = PADS
        .iter()
        .map(|(_, name)| drum_copy(&host, name))
        .collect();
    host.run_ok(
        "pad.drop",
        json!({ "rack": rack, "note": PADS[0].0, "samples": samples }),
    );
    let built = pads(&mut host, track);
    // Spec: drum-racks R14, R15. Several files dropped go one to a pad,
    // rising a note each from the pad they were dropped on.
    ratchet.check(
        "beat/drum rack",
        "each sound lands on its own pad from C1 up, in a sampler named after its file",
        ensure(
            built.len() == PADS.len()
                && built
                    .iter()
                    .zip(PADS)
                    .all(|((note, name, kind), (pad, file))| {
                        *note == u64::from(pad)
                            && file.starts_with(name.as_str())
                            && kind == SAMPLER
                    }),
            || format!("the rack's pads: {built:?}"),
        ),
    );

    // The bar, played loose.
    let clip = new_id(
        &host.batch_ok(
            "Beat clip",
            vec![json!({ "command": "clip.create", "args": { "track": track,
                "clip": fixture::midi_clip("beat clip", 0, BAR,
                                           fixture::midi_data(loose_bar())) } })],
        ),
        "beat clip",
    );
    host.settle();
    let solo = ExportSpec {
        extra: json!({ "rendered_track": { "track": track } }),
        ..ExportSpec::default()
    };
    let loose = host.export(0, BAR, &solo);
    assert_finite(&loose, "the loose bar");
    ratchet.check(
        "beat/play the pads",
        "the pads sound, the off-beat hats a 64th late",
        ensure(loose.peak_dbfs() > -30.0, || {
            format!("peak {:.1} dBFS", loose.peak_dbfs())
        })
        .and_then(|()| off_beats_at(&loose, LOOSE)),
    );

    // Quantised to sixteenths: every note on the grid, and every off-beat
    // hat sounds on its line.
    let count = notes(&mut host, clip).len();
    host.run_ok(
        "note.quantize",
        json!({ "clip": clip, "notes": (0..count).collect::<Vec<_>>(), "grid": SIXTEENTH,
                "amount": 100, "target": "note_start" }),
    );
    let quantised = notes(&mut host, clip);
    host.settle();
    let straight = host.export(0, BAR, &solo);
    ratchet.check(
        "beat/quantise",
        "every note starts on a sixteenth and the hats sound on their lines",
        ensure(
            quantised.len() == count && quantised.iter().all(|n| n.1 % SIXTEENTH == 0),
            || format!("the notes after quantising: {quantised:?}"),
        )
        .and_then(|()| off_beats_at(&straight, 0)),
    );

    // A swing groove from the pool: the off-beat eighths half a sixteenth
    // late, at full timing. The notes stay where they are.
    let groove = new_id(
        &host.batch_ok(
            "Swing",
            vec![json!({ "command": "groove.add", "args": { "groove": {
                "id": { "$new": "swing" }, "name": "Swing 8", "length": BEAT, "base": "1/8",
                "notes": [ { "position": 0, "velocity": 100 },
                           { "position": BEAT / 2 + SWING, "velocity": 100 } ] } } })],
        ),
        "swing",
    );
    host.run_ok(
        "clip.set_groove",
        json!({ "clips": [clip], "groove": groove }),
    );
    // Spec: grooves R38. The default set's Global Amount is 0 %, so a
    // groove changes nothing until it is raised.
    host.run_ok("groove.set_global_amount", json!({ "amount": 100.0 }));
    host.settle();
    let swung = host.export(0, BAR, &solo);
    // Spec: grooves R7. A note on a grid line moves by Timing times the
    // line's offset. R14: the clip's notes stay as they are.
    ratchet.check(
        "beat/groove",
        "the off-beat hats play half a sixteenth late and the notes stay on the grid",
        ensure(notes(&mut host, clip) == quantised, || {
            "applying the groove moved the clip's notes".to_owned()
        })
        .and_then(|()| off_beats_at(&swung, SWING)),
    );

    // Humanised: the hats' velocities moved at random by up to 20, the same
    // way for the same seed.
    let hats: Vec<usize> = quantised
        .iter()
        .enumerate()
        .filter(|(_, n)| n.0 == 38)
        .map(|(index, _)| index)
        .collect();
    let humanise = json!({ "clip": clip, "notes": hats, "field": "velocity", "amount": 20.0,
                           "seed": 7 });
    host.run_ok("note.randomize", humanise.clone());
    let first = notes(&mut host, clip);
    host.run_ok("edit.undo", json!({}));
    host.run_ok("note.randomize", humanise);
    let again = notes(&mut host, clip);
    let velocities: Vec<u64> = first.iter().filter(|n| n.0 == 38).map(|n| n.2).collect();
    ratchet.check(
        "beat/humanise",
        "the hats' velocities move within 20 of 100, the same for the same seed",
        ensure(
            velocities.iter().all(|v| (80..=120).contains(v))
                && velocities.iter().any(|&v| v != 100)
                && first == again
                && first
                    .iter()
                    .filter(|n| n.0 != 38)
                    .eq(quantised.iter().filter(|n| n.0 != 38)),
            || format!("the hats' velocities: {velocities:?}; the same seed again: {again:?}"),
        ),
    );

    // A second bar, written by the Euclidean generator: pulses spread over
    // sixteen sixteenths.
    let kicks = new_id(
        &host.batch_ok(
            "Kick bar",
            vec![json!({ "command": "clip.create", "args": { "track": track,
                "clip": fixture::midi_clip("kick bar", BAR, BAR, fixture::midi_data([])) } })],
        ),
        "kick bar",
    );
    let pulses: &[usize] = if full_depth() { &[3, 7, 5] } else { &[5] };
    let mut written = Vec::new();
    for (pass, &density) in pulses.iter().enumerate() {
        // A generator replaces only the notes it overlaps, so each density
        // but the last is undone before the next.
        if pass > 0 {
            host.run_ok("edit.undo", json!({}));
        }
        host.run_ok(
            "midi_tool.apply",
            json!({ "clip": kicks, "tool": "euclidean", "seed": 1,
                    "range": { "start": 0, "end": BAR },
                    "values": { "steps": 16, "density": density, "division": "1/16",
                                "pitch_1": 36 } }),
        );
        written = notes(&mut host, kicks);
        ratchet.check(
            &format!("beat/euclidean {density} of 16"),
            "the generator writes that many kicks, each on a sixteenth",
            ensure(
                written.len() == density
                    && written
                        .iter()
                        .all(|n| n.0 == 36 && n.1 % SIXTEENTH == 0 && n.1 < BAR),
                || format!("the notes written: {written:?}"),
            ),
        );
    }
    host.settle();
    let second = host.export(BAR, BAR, &solo);
    let heard: Vec<(f64, isize)> = written
        .iter()
        .skip(1)
        .map(|n| onset(&second, at_tick(n.1), 96))
        .collect();
    ratchet.check(
        "beat/euclidean 5 of 16",
        "each kick sounds where it was written",
        ensure(heard.iter().all(|&(rise, _)| rise >= 6.0), || {
            format!("each kick after the first, its rise and offset: {heard:.1?}")
        }),
    );

    // Undone to the start and redone; saved and reopened, the same.
    let finished = host.export(0, 2 * BAR, &solo);
    let end = host.document().clone();
    host.check();
    let steps = undo_to(&mut host, &start);
    ratchet.check(
        "beat/undo to the start",
        "undo steps back to the set the beat started from",
        steps.as_ref().map(drop).map_err(Clone::clone),
    );
    if let Ok(steps) = steps {
        ratchet.check(
            "beat/undo to the start",
            "redo brings every edit back",
            redo_to(&mut host, steps, &end),
        );
    }
    if host.document() == &end {
        ratchet.check(
            "beat/save and reopen",
            "the reopened set is the one saved",
            save_and_reopen(&mut host),
        );
        let reopened = host.export(0, 2 * BAR, &solo);
        let residual = aligned_residual_db(&finished, &reopened, 0);
        ratchet.check(
            "beat/save and reopen",
            "the reopened set sounds as it did",
            ensure(residual < -60.0, || format!("residual {residual:.1} dB")),
        );
    }
    ratchet.finish();
}

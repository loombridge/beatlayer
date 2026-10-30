//! A remix: the drum loop brought in at its own tempo, warped to a slower
//! song, played in each warp mode, sliced to MIDI into a drum rack, a
//! Cascade chord progression under it through Auto Filter with its cutoff
//! automated, a reverb on a return fed by a send, the two bars arranged
//! into sections, exported, saved, reopened, and undone back to the set it
//! started from.

use std::path::Path;

use bl_devices::auto_filter::{AUTO_FILTER, FREQ as FILTER_FREQ};
use bl_devices::reverb::REVERB;
use bl_e2e::audio::{aligned_residual_db, assert_finite};
use bl_e2e::fixture::{self, POLYSYNTH};
use bl_e2e::{Audio, BAR, Driver, ExportSpec, Host, Known, full_depth};
use serde_json::{Value, json};

use super::{
    correlation, drum_copy, ensure, envelope, frame, high_band_db, new_id, onset, redo_to, rms_db,
    save_and_reopen, tempo, tone_db, track_named, undo_to,
};

/// Where the drum loop's hits are, in seconds into its file
/// (`tests/fixtures/drums/drum-loop.hits`): one bar at 120 BPM.
const HITS: [f64; 9] = [0.0, 0.25, 0.5, 0.75, 1.0, 1.25, 1.375, 1.5, 1.75];

/// The loop's own tempo.
const LOOP_BPM: f64 = 120.0;

/// The song's tempo the remix moves to.
const SONG_BPM: f64 = 90.0;

/// How long a bar of the song lasts, in seconds.
const SONG_BAR: f64 = 4.0 * 60.0 / SONG_BPM;

/// Every warp mode, as `clip.set_warp_mode` names them, and whether it
/// keeps a hit's attack where it was (Beats and Re-Pitch) or smears it
/// over its grains (the others).
const WARP_MODES: [(&str, bool); 6] = [
    ("beats", true),
    ("tones", false),
    ("texture", false),
    ("re_pitch", true),
    ("complex", false),
    ("complex_pro", false),
];

/// The chords, half a bar each: C, A minor, F and G, root first.
const PROGRESSION: [[u8; 3]; 4] = [[60, 64, 67], [57, 60, 64], [53, 57, 60], [55, 59, 62]];

/// A MIDI key's frequency in hertz.
fn hz(key: u8) -> f64 {
    440.0 * 2f64.powf((f64::from(key) - 69.0) / 12.0)
}

/// An export of one track alone, its own devices and mixer but no returns.
fn solo(track: u64) -> ExportSpec {
    ExportSpec {
        extra: json!({ "rendered_track": { "track": track } }),
        ..ExportSpec::default()
    }
}

/// The drum loop's rhythm stretched by `stretch`: its file's level every
/// 10 ms of the stretched time, for comparing with a render's [`envelope`].
fn stretched_rhythm(stretch: f64, frames: usize) -> Vec<f64> {
    let file = Audio::decode(Path::new(&fixture::drums("drum-loop.wav")))
        .unwrap_or_else(|error| panic!("the drum loop: {error}"));
    let mono = file.mono();
    (0..frames / 480)
        .map(|window| {
            let from = ((window * 480) as f64 / stretch) as usize;
            let to = (((window + 1) * 480) as f64 / stretch) as usize;
            let part = &mono[from.min(mono.len())..to.min(mono.len())];
            if part.is_empty() {
                return 0.0;
            }
            (part.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / part.len() as f64).sqrt()
        })
        .collect()
}

/// Whether `audio` keeps the loop's rhythm, stretched by `stretch`: its
/// level every 10 ms follows the file's, stretched, closely.
fn keeps_rhythm(audio: &Audio, stretch: f64, least: f64) -> Result<(), String> {
    let r = correlation(&envelope(audio), &stretched_rhythm(stretch, audio.frames()));
    ensure(r >= least, || {
        format!("its level follows the stretched loop's with correlation {r:.3}, under {least}")
    })
}

/// Whether every hit of the loop starts within 2 ms of where the stretched
/// grid puts it, rising at least 6 dB there.
fn hits_on_time(audio: &Audio, stretch: f64) -> Result<(), String> {
    let found: Vec<(f64, isize)> = HITS
        .iter()
        .skip(1)
        .map(|&t| onset(audio, frame(t * stretch), 96))
        .collect();
    ensure(found.iter().all(|&(rise, _)| rise >= 6.0), || {
        format!("each hit after the first, its rise in dB and its offset in frames: {found:.1?}")
    })
}

#[test]
fn a_remix_warps_slices_arranges_exports_reopens_and_undoes_to_the_start() {
    let mut host = Host::new();
    let mut ratchet = Known::load().ratchet("scenarios");
    let start = host.document().clone();

    // The loop at its own tempo: the song starts at 120 BPM, as the loop is.
    let placed = host.run_ok(
        "browser.add_to_arrangement",
        json!({ "paths": [drum_copy(&host, "drum-loop.wav")], "at": 0 }),
    );
    let loop_clip = placed["clips"][0]["clip"].as_u64().expect("a clip");
    let loop_track = placed["clips"][0]["track"].as_u64().expect("a track");
    ratchet.check(
        "remix/import",
        "the loop lasts one bar at its own tempo",
        ensure(placed["clips"][0]["length"] == json!(BAR), || {
            format!("placed {placed}")
        }),
    );
    host.settle();

    // Warp on, then the song slows to 90 BPM: the warped loop keeps its bar.
    host.run_ok("clip.set_warp", json!({ "clip": loop_clip, "warp": true }));
    host.run_ok("song.set_tempo", json!({ "tempo": tempo(SONG_BPM) }));
    host.settle();
    let read = host.query(json!({ "id": loop_clip, "detail": "full" }));
    // Spec: warping R2. Switching Warp on gives the clip a marker at its
    // start and the tempo it played at.
    ratchet.check(
        "remix/warp to a new tempo",
        "the warped loop keeps its own tempo and lasts one bar of the new one",
        ensure(
            read["length"]["ticks"] == json!(BAR)
                && read["saved"]["data"]["audio"]["tempo"].as_f64() == Some(LOOP_BPM),
            || format!("{read}"),
        ),
    );
    let stretch = LOOP_BPM / SONG_BPM;

    // Each warp mode: a bar is a bar at 90 BPM, and the loop's rhythm is
    // stretched to fill it.
    let modes: Vec<(&str, bool)> = if full_depth() {
        WARP_MODES.to_vec()
    } else {
        vec![WARP_MODES[0], WARP_MODES[3], WARP_MODES[5]]
    };
    let mut warped_loop = None;
    for (mode, exact) in modes {
        host.run_ok(
            "clip.set_warp_mode",
            json!({ "clip": loop_clip, "mode": mode }),
        );
        host.settle();
        let bar = host.export(0, BAR, &solo(loop_track));
        assert_finite(&bar, mode);
        let key = format!("remix/warp mode {mode}");
        ratchet.check(
            &key,
            "the loop stretched to a bar of the new tempo keeps its rhythm",
            ensure(bar.frames() == frame(SONG_BAR), || {
                format!("{} frames", bar.frames())
            })
            .and_then(|()| keeps_rhythm(&bar, stretch, if exact { 0.9 } else { 0.75 })),
        );
        if exact {
            ratchet.check(
                &key,
                "each hit starts on the stretched grid",
                hits_on_time(&bar, stretch),
            );
        }
        if mode == "beats" {
            warped_loop = Some(bar);
        }
    }
    host.run_ok(
        "clip.set_warp_mode",
        json!({ "clip": loop_clip, "mode": "beats" }),
    );
    let warped_loop = warped_loop.expect("Beats mode played");

    // Slice to a new MIDI track at the transients: a drum rack, a pad per
    // slice, playing the loop back as the warped clip plays it.
    if full_depth() {
        for (by, slices) in [("quarter", 4), ("eighth", 8), ("sixteenth", 16)] {
            let sliced = host.run_ok(
                "clip.slice_to_midi_track",
                json!({ "clip": loop_clip, "by": by }),
            );
            // Spec: slice-to-midi R4. A slice at every division from the
            // region's start, counted in the clip's beats.
            ratchet.check(
                &format!("remix/slice by {by}"),
                "one slice per division of the bar",
                ensure(sliced["slices"] == json!(slices), || format!("{sliced}")),
            );
            host.run_ok("edit.undo", json!({}));
        }
    }
    let transients = host.run_ok("clip.transients", json!({ "clip": loop_clip }));
    let found = transients["transients"].as_array().map_or(0, Vec::len);
    let sliced = host.run_ok(
        "clip.slice_to_midi_track",
        json!({ "clip": loop_clip, "by": "transients" }),
    );
    let slice_track = sliced["track"].as_u64().expect("a sliced track");
    let pads = drum_pads(&mut host, slice_track);
    // Spec: slice-to-midi R4, R7. The region's start is a slice, then one
    // at each transient inside the region, each on a pad of its own.
    ratchet.check(
        "remix/slice to a drum rack",
        "the rack has a pad for each slice: one at the start and one per hit",
        ensure(
            found == HITS.len() && sliced["slices"] == json!(found + 1) && pads == found + 1,
            || format!("{found} transients, {sliced}, {pads} pads"),
        ),
    );
    host.run_ok(
        "mixer.set_mute",
        json!({ "track": loop_track, "mute": true }),
    );
    host.settle();
    let replayed = host.export(0, BAR, &solo(slice_track));
    // Spec: slice-to-midi R6. With Preserve warped timing on, the slices are
    // cut from the clip's warped audio, so the rack plays what the clip did.
    let r = correlation(&envelope(&replayed), &envelope(&warped_loop));
    ratchet.check(
        "remix/slice to a drum rack",
        "the sliced rack plays the loop as the warped clip did",
        ensure(r > 0.9, || {
            format!(
                "the rack's level follows the warped clip's with correlation {r:.3}; the rack \
                 against the loop unstretched: {:.3}",
                correlation(
                    &envelope(&replayed),
                    &stretched_rhythm(1.0, replayed.frames())
                )
            )
        }),
    );

    // Cascade under it: a chord progression, each chord heard in its half
    // bar.
    let keys = cascade_progression(&mut host);
    host.settle();
    let chords = host.export(0, 2 * BAR, &solo(keys));
    assert_finite(&chords, "the chords");
    let half = chords.frames() / 4;
    let mut heard = Vec::new();
    for (index, chord) in PROGRESSION.iter().enumerate() {
        let root = hz(chord[0]);
        let own = tone_db(&chords, root, index * half + half / 8, (index + 1) * half);
        let elsewhere = (0..4)
            .filter(|&other| !PROGRESSION[other].contains(&chord[0]))
            .map(|other| tone_db(&chords, root, other * half + half / 8, (other + 1) * half))
            .fold(f64::NEG_INFINITY, f64::max);
        heard.push((own, elsewhere));
    }
    ratchet.check(
        "remix/chord progression",
        "each chord's root sounds in its own half bar, far louder than where it isn't played",
        ensure(
            heard.iter().all(|(own, elsewhere)| own - elsewhere > 15.0),
            || format!("each root's level in its half bar and elsewhere: {heard:.1?}"),
        ),
    );

    // Auto Filter after Cascade. Open at 8 kHz, then its cutoff automated
    // from 300 Hz to 8 kHz over the two bars: the chords start dark and end
    // as bright as with the filter open.
    let filter = host.run_ok(
        "device.add",
        json!({ "track": keys, "device": { "kind": { "native": { "device_type": AUTO_FILTER } },
                "name": "Auto Filter", "role": "audio_effect" } }),
    )["device"]
        .as_u64()
        .expect("an Auto Filter");
    host.run_ok(
        "device.set_param",
        json!({ "device": filter, "param": FILTER_FREQ.0, "value": 8_000.0 }),
    );
    host.settle();
    let open = host.export(0, 2 * BAR, &solo(keys));
    for (at, value) in [(0, 300.0), (2 * BAR, 8_000.0)] {
        host.run_ok(
            "automation.set_breakpoint",
            json!({ "track": keys, "at": at, "value": value,
                    "target": { "type": "device", "device": filter, "param": FILTER_FREQ.0 } }),
        );
    }
    host.settle();
    let swept = host.export(0, 2 * BAR, &solo(keys));
    let first = (frame(0.02), frame(0.12));
    let last = (3 * half + half / 4, 3 * half + 3 * half / 4);
    let darker = high_band_db(&open, first.0, first.1) - high_band_db(&swept, first.0, first.1);
    let end = high_band_db(&open, last.0, last.1) - high_band_db(&swept, last.0, last.1);
    // Spec: automation R2, R3. The envelope drives its parameter, along a
    // straight line between its breakpoints.
    ratchet.check(
        "remix/filter automation",
        "the cutoff automation starts the chords dark and opens to the filter's open sound",
        ensure(darker > 12.0 && end.abs() < 3.0, || {
            format!(
                "above 1.5 kHz the automated chords are {darker:.1} dB under the open filter at \
                 the start and {end:+.1} dB at the end"
            )
        }),
    );

    // A reverb on the first return, fed by a send from the chords: after
    // the chords end, the reverb's tail sounds, and without the send it
    // doesn't.
    let verb_return = track_named(&mut host, "A Return");
    host.run_ok(
        "device.insert",
        json!({ "track": verb_return, "index": 0,
                "device": fixture::native("remix reverb", REVERB, "audio_effect") }),
    );
    host.run_ok(
        "mixer.set_send",
        json!({ "track": keys, "to": verb_return, "level": 0.5 }),
    );
    host.settle();
    let after = (frame(2.0 * SONG_BAR + 0.15), frame(2.0 * SONG_BAR + 0.6));
    let with_send = host.export(0, 3 * BAR, &ExportSpec::default());
    host.run_ok("edit.undo", json!({}));
    host.settle();
    let dry = host.export(0, 3 * BAR, &ExportSpec::default());
    host.run_ok("edit.redo", json!({}));
    let (wet_tail, dry_tail) = (
        rms_db(&with_send, after.0, after.1),
        rms_db(&dry, after.0, after.1),
    );
    ratchet.check(
        "remix/reverb on a return",
        "the send feeds the return's reverb, whose tail outlasts the chords",
        ensure(wet_tail > -70.0 && wet_tail - dry_tail > 20.0, || {
            format!("after the chords: {wet_tail:.1} dBFS with the send, {dry_tail:.1} without")
        }),
    );

    // Sections: the two bars duplicated after themselves, and a locator at
    // the start of each.
    host.run_ok(
        "arrangement.duplicate_time",
        json!({ "start": 0, "end": 2 * BAR }),
    );
    for at in [0, 2 * BAR] {
        host.run_ok("locator.add", json!({ "at": at }));
    }
    let song = host.query(json!({ "path": "song", "detail": "full" }));
    let keys_read = host.query(json!({ "id": keys }));
    ratchet.check(
        "remix/arrange sections",
        "the chords repeat in the duplicated section and a locator marks each section",
        arranged(&song, &keys_read),
    );

    // The export: four bars of the whole remix, at a plausible loudness.
    host.settle();
    let mix = host.export(0, 4 * BAR, &ExportSpec::default());
    assert_finite(&mix, "the remix");
    let lufs = mix.integrated_lufs().unwrap_or(f64::NEG_INFINITY);
    ratchet.check(
        "remix/export",
        "the mix's loudness lands between -36 and -3 LUFS",
        ensure((-36.0..=-3.0).contains(&lufs), || {
            format!("{lufs:.1} LUFS, peak {:.1} dBFS", mix.peak_dbfs())
        }),
    );
    let second = rms_db(&mix, mix.frames() / 2, mix.frames());
    ratchet.check(
        "remix/export",
        "the duplicated section sounds",
        ensure(second > -40.0, || {
            format!("bars 3 and 4 at {second:.1} dBFS")
        }),
    );

    // Undo all of it, step by step, back to the set it started from, then
    // redo it.
    let end = host.document().clone();
    host.check();
    let steps = undo_to(&mut host, &start);
    ratchet.check(
        "remix/undo to the start",
        "undo steps back to the set the remix started from",
        steps.as_ref().map(drop).map_err(Clone::clone),
    );
    if let Ok(steps) = steps {
        ratchet.check(
            "remix/undo to the start",
            "redo brings every edit back",
            redo_to(&mut host, steps, &end),
        );
    }

    // Saved and reopened, the set is the same and sounds the same.
    if host.document() == &end {
        ratchet.check(
            "remix/save and reopen",
            "the reopened set is the one saved",
            save_and_reopen(&mut host),
        );
        let reopened = host.export(0, 4 * BAR, &ExportSpec::default());
        let residual = aligned_residual_db(&mix, &reopened, 0);
        ratchet.check(
            "remix/save and reopen",
            "the reopened set sounds as it did",
            ensure(residual < -60.0, || format!("residual {residual:.1} dB")),
        );
    }
    ratchet.finish();
}

/// How many pads the drum rack on `track` has.
fn drum_pads(driver: &mut impl Driver, track: u64) -> usize {
    let read = driver.query(json!({ "id": track }));
    read["devices"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|device| device["kind"]["rack"]["kind"] == json!("drum"))
        .and_then(|rack| rack["chains"].as_array())
        .map_or(0, Vec::len)
}

/// A MIDI track `Keys` with Cascade, quieter than its default so the chords
/// leave headroom, and a two-bar clip of [`PROGRESSION`].
fn cascade_progression(driver: &mut impl Driver) -> u64 {
    let notes = PROGRESSION.iter().enumerate().flat_map(|(index, chord)| {
        let at = index as i64 * BAR / 2;
        chord.map(|key| (key, at, BAR / 2 - BAR / 16, 100))
    });
    let made = driver.batch_ok(
        "Chords",
        vec![
            json!({ "command": "track.create", "args": { "id": { "$new": "remix keys" },
                    "kind": "midi", "name": "Keys", "index": 0 } }),
            json!({ "command": "device.insert", "args": { "track": { "$ref": "remix keys" },
                    "index": 0, "device": fixture::native("remix cascade", POLYSYNTH,
                    "instrument") } }),
            json!({ "command": "mixer.set_volume", "args": { "track": { "$ref": "remix keys" },
                    "volume": 0.25 } }),
            json!({ "command": "clip.create", "args": { "track": { "$ref": "remix keys" },
                    "clip": fixture::midi_clip("remix chords", 0, 2 * BAR,
                    fixture::midi_data(notes)) } }),
        ],
    );
    new_id(&made, "remix keys")
}

/// Whether the song has a locator at bar 1 and bar 3, and the Keys track
/// two clips.
fn arranged(song: &Value, keys: &Value) -> Result<(), String> {
    let positions: Vec<i64> = song["saved"]["settings"]["locators"]["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|locator| locator["position"].as_i64())
        .collect();
    ensure(
        positions.contains(&0)
            && positions.contains(&(2 * BAR))
            && keys["clips"]["total"] == json!(2),
        || {
            format!(
                "locators at {positions:?}; the Keys track's clips: {}",
                keys["clips"]
            )
        },
    )
}

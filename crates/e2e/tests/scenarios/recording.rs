//! Recording: an audio track armed on the null device's input, which
//! carries a 440 Hz tone, records two bars into the Arrangement, and the
//! take lands as a clip of that length holding the tone; then notes played
//! on the computer MIDI keyboard into an armed Cascade track come back as a
//! clip through Capture MIDI. The edits are undone back to the start,
//! redone, and the set saved and reopened.

use bl_e2e::fixture::{self, POLYSYNTH};
use bl_e2e::{BAR, BEAT, Driver, ExportSpec, Host, Known};
use serde_json::json;

use super::{ensure, frame, redo_to, rms_db, save_and_reopen, tone_db, track_named, undo_to};

/// The input tone's frequency in hertz.
const TONE_HZ: f64 = 440.0;

/// The input tone's amplitude: -6 dBFS.
const TONE_AMPLITUDE: f32 = 0.5;

/// The computer keyboard's keys played, and the notes they give at the
/// keyboard's starting octave: C, E and G from C3.
const PLAYED: [(&str, u64); 3] = [("A", 60), ("D", 64), ("J", 71)];

/// The transport's position, in ticks.
fn position(host: &mut Host) -> i64 {
    host.run_ok("transport.state", json!({}))["position"]
        .as_i64()
        .unwrap_or_default()
}

/// The clips on `track`, as the query tool lists them.
fn clips(driver: &mut impl Driver, track: u64) -> Vec<serde_json::Value> {
    driver.query(json!({ "id": track, "limit": 100 }))["clips"]["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[test]
fn a_recording_lands_as_a_clip_of_the_right_length_with_the_input_in_it() {
    let mut host = Host::new();
    let mut ratchet = Known::load().ratchet("scenarios");
    let start = host.document().clone();
    host.recorder()
        .set_input_tone(Some((0, TONE_HZ, TONE_AMPLITUDE)));

    // An audio track on input 1 alone, armed.
    let audio = track_named(&mut host, "3 Audio");
    host.run_ok(
        "routing.set_input",
        json!({ "track": audio,
                "input": { "type": "external", "channel": { "type": "mono", "channel": 1 } } }),
    );
    host.run_ok("mixer.set_arm", json!({ "track": audio, "arm": true }));

    // Two bars recorded from the song's start.
    // The null device's inputs opened, as Settings > Audio's Audio Input
    // Device does: the set starts with none.
    host.run_ok("audio.configure", json!({ "input_device": "null:null" }));
    host.settle();

    // A recording goes into the set's project folder, so the set is saved
    // first.
    let project = host.dir().join("project");
    std::fs::create_dir_all(&project).expect("a project folder");
    host.save_to(&project);
    host.run_ok("transport.set_recording", json!({ "recording": true }));
    if !host.is_playing() {
        host.play();
    }
    host.wait_until("two bars to be recorded", |host| position(host) >= 2 * BAR);
    let stopped_at = position(&mut host);
    host.run_ok("transport.stop_recording", json!({}));
    host.stop();
    host.wait_until("the take to land as a clip", |host| {
        !clips(host, audio).is_empty()
    });
    host.settle();
    let takes = clips(&mut host, audio);
    let length = takes[0]["length"]["ticks"].as_i64().unwrap_or_default();
    let starts = takes[0]["start"]["ticks"].as_i64().unwrap_or(-1);
    // Spec: recording R1, R4. An armed audio track records its input, and
    // when the take ends its file becomes one clip at the take's frames.
    ratchet.check(
        "recording/audio take",
        "the take lands as one clip from the start to where recording stopped",
        ensure(
            takes.len() == 1 && starts == 0 && (length - stopped_at).abs() <= BEAT / 4,
            || format!("recording stopped at {stopped_at} ticks; the track's clips: {takes:?}"),
        ),
    );

    // The take holds the input: the tone at its level, from its start to
    // its end, heard through the track once it is disarmed.
    host.run_ok("mixer.set_arm", json!({ "track": audio, "arm": false }));
    host.recorder().set_input_tone(None);
    host.settle();
    let played = host.export(
        0,
        2 * BAR,
        &ExportSpec {
            extra: json!({ "rendered_track": { "track": audio } }),
            ..ExportSpec::default()
        },
    );
    let levels: Vec<f64> = [(0.05, 0.5), (1.75, 2.25), (3.4, 3.9)]
        .iter()
        .map(|&(from, to)| tone_db(&played, TONE_HZ, frame(from), frame(to)))
        .collect();
    let whole = rms_db(&played, frame(0.05), frame(3.9));
    let expected = bl_e2e::audio::amplitude_db(f64::from(TONE_AMPLITUDE));
    ratchet.check(
        "recording/audio take",
        "the take plays the input tone at its level from start to end",
        ensure(
            levels.iter().all(|level| (level - expected).abs() < 3.0),
            || {
                format!(
                    "the tone's level near the take's start, middle and end: {levels:.1?} dBFS, \
                     against {expected:.1}; the take's whole level {whole:.1} dBFS"
                )
            },
        ),
    );

    // Capture MIDI: three notes played on the computer keyboard into an
    // armed Cascade track, the transport stopped, come back as a clip.
    let keys = track_named(&mut host, "1 MIDI");
    host.batch_ok(
        "Keys",
        vec![
            json!({ "command": "device.insert", "args": { "track": keys, "index": 0,
                "device": fixture::native("recording synth", POLYSYNTH, "instrument") } }),
        ],
    );
    host.run_ok("mixer.set_arm", json!({ "track": keys, "arm": true }));
    host.run_ok("midi.toggle_computer_keyboard", json!({ "on": true }));
    host.settle();
    for (key, _) in PLAYED {
        host.run_ok(
            "midi.computer_keyboard_key",
            json!({ "key": key, "pressed": true }),
        );
        host.advance_ms(200);
        host.run_ok(
            "midi.computer_keyboard_key",
            json!({ "key": key, "pressed": false }),
        );
        host.advance_ms(100);
    }
    let captured = host.run("midi.capture", json!({}));
    let made: Vec<serde_json::Value> = captured
        .as_ref()
        .ok()
        .and_then(|reply| reply["clips"].as_array().cloned())
        .unwrap_or_default();
    let notes: Vec<u64> = made
        .first()
        .map(|clip| {
            let read = host.query(json!({ "id": clip["clip"], "detail": "full" }));
            read["content"]["notes"]["items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|note| note["key"].as_u64())
                .collect()
        })
        .unwrap_or_default();
    // Spec: capture-midi R1, R4. The notes played on a track that monitors its
    // input come back as one clip, in a Session slot when the transport is
    // stopped, with their pitches and order.
    ratchet.check(
        "recording/capture MIDI",
        "the notes played on the computer keyboard come back as a clip",
        captured.map(drop).and_then(|()| {
            ensure(
                made.len() == 1
                    && made[0]["track"] == json!(keys)
                    && notes == PLAYED.map(|(_, note)| note),
                || format!("the clips made: {made:?}; their notes: {notes:?}"),
            )
        }),
    );
    host.run_ok("mixer.set_arm", json!({ "track": keys, "arm": false }));

    // Undone to the start and redone; saved and reopened, the same.
    host.check();
    let end = host.document().clone();
    let steps = undo_to(&mut host, &start);
    ratchet.check(
        "recording/undo to the start",
        "undo steps back to the set the recording started from",
        steps.as_ref().map(drop).map_err(Clone::clone),
    );
    if let Ok(steps) = steps {
        ratchet.check(
            "recording/undo to the start",
            "redo brings every edit back",
            redo_to(&mut host, steps, &end),
        );
    }
    if host.document() == &end {
        ratchet.check(
            "recording/save and reopen",
            "the reopened set is the one saved",
            save_and_reopen(&mut host),
        );
    }
    ratchet.finish();
}

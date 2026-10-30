//! Long runs: the engine keeps playing what it should after hundreds of
//! loops, wraps and launches, and at the end of a long arrangement. Every
//! run is paced by hand on the null device, so minutes of song take a
//! fraction of that: most of them play at 1,000 BPM, where a beat lasts
//! 60 ms.
//!
//! Each case is checked through the known-failures ratchet: the key is the
//! case, the property `keeps playing`.

use bl_e2e::fixture::{self, POLYSYNTH};
use bl_e2e::{Audio, BAR, BEAT, Driver, Host, Known, full_depth};
use serde_json::{Value, json};

/// The ratchet's suite.
const SUITE: &str = "background";

/// The property every case checks.
const KEEPS_PLAYING: &str = "keeps playing";

/// The tempo the fast runs play at, in BPM: the API's highest.
const FAST: f64 = 1_000.0;

/// A beat at [`FAST`], in milliseconds.
const LAP_MS: u64 = 60;

/// The engine's sample rate on the null device.
const RATE: u32 = 48_000;

/// A lap is quiet below this peak, in dBFS. A Cascade note at the default
/// settings peaks near 0 dBFS.
const SOUNDS: f64 = -40.0;

/// The laps checked in one capture, so a long run never holds all its audio.
const CHUNK: u64 = 100;

/// The default set's `1 MIDI` track with Cascade on it, and the default
/// set's first two scenes.
struct Synth {
    track: u64,
    scenes: [u64; 2],
}

/// Puts Cascade on `1 MIDI`.
fn synth(host: &mut Host) -> Synth {
    let track = host.query(json!({ "path": "track \"1 MIDI\"" }))["id"]
        .as_u64()
        .expect("the default set's first MIDI track");
    let scene = |host: &mut Host, n: u32| {
        host.query(json!({ "path": format!("scene {n}") }))["id"]
            .as_u64()
            .expect("a default scene")
    };
    let scenes = [scene(host, 1), scene(host, 2)];
    host.batch_ok(
        "Cascade",
        vec![
            json!({ "command": "device.insert", "args": { "track": track, "index": 0,
                "device": fixture::native("cascade", POLYSYNTH, "instrument") } }),
        ],
    );
    Synth { track, scenes }
}

/// One note on `key` at the start, half a beat long.
fn one_note(key: u8) -> Value {
    fixture::midi_data([(key, 0, BEAT / 2, 100)])
}

/// Makes the song's tempo `bpm` throughout.
fn tempo(host: &mut Host, bpm: f64) {
    host.run_ok("song.set_tempo_breakpoint", json!({ "at": 0, "bpm": bpm }));
}

/// Fails unless every `lap_ms` window of `audio` sounds: a note starts in
/// every lap, so a window of a lap's length always holds one.
fn every_lap_sounds(audio: &Audio, lap_ms: u64, first_lap: u64) -> Result<(), String> {
    if let Some((channel, frame)) = audio.first_non_finite() {
        return Err(format!(
            "a sample that isn't finite at {frame} on channel {channel}"
        ));
    }
    let lap = (u64::from(audio.rate) * lap_ms / 1_000) as usize;
    let laps = audio.frames() / lap;
    for n in 0..laps {
        let window = audio.slice(n * lap, (n + 1) * lap);
        if window.peak_dbfs() < SOUNDS {
            return Err(format!(
                "lap {} is quiet: peak {:.1} dBFS",
                first_lap + n as u64,
                window.peak_dbfs()
            ));
        }
    }
    Ok(())
}

/// Plays on for `laps` laps of `lap_ms`, a chunk at a time, and fails at the
/// first lap that's quiet.
fn laps_sound(host: &mut Host, laps: u64, lap_ms: u64) -> Result<(), String> {
    let mut done = 0;
    while done < laps {
        let chunk = CHUNK.min(laps - done);
        let audio = host.capture_for(chunk * lap_ms);
        every_lap_sounds(&audio, lap_ms, done)?;
        done += chunk;
    }
    Ok(())
}

/// Where `audio` first reaches [`SOUNDS`], in frames.
fn onset(audio: &Audio) -> Option<usize> {
    let threshold = 10f64.powf(SOUNDS / 20.0) as f32;
    (0..audio.frames()).find(|&frame| {
        audio
            .channels
            .iter()
            .any(|channel| channel[frame].abs() >= threshold)
    })
}

/// Checks `outcome` as `key` against the known failures.
fn ratchet(key: &str, outcome: Result<(), String>) {
    let mut ratchet = Known::load().ratchet(SUITE);
    ratchet.check(key, KEEPS_PLAYING, outcome);
    ratchet.finish();
}

#[test]
fn a_one_beat_session_loop_still_sounds_after_hundreds_of_laps() {
    let laps = if full_depth() { 3_000 } else { 300 };
    let mut host = Host::new();
    let synth = synth(&mut host);
    let made = host.batch_ok(
        "A one-beat loop",
        vec![
            json!({ "command": "clip.create_in_slot", "args": { "track": synth.track,
                "scene": synth.scenes[0],
                "clip": fixture::midi_clip("Beat", 0, BEAT, one_note(60)) } }),
        ],
    );
    let clip = made["ids"]["Beat"].as_u64().expect("the clip's ID");
    host.run_ok(
        "clip.set_loop_enabled",
        json!({ "clip": clip, "enabled": true }),
    );
    tempo(&mut host, FAST);
    host.settle();
    // Stopped, a launch starts the transport and the clip at once.
    host.run_ok(
        "session.launch",
        json!({ "track": synth.track, "scene": synth.scenes[0] }),
    );
    host.wait_until("the launch to start the song", Host::is_playing);
    ratchet(
        "a one-beat Session loop played for hundreds of laps",
        laps_sound(&mut host, laps, LAP_MS),
    );
}

#[test]
fn a_looped_arrangement_region_still_plays_its_clip_after_hundreds_of_wraps() {
    let wraps = if full_depth() { 5_000 } else { 500 };
    let mut host = Host::new();
    let synth = synth(&mut host);
    host.batch_ok(
        "A one-beat region",
        vec![
            json!({ "command": "clip.create", "args": { "track": synth.track,
                    "clip": fixture::midi_clip("Beat", 0, BEAT, one_note(60)) } }),
            json!({ "command": "transport.set_loop", "args": { "start": 0, "length": BEAT } }),
            json!({ "command": "transport.set_loop_enabled", "args": { "enabled": true } }),
        ],
    );
    tempo(&mut host, FAST);
    host.settle();
    host.locate(0);
    host.play();
    ratchet(
        "a looped Arrangement region wrapped hundreds of times",
        laps_sound(&mut host, wraps, LAP_MS),
    );
}

/// The test clock: how much audio the null device has played, in seconds.
/// A frame of [`Host::pump`] runs whole blocks, a little more than its
/// 5 ms, so time is read here rather than counted in calls.
fn clock(host: &mut Host) -> f64 {
    host.control()
        .clock()
        .expect("the null device is paced by hand")
        .elapsed()
        .as_secs_f64()
}

/// Plays on until the test clock reaches `at` seconds.
fn advance_to(host: &mut Host, at: f64) {
    while clock(host) < at {
        host.pump();
    }
}

/// Keeps what plays from now until the test clock reaches `at` seconds,
/// and when the capture started, on the test clock.
fn capture_to(host: &mut Host, at: f64) -> (f64, Audio) {
    let from = clock(host);
    host.recorder().start();
    advance_to(host, at);
    (from, host.recorder().stop())
}

/// Where the song's start lies on the test clock, from the transport's
/// position, at a constant `bpm`.
fn song_start(host: &mut Host, bpm: f64) -> f64 {
    let now = clock(host);
    let status = host
        .tool("transport", json!({ "action": "status" }))
        .expect("the transport's status");
    let ticks = status["position"]["ticks"].as_f64().unwrap_or_default();
    now - ticks / BEAT as f64 * 60.0 / bpm
}

#[test]
fn a_ten_minute_arrangement_with_tempo_automation_plays_its_last_clip_on_time() {
    // The tempo rises in a straight line from 60 BPM at the start to 180 at
    // beat 1,092, where the last clip starts. In beats the ramp is linear,
    // so beat b sounds at 60 b / 120 ln(T(b) / 60) seconds: the last clip
    // at 546 ln 3, about 599.84 s, and the song ends a bar later, ten
    // minutes in.
    const LAST: i64 = 1_092;
    let mut host = Host::new();
    let synth = synth(&mut host);
    host.batch_ok(
        "Ten minutes",
        vec![
            json!({ "command": "clip.create", "args": { "track": synth.track,
                    "clip": fixture::midi_clip("First", 0, BEAT, one_note(60)) } }),
            json!({ "command": "clip.create", "args": { "track": synth.track,
                    "clip": fixture::midi_clip("Last", LAST * BEAT, BAR, one_note(67)) } }),
            json!({ "command": "song.set_tempo_breakpoint", "args": { "at": 0, "bpm": 60.0 } }),
            json!({ "command": "song.set_tempo_breakpoint",
                    "args": { "at": LAST * BEAT, "bpm": 180.0 } }),
        ],
    );
    host.settle();
    host.locate(0);
    host.settle();
    let expected = 546.0 * 3f64.ln();
    let outcome = (|| {
        // The first clip's note starts the song: where it sounds is where
        // the song starts on the test clock.
        let from = clock(&mut host);
        host.recorder().start();
        host.play();
        advance_to(&mut host, from + 1.0);
        let first = host.recorder().stop();
        let start =
            from + onset(&first).ok_or("the first clip is silent")? as f64 / f64::from(RATE);
        // Unheard, up to half a second before the last clip.
        advance_to(&mut host, start + expected - 0.5);
        let (from, last) = capture_to(&mut host, start + expected + 1.0);
        let at = onset(&last).ok_or_else(|| {
            format!(
                "nothing sounds from {:.2} s to {:.2} s",
                from - start,
                expected + 1.0
            )
        })?;
        let heard = from + at as f64 / f64::from(RATE) - start;
        eprintln!("the last clip sounds at {heard:.4} s, expected {expected:.4} s");
        // Within 5 ms: the engine follows the ramp, not an approximation.
        if (heard - expected).abs() > 0.005 {
            return Err(format!(
                "the last clip sounds at {heard:.4} s, not {expected:.4} s"
            ));
        }
        Ok(())
    })();
    ratchet("a ten-minute arrangement with tempo automation", outcome);
}

/// Launches a one-bar Session loop `into` seconds into the second bar of an
/// active two-bar Arrangement loop at 120 BPM, and checks it lands on the
/// wrap, the next line of the one-bar launch grid.
fn launch_in_the_loops_last_bar(into: f64) -> Result<(), String> {
    let mut host = Host::new();
    let synth = synth(&mut host);
    let notes = (0..4).map(|n| (60, n * BEAT, BEAT / 2, 100));
    host.batch_ok(
        "A loop in the Arrangement and a clip in the Session",
        vec![
            json!({ "command": "clip.create_in_slot", "args": { "track": synth.track,
                    "scene": synth.scenes[0],
                    "clip": fixture::midi_clip("Bar", 0, BAR, fixture::midi_data(notes)) } }),
            json!({ "command": "transport.set_loop", "args": { "start": 0, "length": 2 * BAR } }),
            json!({ "command": "transport.set_loop_enabled", "args": { "enabled": true } }),
        ],
    );
    tempo(&mut host, 120.0);
    host.settle();
    host.locate(0);
    host.play();
    host.advance_ms(100);
    // A bar lasts two seconds, so the loop wraps four seconds in.
    let start = song_start(&mut host, 120.0);
    let wrap = start + 4.0;
    advance_to(&mut host, start + 2.0 + into);
    host.run_ok(
        "session.launch",
        json!({ "track": synth.track, "scene": synth.scenes[0] }),
    );
    // Queued until the wrap; this capture ends 50 ms before it.
    let (_, queued) = capture_to(&mut host, wrap - 0.05);
    if !queued.is_silent(SOUNDS) {
        return Err(format!(
            "the clip sounded before the wrap: peak {:.1} dBFS",
            queued.peak_dbfs()
        ));
    }
    let (from, after) = capture_to(&mut host, wrap + 2.0);
    let Some(at) = onset(&after) else {
        return Err("the launch never landed: nothing sounds after the wrap".to_owned());
    };
    // On the wrap, within a block or two of where the position put it.
    let late = from + at as f64 / f64::from(RATE) - wrap;
    if late.abs() > 0.015 {
        return Err(format!(
            "the clip started {:.1} ms from the wrap",
            late * 1_000.0
        ));
    }
    // And it plays on as a loop, a note each beat.
    every_lap_sounds(&after.slice(at, after.frames()), 500, 0)
}

#[test]
fn a_session_launch_in_the_last_bar_of_an_arrangement_loop_lands_on_the_wrap() {
    // Spec: launch-quantisation R37. A launch whose grid line is the loop's
    // end lands as the playhead wraps, on the loop's start frame.
    let mut ratchet = Known::load().ratchet(SUITE);
    let points: &[(f64, &str)] = if full_depth() {
        &[
            (
                0.1,
                "a Session launch early in an Arrangement loop's last bar",
            ),
            (
                1.0,
                "a Session launch mid-way through an Arrangement loop's last bar",
            ),
            (1.7, "a Session launch in an Arrangement loop's last beat"),
        ]
    } else {
        &[(
            1.0,
            "a Session launch mid-way through an Arrangement loop's last bar",
        )]
    };
    for &(into, key) in points {
        ratchet.check(key, KEEPS_PLAYING, launch_in_the_loops_last_bar(into));
    }
    ratchet.finish();
}

#[test]
fn follow_actions_keep_launching_for_hundreds_of_launches() {
    let launches = if full_depth() { 2_000 } else { 200 };
    let mut host = Host::new();
    let synth = synth(&mut host);
    let made = host.batch_ok(
        "Two clips that follow each other",
        vec![
            json!({ "command": "clip.create_in_slot", "args": { "track": synth.track,
                    "scene": synth.scenes[0],
                    "clip": fixture::midi_clip("A", 0, BEAT, one_note(60)) } }),
            json!({ "command": "clip.create_in_slot", "args": { "track": synth.track,
                    "scene": synth.scenes[1],
                    "clip": fixture::midi_clip("B", 0, BEAT, one_note(67)) } }),
        ],
    );
    for name in ["A", "B"] {
        let clip = made["ids"][name].as_u64().expect("the clip's ID");
        // Spec: follow-actions R5. With one other clip in the group, Other
        // always launches it.
        host.run_ok(
            "clip.set_follow_actions",
            json!({ "clip": clip, "enabled": true, "action_a": "other", "chance_a": 100,
                    "linked": false, "time": BEAT }),
        );
    }
    tempo(&mut host, FAST);
    host.settle();
    host.run_ok(
        "session.launch",
        json!({ "track": synth.track, "scene": synth.scenes[0] }),
    );
    host.wait_until("the launch to start the song", Host::is_playing);
    ratchet(
        "follow actions over hundreds of launches",
        laps_sound(&mut host, launches, LAP_MS),
    );
}

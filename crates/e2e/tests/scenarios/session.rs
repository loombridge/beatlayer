//! A Session performance: two tracks of Cascade clips, each clip holding
//! one note so its pitch says which clip sounds. A scene launches, a clip
//! is queued to the next bar, a follow action moves a track on, a legato
//! clip takes over mid-loop, a clip with its own launch quantisation
//! starts at once, and Stop All Clips ends it on the grid, all captured
//! from the hand-paced device in one run. Then the set's edits are undone
//! back to the start, redone, saved and reopened.

use bl_e2e::fixture::{self, POLYSYNTH};
use bl_e2e::{Audio, BAR, BEAT, Driver, Host, Known, SIXTEENTH};
use serde_json::{Value, json};

use super::{
    RATE, ensure, new_id, redo_to, rms_db, save_and_reopen, tone_db, track_named, undo_to,
};

/// How long a tick lasts at the default set's 120 BPM, in seconds.
const SECONDS_PER_TICK: f64 = 0.5 / BEAT as f64;

/// A MIDI key's frequency in hertz.
fn hz(key: u8) -> f64 {
    440.0 * 2f64.powf((f64::from(key) - 69.0) / 12.0)
}

/// The clips: name, track (0 or 1), scene (0 to 2), and the key held over
/// each half of its bar. Their fundamentals and first harmonics stay apart.
const CLIPS: [(&str, usize, usize, [u8; 2]); 5] = [
    ("A", 0, 0, [60, 60]),
    ("B", 0, 1, [66, 66]),
    ("E", 0, 2, [64, 69]),
    ("C", 1, 0, [51, 51]),
    ("D", 1, 1, [55, 55]),
];

/// A one-bar looping clip for a slot, holding `keys[0]` over its first
/// half and `keys[1]` over its second.
fn looping_clip(name: &str, keys: [u8; 2]) -> Value {
    let notes = if keys[0] == keys[1] {
        vec![(keys[0], 0, BAR - SIXTEENTH, 100)]
    } else {
        vec![
            (keys[0], 0, BAR / 2 - SIXTEENTH, 100),
            (keys[1], BAR / 2, BAR / 2 - SIXTEENTH, 100),
        ]
    };
    let mut data = fixture::midi_data(notes);
    data["midi"]["looping"] = json!(true);
    data["midi"]["loop_end"] = json!(BAR);
    fixture::midi_clip(&format!("session {name}"), 0, BAR, data)
}

/// The transport's position, in ticks.
fn position(host: &mut Host) -> i64 {
    host.run_ok("transport.state", json!({}))["position"]
        .as_i64()
        .unwrap_or_default()
}

/// Pumps the device until the song reaches `ticks`.
fn advance_to(host: &mut Host, ticks: i64) {
    host.wait_until("the song to reach its next cue", |host| {
        position(host) >= ticks
    });
}

/// How a capture sounds over `from` to `to` seconds of the song: the level
/// of each clip's pitch in it, by clip name.
struct Window<'a> {
    audio: &'a Audio,
    from: usize,
    to: usize,
}

impl Window<'_> {
    /// The level of `key`'s fundamental in the window, in dBFS.
    fn level(&self, key: u8) -> f64 {
        tone_db(self.audio, hz(key), self.from, self.to)
    }

    /// `Ok` if each key of `on` sounds and none of `off` does: each of `on`
    /// at least -40 dBFS, each of `off` at least 30 dB under the quietest
    /// of `on`.
    fn hears(&self, on: &[u8], off: &[u8]) -> Result<(), String> {
        let heard: Vec<(u8, f64)> = on.iter().map(|&key| (key, self.level(key))).collect();
        let quiet: Vec<(u8, f64)> = off.iter().map(|&key| (key, self.level(key))).collect();
        let floor = heard.iter().map(|h| h.1).fold(f64::INFINITY, f64::min);
        ensure(
            floor >= -40.0 && quiet.iter().all(|q| q.1 <= floor - 30.0),
            || {
                format!(
                    "from {:.2} s to {:.2} s, the keys that should sound and their levels: \
                     {heard:.1?}; those that shouldn't: {quiet:.1?}",
                    self.from as f64 / RATE,
                    self.to as f64 / RATE
                )
            },
        )
    }
}

/// The window of `audio` from `from` to `to` seconds of the song.
fn window(audio: &Audio, from: f64, to: f64) -> Window<'_> {
    Window {
        audio,
        from: (from * RATE) as usize,
        to: (to * RATE) as usize,
    }
}

/// The ticks `seconds` into the song at 120 BPM.
fn ticks(seconds: f64) -> i64 {
    (seconds / SECONDS_PER_TICK).round() as i64
}

#[test]
fn a_session_performance_launches_follows_and_stops_on_the_grid() {
    let mut host = Host::new();
    let mut ratchet = Known::load().ratchet("scenarios");
    let start = host.document().clone();
    let scenes = fixture::scenes(&mut host);
    let tracks = [
        track_named(&mut host, "1 MIDI"),
        track_named(&mut host, "2 MIDI"),
    ];

    // Cascade on both tracks, quiet enough for two notes at once, and the
    // clips in their slots.
    let mut commands = Vec::new();
    for (index, track) in tracks.iter().enumerate() {
        commands.push(
            json!({ "command": "device.insert", "args": { "track": track, "index": 0,
            "device": fixture::native(&format!("session synth {index}"), POLYSYNTH,
                                      "instrument") } }),
        );
        commands.push(json!({ "command": "mixer.set_volume",
                              "args": { "track": track, "volume": 0.25 } }));
    }
    for (name, track, scene, keys) in CLIPS {
        commands.push(json!({ "command": "clip.create_in_slot", "args": {
            "track": tracks[track], "scene": scenes[scene], "clip": looping_clip(name, keys) } }));
    }
    let made = host.batch_ok("Session clips", commands);
    let clip = |name: &str| new_id(&made, &format!("session {name}"));
    let key = |name: &str| CLIPS.iter().find(|c| c.0 == name).expect("a clip").3[0];

    // C follows on to the next slot, D, after one pass; E is legato; C has
    // launch quantisation of its own: none.
    host.run_ok(
        "clip.set_follow_actions",
        json!({ "clip": clip("C"), "enabled": true, "action_a": "next", "action_b": "no_action",
                "chance_a": 100, "linked": true, "multiplier": 1 }),
    );
    host.run_ok(
        "clip.set_legato",
        json!({ "clip": clip("E"), "legato": true }),
    );
    host.run_ok(
        "clip.set_launch_quantization",
        json!({ "clip": clip("C"), "quantization": "none" }),
    );
    host.run_ok(
        "session.set_quantization",
        json!({ "quantization": "1_bar" }),
    );
    host.settle();
    let edited = host.document().clone();

    // The performance, captured in one take.
    host.recorder().start();
    host.run_ok("session.launch_scene", json!({ "scene": scenes[0] }));
    advance_to(&mut host, ticks(0.5));
    host.run_ok(
        "session.launch",
        json!({ "track": tracks[0], "scene": scenes[1] }),
    );
    advance_to(&mut host, ticks(4.0));
    host.run_ok(
        "session.set_quantization",
        json!({ "quantization": "half" }),
    );
    advance_to(&mut host, ticks(4.5));
    host.run_ok(
        "session.launch",
        json!({ "track": tracks[0], "scene": scenes[2] }),
    );
    advance_to(&mut host, ticks(6.25));
    host.run_ok(
        "session.launch",
        json!({ "track": tracks[1], "scene": scenes[0] }),
    );
    advance_to(&mut host, ticks(7.5));
    host.run_ok("session.stop_all", json!({}));
    advance_to(&mut host, ticks(9.0));
    let take = host.recorder().stop();
    host.stop();
    bl_e2e::audio::assert_finite(&take, "the performance");

    // Spec: launch-quantisation R7. A scene launched with the transport
    // stopped starts it, and its clips start together.
    ratchet.check(
        "session/launch a scene",
        "the scene's clips sound together from the start",
        window(&take, 0.1, 1.9).hears(&[key("A"), key("C")], &[key("B"), key("D")]),
    );
    // Spec: launch-quantisation R2, R11. A clip launched while the song
    // plays waits for the next line of the grid, the old clip playing until
    // then.
    ratchet.check(
        "session/launch quantisation",
        "the clip launched mid-bar takes over at the next bar, not before",
        window(&take, 0.6, 1.9)
            .hears(&[key("A")], &[key("B")])
            .and_then(|()| window(&take, 2.1, 3.9).hears(&[key("B")], &[key("A")])),
    );
    // Spec: follow-actions R5, R9. A Linked looping clip follows after its
    // loop times the multiplier; Next launches the next clip of its group.
    ratchet.check(
        "session/follow action",
        "the clip with Next as its follow action hands over to the next slot after one pass",
        window(&take, 2.1, 3.9).hears(&[key("D")], &[key("C")]),
    );
    // Spec: launch-quantisation R27. A Legato clip takes over the clip
    // time the clip it replaces has reached.
    let e = CLIPS[2].3;
    ratchet.check(
        "session/legato",
        "the legato clip takes over half way through its bar, as the old clip was",
        window(&take, 5.1, 5.9)
            .hears(&[e[1]], &[e[0], key("B")])
            .and_then(|()| window(&take, 6.1, 6.9).hears(&[e[0]], &[e[1]])),
    );
    // Spec: launch-quantisation R25, R3. A clip's own launch quantisation
    // places its launch; None launches at once.
    ratchet.check(
        "session/clip launch quantisation",
        "the clip with no quantisation of its own starts as soon as it is launched",
        window(&take, 5.6, 6.2)
            .hears(&[key("D")], &[key("C")])
            .and_then(|()| window(&take, 6.35, 7.1).hears(&[key("C")], &[key("D")])),
    );
    // Spec: launch-quantisation R6, R26. Stop All Clips stops every clip
    // on the next line of the global grid.
    let playing = rms_db(&take, (7.6 * RATE) as usize, (7.95 * RATE) as usize);
    let after = rms_db(&take, (8.6 * RATE) as usize, (9.0 * RATE) as usize);
    ratchet.check(
        "session/stop all",
        "Stop All Clips lets the clips play to the next half bar, then they stop",
        ensure(playing > -40.0 && after < playing - 40.0, || {
            format!("{playing:.1} dBFS before the line, {after:.1} dBFS half a second after")
        }),
    );

    // The set's edits undone to the start and redone; saved and reopened.
    host.check();
    let end = host.document().clone();
    let steps = undo_to(&mut host, &start);
    ratchet.check(
        "session/undo to the start",
        "undo steps back to the set the performance started from",
        steps.as_ref().map(drop).map_err(Clone::clone),
    );
    if let Ok(steps) = steps {
        ratchet.check(
            "session/undo to the start",
            "redo brings every edit back",
            redo_to(&mut host, steps, &end),
        );
    }
    ratchet.check(
        "session/performance leaves the set alone",
        "launching, following and stopping clips change nothing in the set but the quantisation",
        ensure(
            bl_e2e::document_diff(&edited, &end, 8)
                .iter()
                .all(|line| line.contains("quantization")),
            || bl_e2e::document_diff(&edited, &end, 8).join("\n"),
        ),
    );
    ratchet.check(
        "session/save and reopen",
        "the reopened set is the one saved",
        save_and_reopen(&mut host),
    );
    ratchet.finish();
}

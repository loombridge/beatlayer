//! MIDI Timecode in (WP-7.13, `docs/specs/midi-timecode.md` R1, R10 to R12):
//! an input's Sync Type, frame rate and offset are set by ID and kept across
//! a relaunch; timecode from a test port on a null device paced by hand
//! starts the song with EXT on, lights the upper lamp, and is reported by
//! `sync.state`; an output has no timecode options.

use std::time::{Duration, Instant};

use bl_audio_io::NullControl;
use bl_midi_io::{Direction, TestBackend};
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::{Shell, testing};
use serde_json::{Value, json};

const PORT: &str = "Tape";

fn start(dir: &std::path::Path) -> (Shell, TestBackend) {
    let prefs = Preferences::load(dir).unwrap();
    let test = TestBackend::new();
    test.add_port(Direction::Input, PORT);
    test.add_port(Direction::Output, "Synth");
    let mut shell = Shell::new(
        prefs.clone(),
        Some(dir.to_owned()),
        AudioHost::null(prefs.audio),
        Box::new(ProjectFiles::new()),
    );
    shell.use_midi_backend(Box::new(test.clone()));
    (shell, test)
}

fn port(shell: &mut Shell, name: &str) -> Value {
    let ports = shell.run("midi.ports", json!({})).unwrap();
    ports["ports"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == name)
        .unwrap()
        .clone()
}

// Spec: midi-timecode R1, R11. The options are set by ID, refused out of
// range and on an output, and kept across a relaunch.
#[test]
fn the_sync_type_rate_and_offset_are_set_by_id_and_kept_across_a_relaunch() {
    let dir = std::env::temp_dir().join(format!("bl-shell-timecode-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (mut shell, _) = start(&dir);
    let tape = port(&mut shell, PORT);
    assert_eq!(tape["sync_type"], "midi_clock");
    assert_eq!(tape["timecode_rate"], "smpte_all");
    assert_eq!(tape["timecode_offset"], "00:00:00:00");

    shell
        .run(
            "midi.set_port",
            json!({
                "direction": "input", "name": PORT, "sync": true,
                "sync_type": "midi_timecode", "timecode_rate": "fps_29_97_drop",
                "timecode_offset": "01:00:00;02",
            }),
        )
        .unwrap();
    let tape = port(&mut shell, PORT);
    assert_eq!(tape["sync_type"], "midi_timecode");
    assert_eq!(tape["timecode_rate"], "fps_29_97_drop");
    assert_eq!(tape["timecode_offset"], "01:00:00:02");

    for (args, why) in [
        (json!({ "timecode_offset": "24:00:00:00" }), "hours"),
        (json!({ "timecode_offset": "00:00:00:30" }), "frames"),
        (json!({ "timecode_offset": "ten" }), "text"),
    ] {
        let mut args = args;
        args["direction"] = json!("input");
        args["name"] = json!(PORT);
        assert!(shell.run("midi.set_port", args).is_err(), "{why}");
    }
    assert_eq!(port(&mut shell, PORT)["timecode_offset"], "01:00:00:02");
    // An output never sends timecode, so it has no options.
    for field in [
        ("sync_type", json!("midi_timecode")),
        ("timecode_rate", json!("fps_25")),
        ("timecode_offset", json!("00:00:01:00")),
    ] {
        let mut args = json!({ "direction": "output", "name": "Synth" });
        args[field.0] = field.1;
        assert!(shell.run("midi.set_port", args).is_err(), "{}", field.0);
    }
    drop(shell);

    let (mut shell, _) = start(&dir);
    shell.refresh_midi();
    let tape = port(&mut shell, PORT);
    assert_eq!(tape["sync_type"], "midi_timecode");
    assert_eq!(tape["timecode_rate"], "fps_29_97_drop");
    assert_eq!(tape["timecode_offset"], "01:00:00:02");
    assert_eq!(tape["sync"], true);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A shell on a null device paced by hand, with an input set to MIDI
/// Timecode at SMPTE All, and EXT on if `ext`.
fn rig(ext: bool) -> (Shell, NullControl, TestBackend, Instant) {
    let (mut shell, control) = testing::rig();
    let test = TestBackend::new();
    test.add_port(Direction::Input, PORT);
    shell.use_midi_backend(Box::new(test.clone()));
    shell.use_test_clock(control.clock().expect("manual pacing"));
    shell.start_audio();
    shell
        .run(
            "midi.set_port",
            json!({
                "direction": "input", "name": PORT, "sync": true,
                "sync_type": "midi_timecode",
            }),
        )
        .unwrap();
    shell
        .run("transport.set_external_sync", json!({ "on": ext }))
        .unwrap();
    testing::settle(&control, &mut shell);
    let base = control.clock().unwrap().now();
    (shell, control, test, base)
}

/// Sends quarter frames at 25 fps from frame `from`, `frames` of them, the
/// first sequence `after` after `base`.
fn timecode(test: &TestBackend, base: Instant, after: Duration, from: u32, frames: u32) {
    let frame = Duration::from_millis(40);
    for pair in 0..frames / 2 {
        let n = from + pair * 2;
        let (h, m, s, f) = (n / 90_000, n / 1_500 % 60, n / 25 % 60, n % 25);
        let nibbles = [
            f & 15,
            f >> 4,
            s & 15,
            s >> 4,
            m & 15,
            m >> 4,
            h & 15,
            (h >> 4) | 1 << 1,
        ];
        for (k, nibble) in nibbles.into_iter().enumerate() {
            let at = base + after + frame * (pair * 2) + frame * k as u32 / 4;
            assert!(test.send(PORT, at, &[0xF1, ((k as u32) << 4 | nibble) as u8]));
        }
    }
}

// Spec: midi-timecode R6, R10, R12. With EXT on, timecode starts the song,
// the upper lamp lights as whole times arrive, and sync.state reports the
// last whole time.
#[test]
fn timecode_starts_the_song_lights_the_upper_lamp_and_is_reported() {
    let (mut shell, control, test, base) = rig(true);
    timecode(&test, base, Duration::from_millis(5), 250, 100);
    testing::advance(&control, &mut shell, 400);
    shell.poll();
    let state = shell.run("transport.state", json!({})).unwrap();
    assert_eq!(state["playing"], true);
    assert!(shell.sync_input_lit(), "a whole time arrived a moment ago");
    let sync = shell.run("sync.state", json!({})).unwrap();
    assert_eq!(sync["leading_input"], PORT);
    assert_eq!(sync["receiving"], true);
    assert_eq!(sync["tempo"], Value::Null, "timecode brings no tempo");
    let report = &sync["timecode"];
    assert_eq!(report["rate"], "fps_25");
    assert_eq!(report["running"], true);
    let time = report["time"].as_str().unwrap();
    assert!(
        time.starts_with("00:00:10:") || time.starts_with("00:00:11:"),
        "{time}"
    );
    // Ten seconds on at 120 BPM is 20 quarter notes.
    let ticks = report["position_ticks"].as_i64().unwrap();
    assert!(ticks > 20 * 705_600 && ticks < 23 * 705_600, "{ticks}");

    // The stream ends: 100 ms on, the song stops.
    testing::advance(&control, &mut shell, 4_000);
    let state = shell.run("transport.state", json!({})).unwrap();
    assert_eq!(state["playing"], false);
    assert_eq!(
        shell.run("sync.state", json!({})).unwrap()["timecode"]["running"],
        false
    );
}

// Spec: midi-timecode R10. The lamp lights whether EXT is on or not, and
// nothing plays with it off.
#[test]
fn the_upper_lamp_lights_for_timecode_with_ext_off_and_nothing_plays() {
    let (mut shell, control, test, base) = rig(false);
    timecode(&test, base, Duration::from_millis(5), 0, 50);
    testing::advance(&control, &mut shell, 300);
    shell.poll();
    assert!(shell.sync_input_lit());
    assert_eq!(
        shell.run("transport.state", json!({})).unwrap()["playing"],
        false
    );
    assert_eq!(
        shell.run("sync.state", json!({})).unwrap()["timecode"]["running"],
        false
    );
}

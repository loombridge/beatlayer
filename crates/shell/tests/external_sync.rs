//! EXT and Options ▸ External Sync (WP-7.12, `docs/specs/midi-clock.md` R15
//! to R17, R22 to R25): the switch is there only while an input has Sync on
//! and is kept between starts; with it on the song follows the incoming
//! clock on a null device paced by hand, the Tempo field shows the followed
//! tempo and can't be changed, a clip that leads follows too, and with EXT
//! off the tempo map plays again.

use std::time::{Duration, Instant};

use bl_audio_io::NullControl;
use bl_core::time::{Bpm, TempoMap, Ticks};
use bl_midi_io::{Direction, TestBackend};
use bl_model::{AudioData, Clip, ClipData, ClipId, ContentHash, TrackId, TrackKind, WarpMarker};
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::transport::{Field, Group};
use bl_shell::{Shell, testing, transport};
use serde_json::json;

const PORT: &str = "Sequencer";

/// A shell on the preferences in `dir`, with an input plugged in.
fn start(dir: &std::path::Path) -> (Shell, TestBackend) {
    let prefs = Preferences::load(dir).unwrap();
    let test = TestBackend::new();
    test.add_port(Direction::Input, PORT);
    let mut shell = Shell::new(
        prefs.clone(),
        Some(dir.to_owned()),
        AudioHost::null(prefs.audio),
        Box::new(ProjectFiles::new()),
    );
    shell.use_midi_backend(Box::new(test.clone()));
    (shell, test)
}

fn sync_on(shell: &mut Shell, on: bool) {
    shell
        .run(
            "midi.set_port",
            json!({ "direction": "input", "name": PORT, "sync": on }),
        )
        .unwrap();
}

fn ext(shell: &mut Shell) -> bool {
    shell.run("transport.state", json!({})).unwrap()["external_sync"] == true
}

// Spec: midi-clock R15, R25. EXT needs an input with Sync on, is kept across
// a restart, and goes off with the last Sync input.
#[test]
fn ext_is_available_only_while_an_input_has_sync_on_and_is_kept_across_a_relaunch() {
    let dir = std::env::temp_dir().join(format!("bl-shell-ext-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (mut shell, _) = start(&dir);
    let state = shell.run("sync.state", json!({})).unwrap();
    assert_eq!(state["external_sync_available"], false);
    assert!(!ext(&mut shell));
    assert!(!shell.can_invoke("transport.set_external_sync", &json!({})));
    assert!(
        shell
            .run("transport.set_external_sync", json!({ "on": true }))
            .is_err(),
        "no input has Sync on"
    );

    sync_on(&mut shell, true);
    assert_eq!(
        shell.run("sync.state", json!({})).unwrap()["external_sync_available"],
        true
    );
    // The Options item is enabled, and ticked once EXT is on.
    assert!(shell.can_invoke("transport.set_external_sync", &json!({})));
    assert_eq!(
        shell.switch_state("transport.set_external_sync"),
        Some(false)
    );
    let on = shell.run("transport.set_external_sync", json!({})).unwrap();
    assert_eq!(on["on"], true, "left out, it switches to the other state");
    assert!(ext(&mut shell));
    assert_eq!(
        shell.switch_state("transport.set_external_sync"),
        Some(true)
    );
    drop(shell);

    let (mut shell, _) = start(&dir);
    shell.refresh_midi();
    assert!(ext(&mut shell), "EXT is kept");
    assert_eq!(
        shell.run("sync.state", json!({})).unwrap()["input_ports"],
        1
    );
    // The last Sync input off is EXT off.
    sync_on(&mut shell, false);
    assert!(!ext(&mut shell));
    sync_on(&mut shell, true);
    assert!(!ext(&mut shell), "and it stays off when Sync returns");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A shell on a null device paced by hand, with an input with Sync on and
/// EXT on, playing.
fn following() -> (Shell, NullControl, TestBackend, Instant) {
    let (mut shell, control) = testing::rig();
    let test = TestBackend::new();
    test.add_port(Direction::Input, PORT);
    shell.use_midi_backend(Box::new(test.clone()));
    shell.use_test_clock(control.clock().expect("manual pacing"));
    shell.start_audio();
    sync_on(&mut shell, true);
    shell
        .run("transport.set_external_sync", json!({ "on": true }))
        .unwrap();
    testing::settle(&control, &mut shell);
    let base = control.clock().unwrap().now();
    (shell, control, test, base)
}

/// Sends `count` clocks at `bpm`, the first `after` after `base`.
fn clocks(test: &TestBackend, base: Instant, after: Duration, bpm: f64, count: u32) {
    let period = Duration::from_secs_f64(60.0 / (bpm * 24.0));
    for k in 0..count {
        assert!(test.send(PORT, base + after + period * k, &[0xF8]));
    }
}

fn tempo_text(shell: &Shell) -> String {
    transport::text(shell, Field::new(Group::Tempo, 0))
}

// Spec: midi-clock R15 to R17, R24. The song follows the clock: the Tempo
// field shows its tempo and refuses edits while it arrives; EXT off plays
// the map's tempo again.
#[test]
fn the_tempo_field_shows_the_followed_tempo_and_ext_off_gives_the_map_back() {
    let (mut shell, control, test, base) = following();
    clocks(&test, base, Duration::from_millis(5), 140.0, 160);
    shell.run("transport.play", json!({})).unwrap();
    testing::advance(&control, &mut shell, 2_500);
    assert_eq!(tempo_text(&shell), "140.00");
    assert!(!transport::editable(&shell, Group::Tempo));
    let state = shell.run("transport.state", json!({})).unwrap();
    assert_eq!(state["external_sync"], true);
    assert!((state["tempo"].as_f64().unwrap() - 140.0).abs() < 0.01);
    assert_eq!(state["tempo_editable"], false);
    let sync = shell.run("sync.state", json!({})).unwrap();
    assert_eq!(sync["receiving"], true);
    assert_eq!(sync["leading_input"], PORT);
    assert!(sync["clocks_received"].as_u64().unwrap() > 80);
    assert!((sync["tempo"].as_f64().unwrap() - 140.0).abs() < 0.1);
    assert!(
        transport::step(&shell, Field::new(Group::Tempo, 0), 1, false).is_none(),
        "the clock sets the tempo"
    );
    // The upper lamp lights for 100 ms a quarter note, so over a second of
    // clock it is seen both lit and dark.
    let seen: Vec<bool> = (0..50)
        .map(|_| {
            testing::advance(&control, &mut shell, 20);
            shell.poll();
            shell.sync_input_lit()
        })
        .collect();
    assert!(seen.contains(&true) && seen.contains(&false), "{seen:?}");

    // EXT off: the map's tempo, on the same beat.
    let before = shell.run("transport.state", json!({})).unwrap()["position"]
        .as_i64()
        .unwrap();
    shell
        .run("transport.set_external_sync", json!({ "on": false }))
        .unwrap();
    testing::advance(&control, &mut shell, 100);
    assert_eq!(tempo_text(&shell), "120.00");
    assert!(transport::editable(&shell, Group::Tempo));
    let state = shell.run("transport.state", json!({})).unwrap();
    assert_eq!(state["playing"], true);
    let after = state["position"].as_i64().unwrap();
    assert!(
        after > before && after - before < 705_600,
        "{before} to {after}"
    );
}

// Spec: midi-clock R17, R24; warping R55. The tempo map's automation isn't
// played while the clock leads, and plays again after.
#[test]
fn tempo_automation_is_left_alone_while_the_clock_leads_and_plays_again_after() {
    let (mut shell, control, test, base) = following();
    let ramp = TempoMap::from_breakpoints([
        (Ticks::ZERO, Bpm::new(90.0).unwrap()),
        (Ticks(705_600 * 16), Bpm::new(180.0).unwrap()),
    ])
    .unwrap();
    shell
        .run("song.set_tempo", json!({ "tempo": ramp }))
        .unwrap();
    clocks(&test, base, Duration::from_millis(5), 100.0, 200);
    shell.run("transport.play", json!({})).unwrap();
    testing::advance(&control, &mut shell, 3_000);
    // The field shows the clock's 100 BPM, not the ramp's tempo.
    assert_eq!(tempo_text(&shell), "100.00");
    let map = shell.editor().document().settings().tempo.clone();
    assert_eq!(map, ramp, "the set's tempo automation is as it was");
    shell
        .run("transport.set_external_sync", json!({ "on": false }))
        .unwrap();
    testing::advance(&control, &mut shell, 50);
    let position = Ticks(
        shell.run("transport.state", json!({})).unwrap()["position"]
            .as_i64()
            .unwrap(),
    );
    let want = ramp.bpm_at(position).get();
    assert_eq!(tempo_text(&shell), format!("{want:.2}"));
    // Editing is refused over a ramp as before: the map isn't one tempo.
    assert!(!transport::editable(&shell, Group::Tempo));
}

/// A leading clip of one bar on a new audio track.
fn add_leader(shell: &mut Shell) -> ClipId {
    const Q: i64 = 705_600;
    let track: TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            json!({ "id": track, "kind": TrackKind::Audio, "name": "Lead", "index": 0 }),
        )
        .unwrap();
    let id: ClipId = shell.new_id();
    let data = AudioData {
        end: Some((4 * Q) as f64),
        tempo: Some(120.0),
        warp: true,
        warp_markers: vec![
            WarpMarker {
                time: 0.0,
                beat: Ticks(0),
            },
            WarpMarker {
                time: 4.0,
                beat: Ticks(4 * Q),
            },
        ],
        ..AudioData::new(ContentHash([3; 32]))
    };
    let clip = Clip::new(id, Ticks(0), Ticks(4 * Q), ClipData::Audio(data));
    shell
        .run("clip.create", json!({ "track": track, "clip": clip }))
        .unwrap();
    shell
        .run(
            "clip.set_tempo_leader",
            json!({ "clip": id, "leader": true }),
        )
        .unwrap();
    id
}

// Spec: midi-clock R17, R24; warping R55. A leading clip plays as a follower
// while EXT is on: the clock's tempo is the field's, not the clip's, and the
// set keeps its leader, which leads again with EXT off.
#[test]
fn a_leader_follows_the_clock_under_ext_and_leads_again_after() {
    let (mut shell, control, test, base) = following();
    add_leader(&mut shell);
    assert_eq!(tempo_text(&shell), "120.00", "the song's own tempo");
    clocks(&test, base, Duration::from_millis(5), 100.0, 200);
    shell.run("transport.play", json!({})).unwrap();
    testing::advance(&control, &mut shell, 2_000);
    assert_eq!(tempo_text(&shell), "100.00");
    assert!(shell.editor().document().any_tempo_leader(), "still leads");
    assert!(shell.external_sync(), "the Lead/Follow switches are off");
    shell
        .run("transport.set_external_sync", json!({ "on": false }))
        .unwrap();
    testing::advance(&control, &mut shell, 50);
    assert_eq!(tempo_text(&shell), "120.00");
    assert!(!shell.external_sync());
    assert!(shell.editor().document().any_tempo_leader());
}

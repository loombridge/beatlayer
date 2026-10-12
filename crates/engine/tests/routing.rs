//! WP-2.19's acceptance in the engine: routing and returns (the routing
//! spec).
//!
//! - A track routed into another plays through it, while it monitors.
//! - A track's input taken from another track before its devices, after
//!   them, or after its mixer.
//! - A resampling track carries the Main mix and adds nothing to it.
//! - A loop doesn't hang the engine: the routing that closes it is silent.
//! - Fifty return tracks play.
//! - Solo across routing.
//!
//! Every track is a DC level, so Main's output says exactly what's heard.
//! Routing commands run by ID with JSON arguments, as an agent runs them
//! (D17), and every block runs as a real-time section under the
//! allocation trap.

bl_rt::install_trap!();

mod common;

use bl_commands::Registry as Commands;
use bl_engine::routing::Graph;
use bl_model::{Editor, TrackId, TrackKind, registry};
use common::{Rig, dc, track};
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

/// Frames to render after a change, so a 20 ms glide has finished before
/// the last block of 256 frames.
const SETTLE: usize = 1280;

fn settled(rig: &mut Rig) -> f32 {
    rig.render(SETTLE, 256);
    let last = rig.last();
    assert!(last.iter().all(|&s| s == last[0]), "a steady level");
    last[0]
}

fn output_to(editor: &mut Editor, from: TrackId, to: TrackId) {
    run(
        editor,
        "routing.set_output",
        json!({ "track": from, "output": { "type": "track", "track": to } }),
    );
}

fn monitor(editor: &mut Editor, track: TrackId, monitor: &str) {
    run(
        editor,
        "routing.set_monitor",
        json!({ "track": track, "monitor": monitor }),
    );
}

// Spec: routing R10, R11
#[test]
fn a_track_routed_to_another_plays_through_it_while_it_monitors() {
    let mut rig = Rig::new(256);
    let ed = &mut rig.editor;
    let b = track(ed, TrackKind::Audio, None);
    let a = track(ed, TrackKind::Audio, None);
    dc(ed, a, 0.5);
    dc(ed, b, 0.125);
    run(ed, "mixer.set_volume", json!({ "track": b, "volume": 0.5 }));
    rig.sync();
    rig.render(512, 256);
    assert_eq!(rig.last()[0], 0.5 + 0.0625, "both to Main");

    // A into B's Track In: B is on Auto and not armed, so A isn't heard.
    output_to(&mut rig.editor, a, b);
    assert!(rig.sync().program, "a routing change is a new program");
    assert_eq!(settled(&mut rig), 0.0625);
    // Monitor In: A plays through B's devices and B's fader.
    monitor(&mut rig.editor, b, "in");
    assert!(!rig.sync().program, "monitoring opens a route, no program");
    assert_eq!(settled(&mut rig), (0.5 + 0.125) * 0.5);
    // Auto plays it while B is armed.
    monitor(&mut rig.editor, b, "auto");
    run(
        &mut rig.editor,
        "mixer.set_arm",
        json!({ "track": b, "arm": true }),
    );
    assert!(!rig.sync().program);
    assert_eq!(settled(&mut rig), (0.5 + 0.125) * 0.5);
    // Off never plays it.
    monitor(&mut rig.editor, b, "off");
    rig.sync();
    assert_eq!(settled(&mut rig), 0.0625);
    // Sends Only: A goes nowhere but its sends.
    run(
        &mut rig.editor,
        "routing.set_output",
        json!({ "track": a, "output": { "type": "sends_only" } }),
    );
    rig.sync();
    assert_eq!(settled(&mut rig), 0.0625);
}

// Spec: routing R12
#[test]
fn an_input_taps_another_track_before_its_devices_after_them_or_after_its_mixer() {
    let mut rig = Rig::new(256);
    let ed = &mut rig.editor;
    let b = track(ed, TrackKind::Audio, None);
    let a = track(ed, TrackKind::Audio, None);
    let source = track(ed, TrackKind::Audio, None);
    // The source sends 1 into A's input; A adds 0.5 in its chain and its
    // fader halves it.
    dc(ed, source, 1.0);
    run(
        ed,
        "routing.set_output",
        json!({ "track": source, "output": { "type": "sends_only" } }),
    );
    run(
        ed,
        "routing.set_input",
        json!({ "track": a, "input": { "type": "track", "track": source } }),
    );
    monitor(ed, a, "in");
    dc(ed, a, 0.5);
    run(ed, "mixer.set_volume", json!({ "track": a, "volume": 0.5 }));
    run(
        ed,
        "routing.set_output",
        json!({ "track": a, "output": { "type": "sends_only" } }),
    );
    monitor(ed, b, "in");
    rig.sync();
    for (tap, heard) in [("pre_fx", 1.0), ("post_fx", 1.5), ("post_mixer", 0.75)] {
        run(
            &mut rig.editor,
            "routing.set_input",
            json!({ "track": b, "input": { "type": "track", "track": a, "tap": tap } }),
        );
        rig.sync();
        assert_eq!(settled(&mut rig), heard, "{tap}");
    }
    // A new track's input from a track taps after its mixer.
    run(
        &mut rig.editor,
        "routing.set_input",
        json!({ "track": b, "input": { "type": "track", "track": a } }),
    );
    rig.sync();
    assert_eq!(settled(&mut rig), 0.75);
}

// Spec: routing R13
#[test]
fn a_resampling_track_carries_the_main_mix_and_adds_nothing_to_it() {
    let mut rig = Rig::new(480);
    let ed = &mut rig.editor;
    let resampler = track(ed, TrackKind::Audio, None);
    let ret = track(ed, TrackKind::Return, None);
    let a = track(ed, TrackKind::Audio, None);
    dc(ed, a, 0.25);
    run(
        ed,
        "mixer.set_send",
        json!({ "track": a, "to": ret, "level": 0.5 }),
    );
    run(
        ed,
        "routing.set_input",
        json!({ "track": resampler, "input": { "type": "resampling" } }),
    );
    monitor(ed, resampler, "in");
    // Its sends would feed back through the return: they're silent too.
    run(
        ed,
        "mixer.set_send",
        json!({ "track": resampler, "to": ret, "level": 1.0 }),
    );
    let main = ed.document().main_id();
    rig.sync();
    let mix = 0.25 + 0.125;
    assert_eq!(
        settled(&mut rig),
        mix,
        "Main is the mix, without the resampler"
    );
    let state = rig.monitor.state();
    assert_eq!(
        state.meter(resampler).unwrap(),
        state.meter(main).unwrap(),
        "the resampling track carries Main's signal"
    );
    assert_eq!(state.meter(resampler).unwrap(), [mix, mix]);
    let graph = Graph::of(rig.editor.document());
    assert!(graph.hears_main.contains(&resampler));
}

// Spec: routing R5, R14
#[test]
fn a_loop_plays_without_hanging_and_the_routing_that_closes_it_is_silent() {
    let mut rig = Rig::new(256);
    let ed = &mut rig.editor;
    let b = track(ed, TrackKind::Audio, None);
    let a = track(ed, TrackKind::Audio, None);
    dc(ed, a, 0.5);
    dc(ed, b, 0.25);
    monitor(ed, a, "in");
    monitor(ed, b, "in");
    // A's output into B, and A's input from B: a loop, which Live allows.
    output_to(ed, a, b);
    run(
        ed,
        "routing.set_input",
        json!({ "track": a, "input": { "type": "track", "track": b } }),
    );
    rig.sync();
    let graph = Graph::of(rig.editor.document());
    assert_eq!(graph.broken, vec![(b, a)], "the input closes the loop");
    // B carries A and itself to Main; A's input from B is silent.
    assert_eq!(settled(&mut rig), 0.75);

    // A group routed into one of its own tracks: the group's output loses.
    let ed = &mut rig.editor;
    let group = track(ed, TrackKind::Group, None);
    let inner = track(ed, TrackKind::Audio, Some(group));
    monitor(ed, inner, "in");
    output_to(ed, group, inner);
    rig.sync();
    let graph = Graph::of(rig.editor.document());
    assert!(graph.broken.contains(&(group, inner)));
    assert_eq!(settled(&mut rig), 0.75);
}

// Spec: routing R9 (IMP-01)
#[test]
fn fifty_return_tracks_play() {
    let mut rig = Rig::new(256);
    let ed = &mut rig.editor;
    let returns: Vec<TrackId> = (0..50)
        .map(|_| track(ed, TrackKind::Return, None))
        .collect();
    let a = track(ed, TrackKind::Audio, None);
    dc(ed, a, 1.0);
    run(
        ed,
        "routing.set_output",
        json!({ "track": a, "output": { "type": "sends_only" } }),
    );
    for &to in &returns {
        run(
            ed,
            "mixer.set_send",
            json!({ "track": a, "to": to, "level": 1.0 / 64.0 }),
        );
    }
    rig.sync();
    assert_eq!(settled(&mut rig), 50.0 / 64.0);
    assert_eq!(rig.editor.document().returns().len(), 50);
}

// Spec: routing R15
#[test]
fn solo_hears_the_tracks_routed_into_a_soloed_track() {
    let mut rig = Rig::new(256);
    let ed = &mut rig.editor;
    let other = track(ed, TrackKind::Audio, None);
    let tapper = track(ed, TrackKind::Audio, None);
    let b = track(ed, TrackKind::Audio, None);
    let a = track(ed, TrackKind::Audio, None);
    dc(ed, a, 0.5);
    dc(ed, b, 0.25);
    dc(ed, other, 0.125);
    monitor(ed, b, "in");
    output_to(ed, a, b);
    // The tapper takes A after its mixer: solo silences that with A.
    monitor(ed, tapper, "in");
    run(
        ed,
        "routing.set_input",
        json!({ "track": tapper, "input": { "type": "track", "track": a } }),
    );
    rig.sync();
    assert_eq!(settled(&mut rig), 0.75 + 0.5 + 0.125);

    // Soloing B still plays A, routed into it.
    run(
        &mut rig.editor,
        "mixer.solo_exclusive",
        json!({ "track": b }),
    );
    assert!(!rig.sync().program);
    assert_eq!(settled(&mut rig), 0.75);
    // Soloing A plays A, through B, and nothing else.
    run(
        &mut rig.editor,
        "mixer.solo_exclusive",
        json!({ "track": a }),
    );
    rig.sync();
    assert_eq!(settled(&mut rig), 0.75);
    // Soloing the tapper: A's Post Mixer signal goes quiet with A's output.
    run(
        &mut rig.editor,
        "mixer.solo_exclusive",
        json!({ "track": tapper }),
    );
    rig.sync();
    assert_eq!(settled(&mut rig), 0.0);
    // Taken before A's devices instead, it's heard.
    run(
        &mut rig.editor,
        "routing.set_input",
        json!({ "track": tapper, "input": { "type": "track", "track": a, "tap": "post_fx" } }),
    );
    rig.sync();
    assert_eq!(settled(&mut rig), 0.5);
}

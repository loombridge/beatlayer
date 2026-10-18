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
use bl_engine::testing::{GAIN, GAIN_PARAM};
use bl_model::{ChainId, Command, Editor, RackKind, TrackId, TrackKind, registry};
use common::{Rig, dc, effect_in, rack, rack_in, track};
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

/// A source track of 1.0 into Main with an audio effect rack after it: a
/// chain of a gain of 0.5 at chain volume 0.5 (Post FX 0.5, Post Mixer
/// 0.25), and an empty chain at volume 0 that adds nothing. A tapping track
/// of no devices that monitors its input, into Main too.
fn tapped() -> (Rig, TrackId, TrackId, Vec<ChainId>) {
    let mut rig = Rig::new(256);
    let ed = &mut rig.editor;
    // The tapper is made last, so it comes first in display order.
    let source = track(ed, TrackKind::Audio, None);
    let tapper = track(ed, TrackKind::Audio, None);
    dc(ed, source, 1.0);
    let (_, chains) = rack(ed, source, RackKind::AudioEffect, 2);
    effect_in(ed, chains[0], GAIN, &[(GAIN_PARAM, 0.5)]);
    ed.apply(Command::SetChainVolume {
        chain: chains[0],
        volume: 0.5,
    })
    .unwrap();
    ed.apply(Command::SetChainVolume {
        chain: chains[1],
        volume: 0.0,
    })
    .unwrap();
    monitor(ed, tapper, "in");
    (rig, source, tapper, chains)
}

fn tap_chain(rig: &mut Rig, tapper: TrackId, source: TrackId, chain: ChainId, tap: &str) {
    run(
        &mut rig.editor,
        "routing.set_input",
        json!({ "track": tapper, "input":
                { "type": "track", "track": source, "tap": tap, "chain": chain } }),
    );
    rig.sync();
}

// Spec: racks R38, R40. Each tap carries its chain's signal at its point,
// at any block size, and taking one leaves the rack's output as it was.
#[test]
fn chain_taps_carry_the_signal_at_their_point() {
    for size in [1, 64, 256] {
        let (mut rig, source, tapper, chains) = tapped();
        rig.sync();
        let alone = settled_at(&mut rig, size);
        assert_eq!(alone, 0.25, "the rack's sum, with nothing tapping it");
        for (tap, heard) in [("pre_fx", 1.0), ("post_fx", 0.5), ("post_mixer", 0.25)] {
            tap_chain(&mut rig, tapper, source, chains[0], tap);
            assert_eq!(
                settled_at(&mut rig, size),
                alone + heard,
                "{tap}, block size {size}"
            );
            let meter = rig.monitor.state().meter(source).unwrap();
            assert_eq!(
                meter,
                [alone, alone],
                "{tap}: the rack's output is unchanged"
            );
        }
        // The other chain, at volume 0: Post FX is its devices' output, Post
        // Mixer is silent.
        tap_chain(&mut rig, tapper, source, chains[1], "post_fx");
        assert_eq!(settled_at(&mut rig, size), alone + 1.0, "an empty chain");
        tap_chain(&mut rig, tapper, source, chains[1], "post_mixer");
        assert_eq!(settled_at(&mut rig, size), alone, "after a mixer at −∞ dB");
    }
}

fn settled_at(rig: &mut Rig, size: usize) -> f32 {
    rig.render(SETTLE, size);
    let last = rig.last();
    assert!(last.iter().all(|&s| s == last[0]), "a steady level");
    last[0]
}

// Spec: racks R38. A rack inside a chain offers its chains too, and a
// switched-off rack carries nothing to a route that taps its chains.
#[test]
fn a_nested_racks_chain_is_tapped_and_a_rack_that_is_off_is_silent() {
    let (mut rig, source, tapper, chains) = tapped();
    let (nested, inner) = rack_in(&mut rig.editor, chains[0], RackKind::AudioEffect, 1);
    effect_in(&mut rig.editor, inner[0], GAIN, &[(GAIN_PARAM, 0.5)]);
    // The outer chain's gain leaves 0.5 at the inner rack, and its gain
    // 0.25 after; the rack's sum is that at the outer chain's volume.
    let alone = 0.125;
    tap_chain(&mut rig, tapper, source, inner[0], "post_fx");
    assert_eq!(
        settled_at(&mut rig, 64),
        alone + 0.25,
        "after the inner gain"
    );
    tap_chain(&mut rig, tapper, source, inner[0], "pre_fx");
    assert_eq!(
        settled_at(&mut rig, 64),
        alone + 0.5,
        "at the inner rack's input"
    );

    rig.apply(Command::SetDeviceEnabled {
        device: nested,
        enabled: false,
    });
    assert_eq!(
        settled_at(&mut rig, 64),
        0.25,
        "the inner rack passes its input on, and its chains tap silence"
    );
}

// Spec: racks R40. Solo silences a track's Post Mixer tap with its output,
// but never a chain's taps, which come before the track's mixer.
#[test]
fn solo_never_silences_a_chain_tap() {
    let (mut rig, source, tapper, chains) = tapped();
    for (tap, heard) in [("pre_fx", 1.0), ("post_fx", 0.5), ("post_mixer", 0.25)] {
        tap_chain(&mut rig, tapper, source, chains[0], tap);
        run(
            &mut rig.editor,
            "mixer.solo_exclusive",
            json!({ "track": tapper }),
        );
        rig.sync();
        assert_eq!(
            settled(&mut rig),
            heard,
            "{tap}: the source is quiet, the tap isn't"
        );
    }
    // Contrast: the source's own Post Mixer is silenced with it.
    run(
        &mut rig.editor,
        "routing.set_input",
        json!({ "track": tapper, "input": { "type": "track", "track": source } }),
    );
    rig.sync();
    assert_eq!(settled(&mut rig), 0.0);
}

// Spec: racks R40, routing R14. The tapped track runs first, whatever the
// display order, and a chain tap that closes a loop carries nothing.
#[test]
fn a_chain_tap_runs_after_the_tapped_track_and_a_loop_is_silent() {
    let (mut rig, source, tapper, chains) = tapped();
    tap_chain(&mut rig, tapper, source, chains[0], "post_fx");
    // The tapper comes first in display order: were it to run first, it
    // would hear the source's last block, and the first has none.
    rig.render(64, 64);
    assert_eq!(
        rig.last()[0],
        0.25 + 0.5,
        "the tap is heard in the same block"
    );
    // The source takes the tapper's output: the tap would close a loop.
    monitor(&mut rig.editor, source, "in");
    run(
        &mut rig.editor,
        "routing.set_input",
        json!({ "track": source, "input": { "type": "track", "track": tapper } }),
    );
    rig.sync();
    let graph = Graph::of(rig.editor.document());
    assert_eq!(graph.broken.len(), 1, "one of the two routes is broken");
}

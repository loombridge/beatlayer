//! WP-5.45's acceptance in the engine: an instrument's extra outputs as
//! routing sources (the routing spec).
//!
//! - A track that takes one carries exactly that output's signal, at any
//!   block size, while the instrument's main output plays on down its own
//!   chain.
//! - Solo never silences the route, and it lines up at Main with everything
//!   else, held back for delay compensation as every route is.
//! - A switched-off instrument renders silence there.
//!
//! Every block runs as a real-time section under the allocation trap.

use crate::common;
use bl_commands::Registry as Commands;
use bl_engine::routing::Graph;
use bl_engine::testing::{DELAY, DELAY_FRAMES, MULTI, VALUE};
use bl_model::{
    Command, Device, DeviceId, DeviceKind, DeviceRole, Editor, TrackId, TrackKind, registry,
};
use common::{Rig, dc, track};
use serde_json::{Value, json};

/// Runs a document command by ID, as an agent does.
fn run(editor: &mut Editor, id: &str, args: Value) {
    let mut commands = Commands::new();
    registry::register(&mut commands).unwrap();
    let spec = commands.get(id).unwrap();
    bl_commands::validate(spec.args.as_ref().unwrap(), &args).unwrap();
    registry::run(editor, id, args).unwrap_or_else(|e| panic!("{id}: {e}"));
}

/// The latency the test device adds.
const LATENCY: usize = 500;

/// Adds the multi-output test instrument, at `value`, at the end of a MIDI
/// track's chain.
fn multi(editor: &mut Editor, track: TrackId, value: f64) -> DeviceId {
    let id = editor.new_id();
    let mut device = Device::new(
        id,
        "Test Multi",
        DeviceKind::Native {
            device_type: MULTI.to_owned(),
        },
    )
    .with_role(DeviceRole::Instrument);
    device.params.insert(VALUE, value);
    let index = editor.document().track(track).unwrap().devices().len();
    editor
        .apply(Command::InsertDevice {
            track,
            index,
            device,
        })
        .unwrap();
    id
}

fn monitor(editor: &mut Editor, track: TrackId, monitor: &str) {
    run(
        editor,
        "routing.set_monitor",
        json!({ "track": track, "monitor": monitor }),
    );
}

/// A MIDI track with the multi-output instrument at `value`, going to Main
/// as a MIDI track does, so what Main carries includes its main output.
fn source(rig: &mut Rig, value: f64) -> (TrackId, DeviceId) {
    let (source, device) = {
        let ed = &mut rig.editor;
        let source = track(ed, TrackKind::Midi, None);
        let device = multi(ed, source, value);
        (source, device)
    };
    rig.sync();
    (source, device)
}

/// An audio track, monitoring, that takes `source`'s instrument output.
fn tap_output(rig: &mut Rig, source: TrackId, device: DeviceId, output: u32) -> TrackId {
    let tapper = {
        let ed = &mut rig.editor;
        let tapper = track(ed, TrackKind::Audio, None);
        monitor(ed, tapper, "in");
        run(
            ed,
            "routing.set_input",
            json!({ "track": tapper, "input":
                    { "type": "track", "track": source,
                      "output": { "device": device, "output": output } } }),
        );
        tapper
    };
    rig.sync();
    tapper
}

/// Sets a tapper to take `source` at `tap` instead.
fn tap_track(rig: &mut Rig, tapper: TrackId, source: TrackId, tap: &str) {
    {
        let ed = &mut rig.editor;
        run(
            ed,
            "routing.set_input",
            json!({ "track": tapper, "input":
                    { "type": "track", "track": source, "tap": tap } }),
        );
    }
    rig.sync();
}

/// The level a block of `size` frames settles at, all of it the same.
fn settled(rig: &mut Rig, size: usize) -> f32 {
    // A settle pass, so a 20 ms glide has finished, then one block read.
    rig.render(1280, size);
    let out = rig.render(size.max(64), size);
    let last = *out.last().unwrap();
    assert!(
        out.iter().all(|&s| (s - last).abs() < 1e-6),
        "a steady level at block size {size}: {out:?}"
    );
    last
}

// Spec: routing R12, R16. A track that takes an instrument's extra output
// carries that output's signal alone, at any block size, while the
// instrument's main output goes on down its own chain.
#[test]
fn a_track_takes_an_instruments_extra_output_while_the_main_plays_on() {
    for size in [1, 64, 256, 4096] {
        let mut rig = Rig::new(4096);
        let (source, multi) = source(&mut rig, 5.0);
        let first = tap_output(&mut rig, source, multi, 0);
        let second = tap_output(&mut rig, source, multi, 1);
        // Main carries the instrument's own output, 5, and the two routes,
        // `Output 2` at 7 and `Output 3` at 8, with nothing held back.
        assert_eq!(
            settled(&mut rig, size),
            5.0 + 7.0 + 8.0,
            "the main output, {first} and {second} at block size {size}"
        );
    }
}

// Spec: routing R2, R16. A track that takes an instrument output runs after
// the instrument's track, in whatever order the tracks are displayed.
#[test]
fn the_tapped_track_runs_after_the_instrument() {
    let mut rig = Rig::new(256);
    let (source, multi) = source(&mut rig, 5.0);
    // The tapper is made after the source, so it comes first in display
    // order: were it to run first, it would hear nothing in the first block.
    let tapper = tap_output(&mut rig, source, multi, 0);
    // Main carries the instrument's own 5 and the tapped 7.
    assert_eq!(settled(&mut rig, 64), 5.0 + 7.0, "heard in the same block");
    assert!(rig.editor.document().track(tapper).is_some());
}

// Spec: routing R15. Solo silences a track's own output, never a route from
// an instrument output, as it never silences a chain tap.
#[test]
fn solo_never_silences_an_instrument_output() {
    let mut rig = Rig::new(256);
    let (source, multi) = source(&mut rig, 5.0);
    let tapper = tap_output(&mut rig, source, multi, 0);
    {
        let ed = &mut rig.editor;
        run(ed, "mixer.solo_exclusive", json!({ "track": tapper }));
    }
    rig.sync();
    // Soloed, the source is out of the mix, and the tapped 7 alone.
    assert_eq!(settled(&mut rig, 64), 7.0, "the tapped output is heard");
    // Contrast: the source's own Post Mixer tap is silenced with it.
    tap_track(&mut rig, tapper, source, "post_mixer");
    assert_eq!(settled(&mut rig, 64), 0.0, "the source's output is");
}

// Spec: routing R12, delay-compensation R2. A route from an instrument
// output is a path into a track like any other: the track's other paths and
// Main hold it to the longest one, so everything lands together.
#[test]
fn an_instrument_output_lines_up_at_main() {
    let mut rig = Rig::new(64);
    let (source, multi) = source(&mut rig, 1.0);
    // A device after the instrument adds its latency to the instrument's
    // track, but not to the signal at the instrument's own output: a route
    // that treated the output like the end of the chain would hold the other
    // paths back by a latency the output doesn't have.
    let delay = {
        let ed = &mut rig.editor;
        let mut delay = Device::new(
            ed.new_id(),
            "",
            DeviceKind::Native {
                device_type: DELAY.to_owned(),
            },
        );
        delay.params.insert(DELAY_FRAMES, LATENCY as f64);
        delay
    };
    rig.apply(Command::InsertDevice {
        track: source,
        index: 1,
        device: delay,
    });
    let other = {
        let ed = &mut rig.editor;
        let other = track(ed, TrackKind::Audio, None);
        dc(ed, other, 3.0);
        other
    };
    rig.sync();
    let tapper = tap_output(&mut rig, source, multi, 0);
    assert!(
        rig.editor
            .document()
            .track(tapper)
            .is_some_and(|t| t.routing().monitor == bl_model::Monitor::In)
    );
    // The tap is 3 (1 + 0 + 2), and the instrument's own output is 1 behind
    // the delay the chain adds: with the route's latency worked out as the
    // signal at the instrument's output, everything lands together at
    // frame LATENCY, and nothing is heard before it.
    let out = rig.render(LATENCY + 256, 64);
    let silence = out[..LATENCY].iter().fold(0.0f32, |m, &s| m.max(s.abs()));
    assert!(silence < 1e-6, "nothing before the latency: {silence}");
    let after = out[LATENCY..]
        .iter()
        .fold(0.0f32, |m, &s| m.max((s - 7.0).abs()));
    assert!(after < 1e-6, "everything lands together after it: {after}");
    assert!(rig.editor.document().track(other).is_some());
}

// Spec: device-framework R32, routing R12. A switched-off instrument renders
// nothing, so a route from one of its outputs carries silence.
#[test]
fn a_switched_off_instrument_renders_silence_on_its_extra_outputs() {
    let mut rig = Rig::new(256);
    let (source, multi) = source(&mut rig, 5.0);
    let tapper = tap_output(&mut rig, source, multi, 0);
    assert_eq!(settled(&mut rig, 64), 5.0 + 7.0);
    rig.apply(Command::SetDeviceEnabled {
        device: multi,
        enabled: false,
    });
    assert!(rig.editor.document().track(tapper).is_some());
    assert_eq!(
        settled(&mut rig, 64),
        0.0,
        "switched off, its outputs are both silent"
    );
}

// Spec: routing R5, R14. A route from an instrument output is an edge of the
// graph like any other, so a loop through one is broken like any other.
#[test]
fn an_instrument_output_route_is_an_edge_of_the_graph() {
    let mut rig = Rig::new(256);
    let (source, multi) = source(&mut rig, 5.0);
    let tapper = tap_output(&mut rig, source, multi, 0);
    let graph = Graph::of(rig.editor.document());
    assert!(
        graph
            .edges
            .iter()
            .any(|edge| edge.output.is_some() && edge.from == source && edge.to == tapper),
        "the route is in the graph"
    );
}

// Spec: routing R16. An output index past the ones the instrument's type
// declares carries nothing, never another device's buffer: the track hears
// the instrument's own output alone.
#[test]
fn an_output_past_the_ones_the_instrument_declares_carries_nothing() {
    let mut rig = Rig::new(256);
    let (source, multi) = source(&mut rig, 5.0);
    let tapper = tap_output(&mut rig, source, multi, 0);
    assert_eq!(settled(&mut rig, 64), 5.0 + 7.0);
    // The instrument declares two extra outputs; a third is nothing.
    {
        let ed = &mut rig.editor;
        run(
            ed,
            "routing.set_input",
            json!({ "track": tapper, "input":
                    { "type": "track", "track": source,
                      "output": { "device": multi, "output": 2 } } }),
        );
    }
    rig.sync();
    assert_eq!(
        settled(&mut rig, 64),
        5.0,
        "Main keeps the instrument's own output, with nothing from the route"
    );
}

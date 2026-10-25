//! The MIDI meters (WP-6.69), headless: a MIDI track with no instrument and
//! no audio effect shows a MIDI meter in its Mixer strip and its
//! Arrangement header that lights to a note's velocity and falls dark, its
//! lowest dot blue for a per-note expression, and a level meter once an
//! instrument is added; the Device View draws a meter in every gap, MIDI
//! before the instrument and audio after it. The engine runs on the null
//! device, paced by hand.
//!
//! Spec: midi-meters R1 to R5, R7, R8.

use crate::common;
use bl_engine::testing;
use bl_engine::{ChainMeter, MeterOwner, MeterSite, PointReading};
use bl_model::{DeviceId, TrackId};
use bl_shell::mixer_view::MixerViewState;
use bl_shell::{Shell, mixer_view_commands as ids};
use common::{Harness, Node, Window};
use egui::accesskit::Role;
use egui::epaint::Shape;
use egui::{Color32, Rect, pos2};
use serde_json::{Value, json};

fn harness() -> Harness {
    let mut h = Harness::manual_with_title_bar(Window::Main);
    h.settle();
    h
}

fn first_track(shell: &Shell) -> TrackId {
    shell.editor().document().top_level().next().unwrap()
}

fn nodes(h: &mut Harness) -> Vec<Node> {
    Harness::nodes(&h.settle())
}

/// The nodes labelled `label` inside the group or row header labelled
/// `within`: below it in the tree, or within its bounds.
fn inside<'a>(nodes: &'a [Node], within: &str, label: &str) -> Vec<&'a Node> {
    let parents: Vec<_> = nodes.iter().filter(|n| n.label == within).collect();
    nodes
        .iter()
        .filter(|n| n.label == label)
        .filter(|n| {
            parents.iter().any(|parent| {
                Harness::descends_from(nodes, n.id, parent.id)
                    || (parent.role == Role::RowHeader
                        && n.bounds.is_some()
                        && (rect_of(parent).top()..rect_of(parent).bottom())
                            .contains(&rect_of(n).center().y))
            })
        })
        .collect()
}

fn rect_of(node: &Node) -> Rect {
    let b = node.bounds.expect("a node with bounds");
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

fn midi_point(velocity: f32, channel: f32, expression: f32) -> PointReading {
    PointReading::Midi {
        velocity,
        channel,
        expression,
    }
}

fn set(h: &mut Harness, owner: MeterOwner, at: MeterSite, reading: PointReading) {
    h.shell
        .set_chain_meters(&[ChainMeter { owner, at, reading }]);
}

/// How many dots of `colour` the last frame drew in `rect`.
fn dots(h: &mut Harness, rect: Rect, colour: Color32) -> usize {
    let output = h.settle();
    output
        .shapes
        .iter()
        .filter(|clipped| match &clipped.shape {
            Shape::Circle(circle) => circle.fill == colour && rect.contains(circle.center),
            _ => false,
        })
        .count()
}

fn add(h: &mut Harness, device: Value) -> DeviceId {
    let added = h
        .shell
        .run("device.add", json!({ "device": device }))
        .unwrap();
    h.settle();
    serde_json::from_value(added["device"].clone()).unwrap()
}

fn lift() -> Value {
    json!({
        "kind": bl_model::DeviceKind::Native { device_type: bl_devices::lift::LIFT.to_owned() },
        "name": "Lift",
        "role": bl_model::DeviceRole::MidiEffect,
    })
}

fn instrument() -> Value {
    let t = bl_devices::polysynth_type();
    json!({ "kind": t.kind, "name": t.name, "role": t.role })
}

fn through() -> Value {
    let t = testing::through_type();
    json!({ "kind": t.kind, "name": t.name, "role": t.role })
}

// Spec: midi-meters R1, R3, R8
#[test]
fn a_midi_track_with_no_devices_has_a_midi_meter_in_its_strip_and_header_that_falls_dark() {
    let mut h = harness();
    let track = first_track(&h.shell);
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    // The header, in the Arrangement.
    let all = nodes(&mut h);
    let header = inside(&all, "1 MIDI", "MIDI Activity");
    assert!(!header.is_empty(), "the header's MIDI meter");
    assert!(inside(&all, "1 MIDI", "Track Meter").is_empty());
    // The Session View's strip has one too.
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    let all = nodes(&mut h);
    assert!(
        !inside(&all, "1 MIDI", "MIDI Activity").is_empty(),
        "the strip's MIDI meter"
    );
    assert!(inside(&all, "1 MIDI", "Track Meter").is_empty());
    // A clip plays a note of velocity 100: the engine publishes it and the
    // meter says its velocity.
    let clip: bl_model::ClipId = h.shell.new_id();
    h.shell
        .run(
            "clip.create",
            json!({ "track": track, "clip": {
                "id": clip, "name": "Riff", "start": 0, "length": 705_600,
                "offset": 0, "mute": false,
                "data": { "midi": { "notes": [
                    { "key": 60, "velocity": 100, "start": 0, "length": 100_000 }
                ] } } } }),
        )
        .unwrap();
    h.shell.run("transport.play", json!({})).unwrap();
    let mut lit = 0.0f64;
    for _ in 0..300 {
        let all = nodes(&mut h);
        lit = inside(&all, "1 MIDI", "MIDI Activity")[0]
            .numeric
            .unwrap_or(0.0);
        if lit > 0.0 {
            break;
        }
    }
    assert!(lit > 0.0, "a played note lights the meter: {lit}");
    assert!(
        lit <= 100.0,
        "the meter says the velocity, at most 100: {lit}"
    );
    // 300 ms on, it is dark.
    for _ in 0..60 {
        h.frame(vec![]);
    }
    let all = nodes(&mut h);
    let meter = inside(&all, "1 MIDI", "MIDI Activity")[0];
    assert_eq!(meter.numeric, Some(0.0));
    assert_eq!(meter.value, "0");
}

// Spec: midi-meters R3, R4, R5, R7
#[test]
fn the_dots_light_to_the_velocity_and_the_lowest_dot_to_a_channel_message_or_an_expression() {
    let mut h = harness();
    let track = first_track(&h.shell);
    h.shell.run("audio.toggle_engine", json!({})).unwrap();
    h.settle();
    let owner = MeterOwner::Track(track);
    let c = h.theme.colors.clone();
    set(&mut h, owner, MeterSite::End, midi_point(0.5, 0.0, 0.0));
    let all = nodes(&mut h);
    let meter = inside(&all, "1 MIDI", "MIDI Activity")[0];
    let rect = rect_of(meter);
    assert_eq!(meter.value, "64", "velocity 0.5 of 127, rounded");
    let total = (rect.height() - 2.0).div_euclid(4.0) as usize;
    assert_eq!(dots(&mut h, rect, c.meter.0), total.div_ceil(2));
    // A louder note lights more; a quieter one fewer.
    set(&mut h, owner, MeterSite::End, midi_point(1.0, 0.0, 0.0));
    assert_eq!(dots(&mut h, rect, c.meter.0), total);
    set(&mut h, owner, MeterSite::End, midi_point(0.01, 0.0, 0.0));
    assert_eq!(dots(&mut h, rect, c.meter.0), 1);
    // A channel message lights the lowest dot alone.
    set(&mut h, owner, MeterSite::End, midi_point(0.0, 1.0, 0.0));
    assert_eq!(dots(&mut h, rect, c.meter.0), 1);
    assert_eq!(dots(&mut h, rect, c.meter_expression.0), 0);
    // A per-note expression makes it blue, over a channel message's colour.
    set(&mut h, owner, MeterSite::End, midi_point(0.0, 1.0, 1.0));
    assert_eq!(dots(&mut h, rect, c.meter_expression.0), 1);
    assert_eq!(dots(&mut h, rect, c.meter.0), 0);
    // Lit, the window redraws each frame; dark, it stops.
    let output = h.frame(vec![]);
    let delay = output
        .viewport_output
        .values()
        .next()
        .unwrap()
        .repaint_delay;
    assert_eq!(delay, std::time::Duration::ZERO);
    set(&mut h, owner, MeterSite::End, midi_point(0.0, 0.0, 0.0));
    h.frame(vec![]);
    let output = h.frame(vec![]);
    let delay = output
        .viewport_output
        .values()
        .next()
        .unwrap()
        .repaint_delay;
    assert!(delay > std::time::Duration::ZERO);
}

// Spec: midi-meters R1, R6
#[test]
fn an_instrument_switches_the_tracks_meter_to_a_level_meter_at_once() {
    let mut h = harness();
    let track = first_track(&h.shell);
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    h.shell.run("audio.toggle_engine", json!({})).unwrap();
    h.settle();
    set(
        &mut h,
        MeterOwner::Track(track),
        MeterSite::End,
        midi_point(1.0, 0.0, 0.0),
    );
    let kind = |h: &mut Harness| -> Value {
        let state: MixerViewState =
            serde_json::from_value(h.shell.run(ids::STATE, json!({})).unwrap()).unwrap();
        serde_json::to_value(&state.meters).unwrap()
    };
    h.shell.set_meters(&[bl_engine::TrackMeter {
        track,
        peak: [0.25; 2],
        rms: [0.0; 2],
        impact: 0.0,
        gain_reduction: 0.0,
    }]);
    assert_eq!(kind(&mut h)[0]["kind"], "midi");
    assert!(!inside(&nodes(&mut h), "1 MIDI", "MIDI Activity").is_empty());
    add(&mut h, instrument());
    assert_eq!(kind(&mut h)[0]["kind"], "level");
    let all = nodes(&mut h);
    assert!(inside(&all, "1 MIDI", "MIDI Activity").is_empty());
    assert!(!inside(&all, "1 MIDI", "Track Meter").is_empty());
}

// Spec: midi-meters R2, R8
#[test]
fn the_device_view_draws_a_meter_in_every_gap_midi_before_the_instrument_and_audio_after_it() {
    let mut h = harness();
    let track = first_track(&h.shell);
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let effect = add(&mut h, lift());
    let source = add(&mut h, instrument());
    let after = add(&mut h, through());
    let owner = MeterOwner::Track(track);
    let at = |device: DeviceId| MeterSite::Before(device);
    let silent = PointReading::Audio { peak: [0.0; 2] };
    h.shell.run("audio.toggle_engine", json!({})).unwrap();
    h.settle();
    h.shell.set_chain_meters(&[
        ChainMeter {
            owner,
            at: at(effect),
            reading: midi_point(0.0, 0.0, 0.0),
        },
        ChainMeter {
            owner,
            at: at(source),
            reading: midi_point(0.0, 0.0, 0.0),
        },
        ChainMeter {
            owner,
            at: at(after),
            reading: silent,
        },
        ChainMeter {
            owner,
            at: MeterSite::End,
            reading: PointReading::Audio { peak: [0.5; 2] },
        },
    ]);
    let all = nodes(&mut h);
    let view = all.iter().find(|n| n.label == "Device View").unwrap();
    let within = |label: &str| {
        all.iter()
            .filter(|n| n.label == label && Harness::descends_from(&all, n.id, view.id))
            .count()
    };
    assert_eq!(within("MIDI Activity"), 2, "before the first two devices");
    assert_eq!(
        within("Level Meter"),
        2,
        "before the audio effect, and the end"
    );
    // The agent reads the same, by ID.
    let state = h.shell.run("device_view.state", json!({})).unwrap();
    let meters = state["meters"].as_array().unwrap();
    let kinds: Vec<&str> = meters.iter().map(|m| m["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["midi", "midi", "level", "level"]);
    assert_eq!(meters[3]["before"], Value::Null);
    assert_eq!(meters[3]["peak"], json!([0.5, 0.5]));
    assert_eq!(meters[0]["before"], json!(effect));
}

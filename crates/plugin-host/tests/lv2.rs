//! LV2 hosting, part 1 (WP-5.39): the test plug-in (`bl-test-lv2`) found in
//! its bundle, instantiated, run block by block as real-time sections under
//! the allocation trap, and its state saved and restored. They run where
//! lilv is: on Linux always (a missing lilv fails them), elsewhere only when
//! `BL_TEST_LILV` names a lilv library.

use std::path::Path;

use bl_plugin_host::lv2::{Instance, Lv2Error, MidiEvent, PortKind, World};
use bl_test_lv2::{
    ECHO_TRANSPOSE, GAIN, GAIN_MAX, LAST_NOTE_PORT, LATENCY, PLUGIN_NAME, PLUGIN_URI,
};

const RATE: f64 = 48_000.0;
const MAX_FRAMES: usize = 4096;

/// A world that knows the test bundle, or `None` off Linux without lilv.
fn world() -> Option<World> {
    let world = match std::env::var_os("BL_TEST_LILV") {
        Some(library) => World::with_library(Path::new(&library)),
        None => World::new(),
    };
    match world {
        Ok(world) => {
            world
                .load_bundle(bl_test_lv2::bundle())
                .expect("the bundle loads");
            Some(world)
        }
        Err(Lv2Error::Unavailable) if !cfg!(target_os = "linux") => None,
        Err(e) => panic!("lilv should be here: {e}"),
    }
}

fn instance(world: &World) -> Instance {
    world
        .instantiate(PLUGIN_URI, RATE, MAX_FRAMES)
        .expect("the plug-in instantiates")
}

fn note(frame: u32, status: u8, key: u8, velocity: u8) -> MidiEvent {
    MidiEvent {
        frame,
        len: 3,
        data: [status, key, velocity],
    }
}

/// Runs one block of `input` on both channels, as a real-time section.
fn run(instance: &mut Instance, input: &[f32], midi: &[MidiEvent]) -> [Vec<f32>; 2] {
    let (mut left, mut right) = (vec![0.0; input.len()], vec![0.0; input.len()]);
    bl_rt::rt_test!({
        instance
            .run(
                input.len(),
                &[input, input],
                &mut [&mut left, &mut right],
                midi,
            )
            .expect("the block runs");
    });
    [left, right]
}

#[test]
fn the_plug_in_is_found_with_its_ports() {
    let Some(world) = world() else { return };
    let plugins = world.plugins();
    let found = plugins
        .iter()
        .find(|p| p.uri == PLUGIN_URI)
        .expect("the test plug-in");
    assert_eq!(found.name, PLUGIN_NAME);
    let kinds: Vec<_> = found
        .ports
        .iter()
        .map(|p| (p.symbol.as_str(), p.kind, p.input))
        .collect();
    assert_eq!(
        kinds,
        [
            ("in_l", PortKind::Audio, true),
            ("in_r", PortKind::Audio, true),
            ("out_l", PortKind::Audio, false),
            ("out_r", PortKind::Audio, false),
            ("gain", PortKind::Control, true),
            ("latency", PortKind::Control, false),
            ("last_note", PortKind::Control, false),
            ("midi_in", PortKind::Atom { midi: true }, true),
            ("midi_out", PortKind::Atom { midi: true }, false),
        ]
    );
    let gain = &found.ports[GAIN as usize];
    assert_eq!((gain.min, gain.max, gain.default), (0.0, GAIN_MAX, 1.0));
}

#[test]
fn blocks_of_every_size_are_run_with_the_gain_applied() {
    let Some(world) = world() else { return };
    let mut instance = instance(&world);
    instance.set_control(GAIN, 0.5);
    for frames in [1, 100, 512, 4096] {
        let input: Vec<f32> = (0..frames).map(|i| i as f32 / frames as f32).collect();
        let [left, right] = run(&mut instance, &input, &[]);
        let expected: Vec<f32> = input.iter().map(|x| x * 0.5).collect();
        assert_eq!(left, expected, "left, {frames} frames");
        assert_eq!(right, expected, "right, {frames} frames");
    }
}

#[test]
fn notes_go_in_and_come_back_an_octave_up_on_their_frames() {
    let Some(world) = world() else { return };
    let mut instance = instance(&world);
    let sent = [note(3, 0x90, 60, 100), note(70, 0x80, 60, 0)];
    run(&mut instance, &[0.0; 100], &sent);
    let up = |event: &MidiEvent| MidiEvent {
        data: [event.data[0], event.data[1] + ECHO_TRANSPOSE, event.data[2]],
        ..*event
    };
    let back: Vec<MidiEvent> = instance.midi_out().collect();
    assert_eq!(back, [up(&sent[0]), up(&sent[1])]);
    // A block with none sends none: the last block's notes don't linger.
    run(&mut instance, &[0.0; 100], &[]);
    assert_eq!(instance.midi_out().count(), 0);
}

#[test]
fn the_latency_port_is_read() {
    let Some(world) = world() else { return };
    let mut instance = instance(&world);
    run(&mut instance, &[0.0; 64], &[]);
    assert_eq!(instance.latency(), LATENCY);
}

#[test]
fn state_is_saved_and_restored_into_another_instance() {
    let Some(world) = world() else { return };
    let mut first = instance(&world);
    first.set_control(GAIN, 1.5);
    run(&mut first, &[0.0; 64], &[note(0, 0x90, 64, 90)]);
    let saved = first.save_state().expect("the state is saved");
    let mut second = instance(&world);
    assert_eq!(second.control(GAIN), 1.0);
    second.restore_state(&saved).expect("the state is restored");
    // The plug-in's own state shows on its output after a block.
    run(&mut second, &[0.0; 64], &[]);
    assert_eq!(second.control(GAIN), 1.5, "the control input");
    assert_eq!(
        second.control(LAST_NOTE_PORT),
        64.0,
        "the plug-in's own state"
    );
    assert!(matches!(
        second.restore_state("not a state"),
        Err(Lv2Error::Failed(_))
    ));
}

#[test]
fn what_cannot_run_is_refused_before_the_plug_in_runs() {
    let Some(world) = world() else { return };
    assert!(matches!(
        world.instantiate("urn:nothing", RATE, 64),
        Err(Lv2Error::Failed(_))
    ));
    let mut instance = world
        .instantiate(PLUGIN_URI, RATE, 64)
        .expect("the plug-in");
    let (mut left, mut right) = (vec![0.0; 65], vec![0.0; 65]);
    let input = [0.0; 65];
    assert_eq!(
        instance.run(65, &[&input, &input], &mut [&mut left, &mut right], &[]),
        Err(Lv2Error::BadBlock)
    );
    let many = vec![note(0, 0x90, 60, 100); 1000];
    assert_eq!(
        instance.run(64, &[&input, &input], &mut [&mut left, &mut right], &many),
        Err(Lv2Error::TooManyEvents)
    );
}

#[cfg(not(target_os = "linux"))]
#[test]
fn off_linux_lv2_is_unavailable() {
    assert!(matches!(World::new(), Err(Lv2Error::Unavailable)));
}

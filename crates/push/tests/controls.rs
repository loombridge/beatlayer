//! The controls (`docs/specs/push-2.md`, R6 to R12, R28 and R29): the table,
//! the encoders, the touch strip, the transport buttons and the footswitches.

use bl_core::midi::MidiSource;
use bl_core::time::{Bpm, TempoMap};
use bl_push::controls::{self, BUTTONS, LedKind};
use bl_push::sim::LIVE;
use bl_push::state::Snapshot;
use serde_json::json;

use crate::rig::Rig;

/// The buttons the surface runs itself: the rest reach the modes' content.
const CORE: [&str; 15] = [
    "tap_tempo", "metronome", "play", "undo", "double_loop", "setup", "scale", "user", "note",
    "session", "device", "browse", "mix", "clip", "shift",
];

fn is_core(name: &str) -> bool {
    CORE.contains(&name) || name.starts_with("lower_")
}

// Spec: push-2 R6. Every control in the table decodes: a pad's note with its
// velocity, an encoder's touch and turn, a button's press and release, and
// the surface lights the LED with the same number.
#[test]
fn every_control_in_the_table_decodes_and_lights_its_led_with_the_right_message() {
    let mut rig = Rig::new();
    rig.flags.lock().unwrap().light_all = true;
    rig.start();
    rig.tick();

    // The LEDs: what the device shows is what the surface wants, for every
    // pad and every button, through the note-on of a pad and the controller
    // of a button.
    let snapshot: Snapshot = rig.snapshot();
    for button in BUTTONS {
        assert_eq!(rig.sim.led(button.name), snapshot.led(button.name), "{}", button.name);
        if !is_core(button.name) && !button.name.starts_with("upper_") && !button.name.starts_with("scene_") {
            let expected = match button.led {
                LedKind::White => 127,
                LedKind::Rgb => 126,
            };
            assert_eq!(rig.sim.led(button.name).unwrap().colour, expected, "{}", button.name);
        }
    }
    for row in 0..8 {
        for column in 0..8 {
            let name = format!("pad_{row}_{column}");
            assert_eq!(rig.sim.led(&name).unwrap().colour, 125, "{name}");
            assert_eq!(rig.sim.led(&name), snapshot.led(&name));
        }
    }

    // The controls: every pad with its velocity.
    rig.heard.take();
    for row in 0..8 {
        for column in 0..8 {
            assert!(rig.sim.press_pad(row, column, 1 + row * 8 + column));
            assert!(rig.sim.release_pad(row, column));
        }
    }
    rig.advance(1);
    let heard = rig.heard.take();
    for row in 0..8u8 {
        for column in 0..8u8 {
            let velocity = 1 + row * 8 + column;
            assert!(heard.contains(&format!("Note: pad {row},{column} On({velocity})")), "{row},{column}");
            assert!(heard.contains(&format!("Note: pad {row},{column} Off")), "{row},{column}");
        }
    }

    // Every button that reaches a content: its press, and its tap.
    for button in BUTTONS.iter().filter(|b| !is_core(b.name)) {
        // A modifier or not, a button on its own is a tap.
        assert!(rig.sim.press(button.name));
        rig.advance(1);
        assert!(rig.sim.let_go(button.name));
        rig.advance(1);
        let heard = rig.heard.take();
        assert!(heard.contains(&format!("Device: {} Press", button.name)), "{}: {heard:?}", button.name);
        assert!(heard.contains(&format!("Device: {} Tap", button.name)), "{}: {heard:?}", button.name);
    }

    // The eight track encoders: touch, turn both ways, release.
    for index in 0..8u8 {
        assert!(rig.sim.touch(index));
        assert!(rig.sim.turn(index, 5));
        assert!(rig.sim.turn(index, -3));
        assert!(rig.sim.release(index));
    }
    rig.advance(1);
    let heard = rig.heard.take();
    for index in 0..8u8 {
        for what in ["Touch", "Steps(5)", "Steps(-3)", "Release"] {
            assert!(
                heard.contains(&format!("Device: encoder {index} {what} shift=false")),
                "{index} {what}"
            );
        }
    }
}

// Spec: push-2 R7. A tap is a release within 300 ms; the button acts on the
// release.
#[test]
fn a_button_that_acts_on_a_tap_acts_when_it_is_released() {
    let mut rig = Rig::new();
    rig.start();
    assert!(rig.sim.press("undo"));
    rig.advance(100);
    assert!(rig.commands().is_empty(), "nothing yet");
    assert!(rig.sim.let_go("undo"));
    rig.advance(1);
    assert_eq!(rig.commands(), ["edit.undo {}"]);
}

// Spec: push-2 R12. Play/Stop, Shift + Play/Stop, Metronome, Undo, Shift +
// Undo and Double Loop run the commands the app's own controls run.
#[test]
fn the_transport_buttons_run_their_commands() {
    let mut rig = Rig::new();
    rig.start();
    rig.tick();
    rig.tap("play");
    assert_eq!(rig.commands(), ["transport.play_stop {}"]);
    rig.tap("metronome");
    assert_eq!(rig.commands(), ["transport.set_metronome_enabled {\"enabled\":true}"]);
    rig.tap("undo");
    rig.tap("double_loop");
    assert_eq!(rig.commands(), ["edit.undo {}", "clip.duplicate_loop {}"]);

    // Shift + Play/Stop while stopped goes to 1.1.1; while playing it stops.
    assert!(rig.sim.press("shift"));
    rig.advance(1);
    rig.tap("play");
    rig.tap("undo");
    assert_eq!(rig.commands(), ["transport.return_to_start {}", "edit.redo {}"]);
    rig.model.transport.playing = true;
    rig.tick();
    rig.tap("play");
    assert_eq!(rig.commands(), ["transport.play_stop {}"]);
    assert!(rig.sim.let_go("shift"));
    rig.advance(1);
    assert!(rig.commands().is_empty(), "Shift used in a combination does nothing alone");

    // Tap Tempo taps at once.
    assert!(rig.sim.press("tap_tempo"));
    rig.advance(1);
    assert_eq!(rig.commands(), ["transport.tap_tempo {}"]);
    assert!(rig.sim.let_go("tap_tempo"));
    rig.advance(1);
    assert!(rig.commands().is_empty());
}

// Spec: push-2 R10. The master encoder sets Main's volume, and with Shift the
// Preview/Cue Volume; one touch is one gesture; the steps add up before the
// document shows them.
#[test]
fn the_master_encoder_sets_main_volume_and_with_shift_the_cue_volume() {
    let mut rig = Rig::new();
    rig.start();
    rig.tick();
    let main = rig.model.document.main_id();
    let start = bl_model::fader::fader_position(1.0);

    assert!(rig.sim.touch(controls::MASTER));
    assert!(rig.sim.turn(controls::MASTER, 1));
    assert!(rig.sim.turn(controls::MASTER, 4));
    assert!(rig.sim.release(controls::MASTER));
    rig.advance(1);
    let requests = rig.requests();
    let volumes: Vec<f64> = requests
        .iter()
        .filter_map(|r| r.command())
        .map(|c| {
            assert_eq!(c.id, "mixer.set_volume");
            assert_eq!(c.args["track"], json!(main));
            assert!(c.gesture.is_some_and(|g| !g.released), "held until the touch ends");
            c.args["volume"].as_f64().unwrap()
        })
        .collect();
    assert_eq!(volumes.len(), 2);
    for (volume, steps) in volumes.iter().zip([1.0, 5.0]) {
        let expected = bl_model::fader::fader_gain(start + steps / 200.0);
        assert!((volume - expected).abs() < 1e-9, "{volume} vs {expected}");
    }
    assert!(matches!(requests.last().unwrap().ask, bl_control::Ask::EndGesture(_)), "one touch, one undo step");

    // With Shift: the cue volume, and finer.
    assert!(rig.sim.press("shift"));
    rig.advance(1);
    assert!(rig.sim.touch(controls::MASTER));
    assert!(rig.sim.turn(controls::MASTER, 10));
    assert!(rig.sim.release(controls::MASTER));
    rig.advance(1);
    let commands = rig.commands();
    assert_eq!(commands.len(), 2);
    assert!(commands[0].starts_with("mixer.set_cue_volume "), "{commands:?}");
    let volume: serde_json::Value = serde_json::from_str(commands[0].trim_start_matches("mixer.set_cue_volume ")).unwrap();
    let expected = bl_model::fader::fader_gain(bl_model::fader::fader_position(1.0) + 10.0 / 2000.0);
    assert!((volume["volume"].as_f64().unwrap() - expected).abs() < 1e-9);
}

// Spec: push-2 R10. The tempo encoder moves 1 BPM a step, 0.1 with Shift,
// within 20 to 999.
#[test]
fn the_tempo_encoder_moves_one_bpm_a_step_and_a_tenth_with_shift_within_the_range() {
    let mut rig = Rig::new();
    rig.start();
    rig.tick();
    let constant = |bpm| serde_json::to_value(TempoMap::constant(Bpm::new(bpm).unwrap())).unwrap();
    let tempo_of = |command: &str| -> serde_json::Value {
        let args: serde_json::Value = serde_json::from_str(command.trim_start_matches("song.set_tempo ")).unwrap();
        args["tempo"].clone()
    };

    assert!(rig.sim.touch(controls::TEMPO));
    assert!(rig.sim.turn(controls::TEMPO, 3));
    assert!(rig.sim.turn(controls::TEMPO, -1));
    assert!(rig.sim.release(controls::TEMPO));
    rig.advance(1);
    let commands = rig.commands();
    assert_eq!(tempo_of(&commands[0]), constant(123.0));
    assert_eq!(tempo_of(&commands[1]), constant(122.0));
    assert!(commands[2].starts_with("end "));

    // The document has the new tempo by now.
    rig.model.tempo = Bpm::new(122.0).unwrap();
    rig.advance(600);
    assert!(rig.sim.press("shift"));
    rig.advance(1);
    assert!(rig.sim.turn(controls::TEMPO, 3));
    rig.advance(1);
    assert_eq!(tempo_of(&rig.commands()[0]), constant(122.3));
    assert!(rig.sim.let_go("shift"));

    // The ends of the range.
    rig.model.tempo = Bpm::new(998.0).unwrap();
    rig.tick();
    assert!(rig.sim.touch(controls::TEMPO));
    assert!(rig.sim.turn(controls::TEMPO, 5));
    rig.advance(1);
    assert_eq!(tempo_of(&rig.commands()[0]), constant(999.0));
    assert!(rig.snapshot().screen.notification.unwrap().contains("999.0 BPM"), "touching shows the tempo");
}

// Spec: push-2 R10, R19. The swing encoder sets the amount the preferences
// keep, and touching it shows the amount on the display.
#[test]
fn the_swing_encoder_sets_the_swing_amount_and_shows_it() {
    let mut rig = Rig::new();
    rig.start();
    rig.tick();
    let revision = rig.shared.revision();
    assert!(rig.sim.touch(controls::SWING));
    rig.advance(1);
    assert_eq!(rig.snapshot().screen.notification.as_deref(), Some("Swing 0 %"));
    assert!(rig.sim.turn(controls::SWING, 20));
    rig.advance(1);
    assert!((rig.shared.swing() - 0.1).abs() < 1e-12);
    assert!(rig.shared.revision() > revision, "the shell sees the change to save it");
    assert_eq!(rig.snapshot().screen.notification.as_deref(), Some("Swing 10 %"));
    // The notification goes after 1.5 s.
    rig.advance(1499);
    assert!(rig.snapshot().screen.notification.is_some());
    rig.advance(2);
    assert_eq!(rig.snapshot().screen.notification, None);
}

// Spec: push-2 R11. The touch strip's three configurations send their flags,
// and a touch, a position and a release reach the content.
#[test]
fn the_touch_strip_has_three_configurations_and_each_use_sends_its_flags() {
    let mut rig = Rig::new();
    rig.flags.lock().unwrap().hooks = true;
    rig.start();
    rig.tick();
    assert_eq!(rig.sim.device.strip_config, 0x68, "pitch bend to begin with");

    rig.tap("add_track");
    assert_eq!(rig.sim.device.strip_config, 0x0C);
    rig.tap("add_device");
    assert_eq!(rig.sim.device.strip_config, 0x03);
    rig.tap("repeat");
    assert_eq!(rig.sim.device.strip_config, 0x68);
    assert_eq!(rig.snapshot().strip, bl_push::content::StripUse::PitchBend);

    // As pitch bend, then as modulation.
    rig.heard.take();
    assert!(rig.sim.strip_touch());
    assert!(rig.sim.strip_move(0.5));
    assert!(rig.sim.strip_release());
    rig.tap("add_track");
    rig.heard.take();
    assert!(rig.sim.strip_move(1.0));
    rig.advance(1);
    let heard = rig.heard.take();
    assert!(heard.contains(&"Note: strip Position(1.0)".to_owned()), "{heard:?}");
}

// Spec: push-2 R11. In the range configuration the host lights the 31 LEDs
// with one sysex, and sends it again only when they change.
#[test]
fn the_range_configuration_has_the_surface_light_the_strip() {
    let mut rig = Rig::new();
    rig.flags.lock().unwrap().hooks = true;
    rig.start();
    rig.tick();
    rig.tap("add_device");
    let lit = rig.sim.sysex().iter().filter(|m| m.get(6) == Some(&bl_push::sysex::id::SET_TOUCH_STRIP_LEDS)).count();
    assert_eq!(lit, 1, "sent once on entering the range");
    rig.advance(20);
    let again = rig.sim.sysex().iter().filter(|m| m.get(6) == Some(&bl_push::sysex::id::SET_TOUCH_STRIP_LEDS)).count();
    assert_eq!(again, 1, "and not again while nothing changes");
}

// Spec: push-2 R9. Pressure reaches the content as channel pressure or
// polyphonic key pressure.
#[test]
fn a_pads_pressure_reaches_the_content() {
    let mut rig = Rig::new();
    rig.start();
    assert!(rig.sim.press_pad(1, 1, 90));
    assert!(rig.sim.pressure(1, 1, 70));
    assert!(rig.sim.channel_pressure(80));
    assert!(rig.sim.release_pad(1, 1));
    rig.advance(1);
    let heard = rig.heard.take();
    for expected in [
        "Note: pad 1,1 On(90)",
        "Note: pad 1,1 Pressure(70)",
        "Note: pad 1,1 ChannelPressure(80)",
        "Note: pad 1,1 Off",
    ] {
        assert!(heard.contains(&expected.to_owned()), "{expected}: {heard:?}");
    }
}

// Spec: push-2 R28. Footswitch 1 is a sustain pedal: its down and up reach the
// tracks as controller 64 with the values 127 and 0.
#[test]
fn footswitch_1_reaches_a_track_as_controller_64() {
    let mut rig = Rig::new();
    rig.start();
    assert!(rig.sim.footswitch(1, true));
    assert!(rig.sim.footswitch(1, false));
    rig.settle();
    let reached = rig.reached_tracks();
    let messages: Vec<Vec<u8>> = reached.iter().map(|e| e.message.bytes().to_vec()).collect();
    assert_eq!(messages, [vec![0xB0, 64, 127], vec![0xB0, 64, 0]]);
    assert!(reached.iter().all(|e| e.source == MidiSource::Port(bl_core::midi::PortId::from_name(LIVE))));
}

// Spec: push-2 R29. A tap of footswitch 2 acts as Record after 300 ms; two
// taps within 300 ms act as New instead.
#[test]
fn footswitch_2_taps_record_and_double_taps_new() {
    let mut rig = Rig::new();
    rig.start();
    rig.tick();
    rig.heard.take();
    assert!(rig.sim.footswitch(2, true));
    assert!(rig.sim.footswitch(2, false));
    rig.advance(299);
    assert!(rig.heard.take().is_empty(), "waits 300 ms before acting on a first tap");
    rig.advance(1);
    let heard = rig.heard.take();
    for gesture in ["Press", "Release", "Tap"] {
        assert!(heard.contains(&format!("Device: record {gesture}")), "{heard:?}");
    }

    // Two taps within 300 ms.
    assert!(rig.sim.footswitch(2, true));
    assert!(rig.sim.footswitch(2, false));
    rig.advance(250);
    assert!(rig.sim.footswitch(2, true));
    assert!(rig.sim.footswitch(2, false));
    rig.advance(400);
    let heard = rig.heard.take();
    assert!(heard.contains(&"Device: new Tap".to_owned()), "{heard:?}");
    assert!(!heard.iter().any(|h| h.contains("record")), "no Record before New: {heard:?}");
}

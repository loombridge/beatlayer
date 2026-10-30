//! LEDs, the clock and the brightness (`docs/specs/push-2.md`, R13 to R16 and
//! R18): what is sent when, and what the device shows.

use std::time::Duration;

use bl_model::Command;
use bl_push::leds::Led;
use bl_push::sysex;

use crate::rig::Rig;

// Spec: push-2 R13. Only the LEDs that changed are sent.
#[test]
fn only_changed_leds_are_sent() {
    let mut rig = Rig::new();
    rig.start();
    rig.tick();
    rig.sim.forget_sent();
    rig.tick();
    rig.tick();
    assert!(
        rig.sim.sent().is_empty(),
        "nothing changed, nothing sent: {:?}",
        rig.sim.sent()
    );

    rig.model.transport.playing = true;
    rig.tick();
    assert_eq!(
        rig.sim.sent(),
        [vec![0xB0, 85, 126]],
        "the Play button went green, and nothing else"
    );
    assert_eq!(rig.sim.led("play"), Some(Led::steady(126)));
}

// Spec: push-2 R13. A white LED is off when its button does nothing, dim when
// it does something and full while its function is active or it is held.
#[test]
fn a_white_led_is_off_dim_or_full() {
    let mut rig = Rig::new();
    rig.start();
    rig.tick();
    assert_eq!(
        rig.sim.led("undo").unwrap(),
        Led::steady(16),
        "dim: it does something"
    );
    assert_eq!(
        rig.sim.led("scale").unwrap(),
        Led::steady(0),
        "off: Scale does nothing here"
    );
    assert_eq!(
        rig.sim.led("octave_up").unwrap(),
        Led::steady(0),
        "off: no mode uses it yet"
    );
    rig.flags.lock().unwrap().scale = true;
    rig.tick();
    assert_eq!(rig.sim.led("scale").unwrap(), Led::steady(16));

    // Full while held.
    assert!(rig.sim.press("undo"));
    rig.advance(1);
    assert_eq!(rig.sim.led("undo").unwrap(), Led::steady(127));
    assert!(rig.sim.let_go("undo"));
    rig.advance(1);
    assert_eq!(rig.sim.led("undo").unwrap(), Led::steady(16));

    // Full while its function is active: the open Setup menu.
    rig.tap("setup");
    assert_eq!(rig.sim.led("setup").unwrap(), Led::steady(127));
}

// Spec: push-2 R13, R15. A button that switches something on pulses while it
// is on: its dim level to full over a quarter note (channel 9).
#[test]
fn a_switch_that_is_on_pulses() {
    let mut rig = Rig::new();
    rig.start();
    rig.tick();
    assert_eq!(rig.sim.led("metronome").unwrap(), Led::steady(16));
    rig.apply(Command::SetMetronomeEnabled { enabled: true });
    rig.tick();
    assert_eq!(rig.sim.led("metronome").unwrap(), Led::pulsing(16, 127));
    assert_eq!(rig.sim.led("metronome").unwrap().animation, Some((127, 9)));
}

// Spec: push-2 R18. A display button's LED follows its label: the track's
// colour dim, full while selected, off when there is no label.
#[test]
fn a_display_buttons_led_follows_its_label() {
    let mut rig = Rig::new();
    rig.start();
    let selected = rig.tracks[1];
    rig.model.selection.tracks = vec![selected];
    if let Some(ring) = rig.model.rows[0].ring.as_mut() {
        ring.tracks.truncate(3);
    }
    rig.tick();
    let colours = rig.shared.track_colours();
    let track_colour = |n: usize| {
        let swatch = rig.model.document.track(rig.tracks[n]).unwrap().colour();
        bl_model::swatch_rgb(&colours, swatch)
    };
    let dim = |[r, g, b]: [u8; 3]| {
        [
            (u16::from(r) * 35 / 100) as u8,
            (u16::from(g) * 35 / 100) as u8,
            (u16::from(b) * 35 / 100) as u8,
        ]
    };
    assert_eq!(rig.sim.colour("lower_1"), Some(dim(track_colour(0))));
    assert_eq!(
        rig.sim.colour("lower_2"),
        Some(track_colour(1)),
        "the selected one is full"
    );
    assert_eq!(rig.sim.colour("lower_3"), Some(dim(track_colour(2))));
    for name in ["lower_4", "lower_8", "upper_1", "upper_8"] {
        assert_eq!(
            rig.sim.led(name).unwrap().colour,
            0,
            "{name} has no label: off"
        );
    }
    let screen = rig.snapshot().screen;
    assert_eq!(screen.lower[1].text, "Track 2");
    assert!(screen.lower[1].selected && !screen.lower[0].selected);
    assert!(screen.lower[3].is_empty());
}

// Spec: push-2 R15. MIDI clock goes to the Live port, 24 to the quarter note
// at the song's tempo, each stamped with its moment, and Start whenever the
// transport starts; it carries on while the transport is stopped.
#[test]
fn the_clock_is_24_to_the_beat_stamped_with_start_at_the_transport() {
    let mut rig = Rig::new();
    rig.start();
    for _ in 0..100 {
        rig.advance(20);
    }
    let ticks = |rig: &Rig| -> Vec<std::time::Instant> {
        rig.sim
            .stamped()
            .into_iter()
            .filter(|(_, m)| m == &[0xF8])
            .map(|(at, _)| at)
            .collect()
    };
    let stamps = ticks(&rig);
    assert!(
        stamps.len() > 90,
        "the clock runs while the transport is stopped: {}",
        stamps.len()
    );
    // 120 BPM: a tick every 60 / (120 * 24) s.
    let interval = Duration::from_secs_f64(60.0 / (120.0 * 24.0));
    for pair in stamps.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(gap.abs_diff(interval) < Duration::from_micros(2), "{gap:?}");
    }
    let beat = Duration::from_secs_f64(0.5);
    let span = *stamps.last().unwrap() - stamps[0];
    let beats = span.as_secs_f64() / beat.as_secs_f64();
    let ticks_per_beat = (stamps.len() - 1) as f64 / beats;
    assert!(
        (ticks_per_beat - 24.0).abs() < 0.05,
        "{ticks_per_beat} ticks a beat"
    );
    assert!(
        !rig.sim.stamped().iter().any(|(_, m)| m == &[0xFA]),
        "no Start while stopped"
    );

    // Start with the transport.
    rig.model.transport.playing = true;
    rig.advance(20);
    let starts: Vec<_> = rig
        .sim
        .stamped()
        .into_iter()
        .filter(|(_, m)| m == &[0xFA])
        .collect();
    assert_eq!(starts.len(), 1);
    rig.advance(20);
    assert_eq!(
        rig.sim
            .stamped()
            .iter()
            .filter(|(_, m)| m == &[0xFA])
            .count(),
        1,
        "once"
    );

    // A faster song: a shorter gap.
    rig.model.tempo = bl_core::time::Bpm::new(150.0).unwrap();
    for _ in 0..10 {
        rig.advance(20);
    }
    let stamps = ticks(&rig);
    let last = stamps[stamps.len() - 1] - stamps[stamps.len() - 2];
    assert!(
        last.abs_diff(Duration::from_secs_f64(60.0 / (150.0 * 24.0))) < Duration::from_micros(2),
        "{last:?}"
    );
}

// Spec: push-2 R16, R31. LED Brightness and Display Light map to 0..127 and
// 0..255 and reach the device when they change.
#[test]
fn brightness_reaches_the_device_when_it_changes() {
    let mut rig = Rig::new();
    rig.start();
    rig.tick();
    let mut setup = rig.shared.setup();
    setup.display_light = 50;
    setup.led_brightness = 50;
    rig.shared.set_setup(setup);
    rig.tick();
    assert_eq!(rig.sim.device.display_brightness, 128);
    assert_eq!(rig.sim.device.led_brightness, 64);
    let brightness = rig
        .sim
        .sysex()
        .iter()
        .filter(|m| m.get(6) == Some(&sysex::id::SET_LED_BRIGHTNESS))
        .count();
    assert_eq!(brightness, 2, "once at the start and once for the change");
    setup.display_light = 0;
    setup.led_brightness = 100;
    rig.shared.set_setup(setup);
    rig.tick();
    assert_eq!(rig.sim.device.display_brightness, 0);
    assert_eq!(rig.sim.device.led_brightness, 127);
}

// Spec: push-2 R30. Changing the pad values sends the curve again, in eight
// messages.
#[test]
fn changing_the_pad_values_sends_the_curve_again() {
    let mut rig = Rig::new();
    rig.start();
    rig.tick();
    let curves = |rig: &Rig| {
        rig.sim
            .sysex()
            .iter()
            .filter(|m| m.get(6) == Some(&sysex::id::SET_VELOCITY_CURVE))
            .count()
    };
    assert_eq!(curves(&rig), 8);
    let mut setup = rig.shared.setup();
    setup.sensitivity = 8;
    rig.shared.set_setup(setup);
    rig.tick();
    assert_eq!(curves(&rig), 16);
    assert_eq!(rig.sim.device.curve, setup.curve());
    assert_ne!(rig.sim.device.curve, bl_push::curve::curve(5, 5, 5));
}

// Spec: push-2 R15. A host that stalled for an hour starts the clock afresh,
// rather than sending every tick it missed.
#[test]
fn a_long_stall_does_not_flood_the_port_with_clock() {
    let mut rig = Rig::new();
    rig.start();
    rig.advance(20);
    let before = rig.sim.stamped().len();
    rig.advance(3_600_000);
    let sent = rig.sim.stamped().len() - before;
    assert!(sent < 10, "{sent} ticks after an hour");
}

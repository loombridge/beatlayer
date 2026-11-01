//! The modes (`docs/specs/push-2.md`, R21 to R27, R31 to R34): taps and holds
//! of the mode buttons, Mix's toggle, the menus, the locks, the modifiers,
//! the lower buttons on the ring's tracks, the colour picker, Setup and User
//! Mode.

use bl_push::content::{DisplayMode, MixPage, PadMode};
use bl_push::state::{Command, Menu};
use bl_push::sysex::{self, MidiMode};
use serde_json::json;

use crate::rig::Rig;

fn started() -> Rig {
    let mut rig = Rig::new();
    rig.start();
    rig.tick();
    rig
}

// Spec: push-2 R21, R22. The surface starts in Note Mode with Device on the
// display, and each mode's button is full while it is active, the others dim.
#[test]
fn the_surface_starts_in_note_mode_with_device_and_lights_the_active_buttons() {
    let rig = started();
    let state = rig.snapshot();
    assert_eq!(
        (state.pad_mode, state.display_mode, state.mix_page),
        (PadMode::Note, DisplayMode::Device, MixPage::Track)
    );
    assert_eq!(rig.sim.led("note").unwrap().colour, 127);
    assert_eq!(rig.sim.led("session").unwrap().colour, 16);
    assert_eq!(rig.sim.led("device").unwrap().colour, 127);
    for dim in ["browse", "mix", "clip"] {
        assert_eq!(rig.sim.led(dim).unwrap().colour, 16, "{dim}");
    }
    // Until a package builds a mode's content it shows its name.
    assert_eq!(
        state.screen.title.as_deref(),
        None,
        "the recording content writes nothing"
    );
}

// Spec: push-2 R21. Tapping Session or Note switches; holding one in the
// other mode switches while it is held and back on release; the hold begins
// at 300 ms.
#[test]
fn a_tap_switches_the_pad_mode_and_a_hold_switches_it_while_held() {
    let mut rig = started();
    // A tap, released at 299 ms.
    assert!(rig.sim.press("session"));
    rig.advance(299);
    assert_eq!(rig.snapshot().pad_mode, PadMode::Note);
    assert!(rig.sim.let_go("session"));
    rig.advance(1);
    assert_eq!(rig.snapshot().pad_mode, PadMode::Session, "a tap stays");

    // A hold of Note in Session Mode: Note at 300 ms, Session again on release.
    assert!(rig.sim.press("note"));
    rig.advance(299);
    assert_eq!(rig.snapshot().pad_mode, PadMode::Session);
    rig.advance(1);
    assert_eq!(rig.snapshot().pad_mode, PadMode::Note, "while held");
    assert_eq!(rig.sim.led("note").unwrap().colour, 127);
    assert!(rig.sim.let_go("note"));
    rig.advance(1);
    assert_eq!(rig.snapshot().pad_mode, PadMode::Session, "back on release");
    assert_eq!(rig.sim.led("session").unwrap().colour, 127);
}

// Spec: push-2 R22. Tapping Device, Browse, Mix or Clip switches; Device, Mix
// and Clip are momentary when held; Browse has no hold.
#[test]
fn the_display_modes_switch_on_a_tap_and_are_momentary_when_held_except_browse() {
    let mut rig = started();
    for (name, mode) in [
        ("clip", DisplayMode::Clip),
        ("browse", DisplayMode::Browse),
        ("mix", DisplayMode::Mix),
        ("device", DisplayMode::Device),
    ] {
        rig.tap(name);
        assert_eq!(rig.snapshot().display_mode, mode, "tap {name}");
    }
    for (name, mode) in [("clip", DisplayMode::Clip), ("mix", DisplayMode::Mix)] {
        assert!(rig.sim.press(name));
        rig.advance(300);
        assert_eq!(rig.snapshot().display_mode, mode, "hold {name}");
        assert!(rig.sim.let_go(name));
        rig.advance(1);
        assert_eq!(
            rig.snapshot().display_mode,
            DisplayMode::Device,
            "back after {name}"
        );
    }
    assert!(rig.sim.press("browse"));
    rig.advance(500);
    assert_eq!(
        rig.snapshot().display_mode,
        DisplayMode::Device,
        "Browse has no hold"
    );
    assert!(rig.sim.let_go("browse"));
    rig.advance(1);
    assert_eq!(
        rig.snapshot().display_mode,
        DisplayMode::Device,
        "and a long press isn't a tap"
    );
}

// Spec: push-2 R22. Mix's second tap switches between Track Mix and Global
// Mix, Track Mix the first time.
#[test]
fn tapping_mix_in_mix_switches_between_track_mix_and_global_mix() {
    let mut rig = started();
    rig.tap("mix");
    assert_eq!(
        (rig.snapshot().display_mode, rig.snapshot().mix_page),
        (DisplayMode::Mix, MixPage::Track)
    );
    rig.tap("mix");
    assert_eq!(rig.snapshot().mix_page, MixPage::Global);
    rig.tap("mix");
    assert_eq!(rig.snapshot().mix_page, MixPage::Track);
    assert_eq!(rig.handle_state(), "Note, Track Mix");
}

// Spec: push-2 R23. Setup and Scale open menus that the same button, or a
// display mode's button, closes, leaving the display mode as it was; Scale
// only where the pads allow it.
#[test]
fn setup_and_scale_open_and_close_without_changing_the_mode() {
    let mut rig = started();
    rig.tap("setup");
    assert_eq!(rig.snapshot().menu, Some(Menu::Setup));
    rig.tap("setup");
    assert_eq!(rig.snapshot().menu, None);

    rig.tap("mix");
    rig.tap("setup");
    rig.tap("device");
    let state = rig.snapshot();
    assert_eq!(
        (state.menu, state.display_mode),
        (None, DisplayMode::Mix),
        "closed, mode as it was"
    );

    // Scale does nothing where the pads don't allow it, and its LED is off.
    rig.tap("scale");
    assert_eq!(rig.snapshot().menu, None);
    assert_eq!(rig.sim.led("scale").unwrap().colour, 0);
    rig.flags.lock().unwrap().scale = true;
    rig.tick();
    rig.tap("scale");
    assert_eq!(rig.snapshot().menu, Some(Menu::Scale));
    assert_eq!(rig.sim.led("scale").unwrap().colour, 127, "full while open");
    rig.tap("scale");
    assert_eq!(rig.snapshot().menu, None);
    // One menu at a time.
    rig.tap("setup");
    rig.tap("scale");
    assert_eq!(rig.snapshot().menu, Some(Menu::Scale));
}

// Spec: push-2 R24. Mute, Solo and Stop Clip held alone for 1 s lock on until
// pressed again.
#[test]
fn mute_solo_and_stop_clip_lock_after_a_second_alone() {
    let mut rig = started();
    for name in ["mute", "solo", "stop_clip"] {
        rig.heard.take();
        assert!(rig.sim.press(name));
        rig.advance(999);
        assert!(rig.snapshot().locked.is_empty(), "{name} at 999 ms");
        rig.advance(1);
        assert_eq!(rig.snapshot().locked, [name], "locked at 1 s");
        assert!(rig.heard.all().contains(&format!("Device: {name} Lock")));
        assert!(rig.sim.let_go(name));
        rig.advance(100);
        assert_eq!(rig.snapshot().locked, [name], "stays after release");
        assert!(
            rig.heard
                .all()
                .iter()
                .all(|h| !h.contains(&format!("{name} Tap"))),
            "a lock is no tap"
        );
        rig.tap(name);
        assert!(rig.snapshot().locked.is_empty(), "pressed again");
        assert!(rig.heard.all().contains(&format!("Device: {name} Unlock")));
    }
}

// Spec: push-2 R25. A control pressed with a modifier held is a combination:
// the modifier doesn't do its own action when released, and a modifier alone
// does it.
#[test]
fn a_modifier_used_in_a_combination_does_not_do_its_own_action() {
    let mut rig = started();
    rig.tap("delete");
    let heard = rig.heard.take();
    assert!(
        heard.contains(&"Device: delete Tap".to_owned()),
        "alone it acts: {heard:?}"
    );

    assert!(rig.sim.press("delete"));
    rig.advance(10);
    assert!(rig.sim.press_pad(0, 0, 100));
    assert!(rig.sim.release_pad(0, 0));
    rig.advance(10);
    assert!(rig.sim.let_go("delete"));
    rig.advance(10);
    let heard = rig.heard.take();
    assert!(
        heard.contains(&"Note: pad 0,0 On(100)".to_owned()),
        "the pad reaches the content: {heard:?}"
    );
    assert!(
        !heard.iter().any(|h| h.contains("delete Tap")),
        "Delete used with a pad doesn't act alone: {heard:?}"
    );

    // The same with a button, and with an encoder's touch.
    assert!(rig.sim.press("duplicate"));
    rig.advance(10);
    assert!(rig.sim.touch(0));
    assert!(rig.sim.release(0));
    assert!(rig.sim.let_go("duplicate"));
    rig.advance(10);
    assert!(!rig.heard.take().iter().any(|h| h.contains("duplicate Tap")));
}

// Spec: push-2 R25. A core button pressed with a modifier does its
// combination or nothing, never its own action.
#[test]
fn a_core_button_with_a_modifier_held_does_not_do_its_own_action() {
    let mut rig = started();
    assert!(rig.sim.press("delete"));
    rig.advance(10);
    rig.tap("undo");
    rig.tap("setup");
    assert!(rig.sim.let_go("delete"));
    rig.advance(10);
    assert!(rig.commands().is_empty());
    assert_eq!(rig.snapshot().menu, None);
}

// Spec: push-2 R24, R25. A locked Mute isn't held: Play/Stop still does its
// own action while it is locked.
#[test]
fn play_works_while_mute_is_locked() {
    let mut rig = started();
    assert!(rig.sim.press("mute"));
    rig.advance(1000);
    assert!(rig.sim.let_go("mute"));
    rig.advance(10);
    assert_eq!(rig.snapshot().locked, ["mute"]);
    rig.commands();
    rig.tap("play");
    assert_eq!(rig.commands(), ["transport.play_stop {}"]);
}

// Spec: push-2 R26. The lower display buttons stand for the ring's tracks: a
// press selects the track with `session.select` and in the Arrangement, and
// each is labelled with the track's name.
#[test]
fn the_lower_buttons_select_the_rings_tracks() {
    let mut rig = started();
    let screen = rig.snapshot().screen;
    for column in 0..8 {
        assert_eq!(screen.lower[column].text, format!("Track {}", column + 1));
    }
    rig.tap("lower_3");
    assert_eq!(
        rig.commands(),
        [
            format!("session.select {}", json!({ "track": rig.tracks[2] })),
            format!(
                "arrangement.select_tracks {}",
                json!({ "tracks": [rig.tracks[2]] })
            ),
        ]
    );
    rig.tap("lower_8");
    assert_eq!(
        rig.commands(),
        [
            format!("session.select {}", json!({ "track": rig.tracks[7] })),
            format!(
                "arrangement.select_tracks {}",
                json!({ "tracks": [rig.tracks[7]] })
            ),
        ]
    );

    // A mode that uses them itself gets them instead.
    rig.flags.lock().unwrap().claim_lower = true;
    rig.tick();
    rig.heard.take();
    rig.tap("lower_1");
    assert!(rig.commands().is_empty());
    assert!(
        rig.heard
            .take()
            .contains(&"Device: lower_1 Press".to_owned())
    );
}

// Spec: push-2 R27. Shift and a lower display button open the colour picker:
// the 28 pads of the outer ring show the first two rows of swatches clockwise
// from the bottom left, the inner pads are off, and the target's colour blinks.
// A tap on a ring pad colours the track as one command, and closes the picker.
#[test]
fn the_colour_picker_colours_a_track_with_one_command() {
    let mut rig = started();
    assert!(rig.sim.press("shift"));
    rig.advance(10);
    rig.tap("lower_2");
    assert!(rig.sim.let_go("shift"));
    rig.advance(10);
    assert!(rig.snapshot().picker);
    assert!(rig.commands().is_empty(), "opening it changes nothing");

    let colours = rig.shared.track_colours();
    // The swatches, clockwise from the bottom left.
    let ring = |n: u8| -> (u8, u8) {
        match n {
            0..=6 => (n, 0),
            7..=13 => (7, n - 7),
            14..=20 => (7 - (n - 14), 7),
            _ => (0, 7 - (n - 21)),
        }
    };
    let mut seen = std::collections::BTreeSet::new();
    for n in 0..28u8 {
        let (row, column) = ring(n);
        assert!(seen.insert((row, column)), "28 different pads");
        assert!(
            row == 0 || row == 7 || column == 0 || column == 7,
            "on the outer ring"
        );
        let name = format!("pad_{row}_{column}");
        let led = rig.sim.led(&name).unwrap();
        assert_eq!(
            rig.sim.device.palette[usize::from(led.colour)].0,
            bl_model::swatch_rgb(&colours, n),
            "swatch {n}"
        );
    }
    for row in 1..7 {
        for column in 1..7 {
            assert_eq!(
                rig.sim.led(&format!("pad_{row}_{column}")).unwrap().colour,
                0,
                "inner pad {row},{column}"
            );
        }
    }
    // The track's own swatch blinks.
    let current = rig.model.document.track(rig.tracks[1]).unwrap().colour();
    let (row, column) = ring(current);
    assert!(
        rig.sim
            .led(&format!("pad_{row}_{column}"))
            .unwrap()
            .animation
            .is_some(),
        "swatch {current} blinks"
    );

    // A tap on swatch 5.
    let (row, column) = ring(5);
    assert!(rig.sim.press_pad(row, column, 100));
    assert!(rig.sim.release_pad(row, column));
    rig.advance(10);
    assert_eq!(
        rig.commands(),
        [format!(
            "track.set_colour {}",
            json!({ "tracks": [rig.tracks[1]], "colour": 5 })
        )],
        "one command, one undo step"
    );
    assert!(!rig.snapshot().picker, "closed");
    assert!(
        rig.heard.all().iter().all(|h| !h.contains("pad")),
        "the pad went to the picker, not the content"
    );
}

// Spec: push-2 R27. A target a mode names gets the colour through its own
// command, and any button closes the picker without a change.
#[test]
fn the_picker_takes_any_target_and_any_button_closes_it() {
    let mut rig = Rig::new();
    rig.flags.lock().unwrap().hooks = true;
    rig.start();
    rig.tick();
    rig.tap("accent");
    assert!(rig.snapshot().picker);
    let (row, column) = (7, 3);
    assert!(rig.sim.press_pad(row, column, 100));
    rig.advance(10);
    assert_eq!(
        rig.commands(),
        [format!(
            "test.set_colour {}",
            json!({ "id": 7, "colour": 10 })
        )]
    );

    rig.tap("accent");
    assert!(rig.snapshot().picker);
    rig.tap("undo");
    assert!(!rig.snapshot().picker);
    assert!(
        rig.commands().is_empty(),
        "closed with no change, and the button did nothing"
    );
    // An inner pad changes nothing and the picker stays.
    rig.tap("accent");
    assert!(rig.sim.press_pad(3, 3, 100));
    rig.advance(10);
    assert!(rig.snapshot().picker);
    assert!(rig.commands().is_empty());
}

// Spec: push-2 R31. Setup shows Pad Sensitivity, Gain and Dynamics on
// encoders 1 to 3, the curve over columns 4 and 5, Display Light and LED
// Brightness on 6 and 7, and Workflow on 8; the values are changed by the
// encoders, one value for 10 steps and a percent for 2.
#[test]
fn the_setup_menu_shows_and_changes_its_values() {
    let mut rig = started();
    rig.tap("setup");
    let screen = rig.snapshot().screen;
    let names: Vec<&str> = screen.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Pad Sensitivity",
            "Pad Gain",
            "Pad Dynamics",
            "",
            "",
            "Display Light",
            "LED Brightness",
            "Workflow"
        ]
    );
    assert_eq!(screen.columns[0].value, "5");
    assert_eq!(screen.columns[5].value, "100 %");
    assert_eq!(screen.columns[7].value, "Scene");
    assert_eq!(
        screen.curve.as_ref().map(Vec::len),
        Some(128),
        "the curve over columns 4 and 5"
    );

    assert!(rig.sim.turn(0, 10));
    assert!(rig.sim.turn(1, -20));
    assert!(rig.sim.turn(2, 5));
    assert!(rig.sim.turn(5, -10));
    assert!(rig.sim.turn(6, -4));
    assert!(rig.sim.turn(7, 10));
    rig.advance(1);
    let setup = rig.shared.setup();
    assert_eq!((setup.sensitivity, setup.gain, setup.dynamics), (6, 3, 5));
    assert_eq!((setup.display_light, setup.led_brightness), (95, 98));
    assert_eq!(setup.workflow, bl_push::curve::Workflow::Clip);
    assert_eq!(
        rig.sim.device.curve,
        setup.curve(),
        "the device has the new curve"
    );
    assert_eq!(rig.sim.device.display_brightness, 242);
    // The content hears nothing of it.
    assert!(rig.heard.all().iter().all(|h| !h.contains("encoder")));
    // Shift: finer.
    assert!(rig.sim.press("shift"));
    rig.advance(1);
    assert!(rig.sim.turn(5, 20));
    rig.advance(1);
    assert_eq!(rig.shared.setup().display_light, 96);
}

// Spec: push-2 R33. Tapping User sets User mode: every LED goes out but User,
// which is full, and the display says so; the controls reach nothing; tapping
// User again sets Live mode and everything is sent again.
#[test]
fn user_mode_turns_the_leds_out_and_back() {
    let mut rig = Rig::new();
    rig.flags.lock().unwrap().light_all = true;
    rig.start();
    rig.tick();
    let before = rig.snapshot();
    assert!(rig.sim.led("record").unwrap().colour != 0);

    rig.tap("user");
    assert_eq!(rig.sim.device.mode, MidiMode::User);
    assert!(rig.sim.sysex().contains(&sysex::midi_mode(MidiMode::User)));
    assert_eq!(rig.sim.led("user").unwrap().colour, 127);
    for name in [
        "record", "play", "pad_0_0", "pad_7_7", "lower_1", "undo", "note",
    ] {
        assert_eq!(rig.sim.led(name).unwrap().colour, 0, "{name}");
    }
    let state = rig.snapshot();
    assert_eq!(state.midi_mode, MidiMode::User);
    assert_eq!(
        state.screen.title.as_deref(),
        Some("Push 2 is in User Mode")
    );
    assert_eq!(rig.handle_state(), "User Mode");

    // The controls now send on the User port, where the surface doesn't read.
    rig.heard.take();
    rig.sim.press("undo");
    rig.sim.press_pad(0, 0, 90);
    rig.advance(10);
    assert!(rig.heard.take().is_empty());
    assert!(rig.commands().is_empty());

    rig.tap("user");
    assert_eq!(rig.sim.device.mode, MidiMode::Live);
    rig.advance(1);
    let after = rig.snapshot();
    assert_eq!(
        (after.pad_mode, after.display_mode),
        (before.pad_mode, before.display_mode),
        "the modes it had"
    );
    for name in [
        "record", "play", "pad_0_0", "pad_7_7", "lower_1", "undo", "note",
    ] {
        assert_eq!(rig.sim.led(name), after.led(name), "{name} is back");
    }
    assert_ne!(rig.sim.led("pad_0_0").unwrap().colour, 0);
}

// Spec: push-2 R34. `push.set_mode` sets both modes as their buttons do.
#[test]
fn set_mode_sets_the_modes_as_the_buttons_do() {
    let mut rig = started();
    rig.tap("setup");
    rig.handle().send(Command::SetMode {
        pad: Some(PadMode::Session),
        display: Some((DisplayMode::Mix, MixPage::Global)),
    });
    rig.tick();
    let state = rig.snapshot();
    assert_eq!(
        (
            state.pad_mode,
            state.display_mode,
            state.mix_page,
            state.menu
        ),
        (PadMode::Session, DisplayMode::Mix, MixPage::Global, None)
    );
    assert!(rig.commands().is_empty(), "it changes nothing in the set");
}

impl Rig {
    /// The surface's own mode as `control_surface.list` reports it.
    fn handle_state(&self) -> String {
        let report = &self.surfaces.report(&self.ports)[0];
        report
            .state
            .as_ref()
            .and_then(|s| s.mode.clone())
            .unwrap_or_default()
    }
}

// Spec: push-2 R19, R20. The count-in bar flashes on each beat, a progress
// bar follows each playing clip under its track's label, and a touched
// encoder highlights its column.
#[test]
fn the_count_in_the_progress_bars_and_a_touched_encoder_show_on_the_display() {
    let mut rig = started();
    assert_eq!(rig.snapshot().screen.count_in, None);
    rig.model.transport.counting_in = true;
    rig.tick();
    assert_eq!(
        rig.snapshot().screen.count_in,
        Some(true),
        "lit on the beat"
    );
    rig.advance(300);
    assert_eq!(
        rig.snapshot().screen.count_in,
        Some(false),
        "dark between beats at 120 BPM"
    );
    rig.advance(200);
    assert_eq!(
        rig.snapshot().screen.count_in,
        Some(true),
        "lit again on the next beat"
    );
    rig.model.transport.counting_in = false;
    rig.tick();
    assert_eq!(rig.snapshot().screen.count_in, None);

    rig.model.progress = vec![(rig.tracks[2], 0.25)];
    rig.tick();
    let screen = rig.snapshot().screen;
    assert_eq!(screen.progress[2], Some(0.25));
    assert!(
        screen
            .progress
            .iter()
            .enumerate()
            .all(|(n, p)| n == 2 || p.is_none())
    );

    assert!(rig.sim.touch(3));
    rig.advance(1);
    assert!(rig.snapshot().screen.columns[3].touched);
    assert!(rig.sim.release(3));
    rig.advance(1);
    assert!(!rig.snapshot().screen.columns[3].touched);
}

// Spec: push-2 R21, R22. A mode whose content isn't built yet shows its name
// and its encoders control nothing.
#[test]
fn a_mode_without_content_shows_its_name_and_its_encoders_do_nothing() {
    let mut rig = Rig::placeholders();
    rig.start();
    rig.tick();
    assert_eq!(rig.snapshot().screen.title.as_deref(), Some("Device"));
    rig.tap("browse");
    assert_eq!(rig.snapshot().screen.title.as_deref(), Some("Browse"));
    rig.tap("mix");
    assert_eq!(rig.snapshot().screen.title.as_deref(), Some("Mix"));
    rig.tap("clip");
    assert_eq!(rig.snapshot().screen.title.as_deref(), Some("Clip"));
    assert!(rig.sim.touch(0));
    assert!(rig.sim.turn(0, 20));
    assert!(rig.sim.release(0));
    rig.advance(1);
    assert!(rig.commands().is_empty(), "the encoders control nothing");
}

// Spec: push-2 R32. Workflow is what the modes read to decide what Duplicate,
// New and the arrows do: the content is told it with every event.
#[test]
fn workflow_reaches_the_modes() {
    let mut rig = Rig::new();
    rig.flags.lock().unwrap().hooks = true;
    rig.start();
    rig.tick();
    rig.tap("duplicate");
    assert!(
        rig.heard
            .take()
            .contains(&"Device: workflow Scene".to_owned())
    );
    rig.tap("setup");
    assert!(rig.sim.turn(7, 10));
    rig.advance(1);
    rig.tap("setup");
    rig.tap("new");
    assert!(
        rig.heard
            .take()
            .contains(&"Device: workflow Clip".to_owned())
    );
}

//! The APC mini mk2 and the APC40 mkII (WP-7.9;
//! `docs/specs/supported-controllers.md`, R35 to R50): each controller is a
//! simulated device on the in-process test backend, with the engine paced by
//! hand, so every rule runs headless.

use bl_audio_io::NullControl;
use bl_control::grid::apc_nearest;
use bl_control::grid::sim_apc::{Apc, Motion, Pad};
use bl_midi_io::TestBackend;
use bl_model::{DeviceId, SceneId, TrackId, TrackKind};
use bl_shell::{Shell, testing};
use serde_json::{Value, json};

/// Palette entries the surfaces name.
const RED: u8 = 5;
const GREEN: u8 = 21;
const YELLOW: u8 = 13;
const BLUE: u8 = 45;

struct Rig {
    shell: Shell,
    control: NullControl,
    test: TestBackend,
    apc: Apc,
}

impl Rig {
    fn mini() -> Self {
        Self::with(Apc::mini)
    }

    fn apc40() -> Self {
        Self::with(Apc::apc40)
    }

    /// A set with a controller plugged in and recognised.
    fn with(make: fn(&TestBackend) -> Apc) -> Self {
        let test = TestBackend::new();
        Self::with_device(make(&test), test)
    }

    fn with_device(apc: Apc, test: TestBackend) -> Self {
        let (mut shell, control) = testing::rig();
        shell.use_midi_backend(Box::new(test.clone()));
        shell.start_audio();
        let mut rig = Self {
            shell,
            control,
            test,
            apc,
        };
        rig.plug_in();
        rig
    }

    /// Plugs the controller in and lets the host recognise it: the inquiry,
    /// its reply, the row filled and the surface started.
    fn plug_in(&mut self) {
        // An empty set, so a track is the column its test puts it in.
        let doc = self.shell.editor().document();
        let existing: Vec<TrackId> = doc.top_level().chain(doc.returns()).collect();
        for track in existing {
            self.run("track.delete", json!({ "track": track }));
        }
        self.apc.plug_in();
        self.shell.refresh_midi();
        for _ in 0..4 {
            self.apc.pump();
            self.shell.poll();
            self.shell.midi().surfaces().settle();
        }
        self.apc.pump();
        self.settle();
    }

    fn run(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .run(id, args)
            .unwrap_or_else(|error| panic!("{id}: {error}"))
    }

    fn invoke(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .invoke(id, args)
            .unwrap_or_else(|error| panic!("{id}: {error}"))
    }

    /// One poll, one tick of the surfaces, and what they sent applied to the
    /// simulated device.
    fn sync(&mut self) {
        self.shell.poll();
        self.shell.midi().surfaces().tick();
        self.shell.midi().surfaces().settle();
        self.apc.pump();
    }

    /// A message's commands run on the first poll and what they changed
    /// shows on the next tick.
    fn settle(&mut self) {
        self.sync();
        self.sync();
        self.sync();
    }

    fn pass(&mut self, millis: u64) {
        testing::advance(&self.control, &mut self.shell, millis);
        self.settle();
    }

    fn row(&mut self) -> Value {
        self.run("control_surface.list", json!({}))["rows"][0].clone()
    }

    fn mode(&mut self) -> String {
        self.row()["surface_state"]["mode"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    fn ring_at(&mut self) -> (u64, u64) {
        let ring = self.row()["surface_state"]["ring"].clone();
        (
            ring["track_index"].as_u64().unwrap(),
            ring["scene_index"].as_u64().unwrap(),
        )
    }

    fn tracks(&mut self, count: usize) -> Vec<TrackId> {
        for n in 0..count {
            self.invoke(
                "track.create",
                json!({ "kind": "midi", "name": format!("T{n}") }),
            );
        }
        let doc = self.shell.editor().document();
        doc.top_level()
            .filter(|t| doc.track(*t).is_some_and(|t| t.kind() == TrackKind::Midi))
            .collect()
    }

    fn returns(&mut self, count: usize) {
        for n in 0..count {
            self.invoke(
                "track.create",
                json!({ "kind": "return", "name": format!("R{n}") }),
            );
        }
    }

    fn scenes(&self) -> Vec<SceneId> {
        self.shell
            .editor()
            .document()
            .scenes()
            .map(|s| s.id)
            .collect()
    }

    /// A MIDI clip in a slot, in colour swatch `colour`.
    fn clip(&mut self, track: TrackId, scene: SceneId, colour: u8) {
        self.run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        );
        let clip = self
            .shell
            .editor()
            .document()
            .slot(track, scene)
            .expect("the clip is there")
            .id;
        self.run(
            "clip.set_colour",
            json!({ "clips": [clip], "colour": colour }),
        );
    }

    fn mixer(&self, track: TrackId) -> bl_model::Mixer {
        *self.shell.editor().document().track(track).unwrap().mixer()
    }

    fn playing(&self, track: TrackId, scene: SceneId) -> bool {
        self.shell.session_states().iter().any(|s| {
            s.track == track && s.scene == scene && s.state == bl_engine::SlotState::Playing
        })
    }

    fn queued(&self, track: TrackId, scene: SceneId) -> bool {
        self.shell.session_states().iter().any(|s| {
            s.track == track && s.scene == scene && s.state == bl_engine::SlotState::Queued
        })
    }

    fn wait_playing(&mut self, track: TrackId, scene: SceneId) {
        testing::wait_for(&self.control, &mut self.shell, "the clip to play", |s| {
            s.session_states().iter().any(|st| {
                st.track == track && st.scene == scene && st.state == bl_engine::SlotState::Playing
            })
        });
        self.settle();
    }

    /// Lets the engine run until the transport plays, or stops.
    fn wait_transport(&mut self, playing: bool) {
        testing::wait_for(&self.control, &mut self.shell, "the transport", |s| {
            s.arrangement().transport().playing == playing
        });
        self.settle();
    }

    fn quantisation_off(&mut self) {
        self.run(
            "session.set_quantization",
            json!({ "quantization": "none" }),
        );
    }

    fn select(&mut self, track: TrackId) {
        self.invoke("arrangement.select_tracks", json!({ "tracks": [track] }));
        self.shell.set_focused_area(bl_shell::Area::Detail);
    }

    fn add(&mut self, device_type: &str, role: &str) -> DeviceId {
        let added: bl_shell::device_commands::DeviceAdded = serde_json::from_value(self.invoke(
            "device.add",
            json!({ "device": {
                "kind": { "native": { "device_type": device_type } },
                "name": device_type,
                "role": role,
            } }),
        ))
        .unwrap();
        added.device
    }

    // The controller's buttons.

    fn pad_tap(&mut self, column: u8, row: u8) {
        self.apc.pad_down(column, row);
        self.apc.pad_up(column, row);
        self.settle();
    }

    /// A button with no strip: press and release.
    fn tap(&mut self, note: u8) {
        self.apc.button_down(note);
        self.settle();
        self.apc.button_up(note);
        self.settle();
    }

    /// A button of track `column`'s strip on its channel.
    fn strip(&mut self, column: u8, note: u8) {
        self.apc.note_on(column, note);
        self.settle();
        self.apc.note_off(column, note);
        self.settle();
    }

    /// The first of two presses with Shift held (the APC40's 62h and the
    /// mini's 7Ah), and the release.
    fn shifted(&mut self, shift: u8, then: impl FnOnce(&mut Self)) {
        self.apc.button_down(shift);
        self.settle();
        then(self);
        self.apc.button_up(shift);
        self.settle();
    }

    fn cc(&mut self, channel: u8, controller: u8, value: u8) {
        self.apc.controller(channel, controller, value);
        self.settle();
    }
}

fn colour_of(swatch: u8) -> u8 {
    apc_nearest(bl_model::swatch_rgb(
        &bl_model::DEFAULT_TRACK_PALETTE,
        swatch,
    ))
}

fn version() -> [u8; 3] {
    [
        env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap(),
        env!("CARGO_PKG_VERSION_MINOR").parse().unwrap(),
        env!("CARGO_PKG_VERSION_PATCH").parse().unwrap(),
    ]
}

// Spec: supported-controllers R2, R3, R35, R72. Plugging the APC mini mk2 in
// fills the first free row with its first port, the introduction follows the
// inquiry, and the second port stays an ordinary MIDI port that plays tracks
// and fills no row of its own.
#[test]
fn plugging_the_apc_mini_in_fills_row_one_and_introduces_beatlayer() {
    let mut rig = Rig::mini();
    let row = rig.row();
    assert_eq!(row["surface"]["id"], "apc-mini-mk2");
    assert_eq!(row["input"]["name"], "APC mini mk2 Control");
    assert_eq!(row["output"]["name"], "APC mini mk2 Control");
    assert_eq!(row["state"], "running");
    let [major, minor, fix] = version();
    assert_eq!(
        &rig.apc.messages()[..2],
        [
            vec![0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7],
            vec![
                0xF0, 0x47, 0x7F, 0x4F, 0x60, 0x00, 0x04, 0x00, major, minor, fix, 0xF7
            ],
        ],
        "the inquiry, then the introduction"
    );
    // R3: the second port stays a port tracks hear, and fills no row.
    let notes = rig.apc.notes_port().unwrap();
    let ports = rig.shell.midi().ports();
    let port = ports
        .iter()
        .find(|p| p.name == notes && p.direction == bl_shell_spec::midi_io::PortDirection::Input)
        .expect("the second port is listed");
    assert!(port.track, "its Track switch stays on");
    assert!(rig.test.is_open(bl_midi_io::Direction::Input, notes));
    let second = rig.run("control_surface.list", json!({}))["rows"][1].clone();
    assert_eq!(second["state"], "empty", "{second}");
    assert_eq!(
        rig.mode(),
        "Session; track buttons: Clip Stop; faders: Volume"
    );
    assert_eq!(rig.row()["surface_state"]["ring"]["tracks"], 8);
    assert_eq!(rig.row()["surface_state"]["ring"]["scenes"], 8);
}

// Spec: supported-controllers R35, R39. The introduction's reply gives the
// faders' positions, so with Takeover Mode Pick Up the first message of a
// fader that crosses its parameter takes it; faders that rested elsewhere
// move nothing until they meet the value.
#[test]
fn the_introductions_reply_gives_takeover_mode_the_faders_positions() {
    for (rests, moved) in [(127, true), (0, false)] {
        let test = TestBackend::new();
        let apc = Apc::mini(&test).with_faders([rests; 9]);
        let mut rig = Rig::with_device(apc, test);
        rig.run("settings.set_takeover_mode", json!({ "mode": "pick_up" }));
        let track = rig.tracks(1)[0];
        rig.settle();
        let volume = rig.mixer(track).volume;
        // The fader moves to a low position: from the top it crosses the
        // parameter, from the bottom it does not.
        rig.apc.fader_to(0, 20);
        rig.settle();
        let now = rig.mixer(track).volume;
        if moved {
            assert!(now < volume, "{now} {volume}: picked up");
        } else {
            assert_eq!(now, volume, "nothing until it meets the value");
        }
    }
}

// Spec: supported-controllers R9, R37. Stopped clips show their colour at
// half brightness and playing ones at full, queued clips blink their colour
// at an eighth, recording ones pulse red at a quarter, and an empty slot of
// an armed track is red at a quarter.
#[test]
fn the_apc_minis_pads_show_slot_states_at_their_brightness_channels() {
    let mut rig = Rig::mini();
    let tracks = rig.tracks(4);
    let scenes = rig.scenes();
    rig.clip(tracks[0], scenes[0], 3);
    rig.clip(tracks[1], scenes[0], 8);
    rig.run(
        "session.set_quantization",
        json!({ "quantization": "1_bar" }),
    );
    rig.run("transport.play", json!({}));
    rig.settle();
    for (column, swatch) in [(0, 3), (1, 8)] {
        assert_eq!(
            rig.apc.pad(column, 0),
            Some(Pad::solid(colour_of(swatch), 50)),
            "stopped: its colour at 50 %"
        );
    }
    assert_eq!(rig.apc.pad(0, 1), None, "an empty slot is dark");
    assert_eq!(rig.apc.pad(7, 7), None, "past the last track");

    rig.pad_tap(0, 0);
    testing::advance(&rig.control, &mut rig.shell, 20);
    assert!(rig.queued(tracks[0], scenes[0]));
    rig.settle();
    assert_eq!(
        rig.apc.pad(0, 0),
        Some(Pad::blinking(colour_of(3), 8)),
        "queued: blinking at an eighth"
    );
    rig.wait_playing(tracks[0], scenes[0]);
    assert_eq!(
        rig.apc.pad(0, 0),
        Some(Pad::solid(colour_of(3), 100)),
        "playing: its colour at 100 %"
    );

    // An armed track's empty slot is red at a quarter; pressing it records.
    rig.quantisation_off();
    rig.run("mixer.set_arm", json!({ "track": tracks[2], "arm": true }));
    rig.settle();
    assert_eq!(rig.apc.pad(2, 1), Some(Pad::solid(RED, 25)));
    rig.pad_tap(2, 1);
    let track = tracks[2];
    testing::wait_for(&rig.control, &mut rig.shell, "the recording", |shell| {
        shell
            .session_states()
            .iter()
            .any(|s| s.track == track && s.state == bl_engine::SlotState::Recording)
    });
    rig.settle();
    assert_eq!(
        rig.apc.pad(2, 1),
        Some(Pad::pulsing(RED, 4)),
        "recording: pulsing at a quarter"
    );
}

// Spec: supported-controllers R36, R38. The scene buttons launch their
// scenes (lit while one of their clips plays); with Shift they choose what
// the track buttons do, Clip Stop, Solo, Mute, Rec Arm and Select, the
// chosen one lit while Shift is held, and each acts and lights as R13 says;
// scene button 8 stops every clip.
#[test]
fn shift_with_the_scene_buttons_chooses_the_track_buttons_function() {
    let mut rig = Rig::mini();
    let tracks = rig.tracks(2);
    let scenes = rig.scenes();
    rig.quantisation_off();
    rig.clip(tracks[0], scenes[0], 3);
    rig.clip(tracks[1], scenes[0], 8);
    rig.run("transport.play", json!({}));
    // A scene button launches its scene, and is lit while a clip plays.
    rig.tap(0x70);
    rig.wait_playing(tracks[0], scenes[0]);
    assert_eq!(rig.apc.scene_button(0), Some(1));
    assert_eq!(rig.apc.scene_button(1), None);

    // Clip Stop at first: lit for the playing track.
    assert_eq!(rig.apc.track_button(0), Some(1));
    assert_eq!(rig.apc.track_button(1), Some(1), "both clips play");
    assert_eq!(rig.apc.track_button(2), None, "no track there");
    rig.tap(0x64);
    testing::wait_for(&rig.control, &mut rig.shell, "the track to stop", |shell| {
        !shell.session_states().iter().any(|s| s.track == tracks[0])
    });
    rig.settle();
    assert_eq!(rig.apc.track_button(0), None, "dark: it doesn't play");

    // Shift shows what is chosen, and the buttons then choose.
    rig.shifted(0x7A, |rig| {
        for (row, lit) in (0..8).zip([true, false, false, false, false, false, false, false]) {
            assert_eq!(rig.apc.scene_button(row).is_some(), lit, "{row}");
        }
    });
    for (note, name) in [
        (0x71, "Solo"),
        (0x72, "Mute"),
        (0x73, "Rec Arm"),
        (0x74, "Select"),
        (0x70, "Clip Stop"),
    ] {
        rig.shifted(0x7A, |rig| {
            rig.tap(note);
            assert_eq!(rig.apc.scene_button(note - 0x70), Some(1));
        });
        assert_eq!(
            rig.mode(),
            format!("Session; track buttons: {name}; faders: Volume")
        );
    }
    // Solo, Mute, Rec Arm and Select act, and light as R13 says.
    let choose = |rig: &mut Rig, note| rig.shifted(0x7A, |rig| rig.tap(note));
    choose(&mut rig, 0x71);
    assert_eq!(rig.apc.track_button(0), None);
    rig.tap(0x64);
    assert!(rig.mixer(tracks[0]).solo);
    assert_eq!(rig.apc.track_button(0), Some(1), "lit while soloed");
    choose(&mut rig, 0x72);
    assert_eq!(rig.apc.track_button(1), Some(1), "lit while heard");
    rig.tap(0x65);
    assert!(rig.mixer(tracks[1]).mute);
    assert_eq!(rig.apc.track_button(1), None);
    choose(&mut rig, 0x73);
    rig.tap(0x65);
    assert!(rig.mixer(tracks[1]).arm);
    assert_eq!(rig.apc.track_button(1), Some(1), "lit while armed");
    choose(&mut rig, 0x74);
    rig.tap(0x65);
    assert_eq!(
        rig.shell.arrangement().selected_tracks().to_vec(),
        vec![tracks[1]]
    );
    assert_eq!(rig.apc.track_button(1), Some(1), "lit while selected");
    assert_eq!(rig.apc.track_button(0), None);

    // Shift with the last scene button stops every clip.
    assert!(rig.playing(tracks[1], scenes[0]));
    rig.shifted(0x7A, |rig| rig.tap(0x77));
    testing::wait_for(
        &rig.control,
        &mut rig.shell,
        "every clip to stop",
        |shell| shell.session_states().is_empty(),
    );
}

fn params(rig: &Rig, device: DeviceId) -> Vec<(u32, u64)> {
    let mut found: Vec<(u32, u64)> = rig
        .shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .params
        .iter()
        .map(|(id, value)| (id.0, value.to_bits()))
        .collect();
    found.sort_unstable();
    found
}

// Spec: supported-controllers R39. Holding Shift, track buttons 1 to 4 choose
// what the faders reach (Volume, Pan, Send, stepping through the sends, and
// the device in focus's bank), lit while Shift is held; track buttons 5 to 8
// move the ring; the ninth fader sets the Main track's volume in every mode.
#[test]
fn shift_with_the_track_buttons_chooses_what_the_faders_reach() {
    let mut rig = Rig::mini();
    let tracks = rig.tracks(10);
    rig.returns(2);
    // Two scenes more than the ring is tall.
    rig.invoke("session.insert_scene", json!({}));
    rig.invoke("session.insert_scene", json!({}));
    let doc = rig.shell.editor().document();
    let (main, first_return, second_return) = (
        doc.main_id(),
        doc.returns().next().unwrap(),
        doc.returns().nth(1).unwrap(),
    );
    rig.select(tracks[0]);
    let device = rig.add("beatlayer.utility", "audio_effect");
    rig.settle();
    assert_eq!(
        rig.mode(),
        "Session; track buttons: Clip Stop; faders: Volume"
    );

    // Volume: the first eight faders set the ring's tracks, the ninth Main.
    rig.apc.fader_to(1, 127);
    rig.apc.fader_to(8, 100);
    rig.settle();
    let top = rig.mixer(tracks[1]).volume;
    let main_volume = rig.mixer(main).volume;
    assert!(
        top > rig.mixer(tracks[0]).volume,
        "the second track's fader is up"
    );
    assert!(
        main_volume > 0.0 && main_volume < top,
        "the ninth is Main's"
    );

    // Pan, lit while Shift is held; the ninth fader still sets Main.
    rig.shifted(0x7A, |rig| {
        assert_eq!(rig.apc.track_button(0), Some(1), "Volume is chosen");
        rig.tap(0x65);
        assert_eq!(rig.apc.track_button(1), Some(1), "Pan is chosen");
        assert_eq!(rig.apc.track_button(0), None);
    });
    assert_eq!(rig.mode(), "Session; track buttons: Clip Stop; faders: Pan");
    rig.apc.fader_to(0, 127);
    rig.apc.fader_to(8, 20);
    rig.settle();
    assert!((rig.mixer(tracks[0]).pan - 1.0).abs() < 1e-9);
    assert!(rig.mixer(main).volume < main_volume, "Main in every mode");

    // Send A, then pressing again steps to Send B.
    rig.shifted(0x7A, |rig| rig.tap(0x66));
    assert!(rig.mode().ends_with("faders: Send"), "{}", rig.mode());
    rig.apc.fader_to(0, 127);
    rig.settle();
    let doc = rig.shell.editor().document();
    let send = |track, to| doc.track(track).unwrap().send(to);
    assert!(send(tracks[0], first_return) > 0.0);
    assert_eq!(send(tracks[0], second_return), 0.0);
    rig.shifted(0x7A, |rig| rig.tap(0x66));
    rig.apc.fader_to(0, 127);
    rig.settle();
    let doc = rig.shell.editor().document();
    assert!(
        doc.track(tracks[0]).unwrap().send(second_return) > 0.0,
        "Send B"
    );

    // Device: the device in focus's bank.
    rig.shifted(0x7A, |rig| rig.tap(0x67));
    assert!(rig.mode().ends_with("faders: Device"), "{}", rig.mode());
    let before = params(&rig, device);
    rig.apc.fader_to(0, 127);
    rig.settle();
    assert_ne!(params(&rig, device), before, "the device's first parameter");

    // Track buttons 5 to 8 with Shift move the ring: up, down, left, right.
    assert_eq!(rig.ring_at(), (0, 0));
    rig.shifted(0x7A, |rig| rig.tap(0x6B));
    assert_eq!(rig.ring_at(), (1, 0), "right");
    rig.shifted(0x7A, |rig| rig.tap(0x6A));
    assert_eq!(rig.ring_at(), (0, 0), "left");
    rig.shifted(0x7A, |rig| {
        assert!(rig.apc.track_button(5).is_some(), "the ring can move down");
        assert_eq!(rig.apc.track_button(4), None, "but not up");
        rig.tap(0x69);
    });
    assert_eq!(rig.ring_at(), (0, 1), "down");
}

fn pad_view(rig: &Rig, rack: DeviceId) -> (u8, Option<u8>) {
    fn find(
        frames: &[bl_shell::device_commands::DeviceFrame],
        id: DeviceId,
    ) -> Option<bl_shell::pad_commands::PadFrame> {
        frames.iter().find_map(|d| {
            if d.id == id {
                d.rack.as_ref().and_then(|r| r.pads.clone())
            } else {
                d.rack.as_ref().and_then(|r| find(&r.devices, id))
            }
        })
    }
    let frame = find(&rig.shell.device_view_state().devices, rack).expect("the rack shows");
    (frame.first, frame.selected)
}

/// A drum rack with a pad on each of `notes`, on the selected track.
fn drum_rack(rig: &mut Rig, notes: &[u8]) -> DeviceId {
    let added: bl_shell::device_commands::DeviceAdded = serde_json::from_value(rig.invoke(
        "device.add",
        json!({ "device": {
            "kind": { "rack": { "kind": "drum" } },
            "name": "Drum Rack",
            "role": "instrument",
        } }),
    ))
    .unwrap();
    for &note in notes {
        rig.invoke(
            "pad.drop",
            json!({ "rack": added.device, "note": note, "device": {
                "kind": { "native": { "device_type": "test.synth" } },
                "name": "Synth", "role": "instrument",
            } }),
        );
    }
    added.device
}

// Spec: supported-controllers R16, R38, R40. Shift with scene button 6 puts
// the pads in Drum mode: they arrive on channel 10 and play the selected
// track's drum rack as the drum layout's areas, lower left first, lit yellow
// where a drum rack pad holds a chain and blue for the selected one; a press
// selects the pad. A Session pad or Shift with Clip Stop gives Session back.
#[test]
fn drum_mode_plays_the_selected_drum_rack() {
    let mut rig = Rig::mini();
    let tracks = rig.tracks(2);
    rig.select(tracks[0]);
    let rack = drum_rack(&mut rig, &[36, 37, 52]);
    rig.settle();
    rig.shifted(0x7A, |rig| rig.tap(0x75));
    assert_eq!(rig.mode(), "Drum; track buttons: Clip Stop; faders: Volume");
    // The lower left area: notes 36 and 37; 38 has no chain. The area above
    // it holds 52 at its bottom left, selected as the last pad dropped.
    assert_eq!(rig.apc.pad(0, 7), Some(Pad::solid(YELLOW, 100)), "36");
    assert_eq!(rig.apc.pad(1, 7), Some(Pad::solid(YELLOW, 100)), "37");
    assert_eq!(rig.apc.pad(2, 7), None, "38 is empty");
    assert_eq!(rig.apc.pad(0, 3), Some(Pad::solid(BLUE, 100)), "52");
    assert_eq!(rig.apc.pad(0, 0), None, "the session's pads aren't lit");

    // Pressing a pad plays the note into the track and selects its pad.
    rig.apc.drum_down(1, 7);
    rig.apc.drum_up(1, 7);
    rig.settle();
    assert_eq!(pad_view(&rig, rack).1, Some(37));
    assert_eq!(rig.apc.pad(1, 7), Some(Pad::solid(BLUE, 100)));
    assert_eq!(rig.apc.pad(0, 3), Some(Pad::solid(YELLOW, 100)));

    // Shift with Clip Stop (scene button 1) shows the Session again.
    rig.shifted(0x7A, |rig| rig.tap(0x70));
    assert!(rig.mode().starts_with("Session"), "{}", rig.mode());
    rig.shifted(0x7A, |rig| rig.tap(0x76));
    assert!(rig.mode().starts_with("Note"), "{}", rig.mode());
    assert_eq!(rig.apc.pad(0, 7), None, "Note mode is the controller's own");
}

// Spec: supported-controllers R11, R17, R35. A grid surface sends its
// controller 24 clocks per quarter note at the set's tempo, stamped with
// their moments, and the clock and every light stop with the surface; the
// controller is given no message of its own to leave in.
#[test]
fn the_apc_mini_keeps_the_clock_and_puts_out_every_light_when_removed() {
    let mut rig = Rig::mini();
    let tracks = rig.tracks(3);
    let scenes = rig.scenes();
    rig.clip(tracks[0], scenes[0], 3);
    rig.run(
        "song.set_tempo",
        json!({ "tempo": [{ "kind": "constant", "start": 0, "bpm": 100.0 }] }),
    );
    for _ in 0..30 {
        rig.pass(20);
    }
    let pulses = rig.apc.clock();
    assert!(pulses.len() > 10, "{}", pulses.len());
    let want = 60.0 / (24.0 * 100.0);
    let gaps: Vec<f64> = pulses
        .windows(2)
        .rev()
        .take(10)
        .map(|w| w[1].duration_since(w[0]).as_secs_f64())
        .collect();
    assert!(gaps.iter().all(|g| (g - want).abs() < 1e-4), "{gaps:?}");
    assert!(!rig.shell.arrangement().transport().playing);

    assert!(rig.apc.lit() >= 1, "{}", rig.apc.lit());
    rig.run("control_surface.set", json!({ "row": 1, "surface": null }));
    rig.apc.pump();
    assert!(rig.apc.all_dark(), "every light is out");
    let count = rig.apc.clock().len();
    for _ in 0..10 {
        rig.pass(20);
    }
    assert_eq!(rig.apc.clock().len(), count, "the clock stops");
}

// The APC40 mkII ------------------------------------------------------------

fn bpm(rig: &Rig) -> f64 {
    match rig
        .shell
        .editor()
        .document()
        .settings()
        .tempo
        .segments()
        .next()
        .unwrap()
    {
        bl_core::time::TempoSegment::Constant { bpm, .. } => bpm.get(),
        other => panic!("{other:?}"),
    }
}

fn settings(rig: &Rig) -> bl_model::Settings {
    rig.shell.editor().document().settings().clone()
}

// Spec: supported-controllers R2, R5, R17, R41. Plugging the APC40 mkII in
// fills the first free row with its one port; the surface sends the
// introduction with mode 42h and Beatlayer's version, and on leaving puts out
// every light, the knob rings included, and sends mode 40h.
#[test]
fn plugging_the_apc40_in_starts_mode_42h_and_removing_it_sends_generic() {
    let mut rig = Rig::apc40();
    let row = rig.row();
    assert_eq!(row["surface"]["id"], "apc40-mkii");
    assert_eq!(row["input"]["name"], "APC40 mkII");
    assert_eq!(row["state"], "running");
    let [major, minor, fix] = version();
    assert_eq!(
        &rig.apc.messages()[..2],
        [
            vec![0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7],
            vec![
                0xF0, 0x47, 0x7F, 0x29, 0x60, 0x00, 0x04, 0x42, major, minor, fix, 0xF7
            ],
        ],
        "the inquiry, then the introduction"
    );
    assert_eq!(rig.apc.mode(), Some(0x42));
    assert_eq!(rig.row()["surface_state"]["ring"]["tracks"], 8);
    assert_eq!(rig.row()["surface_state"]["ring"]["scenes"], 5, "8 x 5");

    let tracks = rig.tracks(2);
    let scenes = rig.scenes();
    rig.clip(tracks[0], scenes[0], 3);
    rig.settle();
    assert!(rig.apc.lit() >= 3, "{}", rig.apc.lit());
    assert_eq!(rig.apc.ring(0x38), Some(3), "the track knobs' Pan rings");

    rig.run("control_surface.set", json!({ "row": 1, "surface": null }));
    rig.apc.pump();
    assert_eq!(
        rig.apc.lit_when_left(),
        Some(0),
        "every light was out first"
    );
    assert!(rig.apc.all_dark());
    assert_eq!(rig.apc.mode(), Some(0x40), "Generic on leaving");
    assert_eq!(rig.apc.ring(0x38), Some(0));
    assert_eq!(rig.apc.ring(0x18), Some(0));
}

// Spec: supported-controllers R6, R9, R42. The grid's top row, notes 20h to
// 27h, is the ring's first scene and the bottom row, 00h to 07h, its fifth; a
// pad shows its slot in the clip's colour, queued flashing green, playing
// pulsing green, an empty slot of an armed track dim red; pressing a pad
// launches its clip and a scene button its scene, lit dim white with clips
// and green while one plays.
#[test]
fn the_apc40s_pads_follow_the_ring_with_the_top_row_first() {
    let mut rig = Rig::apc40();
    let tracks = rig.tracks(3);
    let scenes = rig.scenes();
    rig.clip(tracks[0], scenes[0], 3);
    rig.clip(tracks[1], scenes[4], 8);
    rig.run(
        "session.set_quantization",
        json!({ "quantization": "1_bar" }),
    );
    rig.run("transport.play", json!({}));
    rig.settle();
    assert_eq!(
        rig.apc.rgb(0x20),
        Some(Pad::solid(colour_of(3), 100)),
        "20h"
    );
    assert_eq!(
        rig.apc.rgb(0x01),
        Some(Pad::solid(colour_of(8), 100)),
        "01h"
    );
    assert_eq!(rig.apc.rgb(0x21), None);
    assert_eq!(rig.apc.rgb(0x27), None, "past the last track");
    assert_eq!(rig.apc.scene_pad(0), Some(Pad::solid(2, 100)), "dim white");
    assert_eq!(rig.apc.scene_pad(2), None);

    rig.pad_tap(0, 0);
    testing::advance(&rig.control, &mut rig.shell, 20);
    assert!(rig.queued(tracks[0], scenes[0]));
    rig.settle();
    assert_eq!(
        rig.apc.rgb(0x20),
        Some(Pad::flashing(colour_of(3), Motion::Blinking, GREEN, 8)),
        "queued: flashing green"
    );
    rig.wait_playing(tracks[0], scenes[0]);
    assert_eq!(
        rig.apc.rgb(0x20),
        Some(Pad::flashing(colour_of(3), Motion::Pulsing, GREEN, 4)),
        "playing: pulsing green"
    );
    assert_eq!(rig.apc.scene_pad(0), Some(Pad::solid(GREEN, 100)));

    // A scene button launches its scene.
    rig.quantisation_off();
    rig.tap(0x56);
    rig.wait_playing(tracks[1], scenes[4]);
    assert_eq!(rig.apc.scene_pad(4), Some(Pad::solid(GREEN, 100)));
    // After a play, the pad is solid again once the clip stops.
    rig.run("session.stop_all", json!({}));
    testing::wait_for(
        &rig.control,
        &mut rig.shell,
        "every clip to stop",
        |shell| shell.session_states().is_empty(),
    );
    rig.settle();
    assert_eq!(rig.apc.rgb(0x20), Some(Pad::solid(colour_of(3), 100)));

    // An empty slot of an armed track is dim red; pressing records.
    rig.run("mixer.set_arm", json!({ "track": tracks[2], "arm": true }));
    rig.settle();
    assert_eq!(rig.apc.rgb(0x22), Some(Pad::solid(6, 100)));
    rig.pad_tap(2, 0);
    let track = tracks[2];
    testing::wait_for(&rig.control, &mut rig.shell, "the recording", |shell| {
        shell
            .session_states()
            .iter()
            .any(|s| s.track == track && s.state == bl_engine::SlotState::Recording)
    });
    rig.settle();
    assert_eq!(
        rig.apc.rgb(0x22),
        Some(Pad::flashing(6, Motion::Pulsing, RED, 4)),
        "recording: pulsing red"
    );
}

// Spec: supported-controllers R43. The arrows move the ring one scene or
// track; Bank switches on and off with each press, lit while on, and the
// arrows then move five scenes or eight tracks.
#[test]
fn bank_switches_the_arrows_between_one_step_and_a_page() {
    let mut rig = Rig::apc40();
    rig.tracks(20);
    for _ in 0..7 {
        rig.invoke("session.insert_scene", json!({}));
    }
    // Up 5Eh, Down 5Fh, Right 60h, Left 61h.
    rig.tap(0x60);
    assert_eq!(rig.ring_at(), (1, 0));
    rig.tap(0x5F);
    assert_eq!(rig.ring_at(), (1, 1));
    rig.tap(0x61);
    rig.tap(0x5E);
    assert_eq!(rig.ring_at(), (0, 0));

    assert_eq!(rig.apc.single(0x67, 0), None);
    rig.tap(0x67);
    assert_eq!(rig.apc.single(0x67, 0), Some(1), "Bank is on");
    rig.tap(0x60);
    assert_eq!(rig.ring_at(), (8, 0), "eight tracks");
    rig.tap(0x5F);
    assert_eq!(rig.ring_at(), (8, 5), "five scenes");
    rig.tap(0x67);
    assert_eq!(rig.apc.single(0x67, 0), None, "and off again");
    rig.tap(0x61);
    assert_eq!(rig.ring_at(), (7, 5));
}

// Spec: supported-controllers R43. Holding Shift shows Session Overview: each
// pad stands for a block of eight tracks and five scenes, amber for the
// ring's block (both when the ring straddles two), green while a clip plays in
// it, red with clips none of which play, dark with none; pressing a pad moves
// the ring there, and releasing Shift gives the pads back.
#[test]
fn shift_shows_session_overview_and_a_pad_moves_the_ring() {
    let mut rig = Rig::apc40();
    let tracks = rig.tracks(20);
    for _ in 0..7 {
        rig.invoke("session.insert_scene", json!({}));
    }
    let scenes = rig.scenes();
    rig.quantisation_off();
    rig.clip(tracks[9], scenes[6], 3);
    rig.clip(tracks[17], scenes[11], 3);
    rig.run("transport.play", json!({}));
    rig.settle();
    rig.shifted(0x62, |rig| {
        assert_eq!(
            rig.mode(),
            "Session Overview; faders: Volume; track knobs: Pan; device knobs: Device"
        );
        assert_eq!(
            rig.apc.pad(0, 0),
            Some(Pad::solid(126, 100)),
            "the ring's block"
        );
        assert_eq!(
            rig.apc.pad(1, 1),
            Some(Pad::solid(120, 100)),
            "clips, none play"
        );
        assert_eq!(
            rig.apc.pad(2, 2),
            Some(Pad::solid(120, 100)),
            "clips, none play"
        );
        assert_eq!(rig.apc.pad(2, 0), None, "no clips");
        assert_eq!(rig.apc.pad(3, 0), None, "past the last track");
    });
    // A clip that plays turns its block green.
    rig.pad_tap(0, 0);
    rig.run(
        "session.launch",
        json!({ "track": tracks[9], "scene": scenes[6] }),
    );
    rig.wait_playing(tracks[9], scenes[6]);
    // The ring straddles two blocks.
    rig.tap(0x60);
    rig.tap(0x60);
    rig.tap(0x60);
    rig.shifted(0x62, |rig| {
        assert_eq!(rig.apc.pad(0, 0), Some(Pad::solid(126, 100)));
        assert_eq!(rig.apc.pad(1, 0), Some(Pad::solid(126, 100)), "both blocks");
        assert_eq!(
            rig.apc.pad(1, 1),
            Some(Pad::solid(122, 100)),
            "a clip plays"
        );
    });
    // A pad moves the ring to its block.
    rig.apc.button_down(0x62);
    rig.settle();
    rig.apc.pad_down(2, 2);
    rig.apc.pad_up(2, 2);
    rig.settle();
    rig.apc.button_up(0x62);
    rig.settle();
    assert_eq!(
        rig.ring_at(),
        (12, 10),
        "track 16 is clamped to the last eight"
    );
    assert!(
        rig.apc.rgb(0x20).is_none(),
        "Shift is released: the clips show"
    );
}

// Spec: supported-controllers R44. The Activator mutes and unmutes its
// track, lit while the track is heard; Solo and Record Arm are lit while on;
// A/B steps the crossfade assignment, yellow for A and orange for B; Clip
// Stop stops the track's clip, lit while it plays and blinking while its stop
// waits; Track Select selects, lit for the selected track, and Master selects
// the Main track.
#[test]
fn the_apc40s_strip_buttons_act_and_light() {
    use bl_model::CrossfadeAssign;
    let mut rig = Rig::apc40();
    let tracks = rig.tracks(2);
    let scenes = rig.scenes();
    rig.quantisation_off();
    rig.clip(tracks[0], scenes[0], 3);
    rig.settle();
    // Per-track channels: 0 is the first track, 1 the second.
    assert_eq!(rig.apc.single(0x32, 1), Some(1), "heard: lit");
    rig.strip(1, 0x32);
    assert!(rig.mixer(tracks[1]).mute);
    assert_eq!(rig.apc.single(0x32, 1), None);
    assert_eq!(rig.apc.single(0x32, 0), Some(1), "the other track");
    rig.strip(1, 0x31);
    assert!(rig.mixer(tracks[1]).solo);
    assert_eq!(rig.apc.single(0x31, 1), Some(1));
    rig.strip(1, 0x30);
    assert!(rig.mixer(tracks[1]).arm);
    assert_eq!(rig.apc.single(0x30, 1), Some(1));
    assert_eq!(rig.apc.single(0x30, 0), None);
    assert_eq!(rig.apc.single(0x31, 2), None, "no third track");

    // A/B: yellow for A, orange for B, and round to none.
    let mut seen = Vec::new();
    for _ in 0..3 {
        rig.strip(0, 0x42);
        let assign = rig.mixer(tracks[0]).crossfade;
        let light = rig.apc.single(0x42, 0);
        match assign {
            CrossfadeAssign::A => assert_eq!(light, Some(1), "A is yellow"),
            CrossfadeAssign::B => assert_eq!(light, Some(2), "B is orange"),
            CrossfadeAssign::None => assert_eq!(light, None),
        }
        seen.push(assign);
    }
    for want in [
        CrossfadeAssign::A,
        CrossfadeAssign::B,
        CrossfadeAssign::None,
    ] {
        assert!(seen.contains(&want), "{seen:?}");
    }

    // Track Select, and Master.
    rig.strip(0, 0x33);
    assert_eq!(
        rig.shell.arrangement().selected_tracks().to_vec(),
        vec![tracks[0]]
    );
    assert_eq!(rig.apc.single(0x33, 0), Some(1));
    assert_eq!(rig.apc.single(0x33, 1), None);
    let main = rig.shell.editor().document().main_id();
    rig.tap(0x50);
    assert_eq!(
        rig.shell.arrangement().selected_tracks().to_vec(),
        vec![main]
    );
    assert_eq!(rig.apc.single(0x50, 0), Some(1), "Master is selected");
    assert_eq!(rig.apc.single(0x33, 0), None);

    // Clip Stop: lit while the clip plays, blinking while its stop waits.
    rig.run("transport.play", json!({}));
    rig.pad_tap(0, 0);
    rig.wait_playing(tracks[0], scenes[0]);
    assert_eq!(rig.apc.single(0x34, 0), Some(1), "playing");
    rig.run(
        "session.set_quantization",
        json!({ "quantization": "1_bar" }),
    );
    rig.strip(0, 0x34);
    testing::advance(&rig.control, &mut rig.shell, 20);
    rig.settle();
    assert_eq!(rig.apc.single(0x34, 0), Some(2), "blinking while it waits");
    testing::wait_for(&rig.control, &mut rig.shell, "the stop", |shell| {
        shell.session_states().is_empty()
    });
    rig.settle();
    assert_eq!(rig.apc.single(0x34, 0), None);
}

// Spec: supported-controllers R45. Shift with Track Select 1 to 8 sets the
// global launch quantisation to None, 8 Bars, 4 Bars, 2 Bars, 1 Bar, 1/4,
// 1/8 and 1/16, and selects nothing.
#[test]
fn shift_with_track_select_sets_the_launch_quantisation() {
    use bl_model::LaunchQuantization as Q;
    let mut rig = Rig::apc40();
    rig.tracks(8);
    let selected = rig.shell.arrangement().selected_tracks().to_vec();
    for (column, want) in [
        Q::None,
        Q::Bars8,
        Q::Bars4,
        Q::Bars2,
        Q::Bar,
        Q::Quarter,
        Q::Eighth,
        Q::Sixteenth,
    ]
    .into_iter()
    .enumerate()
    {
        // Another first, so a change is seen.
        let other = if want == Q::Half { Q::Bar } else { Q::Half };
        rig.run("session.set_quantization", json!({ "quantization": other }));
        rig.shifted(0x62, |rig| rig.strip(column as u8, 0x33));
        assert_eq!(rig.shell.launch_settings().quantization, want, "{column}");
    }
    assert_eq!(
        rig.shell.arrangement().selected_tracks().to_vec(),
        selected,
        "no track was selected"
    );
}

// Spec: supported-controllers R46. The track faders set the ring's tracks'
// volume, the master fader the Main track's, the crossfader the crossfader
// and Cue Level, a relative knob, the cue volume; the tempo knob steps the
// tempo a BPM, or a tenth with Shift, Tap Tempo taps it, and Nudge holds the
// playing tempo's nudge while pressed.
#[test]
fn the_apc40s_faders_and_knobs_set_the_mixer_and_the_tempo() {
    let mut rig = Rig::apc40();
    let tracks = rig.tracks(3);
    let main = rig.shell.editor().document().main_id();
    // Track faders: controller 07h on each track's channel.
    rig.cc(2, 0x07, 127);
    let top = rig.mixer(tracks[2]).volume;
    rig.cc(2, 0x07, 0);
    assert!(top > 0.0 && rig.mixer(tracks[2]).volume == 0.0, "{top}");
    assert!(rig.mixer(tracks[0]).volume > 0.0, "the other tracks stay");
    rig.cc(0, 0x0E, 127);
    assert!(
        (rig.mixer(main).volume - top).abs() < 1e-9,
        "the master fader"
    );
    rig.cc(0, 0x0F, 127);
    assert!((settings(&rig).crossfader - 1.0).abs() < 1e-9, "B end");
    rig.cc(0, 0x0F, 0);
    assert!(settings(&rig).crossfader.abs() < 1e-9, "A end");

    // Cue Level is relative: 01h to 3Fh up, 7Fh down to 40h down.
    let cue = settings(&rig).cue_volume;
    for _ in 0..5 {
        rig.cc(0, 0x2F, 0x01);
    }
    let up = settings(&rig).cue_volume;
    assert!(up > cue, "{cue} {up}");
    for _ in 0..5 {
        rig.cc(0, 0x2F, 0x7F);
    }
    assert!((settings(&rig).cue_volume - cue).abs() < 1e-6, "back again");

    // The tempo knob: a BPM a step, even several in one tick.
    rig.run(
        "song.set_tempo",
        json!({ "tempo": [{ "kind": "constant", "start": 0, "bpm": 120.0 }] }),
    );
    rig.settle();
    rig.cc(0, 0x0D, 0x01);
    assert!((bpm(&rig) - 121.0).abs() < 1e-9, "{}", bpm(&rig));
    rig.cc(0, 0x0D, 0x7F);
    rig.cc(0, 0x0D, 0x7F);
    assert!((bpm(&rig) - 119.0).abs() < 1e-9, "{}", bpm(&rig));
    for _ in 0..3 {
        rig.apc.controller(0, 0x0D, 0x01);
    }
    rig.settle();
    assert!((bpm(&rig) - 122.0).abs() < 1e-9, "three steps in one tick");
    // A tenth with Shift held.
    rig.shifted(0x62, |rig| {
        rig.cc(0, 0x0D, 0x01);
        rig.cc(0, 0x0D, 0x01);
    });
    assert!((bpm(&rig) - 122.2).abs() < 1e-9, "{}", bpm(&rig));

    // Tap Tempo taps: two taps half a second apart set 120 BPM.
    rig.run(
        "song.set_tempo",
        json!({ "tempo": [{ "kind": "constant", "start": 0, "bpm": 100.0 }] }),
    );
    rig.pass(50);
    rig.tap(0x63);
    rig.pass(500);
    rig.tap(0x63);
    assert!((bpm(&rig) - 120.0).abs() < 1.0, "{}", bpm(&rig));
    // Nudge holds the nudge while it is pressed.
    rig.run("transport.play", json!({}));
    rig.wait_transport(true);
    let nudge = |rig: &mut Rig| rig.run("transport.state", json!({}))["phase_nudge"].clone();
    for (note, held) in [(0x65, "up"), (0x64, "down")] {
        rig.apc.button_down(note);
        rig.settle();
        rig.pass(20);
        assert_eq!(nudge(&mut rig), held);
        rig.apc.button_up(note);
        rig.settle();
        rig.pass(20);
        assert_eq!(nudge(&mut rig), "none");
    }
}

// Spec: supported-controllers R47. Pan, Sends and User choose what the eight
// track knobs reach, the chosen one lit: Pan the ring's tracks' pan with Pan
// rings, Sends a send of each with Volume rings, and holding Sends while
// pressing Track Select N chooses send N; User leaves them to a manual
// mapping, with their rings off.
#[test]
fn the_pan_sends_and_user_buttons_choose_what_the_track_knobs_reach() {
    let mut rig = Rig::apc40();
    let tracks = rig.tracks(2);
    rig.returns(2);
    let doc = rig.shell.editor().document();
    let (first_return, second_return) =
        (doc.returns().next().unwrap(), doc.returns().nth(1).unwrap());
    rig.settle();
    // Pan at first: lit, with Pan rings on the tracks that exist.
    assert_eq!(rig.apc.single(0x57, 0), Some(1));
    assert_eq!(rig.apc.single(0x58, 0), None);
    assert_eq!(rig.apc.ring(0x38), Some(3), "Pan ring");
    assert_eq!(rig.apc.ring(0x3A), Some(3), "the first return has a pan");
    assert_eq!(rig.apc.ring(0x3E), Some(0), "no track: dark");
    rig.cc(0, 0x30, 127);
    assert!((rig.mixer(tracks[0]).pan - 1.0).abs() < 1e-9);
    // The ring follows a change from elsewhere.
    rig.run("mixer.set_pan", json!({ "track": tracks[1], "pan": -1.0 }));
    rig.settle();
    assert_eq!(rig.apc.ring(0x31), Some(0), "full left");
    assert!(rig.mode().contains("track knobs: Pan"), "{}", rig.mode());

    // Sends: Send A first, Volume rings; a return has no send to a return.
    rig.tap(0x58);
    assert_eq!(rig.apc.single(0x58, 0), Some(1));
    assert_eq!(rig.apc.single(0x57, 0), None);
    assert!(rig.mode().contains("track knobs: Send A"), "{}", rig.mode());
    assert_eq!(rig.apc.ring(0x38), Some(2), "Volume ring");
    assert_eq!(rig.apc.ring(0x3A), Some(0), "a return has no send");
    rig.cc(0, 0x30, 127);
    let doc = rig.shell.editor().document();
    assert!(doc.track(tracks[0]).unwrap().send(first_return) > 0.0);
    assert_eq!(doc.track(tracks[0]).unwrap().send(second_return), 0.0);
    // Holding Sends while pressing Track Select 2 chooses Send B.
    rig.apc.button_down(0x58);
    rig.settle();
    rig.strip(1, 0x33);
    rig.apc.button_up(0x58);
    rig.settle();
    assert!(rig.mode().contains("track knobs: Send B"), "{}", rig.mode());
    assert!(
        rig.shell.arrangement().selected_tracks().is_empty()
            || rig.shell.arrangement().selected_tracks() != [tracks[1]],
        "no track was selected"
    );
    rig.cc(0, 0x30, 127);
    let doc = rig.shell.editor().document();
    assert!(doc.track(tracks[0]).unwrap().send(second_return) > 0.0);

    // User: nothing of the surface's, the rings off.
    rig.tap(0x59);
    assert_eq!(rig.apc.single(0x59, 0), Some(1));
    assert_eq!(rig.apc.ring(0x38), Some(0), "rings off");
    rig.run("mixer.set_pan", json!({ "track": tracks[0], "pan": 0.0 }));
    rig.cc(0, 0x30, 127);
    assert_eq!(rig.mixer(tracks[0]).pan, 0.0, "left to a manual mapping");
}

// Spec: supported-controllers R48. The eight device knobs reach the device in
// focus's bank with Single rings, a knob with no parameter dark; Device Left
// and Right select the previous and next device, Bank Left and Right the
// bank, each lit while there is one; Device On/Off switches the device; Device
// Lock locks it, lit while locked; Clip/Device View and Detail View switch the
// detail area; Shift with those eight buttons chooses banks 1 to 8.
#[test]
fn the_device_section_follows_the_device_in_focus_and_its_banks() {
    let mut rig = Rig::apc40();
    let tracks = rig.tracks(2);
    rig.select(tracks[0]);
    let first = rig.add("beatlayer.utility", "audio_effect");
    let second = rig.add("beatlayer.utility", "audio_effect");
    rig.settle();
    let focus = |rig: &mut Rig| rig.row()["surface_state"]["device"].clone();
    assert_eq!(focus(&mut rig)["device"], json!(second));
    // Device Left is lit (a device is before), Right is not.
    assert_eq!(rig.apc.single(0x3A, 0), Some(1));
    assert_eq!(rig.apc.single(0x3B, 0), None);
    // Bank Left is dark at the first bank, Bank Right lit.
    assert_eq!(rig.apc.single(0x3C, 0), None);
    assert_eq!(rig.apc.single(0x3D, 0), Some(1));
    for knob in 0..8 {
        assert_eq!(
            rig.apc.ring(0x18 + knob),
            Some(1),
            "Single ring: knob {knob}"
        );
    }
    rig.cc(0, 0x10, 127);
    let banks = rig.run("control_surface.banks", json!({ "device": second }));
    let gain = banks["banks"][0]["params"][0]["param"].as_u64().unwrap() as u32;
    let param = |rig: &Rig, device: DeviceId, id: u32| {
        rig.shell
            .editor()
            .document()
            .device(device)
            .and_then(|d| d.params.get(&bl_model::ParamId(id)).copied())
    };
    assert!(
        param(&rig, second, gain).is_some(),
        "knob 1 reaches the first parameter"
    );
    // The ring's value follows the parameter.
    assert_eq!(rig.apc.ring(0x10), Some(127));

    // Device Left selects the previous device; the knobs follow.
    rig.tap(0x3A);
    assert_eq!(focus(&mut rig)["device"], json!(first));
    assert_eq!(rig.apc.single(0x3B, 0), Some(1), "Right is lit now");
    assert_eq!(rig.apc.single(0x3A, 0), None);
    rig.tap(0x3B);
    assert_eq!(focus(&mut rig)["device"], json!(second));

    // Banks: Bank Right steps, the second bank has fewer parameters.
    rig.tap(0x3D);
    assert_eq!(focus(&mut rig)["bank"], 1);
    assert_eq!(rig.apc.single(0x3D, 0), None, "the last bank");
    assert_eq!(rig.apc.single(0x3C, 0), Some(1));
    let in_bank = banks["banks"][1]["params"].as_array().unwrap().len();
    assert!(in_bank < 8, "{in_bank}");
    assert_eq!(
        rig.apc.ring(0x18 + in_bank as u8),
        Some(0),
        "dark: no parameter"
    );
    assert_eq!(rig.apc.ring(0x18), Some(1));
    // Shift with the buttons in order chooses the banks: 1 is the first.
    rig.shifted(0x62, |rig| rig.tap(0x3A));
    assert_eq!(focus(&mut rig)["bank"], 0, "bank 1");
    rig.shifted(0x62, |rig| rig.tap(0x3B));
    assert_eq!(focus(&mut rig)["bank"], 1, "bank 2");
    rig.shifted(0x62, |rig| rig.tap(0x3C));
    assert_eq!(focus(&mut rig)["bank"], 1, "there is no bank 3");

    // Device On/Off, lit while on.
    assert_eq!(rig.apc.single(0x3E, 0), Some(1));
    rig.tap(0x3E);
    let enabled = |rig: &Rig| {
        rig.shell
            .editor()
            .document()
            .device(second)
            .unwrap()
            .enabled
    };
    assert!(!enabled(&rig));
    assert_eq!(rig.apc.single(0x3E, 0), None);
    rig.tap(0x3E);
    assert!(enabled(&rig));

    // Device Lock locks the device in focus to this surface, and again gives
    // it back.
    assert_eq!(rig.apc.single(0x3F, 0), None);
    rig.tap(0x3F);
    assert_eq!(focus(&mut rig)["locked"], "hand");
    assert_eq!(rig.apc.single(0x3F, 0), Some(1), "lit while locked");
    rig.select(tracks[1]);
    assert_eq!(focus(&mut rig)["device"], json!(second), "locked");
    rig.tap(0x3F);
    assert_eq!(rig.apc.single(0x3F, 0), None);

    // Clip/Device View and Detail View.
    let shows = |rig: &Rig| {
        let state = rig.shell.detail().state();
        (state.clip_view, state.device_view)
    };
    let before = shows(&rig);
    rig.tap(0x40);
    assert_ne!(shows(&rig), before, "the other view shows");
    rig.tap(0x41);
    assert_eq!(shows(&rig), (false, false), "the detail area is hidden");
    rig.tap(0x41);
    assert_ne!(shows(&rig), (false, false));
}

// Spec: supported-controllers R49, R50. Play starts the transport, lit while it
// runs; Record switches Arrangement recording; Session switches Session
// Record; Metronome the metronome, lit while on; the Stop note and the
// footswitch are left to manual mappings.
#[test]
fn the_apc40s_transport_buttons_run_and_light() {
    let mut rig = Rig::apc40();
    let tracks = rig.tracks(1);
    rig.run("mixer.set_arm", json!({ "track": tracks[0], "arm": true }));
    assert_eq!(rig.apc.single(0x5B, 0), None);
    rig.tap(0x5B);
    rig.wait_transport(true);
    assert_eq!(rig.apc.single(0x5B, 0), Some(1), "lit while it runs");
    // Stop is left to a manual mapping, and so is the footswitch.
    rig.tap(0x5C);
    rig.cc(0, 0x40, 127);
    rig.pass(50);
    assert!(
        rig.shell.arrangement().transport().playing,
        "Stop isn't taken"
    );
    rig.run("transport.stop", json!({}));
    rig.wait_transport(false);
    assert_eq!(rig.apc.single(0x5B, 0), None);

    // Metronome.
    rig.tap(0x5A);
    assert!(settings(&rig).metronome.enabled);
    assert_eq!(rig.apc.single(0x5A, 0), Some(1));
    rig.tap(0x5A);
    assert!(!settings(&rig).metronome.enabled);
    assert_eq!(rig.apc.single(0x5A, 0), None);

    // Session Record, which runs while the transport does.
    rig.run("transport.play", json!({}));
    rig.wait_transport(true);
    rig.tap(0x66);
    testing::wait_for(&rig.control, &mut rig.shell, "the recording", |s| {
        s.session_states()
            .iter()
            .any(|slot| slot.state == bl_engine::SlotState::Recording)
    });
    rig.settle();
    assert_eq!(rig.apc.single(0x66, 0), Some(1));
    rig.tap(0x66);
    testing::wait_for(&rig.control, &mut rig.shell, "the recording to end", |s| {
        s.session_states()
            .iter()
            .all(|slot| slot.state != bl_engine::SlotState::Recording)
    });
    // The recording ends on the next grid line, a bar away.
    rig.pass(4000);
    assert_eq!(rig.apc.single(0x66, 0), None);
    rig.run("transport.stop", json!({}));
    rig.wait_transport(false);

    // Record switches Arrangement recording.
    rig.tap(0x5D);
    rig.pass(50);
    assert!(rig.run("transport.state", json!({}))["recording"].is_object());
    assert_eq!(rig.apc.single(0x5D, 0), Some(1));
    rig.tap(0x5D);
    rig.pass(50);
    assert_eq!(rig.apc.single(0x5D, 0), None, "recording has ended");
}

//! The Launchkey MK3 and Launchkey MK4 (WP-7.10;
//! `docs/specs/supported-controllers.md`, R51 to R71): each controller is a
//! simulated device on the in-process test backend, with the engine paced by
//! hand, so every rule runs headless.

use bl_audio_io::NullControl;
use bl_control::grid::Light;
use bl_control::launchkey::sim::{Launchkey, Size};
use bl_midi_io::TestBackend;
use bl_model::{DeviceId, SceneId, TrackId, TrackKind};
use bl_shell::{Shell, testing};
use serde_json::{Value, json};

/// Palette entries the surfaces name.
const WHITE: u8 = 3;
const DIM_WHITE: u8 = 1;
const RED: u8 = 5;
const DIM_RED: u8 = 7;
const YELLOW: u8 = 13;
const DIM_YELLOW: u8 = 15;
const GREEN: u8 = 21;
const BLUE: u8 = 45;
const DIM_BLUE: u8 = 47;
const PURPLE: u8 = 49;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Which {
    Mk3(Size),
    Mk4(Size),
}

impl Which {
    const fn mk4(self) -> bool {
        matches!(self, Self::Mk4(_))
    }
}

struct Rig {
    shell: Shell,
    control: NullControl,
    test: TestBackend,
    keys: Launchkey,
}

impl Rig {
    /// A set with the controller plugged in and recognised.
    fn new(which: Which) -> Self {
        let mut rig = Self::unplugged(which);
        rig.plug_in();
        rig
    }

    fn unplugged(which: Which) -> Self {
        let (mut shell, control) = testing::rig();
        let test = TestBackend::new();
        let keys = match which {
            Which::Mk3(size) => Launchkey::mk3(&test, size),
            Which::Mk4(size) => Launchkey::mk4(&test, size),
        };
        shell.use_midi_backend(Box::new(test.clone()));
        shell.start_audio();
        Self {
            shell,
            control,
            test,
            keys,
        }
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
        self.keys.plug_in();
        self.shell.refresh_midi();
        for _ in 0..4 {
            self.keys.pump();
            self.shell.poll();
            self.shell.midi().surfaces().settle();
        }
        self.keys.pump();
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
        self.keys.pump();
    }

    /// A message's commands run on the first poll and what they changed
    /// shows on a later tick.
    fn settle(&mut self) {
        self.sync();
        self.sync();
        self.sync();
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

    fn ring(&mut self) -> (u64, u64) {
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

    /// Adds `device_type` to the selected track's chain.
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

    fn select(&mut self, track: TrackId) {
        self.invoke("arrangement.select_tracks", json!({ "tracks": [track] }));
        self.shell.set_focused_area(bl_shell::Area::Detail);
    }

    fn selected(&self) -> Vec<TrackId> {
        self.shell.arrangement().selected_tracks().to_vec()
    }

    /// The controller sends a press and a release of a Session pad.
    fn tap(&mut self, column: u8, row: u8) {
        self.keys.pad_down(column, row);
        self.keys.pad_up(column, row);
        self.settle();
    }

    fn press(&mut self, cc: u8) {
        self.keys.button_down(cc);
        self.settle();
    }

    fn release(&mut self, cc: u8) {
        self.keys.button_up(cc);
        self.settle();
    }

    fn tap_button(&mut self, cc: u8) {
        self.press(cc);
        self.release(cc);
    }

    fn quantisation_off(&mut self) {
        self.run(
            "session.set_quantization",
            json!({ "quantization": "none" }),
        );
    }

    fn param(&self, device: DeviceId, param: u32) -> Option<f64> {
        self.shell
            .editor()
            .document()
            .device(device)
            .and_then(|d| d.params.get(&bl_model::ParamId(param)).copied())
    }

    fn mixer(&self, track: TrackId) -> bl_model::Mixer {
        *self.shell.editor().document().track(track).unwrap().mixer()
    }
}

fn is_static(light: Option<Light>, colour: u8) -> bool {
    light == Some(Light::Static(colour))
}

const ALL: [Which; 4] = [
    Which::Mk3(Size::K61),
    Which::Mk3(Size::K25),
    Which::Mk4(Size::K61),
    Which::Mk4(Size::K25),
];

// Spec: supported-controllers R2, R3, R51, R61, R72. Plugging a controller in
// fills the first free row with its DAW ports, and the surface starts by
// sending DAW mode and the modes it drives, the fader mode only where there
// are faders; its SysEx header is the 88's or the Mini's where the name says
// so; the MIDI port stays an ordinary MIDI port that plays tracks.
#[test]
fn plugging_a_controller_in_fills_row_one_and_starts_it_in_daw_mode() {
    for (which, id, daw, midi, header_id, modes) in [
        (
            Which::Mk3(Size::K61),
            "launchkey-mk3",
            "Launchkey MK3 61 LKMK3 DAW",
            "Launchkey MK3 61 LKMK3 MIDI",
            0x0F,
            vec![
                vec![0xBF, 0x03, 0x02],
                vec![0xBF, 0x09, 0x02],
                vec![0xBF, 0x0A, 0x01],
            ],
        ),
        (
            Which::Mk3(Size::K25),
            "launchkey-mk3",
            "Launchkey MK3 25 LKMK3 DAW",
            "Launchkey MK3 25 LKMK3 MIDI",
            0x0F,
            vec![vec![0xBF, 0x03, 0x02], vec![0xBF, 0x09, 0x02]],
        ),
        (
            Which::Mk3(Size::K88),
            "launchkey-mk3",
            "Launchkey MK3 88 LKMK3 DAW",
            "Launchkey MK3 88 LKMK3 MIDI",
            0x12,
            vec![
                vec![0xBF, 0x03, 0x02],
                vec![0xBF, 0x09, 0x02],
                vec![0xBF, 0x0A, 0x01],
            ],
        ),
        (
            Which::Mk4(Size::K49),
            "launchkey-mk4",
            "Launchkey MK4 49 DAW",
            "Launchkey MK4 49 MIDI",
            0x14,
            vec![
                vec![0xB6, 0x1D, 0x02],
                vec![0xB6, 0x1E, 0x02],
                vec![0xB6, 0x1F, 0x01],
            ],
        ),
        (
            Which::Mk4(Size::Mini),
            "launchkey-mk4",
            "Launchkey Mini MK4 25 DAW",
            "Launchkey Mini MK4 25 MIDI",
            0x13,
            vec![vec![0xB6, 0x1D, 0x02], vec![0xB6, 0x1E, 0x02]],
        ),
    ] {
        let mut rig = Rig::new(which);
        let row = rig.row();
        assert_eq!(row["surface"]["id"], id);
        assert_eq!(row["input"]["name"], daw);
        assert_eq!(row["output"]["name"], daw);
        assert_eq!(row["state"], "running");
        assert!(rig.keys.daw_mode_on(), "{daw}");
        let sent = rig.keys.messages();
        assert_eq!(sent[0], vec![0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7], "{daw}");
        assert_eq!(sent[1], vec![0x9F, 0x0C, 0x7F], "DAW mode: {daw}");
        assert_eq!(&sent[2..2 + modes.len()], modes.as_slice(), "{daw}");
        // The header is the one the controller's size has.
        rig.settle();
        let header = [0xF0, 0x00, 0x20, 0x29, 0x02, header_id];
        let sysex = rig
            .keys
            .messages()
            .iter()
            .filter(|m| m.first() == Some(&0xF0) && m.get(1) != Some(&0x7E))
            .count();
        assert!(sysex > 0, "the surface sent text: {daw}");
        assert!(
            rig.keys
                .messages()
                .iter()
                .filter(|m| m.first() == Some(&0xF0) && m.get(1) != Some(&0x7E))
                .all(|m| m.starts_with(&header)),
            "the header of {daw}"
        );

        // R3: the MIDI port stays a port tracks hear.
        let ports = rig.shell.midi().ports();
        let port = ports
            .iter()
            .find(|p| p.name == midi && p.direction == bl_shell_spec::midi_io::PortDirection::Input)
            .expect("the MIDI port is listed");
        assert!(port.track, "its Track switch stays on: {midi}");
        assert!(
            rig.test.is_open(bl_midi_io::Direction::Input, midi),
            "{midi} is open"
        );
    }
}

// Spec: supported-controllers R7, R9, R52, R53, R62, R63. The sixteen pads are
// a ring eight tracks wide and two scenes tall: they launch, record and stop
// as the grid's do, light as its pads do, and ▲ and ▼ move the ring a scene,
// lit only while it can move; > launches the ring's first scene.
#[test]
fn the_pads_launch_record_and_stop_in_a_two_row_ring_and_the_arrows_move_it() {
    for which in ALL {
        let mut rig = Rig::new(which);
        let tracks = rig.tracks(10);
        let scenes = rig.scenes();
        rig.quantisation_off();
        rig.clip(tracks[0], scenes[0], 3);
        rig.clip(tracks[1], scenes[1], 5);
        rig.clip(tracks[0], scenes[3], 8);
        rig.settle();
        assert_eq!(rig.mode().split(';').next(), Some("Session"));
        for (column, row, swatch) in [(0, 0, 3), (1, 1, 5)] {
            let rgb = bl_model::swatch_rgb(&bl_model::DEFAULT_TRACK_PALETTE, swatch);
            let want = bl_control::grid::nearest(rgb);
            assert!(
                is_static(rig.keys.pad(column, row), want),
                "the clip in colour {swatch} lights its pad with entry {want}, got {:?}",
                rig.keys.pad(column, row)
            );
        }
        assert_eq!(rig.keys.pad(2, 0), None, "an empty slot is dark");

        // Launching: the pad pulses green while its clip plays.
        rig.run("transport.play", json!({}));
        rig.tap(0, 0);
        let (track, scene) = (tracks[0], scenes[0]);
        testing::wait_for(&rig.control, &mut rig.shell, "the clip to play", |shell| {
            shell.session_states().iter().any(|s| {
                s.track == track && s.scene == scene && s.state == bl_engine::SlotState::Playing
            })
        });
        rig.settle();
        assert_eq!(rig.keys.pad(0, 0), Some(Light::Pulse(GREEN)));

        // An unarmed track's empty slot stops its track; an armed one's is
        // dim red and records.
        rig.tap(0, 1);
        testing::wait_for(&rig.control, &mut rig.shell, "the clip to stop", |shell| {
            shell.session_states().is_empty()
        });
        rig.run("mixer.set_arm", json!({ "track": tracks[2], "arm": true }));
        rig.settle();
        assert!(is_static(rig.keys.pad(2, 0), DIM_RED));
        rig.tap(2, 0);
        let track = tracks[2];
        testing::wait_for(&rig.control, &mut rig.shell, "the recording", |shell| {
            shell
                .session_states()
                .iter()
                .any(|s| s.track == track && s.state == bl_engine::SlotState::Recording)
        });
        rig.settle();
        assert_eq!(rig.keys.pad(2, 0), Some(Light::Pulse(RED)));
        rig.run("session.stop_recording", json!({}));
        rig.settle();

        // The arrows: ▼ moves the ring a scene, lit only while it can.
        assert_eq!(rig.keys.button(0x6A), None, "▲ at the first scene is dark");
        assert!(is_static(rig.keys.button(0x6B), WHITE));
        rig.tap_button(0x6B);
        assert_eq!(rig.ring(), (0, 1));
        assert!(is_static(rig.keys.button(0x6A), WHITE));
        assert!(
            is_static(
                rig.keys.pad(1, 0),
                bl_control::grid::nearest(bl_model::swatch_rgb(
                    &bl_model::DEFAULT_TRACK_PALETTE,
                    5
                ))
            ),
            "the pads follow the ring"
        );
        rig.tap_button(0x6A);
        assert_eq!(rig.ring(), (0, 0));

        // >: the ring's first scene.
        assert!(
            is_static(rig.keys.button(0x68), DIM_WHITE),
            "a scene with clips"
        );
        rig.tap_button(0x68);
        let (a, scene) = (tracks[0], scenes[0]);
        testing::wait_for(&rig.control, &mut rig.shell, "the scene to play", |shell| {
            shell.session_states().iter().any(|s| {
                s.track == a && s.scene == scene && s.state == bl_engine::SlotState::Playing
            })
        });
        rig.settle();
        assert!(
            is_static(rig.keys.button(0x68), GREEN),
            "green while it plays"
        );
    }
}

// Spec: supported-controllers R52, R62. The track buttons select the previous
// and next track and move the ring a track when the selection would leave it;
// on the MK4 they are lit only when they can move, and with Shift they move
// the ring eight tracks and the screen shows which.
#[test]
fn the_track_buttons_select_and_move_the_ring_past_its_edge() {
    for which in ALL {
        let mut rig = Rig::new(which);
        let tracks = rig.tracks(10);
        rig.invoke("arrangement.select_tracks", json!({ "tracks": [] }));
        rig.settle();
        if which.mk4() {
            assert_eq!(rig.keys.button(0x66), None, "nothing to the left");
            assert!(is_static(rig.keys.button(0x67), WHITE));
        } else {
            assert_eq!(rig.keys.button(0x67), None, "the MK3's have no light");
        }
        rig.tap_button(0x67);
        assert_eq!(rig.selected(), [tracks[0]]);
        for track in &tracks[1..8] {
            rig.tap_button(0x67);
            assert_eq!(rig.selected(), [*track]);
        }
        assert_eq!(rig.ring(), (0, 0), "still in the ring");
        rig.tap_button(0x67);
        assert_eq!(rig.selected(), [tracks[8]]);
        assert_eq!(rig.ring(), (1, 0), "the ring moved a track");
        rig.tap_button(0x67);
        rig.tap_button(0x67);
        assert_eq!(rig.selected(), [tracks[9]], "the last track");
        assert_eq!(rig.ring(), (2, 0), "the ring stops at the last track");
        if which.mk4() {
            assert_eq!(rig.keys.button(0x67), None, "nothing to the right");
            assert!(is_static(rig.keys.button(0x66), WHITE));
            assert_eq!(rig.keys.screen(0x21).fields, ["Track", "T9"]);
        }
        rig.tap_button(0x66);
        assert_eq!(rig.selected(), [tracks[8]]);
        assert_eq!(rig.ring(), (2, 0));

        if which.mk4() {
            // With Shift the ring moves eight tracks.
            rig.press(0x3F);
            assert!(is_static(rig.keys.button(0x66), WHITE));
            assert_eq!(rig.keys.button(0x67), None, "the ring is at the end");
            rig.tap_button(0x66);
            assert_eq!(rig.ring(), (0, 0));
            assert_eq!(rig.keys.screen(0x21).fields, ["Tracks", "1-8"]);
            rig.tap_button(0x67);
            assert_eq!(rig.ring(), (2, 0), "eight, to where the last track ends it");
            assert_eq!(rig.selected(), [tracks[8]], "the selection stays");
            rig.release(0x3F);
        }
    }
}

// Spec: supported-controllers R13, R53, R63. The third button steps the
// bottom row through its track functions and back to clips: the MK3's Stop,
// Solo, Mute and the MK4's Stop, Mute, Solo, lit in the colour of the
// function it shows, and the pads act and light as the grid's track buttons.
#[test]
fn the_bottom_row_cycles_through_its_track_functions() {
    for which in ALL {
        let mut rig = Rig::new(which);
        let tracks = rig.tracks(2);
        let scenes = rig.scenes();
        rig.clip(tracks[0], scenes[1], 3);
        rig.settle();
        assert!(is_static(rig.keys.button(0x69), WHITE), "clips");
        assert!(
            rig.keys.pad(0, 1).is_some(),
            "the bottom row holds its clips"
        );
        let order = if which.mk4() {
            [("Clip Stop", RED), ("Mute", YELLOW), ("Solo", BLUE)]
        } else {
            [("Clip Stop", RED), ("Solo", BLUE), ("Mute", YELLOW)]
        };
        for (name, colour) in order {
            rig.tap_button(0x69);
            assert!(
                is_static(rig.keys.button(0x69), colour),
                "{name}: {:?}",
                rig.keys.button(0x69)
            );
            assert!(
                rig.mode().contains(&format!("track buttons: {name}")),
                "{}",
                rig.mode()
            );
        }
        // The last stop of the cycle: Mute on the MK3, Solo on the MK4.
        if which.mk4() {
            assert!(is_static(rig.keys.pad(0, 1), DIM_BLUE));
            rig.tap(0, 1);
            assert!(rig.mixer(tracks[0]).solo);
            assert!(is_static(rig.keys.pad(0, 1), BLUE), "bright while soloed");
        } else {
            assert!(is_static(rig.keys.pad(0, 1), YELLOW), "heard");
            rig.tap(0, 1);
            assert!(rig.mixer(tracks[0]).mute);
            assert!(is_static(rig.keys.pad(0, 1), DIM_YELLOW), "dim while muted");
        }
        assert_eq!(rig.keys.pad(5, 1), None, "past the last track");
        rig.tap_button(0x69);
        assert!(is_static(rig.keys.button(0x69), WHITE), "back to clips");
        assert!(
            matches!(rig.keys.pad(0, 1), Some(Light::Static(c)) if c != DIM_BLUE && c != DIM_YELLOW)
        );
    }
}

const UTILITY: &str = "beatlayer.utility";

impl Rig {
    /// A device's banks by name, each with its parameter IDs, names and
    /// value texts.
    fn banks(&mut self, device: DeviceId) -> Vec<Value> {
        let banks = self.run("control_surface.banks", json!({ "device": device }));
        banks["banks"].as_array().unwrap().clone()
    }

    /// Two audio tracks and `returns` returns, the first audio track selected
    /// with a Utility on it.
    fn audio_with_utility(&mut self, returns: usize) -> (Vec<TrackId>, DeviceId) {
        for name in ["A", "B"] {
            self.invoke("track.create", json!({ "kind": "audio", "name": name }));
        }
        for n in 0..returns {
            self.invoke(
                "track.create",
                json!({ "kind": "return", "name": format!("R{n}") }),
            );
        }
        let doc = self.shell.editor().document();
        let tracks: Vec<TrackId> = doc.top_level().collect();
        self.select(tracks[0]);
        let utility = self.add(UTILITY, "audio_effect");
        self.settle();
        (tracks, utility)
    }
}

// Spec: supported-controllers R54, R58, R64, R68. The pots (MK3) and
// encoders (MK4) start on the device in focus's bank and follow the mode the
// controller reports (Volume, Pan, the sends; on the MK4 Mixer's pages and
// Sends), reaching the parameter or the ring's track under Takeover Mode,
// and the display shows the parameter's name and value as it moves.
#[test]
fn the_pots_follow_the_reported_mode_and_the_display_shows_what_they_reach() {
    for which in [Which::Mk3(Size::K61), Which::Mk4(Size::K61)] {
        let mut rig = Rig::new(which);
        rig.run("settings.set_takeover_mode", json!({ "mode": "none" }));
        let (tracks, utility) = rig.audio_with_utility(2);
        let banks = rig.banks(utility);
        let width = banks[0]["params"]
            .as_array()
            .unwrap()
            .iter()
            .position(|p| p["param"] == 6)
            .unwrap() as u8;
        let device_mode = if which.mk4() {
            "encoders: Plugin"
        } else {
            "pots: Device"
        };
        assert!(rig.mode().contains(device_mode), "{}", rig.mode());

        // Device: the bank's parameter, and the display.
        rig.run(
            "device.set_param",
            json!({ "device": utility, "param": 6, "value": 100.0 }),
        );
        let before = rig.param(utility, 6);
        rig.keys.pot_to(width, 0);
        rig.settle();
        assert_ne!(rig.param(utility, 6), before);
        let banks = rig.banks(utility);
        let param = &banks[0]["params"][usize::from(width)];
        let (name, text) = (
            param["name"].as_str().unwrap().to_owned(),
            param["text"].as_str().unwrap_or_default().to_owned(),
        );
        if which.mk4() {
            let screen = rig.keys.screen(0x15 + width);
            assert_eq!(screen.config, 0x62);
            assert_eq!(screen.fields[1], name);
            assert_eq!(screen.fields[2], text);
        } else {
            assert_eq!(rig.keys.name(0x38 + width), name);
            assert_eq!(rig.keys.value(0x38 + width), text);
            assert_eq!(rig.keys.row(0), &UTILITY[..16]);
            assert_eq!(rig.keys.row(1), banks[0]["name"].as_str().unwrap());
        }

        // Volume.
        rig.keys.set_pot_mode(1);
        rig.settle();
        let volume = rig.mixer(tracks[1]).volume;
        rig.keys.pot_to(1, 60);
        rig.settle();
        assert_ne!(rig.mixer(tracks[1]).volume, volume);
        let shown = if which.mk4() {
            rig.keys.screen(0x16).fields[2].clone()
        } else {
            rig.keys.value(0x39).to_owned()
        };
        assert!(shown.ends_with(" dB"), "{shown}");

        // Pan: a pot mode of its own on the MK3, a page on the MK4.
        if which.mk4() {
            rig.tap_button(0x34);
            assert!(rig.mode().contains("encoders: Pan"), "{}", rig.mode());
            assert_eq!(rig.keys.screen(0x24).fields, ["Pan"]);
        } else {
            rig.keys.set_pot_mode(3);
            rig.settle();
        }
        rig.keys.pot_to(0, 127);
        rig.settle();
        assert!((rig.mixer(tracks[0]).pan - 1.0).abs() < 1e-9);

        // Sends: Send A and, on the MK3, Send B; on the MK4 the bank
        // buttons step the send.
        let returns: Vec<TrackId> = rig.shell.editor().document().returns().collect();
        if which.mk4() {
            rig.keys.set_pot_mode(4);
            rig.settle();
            rig.keys.pot_to(0, 127);
            rig.settle();
            let doc = rig.shell.editor().document();
            assert!(doc.track(tracks[0]).unwrap().send(returns[0]) > 0.0);
            rig.tap_button(0x34);
            assert!(rig.mode().contains("encoders: Send B"), "{}", rig.mode());
            rig.keys.pot_to(0, 127);
            rig.settle();
        } else {
            rig.keys.set_pot_mode(4);
            rig.settle();
            rig.keys.pot_to(0, 127);
            rig.settle();
            assert!(rig.mode().contains("pots: Send A"));
            rig.keys.set_pot_mode(5);
            rig.settle();
            rig.keys.pot_to(0, 127);
            rig.settle();
            assert!(rig.mode().contains("pots: Send B"));
        }
        let doc = rig.shell.editor().document();
        assert!(
            doc.track(tracks[0]).unwrap().send(returns[1]) > 0.0,
            "Send B"
        );

        // A mode of the controller's own: the pots reach nothing.
        let (applied, refused) = rig.shell.surface_counts();
        rig.keys.set_pot_mode(if which.mk4() { 6 } else { 7 });
        rig.settle();
        rig.keys.pot_to(0, 5);
        rig.settle();
        assert_eq!(rig.shell.surface_counts(), (applied, refused));
    }
}

// Spec: supported-controllers R54, R64. After a bank change the pots follow
// Takeover Mode: with Pick-Up a pot moves nothing until it crosses the value.
#[test]
fn a_pot_after_a_mode_change_follows_takeover_mode() {
    for which in [Which::Mk3(Size::K61), Which::Mk4(Size::K61)] {
        let mut rig = Rig::new(which);
        rig.run("settings.set_takeover_mode", json!({ "mode": "pick_up" }));
        let (_, utility) = rig.audio_with_utility(0);
        let banks = rig.banks(utility);
        let width = banks[0]["params"]
            .as_array()
            .unwrap()
            .iter()
            .position(|p| p["param"] == 6)
            .unwrap() as u8;
        rig.run(
            "device.set_param",
            json!({ "device": utility, "param": 6, "value": 100.0 }),
        );
        let at = rig.param(utility, 6);
        rig.keys.pot_to(width, 0);
        rig.settle();
        assert_eq!(rig.param(utility, 6), at, "the pot has to meet the value");
        rig.keys.pot_to(width, 127);
        rig.settle();
        assert_ne!(rig.param(utility, 6), at, "it crossed it");
    }
}

// Spec: supported-controllers R55, R66, R58. On the controllers with faders,
// the first eight set the ring's tracks' volume and the ninth the Main
// track's (on the MK3 also the device in focus's bank, or a send, in the
// mode the controller reports); the fader buttons arm or select the ring's
// tracks, the ninth switching them, Arm at first.
#[test]
fn the_faders_and_the_fader_buttons_arm_and_select() {
    for which in [Which::Mk3(Size::K61), Which::Mk4(Size::K61)] {
        let mut rig = Rig::new(which);
        rig.run("settings.set_takeover_mode", json!({ "mode": "none" }));
        let (tracks, utility) = rig.audio_with_utility(1);
        let volume = rig.mixer(tracks[1]).volume;
        rig.keys.fader_to(1, 60);
        rig.settle();
        assert_ne!(rig.mixer(tracks[1]).volume, volume);
        let main = rig.shell.editor().document().main_id();
        rig.keys.fader_to(8, 0);
        rig.settle();
        assert_eq!(rig.mixer(main).volume, 0.0, "the ninth fader is Main's");
        let shown = if which.mk4() {
            rig.keys.screen(0x06).fields.clone()
        } else {
            vec![
                String::new(),
                rig.keys.name(0x51).to_owned(),
                rig.keys.value(0x51).to_owned(),
            ]
        };
        assert!(shown[2].ends_with(" dB"), "{shown:?}");

        if !which.mk4() {
            // Device: the bank's parameters; Send A.
            rig.keys.set_fader_mode(2);
            rig.run(
                "device.set_param",
                json!({ "device": utility, "param": 6, "value": 100.0 }),
            );
            rig.settle();
            let before = rig.param(utility, 6);
            let banks = rig.banks(utility);
            let width = banks[0]["params"]
                .as_array()
                .unwrap()
                .iter()
                .position(|p| p["param"] == 6)
                .unwrap() as u8;
            rig.run("settings.set_takeover_mode", json!({ "mode": "none" }));
            rig.keys.fader_to(width, 0);
            rig.settle();
            assert_ne!(rig.param(utility, 6), before);
            rig.keys.set_fader_mode(4);
            rig.settle();
            rig.keys.fader_to(0, 127);
            rig.settle();
            let returns: Vec<TrackId> = rig.shell.editor().document().returns().collect();
            let doc = rig.shell.editor().document();
            assert!(doc.track(tracks[0]).unwrap().send(returns[0]) > 0.0);
            assert!(rig.mode().contains("faders: Send A"), "{}", rig.mode());
        }

        // Arm: bright red when armed, dim when not, dark past the last track.
        assert!(
            rig.mode().contains("fader buttons: Rec Arm"),
            "{}",
            rig.mode()
        );
        assert!(is_static(rig.keys.button(0x25), DIM_RED));
        assert_eq!(rig.keys.button(0x2C), None, "past the last track");
        rig.keys.fader_button(0);
        rig.settle();
        assert!(rig.mixer(tracks[0]).arm);
        assert!(is_static(rig.keys.button(0x25), RED));
        rig.keys.fader_button(0);
        rig.settle();
        assert!(!rig.mixer(tracks[0]).arm);

        // Select: the switch, then a track's own colour.
        rig.keys.fader_button(8);
        rig.settle();
        assert!(
            rig.mode().contains("fader buttons: Select"),
            "{}",
            rig.mode()
        );
        rig.keys.fader_button(1);
        rig.settle();
        assert_eq!(rig.selected(), [tracks[1]]);
        assert!(
            matches!(rig.keys.button(0x26), Some(Light::Static(c)) if c != DIM_RED && c != RED),
            "in the track's colour"
        );
    }
}

// Spec: supported-controllers R51, R61. A controller without faders gets no
// fader mode, and its fader controllers do nothing.
#[test]
fn a_controller_without_faders_ignores_them() {
    for which in [Which::Mk3(Size::K25), Which::Mk4(Size::Mini)] {
        let mut rig = Rig::new(which);
        let tracks = rig.tracks(2);
        rig.settle();
        let volume = rig.mixer(tracks[0]).volume;
        rig.keys.fader_to(0, 5);
        rig.settle();
        assert_eq!(rig.mixer(tracks[0]).volume, volume);
        assert!(!rig.mode().contains("faders"), "{}", rig.mode());
        assert_eq!(rig.keys.button(0x25), None);
    }
}

// Spec: supported-controllers R56. While Device Select is held, the MK3's
// pads are its Device Select pads, lit purple for each bank of the device in
// focus (the shown one white) and each choosing a bank; ▲ and ▼ select the
// previous and next device; Device Lock locks the device in focus to the
// surface or gives it back, lit white while locked.
#[test]
fn device_select_chooses_banks_and_devices_and_device_lock_locks() {
    let mut rig = Rig::new(Which::Mk3(Size::K61));
    let (_, first) = rig.audio_with_utility(0);
    let second = rig.add("beatlayer.eq_eight", "audio_effect");
    rig.run("device.select", json!({ "device": first }));
    rig.settle();
    let banks = rig.banks(first).len();
    assert!(banks >= 2, "the Utility has {banks} banks");

    rig.keys.hold_device_select();
    rig.settle();
    assert_eq!(rig.mode().split(';').next(), Some("Device Select"));
    assert!(is_static(
        rig.keys.light(bl_control::launchkey::Layer::Pad, 0x40),
        WHITE
    ));
    assert!(is_static(
        rig.keys.light(bl_control::launchkey::Layer::Pad, 0x41),
        PURPLE
    ));
    assert_eq!(
        rig.keys
            .light(bl_control::launchkey::Layer::Pad, 0x40 + banks as u8),
        None,
        "no bank there"
    );
    assert_eq!(rig.keys.pad(0, 0), None, "the clip pads are gone");
    rig.keys.select_pad_down(1);
    rig.settle();
    assert_eq!(rig.row()["surface_state"]["device"]["bank"], 1);
    assert!(is_static(
        rig.keys.light(bl_control::launchkey::Layer::Pad, 0x41),
        WHITE
    ));

    // ▼ is the next device, ▲ the previous.
    rig.tap_button(0x6B);
    assert_eq!(
        rig.row()["surface_state"]["device"]["device"],
        json!(second)
    );
    rig.tap_button(0x6A);
    assert_eq!(rig.row()["surface_state"]["device"]["device"], json!(first));
    rig.keys.release_device_select();
    rig.settle();
    assert_eq!(rig.mode().split(';').next(), Some("Session"));

    // Device Lock.
    assert_eq!(rig.keys.button(0x34), None);
    rig.tap_button(0x34);
    assert_eq!(rig.row()["surface_state"]["device"]["locked"], "hand");
    assert!(is_static(rig.keys.button(0x34), WHITE));
    rig.run("device.select", json!({ "device": second }));
    rig.settle();
    assert_eq!(
        rig.row()["surface_state"]["device"]["device"],
        json!(first),
        "locked"
    );
    rig.tap_button(0x34);
    assert!(rig.row()["surface_state"]["device"]["locked"].is_null());
    assert_eq!(rig.keys.button(0x34), None);
}

// Spec: supported-controllers R64. In Plugin mode the MK4's bank buttons step
// the device's banks and, with Shift, select the previous and next device.
#[test]
fn the_mk4_bank_buttons_step_banks_and_with_shift_devices() {
    let mut rig = Rig::new(Which::Mk4(Size::K61));
    let (_, first) = rig.audio_with_utility(0);
    let second = rig.add("beatlayer.eq_eight", "audio_effect");
    rig.run("device.select", json!({ "device": first }));
    rig.settle();
    assert_eq!(rig.row()["surface_state"]["device"]["bank"], 0);
    rig.tap_button(0x34);
    assert_eq!(rig.row()["surface_state"]["device"]["bank"], 1);
    rig.tap_button(0x33);
    assert_eq!(rig.row()["surface_state"]["device"]["bank"], 0);
    rig.press(0x3F);
    rig.tap_button(0x34);
    assert_eq!(
        rig.row()["surface_state"]["device"]["device"],
        json!(second)
    );
    rig.tap_button(0x33);
    assert_eq!(rig.row()["surface_state"]["device"]["device"], json!(first));
    rig.release(0x3F);
}

fn transport(rig: &mut Rig) -> Value {
    // The engine reports a change a block later, and the surface sees it on
    // its next tick.
    testing::advance(&rig.control, &mut rig.shell, 20);
    rig.settle();
    rig.run("transport.state", json!({}))
}

// Spec: supported-controllers R65, R68. In Transport mode the MK4's relative
// encoders reach the play position, the zoom (the selection across tracks in
// the Session View), the track height (across scenes), the loop's start, its
// end, the locators, nothing, and the tempo; the screen names what turned.
#[test]
fn the_transport_encoders_reach_position_zoom_height_loop_locators_and_tempo() {
    let mut rig = Rig::new(Which::Mk4(Size::K61));
    let tracks = rig.tracks(3);
    rig.settle();
    rig.keys.set_pot_mode(5);
    rig.settle();
    assert!(rig.mode().contains("encoders: Transport"), "{}", rig.mode());
    let beat = 705_600;

    // The play position, a beat a step.
    rig.keys.encoder_turn(0, 3);
    rig.settle();
    assert_eq!(transport(&mut rig)["insert_marker"], 3 * beat);
    assert_eq!(rig.keys.screen(0x21).fields, ["Position", "beat 4"]);
    rig.keys.encoder_turn(0, -1);
    rig.settle();
    assert_eq!(transport(&mut rig)["insert_marker"], 2 * beat);

    // The loop brace: start and end.
    let state = transport(&mut rig);
    let (start, length) = (
        state["loop_start"].as_i64().unwrap(),
        state["loop_length"].as_i64().unwrap(),
    );
    rig.keys.encoder_turn(3, 2);
    rig.settle();
    let state = transport(&mut rig);
    assert_eq!(state["loop_start"], start + 2 * beat);
    assert_eq!(state["loop_length"], length - 2 * beat, "the end stays");
    rig.keys.encoder_turn(4, 1);
    rig.settle();
    assert_eq!(transport(&mut rig)["loop_length"], length - 2 * beat + beat);

    // The tempo, a BPM a step; the gesture is one undo step.
    let bpm = transport(&mut rig)["tempo"].as_f64().unwrap();
    rig.keys.encoder_turn(7, 2);
    rig.keys.encoder_turn(7, 1);
    rig.settle();
    assert!((transport(&mut rig)["tempo"].as_f64().unwrap() - bpm - 3.0).abs() < 1e-9);
    assert_eq!(rig.keys.screen(0x21).fields[0], "Tempo");

    // The locators, the zoom and the height run their commands; the
    // seventh encoder reaches nothing.
    let ran = |rig: &Rig| {
        let (applied, refused) = rig.shell.surface_counts();
        applied + refused
    };
    for index in [1, 2, 5] {
        let before = ran(&rig);
        rig.keys.encoder_turn(index, 1);
        rig.settle();
        assert_eq!(ran(&rig), before + 1, "encoder {index}");
    }
    let before = ran(&rig);
    rig.keys.encoder_turn(6, 1);
    rig.settle();
    assert_eq!(ran(&rig), before);

    // In the Session View the second and third move the selection across
    // tracks and scenes.
    rig.invoke("view.toggle_session_arrangement", json!({}));
    rig.invoke(
        "arrangement.select_tracks",
        json!({ "tracks": [tracks[0]] }),
    );
    rig.settle();
    rig.keys.encoder_turn(1, 2);
    rig.settle();
    assert_eq!(rig.selected(), [tracks[2]]);
    rig.keys.encoder_turn(1, -1);
    rig.settle();
    assert_eq!(rig.selected(), [tracks[1]]);
    let scenes = rig.scenes();
    rig.keys.encoder_turn(2, 1);
    rig.settle();
    rig.keys.encoder_turn(2, 1);
    rig.settle();
    let state = rig.run("session.state", json!({}));
    assert_eq!(state["scenes"][0], json!(scenes[1]), "{state}");
}

// Spec: supported-controllers R65. Turning the loop's start on a loop shorter
// than a beat leaves the start where it is, and the surface runs on.
#[test]
fn the_loop_start_encoder_keeps_a_loop_shorter_than_a_beat() {
    let mut rig = Rig::new(Which::Mk4(Size::K61));
    rig.keys.set_pot_mode(5);
    rig.settle();
    let beat = 705_600;
    rig.invoke(
        "transport.set_loop",
        json!({ "start": 0, "length": beat / 4 }),
    );
    rig.settle();
    rig.keys.encoder_turn(3, 1);
    rig.settle();
    assert_eq!(transport(&mut rig)["loop_start"], 0);
    assert_eq!(rig.row()["state"], "running");
}

fn ran(rig: &Rig) -> u64 {
    let (applied, refused) = rig.shell.surface_counts();
    applied + refused
}

fn playing(rig: &mut Rig) -> bool {
    transport(rig)["playing"] == true
}

// Spec: supported-controllers R57, R67. Play starts the transport, Stop stops
// it and, pressed again, returns to the start; Loop switches the Arrangement
// loop; Click the metronome; Undo undoes and, with Shift on the MK4, redoes;
// Capture MIDI runs Capture MIDI; with Shift Play pauses and continues.
#[test]
fn the_transport_and_workflow_buttons_run_their_commands() {
    for which in [Which::Mk3(Size::K61), Which::Mk4(Size::K61)] {
        let mut rig = Rig::new(which);
        let tracks = rig.tracks(2);
        rig.settle();

        rig.tap_button(0x73);
        testing::wait_for(&rig.control, &mut rig.shell, "play", |s| {
            s.arrangement().transport().playing
        });
        assert!(playing(&mut rig));
        rig.tap_button(0x74);
        testing::wait_for(&rig.control, &mut rig.shell, "stop", |s| {
            !s.arrangement().transport().playing
        });
        let stopped = rig.shell.arrangement().transport().position;
        assert!(stopped.get() > 0, "it played a while");
        rig.tap_button(0x74);
        testing::advance(&rig.control, &mut rig.shell, 20);
        assert_eq!(
            rig.shell.arrangement().transport().position.get(),
            0,
            "a second Stop returns to the start"
        );

        // Loop.
        assert_eq!(transport(&mut rig)["loop_on"], false);
        rig.tap_button(0x76);
        assert_eq!(transport(&mut rig)["loop_on"], true);
        rig.tap_button(0x76);
        assert_eq!(transport(&mut rig)["loop_on"], false);

        // Click.
        assert!(!rig.shell.editor().document().settings().metronome.enabled);
        rig.tap_button(0x4C);
        assert!(rig.shell.editor().document().settings().metronome.enabled);
        rig.tap_button(0x4C);
        assert!(!rig.shell.editor().document().settings().metronome.enabled);

        // Undo, and Redo with Shift (MK4).
        rig.run(
            "mixer.set_volume",
            json!({ "track": tracks[0], "volume": 0.5 }),
        );
        rig.tap_button(0x4D);
        assert!((rig.mixer(tracks[0]).volume - 0.5).abs() > 1e-9, "undone");
        if which.mk4() {
            rig.press(0x3F);
            rig.tap_button(0x4D);
            rig.release(0x3F);
            assert!((rig.mixer(tracks[0]).volume - 0.5).abs() < 1e-9, "redone");
        }

        // Capture MIDI.
        let before = ran(&rig);
        rig.tap_button(0x4A);
        assert_eq!(ran(&rig), before + 1);

        // Shift with Play pauses the playing song and continues it.
        rig.tap_button(0x73);
        testing::wait_for(&rig.control, &mut rig.shell, "play", |s| {
            s.arrangement().transport().playing
        });
        if which.mk4() {
            rig.press(0x3F);
            rig.tap_button(0x73);
            testing::wait_for(&rig.control, &mut rig.shell, "pause", |s| {
                !s.arrangement().transport().playing
            });
            let paused = rig.shell.arrangement().transport().position;
            assert!(paused.get() > 0, "the position stays");
            rig.settle();
            rig.tap_button(0x73);
            testing::wait_for(&rig.control, &mut rig.shell, "continue", |s| {
                s.arrangement().transport().playing
            });
            rig.release(0x3F);
        }
    }
}

// Spec: supported-controllers R57, R67. Record switches Session Record; on
// the MK4 it follows the view: Arrangement recording while the Arrangement
// shows, Session Record while the Session View does.
#[test]
fn record_is_session_record_in_the_session_view_and_on_the_mk3() {
    for which in [Which::Mk3(Size::K61), Which::Mk4(Size::K61)] {
        let mut rig = Rig::new(which);
        let tracks = rig.tracks(2);
        rig.run("mixer.set_arm", json!({ "track": tracks[1], "arm": true }));
        rig.quantisation_off();
        let scene = rig.scenes()[0];
        rig.run("session.select", json!({ "scene": scene }));
        if which.mk4() {
            rig.invoke("view.toggle_session_arrangement", json!({}));
        }
        rig.settle();
        rig.run("transport.play", json!({}));
        rig.tap_button(0x75);
        let track = tracks[1];
        testing::wait_for(&rig.control, &mut rig.shell, "the recording", |shell| {
            shell
                .session_states()
                .iter()
                .any(|s| s.track == track && s.state == bl_engine::SlotState::Recording)
        });
        assert_eq!(transport(&mut rig)["arrangement_record"], false);
    }
}

// Spec: supported-controllers R67. While the Arrangement shows, the MK4's
// Record switches Arrangement recording.
#[test]
fn the_mk4s_record_switches_arrangement_recording_while_the_arrangement_shows() {
    let mut rig = Rig::new(Which::Mk4(Size::K61));
    let tracks = rig.tracks(2);
    rig.run("mixer.set_arm", json!({ "track": tracks[1], "arm": true }));
    rig.settle();
    rig.tap_button(0x75);
    assert_eq!(transport(&mut rig)["arrangement_record"], true);
    assert!(
        rig.shell.session_states().is_empty(),
        "Session Record is off"
    );
    rig.tap_button(0x75);
    assert_eq!(transport(&mut rig)["arrangement_record"], false);
}

// Spec: supported-controllers R57. Quantise quantises the notes of the clip
// playing on the selected track to sixteenths.
#[test]
fn quantise_quantises_the_playing_clip_of_the_selected_track() {
    for which in [Which::Mk3(Size::K61), Which::Mk4(Size::K61)] {
        let mut rig = Rig::new(which);
        let tracks = rig.tracks(2);
        let scenes = rig.scenes();
        rig.quantisation_off();
        rig.clip(tracks[0], scenes[0], 3);
        rig.run(
            "session.select",
            json!({ "track": tracks[0], "scene": scenes[0] }),
        );
        rig.run(
            "note.add",
            json!({ "key": 60, "start": 50_000, "length": 100_000 }),
        );
        rig.run("transport.play", json!({}));
        rig.tap(0, 0);
        let (track, scene) = (tracks[0], scenes[0]);
        testing::wait_for(&rig.control, &mut rig.shell, "the clip to play", |shell| {
            shell.session_states().iter().any(|s| {
                s.track == track && s.scene == scene && s.state == bl_engine::SlotState::Playing
            })
        });
        rig.select(tracks[0]);
        rig.settle();
        rig.tap_button(0x4B);
        let clip = rig
            .shell
            .editor()
            .document()
            .slot(tracks[0], scenes[0])
            .unwrap()
            .clone();
        let bl_model::ClipData::Midi(midi) = &clip.data else {
            panic!("a MIDI clip");
        };
        assert_eq!(midi.notes[0].start, bl_core::time::Ticks(0), "quantised");
    }
}

// Spec: supported-controllers R67. On a Mini, Play starts and stops the
// transport and Shift with Record runs Capture MIDI.
#[test]
fn on_a_mini_play_starts_and_stops_and_shift_record_captures() {
    let mut rig = Rig::new(Which::Mk4(Size::Mini));
    rig.tracks(1);
    rig.settle();
    rig.tap_button(0x73);
    testing::wait_for(&rig.control, &mut rig.shell, "play", |s| {
        s.arrangement().transport().playing
    });
    rig.tap_button(0x73);
    testing::wait_for(&rig.control, &mut rig.shell, "stop", |s| {
        !s.arrangement().transport().playing
    });
    let before = ran(&rig);
    rig.press(0x3F);
    rig.tap_button(0x75);
    rig.release(0x3F);
    assert_eq!(ran(&rig), before + 1, "Capture MIDI ran");
    assert_eq!(transport(&mut rig)["arrangement_record"], false);
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

/// The palette entry the colour of `track` lights.
fn colour_of(rig: &Rig, track: TrackId) -> u8 {
    let doc = rig.shell.editor().document();
    let swatch = doc.track(track).unwrap().colour();
    bl_control::grid::nearest(bl_model::swatch_rgb(
        &bl_model::DEFAULT_TRACK_PALETTE,
        swatch,
    ))
}

// Spec: supported-controllers R59. In Drum mode the MK3 lights its sixteen
// drum pads (notes 36 to 51, which the controller plays itself) on channel
// 10 in the selected track's colour where a drum rack pad holds a chain, and
// white for the selected one; another track's pads are dark.
#[test]
fn the_mk3_lights_its_drum_pads_from_the_drum_rack() {
    let mut rig = Rig::new(Which::Mk3(Size::K61));
    let tracks = rig.tracks(2);
    rig.select(tracks[0]);
    let rack = drum_rack(&mut rig, &[36, 37, 52]);
    rig.keys.set_pad_mode(1);
    rig.settle();
    assert!(rig.mode().starts_with("Drum"), "{}", rig.mode());
    let own = colour_of(&rig, tracks[0]);
    assert!(is_static(rig.keys.drum_pad(36), own), "36 holds a chain");
    assert!(is_static(rig.keys.drum_pad(37), own));
    assert_eq!(rig.keys.drum_pad(38), None, "38 is empty");
    assert_eq!(rig.keys.pad(0, 0), None, "the clip pads aren't shown");

    rig.run("pad.select", json!({ "rack": rack, "note": 37 }));
    rig.settle();
    assert!(is_static(rig.keys.drum_pad(37), WHITE), "the selected pad");
    assert_eq!(pad_view(&rig, rack).1, Some(37));

    // Another track: no drum rack, no lights.
    rig.select(tracks[1]);
    rig.settle();
    assert_eq!(rig.keys.drum_pad(36), None);
}

// Spec: supported-controllers R70. In Drum mode the MK4 claims the drum pads
// while the selected track has a drum rack: its pads' notes come from channel
// 10 and play the pads the pad view shows, lit in the track's colour where
// the rack holds a chain; the pad arrows move the pad view by sixteen; the
// pads are released when the track has no drum rack.
#[test]
fn the_mk4_claims_the_drum_pads_while_the_selected_track_has_a_drum_rack() {
    let mut rig = Rig::new(Which::Mk4(Size::K61));
    let tracks = rig.tracks(2);
    rig.select(tracks[0]);
    let rack = drum_rack(&mut rig, &[36, 37, 52]);
    rig.settle();
    assert!(!rig.keys.drum_claimed(), "the pads are the controller's");
    rig.keys.set_pad_mode(1);
    rig.settle();
    assert!(rig.keys.drum_claimed());
    assert!(rig.mode().starts_with("Drum"), "{}", rig.mode());
    let own = colour_of(&rig, tracks[0]);
    assert!(is_static(rig.keys.drum_pad(36), own));
    assert!(is_static(rig.keys.drum_pad(37), own));
    assert_eq!(rig.keys.drum_pad(38), None);

    // A pad plays and selects its rack pad.
    rig.keys.drum_down(37);
    rig.keys.drum_up(37);
    rig.settle();
    assert_eq!(pad_view(&rig, rack).1, Some(37));
    assert!(is_static(rig.keys.drum_pad(37), WHITE));

    // ▲ moves the pad view up by sixteen: 52 is the first pad.
    assert!(is_static(rig.keys.button(0x6A), WHITE));
    rig.tap_button(0x6A);
    assert_eq!(pad_view(&rig, rack).0, 52);
    assert!(is_static(rig.keys.drum_pad(36), own), "52 holds a chain");
    assert_eq!(rig.keys.drum_pad(37), None);
    rig.tap_button(0x6B);
    assert_eq!(pad_view(&rig, rack).0, 36);

    // The ring stays where it is, and another track gives the pads back.
    assert_eq!(rig.ring(), (0, 0));
    rig.select(tracks[1]);
    rig.settle();
    assert!(!rig.keys.drum_claimed());
}

// Spec: supported-controllers R60, R71. The controller's own pad modes
// (Scale Chords, User Chords, Custom, Navigation; the MK4's Sequencer, Arp
// Pattern, Chord Map, User Chord and Custom) leave the ring where it was:
// the pads aren't lit and pressing them does nothing.
#[test]
fn the_controllers_own_pad_modes_leave_the_ring_alone() {
    for (which, own_modes) in [
        (Which::Mk3(Size::K61), vec![0u8, 3, 4, 5, 10]),
        (Which::Mk4(Size::K61), vec![4, 5, 0x0D, 0x0E]),
    ] {
        let mut rig = Rig::new(which);
        let tracks = rig.tracks(3);
        let scenes = rig.scenes();
        rig.clip(tracks[0], scenes[0], 3);
        rig.settle();
        assert!(rig.keys.pad(0, 0).is_some());
        for mode in own_modes {
            rig.keys.set_pad_mode(mode);
            rig.settle();
            assert!(rig.mode().starts_with("Custom"), "{mode}: {}", rig.mode());
            assert_eq!(rig.keys.pad(0, 0), None, "dark in mode {mode}");
            let before = ran(&rig);
            rig.tap(0, 0);
            assert_eq!(ran(&rig), before, "a press does nothing in mode {mode}");
            assert_eq!(rig.ring(), (0, 0));
        }
        rig.keys.set_pad_mode(2);
        rig.settle();
        assert!(rig.keys.pad(0, 0).is_some(), "Session is back");
    }
}

// Spec: supported-controllers R69. The MK4 is sent the set's scale when it
// starts and whenever it changes: Scale Mode on or off, the root and the
// scale's place in the controller's list.
#[test]
fn the_mk4_is_sent_the_sets_scale_on_a_change() {
    let mut rig = Rig::new(Which::Mk4(Size::K61));
    rig.settle();
    assert_eq!(rig.keys.scale(), (false, 0, 0), "the start sends it");
    rig.run(
        "song.set_scale",
        json!({ "enabled": true, "root": 2, "name": "dorian" }),
    );
    rig.settle();
    assert_eq!(rig.keys.scale(), (true, 2, 2));
    rig.run(
        "song.set_scale",
        json!({ "root": 9, "name": "melodic_minor" }),
    );
    rig.settle();
    assert_eq!(rig.keys.scale(), (true, 9, 17));
    rig.run("song.set_scale", json!({ "enabled": false }));
    rig.settle();
    assert!(!rig.keys.scale().0);
}

// Spec: supported-controllers R58, R68. The MK3's default display holds the
// device in focus and its bank's name, and the MK4's stationary display the
// selected track and the device in focus, with its mode names.
#[test]
fn the_displays_hold_the_selected_track_and_the_device_in_focus() {
    let mut rig = Rig::new(Which::Mk3(Size::K61));
    let (_, utility) = rig.audio_with_utility(0);
    let name = rig
        .shell
        .editor()
        .document()
        .device(utility)
        .unwrap()
        .name
        .clone();
    let bank = rig.banks(utility)[0]["name"].as_str().unwrap().to_owned();
    assert_eq!(rig.keys.row(0), &name[..16]);
    assert_eq!(rig.keys.row(1), bank);

    let mut rig = Rig::new(Which::Mk4(Size::K61));
    let (tracks, utility) = rig.audio_with_utility(0);
    let device = rig
        .shell
        .editor()
        .document()
        .device(utility)
        .unwrap()
        .name
        .clone();
    assert_eq!(rig.keys.screen(0x20).fields, ["A", &device[..16]]);
    rig.select(tracks[1]);
    rig.settle();
    assert_eq!(rig.keys.screen(0x20).fields[0], "B");
    assert_eq!(rig.keys.screen(0x22).fields, ["Session"]);
    assert_eq!(rig.keys.screen(0x27).fields, ["Transport"]);
}

// Spec: supported-controllers R17, R51, R61. Removing the row puts out every
// light, gives the display back and sends the controller DAW mode off.
#[test]
fn removing_the_row_puts_out_every_light_and_leaves_daw_mode() {
    for which in [Which::Mk3(Size::K61), Which::Mk4(Size::K61)] {
        let mut rig = Rig::new(which);
        let tracks = rig.tracks(2);
        let scenes = rig.scenes();
        rig.clip(tracks[0], scenes[0], 3);
        rig.settle();
        assert!(rig.keys.lit() > 0);
        rig.run("control_surface.set", json!({ "row": 1, "surface": null }));
        rig.sync();
        assert!(!rig.keys.daw_mode_on());
        assert_eq!(rig.keys.lit_when_left(), Some(0), "dark before it let go");
        assert_eq!(
            rig.keys.messages().last(),
            Some(&vec![0x9F, 0x0C, 0x00]),
            "DAW mode off"
        );
        if !which.mk4() {
            assert!(rig.keys.display_cleared());
        }
    }
}

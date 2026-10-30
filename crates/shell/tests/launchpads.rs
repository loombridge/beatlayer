//! The grid surface and the Launchpad X, Mini MK3 and Pro MK3
//! (WP-7.8; `docs/specs/supported-controllers.md`, R1 to R34 and R72): each
//! controller is a simulated device on the in-process test backend, with the
//! engine paced by hand, so every rule runs headless.

use bl_audio_io::NullControl;
use bl_control::grid::sim::Launchpad;
use bl_control::grid::{Light, nearest};
use bl_midi_io::TestBackend;
use bl_model::{DeviceId, SceneId, TrackId, TrackKind};
use bl_shell::{Shell, testing};
use serde_json::{Value, json};

/// Palette entries the surfaces name.
const GREEN: u8 = 21;
const RED: u8 = 5;
const DIM_RED: u8 = 7;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Which {
    X,
    Mini,
    Pro,
}

struct Rig {
    shell: Shell,
    control: NullControl,
    test: TestBackend,
    pad: Launchpad,
    which: Which,
}

impl Rig {
    /// A set with eight MIDI tracks and eight scenes, and a controller
    /// plugged in and recognised.
    fn new(which: Which) -> Self {
        let mut rig = Self::unplugged(which);
        rig.plug_in();
        rig
    }

    fn unplugged(which: Which) -> Self {
        let (mut shell, control) = testing::rig();
        let test = TestBackend::new();
        let pad = match which {
            Which::X => Launchpad::x(&test),
            Which::Mini => Launchpad::mini(&test),
            Which::Pro => Launchpad::pro(&test),
        };
        shell.use_midi_backend(Box::new(test.clone()));
        shell.start_audio();
        Self {
            shell,
            control,
            test,
            pad,
            which,
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
        self.pad.plug_in();
        self.shell.refresh_midi();
        for _ in 0..4 {
            self.pad.pump();
            self.shell.poll();
            self.shell.midi().surfaces().settle();
        }
        self.pad.pump();
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
        self.pad.pump();
    }

    /// As [`Rig::sync`], twice: a message's commands run on the first poll
    /// and what they changed shows on the second tick.
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

    /// The controller sends a press and a release of a Session pad.
    fn tap(&mut self, column: u8, row: u8) {
        self.pad.pad_down(column, row);
        self.pad.pad_up(column, row);
        self.settle();
    }

    fn press(&mut self, cc: u8) {
        self.pad.button_down(cc);
        self.settle();
    }

    fn release(&mut self, cc: u8) {
        self.pad.button_up(cc);
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
}

fn is_static(light: Option<Light>, colour: u8) -> bool {
    light == Some(Light::Static(colour))
}

// Spec: supported-controllers R2, R3, R18, R25, R29, R72. Plugging a
// controller in fills the first free row with its DAW ports, and the
// surface starts by sending DAW mode and the Session layout; the MIDI port
// stays an ordinary MIDI port that plays tracks.
#[test]
fn plugging_a_controller_in_fills_row_one_and_starts_it_in_daw_mode_and_session() {
    for (which, header, daw, midi, id) in [
        (
            Which::X,
            [0xF0, 0x00, 0x20, 0x29, 0x02, 0x0C],
            "LPX DAW",
            "LPX MIDI",
            "launchpad-x",
        ),
        (
            Which::Mini,
            [0xF0, 0x00, 0x20, 0x29, 0x02, 0x0D],
            "LPMiniMK3 DAW",
            "LPMiniMK3 MIDI",
            "launchpad-mini-mk3",
        ),
        (
            Which::Pro,
            [0xF0, 0x00, 0x20, 0x29, 0x02, 0x0E],
            "LPProMK3 DAW",
            "LPProMK3 MIDI",
            "launchpad-pro-mk3",
        ),
    ] {
        let mut rig = Rig::new(which);
        let row = rig.row();
        assert_eq!(row["surface"]["id"], id);
        assert_eq!(row["input"]["name"], daw);
        assert_eq!(row["output"]["name"], daw);
        assert_eq!(row["state"], "running");
        assert!(rig.pad.daw_mode());
        assert_eq!(rig.pad.layout(), (0, 0), "the Session layout");

        let wrap = |body: &[u8]| {
            let mut bytes = header.to_vec();
            bytes.extend_from_slice(body);
            bytes.push(0xF7);
            bytes
        };
        let session: &[u8] = if which == Which::Pro {
            &[0x00, 0x00, 0x00, 0x00]
        } else {
            &[0x00, 0x00]
        };
        assert_eq!(
            &rig.pad.messages()[..3],
            [
                vec![0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7],
                wrap(&[0x10, 0x01]),
                wrap(session)
            ],
            "the inquiry, DAW mode, then the Session layout"
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

// Spec: supported-controllers R10. A set of clips in four colours lights
// the pads with the palette entries nearest to them, by distance in RGB.
#[test]
fn clips_light_the_pads_with_the_nearest_palette_entries() {
    for which in [Which::X, Which::Mini, Which::Pro] {
        let mut rig = Rig::new(which);
        let tracks = rig.tracks(4);
        let scenes = rig.scenes();
        let swatches = [0u8, 3, 8, 11];
        for (n, &swatch) in swatches.iter().enumerate() {
            rig.clip(tracks[n], scenes[0], swatch);
        }
        rig.settle();
        for (n, &swatch) in swatches.iter().enumerate() {
            let rgb = bl_model::swatch_rgb(&bl_model::DEFAULT_TRACK_PALETTE, swatch);
            let want = nearest(rgb);
            assert!(
                is_static(rig.pad.pad(n as u8, 0), want),
                "the clip in colour {swatch} lights its pad with entry {want}, got {:?}",
                rig.pad.pad(n as u8, 0)
            );
        }
        // A slot with nothing in it is dark.
        assert_eq!(rig.pad.pad(0, 1), None);
        assert_eq!(rig.pad.pad(7, 7), None, "past the last track");
    }
}

// Spec: supported-controllers R9. A launched clip's pad flashes green while
// it waits for the grid and pulses green while it plays, on the frames the
// engine reports.
#[test]
fn launching_turns_a_pad_from_flashing_to_pulsing_green() {
    for which in [Which::X, Which::Mini, Which::Pro] {
        let mut rig = Rig::new(which);
        let tracks = rig.tracks(1);
        let scenes = rig.scenes();
        rig.clip(tracks[0], scenes[0], 3);
        rig.run(
            "session.set_quantization",
            json!({ "quantization": "1_bar" }),
        );
        rig.run("transport.play", json!({}));
        rig.settle();
        let before = rig.pad.pad(0, 0);
        assert!(matches!(before, Some(Light::Static(_))), "{before:?}");
        rig.tap(0, 0);
        testing::advance(&rig.control, &mut rig.shell, 20);
        assert!(rig.queued(tracks[0], scenes[0]), "the launch waits");
        rig.settle();
        match rig.pad.pad(0, 0) {
            Some(Light::Flash { b, .. }) => assert_eq!(b, GREEN),
            other => panic!("queued shows flashing green, got {other:?}"),
        }
        let (track, scene) = (tracks[0], scenes[0]);
        testing::wait_for(&rig.control, &mut rig.shell, "the clip to play", |shell| {
            shell
                .session_states()
                .iter()
                .any(|s| s.track == track && s.scene == scene && s.state == bl_engine::SlotState::Playing)
        });
        rig.settle();
        assert_eq!(rig.pad.pad(0, 0), Some(Light::Pulse(GREEN)));
    }
}

// Spec: supported-controllers R7. An empty pad on an armed track records
// into its slot, and on an unarmed one stops its track; a pad past the last
// track does nothing.
#[test]
fn an_empty_pad_records_on_an_armed_track_and_stops_an_unarmed_one() {
    let mut rig = Rig::new(Which::X);
    let tracks = rig.tracks(2);
    let scenes = rig.scenes();
    rig.quantisation_off();
    rig.clip(tracks[1], scenes[0], 3);
    rig.run("transport.play", json!({}));
    rig.tap(1, 0);
    let (track, scene) = (tracks[1], scenes[0]);
    testing::wait_for(&rig.control, &mut rig.shell, "the clip to play", |shell| {
        shell
            .session_states()
            .iter()
            .any(|s| s.track == track && s.scene == scene && s.state == bl_engine::SlotState::Playing)
    });
    // An unarmed track's empty slot stops its track.
    rig.tap(1, 1);
    testing::wait_for(&rig.control, &mut rig.shell, "the clip to stop", |shell| {
        shell.session_states().is_empty()
    });
    // An armed track's empty slot is dim red, and pressing it records.
    rig.run("mixer.set_arm", json!({ "track": tracks[0], "arm": true }));
    rig.settle();
    assert!(is_static(rig.pad.pad(0, 0), DIM_RED));
    rig.tap(0, 0);
    let track = tracks[0];
    testing::wait_for(&rig.control, &mut rig.shell, "the recording", |shell| {
        shell
            .session_states()
            .iter()
            .any(|s| s.track == track && s.state == bl_engine::SlotState::Recording)
    });
    rig.settle();
    assert_eq!(rig.pad.pad(0, 0), Some(Light::Pulse(RED)));
    // A pad past the last track does nothing.
    let before = rig.shell.surface_counts();
    rig.tap(7, 7);
    assert_eq!(rig.shell.surface_counts(), before);
}

// Spec: supported-controllers R8, R9. A scene button launches its scene, is
// dim white while the scene holds a clip and green while one plays, and a
// button past the last scene is dark.
#[test]
fn a_scene_button_launches_its_scene_and_lights_with_its_clips() {
    for which in [Which::X, Which::Mini, Which::Pro] {
        let mut rig = Rig::new(which);
        let tracks = rig.tracks(2);
        let scenes = rig.scenes();
        rig.quantisation_off();
        rig.clip(tracks[0], scenes[0], 3);
        rig.clip(tracks[1], scenes[0], 5);
        rig.run("transport.play", json!({}));
        rig.settle();
        assert!(is_static(rig.pad.scene_button(0), 1), "dim white with clips");
        assert_eq!(rig.pad.scene_button(1), None, "an empty scene is dark");
        rig.tap_button(89);
        let (a, b, scene) = (tracks[0], tracks[1], scenes[0]);
        testing::wait_for(&rig.control, &mut rig.shell, "the scene to play", |shell| {
            [a, b].iter().all(|t| {
                shell.session_states().iter().any(|s| {
                    s.track == *t && s.scene == scene && s.state == bl_engine::SlotState::Playing
                })
            })
        });
        rig.settle();
        assert!(is_static(rig.pad.scene_button(0), GREEN), "green while playing");
    }
}

// Spec: supported-controllers R12. The arrows move the ring one track or
// one scene, and are lit only while the ring can move their way.
#[test]
fn the_arrows_move_the_ring_and_darken_at_the_edges() {
    for which in [Which::X, Which::Mini, Which::Pro] {
        let mut rig = Rig::new(which);
        rig.tracks(10);
        rig.settle();
        let (up, down, left, right) = match which {
            Which::Pro => (80, 70, 91, 92),
            _ => (91, 92, 93, 94),
        };
        // At the first track and scene: only right and down are lit.
        assert_eq!(rig.pad.button(left), None);
        assert_eq!(rig.pad.button(up), None);
        assert!(rig.pad.button(right).is_some());
        let first = rig.row()["surface_state"]["ring"]["track_index"].clone();
        assert_eq!(first, 0);
        rig.tap_button(right);
        assert_eq!(rig.row()["surface_state"]["ring"]["track_index"], 1);
        assert!(rig.pad.button(left).is_some());
        rig.tap_button(right);
        rig.tap_button(right);
        // Ten tracks and eight columns: the ring stops at the third place,
        // and the right arrow goes dark.
        assert_eq!(rig.row()["surface_state"]["ring"]["track_index"], 2);
        assert_eq!(rig.pad.button(right), None);
        let _ = down;
    }
}

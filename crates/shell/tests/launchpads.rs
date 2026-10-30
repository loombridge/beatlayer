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
            shell.session_states().iter().any(|s| {
                s.track == track && s.scene == scene && s.state == bl_engine::SlotState::Playing
            })
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
        shell.session_states().iter().any(|s| {
            s.track == track && s.scene == scene && s.state == bl_engine::SlotState::Playing
        })
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
        assert!(
            is_static(rig.pad.scene_button(0), 1),
            "dim white with clips"
        );
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
        assert!(
            is_static(rig.pad.scene_button(0), GREEN),
            "green while playing"
        );
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

fn track_of(rig: &Rig, track: TrackId) -> bl_model::Mixer {
    *rig.shell.editor().document().track(track).unwrap().mixer()
}

// Spec: supported-controllers R13, R20. On the Launchpad X, Session switches
// between the clips and Mixer; in Mixer the scene buttons choose, top to
// bottom, Volume, Pan, Send A, Send B, Stop Clip, Mute, Solo and Record Arm
// (the chosen one bright, the others dim white), and the last four turn the
// grid's bottom row into that track function, which acts and lights as R13
// says; pressing the chosen one again goes back to the clips.
#[test]
fn the_x_mixer_turns_the_bottom_row_into_a_track_function() {
    let mut rig = Rig::new(Which::X);
    let tracks = rig.tracks(2);
    rig.invoke("track.create", json!({ "kind": "return", "name": "A" }));
    let scenes = rig.scenes();
    rig.quantisation_off();
    rig.clip(tracks[0], scenes[0], 3);
    rig.clip(tracks[0], scenes[7], 3);
    rig.run("transport.play", json!({}));
    rig.settle();
    assert_eq!(rig.mode(), "Session");
    assert!(
        rig.pad.pad(0, 7).is_some(),
        "the bottom row holds clips in Session"
    );

    // Spec: supported-controllers R24. The buttons the controller keeps for
    // itself are never lit.
    for cc in [95, 96, 97] {
        assert_eq!(rig.pad.button(cc), None, "{cc}");
    }
    rig.tap_button(95);
    assert_eq!(rig.mode(), "Mixer: Volume; faders: Volume");
    for cc in [95, 96, 97] {
        assert_eq!(rig.pad.button(cc), None, "{cc}");
    }
    assert_eq!(
        rig.pad.scene_button(0),
        Some(Light::Static(3)),
        "chosen: bright"
    );
    for row in 1..8 {
        assert!(is_static(rig.pad.scene_button(row), 1), "dim white: {row}");
    }

    // Stop Clip: the bottom row stops the tracks; red and bright while the
    // track plays a clip, dim otherwise, and dark for the return.
    rig.tap_button(49);
    assert_eq!(rig.mode(), "Mixer: Stop Clip; track buttons: Clip Stop");
    assert!(is_static(rig.pad.pad(0, 7), DIM_RED));
    assert!(is_static(rig.pad.pad(1, 7), DIM_RED));
    assert_eq!(rig.pad.pad(2, 7), None, "a return has no stop button");
    assert!(
        matches!(rig.pad.pad(0, 0), Some(Light::Static(c)) if c != DIM_RED),
        "the other rows keep their clips"
    );
    rig.tap(0, 0);
    let (track, scene) = (tracks[0], scenes[0]);
    testing::wait_for(&rig.control, &mut rig.shell, "the clip to play", |shell| {
        shell.session_states().iter().any(|s| {
            s.track == track && s.scene == scene && s.state == bl_engine::SlotState::Playing
        })
    });
    rig.settle();
    assert!(is_static(rig.pad.pad(0, 7), RED), "bright while it plays");
    rig.tap(0, 7);
    testing::wait_for(&rig.control, &mut rig.shell, "the track to stop", |shell| {
        shell.session_states().is_empty()
    });
    rig.settle();
    assert!(is_static(rig.pad.pad(0, 7), DIM_RED));

    // Mute: bright yellow while the track is heard.
    rig.tap_button(39);
    assert_eq!(rig.mode(), "Mixer: Mute; track buttons: Mute");
    assert!(is_static(rig.pad.pad(0, 7), 13));
    rig.tap(0, 7);
    assert!(track_of(&rig, tracks[0]).mute);
    assert!(is_static(rig.pad.pad(0, 7), 15), "dim while muted");
    rig.tap(0, 7);
    assert!(!track_of(&rig, tracks[0]).mute);

    // Solo: blue.
    rig.tap_button(29);
    assert!(is_static(rig.pad.pad(0, 7), 47));
    rig.tap(0, 7);
    assert!(track_of(&rig, tracks[0]).solo);
    assert!(is_static(rig.pad.pad(0, 7), 45));

    // Record Arm: red, and dark for a return.
    rig.tap_button(19);
    assert!(is_static(rig.pad.pad(0, 7), DIM_RED));
    assert_eq!(rig.pad.pad(2, 7), None);
    rig.tap(0, 7);
    assert!(track_of(&rig, tracks[0]).arm);
    assert!(is_static(rig.pad.pad(0, 7), RED));

    // The chosen one again goes back to the clips.
    rig.tap_button(19);
    assert_eq!(rig.mode(), "Session");
    assert!(
        matches!(rig.pad.pad(0, 7), Some(Light::Static(c)) if c != RED),
        "the clip shows again"
    );
    assert!(
        is_static(rig.pad.scene_button(0), 1),
        "the scene buttons are the scenes': dim white, the scene holds a clip"
    );
}

// Spec: supported-controllers R20. A Mixer button held while other controls
// are pressed goes back to the view before it when released.
#[test]
fn a_held_mixer_button_gives_the_view_before_it_back() {
    let mut rig = Rig::new(Which::X);
    rig.tracks(2);
    rig.tap_button(95);
    assert_eq!(rig.mode(), "Mixer: Volume; faders: Volume");
    rig.press(79);
    assert_eq!(rig.mode(), "Mixer: Pan; faders: Pan");
    rig.pad.fader_to(0, 90);
    rig.settle();
    rig.release(79);
    assert_eq!(rig.mode(), "Mixer: Volume; faders: Volume", "after a use");
    // A tap stays.
    rig.tap_button(79);
    assert_eq!(rig.mode(), "Mixer: Pan; faders: Pan");
}

// Spec: supported-controllers R14, R21. The X's grid faders are set up with
// the SysEx message and shown with the DAW Faders layout; they set volume,
// pan and a send, are green, the track's colour, purple and blue, are
// hidden past the last track, and follow an agent's change.
#[test]
fn the_x_faders_set_volume_pan_and_sends_and_follow_an_agents_change() {
    let mut rig = Rig::new(Which::X);
    let tracks = rig.tracks(3);
    rig.invoke("track.create", json!({ "kind": "return", "name": "A" }));
    rig.invoke("track.create", json!({ "kind": "return", "name": "B" }));
    rig.tap_button(95);
    assert_eq!(rig.pad.layout(), (0x0D, 0), "the DAW Faders layout");
    let bank = rig.pad.bank(0);
    assert!(!bank.horizontal);
    for i in 0..5 {
        // Three tracks and two returns: five faders, green.
        assert!(bank.faders[i].shown && bank.faders[i].colour == GREEN);
        assert!(!bank.faders[i].bipolar);
        assert_eq!(bank.faders[i].cc, 21 + i as u8);
    }
    assert!(!bank.faders[5].shown, "hidden past the last track");

    // A fader moves the volume, from its bottom to its top.
    rig.pad.fader_to(1, 127);
    rig.settle();
    let top = track_of(&rig, tracks[1]).volume;
    rig.pad.fader_to(1, 0);
    rig.settle();
    let bottom = track_of(&rig, tracks[1]).volume;
    assert!(top > bottom && bottom == 0.0, "{bottom} {top}");

    // An agent's change moves the fader.
    rig.run(
        "mixer.set_volume",
        json!({ "track": tracks[2], "volume": top }),
    );
    rig.settle();
    assert_eq!(rig.pad.bank(0).faders[2].position, 127);
    rig.run(
        "mixer.set_volume",
        json!({ "track": tracks[2], "volume": 0.0 }),
    );
    rig.settle();
    assert_eq!(rig.pad.bank(0).faders[2].position, 0);

    // Pan: horizontal and bipolar, in the track's colour.
    rig.tap_button(79);
    let bank = rig.pad.bank(0);
    assert!(bank.horizontal);
    assert!(bank.faders[0].bipolar && bank.faders[0].shown);
    rig.run("mixer.set_pan", json!({ "track": tracks[0], "pan": -1.0 }));
    rig.settle();
    assert_eq!(rig.pad.bank(0).faders[0].position, 0);
    rig.pad.fader_to(0, 127);
    rig.settle();
    assert!((track_of(&rig, tracks[0]).pan - 1.0).abs() < 1e-9);

    // Send A purple, Send B blue; a return has no send to a return.
    rig.tap_button(69);
    let bank = rig.pad.bank(0);
    assert_eq!(bank.faders[0].colour, 49);
    assert!(!bank.faders[3].shown, "a return has no send");
    rig.tap_button(59);
    assert_eq!(rig.pad.bank(0).faders[0].colour, 45);
    rig.pad.fader_to(0, 127);
    rig.settle();
    let doc = rig.shell.editor().document();
    let second = doc.returns().nth(1).unwrap();
    assert!(doc.track(tracks[0]).unwrap().send(second) > 0.0);
}

// Spec: supported-controllers R26 to R28. The Mini's Stop/Solo/Mute button
// steps the bottom row through clips, Stop, Solo and Mute, lit white, red,
// blue and yellow; the seven scene buttons above it launch the first seven
// scenes.
#[test]
fn the_minis_stop_solo_mute_button_cycles_the_bottom_row() {
    let mut rig = Rig::new(Which::Mini);
    let tracks = rig.tracks(2);
    let scenes = rig.scenes();
    rig.quantisation_off();
    rig.clip(tracks[0], scenes[7], 3);
    rig.settle();
    assert!(is_static(rig.pad.button(19), 3), "white for clips");
    assert!(rig.pad.pad(0, 7).is_some(), "the bottom row holds clips");
    assert_eq!(rig.mode(), "Session");
    for (colour, mode, pad) in [
        (5, "Session; track buttons: Clip Stop", DIM_RED),
        (45, "Session; track buttons: Solo", 47),
        (13, "Session; track buttons: Mute", 13),
    ] {
        rig.tap_button(19);
        assert!(is_static(rig.pad.button(19), colour), "{mode}");
        assert_eq!(rig.mode(), mode);
        assert!(
            is_static(rig.pad.pad(0, 7), pad),
            "{mode}: {:?}",
            rig.pad.pad(0, 7)
        );
    }
    // Mute acts: a pad of the bottom row mutes its track.
    rig.tap(1, 7);
    assert!(track_of(&rig, tracks[1]).mute);
    rig.tap_button(19);
    assert!(is_static(rig.pad.button(19), 3));
    assert!(rig.pad.pad(0, 7).is_some_and(|l| l != Light::Static(13)));
    // The scene buttons: seven launch scenes, the eighth is the cycle.
    assert!(rig.pad.scene_button(6).is_none());
    assert!(
        is_static(rig.pad.scene_button(7), 3),
        "the bottom button is Stop/Solo/Mute, not a scene's"
    );
}

// Spec: supported-controllers R13, R31. On the Pro the Track Select row
// selects tracks until a track control makes it Record Arm, Mute, Solo or
// Stop Clip; the chosen control pressed again gives the row back.
#[test]
fn the_pros_track_select_row_selects_until_a_track_control_changes_it() {
    let mut rig = Rig::new(Which::Pro);
    let tracks = rig.tracks(3);
    rig.invoke(
        "arrangement.select_tracks",
        json!({ "tracks": [tracks[1]] }),
    );
    rig.settle();
    assert_eq!(rig.mode(), "Session; track buttons: Select");
    let (selected, other) = (rig.pad.button(102), rig.pad.button(101));
    assert!(selected.is_some() && other.is_some() && selected != other);
    rig.tap_button(103);
    assert_eq!(
        rig.shell.arrangement().selected_tracks(),
        [tracks[2]],
        "a Track Select button selects its track"
    );
    // Arm.
    rig.tap_button(1);
    assert_eq!(rig.mode(), "Session; track buttons: Rec Arm");
    assert!(is_static(rig.pad.button(101), DIM_RED));
    assert!(rig.pad.button(104).is_none(), "past the last track: dark");
    rig.tap_button(101);
    assert!(track_of(&rig, tracks[0]).arm);
    assert!(is_static(rig.pad.button(101), RED));
    rig.tap_button(1);
    assert_eq!(rig.mode(), "Session; track buttons: Select");
    // Mute and Solo and Stop.
    rig.tap_button(2);
    rig.tap_button(102);
    assert!(track_of(&rig, tracks[1]).mute);
    rig.tap_button(3);
    rig.tap_button(103);
    assert!(track_of(&rig, tracks[2]).solo);
    rig.tap_button(8);
    assert_eq!(rig.mode(), "Session; track buttons: Clip Stop");
    assert!(is_static(rig.pad.button(101), DIM_RED));
}

// Spec: supported-controllers R14, R31. Volume, Pan and Sends show grid
// faders in the Fader layout's pages 0, 1 and 2, set up with fader banks 0,
// 1 and 2; pressing Sends again steps to the next send; Device shows page 3
// with the faders over the device in focus's bank, the Track Select row
// choosing among the selected track's devices.
#[test]
fn the_pros_faders_follow_its_pages_and_the_device_page_follows_the_focus() {
    let mut rig = Rig::new(Which::Pro);
    let tracks = rig.tracks(2);
    rig.invoke("track.create", json!({ "kind": "return", "name": "A" }));
    rig.invoke("track.create", json!({ "kind": "return", "name": "B" }));
    rig.settle();
    rig.tap_button(4);
    assert_eq!(rig.pad.layout(), (1, 0), "the Fader layout, page 0");
    assert_eq!(
        rig.mode(),
        "Fader: Volume; track buttons: Select; faders: Volume"
    );
    let bank = rig.pad.bank(0);
    assert!(bank.faders[0].shown && bank.faders[3].shown && !bank.faders[4].shown);
    assert_eq!(bank.faders[0].colour, GREEN);
    rig.pad.fader_to(0, 127);
    rig.settle();
    assert!(track_of(&rig, tracks[0]).volume > 0.0);
    rig.run(
        "mixer.set_volume",
        json!({ "track": tracks[0], "volume": 0.0 }),
    );
    rig.settle();
    assert_eq!(rig.pad.bank(0).faders[0].position, 0, "follows the set");

    rig.tap_button(5);
    assert_eq!(rig.pad.layout(), (1, 1));
    assert!(rig.pad.bank(1).horizontal && rig.pad.bank(1).faders[0].bipolar);

    rig.tap_button(6);
    assert_eq!(rig.pad.layout(), (1, 2));
    assert_eq!(rig.pad.bank(2).faders[0].colour, 49, "Send A is purple");
    rig.tap_button(6);
    assert_eq!(rig.pad.layout(), (1, 2), "still the Sends page");
    assert_eq!(rig.pad.bank(2).faders[0].colour, 45, "the next send, blue");
    rig.pad.fader_to(0, 127);
    rig.settle();
    let doc = rig.shell.editor().document();
    let second = doc.returns().nth(1).unwrap();
    assert!(doc.track(tracks[0]).unwrap().send(second) > 0.0);

    // Device: the faders reach the device in focus's bank.
    rig.select(tracks[0]);
    let first = rig.add("test.synth", "instrument");
    let second_device = rig.add("beatlayer.utility", "audio_effect");
    rig.select(tracks[0]);
    rig.tap_button(7);
    assert_eq!(rig.pad.layout(), (1, 3));
    assert!(rig.pad.bank(3).faders[0].shown, "a parameter of the bank");
    assert!(
        is_static(rig.pad.button(101), 1) || is_static(rig.pad.button(101), 3),
        "the devices are on the Track Select row"
    );
    rig.tap_button(102);
    let selected = rig.shell.device_view_state().selected;
    assert_eq!(selected, Some(second_device));
    let _ = first;
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

// Spec: supported-controllers R15, R16, R23, R34. On a track whose first
// instrument is a drum rack the controller shows the drum layout: four 4 x 4
// areas, the lower left the sixteen pads the pad view shows, the one above
// the next sixteen; filled pads yellow, the selected one blue; the arrows
// scroll the pad view by sixteen and four, and scrolling the pad view in the
// window moves the lights; on another track the controller keeps its own
// Note mode.
#[test]
fn a_drum_rack_on_the_selected_track_gives_the_drum_layout_both_ways() {
    for which in [Which::X, Which::Pro] {
        let mut rig = Rig::new(which);
        let tracks = rig.tracks(2);
        rig.select(tracks[0]);
        let rack = drum_rack(&mut rig, &[36, 37, 52]);
        rig.settle();
        assert_eq!(rig.pad.drum_mode(), 1, "Beatlayer's drum layout is on");
        let note_button = if which == Which::Pro { 94 } else { 96 };
        let (up, down, left, right) = match which {
            Which::Pro => (80, 70, 91, 92),
            _ => (91, 92, 93, 94),
        };
        rig.tap_button(note_button);
        assert!(rig.mode().starts_with("Drum"), "{}", rig.mode());
        // The lower left area: notes 36 and 37; 38 has no chain. The area
        // above it holds 52 at its bottom left.
        assert!(is_static(rig.pad.drum_pad(0, 7), 13), "36 is yellow");
        assert!(is_static(rig.pad.drum_pad(1, 7), 13), "37");
        assert_eq!(rig.pad.drum_pad(2, 7), None, "38 is empty");
        assert!(
            is_static(rig.pad.drum_pad(0, 3), 45),
            "52 is above, and selected: the pad dropped last"
        );
        assert_eq!(rig.pad.drum_pad(4, 7), None, "the right-hand area: 68");

        // Pressing a pad selects its drum rack pad.
        rig.pad.drum_down(1, 7);
        rig.pad.drum_up(1, 7);
        rig.settle();
        assert_eq!(pad_view(&rig, rack).1, Some(37));
        assert!(
            is_static(rig.pad.drum_pad(1, 7), 45),
            "the selected pad is blue"
        );
        assert!(is_static(rig.pad.drum_pad(0, 3), 13), "52 is yellow again");

        // The arrows scroll the pad view: up by sixteen, right by four.
        assert!(rig.pad.button(up).is_some() && rig.pad.button(left).is_some());
        rig.tap_button(up);
        assert_eq!(pad_view(&rig, rack).0, 52);
        assert!(
            is_static(rig.pad.drum_pad(0, 7), 13),
            "52 is now the first pad"
        );
        rig.tap_button(right);
        assert_eq!(pad_view(&rig, rack).0, 56);
        rig.tap_button(left);
        rig.tap_button(down);
        assert_eq!(pad_view(&rig, rack).0, 36);

        // Scrolling the pad view in the window moves the layout.
        rig.invoke("rack.move_pads", json!({ "rack": rack, "groups": 1 }));
        rig.settle();
        assert!(is_static(rig.pad.drum_pad(0, 7), 13), "52 is first");
        rig.invoke("rack.move_pads", json!({ "rack": rack, "first": 36 }));
        rig.settle();

        // The ring stays where it is while the drum layout shows.
        let at = rig.row()["surface_state"]["ring"]["track_index"].clone();
        assert_eq!(at, 0);

        // Another track gives the controller its own Note mode back.
        rig.select(tracks[1]);
        rig.settle();
        assert_eq!(rig.pad.drum_mode(), 0);
        assert!(rig.mode().starts_with("Note"), "{}", rig.mode());
    }
}

fn tempo(bpm: f64) -> Value {
    json!({ "tempo": [{ "kind": "constant", "start": 0, "bpm": bpm }] })
}

fn undo_label(rig: &Rig) -> String {
    rig.shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

// Spec: supported-controllers R32. On the Pro, Shift with a pad selects its
// slot, Clear deletes its clip, Duplicate copies it to the slot below,
// Shift with Duplicate doubles its loop and Quantise quantises its notes;
// Play, Record and Shift with Record, Undo, Redo, the metronome and Tap
// Tempo run their commands; while Shift is held the buttons it changes
// light gold.
#[test]
fn the_pros_functions_run_their_commands_and_shift_lights_gold() {
    let mut rig = Rig::new(Which::Pro);
    let tracks = rig.tracks(2);
    let scenes = rig.scenes();
    rig.quantisation_off();
    rig.clip(tracks[0], scenes[0], 3);
    rig.clip(tracks[1], scenes[0], 5);
    rig.settle();

    // Shift: gold on what it changes, and a pad selects its slot.
    rig.press(90);
    for cc in [1, 2, 3, 6, 10] {
        assert!(is_static(rig.pad.button(cc), 61), "{cc} is gold");
    }
    rig.tap(1, 0);
    let state = rig.run("session.state", json!({}));
    assert_eq!(state["slots"][0]["track"], json!(tracks[1]), "{state}");
    assert!(rig.shell.session_states().is_empty(), "nothing launched");
    rig.release(90);
    assert!(is_static(rig.pad.button(1), 1), "gold goes");

    // Duplicate: the clip is copied to the slot below.
    rig.press(50);
    rig.tap(0, 0);
    rig.release(50);
    assert!(
        rig.shell
            .editor()
            .document()
            .slot(tracks[0], scenes[1])
            .is_some(),
        "the slot below holds a copy"
    );

    // Clear: the clip goes.
    rig.press(60);
    rig.tap(0, 1);
    rig.release(60);
    assert!(
        rig.shell
            .editor()
            .document()
            .slot(tracks[0], scenes[1])
            .is_none()
    );

    // Quantise: the notes land on sixteenths.
    rig.run(
        "session.select",
        json!({ "track": tracks[0], "scene": scenes[0] }),
    );
    rig.run(
        "note.add",
        json!({ "key": 60, "start": 50_000, "length": 100_000 }),
    );
    rig.press(40);
    rig.tap(0, 0);
    rig.release(40);
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
    assert_eq!(midi.notes.len(), 1);
    assert_eq!(midi.notes[0].start, bl_core::time::Ticks(0), "quantised");

    // Shift with Duplicate doubles the clip's loop.
    let length = clip.length;
    rig.press(90);
    rig.press(50);
    rig.tap(0, 0);
    rig.release(50);
    rig.release(90);
    let doubled = rig
        .shell
        .editor()
        .document()
        .slot(tracks[0], scenes[0])
        .unwrap()
        .length;
    assert!(doubled > length, "{doubled:?} {length:?}");

    // Shift with the track controls: Undo, Redo, the metronome, Tap Tempo.
    rig.run(
        "mixer.set_volume",
        json!({ "track": tracks[0], "volume": 0.5 }),
    );
    rig.press(90);
    rig.tap_button(1);
    assert!(
        (track_of(&rig, tracks[0]).volume - 0.5).abs() > 1e-9,
        "undone"
    );
    rig.tap_button(2);
    assert!(
        (track_of(&rig, tracks[0]).volume - 0.5).abs() < 1e-9,
        "redone"
    );
    assert!(!rig.shell.editor().document().settings().metronome.enabled);
    rig.tap_button(3);
    assert!(rig.shell.editor().document().settings().metronome.enabled);
    rig.tap_button(3);
    assert!(!rig.shell.editor().document().settings().metronome.enabled);
    let before = rig.shell.surface_counts();
    rig.tap_button(6);
    let after = rig.shell.surface_counts();
    assert_eq!(after.0 + after.1, before.0 + before.1 + 1, "tap tempo ran");
    rig.release(90);

    // Play starts the transport and stops it; Record starts Session Record.
    rig.run("mixer.set_arm", json!({ "track": tracks[1], "arm": true }));
    rig.tap_button(20);
    testing::wait_for(&rig.control, &mut rig.shell, "play", |s| {
        s.arrangement().transport().playing
    });
    rig.settle();
    assert!(is_static(rig.pad.button(20), GREEN));
    rig.tap_button(20);
    testing::wait_for(&rig.control, &mut rig.shell, "stop", |s| {
        !s.arrangement().transport().playing
    });
    rig.tap_button(10);
    testing::wait_for(&rig.control, &mut rig.shell, "the recording", |s| {
        s.session_states()
            .iter()
            .any(|slot| slot.state == bl_engine::SlotState::Recording)
    });
    rig.settle();
    assert!(is_static(rig.pad.button(10), RED));
    // Shift with Record runs Capture MIDI.
    let before = rig.shell.surface_counts();
    rig.press(90);
    rig.tap_button(10);
    rig.release(90);
    let after = rig.shell.surface_counts();
    assert_eq!(
        after.0 + after.1,
        before.0 + before.1 + 1,
        "Capture MIDI ran"
    );
    let _ = undo_label(&rig);
}

// Spec: supported-controllers R33. Holding Session shows Session Overview:
// each pad stands for a block of eight tracks and eight scenes, the ring's
// block light brown, blocks with a playing clip pulsing green, the others
// blue, a block past the set dark; a pad moves the ring to its block, and
// releasing Session goes back.
#[test]
fn holding_session_on_the_pro_shows_session_overview() {
    let mut rig = Rig::new(Which::Pro);
    let tracks = rig.tracks(10);
    let scenes = rig.scenes();
    rig.quantisation_off();
    rig.clip(tracks[9], scenes[0], 3);
    rig.run("transport.play", json!({}));
    rig.settle();
    rig.press(93);
    assert_eq!(
        rig.pad.pad(0, 0),
        Some(Light::Static(105)),
        "the ring's block"
    );
    assert_eq!(rig.pad.pad(1, 0), Some(Light::Static(45)), "blue");
    assert_eq!(rig.pad.pad(2, 0), None, "a block past the set");
    assert_eq!(rig.pad.pad(0, 1), None, "scenes: one block");
    rig.pad.pad_down(1, 0);
    rig.pad.pad_up(1, 0);
    rig.settle();
    assert_eq!(
        rig.row()["surface_state"]["ring"]["track_index"],
        2,
        "the ring stops where its last column holds the last track"
    );
    rig.release(93);
    assert!(
        matches!(rig.pad.pad(7, 0), Some(Light::Static(_))),
        "the clips come back: the clip on the tenth track is in column 7"
    );
    // A playing clip makes its block pulse green.
    let (track, scene) = (tracks[9], scenes[0]);
    rig.tap(7, 0);
    testing::wait_for(&rig.control, &mut rig.shell, "the clip to play", |s| {
        s.session_states().iter().any(|x| {
            x.track == track && x.scene == scene && x.state == bl_engine::SlotState::Playing
        })
    });
    rig.press(93);
    assert_eq!(rig.pad.pad(1, 0), Some(Light::Pulse(GREEN)));
    rig.release(93);
}

// Spec: supported-controllers R22. On the X, Capture MIDI starts and stops
// Session Record when pressed and released within 500 ms, and runs Capture
// MIDI when held longer; it is lit red while Session Record is on.
#[test]
fn the_x_capture_button_records_on_a_tap_and_captures_on_a_hold() {
    let mut rig = Rig::new(Which::X);
    let tracks = rig.tracks(1);
    rig.run("mixer.set_arm", json!({ "track": tracks[0], "arm": true }));
    rig.run("transport.play", json!({}));
    rig.settle();
    assert!(is_static(rig.pad.button(98), 1), "dim white");
    let now = std::time::Instant::now();
    // A tap of 100 ms: Session Record.
    rig.pad.button_down_at(98, now);
    rig.pad
        .button_up_at(98, now + std::time::Duration::from_millis(100));
    rig.settle();
    testing::wait_for(&rig.control, &mut rig.shell, "the recording", |s| {
        s.session_states()
            .iter()
            .any(|slot| slot.state == bl_engine::SlotState::Recording)
    });
    rig.settle();
    assert!(
        is_static(rig.pad.button(98), RED),
        "lit red while it records"
    );
    rig.pad
        .button_down_at(98, now + std::time::Duration::from_secs(5));
    rig.pad
        .button_up_at(98, now + std::time::Duration::from_millis(5_100));
    rig.settle();
    testing::wait_for(&rig.control, &mut rig.shell, "the recording to end", |s| {
        s.session_states()
            .iter()
            .all(|slot| slot.state != bl_engine::SlotState::Recording)
    });
    // A hold of 700 ms: Capture MIDI, which starts no recording.
    let before = rig.shell.surface_counts();
    rig.pad
        .button_down_at(98, now + std::time::Duration::from_secs(10));
    rig.pad
        .button_up_at(98, now + std::time::Duration::from_millis(10_700));
    rig.settle();
    let after = rig.shell.surface_counts();
    assert_eq!(after.0 + after.1, before.0 + before.1 + 1);
    assert!(
        rig.shell
            .session_states()
            .iter()
            .all(|slot| slot.state != bl_engine::SlotState::Recording)
    );
}

// Spec: supported-controllers R11. A grid surface sends its controller 24
// clocks per quarter note at the set's tempo, each stamped with its moment,
// and the clock stops with the surface.
#[test]
fn the_surface_sends_clock_at_the_sets_tempo_and_stops_with_it() {
    let mut rig = Rig::new(Which::X);
    rig.run("song.set_tempo", tempo(120.0));
    for _ in 0..30 {
        rig.pass(20);
    }
    let pulses = rig.pad.clock();
    assert!(pulses.len() > 10, "{}", pulses.len());
    let gaps: Vec<f64> = pulses
        .windows(2)
        .map(|w| w[1].duration_since(w[0]).as_secs_f64())
        .collect();
    let want = 60.0 / (24.0 * 120.0);
    assert!(
        gaps.iter().all(|g| (g - want).abs() < 1e-4),
        "a pulse every {want}: {gaps:?}"
    );
    rig.run("song.set_tempo", tempo(90.0));
    for _ in 0..30 {
        rig.pass(20);
    }
    let last = *rig.pad.clock().last().unwrap();
    let after: Vec<f64> = rig
        .pad
        .clock()
        .windows(2)
        .rev()
        .take(5)
        .map(|w| w[1].duration_since(w[0]).as_secs_f64())
        .collect();
    let want = 60.0 / (24.0 * 90.0);
    assert!(after.iter().all(|g| (g - want).abs() < 1e-4), "{after:?}");
    // The clock goes to the surface port only, and starts no transport.
    assert!(!rig.shell.arrangement().transport().playing);
    // It stops with the surface.
    rig.run("control_surface.set", json!({ "row": 1, "surface": null }));
    rig.pad.pump();
    let count = rig.pad.clock().len();
    for _ in 0..10 {
        rig.pass(20);
    }
    assert_eq!(rig.pad.clock().len(), count);
    let _ = last;
}

// Spec: supported-controllers R17, R3. Removing the row puts out every light
// the surface set and sends the message that gives the controller back its
// standalone state, and the MIDI port stays an ordinary MIDI port.
#[test]
fn removing_the_row_puts_out_every_light_and_sends_standalone() {
    for which in [Which::X, Which::Mini, Which::Pro] {
        let mut rig = Rig::new(which);
        let tracks = rig.tracks(3);
        let scenes = rig.scenes();
        rig.clip(tracks[0], scenes[0], 3);
        rig.settle();
        assert!(rig.pad.lit() >= 3, "{}", rig.pad.lit());
        rig.run("control_surface.set", json!({ "row": 1, "surface": null }));
        rig.pad.pump();
        assert_eq!(
            rig.pad.lit_when_left(),
            Some(0),
            "every light was out before Standalone"
        );
        assert!(!rig.pad.daw_mode());
        assert_eq!(rig.row()["state"], "empty");
        // Not refilled while Beatlayer runs: a person emptied the row.
        rig.shell.refresh_midi();
        for _ in 0..3 {
            rig.pad.pump();
            rig.shell.poll();
        }
        assert_eq!(rig.row()["state"], "empty");
    }
}

// Spec: supported-controllers R26, R28. The Mini's Drums, Keys and User are
// the controller's own layouts: the ring stays where it was while one shows
// and Session brings the Session back; the Mini has no faders and no Mixer.
#[test]
fn the_minis_own_layouts_leave_the_ring_alone_and_it_has_no_mixer() {
    let mut rig = Rig::new(Which::Mini);
    rig.tracks(10);
    rig.tap_button(94);
    assert_eq!(rig.row()["surface_state"]["ring"]["track_index"], 1);
    // Session doesn't open a Mixer on the Mini.
    rig.tap_button(95);
    assert_eq!(rig.mode(), "Session");
    assert_eq!(rig.pad.layout(), (0, 0));
    for cc in [96, 97, 98] {
        rig.tap_button(cc);
        assert_eq!(rig.mode(), "Custom", "{cc}");
        assert_eq!(rig.row()["surface_state"]["ring"]["track_index"], 1);
        assert_eq!(rig.pad.button(cc), None, "left to the controller");
    }
    rig.tap_button(95);
    assert_eq!(rig.mode(), "Session");
    assert!(!rig.pad.bank(0).faders[0].shown);
}

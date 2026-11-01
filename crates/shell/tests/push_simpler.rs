//! Simpler on Push 2 in the shell (`docs/specs/push-2-notes.md`, R67 to R76):
//! its main bank and Edit Mode, slicing from the pads and Convert, driven
//! through the simulated Push 2. Every edit runs the app's own commands, one
//! undo step each.

use std::path::PathBuf;
use std::time::Duration;

use bl_audio_io::NullControl;
use bl_core::clock::TestClock;
use bl_devices::sampler as s;
use bl_midi_io::TestBackend;
use bl_model::{Device, DeviceId, ParamId, TrackId, TrackKind};
use bl_push::sim::Simulated;
use bl_shell::{Shell, testing};
use serde_json::{Value, json};

/// The frames of the sample: two seconds at 48 kHz.
const FRAMES: u64 = 96_000;

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-push-simpler-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Rig {
    shell: Shell,
    #[expect(dead_code, reason = "keeps the null device and its clock alive")]
    control: NullControl,
    clock: TestClock,
    sim: Simulated,
    track: TrackId,
    device: DeviceId,
    _scratch: Scratch,
}

impl Rig {
    /// A Push 2 running on a MIDI track whose Simpler plays four decaying
    /// hits.
    fn new() -> Self {
        let scratch = Scratch::new();
        let (mut shell, control) = testing::rig();
        let test = TestBackend::new();
        shell.use_midi_backend(Box::new(test.clone()));
        let clock = control.clock().expect("manual pacing");
        shell.use_test_clock(clock.clone());
        let sim = Simulated::new(&test, &clock);
        shell.use_push_display(sim.frames().claim());
        shell.start_audio();
        let samples: Vec<f32> = (0..FRAMES as usize)
            .map(|n| {
                let into = (n % 24_000) as f32 / 24_000.0;
                (n as f32 * 0.05).sin() * (-into * 5.0).exp() * 0.5
            })
            .collect();
        let bytes = bl_media::wav_bytes(
            &samples,
            1,
            48_000,
            bl_media::BitDepth::Int16,
            bl_media::Dither::None,
        )
        .unwrap();
        let file = scratch.0.join("hits.wav");
        std::fs::write(&file, bytes).unwrap();
        let track = shell
            .editor()
            .document()
            .tracks()
            .find(|t| t.kind() == TrackKind::Midi)
            .map(bl_model::Track::id)
            .unwrap();
        shell
            .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
            .unwrap();
        let added = shell
            .invoke(
                "device.add",
                json!({ "device": {
                    "kind": { "native": { "device_type": s::SAMPLER } },
                    "name": "Simpler",
                    "role": "instrument",
                } }),
            )
            .unwrap();
        let device: DeviceId = serde_json::from_value(added["device"].clone()).unwrap();
        shell
            .run(
                "device.load_sample",
                json!({ "device": device, "path": file.to_string_lossy() }),
            )
            .unwrap();
        let mut rig = Self {
            shell,
            control,
            clock,
            sim,
            track,
            device,
            _scratch: scratch,
        };
        rig.sim.plug_in();
        rig.shell.refresh_midi();
        rig.settle();
        rig.pass(50);
        rig
    }

    fn run(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .run(id, args)
            .unwrap_or_else(|error| panic!("{id}: {error}"))
    }

    fn settle(&mut self) {
        loop {
            self.shell.midi().surfaces().settle();
            if self.sim.pump() == 0 {
                break;
            }
        }
    }

    fn step(&mut self) {
        self.shell.poll();
        self.shell.midi().surfaces().tick();
        self.settle();
        self.shell.poll();
    }

    fn pass(&mut self, millis: u64) {
        self.clock.advance(Duration::from_millis(millis));
        self.step();
    }

    fn state(&mut self) -> Value {
        self.run("push.state", json!({}))["pushes"][0].clone()
    }

    /// What Device Mode shows of the Simpler, as `push.state` reads it.
    fn simpler(&mut self) -> Value {
        self.state()["device"]["simpler"].clone()
    }

    fn steps(&self) -> usize {
        self.shell.editor().undo_history().count()
    }

    fn tap(&mut self, name: &str) {
        assert!(self.sim.press(name));
        self.pass(50);
        assert!(self.sim.let_go(name));
        self.pass(20);
    }

    fn tap_pad(&mut self, row: u8, column: u8) {
        assert!(self.sim.press_pad(row, column, 100));
        self.pass(20);
        assert!(self.sim.release_pad(row, column));
        self.pass(40);
    }

    /// One touch of encoder `column` with the turns in it.
    fn turn(&mut self, column: u8, turns: &[i32]) {
        assert!(self.sim.touch(column));
        for steps in turns {
            assert!(self.sim.turn(column, *steps));
            self.pass(1);
        }
        assert!(self.sim.release(column));
        self.pass(20);
    }

    fn device(&self) -> Device {
        self.shell
            .editor()
            .document()
            .device(self.device)
            .unwrap()
            .clone()
    }

    fn value(&self, param: ParamId) -> f64 {
        let description = s::sampler_type().description;
        description
            .value(&self.device().params, param)
            .unwrap_or_default()
    }

    fn set(&mut self, param: ParamId, value: f64) {
        self.run(
            "device.set_param",
            json!({ "device": self.device, "param": param, "value": value }),
        );
        self.pass(20);
    }

    fn text(&mut self) -> Vec<String> {
        self.state()["display"]["text"]
            .as_array()
            .unwrap()
            .iter()
            .map(|line| line.as_str().unwrap().to_owned())
            .collect()
    }

    /// The tracks of the set, in order.
    fn tracks(&self) -> Vec<TrackId> {
        self.shell
            .editor()
            .document()
            .top_level()
            .collect::<Vec<_>>()
    }
}

// Spec: push-2-notes R67, R68, R70, R72. The Simpler's main bank is on the
// encoders over its waveform; S Start and S Length place the played part, and
// a touch is one undo step; Start moves the flag; Edit Mode's buttons and the
// Global bank run their commands; `push.state` reads it all back.
#[test]
fn the_main_bank_and_edit_mode_change_the_set_and_read_back() {
    let mut rig = Rig::new();
    let simpler = rig.simpler();
    assert_eq!(simpler["mode"], "classic", "{simpler}");
    assert_eq!(simpler["main_bank"], true);
    assert_eq!(simpler["empty"], false);
    assert_eq!(simpler["region"], json!([0, FRAMES]));
    let text = rig.text();
    assert!(
        text.contains(&"column 3: Start = 0.000 s".to_owned()),
        "{text:?}"
    );
    assert!(text.iter().any(|l| l.starts_with("waveform:")));

    // S Start 50 % and S Length 25 %: the region's third quarter.
    let steps = rig.steps();
    rig.turn(4, &[50, 50]);
    assert_eq!(rig.steps(), steps + 1, "a touch is one undo step");
    assert!((rig.value(s::START) - 50.0).abs() < 1e-9);
    rig.turn(5, &[-50, -50, -50]);
    assert!((rig.value(s::LENGTH) - 25.0).abs() < 1e-9);
    assert_eq!(rig.steps(), steps + 2);

    // Start moves the flag a two-hundredth of the sample a step.
    rig.turn(2, &[10]);
    assert_eq!(
        rig.device().region.unwrap().frames_in(FRAMES),
        (4800, FRAMES)
    );
    assert_eq!(rig.simpler()["region"], json!([4800, FRAMES]));

    // Edit Mode: the second tap on the device's button.
    rig.tap("upper_1");
    let simpler = rig.simpler();
    let names: Vec<&str> = simpler["edit_buttons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["loop", "warp_as", "half", "double", "crop", "reverse"]
    );
    let steps = rig.steps();
    rig.tap("upper_2");
    assert!(rig.value(s::LOOP) >= 0.5, "Loop is on");
    // Warp as: Warp on and the sample's tempo in one undo step.
    rig.tap("upper_3");
    let warp = rig.device().warp;
    assert!(warp.warp);
    let bpm = warp.bpm.expect("the sample's tempo is set");
    assert_eq!(rig.steps(), steps + 2, "Loop and Warp as, a step each");
    rig.tap("upper_4");
    assert!((rig.device().warp.bpm.unwrap() - bpm / 2.0).abs() < 1e-9);
    rig.tap("upper_5");
    assert!((rig.device().warp.bpm.unwrap() - bpm).abs() < 1e-9);

    // The Global bank, on the second lower button.
    rig.tap("lower_2");
    assert_eq!(rig.state()["device"]["bank"], 1);
    rig.turn(0, &[10]);
    assert_eq!(rig.value(s::GLIDE_MODE), 1.0, "Portamento");
}

// Spec: push-2-notes R73, R75. In Slice mode the pads are the slices and
// Nudge, Split Slice and Delete with a pad change them, one undo step each.
#[test]
fn slices_fill_the_pads_and_nudge_split_and_delete_edit_them() {
    let mut rig = Rig::new();
    rig.set(s::MODE, 2.0);
    rig.set(s::SLICE_BY, 2.0);
    rig.set(s::SLICE_REGIONS, 40.0);
    let simpler = rig.simpler();
    assert_eq!(simpler["slices"], 40, "{simpler}");
    // Forty pads are lit, in the order of R75.
    let state = rig.state();
    let lit: Vec<(u8, u8)> = (0..8u8)
        .flat_map(|row| (0..8u8).map(move |column| (row, column)))
        .filter(|(row, column)| state["leds"][format!("pad_{row}_{column}")]["colour"] != 0)
        .collect();
    let expected: Vec<(u8, u8)> = (0..8u8)
        .flat_map(|row| (0..8u8).map(move |column| (row, column)))
        .filter(|(row, column)| {
            u32::from(column / 4) * 32 + u32::from(*row) * 4 + u32::from(column % 4) < 40
        })
        .collect();
    assert_eq!(lit, expected);

    // Manual slices: Split Slice halves the selected one.
    rig.set(s::SLICE_BY, 3.0);
    assert_eq!(rig.simpler()["slices"], 1);
    let steps = rig.steps();
    rig.tap("upper_1");
    rig.tap("upper_4");
    assert_eq!(rig.device().slices, [48_000], "halfway through the sample");
    assert_eq!(rig.steps(), steps + 1);
    assert_eq!(rig.simpler()["slices"], 2);

    // Tapping the second pad selects the new slice; Nudge moves it, a tenth
    // as far with Shift.
    rig.tap_pad(0, 1);
    assert_eq!(rig.simpler()["selected_slice"], 1);
    rig.turn(7, &[10]);
    assert_eq!(rig.device().slices, [48_000 + 4_800]);
    assert!(rig.sim.press("shift"));
    rig.pass(20);
    rig.turn(7, &[10]);
    assert!(rig.sim.let_go("shift"));
    rig.pass(20);
    assert_eq!(rig.device().slices, [48_000 + 4_800 + 480]);

    // Delete and the pad deletes the slice.
    assert!(rig.sim.press("delete"));
    rig.pass(20);
    rig.tap_pad(0, 1);
    assert!(rig.sim.let_go("delete"));
    rig.pass(20);
    assert!(rig.device().slices.is_empty());
}

// Spec: push-2-notes R76. Convert on a Classic Simpler makes a new MIDI track
// after the selected one with a drum rack holding a copy of it on the pad for
// note 36, and copies of the track's other devices; one undo step.
#[test]
fn convert_makes_a_new_track_with_a_drum_rack_from_a_classic_simpler() {
    let mut rig = Rig::new();
    // An effect after the Simpler is copied in its place.
    rig.run(
        "device.add",
        json!({ "device": {
            "kind": { "native": { "device_type": "beatlayer.utility" } },
            "name": "Utility",
            "role": "audio_effect",
        } }),
    );
    rig.run("device.select", json!({ "device": rig.device }));
    rig.pass(40);
    let before = rig.tracks();
    let steps = rig.steps();
    rig.tap("convert");
    assert_eq!(rig.steps(), steps + 1, "one undo step");
    let after = rig.tracks();
    assert_eq!(after.len(), before.len() + 1);
    let at = after.iter().position(|t| *t == rig.track).unwrap();
    let new = after[at + 1];
    assert!(
        !before.contains(&new),
        "the new track follows the selected one"
    );
    let doc = rig.shell.editor().document();
    let devices: Vec<_> = doc.track(new).unwrap().devices().collect();
    assert_eq!(devices.len(), 2, "the rack and the copy of the Utility");
    let rack = devices[0];
    assert_eq!(rack.rack_kind(), Some(bl_model::RackKind::Drum));
    assert_eq!(rack.chains.len(), 1);
    assert_eq!(rack.chains[0].receive, bl_model::Receive::Note(36));
    let copy = &rack.chains[0].devices[0];
    assert_ne!(copy.id, rig.device, "a copy has an ID of its own");
    assert_eq!(
        copy.media_at(0),
        doc.device(rig.device).unwrap().media_at(0)
    );
    assert_eq!(devices[1].name, "Utility");
    // The original track is as it was.
    assert_eq!(doc.track(rig.track).unwrap().devices().count(), 2);
}

// Spec: push-2-notes R76. With the Simpler in Slice mode, Convert replaces it
// with a drum rack whose pads hold its slices; one undo step.
#[test]
fn convert_replaces_a_slicing_simpler_with_a_drum_rack() {
    let mut rig = Rig::new();
    rig.set(s::MODE, 2.0);
    rig.set(s::SLICE_BY, 2.0);
    rig.set(s::SLICE_REGIONS, 4.0);
    rig.run("device.select", json!({ "device": rig.device }));
    rig.pass(40);
    let steps = rig.steps();
    rig.tap("convert");
    assert_eq!(rig.steps(), steps + 1);
    let doc = rig.shell.editor().document();
    let devices: Vec<_> = doc.track(rig.track).unwrap().devices().collect();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].rack_kind(), Some(bl_model::RackKind::Drum));
    assert_eq!(devices[0].chains.len(), 4, "a pad for each slice");
}

// Spec: push-2-notes R76. With a drum rack pad selected, Convert makes a new
// track holding copies of the pad's devices; one undo step.
#[test]
fn convert_makes_a_new_track_of_the_selected_pads_devices() {
    let mut rig = Rig::new();
    rig.run("device.select", json!({ "device": rig.device }));
    let rack_track = rig.run("push.convert", json!({}));
    let rack = rack_track["device"].clone();
    // A drum rack with the Simpler on a pad: the track the first Convert
    // made, whose rack the second acts on.
    let rack: DeviceId = serde_json::from_value(rack).unwrap();
    rig.run(
        "arrangement.select_tracks",
        json!({ "tracks": [rack_track["track"]] }),
    );
    rig.run("device.select", json!({ "device": rack }));
    rig.run("pad.select", json!({ "rack": rack, "note": 36 }));
    rig.pass(40);
    let before = rig.tracks();
    let steps = rig.steps();
    let made = rig.run("push.convert", json!({}));
    assert_eq!(made["from"], "pad");
    assert_eq!(rig.steps(), steps + 1);
    assert_eq!(rig.tracks().len(), before.len() + 1);
    let new: TrackId = serde_json::from_value(made["track"].clone()).unwrap();
    let doc = rig.shell.editor().document();
    let devices: Vec<_> = doc.track(new).unwrap().devices().collect();
    assert_eq!(devices.len(), 1);
    assert_eq!(
        devices[0].id,
        serde_json::from_value::<DeviceId>(made["device"].clone()).unwrap()
    );
    assert_eq!(
        devices[0].media_at(0),
        doc.device(rig.device).unwrap().media_at(0)
    );
}

// Spec: push-2-notes R76. An audio clip offers a new track with a Simpler
// or with a drum rack holding it on its first pad, and lists Convert
// Harmony, Melody and Drums as not built; each is one undo step.
#[test]
fn convert_makes_a_simpler_or_a_drum_rack_of_an_audio_clip() {
    let mut rig = Rig::new();
    let file = rig._scratch.0.join("hits.wav");
    let audio = rig
        .shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .unwrap();
    rig.run("arrangement.select_tracks", json!({ "tracks": [audio] }));
    rig.run(
        "browser.add_to_arrangement",
        json!({ "paths": [file.to_string_lossy()], "track": audio, "at": 0 }),
    );
    let clip = rig
        .shell
        .editor()
        .document()
        .track(audio)
        .unwrap()
        .clips()
        .next()
        .unwrap()
        .id;
    rig.run("arrangement.select_clips", json!({ "clips": [clip] }));
    rig.pass(40);

    // The choices on the display.
    rig.tap("convert");
    let text = rig.text();
    for line in [
        "upper 1: Simpler",
        "upper 2: Drum Rack",
        "upper 3: Convert Harmony",
        "upper 4: Convert Melody",
        "upper 5: Convert Drums",
    ] {
        assert!(text.contains(&line.to_owned()), "{line}: {text:?}");
    }
    assert_eq!(rig.state()["menu"], "convert");
    let steps = rig.steps();
    rig.tap("upper_3");
    assert_eq!(rig.steps(), steps, "a choice not built does nothing");

    let before = rig.tracks();
    rig.tap("convert");
    rig.tap("upper_1");
    assert_eq!(rig.steps(), steps + 1);
    let made = rig.tracks();
    assert_eq!(made.len(), before.len() + 1);
    let new = made[made.iter().position(|t| *t == audio).unwrap() + 1];
    {
        let doc = rig.shell.editor().document();
        let devices: Vec<_> = doc.track(new).unwrap().devices().collect();
        assert_eq!(devices.len(), 1);
        assert!(devices[0].media_at(0).is_some(), "the clip's sample");
    }

    rig.run("arrangement.select_tracks", json!({ "tracks": [audio] }));
    rig.run("arrangement.select_clips", json!({ "clips": [clip] }));
    rig.pass(40);
    rig.tap("convert");
    rig.tap("upper_2");
    let doc = rig.shell.editor().document();
    let drums: Vec<_> = doc
        .top_level()
        .filter_map(|t| doc.track(t))
        .flat_map(|t| t.devices())
        .filter(|d| d.rack_kind() == Some(bl_model::RackKind::Drum))
        .collect();
    assert_eq!(
        drums.len(),
        1,
        "a drum rack holds the sample on its first pad"
    );
    assert_eq!(drums[0].chains[0].receive, bl_model::Receive::Note(36));
}

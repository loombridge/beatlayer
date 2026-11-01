//! Simpler on Push 2 (`docs/specs/push-2-notes.md`, R67 to R75): the empty
//! Simpler, the main bank over the waveform in each mode, Zoom, Edit Mode's
//! buttons, slicing, Pad Slicing and the pads' slices, on the simulated Push 2
//! with a read model the test builds by hand.

use std::sync::Arc;

use bl_control::{Focus, ParamView, SampleView};
use bl_core::midi::MidiInputEvent;
use bl_devices::sampler as s;
use bl_model::{Command, Device, DeviceId, DeviceKind, DeviceRole, ParamId, SampleRegion};
use serde_json::{Value, json};

use bl_push::simpler::FIRST_KEY;

use crate::display::golden;
use crate::rig::Rig;

const RATE: u32 = 48_000;
/// Two seconds: four hits, half a second apart.
const FRAMES: u64 = 96_000;

/// A sample of four decaying hits, whose peaks the display draws.
fn peaks() -> Arc<bl_media::Peaks> {
    let samples: Vec<f32> = (0..FRAMES as usize)
        .map(|n| {
            let into = (n % 24_000) as f32 / 24_000.0;
            let hit = (n / 24_000) as f32;
            (n as f32 * (0.05 + hit * 0.02)).sin() * (-into * 5.0).exp() * (0.5 + hit * 0.15)
        })
        .collect();
    let data = bl_media::SampleData::new(RATE, vec![samples]).unwrap();
    Arc::new(bl_media::Peaks::from_samples(&data).unwrap())
}

struct Simp {
    rig: Rig,
    device: DeviceId,
    bank: usize,
    loaded: bool,
    /// Where the automatic slices start, as the shell would work them out.
    auto: Vec<u64>,
    peaks: Arc<bl_media::Peaks>,
}

fn run(id: &str, args: Value) -> (String, Value) {
    (id.to_owned(), args)
}

impl Simp {
    fn new(mode: f64) -> Self {
        let mut rig = Rig::simplers();
        rig.start();
        let track = rig.tracks[0];
        rig.model.selection.tracks = vec![track];
        let device = rig.editor.new_id();
        rig.apply(Command::InsertDevice {
            track,
            index: 0,
            device: Device::new(
                device,
                "Simpler",
                DeviceKind::Native {
                    device_type: s::SAMPLER.to_owned(),
                },
            )
            .with_role(DeviceRole::Instrument),
        });
        rig.apply(Command::SetDeviceParam {
            device,
            param: s::MODE,
            value: mode,
        });
        let mut simp = Self {
            rig,
            device,
            bank: 0,
            loaded: true,
            auto: vec![24_000, 48_000, 72_000],
            peaks: peaks(),
        };
        simp.refresh();
        simp.rig.advance(20);
        simp.rig.advance(20);
        simp
    }

    fn set(&mut self, param: ParamId, value: f64) {
        self.rig.apply(Command::SetDeviceParam {
            device: self.device,
            param,
            value,
        });
        self.refresh();
    }

    fn doc_device(&self) -> Device {
        self.rig
            .editor
            .document()
            .device(self.device)
            .unwrap()
            .clone()
    }

    /// Builds what the shell would give: the sample, its slices and the
    /// device in focus with its bank.
    fn refresh(&mut self) {
        let device = self.doc_device();
        let description = s::sampler_type().description;
        let value = |id| description.value(&device.params, id).unwrap();
        let region = device.region.map_or((0, FRAMES), |r| r.frames_in(FRAMES));
        let slices = if value(s::MODE) >= 2.0 {
            if value(s::SLICE_BY) as usize == 3 {
                device
                    .slices
                    .iter()
                    .copied()
                    .filter(|f| *f > region.0 && *f < region.1)
                    .collect()
            } else {
                self.auto.clone()
            }
        } else {
            Vec::new()
        };
        let mut samples = std::collections::BTreeMap::new();
        if self.loaded {
            samples.insert(
                self.device,
                SampleView {
                    peaks: Arc::clone(&self.peaks),
                    frames: FRAMES,
                    rate: RATE,
                    slices: Arc::new(slices),
                },
            );
        }
        self.rig.model.samples = Arc::new(samples);
        let banks = description.banks();
        let bank = self.bank.min(banks.len() - 1);
        let params = banks[bank]
            .params
            .iter()
            .map(|id| {
                let descriptor = description.param(*id).unwrap().clone();
                let value = value(*id);
                ParamView {
                    param: *id,
                    name: descriptor.name.clone(),
                    value,
                    text: descriptor.format(value),
                    held: false,
                    descriptor,
                }
            })
            .collect();
        self.rig.model.selected_device = Some(self.device);
        self.rig.model.rows[0].focus = Some(Focus {
            device: self.device,
            lock: None,
            bank,
            banks: banks.iter().map(|b| b.name.clone()).collect(),
            params,
        });
        self.rig.tick();
    }

    fn text(&self) -> Vec<String> {
        self.rig.snapshot().screen.text()
    }

    fn shows(&self, line: &str) -> bool {
        self.text().iter().any(|l| l == line)
    }

    /// The commands asked for, with their arguments.
    fn asked(&mut self) -> Vec<(String, Value)> {
        self.rig
            .commands()
            .into_iter()
            .filter(|c| !c.starts_with("end "))
            .map(|c| {
                let (id, args) = c.split_once(' ').unwrap();
                (id.to_owned(), serde_json::from_str(args).unwrap())
            })
            .collect()
    }

    /// Turns encoder `column` by `steps` inside one touch.
    fn turn(&mut self, column: u8, steps: i32) -> Vec<(String, Value)> {
        self.asked();
        assert!(self.rig.sim.touch(column));
        assert!(self.rig.sim.turn(column, steps));
        self.rig.advance(1);
        let asked = self.asked();
        assert!(self.rig.sim.release(column));
        self.rig.advance(1);
        asked
    }

    fn state(&self) -> bl_push::simpler::State {
        self.rig
            .snapshot()
            .device
            .expect("Device Mode is shown")
            .simpler
            .expect("a Simpler is in focus")
    }

    /// The second tap on the selected device's button: Edit Mode.
    fn edit(&mut self) {
        self.rig.tap("upper_1");
        assert!(self.rig.snapshot().device.unwrap().edit);
    }

    fn pad_on(&mut self, row: u8, column: u8) {
        assert!(self.rig.sim.press_pad(row, column, 100));
        self.rig.settle();
        self.rig.advance(20);
    }

    fn pad_off(&mut self, row: u8, column: u8) {
        assert!(self.rig.sim.release_pad(row, column));
        self.rig.settle();
        self.rig.advance(20);
    }

    fn tap_pad(&mut self, row: u8, column: u8) {
        self.pad_on(row, column);
        self.pad_off(row, column);
    }
}

// Spec: push-2-notes R67. A Simpler with no sample says to press Browse.
#[test]
fn an_empty_simpler_says_to_press_browse_to_load_a_sample() {
    let mut simp = Simp::new(0.0);
    simp.loaded = false;
    simp.refresh();
    assert!(
        simp.shows("title: Press Browse to load a sample"),
        "{:?}",
        simp.text()
    );
    assert!(simp.state().empty);
    golden("simpler_empty", &simp.rig.snapshot().screen);
}

// Spec: push-2-notes R67, R68. The main bank is the waveform with the
// encoders of the mode, in R68's order, in Classic, One-Shot and Slice.
#[test]
fn the_main_bank_shows_the_waveform_and_each_modes_parameters() {
    let mut simp = Simp::new(0.0);
    for (n, line) in [
        "column 1: Zoom = ×1.0",
        "column 2: Mode = Classic",
        "column 3: Start = 0.000 s",
        "column 4: End = 2.000 s",
        "column 5: S Start = 0.0 %",
        "column 6: S Length = 100 %",
        "column 7: S Loop Length = 100 %",
        "column 8: Gain = 0.00 dB",
    ]
    .iter()
    .enumerate()
    {
        assert!(simp.shows(line), "{n}: {line}: {:?}", simp.text());
    }
    assert!(simp.shows("waveform: region 0 to 960, 1 slices"));
    assert_eq!(simp.state().mode, "classic");
    golden("simpler_classic", &simp.rig.snapshot().screen);

    simp.set(s::MODE, 1.0);
    for line in [
        "column 5: Fade In = 0 ms",
        "column 6: Fade Out = 0 ms",
        "column 7: Transpose = 0 st",
        "column 8: Gain = 0.00 dB",
    ] {
        assert!(simp.shows(line), "{line}: {:?}", simp.text());
    }
    golden("simpler_one_shot", &simp.rig.snapshot().screen);

    simp.set(s::MODE, 2.0);
    for line in [
        "column 2: Mode = Slice",
        "column 5: Slice By = Transient",
        "column 6: Sensitivity = 50 %",
        "column 7: Playback = Mono",
        "column 8: Nudge = slice 1",
    ] {
        assert!(simp.shows(line), "{line}: {:?}", simp.text());
    }
    assert!(simp.shows("waveform: region 0 to 960, 4 slices"));
    golden("simpler_slice", &simp.rig.snapshot().screen);
}

// Spec: push-2-notes R68. Start and End move the region's flags, a
// two-hundredth of what the display shows a step.
#[test]
fn start_and_end_move_the_regions_flags() {
    let mut simp = Simp::new(0.0);
    let asked = simp.turn(2, 20);
    assert_eq!(
        asked,
        [run(
            "device.set_sample_region",
            json!({ "device": simp.device, "start": 9600 })
        )]
    );
    let asked = simp.turn(3, -20);
    assert_eq!(
        asked,
        [run(
            "device.set_sample_region",
            json!({ "device": simp.device, "end": 86_400 })
        )]
    );
}

// Spec: push-2-notes R68. S Start, S Length, S Loop Length and Gain are the
// parameters of Classic's main bank, and each takes its encoder.
#[test]
fn classics_parameters_take_their_encoders() {
    for (column, id) in [
        (4u8, s::START),
        (5, s::LENGTH),
        (6, s::LOOP_LENGTH),
        (7, s::GAIN),
        (1, s::MODE),
    ] {
        let mut simp = Simp::new(0.0);
        let asked = simp.turn(column, 20);
        assert_eq!(asked.len(), 1, "column {column}: {asked:?}");
        assert_eq!(asked[0].0, "device.set_param");
        assert_eq!(asked[0].1["param"], json!(id), "column {column}");
        assert_eq!(asked[0].1["device"], json!(simp.device));
    }
    for (column, id) in [
        (4u8, s::FADE_IN),
        (5, s::FADE_OUT),
        (6, s::TRANSPOSE),
        (7, s::GAIN),
    ] {
        let mut simp = Simp::new(1.0);
        let asked = simp.turn(column, 20);
        assert_eq!(asked.len(), 1, "one-shot column {column}: {asked:?}");
        assert_eq!(asked[0].1["param"], json!(id), "one-shot column {column}");
    }
    for (column, id) in [
        (4u8, s::SLICE_BY),
        (5, s::SLICE_SENSITIVITY),
        (6, s::PLAYBACK),
    ] {
        let mut simp = Simp::new(2.0);
        let asked = simp.turn(column, 20);
        assert_eq!(asked.len(), 1, "slice column {column}: {asked:?}");
        assert_eq!(asked[0].1["param"], json!(id), "slice column {column}");
    }
}

// Spec: push-2-notes R68, R73. Slice's own value follows Slice By: Division
// for Beat, Regions for Region, and nothing for Manual.
#[test]
fn the_slice_by_value_is_the_choice_s_own_parameter() {
    for (by, name, id) in [
        (0.0, "Sensitivity", Some(s::SLICE_SENSITIVITY)),
        (1.0, "Division", Some(s::SLICE_DIVISION)),
        (2.0, "Regions", Some(s::SLICE_REGIONS)),
        (3.0, "", None),
    ] {
        let mut simp = Simp::new(2.0);
        simp.set(s::SLICE_BY, by);
        let line = simp.text().into_iter().find(|l| l.starts_with("column 6"));
        match id {
            Some(id) => {
                assert!(line.unwrap().starts_with(&format!("column 6: {name} =")));
                let asked = simp.turn(5, 20);
                assert_eq!(asked[0].1["param"], json!(id));
            }
            None => {
                assert_eq!(line, None, "Manual has no value");
                assert!(simp.turn(5, 20).is_empty());
            }
        }
    }
}

// Spec: push-2-notes R69. Zoom zooms the display in as it turns clockwise,
// around the position control touched last, and changes only the display.
#[test]
fn zoom_zooms_around_the_last_touched_control_and_changes_nothing() {
    let mut simp = Simp::new(0.0);
    simp.rig.apply(Command::SetDeviceRegion {
        device: simp.device,
        region: Some(SampleRegion::Frames {
            start: 12_000,
            end: 84_000,
        }),
    });
    simp.refresh();
    // Nothing touched yet: around Start.
    assert!(simp.turn(0, 100).is_empty(), "Zoom runs no command");
    let state = simp.state();
    assert_eq!(state.around, "start");
    assert!(
        (state.zoom - 2f64.powf(63.0 / 200.0 * 12.0)).abs() < 1e-6,
        "{}",
        state.zoom
    );
    golden("simpler_zoom_start", &simp.rig.snapshot().screen);
    // Touching End zooms around End.
    simp.turn(3, 1);
    simp.turn(0, 1);
    assert_eq!(simp.state().around, "end");
    // Counter-clockwise zooms out, to the whole sample at the least.
    simp.turn(0, -63);
    simp.turn(0, -63);
    simp.turn(0, -63);
    assert!((simp.state().zoom - 1.0).abs() < 1e-9);
}

// Spec: push-2-notes R69. With slices, Zoom zooms around the selected slice.
#[test]
fn zoom_in_slice_mode_zooms_around_the_selected_slice() {
    let mut simp = Simp::new(2.0);
    // Tapping the third slice's pad selects it; Nudge was touched last.
    simp.tap_pad(0, 2);
    assert_eq!(simp.state().selected, 2);
    simp.turn(7, 0);
    simp.turn(0, 100);
    assert_eq!(simp.state().around, "slice");
    golden("simpler_zoom_slice", &simp.rig.snapshot().screen);
}

// Spec: push-2-notes R70, R71. Pressing Simpler's button again opens Edit
// Mode, where the upper buttons of the mode run their commands.
#[test]
fn edit_mode_runs_loop_warp_as_the_halves_crop_and_reverse() {
    let mut simp = Simp::new(0.0);
    simp.edit();
    for line in [
        "upper 1: Back<left arrow>",
        "upper 2: Loop",
        "upper 3: Warp as 1 bar",
        "upper 4: ÷2",
        "upper 5: ×2",
        "upper 6: Crop",
        "upper 7: Reverse",
    ] {
        assert!(simp.shows(line), "{line}: {:?}", simp.text());
    }
    let names: Vec<_> = simp.state().buttons.iter().map(|b| b.name).collect();
    assert_eq!(
        names,
        ["loop", "warp_as", "half", "double", "crop", "reverse"]
    );
    simp.asked();
    simp.rig.tap("upper_2");
    assert_eq!(
        simp.asked(),
        [run(
            "device.set_param",
            json!({ "device": simp.device, "param": s::LOOP, "value": 1.0 })
        )]
    );
    simp.rig.tap("upper_4");
    assert_eq!(
        simp.asked(),
        [run(
            "device.scale_tempo",
            json!({ "device": simp.device, "factor": 0.5 })
        )]
    );
    simp.rig.tap("upper_5");
    assert_eq!(
        simp.asked(),
        [run(
            "device.scale_tempo",
            json!({ "device": simp.device, "factor": 2.0 })
        )]
    );
    simp.rig.tap("upper_6");
    assert_eq!(
        simp.asked(),
        [run("device.crop_sample", json!({ "device": simp.device }))]
    );
    simp.rig.tap("upper_7");
    assert_eq!(
        simp.asked(),
        [run(
            "device.reverse_sample",
            json!({ "device": simp.device })
        )]
    );
}

// Spec: push-2-notes R70. Warp as turns Warp on and sets the sample's tempo
// so the region plays in the nearest power of two bars at the set's tempo,
// as one undo step.
#[test]
fn warp_as_guesses_the_bars_from_the_regions_length_and_the_tempo() {
    let mut simp = Simp::new(0.0);
    // Two seconds at 120 BPM (the rig's) are one bar.
    simp.edit();
    simp.rig.model.tempo = bl_core::time::Bpm::new(150.0).unwrap();
    simp.rig.advance(20);
    // Two seconds at 150 BPM are 1.25 bars: one.
    assert!(simp.shows("upper 3: Warp as 1 bar"), "{:?}", simp.text());
    simp.rig.model.tempo = bl_core::time::Bpm::new(240.0).unwrap();
    simp.rig.advance(20);
    // Two bars at 240 BPM.
    assert!(simp.shows("upper 3: Warp as 2 bars"));
    simp.asked();
    simp.rig.tap("upper_3");
    let requests = simp.rig.requests();
    let ids: Vec<_> = requests
        .iter()
        .filter_map(|r| match &r.ask {
            bl_control::Ask::Command(c) => Some((c.id.clone(), c.args.clone(), c.gesture)),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0].0, "device.set_warp");
    assert_eq!(ids[1].0, "device.set_sample_bpm");
    // Two bars, eight beats, in two seconds: 240 BPM.
    assert!((ids[1].1["bpm"].as_f64().unwrap() - 240.0).abs() < 1e-6);
    assert!(ids[0].2.is_some_and(|g| !g.released) && ids[1].2.is_some_and(|g| g.released));
}

// Spec: push-2-notes R70, R72. The lower buttons choose Simpler's banks, the
// second the Global bank with Glide Mode and Voices.
#[test]
fn edit_modes_second_lower_button_is_the_global_bank() {
    let mut simp = Simp::new(0.0);
    simp.edit();
    assert!(
        simp.shows("lower 1: Simpler (selected)"),
        "{:?}",
        simp.text()
    );
    assert!(simp.shows("lower 2: Global"));
    simp.asked();
    simp.rig.tap("lower_2");
    assert_eq!(
        simp.asked(),
        [run(
            "control_surface.set_bank",
            json!({ "row": 1, "bank": 1 })
        )]
    );
    simp.bank = 1;
    simp.refresh();
    assert!(
        simp.shows("column 1: Glide Mode = Glide"),
        "{:?}",
        simp.text()
    );
    assert!(simp.shows("column 2: Voices = 8"));
    assert!(simp.state().buttons.len() == 6, "Edit Mode's buttons stay");
}

// Spec: push-2-notes R70, R71. In One-Shot and Slice the first button is
// Trigger or Gate; the region's own buttons differ by mode.
#[test]
fn trigger_gate_and_the_slice_buttons() {
    let mut simp = Simp::new(1.0);
    simp.edit();
    assert!(simp.shows("upper 2: Trigger"), "{:?}", simp.text());
    simp.asked();
    simp.rig.tap("upper_2");
    assert_eq!(
        simp.asked(),
        [run(
            "device.set_param",
            json!({ "device": simp.device, "param": s::TRIGGER, "value": 0.0 })
        )]
    );
    simp.set(s::TRIGGER, 0.0);
    assert!(simp.shows("upper 2: Gate"));

    let mut simp = Simp::new(2.0);
    simp.edit();
    let names: Vec<_> = simp.state().buttons.iter().map(|b| b.name).collect();
    assert_eq!(
        names,
        [
            "trigger_gate",
            "pad_slicing",
            "split_slice",
            "warp_as",
            "half",
            "double",
            "reverse"
        ]
    );
}

// Spec: push-2-notes R73. Split Slice adds a slice halfway through the
// selected one.
#[test]
fn split_slice_halves_the_selected_slice() {
    let mut simp = Simp::new(2.0);
    simp.tap_pad(0, 1);
    simp.edit();
    simp.asked();
    simp.rig.tap("upper_4");
    assert_eq!(
        simp.asked(),
        [run(
            "device.add_slice",
            json!({ "device": simp.device, "frame": 36_000 })
        )]
    );
}

// Spec: push-2-notes R73. Nudge moves the selected slice's start, a tenth as
// far with Shift held; a slice placed by hand only.
#[test]
fn nudge_moves_the_selected_manual_slice_a_tenth_as_far_with_shift() {
    let mut simp = Simp::new(2.0);
    simp.set(s::SLICE_BY, 3.0);
    for frame in [24_000u64, 48_000] {
        simp.rig.apply(Command::AddSlice {
            device: simp.device,
            frame,
        });
    }
    simp.refresh();
    simp.tap_pad(0, 1);
    assert_eq!(simp.state().selected, 1);
    let moved = simp.turn(7, 20);
    let from_to = |asked: &[(String, Value)]| {
        assert_eq!(asked.len(), 1, "{asked:?}");
        assert_eq!(asked[0].0, "device.move_slice");
        (
            asked[0].1["from"].as_u64().unwrap(),
            asked[0].1["to"].as_u64().unwrap(),
        )
    };
    // The display spans the whole sample: 1/200 of it, 480 frames, a step.
    assert_eq!(from_to(&moved), (24_000, 33_600));
    assert!(simp.rig.sim.press("shift"));
    simp.rig.settle();
    let fine = simp.turn(7, 20);
    assert_eq!(from_to(&fine), (24_000, 24_960));
    assert!(simp.rig.sim.let_go("shift"));
    simp.rig.advance(1);
    // An automatic slice isn't placed by hand: Nudge leaves it.
    let mut auto = Simp::new(2.0);
    auto.tap_pad(0, 1);
    assert!(auto.turn(7, 20).is_empty());
}

// Spec: push-2-notes R73. With Delete held, tapping a pad deletes its slice;
// tapping a pad selects its slice and plays it.
#[test]
fn delete_and_a_pad_deletes_its_slice() {
    let mut simp = Simp::new(2.0);
    simp.set(s::SLICE_BY, 3.0);
    simp.rig.apply(Command::AddSlice {
        device: simp.device,
        frame: 30_000,
    });
    simp.refresh();
    simp.asked();
    simp.rig.sim.press("delete");
    simp.rig.settle();
    simp.tap_pad(0, 1);
    simp.rig.sim.let_go("delete");
    simp.rig.advance(1);
    assert!(simp.asked().contains(&run(
        "device.delete_slice",
        json!({ "device": simp.device, "frame": 30_000 })
    )));
    // A tap plays the slice's key and selects it.
    simp.rig.reached_tracks();
    simp.tap_pad(0, 0);
    let notes: Vec<_> = simp
        .rig
        .reached_tracks()
        .into_iter()
        .map(|e| e.message.data1())
        .collect();
    assert_eq!(notes, [FIRST_KEY, FIRST_KEY]);
}

// Spec: push-2-notes R74. With Slice By Manual and Pad Slicing on, tapping
// an empty pad while the sample plays adds a slice where it plays, and the
// slices after it move one pad on; Pad Slicing off ends it.
#[test]
fn pad_slicing_adds_a_slice_where_the_sample_plays() {
    let mut simp = Simp::new(2.0);
    simp.set(s::SLICE_BY, 3.0);
    simp.rig.apply(Command::AddSlice {
        device: simp.device,
        frame: 60_000,
    });
    simp.refresh();
    simp.edit();
    simp.rig.tap("upper_3");
    assert!(simp.state().pad_slicing);
    assert!(
        simp.shows("upper 3: Pad Slicing (selected)"),
        "{:?}",
        simp.text()
    );
    // The first pad plays the sample from the region's start.
    simp.asked();
    simp.pad_on(0, 0);
    simp.rig.advance(500);
    // An empty pad, half a second in: 24,000 frames.
    simp.tap_pad(1, 0);
    assert!(simp.asked().iter().any(|(id, args)| {
        id == "device.add_slice"
            && args["frame"]
                .as_u64()
                .is_some_and(|f| (23_000..=25_500).contains(&f))
    }),);
    simp.pad_off(0, 0);
    // Off, an empty pad does nothing.
    simp.rig.tap("upper_3");
    assert!(!simp.state().pad_slicing);
    simp.pad_on(0, 0);
    simp.rig.advance(500);
    simp.tap_pad(1, 0);
    assert!(!simp.asked().iter().any(|(id, _)| id == "device.add_slice"));
}

// Spec: push-2-notes R75. A slicing Simpler's pads are its slices, in groups
// of four from the bottom left, the left half then the right half; each pad
// plays its slice's key.
#[test]
fn forty_slices_fill_the_pads_in_the_order_of_the_spec() {
    let mut simp = Simp::new(2.0);
    simp.auto = (1..40).map(|n| n * 2_400).collect();
    simp.refresh();
    let lit = |simp: &Simp, row: u8, column: u8| {
        simp.rig.sim.colour(&format!("pad_{row}_{column}")).unwrap() != [0, 0, 0]
    };
    for row in 0..8u8 {
        for column in 0..8u8 {
            let at = u32::from(column / 4) * 32 + u32::from(row) * 4 + u32::from(column % 4);
            assert_eq!(lit(&simp, row, column), at < 40, "pad {row},{column}");
        }
    }
    simp.rig.reached_tracks();
    simp.tap_pad(0, 4);
    let first: Vec<u8> = simp
        .rig
        .reached_tracks()
        .into_iter()
        .map(|e: MidiInputEvent| e.message.data1())
        .collect();
    assert_eq!(first[0], FIRST_KEY + 32, "the right half starts at 33");
    simp.rig.reached_tracks();
    // A pad past the last slice plays nothing.
    simp.tap_pad(7, 7);
    assert!(simp.rig.reached_tracks().is_empty());
}

// Spec: push-2-notes R71. Shift with Play/Stop silences the selected
// track's Simpler: All Sound Off reaches the track.
#[test]
fn shift_and_play_stop_silences_the_simpler() {
    let mut simp = Simp::new(1.0);
    simp.rig.reached_tracks();
    assert!(simp.rig.sim.press("shift"));
    simp.rig.settle();
    simp.rig.tap("play");
    simp.rig.sim.let_go("shift");
    simp.rig.advance(1);
    let off: Vec<_> = simp
        .rig
        .reached_tracks()
        .into_iter()
        .map(|e| (e.message.status(), e.message.data1()))
        .collect();
    assert!(off.contains(&(0xB0, 120)), "{off:?}");
}

// Spec: push-2-notes R76. Convert runs `push.convert` for a Simpler, with
// nothing to ask; for an audio clip it opens its choices on the upper
// buttons, of which Convert Harmony, Melody and Drums are listed and do
// nothing; with nothing to convert it does nothing.
#[test]
fn convert_runs_its_command_and_an_audio_clip_offers_its_choices() {
    let mut simp = Simp::new(0.0);
    simp.asked();
    simp.rig.tap("convert");
    assert!(simp.asked().is_empty(), "nothing is selected to convert");
    for what in [
        bl_control::Convert::Simpler,
        bl_control::Convert::Slices,
        bl_control::Convert::Pad,
    ] {
        simp.rig.model.convert = what;
        simp.rig.advance(20);
        simp.rig.tap("convert");
        assert_eq!(simp.asked(), [run("push.convert", json!({}))], "{what:?}");
    }

    simp.rig.model.convert = bl_control::Convert::Clip;
    simp.rig.advance(20);
    simp.rig.tap("convert");
    for line in [
        "upper 1: Simpler",
        "upper 2: Drum Rack",
        "upper 3: Convert Harmony",
        "upper 4: Convert Melody",
        "upper 5: Convert Drums",
    ] {
        assert!(simp.shows(line), "{line}: {:?}", simp.text());
    }
    assert_eq!(
        simp.rig.snapshot().menu,
        Some(bl_push::state::Menu::Convert)
    );
    assert!(simp.asked().is_empty(), "opening the choices runs nothing");
    // A choice not built does nothing and closes the menu.
    simp.rig.tap("upper_4");
    assert!(simp.asked().is_empty());
    assert_eq!(simp.rig.snapshot().menu, None);
    simp.rig.tap("convert");
    simp.rig.tap("upper_2");
    assert_eq!(
        simp.asked(),
        [run("push.convert", json!({ "to": "drum_rack" }))]
    );
    simp.rig.tap("convert");
    simp.rig.tap("upper_1");
    assert_eq!(
        simp.asked(),
        [run("push.convert", json!({ "to": "simpler" }))]
    );
    // The choices close when the clip is no longer selected.
    simp.rig.tap("convert");
    simp.rig.model.convert = bl_control::Convert::Nothing;
    simp.rig.advance(20);
    assert_eq!(simp.rig.snapshot().menu, None);
}

//! Looper (WP-5.26b; the looper spec). Every block runs as a real-time
//! section under the allocation trap.

use crate::common;
use bl_core::time::{Bpm, Frames, TempoMap};
use bl_devices::looper::{
    self, AFTER_RECORD, Action, FEEDBACK, INPUT_OUTPUT, QUANTIZATION, RECORD_LENGTH, REVERSE,
    SONG_CONTROL, SPEED, TEMPO_CONTROL, TRANSPORT,
};
use bl_engine::{Collector, Node, NodeEvent, TransportInfo};
use bl_model::{DeviceKind, ParamId};

/// A 1/32 note at 120 BPM and 48 kHz: the grid step the tests use.
const STEP: usize = 3_000;
/// A 4/4 bar at 120 BPM and 48 kHz.
const BAR: i64 = 96_000;

/// The Transport chooser's values.
const RECORD: f64 = 0.0;
const OVERDUB: f64 = 1.0;
const PLAY: f64 = 2.0;
const STOP: f64 = 3.0;

/// The Quantization chooser's values the tests use.
const Q_OFF: f64 = 0.0;
const Q_BAR: f64 = 4.0;
const Q_STEP: f64 = 13.0;

/// The Input → Output chooser's values.
const IO_ALWAYS: f64 = 0.0;
const IO_NEVER: f64 = 1.0;
const IO_REC_OVR: f64 = 2.0;

/// What the tests record into the loop, so a played-back layer is telling.
const LAYER: f32 = 0.5;

/// A Looper node driven block by block as the engine drives it.
struct Rig {
    node: Box<dyn Node>,
    collector: Collector,
    tempo: TempoMap,
    position: i64,
    playing: bool,
    block: usize,
    /// The display reader, taken once.
    display: Option<bl_engine::DisplayReader>,
}

impl Rig {
    fn new(params: &[(ParamId, f64)], playing: bool, block: usize) -> Self {
        let registry = common::registry();
        let device = common::device(&bl_devices::looper_type(), params);
        let node = common::node(&registry, &device, block.max(1));
        Self {
            node,
            collector: Collector::new(),
            tempo: TempoMap::default(),
            position: 0,
            playing,
            block,
            display: None,
        }
    }

    /// Runs one block of the same `input` on every channel.
    fn block(&mut self, input: f32, events: &[NodeEvent]) -> Vec<Vec<f32>> {
        let input: Vec<Vec<f32>> = (0..common::CHANNELS)
            .map(|_| vec![input; self.block])
            .collect();
        self.block_with(&input, events)
    }

    /// Runs one block of per-channel `input`.
    fn block_with(&mut self, input: &[Vec<f32>], events: &[NodeEvent]) -> Vec<Vec<f32>> {
        let transport = TransportInfo {
            playing: self.playing,
            position: Frames(self.position),
        };
        let out = common::run_at(&mut *self.node, input, events, &self.tempo, transport);
        if self.playing {
            self.position += self.block as i64;
        }
        out
    }

    /// Runs `blocks` blocks of the same `input`, one `events` set each.
    fn run(&mut self, blocks: usize, input: f32, events: &[Vec<NodeEvent>]) -> Vec<Vec<f32>> {
        let mut out = Vec::new();
        for i in 0..blocks {
            out = self.block(input, events.get(i).map_or(&[][..], Vec::as_slice));
        }
        out
    }

    fn set(&self, param: ParamId, value: f64) -> NodeEvent {
        common::set(0, param, value)
    }

    fn send(&mut self, action: Action) {
        let data = self.collector.handle().data(action);
        self.node.receive_data(data);
    }

    /// The display data the node published last.
    fn values(&mut self) -> Vec<f32> {
        if self.display.is_none() {
            self.display = self.node.display();
        }
        let reader = self.display.as_mut().expect("Looper has a display");
        reader.read().values.clone()
    }
}

/// A Looper with the song playing on a 1/32 grid, recording `LAYER` into a
/// one-step loop, playing from `shape`'s start.
fn recorded(params: &[(ParamId, f64)], shape: Option<Vec<Vec<f32>>>) -> Rig {
    let mut rig = Rig::new(params, true, STEP);
    // Block 0 asks for Record, which waits for the grid line: block 1
    // records.
    let event = rig.set(TRANSPORT, RECORD);
    let _ = rig.block(0.0, &[event]);
    // Block 1 records the step; the Play asked for there ends it on the next
    // grid line, so the loop is exactly one step long.
    let input: Vec<Vec<f32>> = (0..common::CHANNELS)
        .map(|_| match &shape {
            Some(channels) => channels[0].clone(),
            None => vec![LAYER; STEP],
        })
        .collect();
    let event = rig.set(TRANSPORT, PLAY);
    let _ = rig.block_with(&input, &[event]);
    rig
}

/// A rig with the choices Looper's tests use.
fn choices(input_output: f64, quantization: f64) -> Vec<(ParamId, f64)> {
    vec![
        (TRANSPORT, STOP),
        (QUANTIZATION, quantization),
        (INPUT_OUTPUT, input_output),
        (TEMPO_CONTROL, 0.0),
    ]
}

/// The highest index above `after` where the output sounds.
fn burst_at(out: &[f32], after: usize) -> Option<usize> {
    out.iter()
        .enumerate()
        .skip(after)
        .find(|(_, s)| s.abs() > 0.25)
        .map(|(i, _)| i)
}

// Spec: looper R1
#[test]
fn looper_is_described_and_laid_out() {
    let type_ = looper::looper_type();
    assert_eq!(type_.name, "Looper");
    assert_eq!(
        type_.kind,
        DeviceKind::Native {
            device_type: looper::LOOPER.to_owned()
        }
    );
    assert_eq!(type_.description.params.len(), 10);
    assert!(type_.description.check().is_ok());
    assert!(type_.description.params.iter().all(|p| p.automatable));
}

// Spec: looper R4, R5
#[test]
fn record_starts_on_the_grid_line_while_the_song_plays_and_at_once_while_stopped() {
    // Input → Output hears the input while recording, so the output says
    // exactly when Transport became Record.
    let mut playing = Rig::new(&choices(IO_REC_OVR, Q_STEP), true, STEP);
    let before = playing.block(1.0, &[]);
    let event = playing.set(TRANSPORT, RECORD);
    let waiting = playing.block(1.0, &[event]);
    let recording = playing.block(1.0, &[]);
    assert!(before[0].iter().all(|s| *s == 0.0), "stopped: silent");
    assert!(
        waiting[0].iter().all(|s| *s == 0.0),
        "the change waits for the grid line"
    );
    assert!(
        recording[0].iter().all(|s| *s == 1.0),
        "recording from the grid line"
    );

    let mut stopped = Rig::new(&choices(IO_REC_OVR, Q_STEP), false, STEP);
    let event = stopped.set(TRANSPORT, RECORD);
    let at_once = stopped.block(1.0, &[event]);
    assert!(
        at_once[0].iter().all(|s| *s == 1.0),
        "with the song stopped the change takes effect at once"
    );
}

// Spec: looper R5
#[test]
fn a_queued_transport_change_taken_back_before_its_grid_line_is_dropped() {
    // Record is asked for, then Stop again before the bar line: nothing
    // records, so every block is silent.
    let mut rig = Rig::new(&choices(IO_REC_OVR, Q_BAR), true, STEP);
    let record = rig.set(TRANSPORT, RECORD);
    let stop = rig.set(TRANSPORT, STOP);
    let mut events = vec![vec![record], vec![stop]];
    events.extend((2..40).map(|_| Vec::new()));
    let last = rig.run(40, 1.0, &events);
    assert!(
        last[0].iter().all(|s| *s == 0.0),
        "the queued Record was taken back: {}",
        last[0][0]
    );
}

// Spec: looper R6, R18, R19
#[test]
fn a_fixed_record_length_ends_after_that_many_bars_and_switches_as_told() {
    for (after, expected) in [(0.0, PLAY), (1.0, OVERDUB)] {
        let mut rig = Rig::new(
            &[
                (TRANSPORT, STOP),
                (QUANTIZATION, Q_BAR),
                (RECORD_LENGTH, 1.0),
                (AFTER_RECORD, after),
                (INPUT_OUTPUT, IO_NEVER),
            ],
            true,
            STEP,
        );
        let event = rig.set(TRANSPORT, RECORD);
        let mut events = vec![vec![event]];
        events.extend((1..65).map(|_| Vec::new()));
        rig.run(65, 1.0, &events);
        let values = rig.values();
        assert_eq!(f64::from(values[0]), expected, "the state after the length");
        assert_eq!(values[1], 0.0, "the buffer holds the recording");
        assert_eq!(
            (values[7] as i64, values[8] as u32, values[9] as u32),
            (1, 0, 0),
            "one bar long"
        );
    }
}

// Spec: looper R7
#[test]
fn overdub_sums_the_layers_over_the_recordings_length() {
    let mut rig = recorded(&choices(IO_NEVER, Q_STEP), None);
    let played = rig.block(0.0, &[]);
    assert!(
        played[0].iter().all(|s| (*s - LAYER).abs() < 1e-6),
        "the recording plays back"
    );
    // The Overdub asked for in block 2 acts in block 3: that pass plays the
    // layer it found and adds 0.25 to it, which the layer below doesn't
    // hear again until the pass after.
    let event = rig.set(TRANSPORT, OVERDUB);
    let _ = rig.block(0.25, &[event]);
    let pass = rig.block(0.25, &[]);
    assert!(
        pass[0].iter().all(|s| (*s - LAYER).abs() < 1e-6),
        "the pass plays the layer before it: {}",
        pass[0][0]
    );
    // The next pass hears the sum.
    let next = rig.block(0.0, &[]);
    assert!(
        next[0].iter().all(|s| (*s - (LAYER + 0.25)).abs() < 1e-6),
        "the layer is part of the loop: {}",
        next[0][0]
    );
}

// Spec: looper R8
#[test]
fn play_sounds_the_buffer_and_stop_silences_it() {
    let mut rig = recorded(&choices(IO_NEVER, Q_STEP), None);
    let playing = rig.block(0.0, &[]);
    assert!(playing[0].iter().all(|s| (*s - LAYER).abs() < 1e-6));
    let event = rig.set(TRANSPORT, STOP);
    let _ = rig.block(0.0, &[event]);
    let stopped = rig.block(0.0, &[]);
    assert!(stopped[0].iter().all(|s| *s == 0.0), "Stop silences it");
}

// Spec: looper R9
#[test]
fn clear_in_overdub_keeps_the_length_and_elsewhere_resets_it() {
    let mut rig = recorded(&choices(IO_NEVER, Q_STEP), None);
    let event = rig.set(TRANSPORT, OVERDUB);
    let _ = rig.block(LAYER, &[event]);
    // The Overdub asked for takes effect on the grid line, so the next block
    // is the pass it makes.
    let _ = rig.block(LAYER, &[]);
    assert_eq!(
        f64::from(rig.values()[0]),
        OVERDUB,
        "the pass is overdubbing"
    );
    rig.send(Action::Clear);
    let cleared = rig.block(0.0, &[]);
    assert!(cleared[0].iter().all(|s| *s == 0.0), "the samples are gone");
    let values = rig.values();
    assert_eq!(
        values[16], STEP as f32,
        "the length is kept: the song plays"
    );
    assert_eq!(
        f64::from(values[0]),
        OVERDUB,
        "and Overdub is still the state"
    );
    // Elsewhere Clear resets the length too.
    let event = rig.set(TRANSPORT, STOP);
    let _ = rig.block(0.0, &[event]);
    let _ = rig.block(0.0, &[]);
    rig.send(Action::Clear);
    let _ = rig.block(0.0, &[]);
    let values = rig.values();
    assert_eq!(values[1], 1.0, "the buffer is empty again");
    assert_eq!(values[16], 0.0, "the length is reset too");
    assert_eq!(f64::from(values[0]), STOP, "and Looper is stopped");
}

// Spec: looper R9
#[test]
fn clear_in_overdub_while_the_song_is_stopped_resets_the_length_too() {
    let mut rig = recorded(&choices(IO_NEVER, Q_STEP), None);
    // With the song stopped, the Overdub asked for acts at once.
    rig.playing = false;
    let event = rig.set(TRANSPORT, OVERDUB);
    let _ = rig.block(LAYER, &[event]);
    assert_eq!(
        f64::from(rig.values()[0]),
        OVERDUB,
        "the pass is overdubbing"
    );
    rig.send(Action::Clear);
    let cleared = rig.block(0.0, &[]);
    assert!(cleared[0].iter().all(|s| *s == 0.0), "the samples are gone");
    let values = rig.values();
    assert_eq!(values[1], 1.0, "the buffer is empty");
    assert_eq!(
        values[16], 0.0,
        "and the length is reset: the song is stopped"
    );
}

// Spec: looper R10
#[test]
fn undo_removes_the_last_pass_and_redo_restores_it_bit_for_bit() {
    let mut rig = recorded(&choices(IO_NEVER, Q_STEP), None);
    let event = rig.set(TRANSPORT, OVERDUB);
    let _ = rig.block(0.25, &[event]);
    let _ = rig.block(0.25, &[]);
    rig.send(Action::Undo);
    let undone = rig.block(0.0, &[]);
    assert!(
        undone[0].iter().all(|s| (*s - LAYER).abs() < 1e-6),
        "the overdub pass is gone: {}",
        undone[0][0]
    );
    rig.send(Action::Redo);
    let redone = rig.block(0.0, &[]);
    assert!(
        redone[0].iter().all(|s| (*s - (LAYER + 0.25)).abs() < 1e-6),
        "and it comes back exactly: {}",
        redone[0][0]
    );
}

// Spec: looper R12
#[test]
fn double_repeats_the_material_and_halve_keeps_the_playing_half() {
    let mut rig = recorded(&choices(IO_NEVER, Q_STEP), None);
    let one_bar = {
        let _ = rig.block(0.0, &[]);
        rig.values()[16]
    };
    assert_eq!(one_bar, STEP as f32, "one step recorded");
    rig.send(Action::Double);
    let doubled = rig.run(2, 0.0, &[]);
    assert!(
        doubled[0].iter().all(|s| (*s - LAYER).abs() < 1e-6),
        "the doubled half holds the same material"
    );
    rig.send(Action::Halve);
    let halved = rig.run(2, 0.0, &[]);
    assert!(
        halved[0].iter().all(|s| (*s - LAYER).abs() < 1e-6),
        "and the half that plays keeps it"
    );
    // Halving it back gives the length the recording started with.
    let values = rig.values();
    assert_eq!(values[16], one_bar, "the length is the recording's again");
}

// Spec: looper R13
#[test]
fn speed_sets_the_rate_of_playback() {
    // At half speed a one-step loop takes two steps to play.
    let shape: Vec<Vec<f32>> = (0..common::CHANNELS)
        .map(|_| (0..STEP).map(|i| if i < 100 { 1.0 } else { 0.0 }).collect())
        .collect();
    let mut rig = recorded(
        &[
            (TRANSPORT, STOP),
            (QUANTIZATION, Q_STEP),
            (INPUT_OUTPUT, IO_NEVER),
            (TEMPO_CONTROL, 0.0),
            (SPEED, 0.5),
        ],
        Some(shape),
    );
    let mut out = rig.block(0.0, &[])[0].clone();
    out.extend(rig.block(0.0, &[])[0].clone());
    out.extend(rig.block(0.0, &[])[0].clone());
    let again = burst_at(&out, 250).expect("the loop repeats");
    assert_eq!(
        again,
        2 * STEP,
        "half speed doubles the loop's duration: {again}"
    );
}

// Spec: looper R14
#[test]
fn reverse_plays_the_buffer_backwards_from_its_position() {
    let shape: Vec<Vec<f32>> = (0..common::CHANNELS)
        .map(|_| (0..STEP).map(|i| i as f32 / STEP as f32).collect())
        .collect();
    let mut rig = recorded(&choices(IO_NEVER, Q_STEP), Some(shape));
    let event = rig.set(REVERSE, 1.0);
    let _ = rig.block(0.0, &[]);
    // R14 through R5: engaging Reverse waits for the grid line.
    let waiting = rig.block(0.0, &[event]);
    assert!(
        waiting[0][0] < 0.1,
        "the change waits for the grid line: {}",
        waiting[0][0]
    );
    let reversed = rig.block(0.0, &[]);
    assert!(
        reversed[0][0] > 0.9,
        "Reverse starts at the end of the recording: {}",
        reversed[0][0]
    );
    assert!(
        reversed[0][STEP - 1] < 0.1,
        "and runs to its start: {}",
        reversed[0][STEP - 1]
    );
}

// Spec: looper R15
#[test]
fn feedback_halves_the_old_layer_each_pass_within_a_hundredth_of_a_db() {
    let mut rig = recorded(
        &[
            (TRANSPORT, STOP),
            (QUANTIZATION, Q_STEP),
            (INPUT_OUTPUT, IO_NEVER),
            (TEMPO_CONTROL, 0.0),
            (FEEDBACK, 50.0),
        ],
        None,
    );
    let event = rig.set(TRANSPORT, OVERDUB);
    let _ = rig.block(0.0, &[event]);
    let _ = rig.block(0.0, &[]); // the pass whose wrap decays the layer
    let after_one = rig.block(0.0, &[]);
    let _ = rig.block(0.0, &[]); // a pass whose wrap decays it again
    let after_two = rig.block(0.0, &[]);
    let db = |value: f32| 20.0 * (value as f64).max(1e-12).log10();
    assert!(
        (db(after_one[0][0]) - db(LAYER / 2.0)).abs() < 0.01,
        "half after one pass: {} dB",
        db(after_one[0][0])
    );
    assert!(
        (db(after_two[0][0]) - db(LAYER / 8.0)).abs() < 0.01,
        "an eighth after three passes: {} dB",
        db(after_two[0][0])
    );
}

// Spec: looper R16
#[test]
fn each_input_to_output_mode_passes_the_input_only_in_its_states() {
    for (mode, record, overdub, play, stop) in [
        (IO_ALWAYS, true, true, true, true),
        (IO_NEVER, false, false, false, false),
        (IO_REC_OVR, true, true, false, false),
        (3.0, true, true, false, true),
    ] {
        // The loop holds LAYER; the input is loud enough to tell them apart.
        let heard = |state: f64| {
            let mut rig = recorded(&choices(mode, Q_STEP), None);
            if state != PLAY {
                let event = rig.set(TRANSPORT, state);
                let _ = rig.block(0.0, &[event]);
                let _ = rig.block(0.0, &[]);
            }
            let out = rig.block(1.0, &[]);
            out[0].iter().any(|s| *s > 0.75)
        };
        assert_eq!(heard(RECORD), record, "mode {mode}, Record");
        assert_eq!(heard(OVERDUB), overdub, "mode {mode}, Overdub");
        assert_eq!(heard(PLAY), play, "mode {mode}, Play");
        assert_eq!(heard(STOP), stop, "mode {mode}, Stop");
    }
}

// Spec: looper R20, R21
#[test]
fn a_loop_recorded_at_120_plays_at_140_over_120_with_follow() {
    // One bar recorded with the song stopped and a fixed length is taken as
    // exactly that bar, so half a bar of frames is 240 BPM: at a song
    // playing at 480 BPM it then plays twice as fast.
    let mut rig = Rig::new(
        &[
            (TRANSPORT, RECORD),
            (QUANTIZATION, Q_OFF),
            (RECORD_LENGTH, 1.0),
            (INPUT_OUTPUT, IO_NEVER),
            (TEMPO_CONTROL, 1.0),
        ],
        false,
        (BAR / 2) as usize,
    );
    let _ = rig.block(0.0, &[]);
    let event = rig.set(TRANSPORT, STOP);
    let _ = rig.block(0.0, &[event]);
    assert_eq!(rig.values()[16], (BAR / 2) as f32, "half a bar recorded");
    assert!(
        (rig.values()[10] - 240.0).abs() < 1.0,
        "recorded at 240 BPM"
    );

    rig.playing = true;
    rig.position = 0;
    let event = rig.set(TRANSPORT, PLAY);
    let _ = rig.block(0.0, &[event]);
    let step = 12_000;
    rig.block = step;

    // At 240 BPM the loop plays at its recorded speed.
    rig.tempo = TempoMap::constant(Bpm::new(240.0).expect("a tempo"));
    let _ = rig.block(0.0, &[]);
    let at_240 = rig.values()[17];
    // At 480 BPM it plays twice as fast.
    rig.tempo = TempoMap::constant(Bpm::new(480.0).expect("a tempo"));
    let _ = rig.block(0.0, &[]);
    let at_480 = rig.values()[17];
    assert_eq!(
        at_240, step as f32,
        "at its own tempo the loop moves a step"
    );
    assert_eq!(
        at_480,
        (3 * step) as f32,
        "at twice the tempo it moves twice as far"
    );
}

// Spec: looper R22
#[test]
fn set_and_follow_asks_for_the_tempo_a_fixed_recording_gives() {
    // With the song stopped, a fixed one-bar length makes the frames
    // recorded exactly one bar, so half a bar of frames is 240 BPM.
    let mut rig = Rig::new(
        &[
            (TRANSPORT, RECORD),
            (QUANTIZATION, Q_BAR),
            (RECORD_LENGTH, 1.0),
            (INPUT_OUTPUT, IO_NEVER),
            (TEMPO_CONTROL, 2.0),
        ],
        false,
        (BAR / 2) as usize,
    );
    let _ = rig.block(LAYER / 2.0, &[]);
    let event = rig.set(TRANSPORT, STOP);
    let _ = rig.block(0.0, &[event]);
    let values = rig.values();
    assert_eq!(values[11], 3.0, "a tempo request");
    assert!(
        (values[15] - 240.0).abs() < 1.0,
        "half a bar of frames is 240 BPM: {}",
        values[15]
    );
}

// Spec: looper R23, R24
#[test]
fn song_control_asks_the_song_to_start_and_stop_once_each() {
    let mut rig = Rig::new(
        &[
            (TRANSPORT, STOP),
            (QUANTIZATION, Q_STEP),
            (INPUT_OUTPUT, IO_NEVER),
            (SONG_CONTROL, 2.0),
        ],
        false,
        STEP,
    );
    let event = rig.set(TRANSPORT, PLAY);
    let _ = rig.block(0.0, &[event]);
    let values = rig.values();
    assert_eq!(values[11], 1.0, "a start request, as the song is stopped");
    let serial = values[12];
    let _ = rig.block(0.0, &[]);
    let values = rig.values();
    assert_eq!(values[12], serial, "a request is asked once");
}

// Spec: looper R26
#[test]
fn the_position_and_length_read_in_bars_and_beats_and_the_state_says_recording() {
    let mut rig = Rig::new(&choices(IO_NEVER, Q_STEP), true, STEP);
    let event = rig.set(TRANSPORT, RECORD);
    let _ = rig.block(0.0, &[event]);
    let _ = rig.block(0.0, &[]);
    let values = rig.values();
    assert_eq!(f64::from(values[0]), RECORD, "the state says recording");
    assert_eq!(values[4], 1.0, "a new recording is at the loop's start");
    // The display's reading: `device.display` says the same.
    let display = looper::display(
        bl_model::DeviceId(bl_core::id::Id::new(1).unwrap()),
        &looper::looper_type().description,
        &Default::default(),
        Some(&values),
    );
    assert_eq!(
        display.reading("State").map(|r| r.text.as_str()),
        Some("Record")
    );
}

// Spec: looper R27
#[test]
fn renders_are_bit_identical_at_any_block_size() {
    // The events land on the same frames whatever the block size, and with
    // quantisation off each acts on its frame.
    let events = [(0usize, RECORD), (300, OVERDUB), (600, PLAY)];
    let run = |block: usize| {
        let mut rig = Rig::new(&choices(IO_ALWAYS, Q_OFF), true, block);
        let mut out: Vec<f32> = Vec::new();
        let frames = 1200;
        let mut done = 0;
        while done < frames {
            let length = block.min(frames - done);
            let here: Vec<NodeEvent> = events
                .iter()
                .filter(|(at, _)| *at >= done && *at < done + length)
                .map(|(at, state)| common::set((at - done) as u32, TRANSPORT, *state))
                .collect();
            let input: Vec<Vec<f32>> = (0..common::CHANNELS).map(|_| vec![0.5; length]).collect();
            let transport = TransportInfo {
                playing: true,
                position: Frames(done as i64),
            };
            let part = common::run_at(&mut *rig.node, &input, &here, &rig.tempo, transport);
            out.extend(part[0].clone());
            done += length;
        }
        out
    };
    let reference = run(64);
    for block in [1, 64, 512, 4096] {
        assert_eq!(run(block), reference, "at a block size of {block}");
    }
}

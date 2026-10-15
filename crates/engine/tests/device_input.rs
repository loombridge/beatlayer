//! WP-2.13's acceptance in the engine: a track whose Audio From is Ext. In
//! hears the audio device's input (the routing spec).
//!
//! - A signal on input channel 3 reaches only the track set to Ext. In 3.
//! - A stereo pair stays in phase: what the pair carries arrives on the
//!   track's two sides unchanged.
//! - The input is heard only while the track monitors it.
//! - The input path allocates nothing: every block runs as a real-time
//!   section under the allocation trap.
//!
//! Routing commands run by ID with JSON arguments, as an agent runs them
//! (D17).

bl_rt::install_trap!();

mod common;

use bl_commands::Registry as Commands;
use bl_engine::DeviceInput;
use bl_model::{TrackId, TrackKind, registry};
use common::{RATE, Rig, track};
use serde_json::{Value, json};

/// Runs a document command by ID, as an agent does.
fn run(editor: &mut bl_model::Editor, id: &str, args: Value) {
    let mut commands = Commands::new();
    registry::register(&mut commands).unwrap();
    let spec = commands.get(id).unwrap();
    bl_commands::validate(spec.args.as_ref().unwrap(), &args).unwrap();
    registry::run(editor, id, args).unwrap_or_else(|e| panic!("{id}: {e}"));
}

/// Sets a track's Audio From to Ext. In and its channel.
fn ext_in(editor: &mut bl_model::Editor, track: TrackId, channel: Value) {
    run(
        editor,
        "routing.set_input",
        json!({ "track": track, "input": { "type": "external", "channel": channel } }),
    );
}

fn monitor(editor: &mut bl_model::Editor, track: TrackId, monitor: &str) {
    run(
        editor,
        "routing.set_monitor",
        json!({ "track": track, "monitor": monitor }),
    );
}

/// One block of the audio device's input: planar channels of constants. A
/// channel past the ones given is silence.
struct Input {
    channels: Vec<Vec<f32>>,
}

impl Input {
    /// A block of `frames` frames: channel `n` carries `levels[n]`.
    fn new(frames: usize, levels: &[f32]) -> Self {
        Self {
            channels: levels.iter().map(|&level| vec![level; frames]).collect(),
        }
    }

    /// A block where channels `left` and `right` carry the same sine, so a
    /// pair taken as one stays in phase, and every other channel is silent.
    fn sine_pair(frames: usize, pair: (usize, usize), hz: f64) -> Self {
        let sine = |i: usize| (std::f64::consts::TAU * hz * i as f64 / RATE.as_f64()).sin() as f32;
        let wave: Vec<f32> = (0..frames).map(sine).collect();
        let mut channels = vec![vec![0.0; frames]; pair.1 + 1];
        channels[pair.0] = wave.clone();
        channels[pair.1] = wave;
        Self { channels }
    }
}

impl DeviceInput for Input {
    fn channels(&self) -> usize {
        self.channels.len()
    }

    fn channel(&self, index: usize) -> &[f32] {
        &self.channels[index]
    }
}

/// Frames to render after a change, so a 20 ms glide has finished before
/// the last block of 256 frames.
const SETTLE: usize = 1280;

/// Renders `SETTLE` frames of 256 and returns the left channel of the last
/// block, which must be steady.
fn settled(rig: &mut Rig, input: &Input) -> f32 {
    rig.render_with_input(SETTLE, 256, input);
    let last = rig.last();
    assert!(last.iter().all(|&s| s == last[0]), "a steady level");
    last[0]
}

// Spec: routing R2, R11 (WP-2.13)
#[test]
fn an_input_channel_reaches_only_the_track_set_to_ext_in_it() {
    let mut rig = Rig::with_input(256, 4);
    let ed = &mut rig.editor;
    let a = track(ed, TrackKind::Audio, None);
    let b = track(ed, TrackKind::Audio, None);
    ext_in(ed, a, json!({ "type": "mono", "channel": 3 }));
    ext_in(ed, b, json!({ "type": "mono", "channel": 1 }));
    monitor(ed, a, "in");
    monitor(ed, b, "in");
    assert!(rig.sync().program, "a routing change is a new program");

    // Channel 1 carries 0.5, channel 3 carries 0.25, the rest nothing.
    let input = Input::new(256, &[0.5, 0.0, 0.25, 0.0]);
    assert_eq!(settled(&mut rig, &input), 0.75, "both tracks to Main");

    // B muted: only A's channel 3, 0.25, not channel 1's 0.5.
    run(
        &mut rig.editor,
        "mixer.set_mute",
        json!({ "track": b, "mute": true }),
    );
    rig.sync();
    assert_eq!(settled(&mut rig, &input), 0.25);
    // A muted and B heard: channel 1's 0.5.
    run(
        &mut rig.editor,
        "mixer.set_mute",
        json!({ "track": a, "mute": true }),
    );
    run(
        &mut rig.editor,
        "mixer.set_mute",
        json!({ "track": b, "mute": false }),
    );
    rig.sync();
    assert_eq!(settled(&mut rig, &input), 0.5);

    // A channel the device doesn't have is silence.
    run(
        &mut rig.editor,
        "mixer.set_mute",
        json!({ "track": a, "mute": false }),
    );
    ext_in(&mut rig.editor, a, json!({ "type": "mono", "channel": 7 }));
    run(
        &mut rig.editor,
        "mixer.set_mute",
        json!({ "track": b, "mute": true }),
    );
    assert!(rig.sync().program);
    assert_eq!(settled(&mut rig, &input), 0.0);
}

// Spec: routing R2, R11 (WP-2.13)
#[test]
fn a_stereo_pair_arrives_in_phase_and_mono_fills_both_sides() {
    let mut rig = Rig::with_input(256, 4);
    let ed = &mut rig.editor;
    let pair = track(ed, TrackKind::Audio, None);
    ext_in(ed, pair, json!({ "type": "pair", "channel": 3 }));
    monitor(ed, pair, "in");
    rig.sync();

    let input = Input::sine_pair(256, (2, 3), 300.0);
    rig.render_with_input(2048, 256, &input);
    let output = rig.processor.output();
    let (left, right) = (output.channel(0), output.channel(1));
    assert!(left.iter().any(|&s| s.abs() > 0.1), "the pair plays at all");
    assert_eq!(left, right, "the pair stays in phase");

    // A single channel feeds both sides.
    ext_in(
        &mut rig.editor,
        pair,
        json!({ "type": "mono", "channel": 3 }),
    );
    assert!(rig.sync().program);
    rig.render_with_input(2048, 256, &input);
    let output = rig.processor.output();
    let (left, right) = (output.channel(0), output.channel(1));
    assert_eq!(left, right, "a mono input is centred");
    assert!(left.iter().any(|&s| s.abs() > 0.1));
}

// Spec: routing R11 (WP-2.13)
#[test]
fn the_input_is_heard_only_while_the_track_monitors_it() {
    let mut rig = Rig::with_input(256, 4);
    let ed = &mut rig.editor;
    let a = track(ed, TrackKind::Audio, None);
    ext_in(ed, a, json!({ "type": "mono", "channel": 3 }));
    rig.sync();
    let input = Input::new(256, &[0.0, 0.0, 0.25, 0.0]);

    // Auto, not armed: silent.
    assert_eq!(settled(&mut rig, &input), 0.0);
    // Armed: heard, without a new program.
    run(
        &mut rig.editor,
        "mixer.set_arm",
        json!({ "track": a, "arm": true }),
    );
    assert!(!rig.sync().program, "arm opens the route, no program");
    assert_eq!(settled(&mut rig, &input), 0.25);
    // Monitor In: heard, armed or not.
    monitor(&mut rig.editor, a, "in");
    run(
        &mut rig.editor,
        "mixer.set_arm",
        json!({ "track": a, "arm": false }),
    );
    assert!(!rig.sync().program);
    assert_eq!(settled(&mut rig, &input), 0.25);
    // Off: never.
    monitor(&mut rig.editor, a, "off");
    rig.sync();
    assert_eq!(settled(&mut rig, &input), 0.0);
}

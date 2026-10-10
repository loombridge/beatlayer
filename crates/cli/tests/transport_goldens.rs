//! WP-1.5's acceptance: in the transport's golden fixtures, every sample is
//! where the transport puts it. A model of the transport, written here
//! from `docs/specs/transport.md` and run one frame at a time apart from
//! the engine's code, says which song position each frame of the render
//! plays, or that it's stopped; each frame of the render must then be the
//! beat tone's sample at that position, to the bit, or silence. So a start
//! lands on its exact sample, a stop and a continue on theirs, and a loop
//! wraps on the frame its end falls on through the tempo map.
//!
//! The null tests against the stored references are in `golden.rs`.

bl_rt::install_trap!();

use bl_cli::golden::{SAMPLE_RATE, fixture};
use bl_cli::render::Cue;
use bl_core::time::{Frames, TempoMap, Ticks};
use bl_engine::TransportCommand;
use bl_engine::nodes::{BEAT_TONE, BeatTone};
use bl_model::{DeviceKind, Document};

/// The transport after the spec's rules, one frame at a time.
struct Model {
    tempo: TempoMap,
    playing: bool,
    position: i64,
    insert_marker: Ticks,
    /// The loop's start and end frames, if it's on.
    looping: Option<(i64, i64)>,
}

impl Model {
    fn new(document: &Document) -> Self {
        let settings = document.settings();
        let tempo = settings.tempo.clone();
        let brace = settings.arrangement_loop;
        let frames = |ticks| tempo.ticks_to_frames(ticks, SAMPLE_RATE).get();
        let looping = brace
            .enabled
            .then(|| (frames(brace.start), frames(brace.end())));
        Self {
            playing: false,
            position: 0,
            insert_marker: Ticks::ZERO,
            looping,
            tempo,
        }
    }

    fn frames(&self, ticks: Ticks) -> i64 {
        self.tempo.ticks_to_frames(ticks, SAMPLE_RATE).get()
    }

    fn apply(&mut self, command: TransportCommand) {
        match command {
            TransportCommand::Play => {
                self.position = self.frames(self.insert_marker);
                self.playing = true;
            }
            TransportCommand::Stop if self.playing => self.playing = false,
            TransportCommand::Stop => {
                self.insert_marker = Ticks::ZERO;
                self.position = 0;
            }
            TransportCommand::Continue => self.playing = true,
            TransportCommand::Locate(ticks) => {
                self.insert_marker = ticks;
                if self.playing {
                    self.position = self.frames(ticks);
                }
            }
            other => panic!("the model has no {other:?}"),
        }
    }

    /// The song position this frame plays, if it plays one, and moves on.
    fn step(&mut self) -> Option<i64> {
        if !self.playing {
            return None;
        }
        let played = self.position;
        self.position += 1;
        if let Some((start, end)) = self.looping
            && self.position == end
        {
            self.position = start;
        }
        Some(played)
    }
}

/// The fixture's beat tone, made as the engine makes it.
fn beat_tone(document: &Document) -> BeatTone {
    let device = document
        .track_order()
        .into_iter()
        .filter_map(|id| document.track(id))
        .flat_map(|track| track.devices())
        .find(|device| {
            matches!(&device.kind, DeviceKind::Native { device_type } if device_type == BEAT_TONE)
        })
        .expect("the fixture has a beat tone");
    BeatTone::new(device)
}

/// Checks every frame of a fixture against the model, and returns how many
/// frames played and how many times the loop wrapped.
fn check(name: &str) -> (usize, usize) {
    let fixture = fixture(name).expect("the fixture exists");
    let scene = fixture.scene();
    let render = fixture.render();
    assert!(render.problems().is_empty(), "{:?}", render.problems());
    let tone = beat_tone(scene.document());
    let mut model = Model::new(scene.document());
    let mut cues: Vec<Cue> = scene.cues.clone();
    cues.reverse();
    let (mut played, mut wraps, mut last) = (0, 0, None);
    for frame in 0..render.frames() {
        while cues.last().is_some_and(|cue| cue.at == frame as u64) {
            model.apply(cues.pop().unwrap().command);
        }
        let expected = match model.step() {
            Some(position) => {
                played += 1;
                if last.is_some_and(|last| position < last) {
                    wraps += 1;
                }
                last = Some(position);
                tone.sample_at(&model.tempo, Frames(position), SAMPLE_RATE)
            }
            None => 0.0,
        };
        for channel in 0..render.channels {
            let sample = render.samples[frame * render.channels + channel];
            assert_eq!(
                sample.to_bits(),
                expected.to_bits(),
                "{name}: frame {frame}, channel {channel}: {sample} where the model has {expected}"
            );
        }
    }
    assert!(cues.is_empty(), "{name}: a cue past the render");
    (played, wraps)
}

#[test]
fn a_start_mid_bar_and_every_move_after_it_land_on_their_samples() {
    // Spec: transport R1 to R6
    let (played, wraps) = check("start-mid-bar");
    // 30,001 to 90,017, 100,003 to 140,009, and 150,011 to the end.
    assert_eq!(played, 60_016 + 40_006 + 29_989);
    assert_eq!(wraps, 1, "the second Stop went back to 1.1.1");
}

#[test]
fn a_loop_wraps_on_its_samples_again_and_again() {
    // Spec: transport R13, R14
    let (played, wraps) = check("loop-wraps");
    assert_eq!(played, 192_000);
    assert_eq!(wraps, 4);
}

#[test]
fn a_loop_inside_a_tempo_ramp_wraps_where_the_map_puts_its_end() {
    // Spec: transport R13, R15
    let (played, wraps) = check("tempo-ramp-loop");
    assert_eq!(played, 216_000);
    assert!(wraps >= 3, "{wraps} wraps");
}

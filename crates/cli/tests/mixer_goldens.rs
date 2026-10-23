//! WP-1.6's acceptance in the mixer's golden fixtures: a fader move glides
//! with no zipper noise, and solo plays exactly what it should.
//!
//! The null tests against the stored references are in `golden.rs`.

use bl_cli::golden::{BLOCK, Render, SAMPLE_RATE, fixture};
use bl_cli::render::{self, Settings};
use bl_engine::mixer::SMOOTHING_SECONDS;
use bl_engine::{Action, ParamValue};
use bl_model::{Command, Document, Editor, TrackId};

/// Renders `document` from its start with nothing scheduled, as long as
/// `frames`, interleaved.
fn render_plain(document: &Document, frames: u64) -> Vec<f32> {
    let settings = Settings {
        sample_rate: SAMPLE_RATE,
        block: BLOCK,
        frames,
    };
    let mut samples = Vec::new();
    render::render(document, &[], &settings, |output| {
        render::interleave(output, &mut samples);
    });
    samples
}

/// Spec: mixer R18. The fader-move fixture moves a 440 Hz tone's volume
/// in 1 dB steps every 1/32 beat, mutes and unmutes it, and sweeps its pan
/// in steps. Dividing the render by the same tone with nothing moving gives
/// the gain on every frame (where the tone isn't near a zero crossing), and
/// that gain never changes by more than a full-scale glide's step, 1/960 of
/// the way from 0 to 1 per frame. A fader that stepped instead would jump by
/// a whole step (0.11 for 1 dB at 0 dB, and 1 for the mute) in one frame.
#[test]
fn a_fader_move_glides_from_value_to_value_with_no_zipper_noise() {
    let fixture = fixture("fader-move").expect("the fixture exists");
    let scene = fixture.scene();
    let moved: Render = fixture.render();
    assert!(moved.problems().is_empty(), "{:?}", moved.problems());
    let dry = render_plain(scene.document(), scene.frames);
    assert_eq!(dry.len(), moved.samples.len());

    // The biggest change one command asks for: the mute, from 1 to 0.
    let biggest = scene
        .schedule
        .iter()
        .map(|command| match command.action {
            Action::Param {
                value: ParamValue::Set(value),
                ..
            } => value,
            _ => 0.0,
        })
        .fold(0.0f64, f64::max);
    assert!(biggest >= 1.0);
    let glide = (SMOOTHING_SECONDS * SAMPLE_RATE.as_f64()).round();
    assert_eq!(glide, 960.0);
    // A gain that glides moves at most this far in a frame. (The pan law's
    // gains reach √2, but a pan step is far smaller than the mute.)
    let limit = biggest / glide;

    let peak = dry.iter().fold(0.0f32, |p, s| p.max(s.abs()));
    let mut steepest = 0.0f64;
    let mut moving_frames = 0;
    for channel in 0..2 {
        let mut last: Option<(usize, f64)> = None;
        for frame in 0..scene.frames as usize {
            let i = frame * 2 + channel;
            let x = f64::from(dry[i]);
            // Near a zero crossing, the ratio says little.
            if x.abs() < f64::from(peak) * 0.2 {
                continue;
            }
            let gain = f64::from(moved.samples[i]) / x;
            if let Some((then, before)) = last {
                let slope = (gain - before).abs() / (frame - then) as f64;
                steepest = steepest.max(slope);
                if slope > 1e-6 {
                    moving_frames += 1;
                }
            }
            last = Some((frame, gain));
        }
    }
    println!(
        "fader-move: the gain moves on {moving_frames} frames, at most {steepest:.6} a frame; \
         a glide's limit is {limit:.6}, and a step would be at least 0.109"
    );
    assert!(moving_frames > 50_000, "the fixture moves the fader");
    // Within the ratio's rounding: f32 samples carry about 7 digits.
    assert!(steepest <= limit + 1e-5, "{steepest} > {limit}");
}

/// Spec: mixer-view R18. The fader-gesture fixture plays the volumes a fader
/// dragged in the Session mixer set, one 60 Hz frame apart (`fader_gesture`
/// in bl-ui's mixer test, which checks the window still makes exactly
/// these). Dividing the render by the same tone at rest gives the gain on
/// every frame, and it never moves faster than a 20 ms glide between two
/// neighbouring volumes allows: a frame of the drag starts a new glide from
/// wherever the last one had reached, at most a fifth of a step behind, so no
/// frame's change is more than 1.2 steps over 960 frames. A fader that
/// stepped would jump a whole step in one frame.
#[test]
fn a_fader_dragged_in_the_mixer_glides_with_no_zipper_noise() {
    let frames = bl_cli::golden::fader_gesture_frames();
    let biggest_step = frames
        .windows(2)
        .map(|pair| (pair[1].1 - pair[0].1).abs())
        .fold(0.0f64, f64::max);
    assert!(
        biggest_step > 0.1,
        "the drag moves the fader: {biggest_step}"
    );
    let fixture = fixture("fader-gesture").expect("the fixture exists");
    let scene = fixture.scene();
    assert_eq!(
        scene.schedule.len(),
        frames.len(),
        "every frame of the drag"
    );
    let moved: Render = fixture.render();
    assert!(moved.problems().is_empty(), "{:?}", moved.problems());
    let dry = render_plain(scene.document(), scene.frames);
    let glide = (SMOOTHING_SECONDS * SAMPLE_RATE.as_f64()).round();
    let limit = 1.2 * biggest_step / glide;

    let peak = dry.iter().fold(0.0f32, |p, s| p.max(s.abs()));
    let mut steepest = 0.0f64;
    let mut moving_frames = 0;
    for channel in 0..2 {
        let mut last: Option<(usize, f64)> = None;
        for frame in 0..scene.frames as usize {
            let i = frame * 2 + channel;
            let x = f64::from(dry[i]);
            if x.abs() < f64::from(peak) * 0.2 {
                continue;
            }
            let gain = f64::from(moved.samples[i]) / x;
            if let Some((then, before)) = last {
                let slope = (gain - before).abs() / (frame - then) as f64;
                steepest = steepest.max(slope);
                if slope > 1e-6 {
                    moving_frames += 1;
                }
            }
            last = Some((frame, gain));
        }
    }
    println!(
        "fader-gesture: the gain moves on {moving_frames} frames, at most {steepest:.6} a frame; \
         the glide's limit is {limit:.6}, and a step would be {biggest_step:.3}"
    );
    assert!(moving_frames > 20_000, "the render follows the drag");
    assert!(steepest <= limit + 1e-5, "{steepest} > {limit}");
}

/// Spec: mixer R10, R11. Soloing the kick in place plays exactly what the
/// same song plays with the hat and the bass deleted: the kick through its
/// group, and its send through the return, to the bit.
#[test]
fn soloing_in_place_is_the_song_without_the_tracks_solo_silences() {
    let fixture = fixture("solo-in-place").expect("the fixture exists");
    let scene = fixture.scene();
    let soloed = fixture.render();
    assert!(soloed.problems().is_empty(), "{:?}", soloed.problems());

    let frames = scene.frames;
    let mut editor: Editor = scene.editor;
    let named = |editor: &Editor, name: &str| -> TrackId {
        editor
            .document()
            .tracks()
            .find(|track| track.name() == name)
            .map(bl_model::Track::id)
            .expect("the fixture's tracks are named")
    };
    for name in ["Hat", "Bass"] {
        let track = named(&editor, name);
        editor.apply(Command::DeleteTrack { track }).unwrap();
    }
    let kick = named(&editor, "Kick");
    editor
        .apply(Command::SetSolo {
            track: kick,
            solo: false,
        })
        .unwrap();
    let without = render_plain(editor.document(), frames);
    assert!(soloed.samples.iter().any(|&s| s != 0.0));
    assert_eq!(
        soloed
            .samples
            .iter()
            .map(|s| s.to_bits())
            .collect::<Vec<_>>(),
        without.iter().map(|s| s.to_bits()).collect::<Vec<_>>()
    );
}

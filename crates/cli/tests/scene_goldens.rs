//! WP-3.5's acceptance: in the scene-tempo golden fixture, every sample is
//! where a scene's tempo puts it. A model of the launch, written here from
//! the session-view spec and run one frame at a time apart from the engine's
//! code, says which song position each frame plays under which tempo: the
//! set's 120 BPM up to the launch frame, then the scene's 60 BPM with the
//! playhead on the same beat. Each frame of the render must then be the beat
//! tone's sample at that position, to the bit.
//!
//! The null test against the stored reference is in `golden.rs`.

bl_rt::install_trap!();

use bl_cli::golden::{SAMPLE_RATE, fixture};
use bl_core::time::{Bpm, Frames, TempoMap, Ticks};
use bl_engine::nodes::{BEAT_TONE, BeatTone};
use bl_model::{DeviceKind, Document};

/// The launch frame: 2.1.1 at 120 BPM, the first bar line after the frame
/// the fixture asks on.
const LAUNCH: usize = 96_000;

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

fn check(block: usize) {
    // Spec: session-view R22
    let fixture = fixture("scene-tempo").expect("the fixture exists");
    let scene = fixture.scene();
    let render = fixture.render_in_blocks(block);
    assert!(render.problems().is_empty(), "{:?}", render.problems());
    let tone = beat_tone(scene.document());
    let before = TempoMap::constant(Bpm::new(120.0).unwrap());
    let after = TempoMap::constant(Bpm::new(60.0).unwrap());
    // The playhead keeps its beat: 2.1.1 is four quarters in, which is 4
    // seconds at 60 BPM.
    let beat = Ticks::QUARTER * 4;
    let resumed = after.ticks_to_frames(beat, SAMPLE_RATE).get();
    assert_eq!(
        before.ticks_to_frames(beat, SAMPLE_RATE).get(),
        LAUNCH as i64
    );
    for frame in 0..render.frames() {
        let expected = if frame < LAUNCH {
            tone.sample_at(&before, Frames(frame as i64), SAMPLE_RATE)
        } else {
            tone.sample_at(
                &after,
                Frames(resumed + (frame - LAUNCH) as i64),
                SAMPLE_RATE,
            )
        };
        for channel in 0..render.channels {
            let sample = render.samples[frame * render.channels + channel];
            assert_eq!(
                sample.to_bits(),
                expected.to_bits(),
                "blocks of {block}: frame {frame}, channel {channel}: {sample} where the model has {expected}"
            );
        }
    }
}

#[test]
fn a_scenes_tempo_applies_on_its_launch_frame_in_the_render() {
    for block in [64, 512, 4_096] {
        check(block);
    }
}

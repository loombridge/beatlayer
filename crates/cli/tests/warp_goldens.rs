//! WP-4.2's acceptance in the warped golden renders: a loop warped at 90 BPM
//! plays every one of its bars on the song's bar lines at 120 and at 140 BPM,
//! and every sample is the file read at the position the warp map and the
//! tempo map put at its frame. A model written here from the warping spec,
//! apart from the engine's code, works out each frame's position (the frame's
//! tick, the contents beat there, the file time at that beat) and reads the
//! file there through a cubic; the render matches it to a fraction of a
//! step. The renders also null-test against their stored references in
//! `golden.rs`.

bl_rt::install_trap!();

use bl_cli::golden::{Render, SAMPLE_RATE, Scene, fixture};
use bl_model::ClipData;

/// The loop file's length in frames: two beats at 90 BPM.
const FILE_FRAMES: f64 = 64_000.0;

/// The file's sample at `position`, through a Catmull-Rom cubic, silence
/// outside the file.
fn read(file: &[f32], position: f64) -> f64 {
    let at = |i: i64| {
        usize::try_from(i)
            .ok()
            .and_then(|i| file.get(i))
            .map_or(0.0, |&s| f64::from(s))
    };
    let floor = position.floor();
    let t = position - floor;
    let i = floor as i64;
    let (a, b, c, d) = (at(i - 1), at(i), at(i + 1), at(i + 2));
    let (m1, m2) = (0.5 * (c - a), 0.5 * (d - b));
    (2.0 * t.powi(3) - 3.0 * t * t + 1.0) * b
        + (t.powi(3) - 2.0 * t * t + t) * m1
        + (-2.0 * t.powi(3) + 3.0 * t * t) * c
        + (t.powi(3) - t * t) * m2
}

/// What the warping spec says each frame plays, interleaved left and right,
/// and the file position the clip reads on each frame (left of the loop's
/// start for none).
fn model(scene: &Scene) -> (Vec<f32>, Vec<f64>) {
    let document = scene.document();
    let tempo = &document.settings().tempo;
    let rate = SAMPLE_RATE.as_f64();
    let mut out = vec![0.0f32; scene.frames as usize * 2];
    let mut positions = vec![f64::NAN; scene.frames as usize];
    for track in document.tracks() {
        for clip in track.clips() {
            let ClipData::Audio(audio) = &clip.data else {
                continue;
            };
            let map = audio.warp_map().expect("the clip is warped");
            let file = scene.media.get(audio.media).expect("the file is there");
            let first = tempo.ticks_to_frames(clip.start, SAMPLE_RATE).get();
            let end = tempo.ticks_to_frames(clip.end(), SAMPLE_RATE).get();
            for frame in first..end.min(scene.frames as i64) {
                // R12: the frame's tick, the contents beat there, wrapped in
                // the loop (R5), and the file time at that beat.
                let tick = tempo.seconds_to_ticks_exact(frame as f64 / rate);
                let mut beat = audio.start + (tick - clip.start.get() as f64);
                let loop_end = audio.loop_end.expect("the loop has an end");
                if audio.looping && beat >= loop_end {
                    beat = audio.loop_start
                        + (beat - loop_end).rem_euclid(loop_end - audio.loop_start);
                }
                let position = map.beat_to_time(beat) * rate;
                positions[frame as usize] = position;
                for c in 0..2 {
                    let channel = file.channel(c.min(file.channel_count() - 1)).unwrap();
                    out[frame as usize * 2 + c] += (read(channel, position) * audio.gain) as f32;
                }
            }
        }
    }
    (out, positions)
}

fn render(name: &str) -> (Scene, Render) {
    let fixture = fixture(name).expect("the fixture exists");
    let scene = fixture.scene();
    let render = fixture.render();
    assert!(render.problems().is_empty(), "{:?}", render.problems());
    (scene, render)
}

// Spec: warping R13
#[test]
fn a_loop_warped_at_90_plays_on_the_bar_lines_at_120_and_140() {
    for (name, song_bpm) in [("warped-loop-120", 120.0), ("warped-loop-140", 140.0)] {
        let (scene, render) = render(name);
        let (expected, positions) = model(&scene);
        assert_eq!(render.samples.len(), expected.len());
        // The render is the file read where the maps say: a Catmull-Rom
        // model agrees within a thousandth of the file's level (the sinc
        // that HiQ chooses differs from a cubic by less). Within the
        // sinc's reach of the file's ends it reads the silence beyond them,
        // which the model's four taps reach less far into, so those frames
        // are left out.
        let mut worst = 0.0f32;
        for (frame, &position) in positions.iter().enumerate() {
            if !(40.0..FILE_FRAMES - 40.0).contains(&position) {
                continue;
            }
            for c in 0..2 {
                let at = frame * 2 + c;
                worst = worst.max((render.samples[at] - expected[at]).abs());
            }
        }
        assert!(worst < 5e-4, "{name}: {worst} from the model");

        // Every bar starts the loop again on its bar line: the position
        // falls back to the file's start on the frame the bar begins.
        let bar = 2.0 * 60.0 / song_bpm * SAMPLE_RATE.as_f64();
        let speed = song_bpm / 90.0;
        for n in 0..3 {
            let line = (bar * f64::from(n)).ceil() as usize;
            assert!(
                positions[line] < speed + 1e-6,
                "{name}: bar {} reads the file at {} on its bar line",
                n + 1,
                positions[line]
            );
            if n > 0 {
                assert!(
                    positions[line - 1] > FILE_FRAMES - 10.0 - speed,
                    "{name}: bar {} ended at {}",
                    n,
                    positions[line - 1]
                );
            }
        }
    }
}

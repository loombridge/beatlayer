//! WP-4.2's acceptance in the warped golden renders: a loop warped at 90 BPM
//! plays every one of its bars on the song's bar lines at 120 and at 140 BPM,
//! and every sample is the file read at the position the warp map and the
//! tempo map put at its frame. A model written here from the warping spec,
//! apart from the engine's code, works out each frame's position (the frame's
//! tick, the contents beat there, the file time at that beat) and reads the
//! file there through a cubic; the render matches it to a fraction of a
//! step. The renders also null-test against their stored references in
//! `golden.rs`.

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
    // The tempo map the set plays, a leading clip's tempo included (R44,
    // R48).
    let tempo = &document.tempo_map();
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
                if audio.looping
                    && let Some(loop_end) = audio.loop_end
                    && beat >= loop_end
                {
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

// Spec: warping R43, R44, R47
#[test]
fn a_leader_plays_at_its_own_speed() {
    let (scene, render) = render("leader-tempo");
    let (expected, positions) = model(&scene);
    assert_eq!(render.samples.len(), expected.len());
    let mut worst = 0.0f32;
    for (frame, &position) in positions.iter().enumerate() {
        if !(40.0..128_000.0 - 40.0).contains(&position) {
            continue;
        }
        for c in 0..2 {
            let at = frame * 2 + c;
            worst = worst.max((render.samples[at] - expected[at]).abs());
        }
    }
    assert!(worst < 5e-4, "{worst} from the model");
    // The leader plays as if unwarped: over its span, which at its own 90 BPM
    // lasts eight thirds of a second, the file advances one frame per song
    // frame.
    for frame in [0usize, 1000, 60_000, 127_000] {
        assert!(
            (positions[frame] - frame as f64).abs() < 1.0,
            "frame {frame} reads the leader at {}",
            positions[frame]
        );
    }
    // Its span ends where the set is at its own 120 again, and the follower
    // reads its own 180 BPM file at two thirds of the song's speed.
    assert!(positions[128_000].abs() < 1.0, "{}", positions[128_000]);
    for frame in [128_001usize, 128_500, 150_000] {
        let read = positions[frame];
        let expected = (frame - 128_000) as f64 * 2.0 / 3.0;
        assert!(
            (read - expected).abs() < 1.0,
            "frame {frame} reads the follower at {read}, not {expected}"
        );
    }
}

// Spec: warping R48
#[test]
fn a_set_with_a_leader_renders_as_it_plays_at_any_block_size() {
    let fixture = fixture("leader-tempo").expect("the fixture exists");
    let reference = fixture.render();
    assert!(
        reference.problems().is_empty(),
        "{:?}",
        reference.problems()
    );
    for block in [1, 61, 512, 4_096] {
        let render = fixture.render_in_blocks(block);
        assert!(
            reference.same_bits(&render),
            "a block of {block} frames differs from the default block"
        );
    }
}

// Spec: browser R20.
#[test]
fn a_preview_against_a_running_song_lands_on_its_bar_lines() {
    let collector = bl_engine::Collector::new();
    let shared = std::sync::Arc::new(bl_engine::preview::PreviewShared::default());
    let (mut control, mut voice) = bl_engine::preview::preview_channel(collector.handle(), shared);

    let rate = 48_000.0;
    let frames = 115_200;
    let channel = vec![0.5f32; frames];
    let buffer = bl_engine::preview::PreviewBuffer::new(vec![channel]).expect("buffer");
    let map = bl_engine::preview::WarpMap::new(
        vec![
            bl_engine::preview::Marker { time: 0.0, beat: 0 },
            bl_engine::preview::Marker {
                time: 2.4,
                beat: bl_core::time::Ticks::WHOLE.get(),
            },
        ],
        100.0,
    )
    .expect("valid warp map");

    let warped = bl_engine::preview::WarpedPreview {
        samples: Box::new(buffer),
        map,
        start: 0.0,
        end: bl_core::time::Ticks::WHOLE.get() as f64,
        loop_start: 0.0,
        loop_end: bl_core::time::Ticks::WHOLE.get() as f64,
        sample_rate: rate,
        tempo: 120.0,
    };
    control.play_warped(Box::new(warped)).expect("queued");

    let tempo_map = bl_core::time::TempoMap::constant(bl_core::time::Bpm::new(120.0).unwrap());
    let signatures = bl_core::time::TimeSignatureMap::default();
    let launch = bl_model::LaunchSettings::default();

    let mut left = vec![0.0f32; 1000];
    let mut right = vec![0.0f32; 1000];

    // Block 1: frame 48,000 (middle of first bar, before 96,000 next bar line).
    let transport = bl_engine::preview::PreviewTransport {
        playing: true,
        song_position: bl_core::time::Frames(48_000),
        tempo_map: &tempo_map,
        time_signatures: &signatures,
        loop_frames: None,
        sample_rate: bl_core::time::SampleRate::new(48_000).unwrap(),
        launch,
    };
    voice.render_with_transport(
        1000,
        [&mut left[..], &mut right[..]].into_iter(),
        Some(transport),
    );
    assert!(
        left.iter().all(|&s| s == 0.0),
        "preview waits for next bar line"
    );

    // Block 2: frame 96,000 (next bar line).
    let transport2 = bl_engine::preview::PreviewTransport {
        playing: true,
        song_position: bl_core::time::Frames(96_000),
        tempo_map: &tempo_map,
        time_signatures: &signatures,
        loop_frames: None,
        sample_rate: bl_core::time::SampleRate::new(48_000).unwrap(),
        launch,
    };
    voice.render_with_transport(
        1000,
        [&mut left[..], &mut right[..]].into_iter(),
        Some(transport2),
    );
    assert!(
        left.iter().any(|&s| s > 0.0),
        "preview starts playing on bar line"
    );
}

/// Plays `name`'s song on the engine's real-time path, following `bpm`
/// (the transport spec, R24), and returns its interleaved samples. An
/// offline render has no follower.
fn render_following(name: &str, bpm: f64, frames: usize) -> Vec<f32> {
    let scene = fixture(name).expect("the fixture exists").scene();
    let config = bl_engine::EngineConfig::new(SAMPLE_RATE, 512);
    let mut registry = bl_engine::Registry::new();
    bl_devices::register(&mut registry);
    let registry = registry.with_media(std::sync::Arc::new(scene.media.clone()));
    let collector = bl_engine::Collector::new();
    let mut compiler = bl_engine::Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, _monitor) = bl_engine::channel(&config);
    sender.send_update(compiler.sync(scene.document()));
    let follower: Box<dyn bl_engine::Follower> = Box::new(bl_engine::testing::following(bpm));
    sender.follower(Some(collector.handle().wrap(follower)));
    for cue in &scene.cues {
        sender.transport_at(cue.command, bl_engine::When::Frame(cue.at));
    }
    assert!(sender.flush());
    let mut out = Vec::new();
    let mut done = 0;
    while done < frames {
        let n = 512.min(frames - done);
        processor.process(n);
        let output = processor.output();
        for i in 0..n {
            out.push(output.channel(0)[i]);
            out.push(output.channel(1)[i]);
        }
        done += n;
    }
    out
}

// Spec: transport R24
#[test]
fn a_warped_set_following_140_nulls_against_its_render_at_140() {
    let (_, at_140) = render("warped-loop-140");
    let following = render_following("warped-loop-120", 140.0, at_140.samples.len() / 2);
    assert_eq!(following.len(), at_140.samples.len());
    // Where the loop starts again, the set following 140 and the set at 140
    // can start it a frame apart; every other frame reads the file where the
    // maps put it, to within the golden tolerance.
    let (mut worst, mut off) = (0.0f32, 0usize);
    for (got, want) in following.iter().zip(&at_140.samples) {
        let gap = (got - want).abs();
        if gap > 5e-4 {
            off += 1;
        } else {
            worst = worst.max(gap);
        }
    }
    assert!(
        off < 64,
        "{off} samples differ by more than 5e-4 (worst of the rest {worst})"
    );
}

/// Plays `name`'s song on the engine's real-time path, following a MIDI
/// clock at `bpm` whose Start begins the song on the first frame (the MIDI
/// clock spec, R17, R20, R22), and returns its interleaved samples.
fn render_following_a_clock(name: &str, bpm: f64, frames: usize) -> Vec<f32> {
    use bl_core::midi::{MidiInputEvent, MidiMessage, MidiSource, PortId, SyncInput};
    use std::time::{Duration, Instant};
    let scene = fixture(name).expect("the fixture exists").scene();
    let config = bl_engine::EngineConfig::new(SAMPLE_RATE, 512);
    let mut registry = bl_engine::Registry::new();
    bl_devices::register(&mut registry);
    let registry = registry.with_media(std::sync::Arc::new(scene.media.clone()));
    let collector = bl_engine::Collector::new();
    let mut compiler = bl_engine::Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, _monitor) = bl_engine::channel(&config);
    sender.send_update(compiler.sync(scene.document()));
    let (mut queue, input) = rtrb::RingBuffer::new(8192);
    processor.set_midi_input(input);
    let port = PortId::from_name("Sequencer");
    sender.sync_inputs(&[SyncInput { port, delay_ms: 0 }]);
    sender.external_sync(true);
    assert!(sender.flush());
    // Steady clocks from twelve before the first block, and Start between
    // two of them: the next clock, the song's first frame, falls on the
    // first block's start. The song plays from the clock alone.
    let start = bl_core::clock::TestClock::new().now();
    let period = Duration::from_secs_f64(60.0 / (bpm * 24.0));
    let seconds = frames as f64 / SAMPLE_RATE.as_f64();
    let count = (seconds / period.as_secs_f64()) as u32 + 24;
    let mut send = |time: Instant, message| {
        let source = MidiSource::Sync(port);
        let event = MidiInputEvent {
            source,
            time,
            message,
        };
        queue.push(event).unwrap();
    };
    for k in (1..=12).rev() {
        send(start - period * k, MidiMessage::CLOCK);
    }
    send(start - period / 2, MidiMessage::START);
    for k in 0..count {
        send(start + period * k, MidiMessage::CLOCK);
    }
    let mut out = Vec::new();
    let mut done = 0;
    while done < frames {
        let n = 512.min(frames - done);
        let now = start + Duration::from_secs_f64(done as f64 / SAMPLE_RATE.as_f64());
        processor.process_at(n, &bl_engine::NoInput, now);
        let output = processor.output();
        for i in 0..n {
            out.push(output.channel(0)[i]);
            out.push(output.channel(1)[i]);
        }
        done += n;
    }
    out
}

// Spec: midi-clock R17, R20, R22. A warped set following a steady 140 BPM
// clock, started by the clock, nulls against its render at 140 within the
// golden tolerance.
#[test]
fn a_warped_set_following_a_140_clock_nulls_against_its_render_at_140() {
    let (_, at_140) = render("warped-loop-140");
    let following = render_following_a_clock("warped-loop-120", 140.0, at_140.samples.len() / 2);
    assert_eq!(following.len(), at_140.samples.len());
    // The clock's tempo is the fit's, a hair off 140: the loop starts again
    // a frame or two from the render's, and every other frame reads the file
    // where the maps put it.
    let (mut worst, mut off) = (0.0f32, 0usize);
    for (got, want) in following.iter().zip(&at_140.samples) {
        let gap = (got - want).abs();
        if gap > 5e-4 {
            off += 1;
        } else {
            worst = worst.max(gap);
        }
    }
    assert!(
        off < 64,
        "{off} samples differ by more than 5e-4 (worst of the rest {worst})"
    );
}

//! Fades with curves, and crossfades between audio clips that meet (the audio
//! clips spec, R23, R24, R31): what each clip plays across the span, the gains
//! that follow the curve law, the parts of a split clip crossfaded playing the
//! whole, and a render the same at any block size, across a tempo ramp, with
//! warped and unwarped clips. Every block runs under the allocation trap.

bl_rt::install_trap!();

mod common;

use std::sync::Arc;

use bl_core::time::{Bpm, TempoMap, Ticks};
use bl_engine::{EngineConfig, MediaMap, Offline, Registry, TransportCommand};
use bl_media::SampleData;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, Editor, TrackId, TrackKind,
    WarpMarker, WarpMode,
};
use common::{RATE, track};

fn hash(n: u8) -> ContentHash {
    ContentHash([n; 32])
}

/// A mono file of two sines, so that any frame played at the wrong place or
/// the wrong gain shows.
fn tone(frames: usize) -> Arc<SampleData> {
    let samples = (0..frames)
        .map(|i| {
            let t = i as f64 / 48_000.0;
            (0.4 * (std::f64::consts::TAU * 220.0 * t).sin()
                + 0.1 * (std::f64::consts::TAU * 1_500.0 * t).sin()) as f32
        })
        .collect();
    Arc::new(SampleData::new(48_000, vec![samples]).unwrap())
}

/// A mono file of one steady level.
fn steady(frames: usize, level: f32) -> Arc<SampleData> {
    Arc::new(SampleData::new(48_000, vec![vec![level; frames]]).unwrap())
}

fn clip(
    editor: &mut Editor,
    track: TrackId,
    start: Ticks,
    length: Ticks,
    audio: AudioData,
) -> ClipId {
    let id = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(id, start, length, ClipData::Audio(audio)),
        })
        .unwrap();
    id
}

/// Plays `editor`'s song from 1.1.1 for `frames` frames in blocks of `block`
/// and returns the left channel.
fn play(editor: &Editor, media: &MediaMap, frames: u64, block: usize) -> Vec<f32> {
    let registry = Registry::new().with_media(Arc::new(media.clone()));
    let mut offline = Offline::new(EngineConfig::new(RATE, block), registry);
    offline.sync(editor.document());
    assert!(offline.missing_media().is_empty());
    offline.sender().transport(TransportCommand::Play);
    let mut out = Vec::new();
    offline.render(frames, |output| out.extend_from_slice(output.channel(0)));
    assert_eq!(offline.heap_operations(), 0);
    out
}

fn frame_of(tempo: &TempoMap, ticks: Ticks) -> usize {
    usize::try_from(tempo.ticks_to_frames(ticks, RATE).get()).unwrap()
}

fn seconds(frames: usize) -> f64 {
    frames as f64 / RATE.as_f64()
}

// Spec: audio-clips R23, R24. Across a crossfade the earlier clip fades out
// and the later fades in by the curve law, each playing on past its edge, and
// outside the span each plays alone.
#[test]
fn a_crossfade_blends_the_two_clips_across_its_span_by_the_curve_law() {
    for curve in [0.0, 0.5, -0.75] {
        let mut editor = Editor::new();
        let a = track(&mut editor, TrackKind::Audio, None);
        let tempo = editor.document().settings().tempo.clone();
        let (start, length) = (Ticks::QUARTER, Ticks::QUARTER * 2);
        let boundary = start + length;
        // The later clip starts a half second into its file, so it can play
        // on before its start.
        let first = clip(&mut editor, a, start, length, AudioData::new(hash(1)));
        let second = clip(
            &mut editor,
            a,
            boundary,
            length,
            AudioData {
                start: 0.5,
                ..AudioData::new(hash(2))
            },
        );
        let (before, after) = (100, 300);
        editor
            .apply(Command::SetCrossfade {
                track: a,
                boundary,
                before: seconds(before),
                after: seconds(after),
                curve,
            })
            .unwrap();
        for (id, fade_in, fade_out) in
            [(first, 0.0, seconds(before)), (second, seconds(after), 0.0)]
        {
            let audio = match &editor.document().clip(id).unwrap().data {
                ClipData::Audio(audio) => audio.clone(),
                ClipData::Midi(_) => unreachable!(),
            };
            assert_eq!((audio.fade_in, audio.fade_out), (fade_in, fade_out));
        }
        let mut media = MediaMap::new();
        media.insert(hash(1), steady(96_000, 0.5));
        media.insert(hash(2), steady(96_000, 0.25));
        let left = play(&editor, &media, 120_000, 64);

        let (from, to) = (frame_of(&tempo, start), frame_of(&tempo, boundary));
        let end = frame_of(&tempo, boundary + length);
        let power = 4f64.powf(-curve);
        let span = (before + after) as f64;
        for (frame, &sample) in left.iter().enumerate() {
            let into = frame as i64 - (to as i64 - before as i64);
            let x = (into as f64 / span).clamp(0.0, 1.0);
            let expected = if frame < from || frame >= end {
                0.0
            } else {
                // Gains in f64 and one rounding to f32 per clip, as the
                // player adds them.
                let earlier = if frame >= to + after || frame < from {
                    0.0
                } else {
                    0.5 * (1.0 - x).powf(power)
                };
                let later = if frame < to - before {
                    0.0
                } else {
                    0.25 * x.powf(power)
                };
                f64::from((earlier) as f32) + f64::from((later) as f32)
            };
            assert!(
                (f64::from(sample) - expected).abs() < 1e-6,
                "curve {curve}, frame {frame}: {sample} against {expected}"
            );
        }
        // The middle of a straight crossfade of equal gain: half of each.
        if curve == 0.0 {
            let middle = to - before + (before + after) / 2;
            let expected = 0.5 * 0.5 + 0.25 * 0.5;
            assert!((f64::from(left[middle]) - expected).abs() < 1e-6);
        }
    }
}

/// A song of one track holding `audio` from bar 1 for `length` ticks, then, if
/// `at` splits it, the same material in two parts that meet there.
fn song(
    tempo: &TempoMap,
    audio: &AudioData,
    length: Ticks,
    split: Option<(Ticks, f64, f64)>,
) -> Editor {
    let mut editor = Editor::new();
    editor
        .apply(Command::SetTempo {
            tempo: tempo.clone(),
        })
        .unwrap();
    let a = track(&mut editor, TrackKind::Audio, None);
    let start = Ticks::QUARTER + Ticks(3_141);
    let id = clip(&mut editor, a, start, length, audio.clone());
    if let Some((at, before, after)) = split {
        let whole = editor.document().clip(id).unwrap().clone();
        let right = Clip {
            id: editor.new_id(),
            ..whole.part(at, whole.end(), tempo)
        };
        editor
            .apply(Command::SetClipLength {
                clip: id,
                length: at - start,
            })
            .unwrap();
        editor
            .apply(Command::CreateClip {
                track: a,
                clip: right,
            })
            .unwrap();
        editor
            .apply(Command::SetCrossfade {
                track: a,
                boundary: at,
                before,
                after,
                curve: 0.0,
            })
            .unwrap();
    }
    editor
}

fn tempo_ramp() -> TempoMap {
    TempoMap::from_breakpoints([
        (Ticks::ZERO, Bpm::new(97.3).unwrap()),
        (Ticks::WHOLE * 2, Bpm::new(141.1).unwrap()),
    ])
    .unwrap()
}

fn warped_tone() -> AudioData {
    AudioData {
        warp: true,
        warp_markers: vec![WarpMarker {
            time: 0.0,
            beat: Ticks::ZERO,
        }],
        tempo: Some(120.0),
        // Re-Pitch through the cubic: the whole clip and its parts read the
        // file at the same positions, to the table's tolerance.
        warp_mode: WarpMode::RePitch,
        high_quality: false,
        ..AudioData::new(hash(1))
    }
}

// Spec: audio-clips R24, R26, R31. The two parts of a split clip, crossfaded
// with a straight curve, play what the whole clip did to under −120 dBFS where
// both read the file at the same positions (unwarped, under a tempo ramp;
// warped, under a steady tempo), and to the warped table's hundredth of a
// frame across a ramp (the warping spec, R12), at any block size.
#[test]
fn the_crossfaded_parts_of_a_split_clip_null_against_the_whole() {
    let steady_tempo = TempoMap::constant(Bpm::new(120.0).unwrap());
    let unwarped = AudioData {
        start: 0.0371,
        ..AudioData::new(hash(1))
    };
    let mut media = MediaMap::new();
    media.insert(hash(1), tone(400_000));
    let length = Ticks::WHOLE * 2 + Ticks(2_718);
    let cases = [
        ("unwarped, ramp", unwarped, tempo_ramp(), 1e-6),
        ("warped, steady", warped_tone(), steady_tempo, 1e-6),
        ("warped, ramp", warped_tone(), tempo_ramp(), 4e-4),
    ];
    for (name, audio, tempo, bound) in cases {
        let whole = play(&song(&tempo, &audio, length, None), &media, 180_000, 512);
        assert!(whole.iter().any(|&s| s != 0.0), "{name}");
        // Two milliseconds each side, a lopsided span, and all of it after
        // the boundary.
        for (i, (before, after)) in [(0.002, 0.002), (0.013, 0.0371), (0.0, 0.25)]
            .into_iter()
            .enumerate()
        {
            let at = Ticks::QUARTER + Ticks(3_141) + Ticks(length.get() * (i as i64 + 1) / 4);
            let editor = song(&tempo, &audio, length, Some((at, before, after)));
            let reference = play(&editor, &media, 180_000, 512);
            let worst = whole
                .iter()
                .zip(&reference)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(
                worst < bound,
                "{name}, span {before}+{after}: off by {worst}"
            );
            for block in [1, 61, 4096] {
                let other = play(&editor, &media, 180_000, block);
                let bits = |x: &[f32]| x.iter().map(|s| s.to_bits()).collect::<Vec<_>>();
                assert_eq!(bits(&reference), bits(&other), "{name}: blocks of {block}");
            }
        }
    }
}

// Spec: audio-clips R24. Two unrelated clips crossfaded: the earlier plays on
// past its end and the later before its start, each silent where it has no
// audio there (an earlier clip whose End marker came first, a later clip at the
// start of its file).
#[test]
fn a_clip_is_silent_across_the_span_where_it_has_no_audio() {
    let mut editor = Editor::new();
    let a = track(&mut editor, TrackKind::Audio, None);
    let tempo = editor.document().settings().tempo.clone();
    let length = Ticks::QUARTER;
    let boundary = Ticks::QUARTER * 2;
    clip(
        &mut editor,
        a,
        Ticks::QUARTER,
        length,
        AudioData {
            end: Some(seconds(frame_of(&tempo, length)) + 0.001),
            ..AudioData::new(hash(1))
        },
    );
    clip(&mut editor, a, boundary, length, AudioData::new(hash(2)));
    let to = frame_of(&tempo, boundary);
    // The later clip starts at its file's start, so it can't be read before
    // its start: the crossfade runs 480 frames, all of them after the boundary.
    editor
        .apply(Command::SetCrossfade {
            track: a,
            boundary,
            before: 0.0,
            after: seconds(480),
            curve: 0.0,
        })
        .unwrap();
    let mut media = MediaMap::new();
    media.insert(hash(1), steady(96_000, 0.5));
    media.insert(hash(2), steady(96_000, 0.25));
    let left = play(&editor, &media, 52_000, 64);
    // The earlier clip's End marker is 48 frames past its end, so it plays
    // those and nothing after; the later clip fades in over 480.
    for e in 0..480 {
        let frame = to + e;
        let earlier = if e < 48 {
            0.5 * (1.0 - e as f64 / 480.0)
        } else {
            0.0
        };
        let later = 0.25 * (e as f64 / 480.0);
        assert!(
            (f64::from(left[frame]) - (earlier + later)).abs() < 1e-6,
            "{e} frames in"
        );
    }
    assert!((left[to - 1] - 0.5).abs() < 1e-6);
    assert!((left[to + 480] - 0.25).abs() < 1e-6);
}

// Spec: audio-clips R24, R25. A crossfade that reaches back exactly to the
// start of the later clip's file still plays that clip over the whole span:
// two clips of one level sum to that level, with no dip at the boundary.
#[test]
fn a_crossfade_that_reaches_the_files_start_plays_the_whole_span() {
    let mut editor = Editor::new();
    let a = track(&mut editor, TrackKind::Audio, None);
    let tempo = editor.document().settings().tempo.clone();
    let (start, length) = (Ticks::QUARTER, Ticks::QUARTER * 2);
    let boundary = start + length;
    let before = 0.01;
    clip(&mut editor, a, start, length, AudioData::new(hash(1)));
    clip(
        &mut editor,
        a,
        boundary,
        length,
        AudioData {
            start: before,
            ..AudioData::new(hash(2))
        },
    );
    editor
        .apply(Command::SetCrossfade {
            track: a,
            boundary,
            before,
            after: 0.0,
            curve: 0.0,
        })
        .unwrap();
    let mut media = MediaMap::new();
    media.insert(hash(1), steady(96_000, 0.5));
    media.insert(hash(2), steady(96_000, 0.5));
    let left = play(&editor, &media, 120_000, 64);
    let to = frame_of(&tempo, boundary);
    let span = (before * RATE.as_f64()) as usize;
    for (frame, sample) in left.iter().enumerate().take(to + 10).skip(to - span + 2) {
        assert!((sample - 0.5).abs() < 1e-3, "frame {frame}: {sample}");
    }
}

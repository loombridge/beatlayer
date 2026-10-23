//! Warped audio clips under a groove (WP-6.19): where a clip's audio plays,
//! read back off a ramp file whose every sample says which file frame it is
//! and off the generated drum loop, and the gain steps of Velocity, at
//! several block sizes under the allocation trap. Spec: grooves R12 to R15.

use crate::common;
use std::sync::Arc;

use bl_core::time::{Bpm, TICKS_PER_QUARTER, TempoMap, Ticks};
use bl_engine::{EngineConfig, MediaMap, Offline, Registry, SessionCommand, TransportCommand};
use bl_media::SampleData;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, Editor, Groove, GrooveId, GrooveNote,
    TrackId, TrackKind, WarpMarker, WarpMode,
};
use common::{RATE, track};

const BAR: Ticks = Ticks(TICKS_PER_QUARTER * 4);
const STEP: i64 = TICKS_PER_QUARTER / 4;
/// Ticks to a frame at 120 BPM and 48 kHz.
const TICKS_PER_FRAME: f64 = TICKS_PER_QUARTER as f64 / 24_000.0;
const MEDIA: ContentHash = ContentHash([7; 32]);

/// A file whose every sample says where it is: frame `i` is `(i + 1)` steps.
const RAMP_STEP: f32 = 1.0 / 65_536.0;

fn ramp(frames: usize) -> Arc<SampleData> {
    Arc::new(
        SampleData::new(
            48_000,
            vec![(0..frames).map(|i| (i + 1) as f32 * RAMP_STEP).collect()],
        )
        .unwrap(),
    )
}

/// The file frame a ramp sample says it is.
fn frame_of_sample(sample: f32) -> f64 {
    f64::from(sample) / f64::from(RAMP_STEP) - 1.0
}

/// How far each sixteenth of the tests' groove sits from its line, in steps:
/// on the line, an eighth late, an eighth early, a quarter late.
const OFFSETS: [f64; 4] = [0.0, 0.125, -0.125, 0.25];

fn offset(line: i64) -> f64 {
    OFFSETS[line.rem_euclid(4) as usize] * STEP as f64
}

/// The tests' groove: a bar of sixteenths, each at its line's offset, loud
/// on the first of every four and soft on the others.
fn push(id: GrooveId) -> Groove {
    let notes = (0..16)
        .map(|i| GrooveNote {
            position: Ticks((i * STEP) + offset(i) as i64),
            velocity: if i % 4 == 0 { 127 } else { 65 },
        })
        .collect();
    Groove::new(id, "Push".to_owned(), BAR, notes)
}

/// A warped Re-Pitch clip of the file at the song's own tempo.
fn warped(mode: WarpMode) -> AudioData {
    AudioData {
        warp: true,
        warp_markers: vec![WarpMarker {
            time: 0.0,
            beat: Ticks::ZERO,
        }],
        tempo: Some(120.0),
        warp_mode: mode,
        high_quality: false,
        ..AudioData::new(MEDIA)
    }
}

/// A song with a clip of `audio` on an audio track: in the Arrangement for
/// `bars` bars, or in the first Session slot, with a groove shaped by
/// `shape` on it.
struct Song {
    editor: Editor,
    track: TrackId,
    scene: bl_model::SceneId,
    clip: ClipId,
    groove: GrooveId,
}

fn song(audio: AudioData, slot: bool, bars: i64, shape: impl FnOnce(&mut Groove)) -> Song {
    let mut editor = Editor::new();
    editor
        .apply(Command::SetTempo {
            tempo: TempoMap::constant(Bpm::new(120.0).unwrap()),
        })
        .unwrap();
    let track = track(&mut editor, TrackKind::Audio, None);
    let scene = editor.document().scenes().next().unwrap().id;
    let clip: ClipId = editor.new_id();
    let made = Clip::new(clip, Ticks::ZERO, BAR * bars, ClipData::Audio(audio));
    editor
        .apply(if slot {
            Command::CreateSlotClip {
                track,
                scene,
                clip: made,
            }
        } else {
            Command::CreateClip { track, clip: made }
        })
        .unwrap();
    let groove: GrooveId = editor.new_id();
    let mut pool_groove = push(groove);
    shape(&mut pool_groove);
    editor
        .apply(Command::AddGroove {
            groove: pool_groove,
        })
        .unwrap();
    editor
        .apply(Command::SetClipGroove {
            clips: vec![clip],
            groove: Some(groove),
        })
        .unwrap();
    Song {
        editor,
        track,
        scene,
        clip,
        groove,
    }
}

impl Song {
    /// Renders `frames` frames in blocks of `block`, left channel, under the
    /// allocation trap. A Session clip is launched first.
    fn render(&self, media: MediaMap, frames: u64, block: usize, launch: bool) -> Vec<f32> {
        let registry = Registry::new().with_media(Arc::new(media));
        let mut offline = Offline::new(EngineConfig::new(RATE, block), registry);
        offline.sync(self.editor.document());
        assert!(offline.missing_media().is_empty());
        if launch {
            offline.sender().session(SessionCommand::Launch {
                track: self.track,
                scene: self.scene,
            });
        } else {
            offline.sender().transport(TransportCommand::Play);
        }
        let mut out = Vec::new();
        offline.render(frames, |output| out.extend_from_slice(output.channel(0)));
        assert_eq!(offline.heap_operations(), 0, "blocks of {block}");
        out
    }
}

fn ramp_media() -> MediaMap {
    let mut media = MediaMap::new();
    media.insert(MEDIA, ramp(200_000));
    // A hit on every sixteenth, which Beats cuts its segments at.
    media.set_transients(MEDIA, (0..16).map(|i| f64::from(i) * 0.125).collect());
    media
}

/// Where the groove puts the file frame a song frame reads, worked out
/// from the law's words and not from the model's code: line `i` plays at
/// its place plus Timing's share of its offset, and the audio between two
/// lines is stretched between where they play (the first line and the pass's
/// end stay).
fn expected_position(frame: f64, timing: f64, bars: f64) -> f64 {
    let played = frame * TICKS_PER_FRAME;
    let lines = (16.0 * bars) as i64;
    let place = |i: i64| {
        (i * STEP) as f64
            + if i == 0 || i == lines {
                0.0
            } else {
                timing * offset(i)
            }
    };
    let mut i = 0;
    while i + 1 < lines && place(i + 1) <= played {
        i += 1;
    }
    let beat = (i * STEP) as f64 + (played - place(i)) / (place(i + 1) - place(i)) * STEP as f64;
    beat / TICKS_PER_FRAME
}

fn check_positions(out: &[f32], timing: f64) {
    for frame in (1..96_000).step_by(97) {
        let want = expected_position(frame as f64, timing, 1.0);
        let got = frame_of_sample(out[frame]);
        assert!(
            (got - want).abs() < 0.05,
            "song frame {frame}: the file's frame {got}, wanted {want}"
        );
    }
}

#[test]
fn a_warped_clips_audio_stretches_between_its_grooves_lines_at_any_block_size() {
    // Spec: grooves R6, R7, R12, R15
    for timing in [100.0, 50.0] {
        let song = song(warped(WarpMode::RePitch), false, 1, |g| g.timing = timing);
        let mut reference: Option<Vec<f32>> = None;
        for block in [1, 61, 512, 4_096] {
            let out = song.render(ramp_media(), 96_000, block, false);
            check_positions(&out, timing / 100.0);
            match &reference {
                None => reference = Some(out),
                Some(reference) => assert_eq!(&out, reference, "blocks of {block}"),
            }
        }
    }
}

#[test]
fn a_launched_session_clip_plays_through_the_same_displacement() {
    // Spec: grooves R12
    let song = song(warped(WarpMode::RePitch), true, 1, |_| {});
    let mut reference: Option<Vec<f32>> = None;
    for block in [1, 61, 512, 4_096] {
        let out = song.render(ramp_media(), 96_000, block, true);
        check_positions(&out, 1.0);
        match &reference {
            None => reference = Some(out),
            Some(reference) => assert_eq!(&out, reference, "blocks of {block}"),
        }
    }
}

#[test]
fn every_warp_mode_plays_through_the_groove() {
    // Spec: grooves R12; warp modes
    for mode in [
        WarpMode::Beats,
        WarpMode::Tones,
        WarpMode::Texture,
        WarpMode::RePitch,
        WarpMode::Complex,
        WarpMode::ComplexPro,
    ] {
        let grooved = song(warped(mode), false, 1, |_| {});
        let plain = song_without_groove(warped(mode));
        let a = grooved.render(ramp_media(), 96_000, 512, false);
        let b = plain.render(ramp_media(), 96_000, 512, false);
        assert_ne!(a, b, "{mode:?} plays the groove");
        // Beats and Re-Pitch read the table frame by frame, so they render the
        // same at any block size; the stretching modes keep state between blocks.
        if matches!(mode, WarpMode::Beats | WarpMode::RePitch) {
            assert_eq!(
                a,
                grooved.render(ramp_media(), 96_000, 4_096, false),
                "{mode:?}"
            );
        }
    }
}

fn song_without_groove(audio: AudioData) -> Song {
    let mut song = song(audio, false, 1, |_| {});
    song.editor
        .apply(Command::SetClipGroove {
            clips: vec![song.clip],
            groove: None,
        })
        .unwrap();
    song
}

#[test]
fn an_unwarped_clip_and_a_clip_with_no_groove_play_as_they_did() {
    // Spec: grooves R12, R14
    let unwarped = AudioData::new(MEDIA);
    let with_groove = song(unwarped.clone(), false, 1, |_| {});
    let without = song_without_groove(unwarped);
    assert_eq!(
        with_groove.render(ramp_media(), 96_000, 512, false),
        without.render(ramp_media(), 96_000, 512, false),
        "an unwarped clip keeps its groove and plays it unchanged"
    );
    // Switching Warp off leaves the groove on the clip and plays the file as
    // it is.
    let mut audio = warped(WarpMode::RePitch);
    audio.warp = false;
    let off = song(audio.clone(), false, 1, |_| {});
    assert_eq!(
        off.editor.document().clip(off.clip).unwrap().groove,
        Some(off.groove)
    );
    assert_eq!(
        off.render(ramp_media(), 96_000, 512, false),
        song_without_groove(audio).render(ramp_media(), 96_000, 512, false)
    );
    // A groove at 0 % Timing, Quantize, Random and Velocity changes nothing.
    let inert = song(warped(WarpMode::RePitch), false, 1, |g| g.timing = 0.0);
    assert_eq!(
        inert.render(ramp_media(), 96_000, 512, false),
        song_without_groove(warped(WarpMode::RePitch)).render(ramp_media(), 96_000, 512, false)
    );
}

#[test]
fn velocity_sets_the_gain_from_each_line_to_the_next_and_glides_over_a_millisecond() {
    // Spec: grooves R13
    let velocity = song(warped(WarpMode::RePitch), false, 1, |g| {
        g.timing = 0.0;
        g.velocity = 100.0;
    });
    let plain = song_without_groove(warped(WarpMode::RePitch));
    let got = velocity.render(ramp_media(), 96_000, 512, false);
    let base = plain.render(ramp_media(), 96_000, 512, false);
    // The groove's mean velocity is (4 * 127 + 12 * 65) / 16 = 80.5: the loud
    // lines gain 46.5 / 127 and the soft ones lose 15.5 / 127.
    let gain = |line: i64| {
        if line % 4 == 0 {
            1.0 + 46.5 / 127.0
        } else {
            1.0 - 15.5 / 127.0
        }
    };
    let step_frames = (STEP as f64 / TICKS_PER_FRAME) as i64;
    for line in 1..16 {
        let start = line * step_frames;
        // Past the glide, and before the next step, the gain is the line's.
        for frame in [start + 60, start + step_frames / 2, start + step_frames - 5] {
            let ratio = f64::from(got[frame as usize]) / f64::from(base[frame as usize]);
            assert!(
                (ratio - gain(line)).abs() < 1e-4,
                "line {line}, frame {frame}: gain {ratio}, wanted {}",
                gain(line)
            );
        }
        // Inside the glide it is between the two gains.
        let middle = f64::from(got[(start + 24) as usize]) / f64::from(base[(start + 24) as usize]);
        let (a, b) = (gain(line - 1), gain(line));
        assert!(
            middle > a.min(b) - 1e-4 && middle < a.max(b) + 1e-4,
            "line {line} glides: {middle} between {a} and {b}"
        );
    }
    // The gain changes frame by frame, never in a jump of more than a
    // millisecond's share of the step.
    let largest = got
        .windows(2)
        .zip(base.windows(2))
        .map(|(g, b)| {
            let (now, then) = (f64::from(g[1] / b[1]), f64::from(g[0] / b[0]));
            (now - then).abs()
        })
        .filter(|jump| jump.is_finite())
        .fold(0.0_f64, f64::max);
    let at = got
        .windows(2)
        .zip(base.windows(2))
        .position(|(g, b)| (f64::from(g[1] / b[1]) - f64::from(g[0] / b[0])).abs() == largest);
    assert!(
        largest < 0.02,
        "the largest jump in gain is {largest} at {at:?}"
    );
}

#[test]
fn random_draws_for_each_grid_line_each_pass_and_a_render_repeats() {
    // Spec: grooves R8, R12
    let mut audio = warped(WarpMode::RePitch);
    audio.looping = true;
    audio.loop_start = 0.0;
    audio.loop_end = Some(BAR.get() as f64);
    let random = |g: &mut Groove| {
        g.timing = 0.0;
        g.random = 100.0;
    };
    let song_a = song(audio.clone(), false, 2, random);
    let first = song_a.render(ramp_media(), 192_000, 512, false);
    // The pass's two halves read differently: each line draws again.
    let pass = |out: &[f32], n: usize| {
        (1..96_000)
            .step_by(97)
            .map(|f| frame_of_sample(out[n * 96_000 + f]) - f as f64)
            .collect::<Vec<_>>()
    };
    let (zero, one) = (pass(&first, 0), pass(&first, 1));
    assert!(zero.iter().any(|d| d.abs() > 1.0), "Random moves the audio");
    assert_ne!(zero, one);
    // No line moves more than a quarter of a step: 1500 frames.
    assert!(zero.iter().chain(&one).all(|d| d.abs() <= 1_501.0));
    // Two renders from the transport's start are the same, at any block size.
    for block in [1, 4_096] {
        let again =
            song(audio.clone(), false, 2, random).render(ramp_media(), 192_000, block, false);
        assert_eq!(again, first, "blocks of {block}");
    }
}

/// The generated drum loop (`tests/fixtures/gen/gen_drums.py`): its audio,
/// the transients `bl-analysis` finds in it, and the time of each hit.
fn drum_loop() -> (Arc<SampleData>, Vec<f64>, Vec<f64>) {
    // A folder of its own: the tests run in parallel and each needs the file.
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let folder = std::env::temp_dir().join(format!("bl-grooves-{}-{unique}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("drum-loop.wav");
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/drums/drum-loop.wav"
        ),
        &path,
    )
    .unwrap();
    let decoded = bl_media::decode_file(&path).unwrap();
    let hits: Vec<f64> = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/drums/drum-loop.hits"
    ))
    .unwrap()
    .lines()
    .map(|line| line.parse().unwrap())
    .collect();
    let transients = bl_analysis::onsets::transients(
        &bl_analysis::Audio::new(decoded.channel(0).unwrap(), 1, 48_000).unwrap(),
    );
    let channels = (0..decoded.channel_count())
        .map(|c| decoded.channel(c).unwrap().to_vec())
        .collect();
    (
        Arc::new(SampleData::new(48_000, channels).unwrap()),
        transients,
        hits,
    )
}

fn drum_media() -> MediaMap {
    let (sample, transients, _) = drum_loop();
    let mut map = MediaMap::new();
    map.insert(MEDIA, sample);
    map.set_transients(MEDIA, transients);
    map
}

/// The frame song tick `tick` falls on at 120 BPM.
fn frame_at(tick: f64) -> i64 {
    (tick / TICKS_PER_FRAME).round() as i64
}

/// Where in `out` the file's audio at `hit` plays, searched 10 ms either
/// side of `near`: the frame whose eight samples are the file's there.
fn plays_at(out: &[f32], file: &[f32], hit: usize, near: i64) -> i64 {
    let error = |f: usize| -> f64 {
        (0..8)
            .map(|k| {
                let d = f64::from(out[f + k]) - f64::from(file[hit + k]);
                d * d
            })
            .sum()
    };
    let lo = (near - 480).max(0) as usize;
    let hi = ((near + 480) as usize).min(out.len() - 8);
    (lo..=hi)
        .min_by(|&a, &b| error(a).total_cmp(&error(b)))
        .unwrap() as i64
}

#[test]
fn a_drum_loop_under_a_groove_plays_each_hit_on_the_line_the_groove_moves_it_to() {
    // Spec: grooves R7, R12, R15. Beats starts each hit's segment on its
    // beat, so the file's audio at the hit comes out on the frame the groove
    // puts it.
    let (_, transients, hits) = drum_loop();
    let file = drum_loop().0.channel(0).unwrap().to_vec();
    let song = song(warped(WarpMode::Beats), false, 1, |_| {});
    let mut reference: Option<Vec<f32>> = None;
    for block in [1, 61, 512, 4_096] {
        let out = song.render(drum_media(), 96_000 + 8_000, block, false);
        for (hit, transient) in hits.iter().zip(&transients) {
            // The hit is on its sixteenth, which the groove moves by its
            // offset.
            let line = (hit * 8.0).round() as i64;
            let want = frame_at((line * STEP) as f64 + offset(line));
            let at = plays_at(&out, &file, (transient * 48_000.0).round() as usize, want);
            assert!(
                (at - want).abs() <= 48,
                "hit on line {line}, blocks of {block}: plays at {at}, wanted {want}"
            );
        }
        match &reference {
            None => reference = Some(out),
            Some(reference) => assert_eq!(&out, reference, "blocks of {block}"),
        }
    }
}

#[test]
fn quantize_moves_a_drum_loops_hits_to_their_lines_before_the_groove_acts() {
    // Spec: grooves R12. The clip's hits are a 1/64 late; Quantize at 100 %
    // with the groove's Timing at 0 puts each on its line.
    let (_, transients, hits) = drum_loop();
    let file = drum_loop().0.channel(0).unwrap().to_vec();
    let mut late = warped(WarpMode::Beats);
    late.warp_markers = vec![WarpMarker {
        time: 0.0,
        beat: Ticks(STEP / 16),
    }];
    let quantized = song(late.clone(), false, 1, |g| {
        g.timing = 0.0;
        g.quantize = 100.0;
    });
    let unquantized = song(late, false, 1, |g| {
        g.timing = 0.0;
        g.quantize = 0.0;
    });
    let out = quantized.render(drum_media(), 96_000 + 8_000, 512, false);
    let raw = unquantized.render(drum_media(), 96_000 + 8_000, 512, false);
    for (hit, transient) in hits.iter().zip(&transients) {
        let line = (hit * 8.0).round() as i64;
        let at_hit = (transient * 48_000.0).round() as usize;
        let on_line = frame_at((line * STEP) as f64);
        let late_frame = frame_at((line * STEP + STEP / 16) as f64);
        let moved = plays_at(&out, &file, at_hit, on_line);
        assert!(
            (moved - on_line).abs() <= 48,
            "hit on line {line} plays at {moved}, wanted its line at {on_line}"
        );
        let kept = plays_at(&raw, &file, at_hit, late_frame);
        assert!(
            (kept - late_frame).abs() <= 48,
            "without Quantize the hit stays at {kept}, wanted {late_frame}"
        );
    }
}

#[test]
fn a_drum_loop_in_re_pitch_follows_the_groove_and_renders_the_same_at_any_block_size() {
    // Spec: grooves R12, R13 (S13). Re-Pitch plays through the groove too.
    let (_, _, hits) = drum_loop();
    let song = song(warped(WarpMode::RePitch), false, 1, |_| {});
    let mut reference: Option<Vec<f32>> = None;
    for block in [1, 61, 512, 4_096] {
        let out = song.render(drum_media(), 96_000, block, false);
        if block == 512 {
            let heard =
                bl_analysis::onsets::transients(&bl_analysis::Audio::new(&out, 1, 48_000).unwrap());
            for hit in &hits {
                let line = (hit * 8.0).round() as i64;
                let want = frame_at((line * STEP) as f64 + offset(line)) as f64 / 48_000.0;
                let near = heard
                    .iter()
                    .map(|t| (t - want).abs())
                    .fold(f64::INFINITY, f64::min);
                assert!(
                    near < 0.004,
                    "hit on line {line}: no onset within 4 ms of {want} ({near})"
                );
            }
        }
        match &reference {
            None => reference = Some(out),
            Some(reference) => assert_eq!(&out, reference, "blocks of {block}"),
        }
    }
}

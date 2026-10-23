//! Warped playback (the warping spec, R12 to R19, and the warp modes spec,
//! R4, R9 to R13): which file frame a warped clip plays at each song frame
//! under a steady tempo and a ramp, exactly the same at any block size, with
//! every block a real-time section under the allocation trap.

use crate::common;
use std::sync::Arc;

use bl_core::time::{Bpm, TempoMap, Ticks};
use bl_engine::{EngineConfig, MediaMap, Offline, Registry, TransportCommand, When};
use bl_media::SampleData;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, Editor, TrackId, TrackKind,
    WarpMarker, WarpMode,
};
use common::{RATE, track};

/// A step between neighbouring samples of a ramp: 2⁻¹⁶, exact in `f32`.
const STEP: f32 = 1.0 / 65_536.0;

/// A file whose every sample says where it is: frame `i` is `i + 1` steps on
/// the left and minus that on the right. A straight line, which both
/// interpolators read exactly, so a render shows the file position of each
/// frame of the song to within `f32`'s rounding.
fn ramp(frames: usize) -> Arc<SampleData> {
    let left: Vec<f32> = (0..frames).map(|i| (i + 1) as f32 * STEP).collect();
    let right = left.iter().map(|s| -s).collect();
    Arc::new(SampleData::new(48_000, vec![left, right]).unwrap())
}

/// The file position a left-channel sample of a ramp says it was read at.
fn position(sample: f32) -> f64 {
    f64::from(sample) / f64::from(STEP) - 1.0
}

fn hash(n: u8) -> ContentHash {
    ContentHash([n; 32])
}

fn bpm(value: f64) -> Bpm {
    Bpm::new(value).unwrap()
}

/// A warped clip of the ramp: its only warp marker puts the file's start on
/// the clip's 1.1.1, and the file runs at `file_bpm`.
fn warped(file_bpm: f64) -> AudioData {
    AudioData {
        warp: true,
        warp_markers: vec![WarpMarker {
            time: 0.0,
            beat: Ticks::ZERO,
        }],
        tempo: Some(file_bpm),
        // Re-Pitch through the cubic, exact on a ramp.
        warp_mode: WarpMode::RePitch,
        high_quality: false,
        ..AudioData::new(hash(1))
    }
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

fn set_tempo(editor: &mut Editor, breakpoints: &[(Ticks, f64)]) {
    let tempo = TempoMap::from_breakpoints(breakpoints.iter().map(|&(at, value)| (at, bpm(value))))
        .unwrap();
    editor.apply(Command::SetTempo { tempo }).unwrap();
}

/// Plays `editor`'s song from 1.1.1 for `frames` frames in blocks of
/// `block`, with `cues` on their frames, and returns the left channel.
fn play(
    editor: &Editor,
    media: &MediaMap,
    frames: u64,
    block: usize,
    cues: &[(u64, TransportCommand)],
) -> Vec<f32> {
    let registry = Registry::new().with_media(Arc::new(media.clone()));
    let mut offline = Offline::new(EngineConfig::new(RATE, block), registry);
    offline.sync(editor.document());
    assert!(offline.missing_media().is_empty());
    offline.sender().transport(TransportCommand::Play);
    for &(frame, command) in cues {
        offline.sender().transport_at(command, When::Frame(frame));
    }
    let mut out = Vec::new();
    offline.render(frames, |output| out.extend_from_slice(output.channel(0)));
    assert_eq!(offline.heap_operations(), 0);
    out
}

/// The file position the song's tempo map and a clip's warp map put at song
/// frame `frame`, worked out from first principles: the frame's tick, the
/// contents beat there, the file time at that beat.
fn mapped(tempo: &TempoMap, clip_start: Ticks, audio: &AudioData, frame: i64) -> f64 {
    let ticks = tempo.seconds_to_ticks_exact(frame as f64 / RATE.as_f64());
    let beat = audio.start + (ticks - clip_start.get() as f64);
    audio.warp_map().unwrap().beat_to_time(beat) * RATE.as_f64()
}

/// A song of one audio track with the warped `audio` on it from `start` for
/// `length`, under `tempo`'s breakpoints.
fn song(tempo: &[(Ticks, f64)], start: Ticks, length: Ticks, audio: AudioData) -> Editor {
    let mut editor = Editor::new();
    set_tempo(&mut editor, tempo);
    let a = track(&mut editor, TrackKind::Audio, None);
    clip(&mut editor, a, start, length, audio);
    editor
}

fn media() -> MediaMap {
    let mut media = MediaMap::new();
    // A little past the two seconds the tests read to, so the interpolator's
    // filter, which reaches a few hundred frames ahead of a faster read,
    // finds the ramp rather than the file's end.
    media.insert(hash(1), ramp(97_000));
    media
}

/// The song's tempo map, as its document holds it.
fn tempo_of(editor: &Editor) -> TempoMap {
    editor.document().settings().tempo.clone()
}

// Spec: warping R12. Under a steady tempo and under a ramp, a warped clip
// plays the file position its beat maps to, within a hundredth of a frame
// (and the ramp's `f32` rounding), at every block size.
#[test]
fn a_warped_clip_plays_the_file_position_its_beat_maps_to_at_any_block_size() {
    let media = media();
    let steady = [(Ticks::ZERO, 120.0)];
    let ramping = [(Ticks::ZERO, 90.0), (Ticks::QUARTER * 2, 150.0)];
    for (name, tempo, file_bpm) in [("steady", &steady[..], 90.0), ("ramp", &ramping[..], 100.0)] {
        let audio = warped(file_bpm);
        let editor = song(tempo, Ticks::ZERO, Ticks::QUARTER * 2, audio.clone());
        let map = tempo_of(&editor);
        let end = map.ticks_to_frames(Ticks::QUARTER * 2, RATE).get() as usize;
        let reference = play(&editor, &media, end as u64, 512, &[]);
        for block in [1, 61, 512, 4_096] {
            let got = play(&editor, &media, end as u64, block, &[]);
            assert_eq!(got, reference, "{name}: blocks of {block}");
        }
        let mut worst: f64 = 0.0;
        // From where the interpolator's filter, which reaches a few hundred
        // frames either side, no longer reaches back past the file's start.
        for (frame, &sample) in reference.iter().enumerate().skip(300) {
            let want = mapped(&map, Ticks::ZERO, &audio, frame as i64);
            worst = worst.max((position(sample) - want).abs());
        }
        assert!(worst < 0.02, "{name}: {worst} frames from the map");
    }
}

// Spec: warp-modes R4. A warped clip whose file runs at the song's tempo
// reads the file frame for frame: it adds its samples to the bit, as an
// unwarped clip does.
#[test]
fn a_warped_clip_at_the_songs_tempo_plays_to_the_bit_what_an_unwarped_one_does() {
    let media = media();
    // The Start marker is the file's 0.25 s, half a beat at 120 BPM.
    let unwarped = AudioData {
        start: 0.25,
        ..AudioData::new(hash(1))
    };
    let neutral = AudioData {
        start: Ticks::QUARTER.get() as f64 / 2.0,
        warp_mode: WarpMode::Beats,
        high_quality: true,
        ..warped(120.0)
    };
    let start = Ticks(12_345);
    let plain = song(&[(Ticks::ZERO, 120.0)], start, Ticks::QUARTER * 2, unwarped);
    let editor = song(&[(Ticks::ZERO, 120.0)], start, Ticks::QUARTER * 2, neutral);
    let want = play(&plain, &media, 60_000, 512, &[]);
    for block in [1, 61, 4_096] {
        assert_eq!(
            play(&editor, &media, 60_000, block, &[]),
            want,
            "blocks of {block}"
        );
    }
    assert!(want.iter().any(|&sample| sample != 0.0));
}

// Spec: warping R12 and R13. A song twice as fast reads the file twice as
// fast, and a song at half the tempo reads it at half the speed: the pitch
// follows (Re-Pitch).
#[test]
fn a_faster_song_reads_the_file_faster() {
    let media = media();
    for (song_bpm, speed) in [(120.0, 2.0), (30.0, 0.5)] {
        let editor = song(
            &[(Ticks::ZERO, song_bpm)],
            Ticks::ZERO,
            Ticks::QUARTER,
            warped(60.0),
        );
        let out = play(&editor, &media, 20_000, 256, &[]);
        let step = position(out[1_001]) - position(out[1_000]);
        assert!((step - speed).abs() < 0.01, "{song_bpm} BPM: {step}");
    }
}

// Spec: warping R19. A locate into a warped clip plays from the file position
// the map gives for the frame it lands on, on that frame; so does the
// Arrangement loop's wrap.
#[test]
fn a_locate_into_a_warped_clip_plays_from_its_mapped_position() {
    let media = media();
    let mut editor = song(
        &[(Ticks::ZERO, 120.0)],
        Ticks::ZERO,
        Ticks::WHOLE,
        warped(60.0),
    );
    // Twice the speed: beat 1.5 is the file's 1.5 beats at 60 BPM: 1.5 s.
    let cues = [(1_000, TransportCommand::Locate(Ticks::QUARTER * 3 / 2))];
    let out = play(&editor, &media, 6_000, 128, &cues);
    assert_eq!(position(out[999]), 1_998.0);
    assert!((position(out[1_000]) - 72_000.0).abs() < 0.02);
    assert!((position(out[1_001]) - 72_002.0).abs() < 0.02);
    // The Arrangement loop from beat 2 to beat 3 (song frames 24,000 to
    // 48,000) sends playback back to beat 2, whose file position is twice
    // the frame.
    editor
        .apply(Command::SetLoop {
            start: Ticks::QUARTER,
            length: Ticks::QUARTER,
        })
        .unwrap();
    editor
        .apply(Command::SetLoopEnabled { enabled: true })
        .unwrap();
    let out = play(&editor, &media, 60_000, 128, &[]);
    assert!((position(out[47_999]) - 95_998.0).abs() < 0.02);
    assert!((position(out[48_000]) - 48_000.0).abs() < 0.02);
    assert!((position(out[48_001]) - 48_002.0).abs() < 0.02);
}

// Spec: warping R5. A warped clip's loop wraps on the exact frame its brace
// ends, in the beats of its contents, under a steady tempo and a ramp.
#[test]
fn a_warped_loop_wraps_on_the_frame_its_brace_ends() {
    let media = media();
    let quarter = Ticks::QUARTER.get() as f64;
    let audio = AudioData {
        looping: true,
        start: quarter,
        loop_start: quarter,
        loop_end: Some(2.0 * quarter),
        ..warped(120.0)
    };
    let editor = song(
        &[(Ticks::ZERO, 120.0)],
        Ticks::ZERO,
        Ticks::QUARTER * 4,
        audio.clone(),
    );
    let out = play(&editor, &media, 100_000, 512, &[]);
    // At 120 BPM a beat is 24,000 frames: the clip plays the file's beat
    // 1 to beat 2 (frames 24,000 to 48,000), then again.
    assert_eq!(position(out[0]), 24_000.0);
    assert_eq!(position(out[23_999]), 47_999.0);
    assert_eq!(position(out[24_000]), 24_000.0);
    assert_eq!(position(out[47_999]), 47_999.0);
    assert_eq!(position(out[48_000]), 24_000.0);
    // The same loop in a song at 90 BPM reads 0.75 file frames to the frame,
    // and wraps at the frame its beat ends.
    let slow = song(
        &[(Ticks::ZERO, 90.0)],
        Ticks::ZERO,
        Ticks::QUARTER * 4,
        audio,
    );
    let out = play(&slow, &media, 100_000, 512, &[]);
    let wrap = 32_000;
    assert!((position(out[wrap - 1]) - (48_000.0 - 0.75)).abs() < 0.02);
    assert!((position(out[wrap]) - 24_000.0).abs() < 0.02);
}

// Spec: warping R5. The parts of a split warped clip play exactly what the
// whole did.
#[test]
fn split_warped_clips_play_exactly_what_the_whole_did() {
    let media = media();
    let tempo =
        TempoMap::from_breakpoints([(Ticks::ZERO, bpm(100.0)), (Ticks::QUARTER * 2, bpm(140.0))])
            .unwrap();
    let audio = warped(110.0);
    let start = Ticks::QUARTER / 2 + Ticks(321);
    let length = Ticks::QUARTER * 2;
    let ramping = [(Ticks::ZERO, 100.0), (Ticks::QUARTER * 2, 140.0)];
    let whole = song(&ramping, start, length, audio.clone());
    let expected = play(&whole, &media, 70_000, 512, &[]);

    let mut parts = Editor::new();
    set_tempo(&mut parts, &ramping);
    let a = track(&mut parts, TrackKind::Audio, None);
    let whole_clip = Clip::new(parts.new_id(), start, length, ClipData::Audio(audio));
    for (from, to) in [
        (start, start + Ticks::QUARTER / 3),
        (start + Ticks::QUARTER / 3, start + Ticks::QUARTER),
        (start + Ticks::QUARTER, whole_clip.end()),
    ] {
        let piece = Clip {
            id: parts.new_id(),
            ..whole_clip.part(from, to, &tempo)
        };
        parts
            .apply(Command::CreateClip {
                track: a,
                clip: piece,
            })
            .unwrap();
    }
    let got = play(&parts, &media, 70_000, 512, &[]);
    // Positions agree to a billionth of a frame, so samples agree to `f32`'s
    // rounding.
    for (frame, (a, b)) in got.iter().zip(&expected).enumerate() {
        assert!((a - b).abs() < 1e-6, "frame {frame}: {a} against {b}");
    }
}

// Spec: warping R17, warp-modes R5. Warp edits, mode and setting changes,
// HiQ and RAM switches, locates and tempo swaps while warped clips play
// touch the heap on the audio thread never (the same rounds `rtsan/tests/
// warping.rs` plays under RealtimeSanitizer).
#[test]
fn warp_edits_locates_and_tempo_swaps_play_on_under_the_allocation_trap() {
    use bl_model::BeatsUpdate;

    let mut rig = common::Rig::with_registry(256, |registry| {
        let mut media = MediaMap::new();
        media.insert(hash(1), ramp(96_000));
        media.insert(hash(2), ramp(12_000));
        registry.set_media(Arc::new(media));
    });
    set_tempo(
        &mut rig.editor,
        &[(Ticks::ZERO, 70.0), (Ticks::WHOLE * 2, 170.0)],
    );
    let tracks: Vec<TrackId> = (0..3)
        .map(|_| track(&mut rig.editor, TrackKind::Audio, None))
        .collect();
    let quarter = Ticks::QUARTER.get() as f64;
    let warped = |media: u8, tempo: f64, looping: bool, high_quality: bool| AudioData {
        warp_markers: vec![
            WarpMarker {
                time: 0.0,
                beat: Ticks::ZERO,
            },
            WarpMarker {
                time: 0.5,
                beat: Ticks::QUARTER,
            },
        ],
        tempo: Some(tempo),
        high_quality,
        looping,
        loop_start: quarter / 2.0,
        loop_end: looping.then_some(quarter * 3.0),
        media: hash(media),
        ..warped(tempo)
    };
    let mut clips = Vec::new();
    for (track, start, audio) in [
        (tracks[0], Ticks::ZERO, warped(1, 100.0, true, true)),
        (tracks[1], Ticks(12_345), warped(1, 130.0, false, false)),
        (tracks[1], Ticks::QUARTER, warped(2, 90.0, true, false)),
        (tracks[2], Ticks::ZERO, warped(1, 120.0, false, true)),
    ] {
        clips.push(clip(&mut rig.editor, track, start, Ticks::WHOLE * 2, audio));
    }
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    for round in 0..600u64 {
        let clip = clips[(round % 4) as usize];
        let command = match round % 24 {
            0 => Some(Command::SetClipHighQuality {
                clip,
                high_quality: round % 48 == 0,
            }),
            3 => Some(Command::SetClipRamMode {
                clip,
                ram: round % 48 == 3,
            }),
            6 => Some(Command::SetClipWarpMode {
                clip,
                mode: if round % 48 == 6 {
                    WarpMode::Complex
                } else {
                    WarpMode::RePitch
                },
            }),
            9 => Some(Command::SetSegmentBpm {
                clip,
                marker: Some(0),
                bpm: 80.0 + (round % 60) as f64,
            }),
            12 => Some(Command::InsertWarpMarker {
                clips: vec![clip],
                beat: Some(Ticks(100_000 + (round as i64 % 7) * 90_000)),
                time: None,
                pin_neighbours: false,
                transients: None,
            }),
            15 => Some(Command::SetClipWarpSettings {
                clip,
                beats: Some(BeatsUpdate {
                    transient_envelope: Some((round % 101) as u32),
                    ..BeatsUpdate::default()
                }),
                tones: None,
                texture: None,
                complex_pro: None,
            }),
            18 => Some(Command::SetClipGain {
                clip,
                gain: (round % 7) as f64 / 6.0,
            }),
            21 if round % 240 == 21 => Some(Command::SetTempo {
                tempo: TempoMap::constant(bpm(100.0 + (round % 400) as f64 / 10.0)),
            }),
            _ => None,
        };
        if let Some(command) = command
            && rig.editor.apply(command).is_ok()
        {
            rig.sync();
        }
        if round % 100 == 50 {
            rig.sender
                .transport(TransportCommand::Locate(Ticks(98_765 * (round % 7) as i64)));
        }
        rig.block(256);
    }
    let heard = tracks
        .iter()
        .filter_map(|&t| rig.monitor.state().meter(t))
        .any(|[left, _]| left > 0.0);
    assert!(heard, "the clips play");
}

/// The frequency in hertz of the steady sine `samples` (at 48 kHz), from the
/// times its upward zero crossings fall at, each worked out between frames
/// by a straight line.
fn pitch(samples: &[f32]) -> f64 {
    let crossings: Vec<f64> = samples
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| pair[0] < 0.0 && pair[1] >= 0.0)
        .map(|(i, pair)| i as f64 + f64::from(-pair[0]) / f64::from(pair[1] - pair[0]))
        .collect();
    let cycles = (crossings.len() - 1) as f64;
    cycles * RATE.as_f64() / (crossings[crossings.len() - 1] - crossings[0])
}

// Spec: warp-modes R9, R35. Re-Pitch's pitch follows the tempo ratio: a
// 440 Hz tone warped at 100 BPM in a song at 130 plays 1.3 times as high,
// within a cent, through either interpolator.
#[test]
fn re_pitch_follows_the_tempo_ratio_within_a_cent() {
    let tone: Vec<f32> = (0..96_000)
        .map(|i| (440.0 * std::f64::consts::TAU * f64::from(i) / 48_000.0).sin() as f32 * 0.5)
        .collect();
    let mut media = MediaMap::new();
    media.insert(
        hash(1),
        Arc::new(SampleData::new(48_000, vec![tone.clone(), tone]).unwrap()),
    );
    for (song_bpm, file_bpm) in [(130.0, 100.0), (75.0, 100.0), (240.0, 60.0)] {
        for high_quality in [false, true] {
            let audio = AudioData {
                high_quality,
                ..warped(file_bpm)
            };
            let editor = song(&[(Ticks::ZERO, song_bpm)], Ticks::ZERO, Ticks::WHOLE, audio);
            // Skip the interpolator's first frames.
            let heard = play(&editor, &media, 40_000, 512, &[]);
            let want = 440.0 * song_bpm / file_bpm;
            // While the file lasts: at four times the speed, 20,000 frames read it all.
            let got = pitch(&heard[2_000..20_000]);
            let cents = 1_200.0 * (got / want).log2();
            assert!(
                cents.abs() < 1.0,
                "{song_bpm} BPM over {file_bpm}, HiQ {high_quality}: {got} Hz for {want}, {cents} cents"
            );
        }
    }
}

// Spec: warp-modes R13. A warped clip's gain and fades scale what its mode
// puts out, from the clip's edges in the Arrangement.
#[test]
fn gain_and_fades_scale_the_modes_output() {
    let media = media();
    let audio = AudioData {
        gain: 0.5,
        fade_in: 0.01,
        fade_out: 0.01,
        ..warped(120.0)
    };
    let length = Ticks::QUARTER;
    let editor = song(&[(Ticks::ZERO, 120.0)], Ticks::ZERO, length, audio);
    let out = play(&editor, &media, 24_000, 128, &[]);
    let file = |frame: usize| f64::from((frame + 1) as f32 * STEP);
    for frame in [0, 1, 100, 479, 480, 5_000, 23_519, 23_520, 23_999] {
        // 480 frames of fade at 48 kHz, from the clip's edges.
        let fade = (frame as f64 / 480.0).min(((24_000 - frame) as f64 / 480.0).min(1.0));
        let want = (file(frame) * (0.5 * fade)) as f32;
        assert!(
            (out[frame] - want).abs() < 1e-9,
            "frame {frame}: {} against {want}",
            out[frame]
        );
    }
}

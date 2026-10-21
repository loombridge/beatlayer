//! Beats (WP-4.4, the warp modes spec R4, R8 and R14 to R17): the clip cut
//! into segments at its transients, each started on its beat and read at the
//! pitch ratio, with the loop modes filling a short one — the same sample at
//! any block size, every block a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use std::sync::Arc;

use bl_analysis::Audio as Analysed;
use bl_analysis::onsets;
use bl_core::time::{Bpm, TempoMap, Ticks};
use bl_engine::{EngineConfig, MediaMap, Offline, Registry, TransportCommand};
use bl_media::SampleData;
use bl_model::{
    AudioData, BeatsSettings, Clip, ClipData, Command, ContentHash, Editor, Preserve,
    TextureSettings, TonesSettings, TrackKind, TransientLoopMode, WarpMarker, WarpMode,
};
use common::{RATE, track};

/// The generated drum loop (`tests/fixtures/gen/gen_drums.py`): its audio,
/// the transients `bl-analysis` finds in it, and the time of each hit.
fn drum_loop() -> (Arc<SampleData>, Vec<f64>, Vec<f64>) {
    // A folder of its own: the tests run in parallel and each needs the file.
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let folder =
        std::env::temp_dir().join(format!("bl-warp-modes-{}-{unique}", std::process::id()));
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
    let transients =
        onsets::transients(&Analysed::new(decoded.channel(0).unwrap(), 1, 48_000).unwrap());
    let mut channels = Vec::new();
    for c in 0..decoded.channel_count() {
        channels.push(decoded.channel(c).unwrap().to_vec());
    }
    let sample = Arc::new(SampleData::new(48_000, channels).unwrap());
    (sample, transients, hits)
}

const MEDIA: ContentHash = ContentHash([7; 32]);

/// The drum loop's media at the engine's rate, with its transients.
fn media() -> MediaMap {
    let (sample, transients, _) = drum_loop();
    let mut map = MediaMap::new();
    map.insert(MEDIA, sample);
    map.set_transients(MEDIA, transients);
    map
}

/// A Beats clip of `media` warped at 120 BPM, in a song at `tempo`, with
/// `settings` and the loop's own controls.
fn song(tempo: f64, length: Ticks, settings: BeatsSettings, mode: WarpMode) -> Editor {
    let mut editor = Editor::new();
    editor
        .apply(Command::SetTempo {
            tempo: TempoMap::constant(Bpm::new(tempo).unwrap()),
        })
        .unwrap();
    let audio_track = track(&mut editor, TrackKind::Audio, None);
    let mut audio = AudioData {
        warp: true,
        warp_markers: vec![WarpMarker {
            time: 0.0,
            beat: Ticks::ZERO,
        }],
        tempo: Some(120.0),
        warp_mode: mode,
        ..AudioData::new(MEDIA)
    };
    audio.warp_settings.beats = settings;
    let clip = Clip::new(editor.new_id(), Ticks::ZERO, length, ClipData::Audio(audio));
    editor
        .apply(Command::CreateClip {
            track: audio_track,
            clip,
        })
        .unwrap();
    editor
}

fn beats() -> BeatsSettings {
    AudioData::new(ContentHash([0; 32])).warp_settings.beats
}

/// Plays a song for `frames` frames in blocks of `block`, under the
/// allocation trap, and returns the left channel.
fn play(editor: &Editor, frames: u64, block: usize) -> Vec<f32> {
    let registry = Registry::new().with_media(Arc::new(media()));
    let mut offline = Offline::new(EngineConfig::new(RATE, block), registry);
    offline.sync(editor.document());
    assert!(offline.missing_media().is_empty());
    offline.sender().transport(TransportCommand::Play);
    let mut out = Vec::new();
    offline.render(frames, |output| out.extend_from_slice(output.channel(0)));
    assert_eq!(offline.heap_operations(), 0, "block size {block}");
    out
}

// Spec: warp-modes R4. At the clip's own tempo (speed 1) every Preserve and
// loop mode plays the file's frames unchanged, to the bit, at Transient
// Envelope 100.
#[test]
fn beats_at_its_own_tempo_plays_to_the_bit_for_every_setting() {
    // The clip is two seconds of the loop: one whole note at 120 BPM.
    let length = Ticks::WHOLE;
    let unwarped = {
        let mut editor = Editor::new();
        editor
            .apply(Command::SetTempo {
                tempo: TempoMap::constant(Bpm::new(120.0).unwrap()),
            })
            .unwrap();
        let audio_track = track(&mut editor, TrackKind::Audio, None);
        let clip = Clip::new(
            editor.new_id(),
            Ticks::ZERO,
            length,
            ClipData::Audio(AudioData::new(MEDIA)),
        );
        editor
            .apply(Command::CreateClip {
                track: audio_track,
                clip,
            })
            .unwrap();
        editor
    };
    let want = play(&unwarped, 96_000, 512);
    let presets = [
        Preserve::Transients,
        Preserve::Bar,
        Preserve::Half,
        Preserve::Quarter,
        Preserve::Eighth,
        Preserve::Sixteenth,
        Preserve::ThirtySecond,
    ];
    let loops = [
        TransientLoopMode::LoopOff,
        TransientLoopMode::LoopForward,
        TransientLoopMode::LoopBackAndForth,
    ];
    for preserve in presets {
        for transient_loop_mode in loops {
            let settings = BeatsSettings {
                preserve,
                transient_loop_mode,
                transient_envelope: 100,
            };
            let editor = song(120.0, length, settings, WarpMode::Beats);
            for block in [1, 61, 512, 4_096] {
                assert_eq!(
                    play(&editor, 96_000, block),
                    want,
                    "{preserve:?} {transient_loop_mode:?}, blocks of {block}"
                );
            }
        }
    }
}

// Spec: warp-modes R14, R35 (Beats' timing row). A generated drum loop warped
// at 120 BPM keeps every hit within 1 ms of its beat at 60, 90, 180 and
// 240 BPM, at every block size. A hit is a segment's start, and the segment
// starts on the frame its beat falls on through the warp map, so the file's
// samples at each hit come out of the render at that frame: the test reads
// them back there, which pins every hit to its beat to the sample.
#[test]
fn beats_keeps_every_hit_within_1_ms_of_its_beat() {
    let length = Ticks::WHOLE;
    let (sample, transients, hits) = drum_loop();
    let file = sample.channel(0).unwrap();
    // The clip is at 120 BPM, so a file second is two beats.
    let beats_per_second = 120.0 / 60.0;
    let rate = f64::from(RATE.hz());
    for tempo in [60.0, 90.0, 180.0, 240.0] {
        let editor = song(tempo, length, beats(), WarpMode::Beats);
        let tempo_map = editor.document().settings().tempo.clone();
        // Each hit's beat frame (where the target says its beginning lands),
        // and the file frames the engine starts the segment on: the transient
        // bl-analysis found, which its own test puts within 1 ms of the hit.
        let want: Vec<(usize, usize)> = hits
            .iter()
            .zip(&transients)
            .map(|(&hit, &transient)| {
                let ticks =
                    Ticks((hit * beats_per_second * Ticks::QUARTER.get() as f64).round() as i64);
                let beat = tempo_map.ticks_to_frames(ticks, RATE).get() as usize;
                (beat, (transient * rate).round() as usize)
            })
            .collect();
        // The whole note at this tempo, and a second past it.
        let frames = ((length_seconds(tempo) + 1.0) * rate) as u64;
        for block in [1, 61, 512, 4_096] {
            let out = play(&editor, frames, block);
            for (i, &(frame, hit)) in want.iter().enumerate() {
                // The file's audio at the hit plays out of the render at the
                // frame its segment starts on. Search 10 ms either side of the
                // beat for where it is (a wider window than the target, so
                // the check isn't circular) and require the engine to have put
                // it within 1 ms (48 frames at 48 kHz).
                let lo = frame.saturating_sub(480);
                let hi = (frame + 480).min(out.len() - 8);
                let error = |f: usize| -> f64 {
                    (0..8)
                        .map(|k| {
                            let d = f64::from(out[f + k]) - f64::from(file[hit + k]);
                            d * d
                        })
                        .sum()
                };
                let best = (lo..=hi)
                    .min_by(|&a, &b| error(a).total_cmp(&error(b)))
                    .unwrap();
                let off = (best as i64 - frame as i64).abs();
                assert!(
                    off <= 48,
                    "hit {i} at {tempo} BPM, blocks of {block}: plays at {best}, {off} frames from \
                     its beat {frame}"
                );
            }
        }
    }
}

/// The whole note's length in seconds at `tempo`.
fn length_seconds(tempo: f64) -> f64 {
    4.0 * 60.0 / tempo
}

/// A song of `MEDIA` warped at 120 BPM in `mode`, with `grain_ms`'s Grain
/// Size and (in Texture) `flux`, in a song at `tempo` (T125).
fn grain_song(tempo: f64, mode: WarpMode, grain_ms: f64, flux: f64) -> Editor {
    let mut editor = Editor::new();
    editor
        .apply(Command::SetTempo {
            tempo: TempoMap::constant(Bpm::new(tempo).unwrap()),
        })
        .unwrap();
    let audio_track = track(&mut editor, TrackKind::Audio, None);
    let mut audio = AudioData {
        warp: true,
        warp_markers: vec![WarpMarker {
            time: 0.0,
            beat: Ticks::ZERO,
        }],
        tempo: Some(120.0),
        warp_mode: mode,
        ..AudioData::new(MEDIA)
    };
    match mode {
        WarpMode::Tones => {
            audio.warp_settings.tones = TonesSettings {
                grain_size: grain_ms,
            };
        }
        WarpMode::Texture => {
            audio.warp_settings.texture = TextureSettings {
                grain_size: grain_ms,
                flux,
            };
        }
        _ => {}
    }
    let clip = Clip::new(
        editor.new_id(),
        Ticks::ZERO,
        Ticks::WHOLE,
        ClipData::Audio(audio),
    );
    editor
        .apply(Command::CreateClip {
            track: audio_track,
            clip,
        })
        .unwrap();
    editor
}

// Spec: warp-modes R4, R6. Tones and Texture at the clip's own tempo (speed 1)
// play the file's frames unchanged, to the bit, at any Grain Size and Flux and
// at any block size, each block a real-time section under the allocation trap.
#[test]
fn tones_and_texture_are_neutral_at_their_own_tempo_at_any_block_size() {
    let length = Ticks::WHOLE;
    let unwarped = {
        let mut editor = Editor::new();
        editor
            .apply(Command::SetTempo {
                tempo: TempoMap::constant(Bpm::new(120.0).unwrap()),
            })
            .unwrap();
        let audio_track = track(&mut editor, TrackKind::Audio, None);
        let clip = Clip::new(
            editor.new_id(),
            Ticks::ZERO,
            length,
            ClipData::Audio(AudioData::new(MEDIA)),
        );
        editor
            .apply(Command::CreateClip {
                track: audio_track,
                clip,
            })
            .unwrap();
        editor
    };
    let want = play(&unwarped, 96_000, 512);
    for (mode, grain_ms, flux) in [
        (WarpMode::Tones, 10.0, 0.0),
        (WarpMode::Tones, 30.0, 0.0),
        (WarpMode::Tones, 100.0, 0.0),
        (WarpMode::Texture, 10.0, 0.0),
        (WarpMode::Texture, 65.0, 25.0),
        (WarpMode::Texture, 500.0, 100.0),
    ] {
        let editor = grain_song(120.0, mode, grain_ms, flux);
        for block in [1, 61, 512, 4_096] {
            assert_eq!(
                play(&editor, 96_000, block),
                want,
                "{mode:?} at {grain_ms} ms, Flux {flux}, blocks of {block}"
            );
        }
    }
}

// Spec: warp-modes R5, R19. Texture at its largest Grain Size and at Flux's
// maximum still plays at a speed other than 1: the window a track's clips are
// read through holds the widest reach their settings ask for, so a mode never
// falls back to silence.
#[test]
fn texture_at_its_largest_grain_size_still_plays() {
    for (grain_ms, flux) in [(500.0, 0.0), (500.0, 25.0), (500.0, 100.0)] {
        let editor = grain_song(180.0, WarpMode::Texture, grain_ms, flux);
        let out = play(&editor, 48_000, 512);
        let sounding = out.iter().filter(|s| s.abs() > 1e-4).count();
        assert!(
            sounding > out.len() / 4,
            "Texture at {grain_ms} ms, Flux {flux}: {sounding} of {} frames hold sound",
            out.len()
        );
    }
}

/// The click file's media, a click at file frame `click_at`.
const CLICK: ContentHash = ContentHash([11; 32]);

/// A quiet file holding one click at frame `at`, one channel.
fn click_file(frames: usize, at: usize) -> Arc<SampleData> {
    let mut left = vec![0.0f32; frames];
    for sample in left
        .iter_mut()
        .take((at + 2).min(frames))
        .skip(at.saturating_sub(1))
    {
        *sample = 1.0;
    }
    Arc::new(SampleData::new(48_000, vec![left]).unwrap())
}

/// A song of the click file warped at 120 BPM in `mode`, in a song at 120 BPM,
/// one whole note long: at speed 1 the file frame a moment plays is the song
/// frame its warp map puts it on.
fn click_song(mode: WarpMode) -> Editor {
    let mut editor = Editor::new();
    editor
        .apply(Command::SetTempo {
            tempo: TempoMap::constant(Bpm::new(120.0).unwrap()),
        })
        .unwrap();
    let audio_track = track(&mut editor, TrackKind::Audio, None);
    let audio = AudioData {
        warp: true,
        warp_markers: vec![WarpMarker {
            time: 0.0,
            beat: Ticks::ZERO,
        }],
        tempo: Some(120.0),
        warp_mode: mode,
        ..AudioData::new(CLICK)
    };
    let clip = Clip::new(
        editor.new_id(),
        Ticks::ZERO,
        Ticks::WHOLE,
        ClipData::Audio(audio),
    );
    editor
        .apply(Command::CreateClip {
            track: audio_track,
            clip,
        })
        .unwrap();
    editor
}

/// The frame of the loudest sample of `out` within `window` frames of
/// `around`.
fn loudest(out: &[f32], around: usize, window: usize) -> usize {
    let lo = around.saturating_sub(window);
    let hi = (around + window).min(out.len() - 1);
    (lo..=hi)
        .max_by(|&a, &b| out[a].abs().total_cmp(&out[b].abs()))
        .unwrap()
}

/// Plays `editor` with `media` for `frames` frames in blocks of `block`, under
/// the allocation trap, and returns the left channel.
fn play_with(editor: &Editor, media: &MediaMap, frames: u64, block: usize) -> Vec<f32> {
    let registry = Registry::new().with_media(Arc::new(media.clone()));
    let mut offline = Offline::new(EngineConfig::new(RATE, block), registry);
    offline.sync(editor.document());
    assert!(offline.missing_media().is_empty());
    offline.sender().transport(TransportCommand::Play);
    let mut out = Vec::new();
    offline.render(frames, |output| out.extend_from_slice(output.channel(0)));
    assert_eq!(offline.heap_operations(), 0, "block size {block}");
    out
}

// Spec: warp-modes R24, R35 (Complex's timing row). A Complex or Complex Pro
// clip's click lands on the frame its warp map puts it on, within 20 ms, and
// the mode reports no latency: it plays the same frame as the same file in
// Re-Pitch, which adds none, so the stretcher's own read-ahead reaches the
// mixer as no delay.
#[test]
fn complex_clips_land_on_time_and_report_no_latency() {
    let click_at = 48_000usize;
    let mut media = MediaMap::new();
    media.insert(CLICK, click_file(96_000, click_at));
    // Re-Pitch reads the file frame for frame at speed 1, so its click is the
    // reference: the frame the warp map puts it on.
    let reference = play_with(&click_song(WarpMode::RePitch), &media, 60_000, 512);
    let want = loudest(&reference, click_at, 4_800);
    assert!(
        (want as i64 - click_at as i64).abs() <= 48,
        "Re-Pitch's own click landed at {want}"
    );
    for mode in [WarpMode::Complex, WarpMode::ComplexPro] {
        for block in [64, 512] {
            let out = play_with(&click_song(mode), &media, 60_000, block);
            let at = loudest(&out, click_at, 2_400);
            let off = (at as i64 - click_at as i64).abs();
            assert!(
                off <= 960,
                "{mode:?} at blocks of {block}: its click landed at {at}, {off} frames from \
                 {click_at}, past 20 ms"
            );
            let late = (at as i64 - want as i64).abs();
            assert!(
                late <= 960,
                "{mode:?}: its click landed {late} frames from Re-Pitch's own, so the mode \
                 reports latency"
            );
        }
    }
}

// Spec: warp-modes R8. A change of a playing clip's warp mode crossfades from
// what was playing, in a straight line over 20 ms.
#[test]
fn a_mode_change_crossfades_over_20_ms() {
    // Half the song's tempo, where Re-Pitch and Beats sound quite different:
    // Re-Pitch reads the file at half speed, Beats at 1:1 with its loops.
    let length = Ticks::WHOLE * 4;
    let tempo = 60.0;
    let block = 512;
    // The mode takes over from the first frame of the block the new list
    // arrives in (R8), so the change lands on a block's start.
    let change = 23_552_u64;
    let frames = 72_000_u64;
    let old = play(
        &song(tempo, length, beats(), WarpMode::RePitch),
        frames,
        block,
    );
    let new = play(
        &song(tempo, length, beats(), WarpMode::Beats),
        frames,
        block,
    );

    let editor = song(tempo, length, beats(), WarpMode::RePitch);
    let clip = editor
        .document()
        .tracks()
        .flat_map(|track| track.clips())
        .next()
        .unwrap()
        .id;
    let registry = Registry::new().with_media(Arc::new(media()));
    let mut offline = Offline::new(EngineConfig::new(RATE, block), registry);
    offline.sync(editor.document());
    offline.sender().transport(TransportCommand::Play);
    let mut mixed = Vec::new();
    let mut editor = editor;
    let mut done = 0;
    while done < frames {
        if done == change {
            editor
                .apply(Command::SetClipWarpMode {
                    clip,
                    mode: WarpMode::Beats,
                })
                .unwrap();
            offline.sync(editor.document());
        }
        let length = (block as u64).min(frames - done);
        mixed.extend_from_slice(offline.process(length as usize).channel(0));
        done += length;
    }
    assert_eq!(offline.heap_operations(), 0);
    // A straight line across the 20 ms after the change: 960 frames at the
    // engine's rate.
    // At the change the output is still mostly the old mode's, and over the
    // next 20 ms it moves to the new one's: a crossfade, not a step.
    for frame in [0, 240, 480, 720, 959] {
        let at = (change as usize) + frame;
        let t = (frame + 1) as f64 / 960.0;
        let want = f64::from(old[at]) * (1.0 - t) + f64::from(new[at]) * t;
        assert!(
            (f64::from(mixed[at]) - want).abs() < 1e-3,
            "frame {frame} of the fade: {} under {want}",
            mixed[at]
        );
    }
    let (was, now, got) = (
        f64::from(old[change as usize]),
        f64::from(new[change as usize]),
        f64::from(mixed[change as usize]),
    );
    assert!(
        (got - was).abs() < (now - was).abs() / 4.0,
        "the fade starts from the old mode: {got} between {was} and {now}"
    );
    assert_eq!(
        mixed[..change as usize],
        old[..change as usize],
        "before the change"
    );
    assert_eq!(
        mixed[change as usize + 960..],
        new[change as usize + 960..],
        "after the fade"
    );
}

// Spec: warp-modes R10. In Re-Pitch, Transpose and Detune don't act: the
// clip plays exactly what it played before they were set, and their values
// stay with the clip.
#[test]
fn re_pitch_ignores_the_clips_transpose_and_detune() {
    let length = Ticks::WHOLE * 2;
    let before = play(
        &song(120.0, length, beats(), WarpMode::RePitch),
        24_000,
        512,
    );
    let mut editor = song(120.0, length, beats(), WarpMode::RePitch);
    let clip = editor
        .document()
        .tracks()
        .flat_map(|track| track.clips())
        .next()
        .unwrap()
        .id;
    editor
        .apply(Command::SetClipPitch {
            clip,
            transpose: Some(12),
            detune: Some(25.0),
        })
        .unwrap();
    let after = play(&editor, 24_000, 512);
    assert_eq!(before, after, "Re-Pitch ignores the clip's pitch");
    // The values stay with the clip (R10).
    let ClipData::Audio(audio) = &editor.document().clip(clip).unwrap().data else {
        panic!("an audio clip");
    };
    assert_eq!((audio.transpose, audio.detune), (12, 25.0));
}

// Spec: warp-modes R25. The pitch change is held to −48 to 48 semitones,
// and a value outside the range is refused, changing nothing.
#[test]
fn transposition_stops_at_48_semitones() {
    let mut editor = song(120.0, Ticks::WHOLE * 2, beats(), WarpMode::Beats);
    let clip = editor
        .document()
        .tracks()
        .flat_map(|track| track.clips())
        .next()
        .unwrap()
        .id;
    editor
        .apply(Command::SetClipPitch {
            clip,
            transpose: Some(48),
            detune: None,
        })
        .unwrap();
    let ClipData::Audio(audio) = &editor.document().clip(clip).unwrap().data else {
        panic!("an audio clip");
    };
    assert_eq!(audio.transpose, 48);
    assert_eq!(audio.pitch_change(), 48.0);
    // Outside the range, the command refuses and changes nothing.
    let before = editor.document().clone();
    assert!(
        editor
            .apply(Command::SetClipPitch {
                clip,
                transpose: Some(49),
                detune: None,
            })
            .is_err()
    );
    assert_eq!(
        editor.document().clip(clip).unwrap().data,
        before.clip(clip).unwrap().data
    );
}

//! Beats (WP-4.4, the warp modes spec R4, R8 and R14 to R17): the clip cut
//! into segments at its transients, each started on its beat and read at the
//! pitch ratio, with the loop modes filling a short one — the same sample at
//! any block size, every block a real-time section under the allocation trap.

use crate::common;
use std::sync::Arc;

use bl_analysis::Audio as Analysed;
use bl_analysis::onsets;
use bl_core::time::{Bpm, TempoMap, Ticks};
use bl_engine::{EngineConfig, MediaMap, Offline, Registry, TransportCommand};
use bl_media::SampleData;
use bl_model::{
    AudioData, BeatsSettings, Clip, ClipData, Command, ComplexProUpdate, ContentHash, Editor,
    Preserve, TextureSettings, TonesSettings, TrackKind, TransientLoopMode, WarpMarker, WarpMode,
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
    play_media(editor, media(), frames, block)
}

fn play_media(editor: &Editor, media: MediaMap, frames: u64, block: usize) -> Vec<f32> {
    let registry = Registry::new().with_media(Arc::new(media));
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

/// The energy centroid of `out` near `around`, in frames: where a click's
/// energy sits. A mode that reports no latency leaves it where Re-Pitch's own,
/// which adds none, is.
fn click_centroid(out: &[f32], around: usize, window: usize) -> f64 {
    let lo = around.saturating_sub(window);
    let hi = (around + window).min(out.len());
    let (mut weight, mut weighted) = (0.0f64, 0.0f64);
    for (i, &s) in out[lo..hi].iter().enumerate() {
        let w = f64::from(s) * f64::from(s);
        weight += w;
        weighted += w * (lo + i) as f64;
    }
    weighted / weight
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
// clip's click lands on the frame its warp map puts it on, within 20 ms (R35's
// timing target), and the mode reports no latency: the click's energy sits on
// the same frame as the same file in Re-Pitch, which adds none, to within a
// couple of frames, since the prime drops the stretcher's own read-ahead. That
// centroid is R24's measure: the frame the mixer must see.
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
    let reference_centroid = click_centroid(&reference, click_at, 24_000);
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
            let late = (click_centroid(&out, click_at, 24_000) - reference_centroid).abs();
            assert!(
                late <= 4.0,
                "{mode:?} at blocks of {block}: its click's energy sits {late:.2} frames from \
                 Re-Pitch's own, so the mode reports latency"
            );
        }
    }
}

// Spec: warp-modes R5, R6. Two of a track's Complex clips that overlap play
// through their own stretchers, neither the other's continuation, so the render
// is the same at any block size: a shared stretcher would interleave the two
// clips and make the output depend on where the block boundaries fell.
#[test]
fn overlapping_complex_clips_do_not_share_a_stretcher() {
    let mut editor = Editor::new();
    editor
        .apply(Command::SetTempo {
            tempo: TempoMap::constant(Bpm::new(120.0).unwrap()),
        })
        .unwrap();
    let audio_track = track(&mut editor, TrackKind::Audio, None);
    // Two whole notes of the same file, the second starting a beat in, so they
    // sound together for a whole note.
    for at in [Ticks::ZERO, Ticks::QUARTER * 2] {
        let audio = AudioData {
            warp: true,
            warp_markers: vec![WarpMarker {
                time: 0.0,
                beat: Ticks::ZERO,
            }],
            tempo: Some(120.0),
            warp_mode: WarpMode::Complex,
            ..AudioData::new(MEDIA)
        };
        let clip = Clip::new(editor.new_id(), at, Ticks::WHOLE, ClipData::Audio(audio));
        editor
            .apply(Command::CreateClip {
                track: audio_track,
                clip,
            })
            .unwrap();
    }
    let reference = play(&editor, 96_000, 512);
    for block in [64, 512, 4_096] {
        assert_eq!(
            play(&editor, 96_000, block),
            reference,
            "overlapping Complex clips, blocks of {block}"
        );
    }
}

// Spec: warp-modes R5, R21. A change to a clip's Formants leaves the track's
// stretcher pool the shape it had, so the list the change sends carries no
// pool and the player keeps the one it has: the clip goes on sounding through
// it rather than falling silent, and the change makes no stretcher again.
#[test]
fn a_formants_change_keeps_the_pool_and_goes_on_sounding() {
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
        warp_mode: WarpMode::ComplexPro,
        ..AudioData::new(MEDIA)
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
    let clip = editor
        .document()
        .tracks()
        .flat_map(|track| track.clips())
        .next()
        .unwrap()
        .id;
    // A control render of the same clip with no change, to compare against.
    let control = {
        let registry = Registry::new().with_media(Arc::new(media()));
        let mut offline = Offline::new(EngineConfig::new(RATE, 512), registry);
        offline.sync(editor.document());
        offline.sender().transport(TransportCommand::Play);
        let mut out = Vec::new();
        offline.render(24_000, |output| out.extend_from_slice(output.channel(0)));
        out
    };
    let registry = Registry::new().with_media(Arc::new(media()));
    let mut offline = Offline::new(EngineConfig::new(RATE, 512), registry);
    offline.sync(editor.document());
    offline.sender().transport(TransportCommand::Play);
    // Play through the pool, then change Formants mid-play.
    let mut before = Vec::new();
    offline.render(12_000, |output| before.extend_from_slice(output.channel(0)));
    editor
        .apply(Command::SetClipWarpSettings {
            clip,
            beats: None,
            tones: None,
            texture: None,
            complex_pro: Some(ComplexProUpdate {
                formants: Some(50.0),
                envelope: None,
            }),
        })
        .unwrap();
    offline.sync(editor.document());
    let mut after = Vec::new();
    offline.render(12_000, |output| after.extend_from_slice(output.channel(0)));
    assert_eq!(offline.heap_operations(), 0);
    assert_eq!(
        before.as_slice(),
        &control[..12_000],
        "the clip before the change"
    );
    // R22: with no pitch change, Formants changes nothing to the bit, so the
    // change is inaudible and the clip goes on exactly as the control render
    // did. That pins that the player kept its pool: a player that dropped it
    // would fall silent here.
    assert_eq!(
        after.as_slice(),
        &control[12_000..],
        "the Formants change altered the sound"
    );
}

// Spec: warp-modes R5, R22, R35. A transposed Complex Pro clip, whose
// formants Beatlayer's own correction holds after the stretcher, plays under
// the allocation trap at small blocks, and the correction changes the sound
// that Formants 0 % leaves as the stretcher made it.
#[test]
fn a_transposed_complex_pro_clip_corrects_formants_under_the_allocation_trap() {
    let render = |formants: f64, block: usize| {
        let mut editor = song(120.0, Ticks::WHOLE, beats(), WarpMode::ComplexPro);
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
                transpose: Some(7),
                detune: None,
            })
            .unwrap();
        editor
            .apply(Command::SetClipWarpSettings {
                clip,
                beats: None,
                tones: None,
                texture: None,
                complex_pro: Some(ComplexProUpdate {
                    formants: Some(formants),
                    envelope: Some(294),
                }),
            })
            .unwrap();
        play(&editor, 48_000, block)
    };
    let corrected = render(100.0, 64);
    assert_eq!(corrected, render(100.0, 512), "the same at any block size");
    assert_ne!(corrected, render(0.0, 64), "the correction changed nothing");
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

// Spec: warp-modes R8. Only a change to a playing clip crossfades: a mode set
// while the song is stopped plays from its first frame at the next start, and
// so does a start after a stop that came inside a crossfade.
#[test]
fn a_mode_change_that_the_song_didnt_play_through_crossfades_nothing() {
    let length = Ticks::WHOLE * 4;
    let (tempo, block, frames) = (60.0, 256_usize, 24_000_u64);
    let want = play(&song(tempo, length, beats(), WarpMode::Beats), frames, block);
    let start = |stop_inside_the_fade: bool| {
        let mut editor = song(tempo, length, beats(), WarpMode::RePitch);
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
        let run_blocks = |offline: &mut Offline, blocks: usize| {
            for _ in 0..blocks {
                offline.process(block);
            }
        };
        if stop_inside_the_fade {
            offline.sender().transport(TransportCommand::Play);
            run_blocks(&mut offline, 4);
        } else {
            run_blocks(&mut offline, 4);
        }
        editor
            .apply(Command::SetClipWarpMode {
                clip,
                mode: WarpMode::Beats,
            })
            .unwrap();
        offline.sync(editor.document());
        if stop_inside_the_fade {
            // Two blocks of the 20 ms fade (960 frames), then a stop, and a
            // second stop to return to the song's start.
            run_blocks(&mut offline, 2);
            offline.sender().transport(TransportCommand::Stop);
            run_blocks(&mut offline, 2);
            offline.sender().transport(TransportCommand::Stop);
        }
        run_blocks(&mut offline, 4);
        offline.sender().transport(TransportCommand::Play);
        let mut out = Vec::new();
        offline.render(frames, |output| out.extend_from_slice(output.channel(0)));
        assert_eq!(offline.heap_operations(), 0);
        out
    };
    assert_eq!(start(false), want, "a mode set while stopped");
    assert_eq!(start(true), want, "a stop inside the crossfade");
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

/// 500 ms segments of `signal`, at 48 kHz for just over two seconds, with a
/// transient at each beat of a 120 BPM clip: the material the joins are
/// pinned on.
fn joins_media(signal: impl Fn(usize) -> f32) -> MediaMap {
    let mut map = MediaMap::new();
    let sample = SampleData::new(48_000, vec![(0..100_000).map(signal).collect()]).unwrap();
    map.insert(MEDIA, Arc::new(sample));
    map.set_transients(MEDIA, vec![0.5, 1.0, 1.5]);
    map
}

/// The joins' clip: Beats at Transients, a whole note long, in a song at
/// `tempo`.
fn joins_song(tempo: f64, loop_mode: TransientLoopMode, envelope: u32) -> Editor {
    let settings = BeatsSettings {
        preserve: Preserve::Transients,
        transient_loop_mode: loop_mode,
        transient_envelope: envelope,
    };
    song(tempo, Ticks::WHOLE, settings, WarpMode::Beats)
}

/// Plays the joins' song at every block size the acceptance names, requires
/// the same samples from each (R6) and returns them.
fn joins(editor: &Editor, signal: impl Fn(usize) -> f32 + Copy, frames: u64) -> Vec<f32> {
    let want = play_media(editor, joins_media(signal), frames, 512);
    for block in [1, 64, 4_096] {
        assert_eq!(
            play_media(editor, joins_media(signal), frames, block),
            want,
            "blocks of {block}"
        );
    }
    want
}

/// The file frame a ramp file's output frame plays.
fn ramp(i: usize) -> f32 {
    i as f32 / 65_536.0
}

fn played(out: &[f32], frame: usize) -> f64 {
    f64::from(out[frame]) * 65_536.0
}

fn near(got: f32, want: f64, frame: usize) {
    assert!(
        (f64::from(got) - want).abs() < 1e-3,
        "frame {frame}: {got} against {want}"
    );
}

// Spec: warp-modes R15. Played at half speed with Loop Off, a segment fades
// out linearly over 10 ms, ending 2 ms before the file frame the next would
// begin at, then is silent; the next fades in over 10 ms, ending 2 ms before
// its transient. No step anywhere.
#[test]
fn loop_off_fades_out_and_the_next_segment_fades_in() {
    let out = joins(
        &joins_song(60.0, TransientLoopMode::LoopOff, 100),
        |_| 1.0,
        49_000,
    );
    // Segment 0 sounds 23,904 frames: its 24,000 less 2 ms.
    assert_eq!(out[23_423], 1.0);
    near(out[23_424 + 240], 0.5, 23_664);
    assert_eq!(out[23_904], 0.0);
    assert_eq!(out[45_000], 0.0);
    // Segment 1 begins 12 ms before its transient, at song frame 48,000.
    assert_eq!(out[47_423], 0.0);
    near(out[47_424 + 240], 0.5, 47_664);
    assert_eq!(out[47_904], 1.0);
    for pair in out.windows(2) {
        assert!((pair[1] - pair[0]).abs() < 0.01, "no step: {pair:?}");
    }
}

// Spec: warp-modes R15. Loop Forward and Back-and-Forth turn about 22 ms
// before the next segment's file frame, in a 10 ms crossfade centred on the
// turn: Back-and-Forth reads backwards to the middle of what lies before it,
// Loop Forward jumps back to it. The file here is a ramp, so each output
// frame says which file frame it plays.
#[test]
fn loop_turns_crossfade_around_the_end_and_the_middle() {
    // Segment 0 plays file frames 0 to 24,000; it turns at 22,944, and the
    // middle of what lies before that is 11,472.
    let forth = joins(
        &joins_song(60.0, TransientLoopMode::LoopBackAndForth, 100),
        ramp,
        46_000,
    );
    assert_eq!(played(&forth, 22_000), 22_000.0);
    assert!(
        (played(&forth, 22_944) - 22_944.0).abs() < 2.0,
        "both at the turn"
    );
    assert!(
        (played(&forth, 23_500) - 22_388.0).abs() < 2.0,
        "backwards from it"
    );
    // The next turn is a pass later (11,472 frames), at the middle.
    assert!(
        (played(&forth, 34_416) - 11_472.0).abs() < 2.0,
        "both at the middle"
    );
    assert!(
        (played(&forth, 34_916) - 11_972.0).abs() < 2.0,
        "and forwards from it"
    );
    let forward = joins(
        &joins_song(60.0, TransientLoopMode::LoopForward, 100),
        ramp,
        46_000,
    );
    assert_eq!(played(&forward, 22_000), 22_000.0);
    assert!(
        (played(&forward, 22_944) - (22_944.0 + 11_472.0) / 2.0).abs() < 2.0,
        "a crossfade"
    );
    assert!(
        (played(&forward, 23_444) - 11_972.0).abs() < 2.0,
        "forwards from the middle"
    );
}

// Spec: warp-modes R15. A song faster than the clip cuts a segment short with
// a 10 ms crossfade into the next, which began 12 ms before its transient.
#[test]
fn a_segment_cut_short_crossfades_into_the_next() {
    let out = joins(
        &joins_song(180.0, TransientLoopMode::LoopBackAndForth, 100),
        ramp,
        16_500,
    );
    // The next segment's transient is at song frame 16,000, file frame
    // 24,000; it begins 576 frames earlier.
    assert_eq!(played(&out, 15_423), 15_423.0);
    let middle = 15_424 + 240;
    let want = (middle as f64 + (24_000.0 - 576.0 + 240.0)) / 2.0;
    assert!((played(&out, middle) - want).abs() < 2.0);
    assert!((played(&out, 15_424 + 480) - (24_000.0 - 96.0)).abs() < 2.0);
}

// Spec: warp-modes R16. At Transient Envelope 50 each segment decays from its
// transient, half at about 65 ms, and the next fades in over 2 ms only.
#[test]
fn a_lowered_transient_envelope_decays_and_fades_in_briefly() {
    let out = joins(
        &joins_song(60.0, TransientLoopMode::LoopOff, 50),
        |_| 1.0,
        49_000,
    );
    near(out[3_128], 0.5, 3_128);
    near(out[6_256], 0.25, 6_256);
    // The next fade in is 96 frames, ending 34 frames before the transient.
    assert_eq!(out[47_869], 0.0);
    near(out[47_870 + 48], 0.5, 47_918);
    assert_eq!(out[47_966], 1.0);
}

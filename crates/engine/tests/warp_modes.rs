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
    AudioData, BeatsSettings, Clip, ClipData, Command, ContentHash, Editor, Preserve, TrackKind,
    TransientLoopMode, WarpMarker, WarpMode,
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
            for block in [1, 61, 4_096] {
                assert_eq!(
                    play(&editor, 96_000, block),
                    want,
                    "{preserve:?} {transient_loop_mode:?}, blocks of {block}"
                );
            }
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

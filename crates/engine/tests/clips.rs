//! Audio clip playback (the audio clips spec): clips placed through
//! the tempo map, their markers, loop, gain, and fades, sample-exact at any
//! block size, edited without a recompile, and their media freed off the
//! audio thread. Every block runs as a real-time section under the
//! allocation trap.

bl_rt::install_trap!();

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bl_core::time::{Bpm, Frames, SampleRate, TempoMap, Ticks};
use bl_engine::{
    EngineConfig, MediaLibrary, MediaMap, Offline, OpenedSource, Registry, TransportCommand, When,
};
use bl_media::{MediaStore, MemorySource, SampleData, convert_sample_rate};
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, Editor, TrackId, TrackKind,
};
use common::{RATE, Rig, track};

/// A step between neighbouring samples of a ramp: 2⁻¹⁶, exact in `f32`.
const STEP: f32 = 1.0 / 65_536.0;

/// A file whose every sample says where it is: frame `i` is `(i + 1)` steps
/// on the left and minus that on the right, so a render shows exactly which
/// frame of the file each frame of the song played.
fn ramp(frames: usize, rate: u32) -> Arc<SampleData> {
    let left: Vec<f32> = (0..frames).map(|i| (i + 1) as f32 * STEP).collect();
    let right = left.iter().map(|s| -s).collect();
    Arc::new(SampleData::new(rate, vec![left, right]).unwrap())
}

/// The left-channel sample a ramp plays at its frame `i`.
fn at(i: i64) -> f32 {
    (i + 1) as f32 * STEP
}

fn hash(n: u8) -> ContentHash {
    ContentHash([n; 32])
}

/// Puts a clip on `track`.
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

/// Plays `editor`'s song from 1.1.1 (and through any `cues`, each on its
/// frame) for `frames` frames in blocks of `block`, with clips' audio from
/// `media`, and returns the left and right channels.
fn play(
    editor: &Editor,
    media: &MediaMap,
    frames: u64,
    block: usize,
    cues: &[(u64, TransportCommand)],
) -> [Vec<f32>; 2] {
    let registry = Registry::new().with_media(Arc::new(media.clone()));
    let mut offline = Offline::new(EngineConfig::new(RATE, block), registry);
    offline.sync(editor.document());
    assert!(offline.missing_media().is_empty());
    offline.sender().transport(TransportCommand::Play);
    for &(frame, command) in cues {
        offline.sender().transport_at(command, When::Frame(frame));
    }
    let mut out = [Vec::new(), Vec::new()];
    offline.render(frames, |output| {
        for (c, channel) in out.iter_mut().enumerate() {
            channel.extend_from_slice(output.channel(c));
        }
    });
    assert_eq!(offline.heap_operations(), 0);
    out
}

fn frames_at(tempo: &TempoMap, ticks: Ticks) -> usize {
    usize::try_from(tempo.ticks_to_frames(ticks, RATE).get()).unwrap()
}

// Spec: audio-clips R16. A clip split in two, or trimmed at its left edge
// (WP-1.8b), keeps its contents where they were: its Start marker moves on
// by the time its start did, as Live's does, so the parts together play
// exactly what the whole clip played.
#[test]
fn a_split_clips_parts_play_on_from_where_the_whole_clip_was() {
    let tempo = TempoMap::default();
    let start = Ticks::QUARTER + Ticks(12_345);
    let length = Ticks::WHOLE;
    let split = start + Ticks::QUARTER + Ticks(999);
    let mut media = MediaMap::new();
    media.insert(hash(1), ramp(96_000, 48_000));
    let audio = AudioData {
        start: 0.25,
        ..AudioData::new(hash(1))
    };

    let mut whole = Editor::new();
    let a = track(&mut whole, TrackKind::Audio, None);
    clip(&mut whole, a, start, length, audio.clone());
    let expected = play(&whole, &media, 96_000, 512, &[]);

    let mut parts = Editor::new();
    let b = track(&mut parts, TrackKind::Audio, None);
    clip(&mut parts, b, start, split - start, audio.clone());
    let whole = Clip::new(parts.new_id(), start, length, ClipData::Audio(audio));
    let piece = Clip {
        id: parts.new_id(),
        ..whole.part(split, whole.end(), &tempo)
    };
    parts
        .apply(Command::CreateClip {
            track: b,
            clip: piece,
        })
        .unwrap();
    let [left, _] = play(&parts, &media, 96_000, 512, &[]);
    assert_eq!(left, expected[0]);
    // The right part on its own plays from the split, a quarter and a bit
    // past the Start marker.
    let first = frames_at(&tempo, split);
    let into = first - frames_at(&tempo, start) + 12_000;
    assert_eq!(left[first], at(into as i64));
}

// Spec: audio-clips R16; arrangement-editing R7, R8, R16. Under a tempo
// ramp, at a rate whose frames don't fall on ticks, a clip split anywhere
// or trimmed at its left edge plays, at every frame, exactly what the whole
// clip played there, looped or not: its Start marker moved by the time its
// start did, as Live's does.
#[test]
fn trimmed_and_split_clips_play_exactly_what_the_whole_clip_did() {
    let tempo = TempoMap::from_breakpoints([
        (Ticks::ZERO, Bpm::new(97.3).unwrap()),
        (Ticks::WHOLE * 2, Bpm::new(141.1).unwrap()),
    ])
    .unwrap();
    let rate = SampleRate::HZ_44100;
    let mut media = MediaMap::new();
    media.insert(hash(1), ramp(400_000, 48_000));
    let render = |editor: &Editor| {
        let registry = Registry::new().with_media(Arc::new(media.clone()));
        let mut offline = Offline::new(EngineConfig::new(rate, 300), registry);
        offline.sync(editor.document());
        offline.sender().transport(TransportCommand::Play);
        let mut left = Vec::new();
        offline.render(350_000, |output| left.extend_from_slice(output.channel(0)));
        assert_eq!(offline.heap_operations(), 0);
        left
    };
    let looped = AudioData {
        start: 0.0123,
        looping: true,
        loop_start: 0.5,
        loop_end: Some(0.81),
        ..AudioData::new(hash(1))
    };
    let unlooped = AudioData {
        start: 0.0371,
        end: Some(5.0),
        ..AudioData::new(hash(1))
    };
    let start = Ticks::QUARTER + Ticks(3_141);
    let length = Ticks::WHOLE * 2 + Ticks(2_718);
    for (round, audio) in [looped, unlooped].into_iter().enumerate() {
        let song = |edit: &dyn Fn(&mut Editor, TrackId, ClipId)| {
            let mut editor = Editor::new();
            editor
                .apply(Command::SetTempo {
                    tempo: tempo.clone(),
                })
                .unwrap();
            let a = track(&mut editor, TrackKind::Audio, None);
            let id = clip(&mut editor, a, start, length, audio.clone());
            edit(&mut editor, a, id);
            editor
        };
        let expected = render(&song(&|_, _, _| {}));
        assert!(expected.iter().any(|&s| s != 0.0));
        for i in 1..6_i64 {
            let at = start + Ticks(length.get() * i / 6 + 977 * i);
            // Split at `at`: the left part keeps the start, the right part
            // is the whole clip's part from there.
            let split = render(&song(&|editor, a, id| {
                let whole = editor.document().clip(id).unwrap().clone();
                let right = Clip {
                    id: editor.new_id(),
                    ..whole.part(at, whole.end(), &tempo)
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
            }));
            assert_eq!(split, expected, "{round}: split at {at:?}");
            // Trimmed from the left to `at`, then the part before added
            // back as a clip of its own.
            let trimmed = render(&song(&|editor, a, id| {
                let whole = editor.document().clip(id).unwrap().clone();
                editor
                    .apply(Command::TrimClipStart {
                        clip: id,
                        start: at,
                    })
                    .unwrap();
                let left = Clip {
                    id: editor.new_id(),
                    ..whole.part(start, at, &tempo)
                };
                editor
                    .apply(Command::CreateClip {
                        track: a,
                        clip: left,
                    })
                    .unwrap();
            }));
            assert_eq!(trimmed, expected, "{round}: trimmed to {at:?}");
        }
    }
}

// Spec: audio-clips R1, R2, R5
#[test]
fn a_clip_plays_its_file_from_the_frame_its_start_falls_on_to_the_frame_its_end_does() {
    let mut editor = Editor::new();
    let a = track(&mut editor, TrackKind::Audio, None);
    // Off the grid, so the start falls between frames.
    let start = Ticks::QUARTER + Ticks(12_345);
    let length = Ticks::QUARTER * 2 + Ticks(777);
    clip(&mut editor, a, start, length, AudioData::new(hash(1)));
    let mut media = MediaMap::new();
    media.insert(hash(1), ramp(96_000, 48_000));

    let [left, right] = play(&editor, &media, 96_000, 512, &[]);
    let tempo = TempoMap::default();
    let (first, end) = (frames_at(&tempo, start), frames_at(&tempo, start + length));
    assert_eq!((first, end), (24_420, 72_446));
    assert!(left[..first].iter().all(|&s| s == 0.0));
    // At 0 dB, through mixers at rest, the file's samples arrive unchanged.
    for f in first..end {
        let i = (f - first) as i64;
        assert_eq!(left[f], at(i), "frame {f}");
        assert_eq!(right[f], -at(i), "frame {f}");
    }
    assert!(left[end..].iter().all(|&s| s == 0.0));
}

// Spec: audio-clips R2
#[test]
fn across_a_tempo_change_the_audio_runs_on_and_the_end_follows_the_map() {
    let mut editor = Editor::new();
    let tempo = TempoMap::from_breakpoints([
        (Ticks::ZERO, Bpm::new(120.0).unwrap()),
        (Ticks::QUARTER * 2, Bpm::new(120.0).unwrap()),
        (Ticks::QUARTER * 4, Bpm::new(60.0).unwrap()),
    ])
    .unwrap();
    editor
        .apply(Command::SetTempo {
            tempo: tempo.clone(),
        })
        .unwrap();
    let a = track(&mut editor, TrackKind::Audio, None);
    let start = Ticks::QUARTER;
    let length = Ticks::QUARTER * 4;
    clip(&mut editor, a, start, length, AudioData::new(hash(1)));
    let mut media = MediaMap::new();
    media.insert(hash(1), ramp(200_000, 48_000));

    let [left, _] = play(&editor, &media, 200_000, 333, &[]);
    let (first, end) = (frames_at(&tempo, start), frames_at(&tempo, start + length));
    // A beat at 120 BPM, then a ramp: the end is later than at a steady 120.
    assert_eq!(first, 24_000);
    assert!(end > first + 96_000, "{end}");
    for (i, &sample) in left[first..end].iter().enumerate() {
        assert_eq!(sample, at(i as i64), "frame {}", first + i);
    }
    assert!(left[end..].iter().all(|&s| s == 0.0));
}

// Spec: audio-clips R3
#[test]
fn the_start_and_end_markers_choose_what_plays_and_silence_follows_the_end() {
    let mut editor = Editor::new();
    let a = track(&mut editor, TrackKind::Audio, None);
    // From 0.25 s into the file to 0.5 s, in a clip a second long.
    let audio = AudioData {
        start: 0.25,
        end: Some(0.5),
        ..AudioData::new(hash(1))
    };
    clip(&mut editor, a, Ticks::ZERO, Ticks::QUARTER * 2, audio);
    // A file shorter than its clip: silent past its end.
    let b = track(&mut editor, TrackKind::Audio, None);
    clip(
        &mut editor,
        b,
        Ticks::ZERO,
        Ticks::QUARTER * 2,
        AudioData::new(hash(2)),
    );
    let mut media = MediaMap::new();
    media.insert(hash(1), ramp(96_000, 48_000));
    media.insert(
        hash(2),
        Arc::new(SampleData::new(48_000, vec![vec![-1.0; 100]]).unwrap()),
    );

    let [left, _] = play(&editor, &media, 48_000, 256, &[]);
    for (f, &sample) in left[..12_000].iter().enumerate() {
        let short = if f < 100 { -1.0 } else { 0.0 };
        assert_eq!(sample, at(12_000 + f as i64) + short, "frame {f}");
    }
    assert!(left[12_000..].iter().all(|&s| s == 0.0));
}

// Spec: audio-clips R4
#[test]
fn a_looped_clip_plays_into_its_brace_and_repeats_it_on_the_exact_frame() {
    let mut editor = Editor::new();
    let a = track(&mut editor, TrackKind::Audio, None);
    // Start at 0.1 s, loop from 0.2 s to 0.3 s, for a second.
    let audio = AudioData {
        start: 0.1,
        looping: true,
        loop_start: 0.2,
        loop_end: Some(0.3),
        ..AudioData::new(hash(1))
    };
    clip(&mut editor, a, Ticks::ZERO, Ticks::QUARTER * 2, audio);
    let mut media = MediaMap::new();
    media.insert(hash(1), ramp(48_000, 48_000));

    let [left, _] = play(&editor, &media, 48_000, 100, &[]);
    for (f, &sample) in left.iter().enumerate() {
        let elapsed = f as i64;
        let file = if elapsed < 9_600 {
            4_800 + elapsed
        } else {
            9_600 + (elapsed - 9_600) % 4_800
        };
        assert_eq!(sample, at(file), "frame {f}");
    }
    // With the loop off, the same clip plays on past the brace.
    let clip_id = editor
        .document()
        .track(a)
        .unwrap()
        .clips()
        .next()
        .unwrap()
        .id;
    editor
        .apply(Command::SetClipLoopEnabled {
            clip: clip_id,
            enabled: false,
        })
        .unwrap();
    let [left, _] = play(&editor, &media, 48_000, 100, &[]);
    assert_eq!(left[20_000], at(24_800));
}

// Spec: audio-clips R5, R6
#[test]
fn gain_and_fades_scale_the_clip_from_its_edges() {
    let mut editor = Editor::new();
    let a = track(&mut editor, TrackKind::Audio, None);
    // A steady level of 0.5, at -6 dB, with a 100-frame fade in and a
    // 200-frame fade out, starting mid-file.
    let audio = AudioData {
        start: 0.5,
        gain: 0.5,
        fade_in: 100.0 / 48_000.0,
        fade_out: 200.0 / 48_000.0,
        ..AudioData::new(hash(1))
    };
    let start = Ticks::QUARTER / 4;
    clip(&mut editor, a, start, Ticks::QUARTER, audio);
    let mut media = MediaMap::new();
    media.insert(
        hash(1),
        Arc::new(SampleData::new(48_000, vec![vec![0.5; 96_000]]).unwrap()),
    );

    let [left, right] = play(&editor, &media, 48_000, 64, &[]);
    let (first, end) = (6_000, 30_000);
    assert_eq!(left, right, "a mono file plays on both channels");
    assert!(left[..first].iter().all(|&s| s == 0.0));
    for e in 0..(end - first) {
        let fade_in = (e as f64 / 100.0).min(1.0);
        let fade_out = ((end - first - e) as f64 / 200.0).min(1.0);
        let expected = (0.5 * 0.5 * fade_in.min(fade_out)) as f32;
        assert_eq!(left[first + e], expected, "{e} frames in");
    }
    assert_eq!(left[first], 0.0, "the first frame is silent");
    assert_eq!(left[first + 50], 0.125);
    assert_eq!(left[end - 1], (0.25f64 / 200.0) as f32);
    assert!(left[end..].iter().all(|&s| s == 0.0));
}

// Spec: audio-clips R1, R8
#[test]
fn deactivated_clips_are_silent_and_overlapping_ones_add() {
    let mut editor = Editor::new();
    let a = track(&mut editor, TrackKind::Audio, None);
    clip(
        &mut editor,
        a,
        Ticks::ZERO,
        Ticks::QUARTER,
        AudioData::new(hash(1)),
    );
    clip(
        &mut editor,
        a,
        Ticks::QUARTER / 2,
        Ticks::QUARTER,
        AudioData::new(hash(2)),
    );
    let muted = clip(
        &mut editor,
        a,
        Ticks::ZERO,
        Ticks::QUARTER * 4,
        AudioData::new(hash(2)),
    );
    editor
        .apply(Command::SetClipMute {
            clip: muted,
            mute: true,
        })
        .unwrap();
    let mut media = MediaMap::new();
    media.insert(
        hash(1),
        Arc::new(SampleData::new(48_000, vec![vec![0.25; 48_000]]).unwrap()),
    );
    media.insert(
        hash(2),
        Arc::new(SampleData::new(48_000, vec![vec![0.5; 48_000]]).unwrap()),
    );

    let [left, _] = play(&editor, &media, 48_000, 512, &[]);
    assert!(left[..12_000].iter().all(|&s| s == 0.25));
    assert!(left[12_000..24_000].iter().all(|&s| s == 0.75));
    assert!(left[24_000..36_000].iter().all(|&s| s == 0.5));
    assert!(left[36_000..].iter().all(|&s| s == 0.0));
}

// Spec: audio-clips R10, R20, R21
#[test]
fn a_locate_or_a_loop_wrap_into_a_clip_plays_its_file_from_that_point() {
    let mut editor = Editor::new();
    let a = track(&mut editor, TrackKind::Audio, None);
    clip(
        &mut editor,
        a,
        Ticks::ZERO,
        Ticks::WHOLE,
        AudioData::new(hash(1)),
    );
    // The Arrangement loop from beat 2 to beat 3.
    editor
        .apply(Command::SetLoop {
            start: Ticks::QUARTER,
            length: Ticks::QUARTER,
        })
        .unwrap();
    editor
        .apply(Command::SetLoopEnabled { enabled: true })
        .unwrap();
    let mut media = MediaMap::new();
    media.insert(hash(1), ramp(96_000, 48_000));

    // Jump to beat 1.5 at frame 1,000, then play into the loop.
    let cues = [(1_000, TransportCommand::Locate(Ticks::QUARTER / 2))];
    let [left, _] = play(&editor, &media, 72_000, 128, &cues);
    assert_eq!(left[999], at(999));
    assert_eq!(left[1_000], at(12_000));
    // From frame 1,000 at song frame 12,000, the loop's end (48,000) comes
    // 36,000 frames later, and playback goes on from its start (24,000).
    assert_eq!(left[36_999], at(47_999));
    assert_eq!(left[37_000], at(24_000));
    assert_eq!(left[60_999], at(47_999));
    assert_eq!(left[61_000], at(24_000));
}

// Spec: audio-clips R11
#[test]
fn the_block_size_changes_nothing() {
    let mut editor = Editor::new();
    let a = track(&mut editor, TrackKind::Audio, None);
    let audio = AudioData {
        start: 0.013,
        looping: true,
        loop_start: 0.05,
        loop_end: Some(0.0731),
        gain: 0.7,
        ..AudioData::new(hash(1)).with_edge_fades()
    };
    clip(&mut editor, a, Ticks(98_765), Ticks::QUARTER * 3, audio);
    let b = track(&mut editor, TrackKind::Audio, None);
    clip(
        &mut editor,
        b,
        Ticks(4_321),
        Ticks::QUARTER,
        AudioData::new(hash(2)),
    );
    let mut media = MediaMap::new();
    media.insert(hash(1), ramp(48_000, 48_000));
    media.insert(hash(2), ramp(48_000, 44_100));
    let reference = play(&editor, &media, 96_000, 512, &[]);
    assert!(reference[0].iter().any(|&s| s != 0.0));
    for block in [1, 61, 4096] {
        let other = play(&editor, &media, 96_000, block, &[]);
        let bits =
            |x: &[Vec<f32>; 2]| -> Vec<u32> { x.iter().flatten().map(|s| s.to_bits()).collect() };
        assert_eq!(bits(&reference), bits(&other), "blocks of {block}");
    }
}

// Spec: audio-clips R12
#[test]
fn clip_edits_reach_the_player_without_a_program() {
    let mut media = MediaMap::new();
    media.insert(
        hash(1),
        Arc::new(SampleData::new(48_000, vec![vec![0.5; 240_000]]).unwrap()),
    );
    let media = Arc::new(media);
    let mut rig = Rig::with_registry(64, |registry| registry.set_media(media.clone()));
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    rig.block(64);
    assert!(rig.last().iter().all(|&s| s == 0.0));

    // A new clip, a gain, a move, and a deletion: no program, only data.
    let id = clip(
        &mut rig.editor,
        a,
        Ticks::ZERO,
        Ticks::WHOLE,
        AudioData::new(hash(1)),
    );
    let synced = rig.sync();
    assert!(!synced.program);
    rig.block(64);
    assert!(rig.last().iter().all(|&s| s == 0.5));
    let synced = rig.apply(Command::SetClipGain {
        clip: id,
        gain: 0.5,
    });
    assert!(!synced.program);
    rig.block(64);
    assert!(rig.last().iter().all(|&s| s == 0.25));
    rig.apply(Command::MoveClip {
        clip: id,
        track: a,
        start: Ticks::WHOLE,
    });
    rig.block(64);
    assert!(rig.last().iter().all(|&s| s == 0.0));
    rig.apply(Command::MoveClip {
        clip: id,
        track: a,
        start: Ticks::ZERO,
    });
    rig.block(64);
    assert!(rig.last().iter().all(|&s| s == 0.25));
    assert_eq!(rig.monitor.state().swaps, 1);

    // A tempo change recompiles, and the player migrates with its clips,
    // which are placed again under the new map: at 60 BPM the bar-long clip
    // lasts four seconds, to frame 192,000, where at 120 BPM it ended at
    // frame 96,000.
    let synced = rig.apply(Command::SetTempo {
        tempo: TempoMap::constant(Bpm::new(60.0).unwrap()),
    });
    assert!(synced.program);
    rig.sender
        .transport(TransportCommand::Locate(Ticks::QUARTER * 5 / 2));
    rig.block(64);
    let state = rig.monitor.state();
    assert_eq!(state.swaps, 2);
    assert_eq!(state.position, Frames(120_000 + 64));
    assert!(rig.last().iter().all(|&s| s == 0.25));
    rig.sender
        .transport(TransportCommand::Locate(Ticks::WHOLE + Ticks::QUARTER));
    rig.block(64);
    assert!(rig.last().iter().all(|&s| s == 0.0), "past the clip's end");

    // Deleting the clip leaves the player with nothing.
    let synced = rig.apply(Command::DeleteClip { clip: id });
    assert!(!synced.program);
    rig.block(64);
    assert!(rig.last().iter().all(|&s| s == 0.0));
    assert_eq!(rig.monitor.state().counters.data_unrouted, 0);
}

/// Audio that counts its drops.
struct Counted {
    data: SampleData,
    dropped: Arc<AtomicUsize>,
}

impl AsRef<SampleData> for Counted {
    fn as_ref(&self) -> &SampleData {
        &self.data
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

/// A library that makes a new source over new [`Counted`] audio each time
/// it's asked.
struct Counting(Arc<AtomicUsize>);

impl MediaLibrary for Counting {
    fn open(&self, _: ContentHash, _: SampleRate, _: u64) -> Option<OpenedSource> {
        let counted = Counted {
            data: SampleData::new(48_000, vec![vec![0.5; 4_800]]).unwrap(),
            dropped: Arc::clone(&self.0),
        };
        Some(OpenedSource {
            source: Box::new(CountedSource {
                inner: MemorySource::new(Arc::new(counted.data.clone())),
                _counted: counted,
            }),
            cues: None,
        })
    }
}

/// A source in memory that holds a [`Counted`], so dropping the source
/// counts.
struct CountedSource {
    inner: MemorySource,
    _counted: Counted,
}

impl bl_media::AudioSource for CountedSource {
    fn channels(&self) -> usize {
        self.inner.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn frames(&self) -> Option<u64> {
        self.inner.frames()
    }
    fn position(&self) -> u64 {
        self.inner.position()
    }
    fn read(
        &mut self,
        position: u64,
        out: &mut bl_media::PlanarMut<'_>,
        transition: bl_media::Transition,
    ) -> bl_media::Filled {
        self.inner.read(position, out, transition)
    }
    fn seek(&mut self, position: u64) {
        self.inner.seek(position);
    }
    fn stop(&mut self) {
        self.inner.stop();
    }
    fn buffered(&mut self) -> u64 {
        self.inner.buffered()
    }
    fn stats(&self) -> bl_media::StreamStats {
        self.inner.stats()
    }
}

// Spec: audio-clips R13, R18
#[test]
fn the_audio_thread_never_frees_a_clips_media() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let library = Arc::new(Counting(Arc::clone(&dropped)));
    let mut rig = Rig::with_registry(64, |registry| registry.set_media(library));
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    let id = clip(
        &mut rig.editor,
        a,
        Ticks::ZERO,
        Ticks::WHOLE,
        AudioData::new(hash(1)),
    );
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    rig.block(64);
    assert!(rig.last().iter().all(|&s| s == 0.5));

    // A new list replaces the first, and the player carries the clip's
    // source across to it: nothing is opened or let go of.
    rig.apply(Command::SetClipGain {
        clip: id,
        gain: 0.5,
    });
    rig.block(64);
    assert_eq!(rig.last()[0], 0.25);
    rig.collector.collect();
    assert_eq!(dropped.load(Ordering::SeqCst), 0);

    // Deleting the clip lets go of its source inside a block that the
    // allocation trap watches; only the collector frees it.
    rig.apply(Command::DeleteClip { clip: id });
    rig.block(64);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    rig.collector.collect();
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    // So does moving a clip to another track, whose player opens one of its
    // own, and so does deleting the track, whose program goes with the
    // player in it.
    let id = clip(
        &mut rig.editor,
        a,
        Ticks::ZERO,
        Ticks::WHOLE,
        AudioData::new(hash(1)),
    );
    let b = track(&mut rig.editor, TrackKind::Audio, None);
    rig.sync();
    rig.block(64);
    rig.apply(Command::MoveClip {
        clip: id,
        track: b,
        start: Ticks::ZERO,
    });
    rig.block(64);
    assert!(rig.last().iter().all(|&s| s == 0.5));
    rig.collector.collect();
    assert_eq!(dropped.load(Ordering::SeqCst), 2);
    rig.apply(Command::DeleteTrack { track: b });
    rig.block(64);
    rig.collector.collect();
    // The old program is gone, but the player's list goes on the next pass.
    rig.collector.collect();
    assert_eq!(dropped.load(Ordering::SeqCst), 3);
}

// Spec: audio-clips R14
#[test]
fn a_clip_whose_media_is_missing_plays_silence_and_is_reported() {
    let mut editor = Editor::new();
    let a = track(&mut editor, TrackKind::Audio, None);
    clip(
        &mut editor,
        a,
        Ticks::ZERO,
        Ticks::WHOLE,
        AudioData::new(hash(1)),
    );
    clip(
        &mut editor,
        a,
        Ticks::ZERO,
        Ticks::WHOLE,
        AudioData::new(hash(9)),
    );
    let mut media = MediaMap::new();
    media.insert(
        hash(1),
        Arc::new(SampleData::new(48_000, vec![vec![0.5; 4_800]]).unwrap()),
    );
    let registry = Registry::new().with_media(Arc::new(media));
    let mut offline = Offline::new(EngineConfig::new(RATE, 256), registry);
    offline.sync(editor.document());
    assert_eq!(offline.missing_media(), &[hash(9)]);
    offline.sender().transport(TransportCommand::Play);
    assert!(offline.process(256).channel(0).iter().all(|&s| s == 0.5));

    // Without a media library, every clip is missing.
    let mut offline = Offline::new(EngineConfig::new(RATE, 256), Registry::new());
    offline.sync(editor.document());
    assert_eq!(offline.missing_media(), &[hash(1), hash(9)]);
    offline.sender().transport(TransportCommand::Play);
    assert!(offline.process(256).channel(0).iter().all(|&s| s == 0.0));
}

/// A folder in the system's temporary folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-engine-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a 32-bit float WAV file of `channels`.
fn write_wav(path: &std::path::Path, rate: u32, channels: &[Vec<f32>]) {
    let mut bytes = Vec::new();
    let frames = channels[0].len();
    let block_align = 4 * channels.len() as u16;
    let data_len = (frames * usize::from(block_align)) as u32;
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&3u16.to_le_bytes());
    bytes.extend_from_slice(&(channels.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * u32::from(block_align)).to_le_bytes());
    bytes.extend_from_slice(&block_align.to_le_bytes());
    bytes.extend_from_slice(&32u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for frame in 0..frames {
        for channel in channels {
            bytes.extend_from_slice(&channel[frame].to_le_bytes());
        }
    }
    std::fs::write(path, bytes).unwrap();
}

// Spec: audio-clips R2, R14
#[test]
fn the_media_store_supplies_a_file_at_the_engines_rate() {
    let temp = TempDir::new("media-store");
    let path = temp.0.join("ramp.wav");
    let file = ramp(44_100, 44_100);
    write_wav(
        &path,
        44_100,
        &file.channels().map(<[f32]>::to_vec).collect::<Vec<_>>(),
    );
    let store = Arc::new(MediaStore::new());
    let imported = store.import(&path).unwrap();
    let media = ContentHash(*imported.as_bytes());

    let mut editor = Editor::new();
    let a = track(&mut editor, TrackKind::Audio, None);
    clip(
        &mut editor,
        a,
        Ticks::ZERO,
        Ticks::WHOLE,
        AudioData::new(media),
    );
    let registry = Registry::new().with_media(store.clone());
    let mut offline = Offline::new(EngineConfig::new(RATE, 512), registry);
    offline.sync(editor.document());
    assert!(offline.missing_media().is_empty());
    offline.sender().transport(TransportCommand::Play);
    let mut left = Vec::new();
    offline.render(48_000, |output| left.extend_from_slice(output.channel(0)));

    // A second of the file at 44.1 kHz plays as its second at 48 kHz.
    let converted = convert_sample_rate(&file, 48_000).unwrap();
    assert_eq!(converted.frames(), 48_000);
    assert_eq!(left, converted.channel(0).unwrap());

    // A set that names the file, opened after the file changed: the store
    // checks the file's content, and the media is missing.
    write_wav(&path, 44_100, &[vec![0.0; 10]]);
    let reopened = Arc::new(MediaStore::new());
    reopened.add_reference(imported, &path, 0);
    let mut offline = Offline::new(
        EngineConfig::new(RATE, 512),
        Registry::new().with_media(reopened),
    );
    offline.sync(editor.document());
    assert_eq!(offline.missing_media(), &[media]);
}

// Spec: warp-modes R27. With Warp off, Transpose and Detune change the
// clip's speed and its pitch together, as a tape running faster does: the
// file is read at the pitch ratio, so a clip transposed up an octave runs
// through its file twice as fast and sounds an octave up.
#[test]
fn an_unwarped_clip_transposed_changes_speed_and_pitch() {
    let mut editor = Editor::new();
    let track = track(&mut editor, TrackKind::Audio, None);
    // A ramp: frame `i` says where it is, so a render shows which frame of
    // the file each frame of the song played.
    let mut media = MediaMap::new();
    media.insert(hash(3), ramp(4_096, RATE.hz()));
    let mut audio = AudioData::new(hash(3));
    audio.transpose = 12;
    let clip = clip(&mut editor, track, Ticks::ZERO, Ticks::WHOLE, audio);
    let [left, _] = play(&editor, &media, 512, 64, &[]);
    // Twice as fast: the song's frame `i` plays the file's frame `2 * i`.
    for (i, &sample) in left.iter().enumerate() {
        assert_eq!(sample, at(2 * i as i64), "frame {i}");
    }
    // Untransposed, the file is read frame for frame.
    editor
        .apply(Command::SetClipPitch {
            clip,
            transpose: Some(0),
            detune: Some(0.0),
        })
        .unwrap();
    let [left, _] = play(&editor, &media, 512, 64, &[]);
    for (i, &sample) in left.iter().enumerate() {
        assert_eq!(sample, at(i as i64), "frame {i}");
    }
}

// Spec: warp-modes R12, R27. The interpolator reads the file where it is
// read at a rate other than 1: HiQ on is the band-limited one.
#[test]
fn an_unwarped_clip_transposed_reads_through_the_interpolator() {
    // A sine of 440 Hz, so a transposition up an octave doubles its pitch.
    let rate = RATE.hz();
    let left: Vec<f32> = (0..8_192)
        .map(|i| (2.0 * std::f64::consts::PI * 440.0 * f64::from(i) / f64::from(rate)).sin() as f32)
        .collect();
    let mut media = MediaMap::new();
    media.insert(
        hash(4),
        Arc::new(SampleData::new(rate, vec![left]).unwrap()),
    );
    let mut editor = Editor::new();
    let track = track(&mut editor, TrackKind::Audio, None);
    let mut audio = AudioData::new(hash(4));
    audio.transpose = 12;
    clip(&mut editor, track, Ticks::ZERO, Ticks::WHOLE, audio);
    let [played, _] = play(&editor, &media, 4_096, 64, &[]);
    // The strongest spectral peak of the played audio is at twice the
    // sine's own frequency: the clip sounds an octave up.
    let peak = strongest(&played);
    assert!(
        (peak - 880.0).abs() < 5.0,
        "the clip plays at {peak} Hz, not 880 Hz"
    );
}

// The frequency of the strongest peak of a signal, by a coarse scan of its
// spectrum.
fn strongest(signal: &[f32]) -> f64 {
    let rate = f64::from(RATE.hz());
    // Goertzel over a set of candidate frequencies, in 10 Hz steps.
    let mut best = (0.0, 0.0);
    let mut frequency = 100.0;
    while frequency < 4_000.0 {
        let w = 2.0 * std::f64::consts::PI * frequency / rate;
        let coefficient = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0, 0.0);
        for &sample in signal {
            let s0 = f64::from(sample) + coefficient * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        let power = s1 * s1 + s2 * s2 - coefficient * s1 * s2;
        if power > best.1 {
            best = (frequency, power);
        }
        frequency += 10.0;
    }
    best.0
}

// Spec: audio-clip-editing R7. A clip whose file is a reversed copy plays
// it backwards, to the bit, at any block size.
#[test]
fn a_reversed_clip_plays_its_audio_backwards() {
    let frames = 4_096;
    // The file: a ramp backwards.
    let left: Vec<f32> = (0..frames).map(|i| (frames - i) as f32 * STEP).collect();
    let right: Vec<f32> = left.iter().map(|s| -s).collect();
    let mut media = MediaMap::new();
    media.insert(
        hash(5),
        Arc::new(SampleData::new(RATE.hz(), vec![left, right]).unwrap()),
    );
    let mut editor = Editor::new();
    let track = track(&mut editor, TrackKind::Audio, None);
    clip(
        &mut editor,
        track,
        Ticks::ZERO,
        Ticks::WHOLE,
        AudioData::new(hash(5)),
    );
    for block in [1, 61, 512, 4_096] {
        let [played, _] = play(&editor, &media, 1_024, block, &[]);
        for (i, &sample) in played.iter().enumerate() {
            assert_eq!(
                sample,
                (frames - i) as f32 * STEP,
                "block {block}, frame {i}"
            );
        }
    }
}

// Spec: comping R14, R16. An auditioned take lane plays in place of the main
// lane, from the next block and with no new program, at any block size; the
// main lane's edits during the audition are what it plays once it is off.
#[test]
fn an_auditioned_take_lane_plays_in_place_of_the_main_lane_with_no_program() {
    for block in [1, 61, 512, 4_096] {
        let mut media = MediaMap::new();
        media.insert(
            hash(1),
            Arc::new(SampleData::new(48_000, vec![vec![0.5; 96_000]]).unwrap()),
        );
        media.insert(
            hash(2),
            Arc::new(SampleData::new(48_000, vec![vec![0.25; 96_000]]).unwrap()),
        );
        let media = Arc::new(media);
        let mut rig = Rig::with_registry(block, |registry| registry.set_media(media.clone()));
        let a = track(&mut rig.editor, TrackKind::Audio, None);
        let main = clip(
            &mut rig.editor,
            a,
            Ticks::ZERO,
            Ticks::WHOLE,
            AudioData::new(hash(1)),
        );
        let lane: bl_model::TakeLaneId = rig.editor.new_id();
        rig.editor
            .apply(Command::AddTakeLane {
                track: a,
                lane: bl_model::NewTakeLane {
                    id: Some(lane),
                    name: "Take 1".to_owned(),
                    clips: Vec::new(),
                },
                index: None,
            })
            .unwrap();
        let lane_clip = Clip::new(
            rig.editor.new_id(),
            Ticks::ZERO,
            Ticks::WHOLE,
            ClipData::Audio(AudioData::new(hash(2))),
        );
        rig.editor
            .apply(Command::CreateTakeClip {
                lane,
                clip: lane_clip,
            })
            .unwrap();
        rig.sync();
        rig.sender.transport(TransportCommand::Play);
        let play_block = |rig: &mut Rig| {
            for _ in 0..(4_096 / block).min(8) {
                rig.block(block);
            }
            rig.last()
        };
        // The take lane plays nothing until it is auditioned.
        assert!(
            play_block(&mut rig).iter().all(|&s| s == 0.5),
            "block {block}"
        );

        let synced = rig.apply(Command::SetAudition { lane, on: true });
        assert!(!synced.program, "block {block}");
        assert!(
            play_block(&mut rig).iter().all(|&s| s == 0.25),
            "block {block}"
        );

        // An edit of the main lane during the audition isn't heard...
        rig.apply(Command::SetClipGain {
            clip: main,
            gain: 0.5,
        });
        assert!(
            play_block(&mut rig).iter().all(|&s| s == 0.25),
            "block {block}"
        );

        // ...until the audition is off.
        let synced = rig.apply(Command::SetAudition { lane, on: false });
        assert!(!synced.program, "block {block}");
        assert!(
            play_block(&mut rig).iter().all(|&s| s == 0.25),
            "block {block}: 0.5 at -6 dB"
        );
        assert_eq!(
            rig.monitor.state().swaps,
            1,
            "no new program, block {block}"
        );
        assert_eq!(rig.monitor.state().counters.data_unrouted, 0);
    }
}

//! Grooves' audio-thread work under RealtimeSanitizer (WP-6.19): a grooved
//! MIDI clip placing its notes through Quantize, Timing and Random and
//! changing their velocities, and a grooved warped clip playing its audio
//! through the groove's displacement with Velocity's gain steps, while the
//! groove's parameters, the Global Amount and the clips' grooves are edited,
//! the transport locates and the tempo ramps. RTSan stops the process at the
//! first allocation, free, lock, or blocking call in any of it, and the
//! allocation trap, installed as in every Beatlayer test binary, fails the
//! test at any heap use it sees.

bl_rt::install_trap!();

use std::sync::Arc;

use bl_core::time::{Bpm, SampleRate, TempoMap, Ticks};
use bl_engine::{Collector, Compiler, EngineConfig, MediaMap, Registry, TransportCommand, channel};
use bl_media::SampleData;
use bl_model::{
    AudioData, Clip, ClipData, Command, ContentHash, Editor, Groove, GrooveBase, GrooveId,
    GrooveNote, MidiData, Note, TrackKind, WarpMarker, WarpMode,
};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

const STEP: i64 = Ticks::QUARTER.get() / 4;

/// A stereo file of `frames` frames at 48 kHz, quiet.
fn file(frames: usize, level: f32) -> Arc<SampleData> {
    let left: Vec<f32> = (0..frames)
        .map(|i| if i % 100 < 50 { level } else { -level })
        .collect();
    let right = left.iter().map(|s| s * 0.5).collect();
    Arc::new(SampleData::new(48_000, vec![left, right]).unwrap())
}

#[test]
fn grooved_clips_pass_under_rtsan() {
    let mut media = MediaMap::new();
    media.insert(ContentHash([1; 32]), file(96_000, 0.001));
    media.set_transients(
        ContentHash([1; 32]),
        (0..16).map(|i| f64::from(i) * 0.125 + 0.003).collect(),
    );
    let registry = Registry::new().with_media(Arc::new(media));
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    let midi_track = editor.new_id();
    let audio_track = editor.new_id();
    for (index, (id, kind)) in [
        (midi_track, TrackKind::Midi),
        (audio_track, TrackKind::Audio),
    ]
    .into_iter()
    .enumerate()
    {
        editor
            .apply(Command::CreateTrack {
                id,
                kind,
                name: String::new(),
                parent: None,
                index,
            })
            .unwrap();
    }
    // A looped MIDI clip of sixteenths, and a looped warped Beats clip and a
    // warped Re-Pitch clip of the file, on the audio track.
    let notes: Vec<Note> = (0..16)
        .map(|i| Note::new(60 + (i % 5) as u8, Ticks(i * STEP), Ticks(STEP / 2), 100))
        .collect();
    let mut midi = MidiData::new(notes);
    midi.looping = true;
    midi.loop_end = Some(Ticks::WHOLE);
    let warped = |mode: WarpMode| AudioData {
        warp: true,
        warp_markers: vec![WarpMarker {
            time: 0.0,
            beat: Ticks::ZERO,
        }],
        tempo: Some(120.0),
        warp_mode: mode,
        high_quality: false,
        looping: true,
        loop_start: 0.0,
        loop_end: Some(Ticks::WHOLE.get() as f64),
        ..AudioData::new(ContentHash([1; 32])).with_edge_fades()
    };
    let mut clips = Vec::new();
    for (track, start, data) in [
        (midi_track, Ticks::ZERO, ClipData::Midi(midi)),
        (
            audio_track,
            Ticks::ZERO,
            ClipData::Audio(warped(WarpMode::RePitch)),
        ),
        (
            audio_track,
            Ticks::WHOLE * 4,
            ClipData::Audio(warped(WarpMode::Beats)),
        ),
    ] {
        let id = editor.new_id();
        let clip = Clip::new(id, start, Ticks::WHOLE * 3, data);
        editor.apply(Command::CreateClip { track, clip }).unwrap();
        clips.push(id);
    }
    // A slot clip too, which a launch plays through the same groove.
    let scene = editor.document().scenes().next().unwrap().id;
    let slot = editor.new_id();
    editor
        .apply(Command::CreateSlotClip {
            track: audio_track,
            scene,
            clip: Clip::new(
                slot,
                Ticks::ZERO,
                Ticks::WHOLE,
                ClipData::Audio(warped(WarpMode::RePitch)),
            ),
        })
        .unwrap();
    clips.push(slot);
    let groove: GrooveId = editor.new_id();
    let mut made = Groove::new(
        groove,
        "Swing".to_owned(),
        Ticks::WHOLE,
        (0..16)
            .map(|i| GrooveNote {
                position: Ticks(i * STEP + if i % 2 == 1 { STEP / 4 } else { 0 }),
                velocity: if i % 4 == 0 { 127 } else { 70 },
            })
            .collect(),
    );
    made.quantize = 50.0;
    made.random = 40.0;
    made.velocity = 60.0;
    editor.apply(Command::AddGroove { groove: made }).unwrap();
    editor
        .apply(Command::SetClipGroove {
            clips: clips.clone(),
            groove: Some(groove),
        })
        .unwrap();
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    let mut programs = 1;
    for round in 0..2_000u64 {
        // A groove edit every few blocks, reaching the playing clips as node
        // data.
        let command = match round % 20 {
            0 => Some(Command::SetGroove {
                groove,
                base: Some(GrooveBase::ALL[(round / 20 % 6) as usize]),
                quantize: Some((round % 7) as f64 * 15.0),
                timing: Some(100.0 - (round % 5) as f64 * 20.0),
                random: Some((round % 3) as f64 * 50.0),
                velocity: Some((round % 9) as f64 * 25.0 - 100.0),
            }),
            5 => Some(Command::SetGlobalGrooveAmount {
                amount: (round % 14) as f64 * 10.0,
            }),
            10 => Some(Command::SetClipGroove {
                clips: vec![clips[(round / 10 % 4) as usize]],
                groove: (round % 40 != 10).then_some(groove),
            }),
            // Now and then a tempo ramp: a program swap.
            15 if round % 200 == 15 => Some(Command::SetTempo {
                tempo: TempoMap::from_breakpoints([
                    (
                        Ticks::ZERO,
                        Bpm::new(90.0 + (round % 400) as f64 / 10.0).unwrap(),
                    ),
                    (Ticks::WHOLE * 4, Bpm::new(150.0).unwrap()),
                ])
                .unwrap(),
            }),
            _ => None,
        };
        if let Some(command) = command {
            editor.apply(command).unwrap();
            let update = compiler.sync(editor.document());
            programs += u64::from(update.program.is_some());
            sender.send_update(update);
        }
        if round % 100 == 50 {
            sender.transport(TransportCommand::Locate(Ticks(98_765 * (round % 7) as i64)));
        }
        if round % 400 == 100 {
            sender.session(bl_engine::SessionCommand::Launch {
                track: audio_track,
                scene,
            });
        }
        assert!(sender.flush());

        bl_rt::rt_test!({
            processor.process(256);
        });
        if round % 50 == 0 {
            collector.collect();
        }
    }

    let state = monitor.state().clone();
    assert_eq!(state.counters.data_unrouted, 0);
    assert_eq!(state.counters.migration_mismatches, 0);
    assert_eq!(state.swaps, programs);
    assert!(
        state.counters.data_delivered > 200,
        "{} lists",
        state.counters.data_delivered
    );
    assert!(
        state.meter(audio_track).is_some_and(|[left, _]| left > 0.0),
        "the grooved audio plays"
    );
    eprintln!(
        "2,000 blocks of grooved clips under RealtimeSanitizer: {} lists, {} swaps",
        state.counters.data_delivered, state.swaps
    );
}

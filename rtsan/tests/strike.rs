//! Strike, the one-shot drum sampler, on the audio thread under
//! RealtimeSanitizer (WP-5.18): a clip of many notes plays a sample through
//! each of its nine playback effects in turn, with the filter on and the
//! modulation route moving it, while its parameters change. RTSan stops the
//! process at the first allocation, free, lock, or blocking call on the
//! audio thread, and the allocation trap fails the test at any heap use it
//! sees.

bl_rt::install_trap!();

use std::sync::Arc;

use bl_core::time::{SampleRate, TICKS_PER_QUARTER, Ticks};
use bl_devices::drum_sampler::{
    FILTER_ON, FILTER_TYPE, FX_ON, FX_TYPE, FX1, FX2, MOD_AMOUNT, MOD_DEST, drum_sampler_type,
};
use bl_engine::{Collector, Compiler, EngineConfig, MediaMap, Registry, TransportCommand, channel};
use bl_media::SampleData;
use bl_model::{Clip, ClipData, Command, ContentHash, Editor, MidiData, Note, ParamId, TrackKind};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

#[test]
fn every_playback_effect_passes_under_rtsan() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let hash = ContentHash([5u8; 32]);
    // A second of a tone with some edge, so every effect has something to
    // work on.
    let tone: Vec<f32> = (0..48_000)
        .map(|i| (i as f32 * 0.03).sin() * 0.6 + ((i % 61) as f32 / 61.0 - 0.5) * 0.2)
        .collect();
    let mut media = MediaMap::new();
    media.insert(hash, Arc::new(SampleData::new(48_000, vec![tone]).unwrap()));
    let mut collector = Collector::new();
    let mut registry = Registry::empty();
    bl_devices::register(&mut registry);
    let mut compiler = Compiler::new(
        config,
        registry.with_media(Arc::new(media)),
        collector.handle(),
    );
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let mut strike = drum_sampler_type().device(editor.new_id());
    strike.media = vec![Some(hash)];
    let device = strike.id;
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: strike,
        })
        .unwrap();
    // A bar of sixteenth notes on many keys, looping.
    let sixteenth = Ticks(TICKS_PER_QUARTER / 4);
    let notes =
        (0..64u8).map(|i| Note::new(48 + i % 24, sixteenth * i64::from(i), sixteenth * 3, 90));
    let clip = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(TICKS_PER_QUARTER * 16),
                ClipData::Midi(MidiData::new(notes)),
            ),
        })
        .unwrap();
    sender.send_update(compiler.sync(editor.document()));
    assert!(sender.flush());
    sender.transport(TransportCommand::Play);

    let set = |param: ParamId, value: f64| Command::SetDeviceParam {
        device,
        param,
        value,
    };
    for command in [
        set(FX_ON, 1.0),
        set(FILTER_ON, 1.0),
        set(MOD_DEST, 4.0),
        set(MOD_AMOUNT, 60.0),
    ] {
        editor.apply(command).unwrap();
    }
    sender.send_update(compiler.sync(editor.document()));
    for round in 0..1_800u64 {
        // A new setting every few blocks, so changes land mid-stream: each
        // of the nine effects for 200 blocks.
        let command = match round % 10 {
            0 => Some(set(FX_TYPE, (round / 200) as f64)),
            3 => Some(set(FX1, (round * 7 % 101) as f64)),
            6 => Some(set(FX2, (round * 13 % 101) as f64)),
            8 => Some(set(FILTER_TYPE, (round / 10 % 4) as f64)),
            _ => None,
        };
        if let Some(command) = command {
            editor.apply(command).unwrap();
            sender.send_update(compiler.sync(editor.document()));
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
    assert_eq!(state.counters.events_overflowed, 0);
    assert!(state.counters.events_applied > 100);
    eprintln!(
        "1,800 blocks of Strike through all nine effects under RealtimeSanitizer: {} parameter \
         changes, peak engine load {:.1}%",
        state.counters.events_applied,
        state.cpu_load_max * 100.0
    );
}

//! ARA 2 playback's audio-thread work under RealtimeSanitizer (IMP-20; the
//! ARA hosting spec, R7, R8): the engine plays a track whose ARA plug-in
//! renders its clips, beside a clip the plug-in can't render, which the clip
//! player keeps, while clips are moved, trimmed, made and deleted, which
//! hands the plug-in back to the main thread and activates it again between
//! blocks. RTSan stops the process at the first allocation, free, lock, or
//! blocking call on the audio thread, in the engine, the VST3 node and the
//! plug-in's render alike; the allocation trap, installed as in every
//! Beatlayer test binary, fails the test at any heap use it sees. The
//! plug-in's audio readers run on the main thread here, and the host counts
//! any that run inside a render call: there must be none.

bl_rt::install_trap!();

use std::collections::BTreeMap;
use std::sync::Arc;

use bl_core::time::{SampleRate, Ticks};
use bl_engine::{Collector, Compiler, EngineConfig, MediaMap, Registry, TransportCommand, channel};
use bl_media::SampleData;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, Device, DeviceId, Editor, PluginState,
    TrackKind,
};
use bl_plugin_host::ara::AraMedia;
use bl_plugin_host::vst3::Vst3Host;

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

const QUARTER: i64 = 705_600;

/// A stereo file of `frames` frames at 48 kHz that differs everywhere.
fn file(frames: usize) -> Arc<SampleData> {
    let channel = |seed: usize| -> Vec<f32> {
        (0..frames)
            .map(|at| (((at * 31 + seed * 7) % 1999) as f32 / 1999.0 - 0.5) * 0.1)
            .collect()
    };
    Arc::new(SampleData::new(48_000, vec![channel(0), channel(1)]).unwrap())
}

/// The set's files for the ARA plug-in.
struct Files(BTreeMap<ContentHash, Arc<SampleData>>);

impl AraMedia for Files {
    fn audio(&self, media: ContentHash) -> Option<Arc<SampleData>> {
        self.0.get(&media).cloned()
    }
}

#[test]
fn ara_playback_passes_under_rtsan() {
    let (one, two) = (ContentHash([1; 32]), ContentHash([2; 32]));
    let mut media = MediaMap::new();
    media.insert(one, file(96_000));
    media.insert(two, file(96_000));
    let host = Vst3Host::new();
    let types = host.load(bl_test_ara::bundle()).unwrap();
    host.set_ara_media(Arc::new(Files(BTreeMap::from([
        (one, file(96_000)),
        (two, file(96_000)),
    ]))));
    let mut registry = Registry::new().with_media(Arc::new(media));
    registry.add(host.factory());
    let config = EngineConfig::new(SampleRate::HZ_48000, 512);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Audio,
            name: "Vocal".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let device: DeviceId = editor.new_id();
    let mut plug_in = Device::new(
        device,
        types[0].device_type.name.clone(),
        types[0].device_type.kind.clone(),
    );
    plug_in.plugin = Some(PluginState {
        path: bl_test_ara::bundle().to_string_lossy().into_owned(),
        data: Vec::new(),
    });
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: plug_in,
        })
        .unwrap();
    // Two clips the plug-in renders, and one at half gain, which the clip
    // player keeps.
    let mut clips: Vec<ClipId> = Vec::new();
    for (start, media, gain) in [
        (0, one, 1.0),
        (4 * QUARTER, two, 1.0),
        (2 * QUARTER, one, 0.5),
    ] {
        let id = editor.new_id();
        let mut audio = AudioData::new(media);
        audio.gain = gain;
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(id, Ticks(start), Ticks(2 * QUARTER), ClipData::Audio(audio)),
            })
            .unwrap();
        clips.push(id);
    }
    let mut revision = 1;
    host.know(editor.document());
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);
    let mut programs = 1;
    let mut heard = false;
    for round in 0..1_500u64 {
        let command = match round % 60 {
            10 => Some(Command::MoveClip {
                clip: clips[0],
                track,
                start: Ticks(QUARTER * (round % 5) as i64),
            }),
            20 => Some(Command::TrimClipStart {
                clip: clips[1],
                start: Ticks(4 * QUARTER + QUARTER / 4),
            }),
            30 => Some(Command::DeleteClip { clip: clips[1] }),
            40 => {
                editor.undo().ok();
                None
            }
            50 => Some(Command::SetClipMute {
                clip: clips[0],
                mute: round % 120 == 50,
            }),
            _ => None,
        };
        if let Some(command) = command {
            editor.apply(command).unwrap();
        }
        if matches!(round % 60, 10 | 20 | 30 | 40 | 50) {
            revision += 1;
            let update = compiler.sync(editor.document());
            programs += u64::from(update.program.is_some());
            sender.send_update(update);
        }
        // The main thread's work between blocks: the plug-in's activation,
        // its edits, and its hand-back.
        host.idle();
        host.sync_ara(editor.document(), revision);
        if round % 100 == 50 {
            sender.transport(TransportCommand::Locate(Ticks(
                QUARTER * (round % 7) as i64,
            )));
        }
        assert!(sender.flush());
        let frames = [1, 100, 256, 512][(round % 4) as usize];
        bl_rt::rt_test!({
            processor.process(frames);
        });
        if round % 50 == 0 {
            collector.collect();
        }
        heard |= monitor
            .state()
            .meter(track)
            .is_some_and(|[left, _]| left > 0.0);
    }
    assert!(heard, "the track plays");
    let status = host.ara_status();
    assert_eq!(status.len(), 1);
    assert!(status[0].reader_calls > 0, "the plug-in read the files");
    assert_eq!(status[0].audio_thread_reads, 0);
    assert!(
        status[0].edit_cycles > 4,
        "the clips' edits reached the plug-in"
    );
    eprintln!(
        "1,500 blocks of ARA playback under RealtimeSanitizer: {} programs, {} edit cycles",
        programs, status[0].edit_cycles
    );
}

//! An LV2 plug-in on the audio thread under RealtimeSanitizer (WP-5.39; the
//! LV2 hosting spec, R17): the test plug-in processes blocks that split at
//! parameter changes, take notes, send them on, get the song's position and
//! schedule worker jobs, while the allocation trap fails the test at any heap
//! use. RTSan stops the process at the first allocation, free, lock or
//! blocking call on the audio thread. They run where lilv is: on Linux, and
//! elsewhere when `BL_TEST_LILV` names a lilv library.

bl_rt::install_trap!();

use bl_core::time::{Frames, SampleRate};
use bl_engine::{
    Block, NodeConfig, NodeEvent, NodeFactory, NoteBuffer, NoteEvent, NoteKind, ParamValue,
    TransportInfo,
};
use bl_model::{Device, DeviceId, DeviceKind, ParamId, PluginFormat};
use bl_plugin_host::lv2::{Lv2Error, Lv2Host};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

const RATE: SampleRate = SampleRate::HZ_48000;
const FRAMES: usize = 256;

fn host() -> Option<Lv2Host> {
    let host = Lv2Host::new();
    if let Some(library) = std::env::var_os("BL_TEST_LILV") {
        host.use_library(library);
    }
    match host.load(bl_test_lv2::bundle()) {
        Ok(_) => Some(host),
        Err(Lv2Error::Unavailable) if !cfg!(target_os = "linux") => None,
        Err(e) => panic!("the test LV2 plug-in should load: {e}"),
    }
}

fn note(offset: u32, kind: NoteKind, key: u8) -> NoteEvent {
    NoteEvent {
        offset,
        kind,
        channel: 0,
        key,
        velocity: 0.8,
        note_id: None,
    }
}

#[test]
fn an_lv2_plugin_processes_blocks_under_rtsan() {
    let Some(host) = host() else { return };
    let config = NodeConfig {
        round_trip_latency: 0,
        sample_rate: RATE,
        max_frames: FRAMES,
        channels: 2,
    };
    let device = Device::new(
        DeviceId(bl_core::id::Id::new(1).unwrap()),
        bl_test_lv2::PLUGIN_NAME,
        DeviceKind::Plugin {
            format: PluginFormat::Lv2,
            plugin_id: bl_test_lv2::PLUGIN_URI.to_owned(),
        },
    );
    let mut node = host.factory().create(&device, &config).expect("a node");
    host.idle();
    let mut data = vec![0.5f32; FRAMES * 2];
    let mut out = NoteBuffer::with_capacity(64);
    let mut position = 0i64;
    for round in 0..200u32 {
        let events = [
            NodeEvent {
                offset: 17,
                param: ParamId(bl_test_lv2::GAIN),
                value: ParamValue::Set(f64::from(round % 5) / 4.0),
            },
            NodeEvent {
                offset: 200,
                param: ParamId(bl_test_lv2::GAIN),
                value: ParamValue::Set(1.0),
            },
        ];
        let notes = [
            note(3, NoteKind::On, 60 + (round % 12) as u8),
            note(100, NoteKind::Off, 60 + (round % 12) as u8),
        ];
        out.clear();
        let transport = TransportInfo {
            playing: true,
            position: Frames(position),
        };
        {
            let mut block = Block::new(&mut data, 2, FRAMES, FRAMES, &events, transport, RATE)
                .with_notes(&notes, &mut out);
            bl_rt::rt_test!({ node.process(&mut block) });
        }
        position += FRAMES as i64;
        // The echoed note, and the beat the plug-in hears in each of the
        // block's three parts while the song plays.
        assert_eq!(
            out.notes().iter().filter(|n| n.kind == NoteKind::On).count(),
            4
        );
        // The main thread's side runs between blocks, not on the audio
        // thread.
        host.idle();
    }
    assert_eq!(node.latency(), bl_test_lv2::LATENCY);
}

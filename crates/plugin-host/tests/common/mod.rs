//! What the CLAP host's tests share: the test plug-in built from source
//! (`bl-test-clap`) as a CLAP bundle, and its nodes run block by block, each
//! block a real-time section under the allocation trap.

#![allow(dead_code, unused_imports, reason = "each test file uses some")]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use bl_core::id::Id;
use bl_core::time::SampleRate;
use bl_engine::{
    Block, Node, NodeConfig, NodeEvent, NoteBuffer, NoteEvent, ParamValue, TransportInfo,
};
use bl_model::{Device, DeviceId, DeviceKind, ParamId, PluginFormat};

pub use bl_test_clap::{GAIN, LAST_NOTE, LATENCY, PLUGIN_ID, PLUGIN_NAME};

pub const RATE: SampleRate = SampleRate::HZ_48000;
pub const CHANNELS: usize = 2;
pub const FRAMES: usize = 64;

pub fn config() -> NodeConfig {
    NodeConfig {
        sample_rate: RATE,
        max_frames: FRAMES,
        channels: CHANNELS,
    }
}

/// The test plug-in as a CLAP bundle, made once per test run from the
/// cdylib Cargo built beside this test: on macOS a `.clap` bundle with its
/// `Info.plist`, elsewhere the library renamed `.clap`, as the CLAP
/// specification lays them out.
pub fn bundle() -> &'static Path {
    static BUNDLE: OnceLock<PathBuf> = OnceLock::new();
    BUNDLE.get_or_init(|| {
        let library = library();
        let dir = std::env::temp_dir().join(format!("beatlayer-test-clap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary folder");
        let bundle = dir.join("Test Gain Echo.clap");
        if cfg!(target_os = "macos") {
            let macos = bundle.join("Contents").join("MacOS");
            std::fs::create_dir_all(&macos).expect("the bundle's folders");
            std::fs::copy(&library, macos.join("Test Gain Echo")).expect("the library");
            std::fs::write(
                bundle.join("Contents").join("Info.plist"),
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Test Gain Echo</string>
<key>CFBundleIdentifier</key><string>io.beatlayer.test.gain-echo</string>
<key>CFBundlePackageType</key><string>BNDL</string>
</dict></plist>
"#,
            )
            .expect("the bundle's Info.plist");
        } else {
            std::fs::copy(&library, &bundle).expect("the library");
        }
        bundle
    })
}

/// The newest test plug-in library Cargo built beside this test binary.
fn library() -> PathBuf {
    let exe = std::env::current_exe().expect("the test binary's path");
    let deps = exe.parent().expect("the test binary's folder");
    let prefix = format!("{}bl_test_clap", std::env::consts::DLL_PREFIX);
    let suffix = std::env::consts::DLL_SUFFIX;
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = [deps, deps.parent().unwrap_or(deps)]
        .iter()
        .flat_map(|dir| std::fs::read_dir(dir).into_iter().flatten().flatten())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix) && n.ends_with(suffix))
        })
        .filter_map(|path| Some((path.metadata().ok()?.modified().ok()?, path)))
        .collect();
    found.sort();
    found
        .pop()
        .map(|(_, path)| path)
        .expect("Cargo builds the test plug-in's cdylib beside the tests")
}

/// A Test Gain Echo device with `params` set.
pub fn device(id: u64, params: &[(u32, f64)]) -> Device {
    let mut device = Device::new(
        DeviceId(Id::new(id).expect("an ID")),
        PLUGIN_NAME,
        DeviceKind::Plugin {
            format: PluginFormat::Clap,
            plugin_id: PLUGIN_ID.to_owned(),
        },
    );
    for (param, value) in params {
        device.params.insert(ParamId(*param), *value);
    }
    device
}

pub fn set(offset: u32, param: u32, value: f64) -> NodeEvent {
    NodeEvent {
        offset,
        param: ParamId(param),
        value: ParamValue::Set(value),
    }
}

/// What a block put out: each channel, and the notes the node sent on.
pub struct Output {
    pub channels: Vec<Vec<f32>>,
    pub notes: Vec<NoteEvent>,
}

/// Runs one block of `input` on every channel through `node`, with
/// `events` and `notes`, as a real-time section.
pub fn run(node: &mut dyn Node, input: &[f32], events: &[NodeEvent], notes: &[NoteEvent]) -> Output {
    let frames = input.len();
    let mut data: Vec<f32> = (0..CHANNELS).flat_map(|_| input.iter().copied()).collect();
    let mut out = NoteBuffer::with_capacity(64);
    {
        let mut block = Block::new(
            &mut data,
            CHANNELS,
            frames,
            frames,
            events,
            TransportInfo::default(),
            RATE,
        )
        .with_notes(notes, &mut out);
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    Output {
        channels: data.chunks(frames).map(<[f32]>::to_vec).collect(),
        notes: out.notes().to_vec(),
    }
}

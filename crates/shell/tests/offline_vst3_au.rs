//! Exports and previews with VST3 plug-ins and Audio Units (WP-4.21; the
//! offline plug-in renders spec, as `offline_plugins.rs` has it for CLAP):
//! a render borrows the plug-ins the engine plays, so the test VST3
//! plug-in, built from source, and Apple's AUDelay on macOS render exactly
//! as the engine plays them, and go back to the engine afterwards, however
//! the render ends. Every block of a render runs under the allocation trap.

bl_rt::install_trap!();

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::{Duration, Instant};

use bl_core::time::{SampleRate, Ticks};
use bl_engine::{
    EngineConfig, MediaLibrary, NodeConfig, Offline, Registry, TransportCommand, When,
};
use bl_model::{DeviceId, Document, ParamId, TrackId};
use bl_plugin_host::Hosts;
use bl_shell::export::{self, ExportError, Job, Length, Render, Rendered};
use bl_shell::export_commands as export_ids;
use bl_shell::plugin_commands::{self as plugin_ids, PluginInfo, PluginLoaded};
use bl_shell::{Shell, ShellError, browser_commands};
use serde_json::json;

/// Frames rendered: half a second at 48 kHz.
const FRAMES: u64 = 24_000;

/// AUDelay's parameters, in the order its tree lists them.
#[cfg(target_os = "macos")]
const AU_MIX: u32 = 0;
#[cfg(target_os = "macos")]
const AU_DELAY_TIME: u32 = 1;

/// The plug-in formats this platform hosts besides CLAP.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Format {
    Vst3,
    #[cfg(target_os = "macos")]
    AudioUnit,
}

fn formats() -> Vec<Format> {
    vec![
        Format::Vst3,
        #[cfg(target_os = "macos")]
        Format::AudioUnit,
    ]
}

impl Format {
    /// A parameter of the test device, and the value the set gives it.
    fn param(self) -> (u32, f64) {
        match self {
            Self::Vst3 => (bl_test_vst3::GAIN, 0.5),
            #[cfg(target_os = "macos")]
            Self::AudioUnit => (AU_MIX, 100.0),
        }
    }

    /// The settings the device's plug-in is active for, from the host that
    /// hosts its format.
    fn active_for(self, shell: &Shell, device: DeviceId) -> Option<NodeConfig> {
        match self {
            Self::Vst3 => shell.vst3().status(device)?.active_for,
            #[cfg(target_os = "macos")]
            Self::AudioUnit => shell.plugin_hosts().au?.status(device)?.active_for,
        }
    }

    /// The device's plug-in state, as the set would save it.
    fn state(self, shell: &Shell, device: DeviceId) -> bl_model::PluginState {
        match self {
            Self::Vst3 => shell.vst3().save_state(device).unwrap(),
            #[cfg(target_os = "macos")]
            Self::AudioUnit => shell.plugin_hosts().au.unwrap().save_state(device).unwrap(),
        }
    }

    /// Whether the format's host can lend its plug-ins again.
    fn can_lend(self, shell: &Shell) -> bool {
        match self {
            Self::Vst3 => shell.vst3().lend().is_ok(),
            #[cfg(target_os = "macos")]
            Self::AudioUnit => shell.plugin_hosts().au.unwrap().lend().is_ok(),
        }
    }
}

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-offline-vst3-au-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, rest: &str) -> PathBuf {
        self.0.join(rest)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a 16-bit mono WAV file of one second at 48 kHz: a ramp.
fn wav(path: &Path) {
    let frames = 48_000u32;
    let mut bytes = Vec::new();
    let data = frames * 2;
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let sample = ((i % 100) as i16 - 50) * 200;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// A shell holding a one-second clip at 1.1.1, with the audio engine
/// running if `audio`, and the clip's track.
fn shell_with_a_clip(scratch: &Scratch, audio: bool) -> (Shell, TrackId) {
    let mut shell = Shell::headless();
    if audio {
        shell.start_audio();
        assert!(shell.audio().engine_running());
    }
    let file = scratch.path("Ramp.wav");
    wav(&file);
    shell
        .run(
            browser_commands::ADD_TO_ARRANGEMENT,
            json!({ "paths": [file.to_string_lossy()], "at": 0 }),
        )
        .unwrap();
    let track = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.clips().next().is_some())
        .map(bl_model::Track::id)
        .expect("the clip's track");
    (shell, track)
}

/// Loads the format's test plug-in onto `track` with its parameters set,
/// and returns its device.
fn load(shell: &mut Shell, track: TrackId, format: Format) -> DeviceId {
    let loaded: PluginLoaded = match format {
        Format::Vst3 => serde_json::from_value(
            shell
                .run(
                    plugin_ids::LOAD,
                    json!({ "path": bl_test_vst3::bundle(), "track": track }),
                )
                .unwrap(),
        ),
        #[cfg(target_os = "macos")]
        Format::AudioUnit => serde_json::from_value(
            shell
                .run(
                    plugin_ids::LOAD_AU,
                    json!({ "type": "aufx", "subtype": "dely", "manufacturer": "appl",
                            "track": track }),
                )
                .unwrap(),
        ),
    }
    .unwrap();
    let mut params = vec![format.param()];
    #[cfg(target_os = "macos")]
    if format == Format::AudioUnit {
        // Fully wet at a short delay, so the render holds the unit's echo
        // of the ramp.
        params.push((AU_DELAY_TIME, 0.01));
    }
    for (param, value) in params {
        shell
            .run(
                "device.set_param",
                json!({ "device": loaded.device, "param": param, "value": value }),
            )
            .unwrap();
    }
    loaded.device
}

/// A shell with a clip and the format's plug-in on its track.
fn shell_with(scratch: &Scratch, format: Format, audio: bool) -> (Shell, DeviceId) {
    let (mut shell, track) = shell_with_a_clip(scratch, audio);
    let device = load(&mut shell, track, format);
    (shell, device)
}

fn media(shell: &Shell) -> Arc<dyn MediaLibrary> {
    shell.media_store()
}

/// What the device's plug-in reports for a parameter.
fn value(info: &PluginInfo, param: u32) -> Option<f64> {
    info.values
        .iter()
        .find(|(id, _)| *id == ParamId(param))
        .map(|(_, v)| *v)
}

/// Polls the shell, as the app's frames do, until the device's plug-in is
/// running in the engine with the set's value.
fn running(shell: &mut Shell, device: DeviceId, format: Format) -> PluginInfo {
    let (param, wanted) = format.param();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        shell.poll();
        if let Ok(info) = shell.run(plugin_ids::INFO, json!({ "device": device }))
            && let Ok(info) = serde_json::from_value::<PluginInfo>(info)
            && info.active
            && value(&info, param) == Some(wanted)
        {
            return info;
        }
        assert!(Instant::now() < until, "the {format:?} plug-in never ran");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn spec(sample_rate: u32, block: usize) -> Render {
    Render {
        start: Ticks::ZERO,
        length: Length::Frames(FRAMES * u64::from(sample_rate) / 48_000),
        sample_rate,
        engine_rate: 48_000,
        block,
    }
}

/// Renders the set as exports and previews do, borrowing from `plugins`.
fn render(
    document: &Document,
    media: Arc<dyn MediaLibrary>,
    plugins: Hosts<'_>,
    spec: &Render,
    progress: &mut dyn FnMut(f64),
) -> Result<(Vec<f32>, Rendered), ExportError> {
    let mut samples = Vec::new();
    let rendered = export::render(
        document,
        media,
        plugins,
        spec,
        60.0,
        &AtomicBool::new(false),
        progress,
        &mut |block| {
            samples.extend_from_slice(block);
            Ok(())
        },
    )?;
    assert_eq!(rendered.heap_operations, 0, "the real-time rules");
    Ok((samples, rendered))
}

/// The set as the engine plays it on a device: the same processor, with a
/// registry holding the plug-in hosts' factories as the audio host's does,
/// from 1.1.1.
fn as_the_engine_plays(document: &Document, media: Arc<dyn MediaLibrary>) -> Vec<f32> {
    let clap = bl_plugin_host::clap::ClapHost::new();
    clap.load(bl_test_clap::bundle()).unwrap();
    let vst3 = bl_plugin_host::vst3::Vst3Host::new();
    vst3.load(bl_test_vst3::bundle()).unwrap();
    let au = bl_plugin_host::au::AuHost::new();
    #[cfg(target_os = "macos")]
    au.load("aufx", "dely", "appl").unwrap();
    let mut registry = Registry::new();
    bl_devices::register(&mut registry);
    registry.add(clap.factory());
    registry.add(vst3.factory());
    registry.add(au.factory());
    let mut offline = Offline::new(
        EngineConfig::new(SampleRate::HZ_48000, export::BLOCK),
        registry.with_media(media),
    );
    assert!(offline.sync(document).is_empty());
    offline.sender().transport(TransportCommand::SetLoop(None));
    offline
        .sender()
        .transport(TransportCommand::Locate(Ticks::ZERO));
    offline
        .sender()
        .transport_at(TransportCommand::Play, When::Frame(0));
    let mut samples = Vec::new();
    offline.render(FRAMES, |out| {
        for frame in 0..out.frames() {
            samples.extend((0..out.channels()).map(|c| out.channel(c)[frame]));
        }
    });
    samples
}

fn bits(samples: &[f32]) -> Vec<u32> {
    samples.iter().map(|s| s.to_bits()).collect()
}

// Spec: offline-plugin-renders R1, R3, R11
#[test]
fn an_export_renders_the_plugin_as_the_engine_plays_it() {
    for format in formats() {
        let scratch = Scratch::new("as-played");
        let (mut shell, device) = shell_with(&scratch, format, true);
        running(&mut shell, device, format);
        let document = shell.editor().document().clone();

        let (exported, rendered) = render(
            &document,
            media(&shell),
            shell.plugin_hosts(),
            &spec(48_000, export::BLOCK),
            &mut |_| {},
        )
        .unwrap();
        assert!(rendered.unhosted.is_empty(), "{format:?}");
        let played = as_the_engine_plays(&document, media(&shell));
        assert_eq!(
            bits(&exported),
            bits(&played),
            "{format:?}, sample for sample"
        );

        // Not passed through.
        let (through, _) = render(
            &document,
            media(&shell),
            Hosts::default(),
            &spec(48_000, export::BLOCK),
            &mut |_| {},
        )
        .unwrap();
        assert!(through.iter().any(|s| *s != 0.0));
        assert_ne!(bits(&exported), bits(&through), "{format:?}");

        // The command an agent and `bl-cli export` run writes it to the
        // file.
        let path = scratch.path("plugin.wav");
        shell
            .run(
                export_ids::AUDIO,
                json!({ "path": path.to_string_lossy(), "bit_depth": 32,
                        "sample_rate": 48000 }),
            )
            .unwrap();
        let file = bl_media::decode_file(&path).unwrap();
        let left = file.channel(0).unwrap();
        for (n, sample) in left.iter().take(FRAMES as usize).enumerate() {
            assert_eq!(
                sample.to_bits(),
                exported[n * 2].to_bits(),
                "{format:?}, frame {n}"
            );
        }
    }
}

// Spec: offline-plugin-renders R5
#[test]
fn renders_with_a_vst3_plugin_or_unit_are_the_same_every_time_and_at_any_block_size() {
    for format in formats() {
        let scratch = Scratch::new("determinism");
        // No audio device, as `bl-cli export` runs: the render makes the
        // device's plug-in itself.
        let (shell, _) = shell_with(&scratch, format, false);
        let document = shell.editor().document().clone();
        let once = |block| {
            render(
                &document,
                media(&shell),
                shell.plugin_hosts(),
                &spec(48_000, block),
                &mut |_| {},
            )
            .unwrap()
            .0
        };
        let first = once(export::BLOCK);
        assert!(first.iter().any(|s| *s != 0.0), "{format:?}");
        for block in [export::BLOCK, 64, 512, 4096] {
            assert_eq!(
                bits(&once(block)),
                bits(&first),
                "{format:?}, block {block}"
            );
        }
    }
}

// Spec: offline-plugin-renders R3, R4, R9
#[test]
fn a_render_at_another_rate_and_block_reactivates_the_plugin_and_gives_it_back() {
    for format in formats() {
        let scratch = Scratch::new("rates");
        let (mut shell, device) = shell_with(&scratch, format, true);
        let live = running(&mut shell, device, format);
        let engine = format
            .active_for(&shell, device)
            .expect("active for the engine");
        let state = format.state(&shell, device);

        let document = shell.editor().document().clone();
        let mut during = None;
        render(
            &document,
            media(&shell),
            shell.plugin_hosts(),
            &spec(96_000, 100),
            &mut |_| {
                if during.is_none() {
                    during = format.active_for(&shell, device);
                }
            },
        )
        .unwrap();
        assert_eq!(
            during,
            Some(NodeConfig {
                sample_rate: SampleRate::new(96_000).unwrap(),
                max_frames: 100,
                channels: 2,
            }),
            "{format:?}, reactivated for the render"
        );

        // Back in the engine, at its own settings, with its state.
        let after = running(&mut shell, device, format);
        assert_eq!(
            format.active_for(&shell, device),
            Some(engine),
            "{format:?}"
        );
        assert_eq!(after.failure, None);
        assert_eq!(after.values, live.values, "{format:?}");
        assert_eq!(format.state(&shell, device), state, "{format:?}");
    }
}

// Spec: offline-plugin-renders R4
#[test]
fn a_cancelled_export_gives_every_plugin_back() {
    for format in formats() {
        let scratch = Scratch::new("cancel");
        let (mut shell, device) = shell_with(&scratch, format, true);
        let live = running(&mut shell, device, format);
        let engine = format.active_for(&shell, device);
        let state = format.state(&shell, device);
        let document = shell.editor().document().clone();
        let cancel = AtomicBool::new(false);
        let result = export::export(
            &document,
            media(&shell),
            shell.plugin_hosts(),
            &Job {
                path: scratch.path("cancelled.wav"),
                file_type: bl_media::FileType::Wav,
                bit_depth: bl_media::BitDepth::Int24,
                dither: bl_media::Dither::Triangular,
                sample_rate: 44_100,
                engine_rate: 48_000,
                normalize: false,
                start: Ticks::ZERO,
                length: Length::Frames(FRAMES),
                replace: false,
                block: export::BLOCK,
            },
            &cancel,
            &mut |_| cancel.store(true, Relaxed),
        );
        assert_eq!(result.unwrap_err(), ExportError::Cancelled);
        assert!(!scratch.path("cancelled.wav").exists());

        let after = running(&mut shell, device, format);
        assert_eq!(after.values, live.values, "{format:?}");
        assert_eq!(format.active_for(&shell, device), engine, "{format:?}");
        assert_eq!(format.state(&shell, device), state, "{format:?}");
        // And the next render can borrow it again.
        assert!(format.can_lend(&shell), "{format:?}");
    }
}

// Spec: offline-plugin-renders R7, R8
#[test]
fn a_preview_renders_the_plugin_and_waits_for_the_song_to_stop() {
    for format in formats() {
        let scratch = Scratch::new("preview");
        let (mut shell, device) = shell_with(&scratch, format, true);
        running(&mut shell, device, format);
        let playing = |shell: &mut Shell| -> bool {
            shell.run("transport.state", json!({})).unwrap()["playing"]
                .as_bool()
                .unwrap()
        };
        let until = |shell: &mut Shell, want: bool| {
            let until = Instant::now() + Duration::from_secs(10);
            while playing(shell) != want {
                assert!(Instant::now() < until, "the transport never got there");
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        shell.run("transport.play", json!({})).unwrap();
        until(&mut shell, true);

        let refused = shell
            .run("perception.preview", json!({ "length": 705_600 }))
            .unwrap_err();
        assert!(
            matches!(&refused, ShellError::Unavailable { reason, .. } if reason.contains("transport.stop")),
            "{format:?}: {refused:?}"
        );

        // An export stops the song and leaves it stopped.
        shell
            .run(
                export_ids::AUDIO,
                json!({ "path": scratch.path("while-playing.wav").to_string_lossy(),
                        "bit_depth": 24, "sample_rate": 48000 }),
            )
            .unwrap();
        until(&mut shell, false);
        running(&mut shell, device, format);

        // Stopped, the preview renders the plug-in: the same samples as an
        // export of the range.
        let preview = shell
            .run("perception.preview", json!({ "length": 705_600 }))
            .unwrap();
        let samples = shell
            .previews()
            .get(preview["id"].as_u64().unwrap())
            .unwrap()
            .samples
            .clone();
        let document = shell.editor().document().clone();
        let (exported, _) = render(
            &document,
            media(&shell),
            shell.plugin_hosts(),
            &spec(48_000, export::BLOCK),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(bits(&samples), bits(&exported), "{format:?}");
        running(&mut shell, device, format);
    }
}

// Spec: offline-plugin-renders R1, R5
#[test]
fn a_set_with_a_clap_a_vst3_and_an_audio_unit_device_renders_all_three_in_one_export() {
    let scratch = Scratch::new("all-three");
    let (mut shell, track) = shell_with_a_clip(&scratch, true);
    let clap: PluginLoaded = serde_json::from_value(
        shell
            .run(
                plugin_ids::LOAD,
                json!({ "path": bl_test_clap::bundle(), "track": track }),
            )
            .unwrap(),
    )
    .unwrap();
    shell
        .run(
            "device.set_param",
            json!({ "device": clap.device, "param": bl_test_clap::GAIN, "value": 0.5 }),
        )
        .unwrap();
    let others: Vec<(Format, DeviceId)> = formats()
        .into_iter()
        .map(|format| (format, load(&mut shell, track, format)))
        .collect();
    for (format, device) in &others {
        running(&mut shell, *device, *format);
    }
    let document = shell.editor().document().clone();
    let render_with = |hosts: Hosts<'_>| {
        render(
            &document,
            media(&shell),
            hosts,
            &spec(48_000, export::BLOCK),
            &mut |_| {},
        )
        .unwrap()
        .0
    };

    let all = render_with(shell.plugin_hosts());
    assert_eq!(
        bits(&all),
        bits(&as_the_engine_plays(&document, media(&shell))),
        "as the engine plays them"
    );
    // Each format's devices are in it: without that format's host, the
    // render is something else.
    let hosts = shell.plugin_hosts();
    assert_ne!(
        bits(&all),
        bits(&render_with(Hosts {
            clap: None,
            ..hosts
        })),
        "CLAP"
    );
    assert_ne!(
        bits(&all),
        bits(&render_with(Hosts {
            vst3: None,
            ..hosts
        })),
        "VST3"
    );
    #[cfg(target_os = "macos")]
    assert_ne!(
        bits(&all),
        bits(&render_with(Hosts { au: None, ..hosts })),
        "Audio Unit"
    );
    // And all three are back in the engine.
    for (format, device) in &others {
        running(&mut shell, *device, *format);
    }
    let until = Instant::now() + Duration::from_secs(10);
    while !shell
        .plugins()
        .status(clap.device)
        .is_some_and(|status| status.active)
    {
        shell.poll();
        assert!(Instant::now() < until, "the CLAP plug-in never ran");
        std::thread::sleep(Duration::from_millis(5));
    }
}

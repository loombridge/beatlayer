//! Exports and previews with plug-ins (WP-2.6b; the offline plug-in renders
//! spec): a render borrows the plug-ins the engine plays, so the test
//! plug-in, built from source, renders exactly as the engine plays it, and
//! goes back to the engine afterwards, however the render ends. Every block
//! of a render runs under the allocation trap.

bl_rt::install_trap!();

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::{Duration, Instant};

use bl_core::time::{SampleRate, Ticks};
use bl_engine::{
    EngineConfig, MediaLibrary, NodeConfig, Offline, Registry, TransportCommand, When,
};
use bl_model::{DeviceId, Document};
use bl_plugin_host::Hosts;
use bl_plugin_host::clap::{ClapHost, PluginStatus};
use bl_shell::export::{self, ExportError, Job, Length, Render, Rendered};
use bl_shell::export_commands::{self as export_ids, ExportResult};
use bl_shell::plugin_commands::{self as plugin_ids, PluginLoaded};
use bl_shell::{Shell, ShellError, browser_commands};
use bl_test_clap::GAIN;
use serde_json::json;

/// Frames rendered: half a second at 48 kHz.
const FRAMES: u64 = 24_000;

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-offline-plugins-{}-{name}", std::process::id()));
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

/// A shell holding a one-second clip at 1.1.1 on a track with the test
/// plug-in at a gain of 0.5, with the audio engine running if `playing`.
fn shell_with_a_plugin(scratch: &Scratch, audio: bool) -> (Shell, DeviceId) {
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
    let loaded: PluginLoaded = serde_json::from_value(
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
            json!({ "device": loaded.device, "param": GAIN, "value": 0.5 }),
        )
        .unwrap();
    (shell, loaded.device)
}

fn media(shell: &Shell) -> Arc<dyn MediaLibrary> {
    shell.media_store()
}

/// Polls the shell, as the app's frames do, until the device's plug-in
/// is running in the engine at a gain of `gain`.
fn running(shell: &mut Shell, device: DeviceId, gain: f64) -> PluginStatus {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        shell.poll();
        if let Some(status) = shell.plugins().status(device)
            && status.active
            && status.values.iter().any(|(p, v)| p.0 == GAIN && *v == gain)
        {
            return status;
        }
        assert!(Instant::now() < until, "the plug-in never ran");
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

/// Renders the set as exports and previews do, borrowing `plugins`.
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
        None,
    )?;
    assert_eq!(rendered.heap_operations, 0, "the real-time rules");
    Ok((samples, rendered))
}

/// The set as the engine plays it on a device: the same processor, with a
/// registry holding a plug-in host's factory as the audio host's does,
/// from 1.1.1.
fn as_the_engine_plays(document: &Document, media: Arc<dyn MediaLibrary>) -> Vec<f32> {
    let host = ClapHost::new();
    host.load(bl_test_clap::bundle()).unwrap();
    let mut registry = Registry::new();
    bl_devices::register(&mut registry);
    registry.add(host.factory());
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

// Spec: offline-plugin-renders R1, R3
#[test]
fn an_export_renders_the_plugin_as_the_engine_plays_it() {
    let scratch = Scratch::new("as-played");
    let (mut shell, device) = shell_with_a_plugin(&scratch, true);
    running(&mut shell, device, 0.5);
    let document = shell.editor().document().clone();

    let (exported, rendered) = render(
        &document,
        media(&shell),
        shell.plugin_hosts(),
        &spec(48_000, export::BLOCK),
        &mut |_| {},
    )
    .unwrap();
    assert!(rendered.unhosted.is_empty());
    let played = as_the_engine_plays(&document, media(&shell));
    assert_eq!(bits(&exported), bits(&played), "sample for sample");

    // Not passed through: the plug-in's gain is in it.
    let (through, _) = render(
        &document,
        media(&shell),
        Hosts::default(),
        &spec(48_000, export::BLOCK),
        &mut |_| {},
    )
    .unwrap();
    assert!(through.iter().any(|s| *s != 0.0));
    for (out, dry) in exported.iter().zip(&through) {
        assert!((out - dry * 0.5).abs() <= 1e-6, "{out} against {dry}");
    }

    // The command an agent and `bl-cli export` run writes it to the file:
    // at a gain of 1, the peak is 6 dB higher.
    let export = |shell: &mut Shell, name: &str| -> ExportResult {
        let path = scratch.path(name);
        serde_json::from_value(
            shell
                .run(
                    export_ids::AUDIO,
                    json!({ "path": path.to_string_lossy(), "bit_depth": 32,
                            "sample_rate": 48000 }),
                )
                .unwrap(),
        )
        .unwrap()
    };
    let half = export(&mut shell, "half.wav");
    let file = bl_media::decode_file(scratch.path("half.wav")).unwrap();
    let left = file.channel(0).unwrap();
    for (n, sample) in left.iter().take(FRAMES as usize).enumerate() {
        assert_eq!(sample.to_bits(), exported[n * 2].to_bits(), "frame {n}");
    }
    shell
        .run(
            "device.set_param",
            json!({ "device": device, "param": GAIN, "value": 1.0 }),
        )
        .unwrap();
    let whole = export(&mut shell, "whole.wav");
    let difference = whole.peak_dbfs.unwrap() - half.peak_dbfs.unwrap();
    assert!((difference - 6.0206).abs() < 0.01, "{difference}");
}

// Spec: offline-plugin-renders R2, R4
#[test]
fn a_render_at_another_rate_and_block_reactivates_the_plugin_and_gives_it_back() {
    let scratch = Scratch::new("rates");
    let (mut shell, device) = shell_with_a_plugin(&scratch, true);
    let live = running(&mut shell, device, 0.5);
    let engine = live.active_for.expect("active for the engine");
    let state = shell.plugins().save_state(device).unwrap();

    let document = shell.editor().document().clone();
    let plugins = shell.plugins();
    let mut during = None;
    render(
        &document,
        media(&shell),
        shell.plugin_hosts(),
        &spec(96_000, 100),
        &mut |_| {
            if during.is_none() {
                during = plugins.status(device).and_then(|s| s.active_for);
            }
        },
    )
    .unwrap();
    assert_eq!(
        during,
        Some(NodeConfig {
            round_trip_latency: 0,
            sample_rate: SampleRate::new(96_000).unwrap(),
            max_frames: 100,
            channels: 2,
        }),
        "reactivated for the render"
    );

    // Back in the engine, at its own settings, with its state.
    let after = running(&mut shell, device, 0.5);
    assert_eq!(after.active_for, Some(engine));
    assert_eq!(after.failure, None);
    assert_eq!(after.values, live.values);
    assert_eq!(shell.plugins().save_state(device).unwrap(), state);
}

// Spec: offline-plugin-renders R1
#[test]
fn a_set_without_a_plugin_renders_without_borrowing_from_the_host() {
    // Its render takes nothing from the host, so the engine playing it is
    // left alone: it renders even while the host's plug-ins are out on
    // another loan, and exactly as it does with no host at all.
    let scratch = Scratch::new("no-plugin");
    let mut shell = Shell::headless();
    shell.start_audio();
    let file = scratch.path("Ramp.wav");
    wav(&file);
    shell
        .run(
            browser_commands::ADD_TO_ARRANGEMENT,
            json!({ "paths": [file.to_string_lossy()], "at": 0 }),
        )
        .unwrap();
    let document = shell.editor().document().clone();
    let without = render(
        &document,
        media(&shell),
        Hosts::default(),
        &spec(48_000, 256),
        &mut |_| {},
    )
    .unwrap()
    .0;
    assert!(without.iter().any(|s| *s != 0.0));

    let plugins = shell.plugins();
    let other = plugins.lend().unwrap();
    let (with, _) = render(
        &document,
        media(&shell),
        shell.plugin_hosts(),
        &spec(48_000, 256),
        &mut |_| {},
    )
    .unwrap();
    drop(other);
    assert_eq!(bits(&with), bits(&without));
}

// Spec: offline-plugin-renders R6
#[test]
fn a_plugin_that_fails_mid_export_goes_silent_and_the_export_completes() {
    let scratch = Scratch::new("failure");
    let (mut shell, device) = shell_with_a_plugin(&scratch, true);
    running(&mut shell, device, 0.5);
    let document = shell.editor().document().clone();
    let history = shell.editor().undo_history().len();
    let plugins = shell.plugins();
    let path = scratch.path("failing.wav");
    let mut blocks = 0;
    let report = export::export(
        &document,
        media(&shell),
        shell.plugin_hosts(),
        &Job {
            path: path.clone(),
            file_type: bl_media::FileType::Wav,
            bit_depth: bl_media::BitDepth::Float32,
            dither: bl_media::Dither::None,
            sample_rate: 48_000,
            engine_rate: 48_000,
            normalize: false,
            start: Ticks::ZERO,
            length: Length::Frames(FRAMES),
            replace: false,
            render_as_loop: false,
            block: export::BLOCK,
        },
        &AtomicBool::new(false),
        &mut |_| {
            // After the first block, the plug-in starts failing.
            blocks += 1;
            if blocks == 1 {
                plugins
                    .load_state(device, bl_test_clap::FAIL_PROCESS)
                    .unwrap();
            }
        },
    )
    .expect("the export completes");
    assert_eq!(report.frames, FRAMES);
    assert_eq!(report.heap_operations, 0);

    let file = bl_media::decode_file(&path).unwrap();
    let left = file.channel(0).unwrap();
    assert!(
        left[..export::BLOCK].iter().any(|s| *s != 0.0),
        "the first block"
    );
    assert!(
        left[export::BLOCK..].iter().all(|s| *s == 0.0),
        "silent from then on"
    );

    // In the engine it's silent from then on too, and says why; the set is
    // as it was.
    shell.poll();
    let status = shell.plugins().status(device).unwrap();
    assert!(!status.active);
    assert!(status.failure.unwrap().contains("offline render"));
    assert_eq!(shell.editor().undo_history().len(), history);
}

// Spec: offline-plugin-renders R4
#[test]
fn a_cancelled_export_gives_every_plugin_back() {
    let scratch = Scratch::new("cancel");
    let (mut shell, device) = shell_with_a_plugin(&scratch, true);
    let live = running(&mut shell, device, 0.5);
    let state = shell.plugins().save_state(device).unwrap();
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
            render_as_loop: false,
            block: export::BLOCK,
        },
        &cancel,
        &mut |_| cancel.store(true, Relaxed),
    );
    assert_eq!(result.unwrap_err(), ExportError::Cancelled);
    assert!(!scratch.path("cancelled.wav").exists());

    let after = running(&mut shell, device, 0.5);
    assert_eq!(after.active_for, live.active_for);
    assert_eq!(shell.plugins().save_state(device).unwrap(), state);
    // And the next render can borrow it again.
    assert!(shell.plugins().lend().is_ok());
}

// Spec: offline-plugin-renders R9
#[test]
fn renders_with_a_plugin_are_the_same_every_time_and_at_any_block_size() {
    let scratch = Scratch::new("determinism");
    // No audio device, as `bl-cli export` runs: the render makes the
    // device's plug-in itself.
    let (shell, _) = shell_with_a_plugin(&scratch, false);
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
    assert!(first.iter().any(|s| *s != 0.0));
    for block in [export::BLOCK, export::BLOCK, 100, 1024] {
        assert_eq!(bits(&once(block)), bits(&first), "block {block}");
    }
}

// Spec: offline-plugin-renders R7, R8
#[test]
fn a_preview_waits_for_the_song_to_stop_and_an_export_stops_it() {
    let scratch = Scratch::new("playing");
    let (mut shell, device) = shell_with_a_plugin(&scratch, true);
    running(&mut shell, device, 0.5);
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
        "{refused:?}"
    );

    // An export stops the song, as Live's does, and leaves it stopped.
    shell
        .run(
            export_ids::AUDIO,
            json!({ "path": scratch.path("while-playing.wav").to_string_lossy(),
                    "bit_depth": 24, "sample_rate": 48000 }),
        )
        .unwrap();
    until(&mut shell, false);
    running(&mut shell, device, 0.5);

    // Stopped, the preview renders.
    shell
        .run("perception.preview", json!({ "length": 705_600 }))
        .unwrap();
    running(&mut shell, device, 0.5);
}

/// A shell holding a one-second clip at 1.1.1 on a track with the kit's
/// Example Gain plug-in at a gain of 0.5.
fn shell_with_a_kit_device(scratch: &Scratch) -> (Shell, DeviceId) {
    let mut shell = Shell::headless();
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
    let types = shell.kit().device_types();
    let gain_type = types
        .iter()
        .find(|t| t.name == "Example Gain")
        .expect("Example Gain device type");
    let added = shell
        .run(
            "device.add",
            json!({
                "device": {
                    "kind": gain_type.kind,
                    "name": gain_type.name,
                    "role": gain_type.role,
                },
                "track": track,
            }),
        )
        .unwrap();
    let device: DeviceId =
        serde_json::from_value(added["device"].clone()).expect("the new device ID");
    shell
        .run(
            "device.set_param",
            json!({ "device": device, "param": lspk_example_gain::GAIN, "value": 0.5 }),
        )
        .unwrap();
    (shell, device)
}

// Spec: device-framework R21, offline-plugin-renders R1, R5
#[test]
fn an_export_renders_a_kit_device_and_stays_off_the_heap() {
    let scratch = Scratch::new("kit-device");
    // A dry set without the device:
    let file = scratch.path("Ramp.wav");
    wav(&file);
    let mut dry_shell = Shell::headless();
    dry_shell
        .run(
            browser_commands::ADD_TO_ARRANGEMENT,
            json!({ "paths": [file.to_string_lossy()], "at": 0 }),
        )
        .unwrap();
    let dry_doc = dry_shell.editor().document().clone();
    let (dry, _) = render(
        &dry_doc,
        media(&dry_shell),
        Hosts::default(),
        &spec(48_000, export::BLOCK),
        &mut |_| {},
    )
    .unwrap();
    assert!(dry.iter().any(|s| *s != 0.0), "the dry clip plays");

    // The set with Example Gain at 0.5:
    let (mut shell, device) = shell_with_a_kit_device(&scratch);
    let document = shell.editor().document().clone();

    let (exported, rendered) = render(
        &document,
        media(&shell),
        Hosts::default(),
        &spec(48_000, export::BLOCK),
        &mut |_| {},
    )
    .unwrap();
    assert!(
        rendered.unhosted.is_empty(),
        "kit device is hosted in the render"
    );
    assert_eq!(
        rendered.heap_operations, 0,
        "the real-time rules: 0 heap violations"
    );
    assert_eq!(exported.len(), dry.len());
    for (out, input) in exported.iter().zip(&dry) {
        assert!((out - input * 0.5).abs() <= 1e-6, "{out} against {input}");
    }

    // Running `export.audio` writes the processed audio to disk:
    let path = scratch.path("kit_gain.wav");
    let result: ExportResult = serde_json::from_value(
        shell
            .run(
                export_ids::AUDIO,
                json!({
                    "path": path.to_string_lossy(),
                    "bit_depth": 32,
                    "sample_rate": 48_000,
                }),
            )
            .unwrap(),
    )
    .unwrap();
    let file = bl_media::decode_file(&path).unwrap();
    let left = file.channel(0).unwrap();
    for (n, sample) in left.iter().take(FRAMES as usize).enumerate() {
        assert_eq!(sample.to_bits(), exported[n * 2].to_bits(), "frame {n}");
    }

    // At gain 1.0, the peak is ~6.02 dB higher:
    shell
        .run(
            "device.set_param",
            json!({ "device": device, "param": lspk_example_gain::GAIN, "value": 1.0 }),
        )
        .unwrap();
    let whole_path = scratch.path("kit_gain_whole.wav");
    let whole_result: ExportResult = serde_json::from_value(
        shell
            .run(
                export_ids::AUDIO,
                json!({
                    "path": whole_path.to_string_lossy(),
                    "bit_depth": 32,
                    "sample_rate": 48_000,
                }),
            )
            .unwrap(),
    )
    .unwrap();
    let difference = whole_result.peak_dbfs.unwrap() - result.peak_dbfs.unwrap();
    assert!((difference - 6.0206).abs() < 0.01, "{difference}");
}

// Spec: offline-plugin-renders R5
#[test]
fn renders_with_a_kit_device_are_the_same_every_time_and_at_any_block_size() {
    let scratch = Scratch::new("kit-determinism");
    let (shell, _) = shell_with_a_kit_device(&scratch);
    let document = shell.editor().document().clone();
    let once = |block| {
        render(
            &document,
            media(&shell),
            Hosts::default(),
            &spec(48_000, block),
            &mut |_| {},
        )
        .unwrap()
        .0
    };
    let first = once(export::BLOCK);
    assert!(first.iter().any(|s| *s != 0.0));
    for block in [export::BLOCK, 100, 1024, 64] {
        assert_eq!(bits(&once(block)), bits(&first), "block {block}");
    }
}

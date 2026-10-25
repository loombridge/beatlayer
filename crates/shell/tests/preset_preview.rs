//! Previewing instrument presets in the browser (WP-6.67; the browser spec,
//! R27 and R28). Driven through `Shell::run`, as an agent drives it (D17), on
//! real preset files.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use bl_audio_io::NullControl;
use bl_core::id::Id;
use bl_core::time::{Bpm, TICKS_PER_QUARTER, TempoMap, Ticks};
use bl_media::SampleData;
use bl_model::{
    Chain, ChainId, Clip, ClipData, Command, Device, DeviceId, DeviceKind, DeviceRole, Editor,
    MidiData, Note, ParamId, PluginFormat, PluginState, RackKind, TrackId, TrackKind,
};
use bl_project::preset::Preset;
use bl_project::rack_preset::{RackPreset, Samples};
use bl_shell::{Shell, testing};
use serde_json::json;

const BEAT: i64 = TICKS_PER_QUARTER;

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-preset-preview-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn library(&self) -> PathBuf {
        self.0.join("User Library")
    }

    /// Where the preset `name` goes in the User Library.
    fn preset(&self, name: &str) -> PathBuf {
        let folder = self.library().join("Presets");
        std::fs::create_dir_all(&folder).unwrap();
        folder.join(format!("{name}.blpreset"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn id(n: u64) -> Id {
    Id::new(n).unwrap()
}

/// The native polysynth, an instrument.
fn synth(n: u64) -> Device {
    let t = bl_devices::polysynth_type();
    Device::new(DeviceId(id(n)), t.name.to_string(), t.kind).with_role(t.role)
}

/// The test plug-in's gain, as a set saves it: with the bundle it came from.
fn clap_gain(n: u64, gain: f64) -> Device {
    let mut device = Device::new(
        DeviceId(id(n)),
        bl_test_clap::PLUGIN_NAME,
        DeviceKind::Plugin {
            format: PluginFormat::Clap,
            plugin_id: bl_test_clap::PLUGIN_ID.to_owned(),
        },
    );
    device.plugin = Some(PluginState {
        path: text(bl_test_clap::bundle()),
        data: Vec::new(),
    });
    device.params.insert(ParamId(bl_test_clap::GAIN), gain);
    device
}

fn chain(n: u64, devices: Vec<Device>) -> Chain {
    let mut chain = Chain::new(ChainId(id(n)), "Chain");
    chain.devices = devices;
    chain
}

/// A Drum Rack of pads: each a note, the note its devices play, and its
/// devices.
fn drum_rack(pads: &[(u8, u8, Vec<Device>)]) -> Device {
    let mut rack = Device::rack(DeviceId(id(100)), RackKind::Drum);
    for (n, (key, play, devices)) in (200..).zip(pads) {
        let mut pad = Chain::for_pad(ChainId(id(n)), "Pad", *key);
        pad.play = *play;
        pad.devices = devices.clone();
        rack.chains.push(pad);
    }
    rack
}

fn save_rack(scratch: &Scratch, name: &str, rack: Device) -> PathBuf {
    let path = scratch.preset(name);
    RackPreset::new(rack, Samples::new())
        .unwrap()
        .save(&path)
        .unwrap();
    path
}

fn save_device(scratch: &Scratch, name: &str, device: &Device) -> PathBuf {
    let path = scratch.preset(name);
    Preset::of(device).save(&path).unwrap();
    path
}

/// A shell with a User Library and the audio running.
fn rig(scratch: &Scratch) -> (Shell, NullControl) {
    let (mut shell, control) = testing::rig();
    bl_shell::browser::places::create_user_library(&scratch.library()).unwrap();
    shell.set_user_library(Some(scratch.library()));
    shell.start_audio();
    (shell, control)
}

/// Polls until the background render is in: it takes real time, which no
/// step of test time stands for.
fn render_done(shell: &mut Shell) {
    for _ in 0..5_000_000 {
        shell.poll();
        if shell.browser_preview().loading.is_none() {
            return;
        }
        std::thread::yield_now();
    }
    panic!("the render never finished");
}

fn select_and_render(shell: &mut Shell, file: &Path) {
    shell
        .run("browser.select", json!({ "paths": [text(file)] }))
        .unwrap();
    render_done(shell);
}

fn render_of(file: &Path, bpm: f64) -> Result<SampleData, String> {
    bl_shell::browser::preview::render_preset(file, 48_000, bpm, &AtomicBool::new(false))
        .map(|(data, _)| data)
}

/// What exporting a set of one MIDI track holding `device` and `notes` gives
/// for `frames` frames at 120 BPM: the left channel.
fn exported(mut device: Device, notes: Vec<Note>, length: Ticks, frames: usize) -> Vec<f32> {
    let mut editor = Editor::new();
    editor
        .apply(Command::SetTempo {
            tempo: TempoMap::constant(Bpm::new(120.0).unwrap()),
        })
        .unwrap();
    device.reassign_ids(&mut || editor.new_id::<DeviceId>().0);
    let track: TrackId = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: "Reference".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    let clip = Clip::new(
        editor.new_id(),
        Ticks::ZERO,
        length,
        ClipData::Midi(MidiData::new(notes)),
    );
    editor.apply(Command::CreateClip { track, clip }).unwrap();
    let mut samples = Vec::new();
    bl_shell::export::render(
        editor.document(),
        std::sync::Arc::new(bl_media::MediaStore::new()),
        bl_shell::export::Hosts::default(),
        &bl_shell::export::Render {
            start: Ticks::ZERO,
            length: bl_shell::export::Length::Frames(frames as u64),
            sample_rate: 48_000,
            engine_rate: 48_000,
            block: bl_shell::export::BLOCK,
        },
        60.0,
        &AtomicBool::new(false),
        &mut |_| {},
        &mut |block| {
            samples.extend_from_slice(block);
            Ok(())
        },
        None,
    )
    .unwrap();
    samples.iter().step_by(2).copied().collect()
}

// Spec: browser R27. A preset of a native instrument previews as its offline
// render of C3 at velocity 100 for a bar at the set's tempo, followed until
// the sound ends; it plays once, at once, whatever Raw says.
#[test]
fn an_instrument_preset_previews_as_its_render_of_the_phrase_once_whatever_raw_says() {
    let scratch = Scratch::new("synth");
    let (mut shell, control) = rig(&scratch);
    let file = save_device(&scratch, "Lead", &synth(1));

    // The render is what exporting the phrase gives, cut where the sound ends.
    let data = render_of(&file, 120.0).unwrap();
    let phrase = 4 * 24_000;
    let reference = exported(
        synth(1),
        vec![Note::new(60, Ticks::ZERO, Ticks(4 * BEAT), 100)],
        Ticks(4 * BEAT),
        2 * phrase,
    );
    assert!(data.frames() >= phrase && data.frames() <= 2 * phrase);
    assert!(reference.iter().any(|s| *s != 0.0), "the synth plays");
    assert_eq!(data.channel(0).unwrap(), &reference[..data.frames()]);
    let last = data.channel(0).unwrap()[data.frames() - 1];
    assert!(
        data.frames() == phrase || last.abs() >= 0.001,
        "it ends where the sound falls below -60 dBFS"
    );
    assert!(
        reference[data.frames()..].iter().all(|s| s.abs() < 0.001),
        "and nothing is lost after"
    );
    // At another tempo it's another length.
    assert!(render_of(&file, 90.0).unwrap().frames() >= 4 * 32_000);

    // While the song plays, a warped preview would wait for the next bar;
    // this one plays at once, with Raw off and on, and once.
    shell.run("transport.play", json!({})).unwrap();
    testing::advance(&control, &mut shell, 500);
    for raw in [false, true] {
        shell.run("browser.set_raw", json!({ "raw": raw })).unwrap();
        select_and_render(&mut shell, &file);
        testing::advance(&control, &mut shell, 100);
        shell.poll();
        let preview = shell.browser_preview();
        assert_eq!(preview.playing, Some(text(&file)), "{preview:?}");
        assert!(preview.position_seconds > 0.0, "{raw}: {preview:?}");
        assert!((preview.length_seconds - data.frames() as f64 / 48_000.0).abs() < 0.01);
        testing::wait_for(&control, &mut shell, "the preview to end", |shell| {
            shell.poll();
            shell.browser_preview().playing.is_none()
        });
        testing::advance(&control, &mut shell, 500);
        shell.poll();
        assert_eq!(shell.browser_preview().playing, None, "it plays once");
    }
    // The audio thread played them without touching the heap.
    let stats = shell.audio().stats().expect("a stream is open");
    assert_eq!(stats.heap_operations, 0, "{stats:?}");
}

/// How alike two stretches of sound are, from −1 to 1.
fn alike(a: &[f32], b: &[f32]) -> f64 {
    let dot: f64 = a.iter().zip(b).map(|(a, b)| f64::from(*a * *b)).sum();
    let size = |s: &[f32]| s.iter().map(|x| f64::from(x * x)).sum::<f64>().sqrt();
    dot / (size(a) * size(b)).max(1e-12)
}

// Spec: browser R27. A Drum Rack's preview plays each pad that holds a chain,
// lowest note first, a sixteenth note each.
#[test]
fn a_drum_rack_preset_plays_its_pads_in_order() {
    let scratch = Scratch::new("drums");
    // Three pads sounding an octave apart, listed out of order, and a pad
    // with no chain in it.
    let pad = |key, play, n| (key, play, vec![synth(n)]);
    let rack = drum_rack(&[
        pad(38, 48, 13),
        pad(36, 60, 11),
        (40, 60, Vec::new()),
        pad(37, 72, 12),
    ]);
    let file = save_rack(&scratch, "Pads", rack);
    let data = render_of(&file, 120.0).unwrap();
    let step = 6_000;
    assert!(data.frames() >= 3 * step);

    // Each pad on its own, as a rack of one.
    let alone: Vec<Vec<f32>> = [60, 72, 48]
        .iter()
        .enumerate()
        .map(|(n, play)| {
            let file = save_rack(
                &scratch,
                &format!("Alone {n}"),
                drum_rack(&[(36, *play, vec![synth(11)])]),
            );
            render_of(&file, 120.0).unwrap().channel(0).unwrap()[..step].to_vec()
        })
        .collect();
    let played = data.channel(0).unwrap();
    for pad in 0..3 {
        let heard = &played[pad * step..(pad + 1) * step];
        let likeness: Vec<f64> = alone.iter().map(|a| alike(heard, a)).collect();
        let best = likeness
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert_eq!(best, pad, "the sixteenth {pad} is pad {pad}: {likeness:?}");
    }
}

/// A rig with the polysynth then the test plug-in on a MIDI track of the set,
/// the plug-in's gain at `gain`.
fn set_with_plugin(scratch: &Scratch, gain: f64) -> (Shell, NullControl, DeviceId) {
    let (mut shell, control) = rig(scratch);
    let track = {
        let doc = shell.editor().document();
        doc.top_level()
            .find(|id| doc.track(*id).is_some_and(|t| t.kind() == TrackKind::Midi))
            .unwrap()
    };
    shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let loaded: bl_shell::plugin_commands::PluginLoaded = serde_json::from_value(
        shell
            .run(
                "plugin.load",
                json!({ "path": bl_test_clap::bundle(), "track": track }),
            )
            .unwrap(),
    )
    .unwrap();
    shell
        .run(
            "device.set_param",
            json!({ "device": loaded.device, "param": bl_test_clap::GAIN, "value": gain }),
        )
        .unwrap();
    testing::wait_for(&control, &mut shell, "the plug-in to run", |shell| {
        shell.poll();
        shell
            .plugins()
            .status(loaded.device)
            .is_some_and(|s| s.active)
    });
    (shell, control, loaded.device)
}

// Spec: browser R27, R28; offline plug-in renders R3, R11. A plug-in in an
// Instrument Rack preset is made fresh from its saved state: the set's own
// instance of the plug-in isn't touched.
#[test]
fn a_clap_plugin_in_a_rack_preset_renders_through_a_fresh_instance() {
    let scratch = Scratch::new("clap");
    let (shell, _control, device) = set_with_plugin(&scratch, 0.25);
    let before = (
        shell.plugins().status(device),
        shell.plugins().save_state(device).unwrap(),
        shell.editor().undo_history().len(),
    );

    let through = save_rack(
        &scratch,
        "Quiet",
        Device::rack(DeviceId(id(100)), RackKind::Instrument)
            .with_chain(chain(200, vec![synth(1), clap_gain(2, 0.5)])),
    );
    let plain = save_rack(
        &scratch,
        "Plain",
        Device::rack(DeviceId(id(100)), RackKind::Instrument)
            .with_chain(chain(200, vec![synth(1)])),
    );
    let (through, dry) = (
        render_of(&through, 120.0).unwrap(),
        render_of(&plain, 120.0).unwrap(),
    );
    assert!(dry.channel(0).unwrap().iter().any(|s| *s != 0.0));
    // Quieter, so its sound falls below -60 dBFS sooner.
    assert!(through.frames() <= dry.frames());
    for (out, input) in through
        .channel(0)
        .unwrap()
        .iter()
        .zip(dry.channel(0).unwrap())
    {
        assert!((out - input * 0.5).abs() <= 1e-6, "{out} against {input}");
    }
    assert_eq!(
        (
            shell.plugins().status(device),
            shell.plugins().save_state(device).unwrap(),
            shell.editor().undo_history().len(),
        ),
        before,
        "the set's instance and the set are as they were"
    );
    assert!(
        shell.plugins().lend().is_ok(),
        "nothing of the set's is lent"
    );
}

// Spec: browser R28. Selecting another item drops the render under way; a
// preset of an effect, and one whose plug-in this machine lacks, preview
// nothing and the strip says why; the set stays as it was.
#[test]
fn effects_and_missing_plugins_preview_nothing_and_another_item_drops_a_render() {
    let scratch = Scratch::new("none");
    let (mut shell, _control) = rig(&scratch);
    let steps = shell.editor().undo_history().len();
    let lead = save_device(&scratch, "Lead", &synth(1));
    let effect = save_device(
        &scratch,
        "Gain",
        &Device::new(
            DeviceId(id(2)),
            "Gain",
            bl_engine::testing::gain_type().kind,
        ),
    );
    let missing = save_device(
        &scratch,
        "Gone",
        &Device::new(
            DeviceId(id(3)),
            "Gone",
            DeviceKind::Plugin {
                format: PluginFormat::Clap,
                plugin_id: "io.beatlayer.test.not-installed".to_owned(),
            },
        )
        .with_role(DeviceRole::Instrument),
    );

    // The effect straight after the instrument: the render under way is
    // dropped, and the effect says why it plays nothing.
    shell
        .run("browser.select", json!({ "paths": [text(&lead)] }))
        .unwrap();
    select_and_render(&mut shell, &effect);
    let preview = shell.browser_preview();
    assert_eq!(preview.playing, None);
    assert!(
        preview
            .note
            .as_deref()
            .is_some_and(|n| n.contains("plays by itself")),
        "{preview:?}"
    );
    assert!(shell.message().is_some_and(|m| m.error));

    select_and_render(&mut shell, &missing);
    let preview = shell.browser_preview();
    assert_eq!(preview.playing, None);
    assert!(
        preview.note.as_deref().is_some_and(|n| n.contains("Gone")),
        "{preview:?}"
    );

    // An instrument that does play clears the note.
    select_and_render(&mut shell, &lead);
    let preview = shell.browser_preview();
    assert_eq!((preview.note, preview.playing), (None, Some(text(&lead))));

    // A render told to stop at once gives no audio.
    assert!(
        bl_shell::browser::preview::render_preset(&lead, 48_000, 120.0, &AtomicBool::new(true))
            .is_err()
    );
    assert_eq!(
        shell.editor().undo_history().len(),
        steps,
        "the set is as it was"
    );
}

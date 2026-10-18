//! SFZ import in the shell (WP-5.14; the multisampler spec, R47, R49 to
//! R52): `device.import_sfz` on the generated Sine Bank, its tracks, devices
//! and undo step, and what the instrument plays when it renders. Driven
//! through `Shell::run`, as an agent drives it (D17).

use std::path::{Path, PathBuf};

use bl_core::time::{SampleRate, Ticks};
use bl_devices::multisampler::{self as mosaic, MULTISAMPLER};
use bl_engine::{EngineConfig, Offline, Registry, TransportCommand};
use bl_model::{
    Clip, ClipData, Command, DeviceId, DeviceKind, DeviceRole, Layer, MidiData, Note, ReleaseMode,
    SustainMode, TrackId, TrackKind,
};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::multisampler_commands::SfzImported;
use bl_shell::settings::Preferences;
use serde_json::json;

/// The instrument `tests/fixtures/gen/gen_media.py --sfz` writes.
fn sine_kit() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sfz/Sine Bank.sfz")
}

fn shell() -> Shell {
    Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    )
}

fn import(shell: &mut Shell, args: serde_json::Value) -> SfzImported {
    serde_json::from_value(shell.run("device.import_sfz", args).unwrap()).unwrap()
}

fn layers(shell: &Shell, device: DeviceId) -> Vec<Layer> {
    shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .layers
        .list
        .clone()
}

fn value(shell: &Shell, device: DeviceId, param: bl_model::ParamId) -> f64 {
    let found = shell.editor().document().device(device).unwrap();
    *found.params.get(&param).unwrap_or(&f64::NAN)
}

fn midi_tracks(shell: &Shell) -> Vec<TrackId> {
    shell
        .editor()
        .document()
        .tracks()
        .filter(|track| track.kind() == TrackKind::Midi)
        .map(bl_model::Track::id)
        .collect()
}

// Spec: multisampler R47
#[test]
fn an_sfz_makes_a_multisampler_on_a_new_midi_track_as_one_undo_step() {
    let mut shell = shell();
    let tracks = midi_tracks(&shell).len();
    let history = shell.editor().undo_history().len();
    let imported = import(&mut shell, json!({ "path": sine_kit().to_string_lossy() }));
    assert_eq!(midi_tracks(&shell).len(), tracks + 1, "a new MIDI track");
    assert_eq!(
        shell.editor().undo_history().len(),
        history + 1,
        "one undo step"
    );
    let doc = shell.editor().document();
    let track = doc.track(imported.track).unwrap();
    assert_eq!(track.name(), "Sine Bank");
    let device = doc.device(imported.device).unwrap();
    assert!(
        matches!(&device.kind, DeviceKind::Native { device_type } if device_type == MULTISAMPLER)
    );
    assert_eq!(device.role, DeviceRole::Instrument);
    // Five regions are layers, in the order a round robin takes them; the
    // sixth names a sample that isn't there.
    let list = layers(&shell, imported.device);
    let names: Vec<_> = list.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["low_soft", "low_hard", "mid_a", "mid_b", "high"]);
    assert_eq!(imported.layers.len(), 5);
    assert_eq!(imported.left_out.len(), 1);
    assert_eq!(imported.left_out[0].sample, "missing.wav");
    assert_eq!(imported.left_out[0].region, 6);
    let unread: Vec<_> = imported.unread.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(unread, ["group"], "group is read by nothing");
    let settled: Vec<_> = imported
        .settled
        .iter()
        .map(|c| (c.name.as_str(), c.regions))
        .collect();
    assert_eq!(
        settled,
        [("amp_veltrack", 6), ("ampeg_release", 6)],
        "of the six regions the file has"
    );
    // The settings most regions agree on set the device's controls, and a
    // sequence turns Round Robin on in Forward mode.
    assert_eq!(value(&shell, imported.device, mosaic::VEL_VOL), 0.0);
    assert!((value(&shell, imported.device, mosaic::ENV_RELEASE) - 50.0).abs() < 1e-9);
    assert_eq!(value(&shell, imported.device, mosaic::ROUND_ROBIN), 1.0);
    assert_eq!(value(&shell, imported.device, mosaic::RR_MODE), 0.0);
    // Zones, roots and loops.
    let [soft, hard, mid_a, mid_b, high] = &list[..] else {
        panic!("{names:?}");
    };
    assert_eq!((soft.key.low, soft.key.high, soft.root_key), (36, 47, 40));
    assert_eq!((soft.velocity.low, soft.velocity.high), (1, 63));
    assert_eq!((hard.velocity.low, hard.velocity.high), (64, 127));
    assert_eq!(
        (mid_a.key.low, mid_a.key.high, mid_a.key.fade_high),
        (48, 59, 3)
    );
    assert_eq!(mid_b.root_key, 54);
    assert_eq!(
        (high.key.low, high.key.high, high.key.fade_low),
        (56, 71, 3)
    );
    assert_eq!(
        (high.sustain, high.release),
        (SustainMode::Loop, ReleaseMode::Off)
    );
    assert_eq!((high.loop_start, high.loop_end), (0, 4800));
    assert_eq!(soft.sustain, SustainMode::Off);
    assert!(!high.one_shot);
    // The new layers are selected and the device is the selected one.
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(midi_tracks(&shell).len(), tracks, "undone in one step");
    assert!(shell.editor().document().device(imported.device).is_none());
    shell.run("edit.redo", json!({})).unwrap();
    assert_eq!(layers(&shell, imported.device).len(), 5);
}

// Spec: multisampler R47
#[test]
fn an_sfz_goes_on_a_given_midi_track_or_adds_layers_to_a_given_multisampler() {
    let mut shell = shell();
    let midi = midi_tracks(&shell)[0];
    let tracks = midi_tracks(&shell).len();
    let on_track = import(
        &mut shell,
        json!({ "path": sine_kit().to_string_lossy(), "track": midi }),
    );
    assert_eq!(on_track.track, midi, "on the track given");
    assert_eq!(midi_tracks(&shell).len(), tracks);
    // Given the multisampler, only layers come, at the end of the list.
    let history = shell.editor().undo_history().len();
    let added = import(
        &mut shell,
        json!({ "path": sine_kit().to_string_lossy(), "device": on_track.device }),
    );
    assert_eq!(added.device, on_track.device);
    assert_eq!(layers(&shell, on_track.device).len(), 10);
    assert_eq!(shell.editor().undo_history().len(), history + 1);
    // An audio track can't hold the multisampler.
    let audio = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .unwrap();
    let error = shell
        .run(
            "device.import_sfz",
            json!({ "path": sine_kit().to_string_lossy(), "track": audio }),
        )
        .unwrap_err();
    assert!(error.to_string().contains("MIDI track"), "{error}");
    // A device that isn't a multisampler, a missing file and a file with no
    // region left change nothing.
    let before = shell.editor().undo_history().len();
    let empty = std::env::temp_dir().join(format!("bl-empty-{}.sfz", std::process::id()));
    std::fs::write(&empty, "<region> sample=nothing.wav\n").unwrap();
    for args in [
        json!({ "path": "/definitely/not/here.sfz" }),
        json!({ "path": empty.to_string_lossy() }),
    ] {
        assert!(shell.run("device.import_sfz", args).is_err());
    }
    std::fs::remove_file(&empty).ok();
    assert_eq!(shell.editor().undo_history().len(), before);
}

/// Renders `frames` frames of the shell's document at 48 kHz offline with
/// `notes` (key, start in eighths of a whole note, length in the same,
/// velocity) on the multisampler's track, and returns the first channel.
fn render(shell: &Shell, device: DeviceId, notes: &[(u8, i64, i64, u8)], frames: u64) -> Vec<f32> {
    let mut editor =
        bl_model::Editor::from_document(shell.editor().document().clone(), shell.editor().ids());
    let track = editor.document().device_track(device).unwrap();
    let clip = editor.new_id();
    let eighth = Ticks::WHOLE.get() / 8;
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks::WHOLE * 8,
                ClipData::Midi(MidiData::new(notes.iter().map(
                    |&(key, start, length, velocity)| {
                        Note::new(key, Ticks(start * eighth), Ticks(length * eighth), velocity)
                    },
                ))),
            ),
        })
        .unwrap();
    let mut registry = Registry::new();
    bl_devices::register(&mut registry);
    let registry = registry.with_media(shell.media_store());
    let mut offline = Offline::new(EngineConfig::new(SampleRate::HZ_48000, 512), registry);
    assert!(offline.sync(editor.document()).is_empty());
    offline.sender().transport(TransportCommand::Play);
    let mut out = Vec::new();
    offline.render(frames, |block| out.extend_from_slice(block.channel(0)));
    assert_eq!(offline.heap_operations(), 0, "no allocation in a block");
    assert!(offline.missing_media().is_empty());
    out
}

/// The amplitude of `hz` in `signal`'s `frames` frames from `from`, by a
/// least squares fit, at 48 kHz.
fn amplitude(signal: &[f32], from: usize, frames: usize, hz: f64) -> f64 {
    let (mut cc, mut ss, mut cs, mut xc, mut xs) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for i in 0..frames {
        let (s, c) = (core::f64::consts::TAU * hz * (from + i) as f64 / 48_000.0).sin_cos();
        let x = f64::from(signal[from + i]);
        cc += c * c;
        ss += s * s;
        cs += c * s;
        xc += x * c;
        xs += x * s;
    }
    let det = cc * ss - cs * cs;
    ((xc * ss - xs * cs) / det).hypot((xs * cc - xc * cs) / det)
}

// Spec: multisampler R49
#[test]
fn a_generated_sfz_plays_as_its_file_says() {
    let mut shell = shell();
    let device = import(&mut shell, json!({ "path": sine_kit().to_string_lossy() })).device;
    // Notes an eighth of a whole note (0.25 s, 12,000 frames) apart, each
    // an eighth long, so a sample of 4,800 frames has ended before the next.
    let notes = [
        (40, 0, 1, 30),  // the soft layer at its root: 110 Hz
        (40, 1, 1, 110), // the hard layer: 165 Hz
        (54, 2, 1, 100), // the round robin's first: 330 Hz
        (54, 3, 1, 100), // its second: 495 Hz
        (54, 4, 1, 100), // and round again: 330 Hz
        (57, 5, 1, 100), // across the crossfade
        (64, 6, 1, 100), // the loop
    ];
    let out = render(&shell, device, &notes, 12_000 * 8);
    let at = |note: usize| note * 12_000 + 600;
    let level = 0.5;
    // Two velocity layers: each at its own pitch, and only one of them.
    for (note, hz, other) in [(0, 110.0, 165.0), (1, 165.0, 110.0)] {
        let a = amplitude(&out, at(note), 3600, hz);
        assert!((a - level).abs() < 0.02, "note {note}: {a}");
        assert!(amplitude(&out, at(note), 3600, other) < 0.08, "note {note}");
    }
    // A round robin: one sample a note, in order.
    for (note, hz) in [(2, 330.0), (3, 495.0), (4, 330.0)] {
        let a = amplitude(&out, at(note), 3600, hz);
        assert!((a - level).abs() < 0.02, "round robin {note}: {a}");
    }
    // Key 57, three keys into the crossfade of 56 to 59: the middle zone's
    // sample (the round robin's turn: the second) at 2/3 and the high zone's
    // at 1/3 of the way, constant-power.
    let mid = amplitude(&out, at(5), 3600, 495.0 * 2f64.powf(3.0 / 12.0));
    let high = amplitude(&out, at(5), 3600, 660.0 * 2f64.powf(-7.0 / 12.0));
    let law = |share: f64| (share * core::f64::consts::FRAC_PI_2).sin() * level;
    assert!((mid - law(2.0 / 3.0)).abs() < 0.03, "{mid}");
    assert!((high - law(1.0 / 3.0)).abs() < 0.03, "{high}");
    // The loop holds a note on past its sample's 4,800 frames: the same
    // note without a loop (the middle zone's) has ended by then.
    let held = [(64, 0, 4, 100), (54, 4, 4, 100)];
    let out = render(&shell, device, &held, 12_000 * 8);
    assert!(
        amplitude(&out, 20_000, 9_600, 660.0) > 0.45,
        "the loop sustains"
    );
    let late = 12_000 * 4 + 20_000;
    assert!(
        out[late..late + 9_600].iter().all(|s| s.abs() < 0.01),
        "no loop, no sound"
    );
}

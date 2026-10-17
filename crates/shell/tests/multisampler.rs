//! The multisampler in the shell (WP-5.13; the multisampler spec, R2, R3,
//! R7, R11, R13, R20, R29, R30): adding layers from files, the list's
//! edits, normalising, Snap, Auto Select, a set with 512 layers, and the
//! title. Driven through `Shell::run`, as an agent drives it (D17), on real
//! files and bl-project's real set files.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bl_core::time::{SampleRate, Ticks};
use bl_devices::multisampler::MULTISAMPLER;
use bl_engine::{EngineConfig, Offline, Registry, TransportCommand};
use bl_media::BitDepth;
use bl_model::{
    Clip, ClipData, Command, DeviceId, DeviceKind, DeviceRole, Layer, LayerId, MidiData, Note,
    TrackKind,
};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::device_commands::DeviceAdded;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use serde_json::{Value, json};

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-multisampler-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a mono 16-bit WAV of `frames` frames of `value`, with the
/// sampler chunk a file may carry: its root key and one loop.
fn wav(path: &Path, frames: usize, value: f32, smpl: Option<(u32, (u32, u32))>) {
    let samples = vec![value; frames];
    let mut bytes =
        bl_media::wav_bytes(&samples, 1, 48_000, BitDepth::Int16, bl_media::Dither::None).unwrap();
    if let Some((key, (start, end))) = smpl {
        let mut chunk = Vec::new();
        for word in [0, 0, 20_833, key, 0, 0, 0, 1, 0, 0, 0, start, end, 0, 0] {
            chunk.extend_from_slice(&u32::to_le_bytes(word));
        }
        let at = bytes.windows(4).position(|w| w == b"data").unwrap();
        let mut head = bytes[..at].to_vec();
        head.extend_from_slice(b"smpl");
        head.extend_from_slice(&u32::to_le_bytes(chunk.len() as u32));
        head.extend_from_slice(&chunk);
        head.extend_from_slice(&bytes[at..]);
        let riff = head.len() as u32 - 8;
        head[4..8].copy_from_slice(&riff.to_le_bytes());
        bytes = head;
    }
    std::fs::write(path, bytes).unwrap();
}

fn shell(scratch: &Scratch) -> Shell {
    Shell::new(
        Preferences::default(),
        Some(scratch.path("config")),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    )
}

/// Adds a multisampler to the default set's MIDI track, as `device.add`
/// does from the browser.
fn add_multisampler(shell: &mut Shell) -> DeviceId {
    let midi = shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Midi)
        .map(bl_model::Track::id)
        .expect("the default set has a MIDI track");
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [midi] }))
        .unwrap();
    let added: DeviceAdded = serde_json::from_value(
        shell
            .run(
                "device.add",
                json!({
                    "device": {
                        "kind": DeviceKind::Native { device_type: MULTISAMPLER.to_owned() },
                        "name": "Mosaic",
                        "role": DeviceRole::Instrument,
                    }
                }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
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

fn add_layers(shell: &mut Shell, device: DeviceId, files: &[&Path]) -> Value {
    shell
        .run(
            "device.add_layers",
            json!({ "device": device, "paths": files.iter().map(|f| f.to_string_lossy()).collect::<Vec<_>>() }),
        )
        .unwrap()
}

fn state(shell: &mut Shell, device: DeviceId) -> Value {
    // The Device View shows the selected track's chain.
    let track = shell.editor().document().device_track(device).unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let state = shell.run("device_view.state", json!({})).unwrap();
    state["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == json!(device))
        .unwrap()["multisampler"]
        .clone()
}

// Spec: multisampler R3
#[test]
fn added_layers_take_the_whole_ranges_and_the_files_root_key() {
    let scratch = Scratch::new("add");
    let (with, without) = (scratch.path("Pad C.wav"), scratch.path("Bare.wav"));
    wav(&with, 4800, 0.5, Some((64, (1000, 2999))));
    wav(&without, 2400, 0.25, None);
    let mut shell = shell(&scratch);
    let device = add_multisampler(&mut shell);
    let before = shell.editor().undo_history().len();

    let added = add_layers(&mut shell, device, &[&with, &without]);
    assert_eq!(added["skipped"], json!([]));
    let layers = layers(&shell, device);
    assert_eq!(layers.len(), 2);
    let (a, b) = (&layers[0], &layers[1]);
    assert_eq!((a.name.as_str(), b.name.as_str()), ("Pad C", "Bare"));
    for layer in [a, b] {
        assert_eq!((layer.key.low, layer.key.high), (0, 127));
        assert_eq!((layer.velocity.low, layer.velocity.high), (1, 127));
        assert_eq!((layer.select.low, layer.select.high), (0, 127));
        assert_eq!(
            (layer.key.fade_low, layer.key.fade_high, layer.velocity.fade_low),
            (0, 0, 0),
            "no fades"
        );
        assert_eq!(layer.sustain, bl_model::SustainMode::Off, "no loop played");
        assert_eq!((layer.start, layer.end), (0, layer.length));
    }
    assert_eq!(a.root_key, 64, "the file's sampler chunk");
    assert_eq!((a.loop_start, a.loop_end), (1000, 3000), "its first loop");
    assert_eq!(b.root_key, 60, "C3 where the file names none");
    assert_eq!((b.loop_start, b.loop_end, b.length), (0, 2400, 2400));
    assert_eq!(
        shell.editor().undo_history().len(),
        before + 1,
        "one undo step"
    );
    // The new layers are the selection, and a file that isn't audio is
    // named and skipped.
    assert_eq!(state(&mut shell, device)["selection"].as_array().unwrap().len(), 2);
    let junk = scratch.path("notes.txt");
    std::fs::write(&junk, "not audio").unwrap();
    let again = add_layers(&mut shell, device, &[&without, &junk]);
    assert_eq!(again["skipped"].as_array().unwrap().len(), 1);
    assert_eq!(layers_len(&shell, device), 3);
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(layers_len(&shell, device), 2);
}

/// Saves the set in the scratch folder and says where it went.
fn save_as(shell: &mut Shell, scratch: &Scratch) -> PathBuf {
    let saved: bl_shell::commands::SetResult = serde_json::from_value(
        shell
            .run(
                "set.save_as",
                json!({ "path": scratch.path("Song.blset").to_string_lossy() }),
            )
            .unwrap(),
    )
    .unwrap();
    PathBuf::from(saved.path.expect("the set was saved"))
}

fn layers_len(shell: &Shell, device: DeviceId) -> usize {
    layers(shell, device).len()
}

/// Renders `frames` frames of the shell's document at 48 kHz offline, with
/// a note at C3 held for the whole of it on the multisampler's track, and
/// returns the first channel and whether any media was missing.
fn play(shell: &Shell, device: DeviceId, frames: u64) -> (Vec<f32>, usize) {
    let mut editor = bl_model::Editor::from_document(
        shell.editor().document().clone(),
        shell.editor().ids(),
    );
    let track = editor.document().device_track(device).unwrap();
    let clip = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks::WHOLE * 8,
                ClipData::Midi(MidiData::new([Note::new(60, Ticks::ZERO, Ticks::WHOLE * 8, 100)])),
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
    (out, offline.missing_media().len())
}

// Spec: multisampler R2
#[test]
fn five_hundred_and_twelve_layers_play_save_and_reopen() {
    let scratch = Scratch::new("many");
    let file = scratch.path("Tick.wav");
    wav(&file, 4800, 0.001, None);
    let mut shell = shell(&scratch);
    let device = add_multisampler(&mut shell);
    let paths: Vec<String> = (0..512).map(|_| file.to_string_lossy().into_owned()).collect();
    shell
        .run("device.add_layers", json!({ "device": device, "paths": paths }))
        .unwrap();
    assert_eq!(layers_len(&shell, device), 512);
    // Each layer plays: 512 voices of 0.001 sum to about half.
    let (out, missing) = play(&shell, device, 4000);
    assert_eq!(missing, 0);
    assert!((out[2000] - 0.512).abs() < 0.02, "{}", out[2000]);

    let set = save_as(&mut shell, &scratch);
    shell
        .run("set.open", json!({ "path": set.to_string_lossy() }))
        .unwrap();
    assert_eq!(layers_len(&shell, device), 512, "the set keeps them");
    let (out, missing) = play(&shell, device, 4000);
    assert_eq!(missing, 0, "the media table found the file again");
    assert!((out[2000] - 0.512).abs() < 0.02);
    // Each edit undoes as one step.
    shell
        .run(
            "device.rename_layer",
            json!({ "device": device, "layer": 3, "name": "Third" }),
        )
        .unwrap();
    shell.run("edit.undo", json!({})).unwrap();
    assert_ne!(layers(&shell, device)[2].name, "Third");
}

// Spec: multisampler R2
#[test]
fn a_layer_with_missing_media_is_silent_and_reported() {
    let scratch = Scratch::new("missing");
    let mut shell = shell(&scratch);
    let device = add_multisampler(&mut shell);
    let layer = Layer::new(
        LayerId(0),
        "Gone",
        bl_model::ContentHash([9; 32]),
        4800,
        48_000,
        1,
    );
    shell
        .run(
            "device.insert_layers",
            json!({ "device": device, "index": null, "layers": [layer] }),
        )
        .unwrap();
    let (out, missing) = play(&shell, device, 2000);
    assert!(out.iter().all(|s| *s == 0.0), "silence");
    assert_eq!(missing, 1, "reported");
}

// Spec: multisampler R11
#[test]
fn sorting_and_select_all_with_same_range() {
    let scratch = Scratch::new("sort");
    let mut shell = shell(&scratch);
    let device = add_multisampler(&mut shell);
    let files: Vec<PathBuf> = ["Cello", "Alto", "Bass"]
        .iter()
        .map(|name| {
            let file = scratch.path(&format!("{name}.wav"));
            wav(&file, 480, 0.1, None);
            file
        })
        .collect();
    let refs: Vec<&Path> = files.iter().map(PathBuf::as_path).collect();
    add_layers(&mut shell, device, &refs);
    let names = |shell: &Shell| -> Vec<String> {
        layers(shell, device).iter().map(|l| l.name.clone()).collect()
    };
    shell
        .run("device.sort_layers", json!({ "device": device, "by": "name", "descending": false }))
        .unwrap();
    assert_eq!(names(&shell), ["Alto", "Bass", "Cello"]);
    shell
        .run("device.sort_layers", json!({ "device": device, "by": "name", "descending": true }))
        .unwrap();
    assert_eq!(names(&shell), ["Cello", "Bass", "Alto"]);

    let ids: Vec<u32> = layers(&shell, device).iter().map(|l| l.id.0).collect();
    shell
        .run(
            "device.set_layer_zone",
            json!({ "device": device, "kind": "key", "zones": [
                { "layer": ids[0], "zone": { "low": 10, "high": 20 } },
                { "layer": ids[2], "zone": { "low": 10, "high": 20 } }] }),
        )
        .unwrap();
    let selected = shell
        .run(
            "device.select_layers",
            json!({ "device": device, "layers": [ids[0]], "mode": "same_range" }),
        )
        .unwrap();
    assert_eq!(selected["selection"], json!([ids[0], ids[2]]));
    let toggled = shell
        .run(
            "device.select_layers",
            json!({ "device": device, "layers": [ids[1]], "mode": "add" }),
        )
        .unwrap();
    assert_eq!(toggled["selection"].as_array().unwrap().len(), 3);
}

// Spec: multisampler R13
#[test]
fn normalize_volume_and_pan_set_each_layer() {
    let scratch = Scratch::new("normalize");
    let quiet = scratch.path("Quiet.wav");
    wav(&quiet, 4800, 0.25, None);
    let mut shell = shell(&scratch);
    let device = add_multisampler(&mut shell);
    add_layers(&mut shell, device, &[&quiet]);
    let before = shell.editor().undo_history().len();
    shell
        .run("device.normalize_layers", json!({ "device": device, "what": "volume" }))
        .unwrap();
    let volume = layers(&shell, device)[0].volume;
    assert!((volume - 4.0).abs() < 0.01, "a peak of 0.25 takes four: {volume}");
    assert_eq!(shell.editor().undo_history().len(), before + 1);
    // A mono layer keeps its pan.
    shell
        .run("device.normalize_layers", json!({ "device": device, "what": "pan" }))
        .unwrap();
    assert_eq!(layers(&shell, device)[0].pan, 0.0);
}

// Spec: multisampler R20
#[test]
fn snap_puts_set_markers_on_zero_crossings() {
    let scratch = Scratch::new("snap");
    // A 480 Hz sine at 48 kHz: a crossing every 50 frames.
    let sine = scratch.path("Sine.wav");
    let samples: Vec<f32> = (0..4800)
        .map(|i| (i as f32 * 480.0 / 48_000.0 * std::f32::consts::TAU).sin() * 0.5)
        .collect();
    std::fs::write(
        &sine,
        bl_media::wav_bytes(&samples, 1, 48_000, BitDepth::Int16, bl_media::Dither::None).unwrap(),
    )
    .unwrap();
    let mut shell = shell(&scratch);
    let device = add_multisampler(&mut shell);
    add_layers(&mut shell, device, &[&sine]);
    let marker = |shell: &mut Shell, snap: bool| {
        shell
            .run("device.set_layer_options", json!({ "device": device, "snap": snap }))
            .unwrap();
        shell
            .run(
                "device.set_layer",
                json!({ "device": device, "settings": { "loop_start": 1010, "loop_end": 3012 } }),
            )
            .unwrap();
        let layer = layers(shell, device).remove(0);
        (layer.loop_start, layer.loop_end)
    };
    assert_eq!(marker(&mut shell, false), (1010, 3012), "Snap off sets them as given");
    let (start, end) = marker(&mut shell, true);
    assert_eq!((start % 50, end % 50), (0, 0), "{start} {end}");
    assert!(start.abs_diff(1010) <= 25 && end.abs_diff(3012) <= 25);
    // Snap Markers snaps a loop's markers once, whatever Snap says.
    shell
        .run("device.set_layer_options", json!({ "device": device, "snap": false }))
        .unwrap();
    shell
        .run(
            "device.update_layers",
            json!({ "device": device, "layers": [1], "settings": { "loop_start": 1010, "loop_end": 3012 } }),
        )
        .unwrap();
    shell
        .run("device.snap_layer_markers", json!({ "device": device, "layers": [1] }))
        .unwrap();
    let layer = layers(&shell, device).remove(0);
    assert_eq!((layer.loop_start % 50, layer.loop_end % 50), (0, 0));
}

// Spec: multisampler R7
#[test]
fn auto_select_follows_the_incoming_note() {
    let scratch = Scratch::new("auto");
    let mut shell = shell(&scratch);
    let device = add_multisampler(&mut shell);
    let files: Vec<PathBuf> = ["Low", "High"]
        .iter()
        .map(|name| {
            let file = scratch.path(&format!("{name}.wav"));
            wav(&file, 480, 0.1, None);
            file
        })
        .collect();
    let refs: Vec<&Path> = files.iter().map(PathBuf::as_path).collect();
    add_layers(&mut shell, device, &refs);
    let ids: Vec<u32> = layers(&shell, device).iter().map(|l| l.id.0).collect();
    shell
        .run(
            "device.set_layer_zone",
            json!({ "device": device, "kind": "key", "zones": [
                { "layer": ids[0], "zone": { "low": 0, "high": 59 } },
                { "layer": ids[1], "zone": { "low": 60, "high": 127 } }] }),
        )
        .unwrap();
    shell
        .run("midi.toggle_computer_keyboard", json!({ "on": true }))
        .unwrap();
    shell
        .run("device.set_multisampler_view", json!({ "device": device, "auto_select": true }))
        .unwrap();
    shell
        .run("device.select_layers", json!({ "device": device, "layers": [ids[0]] }))
        .unwrap();
    // The A key plays C3 with the keyboard's octave at 0: layer two takes it.
    let lowest = shell.run("midi.computer_keyboard_octave", json!({ "steps": 0 })).unwrap()["lowest_note"]
        .as_u64()
        .unwrap();
    assert_eq!(lowest, 60);
    shell
        .run("midi.computer_keyboard_key", json!({ "key": "a", "pressed": true }))
        .unwrap();
    assert_eq!(state(&mut shell, device)["selection"], json!([ids[1]]));
    shell
        .run("midi.computer_keyboard_key", json!({ "key": "a", "pressed": false }))
        .unwrap();
    assert_eq!(
        state(&mut shell, device)["selection"],
        json!([ids[0]]),
        "the note's end gives the selection back"
    );
}

// Spec: multisampler R1, R30
#[test]
fn the_views_are_saved_with_the_set_and_are_no_undo_step() {
    let scratch = Scratch::new("views");
    let mut shell = shell(&scratch);
    let device = add_multisampler(&mut shell);
    let steps = shell.editor().undo_history().len();
    let view = shell
        .run(
            "device.set_multisampler_view",
            json!({ "device": device, "tab": "filter_global", "zone_editor": true, "editor": "velocity" }),
        )
        .unwrap();
    assert_eq!(view["tab"], "filter_global");
    assert_eq!(shell.editor().undo_history().len(), steps, "no undo step");
    let set = save_as(&mut shell, &scratch);
    shell
        .run("device.set_multisampler_view", json!({ "device": device, "tab": "midi" }))
        .unwrap();
    shell
        .run("set.open", json!({ "path": set.to_string_lossy() }))
        .unwrap();
    let reopened = state(&mut shell, device);
    assert_eq!(reopened["tab"], "filter_global");
    assert_eq!(reopened["zone_editor"], json!(true));
    assert_eq!(reopened["editor"], "velocity");
}

// Spec: multisampler R29
#[test]
fn the_title_shows_the_first_layers_name() {
    let scratch = Scratch::new("title");
    let mut shell = shell(&scratch);
    let device = add_multisampler(&mut shell);
    let title = |shell: &mut Shell| -> String {
        let state = shell.run("device_view.state", json!({})).unwrap();
        state["devices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == json!(device))
            .unwrap()["title"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(title(&mut shell), "Mosaic", "the device's own name with no layers");
    let file = scratch.path("Strings.wav");
    wav(&file, 480, 0.1, None);
    add_layers(&mut shell, device, &[&file]);
    assert_eq!(title(&mut shell), "Strings");
    let _ = Arc::new(0);
}

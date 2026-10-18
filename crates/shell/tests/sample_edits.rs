//! Sample editing in the sampler devices (WP-5.15, IMP-28; the multisampler
//! spec, R55 to R63): the edit list, Revert Edits, Crop and Reverse as
//! processed copies, Simpler's Normalize Volumes, and the conversions
//! between Simpler and the multisampler. Driven through `Shell::run`, as an
//! agent drives it (D17), on real files and bl-project's real set files.

use std::path::{Path, PathBuf};

use bl_devices::multisampler::MULTISAMPLER;
use bl_devices::sampler::{self, SAMPLER};
use bl_media::BitDepth;
use bl_model::{
    ContentHash, Device, DeviceId, DeviceKind, DeviceRole, SampleEdit, SampleEditKind, SampleRange,
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
            "bl-sample-edits-{name}-{}-{}",
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

/// The frames the test files have.
const FRAMES: usize = 4800;

/// Writes a mono 16-bit WAV that rises from 0 to 0.25, so every frame is
/// different and the largest magnitude is its last.
fn wav(path: &Path) {
    let samples: Vec<f32> = (0..FRAMES)
        .map(|i| i as f32 / FRAMES as f32 * 0.25)
        .collect();
    let bytes =
        bl_media::wav_bytes(&samples, 1, 48_000, BitDepth::Int16, bl_media::Dither::None).unwrap();
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

fn add_device(shell: &mut Shell, device_type: &str, name: &str) -> DeviceId {
    // A chain holds one instrument, so each device gets a MIDI track.
    shell
        .invoke("track.create", json!({ "kind": "midi", "name": name }))
        .unwrap();
    let midi = shell
        .editor()
        .document()
        .tracks()
        .filter(|track| track.kind() == TrackKind::Midi)
        .last()
        .map(bl_model::Track::id)
        .expect("the new MIDI track");
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [midi] }))
        .unwrap();
    let added: DeviceAdded = serde_json::from_value(
        shell
            .run(
                "device.add",
                json!({
                    "device": {
                        "kind": DeviceKind::Native { device_type: device_type.to_owned() },
                        "name": name,
                        "role": DeviceRole::Instrument,
                    }
                }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
}

/// A Simpler playing `file`.
fn snapshot(shell: &mut Shell, file: &Path) -> DeviceId {
    let device = add_device(shell, SAMPLER, "Simpler");
    shell
        .run(
            "device.load_sample",
            json!({ "device": device, "path": file.to_string_lossy() }),
        )
        .unwrap();
    device
}

/// A multisampler with one layer of `file`.
fn multisampler(shell: &mut Shell, file: &Path) -> DeviceId {
    let device = add_device(shell, MULTISAMPLER, "Sampler");
    shell
        .run(
            "device.add_layers",
            json!({ "device": device, "paths": [file.to_string_lossy()] }),
        )
        .unwrap();
    device
}

fn device(shell: &Shell, id: DeviceId) -> Device {
    shell.editor().document().device(id).unwrap().clone()
}

fn edit(shell: &mut Shell, device: DeviceId, kind: &str, range: Option<(u64, u64)>) -> Value {
    let mut args = json!({ "device": device, "kind": kind });
    if let Some((from, to)) = range {
        args["range"] = json!({ "from": from, "to": to });
    }
    shell.run("device.edit_sample", args).unwrap()
}

fn undo(shell: &mut Shell) {
    shell.run("edit.undo", json!({})).unwrap();
}

fn set_param(shell: &mut Shell, device: DeviceId, param: bl_model::ParamId, value: f64) {
    shell
        .run(
            "device.set_param",
            json!({ "device": device, "param": param, "value": value }),
        )
        .unwrap();
}

fn file_hash(path: &Path) -> bl_media::ContentHash {
    bl_media::ContentHash::of_file(path).unwrap()
}

// Spec: multisampler R55
#[test]
fn each_edit_is_one_undo_step_and_leaves_the_file_untouched() {
    let scratch = Scratch::new("steps");
    let file = scratch.path("Ramp.wav");
    wav(&file);
    let before = file_hash(&file);
    let mut shell = shell(&scratch);
    let snap = snapshot(&mut shell, &file);
    let layered = multisampler(&mut shell, &file);
    let media = device(&shell, snap).media_at(0).unwrap();
    assert_eq!(media.0, *before.as_bytes());
    for (kind, range, length) in [
        ("trim", Some((1_200, 3_600)), 2_400),
        ("cut", Some((100, 900)), FRAMES as u64 - 800),
        ("reverse", Some((0, 1_000)), FRAMES as u64),
        ("reverse", None, FRAMES as u64),
        ("fade_in", Some((0, 2_000)), FRAMES as u64),
        ("fade_out", None, FRAMES as u64),
        ("normalize", Some((0, 2_400)), FRAMES as u64),
    ] {
        for (id, layer) in [(snap, None), (layered, Some(1))] {
            let edited = edit(&mut shell, id, kind, range);
            assert_eq!(edited["length"], json!(length), "{kind} on {id}");
            assert_eq!(edited["edits"].as_array().unwrap().len(), 1);
            let held = device(&shell, id);
            let edits = match layer {
                Some(_) => held.layers.list[0].edits.clone(),
                None => held.sample_edits.clone(),
            };
            let audio = shell.edited_audio(media, &edits).unwrap();
            assert_eq!(audio.frames() as u64, length);
            // One undo step takes the whole edit back.
            undo(&mut shell);
            let held = device(&shell, id);
            assert!(
                held.sample_edits.is_empty() && held.layers.list.iter().all(|l| l.edits.is_empty())
            );
        }
    }
    assert_eq!(
        file_hash(&file),
        before,
        "the source file's bytes are unchanged"
    );
}

// Spec: multisampler R55, R56
#[test]
fn a_layers_markers_follow_a_cut_through_the_command() {
    let scratch = Scratch::new("markers");
    let file = scratch.path("Ramp.wav");
    wav(&file);
    let mut shell = shell(&scratch);
    let layered = multisampler(&mut shell, &file);
    shell
        .run(
            "device.update_layers",
            json!({ "device": layered, "layers": [1], "settings": { "start": 100, "end": 4_000 } }),
        )
        .unwrap();
    edit(&mut shell, layered, "cut", Some((500, 1_500)));
    let layer = device(&shell, layered).layers.list[0].clone();
    assert_eq!((layer.length, layer.start, layer.end), (3_800, 100, 3_000));
    // A selection past the edited sample's end is refused and changes nothing.
    let error = shell
        .run(
            "device.edit_sample",
            json!({ "device": layered, "kind": "trim", "range": { "from": 0, "to": 4_800 } }),
        )
        .unwrap_err();
    assert!(error.to_string().contains("selection"), "{error}");
    assert_eq!(device(&shell, layered).layers.list[0], layer);
}

// Spec: multisampler R58
#[test]
fn revert_edits_clears_the_list_as_one_step() {
    let scratch = Scratch::new("revert");
    let file = scratch.path("Ramp.wav");
    wav(&file);
    let mut shell = shell(&scratch);
    let snap = snapshot(&mut shell, &file);
    let layered = multisampler(&mut shell, &file);
    for id in [snap, layered] {
        edit(&mut shell, id, "trim", Some((0, 2_400)));
        edit(&mut shell, id, "fade_out", None);
        let reverted = shell
            .run(
                "device.edit_sample",
                json!({ "device": id, "revert": true }),
            )
            .unwrap();
        assert_eq!(reverted["edits"], json!([]));
        assert_eq!(reverted["length"], json!(FRAMES));
        let held = device(&shell, id);
        assert!(
            held.sample_edits.is_empty() && held.layers.list.iter().all(|l| l.edits.is_empty())
        );
        assert!(held.layers.list.iter().all(|l| l.length == FRAMES as u64));
        // One undo step brings both edits back.
        undo(&mut shell);
        let held = device(&shell, id);
        assert_eq!(
            held.sample_edits.len()
                + held
                    .layers
                    .list
                    .iter()
                    .map(|l| l.edits.len())
                    .sum::<usize>(),
            2
        );
    }
}

// Spec: multisampler R59
#[test]
fn crop_and_reverse_write_a_processed_copy_and_keep_the_original() {
    let scratch = Scratch::new("copies");
    let file = scratch.path("Ramp.wav");
    wav(&file);
    let before = file_hash(&file);
    let mut shell = shell(&scratch);
    let snap = snapshot(&mut shell, &file);
    let layered = multisampler(&mut shell, &file);
    shell
        .run(
            "set.save_as",
            json!({ "path": scratch.path("Song.blset").to_string_lossy() }),
        )
        .unwrap();
    let source = shell.media_store().load(before).unwrap();
    let ramp = source.channel(0).unwrap().to_vec();

    // Simpler's Crop keeps the region (a quarter in, half the length).
    set_param(&mut shell, snap, sampler::START, 25.0);
    set_param(&mut shell, snap, sampler::LENGTH, 50.0);
    edit(&mut shell, snap, "fade_in", Some((0, 10)));
    let cropped = shell
        .run("device.crop_sample", json!({ "device": snap }))
        .unwrap();
    let path = PathBuf::from(cropped["path"].as_str().unwrap());
    assert!(
        path.to_string_lossy().contains("Processed"),
        "{}",
        path.display()
    );
    assert!(path.starts_with(scratch.path("")), "the project folder");
    let copy = bl_media::decode_file(&path).unwrap();
    assert_eq!(
        (copy.sample_rate(), copy.channel_count(), copy.frames()),
        (48_000, 1, 2_400)
    );
    let edited = shell
        .edited_audio(
            ContentHash(*before.as_bytes()),
            &[SampleEdit {
                kind: SampleEditKind::FadeIn,
                range: Some(SampleRange { from: 0, to: 10 }),
            }],
        )
        .unwrap();
    assert_eq!(
        copy.channel(0).unwrap(),
        &edited.channel(0).unwrap()[1_200..3_600],
        "the edits are applied"
    );
    let now = device(&shell, snap);
    assert_eq!(
        now.media_at(0).unwrap(),
        serde_json::from_value(cropped["media"].clone()).unwrap()
    );
    assert!(now.sample_edits.is_empty());
    let value = |id| {
        sampler::sampler_type()
            .description
            .value(&now.params, id)
            .unwrap()
    };
    assert_eq!(
        (value(sampler::START), value(sampler::LENGTH)),
        (0.0, 100.0)
    );
    // One undo step puts the original and the region back.
    undo(&mut shell);
    let back = device(&shell, snap);
    assert_eq!(back.media_at(0).unwrap().0, *before.as_bytes());
    assert_eq!(back.sample_edits.len(), 1);
    let value = |id| {
        sampler::sampler_type()
            .description
            .value(&back.params, id)
            .unwrap()
    };
    assert_eq!(
        (value(sampler::START), value(sampler::LENGTH)),
        (25.0, 50.0)
    );

    // A layer's Reverse writes the whole sample reversed and keeps its markers.
    shell
        .run(
            "device.update_layers",
            json!({ "device": layered, "layers": [1], "settings": { "start": 100, "end": 4_000 } }),
        )
        .unwrap();
    let reversed = shell
        .run("device.reverse_sample", json!({ "device": layered }))
        .unwrap();
    let path = PathBuf::from(reversed["path"].as_str().unwrap());
    assert!(path.to_string_lossy().contains("Reverse"));
    let copy = bl_media::decode_file(&path).unwrap();
    let mut backwards = ramp.clone();
    backwards.reverse();
    assert_eq!(copy.channel(0).unwrap(), &backwards[..]);
    let layer = device(&shell, layered).layers.list[0].clone();
    assert_eq!(
        (layer.start, layer.end, layer.length),
        (100, 4_000, FRAMES as u64)
    );
    assert!(layer.edits.is_empty());
    assert_ne!(layer.media.0, *before.as_bytes());

    // A layer's Crop moves its markers as a trim does.
    undo(&mut shell);
    let layer_crop = shell
        .run("device.crop_sample", json!({ "device": layered }))
        .unwrap();
    assert_eq!(layer_crop["length"], json!(3_900));
    let layer = device(&shell, layered).layers.list[0].clone();
    assert_eq!((layer.start, layer.end, layer.length), (0, 3_900, 3_900));

    assert_eq!(
        file_hash(&file),
        before,
        "the original file stays as it was"
    );
}

// Spec: multisampler R59
#[test]
fn an_unsaved_set_refuses_crop() {
    let scratch = Scratch::new("unsaved");
    let file = scratch.path("Ramp.wav");
    wav(&file);
    let mut shell = shell(&scratch);
    let snap = snapshot(&mut shell, &file);
    set_param(&mut shell, snap, sampler::LENGTH, 50.0);
    let before = device(&shell, snap);
    for command in ["device.crop_sample", "device.reverse_sample"] {
        let error = shell.run(command, json!({ "device": snap })).unwrap_err();
        assert!(error.to_string().contains("saved"), "{error}");
        let message = shell.message().expect("a message in the Status Bar");
        assert!(
            message.error && message.text.contains("saved"),
            "{}",
            message.text
        );
    }
    assert_eq!(device(&shell, snap), before, "the device is as it was");
}

// Spec: multisampler R60
#[test]
fn snapshots_normalize_volumes_sets_its_gain() {
    let scratch = Scratch::new("normalize");
    let file = scratch.path("Ramp.wav");
    wav(&file);
    let mut shell = shell(&scratch);
    let snap = snapshot(&mut shell, &file);
    // The region's largest magnitude is the last frame it holds: half the
    // sample, 0.125.
    set_param(&mut shell, snap, sampler::LENGTH, 50.0);
    shell
        .run(
            "device.normalize_layers",
            json!({ "device": snap, "what": "volume" }),
        )
        .unwrap();
    let held = device(&shell, snap);
    let gain = sampler::sampler_type()
        .description
        .value(&held.params, sampler::GAIN)
        .unwrap();
    assert!((gain - 8.0).abs() < 0.01, "{gain}");
    undo(&mut shell);
    let held = device(&shell, snap);
    assert_eq!(
        sampler::sampler_type()
            .description
            .value(&held.params, sampler::GAIN),
        Some(1.0)
    );
}

// Spec: multisampler R62
#[test]
fn snapshot_and_the_multisampler_convert_both_ways() {
    let scratch = Scratch::new("convert");
    let file = scratch.path("Ramp.wav");
    wav(&file);
    let mut shell = shell(&scratch);
    let snap = snapshot(&mut shell, &file);
    shell
        .run(
            "device.rename",
            json!({ "device": snap, "name": "My Sample" }),
        )
        .unwrap();
    set_param(&mut shell, snap, sampler::START, 25.0);
    set_param(&mut shell, snap, sampler::LENGTH, 50.0);
    set_param(&mut shell, snap, sampler::LOOP, 1.0);
    set_param(&mut shell, snap, sampler::LOOP_LENGTH, 50.0);
    set_param(&mut shell, snap, sampler::ATTACK, 20.0);
    set_param(&mut shell, snap, sampler::MODE, 2.0);
    edit(&mut shell, snap, "fade_out", None);
    let track = shell.editor().document().device_track(snap).unwrap();

    let converted = shell
        .run(
            "device.convert",
            json!({ "device": snap, "to": "multisampler" }),
        )
        .unwrap();
    let mosaic: DeviceId = serde_json::from_value(converted["device"].clone()).unwrap();
    let dropped = converted["dropped"].to_string();
    assert!(
        dropped.contains("Slice"),
        "Slicing doesn't carry over: {dropped}"
    );
    assert!(shell.editor().document().device(snap).is_none());
    let new = device(&shell, mosaic);
    assert_eq!(new.name, "My Sample", "the name is kept");
    assert_eq!(shell.editor().document().device_track(mosaic), Some(track));
    let layer = new.layers.list[0].clone();
    assert_eq!(
        layer.media.0,
        *file_hash(&file).as_bytes(),
        "the sample is kept"
    );
    assert_eq!(
        (layer.start, layer.end),
        (1_200, 3_600),
        "the region is the markers"
    );
    assert_eq!(
        layer.sustain,
        bl_model::SustainMode::Loop,
        "the loop is a sustain loop"
    );
    assert_eq!((layer.loop_start, layer.loop_end), (2_400, 3_600));
    assert_eq!(layer.edits.len(), 1, "the edits go with the sample");
    let attack = bl_devices::multisampler::multisampler_type()
        .description
        .value(&new.params, bl_devices::multisampler::ENV_ATTACK)
        .unwrap();
    assert_eq!(attack, 20.0, "the envelope carries over");
    // One undo step puts Simpler back.
    undo(&mut shell);
    assert!(shell.editor().document().device(mosaic).is_none());
    assert_eq!(device(&shell, snap).sample_edits.len(), 1);
    // And back again: Classic mode playing the first layer.
    let converted = shell
        .run(
            "device.convert",
            json!({ "device": snap, "to": "multisampler" }),
        )
        .unwrap();
    let mosaic: DeviceId = serde_json::from_value(converted["device"].clone()).unwrap();
    shell
        .run(
            "device.add_layers",
            json!({ "device": mosaic, "paths": [file.to_string_lossy()] }),
        )
        .unwrap();
    let back = shell
        .run(
            "device.convert",
            json!({ "device": mosaic, "to": "snapshot" }),
        )
        .unwrap();
    assert!(
        back["dropped"].to_string().contains("1 more layer"),
        "{back}"
    );
    let snap: DeviceId = serde_json::from_value(back["device"].clone()).unwrap();
    let held = device(&shell, snap);
    let value = |id| {
        sampler::sampler_type()
            .description
            .value(&held.params, id)
            .unwrap()
    };
    assert_eq!(value(sampler::MODE), 0.0);
    assert_eq!(held.name, "My Sample");
    assert!(
        (value(sampler::START) - 25.0).abs() < 1e-9 && (value(sampler::LENGTH) - 50.0).abs() < 1e-9
    );
    assert_eq!(value(sampler::LOOP), 1.0);
    assert!((value(sampler::LOOP_LENGTH) - 50.0).abs() < 1e-9);
    assert_eq!(held.media_at(0).unwrap().0, *file_hash(&file).as_bytes());
    assert_eq!(held.sample_edits.len(), 1);
}

// Spec: multisampler R55
#[test]
fn edits_are_saved_with_the_set_and_come_back() {
    let scratch = Scratch::new("reopen");
    let file = scratch.path("Ramp.wav");
    wav(&file);
    let mut shell = shell(&scratch);
    let snap = snapshot(&mut shell, &file);
    let layered = multisampler(&mut shell, &file);
    for id in [snap, layered] {
        edit(&mut shell, id, "cut", Some((100, 900)));
        edit(&mut shell, id, "fade_in", Some((0, 50)));
    }
    let (snap_edits, layer_edits) = (
        device(&shell, snap).sample_edits,
        device(&shell, layered).layers.list[0].edits.clone(),
    );
    assert_eq!(snap_edits.len(), 2);
    let saved = shell
        .run(
            "set.save_as",
            json!({ "path": scratch.path("Song.blset").to_string_lossy() }),
        )
        .unwrap();
    shell
        .run("set.open", json!({ "path": saved["path"] }))
        .unwrap();
    assert_eq!(device(&shell, snap).sample_edits, snap_edits);
    let layer = device(&shell, layered).layers.list[0].clone();
    assert_eq!(layer.edits, layer_edits);
    assert_eq!(
        layer.length,
        FRAMES as u64 - 800,
        "the layer's length is the edited sample's"
    );
}

// Spec: multisampler R57
#[test]
fn the_engine_plays_the_edited_sample_and_shares_it() {
    use bl_engine::MediaLibrary;
    let scratch = Scratch::new("shared");
    let file = scratch.path("Ramp.wav");
    wav(&file);
    let mut shell = shell(&scratch);
    let snap = snapshot(&mut shell, &file);
    edit(&mut shell, snap, "cut", Some((0, 1_000)));
    let held = device(&shell, snap);
    let media = held.media_at(0).unwrap();
    // The display's copy and the engine's are one: the same source with the
    // same list is rendered once.
    let shown = shell.edited_audio(media, &held.sample_edits).unwrap();
    let played = shell
        .media_store()
        .edited_sample(
            media,
            bl_core::time::SampleRate::HZ_48000,
            &held.sample_edits,
        )
        .unwrap();
    assert!(std::sync::Arc::ptr_eq(&shown, &played));
    assert_eq!(shown.frames(), FRAMES - 1_000);
    // Another list is another copy, and the source is untouched.
    let other = shell.edited_audio(media, &[]).unwrap();
    assert_eq!(other.frames(), FRAMES);
    assert!(!std::sync::Arc::ptr_eq(&shown, &other));
}

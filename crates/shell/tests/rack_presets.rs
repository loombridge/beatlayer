//! Rack and chain presets, and hot-swapping to load them (WP-5.8; the racks
//! spec, R42 to R47). Driven through `Shell::run`, as an agent drives it
//! (D17), on real files.

use std::path::{Path, PathBuf};

use bl_devices::sampler::SAMPLER;
use bl_engine::testing;
use bl_media::BitDepth;
use bl_model::{ChainId, DeviceId, DeviceKind, DeviceRole, TrackId};
use bl_shell::device_commands::{ChainAdded, DeviceAdded, DeviceViewState, PresetResult};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::{Area, Shell};
use serde_json::{Value, json};

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-rack-presets-{name}-{}-{}",
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

/// Writes a short mono WAV file that isn't silence.
fn wav(path: &Path, frames: usize) {
    let samples: Vec<f32> = (0..frames)
        .map(|i| (i as f32 * 440.0 / 48_000.0 * std::f32::consts::TAU).sin() * 0.25)
        .collect();
    let bytes =
        bl_media::wav_bytes(&samples, 1, 48_000, BitDepth::Int16, bl_media::Dither::None).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// A shell with its own preferences and a User Library, on the default set
/// with its MIDI track selected and the Device View focused.
fn shell_in(scratch: &Scratch, name: &str) -> (Shell, TrackId, TrackId) {
    let mut shell = Shell::new(
        Preferences::default(),
        Some(scratch.path(&format!("{name}-config"))),
        bl_shell::audio::AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    shell.set_user_library(Some(scratch.path(&format!("{name}-library"))));
    let top: Vec<TrackId> = shell.editor().document().top_level().collect();
    let (midi, audio) = (top[0], top[2]);
    select_track(&mut shell, midi);
    shell.set_focused_area(Area::Detail);
    (shell, midi, audio)
}

fn select_track(shell: &mut Shell, track: TrackId) {
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
}

fn rack_of(kind: &str) -> Value {
    json!({ "kind": { "rack": { "kind": kind } }, "name": "Rack", "role": rack_role(kind) })
}

fn rack_role(kind: &str) -> &'static str {
    match kind {
        "audio_effect" => "audio_effect",
        "midi_effect" => "midi_effect",
        _ => "instrument",
    }
}

fn gain() -> Value {
    let t = testing::gain_type();
    json!({ "kind": t.kind, "name": t.name, "role": t.role })
}

fn sampler() -> Value {
    json!({
        "kind": DeviceKind::Native { device_type: SAMPLER.to_owned() },
        "name": "Snapshot",
        "role": DeviceRole::Instrument,
    })
}

fn add(shell: &mut Shell, args: Value) -> DeviceAdded {
    serde_json::from_value(shell.run("device.add", args).unwrap()).unwrap()
}

fn add_chain(shell: &mut Shell, rack: DeviceId, name: &str) -> ChainId {
    let id: ChainId = shell.new_id();
    let index = shell.editor().document().device(rack).unwrap().chains.len();
    shell
        .run(
            "chain.add",
            json!({ "rack": rack, "index": index, "chain": { "id": id, "name": name, "devices": [] } }),
        )
        .unwrap();
    id
}

fn steps(shell: &Shell) -> usize {
    shell.editor().undo_history().len()
}

fn last_step(shell: &Shell) -> String {
    shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn track_devices(shell: &Shell, track: TrackId) -> Vec<DeviceId> {
    shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .devices()
        .map(|d| d.id)
        .collect()
}

fn view(shell: &Shell) -> DeviceViewState {
    shell.device_view_state()
}

fn save_preset(shell: &mut Shell, device: DeviceId) -> PresetResult {
    serde_json::from_value(
        shell
            .run("device.save_preset", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap()
}

/// Selects `path` in the browser and loads it, as Enter does.
fn load_selected(shell: &mut Shell, path: &str) -> Result<Value, bl_shell::ShellError> {
    shell
        .run("browser.select", json!({ "paths": [path] }))
        .unwrap();
    shell.set_focused_area(Area::Browser);
    let loaded = shell.run("browser.load_selected", json!({}));
    shell.set_focused_area(Area::Detail);
    loaded
}

/// An instrument rack holding a chain "Keys" with a sampler that plays
/// `file`, on the MIDI track.
fn rack_with_sampler(shell: &mut Shell, file: &Path) -> (DeviceId, ChainId, DeviceId) {
    let rack = add(shell, json!({ "device": rack_of("instrument") })).device;
    let chain = add_chain(shell, rack, "Keys");
    shell
        .run("chain.set_gain", json!({ "chain": chain, "volume": 0.5 }))
        .unwrap();
    let sampler = add(shell, json!({ "device": sampler(), "chain": chain })).device;
    shell
        .run(
            "device.load_sample",
            json!({ "device": sampler, "path": file.to_string_lossy() }),
        )
        .unwrap();
    (rack, chain, sampler)
}

// Spec: racks R42, R47
#[test]
fn a_saved_rack_loads_into_another_set_with_its_sample_and_survives_save_and_reopen() {
    let scratch = Scratch::new("rack");
    let file = scratch.path("Kick.wav");
    wav(&file, 4800);
    let (mut shell, _, _) = shell_in(&scratch, "a");
    let (rack, _, sampler) = rack_with_sampler(&mut shell, &file);
    let sample = shell
        .editor()
        .document()
        .device(sampler)
        .unwrap()
        .media_at(0)
        .unwrap();

    let saved = save_preset(&mut shell, rack);
    let path = PathBuf::from(&saved.path);
    assert!(
        path.ends_with("a-library/Presets/Instruments/Instrument Rack/Instrument Rack.blpreset"),
        "{path:?}"
    );
    let folder: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().collect();
    assert_eq!(folder.len(), 1, "written whole, nothing left beside it");

    // Another set, whose media store has never seen the sample, and whose
    // User Library is its own.
    let (mut other, other_midi, _) = shell_in(&scratch, "b");
    assert!(
        !other
            .media_store()
            .contains(bl_media::ContentHash::from_bytes(sample.0))
    );
    let before = steps(&other);
    let loaded: DeviceAdded = serde_json::from_value(
        other
            .run("device.add_preset", json!({ "path": saved.path }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(steps(&other), before + 1, "one undo step");
    assert_eq!(last_step(&other), "Insert Device");
    assert_eq!(track_devices(&other, other_midi), vec![loaded.device]);
    let doc = other.editor().document();
    let copy = doc.device(loaded.device).unwrap();
    let original = shell.editor().document().device(rack).unwrap();
    assert_eq!(copy.chains.len(), 1);
    assert_eq!(copy.chains[0].name, "Keys");
    assert_eq!(copy.chains[0].mixer.volume, 0.5);
    let inner = &copy.chains[0].devices[0];
    assert_eq!(inner.kind, original.chains[0].devices[0].kind);
    assert_eq!(inner.media_at(0), Some(sample), "plays the same sample");
    let entry = other
        .media_store()
        .entry(bl_media::ContentHash::from_bytes(sample.0))
        .expect("the media store has it now");
    assert!(
        entry.path.starts_with(scratch.path("b-library")),
        "{:?}",
        entry.path
    );
    assert_eq!(
        std::fs::read(&entry.path).unwrap(),
        std::fs::read(&file).unwrap()
    );

    // Undo takes the rack out, redo brings it back.
    other.run("edit.undo", json!({})).unwrap();
    assert!(track_devices(&other, other_midi).is_empty());
    other.run("edit.redo", json!({})).unwrap();
    assert_eq!(track_devices(&other, other_midi), vec![loaded.device]);

    // A set holding the loaded rack saves and reopens with it.
    let set = scratch.path("Song.blset");
    let result: bl_shell::commands::SetResult = serde_json::from_value(
        other
            .run("set.save_as", json!({ "path": set.to_string_lossy() }))
            .unwrap(),
    )
    .unwrap();
    let set = result.path.expect("the set was saved");
    other.run("set.open", json!({ "path": set })).unwrap();
    let reopened = other.editor().document().device(loaded.device).unwrap();
    assert_eq!(reopened.chains[0].devices[0].media_at(0), Some(sample));
    assert!(
        other
            .media_store()
            .entry(bl_media::ContentHash::from_bytes(sample.0))
            .is_some()
    );
}

// Spec: racks R42
#[test]
fn newer_and_damaged_rack_presets_change_nothing() {
    let scratch = Scratch::new("refused");
    let file = scratch.path("Kick.wav");
    wav(&file, 4800);
    let (mut shell, midi, _) = shell_in(&scratch, "a");
    let (rack, _, _) = rack_with_sampler(&mut shell, &file);
    let saved = save_preset(&mut shell, rack);
    let good = std::fs::read_to_string(&saved.path).unwrap();
    let before = steps(&shell);
    let devices = track_devices(&shell, midi);

    let newer = good.replacen("\"schema\": 2", "\"schema\": 3", 1);
    let damaged = &good[..good.len() / 2];
    for (name, text) in [("Newer", newer.as_str()), ("Damaged", damaged)] {
        let path = scratch.path(&format!("{name}.blpreset"));
        std::fs::write(&path, text).unwrap();
        let path = path.to_string_lossy().into_owned();
        for (command, args) in [
            ("device.add_preset", json!({ "path": path })),
            (
                "device.load_preset",
                json!({ "device": rack, "path": path }),
            ),
            ("device.replace", json!({ "device": rack, "preset": path })),
        ] {
            let refused = shell.run(command, args);
            assert!(refused.is_err(), "{command} loaded a {name} preset");
        }
    }
    assert_eq!(steps(&shell), before);
    assert_eq!(track_devices(&shell, midi), devices);
}

// Spec: racks R43
#[test]
fn a_racks_default_preset_starts_new_racks_of_its_kind() {
    let scratch = Scratch::new("default");
    let file = scratch.path("Kick.wav");
    wav(&file, 4800);
    let (mut shell, _, _) = shell_in(&scratch, "a");
    let (rack, _, _) = rack_with_sampler(&mut shell, &file);
    let saved: PresetResult = serde_json::from_value(
        shell
            .run("device.save_default_preset", json!({ "device": rack }))
            .unwrap(),
    )
    .unwrap();
    assert!(
        saved
            .path
            .ends_with("a-library/Defaults/Instruments/Instrument Rack.blpreset"),
        "{}",
        saved.path
    );

    // On an audio track an instrument goes on a new MIDI track.
    let audio = shell.editor().document().top_level().nth(2).unwrap();
    select_track(&mut shell, audio);
    let steps_before = steps(&shell);
    let new = add(&mut shell, json!({ "device": rack_of("instrument") }));
    assert_eq!(steps(&shell), steps_before + 1, "one undo step");
    assert_eq!(new.default_preset.as_deref(), Some("Instrument Rack"));
    let doc = shell.editor().document();
    let made = doc.device(new.device).unwrap();
    assert_eq!(made.chains.len(), 1);
    assert_eq!(made.chains[0].name, "Keys");
    assert_ne!(made.chains[0].id, doc.device(rack).unwrap().chains[0].id);
    // Racks of other kinds start empty.
    let other = add(&mut shell, json!({ "device": rack_of("audio_effect") }));
    assert!(
        shell
            .editor()
            .document()
            .device(other.device)
            .unwrap()
            .chains
            .is_empty()
    );
}

// Spec: racks R43
#[test]
fn empty_racks_and_rack_presets_load_from_their_labels() {
    let scratch = Scratch::new("labels");
    let file = scratch.path("Kick.wav");
    wav(&file, 4800);
    let (mut shell, midi, audio) = shell_in(&scratch, "a");
    for (label, rack, kind) in [
        ("drums", "Drum Rack", "drum"),
        ("instruments", "Instrument Rack", "instrument"),
        ("audio_effects", "Audio Effect Rack", "audio_effect"),
        ("midi_effects", "MIDI Effect Rack", "midi_effect"),
    ] {
        let track = if label == "audio_effects" {
            audio
        } else {
            midi
        };
        select_track(&mut shell, track);
        let rows: bl_shell_spec::browser::BrowserState = serde_json::from_value(
            shell
                .run("browser.open_place", json!({ "label": label }))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(rows.rows[0].name, rack, "{label} lists its rack first");
        assert_eq!(rows.rows[0].path, format!("rack:{kind}"));
        let before = steps(&shell);
        load_selected(&mut shell, &rows.rows[0].path).unwrap();
        assert_eq!(steps(&shell), before + 1, "{label}: one undo step");
        let doc = shell.editor().document();
        let added = doc.track(track).unwrap().devices().last().unwrap();
        assert_eq!(added.rack_kind().map(|k| k.name()), Some(rack));
        assert!(added.chains.is_empty(), "an empty rack has no chains");
        shell.run("edit.undo", json!({})).unwrap();
    }

    // A rack preset in the User Library lists with the presets and loads
    // the same way.
    select_track(&mut shell, midi);
    let (rack, _, _) = rack_with_sampler(&mut shell, &file);
    let saved = save_preset(&mut shell, rack);
    shell
        .run("browser.open_place", json!({ "label": "user_library" }))
        .unwrap();
    let folder = Path::new(&saved.path).parent().unwrap();
    let library = scratch.path("a-library");
    let route: Vec<&Path> = folder
        .ancestors()
        .take_while(|p| *p != library)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    for path in route {
        shell
            .run(
                "browser.open_folder",
                json!({ "path": path.to_string_lossy(), "open": true, "keep_others": true }),
            )
            .unwrap();
    }
    let browser = shell.browser_view();
    assert!(browser.position_of(Path::new(&saved.path)).is_some());
    // An instrument goes on a new MIDI track from the audio track.
    select_track(&mut shell, audio);
    let before = shell.editor().document().top_level().count();
    load_selected(&mut shell, &saved.path).unwrap();
    assert_eq!(shell.editor().document().top_level().count(), before + 1);
}

// Spec: racks R44
#[test]
fn q_with_nothing_selected_arms_the_tracks_first_device() {
    let scratch = Scratch::new("q");
    let (mut shell, midi, audio) = shell_in(&scratch, "a");
    select_track(&mut shell, audio);
    let first = add(&mut shell, json!({ "device": gain() })).device;
    let second = add(&mut shell, json!({ "device": gain() })).device;
    shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
    assert!(view(&shell).selected.is_none());
    shell.run("device.hot_swap", json!({})).unwrap();
    assert_eq!(view(&shell).hot_swap.unwrap().device, first);
    // Again ends it; with the second selected it arms that one.
    shell.run("device.hot_swap", json!({})).unwrap();
    assert!(view(&shell).hot_swap.is_none());
    shell
        .run("device.select", json!({ "device": second }))
        .unwrap();
    shell.run("device.hot_swap", json!({})).unwrap();
    assert_eq!(view(&shell).hot_swap.unwrap().device, second);
    shell
        .run("device.hot_swap", json!({ "on": false }))
        .unwrap();

    // On a MIDI track it is the instrument.
    select_track(&mut shell, midi);
    let synth = add(&mut shell, json!({ "device": sampler() })).device;
    shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
    shell.run("device.hot_swap", json!({})).unwrap();
    let hot = view(&shell).hot_swap.unwrap();
    assert_eq!(
        (hot.device, hot.label.as_str()),
        (synth, "Swapping Instrument")
    );
    shell
        .run("device.hot_swap", json!({ "on": false }))
        .unwrap();
    // A track with nothing to swap says so.
    let empty = shell.editor().document().top_level().nth(3).unwrap();
    select_track(&mut shell, empty);
    shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
    assert!(shell.run("device.hot_swap", json!({})).is_err());
}

// Spec: racks R45
#[test]
fn hot_swap_replaces_a_device_with_one_of_the_same_role_and_stays_on() {
    let scratch = Scratch::new("swap");
    let (mut shell, _, audio) = shell_in(&scratch, "a");
    select_track(&mut shell, audio);
    let effect = add(&mut shell, json!({ "device": gain() })).device;

    // An audio effect rack preset, made from a rack holding a gain.
    let rack = add(&mut shell, json!({ "device": rack_of("audio_effect") })).device;
    let chain = add_chain(&mut shell, rack, "Wide");
    add(&mut shell, json!({ "device": gain(), "chain": chain }));
    let preset = save_preset(&mut shell, rack);
    shell
        .run("device.remove", json!({ "device": rack }))
        .unwrap();
    assert_eq!(track_devices(&shell, audio), vec![effect]);

    shell
        .run("device.hot_swap", json!({ "device": effect }))
        .unwrap();
    let before = steps(&shell);
    load_selected(&mut shell, &preset.path).unwrap();
    assert_eq!(steps(&shell), before + 1, "one undo step");
    assert_eq!(last_step(&shell), "Load Preset");
    let devices = track_devices(&shell, audio);
    assert_eq!(devices.len(), 1);
    let doc = shell.editor().document();
    assert_eq!(doc.device(devices[0]).unwrap().chains[0].name, "Wide");
    assert!(doc.device(effect).is_none(), "the effect is gone");
    assert_eq!(
        view(&shell).hot_swap.unwrap().device,
        devices[0],
        "hot-swap stays on, for the replacement"
    );

    // The next load replaces what the last put there: the empty audio
    // effect rack from the browser's label.
    shell
        .run("browser.open_place", json!({ "label": "audio_effects" }))
        .unwrap();
    load_selected(&mut shell, "rack:audio_effect").unwrap();
    let devices_now = track_devices(&shell, audio);
    assert_eq!(devices_now.len(), 1);
    assert_ne!(devices_now[0], devices[0]);
    assert!(
        shell
            .editor()
            .document()
            .device(devices_now[0])
            .unwrap()
            .chains
            .is_empty()
    );
    assert_eq!(view(&shell).hot_swap.unwrap().device, devices_now[0]);

    // One undo step each way, and an instrument can't take an effect's place.
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(track_devices(&shell, audio), devices);
    let instrument = save_instrument_preset(&mut shell, &scratch);
    shell
        .run("device.hot_swap", json!({ "device": devices[0] }))
        .unwrap();
    let refused = load_selected(&mut shell, &instrument);
    assert!(refused.is_err(), "an instrument for an audio effect");
    assert_eq!(track_devices(&shell, audio), devices);
}

/// An instrument rack preset file, made on the MIDI track.
fn save_instrument_preset(shell: &mut Shell, scratch: &Scratch) -> String {
    let midi = shell.editor().document().top_level().next().unwrap();
    select_track(shell, midi);
    let rack = add(shell, json!({ "device": rack_of("instrument") })).device;
    let path = scratch.path("Instrument.blpreset");
    shell
        .run(
            "device.save_preset",
            json!({ "device": rack, "path": path.to_string_lossy() }),
        )
        .unwrap();
    let audio = shell.editor().document().top_level().nth(2).unwrap();
    select_track(shell, audio);
    path.to_string_lossy().into_owned()
}

// Spec: racks R45, R46
#[test]
fn a_chain_saves_to_a_file_and_loads_back_and_an_armed_chain_is_replaced() {
    let scratch = Scratch::new("chain");
    let (mut shell, _, audio) = shell_in(&scratch, "a");
    select_track(&mut shell, audio);
    let rack = add(&mut shell, json!({ "device": rack_of("audio_effect") })).device;
    let first = add_chain(&mut shell, rack, "Wide");
    let second = add_chain(&mut shell, rack, "Narrow");
    let device = add(&mut shell, json!({ "device": gain(), "chain": first })).device;
    shell
        .run("chain.set_gain", json!({ "chain": first, "volume": 0.25 }))
        .unwrap();

    // Saved to a folder, as dragging the row onto one does.
    let path = scratch.path("Places/Wide.blpreset");
    let saved: Value = shell
        .run(
            "chain.save_preset",
            json!({ "chain": first, "path": path.to_string_lossy() }),
        )
        .unwrap();
    assert_eq!(saved["name"], "Wide");
    assert!(path.is_file());

    // Dropped on the chain list: a new chain at the end, one undo step.
    let before = steps(&shell);
    let added: ChainAdded = serde_json::from_value(
        shell
            .run(
                "chain.load_preset",
                json!({ "path": path.to_string_lossy(), "rack": rack }),
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(steps(&shell), before + 1);
    let doc = shell.editor().document();
    let chains = &doc.device(rack).unwrap().chains;
    assert_eq!(chains.len(), 3);
    assert_eq!(chains[2].id, added.chain);
    assert_eq!(chains[2].name, "Wide");
    assert_eq!(chains[2].mixer.volume, 0.25);
    assert_ne!(chains[2].devices[0].id, device, "new IDs");
    assert_eq!(chains[2].devices[0].kind, chains[0].devices[0].kind);

    // Armed by its Hot-Swap button, the second chain is replaced in its
    // place by a chain preset loaded from the browser, and stays armed.
    shell
        .run("chain.hot_swap", json!({ "chain": second }))
        .unwrap();
    let before = steps(&shell);
    load_selected(&mut shell, &path.to_string_lossy()).unwrap();
    assert_eq!(steps(&shell), before + 1);
    let doc = shell.editor().document();
    let chains = &doc.device(rack).unwrap().chains;
    assert_eq!(chains.len(), 3, "replaced, not added");
    assert!(doc.chain(second).is_none());
    assert_eq!(chains[1].name, "Wide", "in the old chain's place");
    let new = chains[1].id;
    let rows = || -> Vec<(ChainId, bool)> {
        view(&shell)
            .devices
            .iter()
            .find(|d| d.id == rack)
            .and_then(|d| d.rack.clone())
            .unwrap()
            .chains
            .iter()
            .map(|row| (row.id, row.hot_swap))
            .collect()
    };
    assert!(rows().contains(&(new, true)), "the new chain is armed");

    // A device from the browser becomes the armed chain's only device,
    // keeping its name and mixer.
    let before = steps(&shell);
    let effects: bl_shell_spec::browser::BrowserState = serde_json::from_value(
        shell
            .run("browser.open_place", json!({ "label": "audio_effects" }))
            .unwrap(),
    )
    .unwrap();
    let native = effects
        .rows
        .iter()
        .find(|row| !row.path.starts_with("rack:"))
        .expect("a native audio effect is listed")
        .path
        .clone();
    load_selected(&mut shell, &native).unwrap();
    assert_eq!(steps(&shell), before + 1);
    let doc = shell.editor().document();
    let chain = doc.chain(new).unwrap();
    assert_eq!(chain.devices.len(), 1);
    assert_eq!(chain.name, "Wide");
    assert_eq!(chain.mixer.volume, 0.25);

    // A chain preset refused by a rack of another kind changes nothing.
    let midi = shell.editor().document().top_level().next().unwrap();
    select_track(&mut shell, midi);
    let midi_rack = add(&mut shell, json!({ "device": rack_of("midi_effect") })).device;
    let steps_now = steps(&shell);
    assert!(
        shell
            .run(
                "chain.load_preset",
                json!({ "path": path.to_string_lossy(), "rack": midi_rack }),
            )
            .is_err()
    );
    assert_eq!(steps(&shell), steps_now);
}

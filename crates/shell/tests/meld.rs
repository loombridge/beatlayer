//! Meld in the shell (WP-8.17; the meld spec, R3, R5 and R19): its macros
//! are named by their oscillator's type and keep their IDs, its Semitones
//! read in scale degrees while Use Current Scale is on, and its engine
//! selector is view state. Driven through `Shell::run`, as an agent drives
//! it.

use bl_devices::meld::{
    FILTER_MACRO_1, FILTER_TYPE, MACRO_1, MACRO_2, MELD, SEMITONES, TYPE, USE_SCALE, param,
    route_amount,
};
use bl_model::{DeviceId, DeviceKind, DeviceRole, TrackId, TrackKind};
use bl_shell::Shell;
use bl_shell::device_commands::{DeviceAdded, DeviceParameters};
use serde_json::json;

fn midi_track(shell: &Shell) -> TrackId {
    shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Midi)
        .map(bl_model::Track::id)
        .unwrap()
}

fn add_meld(shell: &mut Shell) -> DeviceId {
    let track = midi_track(shell);
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    shell.set_focused_area(bl_shell::Area::Detail);
    let added: DeviceAdded = serde_json::from_value(
        shell
            .run(
                "device.add",
                json!({ "device": {
                    "kind": DeviceKind::Native { device_type: MELD.to_owned() },
                    "name": "Meld",
                    "role": DeviceRole::Instrument,
                } }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
}

fn set(shell: &mut Shell, device: DeviceId, id: bl_model::ParamId, value: f64) {
    shell
        .run(
            "device.set_param",
            json!({ "device": device, "param": id, "value": value }),
        )
        .unwrap();
}

fn read(shell: &mut Shell, device: DeviceId) -> DeviceParameters {
    serde_json::from_value(
        shell
            .run("device.parameters", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap()
}

// Spec: meld R3, R5
#[test]
fn the_macros_are_named_by_the_type_and_semitones_read_in_scale_degrees() {
    let mut shell = Shell::headless();
    let device = add_meld(&mut shell);
    let name = |read: &DeviceParameters, id| {
        let p = read.params.iter().find(|p| p.descriptor.id == id).unwrap();
        (p.descriptor.name.clone(), p.text.clone())
    };
    let first = read(&mut shell, device);
    assert_eq!(name(&first, param(0, MACRO_1)).0, "Shape");
    assert_eq!(name(&first, param(0, MACRO_2)).0, "Tone");
    assert_eq!(name(&first, param(0, FILTER_MACRO_1)).0, "Q");
    // Square Sync names its macros Freq 1 and Freq 2, with the same IDs and
    // values.
    set(&mut shell, device, param(0, MACRO_1), 40.0);
    set(&mut shell, device, param(0, TYPE), 3.0);
    let second = read(&mut shell, device);
    assert_eq!(
        name(&second, param(0, MACRO_1)),
        ("Freq 1".to_owned(), "40 %".to_owned())
    );
    assert_eq!(name(&second, param(0, MACRO_2)).0, "Freq 2");
    assert_eq!(
        name(&second, param(1, MACRO_1)).0,
        "Shape",
        "B keeps its own type"
    );
    // The filter's macros follow its type.
    set(&mut shell, device, param(1, FILTER_TYPE), 8.0);
    assert_eq!(
        name(&read(&mut shell, device), param(1, FILTER_MACRO_1)).0,
        "Boost"
    );
    // Semitones read in degrees while Use Current Scale is on.
    set(&mut shell, device, param(0, SEMITONES), 2.0);
    assert_eq!(
        name(&read(&mut shell, device), param(0, SEMITONES)).1,
        "2 st"
    );
    set(&mut shell, device, USE_SCALE, 1.0);
    assert_eq!(
        name(&read(&mut shell, device), param(0, SEMITONES)).1,
        "2 sd"
    );
}

// Spec: meld R19
#[test]
fn the_engine_selector_is_view_state_that_shows_one_engines_controls() {
    let mut shell = Shell::headless();
    let device = add_meld(&mut shell);
    let history = |shell: &Shell| -> Vec<String> {
        shell.editor().undo_history().map(str::to_owned).collect()
    };
    let (steps, dirty) = (history(&shell), shell.is_dirty());
    let reply: serde_json::Value = shell
        .invoke(
            "device.set_page",
            json!({ "device": device, "node": "Engine", "page": "B" }),
        )
        .unwrap();
    assert_eq!(reply["devices"][0]["panel"]["pages"][0]["shown"], "B");
    assert_eq!(history(&shell), steps, "no undo step");
    assert_eq!(shell.is_dirty(), dirty, "and the set is unchanged");
}

// ---- Meld, part 4 (WP-8.20; R62 to R64, R68 and R70) ----

fn routes(shell: &Shell, device: DeviceId) -> Vec<(String, String, f64)> {
    let document = shell.editor().document();
    let device = document.device(device).unwrap();
    let mut routes: Vec<_> = device
        .routes
        .iter()
        .map(|r| {
            let amount = device
                .params
                .get(&route_amount(usize::from(r.slot)))
                .copied()
                .unwrap_or(0.0);
            (r.source.clone(), r.destination.clone(), amount)
        })
        .collect();
    routes.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    routes
}

fn route(shell: &mut Shell, device: DeviceId, source: &str, destination: &str, amount: f64) {
    shell
        .run(
            "device.set_route",
            json!({
                "device": device,
                "source": source,
                "destination": destination,
                "amount": amount,
            }),
        )
        .unwrap();
}

fn last_step(shell: &Shell) -> String {
    shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn row(source: &str, destination: &str, amount: f64) -> (String, String, f64) {
    (source.to_owned(), destination.to_owned(), amount)
}

// Spec: meld R64
#[test]
fn copy_to_the_other_engine_replaces_its_routes_into_those_destinations_in_one_undo_step() {
    let mut shell = Shell::headless();
    let device = add_meld(&mut shell);
    route(&mut shell, device, "A LFO 1", "A Filter Frequency", 40.0);
    route(&mut shell, device, "A Mod Env", "A Pitch Quant", 20.0);
    route(&mut shell, device, "Velocity", "A Volume", 30.0);
    route(&mut shell, device, "A LFO 2", "Mixer Drive", 10.0);
    // Engine B has a route into a destination A also has, which the copy
    // replaces, and one into another, which it leaves.
    route(&mut shell, device, "B LFO 2", "B Filter Frequency", 70.0);
    route(&mut shell, device, "B LFO 2", "B Pan", 50.0);
    let before = routes(&shell, device);
    let steps = shell.editor().undo_history().count();
    let copied = shell
        .run(
            "meld.copy_modulation",
            json!({ "device": device, "engine": "A" }),
        )
        .unwrap();
    assert_eq!(
        copied,
        json!({ "device": device, "from": "A", "to": "B", "routes": 3 })
    );
    assert_eq!(
        shell.editor().undo_history().count(),
        steps + 1,
        "one undo step"
    );
    assert_eq!(last_step(&shell), "Copy Modulation");
    let mut want = vec![
        row("A LFO 1", "A Filter Frequency", 40.0),
        row("A Mod Env", "A Pitch Quant", 20.0),
        row("Velocity", "A Volume", 30.0),
        row("A LFO 2", "Mixer Drive", 10.0),
        row("B LFO 1", "B Filter Frequency", 40.0),
        row("B Mod Env", "B Pitch Quant", 20.0),
        row("Velocity", "B Volume", 30.0),
        row("B LFO 2", "B Pan", 50.0),
    ];
    want.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    assert_eq!(routes(&shell, device), want);
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(
        routes(&shell, device),
        before,
        "undo brings every route back"
    );
    // The other way: B's routes go to A, replacing A's.
    shell.run("edit.redo", json!({})).unwrap();
    route(&mut shell, device, "B LFO 1", "B Tone", 15.0);
    shell
        .run(
            "meld.copy_modulation",
            json!({ "device": device, "engine": "B" }),
        )
        .unwrap();
    assert!(routes(&shell, device).contains(&row("A LFO 1", "A Tone", 15.0)));
}

// Spec: meld R64
#[test]
fn copy_merges_two_slots_with_the_same_source_and_destination_into_one_route() {
    let mut shell = Shell::headless();
    let device = add_meld(&mut shell);
    for (slot, amount) in [(0, 30.0), (1, 20.0)] {
        shell
            .run(
                "device.set_route",
                json!({
                    "device": device,
                    "slot": slot,
                    "source": "A LFO 1",
                    "destination": "A Filter Frequency",
                    "amount": amount,
                }),
            )
            .unwrap();
    }
    shell
        .run(
            "meld.copy_modulation",
            json!({ "device": device, "engine": "A" }),
        )
        .unwrap();
    let copied: Vec<_> = routes(&shell, device)
        .into_iter()
        .filter(|r| r.1 == "B Filter Frequency")
        .collect();
    assert_eq!(copied, [row("B LFO 1", "B Filter Frequency", 50.0)]);
}

// Spec: meld R64
#[test]
fn clear_removes_the_engines_routes_and_its_modulators_routes_to_the_globals_in_one_undo_step() {
    let mut shell = Shell::headless();
    let device = add_meld(&mut shell);
    route(&mut shell, device, "B LFO 1", "B Filter Frequency", 40.0);
    route(&mut shell, device, "Velocity", "B Volume", 30.0);
    route(&mut shell, device, "B LFO 2", "Volume", 10.0);
    route(&mut shell, device, "Random", "Volume", 25.0);
    route(&mut shell, device, "A LFO 1", "A Pan", 60.0);
    let steps = shell.editor().undo_history().count();
    let cleared = shell
        .run(
            "meld.clear_modulation",
            json!({ "device": device, "engine": "B" }),
        )
        .unwrap();
    assert_eq!(
        cleared,
        json!({ "device": device, "engine": "B", "routes": 3 })
    );
    assert_eq!(shell.editor().undo_history().count(), steps + 1);
    assert_eq!(last_step(&shell), "Clear Modulation");
    assert_eq!(
        routes(&shell, device),
        [row("A LFO 1", "A Pan", 60.0), row("Random", "Volume", 25.0)],
        "engine A's and the shared sources' routes stay, and a cleared slot's amount is 0"
    );
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(routes(&shell, device).len(), 5);
}

// Spec: meld R64
#[test]
fn copy_and_clear_refuse_an_engine_that_isnt_there_a_full_matrix_and_a_device_that_isnt_a_meld() {
    let mut shell = Shell::headless();
    let device = add_meld(&mut shell);
    for command in ["meld.copy_modulation", "meld.clear_modulation"] {
        assert!(
            shell
                .run(command, json!({ "device": device, "engine": "C" }))
                .is_err()
        );
    }
    // Sixty-four routes fill the matrix: a copy that needs a slot is refused.
    let sources = ["A LFO 1", "A LFO 2", "A Mod Env", "A Amp Env", "A LFO 1 FX"];
    let destinations = [
        "A Semitones",
        "A Cents",
        "A Pitch Quant",
        "A Macro 1",
        "A Macro 2",
        "A Filter Frequency",
        "A Filter Macro 1",
        "A Filter Macro 2",
        "A Attack",
        "A Decay",
        "A Sustain",
        "A Release",
        "A Volume",
    ];
    let mut made = 0;
    'fill: for destination in destinations {
        for source in sources {
            route(&mut shell, device, source, destination, 10.0);
            made += 1;
            if made == 64 {
                break 'fill;
            }
        }
    }
    assert_eq!(routes(&shell, device).len(), 64);
    let before = routes(&shell, device);
    assert!(
        shell
            .run(
                "meld.copy_modulation",
                json!({ "device": device, "engine": "A" })
            )
            .is_err(),
        "the copy needs slots the matrix hasn't"
    );
    assert_eq!(routes(&shell, device), before, "nothing changed");
    let mut other = Shell::headless();
    let collision = add_device(&mut other, bl_devices::collision::COLLISION);
    assert!(
        other
            .run(
                "meld.copy_modulation",
                json!({ "device": collision, "engine": "A" })
            )
            .is_err()
    );
}

fn add_device(shell: &mut Shell, device_type: &str) -> DeviceId {
    let track = midi_track(shell);
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    shell.set_focused_area(bl_shell::Area::Detail);
    let added: DeviceAdded = serde_json::from_value(
        shell
            .run(
                "device.add",
                json!({ "device": {
                    "kind": DeviceKind::Native { device_type: device_type.to_owned() },
                    "name": "Device",
                    "role": DeviceRole::Instrument,
                } }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
}

// Spec: meld R62
#[test]
fn a_route_of_one_engine_to_the_other_is_refused_and_the_names_are_unambiguous() {
    let mut shell = Shell::headless();
    let device = add_meld(&mut shell);
    for (source, destination) in [("A LFO 1", "B Filter Frequency"), ("B Mod Env", "A Volume")] {
        assert!(
            shell
                .run(
                    "device.set_route",
                    json!({ "device": device, "source": source, "destination": destination })
                )
                .is_err(),
            "{source} to {destination}"
        );
    }
    // Both engines have their own Filter Frequency, and the shared sources
    // reach either.
    route(&mut shell, device, "Velocity", "A Filter Frequency", 10.0);
    route(&mut shell, device, "Velocity", "B Filter Frequency", 20.0);
    assert_eq!(routes(&shell, device).len(), 2);
}

// Spec: meld R68
#[test]
fn switching_tabs_is_view_state_and_never_an_undo_step() {
    let mut shell = Shell::headless();
    let device = add_meld(&mut shell);
    let steps = shell.editor().undo_history().count();
    let dirty = shell.is_dirty();
    for page in ["LFOs", "Matrix", "MIDI", "MPE", "Settings", "Envelopes"] {
        let reply: serde_json::Value = shell
            .run(
                "device.set_page",
                json!({ "device": device, "node": "A Tabs", "page": page }),
            )
            .unwrap();
        let shown = reply["devices"][0]["panel"]["pages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["node"] == "A Tabs")
            .map(|node| node["shown"].clone());
        assert_eq!(shown, Some(json!(page)), "{page} shows");
    }
    shell
        .run(
            "device.set_page",
            json!({ "device": device, "node": "Engine", "page": "B" }),
        )
        .unwrap();
    assert_eq!(shell.editor().undo_history().count(), steps, "no undo step");
    assert_eq!(shell.is_dirty(), dirty, "and the set is unchanged");
}

/// A folder for one test's files, removed when the test ends.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-meld-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// Spec: meld R70
#[test]
fn routes_and_settings_save_with_the_set_and_read_back_when_it_reopens() {
    use bl_shell::audio::AudioHost;
    use bl_shell::files::ProjectFiles;
    use bl_shell::settings::Preferences;
    let scratch = Scratch::new("reopen");
    let mut shell = Shell::new(
        Preferences::default(),
        Some(scratch.0.join("config")),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    let device = add_meld(&mut shell);
    route(&mut shell, device, "A LFO 1", "A Filter Frequency", 40.0);
    route(&mut shell, device, "Velocity", "B Volume", -30.0);
    route(&mut shell, device, "A Mod Env", "Mixer Drive", 15.0);
    for (offset, value) in [
        (bl_devices::meld::ENV_MODE, 3.0),
        (bl_devices::meld::LFO1_TYPE, 4.0),
        (bl_devices::meld::LFO2_RATE, 9.5),
        (bl_devices::meld::LINK, 1.0),
        (bl_devices::meld::FX2_TYPE, 7.0),
    ] {
        set(&mut shell, device, param(0, offset), value);
    }
    let want = routes(&shell, device);
    let saved: bl_shell::commands::SetResult = serde_json::from_value(
        shell
            .run(
                "set.save_as",
                json!({ "path": scratch.0.join("Song.blset").to_string_lossy() }),
            )
            .unwrap(),
    )
    .unwrap();
    shell
        .run("set.open", json!({ "path": saved.path.expect("saved") }))
        .unwrap();
    assert_eq!(routes(&shell, device), want);
    let read = read(&mut shell, device);
    let value = |id| {
        read.params
            .iter()
            .find(|p| p.descriptor.id == id)
            .unwrap()
            .value
    };
    assert_eq!(value(param(0, bl_devices::meld::ENV_MODE)), 3.0);
    assert_eq!(value(param(0, bl_devices::meld::LFO1_TYPE)), 4.0);
    assert_eq!(value(param(0, bl_devices::meld::LFO2_RATE)), 9.5);
    assert_eq!(value(param(0, bl_devices::meld::LINK)), 1.0);
    assert_eq!(value(param(0, bl_devices::meld::FX2_TYPE)), 7.0);
}

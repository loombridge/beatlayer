//! Device pages in the shell (WP-8.1; the device framework spec, R41 to
//! R45; the multisampler spec, R75): a pages node's shown page is view state
//! that `device.set_page` sets and `device_view.state` reads back, that is
//! no undo step and never marks the set changed, and that a set keeps. The
//! device is the framework's test pages device.

use std::path::PathBuf;

use bl_devices::multisampler::MULTISAMPLER;
use bl_devices::testing::{self, PAGES_SHELL, PAGES_TABS, PAGES_TITLE};
use bl_model::device_layout::Size;
use bl_model::{DeviceId, DeviceKind, DeviceRole, TrackId, TrackKind};
use bl_shell::audio::AudioHost;
use bl_shell::device_commands::{DeviceAdded, DeviceFrame, FloatingWindows, PagesFrame};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::{Area, Shell};
use serde_json::{Value, json};

fn shell() -> (Shell, TrackId) {
    let mut shell = Shell::headless();
    shell.set_device_types(testing::device_types());
    let audio = shell.editor().document().top_level().nth(2).unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    shell.set_focused_area(Area::Detail);
    (shell, audio)
}

fn add_pages(shell: &mut Shell) -> DeviceId {
    let t = testing::pages_type();
    let added: DeviceAdded = serde_json::from_value(
        shell
            .invoke(
                "device.add",
                json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
}

fn frame(shell: &Shell, device: DeviceId) -> DeviceFrame {
    shell
        .device_view_state()
        .devices
        .into_iter()
        .find(|d| d.id == device)
        .unwrap()
}

fn shown(shell: &Shell, device: DeviceId, node: &str) -> String {
    let frame = frame(shell, device);
    let pages = &frame.panel.as_ref().unwrap().pages;
    pages.iter().find(|p| p.node == node).unwrap().shown.clone()
}

/// The parameters whose controls the device's body places now.
fn placed(shell: &Shell, device: DeviceId) -> Vec<u32> {
    let frame = frame(shell, device);
    let layout = frame.panel.as_ref().unwrap().layout(false);
    let mut placed: Vec<u32> = layout
        .place(layout.docked_size())
        .controls
        .iter()
        .map(|c| c.param.0)
        .collect();
    placed.sort_unstable();
    placed
}

// Spec: device-framework R43
#[test]
fn a_new_device_shows_each_nodes_first_page_and_set_page_reads_back() {
    let (mut shell, _) = shell();
    let device = add_pages(&mut shell);
    let panel = frame(&shell, device).panel.unwrap();
    let names = |pages: &[&str]| pages.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        panel.pages,
        [
            PagesFrame {
                node: PAGES_TABS.to_owned(),
                style: bl_model::device_layout::PageStyle::Tabs,
                pages: names(&["Osc", "Filter", "Amp"]),
                shown: "Osc".to_owned(),
            },
            PagesFrame {
                node: PAGES_SHELL.to_owned(),
                style: bl_model::device_layout::PageStyle::Shell,
                pages: names(&["Env", "LFO", "Out"]),
                shown: "Env".to_owned(),
            },
            PagesFrame {
                node: PAGES_TITLE.to_owned(),
                style: bl_model::device_layout::PageStyle::TitleBar,
                pages: names(&["Main", "Aux"]),
                shown: "Main".to_owned(),
            },
        ]
    );
    // The first pages' controls, and the shell's three sections.
    assert_eq!(placed(&shell, device), [0, 1, 5, 6, 7, 8, 11]);

    // By name, by index; each node on its own.
    shell
        .invoke(
            "device.set_page",
            json!({ "device": device, "node": PAGES_TABS, "page": "Filter" }),
        )
        .unwrap();
    assert_eq!(shown(&shell, device, PAGES_TABS), "Filter");
    assert_eq!(shown(&shell, device, PAGES_SHELL), "Env");
    assert_eq!(placed(&shell, device), [2, 3, 5, 6, 7, 8, 11]);
    let reply: Value = shell
        .invoke(
            "device.set_page",
            json!({ "device": device, "node": PAGES_SHELL, "page": 2 }),
        )
        .unwrap();
    assert_eq!(
        reply["devices"][0]["panel"]["pages"][1]["shown"], "Out",
        "the command returns the state it changed"
    );
    assert_eq!(placed(&shell, device), [2, 3, 5, 6, 7, 10, 11]);

    // What the device has not got is refused, and changes nothing.
    for args in [
        json!({ "device": device, "node": "Nope", "page": "Osc" }),
        json!({ "device": device, "node": PAGES_TABS, "page": "Nope" }),
        json!({ "device": device, "node": PAGES_TABS, "page": 3 }),
    ] {
        assert!(shell.invoke("device.set_page", args).is_err());
    }
    assert_eq!(shown(&shell, device, PAGES_TABS), "Filter");
    // A device with no pages nodes has none to show.
    let utility = testing::utility_type();
    let other: DeviceAdded = serde_json::from_value(
        shell
            .invoke(
                "device.add",
                json!({ "device": { "kind": utility.kind, "name": utility.name, "role": utility.role } }),
            )
            .unwrap(),
    )
    .unwrap();
    assert!(
        shell
            .invoke(
                "device.set_page",
                json!({ "device": other.device, "node": PAGES_TABS, "page": "Osc" }),
            )
            .is_err()
    );
}

// Spec: device-framework R43
#[test]
fn switching_pages_is_no_undo_step_and_leaves_the_set_unchanged() {
    let (mut shell, _) = shell();
    let device = add_pages(&mut shell);
    let history = |shell: &Shell| -> Vec<String> {
        shell.editor().undo_history().map(str::to_owned).collect()
    };
    let (steps, dirty) = (history(&shell), shell.is_dirty());
    shell
        .invoke(
            "device.set_page",
            json!({ "device": device, "node": PAGES_TABS, "page": "Amp" }),
        )
        .unwrap();
    assert_eq!(history(&shell), steps);
    assert_eq!(shell.is_dirty(), dirty);
}

// Spec: device-framework R42, R15
#[test]
fn a_floating_window_shows_the_same_page_and_grows_with_its_display_cells() {
    let (mut shell, _) = shell();
    let device = add_pages(&mut shell);
    shell
        .run(
            "device.set_floating",
            json!({ "device": device, "floating": true }),
        )
        .unwrap();
    let windows = |shell: &mut Shell| -> FloatingWindows {
        serde_json::from_value(shell.run("device.windows", json!({})).unwrap()).unwrap()
    };
    let docked = windows(&mut shell).windows[0].clone();
    assert_eq!(docked.view.panel.as_ref().unwrap().pages[0].shown, "Osc");
    assert!(!docked.grows_across, "the first page has no display cell");
    let min = (docked.min_width, docked.min_height);
    shell
        .invoke(
            "device.set_page",
            json!({ "device": device, "node": PAGES_TABS, "page": "Amp" }),
        )
        .unwrap();
    let after = windows(&mut shell).windows[0].clone();
    assert_eq!(after.view.panel.as_ref().unwrap().pages[0].shown, "Amp");
    assert_eq!(
        (after.min_width, after.min_height),
        min,
        "the size is the largest page's"
    );
    assert!(
        after.grows_across && after.grows_down,
        "the shown page's cell"
    );
    let size = after
        .view
        .panel
        .as_ref()
        .unwrap()
        .layout(false)
        .fit(Size::new(500.0, 400.0));
    assert_eq!(size, Size::new(500.0, 400.0));
}

// Spec: device-framework R45
#[test]
fn a_parameter_on_a_hidden_page_is_set_read_and_banked_as_a_shown_one() {
    let (mut shell, _) = shell();
    let device = add_pages(&mut shell);
    // P3 is on the Filter tab, hidden while Osc shows.
    assert!(!placed(&shell, device).contains(&3));
    shell
        .invoke(
            "device.set_param",
            json!({ "device": device, "param": 3, "value": 40.0 }),
        )
        .unwrap();
    let parameters: Value = shell
        .invoke("device.parameters", json!({ "device": device }))
        .unwrap();
    let all = parameters["params"].as_array().unwrap();
    assert_eq!(all.len(), 13, "every parameter, shown or not");
    let hidden = all.iter().find(|p| p["id"] == json!(3)).unwrap();
    assert_eq!(hidden["value"], json!(40.0));
    // It keeps its value when its page shows.
    shell
        .invoke(
            "device.set_page",
            json!({ "device": device, "node": PAGES_TABS, "page": "Filter" }),
        )
        .unwrap();
    let panel = frame(&shell, device).panel.unwrap();
    assert_eq!(panel.value(3), 40.0);
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-pages-{name}-{}-{}",
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

fn files_shell(scratch: &Scratch) -> Shell {
    let mut shell = Shell::new(
        Preferences::default(),
        Some(scratch.0.join("config")),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    let mut types = bl_devices::device_types();
    types.extend(testing::device_types());
    shell.set_device_types(types);
    let audio = shell.editor().document().top_level().nth(2).unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    shell
}

/// Saves the set as a file of `scratch`, and returns where it went.
fn save(shell: &mut Shell, scratch: &Scratch) -> PathBuf {
    let saved: bl_shell::commands::SetResult = serde_json::from_value(
        shell
            .run(
                "set.save_as",
                json!({ "path": scratch.0.join("Song.blset").to_string_lossy() }),
            )
            .unwrap(),
    )
    .unwrap();
    PathBuf::from(saved.path.expect("the set was saved"))
}

fn set_page(shell: &mut Shell, device: DeviceId, node: &str, page: &str) {
    shell
        .run(
            "device.set_page",
            json!({ "device": device, "node": node, "page": page }),
        )
        .unwrap();
}

// Spec: device-framework R43
#[test]
fn a_set_saved_and_reopened_opens_on_its_pages() {
    let scratch = Scratch::new("saved");
    let mut shell = files_shell(&scratch);
    let device = add_pages(&mut shell);
    set_page(&mut shell, device, PAGES_TABS, "Amp");
    set_page(&mut shell, device, PAGES_SHELL, "LFO");
    let path = save(&mut shell, &scratch);
    // Pages switched after the save are not what the set keeps.
    set_page(&mut shell, device, PAGES_TABS, "Osc");
    set_page(&mut shell, device, PAGES_SHELL, "Out");
    shell
        .run("set.open", json!({ "path": path.to_string_lossy() }))
        .unwrap();
    let audio = shell.editor().document().device_track(device).unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    assert_eq!(shown(&shell, device, PAGES_TABS), "Amp");
    assert_eq!(shown(&shell, device, PAGES_SHELL), "LFO");
}

fn add_multisampler(shell: &mut Shell) -> DeviceId {
    let midi = shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Midi)
        .map(bl_model::Track::id)
        .unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [midi] }))
        .unwrap();
    let added: DeviceAdded = serde_json::from_value(
        shell
            .run(
                "device.add",
                json!({ "device": {
                    "kind": DeviceKind::Native { device_type: MULTISAMPLER.to_owned() },
                    "name": "Sampler",
                    "role": DeviceRole::Instrument,
                } }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
}

// Spec: multisampler R75
#[test]
fn the_multisamplers_tabs_are_pages_and_a_set_saved_before_them_opens_on_its_tab() {
    let scratch = Scratch::new("tabs");
    let mut shell = files_shell(&scratch);
    let device = add_multisampler(&mut shell);
    let tabs = |shell: &Shell| shown(shell, device, "Tabs");
    assert_eq!(tabs(&shell), "Sample");
    let panel = frame(&shell, device).panel.unwrap();
    assert_eq!(
        panel.pages[0].pages,
        ["Sample", "Pitch/Osc", "Filter/Global", "Modulation", "MIDI"]
    );
    assert_eq!(
        panel.pages[0].style,
        bl_model::device_layout::PageStyle::TitleBar
    );
    // The page and the saved view's tab are one thing, whichever sets it.
    set_page(&mut shell, device, "Tabs", "Modulation");
    let view = shell
        .run("device.set_multisampler_view", json!({ "device": device }))
        .unwrap();
    assert_eq!(view["tab"], "modulation");
    shell
        .run(
            "device.set_multisampler_view",
            json!({ "device": device, "tab": "filter_global" }),
        )
        .unwrap();
    assert_eq!(tabs(&shell), "Filter/Global");
    shell
        .run(
            "device.set_page",
            json!({ "device": device, "node": "Tabs", "page": 4 }),
        )
        .unwrap();
    assert_eq!(tabs(&shell), "MIDI");
    // A tab's controls are placed only while it shows, and the device is as
    // wide whichever shows.
    let width = |shell: &Shell| {
        frame(shell, device)
            .panel
            .unwrap()
            .layout(false)
            .docked_size()
            .width
    };
    let widest = width(&shell);
    for tab in ["Sample", "Pitch/Osc", "Filter/Global", "Modulation"] {
        set_page(&mut shell, device, "Tabs", tab);
        assert_eq!(width(&shell), widest, "{tab}");
    }
    // The set keeps the tab where it always has, so an older set opens on it.
    set_page(&mut shell, device, "Tabs", "Filter/Global");
    let path = save(&mut shell, &scratch);
    shell
        .run("set.open", json!({ "path": path.to_string_lossy() }))
        .unwrap();
    let midi = shell.editor().document().device_track(device).unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [midi] }))
        .unwrap();
    assert_eq!(tabs(&shell), "Filter/Global");
}

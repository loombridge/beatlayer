//! Browse Mode (`docs/specs/push-2-tracks.md`, R5 to R14) on the simulated
//! Push 2, with a catalogue of the test's own.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bl_control::browse::{Browse, Catalogue, Entry, Load};
use bl_model::{Command, Device, DeviceId, DeviceKind, DeviceRole, TrackKind};
use bl_push::browse::LoadKind;
use serde_json::json;

use crate::rig::Rig;

/// How many presets the catalogue has indexed so far.
#[derive(Debug, Default)]
struct Fake {
    presets: Arc<AtomicUsize>,
}

fn folder(key: &str, name: &str) -> Entry {
    Entry {
        key: key.to_owned(),
        name: name.to_owned(),
        load: None,
        folder: true,
    }
}

fn device(name: &str, role: DeviceRole) -> Entry {
    Entry {
        key: name.to_owned(),
        name: name.to_owned(),
        load: Some(Load::Device {
            new_device: json!({ "name": name }),
            role,
        }),
        folder: false,
    }
}

fn preset(name: &str, role: DeviceRole) -> Entry {
    Entry {
        key: format!("/p/{name}"),
        name: name.to_owned(),
        load: Some(Load::Preset {
            path: format!("/p/{name}.blpreset"),
            role: Some(role),
        }),
        folder: false,
    }
}

impl Catalogue for Fake {
    fn labels(&self, role: Option<DeviceRole>) -> Vec<Entry> {
        let all = [
            ("sounds", "Sounds", Some(DeviceRole::Instrument)),
            ("drums", "Drums", Some(DeviceRole::Instrument)),
            ("instruments", "Instruments", Some(DeviceRole::Instrument)),
            (
                "audio_effects",
                "Audio Effects",
                Some(DeviceRole::AudioEffect),
            ),
            ("midi_effects", "MIDI Effects", Some(DeviceRole::MidiEffect)),
            ("plug_ins", "Plug-Ins", None),
        ];
        all.into_iter()
            .filter(|(_, _, own)| role.is_none() || own.is_none() || *own == role)
            .map(|(key, name, _)| folder(key, name))
            .collect()
    }

    fn children(&self, key: &str, _role: Option<DeviceRole>) -> Vec<Entry> {
        match key {
            "sounds" => (0..self.presets.load(Ordering::Relaxed))
                .map(|n| preset(&format!("Sound {n}"), DeviceRole::Instrument))
                .collect(),
            "drums" => vec![device("Drum Rack", DeviceRole::Instrument)],
            "instruments" => (1..=12)
                .map(|n| device(&format!("Instrument {n:02}"), DeviceRole::Instrument))
                .collect(),
            "audio_effects" => vec![
                folder("fx_presets", "Presets"),
                device("Reverb", DeviceRole::AudioEffect),
                device("Delay", DeviceRole::AudioEffect),
            ],
            "fx_presets" => vec![preset("Big Hall", DeviceRole::AudioEffect)],
            "midi_effects" => vec![device("Arpeggiator", DeviceRole::MidiEffect)],
            "plug_ins" => vec![folder("plug-ins:/Acme", "Acme")],
            "plug-ins:/Acme" => vec![Entry {
                key: "/acme.clap#synth".to_owned(),
                name: "Acme Synth".to_owned(),
                load: Some(Load::Plugin {
                    path: "/acme.clap".to_owned(),
                    plugin: "synth".to_owned(),
                    role: DeviceRole::Instrument,
                }),
                folder: false,
            }],
            _ => Vec::new(),
        }
    }
}

struct Browsing {
    rig: Rig,
    presets: Arc<AtomicUsize>,
    revision: u64,
}

impl Browsing {
    fn new() -> Self {
        let mut rig = Rig::browsing();
        let presets = Arc::new(AtomicUsize::new(0));
        rig.model.browse = Some(Browse {
            revision: 1,
            preview: false,
            catalogue: Arc::new(Fake {
                presets: Arc::clone(&presets),
            }),
        });
        rig.model.selection.tracks = vec![rig.tracks[0]];
        rig.start();
        rig.tick();
        Self {
            rig,
            presets,
            revision: 1,
        }
    }

    fn browse(&mut self) {
        self.rig.tap("browse");
    }

    /// What Browse Mode shows: its columns' selections and what Load does.
    fn state(&self) -> bl_push::browse::State {
        self.rig.snapshot().browse.expect("Browse Mode is shown")
    }

    fn selected(&self, column: usize) -> Option<usize> {
        self.state().columns[column].selected
    }

    fn names(&self, column: usize) -> Vec<String> {
        self.state().columns[column].entries.clone()
    }

    fn set_preview(&mut self, on: bool) {
        self.rig.model.browse.as_mut().unwrap().preview = on;
        self.rig.tick();
    }

    /// The catalogue has changed: the surface reads it again.
    fn indexed(&mut self, presets: usize) {
        self.presets.store(presets, Ordering::Relaxed);
        self.revision += 1;
        self.rig.model.browse.as_mut().unwrap().revision = self.revision;
        self.rig.tick();
    }

    fn select_device(&mut self, track: usize, role: DeviceRole) -> DeviceId {
        let id = self.rig.editor.new_id();
        let device = Device::new(
            id,
            "Selected",
            DeviceKind::Native {
                device_type: "test".to_owned(),
            },
        )
        .with_role(role);
        self.rig.apply(Command::InsertDevice {
            track: self.rig.tracks[track],
            index: 0,
            device,
        });
        self.rig.model.selection.tracks = vec![self.rig.tracks[track]];
        self.rig.model.selected_device = Some(id);
        self.rig.tick();
        id
    }
}

// Spec: push-2-tracks R5, R6, R14. Browse on an empty MIDI track lists
// everything a MIDI track can hold, with what the first label holds in the
// column after it, and changes nothing in the set.
#[test]
fn browse_on_an_empty_midi_track_lists_what_it_can_hold() {
    let mut b = Browsing::new();
    b.browse();
    let state = b.state();
    assert_eq!(state.load, LoadKind::Add);
    assert_eq!(
        state.columns[0].entries,
        [
            "Sounds",
            "Drums",
            "Instruments",
            "Audio Effects",
            "MIDI Effects",
            "Plug-Ins"
        ]
    );
    assert_eq!(state.columns[0].selected, Some(0));
    assert_eq!(state.in_use, 0);
    let text = b.rig.snapshot().screen.text().join("\n");
    assert!(text.contains("list 1 (in use): *Sounds/, Drums/"), "{text}");
    assert!(
        b.rig.commands().is_empty(),
        "Browse Mode changes nothing until Load"
    );
}

// Spec: push-2-tracks R5. What is listed depends on the device last selected
// on the track: an instrument gives what replaces an instrument, an audio
// effect audio effects, a MIDI effect MIDI effects; an audio track takes audio
// effects only.
#[test]
fn what_is_listed_follows_the_selected_device_and_track() {
    let mut b = Browsing::new();
    b.select_device(0, DeviceRole::Instrument);
    b.browse();
    assert_eq!(
        b.names(0),
        ["Sounds", "Drums", "Instruments", "Plug-Ins"],
        "an instrument is replaced by instruments"
    );
    b.select_device(1, DeviceRole::AudioEffect);
    b.browse();
    assert_eq!(b.names(0), ["Audio Effects", "Plug-Ins"]);
    b.select_device(2, DeviceRole::MidiEffect);
    b.browse();
    assert_eq!(b.names(0), ["MIDI Effects", "Plug-Ins"]);

    let audio = b.rig.editor.new_id();
    b.rig.apply(Command::CreateTrack {
        id: audio,
        kind: TrackKind::Audio,
        name: "Audio".to_owned(),
        parent: None,
        index: 10,
    });
    b.rig.model.selection.tracks = vec![audio];
    b.rig.model.selected_device = None;
    b.rig.tick();
    b.browse();
    assert_eq!(b.names(0), ["Audio Effects", "Plug-Ins"]);
}

// Spec: push-2-tracks R7. Each encoder scrolls the column it sits over, one
// entry for every ten steps, and choosing an entry fills the columns right
// of it again.
#[test]
fn an_encoder_scrolls_its_column_and_fills_the_next() {
    let mut b = Browsing::new();
    b.browse();
    assert!(b.rig.sim.turn(0, 9));
    b.rig.advance(1);
    assert_eq!(b.selected(0), Some(0), "nine steps are not a list step");
    assert!(b.rig.sim.turn(0, 1));
    b.rig.advance(1);
    assert_eq!(b.selected(0), Some(1));
    assert_eq!(b.names(1), ["Drum Rack"]);
    // Two entries on, the label holds twelve devices.
    assert!(b.rig.sim.turn(0, 10));
    b.rig.advance(1);
    assert_eq!(b.selected(0), Some(2));
    assert_eq!(b.names(1).len(), 12);
    // The second encoder scrolls the second column.
    assert!(b.rig.sim.turn(1, 30));
    b.rig.advance(1);
    assert_eq!(
        b.selected(1),
        Some(2),
        "from no selection, three entries on"
    );
    assert_eq!(b.state().in_use, 1);
    // The ends stop it.
    for _ in 0..3 {
        assert!(b.rig.sim.turn(1, 63));
    }
    b.rig.advance(1);
    assert_eq!(b.selected(1), Some(11));
    assert!(b.rig.sim.turn(0, -400));
    b.rig.advance(1);
    assert_eq!(b.selected(0), Some(0));
    assert_eq!(
        b.state().columns.len(),
        2,
        "the columns after were fetched again"
    );
}

// Spec: push-2-tracks R7, R8. The arrows move the selection of the column in
// use and the column in use; the seventh upper button goes up the hierarchy
// and the eighth down.
#[test]
fn the_arrows_and_the_hierarchy_buttons_move_the_columns() {
    let mut b = Browsing::new();
    b.browse();
    b.rig.tap("down");
    assert_eq!(b.selected(0), Some(1));
    b.rig.tap("up");
    b.rig.tap("up");
    assert_eq!(b.selected(0), Some(0), "the first entry stops Up");
    // Sounds holds nothing yet: there is no column to open.
    b.rig.tap("right");
    assert_eq!(b.state().in_use, 0);
    b.rig.tap("down");
    b.rig.tap("right");
    assert_eq!(b.state().in_use, 1);
    b.rig.tap("left");
    assert_eq!(b.state().in_use, 0);
    b.rig.tap("upper_8");
    assert_eq!(b.state().in_use, 1);
    b.rig.tap("upper_7");
    assert_eq!(b.state().in_use, 0);
    // Down opens the folder in Audio Effects, with the folders first.
    for _ in 0..2 {
        b.rig.tap("down");
    }
    b.rig.tap("upper_8");
    assert_eq!(b.names(1), ["Presets", "Reverb", "Delay"]);
    assert_eq!(b.selected(1), Some(0));
    b.rig.tap("upper_8");
    assert_eq!(b.state().in_use, 2, "the selected folder opened");
    assert_eq!(b.names(2), ["Big Hall"]);
    let labels = b.rig.snapshot().screen;
    assert_eq!(labels.upper[6].text, "Up");
    assert_eq!(labels.upper[7].text, "Down");
}

// Spec: push-2-tracks R6. As the selection goes deeper the columns move left,
// so the column in use and the one after it stay on the display.
#[test]
fn deeper_columns_move_left() {
    let mut b = Browsing::new();
    b.browse();
    let text = b.rig.snapshot().screen.text().join("\n");
    assert!(
        text.contains("list 1 (in use)") && text.contains("list 2:"),
        "{text}"
    );
    for _ in 0..5 {
        b.rig.tap("down");
    }
    b.rig.tap("upper_8");
    b.rig.tap("upper_8");
    let state = b.state();
    assert_eq!(state.columns.len(), 3, "Plug-Ins, Acme, Acme Synth");
    let text = b.rig.snapshot().screen.text().join("\n");
    assert!(text.contains("list 3 (in use): *Acme Synth"), "{text}");
}

// Spec: push-2-tracks R9, R11. Load on a device adds it to the selected
// track; it does nothing while the selection is a folder or a label.
#[test]
fn load_adds_the_chosen_device_and_does_nothing_on_a_folder() {
    let mut b = Browsing::new();
    b.browse();
    assert!(
        b.rig.snapshot().screen.lower[7].is_empty(),
        "a label can't load"
    );
    b.rig.tap("lower_8");
    assert!(b.rig.commands().is_empty());
    assert!(b.rig.sim.turn(0, 20));
    b.rig.advance(1);
    b.rig.tap("upper_8");
    assert_eq!(b.rig.snapshot().screen.lower[7].text, "Load");
    b.rig.tap("lower_8");
    assert_eq!(
        b.rig.commands(),
        [r#"device.add {"device":{"name":"Instrument 01"}}"#]
    );
    assert_eq!(
        b.rig.snapshot().display_mode,
        bl_push::content::DisplayMode::Browse
    );
}

// Spec: push-2-tracks R5, R11. With an instrument selected Load replaces it,
// and Push 2 stays in Browse Mode with the loaded device as the one the next
// Load replaces, each Load one command.
#[test]
fn load_replaces_the_selected_device_and_a_second_load_replaces_that() {
    let mut b = Browsing::new();
    let first = b.select_device(0, DeviceRole::Instrument);
    b.browse();
    assert_eq!(b.state().load, LoadKind::Replace(first));
    assert!(b.rig.sim.turn(0, 20));
    b.rig.advance(1);
    b.rig.tap("upper_8");
    b.rig.tap("lower_8");
    assert_eq!(
        b.rig.commands(),
        [format!(
            r#"device.replace {{"device":{},"new_device":{{"name":"Instrument 01"}}}}"#,
            serde_json::to_string(&first).unwrap()
        )]
    );
    // The shell selects the new device; Push 2 follows it.
    let second = b.rig.editor.new_id();
    b.rig.model.selected_device = Some(second);
    b.rig.apply(Command::RemoveDevice { device: first });
    b.rig.apply(Command::InsertDevice {
        track: b.rig.tracks[0],
        index: 0,
        device: Device::new(
            second,
            "Loaded",
            DeviceKind::Native {
                device_type: "test".to_owned(),
            },
        )
        .with_role(DeviceRole::Instrument),
    });
    b.rig.tick();
    assert_eq!(b.state().load, LoadKind::Replace(second));
    assert_eq!(b.state().in_use, 1, "the columns stayed where they were");
    b.rig.tap("lower_8");
    assert!(b.rig.commands()[0].starts_with(&format!(
        r#"device.replace {{"device":{}"#,
        serde_json::to_string(&second).unwrap()
    )));
}

// Spec: push-2-tracks R5. A preset adds as `device.add_preset` and replaces as
// `device.replace` with the preset; a plug-in adds as the window's browser
// adds it.
#[test]
fn presets_and_plug_ins_load_by_their_own_commands() {
    let mut b = Browsing::new();
    b.indexed(2);
    b.browse();
    b.rig.tap("upper_8");
    b.rig.tap("lower_8");
    assert_eq!(
        b.rig.commands(),
        [r#"device.add_preset {"path":"/p/Sound 0.blpreset"}"#]
    );
    for _ in 0..5 {
        b.rig.tap("up");
    }
    b.rig.tap("upper_7");
    for _ in 0..5 {
        b.rig.tap("down");
    }
    b.rig.tap("upper_8");
    b.rig.tap("upper_8");
    b.rig.tap("lower_8");
    assert_eq!(
        b.rig.commands(),
        [r#"plugin.load {"path":"/acme.clap","plugin":"synth"}"#]
    );
}

// Spec: push-2-tracks R12. Add Track puts Push 2 in Browse Mode with the three
// kinds in the leftmost column and what an empty track of each takes after
// it; Load on a kind inserts an empty track of it.
#[test]
fn add_track_offers_the_kinds_and_inserts_an_empty_track() {
    let mut b = Browsing::new();
    b.rig.tap("add_track");
    let state = b.state();
    assert_eq!(state.load, LoadKind::AddTrack);
    assert_eq!(state.columns[0].entries, ["MIDI", "Audio", "Return"]);
    assert_eq!(
        state.columns[1].entries.len(),
        6,
        "a MIDI track takes everything"
    );
    b.rig.tap("down");
    assert_eq!(
        b.names(1),
        ["Audio Effects", "Plug-Ins"],
        "an audio track takes audio effects"
    );
    b.rig.tap("lower_8");
    assert_eq!(b.rig.commands(), [r#"track.create {"kind":"audio"}"#]);
}

// Spec: push-2-tracks R12, R1. Load on a device with Add Track inserts the
// track and loads the device in one undo step: both commands share one
// gesture, and the second is its last.
#[test]
fn add_track_with_a_device_is_one_gesture() {
    let mut b = Browsing::new();
    b.rig.tap("add_track");
    b.rig.tap("upper_8");
    for _ in 0..2 {
        b.rig.tap("down");
    }
    b.rig.tap("upper_8");
    b.rig.tap("lower_8");
    let requests = b.rig.requests();
    let commands: Vec<_> = requests
        .iter()
        .filter_map(|r| match &r.ask {
            bl_control::Ask::Command(c) => Some(c),
            _ => None,
        })
        .collect();
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].id, "track.create");
    assert_eq!(commands[0].args, json!({ "kind": "midi" }));
    assert_eq!(commands[1].id, "device.add");
    let (first, last) = (commands[0].gesture.unwrap(), commands[1].gesture.unwrap());
    assert_eq!(first.control, last.control);
    assert!(!first.released && last.released);
    // The new track is selected: the next Load acts on it, and Push 2 stays
    // in Browse Mode.
    assert_eq!(b.state().load, LoadKind::Add);
    assert_eq!(
        b.rig.snapshot().display_mode,
        bl_push::content::DisplayMode::Browse
    );
}

// Spec: push-2-tracks R10. The Preview button switches the browser's
// Preview; with it on, choosing a preset previews it, and choosing anything
// else stops it.
#[test]
fn preview_switches_the_browsers_switch_and_choosing_a_preset_previews_it() {
    let mut b = Browsing::new();
    b.indexed(1);
    b.browse();
    b.rig.tap("lower_7");
    assert_eq!(b.rig.commands(), ["browser.set_preview {}"]);
    assert!(!b.rig.snapshot().screen.lower[6].selected);
    b.set_preview(true);
    assert!(
        b.rig.snapshot().screen.lower[6].selected,
        "lit while it is on"
    );
    // Into Sounds: its first preset is chosen and previews.
    b.rig.tap("upper_8");
    assert_eq!(
        b.rig.commands(),
        [r#"browser.preview {"path":"/p/Sound 0.blpreset"}"#]
    );
    b.rig.tap("upper_7");
    b.rig.tap("down");
    assert_eq!(b.rig.commands(), ["browser.stop_preview {}"]);
}

// Spec: push-2-tracks R13. The lists fill as the catalogue does; Browse Mode
// shows what is there and reads again when it changes.
#[test]
fn the_columns_fill_as_the_index_does() {
    let mut b = Browsing::new();
    b.browse();
    assert!(b.names(1).is_empty(), "nothing is indexed yet");
    b.indexed(3);
    assert_eq!(b.names(1), ["Sound 0", "Sound 1", "Sound 2"]);
    b.indexed(5);
    assert_eq!(b.names(1).len(), 5);
}

// Spec: push-2-tracks R14. `push.state` reads what Load does, the columns
// with their entries and selection, and the column in use.
#[test]
fn the_state_reads_back_what_browse_mode_shows() {
    let mut b = Browsing::new();
    let device = b.select_device(0, DeviceRole::Instrument);
    b.browse();
    assert!(b.rig.sim.turn(0, 20));
    b.rig.advance(1);
    b.rig.tap("upper_8");
    let state = b.state();
    assert_eq!(state.load, LoadKind::Replace(device));
    assert_eq!(state.in_use, 1);
    assert_eq!(state.columns[0].selected, Some(2));
    assert_eq!(state.columns[1].selected, Some(0));
    assert_eq!(state.columns[1].entries[0], "Instrument 01");
}

// Spec: push-2-tracks R25. Add Device lists everything the track can hold,
// instruments included, whatever device is selected; Load adds a device after
// the selected one, and an instrument replaces the track's instrument.
#[test]
fn add_device_adds_after_the_selected_device_and_an_instrument_replaces_the_instrument() {
    let mut b = Browsing::new();
    let instrument = b.select_device(0, DeviceRole::Instrument);
    b.rig.tap("add_device");
    assert_eq!(
        b.rig.snapshot().display_mode,
        bl_push::content::DisplayMode::Browse,
        "Add Device leads to Browse Mode"
    );
    assert_eq!(
        b.names(0),
        [
            "Sounds",
            "Drums",
            "Instruments",
            "Audio Effects",
            "MIDI Effects",
            "Plug-Ins"
        ],
        "with an instrument selected, Browse would list only instruments"
    );
    assert_eq!(b.state().load, LoadKind::Add);
    assert!(b.rig.commands().is_empty());

    // An instrument replaces the track's instrument.
    b.rig.tap("down");
    b.rig.tap("down");
    b.rig.tap("upper_8");
    assert_eq!(b.state().load, LoadKind::Replace(instrument));
    b.rig.tap("lower_8");
    assert_eq!(
        b.rig.commands(),
        [format!(
            r#"device.replace {{"device":{},"new_device":{{"name":"Instrument 01"}}}}"#,
            serde_json::to_string(&instrument).unwrap()
        )]
    );

    // An audio effect is added, after the device selected.
    b.rig.tap("upper_7");
    b.rig.tap("down");
    b.rig.tap("upper_8");
    b.rig.tap("down");
    assert_eq!(b.selected(1), Some(1));
    assert_eq!(b.state().load, LoadKind::Add);
    b.rig.tap("lower_8");
    assert_eq!(
        b.rig.commands(),
        [r#"device.add {"device":{"name":"Reverb"}}"#]
    );
}

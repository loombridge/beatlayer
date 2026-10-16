//! WP-2.19b in the shell: a device's sidechain section and its choosers,
//! and Duplicate Track Without Clips (IMP-12). The set is the one the
//! observations were made in: MIDI tracks 1 and 2 without instruments,
//! audio tracks 3 and 4, returns A-Reverb and B-Delay, and Main. The
//! behaviour is in the routing spec.

use bl_engine::testing;
use bl_model::{Input, Tap, TrackId};
use bl_shell::device_commands::{DeviceAdded, DeviceViewState};
use bl_shell::menus::{Action, MenuPlatform, menu_bar};
use bl_shell::routing::{self, Chooser, InOut};
use bl_shell::track_commands::{self, TracksDuplicated};
use bl_shell::{Area, Shell};
use serde_json::{Value, json};

struct Set {
    shell: Shell,
    audio: [TrackId; 2],
    returns: [TrackId; 2],
}

fn observed_set() -> Set {
    let mut shell = Shell::headless();
    let doc = shell.editor().document();
    let top: Vec<TrackId> = doc.top_level().collect();
    let returns: Vec<TrackId> = doc.returns().collect();
    for (track, name) in [(returns[0], "A Reverb"), (returns[1], "B Delay")] {
        shell
            .invoke("track.rename", json!({ "track": track, "name": name }))
            .unwrap();
    }
    Set {
        shell,
        audio: [top[2], top[3]],
        returns: [returns[0], returns[1]],
    }
}

fn key_device() -> Value {
    let t = testing::key_type();
    json!({ "kind": t.kind, "name": t.name, "role": t.role, "sidechain": t.sidechain })
}

/// Adds the sidechain probe to `track`, as the browser does.
fn add_key(shell: &mut Shell, track: TrackId) -> DeviceAdded {
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    shell.set_focused_area(Area::Detail);
    serde_json::from_value(
        shell
            .invoke("device.add", json!({ "device": key_device() }))
            .unwrap(),
    )
    .unwrap()
}

fn choose(shell: &mut Shell, chooser: &Chooser, label: &str) {
    let run = chooser
        .run_for(label)
        .unwrap_or_else(|| panic!("{label} isn't a live entry of {:?}", chooser.labels()))
        .clone();
    shell.invoke(&run.id, run.args).unwrap();
}

fn last_step(shell: &Shell) -> String {
    shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn state(shell: &Shell) -> DeviceViewState {
    shell.device_view_state()
}

// Spec: routing R29, R30, R31
#[test]
fn the_sidechain_section_lists_its_sources_as_live_does() {
    let Set {
        mut shell, audio, ..
    } = observed_set();
    let added = add_key(&mut shell, audio[1]);
    let device = added.device;
    let doc = shell.editor().document();

    // A new device: External off, No Input, Mix 100 %, SC Gain 0 dB; the
    // section hidden.
    let section = state(&shell).devices[0].sidechain.clone().unwrap();
    assert!(!section.shown && !section.enabled && !section.listen);
    assert_eq!((section.source.as_str(), section.tap), ("No Input", None));
    assert_eq!((section.mix, section.gain), (1.0, 1.0));

    // Source: the audio tracks, the device's own among them, the returns,
    // Main ─ No Input; the channel empty.
    let source = routing::sidechain_source(doc, device).unwrap();
    assert_eq!(
        source.labels(),
        [
            "3-Audio", "4-Audio", "A-Reverb", "B-Delay", "Main", "─", "No Input"
        ]
    );
    assert_eq!(source.text, "No Input");
    let channel = routing::sidechain_channel(doc, device).unwrap();
    assert!(channel.entries.is_empty() && channel.text.is_empty());

    // Choosing a track takes it Post FX, and leaves External off; one undo
    // step, Change "Routing".
    choose(&mut shell, &source, "3-Audio");
    assert_eq!(last_step(&shell), "Change \"Routing\"");
    let doc = shell.editor().document();
    let sidechain = doc.device(device).unwrap().sidechain.clone().unwrap();
    assert_eq!(
        sidechain.input,
        Input::Track {
            track: audio[0],
            tap: Tap::PostFx
        }
    );
    assert!(!sidechain.enabled);
    let channel = routing::sidechain_channel(doc, device).unwrap();
    assert_eq!(channel.labels(), ["Pre FX", "Post FX", "Post Mixer"]);
    assert_eq!(channel.text, "Post FX");
    choose(&mut shell, &channel, "Pre FX");
    let source = routing::sidechain_source(shell.editor().document(), device).unwrap();
    assert_eq!(source.text, "3-Audio");
    choose(&mut shell, &source, "A-Reverb");
    assert_eq!(
        state(&shell).devices[0].sidechain.clone().unwrap().source,
        "A-Reverb"
    );

    // The section's switches and boxes, each an undo step named as Live
    // names it.
    for (id, field, value, step) in [
        (
            "device.set_sidechain_enabled",
            "enabled",
            json!(true),
            "Change \"Sidechain On\"",
        ),
        (
            "device.set_sidechain_mix",
            "mix",
            json!(0.5),
            "Change \"Mix\"",
        ),
        (
            "device.set_sidechain_gain",
            "gain",
            json!(2.0),
            "Change \"SC Gain\"",
        ),
        (
            "device.set_sidechain_listen",
            "listen",
            json!(true),
            "Change \"Sidechain Listen\"",
        ),
    ] {
        shell
            .invoke(id, json!({ "device": device, field: value }))
            .unwrap();
        assert_eq!(last_step(&shell), step);
    }
    let section = state(&shell).devices[0].sidechain.clone().unwrap();
    assert!(section.enabled && section.listen);
    assert_eq!((section.mix, section.gain), (0.5, 2.0));

    // Showing the section changes only the view.
    let steps = shell.editor().undo_history().len();
    shell.invoke("device.show_sidechain", json!({})).unwrap();
    assert!(state(&shell).devices[0].sidechain.clone().unwrap().shown);
    shell
        .run(
            "device.show_sidechain",
            json!({ "device": device, "shown": false }),
        )
        .unwrap();
    assert!(!state(&shell).devices[0].sidechain.clone().unwrap().shown);
    assert_eq!(shell.editor().undo_history().len(), steps);

    // Audio To still offers only Track In for the device's track: tracks
    // aren't routed into sidechains in Live 12.
    shell
        .invoke(
            "routing.set_output",
            json!({ "track": audio[0], "output": { "type": "track", "track": audio[1] } }),
        )
        .unwrap();
    let InOut::Clips { output_channel, .. } =
        routing::in_out(shell.editor().document(), audio[0], &shell.interface()).unwrap()
    else {
        panic!("an audio track");
    };
    assert_eq!(output_channel.labels(), ["Track In"]);
    assert!(!output_channel.enabled());
}

// Spec: routing R25
#[test]
fn deleting_the_source_leaves_no_input() {
    let Set {
        mut shell, audio, ..
    } = observed_set();
    let device = add_key(&mut shell, audio[1]).device;
    let source = routing::sidechain_source(shell.editor().document(), device).unwrap();
    choose(&mut shell, &source, "3-Audio");
    shell
        .invoke(
            "device.set_sidechain_enabled",
            json!({ "device": device, "enabled": true }),
        )
        .unwrap();
    shell
        .run("track.delete", json!({ "track": audio[0] }))
        .unwrap();
    let section = state(&shell).devices[0].sidechain.clone().unwrap();
    assert_eq!(section.source, "No Input");
    assert!(section.enabled, "External stays on");
    shell.invoke("edit.undo", json!({})).unwrap();
    assert_eq!(
        state(&shell).devices[0].sidechain.clone().unwrap().source,
        "3-Audio"
    );
}

// Spec: routing R28 (IMP-12)
#[test]
fn duplicate_track_without_clips_copies_all_but_the_clips() {
    let Set {
        mut shell,
        audio,
        returns,
    } = observed_set();
    let track = audio[1];
    let device = add_key(&mut shell, track).device;
    let source = routing::sidechain_source(shell.editor().document(), device).unwrap();
    choose(&mut shell, &source, "3-Audio");
    let clip: bl_model::ClipId = shell.new_id();
    for (id, args) in [
        ("mixer.set_volume", json!({ "track": track, "volume": 0.5 })),
        ("mixer.set_pan", json!({ "track": track, "pan": -0.25 })),
        (
            "mixer.set_send",
            json!({ "track": track, "to": returns[1], "level": 0.75 }),
        ),
        (
            "routing.set_input",
            json!({ "track": track, "input": { "type": "resampling" } }),
        ),
        (
            "routing.set_monitor",
            json!({ "track": track, "monitor": "in" }),
        ),
        (
            "routing.set_output",
            json!({ "track": track, "output": { "type": "sends_only" } }),
        ),
        (
            "device.set_sidechain_mix",
            json!({ "device": device, "mix": 0.25 }),
        ),
        (
            "clip.create",
            json!({ "track": track, "clip": {
                "id": clip, "name": "", "start": 0, "length": 3840, "mute": false,
                "data": { "audio": { "media": "ab".repeat(32) } },
            } }),
        ),
    ] {
        shell.run(id, args).unwrap_or_else(|e| panic!("{id}: {e}"));
    }
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let steps = shell.editor().undo_history().len();

    // The Edit menu's item, next to Duplicate, runs it on the selection.
    let bar = menu_bar(MenuPlatform::Mac);
    let edit = bar.iter().find(|m| m.title == "Edit").unwrap();
    let at = edit
        .items
        .iter()
        .position(|i| i.label == "Duplicate Track Without Clips")
        .unwrap();
    assert_eq!(edit.items[at - 1].label, "Duplicate");
    let Action::Command(id) = edit.items[at].action else {
        panic!("it runs a command");
    };
    assert_eq!(id, track_commands::DUPLICATE_WITHOUT_CLIPS);
    assert!(shell.can_invoke(id, &json!({})));
    let result: TracksDuplicated =
        serde_json::from_value(shell.invoke(id, json!({})).unwrap()).unwrap();
    let [copy] = result.tracks[..] else {
        panic!("one copy");
    };

    // One undo step, Live's name made distinct; the copy straight after the
    // original, next numbered, and selected.
    assert_eq!(shell.editor().undo_history().len(), steps + 1);
    assert_eq!(last_step(&shell), "Duplicate Track(s) Without Clips");
    let doc = shell.editor().document();
    let top: Vec<TrackId> = doc.top_level().collect();
    assert_eq!(top[top.iter().position(|&t| t == track).unwrap() + 1], copy);
    assert_eq!(shell.arrangement().selected_tracks(), [copy]);
    let (original, copied) = (doc.track(track).unwrap(), doc.track(copy).unwrap());
    assert_eq!(original.name(), "4 Audio");
    assert_eq!(copied.name(), "5 Audio");
    assert_eq!(copied.mixer(), original.mixer());
    assert_eq!(copied.routing(), original.routing());
    assert_eq!(
        copied.sends().collect::<Vec<_>>(),
        original.sends().collect::<Vec<_>>()
    );
    assert_eq!(copied.clips().len(), 0, "no clips");
    assert_eq!(original.clips().len(), 1);
    let (d, c) = (
        original.devices().next().unwrap(),
        copied.devices().next().unwrap(),
    );
    assert_ne!(c.id, d.id, "a new device");
    assert_eq!(
        (&c.name, &c.kind, c.sidechain.clone()),
        (&d.name, &d.kind, d.sidechain.clone())
    );

    // An agent names the tracks; Main can't be duplicated.
    let main = doc.main_id();
    assert!(shell.run(id, json!({ "tracks": [main] })).is_err());
    shell.run(id, json!({ "tracks": [returns[0]] })).unwrap();
    let names: Vec<String> = shell
        .editor()
        .document()
        .returns()
        .map(|r| {
            shell
                .editor()
                .document()
                .track(r)
                .unwrap()
                .name()
                .to_owned()
        })
        .collect();
    assert_eq!(names, ["A Reverb", "B Reverb", "B Delay"]);

    // Undo takes the copy away.
    shell.invoke("edit.undo", json!({})).unwrap();
    shell.invoke("edit.undo", json!({})).unwrap();
    assert!(shell.editor().document().track(copy).is_none());
}

//! The In/Out section's choosers (WP-2.19): each lists what Live 12.4.6's
//! lists, in the same order, with Live's defaults chosen, and each entry
//! runs a registered routing command. The set is the one the observations
//! were made in: MIDI tracks 1 and 2 without instruments, audio tracks 3
//! and 4, returns A-Reverb and B-Delay, and Main. The behaviour is in the
//! routing spec.

use bl_model::{Channel, Input, Monitor, Output, Tap, TrackId, TrackKind};
use bl_shell::Shell;
use bl_shell::routing::{self, Chooser, InOut, return_letter, track_label};
use serde_json::json;

struct Set {
    shell: Shell,
    midi: [TrackId; 2],
    audio: [TrackId; 2],
    returns: [TrackId; 2],
}

/// A new set is the observed one: Live's default set, whose returns are
/// renamed as they were in the session.
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
        midi: [top[0], top[1]],
        audio: [top[2], top[3]],
        returns: [returns[0], returns[1]],
    }
}

fn section(shell: &Shell, track: TrackId) -> InOut {
    routing::in_out(shell.editor().document(), track, &shell.interface()).unwrap()
}

/// The In/Out section with an interface whose channels `interface` names,
/// for the choosers that list the device's channels.
fn section_with(shell: &Shell, track: TrackId, interface: &routing::Interface) -> InOut {
    routing::in_out(shell.editor().document(), track, interface).unwrap()
}

fn clips(shell: &Shell, track: TrackId) -> (Chooser, Chooser, Monitor, Chooser, Chooser) {
    match section(shell, track) {
        InOut::Clips {
            input,
            input_channel,
            monitor,
            output,
            output_channel,
        } => (input, input_channel, monitor.chosen, output, output_channel),
        other => panic!("not a clip track's section: {other:?}"),
    }
}

fn output_only(shell: &Shell, track: TrackId) -> (Chooser, Chooser) {
    match section(shell, track) {
        InOut::Output {
            output,
            output_channel,
        } => (output, output_channel),
        other => panic!("not a group's or a return's section: {other:?}"),
    }
}

/// Chooses the entry labelled `label`, as a click on it does.
fn choose(shell: &mut Shell, chooser: &Chooser, label: &str) {
    let run = chooser
        .run_for(label)
        .unwrap_or_else(|| panic!("{label} isn't a live entry of {:?}", chooser.labels()))
        .clone();
    shell.invoke(&run.id, run.args).unwrap();
}

// Spec: routing R16, R21
#[test]
fn every_chooser_lists_what_lives_lists_in_the_same_order() {
    let Set {
        mut shell,
        midi,
        audio,
        returns,
    } = observed_set();
    let doc = shell.editor().document();
    assert_eq!(track_label(doc, audio[1]), "4-Audio");
    assert_eq!(track_label(doc, returns[0]), "A-Reverb");
    assert_eq!(track_label(doc, doc.main_id()), "Main");

    // An audio track: Ext. In, Monitor Auto, Main.
    let (input, channel, monitor, output, out_channel) = clips(&shell, audio[0]);
    assert_eq!(
        input.labels(),
        [
            "Ext. In",
            "Configure…",
            "─",
            "Resampling",
            "─",
            "4-Audio",
            "A-Reverb",
            "B-Delay",
            "Main",
            "─",
            "No Input"
        ]
    );
    assert_eq!(input.text, "Ext. In");
    assert_eq!(channel.pending, None, "the channel list follows the device");
    assert_eq!(channel.text, "1/2", "the first pair, with no device open");
    assert_eq!(channel.labels(), Vec::<&str>::new());
    assert_eq!(monitor, Monitor::Auto);
    assert_eq!(
        output.labels(),
        [
            "Ext. Out",
            "Configure…",
            "─",
            "Main",
            "4-Audio",
            "─",
            "Sends Only"
        ]
    );
    assert_eq!(output.text, "Main");
    assert_eq!(out_channel.text, "");

    // Its input from track 4: Pre FX, Post FX, Post Mixer, Post Mixer chosen.
    choose(&mut shell, &input, "4-Audio");
    let (input, channel, ..) = clips(&shell, audio[0]);
    assert_eq!(input.text, "4-Audio");
    assert_eq!(channel.labels(), ["Pre FX", "Post FX", "Post Mixer"]);
    assert_eq!(channel.text, "Post Mixer");
    choose(&mut shell, &channel, "Post FX");
    assert_eq!(
        shell
            .editor()
            .document()
            .track(audio[0])
            .unwrap()
            .routing()
            .input,
        Input::Track {
            track: audio[1],
            tap: Tap::PostFx,
            chain: None,
        }
    );
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Change \"Routing\"")
    );

    // A MIDI track: All Ins, and No Output.
    let (input, channel, monitor, output, _) = clips(&shell, midi[0]);
    assert_eq!(
        input.labels(),
        [
            "All Ins",
            "Computer Keyboard",
            "Configure…",
            "─",
            "2-MIDI",
            "─",
            "No Input"
        ]
    );
    assert_eq!(input.text, "All Ins");
    assert!(input.run_for("All Ins").is_some());
    assert_eq!(channel.text, "All Channels");
    assert_eq!(channel.pending, None);
    assert_eq!(monitor, Monitor::Auto);
    assert_eq!(
        output.labels(),
        ["Configure…", "─", "2-MIDI", "─", "No Output"]
    );
    assert_eq!(output.text, "No Output");
    choose(&mut shell, &output, "2-MIDI");
    let (.., output, out_channel) = clips(&shell, midi[0]);
    assert_eq!(output.text, "2-MIDI");
    assert_eq!(out_channel.text, "Track In");
    choose(&mut shell, &input, "2-MIDI");
    let (_, channel, monitor, ..) = clips(&shell, midi[0]);
    assert_eq!(channel.labels(), ["Pre FX", "Post FX", "Post Mixer"]);
    assert_eq!(channel.text, "Post Mixer");
    assert_eq!(monitor, Monitor::Auto);

    // A return: Main, the audio tracks, Sends Only; no other return.
    let (output, _) = output_only(&shell, returns[0]);
    assert_eq!(
        output.labels(),
        [
            "Ext. Out",
            "Configure…",
            "─",
            "Main",
            "3-Audio",
            "4-Audio",
            "─",
            "Sends Only"
        ]
    );
    assert_eq!(output.text, "Main");
    choose(&mut shell, &output, "3-Audio");
    let (_, channel) = output_only(&shell, returns[0]);
    assert_eq!(channel.labels(), ["Track In"]);

    // Main: Cue Out and Main Out list the interface's outputs, which
    // arrive with the audio interface (WP-2.13); with none open, only
    // Configure…, as Live shows with no audio device (observed).
    match section(&shell, shell.editor().document().main_id()) {
        InOut::Main { cue, main } => {
            assert_eq!(cue.labels(), ["Configure…", "─"]);
            assert_eq!(cue.text, "1/2");
            assert_eq!(main.labels(), ["Configure…", "─"]);
            assert_eq!(main.text, "1/2");
        }
        other => panic!("{other:?}"),
    }
}

/// The Ext. In channel and Main Out choosers list the open interface's
/// channels, and the agent API sets both, as the routing spec's commands do
/// (WP-2.13).
#[test]
fn the_interface_choosers_list_its_channels_and_the_agent_api_sets_them() {
    let Set {
        mut shell, audio, ..
    } = observed_set();
    let interface = routing::Interface {
        inputs: 4,
        outputs: 4,
        ..routing::Interface::default()
    };

    // Ext. In's channel: the single channels, then the pairs.
    let channel_of = |shell: &Shell| match section_with(shell, audio[0], &interface) {
        InOut::Clips { input_channel, .. } => input_channel,
        other => panic!("{other:?}"),
    };
    let channel = channel_of(&shell);
    assert_eq!(channel.labels(), ["1", "2", "3", "4", "1/2", "3/4"]);
    assert_eq!(channel.text, "1/2");
    shell
        .run(
            "routing.set_input",
            json!({ "track": audio[0], "input": { "type": "external", "channel": { "type": "mono", "channel": 3 } } }),
        )
        .unwrap();
    assert_eq!(
        shell
            .editor()
            .document()
            .track(audio[0])
            .unwrap()
            .routing()
            .input,
        Input::External {
            channel: Channel::Mono(3)
        }
    );
    assert_eq!(channel_of(&shell).text, "3");
    let channel = channel_of(&shell);
    choose(&mut shell, &channel, "4");
    assert_eq!(
        shell
            .editor()
            .document()
            .track(audio[0])
            .unwrap()
            .routing()
            .input,
        Input::External {
            channel: Channel::Mono(4)
        }
    );
    let channel = channel_of(&shell);
    choose(&mut shell, &channel, "3/4");
    assert_eq!(
        shell
            .editor()
            .document()
            .track(audio[0])
            .unwrap()
            .routing()
            .input,
        Input::External {
            channel: Channel::Pair(3)
        }
    );

    // Main Out and Cue Out: the interface's output pairs, set by command.
    let main_id = shell.editor().document().main_id();
    let (cue, main) = match section_with(&shell, main_id, &interface) {
        InOut::Main { cue, main } => (cue, main),
        other => panic!("{other:?}"),
    };
    assert_eq!(cue.labels(), ["Configure…", "─", "1/2", "3/4"]);
    assert_eq!(main.labels(), ["Configure…", "─", "1/2", "3/4"]);
    assert_eq!(main.text, "1/2");
    shell
        .run(
            "routing.set_main_out",
            json!({ "channel": { "type": "pair", "channel": 3 } }),
        )
        .unwrap();
    shell
        .run(
            "routing.set_cue_out",
            json!({ "channel": { "type": "pair", "channel": 2 } }),
        )
        .unwrap();
    let routing = shell
        .editor()
        .document()
        .track(main_id)
        .unwrap()
        .routing()
        .clone();
    assert_eq!(routing.main_out, Channel::Pair(3));
    assert_eq!(routing.cue_out, Channel::Pair(2));
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Change \"Routing\"")
    );
    let (cue, main) = match section_with(&shell, main_id, &interface) {
        InOut::Main { cue, main } => (cue, main),
        other => panic!("{other:?}"),
    };
    assert_eq!(main.text, "3/4");
    assert_eq!(cue.text, "2/3");
    // A mono channel is refused for Main Out.
    assert!(
        shell
            .run(
                "routing.set_main_out",
                json!({ "channel": { "type": "mono", "channel": 1 } }),
            )
            .is_err()
    );
}

/// Ext. Out is a live choice of every Audio To that lists it, with a channel
/// chooser of the interface's outputs, and the agent API sets it
/// (WP-2.13).
// Spec: routing R3, R16, R20
#[test]
fn ext_out_takes_the_interfaces_outputs_as_its_channel() {
    let Set {
        mut shell,
        audio,
        returns,
        ..
    } = observed_set();
    let interface = routing::Interface {
        outputs: 4,
        ..routing::Interface::default()
    };
    let to = |shell: &Shell, track: TrackId, interface: &routing::Interface| match section_with(
        shell, track, interface,
    ) {
        InOut::Clips {
            output,
            output_channel,
            ..
        }
        | InOut::Output {
            output,
            output_channel,
        } => (output, output_channel),
        other => panic!("{other:?}"),
    };

    // Chosen, it takes the first pair; one undo step.
    let (output, _) = to(&shell, audio[0], &interface);
    choose(&mut shell, &output, "Ext. Out");
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Change \"Routing\"")
    );
    let (output, channel) = to(&shell, audio[0], &interface);
    assert_eq!(output.text, "Ext. Out");
    assert_eq!(channel.labels(), ["1", "2", "3", "4", "1/2", "3/4"]);
    assert_eq!(channel.text, "1/2");

    // Dimmed with no device open.
    let (_, channel) = to(&shell, audio[0], &routing::Interface::default());
    assert!(!channel.enabled());
    assert_eq!(channel.text, "1/2");

    // Another channel; choosing Ext. Out again keeps it.
    let (_, channel) = to(&shell, audio[0], &interface);
    choose(&mut shell, &channel, "3");
    let (output, channel) = to(&shell, audio[0], &interface);
    assert_eq!(channel.text, "3");
    choose(&mut shell, &output, "Ext. Out");
    let routing = shell
        .editor()
        .document()
        .track(audio[0])
        .unwrap()
        .routing()
        .clone();
    assert_eq!(
        routing.output,
        Output::External {
            channel: Channel::Mono(3)
        }
    );

    // A return takes it by command, with its channel.
    shell
        .run(
            "routing.set_output",
            json!({ "track": returns[0], "output": { "type": "external", "channel": { "type": "pair", "channel": 3 } } }),
        )
        .unwrap();
    let (output, channel) = to(&shell, returns[0], &interface);
    assert_eq!(
        (output.text.as_str(), channel.text.as_str()),
        ("Ext. Out", "3/4")
    );

    // A set opened on a device with fewer outputs keeps the choice, drawn
    // dimmed (unobserved).
    let two = routing::Interface {
        outputs: 2,
        ..routing::Interface::default()
    };
    let (_, channel) = to(&shell, returns[0], &two);
    assert_eq!((channel.text.as_str(), channel.enabled()), ("3/4", false));
    shell
        .run(
            "routing.set_output",
            json!({ "track": returns[0], "output": { "type": "external", "channel": { "type": "pair", "channel": 65535 } } }),
        )
        .unwrap();
    assert!(!to(&shell, returns[0], &two).1.enabled(), "no overflow");
}

// Spec: routing R16, R6
#[test]
fn grouped_tracks_list_their_group_and_a_group_lists_its_tracks() {
    let Set {
        mut shell, audio, ..
    } = observed_set();
    // Group tracks 3 and 4, as Cmd-G does: the group goes where track 3
    // was.
    shell
        .invoke(
            "track.create",
            json!({ "kind": "group", "name": "3 Group", "index": 2 }),
        )
        .unwrap();
    let group = shell.editor().document().top_level().nth(2).unwrap();
    for (i, track) in audio.into_iter().enumerate() {
        shell
            .invoke(
                "track.move",
                json!({ "track": track, "parent": group, "index": i }),
            )
            .unwrap();
    }
    let doc = shell.editor().document();
    assert_eq!(track_label(doc, group), "3-Group");
    assert_eq!(track_label(doc, audio[1]), "5-Audio");

    let (_, _, _, output, _) = clips(&shell, audio[0]);
    assert_eq!(
        output.labels(),
        [
            "Ext. Out",
            "Configure…",
            "─",
            "Main",
            "3-Group",
            "5-Audio",
            "─",
            "Sends Only"
        ]
    );
    assert_eq!(
        output.text, "3-Group",
        "grouping sends a track to its group"
    );
    let (group_output, _) = output_only(&shell, group);
    assert_eq!(
        group_output.labels(),
        [
            "Ext. Out",
            "Configure…",
            "─",
            "Main",
            "─",
            "4-Audio",
            "5-Audio",
            "─",
            "Sends Only"
        ]
    );
    assert_eq!(group_output.text, "Main");
    let (input, ..) = clips(&shell, audio[1]);
    assert_eq!(
        input.labels(),
        [
            "Ext. In",
            "Configure…",
            "─",
            "Resampling",
            "─",
            "3-Group",
            "4-Audio",
            "A-Reverb",
            "B-Delay",
            "Main",
            "─",
            "No Input"
        ]
    );
    // Main from inside the group, then back to the group.
    choose(&mut shell, &output, "Main");
    assert_eq!(
        shell
            .editor()
            .document()
            .track(audio[0])
            .unwrap()
            .routing()
            .output,
        Output::Main
    );
    let (.., output, _) = clips(&shell, audio[0]);
    assert_eq!(output.text, "Main");
    choose(&mut shell, &output, "3-Group");
    assert_eq!(
        shell
            .editor()
            .document()
            .track(audio[0])
            .unwrap()
            .routing()
            .output,
        Output::Default
    );
}

// Spec: routing R7
#[test]
fn deleting_the_other_end_leaves_sends_only_and_no_input() {
    let Set {
        mut shell,
        audio,
        returns,
        ..
    } = observed_set();
    let (output, _) = output_only(&shell, returns[0]);
    choose(&mut shell, &output, "3-Audio");
    let (input, ..) = clips(&shell, audio[1]);
    choose(&mut shell, &input, "3-Audio");
    shell
        .invoke("track.delete", json!({ "track": audio[0] }))
        .unwrap();
    let (output, _) = output_only(&shell, returns[0]);
    assert_eq!(output.text, "Sends Only");
    let (input, ..) = clips(&shell, audio[1]);
    assert_eq!(input.text, "No Input");
    shell.invoke("edit.undo", json!({})).unwrap();
    let (output, _) = output_only(&shell, returns[0]);
    assert_eq!(output.text, "3-Audio", "undo restores the routing");
}

// Spec: routing R9 (IMP-01)
#[test]
fn returns_past_z_take_two_letters() {
    assert_eq!(return_letter(0), "A");
    assert_eq!(return_letter(25), "Z");
    assert_eq!(return_letter(26), "AA");
    assert_eq!(return_letter(27), "AB");
    assert_eq!(return_letter(51), "AZ");
    assert_eq!(return_letter(52), "BA");
    let mut shell = Shell::headless();
    for _ in 0..28 {
        shell
            .invoke("track.create", json!({ "kind": "return" }))
            .unwrap();
    }
    let doc = shell.editor().document();
    let last = doc.returns().last().unwrap();
    assert_eq!(doc.track(last).unwrap().name(), "AD Return");
    assert_eq!(track_label(doc, last), "AD-Return");
    assert!(
        doc.tracks()
            .filter(|t| t.kind() == TrackKind::Return)
            .count()
            == 30
    );
}

/// Adds a rack named `name` of `kind` to the selected track, or into
/// `chain`, with chains of these names; returns the rack and its chains.
fn add_rack(
    shell: &mut Shell,
    chain: Option<bl_model::ChainId>,
    kind: &str,
    role: &str,
    name: &str,
    chains: &[&str],
) -> (bl_model::DeviceId, Vec<bl_model::ChainId>) {
    let device = json!({ "kind": { "rack": { "kind": kind } }, "name": name, "role": role });
    let args = match chain {
        Some(chain) => json!({ "device": device, "chain": chain }),
        None => json!({ "device": device }),
    };
    let rack: bl_model::DeviceId =
        serde_json::from_value(shell.invoke("device.add", args).unwrap()["device"].clone())
            .unwrap();
    shell
        .invoke("device.rename", json!({ "device": rack, "name": name }))
        .unwrap();
    let ids = chains
        .iter()
        .enumerate()
        .map(|(n, chain_name)| {
            let id: bl_model::ChainId = shell.new_id();
            shell
                .invoke(
                    "chain.add",
                    json!({ "rack": rack, "index": n,
                            "chain": { "id": id, "name": chain_name, "devices": [] } }),
                )
                .unwrap();
            id
        })
        .collect();
    (rack, ids)
}

fn select(shell: &mut Shell, track: TrackId) {
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
}

// Spec: racks R38, R39
#[test]
fn a_track_with_racks_offers_three_taps_per_chain() {
    let Set {
        mut shell,
        midi,
        audio,
        ..
    } = observed_set();
    // Track 4 holds an audio effect rack of two chains.
    select(&mut shell, audio[1]);
    let (_, chains) = add_rack(
        &mut shell,
        None,
        "audio_effect",
        "audio_effect",
        "Verb",
        &["Wet", "Dry"],
    );
    // A MIDI track is listed once it holds an instrument, with the taps of
    // its instrument rack's chains.
    let (input, ..) = clips(&shell, audio[0]);
    assert!(!input.labels().contains(&"1-MIDI"), "no instrument yet");
    select(&mut shell, midi[0]);
    add_rack(
        &mut shell,
        None,
        "instrument",
        "instrument",
        "Keys",
        &["Pad"],
    );
    let (input, ..) = clips(&shell, audio[0]);
    assert_eq!(
        input.labels(),
        [
            "Ext. In",
            "Configure…",
            "─",
            "Resampling",
            "─",
            "1-MIDI",
            "4-Audio",
            "A-Reverb",
            "B-Delay",
            "Main",
            "─",
            "No Input"
        ]
    );

    // Choosing the track taps its Post Mixer; its channel chooser then
    // lists its own taps and a rack | chain | tap for each of the chains.
    choose(&mut shell, &input, "4-Audio");
    let (_, channel, ..) = clips(&shell, audio[0]);
    assert_eq!(
        channel.labels(),
        [
            "Pre FX",
            "Post FX",
            "Post Mixer",
            "Verb | Wet | Pre FX",
            "Verb | Wet | Post FX",
            "Verb | Wet | Post Mixer",
            "Verb | Dry | Pre FX",
            "Verb | Dry | Post FX",
            "Verb | Dry | Post Mixer",
        ]
    );
    assert_eq!(channel.text, "Post Mixer");
    choose(&mut shell, &channel, "Verb | Dry | Post FX");
    assert_eq!(
        shell
            .editor()
            .document()
            .track(audio[0])
            .unwrap()
            .routing()
            .input,
        Input::Track {
            track: audio[1],
            tap: Tap::PostFx,
            chain: Some(chains[1]),
        }
    );
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Change \"Routing\"")
    );
    let (input, channel, ..) = clips(&shell, audio[0]);
    assert_eq!(input.text, "4-Audio");
    assert_eq!(channel.text, "Verb | Dry | Post FX");
    // Back to the track's own tap.
    choose(&mut shell, &channel, "Post FX");
    let (_, channel, ..) = clips(&shell, audio[0]);
    assert_eq!(channel.text, "Post FX");

    // A MIDI track's instrument rack is tapped the same way.
    choose(&mut shell, &input, "1-MIDI");
    let (_, channel, ..) = clips(&shell, audio[0]);
    assert_eq!(channel.labels()[3], "Keys | Pad | Pre FX");
    // MIDI From offers the tracks only.
    let (input, ..) = clips(&shell, midi[1]);
    choose(&mut shell, &input, "1-MIDI");
    let (_, channel, ..) = clips(&shell, midi[1]);
    assert_eq!(channel.labels(), ["Pre FX", "Post FX", "Post Mixer"]);
}

// Spec: racks R39
#[test]
fn chain_taps_read_rack_chain_and_tap_in_signal_order() {
    let Set {
        mut shell, audio, ..
    } = observed_set();
    select(&mut shell, audio[1]);
    // Outer's first chain holds Inner; a drum rack's chains aren't offered.
    let (_, outer) = add_rack(
        &mut shell,
        None,
        "audio_effect",
        "audio_effect",
        "Outer",
        &["A", "B"],
    );
    add_rack(
        &mut shell,
        Some(outer[0]),
        "audio_effect",
        "audio_effect",
        "Inner",
        &["I1", "I2"],
    );
    let (input, ..) = clips(&shell, audio[0]);
    choose(&mut shell, &input, "4-Audio");
    let (_, channel, ..) = clips(&shell, audio[0]);
    let racks_and_chains: Vec<&str> = channel
        .labels()
        .into_iter()
        .skip(3)
        .filter_map(|label| label.strip_suffix(" | Pre FX"))
        .collect();
    assert_eq!(
        racks_and_chains,
        ["Outer | A", "Inner | I1", "Inner | I2", "Outer | B"],
        "a nested rack straight after the chain that holds it"
    );
    assert_eq!(channel.labels().len(), 3 + 4 * 3);
}

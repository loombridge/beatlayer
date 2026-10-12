//! The In/Out section's choosers (WP-2.19): each lists what Live 12.4.6's
//! lists, in the same order, with Live's defaults chosen, and each entry
//! runs a registered routing command. The set is the one the observations
//! were made in: MIDI tracks 1 and 2 without instruments, audio tracks 3
//! and 4, returns A-Reverb and B-Delay, and Main. The behaviour is in the
//! routing spec.

use bl_model::{Input, Monitor, Output, Tap, TrackId, TrackKind};
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
    routing::in_out(shell.editor().document(), track).unwrap()
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

// Spec: routing R16
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
    assert_eq!(channel.pending, Some(routing::DUPLEX_AUDIO));
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
            tap: Tap::PostFx
        }
    );
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Change \"Routing\"")
    );

    // A MIDI track: All Ins (disabled until MIDI input), and No Output.
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
    assert!(input.run_for("All Ins").is_none(), "MIDI input is WP-2.2's");
    assert_eq!(channel.pending, Some(routing::MIDI_IO));
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

    // Main: Cue Out and Main Out wait for the audio interface's outputs.
    match section(&shell, shell.editor().document().main_id()) {
        InOut::Main { cue, main } => {
            assert_eq!(cue.labels(), ["Configure…"]);
            assert_eq!(main.pending, Some(routing::DUPLEX_AUDIO));
        }
        other => panic!("{other:?}"),
    }
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

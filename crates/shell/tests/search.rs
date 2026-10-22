//! Command search and track search in the shell (WP-6.42, IMP-16 and
//! IMP-18; the command palette spec, R1 to R16): the rows the Commands
//! scope lists and how a text ranks them, the tracks the Tracks scope finds,
//! what Return does, Show Only Matches, and the commands an agent uses. The
//! field's drawing is `bl-ui`'s tests.

use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_model::{
    Chain, ChainId, Command, Device, DeviceId, DeviceKind, PluginFormat, RackKind, TrackId,
};
use bl_shell::menus::{Action, MenuPlatform, menu_bar};
use bl_shell::search_commands::{CommandRows, SearchScope, SearchState, TrackRows};
use bl_shell::{Shell, browser_commands, export_commands};
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();

fn tracks(shell: &Shell) -> Vec<TrackId> {
    shell.editor().document().top_level().collect()
}

fn state(shell: &mut Shell) -> SearchState {
    serde_json::from_value(shell.run("search.state", json!({})).unwrap()).unwrap()
}

fn open(shell: &mut Shell, scope: &str) {
    shell.run("search.open", json!({ "scope": scope })).unwrap();
}

fn type_text(shell: &mut Shell, text: &str) -> SearchState {
    serde_json::from_value(
        shell
            .run("search.set_text", json!({ "text": text }))
            .unwrap(),
    )
    .unwrap()
}

fn rows(shell: &mut Shell, text: &str) -> CommandRows {
    serde_json::from_value(
        shell
            .run("search.commands", json!({ "text": text }))
            .unwrap(),
    )
    .unwrap()
}

fn found(shell: &mut Shell, text: &str) -> TrackRows {
    serde_json::from_value(shell.run("search.tracks", json!({ "text": text })).unwrap()).unwrap()
}

fn rename(shell: &mut Shell, track: TrackId, name: &str) {
    shell
        .run("track.rename", json!({ "track": track, "name": name }))
        .unwrap();
}

/// A plug-in device named `name`, inserted at the end of a track's chain.
fn plug_in(shell: &mut Shell, track: TrackId, name: &str) -> DeviceId {
    shell.edit(|editor| {
        let id: DeviceId = editor.new_id();
        let device = Device::new(
            id,
            name,
            DeviceKind::Plugin {
                format: PluginFormat::Clap,
                plugin_id: format!("beatlayer.tests.{name}"),
            },
        );
        let index = editor.document().track(track).unwrap().devices().len();
        editor
            .apply(Command::InsertDevice {
                track,
                index,
                device,
            })
            .unwrap();
        id
    })
}

/// An audio effect rack on a track holding a device named `inside` in its
/// chain, which is then renamed `renamed`.
fn nested_device(shell: &mut Shell, track: TrackId, inside: &str, renamed: &str) {
    let rack = plug_in_rack(shell, track);
    shell.edit(|editor| {
        let chain: ChainId = editor.new_id();
        editor
            .apply(Command::AddChain {
                rack,
                index: 0,
                chain: Chain::new(chain, "Chain"),
            })
            .unwrap();
        let id: DeviceId = editor.new_id();
        let device = Device::new(
            id,
            inside,
            DeviceKind::Plugin {
                format: PluginFormat::Clap,
                plugin_id: "beatlayer.tests.inside".to_owned(),
            },
        );
        editor
            .apply(Command::InsertChainDevice {
                chain,
                index: 0,
                device,
            })
            .unwrap();
        editor
            .apply(Command::RenameDevice {
                device: id,
                name: renamed.to_owned(),
            })
            .unwrap();
    });
}

fn plug_in_rack(shell: &mut Shell, track: TrackId) -> DeviceId {
    shell.edit(|editor| {
        let id: DeviceId = editor.new_id();
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device: Device::rack(id, RackKind::AudioEffect),
            })
            .unwrap();
        id
    })
}

/// The set with its first group holding its first two tracks, folded.
fn grouped(shell: &mut Shell) -> (TrackId, Vec<TrackId>) {
    shell
        .invoke("track.create", json!({ "kind": "group" }))
        .unwrap();
    let group = *tracks(shell).last().unwrap();
    let inside = tracks(shell)[..2].to_vec();
    for track in &inside {
        shell
            .run(
                "track.move",
                json!({ "track": track, "parent": group, "index": 0 }),
            )
            .unwrap();
    }
    shell
        .run("arrangement.fold_tracks", json!({ "tracks": [group] }))
        .unwrap();
    (group, inside)
}

/// A 16-bit mono WAV of one second at 48 kHz: a quiet ramp.
fn wav(path: &Path) {
    let frames = 48_000u32;
    let data = frames * 2;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let sample = ((i % 100) as i16 - 50) * 20;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// A shell holding a one-second file as a clip at bar 5.
fn shell_with_a_clip(folder: &Path) -> Shell {
    let mut shell = Shell::headless();
    let file = folder.join("Tone.wav");
    wav(&file);
    shell
        .run(
            browser_commands::ADD_TO_ARRANGEMENT,
            json!({ "paths": [file.to_string_lossy()], "at": 4 * BAR }),
        )
        .unwrap();
    shell
}

/// A folder of its own under the system's temporary folder.
fn scratch(name: &str) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("bl-search-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    folder
}

fn selected(shell: &Shell) -> Vec<TrackId> {
    shell.arrangement().selected_tracks().to_vec()
}

// Spec: command-palette R4. Every menu item that runs a command, with the
// arguments it fixes, is one row, and so is every shortcut a key runs.
#[test]
fn every_menu_command_and_every_bound_shortcut_is_one_row() {
    let shell = Shell::headless();
    let all = shell.search_all_commands();
    let count = |id: &str, args: &Value| {
        all.iter()
            .filter(|row| row.command == id && &Value::Object(row.args.clone()) == args)
            .count()
    };
    let mut checked = 0;
    for menu in menu_bar(MenuPlatform::current()) {
        for item in menu.walk().filter(|item| item.is_shown()) {
            match item.action {
                Action::Command(id) => {
                    assert_eq!(count(id, &json!({})), 1, "{}: {id}", item.label);
                    checked += 1;
                }
                Action::Choice(id, args) => {
                    let args: Value = serde_json::from_str(args).unwrap();
                    assert_eq!(count(id, &args), 1, "{}: {id}", item.label);
                    checked += 1;
                }
                _ => {}
            }
        }
    }
    assert!(checked > 40, "the menus' commands were checked");
    let contexts = shell.active_contexts();
    for entry in shell.keymap().entries() {
        if entry.status != bl_shell::keymap::Status::Bound
            || entry.kind != "key"
            || entry.trigger == bl_shell::keymap::Trigger::Held
            || entry
                .bindings(bl_shell::keymap::Platform::current())
                .is_empty()
        {
            continue;
        }
        let command = contexts
            .iter()
            .find_map(|c| entry.commands.iter().find(|k| k.contexts.contains(c)))
            .or(entry.commands.first())
            .unwrap();
        assert_eq!(
            count(&command.id, &Value::Object(command.args.clone())),
            1,
            "{}: {}",
            entry.id,
            command.id
        );
    }
}

// Spec: command-palette R4. A command that needs a value typed or dragged
// has no row, and the Insert tracks are rows of one command.
#[test]
fn the_rows_fix_their_arguments_and_leave_out_what_needs_a_value() {
    let shell = Shell::headless();
    let all = shell.search_all_commands();
    let insert: Vec<_> = all
        .iter()
        .filter(|row| row.command == "track.create")
        .map(|row| (row.label.as_str(), row.args.get("kind").cloned()))
        .collect();
    assert!(insert.contains(&("Insert Audio Track", Some(json!("audio")))));
    assert!(insert.contains(&("Insert MIDI Track", Some(json!("midi")))));
    for command in [
        "mixer.set_volume",
        "arrangement.set_grid",
        "chain.set_colour",
    ] {
        assert!(all.iter().all(|row| row.command != command), "{command}");
    }
    assert!(
        all.iter()
            .all(|row| !row.label.is_empty() && !row.menu.is_empty()),
        "every row has its label and its menu"
    );
}

// Spec: command-palette R6, R7. "dup" ranks Duplicate first and finds
// Duplicate Track Without Clips; rows that can't run come after those that
// can.
#[test]
fn a_few_letters_find_and_rank_commands() {
    let folder = scratch("rank");
    let mut shell = shell_with_a_clip(&folder);
    let clip = shell
        .editor()
        .document()
        .tracks()
        .find_map(|track| track.clips().next().map(|clip| clip.id))
        .unwrap();
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    let dup = rows(&mut shell, "dup").rows;
    assert_eq!(dup[0].label, "Duplicate");
    assert!(dup[0].available);
    assert!(
        dup.iter()
            .any(|row| row.label == "Duplicate Track Without Clips" && row.menu == "Edit"),
        "{:?}",
        dup.iter().map(|r| &r.label).collect::<Vec<_>>()
    );
    let _ = std::fs::remove_dir_all(&folder);
    // Every word must match: a word starting a word of the label or the
    // menu, or anywhere in the description.
    let two = rows(&mut shell, "insert midi").rows;
    assert_eq!(two[0].label, "Insert MIDI Track");
    let audio = two.iter().position(|row| row.label == "Insert Audio Track");
    assert!(
        audio.is_none_or(|at| at > 0),
        "matched through the description, after"
    );
    let by_menu = rows(&mut shell, "mixer volume").rows;
    assert!(!by_menu.is_empty(), "words match the menu and the label");
    let by_description = rows(&mut shell, "xylophone").rows;
    assert!(by_description.is_empty());
    assert!(
        rows(&mut shell, "").rows.is_empty(),
        "nothing typed lists nothing"
    );
    // Rows that can't run come last.
    let undo = rows(&mut shell, "u").rows;
    let first_unavailable = undo.iter().position(|row| !row.available);
    if let Some(at) = first_unavailable {
        assert!(undo[at..].iter().all(|row| !row.available));
    }
}

// Spec: command-palette R5. A row shows the keys the keymap binds to it
// now, a person's own included.
#[test]
fn a_row_shows_the_keymap_s_keys() {
    let mut shell = Shell::headless();
    let before = rows(&mut shell, "duplicate track without").rows;
    assert!(before[0].keys.is_empty());
    let (platform, keys) = if cfg!(target_os = "macos") {
        ("mac", "Cmd+Option+D")
    } else {
        ("win", "Ctrl+Alt+D")
    };
    shell
        .run(
            "keymap.bind",
            json!({
                "command": "track.duplicate_without_clips",
                "keys": [keys],
                "platform": platform,
                "context": "global",
            }),
        )
        .unwrap();
    let after = rows(&mut shell, "duplicate track without").rows;
    assert_eq!(after[0].keys, [keys]);
    // Cmd-Shift-P opens the Commands scope, Cmd-Shift-O the Tracks scope.
    let search = rows(&mut shell, "search commands").rows;
    let expected = if cfg!(target_os = "macos") {
        "Cmd+Shift+P"
    } else {
        "Ctrl+Shift+P"
    };
    assert_eq!(search[0].keys, [expected]);
    assert_eq!(search[0].menu, "Help");
    let track = rows(&mut shell, "find track").rows;
    let expected = if cfg!(target_os = "macos") {
        "Cmd+Shift+O"
    } else {
        "Ctrl+Shift+O"
    };
    assert_eq!(track[0].keys, [expected]);
    assert_eq!(track[0].menu, "Navigate");
}

// Spec: command-palette R1, R2, R3, R15. Opening, typing, switching scope
// and closing change nothing in the set and are no undo step.
#[test]
fn the_field_opens_types_switches_and_closes_without_touching_the_set() {
    let mut shell = Shell::headless();
    let document = shell.editor().document().clone();
    let history = shell.editor().undo_history().len();
    let dirty = shell.is_dirty();
    assert!(!state(&mut shell).open);
    open(&mut shell, "commands");
    let opened = state(&mut shell);
    assert!(opened.open);
    assert_eq!(opened.scope, SearchScope::Commands);
    assert_eq!(opened.text, "");
    assert!(opened.commands.is_empty() && opened.highlight.is_none());
    let typed = type_text(&mut shell, "insert");
    assert!(typed.commands.len() > 2);
    assert_eq!(typed.highlight, Some(0));
    // The other scope's key keeps the text.
    open(&mut shell, "tracks");
    let tracks_scope = state(&mut shell);
    assert_eq!(
        (tracks_scope.scope, tracks_scope.text.as_str()),
        (SearchScope::Tracks, "insert")
    );
    assert!(tracks_scope.commands.is_empty());
    shell
        .run("search.set_scope", json!({ "scope": "commands" }))
        .unwrap();
    shell.run("search.close", json!({})).unwrap();
    let closed = state(&mut shell);
    assert!(!closed.open && closed.text.is_empty() && closed.commands.is_empty());
    assert_eq!(shell.editor().document(), &document);
    assert_eq!(shell.editor().undo_history().len(), history);
    assert_eq!(shell.is_dirty(), dirty);
    // The field's commands need it open.
    assert!(
        shell
            .run("search.set_text", json!({ "text": "x" }))
            .is_err()
    );
}

// Spec: command-palette R8. Down and Up wrap; Return runs the highlighted
// row as one undo step, with the selection as it was, and closes the field.
#[test]
fn return_runs_the_highlighted_row_as_one_undo_step_and_closes_the_field() {
    let mut shell = Shell::headless();
    let before = tracks(&shell).len();
    let history = shell.editor().undo_history().len();
    open(&mut shell, "commands");
    let typed = type_text(&mut shell, "insert midi track");
    assert_eq!(typed.commands[0].label, "Insert MIDI Track");
    assert_eq!(typed.commands[0].command, "track.create");
    // Up from the first row wraps to the last; Down returns.
    let last = state_after(&mut shell, "search.previous");
    assert_eq!(last.highlight, Some(last.commands.len() - 1));
    let first = state_after(&mut shell, "search.next");
    assert_eq!(first.highlight, Some(0));
    shell.search_run_row(0);
    assert!(!shell.search_open());
    assert_eq!(tracks(&shell).len(), before + 1);
    assert_eq!(shell.editor().undo_history().len(), history + 1);
    shell.invoke("edit.undo", json!({})).ok();
    assert_eq!(tracks(&shell).len(), before, "one undo step");
}

fn state_after(shell: &mut Shell, command: &str) -> SearchState {
    serde_json::from_value(shell.run(command, json!({})).unwrap()).unwrap()
}

// Spec: command-palette R7. A row that can't run now does nothing and the
// Status Bar says so.
#[test]
fn an_unavailable_row_runs_nothing_and_says_so() {
    let mut shell = Shell::headless();
    open(&mut shell, "commands");
    let typed = type_text(&mut shell, "undo");
    let at = typed
        .commands
        .iter()
        .position(|row| row.command == "edit.undo")
        .expect("Undo is a row");
    assert!(!typed.commands[at].available, "nothing to undo");
    for _ in 0..at {
        state_after(&mut shell, "search.next");
    }
    shell.search_run_row(at);
    assert!(shell.search_open(), "the field stays open");
    let message = shell.message().expect("a message");
    assert!(
        message.text.contains("isn't available now"),
        "{}",
        message.text
    );
    assert_eq!(shell.editor().undo_history().len(), 0);
}

// Spec: command-palette R9, R16. A track is found by its name, by a device's
// own name or given name inside a nested rack, and by a plug-in's name.
#[test]
fn tracks_are_found_by_name_device_and_plug_in() {
    let mut shell = Shell::headless();
    let ids = tracks(&shell);
    rename(&mut shell, ids[0], "Lead Vocal");
    nested_device(&mut shell, ids[1], "Auto Filter", "Wobble");
    plug_in(&mut shell, ids[2], "Cool Synth");

    let by_name = found(&mut shell, "vocal").rows;
    assert_eq!(by_name.len(), 1);
    assert_eq!((by_name[0].track, by_name[0].by_name), (ids[0], true));
    assert_eq!(by_name[0].device, None);

    // The renamed device inside the rack matches its own name and its new
    // one.
    for text in ["auto filter", "WOBBLE"] {
        let by_device = found(&mut shell, text).rows;
        assert_eq!(by_device.len(), 1, "{text}");
        assert_eq!(by_device[0].track, ids[1]);
        assert!(!by_device[0].by_name);
        assert!(by_device[0].device.is_some());
    }
    let by_plug_in = found(&mut shell, "cool").rows;
    assert_eq!(by_plug_in.len(), 1);
    assert_eq!(by_plug_in[0].track, ids[2]);
    assert_eq!(by_plug_in[0].device.as_deref(), Some("Cool Synth"));
    // Returns and Main can match, and the list is in display order.
    let main = shell.editor().document().main_id();
    let all = found(&mut shell, "main").rows;
    assert_eq!(all.last().map(|r| r.track), Some(main));
    assert!(found(&mut shell, "").rows.is_empty());
    assert!(found(&mut shell, "no track is called this").rows.is_empty());
}

// Spec: command-palette R11. Return walks the matches in display order,
// wrapping, opening a folded group that holds one; Shift-Return walks back.
#[test]
fn return_walks_the_matches_and_opens_a_folded_group() {
    let mut shell = Shell::headless();
    let (group, inside) = grouped(&mut shell);
    for (track, name) in inside.iter().zip(["Kick", "Kicker"]) {
        rename(&mut shell, *track, name);
    }
    let outside = tracks(&shell)[1];
    rename(&mut shell, outside, "Sidekick");
    shell
        .run("arrangement.select_tracks", json!({ "tracks": [] }))
        .unwrap();
    open(&mut shell, "tracks");
    let typed = type_text(&mut shell, "kick");
    assert_eq!(typed.tracks.len(), 3);
    assert!(shell.arrangement().is_folded(group));
    let order: Vec<TrackId> = typed.tracks.iter().map(|row| row.track).collect();
    let history = shell.editor().undo_history().len();
    let mut visited = Vec::new();
    for _ in 0..4 {
        state_after(&mut shell, "search.next");
        visited.push(selected(&shell));
    }
    // With none selected, the first match; then each next one; then the
    // first again.
    assert_eq!(visited[0], [order[0]]);
    assert_eq!(visited[1], [order[1]]);
    assert_eq!(visited[2], [order[2]]);
    assert_eq!(visited[3], [order[0]]);
    assert!(!shell.arrangement().is_folded(group), "the group opened");
    assert!(shell.search_open(), "the field stays open");
    state_after(&mut shell, "search.previous");
    assert_eq!(selected(&shell), [order[2]]);
    assert_eq!(shell.editor().undo_history().len(), history, "no undo step");
}

// Spec: command-palette R10. Matching tracks are outlined while the field
// is open on the Tracks scope, and the outlines follow the set.
#[test]
fn the_outlines_follow_the_text_the_scope_and_the_set() {
    let mut shell = Shell::headless();
    let ids = tracks(&shell);
    rename(&mut shell, ids[0], "Bass");
    assert!(!shell.search_outlines(ids[0]));
    open(&mut shell, "tracks");
    type_text(&mut shell, "bass");
    assert!(shell.search_outlines(ids[0]));
    assert!(!shell.search_outlines(ids[1]));
    // A rename while the field is open moves the outline.
    rename(&mut shell, ids[1], "Bassline");
    assert!(shell.search_outlines(ids[1]));
    rename(&mut shell, ids[0], "Drums");
    assert!(!shell.search_outlines(ids[0]));
    shell
        .run("search.set_scope", json!({ "scope": "commands" }))
        .unwrap();
    assert!(!shell.search_outlines(ids[1]));
    shell
        .run("search.set_scope", json!({ "scope": "tracks" }))
        .unwrap();
    assert!(shell.search_outlines(ids[1]));
    type_text(&mut shell, "");
    assert!(!shell.search_outlines(ids[1]));
}

// Spec: command-palette R12. Show Only Matches hides the other tracks in
// the lanes, the Session View's columns and both mixers, the groups that
// hold a match unfolded and the others' returns hidden; closing shows them
// again, each group folded as it was.
#[test]
fn show_only_matches_hides_the_others_until_the_search_ends() {
    let mut shell = Shell::headless();
    let (group, inside) = grouped(&mut shell);
    rename(&mut shell, inside[0], "Pad Left");
    let outside = tracks(&shell)[1];
    rename(&mut shell, outside, "Pad Right");
    shell
        .invoke("track.create", json!({ "kind": "return" }))
        .unwrap();
    let all_columns = shell.mixer_columns();
    let all_rows = shell.arrangement().rows(shell.editor().document());
    let returns = shell.shown_returns();
    assert!(!returns.is_empty());
    let document = shell.editor().document().clone();
    let history = shell.editor().undo_history().len();

    open(&mut shell, "tracks");
    // No match yet, nothing hidden.
    type_text(&mut shell, "pad");
    assert_eq!(shell.mixer_columns(), all_columns, "off until switched on");
    shell
        .run("search.set_only_matches", json!({ "on": true }))
        .unwrap();
    let columns = shell.mixer_columns();
    let mut expected = vec![group, inside[0], outside];
    expected.sort();
    let mut got = columns.clone();
    got.sort();
    assert_eq!(got, expected, "the matches and the group that holds one");
    let rows = shell.arrangement().rows(shell.editor().document());
    let shown: Vec<TrackId> = rows.iter().map(|r| r.track).collect();
    assert!(shown.contains(&group) && shown.contains(&inside[0]) && shown.contains(&outside));
    assert!(
        !shown.contains(&inside[1]),
        "a track that doesn't match is hidden"
    );
    assert!(
        shell.shown_returns().is_empty(),
        "returns that don't match are hidden"
    );
    assert!(
        shell.arrangement().is_folded(group)
            && !shell
                .arrangement()
                .is_folded_in(shell.editor().document(), group),
        "the group shows unfolded while its fold is kept"
    );
    // Text that matches nothing hides nothing.
    type_text(&mut shell, "zzz");
    assert_eq!(shell.mixer_columns(), all_columns);
    type_text(&mut shell, "pad");
    assert_ne!(shell.mixer_columns(), all_columns);
    // Closing the field shows everything again.
    shell.run("search.close", json!({})).unwrap();
    assert_eq!(shell.mixer_columns(), all_columns);
    assert_eq!(
        shell.arrangement().rows(shell.editor().document()),
        all_rows
    );
    assert_eq!(shell.shown_returns(), returns);
    assert!(shell.arrangement().is_folded(group), "folded as it was");
    // Nothing was saved or recorded as an undo step.
    assert_eq!(shell.editor().document(), &document);
    assert_eq!(shell.editor().undo_history().len(), history);
    // The switch starts off each time the field opens.
    open(&mut shell, "tracks");
    assert!(!state(&mut shell).only_matches);
}

// Spec: command-palette R12. The tracks Show Only Matches hides still play:
// a render of the set with the filter on equals one with it off.
#[test]
fn hidden_tracks_still_play() {
    let folder = scratch("render");
    let mut shell = shell_with_a_clip(&folder);
    let played = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.clips().len() > 0)
        .map(bl_model::Track::id)
        .unwrap();
    let other = tracks(&shell).into_iter().find(|t| *t != played).unwrap();
    rename(&mut shell, other, "Silent");
    let render = |shell: &mut Shell, name: &str| {
        let path = folder.join(name);
        shell
            .run(
                export_commands::AUDIO,
                json!({ "path": path.to_string_lossy(), "bit_depth": 24, "sample_rate": 48000 }),
            )
            .unwrap();
        std::fs::read(path).unwrap()
    };
    let plain = render(&mut shell, "plain.wav");
    open(&mut shell, "tracks");
    type_text(&mut shell, "silent");
    shell
        .run("search.set_only_matches", json!({ "on": true }))
        .unwrap();
    assert!(
        !shell
            .arrangement()
            .rows(shell.editor().document())
            .iter()
            .any(|row| row.track == played),
        "the playing track is hidden"
    );
    let filtered = render(&mut shell, "filtered.wav");
    assert!(plain == filtered, "the render doesn't change");
    let _ = std::fs::remove_dir_all(&folder);
}

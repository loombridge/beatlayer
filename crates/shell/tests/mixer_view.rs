//! The Mixer view's commands and its controls' commands, driven headless
//! through `Shell::run` and `Shell::invoke`, as agents, menus, keys, and the
//! window's controls drive them (WP-1.9). The behaviour is in the
//! mixer view spec.

use bl_commands::Permission;
use bl_model::TrackId;
use bl_shell::mixer_controls::{self as controls, Control, Switch};
use bl_shell::mixer_view::MixerViewState;
use bl_shell::mixer_view_commands as ids;
use bl_shell::{Invocation, KEY_TRACK_NUMBER, Shell};
use serde_json::{Value, json};

fn state(shell: &mut Shell) -> MixerViewState {
    serde_json::from_value(shell.run(ids::STATE, json!({})).unwrap()).unwrap()
}

fn tracks(shell: &Shell) -> Vec<TrackId> {
    shell.editor().document().top_level().collect()
}

fn mixer(shell: &Shell, track: TrackId) -> bl_model::Mixer {
    *shell.editor().document().track(track).unwrap().mixer()
}

fn undo_steps(shell: &Shell) -> usize {
    shell.editor().undo_history().count()
}

/// Runs what a control asked for as the window does: several commands in
/// one gesture.
fn apply(shell: &mut Shell, label: &str, invocations: Vec<Invocation>) {
    let several = invocations.len() > 1;
    if several {
        shell.begin_gesture(label);
    }
    for invocation in invocations {
        shell
            .invoke(&invocation.id, invocation.args.clone())
            .unwrap_or_else(|e| panic!("{}: {e}", invocation.id));
    }
    if several {
        shell.end_gesture();
    }
}

fn select(shell: &mut Shell, selected: &[TrackId], title_bars: bool) {
    shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": selected, "title_bars": title_bars }),
        )
        .unwrap();
}

// Spec: mixer-view R11, R12.
#[test]
fn a_new_set_shows_the_session_mixer_and_hides_the_arrangement_s() {
    let mut shell = Shell::headless();
    let shown = state(&mut shell);
    assert!(shown.session.shown);
    assert!(!shown.arrangement.shown);
    assert!(shown.in_out && shown.sends && shown.volume && shown.return_tracks);
    assert!(!shown.crossfader, "the Crossfader section starts hidden");
    // The Arrangement is in front in a new set: the Mixer item is
    // unticked there, and ticked in the Session View.
    assert_eq!(shell.switch_state(ids::TOGGLE), Some(false));
    shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    assert_eq!(shell.switch_state(ids::TOGGLE), Some(true));
    // A new set goes back to the defaults.
    shell
        .run(ids::TOGGLE, json!({ "view": "arrangement" }))
        .unwrap();
    shell.run(ids::SENDS, json!({})).unwrap();
    shell.run("set.new", json!({})).unwrap();
    let shown = state(&mut shell);
    assert!(!shown.arrangement.shown && shown.sends);
}

// Spec: mixer-view R11, R12.
#[test]
fn mixer_and_section_commands_switch_and_tick_their_items() {
    let mut shell = Shell::headless();
    // View ▸ Mixer acts on the view in front.
    shell.invoke(ids::TOGGLE, json!({})).unwrap();
    let shown = state(&mut shell);
    assert!(shown.arrangement.shown && shown.session.shown);
    assert_eq!(shell.switch_state(ids::TOGGLE), Some(true));
    shell.invoke(ids::TOGGLE, json!({ "shown": true })).unwrap();
    assert!(state(&mut shell).arrangement.shown, "shown stays shown");
    shell
        .invoke(ids::TOGGLE, json!({ "view": "session", "shown": false }))
        .unwrap();
    assert!(!state(&mut shell).session.shown);
    for (id, read, initial) in [
        (
            ids::IN_OUT,
            (|s: &MixerViewState| s.in_out) as fn(&MixerViewState) -> bool,
            true,
        ),
        (ids::SENDS, |s| s.sends, true),
        (ids::VOLUME, |s| s.volume, true),
        (ids::CROSSFADER, |s| s.crossfader, false),
        (ids::RETURN_TRACKS, |s| s.return_tracks, true),
    ] {
        assert!(shell.can_invoke(id, &json!({})), "{id}");
        assert_eq!(shell.switch_state(id), Some(initial), "{id}");
        shell.invoke(id, json!({})).unwrap();
        assert_eq!(read(&state(&mut shell)), !initial, "{id}");
        assert_eq!(shell.switch_state(id), Some(!initial), "{id}");
        shell.invoke(id, json!({ "shown": !initial })).unwrap();
        assert_eq!(
            read(&state(&mut shell)),
            !initial,
            "{id}: shown stays shown"
        );
        shell.invoke(id, json!({ "shown": initial })).unwrap();
        assert_eq!(read(&state(&mut shell)), initial, "{id}");
    }
    // None of them is an undo step or touches the set.
    assert!(!shell.is_dirty());
    assert_eq!(undo_steps(&shell), 0);
}

// Spec: mixer-view R13, R16.
#[test]
fn every_mixer_view_command_is_registered_read_only_and_reachable() {
    let mut shell = Shell::headless();
    for id in ids::IDS {
        let spec = shell.registry().get(id).unwrap_or_else(|| panic!("{id}"));
        assert_eq!(spec.permission, Some(Permission::Read), "{id}");
        assert!(!spec.description.is_empty() && spec.args.is_some(), "{id}");
    }
    for (id, args) in [
        (ids::STATE, json!({})),
        (ids::TOGGLE, json!({ "view": "session" })),
        (ids::IN_OUT, json!({ "shown": false })),
        (ids::SENDS, json!({})),
        (ids::VOLUME, json!({})),
        (ids::CROSSFADER, json!({})),
        (ids::RETURN_TRACKS, json!({})),
        (ids::SCROLL, json!({ "view": "session", "x": 10.0 })),
        (ids::FOCUS, json!({})),
    ] {
        let value = shell.run(id, args).unwrap_or_else(|e| panic!("{id}: {e}"));
        let _: MixerViewState = serde_json::from_value(value).unwrap();
    }
    // The focus command shows the Arrangement's mixer and asks the window
    // to move the focus there once.
    let shown = state(&mut shell);
    assert!(shown.arrangement.shown);
    assert_eq!(shell.mixer_view().focus_requests(), 1);
    // It applies with the Arrangement focused, as Navigate ▸ Mixer does.
    assert!(shell.can_invoke(ids::FOCUS, &json!({})));
    shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    assert!(!shell.can_invoke(ids::FOCUS, &json!({})));
    assert!(
        shell
            .run(ids::TOGGLE, json!({ "view": "nowhere" }))
            .is_err()
    );
    assert!(shell.run(ids::SCROLL, json!({})).is_err());
    assert_eq!(undo_steps(&shell), 0);
}

// Spec: mixer-view R14.
#[test]
fn the_mixer_scrolls_within_its_tracks() {
    let mut shell = Shell::headless();
    let width = f64::from(ids::COLUMN_WIDTH);
    shell
        .run(ids::SCROLL, json!({ "view": "session", "x": 100.0 }))
        .unwrap();
    assert_eq!(state(&mut shell).session.scroll, 100.0);
    assert_eq!(state(&mut shell).arrangement.scroll, 0.0, "each its own");
    shell
        .run(ids::SCROLL, json!({ "view": "session", "x": 1e6 }))
        .unwrap();
    // Four tracks: until the fourth is the first in view.
    assert_eq!(state(&mut shell).session.scroll, width * 3.0);
    shell
        .run(ids::SCROLL, json!({ "view": "session", "x": -1e6 }))
        .unwrap();
    assert_eq!(state(&mut shell).session.scroll, 0.0);
    // Deleting tracks pulls the scroll back.
    shell
        .run(ids::SCROLL, json!({ "view": "session", "x": 1e6 }))
        .unwrap();
    let last = *tracks(&shell).last().unwrap();
    shell.run("track.delete", json!({ "track": last })).unwrap();
    assert_eq!(state(&mut shell).session.scroll, width * 2.0);
}

// Spec: mixer-view R7, R20.
#[test]
fn a_click_on_solo_or_arm_is_exclusive_and_cmd_adds() {
    let mut shell = Shell::headless();
    let t = tracks(&shell);
    let click = |shell: &mut Shell, switch: Switch, track: TrackId, add: bool| {
        let doc = shell.editor().document().clone();
        let invocations = controls::clicked(&doc, switch, track, &[], add);
        apply(shell, switch.gesture(), invocations);
    };
    for switch in [Switch::Solo, Switch::Arm] {
        let on = |shell: &Shell| -> Vec<bool> {
            t.iter()
                .map(|track| switch.is_on(shell.editor().document(), *track))
                .collect()
        };
        click(&mut shell, switch, t[0], false);
        assert_eq!(on(&shell), [true, false, false, false], "{switch:?}");
        // A plain click releases the others.
        click(&mut shell, switch, t[1], false);
        assert_eq!(on(&shell), [false, true, false, false], "{switch:?}");
        // Cmd (Ctrl) adds.
        click(&mut shell, switch, t[2], true);
        assert_eq!(on(&shell), [false, true, true, false], "{switch:?}");
        // A plain click on one lit among others keeps it alone.
        click(&mut shell, switch, t[2], false);
        assert_eq!(on(&shell), [false, false, true, false], "{switch:?}");
        // On the only one lit, it goes off.
        click(&mut shell, switch, t[2], false);
        assert_eq!(on(&shell), [false; 4], "{switch:?}");
        // Cmd-click on a lit one turns only it off.
        click(&mut shell, switch, t[0], true);
        click(&mut shell, switch, t[3], true);
        click(&mut shell, switch, t[0], true);
        assert_eq!(on(&shell), [false, false, false, true], "{switch:?}");
        click(&mut shell, switch, t[3], false);
    }
    // The activator only toggles.
    click(&mut shell, Switch::Activator, t[1], false);
    assert!(mixer(&shell, t[1]).mute);
    click(&mut shell, Switch::Activator, t[1], true);
    assert!(!mixer(&shell, t[1]).mute);
    // Main has none of them; returns have no Arm.
    let doc = shell.editor().document().clone();
    let main = doc.main_id();
    let a = doc.returns().next().unwrap();
    assert!(controls::clicked(&doc, Switch::Solo, main, &[], false).is_empty());
    assert!(controls::clicked(&doc, Switch::Arm, a, &[], false).is_empty());
    assert!(!controls::clicked(&doc, Switch::Solo, a, &[], false).is_empty());
}

// Spec: mixer-view R8.
#[test]
fn a_change_on_a_selected_track_changes_every_selected_track() {
    let mut shell = Shell::headless();
    let t = tracks(&shell);
    // Different levels on three tracks, then select two of them.
    shell
        .run("mixer.set_volume", json!({ "track": t[0], "volume": 0.5 }))
        .unwrap();
    shell
        .run("mixer.set_pan", json!({ "track": t[1], "pan": 0.5 }))
        .unwrap();
    select(&mut shell, &[t[0], t[1]], false);
    let selected = shell.arrangement().selected_tracks().to_vec();
    let doc = shell.editor().document().clone();
    let starts: Vec<(TrackId, f64)> = controls::targets(t[1], &selected, |_| true)
        .into_iter()
        .map(|track| (track, Control::Volume.value(&doc, track).unwrap()))
        .collect();
    assert_eq!(starts[0].0, t[1], "the clicked track first");
    let before = undo_steps(&shell);
    // Nine points of drag down from 0 dB.
    apply(
        &mut shell,
        Control::Volume.gesture(),
        controls::moved(Control::Volume, &starts, -9.0),
    );
    assert_eq!(undo_steps(&shell), before + 1, "one step");
    // The dragged track moves along the drag's scale, the other selected
    // track by the same dB (observed), the unselected track not at all.
    let db = |track| controls::gain_db(mixer(&shell, track).volume);
    let moved_db = db(t[1]);
    let expected = controls::gain_db(controls::fader_gain(
        controls::fader_position(1.0) - 9.0 / controls::FADER_TRAVEL_POINTS,
    ));
    assert!((moved_db - expected).abs() < 1e-9);
    assert!((db(t[0]) - (controls::gain_db(0.5) + moved_db)).abs() < 1e-9);
    assert_eq!(mixer(&shell, t[2]).volume, 1.0);
    assert_eq!(
        shell.editor().undo_history().next().unwrap(),
        controls::VOLUME_GESTURE
    );
    // Pan moves both by the same amount, each kept in range.
    let doc = shell.editor().document().clone();
    let starts: Vec<(TrackId, f64)> = [t[0], t[1]]
        .iter()
        .map(|track| (*track, Control::Pan.value(&doc, *track).unwrap()))
        .collect();
    apply(
        &mut shell,
        Control::Pan.gesture(),
        controls::moved(Control::Pan, &starts, 0.75),
    );
    assert_eq!(mixer(&shell, t[0]).pan, 0.75);
    assert_eq!(mixer(&shell, t[1]).pan, 1.0);
    // A switch click on a selected track sets every selected track; an
    // exclusive Solo releases only the others.
    shell
        .run("mixer.set_solo", json!({ "track": t[3], "solo": true }))
        .unwrap();
    let doc = shell.editor().document().clone();
    apply(
        &mut shell,
        Switch::Solo.gesture(),
        controls::clicked(&doc, Switch::Solo, t[1], &selected, false),
    );
    let soloed: Vec<bool> = t.iter().map(|track| mixer(&shell, *track).solo).collect();
    assert_eq!(soloed, [true, true, false, false]);
    let doc = shell.editor().document().clone();
    apply(
        &mut shell,
        Switch::Activator.gesture(),
        controls::clicked(&doc, Switch::Activator, t[0], &selected, false),
    );
    assert!(mixer(&shell, t[0]).mute && mixer(&shell, t[1]).mute && !mixer(&shell, t[2]).mute);
    // A track that isn't selected changes alone.
    let doc = shell.editor().document().clone();
    assert_eq!(
        controls::clicked(&doc, Switch::Activator, t[2], &selected, false).len(),
        1
    );
}

// Spec: mixer-view R9.
#[test]
fn title_bar_keys_toggle_the_selected_tracks_as_one_step() {
    let mut shell = Shell::headless();
    let t = tracks(&shell);
    // Without a selected title bar, S has nothing to act on.
    assert!(!shell.can_invoke("mixer.set_solo", &json!({})));
    select(&mut shell, &[t[1], t[2]], true);
    for (id, read) in [
        (
            "mixer.set_solo",
            (|m: &bl_model::Mixer| m.solo) as fn(&bl_model::Mixer) -> bool,
        ),
        ("mixer.set_mute", |m| m.mute),
        ("mixer.set_arm", |m| m.arm),
    ] {
        assert!(shell.can_invoke(id, &json!({})), "{id}");
        let before = undo_steps(&shell);
        shell.invoke(id, json!({})).unwrap();
        assert_eq!(undo_steps(&shell), before + 1, "{id}: one step");
        assert!(
            read(&mixer(&shell, t[1])) && read(&mixer(&shell, t[2])),
            "{id}"
        );
        assert!(!read(&mixer(&shell, t[0])), "{id}");
        shell.invoke(id, json!({})).unwrap();
        assert!(
            !read(&mixer(&shell, t[1])) && !read(&mixer(&shell, t[2])),
            "{id}"
        );
    }
    // F1 to F8 name a track by its number.
    let mut args = serde_json::Map::new();
    args.insert(KEY_TRACK_NUMBER.to_owned(), Value::from(4));
    assert!(shell.can_invoke("mixer.set_mute", &Value::Object(args.clone())));
    shell
        .invoke("mixer.set_mute", Value::Object(args.clone()))
        .unwrap();
    assert!(mixer(&shell, t[3]).mute && !mixer(&shell, t[1]).mute);
    args.insert(KEY_TRACK_NUMBER.to_owned(), Value::from(8));
    assert!(!shell.can_invoke("mixer.set_mute", &Value::Object(args)));
    // With a track named, the commands run as they always did.
    shell
        .invoke("mixer.set_solo", json!({ "track": t[0], "solo": true }))
        .unwrap();
    assert!(mixer(&shell, t[0]).solo);
}

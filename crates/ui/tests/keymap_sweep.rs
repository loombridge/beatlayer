//! The keymap sweep (WP-6.70): every shortcut still pending from M1 to M3
//! is bound, handled by the focused control, or waits for the package that
//! builds it. Each key bound here is pressed in its context, and what it
//! runs is checked.
//!
//! Spec: transport R30; track context menu R17, R18; session context menus
//! R17; session view R44; device chains R43; arrangement editing R24;
//! ui-parity README, "How UI packages use it".

use crate::common;
use bl_commands::Context;
use bl_model::{SceneId, TrackId, TrackKind};
use bl_shell::{Area, Effect};
use bl_ui::keymap::{Keymap, Platform, Status};
use common::{Harness, Window, command};
use egui::{Event, Key, Modifiers};
use serde_json::json;

const BAR: i64 = 2_822_400;

fn harness() -> Harness {
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    h
}

fn session() -> Harness {
    let mut h = harness();
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    h.settle();
    h
}

fn tracks(h: &Harness) -> Vec<TrackId> {
    h.shell.editor().document().top_level().collect()
}

fn scenes(h: &Harness) -> Vec<SceneId> {
    h.shell
        .editor()
        .document()
        .scenes()
        .map(|scene| scene.id)
        .collect()
}

fn steps(h: &Harness) -> usize {
    h.shell.editor().undo_history().count()
}

fn press(h: &mut Harness, key: Key, modifiers: Modifiers) {
    h.key(key, modifiers);
    h.settle();
}

fn select_title_bars(h: &mut Harness, tracks: &[TrackId]) {
    h.shell
        .invoke(
            "arrangement.select_tracks",
            json!({ "tracks": tracks, "title_bars": true }),
        )
        .unwrap();
    h.settle();
}

fn kind_of(h: &Harness, track: TrackId) -> TrackKind {
    h.shell.editor().document().track(track).unwrap().kind()
}

// Spec: transport R30 (S-TRANS-006). F9 switches Record to Arrangement.
#[test]
fn f9_switches_recording_on_and_off() {
    let mut h = Harness::manual_with_title_bar(Window::Main);
    h.settle();
    let recording = |h: &mut Harness| {
        h.shell.run("transport.state", json!({})).unwrap()["arrangement_record"]
            .as_bool()
            .unwrap()
    };
    assert!(!recording(&mut h));
    press(&mut h, Key::F9, Modifiers::NONE);
    assert!(recording(&mut h), "F9 starts recording");
    press(&mut h, Key::F9, Modifiers::NONE);
    assert!(!recording(&mut h), "F9 again stops it");
}

// Spec: transport R30 (S-TRANS-007). Shift-F9 arms the selected tracks.
#[test]
fn shift_f9_arms_the_selected_tracks() {
    let mut h = harness();
    let tracks = tracks(&h);
    let armable = tracks[0];
    assert_ne!(kind_of(&h, armable), TrackKind::Group);
    select_title_bars(&mut h, &[armable]);
    let armed = |h: &Harness| {
        h.shell
            .editor()
            .document()
            .track(armable)
            .unwrap()
            .mixer()
            .arm
    };
    assert!(!armed(&h));
    press(&mut h, Key::F9, Modifiers::SHIFT);
    assert!(armed(&h), "Shift-F9 arms the track");
    press(&mut h, Key::F9, Modifiers::SHIFT);
    assert!(!armed(&h), "and again disarms it");
}

// Spec: session-context-menus R17 (S-FILE-008, M-FILE-016). Cmd-Shift-E, and
// the File menu's item, export the selected MIDI clip: the app asks where.
#[test]
fn cmd_shift_e_exports_the_selected_midi_clip() {
    let mut h = session();
    let midi = *tracks(&h)
        .iter()
        .find(|t| kind_of(&h, **t) == TrackKind::Midi)
        .expect("a MIDI track");
    let scene = scenes(&h)[0];
    h.shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": midi, "scene": scene }),
        )
        .unwrap();
    let clip = h.shell.editor().document().slot(midi, scene).unwrap().id;
    assert!(
        !h.shell.can_invoke("midi.export_clip", &json!({})),
        "nothing is selected yet"
    );
    h.shell
        .invoke(
            "session.select",
            json!({ "track": midi, "scene": scene, "mode": "replace" }),
        )
        .unwrap();
    h.settle();
    assert!(
        h.shell.can_invoke("midi.export_clip", &json!({})),
        "File ▸ Export MIDI Clip... is enabled with a MIDI clip selected"
    );
    h.effects.clear();
    press(&mut h, Key::E, command() | Modifiers::SHIFT);
    assert!(
        h.effects.iter().any(|effect| matches!(
            effect,
            Effect::PickMidiExportPath { clip: asked, .. } if *asked == clip
        )),
        "{:?}",
        h.effects
    );
}

// Spec: track-context-menu R17 (S-TRACK-005; I-FOCUS-011). Tab in a track's name
// keeps it and starts the next track's.
#[test]
fn tab_while_renaming_a_track_moves_to_the_next_track() {
    let mut h = harness();
    let tracks = tracks(&h);
    select_title_bars(&mut h, &[tracks[0]]);
    h.shell.invoke("track.rename", json!({})).unwrap();
    h.settle();
    assert!(h.main.is_typing(), "Rename starts typing a name");
    h.frame(vec![Event::Text("Drums".to_owned())]);
    press(&mut h, Key::Tab, Modifiers::NONE);
    let name = |h: &Harness, t: TrackId| {
        h.shell
            .editor()
            .document()
            .track(t)
            .unwrap()
            .name()
            .to_owned()
    };
    assert!(name(&h, tracks[0]).contains("Drums"), "Tab keeps the name");
    assert!(h.main.is_typing(), "and starts the next track's");
    assert_eq!(h.shell.arrangement().selected_tracks(), [tracks[1]]);
    h.frame(vec![Event::Text("Bass".to_owned())]);
    press(&mut h, Key::Enter, Modifiers::NONE);
    assert!(name(&h, tracks[1]).contains("Bass"));
    assert!(!h.main.is_typing());
}

// Spec: session-context-menus R17 (S-EDIT-012). The same in the Session View's scenes.
#[test]
fn tab_while_renaming_a_scene_moves_to_the_next_scene() {
    let mut h = session();
    let scenes = scenes(&h);
    h.shell
        .invoke(
            "session.select",
            json!({ "scene": scenes[0], "mode": "replace" }),
        )
        .unwrap();
    h.settle();
    h.shell.invoke("session.rename", json!({})).unwrap();
    h.settle();
    h.frame(vec![Event::Text("Intro".to_owned())]);
    press(&mut h, Key::Tab, Modifiers::NONE);
    let name = |h: &Harness, s: SceneId| h.shell.editor().document().scene(s).unwrap().name.clone();
    assert_eq!(name(&h, scenes[0]), "Intro");
    h.frame(vec![Event::Text("Verse".to_owned())]);
    press(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(name(&h, scenes[1]), "Verse");
}

// Spec: device chains R43 (S-A11Y-014). Left and Right select the device
// before or after the selected one.
#[test]
fn left_and_right_move_between_devices() {
    let mut h = harness();
    let audio = tracks(&h)[2];
    h.shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    let mut ids = Vec::new();
    let (gain, through) = (
        bl_engine::testing::gain_type(),
        bl_engine::testing::through_type(),
    );
    for t in [gain, through] {
        let device = json!({ "kind": t.kind, "name": t.name, "role": t.role });
        let added: bl_shell::device_commands::DeviceAdded = serde_json::from_value(
            h.shell
                .run("device.add", json!({ "device": device }))
                .unwrap(),
        )
        .unwrap();
        ids.push(added.device);
    }
    h.shell.set_focused_area(Area::Detail);
    h.settle();
    h.shell
        .invoke("device.select", json!({ "device": ids[0] }))
        .unwrap();
    h.settle();
    press(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert_eq!(h.shell.devices().selected(), Some(ids[1]), "Right");
    press(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert_eq!(h.shell.devices().selected(), Some(ids[1]), "the last stays");
    press(&mut h, Key::ArrowLeft, Modifiers::NONE);
    assert_eq!(h.shell.devices().selected(), Some(ids[0]), "Left");
}

// Spec: track-context-menu R18 (S-TRACK-012). Cmd-Up and Cmd-Down move the
// selected title bars' tracks a place, keeping the gaps, as one undo step.
#[test]
fn cmd_down_and_up_move_the_selected_tracks_in_the_arrangement() {
    let mut h = harness();
    let t = tracks(&h);
    select_title_bars(&mut h, &[t[0], t[2]]);
    let before = steps(&h);
    press(&mut h, Key::ArrowDown, command());
    assert_eq!(tracks(&h)[..4], [t[1], t[0], t[3], t[2]]);
    assert_eq!(steps(&h), before + 1, "one undo step");
    press(&mut h, Key::ArrowUp, command());
    assert_eq!(tracks(&h)[..4], [t[0], t[1], t[2], t[3]]);
    // At the top, the first selected track holds the others in their places.
    press(&mut h, Key::ArrowUp, command());
    assert_eq!(tracks(&h)[..4], [t[0], t[1], t[2], t[3]]);
}

// Spec: track-context-menu R18 (S-SES-015). Cmd-Left and Cmd-Right move the
// selected tracks a place.
#[test]
fn cmd_right_moves_the_selected_track_in_the_session_view() {
    let mut h = session();
    let t = tracks(&h);
    h.shell
        .invoke(
            "session.select",
            json!({ "track": t[0], "mode": "replace" }),
        )
        .unwrap();
    h.settle();
    press(&mut h, Key::ArrowRight, command());
    assert_eq!(tracks(&h)[..2], [t[1], t[0]]);
    press(&mut h, Key::ArrowLeft, command());
    assert_eq!(tracks(&h)[..2], [t[0], t[1]]);
}

// Spec: session-view R44 (S-SES-016). Cmd-Down and Cmd-Up move the selected
// scenes a place, keeping the gaps between them.
#[test]
fn cmd_down_moves_the_selected_scenes() {
    let mut h = session();
    let s = scenes(&h);
    h.shell
        .invoke(
            "session.select",
            json!({ "scene": s[0], "mode": "replace" }),
        )
        .unwrap();
    h.shell
        .invoke("session.select", json!({ "scene": s[2], "mode": "add" }))
        .unwrap();
    h.settle();
    let before = steps(&h);
    press(&mut h, Key::ArrowDown, command());
    assert_eq!(scenes(&h)[..4], [s[1], s[0], s[3], s[2]]);
    assert_eq!(steps(&h), before + 1, "one undo step");
    press(&mut h, Key::ArrowUp, command());
    assert_eq!(scenes(&h)[..4], [s[0], s[1], s[2], s[3]]);
}

// Spec: arrangement-editing R24 (S-ARR-037; C-TIME-006). Cmd-Shift-M makes an
// empty MIDI clip over the time selection of a MIDI track.
#[test]
fn cmd_shift_m_inserts_an_empty_midi_clip_over_the_time_selection() {
    let mut h = harness();
    let midi = *tracks(&h)
        .iter()
        .find(|t| kind_of(&h, **t) == TrackKind::Midi)
        .expect("a MIDI track");
    assert!(
        !h.shell
            .can_invoke("arrangement.insert_empty_midi_clip", &json!({})),
        "nothing is selected"
    );
    h.shell
        .invoke(
            "arrangement.select_time",
            json!({ "start": BAR, "end": 3 * BAR, "tracks": [midi] }),
        )
        .unwrap();
    h.settle();
    assert!(
        h.shell
            .can_invoke("arrangement.insert_empty_midi_clip", &json!({})),
        "the time selection's menu item is enabled"
    );
    let before = steps(&h);
    press(&mut h, Key::M, command() | Modifiers::SHIFT);
    let doc = h.shell.editor().document();
    let clips: Vec<_> = doc.track(midi).unwrap().clips().collect();
    assert_eq!(clips.len(), 1);
    assert_eq!(
        (clips[0].start.get(), clips[0].length.get()),
        (BAR, 2 * BAR)
    );
    assert_eq!(steps(&h), before + 1, "one undo step");
    // A track with a clip in the span is left alone.
    let error = h
        .shell
        .run("arrangement.insert_empty_midi_clip", json!({}))
        .unwrap_err();
    assert!(error.to_string().contains("already has a clip"), "{error}");
}

// Spec: device-chains R43 (S-DEV-007, S-TRACK-015). Enter in the browser loads
// the selected device, the command S-BRW-004 runs.
#[test]
fn enter_in_the_browser_loads_the_selected_item() {
    let keymap = Keymap::live12();
    for shortcut in ["S-BRW-004", "S-DEV-007", "S-TRACK-015"] {
        let entry = keymap.entry_for(shortcut).expect(shortcut);
        assert_eq!(entry.status, Status::Bound, "{shortcut}");
        assert!(
            entry
                .commands
                .iter()
                .any(|c| c.id == "browser.load_selected" && c.contexts == [Context::Browser]),
            "{shortcut}: {:?}",
            entry.commands
        );
    }
    let hit = keymap
        .resolve(
            Platform::Mac,
            Key::Enter,
            common_mods(),
            &[Context::Browser, Context::Global],
        )
        .expect("Enter is bound in the browser");
    assert_eq!(hit.command.id, "browser.load_selected");
}

fn common_mods() -> bl_ui::keymap::Mods {
    bl_ui::keymap::Mods {
        ctrl: false,
        alt: false,
        shift: false,
        cmd: false,
    }
}

// The gestures the views read themselves are marked so in the keymap, and the
// shortcuts that wait for a package say which.
#[test]
fn the_views_gestures_are_marked_and_the_waiting_shortcuts_name_their_packages() {
    let keymap = Keymap::live12();
    for shortcut in [
        "S-CTX-001",
        "S-DEV-005",
        "S-ENV-003",
        "S-ARR-036",
        "S-SES-004",
        "S-SES-017",
        "S-A11Y-014",
        "S-EDIT-012",
        "S-TRACK-005",
        "S-TRACK-012",
        "S-SES-015",
        "S-SES-016",
    ] {
        let entry = keymap.entry_for(shortcut).expect(shortcut);
        assert!(
            matches!(entry.status, Status::Widget(_)),
            "{shortcut}: {:?}",
            entry.status
        );
    }
    for (shortcut, milestone, package) in [
        ("S-ARR-012", "M6", "WP-6.73"),
        ("S-LOOP-007", "M6", "WP-6.73"),
        ("S-LOOP-008", "M6", "WP-6.73"),
        ("S-LOOP-009", "M6", "WP-6.73"),
        ("S-LOOP-010", "M6", "WP-6.73"),
        ("S-TRANS-003", "M6", "WP-6.74"),
        ("S-TRANS-004", "M6", "WP-6.74"),
        ("S-DEV-010", "M6", "WP-6.75"),
        ("S-DEV-004", "M6", "WP-6.76"),
        ("S-FOCUS-009", "M2", "WP-6.72"),
    ] {
        let entry = keymap.entry_for(shortcut).expect(shortcut);
        assert_eq!(
            entry.status,
            Status::Pending {
                milestone: milestone.to_owned(),
                package: Some(package.to_owned()),
            },
            "{shortcut}"
        );
    }
}

//! The keyboard and screen-reader baseline for every M1 view (WP-1.14),
//! headless: what the accessibility tree gives a screen reader for tracks,
//! clips, and faders; where the keyboard focus goes and what the tree
//! reports as focused as it moves; Tab, the neighbour keys, and the Navigate
//! menu; Outline View in Focus; Display & Input's keyboard settings; and a
//! walk of the whole command registry that fails for any command with no
//! keyboard path that isn't listed with its reason.
//!
//! Spec: accessibility R1 to R9.

mod common;

use std::collections::BTreeMap;

use bl_core::time::Ticks;
use bl_model::{AudioData, Clip, ClipData, ClipId, Command, TrackId};
use bl_shell::files::factory_default_set;
use bl_shell::menus::{Action, MenuPlatform, menu_bar};
use bl_shell::mixer_view::MixerOf;
use bl_shell::{Area, Shell};
use bl_ui::arrangement::benchmark;
use bl_ui::arrangement::headers::title_id;
use bl_ui::focus::Slot;
use bl_ui::in_out::control_id;
use bl_ui::keymap::{Keymap, Platform, Status};
use bl_ui::layout::{ELEMENTS, Strip};
use bl_ui::main_window::{UiEvent, element_id, main_view_id};
use bl_ui::menus::item_state;
use bl_ui::mixer::{Place, column_id, fader_id, pan_id, send_id, switch_id, volume_field_id};
use common::{Harness, Window, command};
use egui::accesskit::{self, Role};
use egui::{FullOutput, Id, Key, Modifiers};
use serde_json::json;

const BAR: i64 = Ticks::WHOLE.get();

/// The factory set with a clip called "Kick" on 3 Audio over bars 3 to 7,
/// drawn with Live's title bar at the reference size.
fn set() -> (Harness, Vec<TrackId>, ClipId) {
    let mut editor = factory_default_set();
    let tracks: Vec<TrackId> = editor.document().top_level().collect();
    let id: ClipId = editor.new_id();
    let mut clip = Clip::new(
        id,
        Ticks(2 * BAR),
        Ticks(4 * BAR),
        ClipData::Audio(AudioData::new(benchmark::media_hash(0))),
    );
    clip.name = "Kick".to_owned();
    editor
        .apply(Command::CreateClip {
            track: tracks[2],
            clip,
        })
        .unwrap();
    editor.clear_history().unwrap();
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell = benchmark::shell_with(editor);
    h.settle();
    (h, tracks, id)
}

/// The accessibility tree's nodes, by ID.
fn tree(output: &FullOutput) -> BTreeMap<accesskit::NodeId, accesskit::Node> {
    output
        .platform_output
        .accesskit_update
        .as_ref()
        .expect("an AccessKit tree update")
        .nodes
        .iter()
        .cloned()
        .collect()
}

/// The node a screen reader reads as focused: the tree's focus, or its
/// active descendant (R2).
fn focused(output: &FullOutput) -> Option<(accesskit::NodeId, accesskit::Node)> {
    let update = output.platform_output.accesskit_update.as_ref()?;
    let nodes = tree(output);
    let focus = update.focus;
    let node = nodes.get(&focus)?;
    let id = node.active_descendant().unwrap_or(focus);
    nodes.get(&id).map(|n| (id, n.clone()))
}

/// The label of what a screen reader reads as focused.
fn focused_label(output: &FullOutput) -> String {
    // A text field without a label is read by its placeholder.
    focused(output)
        .and_then(|(_, n)| n.label().or_else(|| n.placeholder()).map(str::to_owned))
        .unwrap_or_default()
}

fn press(h: &mut Harness, key: Key, modifiers: Modifiers) -> FullOutput {
    h.key(key, modifiers);
    h.settle()
}

/// Option on macOS, Ctrl elsewhere: the neighbour keys' modifier
/// (S-FOCUS-013).
fn neighbour() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::ALT
    } else {
        Modifiers::CTRL
    }
}

fn focused_id(h: &Harness) -> Option<Id> {
    h.ctx.memory(egui::Memory::focused)
}

fn tab_navigation_on(h: &mut Harness) {
    if !h.shell.tab_navigation() {
        h.shell
            .run("focus.toggle_tab_navigation", json!({}))
            .unwrap();
    }
}

// Spec: accessibility R1.
#[test]
fn screen_readers_read_track_names_clip_names_and_fader_values() {
    let (mut h, tracks, clip) = set();
    let output = h.settle();
    let nodes = tree(&output);
    let node = |id: Id| nodes.get(&id.accesskit_id()).cloned().unwrap();

    // Tracks: each header's title is a row header named for the track.
    let names: Vec<String> = Harness::nodes(&output)
        .into_iter()
        .filter(|n| n.role == Role::RowHeader)
        .map(|n| n.label)
        .collect();
    for name in [
        "1 MIDI", "2 MIDI", "3 Audio", "4 Audio", "A Return", "B Return", "Main",
    ] {
        assert!(names.iter().any(|n| n == name), "{name} in {names:?}");
    }
    let title = node(title_id(tracks[2]));
    assert_eq!(title.role(), Role::RowHeader);
    assert_eq!(title.label(), Some("3 Audio"));
    assert_eq!(title.description(), Some("Audio track 3"));

    // Clips: a list item named for the clip, with where it plays.
    let kick = node(bl_ui::arrangement::clip_id(clip));
    assert_eq!(kick.role(), Role::ListItem);
    assert_eq!(kick.label(), Some("Kick"));
    assert_eq!(kick.value(), Some("3.1.1 to 7.1.1"));
    assert_eq!(kick.is_selected(), Some(false));

    // Faders: sliders whose value is in dB, as the Status Bar shows it.
    let volume = node(volume_field_id(Place::Header, tracks[2]));
    assert_eq!(volume.role(), Role::Slider);
    assert_eq!(volume.label(), Some("Track Volume"));
    assert_eq!(volume.value(), Some("0.0 dB"));
    h.shell
        .run(
            "mixer.set_volume",
            json!({ "track": tracks[2], "volume": 0.5 }),
        )
        .unwrap();
    let nodes = tree(&h.settle());
    let volume = &nodes[&volume_field_id(Place::Header, tracks[2]).accesskit_id()];
    assert_eq!(volume.value(), Some("-6.0 dB"));
    assert!((volume.numeric_value().unwrap() + 6.02).abs() < 0.01);
    h.shell
        .run(
            "mixer.set_volume",
            json!({ "track": tracks[2], "volume": 0.0 }),
        )
        .unwrap();
    let nodes = tree(&h.settle());
    let volume = &nodes[&volume_field_id(Place::Header, tracks[2]).accesskit_id()];
    assert_eq!(volume.value(), Some("-inf dB"));
    let pan = &nodes[&pan_id(Place::Header, tracks[2]).accesskit_id()];
    assert_eq!((pan.label(), pan.value()), (Some("Track Pan"), Some("C")));

    // The Session View's mixer: a group per track, its fader in dB.
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    let nodes = tree(&h.settle());
    let column = &nodes[&column_id(MixerOf::Session, tracks[3]).accesskit_id()];
    assert_eq!(
        (column.role(), column.label()),
        (Role::Group, Some("4 Audio"))
    );
    let fader = &nodes[&fader_id(Place::Mixer(MixerOf::Session), tracks[3]).accesskit_id()];
    assert_eq!(
        (fader.role(), fader.label(), fader.value()),
        (Role::Slider, Some("Track Volume"), Some("0.0 dB"))
    );
    let fader = &nodes[&fader_id(Place::Mixer(MixerOf::Session), tracks[2]).accesskit_id()];
    assert_eq!(fader.value(), Some("-inf dB"));
}

// Spec: modulation R7. A modulated control's value reads as its base, with
// the value it plays, in one wording for a fader, the header's field, a
// send, pan and a device's knob.
#[test]
fn a_modulated_control_reads_its_base_and_played_value() {
    use bl_devices::testing::{self, UTILITY_GAIN};
    use bl_model::{AutomationTarget, MixerParam};
    use bl_shell::mixer_controls::{db_gain, send_text};
    use bl_ui::device_panel::param_control_id;

    let (mut h, tracks, _) = set();
    let t = tracks[2];
    let ret = h.shell.editor().document().returns().next().unwrap();
    h.shell.set_device_types(testing::device_types());
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [t] }))
        .unwrap();
    let utility = testing::utility_type();
    let added: bl_shell::device_commands::DeviceAdded = serde_json::from_value(
        h.shell
            .run(
                "device.add",
                json!({ "device": { "kind": utility.kind, "name": utility.name, "role": utility.role } }),
            )
            .unwrap(),
    )
    .unwrap();
    for (id, args) in [
        (
            "mixer.set_volume",
            json!({ "track": t, "volume": db_gain(-6.0) }),
        ),
        ("mixer.set_pan", json!({ "track": t, "pan": 0.0 })),
        (
            "mixer.set_send",
            json!({ "track": t, "to": ret, "level": 0.5 }),
        ),
        (
            "device.set_param",
            json!({ "device": added.device, "param": UTILITY_GAIN, "value": db_gain(-6.0) }),
        ),
    ] {
        h.shell.run(id, args).unwrap();
    }
    let device = |param| AutomationTarget::Device {
        device: added.device,
        param,
    };
    let mixer = |param| AutomationTarget::Mixer { param };
    let played = [
        (mixer(MixerParam::Volume), db_gain(-6.0), db_gain(-12.0)),
        (mixer(MixerParam::Pan), 0.0, 0.5),
        (AutomationTarget::Send { to: ret }, 0.5, 0.25),
        (device(UTILITY_GAIN), db_gain(-6.0), db_gain(-18.0)),
    ];
    h.shell.set_modulated(
        played
            .iter()
            .map(|&(target, base, played)| {
                let (node, param) = bl_engine::target_node(t, &target).unwrap();
                bl_engine::PlayedParam {
                    node,
                    param,
                    base,
                    played,
                }
            })
            .collect(),
    );
    let value = |nodes: &BTreeMap<accesskit::NodeId, accesskit::Node>, id: Id| {
        nodes[&id.accesskit_id()].value().map(str::to_owned)
    };
    let nodes = tree(&h.settle());
    let send = |level: f64| format!("{} dB", send_text(level));
    // The header's volume field, pan and send, and the Device View's knob.
    assert_eq!(
        value(&nodes, volume_field_id(Place::Header, t)).as_deref(),
        Some("-6.0 dB, modulated to -12.0 dB")
    );
    assert_eq!(
        value(&nodes, pan_id(Place::Header, t)).as_deref(),
        Some("C, modulated to 25R")
    );
    assert_eq!(
        value(&nodes, send_id(Place::Header, t, ret)),
        Some(format!("{}, modulated to {}", send(0.5), send(0.25)))
    );
    assert_eq!(
        value(&nodes, param_control_id(added.device, UTILITY_GAIN)).as_deref(),
        Some("-6.0 dB, modulated to -18.0 dB")
    );
    // The Session View's fader, pan and send say the same.
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    let nodes = tree(&h.settle());
    let strip = Place::Mixer(MixerOf::Session);
    assert_eq!(
        value(&nodes, fader_id(strip, t)).as_deref(),
        Some("-6.0 dB, modulated to -12.0 dB")
    );
    assert_eq!(
        value(&nodes, pan_id(strip, t)).as_deref(),
        Some("C, modulated to 25R")
    );
    assert_eq!(
        value(&nodes, send_id(strip, t, ret)),
        Some(format!("{}, modulated to {}", send(0.5), send(0.25)))
    );
    // A control that plays its base reads as before.
    h.shell.set_modulated(Vec::new());
    let nodes = tree(&h.settle());
    assert_eq!(
        value(&nodes, fader_id(strip, t)).as_deref(),
        Some("-6.0 dB")
    );
}

// Spec: accessibility R2.
#[test]
fn the_screen_reader_follows_the_keyboard_focus() {
    let (mut h, tracks, clip) = set();
    // Option-2 (Alt-2): the Arrangement View has the keyboard.
    let output = press(&mut h, Key::Num2, Modifiers::ALT);
    assert_eq!(focused_id(&h), Some(main_view_id()));
    let (_, pane) = focused(&output).unwrap();
    assert_eq!(
        (pane.role(), pane.label()),
        (Role::Pane, Some("Arrangement View"))
    );
    // Down selects a track: the screen reader reads its title.
    let output = press(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(h.shell.arrangement().selected_tracks(), [tracks[0]]);
    assert_eq!(focused_label(&output), "1 MIDI");
    press(&mut h, Key::ArrowDown, Modifiers::NONE);
    let output = press(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(focused_label(&output), "3 Audio");
    let (id, title) = focused(&output).unwrap();
    assert_eq!(id, title_id(tracks[2]).accesskit_id());
    assert_eq!(title.role(), Role::RowHeader);

    // A selected clip: the screen reader reads the clip.
    h.shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    let output = h.settle();
    let (_, kick) = focused(&output).unwrap();
    assert_eq!(kick.label(), Some("Kick"));
    assert_eq!(kick.is_selected(), Some(true));

    // Tab: the selected track's title bar, then its controls, each read
    // as the focused control itself.
    h.shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[2]] }),
        )
        .unwrap();
    tab_navigation_on(&mut h);
    let output = press(&mut h, Key::Tab, Modifiers::NONE);
    assert_eq!(focused_id(&h), Some(title_id(tracks[2])));
    assert_eq!(focused_label(&output), "3 Audio");
    let output = press(&mut h, Key::Tab, Modifiers::NONE);
    let (_, control) = focused(&output).unwrap();
    assert_eq!(control.label(), Some("Input Type"));
    assert_eq!(control.value(), Some("Ext. In"));
    for _ in 0..5 {
        press(&mut h, Key::Tab, Modifiers::NONE);
    }
    let output = h.settle();
    let (_, control) = focused(&output).unwrap();
    assert_eq!(control.label(), Some("Track Activator"));
    assert_eq!(control.value(), Some("on"));

    // Navigate ▸ Mixer: the column, named for its track.
    let output = press(&mut h, Key::M, Modifiers::ALT | Modifiers::SHIFT);
    assert_eq!(
        focused_id(&h),
        Some(column_id(MixerOf::Arrangement, tracks[2]))
    );
    assert_eq!(focused_label(&output), "3 Audio");
}

/// The Control Bar's and Status Bar's elements that take the keyboard.
fn live_elements(strip: Strip) -> Vec<Id> {
    ELEMENTS
        .iter()
        .filter(|p| p.strip == strip && bl_ui::main_window::is_live(p.element))
        .map(|p| element_id(p.element))
        .collect()
}

// Spec: accessibility R3, R5.
#[test]
fn tab_moves_within_the_focused_area_and_wraps_with_wrap_tab_navigation() {
    let (mut h, _, _) = set();
    tab_navigation_on(&mut h);
    assert!(
        h.shell.wrap_navigation(),
        "on in a new install, as in Live 12.4.6"
    );
    press(&mut h, Key::Num0, Modifiers::ALT);
    let first = focused_id(&h).unwrap();
    assert_eq!(first, element_id(bl_ui::layout::Element::BrowserToggle));
    let bar = live_elements(Strip::ControlBar);
    // Shift-Tab from the first goes round to the last, Tab back to the first.
    press(&mut h, Key::Tab, Modifiers::SHIFT);
    let last = focused_id(&h).unwrap();
    assert!(bar.contains(&last) && last != first);
    press(&mut h, Key::Tab, Modifiers::NONE);
    assert_eq!(focused_id(&h), Some(first));
    // Without wrapping, Tab stays in the Control Bar and stops at its end.
    h.shell
        .run("focus.toggle_wrap_navigation", json!({}))
        .unwrap();
    let mut seen = vec![first];
    for _ in 0..bar.len() + 3 {
        press(&mut h, Key::Tab, Modifiers::NONE);
        let now = focused_id(&h).unwrap();
        assert!(bar.contains(&now), "Tab stays in the Control Bar: {now:?}");
        seen.push(now);
    }
    assert_eq!(*seen.last().unwrap(), last, "without wrapping it stops");
    assert_eq!(seen[seen.len() - 2], last);
    assert_eq!(h.shell.focused_area(), Area::ControlBar);

    // The browser: from the search field to the sidebar to the list, each
    // part's keys applying as the shell moves its focus there.
    press(&mut h, Key::F, command());
    assert_eq!(focused_id(&h), Some(bl_ui::browser::search_id()));
    press(&mut h, Key::Tab, Modifiers::NONE);
    assert_eq!(focused_id(&h), Some(bl_ui::browser::sidebar_id()));
    assert_eq!(
        h.shell.browser_view().focus(),
        bl_shell::browser::Part::Sidebar
    );
    press(&mut h, Key::Tab, Modifiers::NONE);
    assert_eq!(focused_id(&h), Some(bl_ui::browser::list_id()));
    assert_eq!(
        h.shell.browser_view().focus(),
        bl_shell::browser::Part::Content
    );
}

// Spec: accessibility R4.
#[test]
fn tab_runs_through_the_arrangement_and_the_neighbour_keys_to_the_next_track() {
    let (mut h, tracks, _) = set();
    tab_navigation_on(&mut h);
    press(&mut h, Key::Num2, Modifiers::ALT);
    h.shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[2]] }),
        )
        .unwrap();
    h.settle();
    use bl_shell::mixer_controls::Switch;
    let header = |switch| switch_id(Place::Header, switch, tracks[2]);
    let in_out = |slot| control_id(Place::Header, tracks[2], slot, 0);
    let returns: Vec<TrackId> = h.shell.editor().document().returns().collect();
    // Live's order (12.4.6), without the controls Beatlayer doesn't have
    // yet: the selected track's header (its In/Out choosers, its mixer
    // column, its sends), then Optimize Height and Width.
    let expected = [
        title_id(tracks[2]),
        in_out(Slot::InputType),
        in_out(Slot::InputChannel),
        in_out(Slot::Monitoring),
        in_out(Slot::OutputType),
        in_out(Slot::OutputChannel),
        header(Switch::Activator),
        header(Switch::Solo),
        header(Switch::Arm),
        volume_field_id(Place::Header, tracks[2]),
        pan_id(Place::Header, tracks[2]),
        send_id(Place::Header, tracks[2], returns[0]),
        send_id(Place::Header, tracks[2], returns[1]),
        bl_ui::arrangement::optimize_height_id(),
        bl_ui::arrangement::optimize_width_id(),
    ];
    for want in expected {
        press(&mut h, Key::Tab, Modifiers::NONE);
        assert_eq!(focused_id(&h), Some(want));
    }
    // Wrap Tab Navigation is on: round to the title bar.
    press(&mut h, Key::Tab, Modifiers::NONE);
    assert_eq!(focused_id(&h), Some(title_id(tracks[2])));
    press(&mut h, Key::Tab, Modifiers::SHIFT);
    assert_eq!(
        focused_id(&h),
        Some(bl_ui::arrangement::optimize_width_id())
    );
    // Off, it stays at the end.
    h.shell
        .run("focus.toggle_wrap_navigation", json!({}))
        .unwrap();
    press(&mut h, Key::Tab, Modifiers::NONE);
    assert_eq!(
        focused_id(&h),
        Some(bl_ui::arrangement::optimize_width_id())
    );
    for _ in 0..4 {
        press(&mut h, Key::Tab, Modifiers::SHIFT);
    }
    assert_eq!(focused_id(&h), Some(pan_id(Place::Header, tracks[2])));
    // Option-Tab (Ctrl-Tab): the same control on the next track, and back,
    // leaving the selection as it was.
    press(&mut h, Key::Tab, neighbour());
    assert_eq!(focused_id(&h), Some(pan_id(Place::Header, tracks[3])));
    assert_eq!(h.shell.arrangement().selected_tracks(), [tracks[2]]);
    assert_eq!(h.shell.focused_track(), Some(tracks[3]));
    press(&mut h, Key::Tab, neighbour() | Modifiers::SHIFT);
    assert_eq!(focused_id(&h), Some(pan_id(Place::Header, tracks[2])));
    // Esc goes back to the track's title bar.
    press(&mut h, Key::Escape, Modifiers::NONE);
    assert_eq!(focused_id(&h), Some(main_view_id()));
    assert!(h.shell.arrangement().title_bars_selected());
    assert_eq!(h.shell.arrangement().selected_tracks(), [tracks[2]]);

    // From the volume field, which takes typed values, the keys move too.
    for _ in 0..10 {
        press(&mut h, Key::Tab, Modifiers::NONE);
    }
    assert_eq!(
        focused_id(&h),
        Some(volume_field_id(Place::Header, tracks[2]))
    );
    press(&mut h, Key::Tab, neighbour());
    assert_eq!(
        focused_id(&h),
        Some(volume_field_id(Place::Header, tracks[3]))
    );
    press(&mut h, Key::Escape, Modifiers::NONE);
    assert_eq!(focused_id(&h), Some(main_view_id()));
}

fn peak_id(place: Place, track: TrackId) -> Id {
    Id::new(("mixer-peak", place, track))
}

// Spec: accessibility R4.
#[test]
fn tab_goes_round_a_session_strip_in_live_s_order() {
    let (mut h, tracks, _) = set();
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    tab_navigation_on(&mut h);
    h.shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[2]] }),
        )
        .unwrap();
    h.ctx.memory_mut(|m| m.request_focus(main_view_id()));
    h.settle();
    use bl_shell::mixer_controls::Switch;
    let session = Place::Mixer(MixerOf::Session);
    let strip = |switch| switch_id(session, switch, tracks[2]);
    let in_out = |track, slot| control_id(session, track, slot, 0);
    let returns: Vec<TrackId> = h.shell.editor().document().returns().collect();
    let scene = h
        .shell
        .editor()
        .document()
        .scenes()
        .next()
        .expect("the default scenes")
        .id;
    // The first Tab goes past the title bar to the clip slot, then the
    // strip in Live's order: the In/Out choosers, the sends, and the mixer.
    let expected = [
        bl_ui::session_view::slot_id(tracks[2], scene),
        in_out(tracks[2], Slot::InputType),
        in_out(tracks[2], Slot::InputChannel),
        in_out(tracks[2], Slot::Monitoring),
        in_out(tracks[2], Slot::OutputType),
        in_out(tracks[2], Slot::OutputChannel),
        send_id(session, tracks[2], returns[0]),
        send_id(session, tracks[2], returns[1]),
        peak_id(session, tracks[2]),
        pan_id(session, tracks[2]),
        strip(Switch::Activator),
        strip(Switch::Solo),
        strip(Switch::Arm),
        fader_id(session, tracks[2]),
        column_id(MixerOf::Session, tracks[2]),
        bl_ui::session_view::slot_id(tracks[2], scene),
    ];
    for want in expected {
        press(&mut h, Key::Tab, Modifiers::NONE);
        assert_eq!(focused_id(&h), Some(want));
    }
    // Without wrapping, Tab stays on Track Volume; Esc goes to the title
    // bar, which the column stands for.
    h.shell
        .run("focus.toggle_wrap_navigation", json!({}))
        .unwrap();
    for _ in 0..16 {
        press(&mut h, Key::Tab, Modifiers::NONE);
    }
    assert_eq!(focused_id(&h), Some(fader_id(session, tracks[2])));
    press(&mut h, Key::Escape, Modifiers::NONE);
    assert_eq!(focused_id(&h), Some(column_id(MixerOf::Session, tracks[2])));
    // A MIDI track without an instrument: its title bar, its In/Out
    // choosers, and its switches.
    h.ctx
        .memory_mut(|m| m.request_focus(column_id(MixerOf::Session, tracks[0])));
    h.settle();
    let midi = |switch| switch_id(session, switch, tracks[0]);
    for want in [
        bl_ui::session_view::slot_id(tracks[0], scene),
        in_out(tracks[0], Slot::InputType),
        in_out(tracks[0], Slot::InputChannel),
        in_out(tracks[0], Slot::Monitoring),
        in_out(tracks[0], Slot::OutputType),
        in_out(tracks[0], Slot::OutputChannel),
        midi(Switch::Activator),
        midi(Switch::Solo),
        midi(Switch::Arm),
    ] {
        press(&mut h, Key::Tab, Modifiers::NONE);
        assert_eq!(focused_id(&h), Some(want));
    }
}

// Spec: accessibility R4.
#[test]
fn the_neighbour_keys_bring_a_column_out_of_view_into_it() {
    let (mut h, _, _) = set();
    for _ in 0..20 {
        h.shell
            .invoke("track.create", json!({ "kind": "audio" }))
            .unwrap();
    }
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    tab_navigation_on(&mut h);
    h.settle();
    // The audio tracks' columns, from 3 Audio on.
    let columns = h.shell.mixer_columns()[2..].to_vec();
    h.ctx
        .memory_mut(|m| m.request_focus(column_id(MixerOf::Session, columns[0])));
    h.settle();
    let scene = h
        .shell
        .editor()
        .document()
        .scenes()
        .next()
        .expect("the default scenes")
        .id;
    let slot = |track| bl_ui::session_view::slot_id(track, scene);
    press(&mut h, Key::Tab, Modifiers::NONE);
    assert_eq!(focused_id(&h), Some(slot(columns[0])));
    for want in &columns[1..16] {
        press(&mut h, Key::Tab, neighbour());
        h.settle();
        assert_eq!(focused_id(&h), Some(slot(*want)));
    }
    let scroll = h.shell.mixer_view().of(MixerOf::Session).scroll;
    assert!(scroll > 0.0, "the mixer scrolled the column into view");
}

/// Every rectangle drawn with the focus colour in a frame.
fn focus_strokes(output: &FullOutput, colour: egui::Color32) -> Vec<egui::Rect> {
    fn walk(shape: &egui::Shape, colour: egui::Color32, out: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Rect(rect) if rect.stroke.color == colour && rect.stroke.width > 0.0 => {
                out.push(rect.rect);
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, colour, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, colour, &mut out);
    }
    out
}

// Spec: accessibility R7.
#[test]
fn outline_view_in_focus_outlines_the_focused_area_and_control_until_switched_off() {
    let (mut h, tracks, _) = set();
    let colour = h.theme.colors.focus.0;
    assert!(h.shell.outline_view_in_focus(), "on, as in Live 12.4.6");
    tab_navigation_on(&mut h);
    press(&mut h, Key::Num2, Modifiers::ALT);
    h.shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[2]] }),
        )
        .unwrap();
    // The title bar, the five In/Out controls, then the activator.
    for _ in 0..7 {
        press(&mut h, Key::Tab, Modifiers::NONE);
    }
    let output = h.settle();
    let strokes = focus_strokes(&output, colour);
    let drawn = |nodes: &[common::Node], role: Role, label: &str| {
        let b = nodes
            .iter()
            .find(|n| n.role == role && n.label == label && n.bounds.is_some())
            .and_then(|n| n.bounds)
            .unwrap();
        egui::Rect::from_min_max(
            egui::pos2(b.x0 as f32, b.y0 as f32),
            egui::pos2(b.x1 as f32, b.y1 as f32),
        )
    };
    let around = |strokes: &[egui::Rect], area: egui::Rect| {
        strokes
            .iter()
            .any(|r| (r.min - area.min).length() < 1.5 && (r.max - area.max).length() < 1.5)
    };
    let nodes = Harness::nodes(&output);
    let main = drawn(&nodes, Role::Pane, "Arrangement View").shrink(1.0);
    assert!(
        around(&strokes, main),
        "a border round the focused view: {strokes:?}"
    );
    let activator = nodes
        .iter()
        .find(|n| {
            n.id == switch_id(
                Place::Header,
                bl_shell::mixer_controls::Switch::Activator,
                tracks[2],
            )
            .accesskit_id()
        })
        .and_then(|n| n.bounds)
        .unwrap();
    let activator = egui::Rect::from_min_max(
        egui::pos2(activator.x0 as f32, activator.y0 as f32),
        egui::pos2(activator.x1 as f32, activator.y1 as f32),
    );
    assert!(
        strokes
            .iter()
            .any(|r| r.contains_rect(activator) && r.width() < activator.width() + 6.0),
        "an outline round the focused control"
    );
    // The Control Bar has the focus: the border goes there.
    press(&mut h, Key::Num0, Modifiers::ALT);
    let output = h.settle();
    let bar = drawn(&Harness::nodes(&output), Role::Toolbar, "Control Bar").shrink(1.0);
    assert!(around(&focus_strokes(&output, colour), bar));
    assert!(!around(&focus_strokes(&output, colour), main));
    // Switched off, nothing shows where the focus is.
    h.shell
        .run("settings.set_outline_view_in_focus", json!({ "on": false }))
        .unwrap();
    let output = h.settle();
    assert!(focused_id(&h).is_some());
    assert!(
        focus_strokes(&output, colour).is_empty(),
        "nothing shows where the focus is"
    );
}

fn open_settings(page: &str) -> Harness {
    let mut h = Harness::new(Window::Settings);
    h.shell
        .invoke("app.settings", json!({ "page": page }))
        .unwrap();
    h.shell.take_effects();
    h.settings.refresh();
    h.settle();
    h
}

/// Presses Tab until a screen reader would read `label` as focused, as a
/// person moving through a window with the keyboard does. False if Tab
/// never gets there.
fn tab_to(h: &mut Harness, label: &str) -> bool {
    for _ in 0..120 {
        let output = h.settle();
        if focused_label(&output) == label {
            return true;
        }
        h.key(Key::Tab, Modifiers::NONE);
    }
    false
}

// Spec: accessibility R6.
#[test]
fn display_and_input_has_live_s_keyboard_settings_in_order() {
    let mut h = open_settings("display-input");
    let nodes = Harness::nodes(&h.settle());
    let top = |label: &str| {
        Harness::node(&nodes, label)
            .bounds
            .unwrap_or_else(|| panic!("{label} has bounds"))
            .y0
    };
    let order = [
        "Zoom",
        "Outline View in Focus",
        "Show Scroll Bars",
        "Follow Behavior",
        "Hide Labels",
        "Use Tab Key to Move Focus",
        "Wrap Tab Navigation",
        "Move Clips With Arrow Keys",
    ];
    for pair in order.windows(2) {
        assert!(
            top(pair[0]) < top(pair[1]),
            "{} before {}",
            pair[0],
            pair[1]
        );
    }
    for built in [
        "Outline View in Focus",
        "Use Tab Key to Move Focus",
        "Wrap Tab Navigation",
    ] {
        let node = Harness::node(&nodes, built);
        assert!(!node.disabled, "{built}");
        assert_eq!(node.role, Role::Button, "{built}");
    }
    // The rows later packages build show the values Live 12.4.6 showed.
    for (pending, value) in [
        ("Show Scroll Bars", "Follow System"),
        ("Follow Behavior", "Scroll"),
        ("Hide Labels", "Show"),
        ("Move Clips With Arrow Keys", "On"),
    ] {
        let node = Harness::node(&nodes, pending);
        assert!(
            node.disabled && node.description.contains("later package"),
            "{pending}"
        );
        assert_eq!(node.value, value, "{pending}");
    }
    // Each switch by the keyboard: Tab to it, Enter. Live's new-install
    // values: Outline View in Focus on, Use Tab Key off, Wrap Tab on.
    for (label, on, was) in [
        (
            "Outline View in Focus",
            Shell::outline_view_in_focus as fn(&Shell) -> bool,
            true,
        ),
        ("Use Tab Key to Move Focus", Shell::tab_navigation, false),
        ("Wrap Tab Navigation", Shell::wrap_navigation, true),
    ] {
        assert_eq!(on(&h.shell), was, "{label} in a new install");
        assert!(tab_to(&mut h, label), "Tab reaches {label}");
        press(&mut h, Key::Enter, Modifiers::NONE);
        assert_eq!(on(&h.shell), !was, "Enter switched {label}");
    }
}

fn menu_item(h: &Harness, row: &str) -> bl_ui::menus::ItemState {
    let bar = menu_bar(MenuPlatform::current());
    let item = bar
        .iter()
        .flat_map(bl_shell::menus::Menu::walk)
        .find(|item| item.row == row)
        .unwrap_or_else(|| panic!("{row} is in the menus"));
    item_state(item, &h.shell, &h.keymap, Platform::current())
}

// Spec: accessibility R4, R5, R9.
#[test]
fn the_navigate_and_options_menus_reach_their_commands() {
    let (mut h, tracks, _) = set();
    // Next and Previous Neighbor follow whether there's a neighbour, not
    // the Tab setting (observed in Live 12.4.6), and show their keys.
    assert!(!h.shell.tab_navigation());
    for row in ["M-NAV-014", "M-NAV-015"] {
        let item = menu_item(&h, row);
        assert!(!item.enabled, "{row}: no track selected or focused");
        assert!(item.chord.is_some(), "{row} shows its keys");
    }
    h.shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[0]] }),
        )
        .unwrap();
    let previous = menu_item(&h, "M-NAV-014");
    let next = menu_item(&h, "M-NAV-015");
    assert!(!previous.enabled, "track 1 has no previous neighbour");
    assert!(next.enabled);
    assert_eq!(next.invocation.as_ref().unwrap().id, "focus.next_neighbor");
    // Chosen with the Arrangement focused, Next Neighbor moves the focus to
    // the next track and leaves the selection.
    press(&mut h, Key::Num2, Modifiers::ALT);
    h.shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[0]] }),
        )
        .unwrap();
    h.shell.invoke("focus.next_neighbor", json!({})).unwrap();
    let effects = h.shell.take_effects();
    for effect in &effects {
        h.main.apply_effect(&h.ctx, effect);
    }
    h.settle();
    assert_eq!(focused_id(&h), Some(title_id(tracks[1])));
    assert_eq!(h.shell.arrangement().selected_tracks(), [tracks[0]]);
    assert!(menu_item(&h, "M-NAV-014").enabled, "now track 2's");

    let wrap = menu_item(&h, "M-NAV-019");
    assert_eq!((wrap.enabled, wrap.checked), (true, Some(true)));
    h.shell
        .invoke(&wrap.invocation.unwrap().id, json!({}))
        .unwrap();
    assert_eq!(menu_item(&h, "M-NAV-019").checked, Some(false));
    // Options ▸ Solo in Place switches the set's setting, one undo step.
    let solo = menu_item(&h, "M-OPT-025");
    assert_eq!((solo.enabled, solo.checked), (true, Some(true)));
    let invocation = solo.invocation.unwrap();
    h.shell.invoke(&invocation.id, invocation.args).unwrap();
    assert!(!h.shell.editor().document().settings().solo_in_place);
    assert_eq!(menu_item(&h, "M-OPT-025").checked, Some(false));
    h.shell.invoke("edit.undo", json!({})).unwrap();
    assert!(h.shell.editor().document().settings().solo_in_place);
}

/// How a command is reached without a pointer, when it has no key and no
/// menu item.
enum Path {
    /// A control the keyboard reaches: the test opens `window`, runs
    /// `setup`, moves the focus where a person starts (`start`), presses
    /// Tab until the control is focused, then `key`, and checks the command
    /// ran.
    Control {
        window: Window,
        page: &'static str,
        setup: fn(&mut Harness),
        start: fn(&mut Harness),
        label: &'static str,
        key: Key,
    },
    /// No keyboard path of its own, with the reason (the accessibility
    /// spec, R8, lists each).
    Exception(Reason),
}

/// Why a command has no keyboard path of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Reason {
    /// A query for agents: a person reads the same on screen.
    Query,
    /// For agents only, with no person's form and no Live feature behind
    /// it, named in the text.
    AgentOnly(&'static str),
    /// An agent's form of a pointer gesture or dialog, whose keyboard path
    /// is another command or the system's dialog, named in the text.
    Equivalent(&'static str),
    /// Its view comes in a later milestone; until then agents reach it.
    LaterView(&'static str),
    /// Live itself has no keyboard path for it.
    Live(&'static str),
    /// Live's own keyboard path for it isn't observed yet, named in the
    /// text: its place in the mixers' Tab order and its key, which the next
    /// observation session records.
    Unobserved(&'static str),
    /// A gap in M1: what's missing, and what closes it.
    Gap(&'static str),
}

fn nothing(_: &mut Harness) {}

/// A Session clip on the first MIDI track, selected, with the Clip View on
/// its Envelopes tab showing Pan (WP-3.16).
fn envelopes_tab(h: &mut Harness) {
    let track = h.shell.editor().document().top_level().next().unwrap();
    let scene = h.shell.editor().document().scenes().next().unwrap().id;
    h.shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    h.shell
        .run("session.select", json!({ "track": track, "scene": scene }))
        .unwrap();
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    h.shell
        .run("view.clip_tab", json!({ "tab": "envelopes" }))
        .unwrap();
    h.shell
        .run(
            "view.clip_envelope",
            json!({ "target": { "type": "mixer", "param": "pan" } }),
        )
        .unwrap();
    h.settle();
}

/// As [`envelopes_tab`], with a breakpoint on Pan's envelope.
fn envelopes_tab_with_an_envelope(h: &mut Harness) {
    envelopes_tab(h);
    let track = h.shell.editor().document().top_level().next().unwrap();
    let scene = h.shell.editor().document().scenes().next().unwrap().id;
    let clip = h.shell.editor().document().slot(track, scene).unwrap().id;
    h.shell
        .run(
            "clip.set_envelope_breakpoint",
            json!({
                "clip": clip,
                "target": { "type": "mixer", "param": "pan" },
                "at": 0,
                "value": 0.25
            }),
        )
        .unwrap();
    h.settle();
}

/// Shows the tracks' automation lanes, so their choosers are drawn.
fn automation_mode_on(h: &mut Harness) {
    h.shell
        .run("arrangement.automation_mode", json!({ "on": true }))
        .unwrap();
    h.settle();
}

/// A MIDI input port for the Tempo & MIDI page's table to list (WP-2.2).
fn a_midi_port(h: &mut Harness) {
    let test = bl_midi_io::TestBackend::new();
    test.add_port(bl_midi_io::Direction::Input, "Keys");
    h.shell.use_midi_backend(Box::new(test));
}

fn arrangement_track_3(h: &mut Harness) {
    h.key(Key::Num2, Modifiers::ALT);
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
}

fn arrangement_track_1(h: &mut Harness) {
    h.key(Key::Num2, Modifiers::ALT);
    let track = h.shell.editor().document().top_level().next().unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
}

fn session_main(h: &mut Harness) {
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    h.settle();
    let main = h.shell.editor().document().main_id();
    h.ctx
        .memory_mut(|m| m.request_focus(column_id(MixerOf::Session, main)));
    h.settle();
}

/// The shell runs audio on a four-channel null device, so the In/Out
/// choosers list two output pairs and four inputs, as an interface does
/// (WP-2.13).
fn four_channel_interface(h: &mut Harness) {
    use bl_audio_io::{BackendKind, NullBackend, NullOptions};
    use bl_shell::audio::{AudioHost, AudioSettings};
    use bl_shell::files::ProjectFiles;
    use bl_shell::settings::Preferences;

    let backend = NullBackend::with_options(NullOptions {
        channels: 4,
        ..NullOptions::default()
    });
    let audio = AudioHost::with_backends(
        vec![BackendKind::Null],
        Box::new(move |_| Ok(Box::new(backend.clone()))),
        AudioSettings::default(),
    );
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        audio,
        Box::new(ProjectFiles::new()),
    );
    shell.start_audio();
    h.shell = shell;
}

/// A Test Gain on audio track 3, the selected track, with no device
/// selected (WP-2.4).
fn a_device(h: &mut Harness) {
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let t = bl_engine::testing::gain_type();
    h.shell
        .run(
            "device.add",
            json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

/// As [`a_rack`], with its chain select editor shown (WP-5.3).
fn a_rack_with_the_zone_editor(h: &mut Harness) {
    a_rack(h);
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    let rack = h
        .shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .devices()
        .last()
        .unwrap()
        .id;
    h.shell
        .run(
            "rack.set_zone_editor",
            json!({ "rack": rack, "editor": "select" }),
        )
        .unwrap();
}

/// The test plug-in on audio track 3, the selected track, once it offers
/// its editor, with no device selected (WP-2.10).
fn a_plugin(h: &mut Harness) {
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let loaded = h
        .shell
        .run(
            "plugin.load",
            json!({ "path": bl_test_clap::bundle(), "track": track }),
        )
        .unwrap();
    let device: bl_model::DeviceId = serde_json::from_value(loaded["device"].clone()).unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !h.shell.plugin_info(device).is_some_and(|i| i.has_window) {
        assert!(
            std::time::Instant::now() < until,
            "the plug-in never offered its editor"
        );
        h.shell.poll();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

/// An audio effect rack with two chains, Pad and Lead, on audio track 3, the
/// selected track, with no device selected (WP-5.2).
fn a_rack(h: &mut Harness) {
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let added = h
        .shell
        .run(
            "device.add",
            json!({ "device": {
                "kind": { "rack": { "kind": "audio_effect" } },
                "name": "Rack",
                "role": "audio_effect",
            } }),
        )
        .unwrap();
    let rack: bl_model::DeviceId = serde_json::from_value(added["device"].clone()).unwrap();
    for (index, name) in ["Pad", "Lead"].into_iter().enumerate() {
        let id: bl_model::ChainId = h.shell.new_id();
        h.shell
            .run(
                "chain.add",
                json!({ "rack": rack, "index": index, "chain": { "id": id, "name": name, "devices": [] } }),
            )
            .unwrap();
    }
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

/// A drum rack on the first MIDI track, the selected track, with a pad on C1
/// (a chain named Gain), a return chain named Reverb, and the rack's
/// Input/Output, Send and Return switches on, no device selected (WP-5.6).
fn a_drum_rack(h: &mut Harness) {
    let track = h.shell.editor().document().top_level().next().unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let added = h
        .shell
        .run(
            "device.add",
            json!({ "device": {
                "kind": { "rack": { "kind": "drum" } },
                "name": "Drum Rack",
                "role": "instrument",
            } }),
        )
        .unwrap();
    let rack: bl_model::DeviceId = serde_json::from_value(added["device"].clone()).unwrap();
    let t = bl_engine::testing::gain_type();
    let effect = json!({ "kind": t.kind, "name": t.name, "role": t.role });
    h.shell
        .run(
            "pad.drop",
            json!({ "rack": rack, "note": 36, "device": effect }),
        )
        .unwrap();
    h.shell
        .run(
            "pad.rename",
            json!({ "rack": rack, "note": 36, "name": "Gain" }),
        )
        .unwrap();
    let made = h
        .shell
        .run(
            "chain.create",
            json!({ "rack": rack, "device": effect, "returns": true }),
        )
        .unwrap();
    h.shell
        .run(
            "chain.rename",
            json!({ "chain": made["chain"], "name": "Reverb" }),
        )
        .unwrap();
    for view in ["input_output", "send", "returns"] {
        h.shell
            .run(
                "rack.set_view",
                json!({ "rack": rack, "view": view, "shown": true }),
            )
            .unwrap();
    }
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

/// The eight-slot drum sampler on the third track, the selected track, no
/// device selected (WP-5.17).
fn a_drum_sampler(h: &mut Harness) {
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let t = bl_devices::kitbag_type();
    h.shell
        .run(
            "device.add",
            json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

/// The Test Sidechain on audio track 3, the selected track, its sidechain
/// section shown, no device selected (WP-2.19b).
fn a_sidechained_device(h: &mut Harness) {
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let t = bl_engine::testing::key_type();
    let added = h
        .shell
        .run(
            "device.add",
            json!({ "device": {
                "kind": t.kind, "name": t.name, "role": t.role, "sidechain": true,
            } }),
        )
        .unwrap();
    h.shell
        .run(
            "device.show_sidechain",
            json!({ "device": added["device"], "shown": true }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

/// The Test MIDI Sidechain on audio track 3, the selected track, its
/// sidechain section shown, no device selected (WP-5.27).
fn a_midi_sidechained_device(h: &mut Harness) {
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let t = bl_engine::testing::hear_type();
    let added = h
        .shell
        .run(
            "device.add",
            json!({ "device": {
                "kind": t.kind, "name": t.name, "role": t.role, "midi_sidechain": true,
            } }),
        )
        .unwrap();
    h.shell
        .run(
            "device.show_sidechain",
            json!({ "device": added["device"], "shown": true }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

/// An External Instrument on a new MIDI track, its section showing, no
/// device selected (WP-5.34).
fn a_hardware_device(h: &mut Harness) {
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let t = bl_devices::hardware_instrument_type();
    h.shell
        .run(
            "device.add",
            json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

/// An LFO and a test utility on the selected audio track (WP-5.10),
/// with the LFO's slot 1 mapped to the utility's gain when `mapped`
/// and its Map button lit when `lit`.
fn modulators_on_track(h: &mut Harness, mapped: bool, lit: bool) {
    let mut types = bl_devices::testing::device_types();
    types.extend(bl_devices::device_types());
    h.shell.set_device_types(types);
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let add = |h: &mut Harness, t: &bl_engine::DeviceType| -> serde_json::Value {
        h.shell
            .run(
                "device.add",
                json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
            )
            .unwrap()["device"]
            .clone()
    };
    let utility = add(h, &bl_devices::testing::utility_type());
    let wave = add(h, &bl_devices::modulator::wave_mod_type());
    if mapped {
        h.shell
            .run(
                "modulator.map",
                json!({ "device": wave, "slot": 0,
                        "target": { "type": "device", "device": utility, "param": 0 } }),
            )
            .unwrap();
    }
    if lit {
        h.shell
            .run("modulator.arm_map", json!({ "device": wave, "slot": 0 }))
            .unwrap();
    }
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

fn a_wave_mod(h: &mut Harness) {
    modulators_on_track(h, false, false);
}

fn a_mapped_wave_mod(h: &mut Harness) {
    modulators_on_track(h, true, false);
}

fn a_lit_map_button(h: &mut Harness) {
    modulators_on_track(h, false, true);
}

/// A Shaper on the selected audio track.
fn a_curve_mod(h: &mut Harness) {
    h.shell.set_device_types(bl_devices::device_types());
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let t = bl_devices::modulator::curve_mod_type();
    h.shell
        .run(
            "device.add",
            json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

/// A Compressor, whose display has views, on the selected audio track.
fn a_squeeze(h: &mut Harness) {
    h.shell.set_device_types(bl_devices::device_types());
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let types = bl_devices::device_types();
    let t = types
        .iter()
        .find(|t| t.name == "Compressor")
        .expect("the compressor is offered");
    h.shell
        .run(
            "device.add",
            json!({ "device": {
                "kind": t.kind, "name": t.name, "role": t.role, "sidechain": t.sidechain,
            } }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

/// The framework's test effects on audio track 3, the selected track, no
/// device selected: a device with an expanded part, so it has the expand
/// toggle (WP-5.42).
fn an_expandable_device(h: &mut Harness) {
    h.shell
        .set_device_types(bl_devices::testing::device_types());
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let t = bl_devices::testing::effects_type();
    h.shell
        .run(
            "device.add",
            json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

/// A multisampler with a layer on a MIDI track, selected, its zone editor
/// open (WP-5.13).
fn a_multisampler(h: &mut Harness) {
    h.shell.set_device_types(bl_devices::device_types());
    let midi = h
        .shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == bl_model::TrackKind::Midi)
        .map(bl_model::Track::id)
        .unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [midi] }))
        .unwrap();
    let t = bl_devices::multisampler_type();
    let added = h
        .shell
        .run(
            "device.add",
            json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
        )
        .unwrap();
    let dir = std::env::temp_dir().join(format!("bl-access-multi-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // The second layer's name is its own: the first one names the device's
    // title.
    let files: Vec<String> = ["Alpha", "Tone"]
        .iter()
        .map(|name| {
            let file = dir.join(format!("{name}.wav"));
            std::fs::write(
                &file,
                bl_media::wav_bytes(
                    &[0.5; 4800],
                    1,
                    48_000,
                    bl_media::BitDepth::Int16,
                    bl_media::Dither::None,
                )
                .unwrap(),
            )
            .unwrap();
            file.to_string_lossy().into_owned()
        })
        .collect();
    h.shell
        .run(
            "device.add_layers",
            json!({ "device": added["device"], "paths": files }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": added["device"] }))
        .unwrap();
    h.shell
        .run(
            "device.set_multisampler_view",
            json!({ "device": added["device"], "zone_editor": true }),
        )
        .unwrap();
}

/// The keyboard in the Device View, as a click there leaves it.
fn device_view(h: &mut Harness) {
    h.shell.set_focused_area(bl_shell::Area::Detail);
    h.ctx.memory_mut(|m| {
        if let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
    });
}

/// The list of the set's plug-ins, open in the detail area, with the
/// keyboard in it (IMP-29, WP-6.41).
fn plugin_list(h: &mut Harness) {
    h.shell
        .run("view.toggle_plugin_list", json!({}))
        .expect("the list opens");
    h.shell.set_focused_area(bl_shell::Area::Detail);
    h.ctx.memory_mut(|m| {
        if let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
    });
}

/// A Simpler on the first MIDI track, that track selected, no device selected
/// (WP-5.15b): the Warp controls show in the Device View.
fn a_snapshot(h: &mut Harness) {
    h.shell.set_device_types(bl_devices::device_types());
    let midi = h
        .shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == bl_model::TrackKind::Midi)
        .map(bl_model::Track::id)
        .unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [midi] }))
        .unwrap();
    let t = bl_devices::sampler_type();
    h.shell
        .run(
            "device.add",
            json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

fn control_bar(h: &mut Harness) {
    h.key(Key::Num0, Modifiers::ALT);
}

/// The assistant's pane open.
fn assistant_open(h: &mut Harness) {
    h.shell.run("view.toggle_assistant", json!({})).unwrap();
}

/// The pane open with a message sent and no turn taking it, so Stop shows.
fn assistant_working(h: &mut Harness) {
    assistant_open(h);
    h.shell
        .run("assistant.send", json!({ "text": "Hello" }))
        .unwrap();
}

fn browser_search(h: &mut Harness) {
    h.key(Key::F, command());
}

fn server_on(h: &mut Harness) {
    h.shell
        .run("agent.set_server", json!({ "on": true }))
        .unwrap();
}

fn an_agent(h: &mut Harness) {
    h.shell
        .run("agent.approve_client", json!({ "client": "Tester" }))
        .unwrap();
}

fn server_and_agent(h: &mut Harness) {
    server_on(h);
    an_agent(h);
}

/// An assistant API key stored in the headless keychain, so the Agents
/// page shows its Forget button.
fn assistant_key_set(h: &mut Harness) {
    h.shell
        .run("assistant.set_key", json!({ "key": "sk-headless-test" }))
        .unwrap();
}

/// Every command without a key or a menu item, and how it's reached.
fn paths() -> BTreeMap<&'static str, Path> {
    use Path::{Control, Exception};
    use Reason::{AgentOnly, Equivalent, Gap, LaterView, Live, Query, Unobserved};
    let main = |setup: fn(&mut Harness), start: fn(&mut Harness), label, key| Control {
        window: Window::Main,
        page: "",
        setup,
        start,
        label,
        key,
    };
    let settings = |page, setup: fn(&mut Harness), label, key| Control {
        window: Window::Settings,
        page,
        setup,
        start: nothing,
        label,
        key,
    };
    let clip_view = LaterView("the Clip View (milestone M2)");
    // The Clip View's note editing (WP-2.3): the pointer reaches each of
    // these — a click, a marquee, and drags in the note editor — and their
    // keys (shortcuts.md, `note-editor` and `note-selection`) come with a
    // later package.
    let note_editor = LaterView("the note editor's keys (the note-editor shortcuts, milestone M2)");
    let clip_launch = LaterView("the Clip View's Launch panel (WP-3.9)");
    let audio_panel = LaterView("the Audio Utilities panel's controls (WP-4.10b)");
    let clip_title_bar = LaterView("the audio clip's title bar Save button (WP-4.10b)");
    let auto_warp = LaterView(
        "the Sample Editor's context menu's Set 1.1.1 Here and Warp items, drawn disabled \
         until they run auto-warp's commands",
    );
    // The Envelopes tab (WP-3.16): the pointer draws, moves, curves and
    // deletes breakpoints, and the keys for the breakpoints come with a
    // later package; its choosers and Linked switch are reached by Tab.
    let clip_envelopes =
        LaterView("the Envelopes tab's keys for breakpoints (the clip envelopes package)");
    let device_params = Equivalent(
        "a described device's parameter controls in the Device View: Tab to one, then Up and \
         Down step it and Delete sets its default (the device framework spec, R17)",
    );
    let device_menu = Equivalent(
        "the device's context menu, from its Show Options button (Enter, then the arrows), as \
         in Live",
    );
    let track_menu = Equivalent(
        "the track's context menu, opened from its title bar with the context menu key \
         (S-CTX-002), then the arrows and Enter, as in Live",
    );
    let metronome_menu = Equivalent(
        "the Metronome settings menu, from its pop-up arrow or context menu, as in Live \
         (manual 19.4.1)",
    );
    let recording =
        LaterView("the Record button and transport recording (milestone M2, S-TRANS-006)");
    let editing = Equivalent(
        "the Arrangement's editing keys run it: paste, Delete, Left and Right, and Enter then \
         Left or Right (arrangement-editing R12)",
    );
    BTreeMap::from([
        // The Control Bar's transport (control-bar-transport R11).
        (
            "transport.play",
            main(nothing, control_bar, "Play", Key::Enter),
        ),
        (
            "transport.stop",
            main(nothing, control_bar, "Stop", Key::Enter),
        ),
        (
            "transport.set_loop_enabled",
            main(nothing, control_bar, "Arrangement Loop", Key::Enter),
        ),
        (
            "transport.set_punch_in",
            main(nothing, control_bar, "Punch-In", Key::Enter),
        ),
        (
            "transport.set_punch_out",
            main(nothing, control_bar, "Punch-Out", Key::Enter),
        ),
        (
            "transport.set_metronome_count_in",
            Exception(metronome_menu),
        ),
        ("transport.set_metronome_sound", Exception(metronome_menu)),
        ("transport.set_metronome_rhythm", Exception(metronome_menu)),
        ("transport.set_recording", Exception(recording)),
        ("transport.stop_recording", Exception(recording)),
        // Tempo breakpoints and time-signature markers (WP-2.20): the
        // arrangement's tempo envelope and marker lane aren't drawn yet, so
        // agents reach these through the agent API until they are.
        (
            "song.set_tempo_breakpoint",
            Exception(LaterView("the arrangement's tempo envelope (WP-2.20)")),
        ),
        (
            "song.move_tempo_breakpoint",
            Exception(LaterView("the arrangement's tempo envelope (WP-2.20)")),
        ),
        (
            "song.delete_tempo_breakpoint",
            Exception(LaterView("the arrangement's tempo envelope (WP-2.20)")),
        ),
        (
            "song.set_time_signature_change",
            Exception(LaterView(
                "the arrangement's time-signature marker lane (WP-2.20)",
            )),
        ),
        (
            "song.move_time_signature_change",
            Exception(LaterView(
                "the arrangement's time-signature marker lane (WP-2.20)",
            )),
        ),
        (
            "song.delete_time_signature_change",
            Exception(LaterView(
                "the arrangement's time-signature marker lane (WP-2.20)",
            )),
        ),
        // Automation (WP-2.12): the Control Bar's Re-Enable Automation
        // button, and the arrangement's automation lanes, which aren't
        // drawn yet.
        (
            "automation.re_enable",
            main(nothing, control_bar, "Re-Enable Automation", Key::Enter),
        ),
        (
            "arrangement.automation_arm",
            main(nothing, control_bar, "Automation Arm", Key::Enter),
        ),
        (
            "automation.set_breakpoint",
            Exception(LaterView("the arrangement's automation lanes (WP-2.12)")),
        ),
        (
            "automation.set_curve",
            Exception(LaterView("the arrangement's automation lanes (WP-2.12)")),
        ),
        (
            "automation.move_breakpoint",
            Exception(LaterView("the arrangement's automation lanes (WP-2.12)")),
        ),
        (
            "automation.delete_breakpoint",
            Exception(LaterView("the arrangement's automation lanes (WP-2.12)")),
        ),
        (
            "automation.delete",
            Exception(LaterView("the arrangement's automation lanes (WP-2.12)")),
        ),
        (
            "automation.re_enable_for",
            Exception(LaterView("the arrangement's automation lanes (WP-2.12)")),
        ),
        // A track's switches (mixer-view R7): Enter on Solo or Arm.
        (
            "mixer.solo_exclusive",
            main(nothing, arrangement_track_3, "Solo", Key::Enter),
        ),
        (
            "mixer.arm_exclusive",
            main(nothing, arrangement_track_3, "Arm Recording", Key::Enter),
        ),
        // Typing in the browser's search field (browser R5).
        (
            "browser.search",
            main(nothing, browser_search, "Search Field", Key::K),
        ),
        // The Settings window's controls (Tab, then Enter or Up and Down).
        (
            "agent.set_server",
            settings("agents", nothing, "Agent Server", Key::Enter),
        ),
        (
            "agent.new_token",
            settings("agents", server_on, "New Token", Key::Enter),
        ),
        (
            "agent.set_autonomous",
            settings("agents", an_agent, "Autonomous: Tester", Key::Enter),
        ),
        (
            "agent.forget_client",
            settings("agents", an_agent, "Forget Tester", Key::Enter),
        ),
        (
            "agent.new_client_token",
            settings("agents", server_and_agent, "New Token: Tester", Key::Enter),
        ),
        // The built-in assistant's settings on the Agents page (WP-2.23,
        // part 4).
        (
            "assistant.set_provider",
            settings("agents", nothing, "Assistant Provider", Key::ArrowDown),
        ),
        (
            "assistant.set_base_url",
            settings("agents", nothing, "Assistant Base URL", Key::K),
        ),
        (
            "assistant.set_model",
            settings("agents", nothing, "Assistant Model", Key::K),
        ),
        (
            "assistant.set_key",
            Exception(Equivalent(
                "the API key field, typed into and then stored with Set: the key has no one \
                 key of its own, since it changes the keychain",
            )),
        ),
        (
            "assistant.forget_key",
            settings(
                "agents",
                assistant_key_set,
                "Forget Assistant API Key",
                Key::Enter,
            ),
        ),
        (
            "assistant.toggle_activity_log",
            settings("agents", nothing, "Open Assistant Activity Log", Key::Enter),
        ),
        // The built-in assistant's panel (WP-2.23, part 3; D24): the pane's
        // controls, reached from the Control Bar with Tab.
        (
            "assistant.toggle_floating",
            main(assistant_open, nothing, "Float Assistant", Key::Enter),
        ),
        (
            "assistant.stop",
            main(assistant_working, nothing, "Stop Assistant", Key::Enter),
        ),
        (
            "assistant.set_panel_width",
            main(assistant_open, nothing, "Assistant Width", Key::ArrowLeft),
        ),
        (
            "assistant.send",
            Exception(Equivalent(
                "the prompt field, typed into and then sent with Enter, or with the Send button                  after it: sending needs a message, so there is no one key",
            )),
        ),
        (
            "audio.configure",
            settings("audio", nothing, "Buffer Size", Key::ArrowDown),
        ),
        (
            "audio.set_test_tone",
            settings("audio", nothing, "Test Tone", Key::Enter),
        ),
        (
            "settings.set_theme",
            settings("theme-colors", nothing, "Theme", Key::ArrowDown),
        ),
        (
            "settings.set_create_fades_on_clip_edges",
            settings(
                "record-warp-launch",
                nothing,
                "Create Fades on Clip Edges",
                Key::Enter,
            ),
        ),
        (
            "settings.set_temporary_folder",
            settings("file-folder", nothing, "Browse", Key::Enter),
        ),
        (
            "settings.set_warp_recorded_audio",
            settings(
                "record-warp-launch",
                nothing,
                "Warp recorded audio",
                Key::Enter,
            ),
        ),
        (
            "settings.set_loop_warp_short_samples",
            settings(
                "record-warp-launch",
                nothing,
                "Loop/Warp Short Samples",
                Key::ArrowUp,
            ),
        ),
        (
            "settings.set_auto_warp_long_samples",
            settings(
                "record-warp-launch",
                nothing,
                "Auto-Warp Long Samples",
                Key::Enter,
            ),
        ),
        (
            "settings.set_default_warp_mode",
            settings(
                "record-warp-launch",
                nothing,
                "Default Warp Mode",
                Key::ArrowDown,
            ),
        ),
        (
            "settings.set_default_launch_mode",
            settings(
                "record-warp-launch",
                nothing,
                "Default Launch Mode",
                Key::ArrowDown,
            ),
        ),
        (
            "settings.set_default_launch_quantization",
            settings(
                "record-warp-launch",
                nothing,
                "Default Launch Quantization",
                Key::ArrowDown,
            ),
        ),
        (
            "settings.set_select_on_launch",
            settings(
                "record-warp-launch",
                nothing,
                "Select on Launch",
                Key::Enter,
            ),
        ),
        (
            "settings.set_select_next_scene_on_launch",
            settings(
                "record-warp-launch",
                nothing,
                "Select Next Scene on Launch",
                Key::Enter,
            ),
        ),
        (
            "settings.set_start_recording_on_scene_launch",
            settings(
                "record-warp-launch",
                nothing,
                "Start Recording on Scene Launch",
                Key::Enter,
            ),
        ),
        (
            "settings.set_record_session_automation_in",
            settings(
                "record-warp-launch",
                nothing,
                "Record Session automation in",
                Key::ArrowDown,
            ),
        ),
        (
            "settings.set_auto_hide_plug_in_windows",
            settings("plug-ins", nothing, "Auto-Hide Plug-In Windows", Key::Enter),
        ),
        (
            "settings.set_outline_view_in_focus",
            settings(
                "display-input",
                nothing,
                "Outline View in Focus",
                Key::Enter,
            ),
        ),
        (
            "view.set_zoom",
            settings("display-input", nothing, "Zoom", Key::ArrowUp),
        ),
        (
            "midi.set_port",
            settings("tempo-midi", a_midi_port, "In: Keys Track", Key::Enter),
        ),
        // Queries.
        ("arrangement.state", Exception(Query)),
        ("browser.state", Exception(Query)),
        ("mixer_view.state", Exception(Query)),
        ("transport.state", Exception(Query)),
        ("settings.state", Exception(Query)),
        ("view.state", Exception(Query)),
        ("modulation.state", Exception(Query)),
        ("audio.status", Exception(Query)),
        ("audio.devices", Exception(Query)),
        ("midi.ports", Exception(Query)),
        ("set.list_recoverable", Exception(Query)),
        // The File Manager (WP-6.63): its own rows and buttons reach
        // these, and its views open from the File and View menus.
        ("file_manager.state", Exception(Query)),
        ("set.backups", Exception(Query)),
        (
            "file_manager.replace_file",
            Exception(Equivalent("the File Manager's rows (WP-6.63)")),
        ),
        (
            "file_manager.hot_swap",
            Exception(Equivalent("the File Manager's rows (WP-6.63)")),
        ),
        (
            "file_manager.edit_file",
            Exception(Equivalent("the File Manager's rows (WP-6.63)")),
        ),
        (
            "file_manager.delete_unused",
            Exception(Equivalent("the File Manager's rows (WP-6.63)")),
        ),
        (
            "file_manager.set_search_folder",
            Exception(Equivalent("the File Manager's rows (WP-6.63)")),
        ),
        (
            "set.open_backup",
            Exception(Equivalent("File ▸ Open Beatlayer Set Backups (WP-6.63)")),
        ),
        // Agent perception (WP-1.16): how an agent hears and sees the set,
        // which a person does with their own ears and eyes.
        (
            "perception.preview",
            Exception(AgentOnly("an agent's audio preview of a range or track")),
        ),
        (
            "perception.analyze",
            Exception(AgentOnly("an agent's analysis of the set's audio as text")),
        ),
        (
            "perception.capture_view",
            Exception(AgentOnly("an agent's capture of a view as an image")),
        ),
        // The automation lane's choosers (WP-2.12): Tab to the device
        // chooser, then Up or Down steps the parameter the lane shows.
        (
            "arrangement.show_automation",
            main(
                automation_mode_on,
                nothing,
                "Automation Device",
                Key::ArrowDown,
            ),
        ),
        // Agents' forms of gestures and dialogs.
        (
            "arrangement.move_clips",
            Exception(Equivalent(
                "a drag; the keyboard moves clips with Left and Right \
                 (`arrangement.nudge`), or cut and paste",
            )),
        ),
        (
            "clip.stretch",
            Exception(Equivalent(
                "a Shift-drag on a warped clip's edge (S-ARR-006); its length \
                 is reached from the keyboard by `arrangement.resize_clips` \
                 after the Clip View's BPM field has set the clip's tempo \
                 (`clip.set_segment_bpm`)",
            )),
        ),
        (
            "clip.slide_contents",
            Exception(Equivalent(
                "a Shift+Option-drag on a clip's contents (S-ARR-005); the \
                 same contents are reached from the keyboard by the Clip \
                 View's Start and loop fields (`clip.set_region`, \
                 `clip.set_loop`)",
            )),
        ),
        (
            "song.unfollow_tempo_automation",
            Exception(Equivalent(
                "the tempo field's context menu (the Context Menu key or \
                 Shift-F10 with the field focused)",
            )),
        ),
        (
            "arrangement.select_clips",
            Exception(Equivalent(
                "a click; the keyboard selects with Shift and the arrows \
                 (`arrangement.select_time`) and Cmd-A (Ctrl-A)",
            )),
        ),
        (
            "arrangement.scroll",
            Exception(Equivalent(
                "the wheel; the view follows the tracks the keys select and the playhead \
                 (Follow), and the zoom keys change what it shows",
            )),
        ),
        (
            "arrangement.show_range",
            Exception(Equivalent(
                "an agent's view move; a person zooms with Z, X, and the zoom keys",
            )),
        ),
        (
            "arrangement.show_track",
            Exception(Equivalent(
                "an agent's view move; the view follows the track the keys select",
            )),
        ),
        (
            "mixer_view.scroll",
            Exception(Equivalent(
                "the wheel; Navigate ▸ Mixer, Left and Right, and the neighbour keys bring the \
                 focused column into view",
            )),
        ),
        // The fold button in a track's title bar (racks spec, R27): Live
        // names the button (25.8) but its key isn't observed yet, and its
        // place in the Tab order with it (the racks spec's open question
        // 19).
        (
            "mixer_view.fold_chains",
            Exception(Unobserved(
                "the fold button in the track's title bar: Live's own key for it isn't observed \
                 yet, and its place in the mixers' Tab order with it",
            )),
        ),
        // The Session View's grid (WP-3.7): the pointer's selection, drags,
        // drops, and the key the view reads itself.
        ("session.state", Exception(Query)),
        (
            "session.scroll",
            Exception(Equivalent(
                "the wheel; the arrows and Page Up and Page Down bring the scenes into view",
            )),
        ),
        (
            "session.delete",
            Exception(Equivalent(
                "the Delete key with the grid focused; the slot and scene context menus arrive \
                 with the Session View's menus",
            )),
        ),
        (
            "session.copy_clip_to_slot",
            Exception(Equivalent(
                "Option-dragging (Ctrl-dragging) a clip to another slot; dragging moves it \
                 (`clip.move_to_slot`)",
            )),
        ),
        (
            "session.drop_files",
            Exception(Equivalent(
                "dropping files from the browser, Finder, or Explorer on a slot",
            )),
        ),
        (
            "session.duplicate_scene",
            Exception(Equivalent(
                "the scene's context menu, Edit ▸ Duplicate (the menu arrives with a later \
                 package)",
            )),
        ),
        // The Session View's launching (WP-3.2): the launch button on a slot
        // with a clip, the scene launch buttons, and the status row arrive
        // with the launching UI (WP-3.8); the commands are the engine's.
        (
            "session.launch",
            Exception(LaterView(
                "the Session View's launch buttons and status row (WP-3.8)",
            )),
        ),
        (
            "session.release",
            Exception(LaterView(
                "the Session View's launch buttons, which send the release (WP-3.8)",
            )),
        ),
        (
            "session.launch_scene",
            Exception(LaterView(
                "the Session View's scene launch buttons (WP-3.8)",
            )),
        ),
        (
            "session.stop_track",
            Exception(LaterView(
                "the Session View's stop buttons and status row (WP-3.8)",
            )),
        ),
        (
            "session.stop_all",
            Exception(LaterView(
                "the Session View's Stop All Clips button (WP-3.8)",
            )),
        ),
        (
            "session.back_to_arrangement_track",
            Exception(LaterView(
                "the Arrangement's per-track Back to Arrangement buttons (WP-3.8)",
            )),
        ),
        // Session recording (WP-3.11): Session Record is on a shortcut and the
        // Playback menu; the slots' record buttons arrive with the launching
        // UI.
        (
            "session.record_slot",
            Exception(LaterView("the Session View's Clip Record buttons (WP-3.8)")),
        ),
        (
            "session.overdub",
            Exception(Equivalent(
                "Session Record (`session.record`) overdubs the MIDI clip that plays",
            )),
        ),
        (
            "session.stop_recording",
            Exception(Equivalent(
                "Session Record again (`session.record`) ends the recording",
            )),
        ),
        (
            "browser.add_to_arrangement",
            Exception(Equivalent(
                "a drag; Enter adds the selected files (`browser.load_selected`)",
            )),
        ),
        (
            "browser.stop_preview",
            Exception(Equivalent(
                "selecting a folder, or switching Options ▸ Browser File Preview off, stops a \
                 preview",
            )),
        ),
        (
            "browser.scrub_preview",
            Exception(Live(
                "clicking in the browser's preview waveform is the pointer's way; Live has no \
                 keyboard shortcut for scrubbing a preview",
            )),
        ),
        (
            "browser.set_raw",
            Exception(Live(
                "clicking the Raw button in the browser's preview strip; Live has no keyboard \
                 shortcut for the Raw switch",
            )),
        ),
        (
            "set.discard_changes",
            Exception(Equivalent(
                "the save-changes dialog's Don't Save, the system's dialog",
            )),
        ),
        (
            "set.relink_missing",
            Exception(Equivalent(
                "the File Manager's Automatic Search button, which needs the File Manager (M-FILE-007); \
                 it also runs by itself when a set with missing files is opened",
            )),
        ),
        (
            "agent.approve_client",
            Exception(Equivalent(
                "the system dialog that asks when an agent first connects, Allow",
            )),
        ),
        ("clip.create", Exception(editing)),
        (
            "track.add_take_lane",
            Exception(LaterView(
                "the take lanes' view and comping (milestone M6); loop recording makes the lanes",
            )),
        ),
        ("clip.delete", Exception(editing)),
        ("clip.move", Exception(editing)),
        ("clip.move_by", Exception(editing)),
        ("clip.set_length", Exception(editing)),
        ("clip.trim_start", Exception(editing)),
        // Views of later milestones.
        ("clip.set_gain", Exception(clip_view)),
        ("clip.set_launch_mode", Exception(clip_view)),
        ("clip.set_loop", Exception(clip_view)),
        ("clip.set_loop_enabled", Exception(clip_view)),
        ("clip.set_midi_loop", Exception(clip_view)),
        ("clip.set_midi_region", Exception(clip_view)),
        ("clip.set_data", Exception(clip_view)),
        // The crop's document-level commands (WP-4.13): `clip.crop` is the
        // one a person and an agent run, from the menus and Cmd-Shift-J, and
        // it applies these.
        (
            "clip.crop_region",
            Exception(Equivalent(
                "clip.crop, from the clip's and time selection's menus and Cmd-Shift-J (audio-clip-editing R14)",
            )),
        ),
        (
            "clip.crop_span",
            Exception(Equivalent(
                "clip.crop, from the Sample Editor's menu (audio-clip-editing R16)",
            )),
        ),
        // The reversal's commands (WP-4.7): `clip.reverse` is the one a
        // person and an agent run, from the Audio Utilities panel's Reverse
        // button, and it applies `clip.reverse_file`.
        ("clip.reverse", Exception(audio_panel)),
        (
            "clip.reverse_file",
            Exception(Equivalent(
                "clip.reverse, from the Audio Utilities panel's Reverse button (audio-clip-editing R7)",
            )),
        ),
        // The clip's pitch (WP-4.7): the Audio Utilities panel's Transpose
        // and Detune fields.
        ("clip.set_pitch", Exception(audio_panel)),
        // The freeze's document-level commands (WP-4.14): `track.freeze` and
        // `track.unfreeze` are the ones a person and an agent run, from the
        // menus and Cmd-Option-Shift-F, and they apply these.
        (
            "track.set_frozen",
            Exception(Equivalent(
                "track.freeze, from the Edit menu, the track and clip menus, and Cmd-Option-Shift-F (freeze-and-bounce R13)",
            )),
        ),
        (
            "track.clear_frozen",
            Exception(Equivalent(
                "track.unfreeze, the same item read as Unfreeze Track (freeze-and-bounce R10, R13)",
            )),
        ),
        (
            "track.unfreeze",
            Exception(Equivalent(
                "the Freeze Track item, which reads Unfreeze Track and unfreezes a selection that is frozen (freeze-and-bounce R1, R10)",
            )),
        ),
        ("clip.set_envelope_breakpoint", Exception(clip_envelopes)),
        ("clip.set_envelope_curve", Exception(clip_envelopes)),
        ("clip.move_envelope_breakpoint", Exception(clip_envelopes)),
        ("clip.delete_envelope_breakpoint", Exception(clip_envelopes)),
        ("clip.delete_envelope", Exception(clip_envelopes)),
        (
            "clip.set_envelope_linked",
            main(
                envelopes_tab_with_an_envelope,
                device_view,
                "Linked",
                Key::Enter,
            ),
        ),
        (
            "view.clip_envelope",
            main(
                envelopes_tab,
                device_view,
                "Envelope Control",
                Key::ArrowDown,
            ),
        ),
        (
            "clip.set_follow_actions",
            Exception(Unobserved(
                "the follow controls in the Launch Properties panel, which Tab reaches; their place in Live's Tab order",
            )),
        ),
        ("clip.edit_sample", Exception(audio_panel)),
        (
            "clip.replace_sample",
            Exception(Unobserved(
                "a file dropped from the browser on the Clip View replaces the clip's sample (the \
             sample editor spec, R18)",
            )),
        ),
        (
            "settings.set_sample_editor",
            Exception(LaterView(
                "the File & Folder page's Sample Editor Browse button, the second Browse on the \
                 page (WP-4.10b)",
            )),
        ),
        ("clip.set_warp", Exception(audio_panel)),
        ("clip.set_warp_mode", Exception(audio_panel)),
        ("clip.set_warp_settings", Exception(audio_panel)),
        ("clip.set_ram_mode", Exception(audio_panel)),
        ("clip.set_tempo_leader", Exception(audio_panel)),
        ("clip.set_high_quality", Exception(audio_panel)),
        (
            "clip.insert_warp_markers",
            Exception(Equivalent(
                "Insert Warp Marker (Cmd+I, S-SAMPLE-009) with a time selection in the Sample \
                 Editor, which puts a marker at each transient in it (the sample editor spec, \
                 R10)",
            )),
        ),
        (
            "clip.reset_transients",
            Exception(Equivalent(
                "the Sample Editor's context menu, opened from the keyboard with Shift+F10 \
                 (Ctrl+Option+Shift+M on macOS): its Reset Transients item",
            )),
        ),
        (
            "clip.move_transient",
            Exception(Live(
                "the manual's Sample Editor keys (42.11) have none for moving a transient; \
                 Shift-dragging a pseudo-warp marker is the mouse's way",
            )),
        ),
        ("clip.transients", Exception(Query)),
        (
            "clip.shift_warp_marker",
            Exception(Live(
                "the manual's Sample Editor keys (42.11) have none for moving the waveform \
                 under a marker; Shift-dragging the marker is the mouse's way",
            )),
        ),
        ("clip.set_segment_bpm", Exception(audio_panel)),
        ("clip.scale_tempo", Exception(audio_panel)),
        ("clip_view.sample_editor", Exception(Query)),
        (
            "clip_view.select_transients",
            Exception(Equivalent(
                "an agent's form of clicking a transient's pseudo-warp marker; Shift+Left and \
                 Shift+Right select a span, and Delete Transient (Cmd+Shift+Delete) acts on the \
                 transients in it",
            )),
        ),
        (
            "browser.show_file",
            Exception(Equivalent(
                "the Sample Editor's context menu, opened from the keyboard with Shift+F10 \
                 (Ctrl+Option+Shift+M on macOS): its Show in Browser item",
            )),
        ),
        ("clip.set_1_1_1_here", Exception(auto_warp)),
        ("clip.warp_from_here", Exception(auto_warp)),
        ("clip.warp_as_loop", Exception(auto_warp)),
        ("clip.warp_selection_as_loop", Exception(auto_warp)),
        ("clip.save_default", Exception(clip_title_bar)),
        ("clip.defaults", Exception(clip_title_bar)),
        ("clip.set_legato", Exception(clip_launch)),
        ("clip.set_launch_quantization", Exception(clip_launch)),
        ("clip.set_velocity_amount", Exception(clip_launch)),
        ("clip_view.select_notes", Exception(note_editor)),
        ("note.add", Exception(note_editor)),
        ("note.delete", Exception(note_editor)),
        // `note.move` has its own path now: the note-selection keys
        // transpose the selected notes (S-A11Y-008, S-A11Y-009).
        ("note.resize", Exception(note_editor)),
        ("note.set_value", Exception(note_editor)),
        ("note.ramp_value", Exception(note_editor)),
        ("view.lane", Exception(note_editor)),
        // The list of the set's plug-ins (IMP-29, WP-6.41): Enter on its
        // Show Only Missing switch.
        (
            "view.set_plugin_list_only_missing",
            main(nothing, plugin_list, "Show Only Missing", Key::Enter),
        ),
        // The Device View (WP-2.4): Enter on a title bar's controls.
        (
            "device.select",
            main(a_device, device_view, "Test Gain", Key::Enter),
        ),
        (
            "device.set_enabled",
            main(a_device, device_view, "Device Activator", Key::Enter),
        ),
        (
            "device.save_preset",
            main(a_device, device_view, "Save Preset", Key::Enter),
        ),
        (
            "device.expand",
            main(an_expandable_device, device_view, "Expand", Key::Enter),
        ),
        ("device.set_show_preset_name", Exception(device_menu)),
        ("device.set_role", Exception(device_menu)),
        ("device.save_default_preset", Exception(device_menu)),
        ("device.swap_to_snapshot", Exception(device_menu)),
        ("track.set_devices_enabled", Exception(track_menu)),
        (
            "device.load_preset",
            Exception(Equivalent(
                "Enter on a preset in the browser while hot-swapping (`browser.load_selected`)",
            )),
        ),
        (
            "device.set_state",
            Exception(Equivalent("loading a preset (`device.load_preset`)")),
        ),
        (
            "device.move",
            Exception(Equivalent(
                "Cmd-X on the selected device, then Cmd-V before another (`device.cut`, \
                 `device.paste`)",
            )),
        ),
        (
            "device.add",
            Exception(LaterView(
                "the browser's device labels, once Beatlayer has devices (WP-2.5)",
            )),
        ),
        (
            "device.insert",
            Exception(Equivalent(
                "adding a device as a person does (`device.add`)",
            )),
        ),
        ("device.set_param", Exception(device_params)),
        // Racks (WP-5.1): Group and Ungroup are on Cmd-G and Cmd-Shift-G;
        // the rack's chain list and its controls arrive with the Device
        // View's rack (WP-5.2), until then agents reach them.
        (
            "device.wrap_in_rack",
            Exception(Equivalent("Group (`device.group`, Cmd-G)")),
        ),
        (
            "device.unwrap_rack",
            Exception(Equivalent("Ungroup (`device.ungroup`, Cmd-Shift-G)")),
        ),
        (
            "device.insert_in_chain",
            Exception(Equivalent(
                "adding a device as a person does (`device.add`)",
            )),
        ),
        (
            "device.move_to_chain",
            Exception(Equivalent(
                "Cmd-X on the selected device, then Cmd-V before another (`device.cut`, \
                 `device.paste`)",
            )),
        ),
        (
            "device.set_enabled_all",
            Exception(Equivalent(
                "Option-Enter (Alt-Enter) on the rack's Device Activator, as Option-click does \
                 (I-CLICK-024)",
            )),
        ),
        // The rack's frame (WP-5.2): the chain list's rows and their
        // controls are in the Device View's Tab order, and its keys are
        // the rack-chains context's.
        (
            "chain.add",
            Exception(Equivalent(
                "the chain list's drop area (`chain.create`), or Paste there (`chain.paste`, \
                 Cmd-V)",
            )),
        ),
        (
            "chain.create",
            Exception(Equivalent(
                "a drag onto the chain list's drop area; the keyboard pastes a chain \
                 (`chain.paste`, Cmd-V)",
            )),
        ),
        (
            "chain.save_preset",
            Exception(Gap(
                "a drag of the chain's row into the browser has no keyboard form yet (racks R46)",
            )),
        ),
        (
            "chain.load_preset",
            Exception(Equivalent(
                "Enter on a chain preset in the browser (`browser.load_selected`)",
            )),
        ),
        (
            "device.add_preset",
            Exception(Equivalent(
                "Enter on a preset in the browser (`browser.load_selected`)",
            )),
        ),
        (
            "device.replace",
            Exception(Equivalent(
                "Enter on a preset or device in the browser while hot-swapping \
                 (`browser.load_selected`)",
            )),
        ),
        (
            "chain.drop_on_track",
            Exception(Equivalent(
                "a drag onto the track's own chain; the keyboard cuts the chain's devices and \
                 pastes them (`device.cut`, `device.paste`)",
            )),
        ),
        (
            "chain.remove",
            Exception(Equivalent("Delete in the chain list (`chain.delete`)")),
        ),
        (
            "chain.move",
            Exception(Equivalent(
                "a drag in the chain list; the keyboard cuts the chain and pastes it \
                 (`chain.cut`, `chain.paste`)",
            )),
        ),
        (
            "chain.set_colour",
            Exception(Equivalent(
                "the chain's context menu, from its row with Shift-F10 (S-CTX-002)",
            )),
        ),
        (
            "chain.set_info",
            Exception(LaterView(
                "the chain's Edit Info Text, which waits for the Info View (WP-2.24)",
            )),
        ),
        (
            "chain.set_gain",
            main(a_rack, device_view, "Chain Volume of Pad", Key::ArrowUp),
        ),
        (
            "chain.set_pan",
            main(a_rack, device_view, "Chain Pan of Pad", Key::ArrowUp),
        ),
        (
            "chain.set_enabled",
            main(a_rack, device_view, "Chain Activator of Pad", Key::Enter),
        ),
        (
            "chain.set_solo",
            Exception(Equivalent(
                "the chain's Solo button, or S in the chain list (`chain.solo`)",
            )),
        ),
        (
            "chain.set_zone",
            main(
                a_rack_with_the_zone_editor,
                device_view,
                "Pad 0 to 0, body",
                Key::ArrowRight,
            ),
        ),
        (
            "rack.set_zone_editor",
            main(a_rack, device_view, "Chain", Key::Enter),
        ),
        (
            "rack.set_auto_select",
            main(a_rack, device_view, "Auto Select", Key::Enter),
        ),
        // Spec: macros R14, R15 and R19. The Macro Controls' buttons, the
        // macros' knobs, and their context menus reach these with the
        // pointer; their keys wait for a later package.
        (
            "rack.set_macro_count",
            Exception(Equivalent("Show More and Show Fewer")),
        ),
        (
            "rack.map_mode",
            Exception(Equivalent("the macro view's Map switch")),
        ),
        (
            "rack.select_macro",
            Exception(Equivalent("a click on a macro's knob")),
        ),
        (
            "rack.map_macro",
            Exception(Equivalent("Map mode's Map button")),
        ),
        (
            "rack.set_mapping_range",
            Exception(Equivalent("the mappings list's Min and Max boxes")),
        ),
        (
            "rack.invert_mapping",
            Exception(Equivalent("the mappings list's Invert Range")),
        ),
        (
            "rack.unmap_macro",
            Exception(Equivalent("the mappings list's Delete")),
        ),
        (
            "rack.randomize_macros",
            Exception(Equivalent("the Rand button")),
        ),
        (
            "rack.rename_macro",
            Exception(Equivalent("Rename on a macro's context menu")),
        ),
        (
            "rack.set_macro_colour",
            Exception(Equivalent("Colour on a macro's context menu")),
        ),
        (
            "rack.set_macro_info_text",
            Exception(Gap(
                "a macro's info text has no UI yet; M-EDIT-025 (Edit Info Text) adds the menu \
                 item",
            )),
        ),
        (
            "rack.set_macro_randomize_excluded",
            Exception(Equivalent(
                "Exclude Macro from Randomization on a macro's context menu",
            )),
        ),
        (
            "rack.set_macro_style",
            Exception(Equivalent("Macro Style on a macro's context menu")),
        ),
        (
            "rack.set_mapped_value",
            Exception(Equivalent("the macro commands' own step")),
        ),
        (
            "rack.macros",
            Exception(Equivalent(
                "what the Macro Controls and the mappings list show",
            )),
        ),
        // Spec: macros R25 to R30 and R32. The Macro Variations view's
        // buttons, its rows and the macro's context menu reach these with the
        // pointer; Rename, Duplicate and Delete also take Cmd+R, Cmd+D and the
        // Delete key through the device's own commands.
        (
            "rack.store_variation",
            Exception(Equivalent("the variations view's New button")),
        ),
        (
            "rack.launch_variation",
            Exception(Equivalent("a variation row's Launch button")),
        ),
        (
            "rack.overwrite_variation",
            Exception(Equivalent("a variation row's Overwrite button")),
        ),
        (
            "rack.rename_variation",
            Exception(Equivalent("Rename on a variation row")),
        ),
        (
            "rack.duplicate_variation",
            Exception(Equivalent("Duplicate on a variation row")),
        ),
        (
            "rack.delete_variation",
            Exception(Equivalent("Delete on a variation row")),
        ),
        (
            "rack.set_macro_variation_excluded",
            Exception(Equivalent(
                "Exclude Macro From Variations on a macro's context menu",
            )),
        ),
        (
            "rack.show_variations",
            Exception(Equivalent("the view column's Show/Hide Macro Variations")),
        ),
        (
            "rack.select_variation",
            Exception(Equivalent("a click on a variation's row")),
        ),
        (
            "chain.select",
            main(a_rack, device_view, "Chain Pad", Key::Enter),
        ),
        (
            "chain.hot_swap",
            main(a_rack, device_view, "Hot-Swap of Pad", Key::Enter),
        ),
        // WP-5.6: a drum rack's pad view and the chain list's columns.
        (
            "pad.select",
            main(a_drum_rack, device_view, "Pad C1: Gain", Key::Enter),
        ),
        (
            "rack.move_pads",
            main(a_drum_rack, device_view, "Pad C1: Gain", Key::ArrowDown),
        ),
        (
            "pad.set_mute",
            main(a_drum_rack, device_view, "Mute of Pad C1: Gain", Key::Enter),
        ),
        (
            "pad.set_solo",
            main(a_drum_rack, device_view, "Solo of Pad C1: Gain", Key::Enter),
        ),
        (
            "pad.preview",
            main(
                a_drum_rack,
                device_view,
                "Preview of Pad C1: Gain",
                Key::Enter,
            ),
        ),
        (
            "pad.hot_swap",
            main(
                a_drum_rack,
                device_view,
                "Hot-Swap of Pad C1: Gain",
                Key::Enter,
            ),
        ),
        (
            "chain.preview",
            main(a_drum_rack, device_view, "Preview of Gain", Key::Enter),
        ),
        (
            "chain.set_receive",
            main(a_drum_rack, device_view, "Receive of Gain", Key::ArrowDown),
        ),
        (
            "chain.set_play",
            main(a_drum_rack, device_view, "Play of Gain", Key::ArrowDown),
        ),
        (
            "chain.set_choke",
            main(a_drum_rack, device_view, "Choke of Gain", Key::ArrowDown),
        ),
        (
            "chain.set_send",
            main(
                a_drum_rack,
                device_view,
                "Send of Gain to Reverb",
                Key::ArrowUp,
            ),
        ),
        (
            "chain.set_audio_to",
            main(
                a_drum_rack,
                device_view,
                "Audio To of Reverb",
                Key::ArrowDown,
            ),
        ),
        (
            "device.group_to_drum_rack",
            Exception(Equivalent(
                "the device's context menu (Group to Drum Rack, C-DEVICE-013), from its Show \
                 Options button (Enter, then the arrows), as in Live",
            )),
        ),
        (
            "pad.hot_swap_sample",
            Exception(Equivalent(
                "the pad's Hot-Swap button (`pad.hot_swap`) and the browser: the Hot-Swap \
                 Sample button shows only while the pointer is over the pad (drum racks R19)",
            )),
        ),
        (
            "pad.drop",
            Exception(Equivalent(
                "a drag from the browser or the desktop onto a pad; the keyboard hot-swaps the \
                 pad (`pad.hot_swap`) and loads from the browser (`browser.load_selected`)",
            )),
        ),
        (
            "pad.swap",
            Exception(Equivalent(
                "a drag from one pad to another; changing a chain's Receive moves it to \
                 another pad (`chain.set_receive`)",
            )),
        ),
        (
            "pad.rename",
            Exception(Equivalent(
                "the pad's context menu, from the pad with Shift-F10 (S-CTX-002)",
            )),
        ),
        (
            "pad.delete",
            Exception(Equivalent(
                "the pad's context menu, from the pad with Shift-F10 (S-CTX-002), or Delete in \
                 the chain list (`chain.delete`)",
            )),
        ),
        (
            "pad.save_default",
            Exception(Equivalent(
                "the pad's context menu, from the pad with Shift-F10 (S-CTX-002)",
            )),
        ),
        (
            "chain.extract",
            Exception(LaterView(
                "the Session mixer's chain strips, from which a chain is dragged out (WP-5.19)",
            )),
        ),
        (
            "chain.extract_return",
            Exception(LaterView(
                "the Session mixer's chain strips, from which a return chain is dragged onto \
                 the returns (WP-5.19)",
            )),
        ),
        (
            "device.load_sample",
            Exception(Gap(
                "loading a sample has no key or menu item yet: dropping the file on the \
                 sampler's Device View frame is the only way, and where Live's own path (its \
                 Hot-Swap control) goes is to observe (sampler R11)",
            )),
        ),
        (
            "device.set_sample",
            Exception(Equivalent("loading a sample (`device.load_sample`)")),
        ),
        // WP-5.10: a modulator's Map and Unmap buttons, the controls it
        // maps while Map is lit (Enter on one), and the Shaper's preset
        // buttons, which draw its shape.
        (
            "modulator.arm_map",
            main(a_wave_mod, device_view, "Map 1", Key::Enter),
        ),
        (
            "modulator.unmap",
            main(a_mapped_wave_mod, device_view, "Unmap 1", Key::Enter),
        ),
        (
            "modulator.map",
            main(a_lit_map_button, device_view, "Gain", Key::Enter),
        ),
        (
            "modulator.set_shape",
            main(a_curve_mod, device_view, "Shape preset 1", Key::Enter),
        ),
        // The drum sampler's slot cells (WP-5.17): Tab to one, then Enter
        // selects the slot.
        (
            "device.select_slot",
            main(a_drum_sampler, device_view, "Slot 2", Key::Enter),
        ),
        // A device's sidechain strip and section (routing R29): Enter on
        // its buttons, Up and Down on its chooser and number boxes.
        (
            "device.show_sidechain",
            main(
                a_sidechained_device,
                device_view,
                "Show/Hide Sidechain",
                Key::Enter,
            ),
        ),
        (
            "device.set_sidechain_listen",
            main(
                a_sidechained_device,
                device_view,
                "Sidechain Listen",
                Key::Enter,
            ),
        ),
        (
            "device.set_sidechain_enabled",
            main(a_sidechained_device, device_view, "External", Key::Enter),
        ),
        (
            "device.set_sidechain_input",
            main(
                a_sidechained_device,
                device_view,
                "Sidechain Source",
                Key::ArrowUp,
            ),
        ),
        (
            "device.set_midi_sidechain_source",
            main(
                a_midi_sidechained_device,
                device_view,
                "MIDI Sidechain Source",
                Key::ArrowUp,
            ),
        ),
        // Down on the external instrument's channel chooser.
        (
            "device.set_external",
            main(
                a_hardware_device,
                device_view,
                "External MIDI Channel",
                Key::ArrowDown,
            ),
        ),
        (
            "device.set_sidechain_mix",
            main(a_sidechained_device, device_view, "Mix", Key::ArrowDown),
        ),
        (
            "device.set_sidechain_gain",
            main(a_sidechained_device, device_view, "SC Gain", Key::ArrowDown),
        ),
        // The multisampler (WP-5.13): the zone editor's header buttons and
        // layer rows, and the Sample tab's fields, are in the Device View's
        // Tab order.
        (
            "device.set_multisampler_view",
            main(a_multisampler, device_view, "Auto", Key::Enter),
        ),
        (
            "device.hot_swap_layer",
            main(a_multisampler, device_view, "Hot-Swap Sample", Key::Enter),
        ),
        (
            "device.select_layers",
            main(a_multisampler, device_view, "Tone", Key::Enter),
        ),
        (
            "device.delete_layers",
            Exception(Gap(
                "the layer list's context menu, which the keyboard doesn't open yet, and the \
                 Delete key, which deletes the selected device (multisampler R11)",
            )),
        ),
        (
            "device.set_layer",
            main(a_multisampler, device_view, "Root", Key::ArrowUp),
        ),
        (
            "device.set_layer_options",
            main(a_multisampler, device_view, "Snap", Key::Enter),
        ),
        (
            "device.insert_layers",
            Exception(Equivalent(
                "`device.add_layers`, which reads the files; this form takes layers whose \
                 samples are already in the set's media",
            )),
        ),
        (
            "device.update_layers",
            Exception(Equivalent(
                "`device.set_layer`, which sets the same settings and snaps the markers",
            )),
        ),
        (
            "device.set_layer_media",
            Exception(Equivalent(
                "`device.set_layer_sample`, which reads the file",
            )),
        ),
        (
            "device.set_layer_sample",
            Exception(Equivalent(
                "Hot-Swap Sample (`device.hot_swap_layer`), then Enter in the browser \
                 (`browser.load_selected`) on the file",
            )),
        ),
        (
            "device.import_sfz",
            Exception(Equivalent(
                "`browser.load_selected`, which Enter runs on a selected SFZ instrument in the \
                 browser and loads as a multisampler (multisampler R47); this form names the \
                 file, the track or the device",
            )),
        ),
        (
            "device.add_layers",
            Exception(Gap(
                "adding samples to the layer list is a drop of files on the zone editor; the \
                 keyboard has no way to name files yet (multisampler R3)",
            )),
        ),
        (
            "device.set_layer_zone",
            Exception(Gap(
                "dragging a zone's edge, body or fade in the zone editor has no keyboard \
                 form yet (multisampler R10)",
            )),
        ),
        // Sample editing in the sampler devices (WP-5.15): the display's
        // context menu, which the keyboard doesn't open yet, and gestures
        // on the waveform.
        (
            "device.edit_sample",
            Exception(Gap(
                "the sample display's context menu, which the keyboard doesn't open yet \
                 (multisampler R55)",
            )),
        ),
        (
            "device.crop_sample",
            Exception(Gap(
                "the sample display's context menu, which the keyboard doesn't open yet \
                 (multisampler R59)",
            )),
        ),
        (
            "device.reverse_sample",
            Exception(Gap(
                "the sample display's context menu, which the keyboard doesn't open yet \
                 (multisampler R59)",
            )),
        ),
        (
            "device.audition_sample",
            Exception(Gap(
                "a click on the sample display, which has no keyboard form yet (multisampler R54)",
            )),
        ),
        (
            "device.convert",
            Exception(Gap(
                "the device's title bar menu, which the keyboard doesn't open yet \
                 (multisampler R62)",
            )),
        ),
        (
            "device.add_sample_edit",
            Exception(Equivalent(
                "`device.edit_sample`, which checks the selection against the sample",
            )),
        ),
        (
            "device.clear_sample_edits",
            Exception(Equivalent(
                "`device.edit_sample` with `revert`, which reads the unedited length itself",
            )),
        ),
        (
            "device.set_processed_layer_sample",
            Exception(Equivalent(
                "`device.crop_sample` and `device.reverse_sample`, which write the copy",
            )),
        ),
        // Manual slicing in the sample display, and the Warp controls
        // (WP-5.15b): the slicing gestures have no keyboard form yet, and
        // the Warp controls are in the Device View's Tab order.
        (
            "device.add_slice",
            Exception(Gap(
                "a double-click on the sample display, which has no keyboard form yet \
                 (sampler R20)",
            )),
        ),
        (
            "device.move_slice",
            Exception(Gap(
                "dragging a slice in the sample display, which has no keyboard form yet \
                 (sampler R20)",
            )),
        ),
        (
            "device.delete_slice",
            Exception(Gap(
                "a double-click on a slice in the sample display, which has no keyboard form yet \
                 (sampler R20)",
            )),
        ),
        (
            "device.set_warp",
            main(a_snapshot, device_view, "Warp", Key::Enter),
        ),
        (
            "device.set_warp_mode",
            main(a_snapshot, device_view, "Warp Mode", Key::Enter),
        ),
        (
            "device.set_warp_settings",
            main(a_snapshot, device_view, "Preserve", Key::Enter),
        ),
        (
            "device.set_sample_bpm",
            main(a_snapshot, device_view, "Sample BPM", Key::ArrowUp),
        ),
        (
            "device.scale_tempo",
            main(a_snapshot, device_view, "Double Tempo", Key::Enter),
        ),
        (
            "device.snap_layer_markers",
            Exception(Gap(
                "the loop marker's context menu, which the keyboard doesn't open yet \
                 (multisampler R20)",
            )),
        ),
        (
            "device.duplicate_layers",
            Exception(Gap(
                "the layer list's context menu, which the keyboard doesn't open yet \
                 (multisampler R11)",
            )),
        ),
        (
            "device.rename_layer",
            Exception(Gap(
                "the layer list's context menu and a double-click, which the keyboard doesn't \
                 reach yet (multisampler R11)",
            )),
        ),
        (
            "device.move_layers",
            Exception(Gap(
                "no control moves layers by hand yet: the list's sorts reorder it (multisampler \
                 R11)",
            )),
        ),
        (
            "device.sort_layers",
            Exception(Gap(
                "the layer list's context menu, which the keyboard doesn't open yet \
                 (multisampler R11)",
            )),
        ),
        (
            "device.distribute_layers",
            Exception(Gap(
                "the layer list's context menu, which the keyboard doesn't open yet \
                 (multisampler R11)",
            )),
        ),
        (
            "device.normalize_layers",
            Exception(Gap(
                "the layer list's and the waveform's context menus, which the keyboard \
                 doesn't open yet (multisampler R11)",
            )),
        ),
        ("device_view.state", Exception(Query)),
        ("device.parameters", Exception(Query)),
        ("device.display", Exception(Query)),
        // A display's view buttons (device framework R38): Enter on one.
        (
            "device.set_display_view",
            main(a_squeeze, device_view, "Activity view", Key::Enter),
        ),
        // Floating device windows (WP-2.5b): Float and Dock are
        // `device.set_floating`, on a key and in the device's menu.
        ("device.windows", Exception(Query)),
        (
            "device.set_window",
            Exception(Equivalent(
                "moving or resizing a floating device's window, which the system's window \
                 keys and commands do",
            )),
        ),
        // Plug-ins (WP-2.6): loaded by path until the browser lists them
        // (WP-2.9); the window's button is in the plug-in device's title
        // bar (WP-2.10, L-DEV-010).
        ("plugin.info", Exception(Query)),
        (
            "plugin.load",
            Exception(LaterView("the browser's plug-ins (WP-2.9)")),
        ),
        // Audio Units (WP-2.8) load by component description until the
        // browser lists them (WP-2.8's second part) and the scanner finds
        // them.
        (
            "plugin.load_au",
            Exception(LaterView("the browser's Audio Units (WP-2.8)")),
        ),
        (
            "plugin.show_window",
            main(
                a_plugin,
                device_view,
                "Show/Hide Plug-In Window",
                Key::Enter,
            ),
        ),
        (
            "plugin.set_window",
            Exception(Equivalent(
                "moving or resizing a plug-in window, which the system's window keys and \
                 commands do, as for a floating device's window",
            )),
        ),
        (
            "plugin.configure",
            main(a_plugin, device_view, "Configure", Key::Enter),
        ),
        (
            "device.set_configured_params",
            Exception(Equivalent(
                "plugin.configure_param, plugin.unconfigure_param and \
                 plugin.move_configured_param, which edit the configured list one entry at a \
                 time",
            )),
        ),
        (
            "plugin.configure_param",
            Exception(Equivalent(
                "moving a parameter in the plug-in's own window while Configure is on, which \
                 adds it to the device's panel",
            )),
        ),
        (
            "plugin.unconfigure_param",
            Exception(AgentOnly(
                "removing a configured parameter; the device panel draws knobs only, so a \
                 person reaches it through the agent API",
            )),
        ),
        (
            "plugin.move_configured_param",
            Exception(AgentOnly(
                "reordering a device's configured parameters; the device panel draws knobs \
                 only, so a person reaches it through the agent API",
            )),
        ),
        (
            "plugin.set_panel_param",
            Exception(Equivalent(
                "right-clicking a panel knob and picking a parameter, which remaps that knob",
            )),
        ),
        // WP-2.9b: Settings ▸ Plug-Ins' Rescan and Plug-In Sources
        // switches. The app scans by itself at start; a scan of named paths
        // is an agent's.
        (
            "plugin.scan",
            Exception(AgentOnly(
                "scanning named files or folders; the app scans the system folders itself",
            )),
        ),
        (
            "plugin.rescan",
            settings("plug-ins", nothing, "Rescan Plug-Ins", Key::Enter),
        ),
        ("plugin.list", Exception(Query)),
        (
            "plugin.export_list",
            Exception(Equivalent(
                "the Export… button in the list of the set's plug-ins, which asks where to \
                 save the list",
            )),
        ),
        (
            "plugin.settings",
            settings(
                "plug-ins",
                nothing,
                "Use VST3 Plug-In System Folders",
                Key::Enter,
            ),
        ),
        // A send knob and the In/Out choosers (the routing spec): Up and
        // Down on them.
        (
            "mixer.set_send",
            main(nothing, arrangement_track_3, "A-Return", Key::ArrowUp),
        ),
        (
            "routing.set_input",
            main(nothing, arrangement_track_3, "Input Type", Key::ArrowDown),
        ),
        (
            "routing.set_output",
            main(nothing, arrangement_track_3, "Output Type", Key::ArrowDown),
        ),
        (
            "routing.set_midi_output",
            main(nothing, arrangement_track_1, "Output Type", Key::ArrowUp),
        ),
        // Left and Right on the Monitor switch.
        (
            "routing.set_monitor",
            main(nothing, arrangement_track_3, "Monitoring", Key::ArrowRight),
        ),
        // Main's Cue Out and Main Out (WP-2.13): Up and Down on them step
        // through the interface's output pairs, which need more than one
        // pair to step to.
        (
            "routing.set_cue_out",
            main(
                four_channel_interface,
                session_main,
                "Cue Output",
                Key::ArrowDown,
            ),
        ),
        (
            "routing.set_main_out",
            main(
                four_channel_interface,
                session_main,
                "Main Output",
                Key::ArrowDown,
            ),
        ),
        // A return's Pre/Post switch, in Main's column: Enter switches it.
        (
            "mixer.set_send_tap",
            main(nothing, session_main, "Return A Pre/Post", Key::Enter),
        ),
        // Main's Cue Volume knob (WP-3.18): Up and Down step it.
        (
            "mixer.set_cue_volume",
            main(nothing, session_main, "Cue Volume", Key::ArrowDown),
        ),
        // The Crossfader section (WP-3.17): Main's crossfader and each
        // track's A and B buttons. Live's own key for them and their place
        // in the mixers' Tab order, with the section shown, aren't observed
        // yet; the next observation session records them.
        (
            "mixer.set_crossfader",
            Exception(Unobserved(
                "a drag on Main's crossfader in the Crossfader section",
            )),
        ),
        (
            "mixer.set_crossfade_assign",
            Exception(Unobserved(
                "a click on a track's A or B button in the Crossfader section",
            )),
        ),
        // The Track Options section (WP-5.36): a track's Track Delay, its
        // ms/samples unit switch, and its Keep Monitoring Latency switch,
        // and the pan control's Split Stereo Pan Mode controls (C-PAN-001,
        // C-PAN-002). Live's own keys for them and their place in the
        // mixers' Tab order aren't observed yet.
        (
            "mixer.set_track_delay",
            Exception(Unobserved(
                "a drag or typed value in a track's Track Options",
            )),
        ),
        (
            "mixer.set_pan_mode",
            Exception(Unobserved(
                "a choice from the pan control's context menu (C-PAN-001, C-PAN-002)",
            )),
        ),
        (
            "mixer.set_split_pan_left",
            Exception(Unobserved(
                "a drag on a split pan control in Split Stereo Pan Mode",
            )),
        ),
        (
            "mixer.set_split_pan_right",
            Exception(Unobserved(
                "a drag on a split pan control in Split Stereo Pan Mode",
            )),
        ),
        (
            "mixer.set_keep_monitoring_latency",
            Exception(Unobserved(
                "a click on a track's Keep Monitoring Latency switch",
            )),
        ),
        // Live's own exception.
        (
            "clip.set_fades",
            Exception(Live(
                "Live doesn't support editing audio clip fades with a screen reader (manual \
                 41.4.1.3); its fade keys, Cmd-Option-F and Cmd-Option-Delete (S-ARR-008, \
                 S-ARR-009), aren't bound yet",
            )),
        ),
        // MIDI file import and export (WP-2.21): the menus show them, and a
        // person reaches them once the file dialogs are built.
        (
            "midi.import_file",
            Exception(Equivalent(
                "the Create menu's Import MIDI File... dialog, which waits on the file dialogs \
                 (midi-clips R16)",
            )),
        ),
        (
            "midi.export_clip",
            Exception(Equivalent(
                "the File menu's Export MIDI Clip... dialog, which waits on the file dialogs \
                 (midi-clips R19)",
            )),
        ),
        // The Session View's scenes and slots (WP-3.1): the grid, its
        // context menus, and their keys are WP-3.7's.
        (
            "scene.create",
            Exception(LaterView("the Session grid and its menus (WP-3.7)")),
        ),
        (
            "scene.delete",
            Exception(LaterView("the Session grid and its menus (WP-3.7)")),
        ),
        (
            "scene.move",
            Exception(LaterView("the Session grid and its menus (WP-3.7)")),
        ),
        (
            "scene.duplicate",
            Exception(LaterView("the Session grid and its menus (WP-3.7)")),
        ),
        (
            "scene.rename",
            Exception(LaterView("the Session grid and its menus (WP-3.7)")),
        ),
        (
            "scene.set_tempo",
            Exception(Unobserved(
                "the scene's Tempo field, which a click focuses and Up, Down, digits, and Delete edit; its place in Live's Tab order",
            )),
        ),
        (
            "scene.set_follow_actions",
            Exception(Unobserved(
                "the follow controls in the Scene View, which Tab reaches; their place in Live's Tab order",
            )),
        ),
        (
            "session.set_follow_actions_enabled",
            Exception(Unobserved(
                "the switch in Main's status row, which Tab reaches with the grid's buttons; its place in Live's Tab order",
            )),
        ),
        (
            "scene.set_time_signature",
            Exception(Unobserved(
                "the scene's Signature fields, which a click focuses and Up, Down, digits, and Delete edit; their place in Live's Tab order",
            )),
        ),
        (
            "clip.create_in_slot",
            Exception(LaterView("the Session grid and its menus (WP-3.7)")),
        ),
        (
            "clip.move_to_slot",
            Exception(LaterView("the Session grid and its menus (WP-3.7)")),
        ),
        // The autosampler's dialog (WP-5.16, IMP-27).
        (
            "device.autosample_open",
            Exception(Gap(
                "the device's title bar menu, which the keyboard doesn't open yet \
                 (multisampler R64)",
            )),
        ),
        (
            "device.autosample",
            Exception(Equivalent(
                "the autosampler's dialog's Autosample, which Tab reaches",
            )),
        ),
        (
            "device.autosample_cancel",
            Exception(Equivalent(
                "the autosampler's dialog's Cancel, which Tab reaches",
            )),
        ),
        // Separate Stems' dialog (IMP-11).
        ("stems.status", Exception(Query)),
        (
            "stems.install",
            Exception(Equivalent(
                "the Separate Stems dialog's Download Required Packages Now, which Tab reaches",
            )),
        ),
        (
            "stems.separate",
            Exception(Equivalent(
                "the Separate Stems dialog's Separate, which Tab reaches",
            )),
        ),
        (
            "stems.cancel",
            Exception(Equivalent(
                "the Separate Stems dialog's Cancel, which Tab reaches",
            )),
        ),
        (
            "stems.close",
            Exception(Equivalent("the Separate Stems dialog's Cancel, or Esc")),
        ),
        // Slice to New MIDI Track's dialog (WP-4.16).
        (
            "clip.slice_to_midi_track",
            Exception(Equivalent(
                "the Slice to New MIDI Track dialog's OK, which Tab reaches",
            )),
        ),
        (
            "clip.close_slice_dialog",
            Exception(Equivalent(
                "the Slice to New MIDI Track dialog's Cancel, or Esc",
            )),
        ),
        // Gaps.
        (
            "export.audio",
            Exception(Gap(
                "the Export Audio/Video dialog, which waits on observing Live (export R16)",
            )),
        ),
        (
            "track.move",
            Exception(Gap(
                "moving tracks with Cmd and the arrows (S-TRACK-012), not bound yet",
            )),
        ),
        (
            "set.recover",
            Exception(Gap(
                "the recovery prompt at launch, which the app doesn't show yet (crash-recovery)",
            )),
        ),
        (
            "set.discard_recovery",
            Exception(Gap(
                "the recovery prompt at launch, which the app doesn't show yet (crash-recovery)",
            )),
        ),
        (
            "settings.set_continuous_save",
            Exception(Gap(
                "the Save Continuously switch on File & Folder, not drawn yet (crash-recovery \
                 R11)",
            )),
        ),
    ])
}

/// Every registered command, from every crate that registers them.
fn registry() -> bl_commands::Registry {
    let mut registry = bl_commands::Registry::new();
    bl_model::registry::register(&mut registry).unwrap();
    bl_project::registry::register(&mut registry).unwrap();
    bl_engine::transport::register(&mut registry).unwrap();
    bl_engine::session::register(&mut registry).unwrap();
    bl_shell::register(&mut registry).unwrap();
    registry
}

/// The commands a key runs (a bound key or key sequence), on either
/// platform.
fn keyed(keymap: &Keymap) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for entry in keymap.entries() {
        if !matches!(entry.kind.as_str(), "key" | "sequence") {
            continue;
        }
        match &entry.status {
            Status::Bound => {
                for command in &entry.commands {
                    out.entry(command.id.clone())
                        .or_insert_with(|| entry.id.clone());
                }
            }
            // The focused control's keys name the commands they run.
            Status::Widget(handler) => {
                for id in handler.split('`').skip(1).step_by(2) {
                    out.entry(id.to_owned()).or_insert_with(|| entry.id.clone());
                }
            }
            _ => {}
        }
    }
    out
}

/// The commands a menu item runs, on either platform: directly, or through
/// a bound shortcut.
fn on_menus(keymap: &Keymap) -> BTreeMap<String, String> {
    let every_context = bl_commands::Context::ALL;
    let mut out = BTreeMap::new();
    for platform in [MenuPlatform::Mac, MenuPlatform::Other] {
        for menu in menu_bar(platform) {
            for item in menu.walk().filter(|item| item.is_shown()) {
                let ids: Vec<String> = match item.action {
                    Action::Command(id) | Action::Choice(id, _) => vec![id.to_owned()],
                    Action::Shortcut(shortcuts) => shortcuts
                        .iter()
                        .filter_map(|s| keymap.command_for(s, &every_context))
                        .map(|c| c.id.clone())
                        .collect(),
                    _ => Vec::new(),
                };
                for id in ids {
                    out.entry(id).or_insert_with(|| item.row.to_owned());
                }
            }
        }
    }
    out
}

/// Opens the window a control is in, gets there with the keyboard, and
/// presses its key: true if the command ran.
fn reach(command_id: &str, path: &Path) -> Result<(), String> {
    let Path::Control {
        window,
        page,
        setup,
        start,
        label,
        key,
    } = path
    else {
        return Ok(());
    };
    let mut h = match window {
        Window::Main => {
            let mut h = Harness::with_title_bar(Window::Main);
            h.settle();
            h
        }
        Window::Settings => open_settings(page),
        Window::Device(_) | Window::Assistant => {
            unreachable!("no keyboard path starts in a floating window")
        }
    };
    setup(&mut h);
    if *window == Window::Main {
        tab_navigation_on(&mut h);
    }
    h.settings.refresh();
    h.settle();
    start(&mut h);
    if !tab_to(&mut h, label) {
        return Err(format!("{command_id}: Tab never reached {label:?}"));
    }
    h.take_events();
    if *key == Key::K {
        // Typing into a text field.
        h.frame(vec![egui::Event::Text("k".to_owned())]);
    } else {
        h.key(*key, Modifiers::NONE);
    }
    h.settle();
    let ran = h
        .take_events()
        .iter()
        .any(|event| matches!(event, UiEvent::Invoke(i) if i.id == command_id));
    if ran {
        Ok(())
    } else {
        Err(format!("{command_id}: {label:?} didn't run it"))
    }
}

// Spec: accessibility R8.
#[test]
fn every_m1_command_is_reachable_from_the_keyboard() {
    let registry = registry();
    let keymap = Keymap::live12();
    let keyed = keyed(&keymap);
    let menus = on_menus(&keymap);
    let paths = paths();
    let mut problems = Vec::new();
    let mut table = Vec::new();
    for spec in registry.iter() {
        let id = spec.id.as_ref();
        let key = keyed.get(id);
        let menu = menus.get(id);
        match (key, menu, paths.get(id)) {
            (Some(shortcut), _, Some(_)) | (None, Some(shortcut), Some(Path::Exception(_))) => {
                problems.push(format!(
                    "{id} is listed with its own path, but {shortcut} reaches it: take it out of \
                     the list"
                ));
            }
            (Some(shortcut), _, None) => table.push((id, format!("key, {shortcut}"))),
            (None, Some(row), None) => table.push((id, format!("menu, {row}"))),
            (None, menu, Some(path @ Path::Control { label, .. })) => match reach(id, path) {
                Ok(()) => table.push((
                    id,
                    format!(
                        "control, {label}{}",
                        menu.map(|row| format!("; menu, {row}")).unwrap_or_default()
                    ),
                )),
                Err(problem) => problems.push(problem),
            },
            (None, None, Some(Path::Exception(reason))) => {
                table.push((id, format!("{reason:?}")));
            }
            (None, None, None) => problems.push(format!(
                "{id} has no key, no menu item, and no keyboard-reachable control: give it one, \
                 or list it in `paths` with the reason (the accessibility spec, R8)"
            )),
        }
    }
    for id in paths.keys() {
        if registry.get(id).is_none() {
            problems.push(format!("{id} is listed but isn't a registered command"));
        }
    }
    for (id, how) in &table {
        println!("| `{id}` | {how} |");
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    let gaps: Vec<_> = paths
        .iter()
        .filter(|(_, p)| matches!(p, Path::Exception(Reason::Gap(_))))
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(
        gaps,
        [
            "chain.save_preset",
            "device.add_layers",
            "device.add_slice",
            "device.audition_sample",
            "device.autosample_open",
            "device.convert",
            "device.crop_sample",
            "device.delete_layers",
            "device.delete_slice",
            "device.distribute_layers",
            "device.duplicate_layers",
            "device.edit_sample",
            "device.load_sample",
            "device.move_layers",
            "device.move_slice",
            "device.normalize_layers",
            "device.rename_layer",
            "device.reverse_sample",
            "device.set_layer_zone",
            "device.snap_layer_markers",
            "device.sort_layers",
            "export.audio",
            "rack.set_macro_info_text",
            "set.discard_recovery",
            "set.recover",
            "settings.set_continuous_save",
            "track.move",
        ],
        "the gaps the accessibility spec lists"
    );
}

// Spec: accessibility R8.
#[test]
fn the_reachability_test_fails_for_a_command_with_no_keyboard_path() {
    let mut registry = registry();
    registry
        .register(
            bl_commands::CommandSpec::new("test.unreachable", "Unreachable")
                .description("A command nothing reaches, to show the test fails.")
                .permission(bl_commands::Permission::Read)
                .context(bl_commands::Context::Global)
                .args::<bl_commands::NoArgs>()
                .result::<()>(),
        )
        .unwrap();
    let keymap = Keymap::live12();
    let keyed = keyed(&keymap);
    let menus = on_menus(&keymap);
    let paths = paths();
    let unreachable: Vec<&str> = registry
        .iter()
        .map(|spec| spec.id.as_ref())
        .filter(|id| {
            !keyed.contains_key(*id) && !menus.contains_key(*id) && !paths.contains_key(id)
        })
        .collect();
    assert_eq!(unreachable, ["test.unreachable"]);
}

// Spec: accessibility R5 (S-MENU-001 to S-MENU-004).
#[test]
fn alt_and_a_letter_open_the_drawn_menu_bar_s_menus() {
    // The menu bar Beatlayer draws where there's no native one (Linux).
    let (mut h, _, _) = set();
    h.menu_bar = Some(menu_bar(MenuPlatform::Other));
    h.settle();
    let output = press(&mut h, Key::N, Modifiers::ALT);
    assert!(egui::Popup::is_any_open(&h.ctx), "Alt-N opened Navigate");
    // An item reads its label, then its keys.
    assert_eq!(
        focused_label(&output),
        "Control Bar Alt+0",
        "its first item"
    );
    // Down moves to the next item that can be chosen; Enter chooses it.
    // Navigate's Session View (Option-1) is live now (WP-3.7).
    let output = press(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(focused_label(&output), "Session View Alt+1");
    let output = press(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(focused_label(&output), "Arrangement View Alt+2");
    h.shell.set_focused_area(Area::ControlBar);
    h.take_events();
    press(&mut h, Key::Enter, Modifiers::NONE);
    assert!(
        h.take_events()
            .iter()
            .any(|e| matches!(e, UiEvent::Invoke(i) if i.id == "focus.arrangement"))
    );
    assert_eq!(h.shell.focused_area(), Area::MainView);
    assert!(!egui::Popup::is_any_open(&h.ctx), "choosing closed it");
    // Esc closes a menu without choosing.
    press(&mut h, Key::E, Modifiers::ALT);
    assert!(egui::Popup::is_any_open(&h.ctx));
    press(&mut h, Key::Escape, Modifiers::NONE);
    assert!(!egui::Popup::is_any_open(&h.ctx));
}

/// The keys that open the focused item's context menu (S-CTX-002):
/// Ctrl-Option-Shift-M on macOS, Shift-F10 elsewhere.
fn context_menu_keys(h: &mut Harness) -> FullOutput {
    if cfg!(target_os = "macos") {
        press(
            &mut *h,
            Key::M,
            Modifiers::CTRL | Modifiers::ALT | Modifiers::SHIFT,
        )
    } else {
        press(&mut *h, Key::F10, Modifiers::SHIFT)
    }
}

/// Presses Down until a screen reader would read `label` as focused.
fn down_to(h: &mut Harness, label: &str) -> bool {
    for _ in 0..40 {
        let output = h.settle();
        if focused_label(&output).starts_with(label) {
            return true;
        }
        h.key(Key::ArrowDown, Modifiers::NONE);
    }
    false
}

// Spec: accessibility R8 (S-CTX-002).
#[test]
fn the_keys_open_the_arrangement_s_context_menus() {
    let (mut h, _, clip) = set();
    press(&mut h, Key::Num2, Modifiers::ALT);
    // With a clip selected, the clip's menu, its first item focused.
    h.shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    let output = context_menu_keys(&mut h);
    assert!(egui::Popup::is_any_open(&h.ctx));
    assert!(
        focused_label(&output).starts_with("Cut"),
        "{}",
        focused_label(&output)
    );
    assert!(
        down_to(&mut h, "Loop Selection"),
        "the arrow keys move through it"
    );
    press(&mut h, Key::Escape, Modifiers::NONE);
    assert!(!egui::Popup::is_any_open(&h.ctx));
    // With none, the time selection's: its grid choices set the grid.
    h.shell
        .run("arrangement.select_clips", json!({ "clips": [] }))
        .unwrap();
    h.ctx.memory_mut(|m| m.request_focus(main_view_id()));
    h.settle();
    context_menu_keys(&mut h);
    assert!(egui::Popup::is_any_open(&h.ctx));
    assert!(
        down_to(&mut h, "Narrowest"),
        "Down reaches the grid choices"
    );
    h.take_events();
    press(&mut h, Key::Enter, Modifiers::NONE);
    assert!(
        h.take_events()
            .iter()
            .any(|e| matches!(e, UiEvent::Invoke(i) if i.id == "arrangement.set_grid"))
    );
}

// Spec: accessibility R8 (S-CTX-002).
#[test]
fn the_keys_open_a_sidebar_place_s_context_menu() {
    let dir = std::env::temp_dir().join(format!("bl-ui-a11y-{}", std::process::id()));
    let loops = dir.join("Loops");
    std::fs::create_dir_all(&loops).unwrap();
    let (mut h, _, _) = set();
    h.shell
        .run(
            "browser.add_folder",
            json!({ "path": loops.to_string_lossy() }),
        )
        .unwrap();
    h.shell
        .run(
            "browser.open_place",
            json!({ "label": { "folder": loops.to_string_lossy() } }),
        )
        .unwrap();
    h.shell
        .run("focus.browser", json!({ "part": "sidebar" }))
        .unwrap();
    h.settle();
    assert_eq!(focused_id(&h), Some(bl_ui::browser::sidebar_id()));
    let output = context_menu_keys(&mut h);
    assert_eq!(focused_label(&output), "Remove from Sidebar");
    h.take_events();
    press(&mut h, Key::Enter, Modifiers::NONE);
    assert!(
        h.take_events()
            .iter()
            .any(|e| matches!(e, UiEvent::Invoke(i) if i.id == "browser.remove_folder"))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_sample_editors_markers_reach_assistive_technology() {
    // Spec: sample-editor R20 (accessibility R4).
    use bl_ui::arrangement::peaks::MemoryPeaks;
    let (mut h, _tracks, clip) = set();
    let mut peaks = MemoryPeaks::new();
    peaks.insert(
        benchmark::media_hash(0),
        std::sync::Arc::new(
            bl_media::Peaks::from_samples(&benchmark::media_samples(
                &mut benchmark::Random::new(1),
                16.0,
            ))
            .unwrap(),
        ),
    );
    h.main
        .arrangement_mut()
        .set_peak_source(Box::new(peaks), None);
    h.shell
        .run("clip.set_warp", json!({ "clip": clip, "warp": true }))
        .unwrap();
    h.shell
        .run(
            "clip.insert_warp_marker",
            json!({ "clips": [clip], "time": 8.0 }),
        )
        .unwrap();
    h.shell
        .run(
            "clip.insert_transient",
            json!({ "clip": clip, "time": 4.0 }),
        )
        .unwrap();
    h.shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    h.shell
        .run("clip_view.select_warp_markers", json!({ "markers": [1] }))
        .unwrap();
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    let view = Harness::node(&nodes, "Clip View").bounds.unwrap();
    let inside = |node: &common::Node| {
        let b = node.bounds.unwrap();
        b.x0 >= view.x0 - 1.0 && b.x1 <= view.x1 + 1.0 && b.y0 >= view.y0 && b.y1 <= view.y1 + 1.0
    };
    // The waveform is a region naming the clip.
    let wave = nodes
        .iter()
        .find(|n| n.label.starts_with("Waveform of Kick"))
        .expect("the waveform is a region naming the clip");
    assert_eq!(wave.role, Role::Group);
    // Each marker is an item with its name, its value, and whether it is
    // selected.
    let first = Harness::node(&nodes, "Warp Marker 1");
    let second = Harness::node(&nodes, "Warp Marker 2");
    assert_eq!(first.role, Role::ListItem);
    assert_eq!(first.selected, Some(false));
    assert_eq!(second.selected, Some(true));
    assert!(
        second.value.contains("8.000 s in the file"),
        "{}",
        second.value
    );
    assert!(second.value.contains("beat 5.1.1"), "{}", second.value);
    let transient = Harness::node(&nodes, "Transient 1");
    assert!(
        transient.value.contains("4.000 s in the file"),
        "{}",
        transient.value
    );
    for name in ["Start Marker", "End Marker", "Loop Brace"] {
        let node = Harness::node(&nodes, name);
        assert_eq!(node.role, Role::ListItem, "{name}");
        assert!(!node.value.is_empty(), "{name}");
    }
    for node in [
        wave,
        first,
        second,
        transient,
        Harness::node(&nodes, "Start Marker"),
    ] {
        assert!(inside(node), "{} lies outside the Clip View", node.label);
    }
}

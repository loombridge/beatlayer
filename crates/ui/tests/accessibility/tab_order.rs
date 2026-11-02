//! Tab, the neighbour keys and where the focus goes.

use crate::accessibility::helpers::*;
use crate::common::command;
use bl_model::TrackId;
use bl_shell::Area;
use bl_shell::arrangement_commands::ArrangementState;
use bl_shell::mixer_view::MixerOf;
use bl_ui::arrangement::headers::title_id;
use bl_ui::focus::Slot;
use bl_ui::in_out::control_id;
use bl_ui::layout::{ELEMENTS, Strip};
use bl_ui::main_window::{element_id, main_view_id};
use bl_ui::mixer::{Place, column_id, fader_id, pan_id, send_id, switch_id, volume_field_id};
use egui::{Id, Key, Modifiers};
use serde_json::json;

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

    // The browser: from the search field to the Filter View toggle and
    // menu, the sidebar, the Filter View, then the content pane's header
    // and list, each part's keys applying as the shell moves its focus
    // there (browser-tags R23, I-FOCUS-007).
    press(&mut h, Key::F, command());
    assert_eq!(focused_id(&h), Some(bl_ui::browser::search_id()));
    press(&mut h, Key::Tab, Modifiers::NONE);
    assert_eq!(
        focused_id(&h),
        Some(bl_ui::browser::control_id("Show Filter View"))
    );
    press(&mut h, Key::Tab, Modifiers::NONE);
    assert_eq!(
        focused_id(&h),
        Some(bl_ui::browser::control_id("Filter View Menu"))
    );
    press(&mut h, Key::Tab, Modifiers::NONE);
    assert_eq!(focused_id(&h), Some(bl_ui::browser::sidebar_id()));
    assert_eq!(
        h.shell.browser_view().focus(),
        bl_shell::browser::Part::Sidebar
    );
    press(&mut h, Key::Tab, Modifiers::NONE);
    assert_eq!(focused_id(&h), Some(bl_ui::browser::filter_view_id()));
    press(&mut h, Key::Tab, Modifiers::NONE);
    assert_eq!(focused_id(&h), Some(bl_ui::browser::content_options_id()));
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
    // yet: the locator buttons and Automation Mode above the track
    // headers, then the selected track's header (its In/Out choosers, its
    // mixer column, its sends), then Lock Envelopes, the control above the
    // headers (accessibility R10), then Optimize Height and Width.
    let expected = [
        bl_ui::arrangement::set_locator_id(),
        bl_ui::arrangement::previous_locator_id(),
        bl_ui::arrangement::next_locator_id(),
        bl_ui::arrangement::automation_mode_id(),
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
        bl_ui::arrangement::lock_envelopes_id(),
        bl_ui::arrangement::optimize_height_id(),
        bl_ui::arrangement::optimize_width_id(),
    ];
    for want in expected {
        press(&mut h, Key::Tab, Modifiers::NONE);
        assert_eq!(focused_id(&h), Some(want));
    }
    // Wrap Tab Navigation is on: round to Set Locator.
    press(&mut h, Key::Tab, Modifiers::NONE);
    assert_eq!(focused_id(&h), Some(bl_ui::arrangement::set_locator_id()));
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
    for _ in 0..5 {
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
    for _ in 0..14 {
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

// Spec: arranger-track R14.
#[test]
fn tab_reaches_the_sections_in_time_order_straight_after_lock_envelopes() {
    let (mut h, tracks, _) = set();
    for (from, to) in [(0, 2), (2, 4), (4, 6)] {
        let whole = bl_core::time::Ticks::WHOLE.get();
        h.shell
            .run(
                "arranger.add_section",
                json!({ "start": from * whole, "end": to * whole }),
            )
            .unwrap();
    }
    h.shell
        .run("arrangement.arranger_track", json!({ "on": true }))
        .unwrap();
    tab_navigation_on(&mut h);
    press(&mut h, Key::Num2, Modifiers::ALT);
    h.shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[2]] }),
        )
        .unwrap();
    h.settle();
    let sections: Vec<Id> = h
        .shell
        .editor()
        .document()
        .sections()
        .iter()
        .map(|s| bl_ui::arrangement::arranger::section_id(s.id))
        .collect();
    let mut order = Vec::new();
    for _ in 0..30 {
        press(&mut h, Key::Tab, Modifiers::NONE);
        order.push(focused_id(&h).unwrap());
    }
    let lock = order
        .iter()
        .position(|id| *id == bl_ui::arrangement::lock_envelopes_id())
        .expect("Lock Envelopes is in the order");
    assert_eq!(order[lock + 1..=lock + 3], sections[..]);
    assert_eq!(
        order[lock + 4],
        bl_ui::arrangement::optimize_height_id(),
        "Optimize Height follows the sections"
    );
}

fn peak_id(place: Place, track: TrackId) -> Id {
    Id::new(("mixer-peak", place, track))
}

// Spec: accessibility R10; time-editing R3, R12.
#[test]
fn a_new_locator_takes_the_keyboard_focus_and_delete_removes_the_reached_one() {
    let (mut h, _, _) = set();
    tab_navigation_on(&mut h);
    h.shell
        .run("locator.add", json!({ "at": 2_822_400 }))
        .unwrap();
    h.settle();
    let locator = h.shell.editor().document().locators()[0].id;
    // A new locator takes the keyboard focus (time-editing R3).
    assert_eq!(
        focused_id(&h),
        Some(bl_ui::arrangement::locator_id(locator))
    );
    // Tab reaching a locator selects it (time-editing R12).
    h.shell
        .run("locator.select", json!({ "locators": [] }))
        .unwrap();
    h.settle();
    // Backwards from the locator to Set Locator, then forwards to it again.
    for _ in 0..3 {
        press(&mut h, Key::Tab, Modifiers::SHIFT);
    }
    assert_eq!(focused_id(&h), Some(bl_ui::arrangement::set_locator_id()));
    for _ in 0..3 {
        press(&mut h, Key::Tab, Modifiers::NONE);
    }
    assert_eq!(
        focused_id(&h),
        Some(bl_ui::arrangement::locator_id(locator))
    );
    let state: ArrangementState =
        serde_json::from_value(h.shell.run("arrangement.state", json!({})).unwrap()).unwrap();
    assert_eq!(state.selected_locators, vec![locator]);
    // Delete deletes the reached locator (time-editing R4, R12).
    press(&mut h, Key::Delete, Modifiers::NONE);
    assert!(h.shell.editor().document().locators().is_empty());
}

// Spec: arrangement-view R24; accessibility R4.
#[test]
fn the_hidden_track_controls_leave_the_tab_order() {
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
    // The In/Out choosers, the volume, the pan, and the sends are hidden:
    // they leave the Tab order, and the rest of the header's controls stay
    // in it.
    for id in [
        "arrangement.show_in_out",
        "arrangement.show_volume",
        "arrangement.show_return_tracks",
    ] {
        h.shell.run(id, json!({ "on": false })).unwrap();
    }
    h.settle();
    let expected = [
        // Spec: accessibility R10. The controls above the track headers
        // come first.
        bl_ui::arrangement::set_locator_id(),
        bl_ui::arrangement::previous_locator_id(),
        bl_ui::arrangement::next_locator_id(),
        bl_ui::arrangement::automation_mode_id(),
        title_id(tracks[2]),
        header(Switch::Activator),
        header(Switch::Solo),
        header(Switch::Arm),
        // The Lock Envelopes switch, the control above the headers
        // (accessibility R10), is a Tab stop whatever the header shows.
        bl_ui::arrangement::lock_envelopes_id(),
        bl_ui::arrangement::optimize_height_id(),
        bl_ui::arrangement::optimize_width_id(),
    ];
    for want in expected {
        press(&mut h, Key::Tab, Modifiers::NONE);
        assert_eq!(focused_id(&h), Some(want));
    }
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

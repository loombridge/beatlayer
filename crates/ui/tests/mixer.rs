//! The Mixer view (WP-1.9), headless: the Session View's columns and mixer
//! and the Arrangement's mixer sit where layout.md puts them, at 1600 ×
//! 1000 points and at every zoom; every control works by mouse, keyboard,
//! and assistive technology, running the registered commands an agent runs;
//! and a fader dragged through the window plays the gesture the golden
//! render checks for zipper noise.
//!
//! Spec: mixer-view R1 to R19.

mod common;

use std::path::PathBuf;

use bl_engine::TrackMeter;
use bl_model::TrackId;
use bl_shell::menus::{MenuPlatform, menu_bar};
use bl_shell::mixer_controls::{self as controls, Control};
use bl_shell::mixer_view::{MixerOf, MixerViewState};
use bl_shell::mixer_view_commands as ids;
use bl_shell::settings::zoom_factor;
use bl_shell::{Shell, ZOOM_STEPS};
use bl_ui::keymap::Platform;
use bl_ui::layout::mixer::{MixerLayout, Sections, SessionLayout, arrangement_split};
use bl_ui::layout::{ArrangementLayout, REFERENCE, Row, ShellLayout, TITLE_BAR};
use bl_ui::menus::item_state;
use bl_ui::mixer::{Place, fader_id, volume_field_id};
use common::{Harness, Node, Window, command};
use egui::accesskit::{Action, Role};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, Vec2, pos2, vec2};
use serde_json::json;

/// A main window that draws Live's title bar, so every position is in
/// layout.md's window coordinates.
fn harness() -> Harness {
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    h
}

/// The same, with the Session View in front.
fn session() -> Harness {
    let mut h = harness();
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    h.settle();
    h
}

fn state(shell: &mut Shell) -> MixerViewState {
    serde_json::from_value(shell.run(ids::STATE, json!({})).unwrap()).unwrap()
}

fn tracks(shell: &Shell) -> Vec<TrackId> {
    shell.editor().document().top_level().collect()
}

fn mixer(shell: &Shell, track: TrackId) -> bl_model::Mixer {
    *shell.editor().document().track(track).unwrap().mixer()
}

fn rect_of(node: &Node) -> Rect {
    let b = node.bounds.expect("a node with bounds");
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

/// The node labelled `label` inside the group labelled `group` (a mixer
/// column, named for its track), or inside the mixer labelled `mixer`.
fn inside<'a>(nodes: &'a [Node], within: &str, label: &str) -> Option<&'a Node> {
    let parent = nodes.iter().find(|n| n.label == within)?.id;
    nodes
        .iter()
        .find(|n| n.label == label && Harness::descends_from(nodes, n.id, parent))
}

fn tree(h: &mut Harness) -> Vec<Node> {
    let output = h.settle();
    Harness::nodes(&output)
}

/// A column's control's rectangle.
fn control(h: &mut Harness, column: &str, label: &str) -> Rect {
    let nodes = tree(h);
    rect_of(inside(&nodes, column, label).unwrap_or_else(|| panic!("{column}: {label}")))
}

fn click(h: &mut Harness, at: Pos2, modifiers: Modifiers) {
    let held = Event::ModifiersChanged(modifiers);
    h.frame(vec![held.clone(), Event::PointerMoved(at)]);
    for pressed in [true, false] {
        h.frame(vec![
            held.clone(),
            Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers,
            },
        ]);
    }
    h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    h.settle();
}

fn double_click(h: &mut Harness, at: Pos2) {
    // Long enough after any earlier click that egui doesn't count it in.
    h.time += 1.0;
    h.frame(vec![Event::PointerMoved(at)]);
    for _ in 0..2 {
        for pressed in [true, false] {
            h.frame(vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            }]);
        }
    }
    h.settle();
}

/// A press at `from`, a move by each of `steps` in its own frame, and a
/// release, with `modifiers` held. Returns a value read after every frame.
fn drag<T>(
    h: &mut Harness,
    from: Pos2,
    steps: &[Vec2],
    modifiers: Modifiers,
    mut read: impl FnMut(&Harness) -> T,
) -> Vec<T> {
    let held = Event::ModifiersChanged(modifiers);
    let mut out = Vec::new();
    h.frame(vec![held.clone(), Event::PointerMoved(from)]);
    h.frame(vec![
        held.clone(),
        Event::PointerButton {
            pos: from,
            button: PointerButton::Primary,
            pressed: true,
            modifiers,
        },
    ]);
    out.push(read(h));
    let mut at = from;
    for step in steps {
        at += *step;
        h.frame(vec![held.clone(), Event::PointerMoved(at)]);
        out.push(read(h));
    }
    h.frame(vec![
        held,
        Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: false,
            modifiers,
        },
    ]);
    out.push(read(h));
    h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    h.settle();
    out
}

fn key(h: &mut Harness, key: Key, modifiers: Modifiers) {
    h.key(key, modifiers);
    h.settle();
}

/// layout.md, or `None` in a checkout without the planning documents.
fn layout_md() -> Option<String> {
    let docs = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs");
    if !docs.exists() {
        eprintln!(
            "skipped: the UI parity reference isn't published yet, and this checkout doesn't have it"
        );
        return None;
    }
    Some(std::fs::read_to_string(docs.join("ui-parity/layout.md")).unwrap())
}

// Spec: mixer-view R1.
#[test]
fn the_session_mixer_sits_where_live_puts_it() {
    let mut h = session();
    let nodes = tree(&mut h);
    let mixer = rect_of(Harness::node(&nodes, "Session Mixer"));
    // L-SES-009 under the 17-point status row, to the main view's bottom
    // (766); its sections 115 + 56 + 123 points.
    assert_eq!(
        (mixer.left(), mixer.top(), mixer.right(), mixer.bottom()),
        (320.0, 472.0, 1591.0, 766.0)
    );
    // L-SES-001: columns from 320, 94 points each; L-SES-005 and 006: the
    // returns before Main, which ends at 1591 and is 104 points wide.
    let column = |name: &str| rect_of(inside(&nodes, "Session Mixer", name).unwrap());
    for (i, name) in ["1 MIDI", "2 MIDI", "3 Audio", "4 Audio"]
        .iter()
        .enumerate()
    {
        let rect = column(name);
        assert_eq!(rect.left(), 320.0 + 94.0 * i as f32, "{name}");
        assert_eq!(rect.width(), 94.0, "{name}");
    }
    assert!((column("A Return").left() - 1298.0).abs() <= 2.0);
    assert!((column("B Return").left() - 1393.0).abs() <= 2.0);
    let main = column("Main");
    assert_eq!((main.left(), main.right()), (1487.0, 1591.0));
    // The strip's controls sit in the strip's 123 points at the bottom.
    let volume = rect_of(inside(&nodes, "1 MIDI", "Track Volume").unwrap());
    assert!(volume.top() >= 643.0 && volume.left() >= 320.0 && volume.right() <= 414.0);
    // The sends' 56 points over it, In/Out's 115 over them.
    let send = rect_of(inside(&nodes, "1 MIDI", "Send A").unwrap());
    assert!(send.top() >= 587.0 && send.bottom() <= 643.0);
    let input = rect_of(inside(&nodes, "1 MIDI", "Input Type").unwrap());
    assert!(input.top() >= 472.0 && input.bottom() <= 587.0);
}

// Spec: mixer-view R1, R12.
#[test]
fn hiding_sections_and_the_mixer_gives_the_slots_their_height() {
    let mut h = session();
    h.shell.run(ids::IN_OUT, json!({})).unwrap();
    let nodes = tree(&mut h);
    let mixer = rect_of(Harness::node(&nodes, "Session Mixer"));
    assert_eq!(mixer.top(), 766.0 - 56.0 - 123.0);
    assert!(inside(&nodes, "1 MIDI", "Input Type").is_none());
    h.shell.run(ids::RETURN_TRACKS, json!({})).unwrap();
    let nodes = tree(&mut h);
    assert!(inside(&nodes, "Session Mixer", "A Return").is_none());
    h.shell.run(ids::TOGGLE, json!({})).unwrap();
    let nodes = tree(&mut h);
    assert!(nodes.iter().all(|n| n.label != "Session Mixer"));
}

// Spec: mixer-view R2.
#[test]
fn the_arrangement_mixer_splits_the_main_view() {
    let mut h = harness();
    let before = rect_of(Harness::node(&tree(&mut h), "Track Lanes"));
    h.shell.run(ids::TOGGLE, json!({})).unwrap();
    let nodes = tree(&mut h);
    let mixer = rect_of(Harness::node(&nodes, "Arrangement Mixer"));
    // L-ARR-018: the mixer from 407 to 729; the lanes and pinned rows above
    // 397.
    assert_eq!(
        (mixer.left(), mixer.top(), mixer.right(), mixer.bottom()),
        (320.0, 407.0, 1591.0, 729.0)
    );
    let lanes = rect_of(Harness::node(&nodes, "Track Lanes"));
    assert_eq!(lanes.top(), before.top());
    assert!(lanes.bottom() <= 397.0);
    // The time ruler stays at the bottom (L-ARR-016).
    let time = rect_of(Harness::node(&nodes, "Time Ruler"));
    assert_eq!((time.top(), time.bottom()), (746.0, 760.0));
    // The strips stack from the mixer's bottom.
    let volume = rect_of(inside(&nodes, "Arrangement Mixer", "Track Volume").unwrap());
    assert!(volume.top() >= 729.0 - 123.0);
    let layout = ArrangementLayout::compute_with_mixer(
        Rect::from_min_max(pos2(320.0, 61.0), pos2(1591.0, 766.0)),
        true,
        3,
    );
    assert_eq!(layout.pinned_headers.bottom(), 397.0);
    assert!(layout.grid_indicator.bottom() <= 397.0);
    // Hidden again, the lanes have their height back.
    h.shell.run(ids::TOGGLE, json!({})).unwrap();
    let lanes = rect_of(Harness::node(&tree(&mut h), "Track Lanes"));
    assert_eq!(lanes, before);
}

// Spec: mixer-view R3, R5.
#[test]
fn every_strip_has_its_controls_in_live_s_order() {
    let mut h = session();
    let nodes = tree(&mut h);
    let get = |column: &str, label: &str| inside(&nodes, column, label).map(rect_of);
    for column in ["1 MIDI", "3 Audio"] {
        let volume = get(column, "Track Volume").unwrap();
        let pan = get(column, "Track Panning").unwrap();
        let fader = get(column, "Track Volume Fader").unwrap();
        let meter = get(column, "Track Meter").unwrap();
        let activator = get(column, "Track Activator").unwrap();
        let solo = get(column, "Solo").unwrap();
        let arm = get(column, "Arm Recording").unwrap();
        // The volume field and pan at the top left, pan under the field.
        assert!(pan.top() >= volume.bottom(), "{column}");
        assert!((pan.left() - volume.left()).abs() < 0.5, "{column}");
        // The fader at the right, the meter at the column's right edge.
        assert!(
            fader.left() >= volume.right() && meter.left() >= fader.right(),
            "{column}"
        );
        // Then the activator, Solo, and Arm.
        for button in [activator, solo, arm] {
            assert!(
                button.top() >= fader.bottom() && button.top() >= pan.bottom(),
                "{column}"
            );
        }
        assert!(
            activator.right() <= solo.left() && solo.right() <= arm.left(),
            "{column}"
        );
        // In/Out in Live's order.
        let rows: Vec<f32> = [
            "Input Type",
            "Input Channel",
            "Monitor",
            "Output Type",
            "Output Channel",
        ]
        .iter()
        .map(|label| get(column, label).unwrap().top())
        .collect();
        assert!(rows.windows(2).all(|w| w[0] < w[1]), "{column}: {rows:?}");
        // Sends A and B, staggered.
        let (a, b) = (
            get(column, "Send A").unwrap(),
            get(column, "Send B").unwrap(),
        );
        assert!(a.right() <= b.left() && a.bottom() <= b.top(), "{column}");
    }
    // A return shows its letter and has no Arm; Main has no activator or
    // Solo of its own, but the cue volume and the Solo/Cue button.
    let activator = inside(&nodes, "A Return", "Track Activator").unwrap();
    assert!(activator.description.starts_with("Track A"));
    assert!(inside(&nodes, "A Return", "Arm Recording").is_none());
    assert!(inside(&nodes, "A Return", "Solo").is_some());
    assert!(inside(&nodes, "Main", "Track Activator").is_none());
    assert!(inside(&nodes, "Main", "Solo").is_none());
    let cue = inside(&nodes, "Main", "Cue Volume").unwrap();
    assert!(cue.disabled && cue.description.contains("Phase 3"));
    assert!(inside(&nodes, "Main", "Solo/Cue Mode").unwrap().disabled);
    assert!(inside(&nodes, "Main", "Track Volume Fader").is_some());
    assert!(
        inside(&nodes, "Main", "Return A Pre/Post")
            .unwrap()
            .disabled
    );
}

// Spec: mixer-view R4.
#[test]
fn the_arrangement_headers_mixer_controls_are_live() {
    let mut h = harness();
    let t = tracks(&h.shell);
    let nodes = tree(&mut h);
    // The first track's header: its Solo is the topmost one.
    let mut solos: Vec<Rect> = nodes
        .iter()
        .filter(|n| n.label == "Solo")
        .map(rect_of)
        .collect();
    solos.sort_by(|a, b| a.top().total_cmp(&b.top()));
    // In the header's mixer column, 1479 to 1577 (L-ARR-006).
    assert!(solos[0].left() >= 1479.0 && solos[0].right() <= 1577.0);
    click(&mut h, solos[0].center(), Modifiers::NONE);
    assert!(mixer(&h.shell, t[0]).solo);
    // Volume and pan step from the keyboard once clicked.
    let mut volumes: Vec<Rect> = Harness::nodes(&h.settle())
        .iter()
        .filter(|n| n.label == "Track Volume")
        .map(rect_of)
        .collect();
    volumes.sort_by(|a, b| a.top().total_cmp(&b.top()));
    click(&mut h, volumes[0].center(), Modifiers::NONE);
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert!((controls::gain_db(mixer(&h.shell, t[0]).volume) + 1.0).abs() < 1e-9);
    let nodes = tree(&mut h);
    let meters = nodes.iter().filter(|n| n.label == "Track Meter").count();
    assert!(meters >= 7, "a meter in every header: {meters}");
}

// Spec: mixer-view R6.
#[test]
fn dragging_a_fader_or_a_field_is_one_undo_step() {
    let mut h = session();
    let t = tracks(&h.shell);
    let fader = control(&mut h, "1 MIDI", "Track Volume Fader");
    let handle = pos2(
        fader.center().x,
        fader.bottom() - fader.height() * controls::fader_position(1.0) as f32,
    );
    let before = h.shell.editor().undo_history().len();
    let positions = drag(
        &mut h,
        handle,
        &[vec2(0.0, 2.0); 20],
        Modifiers::NONE,
        |h| controls::fader_position(mixer(&h.shell, t[0]).volume),
    );
    assert_eq!(h.shell.editor().undo_history().len(), before + 1);
    assert_eq!(
        h.shell.editor().undo_history().next(),
        Some(controls::VOLUME_GESTURE)
    );
    // The handle follows the pointer: whatever egui's drag threshold took,
    // every point after it moved the fader a point's worth.
    let moved = positions[0] - positions.last().unwrap();
    assert!(moved > 0.0);
    let per_point = 1.0 / f64::from(fader.height());
    assert!(
        moved <= 40.0 * per_point + 1e-9 && moved >= 30.0 * per_point,
        "{moved}"
    );
    // The field: 100 points of drag for the whole travel, Shift ten times
    // finer.
    let field = control(&mut h, "2 MIDI", "Track Volume");
    let start = controls::fader_position(mixer(&h.shell, t[1]).volume);
    drag(
        &mut h,
        field.center(),
        &[vec2(0.0, -2.0); 10],
        Modifiers::NONE,
        |_| (),
    );
    let coarse = controls::fader_position(mixer(&h.shell, t[1]).volume) - start;
    assert!(coarse > 0.1 && coarse <= 0.2 + 1e-9, "{coarse}");
    let start = controls::fader_position(mixer(&h.shell, t[1]).volume);
    drag(
        &mut h,
        field.center(),
        &[vec2(0.0, 2.0); 10],
        Modifiers::SHIFT,
        |_| (),
    );
    let fine = start - controls::fader_position(mixer(&h.shell, t[1]).volume);
    assert!(fine > 0.01 && fine <= 0.02 + 1e-9, "{fine}");
    // Pan: its whole range per 200 points.
    let pan = control(&mut h, "3 Audio", "Track Panning");
    drag(
        &mut h,
        pan.center(),
        &[vec2(0.0, -2.0); 20],
        Modifiers::NONE,
        |_| (),
    );
    let p = mixer(&h.shell, t[2]).pan;
    assert!(p > 0.2 && p <= 0.4 + 1e-9, "{p}");
    assert_eq!(
        h.shell.editor().undo_history().next(),
        Some(controls::PAN_GESTURE)
    );
}

// Spec: mixer-view R6.
#[test]
fn double_click_delete_and_the_triangle_reset() {
    let mut h = session();
    let t = tracks(&h.shell);
    for track in &t {
        h.shell
            .run(
                "mixer.set_volume",
                json!({ "track": track, "volume": 0.25 }),
            )
            .unwrap();
        h.shell
            .run("mixer.set_pan", json!({ "track": track, "pan": -0.5 }))
            .unwrap();
    }
    let field = control(&mut h, "1 MIDI", "Track Volume");
    double_click(&mut h, field.center());
    assert_eq!(mixer(&h.shell, t[0]).volume, 1.0);
    let fader = control(&mut h, "2 MIDI", "Track Volume Fader");
    click(&mut h, fader.center(), Modifiers::NONE);
    key(&mut h, Key::Delete, Modifiers::NONE);
    assert_eq!(mixer(&h.shell, t[1]).volume, 1.0);
    let pan = control(&mut h, "1 MIDI", "Track Panning");
    double_click(&mut h, pan.center());
    assert_eq!(mixer(&h.shell, t[0]).pan, 0.0);
    let triangle = control(&mut h, "2 MIDI", "Reset Panning");
    click(&mut h, triangle.center(), Modifiers::NONE);
    assert_eq!(mixer(&h.shell, t[1]).pan, 0.0);
    // Up and Down step volume by a dB, Shift by a tenth.
    let field = control(&mut h, "3 Audio", "Track Volume");
    click(&mut h, field.center(), Modifiers::NONE);
    key(&mut h, Key::ArrowUp, Modifiers::NONE);
    let db = controls::gain_db(mixer(&h.shell, t[2]).volume);
    assert!(
        (db - (controls::gain_db(0.25).round() + 1.0)).abs() < 1e-9,
        "{db}"
    );
    key(&mut h, Key::ArrowUp, Modifiers::SHIFT);
    let fine = controls::gain_db(mixer(&h.shell, t[2]).volume);
    assert!((fine - db - 0.1).abs() < 1e-9, "{fine}");
}

// Spec: mixer-view R7.
#[test]
fn the_activator_solo_and_arm_take_clicks_with_modifiers() {
    let mut h = session();
    let t = tracks(&h.shell);
    let solo = |h: &mut Harness, column: &str| control(h, column, "Solo").center();
    let at = solo(&mut h, "1 MIDI");
    click(&mut h, at, Modifiers::NONE);
    let at = solo(&mut h, "2 MIDI");
    click(&mut h, at, Modifiers::NONE);
    assert!(!mixer(&h.shell, t[0]).solo && mixer(&h.shell, t[1]).solo);
    let at = solo(&mut h, "3 Audio");
    click(&mut h, at, command());
    assert!(mixer(&h.shell, t[1]).solo && mixer(&h.shell, t[2]).solo);
    let arm = control(&mut h, "4 Audio", "Arm Recording");
    click(&mut h, arm.center(), Modifiers::NONE);
    assert!(mixer(&h.shell, t[3]).arm);
    let activator = control(&mut h, "4 Audio", "Track Activator");
    click(&mut h, activator.center(), Modifiers::NONE);
    assert!(mixer(&h.shell, t[3]).mute);
    let nodes = tree(&mut h);
    let node = inside(&nodes, "4 Audio", "Track Activator").unwrap();
    assert_eq!(node.value, "off");
    assert_eq!(inside(&nodes, "3 Audio", "Solo").unwrap().value, "on");
}

// Spec: mixer-view R9.
#[test]
fn s_0_c_and_the_function_keys_switch_tracks() {
    let mut h = harness();
    let t = tracks(&h.shell);
    h.shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [t[2]], "title_bars": true }),
        )
        .unwrap();
    key(&mut h, Key::S, Modifiers::NONE);
    assert!(mixer(&h.shell, t[2]).solo);
    key(&mut h, Key::Num0, Modifiers::NONE);
    assert!(mixer(&h.shell, t[2]).mute);
    key(&mut h, Key::C, Modifiers::NONE);
    assert!(mixer(&h.shell, t[2]).arm);
    // F2 switches track 2 wherever the focus is.
    key(&mut h, Key::F2, Modifiers::NONE);
    assert!(mixer(&h.shell, t[1]).mute);
    key(&mut h, Key::F2, Modifiers::NONE);
    assert!(!mixer(&h.shell, t[1]).mute);
}

// Spec: mixer-view R10.
#[test]
fn meters_show_the_engine_s_levels_and_redraw_until_silent() {
    let mut h = session();
    let t = tracks(&h.shell);
    // The engine plays nothing in a new set, so with it off a test sets the
    // meters as the engine would publish them.
    h.shell.run("audio.toggle_engine", json!({})).unwrap();
    h.frame(vec![]);
    let half = controls::db_gain(-6.0) as f32;
    let meters = [TrackMeter {
        track: t[0],
        peak: [half, half * 0.5],
        rms: [half * 0.5, half * 0.25],
    }];
    h.shell.set_meters(&meters);
    let output = h.frame(vec![]);
    let nodes = Harness::nodes(&output);
    let meter = inside(&nodes, "1 MIDI", "Track Meter").unwrap();
    assert_eq!(meter.role, Role::Meter);
    assert_eq!(meter.value, "peak -6.0, RMS -12.0");
    assert!((meter.numeric.unwrap() + 6.0).abs() < 1e-3);
    let delay = output
        .viewport_output
        .values()
        .next()
        .unwrap()
        .repaint_delay;
    assert_eq!(delay, std::time::Duration::ZERO, "the meters redraw");
    // Silent meters stop redrawing every frame.
    h.shell.set_meters(&[TrackMeter {
        track: t[0],
        peak: [0.0; 2],
        rms: [0.0; 2],
    }]);
    h.frame(vec![]);
    let output = h.frame(vec![]);
    let delay = output
        .viewport_output
        .values()
        .next()
        .unwrap()
        .repaint_delay;
    assert!(delay > std::time::Duration::ZERO);
    let nodes = Harness::nodes(&output);
    assert_eq!(
        inside(&nodes, "1 MIDI", "Track Meter").unwrap().value,
        "peak -inf, RMS -inf"
    );
}

/// The state of the View menu's item `row`.
fn menu_item(h: &Harness, row: &str) -> bl_ui::menus::ItemState {
    let menus = menu_bar(MenuPlatform::current());
    let item = menus
        .iter()
        .flat_map(bl_shell::menus::Menu::walk)
        .find(|item| item.row == row)
        .unwrap();
    item_state(item, &h.shell, &h.keymap, Platform::current())
}

// Spec: mixer-view R11, R12.
#[test]
fn the_status_bar_toggle_and_its_menu_show_and_hide_the_mixer() {
    let mut h = harness();
    let nodes = tree(&mut h);
    let toggle = Harness::node(&nodes, "Show/Hide Mixer");
    assert!(!toggle.disabled && toggle.value == "hidden");
    click(&mut h, rect_of(toggle).center(), Modifiers::NONE);
    assert!(state(&mut h.shell).arrangement.shown);
    let nodes = tree(&mut h);
    assert_eq!(Harness::node(&nodes, "Show/Hide Mixer").value, "shown");
    // The menu arrow lists the Mixer Controls choices in the View menu's
    // order.
    let arrow = rect_of(Harness::node(&nodes, "Mixer Config Menu"));
    click(&mut h, arrow.center(), Modifiers::NONE);
    let nodes = tree(&mut h);
    let mut items: Vec<(f32, String, bool)> = nodes
        .iter()
        .filter(|n| {
            n.role == Role::Button
                && [
                    "In/Out",
                    "Sends",
                    "Volume",
                    "Track Options",
                    "Crossfader",
                    "Performance Impact",
                    "Return Tracks",
                ]
                .iter()
                .any(|label| n.label.trim_start_matches("✔ ").starts_with(label))
        })
        .map(|n| (rect_of(n).top(), n.label.clone(), n.disabled))
        .collect();
    items.sort_by(|a, b| a.0.total_cmp(&b.0));
    let labels: Vec<String> = items
        .iter()
        .map(|(_, l, _)| {
            let l = l.trim_start_matches("✔ ");
            [
                "In/Out",
                "Sends",
                "Volume",
                "Track Options",
                "Crossfader",
                "Performance Impact",
                "Return Tracks",
            ]
            .iter()
            .find(|name| l.starts_with(**name))
            .unwrap()
            .to_string()
        })
        .collect();
    assert_eq!(
        labels,
        [
            "In/Out",
            "Sends",
            "Volume",
            "Track Options",
            "Crossfader",
            "Performance Impact",
            "Return Tracks"
        ]
    );
    // Track Options, Crossfader, and Performance Impact wait.
    assert!(items[3].2 && items[4].2 && items[5].2 && !items[1].2);
    let sends = items[1].0;
    let at = nodes
        .iter()
        .find(|n| n.label.trim_start_matches("✔ ").starts_with("Sends") && n.role == Role::Button)
        .map(|n| rect_of(n).center())
        .unwrap();
    let _ = sends;
    click(&mut h, at, Modifiers::NONE);
    assert!(!state(&mut h.shell).sends);
}

// Spec: mixer-view R11, R12.
#[test]
fn the_view_menu_and_keys_show_and_hide_the_mixer_and_its_sections() {
    let mut h = harness();
    let item = menu_item(&h, "M-VIEW-031");
    assert_eq!(
        (item.label.as_str(), item.enabled, item.checked),
        ("Mixer", true, Some(false))
    );
    assert!(item.chord.is_some(), "Cmd-Option-M shows beside it");
    let both = command() | Modifiers::ALT;
    key(&mut h, Key::M, both);
    assert!(state(&mut h.shell).arrangement.shown);
    assert_eq!(menu_item(&h, "M-VIEW-031").checked, Some(true));
    for (row, key_) in [
        ("M-VIEW-032a", Key::I),
        ("M-VIEW-032b", Key::S),
        ("M-VIEW-032g", Key::R),
    ] {
        assert_eq!(menu_item(&h, row).checked, Some(true), "{row}");
        key(&mut h, key_, both);
        assert_eq!(menu_item(&h, row).checked, Some(false), "{row}");
    }
    let volume = menu_item(&h, "M-VIEW-032c");
    assert!(volume.enabled && volume.checked == Some(true));
    for row in ["M-VIEW-032d", "M-VIEW-032e", "M-VIEW-032f", "M-VIEW-033a"] {
        assert!(!menu_item(&h, row).enabled, "{row} waits");
    }
    let shown = state(&mut h.shell);
    assert!(!shown.in_out && !shown.sends && !shown.return_tracks && shown.volume);
    key(&mut h, Key::M, both);
    assert!(!state(&mut h.shell).arrangement.shown);
}

// Spec: mixer-view R13.
#[test]
fn option_shift_m_moves_the_focus_to_the_mixer() {
    let mut h = harness();
    let t = tracks(&h.shell);
    let navigate = menu_item(&h, "M-NAV-010");
    assert!(navigate.enabled);
    key(&mut h, Key::M, Modifiers::ALT | Modifiers::SHIFT);
    h.settle();
    assert!(state(&mut h.shell).arrangement.shown);
    let focused = h.ctx.memory(egui::Memory::focused);
    assert_eq!(
        focused,
        Some(volume_field_id(Place::Mixer(MixerOf::Arrangement), t[0]))
    );
    key(&mut h, Key::ArrowUp, Modifiers::NONE);
    assert!((controls::gain_db(mixer(&h.shell, t[0]).volume) - 1.0).abs() < 1e-9);
    // With a track selected, its strip.
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [t[2]] }))
        .unwrap();
    h.shell.set_focused_area(bl_shell::Area::MainView);
    key(&mut h, Key::M, Modifiers::ALT | Modifiers::SHIFT);
    h.settle();
    assert_eq!(
        h.ctx.memory(egui::Memory::focused),
        Some(volume_field_id(Place::Mixer(MixerOf::Arrangement), t[2]))
    );
    // In the Session View, Navigate ▸ Mixer is disabled, as Live shows it.
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    assert!(!menu_item(&h, "M-NAV-010").enabled);
}

/// A set with `n` audio tracks more than the default set's.
fn many_tracks(h: &mut Harness, n: usize) {
    for _ in 0..n {
        h.shell
            .invoke("track.create", json!({ "kind": "audio" }))
            .unwrap();
    }
    h.settle();
}

// Spec: mixer-view R14.
#[test]
fn the_wheel_scrolls_the_mixer() {
    let mut h = session();
    many_tracks(&mut h, 20);
    let mixer = rect_of(Harness::node(&tree(&mut h), "Session Mixer"));
    let over = mixer.center();
    h.frame(vec![Event::PointerMoved(over)]);
    for _ in 0..10 {
        h.frame(vec![Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, -40.0),
            modifiers: Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        }]);
    }
    h.settle();
    let scroll = state(&mut h.shell).session.scroll;
    assert!(scroll > 100.0, "{scroll}");
    let nodes = tree(&mut h);
    assert!(
        inside(&nodes, "Session Mixer", "1 MIDI").is_none(),
        "scrolled out of view"
    );
    // The returns and Main stay put.
    let main = rect_of(inside(&nodes, "Session Mixer", "Main").unwrap());
    assert_eq!(main.right(), 1591.0);
}

// Spec: mixer-view R15.
#[test]
fn every_mixer_control_reaches_assistive_technology() {
    let mut h = session();
    let nodes = tree(&mut h);
    for column in ["1 MIDI", "4 Audio", "A Return", "Main"] {
        assert_eq!(
            nodes
                .iter()
                .find(|n| n.label == column && n.role == Role::Group)
                .map(|n| n.role),
            Some(Role::Group),
            "{column}"
        );
        for (label, role) in [
            ("Track Volume", Role::Slider),
            ("Track Panning", Role::Slider),
            ("Track Volume Fader", Role::Slider),
            ("Track Meter", Role::Meter),
        ] {
            let node = inside(&nodes, column, label).unwrap_or_else(|| panic!("{column}: {label}"));
            assert_eq!(node.role, role, "{column}: {label}");
            assert!(
                !node.disabled && !node.value.is_empty(),
                "{column}: {label}"
            );
        }
    }
    let volume = inside(&nodes, "1 MIDI", "Track Volume").unwrap();
    assert_eq!(volume.value, "0.0 dB");
    assert_eq!(
        inside(&nodes, "1 MIDI", "Track Panning").unwrap().value,
        "C"
    );
    let solo = inside(&nodes, "1 MIDI", "Solo").unwrap();
    assert_eq!((solo.role, solo.value.as_str()), (Role::Button, "off"));
    let send = inside(&nodes, "1 MIDI", "Send A").unwrap();
    assert!(send.disabled && send.description.contains("WP-2.19"));
    let monitor = inside(&nodes, "1 MIDI", "Monitor").unwrap();
    assert!(monitor.disabled && monitor.description.contains("WP-2.13"));
    // A screen reader's increment steps the volume.
    let t = tracks(&h.shell);
    h.accesskit(
        volume_field_id(Place::Mixer(MixerOf::Session), t[0]),
        Action::Increment,
    );
    h.settle();
    assert!((controls::gain_db(mixer(&h.shell, t[0]).volume) - 1.0).abs() < 1e-9);
    h.accesskit(
        fader_id(Place::Mixer(MixerOf::Session), t[0]),
        Action::Decrement,
    );
    h.settle();
    assert!(controls::gain_db(mixer(&h.shell, t[0]).volume).abs() < 1e-9);
}

// Spec: mixer-view R16.
#[test]
fn every_mixer_control_is_reachable_from_the_agent_api() {
    let mut h = session();
    let t = tracks(&h.shell);
    // Every command a control runs is registered with a schema, a
    // description, and a permission class, and an agent runs it by ID.
    for id in [
        "mixer.set_volume",
        "mixer.set_pan",
        "mixer.set_mute",
        "mixer.set_solo",
        "mixer.solo_exclusive",
        "mixer.set_arm",
        "mixer.arm_exclusive",
    ]
    .iter()
    .chain(ids::IDS)
    {
        let spec = h.shell.registry().get(id).unwrap_or_else(|| panic!("{id}"));
        assert!(
            !spec.description.is_empty() && spec.permission.is_some() && spec.args.is_some(),
            "{id}"
        );
    }
    for (id, args) in [
        ("mixer.set_volume", json!({ "track": t[0], "volume": 0.5 })),
        ("mixer.set_pan", json!({ "track": t[0], "pan": -1.0 })),
        ("mixer.solo_exclusive", json!({ "track": t[1] })),
        ("mixer.arm_exclusive", json!({ "track": t[2] })),
        ("mixer.set_mute", json!({ "track": t[3], "mute": true })),
        (ids::IN_OUT, json!({ "shown": false })),
    ] {
        h.shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id}: {e}"));
    }
    // What the agent changed is what the window shows.
    let nodes = tree(&mut h);
    assert_eq!(
        inside(&nodes, "1 MIDI", "Track Volume").unwrap().value,
        "-6.0 dB"
    );
    assert_eq!(
        inside(&nodes, "1 MIDI", "Track Panning").unwrap().value,
        "50L"
    );
    assert_eq!(inside(&nodes, "2 MIDI", "Solo").unwrap().value, "on");
    assert_eq!(
        inside(&nodes, "3 Audio", "Arm Recording").unwrap().value,
        "on"
    );
    assert_eq!(
        inside(&nodes, "4 Audio", "Track Activator").unwrap().value,
        "off"
    );
    assert!(inside(&nodes, "1 MIDI", "Input Type").is_none());
    // And a control's change is the agent's command.
    let doc = h.shell.editor().document().clone();
    let change = controls::moved(Control::Volume, &[(t[0], 0.5)], 0.1);
    assert_eq!(change[0].id, "mixer.set_volume");
    let click = controls::clicked(&doc, controls::Switch::Solo, t[0], &[], false);
    assert_eq!(click[0].id, "mixer.solo_exclusive");
}

// Spec: mixer-view R17.
#[test]
fn only_the_columns_in_view_are_built() {
    let mut h = session();
    many_tracks(&mut h, 196);
    assert_eq!(h.shell.mixer_columns().len(), 200);
    h.settle();
    let built = h.main.mixer().built_columns();
    // About ten track columns fit before the returns, plus two returns and
    // Main.
    assert!(built <= 15, "{built}");
    h.shell
        .run(ids::SCROLL, json!({ "view": "session", "x": 94.0 * 150.0 }))
        .unwrap();
    let nodes = tree(&mut h);
    let first = nodes
        .iter()
        .filter(|n| n.role == Role::Group && n.label.ends_with(" Audio"))
        .filter_map(|n| n.bounds.map(|b| (b.x0, n.label.clone())))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .unwrap();
    assert_eq!(first.0, 320.0);
    assert!(h.main.mixer().built_columns() <= 15);
}

/// Every row the mixers place at a content size, at 100%.
fn mixer_rows(size: Vec2) -> Vec<Row> {
    let shell = ShellLayout::compute(size, true);
    let session = SessionLayout::compute(shell.main_view, Some(Sections::DEFAULT));
    let mut rows = session.rows();
    rows.extend(MixerLayout::compute(session.mixer.unwrap(), 4, 2, Sections::DEFAULT, 0.0).rows());
    let arrangement = ArrangementLayout::compute_with_mixer(shell.main_view, true, 3);
    rows.extend(arrangement.rows());
    let (_, mixer) = arrangement_split(shell.main_view);
    rows.extend(MixerLayout::compute(mixer, 4, 2, Sections::DEFAULT, 0.0).rows());
    rows
}

// Spec: mixer-view R19 (interface-zoom R1).
#[test]
fn mixer_rows_keep_their_places_at_every_zoom() {
    let at_100 = mixer_rows(REFERENCE);
    for percent in ZOOM_STEPS {
        let zoom = zoom_factor(percent);
        // A window `zoom` times as large lays out the same layout points.
        let size = REFERENCE * zoom / zoom;
        let zoomed: Vec<Row> = mixer_rows(size)
            .into_iter()
            .map(|r| r.scale(zoom))
            .collect();
        assert_eq!(zoomed.len(), at_100.len());
        for (a, b) in at_100.iter().zip(&zoomed) {
            let scaled = a.scale(zoom).rect;
            assert!(
                (scaled.min - b.rect.min).length() < 1e-2
                    && (scaled.max - b.rect.max).length() < 1e-2,
                "{percent}%: {}",
                a.name
            );
        }
        // Drawn: the Session mixer's region is the 100% one times the zoom.
        let mut h = Harness::with_title_bar(Window::Main);
        h.shell
            .run("view.set_zoom", json!({ "percent": percent }))
            .unwrap();
        h.shell
            .run("view.toggle_session_arrangement", json!({}))
            .unwrap();
        h.size = vec2(1600.0 * zoom, (1000.0 - TITLE_BAR) * zoom + TITLE_BAR);
        let nodes = tree(&mut h);
        let drawn = rect_of(Harness::node(&nodes, "Session Mixer"));
        let top = TITLE_BAR / zoom;
        let expected = ShellLayout::compute(REFERENCE, true).main_view;
        assert!(
            (drawn.top() - (472.0 - TITLE_BAR + top)).abs() < 0.1,
            "{percent}%: {drawn:?}"
        );
        assert!((drawn.left() - expected.left()).abs() < 0.1, "{percent}%");
        assert!(
            (drawn.bottom() - (expected.bottom() + top)).abs() < 0.1,
            "{percent}%"
        );
    }
    // Nothing in the mixers overlaps what it doesn't at 100%: the sections
    // stack, the columns sit side by side.
    let session_mixer: Vec<&Row> = at_100
        .iter()
        .filter(|r| r.id.starts_with("L-MIX"))
        .collect();
    for pair in session_mixer.windows(2) {
        let (a, b) = (pair[0].rect, pair[1].rect);
        if a.top() < 700.0 && b.top() < 700.0 {
            assert!(a.bottom() <= b.top() + 1e-3 || b.bottom() <= a.top() + 1e-3);
        }
    }
}

// Spec: mixer-view R1 to R5.
#[test]
fn every_cited_row_exists() {
    let Some(text) = layout_md() else {
        return;
    };
    let mut cited: Vec<&str> = mixer_rows(REFERENCE).iter().map(|r| r.id).collect();
    cited.extend([
        "L-STAT-008",
        "L-ARR-006",
        "L-ARR-009",
        "L-MIX-004",
        "L-MIX-009",
    ]);
    for row in cited {
        assert!(
            text.contains(&format!("| {row} |")),
            "{row} isn't in layout.md"
        );
    }
}

/// The scripted drag the golden render plays (R18): the first track's
/// fader in the Session mixer, pressed on its handle, pulled down quickly,
/// then up slowly past where it began, then down a little, at 60 frames a
/// second, with the volume the set holds after every frame.
fn fader_gesture() -> Vec<f64> {
    let mut h = session();
    let t = tracks(&h.shell);
    let fader = control(&mut h, "1 MIDI", "Track Volume Fader");
    let handle = pos2(
        fader.center().x,
        fader.bottom() - fader.height() * controls::fader_position(1.0) as f32,
    );
    let mut steps = vec![vec2(0.0, 3.0); 12];
    steps.extend(vec![vec2(0.0, -1.5); 30]);
    steps.extend(vec![vec2(0.0, 0.0); 6]);
    steps.extend(vec![vec2(0.0, 2.0); 8]);
    let volumes = drag(&mut h, handle, &steps, Modifiers::NONE, |h| {
        mixer(&h.shell, t[0]).volume
    });
    assert_eq!(h.shell.editor().undo_history().len(), 1, "one undo step");
    volumes
}

/// Where the recorded gesture lives, for the golden render's fixture.
fn gesture_file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/ui/fader-gesture.json")
}

// Spec: mixer-view R18. The gesture the `fader-gesture` golden render plays
// is the one this drag makes through the window and the shell. Rewrite it
// with `BL_UPDATE_GESTURES=1 cargo test -p bl-ui --test mixer`, then remake
// the reference with `cargo xtask golden --update fader-gesture`.
#[test]
fn a_fader_drag_runs_the_recorded_gesture() {
    let volumes = fader_gesture();
    // It moves: down, then up past 0 dB, and every frame's volume is a
    // fader setting.
    let lowest = volumes.iter().copied().fold(f64::INFINITY, f64::min);
    let highest = volumes.iter().copied().fold(0.0, f64::max);
    assert!(controls::gain_db(lowest) < -3.0 && controls::gain_db(highest) > 0.5);
    let recorded = json!({
        "about": "The first track's volume after each 60 Hz frame of a fader drag in the Session \
                  mixer, made by crates/ui/tests/mixer.rs (fader_gesture); the fader-gesture \
                  golden fixture plays it.",
        "frame_rate": 60,
        "volumes": volumes,
    });
    let path = gesture_file();
    if std::env::var_os("BL_UPDATE_GESTURES").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&recorded).unwrap() + "\n",
        )
        .unwrap();
    }
    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("the recorded gesture"))
            .unwrap();
    assert_eq!(
        stored["volumes"], recorded["volumes"],
        "the drag no longer makes the recorded gesture; rewrite it (see this test's comment)"
    );
}

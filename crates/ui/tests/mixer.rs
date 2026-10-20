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
use bl_model::{ChainId, CrossfadeAssign, DeviceId, TrackId};
use bl_shell::menus::{MenuPlatform, menu_bar};
use bl_shell::mixer_controls::{self as controls, Control};
use bl_shell::mixer_view::{MixerOf, MixerViewState};
use bl_shell::mixer_view_commands as ids;
use bl_shell::settings::zoom_factor;
use bl_shell::{Shell, ZOOM_STEPS};
use bl_ui::keymap::Platform;
use bl_ui::layout::mixer::{MixerLayout, Sections, SessionLayout, TrackColumn, arrangement_mixer};
use bl_ui::layout::{ArrangementLayout, REFERENCE, Row, ShellLayout, TITLE_BAR};
use bl_ui::menus::item_state;
use bl_ui::mixer::{Place, column_id, fader_id, volume_field_id};
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
/// A group (a column or a mixer) comes first where a control shares its
/// name, as each send shares its return track's (observed in Live 12.4.6).
fn inside<'a>(nodes: &'a [Node], within: &str, label: &str) -> Option<&'a Node> {
    let named = |name: &str| {
        let mut matching: Vec<&'a Node> = nodes.iter().filter(|n| n.label == name).collect();
        matching.sort_by_key(|n| n.role != Role::Group);
        matching
    };
    let parent = named(within).first()?.id;
    named(label)
        .into_iter()
        .find(|n| Harness::descends_from(nodes, n.id, parent))
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

/// A strip control's place relative to its strip section's top-left
/// corner, as the observations give them.
fn relative(rect: Rect, column: Rect, strip_top: f32) -> (f32, f32, f32, f32) {
    (
        rect.left() - column.left(),
        rect.top() - strip_top,
        rect.right() - column.left(),
        rect.bottom() - strip_top,
    )
}

/// The default set's strip section top in the Session mixer: the main
/// view's bottom (766) less the colour strip (6.5) and the strip (123).
const STRIP_TOP: f32 = 766.0 - 6.5 - 123.0;

// Spec: mixer-view R1.
#[test]
fn the_session_mixer_sits_where_live_puts_it() {
    let mut h = session();
    let nodes = tree(&mut h);
    let mixer = rect_of(Harness::node(&nodes, "Session Mixer"));
    // L-SES-009 under the status row, to the main view's bottom (766): the
    // sections 115 + 56 + 123 points and the 6.5-point colour strip.
    assert_eq!(
        (mixer.left(), mixer.top(), mixer.right(), mixer.bottom()),
        (320.0, 766.0 - 300.5, 1591.0, 766.0)
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
    // The sends' 56 points over the strip, In/Out's 115 over them.
    let send = rect_of(inside(&nodes, "3 Audio", "A-Return").unwrap());
    assert!(send.top() >= STRIP_TOP - 56.0 && send.bottom() <= STRIP_TOP);
    let input = rect_of(inside(&nodes, "3 Audio", "Input Type").unwrap());
    assert!(input.top() >= STRIP_TOP - 171.0 && input.bottom() <= STRIP_TOP - 56.0);
}

// Spec: mixer-view R3. Where Live 12.4.6 has each control, relative to the
// strip section's top-left corner (observed, ±1 point).
#[test]
fn every_strip_has_its_controls_where_live_has_them() {
    let mut h = session();
    let nodes = tree(&mut h);
    let column = rect_of(inside(&nodes, "Session Mixer", "3 Audio").unwrap());
    let get = |label: &str| {
        let rect = rect_of(inside(&nodes, "3 Audio", label).unwrap_or_else(|| panic!("{label}")));
        relative(rect, column, STRIP_TOP)
    };
    // The top-left field is the peak level readout, not the volume.
    assert_eq!(get("Peak Level"), (8.0, 13.0, 42.0, 24.0));
    let peak = inside(&nodes, "3 Audio", "Peak Level").unwrap();
    assert_eq!(peak.value, "-inf dB");
    // Pan centred at (25, 46), about 25 points across.
    let (l, t, r, b) = get("Track Pan");
    assert_eq!(((l + r) / 2.0, (t + b) / 2.0), (25.0, 46.0));
    assert!((r - l - 25.0).abs() <= 1.0);
    // The activator, Solo, and Arm in a column at the left.
    assert_eq!(get("Track Activator"), (15.0, 64.0, 36.0, 85.0));
    assert_eq!(get("Solo"), (15.0, 91.0, 36.0, 102.5));
    assert_eq!(get("Arm Recording"), (15.0, 106.0, 36.0, 119.0));
    // The fader's handle column left of the 11-point meter.
    let fader = get("Track Volume");
    assert_eq!((fader.0, fader.2), (46.0, 59.0));
    assert_eq!(get("Track Meter"), (60.0, 11.0, 71.0, 119.0));
    // No volume number field in the strip: the only Track Volume is the
    // fader.
    let volumes = nodes
        .iter()
        .filter(|n| {
            n.label == "Track Volume"
                && Harness::descends_from(
                    &nodes,
                    n.id,
                    inside(&nodes, "Session Mixer", "3 Audio").unwrap().id,
                )
        })
        .count();
    assert_eq!(volumes, 1);
    // In/Out in Live's order.
    let rows: Vec<f32> = [
        "Input Type",
        "Input Channel",
        "Monitoring",
        "Output Type",
        "Output Channel",
    ]
    .iter()
    .map(|label| get(label).1)
    .collect();
    assert!(rows.windows(2).all(|w| w[0] < w[1]), "{rows:?}");
    // A MIDI track without an instrument: a MIDI meter and the switches,
    // no volume or pan.
    assert!(inside(&nodes, "1 MIDI", "MIDI Meter").is_some());
    assert!(inside(&nodes, "1 MIDI", "Track Volume").is_none());
    assert!(inside(&nodes, "1 MIDI", "Track Pan").is_none());
    assert!(inside(&nodes, "1 MIDI", "Arm Recording").is_some());
    // A return shows its letter and has no Arm; Main has no activator or
    // Solo of its own, but the cue volume and the Solo/Cue button.
    let activator = inside(&nodes, "A Return", "Track Activator").unwrap();
    assert!(activator.description.starts_with("Track A"));
    assert!(inside(&nodes, "A Return", "Arm Recording").is_none());
    assert!(inside(&nodes, "A Return", "Solo").is_some());
    assert!(inside(&nodes, "Main", "Track Activator").is_none());
    assert!(inside(&nodes, "Main", "Solo").is_none());
    // The Cue Volume knob works; the Solo/Cue button is dimmed and reads
    // Solo while Cue Out and Main Out share 1/2, as in a new set (observed).
    assert!(!inside(&nodes, "Main", "Cue Volume").unwrap().disabled);
    let button = inside(&nodes, "Main", "Solo/Cue Mode").unwrap();
    assert!(button.disabled && button.value == "Solo");
    assert!(inside(&nodes, "Main", "Track Volume").is_some());
    // Each return's Pre/Post switch, on Main's column, Post in a new set
    // (observed).
    let pre_post = inside(&nodes, "Main", "Return A Pre/Post").unwrap();
    assert!(!pre_post.disabled && pre_post.value == "Post");
}

// Spec: mixer-view R3, R5. A group's strip has the activator and Solo but
// no Arm, its In/Out only Audio To; it takes a number in the sequence.
#[test]
fn a_group_has_its_number_its_switches_and_audio_to() {
    let mut h = session();
    let t = tracks(&h.shell);
    let group: TrackId = h.shell.new_id();
    h.shell
        .run(
            "track.create",
            json!({ "id": group, "kind": "group", "name": "3 Group", "index": 2 }),
        )
        .unwrap();
    for (i, track) in [t[2], t[3]].iter().enumerate() {
        h.shell
            .run(
                "track.move",
                json!({ "track": track, "parent": group, "index": i }),
            )
            .unwrap();
    }
    let nodes = tree(&mut h);
    let number = |name: &str| {
        inside(&nodes, name, "Track Activator")
            .unwrap()
            .description
            .split(':')
            .next()
            .unwrap()
            .to_owned()
    };
    assert_eq!(number("3 Group"), "Track 3");
    assert_eq!(number("3 Audio"), "Track 4");
    assert_eq!(number("4 Audio"), "Track 5");
    assert!(inside(&nodes, "3 Group", "Solo").is_some());
    assert!(inside(&nodes, "3 Group", "Arm Recording").is_none());
    assert!(inside(&nodes, "3 Group", "Input Type").is_none());
    assert!(inside(&nodes, "3 Group", "Output Type").is_some());
    // F3 switches the group's activator: F1 to F8 count it.
    key(&mut h, Key::F3, Modifiers::NONE);
    assert!(mixer(&h.shell, group).mute);
}

// Spec: mixer-view R1, R12.
#[test]
fn hiding_sections_and_the_mixer_gives_the_slots_their_height() {
    let mut h = session();
    h.shell.run(ids::IN_OUT, json!({})).unwrap();
    let nodes = tree(&mut h);
    let mixer = rect_of(Harness::node(&nodes, "Session Mixer"));
    assert_eq!(mixer.top(), 766.0 - 6.5 - 56.0 - 123.0);
    assert!(inside(&nodes, "3 Audio", "Input Type").is_none());
    h.shell.run(ids::RETURN_TRACKS, json!({})).unwrap();
    let nodes = tree(&mut h);
    assert!(inside(&nodes, "Session Mixer", "A Return").is_none_or(|n| n.role != Role::Group));
    h.shell.run(ids::TOGGLE, json!({})).unwrap();
    let nodes = tree(&mut h);
    assert!(nodes.iter().all(|n| n.label != "Session Mixer"));
}

// Spec: mixer-view R12, R23.
#[test]
fn the_crossfader_section_shows_the_assign_buttons_and_main_s_fader() {
    let mut h = session();
    // Hidden by default (observed): no buttons, no fader.
    let nodes = tree(&mut h);
    assert!(nodes.iter().all(|n| n.label != "Crossfade A"));
    assert!(nodes.iter().all(|n| n.label != "Crossfader"));
    assert!(!state(&mut h.shell).crossfader);

    h.shell.run(ids::CROSSFADER, json!({})).unwrap();
    let nodes = tree(&mut h);
    // The 28-point section under the strips, over the colour strip.
    let mixer_rect = rect_of(Harness::node(&nodes, "Session Mixer"));
    assert_eq!(mixer_rect.top(), 766.0 - 300.5 - 28.0);
    // A track and a return have A and B buttons, both off in a new set.
    for column in ["3 Audio", "4 Audio", "A Return"] {
        for side in ["A", "B"] {
            let label = format!("Crossfade {side}");
            let button = inside(&nodes, column, &label).unwrap_or_else(|| panic!("{column}"));
            assert_eq!((button.role, button.value.as_str()), (Role::Button, "off"));
            let column_rect = rect_of(inside(&nodes, "Session Mixer", column).unwrap());
            assert!(column_rect.contains_rect(rect_of(button)));
        }
    }
    // A MIDI track without an instrument gets none (observed), and Main
    // has the crossfader instead, centred in a new set.
    assert!(inside(&nodes, "1 MIDI", "Crossfade A").is_none());
    assert!(inside(&nodes, "Main", "Crossfade A").is_none());
    let fader = inside(&nodes, "Main", "Crossfader").unwrap();
    assert_eq!((fader.role, fader.value.as_str()), (Role::Slider, "C"));

    // A click assigns the clicked side; the sides are exclusive; clicking
    // the lit side releases the track (observed).
    let t = tracks(&h.shell);
    let a = control(&mut h, "3 Audio", "Crossfade A");
    click(&mut h, a.center(), Modifiers::NONE);
    assert_eq!(mixer(&h.shell, t[2]).crossfade, CrossfadeAssign::A);
    assert_eq!(
        h.shell.editor().undo_history().next(),
        Some("Assign crossfade")
    );
    assert_eq!(
        inside(&tree(&mut h), "3 Audio", "Crossfade A")
            .unwrap()
            .value,
        "on"
    );
    let b = control(&mut h, "3 Audio", "Crossfade B");
    click(&mut h, b.center(), Modifiers::NONE);
    assert_eq!(mixer(&h.shell, t[2]).crossfade, CrossfadeAssign::B);
    click(&mut h, b.center(), Modifiers::NONE);
    assert_eq!(mixer(&h.shell, t[2]).crossfade, CrossfadeAssign::None);

    // A drag moves the crossfader relative to the pointer, as one undo
    // step, and the Status Bar reads its position while it moves.
    let before = h.shell.editor().undo_history().len();
    let fader = control(&mut h, "Main", "Crossfader");
    let positions = drag(
        &mut h,
        fader.center(),
        &[vec2(10.0, 0.0); 4],
        Modifiers::NONE,
        |h| h.shell.editor().document().settings().crossfader,
    );
    assert_eq!(positions[0], 0.5, "a press doesn't move it");
    assert!(*positions.last().unwrap() > 0.5, "{positions:?}");
    assert_eq!(h.shell.editor().undo_history().len(), before + 1);
    assert_eq!(
        h.shell.editor().undo_history().next(),
        Some(controls::CROSSFADER_GESTURE)
    );
    // And the fader shows where it now is: the same drag's readout.
    let held = Event::ModifiersChanged(Modifiers::NONE);
    h.frame(vec![held.clone(), Event::PointerMoved(fader.center())]);
    h.frame(vec![Event::PointerButton {
        pos: fader.center(),
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    let mut readouts = Vec::new();
    let mut x = fader.center().x;
    for _ in 0..4 {
        x += 6.0;
        let output = h.frame(vec![Event::PointerMoved(pos2(x, fader.center().y))]);
        readouts.push(
            Harness::node(&Harness::nodes(&output), "Status")
                .value
                .clone(),
        );
    }
    let now = h.shell.editor().document().settings().crossfader;
    assert!(
        readouts
            .last()
            .is_some_and(|r| *r == controls::crossfader_readout(now)),
        "{readouts:?}"
    );
    assert!(
        readouts
            .iter()
            .skip_while(|r| r.is_empty())
            .all(|r| r.starts_with("Crossfade: ")),
        "{readouts:?}"
    );
    h.frame(vec![Event::PointerButton {
        pos: pos2(x, fader.center().y),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);

    // Hiding the section again takes the buttons and the fader away.
    h.shell.run(ids::CROSSFADER, json!({})).unwrap();
    let nodes = tree(&mut h);
    assert!(
        nodes
            .iter()
            .all(|n| n.label != "Crossfade A" && n.label != "Crossfader")
    );
    assert!(!state(&mut h.shell).crossfader);
}

// Spec: mixer-view R2.
#[test]
fn the_arrangement_mixer_is_the_session_mixer_under_the_lanes() {
    let mut h = harness();
    let before = rect_of(Harness::node(&tree(&mut h), "Track Lanes"));
    h.shell.run(ids::TOGGLE, json!({})).unwrap();
    let nodes = tree(&mut h);
    let mixer = rect_of(Harness::node(&nodes, "Arrangement Mixer"));
    // Its sections start where the Session mixer's do, under a 17-point
    // row of title bars; it runs to the main view's bottom.
    assert_eq!(
        (mixer.left(), mixer.top(), mixer.right(), mixer.bottom()),
        (320.0, 766.0 - 300.5 - 17.0, 1591.0, 766.0)
    );
    // The same columns at the same x positions.
    for (i, name) in ["1 MIDI", "2 MIDI", "3 Audio", "4 Audio"]
        .iter()
        .enumerate()
    {
        let rect = rect_of(inside(&nodes, "Arrangement Mixer", name).unwrap());
        assert_eq!(rect.left(), 320.0 + 94.0 * i as f32, "{name}");
    }
    // The lanes, the pinned rows, and the time ruler stay above it.
    let lanes = rect_of(Harness::node(&nodes, "Track Lanes"));
    assert_eq!(lanes.top(), before.top());
    let time = rect_of(Harness::node(&nodes, "Time Ruler"));
    assert!(lanes.bottom() < time.top() && time.bottom() <= mixer.top());
    let layout = ArrangementLayout::compute_with_mixer(
        Rect::from_min_max(pos2(320.0, 61.0), pos2(1591.0, 766.0)),
        true,
        3,
        Sections::DEFAULT,
    );
    assert!(layout.pinned_headers.bottom() <= layout.time_ruler.top());
    assert_eq!(layout.mixer, Some(mixer));
    // The strips sit as in the Session.
    let column = rect_of(inside(&nodes, "Arrangement Mixer", "3 Audio").unwrap());
    let peak = rect_of(inside(&nodes, "3 Audio", "Peak Level").unwrap());
    assert_eq!(relative(peak, column, STRIP_TOP).0, 8.0);
    assert_eq!(peak.top() - STRIP_TOP, 13.0);
    // Hidden again, the lanes have their height back.
    h.shell.run(ids::TOGGLE, json!({})).unwrap();
    let lanes = rect_of(Harness::node(&tree(&mut h), "Track Lanes"));
    assert_eq!(lanes, before);
}

// Spec: mixer-view R4, R6.
#[test]
fn the_arrangement_headers_mixer_controls_are_live_and_take_typed_values() {
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
    // The volume field steps from the keyboard once clicked.
    let mut volumes: Vec<Rect> = Harness::nodes(&h.settle())
        .iter()
        .filter(|n| n.label == "Track Volume")
        .map(rect_of)
        .collect();
    volumes.sort_by(|a, b| a.top().total_cmp(&b.top()));
    click(&mut h, volumes[0].center(), Modifiers::NONE);
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert!((controls::gain_db(mixer(&h.shell, t[0]).volume) + 1.0).abs() < 1e-9);
    // Typing -12 and Return sets −12.0 dB; while typing, keys go to the
    // field.
    for k in [Key::Minus, Key::Num1, Key::Num2] {
        key(&mut h, k, Modifiers::NONE);
    }
    assert!(h.main.is_typing());
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert!((controls::gain_db(mixer(&h.shell, t[0]).volume) + 12.0).abs() < 1e-9);
    // Esc drops a typed value and gives the keyboard back.
    key(&mut h, Key::Num3, Modifiers::NONE);
    assert!(h.main.is_typing());
    key(&mut h, Key::Escape, Modifiers::NONE);
    assert!((controls::gain_db(mixer(&h.shell, t[0]).volume) + 12.0).abs() < 1e-9);
    assert!(!h.main.is_typing());
    let nodes = tree(&mut h);
    let meters = nodes.iter().filter(|n| n.label == "Track Meter").count();
    assert!(meters >= 7, "a meter in every header: {meters}");
}

// Spec: mixer-view R6.
#[test]
fn a_fader_drag_is_relative_one_undo_step_and_shows_in_the_status_bar() {
    let mut h = session();
    let t = tracks(&h.shell);
    let fader = control(&mut h, "3 Audio", "Track Volume");
    // Pressed anywhere on the fader, not on its handle: nothing jumps.
    let at = pos2(fader.center().x, fader.bottom() - 2.0);
    let before = h.shell.editor().undo_history().len();
    let mut readouts = Vec::new();
    let volumes = drag(&mut h, at, &[vec2(0.0, -1.0); 30], Modifiers::NONE, |h| {
        mixer(&h.shell, t[2]).volume
    });
    assert_eq!(volumes[0], 1.0, "a press doesn't move it");
    assert_eq!(h.shell.editor().undo_history().len(), before + 1);
    assert_eq!(
        h.shell.editor().undo_history().next(),
        Some(controls::VOLUME_GESTURE)
    );
    // Up past the top: it stops at +6 dB.
    assert!((controls::gain_db(*volumes.last().unwrap()) - 6.0).abs() < 1e-9);
    // Down: the handle follows the pointer a point per point along the
    // fader's scale, and the Status Bar shows the volume while dragging.
    let held = Event::ModifiersChanged(Modifiers::NONE);
    h.frame(vec![held.clone(), Event::PointerMoved(at)]);
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    let mut y = at.y;
    for _ in 0..12 {
        y += 1.5;
        let output = h.frame(vec![Event::PointerMoved(pos2(at.x, y))]);
        let nodes = Harness::nodes(&output);
        readouts.push(Harness::node(&nodes, "Status").value.clone());
    }
    let now = mixer(&h.shell, t[2]).volume;
    assert!(
        readouts
            .last()
            .is_some_and(|r| *r == controls::volume_readout(now)),
        "{readouts:?}"
    );
    // From the frame the drag starts (past egui's drag threshold) on.
    let shown: Vec<&String> = readouts.iter().skip_while(|r| r.is_empty()).collect();
    assert!(shown.len() >= 8, "{readouts:?}");
    assert!(
        shown.iter().all(|r| r.starts_with("Track Volume: ")),
        "{readouts:?}"
    );
    // A further 30 points down moves the handle 30 points down the scale.
    let before = controls::fader_position(mixer(&h.shell, t[2]).volume);
    for _ in 0..20 {
        y += 1.5;
        h.frame(vec![Event::PointerMoved(pos2(at.x, y))]);
    }
    let after = controls::fader_position(mixer(&h.shell, t[2]).volume);
    assert!(
        ((before - after) * controls::FADER_TRAVEL_POINTS - 30.0).abs() < 1e-6,
        "{before} {after}"
    );
    // On past the bottom: −∞, and it stays there.
    for _ in 0..80 {
        y += 1.5;
        h.frame(vec![Event::PointerMoved(pos2(at.x, y))]);
    }
    assert_eq!(mixer(&h.shell, t[2]).volume, 0.0, "−∞ at the bottom");
    h.frame(vec![Event::PointerButton {
        pos: pos2(at.x, y),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    let nodes = tree(&mut h);
    assert!(
        !Harness::node(&nodes, "Status")
            .value
            .starts_with("Track Volume")
    );
    // Pan: 0.8 steps a point, up to the right (observed): 40 points less
    // egui's drag threshold.
    let pan = control(&mut h, "3 Audio", "Track Pan");
    drag(
        &mut h,
        pan.center(),
        &[vec2(0.0, -2.0); 20],
        Modifiers::NONE,
        |_| (),
    );
    let p = mixer(&h.shell, t[2]).pan;
    assert!(p > 0.5 && p <= 0.64 + 1e-9, "{p}");
    assert_eq!(
        h.shell.editor().undo_history().next(),
        Some(controls::PAN_GESTURE)
    );
}

// Spec: mixer-view R6. Shift makes a drag about three times finer
// (observed by hand on a pan knob: 9R with Shift where 29R without).
#[test]
fn shift_makes_a_drag_three_times_finer() {
    let mut h = session();
    let t = tracks(&h.shell);
    let pan = control(&mut h, "3 Audio", "Track Pan");
    drag(
        &mut h,
        pan.center(),
        &[vec2(0.0, -2.0); 20],
        Modifiers::NONE,
        |_| (),
    );
    let plain = mixer(&h.shell, t[2]).pan;
    h.shell
        .run("mixer.set_pan", json!({ "track": t[2], "pan": 0.0 }))
        .unwrap();
    h.time += 1.0;
    drag(
        &mut h,
        pan.center(),
        &[vec2(0.0, -2.0); 20],
        Modifiers::SHIFT,
        |_| (),
    );
    let fine = mixer(&h.shell, t[2]).pan;
    assert!(fine > 0.0, "{fine}");
    assert!(
        (plain / fine - controls::FINE as f64).abs() < 1e-6,
        "{plain} {fine}"
    );
}

// Spec: mixer-view R6, R8. Two selected tracks move by the same dB, with
// the keys and with a drag (observed).
#[test]
fn selected_tracks_move_by_the_same_db() {
    let mut h = session();
    let t = tracks(&h.shell);
    for (track, db) in [(t[2], -50.0), (t[3], -40.0)] {
        h.shell
            .run(
                "mixer.set_volume",
                json!({ "track": track, "volume": controls::db_gain(db) }),
            )
            .unwrap();
    }
    h.shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [t[2], t[3]] }),
        )
        .unwrap();
    let fader = control(&mut h, "3 Audio", "Track Volume");
    click(&mut h, fader.center(), Modifiers::NONE);
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    let db = |h: &Harness, track| {
        (controls::gain_db(mixer(&h.shell, track).volume) * 10.0).round() / 10.0
    };
    assert_eq!((db(&h, t[2]), db(&h, t[3])), (-51.0, -41.0));
    let before = h.shell.editor().undo_history().len();
    // Long enough after the click that egui doesn't see a double-click.
    h.time += 1.0;
    drag(
        &mut h,
        fader.center(),
        &[vec2(0.0, -1.0); 12],
        Modifiers::NONE,
        |_| (),
    );
    assert_eq!(h.shell.editor().undo_history().len(), before + 1);
    let (a, b) = (db(&h, t[2]), db(&h, t[3]));
    assert!(a > -51.0);
    assert!(((a + 51.0) - (b + 41.0)).abs() < 0.11, "{a} {b}");
}

// Spec: mixer-view R6.
#[test]
fn double_click_delete_keys_and_the_triangle() {
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
    // A double-click on the fader resets it to 0 dB (observed).
    let fader = control(&mut h, "3 Audio", "Track Volume");
    double_click(&mut h, fader.center());
    assert_eq!(mixer(&h.shell, t[2]).volume, 1.0);
    let fader = control(&mut h, "4 Audio", "Track Volume");
    click(&mut h, fader.center(), Modifiers::NONE);
    key(&mut h, Key::Delete, Modifiers::NONE);
    assert_eq!(mixer(&h.shell, t[3]).volume, 1.0);
    let pan = control(&mut h, "3 Audio", "Track Pan");
    double_click(&mut h, pan.center());
    assert_eq!(mixer(&h.shell, t[2]).pan, 0.0);
    let triangle = control(&mut h, "4 Audio", "Reset Panning");
    click(&mut h, triangle.center(), Modifiers::NONE);
    assert_eq!(mixer(&h.shell, t[3]).pan, 0.0);
    // Up and Down step a dB, Shift a tenth; the ends as recorded.
    click(&mut h, fader.center(), Modifiers::NONE);
    key(&mut h, Key::ArrowUp, Modifiers::NONE);
    let db = |h: &Harness| (controls::gain_db(mixer(&h.shell, t[3]).volume) * 10.0).round() / 10.0;
    assert_eq!(db(&h), 1.0);
    key(&mut h, Key::ArrowUp, Modifiers::SHIFT);
    assert_eq!(db(&h), 1.1);
    for _ in 0..6 {
        key(&mut h, Key::ArrowUp, Modifiers::NONE);
    }
    assert_eq!(db(&h), 6.0, "it stops at the top");
}

// Spec: mixer-view R7; mixer S5, S10 (WP-3.18). The Solo/Cue button on
// Main's strip switches the tracks' Solo switches to Cue switches, with no
// undo step, once Cue Out and Main Out are apart; the Cue Volume knob drags,
// steps and resets, each an undo step (observed 18 October 2026).
#[test]
fn the_solo_cue_button_and_the_cue_volume_knob_work_on_main_s_strip() {
    let mut h = session();
    let t = tracks(&h.shell);
    let undo = |h: &Harness| h.shell.editor().undo_history().len();
    let cue_mode = |h: &Harness| h.shell.editor().document().settings().cue_mode;
    let volume = |h: &Harness| h.shell.editor().document().settings().cue_volume;

    // Dimmed and inert while the outputs overlap, and so are Options ▸ Solo
    // Switches and Cue Switches, with Solo Switches ticked.
    let button = control(&mut h, "Main", "Solo/Cue Mode");
    click(&mut h, button.center(), Modifiers::NONE);
    assert!(!cue_mode(&h));
    let (solo_switches, cue_switches) = (menu_item(&h, "M-OPT-023"), menu_item(&h, "M-OPT-024"));
    assert!(!solo_switches.enabled && !cue_switches.enabled);
    assert_eq!(
        (solo_switches.checked, cue_switches.checked),
        (Some(true), Some(false))
    );

    h.shell
        .run(
            "routing.set_cue_out",
            json!({ "channel": { "type": "pair", "channel": 3 } }),
        )
        .unwrap();
    let nodes = tree(&mut h);
    let node = inside(&nodes, "Main", "Solo/Cue Mode").unwrap();
    assert!(!node.disabled && node.value == "Solo");
    assert!(inside(&nodes, "3 Audio", "Solo").is_some());
    assert!(menu_item(&h, "M-OPT-023").enabled && menu_item(&h, "M-OPT-024").enabled);

    // Cue Switches from the menu is the same setting as the button.
    let steps = undo(&h);
    let item = menu_item(&h, "M-OPT-024");
    let invocation = item.invocation.unwrap();
    h.shell.run(&invocation.id, invocation.args).unwrap();
    assert!(cue_mode(&h));
    assert_eq!(
        (
            menu_item(&h, "M-OPT-023").checked,
            menu_item(&h, "M-OPT-024").checked
        ),
        (Some(false), Some(true))
    );
    click(&mut h, button.center(), Modifiers::NONE);
    assert!(!cue_mode(&h), "the button toggles back");
    assert_eq!(menu_item(&h, "M-OPT-023").checked, Some(true));
    click(&mut h, button.center(), Modifiers::NONE);
    assert!(cue_mode(&h));
    assert_eq!(undo(&h), steps, "switching the mode is no undo step");
    let nodes = tree(&mut h);
    assert_eq!(
        inside(&nodes, "Main", "Solo/Cue Mode").unwrap().value,
        "Cue"
    );
    // The tracks' Solo switches are Cue switches now, and set the same flag.
    assert!(inside(&nodes, "3 Audio", "Solo").is_none());
    let cue = control(&mut h, "3 Audio", "Cue");
    click(&mut h, cue.center(), Modifiers::NONE);
    assert!(mixer(&h.shell, t[2]).solo);
    let nodes = tree(&mut h);
    assert_eq!(inside(&nodes, "3 Audio", "Cue").unwrap().value, "on");
    click(&mut h, button.center(), Modifiers::NONE);
    assert!(!cue_mode(&h));

    // The knob: a drag up moves it along the fader's scale to +6 dB in one
    // undo step, the Status Bar names it, and a double-click resets it.
    let knob = control(&mut h, "Main", "Cue Volume");
    let steps = undo(&h);
    drag(
        &mut h,
        knob.center(),
        &[vec2(0.0, -1.0); 30],
        Modifiers::NONE,
        |_| (),
    );
    assert!((controls::gain_db(volume(&h)) - 6.0).abs() < 1e-9);
    assert_eq!(undo(&h), steps + 1);
    assert_eq!(
        h.shell.editor().undo_history().next(),
        Some(controls::CUE_VOLUME_GESTURE)
    );
    double_click(&mut h, knob.center());
    assert_eq!(volume(&h), 1.0);
    // Up and Down step a dB; Delete resets.
    click(&mut h, knob.center(), Modifiers::NONE);
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert!((controls::gain_db(volume(&h)) + 1.0).abs() < 1e-9);
    key(&mut h, Key::Delete, Modifiers::NONE);
    assert_eq!(volume(&h), 1.0);
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
        track: t[2],
        peak: [half, half * 0.5],
        rms: [half * 0.5, half * 0.25],
    }];
    h.shell.set_meters(&meters);
    let output = h.frame(vec![]);
    let nodes = Harness::nodes(&output);
    let meter = inside(&nodes, "3 Audio", "Track Meter").unwrap();
    assert_eq!(meter.role, Role::Meter);
    assert_eq!(meter.value, "peak -6.0, RMS -12.0");
    assert!((meter.numeric.unwrap() + 6.0).abs() < 1e-3);
    assert_eq!(
        inside(&nodes, "3 Audio", "Peak Level").unwrap().value,
        "-6.0 dB"
    );
    let delay = output
        .viewport_output
        .values()
        .next()
        .unwrap()
        .repaint_delay;
    assert_eq!(delay, std::time::Duration::ZERO, "the meters redraw");
    // Silent meters stop redrawing every frame.
    h.shell.set_meters(&[TrackMeter {
        track: t[2],
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
        inside(&nodes, "3 Audio", "Track Meter").unwrap().value,
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

const MIXER_CONTROLS: [&str; 7] = [
    "In/Out",
    "Sends",
    "Volume",
    "Track Options",
    "Crossfader",
    "Performance Impact",
    "Return Tracks",
];

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
    let name = |label: &str| {
        let label = label.trim_start_matches("✔ ");
        MIXER_CONTROLS
            .iter()
            .find(|name| label.starts_with(**name))
            .copied()
    };
    let mut items: Vec<(f32, &str, bool)> = nodes
        .iter()
        .filter(|n| n.role == Role::Button)
        .filter_map(|n| name(&n.label).map(|l| (rect_of(n).top(), l, n.disabled)))
        .collect();
    items.sort_by(|a, b| a.0.total_cmp(&b.0));
    let labels: Vec<&str> = items.iter().map(|(_, l, _)| *l).collect();
    assert_eq!(labels, MIXER_CONTROLS);
    // Track Options (WP-5.36) and the Crossfader section (WP-3.17) are built
    // now; Performance Impact waits, and Sends is enabled.
    assert!(!items[3].2 && !items[4].2 && items[5].2 && !items[1].2);
    let at = nodes
        .iter()
        .find(|n| n.role == Role::Button && name(&n.label) == Some("Sends"))
        .map(|n| rect_of(n).center())
        .unwrap();
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
    // In/Out ⌥⌘I, Sends ⌥⌘S, Return Tracks ⌥⌘R, each ticked in a new set;
    // Volume ticked, with no key.
    for (row, key_) in [
        ("M-VIEW-032a", Key::I),
        ("M-VIEW-032b", Key::S),
        ("M-VIEW-032g", Key::R),
    ] {
        let item = menu_item(&h, row);
        assert_eq!(item.checked, Some(true), "{row}");
        assert!(item.chord.is_some(), "{row}");
        key(&mut h, key_, both);
        assert_eq!(menu_item(&h, row).checked, Some(false), "{row}");
    }
    let volume = menu_item(&h, "M-VIEW-032c");
    assert!(volume.enabled && volume.checked == Some(true) && volume.chord.is_none());
    // The Crossfader section: built, unticked in a new set (observed:
    // hidden by default), with no key.
    let crossfader = menu_item(&h, "M-VIEW-032e");
    assert!(crossfader.enabled && crossfader.checked == Some(false) && crossfader.chord.is_none());
    h.shell.run(ids::CROSSFADER, json!({})).unwrap();
    assert_eq!(menu_item(&h, "M-VIEW-032e").checked, Some(true));
    h.shell.run(ids::CROSSFADER, json!({})).unwrap();
    // Track Options is built now (WP-5.36), unticked in a new set; the
    // Arrangement's (M-VIEW-033c) shares it.
    let track_options = menu_item(&h, "M-VIEW-032d");
    assert!(
        track_options.enabled
            && track_options.checked == Some(false)
            && track_options.chord.is_none()
    );
    assert!(menu_item(&h, "M-VIEW-033c").enabled);
    for row in ["M-VIEW-032f", "M-VIEW-033a"] {
        assert!(!menu_item(&h, row).enabled, "{row} waits");
    }
    let shown = state(&mut h.shell);
    assert!(!shown.in_out && !shown.sends && !shown.return_tracks && shown.volume);
    key(&mut h, Key::M, both);
    assert!(!state(&mut h.shell).arrangement.shown);
    // The submenus' separators, as observed: Volume | Track Options ...
    // Performance Impact | Return Tracks; and In/Out, Volume, Track Options |
    // Return Tracks.
    let menus = menu_bar(MenuPlatform::current());
    let shape = |row: &str| -> Vec<&str> {
        menus
            .iter()
            .flat_map(bl_shell::menus::Menu::walk)
            .find(|item| item.row == row)
            .unwrap()
            .items
            .iter()
            .map(|item| {
                if item.is_separator() {
                    "—"
                } else {
                    item.label
                }
            })
            .collect()
    };
    assert_eq!(
        shape("M-VIEW-032"),
        [
            "In/Out",
            "Sends",
            "Volume",
            "—",
            "Track Options",
            "Crossfader",
            "Performance Impact",
            "—",
            "Return Tracks"
        ]
    );
    assert_eq!(
        shape("M-VIEW-033"),
        ["In/Out", "Volume", "Track Options", "—", "Return Tracks"]
    );
    // Arrangement Track Controls is disabled while the Session View is in
    // front.
    assert!(menu_item(&h, "M-VIEW-033").enabled);
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    assert!(!menu_item(&h, "M-VIEW-033").enabled);
}

// Spec: mixer-view R13.
#[test]
fn option_shift_m_puts_the_focus_in_the_mixer_at_the_selected_track() {
    let mut h = harness();
    let t = tracks(&h.shell);
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [t[1]] }))
        .unwrap();
    assert!(menu_item(&h, "M-NAV-010").enabled);
    key(&mut h, Key::M, Modifiers::ALT | Modifiers::SHIFT);
    h.settle();
    assert!(state(&mut h.shell).arrangement.shown);
    assert_eq!(
        h.ctx.memory(egui::Memory::focused),
        Some(column_id(MixerOf::Arrangement, t[1]))
    );
    // Right moves the selection to the next track, and the focus with it.
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    h.settle();
    assert_eq!(h.shell.arrangement().selected_tracks(), [t[2]]);
    assert_eq!(
        h.ctx.memory(egui::Memory::focused),
        Some(column_id(MixerOf::Arrangement, t[2]))
    );
    key(&mut h, Key::ArrowLeft, Modifiers::NONE);
    h.settle();
    assert_eq!(h.shell.arrangement().selected_tracks(), [t[1]]);
    // Down changes nothing.
    let volumes: Vec<f64> = t.iter().map(|x| mixer(&h.shell, *x).volume).collect();
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    let after: Vec<f64> = t.iter().map(|x| mixer(&h.shell, *x).volume).collect();
    assert_eq!(volumes, after);
    assert_eq!(h.shell.arrangement().selected_tracks(), [t[1]]);
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
fn the_wheel_scrolls_the_mixer_continuously() {
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
    // A column can be partly in view.
    let lefts: Vec<f32> = nodes
        .iter()
        .filter(|n| n.role == Role::Group && n.label.ends_with(" Audio"))
        .filter_map(|n| n.bounds.map(|b| b.x0 as f32))
        .collect();
    assert!(
        lefts.iter().any(|x| (x - 320.0).abs() < 1e-3),
        "the first column in view starts at the mixer's left"
    );
    // The returns and Main stay put.
    let main = rect_of(inside(&nodes, "Session Mixer", "Main").unwrap());
    assert_eq!(main.right(), 1591.0);
    let a = rect_of(inside(&nodes, "Session Mixer", "A Return").unwrap());
    assert!((a.left() - 1298.0).abs() <= 2.0);
}

// Spec: mixer-view R15.
#[test]
fn every_mixer_control_reaches_assistive_technology() {
    let mut h = session();
    let nodes = tree(&mut h);
    for column in ["3 Audio", "4 Audio", "A Return", "Main"] {
        assert!(
            nodes
                .iter()
                .any(|n| n.label == column && n.role == Role::Group),
            "{column}"
        );
        // Live's tree calls Main's pan Main Pan (observed in Live 12.4.6).
        let pan = if column == "Main" {
            "Main Pan"
        } else {
            "Track Pan"
        };
        for (label, role) in [
            ("Peak Level", Role::Label),
            ("Track Volume", Role::Slider),
            (pan, Role::Slider),
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
    let volume = inside(&nodes, "3 Audio", "Track Volume").unwrap();
    assert_eq!(volume.value, "0.0 dB");
    assert_eq!(inside(&nodes, "3 Audio", "Track Pan").unwrap().value, "C");
    let solo = inside(&nodes, "3 Audio", "Solo").unwrap();
    assert_eq!((solo.role, solo.value.as_str()), (Role::Button, "off"));
    // The sends and the In/Out section work since WP-2.19; the Ext. In
    // channel and Main Out list the interface's channels, and the harness's
    // device has no input open, so there's nothing to choose (WP-2.13).
    let send = inside(&nodes, "3 Audio", "A-Return").unwrap();
    assert!(!send.disabled && send.value == "-inf dB");
    let monitor = inside(&nodes, "3 Audio", "Monitoring").unwrap();
    assert!(!monitor.disabled && monitor.value == "Auto");
    let input = inside(&nodes, "3 Audio", "Input Type").unwrap();
    assert!(!input.disabled && input.value == "Ext. In");
    let channel = inside(&nodes, "3 Audio", "Input Channel").unwrap();
    assert!(channel.disabled && channel.value == "1/2");
    let main_out = inside(&nodes, "Main", "Main Output").unwrap();
    assert!(!main_out.disabled && main_out.value == "1/2");
    // A screen reader's increment and decrement step the fader.
    let t = tracks(&h.shell);
    h.accesskit(
        fader_id(Place::Mixer(MixerOf::Session), t[2]),
        Action::Increment,
    );
    h.settle();
    assert!((controls::gain_db(mixer(&h.shell, t[2]).volume) - 1.0).abs() < 1e-9);
    h.accesskit(
        fader_id(Place::Mixer(MixerOf::Session), t[2]),
        Action::Decrement,
    );
    h.settle();
    assert!(controls::gain_db(mixer(&h.shell, t[2]).volume).abs() < 1e-9);
    let _ = volume_field_id(Place::Header, t[2]);
    // The Crossfader section's controls (WP-3.17): a track's A and B
    // buttons and Main's crossfader, each named and readable.
    h.shell.run(ids::CROSSFADER, json!({})).unwrap();
    let nodes = tree(&mut h);
    for side in ["A", "B"] {
        let label = format!("Crossfade {side}");
        let button = inside(&nodes, "3 Audio", &label).unwrap();
        assert_eq!((button.role, button.value.as_str()), (Role::Button, "off"));
        assert!(inside(&nodes, "A Return", &label).is_some());
    }
    let fader = inside(&nodes, "Main", "Crossfader").unwrap();
    assert_eq!((fader.role, fader.value.as_str()), (Role::Slider, "C"));
    assert!(!fader.disabled);
}

// Spec: mixer-view R16.
#[test]
fn every_mixer_control_is_reachable_from_the_agent_api() {
    let mut h = session();
    let t = tracks(&h.shell);
    for id in [
        "mixer.set_volume",
        "mixer.set_pan",
        "mixer.set_mute",
        "mixer.set_solo",
        "mixer.solo_exclusive",
        "mixer.set_arm",
        "mixer.arm_exclusive",
        "mixer.set_crossfader",
        "mixer.set_crossfade_assign",
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
        ("mixer.set_volume", json!({ "track": t[2], "volume": 0.5 })),
        ("mixer.set_pan", json!({ "track": t[2], "pan": -1.0 })),
        ("mixer.solo_exclusive", json!({ "track": t[1] })),
        ("mixer.arm_exclusive", json!({ "track": t[3] })),
        ("mixer.set_mute", json!({ "track": t[3], "mute": true })),
        (ids::IN_OUT, json!({ "shown": false })),
        (ids::CROSSFADER, json!({})),
        (
            "mixer.set_crossfade_assign",
            json!({ "track": t[2], "assign": "a" }),
        ),
        ("mixer.set_crossfader", json!({ "position": 0.75 })),
    ] {
        h.shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id}: {e}"));
    }
    let nodes = tree(&mut h);
    assert_eq!(
        inside(&nodes, "3 Audio", "Track Volume").unwrap().value,
        "-6.0 dB"
    );
    assert_eq!(inside(&nodes, "3 Audio", "Track Pan").unwrap().value, "50L");
    assert_eq!(inside(&nodes, "2 MIDI", "Solo").unwrap().value, "on");
    assert_eq!(
        inside(&nodes, "4 Audio", "Arm Recording").unwrap().value,
        "on"
    );
    assert_eq!(
        inside(&nodes, "4 Audio", "Track Activator").unwrap().value,
        "off"
    );
    assert!(inside(&nodes, "3 Audio", "Input Type").is_none());
    // The section's controls follow the commands: the A button lit, and
    // the fader 25 of 50 steps towards B.
    assert_eq!(
        inside(&nodes, "3 Audio", "Crossfade A").unwrap().value,
        "on"
    );
    assert_eq!(
        inside(&nodes, "Main", "Crossfader").unwrap().value,
        controls::crossfader_text(0.75)
    );
    let doc = h.shell.editor().document().clone();
    let change = controls::moved(Control::Volume, &[(t[2], 0.5)], 1.0);
    assert_eq!(change[0].id, "mixer.set_volume");
    let click = controls::clicked(&doc, controls::Switch::Solo, t[2], &[], false);
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
    let shell = ShellLayout::compute(size, true, true);
    let session = SessionLayout::compute(shell.main_view, Some(Sections::DEFAULT));
    let mut rows = session.rows();
    rows.extend(
        MixerLayout::compute(
            session.mixer.unwrap(),
            &[TrackColumn::track(); 4],
            2,
            Sections::DEFAULT,
            0.0,
        )
        .rows(),
    );
    let arrangement =
        ArrangementLayout::compute_with_mixer(shell.main_view, true, 3, Sections::DEFAULT);
    rows.extend(arrangement.rows());
    let mixer = arrangement_mixer(shell.main_view, Sections::DEFAULT);
    rows.extend(
        MixerLayout::compute(
            mixer.with_min_y(mixer.top() + 17.0),
            &[TrackColumn::track(); 4],
            2,
            Sections::DEFAULT,
            0.0,
        )
        .rows(),
    );
    rows
}

// Spec: mixer-view R19 (interface-zoom R1).
#[test]
fn mixer_rows_keep_their_places_at_every_zoom() {
    let at_100 = mixer_rows(REFERENCE);
    for percent in ZOOM_STEPS {
        let zoom = zoom_factor(percent);
        // A window `zoom` times as large lays out the same layout points.
        let zoomed: Vec<Row> = mixer_rows(REFERENCE)
            .into_iter()
            .map(|r| r.scale(zoom))
            .collect();
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
        let expected = ShellLayout::compute(REFERENCE, true, true).main_view;
        assert!(
            (drawn.top() - (expected.bottom() - 300.5 + top)).abs() < 0.1,
            "{percent}%: {drawn:?}"
        );
        assert!((drawn.left() - expected.left()).abs() < 0.1, "{percent}%");
        assert!(
            (drawn.bottom() - (expected.bottom() + top)).abs() < 0.1,
            "{percent}%"
        );
    }
    // In the mixers, the sections stack without overlapping.
    let sections: Vec<&Row> = at_100
        .iter()
        .filter(|r| r.id.starts_with("L-MIX"))
        .collect();
    for pair in sections.windows(2) {
        let (a, b) = (pair[0].rect, pair[1].rect);
        if a.left() == b.left() && (a.top() - b.top()).abs() < 400.0 {
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
    cited.extend(["L-STAT-008", "L-ARR-006", "L-ARR-009", "L-MIX-004"]);
    for row in cited {
        assert!(
            text.contains(&format!("| {row} |")),
            "{row} isn't in layout.md"
        );
    }
}

/// The scripted drag the golden render plays (R18): the first audio
/// track's fader in the Session mixer, pressed, pulled down, then up past
/// where it began, then down a little, at 60 frames a second, with the
/// volume the set holds after every frame.
fn fader_gesture() -> Vec<f64> {
    let mut h = session();
    let t = tracks(&h.shell);
    let fader = control(&mut h, "3 Audio", "Track Volume");
    let mut steps = vec![vec2(0.0, 1.5); 12];
    steps.extend(vec![vec2(0.0, -0.75); 30]);
    steps.extend(vec![vec2(0.0, 0.0); 6]);
    steps.extend(vec![vec2(0.0, 1.0); 8]);
    let volumes = drag(&mut h, fader.center(), &steps, Modifiers::NONE, |h| {
        mixer(&h.shell, t[2]).volume
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
    let lowest = volumes.iter().copied().fold(f64::INFINITY, f64::min);
    let highest = volumes.iter().copied().fold(0.0, f64::max);
    assert!(controls::gain_db(lowest) < -3.0 && controls::gain_db(highest) > 0.5);
    let recorded = json!({
        "about": "The first audio track's volume after each 60 Hz frame of a fader drag in the \
                  Session mixer, made by crates/ui/tests/mixer.rs (fader_gesture); the \
                  fader-gesture golden fixture plays it.",
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

// Spec: routing R16, R17. The In/Out choosers open on a click and run the
// chosen entry's routing command; the Monitor switch takes a click on its
// state.
#[test]
fn the_in_out_choosers_and_monitor_route_the_track() {
    let mut h = session();
    let t = tracks(&h.shell);
    let routing = |h: &Harness| {
        h.shell
            .editor()
            .document()
            .track(t[2])
            .unwrap()
            .routing()
            .clone()
    };
    // Audio From: open the list, choose 4-Audio.
    let input = control(&mut h, "3 Audio", "Input Type");
    h.click(input.center());
    let output = h.settle();
    let entry = Harness::nodes(&output)
        .into_iter()
        .find(|n| n.label == "4-Audio" && n.bounds.is_some())
        .expect("the list shows the other audio track");
    let before = h.shell.editor().undo_history().len();
    h.click(rect_of(&entry).center());
    h.settle();
    assert_eq!(
        routing(&h).input,
        bl_model::Input::Track {
            track: t[3],
            tap: bl_model::Tap::PostMixer,
            chain: None,
            output: None,
        }
    );
    assert_eq!(h.shell.editor().undo_history().len(), before + 1);
    assert_eq!(
        h.shell.editor().undo_history().next(),
        Some("Change \"Routing\"")
    );
    // Monitor: a click on In.
    let monitor = control(&mut h, "3 Audio", "Monitoring");
    h.click(pos2(monitor.left() + 3.0, monitor.center().y));
    h.settle();
    assert_eq!(routing(&h).monitor, bl_model::Monitor::In);
    // With the focus on it, Right moves it to Auto.
    h.key(Key::ArrowRight, Modifiers::NONE);
    h.settle();
    assert_eq!(routing(&h).monitor, bl_model::Monitor::Auto);
    // Audio To: Up and Down on the focused chooser step through the list.
    let output_type = control(&mut h, "3 Audio", "Output Type");
    h.click(output_type.center());
    h.key(Key::Escape, Modifiers::NONE);
    h.settle();
    h.ctx.memory_mut(|m| {
        m.request_focus(bl_ui::in_out::control_id(
            Place::Mixer(MixerOf::Session),
            t[2],
            bl_ui::focus::Slot::OutputType,
            0,
        ));
    });
    h.settle();
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.settle();
    assert_eq!(routing(&h).output, bl_model::Output::Track { track: t[3] });
    let nodes = tree(&mut h);
    assert_eq!(
        inside(&nodes, "3 Audio", "Output Channel").unwrap().value,
        "Track In"
    );
}

// Spec: routing R18. A send knob drags from −∞ to 0 dB as one undo step;
// a double-click puts it back to −∞.
#[test]
fn a_send_knob_drags_as_one_undo_step() {
    let mut h = session();
    let t = tracks(&h.shell);
    let ret = h.shell.editor().document().returns().next().unwrap();
    let send = |h: &Harness| h.shell.editor().document().track(t[2]).unwrap().send(ret);
    let knob = control(&mut h, "3 Audio", "A-Return");
    let before = h.shell.editor().undo_history().len();
    let levels = drag(
        &mut h,
        knob.center(),
        &[vec2(0.0, -2.0); 40],
        Modifiers::NONE,
        send,
    );
    assert_eq!(levels[0], 0.0, "a press doesn't move it");
    assert_eq!(*levels.last().unwrap(), 1.0, "it stops at 0 dB");
    assert!(levels.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(h.shell.editor().undo_history().len(), before + 1);
    assert_eq!(
        h.shell.editor().undo_history().next(),
        Some(controls::SEND_GESTURE)
    );
    let nodes = tree(&mut h);
    assert_eq!(
        inside(&nodes, "3 Audio", "A-Return").unwrap().value,
        "0.0 dB"
    );
    // Up and Down step a dB.
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.settle();
    assert!((controls::gain_db(send(&h)) + 1.0).abs() < 1e-9);
    // A double-click: back to -inf.
    let at = knob.center();
    for _ in 0..2 {
        h.click(at);
    }
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
    assert_eq!(send(&h), 0.0);
    // Control::Send names the knob's command, as an agent runs it.
    assert_eq!(Control::Send(ret).set(t[2], 0.5).id, "mixer.set_send");
}

// Spec: routing R19. A return's Pre/Post switch, on Main's column.
#[test]
fn a_returns_pre_post_switch_on_mains_column_switches_it() {
    let mut h = session();
    let ret = h.shell.editor().document().returns().next().unwrap();
    let switch = control(&mut h, "Main", "Return A Pre/Post");
    h.click(switch.center());
    h.settle();
    let tap = |h: &Harness| h.shell.editor().document().track(ret).unwrap().mixer().tap;
    assert_eq!(tap(&h), bl_model::SendTap::Pre);
    let nodes = tree(&mut h);
    assert_eq!(
        inside(&nodes, "Main", "Return A Pre/Post").unwrap().value,
        "Pre"
    );
    h.click(switch.center());
    h.settle();
    assert_eq!(tap(&h), bl_model::SendTap::Post);
}

// Spec: mixer-view R3, R4, R29. In Split Stereo Pan Mode two controls sit
// beside the pan knob, one placing each input channel; each runs its own
// command on a drag.
#[test]
fn the_split_controls_sit_beside_the_pan_knob() {
    let mut h = session();
    let t = tracks(&h.shell);
    // A new track is in Stereo Pan Mode: only the pan knob shows.
    let nodes = tree(&mut h);
    assert!(inside(&nodes, "3 Audio", "Split Pan Left").is_none());
    assert!(inside(&nodes, "3 Audio", "Split Pan Right").is_none());
    // Split Stereo Pan Mode, as the pan control's context menu sets it.
    h.shell
        .run(
            "mixer.set_pan_mode",
            json!({ "track": t[2], "mode": "split" }),
        )
        .unwrap();
    let nodes = tree(&mut h);
    let column = rect_of(inside(&nodes, "Session Mixer", "3 Audio").unwrap());
    let pan = rect_of(inside(&nodes, "3 Audio", "Track Pan").unwrap());
    let left = rect_of(inside(&nodes, "3 Audio", "Split Pan Left").unwrap());
    let right = rect_of(inside(&nodes, "3 Audio", "Split Pan Right").unwrap());
    // Beside the pan knob, to its left and right, sharing its row and inside
    // the column.
    let p = relative(pan, column, STRIP_TOP);
    let l = relative(left, column, STRIP_TOP);
    let r = relative(right, column, STRIP_TOP);
    assert!(l.2 <= p.0, "left {l:?} is left of pan {p:?}");
    assert!(r.0 >= p.2, "right {r:?} is right of pan {p:?}");
    for split in [l, r] {
        assert!(
            split.1 >= p.1 && split.3 <= p.3,
            "the split shares the pan's row: {split:?} vs {p:?}"
        );
        assert!(split.0 >= 0.0 && split.2 <= column.width(), "{split:?}");
    }
    // Dragging the left control right runs mixer.set_split_pan_left and
    // moves split_left to 1.0, in one undo step.
    let before = h.shell.editor().undo_history().len();
    drag(
        &mut h,
        left.center(),
        &[vec2(0.0, -2.0); 30],
        Modifiers::NONE,
        |_| (),
    );
    assert_eq!(mixer(&h.shell, t[2]).split_left, 1.0);
    assert_eq!(h.shell.editor().undo_history().len(), before + 1);
    assert_eq!(
        h.shell.editor().undo_history().next(),
        Some("Change \"Split Pan Left\"")
    );
    // The right control moves the other way, its own command.
    let before = h.shell.editor().undo_history().len();
    drag(
        &mut h,
        right.center(),
        &[vec2(0.0, 2.0); 30],
        Modifiers::NONE,
        |_| (),
    );
    assert_eq!(mixer(&h.shell, t[2]).split_right, -1.0);
    assert_eq!(h.shell.editor().undo_history().len(), before + 1);
    assert_eq!(
        h.shell.editor().undo_history().next(),
        Some("Change \"Split Pan Right\"")
    );
}

// A rack's chains folded out as strips in the mixers (racks spec, R27 to
// R29).

/// A device added to a track or a chain.
#[derive(serde::Deserialize)]
struct Added {
    device: DeviceId,
}

/// A rack of `kind` on the shown track, with `names` as its chains' names.
fn add_rack(h: &mut Harness, kind: &str, role: &str, names: &[&str]) -> (DeviceId, Vec<ChainId>) {
    let added: Added = serde_json::from_value(
        h.shell
            .run(
                "device.add",
                json!({
                    "device": {
                        "kind": { "rack": { "kind": kind } },
                        "name": "Rack",
                        "role": role
                    }
                }),
            )
            .unwrap(),
    )
    .unwrap();
    let mut chains = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let id: ChainId = h.shell.new_id();
        h.shell
            .invoke(
                "chain.add",
                json!({
                    "rack": added.device,
                    "index": index,
                    "chain": { "id": id, "name": name, "devices": [] }
                }),
            )
            .unwrap();
        chains.push(id);
    }
    h.settle();
    (added.device, chains)
}

/// A MIDI track with an instrument rack of two chains, the chains folded
/// out as strips in the mixers.
fn folded_rack(h: &mut Harness) -> (TrackId, DeviceId, Vec<ChainId>) {
    let track = tracks(&h.shell)[0];
    h.shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let (rack, chains) = add_rack(h, "instrument", "instrument", &["Keys", "Bass"]);
    let nodes = tree(h);
    let button = Harness::node(&nodes, "Fold Chains");
    click(h, rect_of(button).center(), Modifiers::NONE);
    h.settle();
    (track, rack, chains)
}

/// A chain's Volume in the set.
fn chain_volume(shell: &Shell, chain: ChainId) -> f64 {
    shell
        .editor()
        .document()
        .chain(chain)
        .expect("the chain")
        .mixer
        .volume
}

/// The chain list's row for a chain, in the Device View.
fn chain_row(h: &mut Harness, rack: DeviceId, index: usize) -> bl_ui::rack_view::ChainRowRects {
    let nodes = tree(h);
    let view = rect_of(Harness::node(&nodes, "Device View"));
    let layouts = bl_ui::rack_view::layouts(&h.shell, view);
    let layout = layouts
        .iter()
        .find(|(id, _)| *id == rack)
        .unwrap_or_else(|| panic!("no layout for {rack}"))
        .1
        .clone();
    layout
        .chain_list
        .as_ref()
        .expect("the chain list shows")
        .rows[index]
}

// Spec: racks R27, and mixer-view R1, R2.
#[test]
fn a_racks_chains_fold_out_as_strips_without_slots() {
    let mut h = session();
    let (_track, rack, chains) = folded_rack(&mut h);
    // The chains are folded out as strips after the track.
    let state = state(&mut h.shell);
    assert_eq!(state.chains.len(), 2);
    assert_eq!(state.chains[0].chain, chains[0]);
    assert_eq!(state.chains[1].chain, chains[1]);
    assert!(state.racks.iter().any(|r| r.rack == rack && r.folded));
    // The strips are in the mixers, named for their chain and their rack.
    let nodes = tree(&mut h);
    assert!(
        nodes
            .iter()
            .any(|n| n.label == "Chain Strip Keys of Instrument Rack")
    );
    assert!(
        nodes
            .iter()
            .any(|n| n.label == "Chain Strip Bass of Instrument Rack")
    );
    // A chain's column has no clip slots.
    let slots = nodes.iter().filter(|n| n.role == Role::ListItem).count();
    assert!(slots > 0);
    // The Arrangement's mixer shows them the same way.
    h.shell
        .run(ids::TOGGLE, json!({ "view": "arrangement" }))
        .unwrap();
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    h.settle();
    let nodes = tree(&mut h);
    assert!(
        nodes
            .iter()
            .any(|n| n.label == "Chain Strip Keys of Instrument Rack")
    );
    assert!(
        nodes
            .iter()
            .any(|n| n.label == "Chain Strip Bass of Instrument Rack")
    );
}

// Spec: racks R27.
#[test]
fn a_nested_rack_folds_out_from_its_chains_strip() {
    let mut h = session();
    let (_track, _rack, chains) = folded_rack(&mut h);
    // A rack inside a chain has its own fold button, on that chain's strip.
    let nested: Added = serde_json::from_value(
        h.shell
            .run(
                "device.add",
                json!({
                    "device": {
                        "kind": { "rack": { "kind": "instrument" } },
                        "name": "Inner",
                        "role": "instrument"
                    },
                    "chain": chains[0]
                }),
            )
            .unwrap(),
    )
    .unwrap();
    let mut inner = Vec::new();
    for (index, name) in ["Deep", "Wide"].iter().enumerate() {
        let id: ChainId = h.shell.new_id();
        h.shell
            .invoke(
                "chain.add",
                json!({
                    "rack": nested.device,
                    "index": index,
                    "chain": { "id": id, "name": name, "devices": [] }
                }),
            )
            .unwrap();
        inner.push(id);
    }
    h.settle();
    // The strip's fold button folds the nested rack's chains out. It is in
    // the chain's own column, right of the track's.
    let nodes = tree(&mut h);
    let folds: Vec<Rect> = nodes
        .iter()
        .filter(|n| n.label == "Fold Chains")
        .map(rect_of)
        .collect();
    assert_eq!(folds.len(), 2, "the track's and the chain strip's");
    let at = folds
        .iter()
        .find(|r| r.left() > 414.0)
        .expect("the chain strip's fold button")
        .center();
    assert!(
        at.y < 100.0,
        "the strip's fold button is in the title row: {at:?}"
    );
    click(&mut h, at, Modifiers::NONE);
    let state = state(&mut h.shell);
    assert_eq!(state.chains.len(), 4);
    // The nested rack's chains follow the chain that holds it.
    assert_eq!(state.chains[0].chain, chains[0]);
    assert_eq!(state.chains[1].chain, inner[0]);
    assert_eq!(state.chains[2].chain, inner[1]);
    assert_eq!(state.chains[3].chain, chains[1]);
}

// Spec: racks R27.
#[test]
fn a_strips_volume_and_the_chain_lists_move_each_other() {
    let mut h = session();
    let (_track, rack, chains) = folded_rack(&mut h);
    // The strip's Volume moves the chain, and the chain list's slider with
    // it, in one undo step.
    let slider = control(
        &mut h,
        "Chain Strip Keys of Instrument Rack",
        "Chain Volume of Keys",
    );
    let before = h.shell.editor().undo_history().len();
    drag(
        &mut h,
        slider.center(),
        &[vec2(-20.0, 0.0)],
        Modifiers::NONE,
        |_| (),
    );
    let now = chain_volume(&h.shell, chains[0]);
    assert!(now < 1.0, "{now}");
    assert_eq!(h.shell.editor().undo_history().len(), before + 1);
    // The chain list's slider shows the new value.
    let nodes = tree(&mut h);
    let listed = Harness::node(&nodes, "Chain Volume of Keys");
    assert_eq!(listed.value, bl_ui::rack_view::volume_text(now));
    // And the other way round: the chain list's slider moves the strip.
    let row = chain_row(&mut h, rack, 0);
    let before = h.shell.editor().undo_history().len();
    drag(
        &mut h,
        row.volume.center(),
        &[vec2(20.0, 0.0)],
        Modifiers::NONE,
        |_| (),
    );
    let now = chain_volume(&h.shell, chains[0]);
    assert!(now > 0.0 && now != 1.0, "{now}");
    assert_eq!(h.shell.editor().undo_history().len(), before + 1);
}

// Spec: racks R28.
#[test]
fn selected_chain_strips_change_together_and_the_chain_list_does_not() {
    let mut h = session();
    let (_track, rack, chains) = folded_rack(&mut h);
    // Two strips selected: a change on one reaches the other.
    let nodes = tree(&mut h);
    let keys = Harness::node(&nodes, "Chain Keys of Instrument Rack");
    let at = rect_of(keys).center();
    click(&mut h, at, Modifiers::NONE);
    let nodes = tree(&mut h);
    let bass = Harness::node(&nodes, "Chain Bass of Instrument Rack");
    let at = rect_of(bass).center();
    click(&mut h, at, Modifiers::COMMAND);
    // The first chain's Volume moves, and the second's with it, keeping
    // their difference.
    let slider = control(
        &mut h,
        "Chain Strip Keys of Instrument Rack",
        "Chain Volume of Keys",
    );
    drag(
        &mut h,
        slider.center(),
        &[vec2(-20.0, 0.0)],
        Modifiers::NONE,
        |_| (),
    );
    let first = chain_volume(&h.shell, chains[0]);
    let second = chain_volume(&h.shell, chains[1]);
    assert!(first < 1.0, "{first}");
    assert!(second < 1.0, "{second}");
    assert!((first - second).abs() < 1e-9, "{first} {second}");
    // The chain list's own slider changes only its own chain.
    let row = chain_row(&mut h, rack, 0);
    drag(
        &mut h,
        row.volume.center(),
        &[vec2(20.0, 0.0)],
        Modifiers::NONE,
        |_| (),
    );
    let first = chain_volume(&h.shell, chains[0]);
    let second = chain_volume(&h.shell, chains[1]);
    assert!(first > second, "{first} {second}");
}

// Spec: racks R28.
#[test]
fn clicking_a_strips_title_bar_shows_only_its_chains_devices() {
    let mut h = session();
    let (_track, _rack, chains) = folded_rack(&mut h);
    // A device in the first chain, so the Device View has something to
    // show.
    h.shell
        .run(
            "device.add",
            json!({
                "device": {
                    "kind": bl_engine::testing::gain_type().kind,
                    "name": "Gain",
                    "role": "audio_effect"
                },
                "chain": chains[0]
            }),
        )
        .unwrap();
    h.settle();
    // Clicking the strip's title bar selects the chain, and the Device View
    // shows only its devices.
    let nodes = tree(&mut h);
    let title = Harness::node(&nodes, "Chain Bass of Instrument Rack");
    let at = rect_of(title).center();
    click(&mut h, at, Modifiers::NONE);
    let state = h.shell.device_view_state();
    let shown = state.devices.iter().filter(|d| d.title == "Gain").count();
    assert_eq!(shown, 0, "the other chain's devices are shown");
}

// Spec: racks R29.
#[test]
fn chain_strips_reorder_rename_and_group() {
    let mut h = session();
    let (_track, rack, chains) = folded_rack(&mut h);
    // Dragging a strip moves the chain among its rack's chains, in one undo
    // step.
    let nodes = tree(&mut h);
    let title = Harness::node(&nodes, "Chain Bass of Instrument Rack");
    let at = rect_of(title).center();
    let nodes = tree(&mut h);
    let keys = Harness::node(&nodes, "Chain Keys of Instrument Rack");
    let target = rect_of(keys).center();
    let before = h.shell.editor().undo_history().len();
    drag(
        &mut h,
        at,
        &[vec2(target.x - at.x, 0.0)],
        Modifiers::NONE,
        |_| (),
    );
    let order: Vec<ChainId> = h
        .shell
        .editor()
        .document()
        .device(rack)
        .unwrap()
        .chains
        .iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(order, [chains[1], chains[0]]);
    assert_eq!(h.shell.editor().undo_history().len(), before + 1);
    // Renaming from a strip: the context menu's Rename Chain, then the name
    // typed in the strip's title bar.
    let nodes = tree(&mut h);
    let title = Harness::node(&nodes, "Chain Bass of Instrument Rack");
    let at = rect_of(title).center();
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Secondary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Secondary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
    let nodes = tree(&mut h);
    let rename = nodes
        .iter()
        .find(|n| n.label == "Rename Chain")
        .expect("the rename item");
    let at = rect_of(rename).center();
    click(&mut h, at, Modifiers::NONE);
    // The name is typed in the strip's title bar, and Return gives it to
    // the chain.
    h.settle();
    h.frame(vec![Event::Text("A".to_owned())]);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    let name = h
        .shell
        .editor()
        .document()
        .chain(chains[1])
        .unwrap()
        .name
        .clone();
    assert_eq!(name, "BassA");
    // Grouping from a strip groups the selected chains, in one undo step.
    // Both chains are selected: a click, then Cmd-click.
    let nodes = tree(&mut h);
    let keys = Harness::node(&nodes, "Chain Keys of Instrument Rack");
    let at = rect_of(keys).center();
    click(&mut h, at, Modifiers::NONE);
    let nodes = tree(&mut h);
    let bass = Harness::node(&nodes, "Chain BassA of Instrument Rack");
    let at = rect_of(bass).center();
    click(&mut h, at, Modifiers::COMMAND);
    let nodes = tree(&mut h);
    let title = Harness::node(&nodes, "Chain Keys of Instrument Rack");
    let at = rect_of(title).center();
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Secondary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Secondary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
    let nodes = tree(&mut h);
    let group = nodes
        .iter()
        .find(|n| n.label == "Group Chains")
        .expect("the group item");
    let at = rect_of(group).center();
    let before = h.shell.editor().undo_history().len();
    click(&mut h, at, Modifiers::NONE);
    assert_eq!(h.shell.editor().undo_history().len(), before + 1);
    // The chains are now the chains of a new rack in the first one's place.
    let doc = h.shell.editor().document();
    let grouped: Vec<ChainId> = doc
        .device(rack)
        .unwrap()
        .chains
        .iter()
        .filter(|c| c.devices.first().is_some_and(|d| d.rack_kind().is_some()))
        .map(|c| c.id)
        .collect();
    assert_eq!(grouped.len(), 1, "one chain holds the new rack");
    let inner = &doc.chain(grouped[0]).unwrap().devices[0];
    assert_eq!(inner.chains.len(), 2, "the grouped chains are its chains");
}

// Spec: mixer-view R17.
#[test]
fn only_the_strips_in_view_are_built() {
    let mut h = session();
    let (_track, _rack, _chains) = folded_rack(&mut h);
    // The strips in view are built, and no more.
    let built = h.main.mixer().built_columns();
    assert!(built > 0);
    assert!(built <= 15);
}

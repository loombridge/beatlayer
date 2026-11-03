//! The MIDI effects' panels as Live lays them out (layout L-DEV-048 to
//! L-DEV-058): each device's panel holds Live's controls in Live's order and
//! kinds and none else, and the parts new with them (the stepper, Scale's grid,
//! Chord's Velocity and Chance buttons, the switch box, MPE Control's tab
//! column) do what they draw.
//!
//! Spec: device-framework R52 to R59; midi-scale-map R13; midi-chord R19.

use crate::common;
use bl_devices::{arpeggiator as arp, lift, remap, stack};
use bl_model::device_layout::{PageChoice, Widget};
use bl_model::{DeviceId, ParamId, TrackId};
use bl_shell::device_commands::{DeviceAdded, TITLE_BAR_HEIGHT};
use bl_ui::device_panel::{body_rect, place, place_with_pages};
use bl_ui::device_view::layout;
use common::{Harness, Window};
use egui::{Rect, pos2};
use serde_json::json;

use Widget::{
    Check, Chooser, Grid, IconSegments, Knob, Marked, Number, Outlined, Slider, SmallKnob, Stack,
    Stepper, Switch, Toggle,
};

const GRID_4_2: Widget = Grid {
    columns: 4,
    rows: 2,
};

/// What `device`'s panel holds with its pages `shown` (by node, then page) and
/// its expanded part closed: each control's parameter name and widget, in the
/// order the layout places them.
fn panel(device: &str, shown: &[(&str, &str)]) -> Vec<(String, Widget)> {
    let kind = bl_devices::device_types()
        .into_iter()
        .find(|t| t.name == device)
        .unwrap_or_else(|| panic!("{device} is a device"));
    let description = kind.description;
    let choice: PageChoice = shown
        .iter()
        .map(|(node, page)| ((*node).to_owned(), (*page).to_owned()))
        .collect();
    let layout = description.layout.with_pages(&choice).shown(false);
    layout
        .place(layout.docked_size())
        .controls
        .iter()
        .map(|c| (description.param(c.param).unwrap().name.clone(), c.widget))
        .collect()
}

fn expect(device: &str, shown: &[(&str, &str)], live: &[(&str, Widget)]) {
    let found = panel(device, shown);
    let live: Vec<(String, Widget)> = live.iter().map(|(n, w)| ((*n).to_owned(), *w)).collect();
    assert_eq!(found, live, "{device}'s panel");
}

// Spec: device-framework R52, R54
#[test]
fn arpeggiators_panel_holds_lives_controls_in_lives_order() {
    expect(
        "Arpeggiator",
        &[],
        &[
            ("Style", Chooser),
            ("Hold", Outlined),
            ("Offset", Number),
            ("Groove", Chooser),
            ("Rate Free", Knob),
            ("Distance", Knob),
            ("Steps", SmallKnob),
            ("Gate", SmallKnob),
            ("Retrigger", Chooser),
            ("Interval", SmallKnob),
            ("Repeats", Number),
            ("Velocity", Switch),
            ("Decay", Number),
            ("Target", Number),
            ("Retrig Vel", Toggle),
            ("Root", Chooser),
            ("Scale", Chooser),
        ],
    );
}

// Spec: device-framework R52, R54, R57
#[test]
fn the_other_midi_effects_panels_hold_lives_controls_in_lives_order() {
    expect(
        "Velocity",
        &[],
        &[
            ("Operation", Chooser),
            ("Mode", Stack(3)),
            ("Drive", SmallKnob),
            ("Comp.", SmallKnob),
            ("Random", SmallKnob),
            ("Out Hi", SmallKnob),
            ("Out Low", SmallKnob),
            ("Range", Slider),
            ("Lowest", Number),
        ],
    );
    expect(
        "Pitch",
        &[],
        &[
            ("Pitch", Knob),
            ("Step Width", Stepper),
            ("Lowest", Number),
            ("Range", Slider),
            ("Mode", Chooser),
        ],
    );
    expect(
        "Random",
        &[],
        &[
            ("Chance", SmallKnob),
            ("Choices", SmallKnob),
            ("Interval", SmallKnob),
            ("Mode", Stack(2)),
            ("Sign", Stack(3)),
        ],
    );
    expect(
        "Scale",
        &[],
        &[
            ("Base", Chooser),
            ("Scale Name", Chooser),
            ("Transpose", SmallKnob),
            ("Fold", Toggle),
            ("Lowest", Number),
            ("Range", Slider),
        ],
    );
    expect(
        "Note Length",
        &[],
        &[
            ("Trigger", Stack(2)),
            ("Gate", SmallKnob),
            ("Length", SmallKnob),
            ("Rel Vel", SmallKnob),
            ("Decay", SmallKnob),
            ("Key Scale", SmallKnob),
        ],
    );
    expect("MIDI Monitor", &[], &[]);
}

// Spec: device-framework R54
#[test]
fn chords_six_knobs_each_have_an_unnamed_amount_box_and_four_controls_follow() {
    let mut live = Vec::new();
    for n in 1..=6 {
        live.push((format!("Shift {n}"), SmallKnob));
        live.push((format!("Velocity {n}"), Marked));
    }
    live.extend(
        [
            ("Strum", SmallKnob),
            ("Tension", Marked),
            ("Crescendo", Marked),
            ("Amounts", Stack(2)),
        ]
        .map(|(n, w)| (n.to_owned(), w)),
    );
    assert_eq!(panel("Chord", &[]), live);
}

// Spec: device-framework R41, R55
#[test]
fn cc_controls_two_pages_hold_eight_cells_each() {
    let first = panel("CC Control", &[("Controls", "1-8")]);
    let names =
        |panel: &[(String, Widget)]| panel.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
    assert_eq!(
        names(&first),
        [
            "Mod Wheel",
            "Pitch Bend",
            "Pressure",
            "Custom A",
            "Custom A CC",
            "Custom B",
            "Custom B CC",
            "Custom C",
            "Custom C CC",
            "Custom D",
            "Custom D CC",
            "Custom E",
            "Custom E CC",
        ]
    );
    assert_eq!(first[0].1, Knob);
    assert_eq!(first[3].1, Check);
    assert_eq!(first[5].1, SmallKnob);
    let second = panel("CC Control", &[("Controls", "9-16")]);
    assert_eq!(second.len(), 16);
    assert_eq!(second[0].0, "Custom F");
    assert_eq!(second[14].0, "Custom M");
    assert!(second.iter().all(|(_, w)| [SmallKnob, Chooser].contains(w)));
}

// Spec: device-framework R41, R54
#[test]
fn mpe_controls_three_tabs_each_show_a_sources_curve_and_range() {
    for source in ["Press", "Slide", "NotePB"] {
        expect(
            "MPE Control",
            &[("Source", source)],
            &[
                (&format!("{source} Curve A"), SmallKnob),
                (&format!("{source} Curve"), IconSegments(2)),
                (&format!("{source} Min"), Number),
                (&format!("{source} Max"), Number),
                (&format!("{source} X"), Number),
                (&format!("{source} Y"), Number),
            ],
        );
    }
}

// Spec: device-framework R54
#[test]
fn note_echos_delay_time_is_a_grid_of_eight_buttons() {
    let found = panel("Note Echo", &[]);
    let live: Vec<(&str, Widget)> = vec![
        ("Delay Time", GRID_4_2),
        ("Sync", Toggle),
        ("Swing", Number),
        ("Input", Stack(2)),
        ("Pitch", Number),
        ("Delay", SmallKnob),
        ("Fback", SmallKnob),
        ("MPE", Toggle),
        ("Press", Number),
        ("Slide", Number),
        ("Note PB", Number),
    ];
    // Time Free, which Live shows only while Sync is off, follows these.
    assert_eq!(
        found[..live.len()],
        live.iter()
            .map(|(n, w)| ((*n).to_owned(), *w))
            .collect::<Vec<_>>()[..]
    );
}

fn harness() -> Harness {
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let tracks: Vec<TrackId> = h.shell.editor().document().top_level().collect();
    h.shell
        .invoke(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[2]] }),
        )
        .unwrap();
    h.settle();
    h
}

fn add(h: &mut Harness, name: &str) -> DeviceId {
    let t = bl_devices::device_types()
        .into_iter()
        .find(|t| t.name == name)
        .unwrap();
    let added: DeviceAdded = serde_json::from_value(
        h.shell
            .run(
                "device.add",
                json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
            )
            .unwrap(),
    )
    .unwrap();
    h.settle();
    added.device
}

/// The panel's body as drawn.
fn body(h: &mut Harness, device: DeviceId) -> (bl_shell::device_commands::DevicePanel, Rect) {
    let nodes = Harness::nodes(&h.settle());
    let b = Harness::node(&nodes, "Device View").bounds.unwrap();
    let view = Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    );
    let frame = layout(&h.shell, view)
        .into_iter()
        .find(|(id, _)| *id == device)
        .unwrap()
        .1;
    let panel = h
        .shell
        .device_view_state()
        .devices
        .iter()
        .find(|d| d.id == device)
        .unwrap()
        .panel
        .clone()
        .unwrap();
    (panel, body_rect(frame, TITLE_BAR_HEIGHT))
}

fn value(h: &Harness, device: DeviceId, param: ParamId, default: f64) -> f64 {
    h.shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .params
        .get(&param)
        .copied()
        .unwrap_or(default)
}

fn set(h: &mut Harness, device: DeviceId, param: ParamId, value: f64) {
    h.shell
        .invoke(
            "device.set_param",
            json!({ "device": device, "param": param, "value": value }),
        )
        .unwrap();
    h.settle();
}

// Spec: device-framework R52. A stepper's round buttons move the parameter it
// steps by the box's amount.
#[test]
fn pitchs_buttons_step_it_by_step_width() {
    let mut h = harness();
    let device = add(&mut h, "Pitch");
    let (panel, area) = body(&mut h, device);
    let (controls, _) = place(&panel, area);
    let rect = controls
        .iter()
        .find(|(p, w, _)| *p == lift::STEP_WIDTH && *w == Stepper)
        .unwrap()
        .2;
    assert_eq!(rect.size(), egui::vec2(66.0, 12.0));
    let at = |left: bool| {
        pos2(
            if left {
                rect.left() + 6.0
            } else {
                rect.right() - 6.0
            },
            rect.center().y,
        )
    };
    h.click(at(false));
    h.settle();
    assert_eq!(
        value(&h, device, lift::PITCH, 0.0),
        12.0,
        "+ steps up by 12 st"
    );
    h.click(at(false));
    h.settle();
    assert_eq!(value(&h, device, lift::PITCH, 0.0), 24.0);
    set(&mut h, device, lift::STEP_WIDTH, 5.0);
    h.click(at(true));
    h.settle();
    assert_eq!(
        value(&h, device, lift::PITCH, 0.0),
        19.0,
        "− steps down by 5"
    );
}

// Spec: midi-scale-map R13. A click on a cell of the User grid sends its
// column's note to its row, and a click on the lit cell sends it nowhere.
#[test]
fn a_click_on_scales_grid_sends_a_note_to_a_row_or_nowhere() {
    let mut h = harness();
    let device = add(&mut h, "Scale");
    let (panel, area) = body(&mut h, device);
    let (_, cells) = place(&panel, area);
    let grid = cells
        .iter()
        .find(|(n, _, _)| n == remap::GRID_CELL)
        .unwrap()
        .1;
    let cell = |column: u32, row: u32| {
        pos2(
            grid.left() + grid.width() * (column as f32 + 0.5) / 13.0,
            grid.bottom() - grid.height() * (row as f32 + 0.5) / 13.0,
        )
    };
    let map = |h: &Harness, column| value(h, device, remap::map(column), f64::from(column));
    assert_eq!(map(&h, 3), 3.0, "the identity grid");
    h.click(cell(3, 8));
    h.settle();
    assert_eq!(map(&h, 3), 8.0, "column 3 goes to row 8");
    h.click(cell(3, 8));
    h.settle();
    assert_eq!(map(&h, 3), -1.0, "the lit cell sends it nowhere");
    // A named scale's grid takes no click.
    set(&mut h, device, remap::SCALE_NAME, 1.0);
    h.click(cell(5, 2));
    h.settle();
    assert_eq!(map(&h, 5), 5.0);
}

// Spec: midi-chord R19. The Velocity and Chance buttons turn the six boxes
// between the shifts' velocities and their chances.
#[test]
fn chords_buttons_turn_its_boxes_between_velocities_and_chances() {
    let mut h = harness();
    let device = add(&mut h, "Chord");
    let boxes = |h: &mut Harness| {
        let (panel, area) = body(h, device);
        place(&panel, area)
            .0
            .into_iter()
            .filter(|(_, w, _)| *w == Marked)
            .map(|(p, _, _)| p)
            .collect::<Vec<_>>()
    };
    let velocities: Vec<ParamId> = (0..6).map(stack::velocity).collect();
    let chances: Vec<ParamId> = (0..6).map(stack::chance).collect();
    let shown = boxes(&mut h);
    assert!(
        shown[..6].iter().all(|p| velocities.contains(p)),
        "{shown:?}"
    );
    set(&mut h, device, stack::AMOUNTS, 1.0);
    let shown = boxes(&mut h);
    assert!(shown[..6].iter().all(|p| chances.contains(p)), "{shown:?}");
}

// Spec: device-framework R54. A switch box reads On or Off and turns when
// pressed.
#[test]
fn arpeggiators_velocity_box_turns_on_when_pressed() {
    let mut h = harness();
    let device = add(&mut h, "Arpeggiator");
    let (panel, area) = body(&mut h, device);
    let (controls, _) = place(&panel, area);
    let rect = controls
        .iter()
        .find(|(p, _, _)| *p == arp::VELOCITY_ON)
        .unwrap()
        .2;
    assert_eq!(value(&h, device, arp::VELOCITY_ON, 0.0), 0.0);
    h.click(pos2(rect.center().x, rect.bottom() - 6.0));
    h.settle();
    assert_eq!(value(&h, device, arp::VELOCITY_ON, 0.0), 1.0);
}

// Spec: device-framework R41. MPE Control's tabs are a column at the left of
// the panel; pressing one shows that source's curve and range.
#[test]
fn a_tab_in_mpe_controls_column_shows_its_source() {
    let mut h = harness();
    let device = add(&mut h, "MPE Control");
    let (panel, area) = body(&mut h, device);
    let (_, _, pages) = place_with_pages(&panel, false, area);
    let node = pages.iter().find(|p| p.name == "Source").unwrap();
    assert_eq!(node.names, ["Press", "Slide", "NotePB"]);
    let tabs: Vec<Rect> = node
        .buttons
        .iter()
        .map(|r| Rect::from_min_size(pos2(r.x, r.y), egui::vec2(r.width, r.height)))
        .collect();
    assert!(
        tabs[0].left() == tabs[1].left() && tabs[1].top() > tabs[0].bottom(),
        "stacked"
    );
    assert_eq!(tabs[0].width(), 52.0);
    h.click(tabs[1].center());
    h.settle();
    let (panel, area) = body(&mut h, device);
    let (controls, _) = place(&panel, area);
    let names: Vec<&str> = controls
        .iter()
        .map(|(p, _, _)| panel.description.param(*p).unwrap().name.as_str())
        .collect();
    assert!(
        names.contains(&"Slide Min") && !names.contains(&"Press Min"),
        "{names:?}"
    );
}

//! The layout layer against the UI parity reference (D16): at 1600 × 1000
//! points every pane and control the shell places is where layout.md
//! measured it in Live 12.4.3, and every row it cites exists.

use std::path::PathBuf;

use bl_ui::layout::mixer::Sections;
use bl_ui::layout::{
    ArrangementLayout, D24, ELEMENTS, Element, LINK_MENU_WIDTH, LINK_WIDTH, MINIMUM, REFERENCE,
    ShellLayout, Strip, TITLE_BAR, layout_size, placement,
};
use egui::{Rect, Vec2, pos2, vec2};

/// layout.md, or `None` in a checkout without the planning documents (they
/// aren't published yet), where the test skips.
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

/// The content-area y of a y measured in Live's window.
fn y(window_y: f32) -> f32 {
    window_y - TITLE_BAR
}

#[test]
fn the_panes_sit_where_live_puts_them() {
    let layout = ShellLayout::compute(REFERENCE, true, true);
    let browser = layout.browser.expect("the browser shows by default");
    // L-WIN-004: x 9 to 311, from 61 to the detail area's top less 8.
    assert_eq!((browser.left(), browser.right()), (9.0, 311.0));
    assert_eq!(browser.top(), y(61.0));
    assert_eq!(browser.bottom(), y(766.0));
    // L-WIN-005: x 320 to 1591, 61 to 766.
    let main = layout.main_view;
    assert_eq!(
        (main.left(), main.right(), main.top(), main.bottom()),
        (320.0, 1591.0, y(61.0), y(766.0))
    );
    // L-WIN-006: 774 to 962; L-INFO-001 x 9 to 240; L-DEV-001 x 249 to 1591.
    assert_eq!(
        (layout.detail.top(), layout.detail.bottom()),
        (y(774.0), y(962.0))
    );
    let info = layout.info_view.expect("the Info View shows by default");
    assert_eq!((info.left(), info.right()), (9.0, 240.0));
    assert_eq!(
        (
            layout.device_view.unwrap().left(),
            layout.device_view.unwrap().right()
        ),
        (249.0, 1591.0)
    );
    // L-WIN-007: 968 to 993, full width.
    let status = layout.status_bar;
    assert_eq!(
        (status.top(), status.bottom(), status.width()),
        (y(968.0), y(993.0), 1600.0)
    );
    // L-WIN-003: the Control Bar's controls are 18 points tall, centred about
    // 37 points from the window's top.
    let tempo = layout.rect(Element::Tempo);
    assert_eq!(tempo.height(), 18.0);
    assert!(
        (tempo.center().y - y(37.0)).abs() <= 1.0,
        "{}",
        tempo.center().y
    );
}

#[test]
fn each_control_starts_where_layout_md_measured_it() {
    let layout = ShellLayout::compute(REFERENCE, true, true);
    // The left edges layout.md gives, at 1600 × 1000 with the Learn View
    // closed. Each comes straight from the row's text, except that the
    // Control Bar's right group sits 20 points left of Live's: the
    // assistant's toggle takes the far right (D24).
    let expected: &[(Element, f32)] = &[
        (Element::BrowserToggle, 9.0),
        (Element::TapTempo, 54.0),
        (Element::Tempo, 82.0),
        (Element::NudgeDown, 126.0),
        (Element::NudgeUp, 145.0),
        (Element::SignatureNumerator, 167.0),
        (Element::SignatureDenominator, 189.0),
        (Element::Metronome, 209.0),
        (Element::MetronomeMenu, 237.0),
        (Element::Quantization, 250.0),
        (Element::ScaleMode, 314.0),
        (Element::RootNote, 335.0),
        (Element::ScaleName, 381.0),
        (Element::Follow, 555.0),
        (Element::PositionBars, 582.0),
        (Element::Play, 678.0),
        (Element::Stop, 701.0),
        (Element::Record, 725.0),
        (Element::Overdub, 758.0),
        (Element::AutomationArm, 778.0),
        (Element::ReEnableAutomation, 796.0),
        (Element::CaptureMidi, 816.0),
        (Element::SessionRecord, 834.0),
        (Element::LoopStartBars, 875.0),
        (Element::PunchIn, 956.0),
        (Element::Loop, 979.0),
        (Element::PunchOut, 1002.0),
        (Element::LoopLengthBars, 1029.0),
        (Element::DrawMode, 1263.0),
        (Element::ComputerKeyboard, 1282.0),
        (Element::KeyMap, 1301.0),
        (Element::KeyIndicators, 1330.0),
        (Element::MidiMap, 1339.0),
        (Element::SampleRate, 1369.0),
        (Element::Cpu, 1419.0),
        (Element::Overload, 1478.0),
        (Element::RemoteIndicators, 1498.0),
        (Element::MidiIndicators, 1507.0),
        (Element::SessionView, 1529.0),
        (Element::AssistantToggle, 1572.0),
        (Element::InfoToggle, 17.0),
        (Element::Message, 44.0),
        (Element::ClipViewToggle, 1259.0),
        (Element::ClipSelector, 1270.0),
        (Element::DeviceViewToggle, 1486.0),
        (Element::DeviceSelector, 1497.0),
        (Element::MixerToggle, 1557.0),
    ];
    for (element, x) in expected {
        assert_eq!(layout.rect(*element).left(), *x, "{element:?}");
    }
    // Spans layout.md gives as from-to.
    for (element, right) in [
        (Element::Tempo, 126.0),
        (Element::Quantization, 295.0),
        (Element::ScaleName, 452.0),
        (Element::PositionSixteenths, 659.0),
        (Element::LoopStartSixteenths, 953.0),
        (Element::LoopLengthSixteenths, 1106.0),
        (Element::SampleRate, 1419.0),
        (Element::CpuMenu, 1476.0),
        (Element::ArrangementView, 1570.0),
        (Element::AssistantToggle, 1590.0),
        (Element::Message, 1240.0),
        (Element::ClipSelector, 1468.0),
        (Element::DeviceSelector, 1538.0),
        (Element::MixerMenu, 1591.0),
    ] {
        assert_eq!(layout.rect(element).right(), right, "{element:?}");
    }
}

#[test]
fn groups_stretch_with_the_window_and_controls_keep_their_size() {
    let wide = ShellLayout::compute(REFERENCE + vec2(280.0, 0.0), true, true);
    let reference = ShellLayout::compute(REFERENCE, true, true);
    // The left groups stay; the right groups follow the right edge; the
    // middle groups take their share of the extra width.
    assert_eq!(wide.rect(Element::Tempo), reference.rect(Element::Tempo));
    assert_eq!(wide.rect(Element::Cpu).left(), 1419.0 + 280.0);
    // The stretching gaps are 103 and 157 points (D24): the middle takes
    // 103/260 of the extra 280.
    let play = wide.rect(Element::Play).left();
    assert!(
        (play - (678.0 + 280.0 * 103.0 / 260.0)).abs() < 1e-3,
        "{play}"
    );
    for (placement, (_, rect)) in ELEMENTS.iter().zip(&wide.elements) {
        // The Global Amount is hidden unless the pool holds a groove, and the
        // sync lamps and the Resync button unless an input has Sync on or
        // Show Resync Button is on, the Link toggle unless Show Link Toggle
        // is Show, and New unless a map mode is on.
        let hidden = matches!(
            placement.element,
            Element::GlobalAmount
                | Element::ExternalSync
                | Element::SyncInput
                | Element::SyncOutput
                | Element::ResyncExternalHardware
                | Element::LinkToggle
                | Element::LinkMenu
                | Element::New
        );
        if placement.element != Element::Message && !hidden {
            assert_eq!(rect.width(), placement.width, "{:?}", placement.element);
        }
    }
    // Taller: the detail area and the Status Bar keep their heights.
    let tall = ShellLayout::compute(REFERENCE + vec2(0.0, 200.0), true, true);
    assert_eq!(tall.detail.height(), 188.0);
    assert_eq!(tall.status_bar.height(), 25.0);
    assert_eq!(
        tall.main_view.height(),
        reference.main_view.height() + 200.0
    );
}

#[test]
fn every_cited_row_exists_and_every_control_bar_row_is_placed() {
    let Some(text) = layout_md() else {
        return;
    };
    for placement in ELEMENTS.iter().filter(|p| p.row != D24) {
        assert!(
            text.contains(&format!("| {} |", placement.row)),
            "{}",
            placement.row
        );
    }
    // Every Control Bar and Status Bar row is placed, except those hidden in
    // Live's default state (Phase 7's EXT, Resync, Link progress, New, and the
    // MIDI mapping mode chooser, which show only with sync, Link peers or a
    // map mode) and the Status Bar's note readout, which shares the message
    // area.
    let not_placed = [
        "L-CTRL-002",
        "L-CTRL-003",
        "L-CTRL-008",
        "L-CTRL-028",
        "L-CTRL-029",
        "L-CTRL-030",
        "L-CTRL-031",
        "L-STAT-003",
        "L-STAT-009",
    ];
    for line in text.lines() {
        let Some(row) = line
            .strip_prefix("| ")
            .and_then(|rest| rest.split(' ').next())
            .filter(|id| id.starts_with("L-CTRL-") || id.starts_with("L-STAT-"))
        else {
            continue;
        };
        let placed = ELEMENTS.iter().any(|p| p.row == row);
        assert!(placed || not_placed.contains(&row), "{row} isn't placed");
    }
    let strips: Vec<Strip> = ELEMENTS.iter().map(|p| p.strip).collect();
    assert!(strips.contains(&Strip::ControlBar) && strips.contains(&Strip::StatusBar));
}

#[test]
fn the_widest_assistant_pane_leaves_the_minimum_window_without_overlaps() {
    // D24: the pane takes its width from the main view, the detail area and
    // the Status Bar; at the smallest window and the widest pane the Status
    // Bar's message area still ends before the controls to its right.
    let layout = ShellLayout::compute_with_assistant(MINIMUM, true, true, Some(640.0));
    let pane = layout.assistant.expect("the pane shows");
    assert!(layout.main_view.width() > 0.0 && layout.main_view.right() < pane.left());
    assert!(
        layout.device_view.unwrap().width() > 0.0
            && layout.device_view.unwrap().right() < pane.left()
    );
    let message = layout.rect(Element::Message);
    let clip = layout.rect(Element::ClipViewToggle);
    assert!(message.right() <= clip.left(), "{message:?} {clip:?}");
    for (element, rect) in &layout.elements {
        if placement(*element).strip == Strip::StatusBar {
            assert!(rect.right() <= pane.left(), "{element:?}");
        }
    }
}

fn with_groove(size: Vec2) -> ShellLayout {
    ShellLayout::compute_with_detail(
        size,
        true,
        true,
        None,
        None,
        bl_ui::layout::DetailViews::default().with_global_amount(true),
    )
}

// Spec: grooves R30; L-CTRL-008. While the pool holds a groove the Control
// Bar has the Global Amount field at x 209 to 249, and everything to its
// right in the left group moves 40 points right; the middle and right groups
// stay where they are at the reference width, since the gaps between the
// groups stretch to take the 40 points.
#[test]
fn the_global_amount_moves_the_left_group_40_points_and_the_gaps_take_them() {
    let hidden = ShellLayout::compute(REFERENCE, true, true);
    let shown = with_groove(REFERENCE);
    assert_eq!(hidden.rect(Element::GlobalAmount).width(), 0.0, "hidden");
    let field = shown.rect(Element::GlobalAmount);
    assert_eq!((field.left(), field.right()), (209.0, 249.0));
    for element in [
        Element::Tempo,
        Element::SignatureNumerator,
        Element::SignatureDenominator,
    ] {
        assert_eq!(shown.rect(element), hidden.rect(element), "{element:?}");
    }
    for element in [
        Element::Metronome,
        Element::MetronomeMenu,
        Element::Quantization,
        Element::ScaleMode,
        Element::RootNote,
        Element::ScaleName,
    ] {
        assert_eq!(
            shown.rect(element).left(),
            hidden.rect(element).left() + 40.0,
            "{element:?}"
        );
    }
    for element in [
        Element::Follow,
        Element::Play,
        Element::LoopLengthSixteenths,
        Element::Cpu,
        Element::ArrangementView,
    ] {
        assert_eq!(shown.rect(element), hidden.rect(element), "{element:?}");
    }
}

// Spec: grooves R30. The Control Bar's controls never overlap and stay in
// order: where the gaps can't take the 40 points the window gives the field no
// room, from the smallest window up to 40 points wider.
#[test]
fn the_global_amount_hides_where_the_gaps_have_no_room_for_it() {
    for width in [
        MINIMUM.x,
        MINIMUM.x + 39.0,
        MINIMUM.x + 40.0,
        MINIMUM.x + 41.0,
        1500.0,
        REFERENCE.x,
        REFERENCE.x + 300.0,
    ] {
        let layout = with_groove(vec2(width, REFERENCE.y));
        let room = layout.rect(Element::GlobalAmount).width() > 0.0;
        assert_eq!(room, width >= MINIMUM.x + 40.0, "at {width}");
        let bar: Vec<Rect> = ELEMENTS
            .iter()
            .zip(&layout.elements)
            .filter(|(p, _)| p.strip == bl_ui::layout::Strip::ControlBar)
            .map(|(_, (_, rect))| *rect)
            .filter(|rect| rect.width() > 0.0)
            .collect();
        for pair in bar.windows(2) {
            assert!(
                pair[0].right() <= pair[1].left() + 1e-3,
                "at {width}: {:?} and {:?} overlap",
                pair[0],
                pair[1]
            );
        }
    }
    // The second window's Control Bar is laid out as the main window's is at
    // its smallest width: no room.
    let second = ShellLayout::compute_second(
        vec2(990.0, 703.0 - TITLE_BAR),
        false,
        bl_ui::layout::DetailViews::default().with_global_amount(true),
    );
    assert_eq!(second.rect(Element::GlobalAmount).width(), 0.0);
}

// Spec: arrangement-view R1, mixer-view R32.
#[test]
fn a_short_arrangement_keeps_its_parts_inside_its_view() {
    // At the minimum window the main view is about 368 points tall, and all
    // the mixer's sections take 340 of them.
    let view = Rect::from_min_max(pos2(0.0, 38.0), pos2(900.0, 406.0));
    let sections = Sections {
        crossfader: true,
        ..Sections::DEFAULT
    };
    let layout = ArrangementLayout::compute_with_mixer(view, true, 2, sections);
    let mixer = layout.mixer.unwrap();
    for (name, rect) in [
        ("beat ruler", layout.beat_ruler),
        ("scrub", layout.scrub),
        ("set locator", layout.set_locator),
        ("lanes", layout.lanes),
        ("headers", layout.headers),
        ("pinned lanes", layout.pinned_lanes),
        ("pinned headers", layout.pinned_headers),
        ("time ruler", layout.time_ruler),
        ("optimize width", layout.optimize_width),
    ] {
        assert!(
            rect.top() >= view.top() && rect.bottom() <= mixer.top(),
            "{name} {rect:?} is outside the view above the mixer at {}",
            mixer.top()
        );
    }
}

// Spec: arrangement-view R1, mixer-view R32; time-editing R29. The
// time-signature lane, added after the parts were fitted, stays above the
// mixer too, and no part turns upside down.
#[test]
fn a_short_arrangement_with_a_signature_lane_keeps_its_parts_inside_its_view() {
    let view = Rect::from_min_max(pos2(0.0, 38.0), pos2(900.0, 406.0));
    for sections in [
        Sections::DEFAULT,
        Sections {
            crossfader: true,
            ..Sections::DEFAULT
        },
    ] {
        let layout =
            ArrangementLayout::compute_with_mixer(view, true, 2, sections).with_signature_lane();
        let mixer = layout.mixer.unwrap();
        for (name, rect) in [
            ("beat ruler", layout.beat_ruler),
            ("scrub", layout.scrub),
            ("signature lane", layout.signature_lane.unwrap()),
            ("lanes", layout.lanes),
            ("headers", layout.headers),
        ] {
            assert!(
                rect.top() >= view.top() && rect.bottom() <= mixer.top(),
                "{name} {rect:?} is outside the view above the mixer at {}",
                mixer.top()
            );
        }
        assert!(layout.lanes.min.y <= layout.lanes.max.y);
        assert!(layout.headers.min.y <= layout.headers.max.y);
    }
}

/// The main window's layout with the Link toggle shown or hidden, on a set
/// whose pool holds the factory default groove (the Global Amount shows).
fn with_link(size: Vec2, link: bool) -> ShellLayout {
    ShellLayout::compute_with_detail(
        size,
        true,
        true,
        None,
        None,
        bl_ui::layout::DetailViews::default()
            .with_global_amount(true)
            .with_link(link),
    )
}

// Spec: link R17; L-CTRL-002. The Link toggle and its arrow sit in the tempo
// group before Tap, 2 points apart, and everything after them in the left
// group moves right by their widths and gaps; nothing in the Control Bar
// overlaps another, at 1600 × 1000 and at 50 %, 100 % and 200 % zoom, and
// with the toggle hidden the group is back where Live measured it.
#[test]
fn the_link_toggle_sits_before_tap_and_moves_the_tempo_group_right() {
    let taken = LINK_WIDTH + 2.0 + LINK_MENU_WIDTH + 2.0;
    for zoom in [0.5_f32, 1.0, 2.0] {
        let size = layout_size(REFERENCE * zoom, zoom);
        let (shown, hidden) = (with_link(size, true), with_link(size, false));
        let what = format!("at {:.0} %", zoom * 100.0);
        let toggle = shown.rect(Element::LinkToggle);
        let arrow = shown.rect(Element::LinkMenu);
        let tap = shown.rect(Element::TapTempo);
        assert_eq!(
            (toggle.left(), toggle.width()),
            (54.0, LINK_WIDTH),
            "{what}"
        );
        assert_eq!(arrow.left(), toggle.right() + 2.0, "{what}");
        assert_eq!(arrow.width(), LINK_MENU_WIDTH, "{what}");
        assert_eq!(tap.left(), arrow.right() + 2.0, "{what}: before Tap");
        assert_eq!(tap.left(), hidden.rect(Element::TapTempo).left() + taken);
        for element in [
            Element::Tempo,
            Element::NudgeDown,
            Element::NudgeUp,
            Element::SignatureNumerator,
            Element::SignatureDenominator,
            Element::GlobalAmount,
            Element::Metronome,
            Element::Quantization,
            Element::ScaleName,
        ] {
            assert_eq!(
                shown.rect(element).left(),
                hidden.rect(element).left() + taken,
                "{what}: {element:?} moves right"
            );
        }
        for element in [Element::Follow, Element::Play, Element::DrawMode] {
            assert_eq!(
                shown.rect(element),
                hidden.rect(element),
                "{what}: {element:?}"
            );
        }
        // Nothing overlaps, in Live's order.
        let rects: Vec<Rect> = ELEMENTS
            .iter()
            .zip(&shown.elements)
            .filter(|(p, _)| p.strip == Strip::ControlBar && p.element != Element::SyncOutput)
            .map(|(_, (_, rect))| *rect)
            .collect();
        for pair in rects.windows(2) {
            assert!(pair[0].right() <= pair[1].left() + 0.01, "{what}: {pair:?}");
        }
        // Hidden, the group is where Live measured it.
        assert_eq!(hidden.rect(Element::LinkToggle).width(), 0.0, "{what}");
        assert_eq!(hidden.rect(Element::LinkMenu).width(), 0.0, "{what}");
        assert_eq!(hidden.rect(Element::TapTempo).left(), 54.0, "{what}");
        assert_eq!(hidden.rect(Element::Tempo).left(), 82.0, "{what}");
    }
}

// Spec: link R17. Where the Control Bar has no room for the toggle, in the
// smallest window and in the second window, it hides and the tempo group
// keeps Live's order.
#[test]
fn the_link_toggle_hides_where_the_gaps_have_no_room_for_it() {
    let narrow = with_link(MINIMUM, true);
    assert_eq!(narrow.rect(Element::LinkToggle).width(), 0.0);
    assert_eq!(narrow.rect(Element::LinkMenu).width(), 0.0);
    assert_eq!(narrow.rect(Element::TapTempo).left(), 54.0);
    let second = ShellLayout::compute_second(
        REFERENCE,
        true,
        bl_ui::layout::DetailViews::default().with_link(true),
    );
    assert_eq!(second.rect(Element::LinkToggle).width(), 0.0);
    assert_eq!(second.rect(Element::TapTempo).left(), 54.0);
}

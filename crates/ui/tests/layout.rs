//! The layout layer against the UI parity reference (D16): at 1600 × 1000
//! points every pane and control the shell places is where layout.md
//! measured it in Live 12.4.3, and every row it cites exists.

use std::path::PathBuf;

use bl_ui::layout::{
    D24, ELEMENTS, Element, MINIMUM, REFERENCE, ShellLayout, Strip, TITLE_BAR, placement,
};
use egui::vec2;

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
        (layout.device_view.left(), layout.device_view.right()),
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
        if placement.element != Element::Message {
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
    // Live's default state and the Status Bar's note readout, which shares the
    // message area.
    let not_placed = ["L-CTRL-002", "L-CTRL-003", "L-CTRL-008", "L-STAT-003"];
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
    assert!(layout.device_view.width() > 0.0 && layout.device_view.right() < pane.left());
    let message = layout.rect(Element::Message);
    let clip = layout.rect(Element::ClipViewToggle);
    assert!(message.right() <= clip.left(), "{message:?} {clip:?}");
    for (element, rect) in &layout.elements {
        if placement(*element).strip == Strip::StatusBar {
            assert!(rect.right() <= pane.left(), "{element:?}");
        }
    }
}

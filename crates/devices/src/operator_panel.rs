//! Operator's panel as Live lays it out (layout L-DEV-105): the four
//! oscillators' rows down the left, a display with the fields of the row
//! clicked last in the middle, and the LFO, the filter, the pitch envelope
//! and the global controls in four rows at the right, each at the place
//! measured on Live 12.4.16 on 2 and 3 November 2026.

use bl_model::device_layout::{
    BODY_HEIGHT, BandLabel, BandRule, Control, ControlBand, DisplayCell, LayoutNode, PLAIN_HEIGHT,
    Page, PageStyle, PagesNode, Widget,
};
use bl_model::{DeviceLayout, ParamId};

use crate::fm::OPERATORS;
use crate::operator::*;

/// The name of the pages node whose page is the row the display shows.
pub const ROWS: &str = "Sections";
/// The name of the cell that draws the shown row's envelope or partials, and
/// of the global page's, which draws nothing of its own.
pub const CURVE: &str = "Display";
pub const GLOBAL: &str = "Global";

const LEFT: f32 = 197.5;
const DISPLAY: f32 = 274.4;
const RIGHT: f32 = 196.7;
/// The curve's height, and what is left of the body under it for the fields.
const CURVE_HEIGHT: f32 = 54.3;
const FIELDS: f32 = BODY_HEIGHT - CURVE_HEIGHT;
/// A row of the oscillators' column, its height and the pitch between two.
const ROW: f32 = 39.6;

/// The left columns of the display's fields, the right half's two, and the
/// tops of the three rows of fields, from the top of the fields' band.
const COLUMNS: [f32; 4] = [4.7, 51.0, 91.2, 131.6];
const HALF: [f32; 2] = [179.8, 226.4];
const LINES: [f32; 3] = [16.3, 44.3, 72.1];
const FIELD_WIDTH: f32 = 40.0;

fn plain(param: ParamId, x: f32, y: f32, trim: u8) -> Control {
    Control::new(param, Widget::Unboxed, x, y)
        .sized(FIELD_WIDTH, PLAIN_HEIGHT)
        .trimmed(trim)
}

fn chooser(param: ParamId, x: f32, y: f32, trim: u8) -> Control {
    Control::new(param, Widget::UnboxedChooser, x, y)
        .sized(FIELD_WIDTH, PLAIN_HEIGHT)
        .trimmed(trim)
}

/// A value on the line of a row's names (`name`) or of its values.
fn value(param: ParamId, x: f32, line: f32, on_names: bool) -> Control {
    let y = if on_names { line - 1.5 } else { line + 10.5 };
    Control::new(param, Widget::Unboxed, x, y)
        .unnamed()
        .sized(36.0, 12.0)
}

fn word(text: &str, x: f32, line: f32) -> BandLabel {
    BandLabel::left(text, x, line, 40.0)
}

/// An envelope's fields in the left half of the display: its times over its
/// levels over its loop, with `extra` the controls that sit in the free
/// places (a oscillator's velocity and key; the shaped envelopes' end).
fn envelope(
    [attack, decay, release, time_vel]: [ParamId; 4],
    [initial, peak, sustain]: [ParamId; 3],
    (loop_mode, loop_time, repeat): (ParamId, ParamId, ParamId),
    (trim, time_trim, repeat_trim): (u8, u8, u8),
) -> Vec<Control> {
    vec![
        plain(attack, COLUMNS[0], LINES[0], trim),
        plain(decay, COLUMNS[1], LINES[0], trim),
        plain(release, COLUMNS[2], LINES[0], trim),
        plain(time_vel, COLUMNS[3], LINES[0], trim),
        plain(initial, COLUMNS[0], LINES[1], trim),
        plain(peak, COLUMNS[1], LINES[1], trim),
        plain(sustain, COLUMNS[2], LINES[1], trim),
        chooser(loop_mode, COLUMNS[0], LINES[2], trim),
        plain(loop_time, COLUMNS[1], LINES[2], time_trim).shown_while(loop_mode, 1),
        plain(repeat, COLUMNS[1], LINES[2], repeat_trim).shown_between(loop_mode, 2, 3),
    ]
}

/// The fields of a page: the envelope's, a rule and a heading over each half,
/// and the right half's own controls.
fn fields(controls: Vec<Control>, mut labels: Vec<BandLabel>, headings: [&str; 2]) -> LayoutNode {
    labels.push(BandLabel::left(headings[0], 12.5, 2.4, 60.0));
    labels.push(BandLabel::left(headings[1], 187.5, 2.4, 60.0));
    let band = ControlBand::free(DISPLAY, FIELDS, controls)
        .with_labels(labels)
        .with_rules(vec![
            BandRule::across(4.7, 12.2, 169.0),
            BandRule::across(179.2, 12.2, 91.5),
        ]);
    LayoutNode::Column(vec![
        LayoutNode::Cell(DisplayCell::new(CURVE, DISPLAY, CURVE_HEIGHT, 1.0)),
        LayoutNode::Band(band),
    ])
}

/// Oscillator `op`'s page.
fn oscillator(op: usize) -> Page {
    let id = |c| osc(op, c);
    let mut controls = envelope(
        [
            id(ENV_ATTACK),
            id(ENV_DECAY),
            id(ENV_RELEASE),
            id(ENV_TIME_VEL),
        ],
        [id(ENV_INITIAL), id(ENV_PEAK), id(ENV_SUSTAIN)],
        (id(ENV_LOOP), id(ENV_LOOP_TIME), id(ENV_REPEAT)),
        (1, 2, 2),
    );
    controls.extend([
        plain(id(OSC_LEVEL_VEL), COLUMNS[3], LINES[1], 3),
        plain(id(OSC_LEVEL_KEY), COLUMNS[3], LINES[2], 3),
        chooser(id(OSC_WAVE), HALF[0], LINES[0], 1),
        plain(id(OSC_FEEDBACK), HALF[0], LINES[1], 1),
        Control::new(
            id(OSC_REPEAT_ON),
            Widget::UnboxedChooser,
            HALF[1],
            LINES[1] + 10.5,
        )
        .unnamed()
        .sized(24.0, 12.0),
        Control::new(
            id(OSC_REPEAT),
            Widget::Unboxed,
            HALF[1] + 25.5,
            LINES[1] + 10.5,
        )
        .unnamed()
        .sized(22.0, 12.0)
        .shown_while(id(OSC_REPEAT_ON), 1),
        Control::new(
            id(OSC_RETRIGGER),
            Widget::Abbreviated(1),
            HALF[0],
            LINES[2] + 10.5,
        )
        .sized(11.0, 12.0)
        .trimmed(1),
        value(id(OSC_PHASE), HALF[0] + 14.0, LINES[2], false).sized(30.0, 12.0),
        // Narrower than a field: Q sits at its right.
        plain(id(OSC_VEL), HALF[1], LINES[2], 1).sized(26.0, PLAIN_HEIGHT),
        Control::new(
            id(OSC_Q),
            Widget::Abbreviated(1),
            HALF[1] + 28.0,
            LINES[2] + 10.5,
        )
        .sized(11.0, 12.0)
        .trimmed(1),
    ]);
    let labels = vec![
        word("Repeat", HALF[1], LINES[1]),
        word("Phase", HALF[0], LINES[2]),
    ];
    let name = ["A", "B", "C", "D"][op];
    Page::new(
        &format!("Osc {name}"),
        fields(controls, labels, ["Envelope", "Oscillator"]),
    )
    .with_select(&[
        id(OSC_COARSE),
        id(OSC_FINE),
        id(OSC_FIXED),
        id(OSC_LEVEL),
        id(OSC_ON),
    ])
}

/// The fields every shaped envelope (the LFO's, the filter's, the pitch's)
/// has: its times, its levels, its loop and its End, in the left half.
fn shaped_envelope(base: u32) -> Vec<Control> {
    let id = |c| shaped(base, c);
    let mut controls = envelope(
        [
            id(SHAPED_ATTACK),
            id(SHAPED_DECAY),
            id(SHAPED_RELEASE),
            id(SHAPED_TIME_VEL),
        ],
        [id(SHAPED_INITIAL), id(SHAPED_PEAK), id(SHAPED_SUSTAIN)],
        (id(SHAPED_LOOP), id(SHAPED_LOOP_TIME), id(SHAPED_REPEAT)),
        (1, 2, 1),
    );
    controls.push(plain(id(SHAPED_END), COLUMNS[2], LINES[2], 1));
    controls
}

/// The five buttons A B C D and `last` of an Aux or LFO destination row.
fn destination_buttons(first: u32, last: (ParamId, u8, u8)) -> Vec<Control> {
    let y = LINES[0] + 10.5;
    let mut controls: Vec<Control> = (0..OPERATORS as u32)
        .map(|op| {
            Control::new(
                ParamId(first + op),
                Widget::Abbreviated(1),
                HALF[0] - 2.6 + f32::from(op as u8) * 14.2,
                y,
            )
            .sized(11.4, 12.0)
            .trimmed(2)
        })
        .collect();
    let (param, chars, trim) = last;
    controls.push(
        Control::new(
            param,
            Widget::Abbreviated(chars),
            HALF[0] - 2.6 + 4.0 * 14.2,
            y,
        )
        .sized(15.0, 12.0)
        .trimmed(trim),
    );
    controls
}

fn lfo_page() -> Page {
    let mut controls = shaped_envelope(LFO_ENV);
    controls.extend(destination_buttons(LFO_OSC, (LFO_FILTER, 3, 1)));
    controls.extend([
        value(LFO_AMOUNT_A, 226.0, LINES[0], true),
        value(LFO_AMOUNT_B, 226.0, LINES[1], true),
        Control::new(LFO_DEST_B, Widget::UnboxedChooser, HALF[0], LINES[1] + 10.5)
            .unnamed()
            .sized(60.0, 12.0),
        plain(LFO_RATE_KEY, HALF[0], LINES[2], 1),
        plain(LFO_AMT_VEL, HALF[1], LINES[2], 1),
    ]);
    let labels = vec![
        word("Dest. A", HALF[0], LINES[0]),
        word("Dest. B", HALF[0], LINES[1]),
    ];
    Page::new("LFO", fields(controls, labels, ["Envelope", "LFO"])).with_select(&[
        LFO_ON,
        LFO_WAVE,
        LFO_RANGE,
        LFO_RETRIGGER,
        LFO_RATE,
        LFO_AMOUNT,
    ])
}

fn filter_page() -> Page {
    let mut controls = shaped_envelope(FILTER_ENV);
    controls.extend([
        plain(FREQ_ENV, COLUMNS[3], LINES[1], 2),
        plain(FILTER_DRIVE, HALF[0], LINES[0], 1),
        plain(FREQ_VEL, HALF[0], LINES[1], 0),
        plain(SHAPER_DRIVE, HALF[1], LINES[1], 1),
        plain(FREQ_KEY, HALF[0], LINES[2], 0),
        plain(SHAPER_WET, HALF[1], LINES[2], 1),
        Control::new(
            SHAPER_TYPE,
            Widget::UnboxedChooser,
            HALF[1],
            LINES[0] + 10.5,
        )
        .unnamed()
        .sized(40.0, 12.0),
    ]);
    let labels = vec![word("Shaper", HALF[1], LINES[0])];
    Page::new("Filter", fields(controls, labels, ["Envelope", "Filter"])).with_select(&[
        FILTER_ON,
        FILTER_TYPE,
        FILTER_SLOPE,
        FILTER_CIRCUIT,
        FILTER_FREQ,
        FILTER_RES,
    ])
}

fn pitch_page() -> Page {
    let mut controls = shaped_envelope(PITCH_ENV);
    controls.extend(destination_buttons(PITCH_OSC, (PITCH_LFO, 3, 1)));
    controls.extend([
        value(PITCH_AMOUNT_A, 226.0, LINES[0], true),
        value(PITCH_AMOUNT_B, 226.0, LINES[1], true),
        Control::new(
            PITCH_DEST_B,
            Widget::UnboxedChooser,
            HALF[0],
            LINES[1] + 10.5,
        )
        .unnamed()
        .sized(60.0, 12.0),
        Control::new(GLIDE_ON, Widget::Abbreviated(1), HALF[0], LINES[2] + 10.5).sized(12.0, 12.0),
        plain(GLIDE_TIME, HALF[1], LINES[2], 1),
    ]);
    let labels = vec![
        word("Dest. A", HALF[0], LINES[0]),
        word("Dest. B", HALF[0], LINES[1]),
        word("Glide", HALF[0], LINES[2]),
    ];
    Page::new("Pitch", fields(controls, labels, ["Envelope", "Aux"])).with_select(&[
        PITCH_ON,
        PITCH_AMOUNT,
        SPREAD,
        TRANSPOSE,
    ])
}

/// The global page: the eleven algorithms in a row, the table of the five
/// global modulation sources, and the voices and the pan along its foot.
fn global_page() -> Page {
    let mut controls =
        vec![Control::new(ALGORITHM, Widget::IconSegments(11), 8.0, 0.5).sized(250.0, 12.0)];
    let mut labels = Vec::new();
    for (k, source) in TARGET_SOURCES.into_iter().enumerate() {
        let y = 46.0 + 15.0 * k as f32;
        labels.push(BandLabel::left(source, 15.6, y, 50.0));
        controls.push(
            Control::new(target_dest(k, false), Widget::UnboxedChooser, 68.3, y)
                .unnamed()
                .sized(44.0, 12.0),
        );
        controls.push(
            Control::new(target_amount(k, false), Widget::Unboxed, 127.8, y)
                .unnamed()
                .sized(32.0, 12.0),
        );
        match k {
            1 | 3 => {
                labels.push(BandLabel::left(
                    if k == 1 { "Time" } else { "Pitch" },
                    164.4,
                    y,
                    30.0,
                ));
                let param = if k == 1 { TIME_KEY } else { PB_RANGE };
                controls.push(
                    Control::new(param, Widget::Unboxed, 223.9, y)
                        .unnamed()
                        .sized(36.0, 12.0),
                );
            }
            _ => {
                controls.push(
                    Control::new(target_dest(k, true), Widget::UnboxedChooser, 164.4, y)
                        .unnamed()
                        .sized(44.0, 12.0),
                );
                controls.push(
                    Control::new(target_amount(k, true), Widget::Unboxed, 223.9, y)
                        .unnamed()
                        .sized(32.0, 12.0),
                );
            }
        }
    }
    for (text, x) in [
        ("Conn. A", 66.7),
        ("Amt. A", 126.7),
        ("Conn. B", 164.4),
        ("Amt. B", 223.3),
    ] {
        labels.push(BandLabel::left(text, x, 34.7, 40.0));
    }
    for (text, x) in [("Rtg", 45.6), ("Interpol.", 66.7), ("Antialias", 108.9)] {
        labels.push(BandLabel::left(text, x, 118.9, 40.0));
    }
    controls.extend([
        chooser(VOICES, 11.0, 118.9, 0).sized(30.0, PLAIN_HEIGHT),
        Control::new(RETRIGGER, Widget::Abbreviated(1), 45.6, 126.9).sized(11.7, 12.0),
        Control::new(INTERPOLATION, Widget::Check, 66.7, 126.9)
            .sized(15.6, 12.0)
            .unnamed(),
        Control::new(ANTIALIAS, Widget::Check, 108.9, 126.9)
            .sized(15.6, 12.0)
            .unnamed(),
        plain(PAN, 151.7, 118.9, 0).sized(26.0, PLAIN_HEIGHT),
        plain(PAN_KEY, 180.0, 118.9, 0).sized(38.0, PLAIN_HEIGHT),
        plain(PAN_RANDOM, 221.1, 118.9, 0),
    ]);
    let band = ControlBand::free(DISPLAY, BODY_HEIGHT, controls)
        .with_labels(labels)
        .with_rules(vec![
            BandRule::across(4.0, 29.8, 266.0),
            BandRule::across(4.0, 113.5, 266.0),
        ]);
    Page::new(
        "Global",
        LayoutNode::Cell(DisplayCell::new(GLOBAL, DISPLAY, BODY_HEIGHT, 1.0).with_top(band)),
    )
    .with_select(&[TIME, TONE, VOLUME])
}

/// The oscillators' rows, down the left: D at the top and A at the foot.
fn rows() -> LayoutNode {
    let mut controls = Vec::new();
    let mut labels = Vec::new();
    for (row, op) in (0..OPERATORS).rev().enumerate() {
        let y = ROW * row as f32;
        let id = |c| osc(op, c);
        let mini = |param, x: f32| Control::new(param, Widget::MiniKnob, x, y + 1.7).trimmed(1);
        controls.extend([
            mini(id(OSC_COARSE), 4.5),
            mini(id(OSC_FINE), 59.6),
            Control::new(id(OSC_FIXED), Widget::Check, 106.2, y + 18.4)
                .sized(9.0, 9.0)
                .unnamed(),
            mini(id(OSC_LEVEL), 129.2),
            Control::new(id(OSC_ON), Widget::Abbreviated(1), 178.0, y + 13.0).sized(11.2, 11.2),
        ]);
        labels.push(BandLabel::new("Fixed", 99.9, y + 1.7, 24.0));
    }
    LayoutNode::Band(ControlBand::free(LEFT, BODY_HEIGHT, controls).with_labels(labels))
}

/// The four rows at the right: LFO, Filter, Pitch and the global controls.
fn right() -> LayoutNode {
    let mini = |param, x: f32, y: f32| Control::new(param, Widget::MiniKnob, x, y).trimmed(1);
    let whole = |param, x: f32, y: f32| Control::new(param, Widget::MiniKnob, x, y);
    let boxed = |param, x: f32, y: f32, width: f32| {
        Control::new(param, Widget::BareChooser, x, y).sized(width, 12.0)
    };
    let controls = vec![
        // The LFO's.
        Control::new(LFO_ON, Widget::Check, 7.3, 4.3)
            .sized(11.5, 11.5)
            .unnamed(),
        boxed(LFO_WAVE, 7.3, 20.5, 44.9),
        boxed(LFO_RANGE, 56.0, 20.5, 16.8),
        Control::new(LFO_RETRIGGER, Widget::Abbreviated(1), 76.7, 20.5)
            .sized(11.2, 12.0)
            .trimmed(1),
        mini(LFO_RATE, 94.9, 2.5),
        mini(LFO_AMOUNT, 148.0, 2.5),
        // The filter's.
        Control::new(FILTER_ON, Widget::Check, 7.3, 45.0)
            .sized(11.5, 11.5)
            .unnamed(),
        boxed(FILTER_TYPE, 51.3, 44.1, 36.3),
        Control::new(FILTER_SLOPE, Widget::Segments(2), 7.3, 60.7).sized(38.0, 11.0),
        boxed(FILTER_CIRCUIT, 51.3, 59.8, 36.3),
        mini(FILTER_FREQ, 95.2, 42.0),
        mini(FILTER_RES, 146.8, 42.0),
        // The pitch envelope's.
        Control::new(PITCH_ON, Widget::Check, 7.3, 95.2)
            .sized(11.5, 11.5)
            .unnamed(),
        whole(PITCH_AMOUNT, 43.5, 81.8),
        whole(SPREAD, 95.0, 81.8),
        whole(TRANSPOSE, 146.0, 81.8),
        // The global ones.
        whole(TIME, 44.0, 123.4),
        whole(TONE, 95.0, 123.4),
        whole(VOLUME, 146.0, 123.4),
    ];
    let labels = vec![
        BandLabel::left("LFO", 22.1, 5.4, 30.0),
        BandLabel::left("Filter", 22.1, 46.7, 30.0),
    ];
    let rules = [38.0, 77.5, 117.5]
        .map(|y| BandRule::across(2.0, y, RIGHT - 4.0))
        .to_vec();
    LayoutNode::Band(
        ControlBand::free(RIGHT, BODY_HEIGHT, controls)
            .with_labels(labels)
            .with_rules(rules),
    )
}

/// Operator's layout.
pub(crate) fn layout() -> DeviceLayout {
    let gap = |width| LayoutNode::Band(ControlBand::free(width, BODY_HEIGHT, Vec::new()));
    let mut pages: Vec<Page> = (0..OPERATORS).map(oscillator).collect();
    pages.extend([lfo_page(), filter_page(), pitch_page(), global_page()]);
    DeviceLayout::new(LayoutNode::Row(vec![
        rows(),
        gap(2.3),
        LayoutNode::Pages(PagesNode::new(ROWS, PageStyle::Regions, pages)),
        gap(2.8),
        right(),
    ]))
}

//! Drift's panel as Live lays it out (layout L-DEV-065): the oscillators and
//! the mix on a black ground, the filter, the two envelopes over their
//! display, the LFO and Mod tabs, and the global controls at the right,
//! each at the place measured on Live 12.4.16 on 2 November 2026.

use bl_model::device_layout::{
    BODY_HEIGHT, BandLabel, BandRule, Control, ControlBand, DisplayCell, Glyph, LayoutNode, Page,
    PageStyle, PagesNode, TitlePart, Widget,
};
use bl_model::{DeviceLayout, ParamId};

use crate::polysynth::*;

/// The name of the cell that holds the oscillators and the mix.
pub const OSCILLATORS: &str = "Oscillators";
/// The names of the envelope display's pages, which the buttons 1 and 2 show,
/// and of its cell.
pub const ENVELOPE_VIEW: &str = "Envelope View";
pub const ENVELOPE: &str = "Envelope";
/// The names of the LFO's cell and of the tabs' node.
pub const LFO_DISPLAY: &str = "LFO";
pub const TABS: &str = "Tabs";
/// The Mod tab's cell.
pub const MOD_DISPLAY: &str = "Mod";

/// Where the pitch modulation's display sits in the oscillators' cell:
/// left, top, width and height.
pub const PITCH_DISPLAY: [f32; 4] = [85.4, 106.2, 84.3, 47.5];

const OSCILLATORS_WIDTH: f32 = 254.0;
const FILTER_WIDTH: f32 = 96.5;
const ENVELOPES_WIDTH: f32 = 203.5;
const MODULATION_WIDTH: f32 = 93.5;
const GLOBAL_WIDTH: f32 = 114.8;
/// The heights of Envelope 1's row, the display and Envelope 2's row.
const ROW_1: f32 = 54.3;
const DISPLAY: f32 = 49.0;
const ROW_2: f32 = BODY_HEIGHT - ROW_1 - DISPLAY;

fn small(param: ParamId, centre: f32, top: f32) -> Control {
    Control::new(param, Widget::SmallKnob, centre - 16.0, top)
}

fn label(text: &str, centre: f32, top: f32) -> BandLabel {
    BandLabel::new(text, centre - 22.0, top, 44.0)
}

/// The oscillators, the mix and the pitch modulation.
fn oscillators() -> LayoutNode {
    let tall = |param, widget, x, y| Control::new(param, widget, x, y).sized(11.6, 23.7);
    let controls = vec![
        Control::new(OSC1_WAVE, Widget::UnboxedChooser, 12.0, 25.5)
            .unnamed()
            .sized(30.0, 12.0),
        small(OSC1_OCT, 64.7, 4.4).trimmed(2),
        small(OSC1_SHAPE, 104.5, 4.4).trimmed(2),
        Control::new(SHAPE_MOD_SOURCE, Widget::BareChooser, 128.0, 11.3).sized(38.6, 12.0),
        Control::new(SHAPE_MOD_AMOUNT, Widget::Bare, 128.0, 26.1).sized(38.6, 12.0),
        Control::new(OSC2_WAVE, Widget::UnboxedChooser, 12.0, 80.5)
            .unnamed()
            .sized(30.0, 12.0),
        small(OSC2_OCT, 83.7, 57.8).trimmed(2),
        small(OSC2_DETUNE, 135.0, 57.8).trimmed(2),
        Control::new(PITCH_MOD1_SOURCE, Widget::UnboxedChooser, 7.4, 127.5)
            .unnamed()
            .sized(34.0, 12.0),
        Control::new(PITCH_MOD2_SOURCE, Widget::UnboxedChooser, 46.9, 127.5)
            .unnamed()
            .sized(34.0, 12.0),
        Control::new(PITCH_MOD1_AMOUNT, Widget::Unboxed, 7.4, 142.4)
            .unnamed()
            .sized(34.0, 12.0),
        Control::new(PITCH_MOD2_AMOUNT, Widget::Unboxed, 46.9, 142.4)
            .unnamed()
            .sized(34.0, 12.0),
        tall(OSC1, Widget::Abbreviated(1), 179.2, 13.6).trimmed(1),
        tall(OSC2, Widget::Abbreviated(1), 179.2, 67.0).trimmed(1),
        tall(NOISE, Widget::Abbreviated(1), 179.2, 120.5),
        small(OSC1_GAIN, 211.3, 4.4).trimmed(3),
        small(OSC2_GAIN, 211.3, 57.8).trimmed(3),
        small(NOISE_GAIN, 211.3, 111.2).trimmed(2),
        tall(OSC1_FILTER, Widget::IconToggle, 233.8, 13.6).with_glyph(Glyph::Right),
        tall(OSC2_FILTER, Widget::IconToggle, 233.8, 67.0).with_glyph(Glyph::Right),
        tall(NOISE_FILTER, Widget::IconToggle, 233.8, 120.5).with_glyph(Glyph::Right),
        Control::new(RETRIGGER, Widget::Abbreviated(1), 178.6, 47.4).sized(12.0, 12.0),
    ];
    let labels = vec![
        label("Osc 1", 26.7, 14.2),
        label("Osc 2", 26.7, 67.0),
        BandLabel::left("Pitch Mod", 5.3, 116.0, 70.0),
        label("Osc 1", 211.3, 4.4),
        label("Osc 2", 211.3, 57.8),
        label("Noise", 211.3, 111.2),
    ];
    let rules = vec![
        BandRule::across(5.3, 52.8, 172.0),
        BandRule::across(190.6, 52.8, 59.4),
        BandRule::across(5.3, 106.2, 75.4),
        BandRule::across(172.0, 106.2, 78.0),
    ];
    LayoutNode::Cell(
        DisplayCell::new(OSCILLATORS, OSCILLATORS_WIDTH, BODY_HEIGHT, 1.0).with_top(
            ControlBand::free(OSCILLATORS_WIDTH, BODY_HEIGHT, controls)
                .with_labels(labels)
                .with_rules(rules),
        ),
    )
}

/// The filter's column.
fn filter() -> LayoutNode {
    let name = |p, rest: &[Control]| {
        let mut all = vec![p];
        all.extend_from_slice(rest);
        all
    };
    let controls = name(
        Control::new(FREQ, Widget::Knob, 8.8, 8.9),
        &[
            Control::new(FILTER_TYPE, Widget::Cycle, 56.3, 19.6).sized(26.0, 12.0),
            Control::new(KEY, Widget::Number, 56.3, 36.1).sized(26.6, 21.0),
            small(RES, 30.2, 68.2),
            small(HP, 69.1, 68.2),
            Control::new(FREQ_MOD1_SOURCE, Widget::BareChooser, 4.2, 127.9).sized(41.2, 12.0),
            Control::new(FREQ_MOD2_SOURCE, Widget::BareChooser, 48.9, 127.9).sized(41.6, 12.0),
            Control::new(FREQ_MOD1_AMOUNT, Widget::BareSlider, 4.2, 142.4).sized(41.2, 12.0),
            Control::new(FREQ_MOD2_AMOUNT, Widget::BareSlider, 48.9, 142.4).sized(41.6, 12.0),
        ],
    );
    LayoutNode::Band(
        ControlBand::free(FILTER_WIDTH, BODY_HEIGHT, controls)
            .with_labels(vec![
                label("Type", 69.7, 4.4),
                BandLabel::left("Freq Mod", 3.0, 116.0, 50.0),
            ])
            .with_rules(vec![BandRule::across(50.0, 123.0, 42.0)]),
    )
}

/// An envelope's row of four knobs.
fn row(knobs: [ParamId; 4]) -> Vec<Control> {
    knobs
        .iter()
        .zip([38.6, 84.6, 130.6, 176.6])
        .map(|(&p, centre)| small(p, centre, 4.4).trimmed(2))
        .collect()
}

/// Both envelopes with the display between them.
fn envelopes() -> LayoutNode {
    let first = ControlBand::free(
        ENVELOPES_WIDTH,
        ROW_1,
        row([ENV1_ATTACK, ENV1_DECAY, ENV1_SUSTAIN, ENV1_RELEASE]),
    )
    .with_labels(vec![BandLabel::new("1", 4.0, 4.4, 12.0)]);
    // Envelope 2's knobs give way to the cycling envelope's while it cycles:
    // Rate (with its time modes), Tilt and Hold, in the places of Attack,
    // Decay and Sustain, and the modes in Release's.
    let adsr = |p: ParamId, centre: f32| small(p, centre, 6.7).trimmed(2).shown_while(ENV2_MODE, 0);
    let cycling =
        |p: ParamId, centre: f32| small(p, centre, 6.7).trimmed(2).shown_while(ENV2_MODE, 1);
    let second = ControlBand::free(
        ENVELOPES_WIDTH,
        ROW_2,
        vec![
            adsr(ENV2_ATTACK, 38.6),
            adsr(ENV2_DECAY, 84.6),
            adsr(ENV2_SUSTAIN, 130.6),
            adsr(ENV2_RELEASE, 176.6),
            cycling(CYC_RATE, 38.6),
            cycling(CYC_TILT, 84.6),
            cycling(CYC_HOLD, 130.6),
            Control::new(
                CYC_MODE,
                Widget::Grid {
                    columns: 2,
                    rows: 2,
                },
                156.0,
                17.7,
            )
            .sized(41.0, 27.0)
            .shown_while(ENV2_MODE, 1),
            Control::new(ENV2_MODE, Widget::IconToggle, 4.5, 23.7)
                .sized(12.0, 12.0)
                .with_glyph(Glyph::Loop),
        ],
    )
    .with_labels(vec![BandLabel::new("2", 4.0, 6.7, 12.0)]);
    let view = |page: &str| {
        let cell = DisplayCell::new(ENVELOPE, ENVELOPES_WIDTH, DISPLAY, 1.0);
        Page::new(page, LayoutNode::Cell(cell))
    };
    let display = PagesNode::new(
        ENVELOPE_VIEW,
        PageStyle::Placed,
        vec![
            view("1").with_button(3.3, 3.6, 13.0, 12.0),
            view("2").with_button(3.3, 34.1, 13.0, 12.0),
        ],
    );
    LayoutNode::Column(vec![
        LayoutNode::Band(first),
        LayoutNode::Pages(display),
        LayoutNode::Band(second),
    ])
}

/// The LFO tab: its rate and time modes over its display, over its amount and
/// its own modulation.
fn lfo() -> LayoutNode {
    let rate = ControlBand::free(
        MODULATION_WIDTH,
        ROW_1,
        vec![
            small(LFO_RATE, 24.7, 4.4).trimmed(1),
            Control::new(
                LFO_MODE,
                Widget::Grid {
                    columns: 2,
                    rows: 2,
                },
                52.0,
                13.6,
            )
            .sized(39.0, 27.0),
        ],
    );
    let display =
        DisplayCell::new(LFO_DISPLAY, MODULATION_WIDTH, DISPLAY, 1.0).with_top(ControlBand::free(
            MODULATION_WIDTH,
            16.0,
            vec![
                Control::new(LFO_SHAPE, Widget::UnboxedChooser, 4.5, 2.5)
                    .unnamed()
                    .sized(62.0, 12.0),
                Control::new(LFO_RETRIGGER, Widget::Abbreviated(1), 80.7, 3.0)
                    .sized(12.0, 12.0)
                    .trimmed(1),
            ],
        ));
    let amount = ControlBand::free(
        MODULATION_WIDTH,
        ROW_2,
        vec![
            small(LFO_AMOUNT, 24.7, 6.7).trimmed(1),
            Control::new(LFO_MOD_SOURCE, Widget::BareChooser, 50.5, 17.7).sized(37.7, 12.0),
            Control::new(LFO_MOD_AMOUNT, Widget::Bare, 50.5, 32.6).sized(37.7, 12.0),
        ],
    )
    .with_labels(vec![label("Mod", 60.0, 6.7)]);
    LayoutNode::Column(vec![
        LayoutNode::Band(rate),
        LayoutNode::Cell(display),
        LayoutNode::Band(amount),
    ])
}

/// The Mod tab: three slots, each a source, its amount and its destination
/// under it.
fn modulation() -> LayoutNode {
    let slot = |source, amount, dest, top: f32| {
        [
            Control::new(source, Widget::UnboxedChooser, 8.0, top)
                .unnamed()
                .sized(78.0, 12.0),
            Control::new(amount, Widget::Unboxed, 8.0, top + 14.0)
                .unnamed()
                .sized(60.0, 12.0),
            Control::new(dest, Widget::UnboxedChooser, 16.0, top + 28.0)
                .unnamed()
                .sized(70.0, 12.0),
        ]
    };
    let mut controls = Vec::new();
    let mut rules = Vec::new();
    for (n, (s, a, d)) in [
        (MOD1_SOURCE, MOD1_AMOUNT, MOD1_DEST),
        (MOD2_SOURCE, MOD2_AMOUNT, MOD2_DEST),
        (MOD3_SOURCE, MOD3_AMOUNT, MOD3_DEST),
    ]
    .into_iter()
    .enumerate()
    {
        let top = 8.0 + 52.0 * n as f32;
        controls.extend(slot(s, a, d, top));
        rules.push(BandRule::down(4.0, top + 14.0, 20.0));
        rules.push(BandRule::across(4.0, top + 34.0, 8.0));
        if n > 0 {
            rules.push(BandRule::across(4.0, top - 6.0, MODULATION_WIDTH - 8.0));
        }
    }
    LayoutNode::Cell(
        DisplayCell::new(MOD_DISPLAY, MODULATION_WIDTH, BODY_HEIGHT, 1.0)
            .with_top(ControlBand::free(MODULATION_WIDTH, BODY_HEIGHT, controls).with_rules(rules)),
    )
}

/// The global controls at the right: Mode, Voices, Drift, Legato and Glide
/// down the first column and Volume, Vel > Vol, Transpose, Note PB and PB
/// Range down the second.
fn global() -> LayoutNode {
    const A: f32 = 6.8;
    const B: f32 = 62.9;
    const WIDE: f32 = 46.9;
    let boxed = |p, widget, x, y, height| Control::new(p, widget, x, y).sized(WIDE, height);
    let controls = vec![
        boxed(VOICE_MODE, Widget::Chooser, A, 4.4, 21.0).trimmed(1),
        boxed(THICKNESS, Widget::Slider, A, 25.9, 21.0).shown_while(VOICE_MODE, 1),
        boxed(SPREAD, Widget::Slider, A, 25.9, 21.0).shown_while(VOICE_MODE, 2),
        boxed(STRENGTH, Widget::Slider, A, 25.9, 21.0).shown_while(VOICE_MODE, 3),
        boxed(VOICES, Widget::Chooser, A, 47.5, 21.0),
        boxed(DRIFT, Widget::Slider, A, 75.6, 21.0),
        boxed(LEGATO, Widget::Toggle, A, 114.5, 12.0).greyed_while(VOICE_MODE, 0),
        boxed(GLIDE, Widget::Bare, A, 142.4, 12.0),
        small(VOLUME, 72.7, 4.4),
        boxed(VEL_VOL, Widget::Slider, B, 47.5, 21.0),
        boxed(TRANSPOSE, Widget::Number, B, 75.6, 21.0),
        boxed(NOTE_PB, Widget::Toggle, B, 114.5, 12.0),
        boxed(PB_RANGE, Widget::Slider, B, 131.7, 21.0),
    ];
    LayoutNode::Band(
        ControlBand::free(GLOBAL_WIDTH, BODY_HEIGHT, controls)
            .with_labels(vec![BandLabel::left("Glide", A, 131.7, 40.0)])
            .with_rules(vec![BandRule::down(58.8, 4.0, 150.0)]),
    )
}

/// Drift's layout.
pub(crate) fn layout() -> DeviceLayout {
    let tabs = PagesNode::new(
        TABS,
        PageStyle::TitleBar,
        vec![Page::new("LFO", lfo()), Page::new("Mod", modulation())],
    )
    .with_title_at(554.0);
    DeviceLayout::new(LayoutNode::Row(vec![
        oscillators(),
        filter(),
        envelopes(),
        LayoutNode::Pages(tabs),
        global(),
    ]))
    .with_title(vec![
        TitlePart::Section("Osc Mix".to_owned(), 210.7),
        TitlePart::Section("Filter".to_owned(), 301.6),
        TitlePart::Section("Envelopes".to_owned(), 447.0),
    ])
}

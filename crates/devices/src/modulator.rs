//! The first two modulators (WP-5.10; the modulation spec, R12 to R23): the
//! LFO and the Shaper, named and behaving as Live's devices of those names.
//! Both are audio effects that leave their
//! audio alone: the engine works their output out before any track runs
//! ([`NativeDevice::control`]) and writes, for each of the eight slots, what
//! the slot contributes to its target (R14) or holds it at (R16).
//!
//! The output is worked out once every [`GRID`] frames, on a grid counted
//! from the frame the transport last started (from the engine's first frame
//! before it has ever started), and runs in a straight line from each value
//! to the next, so a target never steps and the same song plays the same
//! samples at any block size (R20). Every random part comes from generators
//! seeded by the device and started again when the transport starts (R21).

use bl_core::time::{Frames, SampleRate, TempoMap};
use bl_dsp::Random;
use bl_engine::modbus::{NOT_HELD, Shape};
use bl_engine::{DeviceType, NodeData, Registry};
use bl_model::device_layout::{
    BODY_HEIGHT, BandLabel, Control, ControlBand, DeviceLayout, DisplayCell, Glyph, LayoutNode,
    Page, PageStyle, PagesNode, Unit, Widget,
};
use bl_model::modulator::{
    self, CURVE_MOD, DEPTH, MODE, MULTIMAP_HIDDEN, MULTIMAP_NODE, MULTIMAP_SHOWN, POLARITY, SLOTS,
    ShapePoint, WAVE_MOD, default_shape, shape_value,
};
use bl_model::{DeviceDescription, DeviceRole, ParamDescriptor, ParamId, ParamUnit};

use crate::framework::{Modulation, NativeDevice, ParamBank, add};

/// The frames between the points a modulator's output is worked out at: the
/// plug-in kit's control rate (R20).
pub const GRID: u64 = 32;

// The parameters both modulators share, beside Depth, Mod and Polarity
// (the model's).
/// Jitter: randomness added to the output, from 0 % to 100 %.
pub const JITTER: ParamId = ParamId(0);
/// Smooth: softens sharp changes, from 0 % to 100 %.
pub const SMOOTH: ParamId = ParamId(1);
/// The time-mode switch: the rate in Hz or in beat divisions.
pub const TIME_MODE: ParamId = ParamId(2);
/// The rate in Hz.
pub const RATE_HZ: ParamId = ParamId(3);
/// Multiplies the rate in Hz by 10.
pub const TIMES_TEN: ParamId = ParamId(4);
/// The rate in beat divisions of the song's tempo.
pub const RATE_BEATS: ParamId = ParamId(5);
/// Offset: moves the output's centre, from −100 % to 100 %.
pub const OFFSET: ParamId = ParamId(6);
/// Phase: where in the cycle it starts, from 0 % to 100 %.
pub const PHASE: ParamId = ParamId(7);
/// R: turning it on restarts the cycle from its Phase.
pub const RESTART: ParamId = ParamId(8);

// The LFO's own.
/// The waveform.
pub const WAVE: ParamId = ParamId(10);
/// Shape: bends the waveform.
pub const SHAPE: ParamId = ParamId(11);
/// Steps: turns the waveform into steps.
pub const STEPS: ParamId = ParamId(12);
/// Hold: freezes the output.
pub const HOLD: ParamId = ParamId(13);

// The Shaper's own.
/// The trigger mode: Loop, 1-Shot or Manual.
pub const TRIGGER_MODE: ParamId = ParamId(10);
/// T: turning it on plays the shape once, in 1-Shot mode.
pub const TRIGGER: ParamId = ParamId(11);
/// Manual: the point of the shape the output takes, in Manual mode.
pub const MANUAL: ParamId = ParamId(12);
pub use bl_model::modulator::{GRID_DIVISIONS, GRIDS, SNAP};

/// The LFO's waveforms, in the order of its chooser (R22).
pub const WAVES: [&str; 9] = [
    "Sine", "Up", "Down", "Triangle", "Square", "Random", "Bin", "Stray", "Glider",
];

/// The rate's beat divisions, in the order of its chooser, with each one's
/// length in quarter notes (Beatlayer's own).
pub const DIVISIONS: [(&str, f64); 9] = [
    ("1/32", 0.125),
    ("1/16", 0.25),
    ("1/8", 0.5),
    ("1/4", 1.0),
    ("1/2", 2.0),
    ("1 bar", 4.0),
    ("2 bars", 8.0),
    ("4 bars", 16.0),
    ("8 bars", 32.0),
];

/// The Shaper's trigger modes.
pub const TRIGGER_MODES: [&str; 3] = ["Loop", "1-Shot", "Manual"];

/// The LFO's own parameters, which CV LFO shares, and the shared ones.
pub(crate) fn wave_params() -> Vec<ParamDescriptor> {
    let mut params = vec![
        ParamDescriptor::choice(WAVE, "Waveform", &WAVES, 0)
            .describe("The waveform: sine, rising or falling ramps, triangle, square, or one of the random waves."),
        ParamDescriptor::continuous(SHAPE, "Shape", (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .decimals(0)
            .describe("Bends the waveform. Inactive for Random, Bin, Stray and Glider."),
        ParamDescriptor::integer(STEPS, "Steps", (0, 24), 0, ParamUnit::None)
            .describe("Turns the waveform into this many steps; 0 leaves it smooth. Inactive for Random, Bin and Square."),
        ParamDescriptor::toggle(HOLD, "Hold", false)
            .describe("Freezes the output. Released, it carries on from the held value."),
    ];
    params.extend(shared_params());
    params
}

/// The banks of the LFO's controls, which CV LFO shares.
pub(crate) const WAVE_BANKS: [(&str, &[&str]); 2] = [
    (
        "Wave",
        &["Waveform", "Shape", "Steps", "Hold", "Jitter", "Smooth"],
    ),
    (
        "Timing",
        &[
            "Time Mode",
            "Rate",
            "x10",
            "Beat Rate",
            "Offset",
            "Phase",
            "R",
        ],
    ),
];

/// The LFO's type.
#[must_use]
pub fn wave_mod_type() -> DeviceType {
    let mut params = wave_params();
    params.extend(mapping_params(
        SLOTS,
        (
            "Depth",
            "How far the modulator reaches: 0 % none, 100 % full.",
        ),
    ));
    DeviceType::native(WAVE_MOD, "LFO", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_alternate_at((TIME_MODE, 1), RATE_HZ, RATE_BEATS)
            .with_relabels(vec![rate_label(RATE_BEATS, TIME_MODE)])
            .with_layout(wave_layout())
            .with_banks(&[
                WAVE_BANKS[0],
                WAVE_BANKS[1],
                ("Mapping", &["Mod", "Polarity", "Depth"]),
                (
                    "Targets 1-2",
                    &["Amount 1", "Min 1", "Max 1", "Amount 2", "Min 2", "Max 2"],
                ),
                (
                    "Targets 3-4",
                    &["Amount 3", "Min 3", "Max 3", "Amount 4", "Min 4", "Max 4"],
                ),
                (
                    "Targets 5-6",
                    &["Amount 5", "Min 5", "Max 5", "Amount 6", "Min 6", "Max 6"],
                ),
                (
                    "Targets 7-8",
                    &["Amount 7", "Min 7", "Max 7", "Amount 8", "Min 8", "Max 8"],
                ),
            ]),
    )
}

/// The label a synced rate or delay reads under, Live's: the knob is one
/// control whichever unit is lit.
pub(crate) fn rate_label(synced: ParamId, mode: ParamId) -> bl_model::param::Relabel {
    bl_model::param::Relabel {
        param: synced,
        by: mode,
        labels: vec![bl_model::param::ParamLabel::named("Rate")],
    }
}

/// The Shaper's own parameters, which CV Shaper shares, and the shared ones.
pub(crate) fn curve_params() -> Vec<ParamDescriptor> {
    let mut params = vec![
        ParamDescriptor::choice(TRIGGER_MODE, "Trigger Mode", &TRIGGER_MODES, 0).describe(
            "Loop repeats the shape at the rate, 1-Shot plays it once each time T turns on, and Manual outputs the point of the shape the Manual control picks.",
        ),
        ParamDescriptor::toggle(TRIGGER, "T", false)
            .describe("Turning it on plays the shape once in 1-Shot mode."),
        ParamDescriptor::continuous(MANUAL, "Manual", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .decimals(0)
            .describe("The point of the shape the output takes in Manual mode."),
        ParamDescriptor::choice(
            GRID_DIVISIONS,
            "Grid",
            &GRIDS.map(|(name, _)| name),
            1,
        )
        .describe("The display's divisions of the cycle."),
        ParamDescriptor::toggle(SNAP, "Snap", false)
            .describe("Snaps breakpoints to the grid as they are placed and dragged."),
    ];
    params.extend(shared_params());
    params
}

/// The banks of the Shaper's controls, which CV Shaper shares.
pub(crate) const CURVE_BANKS: [(&str, &[&str]); 2] = [
    (
        "Shape",
        &[
            "Trigger Mode",
            "T",
            "Manual",
            "Grid",
            "Snap",
            "Jitter",
            "Smooth",
        ],
    ),
    (
        "Timing",
        &[
            "Time Mode",
            "Rate",
            "x10",
            "Beat Rate",
            "Offset",
            "Phase",
            "R",
        ],
    ),
];

/// The Shaper's type.
#[must_use]
pub fn curve_mod_type() -> DeviceType {
    // Live's Shaper reads its Phase to two decimals and has no ×10: its Rate
    // is in sync or in Hz.
    let mut params: Vec<_> = curve_params()
        .into_iter()
        .filter(|p| p.id != TIMES_TEN)
        .map(|p| match p.id {
            PHASE => p.decimals(2),
            // Live's Shaper starts with its Rate in sync, and Snap lit.
            TIME_MODE => ParamDescriptor::choice(TIME_MODE, "Time Mode", &["Hz", "Beats"], 1)
                .describe("Sets the rate in Hz, running freely, or in beat divisions, locked to the song position."),
            SNAP => ParamDescriptor::toggle(SNAP, "Snap", true)
                .describe("Snaps breakpoints to the grid as they are placed and dragged."),
            _ => p,
        })
        .collect();
    params.extend(mapping_params(
        SLOTS,
        (
            "Depth",
            "How far the modulator reaches: 0 % none, 100 % full.",
        ),
    ));
    DeviceType::native(CURVE_MOD, "Shaper", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_alternate_at((TIME_MODE, 1), RATE_HZ, RATE_BEATS)
            .with_relabels(vec![rate_label(RATE_BEATS, TIME_MODE)])
            .with_layout(curve_layout())
            .with_banks(&[
                CURVE_BANKS[0],
                (
                    "Timing",
                    &["Time Mode", "Rate", "Beat Rate", "Offset", "Phase", "R"],
                ),
                ("Mapping", &["Mod", "Polarity", "Depth"]),
                (
                    "Targets 1-2",
                    &["Amount 1", "Min 1", "Max 1", "Amount 2", "Min 2", "Max 2"],
                ),
                (
                    "Targets 3-4",
                    &["Amount 3", "Min 3", "Max 3", "Amount 4", "Min 4", "Max 4"],
                ),
                (
                    "Targets 5-6",
                    &["Amount 5", "Min 5", "Max 5", "Amount 6", "Min 6", "Max 6"],
                ),
                (
                    "Targets 7-8",
                    &["Amount 7", "Min 7", "Max 7", "Amount 8", "Min 8", "Max 8"],
                ),
            ]),
    )
}

/// The parameters both modulators share.
pub(crate) fn shared_params() -> Vec<ParamDescriptor> {
    vec![
        ParamDescriptor::continuous(JITTER, "Jitter", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .decimals(0)
            .describe("Adds randomness to the output."),
        ParamDescriptor::continuous(SMOOTH, "Smooth", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .decimals(0)
            .describe("Softens sharp changes in the output."),
        ParamDescriptor::choice(TIME_MODE, "Time Mode", &["Hz", "Beats"], 0)
            .describe("Sets the rate in Hz, running freely, or in beat divisions, locked to the song position."),
        ParamDescriptor::continuous(RATE_HZ, "Rate", (0.01, 40.0), 1.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("Cycles per second."),
        ParamDescriptor::toggle(TIMES_TEN, "x10", false).describe("Multiplies the rate in Hz by 10."),
        ParamDescriptor::choice(
            RATE_BEATS,
            "Beat Rate",
            &DIVISIONS.map(|(name, _)| name),
            3,
        )
        .describe("The length of one cycle, in beat divisions of the song's tempo."),
        ParamDescriptor::continuous(OFFSET, "Offset", (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .decimals(0)
            .describe("Moves the output's centre; 0 % leaves it centred. The output stays within 0 to 1."),
        ParamDescriptor::continuous(PHASE, "Phase", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .decimals(0)
            .describe("Where in the cycle it starts, as a share of the cycle."),
        ParamDescriptor::toggle(RESTART, "R", false)
            .describe("Turning it on restarts the cycle from its Phase."),
    ]
}

/// Mod, Polarity, Depth (named `depth.0`, described by `depth.1`) and each
/// of `slots` slots' Amount, Min and Max (R13, R14, R16).
pub(crate) fn mapping_params(slots: u8, depth: (&str, &str)) -> Vec<ParamDescriptor> {
    mapping_params_with(slots, Some(depth), (["Mod", "Remote"], 0))
}

/// The same, with the Mod switch's two names and Polarity's default
/// (`0` Bipolar, `1` Unipolar), and no Depth where `depth` is `None`.
pub(crate) fn mapping_params_with(
    slots: u8,
    depth: Option<(&str, &str)>,
    (mode_names, polarity): ([&str; 2], usize),
) -> Vec<ParamDescriptor> {
    let mut params = vec![
        ParamDescriptor::choice(MODE, "Mod", &mode_names, 0).describe(
            "Modulation moves each target round its own value. Remote Control sets each target between its Min and Max.",
        ),
        ParamDescriptor::choice(POLARITY, "Polarity", &["Bipolar", "Unipolar"], polarity).describe(
            "Bipolar swings both ways round the target's value, Unipolar one way from it.",
        ),
    ];
    if let Some(depth) = depth {
        params.push(
            ParamDescriptor::continuous(DEPTH, depth.0, (0.0, 100.0), 100.0, ParamUnit::Percent)
                .decimals(0)
                .describe(depth.1),
        );
    }
    for slot in 0..slots {
        let n = slot + 1;
        params.push(
            ParamDescriptor::continuous(
                modulator::amount(slot),
                &format!("Amount {n}"),
                (-100.0, 100.0),
                50.0,
                ParamUnit::Percent,
            )
            .decimals(0)
            .describe("How much of the target's travel the modulation reaches in Modulation mode; negative inverts it."),
        );
        params.push(
            ParamDescriptor::continuous(
                modulator::min(slot),
                &format!("Min {n}"),
                (0.0, 100.0),
                0.0,
                ParamUnit::Percent,
            )
            .decimals(0)
            .describe("Where on its travel the target sits at the modulator's lowest, in Remote Control mode."),
        );
        params.push(
            ParamDescriptor::continuous(
                modulator::max(slot),
                &format!("Max {n}"),
                (0.0, 100.0),
                100.0,
                ParamUnit::Percent,
            )
            .decimals(0)
            .describe("Where on its travel the target sits at the modulator's highest, in Remote Control mode."),
        );
    }
    params
}

/// The cells of the slots' mappings: slot 1's inside the body, the others
/// in the part Show Multimap adds (R17).
pub(crate) fn mapping_cells(slots: u8) -> (LayoutNode, LayoutNode) {
    let cell = |slot: u8| {
        LayoutNode::Cell(
            DisplayCell::new(&format!("Mapping {}", slot + 1), 180.0, BODY_HEIGHT, 1.0)
                .with_bottom(ControlBand::stacked(
                    62.0,
                    &[
                        (modulator::amount(slot), Widget::Knob),
                        (modulator::min(slot), Widget::Knob),
                        (modulator::max(slot), Widget::Knob),
                    ],
                )),
        )
    };
    (cell(0), LayoutNode::Row((1..slots).map(cell).collect()))
}

/// The LFO's controls and display, which CV LFO shares (cv-tools R34): the
/// waveform's band, its display and the rate's band.
pub(crate) fn wave_parts() -> [LayoutNode; 3] {
    let wave = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (WAVE, Widget::Chooser),
            (SHAPE, Widget::Knob),
            (STEPS, Widget::Number),
            (JITTER, Widget::Knob),
            (SMOOTH, Widget::Knob),
        ],
    );
    let rate = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (TIME_MODE, Widget::Chooser),
            (RATE_HZ, Widget::Knob),
            (TIMES_TEN, Widget::Toggle),
            (RATE_BEATS, Widget::Chooser),
            (PHASE, Widget::Knob),
            (OFFSET, Widget::Knob),
            (HOLD, Widget::Toggle),
            (RESTART, Widget::Toggle),
        ],
    );
    [
        LayoutNode::Band(wave),
        LayoutNode::Cell(DisplayCell::new("Wave", 150.0, BODY_HEIGHT, 1.0)),
        LayoutNode::Band(rate),
    ]
}

/// The width of a modulator's display cell, in points (layout L-DEV-044:
/// Envelope Follower's is 234 by 97).
pub(crate) const CELL_WIDTH: f32 = 234.0;

/// The height of the mapping row along the top of a display cell: a 12-point
/// button 3 points from the edge.
pub(crate) const MAPPING_ROW: f32 = 18.0;

/// Where the mapping row's parts sit along the cell, in points. The Map
/// button, at the cell's left, and the Show Multimap button, at its right,
/// are the panel's own; the rest are controls (layout L-DEV-016).
const MOD_X: f32 = 85.0;
const POLARITY_X: f32 = 129.0;
const AMOUNT_X: f32 = 147.0;
const MAX_X: f32 = 168.0;

/// The mapping row of slot `slot`'s controls at height `y` in a band: Mod, and
/// beside it Polarity and Amount, or while the Mod switch reads Remote the
/// Min and Max that replace them.
fn mapping_controls(slot: u8, y: f32, with_mode: bool) -> Vec<Control> {
    let at = Control::new;
    let mut controls = Vec::new();
    if with_mode {
        controls.push(at(MODE, Widget::Cycle, MOD_X, y).sized(39.0, 12.0));
        controls.push(
            at(POLARITY, Widget::IconToggle, POLARITY_X, y)
                .sized(12.0, 12.0)
                .with_glyph(Glyph::PlusMinus)
                .shown_only_while(MODE, 0),
        );
    }
    controls.push(
        at(modulator::amount(slot), Widget::Bare, AMOUNT_X, y)
            .sized(30.0, 12.0)
            .shown_only_while(MODE, 0),
    );
    controls.push(
        at(modulator::min(slot), Widget::Bare, POLARITY_X, y)
            .sized(36.0, 12.0)
            .shown_only_while(MODE, 1),
    );
    controls.push(
        at(modulator::max(slot), Widget::Bare, MAX_X, y)
            .sized(36.0, 12.0)
            .shown_only_while(MODE, 1),
    );
    controls
}

/// The row along the top of a modulator's display cell for its first slot:
/// Map, Mod, Polarity, Amount and Show Multimap (layout L-DEV-016).
pub(crate) fn mapping_row() -> ControlBand {
    ControlBand::free(CELL_WIDTH, MAPPING_ROW, mapping_controls(0, 3.0, true))
}

/// The same row in a band `height` tall that also holds the device's own
/// `extra` controls and `labels`, down the cell's right-hand side.
pub(crate) fn mapping_row_with(
    height: f32,
    extra: Vec<Control>,
    labels: Vec<bl_model::device_layout::BandLabel>,
) -> ControlBand {
    let mut controls = mapping_controls(0, 3.0, true);
    controls.extend(extra);
    ControlBand::free(CELL_WIDTH, height, controls).with_labels(labels)
}

/// The list Show Multimap shows in place of the whole panel: a row of Map,
/// Mod, Polarity and Amount for each of the eight slots, 15 points apart. The
/// first slot's controls are the panel's own, and every row's Mod and Polarity
/// are the device's one pair, so the other seven slots' controls are all
/// this cell places.
fn multimap_page() -> LayoutNode {
    let controls = (1..SLOTS)
        .flat_map(|slot| mapping_controls(slot, 3.0 + 15.0 * f32::from(slot), false))
        .collect();
    LayoutNode::Cell(
        DisplayCell::new("Multimap", CELL_WIDTH, BODY_HEIGHT - 8.0, 1.0)
            .with_inset(4.0)
            .with_top(ControlBand::free(CELL_WIDTH, 126.0, controls)),
    )
}

/// A modulator's layout: its own `panel`, which Show Multimap replaces with
/// the list of its eight mappings.
pub(crate) fn mapped_layout(panel: LayoutNode) -> DeviceLayout {
    DeviceLayout::new(LayoutNode::Pages(PagesNode::new(
        MULTIMAP_NODE,
        PageStyle::Switch,
        vec![
            Page::new(MULTIMAP_HIDDEN, panel),
            Page::new(MULTIMAP_SHOWN, multimap_page()),
        ],
    )))
}

/// The Live LFO's panel (layout L-DEV-016): the waveform display with its
/// mapping row along the top and Waveform, Shape, Steps, Jitter and Smooth
/// along the foot; under it Rate with its Hz and sync buttons and ×10, Depth,
/// Offset, and Phase over Hold and R.
fn wave_panel() -> LayoutNode {
    let at = Control::new;
    let cell = DisplayCell::new("Wave", CELL_WIDTH, 96.0, 1.0)
        .with_inset(4.0)
        .with_top(mapping_row())
        .with_bottom(ControlBand::free(
            CELL_WIDTH,
            30.0,
            vec![
                at(WAVE, Widget::Chooser, 4.0, 5.0),
                at(SHAPE, Widget::Number, 62.0, 5.0).sized(40.0, 21.0),
                at(STEPS, Widget::Number, 105.0, 5.0).sized(40.0, 21.0),
                at(JITTER, Widget::Number, 152.0, 5.0).sized(40.0, 21.0),
                at(SMOOTH, Widget::Number, 194.0, 5.0).sized(40.0, 21.0),
            ],
        ));
    let controls = ControlBand::free(
        CELL_WIDTH + 8.0,
        54.0,
        vec![
            at(RATE_HZ, Widget::SmallKnob, 15.0, 4.0)
                .with_unit_right(TIME_MODE, [Unit::Hz, Unit::Sync]),
            at(TIMES_TEN, Widget::Checkbox, 57.0, 36.0).sized(40.0, 12.0),
            at(DEPTH, Widget::SmallKnob, 98.0, 4.0),
            at(OFFSET, Widget::SmallKnob, 141.0, 4.0),
            at(PHASE, Widget::Number, 189.0, 6.0).sized(40.0, 21.0),
            at(HOLD, Widget::Toggle, 189.0, 29.0).sized(25.0, 12.0),
            at(RESTART, Widget::Round, 216.0, 29.0),
        ],
    );
    LayoutNode::Column(vec![LayoutNode::Cell(cell), LayoutNode::Band(controls)])
}

fn wave_layout() -> DeviceLayout {
    mapped_layout(wave_panel())
}

/// The Shaper's controls and display, which CV Shaper shares (cv-tools
/// R35): the trigger's band, the shape's display and the rate's band.
pub(crate) fn curve_parts() -> [LayoutNode; 3] {
    let controls = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (TRIGGER_MODE, Widget::Chooser),
            (TRIGGER, Widget::Toggle),
            (MANUAL, Widget::Knob),
            (JITTER, Widget::Knob),
            (SMOOTH, Widget::Knob),
        ],
    );
    let display =
        DisplayCell::new("Curve", 200.0, BODY_HEIGHT, 2.0).with_bottom(ControlBand::stacked(
            44.0,
            &[(GRID_DIVISIONS, Widget::Chooser), (SNAP, Widget::Toggle)],
        ));
    let rate = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (TIME_MODE, Widget::Chooser),
            (RATE_HZ, Widget::Knob),
            (TIMES_TEN, Widget::Toggle),
            (RATE_BEATS, Widget::Chooser),
            (PHASE, Widget::Knob),
            (OFFSET, Widget::Knob),
            (RESTART, Widget::Toggle),
        ],
    );
    [
        LayoutNode::Band(controls),
        LayoutNode::Cell(display),
        LayoutNode::Band(rate),
    ]
}

/// The Live Shaper's panel (layout L-DEV-016): the shape display with its
/// mapping row along the top and the presets, Grid and Snap along the foot; at
/// the right the output display with Jitter, Smooth, R, Phase and Offset
/// inside, the Loop, 1-Shot and Manual row, and Rate with its Hz and sync
/// buttons beside Depth.
fn curve_panel() -> LayoutNode {
    let at = Control::new;
    let curve = DisplayCell::new("Curve", 265.0, 150.0, 3.0)
        .with_inset(4.0)
        .with_top(mapping_row())
        .with_bottom(ControlBand::free(
            265.0,
            20.0,
            vec![
                at(GRID_DIVISIONS, Widget::Inline, 182.0, 4.0).sized(46.0, 12.0),
                at(SNAP, Widget::Toggle, 234.0, 4.0).sized(28.0, 12.0),
            ],
        ));
    let output = DisplayCell::new("Output", 137.0, 74.0, 1.0)
        .with_inset(4.0)
        .with_bottom(
            ControlBand::free(
                137.0,
                38.0,
                vec![
                    at(JITTER, Widget::Inline, 9.0, 5.0).sized(60.0, 12.0),
                    at(SMOOTH, Widget::Inline, 72.0, 5.0).sized(60.0, 12.0),
                    at(RESTART, Widget::Round, 4.0, 22.0)
                        .greyed_while(TIME_MODE, 1)
                        .hidden_while(TRIGGER_MODE, 2),
                    at(PHASE, Widget::Bare, 31.0, 22.0).sized(38.0, 12.0),
                    at(OFFSET, Widget::Inline, 74.0, 22.0).sized(46.0, 12.0),
                    at(TRIGGER, Widget::Round, 122.0, 22.0).shown_only_while(TRIGGER_MODE, 1),
                ],
            )
            .with_labels(vec![BandLabel::new("Ø", 19.0, 23.5, 11.0)]),
        );
    let controls = ControlBand::free(
        145.0,
        76.0,
        vec![
            at(TRIGGER_MODE, Widget::Segments(3), 6.0, 4.0).sized(129.0, 12.0),
            at(RATE_HZ, Widget::SmallKnob, 8.0, 20.0)
                .with_unit_right(TIME_MODE, [Unit::Hz, Unit::Sync])
                .hidden_while(TRIGGER_MODE, 2),
            at(MANUAL, Widget::SmallKnob, 8.0, 20.0).shown_only_while(TRIGGER_MODE, 2),
            at(DEPTH, Widget::SmallKnob, 98.0, 20.0),
        ],
    );
    LayoutNode::Row(vec![
        LayoutNode::Cell(curve),
        LayoutNode::Column(vec![LayoutNode::Cell(output), LayoutNode::Band(controls)]),
    ])
}

fn curve_layout() -> DeviceLayout {
    mapped_layout(curve_panel())
}

/// Adds the two modulators to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, wave_mod_type(), |device, config| {
        Modulator::new(WaveSource::new(), config.sample_rate, device.id.0.get())
    });
    add(registry, curve_mod_type(), |device, config| {
        Modulator::new(
            CurveSource::new(device.shape.clone().unwrap_or_else(default_shape)),
            config.sample_rate,
            device.id.0.get(),
        )
    });
}

/// How fast a cycle runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Rate {
    /// Cycles per second, running freely.
    Hz(f64),
    /// The length of one cycle in quarter notes, locked to the song.
    Beats(f64),
}

/// What a source needs to know about the instant it works out.
pub(crate) struct Eval<'a> {
    pub(crate) rate: SampleRate,
    pub(crate) tempo: &'a TempoMap,
    /// The song frame of the instant while the song plays; its position
    /// otherwise.
    pub(crate) song: i64,
    /// Song frames per frame of the stretch (the transport spec, R24).
    pub(crate) speed: f64,
    /// The frame of the stretch whose parameters the instant is worked out
    /// from.
    pub(crate) f: usize,
    /// The frames between this instant and the last one worked out: [`GRID`]
    /// for a modulator, 1 for a CV device (cv-tools R33).
    pub(crate) step: u64,
}

impl Eval<'_> {
    /// Parameter `id` at the frame the instant is worked out from.
    pub(crate) fn param(&self, params: &ParamBank, id: ParamId) -> f64 {
        at(params, id, self.f)
    }

    /// The song position of `frame`, in quarter notes.
    fn quarters(&self, frame: i64) -> f64 {
        self.tempo
            .frames_to_ticks(Frames(frame), self.rate)
            .as_quarters()
    }
}

/// The shared parameters at the instant a source works out.
#[derive(Clone, Copy)]
pub(crate) struct Common {
    rate: Rate,
    /// Phase, as a share of the cycle.
    phase: f64,
    restart: bool,
}

/// A modulator's own part: the shape of its output.
pub(crate) trait Source: Send + 'static {
    /// Starts again: the transport started.
    fn restart(&mut self, seed: u64);
    /// The output at the instant `e`, from 0 to 1, before Jitter, Smooth and
    /// Offset; `None` to keep the last output (the LFO's Hold).
    fn eval(&mut self, e: &Eval<'_>, c: Common, params: &ParamBank) -> Option<f64>;
    /// Takes new data (the Shaper's shape).
    fn receive(&mut self, data: &NodeData);
}

/// Reads the value of parameter `id` at frame `f` of the stretch.
pub(crate) fn at(params: &ParamBank, id: ParamId, f: usize) -> f64 {
    params
        .index(id)
        .map_or(0.0, |index| f64::from(params.ramp(index)[f]))
}

/// The parameters of a modulator's mappings, looked up once, and the rows it
/// writes from its output (R13, R14, R16).
pub(crate) struct Mapper {
    depth: usize,
    mode: usize,
    polarity: usize,
    amounts: [usize; SLOTS as usize],
    mins: [usize; SLOTS as usize],
    maxes: [usize; SLOTS as usize],
}

impl Mapper {
    pub(crate) fn new(params: &ParamBank) -> Self {
        let index = |id: ParamId| params.index(id).expect("the description holds it");
        Self {
            depth: index(DEPTH),
            mode: index(MODE),
            polarity: index(POLARITY),
            amounts: std::array::from_fn(|s| index(modulator::amount(s as u8))),
            mins: std::array::from_fn(|s| index(modulator::min(s as u8))),
            maxes: std::array::from_fn(|s| index(modulator::max(s as u8))),
        }
    }

    /// Writes every slot's two rows for frame `f` of the stretch, from the
    /// output `u` (0 to 1) of the modulator on that frame.
    pub(crate) fn write(
        &self,
        modulation: &mut Modulation<'_, '_>,
        params: &ParamBank,
        f: usize,
        u: f64,
    ) {
        let depth = f64::from(params.ramp(self.depth)[f]) / 100.0;
        let remote = params.ramp(self.mode)[f] >= 0.5;
        let unipolar = params.ramp(self.polarity)[f] >= 0.5;
        for slot in 0..SLOTS as usize {
            let amount = f64::from(params.ramp(self.amounts[slot])[f]) / 100.0;
            let (contribution, hold) = if remote {
                let low = f64::from(params.ramp(self.mins[slot])[f]) / 100.0;
                let high = f64::from(params.ramp(self.maxes[slot])[f]) / 100.0;
                (0.0, (low + depth * u * (high - low)).clamp(0.0, 1.0) as f32)
            } else {
                let swing = if unipolar { u } else { 2.0 * u - 1.0 };
                ((depth * amount * swing) as f32, NOT_HELD)
            };
            modulation.row_mut(slot * 2)[f] = contribution;
            modulation.row_mut(slot * 2 + 1)[f] = hold;
        }
    }
}

/// A source with the output stage the LFO and the Shaper share with CV LFO
/// and CV Shaper (modulation R21, R22; cv-tools R33): Jitter, Smooth and
/// Offset on what the source gives. The modulators work it out once every
/// [`GRID`] frames, the CV devices on every frame, from this one law.
pub(crate) struct Signal<S> {
    source: S,
    seed: u64,
    jitter: Random,
    smoothed: f64,
    /// A value was worked out since the start.
    primed: bool,
    last: f64,
}

impl<S: Source> Signal<S> {
    pub(crate) fn new(mut source: S, seed: u64) -> Self {
        source.restart(seed);
        Self {
            source,
            seed,
            jitter: Random::seeded(seed),
            smoothed: 0.0,
            primed: false,
            last: 0.0,
        }
    }

    /// The transport started: everything begins again (R20, R21).
    pub(crate) fn restart(&mut self) {
        self.primed = false;
        self.smoothed = 0.0;
        self.last = 0.0;
        self.jitter = Random::seeded(self.seed ^ 0x5EED);
        self.source.restart(self.seed);
    }

    /// Takes new data (the Shaper's shape).
    pub(crate) fn receive(&mut self, data: &NodeData) {
        self.source.receive(data);
    }

    /// The output at the instant `e`, from 0 to 1, worked out from the
    /// parameters at `e.f`. Jitter adds up to half its share of `noise`'s
    /// draw, made only while the source isn't held, whether or not Jitter is
    /// used, so the stream is the same whatever the control says (R21).
    pub(crate) fn value(
        &mut self,
        e: &Eval<'_>,
        params: &ParamBank,
        noise: impl FnOnce(&mut Random) -> f64,
    ) -> f64 {
        let hz = e.param(params, RATE_HZ)
            * if e.param(params, TIMES_TEN) >= 0.5 {
                10.0
            } else {
                1.0
            };
        let beats = DIVISIONS[(e.param(params, RATE_BEATS) as usize).min(DIVISIONS.len() - 1)].1;
        let c = Common {
            rate: if e.param(params, TIME_MODE) >= 0.5 {
                Rate::Beats(beats)
            } else {
                Rate::Hz(hz)
            },
            phase: e.param(params, PHASE) / 100.0,
            restart: e.param(params, RESTART) >= 0.5,
        };
        let primed = std::mem::replace(&mut self.primed, true);
        let Some(w) = self.source.eval(e, c, params) else {
            return self.last;
        };
        let noise = noise(&mut self.jitter);
        let w = (w + e.param(params, JITTER) / 100.0 * 0.5 * noise).clamp(0.0, 1.0);
        let smooth = e.param(params, SMOOTH) / 100.0;
        // Smooth's share is per grid step; a shorter step keeps the same
        // time constant.
        let kept = if e.step == GRID {
            0.98 * smooth
        } else {
            (0.98 * smooth).powf(e.step as f64 / GRID as f64)
        };
        self.smoothed = if primed {
            self.smoothed + (w - self.smoothed) * (1.0 - kept)
        } else {
            w
        };
        self.last = (self.smoothed + e.param(params, OFFSET) / 100.0).clamp(0.0, 1.0);
        self.last
    }
}

/// A modulator: a signal, with the clock that places its output on the grid
/// and the slot rows every modulator shares.
struct Modulator<S> {
    signal: Signal<S>,
    rate: SampleRate,
    /// Frames since the transport last started, or since the engine's first
    /// frame before it has.
    clock: u64,
    was_playing: bool,
    primed: bool,
    /// The output at the last grid point and the next one.
    from: f64,
    to: f64,
}

impl<S: Source> Modulator<S> {
    fn new(source: S, rate: SampleRate, seed: u64) -> Self {
        Self {
            signal: Signal::new(source, seed),
            rate,
            clock: 0,
            was_playing: false,
            primed: false,
            from: 0.0,
            to: 0.0,
        }
    }

    /// The transport started: everything begins again (R20, R21).
    fn restart(&mut self) {
        self.clock = 0;
        self.primed = false;
        self.signal.restart();
    }

    /// The output at `lookahead` frames past frame `f` of the stretch:
    /// worked out from the parameters at `f`.
    fn work_out(
        &mut self,
        modulation: &Modulation<'_, '_>,
        params: &ParamBank,
        f: usize,
        lookahead: i64,
    ) -> f64 {
        // Spec: modulation R20. The song position stands still while the
        // song is stopped, at any block size.
        let song = modulation.song_position().0;
        let e = Eval {
            rate: self.rate,
            tempo: modulation.tempo(),
            song: if modulation.playing() {
                song + ((f as i64 + lookahead) as f64 * modulation.speed()).round() as i64
            } else {
                song
            },
            speed: modulation.speed(),
            f,
            step: GRID,
        };
        self.signal.value(&e, params, Random::next_bipolar)
    }
}

impl<S: Source> NativeDevice for Modulator<S> {
    fn process(&mut self, _: &mut crate::Audio<'_, '_>, _: &ParamBank) {}

    fn is_modulator(&self) -> bool {
        true
    }

    fn receive_data(&mut self, data: NodeData) {
        self.signal.receive(&data);
    }

    fn control(&mut self, modulation: &mut Modulation<'_, '_>, params: &ParamBank) {
        let playing = modulation.playing();
        if playing && !self.was_playing {
            self.restart();
        }
        self.was_playing = playing;
        let mapper = Mapper::new(params);
        for f in 0..modulation.frames() {
            if self.clock.is_multiple_of(GRID) {
                if self.primed {
                    self.from = self.to;
                } else {
                    self.from = self.work_out(modulation, params, f, 0);
                    self.primed = true;
                }
                self.to = self.work_out(modulation, params, f, GRID as i64);
            }
            let t = (self.clock % GRID) as f64 / GRID as f64;
            let u = self.from + (self.to - self.from) * t;
            mapper.write(modulation, params, f, u);
            self.clock += 1;
        }
    }
}

/// Where a cycle is: the cycles run since it started, and what Hold, R and a
/// synced rate add to them.
#[derive(Default)]
struct Phasor {
    /// Cycles run at a free rate since the start or the last restart.
    cycles: f64,
    /// What a synced rate adds so the output carries on from where Hold or
    /// a restart left it.
    shift: f64,
    primed: bool,
    /// The total phase when Hold began, while it holds.
    held: Option<f64>,
    last_total: f64,
    restart: bool,
}

impl Phasor {
    /// The total phase, in cycles, at the instant `e` (R22): a free rate
    /// adds a step's worth each time it is asked, a synced one reads the
    /// song position, and Phase and R apply to both.
    fn total(&mut self, e: &Eval<'_>, c: Common) -> f64 {
        let (raw, synced) = match c.rate {
            Rate::Hz(hz) => {
                if self.primed && self.held.is_none() {
                    self.cycles += hz * e.step as f64 / e.rate.as_f64();
                }
                (self.cycles, false)
            }
            Rate::Beats(beats) => (e.quarters(e.song) / beats, true),
        };
        self.primed = true;
        if let Some(held) = self.held.take() {
            // Released: carry on from the held value.
            if synced {
                self.shift = held - raw - c.phase;
            }
        }
        if c.restart && !self.restart {
            // R turned on: back to Phase.
            if synced {
                self.shift = -raw;
            } else {
                self.cycles = 0.0;
            }
        }
        self.restart = c.restart;
        let total = if synced {
            raw + self.shift + c.phase
        } else {
            self.cycles + c.phase
        };
        // A sum of many small steps drifts by a rounding error, which would
        // put a cycle's end on either side of its start: snap it.
        let total = (total * 1e9).round() / 1e9;
        self.last_total = total;
        total
    }

    /// Freezes the phase where it is.
    fn hold(&mut self) {
        self.held.get_or_insert(self.last_total);
    }
}

/// The LFO's source: nine waveforms.
pub(crate) struct WaveSource {
    phasor: Phasor,
    /// The cycle the random draws are for.
    cycle: i64,
    random: Random,
    /// The draws of this cycle and the last, and the Stray walk.
    current: f64,
    previous: f64,
    walk: f64,
}

impl WaveSource {
    pub(crate) fn new() -> Self {
        let mut source = Self {
            phasor: Phasor::default(),
            cycle: 0,
            random: Random::seeded(0),
            current: 0.5,
            previous: 0.5,
            walk: 0.5,
        };
        source.restart(0);
        source
    }

    /// Draws the random values of every cycle up to `cycle` (a long jump
    /// draws a few).
    fn advance_to(&mut self, cycle: i64) {
        let steps = (cycle - self.cycle).clamp(0, 4);
        for _ in 0..steps {
            self.previous = self.current;
            self.current = (self.random.next_bipolar() + 1.0) / 2.0;
            let step = (self.random.next_bipolar()) * 0.25;
            let walked = self.walk + step;
            // Reflect at the ends so the walk stays within 0 to 1.
            self.walk = if walked < 0.0 {
                -walked
            } else if walked > 1.0 {
                2.0 - walked
            } else {
                walked
            }
            .clamp(0.0, 1.0);
        }
        self.cycle = cycle;
    }
}

impl WaveSource {
    /// The waveform `wave` at `total` cycles, from 0 to 1, bent by `shape`
    /// (−1 to 1) and cut into `steps` steps (R22): Shape bends none of the
    /// random waves, and Steps none of Random, Bin and Square.
    fn wave(&mut self, wave: usize, total: f64, shape: f64, steps: f64) -> f64 {
        let cycle = total.floor();
        self.advance_to(cycle as i64);
        let frac = total - cycle;
        let random = matches!(wave, 5..=8);
        let bent = if random {
            frac
        } else {
            frac.powf(4f64.powf(-shape))
        };
        let mut w = match wave {
            0 => 0.5 - 0.5 * (std::f64::consts::TAU * bent).cos(),
            1 => bent,
            2 => 1.0 - bent,
            3 => 1.0 - (2.0 * bent - 1.0).abs(),
            4 => f64::from(u8::from(bent < 0.5)),
            5 => self.current,
            6 => f64::from(u8::from(self.current >= 0.5)),
            7 => self.walk,
            _ => self.previous + (self.current - self.previous) * frac,
        };
        if steps >= 2.0 && !matches!(wave, 4..=6) {
            w = (w * (steps - 1.0)).round() / (steps - 1.0);
        }
        w.clamp(0.0, 1.0)
    }
}

impl Source for WaveSource {
    fn restart(&mut self, seed: u64) {
        self.phasor = Phasor::default();
        self.random = Random::seeded(seed);
        self.cycle = 0;
        self.current = (self.random.next_bipolar() + 1.0) / 2.0;
        self.previous = self.current;
        self.walk = 0.5;
    }

    fn eval(&mut self, e: &Eval<'_>, c: Common, params: &ParamBank) -> Option<f64> {
        if e.param(params, HOLD) >= 0.5 {
            self.phasor.hold();
            return None;
        }
        let total = self.phasor.total(e, c);
        Some(self.wave(
            e.param(params, WAVE) as usize,
            total,
            e.param(params, SHAPE) / 100.0,
            e.param(params, STEPS),
        ))
    }

    fn receive(&mut self, _: &NodeData) {}
}

/// The Shaper's source: a breakpoint shape.
pub(crate) struct CurveSource {
    points: Vec<ShapePoint>,
    phasor: Phasor,
    /// How far a 1-Shot has played, from 0 to 1, and whether it is playing.
    progress: f64,
    running: bool,
    triggered: bool,
}

impl CurveSource {
    pub(crate) fn new(mut points: Vec<ShapePoint>) -> Self {
        // Room for the longest shape, so a new one arriving on the audio
        // thread never allocates (R20).
        points.reserve(modulator::MAX_POINTS.saturating_sub(points.len()));
        Self {
            points,
            phasor: Phasor::default(),
            progress: 0.0,
            running: false,
            triggered: false,
        }
    }
}

impl Source for CurveSource {
    fn restart(&mut self, _: u64) {
        self.phasor = Phasor::default();
        self.progress = 0.0;
        self.running = false;
        self.triggered = false;
    }

    fn eval(&mut self, e: &Eval<'_>, c: Common, params: &ParamBank) -> Option<f64> {
        let first = |id: ParamId| e.param(params, id);
        let mode = first(TRIGGER_MODE) as usize;
        // Spec: modulation R23. R is inactive in beat divisions.
        let c = Common {
            restart: c.restart && matches!(c.rate, Rate::Hz(_)),
            ..c
        };
        let x = match mode {
            // Loop: the shape repeats at the rate.
            0 => self.phasor.total(e, c).rem_euclid(1.0),
            // Manual: the control's position picks the point.
            2 => first(MANUAL) / 100.0,
            // 1-Shot: T turning on starts it, and it rests at its end.
            _ => {
                let on = first(TRIGGER) >= 0.5;
                if on && !self.triggered {
                    self.progress = 0.0;
                    self.running = true;
                }
                self.triggered = on;
                let step = match c.rate {
                    Rate::Hz(hz) => hz * e.step as f64 / e.rate.as_f64(),
                    Rate::Beats(beats) => {
                        (e.quarters(e.song + (e.step as f64 * e.speed).round() as i64)
                            - e.quarters(e.song))
                            / beats
                    }
                };
                if self.running {
                    self.progress = (self.progress + step).min(1.0);
                    self.running = self.progress < 1.0;
                }
                self.progress
            }
        };
        Some(shape_value(&self.points, x).clamp(0.0, 1.0))
    }

    fn receive(&mut self, data: &NodeData) {
        if let Some(shape) = data.downcast_ref::<Shape>() {
            self.points.clone_from(&shape.0);
        }
    }
}

/// The LFO's display (R22): one cycle of its waveform as the controls
/// set it, or four of a random one, from its seed's first draws, with
/// Offset applied.
#[must_use]
pub fn wave_display(
    device: bl_model::DeviceId,
    description: &DeviceDescription,
    values: &bl_model::OrdMap<ParamId, f64>,
) -> bl_model::DeviceDisplay {
    const POINTS: usize = 96;
    let value = |id: ParamId| description.value(values, id).unwrap_or(0.0);
    let wave = value(WAVE) as usize;
    let cycles = if matches!(wave, 5..=8) { 4.0 } else { 1.0 };
    let mut source = WaveSource::new();
    let offset = value(OFFSET) / 100.0;
    let samples = (0..=POINTS)
        .map(|i| {
            let total = cycles * i as f64 / POINTS as f64;
            let w = source.wave(wave, total, value(SHAPE) / 100.0, value(STEPS));
            (w + offset).clamp(0.0, 1.0) as f32
        })
        .collect();
    bl_model::DeviceDisplay {
        spectrograms: Vec::new(),
        device,
        live: false,
        view: None,
        series: vec![bl_model::DisplaySeries {
            name: "Wave".to_owned(),
            unit: String::new(),
            x_unit: "cycles".to_owned(),
            x_range: [0.0, cycles as f32],
            log_x: false,
            y_range: [0.0, 1.0],
            detail: false,
            values: samples,
        }],
        markers: Vec::new(),
        readings: Vec::new(),
    }
}

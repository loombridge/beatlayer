//! The device layout model (WP-2.5, IMP-14; the device framework spec): a
//! device's body is fixed bands of controls and one or more stretchable
//! display cells (graphs, spectra, waveforms, envelope editors), nested in
//! rows and columns.
//!
//! The docked size, every band and cell at its own size, is the smallest a
//! device can be. Given more room, as a floating window (WP-2.5b) gives it,
//! the extra width and height go only to display cells, shared by their
//! weights; a band of controls keeps its size and stays pinned to the
//! edges around it, so controls never change size or spacing. A device
//! with no display cell doesn't grow.
//!
//! Sizes and positions are in points at 100%; the interface zoom (WP-1.17)
//! scales them, docked and floating alike. Placing a layout
//! ([`DeviceLayout::place`]) is pure arithmetic, so it's tested headless.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ids::ParamId;
use crate::param::{ParamDescriptor, ParamKind};

/// A device body's height in the Device View at 100%, in points: the view's
/// 188 points (layout L-DEV-001, observed in Live 12.4.3) less the 7-point
/// gaps above and below the frame and the 16-point title bar.
pub const BODY_HEIGHT: f32 = 158.0;

/// The space around and between the controls of a generic layout's band, in
/// points. (Live's spacing is to observe.)
pub const PADDING: f32 = 4.0;

/// A size in points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Size {
    /// The width.
    pub width: f32,
    /// The height.
    pub height: f32,
}

impl Size {
    /// A size.
    #[must_use]
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

/// A rectangle in points, from the body's top-left corner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Rect {
    /// Its left edge.
    pub x: f32,
    /// Its top edge.
    pub y: f32,
    /// Its width.
    pub width: f32,
    /// Its height.
    pub height: f32,
}

impl Rect {
    /// A rectangle.
    #[must_use]
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Its size.
    #[must_use]
    pub const fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }

    /// Its right edge.
    #[must_use]
    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    /// Its bottom edge.
    #[must_use]
    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    /// Whether `other` lies inside it.
    #[must_use]
    pub fn contains(&self, other: &Rect) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.right() <= self.right() + 1e-3
            && other.bottom() <= self.bottom() + 1e-3
    }
}

/// How a control draws its parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Widget {
    /// A round knob with its name above and its value below: continuous
    /// values and gains.
    Knob,
    /// A switch button with its name above: toggles.
    Toggle,
    /// A field that opens a menu of choices, with its name above.
    Chooser,
    /// A number field, dragged or typed, with its name above.
    Number,
}

impl Widget {
    /// The generic UI's widget for a parameter of `kind`.
    #[must_use]
    pub fn for_kind(kind: &ParamKind) -> Self {
        match kind {
            ParamKind::Continuous { .. } | ParamKind::Gain { .. } => Self::Knob,
            ParamKind::Toggle => Self::Toggle,
            ParamKind::Choice { .. } => Self::Chooser,
            ParamKind::Integer { .. } => Self::Number,
        }
    }

    /// The space the control takes, its labels included, in points.
    /// (Beatlayer's own sizes; Live's are to observe.)
    #[must_use]
    pub const fn size(self) -> Size {
        match self {
            Self::Knob => Size::new(48.0, 52.0),
            Self::Toggle | Self::Chooser | Self::Number => Size::new(48.0, 32.0),
        }
    }
}

/// One control in a band: a parameter, drawn by a widget, at a place in the
/// band.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Control {
    /// The parameter.
    pub param: ParamId,
    /// How it's drawn.
    pub widget: Widget,
    /// Its left edge, from the band's.
    pub x: f32,
    /// Its top edge, from the band's.
    pub y: f32,
}

impl Control {
    /// Its rectangle in its band.
    #[must_use]
    pub fn rect(&self) -> Rect {
        let size = self.widget.size();
        Rect::new(self.x, self.y, size.width, size.height)
    }
}

/// A band of controls, of a fixed size.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ControlBand {
    /// Its width.
    pub width: f32,
    /// Its height.
    pub height: f32,
    /// Its controls.
    pub controls: Vec<Control>,
}

impl ControlBand {
    /// A band `height` tall holding `controls` top to bottom, then in
    /// further columns left to right as each fills, with [`PADDING`] around
    /// and between them.
    #[must_use]
    pub fn stacked(height: f32, controls: &[(ParamId, Widget)]) -> Self {
        let mut placed = Vec::with_capacity(controls.len());
        let (mut x, mut y, mut column_width) = (PADDING, PADDING, 0.0f32);
        for (param, widget) in controls {
            let size = widget.size();
            if y > PADDING && y + size.height + PADDING > height {
                x += column_width + PADDING;
                y = PADDING;
                column_width = 0.0;
            }
            placed.push(Control {
                param: *param,
                widget: *widget,
                x,
                y,
            });
            y += size.height + PADDING;
            column_width = column_width.max(size.width);
        }
        Self {
            width: x + column_width + PADDING,
            height,
            controls: placed,
        }
    }
}

/// A display cell: a graph, a spectrum, a waveform, or an envelope editor,
/// which grows with the room the device has. It may hold a row of controls
/// inside its top edge and another inside its bottom edge (observed in
/// Live 12.4.6: Compressor's display has Thresh, GR, Output, and Out along
/// its top, and its view buttons, Knee, Look., and Env. along its bottom);
/// they keep their size and stay pinned to those edges as the cell grows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DisplayCell {
    /// What it shows, for the device that draws it and for screen readers.
    pub name: String,
    /// Its docked width, the least it has.
    pub width: f32,
    /// Its docked height, the least it has.
    pub height: f32,
    /// Its share of extra room, against the other cells it shares with.
    pub weight: f32,
    /// Controls inside its top edge, from its left.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top: Option<ControlBand>,
    /// Controls inside its bottom edge, from its left.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom: Option<ControlBand>,
}

impl DisplayCell {
    /// A cell named `name`, `width` by `height` docked, taking `weight`
    /// shares of extra room, with no controls inside it.
    #[must_use]
    pub fn new(name: &str, width: f32, height: f32, weight: f32) -> Self {
        Self {
            name: name.to_owned(),
            width,
            height,
            weight,
            top: None,
            bottom: None,
        }
    }

    /// The same, with `band` inside its top edge.
    #[must_use]
    pub fn with_top(mut self, band: ControlBand) -> Self {
        self.top = Some(band);
        self
    }

    /// The same, with `band` inside its bottom edge.
    #[must_use]
    pub fn with_bottom(mut self, band: ControlBand) -> Self {
        self.bottom = Some(band);
        self
    }
}

/// A part of a device's layout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LayoutNode {
    /// Controls, at a fixed size.
    Band(ControlBand),
    /// A display cell, which grows.
    Cell(DisplayCell),
    /// Parts left to right.
    Row(Vec<LayoutNode>),
    /// Parts top to bottom.
    Column(Vec<LayoutNode>),
}

impl LayoutNode {
    /// Its docked size.
    fn natural(&self) -> Size {
        match self {
            Self::Band(band) => Size::new(band.width, band.height),
            Self::Cell(cell) => Size::new(cell.width, cell.height),
            Self::Row(children) => children.iter().fold(Size::default(), |s, c| {
                let n = c.natural();
                Size::new(s.width + n.width, s.height.max(n.height))
            }),
            Self::Column(children) => children.iter().fold(Size::default(), |s, c| {
                let n = c.natural();
                Size::new(s.width.max(n.width), s.height + n.height)
            }),
        }
    }

    /// Its share of extra width.
    fn grows_across(&self) -> f32 {
        match self {
            Self::Band(_) => 0.0,
            Self::Cell(cell) => cell.weight,
            Self::Row(children) => children.iter().map(Self::grows_across).sum(),
            Self::Column(children) => children.iter().map(Self::grows_across).fold(0.0, f32::max),
        }
    }

    /// Its share of extra height.
    fn grows_down(&self) -> f32 {
        match self {
            Self::Band(_) => 0.0,
            Self::Cell(cell) => cell.weight,
            Self::Row(children) => children.iter().map(Self::grows_down).fold(0.0, f32::max),
            Self::Column(children) => children.iter().map(Self::grows_down).sum(),
        }
    }

    fn place(&self, at: Rect, placed: &mut Placed) {
        match self {
            Self::Band(band) => {
                // A band keeps its size, pinned to its slot's top-left; the
                // parts before and after it pin it to its edges.
                place_band(band, at.x, at.y, placed);
            }
            Self::Cell(cell) => {
                // A cell grows only where its weight earns it room.
                let width = if cell.weight > 0.0 {
                    at.width
                } else {
                    cell.width
                };
                let height = if cell.weight > 0.0 {
                    at.height
                } else {
                    cell.height
                };
                let rect = Rect::new(at.x, at.y, width.max(cell.width), height.max(cell.height));
                placed.cells.push(PlacedCell {
                    name: cell.name.clone(),
                    rect,
                });
                if let Some(band) = &cell.top {
                    place_band(band, rect.x, rect.y, placed);
                }
                if let Some(band) = &cell.bottom {
                    place_band(band, rect.x, rect.bottom() - band.height, placed);
                }
            }
            Self::Row(children) => {
                let natural = self.natural();
                let extra = (at.width - natural.width).max(0.0);
                let weight = self.grows_across();
                let mut x = at.x;
                for child in children {
                    let share = if weight > 0.0 {
                        extra * child.grows_across() / weight
                    } else {
                        0.0
                    };
                    let width = child.natural().width + share;
                    child.place(Rect::new(x, at.y, width, at.height), placed);
                    x += width;
                }
            }
            Self::Column(children) => {
                let natural = self.natural();
                let extra = (at.height - natural.height).max(0.0);
                let weight = self.grows_down();
                let mut y = at.y;
                for child in children {
                    let share = if weight > 0.0 {
                        extra * child.grows_down() / weight
                    } else {
                        0.0
                    };
                    let height = child.natural().height + share;
                    child.place(Rect::new(at.x, y, at.width, height), placed);
                    y += height;
                }
            }
        }
    }

    fn visit<'a>(&'a self, f: &mut impl FnMut(&'a LayoutNode)) {
        f(self);
        if let Self::Row(children) | Self::Column(children) = self {
            for child in children {
                child.visit(f);
            }
        }
    }
}

/// Places a band at its size with its top-left corner at `x`, `y`.
fn place_band(band: &ControlBand, x: f32, y: f32, placed: &mut Placed) {
    let rect = Rect::new(x, y, band.width, band.height);
    placed.bands.push(rect);
    placed
        .controls
        .extend(band.controls.iter().map(|control| PlacedControl {
            param: control.param,
            widget: control.widget,
            rect: Rect::new(
                rect.x + control.x,
                rect.y + control.y,
                control.widget.size().width,
                control.widget.size().height,
            ),
        }));
}

/// A device's layout (IMP-14): bands of controls and display cells in rows
/// and columns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceLayout {
    /// The whole body.
    pub root: LayoutNode,
    /// The part the device adds to its body while it is expanded (L-DEV-004,
    /// L-DEV-007), if it has one: placed to the right of [`root`](Self::root).
    /// It places its parameters' controls like the rest of the layout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expanded: Option<LayoutNode>,
}

impl Default for DeviceLayout {
    fn default() -> Self {
        Self::new(LayoutNode::Row(Vec::new()))
    }
}

/// A control where a layout placed it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlacedControl {
    /// The parameter.
    pub param: ParamId,
    /// How it's drawn.
    pub widget: Widget,
    /// Where, from the body's top-left corner.
    pub rect: Rect,
}

/// A display cell where a layout placed it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlacedCell {
    /// What it shows.
    pub name: String,
    /// Where, from the body's top-left corner.
    pub rect: Rect,
}

/// A layout placed at a size.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Placed {
    /// The size it was placed at: the size asked for, at least the docked
    /// size, and no larger along a side nothing grows along.
    pub size: Size,
    /// Its bands, in the layout's order.
    pub bands: Vec<Rect>,
    /// Its controls, band by band.
    pub controls: Vec<PlacedControl>,
    /// Its display cells, in the layout's order.
    pub cells: Vec<PlacedCell>,
}

impl DeviceLayout {
    /// A layout of `root`.
    #[must_use]
    pub fn new(root: LayoutNode) -> Self {
        Self {
            root,
            expanded: None,
        }
    }

    /// The same, with `part` as what the device adds while expanded.
    #[must_use]
    pub fn with_expanded(mut self, part: LayoutNode) -> Self {
        self.expanded = Some(part);
        self
    }

    /// Whether the device has an expanded part, so its title bar has the
    /// expand toggle.
    #[must_use]
    pub fn has_expanded(&self) -> bool {
        self.expanded.is_some()
    }

    /// The layout as drawn: the expanded part, if there is one, joined to
    /// the right of the rest while `expanded`, and left out otherwise. The
    /// result has no expanded part of its own, so [`docked_size`],
    /// [`place`] and the rest answer for what is shown.
    ///
    /// [`docked_size`]: Self::docked_size
    /// [`place`]: Self::place
    #[must_use]
    pub fn shown(&self, expanded: bool) -> Self {
        match (&self.expanded, expanded) {
            (Some(part), true) => Self::new(LayoutNode::Row(vec![self.root.clone(), part.clone()])),
            _ => Self::new(self.root.clone()),
        }
    }

    /// The generic UI's layout of `params`: a band [`BODY_HEIGHT`] tall
    /// holding each parameter's control ([`Widget::for_kind`]) top to
    /// bottom, in further columns as each fills. No display cell, so it
    /// doesn't grow.
    #[must_use]
    pub fn generic(params: &[ParamDescriptor]) -> Self {
        let controls: Vec<(ParamId, Widget)> = params
            .iter()
            .map(|p| (p.id, Widget::for_kind(&p.kind)))
            .collect();
        Self::new(LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &controls,
        )))
    }

    /// The docked size: every band and cell at its own size, and the
    /// smallest the device can be.
    #[must_use]
    pub fn docked_size(&self) -> Size {
        self.root.natural()
    }

    /// Whether it grows across and down: only if a display cell does.
    #[must_use]
    pub fn grows(&self) -> (bool, bool) {
        (self.root.grows_across() > 0.0, self.root.grows_down() > 0.0)
    }

    /// The size it takes when offered `size`: at least the docked size, and
    /// the docked size along a side nothing grows along.
    #[must_use]
    pub fn fit(&self, size: Size) -> Size {
        let docked = self.docked_size();
        let (across, down) = self.grows();
        let pick = |offered: f32, least: f32, grows: bool| {
            if grows && offered.is_finite() {
                offered.max(least)
            } else {
                least
            }
        };
        Size::new(
            pick(size.width, docked.width, across),
            pick(size.height, docked.height, down),
        )
    }

    /// Where every band, control, and display cell sits when the device is
    /// offered `size` ([`fit`](Self::fit)).
    #[must_use]
    pub fn place(&self, size: Size) -> Placed {
        let size = self.fit(size);
        let mut placed = Placed {
            size,
            ..Placed::default()
        };
        self.root
            .place(Rect::new(0.0, 0.0, size.width, size.height), &mut placed);
        placed
    }

    /// Why the layout doesn't fit `params`, if it doesn't: a control for a
    /// parameter that isn't there or is placed twice, a parameter with no
    /// control, a control outside its band, or a cell with a size or weight
    /// that isn't a finite number above 0.
    ///
    /// # Errors
    ///
    /// The first problem.
    pub fn check(&self, params: &[ParamDescriptor]) -> Result<(), String> {
        let mut seen: Vec<ParamId> = Vec::new();
        let mut problem = None;
        // Every control counts, in the part that is always there or in the
        // expanded one.
        let whole = self.shown(true);
        whole.root.visit(&mut |node| {
            if problem.is_some() {
                return;
            }
            match node {
                LayoutNode::Band(band) => check_band(band, params, &mut seen, &mut problem),
                LayoutNode::Cell(cell) => {
                    let fine = |n: f32| n.is_finite() && n > 0.0;
                    if !(fine(cell.width) && fine(cell.height) && fine(cell.weight)) {
                        problem = Some(format!(
                            "display cell {} needs a size and weight above 0",
                            cell.name
                        ));
                    }
                    let inside: f32 = [&cell.top, &cell.bottom]
                        .into_iter()
                        .flatten()
                        .map(|band| {
                            if band.width > cell.width {
                                problem.get_or_insert_with(|| {
                                    format!("a band is wider than display cell {}", cell.name)
                                });
                            }
                            check_band(band, params, &mut seen, &mut problem);
                            band.height
                        })
                        .sum();
                    if inside > cell.height {
                        problem.get_or_insert_with(|| {
                            format!("display cell {}'s bands don't fit inside it", cell.name)
                        });
                    }
                }
                LayoutNode::Row(_) | LayoutNode::Column(_) => {}
            }
        });
        if let Some(problem) = problem {
            return Err(problem);
        }
        if let Some(missing) = params.iter().find(|p| !seen.contains(&p.id)) {
            return Err(format!(
                "the layout has no control for parameter {} ({})",
                missing.id.0, missing.name
            ));
        }
        Ok(())
    }
}

/// Checks one band's controls: each for a parameter there is, placed once,
/// inside the band.
fn check_band(
    band: &ControlBand,
    params: &[ParamDescriptor],
    seen: &mut Vec<ParamId>,
    problem: &mut Option<String>,
) {
    let whole = Rect::new(0.0, 0.0, band.width, band.height);
    for control in &band.controls {
        if problem.is_some() {
            return;
        }
        let n = control.param.0;
        if !params.iter().any(|p| p.id == control.param) {
            *problem = Some(format!(
                "the layout has a control for parameter {n}, which the device doesn't have"
            ));
        } else if seen.contains(&control.param) {
            *problem = Some(format!("the layout places parameter {n} twice"));
        } else if !whole.contains(&control.rect()) {
            *problem = Some(format!("parameter {n}'s control lies outside its band"));
        }
        seen.push(control.param);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::param::ParamUnit;

    fn knobs(first: u32, count: u32) -> Vec<(ParamId, Widget)> {
        (first..first + count)
            .map(|n| (ParamId(n), Widget::Knob))
            .collect()
    }

    fn params(count: u32) -> Vec<ParamDescriptor> {
        (0..count)
            .map(|n| {
                ParamDescriptor::continuous(
                    ParamId(n),
                    &format!("P{n}"),
                    (0.0, 1.0),
                    0.5,
                    ParamUnit::None,
                )
            })
            .collect()
    }

    /// Controls on the left and right, a graph between them over a strip
    /// of controls, and a spectrum below that gets half the graph's share.
    fn layout() -> DeviceLayout {
        DeviceLayout::new(LayoutNode::Row(vec![
            LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, &knobs(0, 2))),
            LayoutNode::Column(vec![
                LayoutNode::Cell(DisplayCell::new("Graph", 120.0, 60.0, 2.0)),
                LayoutNode::Band(ControlBand::stacked(60.0, &knobs(2, 1))),
                LayoutNode::Cell(DisplayCell::new("Spectrum", 120.0, 38.0, 1.0)),
            ]),
            LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, &knobs(3, 2))),
        ]))
    }

    // Spec: device-chains R34
    #[test]
    fn an_expanded_part_joins_the_body_to_the_right_only_while_shown() {
        let layout = DeviceLayout::new(LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &knobs(0, 2),
        )))
        .with_expanded(LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &knobs(2, 2),
        )));
        // Every parameter is placed once, in the body or the expanded part.
        layout.check(&params(4)).unwrap();
        assert!(layout.check(&params(5)).is_err(), "a control is missing");
        assert!(layout.has_expanded());
        let (hidden, shown) = (layout.shown(false), layout.shown(true));
        assert!(!hidden.has_expanded() && !shown.has_expanded());
        assert_eq!(hidden.docked_size(), Size::new(56.0, BODY_HEIGHT));
        assert_eq!(shown.docked_size(), Size::new(112.0, BODY_HEIGHT));
        let placed = shown.place(shown.docked_size());
        let at = |param: u32| {
            placed
                .controls
                .iter()
                .find(|c| c.param == ParamId(param))
                .unwrap()
                .rect
        };
        assert_eq!(at(0), hidden.place(hidden.docked_size()).controls[0].rect);
        assert!(at(2).x >= 56.0, "the expanded part sits right of the body");
        // A parameter placed in both parts is refused, as in one band.
        let twice = DeviceLayout::new(LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &knobs(0, 2),
        )))
        .with_expanded(LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &knobs(1, 2),
        )));
        assert!(twice.check(&params(3)).is_err());
        // A layout with no expanded part shows as it is.
        let plain = DeviceLayout::generic(&params(2));
        assert_eq!(plain.shown(true), plain);
    }

    // Spec: device-framework R10
    #[test]
    fn the_docked_size_is_every_part_at_its_own_size() {
        let layout = layout();
        layout.check(&params(5)).unwrap();
        let docked = layout.docked_size();
        assert_eq!(docked, Size::new(56.0 + 120.0 + 56.0, BODY_HEIGHT));
        let placed = layout.place(Size::new(10.0, 10.0));
        assert_eq!(placed.size, docked, "never smaller than docked");
        assert_eq!(placed.cells[0].rect, Rect::new(56.0, 0.0, 120.0, 60.0));
    }

    // Spec: device-framework R11
    #[test]
    fn extra_room_goes_only_to_display_cells_and_controls_keep_their_size() {
        let layout = layout();
        let docked = layout.place(layout.docked_size());
        for (w, h) in [
            (300.0, 158.0),
            (232.0, 400.0),
            (900.0, 700.0),
            (1234.5, 321.0),
        ] {
            let grown = layout.place(Size::new(w, h));
            assert_eq!(grown.size, Size::new(w, h));
            assert_eq!(grown.controls.len(), docked.controls.len());
            for (before, after) in docked.controls.iter().zip(&grown.controls) {
                assert_eq!(
                    before.rect.size(),
                    after.rect.size(),
                    "controls keep their size"
                );
            }
            for (before, after) in docked.bands.iter().zip(&grown.bands) {
                assert_eq!(before.size(), after.size(), "bands keep their size");
            }
            // The left band stays pinned left, the right one right.
            assert_eq!(grown.bands[0], docked.bands[0]);
            assert_eq!(grown.bands[2].right(), w);
            assert_eq!(grown.bands[2].y, 0.0);
            // The middle strip of controls stays between the cells.
            assert_eq!(grown.bands[1].y, grown.cells[0].rect.bottom());
            assert_eq!(grown.cells[1].rect.y, grown.bands[1].bottom());
            // The cells take all the extra, shared by weight: 2 to 1 down.
            let extra_w = w - docked.size.width;
            let extra_h = h - docked.size.height;
            for (before, after) in docked.cells.iter().zip(&grown.cells) {
                assert!((after.rect.width - before.rect.width - extra_w).abs() < 1e-3);
            }
            let graph = grown.cells[0].rect.height - docked.cells[0].rect.height;
            let spectrum = grown.cells[1].rect.height - docked.cells[1].rect.height;
            assert!((graph - extra_h * 2.0 / 3.0).abs() < 1e-3, "{graph}");
            assert!((spectrum - extra_h / 3.0).abs() < 1e-3, "{spectrum}");
        }
    }

    // Spec: device-framework R11
    #[test]
    fn rows_of_controls_inside_a_cell_stay_on_its_edges() {
        let cell = DisplayCell::new("Display", 160.0, 158.0, 1.0)
            .with_top(ControlBand::stacked(40.0, &[(ParamId(0), Widget::Toggle)]))
            .with_bottom(ControlBand::stacked(40.0, &[(ParamId(1), Widget::Number)]));
        let layout = DeviceLayout::new(LayoutNode::Cell(cell));
        layout.check(&params(2)).unwrap();
        let docked = layout.place(layout.docked_size());
        let grown = layout.place(Size::new(500.0, 400.0));
        assert_eq!(grown.cells[0].rect, Rect::new(0.0, 0.0, 500.0, 400.0));
        assert_eq!(
            grown.bands[0], docked.bands[0],
            "the top row stays at the top"
        );
        assert_eq!(
            grown.bands[1].bottom(),
            400.0,
            "the bottom row stays at the bottom"
        );
        assert_eq!(grown.bands[1].size(), docked.bands[1].size());
        assert_eq!(
            grown.controls[1].rect.size(),
            docked.controls[1].rect.size()
        );
        let crowded = DisplayCell::new("Display", 160.0, 50.0, 1.0)
            .with_top(ControlBand::stacked(40.0, &[(ParamId(0), Widget::Toggle)]))
            .with_bottom(ControlBand::stacked(40.0, &[(ParamId(1), Widget::Number)]));
        assert!(
            DeviceLayout::new(LayoutNode::Cell(crowded))
                .check(&params(2))
                .is_err()
        );
    }

    // Spec: device-framework R12
    #[test]
    fn a_device_without_display_cells_stays_at_its_docked_size() {
        let p = params(7);
        let layout = DeviceLayout::generic(&p);
        layout.check(&p).unwrap();
        assert_eq!(layout.grows(), (false, false));
        let docked = layout.docked_size();
        assert_eq!(docked.height, BODY_HEIGHT);
        assert_eq!(layout.place(Size::new(2000.0, 900.0)).size, docked);
        // Two knobs fill a column of 158 points; seven make four columns.
        assert_eq!(docked.width, PADDING + 4.0 * (48.0 + PADDING));
    }

    // Spec: device-framework R10
    #[test]
    fn a_layout_must_place_every_parameter_once() {
        let p = params(3);
        let missing = DeviceLayout::new(LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &knobs(0, 2),
        )));
        assert!(missing.check(&p).unwrap_err().contains("no control"));
        let twice = DeviceLayout::new(LayoutNode::Row(vec![
            LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, &knobs(0, 3))),
            LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, &knobs(2, 1))),
        ]));
        assert!(twice.check(&p).unwrap_err().contains("twice"));
        let unknown = DeviceLayout::new(LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &knobs(0, 4),
        )));
        assert!(unknown.check(&p).is_err());
    }
}

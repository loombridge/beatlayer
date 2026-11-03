//! What the MIDI effects' layouts share: the empty bands that space a display
//! cell from the body's edge, as Live's body padding does (layout L-DEV-044).

use bl_model::device_layout::{ControlBand, DisplayCell, LayoutNode, PADDING};

/// An empty band of `width` by `height` points.
pub(crate) fn spacer(width: f32, height: f32) -> LayoutNode {
    LayoutNode::Band(ControlBand::free(width, height, Vec::new()))
}

/// A display cell with [`PADDING`] of room at its left, for a column that
/// puts a [`spacer`] above it.
pub(crate) fn inset(cell: DisplayCell) -> LayoutNode {
    LayoutNode::Row(vec![spacer(PADDING, 0.0), LayoutNode::Cell(cell)])
}

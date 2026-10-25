//! The widths of the mixer's and the Session View's columns (the mixer view
//! spec, R36): one entry for each track, return track or Main whose column
//! was resized, in points at 100%.
//!
//! Saved with the set beside the document, so resizing a column is no undo
//! step. A track with no entry has the default width, which is what a set
//! saved before columns could be resized opens with.

use std::collections::BTreeMap;

use crate::ids::TrackId;

/// The narrowest a column goes, in points.
pub const MIN_COLUMN_WIDTH: f32 = 40.0;

/// The widest a column goes, in points.
pub const MAX_COLUMN_WIDTH: f32 = 400.0;

/// The columns' widths, as a set saves them. A column at its default width
/// has no entry.
pub type ColumnWidths = BTreeMap<TrackId, f32>;

/// `width` kept within [`MIN_COLUMN_WIDTH`] and [`MAX_COLUMN_WIDTH`]; a width
/// that isn't a number is the narrowest.
#[must_use]
pub fn clamp_column_width(width: f32) -> f32 {
    if width.is_nan() {
        MIN_COLUMN_WIDTH
    } else {
        width.clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH)
    }
}

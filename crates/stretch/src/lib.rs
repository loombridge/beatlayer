//! Warping: warp maps and every warp mode.

pub mod beats;
mod interp;
pub mod map;
mod mode;
mod table;

pub use beats::{Loop, Plan, Preserve, Segment, Stretch};
pub use interp::Interpolation;
pub use map::{MAX_BPM, MIN_BPM, Marker, WarpMap, ticks_in};
pub use mode::{Beats, RePitch, Request, WarpMode, Window};
pub use table::{Piece, WarpTable};

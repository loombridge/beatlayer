//! Warping: warp maps and every warp mode.

mod interp;
pub mod map;
mod mode;
mod table;

pub use interp::Interpolation;
pub use map::{MAX_BPM, MIN_BPM, Marker, WarpMap, ticks_in};
pub use mode::{RePitch, Request, WarpMode, Window};
pub use table::{Piece, WarpTable};

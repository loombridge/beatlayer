//! Warping: warp maps and every warp mode.

pub mod beats;
pub mod granular;
mod interp;
pub mod map;
mod mode;
mod table;
pub mod texture;
pub mod tones;

pub use beats::{Loop, Plan, Preserve, Segment, Stretch};
pub use interp::Interpolation;
pub use map::{MAX_BPM, MIN_BPM, Marker, WarpMap, ticks_in};
pub use mode::{Beats, RePitch, Request, WarpMode, Window, window_frames};
pub use table::{Piece, WarpTable};
pub use texture::Texture;
pub use tones::Tones;

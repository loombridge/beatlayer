//! Musical time: ticks, frames, tempo, time signatures, and the
//! bar-beat-sixteenth display.
//!
//! The document places everything in [`Ticks`], integers at
//! [`TICKS_PER_QUARTER`] to the quarter note, so edits and comparisons are
//! exact. Devices count [`Frames`] on their sample clock. The [`TempoMap`]
//! converts between the two through `f64` seconds, which appear only inside
//! conversions. The [`TimeSignatureMap`] groups ticks into bars and beats for
//! display as a [`BarBeatSixteenth`].
//!
//! Tick zero, second zero, and frame zero are the same instant: the start of
//! the arrangement, shown as `1.1.1`.
//!
//! ```
//! use bl_core::time::{Bpm, SampleRate, TempoMap, Ticks, TimeSignatureMap};
//!
//! let tempo = TempoMap::constant(Bpm::new(120.0)?);
//! let bars = TimeSignatureMap::default(); // 4/4 throughout
//!
//! let at = bars.parse_position("3.2.1")?;
//! assert_eq!(at, Ticks::from_quarters(9));
//! assert_eq!(bars.to_bbs(at).to_string(), "3.2.1");
//! assert_eq!(tempo.ticks_to_seconds(at), 4.5);
//! assert_eq!(tempo.ticks_to_frames(at, SampleRate::HZ_48000).get(), 216_000);
//! # Ok::<(), bl_core::time::TimeError>(())
//! ```
//!
//! The rules taken from Live are in the musical time spec.

mod bbs;
mod error;
mod signature;
mod tempo;
mod units;

pub use bbs::BarBeatSixteenth;
pub use error::TimeError;
pub use signature::{FragmentaryBar, TimeSignature, TimeSignatureChange, TimeSignatureMap};
pub use tempo::{Bpm, TempoMap, TempoSegment};
pub use units::{Frames, SampleRate, TICKS_PER_QUARTER, Ticks};

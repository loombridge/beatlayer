//! Time types, tempo map, IDs, units, and shared errors.
//!
//! So far this holds musical time, in [`time`], stable object IDs, in
//! [`id`], and the MIDI messages the device layer hands the engine, in
//! [`midi`]. With the `schemars` feature, the types that commands carry
//! have JSON Schemas for the command registry.

pub mod clock;
pub mod id;
pub mod midi;
pub mod scale;
#[cfg(feature = "schemars")]
mod schema;
pub mod time;
pub mod timecode;

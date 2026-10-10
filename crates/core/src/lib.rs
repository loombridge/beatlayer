//! Time types, tempo map, IDs, units, and shared errors.
//!
//! So far this holds musical time, in [`time`], and stable object IDs, in
//! [`id`]. With the `schemars` feature, the types that commands carry have
//! JSON Schemas for the command registry.

pub mod id;
#[cfg(feature = "schemars")]
mod schema;
pub mod time;

//! The signal graph a document's routing makes (the routing spec). It lives
//! in the model, where the modulator mapping commands also use it (the
//! modulation spec, R28); the engine's paths are unchanged.

pub use bl_model::graph::*;

//! Native instruments, audio effects, and MIDI effects, and the framework
//! they're built on (WP-2.5; the device framework spec).
//!
//! A native device is a [`NativeDevice`]: it processes audio in place, and
//! reads its parameters, already clamped to their ranges and smoothed frame
//! by frame, from a [`ParamBank`]. Its type is a
//! [`DeviceType`](bl_engine::DeviceType) with a
//! [`DeviceDescription`](bl_model::DeviceDescription): every parameter's
//! descriptor (range, unit, how it reads, default, automatable) and the
//! device's layout of control bands and display cells (IMP-14). [`add`]
//! registers one with the engine, wrapped in a [`NativeNode`] that splits
//! each block at its parameter changes so each lands on its exact frame.
//! The generic UI (`bl-ui`) draws any device from its description; an
//! [`Oversampler`](bl_dsp::Oversampler) runs a nonlinear stage at a
//! multiple of the rate.
//!
//! Plug-ins built on the plug-in kit run as native devices too, compiled
//! in: [`kit`] adapts them, and a [`KitHost`] on the main thread keeps
//! their instances and runs their commands.
//!
//! Beatlayer ships its first native devices: the limiter ([`limiter`]),
//! the tempo-synced delay ([`delay`]), the algorithmic reverb ([`reverb`]),
//! the Utility device ([`utility`], WP-2.18b) and the parametric EQ
//! ([`eq`], WP-2.18a); [`register`] adds them to the engine. The polysynth
//! ([`polysynth`], WP-2.17) is the first instrument. The rest of the
//! instruments and core effects ship with WP-2.16 to 2.18. [`testing`] has
//! the devices the framework's tests use. The kit's two example plug-ins
//! join the device list with the `kit-examples` feature, off by default.

pub mod delay;
pub mod eq;
mod framework;
pub mod kit;
pub mod limiter;
pub mod polysynth;
pub mod reverb;
pub mod testing;
pub mod utility;

pub use delay::{DELAY, delay_type};
pub use eq::{Contour, EQ, eq_type};
pub use framework::{Audio, NativeDevice, NativeNode, ParamBank, add};
pub use kit::{KitFactory, KitHost};
pub use limiter::{Brickwall, limiter_type};
pub use polysynth::{POLYSYNTH, PolySynth, polysynth_type};
pub use reverb::{REVERB, reverb_type};

use bl_engine::{DeviceType, Registry};

/// Adds Beatlayer's native devices to `registry`: the polysynth, the
/// limiter, the delay, the reverb, Utility and the EQ.
pub fn register(registry: &mut Registry) {
    polysynth::register(registry);
    limiter::register(registry);
    delay::register(registry);
    reverb::register(registry);
    utility::register(registry);
    eq::register(registry);
}

/// The native device types Beatlayer ships, for the browser and the shell:
/// the polysynth, the limiter, the delay, the reverb, Utility and the EQ.
#[must_use]
pub fn device_types() -> Vec<DeviceType> {
    let mut registry = Registry::empty();
    register(&mut registry);
    registry.device_types()
}

/// A [`KitHost`] with the kit plug-ins compiled into Beatlayer: its
/// examples with the `kit-examples` feature, none otherwise.
#[must_use]
pub fn kit_host() -> KitHost {
    let host = KitHost::new();
    kit::add_examples(&host);
    host
}

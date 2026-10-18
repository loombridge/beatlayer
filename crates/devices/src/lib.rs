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
//! Beatlayer ships its first native devices: the compressor ([`compressor`]),
//! the parametric EQ ([`eq`], WP-2.18a), the algorithmic reverb
//! ([`reverb`]), the limiter ([`limiter`]), the tempo-synced delay
//! ([`delay`]), the Utility device ([`utility`], WP-2.18b) and the multiband
//! dynamics ([`multiband_dynamics`], WP-5.22); [`register`]
//! adds them to the engine, with the three delays of WP-5.25
//! ([`filter_delay`], [`grain_delay`], [`alignment_delay`]). The instruments
//! are the polysynth
//! ([`polysynth`], WP-2.17), the sampler ([`sampler`], WP-2.16), the
//! multisampler ([`multisampler`], WP-5.13) and the eight-slot drum sampler
//! ([`kitbag`], WP-5.17). The
//! rest of the core effects ship with WP-2.18; the lo-fi effects are in
//! [`degrade`], [`crusher`] and [`wax`] (WP-5.24). [`testing`] has the devices
//! the framework's tests use. The kit's two example plug-ins join the
//! device list with the `kit-examples` feature, off by default.

pub mod alignment_delay;
pub mod auto_filter;
pub mod beat_repeat;
pub mod bus_compressor;
pub mod channel_eq;
pub mod compressor;
pub mod crusher;
pub mod degrade;
pub mod delay;
pub mod dice;
pub mod drum_sampler;
mod dynamics;
pub mod eq;
pub mod filter_delay;
mod framework;
pub mod gate;
pub mod grain_delay;
pub mod hardware;
pub mod hold;
pub mod kit;
pub mod kitbag;
pub mod lfo;
pub mod lift;
pub mod limiter;
mod midi_fx;
pub mod modulator;
pub mod multiband_dynamics;
pub mod multisampler;
pub mod overdrive;
pub mod polysynth;
pub mod remap;
pub mod reverb;
pub mod sampler;
pub mod saturator;
mod sections;
pub mod stack;
pub mod testing;
pub mod three_band_eq;
pub mod touch;
pub mod tube;
pub mod utility;
pub mod wax;

pub use alignment_delay::{ALIGNMENT_DELAY, alignment_delay_type};
pub use auto_filter::{AUTO_FILTER, AutoFilter, auto_filter_type};
pub use beat_repeat::{BEAT_REPEAT, beat_repeat_type};
pub use bus_compressor::{BUS_COMPRESSOR, Bond, bus_compressor_type};
pub use channel_eq::{CHANNEL_EQ, Strip, channel_eq_type};
pub use compressor::{COMPRESSOR, Squeeze, compressor_type};
pub use crusher::{CRUSHER, crusher_type};
pub use degrade::{DEGRADE, degrade_type};
pub use delay::{DELAY, delay_type};
pub use dice::{DICE, dice_type};
pub use drum_sampler::{DRUM_SAMPLER, DrumSampler, drum_sampler_type};
pub use eq::{Contour, EQ, eq_type};
pub use filter_delay::{FILTER_DELAY, filter_delay_type};
pub use framework::{Audio, Modulation, NativeDevice, NativeNode, ParamBank, add};
pub use gate::{GATE, Shutter, gate_type};
pub use grain_delay::{GRAIN_DELAY, grain_delay_type};
pub use hardware::{HARDWARE_INSTRUMENT, hardware_instrument_type};
pub use hold::{HOLD, hold_type};
pub use kit::{KitFactory, KitHost};
pub use kitbag::{KITBAG, Kitbag, kitbag_type};
pub use lfo::{Lfo, Rate, Shape, Stereo, Wave};
pub use lift::{LIFT, lift_type};
pub use limiter::{Brickwall, limiter_type};
pub use multiband_dynamics::{MULTIBAND_DYNAMICS, Triband, multiband_dynamics_type};
pub use multisampler::{MULTISAMPLER, Multisampler, multisampler_type};
pub use overdrive::{OVERDRIVE, overdrive_type};
pub use polysynth::{POLYSYNTH, PolySynth, polysynth_type};
pub use remap::{REMAP, remap_type};
pub use reverb::{REVERB, reverb_type};
pub use sampler::{Sampler, sampler_type};
pub use saturator::{SATURATOR, saturator_type};
pub use stack::{STACK, stack_type};
pub use three_band_eq::{THREE_BAND_EQ, Triad, three_band_eq_type};
pub use touch::{TOUCH, touch_type};
pub use tube::{TUBE, tube_type};
pub use wax::{WAX, wax_type};

use bl_engine::{DeviceType, Registry};
use bl_model::{DeviceDescription, DeviceDisplay, DeviceId, OrdMap, ParamId};

/// The display of the native device `device_type`, if its type draws one
/// from its parameters and the data its node publishes (the device
/// framework spec, R36): `values` are the device's set values and `data` the
/// newest data its node published, if an engine runs.
#[must_use]
pub fn display(
    device_type: &str,
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> Option<DeviceDisplay> {
    match device_type {
        EQ => Some(eq::display(device, description, values, data)),
        COMPRESSOR => Some(compressor::display(device, description, values, data)),
        CHANNEL_EQ => Some(channel_eq::display(device, description, values, data)),
        BUS_COMPRESSOR => Some(bus_compressor::display(device, description, values, data)),
        GATE => Some(gate::display(device, description, values, data)),
        THREE_BAND_EQ => Some(three_band_eq::display(device, description, values, data)),
        DEGRADE => Some(degrade::display(device, data)),
        AUTO_FILTER => Some(auto_filter::display(device, description, values, data)),
        SATURATOR => Some(saturator::display(device, description, values, data)),
        OVERDRIVE => Some(overdrive::display(device, description, values, data)),
        TUBE => Some(tube::display(device, description, values, data)),
        bl_model::modulator::WAVE_MOD => Some(modulator::wave_display(device, description, values)),
        MULTIBAND_DYNAMICS => Some(multiband_dynamics::display(
            device,
            description,
            values,
            data,
        )),
        _ => None,
    }
}

/// Adds Beatlayer's native devices to `registry`: the compressor, the
/// polysynth, the limiter, the delay, the reverb, Utility, the EQ, the
/// sampler and the eight-slot drum sampler.
pub fn register(registry: &mut Registry) {
    compressor::register(registry);
    polysynth::register(registry);
    limiter::register(registry);
    delay::register(registry);
    filter_delay::register(registry);
    grain_delay::register(registry);
    alignment_delay::register(registry);
    reverb::register(registry);
    utility::register(registry);
    eq::register(registry);
    channel_eq::register(registry);
    three_band_eq::register(registry);
    gate::register(registry);
    bus_compressor::register(registry);
    sampler::register(registry);
    multisampler::register(registry);
    drum_sampler::register(registry);
    kitbag::register(registry);
    modulator::register(registry);
    lift::register(registry);
    touch::register(registry);
    dice::register(registry);
    remap::register(registry);
    stack::register(registry);
    hold::register(registry);
    hardware::register(registry);
    degrade::register(registry);
    crusher::register(registry);
    wax::register(registry);
    auto_filter::register(registry);
    saturator::register(registry);
    overdrive::register(registry);
    tube::register(registry);
    beat_repeat::register(registry);
    multiband_dynamics::register(registry);
}

/// The native device types Beatlayer ships, for the browser and the shell:
/// the compressor, the polysynth, the limiter, the delay, the reverb,
/// Utility, the EQ, the sampler and the eight-slot drum sampler.
#[must_use]
pub fn device_types() -> Vec<DeviceType> {
    let mut registry = Registry::empty();
    register(&mut registry);
    registry.device_types()
}

/// Turns the clip envelopes of a set saved before relative modulation into
/// modulation that plays what they played (the modulation spec, R5), with
/// Beatlayer's native devices' descriptions. A plug-in's parameters have
/// none here: its envelopes played nothing before, and keep their values,
/// clamped.
pub fn relative_clip_envelopes(document: &mut bl_model::Document) {
    let types = device_types();
    document.relative_clip_envelopes(&|device, param| {
        let kind = types.iter().find(|t| t.kind == device.kind)?;
        kind.description.param(param).cloned()
    });
}

/// Whether the native device type `device_type` plays samples that
/// `device.load_sample` loads into it: the sampler and the two drum
/// samplers.
#[must_use]
pub fn plays_samples(device_type: &str) -> bool {
    matches!(
        device_type,
        sampler::SAMPLER | kitbag::KITBAG | DRUM_SAMPLER
    )
}

/// A [`KitHost`] with the kit plug-ins compiled into Beatlayer: its
/// examples with the `kit-examples` feature, none otherwise.
#[must_use]
pub fn kit_host() -> KitHost {
    let host = KitHost::new();
    kit::add_examples(&host);
    host
}

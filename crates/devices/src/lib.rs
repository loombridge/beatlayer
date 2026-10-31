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
//! Each native device lives in its own module, and [`register`] adds them
//! all to the engine. [`testing`] has the devices the framework's tests
//! use. The kit's two example plug-ins join the device list with the
//! `kit-examples` feature, off by default.

pub mod alignment_delay;
pub mod arpeggiator;
pub mod articulation_map;
pub mod auto_filter;
pub mod auto_pan_tremolo;
pub mod beat_repeat;
pub mod bus_compressor;
pub mod channel_eq;
pub mod chorus_ensemble;
pub mod compressor;
pub mod crusher;
mod cv;
pub mod cv_clock_in;
pub mod cv_clock_out;
pub mod cv_in;
pub mod cv_instrument;
pub mod cv_modulators;
pub mod cv_triggers;
pub mod cv_utility;
pub mod degrade;
pub mod delay;
pub mod dice;
pub mod drum_bus;
pub mod drum_sampler;
mod dynamics;
pub mod eq;
pub mod filter_delay;
mod filters;
mod framework;
pub mod gate;
pub mod gate_mod;
pub mod grain_delay;
pub mod hardware;
pub mod hardware_effect;
pub mod hold;
pub mod kit;
pub mod kitbag;
pub mod level_mod;
pub mod lfo;
pub mod lift;
pub mod limiter;
pub mod looper;
pub mod midi_cc;
mod midi_fx;
pub mod midi_monitor;
pub mod midi_mpe;
pub mod modulator;
pub mod multiband_dynamics;
pub mod multisampler;
mod multisampler_tabs;
pub mod note_echo;
pub mod oscilloscope;
pub mod overdrive;
pub mod phaser_flanger;
pub mod pitchloop89;
pub mod play_mod;
pub mod polysynth;
pub mod remap;
pub mod reverb;
mod ring;
pub mod roar;
pub mod sampler;
pub mod saturator;
mod sections;
pub mod shifter;
mod signal_mod;
pub mod spectrum_analyser;
pub mod stack;
pub mod strike_mod;
pub mod testing;
pub mod three_band_eq;
pub mod touch;
pub mod tube;
pub mod tuner;
pub mod utility;
pub mod vector_delay;
pub mod wax;

pub use alignment_delay::{ALIGNMENT_DELAY, alignment_delay_type};
pub use arpeggiator::{ARPEGGIATOR, arpeggiator_type};
pub use auto_filter::{AUTO_FILTER, AutoFilter, auto_filter_type};
pub use auto_pan_tremolo::{AUTO_PAN_TREMOLO, auto_pan_tremolo_type};
pub use beat_repeat::{BEAT_REPEAT, beat_repeat_type};
pub use bus_compressor::{BUS_COMPRESSOR, Bond, bus_compressor_type};
pub use channel_eq::{CHANNEL_EQ, Strip, channel_eq_type};
pub use chorus_ensemble::{CHORUS_ENSEMBLE, chorus_ensemble_type};
pub use compressor::{COMPRESSOR, Squeeze, compressor_type};
pub use crusher::{CRUSHER, crusher_type};
pub use cv_clock_in::{CV_CLOCK_IN, cv_clock_in_type};
pub use cv_clock_out::{CV_CLOCK_OUT, cv_clock_out_type};
pub use cv_in::cv_in_type;
pub use cv_instrument::{CV_INSTRUMENT, CvCommand, cv_instrument_type};
pub use cv_modulators::{cv_curve_type, cv_level_type, cv_wave_type};
pub use cv_triggers::{CV_TRIGGERS, cv_triggers_type};
pub use cv_utility::{CV_UTILITY, cv_utility_type};
pub use degrade::{DEGRADE, degrade_type};
pub use delay::{DELAY, delay_type};
pub use dice::{DICE, dice_type};
pub use drum_bus::{DRUM_BUSS, drum_buss_type};
pub use drum_sampler::{DRUM_SAMPLER, DrumSampler, drum_sampler_type};
pub use eq::{Contour, EQ, eq_type};
pub use filter_delay::{FILTER_DELAY, filter_delay_type};
pub use framework::{Audio, Modulation, NativeDevice, NativeNode, ParamBank, add};
pub use gate::{GATE, Shutter, gate_type};
pub use grain_delay::{GRAIN_DELAY, grain_delay_type};
pub use hardware::{HARDWARE_INSTRUMENT, hardware_instrument_type};
pub use hardware_effect::{HARDWARE_EFFECT, hardware_effect_type};
pub use hold::{HOLD, hold_type};
pub use kit::{KitFactory, KitHost};
pub use kitbag::{KITBAG, Kitbag, kitbag_type};
pub use lfo::{Lfo, Rate, Shape, Stereo, Wave};
pub use lift::{LIFT, lift_type};
pub use limiter::{Brickwall, limiter_type};
pub use looper::{LOOPER, looper_type};
pub use midi_cc::{CC_CONTROL, cc_control_type};
pub use midi_monitor::{MIDI_MONITOR, midi_monitor_type};
pub use midi_mpe::{MPE_CONTROL, mpe_control_type};
pub use multiband_dynamics::{MULTIBAND_DYNAMICS, Triband, multiband_dynamics_type};
pub use multisampler::{MULTISAMPLER, Multisampler, multisampler_type};
pub use note_echo::{NOTE_ECHO, note_echo_type};
pub use oscilloscope::{OSCILLOSCOPE, Oscilloscope, oscilloscope_type};
pub use overdrive::{OVERDRIVE, overdrive_type};
pub use phaser_flanger::{PHASER_FLANGER, phaser_flanger_type};
pub use pitchloop89::{PITCHLOOP89, pitchloop89_type};
pub use polysynth::{POLYSYNTH, PolySynth, polysynth_type};
pub use remap::{REMAP, remap_type};
pub use reverb::{REVERB, reverb_type};
pub use roar::{ROAR, roar_type};
pub use sampler::{Sampler, sampler_type, slice_boundaries};
pub use saturator::{SATURATOR, saturator_type};
pub use shifter::{SHIFTER, shifter_type};
pub use spectrum_analyser::{SPECTRUM, Spectrum, spectrum_type};
pub use stack::{STACK, stack_type};
pub use three_band_eq::{THREE_BAND_EQ, Triad, three_band_eq_type};
pub use touch::{TOUCH, touch_type};
pub use tube::{TUBE, tube_type};
pub use tuner::{TUNER, Tuner, tuner_type};
pub use vector_delay::{VECTOR_DELAY, vector_delay_type};
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
    display_for_view(device_type, device, description, values, data, None)
}

/// What `device_type`'s display shows for a specific view: `values` are the device's
/// set values, `data` the newest data its node published, and `view` the active display view.
#[must_use]
pub fn display_for_view(
    device_type: &str,
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
    view: Option<&str>,
) -> Option<DeviceDisplay> {
    display_in(device_type, device, description, values, data, view, None)
}

/// As [`display_for_view`], for a device that follows the set's `tuning`
/// (the tunings spec, R33): a Tuner names its notes by it.
#[must_use]
pub fn display_in(
    device_type: &str,
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
    view: Option<&str>,
    tuning: Option<&bl_model::Tuning>,
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
        AUTO_PAN_TREMOLO => Some(auto_pan_tremolo::display(device, description, values, data)),
        DRUM_BUSS => Some(drum_bus::display(device, description, values, data)),
        SATURATOR => Some(saturator::display(device, description, values, data)),
        OVERDRIVE => Some(overdrive::display(device, description, values, data)),
        TUBE => Some(tube::display(device, description, values, data)),
        ROAR => Some(roar::display(device, description, values, data, view)),
        bl_model::modulator::WAVE_MOD => Some(modulator::wave_display(device, description, values)),
        bl_model::modulator::PLAY_MOD => Some(play_mod::display(device, description, values, data)),
        bl_model::modulator::STRIKE_MOD => {
            Some(strike_mod::display(device, description, values, data))
        }
        bl_model::modulator::GATE_MOD => Some(gate_mod::display(device, description, values, data)),
        bl_model::modulator::LEVEL_MOD => {
            Some(level_mod::display(device, description, values, data))
        }
        MULTIBAND_DYNAMICS => Some(multiband_dynamics::display(
            device,
            description,
            values,
            data,
        )),
        PHASER_FLANGER => Some(phaser_flanger::display(device, description, values, data)),
        CHORUS_ENSEMBLE => Some(chorus_ensemble::display(device, description, values, data)),
        MIDI_MONITOR => Some(midi_monitor::display(device, values, data)),
        SPECTRUM => Some(spectrum_analyser::display(
            device,
            description,
            values,
            data,
        )),
        TUNER => Some(tuner::display_in(
            device,
            description,
            values,
            data,
            view,
            tuning,
        )),
        OSCILLOSCOPE => Some(oscilloscope::display(device, description, values, data)),
        LOOPER => Some(looper::display(device, description, values, data)),
        VECTOR_DELAY => Some(vector_delay::display(device, description, values, data)),
        CV_INSTRUMENT => Some(cv_instrument::display(device, data)),
        CV_TRIGGERS => Some(cv_triggers::display(device, data)),
        CV_CLOCK_OUT => Some(cv_clock_out::display(device, data)),
        CV_CLOCK_IN => Some(cv_clock_in::display(device, data)),
        CV_UTILITY => Some(cv_utility::display(device, data)),
        bl_model::modulator::CV_IN => Some(cv_in::display(device, data)),
        bl_model::modulator::CV_WAVE => Some(cv_modulators::wave_display(
            device,
            description,
            values,
            data,
        )),
        bl_model::modulator::CV_CURVE => Some(cv_modulators::curve_display(device, data)),
        bl_model::modulator::CV_LEVEL => Some(cv_modulators::level_display(
            device,
            description,
            values,
            data,
        )),
        _ => None,
    }
}

/// Adds every Beatlayer native device to `registry`.
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
    level_mod::register(registry);
    gate_mod::register(registry);
    strike_mod::register(registry);
    play_mod::register(registry);
    lift::register(registry);
    touch::register(registry);
    dice::register(registry);
    remap::register(registry);
    stack::register(registry);
    hold::register(registry);
    arpeggiator::register(registry);
    note_echo::register(registry);
    midi_cc::register(registry);
    midi_mpe::register(registry);
    midi_monitor::register(registry);
    articulation_map::register(registry);
    hardware::register(registry);
    hardware_effect::register(registry);
    cv_instrument::register(registry);
    cv_triggers::register(registry);
    cv_clock_out::register(registry);
    cv_clock_in::register(registry);
    cv_utility::register(registry);
    cv_in::register(registry);
    cv_modulators::register(registry);
    degrade::register(registry);
    crusher::register(registry);
    wax::register(registry);
    auto_filter::register(registry);
    auto_pan_tremolo::register(registry);
    saturator::register(registry);
    drum_bus::register(registry);
    overdrive::register(registry);
    tube::register(registry);
    roar::register(registry);
    beat_repeat::register(registry);
    multiband_dynamics::register(registry);
    shifter::register(registry);
    phaser_flanger::register(registry);
    chorus_ensemble::register(registry);
    spectrum_analyser::register(registry);
    tuner::register(registry);
    oscilloscope::register(registry);
    looper::register(registry);
    pitchloop89::register(registry);
    vector_delay::register(registry);
}

/// The native device types Beatlayer ships, for the browser and the shell.
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

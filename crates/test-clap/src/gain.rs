//! CLAP's gain-adjustment metering extension
//! (`clap.gain-adjustment-metering/0`, a draft extension) for the test
//! plug-in: it reports the gain adjustment, in dB, it applied to the last
//! sample of the block it just processed, so a test can see a reduction on
//! the track's meter (the mixer view spec, R21).

use std::ffi::CStr;

use clack_plugin::extensions::prelude::*;

/// The extension's ABI, as CLAP declares it: `get` is called on the audio
/// thread and returns the adjustment in dB.
#[derive(Clone, Copy)]
#[repr(C)]
struct Raw {
    get: Option<unsafe extern "C" fn(*const clap_plugin) -> f64>,
}

/// The plug-in's side of the extension. The plug-in only provides the ABI,
/// so it holds no state of its own.
#[derive(Copy, Clone)]
pub struct PluginGainAdjustmentMetering;

// SAFETY: `Raw` is repr(C) and ABI-compatible with the CLAP extension
// struct, and the identifier is the extension's own.
unsafe impl Extension for PluginGainAdjustmentMetering {
    const IDENTIFIERS: &[&CStr] = &[c"clap.gain-adjustment-metering/0"];
    type ExtensionSide = PluginExtensionSide;

    unsafe fn from_raw(_raw: RawExtension<PluginExtensionSide>) -> Self {
        Self
    }
}

/// The plug-in side of the extension: what it reports.
pub trait PluginGainAdjustmentMeteringImpl {
    /// The gain adjustment in dB the plug-in applied to the last sample of
    /// the block it last processed: zero when it reduces nothing.
    fn gain_adjustment(&self) -> f64;
}

// SAFETY: the struct is repr(C) and ABI-compatible with the CLAP extension,
// and the function it points at matches the extension's signature.
unsafe impl<P> ExtensionImplementation<P> for PluginGainAdjustmentMetering
where
    for<'a> P: Plugin<MainThread<'a>: PluginGainAdjustmentMeteringImpl>,
{
    const IMPLEMENTATION: RawExtensionImplementation = RawExtensionImplementation::new(&Raw {
        get: Some(get::<P>),
    });
}

#[allow(clippy::missing_safety_doc)]
unsafe extern "C" fn get<P>(plugin: *const clap_plugin) -> f64
where
    for<'a> P: Plugin<MainThread<'a>: PluginGainAdjustmentMeteringImpl>,
{
    // SAFETY: the host passes the plug-in instance this extension was
    // registered with, and the wrapper holds it for the call.
    unsafe {
        PluginWrapper::<P>::handle(plugin, |plugin| Ok(plugin.main_thread().gain_adjustment()))
    }
    .unwrap_or(0.0)
}

//! CLAP's gain-adjustment metering extension
//! (`clap.gain-adjustment-metering/0`, a draft extension): the gain
//! adjustment, in dB, a plug-in applied to the last sample of the block it
//! just processed. A compressor or limiter reports its reduction through it;
//! the engine reads it after each block for the track's gain-reduction meter
//! (the mixer view spec, R40). A plug-in that doesn't implement it reports
//! nothing.

use clack_host::extensions::prelude::*;
use core::ffi::c_void;
use std::ffi::CStr;

/// The extension's ABI, as CLAP declares it: `get` is called on the audio
/// thread and returns the adjustment in dB (negative for a reduction).
#[derive(Clone, Copy)]
#[repr(C)]
struct Raw {
    get: Option<unsafe extern "C" fn(*const c_void) -> f64>,
}

/// The plug-in's side of the extension.
#[derive(Copy, Clone)]
pub(crate) struct PluginGainAdjustmentMetering(RawExtension<PluginExtensionSide, Raw>);

// SAFETY: `Raw` is repr(C) and ABI-compatible with the CLAP extension
// struct, and the identifier is the extension's own.
unsafe impl Extension for PluginGainAdjustmentMetering {
    const IDENTIFIERS: &[&CStr] = &[c"clap.gain-adjustment-metering/0"];
    type ExtensionSide = PluginExtensionSide;

    unsafe fn from_raw(raw: RawExtension<PluginExtensionSide>) -> Self {
        // SAFETY: the caller guarantees the pointer is this extension.
        Self(unsafe { raw.cast() })
    }
}

/// The gain adjustment, in dB, the plug-in applied to the last sample of
/// the block it last processed, or `None` when it implements no such report.
/// Audio thread only, as CLAP specifies.
pub(crate) fn reduction(plugin: &PluginAudioProcessorHandle<'_>) -> Option<f64> {
    let extension = plugin.get_extension::<PluginGainAdjustmentMetering>()?;
    let get = plugin.use_extension(&extension.0).get?;
    // SAFETY: `get` is the plug-in's own function, and this is its audio
    // thread, where CLAP allows the call.
    Some(unsafe { get(plugin.as_raw_ptr().cast()) })
}

//! Fuzzes the device preset decoder: bytes as read from a `.blpreset` file,
//! through JSON parsing, schema version check, device info, and parameter decoding.

#![no_main]

use beatlayer_fuzz::check_preset;
use bl_project::preset::Preset;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(preset) = Preset::decode(data) {
        check_preset(&preset);
    }
});

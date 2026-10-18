//! Fuzzes the SFZ reader: bytes as read from an `.sfz` file, through its
//! comments, `#define`s, `#include`s (none found here), headers and opcodes.

#![no_main]

use std::path::Path;

use bl_interop::sfz;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let import = sfz::parse(data, Path::new("/lib"), &|_| None);
    // What the reader hands on keeps the layers' own rules.
    for region in &import.regions {
        assert!(region.key.low <= region.key.high);
        assert!(region.velocity.low >= 1 && region.velocity.low <= region.velocity.high);
        assert!((-50.0..=50.0).contains(&region.detune) && (-50.0..=50.0).contains(&region.pan));
    }
});

//! Fuzzes the set loader from the JSON on, past the compression and its
//! checksum, which random bytes rarely get through.

#![no_main]

use beatlayer_fuzz::{OPTIONS, check_loaded};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok((project, info)) = bl_project::decode_json(data, &OPTIONS) {
        check_loaded(&project, &info);
    }
});

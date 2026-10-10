//! Fuzzes the whole set loader: bytes as read from a `.blset` file, through
//! zstd, the checksum, the JSON, migrations, and the document model's rules.

#![no_main]

use beatlayer_fuzz::{OPTIONS, check_loaded};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok((project, info)) = bl_project::decode(data, &OPTIONS) {
        check_loaded(&project, &info);
    }
});

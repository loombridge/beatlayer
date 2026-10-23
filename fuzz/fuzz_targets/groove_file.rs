//! Fuzzes the groove file decoder: bytes as read from a `.blgroove` file,
//! through JSON parsing, schema version check, and the grooves spec's limits.

#![no_main]

use beatlayer_fuzz::check_groove;
use bl_project::groove_file::GrooveFile;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(groove) = GrooveFile::decode(data) {
        check_groove(&groove);
    }
});

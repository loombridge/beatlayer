//! Fuzzes the pack reader: bytes as read from a `.blpack` file, through the
//! ZIP records, the manifest, every entry's name, and the size and checksum
//! of every file.

#![no_main]

use std::io::Cursor;
use std::path::Component;

use bl_project::pack::{SCHEMA, verify};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // A pack that declares more than this is refused unread, so a
    // decompression bomb stops quickly.
    if let Ok(plan) = verify(Cursor::new(data), 8 << 20) {
        assert!(plan.manifest.schema <= SCHEMA);
        for entry in &plan.entries {
            assert!(
                entry.path().components().all(|c| matches!(c, Component::Normal(_))),
                "an entry leaves the project folder: {:?}",
                entry.path()
            );
        }
    }
});

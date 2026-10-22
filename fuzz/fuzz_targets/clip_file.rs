//! Fuzzes the clip file reader: bytes as read from a `.blclip` file, through
//! zstd, the checksum, the JSON, and the clip file's rules.

#![no_main]

use beatlayer_fuzz::check_clip_file;
use bl_project::clip_file::ClipFile;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(file) = ClipFile::decode(data) {
        check_clip_file(&file);
    }
});

//! Fuzzes the peak file reader: bytes as read from a `.blpeaks` file,
//! through magic, header, run table, data block, and BLAKE3 checksum.

#![no_main]

use beatlayer_fuzz::check_peaks;
use bl_media::ContentHash;
use bl_media::peaks::Peaks;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let source = match data.get(16..48) {
        Some(slice) => ContentHash::from_bytes(slice.try_into().unwrap()),
        None => ContentHash::from_bytes([0; 32]),
    };
    if let Ok(peaks) = Peaks::from_file_bytes(data, source) {
        check_peaks(&peaks, source);
    }
});

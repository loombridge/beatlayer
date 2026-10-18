//! Fuzzes the MIDI file reader: bytes as read from a `.mid`, through the
//! size cap, the header chunk, the track chunks and their events, and the
//! tempo and time-signature maps.

#![no_main]

use bl_interop::midi;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(imported) = midi::read_bytes(data) else {
        return;
    };
    // What the reader hands on keeps the clip's own rules.
    for track in &imported.tracks {
        for note in &track.notes {
            assert!(note.length.0 > 0, "a note without length");
            assert!(note.key <= 127 && (1..=127).contains(&note.velocity));
            assert!(note.start.saturating_add(note.length) <= track.length);
        }
        assert!(track.notes.is_sorted_by_key(|note| (note.start, note.key)));
    }
});

//! The capacity target's song (the architecture, "Requirements that shape
//! the design"): many audio tracks with an EQ and a compressor each, which
//! the multicore scheduler (WP-5.37) must play at 48 kHz in blocks of 128
//! frames without a dropout.
//!
//! Each track's sound is a test tone of its own pitch, so the EQ and the
//! compressor have a signal to work on and a render shows any sum that came
//! in the wrong order.

use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_model::{Command, Device, DeviceKind, Editor, TrackId, TrackKind};

/// A song of `tracks` audio tracks, each a test tone at −24 dBFS through
/// EQ Eight (the EQ) and Compressor (the compressor), all into Main. Returns the
/// editor holding it and the tracks, in order.
///
/// # Panics
///
/// If the model refuses a command, which it doesn't for this song.
#[must_use]
pub fn song(tracks: usize) -> (Editor, Vec<TrackId>) {
    let mut editor = Editor::new();
    let mut made = Vec::with_capacity(tracks);
    for n in 0..tracks {
        let id = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind: TrackKind::Audio,
                name: format!("Track {}", n + 1),
                parent: None,
                index: n,
            })
            .expect("a track");
        for (index, device_type) in [TEST_TONE, bl_devices::EQ, bl_devices::COMPRESSOR]
            .into_iter()
            .enumerate()
        {
            let mut device = Device::new(
                editor.new_id(),
                "",
                DeviceKind::Native {
                    device_type: device_type.to_owned(),
                },
            );
            if device_type == TEST_TONE {
                device
                    .params
                    .insert(test_tone::FREQUENCY, 110.0 * (1.0 + n as f64 * 0.37));
                device.params.insert(test_tone::LEVEL, -24.0);
            }
            editor
                .apply(Command::InsertDevice {
                    track: id,
                    index,
                    device,
                })
                .expect("a device");
        }
        made.push(id);
    }
    (editor, made)
}

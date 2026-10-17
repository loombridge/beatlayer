//! A device's display data, from its node to the main thread (the device
//! framework spec, R35 to R37): after each block a node publishes the
//! latest values its display draws (a spectrum, a gain reduction), through
//! a lock-free triple buffer made with the node, as the engine publishes
//! its meters. The shell reads the newest; nothing on the audio thread
//! allocates, locks, or blocks.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use bl_model::DeviceId;

/// What a node published: the values of its display, and how many times
/// it has published.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisplayData {
    /// How many blocks have published, counting from 1.
    pub published: u64,
    /// The values, as the device type lays them out.
    pub values: Vec<f32>,
}

/// The audio thread's end of a display's handoff.
pub struct DisplayWriter {
    input: triple_buffer::Input<DisplayData>,
    published: u64,
}

impl DisplayWriter {
    /// Publishes the display's values: `fill` writes them in place, and
    /// the newest are what the reader sees next. Never allocates.
    pub fn publish(&mut self, fill: impl FnOnce(&mut [f32])) {
        // The buffer being written holds an older publication, so the count
        // is kept here.
        self.published += 1;
        let data = self.input.input_buffer_mut();
        fill(&mut data.values);
        data.published = self.published;
        self.input.publish();
    }
}

/// The main thread's end of a display's handoff.
pub struct DisplayReader(triple_buffer::Output<DisplayData>);

impl DisplayReader {
    /// The newest data the node published, or the zeros it started with.
    pub fn read(&mut self) -> &DisplayData {
        self.0.read()
    }
}

/// A display of `len` values, started at zero, made off the audio thread.
#[must_use]
pub fn display_channel(len: usize) -> (DisplayWriter, DisplayReader) {
    let (input, output) = triple_buffer::triple_buffer(&DisplayData {
        published: 0,
        values: vec![0.0; len],
    });
    (
        DisplayWriter {
            input,
            published: 0,
        },
        DisplayReader(output),
    )
}

/// The readers of every device's display in the running program, shared
/// between the compiler, which adds a node's when it makes the node, and
/// the engine handle the shell reads through. Only those two threads take
/// its lock; the audio thread never sees it.
#[derive(Clone, Default)]
pub struct Displays(Arc<Mutex<BTreeMap<DeviceId, DisplayReader>>>);

impl Displays {
    /// Adds the reader of `device`'s display, replacing an older node's.
    pub(crate) fn insert(&self, device: DeviceId, reader: DisplayReader) {
        self.lock().insert(device, reader);
    }

    /// Forgets the devices `keep` refuses: those the program no longer has.
    pub(crate) fn retain(&self, mut keep: impl FnMut(DeviceId) -> bool) {
        self.lock().retain(|device, _| keep(*device));
    }

    /// Copies the newest data of every display into `out`, reusing what it
    /// already holds.
    pub fn read_all(&self, out: &mut BTreeMap<DeviceId, DisplayData>) {
        let mut readers = self.lock();
        out.retain(|device, _| readers.contains_key(device));
        for (device, reader) in readers.iter_mut() {
            let data = reader.read();
            let slot = out.entry(*device).or_default();
            slot.published = data.published;
            slot.values.clear();
            slot.values.extend_from_slice(&data.values);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<DeviceId, DisplayReader>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl std::fmt::Debug for Displays {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Displays").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reader_sees_the_newest_publication() {
        let device = DeviceId(bl_core::id::Id::new(1).unwrap());
        let (mut writer, reader) = display_channel(2);
        let displays = Displays::default();
        displays.insert(device, reader);
        let mut read = BTreeMap::new();
        displays.read_all(&mut read);
        assert_eq!(read[&device].published, 0);
        writer.publish(|v| v.copy_from_slice(&[1.0, 2.0]));
        writer.publish(|v| v.copy_from_slice(&[3.0, 4.0]));
        displays.read_all(&mut read);
        assert_eq!(read[&device].published, 2);
        assert_eq!(read[&device].values, [3.0, 4.0]);
        displays.retain(|_| false);
        displays.read_all(&mut read);
        assert!(read.is_empty());
    }
}

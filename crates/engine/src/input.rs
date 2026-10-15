//! The audio device's input, as the engine reads it (WP-2.13).
//!
//! A track whose Audio From is Ext. In hears the device's input: the
//! processor is given one block of it per call, through [`DeviceInput`],
//! and copies it into the running program before the tasks run. Copying,
//! not borrowing, keeps every node's buffer the program's own, made when
//! the engine was.

/// One block of the audio device's input: planar channels of the same
/// length.
///
/// Implemented by whoever runs the engine on a device — `bl-shell`'s
/// callback wraps the backend's input buffer — and by [`NoInput`] where
/// there is none.
pub trait DeviceInput {
    /// How many channels the device's input has.
    fn channels(&self) -> usize;

    /// One channel's samples for this block.
    ///
    /// # Panics
    ///
    /// If `index` isn't less than [`channels`](Self::channels).
    fn channel(&self, index: usize) -> &[f32];
}

/// A device with no input: what an offline render, and any caller without a
/// device, passes.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoInput;

impl DeviceInput for NoInput {
    fn channels(&self) -> usize {
        0
    }

    fn channel(&self, _index: usize) -> &[f32] {
        &[]
    }
}

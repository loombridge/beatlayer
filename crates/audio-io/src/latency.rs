//! The latency a device reports, in frames, in each direction (WP-2.13).
//!
//! A recording is moved earlier by it (Spec: recording R10), so it counts the
//! buffer once. Core Audio reads the device's properties ([`core_audio`]).
//! Where `cpal` hides the handle a query needs (WASAPI's audio client, ALSA's
//! PCM, JACK's client), the stream measures the gap between its callbacks'
//! own timestamps instead ([`Gauge`]); that gap already holds the buffer.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// How many callbacks of each direction the fallback averages before it
/// publishes.
const MEASURED_CALLBACKS: u32 = 16;

/// What a measurement slot holds before the gauge has published: a slot keeps
/// `frames + 1`, so zero means not yet.
const UNPUBLISHED: u32 = 0;

/// A frame count the audio thread publishes once, read from any thread.
pub(crate) type Slot = Arc<AtomicU32>;

/// The published frames, or `None` before the gauge has finished.
pub(crate) fn published(slot: &AtomicU32) -> Option<u32> {
    match slot.load(Ordering::Acquire) {
        UNPUBLISHED => None,
        stored => Some(stored - 1),
    }
}

/// Averages the gap between a callback's timestamps over its first callbacks
/// and publishes it once. It lives in the callback and touches only atomics.
pub(crate) struct Gauge {
    slot: Slot,
    sample_rate: u32,
    total: Duration,
    seen: u32,
}

impl Gauge {
    pub(crate) fn new(slot: Slot, sample_rate: u32) -> Self {
        Self {
            slot,
            sample_rate,
            total: Duration::ZERO,
            seen: 0,
        }
    }

    /// Takes one callback's gap. Real-time safe.
    pub(crate) fn feed(&mut self, gap: Duration) {
        if self.seen == MEASURED_CALLBACKS {
            return;
        }
        self.total += gap;
        self.seen += 1;
        if self.seen == MEASURED_CALLBACKS {
            let mean = self.total.as_secs_f64() / f64::from(MEASURED_CALLBACKS);
            let frames = (mean * f64::from(self.sample_rate)).round() as u32;
            self.slot.store(frames.saturating_add(1), Ordering::Release);
        }
    }
}

/// Core Audio's report for one direction: the device's latency, its safety
/// offset, the stream's latency in that scope, and the buffer, which none of
/// the first three holds.
#[cfg(target_os = "macos")]
fn core_audio_frames(device: u32, safety: u32, stream: u32, buffer: u32) -> u32 {
    device
        .saturating_add(safety)
        .saturating_add(stream)
        .saturating_add(buffer)
}

/// Reads a Core Audio device's latency, input and output, through the same
/// bindings `cpal` uses.
#[cfg(target_os = "macos")]
pub(crate) mod core_audio {
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::ptr::NonNull;

    use objc2_core_audio::{
        AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress,
        AudioObjectPropertyScope, AudioObjectPropertySelector, kAudioDevicePropertyBufferFrameSize,
        kAudioDevicePropertyLatency, kAudioDevicePropertySafetyOffset, kAudioDevicePropertyStreams,
        kAudioHardwarePropertyTranslateUIDToDevice, kAudioObjectPropertyElementMain,
        kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeInput,
        kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject, kAudioStreamPropertyLatency,
    };
    use objc2_core_foundation::CFString;

    use super::core_audio_frames;

    /// One property of `object`, or `None` if Core Audio refuses it. `T` is
    /// the property's type, a `u32` or an `AudioObjectID`.
    fn property<T: Default>(
        object: AudioObjectID,
        selector: AudioObjectPropertySelector,
        scope: AudioObjectPropertyScope,
        qualifier: Option<&CFString>,
    ) -> Option<T> {
        let mut address = AudioObjectPropertyAddress {
            mSelector: selector,
            mScope: scope,
            mElement: kAudioObjectPropertyElementMain,
        };
        let mut value = T::default();
        let mut size = size_of::<T>() as u32;
        let (qualifier_size, qualifier) = match qualifier {
            Some(string) => (
                size_of::<*const CFString>() as u32,
                std::ptr::from_ref(&string).cast::<c_void>(),
            ),
            None => (0, std::ptr::null()),
        };
        // SAFETY: the address, size and value are live locals, `T` is the
        // size of the property's value, and the qualifier, when there is
        // one, is a pointer to a CFStringRef that outlives the call.
        let status = unsafe {
            AudioObjectGetPropertyData(
                object,
                NonNull::from(&mut address),
                qualifier_size,
                qualifier,
                NonNull::from(&mut size),
                NonNull::from(&mut value).cast(),
            )
        };
        (status == 0).then_some(value)
    }

    /// The first stream of `device` in `scope`.
    fn first_stream(
        device: AudioObjectID,
        scope: AudioObjectPropertyScope,
    ) -> Option<AudioObjectID> {
        // Asking for one `AudioObjectID` of a list returns the first.
        property(device, kAudioDevicePropertyStreams, scope, None)
    }

    /// The frames between a sound and the stream in `scope`: the device's
    /// latency, safety offset and stream latency, and the current buffer.
    fn direction(device: AudioObjectID, scope: AudioObjectPropertyScope) -> u32 {
        let read = |selector| property::<u32>(device, selector, scope, None).unwrap_or(0);
        let stream = first_stream(device, scope).map_or(0, |stream| {
            property::<u32>(
                stream,
                kAudioStreamPropertyLatency,
                kAudioObjectPropertyScopeGlobal,
                None,
            )
            .unwrap_or(0)
        });
        core_audio_frames(
            read(kAudioDevicePropertyLatency),
            read(kAudioDevicePropertySafetyOffset),
            stream,
            read(kAudioDevicePropertyBufferFrameSize),
        )
    }

    /// The input and output latency of the device with this UID, or `None`
    /// when Core Audio doesn't know the device.
    pub(crate) fn of_device(uid: &str) -> Option<(u32, u32)> {
        let uid = CFString::from_str(uid);
        let device: AudioObjectID = property(
            kAudioObjectSystemObject as AudioObjectID,
            kAudioHardwarePropertyTranslateUIDToDevice,
            kAudioObjectPropertyScopeGlobal,
            Some(&uid),
        )?;
        Some((
            direction(device, kAudioObjectPropertyScopeInput),
            direction(device, kAudioObjectPropertyScopeOutput),
        ))
    }

    /// The default devices on the machine running the test: each is read
    /// through `cpal`, so the UID is the one a stream opens with.
    #[cfg(test)]
    mod tests {
        use std::str::FromStr;

        use cpal::traits::{DeviceTrait, HostTrait};

        use super::*;

        /// Adds up each property of the default output device on its own and
        /// compares with `of_device`. Skips on a machine with no device.
        #[test]
        fn the_default_device_reports_the_sum_of_its_properties() {
            let host = cpal::default_host();
            let Some(device) = host.default_output_device() else {
                return;
            };
            let id = crate::DeviceId::from_str(&device.id().unwrap().to_string()).unwrap();
            let uid = id.local();
            let name = device
                .description()
                .map(|d| d.name().to_owned())
                .unwrap_or_default();
            let (input, output) = of_device(uid).expect("Core Audio knows its own default device");

            let uid_string = CFString::from_str(uid);
            let object: AudioObjectID = property(
                kAudioObjectSystemObject as AudioObjectID,
                kAudioHardwarePropertyTranslateUIDToDevice,
                kAudioObjectPropertyScopeGlobal,
                Some(&uid_string),
            )
            .unwrap();
            for (scope, reported) in [
                (kAudioObjectPropertyScopeInput, input),
                (kAudioObjectPropertyScopeOutput, output),
            ] {
                let read = |selector| property::<u32>(object, selector, scope, None).unwrap_or(0);
                let stream = first_stream(object, scope).map_or(0, |stream| {
                    property::<u32>(
                        stream,
                        kAudioStreamPropertyLatency,
                        kAudioObjectPropertyScopeGlobal,
                        None,
                    )
                    .unwrap_or(0)
                });
                let (latency, safety, buffer) = (
                    read(kAudioDevicePropertyLatency),
                    read(kAudioDevicePropertySafetyOffset),
                    read(kAudioDevicePropertyBufferFrameSize),
                );
                println!(
                    "{name} ({scope:#x}): device {latency} + safety {safety} + stream {stream} + buffer {buffer} = {reported} frames"
                );
                assert_eq!(reported, latency + safety + stream + buffer);
            }
            if input == 0 && output == 0 {
                println!("{name} reports no latency");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gauge_publishes_the_mean_gap_once_after_its_callbacks() {
        let slot = Slot::default();
        let mut gauge = Gauge::new(Arc::clone(&slot), 48_000);
        for _ in 0..MEASURED_CALLBACKS - 1 {
            gauge.feed(Duration::from_millis(10));
        }
        assert_eq!(published(&slot), None);

        gauge.feed(Duration::from_millis(10));
        assert_eq!(published(&slot), Some(480));

        // A later callback changes nothing.
        gauge.feed(Duration::from_secs(1));
        assert_eq!(published(&slot), Some(480));
    }

    #[test]
    fn a_gap_of_zero_is_published_as_zero_not_as_missing() {
        let slot = Slot::default();
        let mut gauge = Gauge::new(Arc::clone(&slot), 44_100);
        for _ in 0..MEASURED_CALLBACKS {
            gauge.feed(Duration::ZERO);
        }
        assert_eq!(published(&slot), Some(0));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn core_audio_counts_the_buffer_once_beside_the_three_properties() {
        assert_eq!(core_audio_frames(24, 32, 11, 256), 323);
        assert_eq!(core_audio_frames(0, 0, 0, 128), 128);
        assert_eq!(core_audio_frames(u32::MAX, 1, 1, 1), u32::MAX);
    }
}

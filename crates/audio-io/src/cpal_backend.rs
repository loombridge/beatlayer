//! Core Audio, WASAPI (shared), PipeWire, JACK, and ALSA through `cpal`.
//!
//! `cpal` delivers interleaved samples in the device's format. Each device
//! callback is split into blocks of at most `max_block_frames`, rendered into
//! planar scratch buffers by the [`CallbackRunner`], then interleaved and
//! converted into the device's buffer.
//!
//! Two `cpal` behaviours matter on macOS. A stream at a rate other than the
//! device's current one changes the device's nominal rate, which persists
//! after Beatlayer quits, so a request without a rate uses the current rate.
//! And a stream on the default device follows the system default when it
//! changes, reporting each change, while a stream on a named device stays on
//! it and reports when it goes away.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample, SupportedBufferSize, SupportedStreamConfigRange};

use bl_rt::RtThreadConfig;

use crate::buffer::PlanarScratch;
use crate::runner::{CallbackRunner, frames_to_duration};
use crate::stats::Counters;
use crate::{
    AudioBuffer, AudioCallback, Backend, BackendKind, BufferSizes, DEFAULT_MAX_BLOCK_FRAMES,
    DeviceId, DeviceInfo, DeviceSelector, Error, ErrorKind, OpenDevice, STANDARD_SAMPLE_RATES,
    SampleFormat, Stream, StreamInfo, StreamRequest, StreamStats, Workgroup, WorkgroupInfo,
};

/// The buffer size assumed for an audio thread's scheduling period when the
/// request leaves it to the device, which `cpal` reports only once audio
/// runs. Only macOS uses the period, and only for a thread Core Audio hasn't
/// already made real-time.
const ASSUMED_BUFFER: u32 = 512;

/// The `cpal` backends this machine can run, best first.
pub(crate) fn available() -> Vec<BackendKind> {
    let mut kinds: Vec<BackendKind> = cpal::available_hosts()
        .iter()
        .filter_map(|host| BackendKind::from_id(&host.to_string()))
        .collect();
    let default = default_kind();
    kinds.sort_by_key(|kind| *kind != default);
    kinds
}

/// The backend `cpal` would pick by default.
pub(crate) fn default_kind() -> BackendKind {
    BackendKind::from_id(&cpal::default_host().id().to_string()).unwrap_or(BackendKind::Null)
}

/// One `cpal` host.
pub(crate) struct CpalBackend {
    kind: BackendKind,
    host: cpal::Host,
}

impl CpalBackend {
    pub(crate) fn new(kind: BackendKind) -> Result<Self, Error> {
        if kind == BackendKind::PipeWire {
            return Err(Error::new(
                ErrorKind::BackendUnavailable,
                "Beatlayer has no native PipeWire backend yet; \
                 PipeWire plays through the alsa and jack backends",
            ));
        }
        let unavailable = |why: String| {
            Error::new(
                ErrorKind::BackendUnavailable,
                format!("{kind} isn't available here: {why}"),
            )
        };
        let id = cpal::HostId::from_str(kind.id()).map_err(|e| unavailable(e.to_string()))?;
        let host = cpal::host_from_id(id).map_err(|e| unavailable(e.to_string()))?;
        Ok(Self { kind, host })
    }

    fn device_id(&self, device: &cpal::Device) -> Result<DeviceId, Error> {
        let id = device.id().map_err(from_cpal)?;
        DeviceId::from_str(&id.to_string())
    }

    fn describe(
        &self,
        device: &cpal::Device,
        default_output: Option<&DeviceId>,
        default_input: Option<&DeviceId>,
    ) -> Result<DeviceInfo, Error> {
        let id = self.device_id(device)?;
        let name = device
            .description()
            .map_or_else(|_| device.to_string(), |d| d.name().to_owned());
        let outputs: Vec<SupportedStreamConfigRange> = device
            .supported_output_configs()
            .map(Iterator::collect)
            .unwrap_or_default();
        let inputs: Vec<SupportedStreamConfigRange> = device
            .supported_input_configs()
            .map(Iterator::collect)
            .unwrap_or_default();
        let ranges = if outputs.is_empty() {
            &inputs
        } else {
            &outputs
        };
        let current_sample_rate = if outputs.is_empty() {
            device.default_input_config().ok()
        } else {
            device.default_output_config().ok()
        }
        .map(|config| config.sample_rate());

        Ok(DeviceInfo {
            is_default_output: default_output == Some(&id),
            is_default_input: default_input == Some(&id),
            id,
            name,
            output_channels: max_channels(&outputs),
            input_channels: max_channels(&inputs),
            sample_rates: sample_rates(ranges, current_sample_rate),
            current_sample_rate,
            buffer_sizes: buffer_sizes(ranges),
            default_buffer_size: None,
        })
    }
}

impl Backend for CpalBackend {
    fn kind(&self) -> BackendKind {
        self.kind
    }

    fn devices(&self) -> Result<Vec<DeviceInfo>, Error> {
        let id_of = |device: Option<cpal::Device>| device.and_then(|d| self.device_id(&d).ok());
        let default_output = id_of(self.host.default_output_device());
        let default_input = id_of(self.host.default_input_device());
        let devices = self.host.devices().map_err(from_cpal)?;
        Ok(devices
            .filter_map(|device| {
                self.describe(&device, default_output.as_ref(), default_input.as_ref())
                    .ok()
            })
            .collect())
    }

    fn open_stream(
        &self,
        request: &StreamRequest,
        callback: Box<dyn AudioCallback>,
    ) -> Result<Box<dyn Stream>, Error> {
        if request.input.is_some() {
            return Err(Error::new(
                ErrorKind::Unsupported,
                format!(
                    "{} streams through cpal are output-only: cpal 0.18 has no duplex streams \
                     on Core Audio or PipeWire. Input arrives with recording (WP-2.13).",
                    self.kind
                ),
            ));
        }
        let output = request.output.as_ref().ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidInput,
                "a stream needs output, input, or both",
            )
        })?;

        let (device, follows_default) = match &output.device {
            DeviceSelector::Default => {
                let device = self.host.default_output_device().ok_or_else(|| {
                    Error::new(
                        ErrorKind::DeviceNotFound,
                        format!("{} has no default output device", self.kind),
                    )
                })?;
                (device, true)
            }
            DeviceSelector::Id(id) => {
                if id.backend() != self.kind {
                    return Err(Error::new(
                        ErrorKind::DeviceNotFound,
                        format!("`{id}` isn't a {} device", self.kind),
                    ));
                }
                let cpal_id = cpal::DeviceId::from_str(id.as_str()).map_err(from_cpal)?;
                let device = self.host.device_by_id(&cpal_id).ok_or_else(|| {
                    Error::new(ErrorKind::DeviceNotFound, format!("no device `{id}`"))
                })?;
                (device, false)
            }
        };
        let id = self.device_id(&device)?;
        let name = device
            .description()
            .map_or_else(|_| device.to_string(), |d| d.name().to_owned());

        let current = device.default_output_config().map_err(from_cpal)?;
        let sample_rate = request.sample_rate.unwrap_or(current.sample_rate());
        let channels = output.channels.unwrap_or(current.channels());
        let ranges: Vec<SupportedStreamConfigRange> = device
            .supported_output_configs()
            .map_err(from_cpal)?
            .collect();
        let range = choose_range(&ranges, channels, sample_rate, current.sample_format())
            .ok_or_else(|| {
                let noun = if channels == 1 { "channel" } else { "channels" };
                Error::new(
                    ErrorKind::UnsupportedConfig,
                    format!("{name} can't play {channels} {noun} at {sample_rate} Hz"),
                )
            })?;
        let buffer_size = match request.buffer_size {
            None => cpal::BufferSize::Default,
            Some(frames) => {
                if let SupportedBufferSize::Range { min, max } = *range.buffer_size()
                    && !(min..=max).contains(&frames)
                {
                    return Err(Error::new(
                        ErrorKind::UnsupportedConfig,
                        format!("{name} takes buffers of {min} to {max} frames, not {frames}"),
                    ));
                }
                cpal::BufferSize::Fixed(frames)
            }
        };
        let config = cpal::StreamConfig {
            channels,
            sample_rate,
            buffer_size,
        };
        let max_block_frames = request
            .buffer_size
            .unwrap_or(0)
            .max(DEFAULT_MAX_BLOCK_FRAMES);

        // Core Audio's I/O thread is real-time and already in its device's
        // workgroup; other real-time threads join the same one.
        let workgroup = if self.kind == BackendKind::CoreAudio {
            Workgroup::of_core_audio_device(id.local()).ok()
        } else {
            None
        };
        let thread_config = RtThreadConfig {
            period: frames_to_duration(
                u64::from(request.buffer_size.unwrap_or(ASSUMED_BUFFER)),
                sample_rate,
            ),
            promote: true,
            workgroup: workgroup.clone(),
        };

        let counters = Arc::new(Counters::new());
        let render = Render {
            runner: CallbackRunner::new(
                callback,
                Arc::clone(&counters),
                sample_rate,
                true,
                thread_config,
            ),
            scratch: PlanarScratch::new(channels.into(), max_block_frames as usize),
            channels: channels.into(),
            sample_rate,
        };
        let format = range.sample_format();
        let stream = build(&device, config, format, render, Arc::clone(&counters))?;
        let info = StreamInfo {
            backend: self.kind,
            output_device: Some(OpenDevice { id, name }),
            input_device: None,
            follows_default,
            sample_rate,
            output_channels: channels,
            input_channels: 0,
            sample_format: sample_format(format),
            requested_buffer_size: request.buffer_size,
            buffer_size: stream.buffer_size().ok(),
            max_block_frames,
            workgroup: workgroup.as_ref().map(WorkgroupInfo::of),
        };
        Ok(Box::new(CpalStream {
            stream,
            info,
            counters,
            workgroup,
        }))
    }
}

/// The supported range to open: the device's current format if it fits,
/// otherwise the most precise format that does.
fn choose_range(
    ranges: &[SupportedStreamConfigRange],
    channels: u16,
    sample_rate: u32,
    current_format: cpal::SampleFormat,
) -> Option<SupportedStreamConfigRange> {
    let fits = |range: &&SupportedStreamConfigRange| {
        range.channels() == channels
            && range.contains_rate(sample_rate)
            && to_sample_format(range.sample_format()).is_some()
    };
    let preference = |format: cpal::SampleFormat| match format {
        f if f == current_format => 0,
        cpal::SampleFormat::F32 => 1,
        cpal::SampleFormat::I32 => 2,
        cpal::SampleFormat::I24 => 3,
        cpal::SampleFormat::F64 => 4,
        cpal::SampleFormat::I16 => 5,
        _ => 6,
    };
    ranges
        .iter()
        .filter(fits)
        .min_by_key(|range| preference(range.sample_format()))
        .copied()
}

fn max_channels(ranges: &[SupportedStreamConfigRange]) -> u16 {
    ranges.iter().map(|r| r.channels()).max().unwrap_or(0)
}

/// The standard rates any range accepts, plus the current rate.
fn sample_rates(ranges: &[SupportedStreamConfigRange], current: Option<u32>) -> Vec<u32> {
    let mut rates: Vec<u32> = STANDARD_SAMPLE_RATES
        .into_iter()
        .chain(current)
        .filter(|&rate| ranges.iter().any(|r| r.contains_rate(rate)))
        .collect();
    rates.sort_unstable();
    rates.dedup();
    rates
}

/// The widest buffer range across the device's formats.
fn buffer_sizes(ranges: &[SupportedStreamConfigRange]) -> BufferSizes {
    ranges
        .iter()
        .filter_map(|range| match *range.buffer_size() {
            SupportedBufferSize::Range { min, max } => Some((min, max)),
            SupportedBufferSize::Unknown => None,
        })
        .reduce(|(a, b), (c, d)| (a.min(c), b.max(d)))
        .map_or(BufferSizes::Unknown, |(min, max)| BufferSizes::Range {
            min,
            max,
        })
}

fn to_sample_format(format: cpal::SampleFormat) -> Option<SampleFormat> {
    Some(match format {
        cpal::SampleFormat::I8 => SampleFormat::I8,
        cpal::SampleFormat::I16 => SampleFormat::I16,
        cpal::SampleFormat::I24 => SampleFormat::I24,
        cpal::SampleFormat::I32 => SampleFormat::I32,
        cpal::SampleFormat::I64 => SampleFormat::I64,
        cpal::SampleFormat::U8 => SampleFormat::U8,
        cpal::SampleFormat::U16 => SampleFormat::U16,
        cpal::SampleFormat::U24 => SampleFormat::U24,
        cpal::SampleFormat::U32 => SampleFormat::U32,
        cpal::SampleFormat::U64 => SampleFormat::U64,
        cpal::SampleFormat::F32 => SampleFormat::F32,
        cpal::SampleFormat::F64 => SampleFormat::F64,
        _ => return None,
    })
}

fn sample_format(format: cpal::SampleFormat) -> SampleFormat {
    // `choose_range` only picks formats `to_sample_format` knows.
    to_sample_format(format).unwrap_or(SampleFormat::F32)
}

fn from_cpal(error: cpal::Error) -> Error {
    let kind = match error.kind() {
        cpal::ErrorKind::HostUnavailable => ErrorKind::BackendUnavailable,
        cpal::ErrorKind::DeviceNotAvailable => ErrorKind::DeviceNotFound,
        cpal::ErrorKind::DeviceBusy => ErrorKind::DeviceBusy,
        cpal::ErrorKind::UnsupportedConfig => ErrorKind::UnsupportedConfig,
        cpal::ErrorKind::UnsupportedOperation => ErrorKind::Unsupported,
        cpal::ErrorKind::InvalidInput => ErrorKind::InvalidInput,
        _ => ErrorKind::Backend,
    };
    Error::new(kind, error.to_string())
}

/// Counts an error `cpal` reports while a stream runs. `cpal` can call this
/// from the audio thread, so it only touches atomics.
fn record(counters: &Counters, error: &cpal::Error) {
    match error.kind() {
        cpal::ErrorKind::Xrun => counters.record_xrun(),
        cpal::ErrorKind::DeviceChanged => counters.record_device_change(),
        cpal::ErrorKind::DeviceNotAvailable => counters.record_device_lost(),
        cpal::ErrorKind::StreamInvalidated => counters.record_invalidated(),
        cpal::ErrorKind::RealtimeDenied => counters.record_realtime_denied(),
        _ => counters.record_other_error(),
    }
}

/// The state `cpal`'s data callback owns.
struct Render {
    runner: CallbackRunner,
    scratch: PlanarScratch,
    channels: usize,
    sample_rate: u32,
}

impl Render {
    /// Fills one device buffer: splits it into blocks, runs each through the
    /// callback, and interleaves the result into the device's format.
    fn render<T: SizedSample + FromSample<f32>>(
        &mut self,
        data: &mut [T],
        info: &cpal::OutputCallbackInfo,
    ) {
        let channels = self.channels.max(1);
        self.runner.device_callback(data.len() / channels);
        let start = stream_time(info.timestamp().callback);
        let mut done = 0u64;
        for chunk in data.chunks_mut(self.scratch.capacity() * channels) {
            let frames = chunk.len() / channels;
            let timestamp = start + frames_to_duration(done, self.sample_rate);
            let mut block = self.scratch.view_mut(frames);
            self.runner
                .run(Some(timestamp), &AudioBuffer::empty(frames), &mut block);
            interleave(&self.scratch.view(frames), chunk);
            done += frames as u64;
        }
    }
}

fn stream_time(instant: cpal::StreamInstant) -> Duration {
    Duration::from_nanos(u64::try_from(instant.as_nanos()).unwrap_or(u64::MAX))
}

/// Writes planar `source` into interleaved `target`, converting each sample.
fn interleave<T: FromSample<f32>>(source: &AudioBuffer<'_>, target: &mut [T]) {
    let channels = source.channels();
    if channels == 0 {
        return;
    }
    for (channel_index, channel) in source.iter().enumerate() {
        let frames = target.chunks_exact_mut(channels).zip(channel);
        for (frame, &sample) in frames {
            frame[channel_index] = T::from_sample_(sample);
        }
    }
}

fn build(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    format: cpal::SampleFormat,
    render: Render,
    counters: Arc<Counters>,
) -> Result<cpal::Stream, Error> {
    fn typed<T: SizedSample + FromSample<f32> + Send + 'static>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        mut render: Render,
        counters: Arc<Counters>,
    ) -> Result<cpal::Stream, cpal::Error> {
        device.build_output_stream::<T, _, _>(
            config,
            move |data, info| render.render(data, info),
            move |error| record(&counters, &error),
            None,
        )
    }
    use cpal::{I24, U24};
    let stream = match format {
        cpal::SampleFormat::I8 => typed::<i8>(device, config, render, counters),
        cpal::SampleFormat::I16 => typed::<i16>(device, config, render, counters),
        cpal::SampleFormat::I24 => typed::<I24>(device, config, render, counters),
        cpal::SampleFormat::I32 => typed::<i32>(device, config, render, counters),
        cpal::SampleFormat::I64 => typed::<i64>(device, config, render, counters),
        cpal::SampleFormat::U8 => typed::<u8>(device, config, render, counters),
        cpal::SampleFormat::U16 => typed::<u16>(device, config, render, counters),
        cpal::SampleFormat::U24 => typed::<U24>(device, config, render, counters),
        cpal::SampleFormat::U32 => typed::<u32>(device, config, render, counters),
        cpal::SampleFormat::U64 => typed::<u64>(device, config, render, counters),
        cpal::SampleFormat::F32 => typed::<f32>(device, config, render, counters),
        cpal::SampleFormat::F64 => typed::<f64>(device, config, render, counters),
        other => {
            return Err(Error::new(
                ErrorKind::UnsupportedConfig,
                format!("Beatlayer can't write {other} samples"),
            ));
        }
    };
    stream.map_err(from_cpal)
}

struct CpalStream {
    stream: cpal::Stream,
    info: StreamInfo,
    counters: Arc<Counters>,
    workgroup: Option<Workgroup>,
}

impl Stream for CpalStream {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn start(&mut self) -> Result<(), Error> {
        self.counters.request_resync();
        self.stream.play().map_err(from_cpal)
    }

    fn stop(&mut self) -> Result<(), Error> {
        self.stream.pause().map_err(from_cpal)
    }

    fn stats(&self) -> StreamStats {
        self.counters.snapshot()
    }

    fn current_buffer_size(&self) -> Option<u32> {
        self.stream.buffer_size().ok()
    }

    fn workgroup(&self) -> Option<Workgroup> {
        self.workgroup.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(
        channels: u16,
        min: u32,
        max: u32,
        format: cpal::SampleFormat,
    ) -> SupportedStreamConfigRange {
        SupportedStreamConfigRange::new(
            channels,
            min,
            max,
            SupportedBufferSize::Range { min: 16, max: 4096 },
            format,
        )
    }

    #[test]
    fn interleaving_converts_every_sample() {
        let mut scratch = PlanarScratch::new(2, 4);
        {
            let mut view = scratch.view_mut(3);
            view.channel_mut(0).copy_from_slice(&[0.5, -0.5, 1.0]);
            view.channel_mut(1).copy_from_slice(&[0.25, 0.0, -1.0]);
        }
        let mut floats = [9.0f32; 6];
        interleave(&scratch.view(3), &mut floats);
        assert_eq!(floats, [0.5, 0.25, -0.5, 0.0, 1.0, -1.0]);

        let mut ints = [0i16; 6];
        interleave(&scratch.view(3), &mut ints);
        assert_eq!(ints[0], i16::from_sample_(0.5f32));
        assert_eq!(ints[1], i16::from_sample_(0.25f32));
        assert_eq!(ints[3], 0);
    }

    #[test]
    fn the_current_format_wins_when_it_fits() {
        let ranges = [
            range(2, 44_100, 96_000, cpal::SampleFormat::I16),
            range(2, 44_100, 96_000, cpal::SampleFormat::F32),
        ];
        let chosen = choose_range(&ranges, 2, 48_000, cpal::SampleFormat::I16).unwrap();
        assert_eq!(chosen.sample_format(), cpal::SampleFormat::I16);
        let chosen = choose_range(&ranges, 2, 48_000, cpal::SampleFormat::U8).unwrap();
        assert_eq!(chosen.sample_format(), cpal::SampleFormat::F32);
    }

    #[test]
    fn a_range_must_fit_channels_and_rate() {
        let ranges = [range(2, 44_100, 48_000, cpal::SampleFormat::F32)];
        assert!(choose_range(&ranges, 1, 48_000, cpal::SampleFormat::F32).is_none());
        assert!(choose_range(&ranges, 2, 96_000, cpal::SampleFormat::F32).is_none());
        assert!(choose_range(&ranges, 2, 44_100, cpal::SampleFormat::F32).is_some());
    }

    #[test]
    fn device_capabilities_merge_across_ranges() {
        let ranges = [
            range(2, 44_100, 48_000, cpal::SampleFormat::F32),
            SupportedStreamConfigRange::new(
                8,
                96_000,
                96_000,
                SupportedBufferSize::Range { min: 64, max: 8192 },
                cpal::SampleFormat::I32,
            ),
        ];
        assert_eq!(max_channels(&ranges), 8);
        assert_eq!(
            sample_rates(&ranges, Some(44_100)),
            [44_100, 48_000, 96_000]
        );
        assert_eq!(
            buffer_sizes(&ranges),
            BufferSizes::Range { min: 16, max: 8192 }
        );
        assert_eq!(buffer_sizes(&[]), BufferSizes::Unknown);
        assert_eq!(max_channels(&[]), 0);
    }
}

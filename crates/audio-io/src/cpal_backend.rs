//! Core Audio, WASAPI (shared), PipeWire, JACK, and ALSA through `cpal`.
//!
//! `cpal` delivers interleaved samples in the device's format. Each device
//! callback is split into blocks of at most `max_block_frames`, rendered into
//! planar scratch buffers by the [`CallbackRunner`], then interleaved and
//! converted into the device's buffer.
//!
//! `cpal` has no duplex streams, so a stream that asks for input opens two
//! `cpal` streams on one device — the output one and an input one — and
//! [`DuplexBridge`] carries the input samples from the input callback to the
//! output callback through a lock-free ring made when the stream opens.
//! Both callbacks run on the one device's clock, so the ring's fill stays
//! put; it absorbs the jitter between the two callbacks.
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

use bl_rt::{AudioThreadSetup, RtThreadConfig};

use crate::buffer::PlanarScratch;
use crate::latency::{self, Gauge, Slot};
use crate::runner::{CallbackRunner, frames_to_duration};
use crate::stats::Counters;
use crate::{
    AudioBuffer, AudioCallback, Backend, BackendKind, BufferSizes, DEFAULT_MAX_BLOCK_FRAMES,
    DeviceId, DeviceInfo, DeviceSelector, Error, ErrorKind, Latency, OpenDevice,
    STANDARD_SAMPLE_RATES, SampleFormat, Stream, StreamInfo, StreamRequest, StreamStats, Workgroup,
    WorkgroupInfo,
};

/// The buffer size assumed for an audio thread's scheduling period when the
/// request leaves it to the device, which `cpal` reports only once audio
/// runs. Only macOS uses the period, and only for a thread Core Audio hasn't
/// already made real-time.
const ASSUMED_BUFFER: u32 = 512;

/// How many device buffers of duplex input the ring holds, so the input
/// callback can run ahead of the output callback between their deadlines.
const DUPLEX_BACKLOG: usize = 4;

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
        let output = request.output.as_ref().ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidInput,
                "a cpal stream needs output; input opens alongside it",
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
        let config = cpal::StreamConfig {
            channels,
            sample_rate,
            buffer_size: buffer_size(request.buffer_size, &range, &name)?,
        };
        let max_block_frames = request
            .buffer_size
            .unwrap_or(0)
            .max(DEFAULT_MAX_BLOCK_FRAMES);

        // Input opens alongside the output, on the same device: one clock.
        // `cpal` has no duplex streams, so it's a second stream and a
        // lock-free ring between the two callbacks.
        let input = match &request.input {
            None => None,
            Some(port) => {
                if let DeviceSelector::Id(input_id) = &port.device
                    && input_id != &id
                {
                    return Err(Error::new(
                        ErrorKind::UnsupportedConfig,
                        format!(
                            "a cpal stream's input and output must be one device, and `{input_id}` \
                             isn't `{id}`; an aggregate of several devices comes with a later package"
                        ),
                    ));
                }
                let current = device.default_input_config().map_err(from_cpal)?;
                let channels = port.channels.unwrap_or(current.channels());
                let ranges: Vec<SupportedStreamConfigRange> = device
                    .supported_input_configs()
                    .map_err(from_cpal)?
                    .collect();
                let range = choose_range(&ranges, channels, sample_rate, current.sample_format())
                    .ok_or_else(|| {
                    let noun = if channels == 1 { "channel" } else { "channels" };
                    Error::new(
                        ErrorKind::UnsupportedConfig,
                        format!("{name} can't record {channels} {noun} at {sample_rate} Hz"),
                    )
                })?;
                let config = cpal::StreamConfig {
                    channels,
                    sample_rate,
                    buffer_size: buffer_size(request.buffer_size, &range, &name)?,
                };
                Some((range.sample_format(), config))
            }
        };
        let input_channels = input.as_ref().map_or(0, |(_, config)| config.channels);

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
        let (input_slot, output_slot) = (Slot::default(), Slot::default());
        let (input_render, push) = match input_channels {
            0 => (None, None),
            channels => {
                let (render, push) = duplex_input(
                    channels.into(),
                    max_block_frames as usize,
                    Arc::clone(&counters),
                    thread_config.clone(),
                    Gauge::new(Arc::clone(&input_slot), sample_rate),
                );
                (Some(render), Some(push))
            }
        };
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
            input: input_render,
            gauge: Gauge::new(Arc::clone(&output_slot), sample_rate),
        };
        let format = range.sample_format();
        let stream = build(&device, config, format, render, Arc::clone(&counters))?;
        let input_stream = match (input, push) {
            (Some((format, config)), Some(push)) => Some(build_input(
                &device,
                config,
                format,
                push,
                Arc::clone(&counters),
            )?),
            _ => None,
        };
        let reported = reported_latency(self.kind, &id);
        let info = StreamInfo {
            backend: self.kind,
            output_device: Some(OpenDevice {
                id: id.clone(),
                name: name.clone(),
            }),
            input_device: (input_channels > 0).then_some(OpenDevice { id, name }),
            follows_default,
            sample_rate,
            output_channels: channels,
            input_channels,
            sample_format: sample_format(format),
            requested_buffer_size: request.buffer_size,
            buffer_size: stream.buffer_size().ok(),
            max_block_frames,
            input_latency: reported.map_or(0, |(input, _)| input),
            output_latency: reported.map_or(0, |(_, output)| output),
            workgroup: workgroup.as_ref().map(WorkgroupInfo::of),
        };
        Ok(Box::new(CpalStream {
            stream,
            input: input_stream,
            info,
            counters,
            workgroup,
            measured: reported.is_none().then_some((input_slot, output_slot)),
        }))
    }
}

/// The device's input and output latency in frames where the platform has a
/// query `cpal` doesn't wrap: Core Audio's device properties. `None` means the
/// stream measures it instead (WASAPI, ALSA and JACK hide the handle, and so
/// does Core Audio's device when its lookup fails).
fn reported_latency(kind: BackendKind, id: &DeviceId) -> Option<(u32, u32)> {
    #[cfg(target_os = "macos")]
    if kind == BackendKind::CoreAudio {
        return latency::core_audio::of_device(id.local());
    }
    let _ = (kind, id);
    None
}

/// The two sides of the duplex bridge, with room for [`DUPLEX_BACKLOG`]
/// blocks in the ring between them.
fn duplex_input(
    channels: usize,
    frames: usize,
    counters: Arc<Counters>,
    thread_config: RtThreadConfig,
    gauge: Gauge,
) -> (DuplexInput, PushInput) {
    let (producer, consumer) = rtrb::RingBuffer::new(channels * frames * DUPLEX_BACKLOG);
    // The input callback is an audio thread of its own, so it's set up as
    // the output's is (the architecture's real-time rules); the reports it
    // publishes are for whoever reads the stream's, which nothing does
    // here.
    let (setup, _reports) = AudioThreadSetup::new(thread_config);
    (
        DuplexInput {
            consumer,
            scratch: PlanarScratch::new(channels, frames),
            counters: Arc::clone(&counters),
            started: false,
        },
        PushInput {
            producer,
            counters,
            setup,
            gauge,
        },
    )
}

/// The output callback's side of the duplex bridge: the input samples the
/// device delivered, drained into planar scratch for the callback runner.
/// Made when the stream opens: the callback allocates nothing and locks
/// nothing. Where the ring is short (the input side is behind), it plays
/// what's there and silence, and counts an xrun.
struct DuplexInput {
    consumer: rtrb::Consumer<f32>,
    scratch: PlanarScratch,
    counters: Arc<Counters>,
    /// Whether the input side has delivered anything yet.
    started: bool,
}

impl DuplexInput {
    /// Fills the scratch with `frames` frames from the ring, whole frames
    /// only, silence where it's short. Real-time safe.
    fn drain(&mut self, frames: usize) {
        let channels = self.scratch.channels().max(1);
        let whole = (self.consumer.slots() / channels).min(frames);
        let mut view = self.scratch.view_mut(frames);
        for frame in 0..whole {
            for channel in view.iter_mut() {
                // `whole` frames are in the ring; a failure would need
                // another taker.
                channel[frame] = self.consumer.pop().unwrap_or(0.0);
            }
        }
        for channel in view.iter_mut() {
            channel[whole..].fill(0.0);
        }
        // Until the input side has delivered anything, silence is what a
        // stream that just opened plays, not an xrun.
        if whole < frames && (self.started || whole > 0) {
            self.counters.record_xrun();
        }
        self.started |= whole > 0;
    }
}

/// The input callback's side of the duplex bridge: pushes each device block
/// into the ring, interleaved, whole frames only, and drops what doesn't
/// fit (the output side is behind), counting an xrun. The callback
/// allocates nothing and locks nothing.
struct PushInput {
    producer: rtrb::Producer<f32>,
    counters: Arc<Counters>,
    /// The input thread's set-up, run on its first block.
    setup: AudioThreadSetup,
    /// Measures the input latency where the platform won't report it.
    gauge: Gauge,
}

impl PushInput {
    /// Pushes one interleaved device block of `channels` channels. Real-time
    /// safe.
    fn push<T: SizedSample>(&mut self, data: &[T], channels: usize)
    where
        f32: FromSample<T>,
    {
        if !self.setup.is_current_thread() {
            // The API's input thread, set up as the output's is: real-time
            // priority where the API hasn't given it, denormals flushed.
            bl_rt::realtime(|| {
                self.setup.enter();
            });
        }
        let channels = channels.max(1);
        let frames = data.len() / channels;
        let whole = (self.producer.slots() / channels).min(frames);
        for &sample in &data[..whole * channels] {
            // The ring has room; a failure would need another pusher.
            let _ = self.producer.push(f32::from_sample_(sample));
        }
        if whole < frames {
            self.counters.record_xrun();
        }
    }
}

/// The buffer size for a stream on `range`: the request's, if the range
/// takes it, or the device's default.
fn buffer_size(
    requested: Option<u32>,
    range: &SupportedStreamConfigRange,
    name: &str,
) -> Result<cpal::BufferSize, Error> {
    let Some(frames) = requested else {
        return Ok(cpal::BufferSize::Default);
    };
    if let SupportedBufferSize::Range { min, max } = *range.buffer_size()
        && !(min..=max).contains(&frames)
    {
        return Err(Error::new(
            ErrorKind::UnsupportedConfig,
            format!("{name} takes buffers of {min} to {max} frames, not {frames}"),
        ));
    }
    Ok(cpal::BufferSize::Fixed(frames))
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
    /// The duplex input, when the device has one.
    input: Option<DuplexInput>,
    /// Measures the output latency where the platform won't report it.
    gauge: Gauge,
}

impl Render {
    /// Fills one device buffer: splits it into blocks, runs each through the
    /// callback, and interleaves the result into the device's format. With a
    /// duplex input, the block's input comes from the bridge.
    fn render<T: SizedSample + FromSample<f32>>(
        &mut self,
        data: &mut [T],
        info: &cpal::OutputCallbackInfo,
    ) {
        let channels = self.channels.max(1);
        self.runner.device_callback(data.len() / channels);
        let stamps = info.timestamp();
        self.gauge
            .feed(stamps.playback.saturating_duration_since(stamps.callback));
        let start = stream_time(stamps.callback);
        let mut done = 0u64;
        for chunk in data.chunks_mut(self.scratch.capacity() * channels) {
            let frames = chunk.len() / channels;
            let timestamp = start + frames_to_duration(done, self.sample_rate);
            let mut block = self.scratch.view_mut(frames);
            let input = match self.input.as_mut() {
                Some(input) => {
                    input.drain(frames);
                    input.scratch.view(frames)
                }
                None => AudioBuffer::empty(frames),
            };
            self.runner.run(Some(timestamp), &input, &mut block);
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

/// Builds the device's input stream, converting the device's samples and
/// pushing them into `push`.
fn build_input(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    format: cpal::SampleFormat,
    push: PushInput,
    counters: Arc<Counters>,
) -> Result<cpal::Stream, Error> {
    fn typed<T: SizedSample + FromSample<f32> + Send + 'static>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        mut push: PushInput,
        counters: Arc<Counters>,
    ) -> Result<cpal::Stream, cpal::Error>
    where
        f32: FromSample<T>,
    {
        let channels = usize::from(config.channels);
        device.build_input_stream::<T, _, _>(
            config,
            move |data, info| {
                let stamps = info.timestamp();
                push.gauge
                    .feed(stamps.callback.saturating_duration_since(stamps.capture));
                push.push(data, channels);
            },
            move |error| record(&counters, &error),
            None,
        )
    }
    use cpal::{I24, U24};
    let stream = match format {
        cpal::SampleFormat::I8 => typed::<i8>(device, config, push, counters),
        cpal::SampleFormat::I16 => typed::<i16>(device, config, push, counters),
        cpal::SampleFormat::I24 => typed::<I24>(device, config, push, counters),
        cpal::SampleFormat::I32 => typed::<i32>(device, config, push, counters),
        cpal::SampleFormat::I64 => typed::<i64>(device, config, push, counters),
        cpal::SampleFormat::U8 => typed::<u8>(device, config, push, counters),
        cpal::SampleFormat::U16 => typed::<u16>(device, config, push, counters),
        cpal::SampleFormat::U24 => typed::<U24>(device, config, push, counters),
        cpal::SampleFormat::U32 => typed::<u32>(device, config, push, counters),
        cpal::SampleFormat::U64 => typed::<u64>(device, config, push, counters),
        cpal::SampleFormat::F32 => typed::<f32>(device, config, push, counters),
        cpal::SampleFormat::F64 => typed::<f64>(device, config, push, counters),
        other => {
            return Err(Error::new(
                ErrorKind::UnsupportedConfig,
                format!("Beatlayer can't read {other} samples"),
            ));
        }
    };
    stream.map_err(from_cpal)
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

/// The stream's counters. When a duplex stream follows the default device
/// and the default changes, only its output has moved: the input stays on the
/// old device, on another clock, with the old device's latency and workgroup.
/// So the stream reports itself invalidated, and the supervisor reopens both
/// sides on the new default.
fn duplex_stats(counters: &Counters, duplex_follows_default: bool) -> StreamStats {
    let mut stats = counters.snapshot();
    stats.invalidated |= duplex_follows_default && stats.device_changes > 0;
    stats
}

struct CpalStream {
    stream: cpal::Stream,
    /// The device's input stream, when it runs duplex.
    input: Option<cpal::Stream>,
    info: StreamInfo,
    counters: Arc<Counters>,
    workgroup: Option<Workgroup>,
    /// The measured latency, when the platform reports none: the input's and
    /// the output's, each published once.
    measured: Option<(Slot, Slot)>,
}

impl Stream for CpalStream {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn latency(&self) -> Latency {
        match &self.measured {
            Some((input, output)) => Latency {
                input: latency::published(input).unwrap_or(0),
                output: latency::published(output).unwrap_or(0),
            },
            None => Latency {
                input: self.info.input_latency,
                output: self.info.output_latency,
            },
        }
    }

    fn start(&mut self) -> Result<(), Error> {
        self.counters.request_resync();
        if let Some(input) = &self.input {
            input.play().map_err(from_cpal)?;
        }
        self.stream.play().map_err(from_cpal)
    }

    fn stop(&mut self) -> Result<(), Error> {
        let output = self.stream.pause().map_err(from_cpal);
        match &self.input {
            Some(input) => output.and(input.pause().map_err(from_cpal)),
            None => output,
        }
    }

    fn stats(&self) -> StreamStats {
        duplex_stats(
            &self.counters,
            self.input.is_some() && self.info.follows_default,
        )
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

    /// The duplex bridge carries whole frames from the input side to the
    /// output side, and counts the frames neither side has.
    #[test]
    fn the_duplex_bridge_carries_whole_frames_both_ways() {
        let counters = Arc::new(Counters::new());
        let config = RtThreadConfig {
            period: frames_to_duration(4, 48_000),
            promote: false,
            workgroup: None,
        };
        let gauge = Gauge::new(Slot::default(), 48_000);
        let (mut out, mut inp) = duplex_input(2, 4, Arc::clone(&counters), config, gauge);

        // Before the input side has delivered anything, the output plays
        // silence: that's a stream just opened, not an xrun.
        out.drain(2);
        let view = out.scratch.view(2);
        assert!(view.channel(0).iter().all(|&s| s == 0.0));
        assert_eq!(counters.snapshot().xruns, 0);

        // One block of 2 channels, interleaved.
        inp.push(&[1.0f32, 10.0, 2.0, 20.0], 2);
        out.drain(2);
        let view = out.scratch.view(2);
        assert_eq!(view.channel(0), &[1.0, 2.0]);
        assert_eq!(view.channel(1), &[10.0, 20.0]);
        assert_eq!(counters.snapshot().xruns, 0);

        // The output side asks for more than the ring holds: silence, one
        // xrun, and the next block starts on a frame boundary.
        out.drain(2);
        let view = out.scratch.view(2);
        assert!(view.channel(0).iter().all(|&s| s == 0.0));
        assert_eq!(counters.snapshot().xruns, 1);
        inp.push(&[3.0f32, 30.0], 2);
        out.drain(1);
        assert_eq!(out.scratch.view(1).channel(0), &[3.0]);
        assert_eq!(out.scratch.view(1).channel(1), &[30.0]);

        // Pushing more than the ring takes drops whole frames and counts an
        // xrun, rather than splitting a frame across the ring's edge.
        let long: Vec<f32> = (0..64).map(|i| i as f32).collect();
        inp.push(&long, 2);
        assert_eq!(counters.snapshot().xruns, 2);
        out.drain(4);
        let view = out.scratch.view(4);
        assert_eq!(view.channel(0), &[0.0, 2.0, 4.0, 6.0]);
        assert_eq!(view.channel(1), &[1.0, 3.0, 5.0, 7.0]);
    }

    #[test]
    fn a_partial_shortfall_in_the_duplex_bridge_counts_an_xrun() {
        let counters = Arc::new(Counters::new());
        let config = RtThreadConfig {
            period: frames_to_duration(4, 48_000),
            promote: false,
            workgroup: None,
        };
        let gauge = Gauge::new(Slot::default(), 48_000);
        let (mut out, mut inp) = duplex_input(1, 4, Arc::clone(&counters), config, gauge);

        // The ring holds one frame when the output asks for two: the rest is
        // silence, and it's an xrun even though the input has just started.
        inp.push(&[1.0f32], 1);
        out.drain(2);
        assert_eq!(out.scratch.view(2).channel(0), &[1.0, 0.0]);
        assert_eq!(counters.snapshot().xruns, 1);
    }

    #[test]
    fn a_duplex_stream_on_the_default_device_is_invalidated_when_the_default_changes() {
        let counters = Counters::new();
        counters.record_device_change();
        // Only the output follows the default, so the stream reopens.
        assert!(duplex_stats(&counters, true).invalidated);
        // An output alone, or a stream on a chosen device, carries on.
        assert!(!duplex_stats(&counters, false).invalidated);
        assert!(!duplex_stats(&Counters::new(), true).invalidated);
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

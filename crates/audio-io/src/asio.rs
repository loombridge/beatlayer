//! ASIO on Windows through `azo`, which talks to each driver's COM interface
//! directly, so no Steinberg SDK is needed at build time (decision D9).
//!
//! ASIO is full duplex by design: one driver, one callback, input and output
//! buffers side by side. Its callbacks carry no context pointer, so a process
//! can run one ASIO stream at a time, reached through the `ACTIVE` pointer.
//!
//! `azo` runs every driver call on its own COM thread (`azo::utils::Host`) and
//! hands results back through a channel, which allocates and blocks. That's
//! fine for opening, starting, and stopping, but not on the audio thread, so
//! the callback never calls the driver. In particular it doesn't call
//! `ASIOOutputReady`, which is optional: drivers that support it would save
//! one buffer of latency, and calling it needs the driver's interface on the
//! audio thread, which a later package can add.

use std::cell::UnsafeCell;
use std::ffi::{c_long, c_void};
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use azo::driver::{Driver, Metadata, Proxy};
use azo::dto::{BufferSize, ChannelId, Granularity};
use azo::sys::{Bool, Callbacks, MessageSelector, SampleRate, SampleType, Time, TimeInfoFlags};
use azo::utils::Host;
use bl_rt::RtThreadConfig;

use crate::buffer::PlanarScratch;
use crate::runner::{CallbackRunner, frames_to_duration};
use crate::stats::Counters;
use crate::wire::{Encoding, WireFormat};
use crate::{
    AudioCallback, Backend, BackendKind, BufferSizes, DeviceId, DeviceInfo, DeviceSelector, Error,
    ErrorKind, OpenDevice, PortRequest, STANDARD_SAMPLE_RATES, SampleFormat, Stream, StreamInfo,
    StreamRequest, StreamStats,
};

/// The open stream's shared state, or null. ASIO callbacks find it here.
static ACTIVE: AtomicPtr<Shared> = AtomicPtr::new(ptr::null_mut());

/// The open stream's driver as `devices()` lists it, while the stream has it
/// loaded. Behind a lock rather than in [`Shared`], because the app lists
/// devices on its own thread while another can drop the stream.
static OPEN: Mutex<Option<DeviceInfo>> = Mutex::new(None);

/// The rate the open stream's driver last announced, in hertz.
static OPEN_RATE: AtomicU32 = AtomicU32::new(0);

/// The callbacks every driver gets. Static, so they outlive any buffers.
static CALLBACKS: Callbacks = Callbacks {
    buffer_switch,
    sample_rate_did_change,
    asio_message,
    buffer_switch_time_info,
};

/// The ASIO drivers registered on this machine.
pub(crate) struct AsioBackend {
    host: Arc<Host>,
}

impl AsioBackend {
    pub(crate) fn new() -> Self {
        Self { host: Host::new() }
    }

    /// Loads and initialises a driver.
    fn load(&self, driver: &Metadata) -> Result<Proxy, Error> {
        let name = driver.description.to_string_lossy();
        let proxy = self.host.create_driver(driver.clsid).map_err(|e| {
            Error::new(
                ErrorKind::DeviceNotFound,
                format!("couldn't load the {name} ASIO driver: {e}"),
            )
        })?;
        if !proxy.init(None) {
            let why = proxy.last_error().to_string_lossy().into_owned();
            return Err(Error::new(
                ErrorKind::DeviceNotFound,
                format!("the {name} ASIO driver didn't start: {why}"),
            ));
        }
        Ok(proxy)
    }

    fn describe(&self, driver: &Metadata, is_default: bool) -> DeviceInfo {
        // The driver of the open stream is already loaded: a second instance
        // of it can fail, or disturb the running one.
        if let Some(open) = open_listing(&device_id(driver)) {
            return open;
        }
        let mut info = unlisted(driver, is_default);
        // A driver whose hardware is unplugged often won't load; list it anyway.
        if let Ok(proxy) = self.load(driver) {
            list(&mut info, &proxy);
        }
        info
    }
}

/// A driver's entry in the device list before it has been asked anything.
fn unlisted(driver: &Metadata, is_default: bool) -> DeviceInfo {
    DeviceInfo {
        id: device_id(driver),
        name: driver.description.to_string_lossy(),
        is_default_output: is_default,
        is_default_input: is_default,
        output_channels: 0,
        input_channels: 0,
        sample_rates: Vec::new(),
        current_sample_rate: None,
        buffer_sizes: BufferSizes::Unknown,
        default_buffer_size: None,
    }
}

/// Fills in what a loaded driver says about itself.
fn list(info: &mut DeviceInfo, proxy: &Proxy) {
    if let Ok(counts) = proxy.channel_counts() {
        info.output_channels = clamp_channels(counts.out);
        info.input_channels = clamp_channels(counts.in_);
    }
    info.sample_rates = STANDARD_SAMPLE_RATES
        .into_iter()
        .filter(|&rate| proxy.can_sample_rate(f64::from(rate)).is_ok())
        .collect();
    info.current_sample_rate = proxy.get_sample_rate().ok().map(rate_hz);
    if let Ok(sizes) = proxy.buffer_size() {
        info.buffer_sizes = buffer_sizes(&sizes);
        info.default_buffer_size = u32::try_from(sizes.preferred).ok();
    }
}

/// The entry for the driver of the open stream, with the rate the driver last
/// announced, if `id` is that driver.
fn open_listing(id: &DeviceId) -> Option<DeviceInfo> {
    let open = OPEN.lock().unwrap_or_else(PoisonError::into_inner);
    let listing = open.as_ref().filter(|listing| listing.id == *id)?;
    Some(DeviceInfo {
        current_sample_rate: Some(OPEN_RATE.load(Ordering::Relaxed)),
        ..listing.clone()
    })
}

fn drivers() -> Result<Vec<Metadata>, Error> {
    Metadata::enumerate().map_err(|e| {
        Error::new(
            ErrorKind::BackendUnavailable,
            format!("couldn't read the ASIO drivers from the registry: {e}"),
        )
    })
}

fn device_id(driver: &Metadata) -> DeviceId {
    DeviceId::new(BackendKind::Asio, &format!("{:?}", driver.clsid))
}

fn clamp_channels(count: c_long) -> u16 {
    u16::try_from(count.max(0)).unwrap_or(u16::MAX)
}

fn rate_hz(rate: SampleRate) -> u32 {
    rate.round() as u32
}

fn driver_error(what: &str, error: azo::Error) -> Error {
    Error::new(ErrorKind::Backend, format!("{what}: {error}"))
}

/// The buffer sizes a driver accepts, from ASIO's minimum, maximum, and
/// granularity.
fn buffer_sizes(sizes: &BufferSize) -> BufferSizes {
    let (Ok(min), Ok(max)) = (u32::try_from(sizes.min), u32::try_from(sizes.max)) else {
        return BufferSizes::Unknown;
    };
    if min == 0 || min > max {
        return BufferSizes::Unknown;
    }
    match sizes.granularity {
        Some(Granularity::Linear { step }) if step.get() == 1 => BufferSizes::Range { min, max },
        Some(Granularity::Linear { step }) => {
            let step = u32::try_from(step.get()).unwrap_or(max).max(1);
            BufferSizes::List((min..=max).step_by(step as usize).collect())
        }
        Some(Granularity::Exponential) => BufferSizes::List(
            std::iter::successors(Some(min), |size| size.checked_mul(2))
                .take_while(|&size| size <= max)
                .collect(),
        ),
        // No granularity: only the minimum and the maximum.
        None => {
            let mut sizes = vec![min, max];
            sizes.dedup();
            BufferSizes::List(sizes)
        }
    }
}

fn wire_format(sample_type: SampleType) -> Option<WireFormat> {
    let (encoding, big_endian) = match sample_type {
        SampleType::PCM_I16_LSB => (Encoding::I16, false),
        SampleType::PCM_I24_LSB => (Encoding::I24, false),
        SampleType::PCM_I32_LSB => (Encoding::I32, false),
        SampleType::PCM_F32_LSB => (Encoding::F32, false),
        SampleType::PCM_F64_LSB => (Encoding::F64, false),
        SampleType::PCM_I32_LSB_16 => (Encoding::I32Low { bits: 16 }, false),
        SampleType::PCM_I32_LSB_18 => (Encoding::I32Low { bits: 18 }, false),
        SampleType::PCM_I32_LSB_20 => (Encoding::I32Low { bits: 20 }, false),
        SampleType::PCM_I32_LSB_24 => (Encoding::I32Low { bits: 24 }, false),
        SampleType::PCM_I16_MSB => (Encoding::I16, true),
        SampleType::PCM_I24_MSB => (Encoding::I24, true),
        SampleType::PCM_I32_MSB => (Encoding::I32, true),
        SampleType::PCM_F32_MSB => (Encoding::F32, true),
        SampleType::PCM_F64_MSB => (Encoding::F64, true),
        SampleType::PCM_I32_MSB_16 => (Encoding::I32Low { bits: 16 }, true),
        SampleType::PCM_I32_MSB_18 => (Encoding::I32Low { bits: 18 }, true),
        SampleType::PCM_I32_MSB_20 => (Encoding::I32Low { bits: 20 }, true),
        SampleType::PCM_I32_MSB_24 => (Encoding::I32Low { bits: 24 }, true),
        _ => return None,
    };
    Some(WireFormat {
        encoding,
        big_endian,
    })
}

fn sample_format(format: WireFormat) -> SampleFormat {
    match format.encoding {
        Encoding::I16 => SampleFormat::I16,
        Encoding::I24 => SampleFormat::I24,
        Encoding::I32 | Encoding::I32Low { .. } => SampleFormat::I32,
        Encoding::F32 => SampleFormat::F32,
        Encoding::F64 => SampleFormat::F64,
    }
}

/// How many channels a request opens in one direction, or 0 if it doesn't
/// ask for that direction.
fn channels_for(
    port: Option<&PortRequest>,
    available: u16,
    name: &str,
    direction: &str,
) -> Result<u16, Error> {
    let Some(port) = port else {
        return Ok(0);
    };
    let channels = port.channels.unwrap_or(available.min(2));
    if channels == 0 || channels > available {
        return Err(Error::new(
            ErrorKind::UnsupportedConfig,
            format!("{name} has {available} {direction} channels, so it can't open {channels}"),
        ));
    }
    Ok(channels)
}

impl Backend for AsioBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Asio
    }

    fn devices(&self) -> Result<Vec<DeviceInfo>, Error> {
        Ok(drivers()?
            .iter()
            .enumerate()
            .map(|(index, driver)| self.describe(driver, index == 0))
            .collect())
    }

    fn open_stream(
        &self,
        request: &StreamRequest,
        callback: Box<dyn AudioCallback>,
    ) -> Result<Box<dyn Stream>, Error> {
        if request.output.is_none() && request.input.is_none() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "a stream needs output, input, or both",
            ));
        }

        // One driver serves both directions.
        let mut named: Option<&DeviceId> = None;
        let ports = [request.output.as_ref(), request.input.as_ref()];
        for port in ports.into_iter().flatten() {
            if let DeviceSelector::Id(id) = &port.device {
                if named.is_some_and(|other| other != id) {
                    return Err(Error::new(
                        ErrorKind::Unsupported,
                        "ASIO runs one driver at a time, so input and output must use the same one",
                    ));
                }
                named = Some(id);
            }
        }
        let drivers = drivers()?;
        let driver = match named {
            // ASIO has no system default, so the first registered driver stands in.
            None => drivers.first().ok_or_else(|| {
                Error::new(ErrorKind::DeviceNotFound, "no ASIO drivers are installed")
            })?,
            Some(id) => drivers
                .iter()
                .find(|driver| device_id(driver) == *id)
                .ok_or_else(|| {
                    Error::new(ErrorKind::DeviceNotFound, format!("no device `{id}`"))
                })?,
        };
        let name = driver.description.to_string_lossy();
        let proxy = self.load(driver)?;

        if let Some(rate) = request.sample_rate {
            proxy.can_sample_rate(f64::from(rate)).map_err(|_| {
                Error::new(
                    ErrorKind::UnsupportedConfig,
                    format!("{name} doesn't run at {rate} Hz"),
                )
            })?;
            proxy
                .set_sample_rate(f64::from(rate))
                .map_err(|e| driver_error("couldn't set the sample rate", e))?;
        }
        let sample_rate = rate_hz(
            proxy
                .get_sample_rate()
                .map_err(|e| driver_error("couldn't read the sample rate", e))?,
        );
        let mut listing = unlisted(
            driver,
            drivers
                .first()
                .is_some_and(|first| device_id(first) == device_id(driver)),
        );
        list(&mut listing, &proxy);
        let counts = proxy
            .channel_counts()
            .map_err(|e| driver_error("couldn't count channels", e))?;
        let output_channels = channels_for(
            request.output.as_ref(),
            clamp_channels(counts.out),
            &name,
            "output",
        )?;
        let input_channels = channels_for(
            request.input.as_ref(),
            clamp_channels(counts.in_),
            &name,
            "input",
        )?;

        // Drivers often accept only a few sizes, so take the nearest and report it.
        let sizes = proxy
            .buffer_size()
            .map_err(|e| driver_error("couldn't read the buffer sizes", e))?;
        let preferred = u32::try_from(sizes.preferred).unwrap_or(256);
        let frames = request
            .buffer_size
            .and_then(|frames| buffer_sizes(&sizes).nearest(frames))
            .unwrap_or(preferred);

        let channel_ids: Vec<ChannelId> = (0..c_long::from(input_channels))
            .map(|index| ChannelId { input: true, index })
            .chain((0..c_long::from(output_channels)).map(|index| ChannelId {
                input: false,
                index,
            }))
            .collect();
        let mut formats = Vec::with_capacity(channel_ids.len());
        for &channel in &channel_ids {
            let info = proxy
                .channel_info(channel)
                .map_err(|e| driver_error("couldn't read a channel's format", e))?;
            let format = wire_format(info.sample_type).ok_or_else(|| {
                Error::new(
                    ErrorKind::UnsupportedConfig,
                    format!(
                        "{name} uses {} samples, which Beatlayer can't convert",
                        info.sample_type
                    ),
                )
            })?;
            formats.push(format);
        }

        let counters = Arc::new(Counters::new());
        // The driver's thread is set up in its first block, which plays
        // silence: MMCSS "Pro Audio", unless the driver already runs it at
        // THREAD_PRIORITY_TIME_CRITICAL, as most do.
        let thread_config = RtThreadConfig {
            period: frames_to_duration(u64::from(frames), sample_rate),
            promote: true,
            workgroup: None,
        };
        let render = Render {
            runner: CallbackRunner::new(
                callback,
                Arc::clone(&counters),
                sample_rate,
                true,
                thread_config,
            ),
            frames: frames as usize,
            inputs: Vec::new(),
            outputs: Vec::new(),
            input_scratch: PlanarScratch::new(input_channels.into(), frames as usize),
            output_scratch: PlanarScratch::new(output_channels.into(), frames as usize),
            sample_rate,
        };
        let shared = Box::into_raw(Box::new(Shared {
            render: UnsafeCell::new(render),
            counters: Arc::clone(&counters),
            sample_rate: f64::from(sample_rate),
        }));
        if ACTIVE
            .compare_exchange(ptr::null_mut(), shared, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            // SAFETY: `shared` came from `Box::into_raw` above and nothing else has seen it.
            drop(unsafe { Box::from_raw(shared) });
            return Err(Error::new(
                ErrorKind::DeviceBusy,
                "Beatlayer already has an ASIO stream open, and ASIO allows one per process",
            ));
        }

        let device = OpenDevice {
            id: device_id(driver),
            name: name.clone(),
        };
        let device_format = formats.last().copied().unwrap_or(WireFormat {
            encoding: Encoding::F32,
            big_endian: false,
        });
        let info = StreamInfo {
            backend: BackendKind::Asio,
            output_device: (output_channels > 0).then(|| device.clone()),
            input_device: (input_channels > 0).then_some(device),
            follows_default: false,
            sample_rate,
            output_channels,
            input_channels,
            sample_format: sample_format(device_format),
            requested_buffer_size: request.buffer_size,
            buffer_size: Some(frames),
            max_block_frames: frames,
            input_latency: 0,
            output_latency: 0,
            workgroup: None,
        };
        // From here, dropping `stream` releases the slot and the buffers.
        let mut stream = AsioStream {
            proxy,
            shared,
            buffers_created: false,
            info,
            counters,
        };
        OPEN_RATE.store(sample_rate, Ordering::Relaxed);
        *OPEN.lock().unwrap_or_else(PoisonError::into_inner) = Some(listing);

        // SAFETY: `CALLBACKS` is static, so it outlives the buffers.
        let halves = unsafe {
            stream
                .proxy
                .create_buffers(channel_ids, frames as c_long, &raw const CALLBACKS)
        }
        .map_err(|e| driver_error(&format!("{name} couldn't create its buffers"), e))?;
        stream.buffers_created = true;

        // The latencies are valid once the buffers exist. A driver that
        // refuses to say counts as 0, and what it says holds the buffer.
        if let Ok(latencies) = stream.proxy.latencies() {
            stream.info.input_latency = latency_frames(latencies.in_);
            stream.info.output_latency = latency_frames(latencies.out);
        }

        // SAFETY: the driver only calls back after `start`, so nothing else
        // is using the render state yet.
        let render = unsafe { &mut *(*stream.shared).render.get() };
        for (index, (halves, format)) in halves.zip(formats).enumerate() {
            let port = Port { format, halves };
            if index < usize::from(input_channels) {
                render.inputs.push(port);
            } else {
                render.outputs.push(port);
            }
        }
        Ok(Box::new(stream))
    }
}

/// A latency the driver reports in samples, which can't be negative.
fn latency_frames(reported: c_long) -> u32 {
    u32::try_from(reported).unwrap_or(0)
}

/// One channel's double buffer, owned by the driver.
struct Port {
    format: WireFormat,
    halves: [*mut c_void; 2],
}

/// What the callbacks reach through [`ACTIVE`].
struct Shared {
    /// Touched only by the buffer-switch callbacks, which the driver makes one
    /// at a time.
    render: UnsafeCell<Render>,
    /// Lock-free, so any callback on any thread can count events.
    counters: Arc<Counters>,
    sample_rate: f64,
}

struct Render {
    runner: CallbackRunner,
    frames: usize,
    inputs: Vec<Port>,
    outputs: Vec<Port>,
    input_scratch: PlanarScratch,
    output_scratch: PlanarScratch,
    sample_rate: u32,
}

impl Render {
    /// Handles one buffer switch: reads input from half `half` of each input
    /// channel, runs the block, and writes output to the same half of each
    /// output channel.
    fn render(&mut self, half: usize, position: Option<u64>) {
        let frames = self.frames;
        self.runner.device_callback(frames);
        let mut input = self.input_scratch.view_mut(frames);
        for (port, channel) in self.inputs.iter().zip(input.iter_mut()) {
            let source = port.halves[half].cast::<u8>().cast_const();
            if source.is_null() {
                continue;
            }
            // SAFETY: the driver's buffer for this half holds `frames`
            // samples in `port.format`, and the driver doesn't write it
            // during the buffer switch.
            let bytes = unsafe {
                std::slice::from_raw_parts(source, frames * port.format.bytes_per_sample())
            };
            port.format.decode(bytes, channel);
        }

        let timestamp = position.map(|frames| frames_to_duration(frames, self.sample_rate));
        let input = self.input_scratch.view(frames);
        let mut output = self.output_scratch.view_mut(frames);
        self.runner.run(timestamp, &input, &mut output);

        for (port, channel) in self.outputs.iter().zip(output.as_buffer().iter()) {
            let target = port.halves[half].cast::<u8>();
            if target.is_null() {
                continue;
            }
            // SAFETY: as above; the driver doesn't read this half until the
            // buffer switch returns.
            let bytes = unsafe {
                std::slice::from_raw_parts_mut(target, frames * port.format.bytes_per_sample())
            };
            port.format.encode(channel, bytes);
        }
    }
}

/// Runs `f` on the open stream's shared state, if a stream is open.
fn with_shared(f: impl FnOnce(&Shared)) {
    let shared = ACTIVE.load(Ordering::Acquire);
    if !shared.is_null() {
        // SAFETY: the stream clears `ACTIVE` only after stopping the driver
        // and disposing of its buffers, and frees `shared` after that, so a
        // pointer read from `ACTIVE` during a callback is live.
        f(unsafe { &*shared });
    }
}

fn switch_buffers(index: c_long, position: Option<u64>) {
    with_shared(|shared| {
        // SAFETY: the driver makes one buffer switch at a time, and only
        // buffer switches touch `render` while the stream runs.
        let render = unsafe { &mut *shared.render.get() };
        render.render(usize::from(index != 0), position);
    });
}

extern "system" fn buffer_switch(index: c_long, _direct_process: Bool) {
    switch_buffers(index, None);
}

extern "system" fn buffer_switch_time_info(
    params: *mut Time,
    index: c_long,
    _direct_process: Bool,
) -> *mut Time {
    // SAFETY: the driver passes a valid `Time` or null.
    let time = unsafe { params.as_ref() };
    let position = time
        .filter(|time| {
            time.time_info
                .flags
                .contains(TimeInfoFlags::SAMPLE_POSITION_VALID)
        })
        .and_then(|time| u64::try_from(i64::from(time.time_info.sample_position)).ok());
    switch_buffers(index, position);
    params
}

extern "system" fn sample_rate_did_change(rate: SampleRate) {
    OPEN_RATE.store(rate_hz(rate), Ordering::Relaxed);
    with_shared(|shared| {
        if (rate - shared.sample_rate).abs() >= 1.0 {
            shared.counters.record_invalidated();
        }
    });
}

extern "system" fn asio_message(
    selector: MessageSelector,
    value: c_long,
    _message: *const c_void,
    _opt: *const f64,
) -> c_long {
    let answer = |yes: bool| if yes { Bool::TRUE.0 } else { Bool::FALSE.0 };
    match selector {
        MessageSelector::SELECTOR_SUPPORTED => answer(matches!(
            MessageSelector(value),
            MessageSelector::ENGINE_VERSION
                | MessageSelector::RESET_REQUEST
                | MessageSelector::RESYNC_REQUEST
                | MessageSelector::LATENCIES_CHANGED
                | MessageSelector::SUPPORTS_TIME_INFO
                | MessageSelector::OVERLOAD
        )),
        MessageSelector::ENGINE_VERSION => 2,
        MessageSelector::RESET_REQUEST => {
            with_shared(|shared| shared.counters.record_invalidated());
            answer(true)
        }
        MessageSelector::RESYNC_REQUEST | MessageSelector::OVERLOAD => {
            with_shared(|shared| shared.counters.record_xrun());
            answer(true)
        }
        MessageSelector::LATENCIES_CHANGED | MessageSelector::SUPPORTS_TIME_INFO => answer(true),
        // Includes buffer-size changes, which the driver then makes through a
        // reset request.
        _ => answer(false),
    }
}

struct AsioStream {
    proxy: Proxy,
    shared: *mut Shared,
    buffers_created: bool,
    info: StreamInfo,
    counters: Arc<Counters>,
}

impl Stream for AsioStream {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn start(&mut self) -> Result<(), Error> {
        self.counters.request_resync();
        self.proxy
            .start()
            .map_err(|e| driver_error("the ASIO driver didn't start", e))
    }

    fn stop(&mut self) -> Result<(), Error> {
        self.proxy
            .stop()
            .map_err(|e| driver_error("the ASIO driver didn't stop", e))
    }

    fn stats(&self) -> StreamStats {
        self.counters.snapshot()
    }

    fn current_buffer_size(&self) -> Option<u32> {
        self.info.buffer_size
    }
}

impl Drop for AsioStream {
    fn drop(&mut self) {
        let _ = self.proxy.stop();
        if self.buffers_created {
            let _ = self.proxy.dispose_buffers();
        }
        let _ = ACTIVE.compare_exchange(
            self.shared,
            ptr::null_mut(),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        *OPEN.lock().unwrap_or_else(PoisonError::into_inner) = None;
        // SAFETY: the driver has stopped and released its buffers, so no
        // callback reaches `shared` any more, and `ACTIVE` no longer points
        // at it. It came from `Box::into_raw` in `open_stream`.
        drop(unsafe { Box::from_raw(self.shared) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroI32;

    fn sizes(min: i32, max: i32, granularity: Option<Granularity>) -> BufferSize {
        BufferSize {
            min,
            max,
            preferred: min,
            granularity,
        }
    }

    #[test]
    fn granularity_becomes_a_list_of_sizes() {
        let step = |n| {
            Some(Granularity::Linear {
                step: NonZeroI32::new(n).unwrap(),
            })
        };
        assert_eq!(
            buffer_sizes(&sizes(32, 2048, step(1))),
            BufferSizes::Range { min: 32, max: 2048 }
        );
        assert_eq!(
            buffer_sizes(&sizes(64, 256, step(64))),
            BufferSizes::List(vec![64, 128, 192, 256])
        );
        assert_eq!(
            buffer_sizes(&sizes(64, 512, Some(Granularity::Exponential))),
            BufferSizes::List(vec![64, 128, 256, 512])
        );
        assert_eq!(
            buffer_sizes(&sizes(256, 256, None)),
            BufferSizes::List(vec![256])
        );
        assert_eq!(buffer_sizes(&sizes(0, 256, None)), BufferSizes::Unknown);
    }

    #[test]
    fn the_open_driver_is_listed_without_loading_it_again() {
        let id = DeviceId::new(BackendKind::Asio, "open");
        let other = DeviceId::new(BackendKind::Asio, "other");
        assert!(open_listing(&id).is_none());

        let listing = DeviceInfo {
            id: id.clone(),
            name: "Open".to_owned(),
            is_default_output: true,
            is_default_input: true,
            output_channels: 2,
            input_channels: 2,
            sample_rates: vec![44_100, 48_000],
            current_sample_rate: Some(48_000),
            buffer_sizes: BufferSizes::Unknown,
            default_buffer_size: None,
        };
        OPEN_RATE.store(48_000, Ordering::Relaxed);
        *OPEN.lock().unwrap() = Some(listing);

        // The driver announces a new rate: its entry follows, with no load.
        sample_rate_did_change(44_100.0);
        let open = open_listing(&id).unwrap();
        assert_eq!(open.current_sample_rate, Some(44_100));
        assert_eq!(open.output_channels, 2);
        assert!(open_listing(&other).is_none());

        *OPEN.lock().unwrap() = None;
        assert!(open_listing(&id).is_none());
    }

    #[test]
    fn a_drivers_latency_is_its_frames_and_never_negative() {
        assert_eq!(latency_frames(480), 480);
        assert_eq!(latency_frames(0), 0);
        assert_eq!(latency_frames(-1), 0);
    }

    #[test]
    fn every_pcm_sample_type_has_a_wire_format() {
        assert!(wire_format(SampleType::PCM_F32_LSB).is_some());
        assert!(wire_format(SampleType::PCM_I32_MSB_24).is_some());
        assert!(wire_format(SampleType::DSD_I8_LSB_1).is_none());
    }
}

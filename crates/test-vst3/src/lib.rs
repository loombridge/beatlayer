//! A VST3 plug-in built from source for the plug-in host's tests (WP-2.7).
//! It's never shipped; the tests load its bundle by path.
//!
//! "Test Gain Echo" (`bl-test-vst3`) is an audio effect that also takes
//! notes, so one plug-in exercises everything the host does:
//!
//! - **Audio.** One stereo bus in and out: each sample is its input times
//!   the gain, as of the frame of the last parameter change before it,
//!   delayed by the latency it reported when it was activated.
//! - **Parameters in.** The host sends the gain, the latency and the last
//!   note through the process block's `IParameterChanges`, so a change lands
//!   on its exact frame (the gain), or takes effect at the plug-in's next
//!   activation (the latency).
//! - **Parameters out.** The controller performs a gesture on [`LAST_NOTE`]
//!   — `beginEdit`, two `performEdit`s, `endEdit` — when a test hands it a
//!   [`gesture_state`], standing in for its editor; and a change of
//!   [`LATENCY`] asks the host for a restart, as its editor would.
//! - **Latency.** It reports [`LATENCY`]'s value, as of its last activation,
//!   and delays its audio by exactly that.
//! - **State.** The component saves its gain and latency, the controller its
//!   gain, latency and last note; two states make it fail on purpose:
//!   [`FAIL_PROCESS`] makes every process call fail, and [`FAIL_ACTIVATE`]
//!   its next activation.
//! - **Notes in and out.** A note-on adds its velocity to the output on its
//!   frame (an impulse), and every note-on and note-off goes back out an
//!   octave up on the same frame, through the block's `IEventList`.
//!
//! - **MIDI mapping.** The controller maps control change 1 on channel 1 and
//!   the pitch bend on channel 3 to the gain, as `IMidiMapping`, so the host
//!   can send a channel message as a parameter change.
//!
//! - **Note expression.** Every note-on, note-off, note expression value,
//!   polyphonic pressure and parameter change it receives is logged, with
//!   its frame (counted from the plug-in's activation), to the file
//!   [`LOG_VAR`] names; each note expression value and polyphonic pressure
//!   goes back out as it came. With [`EXPRESSIONS_VAR`] set its controller
//!   implements `INoteExpressionController` (tuning, brightness and, by
//!   default, a custom pressure) and `INoteExpressionPhysicalUIMapping`
//!   (the key's Y movement drives brightness, its pressure the custom
//!   type); with [`MAPPING_VAR`] set its `IMidiMapping` maps each channel's
//!   pitch bend, control change 74 and channel pressure to parameters of
//!   their own.
//!
//! The editor itself is the next part's (WP-2.10).

use std::cell::{Cell, UnsafeCell};
use std::ffi::{CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::ptr;
use std::slice;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};

use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComRef, ComWrapper, uid};

/// The plug-in's name, as its classes carry it.
pub const PLUGIN_NAME: &str = "Test Gain Echo";
/// The gain parameter's ID. A linear factor from 0 to 2, 1 by default.
pub const GAIN: u32 = 0;
/// The latency parameter's ID: the frames it reports and delays by, 0 to
/// [`MAX_LATENCY`], stepped. A change asks the host for a restart.
pub const LATENCY: u32 = 1;
/// The last-note parameter's ID: the key of the last note the controller
/// was told, 0 to 127, stepped. The controller sets it itself, in a
/// gesture, as its editor would.
pub const LAST_NOTE: u32 = 2;
/// The gain's plain range: 0 to this, 1 by default.
pub const GAIN_MAX: f64 = 2.0;
/// The most frames the plug-in's latency can be, and the length of its
/// delay line.
pub const MAX_LATENCY: usize = 4096;
/// How far the echoed notes are transposed, in semitones.
pub const ECHO_TRANSPOSE: i16 = 12;
/// What a saved component state starts with.
pub const COMPONENT_MAGIC: &[u8; 4] = b"BLTC";
/// What a saved controller state starts with.
pub const CONTROLLER_MAGIC: &[u8; 4] = b"BLTU";
/// A component state that makes every process call fail.
pub const FAIL_PROCESS: &[u8] = b"FAIL:PROCESS";
/// A component state that makes the next activation fail.
pub const FAIL_ACTIVATE: &[u8] = b"FAIL:ACTIVATE";
/// What a controller state that makes the controller perform one gesture
/// on [`LAST_NOTE`] starts with: its two performed values follow.
pub const GESTURE: &[u8] = b"GESTURE";

const NO_FAILURE: u8 = 0;
const PROCESS_FAILS: u8 = 1;
const ACTIVATE_FAILS: u8 = 2;

/// How many channels the plug-in delays, at most.
const MAX_CHANNELS: usize = 2;

/// The environment variable a test sets to the file the plug-in logs what it
/// receives to, one line each: `frame=<n> <on|off|expression:<type>|
/// pressure|param:<id>> channel=<c> key=<k> id=<note ID, -1 for none>
/// value=<v>`. Unset for no log.
pub const LOG_VAR: &str = "BL_TEST_VST3_LOG";

/// The environment variable a test sets to `on` so the controller
/// implements note expression, or to `no-pressure` so its physical mapping
/// leaves the key's pressure unmapped. Unset for a controller without it.
pub const EXPRESSIONS_VAR: &str = "BL_TEST_VST3_EXPRESSIONS";

/// The environment variable a test sets (to anything) so the controller's
/// MIDI mapping maps channels 2 to 16's pitch bend, control change 74 and
/// channel pressure to [`mapped_param`]s.
pub const MAPPING_VAR: &str = "BL_TEST_VST3_MAPPING";

/// The note expression type the controller gives pressure.
pub const PRESSURE_TYPE: u32 = 100_000;
/// The note expression types it gives tuning and brightness: VST3's own.
pub const TUNING_TYPE: u32 = 2;
pub const BRIGHTNESS_TYPE: u32 = 5;

/// The parameter the MIDI mapping gives `channel`'s (0 to 15) pitch bend
/// (`what` 0), control change 74 (1) or channel pressure (2), when
/// [`MAPPING_VAR`] is set.
#[must_use]
pub const fn mapped_param(channel: u32, what: u32) -> u32 {
    100 + channel * 3 + what
}

/// The environment variable a test sets so a scan crashes (`crash`) or hangs
/// (`hang`) on purpose, as `bl-test-clap` does for CLAP. Only the scanner's
/// tests set it.
pub const TEST_SCAN_VAR: &str = "BL_TEST_VST3_SCAN";

/// How many parameter queues and points one block's changes hold; past that
/// the rest of the block is dropped, as it is in the host.
const MAX_QUEUES: usize = 8;
const MAX_POINTS: usize = 64;

/// A controller state that performs one gesture on [`LAST_NOTE`], beginning
/// with `first` and settling on `second`: what the host's undo step should
/// carry. The tests hand it to the host as the controller's part of a
/// state.
#[must_use]
pub fn gesture_state(first: u8, second: u8) -> Vec<u8> {
    let mut bytes = GESTURE.to_vec();
    bytes.push(first);
    bytes.push(second);
    bytes
}

fn copy_cstring(src: &str, dst: &mut [c_char]) {
    let c_string = CString::new(src).unwrap_or_default();
    let bytes = c_string.as_bytes_with_nul();
    for (src, dst) in bytes.iter().zip(dst.iter_mut()) {
        *dst = *src as c_char;
    }
    if bytes.len() > dst.len()
        && let Some(last) = dst.last_mut()
    {
        *last = 0;
    }
}

fn copy_wstring(src: &str, dst: &mut [TChar]) {
    let mut len = 0;
    for (src, dst) in src.encode_utf16().zip(dst.iter_mut()) {
        *dst = src;
        len += 1;
    }
    if len < dst.len() {
        dst[len] = 0;
    } else if let Some(last) = dst.last_mut() {
        *last = 0;
    }
}

/// The 16 bytes of a class ID as the host writes them: uppercase hex with
/// dashes, as the VST3 SDK's `FUID::toString` does.
#[must_use]
pub fn format_fuid(tuid: &TUID) -> String {
    let bytes: Vec<u8> = tuid.iter().map(|b| *b as u8).collect();
    let hex = |range: &[u8]| range.iter().map(|b| format!("{b:02X}")).collect::<String>();
    format!(
        "{}-{}-{}-{}-{}",
        hex(&bytes[0..4]),
        hex(&bytes[4..6]),
        hex(&bytes[6..8]),
        hex(&bytes[8..10]),
        hex(&bytes[10..16])
    )
}

/// The component's class ID as a string, which is the host's device kind.
#[must_use]
pub fn plugin_id() -> String {
    format_fuid(&Processor::CID)
}

/// One of the plug-in's parameters' plain range.
fn plain_range(id: u32) -> Option<(f64, f64)> {
    match id {
        GAIN => Some((0.0, GAIN_MAX)),
        LATENCY => Some((0.0, MAX_LATENCY as f64)),
        LAST_NOTE => Some((0.0, 127.0)),
        _ => None,
    }
}

/// A normalized value as the plain value the parameter means: whole numbers
/// for the stepped ones.
fn to_plain(id: u32, value_normalized: f64) -> f64 {
    let Some((min, max)) = plain_range(id) else {
        return value_normalized;
    };
    let plain = min + value_normalized.clamp(0.0, 1.0) * (max - min);
    match id {
        LATENCY | LAST_NOTE => plain.round(),
        _ => plain,
    }
}

/// A plain value as its normalized value over the parameter's range.
fn to_normalized(id: u32, plain: f64) -> f64 {
    let Some((min, max)) = plain_range(id) else {
        return plain;
    };
    if max <= min {
        return 0.0;
    }
    ((plain.clamp(min, max) - min) / (max - min)).clamp(0.0, 1.0)
}

/// Every byte the host's stream holds, as a state.
fn read_state(state: *mut IBStream) -> Vec<u8> {
    // SAFETY: the host passes a live stream for the call.
    let Some(stream) = (unsafe { ComRef::from_raw(state) }) else {
        return Vec::new();
    };
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 256];
    loop {
        let mut read = 0i32;
        // SAFETY: a live stream, read into a buffer that lives through the
        // call.
        let result = unsafe {
            stream.read(
                chunk.as_mut_ptr().cast::<c_void>(),
                i32::try_from(chunk.len()).unwrap_or(i32::MAX),
                &mut read,
            )
        };
        if result != kResultOk || read <= 0 {
            break;
        }
        let end = usize::try_from(read).unwrap_or(0).min(chunk.len());
        bytes.extend_from_slice(&chunk[..end]);
    }
    bytes
}

/// Writes `bytes` into the host's stream. False if the stream refused.
fn write_state(state: *mut IBStream, bytes: &[u8]) -> bool {
    // SAFETY: the host passes a live stream for the call.
    let Some(stream) = (unsafe { ComRef::from_raw(state) }) else {
        return false;
    };
    let mut written = 0usize;
    while written < bytes.len() {
        let rest = &bytes[written..];
        let mut wrote = 0i32;
        // SAFETY: a live stream, written from a buffer that lives through
        // the call.
        let result = unsafe {
            stream.write(
                rest.as_ptr().cast_mut().cast::<c_void>(),
                i32::try_from(rest.len()).unwrap_or(i32::MAX),
                &mut wrote,
            )
        };
        if result != kResultOk || wrote <= 0 {
            return false;
        }
        written += usize::try_from(wrote).unwrap_or(0).min(rest.len());
    }
    true
}

/// The `f64` a state's bytes hold at `range`, little-endian.
fn f64_at(bytes: &[u8], range: core::ops::Range<usize>) -> Option<f64> {
    Some(f64::from_le_bytes(bytes.get(range)?.try_into().ok()?))
}

/// The `u32` a state's bytes hold at `range`, little-endian.
fn u32_at(bytes: &[u8], range: core::ops::Range<usize>) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(range)?.try_into().ok()?))
}

/// The processor: the component and its audio processing. The gain and the
/// latency are what the process block's parameter changes set; the delay
/// line is as long as the reported latency, and is preallocated, so no
/// block allocates.
struct Processor {
    gain: AtomicU64,
    latency: AtomicU32,
    /// The latency in force since the last activation, which its delay
    /// line and `getLatencySamples` follow.
    active_latency: AtomicU32,
    failure: AtomicU8,
    /// The delay ring's write cursor, shared by its channels.
    at: Cell<usize>,
    /// Frames processed, for the log.
    frames: AtomicU64,
    /// One ring of [`MAX_LATENCY`] frames per channel.
    delay: UnsafeCell<Vec<f32>>,
}

// SAFETY: the fields are atomics and `Cell`s the host's threads reach
// through VST3's COM contract: the delay ring and its cursor are touched
// only by the process call, on one thread at a time, and the state calls
// touch only the atomics.
unsafe impl Send for Processor {}
// SAFETY: as above.
unsafe impl Sync for Processor {}

impl Processor {
    const CID: TUID = uid(0x6E332252, 0x54224B00, 0xAA69301A, 0xF318797E);

    fn new() -> Self {
        Self {
            gain: AtomicU64::new(1.0f64.to_bits()),
            latency: AtomicU32::new(0),
            active_latency: AtomicU32::new(0),
            failure: AtomicU8::new(NO_FAILURE),
            at: Cell::new(0),
            frames: AtomicU64::new(0),
            delay: UnsafeCell::new(vec![0.0; MAX_CHANNELS * MAX_LATENCY]),
        }
    }

    fn gain(&self) -> f32 {
        f64::from_bits(self.gain.load(Ordering::Relaxed)) as f32
    }

    fn set_gain_normalized(&self, value: f64) {
        let plain = (value * GAIN_MAX).clamp(0.0, GAIN_MAX);
        self.gain.store(plain.to_bits(), Ordering::Relaxed);
    }

    fn set_latency_normalized(&self, value: f64) {
        let frames = to_plain(LATENCY, value).clamp(0.0, MAX_LATENCY as f64);
        self.latency.store(frames as u32, Ordering::Relaxed);
    }

    /// Appends a line to the log, if there is one: `at` frames into the
    /// block that is being processed.
    fn log(&self, at: i32, what: &str, channel: i32, key: i32, id: i32, value: f64) {
        use std::io::Write;
        let Some(path) = std::env::var_os(LOG_VAR) else {
            return;
        };
        let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        else {
            return;
        };
        let _ = writeln!(
            file,
            "frame={} {what} channel={channel} key={key} id={id} value={value}",
            self.frames.load(Ordering::Relaxed) + u64::try_from(at.max(0)).unwrap_or(0),
        );
    }

    /// One frame through the delay ring, at the latency in force.
    fn delayed(&self, channel: usize, input: f32) -> f32 {
        let frames = self.active_latency.load(Ordering::Relaxed) as usize;
        let channel = channel.min(MAX_CHANNELS - 1);
        if frames == 0 || frames > MAX_LATENCY {
            return input;
        }
        let at = self.at.get();
        let read = (at + MAX_LATENCY - frames) % MAX_LATENCY;
        // SAFETY: one process call at a time touches the ring, and the
        // cursor's frame is each channel's own slot.
        unsafe {
            let ring = &mut *self.delay.get();
            let held = ring[channel * MAX_LATENCY + read];
            ring[channel * MAX_LATENCY + at] = input;
            held
        }
    }
}

impl Class for Processor {
    type Interfaces = (IComponent, IAudioProcessor);
}

impl IPluginBaseTrait for Processor {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }

    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IComponentTrait for Processor {
    unsafe fn getControllerClassId(&self, class_id: *mut TUID) -> tresult {
        // SAFETY: the host passes a valid pointer for the call.
        unsafe { *class_id = Controller::<false>::CID };
        kResultOk
    }

    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kNotImplemented
    }

    unsafe fn getBusCount(&self, media_type: MediaType, dir: BusDirection) -> i32 {
        match media_type as MediaTypes {
            MediaTypes_::kAudio => match dir as BusDirections {
                BusDirections_::kInput | BusDirections_::kOutput => 1,
                _ => 0,
            },
            MediaTypes_::kEvent => match dir as BusDirections {
                BusDirections_::kInput | BusDirections_::kOutput => 1,
                _ => 0,
            },
            _ => 0,
        }
    }

    unsafe fn getBusInfo(
        &self,
        media_type: MediaType,
        dir: BusDirection,
        index: i32,
        bus: *mut BusInfo,
    ) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        if !matches!(
            dir as BusDirections,
            BusDirections_::kInput | BusDirections_::kOutput
        ) {
            return kInvalidArgument;
        }
        // SAFETY: the host passes a valid pointer for the call.
        let bus = unsafe { &mut *bus };
        bus.direction = dir;
        bus.busType = BusTypes_::kMain as BusType;
        // The bindings' default enum type is signed on Windows and unsigned
        // elsewhere; the field is always `uint32`.
        bus.flags = BusInfo_::BusFlags_::kDefaultActive as uint32;
        match media_type as MediaTypes {
            MediaTypes_::kAudio => {
                bus.mediaType = MediaTypes_::kAudio as MediaType;
                bus.channelCount = 2;
                copy_wstring(
                    if dir as BusDirections == BusDirections_::kInput {
                        "Input"
                    } else {
                        "Output"
                    },
                    &mut bus.name,
                );
                kResultOk
            }
            MediaTypes_::kEvent => {
                bus.mediaType = MediaTypes_::kEvent as MediaType;
                bus.channelCount = 1;
                copy_wstring(
                    if dir as BusDirections == BusDirections_::kInput {
                        "Event In"
                    } else {
                        "Event Out"
                    },
                    &mut bus.name,
                );
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn getRoutingInfo(
        &self,
        _in_info: *mut RoutingInfo,
        _out_info: *mut RoutingInfo,
    ) -> tresult {
        kNotImplemented
    }

    unsafe fn activateBus(
        &self,
        _media_type: MediaType,
        _dir: BusDirection,
        _index: i32,
        _state: TBool,
    ) -> tresult {
        kResultOk
    }

    unsafe fn setActive(&self, _state: TBool) -> tresult {
        kResultOk
    }

    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        let bytes = read_state(state);
        if bytes == FAIL_PROCESS {
            self.failure.store(PROCESS_FAILS, Ordering::Relaxed);
            return kResultOk;
        }
        if bytes == FAIL_ACTIVATE {
            self.failure.store(ACTIVATE_FAILS, Ordering::Relaxed);
            return kResultOk;
        }
        // An empty chunk carries no state, which is what a test's gesture
        // state has for its component part.
        if bytes.is_empty() {
            self.failure.store(NO_FAILURE, Ordering::Relaxed);
            return kResultOk;
        }
        let (Some(gain), Some(latency), true) = (
            f64_at(&bytes, 4..12),
            u32_at(&bytes, 12..16),
            bytes.get(..4) == Some(COMPONENT_MAGIC.as_slice()),
        ) else {
            return kResultFalse;
        };
        self.gain
            .store(gain.clamp(0.0, GAIN_MAX).to_bits(), Ordering::Relaxed);
        self.latency.store(
            latency.min(u32::try_from(MAX_LATENCY).unwrap_or(u32::MAX)),
            Ordering::Relaxed,
        );
        self.failure.store(NO_FAILURE, Ordering::Relaxed);
        kResultOk
    }

    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        let mut bytes = Vec::with_capacity(16);
        bytes.extend_from_slice(COMPONENT_MAGIC);
        bytes.extend_from_slice(&f64::from_bits(self.gain.load(Ordering::Relaxed)).to_le_bytes());
        bytes.extend_from_slice(&self.latency.load(Ordering::Relaxed).to_le_bytes());
        if write_state(state, &bytes) {
            kResultOk
        } else {
            kResultFalse
        }
    }
}

impl IAudioProcessorTrait for Processor {
    unsafe fn setBusArrangements(
        &self,
        inputs: *mut SpeakerArrangement,
        num_ins: i32,
        outputs: *mut SpeakerArrangement,
        num_outs: i32,
    ) -> tresult {
        if num_ins != 1 || num_outs != 1 {
            return kResultFalse;
        }
        // SAFETY: the host passes valid pointers for the call.
        let (inputs, outputs) = unsafe { (*inputs, *outputs) };
        if inputs == SpeakerArr::kStereo && outputs == SpeakerArr::kStereo {
            kResultTrue
        } else {
            kResultFalse
        }
    }

    unsafe fn getBusArrangement(
        &self,
        dir: BusDirection,
        index: i32,
        arr: *mut SpeakerArrangement,
    ) -> tresult {
        match dir as BusDirections {
            BusDirections_::kInput | BusDirections_::kOutput if index == 0 => {
                // SAFETY: the host passes a valid pointer for the call.
                unsafe { *arr = SpeakerArr::kStereo };
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn canProcessSampleSize(&self, symbolic_sample_size: i32) -> tresult {
        match symbolic_sample_size as SymbolicSampleSizes {
            SymbolicSampleSizes_::kSample32 => kResultOk,
            SymbolicSampleSizes_::kSample64 => kNotImplemented,
            _ => kInvalidArgument,
        }
    }

    unsafe fn getLatencySamples(&self) -> u32 {
        self.active_latency.load(Ordering::Relaxed)
    }

    unsafe fn setupProcessing(&self, _setup: *mut ProcessSetup) -> tresult {
        kResultOk
    }

    unsafe fn setProcessing(&self, state: TBool) -> tresult {
        if state == 0 {
            return kResultOk;
        }
        if self.failure.load(Ordering::Relaxed) == ACTIVATE_FAILS {
            return kResultFalse;
        }
        // The latency in force, which the host reads next and compensates
        // for, is the one this activation starts with.
        let frames = self
            .latency
            .load(Ordering::Relaxed)
            .min(u32::try_from(MAX_LATENCY).unwrap_or(u32::MAX));
        self.active_latency.store(frames, Ordering::Relaxed);
        kResultOk
    }

    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        if self.failure.load(Ordering::Relaxed) == PROCESS_FAILS {
            return kResultFalse;
        }
        // SAFETY: the host passes a valid ProcessData for this call, with
        // its buses and event lists valid for the duration of it.
        let data = unsafe { &*data };
        let frames = data.numSamples.max(0) as usize;
        if frames == 0 || data.numInputs < 1 || data.numOutputs < 1 {
            return kResultOk;
        }
        // SAFETY: numInputs/numOutputs bus buffers, as above.
        let (inputs, outputs) = unsafe {
            (
                slice::from_raw_parts(data.inputs, data.numInputs as usize),
                slice::from_raw_parts(data.outputs, data.numOutputs as usize),
            )
        };
        let (input, output) = (&inputs[0], &outputs[0]);
        if input.numChannels <= 0 || output.numChannels <= 0 {
            return kResultOk;
        }
        // SAFETY: each bus's channel buffers, as above.
        let (in_channels, out_channels) = unsafe {
            (
                slice::from_raw_parts(input.__field0.channelBuffers32, input.numChannels as usize),
                slice::from_raw_parts(
                    output.__field0.channelBuffers32,
                    output.numChannels as usize,
                ),
            )
        };
        let pairs = in_channels.len().min(out_channels.len());

        // The parameter changes: each queue's points, in offset order. Each
        // stretch of the block is rendered at the gain in force, then the
        // changes at its end are applied, so a change lands on its frame.
        // SAFETY: the host passes a valid parameter changes object for
        // the call, or null.
        let changes = unsafe { ComRef::from_raw(data.inputParameterChanges) };
        let mut queues = 0usize;
        let mut ids = [0u32; MAX_QUEUES];
        let mut counts = [0i32; MAX_QUEUES];
        let mut read = [0i32; MAX_QUEUES];
        let mut pointers: [*mut IParamValueQueue; MAX_QUEUES] = [ptr::null_mut(); MAX_QUEUES];
        if let Some(changes) = changes {
            // SAFETY: the host's parameter changes object for this call.
            let count = unsafe { changes.getParameterCount() }.clamp(0, MAX_QUEUES as i32);
            for index in 0..count {
                // SAFETY: an index the object just reported.
                let pointer = unsafe { changes.getParameterData(index) };
                // SAFETY: a queue pointer the object just returned.
                if let Some(queue) = unsafe { ComRef::from_raw(pointer) } {
                    // SAFETY: the queue object as above.
                    ids[queues] = unsafe { queue.getParameterId() };
                    // SAFETY: the queue object as above.
                    counts[queues] = unsafe { queue.getPointCount() }.clamp(0, MAX_POINTS as i32);
                    pointers[queues] = pointer;
                    queues += 1;
                }
            }
        }
        /// The queue at `index`, in `pointers`.
        fn queue(
            pointers: &[*mut IParamValueQueue; MAX_QUEUES],
            index: usize,
        ) -> Option<ComRef<'_, IParamValueQueue>> {
            // SAFETY: the pointer came from the host's changes object.
            unsafe { ComRef::from_raw(pointers[index]) }
        }
        let mut start = 0usize;
        while start < frames {
            // The next change among all queues, or the block's end.
            let mut next = frames as i32;
            for index in 0..queues {
                if read[index] >= counts[index] {
                    continue;
                }
                let Some(queue) = queue(&pointers, index) else {
                    read[index] = counts[index];
                    continue;
                };
                let (mut offset, mut value) = (0i32, 0f64);
                // SAFETY: a point index within what the queue reported.
                if unsafe { queue.getPoint(read[index], &mut offset, &mut value) } == kResultTrue {
                    next = next.min(offset.max(0));
                } else {
                    read[index] = counts[index];
                }
            }
            let end = (next.max(start as i32) as usize).min(frames);
            if end > start {
                let gain = self.gain();
                for pair in 0..pairs {
                    // SAFETY: the bus's channels, each `frames` long, as above.
                    let (input, output) = unsafe {
                        (
                            slice::from_raw_parts(in_channels[pair], frames),
                            slice::from_raw_parts_mut(out_channels[pair], frames),
                        )
                    };
                    for frame in start..end {
                        let held = self.delayed(pair, input[frame]);
                        output[frame] = held * gain;
                        self.at.set((self.at.get() + 1) % MAX_LATENCY);
                    }
                }
            }
            // Apply every change at or before this stretch's end.
            for index in 0..queues {
                loop {
                    if read[index] >= counts[index] {
                        break;
                    }
                    let Some(queue) = queue(&pointers, index) else {
                        break;
                    };
                    let (mut offset, mut value) = (0i32, 0f64);
                    // SAFETY: as in the scan above.
                    if unsafe { queue.getPoint(read[index], &mut offset, &mut value) }
                        != kResultTrue
                    {
                        read[index] = counts[index];
                        break;
                    }
                    if offset.max(0) as usize > end {
                        break;
                    }
                    read[index] += 1;
                    if !value.is_finite() {
                        continue;
                    }
                    self.log(offset, &format!("param:{}", ids[index]), -1, -1, -1, value);
                    match ids[index] {
                        GAIN => self.set_gain_normalized(value),
                        LATENCY => self.set_latency_normalized(value),
                        _ => {}
                    }
                }
            }
            if end >= frames {
                break;
            }
            start = end;
        }

        // The notes: a note-on's impulse on its frame, and every note echoed
        // an octave up on its frame.
        // SAFETY: the host passes valid event lists for the call, or null.
        let events = unsafe { ComRef::from_raw(data.inputEvents) };
        // SAFETY: as above.
        let out = unsafe { ComRef::from_raw(data.outputEvents) };
        if let Some(events) = events {
            // SAFETY: the host's event list for this call.
            let count = unsafe { events.getEventCount() }.max(0);
            for index in 0..count {
                // SAFETY: zeroed is a valid Event (plain old data); the
                // list fills it.
                let mut event: Event = unsafe { std::mem::zeroed() };
                // SAFETY: an index the list just reported.
                if unsafe { events.getEvent(index, &mut event) } != kResultTrue {
                    continue;
                }
                let offset = event.sampleOffset.max(0) as usize;
                // SAFETY: the event's type says which union member it holds.
                let echoed = unsafe {
                    match event.r#type as i32 {
                        x if x == Event_::EventTypes_::kNoteOnEvent as int32 => {
                            let note = event.__field0.noteOn;
                            self.log(
                                event.sampleOffset,
                                "on",
                                i32::from(note.channel),
                                i32::from(note.pitch),
                                note.noteId,
                                f64::from(note.velocity),
                            );
                            if offset < frames {
                                for channel in out_channels.iter().take(pairs) {
                                    // SAFETY: the channel is `frames` long.
                                    *(*channel).add(offset) += note.velocity;
                                }
                            }
                            Some(Event {
                                busIndex: 0,
                                sampleOffset: event.sampleOffset,
                                ppqPosition: 0.0,
                                flags: 0,
                                r#type: Event_::EventTypes_::kNoteOnEvent as u16,
                                __field0: Event__type0 {
                                    noteOn: NoteOnEvent {
                                        channel: note.channel,
                                        pitch: (note.pitch + ECHO_TRANSPOSE).min(127),
                                        tuning: 0.0,
                                        velocity: note.velocity,
                                        length: 0,
                                        noteId: note.noteId,
                                    },
                                },
                            })
                        }
                        x if x == Event_::EventTypes_::kNoteExpressionValueEvent as int32 => {
                            let value = event.__field0.noteExpressionValue;
                            self.log(
                                event.sampleOffset,
                                &format!("expression:{}", value.typeId),
                                -1,
                                -1,
                                value.noteId,
                                value.value,
                            );
                            Some(event)
                        }
                        x if x == Event_::EventTypes_::kPolyPressureEvent as int32 => {
                            let pressure = event.__field0.polyPressure;
                            self.log(
                                event.sampleOffset,
                                "pressure",
                                i32::from(pressure.channel),
                                i32::from(pressure.pitch),
                                pressure.noteId,
                                f64::from(pressure.pressure),
                            );
                            Some(event)
                        }
                        x if x == Event_::EventTypes_::kNoteOffEvent as int32 => {
                            let note = event.__field0.noteOff;
                            self.log(
                                event.sampleOffset,
                                "off",
                                i32::from(note.channel),
                                i32::from(note.pitch),
                                note.noteId,
                                f64::from(note.velocity),
                            );
                            Some(Event {
                                busIndex: 0,
                                sampleOffset: event.sampleOffset,
                                ppqPosition: 0.0,
                                flags: 0,
                                r#type: Event_::EventTypes_::kNoteOffEvent as u16,
                                __field0: Event__type0 {
                                    noteOff: NoteOffEvent {
                                        channel: note.channel,
                                        pitch: (note.pitch + ECHO_TRANSPOSE).min(127),
                                        velocity: note.velocity,
                                        noteId: note.noteId,
                                        tuning: 0.0,
                                    },
                                },
                            })
                        }
                        _ => None,
                    }
                };
                if let (Some(echoed), Some(out)) = (echoed, out) {
                    let mut echoed = echoed;
                    // SAFETY: the host's output list for this call.
                    unsafe { out.addEvent(&mut echoed) };
                }
            }
        }
        self.frames.fetch_add(frames as u64, Ordering::Relaxed);
        kResultOk
    }

    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
}

/// The controller: the plug-in's parameters as a person or an agent sees
/// them, and the gestures a change makes, as its editor's would. Its values
/// are plain; VST3's are normalized.
struct Controller<const EXPRESSIONS: bool> {
    gain: Cell<f64>,
    latency: Cell<f64>,
    last_note: Cell<f64>,
    /// The host's component handler, which it set and keeps alive for as
    /// long as this controller lives.
    handler: Cell<*mut IComponentHandler>,
}

impl<const EXPRESSIONS: bool> Controller<EXPRESSIONS> {
    const CID: TUID = uid(0x1BA8A477, 0xEE0A4B2D, 0x80F50D14, 0x13D2EAA1);

    fn new() -> Self {
        Self {
            gain: Cell::new(1.0),
            latency: Cell::new(0.0),
            last_note: Cell::new(0.0),
            handler: Cell::new(ptr::null_mut()),
        }
    }

    fn plain(&self, id: u32) -> Option<f64> {
        match id {
            GAIN => Some(self.gain.get()),
            LATENCY => Some(self.latency.get()),
            LAST_NOTE => Some(self.last_note.get()),
            _ => None,
        }
    }

    fn set_plain(&self, id: u32, plain: f64) {
        match id {
            GAIN => self.gain.set(plain.clamp(0.0, GAIN_MAX)),
            LATENCY => self.latency.set(plain.clamp(0.0, MAX_LATENCY as f64)),
            LAST_NOTE => self.last_note.set(plain.clamp(0.0, 127.0)),
            _ => {}
        }
    }

    /// Calls the host's handler, if the host set one.
    fn call(&self, f: impl FnOnce(ComRef<'_, IComponentHandler>) -> tresult) {
        let raw = self.handler.get();
        if raw.is_null() {
            return;
        }
        // SAFETY: the host set this handler and keeps it alive while the
        // controller lives.
        if let Some(handler) = unsafe { ComRef::from_raw(raw) } {
            let _ = f(handler);
        }
    }

    /// One gesture on `id`, as the plug-in's editor would make it: a begin,
    /// the performed values, and an end.
    fn gesture(&self, id: u32, values: &[f64]) {
        self.call(|handler| {
            // SAFETY: the handler is a live object of the interface.
            unsafe { handler.beginEdit(id) }
        });
        for value in values {
            let normalized = to_normalized(id, *value);
            self.call(|handler| {
                // SAFETY: as above.
                unsafe { handler.performEdit(id, normalized) }
            });
            self.set_plain(id, *value);
        }
        self.call(|handler| {
            // SAFETY: as above.
            unsafe { handler.endEdit(id) }
        });
    }

    /// Asks the host to restart the plug-in, as an editor would when its
    /// latency changes.
    fn ask_restart(&self, flags: int32) {
        self.call(|handler| {
            // SAFETY: the handler is a live object of the interface.
            unsafe { handler.restartComponent(flags) }
        });
    }
}

impl Class for Controller<false> {
    type Interfaces = (IEditController, IMidiMapping);
}

impl Class for Controller<true> {
    type Interfaces = (
        IEditController,
        IMidiMapping,
        INoteExpressionController,
        INoteExpressionPhysicalUIMapping,
    );
}

/// Whether the pressure type is on offer: [`EXPRESSIONS_VAR`] isn't
/// `no-pressure`.
fn has_pressure() -> bool {
    std::env::var(EXPRESSIONS_VAR).as_deref() != Ok("no-pressure")
}

impl INoteExpressionControllerTrait for Controller<true> {
    unsafe fn getNoteExpressionCount(&self, bus: int32, _channel: int16) -> int32 {
        match (bus, has_pressure()) {
            (0, true) => 3,
            (0, false) => 2,
            _ => 0,
        }
    }

    unsafe fn getNoteExpressionInfo(
        &self,
        bus: int32,
        _channel: int16,
        index: int32,
        info: *mut NoteExpressionTypeInfo,
    ) -> tresult {
        let (id, title, bipolar) = match (bus, index) {
            (0, 0) => (TUNING_TYPE, "Tuning", true),
            (0, 1) => (BRIGHTNESS_TYPE, "Brightness", false),
            (0, 2) if has_pressure() => (PRESSURE_TYPE, "Pressure", false),
            _ => return kInvalidArgument,
        };
        // SAFETY: the host passes a valid pointer for the call.
        let info = unsafe { &mut *info };
        info.typeId = id;
        copy_wstring(title, &mut info.title);
        copy_wstring(title, &mut info.shortTitle);
        copy_wstring("", &mut info.units);
        info.unitId = -1;
        info.valueDesc = NoteExpressionValueDescription {
            defaultValue: if bipolar { 0.5 } else { 0.0 },
            minimum: 0.0,
            maximum: 1.0,
            stepCount: 0,
        };
        info.associatedParameterId = u32::MAX;
        info.flags = if bipolar {
            NoteExpressionTypeInfo_::NoteExpressionTypeFlags_::kIsBipolar as int32
        } else {
            0
        };
        kResultOk
    }

    unsafe fn getNoteExpressionStringByValue(
        &self,
        _bus: int32,
        _channel: int16,
        _id: NoteExpressionTypeID,
        _value: NoteExpressionValue,
        _string: *mut String128,
    ) -> tresult {
        kNotImplemented
    }

    unsafe fn getNoteExpressionValueByString(
        &self,
        _bus: int32,
        _channel: int16,
        _id: NoteExpressionTypeID,
        _string: *const TChar,
        _value: *mut NoteExpressionValue,
    ) -> tresult {
        kNotImplemented
    }
}

impl INoteExpressionPhysicalUIMappingTrait for Controller<true> {
    unsafe fn getPhysicalUIMapping(
        &self,
        bus: int32,
        _channel: int16,
        list: *mut PhysicalUIMapList,
    ) -> tresult {
        if bus != 0 || list.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: the host passes a list of `count` maps it owns for the
        // call.
        let list = unsafe { &*list };
        for index in 0..list.count as usize {
            // SAFETY: an index within the list's count.
            let map = unsafe { &mut *list.map.add(index) };
            map.noteExpressionTypeID = match map.physicalUITypeID {
                PhysicalUITypeIDs_::kPUIYMovement => BRIGHTNESS_TYPE,
                PhysicalUITypeIDs_::kPUIPressure if has_pressure() => PRESSURE_TYPE,
                _ => NoteExpressionTypeIDs_::kInvalidTypeID,
            };
        }
        kResultOk
    }
}

impl<const EXPRESSIONS: bool> IMidiMappingTrait for Controller<EXPRESSIONS> {
    unsafe fn getMidiControllerAssignment(
        &self,
        bus: int32,
        channel: int16,
        controller: CtrlNumber,
        id: *mut ParamID,
    ) -> tresult {
        // Control change 1 on channel 1, and the pitch bend (129) on
        // channel 3.
        let mut mapped = bus == 0
            && matches!((channel, controller), (0, 1) | (2, 129))
            && std::env::var_os(MAPPING_VAR).is_none();
        let mut param = GAIN;
        // Channels 2 to 16 give their pitch bend, control change 74 and
        // channel pressure parameters of their own, when a test asks.
        if !mapped
            && bus == 0
            && (1..16).contains(&channel)
            && std::env::var_os(MAPPING_VAR).is_some()
            && let Some(what) = match controller {
                129 => Some(0),
                74 => Some(1),
                128 => Some(2),
                _ => None,
            }
        {
            mapped = true;
            param = mapped_param(channel as u32, what);
        }
        if mapped && !id.is_null() {
            // SAFETY: the host passes a valid out pointer.
            unsafe { *id = param };
        }
        if mapped { kResultOk } else { kResultFalse }
    }
}

impl<const EXPRESSIONS: bool> IPluginBaseTrait for Controller<EXPRESSIONS> {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }

    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl<const EXPRESSIONS: bool> IEditControllerTrait for Controller<EXPRESSIONS> {
    unsafe fn setComponentState(&self, state: *mut IBStream) -> tresult {
        let bytes = read_state(state);
        // The host told it "the component's state is none": nothing to follow.
        if bytes.is_empty() {
            return kResultOk;
        }
        let (Some(gain), Some(latency), true) = (
            f64_at(&bytes, 4..12),
            u32_at(&bytes, 12..16),
            bytes.get(..4) == Some(COMPONENT_MAGIC.as_slice()),
        ) else {
            return kResultFalse;
        };
        self.gain.set(gain.clamp(0.0, GAIN_MAX));
        self.latency.set(f64::from(latency));
        kResultOk
    }

    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        let bytes = read_state(state);
        if bytes.len() == GESTURE.len() + 2 && bytes.starts_with(GESTURE) {
            // The tests' stand-in for its editor: one gesture on Last Note
            // that begins with the first value and ends with the second,
            // which the host's undo step should carry.
            let first = f64::from(bytes[GESTURE.len()]);
            let second = f64::from(bytes[GESTURE.len() + 1]);
            self.gesture(LAST_NOTE, &[first, second]);
            return kResultOk;
        }
        if bytes.is_empty() {
            return kResultOk;
        }
        let (Some(gain), Some(latency), Some(last), true) = (
            f64_at(&bytes, 4..12),
            u32_at(&bytes, 12..16),
            f64_at(&bytes, 16..24),
            bytes.get(..4) == Some(CONTROLLER_MAGIC.as_slice()),
        ) else {
            return kResultFalse;
        };
        self.gain.set(gain.clamp(0.0, GAIN_MAX));
        self.latency.set(f64::from(latency));
        self.last_note.set(last.clamp(0.0, 127.0));
        kResultOk
    }

    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        let mut bytes = Vec::with_capacity(24);
        bytes.extend_from_slice(CONTROLLER_MAGIC);
        bytes.extend_from_slice(&self.gain.get().to_le_bytes());
        bytes.extend_from_slice(
            &u32::try_from(self.latency.get().clamp(0.0, MAX_LATENCY as f64) as usize)
                .unwrap_or(0)
                .to_le_bytes(),
        );
        bytes.extend_from_slice(&self.last_note.get().to_le_bytes());
        if write_state(state, &bytes) {
            kResultOk
        } else {
            kResultFalse
        }
    }

    unsafe fn getParameterCount(&self) -> i32 {
        3
    }

    unsafe fn getParameterInfo(&self, param_index: i32, info: *mut ParameterInfo) -> tresult {
        let (id, title, steps, default): (u32, &str, i32, f64) = match param_index {
            0 => (GAIN, "Gain", 0, 1.0),
            1 => (LATENCY, "Latency", 4096, 0.0),
            2 => (LAST_NOTE, "Last Note", 127, 0.0),
            _ => return kInvalidArgument,
        };
        // SAFETY: the host passes a valid pointer for the call.
        let info = unsafe { &mut *info };
        info.id = id;
        copy_wstring(title, &mut info.title);
        copy_wstring(title, &mut info.shortTitle);
        copy_wstring("", &mut info.units);
        info.stepCount = steps;
        info.defaultNormalizedValue = to_normalized(id, default);
        info.unitId = 0;
        info.flags = ParameterInfo_::ParameterFlags_::kCanAutomate;
        kResultOk
    }

    unsafe fn getParamStringByValue(
        &self,
        id: ParamID,
        value_normalized: ParamValue,
        string: *mut String128,
    ) -> tresult {
        let Some(plain) = plain_range(id).map(|_| to_plain(id, value_normalized)) else {
            return kInvalidArgument;
        };
        // SAFETY: the host passes a valid pointer for the call.
        let string = unsafe { &mut *string };
        let text = match id {
            LATENCY | LAST_NOTE => format!("{plain:.0}"),
            _ => format!("{plain:.2}"),
        };
        copy_wstring(&text, string);
        kResultOk
    }

    unsafe fn getParamValueByString(
        &self,
        id: ParamID,
        string: *mut TChar,
        value_normalized: *mut ParamValue,
    ) -> tresult {
        if plain_range(id).is_none() {
            return kInvalidArgument;
        }
        let mut len = 0isize;
        // SAFETY: the host passes a NUL-terminated string for the call; it
        // is scanned up to its terminator.
        unsafe {
            while *string.offset(len) != 0 {
                len += 1;
            }
        }
        // SAFETY: the string's units, as measured above.
        let text = unsafe { slice::from_raw_parts(string, len as usize) };
        let Ok(text) = String::from_utf16(text) else {
            return kInvalidArgument;
        };
        let Ok(value) = text.trim().parse::<f64>() else {
            return kInvalidArgument;
        };
        // SAFETY: the host passes a valid pointer for the call.
        unsafe { *value_normalized = to_normalized(id, value) };
        kResultOk
    }

    unsafe fn normalizedParamToPlain(
        &self,
        id: ParamID,
        value_normalized: ParamValue,
    ) -> ParamValue {
        to_plain(id, value_normalized)
    }

    unsafe fn plainParamToNormalized(&self, id: ParamID, plain_value: ParamValue) -> ParamValue {
        to_normalized(id, plain_value)
    }

    unsafe fn getParamNormalized(&self, id: ParamID) -> ParamValue {
        self.plain(id).map_or(0.0, |plain| to_normalized(id, plain))
    }

    unsafe fn setParamNormalized(&self, id: ParamID, value: ParamValue) -> tresult {
        let plain = to_plain(id, value);
        let Some(before) = self.plain(id) else {
            return kInvalidArgument;
        };
        self.set_plain(id, plain);
        if id == LATENCY && plain != before {
            // As its editor would: the latency changed, so ask the host to
            // deactivate and activate the plug-in again.
            self.ask_restart(RestartFlags_::kLatencyChanged as int32);
        }
        kResultOk
    }

    unsafe fn setComponentHandler(&self, handler: *mut IComponentHandler) -> tresult {
        self.handler.set(handler);
        kResultOk
    }

    unsafe fn createView(&self, _name: *const c_char) -> *mut IPlugView {
        ptr::null_mut()
    }
}

/// The factory: one processor class and one controller class.
struct Factory;

impl Class for Factory {
    type Interfaces = (IPluginFactory2,);
}

impl IPluginFactoryTrait for Factory {
    unsafe fn getFactoryInfo(&self, info: *mut PFactoryInfo) -> tresult {
        // SAFETY: the host passes a valid pointer for the call.
        let info = unsafe { &mut *info };
        copy_cstring("Beatlayer tests", &mut info.vendor);
        copy_cstring("https://github.com/loombridge/beatlayer", &mut info.url);
        copy_cstring("code@loombridge.io", &mut info.email);
        info.flags = PFactoryInfo_::FactoryFlags_::kUnicode as int32;
        kResultOk
    }

    unsafe fn countClasses(&self) -> i32 {
        2
    }

    unsafe fn getClassInfo(&self, index: i32, info: *mut PClassInfo) -> tresult {
        // SAFETY: the host passes a valid pointer for the call.
        let info = unsafe { &mut *info };
        let (cid, category) = match index {
            0 => (Processor::CID, "Audio Module Class"),
            1 => (Controller::<false>::CID, "Component Controller Class"),
            _ => return kInvalidArgument,
        };
        info.cid = cid;
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy_cstring(category, &mut info.category);
        copy_cstring(PLUGIN_NAME, &mut info.name);
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        cid: FIDString,
        iid: FIDString,
        obj: *mut *mut c_void,
    ) -> tresult {
        // SAFETY: the host passes valid pointers for the call.
        let class = unsafe { *(cid as *const TUID) };
        let instance: Option<ComPtr<FUnknown>> = if class == Processor::CID {
            ComWrapper::new(Processor::new()).to_com_ptr::<FUnknown>()
        } else if class == Controller::<false>::CID {
            if std::env::var_os(EXPRESSIONS_VAR).is_some() {
                ComWrapper::new(Controller::<true>::new()).to_com_ptr::<FUnknown>()
            } else {
                ComWrapper::new(Controller::<false>::new()).to_com_ptr::<FUnknown>()
            }
        } else {
            None
        };
        let Some(unknown) = instance else {
            return kInvalidArgument;
        };
        let pointer = unknown.as_ptr();
        // SAFETY: the object implements FUnknown, and the host passes a
        // valid out pointer here.
        unsafe { ((*(*pointer).vtbl).queryInterface)(pointer, iid as *mut TUID, obj) }
    }
}

impl IPluginFactory2Trait for Factory {
    unsafe fn getClassInfo2(&self, index: i32, info: *mut PClassInfo2) -> tresult {
        // SAFETY: the host passes a valid pointer for the call.
        let info = unsafe { &mut *info };
        // SAFETY: the delegation forwards the host's call.
        if unsafe {
            IPluginFactoryTrait::getClassInfo(
                self,
                index,
                info as *mut PClassInfo2 as *mut PClassInfo,
            )
        } != kResultOk
        {
            return kInvalidArgument;
        }
        info.classFlags = 0;
        copy_cstring("Fx", &mut info.subCategories);
        copy_cstring("Beatlayer tests", &mut info.vendor);
        copy_cstring("1.0.0", &mut info.version);
        copy_cstring("VST 3.7.0", &mut info.sdkVersion);
        kResultOk
    }
}

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
pub extern "system" fn InitDll() -> bool {
    true
}

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
pub extern "system" fn ExitDll() -> bool {
    true
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub extern "system" fn bundleEntry(_bundle_ref: *mut c_void) -> bool {
    true
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub extern "system" fn bundleExit() -> bool {
    true
}

#[cfg(target_os = "linux")]
#[unsafe(no_mangle)]
pub extern "system" fn ModuleEntry(_library_handle: *mut c_void) -> bool {
    true
}

#[cfg(target_os = "linux")]
#[unsafe(no_mangle)]
pub extern "system" fn ModuleExit() -> bool {
    true
}

/// The module's entry point: the host finds it by this name in the loaded
/// binary and gets a new factory from it.
#[unsafe(no_mangle)]
pub extern "system" fn GetPluginFactory() -> *mut IPluginFactory {
    // Spec: plugin-scanning R5, as `bl-test-clap` does for CLAP. A scan can
    // be made to crash or hang on purpose, so the scanner's child process
    // isolation and timeout are tested with a real VST3 plug-in: the test
    // sets `BL_TEST_VST3_SCAN`.
    match std::env::var(TEST_SCAN_VAR).as_deref() {
        Ok("crash") => std::process::abort(),
        Ok("hang") => loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        },
        _ => {}
    }
    let wrapper = ComWrapper::new(Factory);
    match wrapper.to_com_ptr::<IPluginFactory>() {
        Some(factory) => factory.into_raw(),
        None => ptr::null_mut(),
    }
}

/// For the host's tests: the test plug-in as a VST3 bundle, made once per
/// test run from the cdylib Cargo built beside the running test binary:
/// on macOS a `.vst3` bundle with its `Info.plist` and its binary under
/// `Contents/MacOS`, on Linux under `Contents/x86_64-linux`, and on Windows
/// under `Contents/x86_64-win`, as the VST3 specification lays them out.
#[must_use]
pub fn bundle() -> &'static Path {
    static BUNDLE: OnceLock<PathBuf> = OnceLock::new();
    BUNDLE.get_or_init(|| {
        let library = library();
        let dir = std::env::temp_dir().join(format!("beatlayer-test-vst3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary folder");
        let bundle = dir.join("Test Gain Echo.vst3");
        let contents = bundle.join("Contents");
        let (folder, name) = if cfg!(target_os = "macos") {
            ("MacOS".to_owned(), "Test Gain Echo".to_owned())
        } else if cfg!(target_os = "windows") {
            ("x86_64-win".to_owned(), "Test Gain Echo.vst3".to_owned())
        } else {
            (
                "x86_64-linux".to_owned(),
                format!("{}bl_test_vst3.so", std::env::consts::DLL_PREFIX),
            )
        };
        let binary = contents.join(&folder);
        std::fs::create_dir_all(&binary).expect("the bundle's folders");
        std::fs::copy(&library, binary.join(name)).expect("the library");
        if cfg!(target_os = "macos") {
            std::fs::write(
                contents.join("Info.plist"),
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Test Gain Echo</string>
<key>CFBundleIdentifier</key><string>bl.test.vst3.gain-echo</string>
<key>CFBundlePackageType</key><string>BNDL</string>
</dict></plist>
"#,
            )
            .expect("the bundle's Info.plist");
        }
        bundle
    })
}

/// The newest test plug-in library Cargo built beside the running test
/// binary.
fn library() -> PathBuf {
    let exe = std::env::current_exe().expect("the test binary's path");
    let deps = exe.parent().expect("the test binary's folder");
    let prefix = format!("{}bl_test_vst3", std::env::consts::DLL_PREFIX);
    let suffix = std::env::consts::DLL_SUFFIX;
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = [deps, deps.parent().unwrap_or(deps)]
        .iter()
        .flat_map(|dir| std::fs::read_dir(dir).into_iter().flatten().flatten())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix) && n.ends_with(suffix))
        })
        .filter_map(|path| Some((path.metadata().ok()?.modified().ok()?, path)))
        .collect();
    found.sort();
    found
        .pop()
        .map(|(_, path)| path)
        .expect("Cargo builds the test plug-in's cdylib beside the tests")
}

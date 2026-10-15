//! A VST3 plug-in built from source for the plug-in host's tests (WP-2.7).
//! It's never shipped; the tests load its bundle by path.
//!
//! "Test Gain Echo" (`bl-test-vst3`) is an audio effect that also takes
//! notes, so one plug-in exercises everything the host does in part 1:
//!
//! - **Audio.** One stereo bus in and out: each sample is its input times
//!   the gain, as of the frame of the last parameter change before it.
//! - **Parameters in.** The host sends the gain through the process block's
//!   `IParameterChanges`, so a change lands on its exact frame: the gain is
//!   a normalized parameter of the controller, whose plain range is 0 to 2
//!   (1 by default).
//! - **Notes in and out.** A note-on adds its velocity to the output on its
//!   frame (an impulse), and every note-on and note-off goes back out an
//!   octave up on the same frame, through the block's `IEventList`.
//!
//! State, latency and an editor are the next part's (WP-2.7, T65).

use std::cell::Cell;
use std::ffi::{CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::ptr;
use std::slice;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComRef, ComWrapper, uid};

/// The plug-in's name, as its classes carry it.
pub const PLUGIN_NAME: &str = "Test Gain Echo";
/// The gain parameter's ID.
pub const GAIN: u32 = 0;
/// The gain's plain range: 0 to this, 1 by default.
pub const GAIN_MAX: f64 = 2.0;
/// How far the echoed notes are transposed, in semitones.
pub const ECHO_TRANSPOSE: i16 = 12;

/// How many parameter queues and points one block's changes hold; past that
/// the rest of the block is dropped, as it is in the host.
const MAX_QUEUES: usize = 8;
const MAX_POINTS: usize = 64;

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

/// The processor: the component and its audio processing. The gain is what
/// the process block's parameter changes set.
struct Processor {
    gain: AtomicU64,
}

impl Processor {
    const CID: TUID = uid(0x6E332252, 0x54224B00, 0xAA69301A, 0xF318797E);

    fn new() -> Self {
        Self {
            gain: AtomicU64::new(1.0f64.to_bits()),
        }
    }

    fn gain(&self) -> f32 {
        f64::from_bits(self.gain.load(Ordering::Relaxed)) as f32
    }

    fn set_gain_normalized(&self, value: f64) {
        let plain = (value * GAIN_MAX).clamp(0.0, GAIN_MAX);
        self.gain.store(plain.to_bits(), Ordering::Relaxed);
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
        unsafe { *class_id = Controller::CID };
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
        bus.flags = BusInfo_::BusFlags_::kDefaultActive;
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

    unsafe fn setState(&self, _state: *mut IBStream) -> tresult {
        kNotImplemented
    }

    unsafe fn getState(&self, _state: *mut IBStream) -> tresult {
        kNotImplemented
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
        0
    }

    unsafe fn setupProcessing(&self, _setup: *mut ProcessSetup) -> tresult {
        kResultOk
    }

    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }

    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
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
                        output[frame] = input[frame] * gain;
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
                    if ids[index] == GAIN && value.is_finite() {
                        self.set_gain_normalized(value);
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
                                        noteId: -1,
                                    },
                                },
                            })
                        }
                        x if x == Event_::EventTypes_::kNoteOffEvent as int32 => {
                            let note = event.__field0.noteOff;
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
                                        noteId: -1,
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
        kResultOk
    }

    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
}

/// The controller: the gain parameter as a person or an agent sees it. Its
/// value is normalized, as VST3 parameters are; the plain value is 0 to 2.
struct Controller {
    gain: Cell<f64>,
}

impl Controller {
    const CID: TUID = uid(0x1BA8A477, 0xEE0A4B2D, 0x80F50D14, 0x13D2EAA1);

    fn new() -> Self {
        Self {
            gain: Cell::new(0.5),
        }
    }
}

impl Class for Controller {
    type Interfaces = (IEditController,);
}

impl IPluginBaseTrait for Controller {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }

    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IEditControllerTrait for Controller {
    unsafe fn setComponentState(&self, _state: *mut IBStream) -> tresult {
        kNotImplemented
    }

    unsafe fn setState(&self, _state: *mut IBStream) -> tresult {
        kNotImplemented
    }

    unsafe fn getState(&self, _state: *mut IBStream) -> tresult {
        kNotImplemented
    }

    unsafe fn getParameterCount(&self) -> i32 {
        1
    }

    unsafe fn getParameterInfo(&self, param_index: i32, info: *mut ParameterInfo) -> tresult {
        if param_index != 0 {
            return kInvalidArgument;
        }
        // SAFETY: the host passes a valid pointer for the call.
        let info = unsafe { &mut *info };
        info.id = GAIN;
        copy_wstring("Gain", &mut info.title);
        copy_wstring("Gain", &mut info.shortTitle);
        copy_wstring("", &mut info.units);
        info.stepCount = 0;
        info.defaultNormalizedValue = 0.5;
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
        if id != GAIN {
            return kInvalidArgument;
        }
        // SAFETY: the host passes a valid pointer for the call.
        let string = unsafe { &mut *string };
        copy_wstring(
            &format!("{:.2}", (value_normalized * GAIN_MAX).clamp(0.0, GAIN_MAX)),
            string,
        );
        kResultOk
    }

    unsafe fn getParamValueByString(
        &self,
        id: ParamID,
        string: *mut TChar,
        value_normalized: *mut ParamValue,
    ) -> tresult {
        if id != GAIN {
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
        unsafe { *value_normalized = (value / GAIN_MAX).clamp(0.0, 1.0) };
        kResultOk
    }

    unsafe fn normalizedParamToPlain(
        &self,
        id: ParamID,
        value_normalized: ParamValue,
    ) -> ParamValue {
        if id == GAIN {
            value_normalized * GAIN_MAX
        } else {
            value_normalized
        }
    }

    unsafe fn plainParamToNormalized(&self, id: ParamID, plain_value: ParamValue) -> ParamValue {
        if id == GAIN {
            (plain_value / GAIN_MAX).clamp(0.0, 1.0)
        } else {
            plain_value
        }
    }

    unsafe fn getParamNormalized(&self, id: ParamID) -> ParamValue {
        if id == GAIN { self.gain.get() } else { 0.0 }
    }

    unsafe fn setParamNormalized(&self, id: ParamID, value: ParamValue) -> tresult {
        if id != GAIN {
            return kInvalidArgument;
        }
        self.gain.set(value.clamp(0.0, 1.0));
        kResultOk
    }

    unsafe fn setComponentHandler(&self, _handler: *mut IComponentHandler) -> tresult {
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
            1 => (Controller::CID, "Component Controller Class"),
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
        } else if class == Controller::CID {
            ComWrapper::new(Controller::new()).to_com_ptr::<FUnknown>()
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

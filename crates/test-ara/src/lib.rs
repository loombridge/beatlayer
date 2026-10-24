//! An ARA 2 VST3 plug-in built from source for the plug-in host's tests
//! (IMP-20; the ARA hosting spec). It's never shipped; the tests load its
//! bundle by path.
//!
//! "Test ARA Player" (`bl-test-ara`) is a stereo effect that, once the host
//! binds it to a document controller, plays the playback regions of its
//! renderer from the audio it read through the host's audio reader, at the
//! place and speed the regions say (a stretched region by nearest sample),
//! and nothing else: its input is dropped, as a real ARA playback renderer
//! does. It also keeps what a real plug-in keeps and a test can look at:
//!
//! - **Rules.** It counts every use of ARA that breaks its rules: a region
//!   added to or taken from a renderer while the plug-in is active, an
//!   object destroyed while something still uses it.
//! - **Edits.** Each audio modification counts the times its properties were
//!   updated, which stands in for the edits a real plug-in keeps; its archive
//!   holds the counts, so a test can tell a modification that came back from
//!   one made again, and an archive that was restored from one that wasn't.
//! - **Selections.** Its editor view logs what it was told is selected.
//!
//! The component state is a text report of all of it ([`report_of`] reads it
//! back from the state the host saved).
//!
//! Its one class is both component and controller, with no parameters and no
//! editor.

mod bundle;
mod document;
mod entry;
mod report;

use std::cell::UnsafeCell;
use std::ffi::{CString, c_char, c_void};
use std::mem::size_of;
use std::ptr;
use std::slice;
use std::sync::Arc;
use std::sync::atomic::Ordering::{Relaxed, Release};
use std::sync::atomic::{AtomicBool, AtomicU64};

use bl_ara_sys as ara;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComRef, ComWrapper, uid};

pub use bundle::bundle;
use document::Extension;
pub use report::{ModificationReport, REQUEST, Report, report_of};

use entry::{
    IPlugInEntryPoint, IPlugInEntryPoint2, IPlugInEntryPoint2Trait, IPlugInEntryPointTrait,
};

/// The plug-in's name, as its class carries it.
pub const PLUGIN_NAME: &str = "Test ARA Player";
/// The plug-in's ARA factory ID.
pub const FACTORY_ID: &str = "io.loombridge.beatlayer.test-ara";

/// The ARA factory. Its pointers are static strings and functions.
struct SyncFactory(ara::ARAFactory);

// SAFETY: the factory is read-only, and its pointers are to statics.
unsafe impl Sync for SyncFactory {}

static FACTORY: SyncFactory = SyncFactory(ara::ARAFactory {
    structSize: size_of::<ara::ARAFactory>(),
    lowestSupportedApiGeneration: ara::kARAAPIGeneration_2_0_Final,
    highestSupportedApiGeneration: ara::kARAAPIGeneration_2_X_Draft,
    factoryID: document::FACTORY_ID.as_ptr(),
    initializeARAWithConfiguration: Some(initialize_ara),
    uninitializeARA: Some(uninitialize_ara),
    plugInName: c"Test ARA Player".as_ptr(),
    manufacturerName: c"Beatlayer tests".as_ptr(),
    informationURL: c"https://github.com/loombridge/beatlayer".as_ptr(),
    version: c"1.0.0".as_ptr(),
    createDocumentControllerWithDocument: Some(document::create_document_controller),
    documentArchiveID: document::ARCHIVE_ID.as_ptr(),
    compatibleDocumentArchiveIDsCount: 0,
    compatibleDocumentArchiveIDs: ptr::null(),
    analyzeableContentTypesCount: 0,
    analyzeableContentTypes: ptr::null(),
    supportedPlaybackTransformationFlags: ara::kARAPlaybackTransformationTimestretch,
    supportsStoringAudioFileChunks: 0,
});

unsafe extern "C" fn initialize_ara(_: *const ara::ARAInterfaceConfiguration) {}

unsafe extern "C" fn uninitialize_ara() {}

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
        *dst = src as TChar;
        len += 1;
    }
    if let Some(end) = dst.get_mut(len) {
        *end = 0;
    }
}

/// A class ID as the VST3 SDK's `FUID::toString` writes it.
fn format_fuid(tuid: &TUID) -> String {
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

/// The plug-in's class ID, as the host writes it.
#[must_use]
pub fn plugin_id() -> String {
    format_fuid(&Processor::CID)
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

/// The one class: component, audio processor and controller, and ARA's two
/// entry points.
struct Processor {
    /// Between `setActive(true)` and `setActive(false)`.
    active: Arc<AtomicBool>,
    rate: AtomicU64,
    /// Made when the host binds the instance to a document controller.
    extension: UnsafeCell<Option<Box<Extension>>>,
}

// SAFETY: the extension is set once by the host's bind call, before
// activation, and read by the process call after it.
unsafe impl Send for Processor {}
// SAFETY: as above.
unsafe impl Sync for Processor {}

impl Processor {
    const CID: TUID = uid(0x6A2B_5C11, 0x0F4A_4C57, 0x9E3D_81B4, 0x0A71_C6E5);

    fn new() -> Self {
        Self {
            active: Arc::new(AtomicBool::new(false)),
            rate: AtomicU64::new(48_000.0_f64.to_bits()),
            extension: UnsafeCell::new(None),
        }
    }
}

impl Class for Processor {
    type Interfaces = (
        IComponent,
        IAudioProcessor,
        IEditController,
        IPlugInEntryPoint,
        IPlugInEntryPoint2,
    );
}

impl IPlugInEntryPointTrait for Processor {
    unsafe fn get_factory(&self) -> *const ara::ARAFactory {
        &raw const FACTORY.0
    }

    unsafe fn bind_to_document_controller(
        &self,
        _: ara::ARADocumentControllerRef,
    ) -> *const ara::ARAPlugInExtensionInstance {
        // ARA 1's call: this plug-in is ARA 2 only.
        ptr::null()
    }
}

impl IPlugInEntryPoint2Trait for Processor {
    unsafe fn bind_to_document_controller_with_roles(
        &self,
        _: ara::ARADocumentControllerRef,
        _: ara::ARAPlugInInstanceRoleFlags,
        _: ara::ARAPlugInInstanceRoleFlags,
    ) -> *const ara::ARAPlugInExtensionInstance {
        // SAFETY: the host binds once, before the first activation.
        let extension = unsafe { &mut *self.extension.get() };
        let extension = extension.get_or_insert_with(|| Extension::new(&self.active));
        &raw const extension.instance
    }
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
    unsafe fn getControllerClassId(&self, _class_id: *mut TUID) -> tresult {
        // The component is its own controller.
        kResultFalse
    }

    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kNotImplemented
    }

    unsafe fn getBusCount(&self, media_type: MediaType, dir: BusDirection) -> i32 {
        match (media_type as MediaTypes, dir as BusDirections) {
            (MediaTypes_::kAudio, BusDirections_::kInput | BusDirections_::kOutput) => 1,
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
        if index != 0 || media_type as MediaTypes != MediaTypes_::kAudio {
            return kInvalidArgument;
        }
        // SAFETY: the host passes a valid pointer for the call.
        let bus = unsafe { &mut *bus };
        bus.direction = dir;
        bus.busType = BusTypes_::kMain as BusType;
        bus.flags = BusInfo_::BusFlags_::kDefaultActive as uint32;
        bus.mediaType = MediaTypes_::kAudio as MediaType;
        bus.channelCount = 2;
        copy_wstring("Stereo", &mut bus.name);
        kResultOk
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

    unsafe fn setActive(&self, state: TBool) -> tresult {
        self.active.store(state != 0, Release);
        kResultOk
    }

    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        // A state of [`REQUEST`] stands in for the person playing from the
        // plug-in's editor.
        if read_state(state) == REQUEST {
            document::request_transport();
        }
        kResultOk
    }

    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        if write_state(state, document::report().as_bytes()) {
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
        if symbolic_sample_size as SymbolicSampleSizes == SymbolicSampleSizes_::kSample32 {
            kResultOk
        } else {
            kNotImplemented
        }
    }

    unsafe fn getLatencySamples(&self) -> u32 {
        0
    }

    unsafe fn getTailSamples(&self) -> u32 {
        0
    }

    unsafe fn setupProcessing(&self, setup: *mut ProcessSetup) -> tresult {
        // SAFETY: the host passes a valid setup for the call.
        let rate = unsafe { (*setup).sampleRate };
        self.rate.store(rate.to_bits(), Relaxed);
        kResultOk
    }

    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }

    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        // SAFETY: the host passes a valid ProcessData for this call.
        let data = unsafe { &*data };
        if data.numOutputs < 1 || data.outputs.is_null() {
            return kResultOk;
        }
        // SAFETY: numOutputs bus buffers, as above.
        let output = unsafe { &*data.outputs };
        let frames = usize::try_from(data.numSamples).unwrap_or(0);
        let channels = usize::try_from(output.numChannels).unwrap_or(0);
        // SAFETY: each bus's channel buffers, as above.
        let buffers = unsafe { slice::from_raw_parts(output.__field0.channelBuffers32, channels) };
        let mut out: [&mut [f32]; 2] = [&mut [], &mut []];
        for (slot, buffer) in out.iter_mut().zip(buffers) {
            // SAFETY: a buffer of `frames` samples for each channel.
            *slot = unsafe { slice::from_raw_parts_mut(*buffer, frames) };
            slot.fill(0.0);
        }
        // SAFETY: the extension is set before activation.
        let Some(extension) = (unsafe { &*self.extension.get() }).as_ref() else {
            return kResultOk;
        };
        // SAFETY: the host's process context, or null.
        let Some(context) = (unsafe { data.processContext.as_ref() }) else {
            return kResultOk;
        };
        // The bindings' default enum type is signed on Windows and unsigned
        // elsewhere; the field is always `uint32`.
        let playing = i64::from(ProcessContext_::StatesAndFlags_::kPlaying);
        if i64::from(context.state) & playing == 0 {
            return kResultOk;
        }
        let rate = f64::from_bits(self.rate.load(Relaxed));
        let used = channels.min(2);
        extension
            .renderer
            .render(rate, context.projectTimeSamples, &mut out[..used]);
        kResultOk
    }
}

impl IEditControllerTrait for Processor {
    unsafe fn setComponentState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }

    unsafe fn setState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }

    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        // The controller's part of the state is empty.
        let _ = state;
        kResultOk
    }

    unsafe fn getParameterCount(&self) -> i32 {
        0
    }

    unsafe fn getParameterInfo(&self, _index: i32, _info: *mut ParameterInfo) -> tresult {
        kInvalidArgument
    }

    unsafe fn getParamStringByValue(
        &self,
        _id: ParamID,
        _value: ParamValue,
        _string: *mut String128,
    ) -> tresult {
        kInvalidArgument
    }

    unsafe fn getParamValueByString(
        &self,
        _id: ParamID,
        _string: *mut TChar,
        _value: *mut ParamValue,
    ) -> tresult {
        kInvalidArgument
    }

    unsafe fn normalizedParamToPlain(&self, _id: ParamID, value: ParamValue) -> ParamValue {
        value
    }

    unsafe fn plainParamToNormalized(&self, _id: ParamID, plain: ParamValue) -> ParamValue {
        plain
    }

    unsafe fn getParamNormalized(&self, _id: ParamID) -> ParamValue {
        0.0
    }

    unsafe fn setParamNormalized(&self, _id: ParamID, _value: ParamValue) -> tresult {
        kInvalidArgument
    }

    unsafe fn setComponentHandler(&self, _handler: *mut IComponentHandler) -> tresult {
        kResultOk
    }

    unsafe fn createView(&self, _name: *const c_char) -> *mut IPlugView {
        ptr::null_mut()
    }
}

/// The factory: one class.
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
        1
    }

    unsafe fn getClassInfo(&self, index: i32, info: *mut PClassInfo) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        // SAFETY: the host passes a valid pointer for the call.
        let info = unsafe { &mut *info };
        info.cid = Processor::CID;
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy_cstring("Audio Module Class", &mut info.category);
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
        if class != Processor::CID {
            return kInvalidArgument;
        }
        let Some(unknown) = ComWrapper::new(Processor::new()).to_com_ptr::<FUnknown>() else {
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

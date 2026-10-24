//! The ARA side of the test plug-in: a document controller that keeps the
//! host's sources, modifications and playback regions, reads each source's
//! audio through the host's audio reader when it is given access, plays its
//! renderer's regions from that audio, keeps an archive, and counts every
//! way the host used the API against its rules.
//!
//! It is built after the ARA SDK's own example plug-in, which does the same
//! in C++, but adds what the host's tests look for: a violation counter, a
//! log of the selections its editor view was told, and a per-modification
//! counter that stands in for the edits a real plug-in keeps, so a test can
//! tell an object that came back from one that was made again.

use std::cell::UnsafeCell;
use std::ffi::{CStr, c_char, c_void};
use std::mem::size_of;
use std::ptr;
use std::sync::{Arc, Mutex};
use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU32, AtomicU64, AtomicUsize};

use bl_ara_sys as ara;

/// The plug-in's ARA factory ID.
pub const FACTORY_ID: &CStr = c"io.loombridge.beatlayer.test-ara";
/// The ID of the archives it writes.
pub const ARCHIVE_ID: &CStr = c"io.loombridge.beatlayer.test-ara.archive.1";
/// What its archives start with.
const ARCHIVE_MAGIC: &[u8; 4] = b"BLAR";

/// How many times the host broke one of ARA's rules: changed a renderer's
/// regions while the plug-in was active, or destroyed an object something
/// still used.
pub static VIOLATIONS: AtomicU32 = AtomicU32::new(0);
/// What the editor view was told is selected: how many regions and how many
/// sequences, each time.
pub static SELECTIONS: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

fn violation() {
    VIOLATIONS.fetch_add(1, Relaxed);
}

/// The audio of a source, read whole.
struct Cache {
    channels: Vec<Vec<f32>>,
}

struct Source {
    host: ara::ARAAudioSourceHostRef,
    rate: f64,
    frames: i64,
    channels: i32,
    /// Published once read; the audio thread loads it.
    cache: AtomicPtr<Cache>,
    access: bool,
    active: bool,
    modifications: usize,
}

struct Modification {
    source: *mut Source,
    id: String,
    /// How many times the host updated its properties: the plug-in's own
    /// edits, as far as a test can tell.
    updates: u32,
    /// What the archive said `updates` was, if one was restored.
    restored: Option<u32>,
    active: bool,
    regions: usize,
}

/// A playback region's times, as atomics so a move reaches a running
/// renderer without a lock.
pub struct Region {
    modification: *mut Modification,
    modification_start: AtomicU64,
    modification_duration: AtomicU64,
    playback_start: AtomicU64,
    playback_duration: AtomicU64,
    flags: AtomicI32,
    /// How many renderers hold it.
    renderers: AtomicUsize,
}

impl Region {
    fn set(&self, properties: &ara::ARAPlaybackRegionProperties) {
        // The struct may be packed, so its fields are read by value.
        let properties = *properties;
        self.modification_start
            .store(properties.startInModificationTime.to_bits(), Relaxed);
        self.modification_duration
            .store(properties.durationInModificationTime.to_bits(), Relaxed);
        self.playback_start
            .store(properties.startInPlaybackTime.to_bits(), Relaxed);
        self.playback_duration
            .store(properties.durationInPlaybackTime.to_bits(), Relaxed);
        self.flags.store(properties.transformationFlags, Relaxed);
    }

    fn times(&self) -> (f64, f64, f64, f64) {
        let load = |value: &AtomicU64| f64::from_bits(value.load(Relaxed));
        (
            load(&self.modification_start),
            load(&self.modification_duration),
            load(&self.playback_start),
            load(&self.playback_duration),
        )
    }
}

/// A playback renderer: the regions it plays. They change only while the
/// plug-in is inactive, which the host must see to.
pub struct Renderer {
    active: Arc<AtomicBool>,
    regions: UnsafeCell<Vec<*const Region>>,
}

// SAFETY: the regions change only while the plug-in is inactive, and the
// audio thread reads them only while it is active.
unsafe impl Sync for Renderer {}
// SAFETY: as above.
unsafe impl Send for Renderer {}

impl Renderer {
    pub(crate) fn new(active: Arc<AtomicBool>) -> Self {
        Self {
            active,
            regions: UnsafeCell::new(Vec::new()),
        }
    }

    /// Plays the renderer's regions into `out`, one slice for each channel,
    /// for the song frames from `position`. Allocates nothing.
    pub(crate) fn render(&self, rate: f64, position: i64, out: &mut [&mut [f32]]) {
        let frames = out.first().map_or(0, |channel| channel.len());
        // SAFETY: the list only changes while inactive.
        let regions = unsafe { &*self.regions.get() };
        for &region in regions {
            // SAFETY: a region the renderer holds is alive until the host
            // takes it away.
            let region = unsafe { &*region };
            // SAFETY: a modification and its source outlive their regions.
            let source = unsafe { &*(*region.modification).source };
            let cache = source.cache.load(Acquire);
            if cache.is_null() || (source.rate - rate).abs() > 1e-6 {
                continue;
            }
            // SAFETY: published once, never freed while the source lives.
            let cache = unsafe { &*cache };
            let (modification_start, modification_duration, playback_start, playback_duration) =
                region.times();
            let start = (playback_start * rate).round() as i64;
            let end = ((playback_start + playback_duration) * rate).round() as i64;
            let stretched = region.flags.load(Relaxed) != 0;
            let ratio = if playback_duration > 0.0 {
                modification_duration / playback_duration
            } else {
                1.0
            };
            let origin = modification_start * rate;
            for frame in start.max(position)..end.min(position + frames as i64) {
                let at = if stretched {
                    (origin + (frame - start) as f64 * ratio + 0.5).floor() as i64
                } else {
                    origin.round() as i64 + (frame - start)
                };
                let Ok(at) = usize::try_from(at) else { continue };
                for (channel, data) in cache.channels.iter().enumerate() {
                    if let (Some(sample), Some(out)) = (
                        data.get(at),
                        out.get_mut(channel)
                            .and_then(|out| out.get_mut((frame - position) as usize)),
                    ) {
                        *out += *sample;
                    }
                }
            }
        }
    }

    fn add(&self, region: *const Region) {
        if self.active.load(Acquire) {
            violation();
        }
        // SAFETY: the host calls from the main thread, with the plug-in
        // inactive.
        unsafe { (*self.regions.get()).push(region) };
        // SAFETY: a region the host just gave.
        unsafe { (*region).renderers.fetch_add(1, AcqRel) };
    }

    fn remove(&self, region: *const Region) {
        if self.active.load(Acquire) {
            violation();
        }
        // SAFETY: as above.
        unsafe { (*self.regions.get()).retain(|held| *held != region) };
        // SAFETY: as above.
        unsafe { (*region).renderers.fetch_sub(1, AcqRel) };
    }
}

/// The document the host builds.
pub struct Document {
    host: ara::ARADocumentControllerHostInstance,
    instance: ara::ARADocumentControllerInstance,
    sources: Vec<*mut Source>,
    modifications: Vec<*mut Modification>,
    regions: Vec<*mut Region>,
    /// The archive ID the host gave the archive it asked us to restore.
    pub restored_archive_id: Option<String>,
}

/// The document the host last made, for the report: the plug-in has one per
/// host document and a test has one host.
static DOCUMENT: AtomicPtr<Document> = AtomicPtr::new(ptr::null_mut());

fn text(string: *const c_char) -> String {
    if string.is_null() {
        return String::new();
    }
    // SAFETY: the host's string, valid for the call.
    unsafe { CStr::from_ptr(string) }.to_string_lossy().into_owned()
}

fn document<'a>(reference: ara::ARADocumentControllerRef) -> &'a mut Document {
    // SAFETY: a reference this module gave, until the controller is destroyed.
    unsafe { &mut *reference.cast::<Document>() }
}

// --- Audio -----------------------------------------------------------------

/// Reads a source's whole audio through the host's reader.
fn read(document: &Document, source: &mut Source) {
    if !source.cache.load(Acquire).is_null() {
        return;
    }
    let host = document.host;
    let interface = host.audioAccessControllerInterface;
    if interface.is_null() {
        return;
    }
    // SAFETY: the host's interface table, valid while the controller lives.
    let interface = unsafe { interface.read_unaligned() };
    let (Some(create), Some(read), Some(destroy)) = (
        interface.createAudioReaderForSource,
        interface.readAudioSamples,
        interface.destroyAudioReader,
    ) else {
        return;
    };
    // SAFETY: the host's function with its own references.
    let reader = unsafe { create(host.audioAccessControllerHostRef, source.host, 0) };
    if reader.is_null() {
        return;
    }
    let frames = usize::try_from(source.frames).unwrap_or(0);
    let mut channels: Vec<Vec<f32>> = (0..source.channels).map(|_| vec![0.0; frames]).collect();
    let pointers: Vec<*mut c_void> = channels.iter_mut().map(|c| c.as_mut_ptr().cast()).collect();
    // SAFETY: a buffer of `frames` samples for each of the source's channels.
    let ok = unsafe {
        read(
            host.audioAccessControllerHostRef,
            reader,
            0,
            source.frames,
            pointers.as_ptr(),
        )
    };
    // SAFETY: the reader just made, not in use.
    unsafe { destroy(host.audioAccessControllerHostRef, reader) };
    if ok != 0 {
        source
            .cache
            .store(Box::into_raw(Box::new(Cache { channels })), Release);
    }
}

// --- The document controller interface ---------------------------------------

unsafe extern "C" fn destroy_document_controller(reference: ara::ARADocumentControllerRef) {
    let ptr = reference.cast::<Document>();
    DOCUMENT
        .compare_exchange(ptr, ptr::null_mut(), AcqRel, Acquire)
        .ok();
    // SAFETY: made by `create_document_controller`, destroyed once.
    let document = unsafe { Box::from_raw(ptr) };
    for region in &document.regions {
        // SAFETY: owned boxes the document made.
        drop(unsafe { Box::from_raw(*region) });
    }
    for modification in &document.modifications {
        // SAFETY: as above.
        drop(unsafe { Box::from_raw(*modification) });
    }
    for source in &document.sources {
        // SAFETY: as above; the cache is leaked on purpose, because the audio
        // thread may still be reading it.
        drop(unsafe { Box::from_raw(*source) });
    }
}

unsafe extern "C" fn get_factory(_: ara::ARADocumentControllerRef) -> *const ara::ARAFactory {
    &raw const crate::FACTORY.0
}

unsafe extern "C" fn nothing(_: ara::ARADocumentControllerRef) {}

unsafe extern "C" fn update_document_properties(
    _: ara::ARADocumentControllerRef,
    _: *const ara::ARADocumentProperties,
) {
}

unsafe extern "C" fn create_musical_context(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAMusicalContextHostRef,
    _: *const ara::ARAMusicalContextProperties,
) -> ara::ARAMusicalContextRef {
    Box::into_raw(Box::new(0_u8)).cast()
}

unsafe extern "C" fn update_musical_context_properties(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAMusicalContextRef,
    _: *const ara::ARAMusicalContextProperties,
) {
}

unsafe extern "C" fn update_musical_context_content(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAMusicalContextRef,
    _: *const ara::ARAContentTimeRange,
    _: ara::ARAContentUpdateFlags,
) {
}

unsafe extern "C" fn destroy_musical_context(
    _: ara::ARADocumentControllerRef,
    context: ara::ARAMusicalContextRef,
) {
    // SAFETY: made by `create_musical_context`.
    drop(unsafe { Box::from_raw(context.cast::<u8>()) });
}

unsafe extern "C" fn create_region_sequence(
    _: ara::ARADocumentControllerRef,
    _: ara::ARARegionSequenceHostRef,
    _: *const ara::ARARegionSequenceProperties,
) -> ara::ARARegionSequenceRef {
    Box::into_raw(Box::new(0_u8)).cast()
}

unsafe extern "C" fn update_region_sequence_properties(
    _: ara::ARADocumentControllerRef,
    _: ara::ARARegionSequenceRef,
    _: *const ara::ARARegionSequenceProperties,
) {
}

unsafe extern "C" fn destroy_region_sequence(
    _: ara::ARADocumentControllerRef,
    sequence: ara::ARARegionSequenceRef,
) {
    // SAFETY: made by `create_region_sequence`.
    drop(unsafe { Box::from_raw(sequence.cast::<u8>()) });
}

unsafe extern "C" fn create_audio_source(
    reference: ara::ARADocumentControllerRef,
    host: ara::ARAAudioSourceHostRef,
    properties: *const ara::ARAAudioSourceProperties,
) -> ara::ARAAudioSourceRef {
    // SAFETY: the host's properties, valid for the call.
    let properties = unsafe { properties.read_unaligned() };
    let source = Box::into_raw(Box::new(Source {
        host,
        rate: properties.sampleRate,
        frames: properties.sampleCount,
        channels: properties.channelCount,
        cache: AtomicPtr::new(ptr::null_mut()),
        access: false,
        active: true,
        modifications: 0,
    }));
    document(reference).sources.push(source);
    source.cast()
}

unsafe extern "C" fn update_audio_source_properties(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAAudioSourceRef,
    _: *const ara::ARAAudioSourceProperties,
) {
}

unsafe extern "C" fn update_audio_source_content(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAAudioSourceRef,
    _: *const ara::ARAContentTimeRange,
    _: ara::ARAContentUpdateFlags,
) {
}

unsafe extern "C" fn enable_audio_source_samples_access(
    reference: ara::ARADocumentControllerRef,
    source: ara::ARAAudioSourceRef,
    enable: ara::ARABool,
) {
    let document = document(reference);
    // SAFETY: a source this module made.
    let source = unsafe { &mut *source.cast::<Source>() };
    source.access = enable != 0;
    if source.access {
        read(document, source);
    }
}

unsafe extern "C" fn deactivate_audio_source_for_undo_history(
    _: ara::ARADocumentControllerRef,
    source: ara::ARAAudioSourceRef,
    deactivate: ara::ARABool,
) {
    // SAFETY: a source this module made.
    unsafe { &mut *source.cast::<Source>() }.active = deactivate == 0;
}

unsafe extern "C" fn destroy_audio_source(
    reference: ara::ARADocumentControllerRef,
    source: ara::ARAAudioSourceRef,
) {
    let source = source.cast::<Source>();
    // SAFETY: a source this module made.
    if unsafe { (*source).modifications } != 0 {
        violation();
    }
    document(reference).sources.retain(|held| *held != source);
    // SAFETY: owned, destroyed once; the cache is leaked on purpose.
    drop(unsafe { Box::from_raw(source) });
}

unsafe extern "C" fn create_audio_modification(
    reference: ara::ARADocumentControllerRef,
    source: ara::ARAAudioSourceRef,
    _: ara::ARAAudioModificationHostRef,
    properties: *const ara::ARAAudioModificationProperties,
) -> ara::ARAAudioModificationRef {
    let source = source.cast::<Source>();
    // SAFETY: the host's properties, valid for the call.
    let properties = unsafe { properties.read_unaligned() };
    // SAFETY: a source this module made.
    unsafe { (*source).modifications += 1 };
    let modification = Box::into_raw(Box::new(Modification {
        source,
        id: text(properties.persistentID),
        updates: 0,
        restored: None,
        active: true,
        regions: 0,
    }));
    document(reference).modifications.push(modification);
    modification.cast()
}

unsafe extern "C" fn clone_audio_modification(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAAudioModificationRef,
    _: ara::ARAAudioModificationHostRef,
    _: *const ara::ARAAudioModificationProperties,
) -> ara::ARAAudioModificationRef {
    ptr::null_mut()
}

unsafe extern "C" fn update_audio_modification_properties(
    _: ara::ARADocumentControllerRef,
    modification: ara::ARAAudioModificationRef,
    _: *const ara::ARAAudioModificationProperties,
) {
    // SAFETY: a modification this module made.
    let modification = unsafe { &mut *modification.cast::<Modification>() };
    if !modification.active {
        violation();
    }
    modification.updates += 1;
}

unsafe extern "C" fn deactivate_audio_modification_for_undo_history(
    _: ara::ARADocumentControllerRef,
    modification: ara::ARAAudioModificationRef,
    deactivate: ara::ARABool,
) {
    // SAFETY: a modification this module made.
    unsafe { &mut *modification.cast::<Modification>() }.active = deactivate == 0;
}

unsafe extern "C" fn destroy_audio_modification(
    reference: ara::ARADocumentControllerRef,
    modification: ara::ARAAudioModificationRef,
) {
    let modification = modification.cast::<Modification>();
    // SAFETY: a modification this module made.
    unsafe {
        if (*modification).regions != 0 {
            violation();
        }
        (*(*modification).source).modifications -= 1;
    }
    document(reference)
        .modifications
        .retain(|held| *held != modification);
    // SAFETY: owned, destroyed once.
    drop(unsafe { Box::from_raw(modification) });
}

unsafe extern "C" fn create_playback_region(
    reference: ara::ARADocumentControllerRef,
    modification: ara::ARAAudioModificationRef,
    _: ara::ARAPlaybackRegionHostRef,
    properties: *const ara::ARAPlaybackRegionProperties,
) -> ara::ARAPlaybackRegionRef {
    let modification = modification.cast::<Modification>();
    // SAFETY: a modification this module made.
    unsafe { (*modification).regions += 1 };
    let region = Box::new(Region {
        modification,
        modification_start: AtomicU64::new(0),
        modification_duration: AtomicU64::new(0),
        playback_start: AtomicU64::new(0),
        playback_duration: AtomicU64::new(0),
        flags: AtomicI32::new(0),
        renderers: AtomicUsize::new(0),
    });
    // SAFETY: the host's properties, valid for the call.
    region.set(unsafe { &*properties });
    let region = Box::into_raw(region);
    document(reference).regions.push(region);
    region.cast()
}

unsafe extern "C" fn update_playback_region_properties(
    _: ara::ARADocumentControllerRef,
    region: ara::ARAPlaybackRegionRef,
    properties: *const ara::ARAPlaybackRegionProperties,
) {
    // SAFETY: a region this module made, and the host's properties.
    unsafe { (*region.cast::<Region>()).set(&*properties) };
}

unsafe extern "C" fn destroy_playback_region(
    reference: ara::ARADocumentControllerRef,
    region: ara::ARAPlaybackRegionRef,
) {
    let region = region.cast::<Region>();
    // SAFETY: a region this module made.
    unsafe {
        if (*region).renderers.load(Acquire) != 0 {
            violation();
        }
        (*(*region).modification).regions -= 1;
    }
    document(reference).regions.retain(|held| *held != region);
    // SAFETY: owned, destroyed once.
    drop(unsafe { Box::from_raw(region) });
}

unsafe extern "C" fn content_available(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAAudioSourceRef,
    _: ara::ARAContentType,
) -> ara::ARABool {
    0
}

unsafe extern "C" fn content_analysis_incomplete(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAAudioSourceRef,
    _: ara::ARAContentType,
) -> ara::ARABool {
    0
}

unsafe extern "C" fn request_analysis(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAAudioSourceRef,
    _: ara::ARASize,
    _: *const ara::ARAContentType,
) {
}

unsafe extern "C" fn content_grade(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAAudioSourceRef,
    _: ara::ARAContentType,
) -> ara::ARAContentGrade {
    ara::kARAContentGradeInitial
}

unsafe extern "C" fn create_source_content_reader(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAAudioSourceRef,
    _: ara::ARAContentType,
    _: *const ara::ARAContentTimeRange,
) -> ara::ARAContentReaderRef {
    ptr::null_mut()
}

unsafe extern "C" fn modification_content_available(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAAudioModificationRef,
    _: ara::ARAContentType,
) -> ara::ARABool {
    0
}

unsafe extern "C" fn modification_content_grade(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAAudioModificationRef,
    _: ara::ARAContentType,
) -> ara::ARAContentGrade {
    ara::kARAContentGradeInitial
}

unsafe extern "C" fn create_modification_content_reader(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAAudioModificationRef,
    _: ara::ARAContentType,
    _: *const ara::ARAContentTimeRange,
) -> ara::ARAContentReaderRef {
    ptr::null_mut()
}

unsafe extern "C" fn region_content_available(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAPlaybackRegionRef,
    _: ara::ARAContentType,
) -> ara::ARABool {
    0
}

unsafe extern "C" fn region_content_grade(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAPlaybackRegionRef,
    _: ara::ARAContentType,
) -> ara::ARAContentGrade {
    ara::kARAContentGradeInitial
}

unsafe extern "C" fn create_region_content_reader(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAPlaybackRegionRef,
    _: ara::ARAContentType,
    _: *const ara::ARAContentTimeRange,
) -> ara::ARAContentReaderRef {
    ptr::null_mut()
}

unsafe extern "C" fn reader_event_count(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAContentReaderRef,
) -> ara::ARAInt32 {
    0
}

unsafe extern "C" fn reader_event(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAContentReaderRef,
    _: ara::ARAInt32,
) -> *const c_void {
    ptr::null()
}

unsafe extern "C" fn destroy_content_reader(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAContentReaderRef,
) {
}

unsafe extern "C" fn head_and_tail(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAPlaybackRegionRef,
    head: *mut ara::ARATimeDuration,
    tail: *mut ara::ARATimeDuration,
) {
    // SAFETY: the host's out pointers.
    unsafe {
        head.write_unaligned(0.0);
        tail.write_unaligned(0.0);
    }
}

unsafe extern "C" fn restore_objects(
    reference: ara::ARADocumentControllerRef,
    reader: ara::ARAArchiveReaderHostRef,
    _: *const ara::ARARestoreObjectsFilter,
) -> ara::ARABool {
    let document = document(reference);
    // SAFETY: the host's interface table, valid while the controller lives.
    let archiving = unsafe { document.host.archivingControllerInterface.read_unaligned() };
    let (Some(size), Some(read), Some(id)) = (
        archiving.getArchiveSize,
        archiving.readBytesFromArchive,
        archiving.getDocumentArchiveID,
    ) else {
        return 0;
    };
    let host = document.host.archivingControllerHostRef;
    // SAFETY: the host's functions with its own references.
    let (length, archive_id) = unsafe { (size(host, reader), id(host, reader)) };
    document.restored_archive_id = Some(text(archive_id));
    let mut bytes = vec![0_u8; length];
    // SAFETY: a buffer of `length` bytes.
    if unsafe { read(host, reader, 0, length, bytes.as_mut_ptr()) } == 0 {
        return 0;
    }
    if bytes.get(..4) != Some(ARCHIVE_MAGIC.as_slice()) {
        return 0;
    }
    let mut at = 4;
    let count = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap_or_default());
    at += 4;
    for _ in 0..count {
        let Some(&id_length) = bytes.get(at) else {
            return 0;
        };
        at += 1;
        let id = String::from_utf8_lossy(&bytes[at..at + usize::from(id_length)]).into_owned();
        at += usize::from(id_length);
        let updates = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap_or_default());
        at += 4;
        for &modification in &document.modifications {
            // SAFETY: a modification this module made.
            let modification = unsafe { &mut *modification };
            if modification.id == id {
                modification.updates = updates;
                modification.restored = Some(updates);
            }
        }
    }
    1
}

unsafe extern "C" fn store_objects(
    reference: ara::ARADocumentControllerRef,
    writer: ara::ARAArchiveWriterHostRef,
    _: *const ara::ARAStoreObjectsFilter,
) -> ara::ARABool {
    let document = document(reference);
    let mut bytes = ARCHIVE_MAGIC.to_vec();
    bytes.extend_from_slice(&u32::try_from(document.modifications.len()).unwrap_or(0).to_le_bytes());
    for &modification in &document.modifications {
        // SAFETY: a modification this module made.
        let modification = unsafe { &*modification };
        bytes.push(u8::try_from(modification.id.len()).unwrap_or(0));
        bytes.extend_from_slice(modification.id.as_bytes());
        bytes.extend_from_slice(&modification.updates.to_le_bytes());
    }
    // SAFETY: the host's interface table, valid while the controller lives.
    let archiving = unsafe { document.host.archivingControllerInterface.read_unaligned() };
    let Some(write) = archiving.writeBytesToArchive else {
        return 0;
    };
    // SAFETY: the host's function with its own references.
    unsafe {
        write(
            document.host.archivingControllerHostRef,
            writer,
            0,
            bytes.len(),
            bytes.as_ptr(),
        )
    }
}

unsafe extern "C" fn algorithms_count(_: ara::ARADocumentControllerRef) -> ara::ARAInt32 {
    0
}

unsafe extern "C" fn algorithm_properties(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAInt32,
) -> *const ara::ARAProcessingAlgorithmProperties {
    ptr::null()
}

unsafe extern "C" fn algorithm_for_source(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAAudioSourceRef,
) -> ara::ARAInt32 {
    0
}

unsafe extern "C" fn request_algorithm(
    _: ara::ARADocumentControllerRef,
    _: ara::ARAAudioSourceRef,
    _: ara::ARAInt32,
) {
}

unsafe extern "C" fn licensed(
    _: ara::ARADocumentControllerRef,
    _: ara::ARABool,
    _: ara::ARASize,
    _: *const ara::ARAContentType,
    _: ara::ARAPlaybackTransformationFlags,
) -> ara::ARABool {
    1
}

/// The document controller interface, as a table that outlives every
/// document.
pub struct Interface(pub ara::ARADocumentControllerInterface);

// SAFETY: a table of function pointers.
unsafe impl Sync for Interface {}

pub static INTERFACE: Interface = Interface(ara::ARADocumentControllerInterface {
    structSize: size_of::<ara::ARADocumentControllerInterface>(),
    destroyDocumentController: Some(destroy_document_controller),
    getFactory: Some(get_factory),
    beginEditing: Some(nothing),
    endEditing: Some(nothing),
    notifyModelUpdates: Some(nothing),
    updateDocumentProperties: Some(update_document_properties),
    createMusicalContext: Some(create_musical_context),
    updateMusicalContextProperties: Some(update_musical_context_properties),
    updateMusicalContextContent: Some(update_musical_context_content),
    destroyMusicalContext: Some(destroy_musical_context),
    createAudioSource: Some(create_audio_source),
    updateAudioSourceProperties: Some(update_audio_source_properties),
    updateAudioSourceContent: Some(update_audio_source_content),
    enableAudioSourceSamplesAccess: Some(enable_audio_source_samples_access),
    deactivateAudioSourceForUndoHistory: Some(deactivate_audio_source_for_undo_history),
    destroyAudioSource: Some(destroy_audio_source),
    createAudioModification: Some(create_audio_modification),
    cloneAudioModification: Some(clone_audio_modification),
    updateAudioModificationProperties: Some(update_audio_modification_properties),
    deactivateAudioModificationForUndoHistory: Some(deactivate_audio_modification_for_undo_history),
    destroyAudioModification: Some(destroy_audio_modification),
    createPlaybackRegion: Some(create_playback_region),
    updatePlaybackRegionProperties: Some(update_playback_region_properties),
    destroyPlaybackRegion: Some(destroy_playback_region),
    isAudioSourceContentAvailable: Some(content_available),
    isAudioSourceContentAnalysisIncomplete: Some(content_analysis_incomplete),
    requestAudioSourceContentAnalysis: Some(request_analysis),
    getAudioSourceContentGrade: Some(content_grade),
    createAudioSourceContentReader: Some(create_source_content_reader),
    isAudioModificationContentAvailable: Some(modification_content_available),
    getAudioModificationContentGrade: Some(modification_content_grade),
    createAudioModificationContentReader: Some(create_modification_content_reader),
    isPlaybackRegionContentAvailable: Some(region_content_available),
    getPlaybackRegionContentGrade: Some(region_content_grade),
    createPlaybackRegionContentReader: Some(create_region_content_reader),
    getContentReaderEventCount: Some(reader_event_count),
    getContentReaderDataForEvent: Some(reader_event),
    destroyContentReader: Some(destroy_content_reader),
    createRegionSequence: Some(create_region_sequence),
    updateRegionSequenceProperties: Some(update_region_sequence_properties),
    destroyRegionSequence: Some(destroy_region_sequence),
    getPlaybackRegionHeadAndTailTime: Some(head_and_tail),
    restoreObjectsFromArchive: Some(restore_objects),
    storeObjectsToArchive: Some(store_objects),
    getProcessingAlgorithmsCount: Some(algorithms_count),
    getProcessingAlgorithmProperties: Some(algorithm_properties),
    getProcessingAlgorithmForAudioSource: Some(algorithm_for_source),
    requestProcessingAlgorithmForAudioSource: Some(request_algorithm),
    isLicensedForCapabilities: Some(licensed),
    ..unsafe { std::mem::zeroed() }
});

/// Makes a document controller for the host's interfaces (the factory's
/// `createDocumentControllerWithDocument`).
pub unsafe extern "C" fn create_document_controller(
    host: *const ara::ARADocumentControllerHostInstance,
    _: *const ara::ARADocumentProperties,
) -> *const ara::ARADocumentControllerInstance {
    // SAFETY: the host's instance, valid for the call; copied.
    let host = unsafe { host.read_unaligned() };
    let document = Box::into_raw(Box::new(Document {
        host,
        // SAFETY: filled in below.
        instance: unsafe { std::mem::zeroed() },
        sources: Vec::new(),
        modifications: Vec::new(),
        regions: Vec::new(),
        restored_archive_id: None,
    }));
    // SAFETY: just made, and never moved.
    unsafe {
        (*document).instance = ara::ARADocumentControllerInstance {
            structSize: size_of::<ara::ARADocumentControllerInstance>(),
            documentControllerRef: document.cast(),
            documentControllerInterface: &raw const INTERFACE.0,
        };
    }
    DOCUMENT.store(document, Release);
    // SAFETY: as above.
    unsafe { &raw const (*document).instance }
}

// --- The plug-in extension ---------------------------------------------------

unsafe extern "C" fn add_playback_region(
    renderer: ara::ARAPlaybackRendererRef,
    region: ara::ARAPlaybackRegionRef,
) {
    // SAFETY: a renderer this module made.
    unsafe { &*renderer.cast::<Renderer>() }.add(region.cast());
}

unsafe extern "C" fn remove_playback_region(
    renderer: ara::ARAPlaybackRendererRef,
    region: ara::ARAPlaybackRegionRef,
) {
    // SAFETY: as above.
    unsafe { &*renderer.cast::<Renderer>() }.remove(region.cast());
}

unsafe extern "C" fn add_editor_region(
    renderer: ara::ARAEditorRendererRef,
    region: ara::ARAPlaybackRegionRef,
) {
    // SAFETY: a renderer this module made.
    let renderer = unsafe { &*renderer.cast::<EditorRenderer>() };
    if renderer.active.load(Acquire) {
        violation();
    }
    // SAFETY: a region the host gave.
    unsafe { (*region.cast::<Region>()).renderers.fetch_add(1, AcqRel) };
}

unsafe extern "C" fn remove_editor_region(
    renderer: ara::ARAEditorRendererRef,
    region: ara::ARAPlaybackRegionRef,
) {
    // SAFETY: as above.
    let renderer = unsafe { &*renderer.cast::<EditorRenderer>() };
    if renderer.active.load(Acquire) {
        violation();
    }
    // SAFETY: as above.
    unsafe { (*region.cast::<Region>()).renderers.fetch_sub(1, AcqRel) };
}

unsafe extern "C" fn add_editor_sequence(
    renderer: ara::ARAEditorRendererRef,
    _: ara::ARARegionSequenceRef,
) {
    // SAFETY: a renderer this module made.
    if unsafe { &*renderer.cast::<EditorRenderer>() }.active.load(Acquire) {
        violation();
    }
}

unsafe extern "C" fn remove_editor_sequence(
    renderer: ara::ARAEditorRendererRef,
    _: ara::ARARegionSequenceRef,
) {
    // SAFETY: as above.
    if unsafe { &*renderer.cast::<EditorRenderer>() }.active.load(Acquire) {
        violation();
    }
}

unsafe extern "C" fn notify_selection(
    _: ara::ARAEditorViewRef,
    selection: *const ara::ARAViewSelection,
) {
    // SAFETY: the host's selection, valid for the call.
    let selection = unsafe { selection.read_unaligned() };
    if let Ok(mut log) = SELECTIONS.lock() {
        log.push((selection.playbackRegionRefsCount, selection.regionSequenceRefsCount));
    }
}

unsafe extern "C" fn notify_hide_sequences(
    _: ara::ARAEditorViewRef,
    _: ara::ARASize,
    _: *const ara::ARARegionSequenceRef,
) {
}

unsafe extern "C" fn set_playback_region(
    _: ara::ARAPlugInExtensionRef,
    _: ara::ARAPlaybackRegionRef,
) {
}

unsafe extern "C" fn remove_extension_region(
    _: ara::ARAPlugInExtensionRef,
    _: ara::ARAPlaybackRegionRef,
) {
}

/// An editor renderer: it holds nothing, but the host must not change what
/// it holds while the plug-in is active.
pub struct EditorRenderer {
    active: Arc<AtomicBool>,
}

struct Tables(
    ara::ARAPlaybackRendererInterface,
    ara::ARAEditorRendererInterface,
    ara::ARAEditorViewInterface,
    ara::ARAPlugInExtensionInterface,
);

// SAFETY: tables of function pointers.
unsafe impl Sync for Tables {}

static TABLES: Tables = Tables(
    ara::ARAPlaybackRendererInterface {
        structSize: size_of::<ara::ARAPlaybackRendererInterface>(),
        addPlaybackRegion: Some(add_playback_region),
        removePlaybackRegion: Some(remove_playback_region),
    },
    ara::ARAEditorRendererInterface {
        structSize: size_of::<ara::ARAEditorRendererInterface>(),
        addPlaybackRegion: Some(add_editor_region),
        removePlaybackRegion: Some(remove_editor_region),
        addRegionSequence: Some(add_editor_sequence),
        removeRegionSequence: Some(remove_editor_sequence),
    },
    ara::ARAEditorViewInterface {
        structSize: size_of::<ara::ARAEditorViewInterface>(),
        notifySelection: Some(notify_selection),
        notifyHideRegionSequences: Some(notify_hide_sequences),
    },
    ara::ARAPlugInExtensionInterface {
        structSize: size_of::<ara::ARAPlugInExtensionInterface>(),
        setPlaybackRegion: Some(set_playback_region),
        removePlaybackRegion: Some(remove_extension_region),
    },
);

/// What an instance has of ARA once the host has bound it.
pub struct Extension {
    pub instance: ara::ARAPlugInExtensionInstance,
    pub renderer: Box<Renderer>,
    _editor: Box<EditorRenderer>,
}

impl Extension {
    pub(crate) fn new(active: &Arc<AtomicBool>) -> Box<Self> {
        let renderer = Box::new(Renderer::new(Arc::clone(active)));
        let editor = Box::new(EditorRenderer {
            active: Arc::clone(active),
        });
        Box::new(Self {
            instance: ara::ARAPlugInExtensionInstance {
                structSize: size_of::<ara::ARAPlugInExtensionInstance>(),
                plugInExtensionRef: ptr::null_mut(),
                plugInExtensionInterface: &raw const TABLES.3,
                playbackRendererRef: ptr::from_ref(&*renderer).cast_mut().cast(),
                playbackRendererInterface: &raw const TABLES.0,
                editorRendererRef: ptr::from_ref(&*editor).cast_mut().cast(),
                editorRendererInterface: &raw const TABLES.1,
                editorViewRef: ptr::null_mut(),
                editorViewInterface: &raw const TABLES.2,
            },
            renderer,
            _editor: editor,
        })
    }
}

// --- The report ------------------------------------------------------------------

/// What the plug-in tells the host's tests, as text in its component state.
pub fn report() -> String {
    let mut text = format!("v={};s=", VIOLATIONS.load(Relaxed));
    if let Ok(log) = SELECTIONS.lock() {
        let entries: Vec<String> = log.iter().map(|(r, s)| format!("{r}:{s}")).collect();
        text.push_str(&entries.join(","));
    }
    text.push_str(";m=");
    let document = DOCUMENT.load(Acquire);
    if !document.is_null() {
        // SAFETY: the document is alive until the host destroys it, on the
        // main thread this is called from.
        let document = unsafe { &*document };
        let entries: Vec<String> = document
            .modifications
            .iter()
            .map(|&modification| {
                // SAFETY: owned by the document.
                let modification = unsafe { &*modification };
                format!(
                    "{}:{}:{}:{}",
                    modification.id,
                    modification.updates,
                    modification.restored.map_or(-1, i64::from),
                    u8::from(modification.active)
                )
            })
            .collect();
        text.push_str(&entries.join(","));
        text.push_str(&format!(
            ";a={};sources={}",
            document.restored_archive_id.as_deref().unwrap_or(""),
            document.sources.len()
        ));
    }
    text
}

/// Asks the host for each transport request ARA has, as a plug-in's editor
/// does when the person plays from it: start, a locate, a cycle range, the
/// cycle on, and stop.
pub fn request_transport() {
    let document = DOCUMENT.load(Acquire);
    if document.is_null() {
        return;
    }
    // SAFETY: alive until the host destroys it, on the thread this runs on.
    let host = unsafe { (*document).host };
    if host.playbackControllerInterface.is_null() {
        return;
    }
    // SAFETY: the host's interface table, valid while the controller lives.
    let playback = unsafe { host.playbackControllerInterface.read_unaligned() };
    let reference = host.playbackControllerHostRef;
    // SAFETY: the host's functions with its own reference.
    unsafe {
        if let Some(start) = playback.requestStartPlayback {
            start(reference);
        }
        if let Some(locate) = playback.requestSetPlaybackPosition {
            locate(reference, 2.5);
        }
        if let Some(range) = playback.requestSetCycleRange {
            range(reference, 1.0, 2.0);
        }
        if let Some(cycle) = playback.requestEnableCycle {
            cycle(reference, 1);
        }
        if let Some(stop) = playback.requestStopPlayback {
            stop(reference);
        }
    }
}

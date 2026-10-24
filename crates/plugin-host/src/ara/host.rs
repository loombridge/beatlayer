//! The host side of an ARA document controller: the five interfaces a
//! plug-in calls back into (ARA's audio access, archiving, content access,
//! model update and playback controllers), as `extern "C"` functions over one
//! [`ControllerHost`] per document controller.
//!
//! Every host reference the plug-in holds is a pointer to something this
//! module (or the controller) owns. Audio readers run on the plug-in's own
//! threads and read an immutable `Arc` of the decoded audio, so they take no
//! lock and never wait on the main thread (ara-hosting R7).

use std::cell::Cell;
use std::ffi::{CString, c_char, c_void};
use std::mem::size_of;
use std::slice;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use bl_ara_sys as sys;
use bl_core::time::{Ticks, TempoMap, TempoSegment, TICKS_PER_QUARTER, TimeSignatureMap};
use bl_media::SampleData;

thread_local! {
    /// Whether this thread is inside a plug-in's `process` call, which an
    /// audio reader must never be reached from (ara-hosting R7).
    static RENDERING: Cell<bool> = const { Cell::new(false) };
}

/// Marks the thread as rendering for as long as the guard lives. The VST3
/// node takes one around each call to the plug-in's `process`.
#[derive(Debug)]
pub(crate) struct Rendering(());

impl Rendering {
    pub(crate) fn enter() -> Self {
        RENDERING.with(|rendering| rendering.set(true));
        Self(())
    }
}

impl Drop for Rendering {
    fn drop(&mut self) {
        RENDERING.with(|rendering| rendering.set(false));
    }
}

/// What the plug-in asked the host to do with the transport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlaybackRequest {
    /// Start playing.
    Start,
    /// Stop playing.
    Stop,
    /// Move the playhead to a time in seconds.
    Locate(f64),
    /// Set the cycle (loop) range, in seconds.
    CycleRange { start: f64, duration: f64 },
    /// Switch the cycle on or off.
    Cycle(bool),
}

/// What the plug-in has done through the host's interfaces, for
/// `ara.state` and the tests.
#[derive(Debug, Default)]
pub(crate) struct Stats {
    pub(crate) readers_opened: AtomicU64,
    pub(crate) reader_calls: AtomicU64,
    pub(crate) frames_read: AtomicU64,
    /// Reads that came from inside a `process` call: always 0 in a correct
    /// plug-in (R7).
    pub(crate) audio_thread_reads: AtomicU64,
    pub(crate) content_reads: AtomicU64,
    pub(crate) model_notifications: AtomicU64,
}

/// The tempo map and signatures as the content readers give them.
#[derive(Default)]
pub(crate) struct Content {
    tempo: Vec<sys::ARAContentTempoEntry>,
    bars: Vec<sys::ARAContentBarSignature>,
}

impl Content {
    /// The sync points of `tempo` and the bar signatures of `signatures`
    /// (R3): a tempo entry at quarter 0 and at each tempo change, and one at
    /// each quarter note of a ramp, which the steady stretches between them
    /// follow; and one more a quarter note after the last, which the
    /// plug-in takes the final tempo from.
    pub(crate) fn of(tempo: &TempoMap, signatures: &TimeSignatureMap) -> Self {
        let segments: Vec<TempoSegment> = tempo.segments().collect();
        let mut ticks = vec![0_i64];
        for (at, segment) in segments.iter().enumerate() {
            let start = segment.start().get().max(0);
            ticks.push(start);
            if let TempoSegment::Ramp { .. } = segment
                && let Some(next) = segments.get(at + 1)
            {
                let mut step = start + TICKS_PER_QUARTER;
                while step < next.start().get() && ticks.len() < 4096 {
                    ticks.push(step);
                    step += TICKS_PER_QUARTER;
                }
            }
        }
        let last = ticks.iter().copied().max().unwrap_or(0);
        ticks.push(last + TICKS_PER_QUARTER);
        ticks.sort_unstable();
        ticks.dedup();
        let quarters = |tick: i64| tick as f64 / TICKS_PER_QUARTER as f64;
        Self {
            tempo: ticks
                .iter()
                .map(|&tick| sys::ARAContentTempoEntry {
                    timePosition: tempo.ticks_to_seconds(Ticks(tick)),
                    quarterPosition: quarters(tick),
                })
                .collect(),
            bars: signatures
                .changes()
                .map(|change| sys::ARAContentBarSignature {
                    numerator: i32::from(change.signature.numerator()),
                    denominator: i32::from(change.signature.denominator()),
                    position: quarters(change.at.get()),
                })
                .collect(),
        }
    }

    /// The tempo entries as (seconds, quarters).
    #[cfg(test)]
    pub(crate) fn tempo_entries(&self) -> Vec<(f64, f64)> {
        self.tempo
            .iter()
            .map(|entry| {
                let entry = *entry;
                (entry.timePosition, entry.quarterPosition)
            })
            .collect()
    }
}

/// What one document controller's host interfaces share. It never moves
/// once the controller exists: every host reference is a pointer to it.
pub(crate) struct ControllerHost {
    pub(crate) stats: Stats,
    content: Mutex<Content>,
    requests: Mutex<Vec<PlaybackRequest>>,
    /// The archive the plug-in reads while it restores, and the ID it was
    /// written with.
    archive: Mutex<Option<(Vec<u8>, CString)>>,
    /// What the plug-in has written while it stores.
    written: Mutex<Vec<u8>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ControllerHost {
    pub(crate) fn new() -> Self {
        Self {
            stats: Stats::default(),
            content: Mutex::new(Content::default()),
            requests: Mutex::new(Vec::new()),
            archive: Mutex::new(None),
            written: Mutex::new(Vec::new()),
        }
    }

    /// Gives the content readers the set's tempo map and signatures.
    pub(crate) fn set_content(&self, content: Content) {
        *lock(&self.content) = content;
    }

    /// Takes what the plug-in asked of the transport since the last call.
    pub(crate) fn take_requests(&self) -> Vec<PlaybackRequest> {
        std::mem::take(&mut *lock(&self.requests))
    }

    /// Offers `bytes`, written with the archive ID `id`, to a plug-in that is
    /// about to restore.
    pub(crate) fn offer_archive(&self, bytes: Vec<u8>, id: &str) {
        *lock(&self.archive) = CString::new(id).ok().map(|id| (bytes, id));
    }

    /// Takes back what was offered.
    pub(crate) fn withdraw_archive(&self) {
        *lock(&self.archive) = None;
    }

    /// Starts collecting what the plug-in writes.
    pub(crate) fn begin_writing(&self) {
        lock(&self.written).clear();
    }

    /// What the plug-in wrote.
    pub(crate) fn take_written(&self) -> Vec<u8> {
        std::mem::take(&mut *lock(&self.written))
    }

    /// The host references of this controller, for the host instance.
    pub(crate) fn instance(&self) -> sys::ARADocumentControllerHostInstance {
        let this = std::ptr::from_ref(self).cast_mut();
        sys::ARADocumentControllerHostInstance {
            structSize: size_of::<sys::ARADocumentControllerHostInstance>(),
            audioAccessControllerHostRef: this.cast(),
            audioAccessControllerInterface: &raw const AUDIO_ACCESS,
            archivingControllerHostRef: this.cast(),
            archivingControllerInterface: &raw const ARCHIVING,
            contentAccessControllerHostRef: this.cast(),
            contentAccessControllerInterface: &raw const CONTENT_ACCESS,
            modelUpdateControllerHostRef: this.cast(),
            modelUpdateControllerInterface: &raw const MODEL_UPDATE,
            playbackControllerHostRef: this.cast(),
            playbackControllerInterface: &raw const PLAYBACK,
        }
    }
}

/// The audio of one file, which readers read. A source the plug-in has
/// deactivated for the undo history holds none.
pub(crate) struct SourceHost {
    pub(crate) audio: Mutex<Option<Arc<SampleData>>>,
}

impl SourceHost {
    pub(crate) fn new(audio: Arc<SampleData>) -> Self {
        Self {
            audio: Mutex::new(Some(audio)),
        }
    }
}

/// One audio reader a plug-in opened.
struct Reader {
    audio: Arc<SampleData>,
    wide: bool,
}

/// One content reader a plug-in opened.
enum ContentReader {
    Tempo(Vec<sys::ARAContentTempoEntry>),
    Bars(Vec<sys::ARAContentBarSignature>),
}

/// The controller's host, from a host reference.
///
/// # Safety
///
/// `reference` is a host reference [`ControllerHost::instance`] gave, and the
/// controller is alive: the plug-in only calls with the references it was
/// given, until the controller is destroyed.
unsafe fn host<'a, T>(reference: *mut T) -> &'a ControllerHost {
    // SAFETY: as the function's contract says.
    unsafe { &*reference.cast::<ControllerHost>() }
}

// --- Audio access ---------------------------------------------------------

static AUDIO_ACCESS: sys::ARAAudioAccessControllerInterface =
    sys::ARAAudioAccessControllerInterface {
        structSize: size_of::<sys::ARAAudioAccessControllerInterface>(),
        createAudioReaderForSource: Some(create_audio_reader),
        readAudioSamples: Some(read_audio_samples),
        destroyAudioReader: Some(destroy_audio_reader),
    };

unsafe extern "C" fn create_audio_reader(
    controller: sys::ARAAudioAccessControllerHostRef,
    source: sys::ARAAudioSourceHostRef,
    use_64_bit_samples: sys::ARABool,
) -> sys::ARAAudioReaderHostRef {
    // SAFETY: the references are ones the host gave: the controller's, and a
    // source host the controller keeps alive until the plug-in's source is
    // destroyed.
    let (host, source) = unsafe { (host(controller), &*source.cast::<SourceHost>()) };
    let Some(audio) = lock(&source.audio).clone() else {
        return std::ptr::null_mut();
    };
    host.stats.readers_opened.fetch_add(1, Relaxed);
    let reader = Box::new(Reader {
        audio,
        wide: use_64_bit_samples != 0,
    });
    Box::into_raw(reader).cast()
}

unsafe extern "C" fn read_audio_samples(
    controller: sys::ARAAudioAccessControllerHostRef,
    reader: sys::ARAAudioReaderHostRef,
    position: sys::ARASamplePosition,
    count: sys::ARASampleCount,
    buffers: *const *mut c_void,
) -> sys::ARABool {
    // SAFETY: as above; the plug-in reads with one reader from one thread at
    // a time, as the interface asks.
    let (host, reader) = unsafe { (host(controller), &*reader.cast::<Reader>()) };
    host.stats.reader_calls.fetch_add(1, Relaxed);
    if RENDERING.with(Cell::get) {
        host.stats.audio_thread_reads.fetch_add(1, Relaxed);
    }
    let Ok(count) = usize::try_from(count) else {
        return 0;
    };
    host.stats
        .frames_read
        .fetch_add(count as u64, Relaxed);
    for (channel, data) in reader.audio.channels().enumerate() {
        // SAFETY: the plug-in gives a buffer of `count` samples for each of
        // the source's channels.
        let out = unsafe { *buffers.add(channel) };
        // The part of the request inside the file; the rest is silence.
        let from = usize::try_from(position.max(0)).unwrap_or(usize::MAX);
        let skipped = usize::try_from(-position.min(0)).unwrap_or(usize::MAX).min(count);
        let have = data.get(from.min(data.len())..).unwrap_or_default();
        let copied = have.len().min(count - skipped);
        if reader.wide {
            // SAFETY: a 64-bit reader's buffers hold doubles.
            let out = unsafe { slice::from_raw_parts_mut(out.cast::<f64>(), count) };
            out.fill(0.0);
            for (to, from) in out[skipped..skipped + copied].iter_mut().zip(have) {
                *to = f64::from(*from);
            }
        } else {
            // SAFETY: a 32-bit reader's buffers hold floats.
            let out = unsafe { slice::from_raw_parts_mut(out.cast::<f32>(), count) };
            out.fill(0.0);
            out[skipped..skipped + copied].copy_from_slice(&have[..copied]);
        }
    }
    1
}

unsafe extern "C" fn destroy_audio_reader(
    _controller: sys::ARAAudioAccessControllerHostRef,
    reader: sys::ARAAudioReaderHostRef,
) {
    // SAFETY: made by `create_audio_reader`, destroyed once.
    drop(unsafe { Box::from_raw(reader.cast::<Reader>()) });
}

// --- Archiving ------------------------------------------------------------

static ARCHIVING: sys::ARAArchivingControllerInterface = sys::ARAArchivingControllerInterface {
    structSize: size_of::<sys::ARAArchivingControllerInterface>(),
    getArchiveSize: Some(archive_size),
    readBytesFromArchive: Some(read_archive),
    writeBytesToArchive: Some(write_archive),
    notifyDocumentArchivingProgress: Some(archiving_progress),
    notifyDocumentUnarchivingProgress: Some(archiving_progress),
    getDocumentArchiveID: Some(archive_id),
};

unsafe extern "C" fn archive_size(
    controller: sys::ARAArchivingControllerHostRef,
    _reader: sys::ARAArchiveReaderHostRef,
) -> sys::ARASize {
    // SAFETY: the controller's reference.
    let host = unsafe { host(controller) };
    lock(&host.archive).as_ref().map_or(0, |(bytes, _)| bytes.len())
}

unsafe extern "C" fn read_archive(
    controller: sys::ARAArchivingControllerHostRef,
    _reader: sys::ARAArchiveReaderHostRef,
    position: sys::ARASize,
    length: sys::ARASize,
    buffer: *mut sys::ARAByte,
) -> sys::ARABool {
    // SAFETY: the controller's reference.
    let host = unsafe { host(controller) };
    let archive = lock(&host.archive);
    let Some((bytes, _)) = archive.as_ref() else {
        return 0;
    };
    let Some(part) = position
        .checked_add(length)
        .and_then(|end| bytes.get(position..end))
    else {
        return 0;
    };
    // SAFETY: the plug-in gives a buffer of `length` bytes.
    unsafe { slice::from_raw_parts_mut(buffer, length) }.copy_from_slice(part);
    1
}

unsafe extern "C" fn write_archive(
    controller: sys::ARAArchivingControllerHostRef,
    _writer: sys::ARAArchiveWriterHostRef,
    position: sys::ARASize,
    length: sys::ARASize,
    buffer: *const sys::ARAByte,
) -> sys::ARABool {
    // SAFETY: the controller's reference.
    let host = unsafe { host(controller) };
    let Some(end) = position.checked_add(length) else {
        return 0;
    };
    let mut written = lock(&host.written);
    if written.len() < end {
        written.resize(end, 0);
    }
    // SAFETY: the plug-in gives `length` bytes.
    written[position..end].copy_from_slice(unsafe { slice::from_raw_parts(buffer, length) });
    1
}

unsafe extern "C" fn archiving_progress(_controller: sys::ARAArchivingControllerHostRef, _value: f32) {}

unsafe extern "C" fn archive_id(
    controller: sys::ARAArchivingControllerHostRef,
    _reader: sys::ARAArchiveReaderHostRef,
) -> sys::ARAPersistentID {
    // SAFETY: the controller's reference.
    let host = unsafe { host(controller) };
    // The string lives as long as the archive is offered, which outlasts the
    // restore call this is asked from.
    lock(&host.archive)
        .as_ref()
        .map_or(std::ptr::null(), |(_, id)| id.as_ptr().cast::<c_char>())
}

// --- Content access -------------------------------------------------------

static CONTENT_ACCESS: sys::ARAContentAccessControllerInterface =
    sys::ARAContentAccessControllerInterface {
        structSize: size_of::<sys::ARAContentAccessControllerInterface>(),
        isMusicalContextContentAvailable: Some(context_content_available),
        getMusicalContextContentGrade: Some(context_content_grade),
        createMusicalContextContentReader: Some(create_context_reader),
        isAudioSourceContentAvailable: Some(source_content_available),
        getAudioSourceContentGrade: Some(source_content_grade),
        createAudioSourceContentReader: Some(create_source_reader),
        getContentReaderEventCount: Some(reader_event_count),
        getContentReaderDataForEvent: Some(reader_event),
        destroyContentReader: Some(destroy_content_reader),
    };

unsafe extern "C" fn context_content_available(
    _controller: sys::ARAContentAccessControllerHostRef,
    _context: sys::ARAMusicalContextHostRef,
    content: sys::ARAContentType,
) -> sys::ARABool {
    sys::ARABool::from(
        content == sys::kARAContentTypeTempoEntries || content == sys::kARAContentTypeBarSignatures,
    )
}

unsafe extern "C" fn context_content_grade(
    _controller: sys::ARAContentAccessControllerHostRef,
    _context: sys::ARAMusicalContextHostRef,
    _content: sys::ARAContentType,
) -> sys::ARAContentGrade {
    sys::kARAContentGradeApproved
}

unsafe extern "C" fn create_context_reader(
    controller: sys::ARAContentAccessControllerHostRef,
    _context: sys::ARAMusicalContextHostRef,
    content: sys::ARAContentType,
    _range: *const sys::ARAContentTimeRange,
) -> sys::ARAContentReaderHostRef {
    // SAFETY: the controller's reference.
    let host = unsafe { host(controller) };
    host.stats.content_reads.fetch_add(1, Relaxed);
    let held = lock(&host.content);
    let reader = if content == sys::kARAContentTypeTempoEntries {
        ContentReader::Tempo(held.tempo.clone())
    } else if content == sys::kARAContentTypeBarSignatures {
        ContentReader::Bars(held.bars.clone())
    } else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(reader)).cast()
}

unsafe extern "C" fn source_content_available(
    _controller: sys::ARAContentAccessControllerHostRef,
    _source: sys::ARAAudioSourceHostRef,
    _content: sys::ARAContentType,
) -> sys::ARABool {
    0
}

unsafe extern "C" fn source_content_grade(
    _controller: sys::ARAContentAccessControllerHostRef,
    _source: sys::ARAAudioSourceHostRef,
    _content: sys::ARAContentType,
) -> sys::ARAContentGrade {
    sys::kARAContentGradeInitial
}

unsafe extern "C" fn create_source_reader(
    _controller: sys::ARAContentAccessControllerHostRef,
    _source: sys::ARAAudioSourceHostRef,
    _content: sys::ARAContentType,
    _range: *const sys::ARAContentTimeRange,
) -> sys::ARAContentReaderHostRef {
    std::ptr::null_mut()
}

unsafe extern "C" fn reader_event_count(
    _controller: sys::ARAContentAccessControllerHostRef,
    reader: sys::ARAContentReaderHostRef,
) -> sys::ARAInt32 {
    // SAFETY: made by `create_context_reader`, not yet destroyed.
    let count = match unsafe { &*reader.cast::<ContentReader>() } {
        ContentReader::Tempo(entries) => entries.len(),
        ContentReader::Bars(bars) => bars.len(),
    };
    sys::ARAInt32::try_from(count).unwrap_or(sys::ARAInt32::MAX)
}

unsafe extern "C" fn reader_event(
    _controller: sys::ARAContentAccessControllerHostRef,
    reader: sys::ARAContentReaderHostRef,
    index: sys::ARAInt32,
) -> *const c_void {
    let Ok(index) = usize::try_from(index) else {
        return std::ptr::null();
    };
    // SAFETY: made by `create_context_reader`, not yet destroyed.
    match unsafe { &*reader.cast::<ContentReader>() } {
        ContentReader::Tempo(entries) => entries.get(index).map(|entry| std::ptr::from_ref(entry).cast()),
        ContentReader::Bars(bars) => bars.get(index).map(|bar| std::ptr::from_ref(bar).cast()),
    }
    .unwrap_or(std::ptr::null())
}

unsafe extern "C" fn destroy_content_reader(
    _controller: sys::ARAContentAccessControllerHostRef,
    reader: sys::ARAContentReaderHostRef,
) {
    // SAFETY: made by `create_context_reader`, destroyed once.
    drop(unsafe { Box::from_raw(reader.cast::<ContentReader>()) });
}

// --- Model updates --------------------------------------------------------

static MODEL_UPDATE: sys::ARAModelUpdateControllerInterface =
    sys::ARAModelUpdateControllerInterface {
        structSize: size_of::<sys::ARAModelUpdateControllerInterface>(),
        notifyAudioSourceAnalysisProgress: Some(analysis_progress),
        notifyAudioSourceContentChanged: Some(source_content_changed),
        notifyAudioModificationContentChanged: Some(modification_content_changed),
        notifyPlaybackRegionContentChanged: Some(region_content_changed),
    };

unsafe extern "C" fn analysis_progress(
    controller: sys::ARAModelUpdateControllerHostRef,
    _source: sys::ARAAudioSourceHostRef,
    _state: sys::ARAAnalysisProgressState,
    _value: f32,
) {
    // SAFETY: the controller's reference.
    unsafe { host(controller) }.stats.model_notifications.fetch_add(1, Relaxed);
}

unsafe extern "C" fn source_content_changed(
    controller: sys::ARAModelUpdateControllerHostRef,
    _source: sys::ARAAudioSourceHostRef,
    _range: *const sys::ARAContentTimeRange,
    _flags: sys::ARAContentUpdateFlags,
) {
    // SAFETY: the controller's reference.
    unsafe { host(controller) }.stats.model_notifications.fetch_add(1, Relaxed);
}

unsafe extern "C" fn modification_content_changed(
    controller: sys::ARAModelUpdateControllerHostRef,
    _modification: sys::ARAAudioModificationHostRef,
    _range: *const sys::ARAContentTimeRange,
    _flags: sys::ARAContentUpdateFlags,
) {
    // SAFETY: the controller's reference.
    unsafe { host(controller) }.stats.model_notifications.fetch_add(1, Relaxed);
}

unsafe extern "C" fn region_content_changed(
    controller: sys::ARAModelUpdateControllerHostRef,
    _region: sys::ARAPlaybackRegionHostRef,
    _range: *const sys::ARAContentTimeRange,
    _flags: sys::ARAContentUpdateFlags,
) {
    // SAFETY: the controller's reference.
    unsafe { host(controller) }.stats.model_notifications.fetch_add(1, Relaxed);
}

// --- Playback -------------------------------------------------------------

static PLAYBACK: sys::ARAPlaybackControllerInterface = sys::ARAPlaybackControllerInterface {
    structSize: size_of::<sys::ARAPlaybackControllerInterface>(),
    requestStartPlayback: Some(request_start),
    requestStopPlayback: Some(request_stop),
    requestSetPlaybackPosition: Some(request_locate),
    requestSetCycleRange: Some(request_cycle_range),
    requestEnableCycle: Some(request_cycle),
};

fn request(controller: sys::ARAPlaybackControllerHostRef, request: PlaybackRequest) {
    // SAFETY: the controller's reference.
    lock(&unsafe { host(controller) }.requests).push(request);
}

unsafe extern "C" fn request_start(controller: sys::ARAPlaybackControllerHostRef) {
    request(controller, PlaybackRequest::Start);
}

unsafe extern "C" fn request_stop(controller: sys::ARAPlaybackControllerHostRef) {
    request(controller, PlaybackRequest::Stop);
}

unsafe extern "C" fn request_locate(
    controller: sys::ARAPlaybackControllerHostRef,
    time: sys::ARATimePosition,
) {
    request(controller, PlaybackRequest::Locate(time));
}

unsafe extern "C" fn request_cycle_range(
    controller: sys::ARAPlaybackControllerHostRef,
    start: sys::ARATimePosition,
    duration: sys::ARATimeDuration,
) {
    request(controller, PlaybackRequest::CycleRange { start, duration });
}

unsafe extern "C" fn request_cycle(
    controller: sys::ARAPlaybackControllerHostRef,
    enable: sys::ARABool,
) {
    request(controller, PlaybackRequest::Cycle(enable != 0));
}

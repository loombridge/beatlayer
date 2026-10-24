//! ARA 2 hosting (IMP-20; the ARA hosting spec): a plug-in that implements
//! Celemony's Audio Random Access sees a whole clip's audio, not a live
//! stream, and renders the clips of its track itself.
//!
//! - [`model`]: what the plug-in is told about the set, worked out from the
//!   document alone (R2 to R5).
//! - [`Controller`]: one plug-in's document controller, which the model is
//!   applied to as edit cycles on the main thread (R1, R2, R6).
//! - [`host`]: the interfaces the plug-in calls back into (audio readers
//!   that never run on the audio thread, archives, the tempo and signatures,
//!   the transport requests).
//!
//! The VST3 side, which binds an instance to a controller and keeps its
//! renderer's regions, is `vst3::ara`. Audio Unit and CLAP ARA hosting are
//! not built (the spec says why).

mod controller;
mod host;
pub mod model;

use std::sync::Arc;

use bl_ara_sys as sys;
use bl_media::SampleData;
use bl_model::{ContentHash, DeviceId, TrackId};

pub(crate) use controller::Controller;
pub use controller::Factory;
pub(crate) use controller::describe;
pub use host::PlaybackRequest;
pub(crate) use host::Rendering;
pub use model::Region as AraRegion;

/// Where the audio of the set's files comes from: the media store, or in
/// tests a fixed set of files. Its methods run on the main thread, and the
/// audio it returns is read on the plug-in's own threads.
pub trait AraMedia: Send + Sync {
    /// The file's audio, decoded at the file's own rate, or `None` if it
    /// can't be read.
    fn audio(&self, media: ContentHash) -> Option<Arc<SampleData>>;

    /// How many seconds the file plays. The default decodes it.
    fn seconds(&self, media: ContentHash) -> Option<f64> {
        self.audio(media).map(|audio| audio.duration_seconds())
    }
}

impl AraMedia for bl_media::MediaStore {
    fn audio(&self, media: ContentHash) -> Option<Arc<SampleData>> {
        self.load(bl_media::ContentHash::from_bytes(media.0)).ok()
    }

    fn seconds(&self, media: ContentHash) -> Option<f64> {
        let info = self.info(bl_media::ContentHash::from_bytes(media.0)).ok()?;
        match info.frames {
            Some(frames) => Some(frames as f64 / f64::from(info.sample_rate)),
            None => self.audio(media).map(|audio| audio.duration_seconds()),
        }
    }
}

/// What one ARA plug-in's document controller holds and has done, for
/// `ara.state`.
#[derive(Clone, Debug, PartialEq)]
pub struct AraStatus {
    /// The plug-in's ARA factory ID.
    pub plugin: String,
    /// Its name.
    pub name: String,
    /// Whether it plays regions at another speed than their audio's.
    pub timestretch: bool,
    /// The devices bound to the controller, each with the track it renders
    /// (`None` while it renders none).
    pub devices: Vec<(DeviceId, Option<TrackId>)>,
    /// The region sequences, audio sources and audio modifications it holds.
    pub sequences: usize,
    pub sources: usize,
    pub modifications: usize,
    /// Its playback regions, in order of clip.
    pub regions: Vec<AraRegion>,
    /// How many edit cycles the set has been given in.
    pub edit_cycles: u64,
    /// Audio readers the plug-in opened, calls to read, and frames read.
    pub readers_opened: u64,
    pub reader_calls: u64,
    pub frames_read: u64,
    /// Reads that came from inside a render call: 0 unless the plug-in reads
    /// on the audio thread (R7).
    pub audio_thread_reads: u64,
    /// Files it couldn't be given because they couldn't be read.
    pub missing: Vec<ContentHash>,
    /// Whether it restored an archive when it was made, and why not.
    pub restored: Option<Result<(), String>>,
}

/// The plug-in's editor view, which the Arrangement's selection is told to
/// (R12).
pub(crate) struct EditorView {
    reference: sys::ARAEditorViewRef,
    interface: sys::ARAEditorViewInterface,
}

impl EditorView {
    /// Tells the view what is selected.
    ///
    /// # Safety
    ///
    /// `selection` points to a valid selection for the call.
    pub(crate) unsafe fn notify_selection(&self, selection: *const sys::ARAViewSelection) {
        if let Some(notify) = self.interface.notifySelection {
            // SAFETY: the plug-in's function with its own reference.
            unsafe { notify(self.reference, selection) };
        }
    }
}

/// An instance's ARA extension: the renderers and the editor view the
/// plug-in made when it was bound to a document controller.
pub(crate) struct Extension {
    playback: Option<(
        sys::ARAPlaybackRendererRef,
        sys::ARAPlaybackRendererInterface,
    )>,
    editor: Option<(sys::ARAEditorRendererRef, sys::ARAEditorRendererInterface)>,
    pub(crate) view: Option<EditorView>,
}

/// A copy of an interface the plug-in published, if it published at least
/// all that we use of it.
///
/// # Safety
///
/// `published` is null or points to a plug-in's interface struct that starts
/// with its own size.
unsafe fn copy<T>(published: *const T) -> Option<T> {
    if published.is_null() {
        return None;
    }
    // SAFETY: the contract; every interface struct starts with its size.
    let size = unsafe { published.cast::<usize>().read_unaligned() };
    // SAFETY: only read when the plug-in's struct is at least as big as ours.
    (size >= std::mem::size_of::<T>()).then(|| unsafe { published.read_unaligned() })
}

impl Extension {
    /// Reads the extension instance a plug-in gave when it was bound.
    ///
    /// # Safety
    ///
    /// `instance` is what `bindToDocumentControllerWithRoles` returned, and
    /// stays valid until the plug-in instance is destroyed.
    pub(crate) unsafe fn of(instance: *const sys::ARAPlugInExtensionInstance) -> Option<Self> {
        if instance.is_null() {
            return None;
        }
        // SAFETY: the contract.
        let instance = unsafe { instance.read_unaligned() };
        // SAFETY: the interfaces the plug-in published for this instance.
        unsafe {
            Some(Self {
                playback: copy(instance.playbackRendererInterface)
                    .map(|interface| (instance.playbackRendererRef, interface)),
                editor: copy(instance.editorRendererInterface)
                    .map(|interface| (instance.editorRendererRef, interface)),
                view: copy(instance.editorViewInterface).map(|interface| EditorView {
                    reference: instance.editorViewRef,
                    interface,
                }),
            })
        }
    }

    /// Gives the playback renderer a region to render. The plug-in must not
    /// be active.
    pub(crate) fn add_region(&self, region: sys::ARAPlaybackRegionRef) {
        if let Some((reference, interface)) = &self.playback
            && let Some(add) = interface.addPlaybackRegion
        {
            // SAFETY: the plug-in's function with its own renderer.
            unsafe { add(*reference, region) };
        }
    }

    /// Takes a region away from the playback renderer.
    pub(crate) fn remove_region(&self, region: sys::ARAPlaybackRegionRef) {
        if let Some((reference, interface)) = &self.playback
            && let Some(remove) = interface.removePlaybackRegion
        {
            // SAFETY: as above.
            unsafe { remove(*reference, region) };
        }
    }

    /// Gives the editor renderer a track's whole sequence of regions.
    pub(crate) fn add_sequence(&self, sequence: sys::ARARegionSequenceRef) {
        if let Some((reference, interface)) = &self.editor
            && let Some(add) = interface.addRegionSequence
        {
            // SAFETY: as above.
            unsafe { add(*reference, sequence) };
        }
    }

    /// Takes a sequence away from the editor renderer.
    pub(crate) fn remove_sequence(&self, sequence: sys::ARARegionSequenceRef) {
        if let Some((reference, interface)) = &self.editor
            && let Some(remove) = interface.removeRegionSequence
        {
            // SAFETY: as above.
            unsafe { remove(*reference, sequence) };
        }
    }
}

//! One ARA document controller, as the host drives it (the ARA hosting spec,
//! R1 to R6): created from a plug-in's factory with the host's interfaces,
//! given the set's [`Model`] as edit cycles on the main thread, and
//! destroyed with the set.
//!
//! The controller keeps every object it made: sequences, sources,
//! modifications and playback regions, each with the pointer the plug-in
//! gave and the box whose address is the plug-in's host reference. A
//! modification (and the source it needs) that leaves the set stays in the
//! plug-in, deactivated, so undo brings it back with the plug-in's edits
//! (R6).

use std::collections::{BTreeMap, VecDeque};
use std::ffi::CString;
use std::mem::size_of;
use std::ptr;
use std::sync::Arc;

use bl_ara_sys as sys;
use bl_model::{AraSupport, ClipId, ContentHash, TrackId};

use super::AraMedia;
use super::host::{Content, ControllerHost, PlaybackRequest, SourceHost};
use super::model::{Model, Region, RegionKey};

/// How many removed modifications a controller keeps deactivated, for undo.
const GRAVEYARD: usize = 128;

/// Calls a function of the plug-in's document controller interface, or
/// returns an error saying which one it lacks.
macro_rules! call {
    ($this:expr, $function:ident($($argument:expr),* $(,)?)) => {{
        let function = $this.interface.$function;
        match function {
            // SAFETY: the plug-in published the function for this document
            // controller; the arguments are references it gave or
            // properties that live through the call.
            Some(function) => unsafe { function($this.document, $($argument),*) },
            None => {
                return Err(format!(
                    "the plug-in's document controller has no {}",
                    stringify!($function)
                ))
            }
        }
    }};
}

/// An ARA plug-in as the factory describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Factory {
    /// The factory's ID, which names the plug-in's document controller class.
    pub id: String,
    /// The plug-in's name, for display.
    pub name: String,
    /// What it does with the regions it is given.
    pub support: AraSupport,
    /// The ID of the archives it writes.
    pub archive_id: String,
    /// The IDs of older archives it can read.
    pub compatible_archive_ids: Vec<String>,
}

/// Reads a factory's description.
///
/// # Safety
///
/// `factory` points to a live `ARAFactory`.
pub(crate) unsafe fn describe(factory: *const sys::ARAFactory) -> Result<Factory, String> {
    // SAFETY: as the function's contract says; the struct may be packed, so
    // it is copied out unaligned.
    let found = unsafe { factory.read_unaligned() };
    if found.structSize < sys::kARAFactoryMinSize {
        return Err("it is not an ARA 2 plug-in".to_owned());
    }
    if found.lowestSupportedApiGeneration > sys::kARAAPIGeneration_2_0_Final
        || found.highestSupportedApiGeneration < sys::kARAAPIGeneration_2_0_Final
    {
        return Err("it doesn't support ARA 2.0".to_owned());
    }
    // SAFETY: the factory's strings are valid while its binary is loaded.
    let text = |string: *const std::ffi::c_char| unsafe {
        if string.is_null() {
            String::new()
        } else {
            std::ffi::CStr::from_ptr(string)
                .to_string_lossy()
                .into_owned()
        }
    };
    let compatible = (0..found.compatibleDocumentArchiveIDsCount)
        .map(|at| {
            // SAFETY: the factory publishes that many archive IDs.
            text(unsafe { *found.compatibleDocumentArchiveIDs.add(at) })
        })
        .collect();
    Ok(Factory {
        id: text(found.factoryID),
        name: text(found.plugInName),
        support: AraSupport {
            timestretch: found.supportedPlaybackTransformationFlags
                & sys::kARAPlaybackTransformationTimestretch
                != 0,
        },
        archive_id: text(found.documentArchiveID),
        compatible_archive_ids: compatible,
    })
}

/// A host reference's pointee: only its address matters to the plug-in.
struct Tag(#[expect(dead_code, reason = "the box is the reference")] u8);

fn tag() -> Box<Tag> {
    Box::new(Tag(0))
}

struct Sequence {
    reference: sys::ARARegionSequenceRef,
    _host: Box<Tag>,
    name: String,
    order: i32,
}

struct Source {
    reference: sys::ARAAudioSourceRef,
    host: Box<SourceHost>,
}

struct Modification {
    reference: sys::ARAAudioModificationRef,
    _host: Box<Tag>,
    media: ContentHash,
    name: String,
}

struct PlacedRegion {
    reference: sys::ARAPlaybackRegionRef,
    _host: Box<Tag>,
    region: Region,
}

/// One ARA document controller.
pub(crate) struct Controller {
    pub(crate) factory: Factory,
    pub(crate) host: Box<ControllerHost>,
    /// The plug-in's document controller instance, and its interface copied
    /// out of it (its size may be smaller than ours).
    document: sys::ARADocumentControllerRef,
    interface: sys::ARADocumentControllerInterface,
    _host_instance: Box<sys::ARADocumentControllerHostInstance>,
    media: Arc<dyn AraMedia>,
    /// The factory to shut down with the controller.
    raw_factory: *const sys::ARAFactory,
    context: Option<(sys::ARAMusicalContextRef, Box<Tag>)>,
    sequences: BTreeMap<TrackId, Sequence>,
    sources: BTreeMap<ContentHash, Source>,
    modifications: BTreeMap<ClipId, Modification>,
    regions: BTreeMap<RegionKey, PlacedRegion>,
    /// Removed modifications, oldest first, and the sources they need.
    dead_modifications: VecDeque<ClipId>,
    dead_clips: BTreeMap<ClipId, Modification>,
    dead_sources: BTreeMap<ContentHash, Source>,
    /// What the plug-in was last told about the tempo and signatures.
    applied: Option<Model>,
    /// The archive the set held for this plug-in, restored in the first edit
    /// cycle, and whether it was.
    restore: Option<(Vec<u8>, String)>,
    pub(crate) restored: Option<Result<(), String>>,
    pub(crate) edit_cycles: u64,
    /// Sources whose file couldn't be read, which have no objects.
    pub(crate) missing: Vec<ContentHash>,
}

impl Controller {
    /// Starts ARA in the plug-in and makes a document controller with the
    /// host's interfaces.
    ///
    /// # Safety
    ///
    /// `factory` points to a live `ARAFactory`, which stays valid until the
    /// controller is dropped, and the controller is made and dropped on the
    /// thread the plug-in treats as its main thread.
    pub(crate) unsafe fn create(
        factory: *const sys::ARAFactory,
        media: Arc<dyn AraMedia>,
        restore: Option<(Vec<u8>, String)>,
    ) -> Result<Self, String> {
        // SAFETY: the contract.
        let description = unsafe { describe(factory) }?;
        // SAFETY: the contract.
        let raw = unsafe { factory.read_unaligned() };
        let (Some(initialize), Some(uninitialize), Some(create)) = (
            raw.initializeARAWithConfiguration,
            raw.uninitializeARA,
            raw.createDocumentControllerWithDocument,
        ) else {
            return Err("its ARA factory is incomplete".to_owned());
        };
        // The assert function address must stay valid until ARA is shut
        // down, and may point at nothing.
        static NO_ASSERT: Assert = Assert(std::cell::UnsafeCell::new(None));
        let configuration = sys::ARAInterfaceConfiguration {
            structSize: size_of::<sys::ARAInterfaceConfiguration>(),
            desiredApiGeneration: sys::kARAAPIGeneration_2_0_Final,
            assertFunctionAddress: NO_ASSERT.0.get(),
        };
        // SAFETY: a valid configuration, as the factory asks.
        unsafe { initialize(&raw const configuration) };
        let host = Box::new(ControllerHost::new());
        let host_instance = Box::new(host.instance());
        let name = CString::new("Beatlayer set").unwrap_or_default();
        let properties = sys::ARADocumentProperties {
            structSize: size_of::<sys::ARADocumentProperties>(),
            name: name.as_ptr(),
        };
        // SAFETY: valid host interfaces, which live as long as the
        // controller; the properties are copied during the call.
        let instance = unsafe { create(&raw const *host_instance, &raw const properties) };
        if instance.is_null() {
            // SAFETY: balances the initialize above.
            unsafe { uninitialize() };
            return Err("the plug-in made no document controller".to_owned());
        }
        // SAFETY: the plug-in's instance, valid until the controller is
        // destroyed.
        let instance = unsafe { instance.read_unaligned() };
        if instance.documentControllerInterface.is_null() {
            // SAFETY: balances the initialize above.
            unsafe { uninitialize() };
            return Err("the plug-in's document controller has no interface".to_owned());
        }
        // SAFETY: every field is a number or an optional function pointer, for
        // which all zeros is valid: an empty interface.
        let mut interface: sys::ARADocumentControllerInterface = unsafe { std::mem::zeroed() };
        // SAFETY: the plug-in's interface is `structSize` bytes of function
        // pointers; only that many are copied, the rest stay empty.
        unsafe {
            let published = instance.documentControllerInterface;
            let size = published
                .read_unaligned()
                .structSize
                .min(size_of::<sys::ARADocumentControllerInterface>());
            ptr::copy_nonoverlapping(
                published.cast::<u8>(),
                ptr::from_mut(&mut interface).cast::<u8>(),
                size,
            );
        }
        Ok(Self {
            factory: description,
            host,
            document: instance.documentControllerRef,
            interface,
            _host_instance: host_instance,
            media,
            raw_factory: factory,
            context: None,
            sequences: BTreeMap::new(),
            sources: BTreeMap::new(),
            modifications: BTreeMap::new(),
            regions: BTreeMap::new(),
            dead_modifications: VecDeque::new(),
            dead_clips: BTreeMap::new(),
            dead_sources: BTreeMap::new(),
            applied: None,
            restore,
            restored: None,
            edit_cycles: 0,
            missing: Vec::new(),
        })
    }

    /// The pointer the plug-in's extension binds to.
    pub(crate) fn document(&self) -> sys::ARADocumentControllerRef {
        self.document
    }

    /// The plug-in's regions on `track`.
    pub(crate) fn regions_of(
        &self,
        track: TrackId,
    ) -> impl Iterator<Item = (RegionKey, sys::ARAPlaybackRegionRef)> + '_ {
        self.regions
            .iter()
            .filter(move |(_, placed)| placed.region.track == track)
            .map(|(key, placed)| (*key, placed.reference))
    }

    /// The plug-in's region sequence for `track`, if it has one.
    pub(crate) fn sequence_of(&self, track: TrackId) -> Option<sys::ARARegionSequenceRef> {
        self.sequences
            .get(&track)
            .map(|sequence| sequence.reference)
    }

    /// The region of a clip's pieces, and the sequence of its track, for a
    /// selection.
    pub(crate) fn selection(
        &self,
        clips: &[ClipId],
        tracks: &[TrackId],
    ) -> (
        Vec<sys::ARAPlaybackRegionRef>,
        Vec<sys::ARARegionSequenceRef>,
    ) {
        let regions = self
            .regions
            .iter()
            .filter(|((clip, _), _)| clips.contains(clip))
            .map(|(_, placed)| placed.reference)
            .collect();
        let sequences = tracks
            .iter()
            .filter_map(|&track| self.sequence_of(track))
            .collect();
        (regions, sequences)
    }

    /// What the controller holds, for status reports.
    pub(crate) fn counts(&self) -> (usize, usize, usize) {
        (
            self.sequences.len(),
            self.sources.len(),
            self.modifications.len(),
        )
    }

    /// The regions the plug-in has, as the model describes them.
    pub(crate) fn placed(&self) -> impl Iterator<Item = &Region> + '_ {
        self.regions.values().map(|placed| &placed.region)
    }

    /// What the plug-in asked the transport for.
    pub(crate) fn take_requests(&self) -> Vec<PlaybackRequest> {
        self.host.take_requests()
    }

    /// Whether `model` would change anything the plug-in holds.
    pub(crate) fn differs(&self, model: &Model) -> bool {
        self.applied.as_ref() != Some(model)
    }

    /// The keys of the regions `model` adds and removes, with their tracks:
    /// the membership changes a renderer has to know before the edit cycle.
    pub(crate) fn membership(&self, model: &Model) -> (Vec<TrackId>, Vec<TrackId>) {
        let added = model
            .regions
            .iter()
            .filter(|(key, _)| {
                !self
                    .regions
                    .get(*key)
                    .is_some_and(|placed| self.keeps(key, placed, model))
            })
            .map(|(_, region)| region.track)
            .collect();
        let removed = self
            .regions
            .iter()
            .filter(|(key, placed)| !self.keeps(key, placed, model))
            .map(|(_, placed)| placed.region.track)
            .collect();
        (added, removed)
    }

    /// Whether a region the plug-in holds stays in `model`: on the same
    /// track, and on its clip's same file. A clip cropped to a new file
    /// keeps its ID but gets a new audio modification, and a region can't
    /// move to another modification, so its region is made again.
    fn keeps(&self, key: &RegionKey, placed: &PlacedRegion, model: &Model) -> bool {
        let media = |clip: &ClipId| self.modifications.get(clip).map(|m| m.media);
        model
            .regions
            .get(key)
            .is_some_and(|region| region.track == placed.region.track)
            && model
                .modifications
                .get(&placed.region.clip)
                .map(|m| m.media)
                == media(&placed.region.clip)
    }

    /// Gives the plug-in `model` in one edit cycle (R2): objects that left
    /// it are destroyed or, for the audio, deactivated for the undo history;
    /// objects that are new are made, or brought back; the rest are updated
    /// where they changed. A region is destroyed only after `before_destroy`
    /// has let its renderers go of it.
    ///
    /// # Errors
    ///
    /// Why the plug-in couldn't be edited. What was done stays done.
    pub(crate) fn apply(
        &mut self,
        model: Model,
        before_destroy: &mut dyn FnMut(TrackId, sys::ARAPlaybackRegionRef),
    ) -> Result<(), String> {
        call!(self, beginEditing());
        let result = self.edit(&model, before_destroy);
        call!(self, endEditing());
        self.edit_cycles += 1;
        // Even after a failure: the plug-in is in whatever state it is in,
        // and trying the same model again at every poll would only repeat it.
        self.applied = Some(model);
        result?;
        call!(self, notifyModelUpdates());
        Ok(())
    }

    fn edit(
        &mut self,
        model: &Model,
        before_destroy: &mut dyn FnMut(TrackId, sys::ARAPlaybackRegionRef),
    ) -> Result<(), String> {
        // The musical context, with the tempo and signatures it reads.
        let content = Content::of(&model.tempo, &model.signatures);
        let moved = self
            .applied
            .as_ref()
            .is_some_and(|was| was.tempo != model.tempo || was.signatures != model.signatures);
        self.host.set_content(content);
        let context = match &self.context {
            Some((context, _)) => *context,
            None => {
                let host = tag();
                let name = c"Song";
                let properties = sys::ARAMusicalContextProperties {
                    structSize: size_of::<sys::ARAMusicalContextProperties>(),
                    name: name.as_ptr(),
                    orderIndex: 0,
                    color: ptr::null(),
                };
                let context = call!(
                    self,
                    createMusicalContext(
                        ptr::from_ref(&*host).cast_mut().cast(),
                        &raw const properties
                    )
                );
                self.context = Some((context, host));
                context
            }
        };
        if moved {
            let flags = sys::kARAContentUpdateEverythingChanged;
            call!(
                self,
                updateMusicalContextContent(context, ptr::null(), flags)
            );
        }

        // Regions that left, moved to another track or to another file go
        // first: their renderers let go of them, then they are destroyed.
        let gone: Vec<RegionKey> = self
            .regions
            .iter()
            .filter(|(key, placed)| !self.keeps(key, placed, model))
            .map(|(key, _)| *key)
            .collect();
        for key in gone {
            if let Some(placed) = self.regions.remove(&key) {
                before_destroy(placed.region.track, placed.reference);
                call!(self, destroyPlaybackRegion(placed.reference));
            }
        }

        // Modifications that left stay in the plug-in, deactivated.
        let left: Vec<ClipId> = self
            .modifications
            .iter()
            .filter(|(clip, modification)| {
                model
                    .modifications
                    .get(*clip)
                    .is_none_or(|new| new.media != modification.media)
            })
            .map(|(clip, _)| *clip)
            .collect();
        for clip in left {
            if let Some(modification) = self.modifications.remove(&clip) {
                call!(
                    self,
                    deactivateAudioModificationForUndoHistory(modification.reference, 1)
                );
                self.dead_clips.insert(clip, modification);
                self.dead_modifications.retain(|dead| *dead != clip);
                self.dead_modifications.push_back(clip);
            }
        }
        // Sources no modification needs any more go the same way.
        let unused: Vec<ContentHash> = self
            .sources
            .keys()
            .filter(|media| !model.sources.contains_key(*media))
            .copied()
            .collect();
        for media in unused {
            if let Some(source) = self.sources.remove(&media) {
                self.retire_source(media, source)?;
            }
        }
        self.trim_graveyard()?;

        // Sources, modifications and sequences that are new, or back.
        self.missing.clear();
        for (media, wanted) in &model.sources {
            if self.sources.contains_key(media) {
                continue;
            }
            let Some(audio) = self.media.audio(*media) else {
                self.missing.push(*media);
                continue;
            };
            if let Some(source) = self.dead_sources.remove(media) {
                *lock_audio(&source.host) = Some(audio);
                call!(
                    self,
                    deactivateAudioSourceForUndoHistory(source.reference, 0)
                );
                call!(self, enableAudioSourceSamplesAccess(source.reference, 1));
                self.sources.insert(*media, source);
                continue;
            }
            let source = self.create_source(*media, &wanted.name, audio)?;
            self.sources.insert(*media, source);
        }
        let want_sequences: Vec<TrackId> = model.sequences.iter().map(|s| s.track).collect();
        for wanted in &model.sequences {
            let name = CString::new(wanted.name.clone()).unwrap_or_default();
            let properties = sys::ARARegionSequenceProperties {
                structSize: size_of::<sys::ARARegionSequenceProperties>(),
                name: name.as_ptr(),
                orderIndex: wanted.order,
                musicalContextRef: context,
                color: ptr::null(),
            };
            match self.sequences.get_mut(&wanted.track) {
                Some(sequence) => {
                    if sequence.name != wanted.name || sequence.order != wanted.order {
                        call!(
                            self,
                            updateRegionSequenceProperties(
                                sequence.reference,
                                &raw const properties
                            )
                        );
                        sequence.name.clone_from(&wanted.name);
                        sequence.order = wanted.order;
                    }
                }
                None => {
                    let host = tag();
                    let reference = call!(
                        self,
                        createRegionSequence(
                            ptr::from_ref(&*host).cast_mut().cast(),
                            &raw const properties
                        )
                    );
                    self.sequences.insert(
                        wanted.track,
                        Sequence {
                            reference,
                            _host: host,
                            name: wanted.name.clone(),
                            order: wanted.order,
                        },
                    );
                }
            }
        }
        for (clip, wanted) in &model.modifications {
            let name = CString::new(wanted.name.clone()).unwrap_or_default();
            let id = CString::new(format!("clip-{}", u64::from(clip.0))).unwrap_or_default();
            let properties = sys::ARAAudioModificationProperties {
                structSize: size_of::<sys::ARAAudioModificationProperties>(),
                name: name.as_ptr(),
                persistentID: id.as_ptr(),
            };
            if let Some(existing) = self.modifications.get_mut(clip) {
                if existing.name != wanted.name {
                    call!(
                        self,
                        updateAudioModificationProperties(
                            existing.reference,
                            &raw const properties
                        )
                    );
                    existing.name.clone_from(&wanted.name);
                }
                continue;
            }
            let Some(source) = self.sources.get(&wanted.media) else {
                continue;
            };
            let source_ref = source.reference;
            if let Some(dead) = self.dead_clips.remove(clip) {
                self.dead_modifications.retain(|dead| dead != clip);
                if dead.media == wanted.media {
                    call!(
                        self,
                        deactivateAudioModificationForUndoHistory(dead.reference, 0)
                    );
                    let mut dead = dead;
                    if dead.name != wanted.name {
                        call!(
                            self,
                            updateAudioModificationProperties(
                                dead.reference,
                                &raw const properties
                            )
                        );
                        dead.name.clone_from(&wanted.name);
                    }
                    self.modifications.insert(*clip, dead);
                    continue;
                }
                call!(self, destroyAudioModification(dead.reference));
            }
            let host = tag();
            let reference = call!(
                self,
                createAudioModification(
                    source_ref,
                    ptr::from_ref(&*host).cast_mut().cast(),
                    &raw const properties
                )
            );
            self.modifications.insert(
                *clip,
                Modification {
                    reference,
                    _host: host,
                    media: wanted.media,
                    name: wanted.name.clone(),
                },
            );
        }

        // Regions: new ones are made, changed ones updated.
        for (key, wanted) in &model.regions {
            let (Some(modification), Some(sequence)) = (
                self.modifications.get(&wanted.clip),
                self.sequences.get(&wanted.track),
            ) else {
                continue;
            };
            let (modification_ref, sequence_ref) = (modification.reference, sequence.reference);
            let name = CString::new(wanted.name.clone()).unwrap_or_default();
            let properties = sys::ARAPlaybackRegionProperties {
                structSize: size_of::<sys::ARAPlaybackRegionProperties>(),
                transformationFlags: wanted.flags,
                startInModificationTime: wanted.start_in_modification,
                durationInModificationTime: wanted.duration_in_modification,
                startInPlaybackTime: wanted.start_in_playback,
                durationInPlaybackTime: wanted.duration_in_playback,
                musicalContextRef: context,
                regionSequenceRef: sequence_ref,
                name: name.as_ptr(),
                color: ptr::null(),
            };
            match self.regions.get_mut(key) {
                Some(placed) => {
                    if placed.region != *wanted {
                        call!(
                            self,
                            updatePlaybackRegionProperties(placed.reference, &raw const properties)
                        );
                        placed.region = wanted.clone();
                    }
                }
                None => {
                    let host = tag();
                    let reference = call!(
                        self,
                        createPlaybackRegion(
                            modification_ref,
                            ptr::from_ref(&*host).cast_mut().cast(),
                            &raw const properties
                        )
                    );
                    self.regions.insert(
                        *key,
                        PlacedRegion {
                            reference,
                            _host: host,
                            region: wanted.clone(),
                        },
                    );
                }
            }
        }
        // Sequences that left.
        let left: Vec<TrackId> = self
            .sequences
            .keys()
            .filter(|track| !want_sequences.contains(track))
            .copied()
            .collect();
        for track in left {
            if let Some(sequence) = self.sequences.remove(&track) {
                call!(self, destroyRegionSequence(sequence.reference));
            }
        }

        // The first cycle restores what the set saved (R10).
        if !model.modifications.is_empty()
            && let Some((bytes, id)) = self.restore.take()
        {
            self.host.offer_archive(bytes, &id);
            let done = call!(
                self,
                restoreObjectsFromArchive(
                    ptr::from_ref(&*self.host).cast_mut().cast(),
                    ptr::null()
                )
            );
            self.host.withdraw_archive();
            self.restored = Some(if done == 0 {
                Err("the plug-in couldn't read its archive".to_owned())
            } else {
                Ok(())
            });
        }
        Ok(())
    }

    fn create_source(
        &mut self,
        media: ContentHash,
        name: &str,
        audio: Arc<bl_media::SampleData>,
    ) -> Result<Source, String> {
        let cname = CString::new(name).unwrap_or_default();
        let id = CString::new(
            media
                .0
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
        )
        .unwrap_or_default();
        let properties = sys::ARAAudioSourceProperties {
            structSize: size_of::<sys::ARAAudioSourceProperties>(),
            name: cname.as_ptr(),
            persistentID: id.as_ptr(),
            sampleCount: i64::try_from(audio.frames()).unwrap_or(i64::MAX),
            sampleRate: f64::from(audio.sample_rate()),
            channelCount: i32::try_from(audio.channel_count()).unwrap_or(i32::MAX),
            merits64BitSamples: 0,
        };
        let host = Box::new(SourceHost::new(audio));
        let reference = call!(
            self,
            createAudioSource(
                ptr::from_ref(&*host).cast_mut().cast(),
                &raw const properties
            )
        );
        call!(self, enableAudioSourceSamplesAccess(reference, 1));
        Ok(Source { reference, host })
    }

    /// Deactivates a source nothing in the set needs; it stays for undo.
    fn retire_source(&mut self, media: ContentHash, source: Source) -> Result<(), String> {
        call!(self, enableAudioSourceSamplesAccess(source.reference, 0));
        call!(
            self,
            deactivateAudioSourceForUndoHistory(source.reference, 1)
        );
        // Its audio isn't needed until it comes back.
        *lock_audio(&source.host) = None;
        self.dead_sources.insert(media, source);
        Ok(())
    }

    /// Destroys the oldest removed modifications past [`GRAVEYARD`], and the
    /// deactivated sources no modification needs.
    fn trim_graveyard(&mut self) -> Result<(), String> {
        while self.dead_modifications.len() > GRAVEYARD {
            let Some(clip) = self.dead_modifications.pop_front() else {
                break;
            };
            if let Some(dead) = self.dead_clips.remove(&clip) {
                call!(self, destroyAudioModification(dead.reference));
            }
        }
        let needed: Vec<ContentHash> = self
            .dead_clips
            .values()
            .chain(self.modifications.values())
            .map(|modification| modification.media)
            .collect();
        let unneeded: Vec<ContentHash> = self
            .dead_sources
            .keys()
            .filter(|media| !needed.contains(media))
            .copied()
            .collect();
        for media in unneeded {
            if let Some(source) = self.dead_sources.remove(&media) {
                call!(self, destroyAudioSource(source.reference));
            }
        }
        Ok(())
    }

    /// The plug-in's archive of the set (R10): everything it keeps of the
    /// audio's edits, taken outside an edit cycle. `None` if it wrote none.
    pub(crate) fn archive(&mut self) -> Option<Vec<u8>> {
        let store = self.interface.storeObjectsToArchive?;
        self.host.begin_writing();
        // SAFETY: the plug-in's function, with the host's writer reference;
        // no filter stores everything.
        let done = unsafe {
            store(
                self.document,
                ptr::from_ref(&*self.host).cast_mut().cast(),
                ptr::null(),
            )
        };
        let bytes = self.host.take_written();
        (done != 0 && !bytes.is_empty()).then_some(bytes)
    }

    /// Tells an editor view what the person has selected (R12).
    ///
    /// # Safety
    ///
    /// `view` is the plug-in's editor view for this controller.
    pub(crate) unsafe fn select(
        &self,
        view: &super::EditorView,
        clips: &[ClipId],
        tracks: &[TrackId],
    ) {
        let (regions, sequences) = self.selection(clips, tracks);
        let selection = sys::ARAViewSelection {
            structSize: size_of::<sys::ARAViewSelection>(),
            playbackRegionRefsCount: regions.len(),
            playbackRegionRefs: regions.as_ptr(),
            regionSequenceRefsCount: sequences.len(),
            regionSequenceRefs: sequences.as_ptr(),
            timeRange: ptr::null(),
        };
        // SAFETY: the contract; the arrays live through the call.
        unsafe { view.notify_selection(&raw const selection) };
    }
}

fn lock_audio(host: &SourceHost) -> std::sync::MutexGuard<'_, Option<Arc<bl_media::SampleData>>> {
    host.audio
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Drop for Controller {
    fn drop(&mut self) {
        // Everything the set held goes, in the order ARA needs: regions,
        // modifications, sources, sequences, the context; then the
        // controller itself and the factory (R1).
        let _ = (|| -> Result<(), String> {
            call!(self, beginEditing());
            for placed in std::mem::take(&mut self.regions).into_values() {
                call!(self, destroyPlaybackRegion(placed.reference));
            }
            let modifications = std::mem::take(&mut self.modifications)
                .into_values()
                .chain(std::mem::take(&mut self.dead_clips).into_values());
            for modification in modifications {
                call!(self, destroyAudioModification(modification.reference));
            }
            let sources = std::mem::take(&mut self.sources)
                .into_values()
                .chain(std::mem::take(&mut self.dead_sources).into_values());
            for source in sources {
                call!(self, destroyAudioSource(source.reference));
            }
            for sequence in std::mem::take(&mut self.sequences).into_values() {
                call!(self, destroyRegionSequence(sequence.reference));
            }
            if let Some((context, _)) = self.context.take() {
                call!(self, destroyMusicalContext(context));
            }
            call!(self, endEditing());
            Ok(())
        })();
        if let Some(destroy) = self.interface.destroyDocumentController {
            // SAFETY: the plug-in's function for its own controller, called
            // once.
            unsafe { destroy(self.document) };
        }
        // SAFETY: balances the initialize in `create`; the factory is valid
        // while its module is loaded, which outlives the controller.
        unsafe {
            if let Some(uninitialize) = self.raw_factory.read_unaligned().uninitializeARA {
                uninitialize();
            }
        }
    }
}

/// The assert function address ARA is configured with: a slot that holds
/// nothing, so the plug-in's debug asserts stay quiet.
struct Assert(std::cell::UnsafeCell<sys::ARAAssertFunction>);

// SAFETY: the slot is never written by the host, and the plug-in only reads
// it, on the thread that initialized ARA.
unsafe impl Sync for Assert {}

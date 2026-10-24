//! ARA 2 for VST3 plug-ins (IMP-20; the ARA hosting spec R1, R2, R8, R9): a
//! plug-in whose component answers ARA's `IPlugInEntryPoint2` is bound to
//! its document controller before it is first activated, and the playback
//! regions of its track are given to its renderers while it is inactive.
//!
//! The main thread does all of it: [`Vst3Host::sync_ara`] runs after
//! [`Vst3Host::idle`] each poll, turning the set into edit cycles. A change
//! that adds or removes a region on a track whose plug-in is active asks its
//! node for the plug-in first; the node passes its input through until the
//! plug-in is back (R8). A move, trim or rename of a clip is an update the
//! plug-in takes while it plays.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{c_char, c_void};
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use bl_ara_sys as sys;
use bl_model::{
    AraArchive, AraSupport, ClipId, ContentHash, Device, DeviceId, DeviceKind, Document, TrackId,
    ara_device,
};
use vst3::ComPtr;
use vst3::Steinberg::{TUID, tresult};

use super::{Instance, Main, Vst3Host, lock};
use crate::ara::model::Model;
use crate::ara::{AraMedia, AraStatus, Controller, Extension, Factory, PlaybackRequest};

/// A class ID as the VST3 SDK's `INLINE_UID` writes it.
const fn tuid(l1: u32, l2: u32, l3: u32, l4: u32) -> TUID {
    let bytes = if cfg!(windows) {
        [
            l1 as u8,
            (l1 >> 8) as u8,
            (l1 >> 16) as u8,
            (l1 >> 24) as u8,
            (l2 >> 16) as u8,
            (l2 >> 24) as u8,
            l2 as u8,
            (l2 >> 8) as u8,
            (l3 >> 24) as u8,
            (l3 >> 16) as u8,
            (l3 >> 8) as u8,
            l3 as u8,
            (l4 >> 24) as u8,
            (l4 >> 16) as u8,
            (l4 >> 8) as u8,
            l4 as u8,
        ]
    } else {
        [
            (l1 >> 24) as u8,
            (l1 >> 16) as u8,
            (l1 >> 8) as u8,
            l1 as u8,
            (l2 >> 24) as u8,
            (l2 >> 16) as u8,
            (l2 >> 8) as u8,
            l2 as u8,
            (l3 >> 24) as u8,
            (l3 >> 16) as u8,
            (l3 >> 8) as u8,
            l3 as u8,
            (l4 >> 24) as u8,
            (l4 >> 16) as u8,
            (l4 >> 8) as u8,
            l4 as u8,
        ]
    };
    let mut id = [0 as c_char; 16];
    let mut at = 0;
    while at < 16 {
        id[at] = bytes[at] as c_char;
        at += 1;
    }
    id
}

/// `ARA::IPlugInEntryPoint`, from ARAVST3.h.
const ENTRY_POINT: TUID = tuid(0x1281_4E54, 0xA1CE_4076, 0x82B9_6813, 0x1695_0BD6);
/// `ARA::IPlugInEntryPoint2`, from ARAVST3.h.
const ENTRY_POINT_2: TUID = tuid(0xCD9A_5913, 0xC9EB_46D7, 0x96CA_53AD, 0xD1DB_89F5);

#[repr(C)]
struct UnknownVtbl {
    query: unsafe extern "system" fn(*mut c_void, *const TUID, *mut *mut c_void) -> tresult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[repr(C)]
struct EntryVtbl {
    base: UnknownVtbl,
    get_factory: unsafe extern "system" fn(*mut c_void) -> *const sys::ARAFactory,
    _bind: unsafe extern "system" fn(
        *mut c_void,
        sys::ARADocumentControllerRef,
    ) -> *const sys::ARAPlugInExtensionInstance,
}

#[repr(C)]
struct Entry2Vtbl {
    base: UnknownVtbl,
    bind: unsafe extern "system" fn(
        *mut c_void,
        sys::ARADocumentControllerRef,
        sys::ARAPlugInInstanceRoleFlags,
        sys::ARAPlugInInstanceRoleFlags,
    ) -> *const sys::ARAPlugInExtensionInstance,
}

/// A COM object one of the entry points was asked from, released on drop.
struct Entry(*mut c_void);

impl Entry {
    /// The component's `iid` interface, if it has it.
    fn of(component: &ComPtr<vst3::Steinberg::Vst::IComponent>, iid: &TUID) -> Option<Self> {
        let unknown = component.as_ptr().cast::<c_void>();
        let mut object: *mut c_void = std::ptr::null_mut();
        // SAFETY: every VST3 interface's table starts with `FUnknown`'s three
        // functions; the object is live and is asked for an interface by ID.
        let result = unsafe {
            let table = &**unknown.cast::<*const UnknownVtbl>();
            (table.query)(unknown, iid, &mut object)
        };
        (result == vst3::Steinberg::kResultOk && !object.is_null()).then_some(Self(object))
    }

    fn vtbl<T>(&self) -> &T {
        // SAFETY: a COM object's first word points to its table, which has
        // the layout `T` describes for the interface it was asked as.
        unsafe { &**self.0.cast::<*const T>() }
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        // SAFETY: balances the reference the query added.
        unsafe { (self.vtbl::<UnknownVtbl>().release)(self.0) };
    }
}

/// What a component says about ARA, if it implements ARA 2's entry points:
/// its factory (R1).
pub(super) fn factory_of(
    component: &ComPtr<vst3::Steinberg::Vst::IComponent>,
) -> Option<(*const sys::ARAFactory, Factory)> {
    let entry = Entry::of(component, &ENTRY_POINT)?;
    // The plug-in must bind with roles too: ARA 1 plug-ins aren't hosted.
    Entry::of(component, &ENTRY_POINT_2)?;
    // SAFETY: the entry point's own function.
    let raw = unsafe { (entry.vtbl::<EntryVtbl>().get_factory)(entry.0) };
    if raw.is_null() {
        return None;
    }
    // SAFETY: the factory is valid while the module is loaded.
    let described = unsafe { crate::ara::describe(raw) }.ok()?;
    Some((raw, described))
}

/// The roles every instance is bound for: it renders its track, and shows
/// and previews it, as ARA 1 hosts used every instance.
const ROLES: sys::ARAPlugInInstanceRoleFlags =
    sys::kARAPlaybackRendererRole | sys::kARAEditorRendererRole | sys::kARAEditorViewRole;

/// What a bound instance holds of ARA.
pub(super) struct Binding {
    plugin: String,
    /// Which set's controller it is bound to (see [`State::clear`]).
    generation: u64,
    extension: Extension,
    /// The track whose regions it renders, from the last sync.
    track: Option<TrackId>,
    /// The regions (by the plug-in's pointer) and the sequence its renderers
    /// hold.
    regions: BTreeSet<usize>,
    sequence: Option<usize>,
    /// The set is being edited: the instance stays inactive until it is done.
    pub(super) hold: bool,
}

/// What the host keeps of ARA across instances.
#[derive(Default)]
pub(super) struct State {
    controllers: BTreeMap<String, Controller>,
    media: Option<Arc<dyn AraMedia>>,
    /// The archives the set held for plug-ins that have no controller yet.
    archives: Vec<AraArchive>,
    /// How long each file plays, which the model needs.
    seconds: BTreeMap<ContentHash, f64>,
    synced: Option<u64>,
    /// Something changed since the last sync that the revision doesn't show
    /// (an instance bound).
    dirty: bool,
    /// Counts the sets the host has had, so an instance made for a new set
    /// never binds to the old set's controller.
    generation: u64,
    /// The controllers of sets that were let go of, by generation. Each stays
    /// until the last plug-in bound to it is gone, because ARA asks that the
    /// plug-in no longer refer to a region when it is destroyed.
    retired: Vec<(u64, BTreeMap<String, Controller>)>,
}

impl State {
    /// The controller for the plug-in `factory` describes, made if the set
    /// has none yet (R1), with the archive the set saved for it (R10).
    fn controller(
        &mut self,
        raw: *const sys::ARAFactory,
        factory: &Factory,
    ) -> Result<&mut Controller, String> {
        if !self.controllers.contains_key(&factory.id) {
            let media = self
                .media
                .clone()
                .ok_or_else(|| "the host has no media for ARA".to_owned())?;
            let restore = self
                .archives
                .iter()
                .find(|archive| {
                    archive.plugin == factory.id
                        && (archive.archive_id == factory.archive_id
                            || factory.compatible_archive_ids.contains(&archive.archive_id))
                })
                .map(|archive| (archive.data.clone(), archive.archive_id.clone()));
            // SAFETY: the factory came from a loaded module that outlives
            // the controller (`Main` drops controllers before modules), and
            // this is the main thread.
            let made = unsafe { Controller::create(raw, media, restore) }?;
            self.controllers.insert(factory.id.clone(), made);
        }
        self.controllers
            .get_mut(&factory.id)
            .ok_or_else(|| "no controller".to_owned())
    }

    /// Lets go of every controller, as a set closing does: they wait in
    /// [`State::retired`] for their plug-ins.
    pub(super) fn clear(&mut self) {
        if !self.controllers.is_empty() {
            self.retired
                .push((self.generation, std::mem::take(&mut self.controllers)));
        }
        self.generation += 1;
        self.seconds.clear();
        self.synced = None;
    }

    /// Destroys the retired controllers no plug-in is bound to any more.
    pub(super) fn reap(&mut self, bound: &BTreeSet<u64>) {
        self.retired.retain(|(generation, _)| bound.contains(generation));
    }
}

/// Binds a new instance to its plug-in's document controller (R2), before
/// it is activated or given its state. An instance of a plug-in that isn't
/// an ARA plug-in is left alone.
pub(super) fn bind(main: &mut Main, instance: &mut Instance) -> Result<(), String> {
    let Some((raw, factory)) = factory_of(&instance.component) else {
        return Ok(());
    };
    let Some(entry) = Entry::of(&instance.component, &ENTRY_POINT_2) else {
        return Ok(());
    };
    let controller = main.ara.controller(raw, &factory)?;
    // SAFETY: the entry point's own function, with the controller's
    // document, before the first activation as it asks.
    let raw_extension = unsafe {
        (entry.vtbl::<Entry2Vtbl>().bind)(entry.0, controller.document(), ROLES, ROLES)
    };
    // SAFETY: what the plug-in just returned, valid until the instance goes.
    let extension = unsafe { Extension::of(raw_extension) }
        .ok_or_else(|| "the plug-in didn't bind to the document".to_owned())?;
    instance.ara = Some(Binding {
        generation: main.ara.generation,
        plugin: factory.id,
        extension,
        track: None,
        regions: BTreeSet::new(),
        sequence: None,
        // The plug-in stays inactive until the first sync has given it its
        // regions, so a node never starts it without them.
        hold: true,
    });
    main.ara.dirty = true;
    Ok(())
}

/// Whether an instance is running (or lent to a render), so its regions can't
/// change.
fn busy(instance: &Instance) -> bool {
    instance.handed.is_some() || instance.lent.is_some()
}

/// Asks the instance's node for the plug-in, and keeps it inactive.
fn hold(instance: &mut Instance) {
    if let Some(binding) = &mut instance.ara {
        binding.hold = true;
    }
    if let Some(ends) = &instance.ends {
        ends.status.hand_back.store(true, Relaxed);
    }
}

impl Vst3Host {
    /// Gives ARA hosting the media the set's files are read from.
    pub fn set_ara_media(&self, media: Arc<dyn AraMedia>) {
        if let Ok(main) = lock(&self.inner.main).get() {
            main.ara.media = Some(media);
        }
    }

    /// Learns what the set's VST3 devices are, before a node asks for their
    /// plug-ins: a device whose plug-in the host hasn't loaded yet is loaded
    /// from its saved path, so the compiler knows whether it is an ARA
    /// plug-in (R4), and the archives the set holds are kept for plug-ins
    /// that get their controller next (R10).
    pub fn know(&self, document: &Document) {
        let unknown: Vec<std::path::PathBuf> = {
            let types = lock(&self.inner.types);
            let mut paths = Vec::new();
            for track in document.tracks() {
                for device in track.devices_deep() {
                    if let (DeviceKind::Plugin { format: bl_model::PluginFormat::Vst3, .. }, Some(plugin)) =
                        (&device.kind, &device.plugin)
                        && !plugin.path.is_empty()
                        && !types.iter().any(|known| known.device_type.kind == device.kind)
                    {
                        let path = std::path::PathBuf::from(&plugin.path);
                        if !paths.contains(&path) {
                            paths.push(path);
                        }
                    }
                }
            }
            paths
        };
        for path in unknown {
            // A plug-in that can't be loaded is reported when its device
            // asks for it.
            let _ = self.load(&path);
        }
        let mut guard = lock(&self.inner.main);
        if let Ok(main) = guard.get() {
            let held = &main.ara.controllers;
            main.ara.archives = document
                .settings()
                .ara_archives
                .iter()
                .filter(|archive| !held.contains_key(&archive.plugin))
                .cloned()
                .collect();
        }
    }

    /// Brings every ARA plug-in's document up to date with the set (R2):
    /// the tempo and signatures, a region sequence for each track a plug-in
    /// is on, a source for each file, a modification for each clip and a
    /// playback region for each piece of one, as edit cycles on the main
    /// thread, with undo and redo included since they change the set. Run it
    /// after [`Vst3Host::idle`] each poll; `revision` changes whenever the
    /// set does. Returns whether it edited a document.
    pub fn sync_ara(&self, document: &Document, revision: u64) -> bool {
        let ara_types: Vec<(DeviceKind, Factory)> = lock(&self.inner.types)
            .iter()
            .filter_map(|known| {
                known
                    .ara
                    .clone()
                    .map(|factory| (known.device_type.kind.clone(), factory))
            })
            .collect();
        let mut guard = lock(&self.inner.main);
        let Ok(main) = guard.get() else { return false };
        let waiting = main
            .instances
            .values()
            .any(|instance| instance.ara.as_ref().is_some_and(|binding| binding.hold));
        if main.ara.controllers.is_empty()
            || (main.ara.synced == Some(revision) && !main.ara.dirty && !waiting)
        {
            main.ara.synced = Some(revision);
            return false;
        }
        main.ara.synced = Some(revision);
        main.ara.dirty = false;
        let hosts = |device: &Device| {
            ara_types
                .iter()
                .find(|(kind, _)| *kind == device.kind)
                .map(|(_, factory)| (factory.id.clone(), factory.support))
        };
        let support = |device: &Device| hosts(device).map(|(_, support): (String, AraSupport)| support);

        // Which track each bound device renders.
        let mut renders: BTreeMap<DeviceId, TrackId> = BTreeMap::new();
        for track in document.tracks() {
            if let Some((device, _)) = ara_device(track, support) {
                renders.insert(device.id, track.id());
            }
        }
        for (&device, instance) in &mut main.instances {
            if let Some(binding) = &mut instance.ara {
                binding.track = renders.get(&device).copied();
            }
        }

        let mut edited = false;
        let ids: Vec<String> = main.ara.controllers.keys().cloned().collect();
        let Main { ara, instances, .. } = main;
        for plugin in ids {
            let seconds = &mut ara.seconds;
            let media = ara.media.clone();
            let mut length = |file: ContentHash| -> Option<f64> {
                if let Some(known) = seconds.get(&file) {
                    return Some(*known);
                }
                let found = media.as_ref()?.seconds(file)?;
                seconds.insert(file, found);
                Some(found)
            };
            let model = Model::of(document, &plugin, &hosts, &mut length);
            let Some(controller) = ara.controllers.get_mut(&plugin) else {
                continue;
            };
            // Instances whose track gains or loses a region while they run
            // are asked for first (R8).
            let (added, removed) = controller.membership(&model);
            let mut waiting = false;
            for instance in instances.values_mut() {
                let Some(binding) = &instance.ara else { continue };
                if binding.plugin != plugin {
                    continue;
                }
                let affected = binding
                    .track
                    .is_some_and(|track| added.contains(&track) || removed.contains(&track));
                if affected && busy(instance) {
                    waiting = true;
                    hold(instance);
                }
            }
            if waiting {
                continue;
            }
            if controller.differs(&model) {
                let mut before = |track: TrackId, region: sys::ARAPlaybackRegionRef| {
                    for instance in instances.values_mut() {
                        if let Some(binding) = &mut instance.ara
                            && binding.track == Some(track)
                            && binding.regions.remove(&(region as usize))
                        {
                            binding.extension.remove_region(region);
                        }
                    }
                };
                match controller.apply(model, &mut before) {
                    Ok(_) => edited = true,
                    Err(why) => {
                        for instance in instances.values_mut() {
                            if instance.ara.as_ref().is_some_and(|b| b.plugin == plugin) {
                                instance.failure = Some(format!("ARA: {why}"));
                            }
                        }
                    }
                }
            }
            // Each idle instance holds the regions of its track.
            for instance in instances.values_mut() {
                let idle = !busy(instance);
                let Some(binding) = &mut instance.ara else { continue };
                if binding.plugin != plugin {
                    continue;
                }
                let want: BTreeMap<usize, sys::ARAPlaybackRegionRef> = binding
                    .track
                    .map(|track| {
                        controller
                            .regions_of(track)
                            .map(|(_, region)| (region as usize, region))
                            .collect()
                    })
                    .unwrap_or_default();
                let sequence = binding
                    .track
                    .and_then(|track| controller.sequence_of(track));
                let sequence_key = sequence.map(|s| s as usize);
                let settled = binding.regions.iter().eq(want.keys()) && binding.sequence == sequence_key;
                if settled {
                    binding.hold = false;
                    continue;
                }
                if !idle {
                    // Something moved onto a running plug-in: have it back.
                    hold(instance);
                    continue;
                }
                for (key, region) in &want {
                    if binding.regions.insert(*key) {
                        binding.extension.add_region(*region);
                    }
                }
                let stale: Vec<usize> = binding
                    .regions
                    .iter()
                    .filter(|key| !want.contains_key(key))
                    .copied()
                    .collect();
                for key in stale {
                    binding.regions.remove(&key);
                }
                if binding.sequence != sequence_key {
                    if let Some(old) = binding.sequence {
                        binding.extension.remove_sequence(old as sys::ARARegionSequenceRef);
                    }
                    if let Some(new) = sequence {
                        binding.extension.add_sequence(new);
                    }
                    binding.sequence = sequence_key;
                }
                binding.hold = false;
            }
        }
        // A controller nothing in the set needs any more goes.
        let used: BTreeSet<&str> = main
            .instances
            .values()
            .filter_map(|instance| instance.ara.as_ref().map(|b| b.plugin.as_str()))
            .collect();
        let wanted: BTreeSet<String> = document
            .tracks()
            .flat_map(|track| track.devices())
            .filter_map(|device| hosts(device).map(|(id, _)| id))
            .collect();
        let stale: Vec<String> = main
            .ara
            .controllers
            .keys()
            .filter(|plugin| !used.contains(plugin.as_str()) && !wanted.contains(*plugin))
            .cloned()
            .collect();
        for plugin in stale {
            main.ara.controllers.remove(&plugin);
        }
        edited
    }

    /// The archives of the ARA plug-ins' documents, to keep in the set (R10):
    /// what each running controller stores, and the ones `kept` holds for
    /// plug-ins that aren't running, which stay as they were.
    #[must_use]
    pub fn ara_archives(&self, kept: &[AraArchive]) -> Vec<AraArchive> {
        let mut guard = lock(&self.inner.main);
        let Ok(main) = guard.get() else {
            return kept.to_vec();
        };
        let mut archives: Vec<AraArchive> = kept
            .iter()
            .filter(|archive| !main.ara.controllers.contains_key(&archive.plugin))
            .cloned()
            .collect();
        for (plugin, controller) in &mut main.ara.controllers {
            if let Some(data) = controller.archive() {
                archives.push(AraArchive {
                    plugin: plugin.clone(),
                    archive_id: controller.factory.archive_id.clone(),
                    data,
                });
            }
        }
        archives
    }

    /// Tells the editor views what the Arrangement has selected (R12): the
    /// clips and tracks, to the plug-ins whose tracks they are on.
    pub fn ara_select(&self, clips: &[ClipId], tracks: &[TrackId]) {
        let mut guard = lock(&self.inner.main);
        let Ok(main) = guard.get() else { return };
        for instance in main.instances.values() {
            let Some(binding) = &instance.ara else { continue };
            let (Some(track), Some(view), Some(controller)) = (
                binding.track,
                binding.extension.view.as_ref(),
                main.ara.controllers.get(&binding.plugin),
            ) else {
                continue;
            };
            let on_track = |clip: &ClipId| controller.regions_of(track).any(|((c, _), _)| c == *clip);
            let clips: Vec<ClipId> = clips.iter().copied().filter(on_track).collect();
            let tracks: Vec<TrackId> = tracks.iter().copied().filter(|t| *t == track).collect();
            // SAFETY: the view of the instance bound to this controller.
            unsafe { controller.select(view, &clips, &tracks) };
        }
    }

    /// What the ARA plug-ins have asked of the transport since the last
    /// call.
    #[must_use]
    pub fn take_ara_requests(&self) -> Vec<PlaybackRequest> {
        let mut guard = lock(&self.inner.main);
        let Ok(main) = guard.get() else {
            return Vec::new();
        };
        main.ara
            .controllers
            .values()
            .flat_map(Controller::take_requests)
            .collect()
    }

    /// Each ARA plug-in's document: what it holds and has done.
    #[must_use]
    pub fn ara_status(&self) -> Vec<AraStatus> {
        let mut guard = lock(&self.inner.main);
        let Ok(main) = guard.get() else {
            return Vec::new();
        };
        main.ara
            .controllers
            .iter()
            .map(|(plugin, controller)| {
                let (sequences, sources, modifications) = controller.counts();
                let stats = &controller.host.stats;
                AraStatus {
                    plugin: plugin.clone(),
                    name: controller.factory.name.clone(),
                    timestretch: controller.factory.support.timestretch,
                    devices: main
                        .instances
                        .iter()
                        .filter_map(|(device, instance)| {
                            instance
                                .ara
                                .as_ref()
                                .filter(|binding| binding.plugin == *plugin)
                                .map(|binding| (*device, binding.track))
                        })
                        .collect(),
                    sequences,
                    sources,
                    modifications,
                    regions: controller.placed().cloned().collect(),
                    edit_cycles: controller.edit_cycles,
                    readers_opened: stats.readers_opened.load(Relaxed),
                    reader_calls: stats.reader_calls.load(Relaxed),
                    frames_read: stats.frames_read.load(Relaxed),
                    audio_thread_reads: stats.audio_thread_reads.load(Relaxed),
                    missing: controller.missing.clone(),
                    restored: controller.restored.clone(),
                }
            })
            .collect()
    }
}

/// The sets whose controllers some plug-in, live or closing, is bound to.
pub(super) fn bound_generations(main: &Main) -> BTreeSet<u64> {
    main.instances
        .values()
        .chain(main.closing.iter().map(|(_, instance)| instance))
        .filter_map(|instance| instance.ara.as_ref().map(|binding| binding.generation))
        .collect()
}

//! The compiler: turns a document into an engine program, and decides which
//! edits need a new program at all.

use std::collections::{BTreeMap, BTreeSet};

use bl_core::time::{TempoMap, TimeSignatureMap};
use bl_model::{
    Chain, ChainId, Change, ChangeSet, ContentHash, Device, DeviceId, DeviceKind, DeviceOutput,
    DeviceRole, Document, LaunchMode, Mixer, OrdMap, Output, ParamId, RackKind, SceneId, Tap,
    Track, TrackId, TrackKind,
};

use crate::EngineConfig;
use crate::automation;
use crate::clips::{ClipList, ClipPlayer, SlotSample};
use crate::disjoint::Shared;
use crate::display::Displays;
use crate::drum::{self, ChainHits, DrumNotes};
use crate::gc::{GcHandle, Program};
use crate::layers::{DeviceLayers, LayerAudio, LayerSample};
use crate::message::{LoopRegion, ParamEvent, Punch, TransportCommand};
use crate::midi_clips::{MidiClipList, MidiClipPlayer};
use crate::mixer::{self, Route, Solo, TrackMixer, route};
use crate::node::{NodeData, NodeKey, NoteBuffer, ParamValue};
use crate::points::MeterPoint;
use crate::program::{
    ChainSendRef, DeviceInput, DeviceMidiSidechain, DeviceSidechain, EngineProgram, ExtOut, Input,
    Instance, MergeCursor, Migration, ModKind, NO_CHAIN, NO_DELAY, NO_OUTPUT, NO_OUTPUT_BUFFER,
    NO_SLOT, NO_TASK, NodeSlot, NoteDelay, Rack, RackChain, RackMode, Span, Step, Task,
    delay_storage,
};
use crate::registry::Registry;
use crate::routing::{Edge, Graph, Via};
use crate::sched::Sched;
use crate::session::FollowTable;
use crate::session::{SceneMaps, SceneOverride};
use crate::sidechain::{self, SidechainKey};
use crate::sources::{ClipSources, Loaded, Loader};
use crate::state::{MeterOwner, MeterSite};
use crate::warped::PoolSpec;
use crate::zones::{self, ChainSelector, ZoneFilter};

/// What the audio thread needs after an edit: a new program if the edit
/// changed the song's structure, and parameter changes for nodes that keep
/// running.
///
/// Send it whole and in order ([`Sender::send_update`](crate::Sender::send_update)):
/// each program takes over nodes from the one compiled before it.
#[derive(Debug, Default)]
pub struct Update {
    /// The new program, if the edit needs one.
    pub program: Option<Program>,
    /// Transport commands the document asks for: the Arrangement loop, sent
    /// with every program and whenever the loop changes. They take effect
    /// after the program, so the loop's frames follow its tempo map.
    pub transport: Vec<TransportCommand>,
    /// Parameter changes, which take effect at the start of the next block
    /// after the program, if there is one.
    pub params: Vec<ParamEvent>,
    /// Data for nodes, which reaches them after the program: the clips of
    /// each audio track whose clips changed, for its clip player
    /// ([`NodeKey::Player`]), placed under the program's tempo map.
    pub data: Vec<(NodeKey, NodeData)>,
    /// Devices in the new program that no factory makes a node for, so their
    /// tracks' audio passes through where they'd be.
    pub unhosted: Vec<DeviceId>,
    /// Media that clips in the tracks sent here name and the registry's
    /// media library can't supply: missing, changed, or not audio. Those
    /// clips play silence (the audio clips spec, R14). With a loader thread
    /// (an engine on a device), media is found missing when the loader
    /// tries it, so it's reported by a later update.
    pub missing: Vec<ContentHash>,
}

impl Update {
    /// Whether the update holds nothing to send.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.program.is_none()
            && self.transport.is_empty()
            && self.params.is_empty()
            && self.data.is_empty()
    }
}

/// A layer longer than this many seconds is read in the background with its
/// start held, unless RAM Mode is on (the multisampler spec, R23).
const LONG_LAYER_SECONDS: u64 = 10;

/// The tuning `track` plays in `document`: the set's, unless the track
/// bypasses it (the tunings spec, R14).
fn tuning_of(document: &Document, track: TrackId) -> Option<&bl_model::Tuning> {
    document
        .track(track)
        .filter(|track| track.plays_tuning())
        .and_then(|_| document.settings().tuning.as_ref())
}

/// What a node is, beyond its key: a node migrates only if this matches.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Identity {
    /// A device. Its media is part of what it is (WP-2.16): a device whose
    /// sample changed is a new node, and one the set keeps gets its audio
    /// sent again. An external device's latency is part of it too (WP-5.35):
    /// its node holds a delay line of that many frames, so a new latency
    /// makes a new node.
    Device(DeviceKind, Vec<Option<ContentHash>>, u32),
    Player,
    MidiPlayer,
    Mixer,
    Route,
    Sidechain,
    /// A rack's chain selector (WP-5.3).
    Selector,
    /// The zone filter of a rack's chain (WP-5.3).
    Zones,
    Drum,
}

/// Which clip players a compile sends their clips to, besides the new ones
/// (which get theirs whenever they have clips).
#[derive(Debug, Default)]
struct Resend {
    /// Every player: the tempo map changed, or the program isn't built on
    /// the last one synced.
    all: bool,
    /// The players of these tracks, whose clips changed.
    tracks: BTreeSet<TrackId>,
}

/// Where a node sits in the last program compiled.
#[derive(Clone, Debug)]
struct Placed {
    slot: u32,
    identity: Identity,
    /// The frames the node's own processing adds, as it reported when it was
    /// made, so a migrated node keeps its latency (WP-2.11).
    latency: u32,
    /// The frames a parameter change aimed at the node is held back, so
    /// automation acts on the audio it is meant for (WP-2.11): its slot's
    /// delay, worked out when the program is compiled.
    delay: u32,
}

/// Turns documents into engine programs, off the audio thread.
///
/// It remembers the last document it saw and the layout of the last program
/// it compiled. [`Compiler::sync`] compares a new document with the last
/// one, and compiles only if the song's structure changed: tracks added,
/// removed, or moved; devices added, removed, moved, or switched on or off;
/// a return's Pre/Post switch; the tempo or time-signature map. Mixer
/// settings (volume, pan, the activator, solo, Solo in Place, send levels)
/// and device parameters become [`ParamEvent`]s for the nodes that already
/// run them, the Arrangement loop becomes a [`TransportCommand::SetLoop`],
/// and clip changes travel as node data: a track whose clips changed sends
/// its clip player the whole list again, placed and with its audio from the
/// registry's media library (the audio clips spec). None of them
/// recompiles.
///
/// Each program lists the nodes it takes over from the program compiled
/// before it, so programs must reach the audio thread in the order they
/// were compiled, none skipped.
///
/// Each clip's source comes from the registry's media library, which may
/// read and decode files. A compiler opens sources itself, which suits an
/// offline render on its own thread, until it's given a loader thread
/// ([`Compiler::load_off_thread`]), as an engine on a device does: then a
/// clip joins its track's list when its source is open, and the update the
/// loader's answer brings ([`Compiler::loaded`]) sends it.
pub struct Compiler {
    config: EngineConfig,
    registry: Registry,
    gc: GcHandle,
    generation: u64,
    /// The last document compiled, as the program plays it: with the tempo
    /// and signature of a launched scene in place of the set's, while one
    /// holds.
    synced: Option<Document>,
    /// The last document as the person's set holds it, which a change of
    /// scene override compiles again (WP-3.5).
    original: Option<Document>,
    /// What the audio thread says a launched scene has put in place. It
    /// holds even when the scene's own fields change or the scene goes.
    scene_override: SceneOverride,
    layout: BTreeMap<NodeKey, Placed>,
    /// What each delay line of the last program holds back: its place in
    /// the program and its length in frames.
    delay_layout: BTreeMap<DelayKey, (u32, u32)>,
    /// The place of each MIDI track's note delay in the last program.
    note_layout: BTreeMap<TrackId, u32>,
    sources: ClipSources,
    /// The stretcher pool each track's player holds, by track (WP-4.6, R5,
    /// R21): what the last list it was sent left it with, so a list that
    /// still fits carries none and the player keeps it.
    pool_specs: BTreeMap<TrackId, PoolSpec>,
    /// The long layers being read in the background (WP-5.13, R23), by
    /// media, so an edit that sends the layers again shares them rather
    /// than reading each file again. A sample goes when no node holds it.
    progressive: BTreeMap<ContentHash, std::sync::Weak<bl_media::ProgressiveSample>>,
    /// Where each modulator's rows sit on the last program's bus
    /// (WP-5.10).
    modulator_rows: crate::modbus::ModRows,
    /// The readers of the displays of the devices' nodes.
    displays: Displays,
    /// The counters of the notes that reach drum chains.
    hits: ChainHits,
}

/// The set's Global Amount of groove as a transport command carries it, in
/// tenths of a percent.
fn groove_amount(document: &Document) -> u16 {
    (document.settings().grooves.global_amount * 10.0).round() as u16
}

/// Whether `track` sits inside a frozen group, whose render replaces it
/// (WP-4.14, R14).
fn inside_frozen_group(document: &bl_model::Document, track: TrackId) -> bool {
    let mut parent = document.parent(track);
    while let Some(id) = parent {
        if document.track(id).is_some_and(bl_model::Track::is_frozen) {
            return true;
        }
        parent = document.parent(id);
    }
    false
}

/// Every audio and MIDI slot that holds a clip, as `(scene, track, launch
/// mode)` triples sorted by scene and then track: what a scene launch
/// queues (WP-3.2) and where a press reads its clip's mode (WP-3.3).
fn scene_slots(document: &Document) -> Vec<(SceneId, TrackId, LaunchMode)> {
    let mut slots = Vec::new();
    for track in document.track_order() {
        let Some(track) = document.track(track) else {
            continue;
        };
        for (scene, clip) in track.slots() {
            slots.push((scene, track.id(), clip.launch.mode));
        }
    }
    slots.sort_unstable_by_key(|&(scene, track, _)| (scene, track));
    slots
}

impl Compiler {
    /// A compiler for an engine with `config`, making device nodes with
    /// `registry` and wrapping programs for `gc`'s collector.
    #[must_use]
    pub fn new(config: EngineConfig, registry: Registry, gc: GcHandle) -> Self {
        Self {
            config,
            registry,
            gc,
            generation: 0,
            synced: None,
            original: None,
            scene_override: SceneOverride::default(),
            layout: BTreeMap::new(),
            delay_layout: BTreeMap::new(),
            note_layout: BTreeMap::new(),
            sources: ClipSources::default(),
            pool_specs: BTreeMap::new(),
            progressive: BTreeMap::new(),
            modulator_rows: crate::modbus::ModRows::default(),
            displays: Displays::default(),
            hits: ChainHits::default(),
        }
    }

    /// The counters of the notes that reach the drum chains this compiler's
    /// programs have, which the shell reads (the drum racks spec, R20).
    #[must_use]
    pub fn chain_hits(&self) -> ChainHits {
        self.hits.clone()
    }

    /// The readers of the displays of the nodes this compiler makes, which
    /// the shell reads the newest values through.
    #[must_use]
    pub fn displays(&self) -> Displays {
        self.displays.clone()
    }

    /// Opens clips' sources on a thread of its own from now on, rather than
    /// on the compiler's: each answer goes to `deliver`, to come back
    /// through [`Compiler::loaded`] on the compiler's thread. Without a
    /// media library there's nothing to open, and no thread starts.
    ///
    /// # Errors
    ///
    /// If the thread can't start.
    pub(crate) fn load_off_thread(
        &mut self,
        deliver: impl Fn(Loaded) + Send + 'static,
    ) -> std::io::Result<()> {
        if let Some(media) = self.registry.shared_media() {
            self.sources.set_loader(Loader::start(media, deliver)?);
        }
        Ok(())
    }

    /// What the audio thread needs now that the loader has answered: the
    /// clips of the track whose clip's source is open, or the media it
    /// couldn't open. Nothing if the answer is stale, or no document has
    /// been synced.
    pub(crate) fn loaded(&mut self, loaded: Loaded) -> Update {
        let mut update = Update::default();
        match self.sources.loaded(loaded) {
            Some(Ok(clip)) => {
                let document = self.synced.take();
                match document.as_ref().and_then(|d| d.clip_track(clip)) {
                    Some(track) => {
                        if let Some(document) = &document {
                            self.send_clips(document, Some(document), track, &mut update);
                        }
                    }
                    // The clip went while its source opened.
                    None => self.sources.retain(|_| true, |c| c != clip),
                }
                self.synced = document;
            }
            Some(Err(media)) => update.missing.push(media),
            None => {}
        }
        update
    }

    /// What the audio thread needs now that `media`'s analysis is ready:
    /// the clips of every track that plays the file, sent again, so a Beats
    /// clip's plan cuts at the transients the library now gives (the warp
    /// modes spec, R14). Nothing before a document has been synced.
    pub(crate) fn media_analysed(&mut self, media: ContentHash) -> Update {
        let mut update = Update::default();
        let Some(document) = self.synced.take() else {
            return update;
        };
        let plays = |clip: &bl_model::Clip| matches!(&clip.data, bl_model::ClipData::Audio(audio) if audio.media == media);
        let tracks: Vec<TrackId> = document
            .tracks()
            .filter(|track| {
                track.clips().any(plays)
                    || track.played_clips().any(plays)
                    || track.slots().any(|(_, clip)| plays(clip))
            })
            .map(bl_model::Track::id)
            .collect();
        for track in tracks {
            self.send_clips(&document, Some(&document), track, &mut update);
        }
        self.synced = Some(document);
        update
    }

    /// The last program's generation, or 0 before the first.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// What the audio thread needs to play `document`, given the documents
    /// before it.
    pub fn sync(&mut self, document: &Document) -> Update {
        let effective = self.effective(document);
        self.sync_effective(&effective)
    }

    /// The document as a program plays it: `document` with the tempo and
    /// signature a launched scene put in place (WP-3.5), and, with no scene
    /// override, its leading clips' tempo laid over their spans (the warping
    /// spec, R44). Remembers `document` for [`Compiler::set_override`].
    fn effective(&mut self, document: &Document) -> Document {
        self.original = Some(document.clone());
        let tempo = match self.scene_override.tempo {
            Some((_, bpm)) => TempoMap::constant(bpm),
            None => document.tempo_map(),
        };
        let signature = self
            .scene_override
            .signature
            .map(|(_, signature)| TimeSignatureMap::constant(signature));
        document.with_maps(Some(tempo), signature)
    }

    /// What the audio thread has put in place from a launched scene,
    /// compiled again over the last document: the arrangement's clips and
    /// envelopes land under the tempo it plays at (WP-3.5). Empty before a
    /// document was synced, or when nothing changed.
    pub fn set_override(&mut self, scene_override: SceneOverride) -> Update {
        if self.scene_override == scene_override {
            return Update::default();
        }
        self.scene_override = scene_override;
        match self.original.clone() {
            Some(document) => self.sync(&document),
            None => Update::default(),
        }
    }

    fn sync_effective(&mut self, document: &Document) -> Update {
        let Some(old) = self.synced.take() else {
            return self.compile_effective(document);
        };
        let changes = ChangeSet::between(&old, document);
        let mut params = Vec::new();
        // Spec: modulation R28. A mapping of a modulator driven by audio,
        // notes or MIDI changes which track waits for which, and which rows
        // its target reads.
        let recompile = needs_program(&changes, &old, document, &mut params, &|kind, param| {
            if matches!(kind, bl_model::DeviceKind::Rack { .. }) {
                bl_model::rack_description().param(param).cloned()
            } else {
                self.registry
                    .device_type(kind)
                    .and_then(|device_type| device_type.description.param(param).cloned())
            }
        }) || signal_mappings(&old) != signal_mappings(document);
        route_events(&changes, &old, document, &mut params);
        // Tracks that held or hold a clip that changed: their players get
        // their clips again, and their nodes their envelopes again, since a
        // clip's envelopes modulate those and where the clip sits decides
        // when it plays (WP-3.15).
        let mut tracks: BTreeSet<TrackId> = changes
            .clips
            .keys()
            .flat_map(|&clip| [old.clip_track(clip), document.clip_track(clip)])
            .flatten()
            .collect();
        // Spec: ara-hosting R4. A device that makes a track's clips an ARA
        // plug-in's, or stops doing so, moves them between the plug-in and
        // the player.
        tracks.extend(document.tracks().map(Track::id).filter(|&id| {
            let hosts = |doc: &Document| {
                doc.track(id).and_then(|track| {
                    bl_model::ara_device(track, |device| self.registry.ara_support(device))
                        .map(|(device, support)| (device.id, support))
                })
            };
            hosts(&old) != hosts(document)
        }));
        // Auditioning a take lane swaps the clips the track plays (the
        // comping spec, R14): its player gets them again.
        tracks.extend(changes.tracks.keys().copied().filter(|&id| {
            matches!((old.track(id), document.track(id)),
                (Some(was), Some(now)) if was.audition() != now.audition())
        }));
        // Spec: grooves R15. A groove's parameters, the Global Amount and the
        // pool reach the clips that use them as node data: their tracks'
        // lists are sent again, and no program is made.
        if old.settings().grooves != document.settings().grooves {
            tracks.extend(
                document
                    .tracks()
                    .filter(|track| {
                        track
                            .played_clips()
                            .chain(track.slots().map(|(_, clip)| clip))
                            .any(|clip| clip.groove.is_some())
                    })
                    .map(bl_model::Track::id),
            );
        }
        let mut update = if recompile {
            let resend = Resend {
                all: old.settings().tempo != document.settings().tempo,
                tracks: tracks.clone(),
            };
            self.compile_with(document, &resend)
        } else {
            self.synced = Some(document.clone());
            let mut update = Update::default();
            if old.settings().arrangement_loop != document.settings().arrangement_loop {
                update
                    .transport
                    .push(TransportCommand::SetLoop(loop_region(document)));
            }
            // Spec: transport R21. The song's start time follows its
            // locator or the loop brace as they move.
            if old.song_start_ticks() != document.song_start_ticks() {
                update
                    .transport
                    .push(TransportCommand::SetSongStart(document.song_start_ticks()));
            }
            if punch(&old) != punch(document) {
                update
                    .transport
                    .push(TransportCommand::SetPunch(punch(document)));
            }
            // Spec: grooves R21
            if groove_amount(&old) != groove_amount(document) {
                update
                    .transport
                    .push(TransportCommand::SetGrooveAmount(groove_amount(document)));
            }
            // Spec: scales R15
            if old.settings().scale != document.settings().scale {
                update
                    .transport
                    .push(TransportCommand::SetScale(document.settings().scale));
            }
            if old.settings().metronome != document.settings().metronome {
                update.transport.push(TransportCommand::SetMetronome(
                    document.settings().metronome,
                ));
            }
            for &track in &tracks {
                self.send_clips(document, Some(&old), track, &mut update);
            }
            update
        };
        // The players' sources preload the loop's new start, whether or not
        // a new program was made.
        if old.settings().arrangement_loop != document.settings().arrangement_loop {
            self.sources.cue_loop(self.loop_start_frame(document));
        }
        // A multisampler whose layers changed gets them again as node data:
        // a zone or layer edit never recompiles (WP-5.13, R31).
        for (&id, &change) in &changes.devices {
            if let (Change::Modified, Some(before), Some(after)) =
                (change, old.device(id), document.device(id))
                && before.layers != after.layers
            {
                self.send_layers(document, id, &mut update);
            }
            // A sampler whose sample's edits, warp or slices changed gets the
            // edited sample again as node data, with no new program (R57).
            if let (Change::Modified, Some(before), Some(after)) =
                (change, old.device(id), document.device(id))
                && (before.sample_edits != after.sample_edits
                    || before.warp != after.warp
                    || before.slices != after.slices)
                && before.media == after.media
                && let Some(media) = after.media_at(0)
            {
                self.send_slot_sample(document, id, 0, media, &after.sample_edits, &mut update);
            }
        }
        // A new program's restatement of migrated nodes goes first.
        update.params.extend(params);
        // A track whose envelopes changed sends its nodes their lists again,
        // empty ones included, which clear the envelopes a node had; so does
        // a track whose clips changed, whose clip envelopes (WP-3.15) may
        // have gone the same way. A new tempo map moved every envelope, so
        // every track's goes again.
        let all = old.settings().tempo != document.settings().tempo;
        // Spec: modulation R15, R19. A mapping made, removed or taken away
        // with its target, or a modulator that came or went, changes the
        // lists of the nodes it reaches, and the rows they name.
        let mapped = modulator_mappings(&old) != modulator_mappings(document);
        for (&id, &change) in &changes.devices {
            if let (Change::Modified, Some(before), Some(after)) =
                (change, old.device(id), document.device(id))
                && before.shape != after.shape
            {
                let shape = after
                    .shape
                    .clone()
                    .unwrap_or_else(|| bl_model::modulator::default_shape_of(&after.kind));
                update.data.push((
                    NodeKey::Device(id),
                    self.gc.data(crate::modbus::Shape(shape)),
                ));
            }
            // Spec: mpe R14, R18. A plug-in's MPE Mode and settings reach
            // its node as node data, with no new program.
            if let (Change::Modified, Some(before), Some(after)) =
                (change, old.device(id), document.device(id))
                && before.mpe != after.mpe
            {
                update.data.push((
                    NodeKey::Device(id),
                    self.gc.data(crate::node::PluginMpeData(after.mpe)),
                ));
            }
            // Spec: articulation-map R14. An Articulation Map's list reaches
            // its node as node data, so a change to it reaches the playing
            // device without a new program.
            if let (Change::Modified, Some(before), Some(after)) =
                (change, old.device(id), document.device(id))
                && before.articulations != after.articulations
            {
                let list = bl_model::ArticulationList::of(&after.articulations);
                update.data.push((
                    NodeKey::Device(id),
                    self.gc.data(crate::node::Articulations(list)),
                ));
            }
        }
        // Spec: tunings R18. A tuning loaded, changed or removed, a new
        // reference pitch or range, or a track's Bypass Tuning changed
        // reaches the track's instruments as node data, with no new
        // program. A new program already sent the tuned pitches; it only
        // needs to send twelve equal steps to a track that stopped
        // playing the tuning.
        for track in document.tracks() {
            let id = track.id();
            let (before, after) = (tuning_of(&old, id), tuning_of(document, id));
            if before != after && (!recompile || after.is_none()) {
                self.send_key_pitches(document, id, after.is_some(), &mut update);
            }
        }
        for track in document.tracks() {
            let id = track.id();
            let differs = match (old.track(id), document.track(id)) {
                (Some(before), Some(after)) => before.automation_map() != after.automation_map(),
                // A new track's nodes are new, with no envelopes to clear,
                // and a deleted track's nodes are gone with the track.
                _ => false,
            };
            let clips_changed = tracks.contains(&id);
            if all || differs || clips_changed || mapped {
                // The track whose envelopes or clips changed gets every
                // list, so an empty one clears what a node had; a tempo map
                // that moved them only sends the ones that drive something.
                self.send_automation(
                    document,
                    id,
                    !differs && !clips_changed && !mapped,
                    &mut update,
                );
            }
        }
        update
    }

    /// A whole new program for `document`. Nodes that the last program ran
    /// migrate into it, and every audio track's clip player is sent its
    /// clips.
    pub fn compile(&mut self, document: &Document) -> Update {
        let effective = self.effective(document);
        self.compile_effective(&effective)
    }

    fn compile_effective(&mut self, document: &Document) -> Update {
        let resend = Resend {
            all: true,
            tracks: BTreeSet::new(),
        };
        self.compile_with(document, &resend)
    }

    /// Reports `device`'s node's latency now, as a plug-in that restarted
    /// for a new one does (WP-2.7): the last document is compiled again,
    /// with the device's node kept and the new latency in its slot, so
    /// delay compensation follows the change (delay-compensation R6).
    /// Nothing if the device's node isn't in the last program, or its
    /// latency is unchanged.
    pub fn latency(&mut self, device: DeviceId, frames: u32) -> Update {
        match self.layout.get_mut(&NodeKey::Device(device)) {
            Some(placed) if placed.latency != frames => placed.latency = frames,
            _ => return Update::default(),
        }
        let Some(document) = self.synced.clone() else {
            return Update::default();
        };
        // The clips stay with their players: nothing about them changed.
        self.compile_with(&document, &Resend::default())
    }

    /// Reports the audio interface's round trip now (the device's reported
    /// input and output latency, and Driver Error Compensation), as
    /// the device reports it after a measurement or the compensation
    /// changes: external devices report it to delay compensation (the
    /// external devices spec, R13), so the last document is compiled again
    /// with the new figure. Nothing if it is unchanged, or the set holds no
    /// external device.
    pub fn round_trip(&mut self, frames: u32) -> Update {
        if self.config.round_trip_latency == frames {
            return Update::default();
        }
        self.config.round_trip_latency = frames;
        let Some(document) = self.synced.clone() else {
            return Update::default();
        };
        if !document
            .tracks()
            .any(|track| track.devices_deep().any(|d| d.external.is_some()))
        {
            return Update::default();
        }
        self.compile_with(&document, &Resend::default())
    }

    /// Sends a sampler's node the audio of one of its sample slots, with
    /// `edits` applied (WP-2.16, R57): the media library decodes and renders
    /// here, on the compiler's thread, and shares the result with every
    /// other reader of the same source and list; media it can't supply is
    /// reported missing and plays silence.
    fn send_slot_sample(
        &mut self,
        document: &Document,
        device: DeviceId,
        slot: usize,
        media: ContentHash,
        edits: &[bl_model::SampleEdit],
        update: &mut Update,
    ) {
        let rate = self.config.sample_rate;
        let (warp, slices) = match document.device(device) {
            Some(d) => (d.warp.clone(), std::sync::Arc::from(d.slices.as_slice())),
            None => (bl_model::DeviceWarp::default(), std::sync::Arc::from([])),
        };
        match self
            .registry
            .media()
            .and_then(|library| library.edited_sample(media, rate, edits))
        {
            Some(sample) => update.data.push((
                NodeKey::Device(device),
                self.gc.data(SlotSample {
                    slot,
                    sample,
                    warp,
                    slices,
                }),
            )),
            None => update.missing.push(media),
        }
    }

    /// Sends a multisampler's node its layers, each with its decoded audio
    /// (WP-5.13, R31): the media library decodes here, on the compiler's
    /// thread, and shares each file with every other reader; a layer whose
    /// media it can't supply plays silence and is reported missing. The data
    /// reaches the node after the program, never as a new program.
    fn send_layers(&mut self, document: &Document, device: DeviceId, update: &mut Update) {
        let Some(device) = document.device(device) else {
            return;
        };
        let rate = self.config.sample_rate;
        let library = self.registry.media();
        let mut fresh = Vec::new();
        let layers = device
            .layers
            .list
            .iter()
            .map(|layer| {
                // R23: with RAM Mode off, a layer over ten seconds is read
                // in the background behind its held start; the rest decode
                // whole.
                // The edits are applied to the whole decoded sample (R57), so
                // a layer that has some is read whole.
                let long = !device.layers.ram_mode
                    && layer.edits.is_empty()
                    && layer.length > u64::from(layer.sample_rate) * LONG_LAYER_SECONDS;
                let held = self
                    .progressive
                    .get(&layer.media)
                    .and_then(std::sync::Weak::upgrade)
                    .filter(|_| long);
                let progressive = held.or_else(|| {
                    let head = (LONG_LAYER_SECONDS * u64::from(rate.hz())) as usize;
                    let made = library
                        .filter(|_| long)
                        .and_then(|library| library.progressive(layer.media, rate, head))?;
                    fresh.push((layer.media, std::sync::Arc::downgrade(&made)));
                    Some(made)
                });
                let sample = match progressive {
                    Some(sample) => Some(LayerSample::Progressive(sample)),
                    None => library
                        .and_then(|library| library.edited_sample(layer.media, rate, &layer.edits))
                        .map(LayerSample::Whole),
                };
                if sample.is_none() {
                    update.missing.push(layer.media);
                }
                LayerAudio {
                    layer: layer.clone(),
                    sample,
                }
            })
            .collect::<Vec<_>>();
        self.progressive.retain(|_, held| held.strong_count() > 0);
        self.progressive.extend(fresh);
        update.missing.sort_unstable();
        update.missing.dedup();
        let data = DeviceLayers::new(
            layers,
            device.layers.ram_mode,
            device.layers.linear_loop_fades,
        );
        update
            .data
            .push((NodeKey::Device(device.id), self.gc.data(data)));
    }

    /// Sends an audio track's clip player its clips, from the media library
    /// (WP-1.4). A frozen track's list comes from its freeze files (R6).
    fn send_audio_clips(
        &mut self,
        document: &Document,
        track: &Track,
        previous: Option<&Document>,
        update: &mut Update,
    ) {
        let loop_start = self.loop_start_frame(document);
        let previous = previous.and_then(|last| last.track(track.id()));
        let pool = self.pool_specs.get(&track.id()).copied();
        // Spec: ara-hosting R4. The clips an ARA plug-in on the track
        // renders aren't the player's.
        let ara = self.ara_support(track);
        let built = ClipList::build(
            track,
            previous,
            ara,
            &document.settings().tempo,
            self.config.sample_rate,
            self.config.channels,
            loop_start,
            pool,
            &mut self.sources,
            self.registry.media(),
            &document.settings().grooves,
        );
        self.pool_specs.insert(track.id(), built.pool);
        update.missing.extend(built.missing);
        update.missing.sort_unstable();
        update.missing.dedup();
        update
            .data
            .push((NodeKey::Player(track.id()), self.gc.data(built.list)));
    }

    /// What the ARA plug-in on `track` supports, if it has one (IMP-20).
    fn ara_support(&self, track: &Track) -> Option<bl_model::AraSupport> {
        bl_model::ara_device(track, |device| self.registry.ara_support(device))
            .map(|(_, support)| support)
    }

    /// Sends `track`'s clip player its clips: a list placed under `document`'s
    /// tempo map. An empty list clears the player's.
    fn send_clips(
        &mut self,
        document: &Document,
        previous: Option<&Document>,
        track: TrackId,
        update: &mut Update,
    ) {
        let Some(track) = document.track(track) else {
            return;
        };
        match track.kind() {
            // A frozen MIDI track plays freeze files through the audio clip
            // list; a frozen group or return plays its whole render (R6,
            // R14, R15).
            TrackKind::Midi | TrackKind::Group | TrackKind::Return if track.is_frozen() => {
                self.send_audio_clips(document, track, previous, update);
            }
            TrackKind::Audio => self.send_audio_clips(document, track, previous, update),
            TrackKind::Midi => {
                let list = MidiClipList::build(
                    track,
                    &document.settings().tempo,
                    self.config.sample_rate,
                    &document.settings().grooves,
                );
                update
                    .data
                    .push((NodeKey::Player(track.id()), self.gc.data(list)));
            }
            _ => {}
        }
    }

    /// The song frame where `document`'s Arrangement loop starts, if it's
    /// on: where a wrap lands, which clips' sources preload.
    fn loop_start_frame(&self, document: &Document) -> Option<i64> {
        loop_region(document).map(|region| {
            document
                .settings()
                .tempo
                .ticks_to_frames(region.start, self.config.sample_rate)
                .get()
        })
    }

    fn compile_with(&mut self, document: &Document, resend: &Resend) -> Update {
        // The document the program was playing before this one: a clip whose
        // warp mode changed crossfades from what it was playing (the warp
        // modes spec, R8).
        let previous = self.synced.clone();
        let graph = Graph::of(document);
        let solo = Solo::with(document, &graph);
        // A frozen group plays its own render: the tracks inside it run no
        // devices and reach no output (WP-4.14, R14).
        let hidden: BTreeSet<TrackId> = document
            .track_order()
            .into_iter()
            .filter(|id| inside_frozen_group(document, *id))
            .collect();
        let order: Vec<TrackId> = document
            .track_order()
            .into_iter()
            .filter(|id| !hidden.contains(id))
            .collect();
        let task_of: BTreeMap<TrackId, u32> = order
            .iter()
            .enumerate()
            .map(|(i, &id)| (id, i as u32))
            .collect();
        let build_midi_sources = graph
            .midi_links
            .iter()
            .filter_map(|link| Some((link.device, *task_of.get(&link.from)?)))
            .collect();
        let mut build = Build {
            midi_sources: build_midi_sources,
            ..Build::default()
        };

        // Each track's inputs, in the order the graph lists them.
        let mut inputs: BTreeMap<TrackId, Vec<Input>> = BTreeMap::new();
        let mut sidechain_inputs: BTreeMap<DeviceId, Vec<Input>> = BTreeMap::new();
        let mut taps: BTreeSet<(TrackId, Tap)> = BTreeSet::new();
        // The inputs that tap a rack chain: the track they feed, their
        // place among its inputs, and the chain, which is found once the
        // tasks have added their racks.
        let mut chain_taps: Vec<(TrackId, usize, ChainId)> = Vec::new();
        // The inputs that take a device's extra output (WP-5.45): the track
        // they feed, their place among its inputs, and the output, whose
        // device is found once the tasks have added their chains.
        let mut output_taps: Vec<(TrackId, usize, DeviceOutput)> = Vec::new();
        let mut midi_waits: Vec<(u32, u32)> = Vec::new();
        for edge in &graph.edges {
            let (Some(&from), Some(to)) = (task_of.get(&edge.from), document.track(edge.to)) else {
                continue;
            };
            // Spec: routing R21. A MIDI track's MIDI From another track
            // carries no audio, though the source still runs first.
            if edge.via == Via::Input && to.kind() == TrackKind::Midi {
                midi_waits.extend(task_of.get(&edge.to).map(|&to| (from, to)));
                continue;
            }
            let route = self.route(&mut build, document, &solo, edge, to);
            // A route that takes a device's extra output carries it whatever
            // tap it names, so its tap needs no copy of the track's own
            // signal (the routing spec, R12).
            if edge.tap != Tap::PostMixer && edge.chain.is_none() && edge.output.is_none() {
                taps.insert((edge.from, edge.tap));
            }
            let input = Input {
                route,
                from,
                tap: edge.tap,
                chain: NO_CHAIN,
                device: NO_TASK,
                output: NO_OUTPUT,
                before: 0,
                delay: NO_DELAY,
            };
            match edge.via.sidechain() {
                Some(device) => sidechain_inputs.entry(device).or_default().push(input),
                None => {
                    let to_inputs = inputs.entry(edge.to).or_default();
                    if let Some(chain) = edge.chain {
                        chain_taps.push((edge.to, to_inputs.len(), chain));
                    }
                    if let Some(output) = edge.output {
                        output_taps.push((edge.to, to_inputs.len(), output));
                    }
                    to_inputs.push(input);
                }
            }
        }

        // Cue mode (WP-3.18): every track's signal after its devices and
        // before its mixer reaches the Cue Out's task through its Cue
        // switch.
        let mut cue_inputs = Vec::new();
        if document.cue_active() {
            for track in order.iter().filter_map(|&id| document.track(id)) {
                if track.kind() == TrackKind::Main {
                    continue;
                }
                let Some(&from) = task_of.get(&track.id()) else {
                    continue;
                };
                cue_inputs.push(Input {
                    route: self.cue_route(&mut build, track),
                    from,
                    tap: Tap::PostFx,
                    chain: NO_CHAIN,
                    device: NO_TASK,
                    output: NO_OUTPUT,
                    before: 0,
                    delay: NO_DELAY,
                });
                taps.insert((track.id(), Tap::PostFx));
            }
        }

        // A task per track, in display order.
        for &id in &order {
            let track = document
                .track(id)
                .expect("a document's tree names only its own tracks");
            self.add_task(
                &mut build,
                track,
                document.settings().crossfader,
                inputs.remove(&id).unwrap_or_default(),
                &mut sidechain_inputs,
                clips_audible(track),
            );
        }
        // The Cue Out's task comes last, and plays on the device channels
        // Cue Out names, as an Ext. Out track does.
        let cue_out = document.cue_active().then(|| {
            self.add_cue_task(&mut build, document, cue_inputs);
            ExtOut {
                task: build.tasks.len() as u32 - 1,
                channel: document.main().routing().cue_out,
            }
        });

        // Tasks that are tapped before their devices or their mixer keep a
        // copy there.
        let mut kept = 0;
        for (id, tap) in taps {
            let task = &mut build.tasks[task_of[&id] as usize];
            let slot = match tap {
                Tap::PreFx => &mut task.pre_fx,
                Tap::PostFx => &mut task.post_fx,
                Tap::PostMixer => continue,
            };
            if slot.is_none() {
                *slot = Some(kept);
                kept += 1;
            }
        }

        // Tasks whose notes a MIDI sidechain hears keep a copy of them.
        let mut midi_kept = 0;
        for at in 0..build.midi_sidechains.len() {
            let from = build.midi_sidechains[at].from;
            if from == NO_TASK {
                continue;
            }
            let keep = &mut build.tasks[from as usize].midi_keep;
            let index = *keep.get_or_insert_with(|| {
                midi_kept += 1;
                midi_kept - 1
            });
            build.midi_sidechains[at].kept = index;
        }

        // The chains that routes tap keep a copy at each tap too.
        for (to, at, chain) in chain_taps {
            let task = build.tasks[task_of[&to] as usize];
            let Some(&index) = build.chain_index.get(&chain) else {
                continue;
            };
            let input = &mut build.inputs[task.inputs.start as usize + at];
            input.chain = index;
            let slot = &mut build.rack_chains[index as usize].taps[RackChain::tap_index(input.tap)];
            if slot.is_none() {
                *slot = Some(kept);
                kept += 1;
            }
        }

        // A device whose extra output a route takes renders every output it
        // declares into a buffer of its own, so another track can read one
        // (WP-5.45). The buffers sit one after another, the device's output
        // `n` at its base plus `n`.
        let mut output_bases: BTreeMap<DeviceId, (u32, u32)> = BTreeMap::new();
        for (_, _, output) in &output_taps {
            if output_bases.contains_key(&output.device) {
                continue;
            }
            let Some(&slot) = build.device_slots.get(&output.device) else {
                continue;
            };
            let Some(kind) = document
                .device(output.device)
                .map(|device| device.kind.clone())
            else {
                continue;
            };
            let ports = self.registry.device_outputs(&kind).len() as u32;
            if ports == 0 {
                continue;
            }
            build.slots[slot as usize].outputs = Span {
                start: kept,
                end: kept + ports,
            };
            output_bases.insert(output.device, (kept, ports));
            kept += ports;
        }
        // Spec: external-devices R18. An external effect sends its signal to
        // the interface's outputs, in a buffer of its own that the engine
        // mixes into them.
        for (slot, channel) in &build.external_effects {
            build.slots[*slot as usize].external_out = Some(kept);
            build.slots[*slot as usize].external_to = Some(*channel);
            kept += 1;
        }
        for (to, at, output) in output_taps {
            let task = build.tasks[task_of[&to] as usize];
            let slot = build.device_slots.get(&output.device).copied();
            let buffers = output_bases.get(&output.device).copied();
            let input = &mut build.inputs[task.inputs.start as usize + at];
            // The output's buffer, where the compiler has one for it. A
            // device it has no node for, a type that declares no extra
            // outputs, and an index past those it declares all carry
            // nothing ([`NO_OUTPUT_BUFFER`]) — never another buffer.
            match (slot, buffers) {
                (Some(slot), Some((base, ports))) if output.output < ports => {
                    input.device = slot;
                    input.output = base + output.output;
                }
                _ => input.output = NO_OUTPUT_BUFFER,
            }
        }

        let generation = self.generation + 1;
        let base = (self.generation > 0).then_some(self.generation);
        let tasks = build.tasks.len();
        let mut midi = crate::midi::MidiRoutes::compile(document, &build.tasks);
        let size = self.config.channels * self.config.max_frames;
        let ext_outs = document
            .track_order()
            .into_iter()
            .filter_map(|id| match document.track(id)?.routing().output {
                // A track that hears Main is silent (R14), Ext. Out too.
                Output::External { channel } if !graph.hears_main.contains(&id) => Some(ExtOut {
                    task: task_of[&id],
                    channel,
                }),
                _ => None,
            })
            .chain(cue_out)
            .collect();
        // Spec: modulation R28. A track holding a parameter a modulator
        // driven by audio, notes or MIDI maps waits for the modulator's.
        let waits = graph.modulator_edges(document);
        let wait_tasks: Vec<(u32, u32)> = waits
            .iter()
            .filter_map(|(from, to)| Some((*task_of.get(from)?, *task_of.get(to)?)))
            .chain(midi_waits)
            .collect();
        for &(_, to) in &wait_tasks {
            build.tasks[to as usize].dependencies += 1;
        }
        let (order, dependents) = run_order(&build, &wait_tasks);
        compensate(&mut build, &order, document, self.config.sample_rate.hz());
        // Spec: external-devices R4. A track's MIDI to a port waits as long
        // as its audio would be held back on the way into Main.
        for track in document.tracks() {
            if track.kind() == TrackKind::Midi {
                midi.set_compensation(
                    track.id(),
                    main_compensation(&build, document, &task_of, track.id()),
                );
            }
        }
        // Automation is placed with each node's delay in it (WP-2.11), so
        // remember what every node's turned out to be.
        for slot in &build.slots {
            if let Some(placed) = build.layout.get_mut(&slot.key) {
                placed.delay = slot.delay;
            }
        }
        let (delay_lines, delay_pool) = delay_storage(&build.delays, self.config.channels);
        // A delay line that holds the same signal back by as many frames as
        // in the last program takes the audio it holds over, so a recompile
        // that didn't change it doesn't empty it.
        let delay_migrations: Vec<Migration> = build
            .delay_keys
            .iter()
            .zip(&build.delays)
            .enumerate()
            .filter_map(|(to, (key, &len))| {
                let &(from, was) = self.delay_layout.get(key)?;
                (was == len).then_some(Migration {
                    from,
                    to: to as u32,
                })
            })
            .collect();
        let note_migrations: Vec<Migration> = build
            .note_delays
            .iter()
            .enumerate()
            .filter_map(|(to, (track, _))| {
                Some(Migration {
                    from: *self.note_layout.get(track)?,
                    to: to as u32,
                })
            })
            .collect();
        let note_delays = Shared::new(
            build
                .note_delays
                .iter()
                .map(|&(_, len)| NoteDelay::new(len))
                .collect(),
        );
        // Spec: session-view R23. The set's own maps, the scenes' and the
        // launched override's (made here so the audio thread allocates
        // nothing to switch to them), and the ones the program plays under
        // now.
        let own = self.original.as_ref().unwrap_or(document);
        // The set's own tempo map is the leading clips' tempo laid over the
        // Song Tempo envelope (the warping spec, R44, R48), which is what the
        // program plays under when no scene override is in place.
        let base_tempo = own.tempo_map();
        let base_signatures = own.settings().time_signatures.clone();
        let scene_maps: Vec<SceneMaps> = document
            .scenes()
            .filter(|scene| scene.tempo.is_some() || scene.signature.is_some())
            .map(|scene| SceneMaps {
                scene: scene.id,
                tempo: scene.tempo.map(|bpm| (bpm, TempoMap::constant(bpm))),
                signature: scene
                    .signature
                    .map(|signature| (signature, TimeSignatureMap::constant(signature))),
            })
            .collect();
        let override_tempo = self
            .scene_override
            .tempo
            .map(|(_, bpm)| (bpm, TempoMap::constant(bpm)));
        let override_signatures = self
            .scene_override
            .signature
            .map(|(_, signature)| (signature, TimeSignatureMap::constant(signature)));
        let tempo = override_tempo
            .as_ref()
            .map_or_else(|| document.settings().tempo.clone(), |(_, map)| map.clone());
        let time_signatures = override_signatures.as_ref().map_or_else(
            || document.settings().time_signatures.clone(),
            |(_, map)| map.clone(),
        );
        let modulators = build.modulators.len();
        let mod_rows = crate::modbus::ModRows {
            rows: build
                .modulators
                .iter()
                .enumerate()
                .map(|(m, &(_, device))| {
                    let late = build
                        .signals
                        .iter()
                        .position(|&s| s as usize == m)
                        .map(|k| ((modulators + k) * crate::modbus::ROWS) as u32);
                    let row = crate::modbus::ModRow {
                        direct: crate::modbus::row(m as u32, 0),
                        late,
                    };
                    (device, row)
                })
                .collect(),
            waits: waits.iter().copied().collect(),
        };
        let program = EngineProgram {
            generation,
            base,
            index: build.layout.iter().map(|(&key, p)| (key, p.slot)).collect(),
            slots: Shared::new(build.slots),
            migrations: build.migrations,
            order,
            task_dependents: dependents.iter().flatten().copied().collect(),
            sched: Sched::new(tasks, build.racks.len(), build.rack_chains.len()),
            tasks: with_dependents(build.tasks, &dependents),
            task_inputs: build.inputs,
            steps: build.steps,
            meter_points: build.meter_points,
            merge: Shared::new(vec![MergeCursor::default(); build.rack_chains.len()]),
            selects: Shared::new(vec![0.0; build.racks.len() * self.config.max_frames]),
            racks: build.racks,
            rack_chains: build.rack_chains,
            chain_sends: build.chain_sends,
            sidechain_pool: Shared::new(vec![0.0; build.sidechains.len() * size]),
            sidechains: build.sidechains,
            sidechain_inputs: build.sidechain_inputs,
            midi_sidechains: build.midi_sidechains,
            midi_kept: Shared::new(
                (0..midi_kept)
                    .map(|_| NoteBuffer::with_capacity(1024))
                    .collect(),
            ),
            pool: Shared::new(vec![0.0; (tasks + build.chain_buffers as usize) * size]),
            tap_pool: Shared::new(vec![0.0; kept as usize * size]),
            delay_lines,
            delay_pool,
            note_delays,
            delay_migrations,
            note_migrations,
            device_input: vec![0.0; self.config.input_channels * self.config.max_frames],
            device_input_channels: self.config.input_channels,
            channels: self.config.channels,
            max_frames: self.config.max_frames,
            output: task_of[&document.main_id()],
            cue: cue_out.map(|out| out.task),
            ext_outs,
            external_effects: build.external_effects,
            tempo,
            time_signatures,
            base_tempo,
            base_signatures,
            scene_maps,
            override_tempo,
            override_signatures,
            sample_rate: self.config.sample_rate,
            scene_slots: scene_slots(document),
            follow: FollowTable::of(document),
            session_tracks: document
                .track_order()
                .into_iter()
                .filter(|&id| document.track(id).is_some_and(|t| t.kind().holds_clips()))
                .collect(),
            note_buffers: Shared::new(
                (0..2 * (tasks as u32 + build.chain_buffers))
                    .map(|_| NoteBuffer::with_capacity(1024))
                    .collect(),
            ),
            releases: crate::program::Releases::new(),
            midi,
            modulators: build.modulators.iter().map(|&(slot, _)| slot).collect(),
            lagged: mod_rows.any_late(document),
            task_times: (0..tasks)
                .map(|_| core::sync::atomic::AtomicU64::new(0))
                .collect(),
            bus: Shared::new(vec![
                0.0;
                (build.modulators.len() + build.signals.len())
                    * crate::modbus::ROWS
                    * self.config.max_frames
            ]),
            signals: build.signals,
        };
        self.modulator_rows = mod_rows;
        self.generation = generation;
        self.layout = build.layout;
        self.delay_layout = build
            .delay_keys
            .into_iter()
            .zip(build.delays)
            .enumerate()
            .map(|(at, (key, len))| (key, (at as u32, len)))
            .collect();
        self.note_layout = build
            .note_delays
            .iter()
            .enumerate()
            .map(|(at, &(track, _))| (track, at as u32))
            .collect();
        self.displays
            .retain(|device| self.layout.contains_key(&NodeKey::Device(device)));
        self.hits
            .retain(|chain| self.layout.contains_key(&NodeKey::ChainNotes(chain)));
        self.synced = Some(document.clone());
        let mut update = Update {
            program: Some(self.gc.program(program)),
            transport: vec![
                TransportCommand::SetLoop(loop_region(document)),
                TransportCommand::SetSongStart(document.song_start_ticks()),
                TransportCommand::SetPunch(punch(document)),
                TransportCommand::SetMetronome(document.settings().metronome),
                TransportCommand::SetGrooveAmount(groove_amount(document)),
                TransportCommand::SetScale(document.settings().scale),
            ],
            params: build.restate,
            unhosted: build.unhosted,
            ..Update::default()
        };
        // Devices new in this program that play media get their decoded
        // audio: the sampler's sample, decoded by the media library here,
        // on the compiler's thread, and shared with the set's other readers
        // (WP-2.16). The update reaches the node after the program that
        // made it; a device whose media the library doesn't hold plays
        // silence and is reported missing.
        for (device, slot, media, edits) in &build.media {
            self.send_slot_sample(document, *device, *slot, *media, edits, &mut update);
        }
        for &device in &build.layered {
            self.send_layers(document, device, &mut update);
        }
        // Players that went hold no sources now, nor do new ones yet; open
        // sources of clips that went are closed.
        self.sources.retain(
            |track| document.track(track).is_some() && !build.new_players.contains(&track),
            |clip| document.clip_track(clip).is_some(),
        );
        // A track that went, or whose player is new and so holds nothing,
        // keeps no pool spec: the next list it is sent builds its pool.
        self.pool_specs.retain(|track, _| {
            document.track(*track).is_some() && !build.new_players.contains(track)
        });
        // A new player gets its track's clips if it has any, in the
        // Arrangement or in a Session slot; a player that migrated keeps its
        // own unless they changed, or the tempo map moved them.
        for track in document.tracks() {
            let id = track.id();
            let send = if build.new_players.contains(&id) {
                track.played_clips().next().is_some() || track.slots().next().is_some()
            } else {
                resend.all || resend.tracks.contains(&id)
            };
            if send {
                self.send_clips(document, previous.as_ref(), id, &mut update);
            }
            // Every node is new in a new program, so each is sent the
            // automation that drives its parameters: a node with none is
            // new too, and has none anyway.
            self.send_automation(document, id, true, &mut update);
            // Spec: tunings R18. Every node is new too, so each instrument
            // on a track that plays the tuning is sent its pitches.
            self.send_key_pitches(document, id, true, &mut update);
            // Spec: mpe R14. A plug-in's node starts with MPE Mode off, so
            // each one whose mode or settings differ is sent them.
            for device in document
                .track(id)
                .into_iter()
                .flat_map(bl_model::Track::devices_deep)
                .filter(|device| !device.mpe.is_default())
            {
                update.data.push((
                    NodeKey::Device(device.id),
                    self.gc.data(crate::node::PluginMpeData(device.mpe)),
                ));
            }
        }
        update
    }

    /// Sends each native device on `track` the pitch of every key (the
    /// tunings spec, R12 and R18): the set's tuning's with `tuned`, twelve
    /// equal steps' without. A device that plays no pitch of its own lets
    /// go of it.
    fn send_key_pitches(
        &self,
        document: &Document,
        track: TrackId,
        tuned: bool,
        update: &mut Update,
    ) {
        let Some(track) = document.track(track) else {
            return;
        };
        let tuning = document.settings().tuning.as_ref().filter(|_| tuned);
        if tuned && !(tuning.is_some() && track.plays_tuning()) {
            return;
        }
        let pitches = crate::KeyPitches::of(tuning);
        for device in track
            .devices_deep()
            .filter(|device| matches!(device.kind, bl_model::DeviceKind::Native { .. }))
        {
            update
                .data
                .push((NodeKey::Device(device.id), self.gc.data(pitches)));
        }
    }

    /// Sends each of `track`'s nodes the automation driving its parameters
    /// (WP-2.12): the track's mixer, each of its sends, and each device in
    /// its chain, each held back by the node's delay. An empty list clears
    /// the envelopes a node had, so a node whose envelopes all went follows
    /// its controls again; `driving_only` leaves those out, for a new
    /// program, where a node without envelopes is new and has none.
    fn send_automation(
        &self,
        document: &Document,
        track: TrackId,
        driving_only: bool,
        update: &mut Update,
    ) {
        let Some(track) = document.track(track) else {
            return;
        };
        let rate = self.config.sample_rate;
        for (key, list) in automation::lists(
            document,
            track.id(),
            rate,
            |key| i64::from(self.layout.get(&key).map_or(0, |placed| placed.delay)),
            &self.modulator_rows,
            |kind, param| {
                if matches!(kind, bl_model::DeviceKind::Rack { .. }) {
                    bl_model::rack_description().param(param).cloned()
                } else {
                    self.registry
                        .device_type(kind)
                        .and_then(|device_type| device_type.description.param(param).cloned())
                }
            },
        ) {
            if driving_only && list.is_empty() {
                continue;
            }
            update.data.push((key, self.gc.data(list)));
        }
    }

    /// Keeps `document` as the latest without compiling it, for an engine
    /// with no device running it: [`reconfigure`](Self::reconfigure)
    /// compiles it when one runs again. Only for that case: the next
    /// [`sync`](Self::sync) compares with it as if its program had been
    /// sent. Returns the [`TransportCommand::SetLoop`] and
    /// [`TransportCommand::SetPunch`] a sync would have sent if `document`
    /// changed the Arrangement loop or the punch points: the program
    /// compiled at the restart sets them too, but the processor that takes
    /// over carries the ones it played, so an edit made meanwhile must reach
    /// it after that.
    #[must_use]
    pub fn remember(&mut self, document: &Document) -> Vec<TransportCommand> {
        self.original = Some(document.clone());
        let old = self.synced.replace(document.clone());
        let mut commands = Vec::new();
        if old.as_ref().is_none_or(|old| {
            old.settings().arrangement_loop != document.settings().arrangement_loop
        }) {
            commands.push(TransportCommand::SetLoop(loop_region(document)));
        }
        if old
            .as_ref()
            .is_none_or(|old| old.song_start_ticks() != document.song_start_ticks())
        {
            commands.push(TransportCommand::SetSongStart(document.song_start_ticks()));
        }
        if old.as_ref().is_none_or(|old| punch(old) != punch(document)) {
            commands.push(TransportCommand::SetPunch(punch(document)));
        }
        if old
            .as_ref()
            .is_none_or(|old| groove_amount(old) != groove_amount(document))
        {
            commands.push(TransportCommand::SetGrooveAmount(groove_amount(document)));
        }
        if old
            .as_ref()
            .is_none_or(|old| old.settings().scale != document.settings().scale)
        {
            commands.push(TransportCommand::SetScale(document.settings().scale));
        }
        commands
    }

    /// Switches to `config` (a device's new sample rate or block size) and
    /// compiles the latest document anew for it, with every node made
    /// afresh at the new settings and none taken over: the program for a
    /// processor starting without one (WP-1.13). Empty if no document has
    /// been synced yet.
    pub fn reconfigure(&mut self, config: EngineConfig) -> Update {
        let document = self.original.take();
        self.config = config;
        self.reset();
        // The processor that starts has no launched scene's tempo in place.
        self.scene_override = SceneOverride::default();
        match document {
            Some(document) => self.compile(&document),
            None => Update::default(),
        }
    }

    /// The settings programs are compiled for.
    #[must_use]
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Forgets the last program, so the next compile makes every node anew
    /// and takes none over: for an audio thread that starts without one.
    pub fn reset(&mut self) {
        self.generation = 0;
        self.synced = None;
        self.original = None;
        self.layout.clear();
        self.delay_layout.clear();
        self.note_layout.clear();
        self.sources.reset();
        self.pool_specs.clear();
    }

    /// The slot of the route that carries `edge` into `to`: made with the
    /// route's state, or migrated from the last program and told its state
    /// now, which a routing change may have moved.
    fn route(
        &self,
        build: &mut Build,
        document: &Document,
        solo: &Solo,
        edge: &Edge,
        to: &Track,
    ) -> u32 {
        let config = self.config.node_config();
        let key = route_key(edge);
        let level = match edge.via {
            Via::Send => document.track(edge.from).map_or(0.0, |t| t.send(edge.to)),
            _ => 1.0,
        };
        let muted = solo.edge_muted(edge);
        let closed = edge.monitored && !to.routing().monitoring(to.mixer().arm);
        // Spec: routing R11. Under Monitor Auto the route also closes while
        // the track's clips play, which the program sets each block.
        let auto = edge.monitored && to.routing().monitor == bl_model::Monitor::Auto;
        if self.previous(key, &Identity::Route).is_some() {
            for (param, value) in [
                (route::LEVEL, level),
                (route::SOLO_MUTE, switch(muted)),
                (route::CLOSED, switch(closed)),
                (route::AUTO, switch(auto)),
            ] {
                build.restate.push(ParamEvent::now(key, param, value));
            }
        }
        self.place(build, key, Identity::Route, 0, || {
            Instance::Route(Box::new(
                Route::new(level, muted, &config).closed(closed).auto(auto),
            ))
        })
    }

    /// The slot of `track`'s cue switch: a route that is closed while the
    /// track's Solo switch, which Cue mode makes the Cue switch, is off.
    fn cue_route(&self, build: &mut Build, track: &Track) -> u32 {
        let key = NodeKey::Cue(track.id());
        let muted = !track.mixer().solo;
        if self.previous(key, &Identity::Route).is_some() {
            build
                .restate
                .push(ParamEvent::now(key, route::SOLO_MUTE, switch(muted)));
        }
        let config = self.config.node_config();
        self.place(build, key, Identity::Route, 0, || {
            Instance::Route(Box::new(Route::new(1.0, muted, &config)))
        })
    }

    /// Adds the Cue Out's task: it sums `inputs`, the cue-switched tracks,
    /// and its mixer sets the Cue Volume. It has no devices, and stands in
    /// Main's place in the track list, which it doesn't add to.
    fn add_cue_task(&self, build: &mut Build, document: &Document, inputs: Vec<Input>) {
        let dependencies = inputs.len() as u32;
        let start = build.inputs.len() as u32;
        build.inputs.extend(inputs);
        let inputs = Span {
            start,
            end: build.inputs.len() as u32,
        };
        let config = self.config.node_config();
        let volume = document.settings().cue_volume;
        if self.previous(NodeKey::CueMixer, &Identity::Mixer).is_some() {
            build.restate.push(ParamEvent::now(
                NodeKey::CueMixer,
                mixer::track::VOLUME,
                volume,
            ));
        }
        let mixer = self.place(build, NodeKey::CueMixer, Identity::Mixer, 0, || {
            let mixer = Mixer {
                volume,
                ..Mixer::default()
            };
            Instance::Mixer(Box::new(TrackMixer::new(&mixer, 0.5, &config)))
        });
        build.tasks.push(Task {
            track: document.main_id(),
            inputs,
            device_input: None,
            player: None,
            chain: Span::default(),
            meters: Span::default(),
            sidechains: Span::default(),
            midi_sidechains: Span::default(),
            mixer,
            clip_delay: NO_DELAY,
            note_delay: None,
            pre_fx: None,
            post_fx: None,
            midi_keep: None,
            dependents: Span::default(),
            dependencies,
        });
    }

    /// Adds the task for one track, fed by `inputs` and, into its devices'
    /// sidechains, by what `sidechain_inputs` holds for them; its clips
    /// heard or not.
    fn add_task(
        &self,
        build: &mut Build,
        track: &Track,
        crossfader: f64,
        inputs: Vec<Input>,
        sidechain_inputs: &mut BTreeMap<DeviceId, Vec<Input>>,
        audible: bool,
    ) {
        let mut dependencies = inputs.len() as u32;
        let input_start = build.inputs.len() as u32;
        build.inputs.extend(inputs);
        let inputs = Span {
            start: input_start,
            end: build.inputs.len() as u32,
        };
        let config = self.config.node_config();
        // The track's own input from the audio interface, Ext. In
        // (WP-2.13): its own route, whose Monitor gate opens and closes it
        // like any other input.
        let device_input = match track.routing().input {
            bl_model::Input::External { channel } if track.kind() == TrackKind::Audio => {
                let key = NodeKey::DeviceInput(track.id());
                let closed = !track.routing().monitoring(track.mixer().arm);
                let auto = track.routing().monitor == bl_model::Monitor::Auto;
                if self.previous(key, &Identity::Route).is_some() {
                    for (param, value) in [
                        (route::LEVEL, 1.0),
                        (route::SOLO_MUTE, switch(false)),
                        (route::CLOSED, switch(closed)),
                        (route::AUTO, switch(auto)),
                    ] {
                        build.restate.push(ParamEvent::now(key, param, value));
                    }
                }
                let route = self.place(build, key, Identity::Route, 0, || {
                    Instance::Route(Box::new(
                        Route::new(1.0, false, &config).closed(closed).auto(auto),
                    ))
                });
                Some(DeviceInput { route, channel })
            }
            _ => None,
        };
        // An audio or MIDI track's clip player runs first.
        let player = match track.kind() {
            // A frozen MIDI track plays freeze files through an audio clip
            // player (WP-4.14, R6).
            TrackKind::Audio | TrackKind::Midi | TrackKind::Group | TrackKind::Return
                if track.is_frozen() =>
            {
                Some({
                    let key = NodeKey::Player(track.id());
                    if self.previous(key, &Identity::Player).is_none() {
                        build.new_players.insert(track.id());
                    } else {
                        build.restate.push(ParamEvent::now(
                            key,
                            crate::clips::player::AUDIBLE,
                            switch(audible),
                        ));
                    }
                    self.place(build, key, Identity::Player, 0, || {
                        Instance::Device(Box::new(ClipPlayer::new(&config).audible(audible)))
                    })
                })
            }
            TrackKind::Audio => Some({
                let key = NodeKey::Player(track.id());
                if self.previous(key, &Identity::Player).is_none() {
                    build.new_players.insert(track.id());
                } else {
                    build.restate.push(ParamEvent::now(
                        key,
                        crate::clips::player::AUDIBLE,
                        switch(audible),
                    ));
                }
                self.place(build, key, Identity::Player, 0, || {
                    Instance::Device(Box::new(ClipPlayer::new(&config).audible(audible)))
                })
            }),
            TrackKind::Midi => Some({
                let key = NodeKey::Player(track.id());
                if self.previous(key, &Identity::MidiPlayer).is_none() {
                    build.new_players.insert(track.id());
                } else {
                    build.restate.push(ParamEvent::now(
                        key,
                        crate::clips::player::AUDIBLE,
                        switch(audible),
                    ));
                }
                self.place(build, key, Identity::MidiPlayer, 0, || {
                    Instance::Device(Box::new(MidiClipPlayer::new(&config).audible(audible)))
                })
            }),
            _ => None,
        };
        let sidechain_start = build.sidechains.len() as u32;
        let midi_start = build.midi_sidechains.len() as u32;
        // A frozen track runs none of its devices: its player hands on the
        // freeze files (WP-4.14, R6).
        let (chain, meters) = self.add_flow(
            build,
            track.devices().filter(|_| !track.is_frozen()),
            sidechain_inputs,
            &mut dependencies,
            (
                MeterOwner::Track(track.id()),
                // A frozen track plays its freeze file: audio from the start.
                track.kind() == TrackKind::Midi && !track.is_frozen(),
            ),
        );
        let sidechains = Span {
            start: sidechain_start,
            end: build.sidechains.len() as u32,
        };
        let midi_sidechains = Span {
            start: midi_start,
            end: build.midi_sidechains.len() as u32,
        };
        let mixer = self.place(
            build,
            NodeKey::Mixer(track.id()),
            Identity::Mixer,
            0,
            || {
                Instance::Mixer(Box::new(TrackMixer::new(
                    track.mixer(),
                    crossfader,
                    &config,
                )))
            },
        );
        // A MIDI track's player writes notes, which a note delay holds back
        // to line up with the track's inputs (delay-compensation R3). It is
        // there even with nothing to hold, so the notes it is holding when a
        // recompile replaces it carry over.
        let note_delay = (track.kind() == TrackKind::Midi && !track.is_frozen()).then(|| {
            build.note_delays.push((track.id(), 0));
            build.note_delays.len() as u32 - 1
        });
        build.tasks.push(Task {
            track: track.id(),
            inputs,
            device_input,
            player,
            chain,
            meters,
            sidechains,
            midi_sidechains,
            midi_keep: None,
            mixer,
            clip_delay: NO_DELAY,
            note_delay,
            pre_fx: None,
            post_fx: None,
            dependents: Span::default(),
            dependencies,
        });
    }

    /// Adds the nodes of a chain of devices, racks included, and returns its
    /// steps in `build.steps`. A device with a sidechain gets it, fed by
    /// what `sidechain_inputs` holds for it; `dependencies` counts the
    /// tasks the chain's sidechains wait for.
    fn add_flow<'a>(
        &self,
        build: &mut Build,
        devices: impl Iterator<Item = &'a Device>,
        sidechain_inputs: &mut BTreeMap<DeviceId, Vec<Input>>,
        dependencies: &mut u32,
        meters: (MeterOwner, bool),
    ) -> (Span, Span) {
        let config = self.config.node_config();
        let own = build.tasks.len() as u32;
        let mut steps = Vec::new();
        // Spec: midi-meters R2. A meter in front of each step and after the
        // last; the signal is MIDI until a device that isn't a MIDI effect.
        let (owner, mut midi) = meters;
        let mut points = Vec::new();
        for device in devices {
            if let Some(kind) = device.rack_kind() {
                if let Some(rack) = self.add_rack(
                    build,
                    device,
                    kind,
                    sidechain_inputs,
                    dependencies,
                    midi && device.role != DeviceRole::AudioEffect,
                ) {
                    steps.push(Step::Rack(rack));
                    points.push(MeterPoint::new(owner, MeterSite::Before(device.id), midi));
                    midi &= device.role == DeviceRole::MidiEffect;
                }
                continue;
            }
            let key = NodeKey::Device(device.id);
            // Spec: external-devices R13, R20. An external device reports
            // the interface's round trip and its Hardware Latency, which
            // delay compensation lines the rest of the set up with. An
            // external effect's node holds a delay line of that many frames
            // (R19), so the latency is part of the node's identity.
            let external_latency = device.external.as_ref().map_or(0, |external| {
                self.config.round_trip_latency.saturating_add(
                    external
                        .hardware_latency
                        .frames(self.config.sample_rate.hz()),
                )
            });
            let identity =
                Identity::Device(device.kind.clone(), device.media.clone(), external_latency);
            let (node, latency) = match self.previous(key, &identity) {
                Some(from) => (Err(from), self.previous_latency(key, &identity)),
                None => match self.registry.create(device, &config) {
                    Some(mut node) => {
                        if let Some(reader) = node.display() {
                            self.displays.insert(device.id, reader);
                        }
                        // A new node that plays media gets its decoded audio
                        // with this compile's update (WP-2.16).
                        for (slot, media) in device.media.iter().enumerate() {
                            if let Some(media) = *media {
                                // Only slot 0 has edits (the sampler's one
                                // sample).
                                let edits = if slot == 0 {
                                    device.sample_edits.clone()
                                } else {
                                    Vec::new()
                                };
                                build.media.push((device.id, slot, media, edits));
                            }
                        }
                        if !device.layers.is_default() {
                            build.layered.push(device.id);
                        }
                        let latency = node.latency();
                        (Ok(Instance::Device(node)), latency)
                    }
                    None => {
                        build.unhosted.push(device.id);
                        continue;
                    }
                },
            };
            let latency = if device.external.is_some() {
                external_latency
            } else {
                latency
            };
            let slot = claim(build, key, identity, !device.enabled, latency, node);
            build.device_slots.insert(device.id, slot);
            if let Some(external) = &device.external {
                build.slots[slot as usize].external = Some(external.audio_from);
                // Spec: external-devices R18, R19. An external effect sends
                // its signal to the interface's outputs, in a buffer of its
                // own that the engine mixes into them.
                if let Some(audio_to) = external.audio_to {
                    build.external_effects.push((slot, audio_to));
                }
            }
            // Spec: modulation R20. A modulator works its rows out before
            // any task runs.
            if bl_model::modulator::is_modulator_kind(&device.kind) {
                let row = build.modulators.len() as u32;
                build.slots[slot as usize].modulator =
                    if bl_model::modulator::is_signal_kind(&device.kind) {
                        build.signals.push(row);
                        ModKind::Signal(row)
                    } else {
                        ModKind::Control
                    };
                build.modulators.push((slot, device.id));
            }
            steps.push(Step::Node(slot));
            points.push(MeterPoint::new(owner, MeterSite::Before(device.id), midi));
            midi &= device.role == DeviceRole::MidiEffect;
            if let Some(settings) = &device.sidechain {
                let sources = sidechain_inputs.remove(&device.id).unwrap_or_default();
                *dependencies += sources.iter().filter(|s| s.from != own).count() as u32;
                self.add_sidechain(build, device.id, settings, slot, sources);
            }
            if device.midi_sidechain.is_some() {
                let from = build.midi_sources.get(&device.id).copied();
                *dependencies += u32::from(from.is_some_and(|from| from != own));
                build.midi_sidechains.push(DeviceMidiSidechain {
                    device: slot,
                    from: from.unwrap_or(NO_TASK),
                    kept: NO_TASK,
                });
            }
        }
        let start = build.steps.len() as u32;
        build.steps.extend(steps);
        let points_start = build.meter_points.len() as u32;
        build.meter_points.extend(points);
        build
            .meter_points
            .push(MeterPoint::new(owner, MeterSite::End, midi));
        (
            Span {
                start,
                end: build.steps.len() as u32,
            },
            Span {
                start: points_start,
                end: build.meter_points.len() as u32,
            },
        )
    }

    /// Adds a rack's chains (the racks spec, R3): each one's devices, its
    /// mixer node keyed by the chain, and a buffer of its own. A drum rack's
    /// drum chains also get their note routing and their sends to its return
    /// chains (the drum racks spec). A rack with no chains passes its input
    /// through, so it adds nothing. Returns the rack's place in
    /// `build.racks`.
    fn add_rack(
        &self,
        build: &mut Build,
        device: &Device,
        kind: RackKind,
        sidechain_inputs: &mut BTreeMap<DeviceId, Vec<Input>>,
        dependencies: &mut u32,
        midi: bool,
    ) -> Option<u32> {
        if device.chains.is_empty() {
            return None;
        }
        let config = self.config.node_config();
        let mut chains = Vec::with_capacity(device.chains.len());
        for chain in &device.chains {
            let (steps, meters) = self.add_flow(
                build,
                chain.devices.iter(),
                sidechain_inputs,
                dependencies,
                (MeterOwner::Chain(chain.id), midi),
            );
            let silenced = silenced_by_solo(device, chain);
            let mixer = self.place(
                build,
                NodeKey::ChainMixer(chain.id),
                Identity::Mixer,
                0,
                || {
                    Instance::Mixer(Box::new(TrackMixer::new(
                        &chain_mixer(chain, silenced),
                        0.5,
                        &config,
                    )))
                },
            );
            let zones = self.place(
                build,
                NodeKey::ChainZones(chain.id),
                Identity::Zones,
                0,
                || Instance::Zones(Box::new(ZoneFilter::new(&chain.zones, &config))),
            );
            let notes = if kind == RackKind::Drum && !chain.is_return {
                self.add_drum_notes(build, chain)
            } else {
                NO_SLOT
            };
            chains.push(RackChain {
                steps,
                meters,
                mixer,
                zones,
                buffer: build.chain_buffers,
                delay: NO_DELAY,
                taps: [None; 3],
                notes,
                sends: Span::default(),
                sum_delay: NO_DELAY,
                summed: !chain.is_return || chain.audio_to.is_none(),
            });
            build.chain_buffers += 1;
        }
        let selector = self.place(
            build,
            NodeKey::Device(device.id),
            Identity::Selector,
            0,
            || Instance::Selector(Box::new(ChainSelector::new(device, &config))),
        );
        let start = build.rack_chains.len() as u32;
        let drum_chains = device.chains.iter().take_while(|c| !c.is_return).count();
        let end = start + device.chains.len() as u32;
        let middle = start + drum_chains as u32;
        // A drum chain's send to each return chain, whose place is known
        // now: a route with the send's level, from −∞ dB in a new send.
        for (at, chain) in device.chains.iter().enumerate().take(drum_chains) {
            let sends_start = build.chain_sends.len() as u32;
            if kind == RackKind::Drum {
                for (to, ret) in device.chains.iter().enumerate().skip(drum_chains) {
                    let route = self.add_chain_send(build, chain, ret);
                    build.chain_sends.push(ChainSendRef {
                        route,
                        to: start + to as u32,
                    });
                }
            }
            chains[at].sends = Span {
                start: sends_start,
                end: build.chain_sends.len() as u32,
            };
        }
        build.rack_chains.extend(chains);
        for (at, chain) in device.chains.iter().enumerate() {
            build.chain_index.insert(chain.id, start + at as u32);
        }
        build.racks.push(Rack {
            // The task is pushed once its chain is built, in this place.
            task: build.tasks.len() as u32,
            mode: match kind {
                RackKind::AudioEffect => RackMode::Audio,
                RackKind::Instrument => RackMode::Instrument,
                RackKind::Drum => RackMode::Drum,
                RackKind::MidiEffect => RackMode::Midi,
            },
            chains: Span { start, end: middle },
            returns: Span { start: middle, end },
            bypass: !device.enabled,
            latency: 0,
            selector,
        });
        Some(build.racks.len() as u32 - 1)
    }

    /// The slot of a drum chain's note routing, made or migrated and told
    /// the chain's Receive, Play and Choke now.
    fn add_drum_notes(&self, build: &mut Build, chain: &Chain) -> u32 {
        let key = NodeKey::ChainNotes(chain.id);
        let settings = drum_settings(chain);
        if self.previous(key, &Identity::Drum).is_some() {
            for (param, value) in settings {
                build.restate.push(ParamEvent::now(key, param, value));
            }
        }
        self.place(build, key, Identity::Drum, 0, || {
            Instance::Drum(Box::new(DrumNotes::new(
                settings[0].1 as u8,
                settings[1].1 as u8,
                settings[2].1 as u8,
                self.hits.counter(chain.id),
            )))
        })
    }

    /// The slot of the route that carries a drum chain's send to the return
    /// chain `ret`.
    fn add_chain_send(&self, build: &mut Build, chain: &Chain, ret: &Chain) -> u32 {
        let config = self.config.node_config();
        let key = NodeKey::ChainSend {
            chain: chain.id,
            to: ret.id,
        };
        let level = chain.send(ret.id);
        if self.previous(key, &Identity::Route).is_some() {
            build
                .restate
                .push(ParamEvent::now(key, route::LEVEL, level));
        }
        self.place(build, key, Identity::Route, 0, || {
            Instance::Route(Box::new(Route::new(level, false, &config)))
        })
    }

    /// Adds the sidechain of the device in `slot`: its settings, made or
    /// migrated and told them now, its buffer, and its sources.
    fn add_sidechain(
        &self,
        build: &mut Build,
        device: DeviceId,
        settings: &bl_model::Sidechain,
        slot: u32,
        sources: Vec<Input>,
    ) {
        let config = self.config.node_config();
        let key = NodeKey::Sidechain(device);
        if self.previous(key, &Identity::Sidechain).is_some() {
            build.restate.extend(
                sidechain_settings(settings)
                    .map(|(param, value)| ParamEvent::now(key, param, value)),
            );
        }
        let key = self.place(build, key, Identity::Sidechain, 0, || {
            Instance::Sidechain(Box::new(SidechainKey::new(settings, &config)))
        });
        let start = build.sidechain_inputs.len() as u32;
        build.sidechain_inputs.extend(sources);
        let buffer = build.sidechains.len() as u32;
        build.sidechains.push(DeviceSidechain {
            device: slot,
            key,
            buffer,
            inputs: Span {
                start,
                end: build.sidechain_inputs.len() as u32,
            },
            before: 0,
        });
    }

    /// Where `key`'s node sat in the last program, if it was the same kind
    /// of node, so it migrates.
    fn previous(&self, key: NodeKey, identity: &Identity) -> Option<u32> {
        self.layout
            .get(&key)
            .filter(|placed| placed.identity == *identity)
            .map(|placed| placed.slot)
    }

    /// The latency `key`'s node reported when it was made.
    fn previous_latency(&self, key: NodeKey, identity: &Identity) -> u32 {
        self.layout
            .get(&key)
            .filter(|placed| placed.identity == *identity)
            .map_or(0, |placed| placed.latency)
    }

    /// Gives a mixer or a route a slot: the one that ran under `key` in the
    /// last program, or a new one from `make`.
    fn place(
        &self,
        build: &mut Build,
        key: NodeKey,
        identity: Identity,
        latency: u32,
        make: impl FnOnce() -> Instance,
    ) -> u32 {
        let node = match self.previous(key, &identity) {
            Some(from) => Err(from),
            None => Ok(make()),
        };
        claim(build, key, identity, false, latency, node)
    }
}

/// Puts a node in the next slot: a new node, or (`Err`) the slot of the node
/// in the last program that migrates into it.
fn claim(
    build: &mut Build,
    key: NodeKey,
    identity: Identity,
    bypass: bool,
    latency: u32,
    node: Result<Instance, u32>,
) -> u32 {
    let slot = build.slots.len() as u32;
    let node = match node {
        Ok(node) => Some(node),
        Err(from) => {
            build.migrations.push(Migration { from, to: slot });
            None
        }
    };
    build.slots.push(NodeSlot {
        key,
        node,
        bypass,
        // A switched-off device passes its input through, adding nothing, but
        // its latency is remembered in the layout so it comes back when the
        // device is switched on again.
        latency: if bypass { 0 } else { latency },
        delay: 0,
        modulator: ModKind::None,
        external: None,
        external_to: None,
        external_out: None,
        outputs: Span::default(),
    });
    build.layout.insert(
        key,
        Placed {
            slot,
            identity,
            latency,
            delay: 0,
        },
    );
    slot
}

impl std::fmt::Debug for Compiler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Compiler")
            .field("generation", &self.generation)
            .field("nodes", &self.layout.len())
            .field("sources", &self.sources)
            .finish_non_exhaustive()
    }
}

/// What a delay line holds back, which a recompile finds it by: the line
/// keeps its audio across the swap when the same thing is held back by as
/// many frames (WP-2.11).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum DelayKey {
    /// A track's clips.
    Clips(TrackId),
    /// The signal of a track into a route, which holds the node's key.
    Input(NodeKey, TrackId),
    /// A rack chain's output, held to the rack's slowest chain, given by the
    /// key of the chain's mixer.
    Chain(NodeKey),
    /// A drum rack's chain, held to the rack's sum.
    ChainSum(NodeKey),
}

/// A program under construction.
#[derive(Default)]
struct Build {
    slots: Vec<NodeSlot>,
    layout: BTreeMap<NodeKey, Placed>,
    migrations: Vec<Migration>,
    tasks: Vec<Task>,
    inputs: Vec<Input>,
    /// The steps of every chain, a track's or a rack chain's (WP-5.1).
    steps: Vec<Step>,
    /// The meter points of every chain (WP-6.69): one per step and one at
    /// the end, in the chain's order.
    meter_points: Vec<MeterPoint>,
    racks: Vec<Rack>,
    rack_chains: Vec<RackChain>,
    /// The sends of drum chains to their racks' return chains.
    chain_sends: Vec<ChainSendRef>,
    /// How many rack chains have a buffer of their own so far.
    chain_buffers: u32,
    /// Each rack chain's place in `rack_chains`.
    chain_index: BTreeMap<ChainId, u32>,
    sidechains: Vec<DeviceSidechain>,
    sidechain_inputs: Vec<Input>,
    /// The task each device's MIDI sidechain listens to (WP-5.27).
    midi_sources: BTreeMap<DeviceId, u32>,
    /// The slot each device has in `slots` (WP-5.45).
    device_slots: BTreeMap<DeviceId, u32>,
    midi_sidechains: Vec<DeviceMidiSidechain>,
    unhosted: Vec<DeviceId>,
    /// The modulators' slots and devices, in the order their rows sit on the
    /// bus (WP-5.10).
    modulators: Vec<(u32, DeviceId)>,
    /// Which of the modulators are driven by the signal (WP-5.11): their
    /// places among `modulators`.
    signals: Vec<u32>,
    /// Devices new in this program that play media, with the media they
    /// name, one entry per filled sample slot: their decoded audio goes
    /// with this compile's update (WP-2.16).
    media: Vec<(DeviceId, usize, ContentHash, Vec<bl_model::SampleEdit>)>,
    /// Devices new in this program that hold layers (WP-5.13): their layers,
    /// with each one's decoded audio, go with this compile's update.
    layered: Vec<DeviceId>,
    /// Tracks whose clip player is new in this program.
    new_players: BTreeSet<TrackId>,
    /// The state of routes and players that migrate in, which an edit may
    /// have changed: sent with the program.
    restate: Vec<ParamEvent>,
    /// The length in frames of each delay line the program needs, in the
    /// order the inputs and players name them (WP-2.11).
    delays: Vec<u32>,
    /// What each of `delays` holds back, which the next compile finds it by.
    delay_keys: Vec<DelayKey>,
    /// The note delay of each MIDI track that plays clips, with its length
    /// in frames (WP-2.11).
    note_delays: Vec<(TrackId, u32)>,
    /// The external effects in the program, with their slots and the
    /// interface's outputs they send to (WP-5.35).
    external_effects: Vec<(u32, bl_model::Channel)>,
}

/// The key of the route that carries `edge`.
fn route_key(edge: &Edge) -> NodeKey {
    match edge.via {
        Via::Output => NodeKey::Output(edge.from),
        Via::Send => NodeKey::Send {
            track: edge.from,
            to: edge.to,
        },
        Via::Input => NodeKey::Input(edge.to),
        Via::Sidechain(device) => NodeKey::SidechainInput(device),
        Via::Chain(chain) => NodeKey::ChainOutput(chain),
    }
}

/// A sidechain's settings as its [`SidechainKey`]'s parameters.
fn sidechain_settings(settings: &bl_model::Sidechain) -> impl Iterator<Item = (ParamId, f64)> {
    [
        (sidechain::ENABLED, switch(settings.enabled)),
        (sidechain::GAIN, settings.gain),
        (sidechain::MIX, settings.mix),
        (sidechain::LISTEN, switch(settings.listen)),
    ]
    .into_iter()
}

/// Whether an audio track's clips are heard: not while its Monitor is In
/// (the routing spec).
fn clips_audible(track: &Track) -> bool {
    track.routing().monitor != bl_model::Monitor::In
}

/// The order to run the tasks in, each after every task that feeds it and
/// otherwise in display order (Kahn's algorithm, taking the first ready
/// task each time), and the tasks waiting on each task.
fn run_order(build: &Build, waits: &[(u32, u32)]) -> (Vec<u32>, Vec<Vec<u32>>) {
    let tasks = &build.tasks;
    let mut dependents: Vec<Vec<u32>> = vec![Vec::new(); tasks.len()];
    let mut waiting: Vec<u32> = vec![0; tasks.len()];
    for (t, task) in tasks.iter().enumerate() {
        let sidechains = build.sidechains[task.sidechains.range()]
            .iter()
            .flat_map(|s| &build.sidechain_inputs[s.inputs.range()]);
        // A device listening to its own track before its devices waits on
        // nothing.
        let sidechains = sidechains.filter(|input| input.from as usize != t);
        for input in build.inputs[task.inputs.range()].iter().chain(sidechains) {
            dependents[input.from as usize].push(t as u32);
            waiting[t] += 1;
        }
        // A MIDI sidechain waits for the track whose notes it hears, unless
        // that is its own track.
        for midi in &build.midi_sidechains[task.midi_sidechains.range()] {
            if midi.from != NO_TASK && midi.from as usize != t {
                dependents[midi.from as usize].push(t as u32);
                waiting[t] += 1;
            }
        }
    }
    for &(from, to) in waits {
        dependents[from as usize].push(to);
        waiting[to as usize] += 1;
    }
    let mut left = waiting;
    let mut ready: BTreeSet<u32> = (0..tasks.len() as u32)
        .filter(|&t| left[t as usize] == 0)
        .collect();
    let mut order = Vec::with_capacity(tasks.len());
    while let Some(t) = ready.pop_first() {
        order.push(t);
        for &next in &dependents[t as usize] {
            left[next as usize] -= 1;
            if left[next as usize] == 0 {
                ready.insert(next);
            }
        }
    }
    debug_assert_eq!(order.len(), tasks.len(), "the routing graph has no loop");
    (order, dependents)
}

/// The frames into the song an edge's signal has when it reaches the summing
/// point: none for a Pre FX tap, and the source's whole chain after it.
fn tap_latency(arrival: &[u32], chain: &[u32], from: usize, tap: Tap) -> u32 {
    match tap {
        Tap::PreFx => arrival[from],
        Tap::PostFx | Tap::PostMixer => arrival[from] + chain[from],
    }
}

/// The same for an input that may tap a rack chain, whose `(start, length)`
/// within its task's chain `rack_chains` holds: the signal at the rack's
/// input has the chain's start behind it, and after the chain's devices the
/// chain's length too (the racks spec, R40).
fn input_latency(arrival: &[u32], chain: &[u32], rack_chains: &[(u32, u32)], input: &Input) -> u32 {
    // A device's extra output is there where the device is in its chain, as
    // a Post FX tap is (WP-5.45); one the program has no buffer for carries
    // nothing, so the track waits for none of it.
    if input.output == NO_OUTPUT_BUFFER {
        return arrival[input.from as usize];
    }
    if input.output != NO_OUTPUT {
        return arrival[input.from as usize] + input.before;
    }
    if input.chain == NO_CHAIN {
        return tap_latency(arrival, chain, input.from as usize, input.tap);
    }
    let (start, length) = rack_chains[input.chain as usize];
    let after = if input.tap == Tap::PreFx { 0 } else { length };
    arrival[input.from as usize] + start + after
}

/// Works out and records delay compensation (WP-2.11): how far each track's
/// clips and each route are held back so every path into a track lines up,
/// each sidechain's source lines up with its device's own input, and each
/// node's parameter changes are held back with the audio they act on.
fn compensate(build: &mut Build, order: &[u32], document: &Document, rate: u32) {
    let tasks = build.tasks.len();
    // Spec: delay-compensation R7. While delay compensation is off, nothing
    // is held back and no node's latency counts, so every delay line stays as
    // a new build left it (all [`NO_DELAY`]) and a track's own delay has no
    // effect.
    if !document.settings().delay_compensation {
        return;
    }
    // Spec: mixer R28. Each track's own delay, in frames: positive delays the
    // track, negative plays it early. It joins the compensation here by
    // counting against the latency of the track's path into each summing
    // point, so a positive delay makes the path later and a negative one
    // makes the other paths wait (the mixer spec).
    let delay_of: Vec<i64> = build
        .tasks
        .iter()
        .map(|task| {
            document
                .track(task.track)
                .map_or(0, |track| track.delay().frames(rate))
        })
        .collect();
    // Spec: mixer R28. A track's delay is applied where its output lands, at
    // each input it feeds; Main feeds nothing, so its own delay is applied
    // where its sum is made instead, holding back every input into Main and
    // Main's own parameter changes by it.
    let main = document.main_id();
    let main_delay = document.main().delay().frames(rate);
    // Spec: delay-compensation R8 and R9. A track whose input is monitored,
    // while Reduced Latency When Monitoring is on and it doesn't keep its
    // monitoring latency, is given the lowest possible latency: its own input
    // isn't held back and its path doesn't raise a summing point's target.
    let low_latency: Vec<bool> = build
        .tasks
        .iter()
        .map(|task| {
            document.settings().reduced_latency_monitoring
                && document.track(task.track).is_some_and(|track| {
                    track.routing().monitoring(track.mixer().arm)
                        && !track.keep_monitoring_latency()
                })
        })
        .collect();
    // The frames into the song an input's signal has, counting the source
    // track's own delay. Negative counts make the other paths wait.
    let input_arrival = |arrival: &[u32], chain: &[u32], rack_at: &[(u32, u32)], input: &Input| {
        if low_latency[input.from as usize] {
            return 0i64;
        }
        i64::from(input_latency(arrival, chain, rack_at, input)) - delay_of[input.from as usize]
    };
    // A rack adds its slowest chain's latency and holds the others back to
    // it, so its chains reach the sum aligned (the racks spec, R16). A rack
    // comes after the racks in its chains, so each is worked out from
    // finished ones.
    for r in 0..build.racks.len() {
        let rack = build.racks[r];
        let latencies: Vec<u32> = build.rack_chains[rack.chains.range()]
            .iter()
            .map(|chain| flow_latency(build, chain.steps))
            .collect();
        let slowest = latencies.iter().copied().max().unwrap_or(0);
        for (i, latency) in rack.chains.range().zip(latencies) {
            if slowest > latency {
                let key = DelayKey::Chain(build.slots[build.rack_chains[i].mixer as usize].key);
                let index = line(build, slowest - latency, key);
                build.rack_chains[i].delay = index;
            }
        }
        // A drum rack's sends take the chains at the slowest one's time,
        // the return chains run from there, and the rack's output waits for
        // the slowest return chain it sums (the drum racks spec, R22).
        let returned: Vec<(usize, u32)> = rack
            .returns
            .range()
            .filter(|&i| build.rack_chains[i].summed)
            .map(|i| (i, flow_latency(build, build.rack_chains[i].steps)))
            .collect();
        let total = slowest + returned.iter().map(|&(_, l)| l).max().unwrap_or(0);
        build.racks[r].latency = total;
        if total > slowest {
            for i in rack.chains.range() {
                let key = DelayKey::ChainSum(build.slots[build.rack_chains[i].mixer as usize].key);
                let index = line(build, total - slowest, key);
                build.rack_chains[i].sum_delay = index;
            }
        }
        for (i, latency) in returned {
            if total > slowest + latency {
                let key = DelayKey::Chain(build.slots[build.rack_chains[i].mixer as usize].key);
                let index = line(build, total - slowest - latency, key);
                build.rack_chains[i].delay = index;
            }
        }
    }
    // The frames each track's device chain adds, and how far into it each
    // node's own input is.
    let mut chain = vec![0u32; tasks];
    let mut offsets: Vec<Vec<(u32, u32)>> = vec![Vec::new(); tasks];
    // Where each rack chain's input is in its task's chain, and what its
    // devices add, for routes that tap it.
    let mut rack_at = vec![(0u32, 0u32); build.rack_chains.len()];
    for (t, task) in build.tasks.clone().iter().enumerate() {
        chain[t] = flow_latency(build, task.chain);
        offsets[t] = node_offsets(build, task.chain, &mut rack_at);
        for &(slot, before) in &offsets[t] {
            for s in task.sidechains.range() {
                if build.sidechains[s].device == slot {
                    build.sidechains[s].before = before;
                }
            }
        }
    }
    // Where in its source track's chain a tapped device's extra output is: a
    // route that takes one carries the signal where the device sits, as a
    // Post FX tap of that point would (WP-5.45).
    for t in 0..tasks {
        let task = build.tasks[t];
        for i in task.inputs.range() {
            let (device, from) = (build.inputs[i].device, build.inputs[i].from);
            if device == NO_TASK {
                continue;
            }
            if let Some(&(_, before)) = offsets[from as usize]
                .iter()
                .find(|&&(slot, _)| slot == device)
            {
                build.inputs[i].before = before;
            }
        }
    }
    for (at, rack_chain) in rack_at.iter_mut().zip(&build.rack_chains) {
        at.1 = flow_latency(build, rack_chain.steps);
    }
    // The frames from song time to each track's input sum: what its inputs
    // and its devices' sidechain sources make it wait for.
    let mut arrival = vec![0u32; tasks];
    for &t in order {
        let t = t as usize;
        if low_latency[t] {
            arrival[t] = 0;
            continue;
        }
        let task = build.tasks[t];
        let mut arrived = 0i64;
        for input in &build.inputs[task.inputs.range()] {
            arrived = arrived.max(input_arrival(&arrival, &chain, &rack_at, input));
        }
        for s in task.sidechains.range() {
            let sc = build.sidechains[s];
            for input in &build.sidechain_inputs[sc.inputs.range()] {
                let need = input_arrival(&arrival, &chain, &rack_at, input) - i64::from(sc.before);
                arrived = arrived.max(need);
            }
        }
        arrival[t] = arrived.max(0) as u32;
    }
    // Hold each path back to what the longest one takes, and line each
    // node's parameter changes up with the audio they act on.
    for t in 0..tasks {
        let task = build.tasks[t];
        let a = (i64::from(arrival[t]) + if task.track == main { main_delay } else { 0 }).max(0);
        for i in task.inputs.range() {
            let input = build.inputs[i];
            // Spec: delay-compensation R5. A route scales the signal after
            // its delay line, which is `a` frames behind song time whatever
            // the line's own length, so its send level's changes wait `a`.
            let low = low_latency[input.from as usize];
            build.slots[input.route as usize].delay = if low { 0 } else { a as u32 };
            if low {
                continue;
            }
            let delay = a - input_arrival(&arrival, &chain, &rack_at, &input);
            if delay > 0 {
                let key = input_key(build, &input);
                let index = line(build, delay as u32, key);
                build.inputs[i].delay = index;
            }
        }
        for s in task.sidechains.range() {
            let sc = build.sidechains[s];
            for i in sc.inputs.range() {
                let input = build.sidechain_inputs[i];
                if low_latency[input.from as usize] {
                    continue;
                }
                let delay =
                    (a + i64::from(sc.before)) - input_arrival(&arrival, &chain, &rack_at, &input);
                if delay > 0 {
                    let key = input_key(build, &input);
                    let index = line(build, delay as u32, key);
                    build.sidechain_inputs[i].delay = index;
                }
            }
        }
        // A MIDI track's player writes notes, so the notes are held back,
        // not the buffer, which stays silent until its devices play them.
        if let Some(delay) = task.note_delay {
            build.note_delays[delay as usize].1 = a as u32;
        } else if task.player.is_some() && a > 0 {
            let index = line(build, a as u32, DelayKey::Clips(task.track));
            build.tasks[t].clip_delay = index;
        }
        if let Some(slot) = task.player {
            build.slots[slot as usize].delay = a as u32;
        }
        for &(slot, before) in &offsets[t] {
            build.slots[slot as usize].delay = a as u32 + before;
        }
        build.slots[task.mixer as usize].delay = a as u32 + chain[t];
    }
}

/// The frames `track`'s audio is held back on its way into Main: the delay
/// lines of the inputs that carry it from one track to the next (the delay
/// compensation spec, R2).
fn main_compensation(
    build: &Build,
    document: &Document,
    task_of: &BTreeMap<TrackId, u32>,
    track: TrackId,
) -> u32 {
    let mut total = 0u32;
    let mut at = track;
    // A routing may make a loop, which the engine plays as silence; the
    // walk is bounded by the number of tracks.
    for _ in 0..task_of.len() {
        let Some(to) = document.output_of(at) else {
            break;
        };
        let (Some(&from_task), Some(&to_task)) = (task_of.get(&at), task_of.get(&to)) else {
            break;
        };
        let task = build.tasks[to_task as usize];
        if let Some(input) = build.inputs[task.inputs.range()]
            .iter()
            .find(|i| i.from == from_task && i.tap == Tap::PostMixer)
            && input.delay != NO_DELAY
        {
            total = total.saturating_add(build.delays[input.delay as usize]);
        }
        at = to;
    }
    total
}

/// The frames a chain of steps adds: its devices' latencies and its racks'.
fn flow_latency(build: &Build, steps: Span) -> u32 {
    build.steps[steps.range()]
        .iter()
        .map(|&step| match step {
            Step::Node(slot) => build.slots[slot as usize].latency,
            Step::Rack(rack) => {
                let rack = build.racks[rack as usize];
                if rack.bypass { 0 } else { rack.latency }
            }
        })
        .sum()
}

/// Each node in a chain of steps, a rack's chain mixers and the nodes of
/// the racks inside it included, with the frames into the task's chain at
/// which its input is, given the chain starts `start` frames in (the racks
/// spec, R16): what the audio it acts on is behind song time by, and so what
/// its parameter changes are held back by. A switched-off rack's contents
/// don't run, so they aren't listed. Each rack chain's start is recorded in
/// `rack_at`, by its place in `rack_chains`.
fn node_offsets(build: &Build, steps: Span, rack_at: &mut [(u32, u32)]) -> Vec<(u32, u32)> {
    fn walk(
        build: &Build,
        steps: Span,
        start: u32,
        out: &mut Vec<(u32, u32)>,
        rack_at: &mut [(u32, u32)],
    ) {
        let mut before = start;
        for &step in &build.steps[steps.range()] {
            match step {
                Step::Node(slot) => {
                    out.push((slot, before));
                    before += build.slots[slot as usize].latency;
                }
                Step::Rack(rack) => {
                    let rack = build.racks[rack as usize];
                    if rack.bypass {
                        continue;
                    }
                    // A drum rack's return chains start where its slowest
                    // drum chain ends, which is where its sends take them.
                    let sent = rack
                        .chains
                        .range()
                        .map(|i| flow_latency(build, build.rack_chains[i].steps))
                        .max()
                        .unwrap_or(0);
                    for i in rack.all().range() {
                        let chain = &build.rack_chains[i];
                        let start = if i < rack.returns.start as usize {
                            before
                        } else {
                            before + sent
                        };
                        rack_at[i].0 = start;
                        walk(build, chain.steps, start, out, rack_at);
                        out.push((chain.mixer, start + flow_latency(build, chain.steps)));
                    }
                    // A send's level acts on the audio at that time too.
                    for i in rack.chains.range() {
                        for send in &build.chain_sends[build.rack_chains[i].sends.range()] {
                            out.push((send.route, before + sent));
                        }
                    }
                    before += rack.latency;
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(build, steps, 0, &mut out, rack_at);
    out
}

/// A rack chain's mixer as a node's state: its Volume and Pan, and muted
/// while its activator is off or its rack's solo silences it
/// ([`silenced_by_solo`]).
fn chain_mixer(chain: &Chain, silenced: bool) -> Mixer {
    Mixer {
        volume: chain.mixer.volume,
        pan: chain.mixer.pan,
        mute: !chain.mixer.enabled || silenced,
        ..Mixer::default()
    }
}

/// Whether the solo of a chain of `rack` silences `chain`: while any chain
/// is soloed, the others add nothing (the racks spec, R5). A drum rack's
/// return chains stay audible while only drum chains are soloed, carrying
/// just their sends (the drum racks spec, R9).
fn silenced_by_solo(rack: &Device, chain: &Chain) -> bool {
    let soloed = |returns: bool| {
        rack.chains
            .iter()
            .any(|c| c.mixer.solo && c.is_return == returns)
    };
    !chain.mixer.solo
        && if chain.is_return {
            soloed(true)
        } else {
            soloed(false) || soloed(true)
        }
}

/// The parameters of a drum chain's note routing that the document sets.
fn drum_settings(chain: &Chain) -> [(ParamId, f64); 3] {
    [
        (
            drum::param::RECEIVE,
            match chain.receive {
                bl_model::Receive::AllNotes => drum::param::ALL_NOTES,
                bl_model::Receive::Note(note) => f64::from(note),
            },
        ),
        (drum::param::PLAY, f64::from(chain.play)),
        (drum::param::CHOKE, f64::from(chain.choke.unwrap_or(0))),
    ]
}

/// A new delay line of `len` frames, returning its index in the program.
fn line(build: &mut Build, len: u32, key: DelayKey) -> u32 {
    let index = build.delays.len() as u32;
    build.delays.push(len);
    build.delay_keys.push(key);
    index
}

/// The signal of the track of task `from` into `input`'s route, which a
/// delay line holds back.
fn input_key(build: &Build, input: &Input) -> DelayKey {
    DelayKey::Input(
        build.slots[input.route as usize].key,
        build.tasks[input.from as usize].track,
    )
}

/// Fills in each task's span of the flattened dependents lists.
fn with_dependents(mut tasks: Vec<Task>, dependents: &[Vec<u32>]) -> Vec<Task> {
    let mut start = 0u32;
    for (task, waiting) in tasks.iter_mut().zip(dependents) {
        let end = start + waiting.len() as u32;
        task.dependents = Span { start, end };
        start = end;
    }
    tasks
}

/// The loop the engine plays for `document`: its loop brace if the Loop
/// switch is on.
#[must_use]
pub(crate) fn loop_region(document: &Document) -> Option<LoopRegion> {
    let brace = document.settings().arrangement_loop;
    brace.enabled.then(|| LoopRegion {
        start: brace.start,
        end: brace.end(),
    })
}

/// Where recording punches in and out for `document`: its loop brace's
/// ends, for the Punch-In and Punch-Out switches that are on, whether or not
/// the Loop switch is.
#[must_use]
pub(crate) fn punch(document: &Document) -> Punch {
    let settings = document.settings();
    let brace = settings.arrangement_loop;
    Punch {
        start: settings.punch_in.then_some(brace.start),
        end: settings.punch_out.then(|| brace.end()),
    }
}

/// Sends each parameter a rack's macro drives its new value when the macro
/// moved without automation (macros spec, R7 and R8): the node holding the
/// mapped parameter hears it, as a hand move on the macro reaches it.
fn macro_events(
    document: &Document,
    before: &Device,
    after: &Device,
    params: &mut Vec<ParamEvent>,
    descriptor: &impl Fn(&bl_model::DeviceKind, ParamId) -> Option<bl_model::ParamDescriptor>,
) {
    if after.rack_kind().is_none() {
        return;
    }
    for mapping in &after.macros.mappings {
        let macro_param = bl_model::macro_param(mapping.macro_number);
        let was = before.params.get(&macro_param).copied();
        let is = after.params.get(&macro_param).copied();
        if was == is {
            continue;
        }
        let position =
            mapping.min + is.unwrap_or(0.0).clamp(0.0, 1.0) * (mapping.max - mapping.min);
        match mapping.target {
            bl_model::MacroTarget::Device { device, param } => {
                let Some(kind) = document.device(device).map(|d| d.kind.clone()) else {
                    continue;
                };
                let Some(descriptor) = descriptor(&kind, param) else {
                    continue;
                };
                let value = bl_model::modulation::Travel::Param(&descriptor).value(position);
                params.push(ParamEvent::now(NodeKey::Device(device), param, value));
            }
            bl_model::MacroTarget::Chain { chain, param } => {
                let (id, travel) = match param {
                    bl_model::MixerParam::Volume => {
                        (mixer::track::VOLUME, bl_model::modulation::Travel::Fader)
                    }
                    bl_model::MixerParam::Pan => {
                        (mixer::track::PAN, bl_model::modulation::Travel::Pan)
                    }
                    bl_model::MixerParam::Mute => {
                        (mixer::track::MUTE, bl_model::modulation::Travel::Switch)
                    }
                    // A chain has no split pan (the macros spec, R4).
                    bl_model::MixerParam::SplitLeft | bl_model::MixerParam::SplitRight => continue,
                };
                params.push(ParamEvent::now(
                    NodeKey::ChainMixer(chain),
                    id,
                    travel.value(position),
                ));
            }
        }
    }
}

/// Whether a change set needs a new program; collects parameter changes for
/// the nodes that keep running either way.
fn needs_program(
    changes: &ChangeSet,
    old: &Document,
    new: &Document,
    params: &mut Vec<ParamEvent>,
    descriptor: &impl Fn(&bl_model::DeviceKind, ParamId) -> Option<bl_model::ParamDescriptor>,
) -> bool {
    // The tempo map travels in the program. The Arrangement loop doesn't: a
    // loop edit is a transport command (see `Compiler::sync`).
    let (before, after) = (old.settings(), new.settings());
    let settings = changes.settings
        && (before.tempo != after.tempo
            || before.time_signatures != after.time_signatures
            || before.delay_compensation != after.delay_compensation
            || before.reduced_latency_monitoring != after.reduced_latency_monitoring);
    // Every track's mixer node reads the crossfader, so a moved position
    // reaches every one of them as a parameter change (R22).
    if changes.settings && before.crossfader != after.crossfader {
        for track in new.tracks() {
            params.push(ParamEvent::now(
                NodeKey::Mixer(track.id()),
                mixer::track::CROSSFADER,
                after.crossfader,
            ));
        }
    }
    // The maps a scene's tempo and signature put in place travel in the
    // program too (WP-3.5).
    let scene_maps = |document: &Document| {
        document
            .scenes()
            .map(|scene| (scene.id, scene.tempo, scene.signature))
            .collect::<Vec<_>>()
    };
    let scenes = !changes.scenes.is_empty() && scene_maps(old) != scene_maps(new);
    // Cue mode (WP-3.18): switching it on or off, or moving the Cue Out,
    // changes the program; the Cue Volume reaches the Cue Out's mixer as a
    // parameter, and the Cue switches reach their routes (see
    // `route_events`).
    let cue = (old.cue_active(), new.cue_active());
    let cue_moved =
        cue.0 != cue.1 || (cue.1 && old.main().routing().cue_out != new.main().routing().cue_out);
    if cue == (true, true) && before.cue_volume != after.cue_volume {
        params.push(ParamEvent::now(
            NodeKey::CueMixer,
            mixer::track::VOLUME,
            after.cue_volume,
        ));
    }
    let mut recompile = settings || changes.layout || scenes || cue_moved;
    for (&id, &change) in &changes.tracks {
        match (change, old.track(id), new.track(id)) {
            (Change::Modified, Some(before), Some(after)) => {
                let same_chain = before.same_chain_shape(after);
                // A return's Pre/Post switch moves where its sends tap; a
                // routing change moves routes. Monitor alone doesn't: it
                // opens and closes routes (see `route_events`).
                let (was, is) = (before.routing(), after.routing());
                // A freeze takes the track's devices out of the program and
                // hides a frozen group's tracks (WP-4.14, R6, R14).
                if !same_chain
                    || before.is_frozen() != after.is_frozen()
                    || !before.children().eq(after.children())
                    || before.mixer().tap != after.mixer().tap
                    || was.input != is.input
                    || was.channel != is.channel
                    || was.output != is.output
                    || was.midi_output != is.midi_output
                    || before.delay() != after.delay()
                    || before.keep_monitoring_latency() != after.keep_monitoring_latency()
                {
                    recompile = true;
                }
                // A MIDI track's routes travel in the program (WP-2.2),
                // with whether it monitors; an audio track's monitor change
                // opens and closes routes instead.
                if after.kind() == TrackKind::Midi
                    && (was.monitor != is.monitor || before.mixer().arm != after.mixer().arm)
                {
                    recompile = true;
                }
                mixer_events(id, before.mixer(), after.mixer(), params);
                send_events(old, new, before, after, params);
            }
            _ => recompile = true,
        }
    }
    for (&id, &change) in &changes.devices {
        match (change, old.device(id), new.device(id)) {
            (Change::Modified, Some(before), Some(after)) => {
                let (was, is) = (before.sidechain.as_ref(), after.sidechain.as_ref());
                if before.kind != after.kind
                    || before.enabled != after.enabled
                    || before.media != after.media
                    || was.map(|s| &s.input) != is.map(|s| &s.input)
                    || before.external != after.external
                    || before.midi_sidechain != after.midi_sidechain
                {
                    recompile = true;
                }
                if let (Some(was), Some(is)) = (was, is) {
                    let then: Vec<_> = sidechain_settings(was).collect();
                    for (param, value) in sidechain_settings(is) {
                        if !then.contains(&(param, value)) {
                            params.push(ParamEvent::now(NodeKey::Sidechain(id), param, value));
                        }
                    }
                }
                param_events(NodeKey::Device(id), &before.params, &after.params, params);
                macro_events(new, before, after, params, descriptor);
            }
            _ => recompile = true,
        }
    }
    chain_events(changes, old, new, params);
    // Clip changes never recompile: `Compiler::sync` sends the track's clip
    // player its clips as node data (the audio clips spec, R12). The Session
    // View's slots are the exception: which slots hold a clip travels in the
    // program's scene table, which a scene launch reads (WP-3.2).
    recompile
        || scene_slots(old) != scene_slots(new)
        || FollowTable::of(old) != FollowTable::of(new)
}

/// Every modulator driven by audio, notes or MIDI with its mappings: what
/// decides which tracks wait for which (WP-5.11).
fn signal_mappings(document: &Document) -> Vec<(DeviceId, Vec<bl_model::modulator::Mapping>)> {
    document
        .tracks()
        .flat_map(|track| track.devices_deep())
        .filter(|device| bl_model::modulator::is_signal_kind(&device.kind))
        .map(|device| (device.id, device.mappings.clone()))
        .collect()
}

/// Every modulator in track and chain order with its mappings: what decides
/// which rows the nodes' lists name (WP-5.10).
fn modulator_mappings(document: &Document) -> Vec<(DeviceId, Vec<bl_model::modulator::Mapping>)> {
    document
        .tracks()
        .flat_map(|track| track.devices_deep())
        .filter(|device| bl_model::modulator::is_modulator_kind(&device.kind))
        .map(|device| (device.id, device.mappings.clone()))
        .collect()
}

/// Parameter changes for the mixers of the chains of racks that changed
/// (the racks spec, R6): Volume, Pan, and whether the chain is muted, which
/// its activator and its rack's solo decide together. A chain that is new
/// has a mixer made with its state, so only chains in both documents count,
/// wherever they sit in them.
fn chain_events(changes: &ChangeSet, old: &Document, new: &Document, params: &mut Vec<ParamEvent>) {
    /// A drum chain's note routing and the levels of its sends to the
    /// rack's return chains.
    type Drum = ([(ParamId, f64); 3], Vec<(ChainId, f64)>);
    /// What a chain's nodes hold: its mixer, and for a drum chain its drum
    /// settings.
    struct State {
        mixer: Mixer,
        drum: Option<Drum>,
    }
    fn states(document: &Document, ids: &[DeviceId]) -> BTreeMap<ChainId, State> {
        let mut states = BTreeMap::new();
        for &id in ids {
            let Some(rack) = document.device(id) else {
                continue;
            };
            for chain in &rack.chains {
                let drum =
                    (rack.rack_kind() == Some(RackKind::Drum) && !chain.is_return).then(|| {
                        let sends = rack
                            .chains
                            .iter()
                            .filter(|c| c.is_return)
                            .map(|c| (c.id, chain.send(c.id)))
                            .collect();
                        (drum_settings(chain), sends)
                    });
                states.insert(
                    chain.id,
                    State {
                        mixer: chain_mixer(chain, silenced_by_solo(rack, chain)),
                        drum,
                    },
                );
            }
        }
        states
    }
    let racks: Vec<DeviceId> = changes
        .devices
        .keys()
        .copied()
        .filter(|&id| {
            [old, new]
                .iter()
                .any(|d| d.device(id).is_some_and(|d| d.rack_kind().is_some()))
        })
        .collect();
    if racks.is_empty() {
        return;
    }
    let (before, after) = (states(old, &racks), states(new, &racks));
    for (chain, was) in before {
        let Some(is) = after.get(&chain) else {
            continue;
        };
        mixer_events_for(NodeKey::ChainMixer(chain), &was.mixer, &is.mixer, params);
        if let (Some((then, sent)), Some((now, sends))) = (&was.drum, &is.drum) {
            for (&(param, was), &(_, is)) in then.iter().zip(now) {
                if was != is {
                    params.push(ParamEvent::now(NodeKey::ChainNotes(chain), param, is));
                }
            }
            for &(to, level) in sends {
                // A send to a return chain that is new has a route made
                // with its level.
                if sent.iter().any(|&(t, l)| t == to && l != level) {
                    let key = NodeKey::ChainSend { chain, to };
                    params.push(ParamEvent::now(key, route::LEVEL, level));
                }
            }
        }
    }
    // The zones of the chains in both documents (WP-5.3): each number that
    // changed.
    for &id in &racks {
        for chain in new.device(id).into_iter().flat_map(|rack| &rack.chains) {
            let Some(was) = old.chain(chain.id) else {
                continue;
            };
            let then: Vec<_> = zones::zone_values(&was.zones).collect();
            for (param, value) in zones::zone_values(&chain.zones) {
                if !then.contains(&(param, value)) {
                    params.push(ParamEvent::now(NodeKey::ChainZones(chain.id), param, value));
                }
            }
        }
    }
}

/// A switch as a parameter value.
fn switch(on: bool) -> f64 {
    if on { 1.0 } else { 0.0 }
}

/// Parameter changes for a track's mixer.
fn mixer_events(track: TrackId, before: &Mixer, after: &Mixer, params: &mut Vec<ParamEvent>) {
    mixer_events_for(NodeKey::Mixer(track), before, after, params);
    let key = NodeKey::Mixer(track);
    if before.crossfade != after.crossfade {
        params.push(ParamEvent::now(
            key,
            mixer::track::CROSSFADE,
            mixer::crossfade_value(after.crossfade),
        ));
    }
}

/// Parameter changes for a mixer node's volume, pan and activator.
fn mixer_events_for(key: NodeKey, before: &Mixer, after: &Mixer, params: &mut Vec<ParamEvent>) {
    if before.volume != after.volume {
        params.push(ParamEvent::now(key, mixer::track::VOLUME, after.volume));
    }
    if before.pan != after.pan {
        params.push(ParamEvent::now(key, mixer::track::PAN, after.pan));
    }
    if before.mute != after.mute {
        params.push(ParamEvent::now(key, mixer::track::MUTE, switch(after.mute)));
    }
    // Spec: mixer R22, R29. The pan mode and each split side reach the mixer
    // as parameters, with no new program.
    if before.split_left != after.split_left {
        params.push(ParamEvent::now(
            key,
            mixer::track::SPLIT_LEFT,
            after.split_left,
        ));
    }
    if before.split_right != after.split_right {
        params.push(ParamEvent::now(
            key,
            mixer::track::SPLIT_RIGHT,
            after.split_right,
        ));
    }
    if before.pan_mode != after.pan_mode {
        params.push(ParamEvent::now(
            key,
            mixer::track::PAN_MODE,
            if after.pan_mode == bl_model::PanMode::Split {
                1.0
            } else {
                0.0
            },
        ));
    }
}

/// Parameter changes for a track's sends whose levels changed, to returns
/// that were already there.
fn send_events(
    old: &Document,
    new: &Document,
    before: &Track,
    after: &Track,
    params: &mut Vec<ParamEvent>,
) {
    if !after.kind().has_sends() {
        return;
    }
    for to in new.returns() {
        let level = after.send(to);
        if old.track(to).is_some() && before.send(to) != level {
            let key = NodeKey::Send {
                track: after.id(),
                to,
            };
            params.push(ParamEvent::now(key, route::LEVEL, level));
        }
    }
}

/// Parameter changes for the routes in both documents whose solo or
/// monitoring state changed, and for the clip players whose clips go
/// silent or come back with Monitor In: every other route and player is
/// new, and made with its state (or, migrating into a new program, told
/// it there).
fn route_events(changes: &ChangeSet, old: &Document, new: &Document, params: &mut Vec<ParamEvent>) {
    let switched = changes.tracks.iter().any(|(&id, &change)| {
        change == Change::Modified
            && match (old.track(id), new.track(id)) {
                (Some(before), Some(after)) => {
                    before.mixer().solo != after.mixer().solo
                        || before.mixer().arm != after.mixer().arm
                        || before.routing() != after.routing()
                }
                _ => true,
            }
    });
    let in_place = old.settings().solo_in_place != new.settings().solo_in_place;
    if !(changes.layout || switched || in_place) {
        return;
    }
    let (was, is) = (Graph::of(old), Graph::of(new));
    let (before, after) = (Solo::with(old, &was), Solo::with(new, &is));
    // In Cue mode the Solo switches are the Cue switches, which open and
    // close their routes to the Cue Out.
    if old.cue_active() && new.cue_active() {
        for track in new.tracks().filter(|t| t.kind() != TrackKind::Main) {
            let then = old.track(track.id()).map(|t| t.mixer().solo);
            if then.is_some_and(|solo| solo != track.mixer().solo) {
                params.push(ParamEvent::now(
                    NodeKey::Cue(track.id()),
                    route::SOLO_MUTE,
                    switch(!track.mixer().solo),
                ));
            }
        }
    }
    let state = |document: &Document, solo: &Solo, edge: &Edge| {
        let closed = edge.monitored
            && document
                .track(edge.to)
                .is_some_and(|to| !to.routing().monitoring(to.mixer().arm));
        let auto = edge.monitored
            && document
                .track(edge.to)
                .is_some_and(|to| to.routing().monitor == bl_model::Monitor::Auto);
        (solo.edge_muted(edge), closed, auto)
    };
    for edge in &is.edges {
        let key = route_key(edge);
        let Some(old_edge) = was.edges.iter().find(|e| route_key(e) == key) else {
            continue;
        };
        let (muted_then, closed_then, auto_then) = state(old, &before, old_edge);
        let (muted, closed, auto) = state(new, &after, edge);
        if muted_then != muted {
            params.push(ParamEvent::now(key, route::SOLO_MUTE, switch(muted)));
        }
        if closed_then != closed {
            params.push(ParamEvent::now(key, route::CLOSED, switch(closed)));
        }
        if auto_then != auto {
            params.push(ParamEvent::now(key, route::AUTO, switch(auto)));
        }
    }
    for track in new.tracks().filter(|t| t.kind() == TrackKind::Audio) {
        let Some(then) = old.track(track.id()) else {
            continue;
        };
        if clips_audible(then) != clips_audible(track) {
            params.push(ParamEvent::now(
                NodeKey::Player(track.id()),
                crate::clips::player::AUDIBLE,
                switch(clips_audible(track)),
            ));
        }
        // Ext. In hears the interface while the track monitors it; Monitor
        // or Arm alone doesn't recompile (WP-2.13). Under Monitor Auto it
        // also closes while the track's clips play (the routing spec, R11).
        if matches!(track.routing().input, bl_model::Input::External { .. }) {
            let closed = |t: &Track| !t.routing().monitoring(t.mixer().arm);
            if closed(then) != closed(track) {
                params.push(ParamEvent::now(
                    NodeKey::DeviceInput(track.id()),
                    route::CLOSED,
                    switch(closed(track)),
                ));
            }
            let auto = |t: &Track| t.routing().monitor == bl_model::Monitor::Auto;
            if auto(then) != auto(track) {
                params.push(ParamEvent::now(
                    NodeKey::DeviceInput(track.id()),
                    route::AUTO,
                    switch(auto(track)),
                ));
            }
        }
    }
}

/// Parameter changes for a device whose values changed.
fn param_events(
    key: NodeKey,
    before: &OrdMap<ParamId, f64>,
    after: &OrdMap<ParamId, f64>,
    params: &mut Vec<ParamEvent>,
) {
    for (&param, &value) in after {
        if before.get(&param) != Some(&value) {
            params.push(ParamEvent::now(key, param, value));
        }
    }
    for &param in before.keys() {
        if !after.contains_key(&param) {
            params.push(ParamEvent {
                value: ParamValue::Default,
                ..ParamEvent::now(key, param, 0.0)
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use bl_core::time::{Bpm, SampleRate, TempoMap};
    use bl_model::{Command, Device, Editor, SendTap, TrackKind};

    use super::*;
    use crate::gc::Collector;
    use crate::message::When;
    use crate::node::TransportInfo;
    use crate::nodes::{TEST_TONE, test_tone};

    struct Fixture {
        editor: Editor,
        compiler: Compiler,
        _collector: Collector,
        group: TrackId,
        inner: TrackId,
        outer: TrackId,
        ret: TrackId,
        tone: DeviceId,
    }

    fn track(editor: &mut Editor, kind: TrackKind, parent: Option<TrackId>) -> TrackId {
        let id = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind,
                name: String::new(),
                parent,
                index: 0,
            })
            .unwrap();
        id
    }

    fn tone(editor: &mut Editor, track: TrackId) -> DeviceId {
        let id = editor.new_id();
        let kind = DeviceKind::Native {
            device_type: TEST_TONE.to_owned(),
        };
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device: Device::new(id, "", kind),
            })
            .unwrap();
        id
    }

    impl Fixture {
        /// Main, a return, a group holding an inner track, and an outer track
        /// with a tone.
        fn new() -> Self {
            let mut editor = Editor::new();
            let ret = track(&mut editor, TrackKind::Return, None);
            let group = track(&mut editor, TrackKind::Group, None);
            let inner = track(&mut editor, TrackKind::Midi, Some(group));
            let outer = track(&mut editor, TrackKind::Midi, None);
            let tone = tone(&mut editor, outer);
            let collector = Collector::new();
            let config = EngineConfig::new(SampleRate::HZ_48000, 64);
            let compiler = Compiler::new(config, Registry::new(), collector.handle());
            Self {
                editor,
                compiler,
                _collector: collector,
                group,
                inner,
                outer,
                ret,
                tone,
            }
        }

        fn sync(&mut self) -> Update {
            self.compiler.sync(self.editor.document())
        }

        fn apply(&mut self, command: Command) -> Update {
            self.editor.apply(command).unwrap();
            self.sync()
        }
    }

    #[test]
    fn tracks_run_before_the_tracks_they_feed() {
        let mut f = Fixture::new();
        let update = f.sync();
        let program = update.program.as_ref().unwrap().get();
        let main = f.editor.document().main_id();
        // Top-level tracks in order (outer was created last, at index 0), the
        // group after its track, then the return, then Main.
        assert_eq!(
            program.track_order().collect::<Vec<_>>(),
            vec![f.outer, f.inner, f.group, f.ret, main]
        );
        assert_eq!(program.generation(), 1);
        assert_eq!(program.base(), None);
        // A mixer per track, two MIDI clip players, the tone, an output per
        // track but Main, and a send from each of the three tracks that send
        // to the return.
        assert_eq!(program.node_count(), 5 + 2 + 1 + 4 + 3);
        assert!(update.unhosted.is_empty());
        // The graph a worker pool would walk reaches every task.
        let ready = program.ready_order();
        assert_eq!(ready.len(), program.task_count());
        let position = |t: u32| ready.iter().position(|&r| r == t).unwrap();
        for (t, task) in program.tasks.iter().enumerate() {
            for input in &program.task_inputs[task.inputs.range()] {
                assert!(position(input.from) < position(t as u32));
            }
        }
        // No return takes its sends before the mixer, so no task keeps a
        // copy for them.
        assert!(program.tap_pool.is_empty());
    }

    // Spec: warp-modes R14. A file's analysis landing after its clips were
    // compiled sends the clips of the tracks that play it again, so a Beats
    // clip's plan cuts at the transients the library now gives; a file no
    // clip plays sends nothing.
    #[test]
    fn a_file_analysed_sends_again_only_the_clips_that_play_it() {
        let mut f = Fixture::new();
        let audio_track = track(&mut f.editor, TrackKind::Audio, None);
        let media = bl_model::ContentHash([7; 32]);
        let clip = bl_model::Clip::new(
            f.editor.new_id(),
            bl_core::time::Ticks::ZERO,
            bl_core::time::Ticks::WHOLE,
            bl_model::ClipData::Audio(bl_model::AudioData::new(media)),
        );
        f.apply(Command::CreateClip {
            track: audio_track,
            clip,
        });
        let update = f.compiler.media_analysed(media);
        assert!(update.program.is_none());
        assert!(!update.data.is_empty(), "the audio track's clips go again");
        let other = f.compiler.media_analysed(bl_model::ContentHash([8; 32]));
        assert!(other.data.is_empty());
    }

    // Spec: themes R27. A colour is never heard: colouring a track, a clip or a
    // scene sends the engine no program, no parameter and no clip data.
    #[test]
    fn a_colour_edit_sends_the_engine_nothing() {
        let mut f = Fixture::new();
        let audio_track = track(&mut f.editor, TrackKind::Audio, None);
        let clip = bl_model::Clip::new(
            f.editor.new_id(),
            bl_core::time::Ticks::ZERO,
            bl_core::time::Ticks::WHOLE,
            bl_model::ClipData::Audio(bl_model::AudioData::new(bl_model::ContentHash([7; 32]))),
        );
        let clip_id = clip.id;
        f.apply(Command::CreateClip {
            track: audio_track,
            clip,
        });
        let scene = f.editor.document().scenes().next().unwrap().id;
        for command in [
            Command::SetTrackColour {
                tracks: vec![audio_track, f.group],
                colour: 40,
            },
            Command::SetClipColour {
                clips: vec![clip_id],
                colour: 41,
            },
            Command::SetSceneColour {
                scenes: vec![scene],
                colour: Some(42),
            },
            Command::AssignColourToClips {
                tracks: vec![audio_track],
            },
            Command::AssignColourToGrouped {
                tracks: vec![f.group],
            },
        ] {
            let update = f.apply(command.clone());
            assert!(update.is_empty(), "{command:?}");
        }
    }

    #[test]
    fn mixer_and_parameter_edits_need_no_program() {
        let mut f = Fixture::new();
        f.sync();
        let update = f.apply(Command::SetVolume {
            track: f.outer,
            volume: 0.5,
        });
        assert!(update.program.is_none());
        assert_eq!(
            update.params,
            vec![ParamEvent::now(
                NodeKey::Mixer(f.outer),
                mixer::track::VOLUME,
                0.5
            )]
        );
        let update = f.apply(Command::SetMute {
            track: f.outer,
            mute: true,
        });
        assert_eq!(
            update.params,
            vec![ParamEvent::now(
                NodeKey::Mixer(f.outer),
                mixer::track::MUTE,
                1.0
            )]
        );
        let update = f.apply(Command::SetSend {
            track: f.inner,
            to: f.ret,
            level: 0.25,
        });
        assert!(update.program.is_none());
        assert_eq!(
            update.params,
            vec![ParamEvent::now(
                NodeKey::Send {
                    track: f.inner,
                    to: f.ret
                },
                route::LEVEL,
                0.25
            )]
        );
        let update = f.apply(Command::SetDeviceParam {
            device: f.tone,
            param: test_tone::LEVEL,
            value: -70.0,
        });
        assert!(update.program.is_none());
        assert_eq!(
            update.params,
            vec![ParamEvent::now(
                NodeKey::Device(f.tone),
                test_tone::LEVEL,
                -70.0
            )]
        );
        // Undoing the first value of a parameter sets it back to its default.
        let update = {
            f.editor.undo().unwrap();
            f.sync()
        };
        assert_eq!(update.params.len(), 1);
        assert_eq!(update.params[0].value, ParamValue::Default);
        assert_eq!(update.params[0].at, When::Now);
        // A name changes nothing in the engine.
        let update = f.apply(Command::RenameTrack {
            track: f.outer,
            name: "Tone".to_owned(),
        });
        assert!(update.is_empty());
    }

    // Spec: mixer R22
    #[test]
    fn solo_edits_close_and_open_routes_without_a_program() {
        let mut f = Fixture::new();
        f.sync();
        let solo_mute = |key, on: bool| ParamEvent::now(key, route::SOLO_MUTE, switch(on));
        let output = NodeKey::Output;
        let send = |track, to| NodeKey::Send { track, to };
        // The order of changes on different routes doesn't matter.
        let sorted = |mut events: Vec<ParamEvent>| {
            events.sort_by_key(|e| format!("{e:?}"));
            events
        };

        // Soloing the inner track silences the outer track and its send; the
        // group holds the solo, and Solo in Place keeps the return.
        let update = f.apply(Command::SetSolo {
            track: f.inner,
            solo: true,
        });
        assert!(update.program.is_none());
        assert_eq!(
            sorted(update.params),
            sorted(vec![
                solo_mute(output(f.outer), true),
                solo_mute(send(f.outer, f.ret), true),
            ])
        );
        // Without Solo in Place the return goes quiet too.
        let update = f.apply(Command::SetSoloInPlace { enabled: false });
        assert!(update.program.is_none());
        assert_eq!(update.params, vec![solo_mute(output(f.ret), true)]);
        // Soloing the return instead: the inner track's output closes, the
        // outer track's send to the return opens, and the return is heard.
        let update = f.apply(Command::SoloExclusive { track: f.ret });
        assert_eq!(
            sorted(update.params),
            sorted(vec![
                solo_mute(send(f.outer, f.ret), false),
                solo_mute(output(f.group), true),
                solo_mute(output(f.inner), true),
                solo_mute(output(f.ret), false),
            ])
        );
        // Nothing soloed: every route is open again.
        let update = f.apply(Command::SetSolo {
            track: f.ret,
            solo: false,
        });
        assert_eq!(
            sorted(update.params),
            sorted(vec![
                solo_mute(output(f.outer), false),
                solo_mute(output(f.group), false),
                solo_mute(output(f.inner), false),
            ])
        );
        assert!(
            f.apply(Command::SetSoloInPlace { enabled: true })
                .is_empty()
        );
    }

    #[test]
    fn the_pre_post_switch_recompiles_and_keeps_every_node() {
        let mut f = Fixture::new();
        f.sync();
        let update = f.apply(Command::SetSendTap {
            track: f.ret,
            tap: SendTap::Pre,
        });
        let program = update.program.as_ref().unwrap().get();
        assert_eq!(program.migration_count(), program.node_count());
        // The three tracks that send keep their chains' output.
        let size = 2 * 64;
        assert_eq!(program.tap_pool.len(), 3 * size);
        let ret = program.tasks.iter().find(|t| t.track == f.ret).unwrap();
        assert!(
            program.task_inputs[ret.inputs.range()]
                .iter()
                .all(|input| input.tap == Tap::PostFx)
        );
    }

    #[test]
    fn structural_edits_recompile_and_keep_surviving_nodes() {
        let mut f = Fixture::new();
        f.sync();
        // Moving the tone to another track keeps the node.
        let update = f.apply(Command::MoveDevice {
            device: f.tone,
            track: f.inner,
            index: 0,
        });
        let program = update.program.as_ref().unwrap().get();
        assert_eq!(program.generation(), 2);
        assert_eq!(program.base(), Some(1));
        assert_eq!(program.migration_count(), program.node_count());

        // Switching it off keeps it too.
        let update = f.apply(Command::SetDeviceEnabled {
            device: f.tone,
            enabled: false,
        });
        let program = update.program.as_ref().unwrap().get();
        assert_eq!(program.migration_count(), program.node_count());
        let slot = program.slot_of(NodeKey::Device(f.tone)).unwrap();
        assert!(program.slots[slot as usize].bypass);
        let before = program.node_count();

        // A new audio track brings a clip player, a mixer, an output, a
        // send to the return, and the interface input its Audio From (Ext.
        // In) opens (WP-2.13); the rest migrate. With no clips, the player
        // needs no data.
        let new = track(&mut f.editor, TrackKind::Audio, None);
        let update = f.sync();
        let program = update.program.as_ref().unwrap().get();
        assert_eq!(program.node_count(), before + 5);
        assert_eq!(program.migration_count(), before);
        assert!(update.data.is_empty());
        for key in [
            NodeKey::Player(new),
            NodeKey::Mixer(new),
            NodeKey::Output(new),
            NodeKey::Send {
                track: new,
                to: f.ret,
            },
            NodeKey::DeviceInput(new),
        ] {
            assert!(program.slot_of(key).is_some(), "{key}");
        }

        // A tempo change recompiles, and the program carries the new map.
        let tempo = TempoMap::constant(Bpm::new(90.0).unwrap());
        let update = f.apply(Command::SetTempo {
            tempo: tempo.clone(),
        });
        assert_eq!(update.program.as_ref().unwrap().get().tempo(), &tempo);
    }

    #[test]
    fn unknown_devices_pass_audio_through() {
        let mut f = Fixture::new();
        let id = f.editor.new_id();
        let kind = DeviceKind::Native {
            device_type: "beatlayer.not_yet".to_owned(),
        };
        let update = f.apply(Command::InsertDevice {
            track: f.outer,
            index: 0,
            device: Device::new(id, "", kind),
        });
        assert_eq!(update.unhosted, vec![id]);
        let program = update.program.as_ref().unwrap().get();
        assert!(program.slot_of(NodeKey::Device(id)).is_none());
    }

    #[test]
    fn any_order_the_task_graph_allows_gives_the_same_audio() {
        let mut f = Fixture::new();
        for track in [f.inner, f.ret] {
            tone(&mut f.editor, track);
        }
        f.editor
            .apply(Command::SetSend {
                track: f.outer,
                to: f.ret,
                level: 0.5,
            })
            .unwrap();
        let render = |f: &mut Fixture, ready: bool| {
            f.compiler.reset();
            let mut update = f.sync();
            let program = update.program.as_mut().unwrap().get_mut();
            if ready {
                program.order = program.ready_order();
                assert_ne!(program.order, (0..5).collect::<Vec<_>>());
            }
            let mut lane = crate::sched::Lane::new(f.compiler.config());
            let block = crate::program::Blk {
                frames: 64,
                from: 0,
                until: 64,
                events: &[],
                transport: TransportInfo::default(),
                session: &[],
                groove_amount: 1.0,
                set_scale: bl_model::Scale::default(),
                scale: crate::scales::ScaleRuns::of(bl_model::Scale::default()),
            };
            program.run(None, &mut lane, block);
            program.output(0, 64).to_vec()
        };
        let in_order = render(&mut f, false);
        assert!(in_order.iter().any(|&s| s != 0.0));
        assert_eq!(in_order, render(&mut f, true));
    }

    #[test]
    fn after_a_reset_nothing_migrates() {
        let mut f = Fixture::new();
        f.sync();
        f.compiler.reset();
        let update = f.sync();
        let program = update.program.as_ref().unwrap().get();
        assert_eq!(program.base(), None);
        assert_eq!(program.migration_count(), 0);
    }

    /// The clips in a data message for a clip player.
    fn clips_in(update: &Update, track: TrackId) -> Option<usize> {
        update
            .data
            .iter()
            .find(|(key, _)| *key == NodeKey::Player(track))
            .map(|(_, data)| data.downcast_ref::<ClipList>().unwrap().len())
    }

    /// A factory that says its device is an ARA plug-in, and makes no node.
    struct AraDevices;

    impl crate::NodeFactory for AraDevices {
        fn create(
            &self,
            _device: &bl_model::Device,
            _config: &crate::NodeConfig,
        ) -> Option<Box<dyn crate::Node>> {
            None
        }

        fn ara_support(&self, device: &bl_model::Device) -> Option<bl_model::AraSupport> {
            matches!(&device.kind, bl_model::DeviceKind::Native { device_type } if device_type == "test.ara")
                .then(bl_model::AraSupport::default)
        }
    }

    // Spec: ara-hosting R4
    #[test]
    fn an_ara_plugin_on_a_track_takes_the_clips_it_renders_from_the_player() {
        use bl_core::time::Ticks;
        use bl_model::{AudioData, Clip, ClipData, ContentHash, Device, DeviceKind};

        let mut f = Fixture::new();
        let audio = track(&mut f.editor, TrackKind::Audio, None);
        let mut media = crate::MediaMap::new();
        let file = bl_media::SampleData::new(48_000, vec![vec![0.5; 480]]).unwrap();
        media.insert(ContentHash([1; 32]), std::sync::Arc::new(file));
        f.compiler.registry.set_media(std::sync::Arc::new(media));
        f.compiler.registry.add(AraDevices);
        // One clip as it is, and one at half gain, which the plug-in can't
        // do.
        let plain = f.editor.new_id();
        let quiet = f.editor.new_id();
        let mut update = f.sync();
        for (id, gain) in [(plain, 1.0), (quiet, 0.5)] {
            let mut data = AudioData::new(ContentHash([1; 32]));
            data.gain = gain;
            update = f.apply(Command::CreateClip {
                track: audio,
                clip: Clip::new(id, Ticks::ZERO, Ticks::WHOLE, ClipData::Audio(data)),
            });
        }
        assert_eq!(clips_in(&update, audio), Some(2));

        // The plug-in arrives: the player keeps only the clip it can't play.
        let device = f.editor.new_id();
        let update = f.apply(Command::InsertDevice {
            track: audio,
            index: 0,
            device: Device::new(
                device,
                "ARA",
                DeviceKind::Native {
                    device_type: "test.ara".to_owned(),
                },
            ),
        });
        assert_eq!(clips_in(&update, audio), Some(1));

        // Switched off, it renders nothing, and the player has both again.
        let update = f.apply(Command::SetDeviceEnabled {
            device,
            enabled: false,
        });
        assert_eq!(clips_in(&update, audio), Some(2));
        let update = f.apply(Command::SetDeviceEnabled {
            device,
            enabled: true,
        });
        assert_eq!(clips_in(&update, audio), Some(1));

        // A muted clip is nobody's, and a track that isn't an audio track
        // has no plug-in clips to take.
        f.apply(Command::SetClipMute {
            clip: plain,
            mute: true,
        });
        let update = f.apply(Command::SetClipMute {
            clip: plain,
            mute: false,
        });
        assert_eq!(clips_in(&update, audio), Some(1));
    }

    // Spec: audio-clips R12
    #[test]
    fn clip_edits_send_the_tracks_clips_and_never_a_program() {
        use bl_core::time::Ticks;
        use bl_model::{AudioData, Clip, ClipData, ContentHash};

        let mut f = Fixture::new();
        let audio = track(&mut f.editor, TrackKind::Audio, None);
        let other = track(&mut f.editor, TrackKind::Audio, None);
        let mut media = crate::MediaMap::new();
        let file = bl_media::SampleData::new(48_000, vec![vec![0.5; 480]]).unwrap();
        media.insert(ContentHash([1; 32]), std::sync::Arc::new(file));
        f.compiler.registry.set_media(std::sync::Arc::new(media));
        // No clips yet: the new players need no data.
        let update = f.sync();
        assert!(update.program.is_some());
        assert!(update.data.is_empty());

        let new_clip = |f: &mut Fixture, media: u8| {
            let id = f.editor.new_id();
            let data = ClipData::Audio(AudioData::new(ContentHash([media; 32])));
            f.apply(Command::CreateClip {
                track: audio,
                clip: Clip::new(id, Ticks::ZERO, Ticks::WHOLE, data),
            })
        };
        let update = new_clip(&mut f, 1);
        assert!(update.program.is_none());
        // The clip edit sends the track's player its clips, and its nodes
        // their envelopes again (WP-3.15: a clip's envelopes modulate those,
        // so an empty list clears what the track had). Only the changed
        // track's player is sent.
        assert_eq!(
            update
                .data
                .iter()
                .filter(|(key, _)| matches!(key, NodeKey::Player(id) if *id == audio))
                .count(),
            1
        );
        assert_eq!(clips_in(&update, audio), Some(1));
        assert!(update.missing.is_empty());
        // A clip whose media the library doesn't have is reported, and its
        // track's list leaves it out.
        let update = new_clip(&mut f, 2);
        assert_eq!(clips_in(&update, audio), Some(1));
        assert_eq!(update.missing, vec![ContentHash([2; 32])]);

        // Moving a clip to another track sends both tracks' lists.
        let id = f
            .editor
            .document()
            .track(audio)
            .unwrap()
            .clips()
            .next()
            .unwrap()
            .id;
        let update = f.apply(Command::MoveClip {
            clip: id,
            track: other,
            start: Ticks::QUARTER,
        });
        assert!(update.program.is_none());
        assert_eq!(clips_in(&update, audio), Some(0));
        assert_eq!(clips_in(&update, other), Some(1));
        // A deactivated clip leaves the list.
        let update = f.apply(Command::SetClipMute {
            clip: id,
            mute: true,
        });
        assert_eq!(clips_in(&update, other), Some(0));
        f.apply(Command::SetClipMute {
            clip: id,
            mute: false,
        });

        // A recompile sends lists only where they changed, unless the tempo
        // map moved every clip.
        let update = f.apply(Command::SetDeviceEnabled {
            device: f.tone,
            enabled: false,
        });
        assert!(update.program.is_some());
        assert!(update.data.is_empty());
        let update = f.apply(Command::SetTempo {
            tempo: TempoMap::constant(Bpm::new(90.0).unwrap()),
        });
        assert!(update.program.is_some());
        assert_eq!(clips_in(&update, audio), Some(0));
        assert_eq!(update.missing, vec![ContentHash([2; 32])]);
        assert_eq!(clips_in(&update, other), Some(1));
        // 2 audio track players and 2 MIDI track players.
        assert_eq!(update.data.len(), 4);
    }

    // Spec: warping R17. A warp edit, and the warp mode's settings, send the
    // track's player its clips with their tables, and never a program.
    #[test]
    fn warp_edits_send_node_data_and_never_a_program() {
        use bl_core::time::Ticks;
        use bl_model::{AudioData, BeatsUpdate, Clip, ClipData, ContentHash, WarpMarker, WarpMode};

        let mut f = Fixture::new();
        let audio = track(&mut f.editor, TrackKind::Audio, None);
        let mut media = crate::MediaMap::new();
        let file = bl_media::SampleData::new(48_000, vec![vec![0.5; 96_000]]).unwrap();
        media.insert(ContentHash([1; 32]), std::sync::Arc::new(file));
        f.compiler.registry.set_media(std::sync::Arc::new(media));
        f.sync();
        let id = f.editor.new_id();
        let data = AudioData {
            warp: true,
            warp_markers: vec![WarpMarker {
                time: 0.0,
                beat: Ticks::ZERO,
            }],
            tempo: Some(100.0),
            ..AudioData::new(ContentHash([1; 32]))
        };
        let update = f.apply(Command::CreateClip {
            track: audio,
            clip: Clip::new(id, Ticks::ZERO, Ticks::WHOLE, ClipData::Audio(data)),
        });
        assert_eq!(clips_in(&update, audio), Some(1));
        for command in [
            Command::InsertWarpMarker {
                clips: vec![id],
                beat: Some(Ticks::QUARTER),
                time: None,
                pin_neighbours: false,
                transients: None,
            },
            Command::SetSegmentBpm {
                clip: id,
                marker: Some(0),
                bpm: 90.0,
            },
            Command::SetClipWarpMode {
                clip: id,
                mode: WarpMode::RePitch,
            },
            Command::SetClipWarpSettings {
                clip: id,
                beats: Some(BeatsUpdate {
                    transient_envelope: Some(50),
                    ..BeatsUpdate::default()
                }),
                tones: None,
                texture: None,
                complex_pro: None,
            },
            Command::SetClipHighQuality {
                clip: id,
                high_quality: false,
            },
            Command::SetClipRamMode {
                clip: id,
                ram: true,
            },
            Command::SetClipWarp {
                clip: id,
                warp: false,
            },
            Command::SetClipWarp {
                clip: id,
                warp: true,
            },
        ] {
            let update = f.apply(command.clone());
            assert!(update.program.is_none(), "{command:?} built a program");
            assert_eq!(clips_in(&update, audio), Some(1), "{command:?}");
        }
        // A tempo change recompiles, and sends the tables under the new map.
        let update = f.apply(Command::SetTempo {
            tempo: TempoMap::constant(Bpm::new(90.0).unwrap()),
        });
        assert_eq!(clips_in(&update, audio), Some(1));
    }

    // Spec: audio-clips R18, R19
    #[test]
    fn with_a_loader_a_clip_joins_its_list_once_its_source_is_open() {
        use std::sync::{Arc, mpsc};
        use std::time::Duration;

        use bl_core::time::Ticks;
        use bl_model::{AudioData, Clip, ClipData, ContentHash};

        let mut f = Fixture::new();
        let audio = track(&mut f.editor, TrackKind::Audio, None);
        let mut media = crate::MediaMap::new();
        let file = bl_media::SampleData::new(48_000, vec![vec![0.5; 480]]).unwrap();
        media.insert(ContentHash([1; 32]), Arc::new(file));
        f.compiler.registry.set_media(Arc::new(media));
        let (answers, loader) = mpsc::channel();
        f.compiler
            .load_off_thread(move |loaded| {
                let _ = answers.send(loaded);
            })
            .unwrap();
        f.sync();
        let new_clip = |f: &mut Fixture, media: u8| {
            let id = f.editor.new_id();
            let data = ClipData::Audio(AudioData::new(ContentHash([media; 32])));
            let update = f.apply(Command::CreateClip {
                track: audio,
                clip: Clip::new(id, Ticks::ZERO, Ticks::WHOLE, data),
            });
            (id, update)
        };
        let wait = || loader.recv_timeout(Duration::from_secs(5)).unwrap();

        // The compiler doesn't open the file: the clip waits for the loader,
        // and joins its track's list with the loader's answer.
        let (id, update) = new_clip(&mut f, 1);
        assert_eq!(clips_in(&update, audio), Some(0));
        let update = f.compiler.loaded(wait());
        assert_eq!(clips_in(&update, audio), Some(1));
        assert!(update.program.is_none());

        // An edit keeps the clip's source: nothing more is opened.
        let update = f.apply(Command::SetClipGain {
            clip: id,
            gain: 0.5,
        });
        assert_eq!(clips_in(&update, audio), Some(1));
        assert!(loader.recv_timeout(Duration::from_millis(50)).is_err());

        // Media that isn't there is reported when the loader says so.
        let (_, update) = new_clip(&mut f, 2);
        assert_eq!(clips_in(&update, audio), Some(1));
        assert!(update.missing.is_empty());
        let update = f.compiler.loaded(wait());
        assert!(update.data.is_empty());
        assert_eq!(update.missing, vec![ContentHash([2; 32])]);

        // An answer for a clip that's gone, or from before a reset, is let go.
        let (gone, _) = new_clip(&mut f, 1);
        f.apply(Command::DeleteClip { clip: gone });
        assert!(f.compiler.loaded(wait()).is_empty());
        let (_, _) = new_clip(&mut f, 1);
        f.compiler.reset();
        assert!(f.compiler.loaded(wait()).is_empty());
    }

    /// A library whose files play from memory but stream as far as the
    /// compiler can tell, and which notes the points it is asked to preload.
    struct Cueing {
        files: crate::MediaMap,
        cued: std::sync::Arc<std::sync::Mutex<Vec<(usize, u64)>>>,
    }

    struct Cues(std::sync::Arc<std::sync::Mutex<Vec<(usize, u64)>>>);

    impl crate::SourceCues for Cues {
        fn cue(&self, slot: usize, frame: u64) {
            self.0.lock().unwrap().push((slot, frame));
        }
    }

    impl crate::MediaLibrary for Cueing {
        fn open(
            &self,
            media: bl_model::ContentHash,
            rate: SampleRate,
            start: u64,
        ) -> Option<crate::OpenedSource> {
            let mut opened = crate::MediaLibrary::open(&self.files, media, rate, start)?;
            opened.cues = Some(std::sync::Arc::new(Cues(std::sync::Arc::clone(&self.cued))));
            Some(opened)
        }
    }

    // Spec: disk-streaming R2. A sync that moves the Arrangement loop and
    // also needs a program cues the sources the untouched players hold, as
    // one that needs none does.
    #[test]
    fn a_recompile_that_moves_the_loop_cues_the_new_start() {
        use std::sync::Arc;

        use bl_core::time::Ticks;
        use bl_model::{AudioData, Clip, ClipData, ContentHash};

        let mut f = Fixture::new();
        let audio = track(&mut f.editor, TrackKind::Audio, None);
        let mut files = crate::MediaMap::new();
        let file = bl_media::SampleData::new(48_000, vec![vec![0.5; 100_000]]).unwrap();
        files.insert(ContentHash([1; 32]), Arc::new(file));
        let cued = Arc::default();
        f.compiler.registry.set_media(Arc::new(Cueing {
            files,
            cued: Arc::clone(&cued),
        }));
        let id = f.editor.new_id();
        f.editor
            .apply(Command::CreateClip {
                track: audio,
                clip: Clip::new(
                    id,
                    Ticks::ZERO,
                    Ticks::WHOLE,
                    ClipData::Audio(AudioData::new(ContentHash([1; 32]))),
                ),
            })
            .unwrap();
        f.sync();
        let loop_cue = || cued.lock().unwrap().iter().rfind(|cue| cue.0 == 2).copied();
        assert_eq!(loop_cue(), None);

        // A loop from the second beat, needing no program.
        f.editor
            .apply(Command::SetLoop {
                start: Ticks::QUARTER,
                length: Ticks::QUARTER,
            })
            .unwrap();
        f.editor
            .apply(Command::SetLoopEnabled { enabled: true })
            .unwrap();
        assert!(f.sync().program.is_none());
        assert_eq!(loop_cue(), Some((2, 24_000)));

        // The loop moves to the third beat in a sync that adds a device.
        f.editor
            .apply(Command::SetLoop {
                start: Ticks::QUARTER * 2,
                length: Ticks::QUARTER,
            })
            .unwrap();
        tone(&mut f.editor, f.inner);
        assert!(f.sync().program.is_some());
        assert_eq!(loop_cue(), Some((2, 48_000)));
    }
}

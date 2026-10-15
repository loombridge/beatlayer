//! The compiler: turns a document into an engine program, and decides which
//! edits need a new program at all.

use std::collections::{BTreeMap, BTreeSet};

use bl_model::{
    Change, ChangeSet, ContentHash, DeviceId, DeviceKind, Document, Mixer, OrdMap, ParamId, Tap,
    Track, TrackId, TrackKind,
};

use crate::EngineConfig;
use crate::automation;
use crate::clips::{ClipList, ClipPlayer};
use crate::gc::{GcHandle, Program};
use crate::message::{LoopRegion, ParamEvent, TransportCommand};
use crate::midi_clips::{MidiClipList, MidiClipPlayer};
use crate::mixer::{self, Route, Solo, TrackMixer, route};
use crate::node::{NodeData, NodeKey, NoteBuffer, ParamValue};
use crate::program::{
    DeviceInput, DeviceSidechain, EngineProgram, Input, Instance, Migration, NO_DELAY, NodeSlot,
    Span, Task, delay_storage,
};
use crate::registry::Registry;
use crate::routing::{Edge, Graph, Via};
use crate::sidechain::{self, SidechainKey};
use crate::sources::{ClipSources, Loaded, Loader};

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

/// What a node is, beyond its key: a node migrates only if this matches.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Identity {
    /// A device. Its media is part of what it is (WP-2.16): a device whose
    /// sample changed is a new node, and one the set keeps gets its audio
    /// sent again.
    Device(DeviceKind, Option<ContentHash>),
    Player,
    MidiPlayer,
    Mixer,
    Route,
    Sidechain,
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
    synced: Option<Document>,
    layout: BTreeMap<NodeKey, Placed>,
    sources: ClipSources,
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
            layout: BTreeMap::new(),
            sources: ClipSources::default(),
        }
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
                            self.send_clips(document, track, &mut update);
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

    /// The last program's generation, or 0 before the first.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// What the audio thread needs to play `document`, given the documents
    /// before it.
    pub fn sync(&mut self, document: &Document) -> Update {
        let Some(old) = self.synced.take() else {
            return self.compile(document);
        };
        let changes = ChangeSet::between(&old, document);
        let mut params = Vec::new();
        let recompile = needs_program(&changes, &old, document, &mut params);
        route_events(&changes, &old, document, &mut params);
        // Tracks that held or hold a clip that changed.
        let tracks: BTreeSet<TrackId> = changes
            .clips
            .keys()
            .flat_map(|&clip| [old.clip_track(clip), document.clip_track(clip)])
            .flatten()
            .collect();
        let mut update = if recompile {
            let resend = Resend {
                all: old.settings().tempo != document.settings().tempo,
                tracks,
            };
            self.compile_with(document, &resend)
        } else {
            self.synced = Some(document.clone());
            let mut update = Update::default();
            if old.settings().arrangement_loop != document.settings().arrangement_loop {
                update
                    .transport
                    .push(TransportCommand::SetLoop(loop_region(document)));
                self.sources.cue_loop(self.loop_start_frame(document));
            }
            if old.settings().metronome != document.settings().metronome {
                update.transport.push(TransportCommand::SetMetronome(
                    document.settings().metronome,
                ));
            }
            for track in tracks {
                self.send_clips(document, track, &mut update);
            }
            update
        };
        // A new program's restatement of migrated nodes goes first.
        update.params.extend(params);
        // A track whose envelopes changed sends its nodes their lists again,
        // empty ones included, which clear the envelopes a node had. A new
        // tempo map moved every envelope, so every track's goes again.
        let all = old.settings().tempo != document.settings().tempo;
        for track in document.tracks() {
            let id = track.id();
            let differs = match (old.track(id), document.track(id)) {
                (Some(before), Some(after)) => before.automation_map() != after.automation_map(),
                // A new track's nodes are new, with no envelopes to clear,
                // and a deleted track's nodes are gone with the track.
                _ => false,
            };
            if all || differs {
                // The track whose envelopes changed gets every list, so an
                // empty one clears what a node had; a tempo map that moved
                // them only sends the ones that drive something.
                self.send_automation(document, id, !differs, &mut update);
            }
        }
        update
    }

    /// A whole new program for `document`. Nodes that the last program ran
    /// migrate into it, and every audio track's clip player is sent its
    /// clips.
    pub fn compile(&mut self, document: &Document) -> Update {
        let resend = Resend {
            all: true,
            tracks: BTreeSet::new(),
        };
        self.compile_with(document, &resend)
    }

    /// Sends `track`'s clip player its clips: a list placed under `document`'s
    /// tempo map. An empty list clears the player's.
    fn send_clips(&mut self, document: &Document, track: TrackId, update: &mut Update) {
        let Some(track) = document.track(track) else {
            return;
        };
        match track.kind() {
            TrackKind::Audio => {
                let loop_start = self.loop_start_frame(document);
                let built = ClipList::build(
                    track,
                    &document.settings().tempo,
                    self.config.sample_rate,
                    loop_start,
                    &mut self.sources,
                    self.registry.media(),
                );
                update.missing.extend(built.missing);
                update.missing.sort_unstable();
                update.missing.dedup();
                update
                    .data
                    .push((NodeKey::Player(track.id()), self.gc.data(built.list)));
            }
            TrackKind::Midi => {
                let list =
                    MidiClipList::build(track, &document.settings().tempo, self.config.sample_rate);
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
        let graph = Graph::of(document);
        let solo = Solo::with(document, &graph);
        let order = document.track_order();
        let task_of: BTreeMap<TrackId, u32> = order
            .iter()
            .enumerate()
            .map(|(i, &id)| (id, i as u32))
            .collect();
        let mut build = Build::default();

        // Each track's inputs, in the order the graph lists them.
        let mut inputs: BTreeMap<TrackId, Vec<Input>> = BTreeMap::new();
        let mut sidechain_inputs: BTreeMap<DeviceId, Vec<Input>> = BTreeMap::new();
        let mut taps: BTreeSet<(TrackId, Tap)> = BTreeSet::new();
        for edge in &graph.edges {
            let (Some(&from), Some(to)) = (task_of.get(&edge.from), document.track(edge.to)) else {
                continue;
            };
            let route = self.route(&mut build, document, &solo, edge, to);
            if edge.tap != Tap::PostMixer {
                taps.insert((edge.from, edge.tap));
            }
            let input = Input {
                route,
                from,
                tap: edge.tap,
                delay: NO_DELAY,
            };
            match edge.via.sidechain() {
                Some(device) => sidechain_inputs.entry(device).or_default().push(input),
                None => inputs.entry(edge.to).or_default().push(input),
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
                inputs.remove(&id).unwrap_or_default(),
                &mut sidechain_inputs,
                clips_audible(track),
            );
        }

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

        let generation = self.generation + 1;
        let base = (self.generation > 0).then_some(self.generation);
        let tasks = build.tasks.len();
        let midi = crate::midi::MidiRoutes::compile(document, &build.tasks);
        let size = self.config.channels * self.config.max_frames;
        let (order, dependents) = run_order(&build);
        compensate(&mut build, &order);
        // Automation is placed with each node's delay in it (WP-2.11), so
        // remember what every node's turned out to be.
        for slot in &build.slots {
            if let Some(placed) = build.layout.get_mut(&slot.key) {
                placed.delay = slot.delay;
            }
        }
        let (delay_lines, delay_pool) = delay_storage(&build.delays, self.config.channels);
        let program = EngineProgram {
            generation,
            base,
            index: build.layout.iter().map(|(&key, p)| (key, p.slot)).collect(),
            slots: build.slots,
            migrations: build.migrations,
            order,
            task_dependents: dependents.iter().flatten().copied().collect(),
            tasks: with_dependents(build.tasks, &dependents),
            task_inputs: build.inputs,
            task_nodes: build.nodes,
            sidechain_pool: vec![0.0; build.sidechains.len() * size],
            sidechains: build.sidechains,
            sidechain_inputs: build.sidechain_inputs,
            pool: vec![0.0; tasks * size],
            tap_pool: vec![0.0; kept as usize * size],
            delay_lines,
            delay_pool,
            delay_scratch: vec![0.0; size],
            device_input: vec![0.0; self.config.input_channels * self.config.max_frames],
            device_input_channels: self.config.input_channels,
            device_scratch: vec![0.0; size],
            channels: self.config.channels,
            max_frames: self.config.max_frames,
            output: task_of[&document.main_id()],
            tempo: document.settings().tempo.clone(),
            time_signatures: document.settings().time_signatures.clone(),
            sample_rate: self.config.sample_rate,
            note_buffers: [
                NoteBuffer::with_capacity(1024),
                NoteBuffer::with_capacity(1024),
            ],
            midi,
        };
        self.generation = generation;
        self.layout = build.layout;
        self.synced = Some(document.clone());
        let mut update = Update {
            program: Some(self.gc.program(program)),
            transport: vec![
                TransportCommand::SetLoop(loop_region(document)),
                TransportCommand::SetMetronome(document.settings().metronome),
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
        let rate = self.config.sample_rate;
        for &(device, media) in &build.media {
            match self
                .registry
                .media()
                .and_then(|library| library.sample(media, rate))
            {
                Some(sample) => update
                    .data
                    .push((NodeKey::Device(device), self.gc.data(sample))),
                None => update.missing.push(media),
            }
        }
        // Players that went hold no sources now, nor do new ones yet; open
        // sources of clips that went are closed.
        self.sources.retain(
            |track| document.track(track).is_some() && !build.new_players.contains(&track),
            |clip| document.clip_track(clip).is_some(),
        );
        // A new player gets its track's clips if it has any; a player that
        // migrated keeps its own unless they changed, or the tempo map
        // moved them.
        for track in document.tracks() {
            let id = track.id();
            let send = if build.new_players.contains(&id) {
                track.clips().next().is_some()
            } else {
                resend.all || resend.tracks.contains(&id)
            };
            if send {
                self.send_clips(document, id, &mut update);
            }
            // Every node is new in a new program, so each is sent the
            // automation that drives its parameters: a node with none is
            // new too, and has none anyway.
            self.send_automation(document, id, true, &mut update);
        }
        update
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
        for (key, list) in automation::lists(document, track.id(), rate, |key| {
            i64::from(self.layout.get(&key).map_or(0, |placed| placed.delay))
        }) {
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
    /// sent. Returns the [`TransportCommand::SetLoop`] a sync would have
    /// sent if `document` changed the Arrangement loop: the program
    /// compiled at the restart sets the loop too, but the processor that
    /// takes over carries the loop it played, so a loop edit made meanwhile
    /// must reach it after that.
    #[must_use]
    pub fn remember(&mut self, document: &Document) -> Option<TransportCommand> {
        let changed = self.synced.as_ref().is_none_or(|old| {
            old.settings().arrangement_loop != document.settings().arrangement_loop
        });
        self.synced = Some(document.clone());
        changed.then(|| TransportCommand::SetLoop(loop_region(document)))
    }

    /// Switches to `config` (a device's new sample rate or block size) and
    /// compiles the latest document anew for it, with every node made
    /// afresh at the new settings and none taken over: the program for a
    /// processor starting without one (WP-1.13). Empty if no document has
    /// been synced yet.
    pub fn reconfigure(&mut self, config: EngineConfig) -> Update {
        let document = self.synced.take();
        self.config = config;
        self.reset();
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
        self.layout.clear();
        self.sources.reset();
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
        if self.previous(key, &Identity::Route).is_some() {
            for (param, value) in [
                (route::LEVEL, level),
                (route::SOLO_MUTE, switch(muted)),
                (route::CLOSED, switch(closed)),
            ] {
                build.restate.push(ParamEvent::now(key, param, value));
            }
        }
        self.place(build, key, Identity::Route, 0, || {
            Instance::Route(Box::new(Route::new(level, muted, &config).closed(closed)))
        })
    }

    /// Adds the task for one track, fed by `inputs` and, into its devices'
    /// sidechains, by what `sidechain_inputs` holds for them; its clips
    /// heard or not.
    fn add_task(
        &self,
        build: &mut Build,
        track: &Track,
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
                if self.previous(key, &Identity::Route).is_some() {
                    for (param, value) in [
                        (route::LEVEL, 1.0),
                        (route::SOLO_MUTE, switch(false)),
                        (route::CLOSED, switch(closed)),
                    ] {
                        build.restate.push(ParamEvent::now(key, param, value));
                    }
                }
                let route = self.place(build, key, Identity::Route, 0, || {
                    Instance::Route(Box::new(Route::new(1.0, false, &config).closed(closed)))
                });
                Some(DeviceInput { route, channel })
            }
            _ => None,
        };
        // An audio or MIDI track's clip player runs first.
        let player = match track.kind() {
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
        let node_start = build.nodes.len() as u32;
        let sidechain_start = build.sidechains.len() as u32;
        for device in track.devices() {
            let key = NodeKey::Device(device.id);
            let identity = Identity::Device(device.kind.clone(), device.media);
            let (node, latency) = match self.previous(key, &identity) {
                Some(from) => (Err(from), self.previous_latency(key, &identity)),
                None => match self.registry.create(device, &config) {
                    Some(node) => {
                        // A new node that plays media gets its decoded audio
                        // with this compile's update (WP-2.16).
                        if let Some(media) = device.media {
                            build.media.push((device.id, media));
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
            let slot = claim(build, key, identity, !device.enabled, latency, node);
            build.nodes.push(slot);
            if let Some(settings) = &device.sidechain {
                let sources = sidechain_inputs.remove(&device.id).unwrap_or_default();
                let own = build.tasks.len() as u32;
                dependencies += sources.iter().filter(|s| s.from != own).count() as u32;
                self.add_sidechain(build, device.id, settings, slot, sources);
            }
        }
        let nodes = Span {
            start: node_start,
            end: build.nodes.len() as u32,
        };
        let sidechains = Span {
            start: sidechain_start,
            end: build.sidechains.len() as u32,
        };
        let mixer = self.place(
            build,
            NodeKey::Mixer(track.id()),
            Identity::Mixer,
            0,
            || Instance::Mixer(Box::new(TrackMixer::new(track.mixer(), &config))),
        );
        build.tasks.push(Task {
            track: track.id(),
            inputs,
            device_input,
            player,
            nodes,
            sidechains,
            mixer,
            clip_delay: NO_DELAY,
            pre_fx: None,
            post_fx: None,
            dependents: Span::default(),
            dependencies,
        });
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

/// A program under construction.
#[derive(Default)]
struct Build {
    slots: Vec<NodeSlot>,
    layout: BTreeMap<NodeKey, Placed>,
    migrations: Vec<Migration>,
    tasks: Vec<Task>,
    inputs: Vec<Input>,
    nodes: Vec<u32>,
    sidechains: Vec<DeviceSidechain>,
    sidechain_inputs: Vec<Input>,
    unhosted: Vec<DeviceId>,
    /// Devices new in this program that play media, with the media they
    /// name: their decoded audio goes with this compile's update (WP-2.16).
    media: Vec<(DeviceId, ContentHash)>,
    /// Tracks whose clip player is new in this program.
    new_players: BTreeSet<TrackId>,
    /// The state of routes and players that migrate in, which an edit may
    /// have changed: sent with the program.
    restate: Vec<ParamEvent>,
    /// The length in frames of each delay line the program needs, in the
    /// order the inputs and players name them (WP-2.11).
    delays: Vec<u32>,
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
fn run_order(build: &Build) -> (Vec<u32>, Vec<Vec<u32>>) {
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

/// Works out and records delay compensation (WP-2.11): how far each track's
/// clips and each route are held back so every path into a track lines up,
/// each sidechain's source lines up with its device's own input, and each
/// node's parameter changes are held back with the audio they act on.
fn compensate(build: &mut Build, order: &[u32]) {
    let tasks = build.tasks.len();
    // The frames each track's device chain adds.
    let mut chain = vec![0u32; tasks];
    for (t, task) in build.tasks.iter().enumerate() {
        for &slot in &build.nodes[task.nodes.range()] {
            chain[t] += build.slots[slot as usize].latency;
        }
    }
    // How far into its chain each device's own input is.
    for t in 0..tasks {
        let mut before = 0;
        for &slot in &build.nodes[build.tasks[t].nodes.range()] {
            for s in build.tasks[t].sidechains.range() {
                if build.sidechains[s].device == slot {
                    build.sidechains[s].before = before;
                }
            }
            before += build.slots[slot as usize].latency;
        }
    }
    // The frames from song time to each track's input sum: what its inputs
    // and its devices' sidechain sources make it wait for.
    let mut arrival = vec![0u32; tasks];
    for &t in order {
        let t = t as usize;
        let task = build.tasks[t];
        let mut arrived = 0u32;
        for input in &build.inputs[task.inputs.range()] {
            arrived = arrived.max(tap_latency(
                &arrival,
                &chain,
                input.from as usize,
                input.tap,
            ));
        }
        for s in task.sidechains.range() {
            let sc = build.sidechains[s];
            for input in &build.sidechain_inputs[sc.inputs.range()] {
                let need = tap_latency(&arrival, &chain, input.from as usize, input.tap)
                    .saturating_sub(sc.before);
                arrived = arrived.max(need);
            }
        }
        arrival[t] = arrived;
    }
    // Hold each path back to what the longest one takes, and line each
    // node's parameter changes up with the audio they act on.
    for t in 0..tasks {
        let task = build.tasks[t];
        let a = arrival[t];
        for i in task.inputs.range() {
            let input = build.inputs[i];
            let lat = tap_latency(&arrival, &chain, input.from as usize, input.tap);
            let delay = a.saturating_sub(lat);
            if delay > 0 {
                let index = line(build, delay);
                build.inputs[i].delay = index;
            }
        }
        for s in task.sidechains.range() {
            let sc = build.sidechains[s];
            for i in sc.inputs.range() {
                let input = build.sidechain_inputs[i];
                let lat = tap_latency(&arrival, &chain, input.from as usize, input.tap);
                let delay = (a + sc.before).saturating_sub(lat);
                if delay > 0 {
                    let index = line(build, delay);
                    build.sidechain_inputs[i].delay = index;
                }
            }
        }
        if build.tasks[t].player.is_some() && a > 0 {
            let index = line(build, a);
            build.tasks[t].clip_delay = index;
        }
        if let Some(slot) = task.player {
            build.slots[slot as usize].delay = a;
        }
        let mut before = 0;
        for &slot in &build.nodes[task.nodes.range()] {
            build.slots[slot as usize].delay = a + before;
            before += build.slots[slot as usize].latency;
        }
        build.slots[task.mixer as usize].delay = a + chain[t];
    }
}

/// A new delay line of `len` frames, returning its index in the program.
fn line(build: &mut Build, len: u32) -> u32 {
    let index = build.delays.len() as u32;
    build.delays.push(len);
    index
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

/// Whether a change set needs a new program; collects parameter changes for
/// the nodes that keep running either way.
fn needs_program(
    changes: &ChangeSet,
    old: &Document,
    new: &Document,
    params: &mut Vec<ParamEvent>,
) -> bool {
    // The tempo map travels in the program. The Arrangement loop doesn't: a
    // loop edit is a transport command (see `Compiler::sync`).
    let (before, after) = (old.settings(), new.settings());
    let settings = changes.settings
        && (before.tempo != after.tempo || before.time_signatures != after.time_signatures);
    let mut recompile = settings || changes.layout;
    for (&id, &change) in &changes.tracks {
        match (change, old.track(id), new.track(id)) {
            (Change::Modified, Some(before), Some(after)) => {
                let same_chain = before
                    .devices()
                    .map(|d| d.id)
                    .eq(after.devices().map(|d| d.id));
                // A return's Pre/Post switch moves where its sends tap; a
                // routing change moves routes. Monitor alone doesn't: it
                // opens and closes routes (see `route_events`).
                let (was, is) = (before.routing(), after.routing());
                if !same_chain
                    || !before.children().eq(after.children())
                    || before.mixer().tap != after.mixer().tap
                    || was.input != is.input
                    || was.channel != is.channel
                    || was.output != is.output
                    || was.midi_output != is.midi_output
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
            }
            _ => recompile = true,
        }
    }
    // Clip changes never recompile: `Compiler::sync` sends the track's clip
    // player its clips as node data (the audio clips spec, R12).
    recompile
}

/// A switch as a parameter value.
fn switch(on: bool) -> f64 {
    if on { 1.0 } else { 0.0 }
}

/// Parameter changes for a track's mixer.
fn mixer_events(track: TrackId, before: &Mixer, after: &Mixer, params: &mut Vec<ParamEvent>) {
    let key = NodeKey::Mixer(track);
    if before.volume != after.volume {
        params.push(ParamEvent::now(key, mixer::track::VOLUME, after.volume));
    }
    if before.pan != after.pan {
        params.push(ParamEvent::now(key, mixer::track::PAN, after.pan));
    }
    if before.mute != after.mute {
        params.push(ParamEvent::now(key, mixer::track::MUTE, switch(after.mute)));
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
    let state = |document: &Document, solo: &Solo, edge: &Edge| {
        let closed = edge.monitored
            && document
                .track(edge.to)
                .is_some_and(|to| !to.routing().monitoring(to.mixer().arm));
        (solo.edge_muted(edge), closed)
    };
    for edge in &is.edges {
        let key = route_key(edge);
        let Some(old_edge) = was.edges.iter().find(|e| route_key(e) == key) else {
            continue;
        };
        let (muted_then, closed_then) = state(old, &before, old_edge);
        let (muted, closed) = state(new, &after, edge);
        if muted_then != muted {
            params.push(ParamEvent::now(key, route::SOLO_MUTE, switch(muted)));
        }
        if closed_then != closed {
            params.push(ParamEvent::now(key, route::CLOSED, switch(closed)));
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
        // or Arm alone doesn't recompile (WP-2.13).
        if matches!(track.routing().input, bl_model::Input::External { .. }) {
            let closed = |t: &Track| !t.routing().monitoring(t.mixer().arm);
            if closed(then) != closed(track) {
                params.push(ParamEvent::now(
                    NodeKey::DeviceInput(track.id()),
                    route::CLOSED,
                    switch(closed(track)),
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
            let mut scratch = Vec::new();
            program.run(64, 0, &[], &mut scratch, TransportInfo::default());
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
        assert_eq!(update.data.len(), 1);
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
}

//! The compiler: turns a document into an engine program, and decides which
//! edits need a new program at all.

use std::collections::BTreeMap;

use bl_model::{
    Change, ChangeSet, DeviceId, DeviceKind, Document, Mixer, OrdMap, ParamId, SendTap, Track,
    TrackId,
};

use crate::EngineConfig;
use crate::gc::{GcHandle, Program};
use crate::message::{LoopRegion, ParamEvent, TransportCommand};
use crate::mixer::{self, Route, Solo, TrackMixer, route};
use crate::node::{NodeKey, ParamValue};
use crate::program::{EngineProgram, Input, Instance, Migration, NodeSlot, Span, Task};
use crate::registry::Registry;

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
    /// Devices in the new program that no factory makes a node for, so their
    /// tracks' audio passes through where they'd be.
    pub unhosted: Vec<DeviceId>,
}

impl Update {
    /// Whether the update holds nothing to send.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.program.is_none() && self.transport.is_empty() && self.params.is_empty()
    }
}

/// What a node is, beyond its key: a node migrates only if this matches.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Identity {
    Device(DeviceKind),
    Mixer,
    Route,
}

/// Where a node sits in the last program compiled.
#[derive(Clone, Debug)]
struct Placed {
    slot: u32,
    identity: Identity,
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
/// and clip changes will travel as node data (WP-1.4), so none of them
/// recompiles.
///
/// Each program lists the nodes it takes over from the program compiled
/// before it, so programs must reach the audio thread in the order they
/// were compiled, none skipped.
pub struct Compiler {
    config: EngineConfig,
    registry: Registry,
    gc: GcHandle,
    generation: u64,
    synced: Option<Document>,
    layout: BTreeMap<NodeKey, Placed>,
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
        }
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
        solo_events(&changes, &old, document, &mut params);
        let mut update = if recompile {
            self.compile(document)
        } else {
            self.synced = Some(document.clone());
            let mut update = Update::default();
            if old.settings().arrangement_loop != document.settings().arrangement_loop {
                update
                    .transport
                    .push(TransportCommand::SetLoop(loop_region(document)));
            }
            update
        };
        update.params = params;
        update
    }

    /// A whole new program for `document`. Nodes that the last program ran
    /// migrate into it.
    pub fn compile(&mut self, document: &Document) -> Update {
        let solo = Solo::of(document);
        let mut build = Build::default();
        let mut main_inputs = Vec::new();
        for id in document.top_level() {
            let task = self.add_track(&mut build, document, &solo, id);
            main_inputs.push(self.output(&mut build, &solo, id, task));
        }

        // Every audio, MIDI, and group track sends to every return, in the
        // order the tracks show.
        let senders: Vec<(TrackId, u32)> = document
            .tracks()
            .filter(|track| track.kind().has_sends())
            .filter_map(|track| Some((track.id(), *build.task_of.get(&track.id())?)))
            .collect();
        let mut pre = false;
        for id in document.returns() {
            let track = document
                .track(id)
                .expect("a document's tree names only its own tracks");
            let tap = track.mixer().tap;
            pre |= tap == SendTap::Pre;
            let inputs = senders
                .iter()
                .map(|&(sender, from)| Input {
                    route: self.send(&mut build, document, &solo, sender, id),
                    from,
                    pre: tap == SendTap::Pre,
                })
                .collect();
            let task = self.add_task(&mut build, track, inputs);
            main_inputs.push(self.output(&mut build, &solo, id, task));
        }
        let output = self.add_task(&mut build, document.main(), main_inputs);

        // Tracks that send keep their chain's output for pre-fader sends.
        let mut kept = 0;
        if pre {
            for &(_, task) in &senders {
                build.tasks[task as usize].pre = Some(kept);
                kept += 1;
            }
        }

        let generation = self.generation + 1;
        let base = (self.generation > 0).then_some(self.generation);
        let tasks = build.tasks.len();
        let size = self.config.channels * self.config.max_frames;
        let program = EngineProgram {
            generation,
            base,
            index: build.layout.iter().map(|(&key, p)| (key, p.slot)).collect(),
            slots: build.slots,
            migrations: build.migrations,
            order: (0..tasks as u32).collect(),
            task_dependents: build.dependents.iter().flatten().copied().collect(),
            tasks: with_dependents(build.tasks, &build.dependents),
            task_inputs: build.inputs,
            task_nodes: build.nodes,
            pool: vec![0.0; tasks * size],
            pre_pool: vec![0.0; kept as usize * size],
            channels: self.config.channels,
            max_frames: self.config.max_frames,
            output,
            tempo: document.settings().tempo.clone(),
            sample_rate: self.config.sample_rate,
        };
        self.generation = generation;
        self.layout = build.layout;
        self.synced = Some(document.clone());
        Update {
            program: Some(self.gc.program(program)),
            transport: vec![TransportCommand::SetLoop(loop_region(document))],
            params: Vec::new(),
            unhosted: build.unhosted,
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
    }

    /// Adds a track and, first, the tracks that feed it. Returns its task.
    fn add_track(&self, build: &mut Build, document: &Document, solo: &Solo, id: TrackId) -> u32 {
        let track = document
            .track(id)
            .expect("a document's tree names only its own tracks");
        let mut inputs = Vec::with_capacity(track.children().len());
        for child in track.children() {
            let task = self.add_track(build, document, solo, child);
            inputs.push(self.output(build, solo, child, task));
        }
        self.add_task(build, track, inputs)
    }

    /// The input that carries `track`'s output (its task, `task`) into its
    /// group or Main.
    fn output(&self, build: &mut Build, solo: &Solo, track: TrackId, task: u32) -> Input {
        let config = self.config.node_config();
        let route = self.place(build, NodeKey::Output(track), Identity::Route, || {
            Instance::Route(Box::new(Route::new(1.0, solo.output_muted(track), &config)))
        });
        Input {
            route,
            from: task,
            pre: false,
        }
    }

    /// The slot of `track`'s send to the return `to`.
    fn send(
        &self,
        build: &mut Build,
        document: &Document,
        solo: &Solo,
        track: TrackId,
        to: TrackId,
    ) -> u32 {
        let config = self.config.node_config();
        let level = document.track(track).map_or(0.0, |t| t.send(to));
        self.place(build, NodeKey::Send { track, to }, Identity::Route, || {
            Instance::Route(Box::new(Route::new(
                level,
                solo.send_muted(track, to),
                &config,
            )))
        })
    }

    /// Adds the task for one track, fed by `inputs`.
    fn add_task(&self, build: &mut Build, track: &Track, inputs: Vec<Input>) -> u32 {
        let task = build.tasks.len() as u32;
        for input in &inputs {
            build.dependents[input.from as usize].push(task);
        }
        let dependencies = inputs.len() as u32;
        let input_start = build.inputs.len() as u32;
        build.inputs.extend(inputs);
        let inputs = Span {
            start: input_start,
            end: build.inputs.len() as u32,
        };
        let config = self.config.node_config();
        let node_start = build.nodes.len() as u32;
        for device in track.devices() {
            let key = NodeKey::Device(device.id);
            let identity = Identity::Device(device.kind.clone());
            let node = match self.previous(key, &identity) {
                Some(from) => Err(from),
                None => match self.registry.create(device, &config) {
                    Some(node) => Ok(Instance::Device(node)),
                    None => {
                        build.unhosted.push(device.id);
                        continue;
                    }
                },
            };
            let slot = claim(build, key, identity, !device.enabled, node);
            build.nodes.push(slot);
        }
        let nodes = Span {
            start: node_start,
            end: build.nodes.len() as u32,
        };
        let mixer = self.place(build, NodeKey::Mixer(track.id()), Identity::Mixer, || {
            Instance::Mixer(Box::new(TrackMixer::new(track.mixer(), &config)))
        });
        build.tasks.push(Task {
            track: track.id(),
            inputs,
            nodes,
            mixer,
            pre: None,
            dependents: Span::default(),
            dependencies,
        });
        build.dependents.push(Vec::new());
        build.task_of.insert(track.id(), task);
        task
    }

    /// Where `key`'s node sat in the last program, if it was the same kind
    /// of node, so it migrates.
    fn previous(&self, key: NodeKey, identity: &Identity) -> Option<u32> {
        self.layout
            .get(&key)
            .filter(|placed| placed.identity == *identity)
            .map(|placed| placed.slot)
    }

    /// Gives a mixer or a route a slot: the one that ran under `key` in the
    /// last program, or a new one from `make`.
    fn place(
        &self,
        build: &mut Build,
        key: NodeKey,
        identity: Identity,
        make: impl FnOnce() -> Instance,
    ) -> u32 {
        let node = match self.previous(key, &identity) {
            Some(from) => Err(from),
            None => Ok(make()),
        };
        claim(build, key, identity, false, node)
    }
}

/// Puts a node in the next slot: a new node, or (`Err`) the slot of the node
/// in the last program that migrates into it.
fn claim(
    build: &mut Build,
    key: NodeKey,
    identity: Identity,
    bypass: bool,
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
    build.slots.push(NodeSlot { key, node, bypass });
    build.layout.insert(key, Placed { slot, identity });
    slot
}

impl std::fmt::Debug for Compiler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Compiler")
            .field("generation", &self.generation)
            .field("nodes", &self.layout.len())
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
    task_of: BTreeMap<TrackId, u32>,
    inputs: Vec<Input>,
    nodes: Vec<u32>,
    dependents: Vec<Vec<u32>>,
    unhosted: Vec<DeviceId>,
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
                // A return's Pre/Post switch moves where its sends tap.
                if !same_chain
                    || !before.children().eq(after.children())
                    || before.mixer().tap != after.mixer().tap
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
                if before.kind != after.kind || before.enabled != after.enabled {
                    recompile = true;
                }
                param_events(NodeKey::Device(id), &before.params, &after.params, params);
            }
            _ => recompile = true,
        }
    }
    // Clip changes never recompile: their data goes to the track's clip
    // player as its own message once clips play (WP-1.4).
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

/// Parameter changes for the outputs and sends whose solo state changed,
/// among the tracks in both documents: every other route is new, and made
/// with its state.
fn solo_events(changes: &ChangeSet, old: &Document, new: &Document, params: &mut Vec<ParamEvent>) {
    let solo_switched = changes.tracks.iter().any(|(&id, &change)| {
        change == Change::Modified
            && old.track(id).map(|t| t.mixer().solo) != new.track(id).map(|t| t.mixer().solo)
    });
    let in_place = old.settings().solo_in_place != new.settings().solo_in_place;
    if !(changes.layout || solo_switched || in_place) {
        return;
    }
    let (before, after) = (Solo::of(old), Solo::of(new));
    let kept = |id: TrackId| old.track(id).is_some();
    for track in new.tracks() {
        let id = track.id();
        if id == new.main_id() || !kept(id) {
            continue;
        }
        let muted = after.output_muted(id);
        if before.output_muted(id) != muted {
            params.push(ParamEvent::now(
                NodeKey::Output(id),
                route::SOLO_MUTE,
                switch(muted),
            ));
        }
        if !track.kind().has_sends() {
            continue;
        }
        for to in new.returns().filter(|&to| kept(to)) {
            let muted = after.send_muted(id, to);
            if before.send_muted(id, to) != muted {
                params.push(ParamEvent::now(
                    NodeKey::Send { track: id, to },
                    route::SOLO_MUTE,
                    switch(muted),
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
    use bl_model::{Command, Device, Editor, TrackKind};

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
        // A mixer per track, the tone, an output per track but Main, and a
        // send from each of the three tracks that send to the return.
        assert_eq!(program.node_count(), 5 + 1 + 4 + 3);
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
        assert!(program.pre_pool.is_empty());
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

        // Soloing the inner track silences the outer track and its send; the
        // group holds the solo, and Solo in Place keeps the return.
        let update = f.apply(Command::SetSolo {
            track: f.inner,
            solo: true,
        });
        assert!(update.program.is_none());
        assert_eq!(
            update.params,
            vec![
                solo_mute(output(f.outer), true),
                solo_mute(send(f.outer, f.ret), true),
            ]
        );
        // Without Solo in Place the return goes quiet too.
        let update = f.apply(Command::SetSoloInPlace { enabled: false });
        assert!(update.program.is_none());
        assert_eq!(update.params, vec![solo_mute(output(f.ret), true)]);
        // Soloing the return instead: the inner track's output closes, the
        // outer track's send to the return opens, and the return is heard.
        let update = f.apply(Command::SoloExclusive { track: f.ret });
        assert_eq!(
            update.params,
            vec![
                solo_mute(send(f.outer, f.ret), false),
                solo_mute(output(f.group), true),
                solo_mute(output(f.inner), true),
                solo_mute(output(f.ret), false),
            ]
        );
        // Nothing soloed: every route is open again.
        let update = f.apply(Command::SetSolo {
            track: f.ret,
            solo: false,
        });
        assert_eq!(
            update.params,
            vec![
                solo_mute(output(f.outer), false),
                solo_mute(output(f.group), false),
                solo_mute(output(f.inner), false),
            ]
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
        assert_eq!(program.pre_pool.len(), 3 * size);
        let ret = program.tasks.iter().find(|t| t.track == f.ret).unwrap();
        assert!(
            program.task_inputs[ret.inputs.range()]
                .iter()
                .all(|input| input.pre)
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

        // A new track brings a mixer, an output, and a send to the return;
        // the rest migrate.
        let new = track(&mut f.editor, TrackKind::Audio, None);
        let update = f.sync();
        let program = update.program.as_ref().unwrap().get();
        assert_eq!(program.node_count(), before + 3);
        assert_eq!(program.migration_count(), before);
        for key in [
            NodeKey::Mixer(new),
            NodeKey::Output(new),
            NodeKey::Send {
                track: new,
                to: f.ret,
            },
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
}

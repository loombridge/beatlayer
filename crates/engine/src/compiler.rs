//! The compiler: turns a document into an engine program, and decides which
//! edits need a new program at all.

use std::collections::BTreeMap;

use bl_model::{Change, ChangeSet, DeviceId, DeviceKind, Document, Mixer, OrdMap, ParamId, Track};

use crate::EngineConfig;
use crate::gc::{GcHandle, Program};
use crate::message::ParamEvent;
use crate::node::{Node, NodeKey, ParamValue};
use crate::nodes::{Fader, fader};
use crate::program::{EngineProgram, Migration, NodeSlot, Span, Task};
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
        self.program.is_none() && self.params.is_empty()
    }
}

/// What a node is, beyond its key: a node migrates only if this matches.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Identity {
    Device(DeviceKind),
    Fader,
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
/// the tempo or time-signature map. Mixer settings and device parameters
/// become [`ParamEvent`]s for the nodes that already run them, and clip
/// changes will travel as node data (WP-1.4), so neither recompiles.
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
        let mut update = if recompile {
            self.compile(document)
        } else {
            self.synced = Some(document.clone());
            Update::default()
        };
        update.params = params;
        update
    }

    /// A whole new program for `document`. Nodes that the last program ran
    /// migrate into it.
    pub fn compile(&mut self, document: &Document) -> Update {
        let mut build = Build::default();
        let mut main_inputs = Vec::new();
        for id in document.top_level().chain(document.returns()) {
            main_inputs.push(self.add_track(&mut build, document, id));
        }
        let output = self.add_task(&mut build, document.main(), main_inputs);

        let generation = self.generation + 1;
        let base = (self.generation > 0).then_some(self.generation);
        let tasks = build.tasks.len();
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
            pool: vec![0.0; tasks * self.config.channels * self.config.max_frames],
            channels: self.config.channels,
            max_frames: self.config.max_frames,
            output,
            tempo: document.settings().tempo.clone(),
            sample_rate: self.config.sample_rate,
            peaks: vec![[0.0; 2]; tasks],
            meters: vec![[0.0; 2]; tasks],
        };
        self.generation = generation;
        self.layout = build.layout;
        self.synced = Some(document.clone());
        Update {
            program: Some(self.gc.program(program)),
            params: Vec::new(),
            unhosted: build.unhosted,
        }
    }

    /// Forgets the last program, so the next compile makes every node anew
    /// and takes none over: for an audio thread that starts without one.
    pub fn reset(&mut self) {
        self.generation = 0;
        self.synced = None;
        self.layout.clear();
    }

    /// Adds a track and, first, the tracks that feed it. Returns its task.
    fn add_track(&self, build: &mut Build, document: &Document, id: bl_model::TrackId) -> u32 {
        let track = document
            .track(id)
            .expect("a document's tree names only its own tracks");
        let inputs = track
            .children()
            .map(|child| self.add_track(build, document, child))
            .collect();
        self.add_task(build, track, inputs)
    }

    /// Adds the task for one track's chain, fed by `inputs`.
    fn add_task(&self, build: &mut Build, track: &Track, inputs: Vec<u32>) -> u32 {
        let task = build.tasks.len() as u32;
        let input_span = span(&mut build.inputs, inputs.iter().copied());
        for &input in &inputs {
            build.dependents[input as usize].push(task);
        }
        let node_start = build.nodes.len() as u32;
        for device in track.devices() {
            let config = self.config.node_config();
            let placed = self.place(
                build,
                NodeKey::Device(device.id),
                Identity::Device(device.kind.clone()),
                !device.enabled,
                || self.registry.create(device, &config),
            );
            match placed {
                Some(slot) => build.nodes.push(slot),
                None => build.unhosted.push(device.id),
            }
        }
        let fader = self.place(
            build,
            NodeKey::Fader(track.id()),
            Identity::Fader,
            false,
            || Some(Box::new(Fader::new(track.mixer()))),
        );
        build.nodes.extend(fader);
        build.tasks.push(Task {
            track: track.id(),
            inputs: input_span,
            nodes: Span {
                start: node_start,
                end: build.nodes.len() as u32,
            },
            dependents: Span::default(),
            dependencies: inputs.len() as u32,
        });
        build.dependents.push(Vec::new());
        task
    }

    /// Gives a node a slot: the node that ran under `key` in the last
    /// program if it's the same kind of node, or a new one from `make`.
    /// Returns `None`, adding nothing, if there's neither.
    fn place(
        &self,
        build: &mut Build,
        key: NodeKey,
        identity: Identity,
        bypass: bool,
        make: impl FnOnce() -> Option<Box<dyn Node>>,
    ) -> Option<u32> {
        let slot = build.slots.len() as u32;
        let node = match self.layout.get(&key) {
            Some(placed) if placed.identity == identity => {
                build.migrations.push(Migration {
                    from: placed.slot,
                    to: slot,
                });
                None
            }
            _ => Some(make()?),
        };
        build.slots.push(NodeSlot { key, node, bypass });
        build.layout.insert(key, Placed { slot, identity });
        Some(slot)
    }
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
    inputs: Vec<u32>,
    nodes: Vec<u32>,
    dependents: Vec<Vec<u32>>,
    unhosted: Vec<DeviceId>,
}

/// Appends `items` to a flat list and returns where they went.
fn span(list: &mut Vec<u32>, items: impl IntoIterator<Item = u32>) -> Span {
    let start = list.len() as u32;
    list.extend(items);
    Span {
        start,
        end: list.len() as u32,
    }
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

/// Whether a change set needs a new program; collects parameter changes for
/// the nodes that keep running either way.
fn needs_program(
    changes: &ChangeSet,
    old: &Document,
    new: &Document,
    params: &mut Vec<ParamEvent>,
) -> bool {
    let mut recompile = changes.settings || changes.layout;
    for (&id, &change) in &changes.tracks {
        match (change, old.track(id), new.track(id)) {
            (Change::Modified, Some(before), Some(after)) => {
                let same_chain = before
                    .devices()
                    .map(|d| d.id)
                    .eq(after.devices().map(|d| d.id));
                if !same_chain || !before.children().eq(after.children()) {
                    recompile = true;
                }
                mixer_events(id, before.mixer(), after.mixer(), params);
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

/// Parameter changes for a track's fader.
fn mixer_events(
    track: bl_model::TrackId,
    before: &Mixer,
    after: &Mixer,
    params: &mut Vec<ParamEvent>,
) {
    let key = NodeKey::Fader(track);
    let switch = |on: bool| if on { 1.0 } else { 0.0 };
    if before.volume != after.volume {
        params.push(ParamEvent::now(key, fader::VOLUME, after.volume));
    }
    if before.pan != after.pan {
        params.push(ParamEvent::now(key, fader::PAN, after.pan));
    }
    if before.mute != after.mute {
        params.push(ParamEvent::now(key, fader::MUTE, switch(after.mute)));
    }
    if before.solo != after.solo {
        params.push(ParamEvent::now(key, fader::SOLO, switch(after.solo)));
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
    use bl_model::{Command, Device, Editor, TrackId, TrackKind};

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
        // A fader per track and the tone.
        assert_eq!(program.node_count(), 6);
        assert!(update.unhosted.is_empty());
        // The graph a worker pool would walk reaches every task.
        let ready = program.ready_order();
        assert_eq!(ready.len(), program.task_count());
        let position = |t: u32| ready.iter().position(|&r| r == t).unwrap();
        for (t, task) in program.tasks.iter().enumerate() {
            for &input in &program.task_inputs[task.inputs.range()] {
                assert!(position(input) < position(t as u32));
            }
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
            vec![ParamEvent::now(NodeKey::Fader(f.outer), fader::VOLUME, 0.5)]
        );
        let update = f.apply(Command::SetMute {
            track: f.outer,
            mute: true,
        });
        assert_eq!(
            update.params,
            vec![ParamEvent::now(NodeKey::Fader(f.outer), fader::MUTE, 1.0)]
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

        // A new track brings new nodes; the rest migrate.
        let new = track(&mut f.editor, TrackKind::Audio, None);
        let update = f.sync();
        let program = update.program.as_ref().unwrap().get();
        assert_eq!(program.node_count(), 7);
        assert_eq!(program.migration_count(), 6);
        assert!(program.slot_of(NodeKey::Fader(new)).is_some());

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

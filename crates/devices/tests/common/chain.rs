//! A song with MIDI clips playing through MIDI effects into the engine's
//! note-recording test instrument, on one thread, each block a real-time
//! section under the allocation trap (WP-5.30).

#![allow(dead_code, reason = "each test file uses some")]

use std::sync::Arc;

use bl_core::time::{SampleRate, TICKS_PER_QUARTER, Ticks};
use bl_engine::testing::{self, Probe, RecordedNote};
use bl_engine::{
    Collector, Compiler, EngineConfig, Monitor, NoteKind, Processor, Registry, Sender,
    SessionCommand, TransportCommand, Update, channel,
};
use bl_model::{
    Clip, ClipData, ClipId, Command, Device, DeviceId, DeviceKind, Editor, LaunchQuantization,
    LaunchSettings, MidiData, Note, ParamId, Scale, SceneId, TrackId, TrackKind,
};

pub const RATE: SampleRate = SampleRate::HZ_48000;
/// One beat at 120 BPM, in frames.
pub const BEAT_FRAMES: i64 = 24_000;
pub const BEAT: Ticks = Ticks(TICKS_PER_QUARTER);

/// What the instrument heard: `(frame, kind, key, velocity)`.
pub type Heard = (i64, NoteKind, u8, f32);

pub struct Chain {
    pub editor: Editor,
    pub probe: Arc<Probe>,
    pub compiler: Compiler,
    pub sender: Sender,
    pub processor: Processor,
    pub track: TrackId,
    _collector: Collector,
    _monitor: Monitor,
}

impl Chain {
    /// A MIDI track holding a test instrument, in a song that plays blocks
    /// of up to `max_frames` frames.
    pub fn new(max_frames: usize) -> Self {
        Self::with_registry(max_frames, |_| {})
    }

    /// As [`new`](Self::new), with `extend` adding device types to the
    /// registry.
    pub fn with_registry(max_frames: usize, extend: impl FnOnce(&mut Registry)) -> Self {
        let config = EngineConfig::new(RATE, max_frames);
        let probe = Arc::new(Probe::default());
        let collector = Collector::new();
        let mut registry = testing::registry(&probe);
        bl_devices::register(&mut registry);
        extend(&mut registry);
        let compiler = Compiler::new(config, registry, collector.handle());
        let (sender, processor, monitor) = channel(&config);
        let mut editor = Editor::new();
        let track = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Midi,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        let mut chain = Self {
            editor,
            probe,
            compiler,
            sender,
            processor,
            track,
            _collector: collector,
            _monitor: monitor,
        };
        // The instrument last, after every MIDI effect inserted before it.
        chain.add(testing::INSTRUMENT, &[]);
        chain
    }

    /// Inserts the native device `device_type`, with `params` set, as the
    /// last MIDI effect (in front of the instrument).
    pub fn effect(&mut self, device_type: &str, params: &[(ParamId, f64)]) -> DeviceId {
        let index = self.effects();
        self.insert(index, device_type, params)
    }

    /// How many MIDI effects the track holds.
    pub fn effects(&self) -> usize {
        self.editor
            .document()
            .track(self.track)
            .unwrap()
            .devices()
            .len()
            - 1
    }

    fn add(&mut self, device_type: &str, params: &[(ParamId, f64)]) -> DeviceId {
        let index = self
            .editor
            .document()
            .track(self.track)
            .unwrap()
            .devices()
            .len();
        self.insert(index, device_type, params)
    }

    /// Inserts a device at `index` in the chain.
    pub fn insert(
        &mut self,
        index: usize,
        device_type: &str,
        params: &[(ParamId, f64)],
    ) -> DeviceId {
        let id = self.editor.new_id();
        let mut device = Device::new(
            id,
            "",
            DeviceKind::Native {
                device_type: device_type.to_owned(),
            },
        );
        for &(param, value) in params {
            device.params.insert(param, value);
        }
        self.editor
            .apply(Command::InsertDevice {
                track: self.track,
                index,
                device,
            })
            .unwrap();
        id
    }

    /// A clip of `notes` at the song's start, `beats` beats long.
    pub fn clip(&mut self, beats: i64, notes: impl IntoIterator<Item = Note>) {
        let id = self.editor.new_id();
        self.editor
            .apply(Command::CreateClip {
                track: self.track,
                clip: Clip::new(
                    id,
                    Ticks::ZERO,
                    Ticks(TICKS_PER_QUARTER * beats),
                    ClipData::Midi(MidiData::new(notes)),
                ),
            })
            .unwrap();
    }

    /// A clip of `notes` from beat `at`, `beats` beats long, in `scale`.
    pub fn clip_at(
        &mut self,
        at: i64,
        beats: i64,
        scale: Scale,
        notes: impl IntoIterator<Item = Note>,
    ) -> ClipId {
        let id = self.editor.new_id();
        let mut clip = Clip::new(
            id,
            Ticks(TICKS_PER_QUARTER * at),
            Ticks(TICKS_PER_QUARTER * beats),
            ClipData::Midi(MidiData::new(notes)),
        );
        clip.scale = scale;
        self.editor
            .apply(Command::CreateClip {
                track: self.track,
                clip,
            })
            .unwrap();
        id
    }

    /// A clip of `notes` in the Session slot of the first scene, `beats`
    /// beats long, in `scale`; the scene.
    pub fn slot_clip(
        &mut self,
        beats: i64,
        scale: Scale,
        notes: impl IntoIterator<Item = Note>,
    ) -> SceneId {
        let scene = self.editor.document().scenes().next().unwrap().id;
        let id = self.editor.new_id();
        let mut clip = Clip::new(
            id,
            Ticks::ZERO,
            Ticks(TICKS_PER_QUARTER * beats),
            ClipData::Midi(MidiData::new(notes)),
        );
        clip.scale = scale;
        self.editor
            .apply(Command::CreateSlotClip {
                track: self.track,
                scene,
                clip,
            })
            .unwrap();
        scene
    }

    /// Launches the slot of `scene` on the next beat line.
    pub fn launch(&mut self, scene: SceneId) {
        self.sender
            .session(SessionCommand::SetQuantization(LaunchSettings {
                quantization: LaunchQuantization::Quarter,
                enabled: true,
                ..LaunchSettings::default()
            }));
        self.sender.session(SessionCommand::Launch {
            track: self.track,
            scene,
        });
    }

    /// Sets the scale of every clip on the track.
    pub fn scale_clips(&mut self, scale: Scale) {
        let clips = self
            .editor
            .document()
            .track(self.track)
            .unwrap()
            .clips()
            .map(|clip| clip.id)
            .collect();
        self.apply(scale.command(clips));
    }

    /// Sets the set's current scale.
    pub fn scale_set(&mut self, scale: Scale) {
        self.apply(Command::SetScale {
            enabled: Some(scale.enabled),
            root: Some(scale.root),
            name: Some(scale.name.id().to_owned()),
        });
    }

    /// Applies a command.
    pub fn apply(&mut self, command: Command) {
        self.editor.apply(command).unwrap();
        self.sync();
    }

    /// Sets a parameter of `device`.
    pub fn set(&mut self, device: DeviceId, param: ParamId, value: f64) {
        self.apply(Command::SetDeviceParam {
            device,
            param,
            value,
        });
    }

    /// Syncs the document to the engine.
    pub fn sync(&mut self) {
        let update: Update = self.compiler.sync(self.editor.document());
        self.sender.send_update(update);
        assert!(self.sender.flush(), "the queue is full");
    }

    pub fn play(&mut self) {
        self.sender.transport(TransportCommand::Play);
    }

    pub fn stop(&mut self) {
        self.sender.transport(TransportCommand::Stop);
    }

    /// Processes `frames` frames in blocks of `size`, each a real-time
    /// section.
    pub fn run(&mut self, frames: usize, size: usize) {
        let mut remaining = frames;
        while remaining > 0 {
            let n = remaining.min(size);
            let processor = &mut self.processor;
            bl_rt::rt_test!({ processor.process(n) });
            remaining -= n;
        }
    }

    /// Every note the instrument recorded since the last call.
    pub fn recorded(&self) -> Vec<RecordedNote> {
        self.probe.recorded_notes()
    }

    /// The same, as `(frame, kind, key, velocity)`.
    pub fn heard(&self) -> Vec<Heard> {
        self.recorded()
            .into_iter()
            .map(|n| (n.frame, n.event.kind, n.event.key, n.event.velocity))
            .collect()
    }
}

/// A note of `key` at beat `at`, `beats` long, velocity 100.
pub fn note(key: u8, at: i64, beats: i64) -> Note {
    Note::new(
        key,
        Ticks(TICKS_PER_QUARTER * at),
        Ticks(TICKS_PER_QUARTER * beats),
        100,
    )
}

/// Whether every note-on in `heard` has a later note-off on its key, and no
/// key is left sounding. An off that comes before the on it ends fails; an
/// extra off after the key has ended is harmless, as the instrument ignores it.
pub fn nothing_hangs(heard: &[Heard]) -> bool {
    let mut sounding = std::collections::BTreeMap::<u8, i32>::new();
    for (i, &(_, kind, key, _)) in heard.iter().enumerate() {
        let count = sounding.entry(key).or_default();
        match kind {
            NoteKind::On => *count += 1,
            NoteKind::Off | NoteKind::Choke => {
                *count -= 1;
                let on_later = heard[i..].iter().any(|h| h.2 == key && h.1 == NoteKind::On);
                if *count < 0 && on_later {
                    return false;
                }
            }
            _ => {}
        }
    }
    sounding.values().all(|&n| n <= 0)
}

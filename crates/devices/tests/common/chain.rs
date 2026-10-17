//! A song with MIDI clips playing through MIDI effects into the engine's
//! note-recording test instrument, on one thread, each block a real-time
//! section under the allocation trap (WP-5.30).

#![allow(dead_code, reason = "each test file uses some")]

use std::sync::Arc;

use bl_core::time::{SampleRate, TICKS_PER_QUARTER, Ticks};
use bl_engine::testing::{self, Probe, RecordedNote};
use bl_engine::{
    Collector, Compiler, EngineConfig, Monitor, NoteKind, Processor, Sender, TransportCommand,
    Update, channel,
};
use bl_model::{
    Clip, ClipData, Command, Device, DeviceId, DeviceKind, Editor, MidiData, Note, ParamId,
    TrackId, TrackKind,
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
        let config = EngineConfig::new(RATE, max_frames);
        let probe = Arc::new(Probe::default());
        let collector = Collector::new();
        let mut registry = testing::registry(&probe);
        bl_devices::register(&mut registry);
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

/// Whether every note-on in `heard` has a later note-off on its key, and
/// no key is left sounding.
pub fn nothing_hangs(heard: &[Heard]) -> bool {
    let mut sounding = std::collections::BTreeMap::<u8, i32>::new();
    for &(_, kind, key, _) in heard {
        match kind {
            NoteKind::On => *sounding.entry(key).or_default() += 1,
            NoteKind::Off | NoteKind::Choke => *sounding.entry(key).or_default() -= 1,
            NoteKind::Expression { .. } => {}
        }
    }
    sounding.values().all(|&n| n <= 0)
}

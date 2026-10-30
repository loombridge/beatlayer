//! The rig every test runs on: the host and its ports on the test backend, a
//! simulated Push 2, the test clock, and a recording content for the modes.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bl_control::{CommandRequest, ReadModel, RingView, RowConfig, RowView, RunState, Surfaces};
use bl_core::clock::TestClock;
use bl_core::midi::MidiInputEvent;
use bl_midi_io::{MidiPorts, OutputBus, input_queue};
use bl_model::{Command, Editor, TrackId, TrackKind};
use bl_push::content::{
    Content, Contents, Ctx, Gesture, MakeContents, Pad, PadEvent, Paint, StripEvent, Turn,
};
use bl_push::content::{StripUse, Target};
use bl_push::leds::Led;
use bl_push::sim::{LIVE, Simulated};
use bl_push::{Handle, Shared, Snapshot};
use rtrb::Consumer;

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a recording content heard.
#[derive(Clone, Default)]
pub struct Heard(pub Arc<Mutex<Vec<String>>>);

impl Heard {
    pub fn all(&self) -> Vec<String> {
        lock(&self.0).clone()
    }

    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut *lock(&self.0))
    }
}

/// Settings a test gives its recording contents.
#[derive(Clone, Default)]
pub struct Flags {
    /// Light every button and pad when painting.
    pub light_all: bool,
    /// Claim the lower display buttons.
    pub claim_lower: bool,
    /// Scale is available.
    pub scale: bool,
    /// Play the pads' notes into the tracks.
    pub play_pads: bool,
    /// Let a few buttons ask the surface for things: a strip use, the
    /// picker, a notification.
    pub hooks: bool,
}

pub struct Recorder {
    pub name: &'static str,
    pub heard: Heard,
    pub flags: Arc<Mutex<Flags>>,
}

impl Recorder {
    fn note(&self, what: String) {
        lock(&self.heard.0).push(format!("{}: {what}", self.name));
    }
}

fn button_name(number: u8) -> &'static str {
    bl_push::controls::button(number).map_or("?", |b| b.name)
}

impl Content for Recorder {
    fn name(&self) -> &str {
        self.name
    }

    fn button(&mut self, button: u8, gesture: Gesture, ctx: &mut Ctx<'_>) {
        self.note(format!("{} {gesture:?}", button_name(button)));
        // Hooks that let a test reach what a content may ask of the surface.
        if gesture == Gesture::Press && self.name == "Device" && lock(&self.flags).hooks {
            match button_name(button) {
                "add_device" => ctx.set_strip(StripUse::Range),
                "add_track" => ctx.set_strip(StripUse::Modulation),
                "repeat" => ctx.set_strip(StripUse::PitchBend),
                "accent" => ctx.open_picker(Target::Command {
                    command: "test.set_colour".to_owned(),
                    args: serde_json::json!({ "id": 7 }),
                    current: Some(3),
                }),
                "convert" => ctx.notify("Hello"),
                _ => {}
            }
        }
    }

    fn pad(&mut self, pad: Pad, event: PadEvent, ctx: &mut Ctx<'_>) {
        self.note(format!("pad {},{} {event:?}", pad.row, pad.column));
        if lock(&self.flags).play_pads {
            let note = bl_push::controls::pad_note(pad.row, pad.column);
            match event {
                PadEvent::On(velocity) => ctx
                    .cx
                    .play(bl_core::midi::MidiMessage::note_on(0, note, velocity)),
                PadEvent::Off => ctx.cx.play(bl_core::midi::MidiMessage::note_off(0, note)),
                _ => {}
            }
        }
    }

    fn encoder(&mut self, column: u8, turn: Turn, ctx: &mut Ctx<'_>) {
        self.note(format!(
            "encoder {column} {turn:?} shift={}",
            ctx.modifiers.shift()
        ));
    }

    fn strip(&mut self, event: StripEvent, _ctx: &mut Ctx<'_>) {
        self.note(format!("strip {event:?}"));
    }

    fn paint(&mut self, paint: &mut Paint<'_>) {
        if lock(&self.flags).light_all {
            for button in bl_push::controls::BUTTONS {
                let led = match button.led {
                    bl_push::controls::LedKind::White => Led::steady(127),
                    bl_push::controls::LedKind::Rgb => Led::steady(126),
                };
                paint.lights.button(button.number, led);
            }
            for row in 0..8 {
                for column in 0..8 {
                    paint.lights.pad(row, column, Led::steady(125));
                }
            }
        }
    }

    fn claims_lower(&self) -> bool {
        lock(&self.flags).claim_lower
    }

    fn scale_available(&self) -> bool {
        lock(&self.flags).scale
    }
}

pub struct Rig {
    pub clock: TestClock,
    pub ports: MidiPorts,
    pub surfaces: Surfaces,
    pub sim: Simulated,
    pub shared: Arc<Shared>,
    pub engine: Consumer<MidiInputEvent>,
    pub model: ReadModel,
    pub heard: Heard,
    pub flags: Arc<Mutex<Flags>>,
    pub tracks: Vec<TrackId>,
    pub editor: Editor,
    taken: Vec<CommandRequest>,
}

impl Rig {
    /// A rig with a set of eight MIDI tracks, a Push 2 not yet plugged in,
    /// and recording contents in every mode.
    pub fn new() -> Self {
        Self::build(false)
    }

    /// A rig whose modes have no content of their own yet: the placeholders.
    pub fn placeholders() -> Self {
        Self::build(true)
    }

    fn build(placeholders: bool) -> Self {
        let backend = bl_midi_io::TestBackend::new();
        let clock = TestClock::new();
        let (sender, engine) = input_queue(16_384);
        let bus = OutputBus::new();
        let ports =
            MidiPorts::new(Box::new(backend.clone()), sender.clone()).with_output(bus.clone());
        let mut surfaces = Surfaces::new(sender, bus);
        surfaces.set_clock(clock.clone());
        let sim = Simulated::new(&backend, &clock);
        let shared = Arc::new(Shared::default());
        let heard = Heard::default();
        let flags = Arc::new(Mutex::new(Flags::default()));
        let make: MakeContents = {
            let (heard, flags) = (heard.clone(), Arc::clone(&flags));
            Arc::new(move || {
                let recorder = |name| {
                    Box::new(Recorder {
                        name,
                        heard: heard.clone(),
                        flags: Arc::clone(&flags),
                    }) as Box<dyn Content>
                };
                Contents {
                    pads: [recorder("Note"), recorder("Session")],
                    display: [
                        recorder("Device"),
                        recorder("Browse"),
                        recorder("Mix"),
                        recorder("Clip"),
                    ],
                }
            })
        };
        let make = if placeholders {
            bl_push::placeholder_contents()
        } else {
            make
        };
        surfaces.set_natives(vec![Arc::new(bl_push::kind(
            Arc::clone(&shared),
            sim.frames().claim(),
            make,
        ))]);

        let mut editor = Editor::new();
        let mut tracks = Vec::new();
        for index in 0..10usize {
            let id = editor.new_id();
            editor
                .apply(Command::CreateTrack {
                    id,
                    kind: TrackKind::Midi,
                    name: format!("Track {}", index + 1),
                    parent: None,
                    index,
                })
                .unwrap();
            tracks.push(id);
        }
        let mut model = ReadModel::at_rest(Arc::new(editor.document().clone()));
        let ring: Vec<TrackId> = tracks.iter().take(8).copied().collect();
        model.rows = vec![RowView {
            ring: Some(RingView {
                size: bl_control::Ring {
                    tracks: 8,
                    scenes: 8,
                },
                first_track: ring.first().copied(),
                first_scene: None,
                tracks: ring,
                scenes: Vec::new(),
                moves: bl_control::Moves::default(),
            }),
            focus: None,
        }];
        Self {
            clock,
            ports,
            surfaces,
            sim,
            shared,
            engine,
            model,
            heard,
            flags,
            tracks,
            editor,
            taken: Vec::new(),
        }
    }

    /// Applies `command` to the set and gives the surfaces the new document.
    pub fn apply(&mut self, command: Command) {
        self.editor.apply(command).unwrap();
        self.model.document = Arc::new(self.editor.document().clone());
    }

    /// Plugs the Push 2 in and puts its Live port in row 1, as the shell
    /// does when it finds one.
    pub fn start(&mut self) {
        self.sim.plug_in();
        self.surfaces
            .set_row(
                0,
                RowConfig {
                    surface: Some(bl_push::SURFACE_ID.to_owned()),
                    input: Some(LIVE.to_owned()),
                    output: Some(LIVE.to_owned()),
                },
            )
            .unwrap();
        self.sync();
    }

    /// Lists the ports again and makes the surfaces follow the rows, then
    /// lets the device and the host talk until both are quiet.
    pub fn sync(&mut self) {
        self.ports.refresh().unwrap();
        self.surfaces.reconcile(&mut self.ports);
        self.settle();
    }

    /// Lets the device and the host talk until both are quiet.
    pub fn settle(&mut self) {
        loop {
            self.surfaces.settle();
            if self.sim.pump() == 0 {
                break;
            }
        }
        self.surfaces.settle();
    }

    /// One tick of the surfaces with the model as it stands, then quiet.
    pub fn tick(&mut self) {
        self.surfaces.publish(self.model.clone());
        self.surfaces.tick();
        self.settle();
    }

    /// Moves the clock on by `ms` and ticks once.
    pub fn advance(&mut self, ms: u64) {
        self.clock.advance(Duration::from_millis(ms));
        self.tick();
    }

    pub fn run(&self) -> RunState {
        self.surfaces.report(&self.ports)[0].run
    }

    pub fn handle(&self) -> Arc<Handle> {
        self.surfaces
            .probe(0)
            .expect("the row runs a Push 2")
            .downcast::<Handle>()
            .unwrap_or_else(|_| panic!("the probe is a handle"))
    }

    pub fn snapshot(&self) -> Snapshot {
        self.handle().snapshot()
    }

    /// What surfaces asked for since the last call, in order.
    pub fn requests(&mut self) -> Vec<CommandRequest> {
        self.surfaces.take_requests(&mut self.taken);
        std::mem::take(&mut self.taken)
    }

    /// The commands asked for since the last call, as `id args`.
    pub fn commands(&mut self) -> Vec<String> {
        self.requests()
            .into_iter()
            .map(|r| match &r.ask {
                bl_control::Ask::Command(c) => format!("{} {}", c.id, c.args),
                bl_control::Ask::EndGesture(control) => format!("end {control:#x}"),
                bl_control::Ask::Control(_) => "control".to_owned(),
            })
            .collect()
    }

    pub fn reached_tracks(&mut self) -> Vec<MidiInputEvent> {
        std::iter::from_fn(|| self.engine.pop().ok()).collect()
    }

    /// Taps a button: down, and up within 300 ms, then a tick.
    pub fn tap(&mut self, name: &str) {
        assert!(self.sim.press(name));
        self.settle();
        self.advance(50);
        assert!(self.sim.let_go(name));
        self.advance(1);
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        // Every sysex any test made the surface send is on the allowlist, and
        // none is one of the three that outlast a restart (R5).
        self.sim.pump();
        assert!(
            self.sim.violations.is_empty(),
            "sysex outside the allowlist: {:02X?}",
            self.sim.violations
        );
    }
}

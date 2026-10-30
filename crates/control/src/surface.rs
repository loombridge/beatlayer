//! The `ControlSurface` trait and what a surface is given and answers with
//! (`docs/specs/control-surfaces.md`, R6 and R7).
//!
//! A surface is Beatlayer code written against this interface and run by the
//! host (`host.rs`) on a thread of its own: it receives its input's
//! messages, sends its output's, reads the set and what the engine is
//! doing, asks for the registered commands a person would run, and gets a
//! regular tick. It reads nothing else and changes nothing except through
//! the commands it asks for.

use std::any::Any;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bl_core::midi::MidiMessage;
use bl_core::time::{Bpm, Ticks};
use bl_engine::{SlotStatus, TrackMeter};
use bl_model::{Document, SceneId, TrackId};
use serde_json::Value;

/// How often a surface gets its tick: 50 times a second (R6).
pub const TICK: Duration = Duration::from_millis(20);

/// The transport as the main thread last heard the engine report it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transport {
    /// Whether the song is playing.
    pub playing: bool,
    /// The playhead.
    pub position: Ticks,
    /// Whether the Arrangement loop is on.
    pub loop_on: bool,
    /// Where the loop brace starts.
    pub loop_start: Ticks,
    /// How long the loop brace is.
    pub loop_length: Ticks,
}

/// What is selected in the set: the Arrangement's tracks, and the Session
/// View's slots and scenes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    /// The selected tracks, in the order they were selected.
    pub tracks: Vec<TrackId>,
    /// The selected slots, as a track and a scene.
    pub slots: Vec<(TrackId, SceneId)>,
    /// The selected scenes.
    pub scenes: Vec<SceneId>,
}

/// What the host hands a surface on each tick (R6): the document as the
/// main thread last published it, the engine's published state, and the
/// selection.
#[derive(Clone, Debug)]
pub struct ReadModel {
    /// The set. Cheap to clone and never changed under the surface.
    pub document: Arc<Document>,
    /// The Session slots that are queued, playing or recording, and the
    /// song frame each launched on.
    pub slots: Vec<SlotStatus>,
    /// The transport.
    pub transport: Transport,
    /// The tempo in force at the playhead.
    pub tempo: Bpm,
    /// Each track's meters.
    pub meters: Vec<TrackMeter>,
    /// The selection.
    pub selection: Selection,
}

impl ReadModel {
    /// A model of `document` with the engine at rest: nothing playing, no
    /// meters, nothing selected.
    #[must_use]
    pub fn at_rest(document: Arc<Document>) -> Self {
        Self {
            document,
            slots: Vec::new(),
            transport: Transport {
                playing: false,
                position: Ticks::ZERO,
                loop_on: false,
                loop_start: Ticks::ZERO,
                loop_length: Ticks::ZERO,
            },
            tempo: Bpm::DEFAULT,
            meters: Vec::new(),
            selection: Selection::default(),
        }
    }
}

/// A block of tracks and scenes a surface's grid controls (R28): its size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ring {
    /// How many tracks wide.
    pub tracks: u8,
    /// How many scenes tall; zero for a surface with no clip launching.
    pub scenes: u8,
}

/// What a running surface reports of itself, for `control_surface.list`
/// (R13).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SurfaceState {
    /// The surface's ring.
    pub ring: Option<Ring>,
    /// The surface's own mode, by name.
    pub mode: Option<String>,
}

/// One message for the surface's output: its bytes, whole (a system
/// exclusive message included), and when to send it (R8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Out {
    /// The message.
    pub bytes: Vec<u8>,
    /// The moment the port's backend sends it at, or `None` for at once.
    pub at: Option<Instant>,
    /// The control the message lights or moves, so a surface that sends
    /// faster than its port takes loses the oldest messages for the same
    /// control first.
    pub control: Option<u32>,
}

/// How a command a surface asks for takes part in a gesture (R7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gesture {
    /// The control being held or turned.
    pub control: u32,
    /// Whether this is the control's last change: it has been released.
    pub released: bool,
}

/// A command a surface asks for: run on the main thread, in order, by ID
/// with JSON arguments (R7).
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    /// The command's ID.
    pub id: String,
    /// Its arguments.
    pub args: Value,
    /// Set when the command is part of a gesture on a control.
    pub gesture: Option<Gesture>,
}

/// What a surface does in a call: the messages it sends, the commands it
/// asks for, and the messages it hands to tracks.
#[derive(Debug)]
pub struct Cx {
    pub(crate) now: Instant,
    pub(crate) out: Vec<Out>,
    pub(crate) requests: Vec<Request>,
    pub(crate) handed: Vec<(Instant, MidiMessage)>,
    pub(crate) played: Vec<MidiMessage>,
}

impl Cx {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            now,
            out: Vec::new(),
            requests: Vec::new(),
            handed: Vec::new(),
            played: Vec::new(),
        }
    }

    /// The time now, on the clock the host stamps messages with.
    #[must_use]
    pub const fn now(&self) -> Instant {
        self.now
    }

    /// Sends `bytes` to the output, after everything sent before.
    pub fn send(&mut self, bytes: impl Into<Vec<u8>>) {
        self.push(bytes.into(), None, None);
    }

    /// Sends `bytes` for `control`: if the port can't keep up, an older
    /// message for the same control is dropped before any other (R8).
    pub fn send_for(&mut self, control: u32, bytes: impl Into<Vec<u8>>) {
        self.push(bytes.into(), None, Some(control));
    }

    /// Sends `bytes` on the moment `at`, where the port's backend can
    /// schedule it (R8).
    pub fn send_at(&mut self, at: Instant, bytes: impl Into<Vec<u8>>) {
        self.push(bytes.into(), Some(at), None);
    }

    fn push(&mut self, bytes: Vec<u8>, at: Option<Instant>, control: Option<u32>) {
        self.out.push(Out { bytes, at, control });
    }

    /// Asks for a command to be run, as a click would run it.
    pub fn run(&mut self, id: &str, args: Value) {
        self.request(id, args, None);
    }

    /// Asks for a command that is part of a gesture on `control`: the
    /// changes up to its release are one undo step, and a control with no
    /// release ends the step after 500 ms of rest (R7).
    pub fn run_held(&mut self, control: u32, id: &str, args: Value) {
        self.request(
            id,
            args,
            Some(Gesture {
                control,
                released: false,
            }),
        );
    }

    /// Asks for the last command of a gesture on `control`, which has been
    /// released.
    pub fn run_released(&mut self, control: u32, id: &str, args: Value) {
        self.request(
            id,
            args,
            Some(Gesture {
                control,
                released: true,
            }),
        );
    }

    fn request(&mut self, id: &str, args: Value, gesture: Option<Gesture>) {
        self.requests.push(Request {
            id: id.to_owned(),
            args,
            gesture,
        });
    }

    /// Hands a message back to the tracks, as arriving at `at`: they
    /// receive it as from the surface's input port while that port's Track
    /// switch is on (R4).
    pub fn hand_back(&mut self, at: Instant, message: MidiMessage) {
        self.handed.push((at, message));
    }

    /// Plays a message into the tracks as the surface's input port's own,
    /// whatever the port's Track switch says (R5).
    pub fn play(&mut self, message: MidiMessage) {
        self.played.push(message);
    }
}

/// A control surface. The host calls it on its own thread, one call at a
/// time, and stops it if a call panics or takes longer than 100 ms (R10).
pub trait ControlSurface: Send {
    /// The surface starts: send the controller whatever puts it into the
    /// state the surface drives, and fill every light it owns (R9).
    fn start(&mut self, cx: &mut Cx);

    /// The surface stops: send what gives the controller back its own
    /// state, with its lights put out (R9).
    fn stop(&mut self, cx: &mut Cx);

    /// A message arrived on the surface's input at `at`, whole (R6).
    fn message(&mut self, at: Instant, bytes: &[u8], cx: &mut Cx);

    /// The tick, 50 times a second, with the read model (R6).
    fn tick(&mut self, model: &ReadModel, cx: &mut Cx);

    /// What the surface reports of itself (R13).
    fn state(&self) -> SurfaceState {
        SurfaceState::default()
    }

    /// A handle for a test to read what the surface recorded.
    fn probe(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        None
    }
}

/// A surface the chooser offers.
#[derive(Clone, Copy)]
pub struct Kind {
    /// The surface's ID, as the preferences and commands keep it.
    pub id: &'static str,
    /// The name the chooser shows.
    pub name: &'static str,
    /// Whether the surface needs an output to start (R9).
    pub needs_output: bool,
    /// Whether a device can be locked to the surface (R23).
    pub can_lock: bool,
    /// The size of the surface's ring, if it has one (R13).
    pub ring: Option<Ring>,
    /// The messages Dump sends the controller; none for a surface with no
    /// setup to send (R11).
    pub dump: &'static [&'static [u8]],
    /// Makes the surface.
    pub make: fn() -> Box<dyn ControlSurface>,
}

impl std::fmt::Debug for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Kind")
            .field("id", &self.id)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// Every surface the chooser offers, by name (R1): the native surfaces
/// first in alphabetical order. The test surface is there in test builds
/// only (R16).
#[must_use]
pub fn kinds() -> &'static [Kind] {
    static KINDS: &[Kind] = &[
        #[cfg(feature = "testing")]
        crate::testing::KIND,
    ];
    KINDS
}

/// The surface with this ID.
#[must_use]
pub fn kind(id: &str) -> Option<&'static Kind> {
    kinds().iter().find(|kind| kind.id == id)
}

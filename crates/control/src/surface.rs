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
use bl_model::{
    ChainId, DeviceId, Document, ParamDescriptor, ParamId, RackViewMap, RemoteMode, SceneId,
    TrackId,
};
use serde_json::Value;

use crate::Hit;

/// How often a surface gets its tick: 50 times a second (R6).
pub const TICK: Duration = Duration::from_millis(20);

/// The transport as the main thread last heard the engine report it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transport {
    /// Whether the song is playing.
    pub playing: bool,
    /// The playhead.
    pub position: Ticks,
    /// The insert marker, where Play starts.
    pub insert_marker: Ticks,
    /// Whether the Arrangement loop is on.
    pub loop_on: bool,
    /// Whether the transport is recording.
    pub recording: bool,
    /// Where the loop brace starts.
    pub loop_start: Ticks,
    /// How long the loop brace is.
    pub loop_length: Ticks,
    /// Whether a count-in runs before the recording.
    pub counting_in: bool,
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
    /// Where each track's playing Session clip is in its loop, from 0 to 1.
    pub progress: Vec<(TrackId, f32)>,
    /// What each row of the Control Surfaces table holds: its ring, the
    /// device its device controls reach and its bank (R17 to R35).
    pub rows: Vec<RowView>,
    /// The tracks a ring runs along, in order: the Session View's columns
    /// with a folded group's tracks left out, then the returns (R28).
    pub columns: Vec<TrackId>,
    /// Whether Session Record is on (session recording).
    pub session_record: bool,
    /// Whether the metronome is on.
    pub metronome: bool,
    /// The drum rack on the selected track, if its first instrument at the
    /// top level is one (supported controllers R15, R16).
    pub drum: Option<DrumView>,
    /// The browser's lists, for a surface that browses (push-2-tracks R13).
    pub browse: Option<crate::browse::Browse>,
    /// The device selected in the Device View, if it is on a selected track
    /// (push-2-tracks R5).
    pub selected_device: Option<DeviceId>,
    /// Whether the Session View is the main view, not the Arrangement.
    pub session_view: bool,
    /// What the Device View shows besides the set (push-2-tracks R22, R24).
    pub devices: DeviceViews,
}

/// What the Device View keeps beside the set, for a surface that follows it:
/// view state, never saved as the document and never an undo step.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeviceViews {
    /// The views each rack shows, whose devices view is the rack's fold. A
    /// rack that isn't listed shows them all.
    pub racks: Arc<RackViewMap>,
    /// The chain each rack's devices view shows, for the racks on the shown
    /// track: the chain clicked, or the first while none is. A rack with
    /// several chains selected shows none.
    pub chains: Arc<std::collections::BTreeMap<DeviceId, ChainId>>,
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
                insert_marker: Ticks::ZERO,
                loop_on: false,
                recording: false,
                loop_start: Ticks::ZERO,
                loop_length: Ticks::ZERO,
                counting_in: false,
            },
            tempo: Bpm::DEFAULT,
            meters: Vec::new(),
            selection: Selection::default(),
            progress: Vec::new(),
            rows: Vec::new(),
            columns: Vec::new(),
            session_record: false,
            metronome: false,
            drum: None,
            browse: None,
            selected_device: None,
            session_view: false,
            devices: DeviceViews::default(),
        }
    }

    /// What `row`, counting from 0, holds.
    #[must_use]
    pub fn row(&self, row: usize) -> Option<&RowView> {
        self.rows.get(row)
    }
}

/// The drum rack a grid controller follows: the rack on the selected track
/// and what its pad view shows (supported controllers R15, R16).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrumView {
    /// The rack.
    pub rack: DeviceId,
    /// The note of the pad view's first pad (its lower left one).
    pub first: u8,
    /// The note of the selected pad, if one is.
    pub selected: Option<u8>,
    /// The notes whose pad holds a chain: bit n is note n.
    pub filled: u128,
}

/// How a device came to be locked to a surface (R23, R26).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lock {
    /// By Lock to in the device's menu or `control_surface.lock`.
    Hand,
    /// By arming a track whose instrument it is.
    Arming,
}

/// One parameter of the bank a surface shows (R22, R36).
#[derive(Clone, Debug, PartialEq)]
pub struct ParamView {
    /// The parameter.
    pub param: ParamId,
    /// Its name.
    pub name: String,
    /// Its value.
    pub value: f64,
    /// Its value as the device shows it.
    pub text: String,
    /// Whether a macro holds it, so the surface can't move it (R22).
    pub held: bool,
    /// What it is: its range, kind and default, which a surface that sets
    /// it by steps needs.
    pub descriptor: ParamDescriptor,
}

/// The device a surface's device controls reach (R17 to R21, R35).
#[derive(Clone, Debug, PartialEq)]
pub struct Focus {
    /// The device.
    pub device: DeviceId,
    /// How it is locked to the surface, if it is.
    pub lock: Option<Lock>,
    /// The bank the surface shows, from 0.
    pub bank: usize,
    /// Every bank's name, in order.
    pub banks: Vec<String>,
    /// The parameters of the bank the surface shows, in the order its
    /// device controls take them.
    pub params: Vec<ParamView>,
}

/// The ways a ring can move (R29).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Moves {
    /// A track to the left.
    pub left: bool,
    /// A track to the right.
    pub right: bool,
    /// A scene up.
    pub up: bool,
    /// A scene down.
    pub down: bool,
}

/// A surface's session ring as it stands (R28 to R32).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RingView {
    /// Its size.
    pub size: Ring,
    /// Its first track, by ID.
    pub first_track: Option<TrackId>,
    /// Its first scene, by ID.
    pub first_scene: Option<SceneId>,
    /// The tracks it covers, from its first, as many as exist inside its
    /// width: a mixer control past the last does nothing (R33).
    pub tracks: Vec<TrackId>,
    /// The scenes it covers, from its first, as many as exist inside its
    /// height.
    pub scenes: Vec<SceneId>,
    /// Which ways it can move.
    pub moves: Moves,
}

/// What one running row holds: surface state, never saved (R12).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowView {
    /// The ring, for a surface that has one.
    pub ring: Option<RingView>,
    /// The device its device controls reach, if there is one.
    pub focus: Option<Focus>,
    /// Where each role the surface watches stands on its travel, from 0 to
    /// 1, so it can show a fader whatever moved it (supported controllers
    /// R14). A role that reaches nothing isn't listed.
    pub positions: Vec<(Role, f64)>,
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
    /// The roles whose positions the surface wants in its row's
    /// [`RowView::positions`].
    pub watch: Vec<Role>,
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

/// What a control does in the context the surface is in (R17 to R34):
/// the place a role takes it to is the shell's to resolve when the control
/// moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// Device control 1 to 8, from 0: the parameter of the bank at that
    /// place.
    Device(u8),
    /// The next bank of the device in focus (R18).
    NextBank,
    /// The previous bank.
    PreviousBank,
    /// The slot of the ring at a column and a row, from 0 (R32).
    Slot {
        /// The ring's column.
        column: u8,
        /// The ring's row.
        row: u8,
    },
    /// The launch button of the scene at a ring row.
    SceneLaunch(u8),
    /// Stop All Clips.
    StopAll,
    /// A ring track's stop button, by column (R33).
    Stop(u8),
    /// A ring track's volume.
    Volume(u8),
    /// A ring track's pan.
    Pan(u8),
    /// A ring track's send to the return at this place among the returns,
    /// from 0 (R33).
    Send {
        /// The ring's column.
        column: u8,
        /// The return, from 0.
        send: u8,
    },
    /// A ring track's activator.
    Mute(u8),
    /// A ring track's Solo.
    Solo(u8),
    /// A ring track's Arm.
    Arm(u8),
    /// Selects a ring track, as a click on its title bar does.
    Select(u8),
    /// A ring track's crossfade assignment.
    Crossfade(u8),
    /// The Main track's volume.
    MainVolume,
    /// The crossfader.
    Crossfader,
    /// The Cue Volume knob of Main's strip.
    CueVolume,
    /// Moves the ring (R29).
    Ring(RingStep),
}

/// How a ring moves (R29).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RingStep {
    /// One track left.
    Left,
    /// One track right.
    Right,
    /// One scene up.
    Up,
    /// One scene down.
    Down,
    /// Its width left.
    PageLeft,
    /// Its width right.
    PageRight,
    /// Its height up.
    PageUp,
    /// Its height down.
    PageDown,
}

/// A control the surface's person moved: its role, how its messages read,
/// and what this one is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Control {
    /// What the control is for.
    pub role: Role,
    /// How the controller's value reads: absolute, or relative in one of the
    /// four encodings (remote mapping R20 to R27).
    pub mode: RemoteMode,
    /// What the message was to the control.
    pub hit: Hit,
}

/// What a surface asks for, in the order it asks.
#[derive(Clone, Debug, PartialEq)]
pub enum Ask {
    /// A command.
    Command(Request),
    /// A control moved, which the shell takes to its place (R22).
    Control(Control),
    /// An absolute control the surface knows rests at a value, which the
    /// shell keeps for Takeover Mode without running anything (supported
    /// controllers R35).
    Rest(Control),
    /// The control's gesture ended without a last command: its touch was
    /// let go, so the changes since it was touched are one undo step (R7).
    EndGesture(u32),
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
    pub(crate) row: usize,
    input: String,
    pub(crate) now: Instant,
    pub(crate) out: Vec<Out>,
    pub(crate) requests: Vec<Ask>,
    pub(crate) handed: Vec<(Instant, MidiMessage)>,
    pub(crate) played: Vec<MidiMessage>,
}

impl Cx {
    pub(crate) fn new(row: usize, now: Instant) -> Self {
        Self {
            row,
            input: String::new(),
            now,
            out: Vec::new(),
            requests: Vec::new(),
            handed: Vec::new(),
            played: Vec::new(),
        }
    }

    /// Names the input the surface runs on.
    pub(crate) fn on(mut self, input: &str) -> Self {
        self.input = input.to_owned();
        self
    }

    /// The name of the surface's input port: a controller's size can be in
    /// it.
    #[must_use]
    pub fn input(&self) -> &str {
        &self.input
    }

    /// The surface's row in the Control Surfaces table, from 0: where the
    /// read model keeps what it holds.
    #[must_use]
    pub const fn row(&self) -> usize {
        self.row
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
        self.requests.push(Ask::Command(Request {
            id: id.to_owned(),
            args,
            gesture,
        }));
    }

    /// Tells the shell a control moved: `role` is what it is for, `mode`
    /// how its values read and `hit` what this message was. What the role
    /// reaches (the parameter at its place in the bank, the ring's track)
    /// is resolved there as the message is run, so it is the one the person
    /// sees (R22, R33).
    pub fn control(&mut self, role: Role, mode: RemoteMode, hit: Hit) {
        self.requests
            .push(Ask::Control(Control { role, mode, hit }));
    }

    /// Tells the shell where an absolute control rests that the surface hasn't
    /// moved yet, as the position of its role's parameter in its range, so
    /// Takeover Mode knows where the controller is before its first message
    /// (R35).
    pub fn rest(&mut self, role: Role, value: u8) {
        self.requests.push(Ask::Rest(Control {
            role,
            mode: RemoteMode::Absolute,
            hit: Hit::Value {
                value: u16::from(value),
                full: 127,
            },
        }));
    }

    /// Ends the gesture on `control` where it has no last command to carry
    /// the release, as an encoder's touch being let go does (R7).
    pub fn end_gesture(&mut self, control: u32) {
        self.requests.push(Ask::EndGesture(control));
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

/// A surface the chooser offers: a native surface, or a controller template
/// (R43).
#[derive(Clone)]
pub struct Kind {
    /// The surface's ID, as the preferences and commands keep it.
    pub id: String,
    /// The name the chooser shows.
    pub name: String,
    /// Whether the surface needs an output to start (R9).
    pub needs_output: bool,
    /// Whether a device can be locked to the surface (R23).
    pub can_lock: bool,
    /// The size of the surface's ring, if it has one (R13).
    pub ring: Option<Ring>,
    /// The messages Dump sends the controller; none for a surface with no
    /// setup to send (R11).
    pub dump: Vec<Vec<u8>>,
    /// How the surface's controller is found when it is plugged in
    /// (supported controllers R2); none for a surface chosen by hand.
    pub recognise: Option<Recognise>,
    make: Arc<dyn Fn() -> Box<dyn ControlSurface> + Send + Sync>,
}

impl Kind {
    /// A surface the chooser offers, made by `make`.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        needs_output: bool,
        can_lock: bool,
        ring: Option<Ring>,
        dump: Vec<Vec<u8>>,
        make: impl Fn() -> Box<dyn ControlSurface> + Send + Sync + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            needs_output,
            can_lock,
            ring,
            dump,
            recognise: None,
            make: Arc::new(make),
        }
    }

    /// The surface, found by `recognise` when its controller is plugged in.
    #[must_use]
    pub fn recognised(self, recognise: Recognise) -> Self {
        Self {
            recognise: Some(recognise),
            ..self
        }
    }

    /// Makes the surface.
    #[must_use]
    pub fn make(&self) -> Box<dyn ControlSurface> {
        (self.make)()
    }
}

/// How a controller is recognised by its ports and its identity reply
/// (supported controllers R2, R72).
#[derive(Clone, Copy, Debug)]
pub struct Recognise {
    /// Text, in lower case, that the name of the controller's surface ports
    /// contains.
    pub port: &'static str,
    /// A match of its own for a controller whose ports go by more than one
    /// name, in place of `port`; given the name in lower case.
    pub test: Option<fn(&str) -> bool>,
    /// The bytes that follow the reply's header: the maker's, and where the
    /// maker puts the model in the same place, the model's.
    pub maker: &'static [u8],
    /// Whether the controller has two ports of this name, the first of which
    /// is its surface's: the one after a port that is taken, or being asked,
    /// isn't offered.
    pub pair: bool,
}

impl Recognise {
    /// Whether a port's name is one of the controller's surface ports,
    /// whatever the case (R72).
    #[must_use]
    pub fn names(&self, port: &str) -> bool {
        let port = port.to_lowercase();
        self.test
            .map_or_else(|| port.contains(self.port), |test| test(&port))
    }

    /// Whether `bytes` is a device inquiry's reply from the controller's
    /// maker.
    #[must_use]
    pub fn replies(&self, bytes: &[u8]) -> bool {
        bytes.len() > 5 + self.maker.len()
            && bytes[..5] == [0xF0, 0x7E, bytes[2], 0x06, 0x02]
            && bytes[5..].starts_with(self.maker)
    }
}

impl std::fmt::Debug for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Kind")
            .field("id", &self.id)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// Every native surface the chooser offers, by name (R1): the native
/// surfaces first in alphabetical order. The test surfaces
/// are there in test builds only (R16).
#[must_use]
pub fn kinds() -> &'static [Arc<Kind>] {
    static KINDS: std::sync::OnceLock<Vec<Arc<Kind>>> = std::sync::OnceLock::new();
    KINDS.get_or_init(|| {
        vec![
            Arc::new(crate::grid::apc_mini()),
            Arc::new(crate::grid::apc40()),
            Arc::new(crate::launchkey::mk3()),
            Arc::new(crate::launchkey::mk4()),
            Arc::new(crate::grid::mini()),
            Arc::new(crate::grid::pro()),
            Arc::new(crate::grid::launchpad_x()),
            #[cfg(feature = "testing")]
            Arc::new(crate::testing::kind()),
            #[cfg(feature = "testing")]
            Arc::new(crate::testing::instant()),
        ]
    })
}

/// The native surface with this ID.
#[must_use]
pub fn kind(id: &str) -> Option<Arc<Kind>> {
    kinds().iter().find(|kind| kind.id == id).cloned()
}

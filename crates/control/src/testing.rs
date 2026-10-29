//! The test surface (`docs/specs/control-surfaces.md`, R16): an 8 × 2 ring,
//! a setup to dump, and every call recorded, so every rule of the host runs
//! headless on test ports. Offered in test builds only, by the `testing`
//! feature.

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use bl_core::midi::MidiMessage;
use serde_json::Value;

use crate::surface::{ControlSurface, Cx, Kind, ReadModel, Ring, SurfaceState};

/// What the surface sends when it starts.
pub const HELLO: &[u8] = &[0xF0, 0x7D, 0x01, 0xF7];
/// What it sends when it stops.
pub const GOODBYE: &[u8] = &[0xF0, 0x7D, 0x02, 0xF7];
/// What Dump sends.
pub const SETUP: &[u8] = &[0xF0, 0x7D, 0x03, 0xF7];

/// The test surface as the chooser offers it.
pub const KIND: Kind = Kind {
    id: "test",
    name: "Test Surface",
    needs_output: true,
    can_lock: false,
    ring: Some(Ring {
        tracks: 8,
        scenes: 2,
    }),
    dump: &[SETUP],
    make,
};

fn make() -> Box<dyn ControlSurface> {
    Box::new(TestSurface::new().0)
}

/// One call the host made.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    /// `start`.
    Start,
    /// `stop`.
    Stop,
    /// `message`, with the time it arrived and its bytes.
    Message {
        /// When it arrived.
        at: Instant,
        /// The whole message.
        bytes: Vec<u8>,
    },
    /// `tick`, with how many tracks the read model's set had.
    Tick {
        /// The tracks.
        tracks: usize,
    },
}

/// What the surface does with a message besides lighting its echo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reply {
    /// Takes it: it goes no further.
    #[default]
    Take,
    /// Hands it back to the tracks (R4).
    HandBack,
    /// Plays each note-on it gets into the tracks as its own (R5).
    Play,
}

#[derive(Default)]
struct Inner {
    calls: Vec<Call>,
    reply: Reply,
    /// A command a controller message turns: its ID, and its arguments with
    /// each `"$"` replaced by the controller's value from 0 to 1.
    knob: Option<(String, Value)>,
    sleep: Duration,
    panic: bool,
    /// Sends each echo for this long after the message arrived, stamped
    /// with that moment (R8).
    stamp: Option<Duration>,
}

/// What the test surface recorded, and what a test tells it to do.
#[derive(Clone, Default)]
pub struct Journal(Arc<Mutex<Inner>>);

impl Journal {
    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Every call so far, in order.
    #[must_use]
    pub fn calls(&self) -> Vec<Call> {
        self.inner().calls.clone()
    }

    /// The messages it was given, with the time each arrived.
    #[must_use]
    pub fn messages(&self) -> Vec<(Instant, Vec<u8>)> {
        self.inner()
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::Message { at, bytes } => Some((*at, bytes.clone())),
                _ => None,
            })
            .collect()
    }

    /// How many ticks it has had.
    #[must_use]
    pub fn ticks(&self) -> usize {
        self.inner()
            .calls
            .iter()
            .filter(|call| matches!(call, Call::Tick { .. }))
            .count()
    }

    /// Sets what it does with a message besides lighting its echo.
    pub fn reply(&self, reply: Reply) {
        self.inner().reply = reply;
    }

    /// Makes a controller message (a control change) hold `command` with
    /// `args` as a gesture on that controller, and a note-off of the same
    /// number release it.
    pub fn knob(&self, command: &str, args: Value) {
        self.inner().knob = Some((command.to_owned(), args));
    }

    /// Makes every message take this long.
    pub fn sleep(&self, time: Duration) {
        self.inner().sleep = time;
    }

    /// Makes the surface stamp each echo for this long after the message
    /// arrived, so the port's backend sends it then.
    pub fn stamp(&self, after: Duration) {
        self.inner().stamp = Some(after);
    }

    /// Makes the next message panic.
    pub fn panic(&self) {
        self.inner().panic = true;
    }
}

/// The test surface.
pub struct TestSurface {
    journal: Journal,
    last: HashMap<u8, f64>,
}

impl TestSurface {
    /// A test surface, and the journal it records in.
    #[must_use]
    pub fn new() -> (Self, Journal) {
        let journal = Journal::default();
        let surface = Self {
            journal: journal.clone(),
            last: HashMap::new(),
        };
        (surface, journal)
    }
}

fn fill(args: &Value, value: f64) -> Value {
    match args {
        Value::String(s) if s == "$" => Value::from(value),
        Value::Array(items) => Value::Array(items.iter().map(|v| fill(v, value)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, v)| (key.clone(), fill(v, value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

impl ControlSurface for TestSurface {
    fn start(&mut self, cx: &mut Cx) {
        self.journal.inner().calls.push(Call::Start);
        cx.send(HELLO);
    }

    fn stop(&mut self, cx: &mut Cx) {
        self.journal.inner().calls.push(Call::Stop);
        cx.send(GOODBYE);
    }

    fn message(&mut self, at: Instant, bytes: &[u8], cx: &mut Cx) {
        let (reply, knob, sleep, panic, stamp) = {
            let mut inner = self.journal.inner();
            inner.calls.push(Call::Message {
                at,
                bytes: bytes.to_vec(),
            });
            (
                inner.reply,
                inner.knob.clone(),
                inner.sleep,
                std::mem::take(&mut inner.panic),
                inner.stamp,
            )
        };
        assert!(!panic, "the test surface was told to panic");
        if !sleep.is_zero() {
            std::thread::sleep(sleep);
        }
        // The lights: every message is echoed to the output.
        match stamp {
            Some(after) => cx.send_at(at + after, bytes),
            None => cx.send(bytes),
        }
        let message = MidiMessage::from_bytes(bytes);
        if let (Some((command, args)), Some(message)) = (&knob, message) {
            let control = message.data1();
            match message.status() & 0xF0 {
                0xB0 => {
                    let value = f64::from(message.data2()) / 127.0;
                    self.last.insert(control, value);
                    cx.run_held(u32::from(control), command, fill(args, value));
                }
                0x80 => {
                    let value = self.last.remove(&control).unwrap_or(0.0);
                    cx.run_released(u32::from(control), command, fill(args, value));
                }
                _ => {}
            }
        }
        match (reply, message) {
            (Reply::HandBack, Some(message)) => cx.hand_back(at, message),
            (Reply::Play, Some(message)) if message.is_note_on() => cx.play(message),
            _ => {}
        }
    }

    fn tick(&mut self, model: &ReadModel, _cx: &mut Cx) {
        self.journal.inner().calls.push(Call::Tick {
            tracks: model.document.tracks().count(),
        });
    }

    fn state(&self) -> SurfaceState {
        SurfaceState {
            ring: KIND.ring,
            mode: Some("Test".to_owned()),
        }
    }

    fn probe(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        Some(Arc::new(self.journal.clone()))
    }
}

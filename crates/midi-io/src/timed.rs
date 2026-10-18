//! Scheduled output for a MIDI system that can't schedule: a timer thread
//! holds each message until its moment and then sends it (`docs/specs/
//! external-devices.md`, R5). Windows Multimedia MIDI sends at once, so
//! `MidirBackend` wraps its outputs in this; the wrapper works over any
//! [`OutputConnection`], so tests on every platform use it too.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::backend::{MidiError, OutputConnection};

/// How long before a message's moment the thread stops sleeping and spins.
/// A sleep can end a little late, by a timer tick on some systems, so the
/// last stretch is waited out by polling the clock.
const SPIN: Duration = Duration::from_micros(1_500);

/// The shortest sleep worth taking. A short sleep can take far longer than
/// asked (the system lets timers of a normal thread run late to batch them),
/// so a moment less than this beyond the spin distance is spun for instead.
const MIN_SLEEP: Duration = Duration::from_micros(1_000);

/// The longest the thread sleeps in one go while a message waits. A message
/// that arrives with an earlier moment than the one waited for is noticed
/// within this.
const SLICE: Duration = Duration::from_millis(2);

/// A message waiting for its moment.
struct Pending {
    at: Instant,
    /// Arrival order, so messages with the same moment leave in the order
    /// they came.
    order: u64,
    bytes: Vec<u8>,
}

impl PartialEq for Pending {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Pending {}

impl PartialOrd for Pending {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Pending {
    /// Reversed, so the heap's greatest is the message due first.
    fn cmp(&self, other: &Self) -> Ordering {
        (other.at, other.order).cmp(&(self.at, self.order))
    }
}

#[derive(Default)]
struct State {
    waiting: BinaryHeap<Pending>,
    next_order: u64,
    stop: bool,
}

struct Shared {
    state: Mutex<State>,
    /// Signalled when a message arrives or the wrapper is dropped.
    wake: Condvar,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An output whose scheduled messages go out from a timer thread, on their
/// moment to within a millisecond.
///
/// [`send`](OutputConnection::send) passes straight to the inner connection.
/// [`send_at`](OutputConnection::send_at) queues the message for a thread
/// that sleeps until just before its moment, then polls the clock and sends
/// it as the moment arrives. Messages leave in the order of their moments,
/// and those with the same moment in the order they came. The inner
/// connection can't report a refusal of a message sent later, so such a
/// message is dropped silently. Dropping the wrapper stops the thread and
/// drops the messages still waiting.
///
/// ```
/// use std::time::{Duration, Instant};
/// use bl_midi_io::{Direction, MidiBackend, OutputConnection, TestBackend, TimedOutput};
///
/// let test = TestBackend::new();
/// test.add_port(Direction::Output, "Synth");
/// let mut backend = test.clone();
/// let mut out = TimedOutput::new(backend.open_output("Synth").unwrap());
/// let at = Instant::now() + Duration::from_millis(10);
/// out.send_at(&[0x90, 60, 100], at).unwrap();
/// drop(out); // would drop the message if it hadn't gone yet
/// ```
pub struct TimedOutput {
    inner: Arc<Mutex<Box<dyn OutputConnection>>>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl TimedOutput {
    /// Wraps `inner`, starting the timer thread. If the system can't start
    /// a thread, scheduled messages go out at once instead.
    #[must_use]
    pub fn new(inner: Box<dyn OutputConnection>) -> Self {
        let inner = Arc::new(Mutex::new(inner));
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
        });
        let thread = thread::Builder::new()
            .name("bl-midi-timer".to_owned())
            .spawn({
                let inner = Arc::clone(&inner);
                let shared = Arc::clone(&shared);
                move || run(&inner, &shared)
            })
            .ok();
        Self {
            inner,
            shared,
            thread,
        }
    }
}

impl OutputConnection for TimedOutput {
    fn send(&mut self, bytes: &[u8]) -> Result<(), MidiError> {
        // Through the timer's list, so a late message can't overtake one
        // still waiting there (R6's note-offs after their note-ons).
        self.send_at(bytes, Instant::now())
    }

    fn send_at(&mut self, bytes: &[u8], at: Instant) -> Result<(), MidiError> {
        if self.thread.is_none() {
            return lock(&self.inner).send(bytes);
        }
        // A moment already past is queued too: the thread sends it on its
        // next turn, and every message leaves in the one order.
        let mut state = lock(&self.shared.state);
        let order = state.next_order;
        state.next_order += 1;
        state.waiting.push(Pending {
            at,
            order,
            bytes: bytes.to_vec(),
        });
        drop(state);
        self.shared.wake.notify_one();
        Ok(())
    }
}

impl Drop for TimedOutput {
    fn drop(&mut self) {
        lock(&self.shared.state).stop = true;
        self.shared.wake.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The timer thread: sends each message as its moment comes.
fn run(inner: &Mutex<Box<dyn OutputConnection>>, shared: &Shared) {
    let mut state = lock(&shared.state);
    loop {
        if state.stop {
            return;
        }
        let Some(next) = state.waiting.peek() else {
            state = shared
                .wake
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
            continue;
        };
        let at = next.at;
        let remaining = at.saturating_duration_since(Instant::now());
        if remaining > SPIN + MIN_SLEEP {
            // Sleeping, not waiting on the condition variable with a
            // timeout: a sleep is the call the system times most finely
            // (a condition variable's timeout can round up to a timer tick
            // on Windows). The slice is short so a message due sooner is
            // seen soon.
            drop(state);
            thread::sleep((remaining - SPIN).min(SLICE));
            state = lock(&shared.state);
            continue;
        }
        // Within the spin distance: poll the clock without the lock, so a
        // message can still arrive meanwhile. The thread keeps its core
        // rather than yielding it: on a busy machine a yielded thread can
        // wait several milliseconds for its next turn.
        drop(state);
        while Instant::now() < at {
            std::hint::spin_loop();
        }
        state = lock(&shared.state);
        // Everything due now, earliest first.
        let now = Instant::now();
        let mut due = Vec::new();
        while state.waiting.peek().is_some_and(|p| p.at <= now) {
            due.extend(state.waiting.pop());
        }
        drop(state);
        {
            let mut inner = lock(inner);
            for message in due {
                let _ = inner.send(&message.bytes);
            }
        }
        state = lock(&shared.state);
    }
}

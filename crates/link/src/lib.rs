//! Ableton Link 4 sessions: tempo, phase, start and stop, and Link Audio.
//!
//! Link 4.1.1 is vendored as source under `vendor/` and built by `build.rs`;
//! [`ffi`] binds its C interface by hand. [`Link`] is the handle the main
//! thread holds: it switches Link on and off, shares start and stop, counts
//! the peers, and hears Link's own notifications as [`Event`]s on a channel,
//! never calling into anything else. [`Audio`] is the audio thread's handle,
//! with the session states it reads and writes made before the stream starts
//! (the Link spec, R12, R16): its calls allocate nothing and take no lock of
//! their own, except [`Audio::commit`], which ends in a condition variable's
//! signal.
//!
//! All times are microseconds on Link's own clock ([`Link::clock_micros`]).

mod ffi;

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

/// What Link tells the main thread, from one of its own threads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// The number of other peers in the session changed.
    Peers(u64),
    /// The session's tempo changed, whoever changed it, Beatlayer included.
    Tempo(f64),
    /// The session's start and stop state changed.
    StartStop(bool),
}

/// The Link instance and what its callbacks reach, freed together: the
/// instance first, so no callback runs after its sender is gone.
struct Inner {
    raw: ffi::AblLink,
    _senders: [*mut Sender<Event>; 3],
}

// SAFETY: Link's functions used here are thread-safe, as `abl_link.h` says of
// each, and the senders are only read by Link's threads while the instance
// lives; they are freed after it is destroyed.
unsafe impl Send for Inner {}
// SAFETY: as above.
unsafe impl Sync for Inner {}

impl Drop for Inner {
    fn drop(&mut self) {
        // SAFETY: `raw` was made by `abl_link_create` and is destroyed once.
        // Destroying it stops Link's threads, so nothing calls a callback
        // after it returns, and the senders are freed only then.
        unsafe {
            ffi::abl_link_destroy(self.raw);
            for sender in self._senders {
                drop(Box::from_raw(sender));
            }
        }
    }
}

/// The main thread's Link handle. Off when made: it doesn't talk on the
/// network until [`enable`](Self::enable).
pub struct Link {
    inner: Arc<Inner>,
    events: Receiver<Event>,
}

extern "C" fn on_peers(peers: u64, context: *mut c_void) {
    // SAFETY: `context` is the sender `Link::new` made for this callback,
    // alive until the instance is destroyed.
    let sender = unsafe { &*context.cast::<Sender<Event>>() };
    let _ = sender.send(Event::Peers(peers));
}

extern "C" fn on_tempo(bpm: f64, context: *mut c_void) {
    // SAFETY: as in `on_peers`.
    let sender = unsafe { &*context.cast::<Sender<Event>>() };
    let _ = sender.send(Event::Tempo(bpm));
}

extern "C" fn on_start_stop(playing: bool, context: *mut c_void) {
    // SAFETY: as in `on_peers`.
    let sender = unsafe { &*context.cast::<Sender<Event>>() };
    let _ = sender.send(Event::StartStop(playing));
}

impl Link {
    /// Makes a Link instance, off, whose session starts at `bpm`.
    #[must_use]
    pub fn new(bpm: f64) -> Self {
        let (sender, events) = channel();
        let senders = [(); 3].map(|()| Box::into_raw(Box::new(sender.clone())));
        // SAFETY: the instance is made here and owned by `Inner`; each
        // callback gets its own sender, which `Inner` frees after the
        // instance is destroyed.
        let raw = unsafe {
            let raw = ffi::abl_link_create(bpm);
            ffi::abl_link_set_num_peers_callback(raw, on_peers, senders[0].cast());
            ffi::abl_link_set_tempo_callback(raw, on_tempo, senders[1].cast());
            ffi::abl_link_set_start_stop_callback(raw, on_start_stop, senders[2].cast());
            raw
        };
        Self {
            inner: Arc::new(Inner {
                raw,
                _senders: senders,
            }),
            events,
        }
    }

    /// What Link has told the main thread since the last call, oldest first.
    pub fn events(&self) -> impl Iterator<Item = Event> + '_ {
        self.events.try_iter()
    }

    /// Whether Link talks on the network.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        // SAFETY: thread-safe and real-time-safe, as `abl_link.h` says.
        unsafe { ffi::abl_link_is_enabled(self.inner.raw) }
    }

    /// Switches Link on or off. Switching it on with `bpm` makes the tempo a
    /// session with no peer starts at; one that exists is joined at its own.
    pub fn enable(&self, on: bool, bpm: f64) {
        if on && !self.is_enabled() {
            self.set_tempo(bpm);
        }
        // SAFETY: thread-safe, and not real-time-safe: this is the main
        // thread's.
        unsafe { ffi::abl_link_enable(self.inner.raw, on) };
    }

    /// Whether start and stop are shared with the peers that share them.
    #[must_use]
    pub fn start_stop_sync(&self) -> bool {
        // SAFETY: thread-safe and real-time-safe.
        unsafe { ffi::abl_link_is_start_stop_sync_enabled(self.inner.raw) }
    }

    /// Switches Start Stop Sync.
    pub fn set_start_stop_sync(&self, on: bool) {
        // SAFETY: thread-safe and real-time-safe.
        unsafe { ffi::abl_link_enable_start_stop_sync(self.inner.raw, on) };
    }

    /// How many other peers are in the session.
    #[must_use]
    pub fn peers(&self) -> u64 {
        // SAFETY: thread-safe and real-time-safe.
        unsafe { ffi::abl_link_num_peers(self.inner.raw) }
    }

    /// Link's clock, in microseconds.
    #[must_use]
    pub fn clock_micros(&self) -> i64 {
        // SAFETY: thread-safe and real-time-safe.
        unsafe { ffi::abl_link_clock_micros(self.inner.raw) }
    }

    /// Sets the session's tempo from the main thread, for a session whose
    /// audio thread isn't running: with the stream going, the audio thread
    /// writes it ([`Audio::set_tempo`]).
    pub fn set_tempo(&self, bpm: f64) {
        let state = State::new();
        // SAFETY: the state is this call's own; the app-thread pair is
        // thread-safe.
        unsafe {
            ffi::abl_link_capture_app_session_state(self.inner.raw, state.0);
            ffi::abl_link_set_tempo(state.0, bpm, self.clock_micros());
            ffi::abl_link_commit_app_session_state(self.inner.raw, state.0);
        }
    }

    /// The session as the main thread sees it now, for display and for the
    /// agent API. The quantum is in quarter notes.
    #[must_use]
    pub fn snapshot(&self, quantum: f64) -> Snapshot {
        let state = State::new();
        let now = self.clock_micros();
        // SAFETY: the state is this call's own.
        unsafe {
            ffi::abl_link_capture_app_session_state(self.inner.raw, state.0);
            Snapshot {
                tempo: ffi::abl_link_tempo(state.0),
                playing: ffi::abl_link_is_playing(state.0),
                phase: ffi::abl_link_phase_at_time(state.0, now, quantum),
            }
        }
    }

    /// A handle that makes audio-thread handles, which the audio host keeps
    /// and which keeps the instance alive.
    #[must_use]
    pub fn handle(&self) -> Handle {
        Handle(Arc::clone(&self.inner))
    }

    /// Makes the audio thread's handle, with its session state, before the
    /// stream starts. Each handle writes to the one session.
    #[must_use]
    pub fn audio(&self) -> Audio {
        self.handle().audio()
    }
}

/// What makes [`Audio`] handles: the instance, shared with whoever starts
/// audio streams. It reads nothing of the session itself.
#[derive(Clone)]
pub struct Handle(Arc<Inner>);

impl Handle {
    /// Makes the audio thread's handle, with its session state: call it off
    /// the audio thread, before the stream starts.
    #[must_use]
    pub fn audio(&self) -> Audio {
        Audio {
            link: Arc::clone(&self.0),
            state: State::new(),
        }
    }
}

/// What [`Link::snapshot`] reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snapshot {
    /// The session's tempo, in BPM.
    pub tempo: f64,
    /// Whether the session's start and stop state is playing.
    pub playing: bool,
    /// The session's phase within the quantum now, in quarter notes.
    pub phase: f64,
}

/// A session state, freed when dropped.
struct State(ffi::AblSessionState);

// SAFETY: a session state is only used by one thread at a time: its owner's.
unsafe impl Send for State {}

impl State {
    fn new() -> Self {
        // SAFETY: allocates; the state is freed by `Drop`.
        Self(unsafe { ffi::abl_link_create_session_state() })
    }
}

impl Drop for State {
    fn drop(&mut self) {
        // SAFETY: made by `abl_link_create_session_state`, freed once.
        unsafe { ffi::abl_link_destroy_session_state(self.0) };
    }
}

/// The audio thread's Link handle: one session state, [captured](Self::capture)
/// once per block, read, changed, and [committed](Self::commit) only when a
/// block changed something (the Link spec, R12). It keeps the instance alive.
pub struct Audio {
    link: Arc<Inner>,
    state: State,
}

impl Audio {
    /// Whether Link is on. Real-time-safe.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        // SAFETY: thread-safe and real-time-safe.
        unsafe { ffi::abl_link_is_enabled(self.link.raw) }
    }

    /// Whether Start Stop Sync is on. Real-time-safe.
    #[must_use]
    pub fn start_stop_sync(&self) -> bool {
        // SAFETY: thread-safe and real-time-safe.
        unsafe { ffi::abl_link_is_start_stop_sync_enabled(self.link.raw) }
    }

    /// How many other peers are in the session. Real-time-safe.
    #[must_use]
    pub fn peers(&self) -> u64 {
        // SAFETY: thread-safe and real-time-safe.
        unsafe { ffi::abl_link_num_peers(self.link.raw) }
    }

    /// Link's clock, in microseconds. Real-time-safe.
    #[must_use]
    pub fn clock_micros(&self) -> i64 {
        // SAFETY: thread-safe and real-time-safe.
        unsafe { ffi::abl_link_clock_micros(self.link.raw) }
    }

    /// Takes a snapshot of the session into the handle's state: no
    /// allocation, no lock.
    pub fn capture(&mut self) {
        // SAFETY: audio thread only, which `&mut self` on the one handle
        // and its owner's thread give; real-time-safe.
        unsafe { ffi::abl_link_capture_audio_session_state(self.link.raw, self.state.0) };
    }

    /// Writes the state to the session. Allocation-free, but it ends in a
    /// condition variable's signal, so a block calls it only when it
    /// changed something.
    pub fn commit(&mut self) {
        // SAFETY: as `capture`.
        unsafe { ffi::abl_link_commit_audio_session_state(self.link.raw, self.state.0) };
    }

    /// The captured tempo, in BPM.
    #[must_use]
    pub fn tempo(&self) -> f64 {
        // SAFETY: reads the handle's own state.
        unsafe { ffi::abl_link_tempo(self.state.0) }
    }

    /// Sets the captured tempo, taking effect at `at` microseconds.
    pub fn set_tempo(&mut self, bpm: f64, at: i64) {
        // SAFETY: changes the handle's own state.
        unsafe { ffi::abl_link_set_tempo(self.state.0, bpm, at) };
    }

    /// The captured session's phase at `time` within `quantum` quarter
    /// notes, in `[0, quantum)`.
    #[must_use]
    pub fn phase_at_time(&self, time: i64, quantum: f64) -> f64 {
        // SAFETY: reads the handle's own state.
        unsafe { ffi::abl_link_phase_at_time(self.state.0, time, quantum) }
    }

    /// The captured session's beat at `time`.
    #[must_use]
    pub fn beat_at_time(&self, time: i64, quantum: f64) -> f64 {
        // SAFETY: reads the handle's own state.
        unsafe { ffi::abl_link_beat_at_time(self.state.0, time, quantum) }
    }

    /// The time at which the captured session reaches `beat`.
    #[must_use]
    pub fn time_at_beat(&self, beat: f64, quantum: f64) -> i64 {
        // SAFETY: reads the handle's own state.
        unsafe { ffi::abl_link_time_at_beat(self.state.0, beat, quantum) }
    }

    /// Maps `beat` to `time` as a launch: at once when no peer is in the
    /// session, else at the next time of the same phase.
    pub fn request_beat_at_time(&mut self, beat: f64, time: i64, quantum: f64) {
        // SAFETY: changes the handle's own state.
        unsafe { ffi::abl_link_request_beat_at_time(self.state.0, beat, time, quantum) };
    }

    /// Whether the captured start and stop state is playing.
    #[must_use]
    pub fn is_playing(&self) -> bool {
        // SAFETY: reads the handle's own state.
        unsafe { ffi::abl_link_is_playing(self.state.0) }
    }

    /// When the captured start or stop takes effect.
    #[must_use]
    pub fn time_for_is_playing(&self) -> i64 {
        // SAFETY: reads the handle's own state.
        unsafe { ffi::abl_link_time_for_is_playing(self.state.0) }
    }

    /// Sets the captured start and stop state, taking effect at `at`.
    pub fn set_is_playing(&mut self, playing: bool, at: i64) {
        // SAFETY: changes the handle's own state.
        unsafe { ffi::abl_link_set_is_playing(self.state.0, playing, at) };
    }
}

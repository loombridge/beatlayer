//! The control-surface host: one thread that runs every surface, a watchdog
//! that stops a stuck one, and the main thread's handle on both
//! (`docs/specs/control-surfaces.md`, R3 to R10 and R16).
//!
//! The host thread reads three things: its event queue (messages from the
//! surfaces' inputs, and the main thread's requests to start or stop one),
//! the read model the main thread last published, and the clock. It writes
//! to the surfaces' outputs straight through the output bus, to the engine's
//! input queue for what a surface hands or plays to tracks, and to a list of
//! commands the main thread runs. Nothing here runs on the audio thread.

use std::any::Any;
use std::collections::{HashSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{
    AtomicBool, AtomicU64, Ordering::AcqRel, Ordering::Acquire, Ordering::Relaxed,
};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::thread;
use std::time::{Duration, Instant};

use bl_core::clock::TestClock;
use bl_core::midi::{MidiInputEvent, MidiSource, PortId};
use bl_midi_io::{Direction, InputLive, InputSender, MidiPorts, OutputBus};

use crate::surface::{Ask, ControlSurface, Cx, Kind, Out, ReadModel, SurfaceState, TICK, kinds};

/// How many rows the Control Surfaces table has (R1).
pub const ROWS: usize = 6;

/// How long a call may take before the surface is stopped (R10).
const WATCHDOG: Duration = Duration::from_millis(100);

/// How long the main thread waits for the host to start or stop a surface.
const WAIT: Duration = Duration::from_secs(1);

/// How many messages a surface's port takes in one tick: more than that wait
/// for the next one (R8).
const SEND_PER_TICK: usize = 512;

/// How many messages may wait for the port before the oldest ones for the
/// same control are dropped (R8).
const PENDING_MAX: usize = 2048;

/// One row of the Control Surfaces table: the surface and the ports it talks
/// on, by name (R1, R2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowConfig {
    /// The surface's ID.
    pub surface: Option<String>,
    /// The input's name.
    pub input: Option<String>,
    /// The output's name.
    pub output: Option<String>,
}

/// Whether a row's surface runs (R13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    /// The row names no surface.
    Empty,
    /// The surface waits for a port to appear.
    Waiting,
    /// The surface runs.
    Running,
    /// The surface is stopped, and the row says why.
    Stopped,
}

/// A row as `control_surface.list` and the Settings table show it (R13).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowReport {
    /// What the row is set to.
    pub config: RowConfig,
    /// The surface's name, if the row has one the chooser offers.
    pub surface_name: Option<String>,
    /// Whether the input is present; `None` for no input.
    pub input_present: Option<bool>,
    /// Whether the output is present; `None` for no output.
    pub output_present: Option<bool>,
    /// What the surface is doing.
    pub run: RunState,
    /// Why it is stopped.
    pub error: Option<String>,
    /// Whether Dump is enabled: the surface has a setup to send and its
    /// output is present (R11).
    pub can_dump: bool,
    /// What a running surface reports of itself.
    pub state: Option<SurfaceState>,
}

/// What a surface asked for, and the row it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandRequest {
    /// The row, from 0.
    pub row: usize,
    /// The command, or the control that moved.
    pub ask: Ask,
}

impl CommandRequest {
    /// The command, if this asks for one.
    #[must_use]
    pub const fn command(&self) -> Option<&crate::surface::Request> {
        match &self.ask {
            Ask::Command(request) => Some(request),
            Ask::Control(_) | Ask::Rest(_) | Ask::EndGesture(_) => None,
        }
    }
}

/// How many messages surfaces took and sent: the remote indicators light
/// when either moves (R15).
#[derive(Debug, Default)]
pub struct Traffic {
    /// Messages a surface took from its input.
    pub taken: AtomicU64,
    /// Messages sent to a surface's output.
    pub sent: AtomicU64,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the main thread asks the host thread, and what inputs bring it.
enum Event {
    Message {
        row: usize,
        session: u64,
        at: Instant,
        bytes: Vec<u8>,
    },
    Start {
        row: usize,
        session: u64,
        surface: Box<dyn ControlSurface>,
        link: Link,
        ack: Sender<()>,
    },
    Stop {
        row: usize,
        ack: Sender<()>,
    },
    Tick(Sender<()>),
    Barrier(Sender<()>),
    Shutdown(Sender<()>),
}

/// The ports a running surface talks on.
struct Link {
    /// The input's name, which a surface reads to tell the controller's
    /// size apart.
    name: String,
    input: PortId,
    live: Arc<InputLive>,
    output: Option<PortId>,
}

struct Running {
    surface: Box<dyn ControlSurface>,
    session: u64,
    link: Link,
    /// Messages waiting for the port (R8).
    pending: VecDeque<Out>,
}

/// A row's place in the host, shared with the main thread.
#[derive(Default)]
struct Slot {
    running: Mutex<Option<Running>>,
    info: Mutex<Info>,
}

#[derive(Default)]
struct Info {
    running: bool,
    error: Option<String>,
    state: SurfaceState,
    probe: Option<Arc<dyn Any + Send + Sync>>,
}

/// Why a call ended the surface.
enum Fault {
    Panicked(String),
    TooSlow,
    /// The watchdog gave up on the call and a new host thread took over.
    Abandoned,
}

struct Shared {
    sender: InputSender,
    output: OutputBus,
    traffic: Arc<Traffic>,
    rx: Mutex<Receiver<Event>>,
    slots: Mutex<Vec<Arc<Slot>>>,
    model: Mutex<Option<Arc<ReadModel>>>,
    requests: Mutex<Vec<CommandRequest>>,
    /// The call under way: its row, the thread's generation and its start.
    call: Mutex<Option<(usize, u64, Instant)>>,
    /// Which host thread is the current one. The watchdog moves it on when
    /// it gives up on a stuck call, so the stuck thread ends when it
    /// returns.
    generation: AtomicU64,
    shutdown: AtomicBool,
    clock: Mutex<Option<TestClock>>,
}

impl Shared {
    fn now(&self) -> Instant {
        lock(&self.clock)
            .as_ref()
            .map_or_else(Instant::now, TestClock::now)
    }

    fn slot(&self, row: usize) -> Arc<Slot> {
        Arc::clone(&lock(&self.slots)[row])
    }

    /// Runs `f` on the surface, guarded: a panic, or a call that takes
    /// longer than the watchdog allows, is a fault (R10).
    fn guarded<R>(
        &self,
        row: usize,
        generation: u64,
        running: &mut Running,
        f: impl FnOnce(&mut dyn ControlSurface, &mut Cx) -> R,
    ) -> Result<(R, Cx), Fault> {
        let started = Instant::now();
        *lock(&self.call) = Some((row, generation, started));
        let mut cx = Cx::new(row, self.now()).on(&running.link.name);
        let result = catch_unwind(AssertUnwindSafe(|| f(running.surface.as_mut(), &mut cx)));
        {
            let mut call = lock(&self.call);
            if call.is_some_and(|(_, current, _)| current == generation) {
                *call = None;
            }
        }
        if self.generation.load(Acquire) != generation {
            return Err(Fault::Abandoned);
        }
        match result {
            Err(payload) => Err(Fault::Panicked(describe(payload.as_ref()))),
            Ok(_) if started.elapsed() > WATCHDOG => Err(Fault::TooSlow),
            Ok(result) => Ok((result, cx)),
        }
    }

    /// Sends what a call produced: the messages for the output, the
    /// commands for the main thread, and what reaches tracks (R4, R5, R7,
    /// R8).
    fn deliver(&self, row: usize, running: &mut Running, cx: Cx) {
        let switches = running.link.live.switches();
        let source = if switches.mpe {
            MidiSource::MpePort(running.link.input)
        } else {
            MidiSource::Port(running.link.input)
        };
        if switches.track {
            for (time, message) in cx.handed {
                self.sender.send(MidiInputEvent {
                    source,
                    time,
                    message,
                });
            }
        }
        let now = self.now();
        for (at, message) in cx.played {
            self.sender.send(MidiInputEvent {
                source,
                time: at.unwrap_or(now),
                message,
            });
        }
        if !cx.requests.is_empty() {
            lock(&self.requests).extend(
                cx.requests
                    .into_iter()
                    .map(|ask| CommandRequest { row, ask }),
            );
        }
        running.pending.extend(cx.out);
        self.flush(running);
    }

    /// Sends the messages waiting for the port, in order, as many as it
    /// takes in a tick; when more wait than may, the oldest for the same
    /// control go first (R8).
    fn flush(&self, running: &mut Running) {
        let Some(port) = running.link.output else {
            running.pending.clear();
            return;
        };
        if running.pending.len() > PENDING_MAX {
            thin(&mut running.pending);
        }
        for _ in 0..SEND_PER_TICK {
            let Some(out) = running.pending.pop_front() else {
                break;
            };
            if self.output.send_to(port, &out.bytes, out.at).is_ok() {
                self.traffic.sent.fetch_add(1, Relaxed);
            }
        }
    }

    /// Calls the surface in `slot`, and delivers its answer. A fault stops
    /// it, with the reason in its row. Returns `false` if this thread has
    /// been given up on and must end.
    fn call(
        &self,
        row: usize,
        generation: u64,
        slot: &Slot,
        running: &mut Option<Running>,
        f: impl FnOnce(&mut dyn ControlSurface, &mut Cx),
    ) -> bool {
        let Some(live) = running.as_mut() else {
            return true;
        };
        match self.guarded(row, generation, live, f) {
            Ok(((), cx)) => {
                self.deliver(row, live, cx);
                true
            }
            Err(Fault::Abandoned) => false,
            Err(fault) => {
                lock(&slot.info).error = Some(match fault {
                    Fault::Panicked(why) => format!("it panicked: {why}"),
                    _ => format!("a call took longer than {} ms", WATCHDOG.as_millis()),
                });
                self.stop_row(row, generation, slot, running)
            }
        }
    }

    /// Stops the surface in `slot`: its stop sends the goodbye, and
    /// everything it sent goes out before this returns.
    fn stop_row(
        &self,
        row: usize,
        generation: u64,
        slot: &Slot,
        running: &mut Option<Running>,
    ) -> bool {
        let Some(mut live) = running.take() else {
            return true;
        };
        match self.guarded(row, generation, &mut live, |surface, cx| surface.stop(cx)) {
            Ok(((), cx)) => {
                self.deliver(row, &mut live, cx);
                while !live.pending.is_empty() {
                    self.flush(&mut live);
                }
            }
            Err(Fault::Abandoned) => return false,
            Err(_) => {}
        }
        let mut info = lock(&slot.info);
        info.running = false;
        info.state = SurfaceState::default();
        true
    }

    fn handle(&self, event: Event, generation: u64) -> bool {
        match event {
            Event::Message {
                row,
                session,
                at,
                bytes,
            } => {
                let slot = self.slot(row);
                let mut running = lock(&slot.running);
                if running.as_ref().is_none_or(|r| r.session != session) {
                    return true;
                }
                self.traffic.taken.fetch_add(1, Relaxed);
                self.call(row, generation, &slot, &mut running, |surface, cx| {
                    surface.message(at, &bytes, cx);
                })
            }
            Event::Start {
                row,
                session,
                surface,
                link,
                ack,
            } => {
                let slot = self.slot(row);
                let mut running = lock(&slot.running);
                *running = Some(Running {
                    surface,
                    session,
                    link,
                    pending: VecDeque::new(),
                });
                {
                    let mut info = lock(&slot.info);
                    info.error = None;
                    info.running = true;
                }
                let (mut state, mut probe) = (None, None);
                let alive = self.call(row, generation, &slot, &mut running, |surface, cx| {
                    surface.start(cx);
                    state = Some(surface.state());
                    probe = surface.probe();
                });
                let mut info = lock(&slot.info);
                if let Some(state) = state {
                    info.state = state;
                }
                info.probe = probe;
                let _ = ack.send(());
                alive
            }
            Event::Stop { row, ack } => {
                let slot = self.slot(row);
                let alive = self.stop_row(row, generation, &slot, &mut lock(&slot.running));
                let _ = ack.send(());
                alive
            }
            Event::Tick(ack) => {
                let alive = self.tick(generation);
                let _ = ack.send(());
                alive
            }
            Event::Barrier(ack) => {
                let _ = ack.send(());
                true
            }
            Event::Shutdown(ack) => {
                self.shutdown.store(true, Relaxed);
                for row in 0..ROWS {
                    let slot = self.slot(row);
                    if !self.stop_row(row, generation, &slot, &mut lock(&slot.running)) {
                        break;
                    }
                }
                let _ = ack.send(());
                false
            }
        }
    }

    /// The tick of every running surface (R6).
    fn tick(&self, generation: u64) -> bool {
        let model = lock(&self.model).clone();
        for row in 0..ROWS {
            let slot = self.slot(row);
            let mut running = lock(&slot.running);
            let Some(live) = running.as_mut() else {
                continue;
            };
            self.flush(live);
            let Some(model) = &model else {
                continue;
            };
            let mut state = None;
            if !self.call(row, generation, &slot, &mut running, |surface, cx| {
                surface.tick(model, cx);
                state = Some(surface.state());
            }) {
                return false;
            }
            if let Some(state) = state {
                lock(&slot.info).state = state;
            }
        }
        true
    }

    /// The watchdog's verdict on a call that has gone on too long: the row
    /// reads its error, and a new host thread takes the other surfaces on
    /// (R10).
    fn condemn(self: &Arc<Self>, row: usize, generation: u64) {
        if self
            .generation
            .compare_exchange(generation, generation + 1, AcqRel, Acquire)
            .is_err()
        {
            return;
        }
        let fresh = Slot::default();
        lock(&fresh.info).error = Some(format!(
            "it didn't return within {} ms",
            WATCHDOG.as_millis()
        ));
        lock(&self.slots)[row] = Arc::new(fresh);
        *lock(&self.call) = None;
        spawn(Arc::clone(self), generation + 1);
    }
}

/// Drops the messages a newer one for the same control makes stale, and then
/// the oldest of the rest, until the queue is a half of what it may be.
fn thin(pending: &mut VecDeque<Out>) {
    let mut seen = HashSet::new();
    let mut keep: Vec<bool> = pending
        .iter()
        .rev()
        .map(|out| out.control.is_none_or(|control| seen.insert(control)))
        .collect();
    keep.reverse();
    let mut keep = keep.into_iter();
    pending.retain(|_| keep.next().unwrap_or(true));
    let excess = pending.len().saturating_sub(PENDING_MAX / 2);
    pending.drain(..excess);
}

fn describe(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "no reason given".to_owned())
}

/// Starts a host thread of this generation.
fn spawn(shared: Arc<Shared>, generation: u64) {
    let _ = thread::Builder::new()
        .name("bl-control-surfaces".to_owned())
        .spawn(move || host_loop(&shared, generation));
}

fn host_loop(shared: &Arc<Shared>, generation: u64) {
    let mut next = Instant::now() + TICK;
    while shared.generation.load(Acquire) == generation {
        let wait = next.saturating_duration_since(Instant::now());
        let received = lock(&shared.rx).recv_timeout(wait);
        let alive = match received {
            Ok(event) => shared.handle(event, generation),
            Err(RecvTimeoutError::Timeout) => {
                next = Instant::now() + TICK;
                shared.tick(generation)
            }
            Err(RecvTimeoutError::Disconnected) => return,
        };
        if !alive {
            return;
        }
    }
}

/// Watches the call under way, and gives up on one that takes too long.
fn watchdog(shared: &Weak<Shared>) {
    loop {
        thread::sleep(WATCHDOG / 10);
        let Some(shared) = shared.upgrade() else {
            return;
        };
        if shared.shutdown.load(Relaxed) {
            return;
        }
        let stuck = *lock(&shared.call);
        if let Some((row, generation, since)) = stuck
            && since.elapsed() >= WATCHDOG
        {
            shared.condemn(row, generation);
        }
    }
}

/// The main thread's handle on the host thread.
struct Host {
    shared: Arc<Shared>,
    tx: Sender<Event>,
}

impl Host {
    fn new(sender: InputSender, output: OutputBus, traffic: Arc<Traffic>) -> Self {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            sender,
            output,
            traffic,
            rx: Mutex::new(rx),
            slots: Mutex::new((0..ROWS).map(|_| Arc::new(Slot::default())).collect()),
            model: Mutex::new(None),
            requests: Mutex::new(Vec::new()),
            call: Mutex::new(None),
            generation: AtomicU64::new(0),
            shutdown: AtomicBool::new(false),
            clock: Mutex::new(None),
        });
        spawn(Arc::clone(&shared), 0);
        let weak = Arc::downgrade(&shared);
        let _ = thread::Builder::new()
            .name("bl-control-watchdog".to_owned())
            .spawn(move || watchdog(&weak));
        Self { shared, tx }
    }

    /// Sends an event that carries a reply, and waits for it.
    fn ask(&self, event: impl FnOnce(Sender<()>) -> Event) {
        let (ack, wait) = mpsc::channel();
        if self.tx.send(event(ack)).is_ok() {
            let _ = wait.recv_timeout(WAIT);
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        // Every surface stops, and sends its goodbye (R9).
        self.ask(Event::Shutdown);
    }
}

/// How long a controller has to answer the device inquiry before it is left
/// to be chosen by hand (supported controllers R2).
const PROBE_WAIT: Duration = Duration::from_secs(2);

/// The universal device inquiry (supported controllers R2).
const INQUIRY: [u8; 6] = [0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7];

/// A controller whose ports appeared, asked for its identity.
struct Probe {
    kind: Arc<Kind>,
    input: String,
    output: String,
    replies: Arc<Mutex<Vec<Vec<u8>>>>,
    since: Instant,
}

/// What the main thread keeps of a surface it started: the row it started
/// with.
struct Started {
    config: RowConfig,
}

/// What the main thread holds open for a row (R3).
#[derive(Default)]
struct Held {
    input: Option<String>,
    output: Option<String>,
    live: Option<Arc<InputLive>>,
}

/// The six rows of the Control Surfaces table and the host that runs their
/// surfaces: the main thread's side (R1 to R14).
///
/// Nothing here starts a thread until a row has a surface that can start.
pub struct Surfaces {
    sender: InputSender,
    output: OutputBus,
    clock: Option<TestClock>,
    traffic: Arc<Traffic>,
    host: Option<Host>,
    rows: [RowConfig; ROWS],
    held: [Held; ROWS],
    started: [Option<Started>; ROWS],
    /// Rows whose surface stopped with an error, and stay stopped until the
    /// row is chosen again (R10).
    halted: [bool; ROWS],
    sessions: u64,
    /// Controllers asked for their identity, waiting for the answer.
    probes: Vec<Probe>,
    /// Inputs a person took off a row, which stay off while Beatlayer runs
    /// (supported controllers R2).
    declined: HashSet<String>,
    /// Inputs whose controller didn't answer, until the port goes.
    silent: HashSet<String>,
    /// Whether a row was filled since the last call of [`Surfaces::filled`].
    filled: bool,
    /// The controller templates the chooser offers after the native
    /// surfaces (R43).
    templates: Vec<Arc<Kind>>,
    /// Native surfaces that live in crates above this one, such as Push 2's,
    /// which the chooser lists with the native ones (R1).
    natives: Vec<Arc<Kind>>,
}

impl Surfaces {
    /// Rows that name no surface, which tracks' input `sender` and the
    /// output `output` the surfaces will use.
    #[must_use]
    pub fn new(sender: InputSender, output: OutputBus) -> Self {
        Self {
            sender,
            output,
            clock: None,
            traffic: Arc::new(Traffic::default()),
            host: None,
            rows: Default::default(),
            held: Default::default(),
            started: Default::default(),
            halted: [false; ROWS],
            sessions: 0,
            probes: Vec::new(),
            declined: HashSet::new(),
            silent: HashSet::new(),
            filled: false,
            templates: Vec::new(),
            natives: Vec::new(),
        }
    }

    /// Sets the native surfaces that live in crates above this one (R1).
    pub fn set_natives(&mut self, natives: Vec<Arc<Kind>>) {
        self.natives = natives;
    }

    /// Sets the controller templates the chooser offers (R43). A row that
    /// names one that is gone keeps its name and stops, as a row does for a
    /// missing port (R44).
    pub fn set_templates(&mut self, templates: Vec<Arc<Kind>>) {
        self.templates = templates;
    }

    /// The surfaces the chooser offers: the native ones, then the templates
    /// (R1).
    #[must_use]
    pub fn kinds(&self) -> Vec<Arc<Kind>> {
        // The native surfaces in alphabetical order, those of this crate and
        // those of the crates above it together.
        let mut natives: Vec<Arc<Kind>> = kinds().iter().chain(&self.natives).cloned().collect();
        natives.sort_by(|a, b| a.id.cmp(&b.id));
        natives.extend(self.templates.iter().cloned());
        natives
    }

    /// The surface with this ID, native or a template.
    #[must_use]
    pub fn kind(&self, id: &str) -> Option<Arc<Kind>> {
        kinds()
            .iter()
            .chain(&self.natives)
            .chain(&self.templates)
            .find(|kind| kind.id == id)
            .cloned()
    }

    /// Stamps what surfaces play from `clock` instead of the system's, for
    /// tests that drive the engine by hand.
    pub fn set_clock(&mut self, clock: TestClock) {
        if let Some(host) = &self.host {
            *lock(&host.shared.clock) = Some(clock.clone());
        }
        self.clock = Some(clock);
    }

    /// What each row is set to.
    #[must_use]
    pub const fn rows(&self) -> &[RowConfig; ROWS] {
        &self.rows
    }

    /// Sets a row (R1, R3). Choosing a surface again, even the same one,
    /// restarts it if it stopped with an error (R10).
    ///
    /// # Errors
    ///
    /// A message if the surface isn't one the chooser offers, or a port is
    /// another row's, naming that row.
    pub fn set_row(&mut self, row: usize, config: RowConfig) -> Result<(), String> {
        if let Some(id) = &config.surface
            && self.kind(id).is_none()
        {
            return Err(format!("no control surface has the ID \"{id}\""));
        }
        self.restore(row, config)
    }

    /// Sets a row from the preferences: the same as [`Surfaces::set_row`],
    /// except that a surface this build doesn't offer is kept by its ID, as
    /// a port that isn't present is kept by its name (R2); the row reads
    /// stopped until the surface is there.
    ///
    /// # Errors
    ///
    /// A message if a port is another row's, naming that row.
    pub fn restore(&mut self, row: usize, config: RowConfig) -> Result<(), String> {
        // A port serves one row, an input and an output apart.
        let taken = |name: &Option<String>, of: fn(&RowConfig) -> &Option<String>| {
            let name = name.as_ref()?;
            let other =
                (0..ROWS).find(|&n| n != row && of(&self.rows[n]).as_ref() == Some(name))?;
            Some((name.clone(), other))
        };
        if let Some((name, other)) =
            taken(&config.input, |c| &c.input).or_else(|| taken(&config.output, |c| &c.output))
        {
            return Err(format!(
                "the port \"{name}\" is already used by row {}",
                other + 1
            ));
        }
        // A row a person emptied isn't filled again for its ports.
        if config.surface.is_none()
            && self.rows[row].surface.is_some()
            && let Some(input) = self.rows[row].input.clone()
        {
            self.declined.insert(input);
        }
        self.rows[row] = config;
        self.halted[row] = false;
        Ok(())
    }

    /// Whether a controller was asked for its identity and hasn't answered.
    #[must_use]
    pub fn probing(&self) -> bool {
        !self.probes.is_empty()
    }

    /// Whether a row was filled by a controller's being recognised since the
    /// last call, so the preferences keep it.
    pub fn filled(&mut self) -> bool {
        std::mem::take(&mut self.filled)
    }

    fn named(&self, name: &str) -> bool {
        self.rows
            .iter()
            .any(|r| r.input.as_deref() == Some(name) || r.output.as_deref() == Some(name))
    }

    /// Finds the controllers whose ports appeared (supported controllers
    /// R2): asks the ones no row names for their identity with the MIDI
    /// device inquiry, and fills the first free row with each whose reply
    /// names its maker.
    fn recognise(&mut self, ports: &mut MidiPorts) {
        let present = |ports: &MidiPorts, direction, name: &str| {
            ports.port(direction, name).is_some_and(|port| port.present)
        };
        for probe in std::mem::take(&mut self.probes) {
            let answered = lock(&probe.replies)
                .iter()
                .any(|reply| probe.kind.recognise.is_some_and(|r| r.replies(reply)));
            let gone = !present(ports, Direction::Input, &probe.input)
                || !present(ports, Direction::Output, &probe.output);
            if !(answered || gone) && probe.since.elapsed() < PROBE_WAIT {
                self.probes.push(probe);
                continue;
            }
            ports.release(Direction::Input, &probe.input);
            ports.release(Direction::Output, &probe.output);
            if answered
                && !gone
                && !self.named(&probe.input)
                && let Some(row) = self.rows.iter().position(|r| r.surface.is_none())
            {
                self.rows[row] = RowConfig {
                    surface: Some(probe.kind.id.clone()),
                    input: Some(probe.input),
                    output: Some(probe.output),
                };
                self.halted[row] = false;
                self.filled = true;
            } else if !gone && !answered {
                self.silent.insert(probe.input);
            }
        }
        self.silent
            .retain(|name| present(ports, Direction::Input, name));
        if self.rows.iter().all(|r| r.surface.is_some()) {
            return;
        }
        let states = ports.ports();
        let names = |direction| {
            states
                .iter()
                .filter(move |p| p.present && p.direction == direction)
                .map(|p| p.name.clone())
                .collect::<Vec<_>>()
        };
        let (inputs, outputs) = (names(Direction::Input), names(Direction::Output));
        for kind in kinds() {
            let Some(recognise) = kind.recognise else {
                continue;
            };
            let mut after_taken = false;
            for input in inputs.iter().filter(|name| recognise.names(name)) {
                let taken = self.named(input)
                    || self.declined.contains(input)
                    || self.silent.contains(input)
                    || self.probes.iter().any(|p| &p.input == input);
                // A controller's second port isn't its surface's.
                let second = recognise.pair && after_taken;
                after_taken = taken;
                let Some(output) = outputs.iter().find(|name| recognise.names(name)) else {
                    continue;
                };
                if taken || second || self.named(output) {
                    continue;
                }
                after_taken = true;
                let replies = Arc::new(Mutex::new(Vec::new()));
                let sink = Arc::clone(&replies);
                ports
                    .claim(Direction::Input, input)
                    .hold(Some(Box::new(move |_, bytes| {
                        lock(&sink).push(bytes.to_vec());
                    })));
                ports.claim(Direction::Output, output);
                let sent = self
                    .output
                    .send_to(PortId::from_name(output), &INQUIRY, None);
                if sent.is_err() {
                    ports.release(Direction::Input, input);
                    ports.release(Direction::Output, output);
                    self.silent.insert(input.clone());
                    continue;
                }
                self.probes.push(Probe {
                    kind: Arc::clone(kind),
                    input: input.clone(),
                    output: output.clone(),
                    replies,
                    since: Instant::now(),
                });
            }
        }
    }

    /// Makes what runs follow the rows and the ports: stops a surface whose
    /// row or ports changed or went away, holds the ports the rows name, and
    /// starts the surfaces whose ports are there (R3, R9).
    pub fn reconcile(&mut self, ports: &mut MidiPorts) {
        self.recognise(ports);
        // What the watchdog or a panic stopped stays stopped.
        for row in 0..ROWS {
            if self.started[row].is_some() && self.error_of(row).is_some() {
                self.started[row] = None;
                self.halted[row] = true;
            }
        }
        for row in 0..ROWS {
            if self.started[row].as_ref().map(|s| &s.config) != self.wanted(row, ports).as_ref() {
                self.stop(row);
            }
        }
        // Let go of every port no longer named before holding the new ones,
        // so two rows swapping ports don't release each other's.
        for row in 0..ROWS {
            let held = &mut self.held[row];
            if held.input != self.rows[row].input {
                if let Some(old) = held.input.take() {
                    ports.release(Direction::Input, &old);
                }
                held.live = None;
            }
            if held.output != self.rows[row].output
                && let Some(old) = held.output.take()
            {
                ports.release(Direction::Output, &old);
            }
        }
        for row in 0..ROWS {
            let (input, output) = (self.rows[row].input.clone(), self.rows[row].output.clone());
            let held = &mut self.held[row];
            if held.input.is_none()
                && let Some(name) = input
            {
                held.live = Some(ports.claim(Direction::Input, &name));
                held.input = Some(name);
            }
            if held.output.is_none()
                && let Some(name) = output
            {
                ports.claim(Direction::Output, &name);
                held.output = Some(name);
            }
        }
        for row in 0..ROWS {
            if self.started[row].is_none()
                && !self.halted[row]
                && let Some(config) = self.wanted(row, ports)
            {
                self.start(row, config);
            }
        }
    }

    /// The row's config if its surface can run now: it is one the chooser
    /// offers, its input is open, and so is its output where it needs one.
    fn wanted(&self, row: usize, ports: &MidiPorts) -> Option<RowConfig> {
        let config = &self.rows[row];
        let surface = self.kind(config.surface.as_deref()?)?;
        let open = |direction, name: &Option<String>| {
            name.as_ref()
                .and_then(|name| ports.port(direction, name))
                .is_some_and(|port| port.present && port.open)
        };
        (open(Direction::Input, &config.input)
            && (!surface.needs_output || open(Direction::Output, &config.output)))
        .then(|| config.clone())
    }

    fn host(&mut self) -> &Host {
        self.host.get_or_insert_with(|| {
            let host = Host::new(
                self.sender.clone(),
                self.output.clone(),
                Arc::clone(&self.traffic),
            );
            *lock(&host.shared.clock) = self.clock.clone();
            host
        })
    }

    fn start(&mut self, row: usize, config: RowConfig) {
        let (Some(surface), Some(input), Some(live)) = (
            config.surface.as_deref().and_then(|id| self.kind(id)),
            config.input.as_deref(),
            self.held[row].live.clone(),
        ) else {
            return;
        };
        self.sessions += 1;
        let session = self.sessions;
        let output = config.output.as_deref().map(PortId::from_name);
        let link = Link {
            name: input.to_owned(),
            input: PortId::from_name(input),
            live: Arc::clone(&live),
            output,
        };
        let host = self.host();
        let tx = host.tx.clone();
        live.hold(Some(Box::new(move |at, bytes| {
            let _ = tx.send(Event::Message {
                row,
                session,
                at,
                bytes: bytes.to_vec(),
            });
        })));
        host.ask(|ack| Event::Start {
            row,
            session,
            surface: surface.make(),
            link,
            ack,
        });
        self.started[row] = Some(Started { config });
    }

    fn stop(&mut self, row: usize) {
        if self.started[row].take().is_none() {
            return;
        }
        // The input's messages go the way its switches say again, and the
        // messages already queued for the surface are dropped.
        if let Some(live) = &self.held[row].live {
            live.hold(None);
        }
        if let Some(host) = &self.host {
            host.ask(|ack| Event::Stop { row, ack });
        }
    }

    fn error_of(&self, row: usize) -> Option<String> {
        let host = self.host.as_ref()?;
        let slot = host.shared.slot(row);
        lock(&slot.info).error.clone()
    }

    /// Every row as the table and `control_surface.list` show it (R13).
    #[must_use]
    pub fn report(&self, ports: &MidiPorts) -> Vec<RowReport> {
        (0..ROWS).map(|row| self.report_row(row, ports)).collect()
    }

    fn report_row(&self, row: usize, ports: &MidiPorts) -> RowReport {
        let config = self.rows[row].clone();
        let present = |direction, name: &Option<String>| {
            name.as_ref()
                .map(|name| ports.port(direction, name).is_some_and(|port| port.present))
        };
        let surface = config.surface.as_deref().map(|id| self.kind(id));
        let host_error = self.error_of(row);
        let (run, error) = match (&config.surface, surface.clone()) {
            (None, _) => (RunState::Empty, None),
            (Some(id), Some(None)) => (
                RunState::Stopped,
                Some(format!("no control surface has the ID \"{id}\"")),
            ),
            _ if self.halted[row] || (self.started[row].is_some() && host_error.is_some()) => {
                (RunState::Stopped, host_error)
            }
            _ if self.started[row].is_some() => (RunState::Running, None),
            _ => (RunState::Waiting, None),
        };
        let output_present = present(Direction::Output, &config.output);
        let state = (run == RunState::Running)
            .then(|| {
                let host = self.host.as_ref()?;
                let slot = host.shared.slot(row);
                let state = lock(&slot.info).state.clone();
                Some(state)
            })
            .flatten();
        RowReport {
            surface_name: surface.clone().flatten().map(|kind| kind.name.clone()),
            input_present: present(Direction::Input, &config.input),
            can_dump: surface.flatten().is_some_and(|kind| !kind.dump.is_empty())
                && output_present == Some(true),
            output_present,
            run,
            error,
            state,
            config,
        }
    }

    /// Sends the row's surface's setup to the row's output (R11).
    ///
    /// # Errors
    ///
    /// A message if the row has no surface with a setup, or its output isn't
    /// there.
    pub fn dump(&self, row: usize, ports: &MidiPorts) -> Result<usize, String> {
        let report = self.report_row(row, ports);
        if !report.can_dump {
            return Err(format!(
                "row {} has no surface with a setup to send and an output that is present",
                row + 1
            ));
        }
        let setup = report
            .config
            .surface
            .as_deref()
            .and_then(|id| self.kind(id))
            .map_or_else(Vec::new, |kind| kind.dump.clone());
        let port = report
            .config
            .output
            .as_deref()
            .map(PortId::from_name)
            .ok_or("the row has no output")?;
        for bytes in &setup {
            self.output
                .send_to(port, bytes, None)
                .map_err(|e| e.to_string())?;
            self.traffic.sent.fetch_add(1, Relaxed);
        }
        Ok(setup.len())
    }

    /// Whether any surface runs, so the shell has a read model to publish.
    #[must_use]
    pub fn running(&self) -> bool {
        self.started.iter().any(Option::is_some)
    }

    /// Hands the surfaces the read model for their next tick (R6).
    pub fn publish(&self, model: ReadModel) {
        if let Some(host) = &self.host {
            *lock(&host.shared.model) = Some(Arc::new(model));
        }
    }

    /// Takes the commands surfaces asked for since the last call, in the
    /// order they were asked (R7).
    pub fn take_requests(&self, into: &mut Vec<CommandRequest>) {
        if let Some(host) = &self.host {
            into.append(&mut lock(&host.shared.requests));
        }
    }

    /// How many messages surfaces took and sent: the remote indicators
    /// light when either moves (R15).
    #[must_use]
    pub fn traffic(&self) -> (u64, u64) {
        (
            self.traffic.taken.load(Relaxed),
            self.traffic.sent.load(Relaxed),
        )
    }

    /// Returns once the host has handled everything queued so far: the
    /// messages that arrived, and the starts and stops asked for.
    pub fn settle(&self) {
        if let Some(host) = &self.host {
            host.ask(Event::Barrier);
        }
    }

    /// Gives every running surface its tick now, and returns when they have
    /// had it, instead of waiting for the next of the fifty a second.
    pub fn tick(&self) {
        if let Some(host) = &self.host {
            host.ask(Event::Tick);
        }
    }

    /// What a running surface records for a test, if it records anything
    /// (the test surface does).
    #[must_use]
    pub fn probe(&self, row: usize) -> Option<Arc<dyn Any + Send + Sync>> {
        let host = self.host.as_ref()?;
        let slot = host.shared.slot(row);
        lock(&slot.info).probe.clone()
    }
}

impl std::fmt::Debug for Surfaces {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Surfaces")
            .field("rows", &self.rows)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(control: Option<u32>, tag: u8) -> Out {
        Out {
            bytes: vec![tag],
            at: None,
            control,
        }
    }

    // Spec: control-surfaces R8. A surface that sends faster than its port
    // takes loses the oldest messages for the same control first, and keeps
    // the others, in order.
    #[test]
    fn thinning_drops_the_oldest_for_a_control_before_anything_else() {
        let mut pending: VecDeque<Out> = VecDeque::new();
        for i in 0..PENDING_MAX + 10 {
            // Every message but the last of each pair moves control 1; the
            // rest have no control.
            pending.push_back(out(Some(1), 0));
            pending.push_back(out(None, (i % 200) as u8 + 1));
        }
        let before = pending.len();
        pending.push_back(out(Some(1), 255));
        thin(&mut pending);
        assert!(before > PENDING_MAX);
        // Only the newest message for control 1 survives, then the oldest of
        // the others go until the queue is half of what it may be.
        assert_eq!(pending.iter().filter(|m| m.control == Some(1)).count(), 1);
        assert_eq!(pending.back().unwrap().bytes, [255]);
        assert!(pending.len() <= PENDING_MAX / 2);
    }

    #[test]
    fn thinning_keeps_a_queue_that_has_nothing_stale_in_order() {
        let mut pending: VecDeque<Out> = (0..5).map(|i| out(Some(i), i as u8)).collect();
        thin(&mut pending);
        assert_eq!(
            pending.iter().map(|m| m.bytes[0]).collect::<Vec<_>>(),
            [0, 1, 2, 3, 4]
        );
    }
}

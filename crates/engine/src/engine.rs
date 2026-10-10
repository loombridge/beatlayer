//! The main thread's side of the engine: the queue to the audio thread, and
//! the engine with its compiler and drop threads.

use std::collections::VecDeque;
use std::mem;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use bl_core::time::Ticks;
use bl_model::Document;
use rtrb::{Producer, PushError, RingBuffer};

use crate::EngineConfig;
use crate::compiler::{Compiler, Update};
use crate::gc::{Collector, DROP_PERIOD, DropThread, GcHandle};
use crate::message::{Message, ParamEvent, Report, ScheduledCommand, TransportCommand};
use crate::node::{NodeData, NodeKey};
use crate::processor::{Carry, Processor};
use crate::registry::Registry;
use crate::state::{EngineState, Monitor};

/// Makes the three ends of an engine: the [`Sender`] that feeds the audio
/// thread, the [`Processor`] that runs on it, and the [`Monitor`] that reads
/// what it publishes. Everything the audio thread will need is allocated
/// here.
///
/// [`Engine::start`] wires these to a compiler thread and a drop thread. An
/// offline render uses them directly, on one thread.
#[must_use]
pub fn channel(config: &EngineConfig) -> (Sender, Processor, Monitor) {
    let (producer, consumer) = RingBuffer::new(config.queue_capacity);
    let (report_producer, report_consumer) = RingBuffer::new(config.report_capacity);
    let (state_input, state_output) = triple_buffer::triple_buffer(&EngineState::with_capacity(
        config.max_tracks,
        config.sample_rate,
    ));
    let sender = Sender {
        queue: producer,
        outbox: VecDeque::new(),
    };
    let processor = Processor::new(*config, consumer, report_producer, state_input);
    let monitor = Monitor {
        state: state_output,
        reports: report_consumer,
    };
    (sender, processor, monitor)
}

/// The sending end of the queue to the audio thread.
///
/// Messages arrive in the order they're sent. When the queue is full,
/// messages wait in an outbox here, in order, until [`Sender::flush`] finds
/// room: nothing is dropped, because each program takes over nodes from the
/// one before it.
pub struct Sender {
    queue: Producer<Message>,
    outbox: VecDeque<Message>,
}

impl Sender {
    /// Sends a compiler update: its program, then its parameter changes.
    pub fn send_update(&mut self, update: Update) {
        if let Some(program) = update.program {
            self.push(Message::Program(program));
        }
        for event in update.params {
            self.push(Message::Param(event));
        }
    }

    /// Sends a parameter change.
    pub fn send_param(&mut self, event: ParamEvent) {
        self.push(Message::Param(event));
    }

    /// Sends data to a node, such as a clip's notes after an edit. The node
    /// gets it at the start of the next block; data for a node the running
    /// program doesn't have is dropped.
    pub fn send_data(&mut self, key: NodeKey, data: NodeData) {
        self.push(Message::Data { key, data });
    }

    /// Sends a transport command, which takes effect at the start of the
    /// next block.
    pub fn transport(&mut self, command: TransportCommand) {
        self.push(Message::Transport(command));
    }

    /// Schedules a command at a musical position.
    pub fn schedule(&mut self, command: ScheduledCommand) {
        self.push(Message::Schedule(command));
    }

    /// Cancels a scheduled command that hasn't fired.
    pub fn cancel(&mut self, id: u64) {
        self.push(Message::Cancel(id));
    }

    /// Moves waiting messages into the queue while there's room. Returns
    /// whether none are left waiting.
    pub fn flush(&mut self) -> bool {
        while let Some(message) = self.outbox.pop_front() {
            if let Err(PushError::Full(message)) = self.queue.push(message) {
                self.outbox.push_front(message);
                return false;
            }
        }
        true
    }

    /// Messages waiting for room in the queue.
    #[must_use]
    pub fn backlog(&self) -> usize {
        self.outbox.len()
    }

    fn push(&mut self, message: Message) {
        self.outbox.push_back(message);
        self.flush();
    }
}

impl std::fmt::Debug for Sender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sender")
            .field("backlog", &self.outbox.len())
            .finish_non_exhaustive()
    }
}

/// What the compiler thread has done.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompileCounts {
    /// Documents synced.
    pub syncs: u64,
    /// Documents skipped because a newer one was already waiting.
    pub coalesced: u64,
    /// Programs compiled.
    pub programs: u64,
    /// Syncs that needed no program, only parameter changes or nothing.
    pub without_program: u64,
    /// Parameter changes sent from syncs.
    pub params: u64,
    /// Documents kept without compiling while no device ran the engine; the
    /// newest is compiled when one runs it again (WP-1.13).
    pub deferred: u64,
    /// Restarts on a device, each with a new processor (WP-1.13).
    pub restarts: u64,
}

#[derive(Debug, Default)]
struct Counts {
    syncs: AtomicU64,
    coalesced: AtomicU64,
    programs: AtomicU64,
    without_program: AtomicU64,
    params: AtomicU64,
    deferred: AtomicU64,
    restarts: AtomicU64,
}

/// A request to the compiler thread.
enum Request {
    Sync(Box<Document>),
    Param(ParamEvent),
    Data(NodeKey, NodeData),
    Transport(TransportCommand),
    Schedule(ScheduledCommand),
    Cancel(u64),
    Flush(mpsc::Sender<()>),
    /// No processor runs any more: stop sending to it.
    Detach,
    /// Make a processor for these settings.
    Restart(Box<Restart>),
    /// End the compiler thread.
    Shutdown,
}

/// A restart's settings, what the last processor handed on, and where the
/// new processor goes.
struct Restart {
    config: EngineConfig,
    carry: Carry,
    reply: mpsc::Sender<Processor>,
}

/// What reaches the compiler thread while no processor runs, for the next.
#[derive(Debug, Default)]
struct Detached {
    /// The latest locate.
    locate: Option<Ticks>,
    /// Commands scheduled meanwhile.
    scheduled: Vec<ScheduledCommand>,
    /// Commands cancelled meanwhile, which may be among the carried ones.
    cancelled: Vec<u64>,
}

/// Restarts the engine from another thread: the hook a device supervisor
/// uses when a stream closes and opens again (WP-1.13).
#[derive(Clone, Debug)]
pub(crate) struct Restarter {
    requests: mpsc::Sender<Request>,
}

impl Restarter {
    /// Tells the compiler thread no processor runs: until the next restart,
    /// documents are kept without compiling, a locate and scheduled commands
    /// wait, and play, parameter changes, and node data are dropped.
    pub(crate) fn detach(&self) {
        let _ = self.requests.send(Request::Detach);
    }

    /// A new processor for `config`, with the latest document compiled for
    /// it, the song at `carry`'s position (stopped), and its scheduled
    /// commands waiting. The engine's [`Monitor`] moves to the new
    /// processor. `None` if the compiler thread doesn't answer within
    /// `timeout`.
    pub(crate) fn restart(
        &self,
        config: EngineConfig,
        carry: Carry,
        timeout: Duration,
    ) -> Option<Processor> {
        let (reply, processor) = mpsc::channel();
        let restart = Restart {
            config,
            carry,
            reply,
        };
        self.requests
            .send(Request::Restart(Box::new(restart)))
            .ok()?;
        processor.recv_timeout(timeout).ok()
    }
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Sync(_) => "Sync",
            Self::Param(_) => "Param",
            Self::Data(..) => "Data",
            Self::Transport(_) => "Transport",
            Self::Schedule(_) => "Schedule",
            Self::Cancel(_) => "Cancel",
            Self::Flush(_) => "Flush",
            Self::Detach => "Detach",
            Self::Restart(_) => "Restart",
            Self::Shutdown => "Shutdown",
        })
    }
}

/// The engine, from the main thread: it compiles documents on a thread of
/// its own, frees what the audio thread drops on another, and shows what
/// the audio thread publishes.
///
/// Everything sent through it reaches the audio thread in order, through
/// the compiler thread, which is the queue's one sender. Documents go as
/// snapshots: a [`Document`] clone shares every node with the main thread's
/// copy, so it costs a few reference counts.
///
/// Stop the audio stream (dropping its [`Processor`]) before dropping the
/// engine, so the drop thread's last collection frees the last program.
///
/// On a device, the engine restarts with a new processor whenever the
/// device's stream closes and opens again, perhaps at another sample rate
/// ([`AudioEngine`](crate::AudioEngine), WP-1.13). The engine then reads the
/// new processor's state and reports, and keeps the old one's unread
/// reports for [`reports`](Self::reports).
pub struct Engine {
    requests: Option<mpsc::Sender<Request>>,
    compiler: Option<JoinHandle<()>>,
    monitor: Monitor,
    /// Monitors of processors made by restarts, newest last.
    monitors: mpsc::Receiver<Monitor>,
    /// Reports from processors that have been replaced, not yet read.
    held: Vec<Report>,
    gc: GcHandle,
    counts: Arc<Counts>,
    // Dropped last, after the compiler thread has let go of everything.
    drop_thread: DropThread,
}

impl Engine {
    /// Starts an engine's compiler and drop threads, and returns the engine
    /// and the processor to run on the audio thread.
    ///
    /// # Errors
    ///
    /// If a thread can't start.
    pub fn start(config: EngineConfig, registry: Registry) -> std::io::Result<(Self, Processor)> {
        let (engine, processor) = Self::spawn(config, registry, true)?;
        let processor = processor.expect("an attached engine has a processor");
        Ok((engine, processor))
    }

    /// Starts an engine with no processor, for a device that hasn't opened
    /// yet: documents are kept without compiling until a restart makes the
    /// processor for the device's settings. `config` gives the sizes every
    /// processor keeps; a restart sets the sample rate and block size.
    ///
    /// # Errors
    ///
    /// If a thread can't start.
    pub fn start_detached(config: EngineConfig, registry: Registry) -> std::io::Result<Self> {
        Self::spawn(config, registry, false).map(|(engine, _)| engine)
    }

    fn spawn(
        config: EngineConfig,
        registry: Registry,
        attached: bool,
    ) -> std::io::Result<(Self, Option<Processor>)> {
        let collector = Collector::new();
        let gc = collector.handle();
        let drop_thread = collector.spawn(DROP_PERIOD)?;
        let (sender, processor, monitor) = channel(&config);
        let (sender, processor) = if attached {
            (Some(sender), Some(processor))
        } else {
            (None, None)
        };
        let compiler = Compiler::new(config, registry, gc.clone());
        let counts = Arc::new(Counts::default());
        let (requests, receiver) = mpsc::channel();
        let (new_monitors, monitors) = mpsc::channel();
        let thread = {
            let counts = Arc::clone(&counts);
            thread::Builder::new()
                .name("bl-engine-compiler".into())
                .spawn(move || {
                    compile_loop(compiler, sender, &receiver, &counts, &new_monitors);
                })?
        };
        let engine = Self {
            requests: Some(requests),
            compiler: Some(thread),
            monitor,
            monitors,
            held: Vec::new(),
            gc,
            counts,
            drop_thread,
        };
        Ok((engine, processor))
    }

    /// A handle that restarts the engine from another thread.
    pub(crate) fn restarter(&self) -> Restarter {
        Restarter {
            requests: self
                .requests
                .clone()
                .expect("a running engine has its request channel"),
        }
    }

    /// Moves to the newest processor's monitor, keeping the unread reports
    /// of the ones it replaces.
    fn adopt(&mut self) {
        while let Ok(monitor) = self.monitors.try_recv() {
            let mut old = mem::replace(&mut self.monitor, monitor);
            self.held.extend(old.reports());
        }
    }

    fn request(&self, request: Request) {
        if let Some(requests) = &self.requests {
            // Fails only if the compiler thread has gone, which it does only
            // when the engine drops.
            let _ = requests.send(request);
        }
    }

    /// Brings the engine up to date with `document`: a new program if its
    /// structure changed, parameter changes if only values did.
    pub fn sync(&self, document: &Document) {
        self.request(Request::Sync(Box::new(document.clone())));
    }

    /// Sends a parameter change.
    pub fn send_param(&self, event: ParamEvent) {
        self.request(Request::Param(event));
    }

    /// Sends data to a node (see [`Sender::send_data`]).
    pub fn send_data(&self, key: NodeKey, data: NodeData) {
        self.request(Request::Data(key, data));
    }

    /// Wraps a value as node data, to send with [`Engine::send_data`].
    pub fn data<T: std::any::Any + Send + Sync>(&self, value: T) -> NodeData {
        self.gc.data(value)
    }

    /// Starts the song.
    pub fn play(&self) {
        self.request(Request::Transport(TransportCommand::Play));
    }

    /// Stops the song.
    pub fn stop(&self) {
        self.request(Request::Transport(TransportCommand::Stop));
    }

    /// Moves the song position.
    pub fn locate(&self, at: bl_core::time::Ticks) {
        self.request(Request::Transport(TransportCommand::Locate(at)));
    }

    /// Schedules a command at a musical position (D17).
    pub fn schedule(&self, command: ScheduledCommand) {
        self.request(Request::Schedule(command));
    }

    /// Cancels a scheduled command that hasn't fired.
    pub fn cancel(&self, id: u64) {
        self.request(Request::Cancel(id));
    }

    /// Waits, up to `timeout`, until everything sent so far is on the queue
    /// to the audio thread. Returns whether it got there. (The audio thread
    /// takes it at its next block.) While no device runs the engine, it
    /// returns at once: there's no queue to wait for.
    pub fn flush(&self, timeout: Duration) -> bool {
        let (ack, done) = mpsc::channel();
        self.request(Request::Flush(ack));
        done.recv_timeout(timeout).is_ok()
    }

    /// The latest state the audio thread published.
    pub fn state(&mut self) -> &EngineState {
        self.adopt();
        self.monitor.state()
    }

    /// Reports from the audio thread since the last call.
    pub fn reports(&mut self) -> Vec<Report> {
        self.adopt();
        let mut reports = mem::take(&mut self.held);
        reports.extend(self.monitor.reports());
        reports
    }

    /// What the compiler thread has done.
    #[must_use]
    pub fn counts(&self) -> CompileCounts {
        CompileCounts {
            syncs: self.counts.syncs.load(Relaxed),
            coalesced: self.counts.coalesced.load(Relaxed),
            programs: self.counts.programs.load(Relaxed),
            without_program: self.counts.without_program.load(Relaxed),
            params: self.counts.params.load(Relaxed),
            deferred: self.counts.deferred.load(Relaxed),
            restarts: self.counts.restarts.load(Relaxed),
        }
    }

    /// How many values the drop thread's collector holds: the running
    /// program and its data, and anything dropped but not yet freed.
    #[must_use]
    pub fn held(&self) -> usize {
        self.drop_thread.held()
    }

    /// Asks the drop thread to collect now.
    pub fn collect(&self) {
        self.drop_thread.wake();
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // A restarter may still hold the channel open, so ask the compiler
        // thread to end rather than wait for the channel to close.
        if let Some(requests) = self.requests.take() {
            let _ = requests.send(Request::Shutdown);
        }
        if let Some(thread) = self.compiler.take() {
            let _ = thread.join();
        }
    }
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("counts", &self.counts())
            .field("held", &self.held())
            .finish_non_exhaustive()
    }
}

/// The compiler thread: takes requests in order, compiles documents, and
/// sends everything to the audio thread. With no processor (`sender` is
/// `None`), it keeps what the next one needs instead, until a restart.
fn compile_loop(
    mut compiler: Compiler,
    mut sender: Option<Sender>,
    requests: &mpsc::Receiver<Request>,
    counts: &Counts,
    monitors: &mpsc::Sender<Monitor>,
) {
    let mut batch = Vec::new();
    let mut flushes: Vec<mpsc::Sender<()>> = Vec::new();
    let mut detached = Detached::default();
    loop {
        // While messages wait for room, look again every millisecond.
        let first = if sender.as_ref().is_some_and(|s| s.backlog() > 0) {
            match requests.recv_timeout(Duration::from_millis(1)) {
                Ok(request) => Some(request),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match requests.recv() {
                Ok(request) => Some(request),
                Err(_) => break,
            }
        };
        batch.extend(first);
        batch.extend(requests.try_iter());

        let mut pending = batch.drain(..).peekable();
        while let Some(request) = pending.next() {
            match request {
                Request::Sync(document) => {
                    counts.syncs.fetch_add(1, Relaxed);
                    // Only the newest of documents sent back to back matters.
                    if matches!(pending.peek(), Some(Request::Sync(_))) {
                        counts.coalesced.fetch_add(1, Relaxed);
                        continue;
                    }
                    let Some(sender) = sender.as_mut() else {
                        compiler.remember(&document);
                        counts.deferred.fetch_add(1, Relaxed);
                        continue;
                    };
                    let update = compiler.sync(&document);
                    if update.program.is_some() {
                        counts.programs.fetch_add(1, Relaxed);
                    } else {
                        counts.without_program.fetch_add(1, Relaxed);
                    }
                    counts.params.fetch_add(update.params.len() as u64, Relaxed);
                    sender.send_update(update);
                }
                Request::Param(event) => {
                    if let Some(sender) = sender.as_mut() {
                        sender.send_param(event);
                    }
                }
                Request::Data(key, data) => {
                    if let Some(sender) = sender.as_mut() {
                        sender.send_data(key, data);
                    }
                }
                Request::Transport(command) => match sender.as_mut() {
                    Some(sender) => sender.transport(command),
                    // Starting the song while no device plays it would
                    // start it the moment one comes back; a locate waits.
                    None => {
                        if let TransportCommand::Locate(at) = command {
                            detached.locate = Some(at);
                        }
                    }
                },
                Request::Schedule(command) => match sender.as_mut() {
                    Some(sender) => sender.schedule(command),
                    None => detached.scheduled.push(command),
                },
                Request::Cancel(id) => match sender.as_mut() {
                    Some(sender) => sender.cancel(id),
                    None => {
                        detached.scheduled.retain(|command| command.id != id);
                        detached.cancelled.push(id);
                    }
                },
                Request::Flush(ack) => flushes.push(ack),
                Request::Detach => {
                    // Frees the queue to the processor that stopped, and
                    // anything still waiting for room in it.
                    sender = None;
                    detached = Detached::default();
                }
                Request::Restart(restart) => {
                    let Restart {
                        config,
                        carry,
                        reply,
                    } = *restart;
                    let (next, processor, monitor) = restart_with(
                        &mut compiler,
                        config,
                        carry,
                        mem::take(&mut detached),
                        counts,
                    );
                    // The engine reads the new processor's state from now on.
                    let _ = monitors.send(monitor);
                    if reply.send(processor).is_ok() {
                        sender = Some(next);
                    } else {
                        // Nobody waits for it any more, so nothing runs it.
                        sender = None;
                    }
                }
                Request::Shutdown => return,
            }
        }
        if sender.as_mut().is_none_or(Sender::flush) {
            for ack in flushes.drain(..) {
                let _ = ack.send(());
            }
        }
    }
}

/// Makes a processor for `config` and readies it: the latest document
/// compiled anew, the song located at the carried position (or a locate
/// sent while detached), stopped, and the scheduled commands waiting.
fn restart_with(
    compiler: &mut Compiler,
    config: EngineConfig,
    carry: Carry,
    detached: Detached,
    counts: &Counts,
) -> (Sender, Processor, Monitor) {
    let (mut sender, mut processor, monitor) = channel(&config);
    let update = compiler.reconfigure(config);
    if update.program.is_some() {
        counts.programs.fetch_add(1, Relaxed);
    }
    sender.send_update(update);
    sender.transport(TransportCommand::Locate(
        detached.locate.unwrap_or(carry.position),
    ));
    let carried = carry
        .scheduled
        .into_iter()
        .filter(|command| !detached.cancelled.contains(&command.id));
    for command in carried.chain(detached.scheduled) {
        sender.schedule(command);
    }
    // The processor isn't on a device yet, so this thread can drain its
    // queue: it starts with its program, position, and commands in place,
    // and its state shows them.
    loop {
        let sent = sender.flush();
        processor.prime();
        if sent {
            break;
        }
    }
    counts.restarts.fetch_add(1, Relaxed);
    (sender, processor, monitor)
}

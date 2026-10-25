//! The main thread's side of the engine: the queue to the audio thread, and
//! the engine with its compiler and drop threads.

use std::collections::VecDeque;
use std::mem;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use bl_model::{DeviceId, Document, LaunchSettings};
use rtrb::{Producer, PushError, RingBuffer};

use crate::EngineConfig;
use crate::capture::{CAPTURE_RING, CapturedMidi};
use crate::compiler::{Compiler, Update};
use crate::display::Displays;
use crate::drum::ChainHits;
use crate::gc::{Collector, DROP_PERIOD, DropThread, GcHandle, Owned};
use crate::message::{Message, ParamEvent, Report, ScheduledCommand, TransportCommand, When};
use crate::node::{NodeData, NodeKey};
use crate::processor::{Carry, Processor, starts_playback};
use crate::recording::RecordingPlan;
use crate::registry::Registry;
use crate::session::{SceneOverride, SessionCommand};
use crate::sources::Loaded;
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
    let (capture_producer, capture_consumer) = RingBuffer::new(CAPTURE_RING);
    let (state_input, state_output) = triple_buffer::triple_buffer(&EngineState::with_capacity(
        config.max_tracks,
        config.max_slots,
        config.max_modulated,
        config.max_processing,
        config.max_chain_meters,
        config.sample_rate,
    ));
    let sender = Sender {
        queue: producer,
        outbox: VecDeque::new(),
    };
    let processor = Processor::new(
        *config,
        consumer,
        report_producer,
        state_input,
        capture_producer,
    );
    let monitor = Monitor {
        state: state_output,
        reports: report_consumer,
        captured: capture_consumer,
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
    /// Sends a compiler update: its program, then its node data (clip
    /// players' clips), then its transport commands (the Arrangement loop),
    /// then its parameter changes.
    pub fn send_update(&mut self, update: Update) {
        if let Some(program) = update.program {
            self.push(Message::Program(program));
        }
        for (key, data) in update.data {
            self.push(Message::Data { key, data });
        }
        for command in update.transport {
            self.push(Message::Transport(command, When::Now));
        }
        for event in update.params {
            self.push(Message::Param(event));
        }
    }

    /// Sends the audio device's output latency in frames now (WP-5.34).
    pub fn output_latency(&mut self, frames: u32) {
        self.push(Message::OutputLatency(frames));
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

    /// Sends what the audio thread records while the transport records
    /// (WP-2.13), wrapped so dropping it there frees nothing.
    pub fn record(&mut self, plan: Owned<RecordingPlan>) {
        self.push(Message::Record(plan));
    }

    /// Captures the outputs of a real-time render while the song plays the
    /// range (WP-5.35). The plan's rings close when it goes.
    pub fn render(&mut self, plan: Owned<crate::render::RenderPlan>) {
        self.push(Message::Render(plan));
    }

    /// Sends a transport command, which takes effect at the start of the
    /// next block.
    pub fn transport(&mut self, command: TransportCommand) {
        self.transport_at(command, When::Now);
    }

    /// Sends a transport command that takes effect on a frame of the engine
    /// clock ([`When::Frame`]), splitting the block it falls in, so a start
    /// lands on its exact sample whatever the block size. A frame already
    /// past takes effect at the start of the next block. Commands for the
    /// same frame take effect in the order they were sent. As many as
    /// [`EngineConfig::max_scheduled`](crate::EngineConfig::max_scheduled)
    /// can wait for their frames; the engine drops and counts any more
    /// ([`EngineCounters::transport_overflowed`](crate::EngineCounters::transport_overflowed)).
    pub fn transport_at(&mut self, command: TransportCommand, at: When) {
        self.push(Message::Transport(command, at));
    }

    /// Schedules a command at a musical position.
    pub fn schedule(&mut self, command: ScheduledCommand) {
        self.push(Message::Schedule(command));
    }

    /// Sends a Session View command (WP-3.2): a launch, a stop, or the
    /// global launch quantisation. It takes effect at the start of the next
    /// block; a launch or a stop lands on its quantisation frame.
    pub fn session(&mut self, command: SessionCommand) {
        self.push(Message::Session(command));
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
    /// The latency a device's node reports now, from a plug-in that
    /// changed its own (WP-2.7).
    Latency(DeviceId, u32),
    /// A file's analysis is ready: the clips that play it are compiled
    /// again, so a Beats clip cuts at its transients (warp-modes R14).
    MediaAnalysed(bl_model::ContentHash),
    /// The audio interface's output latency and round trip in frames now
    /// (WP-5.34).
    RoundTrip {
        output: u32,
        round_trip: u32,
    },
    Transport(TransportCommand, When),
    Schedule(ScheduledCommand),
    Cancel(u64),
    /// A Session View command (WP-3.2).
    Session(SessionCommand),
    /// What the audio thread says a launched scene put in place: compile
    /// the last document again under it (WP-3.5).
    Override(SceneOverride),
    Flush(mpsc::Sender<()>),
    /// What to record while the transport records (WP-2.13).
    Record(Box<Owned<RecordingPlan>>),
    /// What to capture while the song plays a range in real time (WP-5.35).
    Render(Box<Owned<crate::render::RenderPlan>>),
    /// No processor runs any more: stop sending to it.
    Detach,
    /// Make a processor for these settings.
    Restart(Box<Restart>),
    /// The loader opened a clip's source, or couldn't.
    Loaded(Box<Loaded>),
    /// End the compiler thread.
    Shutdown,
}

/// A restart's settings, what the last processor handed on, and where the
/// new processor goes.
struct Restart {
    config: EngineConfig,
    carry: Option<Carry>,
    reply: mpsc::Sender<Processor>,
}

/// What reaches the compiler thread while no processor runs, held for the
/// next in the order it came: the song is stopped meanwhile, so these act
/// as they would on a stopped transport once it starts.
type Detached = Vec<Held>;

/// A request held while no processor runs.
#[derive(Debug)]
enum Held {
    /// A transport command for when it was sent: a locate, a stop, a loop
    /// change from a request or from a document, at once or on its frame of
    /// the clock. Nothing that starts the song is held.
    Transport(TransportCommand, When),
    /// A command scheduled meanwhile.
    Schedule(ScheduledCommand),
    /// A cancellation, perhaps of a carried command.
    Cancel(u64),
    /// The launch quantisation, which nothing else carries to the processor
    /// that takes over (a launch or a stop while nothing plays is dropped).
    Quantization(LaunchSettings),
}

/// Restarts the engine from another thread: the hook a device supervisor
/// uses when a stream closes and opens again (WP-1.13; `bl-audio-engine`'s
/// `AudioEngine` is one). When a stream closes, [`Processor::park`] the
/// processor that comes back from it and [`detach`](Self::detach); when
/// one opens, [`restart`](Self::restart) with the [`Carry`] it handed on.
#[derive(Clone, Debug)]
pub struct Restarter {
    requests: mpsc::Sender<Request>,
}

impl Restarter {
    /// Tells the compiler thread no processor runs: until the next restart,
    /// documents are kept without compiling; transport commands (with their
    /// times), scheduled commands, and cancellations wait for the next
    /// processor; and play, parameter changes, and node data are dropped.
    pub fn detach(&self) {
        let _ = self.requests.send(Request::Detach);
    }

    /// A new processor for `config`, with the latest document compiled for
    /// it, the transport and the commands `carry` hands on taken over at
    /// its rate, the song stopped, and then what was sent while detached.
    /// With no `carry` (the device's first stream, or a processor that never
    /// came back), the song starts at 1.1.1 with the document's loop. The
    /// engine's [`Monitor`] moves to the new processor. `None` if the
    /// compiler thread doesn't answer within `timeout`.
    #[must_use]
    pub fn restart(
        &self,
        config: EngineConfig,
        carry: Option<Carry>,
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
            Self::Latency(..) => "Latency",
            Self::MediaAnalysed(_) => "MediaAnalysed",
            Self::RoundTrip { .. } => "RoundTrip",
            Self::Transport(..) => "Transport",
            Self::Schedule(_) => "Schedule",
            Self::Cancel(_) => "Cancel",
            Self::Session(_) => "Session",
            Self::Override(_) => "Override",
            Self::Flush(_) => "Flush",
            Self::Record(_) => "Record",
            Self::Render(_) => "Render",
            Self::Detach => "Detach",
            Self::Restart(_) => "Restart",
            Self::Loaded(_) => "Loaded",
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
/// (`bl-audio-engine`'s `AudioEngine`, WP-1.13). The engine then reads the
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
    /// Captured notes from processors that have been replaced, not yet read.
    held_captured: Vec<CapturedMidi>,
    /// The scene override the compiler thread was last told of.
    override_sent: SceneOverride,
    gc: GcHandle,
    counts: Arc<Counts>,
    displays: Displays,
    hits: ChainHits,
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
        let mut compiler = Compiler::new(config, registry, gc.clone());
        let displays = compiler.displays();
        let hits = compiler.chain_hits();
        let counts = Arc::new(Counts::default());
        let (requests, receiver) = mpsc::channel();
        // Spec: audio-clips R17. Clips' sources open on a thread of their
        // own, so loading a long file never holds up Play or Stop.
        compiler.load_off_thread({
            let requests = requests.clone();
            move |loaded| {
                let _ = requests.send(Request::Loaded(Box::new(loaded)));
            }
        })?;
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
            held_captured: Vec::new(),
            override_sent: SceneOverride::default(),
            gc,
            counts,
            drop_thread,
            displays,
            hits,
        };
        Ok((engine, processor))
    }

    /// A handle that restarts the engine from another thread.
    ///
    /// # Panics
    ///
    /// Never: the request channel lives until the engine drops.
    #[must_use]
    pub fn restarter(&self) -> Restarter {
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
            self.held_captured.extend(old.captured());
            // The processor that takes over starts with the set's own maps,
            // and the compiler thread has compiled for it.
            self.override_sent = SceneOverride::default();
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

    /// Sets what the engine records while the transport records (WP-2.13):
    /// the armed tracks' inputs and the rings their samples go to. The plan
    /// replaces any earlier one; the audio thread drops it when the take
    /// ends, which closes the rings and tells the writer the take is over.
    pub fn record(&self, plan: crate::recording::RecordingPlan) {
        self.request(Request::Record(Box::new(self.gc.wrap(plan))));
    }

    /// Sets what the engine captures while the song plays a range in real
    /// time (WP-5.35): the rendered outputs and the rings their samples go
    /// to. The plan replaces any earlier one; the audio thread drops it
    /// when the render ends, which closes the rings and tells the writer
    /// the render is over.
    pub fn render(&self, plan: crate::render::RenderPlan) {
        self.request(Request::Render(Box::new(self.gc.wrap(plan))));
    }

    /// Reports `device`'s node's latency now, as a plug-in that restarted
    /// for a new one does: the compiler compiles a program whose delay
    /// compensation uses it, so a latency change reaches the running song
    /// (WP-2.7; delay-compensation R6).
    pub fn set_device_latency(&self, device: DeviceId, frames: u32) {
        self.request(Request::Latency(device, frames));
    }

    /// Tells the engine `media`'s analysis is now ready in the media
    /// library: every track that plays the file gets its clips again, so a
    /// Beats clip's plan cuts at the transients the library now gives
    /// (the warp modes spec, R14). The document hasn't changed, so a sync
    /// wouldn't.
    pub fn media_analysed(&self, media: bl_model::ContentHash) {
        self.request(Request::MediaAnalysed(media));
    }

    /// Reports the audio interface's latency now, in frames: `output`, the
    /// output latency it reports, which MIDI to ports is sent after, and
    /// `round_trip`, its reported output and input latency plus Driver
    /// Error Compensation, which the external devices report to delay
    /// compensation, so a change reaches the running song (the external
    /// devices spec, R4, R13).
    pub fn set_latency(&self, output: u32, round_trip: u32) {
        self.request(Request::RoundTrip { output, round_trip });
    }

    /// Wraps a value as node data, to send with [`Engine::send_data`].
    pub fn data<T: std::any::Any + Send + Sync>(&self, value: T) -> NodeData {
        self.gc.data(value)
    }

    /// Plays from the insert marker ([`TransportCommand::Play`]).
    pub fn play(&self) {
        self.transport(TransportCommand::Play);
    }

    /// Stops the song, or returns it to 1.1.1 if it's stopped
    /// ([`TransportCommand::Stop`]).
    pub fn stop(&self) {
        self.transport(TransportCommand::Stop);
    }

    /// Moves the insert marker, and the playhead if the song is playing
    /// ([`TransportCommand::Locate`]).
    pub fn locate(&self, at: bl_core::time::Ticks) {
        self.transport(TransportCommand::Locate(at));
    }

    /// Sends a transport command, which takes effect at the start of the
    /// audio thread's next block.
    pub fn transport(&self, command: TransportCommand) {
        self.transport_at(command, When::Now);
    }

    /// Sends a transport command for a frame of the engine clock (see
    /// [`Sender::transport_at`]).
    pub fn transport_at(&self, command: TransportCommand, at: When) {
        self.request(Request::Transport(command, at));
    }

    /// Schedules a command at a musical position (D17).
    pub fn schedule(&self, command: ScheduledCommand) {
        self.request(Request::Schedule(command));
    }

    /// Cancels a scheduled command that hasn't fired.
    pub fn cancel(&self, id: u64) {
        self.request(Request::Cancel(id));
    }

    /// Sends a Session View command (WP-3.2).
    pub fn session(&self, command: SessionCommand) {
        self.request(Request::Session(command));
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
        // Spec: session-view R23. A scene's launch changed the maps the
        // audio thread plays under: the compiler thread compiles the last
        // document under them, so the Arrangement's clips follow the tempo.
        let scene_override = self.monitor.state().scene_override;
        if scene_override != self.override_sent {
            self.override_sent = scene_override;
            self.request(Request::Override(scene_override));
        }
        self.monitor.state()
    }

    /// The display data the running program's devices publish after each
    /// block (the device framework spec, R26).
    #[must_use]
    pub fn displays(&self) -> &Displays {
        &self.displays
    }

    /// How many notes have reached each drum chain of the running program
    /// (the drum racks spec, R20).
    #[must_use]
    pub fn chain_hits(&self) -> &ChainHits {
        &self.hits
    }

    /// Reports from the audio thread since the last call.
    pub fn reports(&mut self) -> Vec<Report> {
        self.adopt();
        let mut reports = mem::take(&mut self.held);
        reports.extend(self.monitor.reports());
        reports
    }

    /// The notes monitoring MIDI tracks received since the last call,
    /// oldest first, with those of processors that have been replaced
    /// (WP-3.14).
    pub fn captured(&mut self) -> Vec<CapturedMidi> {
        self.adopt();
        let mut notes = mem::take(&mut self.held_captured);
        notes.extend(self.monitor.captured());
        notes
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
    let mut detached = Detached::new();
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
                        // A loop edit still reaches the next processor, after
                        // the loop it carries over.
                        for command in compiler.remember(&document) {
                            detached.push(Held::Transport(command, When::Now));
                        }
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
                Request::RoundTrip { output, round_trip } => {
                    if let Some(sender) = sender.as_mut() {
                        sender.output_latency(output);
                        let update = compiler.round_trip(round_trip);
                        if update.program.is_some() {
                            counts.programs.fetch_add(1, Relaxed);
                        }
                        sender.send_update(update);
                    }
                }
                Request::Latency(device, frames) => {
                    // While no processor runs, the next restart compiles
                    // every node anew, and they read their plug-ins'
                    // latency themselves.
                    if let Some(sender) = sender.as_mut() {
                        let update = compiler.latency(device, frames);
                        if update.program.is_some() {
                            counts.programs.fetch_add(1, Relaxed);
                        } else {
                            counts.without_program.fetch_add(1, Relaxed);
                        }
                        sender.send_update(update);
                    }
                }
                Request::MediaAnalysed(media) => {
                    // While no processor runs, the next restart compiles
                    // every clip anew, asking the library then.
                    if let Some(sender) = sender.as_mut() {
                        let update = compiler.media_analysed(media);
                        counts.without_program.fetch_add(1, Relaxed);
                        sender.send_update(update);
                    }
                }
                Request::Transport(command, at) => match sender.as_mut() {
                    Some(sender) => sender.transport_at(command, at),
                    // Spec: device-changes R7. Starting the song while no
                    // device plays it would start it the moment one comes
                    // back; everything else waits, with its time.
                    None => {
                        if !starts_playback(command) {
                            detached.push(Held::Transport(command, at));
                        }
                    }
                },
                Request::Schedule(command) => match sender.as_mut() {
                    Some(sender) => sender.schedule(command),
                    None => detached.push(Held::Schedule(command)),
                },
                Request::Cancel(id) => match sender.as_mut() {
                    Some(sender) => sender.cancel(id),
                    None => detached.push(Held::Cancel(id)),
                },
                Request::Session(command) => match sender.as_mut() {
                    Some(sender) => sender.session(command),
                    // Spec: device-changes R7. Nothing plays while no device
                    // runs the engine, so only the quantisation waits; a
                    // launch or a stop for the stopped song is dropped.
                    None => {
                        if let SessionCommand::SetQuantization(settings) = command {
                            detached.push(Held::Quantization(settings));
                        }
                    }
                },
                Request::Override(scene_override) => {
                    // Without a processor the next restart compiles the
                    // set's own maps.
                    if let Some(sender) = sender.as_mut() {
                        let update = compiler.set_override(scene_override);
                        if update.program.is_some() {
                            counts.programs.fetch_add(1, Relaxed);
                        }
                        sender.send_update(update);
                    }
                }
                Request::Flush(ack) => flushes.push(ack),
                Request::Record(plan) => {
                    // While no processor runs, nothing records it.
                    if let Some(sender) = sender.as_mut() {
                        sender.record(*plan);
                    }
                }
                Request::Render(plan) => {
                    // While no processor runs, nothing renders in real time.
                    if let Some(sender) = sender.as_mut() {
                        sender.render(*plan);
                    }
                }
                Request::Detach => {
                    // Frees the queue to the processor that stopped, and
                    // anything still waiting for room in it. What's held
                    // already (a second detach) stays held.
                    sender = None;
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
                Request::Loaded(loaded) => {
                    // While no processor runs, the restart's compile asks
                    // for every source again.
                    if let Some(sender) = sender.as_mut() {
                        sender.send_update(compiler.loaded(*loaded));
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

/// Makes a processor for `config` and readies it, in order: the latest
/// document compiled anew for it; the transport and the commands `carry`
/// hands on, taken over at the new rate with the song stopped
/// (device-changes R7); then what was sent while detached, in the order it
/// came, as on a stopped transport. A transport command held for a frame
/// of the old clock lands as long after the restart as it was after the
/// stream closed.
fn restart_with(
    compiler: &mut Compiler,
    config: EngineConfig,
    carry: Option<Carry>,
    detached: Detached,
    counts: &Counts,
) -> (Sender, Processor, Monitor) {
    let (mut sender, mut processor, monitor) = channel(&config);
    let update = compiler.reconfigure(config);
    if update.program.is_some() {
        counts.programs.fetch_add(1, Relaxed);
    }
    sender.send_update(update);
    // The processor isn't on a device yet, so this thread can drain its
    // queue: the program is in place before the carried positions fall on
    // its frames, and the processor starts with everything in place, its
    // state showing it.
    deliver(&mut sender, &mut processor);
    if let Some(carry) = &carry {
        processor.take_over(carry);
    }
    for held in detached {
        match held {
            Held::Transport(command, at) => {
                let at = match (at, &carry) {
                    (When::Frame(frame), Some(carry)) => {
                        When::Frame(carry.frame_at(frame, config.sample_rate))
                    }
                    _ => at,
                };
                sender.transport_at(command, at);
            }
            Held::Schedule(command) => sender.schedule(command),
            Held::Cancel(id) => sender.cancel(id),
            Held::Quantization(settings) => {
                sender.session(SessionCommand::SetQuantization(settings))
            }
        }
    }
    deliver(&mut sender, &mut processor);
    counts.restarts.fetch_add(1, Relaxed);
    (sender, processor, monitor)
}

/// Moves everything `sender` holds into `processor`, which isn't on a
/// device yet.
fn deliver(sender: &mut Sender, processor: &mut Processor) {
    loop {
        let sent = sender.flush();
        processor.prime();
        if sent {
            break;
        }
    }
}

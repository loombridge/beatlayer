//! The audio host: the device the app plays through, the engine that plays
//! the set on it, the settings that choose the device, the test tone, and
//! the CPU meter (L-SET-012, L-CTRL-023, L-CTRL-024).
//!
//! [`AudioHost`] owns one stream from `bl-audio-io`. Changing a setting
//! ([`AudioHost::configure`]) closes the stream and opens a new one with the
//! new settings straight away, so a device, rate, buffer, or channel change
//! applies without restarting the app. If the device refuses, the previous
//! settings are reopened and the error is reported.
//!
//! Each stream plays a `bl-engine` [`Engine`] made for the rate it runs at
//! (WP-1.18): the host keeps the newest document ([`AudioHost::sync`]) for
//! its compiler, sends it the transport's commands
//! ([`AudioHost::send_transport`]), and reads back the playhead it
//! publishes ([`AudioHost::transport_state`]). When the stream closes, the
//! engine goes with it, and the next one starts with the same document and
//! the song where it was, stopped (the control bar transport spec,
//! R16). The stream's callback ([`EngineCallback`]) runs the engine's
//! processor, handing it the device's input for tracks whose Audio From is
//! Ext. In and playing the engine's Main output on the device channels Main
//! Out names (WP-2.13); it plays the Settings window's test tone in its
//! place while the tone is on, at the Tone Volume and Tone Frequency the
//! person set (D23), adds the browser's preview voice (`bl_engine::preview`,
//! WP-1.10), and measures how long it all takes, which the CPU meter shows.
//! It runs on the audio thread, so it follows the real-time rules: no
//! allocation, no lock, no blocking call (`tests/realtime.rs` checks it
//! under the allocation trap, and `rtsan/tests/shell.rs` under
//! RealtimeSanitizer).
//!
//! The settings and reports themselves are data in `bl-shell-spec`,
//! re-exported here.

use std::f64::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

use bl_audio_io::{
    AudioBuffer, AudioBufferMut, AudioCallback, Backend, BackendKind, BufferSizes, DeviceId,
    DeviceInfo, DeviceSelector, Error, PortRequest, ProcessContext, Stream, StreamInfo,
    StreamRequest, StreamStats,
};
use bl_core::clock::TestClock;
use bl_core::time::{SampleRate, Ticks};
use bl_engine::preview::{
    PreviewControl, PreviewSamples, PreviewShared, PreviewStatus, PreviewTransport, PreviewVoice,
    WarpedPreview, preview_channel,
};
use bl_engine::{
    Collector, DROP_PERIOD, DropThread, Engine, EngineConfig, MediaLibrary, MediaStreams, Pool,
    PoolConfig, Processor, RecordingPlan, Registry, Report, ScheduledCommand, SessionCommand,
    SlotStatus, TakeBounds, TransportCommand, When,
};
use bl_media::{MediaStore, StreamerStats};
use bl_model::{Channel, Document, LaunchSettings, TrackId};
pub use bl_shell_spec::audio::{
    AudioDevices, AudioSettings, AudioStatus, COMPENSATION_LIMIT_MS, ConfigureAudio, DeviceListing,
    DeviceRef, DriverInfo, TONE_DEFAULT_DBFS, TONE_DEFAULT_HZ, TONE_MAX_DBFS, TONE_MAX_HZ,
    TONE_MIN_DBFS, TONE_MIN_HZ, ToneSettings, clamped_compensation_ms,
};
use rtrb::{Consumer, Producer, RingBuffer};

/// A device as `audio.devices` lists it.
fn listing(info: &DeviceInfo) -> DeviceListing {
    let (buffer_sizes, buffer_range) = match &info.buffer_sizes {
        BufferSizes::Range { min, max } => (vec![*min, *max], true),
        BufferSizes::List(sizes) => (sizes.clone(), false),
        BufferSizes::Unknown => (Vec::new(), false),
    };
    DeviceListing {
        id: info.id.to_string(),
        name: info.name.clone(),
        output_channels: info.output_channels,
        input_channels: info.input_channels,
        default_output: info.is_default_output,
        default_input: info.is_default_input,
        sample_rates: info.sample_rates.clone(),
        current_sample_rate: info.current_sample_rate,
        buffer_sizes,
        buffer_range,
        default_buffer_size: info.default_buffer_size,
    }
}

/// Opens a backend of a kind. The app uses `bl_audio_io::open_backend`;
/// tests pass their own.
pub type BackendFactory = Box<dyn Fn(BackendKind) -> Result<Box<dyn Backend>, Error>>;

/// What the audio thread and the rest of the app share: relaxed atomics
/// only, so the audio thread never waits.
#[derive(Debug)]
pub struct EngineShared {
    /// Nanoseconds the callback spent processing.
    busy_ns: AtomicU64,
    /// Nanoseconds of audio the callback produced.
    audio_ns: AtomicU64,
    /// Whether the test tone plays.
    tone: AtomicBool,
    /// The test tone's peak amplitude, as `f32` bits.
    tone_amplitude: AtomicU32,
    /// The test tone's frequency in hertz, as `f32` bits.
    tone_hz: AtomicU32,
    /// The 0-based device output channel Main's left channel goes to (Main
    /// Out, WP-2.13); its right channel follows it.
    main_out: AtomicU32,
}

impl Default for EngineShared {
    fn default() -> Self {
        let shared = Self {
            busy_ns: AtomicU64::new(0),
            audio_ns: AtomicU64::new(0),
            tone: AtomicBool::new(false),
            tone_amplitude: AtomicU32::new(0),
            tone_hz: AtomicU32::new(0),
            main_out: AtomicU32::new(0),
        };
        shared.set_tone(ToneSettings::default());
        shared
    }
}

impl EngineShared {
    /// Switches the test tone on or off.
    pub fn set_tone_on(&self, on: bool) {
        self.tone.store(on, Relaxed);
    }

    /// Sets the test tone's level and frequency, within the controls'
    /// ranges.
    pub fn set_tone(&self, tone: ToneSettings) {
        let tone = tone.clamped();
        self.tone_amplitude
            .store(tone.amplitude().to_bits(), Relaxed);
        self.tone_hz
            .store((tone.frequency_hz as f32).to_bits(), Relaxed);
    }

    /// Sets the device channel Main's left channel goes to, from the set's
    /// Main Out.
    pub fn set_main_out(&self, left: u32) {
        self.main_out.store(left, Relaxed);
    }

    /// Nanoseconds of audio the callback has produced.
    #[must_use]
    pub fn audio_ns(&self) -> u64 {
        self.audio_ns.load(Relaxed)
    }
}

/// The left device output channel a set's Main Out names, counted from 0
/// there (the set counts from 1), or 0 for anything else.
fn main_out_left(document: &Document) -> u32 {
    match document
        .track(document.main_id())
        .map(|t| t.routing().main_out)
    {
        Some(Channel::Pair(channel)) => u32::from(channel.saturating_sub(1)),
        _ => 0,
    }
}

/// The audio thread's work: the engine's processor, once it arrives, and
/// the test tone in its place while the tone is on, timed for the CPU
/// meter. It allocates nothing, takes no lock, and makes no system call
/// besides reading the clock.
pub struct EngineCallback {
    shared: Arc<EngineShared>,
    phase: f64,
    /// Where the engine's processor arrives, once the stream has opened at
    /// the rate the engine is made for.
    incoming: Option<Consumer<Processor>>,
    /// The engine, once it has arrived. Until then, and without one, the
    /// output stays silent.
    processor: Option<Processor>,
    /// The browser's preview voice, mixed in after the tone (WP-1.10).
    preview: Option<PreviewVoice>,
    /// With the null device paced by hand, the clock the block's start is
    /// timed from, so MIDI lands on its frame however busy the machine is
    /// (T199). Without one, the block starts now.
    clock: Option<TestClock>,
}

impl EngineCallback {
    /// A callback without an engine, reading the tone settings from
    /// `shared` and adding its timings there: silence, or the test tone.
    #[must_use]
    pub fn new(shared: Arc<EngineShared>) -> Self {
        Self {
            shared,
            phase: 0.0,
            incoming: None,
            processor: None,
            preview: None,
            clock: None,
        }
    }

    /// A callback that plays the engine whose processor arrives through
    /// `incoming` (it takes it at the start of the first block it finds it
    /// there), with the test tone in its place while the tone is on.
    #[must_use]
    pub fn with_engine(shared: Arc<EngineShared>, incoming: Consumer<Processor>) -> Self {
        Self {
            incoming: Some(incoming),
            ..Self::new(shared)
        }
    }

    /// The same, playing the browser's previews through `preview` too
    /// (WP-1.10).
    #[must_use]
    pub fn with_preview(mut self, preview: PreviewVoice) -> Self {
        self.preview = Some(preview);
        self
    }

    /// The same, timing each block from `clock` instead of the system's, for
    /// the null device's manual pacing (T199).
    #[must_use]
    pub fn with_test_clock(mut self, clock: TestClock) -> Self {
        self.clock = Some(clock);
        self
    }
}

impl AudioCallback for EngineCallback {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        input: &AudioBuffer<'_>,
        output: &mut AudioBufferMut<'_>,
    ) {
        let began = Instant::now();
        // With the null device paced by hand, the block's start is the test
        // clock's time (T199); the system's clock still measures how long the
        // callback took.
        let started = self.clock.as_ref().map_or(began, TestClock::now);
        if self.processor.is_none()
            && let Some(incoming) = self.incoming.as_mut()
        {
            self.processor = incoming.pop().ok();
        }
        if let Some(processor) = self.processor.as_mut() {
            // The engine runs in blocks of at most its own size. Its Main
            // output goes to the device channels Main Out names (WP-2.13),
            // and its tracks' Ext. In inputs come from the device's input.
            let main_out = self.shared.main_out.load(Relaxed) as usize;
            let channels = output.channels();
            let mut done = 0;
            while done < ctx.frames {
                let frames = (ctx.frames - done).min(processor.config().max_frames);
                // The block's callback time, on the clock MIDI input is
                // stamped against: the callback's start, moved on by the
                // blocks before it in this callback (docs/specs/midi-io.md,
                // R15, R16).
                let block_at = started
                    + Duration::from_secs_f64(done as f64 / f64::from(ctx.sample_rate.max(1)));
                let block = processor.process_at(
                    frames,
                    &InputPart {
                        input,
                        offset: done,
                        frames,
                    },
                    block_at,
                );
                for c in 0..block.channels() {
                    let to = main_out + c;
                    if to < channels {
                        output.channel_mut(to)[done..done + frames]
                            .copy_from_slice(block.channel(c));
                    }
                }
                // Tracks on Ext. Out add to the channels they name, Main
                // Out's included (the routing spec, R3).
                for c in 0..block.ext_channels().min(channels) {
                    let ext = block.ext_channel(c);
                    for (to, from) in output.channel_mut(c)[done..done + frames]
                        .iter_mut()
                        .zip(ext)
                    {
                        *to += from;
                    }
                }
                done += frames;
            }
        }
        if self.shared.tone.load(Relaxed) {
            let amplitude = f32::from_bits(self.shared.tone_amplitude.load(Relaxed));
            let hz = f64::from(f32::from_bits(self.shared.tone_hz.load(Relaxed)));
            let step = hz / f64::from(ctx.sample_rate.max(1));
            let mut channels = output.iter_mut();
            if let Some(first) = channels.next() {
                for sample in first.iter_mut() {
                    *sample = (self.phase * TAU).sin() as f32 * amplitude;
                    self.phase += step;
                    if self.phase >= 1.0 {
                        self.phase -= 1.0;
                    }
                }
                for channel in channels {
                    channel.copy_from_slice(first);
                }
            }
        }
        if let Some(preview) = &mut self.preview {
            let transport = self.processor.as_ref().map(|p| PreviewTransport {
                playing: p.playing(),
                song_position: p.position(),
                tempo_map: p.tempo_map(),
                time_signatures: p.time_signatures(),
                loop_frames: p.loop_region().map(|r| {
                    (
                        p.tempo_map()
                            .ticks_to_frames(r.start, p.config().sample_rate),
                        p.tempo_map().ticks_to_frames(r.end, p.config().sample_rate),
                    )
                }),
                sample_rate: p.config().sample_rate,
                launch: p.launch(),
            });
            preview.render_with_transport(ctx.frames, output.iter_mut(), transport);
        }
        let busy = u64::try_from(began.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let audio = ctx.frames as u64 * 1_000_000_000 / u64::from(ctx.sample_rate.max(1));
        self.shared.busy_ns.fetch_add(busy, Relaxed);
        self.shared.audio_ns.fetch_add(audio, Relaxed);
    }
}

/// The device's input for one part of a block, as the engine reads it: a
/// view of the callback's input buffer from `offset` for `frames` frames.
struct InputPart<'a> {
    input: &'a AudioBuffer<'a>,
    offset: usize,
    frames: usize,
}

impl bl_engine::DeviceInput for InputPart<'_> {
    fn channels(&self) -> usize {
        self.input.channels()
    }

    fn channel(&self, index: usize) -> &[f32] {
        &self.input.channel(index)[self.offset..self.offset + self.frames]
    }
}

/// How long opening a device waits for the engine's compiler to put the
/// set and the transport on the audio thread's queue, so the first block
/// takes them all at once.
const ENGINE_START_TIMEOUT: Duration = Duration::from_secs(5);

/// How many MIDI messages can wait for the engine's next block; the engine
/// takes at most `MIDI_EVENTS_PER_BLOCK` of them a block.
const MIDI_QUEUE_MESSAGES: usize = 4096;

/// How many messages the queue from the engine to the MIDI output ports
/// holds (the external devices spec, R5).
const MIDI_OUT_MESSAGES: usize = 4096;

// Spec: midi-clock R13. Room for the notes of a block and, for every output
// with Sync on, the clocks of a 4,096-frame block at 44.1 kHz at 999 BPM
// (38) and of 100 ms written ahead for a negative sync delay (40), rounded
// to 80.
const _: () = assert!(
    MIDI_OUT_MESSAGES >= bl_engine::MIDI_EVENTS_PER_BLOCK + bl_engine::MAX_SYNC_OUTPUTS * 80
);

/// The transport as the engine last published it: what the Control Bar's
/// transport section and the Arrangement show. While no engine runs, it's
/// the transport as the last one left it, stopped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EngineTransport {
    /// Whether an engine runs on an open device, so the transport can play.
    pub running: bool,
    /// Whether the song plays.
    pub playing: bool,
    /// The playhead, in ticks: where the song plays, or where it stopped.
    pub position: Ticks,
    /// The insert marker, where Play starts.
    pub insert_marker: Ticks,
    /// Frames the running engine has processed (its clock); 0 without one.
    pub clock: u64,
    /// Whether the transport records (WP-2.13).
    pub recording: bool,
    /// Whether a take is running: the transport records and has begun to
    /// write the armed tracks' input.
    pub take_running: bool,
    /// Whether the count-in is sounding.
    pub counting_in: bool,
    /// The take that last ended, until the next recording starts: the song
    /// frames it covers, which the recorded clips are made from.
    pub take: Option<TakeBounds>,
    /// The take running now, as far as it has got (WP-3.11).
    pub take_so_far: Option<TakeBounds>,
}

/// The audio device the app plays through, the engine that plays the set
/// on it, and the settings that choose the device.
pub struct AudioHost {
    available: Vec<BackendKind>,
    factory: BackendFactory,
    settings: AudioSettings,
    tone: ToneSettings,
    engine_on: bool,
    backend: Option<(BackendKind, Box<dyn Backend>)>,
    stream: Option<Box<dyn Stream>>,
    /// The engine playing through `stream`. Declared after it, so the
    /// stream, and the processor in its callback, goes first.
    engine: Option<Engine>,
    /// The set the engine plays: the newest document synced.
    document: Option<Document>,
    /// The transport as the engine last showed it, to restore when the next
    /// engine starts.
    memory: EngineTransport,
    /// The global launch quantisation and its enabled switch (WP-3.2): the
    /// set's, sent to the engine, so launching waits for the right grid.
    launch: LaunchSettings,
    shared: Arc<EngineShared>,
    error: Option<String>,
    /// The counters at the last CPU sample, and the load measured then.
    last_sample: (u64, u64),
    cpu_load: Option<f32>,
    /// The browser's preview (WP-1.10): the volume and progress, kept
    /// across streams, and the open stream's control.
    preview_shared: Arc<PreviewShared>,
    preview: Option<PreviewControl>,
    /// Frees what the audio thread lets go of, such as finished previews.
    drop_thread: Option<DropThread>,
    /// The set's media, which the engine's clips stream from (WP-1.4b).
    media: Option<Arc<MediaStore>>,
    /// The running engine's streams, for their figures.
    streams: Option<Arc<MediaStreams>>,
    /// The shell's analyses, which give the engine the files' transients
    /// (the warp modes spec, R14).
    analysis: crate::analysed_media::AnalysisSlot,
    /// How many engines have started, so a host can tell a new one, which
    /// holds none of the commands scheduled on the last (WP-1.15b).
    engines: u64,
    /// Reports from engines that have closed, not yet taken.
    held_reports: Vec<Report>,
    /// Captured notes from closed engines, not yet read (WP-3.14).
    held_captured: Vec<bl_engine::CapturedMidi>,
    /// How many engines have been closed: a recording that began on an
    /// earlier one has lost its take.
    engines_closed: u64,
    /// The CLAP host whose factory makes each engine's CLAP nodes
    /// (WP-2.6).
    plugins: Option<bl_plugin_host::clap::ClapHost>,
    /// The Audio Unit host (WP-2.8), when the shell has one: this platform
    /// may not host them.
    au: Option<bl_plugin_host::au::AuHost>,
    /// The VST3 host whose factory makes each engine's VST3 nodes
    /// (WP-2.7).
    vst3: Option<bl_plugin_host::vst3::Vst3Host>,
    /// The LV2 host whose factory makes each engine's LV2 nodes (WP-5.39).
    lv2: Option<bl_plugin_host::lv2::Lv2Host>,
    kit: Option<bl_devices::KitHost>,
    /// Where MIDI input arrives (WP-2.2): each engine's processor gets a
    /// fresh queue from it.
    midi: Option<bl_midi_io::InputSender>,
    /// The bus MIDI output leaves on (WP-5.34): each engine's processor
    /// attaches its own queue to it.
    midi_out: Option<bl_midi_io::OutputBus>,
    /// The output ports with Sync on, which every engine's clock out goes to
    /// (WP-7.11).
    sync_outputs: Vec<bl_core::midi::SyncOutput>,
    /// The output latency and round trip, in frames, the engine was last
    /// told (WP-5.34).
    latency_sent: (u32, u32),
    /// A test clock for the next stream's blocks, with the null device's
    /// manual pacing (T199).
    test_clock: Option<TestClock>,
}

impl std::fmt::Debug for AudioHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioHost")
            .field("settings", &self.settings)
            .field("engine_on", &self.engine_on)
            .field("open", &self.stream.is_some())
            .field("transport", &self.memory)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl AudioHost {
    /// A host on this machine's backends, with these settings. Nothing
    /// opens until [`AudioHost::start`].
    #[must_use]
    pub fn new(settings: AudioSettings) -> Self {
        Self::with_backends(
            bl_audio_io::available_backends(),
            Box::new(bl_audio_io::open_backend),
            settings,
        )
    }

    /// A host on the null device only, for tests and headless runs: it runs
    /// the whole stream path on a timer, without hardware.
    #[must_use]
    pub fn null(settings: AudioSettings) -> Self {
        let settings = AudioSettings {
            driver: Some(BackendKind::Null.id().to_owned()),
            ..settings
        };
        Self::with_backends(
            vec![BackendKind::Null],
            Box::new(bl_audio_io::open_backend),
            settings,
        )
    }

    /// A host on these backends, opened by `factory`.
    #[must_use]
    pub fn with_backends(
        available: Vec<BackendKind>,
        factory: BackendFactory,
        settings: AudioSettings,
    ) -> Self {
        Self {
            available,
            factory,
            settings,
            tone: ToneSettings::default(),
            engine_on: true,
            backend: None,
            stream: None,
            engine: None,
            document: None,
            memory: EngineTransport::default(),
            launch: LaunchSettings::default(),
            shared: Arc::new(EngineShared::default()),
            error: None,
            last_sample: (0, 0),
            cpu_load: None,
            preview_shared: Arc::new(PreviewShared::default()),
            preview: None,
            drop_thread: None,
            media: None,
            streams: None,
            analysis: crate::analysed_media::AnalysisSlot::default(),
            engines: 0,
            held_reports: Vec::new(),
            held_captured: Vec::new(),
            engines_closed: 0,
            plugins: None,
            au: None,
            vst3: None,
            lv2: None,
            kit: None,
            midi: None,
            midi_out: None,
            sync_outputs: Vec::new(),
            latency_sent: (0, 0),
            test_clock: None,
        }
    }

    /// Gives every engine from now on the queue `sender` fills with MIDI
    /// input (WP-2.2; the MIDI I/O spec, R14).
    pub fn set_midi(&mut self, sender: bl_midi_io::InputSender) {
        self.midi = Some(sender);
    }

    /// Times the next stream's blocks from `clock` instead of the system's,
    /// for the null device's manual pacing (T199). It applies to a stream
    /// opened after it, as [`AudioHost::set_midi`] does for its queue.
    pub fn set_test_clock(&mut self, clock: TestClock) {
        self.test_clock = Some(clock);
    }

    /// Gives every engine from now on the queue it sends MIDI to output
    /// ports on (WP-5.34; the external devices spec, R5).
    pub fn set_midi_output(&mut self, bus: bl_midi_io::OutputBus) {
        self.midi_out = Some(bus);
    }

    /// Sets the output ports with Sync on and their options, which the
    /// engine sends MIDI clock to (the MIDI clock spec, R3): now, and for
    /// every engine that starts.
    pub fn set_sync_outputs(&mut self, outputs: Vec<bl_core::midi::SyncOutput>) {
        if outputs == self.sync_outputs {
            return;
        }
        if let Some(engine) = &self.engine {
            engine.sync_outputs(&outputs);
        }
        self.sync_outputs = outputs;
    }

    /// What the engine reports of the MIDI clock it sends; all zeros while
    /// none runs.
    pub fn sync_out_state(&mut self) -> bl_engine::SyncOutState {
        self.engine
            .as_mut()
            .map_or_else(Default::default, |engine| engine.state().sync_out)
    }

    /// Plays clips' audio from `media`, streaming long files from disk, from
    /// the next engine that starts (the audio clips spec, R16). Until then,
    /// and without it, clips play silence.
    pub fn set_media(&mut self, media: Arc<MediaStore>) {
        self.media = Some(media);
    }

    /// Gives every engine from now on the files' transients as `analysis`
    /// reads them, which Beats clips cut at (the warp modes spec, R14).
    pub fn set_analysis(&mut self, analysis: crate::analysed_media::AnalysisSlot) {
        self.analysis = analysis;
    }

    /// Tells the running engine `media`'s analysis is ready, so the clips
    /// that play it are compiled again with its transients. Nothing while
    /// no engine runs: the next one asks when it compiles them.
    pub fn media_analysed(&self, media: bl_model::ContentHash) {
        if let Some(engine) = &self.engine {
            engine.media_analysed(media);
        }
    }

    /// Gives every engine from now on the CLAP host's plug-ins (WP-2.6).
    pub fn set_plugins(&mut self, plugins: bl_plugin_host::clap::ClapHost) {
        self.plugins = Some(plugins);
    }

    /// Gives the Audio Unit host, whose units the engine's registry asks
    /// for nodes (WP-2.8).
    pub fn set_au(&mut self, au: bl_plugin_host::au::AuHost) {
        self.au = Some(au);
    }

    /// Gives every engine from now on the VST3 host's plug-ins (WP-2.7).
    pub fn set_vst3(&mut self, vst3: bl_plugin_host::vst3::Vst3Host) {
        self.vst3 = Some(vst3);
    }

    /// Gives every engine from now on the LV2 host's plug-ins (WP-5.39).
    pub fn set_lv2(&mut self, lv2: bl_plugin_host::lv2::Lv2Host) {
        self.lv2 = Some(lv2);
    }

    /// Gives every engine from now on the kit plug-ins compiled in.
    pub fn set_kit(&mut self, kit: bl_devices::KitHost) {
        self.kit = Some(kit);
    }

    /// The running engine's disk streaming figures: open streams, underruns
    /// (what the disk overload indicator shows), and how busy the disk
    /// thread is. `None` while no engine runs.
    #[must_use]
    pub fn disk_stats(&self) -> Option<StreamerStats> {
        self.engine
            .as_ref()
            .and(self.streams.as_ref())
            .map(|streams| streams.stats())
    }

    /// The settings in force.
    #[must_use]
    pub fn settings(&self) -> &AudioSettings {
        &self.settings
    }

    /// Whether the audio engine is on.
    #[must_use]
    pub fn engine_on(&self) -> bool {
        self.engine_on
    }

    /// Whether a stream is open and running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.stream.is_some()
    }

    /// The driver types on offer, best first.
    #[must_use]
    pub fn drivers(&self) -> &[BackendKind] {
        &self.available
    }

    /// The driver type the settings choose: the named one if this machine
    /// has it, otherwise the platform's default, otherwise the first offered.
    #[must_use]
    pub fn driver(&self) -> BackendKind {
        self.settings
            .driver
            .as_deref()
            .and_then(BackendKind::from_id)
            .filter(|kind| self.available.contains(kind))
            .or_else(|| {
                let default = bl_audio_io::default_backend();
                self.available.contains(&default).then_some(default)
            })
            .or_else(|| self.available.first().copied())
            .unwrap_or(BackendKind::Null)
    }

    /// Opens the device if the engine is on. A failure is kept for
    /// [`AudioHost::status`] and returned.
    ///
    /// # Errors
    ///
    /// The device's error, as text.
    pub fn start(&mut self) -> Result<(), String> {
        if !self.engine_on || self.stream.is_some() {
            return Ok(());
        }
        let result = self.open();
        self.error = result.as_ref().err().cloned();
        result
    }

    /// Closes the device, and with it the engine.
    pub fn stop(&mut self) {
        self.close();
        self.cpu_load = None;
    }

    /// Closes the stream, then its engine, remembering where the transport
    /// was for the next engine.
    fn close(&mut self) {
        let _ = self.transport_state();
        self.memory.playing = false;
        if let Some(engine) = self.engine.as_mut() {
            self.held_reports.extend(engine.reports());
            self.held_captured.extend(engine.captured());
        }
        // The processor goes with the stream's callback; then the engine's
        // threads stop, and its drop thread frees the last program.
        self.stream = None;
        self.preview = None;
        self.engines_closed += u64::from(self.engine.take().is_some());
    }

    /// How many engines have been closed so far.
    #[must_use]
    pub fn engines_closed(&self) -> u64 {
        self.engines_closed
    }

    /// Whether an engine runs on an open device, so the transport can play.
    #[must_use]
    pub fn engine_running(&self) -> bool {
        self.stream.is_some() && self.engine.is_some()
    }

    /// Why the engine can't be reached, when it can't: the reason a command
    /// that needs it is refused.
    #[must_use]
    pub fn engine_refusal(&self) -> Option<String> {
        self.running_engine().err()
    }

    /// The engine's compiler counters — how many programs it compiled, and
    /// so on — for diagnostics and tests. `None` while no engine runs.
    #[must_use]
    pub fn compile_counts(&self) -> Option<bl_engine::CompileCounts> {
        self.engine.as_ref().map(Engine::counts)
    }

    /// Gives the engine the newest document: its compiler brings the
    /// running program up to date (tracks, devices, the tempo map, the
    /// loop). The host keeps it for the next engine too.
    pub fn sync(&mut self, document: &Document) {
        self.document = Some(document.clone());
        self.shared.set_main_out(main_out_left(document));
        if let Some(engine) = &self.engine {
            engine.sync(document);
        }
    }

    /// Tells the engine the node of `device` now reports `frames` of
    /// latency, as a plug-in that restarted for a new one does: the
    /// compiler compiles a program whose delay compensation uses it
    /// (WP-2.7; delay-compensation R6). Nothing while no engine runs: the
    /// nodes the next one makes read their plug-ins' latency themselves.
    pub fn set_device_latency(&mut self, device: bl_model::DeviceId, frames: u32) {
        if let Some(engine) = &self.engine {
            engine.set_device_latency(device, frames);
        }
    }

    /// The devices whose node reports another latency than the engine
    /// compensates for, as a native device's look-ahead moves it (WP-2.11):
    /// each goes to [`set_device_latency`](Self::set_device_latency). Empty
    /// while no engine runs.
    pub fn latency_changes(&mut self) -> Vec<(bl_model::DeviceId, u32)> {
        self.engine
            .as_mut()
            .map_or_else(Vec::new, |engine| engine.state().latency_changes.clone())
    }

    /// Sends a transport command, which the engine runs at the start of its
    /// next block.
    ///
    /// # Errors
    ///
    /// Why nothing can play, while no engine runs: the audio engine is off,
    /// or no device is open.
    pub fn send_transport(&mut self, command: TransportCommand) -> Result<(), String> {
        self.running_engine()?.transport(command);
        Ok(())
    }

    /// Sends a transport command for a frame of the engine clock, which it
    /// runs on that exact frame ([`When::Frame`]).
    ///
    /// # Errors
    ///
    /// As [`AudioHost::send_transport`].
    pub fn send_transport_at(&mut self, command: TransportCommand, at: u64) -> Result<(), String> {
        self.running_engine()?
            .transport_at(command, When::Frame(at));
        Ok(())
    }

    /// The frame of the engine clock that `time` falls on, as MIDI input is
    /// placed (none before the engine's first block), and the rate the clock
    /// counts at.
    ///
    /// # Errors
    ///
    /// As [`AudioHost::send_transport`].
    pub fn clock_frame_at(&mut self, time: Instant) -> Result<(Option<u64>, u32), String> {
        self.running_engine()?;
        let state = self.engine.as_mut().map(Engine::state);
        Ok(state.map_or((None, 0), |state| {
            (
                state.time_base.map(|base| base.frame_at(time)),
                state.sample_rate.hz(),
            )
        }))
    }

    /// Sends a sampler's node a click in its display (WP-5.15, the
    /// multisampler spec, R54): the node plays its sample from a point until
    /// the button comes up. Nothing is written to the set.
    ///
    /// # Errors
    ///
    /// Why nothing can play, while no engine runs, as
    /// [`AudioHost::send_transport`] says.
    pub fn audition(
        &mut self,
        device: bl_model::DeviceId,
        audition: bl_engine::Audition,
    ) -> Result<(), String> {
        let engine = self.running_engine()?;
        engine.send_data(bl_engine::NodeKey::Device(device), engine.data(audition));
        Ok(())
    }

    /// Sends a Session View command (WP-3.2): a slot or a scene launch, or
    /// a stop, which the engine runs at the start of its next block and
    /// lands on the launch quantisation grid.
    ///
    /// # Errors
    ///
    /// Why nothing can play, while no engine runs, as
    /// [`AudioHost::send_transport`] says.
    pub fn send_session(&mut self, command: SessionCommand) -> Result<(), String> {
        self.running_engine()?.session(command);
        Ok(())
    }

    /// Keeps the global launch quantisation and its switch (WP-3.2), telling
    /// the engine that runs and the engine that starts next: launching and
    /// stopping wait for the grid it sets. The set saves it beside the song.
    pub fn set_launch_settings(&mut self, settings: LaunchSettings) {
        self.launch = settings;
        let _ = self.send_session(SessionCommand::SetQuantization(settings));
    }

    /// The global launch quantisation and its switch, as last set.
    #[must_use]
    pub fn launch_settings(&self) -> LaunchSettings {
        self.launch
    }

    /// The Session slots' states the engine last published (WP-3.2): one
    /// entry per queued, playing, or stopping slot, which the UI draws the
    /// launch buttons from. Empty once every slot has stopped, and while no
    /// engine runs.
    pub fn session_into(&mut self, out: &mut Vec<SlotStatus>) {
        out.clear();
        if let Some(engine) = self.engine.as_mut() {
            out.extend_from_slice(&engine.state().session);
        }
    }

    /// The tempo and time signature a launched scene put in place of the
    /// set's, as the engine last published them (WP-3.5). Nothing while no
    /// engine runs.
    pub fn scene_override(&mut self) -> bl_engine::SceneOverride {
        self.engine
            .as_mut()
            .map_or_else(Default::default, |engine| engine.state().scene_override)
    }

    /// The tracks a Session clip has taken over from the Arrangement
    /// (WP-3.6) as the engine last published them: the Back to Arrangement
    /// buttons light from this. Empty once every track returned, and while no
    /// engine runs.
    pub fn session_over_into(&mut self, out: &mut Vec<TrackId>) {
        out.clear();
        if let Some(engine) = self.engine.as_mut() {
            out.extend_from_slice(&engine.state().session_over);
        }
    }

    /// The parameters under modulation as the engine last published them
    /// (WP-5.9): what the knobs' rings and the faders' marks draw from.
    /// Empty while the song doesn't play, and while no engine runs.
    pub fn modulated_into(&mut self, out: &mut Vec<bl_engine::PlayedParam>) {
        out.clear();
        if let Some(engine) = self.engine.as_mut() {
            out.extend_from_slice(&engine.state().modulated);
        }
    }

    /// The rack chains the engine found processing in the last block
    /// (WP-5.3): what Auto Select follows. Empty while no engine runs.
    pub fn processing_into(&mut self, out: &mut Vec<bl_model::ChainId>) {
        out.clear();
        if let Some(engine) = self.engine.as_mut() {
            out.extend_from_slice(&engine.state().processing);
        }
    }

    /// Sends the engine's node `key` a parameter change that takes effect
    /// at the start of the next block, as a held Preview button does for a
    /// drum chain (the drum racks spec, R5). Nothing while no engine runs.
    pub fn send_param(&self, key: bl_engine::NodeKey, param: bl_model::ParamId, value: f64) {
        if let Some(engine) = &self.engine {
            engine.send_param(bl_engine::ParamEvent::now(key, param, value));
        }
    }

    /// How many notes have reached each drum chain of the running program
    /// (the drum racks spec, R20); empty while no engine runs.
    pub fn chain_hits_into(&self, out: &mut std::collections::BTreeMap<bl_model::ChainId, u64>) {
        out.clear();
        if let Some(engine) = &self.engine {
            engine.chain_hits().read_all(out);
        }
    }

    /// Gives the engine what to record while the transport records
    /// (WP-2.13): the armed tracks' inputs and the rings their samples go
    /// to. It replaces any earlier plan; the take that ends drops it, which
    /// closes the rings.
    ///
    /// # Errors
    ///
    /// Why nothing can record, while no engine runs: the audio engine is
    /// off, or no device is open.
    pub fn record(&mut self, plan: RecordingPlan) -> Result<(), String> {
        self.running_engine()?.record(plan);
        Ok(())
    }

    /// Sets what the engine captures while the song plays a range in real
    /// time (WP-5.35): the rendered outputs and the rings their samples go
    /// to.
    ///
    /// # Errors
    ///
    /// Why nothing can render in real time, while no engine runs, as
    /// [`AudioHost::send_transport`] says.
    pub fn render(&mut self, plan: bl_engine::RenderPlan) -> Result<(), String> {
        self.running_engine()?.render(plan);
        Ok(())
    }

    /// Schedules a command at a musical position on the engine's queue,
    /// the queue the transport's commands take, so they arrive in the order
    /// they were sent (the agent API's scheduled commands, WP-1.15b).
    /// The engine runs its action on the exact frame the song reaches it,
    /// and reports it ([`AudioHost::take_reports`]).
    ///
    /// # Errors
    ///
    /// Why nothing can play, while no engine runs, as
    /// [`AudioHost::send_transport`] says.
    pub fn schedule(&mut self, command: ScheduledCommand) -> Result<(), String> {
        self.running_engine()?.schedule(command);
        Ok(())
    }

    /// Cancels a scheduled command that hasn't fired.
    ///
    /// # Errors
    ///
    /// Why nothing can play, while no engine runs.
    pub fn cancel(&mut self, id: u64) -> Result<(), String> {
        self.running_engine()?.cancel(id);
        Ok(())
    }

    /// The notes monitoring MIDI tracks received since the last call, for
    /// Capture MIDI (WP-3.14), the closed engines' first.
    pub fn take_captured(&mut self) -> Vec<bl_engine::CapturedMidi> {
        let mut notes = std::mem::take(&mut self.held_captured);
        if let Some(engine) = self.engine.as_mut() {
            notes.extend(engine.captured());
        }
        notes
    }

    /// The engine's reports since the last call, the closed engines' first:
    /// scheduled commands that fired, each with its frame, or that it had no
    /// room for.
    pub fn take_reports(&mut self) -> Vec<Report> {
        let mut reports = std::mem::take(&mut self.held_reports);
        if let Some(engine) = self.engine.as_mut() {
            reports.extend(engine.reports());
        }
        reports
    }

    /// Waits until everything sent to the running engine so far, the last
    /// document change among it, has reached the audio thread, which takes
    /// it at its next block. Returns whether it did within `timeout`; with
    /// no engine running there's nothing to wait for. Tests use it before
    /// playing into a track they just armed.
    pub fn flush(&self, timeout: std::time::Duration) -> bool {
        self.engine
            .as_ref()
            .is_none_or(|engine| engine.flush(timeout))
    }

    /// How many engines have started: each time a device opens, a new
    /// engine starts with none of the last one's scheduled commands.
    #[must_use]
    pub fn engine_generation(&self) -> u64 {
        self.engines
    }

    fn running_engine(&self) -> Result<&Engine, String> {
        match &self.engine {
            Some(engine) if self.stream.is_some() => Ok(engine),
            _ if !self.engine_on => Err(
                "the audio engine is off; switch it on with `audio.toggle_engine` first".to_owned(),
            ),
            _ => Err(format!(
                "no audio device is open{}; choose one in the Audio settings",
                self.error
                    .as_deref()
                    .map(|error| format!(" ({error})"))
                    .unwrap_or_default()
            )),
        }
    }

    /// Moves the insert marker without moving the playhead
    /// ([`TransportCommand::SetInsertMarker`]), as a click in the
    /// Arrangement does. Without an engine, the next one starts with it.
    pub fn set_insert_marker(&mut self, at: Ticks) {
        let at = at.max(Ticks::ZERO);
        self.memory.insert_marker = at;
        let _ = self.send_transport(TransportCommand::SetInsertMarker(at));
    }

    /// Stops the song and returns it to 1.1.1, as a new or opened set
    /// starts: Stop, then Stop again.
    pub fn reset_transport(&mut self) {
        self.memory = EngineTransport::default();
        let _ = self.send_transport(TransportCommand::Stop);
        let _ = self.send_transport(TransportCommand::Stop);
    }

    /// Whether Arrangement Record was on in the last block the engine
    /// published.
    #[must_use]
    pub fn recording(&self) -> bool {
        self.memory.recording
    }

    /// The transport as the engine last published it, all from the same
    /// block: whether the song plays, the playhead, and the insert marker.
    /// Reading takes no lock. Before a new engine's first block, and while
    /// none runs, it's the transport as the last engine left it, stopped.
    pub fn transport_state(&mut self) -> EngineTransport {
        let running = self.stream.is_some();
        let Some(engine) = self.engine.as_mut() else {
            return EngineTransport {
                running: false,
                playing: false,
                clock: 0,
                ..self.memory
            };
        };
        let state = engine.state();
        if state.program.is_none() {
            // The first block hasn't run: the set and the restored
            // transport are still on their way to it.
            return EngineTransport {
                running,
                playing: false,
                clock: state.clock,
                ..self.memory
            };
        }
        self.memory = EngineTransport {
            running,
            playing: state.playing,
            position: state.position_ticks,
            insert_marker: state.insert_marker,
            clock: state.clock,
            recording: state.recording,
            take_running: state.take_running,
            counting_in: state.counting_in,
            take: state.take,
            take_so_far: state.take_so_far,
        };
        self.memory
    }

    /// Copies the meters the engine last published into `out`, one per
    /// track (the mixer spec, R19), reusing its capacity. Empty while no
    /// engine runs or before its first block.
    pub fn meters_into(&mut self, out: &mut Vec<bl_engine::TrackMeter>) {
        out.clear();
        if let Some(engine) = self.engine.as_mut() {
            let state = engine.state();
            if state.program.is_some() {
                out.extend_from_slice(&state.meters);
            }
        }
    }

    /// Copies the meters between devices the engine last published into
    /// `out` (the MIDI meters spec, R6), reusing its capacity. Empty while no
    /// engine runs or before its first block.
    pub fn chain_meters_into(&mut self, out: &mut Vec<bl_engine::ChainMeter>) {
        out.clear();
        if let Some(engine) = self.engine.as_mut() {
            let state = engine.state();
            if state.program.is_some() {
                out.extend_from_slice(&state.chain_meters);
            }
        }
    }

    /// Copies the display data the engine's devices last published into
    /// `out`, reusing its capacity (the device framework spec, R35). Empty
    /// while no engine runs.
    pub fn displays_into(
        &self,
        out: &mut std::collections::BTreeMap<bl_model::DeviceId, bl_engine::DisplayData>,
    ) {
        match self.engine.as_ref() {
            Some(engine) => engine.displays().read_all(out),
            None => out.clear(),
        }
    }

    /// Switches the engine on or off: off closes the device, on reopens it
    /// with the current settings.
    ///
    /// # Errors
    ///
    /// The device's error, if it couldn't reopen. The engine is on anyway,
    /// and [`AudioHost::status`] reports the error.
    pub fn set_engine(&mut self, on: bool) -> Result<(), String> {
        self.engine_on = on;
        if on {
            self.start()
        } else {
            self.stop();
            self.error = None;
            Ok(())
        }
    }

    /// Plays or stops the test tone.
    pub fn set_test_tone(&self, on: bool) {
        self.shared.set_tone_on(on);
    }

    /// Whether the test tone plays.
    #[must_use]
    pub fn test_tone(&self) -> bool {
        self.shared.tone.load(Relaxed)
    }

    /// The test tone's level and frequency.
    #[must_use]
    pub fn tone(&self) -> ToneSettings {
        self.tone
    }

    /// Sets the test tone's level and frequency, brought within the
    /// controls' ranges; a playing tone changes at the next block.
    pub fn set_tone(&mut self, tone: ToneSettings) {
        self.tone = tone.clamped();
        self.shared.set_tone(self.tone);
    }

    /// The running stream's statistics: dropouts, buffer sizes, and the
    /// heap operations the allocation trap counted inside audio blocks.
    /// `None` while no stream is open.
    #[must_use]
    pub fn stats(&self) -> Option<StreamStats> {
        self.stream.as_ref().map(|stream| stream.stats())
    }

    /// Applies new settings straight away: closes the stream and opens one
    /// with the new settings. While the engine is off, the settings are
    /// only stored.
    ///
    /// # Errors
    ///
    /// A message if a value is malformed or the device refuses the new
    /// settings. The previous settings are then reopened, so sound carries
    /// on as before.
    pub fn configure(&mut self, change: &ConfigureAudio) -> Result<(), String> {
        let next = self.merged(change)?;
        // Driver Error Compensation changes where new recordings fall, not
        // the stream, which keeps running.
        let compensation_only = change.driver_error_compensation_ms.is_some()
            && *change
                == ConfigureAudio {
                    driver_error_compensation_ms: change.driver_error_compensation_ms,
                    ..ConfigureAudio::default()
                };
        if !self.engine_on || compensation_only {
            self.settings = next;
            return Ok(());
        }
        let previous = std::mem::replace(&mut self.settings, next);
        self.close();
        match self.open() {
            Ok(()) => {
                self.error = None;
                Ok(())
            }
            Err(message) => {
                self.settings = previous;
                self.error = self.open().err();
                Err(message)
            }
        }
    }

    /// The settings `change` leads to, checked.
    fn merged(&self, change: &ConfigureAudio) -> Result<AudioSettings, String> {
        let mut next = self.settings.clone();
        if let Some(driver) = &change.driver {
            let kind = BackendKind::from_id(driver).ok_or_else(|| {
                format!("`{driver}` isn't a driver type; `audio.devices` lists them")
            })?;
            if !self.available.contains(&kind) {
                return Err(format!(
                    "the {kind} driver isn't available on this computer"
                ));
            }
            if Some(kind) != next.driver.as_deref().and_then(BackendKind::from_id) {
                next.output_device = None;
                next.input_device = None;
                next.sample_rate = None;
                next.buffer_size = None;
                next.output_channels = None;
            }
            next.driver = Some(kind.id().to_owned());
        }
        if let Some(device) = &change.output_device {
            next.output_device = if device == "default" {
                None
            } else {
                Some(parse_device(device)?)
            };
        }
        if let Some(device) = &change.input_device {
            next.input_device = if device == "none" {
                None
            } else {
                Some(parse_device(device)?)
            };
        }
        if let Some(rate) = change.sample_rate {
            next.sample_rate = (rate != 0).then_some(rate);
        }
        if let Some(frames) = change.buffer_size {
            next.buffer_size = (frames != 0).then_some(frames);
        }
        if let Some(channels) = change.output_channels {
            next.output_channels = (channels != 0).then_some(channels);
        }
        if let Some(ms) = change.driver_error_compensation_ms {
            if !ms.is_finite() {
                return Err(format!(
                    "{ms} isn't a Driver Error Compensation in milliseconds"
                ));
            }
            next.driver_error_compensation_ms = clamped_compensation_ms(ms);
        }
        Ok(next)
    }

    fn backend(&mut self) -> Result<&dyn Backend, String> {
        let kind = self.driver();
        if self.backend.as_ref().is_none_or(|(open, _)| *open != kind) {
            self.backend = None;
            let backend = (self.factory)(kind).map_err(|e| e.to_string())?;
            self.backend = Some((kind, backend));
        }
        Ok(self
            .backend
            .as_ref()
            .map(|(_, backend)| backend.as_ref())
            .expect("just opened"))
    }

    fn open(&mut self) -> Result<(), String> {
        self.close();
        let request = StreamRequest {
            output: Some(PortRequest {
                device: selector(self.settings.output_device.as_deref())?,
                channels: self.settings.output_channels,
            }),
            input: match self.settings.input_device.as_deref() {
                Some(id) => Some(PortRequest {
                    device: selector(Some(id))?,
                    channels: None,
                }),
                None => None,
            },
            sample_rate: self.settings.sample_rate,
            buffer_size: self.settings.buffer_size,
        };
        if self.drop_thread.is_none() {
            let thread = Collector::new()
                .spawn(DROP_PERIOD)
                .map_err(|e| format!("couldn't start the audio drop thread: {e}"))?;
            self.drop_thread = Some(thread);
        }
        let gc = self
            .drop_thread
            .as_ref()
            .map(DropThread::handle)
            .ok_or("the audio drop thread isn't running")?;
        let (preview, voice) = preview_channel(gc, Arc::clone(&self.preview_shared));
        let (handoff, incoming) = RingBuffer::new(1);
        let mut callback =
            EngineCallback::with_engine(Arc::clone(&self.shared), incoming).with_preview(voice);
        if let Some(clock) = &self.test_clock {
            callback = callback.with_test_clock(clock.clone());
        }
        let callback = Box::new(callback);
        let mut stream = self
            .backend()?
            .open_stream(&request, callback)
            .map_err(|e| e.to_string())?;
        // The engine is made for the rate the device runs at.
        let engine =
            self.start_engine(stream.info(), stream.latency(), stream.workgroup(), handoff)?;
        if let Err(error) = stream.start() {
            // The processor goes with the stream, before the engine.
            drop(stream);
            drop(engine);
            return Err(error.to_string());
        }
        self.stream = Some(stream);
        self.engine = Some(engine);
        self.engines += 1;
        self.preview = Some(preview);
        self.last_sample = (
            self.shared.busy_ns.load(Relaxed),
            self.shared.audio_ns.load(Relaxed),
        );
        self.cpu_load = Some(0.0);
        Ok(())
    }

    /// Starts an engine for a stream that runs as `info` says, gives it the
    /// set and the transport where the last engine left it (stopped), and
    /// hands its processor to the stream's callback through `handoff`.
    fn start_engine(
        &mut self,
        info: &StreamInfo,
        latency: bl_audio_io::Latency,
        workgroup: Option<bl_audio_io::Workgroup>,
        mut handoff: Producer<Processor>,
    ) -> Result<Engine, String> {
        let rate = SampleRate::new(info.sample_rate)
            .ok_or_else(|| "the device runs at 0 Hz".to_owned())?;
        let max_frames = (info.max_block_frames as usize).max(1);
        let (output_latency, round_trip_latency) = self.latency_frames(info.sample_rate, latency);
        self.latency_sent = (output_latency, round_trip_latency);
        let config = EngineConfig {
            input_channels: info.input_channels.into(),
            output_channels: info.output_channels.into(),
            output_latency,
            round_trip_latency,
            ..EngineConfig::new(rate, max_frames)
        };
        let mut registry = Registry::new();
        bl_devices::register(&mut registry);
        if let Some(plugins) = &self.plugins {
            registry.add(plugins.factory());
        }
        if let Some(au) = &self.au {
            registry.add(au.factory());
        }
        if let Some(vst3) = &self.vst3 {
            registry.add(vst3.factory());
        }
        if let Some(lv2) = &self.lv2 {
            registry.add(lv2.factory());
        }
        if let Some(kit) = &self.kit {
            registry.add(kit.factory());
        }
        if let Some(media) = &self.media {
            let streams = MediaStreams::new(Arc::clone(media))
                .map_err(|e| format!("couldn't start the disk thread: {e}"))?;
            let streams = Arc::new(streams);
            registry.set_media(crate::analysed_media::AnalysedMedia::library(
                Arc::clone(&streams) as Arc<dyn MediaLibrary>,
                self.analysis.clone(),
            ));
            self.streams = Some(streams);
        }
        let (engine, mut processor) = Engine::start(config, registry)
            .map_err(|e| format!("couldn't start the engine's threads: {e}"))?;
        if let Some(midi) = &self.midi {
            // The processor has no queue yet, so nothing comes back to drop.
            processor.set_midi_input(midi.attach(MIDI_QUEUE_MESSAGES));
        }
        if let Some(bus) = &self.midi_out {
            processor.set_midi_output(bus.attach(MIDI_OUT_MESSAGES));
        }
        // The multicore scheduler (WP-5.37): workers beside the audio
        // thread, joined to the stream's workgroup, started with the stream
        // and stopped with it. A pool that can't start leaves the audio
        // thread to run the song alone.
        let frames = info.buffer_size.unwrap_or(info.max_block_frames);
        let period = Duration::from_secs_f64(f64::from(frames) / f64::from(info.sample_rate));
        let promote = workgroup.is_some() || info.backend != BackendKind::Null;
        let mut pool = PoolConfig::for_stream(period, workgroup, promote);
        // The null device is a test's: it runs as fast as it can, and
        // workers spinning beside it would only crowd the machine.
        if info.backend == BackendKind::Null {
            pool.workers = 0;
        }
        if let Ok(pool) = Pool::start(&config, &pool)
            && pool.workers() > 0
        {
            processor.set_pool(Some(pool));
        }
        if let Some(document) = &self.document {
            engine.sync(document);
        }
        // Spec: control-bar-transport R16. Where the song was, stopped: a
        // Continue, a locate, and a Stop put the playhead back, then the
        // insert marker goes back; the processor's first block takes them
        // all, before anything plays.
        let memory = self.memory;
        if memory.position > Ticks::ZERO {
            engine.transport(TransportCommand::Continue);
            engine.transport(TransportCommand::Locate(memory.position));
            engine.transport(TransportCommand::Stop);
        }
        engine.transport(TransportCommand::SetInsertMarker(memory.insert_marker));
        // Spec: launch-quantisation R1. The set's global quantisation, so
        // launching waits for the right grid from the first block.
        engine.session(SessionCommand::SetQuantization(self.launch));
        engine.sync_outputs(&self.sync_outputs);
        let _ = engine.flush(ENGINE_START_TIMEOUT);
        handoff
            .push(processor)
            .map_err(|_| "couldn't hand the engine to the audio device".to_owned())?;
        Ok(engine)
    }

    /// The device's output latency and the round trip an external device
    /// reports, in frames at `rate`: its reported input and output latency
    /// plus Driver Error Compensation, never less than zero (the external
    /// devices spec, R4, R13).
    fn latency_frames(&self, rate: u32, latency: bl_audio_io::Latency) -> (u32, u32) {
        let compensation = clamped_compensation_ms(self.settings.driver_error_compensation_ms)
            * f64::from(rate)
            / 1000.0;
        let round_trip =
            i64::from(latency.input) + i64::from(latency.output) + compensation.round() as i64;
        (
            latency.output,
            u32::try_from(round_trip.max(0)).unwrap_or(u32::MAX),
        )
    }

    /// Tells the engine the device's latency when it has changed since it
    /// last heard: a backend that measures it from its first callbacks, or
    /// Driver Error Compensation edited while the stream runs.
    fn refresh_latency(&mut self) {
        let (Some(stream), Some(engine)) = (&self.stream, &self.engine) else {
            return;
        };
        let now = self.latency_frames(stream.info().sample_rate, stream.latency());
        if now != self.latency_sent {
            engine.set_latency(now.0, now.1);
            self.latency_sent = now;
        }
    }

    /// Measures the CPU load since the last call: the share of the audio
    /// produced in that time that the callback spent processing, in percent.
    /// The meter calls it a few times a second. `None` while the engine is
    /// off or the device is closed.
    pub fn sample_cpu_load(&mut self) -> Option<f32> {
        if self.stream.is_none() {
            self.cpu_load = None;
            return None;
        }
        self.refresh_latency();
        let busy = self.shared.busy_ns.load(Relaxed);
        let audio = self.shared.audio_ns.load(Relaxed);
        let (last_busy, last_audio) = self.last_sample;
        if audio > last_audio {
            let load = (busy - last_busy) as f64 / (audio - last_audio) as f64 * 100.0;
            self.cpu_load = Some(load as f32);
            self.last_sample = (busy, audio);
        }
        self.cpu_load
    }

    /// The sample rate the open stream runs at, which previews are
    /// converted to.
    #[must_use]
    pub fn sample_rate(&self) -> Option<u32> {
        self.stream.as_ref().map(|stream| stream.info().sample_rate)
    }

    /// The open device's input and output channels, both zero while no
    /// stream is open: what the In/Out choosers list as the interface.
    #[must_use]
    pub fn channels(&self) -> (u16, u16) {
        self.stream.as_ref().map_or((0, 0), |stream| {
            let info = stream.info();
            (info.input_channels, info.output_channels)
        })
    }

    /// Plays a browser preview through the open stream, replacing the one
    /// playing (WP-1.10). The audio must be at [`AudioHost::sample_rate`].
    /// Returns the preview's generation, which [`AudioHost::preview_status`]
    /// reports while it plays.
    ///
    /// # Errors
    ///
    /// A message if no stream is open or the preview can't start.
    pub fn play_preview(&mut self, samples: Box<dyn PreviewSamples>) -> Result<u64, String> {
        let preview = self
            .preview
            .as_mut()
            .ok_or("the audio device isn't running")?;
        preview.play(samples).map_err(|e| e.to_string())
    }

    /// Plays a warped browser preview through the open stream (WP-4.18; spec: browser R19).
    pub fn play_warped_preview(&mut self, warped: Box<WarpedPreview>) -> Result<u64, String> {
        let preview = self
            .preview
            .as_mut()
            .ok_or("the audio device isn't running")?;
        preview.play_warped(warped).map_err(|e| e.to_string())
    }

    /// Scrubs the browser's playing warped preview to `beat` in ticks (WP-4.18; spec: browser R22).
    pub fn scrub_preview(&mut self, beat: f64) -> Result<(), String> {
        let preview = self
            .preview
            .as_mut()
            .ok_or("the audio device isn't running")?;
        preview.scrub(beat).map_err(|e| e.to_string())
    }

    /// Stops the browser's preview, fading it out.
    pub fn stop_preview(&self) {
        if let Some(preview) = &self.preview {
            preview.stop();
        }
    }

    /// Sets the preview volume, a linear gain from 0 (silence) to +6 dB,
    /// kept for later streams too.
    pub fn set_preview_gain(&self, gain: f32) {
        self.preview_shared.set_gain(gain);
    }

    /// What the preview voice plays, as the audio thread last said.
    #[must_use]
    pub fn preview_status(&self) -> PreviewStatus {
        bl_engine::preview::status(&self.preview_shared)
    }

    /// How many frames a recording is moved earlier: the open device's
    /// reported input and output latency plus the Driver Error Compensation
    /// setting, which moves it further when positive and back when negative
    /// (WP-2.13). `None` while no stream is open.
    #[must_use]
    pub fn recording_delay(&self) -> Option<i64> {
        let stream = self.stream.as_ref()?;
        let compensation = clamped_compensation_ms(self.settings.driver_error_compensation_ms)
            * f64::from(stream.info().sample_rate)
            / 1000.0;
        let latency = stream.latency();
        Some(i64::from(latency.input) + i64::from(latency.output) + compensation.round() as i64)
    }

    /// What the device runs at now.
    #[must_use]
    pub fn status(&self) -> AudioStatus {
        let info = self.stream.as_ref().map(|stream| stream.info());
        let stats = self.stream.as_ref().map(|stream| stream.stats());
        let latency = self.stream.as_ref().map(|stream| stream.latency());
        let sample_rate = info.map(|info| info.sample_rate);
        let buffer_size = stats
            .and_then(|stats| stats.last_device_frames)
            .or_else(|| info.and_then(|info| info.buffer_size));
        let device = |device: &bl_audio_io::OpenDevice| DeviceRef {
            id: device.id.to_string(),
            name: device.name.clone(),
        };
        let ms = |frames: u32| {
            sample_rate.map_or(0.0, |rate| {
                f64::from(frames) * 1000.0 / f64::from(rate.max(1))
            })
        };
        AudioStatus {
            engine_on: self.engine_on,
            settings: self.settings.clone(),
            driver: self.driver().id().to_owned(),
            output_device: info.and_then(|info| info.output_device.as_ref().map(device)),
            input_device: info.and_then(|info| info.input_device.as_ref().map(device)),
            follows_default: info.is_some_and(|info| info.follows_default),
            sample_rate,
            requested_buffer_size: self.settings.buffer_size,
            buffer_size,
            output_channels: info.map_or(0, |info| info.output_channels),
            input_channels: info.map_or(0, |info| info.input_channels),
            buffer_ms: buffer_size
                .zip(sample_rate)
                .map(|(frames, rate)| f64::from(frames) * 1000.0 / f64::from(rate.max(1))),
            input_latency_ms: latency.map(|latency| ms(latency.input)),
            output_latency_ms: latency.map(|latency| ms(latency.output)),
            overall_latency_ms: self
                .recording_delay()
                .zip(sample_rate)
                .map(|(frames, rate)| frames as f64 * 1000.0 / f64::from(rate.max(1))),
            cpu_load: self.cpu_load,
            dropouts: stats.map_or(0, |stats| stats.dropouts),
            test_tone: self.test_tone(),
            tone: self.tone,
            error: self.error.clone(),
        }
    }

    /// The driver types on offer and the devices of the one in use.
    pub fn devices(&mut self) -> AudioDevices {
        let drivers = self
            .available
            .iter()
            .map(|kind| DriverInfo {
                id: kind.id().to_owned(),
                name: kind.label().to_owned(),
            })
            .collect();
        let driver = self.driver().id().to_owned();
        let listed = self
            .backend()
            .and_then(|backend| backend.devices().map_err(|e| e.to_string()));
        let (devices, error) = match listed {
            Ok(devices) => (devices.iter().map(listing).collect(), None),
            Err(error) => (Vec::new(), Some(error)),
        };
        AudioDevices {
            drivers,
            driver,
            devices,
            error,
        }
    }
}

fn parse_device(id: &str) -> Result<String, String> {
    id.parse::<DeviceId>()
        .map(|id| id.to_string())
        .map_err(|e| e.message().to_owned())
}

fn selector(id: Option<&str>) -> Result<DeviceSelector, String> {
    match id {
        None => Ok(DeviceSelector::Default),
        Some(id) => id
            .parse::<DeviceId>()
            .map(DeviceSelector::Id)
            .map_err(|e| e.message().to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_block(engine: &mut EngineCallback, data: &mut [f32], channels: usize, rate: u32) {
        let frames = data.len() / channels;
        let mut output = AudioBufferMut::new(data, channels, frames, frames);
        let ctx = ProcessContext {
            sample_rate: rate,
            frames,
            position: 0,
            timestamp: None,
        };
        engine.process(&ctx, &AudioBuffer::empty(frames), &mut output);
    }

    fn peak(data: &[f32]) -> f32 {
        data.iter().fold(0.0f32, |peak, s| peak.max(s.abs()))
    }

    #[test]
    fn the_test_tone_plays_at_its_volume_and_frequency() {
        // Spec: app-shell R13. Only the samples are checked; nothing plays.
        let shared = Arc::new(EngineShared::default());
        shared.set_tone_on(true);
        let mut engine = EngineCallback::new(Arc::clone(&shared));
        let mut data = vec![0.0f32; 2 * 4800];
        run_block(&mut engine, &mut data, 2, 48_000);
        let default = peak(&data);
        let expected = 10f32.powf(-36.0 / 20.0);
        assert!((default - expected).abs() < 1e-4, "-36 dBFS: {default}");
        assert_eq!(shared.audio_ns(), 100_000_000);

        shared.set_tone(ToneSettings {
            volume_db: -60.0,
            frequency_hz: 1000.0,
        });
        let mut data = vec![0.0f32; 4800];
        run_block(&mut engine, &mut data, 1, 48_000);
        let quiet = peak(&data);
        assert!(quiet > 0.0009 && quiet <= 0.001 + 1e-7, "-60 dBFS: {quiet}");
        // 1 kHz at 48 kHz: 48 samples a cycle, so 100 cycles and about as
        // many upward zero crossings in 4800 samples.
        let crossings = data
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count();
        assert!((99..=100).contains(&crossings), "{crossings}");

        shared.set_tone(ToneSettings {
            volume_db: 20.0,
            frequency_hz: 440.0,
        });
        run_block(&mut engine, &mut data, 1, 48_000);
        assert!(peak(&data) <= 1.0, "never past full scale");
    }

    #[test]
    fn silence_is_the_default() {
        let shared = Arc::new(EngineShared::default());
        let mut engine = EngineCallback::new(shared);
        let mut data = vec![0.0f32; 256];
        run_block(&mut engine, &mut data, 1, 44_100);
        assert!(data.iter().all(|s| *s == 0.0));
    }

    /// A detached engine with one test tone on a MIDI track, as `bl-engine`'s
    /// own tests make one.
    fn toned_engine() -> Engine {
        tone_tracks(&[(bl_model::TrackKind::Midi, bl_model::Output::Default)])
    }

    /// An engine with a test tone on a track of each kind and Audio To given.
    fn tone_tracks(tracks: &[(bl_model::TrackKind, bl_model::Output)]) -> Engine {
        use bl_engine::nodes::{TEST_TONE, test_tone};
        use bl_engine::testing::{self, Probe};
        use bl_model::{Command, Device, DeviceKind, Editor};

        let probe = Arc::new(Probe::default());
        let engine = Engine::start_detached(
            EngineConfig::new(SampleRate::HZ_48000, 256),
            testing::registry(&probe),
        )
        .unwrap();
        let mut editor = Editor::new();
        for (i, &(kind, output)) in tracks.iter().enumerate() {
            let track = editor.new_id();
            editor
                .apply(Command::CreateTrack {
                    id: track,
                    kind,
                    name: "Tone".to_owned(),
                    parent: None,
                    index: i,
                })
                .unwrap();
            let mut device = Device::new(
                editor.new_id(),
                "",
                DeviceKind::Native {
                    device_type: TEST_TONE.to_owned(),
                },
            );
            device.params.insert(test_tone::FREQUENCY, 1_000.0);
            device.params.insert(test_tone::LEVEL, -6.0);
            editor
                .apply(Command::InsertDevice {
                    track,
                    index: 0,
                    device,
                })
                .unwrap();
            if output != bl_model::Output::Default {
                editor.apply(Command::SetOutput { track, output }).unwrap();
            }
        }
        engine.sync(editor.document());
        assert!(engine.flush(Duration::from_secs(5)));
        engine
    }

    // Spec: routing R16 (WP-2.13)
    #[test]
    fn main_out_places_the_main_mix_on_the_device_channels_it_names() {
        let engine = toned_engine();
        let processor = engine
            .restarter()
            .restart(
                EngineConfig::new(SampleRate::HZ_48000, 256),
                None,
                Duration::from_secs(5),
            )
            .unwrap();
        let (mut handoff, incoming) = RingBuffer::new(1);
        handoff.push(processor).unwrap();
        let shared = Arc::new(EngineShared::default());
        shared.set_main_out(2); // Main Out 3/4, the set counting from 1.
        let mut engine = EngineCallback::with_engine(Arc::clone(&shared), incoming);

        let mut data = vec![0.0f32; 4 * 256];
        run_block(&mut engine, &mut data, 4, 48_000);
        assert!(
            data[..512].iter().all(|&s| s == 0.0),
            "channels 1/2 stay silent"
        );
        assert!(
            data[512..].iter().any(|&s| s != 0.0),
            "the mix plays on 3/4"
        );
        assert_eq!(&data[512..768], &data[768..1024], "both of the pair");

        // Main Out 1/2 again: the first two channels.
        shared.set_main_out(0);
        data.fill(0.0);
        run_block(&mut engine, &mut data, 4, 48_000);
        assert!(data[..512].iter().any(|&s| s != 0.0));
        assert!(
            data[512..].iter().all(|&s| s == 0.0),
            "channels 3/4 stay silent"
        );
    }

    /// The callback of a four-output device playing `tracks`' tones.
    fn four_outputs(
        tracks: &[(bl_model::TrackKind, bl_model::Output)],
        main_out: u32,
    ) -> EngineCallback {
        let processor = tone_tracks(tracks)
            .restarter()
            .restart(
                EngineConfig {
                    output_channels: 4,
                    ..EngineConfig::new(SampleRate::HZ_48000, 256)
                },
                None,
                Duration::from_secs(5),
            )
            .unwrap();
        let (mut handoff, incoming) = RingBuffer::new(1);
        handoff.push(processor).unwrap();
        let shared = Arc::new(EngineShared::default());
        shared.set_main_out(main_out);
        EngineCallback::with_engine(shared, incoming)
    }

    // Spec: routing R3 (WP-2.13)
    #[test]
    fn a_track_on_ext_out_plays_on_its_channels_and_not_in_main() {
        use bl_model::{Channel, Output, TrackKind};
        let on = |channel| (TrackKind::Audio, Output::External { channel });

        // Pair 3/4: channels 3 and 4 only, Main Out's 1/2 silent.
        let mut engine = four_outputs(&[on(Channel::Pair(3))], 0);
        let mut data = vec![0.0f32; 4 * 256];
        run_block(&mut engine, &mut data, 4, 48_000);
        assert!(data[..512].iter().all(|&s| s == 0.0), "not in Main");
        assert!(data[512..768].iter().any(|&s| s != 0.0));
        assert_eq!(&data[512..768], &data[768..], "both of the pair");

        // Single channel 3: channel 3 alone, and Main untouched.
        let mut engine = four_outputs(&[on(Channel::Mono(3))], 0);
        data.fill(0.0);
        run_block(&mut engine, &mut data, 4, 48_000);
        assert!(data[..512].iter().all(|&s| s == 0.0));
        assert!(data[512..768].iter().any(|&s| s != 0.0));
        assert!(
            data[768..].iter().all(|&s| s == 0.0),
            "channel 4 stays silent"
        );

        // Main Out 1/2 and a track on Ext. Out 1/2 sum.
        let main = (TrackKind::Audio, Output::Default);
        let mut engine = four_outputs(&[main], 0);
        data.fill(0.0);
        run_block(&mut engine, &mut data, 4, 48_000);
        let alone = data[..256].to_vec();
        let mut engine = four_outputs(&[main, on(Channel::Pair(1))], 0);
        data.fill(0.0);
        run_block(&mut engine, &mut data, 4, 48_000);
        let both: Vec<f32> = alone.iter().map(|s| s * 2.0).collect();
        assert_eq!(&data[..256], &both[..], "Main and Ext. Out add on 1/2");
        assert_eq!(&data[256..512], &both[..]);
        assert!(data[512..].iter().all(|&s| s == 0.0));
    }
}

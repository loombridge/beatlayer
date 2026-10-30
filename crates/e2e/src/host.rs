//! The app's host wiring without a window: the shell on the null device
//! paced by hand, its engine running, and the agent API in this process,
//! pumped the way the app's window pumps them each frame.
//!
//! [`Host::pump`] does what `crates/app/src/host.rs` does each frame, in its
//! order: the shell's background work (`Shell::poll`), the agents' render
//! requests (run here, at once, rather than on a worker thread), the export
//! an agent waits for (`Agent::sync_export`), the calls that waited
//! (`Agent::resume`), and the engine's queue and reports
//! (`Agent::sync_engine`); then one step of audio on the null device.
//!
//! The agent API is the app's ([`Agent::for_app`]), with a window that can
//! capture views unless [`HostOptions::window`] is off: renders and exports
//! wait for the host, as they do in the app, and [`Host`] drives them to
//! their end before a tool call returns. It answers every question the
//! person would be asked with yes, and by default its agent is autonomous,
//! as if the person had switched autonomous mode on for it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bl_agent::client::{Client, Transport};
use bl_agent::{Agent, Outcome, Question};
use bl_audio_io::{NullBackend, NullControl, NullOptions, Pacing};
use bl_model::Document;
use bl_shell::{Shell, testing};
use serde_json::{Value, json};

use crate::audio::Audio;
use crate::capture::{Capture, Recorder};
use crate::{Driver, TempDir};

/// The name the host's agent connects with.
pub const AGENT_NAME: &str = "e2e";

/// How many times a call that waits for the host is pumped before the host
/// gives up on it. Each look polls the shell and yields the thread, so this
/// is far longer than any render or export takes; it stops the suite if
/// one never ends.
const DEFERRED_LOOKS: u64 = 20_000_000;

/// How [`Host::with_options`] sets the host up.
#[derive(Clone, Copy, Debug)]
pub struct HostOptions {
    /// Whether the app's window is open, so `preview`, `analyze` and
    /// `export.audio` wait for the host as they do in the app. Off, they run
    /// at once as `bl-cli mcp` runs them.
    pub window: bool,
    /// Whether the person has switched autonomous mode on for the host's
    /// agent. Off, every command that asks first asks, and the host answers
    /// yes.
    pub autonomous: bool,
    /// The null device's input and output channels.
    pub channels: u16,
    /// The latency the null device's streams report, in frames: input and
    /// output.
    pub latency: (u32, u32),
}

impl Default for HostOptions {
    fn default() -> Self {
        Self {
            window: true,
            autonomous: true,
            channels: 2,
            latency: (0, 0),
        }
    }
}

/// The agent API as the app's window serves it, as a client transport: a
/// call that asks the person is answered, and a call that waits for the
/// host is pumped until it ends.
pub struct AppTransport {
    /// The agent API.
    pub agent: Agent,
    /// The shell its commands run on.
    pub shell: Shell,
    /// Answers questions for the person: `true` allows. Yes to everything
    /// unless a test says otherwise.
    pub answer: Box<dyn FnMut(&Question) -> bool>,
    /// The questions asked so far, as the person read them.
    pub asked: Vec<String>,
    control: NullControl,
    session: String,
    /// Replies to calls that waited, by ticket, until their caller takes
    /// them.
    answered: BTreeMap<u64, Option<Value>>,
    /// Tickets of calls that waited again after they resumed, and the
    /// ticket the caller waits on.
    forwarded: BTreeMap<u64, u64>,
}

impl std::fmt::Debug for AppTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppTransport")
            .field("agent", &self.agent)
            .field("asked", &self.asked)
            .finish_non_exhaustive()
    }
}

impl AppTransport {
    /// Everything the app's window does each frame but play audio: the
    /// shell's background work, render requests, the waiting export, the
    /// calls that waited, and the engine's queue and reports.
    pub fn background(&mut self) {
        self.shell.poll();
        for request in self.agent.take_render_requests() {
            let ticket = request.ticket;
            let result = request.execute();
            self.agent.finish_render(&mut self.shell, ticket, result);
        }
        for request in self.agent.take_capture_requests() {
            self.agent.finish_capture(
                request.ticket,
                Err("the end-to-end host draws no window to capture".to_owned()),
            );
        }
        self.agent.sync_export(&mut self.shell);
        while let Some((ticket, outcome)) = self.agent.resume(&mut self.shell) {
            let ticket = self.forwarded.remove(&ticket).unwrap_or(ticket);
            match outcome {
                Outcome::Reply(reply) => {
                    self.answered.insert(ticket, reply);
                }
                Outcome::Ask(question) => {
                    let reply = self.ask(&question);
                    self.answered.insert(ticket, reply);
                }
                Outcome::Wait(again) => {
                    self.forwarded.insert(again, ticket);
                }
            }
        }
        self.agent.sync_engine(&mut self.shell);
    }

    /// One step of the app's frame and [`testing::STEP_MS`] of audio.
    pub fn pump(&mut self) {
        self.background();
        // With the audio engine switched off there is no stream to run.
        if self.shell.audio().is_running() {
            testing::advance(&self.control, &mut self.shell, testing::STEP_MS);
        }
    }

    fn ask(&mut self, question: &Question) -> Option<Value> {
        self.asked.push(question.text());
        let allow = (self.answer)(question);
        self.agent.resolve(&mut self.shell, question.id, allow)
    }

    fn wait_for(&mut self, ticket: u64) -> Result<Option<Value>, String> {
        for _ in 0..DEFERRED_LOOKS {
            self.background();
            if let Some(reply) = self.answered.remove(&ticket) {
                return Ok(reply);
            }
            std::thread::yield_now();
        }
        Err(format!("the call waiting as ticket {ticket} never ended"))
    }
}

impl Transport for AppTransport {
    fn send(&mut self, message: &Value) -> Result<Option<Value>, String> {
        let outcome = self
            .agent
            .handle(&mut self.shell, &self.session, message.clone());
        match outcome {
            Outcome::Reply(reply) => Ok(reply),
            Outcome::Ask(question) => Ok(self.ask(&question)),
            Outcome::Wait(ticket) => self.wait_for(ticket),
        }
    }
}

/// How an export is written, for [`Host::export`].
#[derive(Clone, Debug)]
pub struct ExportSpec {
    /// The file's extension, which picks its type: `wav`, `aif`, `flac` or
    /// `mp3`.
    pub extension: &'static str,
    /// 16, 24 or 32 bits.
    pub bit_depth: u8,
    /// The file's rate; the engine's when `None`.
    pub sample_rate: Option<u32>,
    /// Whether to convert to mono.
    pub mono: bool,
    /// More `export.audio` arguments, merged over the ones above (an
    /// object, or null for none).
    pub extra: Value,
}

impl Default for ExportSpec {
    /// A 32-bit float WAV file at the engine's rate, undithered, of the
    /// Main output.
    fn default() -> Self {
        Self {
            extension: "wav",
            bit_depth: 32,
            sample_rate: None,
            mono: false,
            extra: Value::Null,
        }
    }
}

/// The app without its window: the shell playing through the hand-paced
/// null device, and the agent API, both pumped as the app's window pumps
/// them. Tool calls go through the agent API as an agent's do; see the
/// module's documentation.
pub struct Host {
    client: Client<AppTransport>,
    recorder: Recorder,
    dir: TempDir,
    saved: Option<PathBuf>,
    exports: u64,
}

impl std::fmt::Debug for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Host")
            .field("dir", &self.dir)
            .field("saved", &self.saved)
            .finish_non_exhaustive()
    }
}

impl Default for Host {
    fn default() -> Self {
        Self::new()
    }
}

impl Host {
    /// The app with its window, its engine running on a two-channel null
    /// device, and an autonomous agent connected as [`AGENT_NAME`].
    ///
    /// # Panics
    ///
    /// If the engine doesn't start or the agent can't connect.
    #[must_use]
    pub fn new() -> Self {
        Self::with_options(HostOptions::default())
    }

    /// The app set up as `options` say.
    ///
    /// # Panics
    ///
    /// If the engine doesn't start or the agent can't connect.
    #[must_use]
    pub fn with_options(options: HostOptions) -> Self {
        let recorder = Recorder::new();
        let (backend, control) = NullBackend::with_devices(
            NullOptions {
                pacing: Pacing::Manual,
                channels: options.channels,
                ..NullOptions::default()
            },
            &[("null", "Null device")],
        );
        let backend = backend
            .with_latency(options.latency.0, options.latency.1)
            .with_cable(recorder.cable());
        let (mut shell, control) = testing::rig_on(backend, control);
        shell.start_audio();
        assert!(
            shell.audio().engine_running(),
            "the engine didn't start: {:?}",
            shell.message()
        );
        recorder.set_rate(shell.engine_rate());
        let mut agent = Agent::for_app();
        agent.set_view_capture(options.window);
        let session = agent.open_session(None);
        let transport = AppTransport {
            agent,
            shell,
            answer: Box::new(|_| true),
            asked: Vec::new(),
            control,
            session,
            answered: BTreeMap::new(),
            forwarded: BTreeMap::new(),
        };
        let mut client = Client::new(transport);
        client
            .initialize(AGENT_NAME)
            .unwrap_or_else(|error| panic!("the agent couldn't connect: {error}"));
        if options.autonomous {
            client
                .transport()
                .shell
                .run(
                    "agent.set_autonomous",
                    json!({ "client": AGENT_NAME, "on": true }),
                )
                .unwrap_or_else(|error| panic!("autonomous mode: {error}"));
        }
        let mut host = Self {
            client,
            recorder,
            dir: TempDir::new("host"),
            saved: None,
            exports: 0,
        };
        host.pump();
        host
    }

    /// The transport the client talks to: the agent API, the shell, and the
    /// person's answers.
    pub fn app(&mut self) -> &mut AppTransport {
        self.client.transport()
    }

    /// The MCP client the host's agent calls tools through.
    pub fn client(&mut self) -> &mut Client<AppTransport> {
        &mut self.client
    }

    /// The shell.
    pub fn shell(&mut self) -> &Shell {
        &self.client.transport().shell
    }

    /// The shell, to change it as the person would.
    pub fn shell_mut(&mut self) -> &mut Shell {
        &mut self.client.transport().shell
    }

    /// The open document.
    pub fn document(&mut self) -> &Document {
        self.shell().editor().document()
    }

    /// Whether the set has unsaved changes.
    pub fn is_dirty(&mut self) -> bool {
        self.shell().is_dirty()
    }

    /// Panics, saying which, unless the document follows every rule a
    /// loaded document must (`Document::check_invariants`).
    ///
    /// # Panics
    ///
    /// As it says.
    pub fn check(&mut self) {
        if let Err(error) = self.document().check_invariants() {
            panic!("the document breaks its invariants: {error}");
        }
    }

    /// The control that runs the null device's blocks.
    pub fn control(&mut self) -> &NullControl {
        &self.client.transport().control
    }

    /// The recorder patched to the null device.
    #[must_use]
    pub fn recorder(&self) -> &Recorder {
        &self.recorder
    }

    /// A temporary folder the host removes when it's dropped, for files a
    /// test writes.
    #[must_use]
    pub fn dir(&self) -> &Path {
        self.dir.path()
    }

    /// One frame of the app and [`testing::STEP_MS`] of audio.
    pub fn pump(&mut self) {
        self.app().pump();
    }

    /// `millis` of test time, in frames of [`testing::STEP_MS`] each.
    pub fn advance_ms(&mut self, millis: u64) {
        for _ in 0..millis.div_ceil(testing::STEP_MS) {
            self.pump();
        }
    }

    /// Waits until the engine plays the set as it is now
    /// ([`testing::settle`]), then runs a frame.
    pub fn settle(&mut self) {
        let app = self.client.transport();
        testing::settle(&app.control, &mut app.shell);
        self.pump();
    }

    /// Pumps until `done` holds, for up to two thousand frames (ten seconds
    /// of test time).
    ///
    /// # Panics
    ///
    /// Naming `what`, if it never does.
    pub fn wait_until(&mut self, what: &str, mut done: impl FnMut(&mut Self) -> bool) {
        for _ in 0..2_000 {
            if done(self) {
                return;
            }
            self.pump();
        }
        panic!("waited ten seconds of test time for {what}");
    }

    /// Whether the engine's transport is playing.
    pub fn is_playing(&mut self) -> bool {
        self.shell_mut().is_playing()
    }

    /// Moves the transport to `ticks` through the `transport` tool.
    ///
    /// # Panics
    ///
    /// If the tool refuses.
    pub fn locate(&mut self, ticks: i64) {
        self.tool(
            "transport",
            json!({ "action": "locate", "args": { "position": ticks } }),
        )
        .unwrap_or_else(|error| panic!("locate: {error}"));
    }

    /// Starts the transport through the `transport` tool.
    ///
    /// # Panics
    ///
    /// If the tool refuses, or the engine never plays.
    pub fn play(&mut self) {
        self.tool("transport", json!({ "action": "play" }))
            .unwrap_or_else(|error| panic!("play: {error}"));
        self.wait_until("the transport to play", Self::is_playing);
    }

    /// Stops the transport through the `transport` tool, and waits until it
    /// has.
    ///
    /// # Panics
    ///
    /// If the tool refuses, or the engine never stops.
    pub fn stop(&mut self) {
        self.tool("transport", json!({ "action": "stop" }))
            .unwrap_or_else(|error| panic!("stop: {error}"));
        self.wait_until("the transport to stop", |host| !host.is_playing());
    }

    /// Keeps what the device plays over the next `millis` of test time.
    pub fn capture_for(&mut self, millis: u64) -> Capture {
        self.recorder.start();
        self.advance_ms(millis);
        self.recorder.stop()
    }

    /// Plays the song from `ticks` for `millis` of test time and gives what
    /// the device played, then stops. The capture starts with the call that
    /// starts the transport, so it begins a block or so before the song
    /// does: line it up with [`crate::audio::best_alignment`].
    ///
    /// # Panics
    ///
    /// If the transport refuses.
    pub fn play_from(&mut self, ticks: i64, millis: u64) -> Capture {
        self.locate(ticks);
        self.settle();
        self.recorder.start();
        self.tool("transport", json!({ "action": "play" }))
            .unwrap_or_else(|error| panic!("play: {error}"));
        self.advance_ms(millis);
        let capture = self.recorder.stop();
        self.stop();
        capture
    }

    /// [`play_from`](Self::play_from) the song's start.
    pub fn play_for(&mut self, millis: u64) -> Capture {
        self.play_from(0, millis)
    }

    /// Exports `length` ticks of the song from `start` through the agent
    /// API's `export.audio`, as `spec` says, into the host's folder, and
    /// decodes the file.
    ///
    /// # Panics
    ///
    /// If the export fails or its file doesn't decode.
    pub fn export(&mut self, start: i64, length: i64, spec: &ExportSpec) -> Audio {
        let (_, path) = self
            .export_file(start, length, spec)
            .unwrap_or_else(|error| panic!("export.audio failed: {error}"));
        Audio::decode(&path).unwrap_or_else(|error| panic!("{error}"))
    }

    /// Exports as [`export`](Self::export) does, and gives `export.audio`'s
    /// result and the file's path without decoding it.
    ///
    /// # Errors
    ///
    /// The tool's error text.
    pub fn export_file(
        &mut self,
        start: i64,
        length: i64,
        spec: &ExportSpec,
    ) -> Result<(Value, PathBuf), String> {
        self.exports += 1;
        let path = self
            .dir()
            .join(format!("export-{}.{}", self.exports, spec.extension));
        let mut args = json!({
            "path": path.to_string_lossy(),
            "start": start,
            "length": length,
            "bit_depth": spec.bit_depth,
            "dither": "none",
            "replace": true,
            "convert_to_mono": spec.mono,
        });
        if let Some(rate) = spec.sample_rate {
            args["sample_rate"] = json!(rate);
        }
        if let Value::Object(extra) = &spec.extra {
            for (key, value) in extra {
                args[key] = value.clone();
            }
        }
        let result = self.run("export.audio", args)?;
        Ok((result, path))
    }

    /// Saves the set with `set.save_as` as `E2E.blset` in `dir` (inside a
    /// new project folder there), and gives the file's path, which
    /// [`reopen`](Self::reopen) opens.
    ///
    /// # Panics
    ///
    /// If the save fails.
    pub fn save_to(&mut self, dir: &Path) -> PathBuf {
        let saved = self.run_ok(
            "set.save_as",
            json!({ "path": dir.join("E2E.blset").to_string_lossy() }),
        );
        let path = PathBuf::from(
            saved["path"]
                .as_str()
                .unwrap_or_else(|| panic!("set.save_as gave no path: {saved}")),
        );
        self.saved = Some(path.clone());
        path
    }

    /// Opens the set last saved with [`save_to`](Self::save_to) again, with
    /// `set.open`, replacing the open set (which must have no unsaved
    /// changes).
    ///
    /// # Errors
    ///
    /// The tool's error text, or that nothing was saved.
    pub fn reopen(&mut self) -> Result<Value, String> {
        let path = self
            .saved
            .clone()
            .ok_or("nothing was saved with `save_to`")?;
        let opened = self.run("set.open", json!({ "path": path.to_string_lossy() }));
        self.pump();
        opened
    }
}

impl Driver for Host {
    /// Calls the tool as the agent, then runs one frame of the app, as the
    /// app's loop does after every event.
    fn tool(&mut self, name: &str, args: Value) -> Result<Value, String> {
        let result = self.client.call(name, args)?.ok();
        self.pump();
        result
    }
}

//! The agent API's state on the main thread: the plug-in kit's MCP server
//! (`lspk_agent::Server`) serving Beatlayer's tools, with the project's
//! checkpoints and scheduled commands, view captures waiting for the
//! window, and the engine the host attaches.

use std::sync::Arc;

use bl_core::time::Ticks;
use bl_engine::{EngineState, MediaLibrary, Report};
use bl_model::Document;
use bl_shell::Shell;
use bl_shell::agent::AGENT_APPROVE_CLIENT;
use bl_shell::perception::PreviewRenderOutcome;
use bl_shell::perception_commands::{
    self as perception, AnalyzeArgs, AnalyzeResult, CaptureResult, PreviewArgs, View,
};
use lspk_agent::{
    Call, Credential, Media, Outcome, Program, ResultFormat, Server, ServerInfo, Session,
};
use serde_json::{Value, json};

use crate::guide;
use crate::permission::Mode;
use crate::tools::{self, Ctx, EngineMessage, Playhead, State, ToolOutcome};

/// A view capture the host is asked for (WP-1.16): the app's window draws
/// the view, the host reads the frame back, and hands the image to
/// [`Agent::finish_capture`] with the ticket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureRequest {
    /// What [`Agent::finish_capture`] answers it with.
    pub ticket: u64,
    /// The view.
    pub view: View,
    /// The image's longest side, in pixels: a larger capture is scaled
    /// down to fit.
    pub max_size: u32,
}

/// A view captured as a PNG image.
#[derive(Clone, Debug, PartialEq)]
pub struct Captured {
    /// The PNG file.
    pub png: Vec<u8>,
    /// Its width in pixels.
    pub width: u32,
    /// Its height in pixels.
    pub height: u32,
    /// Image pixels to each of the window's points.
    pub pixels_per_point: f64,
}

/// What `capture_view` asks of the app's window: the call waits until the
/// host answers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Capture {
    /// The view.
    pub(crate) view: View,
    /// The image's longest side, in pixels.
    pub(crate) max_size: u32,
}

/// The kind of audio render an agent asked for (WP-2.28).
#[derive(Clone, Debug, PartialEq)]
pub enum RenderKind {
    /// An audio preview of a range or track.
    Preview(PreviewArgs),
    /// An audio analysis of a range.
    Analyze(AnalyzeArgs),
}

/// What rendering a preview produced (WP-2.28).
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewOutcome {
    /// The document at the time of the request.
    pub document: Document,
    /// The preview arguments.
    pub args: PreviewArgs,
    /// The rendered preview outcome.
    pub outcome: PreviewRenderOutcome,
    /// The pre-encoded WAV bytes.
    pub wav: Vec<u8>,
}

/// What rendering an audio request produced (WP-2.28).
#[derive(Clone, Debug, PartialEq)]
pub enum RenderOutcome {
    /// A preview outcome.
    Preview(PreviewOutcome),
    /// An analysis result.
    Analyze(AnalyzeResult),
}

/// What a perception tool asks of the host when deferring an audio render (WP-2.28).
#[derive(Clone)]
pub struct Render {
    pub kind: RenderKind,
    pub document: Document,
    pub media: Arc<dyn MediaLibrary>,
    pub selection: Option<(Ticks, Ticks)>,
    pub engine_rate: u32,
    pub playing: bool,
}

impl std::fmt::Debug for Render {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Render")
            .field("kind", &self.kind)
            .field("selection", &self.selection)
            .field("engine_rate", &self.engine_rate)
            .field("playing", &self.playing)
            .finish()
    }
}

/// An audio render the host is asked for (WP-2.28): runs on a worker
/// thread and hands the outcome to [`Agent::finish_render`] with the ticket.
#[derive(Clone)]
pub struct RenderRequest {
    /// What [`Agent::finish_render`] answers it with.
    pub ticket: u64,
    /// The deferred render payload.
    pub render: Render,
}

impl std::fmt::Debug for RenderRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderRequest")
            .field("ticket", &self.ticket)
            .field("render", &self.render)
            .finish()
    }
}

impl RenderRequest {
    /// Renders the request (WP-2.28) on a worker thread with optional cancellation.
    ///
    /// # Errors
    ///
    /// A description if the render or analysis failed.
    pub fn execute_with_cancel(
        self,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<RenderOutcome, String> {
        let source = bl_shell::perception::Source {
            document: &self.render.document,
            media: self.render.media,
            selection: self.render.selection,
            engine_rate: self.render.engine_rate,
            plugins: bl_shell::export::Hosts::default(),
            playing: self.render.playing,
            cancel: Some(cancel),
        };
        match self.render.kind {
            RenderKind::Preview(args) => {
                let outcome = bl_shell::perception::render_preview(&source, &args)
                    .map_err(|error| error.to_string())?;
                let wav = bl_shell::perception::encode_wav(
                    &outcome.samples,
                    outcome.rendered.channels,
                    outcome.rendered.sample_rate,
                    args.bit_depth,
                )?;
                Ok(RenderOutcome::Preview(PreviewOutcome {
                    document: self.render.document,
                    args,
                    outcome,
                    wav,
                }))
            }
            RenderKind::Analyze(args) => {
                let previews = bl_shell::perception::Previews::default();
                let result = bl_shell::perception::analyze(&previews, &source, &args)
                    .map_err(|error| error.to_string())?;
                Ok(RenderOutcome::Analyze(result))
            }
        }
    }

    /// Renders the request (WP-2.28) on a worker thread without cancellation.
    ///
    /// # Errors
    ///
    /// A description if the render or analysis failed.
    pub fn execute(self) -> Result<RenderOutcome, String> {
        static NEVER_CANCEL: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        self.execute_with_cancel(&NEVER_CANCEL)
    }
}

/// What a tool asks of the host: a view capture (WP-1.16) or an audio render (WP-2.28).
#[derive(Debug)]
pub(crate) enum Deferred {
    Capture(Capture),
    Render(std::cell::RefCell<Option<Box<Render>>>),
    /// An `export.audio` the shell started on its thread (the export spec,
    /// R44): the call waits until the export ends.
    Export(ExportCall),
}

/// What a waiting `export.audio` call replies with besides the export's
/// result.
#[derive(Debug)]
pub(crate) struct ExportCall {
    pub(crate) asked_first: bool,
    pub(crate) ids: Value,
}

impl Deferred {
    pub(crate) fn render(render: Render) -> Self {
        Self::Render(std::cell::RefCell::new(Some(Box::new(render))))
    }
}

/// Beatlayer as the kit's server sees it: the tools, the guide and
/// previews as resources, and the person's approvals in the agent
/// settings. Commands run on the shell the server is lent with each
/// message.
#[derive(Debug, Default)]
pub(crate) struct Beatlayer {
    /// What the tools share across sessions.
    state: State,
    /// The engine the host last attached ([`Agent::sync_engine`]), by its
    /// generation, so a new one gets the commands still scheduled.
    engine_generation: Option<u64>,
    /// Whether a window runs that can capture views (WP-1.16).
    view_capture: bool,
    /// Captures the host hasn't taken yet.
    capture_requests: Vec<CaptureRequest>,
    /// Audio renders the host hasn't started yet (WP-2.28).
    render_requests: Vec<RenderRequest>,
    /// The call waiting for the export the shell runs (the export spec,
    /// R44): the shell runs one at a time.
    export_ticket: Option<u64>,
}

impl Program for Beatlayer {
    type Host = Shell;
    /// A report from the engine: a scheduled command fired, whose edit runs
    /// on the document.
    type Event = Report;
    type Deferred = Deferred;

    fn info(&self) -> ServerInfo {
        ServerInfo {
            name: "beatlayer".to_owned(),
            title: "Beatlayer".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            instructions: guide::INSTRUCTIONS.to_owned(),
        }
    }

    fn tools(&self) -> Vec<Value> {
        tools::list()
    }

    fn call(
        &mut self,
        call: Call<'_, Shell>,
        name: &str,
        args: Value,
    ) -> Result<ToolOutcome, String> {
        let mut ctx = Ctx {
            mode: call.mode,
            shell: call.host,
            session: call.session,
            state: &mut self.state,
            confirmed: call.confirmed,
            view_capture: self.view_capture,
        };
        Ok(match tools::call(&mut ctx, name, args)? {
            // Spec: agent-perception R9. Only a window can capture a view.
            ToolOutcome::Defer(Deferred::Capture(_)) if !self.view_capture => ToolOutcome::Done(Err(
                "View captures need Beatlayer's window, and none is open here: `bl-cli mcp` runs \
                 headless. Run the Beatlayer app and connect to its agent server (Settings, Tempo \
                 & MIDI, Agents) to capture views; `preview` and `analyze` work here."
                    .to_owned(),
            )),
            outcome => outcome,
        })
    }

    fn resources(&self, shell: &Shell) -> Vec<Value> {
        let mut resources = vec![guide::listing()];
        resources.extend(shell.previews().ids().map(preview_listing));
        resources
    }

    fn resource_templates(&self) -> Vec<Value> {
        vec![json!({
            "uriTemplate": format!("{}{{id}}.wav", perception::PREVIEW_URI),
            "name": "preview",
            "title": "Audio preview",
            "description": "An audio preview the `preview` tool made, by its ID, as a WAV file. \
                            The last 4 are kept.",
            "mimeType": "audio/wav",
        })]
    }

    fn read_resource(&mut self, shell: &mut Shell, uri: &str) -> Result<Vec<Value>, String> {
        if let Some(preview) = perception::preview_of_uri(uri) {
            let bytes = shell
                .previews()
                .get(preview)
                .ok_or_else(|| format!("preview {preview} isn't kept: the last 4 are"))
                .and_then(bl_shell::perception::Preview::wav)?;
            return Ok(vec![json!({
                "uri": uri,
                "mimeType": "audio/wav",
                "blob": crate::base64::encode(&bytes),
            })]);
        }
        guide::read(uri)
            .map(|contents| vec![contents])
            .ok_or_else(|| format!("no resource is at `{uri}`; the guide is at {}", guide::URI))
    }

    fn gesture_under_way(&self, shell: &Shell) -> bool {
        shell.gesture_under_way()
    }

    fn on_event(&mut self, shell: &mut Shell, report: Report) {
        self.state.schedule.on_report(shell, report);
    }

    fn defer(&mut self, ticket: u64, deferred: &Deferred) {
        match deferred {
            Deferred::Capture(capture) => {
                self.capture_requests.push(CaptureRequest {
                    ticket,
                    view: capture.view,
                    max_size: capture.max_size,
                });
            }
            Deferred::Render(cell) => {
                if let Some(render) = cell.borrow_mut().take() {
                    self.render_requests.push(RenderRequest {
                        ticket,
                        render: *render,
                    });
                }
            }
            Deferred::Export(_) => self.export_ticket = Some(ticket),
        }
    }

    fn allows(&self, shell: &Shell, agent: &str) -> bool {
        shell.agent_settings().client(agent).is_some()
    }

    fn approve(&mut self, shell: &mut Shell, client: &str) -> bool {
        shell
            .run(AGENT_APPROVE_CLIENT, json!({ "client": client }))
            .is_ok()
    }

    fn connect_question(&self, agent: &str, client: &str) -> String {
        format!(
            "\u{201c}{agent}\u{201d} ({client}) wants to control Beatlayer through the agent \
             server. Allow it? It will be able to read and edit this set, and to ask before \
             deleting or reaching outside the project."
        )
    }
}

/// The agent API on the main thread: every session, the project's
/// checkpoints and scheduled commands, and calls waiting for the person.
///
/// A transport hands each message to [`Agent::handle`] with the session it
/// came on, and sends back the reply. Commands run on `shell`, the same
/// shell the UI drives, so an agent's command takes the same path as a
/// person's.
///
/// The protocol, sessions, questions for the person, and calls that wait
/// are the plug-in kit's (`lspk_agent::Server`); the tools are Beatlayer's.
#[derive(Debug)]
pub struct Agent {
    server: Server<Beatlayer>,
}

impl Agent {
    /// The agent API for `bl-cli mcp`: no person to ask, so commands that
    /// ask first are refused unless `autonomous`.
    #[must_use]
    pub fn headless(autonomous: bool) -> Self {
        Self::new(Mode::Headless { autonomous })
    }

    /// The agent API for the running app: the person allows each agent once,
    /// and confirms commands that ask first unless that agent is
    /// autonomous.
    #[must_use]
    pub fn for_app() -> Self {
        Self::new(Mode::App)
    }

    fn new(mode: Mode) -> Self {
        Self {
            server: Server::new(mode, Beatlayer::default()),
        }
    }

    /// Writes tool results' text blocks in `format`: compact JSON, the
    /// default, or TOON where a result holds a long table.
    pub fn set_result_format(&mut self, format: ResultFormat) {
        self.server.set_result_format(format);
    }

    /// The kit's server, for the transports.
    pub(crate) fn server(&mut self) -> &mut Server<Beatlayer> {
        &mut self.server
    }

    fn program(&mut self) -> &mut Beatlayer {
        self.server.program_mut()
    }

    /// Says whether a window runs that can capture views (WP-1.16): the
    /// app's host says so once its main window is open. Without one, as in
    /// `bl-cli mcp`, `capture_view` fails, saying why; calls waiting for a
    /// capture when the window goes fail too.
    pub fn set_view_capture(&mut self, available: bool) {
        self.program().view_capture = available;
        if !available {
            for ticket in self.server.deferred() {
                self.server
                    .finish_deferred(ticket, |deferred| match deferred {
                        Deferred::Capture(_) => {
                            Err("the window closed before the view was captured".to_owned())
                        }
                        Deferred::Render(_) => {
                            Err("the window closed before the render finished".to_owned())
                        }
                        Deferred::Export(_) => {
                            Err("the window closed before the export ended; it goes on and \
                             `export.status` reads it"
                                .to_owned())
                        }
                    });
            }
            self.program().export_ticket = None;
            self.program().capture_requests.clear();
            self.program().render_requests.clear();
        }
    }

    /// Whether a window runs that can capture views.
    #[must_use]
    pub fn view_capture(&self) -> bool {
        self.server.program().view_capture
    }

    /// The view captures agents asked for since the last call, oldest
    /// first. Their calls wait ([`Outcome::Wait`]) until the host answers
    /// each with [`Agent::finish_capture`].
    pub fn take_capture_requests(&mut self) -> Vec<CaptureRequest> {
        std::mem::take(&mut self.program().capture_requests)
    }

    /// The render requests agents asked for since the last call, oldest
    /// first (WP-2.28). Their calls wait ([`Outcome::Wait`]) until the host answers
    /// each with [`Agent::finish_render`].
    pub fn take_render_requests(&mut self) -> Vec<RenderRequest> {
        std::mem::take(&mut self.program().render_requests)
    }

    /// Answers a capture request with the image, or why there's none. The
    /// waiting call's reply comes from [`Agent::resume`], with the ticket.
    pub fn finish_capture(&mut self, ticket: u64, result: Result<Captured, String>) {
        self.server.finish_deferred(ticket, |deferred| {
            let Capture { view, .. } = match deferred {
                Deferred::Capture(capture) => capture,
                Deferred::Render(_) | Deferred::Export(_) => {
                    return Err("expected a view capture".to_owned());
                }
            };
            let image = result?;
            let summary = CaptureResult {
                view,
                width: image.width,
                height: image.height,
                mime_type: "image/png".to_owned(),
                bytes: image.png.len() as u64,
                pixels_per_point: (image.pixels_per_point * 1_000.0).round() / 1_000.0,
            };
            Ok((
                serde_json::to_value(&summary).unwrap_or_default(),
                vec![Media {
                    mime_type: "image/png",
                    bytes: image.png,
                    uri: None,
                }],
            ))
        });
    }

    /// Answers a render request with the outcome, or why it failed (WP-2.28).
    /// The waiting call's reply comes from [`Agent::resume`], with the ticket.
    pub fn finish_render(
        &mut self,
        shell: &mut Shell,
        ticket: u64,
        result: Result<RenderOutcome, String>,
    ) {
        self.server
            .finish_deferred(ticket, |deferred| match deferred {
                Deferred::Render(_) => match result {
                    Ok(RenderOutcome::Preview(preview_outcome)) => {
                        let bytes = preview_outcome.wav.len() as u64;
                        let preview = shell.previews_mut().add(
                            preview_outcome.outcome.samples,
                            preview_outcome.args.bit_depth,
                            preview_outcome.outcome.rendered,
                            preview_outcome.document,
                        );
                        let result = preview.result_with_bytes(
                            bytes,
                            preview_outcome.outcome.peak_dbfs,
                            preview_outcome.outcome.took_seconds,
                        );
                        let media = vec![Media {
                            mime_type: "audio/wav",
                            bytes: preview_outcome.wav,
                            uri: Some(bl_shell::perception_commands::preview_uri(preview.id)),
                        }];
                        Ok((serde_json::to_value(&result).unwrap_or_default(), media))
                    }
                    Ok(RenderOutcome::Analyze(analyze_result)) => Ok((
                        serde_json::to_value(&analyze_result).unwrap_or_default(),
                        Vec::new(),
                    )),
                    Err(err) => Err(err),
                },
                Deferred::Capture(_) | Deferred::Export(_) => Err("expected a render".to_owned()),
            });
    }

    /// Answers the call waiting for an export once it has ended (the export
    /// spec, R44): the host calls it as it polls the shell. The waiting
    /// call's reply comes from [`Agent::resume`], with the ticket.
    pub fn sync_export(&mut self, shell: &mut Shell) {
        let Some(ticket) = self.program().export_ticket else {
            // Nobody waits any more (the session closed): the result is
            // dropped, so the next export can start.
            if shell.export_awaited() {
                let _ = shell.take_export_result();
            }
            return;
        };
        let Some(result) = shell.take_export_result() else {
            return;
        };
        self.program().export_ticket = None;
        self.server
            .finish_deferred(ticket, |deferred| match deferred {
                Deferred::Export(call) => result
                    .map(|result| {
                        (
                            serde_json::json!({
                                "ran": 1,
                                "undo_step": null,
                                "asked_first": call.asked_first,
                                "results": [{
                                    "command": bl_shell::export_commands::AUDIO,
                                    "result": result,
                                }],
                                "ids": call.ids,
                            }),
                            Vec::new(),
                        )
                    })
                    .map_err(|error| {
                        format!(
                            "command 1 (`{}`): {error}. Nothing changed.",
                            bl_shell::export_commands::AUDIO
                        )
                    }),
                _ => Err("expected an export".to_owned()),
            });
    }

    /// Where the agent API runs.
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.server.mode()
    }

    /// Starts a session with a new random ID, or the given one, and returns
    /// its ID. Its requests carry no token ([`Credential::Local`]).
    pub fn open_session(&mut self, id: Option<&str>) -> String {
        self.server.open_session(id)
    }

    /// Starts a session whose requests carry `credential`, with a new
    /// random ID or the given one, and returns its ID.
    pub fn open_session_as(&mut self, id: Option<&str>, credential: Credential) -> String {
        self.server.open_session_as(id, credential)
    }

    /// The credential the session's requests carry, if it's open.
    #[must_use]
    pub fn session_credential(&self, id: &str) -> Option<&Credential> {
        self.server.session_credential(id)
    }

    /// Whether a session with this ID is open.
    #[must_use]
    pub fn has_session(&self, id: &str) -> bool {
        self.server.has_session(id)
    }

    /// Ends a session. Calls it left waiting for the person still finish.
    pub fn close_session(&mut self, id: &str) {
        self.server.close_session(id);
    }

    /// The open sessions, by ID, with who's on each.
    pub fn sessions(&self) -> impl Iterator<Item = (&str, &Session)> {
        self.server.sessions()
    }

    /// Ends every session with the rights of the agent called `client`, as
    /// forgetting its approval does.
    pub fn close_sessions_of(&mut self, client: &str) {
        self.server.close_sessions_of(client);
    }

    /// Says whether an engine plays the song here. While one does, the
    /// transport's playback commands go to it and scheduled commands fire;
    /// without one, the playback commands are refused and scheduled
    /// commands wait.
    pub fn set_engine_attached(&mut self, attached: bool) {
        let state = &mut self.program().state;
        state.engine_attached = attached;
        if !attached {
            state.playhead = None;
        }
    }

    /// What the engine must hear since the last call: transport commands,
    /// and commands scheduled and cancelled, in the order agents asked. The
    /// host sends them on to the engine's queue, the queue the UI's
    /// commands take, after syncing the document, so the engine has the song
    /// as the agent left it.
    pub fn take_engine_messages(&mut self) -> Vec<EngineMessage> {
        std::mem::take(&mut self.program().state.outbox)
    }

    /// Hands the agent API the engine's published state, so `transport`'s
    /// `status` can say whether the song plays and where the playhead and
    /// the insert marker are.
    pub fn on_engine_state(&mut self, state: &EngineState) {
        self.program().state.playhead = Some(Playhead::of(state));
    }

    /// Hands the agent API a report from the engine: a scheduled command
    /// that fired runs on the document now, as its agent's undo step, or,
    /// while the person's gesture is under way, once it ends.
    pub fn on_report(&mut self, shell: &mut Shell, report: Report) {
        self.server.on_event(shell, report);
    }

    /// Brings the agent API and the shell's engine up to date, as the
    /// app's host does after every event (WP-1.15b): says whether an engine
    /// plays the set on a device, sends it what agents asked of it (the
    /// transport's commands and scheduled commands) on the queue the
    /// person's transport commands take, hands the engine's reports back
    /// (fired scheduled commands run as their agents' undo steps), and takes
    /// the engine's transport for `transport`'s `status`. A new engine, after
    /// the device changed, gets every command still scheduled.
    pub fn sync_engine(&mut self, shell: &mut Shell) {
        let running = shell.audio().engine_running();
        if running != self.server.program().state.engine_attached {
            self.set_engine_attached(running);
        }
        // Reports from an engine that has closed still count.
        for report in shell.audio_mut().take_reports() {
            self.on_report(shell, report);
        }
        let program = self.server.program_mut();
        if !running {
            // A transport command sent as the engine stopped has nothing to
            // move; scheduled commands wait for the next engine.
            program.state.outbox.retain(|message| {
                !matches!(
                    message,
                    EngineMessage::Transport(_) | EngineMessage::Session(_)
                )
            });
            return;
        }
        let generation = shell.audio().engine_generation();
        if program.engine_generation != Some(generation) {
            program.engine_generation = Some(generation);
            // The new engine holds none of the scheduled commands: send
            // every one still waiting, once.
            program.state.outbox.retain(|message| {
                matches!(
                    message,
                    EngineMessage::Transport(_) | EngineMessage::Session(_)
                )
            });
            let waiting = program.state.schedule.waiting_commands();
            program
                .state
                .outbox
                .extend(waiting.into_iter().map(EngineMessage::Schedule));
        }
        for message in std::mem::take(&mut program.state.outbox) {
            // The engine ran while the call was handled; if it has just
            // stopped, there's nothing to send to. A transport command goes
            // through the shell, so recording is set up as the UI's is.
            let _ = match message {
                EngineMessage::Transport(command) => shell.send_transport(command),
                EngineMessage::Session(command) => shell.audio_mut().send_session(command),
                EngineMessage::Schedule(command) => shell.audio_mut().schedule(command),
                EngineMessage::Cancel(id) => shell.audio_mut().cancel(id),
            };
        }
        let transport = shell.audio_mut().transport_state();
        program.state.playhead = Some(Playhead::new(
            transport.playing,
            transport.position,
            transport.insert_marker,
        ));
    }

    /// How many scheduled commands wait to fire, so a host keeps handing
    /// back the engine's reports while there are any.
    #[must_use]
    pub fn scheduled_waiting(&self) -> usize {
        self.server.program().state.schedule.waiting()
    }

    /// Whether tool calls or engine reports wait for the person's gesture
    /// to end, or view captures the host has answered wait to be handed
    /// back.
    #[must_use]
    pub fn holds_calls(&self) -> bool {
        self.server.holds_calls()
    }

    /// Once the person's gesture has ended, runs what waited for it in the
    /// order it came, each call as its own undo step: engine reports run
    /// their edits, and the next call that waited runs and gives its outcome
    /// with the ticket its [`Outcome::Wait`] had. Call it again until it
    /// gives `None`. While the gesture goes on, or a call it ran waits for
    /// the person's answer, it gives `None`. View captures the host has
    /// answered ([`Agent::finish_capture`]) come first, gesture or not.
    pub fn resume(&mut self, shell: &mut Shell) -> Option<(u64, Outcome)> {
        self.server.resume(shell)
    }

    /// The questions waiting for the person, oldest first.
    pub fn questions(&self) -> impl Iterator<Item = u64> + '_ {
        self.server.questions()
    }

    /// Handles one message from `session`. A message on a session that
    /// isn't open opens it, as stdio's one session is.
    pub fn handle(&mut self, shell: &mut Shell, session: &str, message: Value) -> Outcome {
        self.server.handle(shell, session, message)
    }

    /// Answers a question: `allow` is the person's answer. Returns the reply
    /// to the request that asked, or `None` if no question has that ID.
    ///
    /// Allowing an agent to connect allows it for good
    /// (`agent.approve_client`, run as the person), making it its own token,
    /// and the session gets its rights; if an agent with that name is
    /// already allowed, the session is allowed for itself only, with none
    /// of that agent's rights, since anyone can give a name. Allowing a call
    /// runs it.
    pub fn resolve(&mut self, shell: &mut Shell, question: u64, allow: bool) -> Option<Value> {
        self.server.resolve(shell, question, allow)
    }
}

/// A preview as `resources/list` lists it.
fn preview_listing(id: u64) -> Value {
    json!({
        "uri": perception::preview_uri(id),
        "name": format!("preview-{id}"),
        "title": format!("Audio preview {id}"),
        "mimeType": "audio/wav",
    })
}

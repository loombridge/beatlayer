//! The agent API's state on the main thread: the plug-in kit's MCP server
//! (`lspk_agent::Server`) serving Beatlayer's tools, with the project's
//! checkpoints and scheduled commands, view captures waiting for the
//! window, and the engine the host attaches.

use bl_engine::{EngineState, Report};
use bl_shell::Shell;
use bl_shell::agent::AGENT_APPROVE_CLIENT;
use bl_shell::perception_commands::{self as perception, CaptureResult, View};
use lspk_agent::{Call, Credential, Media, Outcome, Program, Server, ServerInfo, Session};
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
}

impl Program for Beatlayer {
    type Host = Shell;
    /// A report from the engine: a scheduled command fired, whose edit runs
    /// on the document.
    type Event = Report;
    type Deferred = Capture;

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
        };
        Ok(match tools::call(&mut ctx, name, args)? {
            // Spec: agent-perception R9. Only a window can capture a view.
            ToolOutcome::Defer(_) if !self.view_capture => ToolOutcome::Done(Err(
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

    fn defer(&mut self, ticket: u64, capture: &Capture) {
        self.capture_requests.push(CaptureRequest {
            ticket,
            view: capture.view,
            max_size: capture.max_size,
        });
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
                self.finish_capture(
                    ticket,
                    Err("the window closed before the view was captured".to_owned()),
                );
            }
            self.program().capture_requests.clear();
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

    /// Answers a capture request with the image, or why there's none. The
    /// waiting call's reply comes from [`Agent::resume`], with the ticket.
    pub fn finish_capture(&mut self, ticket: u64, result: Result<Captured, String>) {
        self.server.finish_deferred(ticket, |Capture { view, .. }| {
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
            program
                .state
                .outbox
                .retain(|message| !matches!(message, EngineMessage::Transport(_)));
            return;
        }
        let generation = shell.audio().engine_generation();
        if program.engine_generation != Some(generation) {
            program.engine_generation = Some(generation);
            // The new engine holds none of the scheduled commands: send
            // every one still waiting, once.
            program
                .state
                .outbox
                .retain(|message| matches!(message, EngineMessage::Transport(_)));
            let waiting = program.state.schedule.waiting_commands();
            program
                .state
                .outbox
                .extend(waiting.into_iter().map(EngineMessage::Schedule));
        }
        let audio = shell.audio_mut();
        for message in std::mem::take(&mut program.state.outbox) {
            // The engine ran while the call was handled; if it has just
            // stopped, there's nothing to send to.
            let _ = match message {
                EngineMessage::Transport(command) => audio.send_transport(command),
                EngineMessage::Schedule(command) => audio.schedule(command),
                EngineMessage::Cancel(id) => audio.cancel(id),
            };
        }
        let transport = audio.transport_state();
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

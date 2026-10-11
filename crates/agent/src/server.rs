//! The MCP server's state on the main thread: sessions, the methods MCP
//! defines, and calls waiting for the person.

use std::collections::BTreeMap;

use bl_engine::{EngineState, Report};
use bl_shell::Shell;
use bl_shell::agent::AGENT_APPROVE_CLIENT;
use serde_json::{Value, json};

use crate::guide;
use crate::jsonrpc::{self, Message, code};
use crate::permission::Mode;
use crate::tools::{self, Ctx, EngineMessage, Playhead, State, ToolOutcome};

/// The protocol versions this server speaks, newest first. A client asking
/// for another gets the newest.
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// The first version whose tool results carry `structuredContent`.
const STRUCTURED_FROM: &str = "2025-06-18";

/// The session ID `bl-cli mcp`'s one stdio session has.
pub const STDIO_SESSION: &str = "stdio";

/// Who's on the other end of a session, as the client said when it
/// connected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientInfo {
    /// Its name, such as `claude-code`: what the person allows.
    pub name: String,
    /// Its name for people, such as `Claude Code`, if it gave one.
    pub title: Option<String>,
    /// Its version, if it gave one.
    pub version: Option<String>,
}

impl ClientInfo {
    /// The name people see: the title if there is one.
    #[must_use]
    pub fn label(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.name)
    }
}

impl Default for ClientInfo {
    fn default() -> Self {
        Self {
            name: "agent".to_owned(),
            title: None,
            version: None,
        }
    }
}

/// One client's connection.
#[derive(Clone, Debug, Default)]
pub struct Session {
    /// Who it is.
    pub client: ClientInfo,
    /// The protocol version agreed, once it's initialised.
    pub protocol: Option<String>,
    /// The names `run` gave new IDs (`{"$new": name}`), for `{"$ref": name}`.
    pub refs: BTreeMap<String, u64>,
}

/// What handling a message came to.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Send this reply, or nothing for a notification or response.
    Reply(Option<Value>),
    /// Ask the person, then call [`Agent::resolve`] with the answer; the
    /// reply comes from there.
    Ask(Question),
}

/// Something only the person can answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    /// Its ID, for [`Agent::resolve`].
    pub id: u64,
    /// The agent asking, as people see its name.
    pub agent: String,
    /// What's asked.
    pub kind: QuestionKind,
}

/// What the person is asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuestionKind {
    /// An agent the person hasn't allowed wants to connect.
    Connect {
        /// Its name, which the approval is kept under.
        client: String,
    },
    /// An agent wants to run commands that ask first.
    Confirm {
        /// Their most far-reaching class: destructive or external.
        class: bl_commands::Permission,
        /// What would run, a line per command.
        summary: String,
    },
}

impl Question {
    /// The question as the person reads it, in a sentence or two.
    #[must_use]
    pub fn text(&self) -> String {
        match &self.kind {
            QuestionKind::Connect { client } => format!(
                "\u{201c}{}\u{201d} ({client}) wants to control Beatlayer through the agent \
                 server. Allow it? It will be able to read and edit this set, and to ask before \
                 deleting or reaching outside the project.",
                self.agent
            ),
            QuestionKind::Confirm { class, summary } => format!(
                "\u{201c}{}\u{201d} wants to run {} command{}:\n\n{summary}\n\nAllow it?",
                self.agent,
                class,
                if summary.lines().count() == 1 {
                    ""
                } else {
                    "s"
                }
            ),
        }
    }
}

/// A request waiting for the person's answer.
#[derive(Clone, Debug)]
struct Waiting {
    session: String,
    request: Value,
    kind: WaitingKind,
}

#[derive(Clone, Debug)]
enum WaitingKind {
    /// `initialize` from an agent not yet allowed; this is its result.
    Connect { result: Value },
    /// `tools/call`.
    Call { name: String, args: Value },
}

/// The agent API on the main thread: every session, the project's
/// checkpoints and scheduled commands, and calls waiting for the person.
///
/// A transport hands each message to [`Agent::handle`] with the session it
/// came on, and sends back the reply. Commands run on `shell`, the same
/// shell the UI drives, so an agent's command takes the same path as a
/// person's.
#[derive(Debug)]
pub struct Agent {
    mode: Mode,
    sessions: BTreeMap<String, Session>,
    state: State,
    waiting: BTreeMap<u64, Waiting>,
    next_question: u64,
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
            mode,
            sessions: BTreeMap::new(),
            state: State::default(),
            waiting: BTreeMap::new(),
            next_question: 0,
        }
    }

    /// Where the agent API runs.
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Starts a session with a new random ID, or the given one, and returns
    /// its ID.
    pub fn open_session(&mut self, id: Option<&str>) -> String {
        let id = id.map_or_else(new_session_id, str::to_owned);
        self.sessions.entry(id.clone()).or_default();
        id
    }

    /// Whether a session with this ID is open.
    #[must_use]
    pub fn has_session(&self, id: &str) -> bool {
        self.sessions.contains_key(id)
    }

    /// Ends a session. Calls it left waiting for the person still finish.
    pub fn close_session(&mut self, id: &str) {
        self.sessions.remove(id);
    }

    /// The open sessions, by ID, with who's on each.
    pub fn sessions(&self) -> impl Iterator<Item = (&str, &Session)> {
        self.sessions.iter().map(|(id, s)| (id.as_str(), s))
    }

    /// Ends every session of the agent called `client`, as forgetting its
    /// approval does.
    pub fn close_sessions_of(&mut self, client: &str) {
        self.sessions.retain(|_, s| s.client.name != client);
    }

    /// Says whether an engine plays the song here. While one does, the
    /// transport's playback commands go to it and scheduled commands fire;
    /// without one, the playback commands are refused and scheduled
    /// commands wait.
    pub fn set_engine_attached(&mut self, attached: bool) {
        self.state.engine_attached = attached;
        if !attached {
            self.state.playhead = None;
        }
    }

    /// What the engine must hear since the last call: transport commands,
    /// and commands scheduled and cancelled, in the order agents asked. The
    /// host sends them on to the engine's queue, the queue the UI's
    /// commands take, after syncing the document, so the engine has the song
    /// as the agent left it.
    pub fn take_engine_messages(&mut self) -> Vec<EngineMessage> {
        std::mem::take(&mut self.state.outbox)
    }

    /// Hands the agent API the engine's published state, so `transport`'s
    /// `status` can say whether the song plays and where the playhead and
    /// the insert marker are.
    pub fn on_engine_state(&mut self, state: &EngineState) {
        self.state.playhead = Some(Playhead::of(state));
    }

    /// Hands the agent API a report from the engine: a scheduled command
    /// that fired runs on the document now, as its agent's undo step.
    pub fn on_report(&mut self, shell: &mut Shell, report: Report) {
        self.state.schedule.on_report(shell, report);
    }

    /// The questions waiting for the person, oldest first.
    pub fn questions(&self) -> impl Iterator<Item = u64> + '_ {
        self.waiting.keys().copied()
    }

    /// Handles one message from `session`. A message on a session that
    /// isn't open opens it, as stdio's one session is.
    pub fn handle(&mut self, shell: &mut Shell, session: &str, message: Value) -> Outcome {
        let (id, method, params) = match Message::parse(message) {
            Err(error) => return Outcome::Reply(Some(error)),
            Ok(Message::Response) => return Outcome::Reply(None),
            Ok(Message::Notification { .. }) => {
                // `notifications/initialized` and `notifications/cancelled`
                // need nothing from this server.
                return Outcome::Reply(None);
            }
            Ok(Message::Request { id, method, params }) => (id, method, params),
        };
        let open = self.sessions.entry(session.to_owned()).or_default();
        if self.mode == Mode::App
            && method != "initialize"
            && open.protocol.is_some()
            && shell.agent_settings().client(&open.client.name).is_none()
        {
            let name = open.client.name.clone();
            self.sessions.remove(session);
            return Outcome::Reply(Some(jsonrpc::error_response(
                id,
                code::NOT_ALLOWED,
                format!(
                    "the person withdrew {name}'s approval, so this session is over; connect \
                     again to ask"
                ),
            )));
        }
        match method.as_str() {
            "initialize" => self.initialize(shell, session, id, &params),
            "ping" => reply(id, json!({})),
            "tools/list" => reply(id, json!({ "tools": tools::list() })),
            "tools/call" => self.call(shell, session, id, params, false),
            "resources/list" => reply(id, json!({ "resources": [guide::listing()] })),
            "resources/templates/list" => reply(id, json!({ "resourceTemplates": [] })),
            "resources/read" => {
                let uri = params
                    .get("uri")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match guide::read(uri) {
                    Some(contents) => reply(id, json!({ "contents": [contents] })),
                    None => Outcome::Reply(Some(jsonrpc::error_response(
                        id,
                        -32_002,
                        format!("no resource is at `{uri}`; the guide is at {}", guide::URI),
                    ))),
                }
            }
            "prompts/list" => reply(id, json!({ "prompts": [] })),
            "logging/setLevel" => reply(id, json!({})),
            other => Outcome::Reply(Some(jsonrpc::error_response(
                id,
                code::METHOD_NOT_FOUND,
                format!("this server has no method `{other}`"),
            ))),
        }
    }

    fn initialize(
        &mut self,
        shell: &mut Shell,
        session: &str,
        id: Value,
        params: &Value,
    ) -> Outcome {
        let asked = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(PROTOCOL_VERSIONS[0]);
        let version = PROTOCOL_VERSIONS
            .iter()
            .find(|v| **v == asked)
            .unwrap_or(&PROTOCOL_VERSIONS[0]);
        let info = params.get("clientInfo");
        let text = |key: &str| {
            info.and_then(|i| i.get(key))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        let client = ClientInfo {
            name: text("name").unwrap_or_else(|| "agent".to_owned()),
            title: text("title"),
            version: text("version"),
        };
        let entry = self.sessions.entry(session.to_owned()).or_default();
        entry.client = client.clone();
        entry.protocol = Some((*version).to_owned());
        let result = json!({
            "protocolVersion": version,
            "capabilities": {
                "tools": { "listChanged": false },
                "resources": { "listChanged": false, "subscribe": false },
            },
            "serverInfo": {
                "name": "beatlayer",
                "title": "Beatlayer",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "instructions": guide::INSTRUCTIONS,
        });
        if self.mode == Mode::App && shell.agent_settings().client(&client.name).is_none() {
            return self.ask(
                session,
                id,
                WaitingKind::Connect { result },
                client.label().to_owned(),
                QuestionKind::Connect {
                    client: client.name,
                },
            );
        }
        reply(id, result)
    }

    fn call(
        &mut self,
        shell: &mut Shell,
        session: &str,
        id: Value,
        params: Value,
        confirmed: bool,
    ) -> Outcome {
        let Some(name) = params
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return Outcome::Reply(Some(jsonrpc::error_response(
                id,
                code::INVALID_PARAMS,
                "`tools/call` needs the tool's `name`",
            )));
        };
        let args = params.get("arguments").cloned().unwrap_or(Value::Null);
        let Some(open) = self.sessions.get_mut(session) else {
            return Outcome::Reply(Some(jsonrpc::error_response(
                id,
                code::INVALID_REQUEST,
                "the session is over",
            )));
        };
        let structured = open
            .protocol
            .as_deref()
            .is_none_or(|v| v >= STRUCTURED_FROM);
        let mut ctx = Ctx {
            mode: self.mode,
            shell,
            session: open,
            state: &mut self.state,
            confirmed,
        };
        let agent = ctx.agent().to_owned();
        match tools::call(&mut ctx, &name, args.clone()) {
            Err(message) => Outcome::Reply(Some(jsonrpc::error_response(
                id,
                code::INVALID_PARAMS,
                message,
            ))),
            Ok(ToolOutcome::Done(result)) => reply(id, tool_result(result, structured)),
            Ok(ToolOutcome::Ask { class, summary }) => self.ask(
                session,
                id,
                WaitingKind::Call { name, args },
                agent,
                QuestionKind::Confirm { class, summary },
            ),
        }
    }

    fn ask(
        &mut self,
        session: &str,
        request: Value,
        kind: WaitingKind,
        agent: String,
        question: QuestionKind,
    ) -> Outcome {
        self.next_question += 1;
        let id = self.next_question;
        self.waiting.insert(
            id,
            Waiting {
                session: session.to_owned(),
                request,
                kind,
            },
        );
        Outcome::Ask(Question {
            id,
            agent,
            kind: question,
        })
    }

    /// Answers a question: `allow` is the person's answer. Returns the reply
    /// to the request that asked, or `None` if no question has that ID.
    ///
    /// Allowing an agent to connect allows it for good
    /// (`agent.approve_client`, run as the person). Allowing a call runs it.
    pub fn resolve(&mut self, shell: &mut Shell, question: u64, allow: bool) -> Option<Value> {
        let Waiting {
            session,
            request,
            kind,
        } = self.waiting.remove(&question)?;
        Some(match kind {
            WaitingKind::Connect { result } => {
                let name = self
                    .sessions
                    .get(&session)
                    .map(|s| s.client.name.clone())
                    .unwrap_or_default();
                if allow
                    && shell
                        .run(AGENT_APPROVE_CLIENT, json!({ "client": name }))
                        .is_ok()
                {
                    jsonrpc::response(request, result)
                } else {
                    self.sessions.remove(&session);
                    jsonrpc::error_response(
                        request,
                        code::NOT_ALLOWED,
                        format!("the person didn't allow {name} to control Beatlayer"),
                    )
                }
            }
            WaitingKind::Call { name, args } => {
                if allow {
                    match self.call(
                        shell,
                        &session,
                        request.clone(),
                        json!({ "name": name, "arguments": args }),
                        true,
                    ) {
                        Outcome::Reply(Some(reply)) => reply,
                        // It was confirmed, so it never asks again.
                        Outcome::Reply(None) | Outcome::Ask(_) => jsonrpc::error_response(
                            request,
                            code::INTERNAL_ERROR,
                            "the call asked twice",
                        ),
                    }
                } else {
                    let structured = self
                        .sessions
                        .get(&session)
                        .and_then(|s| s.protocol.as_deref())
                        .is_none_or(|v| v >= STRUCTURED_FROM);
                    jsonrpc::response(
                        request,
                        tool_result(
                            Err(format!(
                                "the person declined to let you run this ({name}). Nothing \
                                 changed. Ask them before trying it again."
                            )),
                            structured,
                        ),
                    )
                }
            }
        })
    }
}

fn reply(id: Value, result: Value) -> Outcome {
    Outcome::Reply(Some(jsonrpc::response(id, result)))
}

/// A tool's result as MCP carries it: the JSON as text for every client,
/// and as structured content for clients that read it.
fn tool_result(result: Result<Value, String>, structured: bool) -> Value {
    match result {
        Ok(value) => {
            let text = serde_json::to_string_pretty(&value).unwrap_or_default();
            let mut result = json!({
                "content": [{ "type": "text", "text": text }],
                "isError": false,
            });
            if structured && value.is_object() {
                result["structuredContent"] = value;
            }
            result
        }
        Err(message) => json!({
            "content": [{ "type": "text", "text": message }],
            "isError": true,
        }),
    }
}

/// A new random session ID: 16 bytes from the operating system's secure
/// random source, as hexadecimal digits.
fn new_session_id() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // A session ID only has to be unique; the token is the secret.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        bytes = nanos.to_le_bytes();
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

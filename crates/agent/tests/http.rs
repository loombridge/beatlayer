//! The app's agent server over Streamable HTTP, started headless: a
//! scripted HTTP client connects, is allowed and given its own token, edits
//! the project, and waits while the person confirms a destructive command
//! or finishes a drag. And every request is checked for its token, its
//! host, and its origin, and a client presenting another agent's name gets
//! none of its rights (WP-1.15b).
//!
//! Each test's thread plays the app's main thread: it takes the server's
//! jobs and runs them on the shell, as the app's event loop does, while
//! the client runs on a thread of its own.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use bl_agent::client::{Client, Http};
use bl_agent::http::{HttpConfig, HttpServer, Parked, Pending, Served, resume, serve};
use bl_agent::{Agent, QuestionKind};
use bl_shell::{GesturePhase, Invocation, Shell};
use serde_json::{Value, json};

struct App {
    agent: Agent,
    shell: Shell,
    server: HttpServer,
    token: String,
    /// Calls waiting for the person's gesture to end.
    parked: Vec<Parked>,
}

impl App {
    fn start() -> Self {
        let token = bl_shell::agent::new_token().unwrap();
        let server = HttpServer::start(
            &HttpConfig {
                port: 0,
                token: token.clone(),
            },
            Arc::new(|| {}),
        )
        .unwrap();
        Self {
            agent: Agent::for_app(),
            shell: Shell::headless(),
            server,
            token,
            parked: Vec::new(),
        }
    }

    /// The own token of the allowed agent called `name`, as the Settings
    /// window would show it.
    fn token_of(&self, name: &str) -> String {
        self.shell
            .agent_settings()
            .client(name)
            .unwrap_or_else(|| panic!("{name} isn't allowed"))
            .token
            .expose()
            .to_owned()
    }

    /// Serves the next job, if one comes within a few milliseconds, then
    /// runs what waited for the person's gesture if it has ended, as the
    /// app's event loop does. Questions go to `answer`.
    fn step(&mut self, answer: &mut impl FnMut(&mut Self, Pending)) {
        // The allowed agents' tokens, as the host gives them.
        self.server.set_agent_tokens(self.shell.agent_settings());
        if let Some(job) = self.server.wait_job(Duration::from_millis(5)) {
            let served = serve(&mut self.agent, &mut self.shell, job);
            self.served(served, answer);
        }
        while let Some(served) = resume(&mut self.agent, &mut self.shell, &mut self.parked) {
            self.served(served, answer);
        }
    }

    fn served(&mut self, served: Served, answer: &mut impl FnMut(&mut Self, Pending)) {
        match served {
            Served::Done => {}
            Served::Waiting(pending) => answer(self, pending),
            Served::Parked(parked) => self.parked.push(parked),
        }
    }

    fn addr(&self) -> SocketAddr {
        self.server.local_addr()
    }

    /// Runs `client` on its own thread while this thread serves the jobs it
    /// sends, as the app's main thread does. Questions go to `answer`, which
    /// must finish them.
    fn run<R: Send + 'static>(
        &mut self,
        client: impl FnOnce(SocketAddr, String) -> R + Send + 'static,
        answer: impl FnMut(&mut Self, Pending),
    ) -> R {
        let token = self.token.clone();
        self.run_with(token, client, answer)
    }

    /// As [`App::run`], the client given `token` rather than the server's.
    fn run_with<R: Send + 'static>(
        &mut self,
        token: String,
        client: impl FnOnce(SocketAddr, String) -> R + Send + 'static,
        mut answer: impl FnMut(&mut Self, Pending),
    ) -> R {
        let addr = self.addr();
        self.server.set_agent_tokens(self.shell.agent_settings());
        let handle = thread::spawn(move || client(addr, token));
        let until = Instant::now() + Duration::from_secs(60);
        while !handle.is_finished() {
            assert!(Instant::now() < until, "the client took too long");
            self.step(&mut answer);
        }
        handle.join().unwrap()
    }

    fn track_names(&self) -> Vec<String> {
        let document = self.shell.editor().document();
        document
            .top_level()
            .map(|id| document.track(id).unwrap().name().to_owned())
            .collect()
    }
}

/// Allows every question.
fn allow(app: &mut App, pending: Pending) {
    pending.finish(&mut app.agent, &mut app.shell, true);
}

#[test]
fn an_agent_client_connects_is_allowed_once_and_edits_the_project() {
    let mut app = App::start();
    let mut questions = Vec::new();
    let (init, tools, result) = app.run(
        |addr, token| {
            let mut client = Client::new(Http::new(addr, token));
            let init = client.initialize("claude-code").unwrap();
            assert!(client.transport().session().is_some());
            let tools = client.tools().unwrap();
            let result = client
                .call(
                    "run",
                    json!({
                        "label": "Add drums",
                        "commands": [
                            { "command": "track.create",
                              "args": { "id": {"$new": "drums"}, "kind": "audio",
                                        "name": "Drums", "index": 0 } },
                            { "command": "mixer.set_volume",
                              "args": { "track": {"$ref": "drums"}, "volume": 0.5 } },
                        ],
                    }),
                )
                .unwrap()
                .ok()
                .unwrap();
            (init, tools.len(), result)
        },
        |app, pending| {
            questions.push(pending.question().clone());
            allow(app, pending);
        },
    );
    assert_eq!(init["serverInfo"]["name"], "beatlayer");
    assert_eq!(tools, 10);
    assert_eq!(result["undo_step"], "claude-code: Add drums");
    assert_eq!(app.track_names()[0], "Drums");
    assert_eq!(
        app.shell.editor().undo_history().next(),
        Some("claude-code: Add drums")
    );
    assert!(app.shell.is_dirty());

    // The person was asked once, to let it connect, and the approval stays,
    // with a token of its own.
    assert_eq!(questions.len(), 1);
    assert!(matches!(
        &questions[0].kind,
        QuestionKind::Connect { client } if client == "claude-code"
    ));
    assert!(app.shell.agent_settings().client("claude-code").is_some());
    let own = app.token_of("claude-code");
    assert_eq!(own.len(), 64);
    assert_ne!(own, app.token);
    // Spec: agent-permissions R7. With its own token it isn't asked about
    // again.
    let asked_again = app.run_with(
        own,
        |addr, token| {
            let mut client = Client::new(Http::new(addr, token));
            client.initialize("claude-code").unwrap();
            client.call("query", json!({})).unwrap().ok().unwrap()
        },
        |_, _| panic!("an agent with its own token isn't asked about again"),
    );
    assert_eq!(asked_again["tracks"]["items"][0]["name"], "Drums");
    // With the server's token, anyone can give that name, so it's asked
    // about again.
    let mut asked = 0;
    app.run(
        |addr, token| {
            let mut client = Client::new(Http::new(addr, token));
            client.initialize("claude-code").unwrap();
        },
        |app, pending| {
            asked += 1;
            allow(app, pending);
        },
    );
    assert_eq!(asked, 1);
}

#[test]
fn a_destructive_command_waits_for_the_person_s_confirmation() {
    let mut app = App::start();
    app.shell
        .run("agent.approve_client", json!({ "client": "claude-code" }))
        .unwrap();
    let answered = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&answered);
    let mut confirmations = 0;
    let own = app.token_of("claude-code");
    let results = app.run_with(
        own,
        move |addr, token| {
            let mut client = Client::new(Http::new(addr, token));
            client.initialize("claude-code").unwrap();
            let delete = |client: &mut Client<Http>, track: u64| {
                let result = client
                    .call(
                        "run",
                        json!({ "commands": [{ "command": "track.delete",
                                               "args": { "track": track } }] }),
                    )
                    .unwrap();
                seen.store(true, Ordering::SeqCst);
                result
            };
            let allowed = delete(&mut client, 2);
            seen.store(false, Ordering::SeqCst);
            let declined = delete(&mut client, 3);
            (allowed, declined)
        },
        |app, pending| {
            confirmations += 1;
            let QuestionKind::Confirm { class, summary, .. } = &pending.question().kind else {
                panic!("expected a confirmation");
            };
            assert_eq!(class.id(), "destructive");
            assert!(
                summary.starts_with("Delete Track (destructive): track"),
                "{summary}"
            );
            assert!(pending.question().text().contains("Allow it?"));
            // The call waits: the track is still there, and the client has
            // no answer, however long the person takes.
            thread::sleep(Duration::from_millis(300));
            assert!(!answered.load(Ordering::SeqCst), "the call didn't wait");
            assert_eq!(app.track_names().len(), 4 - (confirmations - 1));
            let allow = confirmations == 1;
            pending.finish(&mut app.agent, &mut app.shell, allow);
        },
    );
    assert_eq!(confirmations, 2);
    let (allowed, declined) = results;
    assert!(!allowed.is_error, "{}", allowed.text);
    assert!(declined.is_error);
    assert!(declined.text.contains("declined"), "{}", declined.text);
    assert_eq!(app.track_names(), ["2 MIDI", "3 Audio", "4 Audio"]);
}

#[test]
fn an_agent_the_person_refuses_gets_no_session() {
    let mut app = App::start();
    let error = app.run(
        |addr, token| {
            let mut client = Client::new(Http::new(addr, token));
            let error = client.initialize("stranger").unwrap_err();
            assert!(client.transport().session().is_none());
            error
        },
        |app, pending| pending.finish(&mut app.agent, &mut app.shell, false),
    );
    assert!(error.contains("didn't allow"), "{error}");
    assert!(app.shell.agent_settings().clients.is_empty());
    assert_eq!(app.agent.sessions().count(), 0);
}

fn header(name: &str, value: impl Into<String>) -> (String, String) {
    (name.to_owned(), value.into())
}

#[test]
fn requests_without_the_token_from_web_pages_or_for_other_hosts_are_refused() {
    let mut app = App::start();
    let port = app.addr().port();
    let initialize = serde_json::to_vec(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2025-11-25", "capabilities": {},
                    "clientInfo": { "name": "probe" } },
    }))
    .unwrap();
    let responses = app.run(
        move |addr, token| {
            let mut http = Http::new(addr, token.clone());
            let host = header("Host", format!("127.0.0.1:{port}"));
            let bearer = header("Authorization", format!("Bearer {token}"));
            let mut send = |headers: Vec<(String, String)>, method: &str, path: &str| {
                http.exchange(
                    method,
                    path,
                    &headers,
                    if method == "POST" { &initialize } else { &[] },
                )
                .unwrap()
            };
            vec![
                ("no token", send(vec![host.clone()], "POST", "/mcp")),
                (
                    "wrong token",
                    send(
                        vec![host.clone(), header("Authorization", "Bearer 0123")],
                        "POST",
                        "/mcp",
                    ),
                ),
                (
                    "a web page",
                    send(
                        vec![
                            host.clone(),
                            bearer.clone(),
                            header("Origin", "https://evil.example"),
                        ],
                        "POST",
                        "/mcp",
                    ),
                ),
                (
                    "a rebound domain",
                    send(
                        vec![
                            header("Host", format!("evil.example:{port}")),
                            bearer.clone(),
                        ],
                        "POST",
                        "/mcp",
                    ),
                ),
                ("no host", send(vec![bearer.clone()], "POST", "/mcp")),
                (
                    "another path",
                    send(vec![host.clone(), bearer.clone()], "POST", "/other"),
                ),
                (
                    "a stream",
                    send(vec![host.clone(), bearer.clone()], "GET", "/mcp"),
                ),
                (
                    "an unknown version",
                    send(
                        vec![
                            host.clone(),
                            bearer.clone(),
                            header("MCP-Protocol-Version", "1999-01-01"),
                        ],
                        "POST",
                        "/mcp",
                    ),
                ),
            ]
        },
        |_, _| panic!("nothing refused reaches the person"),
    );
    let expected = [
        ("no token", 401),
        ("wrong token", 401),
        ("a web page", 403),
        ("a rebound domain", 403),
        ("no host", 403),
        ("another path", 404),
        ("a stream", 405),
        ("an unknown version", 400),
    ];
    for ((name, response), (expected_name, status)) in responses.iter().zip(expected) {
        assert_eq!(*name, expected_name);
        assert_eq!(response.status, status, "{name}: {:?}", response.json());
    }
    assert_eq!(
        responses[0].1.header("www-authenticate"),
        Some("Bearer realm=\"beatlayer\"")
    );
    assert_eq!(responses[6].1.header("allow"), Some("POST, DELETE"));
    // None of them reached the agent API.
    assert_eq!(app.agent.sessions().count(), 0);
    assert!(app.shell.agent_settings().clients.is_empty());
}

#[test]
fn sessions_are_required_after_initialize_and_end_on_delete() {
    let mut app = App::start();
    app.shell
        .run("agent.approve_client", json!({ "client": "probe" }))
        .unwrap();
    let port = app.addr().port();
    let statuses = app.run(
        move |addr, token| {
            let mut client = Client::new(Http::new(addr, token.clone()));
            client.initialize("probe").unwrap();
            let session = client.transport().session().unwrap().to_owned();
            let http = client.transport();
            let base = vec![
                header("Host", format!("localhost:{port}")),
                header("Authorization", format!("Bearer {token}")),
            ];
            let ping = serde_json::to_vec(&json!({ "jsonrpc": "2.0", "id": 9, "method": "ping" }))
                .unwrap();
            let with = |extra: Vec<(String, String)>| {
                let mut headers = base.clone();
                headers.extend(extra);
                headers
            };
            let no_session = http.exchange("POST", "/mcp", &base, &ping).unwrap().status;
            let unknown = http
                .exchange(
                    "POST",
                    "/mcp",
                    &with(vec![header("Mcp-Session-Id", "nope")]),
                    &ping,
                )
                .unwrap()
                .status;
            let mine = vec![header("Mcp-Session-Id", session.clone())];
            let ok = http
                .exchange("POST", "/mcp", &with(mine.clone()), &ping)
                .unwrap();
            let notification = serde_json::to_vec(
                &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            )
            .unwrap();
            let accepted = http
                .exchange("POST", "/mcp", &with(mine.clone()), &notification)
                .unwrap()
                .status;
            let not_json = http
                .exchange("POST", "/mcp", &with(mine.clone()), b"{not json")
                .unwrap();
            let deleted = http
                .exchange("DELETE", "/mcp", &with(mine.clone()), &[])
                .unwrap()
                .status;
            let after = http
                .exchange("POST", "/mcp", &with(mine), &ping)
                .unwrap()
                .status;
            (
                no_session,
                unknown,
                ok.status,
                ok.json(),
                accepted,
                not_json.status,
                not_json.json(),
                deleted,
                after,
            )
        },
        allow,
    );
    let (no_session, unknown, ok, pong, accepted, not_json, parse_error, deleted, after) = statuses;
    assert_eq!(no_session, 400);
    assert_eq!(unknown, 404);
    assert_eq!(ok, 200);
    assert_eq!(pong["result"], json!({}));
    assert_eq!(accepted, 202);
    assert_eq!(not_json, 400);
    assert_eq!(parse_error["error"]["code"], -32_700);
    assert_eq!(deleted, 204);
    assert_eq!(after, 404);
}

#[test]
fn chunked_bodies_and_expect_continue_are_read() {
    use std::io::{Read, Write};
    let mut app = App::start();
    app.shell
        .run("agent.approve_client", json!({ "client": "chunky" }))
        .unwrap();
    let reply = app.run(
        |addr, token| {
            let body = serde_json::to_string(&json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": "2025-06-18", "capabilities": {},
                            "clientInfo": { "name": "chunky" } },
            }))
            .unwrap();
            let (first, second) = body.split_at(body.len() / 2);
            let mut stream = std::net::TcpStream::connect(addr).unwrap();
            write!(
                stream,
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {token}\r\n\
                 Content-Type: application/json\r\nTransfer-Encoding: chunked\r\n\
                 Expect: 100-continue\r\nConnection: close\r\n\r\n",
                addr.port()
            )
            .unwrap();
            write!(
                stream,
                "{:x}\r\n{first}\r\n{:x};ext=1\r\n{second}\r\n0\r\n\r\n",
                first.len(),
                second.len()
            )
            .unwrap();
            let mut text = String::new();
            stream.read_to_string(&mut text).unwrap();
            text
        },
        allow,
    );
    assert!(
        reply.starts_with("HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK"),
        "{reply}"
    );
    assert!(reply.contains("Mcp-Session-Id: "), "{reply}");
    let body: Value = serde_json::from_str(reply.split("\r\n\r\n").last().unwrap()).unwrap();
    assert_eq!(body["result"]["protocolVersion"], "2025-06-18");
}

#[test]
fn stopping_the_server_closes_it() {
    let app = App::start();
    let addr = app.addr();
    assert!(std::net::TcpStream::connect(addr).is_ok());
    app.server.stop();
    // Nothing listens on the port any more.
    thread::sleep(Duration::from_millis(50));
    assert!(std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_err());
}

/// A bar of 4/4, in ticks.
const BAR: u64 = 2_822_400;

/// Puts an empty MIDI clip called `name` on the first track, from `bar`
/// (from 0) for `bars`, as the person would have, and returns its ID.
fn midi_clip(shell: &mut Shell, name: &str, bar: u64, bars: u64) -> u64 {
    let track = shell.editor().document().top_level().next().unwrap();
    let id = shell.new_id::<bl_core::id::Id>().get();
    shell
        .run(
            "clip.create",
            json!({ "track": track.0.get(), "clip": {
                "id": id, "name": name, "start": bar * BAR, "length": bars * BAR,
                "mute": false, "data": { "midi": { "notes": [] } } } }),
        )
        .unwrap();
    id
}

fn ping(addr: SocketAddr, token: &str, session: &str) -> u16 {
    let mut http = Http::new(addr, token.to_owned());
    let body = serde_json::to_vec(&json!({ "jsonrpc": "2.0", "id": 9, "method": "ping" })).unwrap();
    http.exchange(
        "POST",
        "/mcp",
        &[
            header("Host", format!("127.0.0.1:{}", addr.port())),
            header("Authorization", format!("Bearer {token}")),
            header("Mcp-Session-Id", session),
        ],
        &body,
    )
    .unwrap()
    .status
}

#[test]
fn a_client_presenting_another_agent_s_name_gets_none_of_its_rights() {
    // Spec: agent-permissions R7, R8, R9.
    let mut app = App::start();
    app.shell
        .run("agent.approve_client", json!({ "client": "claude-code" }))
        .unwrap();
    app.shell
        .run(
            "agent.set_autonomous",
            json!({ "client": "claude-code", "on": true }),
        )
        .unwrap();
    let own = app.token_of("claude-code");
    let delete = json!({ "commands": [{ "command": "track.delete", "args": { "track": 2 } }] });

    // With the server's token and the autonomous agent's name, a client is
    // asked about, as any client with the server's token is.
    let mut asked = Vec::new();
    let refused = app.run(
        |addr, token| {
            Client::new(Http::new(addr, token))
                .initialize("claude-code")
                .unwrap_err()
        },
        |app, pending| {
            asked.push(pending.question().clone());
            pending.finish(&mut app.agent, &mut app.shell, false);
        },
    );
    assert!(refused.contains("didn't allow"), "{refused}");
    assert!(matches!(&asked[..], [q] if matches!(q.kind, QuestionKind::Connect { .. })));

    // Allowed by a person who can't tell it from the real one by its name,
    // it still asks before deleting: it isn't autonomous.
    let mut confirmations = 0;
    let impostor = {
        let delete = delete.clone();
        app.run(
            move |addr, token| {
                let mut client = Client::new(Http::new(addr, token));
                client.initialize("claude-code").unwrap();
                client.call("run", delete).unwrap()
            },
            |app, pending| {
                let allow = match pending.question().kind {
                    QuestionKind::Connect { .. } => true,
                    QuestionKind::Confirm { .. } => {
                        confirmations += 1;
                        false
                    }
                };
                pending.finish(&mut app.agent, &mut app.shell, allow);
            },
        )
    };
    assert_eq!(confirmations, 1, "the impostor had to ask");
    assert!(impostor.is_error && impostor.text.contains("declined"));
    assert_eq!(app.track_names().len(), 4);
    // Allowing it didn't take over the real agent's approval or token.
    assert_eq!(app.shell.agent_settings().clients.len(), 1);
    assert_eq!(app.token_of("claude-code"), own);

    // The agent's own token is what makes it that agent, whatever name its
    // client gives: it's autonomous, and its undo steps carry its name.
    let deleted = {
        let delete = delete.clone();
        app.run_with(
            own.clone(),
            move |addr, token| {
                let mut client = Client::new(Http::new(addr, token));
                client.initialize("someone-else").unwrap();
                client.call("run", delete).unwrap().ok().unwrap()
            },
            |_, _| panic!("the agent with its own token is autonomous"),
        )
    };
    assert_eq!(deleted["undo_step"], "claude-code: Delete Track");
    assert_eq!(app.track_names().len(), 3);

    // A session belongs to the token it started with: the server's token
    // can't use the agent's session.
    let server = app.token.clone();
    let (theirs, mine) = app.run_with(
        own.clone(),
        move |addr, token| {
            let mut client = Client::new(Http::new(addr, token.clone()));
            client.initialize("claude-code").unwrap();
            let session = client.transport().session().unwrap().to_owned();
            (ping(addr, &server, &session), ping(addr, &token, &session))
        },
        |_, _| panic!("nothing asks"),
    );
    assert_eq!((theirs, mine), (404, 200));

    // A new token for the agent: the old one no longer speaks for it.
    app.shell
        .run("agent.new_client_token", json!({ "client": "claude-code" }))
        .unwrap();
    assert_ne!(app.token_of("claude-code"), own);
    let old = app.run_with(
        own,
        |addr, token| {
            Client::new(Http::new(addr, token))
                .initialize("claude-code")
                .unwrap_err()
        },
        |_, _| panic!("an old token reaches no one"),
    );
    assert!(old.contains("HTTP 401"), "{old}");
}

#[test]
fn an_agent_s_clip_move_over_another_clip_waits_for_confirmation() {
    // Spec: agent-permissions R1, R2. Into empty space it runs; over
    // another clip it waits for the person, then replaces what it covers,
    // as the person's own move does.
    let mut app = App::start();
    app.shell
        .run("agent.approve_client", json!({ "client": "editor" }))
        .unwrap();
    let own = app.token_of("editor");
    let a = midi_clip(&mut app.shell, "A", 0, 1);
    let b = midi_clip(&mut app.shell, "B", 4, 1);
    let answered = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&answered);
    let mut confirmations = Vec::new();
    let (empty, over) = app.run_with(
        own,
        move |addr, token| {
            let mut client = Client::new(Http::new(addr, token));
            client.initialize("editor").unwrap();
            let move_to = |client: &mut Client<Http>, at: &str| {
                client
                    .call(
                        "run",
                        json!({ "commands": [{ "command": "arrangement.move_clips",
                                               "args": { "clips": [a], "to": at } }] }),
                    )
                    .unwrap()
                    .ok()
                    .unwrap()
            };
            let empty = move_to(&mut client, "3.1.1");
            let over = move_to(&mut client, "5.1.1");
            seen.store(true, Ordering::SeqCst);
            (empty, over)
        },
        |app, pending| {
            let QuestionKind::Confirm {
                classes, summary, ..
            } = &pending.question().kind
            else {
                panic!("expected a confirmation");
            };
            confirmations.push((classes.clone(), summary.clone()));
            // The call waits: the clip it would cover is still there.
            thread::sleep(Duration::from_millis(300));
            assert!(!answered.load(Ordering::SeqCst), "the call didn't wait");
            assert!(app.shell.editor().document().clip(clip_id(b)).is_some());
            pending.finish(&mut app.agent, &mut app.shell, true);
        },
    );
    assert_eq!(empty["asked_first"], false);
    assert_eq!(empty["undo_step"], "editor: Move Clips");
    assert_eq!(confirmations.len(), 1, "only the move over a clip asked");
    let (classes, summary) = &confirmations[0];
    assert_eq!(classes, &[bl_commands::Permission::Destructive]);
    assert!(summary.starts_with("Move Clips (destructive)"), "{summary}");
    assert_eq!(over["asked_first"], true);
    let document = app.shell.editor().document();
    assert!(document.clip(clip_id(b)).is_none(), "B was covered");
    assert_eq!(
        document.clip(clip_id(a)).unwrap().start.get(),
        4 * BAR as i64
    );
}

fn clip_id(id: u64) -> bl_model::ClipId {
    bl_model::ClipId(bl_core::id::Id::new(id).unwrap())
}

#[test]
fn an_agent_edit_sent_mid_drag_lands_after_the_drop_as_its_own_undo_step() {
    // Spec: agent-permissions R5, R6.
    let mut app = App::start();
    app.shell
        .run("agent.approve_client", json!({ "client": "editor" }))
        .unwrap();
    let own = app.token_of("editor");
    let clip = midi_clip(&mut app.shell, "Riff", 0, 1);
    let drag = |by: u64| {
        Invocation::new(
            "arrangement.move_clips",
            json!({ "clips": [clip], "by": by * BAR }),
        )
    };
    let steps = app.shell.editor().undo_history().len();
    // The person starts dragging the clip.
    app.shell
        .gesture(GesturePhase::Update, Some(&drag(1)))
        .unwrap();

    // An agent renames a track meanwhile.
    let addr = app.addr();
    app.server.set_agent_tokens(app.shell.agent_settings());
    let client = thread::spawn(move || {
        let mut client = Client::new(Http::new(addr, own));
        client.initialize("editor").unwrap();
        client
            .call(
                "run",
                json!({ "commands": [{ "command": "track.rename",
                                       "args": { "track": 3, "name": "Keys" } }] }),
            )
            .unwrap()
            .ok()
            .unwrap()
    });
    let until = Instant::now() + Duration::from_secs(30);
    let mut never = |_: &mut App, _: Pending| panic!("nothing asks");
    while app.parked.is_empty() {
        assert!(Instant::now() < until, "the call never came");
        app.step(&mut never);
    }
    // Mid-drag it waits: nothing of it has landed, and the client has no
    // answer.
    for _ in 0..20 {
        app.step(&mut never);
    }
    assert!(!client.is_finished(), "the call didn't wait for the drop");
    assert!(app.track_names().iter().all(|name| name != "Keys"));

    // The drag goes on and is dropped.
    app.shell
        .gesture(GesturePhase::Update, Some(&drag(2)))
        .unwrap();
    app.shell.gesture(GesturePhase::Commit, None).unwrap();
    while !client.is_finished() {
        assert!(Instant::now() < until, "the call never ran");
        app.step(&mut never);
    }
    let result = client.join().unwrap();
    assert_eq!(result["undo_step"], "editor: Rename Track");

    // Two steps: the drag, then the agent's edit, each undone on its own.
    let history: Vec<String> = app
        .shell
        .editor()
        .undo_history()
        .map(str::to_owned)
        .collect();
    assert_eq!(history.len(), steps + 2);
    assert_eq!(history[..2], ["editor: Rename Track", "Move clips"]);
    let start = |app: &App| {
        app.shell
            .editor()
            .document()
            .clip(clip_id(clip))
            .unwrap()
            .start
            .get()
    };
    assert_eq!(start(&app), 2 * BAR as i64);
    app.shell.run("edit.undo", json!({})).unwrap();
    assert!(app.track_names().iter().all(|name| name != "Keys"));
    assert_eq!(start(&app), 2 * BAR as i64, "the drag stays");
    app.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(start(&app), 0);
}

/// A real HTTP client, curl, as a check on the server's HTTP: it sends the
/// initialize request and a tool call on one kept-alive connection. Run on
/// demand, where curl is installed:
/// `cargo test -p bl-agent --test http -- --ignored --nocapture`.
#[test]
#[ignore = "needs curl on the PATH"]
fn curl_talks_to_the_server() {
    let mut app = App::start();
    app.shell
        .run("agent.approve_client", json!({ "client": "curl" }))
        .unwrap();
    let output = app.run(
        |addr, token| {
            let url = format!("http://{addr}/mcp");
            let init = json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": "2025-11-25", "capabilities": {},
                            "clientInfo": { "name": "curl" } },
            })
            .to_string();
            std::process::Command::new("curl")
                .args([
                    "--silent",
                    "--show-error",
                    "--include",
                    "-H",
                    &format!("Authorization: Bearer {token}"),
                    "-H",
                    "Content-Type: application/json",
                    "-H",
                    "Accept: application/json, text/event-stream",
                    "--data",
                    &init,
                    &url,
                ])
                .output()
                .expect("curl runs")
        },
        allow,
    );
    let text = String::from_utf8_lossy(&output.stdout);
    eprintln!("{text}");
    assert!(output.status.success());
    assert!(text.starts_with("HTTP/1.1 200 OK"), "{text}");
    assert!(text.contains("Mcp-Session-Id"), "{text}");
    let body: Value = serde_json::from_str(text.split("\r\n\r\n").last().unwrap()).unwrap();
    assert_eq!(body["result"]["serverInfo"]["name"], "beatlayer");
}

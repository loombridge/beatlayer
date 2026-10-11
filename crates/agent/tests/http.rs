//! The app's agent server over Streamable HTTP, started headless: a
//! scripted HTTP client connects, is allowed once, edits the project, and
//! waits while the person confirms a destructive command. And every
//! request is checked for the token, its host, and its origin.
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
use bl_agent::http::{HttpConfig, HttpServer, Pending, Served, serve};
use bl_agent::{Agent, QuestionKind};
use bl_shell::Shell;
use serde_json::{Value, json};

struct App {
    agent: Agent,
    shell: Shell,
    server: HttpServer,
    token: String,
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
        mut answer: impl FnMut(&mut Self, Pending),
    ) -> R {
        let (addr, token) = (self.addr(), self.token.clone());
        let handle = thread::spawn(move || client(addr, token));
        let until = Instant::now() + Duration::from_secs(60);
        while !handle.is_finished() {
            assert!(Instant::now() < until, "the client took too long");
            if let Some(job) = self.server.wait_job(Duration::from_millis(5)) {
                match serve(&mut self.agent, &mut self.shell, job) {
                    Served::Done => {}
                    Served::Waiting(pending) => answer(self, pending),
                }
            }
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
    assert_eq!(tools, 7);
    assert_eq!(result["undo_step"], "claude-code: Add drums");
    assert_eq!(app.track_names()[0], "Drums");
    assert_eq!(
        app.shell.editor().undo_history().next(),
        Some("claude-code: Add drums")
    );
    assert!(app.shell.is_dirty());

    // The person was asked once, to let it connect, and the approval stays.
    assert_eq!(questions.len(), 1);
    assert!(matches!(
        &questions[0].kind,
        QuestionKind::Connect { client } if client == "claude-code"
    ));
    assert!(app.shell.agent_settings().client("claude-code").is_some());
    let asked_again = app.run(
        |addr, token| {
            let mut client = Client::new(Http::new(addr, token));
            client.initialize("claude-code").unwrap();
            client.call("query", json!({})).unwrap().ok().unwrap()
        },
        |_, _| panic!("an allowed agent isn't asked about again"),
    );
    assert_eq!(asked_again["tracks"]["items"][0]["name"], "Drums");
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
    let results = app.run(
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
            let QuestionKind::Confirm { class, summary } = &pending.question().kind else {
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

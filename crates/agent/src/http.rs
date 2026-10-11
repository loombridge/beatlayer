//! The Streamable HTTP transport, which the running app serves on
//! `127.0.0.1` while the person has the agent server switched on.
//!
//! It's MCP's Streamable HTTP over a small HTTP/1.1 server of Beatlayer's
//! own, which needs no async runtime: one thread accepts connections and
//! one thread serves each, up to [`MAX_CONNECTIONS`]. Every request is
//! checked before anything else sees it:
//!
//! - It must name `127.0.0.1`, `localhost`, or `[::1]` with the server's
//!   port as its host, so a web page that rebinds a domain to this machine
//!   is refused (DNS-rebinding protection).
//! - It must carry no `Origin` header: browsers send one, agent clients
//!   don't, so requests from web pages are refused.
//! - It must carry the server's token as a bearer token.
//!
//! A request that passes goes to the main thread as a [`Job`], through the
//! same event loop as the UI's events, so commands run where the person's
//! do. The connection's thread waits for the answer, however long the
//! person takes to confirm. The server answers each POST with one JSON
//! response (MCP allows that instead of an event stream), refuses GET with
//! 405 because it starts no streams of its own, and ends a session on
//! DELETE.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use bl_shell::Shell;
use bl_shell::agent::ENDPOINT_PATH;
use serde_json::Value;

use crate::jsonrpc::{self, code};
use crate::server::{Agent, Outcome, PROTOCOL_VERSIONS, Question};

/// The most connections served at once. More are refused with 503.
pub const MAX_CONNECTIONS: usize = 32;
/// The largest request head (request line and headers), in bytes.
pub const MAX_HEAD: usize = 16 * 1024;
/// The largest request body, in bytes.
pub const MAX_BODY: usize = 8 * 1024 * 1024;
/// How long an idle connection stays open.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// What the server needs to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpConfig {
    /// The port on `127.0.0.1`; 0 picks a free one.
    pub port: u16,
    /// The bearer token every request must carry.
    pub token: String,
}

/// A request for the main thread.
#[derive(Debug)]
pub struct Job {
    request: JobRequest,
    reply: mpsc::Sender<HttpReply>,
}

#[derive(Debug)]
enum JobRequest {
    Message {
        session: Option<String>,
        message: Value,
    },
    Delete {
        session: String,
    },
}

/// The main thread's answer to a job.
#[derive(Debug)]
struct HttpReply {
    status: u16,
    session: Option<String>,
    body: Option<Value>,
}

/// What [`serve`] did with a job.
#[derive(Debug)]
pub enum Served {
    /// It's answered.
    Done,
    /// It waits for the person: show [`Pending::question`], then call
    /// [`Pending::finish`] with the answer.
    Waiting(Pending),
}

/// A job waiting for the person's answer.
#[derive(Debug)]
pub struct Pending {
    question: Question,
    reply: mpsc::Sender<HttpReply>,
    session: String,
    initialize: bool,
}

impl Pending {
    /// What the person is asked.
    #[must_use]
    pub fn question(&self) -> &Question {
        &self.question
    }

    /// Answers the question and sends the reply to the waiting connection.
    pub fn finish(self, agent: &mut Agent, shell: &mut Shell, allow: bool) {
        let body = agent.resolve(shell, self.question.id, allow);
        let failed = body.as_ref().is_none_or(|b| b.get("error").is_some());
        let session = (self.initialize && !failed).then_some(self.session);
        let _ = self.reply.send(HttpReply {
            status: 200,
            session,
            body,
        });
    }
}

/// Runs one job on the main thread: hands its message to the agent API and
/// sends the reply, or returns it to wait for the person.
pub fn serve(agent: &mut Agent, shell: &mut Shell, job: Job) -> Served {
    let Job { request, reply } = job;
    let send = |status, session: Option<String>, body: Option<Value>| {
        let _ = reply.send(HttpReply {
            status,
            session,
            body,
        });
    };
    match request {
        JobRequest::Delete { session } => {
            if agent.has_session(&session) {
                agent.close_session(&session);
                send(204, None, None);
            } else {
                send(404, None, Some(error_body("no session has that ID")));
            }
            Served::Done
        }
        JobRequest::Message { session, message } => {
            let initialize = jsonrpc::is_request(&message)
                && message.get("method").and_then(Value::as_str) == Some("initialize");
            let session = match (session, initialize) {
                (_, true) => agent.open_session(None),
                (Some(id), false) if agent.has_session(&id) => id,
                (Some(_), false) => {
                    send(
                        404,
                        None,
                        Some(error_body(
                            "the session has ended or never existed; initialize again",
                        )),
                    );
                    return Served::Done;
                }
                (None, false) => {
                    send(
                        400,
                        None,
                        Some(error_body(
                            "requests after `initialize` must carry its Mcp-Session-Id header",
                        )),
                    );
                    return Served::Done;
                }
            };
            match agent.handle(shell, &session, message) {
                Outcome::Reply(None) => {
                    send(202, None, None);
                    Served::Done
                }
                Outcome::Reply(Some(body)) => {
                    let ok = body.get("error").is_none();
                    if initialize && !ok {
                        agent.close_session(&session);
                    }
                    send(200, (initialize && ok).then_some(session), Some(body));
                    Served::Done
                }
                Outcome::Ask(question) => Served::Waiting(Pending {
                    question,
                    reply,
                    session,
                    initialize,
                }),
            }
        }
    }
}

fn error_body(message: &str) -> Value {
    jsonrpc::error_response(Value::Null, code::INVALID_REQUEST, message)
}

/// What every connection thread shares.
struct Shared {
    token: String,
    port: u16,
    jobs: Mutex<mpsc::Sender<Job>>,
    wake: Arc<dyn Fn() + Send + Sync>,
    stop: AtomicBool,
    connections: Mutex<Vec<(u64, TcpStream)>>,
    next_connection: AtomicU64,
}

/// The agent server: listening on `127.0.0.1`, handing requests to the
/// main thread as [`Job`]s. Dropping it stops it.
pub struct HttpServer {
    addr: SocketAddr,
    shared: Arc<Shared>,
    jobs: mpsc::Receiver<Job>,
    acceptor: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for HttpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpServer")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

impl HttpServer {
    /// Starts listening. `wake` is called, on a connection's thread, each
    /// time a job is ready, so the main thread's event loop can take it.
    ///
    /// # Errors
    ///
    /// If the port can't be bound, such as when another program has it, or
    /// the token is empty.
    pub fn start(config: &HttpConfig, wake: Arc<dyn Fn() + Send + Sync>) -> io::Result<Self> {
        if config.token.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the agent server needs a token",
            ));
        }
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, config.port))?;
        let addr = listener.local_addr()?;
        let (sender, jobs) = mpsc::channel();
        let shared = Arc::new(Shared {
            token: config.token.clone(),
            port: addr.port(),
            jobs: Mutex::new(sender),
            wake,
            stop: AtomicBool::new(false),
            connections: Mutex::new(Vec::new()),
            next_connection: AtomicU64::new(0),
        });
        let accepting = Arc::clone(&shared);
        let acceptor = thread::Builder::new()
            .name("agent-server".to_owned())
            .spawn(move || accept(&listener, &accepting))?;
        Ok(Self {
            addr,
            shared,
            jobs,
            acceptor: Some(acceptor),
        })
    }

    /// The address it listens on.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// The address agents connect to.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}{ENDPOINT_PATH}", self.addr)
    }

    /// The next job waiting for the main thread, if there is one.
    #[must_use]
    pub fn try_job(&self) -> Option<Job> {
        self.jobs.try_recv().ok()
    }

    /// Waits up to `timeout` for the next job, for tests and headless hosts
    /// that have no event loop to wake.
    #[must_use]
    pub fn wait_job(&self, timeout: Duration) -> Option<Job> {
        self.jobs.recv_timeout(timeout).ok()
    }

    /// How many connections are open.
    #[must_use]
    pub fn connections(&self) -> usize {
        self.shared
            .connections
            .lock()
            .map_or(0, |connections| connections.len())
    }

    /// Stops listening and closes every connection. Requests waiting for
    /// the main thread get 503.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        // Wake the accepting thread, which waits in `accept`.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_secs(1));
        if let Some(acceptor) = self.acceptor.take() {
            let _ = acceptor.join();
        }
        if let Ok(connections) = self.shared.connections.lock() {
            for (_, stream) in connections.iter() {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        if self.acceptor.is_some() {
            self.shutdown();
        }
    }
}

fn accept(listener: &TcpListener, shared: &Arc<Shared>) {
    for stream in listener.incoming() {
        if shared.stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(mut stream) = stream else {
            continue;
        };
        let Ok(registered) = stream.try_clone() else {
            continue;
        };
        let id = shared.next_connection.fetch_add(1, Ordering::Relaxed);
        {
            let Ok(mut connections) = shared.connections.lock() else {
                return;
            };
            if connections.len() >= MAX_CONNECTIONS {
                drop(connections);
                let _ = write_response(
                    &mut stream,
                    &Response::error(503, "too many connections; try again", true),
                );
                continue;
            }
            connections.push((id, registered));
        }
        let serving = Arc::clone(shared);
        let spawned = thread::Builder::new()
            .name("agent-connection".to_owned())
            .spawn(move || {
                let _ = connection(stream, &serving);
                if let Ok(mut connections) = serving.connections.lock() {
                    connections.retain(|(other, _)| *other != id);
                }
            });
        if spawned.is_err()
            && let Ok(mut connections) = shared.connections.lock()
        {
            connections.retain(|(other, _)| *other != id);
        }
    }
}

/// A parsed request.
#[derive(Debug)]
struct Request {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    close: bool,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// A response to write.
#[derive(Debug)]
struct Response {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
    close: bool,
}

impl Response {
    fn json(status: u16, body: &Value) -> Self {
        Self {
            status,
            headers: vec![("Content-Type", "application/json".to_owned())],
            body: serde_json::to_vec(body).unwrap_or_default(),
            close: false,
        }
    }

    fn empty(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
            close: false,
        }
    }

    fn error(status: u16, message: &str, close: bool) -> Self {
        let mut response = Self::json(status, &error_body(message));
        response.close = close;
        response
    }

    fn with(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }
}

/// Why a request couldn't be read.
#[derive(Debug)]
enum ReadError {
    /// The connection closed, failed, or idled out: nothing to answer.
    Closed,
    /// The request is malformed or too large: answer with this status.
    Bad(u16, &'static str),
}

impl From<io::Error> for ReadError {
    fn from(_: io::Error) -> Self {
        Self::Closed
    }
}

/// Serves one connection until it closes, idles out, or the server stops.
fn connection(stream: TcpStream, shared: &Shared) -> io::Result<()> {
    stream.set_read_timeout(Some(IDLE_TIMEOUT))?;
    stream.set_nodelay(true)?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    while !shared.stop.load(Ordering::SeqCst) {
        let request = match read_request(&mut reader, &mut writer) {
            Ok(Some(request)) => request,
            Ok(None) | Err(ReadError::Closed) => return Ok(()),
            Err(ReadError::Bad(status, message)) => {
                return write_response(&mut writer, &Response::error(status, message, true));
            }
        };
        let close = request.close;
        let mut response = respond(shared, request);
        response.close |= close;
        write_response(&mut writer, &response)?;
        if response.close {
            return Ok(());
        }
    }
    Ok(())
}

/// Reads one line of the request head, counting it against `budget`.
fn read_line(reader: &mut impl BufRead, budget: &mut usize) -> Result<Option<String>, ReadError> {
    let mut line = Vec::new();
    let read = reader
        .take(*budget as u64 + 1)
        .read_until(b'\n', &mut line)?;
    if read == 0 {
        return Ok(None);
    }
    if read > *budget {
        return Err(ReadError::Bad(431, "the request's head is too large"));
    }
    *budget -= read;
    if !line.ends_with(b"\n") {
        return Err(ReadError::Closed);
    }
    let text = String::from_utf8(line)
        .map_err(|_| ReadError::Bad(400, "the request's head isn't UTF-8"))?;
    Ok(Some(text.trim_end_matches(['\r', '\n']).to_owned()))
}

fn read_request(
    reader: &mut BufReader<TcpStream>,
    writer: &mut TcpStream,
) -> Result<Option<Request>, ReadError> {
    let mut budget = MAX_HEAD;
    // Blank lines before a request are allowed (RFC 9112, 2.2).
    let start = loop {
        match read_line(reader, &mut budget)? {
            None => return Ok(None),
            Some(line) if line.is_empty() => {}
            Some(line) => break line,
        }
    };
    let mut parts = start.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(ReadError::Bad(400, "the request line isn't HTTP"));
    };
    if !matches!(version, "HTTP/1.1" | "HTTP/1.0") {
        return Err(ReadError::Bad(505, "only HTTP/1.1 is served"));
    }
    let mut headers = Vec::new();
    loop {
        let Some(line) = read_line(reader, &mut budget)? else {
            return Err(ReadError::Closed);
        };
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(ReadError::Bad(400, "a header has no colon"));
        };
        if name.is_empty() || name.ends_with(' ') || line.starts_with([' ', '\t']) {
            return Err(ReadError::Bad(400, "a header is malformed"));
        }
        headers.push((name.to_ascii_lowercase(), value.trim().to_owned()));
    }
    let header = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    let connection = header("connection").map(str::to_ascii_lowercase);
    let close = version == "HTTP/1.0" && connection.as_deref() != Some("keep-alive")
        || connection.as_deref() == Some("close");
    let chunked = header("transfer-encoding")
        .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"));
    let length = header("content-length");
    if chunked && length.is_some() {
        return Err(ReadError::Bad(
            400,
            "a request can't have both a length and chunked encoding",
        ));
    }
    if header("transfer-encoding").is_some() && !chunked {
        return Err(ReadError::Bad(
            501,
            "only chunked transfer encoding is served",
        ));
    }
    let length = match length {
        Some(text) => Some(
            text.parse::<usize>()
                .map_err(|_| ReadError::Bad(400, "the content length isn't a number"))?,
        ),
        None => None,
    };
    if length.is_some_and(|n| n > MAX_BODY) {
        return Err(ReadError::Bad(413, "the request's body is too large"));
    }
    if (chunked || length.is_some_and(|n| n > 0))
        && header("expect").is_some_and(|e| e.eq_ignore_ascii_case("100-continue"))
    {
        writer.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
    }
    let body = if chunked {
        read_chunked(reader)?
    } else {
        let mut body = vec![0; length.unwrap_or(0)];
        reader.read_exact(&mut body)?;
        body
    };
    Ok(Some(Request {
        method: method.to_owned(),
        target: target.to_owned(),
        headers,
        body,
        close,
    }))
}

fn read_chunked(reader: &mut impl BufRead) -> Result<Vec<u8>, ReadError> {
    let mut body = Vec::new();
    let mut budget = MAX_HEAD;
    loop {
        let Some(line) = read_line(reader, &mut budget)? else {
            return Err(ReadError::Closed);
        };
        let size = line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size, 16)
            .map_err(|_| ReadError::Bad(400, "a chunk's size isn't hexadecimal"))?;
        if size == 0 {
            // Trailers, up to the blank line.
            loop {
                match read_line(reader, &mut budget)? {
                    Some(line) if line.is_empty() => return Ok(body),
                    Some(_) => {}
                    None => return Err(ReadError::Closed),
                }
            }
        }
        if body.len() + size > MAX_BODY {
            return Err(ReadError::Bad(413, "the request's body is too large"));
        }
        let start = body.len();
        body.resize(start + size, 0);
        reader.read_exact(&mut body[start..])?;
        let mut end = [0u8; 2];
        reader.read_exact(&mut end)?;
        if &end != b"\r\n" {
            return Err(ReadError::Bad(400, "a chunk doesn't end with CRLF"));
        }
    }
}

/// Whether two strings are equal, taking the same time wherever they differ.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |diff, (x, y)| diff | (x ^ y))
            == 0
}

/// Checks a request and answers it, through the main thread when it's for
/// the agent API.
fn respond(shared: &Shared, request: Request) -> Response {
    // DNS rebinding: the host must be this machine by address or by name.
    let port = shared.port;
    let host_ok = request.header("host").is_some_and(|host| {
        let host = host.to_ascii_lowercase();
        [
            format!("127.0.0.1:{port}"),
            format!("localhost:{port}"),
            format!("[::1]:{port}"),
        ]
        .contains(&host)
    });
    if !host_ok {
        return Response::error(
            403,
            "the request's Host must be 127.0.0.1 or localhost with the server's port",
            true,
        );
    }
    if request.header("origin").is_some() {
        return Response::error(
            403,
            "requests from web pages (with an Origin header) are refused",
            true,
        );
    }
    let path = request.target.split('?').next().unwrap_or_default();
    if path != ENDPOINT_PATH {
        return Response::error(404, "the agent API is at /mcp", false);
    }
    let authorised = request
        .header("authorization")
        .and_then(|value| value.split_once(' '))
        .is_some_and(|(scheme, token)| {
            scheme.eq_ignore_ascii_case("bearer") && same(token.trim(), &shared.token)
        });
    if !authorised {
        return Response::error(
            401,
            "the request needs the agent server's token as a bearer token (Settings, Tempo & \
             MIDI, Agents)",
            false,
        )
        .with("WWW-Authenticate", "Bearer realm=\"beatlayer\"");
    }
    if let Some(version) = request.header("mcp-protocol-version")
        && !PROTOCOL_VERSIONS.contains(&version)
    {
        return Response::error(
            400,
            "this server doesn't speak that MCP-Protocol-Version",
            false,
        );
    }
    let session = request.header("mcp-session-id").map(str::to_owned);
    let job = match request.method.as_str() {
        "POST" => {
            if let Some(accept) = request.header("accept") {
                let accept = accept.to_ascii_lowercase();
                if !accept.contains("application/json") && !accept.contains("*/*") {
                    return Response::error(406, "the server answers with application/json", false);
                }
            }
            if request
                .header("content-type")
                .is_some_and(|t| !t.to_ascii_lowercase().starts_with("application/json"))
            {
                return Response::error(415, "send JSON-RPC as application/json", false);
            }
            let message = match serde_json::from_slice::<Value>(&request.body) {
                Ok(message) => message,
                Err(error) => {
                    return Response::json(
                        400,
                        &jsonrpc::error_response(
                            Value::Null,
                            code::PARSE_ERROR,
                            format!("the body isn't JSON: {error}"),
                        ),
                    );
                }
            };
            JobRequest::Message { session, message }
        }
        "DELETE" => match session {
            Some(session) => JobRequest::Delete { session },
            None => return Response::error(400, "DELETE needs an Mcp-Session-Id", false),
        },
        "GET" => {
            return Response::error(
                405,
                "this server starts no event streams; POST JSON-RPC messages",
                false,
            )
            .with("Allow", "POST, DELETE");
        }
        _ => {
            return Response::error(405, "use POST", false).with("Allow", "POST, DELETE");
        }
    };
    let (reply, answer) = mpsc::channel();
    let sent = shared.jobs.lock().ok().is_some_and(|jobs| {
        jobs.send(Job {
            request: job,
            reply,
        })
        .is_ok()
    });
    if !sent {
        return Response::error(503, "the agent server is stopping", true);
    }
    (shared.wake)();
    match answer.recv() {
        Ok(HttpReply {
            status,
            session,
            body,
        }) => {
            let mut response = match body {
                Some(body) => Response::json(status, &body),
                None => Response::empty(status),
            };
            if let Some(session) = session {
                response = response.with("Mcp-Session-Id", session);
            }
            response
        }
        Err(_) => Response::error(503, "the agent server stopped", true),
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        413 => "Content Too Large",
        415 => "Unsupported Media Type",
        431 => "Request Header Fields Too Large",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        505 => "HTTP Version Not Supported",
        _ => "Unknown",
    }
}

fn write_response(stream: &mut TcpStream, response: &Response) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n",
        response.status,
        reason(response.status),
        response.body.len()
    );
    for (name, value) in &response.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    if response.close {
        head.push_str("Connection: close\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(&response.body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_compare_whole() {
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd"));
        assert!(!same("abc", "ab"));
        assert!(!same("", "a"));
    }

    #[test]
    fn json_is_a_body_and_errors_say_why() {
        let response = Response::error(401, "no token", false).with("WWW-Authenticate", "Bearer");
        assert_eq!(response.status, 401);
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(body["error"]["message"], "no token");
        assert_eq!(reason(405), "Method Not Allowed");
    }
}

//! The agent API exactly as `bl-cli mcp` serves it: `bl_agent::stdio::serve`
//! with a headless, autonomous agent on a headless shell, on a thread of
//! its own, over in-memory pipes in place of standard input and output.
//!
//! It exists to prove work finishes with no window: nothing here polls the
//! shell or runs a render for the server, so what lands, lands because the
//! stdio transport made it land.

use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use bl_agent::Agent;
use bl_agent::client::{Client, Transport};
use bl_shell::Shell;
use serde_json::{Value, json};

use crate::Driver;

/// How long a request waits for its reply before the host gives up: a
/// guard so a server that never answers fails the test rather than hanging
/// the suite. No reply is expected to come anywhere near it.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(120);

/// The name the host's agent connects with.
pub const AGENT_NAME: &str = "e2e-stdio";

/// The server's standard input: the lines the client sends, until the
/// client hangs up.
struct Input {
    lines: Receiver<Vec<u8>>,
    chunk: Vec<u8>,
    at: usize,
}

impl Read for Input {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let chunk = self.fill_buf()?;
        let n = chunk.len().min(buf.len());
        buf[..n].copy_from_slice(&chunk[..n]);
        self.consume(n);
        Ok(n)
    }
}

impl BufRead for Input {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.at == self.chunk.len() {
            self.at = 0;
            // A closed pipe is the end of the input.
            self.chunk = self.lines.recv().unwrap_or_default();
        }
        Ok(&self.chunk[self.at..])
    }

    fn consume(&mut self, amount: usize) {
        self.at += amount;
    }
}

/// The server's standard output: each line it writes goes to the client.
struct Output {
    lines: Sender<Vec<u8>>,
    pending: Vec<u8>,
}

impl Write for Output {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            // A client that has gone reads nothing more.
            let _ = self.lines.send(line);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The client's end of the pipes.
#[derive(Debug)]
pub struct Pipes {
    to_server: Option<Sender<Vec<u8>>>,
    from_server: Receiver<Vec<u8>>,
}

impl Transport for Pipes {
    fn send(&mut self, message: &Value) -> Result<Option<Value>, String> {
        let to_server = self
            .to_server
            .as_ref()
            .ok_or("the server's input is closed")?;
        let mut line = serde_json::to_vec(message).map_err(|e| e.to_string())?;
        line.push(b'\n');
        to_server
            .send(line)
            .map_err(|_| "the server has stopped".to_owned())?;
        if !bl_agent::jsonrpc::is_request(message) {
            return Ok(None);
        }
        let id = &message["id"];
        loop {
            let line = match self.from_server.recv_timeout(REPLY_TIMEOUT) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(format!(
                        "no reply to {} within {} s",
                        message["method"],
                        REPLY_TIMEOUT.as_secs()
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("the server closed its output".to_owned());
                }
            };
            let reply: Value = serde_json::from_slice(&line)
                .map_err(|error| format!("the server wrote something that isn't JSON: {error}"))?;
            if reply.get("id") == Some(id) {
                return Ok(Some(reply));
            }
        }
    }
}

/// `bl-cli mcp --autonomous` in this process: the agent API served over
/// stdio by a thread of its own, and an MCP client connected to it.
pub struct StdioHost {
    client: Client<Pipes>,
    server: Option<JoinHandle<Result<(), String>>>,
}

impl std::fmt::Debug for StdioHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdioHost").finish_non_exhaustive()
    }
}

impl StdioHost {
    /// Serves a new set, and connects as [`AGENT_NAME`].
    ///
    /// # Panics
    ///
    /// If the client can't connect.
    #[must_use]
    pub fn start() -> Self {
        Self::open(None)
    }

    /// Serves the set at `set`, opened first as `bl-cli mcp --set` opens
    /// it, or a new set with `None`, and connects as [`AGENT_NAME`].
    ///
    /// # Panics
    ///
    /// If the client can't connect. A set that doesn't open makes the
    /// server stop, so the connection fails, naming the set's error.
    #[must_use]
    pub fn open(set: Option<PathBuf>) -> Self {
        let (to_server, input) = mpsc::channel();
        let (output, from_server) = mpsc::channel();
        let server = std::thread::Builder::new()
            .name("bl-e2e-stdio".to_owned())
            .spawn(move || {
                let mut shell = Shell::headless();
                if let Some(set) = &set {
                    shell
                        .run(
                            bl_shell::commands::SET_OPEN,
                            json!({ "path": set.to_string_lossy() }),
                        )
                        .map_err(|error| format!("the set didn't open: {error}"))?;
                }
                let mut agent = Agent::headless(true);
                bl_agent::stdio::serve(
                    &mut agent,
                    &mut shell,
                    Input {
                        lines: input,
                        chunk: Vec::new(),
                        at: 0,
                    },
                    Output {
                        lines: output,
                        pending: Vec::new(),
                    },
                )
                .map_err(|error| format!("the transport failed: {error}"))
            })
            .expect("a thread for the server");
        let mut host = Self {
            client: Client::new(Pipes {
                to_server: Some(to_server),
                from_server,
            }),
            server: Some(server),
        };
        if let Err(error) = host.client.initialize(AGENT_NAME) {
            let stopped = host.finish().err().unwrap_or_default();
            panic!("couldn't connect over stdio: {error} {stopped}");
        }
        host
    }

    /// The MCP client, for requests other than tool calls (`ping`,
    /// `resources/read`, `tools/list`).
    pub fn client(&mut self) -> &mut Client<Pipes> {
        &mut self.client
    }

    /// Sends a JSON-RPC request and gives its result.
    ///
    /// # Errors
    ///
    /// The server's error, or why the request couldn't be made.
    pub fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.client.request(method, params)
    }

    /// Closes the server's input, which ends it as end of file ends
    /// `bl-cli mcp`, and waits for its thread.
    ///
    /// # Errors
    ///
    /// Why the server stopped, if it failed or panicked.
    pub fn finish(mut self) -> Result<(), String> {
        self.stop()
    }

    fn stop(&mut self) -> Result<(), String> {
        self.client.transport().to_server = None;
        match self.server.take().map(JoinHandle::join) {
            None | Some(Ok(Ok(()))) => Ok(()),
            Some(Ok(Err(error))) => Err(error),
            Some(Err(_)) => Err("the server's thread panicked".to_owned()),
        }
    }
}

impl Drop for StdioHost {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

impl Driver for StdioHost {
    fn tool(&mut self, name: &str, args: Value) -> Result<Value, String> {
        self.client.call(name, args)?.ok()
    }
}

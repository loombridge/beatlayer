//! The stdio transport: JSON-RPC messages, one per line, on standard input,
//! and replies, one per line, on standard output. `bl-cli mcp` serves the
//! agent API this way, with no window, on any platform. The transport is the
//! plug-in kit's (`lspk_agent::stdio`).

use std::io::{self, BufRead, Read, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

use bl_shell::{Effect, Shell};

use crate::server::Agent;

/// The longest message read, in bytes. A longer line is refused.
pub use lspk_agent::stdio::MAX_MESSAGE;

/// How long the server waits for input before it polls the shell anyway.
const IDLE: Duration = Duration::from_millis(50);

/// A message the server answers with nothing: it stands in for "time to
/// poll" in the input, which the transport reads as it would any line.
const TICK: &[u8] = b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n";

/// Serves the agent API on `input` and `output` until `input` ends or an
/// agent runs `app.quit`.
///
/// The shell is polled before every message and every 50 ms while no one
/// writes, as the app's window polls it every frame, so background work
/// (a stem separation, a processed file) lands without a window. A poll
/// never writes to `output`.
///
/// Headless there's no person to answer a question, so a call that would
/// ask is refused by the agent's permissions before it gets here; one that
/// still does (an agent not made with [`Agent::headless`]) is declined.
///
/// # Errors
///
/// An I/O error reading `input` or writing `output`.
pub fn serve(
    agent: &mut Agent,
    shell: &mut Shell,
    input: impl BufRead + Send + 'static,
    output: impl Write,
) -> io::Result<()> {
    // The transport blocks reading, so a thread of its own reads and hands
    // over whole lines (cut at the longest message), one at a time; the
    // reader below turns a wait into a tick.
    let (send, lines) = mpsc::sync_channel(0);
    std::thread::spawn(move || {
        let mut input = input;
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = input
                .by_ref()
                .take(MAX_MESSAGE as u64 + 1)
                .read_until(b'\n', &mut line);
            if !matches!(read, Ok(1..)) || send.send(line.clone()).is_err() {
                return;
            }
        }
    });
    // The transport calls the closure after every message, ticks included:
    // it polls, then, headless, the only effect a command can ask for that
    // matters is quitting (`app.quit`): the server stops after its reply.
    lspk_agent::stdio::serve_until(
        agent.server(),
        shell,
        Ticking {
            lines,
            chunk: Vec::new(),
            at: 0,
            mid_line: false,
        },
        output,
        |shell| {
            shell.poll();
            shell.take_effects().contains(&Effect::Quit)
        },
    )
}

/// Standard input's lines, with a [`TICK`] before each and in every idle
/// wait. A tick never lands inside a line.
struct Ticking {
    lines: Receiver<Vec<u8>>,
    chunk: Vec<u8>,
    at: usize,
    mid_line: bool,
}

impl Read for Ticking {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let chunk = self.fill_buf()?;
        let n = chunk.len().min(buf.len());
        buf[..n].copy_from_slice(&chunk[..n]);
        self.consume(n);
        Ok(n)
    }
}

impl BufRead for Ticking {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.at == self.chunk.len() {
            self.chunk.clear();
            self.at = 0;
            let next = if self.mid_line {
                // The rest of a line: no tick inside it.
                self.lines.recv().map_err(|_| RecvTimeoutError::Disconnected)
            } else {
                self.chunk.extend_from_slice(TICK);
                self.lines.recv_timeout(IDLE)
            };
            match next {
                Ok(line) => self.chunk.extend_from_slice(&line),
                Err(RecvTimeoutError::Timeout) => {}
                // Input ended: what's left is the end.
                Err(RecvTimeoutError::Disconnected) => self.chunk.clear(),
            }
            self.mid_line = self.chunk.last().is_some_and(|b| *b != b'\n');
        }
        Ok(&self.chunk[self.at..])
    }

    fn consume(&mut self, amount: usize) {
        self.at += amount;
    }
}

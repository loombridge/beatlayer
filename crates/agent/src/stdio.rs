//! The stdio transport: JSON-RPC messages, one per line, on standard input,
//! and replies, one per line, on standard output. `bl-cli mcp` serves the
//! agent API this way, with no window, on any platform.

use std::io::{self, BufRead, Read, Write};

use bl_shell::{Effect, Shell};
use serde_json::Value;

use crate::jsonrpc::{self, code};
use crate::server::{Agent, Outcome, STDIO_SESSION};

/// The longest message read, in bytes. A longer line is refused.
pub const MAX_MESSAGE: usize = 16 * 1024 * 1024;

/// Serves the agent API on `input` and `output` until `input` ends or an
/// agent runs `app.quit`.
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
    input: impl BufRead,
    mut output: impl Write,
) -> io::Result<()> {
    agent.open_session(Some(STDIO_SESSION));
    let mut lines = input;
    let mut line = String::new();
    loop {
        line.clear();
        let read = lines
            .by_ref()
            .take(MAX_MESSAGE as u64 + 1)
            .read_line(&mut line)?;
        if read == 0 {
            return Ok(());
        }
        if line.len() > MAX_MESSAGE && !line.ends_with('\n') {
            // Skip the rest of the line.
            let mut rest = Vec::new();
            lines.read_until(b'\n', &mut rest)?;
            write_line(
                &mut output,
                &jsonrpc::error_response(
                    Value::Null,
                    code::INVALID_REQUEST,
                    format!("a message can be at most {MAX_MESSAGE} bytes"),
                ),
            )?;
            continue;
        }
        let text = line.trim();
        if text.is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(text) {
            Err(error) => Some(jsonrpc::error_response(
                Value::Null,
                code::PARSE_ERROR,
                format!("the message isn't JSON: {error}"),
            )),
            Ok(message) => match agent.handle(shell, STDIO_SESSION, message) {
                Outcome::Reply(reply) => reply,
                Outcome::Ask(question) => agent.resolve(shell, question.id, false),
                // Headless, no person's gesture is ever under way, so
                // nothing waits for one to end; run it now if anything did.
                Outcome::Wait(ticket) => agent
                    .resume(shell)
                    .filter(|(resumed, _)| *resumed == ticket)
                    .and_then(|(_, outcome)| match outcome {
                        Outcome::Reply(reply) => reply,
                        Outcome::Ask(question) => agent.resolve(shell, question.id, false),
                        Outcome::Wait(_) => None,
                    }),
            },
        };
        if let Some(reply) = reply {
            write_line(&mut output, &reply)?;
        }
        // Headless, the only effect a command can ask for that matters is
        // quitting (`app.quit`): the server stops after its reply.
        if shell.take_effects().contains(&Effect::Quit) {
            return Ok(());
        }
    }
}

fn write_line(output: &mut impl Write, message: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, message)?;
    output.write_all(b"\n")?;
    output.flush()
}

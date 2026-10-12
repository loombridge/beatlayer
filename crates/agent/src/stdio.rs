//! The stdio transport: JSON-RPC messages, one per line, on standard input,
//! and replies, one per line, on standard output. `bl-cli mcp` serves the
//! agent API this way, with no window, on any platform. The transport is the
//! plug-in kit's (`lspk_agent::stdio`).

use std::io::{self, BufRead, Write};

use bl_shell::{Effect, Shell};

use crate::server::Agent;

/// The longest message read, in bytes. A longer line is refused.
pub use lspk_agent::stdio::MAX_MESSAGE;

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
    output: impl Write,
) -> io::Result<()> {
    // Headless, the only effect a command can ask for that matters is
    // quitting (`app.quit`): the server stops after its reply.
    lspk_agent::stdio::serve_until(agent.server(), shell, input, output, |shell| {
        shell.take_effects().contains(&Effect::Quit)
    })
}

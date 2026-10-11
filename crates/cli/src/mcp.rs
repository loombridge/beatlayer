//! `bl-cli mcp`: the agent API over stdio, with no window (WP-1.15).
//!
//! An agent client starts `bl-cli mcp` and speaks MCP on its standard input
//! and output, one JSON-RPC message per line, to build or change projects
//! on any platform, a Linux server included. Commands run on a headless
//! shell, the one the app's window drives, with the null audio device.
//! There's no person to ask, so destructive and external commands are
//! refused unless `--autonomous` is given.

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use bl_agent::Agent;
use bl_shell::{Shell, commands as ids};
use serde_json::json;

use crate::Outcome;

#[derive(Debug, clap::Args)]
pub struct McpArgs {
    /// A set to open first: a `.blset` file, or a project folder holding
    /// one set. Without it, the factory default set is open.
    #[arg(long)]
    pub set: Option<PathBuf>,

    /// Let agents run destructive commands (deleting, discarding,
    /// overwriting) and external ones (files outside the project, such as
    /// saving a set elsewhere) without asking. Headless there's no one to
    /// ask, so without this flag they're refused.
    #[arg(long)]
    pub autonomous: bool,
}

/// Runs `bl-cli mcp` until standard input ends.
///
/// # Errors
///
/// A message if the set doesn't open or standard input or output fails.
pub fn run(args: &McpArgs) -> Outcome {
    let mut shell = Shell::headless();
    if let Some(set) = &args.set {
        shell
            .run(ids::SET_OPEN, json!({ "path": set.to_string_lossy() }))
            .map_err(|error| error.to_string())?;
    }
    let mut agent = Agent::headless(args.autonomous);
    eprintln!(
        "bl-cli mcp: serving the agent API on standard input and output ({}; {})",
        shell.set_name(),
        if args.autonomous {
            "autonomous: destructive and external commands run without asking"
        } else {
            "destructive and external commands are refused; --autonomous allows them"
        }
    );
    bl_agent::stdio::serve(
        &mut agent,
        &mut shell,
        io::stdin().lock(),
        io::stdout().lock(),
    )
    .map_err(|error| format!("the agent API's input or output failed: {error}"))?;
    Ok(ExitCode::SUCCESS)
}

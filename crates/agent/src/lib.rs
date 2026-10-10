//! The agent API (WP-1.15, decision D17): Beatlayer driven by an AI agent,
//! over the Model Context Protocol (MCP), with whatever model the person
//! has.
//!
//! It's built on the command registry rather than beside it: a few tools
//! serve every registered command, so a feature that exists for a person
//! exists for an agent by construction (the architecture's "Agent
//! control").
//!
//! | Piece | What it is |
//! |---|---|
//! | [`Agent`] | The server's state on the main thread: sessions, the MCP methods, the tools, and calls waiting for the person |
//! | [`stdio`] | The stdio transport, which `bl-cli mcp` serves headless |
//! | [`http`] | The Streamable HTTP transport, which the app serves on `127.0.0.1` |
//! | [`client`] | A scripted MCP client over either, for scripts and tests |
//! | [`guide`] | The agent guide, `docs/agent-guide.md`, served as a resource |
//!
//! The tools ([`tools`](crate::tools) lists them): discovery
//! (`list_commands`, `describe_command`), `run` (one command or a batch as
//! one undo step labelled with the agent's name, or a dry run), `query`
//! (by ID or readable path, with summaries), `transport` and `schedule`
//! (commands at a bar and beat, on their exact sample), and `checkpoint`.
//!
//! # Where commands run
//!
//! Every command runs on the shell ([`bl_shell::Shell`]) on the main
//! thread, the same shell and thread the UI's commands run on, so an
//! agent's edit takes the person's path and never touches the audio
//! thread. In the app the HTTP server's threads only parse requests and
//! hand them to the main thread's event loop.
//!
//! # Permissions
//!
//! Read and edit commands run. Destructive and external ones ask the person
//! first (an [`Outcome::Ask`]), unless the person turned on autonomous mode
//! for that agent; headless, they're refused unless `bl-cli mcp` was
//! started with `--autonomous`. Commands that change what agents may do
//! (`agent.*`) are always refused. See [`permission`].
//!
//! # Example
//!
//! ```
//! use bl_agent::Agent;
//! use bl_agent::client::{Client, InProcess};
//! use bl_shell::Shell;
//! use serde_json::json;
//!
//! let mut client = Client::new(InProcess::new(Agent::headless(false), Shell::headless()));
//! client.initialize("doc-test")?;
//! let result = client
//!     .call(
//!         "run",
//!         json!({
//!             "label": "Add a bass track",
//!             "commands": [{
//!                 "command": "track.create",
//!                 "args": { "id": {"$new": "bass"}, "kind": "midi", "name": "Bass", "index": 0 },
//!             }],
//!         }),
//!     )?
//!     .ok()?;
//! assert_eq!(result["undo_step"], "doc-test: Add a bass track");
//! let bass = client.call("query", json!({ "path": "track \"Bass\"" }))?.ok()?;
//! assert_eq!(bass["id"], result["ids"]["bass"]);
//! # Ok::<(), String>(())
//! ```

pub mod client;
pub mod guide;
pub mod http;
pub mod jsonrpc;
pub mod permission;
mod server;
pub mod stdio;
mod tools;

pub use server::{
    Agent, ClientInfo, Outcome, PROTOCOL_VERSIONS, Question, QuestionKind, STDIO_SESSION, Session,
};
pub use tools::EngineMessage;

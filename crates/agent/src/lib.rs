//! The agent API (WP-1.15, decision D17): Beatlayer driven by an AI agent,
//! over the Model Context Protocol (MCP), with whatever model the person
//! has.
//!
//! It's built on the command registry rather than beside it: a few tools
//! serve every registered command, so a feature that exists for a person
//! exists for an agent by construction (the architecture's "Agent
//! control").
//!
//! The protocol is the plug-in kit's: `lspk-agent` speaks JSON-RPC and MCP,
//! serves stdio and Streamable HTTP, checks tokens, hosts, and origins,
//! keeps sessions, asks the person, and holds calls that wait. This crate
//! gives it Beatlayer's tools, guide, settings, and engine.
//!
//! | Piece | What it is |
//! |---|---|
//! | [`Agent`] | The server's state on the main thread: sessions, the MCP methods, the tools, and calls waiting for the person |
//! | [`stdio`] | The stdio transport, which `bl-cli mcp` serves headless |
//! | [`http`] | The Streamable HTTP transport, which the app serves on `127.0.0.1` |
//! | [`client`] | A scripted MCP client over either, for scripts and tests |
//! | [`guide`] | The agent guide, `crates/agent/guide.md`, served as a resource |
//!
//! The tools ([`tools`](crate::tools) lists them): discovery
//! (`list_commands`, `describe_command`), `run` (one command or a batch as
//! one undo step labelled with the agent's name, or a dry run), `query`
//! (by ID or readable path, with summaries), `transport` (play, stop,
//! continue, locate, and the loop), `schedule` (commands at a bar and beat,
//! on their exact sample), `checkpoint`, and perception (WP-1.16):
//! `preview` (a range or a track rendered offline, returned as audio),
//! `analyze` (loudness, peaks, spectrum, tempo, key, and transients as
//! numbers), and `capture_view` (a view as an image, which the app's host
//! draws: see [`Agent::take_capture_requests`]).
//!
//! # Where commands run
//!
//! Every command runs on the shell ([`bl_shell::Shell`]) on the main
//! thread, the same shell and thread the UI's commands run on, so an
//! agent's edit takes the person's path and never touches the audio
//! thread. In the app the HTTP server's threads only parse requests and
//! hand them to the main thread's event loop. The transport's playback
//! commands act in the engine: the agent API hands them to the host as
//! [`EngineMessage`]s, and the host puts them on the engine's queue, as it
//! does the UI's.
//!
//! # Permissions
//!
//! Read and edit commands run. Destructive and external ones ask the person
//! first (an [`Outcome::Ask`]), unless the person turned on autonomous mode
//! for that agent; headless, they're refused unless `bl-cli mcp` was
//! started with `--autonomous`. Commands that change what agents may do
//! (`agent.*`) are always refused. See [`permission`].
//!
//! Since WP-1.15b a call's class is decided from its arguments against the
//! set as it is: moving, resizing, nudging, duplicating, or pasting clips
//! over others, or dropping files on them, deletes or shortens what they
//! cover, so such a call is destructive as well. An agent's rights belong
//! to its own token, which Beatlayer makes when the person allows it: a
//! session carries a [`Credential`], and only one carrying the agent's own
//! token (or the one the person allowed when it first asked) has that
//! agent's approval and autonomous mode, whatever name its client gives.
//!
//! # The person's gestures
//!
//! While the person drags clips or a value, the drag is one undo step in
//! the making. Tool calls that come meanwhile wait ([`Outcome::Wait`]), and
//! so do scheduled commands that fire; once the drag ends,
//! [`Agent::resume`] runs them in the order they came, each its own undo
//! step labelled with its agent's name.
//!
//! # The engine
//!
//! The host attaches the engine it plays the set through with
//! [`Agent::sync_engine`]: agents' transport and scheduled commands go on
//! the queue the person's transport commands take, and the engine's
//! reports and transport come back.
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
pub mod permission;
mod server;
pub mod stdio;
mod tools;

pub use lspk_agent::{
    ClientInfo, Credential, Outcome, PROTOCOL_VERSIONS, Question, QuestionKind, STDIO_SESSION,
    Session, base64, jsonrpc,
};
pub use server::{
    Agent, CaptureRequest, Captured, PreviewOutcome, RenderKind, RenderOutcome, RenderRequest,
};
pub use tools::EngineMessage;

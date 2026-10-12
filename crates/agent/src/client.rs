//! A small MCP client for scripts and tests: it drives the agent API the
//! way an agent client does, over stdio (`bl-cli mcp`), over Streamable
//! HTTP (the app's agent server), or in this process.
//!
//! The agent API is Beatlayer's end-to-end test harness too (the
//! architecture's "Testing through the agent API"): a scripted client calls
//! the same tools an agent does.
//!
//! The client and its stdio and HTTP transports are the plug-in kit's
//! (`lspk_agent::client`); [`InProcess`] runs Beatlayer's agent API in this
//! process.
//!
//! ```no_run
//! use bl_agent::client::{Client, Stdio};
//! use serde_json::json;
//!
//! let mut command = std::process::Command::new("bl-cli");
//! command.arg("mcp");
//! let mut client = Client::new(Stdio::spawn(&mut command)?);
//! client.initialize("my-script")?;
//! let overview = client.call("query", json!({}))?.ok()?;
//! println!("{} tracks", overview["counts"]["tracks"]);
//! # Ok::<(), String>(())
//! ```

use bl_shell::Shell;
use serde_json::Value;

pub use lspk_agent::client::{Client, Http, RawResponse, Stdio, ToolResult, Transport};
use lspk_agent::{Credential, Outcome, Question};

use crate::server::Agent;

/// The agent API in this process, with its own shell: for tests that also
/// drive the shell or an engine between calls. Questions for the person go
/// to `answer`. Its session carries no token unless it's made with
/// [`InProcess::with_credential`], as one over HTTP carries one.
pub struct InProcess {
    /// The agent API.
    pub agent: Agent,
    /// The shell its commands run on.
    pub shell: Shell,
    /// Answers questions for the person: `true` allows.
    pub answer: Box<dyn FnMut(&Question) -> bool>,
    /// The questions asked so far, as the person read them.
    pub asked: Vec<String>,
    session: String,
}

impl std::fmt::Debug for InProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InProcess")
            .field("agent", &self.agent)
            .field("asked", &self.asked)
            .finish_non_exhaustive()
    }
}

impl InProcess {
    /// The agent API with `shell`, declining every question.
    #[must_use]
    pub fn new(agent: Agent, shell: Shell) -> Self {
        Self::with_credential(agent, shell, Credential::Local)
    }

    /// The agent API with `shell`, its session's requests carrying
    /// `credential`, as an HTTP client's carry a token, declining every
    /// question.
    #[must_use]
    pub fn with_credential(mut agent: Agent, shell: Shell, credential: Credential) -> Self {
        let session = agent.open_session_as(None, credential);
        Self {
            agent,
            shell,
            answer: Box::new(|_| false),
            asked: Vec::new(),
            session,
        }
    }
}

impl Transport for InProcess {
    fn send(&mut self, message: &Value) -> Result<Option<Value>, String> {
        match self
            .agent
            .handle(&mut self.shell, &self.session, message.clone())
        {
            Outcome::Reply(reply) => Ok(reply),
            Outcome::Ask(question) => {
                self.asked.push(question.text());
                let allow = (self.answer)(&question);
                Ok(self.agent.resolve(&mut self.shell, question.id, allow))
            }
            // One thread can't wait for a gesture it would have to end
            // itself: tests of waiting use `Agent::handle` and
            // `Agent::resume`, or the HTTP server.
            Outcome::Wait(_) => Err(
                "the call waits for the person's gesture to end, which this client can't wait \
                 for"
                .to_owned(),
            ),
        }
    }
}

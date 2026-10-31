//! The Streamable HTTP transport, which the running app serves on
//! `127.0.0.1` while the person has the agent server switched on.
//!
//! It's the plug-in kit's (`lspk_agent::http`): MCP's Streamable HTTP over
//! a small HTTP/1.1 server, which needs no async runtime: one thread
//! accepts connections and one thread serves each, up to
//! [`MAX_CONNECTIONS`]. Every request is checked before anything else sees
//! it:
//!
//! - It must name `127.0.0.1`, `localhost`, or `[::1]` with the server's
//!   port as its host, so a web page that rebinds a domain to this machine
//!   is refused (DNS-rebinding protection).
//! - It must carry no `Origin` header: browsers send one, agent clients
//!   don't, so requests from web pages are refused.
//! - It must carry a token as a bearer token: an allowed agent's own token,
//!   which makes its requests that agent's, or the server's token, with
//!   which a client is asked about each time it connects (WP-1.15b). Tokens
//!   are compared in constant time, every one of them for every request.
//!
//! A request that passes goes to the main thread as a [`Job`], through the
//! same event loop as the UI's events, so commands run where the person's
//! do, with the credential it carried; every request on a session must
//! carry the credential the session started with. A tool call that comes
//! while the person's gesture is under way waits ([`Served::Parked`]) and
//! is answered after the drop ([`resume`]). The connection's thread waits
//! for the answer, however long the person takes to confirm. The server
//! answers each POST with one JSON response (MCP allows that instead of an
//! event stream), refuses GET with 405 because it starts no streams of its
//! own, and ends a session on DELETE.
//!
//! This module gives the kit's server Beatlayer's endpoint (`/mcp`, and the
//! Settings window's Agents page as where the token is) and agent
//! settings, and the agent API's [`Agent`].

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bl_shell::Shell;
use bl_shell::agent::{AgentSettings, ENDPOINT_PATH};
use lspk_agent::http::{self as kit, Endpoint};

pub use lspk_agent::http::{
    HttpConfig, IDLE_TIMEOUT, Job, MAX_BODY, MAX_CONNECTIONS, MAX_HEAD, Parked,
};

use lspk_agent::Question;

use crate::server::Agent;

/// What [`serve`] did with a job.
#[derive(Debug)]
pub enum Served {
    /// It's answered.
    Done,
    /// It waits for the person: show [`Pending::question`], then call
    /// [`Pending::finish`] with the answer.
    Waiting(Pending),
    /// It waits for the person's gesture (a drag) to end: keep it, and
    /// once the gesture has ended, [`resume`] answers it.
    Parked(Parked),
}

impl From<kit::Served> for Served {
    fn from(served: kit::Served) -> Self {
        match served {
            kit::Served::Done => Self::Done,
            kit::Served::Waiting(pending) => Self::Waiting(Pending(pending)),
            kit::Served::Parked(parked) => Self::Parked(parked),
        }
    }
}

/// A job waiting for the person's answer.
#[derive(Debug)]
pub struct Pending(kit::Pending);

impl Pending {
    /// What the person is asked.
    #[must_use]
    pub fn question(&self) -> &Question {
        self.0.question()
    }

    /// Answers the question and sends the reply to the waiting connection,
    /// or keeps an allowed call that waits for the host: [`Served::Parked`].
    pub fn finish(self, agent: &mut Agent, shell: &mut Shell, allow: bool) -> Served {
        self.0.finish(agent.server(), shell, allow).into()
    }
}

/// Once the person's gesture has ended, answers the next call that waited
/// for it, from `parked`: [`Served::Done`] if it's answered, or
/// [`Served::Waiting`] if it asks the person first. Call it again until it
/// gives `None`, which it does while the gesture goes on or nothing waits.
pub fn resume(agent: &mut Agent, shell: &mut Shell, parked: &mut Vec<Parked>) -> Option<Served> {
    kit::resume(agent.server(), shell, parked).map(Served::from)
}

/// Runs one job on the main thread: hands its message to the agent API and
/// sends the reply, or returns it to wait for the person.
pub fn serve(agent: &mut Agent, shell: &mut Shell, job: Job) -> Served {
    kit::serve(agent.server(), shell, job).into()
}

/// The agent server: listening on `127.0.0.1`, handing requests to the
/// main thread as [`Job`]s. Dropping it stops it.
#[derive(Debug)]
pub struct HttpServer(kit::HttpServer);

impl HttpServer {
    /// Starts listening. `wake` is called, on a connection's thread, each
    /// time a job is ready, so the main thread's event loop can take it.
    ///
    /// # Errors
    ///
    /// If the port can't be bound, such as when another program has it, or
    /// the token is empty.
    pub fn start(config: &HttpConfig, wake: Arc<dyn Fn() + Send + Sync>) -> io::Result<Self> {
        let endpoint = Endpoint {
            path: ENDPOINT_PATH.to_owned(),
            realm: "beatlayer".to_owned(),
            token_location: "Settings, Agents".to_owned(),
        };
        kit::HttpServer::start(config, endpoint, wake).map(Self)
    }

    /// Gives the server the allowed agents' own tokens, so a request that
    /// carries one is that agent's (WP-1.15b). The host calls it whenever
    /// the settings may have changed: an agent allowed or forgotten, or its
    /// token replaced.
    pub fn set_agent_tokens(&self, settings: &AgentSettings) {
        self.0.set_agent_tokens(
            settings
                .clients
                .iter()
                // An agent without a token yet has no rights of its own.
                .filter(|client| !client.token.is_empty())
                .map(|client| (client.name.clone(), client.token.expose().to_owned())),
        );
    }

    /// The address it listens on.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.0.local_addr()
    }

    /// The address agents connect to.
    #[must_use]
    pub fn url(&self) -> String {
        self.0.url()
    }

    /// The next job waiting for the main thread, if there is one.
    #[must_use]
    pub fn try_job(&self) -> Option<Job> {
        self.0.try_job()
    }

    /// Waits up to `timeout` for the next job, for tests and headless hosts
    /// that have no event loop to wake.
    #[must_use]
    pub fn wait_job(&self, timeout: Duration) -> Option<Job> {
        self.0.wait_job(timeout)
    }

    /// How many connections are open.
    #[must_use]
    pub fn connections(&self) -> usize {
        self.0.connections()
    }

    /// Stops listening and closes every connection. Requests waiting for
    /// the main thread get 503.
    pub fn stop(self) {
        self.0.stop();
    }
}

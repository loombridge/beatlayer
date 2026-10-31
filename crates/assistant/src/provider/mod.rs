//! The model providers: one trait, three APIs behind it.
//!
//! The assistant talks to a model through a [`Provider`]: it sends one
//! [`Request`] (the conversation and the tools) and receives the turn's
//! [`Event`]s as they stream, ending with a [`Reply`].
//!
//! | Provider | API | Also serves |
//! |---|---|---|
//! | [`OpenAi`] | Chat Completions, streamed | OpenAI itself, OpenRouter, and any local server that speaks the same shape: Ollama, LM Studio, llama.cpp, vLLM |
//! | [`Anthropic`] | Messages, streamed | Claude models |
//! | [`Gemini`] | `streamGenerateContent`, streamed | Google's models |
//!
//! Every provider streams Server-Sent Events ([`crate::sse`]) and turns its
//! events into the same [`Event`]s, including tool calls, which arrive in
//! pieces in two of the three APIs and are assembled here before the caller
//! sees them.

mod anthropic;
mod gemini;
mod openai;

use std::fmt;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

pub use anthropic::Anthropic;
pub use gemini::Gemini;
pub use openai::OpenAi;

use serde_json::Value;

use crate::keychain::{Keychain, KeychainError};
use crate::message::{Event, Reply, Request};

/// A model API streamed one turn at a time.
pub trait Provider: Send + Sync {
    /// Streams one turn: `on_event` sees each text delta and each finished
    /// tool call as it arrives, and the returned [`Reply`] holds the whole
    /// turn.
    ///
    /// # Errors
    ///
    /// If the keychain, the network, the provider's API, or its stream
    /// failed ([`Error`]).
    fn stream(&self, request: &Request, on_event: &mut dyn FnMut(Event)) -> Result<Reply, Error>;
}

/// Why a turn didn't finish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The API key couldn't be read.
    Keychain(KeychainError),
    /// No key is stored for the provider, so nothing was sent.
    MissingKey {
        /// The provider's name, for the message.
        provider: &'static str,
        /// The keychain account it looked under.
        account: String,
    },
    /// The request didn't reach the provider, or the connection failed
    /// part-way.
    Transport(String),
    /// The provider answered with an error: an HTTP status and the API's
    /// own message, or an error it reported inside the stream.
    Api {
        /// The HTTP status, when the error was the response's.
        status: Option<u16>,
        /// What the provider said, without its JSON wrapper.
        message: String,
    },
    /// The stream didn't follow the provider's protocol: malformed JSON or a
    /// missing terminator. A tool call whose arguments aren't JSON is not
    /// one: it is kept ([`ToolCall::unparsed`](crate::tools::ToolCall::unparsed))
    /// and the agent loop tells the model.
    Protocol(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Keychain(error) => write!(f, "{error}"),
            Self::MissingKey { provider, account } => write!(
                f,
                "no {provider} API key is stored in the keychain (account `{account}`)"
            ),
            Self::Transport(why) => write!(f, "the request didn't reach the provider: {why}"),
            Self::Api {
                status: Some(status),
                message,
            } => write!(f, "the provider answered {status}: {message}"),
            Self::Api {
                status: None,
                message,
            } => write!(f, "the provider reported an error: {message}"),
            Self::Protocol(why) => write!(f, "the provider's stream broke: {why}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<KeychainError> for Error {
    fn from(error: KeychainError) -> Self {
        Self::Keychain(error)
    }
}

/// The key stored for `account`, or the error for a provider that has none.
pub(crate) fn stored_key(
    keychain: &dyn Keychain,
    provider: &'static str,
    account: &str,
) -> Result<String, Error> {
    match keychain.read(account)? {
        Some(key) => Ok(key),
        None => Err(Error::MissingKey {
            provider,
            account: account.to_owned(),
        }),
    }
}

/// Sends `body` as one request's JSON with `headers`, and returns the
/// response's streamed body. A response that isn't a success becomes
/// [`Error::Api`] with the API's own message.
pub(crate) fn post(url: &str, headers: &[(&str, &str)], body: &Value) -> Result<ureq::Body, Error> {
    let body = serde_json::to_string(body).map_err(|error| Error::Protocol(error.to_string()))?;
    let mut request = agent().post(url);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = request
        .header("content-type", "application/json")
        .send(body)
        .map_err(|error| Error::Transport(error.to_string()))?;
    let status = response.status().as_u16();
    if (200..300).contains(&status) {
        return Ok(response.into_body());
    }
    let mut response = response;
    let body = response.body_mut().read_to_string().unwrap_or_default();
    Err(api_error(status, &body))
}

/// One HTTP agent for every provider: a failed status is the response's own
/// (its body holds the API's message), certificates verify through the
/// platform's store, and rustls' crypto provider is ring. One shared agent
/// pools the providers' connections; the stem separation packages' download
/// (IMP-11) uses it too.
pub fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            // A dead or silent server fails the turn in half a minute
            // rather than hanging it; the streamed body itself has no
            // deadline, since a model may think for minutes.
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .root_certs(platform_roots())
                    .unversioned_rustls_crypto_provider(Arc::new(
                        rustls::crypto::ring::default_provider(),
                    ))
                    .build(),
            )
            .build()
            .new_agent()
    })
}

/// The platform's own certificate store as rustls' trust roots, so a
/// certificate a person added to their machine is trusted here too.
fn platform_roots() -> ureq::tls::RootCerts {
    let loaded = rustls_native_certs::load_native_certs();
    let certs: Vec<ureq::tls::Certificate<'static>> = loaded
        .certs
        .iter()
        .map(|cert| ureq::tls::Certificate::from_der(cert.as_ref()).to_owned())
        .collect();
    ureq::tls::RootCerts::new_with_certs(&certs)
}

/// The error a provider reported with an HTTP status.
pub(crate) fn api_error(status: u16, body: &str) -> Error {
    Error::Api {
        status: Some(status),
        message: api_message(body),
    }
}

/// The error a provider reported inside its stream.
pub(crate) fn stream_error(body: &str) -> Error {
    Error::Api {
        status: None,
        message: api_message(body),
    }
}

/// The API's own message from an error body, each of the three shaped
/// `{"error": {"message": …}}`, or the body itself when it isn't.
fn api_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| body.trim().to_owned())
}

/// The event's JSON, or a [`Error::Protocol`] naming the malformed stream.
pub(crate) fn json_event(data: &str) -> Result<Value, Error> {
    serde_json::from_str(data)
        .map_err(|error| Error::Protocol(format!("an event isn't JSON: {error}")))
}

/// Reads an SSE body's events as their `data`, mapping a read that fails to
/// [`Error::Transport`].
pub(crate) fn events(body: &mut ureq::Body) -> impl Iterator<Item = Result<String, Error>> + '_ {
    crate::sse::events(body.as_reader())
        .map(|data| data.map_err(|error| Error::Transport(error.to_string())))
}

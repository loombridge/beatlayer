//! The built-in assistant: model providers over the command registry, keys
//! in the OS keychain, and the agent loop that runs a turn (WP-2.23, parts 1
//! and 2).
//!
//! The assistant gives people without an agent client the same way into the
//! commands the agent API serves: a few tools that list, describe and run
//! any registry command and read the project ([`tools`]). A provider
//! ([`provider`]) streams one turn of the conversation ([`message`]) against
//! one model API, and the agent loop ([`agent`]) runs each `run` call as the
//! command it names, with the same permission classes the agent API
//! enforces, as one undo step per turn.
//!
//! Three APIs, one [`provider::Provider`] trait:
//!
//! - [`provider::OpenAi`]: OpenAI's Chat Completions, and every server that
//!   speaks its shape (OpenRouter, Ollama, LM Studio, llama.cpp, vLLM);
//! - [`provider::Anthropic`]: Claude's Messages API;
//! - [`provider::Gemini`]: Google's `streamGenerateContent`.
//!
//! API keys live in the OS keychain and nowhere else ([`keychain`]): never in
//! settings or a project file. Nothing leaves the machine unless a person
//! configures a cloud provider.

pub mod agent;
pub mod keychain;
pub mod message;
pub mod provider;
pub mod tools;

mod schema;
mod sse;

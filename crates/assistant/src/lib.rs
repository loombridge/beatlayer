//! The built-in assistant: model providers over the command registry, and
//! keys in the OS keychain (WP-2.23, part 1: providers and keys).
//!
//! The assistant gives people without an agent client the same tools the
//! agent API serves: every command in [`bl_commands`] is a tool the model may
//! call ([`tools`]). A provider ([`provider`]) streams one turn of the
//! conversation ([`message`]) against one model API, and the agent loop built
//! on it runs each tool call as the command it names.
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

pub mod keychain;
pub mod message;
pub mod provider;
pub mod tools;

mod schema;
mod sse;

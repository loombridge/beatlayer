//! OpenAI's Chat Completions API, streamed.
//!
//! The same shape serves OpenAI itself, OpenRouter, and local servers such
//! as Ollama, LM Studio, llama.cpp, and vLLM: [`OpenAi::base_url`] points
//! the provider at any of them. With no key stored the request carries no
//! `Authorization` header, so a local server needs no key. This doesn't
//! reach the Responses API: the assistant's tools and conversation map onto
//! Chat Completions' `messages`, `tools`, and `stream`.

use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::keychain::Keychain;
use crate::message::{Event, Part, Reply, Request, Role, Stop};
use crate::provider::{self, Error, Provider};
use crate::tools::{ToolCall, ToolSpec};

/// Where OpenAI's own API is. Other servers speak to [`OpenAi::base_url`].
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// One model API over the OpenAI-compatible Chat Completions shape.
pub struct OpenAi {
    keychain: Arc<dyn Keychain>,
    base_url: String,
}

impl OpenAi {
    /// The keychain account the key is stored under, for OpenAI itself and
    /// for any other server reachable through [`OpenAi::base_url`].
    pub const ACCOUNT: &'static str = "openai";

    /// The provider against OpenAI itself, reading its key from `keychain`.
    #[must_use]
    pub fn new(keychain: Arc<dyn Keychain>) -> Self {
        Self {
            keychain,
            base_url: DEFAULT_BASE_URL.to_owned(),
        }
    }

    /// The provider against another OpenAI-compatible server, such as
    /// OpenRouter (`https://openrouter.ai/api/v1`) or a local server.
    #[must_use]
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    fn url(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }
}

impl Provider for OpenAi {
    fn stream(&self, request: &Request, on_event: &mut dyn FnMut(Event)) -> Result<Reply, Error> {
        let mut body = json!({
            "model": request.model,
            "messages": messages(request),
            "stream": true,
            "max_tokens": request.max_tokens,
        });
        if !request.tools.is_empty() {
            body["tools"] = tools(request);
        }
        let authorization = self
            .keychain
            .read(Self::ACCOUNT)?
            .map(|key| format!("Bearer {key}"));
        let mut headers = Vec::new();
        if let Some(authorization) = &authorization {
            headers.push(("authorization", authorization.as_str()));
        }
        let mut response = provider::post(&self.url(), &headers, &body)?;

        let mut reply = Reply {
            text: String::new(),
            tool_calls: Vec::new(),
            stop: Stop::End,
        };
        let mut calls: Vec<Partial> = Vec::new();
        for event in provider::events(&mut response) {
            let data = event?;
            if data.trim() == "[DONE]" {
                break;
            }
            let chunk = provider::json_event(&data)?;
            if chunk.get("error").is_some_and(|error| !error.is_null()) {
                return Err(provider::stream_error(&data));
            }
            let Some(choice) = chunk["choices"]
                .as_array()
                .and_then(|choices| choices.first())
            else {
                continue;
            };
            if let Some(text) = choice["delta"]["content"].as_str()
                && !text.is_empty()
            {
                reply.text.push_str(text);
                on_event(Event::Text(text.to_owned()));
            }
            for call in choice["delta"]["tool_calls"]
                .as_array()
                .into_iter()
                .flatten()
            {
                let index = call["index"].as_u64().unwrap_or(0) as usize;
                if index >= calls.len() {
                    calls.resize_with(index + 1, Partial::default);
                }
                let partial = &mut calls[index];
                if let Some(id) = call["id"].as_str() {
                    partial.id = id.to_owned();
                }
                if let Some(name) = call["function"]["name"].as_str() {
                    partial.name.push_str(name);
                }
                if let Some(arguments) = call["function"]["arguments"].as_str() {
                    partial.arguments.push_str(arguments);
                }
            }
            if let Some(reason) = choice["finish_reason"].as_str() {
                reply.stop = stop(reason);
            }
        }
        for partial in calls {
            let call = partial.finish(reply.tool_calls.len())?;
            on_event(Event::ToolCall(call.clone()));
            reply.tool_calls.push(call);
        }
        Ok(reply)
    }
}

/// One tool call as the stream carries it: its pieces appended as they
/// arrive, finished at the end of the stream.
#[derive(Default)]
struct Partial {
    id: String,
    name: String,
    arguments: String,
}

impl Partial {
    /// The finished call. `count` is how many calls already finished, for a
    /// server that gives a call no ID of its own.
    fn finish(self, count: usize) -> Result<ToolCall, Error> {
        let arguments = if self.arguments.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&self.arguments).map_err(|error| {
                Error::Protocol(format!(
                    "the arguments of `{}` aren't JSON: {error}",
                    self.name
                ))
            })?
        };
        Ok(ToolCall {
            id: if self.id.is_empty() {
                format!("call_{}", count + 1)
            } else {
                self.id
            },
            name: self.name,
            arguments,
        })
    }
}

/// Why the model stopped, in OpenAI's words.
fn stop(reason: &str) -> Stop {
    match reason {
        "stop" | "tool_calls" => Stop::End,
        "length" => Stop::Length,
        other => Stop::Other(other.to_owned()),
    }
}

/// The conversation as Chat Completions `messages`: one system message, then
/// one message per turn, each tool result as its own `tool` message.
fn messages(request: &Request) -> Vec<Value> {
    let mut messages = Vec::new();
    if !request.system.is_empty() {
        messages.push(json!({ "role": "system", "content": request.system }));
    }
    for message in &request.messages {
        match message.role {
            Role::User => {
                let text = message.text();
                if !text.is_empty() {
                    messages.push(json!({ "role": "user", "content": text }));
                }
                for part in &message.parts {
                    if let Part::Result(result) = part {
                        messages.push(json!({
                            "role": "tool",
                            "tool_call_id": result.id,
                            "content": result.content,
                        }));
                    }
                }
            }
            Role::Assistant => {
                let text = message.text();
                let calls: Vec<Value> = message
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        Part::Call(call) => Some(json!({
                            "id": call.id,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                "arguments": call.arguments.to_string(),
                            },
                        })),
                        _ => None,
                    })
                    .collect();
                let mut assistant = Map::new();
                assistant.insert("role".into(), json!("assistant"));
                assistant.insert(
                    "content".into(),
                    if text.is_empty() {
                        Value::Null
                    } else {
                        json!(text)
                    },
                );
                if !calls.is_empty() {
                    assistant.insert("tool_calls".into(), Value::Array(calls));
                }
                messages.push(Value::Object(assistant));
            }
        }
    }
    messages
}

/// The tools as Chat Completions `functions`, each from its command's
/// schema.
fn tools(request: &Request) -> Value {
    Value::Array(
        request
            .tools
            .iter()
            .map(|tool: &ToolSpec| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.input,
                    },
                })
            })
            .collect(),
    )
}

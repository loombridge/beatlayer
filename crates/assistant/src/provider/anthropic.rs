//! Anthropic's Messages API, streamed.
//!
//! Claude's turns arrive as content blocks: a text block streams
//! `text_delta`s, and a tool call opens as a `tool_use` block whose
//! arguments arrive as `input_json_delta` pieces, which the provider
//! assembles into one call per block. A missing key is an error the person
//! sees: Anthropic has no local deployment.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::keychain::Keychain;
use crate::message::{Event, Part, Reply, Request, Role, Stop, Usage};
use crate::provider::{self, Error, Provider};
use crate::tools::{ToolCall, ToolSpec};

/// Where Anthropic's API is.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com/v1";

/// The Messages API version this speaks.
pub const API_VERSION: &str = "2023-06-01";

/// One model API over Anthropic's Messages shape.
pub struct Anthropic {
    keychain: Arc<dyn Keychain>,
    base_url: String,
}

impl Anthropic {
    /// The keychain account the key is stored under.
    pub const ACCOUNT: &'static str = "anthropic";

    /// The provider against Anthropic's API, reading its key from
    /// `keychain`.
    #[must_use]
    pub fn new(keychain: Arc<dyn Keychain>) -> Self {
        Self {
            keychain,
            base_url: DEFAULT_BASE_URL.to_owned(),
        }
    }

    /// The provider against another base URL, such as a proxy.
    #[must_use]
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    fn url(&self) -> String {
        format!("{}/messages", self.base_url.trim_end_matches('/'))
    }
}

impl Provider for Anthropic {
    fn stream(&self, request: &Request, on_event: &mut dyn FnMut(Event)) -> Result<Reply, Error> {
        let key = provider::stored_key(self.keychain.as_ref(), "Anthropic", Self::ACCOUNT)?;
        let mut body = json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "messages": messages(request),
            "stream": true,
        });
        if !request.system.is_empty() {
            body["system"] = json!(request.system);
        }
        if !request.tools.is_empty() {
            body["tools"] = tools(request);
        }
        let mut response = provider::post(
            &self.url(),
            &[("x-api-key", &key), ("anthropic-version", API_VERSION)],
            &body,
        )?;

        let mut reply = Reply {
            text: String::new(),
            tool_calls: Vec::new(),
            stop: Stop::End,
        };
        let mut blocks: Vec<Block> = Vec::new();
        let mut usage: Option<Usage> = None;
        for event in provider::events(&mut response) {
            let data = event?;
            let value = provider::json_event(&data)?;
            match value["type"].as_str().unwrap_or_default() {
                "content_block_start" => {
                    let index = index(&value);
                    if index >= blocks.len() {
                        blocks.resize_with(index + 1, || Block::Text);
                    }
                    blocks[index] = started(&value, &mut reply, on_event);
                }
                "content_block_delta" => {
                    let delta = &value["delta"];
                    match delta["type"].as_str().unwrap_or_default() {
                        "text_delta" => {
                            if let Some(text) = delta["text"].as_str()
                                && !text.is_empty()
                            {
                                reply.text.push_str(text);
                                on_event(Event::Text(text.to_owned()));
                            }
                        }
                        "input_json_delta" => {
                            if let Some(Block::Tool { arguments, .. }) =
                                blocks.get_mut(index(&value))
                                && let Some(piece) = delta["partial_json"].as_str()
                            {
                                arguments.push_str(piece);
                            }
                        }
                        _ => {}
                    }
                }
                "content_block_stop" => {
                    if let Some(Block::Tool {
                        id,
                        name,
                        input,
                        arguments,
                    }) = blocks.get_mut(index(&value))
                    {
                        let call = finished(id, name, input, arguments)?;
                        on_event(Event::ToolCall(call.clone()));
                        reply.tool_calls.push(call);
                    }
                }
                "message_start" => {
                    if let Some(tokens) = value["message"]["usage"]["input_tokens"].as_u64() {
                        usage.get_or_insert_default().input = tokens;
                    }
                }
                "message_delta" => {
                    if let Some(tokens) = value["usage"]["output_tokens"].as_u64() {
                        usage.get_or_insert_default().output = tokens;
                    }
                    if let Some(reason) = value["delta"]["stop_reason"].as_str() {
                        reply.stop = stop(reason);
                    }
                }
                "message_stop" => {
                    if let Some(usage) = usage {
                        on_event(Event::Usage(usage));
                    }
                    break;
                }
                "error" => return Err(provider::stream_error(&data)),
                _ => {}
            }
        }
        Ok(reply)
    }
}

/// One content block of the turn as it streams.
enum Block {
    /// A text block, whose text goes straight into the reply.
    Text,
    /// A tool call: its ID and name from the block's opening, its arguments
    /// assembled from the deltas after it.
    Tool {
        id: String,
        name: String,
        input: Value,
        arguments: String,
    },
}

/// The block's index in the turn.
fn index(value: &Value) -> usize {
    value["index"].as_u64().unwrap_or(0) as usize
}

/// The block a `content_block_start` opens.
fn started(value: &Value, reply: &mut Reply, on_event: &mut dyn FnMut(Event)) -> Block {
    let block = &value["content_block"];
    if block["type"].as_str() == Some("tool_use") {
        return Block::Tool {
            id: block["id"].as_str().unwrap_or_default().to_owned(),
            name: block["name"].as_str().unwrap_or_default().to_owned(),
            input: block["input"].clone(),
            arguments: String::new(),
        };
    }
    // A text block may open with its first text already in place.
    if let Some(text) = block["text"].as_str()
        && !text.is_empty()
    {
        reply.text.push_str(text);
        on_event(Event::Text(text.to_owned()));
    }
    Block::Text
}

/// The call a `content_block_stop` finishes: its deltas' JSON when it
/// streamed some, and its opening input when it didn't.
fn finished(id: &str, name: &str, input: &Value, arguments: &str) -> Result<ToolCall, Error> {
    let arguments = if arguments.trim().is_empty() {
        input.clone()
    } else {
        serde_json::from_str(arguments).map_err(|error| {
            Error::Protocol(format!("the arguments of `{name}` aren't JSON: {error}"))
        })?
    };
    Ok(ToolCall {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments,
    })
}

/// Why the model stopped, in Anthropic's words.
fn stop(reason: &str) -> Stop {
    match reason {
        "end_turn" | "stop_sequence" | "tool_use" => Stop::End,
        "max_tokens" => Stop::Length,
        other => Stop::Other(other.to_owned()),
    }
}

/// The conversation as Messages `messages`, each turn's parts as its content
/// blocks.
fn messages(request: &Request) -> Vec<Value> {
    request
        .messages
        .iter()
        .map(|message| {
            let content: Vec<Value> = message
                .parts
                .iter()
                .filter_map(|part| match part {
                    Part::Text(text) if text.is_empty() => None,
                    Part::Text(text) => Some(json!({ "type": "text", "text": text })),
                    Part::Call(call) => Some(json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": call.arguments,
                    })),
                    Part::Result(result) => Some(json!({
                        "type": "tool_result",
                        "tool_use_id": result.id,
                        "content": result.content,
                    })),
                })
                .collect();
            json!({
                "role": match message.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                },
                "content": content,
            })
        })
        .collect()
}

/// The tools as Messages `tools`, each from its command's schema.
fn tools(request: &Request) -> Value {
    Value::Array(
        request
            .tools
            .iter()
            .map(|tool: &ToolSpec| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.input,
                })
            })
            .collect(),
    )
}

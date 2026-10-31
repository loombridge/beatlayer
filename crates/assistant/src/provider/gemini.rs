//! Google's Gemini API, streamed.
//!
//! `models/{model}:streamGenerateContent?alt=sse` sends each piece of the
//! reply as a candidate: text parts, and function calls, which carry no ID
//! of their own, so the provider makes one. A missing key is an error the
//! person sees: Gemini has no local deployment.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::keychain::Keychain;
use crate::message::{Event, Part, Reply, Request, Role, Stop, Usage};
use crate::provider::{self, Error, Provider};
use crate::tools::{ToolCall, ToolSpec};

/// Where Google's API is.
pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

/// One model API over Gemini's `streamGenerateContent`.
pub struct Gemini {
    keychain: Arc<dyn Keychain>,
    base_url: String,
}

impl Gemini {
    /// The keychain account the key is stored under.
    pub const ACCOUNT: &'static str = "gemini";

    /// The provider against Google's API, reading its key from `keychain`.
    #[must_use]
    pub fn new(keychain: Arc<dyn Keychain>) -> Self {
        Self {
            keychain,
            base_url: DEFAULT_BASE_URL.to_owned(),
        }
    }

    /// The provider against another base URL.
    #[must_use]
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    fn url(&self, model: &str) -> String {
        // A model named as the API's documentation names it (`models/…`)
        // still makes one `models/` in the URL.
        let model = model.strip_prefix("models/").unwrap_or(model);
        format!(
            "{}/models/{model}:streamGenerateContent?alt=sse",
            self.base_url.trim_end_matches('/')
        )
    }
}

impl Provider for Gemini {
    fn stream(&self, request: &Request, on_event: &mut dyn FnMut(Event)) -> Result<Reply, Error> {
        let key = provider::stored_key(self.keychain.as_ref(), "Gemini", Self::ACCOUNT)?;
        let mut body = json!({ "contents": contents(request) });
        if !request.system.is_empty() {
            body["systemInstruction"] = json!({ "parts": [{ "text": request.system }] });
        }
        if !request.tools.is_empty() {
            body["tools"] = tools(request);
        }
        body["generationConfig"] = json!({ "maxOutputTokens": request.max_tokens });
        let mut response = provider::post(
            &self.url(&request.model),
            &[("x-goog-api-key", &key)],
            &body,
        )?;

        let mut reply = Reply {
            text: String::new(),
            tool_calls: Vec::new(),
            stop: Stop::End,
        };
        let mut usage = None;
        for event in provider::events(&mut response) {
            let data = event?;
            let value = provider::json_event(&data)?;
            if let Some(counts) = value
                .get("usageMetadata")
                .filter(|counts| counts.is_object())
            {
                usage = Some(Usage {
                    input: counts["promptTokenCount"].as_u64().unwrap_or_default(),
                    output: counts["candidatesTokenCount"].as_u64().unwrap_or_default(),
                });
            }
            if value.get("error").is_some_and(|error| !error.is_null()) {
                return Err(provider::stream_error(&data));
            }
            if let Some(reason) = value["candidates"][0]["finishReason"].as_str() {
                reply.stop = stop(reason);
            }
            for part in value["candidates"][0]["content"]["parts"]
                .as_array()
                .into_iter()
                .flatten()
            {
                if let Some(text) = part["text"].as_str()
                    && !text.is_empty()
                {
                    reply.text.push_str(text);
                    on_event(Event::Text(text.to_owned()));
                }
                if let Some(function) = part.get("functionCall") {
                    let call = call(function, reply.tool_calls.len() + 1);
                    on_event(Event::ToolCall(call.clone()));
                    reply.tool_calls.push(call);
                }
            }
        }
        if let Some(usage) = usage {
            on_event(Event::Usage(usage));
        }
        Ok(reply)
    }
}

/// The call a `functionCall` part makes. Gemini's calls have no ID, so
/// `count` names this one.
fn call(function: &Value, count: usize) -> ToolCall {
    let id = format!("call_{count}");
    let name = function["name"].as_str().unwrap_or_default().to_owned();
    let arguments = match &function["args"] {
        Value::String(raw) => match serde_json::from_str(raw) {
            Ok(arguments) => arguments,
            Err(error) => return ToolCall::unparsed(id, name, raw, &error.to_string()),
        },
        Value::Null => json!({}),
        arguments => arguments.clone(),
    };
    ToolCall {
        id,
        name,
        arguments,
    }
}

/// Why the model stopped, in Gemini's words.
fn stop(reason: &str) -> Stop {
    match reason {
        "STOP" | "FINISH_REASON_UNSPECIFIED" => Stop::End,
        "MAX_TOKENS" => Stop::Length,
        other => Stop::Other(other.to_owned()),
    }
}

/// The conversation as `contents`: Gemini's turns alternate, so consecutive
/// messages of one role share one content.
fn contents(request: &Request) -> Vec<Value> {
    let mut contents: Vec<Value> = Vec::new();
    for message in &request.messages {
        let role = match message.role {
            Role::User => "user",
            Role::Assistant => "model",
        };
        let parts: Vec<Value> = message
            .parts
            .iter()
            .filter_map(|part| match part {
                Part::Text(text) if text.is_empty() => None,
                Part::Text(text) => Some(json!({ "text": text })),
                Part::Call(call) => {
                    Some(json!({ "functionCall": { "name": call.name, "args": call.arguments } }))
                }
                Part::Result(result) => Some(json!({
                    "functionResponse": {
                        "name": result.name,
                        "response": { "result": result.content },
                    },
                })),
            })
            .collect();
        if parts.is_empty() {
            continue;
        }
        if let Some(last) = contents.last_mut()
            && last["role"].as_str() == Some(role)
            && let Some(all) = last["parts"].as_array_mut()
        {
            all.extend(parts);
        } else {
            contents.push(json!({ "role": role, "parts": parts }));
        }
    }
    contents
}

/// The tools as `functionDeclarations`, each from its command's schema,
/// converted to the subset Gemini's API takes
/// ([`crate::schema::for_gemini`]).
fn tools(request: &Request) -> Value {
    let declarations: Vec<Value> = request
        .tools
        .iter()
        .map(|tool: &ToolSpec| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "parameters": crate::schema::for_gemini(&tool.input),
            })
        })
        .collect();
    json!([{ "functionDeclarations": declarations }])
}

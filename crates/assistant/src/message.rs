//! The conversation, provider-neutral: what the assistant sends and what
//! comes back, before any provider's wire format.
//!
//! A [`Message`] is one turn of the conversation, in [`Part`]s: text, the
//! tool calls the model made, and their results. Each provider turns the
//! same conversation into its own format ([`crate::provider`]).

use serde::{Deserialize, Serialize};

use crate::tools::ToolCall;

/// Who a message is from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    /// The person, or the results of the model's tool calls.
    User,
    /// The model.
    Assistant,
}

/// One message of the conversation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Who it's from.
    pub role: Role,
    /// What it holds, in order.
    pub parts: Vec<Part>,
}

/// One part of a message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Part {
    /// Text.
    Text(String),
    /// A tool call the model made (in an assistant message).
    Call(ToolCall),
    /// The result of a tool call (in a user message).
    Result(ToolResult),
}

/// What running a tool call returned, ready for the model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    /// The ID of the call it answers.
    pub id: String,
    /// The tool's name, which Gemini's format needs (its calls have no ID).
    pub name: String,
    /// What the call returned, as text.
    pub content: String,
}

impl Message {
    /// A message of plain text from `role`.
    #[must_use]
    pub fn new(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            parts: vec![Part::Text(text.into())],
        }
    }

    /// A message from the person.
    #[must_use]
    pub fn user(text: impl Into<String>) -> Self {
        Self::new(Role::User, text)
    }

    /// A message from the model.
    #[must_use]
    pub fn assistant(text: impl Into<String>) -> Self {
        Self::new(Role::Assistant, text)
    }

    /// The model's tool calls, as one assistant message holds them.
    #[must_use]
    pub fn calls(calls: impl IntoIterator<Item = ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            parts: calls.into_iter().map(Part::Call).collect(),
        }
    }

    /// The results of the model's tool calls, as one user message holds
    /// them.
    #[must_use]
    pub fn results(results: impl IntoIterator<Item = ToolResult>) -> Self {
        Self {
            role: Role::User,
            parts: results.into_iter().map(Part::Result).collect(),
        }
    }

    /// The message's text, its parts joined.
    #[must_use]
    pub fn text(&self) -> String {
        let mut text = String::new();
        for part in &self.parts {
            if let Part::Text(part) = part {
                text.push_str(part);
            }
        }
        text
    }
}

/// One turn sent to a provider: the conversation so far, and the tools the
/// model may call.
#[derive(Clone, Debug)]
pub struct Request {
    /// The model to use, such as `gpt-5` or `claude-sonnet-4-5`.
    pub model: String,
    /// The system prompt. Empty for none.
    pub system: String,
    /// The conversation so far.
    pub messages: Vec<Message>,
    /// The tools the model may call: the registry's commands
    /// ([`crate::tools::ToolSet`]).
    pub tools: Vec<crate::tools::ToolSpec>,
    /// The most tokens the reply may take.
    pub max_tokens: u32,
}

/// The most tokens a reply takes unless the caller asks for another count.
pub const DEFAULT_MAX_TOKENS: u32 = 4096;

impl Request {
    /// A request for `model`, with no conversation and no tools yet.
    #[must_use]
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            system: String::new(),
            messages: Vec::new(),
            tools: Vec::new(),
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }
}

/// Something that happened while the model was answering, as it happened:
/// a text delta or a finished tool call.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// More text arrived.
    Text(String),
    /// A tool call is complete: its name is a tool of the request's
    /// [`ToolSet`](crate::tools::ToolSet), and its arguments are parsed.
    ToolCall(ToolCall),
}

/// Why the model stopped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stop {
    /// It finished its turn. It answered, called no tools, or called the
    /// ones in the [`Reply`] and has nothing more to say.
    End,
    /// It ran into the token limit.
    Length,
    /// Anything else a provider names, such as a content filter.
    Other(String),
}

/// One turn's answer, once its stream has ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    /// The text the model streamed.
    pub text: String,
    /// The tool calls it made, in the order they were streamed.
    pub tool_calls: Vec<ToolCall>,
    /// Why it stopped.
    pub stop: Stop,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_joins_its_text() {
        let message = Message {
            role: Role::Assistant,
            parts: vec![
                Part::Text("one ".into()),
                Part::Text("two".into()),
                Part::Call(ToolCall {
                    id: "call_1".into(),
                    name: "clip_create".into(),
                    arguments: serde_json::json!({}),
                }),
            ],
        };
        assert_eq!(message.text(), "one two");
    }
}

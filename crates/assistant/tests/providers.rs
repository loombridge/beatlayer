//! The providers against local mock servers: each wire format end to end.
//!
//! Every test starts a one-request HTTP server on localhost, answers with a
//! canned SSE stream or an error, and checks both what the provider sent
//! (the key, the tool schemas, the URL) and what it made of the answer
//! (streamed text, a tool call resolved to its registry command, a clean
//! error).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bl_assistant::keychain::{Keychain, MemoryKeychain};
use bl_assistant::message::{Event, Message, Request, Stop};
use bl_assistant::provider::{Anthropic, Error, Gemini, OpenAi, Provider};
use bl_assistant::tools::ToolSet;
use bl_commands::{CommandSpec, Registry};
use schemars::JsonSchema;
use serde_json::{Value, json};

/// The command the mock model calls, its argument type naming another, so
/// the schema carries `$defs` and a `$ref`.
fn tools() -> ToolSet {
    let mut registry = Registry::new();
    registry
        .register(
            CommandSpec::new("clip.create", "Add a clip")
                .description("Adds a clip to a track. Undo removes it.")
                .args::<CreateClip>(),
        )
        .unwrap();
    ToolSet::new(registry.iter())
}

/// The arguments of `clip.create`.
#[derive(JsonSchema, serde::Deserialize)]
struct CreateClip {
    /// The track to put the clip on.
    track: Track,
}

/// A track, by number.
#[derive(JsonSchema, serde::Deserialize)]
struct Track {
    /// The track's number, from 1.
    number: u64,
}

#[test]
fn the_mock_arguments_are_the_commands_own_shape() {
    let clip: CreateClip =
        serde_json::from_value(json!({"track": {"number": 1}})).expect("a clip's arguments");
    assert_eq!(clip.track.number, 1);
}

/// One turn to send, with the command above as its one tool.
fn request(tools: &ToolSet) -> Request {
    let mut request = Request::new("test-model");
    request.system = "You build songs.".to_owned();
    request
        .messages
        .push(Message::user("Add a clip to track 1."));
    request.tools = tools.tools().to_vec();
    request
}

/// A keychain holding `key` under `account`.
fn keychain(account: &str, key: &str) -> Arc<MemoryKeychain> {
    let keychain = MemoryKeychain::new();
    keychain.write(account, key).unwrap();
    Arc::new(keychain)
}

/// One SSE stream from its data values, each event dispatched by a blank
/// line.
fn sse(data: &[Value]) -> String {
    let mut stream = String::new();
    for value in data {
        stream.push_str("data: ");
        stream.push_str(&value.to_string());
        stream.push_str("\n\n");
    }
    stream
}

/// What arrived at the mock server.
struct Arrived {
    line: String,
    headers: BTreeMap<String, String>,
    body: String,
}

impl Arrived {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    fn body(&self) -> Value {
        serde_json::from_str(&self.body).expect("a JSON request body")
    }
}

/// A one-request HTTP server: it answers `status` and `body` to the first
/// connection with `content_type`, and hands back what arrived.
struct Mock {
    url: String,
    handle: JoinHandle<Arrived>,
}

impl Mock {
    /// A server answering one SSE stream.
    fn sse(stream: &str) -> Self {
        Self::answer("200 OK", "text/event-stream", stream)
    }

    /// A server answering one response.
    fn answer(status: &str, content_type: &str, body: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a local port");
        listener
            .set_nonblocking(true)
            .expect("a non-blocking listener");
        let url = format!("http://{}", listener.local_addr().expect("the address"));
        let (status, content_type, body) =
            (status.to_owned(), content_type.to_owned(), body.to_owned());
        let handle = thread::spawn(move || {
            let mut stream = accept(&listener);
            let arrived = read_request(&mut stream);
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("the response");
            stream.flush().expect("the response flushes");
            arrived
        });
        Self { url, handle }
    }

    fn url(&self) -> &str {
        &self.url
    }

    /// What arrived, once the request has.
    fn arrived(self) -> Arrived {
        self.handle.join().expect("the server's thread")
    }
}

/// The first connection, waiting up to ten seconds, so a provider that
/// never connects fails the test instead of hanging it.
fn accept(listener: &TcpListener) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                // A non-blocking listener passes its flag to the accepted
                // socket on macOS, and reads here must wait for the request.
                stream.set_nonblocking(false).expect("a blocking stream");
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .expect("a read timeout");
                stream
                    .set_write_timeout(Some(Duration::from_secs(10)))
                    .expect("a write timeout");
                return stream;
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("the provider never connected: {error}"),
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Arrived {
    let mut reader = BufReader::new(stream.try_clone().expect("a handle to the stream"));
    let mut line = String::new();
    reader.read_line(&mut line).expect("the request line");
    let mut headers = BTreeMap::new();
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).expect("a request header");
        if header.trim_end().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let length: usize = headers
        .get("content-length")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    reader.read_exact(&mut body).expect("the request body");
    Arrived {
        line: line.trim_end().to_owned(),
        headers,
        body: String::from_utf8(body).expect("a UTF-8 request body"),
    }
}

#[test]
fn open_ai_streams_text_and_a_tool_call_against_a_local_server() {
    let arguments = json!({"track": {"number": 1}}).to_string();
    let (head, tail) = arguments.split_at(arguments.len() / 2);
    let mut stream = sse(&[
        json!({"choices": [{"index": 0, "delta": {"role": "assistant", "content": "Adding "}}]}),
        json!({"choices": [{"index": 0, "delta": {"content": "a clip."}}]}),
        json!({"choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "id": "call_1", "type": "function", "function": {"name": "clip_create", "arguments": head}}]}}]}),
        json!({"choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "function": {"arguments": tail}}]}}]}),
        json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]}),
    ]);
    stream.push_str("data: [DONE]\n\n");
    let mock = Mock::sse(&stream);
    let provider = OpenAi::new(keychain(OpenAi::ACCOUNT, "sk-test")).base_url(mock.url());
    let tools = tools();
    let mut events = Vec::new();

    let reply = provider
        .stream(&request(&tools), &mut |event| events.push(event))
        .expect("a finished turn");

    assert_eq!(reply.text, "Adding a clip.");
    assert_eq!(reply.stop, Stop::End);
    assert_eq!(
        events,
        [
            Event::Text("Adding ".to_owned()),
            Event::Text("a clip.".to_owned()),
            Event::ToolCall(reply.tool_calls[0].clone()),
        ]
    );
    let command = reply.tool_calls[0]
        .command(&tools)
        .expect("an offered tool");
    assert_eq!(command.command, "clip.create");
    assert_eq!(command.args, json!({"track": {"number": 1}}));

    let arrived = mock.arrived();
    assert_eq!(arrived.line, "POST /chat/completions HTTP/1.1");
    assert_eq!(arrived.header("authorization"), Some("Bearer sk-test"));
    let body = arrived.body();
    assert_eq!(body["model"], "test-model");
    assert_eq!(body["stream"], true);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][1]["content"], "Add a clip to track 1.");
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["function"]["name"], "clip_create");
    assert_eq!(
        body["tools"][0]["function"]["description"],
        "Adds a clip to a track. Undo removes it."
    );
    assert_eq!(
        body["tools"][0]["function"]["parameters"]["$defs"]["Track"]["properties"]["number"]["type"],
        "integer"
    );
}

#[test]
fn open_ai_without_a_key_sends_no_authorization_header() {
    let mock = Mock::sse(&sse(&[
        json!({"choices": [{"index": 0, "delta": {"content": "Hi"}}]}),
    ]));
    let provider = OpenAi::new(Arc::new(MemoryKeychain::new())).base_url(mock.url());

    let reply = provider
        .stream(&request(&tools()), &mut |_| {})
        .expect("a finished turn");

    assert_eq!(reply.text, "Hi");
    assert_eq!(mock.arrived().header("authorization"), None);
}

#[test]
fn open_ai_reports_an_api_error_cleanly() {
    let mock = Mock::answer(
        "401 Unauthorized",
        "application/json",
        &json!({"error": {"message": "Incorrect API key provided.", "type": "invalid_request_error"}})
            .to_string(),
    );
    let provider = OpenAi::new(keychain(OpenAi::ACCOUNT, "sk-bad")).base_url(mock.url());

    let error = provider
        .stream(&request(&tools()), &mut |_| {})
        .expect_err("a refused request");

    assert_eq!(
        error,
        Error::Api {
            status: Some(401),
            message: "Incorrect API key provided.".to_owned(),
        }
    );
    assert!(error.to_string().contains("401"), "{error}");
    mock.arrived();
}

#[test]
fn anthropic_streams_text_and_a_tool_call() {
    let stream = sse(&[
        json!({"type": "message_start", "message": {"id": "msg_1", "role": "assistant"}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Adding "}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "toolu_1", "name": "clip_create", "input": {}}}),
        json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "{\"track\":{\"num"}}),
        json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "ber\":2}}"}}),
        json!({"type": "content_block_stop", "index": 1}),
        json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}}),
        json!({"type": "message_stop"}),
    ]);
    let mock = Mock::sse(&stream);
    let provider = Anthropic::new(keychain(Anthropic::ACCOUNT, "sk-ant-test")).base_url(mock.url());
    let tools = tools();
    let mut events = Vec::new();

    let reply = provider
        .stream(&request(&tools), &mut |event| events.push(event))
        .expect("a finished turn");

    assert_eq!(reply.text, "Adding ");
    assert_eq!(reply.stop, Stop::End);
    assert_eq!(
        events,
        [
            Event::Text("Adding ".to_owned()),
            Event::ToolCall(reply.tool_calls[0].clone()),
        ]
    );
    let command = reply.tool_calls[0]
        .command(&tools)
        .expect("an offered tool");
    assert_eq!(command.command, "clip.create");
    assert_eq!(command.args, json!({"track": {"number": 2}}));

    let arrived = mock.arrived();
    assert_eq!(arrived.line, "POST /messages HTTP/1.1");
    assert_eq!(arrived.header("x-api-key"), Some("sk-ant-test"));
    assert_eq!(arrived.header("anthropic-version"), Some("2023-06-01"));
    let body = arrived.body();
    assert_eq!(body["model"], "test-model");
    assert_eq!(body["max_tokens"], 4096);
    assert_eq!(body["system"], "You build songs.");
    assert_eq!(body["stream"], true);
    assert_eq!(body["tools"][0]["name"], "clip_create");
    assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(
        body["messages"][0]["content"][0]["text"],
        "Add a clip to track 1."
    );
}

#[test]
fn anthropic_reports_an_api_error_cleanly() {
    let mock = Mock::answer(
        "400 Bad Request",
        "application/json",
        &json!({"type": "error", "error": {"type": "invalid_request_error", "message": "max_tokens: must be at least 1"}})
            .to_string(),
    );
    let provider = Anthropic::new(keychain(Anthropic::ACCOUNT, "sk-ant-test")).base_url(mock.url());

    let error = provider
        .stream(&request(&tools()), &mut |_| {})
        .expect_err("a refused request");

    assert_eq!(
        error,
        Error::Api {
            status: Some(400),
            message: "max_tokens: must be at least 1".to_owned(),
        }
    );
    mock.arrived();
}

#[test]
fn gemini_streams_text_and_a_tool_call_and_converts_the_schema() {
    let stream = sse(&[
        json!({"candidates": [{"content": {"role": "model", "parts": [{"text": "Adding "}]}, "index": 0}]}),
        json!({"candidates": [{"content": {"role": "model", "parts": [{"text": "a clip."}, {"functionCall": {"name": "clip_create", "args": {"track": {"number": 3}}}}]}, "finishReason": "STOP", "index": 0}]}),
    ]);
    let mock = Mock::sse(&stream);
    let provider = Gemini::new(keychain(Gemini::ACCOUNT, "goog-test")).base_url(mock.url());
    let tools = tools();
    let mut events = Vec::new();

    let reply = provider
        .stream(&request(&tools), &mut |event| events.push(event))
        .expect("a finished turn");

    assert_eq!(reply.text, "Adding a clip.");
    assert_eq!(reply.stop, Stop::End);
    assert_eq!(
        events,
        [
            Event::Text("Adding ".to_owned()),
            Event::Text("a clip.".to_owned()),
            Event::ToolCall(reply.tool_calls[0].clone()),
        ]
    );
    let call = &reply.tool_calls[0];
    assert_eq!(call.id, "call_1", "Gemini's calls have no ID of their own");
    let command = call.command(&tools).expect("an offered tool");
    assert_eq!(command.command, "clip.create");
    assert_eq!(command.args, json!({"track": {"number": 3}}));

    let arrived = mock.arrived();
    assert_eq!(
        arrived.line,
        "POST /models/test-model:streamGenerateContent?alt=sse HTTP/1.1"
    );
    assert_eq!(arrived.header("x-goog-api-key"), Some("goog-test"));
    let body = arrived.body();
    assert_eq!(
        body["systemInstruction"]["parts"][0]["text"],
        "You build songs."
    );
    assert_eq!(body["generationConfig"]["maxOutputTokens"], 4096);
    assert_eq!(body["contents"][0]["role"], "user");
    assert_eq!(
        body["contents"][0]["parts"][0]["text"],
        "Add a clip to track 1."
    );
    let parameters = &body["tools"][0]["functionDeclarations"][0]["parameters"];
    assert_eq!(
        body["tools"][0]["functionDeclarations"][0]["name"],
        "clip_create"
    );
    assert_eq!(
        parameters["properties"]["track"]["properties"]["number"]["type"],
        "integer"
    );
    assert!(parameters.get("$defs").is_none(), "{parameters}");
}

#[test]
fn gemini_reports_an_api_error_cleanly() {
    let mock = Mock::answer(
        "400 Bad Request",
        "application/json",
        &json!({"error": {"code": 400, "message": "API key not valid.", "status": "INVALID_ARGUMENT"}})
            .to_string(),
    );
    let provider = Gemini::new(keychain(Gemini::ACCOUNT, "goog-bad")).base_url(mock.url());

    let error = provider
        .stream(&request(&tools()), &mut |_| {})
        .expect_err("a refused request");

    assert_eq!(
        error,
        Error::Api {
            status: Some(400),
            message: "API key not valid.".to_owned(),
        }
    );
    mock.arrived();
}

#[test]
fn an_error_inside_a_stream_is_reported() {
    let mock = Mock::sse(&sse(&[
        json!({"error": {"message": "the model is overloaded"}}),
    ]));
    let provider = OpenAi::new(keychain(OpenAi::ACCOUNT, "sk-test")).base_url(mock.url());

    let error = provider
        .stream(&request(&tools()), &mut |_| {})
        .expect_err("a failed stream");

    assert_eq!(
        error,
        Error::Api {
            status: None,
            message: "the model is overloaded".to_owned(),
        }
    );
    mock.arrived();
}

#[test]
fn a_provider_without_a_key_says_so() {
    let provider = Anthropic::new(Arc::new(MemoryKeychain::new()));
    let error = provider
        .stream(&request(&tools()), &mut |_| {})
        .expect_err("a provider with no key");

    assert_eq!(
        error,
        Error::MissingKey {
            provider: "Anthropic",
            account: "anthropic".to_owned(),
        }
    );
    assert!(error.to_string().contains("anthropic"), "{error}");
}

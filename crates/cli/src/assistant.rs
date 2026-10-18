//! `bl-cli assistant`: one turn of the built-in assistant, with no window
//! (WP-2.23, T107).
//!
//! It runs the app's own loop, [`bl_shell::assistant::run_turn`] over
//! `AssistantCommands`, on a headless shell holding the factory default set
//! or the set `--set` names, so a turn is one undo step and its calls go
//! through the same permission classes the app's panel applies. Nobody is
//! there to ask, so a destructive or external call is refused, and the
//! refusal goes back to the model as the app's Deny does, unless
//! `--autonomous` is given, as for `bl-cli mcp`.
//!
//! A cloud provider's key is read from the OS keychain, where the Agents
//! page stores it, and from nowhere else: there is no option, file or
//! environment variable for it.
//!
//! `--record` writes the turn as a [`Transcript`]: what the model was sent
//! and what it answered, request by request. [`Replay`] serves a transcript
//! back as a provider, so the turn runs again with no model and fails if
//! the conversation it is sent differs from the recorded one.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use bl_assistant::agent::{Stopped, Turn};
use bl_assistant::keychain::{Keychain, OsKeychain};
use bl_assistant::message::{Event, Message, Reply, Request};
use bl_assistant::provider::{Anthropic, Error, Gemini, OpenAi, Provider};
use bl_shell::assistant::{
    ASSISTANT_SET_MODEL, AssistantProvider, Decision, PromptRequest, Prompter, run_turn,
};
use bl_shell::{Shell, commands as ids};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::Outcome;

#[derive(Debug, clap::Args)]
pub struct AssistantArgs {
    /// What to ask the assistant.
    pub prompt: String,

    /// The model to ask for, such as `gemma-4-12b-it` or `claude-sonnet-5-5`.
    #[arg(long)]
    pub model: String,

    /// The API: `open-ai` (OpenAI, OpenRouter, and local servers such as
    /// Ollama and LM Studio), `anthropic` or `gemini`.
    #[arg(long, default_value = "open-ai", value_parser = parse_provider)]
    pub provider: AssistantProvider,

    /// Where the API is, such as `http://localhost:11434/v1` for Ollama.
    /// Without it, the provider's own.
    #[arg(long)]
    pub base_url: Option<String>,

    /// A set to open first: a `.blset` file, or a project folder holding
    /// one set. Without it, the factory default set is open.
    #[arg(long)]
    pub set: Option<PathBuf>,

    /// Let the assistant run destructive commands (deleting, discarding,
    /// overwriting) and external ones (files outside the project) without
    /// asking. Headless there's no one to ask, so without this flag they're
    /// refused.
    #[arg(long)]
    pub autonomous: bool,

    /// Write the turn's exchange with the model to this file as a JSON
    /// transcript.
    #[arg(long)]
    pub record: Option<PathBuf>,
}

fn parse_provider(name: &str) -> Result<AssistantProvider, String> {
    AssistantProvider::ALL
        .into_iter()
        .find(|provider| provider.id() == name)
        .ok_or_else(|| {
            let ids: Vec<_> = AssistantProvider::ALL.iter().map(|p| p.id()).collect();
            format!("expected one of: {}", ids.join(", "))
        })
}

/// The provider `kind` names, with its key read from `keychain` when a
/// request is made. `base_url` points it elsewhere than the provider's own
/// API unless it is empty.
#[must_use]
pub fn provider(
    kind: AssistantProvider,
    base_url: &str,
    keychain: Arc<dyn Keychain>,
) -> Box<dyn Provider> {
    match kind {
        AssistantProvider::OpenAi => {
            let provider = OpenAi::new(keychain);
            Box::new(if base_url.is_empty() {
                provider
            } else {
                provider.base_url(base_url)
            })
        }
        AssistantProvider::Anthropic => {
            let provider = Anthropic::new(keychain);
            Box::new(if base_url.is_empty() {
                provider
            } else {
                provider.base_url(base_url)
            })
        }
        AssistantProvider::Gemini => {
            let provider = Gemini::new(keychain);
            Box::new(if base_url.is_empty() {
                provider
            } else {
                provider.base_url(base_url)
            })
        }
    }
}

/// Answers the assistant's questions with no one there: yes to all of them
/// when autonomous, no to all of them otherwise.
pub struct Headless {
    pub autonomous: bool,
}

impl Prompter for Headless {
    fn ask(&mut self, _request: &PromptRequest) -> Decision {
        if self.autonomous {
            Decision::Once
        } else {
            Decision::Deny
        }
    }
}

/// Runs one turn for `prompt` on `shell`, asking `model` through `provider`.
///
/// # Errors
///
/// [`Error`] if the provider failed. The turn's changes so far are still
/// one undo step.
pub fn run_headless(
    shell: &mut Shell,
    provider: &dyn Provider,
    model: &str,
    autonomous: bool,
    prompt: &str,
) -> Result<Turn, Error> {
    shell
        .run(ASSISTANT_SET_MODEL, json!({ "model": model }))
        .expect("the model setting takes any name");
    run_turn(shell, provider, &mut Headless { autonomous }, prompt)
}

/// Runs `bl-cli assistant`.
///
/// # Errors
///
/// A message if the set doesn't open, the transcript can't be written, the
/// provider fails, or the turn is cut short.
pub fn run(args: &AssistantArgs) -> Outcome {
    let mut shell = Shell::headless();
    if let Some(set) = &args.set {
        shell
            .run(ids::SET_OPEN, json!({ "path": set.to_string_lossy() }))
            .map_err(|error| error.to_string())?;
    }
    let base_url = args.base_url.as_deref().unwrap_or_default();
    let api = provider(args.provider, base_url, Arc::new(OsKeychain::new()));
    let recording = Recording::new(api.as_ref());
    eprintln!(
        "bl-cli assistant: asking {} ({}; {})",
        args.model,
        args.provider.label(),
        if args.autonomous {
            "autonomous: destructive and external commands run without asking"
        } else {
            "destructive and external commands are refused; --autonomous allows them"
        }
    );
    let turn = run_headless(
        &mut shell,
        &recording,
        &args.model,
        args.autonomous,
        &args.prompt,
    )
    .map_err(|error| error.to_string())?;
    for step in &turn.steps {
        let outcome = match &step.result {
            Ok(_) => "ran".to_owned(),
            Err(why) => format!("failed: {why}"),
        };
        eprintln!("  {}: {outcome}", step.call.name);
    }
    println!("{}", turn.reply.text);
    if let Some(path) = &args.record {
        recording
            .transcript(args.provider.id(), base_url)
            .write(path)?;
        eprintln!("bl-cli assistant: recorded {}", path.display());
    }
    match turn.stopped {
        None => Ok(ExitCode::SUCCESS),
        Some(Stopped::Cap) => Err("the turn reached its limit on commands".to_owned()),
        Some(Stopped::Cancelled) => Err("the turn was cancelled".to_owned()),
    }
}

/// One turn's exchange with a model, as `--record` writes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    /// The format's version.
    pub version: u32,
    /// The provider's ID (`open-ai`, `anthropic`, `gemini`).
    pub provider: String,
    /// The base URL asked, empty for the provider's own.
    pub base_url: String,
    /// The model asked.
    pub model: String,
    /// The system prompt the model was sent. Kept to show what it was told;
    /// a replay doesn't compare it.
    pub system: String,
    /// The tools the model was offered, by name. Kept to show what it
    /// could call; a replay doesn't compare them (the registry grows), but
    /// the replay test checks every command the transcript calls exists.
    pub tools: Vec<String>,
    /// The requests in order, each with the model's answer.
    pub exchanges: Vec<Exchange>,
}

/// One request to the model and its answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Exchange {
    /// The messages this request added to the conversation: the person's
    /// prompt for the first, and for each later one the results of the
    /// calls the last answer made. The conversation before them is the
    /// earlier requests' messages and answers.
    pub messages: Vec<Message>,
    /// What the model answered: its text and tool calls.
    pub response: Reply,
}

/// The current version of [`Transcript`].
pub const TRANSCRIPT_VERSION: u32 = 1;

impl Transcript {
    /// Reads a transcript from `path`.
    ///
    /// # Errors
    ///
    /// A message if the file can't be read or isn't a transcript.
    pub fn read(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path)
            .map_err(|error| format!("couldn't read {}: {error}", path.display()))?;
        serde_json::from_str(&text)
            .map_err(|error| format!("{} isn't a transcript: {error}", path.display()))
    }

    /// Writes the transcript to `path`.
    ///
    /// # Errors
    ///
    /// A message if the file can't be written.
    pub fn write(&self, path: &Path) -> Result<(), String> {
        let mut text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        text.push('\n');
        fs::write(path, text).map_err(|error| format!("couldn't write {}: {error}", path.display()))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A provider that passes a turn to another and keeps the exchange.
pub struct Recording<'a> {
    inner: &'a dyn Provider,
    taken: Mutex<Taken>,
}

#[derive(Default)]
struct Taken {
    model: String,
    system: String,
    tools: Vec<String>,
    exchanges: Vec<Exchange>,
    /// How many of the conversation's messages the exchanges hold: the
    /// requests' own, and the model's answer to the last.
    held: usize,
}

impl<'a> Recording<'a> {
    #[must_use]
    pub fn new(inner: &'a dyn Provider) -> Self {
        Self {
            inner,
            taken: Mutex::default(),
        }
    }

    /// The exchange so far, for a turn run with `provider` at `base_url`.
    #[must_use]
    pub fn transcript(&self, provider: &str, base_url: &str) -> Transcript {
        let taken = lock(&self.taken);
        Transcript {
            version: TRANSCRIPT_VERSION,
            provider: provider.to_owned(),
            base_url: base_url.to_owned(),
            model: taken.model.clone(),
            system: taken.system.clone(),
            tools: taken.tools.clone(),
            exchanges: taken.exchanges.clone(),
        }
    }
}

impl Provider for Recording<'_> {
    fn stream(&self, request: &Request, on_event: &mut dyn FnMut(Event)) -> Result<Reply, Error> {
        let reply = self.inner.stream(request, on_event)?;
        let mut taken = lock(&self.taken);
        if taken.exchanges.is_empty() {
            taken.model.clone_from(&request.model);
            taken.system.clone_from(&request.system);
            taken.tools = request.tools.iter().map(|tool| tool.name.clone()).collect();
        }
        let added = request
            .messages
            .get(taken.held..)
            .unwrap_or_default()
            .to_vec();
        taken.exchanges.push(Exchange {
            messages: added,
            response: reply.clone(),
        });
        // The agent loop appends the answer to the conversation next.
        taken.held = request.messages.len() + 1;
        Ok(reply)
    }
}

/// A provider that serves a [`Transcript`]'s answers in order, and fails
/// with a diff when the conversation it is sent differs from the recorded
/// one: a changed prompt, or a tool result that is not the one recorded.
pub struct Replay {
    transcript: Transcript,
    served: Mutex<Served>,
}

#[derive(Default)]
struct Served {
    exchanges: usize,
    held: usize,
}

impl Replay {
    #[must_use]
    pub fn new(transcript: Transcript) -> Self {
        Self {
            transcript,
            served: Mutex::default(),
        }
    }

    /// Whether the turn asked for every exchange the transcript holds.
    ///
    /// # Errors
    ///
    /// How many it asked for, if fewer.
    pub fn finished(&self) -> Result<(), String> {
        let asked = lock(&self.served).exchanges;
        let held = self.transcript.exchanges.len();
        if asked == held {
            Ok(())
        } else {
            Err(format!(
                "the turn made {asked} requests, and the transcript holds {held}"
            ))
        }
    }
}

impl Provider for Replay {
    fn stream(&self, request: &Request, on_event: &mut dyn FnMut(Event)) -> Result<Reply, Error> {
        let mut served = lock(&self.served);
        let number = served.exchanges + 1;
        let drifted = |why: String| {
            Error::Protocol(format!(
                "the conversation drifted from the transcript at request {number}: {why}"
            ))
        };
        let Some(exchange) = self.transcript.exchanges.get(served.exchanges) else {
            return Err(drifted(format!(
                "the transcript holds only {} requests",
                self.transcript.exchanges.len()
            )));
        };
        let sent = request.messages.get(served.held..).unwrap_or_default();
        if sent != exchange.messages {
            return Err(drifted(diff(&exchange.messages, sent)));
        }
        served.exchanges += 1;
        served.held = request.messages.len() + 1;
        let reply = exchange.response.clone();
        if !reply.text.is_empty() {
            on_event(Event::Text(reply.text.clone()));
        }
        for call in &reply.tool_calls {
            on_event(Event::ToolCall(call.clone()));
        }
        Ok(reply)
    }
}

/// Where two lists of messages first differ, line by line, as pretty JSON.
fn diff(recorded: &[Message], sent: &[Message]) -> String {
    let lines = |messages: &[Message]| -> Vec<String> {
        serde_json::to_string_pretty(messages)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    };
    let (recorded, sent) = (lines(recorded), lines(sent));
    let line = recorded
        .iter()
        .zip(&sent)
        .position(|(a, b)| a != b)
        .unwrap_or(recorded.len().min(sent.len()));
    let at = |lines: &[String]| {
        lines
            .get(line)
            .cloned()
            .unwrap_or_else(|| "(nothing)".to_owned())
    };
    format!(
        "first difference at line {}\n  recorded: {}\n  sent:     {}",
        line + 1,
        at(&recorded).trim(),
        at(&sent).trim()
    )
}

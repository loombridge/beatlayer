//! Asks a model every question with the data in each format, scores the
//! answers against the keys and counts the tokens the provider reports.
//!
//! `run_the_benchmark` is `--ignored`: it needs a model. Point it at a
//! server with environment variables (the model's name, the server's URL and
//! the API are not secrets; a cloud provider's key is read from the OS
//! keychain, where the Agents page stores it, and from nowhere else):
//!
//! ```text
//! BL_TOON_BENCH_MODEL=gemma-4-12b-it \
//! BL_TOON_BENCH_BASE_URL=http://127.0.0.1:8080/v1 \
//!   cargo test -p bl-cli --test toon_bench -- --ignored --nocapture run_the_benchmark
//! ```
//!
//! `BL_TOON_BENCH_PROVIDER` is `open-ai` (the default, which serves Ollama
//! and LM Studio too), `anthropic` or `gemini`. The report prints to
//! standard output.

use std::collections::BTreeMap;
use std::sync::Arc;

use bl_assistant::keychain::{Keychain, OsKeychain};
use bl_assistant::message::{Event, Message, Request, Usage};
use bl_assistant::provider::{Error, Provider};
use bl_assistant::tools::ToolSet;
use bl_cli::assistant::provider;
use bl_shell::Shell;
use bl_shell::assistant::AssistantProvider;
use serde_json::Value;

use crate::questions::{self, Question, same};
use crate::{CALLS, Call, fixture};

/// How a result is written in the prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Format {
    /// Compact JSON, as the agent API sends it by default.
    Json,
    /// TOON, always: this measures TOON on every result, including those
    /// the server's rule would leave as JSON.
    Toon,
}

impl Format {
    pub const ALL: [Self; 2] = [Self::Json, Self::Toon];

    fn write(self, value: &Value) -> String {
        match self {
            Self::Json => value.to_string(),
            Self::Toon => bl_agent::toon::encode(value).text,
        }
    }
}

const SYSTEM: &str = "You answer questions about the result of a tool of a music production \
    program. Answer with the answer only, on one line, without explanation: a number, a name, or \
    values separated by commas. Give numbers without units.";

/// The prompt for one question about `data`, the result of `tool`.
fn prompt(tool: &str, data: &str, question: &str) -> String {
    format!("The `{tool}` tool returned this result:\n\n{data}\n\nQuestion: {question}")
}

/// What one request cost, and what the model said.
struct Asked {
    answer: String,
    usage: Usage,
}

/// Sends one request with no tools, unless `tools` says otherwise.
fn ask(
    api: &dyn Provider,
    model: &str,
    text: &str,
    tools: Option<&ToolSet>,
) -> Result<Asked, Error> {
    let mut request = Request::new(model);
    SYSTEM.clone_into(&mut request.system);
    request.messages.push(Message::user(text));
    request.max_tokens = 512;
    if let Some(tools) = tools {
        request.tools = tools.tools().to_vec();
    }
    let mut usage = Usage::default();
    let reply = api.stream(&request, &mut |event| {
        if let Event::Usage(counted) = event {
            usage = counted;
        }
    })?;
    // A model may think aloud first: its answer is its last line.
    let answer = reply
        .text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .to_owned();
    Ok(Asked { answer, usage })
}

/// What a fixture cost and scored in one format.
#[derive(Clone, Copy, Debug, Default)]
pub struct Tally {
    pub asked: u64,
    pub right: u64,
    pub input: u64,
    pub output: u64,
}

/// Every question asked of `api` with the data in each format, per fixture,
/// and what a request with no data costs, per fixture.
pub struct Results {
    pub tallies: BTreeMap<(&'static str, Format), Tally>,
    pub overhead: BTreeMap<&'static str, u64>,
}

/// Asks every question in `questions` of `model`, `reps` times each.
pub fn run(
    api: &dyn Provider,
    model: &str,
    questions: &[Question],
    reps: u64,
) -> Result<Results, Error> {
    let mut results = Results {
        tallies: BTreeMap::new(),
        overhead: BTreeMap::new(),
    };
    for Call { name, tool, .. } in CALLS {
        let value = fixture(name);
        let asked_here: Vec<&Question> = questions.iter().filter(|q| q.fixture == name).collect();
        let Some(first) = asked_here.first() else {
            continue;
        };
        let bare = ask(api, model, &prompt(tool, "", first.text), None)?;
        results.overhead.insert(name, bare.usage.input);
        for format in Format::ALL {
            let data = format.write(&value);
            let tally = results.tallies.entry((name, format)).or_default();
            for question in &asked_here {
                for _ in 0..reps {
                    let asked = ask(api, model, &prompt(tool, &data, question.text), None)?;
                    tally.asked += 1;
                    tally.right += u64::from(same(&question.key(), &asked.answer));
                    tally.input += asked.usage.input;
                    tally.output += asked.usage.output;
                }
            }
        }
    }
    Ok(results)
}

/// The tokens the registry's commands cost as tool definitions in a
/// request: the same request with and without them.
pub fn tool_definitions(api: &dyn Provider, model: &str) -> Result<(usize, u64), Error> {
    let shell = Shell::headless();
    let tools = ToolSet::new(shell.registry().iter());
    let with = ask(api, model, "Hello.", Some(&tools))?;
    let without = ask(api, model, "Hello.", None)?;
    Ok((
        tools.tools().len(),
        with.usage.input.saturating_sub(without.usage.input),
    ))
}

/// The report, as Markdown.
pub fn report(results: &Results, model: &str, definitions: Option<(usize, u64)>) -> String {
    let mut out = format!("## TOON against compact JSON, asked of `{model}`\n\n");
    let total = |format: Format| {
        results
            .tallies
            .iter()
            .filter(|((_, f), _)| *f == format)
            .fold(Tally::default(), |sum, (_, t)| Tally {
                asked: sum.asked + t.asked,
                right: sum.right + t.right,
                input: sum.input + t.input,
                output: sum.output + t.output,
            })
    };
    out.push_str("| Format | Right | Input tokens | Output tokens |\n|---|---|---|---|\n");
    for format in Format::ALL {
        let t = total(format);
        out.push_str(&format!(
            "| {format:?} | {}/{} | {} | {} |\n",
            t.right, t.asked, t.input, t.output
        ));
    }
    out.push_str(
        "\nPer result, with the tokens of the data alone (a request's input tokens, less the \
         same request with no data, averaged over the questions):\n\n\
         | Result | Tool | Right, JSON | Right, TOON | Data tokens, JSON | Data tokens, TOON | Saved | Use TOON |\n\
         |---|---|---|---|---|---|---|---|\n",
    );
    for Call { name, tool, .. } in CALLS {
        let (Some(json), Some(toon)) = (
            results.tallies.get(&(name, Format::Json)),
            results.tallies.get(&(name, Format::Toon)),
        ) else {
            continue;
        };
        let overhead = results.overhead.get(name).copied().unwrap_or_default();
        let data = |t: &Tally| (t.input / t.asked.max(1)).saturating_sub(overhead);
        let (json_tokens, toon_tokens) = (data(json), data(toon));
        let saved = 100.0 * (json_tokens as f64 - toon_tokens as f64) / json_tokens.max(1) as f64;
        let use_toon = toon.right >= json.right && saved >= 15.0;
        out.push_str(&format!(
            "| {name} | {tool} | {}/{} | {}/{} | {json_tokens} | {toon_tokens} | {saved:.0}% | {} |\n",
            json.right,
            json.asked,
            toon.right,
            toon.asked,
            if use_toon { "yes" } else { "no" }
        ));
    }
    if let Some((count, tokens)) = definitions {
        out.push_str(&format!(
            "\nThe assistant's tool definitions ({count} tools, one per registry command) cost \
             {tokens} input tokens per request.\n"
        ));
    }
    out
}

/// The provider and model the environment names.
fn from_environment() -> (Box<dyn Provider>, String) {
    let model =
        std::env::var("BL_TOON_BENCH_MODEL").expect("BL_TOON_BENCH_MODEL names the model to ask");
    let kind = std::env::var("BL_TOON_BENCH_PROVIDER").unwrap_or_else(|_| "open-ai".to_owned());
    let kind = AssistantProvider::ALL
        .into_iter()
        .find(|p| p.id() == kind)
        .unwrap_or_else(|| {
            panic!("BL_TOON_BENCH_PROVIDER `{kind}` isn't open-ai, anthropic or gemini")
        });
    let base_url = std::env::var("BL_TOON_BENCH_BASE_URL").unwrap_or_default();
    let keychain: Arc<dyn Keychain> = Arc::new(OsKeychain::new());
    (provider(kind, &base_url, keychain), model)
}

#[test]
#[ignore = "asks a model; see the module's documentation"]
fn run_the_benchmark() {
    let (api, model) = from_environment();
    let reps = std::env::var("BL_TOON_BENCH_REPS")
        .ok()
        .and_then(|reps| reps.parse().ok())
        .unwrap_or(1);
    let questions = questions::all();
    let results = run(api.as_ref(), &model, &questions, reps).unwrap_or_else(|e| panic!("{e}"));
    let definitions = tool_definitions(api.as_ref(), &model).unwrap_or_else(|e| panic!("{e}"));
    println!("{}", report(&results, &model, Some(definitions)));
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use bl_assistant::message::{Reply, Stop};

    use super::*;

    /// A model that answers each question from the key, unless told to get
    /// one wrong, and reports one input token per four bytes sent.
    struct Keyed {
        wrong: &'static str,
        seen: Mutex<usize>,
    }

    impl Provider for Keyed {
        fn stream(
            &self,
            request: &Request,
            on_event: &mut dyn FnMut(Event),
        ) -> Result<Reply, Error> {
            *self.seen.lock().unwrap() += 1;
            let text = request.messages[0].text();
            let answer = questions::all()
                .into_iter()
                .find(|q| text.ends_with(&format!("Question: {}", q.text)))
                .map(|q| {
                    if q.id == self.wrong {
                        "wrong".to_owned()
                    } else {
                        q.key()
                    }
                })
                .unwrap_or_default();
            let sent = text.len() + request.tools.len() * 400;
            on_event(Event::Usage(Usage {
                input: (sent / 4) as u64,
                output: 3,
            }));
            Ok(Reply {
                text: format!("Thinking aloud.\n{answer}\n"),
                tool_calls: Vec::new(),
                stop: Stop::End,
            })
        }
    }

    #[test]
    fn the_runner_scores_answers_and_counts_the_tokens_the_provider_reports() {
        let model = Keyed {
            wrong: "tracks-muted",
            seen: Mutex::new(0),
        };
        let all = questions::all();
        let results = run(&model, "stub", &all, 1).unwrap();
        // One request per question and format, and one with no data per fixture.
        assert_eq!(*model.seen.lock().unwrap(), 2 * all.len() + CALLS.len());
        let tracks = &results.tallies[&("tracks", Format::Toon)];
        let questions_about_tracks = all.iter().filter(|q| q.fixture == "tracks").count() as u64;
        assert_eq!(tracks.asked, questions_about_tracks);
        assert_eq!(tracks.right, questions_about_tracks - 1);
        assert_eq!(tracks.output, 3 * questions_about_tracks);
        let notes_json = results.tallies[&("notes_drums", Format::Json)];
        let notes_toon = results.tallies[&("notes_drums", Format::Toon)];
        assert_eq!(notes_json.right, notes_json.asked);
        // The same notes cost less written as a table.
        assert!(
            notes_toon.input < notes_json.input,
            "{notes_toon:?} {notes_json:?}"
        );

        let (count, tokens) = tool_definitions(&model, "stub").unwrap();
        assert!(count > 100);
        assert_eq!(tokens, (count * 400 / 4) as u64);

        let report = report(&results, "stub", Some((count, tokens)));
        assert!(report.contains("| notes_drums | query |"), "{report}");
        assert!(report.contains("tool definitions"), "{report}");
    }

    #[test]
    fn answers_compare_the_way_a_person_would() {
        assert!(same("Drums, Bass", "drums,bass"));
        assert!(same("96", "96.0"));
        assert!(same("E minor", "**E minor**."));
        assert!(same("250", "250 "));
        assert!(!same("11", "12"));
    }
}

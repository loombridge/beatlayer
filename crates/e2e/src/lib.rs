//! Beatlayer's headless end-to-end suite: the harness the test families in
//! `tests/` build on.
//!
//! The suite drives the whole program the way an agent does, through the
//! agent API, and the way the app's window does each frame, so it catches
//! what crate tests miss: background work that only lands when someone
//! polls, live playback that drifts from the offline render, sets that
//! don't survive a save, edits that undo to something else.
//!
//! | Piece | What it is |
//! |---|---|
//! | [`host`] | [`Host`]: the app's host wiring without a window, on the null device paced by hand |
//! | [`stdio_host`] | [`StdioHost`]: the agent API exactly as `bl-cli mcp` serves it, over in-memory pipes |
//! | [`capture`] | The cable that copies what the null device played |
//! | [`audio`] | [`Audio`], and measurements of it: levels, loudness, alignment |
//! | [`device_rig`] | Every native device type and rack on a track of its own, set and rendered through the agent API |
//! | [`fixture`] | The rich set, built through the agent API, and the [`Ids`] it made |
//! | [`known`] | The known-failures ratchet over `tests/known-failures.toml` |
//! | [`schema`] | Arguments for any command, made from its JSON Schema and a set's objects |
//!
//! Both hosts implement [`Driver`], so a scenario written against it runs
//! in either.
//!
//! Tests never wait on the wall clock: the null device runs only the blocks
//! a test asks for ([`Host::advance_ms`]), so what plays lands on the same
//! frames however busy the machine is.

pub mod audio;
pub mod capture;
pub mod device_rig;
pub mod fixture;
pub mod host;
pub mod known;
pub mod schema;
pub mod stdio_host;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

pub use audio::Audio;
pub use capture::{Capture, Recorder};
pub use fixture::Ids;
pub use host::{ExportSpec, Host};
pub use known::{Known, Ratchet};
pub use stdio_host::StdioHost;

/// A 4/4 bar, in ticks.
pub const BAR: i64 = bl_core::time::Ticks::WHOLE.get();
/// A quarter note, in ticks.
pub const BEAT: i64 = bl_core::time::Ticks::QUARTER.get();
/// A sixteenth note, in ticks.
pub const SIXTEENTH: i64 = bl_core::time::Ticks::SIXTEENTH.get();

/// Something that serves the agent API's tools: [`Host`] in this process,
/// or [`StdioHost`] over stdio. Scenarios and the fixture are written
/// against it, so they run on either.
pub trait Driver {
    /// Calls the tool `name` with `args`, and returns its result, or the
    /// tool's error text.
    ///
    /// # Errors
    ///
    /// The tool's error, or why the call couldn't be made.
    fn tool(&mut self, name: &str, args: Value) -> Result<Value, String>;

    /// Runs one command through the `run` tool, so permissions and `$new`
    /// and `$ref` placeholders apply, and returns the command's own result
    /// (`results[0].result`).
    ///
    /// # Errors
    ///
    /// The tool's error text: the command failed, or was refused.
    fn run(&mut self, command: &str, args: Value) -> Result<Value, String> {
        let reply = self.tool(
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )?;
        Ok(reply["results"][0]["result"].clone())
    }

    /// [`run`](Self::run), panicking with the command and its error if it
    /// fails.
    fn run_ok(&mut self, command: &str, args: Value) -> Value {
        self.run(command, args)
            .unwrap_or_else(|error| panic!("`{command}` failed: {error}"))
    }

    /// Runs `commands` (each `{"command": .., "args": ..}`) as one batch,
    /// one undo step named `label`, and returns the whole reply: `ids` maps
    /// each `$new` name to the ID it got.
    ///
    /// # Errors
    ///
    /// The tool's error text; nothing changed.
    fn batch(&mut self, label: &str, commands: Vec<Value>) -> Result<Value, String> {
        self.tool("run", json!({ "label": label, "commands": commands }))
    }

    /// [`batch`](Self::batch), panicking with the label and the error if it
    /// fails.
    fn batch_ok(&mut self, label: &str, commands: Vec<Value>) -> Value {
        self.batch(label, commands)
            .unwrap_or_else(|error| panic!("batch `{label}` failed: {error}"))
    }

    /// The `query` tool with `args`, panicking if it fails.
    fn query(&mut self, args: Value) -> Value {
        self.tool("query", args.clone())
            .unwrap_or_else(|error| panic!("query {args} failed: {error}"))
    }
}

/// Whether the tests run at full depth: `BL_TEST_DEPTH=full`, as on pushes
/// to main and in the nightly run. Quick, the default, keeps every pull
/// request's run short.
///
/// # Panics
///
/// If `BL_TEST_DEPTH` is set to anything but `quick` or `full`.
#[must_use]
pub fn full_depth() -> bool {
    match std::env::var("BL_TEST_DEPTH").as_deref() {
        Ok("full") => true,
        Ok("quick") | Err(_) => false,
        Ok(other) => panic!("BL_TEST_DEPTH is `{other}`: set it to quick or full"),
    }
}

thread_local! {
    /// The copy of the fixtures [`copy_fixtures`] made for this thread.
    static FIXTURES: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// The test fixtures folder: the repository's `tests/fixtures`, or the copy
/// [`copy_fixtures`] made for this thread. The repository's is a plain
/// absolute path, with no `..` in it, so a set that uses a fixture records it in its media
/// table as a person's file would be.
#[must_use]
pub fn fixtures() -> PathBuf {
    FIXTURES
        .with(|copy| copy.borrow().clone())
        .unwrap_or_else(repository_fixtures)
}

/// The repository's own `tests/fixtures`, canonicalised.
fn repository_fixtures() -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    path.canonicalize().unwrap_or(path)
}

/// Copies the repository's fixture folders named in `folders` (such as
/// `drums` and `media`) into `dir`, and makes [`fixtures`], and so
/// [`fixture::media`] and [`fixture::drums`], name the copies on this
/// thread from now on: for a test whose commands may write beside a file
/// they are handed, or delete it, so the repository's own files stay as
/// they are. Returns `dir`.
///
/// # Panics
///
/// If a folder doesn't copy.
pub fn copy_fixtures(dir: &Path, folders: &[&str]) -> PathBuf {
    fn copy(from: &Path, to: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            let target = to.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                copy(&entry.path(), &target)?;
            } else {
                std::fs::copy(entry.path(), target)?;
            }
        }
        Ok(())
    }
    let repository = repository_fixtures();
    for folder in folders {
        copy(&repository.join(folder), &dir.join(folder))
            .unwrap_or_else(|error| panic!("the {folder} fixtures didn't copy: {error}"));
    }
    FIXTURES.with(|copy| *copy.borrow_mut() = Some(dir.to_path_buf()));
    dir.to_path_buf()
}

/// Where two documents differ, as JSON pointers into their serialised form
/// with the values on each side, at most `limit` of them; empty when they
/// serialise the same. For a readable failure where `Document: PartialEq`
/// says only that they differ.
#[must_use]
pub fn document_diff(
    before: &bl_model::Document,
    after: &bl_model::Document,
    limit: usize,
) -> Vec<String> {
    fn walk(path: &str, a: &Value, b: &Value, out: &mut Vec<String>, limit: usize) {
        if out.len() >= limit || a == b {
            return;
        }
        match (a, b) {
            (Value::Object(x), Value::Object(y)) => {
                let keys: std::collections::BTreeSet<&String> = x.keys().chain(y.keys()).collect();
                for key in keys {
                    let null = Value::Null;
                    walk(
                        &format!("{path}/{key}"),
                        x.get(key).unwrap_or(&null),
                        y.get(key).unwrap_or(&null),
                        out,
                        limit,
                    );
                }
            }
            (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
                for (index, (p, q)) in x.iter().zip(y).enumerate() {
                    walk(&format!("{path}/{index}"), p, q, out, limit);
                }
            }
            _ => {
                let show = |v: &Value| {
                    let text = v.to_string();
                    if text.len() > 200 {
                        format!("{}…", &text[..text.floor_char_boundary(200)])
                    } else {
                        text
                    }
                };
                out.push(format!("{path}: {} then {}", show(a), show(b)));
            }
        }
    }
    let a = serde_json::to_value(before).unwrap_or_default();
    let b = serde_json::to_value(after).unwrap_or_default();
    let mut out = Vec::new();
    walk("", &a, &b, &mut out, limit);
    out
}

/// A folder in the system's temporary folder, removed with everything in it
/// when dropped.
#[derive(Debug)]
pub struct TempDir(PathBuf);

impl TempDir {
    /// A new, empty folder whose name starts `bl-e2e-<name>`.
    ///
    /// # Panics
    ///
    /// If the folder can't be made.
    #[must_use]
    pub fn new(name: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "bl-e2e-{name}-{}-{nanos}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("a temporary folder");
        Self(path)
    }

    /// Its path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

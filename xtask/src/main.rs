//! Repository tasks. Run `cargo xtask help` for the list.

mod package;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use serde_json::Value;

mod commands;
mod keymap;
mod ui_parity;

const USAGE: &str = "\
Usage: cargo xtask <task>

Tasks:
  check          Run every check: format, clippy, tests, layers, the UI parity
                 reference, the command reference, the keymap, deny, and the deny
                 self-test
  layers         Check the crate layering rules
  ui-parity      Validate docs/ui-parity/shortcuts.toml and regenerate shortcuts.md
                 (with --check, fail instead of writing if shortcuts.md is out of date)
  commands       Check the command registry and regenerate docs/commands.md
                 (with --check, fail instead of writing if commands.md is out of date)
  keymap         Regenerate crates/ui/keymaps/live12.toml from shortcuts.toml and the
                 command registry (with --check, fail instead if it's out of date)
  deny-selftest  Prove that deny.toml still rejects an incompatible licence
  package        Build this platform's packages with cargo-packager and check they start
  help           Show this message
";

/// The only crate allowed to depend on `bl-ui`.
const APP: &str = "beatlayer";

/// Crates allowed to depend on windowing and GPU crates.
const UI_ALLOWED: &[&str] = &[APP, "bl-ui", "bl-plugin-host"];

/// Windowing, GPU, and UI-toolkit crates that headless crates must not reach.
const UI_CRATES: &[&str] = &[
    "baseview",
    "eframe",
    "egui",
    "freya",
    "gpui",
    "iced",
    "makepad-widgets",
    "masonry",
    "slint",
    "vizia",
    "wgpu",
    "winit",
    "xilem",
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = workspace_root().and_then(|root| run(&root, &args));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let task = args.first().map(String::as_str);
    let options = args.get(1..).unwrap_or_default();
    match task {
        Some("check") => check(root),
        Some("layers") => layers(root),
        Some("ui-parity") => match options {
            [] => ui_parity::generate(root),
            [flag] if flag == "--check" => ui_parity::check(root),
            _ => Err(format!("usage: cargo xtask ui-parity [--check]\n\n{USAGE}")),
        },
        Some("commands") => match options {
            [] => commands::generate(root),
            [flag] if flag == "--check" => commands::check(root),
            _ => Err(format!("usage: cargo xtask commands [--check]\n\n{USAGE}")),
        },
        Some("keymap") => match options {
            [] => keymap::generate(root),
            [flag] if flag == "--check" => keymap::check(root),
            _ => Err(format!("usage: cargo xtask keymap [--check]\n\n{USAGE}")),
        },
        Some("deny-selftest") => deny_selftest(root),
        Some("package") => package::package(root),
        None | Some("help" | "-h" | "--help") => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown task `{other}`\n\n{USAGE}")),
    }
}

/// Runs every check, reports each result, and fails if any failed.
fn check(root: &Path) -> Result<(), String> {
    require_cargo_deny()?;
    type Step<'a> = (&'a str, Box<dyn Fn() -> Result<(), String> + 'a>);
    let steps: Vec<Step<'_>> = vec![
        (
            "format",
            Box::new(|| cargo(root, &["fmt", "--all", "--check"])),
        ),
        (
            "clippy",
            Box::new(|| {
                cargo(
                    root,
                    &[
                        "clippy",
                        "--workspace",
                        "--all-targets",
                        "--all-features",
                        "--locked",
                        "--",
                        "-D",
                        "warnings",
                    ],
                )
            }),
        ),
        (
            "tests",
            Box::new(|| cargo(root, &["test", "--workspace", "--all-features", "--locked"])),
        ),
        ("layers", Box::new(|| layers(root))),
        ("ui parity reference", Box::new(|| ui_parity::check(root))),
        ("command reference", Box::new(|| commands::check(root))),
        ("keymap", Box::new(|| keymap::check(root))),
        (
            "licences, advisories, bans, sources",
            Box::new(|| cargo(root, &["deny", "--locked", "check"])),
        ),
        ("deny self-test", Box::new(|| deny_selftest(root))),
    ];

    let mut failed = Vec::new();
    for (name, step) in &steps {
        eprintln!("\n==> {name}");
        if let Err(message) = step() {
            eprintln!("{message}");
            failed.push(*name);
        }
    }

    eprintln!("\n==> summary");
    for (name, _) in &steps {
        let status = if failed.contains(name) {
            "FAILED"
        } else {
            "ok"
        };
        eprintln!("{status:>6}  {name}");
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("{} of {} checks failed", failed.len(), steps.len()))
    }
}

/// Enforces the layering in docs/02-architecture.md:
///
/// 1. Only the application crate depends on `bl-ui`.
/// 2. Headless crates never reach a windowing, GPU, or UI-toolkit crate.
///
/// Both rules follow normal and build dependencies, transitively. Dev
/// dependencies don't ship, so they're not checked.
fn layers(root: &Path) -> Result<(), String> {
    let metadata = cargo_metadata(root)?;
    let graph = DependencyGraph::from_metadata(&metadata)?;

    let mut violations = Vec::new();
    for member in &graph.members {
        let name = graph.name(member);
        if name == "xtask" {
            continue;
        }
        if name != APP
            && let Some(path) = graph.path_to(member, |dep| dep == "bl-ui")
        {
            violations.push(format!("{name} depends on bl-ui: {path}"));
        }
        if !UI_ALLOWED.contains(&name)
            && let Some(path) = graph.path_to(member, |dep| UI_CRATES.contains(&dep))
        {
            violations.push(format!("{name} reaches a UI crate: {path}"));
        }
    }

    if violations.is_empty() {
        eprintln!("{} crates follow the layering rules", graph.members.len());
        Ok(())
    } else {
        Err(format!(
            "layering rules broken (see docs/02-architecture.md):\n  {}",
            violations.join("\n  ")
        ))
    }
}

/// Runs cargo-deny against a fixture that depends on a GPL-2.0-only crate,
/// and fails unless deny.toml rejects it.
fn deny_selftest(root: &Path) -> Result<(), String> {
    require_cargo_deny()?;
    let manifest = root.join("xtask/fixtures/licence-reject/Cargo.toml");
    let output = Command::new(cargo_bin())
        .arg("deny")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--config")
        .arg(root.join("deny.toml"))
        .args(["check", "licenses"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("couldn't run cargo deny: {e}"))?;
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        return Err(
            "deny.toml accepted a GPL-2.0-only dependency; the licence policy is broken".into(),
        );
    }
    if !stderr.contains("GPL-2.0-only") {
        return Err(format!(
            "cargo deny failed on the fixture, but not because of its GPL-2.0-only crate:\n{stderr}"
        ));
    }
    eprintln!("deny.toml rejects a GPL-2.0-only dependency, as intended");
    Ok(())
}

/// The resolved dependency graph from `cargo metadata`, by package ID.
struct DependencyGraph {
    members: Vec<String>,
    names: BTreeMap<String, String>,
    edges: BTreeMap<String, Vec<String>>,
}

impl DependencyGraph {
    fn from_metadata(metadata: &Value) -> Result<Self, String> {
        let malformed = |what: &str| format!("unexpected `cargo metadata` output: no {what}");

        let names = metadata["packages"]
            .as_array()
            .ok_or_else(|| malformed("packages"))?
            .iter()
            .filter_map(|p| Some((p["id"].as_str()?.to_owned(), p["name"].as_str()?.to_owned())))
            .collect();

        let members = metadata["workspace_members"]
            .as_array()
            .ok_or_else(|| malformed("workspace_members"))?
            .iter()
            .filter_map(|id| id.as_str().map(str::to_owned))
            .collect();

        let mut edges = BTreeMap::new();
        for node in metadata["resolve"]["nodes"]
            .as_array()
            .ok_or_else(|| malformed("resolve.nodes"))?
        {
            let Some(id) = node["id"].as_str() else {
                continue;
            };
            let deps = node["deps"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|dep| ships(dep))
                .filter_map(|dep| dep["pkg"].as_str().map(str::to_owned))
                .collect();
            edges.insert(id.to_owned(), deps);
        }

        Ok(Self {
            members,
            names,
            edges,
        })
    }

    fn name<'a>(&'a self, id: &'a str) -> &'a str {
        self.names.get(id).map_or(id, String::as_str)
    }

    /// The shortest dependency path from `start` to a package whose name
    /// matches, written as `a -> b -> c`, or `None` if there isn't one.
    fn path_to(&self, start: &str, matches: impl Fn(&str) -> bool) -> Option<String> {
        let mut previous: BTreeMap<&str, &str> = BTreeMap::new();
        let mut seen = BTreeSet::from([start]);
        let mut queue = VecDeque::from([start]);

        while let Some(id) = queue.pop_front() {
            for dep in self.edges.get(id).into_iter().flatten() {
                let dep = dep.as_str();
                if !seen.insert(dep) {
                    continue;
                }
                previous.insert(dep, id);
                if matches(self.name(dep)) {
                    let mut path = vec![self.name(dep)];
                    let mut at = dep;
                    while let Some(&before) = previous.get(at) {
                        path.push(self.name(before));
                        at = before;
                    }
                    path.reverse();
                    return Some(path.join(" -> "));
                }
                queue.push_back(dep);
            }
        }
        None
    }
}

/// Whether a dependency edge from `cargo metadata` ships in a build, that is,
/// whether it's a normal or build dependency rather than only a dev one.
fn ships(dep: &Value) -> bool {
    dep["dep_kinds"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|kind| kind["kind"].is_null() || kind["kind"] == "build")
}

fn cargo_metadata(root: &Path) -> Result<Value, String> {
    let output = Command::new(cargo_bin())
        .args([
            "metadata",
            "--format-version",
            "1",
            "--all-features",
            "--locked",
        ])
        .current_dir(root)
        .output()
        .map_err(|e| format!("couldn't run cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("couldn't parse cargo metadata: {e}"))
}

/// Runs a cargo command in the workspace root, streaming its output.
fn cargo(root: &Path, args: &[&str]) -> Result<(), String> {
    let status = Command::new(cargo_bin())
        .args(args)
        .current_dir(root)
        .status()
        .map_err(|e| format!("couldn't run cargo {}: {e}", args.join(" ")))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo {} failed", args.join(" ")))
    }
}

fn require_cargo_deny() -> Result<(), String> {
    let installed = Command::new(cargo_bin())
        .args(["deny", "--version"])
        .output()
        .is_ok_and(|output| output.status.success());
    if installed {
        Ok(())
    } else {
        Err(
            "cargo-deny isn't installed. Install it with `cargo install --locked cargo-deny`."
                .into(),
        )
    }
}

/// The cargo that invoked this task, so the pinned toolchain is used.
fn cargo_bin() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

/// The root of the workspace that `cargo xtask` was run in.
///
/// This asks cargo at run time rather than using the path xtask was compiled
/// at, because a target directory shared between checkouts (a worktree, or a
/// global `CARGO_TARGET_DIR`) can reuse a binary built in another checkout.
fn workspace_root() -> Result<PathBuf, String> {
    let output = Command::new(cargo_bin())
        .args(["locate-project", "--workspace", "--message-format", "plain"])
        .output()
        .map_err(|e| format!("couldn't run cargo locate-project: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    let manifest = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    manifest
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("{} has no parent directory", manifest.display()))
}

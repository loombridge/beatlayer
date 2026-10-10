//! Repository tasks. Run `cargo xtask help` for the list.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use serde_json::Value;

const USAGE: &str = "\
Usage: cargo xtask <task>

Tasks:
  check          Run every check: format, clippy, tests, layers, deny, and the deny self-test
  layers         Check the crate layering rules
  deny-selftest  Prove that deny.toml still rejects an incompatible licence
  rtsan          Run the real-time tests under LLVM's RealtimeSanitizer, with a runtime
                 already on this machine (Homebrew's llvm on macOS, LLVM 20 or later on
                 Linux, or the file RTSAN_LIB_PATH names). Arguments after `--` go to
                 the test binaries; `-- --nocapture` shows the sanitizer's reports.
  help           Show this message
";

/// The packages with RealtimeSanitizer tests (a `tests/rtsan.rs` that needs
/// their `rtsan` feature).
const RTSAN_PACKAGES: &[&str] = &["bl-rt", "bl-audio-io"];

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
    match args.first().map(String::as_str) {
        Some("check") => check(root),
        Some("layers") => layers(root),
        Some("deny-selftest") => deny_selftest(root),
        Some("rtsan") => {
            let passed_on = match args.get(1).map(String::as_str) {
                None => &[][..],
                Some("--") => &args[2..],
                Some(other) => {
                    return Err(format!(
                        "unexpected `{other}`; put test arguments after `--`"
                    ));
                }
            };
            rtsan(root, passed_on)
        }
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

/// Runs each package's RealtimeSanitizer tests with RTSan linked in.
///
/// `rtsan-standalone-sys` links the runtime only when `RTSAN_ENABLE` is set,
/// and takes it from `RTSAN_LIB_PATH` when that is set; otherwise its build
/// script would fetch LLVM's source and build it. So this task always sets
/// both, to a runtime it found on this machine, and stops if there's none.
/// It builds in `target/rtsan`, so switching RTSan on and off doesn't
/// rebuild the normal target directory.
fn rtsan(root: &Path, test_args: &[String]) -> Result<(), String> {
    let runtime = rtsan_runtime()?;
    eprintln!("RealtimeSanitizer runtime: {}", runtime.display());
    let target_dir = root.join("target").join("rtsan");
    let cargo_test = |package: &str, extra: &[&str]| {
        let mut command = Command::new(cargo_bin());
        command
            .args(["test", "--locked", "-p", package, "--features", "rtsan"])
            .args(["--test", "rtsan"])
            .args(extra)
            .env("RTSAN_ENABLE", "1")
            .env("RTSAN_LIB_PATH", &runtime)
            .env("CARGO_TARGET_DIR", &target_dir)
            .current_dir(root);
        command
    };
    for package in RTSAN_PACKAGES {
        eprintln!("\n==> {package}: tests/rtsan.rs under RealtimeSanitizer");
        let built = cargo_test(package, &["--no-run"])
            .status()
            .map_err(|e| format!("couldn't run cargo test: {e}"))?;
        if !built.success() {
            return Err(format!("{package}'s RealtimeSanitizer tests didn't build"));
        }
        if cfg!(target_os = "macos") {
            resign_rtsan_copies(&target_dir)?;
        }
        let passed_on: Vec<&str> = test_args.iter().map(String::as_str).collect();
        let status = cargo_test(package, &[&["--"][..], &passed_on].concat())
            .status()
            .map_err(|e| format!("couldn't run cargo test: {e}"))?;
        if !status.success() {
            return Err(format!("{package}'s RealtimeSanitizer tests failed"));
        }
    }
    Ok(())
}

/// Signs again the copies of the runtime that `rtsan-standalone-sys` keeps in
/// its build output. Its build script changes each copy's install name, which
/// invalidates the copy's signature, and macOS kills any process that loads a
/// library with an invalid signature. An ad hoc signature, which is what
/// Homebrew's copy has, is enough.
fn resign_rtsan_copies(target_dir: &Path) -> Result<(), String> {
    let pattern = format!(
        "{}/debug/build/rtsan-standalone-sys-*/out/libclang_rt.rtsan_osx_dynamic.dylib",
        target_dir.display()
    );
    for copy in expand(&pattern).into_iter().filter(|p| p.is_file()) {
        let status = Command::new("codesign")
            .args(["--force", "--sign", "-"])
            .arg(&copy)
            .output()
            .map_err(|e| format!("couldn't run codesign: {e}"))?;
        if !status.status.success() {
            return Err(format!(
                "couldn't sign {}: {}",
                copy.display(),
                String::from_utf8_lossy(&status.stderr)
            ));
        }
    }
    Ok(())
}

/// The RealtimeSanitizer runtime to link: `RTSAN_LIB_PATH` if set, otherwise
/// the newest one in the usual LLVM install locations.
fn rtsan_runtime() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("RTSAN_LIB_PATH") {
        let path = PathBuf::from(path);
        return if path.is_file() {
            Ok(path)
        } else {
            Err(format!(
                "RTSAN_LIB_PATH names {}, which isn't a file",
                path.display()
            ))
        };
    }
    let mut found: Vec<PathBuf> = rtsan_patterns()
        .iter()
        .flat_map(|pattern| expand(pattern))
        .filter(|path| path.is_file())
        .collect();
    // Version directories sort as text; good enough to prefer 23 over 20.
    found.sort();
    found.pop().ok_or_else(|| {
        let install = if cfg!(target_os = "macos") {
            "brew install llvm"
        } else {
            "install LLVM 20 or later's compiler-rt (for example `libclang-rt-20-dev` from apt.llvm.org)"
        };
        format!(
            "no RealtimeSanitizer runtime found. Install one ({install}), or set RTSAN_LIB_PATH \
             to libclang_rt.rtsan_osx_dynamic.dylib (macOS) or libclang_rt.rtsan*.a (Linux). \
             Beatlayer never downloads it at build time."
        )
    })
}

/// Where LLVM packages put the RTSan runtime, with `*` for a version or
/// target directory.
fn rtsan_patterns() -> Vec<String> {
    let arch = std::env::consts::ARCH;
    if cfg!(target_os = "macos") {
        let mut prefixes = vec![
            "/opt/homebrew/opt/llvm".to_owned(),
            "/usr/local/opt/llvm".to_owned(),
        ];
        if let Ok(output) = Command::new("brew").args(["--prefix", "llvm"]).output()
            && output.status.success()
        {
            prefixes.push(String::from_utf8_lossy(&output.stdout).trim().to_owned());
        }
        prefixes
            .iter()
            .map(|prefix| {
                format!("{prefix}/lib/clang/*/lib/darwin/libclang_rt.rtsan_osx_dynamic.dylib")
            })
            .collect()
    } else {
        vec![
            format!("/usr/lib/llvm-*/lib/clang/*/lib/linux/libclang_rt.rtsan-{arch}.a"),
            format!("/usr/lib/llvm-*/lib/clang/*/lib/{arch}-*-linux-gnu/libclang_rt.rtsan.a"),
            format!("/usr/lib/clang/*/lib/linux/libclang_rt.rtsan-{arch}.a"),
            format!("/usr/lib/clang/*/lib/{arch}-*-linux-gnu/libclang_rt.rtsan.a"),
        ]
    }
}

/// The paths matching `pattern`, where `*` stands for any run of characters
/// within one path component.
fn expand(pattern: &str) -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from("/")];
    for component in pattern.split('/').filter(|c| !c.is_empty()) {
        let mut next = Vec::new();
        for path in &paths {
            if !component.contains('*') {
                next.push(path.join(component));
                continue;
            }
            let Ok(entries) = std::fs::read_dir(path) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                if matches_glob(component, &name.to_string_lossy()) {
                    next.push(entry.path());
                }
            }
        }
        paths = next;
    }
    paths
}

/// Whether `name` matches `pattern`, where `*` matches any run of characters.
fn matches_glob(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            return rest.ends_with(part);
        }
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.is_empty()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_one_path_component() {
        assert!(matches_glob("llvm-*", "llvm-20"));
        assert!(!matches_glob("llvm-*", "clang-20"));
        assert!(matches_glob("x86_64-*-linux-gnu", "x86_64-pc-linux-gnu"));
        assert!(!matches_glob("x86_64-*-linux-gnu", "x86_64-pc-linux-musl"));
        assert!(matches_glob("*", "23"));
        assert!(matches_glob("darwin", "darwin"));
        assert!(!matches_glob("darwin", "darwinx"));
        assert!(matches_glob("a*b*c", "aXbYc"));
        assert!(!matches_glob("a*b*c", "aXcYb"));
    }

    #[test]
    fn expanding_a_pattern_walks_the_directories() {
        let root = std::env::temp_dir().join(format!("xtask-glob-{}", std::process::id()));
        for version in ["20", "23"] {
            let dir = root.join("lib/clang").join(version).join("lib/darwin");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("libclang_rt.rtsan_osx_dynamic.dylib"), b"").unwrap();
        }
        let pattern = format!(
            "{}/lib/clang/*/lib/darwin/libclang_rt.rtsan_osx_dynamic.dylib",
            root.display()
        );
        let mut found = expand(&pattern);
        found.sort();
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(found.len(), 2);
        assert!(found[1].to_string_lossy().contains("/23/"));
    }
}

//! Repository tasks. Run `cargo xtask help` for the list.

mod affected;
mod package;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use serde_json::Value;

mod commands;
mod dashboard;
mod grooves;
mod keymap;
mod private_terms;
mod progress;
mod test_clock;
mod test_targets;
mod ui_parity;
mod wiki;

const USAGE: &str = "\
Usage: cargo xtask <task>

Tasks:
  check          Run every check: format, clippy, tests (cargo-nextest, then the
                 doctests), layers, the UI parity reference, the command reference,
                 the keymap, deny, the deny self-test, and private terms
                 --affected [--base REF]: test only the packages changed since the
                   merge base of REF (default origin/main) and HEAD, committed or
                   not, and the packages that depend on them; the other steps stay
                   workspace-wide, and some changes (Cargo.lock, xtask, CI, ...)
                   still test everything
                 --without-docs: run only the steps that read docs/ (the UI parity
                   reference, the command reference, the keymap) as a checkout
                   without it runs them, without touching docs/
                 BL_TEST_DEPTH=full runs the property, stress, and kill tests at
                 full depth; the default, quick, runs fewer cases of each
  layers         Check the crate layering rules
  ui-parity      Validate the shortcut reference and regenerate its Markdown form
                 (with --check, fail instead of writing if shortcuts.md is out of date)
  commands       Check the command registry and regenerate the command reference
                 (with --check, fail instead of writing if a file of it is out of date)
  keymap         Regenerate crates/shell-spec/keymaps/live12/ from shortcuts.toml and the
                 command registry (with --check, fail instead if it's out of date)
  grooves        Write Beatlayer's bundled grooves into crates/app/resources/Grooves
                 (with --check, fail instead if the files there aren't the script's output)
  deny-selftest  Prove that deny.toml still rejects an incompatible licence
  private-terms  Fail if a tracked file contains a word listed in .git/info/private-terms
                 (maintainers' checkouts only; skipped without it)
  test-clock     Fail if a test waits on the wall clock (thread::sleep or
                 Instant::now) outside xtask/test-clock-allowlist.txt
  golden         Render the golden fixtures and null-test them against their references
                 in tests/golden/ (`cargo test` does too). With --update, remake the
                 references whose renders changed; say why in the pull request. Fixture
                 names after the options pick some; the default is all.
  transcribe-bench  Measure how accurately Convert Melody, Convert Harmony and Convert Drums
                 find notes, on seeded phrases through Drift and Simpler, and breaks through
                 Drum Samplers, clean, with a room and with noise, and fail below the
                 targets. The pitch kinds need --runtime and --model (or BL_ONNX_RUNTIME and
                 BL_PITCH_MODEL): the Packages manager's ONNX Runtime library and
                 basic-pitch model. With --drums only Drums run, and neither is needed.
                 BL_TRANSCRIBE_CORPUS names an open labelled set to report on as well.
  progress       Print the progress of the roadmap, the parity matrix and Reagent's plan
                 as JSON, read from origin/main (with --worktree, from the files on disk)
  dashboard      Serve the progress page on http://localhost:4106 (or PORT, or --port N),
                 reading origin/main and fetching it every two minutes (--worktree reads
                 the files on disk). Localhost starts it from .claude/launch.json
  wiki           Write docs/ as GitHub wiki pages into the given directory, a checkout
                 of the wiki, replacing the pages there (.github/workflows/wiki.yml)
  package        Build this platform's packages with cargo-packager and check they start
  release-check  Fail unless the given tag is v followed by the workspace version
  rtsan          Run the real-time tests in rtsan/ under LLVM's RealtimeSanitizer, with a
                 runtime already on this machine (the file RTSAN_LIB_PATH names, or
                 Homebrew's llvm on macOS, or LLVM 20 or later on Linux). Arguments after
                 `--` go to the test binaries; `-- --nocapture` shows the reports.
  clippy-windows Run clippy for the Windows target (x86_64-pc-windows-msvc)
  help           Show this message
";

/// The RealtimeSanitizer harness, a workspace of its own (see its Cargo.toml).
const RTSAN_DIR: &str = "rtsan";

/// The cargo-nextest release the check is written for, and CI installs. Keep
/// in step with `.config/nextest.toml` and the CI workflows.
const NEXTEST_VERSION: &str = "0.9.146";

/// The checks that read the planning documents in `docs/`.
const DOCS_STEPS: &[&str] = &["ui parity reference", "command reference", "keymap"];

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

/// Audio backends and the crates that link the platforms' audio libraries,
/// which `xtask` must not reach.
const AUDIO_CRATES: &[&str] = &[
    "bl-audio-io",
    "cpal",
    "alsa",
    "alsa-sys",
    "jack",
    "jack-sys",
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = workspace_root().and_then(|root| run(&root, &args));
    match result {
        Ok(code) => code,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(root: &Path, args: &[String]) -> Result<ExitCode, String> {
    let task = args.first().map(String::as_str);
    let options = args.get(1..).unwrap_or_default();
    match task {
        Some("check") => check(root, &check_options(options)?).map(|()| ExitCode::SUCCESS),
        Some("layers") => layers(root).map(|()| ExitCode::SUCCESS),
        Some("ui-parity") => match options {
            [] => ui_parity::generate(root).map(|()| ExitCode::SUCCESS),
            [flag] if flag == "--check" => ui_parity::check(root).map(|()| ExitCode::SUCCESS),
            _ => Err(format!("usage: cargo xtask ui-parity [--check]\n\n{USAGE}")),
        },
        Some("commands") => match options {
            [] => commands::generate(root).map(|()| ExitCode::SUCCESS),
            [flag] if flag == "--check" => commands::check(root).map(|()| ExitCode::SUCCESS),
            _ => Err(format!("usage: cargo xtask commands [--check]\n\n{USAGE}")),
        },
        Some("keymap") => match options {
            [] => keymap::generate(root).map(|()| ExitCode::SUCCESS),
            [flag] if flag == "--check" => keymap::check(root).map(|()| ExitCode::SUCCESS),
            _ => Err(format!("usage: cargo xtask keymap [--check]\n\n{USAGE}")),
        },
        Some("grooves") => match options {
            [] => grooves::generate(root).map(|()| ExitCode::SUCCESS),
            [flag] if flag == "--check" => grooves::check(root).map(|()| ExitCode::SUCCESS),
            _ => Err(format!("usage: cargo xtask grooves [--check]\n\n{USAGE}")),
        },
        Some("deny-selftest") => deny_selftest(root).map(|()| ExitCode::SUCCESS),
        Some("private-terms") => private_terms::check(root).map(|()| ExitCode::SUCCESS),
        Some("test-clock") => test_clock::check(root).map(|()| ExitCode::SUCCESS),
        Some("golden") => golden(root, options).map(|()| ExitCode::SUCCESS),
        Some("transcribe-bench") => transcribe_bench(root, options).map(|()| ExitCode::SUCCESS),
        Some("progress") => dashboard::print(root, options).map(|()| ExitCode::SUCCESS),
        Some("dashboard") => dashboard::serve(root, options).map(|()| ExitCode::SUCCESS),
        Some("wiki") => match options {
            [out] => wiki::write(root, Path::new(out)).map(|()| ExitCode::SUCCESS),
            _ => Err(format!("usage: cargo xtask wiki <dir>\n\n{USAGE}")),
        },
        Some("package") => package::package(root).map(|()| ExitCode::SUCCESS),
        Some("release-check") => match options {
            [tag] => package::release_check(root, tag).map(|()| ExitCode::SUCCESS),
            _ => Err(format!("usage: cargo xtask release-check <tag>\n\n{USAGE}")),
        },
        Some("rtsan") => {
            let passed_on = match options.first().map(String::as_str) {
                None => &[][..],
                Some("--") => &options[1..],
                Some(other) => {
                    return Err(format!(
                        "unexpected `{other}`; put test arguments after `--`"
                    ));
                }
            };
            rtsan(root, passed_on).map(|()| ExitCode::SUCCESS)
        }
        Some("clippy-windows") => match options {
            [] => clippy_windows(root),
            _ => Err(format!("usage: cargo xtask clippy-windows\n\n{USAGE}")),
        },
        None | Some("help" | "-h" | "--help") => {
            print!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        Some(other) => Err(format!("unknown task `{other}`\n\n{USAGE}")),
    }
}

/// What `cargo xtask check` was asked to do.
#[derive(Debug, Default, PartialEq, Eq)]
struct CheckOptions {
    /// Test only the packages changed since the merge base of this ref and
    /// `HEAD`, and their dependents.
    affected: Option<String>,
    /// Run only the steps that read `docs/`, as a checkout without it does.
    without_docs: bool,
}

fn check_options(options: &[String]) -> Result<CheckOptions, String> {
    let usage = "usage: cargo xtask check [--affected [--base REF] | --without-docs]";
    let mut affected = false;
    let mut base = None;
    let mut without_docs = false;
    let mut options = options.iter();
    while let Some(option) = options.next() {
        match option.as_str() {
            "--affected" => affected = true,
            "--base" => {
                let value = options
                    .next()
                    .ok_or_else(|| format!("--base needs a ref\n{usage}"))?;
                base = Some(value.clone());
            }
            "--without-docs" => without_docs = true,
            other => match other.strip_prefix("--base=") {
                Some(value) => base = Some(value.to_owned()),
                None => return Err(format!("unexpected `{other}`\n{usage}")),
            },
        }
    }
    if base.is_some() && !affected {
        return Err(format!("--base goes with --affected\n{usage}"));
    }
    if without_docs && affected {
        return Err(format!(
            "--without-docs runs only the steps that read docs/, which --affected doesn't \
             narrow; run the two separately\n{usage}"
        ));
    }
    Ok(CheckOptions {
        affected: affected.then(|| base.unwrap_or_else(|| "origin/main".to_owned())),
        without_docs,
    })
}

/// How deep the property, stress, and kill tests go: `BL_TEST_DEPTH`, quick
/// unless it says full. The tests read it themselves; this catches a typo
/// before anything runs.
fn test_depth() -> Result<&'static str, String> {
    match std::env::var("BL_TEST_DEPTH").as_deref() {
        Err(_) | Ok("" | "quick") => Ok("quick"),
        Ok("full") => Ok("full"),
        Ok(other) => Err(format!(
            "BL_TEST_DEPTH is `{other}`: set it to quick or full, or leave it unset for quick"
        )),
    }
}

/// Runs every check, reports each result, and fails if any failed.
fn check(root: &Path, options: &CheckOptions) -> Result<(), String> {
    if !options.without_docs {
        require_cargo_deny()?;
        require_cargo_nextest()?;
    }
    let depth = test_depth()?;
    let workspace = affected::Workspace::from_metadata(&cargo_metadata(root)?)?;
    let total = workspace.packages.len();
    // Without --affected, every package's tests run.
    let tests = options.affected.as_deref().map(|base| {
        eprintln!("\n==> affected packages");
        let tests = affected::select(root, &workspace, base);
        eprintln!("{}", describe(&tests, total));
        tests
    });
    let tested = match &tests {
        Some(affected::Tests::Some { tested, .. }) => Some(tested),
        None | Some(affected::Tests::All(_)) => None,
    };

    // Without the planning documents, or with --without-docs, the checks that
    // read the UI parity reference skip, and the command reference checks
    // the registry alone.
    let reference = !options.without_docs && ui_parity::reference_present(root);
    let mut skipped: Vec<&str> = if reference {
        vec![]
    } else {
        vec!["ui parity reference", "keymap"]
    };
    if tested.is_some_and(BTreeSet::is_empty) {
        skipped.push("tests");
    }
    let partial: &[&str] = if reference {
        &[]
    } else {
        &["command reference"]
    };
    type Step<'a> = (&'a str, Box<dyn Fn() -> Result<(), String> + 'a>);
    let mut steps: Vec<Step<'_>> = vec![
        (
            "format",
            Box::new(|| {
                cargo(root, &["fmt", "--all", "--check"])?;
                // The RealtimeSanitizer harness is a workspace of its own.
                cargo(root.join(RTSAN_DIR).as_path(), &["fmt", "--check"])
            }),
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
        ("test clock", Box::new(|| test_clock::check(root))),
        ("tests", Box::new(|| run_tests(root, tested, depth))),
        ("layers", Box::new(|| layers(root))),
        ("ui parity reference", Box::new(|| ui_parity::check(root))),
        (
            "command reference",
            Box::new(move || {
                if reference {
                    commands::check(root)
                } else {
                    commands::check_registry()
                }
            }),
        ),
        ("keymap", Box::new(|| keymap::check(root))),
        (
            "one test binary per crate",
            Box::new(|| test_targets::check(root)),
        ),
        (
            "licences, advisories, bans, sources",
            Box::new(|| cargo(root, &["deny", "--locked", "check"])),
        ),
        ("deny self-test", Box::new(|| deny_selftest(root))),
        ("private terms", Box::new(|| private_terms::check(root))),
    ];
    if options.without_docs {
        steps.retain(|(name, _)| DOCS_STEPS.contains(name));
    }

    let mut failed = Vec::new();
    for (name, step) in &steps {
        eprintln!("\n==> {name}");
        if skipped.contains(name) {
            if *name == "tests" {
                eprintln!("skipped: no package changed");
            } else {
                eprintln!("{}", ui_parity::ABSENT);
            }
            continue;
        }
        if let Err(message) = step() {
            eprintln!("{message}");
            failed.push(*name);
        }
    }

    eprintln!("\n==> summary");
    for (name, _) in &steps {
        let status = if failed.contains(name) {
            "FAILED"
        } else if skipped.contains(name) {
            "skip"
        } else if partial.contains(name) {
            "part"
        } else {
            "ok"
        };
        eprintln!("{status:>6}  {name}");
    }
    if let Some(tests) = &tests {
        eprintln!("\nTests (--affected): {}", describe(tests, total));
    }
    if options.without_docs {
        eprintln!(
            "\n--without-docs ran the checks that read docs/ as a checkout without it runs \
             them (skip, part), and left docs/ alone."
        );
    } else if !reference {
        eprintln!(
            "\nThe UI parity reference isn't published yet, so this checkout skipped what \
             reads it (skip, part). A maintainer's checkout runs every check."
        );
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("{} of {} checks failed", failed.len(), steps.len()))
    }
}

/// One line on what the tests step tests and why, for example `bl-ui changed
/// → testing bl-ui, beatlayer (2 of 25)`.
fn describe(tests: &affected::Tests, total: usize) -> String {
    let list = |names: &BTreeSet<String>| {
        names
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    };
    match tests {
        affected::Tests::All(why) => {
            format!("{why} → testing every package ({total} of {total})")
        }
        affected::Tests::Some { tested, .. } if tested.is_empty() => {
            format!("only prose changed → testing nothing (0 of {total})")
        }
        affected::Tests::Some { changed, tested } => format!(
            "{} changed → testing {} ({} of {total})",
            list(changed),
            list(tested),
            tested.len()
        ),
    }
}

/// Runs the tests with cargo-nextest, each in a process of its own and as
/// many at once as there are cores, then the doctests, which nextest doesn't
/// run, with `cargo test --doc`. `tested` names the packages whose tests run,
/// or is `None` for every package.
///
/// Both always build the whole workspace, and a selection only filters what
/// runs. Cargo unifies features across the packages it's asked to build, so
/// building only some (`--package`) would build their dependencies with
/// other features: code the full run never tests, and, for a change to
/// bl-ui alone, some 65 crates (wgpu and egui among them) built a second
/// time. Built as a whole, everything that hasn't changed is reused.
fn run_tests(root: &Path, tested: Option<&BTreeSet<String>>, depth: &str) -> Result<(), String> {
    eprintln!("test depth: {depth} (BL_TEST_DEPTH)");
    let mut nextest = vec![
        "nextest".to_owned(),
        "run".to_owned(),
        "--workspace".to_owned(),
        "--all-features".to_owned(),
        "--locked".to_owned(),
    ];
    if let Some(tested) = tested {
        let filter = tested
            .iter()
            .map(|name| format!("package(={name})"))
            .collect::<Vec<_>>()
            .join(" | ");
        // A package with no tests of its own is no failure.
        nextest.extend(["--no-tests=pass".to_owned(), "-E".to_owned(), filter]);
    }
    let nextest: Vec<&str> = nextest.iter().map(String::as_str).collect();
    // Both run, so one run reports every failure. The doctests are few and
    // quick, so they all run.
    let tests = cargo(root, &nextest);
    let docs = cargo(
        root,
        &["test", "--doc", "--workspace", "--all-features", "--locked"],
    );
    tests.and(docs)
}

/// Enforces the layering in the architecture notes:
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
            // Every task builds xtask, so it links no audio backend: on
            // Linux that would need ALSA and JACK headers for any task.
            if let Some(path) = graph.path_to(member, |dep| AUDIO_CRATES.contains(&dep)) {
                violations.push(format!("xtask links an audio backend: {path}"));
            }
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
            "layering rules broken:\n  {}",
            violations.join("\n  ")
        ))
    }
}

/// Runs `bl-cli golden` on tests/golden/, passing `--update` and fixture
/// names through. References are remade only when someone asks, here or
/// with `bl-cli golden --update`, never in a test.
fn golden(root: &Path, options: &[String]) -> Result<(), String> {
    if let Some(flag) = options
        .iter()
        .find(|option| option.starts_with('-') && *option != "--update")
    {
        return Err(format!(
            "unexpected `{flag}`; usage: cargo xtask golden [--update] [FIXTURE...]"
        ));
    }
    let dir = root.join("tests").join("golden");
    let dir = dir
        .to_str()
        .ok_or("the repository's path isn't valid UTF-8")?;
    let mut args = vec![
        "run",
        "--quiet",
        "--locked",
        "--package",
        "bl-cli",
        "--",
        "golden",
        "--dir",
        dir,
    ];
    args.extend(options.iter().map(String::as_str));
    cargo(root, &args)
}

/// Runs `bl-cli transcribe-bench`, passing its options on.
fn transcribe_bench(root: &Path, options: &[String]) -> Result<(), String> {
    let mut args = vec![
        "run",
        "--quiet",
        "--locked",
        "--package",
        "bl-cli",
        "--",
        "transcribe-bench",
    ];
    args.extend(options.iter().map(String::as_str));
    cargo(root, &args)
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

/// Runs the RealtimeSanitizer harness in `rtsan/`, a workspace of its own, with
/// a runtime already on this machine.
///
/// The harness's build script links the runtime that `RTSAN_LIB_PATH` names
/// and never fetches one, so this task finds one first and stops with
/// instructions if there's none. The harness has no lockfile of its own: the
/// task copies the main workspace's, so the harness builds exactly the
/// versions the main workspace locked and `cargo deny` checked, and checks
/// that afterwards. It builds in `target/rtsan`, with `--cfg lspk_rtsan` from
/// `rtsan/.cargo/config.toml`, so the main target directory isn't rebuilt.
fn rtsan(root: &Path, test_args: &[String]) -> Result<(), String> {
    let runtime = rtsan_runtime()?;
    eprintln!("RealtimeSanitizer runtime: {}", runtime.display());
    let options = rtsan_options(&runtime);
    if let Some(options) = &options {
        eprintln!("RTSAN_OPTIONS={options}");
    }
    let harness = root.join(RTSAN_DIR);
    let lockfile = harness.join("Cargo.lock");
    std::fs::copy(root.join("Cargo.lock"), &lockfile)
        .map_err(|e| format!("couldn't copy Cargo.lock into {RTSAN_DIR}/: {e}"))?;
    let cargo_in_harness = |args: &[&str]| {
        let mut command = Command::new(cargo_bin());
        command
            .args(args)
            .env("RTSAN_LIB_PATH", &runtime)
            .env("CARGO_TARGET_DIR", root.join("target").join("rtsan"))
            // RUSTFLAGS would replace the harness's `--cfg lspk_rtsan`.
            .env_remove("RUSTFLAGS")
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .current_dir(&harness);
        if let Some(options) = &options {
            command.env("RTSAN_OPTIONS", options);
        }
        command
    };
    let run = |what: &str, args: &[&str]| -> Result<(), String> {
        eprintln!("\n==> {what}");
        let mut command = cargo_in_harness(args);
        if args.first() == Some(&"metadata") {
            command.stdout(Stdio::null());
        }
        let status = command
            .status()
            .map_err(|e| format!("couldn't run cargo: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("{what} failed"))
        }
    };

    run(
        "rtsan/: resolve with the main workspace's lockfile",
        &["metadata", "--format-version", "1"],
    )?;
    same_versions(&root.join("Cargo.lock"), &lockfile)?;
    run(
        "rtsan/: clippy",
        &["clippy", "--all-targets", "--", "-D", "warnings"],
    )?;
    // Every test binary runs even if one fails, so a run shows all failures.
    let mut test = vec!["test", "--no-fail-fast", "--"];
    test.extend(test_args.iter().map(String::as_str));
    run("rtsan/: the real-time tests under RealtimeSanitizer", &test)
}

/// RTSAN_OPTIONS for the tests: the caller's, plus, on Linux, the path to the
/// `llvm-symbolizer` that came with the runtime, so reports name functions
/// (macOS uses `atos`). `None` if there's nothing to set.
fn rtsan_options(runtime: &Path) -> Option<String> {
    let caller = std::env::var("RTSAN_OPTIONS")
        .ok()
        .filter(|o| !o.is_empty());
    let symbolizer = (cfg!(target_os = "linux")
        && !caller
            .as_deref()
            .is_some_and(|o| o.contains("external_symbolizer_path")))
    .then(|| symbolizer_beside(runtime))
    .flatten()
    .map(|path| format!("external_symbolizer_path={}", path.display()));
    match (caller, symbolizer) {
        (Some(caller), Some(symbolizer)) => Some(format!("{caller}:{symbolizer}")),
        (caller, symbolizer) => caller.or(symbolizer),
    }
}

/// The `llvm-symbolizer` of the LLVM install a runtime belongs to: the
/// runtime sits in `<llvm>/lib/clang/<version>/lib/<target>/`, and the
/// symbolizer in `<llvm>/bin/`. Only that install's symbolizer fits the
/// runtime; a `bin/llvm-symbolizer` further up the tree is another LLVM's.
fn symbolizer_beside(runtime: &Path) -> Option<PathBuf> {
    // The LLVM root is the runtime's sixth ancestor: its name plus
    // `<target>`, `lib`, `<version>`, `clang`, and `lib`.
    let llvm = runtime.ancestors().nth(6)?;
    let symbolizer = llvm.join("bin").join("llvm-symbolizer");
    (symbolizer.is_file()).then_some(symbolizer)
}

/// Fails unless every package `harness_lock` resolves is in `main_lock` at
/// the same version, apart from the harness itself.
fn same_versions(main_lock: &Path, harness_lock: &Path) -> Result<(), String> {
    let read = |path: &Path| {
        std::fs::read_to_string(path).map_err(|e| format!("couldn't read {}: {e}", path.display()))
    };
    let main = locked_packages(&read(main_lock)?);
    let drifted: Vec<String> = locked_packages(&read(harness_lock)?)
        .into_iter()
        .filter(|(name, _)| name != "bl-rtsan")
        .filter(|package| !main.contains(package))
        .map(|(name, version)| format!("{name} {version}"))
        .collect();
    if drifted.is_empty() {
        eprintln!("the harness builds only versions the main Cargo.lock has");
        Ok(())
    } else {
        Err(format!(
            "the harness resolved versions the main Cargo.lock doesn't have: {}",
            drifted.join(", ")
        ))
    }
}

/// The `(name, version)` of every package in a Cargo.lock.
fn locked_packages(lockfile: &str) -> BTreeSet<(String, String)> {
    let value = |line: &str, key: &str| {
        line.strip_prefix(key)
            .and_then(|rest| rest.strip_prefix(" = \""))
            .and_then(|rest| rest.strip_suffix('"'))
            .map(str::to_owned)
    };
    let mut packages = BTreeSet::new();
    let mut name = None;
    for line in lockfile.lines() {
        if line == "[[package]]" {
            name = None;
        } else if let Some(found) = value(line, "name") {
            name = Some(found);
        } else if let (Some(version), Some(found)) = (value(line, "version"), name.take()) {
            packages.insert((found, version));
        }
    }
    packages
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

fn require_cargo_nextest() -> Result<(), String> {
    let installed = Command::new(cargo_bin())
        .args(["nextest", "--version"])
        .output()
        .is_ok_and(|output| output.status.success());
    if installed {
        Ok(())
    } else {
        Err(format!(
            "cargo-nextest isn't installed, and the check runs the tests with it. Install it \
             with `cargo install --locked cargo-nextest@{NEXTEST_VERSION}`."
        ))
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

const WINDOWS_TARGET: &str = "x86_64-pc-windows-msvc";

fn is_target_installed(installed_targets: &str, target: &str) -> bool {
    installed_targets.lines().any(|line| line.trim() == target)
}

fn check_target_installed(installed_targets: &str, target: &str) -> Result<(), String> {
    if is_target_installed(installed_targets, target) {
        Ok(())
    } else {
        Err(format!(
            "the `{target}` target isn't installed. Install it with `rustup target add {target}`."
        ))
    }
}

fn require_windows_target() -> Result<(), String> {
    let output = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .map_err(|e| format!("couldn't run `rustup target list --installed`: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "`rustup target list --installed` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    check_target_installed(&stdout, WINDOWS_TARGET)
}

fn clippy_windows(root: &Path) -> Result<ExitCode, String> {
    require_windows_target()?;
    let status = Command::new(cargo_bin())
        .env("CARGO_FEATURE_PURE", "1")
        .args([
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--locked",
            "--target",
            WINDOWS_TARGET,
            "--",
            "-D",
            "warnings",
        ])
        .current_dir(root)
        .status()
        .map_err(|e| format!("couldn't run cargo clippy: {e}"))?;
    Ok(status
        .code()
        .map_or(ExitCode::FAILURE, |c| ExitCode::from(c as u8)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_options_parse() {
        let parse =
            |args: &[&str]| check_options(&args.iter().map(|&a| a.to_owned()).collect::<Vec<_>>());
        assert_eq!(parse(&[]), Ok(CheckOptions::default()));
        let affected = |base: &str| CheckOptions {
            affected: Some(base.to_owned()),
            without_docs: false,
        };
        assert_eq!(parse(&["--affected"]), Ok(affected("origin/main")));
        assert_eq!(
            parse(&["--affected", "--base", "origin/wp"]),
            Ok(affected("origin/wp"))
        );
        assert_eq!(
            parse(&["--base=upstream/main", "--affected"]),
            Ok(affected("upstream/main"))
        );
        assert_eq!(
            parse(&["--without-docs"]),
            Ok(CheckOptions {
                affected: None,
                without_docs: true,
            })
        );
        for bad in [
            &["--base", "origin/main"][..],
            &["--affected", "--base"],
            &["--affected", "--without-docs"],
            &["--everything"],
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }

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
    fn the_symbolizer_is_found_beside_the_runtime() {
        let tmp = std::env::temp_dir().join(format!("xtask-symbolizer-{}", std::process::id()));
        let llvm = tmp.join("llvm");
        let runtime_dir = llvm.join("lib/clang/20/lib/linux");
        std::fs::create_dir_all(&runtime_dir).unwrap();
        // The nearest `bin/llvm-symbolizer` above the install (as in `/bin`
        // when the install sits at `/`) is another LLVM's; it must be ignored.
        let above = tmp.join("bin/llvm-symbolizer");
        std::fs::create_dir_all(above.parent().unwrap()).unwrap();
        std::fs::write(&above, b"").unwrap();
        let runtime = runtime_dir.join("libclang_rt.rtsan-x86_64.a");
        std::fs::write(&runtime, b"").unwrap();
        let missing = symbolizer_beside(&runtime);
        std::fs::create_dir_all(llvm.join("bin")).unwrap();
        std::fs::write(llvm.join("bin/llvm-symbolizer"), b"").unwrap();
        let found = symbolizer_beside(&runtime);
        std::fs::remove_dir_all(&tmp).unwrap();
        assert_eq!(missing, None);
        assert_eq!(found, Some(llvm.join("bin").join("llvm-symbolizer")));
    }

    #[test]
    fn lockfiles_list_their_packages_with_versions() {
        let lockfile = "\
version = 4

[[package]]
name = \"bl-rt\"
version = \"0.0.0\"
dependencies = [
 \"rtrb\",
]

[[package]]
name = \"rtrb\"
version = \"0.4.0\"
source = \"registry+https://github.com/rust-lang/crates.io-index\"
";
        let packages = locked_packages(lockfile);
        assert_eq!(
            packages.into_iter().collect::<Vec<_>>(),
            [
                ("bl-rt".to_owned(), "0.0.0".to_owned()),
                ("rtrb".to_owned(), "0.4.0".to_owned())
            ]
        );
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
        assert!(
            found[1].components().any(|part| part.as_os_str() == "23"),
            "{found:?}"
        );
    }

    #[test]
    fn windows_target_check() {
        let installed = "aarch64-apple-darwin\nx86_64-apple-darwin\nx86_64-pc-windows-msvc\n";
        assert!(check_target_installed(installed, WINDOWS_TARGET).is_ok());

        let missing = "aarch64-apple-darwin\nx86_64-apple-darwin\n";
        let err = check_target_installed(missing, WINDOWS_TARGET).unwrap_err();
        assert!(
            err.contains("rustup target add x86_64-pc-windows-msvc"),
            "unexpected error message: {err}"
        );

        let crlf = "aarch64-apple-darwin\r\nx86_64-pc-windows-msvc \r\n";
        assert!(check_target_installed(crlf, WINDOWS_TARGET).is_ok());

        let substring = "x86_64-pc-windows-msvc-extra\n";
        assert!(check_target_installed(substring, WINDOWS_TARGET).is_err());
    }
}

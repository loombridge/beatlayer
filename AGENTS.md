# AGENTS.md

Shared context for any agent or person working in this repository. Keep it current and short; private notes belong in your own memory, not here.

## What this is

Beatlayer is an open-source digital audio workstation with the workflow of Ableton Live 12, written in Rust, for macOS (Apple Silicon), Windows, and Linux.

## Read first

1. [docs/01-roadmap.md](docs/01-roadmap.md): milestones, scope, risks.
2. [docs/02-architecture.md](docs/02-architecture.md): document, engine, UI, crates, decisions log.
3. [docs/03-build-plan.md](docs/03-build-plan.md): work packages, acceptance criteria, and the status table. "How an agent uses this plan" is the procedure for every package.
4. [Building without Ableton's code](CONTRIBUTING.md#building-without-abletons-code), in CONTRIBUTING.md: what you may and may not learn from Ableton Live.
5. [docs/04-parity-matrix.md](docs/04-parity-matrix.md): every Live 12 feature and the milestone that covers it.
6. [docs/05-improvements.md](docs/05-improvements.md): improvements over Live, and the rules for adding them (add, don't move).
7. [docs/ui-parity/](docs/ui-parity/) (from WP-0.13): where everything sits in Live 12's interface, and every shortcut. Read it before any UI work.

## Lines not crossed

- No Ableton code, content, or trademarks, by any route.
- Every user-facing action is a registered command with a schema, a description, and a permission class, so agents reach every feature through the agent API (decision D17).
- The UI matches Live 12's placement, menus, and key bindings ([docs/ui-parity/](docs/ui-parity/), decision D16), with Beatlayer's own icons, typeface, and colours. A deliberate difference needs a decisions-log entry.
- No allocation, lock, or blocking call on an audio thread.
- Engine, model, and device crates never depend on a UI crate. `cargo xtask layers` enforces this.
- Every feature has a headless test path.
- No dependency whose licence `cargo deny` rejects. No plug-in binaries or third-party audio in the repo. `spikes/`, `fuzz/`, and `rtsan/` are outside the workspace and exempt from `deny.toml`, because nothing there ships.
- No `git add -A`.

## How work is done

One work package per branch (`wp/<id>-<slug>`) and per pull request, with acceptance evidence pasted in the PR. "How an agent uses this plan" in [the build plan](docs/03-build-plan.md) is the procedure.

`cargo xtask check` must pass on macOS locally, with its summary pasted in the PR. CI runs Linux per pull request, Windows on every merge to main, and macOS nightly, so the local macOS run is the only macOS check a change gets before it merges. A Windows failure on main is fixed forward; for a Windows-specific change, run `gh workflow run ci.yml --ref <branch>` before asking for a merge. Compile-check Windows locally with clippy for `x86_64-pc-windows-msvc`.

Commits: the project's own commits (maintainers' work packages) are authored as `Loombridge <code@loombridge.io>`, never a person's name. Set it once per checkout: `git config user.name Loombridge && git config user.email code@loombridge.io`. Outside contributors commit under their own names with the DCO ([CONTRIBUTING.md](CONTRIBUTING.md)).

Progress: `cargo xtask dashboard` serves a page of the roadmap and the parity matrix as grids of squares, read from `origin/main`; `cargo xtask progress` prints the same as JSON. Each parity row names the work packages that build it ("Built in WP-1.8a"), and a test fails if it names one the build plan doesn't have. The dashboard's port is 4106 (`.claude/launch.json`); the app itself is native and has no dev server.

Merging: a maintainer merges a pull request when `cargo xtask check` and CI pass, review finds no blocker, and the acceptance evidence is complete. A criterion that's pending only on hardware or on a maintainer task doesn't block a merge; the package's PR says what's still pending.

One-time setup: install Rust with rustup (the repo pins the toolchain in `rust-toolchain.toml`), then `cargo install --locked cargo-deny`.

# AGENTS.md

Shared context for any agent or person working in this repository. Keep it current and short; private notes belong in your own memory, not here.

## What this is

Beatlayer is an open-source digital audio workstation with the workflow of Ableton Live 12, written in Rust, for macOS (Apple Silicon), Windows, and Linux.

## Read first

1. [docs/01-roadmap.md](docs/01-roadmap.md): milestones, scope, risks.
2. [docs/02-architecture.md](docs/02-architecture.md): document, engine, UI, crates, decisions log.
3. [docs/03-build-plan.md](docs/03-build-plan.md): work packages, human tasks, and the status table. "How an agent uses this plan" is the procedure for every package.
4. [CONTRIBUTING.md ("Building without Ableton's code")](https://github.com/loombridge/beatlayer/blob/main/CONTRIBUTING.md#building-without-abletons-code): what you may and may not learn from Ableton Live.
5. [docs/04-parity-matrix.md](docs/04-parity-matrix.md): every Live 12 feature and the milestone that covers it.

## Lines not crossed

- No Ableton code, content, or trademarks, by any route.
- No allocation, lock, or blocking call on an audio thread.
- Engine, model, and device crates never depend on a UI crate. `cargo xtask layers` enforces this.
- Every feature has a headless test path.
- No dependency whose licence `cargo deny` rejects. No plug-in binaries or third-party audio in the repo.
- No `git add -A`.

## How work is done

One work package per branch (`wp/<id>-<slug>`) and per pull request, with acceptance evidence pasted in the PR. `cargo xtask check` must pass. The maintainer merges.

One-time setup: install Rust with rustup (the repo pins the toolchain in `rust-toolchain.toml`), then `cargo install --locked cargo-deny`.

## Where things stand (10 October 2026)

WP-0.1 (workspace and tooling) is in review on branch `wp/0.1-workspace-tooling`. Every crate is a stub. Next: the local Phase 0 packages (WP-0.5, 0.6, 0.7, the UI spike, 0.12, and WP-0.3 on macOS).

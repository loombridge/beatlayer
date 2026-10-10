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
6. [docs/05-improvements.md](docs/05-improvements.md): improvements over Live, and the rules for adding them (add, don't move).
7. [docs/ui-parity/](docs/ui-parity/) (from WP-0.13): where everything sits in Live 12's interface, and every shortcut. Read it before any UI work.

## Lines not crossed

- No Ableton code, content, or trademarks, by any route.
- Every user-facing action is a registered command with a schema, a description, and a permission class, so agents reach every feature through the agent API (decision D17).
- The UI matches Live 12's placement, menus, and key bindings ([docs/ui-parity/](docs/ui-parity/), decision D16), with Beatlayer's own icons, typeface, and colours. A deliberate difference needs a decisions-log entry.
- No allocation, lock, or blocking call on an audio thread.
- Engine, model, and device crates never depend on a UI crate. `cargo xtask layers` enforces this.
- Every feature has a headless test path.
- No dependency whose licence `cargo deny` rejects. No plug-in binaries or third-party audio in the repo.
- No `git add -A`.

## How work is done

One work package per branch (`wp/<id>-<slug>`) and per pull request, with acceptance evidence pasted in the PR. `cargo xtask check` must pass.

Merging: on 10 October 2026 the maintainer delegated merging to the orchestrating agent, to keep building phase after phase. That agent merges a pull request when `cargo xtask check` and CI pass, the integration review finds no blocker, and the acceptance evidence is complete. A criterion that's pending only on hardware or on the maintainer's judgement doesn't block a merge; it stays listed under the human tasks. Decisions marked for the maintainer in the build plan (such as H8, the UI framework) still go to the maintainer.

One-time setup: install Rust with rustup (the repo pins the toolchain in `rust-toolchain.toml`), then `cargo install --locked cargo-deny`.

## Where things stand (10 October 2026)

Phase 0. Merged: WP-0.1 (workspace and tooling) and WP-0.2 (CI on macOS, Windows, and Linux). The UI toolkit is egui ([ADR-001](docs/adr/ADR-001-ui-toolkit-egui.md)). In review: WP-0.3, 0.4, 0.5, 0.6, 0.8a, 0.8b, 0.12, and 0.13. Next: WP-0.7, 0.9, 0.11, and 0.14. The build plan's status table is the live queue.

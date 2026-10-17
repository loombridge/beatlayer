# AGENTS.md

Shared context for any agent or person working in this repository. Keep it current and short; private notes belong in your own memory, not here.

## What this is

Beatlayer is an open-source digital audio workstation with the workflow of Ableton Live 12, written in Rust, for macOS (Apple Silicon), Windows, and Linux. A Live user should find every view, panel, menu, and shortcut where Live puts them, with Beatlayer's own icons, typeface, and colours.

## Read first

1. [README.md](README.md): what Beatlayer is and how to build it.
2. [CONTRIBUTING.md](CONTRIBUTING.md), and in it [Building without Ableton's code](CONTRIBUTING.md#building-without-abletons-code): what you may and may not learn from Ableton Live. Read it before you write anything.

The roadmap, architecture, build plan, and parity plan aren't published yet; a maintainer gives you the work package and the context it needs.

## Lines not crossed

- No Ableton code, content, or trademarks, by any route.
- Every user-facing action is a registered command with a schema, a description, and a permission class, so agents reach every feature through the agent API.
- Anything an agent can set, it can read back: every command that isn't `read` names, in the readback table beside the registry, the read command or query field that shows its effect, or says why there is nothing to read. `cargo xtask commands` fails on one that doesn't.
- The UI matches Live 12's placement, menus, and key bindings, with Beatlayer's own icons, typeface, and colours.
- No allocation, lock, or blocking call on an audio thread.
- Engine, model, and device crates never depend on a UI crate. `cargo xtask layers` enforces this.
- Every feature has a headless test path.
- No dependency whose licence `cargo deny` rejects. No plug-in binaries or third-party audio in the repo. `spikes/`, `fuzz/`, and `rtsan/` are outside the workspace and exempt from `deny.toml`, because nothing there ships.
- No `git add -A`.

## How work is done

One work package per branch (`wp/<id>-<slug>`) and per pull request, with acceptance evidence pasted in the PR.

Iterate with `cargo xtask check --affected`, which tests only the crates your branch changes and their dependents. Before a PR, it must pass on macOS locally, with its summary pasted in the PR. That is the only local check: CI builds and tests Windows and Linux on every pull request, and a full macOS check runs every night, which catches anything that breaks outside the crates a change touched. A nightly failure is fixed in a follow-up pull request. `cargo xtask check` and `cargo xtask clippy-windows` remain available when you want the whole picture before CI.

Commits: the project's own commits are authored as `Loombridge <code@loombridge.io>`. Set it once per checkout: `git config user.name Loombridge && git config user.email code@loombridge.io`. Outside contributors commit under their own names with the Developer Certificate of Origin ([CONTRIBUTING.md](CONTRIBUTING.md)).

Merging: a maintainer merges a pull request when `cargo xtask check` and CI pass, review finds no blocker, and the acceptance evidence is complete.

One-time setup: install Rust with rustup (the repo pins the toolchain in `rust-toolchain.toml`), then `cargo install --locked cargo-deny` and `cargo install --locked cargo-nextest@0.9.146`.

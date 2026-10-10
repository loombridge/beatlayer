# Beatlayer

Beatlayer is an open-source digital audio workstation with the workflow of Ableton Live 12: Session and Arrangement views, warping, racks, and a full set of instruments and effects. It's written in Rust and runs natively on macOS (Apple Silicon), Windows, and Linux.

Beatlayer is in Phase 0 of its [build plan](docs/03-build-plan.md). The workspace builds, but every crate is still a stub.

## Documents

| Document | What it covers |
|---|---|
| [Roadmap](docs/01-roadmap.md) | Principles, milestones from M0 to 1.0, scope, size, and risks |
| [Architecture](docs/02-architecture.md) | The document, engine, and UI split; crates; threads; real-time rules; testing; packaging; the decisions log |
| [Build plan](docs/03-build-plan.md) | Work packages, dependencies, acceptance criteria, human tasks, and the status table |
| [Parity matrix](docs/04-parity-matrix.md) | Every Ableton Live 12 feature and the milestone that covers it |
| [Clean-room policy](https://github.com/loombridge/beatlayer/blob/main/CONTRIBUTING.md#building-without-abletons-code) | How Beatlayer learns from Live without using its code or content |

## Develop

1. Install Rust with [rustup](https://rustup.rs). The repo pins the toolchain, so rustup fetches the right version on first build.
1. Install cargo-deny, which checks dependency licences and advisories:

   ```sh
   cargo install --locked cargo-deny
   ```

1. Run every check: format, clippy, tests, crate layering, and the dependency policy.

   ```sh
   cargo xtask check
   ```

Read [CONTRIBUTING.md](CONTRIBUTING.md) and [AGENTS.md](AGENTS.md) before contributing. Every contribution follows the [clean-room policy](https://github.com/loombridge/beatlayer/blob/main/CONTRIBUTING.md#building-without-abletons-code).

## Licence

GPL-3.0-or-later ([LICENSE](LICENSE)), pending the maintainer's confirmation (decision D3 in the architecture).

Ableton, Ableton Live, and Push are trademarks of Ableton AG, and Max is a trademark of Cycling '74. Beatlayer isn't affiliated with or endorsed by either.

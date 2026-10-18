<img src="assets/beatlayer.png" alt="Beatlayer" width="100%">

<p align="center">An open-source, clean-room reimplementation of Ableton Live, rebuilt in pure Rust. If you know Ableton, you know Beatlayer.</p>

It follows Live 12's workflow, with Session and Arrangement views, warping, racks, and a full set of instruments and effects, and runs natively on macOS (Apple Silicon), Windows, and Linux.

Beatlayer is in early development: the playback core (Phase 1 of 10) is being built.

## Documents

- [Roadmap](docs/01-roadmap.md): the phases and milestones, from alpha 0.1 to 1.0.
- [Architecture](docs/02-architecture.md): the engine, the document model, plug-in hosting and the UI.
- [Build plan](docs/03-build-plan.md): every work package, with its acceptance criteria and status.
- [Parity matrix](docs/04-parity-matrix.md): Live 12's features and the milestone that brings each one.
- [Improvements over Live](docs/05-improvements.md).
- [Behaviour specs](docs/specs) and the [UI parity reference](docs/ui-parity).

How Beatlayer learns from Live without using its code or content is in [CONTRIBUTING.md](CONTRIBUTING.md#building-without-abletons-code).

## Develop

1. Install Rust with [rustup](https://rustup.rs). The repo pins the toolchain, so rustup fetches the right version on first build.
1. Install cargo-deny, which checks dependency licences and advisories, and cargo-nextest, which runs the tests in parallel:

   ```sh
   cargo install --locked cargo-deny
   cargo install --locked cargo-nextest@0.9.146
   ```

1. Run every check: format, clippy, tests, crate layering, and the dependency policy. While you work, `--affected` tests only the crates your branch changes and the crates that depend on them, and runs every other check in full.

   ```sh
   cargo xtask check
   cargo xtask check --affected
   ```

1. To build this platform's package, install cargo-packager once, then package:

   ```sh
   cargo install --locked cargo-packager@0.11.8
   cargo xtask package
   ```

   The packages land in `target/packages`: `Beatlayer.app` and a DMG on macOS, an MSI on Windows, and an AppImage and a tarball on Linux. They're unsigned unless the signing variables in `.github/workflows/nightly.yml` are set. The task then starts each packaged binary with `--version` to check it.

Read [CONTRIBUTING.md](CONTRIBUTING.md) and [AGENTS.md](AGENTS.md) before contributing. Every contribution follows the [clean-room rules](CONTRIBUTING.md#building-without-abletons-code).

## Licence

GPL-3.0-or-later ([LICENSE](LICENSE)). Contributions are made under the Developer Certificate of Origin.

Ableton, Ableton Live, and Push are trademarks of Ableton AG, and Max is a trademark of Cycling '74. Beatlayer isn't affiliated with or endorsed by either.

<img src="assets/beatlayer.png" alt="Beatlayer" width="100%">

<p align="center"><b>Make music the way you do in Live 12. Free, open source, and native on macOS, Windows and Linux.</b></p>

<p align="center">
  <a href="docs/03-build-plan.md">Work packages to 1.0</a><br>
  🟩🟩🟩🟩⬜⬜⬜⬜⬜⬜ <b>43%</b>
</p>

If you know Live, you already know Beatlayer. It's written from scratch in Rust and licensed under the GPL. 

## Why Beatlayer

- Every action in Beatlayer is a command an AI assistant can run, through the built-in assistant or MCP, on a cloud model or one running on your own machine. Ask for a 16-bar idea or a sidechain, and if you don't like the result, one undo takes it back.
- Linux gets the same build as macOS and Windows. PipeWire, JACK and LV2 sit beside Core Audio, ASIO and Audio Units, and every feature ships on all three platforms at once.
- Some small frustrations are fixed: return tracks have no limit, gain goes all the way to silence and mono sources bounce to mono. Every action you take is saved continuously, and reversable. Nothing you already know has moved.
- Projects are documented JSON, CLAP is a first-class format with per-note modulation, and DAWproject exchange is on the roadmap.

## 

## What 0.2 can do

Alpha 0.2 makes a whole song and plays it live. Launch clips and scenes in the Session View, with follow actions. Write MIDI in the piano roll, play a sampler and a polysynth, and host CLAP, VST3 and Audio Unit plug-ins. Record audio and MIDI with punch, count-in and loop takes. Automate anything, route sends and sidechains, then export to WAV or MP3. The assistant can build a 16-bar track from a sentence on a local model. Oh, and stem separation works. Today. CPU only for now, GPU accelerated  seperation will come a bit later.

It's an alpha: the builds are unsigned and you'll find rough edges.

## Download

- [macOS (Apple silicon)](https://github.com/loombridge/beatlayer/releases/latest/download/beatlayer-macos-arm64.dmg)
- [Windows (x64)](https://github.com/loombridge/beatlayer/releases/latest/download/beatlayer-windows-x64.msi)
- Linux (x64): [AppImage](https://github.com/loombridge/beatlayer/releases/latest/download/beatlayer-linux-x64.AppImage) or [tarball](https://github.com/loombridge/beatlayer/releases/latest/download/beatlayer-linux-x64.tar.gz)

macOS and Windows warn you before they open an unsigned build. The [release notes](https://github.com/loombridge/beatlayer/releases/latest) say how to open it.

## Coming

- **0.3:** every warp mode, so loops and recordings sound right at any tempo, plus slice to MIDI, freeze and bounce.
- **0.4:** racks with macros, modulation, and every core effect and MIDI effect.
- **Beta 0.5:** comping, MIDI generators, scales and tunings, and projects from other DAWs through DAWproject.
- **On to 1.0:** Link and controller support including Push 2; wavetable, FM, physical-modelling and granular instruments; stem separation on your GPU; audio to MIDI that needs little clean-up; Live set import; and a plug-in sandbox, so a crashing plug-in can't take your song with it.

## We need your help

Beatlayer is in the very early stages of development, with new code shipping daily. There will be bugs. If you find something that doesn't work as expected or flat-out causes a crash, please open a ticket for us to review using [this template](https://github.com/loombridge/beatlayer/issues/new?template=bug_report.yml). Every report helps make Beatlayer something you can rely on in the studio and on stage.

Fill in every field of the template, in English:

- **Expected:** what you expected to happen.
- **Observed:** what happened instead.
- **Steps to reproduce:** the fewest steps that show the bug.

We're sorry, but reports that don't follow the template may be rejected.

If you're interested in contributing, get in touch. 

Feature requests are welcome too!

## Documents

- [Roadmap](docs/01-roadmap.md): the phases and milestones, from alpha 0.2 to 1.0.
- [Architecture](docs/02-architecture.md): the engine, the document model, plug-in hosting and the UI.
- [Build plan](docs/03-build-plan.md): every work package, with its acceptance criteria and status.
- [Parity matrix](docs/04-parity-matrix.md): Live 12's features and the milestone that brings each one.
- [Improvements over Live](docs/05-improvements.md).
- [Behaviour specs](docs/specs) and the [UI parity reference](docs/ui-parity).

How Beatlayer learns from Live without using its code or content is in [CONTRIBUTING.md](CONTRIBUTING.md#building-without-abletons-code).

## Develop

1. Install Rust with [rustup](https://rustup.rs). The repo pins the toolchain, so rustup fetches the right version on first build.

2. Install cargo-deny, which checks dependency licences and advisories, and cargo-nextest, which runs the tests in parallel:
   
   ```sh
   cargo install --locked cargo-deny
   cargo install --locked cargo-nextest@0.9.146
   ```

3. Run every check: format, clippy, tests, crate layering, and the dependency policy. While you work, `--affected` tests only the crates your branch changes and the crates that depend on them, and runs every other check in full.
   
   ```sh
   cargo xtask check
   cargo xtask check --affected
   ```

4. To build this platform's package, install cargo-packager once, then package:
   
   ```sh
   cargo install --locked cargo-packager@0.11.8
   cargo xtask package
   ```
   
   The packages land in `target/packages`: `Beatlayer.app` and a DMG on macOS, an MSI on Windows, and an AppImage and a tarball on Linux. They're unsigned unless the signing variables in `.github/workflows/nightly.yml` are set. The task then starts each packaged binary with `--version` to check it.

Read [CONTRIBUTING.md](CONTRIBUTING.md) and [AGENTS.md](AGENTS.md) before contributing. Every contribution follows the [clean-room rules](CONTRIBUTING.md#building-without-abletons-code).

## Licence

GPL-3.0-or-later ([LICENSE](LICENSE)). Contributions are made under the Developer Certificate of Origin.

Ableton, Ableton Live, and Push are trademarks of Ableton AG, and Max is a trademark of Cycling '74. Beatlayer isn't affiliated with or endorsed by either.

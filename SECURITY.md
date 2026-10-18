# Security policy

Beatlayer opens files from other people (sets, presets, audio, MIDI, sampler instruments), loads plug-ins, can download packages, talks to AI providers, and lets agents drive it through its agent API. Any of those can carry an attack, so reports about them are welcome.

## Report a vulnerability privately

Use GitHub's private reporting: the repository's **Security** tab, then **Report a vulnerability** ([direct link](https://github.com/loombridge/beatlayer/security/advisories/new)). Only the maintainers see the report, and the fix and advisory are worked out with you there.

Don't put a working exploit, a crash dump with personal data, a secret, or a harmful file in a public issue, discussion, or pull request. If you're not sure whether something is a security problem, report it privately anyway; the maintainers will move it to a public issue if it isn't.

## What to include

- The Beatlayer release you downloaded (or `beatlayer --version`), or the commit you built.
- Your operating system and architecture.
- The part of Beatlayer it affects, and what an attacker needs: a file the person opens, a plug-in they load, access to the agent API, or something else.
- The exact steps to reproduce it, what you expected, and what happened.
- Logs, backtraces, or sanitizer output, with credentials, personal paths, and unrelated content removed.
- Any workaround you know of.

## Harmful files

Don't attach a file that could do harm, or one you aren't free to share, to anything public. In your private report, describe it first: its format, size, SHA-256, where it came from (made by hand, from a fuzzer, found in the wild), what it does, and the command or steps that trigger it. The maintainers will ask for the file through the private report if they need it.

A file that reaches the repository as a regression test must be as small as possible, free of personal or proprietary content, and free to redistribute. Never send Ableton's content: its samples, presets, packs, or audio Live rendered from them.

## In scope

- Reading sets (`.blset`), presets (`.blpreset`), recovery journals, and the files they point to.
- Audio import (WAV, RF64, AIFF, FLAC, MP3, Ogg Vorbis, AAC, ALAC), MIDI files, and SFZ and other sampler instruments: crashes, hangs, memory exhaustion, or anything worse from a crafted file.
- The agent API: the HTTP server (`/mcp` on `127.0.0.1`), `bl-cli mcp` over stdio, tokens, and the permission classes that decide when Beatlayer asks the person first.
- The assistant: where API keys are kept, and what a provider's reply can make Beatlayer do.
- Package downloads, such as the stem separation model and runtime, and their checksum checks.
- Paths: writing outside the folder a person chose, following links unexpectedly, or overwriting without asking.
- The release workflow, its checksums and attestations, and the packages it builds.
- Dependencies, where Beatlayer's use of them makes a vulnerability reachable.

Out of scope: a plug-in that misbehaves. Beatlayer runs plug-ins in its own process with the same access as itself (see [Current limits](#current-limits)), so a malicious plug-in is the person's choice of software to run, as in every DAW today. A way to make Beatlayer load a plug-in the person didn't choose is in scope.

## Supported versions

Beatlayer is an alpha. Only the latest release gets security fixes; there are no backports.

## Current limits

These are known and not yet fixed. They're written here so nobody has to discover them.

- **Plug-ins run in Beatlayer's process.** Scanning happens in a separate process with a timeout and a blocklist, but a loaded plug-in can read and write anything Beatlayer can, and its crash takes Beatlayer down. A plug-in sandbox is planned for 1.0.
- **Agent tokens are stored in plain text** in `preferences.json` in Beatlayer's settings folder. Anything that can read that file can act as an allowed agent. The agent server is off until the person turns it on, listens on `127.0.0.1` only, refuses other `Host` and any `Origin` header, and caps connections and request sizes, but it doesn't limit request rates.
- **Audio and MIDI headers aren't bounded** beyond what the decoders do themselves: a file that declares a huge length or channel count is not refused up front. Sets are capped at 512 MiB of JSON and presets at 16 MiB.
- **The builds are unsigned** (ad-hoc signed on macOS). Every release lists each file's SHA-256 in `SHA256SUMS.txt` and carries a build provenance attestation; the [release notes](RELEASE-NOTES.md#checking-a-download) say how to check them.
- **Fuzzing runs by hand,** not in CI (`fuzz/`).

[The security page](docs/security.md) has the threat model behind this policy.

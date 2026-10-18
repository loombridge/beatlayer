# Security

How Beatlayer is attacked, where the boundaries between untrusted input and trusted state sit, what guards each one today, and what doesn't yet. [SECURITY.md](../SECURITY.md) is the policy: how to report a problem and what's in scope. Change this page when a boundary, a guard, or a known gap changes, in the same pull request.

## What there is to protect

- The person's work: sets, recordings, samples, and presets, from being lost, corrupted, or overwritten.
- Everything else on their computer: files outside the folders they chose, their keychain, and their other applications.
- Their accounts: AI provider keys and agent tokens.
- Beatlayer being there when they need it: no crash or hang from a file or a request, least of all in a performance.
- The packages people download: that each one was built by this repository's workflow from the tagged commit.

## Who attacks

- Someone who sends a file: a set, a preset, a sample, a MIDI file, or a sampler instrument, shared online or in a collaboration.
- A program on the same computer that reaches the agent API, or reads the files Beatlayer keeps.
- An agent, or the model behind the assistant, that does more than the person asked, whether by mistake or because something it read told it to.
- A web page in the person's browser that tries to reach the agent server on `127.0.0.1`.
- Someone who tampers with a download, a dependency, or the build.

Plug-ins are not on this list. A plug-in runs inside Beatlayer with Beatlayer's access (see [Gaps](#gaps)), so loading one is trusting it, as in every DAW. Making Beatlayer load a plug-in the person didn't choose is an attack.

## Trust boundaries

```text
UNTRUSTED
  sets (.blset), presets (.blpreset), recovery journals
  audio, MIDI and SFZ files, and the paths sets point to
  agent requests (HTTP /mcp, bl-cli mcp over stdio)
  the assistant's model replies
  downloaded packages (stem separation)
  dependencies, CI actions, release tooling
        |
        v
BOUNDARY
  format readers and their limits          crates/project, crates/media, crates/interop
  agent transports, tokens, permissions    crates/agent, crates/shell
  the command registry's argument schemas  crates/commands
  checksum checks before install           crates/shell/src/stems.rs
  cargo-deny, pinned actions, pinned tools deny.toml, .github/workflows
        |
        v
TRUSTED STATE
  the document and its undo history
  the engine's program
  files the person chose to write
```

Text that comes from a file, a model, or an agent stays untrusted after it crosses: a track name, a sample path, or a model's reply is data to show and store, never a command to run or a path to trust without the person's choice.

## The boundaries, one by one

| Boundary | Guards today | Gaps |
|---|---|---|
| Sets | A zstd frame with its checksum around JSON (serde_json, ruzstd). At most 512 MiB of JSON. The `format` field must match, and a set from a newer schema is refused, not guessed at. Fuzzed by hand (`set_file`, `set_json`). | serde_json's default recursion limit is the only depth limit. |
| Presets | JSON, at most 16 MiB (256 MiB for rack presets). Fuzzed by hand (`preset`). | |
| Audio files | symphonia's decoders. Before a codec is set up, a header declaring more than 256 channels, a rate above 768 kHz, or more than 2^32 frames is refused (`bl_media::limits`). The RF64 reader also refuses zero channels, rate, or block alignment, and takes the length from the file, not the header. A whole decode reserves at most 64 MiB from the header's length and grows as audio arrives; a sampler's progressive load is refused past 2^28 samples, and the file is decoded whole instead. Fuzzed by hand (`audio_header`); peak files too (`peak_file`). | |
| MIDI files | midly. A file over 64 MiB is refused from its length, before it's read. Fuzzed by hand (`midi_file`). | |
| SFZ instruments | Caps on regions, nesting, and sizes. Fuzzed by hand (`sfz`). | |
| Agent API over HTTP | Off until the person turns it on. Listens on `127.0.0.1` only. Refuses a `Host` other than `127.0.0.1`, `localhost`, or `[::1]`, and any `Origin`, so a web page can't reach it through DNS rebinding. A bearer token on every request, compared in constant time: each allowed agent has its own, and a client with only the server's token is asked about every time. 32 connections, 16 KiB of headers, 8 MiB bodies, 60 s idle timeout. | Tokens are kept in plain text in `preferences.json`. No rate limit. |
| Agent permissions | Every action is a registered command with a permission class. `read` and `edit` run; `destructive` and `external` ask the person first unless they made that agent autonomous, and `bl-cli mcp` refuses them unless started with `--autonomous`. The agent server's own commands and the assistant's settings are the person's only. Edits that overlap the person's clips count as destructive ([agent permissions](specs/agent-permissions.md)). Every `edit` can be undone. | |
| The assistant | API keys in the operating system's keychain, never in a settings file. HTTPS through rustls with the platform's roots. Its tool calls are commands, under the same permission classes as any agent. | A provider's base URL can be changed, so a key goes where the person points it. |
| Package downloads | Stem separation's runtime and model come from fixed URLs and are checked against SHA-256s pinned in the source before they're unpacked or loaded; the person is shown what will be downloaded, from where, and where it goes, before anything is fetched. | |
| Plug-ins | Scanned in a separate process (`beatlayer-scan`) with a 30 s timeout; one that crashes or hangs during a scan is blocklisted. | A loaded plug-in runs in Beatlayer's process, with its access, and its crash is Beatlayer's. |
| Supply chain | `cargo deny` on every change (licences, advisories, bans, sources). Every third-party action pinned to a commit. cargo-packager and the AppImage tools pinned to versions and SHA-256s. CI refuses a dependency whose build script downloads code. | |
| Releases | A draft until a person publishes it. `SHA256SUMS.txt` and a build provenance attestation for every package ([release notes](../RELEASE-NOTES.md#checking-a-download)). | Unsigned, but for macOS's ad-hoc signature, so the operating systems warn before the first launch. |

## Gaps

What isn't protected yet, roughly in the order it should be fixed:

1. **Plug-ins share Beatlayer's process.** The plug-in sandbox (Phase 10, [the roadmap](01-roadmap.md)) moves them out, so a crash or a malicious plug-in can't take the set with it.
2. **Agent tokens in plain text.** They belong in the keychain, beside the assistant's keys.
3. **Fuzzing isn't in CI.** The targets exist; a scheduled run would find regressions without anyone remembering to.
4. **Unsigned builds.** Signing and notarisation need a Developer ID and a Windows signing route ([the architecture's packaging section](02-architecture.md#packaging)).
5. **No rate limit on the agent server.**

## Rules for new code

- A reader for an outside format refuses an oversized or malformed input with an error before it allocates for it, and gets a fuzz target.
- A fixed security bug keeps a regression test: the smallest input that showed it, if it can be shared.
- Anything that reaches the network, writes outside the set's folder, or starts another program is an `external` command, so an agent asks first.
- A download is pinned to a URL and a SHA-256 in the source, and checked before it's used.
- Secrets go in the operating system's keychain, never in a settings file, a log, or a set.

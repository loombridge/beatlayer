# Plug-in compatibility: the battery of checks, the compatibility farm and its page, and the quirks list

- **Status:** draft
- **Work packages:** WP-10.9 (`bl-cli plugin-check`, its checks and its report: R1 to R12); WP-10.10 (the manifest, the farm's workflow and the compatibility page: R13 to R18); WP-10.11 (the quirks list, and the first farm round's fixes: R19 to R25). The sandbox each check runs in is [the plug-in sandbox](plugin-sandbox.md)'s; scanning is [plug-in scanning](plugin-scanning.md)'s
- **Parity-matrix row:** none (Beatlayer's own; the architecture's Testing table promises a host conformance suite run against a fixed set of free plug-ins)
- **Checked against:** [the architecture](../02-architecture.md), "Testing" and "Plug-in hosting"; the CLAP specification 1.2, the VST3 SDK, Apple's AudioToolbox documentation and the LV2 specification, for what each format lets a host ask of a plug-in; read 30 October 2026

## The behaviour

Beatlayer hosts plug-ins it has never seen. To know which work, it runs a scripted battery of checks against any plug-in, `bl-cli plugin-check`: it scans the plug-in, makes it at several sample rates and block sizes, feeds it silence, noise and notes, moves every parameter, saves its state and loads it into a fresh instance, compares an offline render with a real-time one, opens its editor where there is a display, and times it. Every check runs in a sandbox host of its own, so a plug-in that crashes or hangs fails that check and the battery carries on with the next. The result is a JSON report and a one-line summary per plug-in.

A farm of machines, one or more per platform, runs the battery every week against a manifest of widely used free plug-ins, and fails when a plug-in that passed last time fails now. Its results, committed to the repository, generate a compatibility page. Where a plug-in needs the host to behave differently to work, a quirks list names the plug-in, its versions and the workaround, and every host reads it.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Beatlayer | [The architecture](../02-architecture.md), "Testing" (Plug-ins: a host conformance suite run against a fixed set of free plug-ins in CLAP, VST3, AU and LV2) and "Plug-in hosting" | What the battery and the farm are for |
| S2 | Specification | The CLAP specification 1.2; the VST3 SDK; Apple's AudioToolbox documentation; the LV2 specification and its extensions | What a host may ask of a plug-in in each format: activation with a rate and largest block, processing, parameters with ranges and values, state, latency, editors; and the version each plug-in reports |
| S3 | Spec | [The plug-in sandbox](plugin-sandbox.md) R1, R33, R37 and R57; [plug-in scanning](plugin-scanning.md) R1, R3, R4 and R10; [offline plug-in renders](offline-plugin-renders.md) R5; [CLAP hosting](clap-hosting.md) R5, R8 to R10 and R13 | Each instance in a host of its own; crashes and hangs as results; how plug-ins are found and described; renders that repeat bit for bit |
| S4 | Beatlayer | `AGENTS.md` and [the build plan](../03-build-plan.md)'s rules: no plug-in binaries in the repository, no dependency that downloads code; [the security page](../security.md): pinned actions | The farm installs nothing and the repository holds no plug-in |

## Rules

### The battery (WP-10.9)

- **R1.** `bl-cli plugin-check PATH... [--format clap|vst3|au|lv2] [--json FILE] [--rates 44100,48000,96000] [--blocks 32,128,1024,variable] [--seconds 10]` checks each plug-in it's given: a plug-in file or bundle, a folder searched as `plugin.scan` searches one ([plug-in scanning](plugin-scanning.md) R1), or an Audio Unit's component description. The rates and block sizes shown are the defaults. (S1, S3)
- **R2.** It runs the checks of R3 for each plug-in in that order. Each check runs in a sandbox host of its own, Individually ([the plug-in sandbox](plugin-sandbox.md) R33), so a crash or hang during a check fails that check with the reason "crashed" or "hung" ([the plug-in sandbox](plugin-sandbox.md) R37) and the next check starts in a new host. (S3)
- **R3.** The checks:
  - **`scan`:** the plug-in is described within the scanner's timeout; its time is reported.
  - **`instantiate`:** it is made and activated at every rate and block size asked for.
  - **`process`:** at each rate, with blocks of the first size, it processes the given number of seconds of silence, then white noise at −12 dBFS, then a phrase of notes (an instrument or a note effect) generated from a fixed seed; every output sample is finite (no NaN, no infinity) and none is subnormal.
  - **`variable_blocks`:** it processes the note phrase in blocks of varying size, 1 frame to the largest, from a fixed seed, with the same output conditions.
  - **`reactivate`:** it is deactivated and activated again, and processes a block.
  - **`parameters`:** every parameter it lists is set to its minimum, its middle and its maximum, each read back within the parameter's step, with a block processed after each and its output finite.
  - **`state`:** its state is saved, loaded into a new instance and saved again; the check passes when the two saved states are equal, or, when they aren't, when the two instances render the note phrase alike (R5); the report says which.
  - **`latency`:** the latency it reports at activation is the same after the processing checks, unless it asked for a restart.
  - **`offline`:** the note phrase rendered in real time on manual pacing and rendered offline ([offline plug-in renders](offline-plugin-renders.md)) are alike (R5).
  - **`editor`:** where a display is available, its editor opens, is resized and closes, three times, with nothing left behind; skipped, with the reason, where there is no display or no editor.
  - **`isolation`:** two instances in one host, given the same input, give the same output, and a parameter change in one doesn't reach the other.
  - **`timing`:** the processing time of each block during `process`, as its median, its 99th percentile and its worst, against the block's period. It never passes or fails; it is reported.
  (S2, S3)
- **R4.** Each check passes, fails with a reason, or is skipped with a reason. A check that needs one that failed (`process` after a failed `instantiate`) is skipped, "after <check> failed". A plug-in passes when none of its checks fails. (S1)
- **R5.** Two renders are alike when they are equal sample for sample, or, for a plug-in that isn't deterministic (two renders of the same input from the same state differ, which the battery finds by rendering the phrase twice in the first `state` instance), when each third-octave band's level differs by at most 0.5 dB; the report says which comparison was used. (S1)
- **R6.** The battery never downloads anything, and writes only its report and its temporary files, which it removes. (S4)
- **R7.** The report is a JSON document: its `format` (`beatlayer-plugin-check`) and `version` (1); the host's Beatlayer version, commit, operating system and its version, and architecture; the time it started; and for each plug-in its format, identifier, name, vendor, version, architectures and file, its result, the quirks applied (R22), and each check with its result, reason, time taken and details. Its schema is in the repository and the report validates against it. (S1)
- **R8.** On standard output the battery prints one line per plug-in: its result, name, format and version, and the checks that failed; and, at the end, how many passed, failed and were skipped. (S1)
- **R9.** It exits with status 0 when every plug-in passed, 1 when any failed, and 2 when it was used wrongly. (S1)
- **R10.** It runs headless on macOS, Windows and Linux, the editor check being skipped where there is no display. (S1)
- **R11.** The test plug-ins (`bl-test-clap`, `bl-test-vst3`, `bl-test-lv2`) pass every check, and each of their fault modes (a crash or hang in scanning, in processing, and in loading state) fails exactly the check it should, with its reason, while the others run. (S3)
- **R12.** The battery uses the hosts and the sandbox the app uses, with the quirks list applied. (S3)

### The farm and the page (WP-10.10)

- **R13.** `compat/plugins.toml` lists the plug-ins the farm checks. Each entry gives the plug-in's name, vendor, formats, the version checked, the platforms it runs on, the vendor's download page and its licence (an SPDX identifier, or `freeware` with the page that states its terms). It never holds a plug-in or a step that downloads one. (S4)
- **R14.** `.github/workflows/compat.yml` runs weekly and on demand, with a job for each platform on a self-hosted runner label (`compat-macos`, `compat-windows`, `compat-linux`, and the Arm labels when their machines exist). Each job builds `bl-cli`, runs the battery over the manifest's plug-ins installed on its machine, uploads the reports as artefacts, and compares them with the previous run's: a plug-in that passed and now fails fails the job; one checked for the first time is reported and never fails it. Every action it uses is pinned to a commit. (S4)
- **R15.** With no runner for a label, its job is skipped with a notice saying so. (S4)
- **R16.** The results kept for the page are committed under `compat/results/<platform>/`, one report per run, named by its date; the newest for each platform counts. (S1)
- **R17.** `cargo xtask compat-page` writes `docs/compatibility.md` from the newest results: a table of plug-in, vendor, version, format, platform, result and date, with the failing checks and the quirks applied, and a sentence that names of plug-ins and vendors identify their products and belong to their owners. `cargo xtask check` generates it again and fails when the page and the results differ. (S1)
- **R18.** No workflow downloads or installs a plug-in. A farm machine's plug-ins are installed by hand, as `compat/README.md` describes. (S4)

### The quirks list (WP-10.11)

- **R19.** `crates/plugin-host/quirks.toml`, built into the hosts, lists workarounds: each entry has a format, a plug-in identifier, a version range (`*`, or a range of the version the plug-in reports), a list of quirks and the reason. `cargo xtask check` refuses an entry with an unknown quirk, a range that doesn't parse, or the same plug-in and range twice. (S2)
- **R20.** An entry applies to an instance when its format and identifier match and the version the plug-in reports (a CLAP descriptor's version, a VST3 class's version, an Audio Unit's version, an LV2 plug-in's minor and micro version) is in its range; the file's version or date is never used. (S2)
- **R21.** The quirks:
  - **`editor_floating_only`:** the editor always opens in the plug-in's own floating window, never embedded.
  - **`no_offline_loan`:** an offline render makes a new instance from the plug-in's state instead of borrowing the engine's ([offline plug-in renders](offline-plugin-renders.md) R12's way).
  - **`fixed_block_size`:** the plug-in is always given blocks of its largest size; the host buffers to make them and reports the buffering as added latency.
  - **`no_variable_block`:** the plug-in is given the stream's blocks whole, never split at a frame inside a block.
  - **`state_on_main_thread`:** the host deactivates the plug-in before loading state into it, and activates it after.
  - **`ignore_latency_change_while_processing`:** a latency change the plug-in reports while processing is read only at its next activation.
  - **`force_individual_process`:** the plug-in always runs Individually, whatever the mode.
  A new quirk is added only for a farm failure that calls for it, with its test, in this list. (S2, S3)
- **R22.** A quirk changes what the host does, never which checks run: the battery runs every check on a plug-in with quirks, and its report lists them. (S1)
- **R23.** `plugin.info` lists the quirks applied to a device's plug-in. (S3)
- **R24.** Each quirk has a test in which a test plug-in imitating the problem fails without the quirk and passes with it. (S1)
- **R25.** Every failure in the first full farm run on each platform is fixed in Beatlayer's hosts with a regression test, given a quirk, or listed on the compatibility page with its reason. (S1)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 to R12 | to be written in WP-10.9 (the test plug-ins pass; each fault mode fails its check; the report validates against its schema) | `bl-cli` test on three platforms |
| R13 to R18 | to be written in WP-10.10 (the manifest parses; the page and its drift check on sample results; a regression found in a sample results folder) | `xtask` test; workflow lint |
| R19 to R25 | to be written in WP-10.11 (a test per quirk; the file validated) | plug-in host test; `cargo xtask check` |

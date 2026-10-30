# Crash reports: what a report holds, where it's kept, the crash notice, Send Crash Reports, and symbols

- **Status:** draft
- **Work packages:** WP-10.15 (the crash monitor, the report and its minidump, storage, `crash.list`: R1 to R11); WP-10.16 (the crash notice, Send Crash Reports, sending and its fallback, symbols and symbolication: R12 to R24). The recovery dialog the notice joins is [crash recovery](crash-recovery.md) R22 and R29's; the Licenses & Updates page is [the app shell](app-shell.md) R25's
- **Parity-matrix row:** none (Beatlayer's own; Live has no equivalent setting, and Beatlayer collects no usage data, so Send Crash Reports takes the place of Live's Send Usage Data)
- **Checked against:** the Live 12 reference manual (online edition), chapter 2 section 2.3.10, read 30 October 2026; the UI parity reference's row L-SET-019; the minidump format as Microsoft documents it (`MINIDUMP_HEADER`, streams for threads, modules, exception and system information); [the security page](../security.md); [the architecture](../02-architecture.md), "Processes and threads" and "Packaging"

## The behaviour

When Beatlayer crashes, a small monitor process it started with itself writes down what happened: a minidump of the crashed process (its threads' stacks and registers, and the libraries it had loaded) and a report naming the version, the platform, the audio settings, the plug-ins that were running, the last commands run (by ID, without their arguments) and, for a Rust panic, its message and backtrace. The report never holds what the person made: no track or clip names, no notes, no sample paths, nothing from the set but how many tracks, devices and clips it had. Reports stay on the machine, in the app's data folder, the newest ten kept.

Nothing leaves the machine unless the person agrees. At the next start, Beatlayer says it quit unexpectedly, beside the recovery dialog, and offers View Report, Send and Don't Send. Settings ▸ Licenses & Updates has Send Crash Reports, with Ask (in a new install), Always and Never, where Live has Send Usage Data. A report is sent over HTTPS to the project's endpoint, or, in a build without one, Send becomes Open an Issue, with the report's text ready to paste. The project keeps debug symbols for every release, so a minidump can be turned into function names.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 2 "First Steps", section 2.3.10 "Licenses & Updates" | The page manages authorisation, automatic updates and usage data |
| S2 | Reference | [layout.md](../ui-parity/layout.md) L-SET-019 (Licenses & Updates: Licenses; Maintenance with Get Automatic Updates and Send Usage Data) and the paragraph after L-SET-020 (rows tied to Ableton's services become Beatlayer's own or are left out, each with a design rule) | The page's sections and rows, and how Beatlayer treats rows tied to Ableton's services |
| S3 | Standard | Microsoft's documentation of the minidump format (`MINIDUMP_HEADER`, `ThreadListStream`, `ModuleListStream`, `ExceptionStream`, `SystemInfoStream`) | What a minidump holds: threads with their stacks and registers, the loaded modules with their build IDs, the exception, and the system |
| S4 | Beatlayer | [The security page](../security.md): "What there is to protect" and "Rules for new code" | Secrets never in logs; anything that reaches the network is an `external` command |
| S5 | Spec | [Crash recovery](crash-recovery.md) R21 to R23 and R29; [the plug-in sandbox](plugin-sandbox.md) R37 and R38; [the app shell](app-shell.md) R23 and R25 | The recovery dialog at start; a plug-in's host crashing alone; `settings.state` and the Licenses & Updates page |

## Rules

### Capturing a crash (WP-10.15)

- **R1.** The app starts a crash monitor, a second copy of its own executable run as a monitor, before the first set opens; `beatlayer-host` processes connect to the same monitor when they start. A crash in `beatlayer-scan` makes no report: the scanner's blocklist records it ([plug-in scanning](plugin-scanning.md) R4). (S5)
- **R2.** When a watched process crashes (an invalid memory access, an abort, an illegal instruction, a stack overflow, or a Rust panic that ends the process), the monitor writes a minidump of it and a report. The crashing process only tells the monitor and waits for it: nothing it does after the crash allocates or takes a lock. (S3)
- **R3.** A Rust panic writes its message, its thread's name and its backtrace into the pending report, then goes on as it would have. A panic caught for one node or one pool task ([the architecture](../02-architecture.md)'s real-time rule 6) isn't a crash and leaves no report. (S5)
- **R4.** A report holds: its kind (the signal or exception, or a panic); the time in UTC; Beatlayer's version and commit; the operating system and its version; the processor's model and architecture, and the memory; which process crashed (the app or a host, and the host's plug-ins); the audio driver type, the device, the sample rate and the buffer size; each plug-in instantiated, by format, identifier, vendor, version, architecture and process; the last 50 command IDs run, with their times, without their arguments; the open set's numbers of tracks, devices and clips; the crashed thread's name; for a panic, its message and backtrace; and the size of the minidump. (S3, S4)
- **R5.** A report never holds a name a person gave (a track, clip, scene, device, set or file), a note or an automation value, a path other than a plug-in's file name, the user's name or the computer's, a token, a key, an agent's name or what the assistant said. In a panic's message and backtrace, the home folder is written `~` and any other path outside Beatlayer's own sources becomes `<path>`. (S4)
- **R6.** A minidump holds the threads' stacks and registers, the loaded modules, the exception and the system information, and no other memory. One larger than 8 MiB isn't kept, and its report says so. (S3)
- **R7.** Reports are kept in the app's data folder, under `Crash Reports`, a report and its minidump named by the time and kind; when there are more than 10, the oldest are deleted at the next start. (S4)
- **R8.** A sandboxed plug-in's crash makes a report of its host, naming the plug-ins the host held, while Beatlayer runs on ([the plug-in sandbox](plugin-sandbox.md) R38). (S5)
- **R9.** `crash.list` (`read`) lists the reports newest first, each with its ID, time, kind, version, process, a one-line summary (a panic's first line, or the crashing frame's module), whether it was sent, and whether it is from the last session; `crash.report` (`read`) returns one report's text; `crash.discard` (`destructive`) deletes one report or all of them. (S4)
- **R10.** Capturing sends nothing anywhere. (S4)
- **R11.** The monitor keeps the report's metadata current outside the crash path (the plug-ins, the audio settings, the command IDs), so that at a crash it only copies what it already has. (S3)

### Consent and sending (WP-10.16)

- **R12.** When Beatlayer starts and `crash.list` has reports from the last session that haven't been sent or declined, and Send Crash Reports is Ask, the recovery dialog ([crash recovery](crash-recovery.md) R22, R29) opens with a notice at its top: that Beatlayer quit unexpectedly (or, for a host's report alone, that a plug-in stopped working), with View Report, Send and Don't Send. With no set to recover, a dialog of the same form holds the notice alone. (S2, S5)
- **R13.** View Report shows each report's text, exactly as it would be sent, and the minidump's size. Send runs `crash.send` for each report the notice lists; Don't Send marks them declined, keeping them on the machine until R7 removes them. Esc does what Don't Send does. The notice is a group in the accessibility tree, its buttons in Tab order, Send focused first. (S5)
- **R14.** Settings ▸ Licenses & Updates ▸ Maintenance has Send Crash Reports, a chooser of Ask, Always and Never, after Get Automatic Updates, in the place of Live's Send Usage Data ([the app shell](app-shell.md) R25). Ask is chosen in a new install. It is kept in the preferences, set by `settings.set_crash_reports`, and read back from `settings.state` as `licenses_updates.send_crash_reports`. (S1, S2)
- **R15.** With Never, no notice shows and nothing is sent; reports are still kept on the machine (R7). (S4)
- **R16.** With Always, reports are sent without asking: at start, those not yet sent, and a host's report as soon as it's written; the Status Bar says once that a report was sent. (S4)
- **R17.** `crash.send` (`external`) posts a report and its minidump over HTTPS (rustls, with the platform's roots) to the endpoint address fixed in Beatlayer's source, with nothing else: no identifier, no cookie, and a user agent of Beatlayer's name and version. A report is sent at most once; one that fails to send stays unsent and is offered again. Sending never happens in the crash path. (S4)
- **R18.** In a build whose endpoint address is empty (development builds and forks), Send reads Open an Issue: it opens the project's issue form in the system's browser with the report's text in its body, and shows the report's folder so the person can attach the minidump. (S4)
- **R19.** `settings.set_crash_reports` is `edit`, a kept preference; `crash.send` names `crash.list`'s `sent` as its readback. (S4)

### Symbols (WP-10.16)

- **R20.** The release and nightly workflows keep debug symbols for every executable they package (dSYM bundles on macOS, PDB files on Windows, separate DWARF files on Linux) as artefacts of the release or run, matched to the binaries by build ID; the packages don't hold them. (S3)
- **R21.** `cargo xtask crash-report FILE [--symbols DIR]` prints a report and, from its minidump, each thread's frames, with function names, files and lines wherever symbols for the module's build ID are found. (S3)
- **R22.** [The security page](../security.md) has a row for crash reports: what a report holds, where it is kept, and when and where it goes. (S4)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 to R3, R6 | to be written in WP-10.15 (a test binary crashed by a null write, an abort, a stack overflow and a panic, on each platform; the minidump read back) | integration |
| R4, R5, R11 | to be written in WP-10.15 (a set seeded with marker strings; every report scanned) | integration |
| R7, R9, R10 | to be written in WP-10.15 | shell test, agent API |
| R8 | to be written in WP-10.15 (the test plug-in's crash-in-process mode) | integration |
| R12, R13 | to be written in WP-10.16 | UI script |
| R14 to R19 | to be written in WP-10.16 (a local test server for Ask, Always and Never; the Open an Issue path) | shell test, agent API |
| R20, R21 | to be written in WP-10.16 | workflow lint; `cargo xtask crash-report` on a crashed release build |
| R22 | the security page's change in WP-10.16 | review |

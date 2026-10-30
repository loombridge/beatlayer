# Plug-ins built for another architecture: x86_64 plug-ins on Apple Silicon, and x64 plug-ins on Windows on Arm

- **Status:** draft
- **Work packages:** WP-10.7 (reading a plug-in's architectures, and x86_64 plug-ins on Apple Silicon through Rosetta: R1 to R12 and R16); WP-10.33 (x64 and Arm64EC plug-ins on Windows on Arm: R13 to R15). The host process they run in is [the plug-in sandbox](plugin-sandbox.md)'s; the scanner and its cache are [plug-in scanning](plugin-scanning.md)'s
- **Parity-matrix row:** Plug-ins: (Beatlayer only) x86_64 plug-ins on Apple Silicon through Rosetta
- **Checked against:** Apple's Mach-O and universal binary headers (`mach-o/loader.h`, `mach-o/fat.h`), its documentation of Rosetta (`sysctl.proc_translated`, launching a process for a chosen architecture with `posix_spawnattr_setbinpref_np`) and its support article on installing Rosetta; Microsoft's PE format documentation (the machine field, the load configuration's CHPE metadata for Arm64EC and Arm64X) and its documentation of x64 emulation on Windows 11 on Arm; the System V ELF specification's `e_machine`; [the architecture](../02-architecture.md), "Plug-in hosting" and "Packaging", read 30 October 2026

## The behaviour

A plug-in is native code built for one or more processor architectures. On an Apple Silicon Mac, many plug-ins still ship only for Intel (x86_64); on a Windows machine with an Arm processor, many ship only for x64. The operating system can run such code through emulation (Rosetta 2 on macOS, Windows' own x64 emulation), but only in a process that is itself of that architecture. Beatlayer therefore reads which architectures each plug-in binary holds when it scans it, and runs a plug-in that has no code for Beatlayer's own architecture in a `beatlayer-host` of the plug-in's architecture, which the system emulates. Such a plug-in is always sandboxed, whatever the hosting mode, and is scanned by a scanner of its architecture too, so its code never runs in Beatlayer's own process.

The browser marks a bridged plug-in, `plugin.info` says which architecture it is and that it is bridged, and a Mac without Rosetta lists such plug-ins as unavailable with the reason and a link to Apple's instructions, never installing Rosetta itself.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Specification | Apple's Mach-O headers (`mach-o/loader.h`: the magic numbers and `cputype`; `mach-o/fat.h`: the fat header, `fat_arch` and `fat_arch_64`, big-endian) | A Mach-O file is thin (one architecture) or fat (a header listing each architecture's slice, with 32- or 64-bit offsets) |
| S2 | Specification | Apple's documentation of Rosetta: `sysctl.proc_translated`; `posix_spawnattr_setbinpref_np` with `CPU_TYPE_X86_64`; launching an x86_64-only executable without Rosetta fails with "bad CPU type in executable" (`EBADARCH`); Apple's support article on installing Rosetta | An x86_64 process runs under Rosetta on Apple Silicon; a launcher can ask for the x86_64 slice of a universal executable; Rosetta may be absent until installed |
| S3 | Specification | Microsoft's PE format documentation (the COFF header's `Machine`: `IMAGE_FILE_MACHINE_AMD64` 0x8664, `IMAGE_FILE_MACHINE_ARM64` 0xAA64; the load configuration directory's CHPE metadata); Microsoft's documentation of Arm64EC, Arm64X and x64 emulation on Windows 11 on Arm | A PE file names one machine; an Arm64EC binary is an x64-machine file with Arm64EC code that loads only into an x64 (emulated) process; an Arm64X binary holds both Arm64 and Arm64EC code and loads into either |
| S4 | Standard | The System V ABI's ELF specification, `e_machine` (`EM_X86_64` 62, `EM_AARCH64` 183) | An ELF file names its machine |
| S5 | Beatlayer | [The architecture](../02-architecture.md), "Plug-in hosting" (In-process first) and "Packaging"; [the roadmap](../01-roadmap.md), "What Beatlayer does that Live doesn't" | The sandbox gives a route to x86_64 plug-ins on Apple Silicon through Rosetta; the scanner ships in the app bundle |
| S6 | Spec | [The plug-in sandbox](plugin-sandbox.md) R1, R2, R19, R20 and R33; [plug-in scanning](plugin-scanning.md) R2, R4, R11 and R14 | Hosts, their region's layout, their priority and the modes; the scanner's cache, blocklist, browser rows and packaging |

## Rules

### Reading a plug-in's architectures (WP-10.7)

- **R1.** The scanner reads which architectures each plug-in binary holds, from its headers alone, without loading it: for a bundle, the executable its `Info.plist` names (`CFBundleExecutable`), or the format's own executable path inside it; for Mach-O, a thin file's `cputype`, or every slice of a fat file, 32- or 64-bit; for PE, the `Machine` field, and for an x64-machine file whose load configuration has CHPE metadata, Arm64EC, or Arm64X where it also holds Arm64 code; for ELF, `e_machine`. It reads at most the first 64 KiB of the file and the load configuration, refuses a header whose counts or offsets point outside the file, and blocklists such a plug-in with the reason ([plug-in scanning](plugin-scanning.md) R4). (S1, S3, S4)
- **R2.** The cache keeps each plug-in's architectures beside its description ([plug-in scanning](plugin-scanning.md) R2, R21). (S6)
- **R3.** A plug-in is native when its binary has code for the architecture Beatlayer runs as: arm64 on an Apple Silicon Mac (a universal binary with an arm64 slice is native); Arm64 or Arm64X on Windows on Arm; x64 on Windows x64; x86_64 or aarch64 on Linux, as Beatlayer is built. It is bridged when it has none but has code a bridge runs: x86_64 on an Apple Silicon Mac, through Rosetta (R4 to R12); x64 or Arm64EC on Windows on Arm, through the system's emulation (R13 to R15). Otherwise it is unavailable, listed with the reason ("built for x86_64 only"). (S1, S2, S3)

### x86_64 plug-ins on Apple Silicon (WP-10.7)

- **R4.** A bridged plug-in is scanned by `beatlayer-scan` built for x86_64, started so the system runs its x86_64 code under Rosetta, with the same timeout, cache and blocklist as the native scanner ([plug-in scanning](plugin-scanning.md) R3, R4, R26). (S2, S6)
- **R5.** A bridged plug-in runs only in a `beatlayer-host` built for its architecture, and always in a host, whatever the Plug-In Hosting setting and the device's override: where they say Within Beatlayer it is hosted By Plug-In; Together gives one host for each architecture. (S5, S6)
- **R6.** Beatlayer never loads a bridged plug-in's code in its own process, to scan it, to read its name, or for anything else. (S5)
- **R7.** A host for another architecture shares its region with Beatlayer as a native one does, the layout being the same on every architecture ([the plug-in sandbox](plugin-sandbox.md) R2). Its audio thread joins Beatlayer's audio workgroup where the system allows it, and otherwise keeps real-time priority alone ([the plug-in sandbox](plugin-sandbox.md) R20). (S2, S6)
- **R8.** The browser's row for a bridged plug-in is its usual row ([plug-in scanning](plugin-scanning.md) R11) with a small mark reading x86 after its format badge; its accessible name ends "for Intel, runs through Rosetta". (S6)
- **R9.** `plugin.scan` and `plugin.info` report each plug-in's `architectures`, and for a device whether it is `bridged`, through which bridge (`rosetta`, `windows_emulation`), and the architecture its host runs as. (S6)
- **R10.** When Rosetta isn't installed (the x86_64 scanner can't start, failing with a bad CPU type, or the probe `sysctl.proc_translated` can't run), every bridged plug-in is listed as unavailable with the reason "needs Rosetta" and a link to Apple's article on installing it, and a set's device for one keeps its state as a missing plug-in's does ([CLAP hosting](clap-hosting.md) R12). Beatlayer never installs Rosetta itself. Once Rosetta is installed, a rescan finds the plug-ins. (S2)
- **R11.** The macOS app holds `beatlayer-scan` and `beatlayer-host` for both arm64 and x86_64, beside the app's executable, signed with the app; the packaging check lists all four ([plug-in scanning](plugin-scanning.md) R14). (S5)
- **R12.** An Audio Unit v2 with only x86_64 code takes the same route: scanned and hosted by the x86_64 helpers. (S1, S6)

### Windows on Arm (WP-10.33)

- **R13.** On Windows on Arm, x64 and Arm64EC plug-ins are bridged: scanned by an x64 `beatlayer-scan` and hosted by an x64 `beatlayer-host`, which the system emulates, as R4 to R6 say for Rosetta. Arm64 and Arm64X plug-ins are native. (S3)
- **R14.** The Windows on Arm packages hold the Arm64 and the x64 builds of both helpers, signed as the app is. The browser's mark for a bridged plug-in reads x64. (S3, S5)
- **R15.** On Linux on Arm there is no bridge: a plug-in with only x86_64 code is unavailable, with the reason. (S4)

### What crosses (WP-10.7)

- **R16.** Audio and events cross to a host of another architecture unchanged, as to a native one ([the plug-in sandbox](plugin-sandbox.md) R3): the test CLAP plug-in's x86_64 build renders exactly what its arm64 build renders. (S6)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R2 | to be written in WP-10.7 (generated headers: thin arm64, thin x86_64, fat, fat64, a malformed header); WP-10.33 adds x64, Arm64, Arm64EC and Arm64X PE files | unit |
| R3 to R7, R16 | to be written in WP-10.7 (an x86_64 build of `bl-test-clap` scanned and rendered on the macOS arm64 runner, nulled against its arm64 build; Within Beatlayer still hosted) | plug-in host test, golden render |
| R8, R9 | to be written in WP-10.7 | UI script, agent API |
| R10 | to be written in WP-10.7 (a simulated launch failure) | unit, shell test |
| R11, R12 | to be written in WP-10.7 | `cargo xtask package`, `codesign --verify` |
| R13, R14 | to be written in WP-10.33 (an x64 build of the test plug-in rendered on the Windows Arm runner) | plug-in host test, package |
| R15 | to be written in WP-10.32 | unit |

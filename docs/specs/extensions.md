# Extensions

- **Status:** draft
- **Work packages:** WP-9.29 (the extension host in `bl-ext`, the package and its manifest, validation, installing, enabling and removing, what a component can reach, a failing extension stopped alone, the commands: R1 to R16); WP-9.30 (MIDI tools as extensions: R17 to R20); WP-9.31 (control surfaces as extensions: R21 to R23); WP-9.32 (devices as extensions: R24 to R31); WP-9.33 (extension device panels: R32 to R37); WP-9.34 (the API to the set: R38 to R45); WP-9.35 (the SDK and the bundled extensions: R46 to R50). The worlds themselves (their types and functions) are documented in the architecture's "Extensions" and in the SDK's guide; what a MIDI tool does around its function is [MIDI Tools](midi-tools.md)'s; what a surface does is [control surfaces](control-surfaces.md)'; what a device's parameters are is [device framework](device-framework.md)'s
- **Parity-matrix row:** Racks and device features: Max for Live API (Live Object Model)
- **Checked against:** Beatlayer's own design; the WebAssembly Component Model and WIT specifications; the Live 12 reference manual (online edition), chapter 32 "Max for Live", read 30 October 2026, for what an extension reaches

## The behaviour

Extensions add MIDI tools, control surfaces and devices to Beatlayer, written in any language that compiles to a WebAssembly component. A person installs an extension package from a file; Beatlayer checks it, compiles it once and keeps the compiled code, and its tools, surfaces and devices appear after Beatlayer's own. An extension reaches only what its world gives it: no files, no network, no clock but the one it's given. One that fails, traps or runs past its time is stopped and switched off alone, with its reason shown where it was, and the set goes on.

An extension can read the set (its tracks, clips, scenes, devices, parameters, the transport and the selection), observe changes to it, and run Beatlayer's registered commands under the same permission classes an agent has: edits run, and destructive or external commands ask the person first. This takes the place Live gives Max for Live's object model; it isn't compatible with Max.

Beatlayer's own bundled extensions (the MIDI Tools Pack's tools, the sequencer, performance and nature devices) are built with the same SDK, from source in the repository.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Standard | The WebAssembly Component Model and the WIT interface definition language, as published by the W3C WebAssembly Community Group and the Bytecode Alliance | Components, worlds, imports and exports, the canonical ABI, and versioned packages |
| S2 | Manual | Live 12 reference manual, chapter 32 "Max for Live" | Max for Live devices are instruments, audio effects and MIDI effects, and reach the set's tracks, clips, devices and parameters; MIDI Tools can be Max devices |
| S3 | Spec | [MIDI Tools](midi-tools.md) R5, R16, R24 to R26; [control surfaces](control-surfaces.md) R6 to R9; [device framework](device-framework.md) (descriptors, G1 and G2); [agent permissions](agent-permissions.md) R1 to R4; [undo history](undo-history.md) (agents' steps marked); [browser](browser.md) (the User Library) | The interfaces extensions implement, and the permission classes |
| S4 | Beatlayer | The architecture's "Extensions", "Real-time rules", "Agent control" (D17) and D12; the roadmap (extensions in any language that compiles to WebAssembly) | Components under `wasmtime` with memory reserved up front, a native modular device beside them; nothing on the audio thread allocates, locks or blocks |

## Rules

### Packages and the host (WP-9.29)

- **R1.** An extension package is a ZIP file with the extension `.blext`, holding `extension.toml` at its root and the components it names. The manifest gives the extension's ID (reverse-domain, such as `org.example.tools`), name, version, author, licence, description, and for each component its file, its kind (`midi-tool`, `control-surface`, `audio-effect`, `instrument`, `midi-effect`), the world version it targets, and the imports it asks for (R38). (S1, S4)
- **R2.** Worlds are versioned as WIT packages, `beatlayer:ext@<version>`. Beatlayer accepts every world version of the same major version up to its own; a component targeting a later minor version or another major version is refused, naming the version. (S1, S4)
- **R3.** Installing (`extension.install`, `external`) checks the package before anything is kept: an unsafe path in the ZIP, a manifest that doesn't parse or lacks a field, a component it doesn't name or that's missing, a component that isn't valid or doesn't export its kind's world, or a duplicate ID with an equal or later version installed, refuses the whole package with the reason and keeps nothing. (S4)
- **R4.** An installed extension goes in the User Library's `Extensions` folder, unpacked, one folder per ID. Installing a later version of an installed extension replaces it; its devices in open sets reload with their state (R28). (S3; Beatlayer's own)
- **R5.** Beatlayer compiles each component ahead of time with `wasmtime` and keeps the compiled code in its cache folder, keyed by the component's hash and `wasmtime`'s version; a restart reuses it, a changed component or a new `wasmtime` compiles again. (S4)
- **R6.** `extension.set_enabled` (`edit`) switches an extension on and off, kept in the preferences; a disabled extension's tools and surfaces aren't listed and its devices in a set become silent placeholders that keep their state (R30). `extension.remove` (`destructive`) removes it and its compiled code. (S4)
- **R7.** `extension.list` (`read`) gives each installed extension's ID, name, version, kinds, whether it's on, and its last failure; `extension.describe` (`read`) gives one extension's manifest, components and every call it may make (R44). (S4)
- **R8.** A component reaches only its world's imports. It has no file system, network, environment, process or clock imports; the time it sees is the time its calls are given. (S1, S4)
- **R9.** Each instance gets a memory reserved up front, of the size its manifest asks for up to 64 MiB, in a pooling allocator, so no call allocates in the host. (S4; the limit is Beatlayer's own)
- **R10.** A component that traps (a fault, an out-of-bounds access, an unreachable instruction, memory exhausted) is stopped, its instance dropped, and its extension marked with the failure; the set and every other extension go on. (S4)
- **R11.** Calls are bounded by epoch interruption: a call that runs past its kind's time limit (R18, R22, R27, R39) is stopped as a trap is (R10). (S4)
- **R12.** A failed component stays off until the person switches its extension on again or Beatlayer restarts. (Beatlayer's own)
- **R13.** `bl-ext` hosts `wasmtime` and depends on no UI crate. (S4)
- **R14.** Every command an extension's components add runs through the registry with the extension's ID in its undo step's name (R41). (S3)
- **R15.** Nothing an extension does can make the audio thread allocate, lock or block (R27). (S4)
- **R16.** Headless, `bl-cli mcp` installs, lists, enables and removes extensions as the app does, in the headless run's own folders. (S4)

### MIDI tools (WP-9.30)

- **R17.** A `midi-tool` component exports `describe` and `run`. `describe` gives the tool's description as a native tool's ([MIDI Tools](midi-tools.md) R5, R26); `run` takes the input R16 there lists and returns the notes it writes. Beatlayer adapts it to the `MidiTool` trait, so it is chosen, set, applied, chained, seeded and undone as a native tool is, through the same `midi_tool.*` commands. (S3)
- **R18.** A run is limited to 250 ms; past it, the run is refused and nothing changes. A run that returns more than 65,536 notes, or notes outside the range or keys, is refused. (S3; the 250 ms is Beatlayer's own)
- **R19.** Extension tools list in the Transform and Generate selectors after the native tools, in alphabetical order, under a separator, each with its extension's name in its description. (S3; Beatlayer's own)
- **R20.** An extension tool's settings are saved with the set under its extension's ID and the tool's ID; a set whose extension is missing or off keeps them, lists the tool as missing, and gives them back when the extension returns. (S3)

### Control surfaces (WP-9.31)

- **R21.** A `control-surface` component exports `start`, `stop`, `on-message` and `tick`; Beatlayer adapts it to the `ControlSurface` trait ([control surfaces](control-surfaces.md) R6), on the host thread, giving it its messages, the 50 Hz tick with the read model, and taking its messages and commands back. (S3)
- **R22.** Each call is limited to 10 ms; a component that traps or runs past it stops alone ([control surfaces](control-surfaces.md) R49), its row saying why. (S3; the 10 ms is Beatlayer's own)
- **R23.** Extension surfaces list in the Control Surface chooser after the native surfaces and the templates ([control surfaces](control-surfaces.md) R49). (S3)

### Devices (WP-9.32)

- **R24.** `audio-effect`, `instrument` and `midi-effect` components describe their parameters as the device framework's descriptors (automatable, modulatable, gains to −∞ dB) and run as native devices do, in the chain, saved with the set, with presets. (S3)
- **R25.** Audio crosses into a device instance through its own memory: the host writes each block's input into buffers the instance reserved and reads its output from them, so no call copies through the host's allocator. Parameter events and notes arrive per block with their frames. (S4)
- **R26.** An instance is made, and its state restored, off the audio thread; the audio thread only calls its `process`. (S4)
- **R27.** A `process` call is limited to a quarter of the block's duration. A call past it is stopped; the device outputs silence from that block on (MIDI effects output nothing), switches itself off, and shows the reason in its title bar; the set goes on. (S4; the quarter is Beatlayer's own)
- **R28.** A device's state is bytes it gives and takes, saved with the set and in its presets; latency it reports is compensated ([delay compensation](delay-compensation.md)). (S3)
- **R29.** Extension devices list in the browser in their categories (Instruments, Audio Effects, MIDI Effects), each with a badge naming its extension, and under an Extensions label listing every extension's devices. (Beatlayer's own)
- **R30.** A set whose extension device is missing or off keeps a placeholder with its name, parameters' values and state, silent, and gives them back when the extension returns. (S4)
- **R31.** An extension device renders identically at any block size; G1 and G2 of [device framework](device-framework.md) hold for its parameters. (S3)

### Device panels (WP-9.33)

- **R32.** An extension device may draw its display cells ([device framework](device-framework.md), IMP-14) through a drawing list: paths, fills, strokes, text by theme role and size, and images from its package, in Beatlayer's typeface and theme colours. `bl-ui` draws the list; `bl-ext` only holds it. (S3, S4)
- **R33.** The drawing comes from a separate UI instance of the component on the main thread, which talks to the audio instance only through parameters and a bounded message queue. (S4)
- **R34.** Pointer and key events in a display cell go to the UI instance, which answers with parameter gestures: a drag is one gesture and one undo step on its parameter. (S3)
- **R35.** The UI instance names regions of the cell with an accessible name, role and value, which reach assistive technology, and Tab reaches each region. (S3)
- **R36.** `device.display` gives an extension device's drawing list and regions, as it gives a native device's display, so agents and tests read the drawing headless. (S4)
- **R37.** A UI instance's call is limited to 8 ms; past it, the cell shows its last drawing and the reason. (Beatlayer's own)

### The API to the set (WP-9.34)

- **R38.** A component that asks for the `set` import in its manifest gets a read model of the set (tracks, clips, scenes, devices, parameters, the transport and the selection) as the main thread last published it, observers that report changes since its last poll, and a call that runs a registered command by its ID with JSON arguments, checked against the command's schema. (S2, S3)
- **R39.** These calls run on the main thread, polled once a frame (or every 50 ms headless), limited to 10 ms a poll; they never reach the audio thread. (S4)
- **R40.** An extension's command calls take the agent permission classes ([agent permissions](agent-permissions.md) R1 to R4): `read` and `edit` run; `destructive` and `external` ask the person first, naming the extension, and are refused headless unless the run allows them. (S3)
- **R41.** Each command an extension runs is one undo step, marked in the undo history with the extension's name, as an agent's steps are. (S3)
- **R42.** An observer sees a volume change, a clip launch and a selection change on the poll after it happens. (S4)
- **R43.** Device and tool components get the same imports when their manifest asks for them, on their own UI or tool calls, never in `process`. (S4)
- **R44.** `extension.describe` lists every command an extension may call, with its class. (S4)
- **R45.** An extension's calls can't reach the agent API's tools beyond the registry's commands and the read model. (S4)

### The SDK and bundled extensions (WP-9.35)

- **R46.** The SDK is the published WIT worlds, a Rust guest crate built on `wit-bindgen` with a template for each kind, and `bl-cli ext` with `new`, `build`, `check` and `pack` subcommands that make an installable package. (S4)
- **R47.** The SDK's WIT files and guest crate carry the world version the host accepts (R2). (S1)
- **R48.** Beatlayer's bundled extensions live under `extensions/` in the repository and are built from source with `cargo xtask` when Beatlayer is packaged; no compiled component is committed. (S4)
- **R49.** Every example under `sdk/examples/` builds in CI and installs in a headless test. (S4)
- **R50.** The author's guide under `docs/extensions/` documents the worlds and the SDK; its code blocks compile. (S4)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 to R16 | to be written in WP-9.29: `crates/ext/tests/host.rs` with test components; `cargo xtask layers` | unit, agent API |
| R17 to R20 | to be written in WP-9.30: `crates/shell/tests/extension_tools.rs` | shell test |
| R21 to R23 | to be written in WP-9.31: the native test surface's scenario run against a component, message for message | unit |
| R24 to R31 | to be written in WP-9.32: renders at block sizes 1, 61 and 512 under the allocation trap and RealtimeSanitizer | engine test, RealtimeSanitizer |
| R32 to R37 | to be written in WP-9.33: an offscreen render against its golden image in both themes; UI scripts | UI script |
| R38 to R45 | to be written in WP-9.34: `crates/agent/tests/extension_set_api.rs` | agent API |
| R46 to R50 | to be written in WP-9.35: CI builds and installs every example | CI, unit |

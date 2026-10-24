# 02. Architecture

Beatlayer is a Rust application in one Cargo workspace. Its core idea is a strict split between three things: the **document** (the song, edited on the main thread as an immutable persistent tree), the **engine** (a compiled, real-time program that makes sound and never waits on anything), and the **UI** (a view of the document and the engine's state that edits the document only through commands). Everything else, such as plug-in hosting, media, devices, control surfaces, and interop, hangs off those three.

The engine and the document have no UI dependency, so the whole product runs headless. That's how it's tested: a project renders offline to a file, and the file is compared against a reference.

## Requirements that shape the design

| Requirement | Target | Why it shapes the design |
|---|---|---|
| Platforms | macOS 13 or later on Apple Silicon; Windows 10 and 11 on x64; Linux x64 (glibc, X11 and Wayland) | Three audio stacks, three plug-in window systems, three packaging routes. Windows on Arm and Linux on Arm are later targets. |
| Latency | The engine adds no buffering beyond the device buffer. Buffers from 32 to 2,048 frames. | No ring buffer between device and engine on the output path; duplex I/O on the input path. |
| Real-time safety | Zero allocations, locks, or blocking calls on audio threads, enforced by tests | Shapes every crate below the UI. See "Real-time rules". |
| Capacity | 128 audio tracks, each with an EQ and a compressor, at 48 kHz and 128 frames on an M1, with no dropouts for 10 minutes | Needs multicore graph processing and SIMD DSP from the start. |
| UI | 60 fps with 200 tracks and 5,000 clips visible in the arrangement, on an M1 and a mid-range Windows laptop | Needs GPU-drawn timelines and waveforms. |
| Determinism | An offline render of a project is bit-identical across runs on the same platform | Seeded randomness, no timing-dependent behaviour in offline renders. Maths that decides which frame something lands on (tempo ramps) uses `libm`, pure Rust, so it rounds alike on every platform. |
| Accessibility | Every control reachable by keyboard and exposed to screen readers | Live 12 has screen-reader support. Built in from the first widget, not added later. |

## Processes and threads

```
┌──────────────────────────── beatlayer (main process) ─────────────────────────────┐
│                                                                                      │
│  Main thread ───────────── owns the Document, runs the UI, applies Commands          │
│     │  Arc<Snapshot>                                                                  │
│     ▼                                                                                 │
│  Compiler thread ───────── Snapshot → EngineProgram (graph, schedule, PDC, clip data) │
│     │  rtrb queue (program swaps, clip-data swaps)        ▲ triple buffers, atomics  │
│     ▼                                                       │ (meters, playhead,       │
│  Audio thread (device callback) ── runs the program ───────┘  clip states, CPU)       │
│     │  wakes                                                                          │
│     ▼                                                                                 │
│  RT worker pool ────────── parallel graph tasks (joined to the audio workgroup)       │
│                                                                                      │
│  Drop thread ───────────── frees anything the audio thread hands back                │
│  Disk thread ───────────── streams long audio files ahead of the playhead            │
│  Background pool ───────── peaks, analysis, autosave, browser indexing, ML           │
└──────────────────────────────────────────────────────────────────────────────────────┘
          │ spawn, IPC
          ▼
  beatlayer-scan (child process) ─── scans plug-ins with a timeout; a crash kills only the scan
  beatlayer-host (child process, later) ─── sandboxed plug-ins and Rosetta-bridged x86_64 plug-ins
```

## Crates

| Crate | Responsibility | Depends on |
|---|---|---|
| `bl-core` | Time types, tempo map, IDs, units, shared errors, and the scale arithmetic the model's scales and the MIDI Tools share | none |
| `bl-commands` | A facade over the plug-in kit's `lspk-commands` (since 12 October 2026), with Beatlayer's reference wording: the command registry: each command's ID, label, description, permission class, contexts, default bindings, and argument and result schemas; the checks that keep them complete; the generated reference | none |
| `bl-model` | The document: tracks, clips, devices, automation, scenes, grooves. Commands, transactions, undo. | `bl-core`, `bl-commands`, `bl-stretch` (the warp map's arithmetic) |
| `bl-project` | Project files, schema versions and migrations, autosave, crash recovery, sample collection | `bl-model` |
| `bl-dsp` | DSP building blocks (the parameter smoother now from the kit's `lspk-params`, re-exported): filters, oscillators, envelopes, FFT, resampling, SIMD helpers | none |
| `bl-engine` | Compiler, real-time graph, scheduler, transport, clip players, mixer, PDC, automation and modulation playback, metering; restarting with a new processor when a device's stream closes and opens again | `bl-core`, `bl-model` (the compiler reads documents), `bl-dsp`, `bl-media` (clips' decoded audio), `bl-rt` |
| `bl-audio-engine` | The engine on an audio device, kept going through unplugging, sample-rate changes, stalls, and sleep (WP-1.13): the device supervisor's client that restarts the engine on each stream | `bl-engine`, `bl-audio-io` |
| `bl-devices` | Native instruments, audio effects, and MIDI effects, and the framework they're built on (WP-2.5) | `bl-dsp`, `bl-engine` traits, `bl-model` (parameter descriptors and layouts), `bl-midi-tools` (the arpeggio and strum laws) |
| `bl-stretch` | Warping: warp maps and every warp mode. The map's arithmetic works in plain numbers (seconds and ticks), so the document can use it | `bl-dsp` |
| `bl-midi-tools` | MIDI Tools: the tool interface (each tool's description and parameters, its input and output), the native transformations and generators, Beatlayer's seeded generator, the chord-bank format, and the arpeggio and strum laws the Arpeggiator and Chord devices share. Pure functions over notes, with no access to the document | `bl-core` |
| `bl-media` | Decode, encode, sample-rate conversion, peak files, disk streaming | `bl-dsp`, `bl-rt` (its logger, for the disk thread) |
| `bl-analysis` | Transients, tempo, key, loudness; ML features (stems, audio to MIDI) behind a feature flag | `bl-dsp`, `bl-media` |
| `bl-ara-sys` | The ARA 2 C API's types and function tables as Rust bindings, generated once from the ARA SDK's header (Apache-2.0) and committed, so nothing is fetched at build time | none |
| `bl-plugin-host` | CLAP, VST3, Audio Unit, and LV2 behind one trait; the scanner; editor windows; ARA 2 hosting for VST3 | `bl-engine` traits, `bl-media` (ARA's audio readers), `bl-ara-sys` |
| `bl-audio-io` | Audio device backends per platform behind one trait; the supervisor that keeps a stream running through device changes | `bl-rt` |
| `bl-rt` | A facade over the plug-in kit's `lspk-rt` (since 12 October 2026): the real-time thread harness: priority, audio workgroups, denormal flushing, the allocation trap, a lock-free logger, RealtimeSanitizer | none |
| `bl-midi-io` | MIDI device I/O, clock, MPE | `bl-core` |
| `bl-control` | MIDI and key mapping (the mappings' value laws, relative controllers, takeover), the control-surface host and its `ControlSurface` trait, instant mappings and device banks, Beatlayer's declarative controller templates, and the supported controllers (WP-7.1 to WP-7.10) | `bl-model`, `bl-midi-io` |
| `bl-push` | Push 2 as a control surface: its controls and LEDs, its modes, and its display, drawn into a 960 × 160 frame and sent over USB through `nusb` (WP-7.19 to 7.28) | `bl-control`, `bl-midi-io`, a 2D rasteriser; never a UI crate |
| `bl-link` | Ableton Link 4 session: tempo, phase, start and stop, Link Audio. Builds Link 4.1.1, vendored as source under `crates/link/vendor/` with standalone asio, through `cc`, and binds its C API (`abl_link`); the `rusty_link` bindings predate Link 4 (WP-7.14) | `bl-core` |
| `bl-interop` | MIDI files, Scala tuning files (`.scl`, `.ascl`), Ableton Live set import, DAWproject import and export | `bl-model`, `bl-project` |
| `bl-ui` | Views, widgets, GPU canvases, theming, accessibility | `bl-model`, `bl-shell`, `bl-media` (peaks for waveforms), `bl-midi-tools` (the tool descriptions the Transform and Generate panels draw their controls from), engine state types |
| `bl-shell-spec` | The app shell's commands and menu bar as data: each shell command's spec with its argument and result types, and the menu bar in Live's order. `xtask` reads them from here, so no `xtask` task links an audio backend | `bl-commands` |
| `bl-shell` | The app shell without a window: the shell's state and the one place its commands run (files, windows, views, focus, settings, the audio device, the transport, the browser), set files through `bl-project`, the audio host that runs the engine on the device with the browser's preview voice, the browser's database, and preferences, so every shell action can be driven and tested headless | `bl-commands`, `bl-model`, `bl-project`, `bl-shell-spec`, `bl-audio-io`, `bl-engine` (the engine and the preview voice), `bl-devices` (the native devices its engines run and the Device View draws), `bl-media` (previews and the media store), `bl-analysis` (measuring the agent API's audio previews, WP-1.16), `bl-midi-tools` (MIDI Tools) |
| `beatlayer` | The application binary: wiring, windows, menus, preferences | all |
| `bl-cli` | Headless tool: render, inspect, convert, benchmark, and serve the agent API (`bl-cli mcp`) | `bl-project`, `bl-engine`, `bl-devices`, `bl-shell`, `bl-agent` |
| `bl-agent` | The agent API: the MCP server over stdio and Streamable HTTP, its tools, permissions, checkpoints, and scheduled commands | `bl-commands`, `bl-model`, `bl-shell` (commands run where the UI's do), `bl-engine` message and state types. Perception's analysis runs on the shell, so the agent API reaches `bl-analysis` through it |
| `bl-assistant` | The built-in assistant: the agent loop and model providers (OpenAI-compatible, Anthropic, Google), with keys in the OS keychain | `bl-agent` |
| `xtask` | Checks, packaging, release. Links data crates only (`bl-commands`, `bl-model`, `bl-shell-spec`), never a UI crate or an audio backend, which `cargo xtask check` enforces | build-time only |

Crate names use the `bl-` prefix, for Beatlayer.

## The document

The document is an immutable, persistent tree (structural sharing through `imbl`). An edit produces a new root that shares every unchanged node with the old one. That gives four things almost for free:

- **Undo and redo.** The undo stack keeps old roots. Memory grows with the size of each change, not the size of the song.
- **Safe sharing.** The compiler, autosave, and analysis read an `Arc` snapshot on other threads while the main thread keeps editing.
- **Cheap change detection.** Pointer equality on subtrees tells the compiler what changed.
- **A path to collaboration.** Commands are serialisable, so a later version can replay them across machines.

Edits happen only through **commands** (`MoveClips`, `SetDeviceParam`, `InsertTime`). A UI gesture, such as a drag, opens a **transaction** that becomes one undo step: the shell runs the gesture's editing command again on each frame, measured from where the gesture began, inside that transaction, and commits it on release (`Shell::gesture`, WP-1.8b). Every object has a stable 64-bit ID that never changes and is never reused within a project.

Audio data never lives in the document. Clips refer to media by content hash, and the media store holds the samples.

Phase 6 adds to the document:

- **Take lanes and link groups** (WP-6.21, WP-6.23). An audio or MIDI track holds its main lane, the Arrangement clips it plays, and its take lanes, each with an ID, a name and clips of its own; every clip, on any lane or in a slot, is found by its ID through the document's index. Link groups of tracks are part of the document, and the commands themselves carry an edit across a link group, so a person's and an agent's edits link alike.
- **Time-placed objects** (WP-6.24b, WP-6.25). Everything placed in song time (clips on Arrangement lanes and take lanes, every track's automation envelopes, the Song Tempo, time-signature markers, locators and the arranger's sections) moves through two document commands, `InsertTime` and `DeleteTime`. The …Time commands, Insert Silence and moving an arranger section are built from them, and a new kind of time-placed object joins them in the same change. With Lock Envelopes off, an Arrangement clip edit carries its tracks' envelopes over the clip's span inside the edit's own transaction. Locators, the song start time, Lock Envelopes and the sections are song-wide settings the set saves.
- **Copies of tracks** (WP-6.52, WP-6.35). Copying a track, within a set (Duplicate, Copy and Paste) or from another set, is one function: every object gets a new ID, references between the copied tracks follow them, and references to tracks outside the copy are kept within a set, or settled when the copy comes from another.
- **Linked clips** (IMP-21, WP-6.44). An edit to a linked clip's contents reaches every clip of its link in the same transaction.
- **Who made each step** (WP-6.28). Each undo step records who made it (the person, an agent by name, or the assistant), and a jump through the history is not a step of its own.
- **Colours and info text** (WP-6.30b, WP-6.58). Tracks, clips and scenes carry a colour (a swatch, or from WP-6.50 a #RRGGBB colour), and a colour edit compiles to nothing in the engine. Tracks, clips, scenes, devices, chains, macros and locators carry the user's own info text, which the Info View shows in place of the registered description.
- **Scale and tuning** (WP-6.7, WP-6.9). Each clip carries its scale; the set's current scale and its tuning are song settings.

### Musical time

| Quantity | Type | Notes |
|---|---|---|
| Musical position and length | `Ticks(i64)` at 705,600 ticks per quarter note | 705,600 = 2⁶ × 3² × 5² × 7², so 1/64 notes and triplet, quintuplet, and septuplet grids land on whole ticks. Edits and comparisons are exact. |
| Device position | `Frames(i64)` | At 48 kHz, i64 frames cover millions of years. |
| Real time | `f64` seconds | Only inside conversions. |

The tempo map is a list of segments, each constant or a linear ramp, with time-signature changes alongside. Conversions between ticks and seconds integrate the segments exactly. Property tests check that the round trip holds.

## The engine

### Compiling the document into a program

The engine never reads the document. The compiler turns a snapshot into an `EngineProgram`: a flat array of nodes, a buffer plan, an execution schedule, the delay lines for plug-in delay compensation, and references to clip data. Compilation runs on its own thread. The finished program goes to the audio thread through a lock-free queue, and the swap happens at a block boundary.

Nodes carry state that must survive a recompile: a reverb's tail, a delay buffer, a plug-in instance. Node instances are created and activated off the audio thread. At the swap, the audio thread moves surviving instances from the old program into the new one. That's a pointer move, with no allocation. The old program, with any removed nodes, goes to the drop thread to be freed.

How that works, as WP-0.9 built it:

- **Node keys.** Each node has a key that outlives compiles: a device's ID, or a track's ID for its mixer, its output, and (on an audio track) its clip player (WP-1.4), or a track's and a return's for a send (WP-1.6). The compiler remembers where each key sat in the last program it compiled. A node whose key and kind (a device's type) are unchanged isn't made again: its slot in the new program stays empty, and the program lists the slot it moves from. Programs therefore reach the audio thread in the order they were compiled, none skipped; when the queue is full, the sender holds messages back in order rather than dropping them.
- **One queue, in order.** Programs, parameter changes, node data, transport commands, and scheduled commands share one `rtrb` queue, so a parameter change never overtakes the program that holds its node. Its one sender is the compiler thread; the main thread's requests reach it over a channel, and documents sent back to back are compiled once, as the newest.
- **Deferred drops.** Each engine has its own `rtgc` collector. A program is wrapped for it when it's compiled, so dropping it on the audio thread only decrements a reference count; the drop thread collects every 10 ms.
- **A switched-off device** recompiles, but its node migrates: it keeps its state and takes parameter changes (through `Node::flush`, as CLAP's `flush` does) without processing.

Most edits never recompile:

| Change | Route to the engine |
|---|---|
| A parameter value, mute, solo, Solo in Place, volume, pan, a send's level | A parameter event on a lock-free queue, applied at a sample offset |
| A clip made, moved, resized, or deleted; its markers, loop, gain, fades, or activator; later its notes or warp markers | The track's clip list is swapped in on its own, as node data |
| A routing, device chain, or track list change, or a return's Pre/Post switch | Recompile the affected part of the program |

### Device chains

How a track's devices run, as WP-2.4 built it ([the device chains spec](specs/device-chains.md)):

- **The chain in the document.** A track's devices are a list in signal order, each with its kind, its role (MIDI effect, instrument, or audio effect), its names, its activator, and its parameter values. The model keeps Live's order (MIDI effects, at most one instrument, audio effects; MIDI effects and instruments only on MIDI tracks) and refuses a command that breaks it.
- **In the track's task.** The chain runs in place on the track's buffer after the clip player and the inputs and before the mixer; a switched-off device's node is skipped but takes parameter changes through `Node::flush`.
- **What recompiles.** Inserting, deleting, moving, or switching a device off or on compiles a program in which only inserted devices' nodes are new; every other node migrates. Renaming, Show Preset Name, parameter values, and loading a preset send parameter changes and no program.
- **The device interface.** A `NodeFactory` lists the device types it offers (`DeviceType`: kind, own name, role) and makes a `Node` for a device of one of them, set to its parameter values; the node processes blocks in place, takes changes at their frames, and reports its latency for delay compensation (WP-2.11). Native devices (through `bl-devices`' framework, WP-2.5) and the plug-in hosts (CLAP since WP-2.6; VST3 and Audio Units with WP-2.7 and 2.8) implement it.
- **Presets** are `bl-project`'s: a device's kind, role, own name, and parameter values in a `.blpreset` file of Beatlayer's own (JSON, schema-versioned), written whole or not at all. A plug-in's own state is saved with the set (WP-2.6), and in device presets from WP-6.66. A rack or one of its chains is schema 2 of the same format: the rack as the set holds it, with its devices' state, plug-ins' own state, and the samples it plays inside the file (WP-5.8).

### Scheduling and multicore

Each track's device chain is sequential, so the unit of parallel work is a chain segment between merge points. The schedule is a dependency graph of those tasks. Each block, the audio thread and the RT worker pool take ready tasks from a lock-free queue. When a task finishes, it decrements its dependents' counters. Workers spin briefly, then sleep on a semaphore, never a mutex.

Without a worker pool, the audio thread runs the tasks alone, in a topological order the compiler works out (WP-0.9). Each task is one track's chain and mixer, run in place on the track's buffer after the buffers that feed it are summed into it, and it lists the tasks waiting on it and how many it waits on, which is what the pool needs. A task reads the buffers that feed it and writes only its own, so tasks that don't depend on each other can run at once: a return pulls its sends from the sending tracks' buffers rather than having them pushed in.

How WP-5.37 built the pool ([the racks spec](specs/racks.md) R15):

- **The pool** (`bl_engine::Pool`) is made where allocating and waiting are allowed, with the stream, and handed to the `Processor`. The app's `AudioHost` and `bl-audio-engine`'s supervisor start one with every stream they open, from the stream's output workgroup (the supervisor passes it to its client) and stop it with the stream, so a device change gives the new stream a pool of its own, joined to the new device's workgroup. It has as many workers as the workgroup says it can run at once, less the audio thread, or, where there is no workgroup, the machine's cores less one (these platforms don't say which cores are the fast ones). A stream on the null device gets none unless it is asked for. `Offline::with_workers` runs a render on a pool without a device, and `Offline::new` still renders on the one thread.
- **Workers.** Each is a thread prepared with `bl-rt` (`prepare_current_thread`: real-time priority, the workgroup, the thread-locals the allocation trap and the logger use) before it reports ready, and leaves the workgroup (`release_current_thread`) before it ends. It runs each piece of work as a real-time section (denormals flushed, the allocation trap counting, RealtimeSanitizer watching) and leaves it to sleep. A worker that finds nothing to take spins for a short while, then sleeps on a semaphore (a POSIX `sem_t` on Linux, which a real-time section may post; Apple's and Windows' thread parking elsewhere) that the next piece of work, or the next block, wakes. The audio thread never sleeps: it takes work and spins until the last task has finished, and a pool whose workers are all asleep only costs the speed-up. When it has finished a block it waits until no worker is inside the program, so a program is swapped or freed only when none holds it.
- **The queue and the counters** belong to the program. `Sched` holds a bounded lock-free queue (an array queue, with room for every task and every rack chain), a counter of the tasks each task still waits for, and the count of tasks left. A thread that finishes a task decrements its dependents' counters, runs the first one that reaches zero itself, and queues the rest.
- **Segments.** A track's task is a segment, and so is each chain of a rack in it, at every depth. The thread that reaches a rack copies its input into the chains' buffers, offers the other chains to the pool as tickets in the same queue, takes chains itself until none is left, waits for the chains that others took, and then adds the chains' buffers into the rack's in the chains' order, so a rack's output doesn't depend on which chain finished first. A MIDI effect rack's chains leave cursors on their notes, which are merged in the same order.
- **What each thread keeps to itself** is a `Lane`: the parameter changes handed to one node, and the scratch a delayed route and an interface input are mapped through. They used to be the program's, shared by the tasks one after another; each worker and the audio thread make their own with the thread. Each task has a pair of note buffers of its own, and each rack chain another.
- **Handing out the parts of a program.** Everything a run writes belongs to one task: its nodes and routes, its buffers, its delay lines, its note buffers, its track's MIDI input. `Shared` (in `bl-engine`'s `disjoint` module, the only place the engine hands out references to shared storage, with its tests) holds them, and a `Turn` (the thread's hold on one task, in `program.rs`) is the only code that asks it for a part. What a task reads from another is that task's finished buffer or the copy of its signal it kept, which the dependencies make final first.
- **Determinism.** A summing point adds its inputs in the order the compiler lists them, whatever finishes first, so a render is the same to the bit with and without a pool, at any block size and with any number of workers: the golden fixtures and a 128-track set are rendered both ways in the tests.
- **A panic** in a task or a chain on a worker is counted (`Pool::panics`) and the block carries on; the task's audio is left as it was. A panic on the audio thread is handled the same way, so the workers are never left waiting for a task that didn't finish.
- **Plug-ins.** A plug-in's node is a node of one task, run by one thread at a time, on whichever thread takes the task. The CLAP host doesn't implement the `thread-check` extension, so there is no answer to give about which thread is the audio thread.
- **The CPU meter** (the manual's 38.1.1) still measures the block's time on the audio thread: from the start of the block to its end, the wait for workers included.

On macOS, workers join the output device's audio workgroup, so the OS schedules them as real-time work on performance cores. On Windows they register with MMCSS ("Pro Audio"). On Linux they set `SCHED_FIFO` directly where the process's rtprio limit allows it, and otherwise ask rtkit. Beatlayer writes this layer itself, in `bl-rt`. Beatlayer's own threads do this set-up (`prepare_current_thread`) before they start real-time work, when it may allocate and wait for rtkit. The audio callback's thread usually belongs to the audio API, and Beatlayer's first code on it is a device callback with its deadline running, so its set-up (`AudioThreadSetup`) is prepared when the stream opens and does on the thread only what has to happen there (real-time rule 7). Core Audio's I/O thread is already real-time and already in its device's workgroup.

### Plug-in delay compensation

Each node reports its latency. At every merge point, the compiler delays the earlier inputs so all paths line up, and it shifts automation and clip playback by the same amount. A device's sidechain input is a merge point too: its source is delayed, or the device's own input is, so the detector hears the two in time and ducking lands where it would with no latency. A latency change from a plug-in triggers a recompile. A **reduced latency when monitoring** option bypasses compensation on armed tracks, as Live does.

### Parameters, automation, and modulation

Every automatable parameter has an ID, a range, a unit, and a default. Its value in each block is the base value (static or from automation) plus the sum of its modulation sources, clamped to the range; the sum is taken in the parameter's travel (its position on the control's scale, where a level's, a pan's and a logarithmic frequency's differ), so a modulation is a share of the travel and acts on top of wherever the base is (the modulation spec). Automation and modulation are evaluated as sample-accurate events, which CLAP and VST3 plug-ins accept directly. Live 12's split between automation (absolute) and modulation (relative, on top) maps onto this model.

How WP-2.5 built it for native devices ([the device framework spec](specs/device-framework.md)):

- **Descriptors** are `bl-model`'s (`param`): each parameter's number, name, kind (continuous with its range, unit, and taper; a gain; a switch; a choice; a whole number), default, and automatable flag, with its control's travel, how its value reads, and how typed text parses. A device type (`bl_engine::DeviceType`) carries its `DeviceDescription`: the descriptors and its layout. A gain holds a linear factor from 0, so −∞ dB is a value the document can hold (rule G1); a description with a parameter that can't be automated is refused when it registers (rule G2).
- **On the audio thread** (`bl-devices`), a `NativeNode` wraps each `NativeDevice`: it splits every block at its parameter events, so each lands on its frame, and keeps a `ParamBank` that clamps each value and moves continuous ones and gains in a straight line over 20 ms (`bl_dsp::LinearSmoother`), frame by frame; the device reads each parameter's value per frame. Every buffer is made with the node. `bl_dsp::Oversampler` runs a stage at 2, 4, or 8 times the rate and reports its latency in whole frames.
- **The layout model** (`bl_model::device_layout`, IMP-14) is bands of controls and display cells in rows and columns. `DeviceLayout::place` lays it out at any size of at least its docked size, giving extra room only to the cells; the Device View places it at its docked size, and WP-2.5b's floating windows at theirs.
- **The generic UI** (`bl_ui::device_panel`) draws any described device from `device_view.state`'s panel, and runs `device.set_param` for each change; `device.parameters` gives agents the same descriptors.

How WP-5.10 added modulators ([the modulation spec](specs/modulation.md), R12 to R23):

- **A modulator is a device that works before the tracks do.** The compiler lists the program's modulators in song order. Before the tasks run, `EngineProgram::run` calls each modulator's `Node::control` over the block's frames, with the block's parameter events; the device writes, for each of its eight slots, two rows of one value per frame on the program's bus (`bl_engine::modbus`): what the slot contributes to its target in Modulation mode and the position it holds the target at in Remote Control mode. A modulator's output is worked out on a 32-frame grid counted from the transport's start and ramped between grid points, so the rows are the same at any block size, and the random parts are seeded per device and reseeded when the transport starts.
- **Targets read the bus.** The compiler gives each target node, on any track, the list of bus rows that map each of its parameters (`AutomationList`); `ParamBank::advance`, the mixer and a send's route add the rows to the clips' modulation in the parameter's travel, and a hold replaces the base. A mapping edit sends the affected lists as node data without a new program. A plug-in's driven parameters read the same rows.
- **Mappings are the document's.** A mapping lives on its modulator (`Device::mappings`), names its target by track, send or device, goes with its target when it is deleted, and follows a device that moves.

How WP-5.11 added modulators driven by audio, notes and MIDI (the modulation spec, R24 to R29):

- **They work their output out in their track.** The Envelope Follower hears the audio at its place in a chain, the Gate, Strike and Play Mods the notes and MIDI; so they run as nodes in the chain (`ModKind::Signal`), not in the pre-pass, and write their rows into the program's bus through the block (`Block::row_mut`) as they go. The pre-pass idles their rows each part. Their output follows the same 32-frame grid, one step behind its level, so it doesn't depend on the block size.
- **Waits and the lag.** `Graph::modulator_edges` (the routing graph now lives in `bl-model`, so `modulator.map` can refuse a loop) lists the tracks that wait for a signal modulator's, after every route and with a loop left out; the compiler adds them to the task graph (`run_order`). A target earlier in its modulator's own chain reads a second set of rows (`ModRows::late`) that the modulator fills before each part from what it wrote 32 frames before, and a program that has one runs in parts of at most 32 frames (`Processor::until_next_event`), so the lag is the same at any block size.

The Arrangement draws a track's automation in its own lane, over its clips, and in any number of lanes of its own under it (WP-6.57); which parameter each lane shows is view state the set keeps, not part of the automation.

### The mixer

How the mixer works, as WP-1.6 built it ([the mixer spec](specs/mixer.md)):

- **A track's task.** It sums its inputs, runs its device chain, then its mixer (`TrackMixer`: pan with Live's constant-power law, volume, and the Track Activator, then the track's peak and RMS meters), all in place on the track's buffer. Its inputs are routes (`Route`), one per feeding track: each member's output into a group, each top-level track's and return's into Main, and every sending track's send into a return. A route's gain is a send's level, or 1 for an output, unless solo closes it.
- **Pre-fader sends.** A track whose sends feed a return switched to Pre keeps a copy of its chain's output before its mixer, and those sends read it; post-fader sends read the track's buffer after its mixer. The Pre/Post switch is the one mixer setting that recompiles.
- **Solo on the main thread.** The compiler works out who solo silences (`Solo`: the solo path, Solo in Place, soloed returns) whenever a solo switch, Solo in Place, or the track tree changes, and sends parameter changes to the outputs and sends whose state changed. The audio thread never walks the track tree.
- **Glides.** Every mixer gain moves to a new value in a straight line over 20 ms from the frame the change takes effect, one frame at a time in the same order whatever the block size, and lands on the value exactly. A gain at rest at 1 leaves samples untouched, so a song with every mixer at rest renders as it did before the mixer.
- **State that survives.** Mixers and routes are nodes with keys, so they migrate across recompiles with their glides and meters; the meters are published with the playhead after every block (`EngineState::meters`).
- **MIDI meters** (WP-6.69). Every MIDI meter point (a MIDI track's chain end, every device boundary) has a slot made with the program; after each block it publishes the highest note-on velocity that passed, whether a channel message passed and whether a per-note expression passed, beside the audio peaks at the boundaries after the instrument ([MIDI meters](specs/midi-meters.md)).

### Clip playback

How the Arrangement's audio clips play, as WP-1.4 built it ([the audio clips spec](specs/audio-clips.md)):

- **A player per audio track.** Every audio track's chain starts with a clip player (`ClipPlayer`, key `NodeKey::Player`), which adds the track's clips to its buffer while the song plays. It migrates across recompiles like any node.
- **Clips as node data.** The compiler turns a track's clips into a list for its player: each clip's start and end in song frames, through the tempo map; the file frames its Start and End markers and loop brace name; its gain; its fades in frames; and its audio at the engine's rate. The list travels on the queue as node data, so making, moving, or editing a clip never builds a program. A sync sends a list to each track whose clips changed, and a tempo change, which recompiles, sends every list again under the new map.
- **Media.** The registry carries a media library (`MediaLibrary`), which opens a source (`bl_media::AudioSource`) for each clip: `MediaStreams`, what the app uses, checks each file's content and streams it from disk, or holds it in memory if it's shorter than 10 seconds; the media store alone decodes files whole, and a `MediaMap` holds audio made in memory for tests and previews. Media it can't supply plays silence and is reported. Opening a source reads the disk, so the compiler thread never does it on a running engine: a loader thread does, and the clip joins its track's list when its source is ready, so a long file never holds up Play or Stop (WP-1.4b). Each list carries the sources opened for it in a lock-free handoff; the player carries the sources of the clips it keeps across from its old list, and the old list takes back the rest, to be freed on the drop thread. An offline render opens sources on its own thread and reads streams in blocking mode.
- **Unwarped clips** ([warping](specs/warping.md), [warp modes](specs/warp-modes.md)). A clip with Warp off plays a file frame to each song frame, whatever the tempo; its markers and brace are positions in the file in seconds, as Live shows an unwarped clip. Arithmetic that decides a sample is integer frames and one `f64` product per sample, so renders are the same at any block size and on every platform. Only a warped clip loops (a looped unwarped clip is an older set's, and keeps its loop).
- **Warped clips** (WP-4.2). A warped clip's markers and brace are ticks of its contents. For each warped Arrangement clip the compiler works out a table from song frames to file positions (`WarpTable`, in `bl-stretch`): the clip's warp map under the tempo map, through its Start marker, End marker and loop. The table is a list of straight pieces (a piece for each tempo change, warp marker and loop pass, and more where the tempo ramps, so it stays within a hundredth of a frame of the exact position), and it travels with the clip list, so a warp edit sends data and no program. The player finds the position of a song frame from the table alone, so a render is the same at any block size; for a stretch over which the position advances by exactly one file frame to each song frame it reads the file frame for frame, and elsewhere it reads a window of the file around the stretch and hands it, with the positions, to the clip's warp mode (`bl_stretch::WarpMode`), through the interpolator HiQ chooses (a long windowed sinc, or a short one). A clip launched from a Session slot has no place in the Arrangement to tabulate for, so it follows the tempo map from its launch frame, frame by frame. A streamed clip keeps the last frames it read, so the overlap between one block's window and the next reads forwards only, and switching a clip's RAM mode hands it from the old source to the new one once the new one holds the audio from where the clip has reached (`clip_source.rs`). Re-Pitch is the mode built so far; the other five play as Re-Pitch until their packages (WP-4.4 to WP-4.6).
- **MIDI clips** (WP-2.1, WP-6.2). A MIDI track's chain starts with the MIDI clip player, which reads its clips' notes and controller envelopes as node data. Each note's chance and velocity deviation roll from the player's seeded generator, so renders repeat. Each clip's probability groups are made when the clip list is built. A group's roll for a pass of the clip (its first play, each loop repetition, each launch, transport start or locate) is worked out from the player's seed, the pass and the group, so every note of the group reaches the same answer, the audio thread keeps and allocates nothing for it, and a render is the same at any block size.
- **Take lanes** (WP-6.21). A track's player gets its main lane's clips or, while one of its take lanes is auditioned, that lane's in their place; switching audition sends the list as data, with no new program.
- **Fades and crossfades** (WP-6.51). A clip's fades, each a length and a curve, and its crossfades with the clips it meets travel with the clip list; across a crossfade each clip is read past its edge from its own source, whose cues preload the crossfade's start.
- **Grooves** (WP-6.19). A clip's groove and the set's Global Amount travel with its track's clip list as node data. The MIDI clip player moves and accents each note as it schedules it, with Random from a generator seeded per clip and restarted when the transport starts; a warped audio clip's table is built through the groove's displacement, so every warp mode follows it ([grooves](specs/grooves.md)).
- **The current scale and the tuning** (WP-6.8, WP-6.9). A track's current scale reaches its devices in each block from the clip playing on it (its Session slot, or the Arrangement clip under the playhead), or else the set's current scale; it travels as data in the clip lists through a `Block` accessor, never a recompile. The set's tuning reaches each instrument on a track that plays it as node data, the tuned pitch of every key; tracks that bypass the tuning, and Drum Rack tracks, get none.
- **Nothing freed on the audio thread.** When a new list replaces the player's, or the program holding the player goes, the audio thread only lets go of the old list: the drop thread frees it, with the last references to its media.

### Transport and clip launching

The transport owns the song position, tempo, loop, and the launch quantisation grid. A command scheduled at a musical position (D17) reaches the engine as a scheduled command: the engine splits the block where the song reaches it, runs its action there (a parameter change, a stop or locate, or nothing but the report, for a command whose effect is on the document), and reports the frame it fired on (WP-0.9).

How the Arrangement's transport works, as WP-1.5 built it ([the transport spec](specs/transport.md)):

- **On the audio thread.** The processor keeps whether the song plays, the song position (the next frame to play, which stays where playback stopped, for Continue), the insert marker (where Play starts), and the loop in ticks and in frames. Play, Stop (twice to return to 1.1.1), Continue, Play/Stop, locate, and the rest are `TransportCommand`s on the one queue, so they arrive in order with programs and parameter changes, and the audio thread decides each against the state it's really in, with no race against the UI.
- **On an exact frame.** A transport command takes effect at the next block or on a frame of the engine clock. The processor splits its block wherever the transport acts (a command's frame, a scheduled command's song position, the loop's end), so every node's block is one unbroken stretch of the song, and a start, stop, jump, or wrap lands on its sample at any block size.
- **Following the tempo map.** Positions in ticks fall on frames through the running program's tempo map. A new map keeps the playhead on its beat and moves the loop's frames with its bars. Nodes see the map (`Block::tempo`), so a node can place its output in ticks, as the beat tone (`beatlayer.test_beat_tone`, the transport's golden-test sound source) does.
- **The loop is in the document.** The brace and the Loop switch are song settings, saved with the set and undoable. The compiler turns a loop edit into a transport command rather than a program, and sends the loop with every program.
- **For the UI.** After each block the engine publishes the playhead in frames and ticks, whether it plays, the insert marker, and the loop through its triple buffer, all from the same block.
- **The held scrub loop** (WP-6.59). Beside the Arrangement loop the engine keeps a held scrub loop, which the scrub area sets while the mouse is held; it wraps on exact frames as the loop does and is never saved.

Session view clip launching runs in the engine, not the UI, because launches must land on the exact sample. Each clip slot has a state machine (stopped, queued, playing, queued to stop, recording). Follow actions use a random number generator seeded per clip, so an offline render plays the same choices every time.

### Offline rendering

An offline render (export, previews, golden tests) runs the engine with no device and no threads (WP-0.10). `bl_engine::Offline` holds a compiler, the queue, the processor, and a collector on one thread, and runs the same `Processor` a device's audio thread runs, block after block, as fast as it can. Between blocks it does what the other threads would: it compiles and sends documents, and frees what the block let go of. Each block is a real-time section, as on a device, so denormals flush the same way and the allocation trap counts heap use in debug and test builds. Nothing in it depends on timing, and parameter changes at a frame and scheduled commands land on their exact frames whatever the block size, so the same song renders to the same samples at any block size, run after run. `bl-cli render PROJECT OUTPUT.wav` plays a set from its start through it.

Exporting audio (WP-1.12, [the export spec](specs/export.md)) runs the same `Offline` engine over a range of the song: it locates to the range's start, switches the Arrangement loop off, and plays to the range's end. At a file rate below the engine's it renders at the engine's rate and converts down as the render arrives (`bl-media`'s converter, a chunk at a time, exactly as converting the whole render would); at or above it, the engine renders at the file's rate. Normalize renders to a scratch file and applies its gain on a second pass. `bl_media::AudioWriter` dithers (rectangular, triangular, or Beatlayer's own noise-shaped dither, from a seeded generator, so exports repeat bit for bit), rounds as the golden references round, and encodes WAV, AIFF, or FLAC a block at a time, into a hidden file beside the destination that's renamed into place only when complete, so a cancelled or failed export leaves nothing. `export.audio` runs it on the shell; `bl-cli export` runs that command headless. In the app an export runs on a worker thread from a snapshot of the set, one at a time, with its progress and Cancel in the Export Audio/Video dialog's progress window, whether a person or an agent started it (WP-6.71). An export of a golden fixture nulls against its reference to the bit. The render loop itself, `bl_shell::export::render` (the range located, the loop off, the blocks, the conversion down), is shared: the agent API's audio previews (WP-1.16) render through it into memory.

The browser's previews of MIDI clip files and instrument presets render offline in the background, with plug-ins made from their saved state and never the set's own instances, and play on the preview voice (WP-6.34b, WP-6.67).

### Real-time rules

These apply to the audio thread and the RT worker pool, and tests enforce them:

1. No allocation or deallocation. Debug and test builds run each audio block inside `bl-rt`'s allocation trap, so a stray allocation fails the test (`rt_test!`) or the stream (`bl-cli tone`). `cargo xtask rtsan` runs the real-time tests under LLVM's RealtimeSanitizer, which also catches locks, blocking calls, and allocation by C code, the system's included; CI runs it on Linux. Its harness, `rtsan/`, is a workspace of its own that links a runtime already installed (`RTSAN_LIB_PATH`), so no crate that fetches or builds LLVM is in the main dependency graph. RealtimeSanitizer sees a Rust `Mutex` on Linux only when it has to wait, because an uncontended futex never leaves user space, and LLVM 20's doesn't stop `std::thread::sleep` on Linux, which calls `clock_nanosleep` there (macOS runs catch it, through `nanosleep`). On macOS a thread's first touch of a thread-local makes the system `malloc` its storage, so code that runs on an audio API's thread uses none in release builds. The trap's own counters are thread-locals, so in debug builds that one allocation happens in a new audio thread's first block, where the trap can't see it.
2. No locks. Use atomics, single-producer queues (`rtrb`), and triple buffers (`triple_buffer`). Objects the audio thread drops go to the drop thread through `rtgc`.
3. No blocking system calls: no file I/O, no printing, no logging. Log through `rt_log!`, which queues a fixed-size record on a lock-free ring that a logger thread drains. Code on a thread the audio API owns logs to a `Writer` made before the stream starts.
4. Bounded work per block. Queues are sized for the worst case; on overflow, drop the message and count it.
5. Denormals are flushed to zero for every block (FTZ and DAZ on x86, the FZ bit on Arm). The previous setting comes back after the block, because the thread may belong to the audio API.
6. A panic in a native device is caught per node. The node outputs silence and reports the failure, and the song keeps playing.
7. One narrow exception: per-thread set-up that the operating system requires on the audio thread itself happens once per thread, before that thread's first block, allocates nothing, and contains nothing that can block. Everything it needs is allocated when the stream opens. On a thread the audio API owns, it takes the place of the thread's first block, which plays silence, and it runs as a real-time section like any block. It is, on macOS, reading the thread's scheduling policy, raising it with `thread_policy_set` if it isn't real-time (Core Audio's I/O thread always is), and `os_workgroup_join`, which applies only to the calling thread (on Core Audio's I/O thread it returns `EALREADY`); on Windows, reading the thread's priority and registering it with MMCSS (`AvSetMmThreadCharacteristicsW`), which applies only to the calling thread; and everywhere, checking that denormals flush. Whatever can block leaves the audio thread. On Linux the audio thread only publishes its thread ID through an atomic, and a helper thread raises it with `sched_setscheduler` or, failing that, rtkit's D-Bus call, both of which take a thread ID. A thread that joined a workgroup leaves it through a thread-exit hook (a pthread key's destructor, set without allocating), because macOS stops a process whose thread exits inside a workgroup and an audio API ends its threads without telling Beatlayer. The tests run this set-up inside the allocation trap, and under RealtimeSanitizer on a thread made with `pthread_create`, from that thread's first instruction.

On Linux without rtprio limits, real-time priority comes from rtkit, which grants it only to a process whose `RLIMIT_RTTIME` hard limit is at most its `RTTimeUSecMax` (200 ms by default). Before its first request, Beatlayer lowers the process's limit to that value. The limit covers the whole process and a lowered hard limit can't be raised again, so from then on the kernel ends the process if any of its real-time threads, plug-ins' threads included, runs that long without blocking. That protects the desktop from a runaway thread, and rule 4 keeps Beatlayer's own threads far below it, but a plug-in that spins at real-time priority for 200 ms takes Beatlayer down with it; running plug-ins in a separate process (Phase 10) is the remedy. Where the rtprio limit allows `SCHED_FIFO` directly, rtkit isn't asked and the limit isn't touched.

## Audio and MIDI I/O

`bl-audio-io` hides each platform's audio stack behind one trait with full-duplex streams: input and output in one callback, which recording with low-latency monitoring needs.

| Platform | Backend | Built on |
|---|---|---|
| macOS | Core Audio, including aggregate devices, with workgroup joining | Beatlayer's own code over `objc2-core-audio` |
| Windows | ASIO | `azo` (MIT; implements ASIO's COM interface, so no Steinberg SDK is downloaded at build time) |
| Windows | WASAPI shared and exclusive | `cpal` for shared; `wasapi` for exclusive |
| Linux | PipeWire, JACK, ALSA | `cpal` |

As of October 2026, `cpal` 0.18 has no duplex streams on Core Audio or PipeWire and no WASAPI exclusive mode; its 0.19 plan covers them. Until then a `cpal` stream that needs input opens a second `cpal` stream for the input on the same device and carries its samples to the output callback through a lock-free ring made when the stream opens (WP-2.13); both callbacks run on the one device's clock. Input and output must be the same device; an aggregate of several devices waits for the in-house Core Audio backend. Beatlayer owns the trait, so a backend can move to `cpal`'s own duplex streams when 0.19 ships without touching the engine.

Until the in-house Core Audio backend lands, macOS also runs on `cpal` (WP-0.3, duplex since WP-2.13). On Linux, `cpal`'s native PipeWire host is off for now, because a build-time dependency of it is licensed Apache-2.0 WITH LLVM-exception, which `deny.toml` doesn't list; PipeWire plays through its ALSA plug-in and its JACK server meanwhile. Whatever the backend, the callback sees planar `f32` blocks of at most 4,096 frames, and a null backend runs the same path on a timer for tests and CI. A stream that doesn't ask for a sample rate opens at the device's current rate, because opening a Core Audio device at another rate changes the device's own setting, which outlives the app. Each stream counts dropouts three ways: xruns the backend reports, breaks in the device clock, and callbacks that overrun their block.

### Device changes

Hardware changes under a running stream: an interface is unplugged, another program sets the device to another sample rate, a driver stops calling back, the machine sleeps (WP-1.13; the rules are in [the device-changes spec](specs/device-changes.md)). A `Supervisor` in `bl-audio-io` owns the stream on a thread of its own and watches it every 50 ms:

| Event | How it's noticed | What happens |
|---|---|---|
| Device gone | The backend reports it, or a stream it invalidated is on a device no longer listed | The stream closes; the device is tried again every second until it's back |
| Sample rate changed | The backend invalidates the stream, and the device now runs at another rate | The stream reopens at once at the new rate, which Beatlayer then follows rather than set back |
| Stream stalled | No frames for three seconds, whatever the backend says | Treated as dead: closed and reopened |
| Sleep and wake | A `PowerMonitor`: by default the jump between the monotonic and wall clocks, or the gap between polls, noticed after waking | A stream that stopped is reopened; one that ran through is left alone. A monitor fed by the operating system's sleep notifications would also close the stream before sleep |
| Default device changed | The backend reports it | The stream follows the default (Core Audio does); only reported |

The supervisor says what happened through its status and events, with a sentence for the Status Bar's message area. Falling back to the system's default output while the chosen device is missing is an option, off by default so audio never jumps to the computer's speakers unasked.

What runs in the stream is a `StreamClient`'s: each time a stream opens, the client makes its callback for the rate and block size the stream actually runs at, and it reaches the audio thread through a one-slot `rtrb` queue that moves one pointer. `bl_audio_engine::AudioEngine` is that client for the engine, in a crate of its own so that `bl-engine` links no audio backend (`xtask` links the engine for the transport's commands); it restarts the engine through `bl_engine::Restarter`. When a stream closes, its callback hands the processor back through another one-slot queue as it's dropped; the processor publishes a stopped transport, and hands on the transport WP-1.5 gave it (the song position and the insert marker in beats, and the Arrangement loop), the engine clock, the scheduled commands that hadn't fired, and the transport commands waiting for their frames. While no stream runs, the compiler thread keeps the newest document without compiling it, so a device away for hours costs nothing, and holds transport commands with their times (except any that would start the song), scheduled commands, and cancellations, in order, for the next processor. When a stream opens, the compiler thread makes a new processor for its settings, compiles the newest document for it with every node made afresh, has it take over the transport at the new rate (the song stopped where it was, on the same beat, the loop on the same bars, the clock carrying on, and each waiting command as long after the restart as it was after the stream closed), applies what it held, and drains the new queue before the processor reaches the audio thread, so its first block plays the song and the UI's state shows it at once. The engine then reads the new processor's state.

`bl-midi-io` uses `midir` for device I/O and the `midi2` crate's message types, so MIDI 2.0 can follow without a model change. Output to hardware is scheduled (WP-5.34): the engine stamps each message with the moment its frame is heard at the output and queues it for the MIDI output thread (`bl_midi_io::OutputBus`), which hands it to the port with that stamp: Core MIDI with a host time stamp on macOS, the ALSA sequencer's queue on Linux, and a timer thread that sends within a millisecond on Windows, since `midir` sends immediately. MIDI clock out (WP-7.11) is stamped the same way: the engine writes each clock, Start, Stop, Continue and Song Position Pointer for the frame it belongs to, ahead of time, so the clock's spacing is the output bus's accuracy, never a timer's; Windows MIDI Services need more of Beatlayer's own backends later, and `midir` has no MIDI 2.0 support. Incoming clock and MIDI Timecode (WP-7.12, WP-7.13) reach sync, not tracks, and the engine follows them through the transport ([MIDI clock](specs/midi-clock.md), [MIDI Timecode](specs/midi-timecode.md)); Link's timeline is captured and committed on the audio thread once per block ([Link](specs/link.md)). MPE is carried end to end as per-note expression: an input port with its MPE switch on is read by zone (the MPE Configuration Message and each zone's bend sensitivity), each member channel's pitch bend, controller 74 and pressure becoming the held note's own pitch, slide and pressure with a note ID of its own; inside the engine a per-note pitch is a share of 48 semitones. On the way out, one MPE MIDI layout (member-channel allocation, each note's starting values before its note-on, the zone's master channel for channel messages) serves Audio Units, plug-ins without a per-note route of their own, and MIDI To ([MPE](specs/mpe.md)).

## Plug-in hosting

`bl-plugin-host` wraps CLAP, VST3, Audio Unit (macOS), and LV2 (Linux) behind one `PluginInstance` trait, the same trait native devices implement.

| Format | Built on | Notes, October 2026 |
|---|---|---|
| CLAP | `clack-host` 0.2 | Feature-complete; its API can still change, so pin it |
| VST3 | `vst3` 0.3 (Coupler) | The VST3 SDK has been MIT-licensed since version 3.8 (October 2025), and the bindings ship pre-generated |
| Audio Unit v2 and v3 | `objc2-audio-toolbox`, `objc2-avf-audio` | `rack` is a reference for AU hosting in Rust |
| LV2 | Beatlayer's own host on the system's lilv (WP-5.39) | The `lilv` C functions it calls are declared by hand and loaded when the host starts, so there is no new crate and a machine without lilv runs Beatlayer with LV2 unavailable; Linux only |

- **Scanning** runs in `beatlayer-scan`, a child process with a timeout. A plug-in that crashes or hangs during a scan goes on a blocklist, and the main app never loads it. Results are cached by path, modification time, and format.
- **Per-note expression** (WP-6.17b): a plug-in device's MPE Mode decides whether per-note pitch and slide reach it; with it on, CLAP plug-ins get note expressions, VST3 plug-ins note expression through `INoteExpressionController`, and Audio Units MPE MIDI; with it off, only per-note pressure, as polyphonic key pressure.
- **Editor windows** are native top-level windows owned by the host, with the plug-in's view embedded as a child: an `NSView` on macOS, an `HWND` on Windows, and an X11 window on Linux. Most Linux plug-ins draw only into X11 windows, so on Wayland the app runs through XWayland when a plug-in editor opens, as Bitwig and REAPER do. All three are built (WP-2.10): on macOS an editor that embeds opens in a utility panel of Beatlayer's own, and one that only floats keeps its own window.
- **In-process first.** Plug-ins run inside the main process until Phase 10. The trait is designed so a plug-in can later move into `beatlayer-host`, a sandbox process that shares audio buffers through shared memory. The sandbox gives crash isolation and, on Apple Silicon, a route to x86_64 plug-ins through Rosetta.
How WP-2.6 built CLAP hosting ([the CLAP hosting spec](specs/clap-hosting.md)), on `clack-host` 0.2.0 pinned exactly:

- **The main thread owns every plug-in.** `bl_plugin_host::clap::ClapHost` lives on the shell's thread, which is CLAP's main thread for every instance: it loads plug-in files by path (until WP-2.9's scanner), keeps one instance per device, activates and deactivates them, saves and loads their state, and opens their editors. `ClapHost::idle`, run by the shell's poll, does its work.
- **Nodes without waiting.** Its `ClapFactory` in the engine's registry makes a CLAP device's node at once, without the plug-in, and queues a request; the main thread activates the plug-in for the node's rate and block size and hands its audio processor over through a one-slot `rtrb` queue. Until it arrives the node passes its input through. The processor goes back the same way when the plug-in asks for a restart (its latency changed) or the node is dropped, and the main thread deactivates it there. An instance outlives its nodes, so chain edits, engine restarts, and undoing a delete keep the plug-in and its state.
- **On the audio thread** the node turns the block's parameter changes and notes into CLAP events on their frames, copies the block's channels into the plug-in's main ports and back, and passes the plug-in's notes on (`Block::send_note`) and its own parameter changes to the main thread, which makes each gesture one undo step. Every buffer and event list is made with the node; an event past their room is dropped and counted. A plug-in that fails to process is never called again and its device is silent.
- **Notes in blocks.** `bl_engine::Block` carries notes in (`Block::notes`) and out (`Block::send_note`) since WP-2.6; the engine feeds devices notes from MIDI clips and inputs with WP-2.1 and 2.2.
- **State in the set.** A device's `plugin` (`bl_model::PluginState`) holds the file the plug-in came from and its own state, base64 in the set file, taken from the running plug-in at each save and copy without an undo step (`Editor::amend_plugin_state`).
- **Offline renders** (export, agents' previews) pass plug-in devices through until WP-2.6b.

- **ARA 2** (IMP-20, [the ARA hosting spec](specs/ara-hosting.md)): a VST3 plug-in that answers ARA's entry points is bound to a document controller the host keeps for its plug-in, and renders its track's clips itself. The main thread turns the set into edit cycles after every change (undo and redo included): the tempo and signatures as the musical context, each track as a region sequence, each file as an audio source, each clip as a modification, each straight stretch of a clip as a playback region. The compiler asks the plug-in's factory which clips it takes (`NodeFactory::ara_support`, `bl_model::ara_renders`) and leaves them out of the track's clip list, which travels as node data. The plug-in's audio readers read the media store on its own threads and never on the audio thread; adding or removing a region on a running plug-in takes it back from its node for a moment. Audio Unit and CLAP ARA hosting aren't built.
- **Plug-in paths** follow each format's conventions, and Beatlayer honours the `CLAP_PATH` environment variable, as the CLAP specification requires.

## Media and warping

| Job | Built on |
|---|---|
| Decode WAV, AIFF, FLAC, MP3, Ogg Vorbis, AAC, ALAC | `symphonia` 0.6, plus Beatlayer's own RF64 and BW64 reader (symphonia reads neither) and AAC delay trimming in MPEG-4 |
| Sample-rate conversion | `rubato`, pinned to an exact version (five major releases in 2026) |
| Content hashing | `blake3`, over the file's bytes |
| Disk streaming | Beatlayer's own, in `bl-media`, after `creek`'s design |
| Encode WAV, FLAC, MP3 | `hound`, `flacenc`, `mp3lame-encoder` (LAME, LGPL-3.0) |
| ML inference (stems, audio to MIDI) | `ort` (ONNX Runtime), pinned while 2.0 is a release candidate, with `load-dynamic`: ONNX Runtime's shared library isn't linked or fetched at build time, but loaded at run time from the packages a person downloads on first use (IMP-11), into `Packages` in the application-data folder, each checked against its SHA-256. Demucs runs from ONNX, with `charon-audio` as the reference; Spotify's basic-pitch model ships as ONNX. |

- **The media store** (`bl-media`) names each file by the BLAKE3 hash of its bytes, not of its decoded samples: hashing needs no decoder, runs at disk speed, and can't change when a decoder does. It checks a file still holds its content before use, relinks moved files by content, and hands the engine decoded audio as `Arc<SampleData>`, at the engine's rate, through the engine's `MediaLibrary` trait (WP-1.4). The [audio import spec](specs/audio-import.md) gives the rules and tolerances.
- **Sample edits** (WP-5.15, IMP-28) are kept in the document as an ordered list per sampler sample, never as audio: `bl_media::edits` renders the list over the decoded sample off the audio thread, and the media store shares the result by the source's hash, the rate and the list, so the display, the compiler and an offline render play one copy. Crop and Reverse are the exception that writes a processed file, through the one writer of the audio clip editing spec.
- **Peaks** are multi-resolution min/max overviews: per channel, the extremes of every 256 frames, then levels that each merge four. They're built in the background on a `rayon` pool, which decodes a long file in parallel segments, keyed by content hash, and saved in the platform's cache folder; a file whose content changes never shows its old peaks. Their data block is what the UI's waveform shader reads, uploaded without conversion. The [waveform peaks spec](specs/waveform-peaks.md) gives the rules, the layout, and the file format.
- **Streaming** (WP-1.3, [spec](specs/disk-streaming.md)): files under 10 seconds load into memory; longer ones stream. One disk thread decodes every stream a block at a time (4,096 frames), converts it to the engine's rate, and keeps sixteen blocks ahead of each reader in the engine (eight by default), so a Complex Pro clip's read-ahead fits the ring; blocks travel to the audio thread and back for reuse on `rtrb` queues, so they're only freed on the disk thread. Each stream also holds cues decoded in advance (1 s at 48 kHz in the engine, 341 ms by default): the clip's start, decoded before the clip can play, and points a jump will land on, such as a loop's start, so a launch or a locate there never waits on the disk. A clip player reads through the `AudioSource` trait (`bl_media::stream`), never waits, and plays silence for audio that isn't there yet: an underrun, counted and reported for the disk overload indicator. A locate during playback crossfades over 4 ms, so it doesn't click. Offline renders read in blocking mode, so they never underrun.
- **Warping** maps clip time to song time through warp markers, then renders through the clip's warp mode. Re-Pitch (resampling), Beats (transient slices), and Tones and Texture (granular) are native. Complex and Complex Pro equivalents use Signalsmith Stretch (MIT) by default.

## UI

The UI is built on egui (D8), through egui-winit and egui-wgpu from Beatlayer's own event loop. Plug-in editor windows are created outside the toolkit, with `baseview` or `winit`.

These rules hold:

- The UI reads document snapshots and engine state, and changes the document only through commands. It holds no song state of its own.
- The dense views (arrangement, Session grid, piano roll, waveforms, meters, automation) draw through custom GPU pipelines (`wgpu`): instanced rectangles for clips and notes, and a shader that draws waveforms straight from peak data. The framework provides windows, input, text, layout, and standard widgets around them.
- Every control exposes a role, name, and value through AccessKit, and every view is reachable from the keyboard.
- Colours, sizes, and fonts come from a theme: Beatlayer's own token files, one light and one dark, toned, adjusted and brought to high contrast by the Theme & Colors settings before anything draws ([themes](specs/themes.md)). Each main window's zoom scales it from 50% to 200%; the Settings window follows the main window's.
- The app can open a second main window (WP-6.29). The shell keeps one state per main window (its main view, detail area, keyboard focus and zoom) and one set, selection and undo history for both.
- Each view's mixer keeps its own sections and height; column widths are per track and saved with the set as view data (WP-6.65).

## Layout, commands, and key bindings

Beatlayer's interface matches Live 12's placement and key bindings (D16), so a Live user meets nothing new. Three pieces make that hold as the UI grows:

- **The layout map** (`docs/ui-parity/layout.md`) says where every view, panel, and control sits, how it's sized relative to its neighbours, and what shows or hides it. Each UI package cites the rows it implements, and its acceptance includes a side-by-side comparison with Live at the same window size.
- **Commands.** Every user action is a named command with an ID, a label, the contexts it applies in (Arrangement, Session, the Clip and Device views, the browser, a text field), and its default bindings. Menus, context menus, shortcuts, key mapping, and MIDI mapping all invoke commands, so an action has one implementation however it's reached. The registry is `bl-commands`, which depends on no other Beatlayer crate so the document, the engine, project files, and the app shell can all register commands in it; each crate that registers commands also runs them by ID. `cargo xtask commands` generates the reference, [docs/commands/](commands/README.md), an index and a file for each command namespace, and `cargo xtask check` fails if it drifts.
- **The keymap.** The default keymap is Live 12's, as data (`crates/shell-spec/keymaps/live12/`) generated from the shortcut data in `docs/ui-parity/shortcuts.toml` (the table in `shortcuts.md` is generated from the same file). It records both platforms' bindings (Cmd on macOS where Windows and Linux use Ctrl, Option where they use Alt), the context each binding applies in, and whether it acts on press or while held, as Live 12's momentary shortcuts do. `cargo xtask keymap` generates it from the shortcut data and the command registry, marking each shortcut bound (with the commands it runs), handled by the system (the platform's menus, and items its standard menus provide, such as macOS's Hide Others and Emoji & Symbols) or the focused control, or pending (with its milestone and, for M1 to M3, the package that binds it, which isn't done), and `cargo xtask check` fails if it drifts. A test fails if a shortcut in the map is neither bound nor marked with the milestone that binds it. A person's own shortcuts (IMP-17, WP-6.43) are a user keymap, `keymap.toml` beside the preferences, layered over the generated one at load: it binds commands by ID and unbinds default shortcuts by ID. The generated keymap never changes, so the keymap test still checks Live's, and one layered keymap feeds the menus, the native menu bar and the command search.

The UI toolkit has to support this: placement to the pixel, dense small text, keyboard focus that moves between areas, and key handling that depends on context.

## Agent control

Every feature in Beatlayer can be driven by an AI agent, with whatever model the user has, cloud or local (D17). It's built on the command layer above rather than beside it, so a feature that exists for a person exists for an agent by construction.

### Commands are the API

Every command in the registry carries, besides its ID and label:

- a typed argument and result schema (JSON Schema, generated from the Rust types);
- a description written for a model: what it does, when to use it, and what it changes;
- a permission class, by the command's effect on the user's work:
  - **read**: changes nothing in the project or on disk (queries, and moving focus or the view);
  - **edit**: an undoable change to the project, or a preference that's trivially changed back;
  - **destructive**: removes project content (deleting tracks, clips, devices, or placing clips over others), or loses work that undo can't restore (discarding unsaved changes, overwriting files, quitting). An agent asks first, even when undo could restore the content, unless the user has turned on autonomous mode for that agent;
  - **external**: touches files outside the project folder, the network, or other applications.

Parameters are addressable too, by stable ID or by a readable path such as track "Bass", device "EQ", parameter "Band 1 Gain". The object model uses Live's vocabulary (track, clip, clip slot, scene, device, rack, chain, parameter), which models already know from their training. A test fails if any command lacks a schema, a description, or a permission class.

### The agent API

`bl-agent` serves the Model Context Protocol (MCP), the open standard that agent clients speak (Claude, ChatGPT, Cursor, Gemini CLI, Goose, and local agent frameworks among them):

| Transport | Use |
|---|---|
| Streamable HTTP on `127.0.0.1`, from the running app | An agent works alongside the user. Off until the user enables it in Settings; each client is approved once; requests carry a token and are checked for their origin. |
| stdio, from `bl-cli mcp` | An agent builds or changes projects with no window open, on any platform, including a Linux server. |

A few well-designed tools serve the whole registry, rather than one tool per menu item:

| Tool group | What it does |
|---|---|
| Discovery | List commands by area or search, and describe one (its schema and description) |
| Run | Run one command or a batch as a single undo step, with a dry-run option that validates without changing anything |
| Query | Read the project (tracks, clips, devices, parameters, automation, tempo map), by ID or path, with summaries for large projects |
| Transport and time | Play, stop, locate, loop; run any command at a given bar and beat, which the engine executes on the exact sample. Agents can't hit musical timing over a network, so they schedule instead. |
| Perception | Render a short preview of any range or track as audio; analyse it as text (loudness, peaks, spectrum, tempo, key, transients); capture any view as an image for vision models (provided by the app when a UI runs) |
| Checkpoints | Save and restore named checkpoints. The document is an immutable tree, so a checkpoint costs almost nothing, and an agent can try variations and roll back. |
| Guide | A short resource explaining Beatlayer's concepts, IDs, and good practice to the agent |

Agent edits are transactions labelled with the agent's name. They show in the undo history and an activity log, and the user can undo any of them. Commands in the destructive and external classes ask the user first, unless the user turns on autonomous mode for that agent. Agents never touch the audio thread: commands go through the same queue as the UI's.

How that works, as WP-1.15 built it:

- **The protocol is Beatlayer's own code**, and since 12 October 2026 it lives in the plug-in kit's `lspk-agent`, which `bl-agent` wraps with Beatlayer's tools through the kit's `Program` trait. `bl-agent` speaks JSON-RPC 2.0 and MCP's lifecycle, tools, and resources itself (protocol versions 2024-11-05 to 2025-11-25; a client asking for another gets the newest), over stdio and over a small HTTP/1.1 server on `std::net` with one thread per connection. The transport layer is separate from the tools.
- **Tools.** `list_commands` and `describe_command` (discovery), `run`, `query`, `transport`, `schedule`, and `checkpoint`; the guide (`crates/agent/guide.md` in the public repository) is the resource `beatlayer://guide`, and `initialize`'s instructions point to it. `run` takes a list of commands: more than one must all be document edits, and they run in one transaction labelled "agent: label", all or none; `dry_run` applies them and cancels. New objects' IDs are written `{"$new": "name"}` and referred to as `{"$ref": "name"}`, in the same call or later ones, so an agent never asks for IDs first. `query` reads by ID or by a readable path (`track "Bass" / clip #1`, `device "EQ" / param 0`), paging long lists.
- **The result format.** A tool result's text block is compact JSON by default. `bl-cli mcp --result-format toon` (the kit's `Server::set_result_format`) writes it as TOON instead when the value holds a table of at least 8 rows and TOON is shorter; a nested object of primitives in a table's rows becomes dotted columns (`mixer.volume`). `structuredContent` is JSON either way. `crates/cli/tests/toon_bench` measures how well a model reads each format and what each costs in tokens.
- **Where commands run.** Every command runs on `bl-shell`'s `Shell`, the one place the UI's commands run, through `bl_model::registry::run` for document edits; `Shell::edit` gives the agent API the editor for batches, dry runs, new IDs, and checkpoints. In the app, the HTTP server's threads hand each request to the winit event loop (a user event) and wait for its answer.
- **Permissions.** `read` and `edit` run. `destructive` and `external` ask the person in a native dialog (the same kind as "Save changes?") while the agent's request waits, unless the person made that agent autonomous; `bl-cli mcp` has no one to ask, so it refuses them unless started with `--autonomous`, and says so. The agent server's own commands (`agent.*`) are refused to agents always. Restoring a checkpoint is destructive when it would remove a track, clip, or device that exists now, and an edit otherwise.
- **The HTTP server.** Off until the person switches it on in Settings, Agents (Beatlayer's own page, straight after Plug-Ins, since Live has no agent settings), on `127.0.0.1` at port 37420 by default. Every request must carry a token as a bearer token (an allowed agent's own, or the server's: each made on first use, kept in the OS keychain (never in the preferences' file), shown on the Agents page, and replaceable there with New Token), name `127.0.0.1`, `localhost`, or `[::1]` with the server's port as its host (DNS-rebinding protection), and carry no `Origin` header, so web pages are refused. POSTs are answered with one JSON response, GET with 405 (the server starts no streams), and DELETE ends a session.
- **The transport.** The transport's playback commands (`transport.play`, `transport.stop`, `transport.continue`, `transport.locate`, and the rest that `bl_engine::transport` registers) act in the engine, not on the document, so the shell doesn't run them: the agent API lists them beside the shell's commands, turns each into a `TransportCommand`, and queues it for the host, which puts it on the engine's one queue, the queue the UI's transport commands take, after syncing the document. The engine acts on it at the start of its next block. The `transport` tool runs them by name (`locate` also takes a bar and beat), and so does `run`. The loop brace and switch (`transport.set_loop`, `transport.set_loop_enabled`) are document edits and run on the shell, so they're undo steps and can batch with other edits. The host says whether an engine plays the song (`Agent::set_engine_attached`) and hands back its published state (`Agent::on_engine_state`), so `transport`'s `status` reports the playhead, the insert marker, and the loop. Without an engine, as in `bl-cli mcp`, the playback commands are refused, saying why; the loop can still be set.
- **Scheduled commands.** `schedule` checks a command against the song as it is, asks first if it must, and queues an engine `ScheduledCommand` at the position's ticks: fader and device parameter commands become the engine's parameter actions, so they change the sound on their exact frame, and `transport.stop` and `transport.locate` become its transport actions, so the song stops or jumps there; others are reports only. Play and Continue would do nothing at a position the playing song reaches, so they're refused. When the engine reports one fired, the main thread runs a document command on the document as the agent's undo step. The host moves the queued messages to the engine and hands back its reports; where no engine plays the song, scheduled commands wait.

How permissions per call, the person's gestures, and agents' tokens work, as WP-1.15b built them ([the agent permissions spec](specs/agent-permissions.md)):

- **A call's class from its arguments.** Most commands have one class. But a clip placed over part of another replaces what it covers, as in Live, so `bl_shell::permission` decides the class of the Arrangement's moves, resizes, nudges, duplicates, and pastes, and of the browser's drops, per call: `Shell::call_permission` tries the edit in a transaction it then cancels (the selection and clipboard copied, so nothing changes) and compares the clips before and after; a call that deletes, shortens, or splits a clip it doesn't act on is destructive as well as its registered class. A drop isn't tried, because reading its files is the external part the person is asked about: it's destructive when a clip on its track ends after where it lands. Classes only ever rise. The question names each command's classes ("destructive and external"), a dry run gives them and the clips a call would replace, and the edit's result is the person's own edit's.
- **The person's gestures come first.** While the shell has a gesture under way (`Shell::gesture_under_way`: a drag that edits the set, or a drag on a value), the agent API holds every tool call (`Outcome::Wait`) and every engine report of a fired scheduled command, in arrival order. When the gesture ends, the host calls `Agent::resume`, which runs them in that order, each as its own undo step; a call that asks first asks then, against the set as the drop left it, and the rest wait for the answer. The HTTP server keeps the waiting call's connection (`Served::Parked`) until `http::resume` answers it. Edits the shell runs (the Arrangement's, the browser's drops) are labelled with the agent's name through `Shell::run_labelled`, as batches already were.
- **Rights go with a token, not a name.** Each allowed agent has a token of its own (`AgentClient::token`, 64 hexadecimal digits made when the person allows it, kept in the OS keychain as a `Secret` that never prints, never in a command's result or the preferences' file). The HTTP server compares each request's bearer token with the server's and every agent's in constant time and hands the main thread a `Credential` (`Server` or `Agent(name)`); a session keeps the credential it started with and every request on it must carry the same. A session with an agent's token has that agent's rights (`Session::agent`), whatever `clientInfo` name it gives. A client with the server's token is asked about at every connection; allowed, it becomes a new agent with its own token, or, if the name it gave is taken, is allowed for that session only and never autonomous. `agent.new_client_token` replaces an agent's token; like every `agent.*` command, only a person runs it.
- **The app's engine is the agent API's.** After every event, and every 10 ms while agents' scheduled commands wait, the app's host calls `Agent::sync_engine`: it says whether the shell's audio host runs an engine, sends agents' transport commands and scheduled commands through `AudioHost::send_transport`, `schedule`, and `cancel` (the engine's one request channel, the one the person's transport commands take, so one queue and one order), hands back the engine's reports (`AudioHost::take_reports`, keeping a closed engine's) and transport, and, when `AudioHost::engine_generation` says a new engine started, sends it every scheduled command still waiting.

How perception works, as WP-1.16 built it ([the agent perception spec](specs/agent-perception.md)):

- **Commands and tools.** `perception.preview`, `perception.analyze`, and `perception.capture_view` are registered commands in the read class (`bl-shell-spec`), and the agent API serves them as the tools `preview`, `analyze`, and `capture_view`. They change nothing and never ask the person.
- **Previews.** The shell renders the range on a copy of the set (with one track soloed alone, for `track`) through the export's render loop, offline on the calling thread with an engine of its own, so the engine playing on the device carries on undisturbed. A preview is cut at 30 seconds and kept as samples, the last four by ID; it becomes a WAV file in memory when it's read (`bl_media::wav_bytes`, the export's quantiser and dither). The tool returns it as an MCP `audio` content block, or an embedded resource for clients of protocol 2024-11-05, and each kept preview is the resource `beatlayer://preview/<id>.wav`.
- **Analysis.** `bl-analysis` measures a preview, or a range rendered for it (up to ten minutes): loudness and true peak by ITU-R BS.1770-4 and EBU R 128, sample peak and RMS, octave bands from Welch-averaged FFTs, transients by spectral flux with a SuperFlux maximum filter and waveform refinement, tempo from the onset function's weighted autocorrelation refined through the transients, and key from a chromagram against Krumhansl and Kessler's profiles. FFTs are `realfft`'s, already in the build through the resampler. Results are numbers with units and a short summary.
- **View captures.** Only a window can draw a view, so `capture_view` returns `Outcome::Wait` and queues a `CaptureRequest`, which the app's host takes after the call (`Agent::take_capture_requests`). The host checks the view is on screen, and right after the window's next frame draws the same tessellated frame again into an offscreen texture with the window's own renderer and reads it back (the function `--screenshot` uses), so the capture doesn't depend on the system presenting the frame. A thread of its own crops the view (from the panes' rectangles the main window's last frame recorded), scales it to fit, and encodes the PNG, and the host hands it back (`Agent::finish_capture`); `Agent::resume` then answers the waiting call, as it answers calls that waited for a gesture. Headless, no host takes requests, and the tool says view captures need the app's window.

### The built-in assistant

`bl-assistant` gives users without an agent client an assistant panel in the app. It runs the agent loop over the same tools, with providers behind one trait:

- **Cloud:** OpenAI-compatible APIs (including OpenRouter), Anthropic, and Google.
- **Local:** any OpenAI-compatible server, such as Ollama, LM Studio, llama.cpp, or vLLM.

API keys live in the OS keychain, never in projects or settings files. Nothing leaves the machine unless the user configures a cloud provider.

### Testing through the agent API

The agent API is also Beatlayer's end-to-end test harness. Scripted MCP clients drive the same tools an agent uses, so every feature's headless test path doubles as proof that agents can reach it. The M1 and M2 exit demos each have a scripted agent version.

## Project files and interop

A project is a folder, as in Live:

```
My Song Project/
  My Song.blset          zstd-compressed JSON, schema-versioned
  Samples/Recorded/
  Samples/Imported/      filled by Collect All and Save
  Backup/                the last 10 saves
```

- Every file carries a schema version. Migrations are pure functions from one version to the next, and every past version has a fixture test.
- Sample references hold a relative path, a content hash, and a size, so a missing-files search can relink by content.
- A set is one zstd frame, with its content checksum, holding a JSON object: `format` and `schema` first, then the revision (how many times it has been saved), the ID allocator, the media table, and the document. `bl-project` compresses with `ruzstd`, a pure-Rust zstd, so no C toolchain is needed and the fuzzer covers the decoder; the reference `zstd` tool reads the files.
- Saves are atomic: the new set goes to a hidden file beside the old one and is flushed to disk, the old set is copied into `Backup/` as `<name> [YYYY-MM-DD HHMMSS].blset` (UTC), and a rename puts the new set in place. A save stopped at any point leaves the old set or the new one, whole.
- Crash recovery and continuous save (IMP-05, WP-1.11) share a journal: every finished edit is appended, within a second and on the `bl-autosave` thread, to a hidden file beside the set (`.<set file>.<session>.bljournal`), each entry a zstd frame naming the revision it follows and holding a snapshot of the saved form or a patch from the entry before. Each session registers in a recovery folder in the user's application data with a lock file it holds while it runs; at launch, a session whose lock can be taken ended without closing its set, and its unsaved entries are offered for recovery. With Save Continuously on, entries are committed save points: loading a set opens it at its newest one. A normal save writes the next revision and retires the journal, so the set format stays as it is. The rules are in the [crash recovery spec](specs/crash-recovery.md).
- The app hosts the journal (WP-6.62): the shell hands each finished edit's snapshot to the `bl-autosave` thread, so neither the interface nor the audio waits on the disk, and at start-up it offers what a crash left. The save history (WP-6.39) lists the save points, the saved file and its backups, and restores one through `Editor::replace` as one undo step. Open Beatlayer Set Backups opens a backup as the set (WP-6.63).
- Presets, racks, and clips have their own file types. A plug-in device's preset carries the plug-in's own state, as rack presets do (WP-6.66). Clip files are `.blclip`, a zstd frame of JSON like a set, with a media table relative to the file ([clip files](specs/clip-files.md)). Project packs are `.blpack`, a ZIP64 archive of the project folder with a manifest ([set exchange](specs/set-exchange.md)). Linked clips (IMP-21) share their contents: the set saves a link's contents once and each linked clip names its link.
- Beside the document, a set saves which main view the main window shows and whether the second window is open (WP-6.29), and MIDI Tools' settings and each track's chosen tools, as folds are, not in the document (WP-6.11).
- The browser's index is in memory until M6, built on a background thread when the places change and searched by file name (WP-1.10, [browser spec](specs/browser.md)). From WP-6.31 the browser's database is a SQLite file (`rusqlite`, with SQLite compiled in) in the configuration folder beside the preferences. It holds the index of the places' files, tags and tag groups, collections and saved labels. A background indexer fills it at start and when places change; it doesn't watch folders. Search finds every typed word anywhere in a name ([browser tags](specs/browser-tags.md)).
- The File Manager (WP-6.63) reads every set and clip file of a project or of the User Library to find missing, external and unused files; deleting unused files moves them to the system's trash.

Interop in `bl-interop`:

- **Standard MIDI Files** in and out (`midly`).
- **DAWproject** in and out, from Phase 6 ([the spec](specs/dawproject.md)). It's the open format from Bitwig and PreSonus, version 1.0 (MIT licence), read and written by Bitwig Studio, Studio One, and Cubase among others: a ZIP container (`zip`, as `bl-shell` already uses it) holding XML (`quick-xml`, as for Live sets), read into Beatlayer's own model of the schema in `bl-interop` and opened as a new set. Plug-in states travel in their formats' own preset files, and Beatlayer's own devices and racks in its own presets.
- **Scala tunings** (`.scl`, and `.ascl` with its extensions) in and out, from WP-6.9 ([tunings](specs/tunings.md)).
- **Ableton Live sets** (`.als`), imported in Phase 9. Live sets are gzipped XML, read with `flate2` and `quick-xml`. The importer maps tracks, clips, warp markers, mixer state, automation, and plug-ins, and maps Live's native devices to Beatlayer's nearest equivalents.

## Extensions

Beatlayer has its own extension layer, in two parts (Phase 9):

- **WebAssembly components** for MIDI tools, control-surface scripts, and DSP devices. Control surfaces follow MIDI Tools: Phase 7 builds the `ControlSurface` trait in `bl-control` with native surfaces and declarative templates ([control surfaces](specs/control-surfaces.md)), and a component adapts the same trait in Phase 9. The API is defined in WIT, extensions run in `wasmtime` with memory reserved up front, and they're sandboxed and portable across all three platforms.
- **A native modular device** (a patcher of `bl-dsp` blocks) for users who build instruments and effects without code.

A MIDI tool, native or an extension, is a description and a function. The description gives its ID, name, kind (transformation or generator) and parameters, each with its kind, range, default, unit, description and whether it counts scale degrees. The function takes the input notes (with their values and expression envelopes), the range, the grid step, the time signature, the clip's scale as a root and steps, the track's drum keys, the parameter values and a seed, and returns the notes it writes. Beatlayer does everything around it: choosing the target, replacing and merging notes, seeds, Auto, undo and chains. Native tools implement the trait in `bl-midi-tools` (WP-6.11); in Phase 9 a WebAssembly component exports the same two functions through a WIT world (`describe`, `run`), and the host adapts it to the trait, so an extension tool appears in the selectors after the native ones and is reached by the same `midi_tool.*` commands ([MIDI Tools](specs/midi-tools.md)).

## Testing

| Layer | How |
|---|---|
| Core types and model | Unit and property tests (`proptest`): tempo round trips, undo after any command sequence, save and load round trips |
| DSP | Measured: frequency and phase response, aliasing, THD, noise floor, against stated limits |
| Engine | Golden renders: fixture songs built in code (`crates/cli/src/golden/fixtures.rs`) render through the code behind `bl-cli render` and are null-tested in `cargo test` against references in `tests/golden/`: 24-bit FLAC files, read back by an independent decoder (`claxon`), with a manifest recording the platform that made each and a hash of the 32-bit float render it came from. Bit-exact on the platform that made the reference (every 24-bit sample, and the float render's hash); a residual below −96 dBFS elsewhere. References change only through `cargo xtask golden --update`, with the reason in the pull request. |
| Real-time safety | Allocation trap in every engine test (`rt_test!`); RealtimeSanitizer through `cargo xtask rtsan` (the `rtsan/` harness, with an LLVM runtime already installed), on Linux in CI; a stress test at 32-frame buffers |
| Time in tests | Tests drive time themselves rather than waiting on the machine. The null device's manual pacing runs a block only when a test asks for it, on the stream's own thread, and one test clock advances by each block's length; the device, the MIDI stamps, and the engine's block-start estimate all read it, so a note played 250 ms later lands on its frame however busy the machine is. The helpers are `bl_shell::testing`. `cargo xtask test-clock` fails a `thread::sleep` or `Instant::now` in test code outside `xtask/test-clock-allowlist.txt`; each entry there says why the wait is the subject, such as a real-time pacing test or a real device. |
| Files | Fuzzing (`cargo-fuzz`) of the project loader, the MIDI file, SFZ and DAWproject readers, and the Live set importer |
| Plug-ins | A host conformance suite run against a fixed set of free plug-ins in CLAP, VST3, AU, and LV2 |
| UI | Offscreen renders compared as images; scripted interaction through the command layer |
| Performance | Benchmarks (`criterion`) for DSP kernels and full-project renders, tracked per commit |
| Parity with Live | Behaviour specs in `docs/specs/`, each with tests that encode the observed behaviour |

## Packaging

`cargo-packager` builds every package below, and CI runs on GitHub's hosted runners (`macos-15` on arm64, `windows-2025`, `ubuntu-24.04`). Arm runners for Windows and Linux exist, so the Arm builds in Phase 10 need no new infrastructure.

`cargo xtask package` builds the current platform's packages and starts each packaged binary to check it, and the nightly workflow runs it on all three platforms (WP-0.11). The configuration is `[package.metadata.packager]` in `crates/app/Cargo.toml`, with the macOS `Info.plist` additions and entitlements in `crates/app/packaging/macos/`. Signing and notarisation run only when their secrets exist; until a signing certificate and a code-signing route exist, the packages are unsigned. Unsigned or not, every release attaches `SHA256SUMS.txt` and a build provenance attestation for each package (`release.yml`), so a download can be checked against the workflow and commit that built it; [the security page](security.md) has the threat model.

| Platform | Package | Notes |
|---|---|---|
| macOS | `.app` in a signed, notarised DMG | Hardened runtime with entitlements for audio input and for loading third-party plug-ins (`com.apple.security.cs.disable-library-validation`). Notarised in CI. Needs a Developer ID. GPL-3.0 rules out the Mac App Store, which Beatlayer doesn't need. |
| Windows | Signed MSI installer and a portable zip | ASIO built in. Needs a code-signing route. |
| Linux | AppImage and a tarball first; Flatpak later | The AppImage and tarball read the standard plug-in paths. A Flatpak can't see plug-ins installed in `/usr/lib/vst3` and similar paths, so it needs the Linux Audio plug-in extensions, as Bitwig's Flathub package uses. |

<a id="decisions-log"></a>
## Design rules

The rules the design follows, each with its ID.

| ID | Rule |
|---|---|
| D1 | Rust for everything, native on macOS (Apple Silicon), Windows, and Linux. |
| D2 | Clean-room development: match Live's behaviour, never its code or content ([policy](https://github.com/loombridge/beatlayer/blob/main/CONTRIBUTING.md#building-without-abletons-code)). |
| D3 | Licence: GPL-3.0-or-later for the application, with contributions under the Developer Certificate of Origin. |
| D4 | Document as an immutable persistent tree with command-based edits and snapshot undo. |
| D5 | Engine as a compiled program, swapped at block boundaries, with node instances migrated across swaps. |
| D6 | Musical time in integer ticks at 705,600 per quarter note. |
| D7 | Native UI in Rust, not a web view. |
| D8 | UI toolkit: egui, through egui-winit and egui-wgpu from Beatlayer's own event loop, with dense views on Beatlayer's own wgpu pipelines. |
| D9 | Audio I/O: Beatlayer's own device trait; Core Audio written in-house; `azo` for ASIO; `cpal` for WASAPI shared, PipeWire, JACK, and ALSA; `wasapi` for exclusive mode. |
| D10 | Plug-in formats: CLAP (`clack-host`), VST3 (`vst3`), AU (`objc2`), LV2 (Beatlayer's own host on the system's lilv, loaded at run time, without the `livi` crate); in-process first, sandbox in Phase 10. No VST2. |
| D11 | Time stretch for Complex and Complex Pro: Signalsmith Stretch. |
| D12 | Extensions as WebAssembly components plus a native modular device. |
| D13 | Project file: folder bundle, zstd-compressed JSON (one checksummed frame), versioned migrations. |
| D14 | Real-time handoff: `rtrb` queues, `rtgc` deferred drops, `triple_buffer` state; an allocation trap (`bl-rt`'s own) and RealtimeSanitizer in tests. |
| D15 | Packaging with `cargo-packager`; Linux ships AppImage and tarball before Flatpak. |
| D16 | UI placement parity with Live 12: the same views, panels, controls, menus, context menus, mouse interactions, terminology, and key bindings on macOS and Windows. Beatlayer's own icons, typeface, and colours. A deliberate difference needs its own rule here. |
| D17 | Agent control from the start: every command has a schema, a description, and a permission class; the agent API is MCP (stdio headless, Streamable HTTP from the app); a built-in assistant works with cloud or local models; agent edits are undoable transactions. |
| D18 | Improvements over Live follow the rules in [docs/05-improvements.md](05-improvements.md): add, don't move; a changed default or placement is an option defaulting to Live's behaviour unless it's accepted as a deliberate difference; general rules G1 (gains reach −∞ dB), G2 (every parameter automatable and modulatable), G3 (bounces keep the source's channel count). |
| D19 | A third-party plug-in's device panel shows eight knobs mapped to its first parameters and a large target that opens its editor, instead of Live's X–Y panel; the editor toggle stays where Live puts it (IMP-03). A deliberate difference from D16. |
| D20 | A new install's factory default set opens in Arrangement View; a saved default set keeps the user's choice, as in Live (IMP-04). A deliberate difference from D16. |
| D21 | Menus and settings use Beatlayer's name where Live's name its own product ("About Beatlayer", "Quit Beatlayer", "Read the Beatlayer manual"), and leave out items for Ableton's services (Splice, purchasing, the Ableton account). Everything else keeps Live's wording and order. |
| D22 | The main window draws Live's compact title bar (23 pt) instead of the platform's standard one, so every top-anchored panel sits where Live puts it (D16). |
| D23 | Settings, Audio, carries Live's test-tone controls (tone, volume, frequency); the default level follows Live's. |
| D24 | The built-in assistant's panel is a full-height pane on the right of the main window, its width resizable (240 to 640 points), opened and closed by a toggle that is the last control on the Control Bar, immediately right of the Arrangement View button, and by View ▸ Assistant. Live's right group of the Control Bar keeps its order and moves left by the toggle's width and gap (20 points), and the minimum window width grows by the same (1340 points). While open, the panel can be floated into its own window or docked back; the width and the choice are kept. |

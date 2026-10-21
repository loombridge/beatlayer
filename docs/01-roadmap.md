# 01. Roadmap

Beatlayer is an open-source digital audio workstation, written in Rust, that runs natively on macOS (Apple Silicon), Windows, and Linux, with the workflow of Ableton Live 12: Session and Arrangement views, warping, racks, and a full set of instruments and effects. The roadmap runs in 11 phases. The first usable release, alpha 0.2, comes at milestone M3, when you can make a whole song and perform it. Version 1.0 comes when [the parity matrix](04-parity-matrix.md) is covered and the app is hardened for daily use.

## Principles

1. **Live's workflow, not Live's code.** Beatlayer matches what Live does and is built clean-room ([policy](https://github.com/loombridge/beatlayer/blob/main/CONTRIBUTING.md#building-without-abletons-code)).
2. **Familiar on first launch.** A Live user finds every view, panel, control, menu item, context menu, and shortcut where Live puts it, and the same words for the same ideas, so there's nothing to learn before making music. Beatlayer draws its own icons, typeface, and colours. The [UI parity reference](ui-parity/README.md) is the specification, and every UI package is checked against it. [Improvements](05-improvements.md) add to what Live users know; they never move it.
3. **Usable early, complete later.** Every milestone ends with something a musician can do that they couldn't do before. Breadth comes after a solid core.
4. **Sound comes first.** A device ships when its measurements pass and it passes a listening check, not when its controls work.
5. **Real-time safe by construction.** The audio path never allocates, locks, or waits, and tests enforce it.
6. **Every platform is first class.** A feature ships on all three platforms at once, unless the platform itself decides (Audio Units on macOS, LV2 on Linux).
7. **Open formats.** CLAP is a first-class plug-in format. Projects are documented JSON. DAWproject and Live set import keep users' work portable.
8. **Accessible from the first widget.** Keyboard and screen-reader support grow with each view rather than being added at the end.
9. **Headless and testable.** Everything that makes sound runs without a UI and is tested by rendering.
10. **Agent-controllable from the start.** Every feature a person can use, an AI agent can use too, through the same commands, with whatever model the user has, cloud or local. An agent can work alongside you or build a project on its own ([architecture](02-architecture.md#agent-control)).

## Milestones

| Milestone | Release | What you can do | Exit demo |
|---|---|---|---|
| M0 Foundations | none | Nothing yet as a user. Developers get a workspace that builds, tests, and packages on all three platforms. | A tone plays on every platform; the UI framework is chosen. |
| M1 Playback core | internal | Import audio, arrange it, mix it, and export it. Connect an AI agent through MCP and have it do the same. | Arrange eight stems, export, and reopen identically, once by hand and once by an agent. |
| M2 Make a track | in alpha 0.2 | Make a song: MIDI, piano roll, native sampler and synth, CLAP, VST3, and AU plug-ins, recording, automation, sends, and sidechain. Ask the built-in assistant, on any cloud or local model, to build or change a track. | A three-minute song made from scratch, and a 16-bar track built by the assistant on a local model. |
| M3 Session view | **alpha 0.2** | Perform: launch clips and scenes with quantisation, follow actions, and record a performance into the arrangement. | A live set performed and captured. |
| M4 Warping | 0.3 | Work with loops and recordings at any tempo, slice to MIDI, freeze, and bounce. | A remix of unwarped stems at a new tempo. |
| M5 Racks and core devices | 0.4 | Build racks with macros, program drum racks, modulate anything, and use every core effect and MIDI effect. | A drum rack beat through a multi-chain effect rack. |
| M6 Editing depth | **beta 0.5** | Use MIDI transformations and generators, scales, tunings, MPE, comping, grooves, and linked-track editing. Bring projects in from Bitwig, Studio One, and Cubase through DAWproject. | A comped vocal and a generated, scale-aware chord part. |
| M7 Control and sync | 0.6 | Map controllers, sync with Link and MIDI clock, and play from Push 2 and grid controllers. | Two machines in a Link session with a mapped controller. |
| M8 Sound-design devices | 0.7 | Use the full instrument and effect set: wavetable, FM, physical modelling, granular, spectral. | A track made with native devices only. |
| M9 Intelligence and interop | 0.8 | Separate stems, convert audio to MIDI, find similar sounds, import Live sets, score to video, and extend Beatlayer with scripts and devices. | An imported Live set that plays back recognisably. |
| M10 Hardening | **1.0** | Rely on it every day: plug-in sandboxing, compatibility with widely used plug-ins, crash reporting, updates, and a manual. | A month of daily use without data loss. |

Releases between milestones are nightly builds.

## What Beatlayer does that Live doesn't

These come from being open and cross-platform. None of them is a reason to delay parity work.

- Runs natively on Linux, with PipeWire, JACK, and LV2.
- Hosts CLAP plug-ins, including per-note polyphonic modulation.
- Runs plug-ins in a sandbox, so a crashing plug-in doesn't take the song down (Phase 10), and bridges x86_64 plug-ins on Apple Silicon.
- Saves projects in a documented, open format, and exchanges them through DAWproject.
- Accepts extensions written in any language that compiles to WebAssembly.
- Fixes long-standing annoyances in Live, such as the cap of 12 return tracks, gain controls that stop short of silence, and stereo-only bounces, without moving anything Live users know ([improvements](05-improvements.md)).
- Lets AI agents control every feature through MCP, with the user's own choice of model, cloud or local, alongside the user or without the UI at all.

## Out of scope for 1.0

| Not in 1.0 | Why |
|---|---|
| Hosting Max or Max for Live devices | Max is Cycling '74's proprietary runtime. Beatlayer's extension SDK and modular device cover the same ground. |
| Ableton's sounds, packs, and presets | Ableton's copyright. Beatlayer ships its own open-licensed library (Phase 9). |
| Ableton Cloud, Splice integration, and other vendor services | Proprietary services with their own terms. |
| Push 3 standalone mode, and Move and Note project sync | Closed hardware platforms. Push 2 as a controller is in scope, through its public documentation. |
| Loading Live's native devices from a Live set | Beatlayer maps them to its nearest equivalents on import instead. |
| Mobile and web versions | Desktop first. |

## Size

The build plan holds about 365 work packages to 1.0, each one agent session: 256 planned in Phases 0 to 6 and about 110 estimated in Phases 7 to 10.

## How the roadmap changes

Change this document when a milestone's scope or order changes, and update [the architecture's design rules](02-architecture.md#decisions-log) if a rule changes with it. The build plan's status table, not this page, tracks progress.

# Beatlayer progress

Where Beatlayer stands against its [roadmap](01-roadmap.md), [build plan](03-build-plan.md), [parity matrix](04-parity-matrix.md) and [improvements over Live](05-improvements.md). A snapshot of the progress dashboard (`cargo xtask dashboard`), taken on 18 October 2026; the build plan's status table is the live record.

**Key:** 🟩 Done · 🟦 In review · 🟨 In progress · ⬜ To do · 🟥 No package yet · ▫️ Planned · ⬛ Out of scope

## Summary

| | Progress | Done | The rest |
|---|---|---|---|
| Work packages | 🟩🟩🟩🟩⬜⬜⬜⬜⬜⬜ 43% | **130** of 301 | 1 in progress, 35 to do, 135 planned |
| Live 12 features | 🟩🟩🟩🟩🟩⬜⬜⬜⬜⬜ 50% | **93** of 186 | 6 in progress, 19 to do, 68 planned |
| Improvements over Live | 🟩🟩⬜⬜⬜⬜⬜⬜⬜⬜ 24% | **7** of 29 | 7 in progress, 8 to do, 7 planned |

Work-package totals count the estimates for phases not yet broken into packages. Out-of-scope features aren't counted.

## Roadmap

One square per work package. Phases 6 to 10 aren't broken into packages yet, so they show the build plan's estimate.

| Phase | Release | Done | Packages |
|---|---|--:|---|
| **0** Foundations | — | 17 of 17 | 🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩 |
| **1** Playback core | internal | 23 of 23 | 🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩 |
| **2** Make a track | in alpha 0.2 | 34 of 35 | 🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟨 |
| **3** Session view | alpha 0.2 | 18 of 18 | 🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩 |
| **4** Warping and audio editing | 0.3 | 9 of 23 | 🟩🟩🟩🟩🟩🟩🟩🟩🟩⬜⬜⬜⬜⬜⬜⬜⬜⬜⬜⬜⬜⬜⬜ |
| **5** Racks, modulation, and core devices | 0.4 | 29 of 50 | 🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩⬜⬜⬜⬜⬜⬜⬜⬜⬜⬜⬜⬜⬜⬜⬜⬜⬜⬜⬜⬜⬜ |
| **6** Editing depth | beta 0.5 | about 25 | ▫️ Not broken into packages yet |
| **7** Control and sync | 0.6 | about 15 | ▫️ Not broken into packages yet |
| **8** Sound-design devices | 0.7 | about 40 | ▫️ Not broken into packages yet |
| **9** Intelligence, interop, extensions | 0.8 | about 30 | ▫️ Not broken into packages yet |
| **10** Hardening and 1.0 | 1.0 | about 25 | ▫️ Not broken into packages yet |

<details>
<summary><b>Phase 0: Foundations</b>: 17 of 17 done</summary>

_A workspace that builds, tests, and packages on all three platforms; sound out of every platform; the core types; the UI framework chosen._

| | WP | Package | PR |
|---|---|---|---|
| 🟩 | 0.1 | Workspace and tooling |  |
| 🟩 | 0.2 | CI on three platforms |  |
| 🟩 | 0.3 | Audio out on all three platforms |  |
| 🟩 | 0.4 | Real-time thread harness |  |
| 🟩 | 0.5 | Time and tempo |  |
| 🟩 | 0.6 | Document model and undo |  |
| 🟩 | 0.7 | Project file, version 1 |  |
| 🟩 | 0.8a | GPUI spike |  |
| 🟩 | 0.8b | egui spike |  |
| 🟩 | 0.8c | decision |  |
| 🟩 | 0.9 | Engine skeleton and program swap |  |
| 🟩 | 0.10 | Offline render and golden tests |  |
| 🟩 | 0.11 | Packaging |  |
| 🟩 | 0.12 | Contributor docs and the clean-room gate |  |
| 🟩 | 0.13 | UI parity reference |  |
| 🟩 | 0.13b | The parity reference and specs brought up to date with Live 12 |  |
| 🟩 | 0.14 | Command registry for people and agents |  |

</details>

<details>
<summary><b>Phase 1: Playback core</b>: 23 of 23 done</summary>

_Import audio, arrange it on a timeline, mix it, export it, and reopen the project exactly as it was._

| | WP | Package | PR |
|---|---|---|---|
| 🟩 | 1.1 | Media decode and sample-rate conversion (WAV, AIFF, FLAC, MP3, Ogg, AAC, ALAC), content hashing, the media… |  |
| 🟩 | 1.2 | Peak files: multi-resolution, background build, cache invalidation |  |
| 🟩 | 1.3 | Disk streaming with preloaded clip starts |  |
| 🟩 | 1.4 | Audio clip playback: region, loop, gain, edge fades, tempo-map placement |  |
| 🟩 | 1.4b | Clip playback streams from disk through WP-1.3's sources, with no decoding on the compiler thread; the Create… |  |
| 🟩 | 1.5 | Transport: play, stop, continue, position, loop brace, tempo-map following |  |
| 🟩 | 1.6 | Mixer graph: volume, pan laws, mute, solo, groups, main bus, peak and RMS meters |  |
| 🟩 | 1.7 | App shell: main window, native menus, preferences (audio device, rate, buffer, channels), CPU meter, file… |  |
| 🟩 | 1.7b | New sets save into a "<name> Project" folder |  |
| 🟩 | 1.8a | Arrangement canvas: track headers, ruler, zoom, scroll, waveforms, playhead follow, loop brace, adaptive grid |  |
| 🟩 | 1.8b | Arrangement editing: select, rubber band, move, copy, resize, split, duplicate, delete, time selection, snap |  |
| 🟩 | 1.9 | Mixer view: faders, pan, meters, mute, solo, arm; the mixer's sections (In/Out, Sends, Return Tracks) and… |  |
| 🟩 | 1.10 | Browser: places, search, audition through the preview output, drag to the arrangement |  |
| 🟩 | 1.11 | Autosave, crash recovery, and the continuous-save option (IMP-05) |  |
| 🟩 | 1.12 | Export audio: range, WAV, AIFF, FLAC, bit depth, sample rate, dither, normalise, faster than real time |  |
| 🟩 | 1.13 | Device changes: unplug, sample-rate change, sleep and wake |  |
| 🟩 | 1.14 | Keyboard and screen-reader baseline for every M1 view |  |
| 🟩 | 1.15 | Agent API: bl-agent MCP server over stdio (bl-cli mcp, headless) and Streamable HTTP from the app; discovery,… |  |
| 🟩 | 1.16 | Agent perception: audio previews of a range or track, analysis as text (loudness, peaks, spectrum, tempo,… |  |
| 🟩 | 1.15b | Agent permissions per call: an edit that would remove or shorten other material (moving, resizing,… |  |
| 🟩 | 1.15c | Agent permissions, interface follow-up (decided after WP-1.15b): each allowed agent's token in its Settings… |  |
| 🟩 | 1.17 | Interface zoom: Live's Zoom Display setting and the View menu's zoom keys, scaling every pane and control… |  |
| 🟩 | 1.18 | Transport in the app: the Control Bar's transport (play, stop, the Arrangement Position and loop fields, the… |  |

</details>

<details>
<summary><b>Phase 2: Make a track</b>: 34 of 35 done</summary>

_Make a whole song from scratch with MIDI, instruments, plug-ins, recording, and automation._

| | WP | Package | PR |
|---|---|---|---|
| 🟩 | 2.1 | MIDI clip model and playback: velocity, release velocity, probability, velocity deviation, mute; note-offs on… | [#23](https://github.com/loombridge/beatlayer/pull/23) |
| 🟩 | 2.2 | MIDI I/O: device preferences (track, sync, remote), per-track input routing, monitoring, timestamps; the… | [#48](https://github.com/loombridge/beatlayer/pull/48) |
| 🟩 | 2.3 | Piano roll: draw, select, move, resize, velocity lane, fold, quantise with strength, transpose, loop region | [#39](https://github.com/loombridge/beatlayer/pull/39) |
| 🟩 | 2.4 | Device chains in the model and engine; the device frame in the UI (on/off, collapse, presets) |  |
| 🟩 | 2.5 | Native device framework: parameter descriptors, smoothing, presets, generic UI, oversampling helper; a device… |  |
| 🟩 | 2.5b | Floating device windows (IMP-14): Float in the device menu, the floating strip in the chain, the Dock button,… |  |
| 🟩 | 2.6 | CLAP hosting: process, note and parameter events, state, latency, editor |  |
| 🟩 | 2.6b | Offline renders with plug-ins: export and agents' previews borrow the live engine's plug-in instances… |  |
| 🟩 | 2.7 | VST3 hosting: process, sample-accurate parameters, state, latency, editor | [#37](https://github.com/loombridge/beatlayer/pull/37) |
| 🟩 | 2.8 | Audio Unit hosting on macOS (v2 and v3): process, parameters, state, editor | [#44](https://github.com/loombridge/beatlayer/pull/44) |
| 🟩 | 2.9 | Plug-in scanner process: timeout, blocklist, cache, rescan, browser category, hidden plug-ins and format… | [#19](https://github.com/loombridge/beatlayer/pull/19) |
| 🟩 | 2.9b | Plug-ins in the packaged app (the spec R11 to R14): beatlayer-scan in the bundle, a background scan at start,… | [#131](https://github.com/loombridge/beatlayer/pull/131) |
| 🟩 | 2.10 | Plug-in editor windows per platform, with resizing, scale factor, and XWayland on Linux; the plug-in device… | [#36](https://github.com/loombridge/beatlayer/pull/36) |
| 🟩 | 2.11 | Plug-in delay compensation, including automation offset and sidechain inputs: a sidechain's source lines up… | [#21](https://github.com/loombridge/beatlayer/pull/21) |
| 🟩 | 2.12 | Automation: lanes, breakpoints, curves, recording, re-enable, sample-accurate playback | [#43](https://github.com/loombridge/beatlayer/pull/43) |
| 🟩 | 2.13 | Audio recording: duplex I/O (with the In/Out section's Ext. In and Ext. Out channels and Main's Cue Out and… | [#41](https://github.com/loombridge/beatlayer/pull/41) |
| 🟩 | 2.14 | MIDI recording: arrangement record, overdub, count-in | [#65](https://github.com/loombridge/beatlayer/pull/65) |
| 🟩 | 2.15 | Metronome: sound, count-in, settings | [#27](https://github.com/loombridge/beatlayer/pull/27) |
| 🟩 | 2.16 | Sampler v1: one-shot, classic, and slice modes; loop; envelopes; filter; LFO; voices; glide | [#31](https://github.com/loombridge/beatlayer/pull/31) |
| 🟩 | 2.17 | Polysynth v1: anti-aliased oscillators, zero-delay-feedback filters, envelopes, LFO, modulation, MPE | [#33](https://github.com/loombridge/beatlayer/pull/33) |
| 🟩 | 2.18a | Core effects 1: parametric EQ with analyser, compressor with sidechain, limiter with true-peak look-ahead | [#30](https://github.com/loombridge/beatlayer/pull/30) |
| 🟩 | 2.18b | Core effects 2: algorithmic reverb, tempo-synced delay, utility | [#29](https://github.com/loombridge/beatlayer/pull/29) |
| 🟩 | 2.19 | Routing and returns: return tracks without a limit (IMP-01), pre- and post-fader sends, the mixer's In/Out… |  |
| 🟩 | 2.19b | Sidechain routing to devices and plug-ins (each device's sidechain section picks its source; Live 12 routes… |  |
| 🟩 | 2.20 | Tempo and time-signature changes in the arrangement | [#16](https://github.com/loombridge/beatlayer/pull/16) |
| 🟩 | 2.21 | MIDI file import and export | [#35](https://github.com/loombridge/beatlayer/pull/35) |
| 🟨 | 2.22 | Alpha release: unsigned builds (ad-hoc signed on macOS, which Apple silicon needs to run at all), installers,… | [#34](https://github.com/loombridge/beatlayer/pull/34) |
| 🟩 | 2.23 | Built-in assistant: the panel, the agent loop, providers (OpenAI-compatible including local servers,… | [#45](https://github.com/loombridge/beatlayer/pull/45) |
| 🟩 | 2.24 | Info View: the help text for whatever the pointer is over, shown and hidden as in Live, written from each… | [#24](https://github.com/loombridge/beatlayer/pull/24) |
| 🟩 | 2.25 | Templates and file management: the default set and templates, Collect All and Save, and finding and relinking… | [#15](https://github.com/loombridge/beatlayer/pull/15) |
| 🟩 | 2.26 | Export, part 2: one track or the selected tracks as well as the Main output, and MP3 at 320 kbps | [#2](https://github.com/loombridge/beatlayer/pull/2) |
| 🟩 | 2.27 | Export, part 3: Beatlayer's own answer to Live's POW-r dither (licensed, so not reproduced): research and… | [#32](https://github.com/loombridge/beatlayer/pull/32) |
| 🟩 | 2.28 | Agent previews off the main thread: perception.preview (WP-1.16) renders on a worker thread, so a long… | [#1](https://github.com/loombridge/beatlayer/pull/1) |
| 🟩 | 2.29 | MIDI controller data in clips (the spec R1 to R9): a MIDI clip's controller envelopes (each CC, pitch bend,… | [#145](https://github.com/loombridge/beatlayer/pull/145) |
| 🟩 | 2.29b | Recording MIDI controller data (the spec R10 to R16): Arrangement Record and MIDI Arrangement Overdub,… | [#176](https://github.com/loombridge/beatlayer/pull/176) |

</details>

<details>
<summary><b>Phase 3: Session view</b>: 18 of 18 done</summary>

_Perform: launch clips and scenes with quantisation, follow actions, and record a performance into the arrangement._

| | WP | Package | PR |
|---|---|---|---|
| 🟩 | 3.1 | Clip slots and scenes in the document: each track's slots, the scenes, and their commands | [#53](https://github.com/loombridge/beatlayer/pull/53) |
| 🟩 | 3.2 | Clip launching, launch quantisation and stop buttons: the engine's slot states, launch and stop commands, the… | [#62](https://github.com/loombridge/beatlayer/pull/62) |
| 🟩 | 3.3 | Launch modes: Trigger, Gate, Toggle, Repeat | [#80](https://github.com/loombridge/beatlayer/pull/80) |
| 🟩 | 3.4 | The clip's launch settings (quantisation, legato, velocity, nudge) and the launch defaults | [#79](https://github.com/loombridge/beatlayer/pull/79) |
| 🟩 | 3.5 | Scenes: tempo and time signature, the Scene View, and Capture and Insert Scene | [#85](https://github.com/loombridge/beatlayer/pull/85) |
| 🟩 | 3.6 | Session clips over the Arrangement, and Back to Arrangement | [#82](https://github.com/loombridge/beatlayer/pull/82) |
| 🟩 | 3.7 | The Session View grid | [#58](https://github.com/loombridge/beatlayer/pull/58) |
| 🟩 | 3.8 | Launching from the grid: the status row and the Main track column | [#87](https://github.com/loombridge/beatlayer/pull/87) |
| 🟩 | 3.9 | The Clip View's Launch Properties panel | [#96](https://github.com/loombridge/beatlayer/pull/96) |
| 🟩 | 3.10 | Follow actions on clips and scenes | [#150](https://github.com/loombridge/beatlayer/pull/150) |
| 🟩 | 3.11 | Session recording and overdub | [#86](https://github.com/loombridge/beatlayer/pull/86) |
| 🟩 | 3.12 | Session automation recording | [#135](https://github.com/loombridge/beatlayer/pull/135) |
| 🟩 | 3.13 | Recording the session into the Arrangement | [#116](https://github.com/loombridge/beatlayer/pull/116) |
| 🟩 | 3.14 | Capture MIDI | [#129](https://github.com/loombridge/beatlayer/pull/129) |
| 🟩 | 3.15 | Clip envelopes: the document and playback | [#61](https://github.com/loombridge/beatlayer/pull/61) |
| 🟩 | 3.16 | Clip envelopes: the Clip View editor | [#91](https://github.com/loombridge/beatlayer/pull/91) |
| 🟩 | 3.17 | Crossfader and per-track A/B assignment | [#55](https://github.com/loombridge/beatlayer/pull/55) |
| 🟩 | 3.18 | Cue output and Cue mode | [#198](https://github.com/loombridge/beatlayer/pull/198) |

</details>

<details>
<summary><b>Phase 4: Warping and audio editing</b>: 9 of 23 done</summary>

_Work with loops and recordings at any tempo, slice them to MIDI, freeze, and bounce._

| | WP | Package | PR |
|---|---|---|---|
| 🟩 | 4.1 | The warp map in the document: the Warp switch, warp markers, a warped clip's markers and loop in beats, the… | [#101](https://github.com/loombridge/beatlayer/pull/101) |
| 🟩 | 4.2 | Warped playback and Re-Pitch: every clip player follows the warp map through the tempo map; Re-Pitch; only… | [#178](https://github.com/loombridge/beatlayer/pull/178) |
| 🟩 | 4.3 | Transient detection and the analysis cache: each file's transients kept by content hash; Insert and Delete… | [#169](https://github.com/loombridge/beatlayer/pull/169) |
| ⬜ | 4.4 | Beats mode: Preserve, Transient Loop Mode, Transient Envelope |  |
| ⬜ | 4.5 | Tones and Texture modes: Grain Size, Flux |  |
| ⬜ | 4.6 | Complex and Complex Pro through Signalsmith Stretch (D11), with Formants and Envelope |  |
| ⬜ | 4.7 | Clip Transpose and Detune through the warp modes, and Reverse |  |
| ⬜ | 4.8 | Warp settings as automation and clip-envelope targets (IMP-09, G2) |  |
| 🟩 | 4.9 | Tempo detection and auto-warp: Auto-Warp Long Samples, Loop/Warp Short Samples, Default Warp Mode; Set 1.1.1… | [#184](https://github.com/loombridge/beatlayer/pull/184) |
| 🟩 | 4.10 | The Sample Editor in the Clip View: the waveform with warp, transient, and pseudo-warp markers; making,… | [#190](https://github.com/loombridge/beatlayer/pull/190) |
| ⬜ | 4.10b | The audio clip's panels in the Clip View: the Audio panel's controls (L-CLIP-008), the clip header, Edit in… |  |
| ⬜ | 4.11 | Stretching a warped clip in the Arrangement (S-ARR-006), sliding a clip's contents (I-DRAG-013, I-DRAG-014),… |  |
| ⬜ | 4.12 | Audio quantise: Quantize on audio clips (Cmd+U and its settings) moves warp markers to the grid |  |
| ⬜ | 4.13 | Crop Clip, Crop Sample, Consolidate, and Consolidate Time to New Scene |  |
| ⬜ | 4.14 | Freeze Track and unfreezing, including group and return tracks (IMP-19) |  |
| ⬜ | 4.15 | Bounce Track in Place, Bounce to New Track, Bounce Group, Paste Bounced Audio, and mono bounces (IMP-10, G3) |  |
| ⬜ | 4.16 | Slice to New MIDI Track: by transients, warp markers, or beat divisions, into a Drum Rack of samplers or one… |  |
| 🟩 | 4.17 | Export, part 4: Render as Loop | [#112](https://github.com/loombridge/beatlayer/pull/112) |
| ⬜ | 4.18 | Tempo-synced preview in the browser |  |
| ⬜ | 4.19 | Warp mode measurements: golden references and measured targets per mode across tempos |  |
| 🟩 | 4.20 | Processed audio files: one writer for Reverse, Crop Sample, Consolidate, Freeze, and Bounce, into the… | [#99](https://github.com/loombridge/beatlayer/pull/99) |
| 🟩 | 4.21 | Offline renders with VST3 and Audio Unit plug-ins, as WP-2.6b made them for CLAP | [#104](https://github.com/loombridge/beatlayer/pull/104) |
| 🟩 | 4.22 | The track header's context menu (C-TRACK), with the items that work today and the rest drawn as their… | [#108](https://github.com/loombridge/beatlayer/pull/108) |

</details>

<details>
<summary><b>Phase 5: Racks, modulation, and core devices</b>: 29 of 50 done</summary>

_Build racks with macros, program drum racks, modulate anything, and use every core effect and MIDI effect._

| | WP | Package | PR |
|---|---|---|---|
| 🟩 | 5.1 | Racks in the document and engine: Instrument, Audio Effect, MIDI Effect, and Drum racks; parallel chains with… | [#105](https://github.com/loombridge/beatlayer/pull/105) |
| 🟩 | 5.2 | The rack in the Device View: the title bar's view switches, the chain list (Show and Hide Chains), chain… | [#141](https://github.com/loombridge/beatlayer/pull/141) |
| 🟩 | 5.3 | Zones: key, velocity, and chain-select zones with fade ranges; the zone editor; Auto Select | [#195](https://github.com/loombridge/beatlayer/pull/195) |
| ⬜ | 5.4 | Macros: up to 16, map mode with per-mapping ranges, Randomize, names; stepped and radio-button macro styles… |  |
| ⬜ | 5.5 | Macro variations: store, recall, launch, exclude |  |
| 🟩 | 5.6 | Drum Rack: pads and the pad view, a sample dropped on a pad, Receive and Play notes, choke groups, pad mute… | [#196](https://github.com/loombridge/beatlayer/pull/196) |
| 🟩 | 5.7 | Rack chain taps as routing sources | [#137](https://github.com/loombridge/beatlayer/pull/137) |
| 🟩 | 5.8 | Rack presets and hot-swap (Q) in the browser | [#181](https://github.com/loombridge/beatlayer/pull/181) |
| 🟩 | 5.9 | Relative modulation: modulation on top of automation in the engine and the Arrangement, on every parameter,… | [#107](https://github.com/loombridge/beatlayer/pull/107) |
| 🟩 | 5.10 | Modulators, part 1: LFO and Shaper, and how a modulator maps to its targets | [#133](https://github.com/loombridge/beatlayer/pull/133) |
| 🟩 | 5.11 | Modulators, part 2: Envelope Follower, Envelope MIDI, Shaper MIDI, Expression Control | [#163](https://github.com/loombridge/beatlayer/pull/163) |
| 🟩 | 5.12 | Device A/B compare (P, Edit ▸ Compare) | [#121](https://github.com/loombridge/beatlayer/pull/121) |
| 🟩 | 5.13 | Multisample sampler, part 1: zones (key, velocity, sample select), the zone editor, playback and voices | [#126](https://github.com/loombridge/beatlayer/pull/126) |
| 🟩 | 5.14 | Multisample sampler, part 2: its remaining tabs (pitch and oscillator, filter and global, modulation, MIDI)… | [#175](https://github.com/loombridge/beatlayer/pull/175) |
| 🟩 | 5.15 | The sampler devices' sample displays, and sample editing in them (IMP-28) | [#174](https://github.com/loombridge/beatlayer/pull/174) |
| ⬜ | 5.15b | The sampler's Warp controls, manual slicing in its display, and its pitch envelope |  |
| ⬜ | 5.16 | The autosampler (IMP-27) |  |
| 🟩 | 5.17 | Beatlayer's equivalent of Impulse | [#128](https://github.com/loombridge/beatlayer/pull/128) |
| 🟩 | 5.18 | Beatlayer's equivalent of Drum Sampler | [#134](https://github.com/loombridge/beatlayer/pull/134) |
| ⬜ | 5.19 | Rack chains in the Mixer: each chain's strip beside its track's in the Session mixer, and in Mixer in… |  |
| 🟩 | 5.20 | Equivalent of Auto Filter | [#142](https://github.com/loombridge/beatlayer/pull/142) |
| ⬜ | 5.20b | Equivalents of Phaser-Flanger and Chorus-Ensemble |  |
| ⬜ | 5.20c | Equivalent of Auto Pan-Tremolo |  |
| 🟩 | 5.21 | Equivalents of Channel EQ, EQ Three, Gate, and Glue Compressor | [#148](https://github.com/loombridge/beatlayer/pull/148) |
| 🟩 | 5.22 | Equivalent of Multiband Dynamics | [#149](https://github.com/loombridge/beatlayer/pull/149) |
| ⬜ | 5.22b | Equivalent of Drum Buss |  |
| 🟩 | 5.23 | Equivalents of Saturator, Overdrive, and Dynamic Tube | [#156](https://github.com/loombridge/beatlayer/pull/156) |
| 🟩 | 5.24 | Equivalents of Erosion, Redux, and Vinyl Distortion | [#155](https://github.com/loombridge/beatlayer/pull/155) |
| 🟩 | 5.25 | Equivalents of Filter Delay, Grain Delay, and Align Delay | [#158](https://github.com/loombridge/beatlayer/pull/158) |
| 🟩 | 5.26 | Equivalent of Beat Repeat | [#161](https://github.com/loombridge/beatlayer/pull/161) |
| ⬜ | 5.26b | Equivalent of Looper |  |
| 🟩 | 5.27 | Equivalent of Shifter | [#177](https://github.com/loombridge/beatlayer/pull/177) |
| ⬜ | 5.28 | Equivalents of Spectrum and Tuner, and an oscilloscope (IMP-26) |  |
| ⬜ | 5.29 | Gain-reduction meters on channel strips, Deactivate All Devices, and Close All Plug-in Windows (IMP-25) |  |
| 🟩 | 5.30 | MIDI effects, part 1: equivalents of Pitch, Velocity, Random, Scale, Chord, and Note Length, with the note… | [#109](https://github.com/loombridge/beatlayer/pull/109) |
| ⬜ | 5.31 | MIDI effects, part 2: equivalents of Arpeggiator and Note Echo |  |
| ⬜ | 5.32 | MIDI effects, part 3: equivalents of CC Control, MPE Control, and MIDI Monitor |  |
| ⬜ | 5.33 | Plug-ins as MIDI effects (IMP-24) |  |
| 🟩 | 5.34 | MIDI out to hardware on a track's MIDI To, and External Instrument with its hardware latency | [#170](https://github.com/loombridge/beatlayer/pull/170) |
| ⬜ | 5.35 | External Audio Effect with its hardware latency, and real-time export with external hardware |  |
| ⬜ | 5.36 | Track Options and latency switches: track delay in milliseconds or samples and Keep Monitoring Latency;… |  |
| 🟩 | 5.37 | The multicore scheduler: a real-time worker pool runs independent chain segments in parallel | [#152](https://github.com/loombridge/beatlayer/pull/152) |
| ⬜ | 5.38 | Deactivated devices and tracks cost nothing (IMP-15), and per-track performance impact |  |
| ⬜ | 5.39 | LV2 hosting on Linux |  |
| ⬜ | 5.40 | Plug-in configure mode, and remapping the plug-in panel's knobs (IMP-03) |  |
| ⬜ | 5.41 | ARA 2 hosting (IMP-20) |  |
| 🟩 | 5.42 | Shared parts for the effect devices: an effects LFO, an envelope follower, seeded randomness, and the device… | [#103](https://github.com/loombridge/beatlayer/pull/103) |
| 🟩 | 5.43 | Device displays: a device's display data reaches its display cells and agents, with the EQ's and the… | [#117](https://github.com/loombridge/beatlayer/pull/117) |
| 🟩 | 5.44 | MIDI channel messages between devices: control change, pitch bend, and channel pressure through device chains | [#124](https://github.com/loombridge/beatlayer/pull/124) |
| ⬜ | 5.45 | Multi-output instruments: an instrument's extra outputs as routing sources |  |

</details>

<details>
<summary><b>Phases 6 to 10</b>: their epics</summary>

| Phase | Release | Epics |
|---|---|---|
| **6** Editing depth | beta 0.5 | The return-tracks scrolling option (IMP-01); save history (IMP-05); hiding plug-ins in the browser (IMP-07); DAWproject import and export; MIDI Tools (transformations and generators); pitch and time utilities; scales and keys; tuning systems; MPE editing; probability, chance, and note-splitting edits; find and select notes; step input; take lanes and comping; linked-track editing; groove pool; locators, copy, insert, and delete time; multi-clip editing; stacked detail views; undo history; second window; browser tags, collections (nested, IMP-07), and filters; the plug-in list's view (IMP-29); themes; project archives and set merging |
| **7** Control and sync | 0.6 | MIDI and key mapping; MIDI clock in and out; MIDI timecode in; Ableton Link 4, including Link Audio; CV tools; the control-surface scripting host; generic controller templates; Push 2 (public interface documentation) and grid controllers |
| **8** Sound-design devices | 0.7 | Wavetable, FM, analogue-modelled, physical-modelling, granular, spectral, and bi-timbral instruments; drum synthesis; hybrid and convolution reverb; multi-stage distortion; vocoder; resonators; amp, cabinet, and pedal; pitch correction; the remaining delays and spectral effects |
| **9** Intelligence, interop, extensions | 0.8 | Stem separation with GPU acceleration and a speed target (IMP-11); audio to MIDI with measured accuracy (IMP-06); similarity search and automatic tagging; tempo follower; Live set and preset import with legacy-device mapping; video tracks and export; the extension SDK; the modular device; extension-built equivalents of Suite's Max devices; a bundled open content library; translations |
| **10** Hardening and 1.0 | 1.0 | Plug-in sandbox and Rosetta bridge; scheduler tuning; a plug-in compatibility farm; opt-in crash reports; updates; the user manual and built-in lessons; an accessibility audit with screen-reader users; a security review of plug-in loading and file parsing; Arm builds for Windows and Linux; a legal review |

</details>

## Parity with Live 12

One square per Live feature for each milestone that delivers part of it, from the parity matrix. A feature's status comes from the work packages that build it.

| Milestone | Release | Done | Features |
|---|---|--:|---|
| **M1** Playback core | internal | 25 of 25 | 🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩 |
| **M2** Make a track | in alpha 0.2 | 37 of 37 | 🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩 |
| **M3** Session view | alpha 0.2 | 14 of 14 | 🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩 |
| **M4** Warping | 0.3 | 4 of 15 | 🟩🟩🟩🟩🟨🟨⬜⬜⬜⬜⬜⬜⬜⬜⬜ |
| **M5** Racks and core devices | 0.4 | 13 of 27 | 🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟩🟨🟨🟨🟨⬜⬜⬜⬜⬜⬜⬜⬜⬜⬜ |
| **M6** Editing depth | beta 0.5 | 0 of 29 | ▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️ |
| **M7** Control and sync | 0.6 | 0 of 9 | ▫️▫️▫️▫️▫️▫️▫️▫️▫️ |
| **M8** Sound-design devices | 0.7 | 0 of 13 | ▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️ |
| **M9** Intelligence and interop | 0.8 | 0 of 15 | ▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️▫️ |
| **M10** Hardening | 1.0 | 0 of 2 | ▫️▫️ |
| Out of scope | | | ⬛⬛⬛⬛⬛⬛⬛⬛⬛ |

<details>
<summary><b>M1 Playback core</b>: 25 of 25 done</summary>

| | Area | Feature |
|---|---|---|
| 🟩 | Interface placement and key bindings | Placement of every view, panel, and control |
| 🟩 | Interface placement and key bindings | Menu bar and context menus: structure, order, and wording of generic commands |
| 🟩 | Interface placement and key bindings | Keyboard shortcuts, macOS and Windows, including held (momentary) shortcuts |
| 🟩 | Interface placement and key bindings | Mouse gestures and modifier-key behaviours |
| 🟩 | Views and workflow | Arrangement View |
| 🟩 | Views and workflow | Mixer, with sections you can show or hide |
| 🟩 | Views and workflow | Control Bar, Status Bar |
| 🟩 | Views and workflow | Interface zoom, per window |
| 🟩 | Views and workflow | Keyboard navigation (focus, area jumps, held shortcuts) |
| 🟩 | Views and workflow | Screen readers (VoiceOver, NVDA, Narrator) |
| 🟩 | Views and workflow | Browser: places, search, preview |
| 🟩 | Audio clips and warping | Clip gain, fades and crossfades, Normalize |
| 🟩 | Audio clips and warping | Split, Crop, Crop to Time Selection (split) |
| 🟩 | Mixing and routing | Audio, MIDI, and Main tracks (audio, Main) |
| 🟩 | Mixing and routing | Group tracks, nested |
| 🟩 | Mixing and routing | Solo and cue modes, cue output, preview volume (preview) |
| 🟩 | Mixing and routing | CPU meter, disk overload indicator |
| 🟩 | Files and export | Sets, templates, Collect All and Save, File Manager (missing files) (project files) |
| 🟩 | Files and export | Crash recovery: the next launch offers the unsaved set |
| 🟩 | Files and export | Save Continuously, with a history of save points (IMP-05) (the journal and the option) _(Beatlayer only)_ |
| 🟩 | Files and export | Audio import: WAV, AIFF, AIFF-C, FLAC, Ogg Vorbis, MP3, M4A (AAC, ALAC), RF64 and BWF over 4 GB |
| 🟩 | Files and export | Export: Main, all tracks as stems, selected tracks, one track; include returns and Main effects; mono; normalise (Main) |
| 🟩 | Files and export | Export formats: WAV, AIFF, FLAC at 16, 24, or 32 bits; MP3 at 320 kbps (WAV, AIFF, FLAC) |
| 🟩 | Files and export | Dither: rectangular, triangular |
| 🟩 | Files and export | Agent control of every feature through MCP, headless or alongside the app _(Beatlayer only)_ |

</details>

<details>
<summary><b>M2 Make a track</b>: 37 of 37 done</summary>

| | Area | Feature |
|---|---|---|
| 🟩 | Views and workflow | Info View |
| 🟩 | Clips and launching | MIDI controller clip envelopes (MIDI Ctrl), per-note pressure, and recording controller data |
| 🟩 | MIDI editing | Draw, select, move, resize, velocity lane, quantise, transpose, fold |
| 🟩 | MIDI editing | Release velocity, velocity deviation, mute, note chance (data and playback) |
| 🟩 | Recording | Arrangement recording, several tracks at once |
| 🟩 | Recording | Punch-in and punch-out |
| 🟩 | Recording | Loop recording into take lanes |
| 🟩 | Recording | MIDI Arrangement Overdub, record quantisation |
| 🟩 | Recording | Count-in, metronome sounds and subdivision, metronome only while recording |
| 🟩 | Recording | Monitoring In, Auto, Off; Keep Monitoring Latency (monitoring) |
| 🟩 | Recording | Recording file type and bit depth, up to 32-bit and 192 kHz |
| 🟩 | Mixing and routing | Audio, MIDI, and Main tracks (MIDI) |
| 🟩 | Mixing and routing | Return tracks and sends |
| 🟩 | Mixing and routing | Automatic plug-in delay compensation, driver error compensation |
| 🟩 | Mixing and routing | Routing taps: before effects, after effects, after mixer; rack chain taps (track taps) |
| 🟩 | Mixing and routing | Resampling, MIDI track to MIDI track, multi-output instruments (resampling, track to track) |
| 🟩 | Mixing and routing | Sidechain to devices and plug-ins |
| 🟩 | Mixing and routing | Up to 256 mono inputs and outputs |
| 🟩 | Automation and modulation | Arrangement automation lanes, several per track; Automation Arm; Re-Enable Automation; tempo automation |
| 🟩 | Automation and modulation | Curved segments |
| 🟩 | Automation and modulation | Computer MIDI keyboard |
| 🟩 | Racks and device features | Rename devices, device presets |
| 🟩 | Racks and device features | Device parameters and controls: knobs, switches, choosers, number boxes, display cells |
| 🟩 | Instruments | Simpler |
| 🟩 | Instruments | Drift |
| 🟩 | Audio effects | EQ Eight, Compressor, Limiter, Reverb, Delay, Utility |
| 🟩 | Plug-ins | VST3 |
| 🟩 | Plug-ins | Audio Units v2 and v3 |
| 🟩 | Plug-ins | Plug-in configure mode, MPE plug-ins, multi-output plug-ins (MPE) |
| 🟩 | Plug-ins | CLAP _(Beatlayer only)_ |
| 🟩 | Files and export | Sets, templates, Collect All and Save, File Manager (missing files) (templates, collect, relink) |
| 🟩 | Files and export | Clip, device preset, and rack preset files (device presets) |
| 🟩 | Files and export | Standard MIDI Files, import and export |
| 🟩 | Files and export | Export: Main, all tracks as stems, selected tracks, one track; include returns and Main effects; mono; normalise (one track) |
| 🟩 | Files and export | Export formats: WAV, AIFF, FLAC at 16, 24, or 32 bits; MP3 at 320 kbps (MP3) |
| 🟩 | Files and export | Built-in assistant on any cloud or local model _(Beatlayer only)_ |
| 🟩 | Other | Tempo automation and time-signature changes |

</details>

<details>
<summary><b>M3 Session view</b>: 14 of 14 done</summary>

| | Area | Feature |
|---|---|---|
| 🟩 | Views and workflow | Session View |
| 🟩 | Clips and launching | Launch modes: Trigger, Gate, Toggle, Repeat |
| 🟩 | Clips and launching | Legato, launch quantisation, velocity amount, nudge, Select on Launch |
| 🟩 | Clips and launching | Clip stop buttons, removable |
| 🟩 | Clips and launching | Follow actions on clips and scenes (No Action, Stop, Play Again, Previous, Next, First, Last, Any, Other, Jump; A and B with chance; linked… |
| 🟩 | Clips and launching | Scene tempo and time signature |
| 🟩 | Clips and launching | Clip envelopes, linked and unlinked |
| 🟩 | Clips and launching | Capture and Insert Scene |
| 🟩 | Clips and launching | Capture MIDI |
| 🟩 | Clips and launching | Crossfader A/B |
| 🟩 | Recording | Session slot recording, Session overdub |
| 🟩 | Recording | Session performance into the Arrangement, Back to Arrangement |
| 🟩 | Mixing and routing | Solo and cue modes, cue output, preview volume (cue) |
| 🟩 | Automation and modulation | Session automation recording |

</details>

<details>
<summary><b>M4 Warping</b>: 4 of 15 done</summary>

| | Area | Feature |
|---|---|---|
| 🟩 | Audio clips and warping | Warp markers, transient markers, pseudo-warp markers |
| 🟩 | Audio clips and warping | Auto-warp long samples, tempo detection, Set 1.1.1 Here, Warp From Here |
| 🟩 | Audio clips and warping | Analysis files (.asd) |
| 🟩 | Files and export | Export: Main, all tracks as stems, selected tracks, one track; include returns and Main effects; mono; normalise (stems, render as loop) |
| 🟨 | Audio clips and warping | Clip pitch, reverse, RAM mode, high-quality interpolation |
| 🟨 | Audio clips and warping | Warp modes: Beats, Tones, Texture, Re-Pitch |
| ⬜ | Views and workflow | Browser: preview in sync with tempo, hot-swap (tempo preview) |
| ⬜ | Audio clips and warping | Warp modes: Complex, Complex Pro (with formants) |
| ⬜ | Audio clips and warping | Clip tempo leaders and followers |
| ⬜ | Audio clips and warping | Audio quantise |
| ⬜ | Audio clips and warping | Split, Crop, Crop to Time Selection (crop) |
| ⬜ | Audio clips and warping | Consolidate |
| ⬜ | Audio clips and warping | Slice to New MIDI Track (transients, warp markers, beat divisions, slicing presets) |
| ⬜ | Audio clips and warping | Freeze Track |
| ⬜ | Audio clips and warping | Bounce Track in Place, Bounce to New Track, Bounce Group, Paste Bounced Audio |

</details>

<details>
<summary><b>M5 Racks and core devices</b>: 13 of 27 done</summary>

| | Area | Feature |
|---|---|---|
| 🟩 | Views and workflow | Browser: preview in sync with tempo, hot-swap (hot-swap) |
| 🟩 | Mixing and routing | Routing taps: before effects, after effects, after mixer; rack chain taps (chain taps) |
| 🟩 | Mixing and routing | Multicore and multiprocessor processing |
| 🟩 | Mixing and routing | MIDI track output to external MIDI ports and channels (hardware synths) |
| 🟩 | Automation and modulation | Relative modulation on top of automation |
| 🟩 | Automation and modulation | Modulators: LFO, Shaper, Envelope Follower, Envelope MIDI, Shaper MIDI, Expression Control |
| 🟩 | Racks and device features | Parallel chains, key, velocity, and chain-select zones with fades, Auto Select |
| 🟩 | Racks and device features | Drum Rack pads, choke groups, return chains, extract chains |
| 🟩 | Racks and device features | Device A/B compare |
| 🟩 | Instruments | Impulse, Drum Sampler |
| 🟩 | Instruments | Sampler (multisample, zones, modulation) |
| 🟩 | Instruments | External Instrument |
| 🟩 | Files and export | Clip, device preset, and rack preset files (racks) |
| 🟨 | Racks and device features | Instrument, Audio Effect, MIDI Effect, and Drum racks |
| 🟨 | Audio effects | Auto Filter, Auto Pan-Tremolo, Beat Repeat, Channel EQ, Chorus-Ensemble, EQ Three, Erosion, Gate, Grain Delay, Looper, Phaser-Flanger,… |
| 🟨 | Audio effects | Align Delay, Drum Buss, Dynamic Tube, Filter Delay, Glue Compressor, Multiband Dynamics, Overdrive, Shifter, Spectrum, Vinyl Distortion |
| 🟨 | MIDI effects | Arpeggiator, CC Control, Chord, MIDI Monitor, MPE Control, Note Echo, Note Length, Pitch, Random, Rotating Rhythm Generator, Scale, Velocity |
| ⬜ | Recording | Monitoring In, Auto, Off; Keep Monitoring Latency (Keep Monitoring Latency) |
| ⬜ | Mixing and routing | Split-stereo panning |
| ⬜ | Mixing and routing | Track delay in milliseconds or samples |
| ⬜ | Mixing and routing | Per-track performance impact |
| ⬜ | Mixing and routing | Resampling, MIDI track to MIDI track, multi-output instruments (multi-output instruments) |
| ⬜ | Automation and modulation | Macros (16 per rack), variations, randomise, map mode with per-mapping ranges |
| ⬜ | Audio effects | External Audio Effect |
| ⬜ | Plug-ins | Plug-in configure mode, MPE plug-ins, multi-output plug-ins (configure mode, multi-out) |
| ⬜ | Plug-ins | LV2 _(Beatlayer only)_ |
| ⬜ | Files and export | Real-time rendering with external hardware, restart after dropouts |

</details>

<details>
<summary><b>M6 Editing depth</b>: 0 of 29 done</summary>

| | Area | Feature |
|---|---|---|
| ▫️ | Views and workflow | Clip View and Device View shown together |
| ▫️ | Views and workflow | Undo History panel |
| ▫️ | Views and workflow | Second window |
| ▫️ | Views and workflow | Themes, light and dark, follow the OS, tone and contrast controls |
| ▫️ | Views and workflow | Browser: collections, tags, tag groups, Tag Editor, Quick Tags, filter view, saved searches, columns, history |
| ▫️ | MIDI editing | Release velocity, velocity deviation, mute, note chance (editing UI) |
| ▫️ | MIDI editing | Probability groups (Play All, Play One) |
| ▫️ | MIDI editing | Split, chop, join, span notes; note stretch markers; deactivate notes |
| ▫️ | MIDI editing | Find and Select Notes |
| ▫️ | MIDI editing | Fold to scale, scale highlighting |
| ▫️ | MIDI editing | Multi-clip editing and focus mode |
| ▫️ | MIDI editing | Step input and step recording |
| ▫️ | MIDI editing | MPE lanes (pitch bend, slide, pressure), Drum Rack pitch-bend lane |
| ▫️ | MIDI editing | MIDI Tools, transformations (Arpeggiate, Chop, Connect, Glissando, LFO, Ornament, Quantize, Recombine, Span, Strum, Time Warp, Velocity… |
| ▫️ | MIDI editing | MIDI Tools, generators (Rhythm, Seed, Shape, Stacks, Euclidean) |
| ▫️ | MIDI editing | Chained MIDI Tools, mapped to keys or MIDI |
| ▫️ | MIDI editing | Pitch and Time Utilities (Fit to Scale, Invert, Transpose, Add Interval, ×2, ÷2, Stretch, Set Length, Humanize, Reverse, Legato) |
| ▫️ | MIDI editing | Scale Mode per clip, MIDI and audio |
| ▫️ | MIDI editing | Tuning systems: .scl and .ascl import, .ascl save, reference pitch, per-track bypass |
| ▫️ | MIDI editing | Groove pool: extract, apply, commit; base, quantise, timing, random, velocity, global amount |
| ▫️ | Recording | Take lanes and comping, take-lane commands |
| ▫️ | Recording | Linked-track editing |
| ▫️ | Automation and modulation | Insert shapes, stretch and skew, simplify, lock envelopes, keyboard breakpoint editing |
| ▫️ | Racks and device features | "Use Current Scale" on devices |
| ▫️ | Files and export | Save Continuously, with a history of save points (IMP-05) (the history browser) _(Beatlayer only)_ |
| ▫️ | Files and export | Clip, device preset, and rack preset files (clips) |
| ▫️ | Files and export | Pack a project into one archive, export Session clips as a new set, merge sets |
| ▫️ | Files and export | DAWproject import and export _(Beatlayer only)_ |
| ▫️ | Other | Locators, Copy Time, insert and delete time |

</details>

<details>
<summary><b>M7 Control and sync</b>: 0 of 9 done</summary>

| | Area | Feature |
|---|---|---|
| ▫️ | Automation and modulation | MIDI map mode, key map mode, takeover modes, relative controllers, instant mappings |
| ▫️ | Instruments | CV Instrument, CV Triggers |
| ▫️ | Audio effects | CV tools (Clock In and Out, Envelope Follower, In, LFO, Shaper, Utility) |
| ▫️ | Sync and control | Ableton Link: tempo, phase, Start Stop Sync |
| ▫️ | Sync and control | Link Audio |
| ▫️ | Sync and control | MIDI Clock in and out with song position; sync delay per port |
| ▫️ | Sync and control | MIDI Timecode in |
| ▫️ | Sync and control | Control-surface scripts (Live ships 139, in Python) |
| ▫️ | Sync and control | Push 2 as a controller |

</details>

<details>
<summary><b>M8 Sound-design devices</b>: 0 of 13 done</summary>

| | Area | Feature |
|---|---|---|
| ▫️ | Instruments | Analog |
| ▫️ | Instruments | Operator |
| ▫️ | Instruments | Wavetable |
| ▫️ | Instruments | Meld |
| ▫️ | Instruments | Collision, Tension, Electric |
| ▫️ | Instruments | Drum Synths (Kick, Snare, Clap, Hi-hat, Cymbal, Tom, FM, Clang) |
| ▫️ | Instruments | Granulator III |
| ▫️ | Instruments | Bass, Poli |
| ▫️ | Audio effects | Auto Shift (pitch correction) |
| ▫️ | Audio effects | Corpus, Resonators, Vocoder |
| ▫️ | Audio effects | Amp, Cabinet, Pedal |
| ▫️ | Audio effects | Echo, Hybrid Reverb, Roar, Spectral Resonator, Spectral Time |
| ▫️ | Audio effects | Convolution Reverb, Color Limiter, Gated Delay, Pitch Hack, PitchLoop89, Re-Enveloper, Spectral Blur, Vector Delay |

</details>

<details>
<summary><b>M9 Intelligence and interop</b>: 0 of 15 done</summary>

| | Area | Feature |
|---|---|---|
| ▫️ | Views and workflow | Browser: automatic tagging of samples and plug-ins |
| ▫️ | Views and workflow | Browser: similarity search and similar-sample swapping |
| ▫️ | Audio clips and warping | Convert Drums, Melody, or Harmony to New MIDI Track |
| ▫️ | Audio clips and warping | Stem separation (vocals, drums, bass, others; fast and high-quality modes; files, clips, time selections; merge stems) |
| ▫️ | Recording | Tempo Follower |
| ▫️ | Racks and device features | Max for Live API (Live Object Model) |
| ▫️ | Instruments | Inspired by Nature (Tree Tone, Emit, Vector FM, Vector Grain) |
| ▫️ | Audio effects | Legacy devices (Auto Filter Legacy, Chorus, Flanger, Phaser, Frequency Shifter, and others) |
| ▫️ | MIDI effects | Melodic Steps, Bouncy Notes, Step Arp, SQ Sequencer, Rhythmic Steps (the Sequencer Pack) |
| ▫️ | MIDI effects | MIDI Tools Pack (extra rhythmic and melodic generators and transformations) |
| ▫️ | MIDI effects | Performance Pack (Performer, Variations, Arrangement Looper, Prearranger) |
| ▫️ | Files and export | Live set import (.als) |
| ▫️ | Files and export | Live device and rack preset import (.adv, .adg) |
| ▫️ | Files and export | Video in the arrangement, video window, video export |
| ▫️ | Other | (Beatlayer's own) a library covering the same ground as Live's Packs, tagged on arrival |

</details>

<details>
<summary><b>M10 Hardening</b>: 0 of 2 done</summary>

| | Area | Feature |
|---|---|---|
| ▫️ | Views and workflow | Learn View (built-in lessons) |
| ▫️ | Sync and control | Push 3 as a controller |

</details>

<details>
<summary><b>Out of scope for 1.0</b></summary>

| Area | Feature | Why |
|---|---|---|
| Views and workflow | Browser: Splice, Ableton Cloud, Push places | Vendor services |
| Audio clips and warping | REX files | The REX SDK is proprietary; in scope only if its terms allow use in a GPL-3.0 application |
| Mixing and routing | Surround mixing | Live 12 is stereo apart from one Max device; revisit after 1.0 |
| Audio effects | Surround Panner | No surround mixing in 1.0 |
| Sync and control | Push 3 standalone, Move, Note, Ableton Cloud sync | Closed platforms and services |
| Plug-ins | VST2 | Steinberg no longer licenses the VST2 SDK |
| Files and export | Packs (.alp), Max devices (.amxd), Move and Note bundles (.ablbundle) | Ableton content and closed platforms |
| Files and export | Dither: POW-r 1, 2, 3 | A licensed proprietary algorithm; Beatlayer's own noise-shaped dither takes its place, built in WP-2.27 |
| Other | Ableton's sound library (samples, presets, racks, grooves, packs) | Ableton's copyright; Beatlayer ships its own open-licensed library in M9 |

</details>

## Beyond Live

The [improvements over Live](05-improvements.md): what Beatlayer does better without making a Live user relearn anything.

| | ID | Improvement | Built in |
|---|---|---|---|
| 🟩 | IMP-02 | Record audio without warping | WP-2.13 |
| 🟩 | IMP-04 | Open in Arrangement View on a new install | WP-1.7 |
| 🟩 | IMP-08 | Delay input gain down to silence | Each delay device; WP-2.18b first |
| 🟩 | IMP-12 | Duplicate a track without its clips | WP-2.19b (the Edit menu item and track.duplicate_without_clips); the track context menu's item and colour wait for that… |
| 🟩 | IMP-13 | Agent server settings | WP-1.15; WP-1.15b (each agent's own token); the page of its own, #20 |
| 🟩 | IMP-14 | Floating, resizable device windows | WP-2.5 (the layout model: control bands and stretchable display cells); WP-2.5b (the floating windows) |
| 🟩 | IMP-28 | Sample editing in Simpler and Sampler | WP-5.15, for both sampler devices' displays |
| 🟨 | IMP-01 | Unlimited return tracks, optionally scrolling with the other tracks | WP-2.19 (unlimited, built: returns past Z take two letters); Phase 6 (the option) |
| 🟨 | IMP-03 | A useful plug-in device panel | WP-2.10 (panel); WP-5.40 (remapping with configure mode) |
| 🟨 | IMP-05 | Continuous save | WP-1.11 (journal and option); Phase 6 (history browser) |
| 🟨 | IMP-07 | Hide plug-ins in the browser | WP-2.9 (scanner data); Phase 6 (browser, nested collections) |
| 🟨 | IMP-11 | Fast stem separation | Built ahead of Phase 9 in #202: Arrangement clips, on the CPU, with ONNX Runtime and the model downloaded on first use.… |
| 🟨 | IMP-15 | Deactivated devices and tracks cost nothing | Device framework and graph compiler (after WP-2.11 and WP-2.5); WP-5.38, after the scheduler (WP-5.37) |
| 🟨 | IMP-29 | List of a set's plug-ins | WP-2.9 (the command); Phase 6 (the view) |
| ⬜ | IMP-09 | Automatable formants in the Complex Pro equivalent | WP-4.8 |
| ⬜ | IMP-10 | Mono bounce in place | WP-4.15 |
| ⬜ | IMP-19 | Freeze groups and return tracks | WP-4.14, with freeze |
| ⬜ | IMP-20 | ARA 2 hosting, then a native pitch editor | WP-5.41 (ARA hosting); Phase 8 or 9 (the pitch editor) |
| ⬜ | IMP-24 | Plug-ins as MIDI effects | WP-5.33 |
| ⬜ | IMP-25 | Mixer additions | WP-5.29 (meters, menu items); Phase 6 (the device section) |
| ⬜ | IMP-26 | Smaller additions | WP-5.4, WP-5.28, and Phase 6, as listed |
| ⬜ | IMP-27 | Autosampler | WP-5.16, with the multisample sampler |
| ▫️ | IMP-06 | Audio to MIDI that needs no clean-up | Phase 9 |
| ▫️ | IMP-16 | Command palette | Phase 6 |
| ▫️ | IMP-17 | Your own keyboard shortcuts | Phase 6 |
| ▫️ | IMP-18 | Track search | Phase 6 |
| ▫️ | IMP-21 | Linked clips | Phase 6 |
| ▫️ | IMP-22 | Arranger track | Phase 6 |
| ▫️ | IMP-23 | Latch and Write automation modes | Phase 6 |

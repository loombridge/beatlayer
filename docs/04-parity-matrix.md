# 04. Parity matrix

This is the definition of done for parity with Ableton Live 12. What Beatlayer adds beyond Live is in [the improvements](05-improvements.md). Each row is a Live feature and the milestone where Beatlayer covers it. **Out** means out of scope for 1.0, with the reason. The source is Live 12's release notes, reference manual, and edition comparison. The reference version observed is 12.4.3.

Live is sold in three editions (Intro, Standard, Suite). Beatlayer has one edition with everything, so edition limits such as Intro's 16 tracks don't apply.

Device names in this page are Live's, and Beatlayer's devices take the same names ([clean-room policy](https://github.com/loombridge/beatlayer/blob/main/CONTRIBUTING.md#building-without-abletons-code)).

Each row's **Notes** name the work packages that build it ("Built in WP-1.8a, WP-1.8b"), and [the progress dashboard](../xtask/src/progress.rs) (`cargo xtask dashboard`) shows the row's status from theirs in the build plan's status table. In a row split across milestones, a package counts for the part in its own phase. A row in Phases 0 to 2 that names no package shows as unassigned, a gap in the plan; later phases are planned until they're broken into packages. When Ableton ships a release, add its features here first.

## Live releases covered

| Release | Date | Headline features |
|---|---|---|
| 12.0 | March 2024 | MIDI Tools, scales, tuning systems, browser tags and similarity search, Mixer in Arrangement, stacked Clip and Device views, keyboard navigation and screen readers, Meld, Roar, Granulator III |
| 12.1 | 8 October 2024 | Auto Shift, Drum Sampler, redesigned Limiter and Saturator, Find and Select Notes, chained MIDI Tools, scales on audio clips, Undo History, interface zoom |
| 12.2 | 11 June 2025 | Redesigned Auto Filter, Bounce Track in Place and Bounce to New Track (replacing Freeze and Flatten), keyboard editing of automation, browser Quick Tags, GPU renderer on Windows |
| 12.3 | 25 November 2025 | Stem separation, Splice integration, Auto Pan-Tremolo, Bounce Group, Paste Bounced Audio, device A/B compare |
| 12.4 | 5 May 2026 | Link Audio, stem separation of a time selection, Learn View, Copy Time, updated Erosion, Chorus-Ensemble, and Delay |

## Interface placement and key bindings

| Live feature | Milestone | Notes |
|---|---|---|
| Placement of every view, panel, and control | M1 onwards | Each UI package matches the rows of `docs/ui-parity/layout.md` it implements (D16). Built in WP-1.7, WP-1.8a, WP-1.9, WP-1.10, WP-6.72 (the Clip View's panel arrangement), WP-6.49b (panning with the middle mouse button) |
| Menu bar and context menus: structure, order, and wording of generic commands | M1 onwards | `docs/ui-parity/menus.md`. Built in WP-1.7, WP-1.8b, WP-4.22 (the track header's), WP-6.56 (the Session View's and the Clip View title bar's) |
| Keyboard shortcuts, macOS and Windows, including held (momentary) shortcuts | M1 onwards | `docs/ui-parity/shortcuts.md`; the keymap test fails on a missing binding. Built in WP-1.7, WP-1.8a, WP-1.8b, WP-1.9, WP-1.10, WP-6.70 (the sweep of pending M1 to M3 shortcuts) |
| Mouse gestures and modifier-key behaviours | M1 onwards | `docs/ui-parity/interactions.md`. Built in WP-1.8a, WP-1.8b, WP-1.9, WP-1.10, WP-6.59, WP-6.60 |
| Holding the mouse wheel (middle button) and dragging pans any view that scrolls (12.0) | M6 | [View options](specs/view-options.md) R1. Built in WP-6.49b |

## Views and workflow

| Live feature | Milestone | Notes |
|---|---|---|
| Arrangement View | M1 | The scrub area's hold and Shift-click: WP-6.59. Built in WP-1.8a, WP-1.8b, WP-6.64, WP-6.59 |
| Session View | M3 | Built in WP-3.1, WP-3.7, WP-3.8 |
| Mixer, with sections you can show or hide | M1 | Sections and Mixer in Arrangement too, moved from M6 because M1's mixer keys need them (11 October 2026); each view keeping its own sections (Live 12.0); extended meters and column widths in WP-6.65. Built in WP-1.6, WP-1.9, WP-6.65 |
| Clip View and Device View shown together | M6 | Separate views from M2. Stacked, with the detail area's switches and Expand Clip View ([spec](specs/detail-views.md)). Built in WP-6.27 |
| Control Bar, Status Bar | M1 | Built in WP-1.5, WP-1.7, WP-1.18 |
| Info View | M2 (help text), M6 (the user's own info text) | [Spec](specs/info-view.md). Built in WP-2.24, WP-6.58 |
| Learn View (built-in lessons) | M10 | Part of the manual work |
| Undo History panel | M6 | Undo itself from M0 ([spec](specs/undo-history.md)). Built in WP-6.28 |
| Second window | M6 | [Spec](specs/second-window.md). Built in WP-6.29; clips between the views come with WP-6.29b |
| Tracks: cut, copy, paste and duplicate whole tracks with their clips, take lanes, devices and automation | M6 | [Spec](specs/track-context-menu.md). Built in WP-6.52 |
| Interface zoom, per window | M1 | Global zoom from the first view; per-window zoom in M6. Built in WP-1.17, WP-6.29 |
| Themes, light and dark, follow the OS, tone and contrast controls | M6 | Theme file from M1; Live's `.ask` themes aren't imported ([spec](specs/themes.md)). Built in WP-6.30 |
| Track, clip and scene colours: the colour palette, Auto-Assign Track Colors, Assign Track Color to Clips and to Grouped Tracks and Clips | M6 | Beatlayer's own swatch colours ([spec](specs/themes.md)). Built in WP-6.30b and WP-6.56 (the Session clips', scenes' and Clip View title bar's menus) |
| Keyboard navigation (focus, area jumps, held shortcuts) | M1 onwards | Each view ships with its keyboard path; parity audit in M10. Tab and the neighbour keys by area and track, Wrap Tab Navigation, Outline View in Focus (off, as in Live 12.4.6), menus and context menus from the keyboard, and every command's keyboard path checked against the registry: WP-1.14 ([spec](specs/accessibility.md)). Built in WP-1.7, WP-1.14 |
| Screen readers (VoiceOver, NVDA, Narrator) | M1 onwards | Orca on Linux as well; audit with screen-reader users in M10. Names, roles, and values for tracks, clips, and faders, and the focus as the keys move it, tested headless through AccessKit ([spec](specs/accessibility.md)). Built in WP-1.14 |
| Browser: places, search, preview | M1 (places, search, sample preview), M6 (Full-Height Browser, clip and preset previews) | Full-Height Browser: WP-6.32 ([spec](specs/browser.md) R26); previews of clip files and instrument presets: WP-6.34b, WP-6.67. Built in WP-1.10, WP-6.32, WP-6.34b, WP-6.67 |
| Browser: preview in sync with tempo, hot-swap | M4 (tempo preview), M5 (hot-swap) | Built in WP-4.18, WP-5.8 |
| Browser: collections, tags, tag groups, Tag Editor, Quick Tags, filter view, saved searches, columns, history | M6 | [Spec](specs/browser-tags.md). Built in WP-6.31, WP-6.32, WP-6.32b |
| Browser: automatic tagging of samples and plug-ins | M9 | The Filter View's Function group and Include Auto Tags wait for it |
| Browser: similarity search and similar-sample swapping | M9 | Audio embeddings run locally |
| Browser: Splice, Ableton Cloud, Push places | Out | Vendor services |

## Clips and launching

| Live feature | Milestone | Notes |
|---|---|---|
| Launch modes: Trigger, Gate, Toggle, Repeat | M3 | Behaviour spec first. Built in WP-3.3 |
| Legato, launch quantisation, velocity amount, nudge, Select on Launch | M3 (the settings, Select on Launch), M7 (Legato, a clip's own quantisation and nudging in the engine; velocity on MIDI and key launches) | [Spec](specs/launch-quantisation.md). Built in WP-3.2, WP-3.4, WP-3.9, WP-7.29, WP-7.2 |
| The Clip View's Main Properties panel: Start and End with Show and Set, Duplicate, Loop, Position and Length, Signature | M6 | Its Groove and Scale rows come with grooves and scales ([spec](specs/clip-properties.md)). Built in WP-6.55 |
| Clip stop buttons, removable | M3 | Built in WP-3.2 |
| Follow actions on clips and scenes (No Action, Stop, Play Again, Previous, Next, First, Last, Any, Other, Jump; A and B with chance; linked and unlinked; global switch) | M3 | Seeded randomness so renders repeat. Built in WP-3.10 |
| Scene tempo and time signature | M3 | Built in WP-3.5 |
| Clip envelopes, linked and unlinked | M3 | ([spec](specs/clip-envelopes.md)). Built in WP-3.15 |
| Capture and Insert Scene | M3 | Built in WP-3.5 |
| Capture MIDI | M3 | ([spec](specs/capture-midi.md)). Built in WP-3.14 (notes); its controller data in WP-2.29b |
| MIDI controller clip envelopes (MIDI Ctrl), per-note pressure, and recording controller data | M2 | ([spec](specs/midi-controller-data.md)). WP-2.29 and WP-2.29b |
| Crossfader A/B | M3 | Built in WP-3.17 |

## Audio clips and warping

| Live feature | Milestone | Notes |
|---|---|---|
| Clip gain, fades and crossfades, Normalize | M1 (playback, edge fades), M6 (crossfades, fade handles, Create Fade) | Arrangement playback of clip gain, edge fades, the Start and End markers, and the loop, with Create Fades on Clip Edges' 4 ms: WP-1.4 ([spec](specs/audio-clips.md)). Clips stream from disk, trimming and splitting move the Start marker, and the Create Fades on Clip Edges setting (On, as observed in Live 12.4.6): WP-1.4b. Fade curves, crossfades between adjacent clips, the fade handles, Create Fade and Delete Fades: WP-6.51 ([spec](specs/audio-clips.md) R23 to R31). Normalize isn't built yet. Built in WP-1.4, WP-1.4b, WP-6.51 |
| Clip pitch, reverse, RAM mode, high-quality interpolation | M4 | Built in WP-4.2, WP-4.7 |
| Warp markers, transient markers, pseudo-warp markers | M4 | Built in WP-4.1, WP-4.3, WP-4.10 |
| Warp modes: Beats, Tones, Texture, Re-Pitch | M4 | Native. Built in WP-4.2, WP-4.4, WP-4.5 |
| Warp modes: Complex, Complex Pro (with formants) | M4 | Through the stretch library in D11; formants and every other warp setting automatable (IMP-09). Built in WP-4.6, WP-4.8 |
| Auto-warp long samples, tempo detection, Set 1.1.1 Here, Warp From Here | M4 | Built in WP-4.9 |
| Clip tempo leaders and followers | M4 | Built in WP-4.11 |
| Audio quantise | M4 | Built in WP-4.12 |
| Analysis files (`.asd`) | M4 | Beatlayer keeps its own analysis cache; `.asd` isn't read. Built in WP-4.3 |
| Split, Crop, Crop to Time Selection | M1 (split), M4 (crop) | Built in WP-1.8b, WP-4.13 |
| Consolidate | M4 | Built in WP-4.13 |
| Slice to New MIDI Track (transients, warp markers, beat divisions, slicing presets) | M4 | Built in WP-4.16 |
| Convert Drums, Melody, or Harmony to New MIDI Track | M9 | On-device ML, with measured accuracy targets (IMP-06) |
| Stem separation (vocals, drums, bass, others; fast and high-quality modes; files, clips, time selections; merge stems) | M9 | On-device ML on the GPU, with a speed target (IMP-11). Arrangement clips, both modes and merging built ahead of Phase 9 in [#202](https://github.com/loombridge/beatlayer/pull/202), on the CPU; files, Session clips, time selections and the GPU later in Phase 9 |
| Freeze Track | M4 | Built in WP-4.14 |
| Bounce Track in Place, Bounce to New Track, Bounce Group, Paste Bounced Audio | M4 | Mono sources bounce to mono files (IMP-10). Built in WP-4.15 |
| REX files | Out | The REX SDK is proprietary; in scope only if its terms allow use in a GPL-3.0 application |

## MIDI editing

| Live feature | Milestone | Notes |
|---|---|---|
| Draw, select, move, resize, velocity lane, quantise, transpose, fold | M2 | The insert marker, time selection, keyboard selection and note clipboard: WP-6.53; the zoom, scroll and Follow keys, the clip markers' keys, and the …Time commands inside a clip: WP-6.54; notes paler the softer their velocity: WP-6.49b. Built in WP-2.3, WP-6.53, WP-6.54, WP-6.49b |
| Release velocity, velocity deviation, mute, note chance | M2 (data and playback), M6 (editing UI) | [Spec](specs/note-editing.md). Built in WP-2.1, WP-6.1 |
| Probability groups (Play All, Play One) | M6 | [Spec](specs/note-editing.md). Built in WP-6.2 |
| Split, chop, join and fit notes to the time range (span); split a clip from the Clip View; note stretch markers; deactivate notes | M6 | [Spec](specs/note-editing.md). Built in WP-6.3, WP-6.1 (deactivating) |
| Find and Select Notes, Invert Selection | M6 | [Spec](specs/find-and-select-notes.md). Built in WP-6.4 |
| Fold to scale, scale highlighting | M6 | [Spec](specs/scales.md). Built in WP-6.7b |
| Multi-clip editing and focus mode | M6 | [Spec](specs/multi-clip-editing.md). Built in WP-6.6 |
| Step input and step recording, the MIDI Note Editor's Preview switch | M6 | [Spec](specs/step-recording.md). Built in WP-6.5 |
| MPE lanes (pitch bend, slide, pressure), Drum Rack pitch-bend lane | M6 | Native devices take per-note expression since M2 and M5; per-note expression in clips, MPE controllers and recording in WP-6.17, the MPE tab and the Drum Rack's pitch-bend lane in WP-6.18 ([spec](specs/mpe.md)). Built in WP-6.17, WP-6.18 |
| MIDI Tools, transformations (Arpeggiate, Chop, Connect, Glissando, LFO, Ornament, Quantize, Recombine, Span, Strum, Time Warp, Velocity Shaper) | M6 | Built on the same API that extensions use in M9 ([spec](specs/midi-transformations.md)). Built in WP-6.11, WP-6.12, WP-6.13, WP-6.13b |
| MIDI Tools, generators (Rhythm, Seed, Shape, Stacks, Euclidean) | M6 | Chord banks in Beatlayer's own format ([spec](specs/midi-generators.md)). Built in WP-6.11, WP-6.14, WP-6.14b |
| Chained MIDI Tools, mapped to keys or MIDI | M6 (chains), M7 (mapping to keys or MIDI) | [Spec](specs/midi-tools.md); mapping a chain: [map modes](specs/map-modes.md). Built in WP-6.15, WP-7.3b |
| Pitch and Time Utilities (Fit to Scale, Invert, Transpose, Add Interval, ×2, ÷2, Stretch, Set Length, Humanize, Reverse, Legato) | M6 | [Spec](specs/pitch-and-time-utilities.md). Built in WP-6.16 |
| Scale Mode per clip, MIDI and audio | M6 | Scale-aware devices follow as each device ships ([spec](specs/scales.md)). Built in WP-6.7, WP-6.7b |
| Tuning systems: `.scl` and `.ascl` import, `.ascl` save, reference pitch and note range, per-track bypass, MIDI controller layouts, tunings on MPE plug-ins | M6 | Beatlayer generates its own bundled tunings (EDO, just intonation, historical temperaments) ([spec](specs/tunings.md)). Built in WP-6.9, WP-6.9b, WP-6.10 |
| Groove pool: extract, apply, commit; base, quantise, timing, random, velocity, global amount | M6 | Beatlayer's own groove files and bundled grooves; `.agr` import in M9 ([spec](specs/grooves.md)). Built in WP-6.19, WP-6.20 |

## Recording

| Live feature | Milestone | Notes |
|---|---|---|
| Arrangement recording, several tracks at once | M2 | Built in WP-2.13, WP-2.14 |
| Punch-in and punch-out | M2 | Built in WP-2.13 |
| Loop recording into take lanes | M2 | Built in WP-2.13 |
| Take lanes and comping, take-lane commands | M6 | [Spec](specs/comping.md). Built in WP-6.21, WP-6.22, WP-6.22b |
| Linked-track editing | M6 | [Spec](specs/linked-track-editing.md). Built in WP-6.23 |
| MIDI Arrangement Overdub, record quantisation | M2 (overdub), M7 (record quantisation) | Built in WP-2.14, WP-7.32 |
| Session slot recording, Session overdub | M3 | Built in WP-3.11 |
| Session performance into the Arrangement, Back to Arrangement | M3 | Built in WP-3.13, WP-3.6 |
| Count-in, metronome sounds and subdivision, metronome only while recording | M2 | Built in WP-2.15 |
| Monitoring In, Auto, Off; Keep Monitoring Latency | M2 (monitoring), M5 (Keep Monitoring Latency) | Built in WP-2.13, WP-5.36 |
| Recording file type and bit depth, up to 32-bit and 192 kHz | M2 | Built in WP-2.13 |
| Tempo Follower | M9 | |

## Mixing and routing

| Live feature | Milestone | Notes |
|---|---|---|
| Audio, MIDI, and Main tracks | M1 (audio, Main), M2 (MIDI) | Built in WP-1.6, WP-2.1 |
| Group tracks, nested | M1 | Summing in the engine: WP-1.6. Built in WP-1.6, WP-1.9 |
| Return tracks and sends | M2 | No limit on returns (IMP-01). Sends from audio, MIDI, and group tracks, pre- and post-fader: WP-1.6. Built in WP-2.19 (the knobs, the Pre/Post switches, unlimited returns); returns' own sends with the track context menu |
| Solo and cue modes, cue output, preview volume | M1 (preview), M3 (cue) | Solo, exclusive solo, and Solo in Place in the engine: WP-1.6. Built in WP-1.6, WP-1.10, WP-3.18 |
| Split-stereo panning | M5 | Built in WP-5.36 |
| Track delay in milliseconds or samples | M5 | Built in WP-5.36 |
| CPU meter, disk overload indicator | M1 | Built in WP-1.3, WP-1.7 |
| Track meters, MIDI activity meters on MIDI tracks, and the meters between devices | M1 (track level meters), M6 (MIDI meters, meters between devices) | [Spec](specs/midi-meters.md). Built in WP-1.9, WP-6.69 |
| Per-track performance impact | M5 | Built in WP-5.38 |
| Automatic plug-in delay compensation, driver error compensation | M2 | Built in WP-2.11 |
| Routing taps: before effects, after effects, after mixer; rack chain taps | M2 (track taps), M5 (chain taps) | Built in WP-2.19, WP-5.7 |
| Resampling, MIDI track to MIDI track, multi-output instruments | M2 (resampling, track to track), M5 (multi-output instruments) | Built in WP-2.19 (resampling, track-to-track audio; MIDI routing kept and shown, carried once WP-2.1 plays MIDI); multi-output instruments in WP-5.45 |
| Sidechain to devices and plug-ins | M2 | WP-2.19b: a device's sidechain section picks its source (Live 12 routes no Audio To into a sidechain), External, Mix, SC Gain, Listen ([routing](specs/routing.md) R22 to R31) |
| Up to 256 mono inputs and outputs | M2 | Built in WP-2.13 |
| Multicore and multiprocessor processing | M5 | The parallel graph scheduler in the architecture's "Scheduling and multicore", which the capacity target (128 tracks on an M1) needs; Phase 10 only tunes it. Built in WP-5.37: a pool of real-time workers, started with the stream and joined to its workgroup, runs tracks and a rack's chains at once, with the same output to the bit as one thread; Live has no setting for it (the manual's 38.1), and neither has Beatlayer |
| MIDI track output to external MIDI ports and channels (hardware synths) | M5 | Needs the scheduled MIDI output backend (architecture, MIDI I/O), shared with External Instrument and MIDI Clock. Built in WP-5.34 |
| Surround mixing | Out | Live 12 is stereo apart from one Max device; revisit after 1.0 |

## Automation and modulation

| Live feature | Milestone | Notes |
|---|---|---|
| Arrangement automation lanes, several per track; Automation Arm; Re-Enable Automation; tempo automation | M2 (one lane, Automation Arm, Re-Enable Automation, tempo), M6 (several lanes per track) | Several lanes per track, the lanes' menus and Snap Automation to Grid: WP-6.57 ([spec](specs/automation.md) R24 to R33). Built in WP-2.12, WP-2.20, WP-6.57 |
| Curved segments | M2 | Built in WP-2.12 |
| Session automation recording | M3 | Built in WP-3.12 |
| Insert shapes, stretch and skew, simplify, lock envelopes, keyboard breakpoint editing | M6 | [Spec](specs/automation-editing.md). The selections in lanes, moving the breakpoints, the edit commands on envelopes and Lock Envelopes are built in WP-6.25; the shapes, stretch and skew, and Simplify Envelope in WP-6.25b, the keys in WP-6.26, and the clip envelopes in WP-6.25c |
| Relative modulation on top of automation | M5 | Built in WP-5.9 |
| Modulators: LFO, Shaper, Envelope Follower, Envelope MIDI, Shaper MIDI, Expression Control | M5 | Built in WP-5.10, WP-5.11 |
| Macros (16 per rack), variations, randomise, map mode with per-mapping ranges | M5 | Built in WP-5.4, WP-5.5 |
| MIDI map mode, key map mode, takeover modes, relative controllers, instant mappings | M7 | [Remote mapping](specs/remote-mapping.md), [map modes](specs/map-modes.md), [control surfaces](specs/control-surfaces.md). Built in WP-7.1, WP-7.2, WP-7.2b, WP-7.3, WP-7.3b, WP-7.4, WP-7.6 |
| Computer MIDI keyboard | M2 | Built in WP-2.2 |
| MPE controllers and MPE out: a MIDI port's MPE switch, MIDI To and the External Instrument sending MPE, MPE/Multi-channel Settings | M6 | [Spec](specs/mpe.md). Built in WP-6.17, WP-6.17b |

## Racks and device features

| Live feature | Milestone | Notes |
|---|---|---|
| Instrument, Audio Effect, MIDI Effect, and Drum racks | M5 | Chain strips in the Mixer too. Built in WP-5.1, WP-5.2, WP-5.19 |
| Parallel chains, key, velocity, and chain-select zones with fades, Auto Select | M5 | Built in WP-5.1, WP-5.3 |
| Drum Rack pads, choke groups, return chains, extract chains | M5 | Built in WP-5.6 |
| Device A/B compare | M5 | Built in WP-5.12 |
| Rename devices, device presets | M2 | Device chains in Live's order, the device frame (activator, fold, Show Preset Name, the context menu), Rename, the device clipboard, Beatlayer preset files with Save Preset, Save as Default Preset, and Hot-Swap Presets: WP-2.4 ([spec](specs/device-chains.md)). Presets load into native devices clamped to their parameters: WP-2.5 ([spec](specs/device-framework.md)); each native device's factory presets ship with it (WP-2.16 to 2.18). Built in WP-2.4, WP-2.5 |
| Device parameters and controls: knobs, switches, choosers, number boxes, display cells | M2 | The native device framework: parameter descriptors (gains to −∞ dB, every parameter automatable), smoothing, the generic UI drawn from them with Live's drag, reset, undo names, and value formats, the layout of control bands and stretchable display cells (IMP-14), and an oversampler: WP-2.5 ([spec](specs/device-framework.md)). Built in WP-2.5 |
| "Use Current Scale" on devices | M6 | Arpeggiator, Chord, Pitch, Random and Scale ([spec](specs/scales.md)); later scale-aware devices with their own packages. Built in WP-6.8 |
| Max for Live API (Live Object Model) | M9 | Beatlayer's extension API; not compatible with Max |

## Instruments

| Live device | Milestone | Notes |
|---|---|---|
| Simpler | M2 (Sampler v1), M7 (the region with S Start, S Length and S Loop Length; Slice mode's Playback; Slice to Drum Rack) | [Spec](specs/sampler.md). Built in WP-2.16, WP-7.33, WP-7.33b |
| Drift | M2 | Polysynth v1. Built in WP-2.17 |
| Impulse, Drum Sampler | M5 | Built in WP-5.17, WP-5.18 |
| Sampler (multisample, zones, modulation) | M5 | Plus SFZ import. Built in WP-5.13, WP-5.14 |
| External Instrument | M5 | With hardware latency setting. Built in WP-5.34 |
| Analog | M8 | |
| Operator | M8 | FM |
| Wavetable | M8 | Wavetables generated or open-licensed |
| Meld | M8 | |
| Collision, Tension, Electric | M8 | Physical modelling |
| Drum Synths (Kick, Snare, Clap, Hi-hat, Cymbal, Tom, FM, Clang) | M8 | |
| Granulator III | M8 | |
| Bass, Poli | M8 | |
| CV Instrument, CV Triggers | M7 | Needs a DC-coupled interface; Live's are Max devices, Beatlayer's are native ([spec](specs/cv-tools.md)). Built in WP-7.17 |
| Inspired by Nature (Tree Tone, Emit, Vector FM, Vector Grain) | M9 | Built with the extension SDK as showcase devices |

## Audio effects

| Live device | Milestone | Notes |
|---|---|---|
| EQ Eight, Compressor, Limiter, Reverb, Delay, Utility | M2 | Built in WP-2.18a, WP-2.18b |
| Auto Filter, Auto Pan-Tremolo, Beat Repeat, Channel EQ, Chorus-Ensemble, EQ Three, Erosion, Gate, Grain Delay, Looper, Phaser-Flanger, Redux, Saturator, Tuner | M5 | To Live's current versions: the redesigned Auto Filter (12.2), Saturator's Bass Shaper curve (12.1), and the 12.4 Erosion and Chorus-Ensemble, with its ensemble mode. Built in WP-5.20, WP-5.20b, WP-5.20c, WP-5.21, WP-5.23, WP-5.24, WP-5.25, WP-5.26, WP-5.26b, WP-5.28 |
| Align Delay, Drum Buss, Dynamic Tube, Filter Delay, Glue Compressor, Multiband Dynamics, Overdrive, Shifter, Spectrum, Vinyl Distortion | M5 | Built in WP-5.21, WP-5.22, WP-5.22b, WP-5.23, WP-5.24, WP-5.25, WP-5.27, WP-5.28 |
| External Audio Effect | M5 | With hardware latency setting. Built in WP-5.35 |
| Auto Shift (pitch correction) | M8 | |
| Corpus, Resonators, Vocoder | M8 | |
| Amp, Cabinet, Pedal | M8 | Cabinet impulse responses recorded or open-licensed |
| Echo, Hybrid Reverb, Roar, Spectral Resonator, Spectral Time | M8 | |
| Convolution Reverb, Color Limiter, Gated Delay, Pitch Hack, PitchLoop89, Re-Enveloper, Spectral Blur, Vector Delay | M8 | Live's versions are Max devices; Beatlayer's are native |
| CV tools (Clock In and Out, Envelope Follower, In, LFO, Shaper, Utility) | M7 | Live's are Max devices, Beatlayer's are native ([spec](specs/cv-tools.md)). Built in WP-7.18, WP-7.18b |
| Legacy devices (Auto Filter Legacy, Chorus, Flanger, Phaser, Frequency Shifter, and others) | M9 | Mapped to current equivalents on Live set import |
| Surround Panner | Out | No surround mixing in 1.0 |

## MIDI effects

| Live device | Milestone | Notes |
|---|---|---|
| Arpeggiator, CC Control, Chord, MIDI Monitor, MPE Control, Note Echo, Note Length, Pitch, Random, Rotating Rhythm Generator, Scale, Velocity | M5 (the devices), M6 (the Arpeggiator's Groove) | Rotating Rhythm Generator has no package yet: the manual doesn't describe it. The Arpeggiator's Groove chooser: WP-6.68 ([spec](specs/arpeggiator.md)). Built in WP-5.30, WP-5.31, WP-5.32, WP-6.68 |
| Articulation Map (Beatlayer's own device, not in Live) | M6 | A list of up to 32 named articulations, each with a trigger and the keyswitches, controllers and channel it sends. Built in WP-6.47 ([spec](specs/articulation-map.md)) |
| Melodic Steps, Bouncy Notes, Step Arp, SQ Sequencer, Rhythmic Steps (the Sequencer Pack) | M9 | Built with the extension SDK |
| MIDI Tools Pack (extra rhythmic and melodic generators and transformations) | M9 | Built with the extension SDK as MIDI Tools; list its tools from the pack's product page when the package is planned |
| Performance Pack (Performer, Variations, Arrangement Looper, Prearranger) | M9 | Built with the extension SDK |

## Sync and control

| Live feature | Milestone | Notes |
|---|---|---|
| Tap tempo, Start Transport With Tap Tempo; Phase Nudge Down and Up | M7 | Built in WP-7.31, WP-7.30 |
| Ableton Link: tempo, phase, Start Stop Sync | M7 | Through the open-source Link library, 4.1.1 ([spec](specs/link.md)). Built in WP-7.14, WP-7.15 |
| Link Audio | M7 | In the open-source Link library since Link 4.0 (4 May 2026) ([spec](specs/link-audio.md)). Built in WP-7.16 |
| MIDI Clock in and out with song position; sync delay per port | M7 | [Spec](specs/midi-clock.md). Built in WP-7.11, WP-7.12 |
| MIDI Timecode in | M7 | [Spec](specs/midi-timecode.md). Built in WP-7.13 |
| Control-surface scripts (Live ships 139, in Python) | M7 | Beatlayer's own control-surface host, with native surfaces and declarative controller templates; the first controllers are Novation's Launchpad X, Mini MK3 and Pro MK3 and Launchkey MK3 and MK4, and Akai's APC mini mk2 and APC40 mkII, the rest from the community; scripts as WebAssembly components come with the extension SDK in M9 ([control surfaces](specs/control-surfaces.md), [supported controllers](specs/supported-controllers.md)). Built in WP-7.5, WP-7.6, WP-7.7, WP-7.8, WP-7.9, WP-7.10 |
| Push 2 as a controller | M7 | Through Ableton's public Push 2 interface documentation ([Push 2](specs/push-2.md), [notes on Push 2](specs/push-2-notes.md), [tracks on Push 2](specs/push-2-tracks.md)). Built in WP-7.19, WP-7.20, WP-7.21, WP-7.22, WP-7.23, WP-7.24, WP-7.25, WP-7.26, WP-7.27, WP-7.28 |
| Push 3 as a controller | M10 | If its protocol is documented by then; otherwise out |
| Push 3 standalone, Move, Note, Ableton Cloud sync | Out | Closed platforms and services |

## Plug-ins

| Live feature | Milestone | Notes |
|---|---|---|
| VST3 | M2 | Built in WP-2.7 |
| Audio Units v2 and v3 | M2 | macOS. Built in WP-2.8 |
| VST2 | Out | Steinberg no longer licenses the VST2 SDK |
| Plug-in configure mode, MPE plug-ins, multi-output plug-ins | M5 (configure mode, multi-out), M6 (MPE) | WP-2.6 hosts a CLAP plug-in's main audio ports and one note port, with per-note IDs. MPE plug-ins (each plug-in's MPE Mode and MPE/Multi-channel Settings, CLAP note expressions, VST3 note expression, MPE MIDI to Audio Units): WP-6.17b ([spec](specs/mpe.md)). Multi-output plug-ins: WP-5.45. Configure mode: built in WP-5.40. Built in WP-5.40, WP-5.45, WP-6.17b |
| (Beatlayer only) CLAP | M2 | Built in WP-2.6 ([spec](specs/clap-hosting.md)); embedded editors with WP-2.10, offline renders with WP-2.6b |
| (Beatlayer only) LV2 | M5 | Linux. Built in WP-5.39 |

## Files and export

| Live feature | Milestone | Notes |
|---|---|---|
| Sets, templates, Collect All and Save, File Manager (missing files) | M1 (project files), M2 (templates, collect, relink) | The commands: WP-2.25; the File Manager's views: WP-6.63. Built in WP-0.7, WP-1.7, WP-1.11, WP-2.25, WP-6.63 |
| Crash recovery: the next launch offers the unsaved set | M1 | [Spec](specs/crash-recovery.md). The library: WP-1.11; the app's journal and recovery prompt: WP-6.62. Built in WP-1.11, WP-6.62 |
| (Beatlayer only) Save Continuously, with a history of save points (IMP-05) | M1 (the journal and the option), M6 (the history browser) | Off by default. The journal and the option in the library: WP-1.11; in the app: WP-6.62; the history browser: WP-6.39. Built in WP-1.11, WP-6.62, WP-6.39 |
| Clip, device preset, and rack preset files | M2 (device presets), M5 (racks), M6 (clips) | Beatlayer's own formats. Clips: [spec](specs/clip-files.md); Collect Files on Export: [file management](specs/file-management.md) R7 to R9; a plug-in device's preset carrying its state, and a plug-in's presets under its row in the browser: WP-6.66. Built in WP-2.5, WP-5.8, WP-6.34, WP-6.34b, WP-6.66 |
| Live set import (`.als`) | M9 | |
| Live device and rack preset import (`.adv`, `.adg`) | M9 | Native devices mapped to Beatlayer equivalents |
| Packs (`.alp`), Max devices (`.amxd`), Move and Note bundles (`.ablbundle`) | Out | Ableton content and closed platforms |
| Pack a project into one archive, export Session clips as a new set, merge sets | M6 | Beatlayer's own pack format ([spec](specs/set-exchange.md)). Opening a set in the browser, merging its tracks, device chains and clips, and saving tracks or Session clips as a new set: WP-6.35; packs: WP-6.35b. Built in WP-6.35 and WP-6.35b |
| Standard MIDI Files, import and export | M2 | Built in WP-2.21 |
| Audio import: WAV, AIFF, AIFF-C, FLAC, Ogg Vorbis, MP3, M4A (AAC, ALAC), RF64 and BWF over 4 GB | M1 | M4A on every platform, not only macOS. Built in WP-1.1, WP-1.10 |
| Export: Main, all tracks as stems, selected tracks, one track; include returns and Main effects; mono; normalise | M1 (Main), M2 (one track), M4 (stems, render as loop), M6 (the dialog, mono) | The Export Audio/Video dialog, Convert to Mono and Create Analysis File: WP-6.71 ([spec](specs/export.md) R28 to R45). Built in WP-1.12, WP-2.26, WP-4.17, WP-6.71 |
| Export formats: WAV, AIFF, FLAC at 16, 24, or 32 bits; MP3 at 320 kbps | M1 (WAV, AIFF, FLAC), M2 (MP3) | Built in WP-1.12, WP-2.26 |
| Dither: rectangular, triangular | M1 | Built in WP-1.12 |
| Dither: POW-r 1, 2, 3 | Out | A licensed proprietary algorithm; Beatlayer's own noise-shaped dither takes its place, built in WP-2.27 |
| Real-time rendering with external hardware, restart after dropouts | M5 | Built in WP-5.35 |
| Video in the arrangement, video window, video export | M9 | |
| (Beatlayer only) DAWproject import and export | M6 | [Spec](specs/dawproject.md). Built in WP-6.36, WP-6.36b, WP-6.37, WP-6.37b |
| (Beatlayer only) Agent control of every feature through MCP, headless or alongside the app | M1, then every milestone | D17; each feature's commands reach the agent API when the feature ships. Built in WP-0.14, WP-1.15, WP-1.16 |
| (Beatlayer only) Built-in assistant on any cloud or local model | M2 | Built in WP-2.23 |

## Other

| Live feature | Milestone | Notes |
|---|---|---|
| Tempo automation and time-signature changes | M2 (the commands, tempo automation), M6 (the time-signature lane, fragmentary bars) | The time-signature lane and fragmentary bars: WP-6.61 ([spec](specs/time-editing.md) R29 to R36). Built in WP-2.20, WP-6.61 |
| Locators, Copy Time, insert and delete time | M6 | [Spec](specs/time-editing.md); inside a MIDI clip, [the note editor](specs/piano-roll.md). Built in WP-6.24, WP-6.24b, WP-6.54 |
| Ableton's sound library (samples, presets, racks, grooves, packs) | Out | Ableton's copyright; Beatlayer ships its own open-licensed library in M9 |
| (Beatlayer's own) a library covering the same ground as Live's Packs, tagged on arrival | M9 | Recorded or open-licensed, never Ableton's: presets for every native instrument and effect; drum kits by genre (hip-hop, trap, club, machine); acoustic drums, dry and in a room; world percussion; grand and upright pianos; electric pianos and organs; guitars and basses; orchestral strings, brass, woodwinds, and mallets, and string and brass quartets; vocal instruments and vocal effect racks; foley and found sounds; drones and textures; loops and MIDI clips. Every item carries browser tags, as Live's factory content does |

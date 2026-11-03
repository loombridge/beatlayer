# Granulator III: grains from a sample in Classic, Loop and Cloud modes, MPE, and capturing audio in real time

- **Status:** draft
- **Work packages:** WP-8.26 (Granulator III, part 1: the grain engine in its three modes, its envelope, filter and volume, MPE and the waveform display: R1 to R17), WP-8.27 (Granulator III, part 2: capturing audio in real time, the captured audio kept as a file, and the panel: R18 to R27). Voices and per-note bends are [the instruments' voice section](instrument-voices.md); the filter is [the instruments' filter](instrument-filter.md); loading and referencing a sample is the [sampler](sampler.md)'s and [the multisampler](multisampler.md)'s; choosing a source is a sidechain's ([routing](routing.md) R22 to R31); where a capture's file goes is [recording](recording.md) R3's
- **Parity-matrix row:** Instruments: Granulator III
- **Checked against:** Ableton's Granulator III pack page (ableton.com/en/packs/granulator-iii/), read 30 October 2026; the device isn't described in the Live 12 reference manual, read 30 October 2026, or the Live 12 release notes

## The behaviour

Granulator III is a granular synthesiser. It plays a sample as a stream of short grains, each a small window of the sample read at the note's pitch and faded in and out, and builds pads, textures and rhythms from them. It has three modes: Classic, a steady stream of grains as the earlier Granulator II played them; Loop, for rhythmic material, its grains falling on the song's beat; and Cloud, for drones and textures, its grains scattered in time, position and pitch. A note plays its own stream, and with an MPE controller each note's bend moves its grains' pitch while slide and pressure move their position, size and shape. Besides a sample loaded the usual way, it can capture audio as it plays, from a source chosen as a sidechain's is, and play grains from it while it fills.

Live's Granulator III is a Max for Live device; Beatlayer's is native, with Live's name. Its product page names its modes, capture and MPE, but not its controls, so every control, range and law below is Beatlayer's own.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Product page | Ableton, "Granulator III" pack page (ableton.com/en/packs/granulator-iii/), read 30 October 2026 | A granular synthesiser for pads, textures and sound design from short, looped and modulated pieces of sound; three modes, Classic (as Granulator II), Loop (for rhythmic material) and Cloud (for drones and textures); audio captured in real time as well as samples; an MPE controller shaping each note's pitch bend, grain position, size and shape |
| S2 | Beatlayer | [The sampler](sampler.md), [the multisampler](multisampler.md) and the `device.load_sample` command; [file management](file-management.md) | Loading a sample by command or by dropping it, referencing it from the set, reporting it missing, collecting it |
| S3 | Beatlayer | [MPE](mpe.md) R5 to R7; [the instruments' voice section](instrument-voices.md) R14 and R15 | Per-note bend, pressure and slide reaching a device |
| S4 | Beatlayer | [Routing](routing.md) R22 to R31; [delay compensation](delay-compensation.md); [recording](recording.md) R3 | A device's sidechain source and its alignment; where recorded files go and how they are named |
| S5 | Beatlayer | [The device framework](device-framework.md) R29 and R35 to R45; [the architecture](../02-architecture.md), "Real-time rules" | Seeded randomness, display data and pages; no allocation, lock or blocking call on the audio thread |

## Rules

**The grain engine (WP-8.26)**

- **R1.** Beatlayer ships Granulator III (`beatlayer.granulator_iii`), an instrument listed under the browser's Instruments label, with factory presets of Beatlayer's own that use generated material. (S1)
- **R2.** A sample loaded by `device.load_sample` or dropped on its display is the grains' source, referenced by the set, reported missing and collected as a sampler's sample is; without a sample the device is silent. (S2)
- **R3.** Mode is Classic, Loop or Cloud (Classic by default). (S1)
- **R4.** Each voice has a pool of 64 grains made with the device; a grain that would start with the pool full is dropped and counted, and `device.display` reports the count. A grain is a window of the source starting at its position, read with linear interpolation at its pitch ratio for Size, and shaped by the grain window; it ends early where the source does. `device.display` reports the dropped count as the reading Dropped Grains. (S5; the pool's size is Beatlayer's own)
- **R5.** Position (0 to 100 % of the source, 0 % by default) is where grains start; Size (1 ms to 1 s, 100 ms by default) is each grain's length, 1 ms to 1 s on a logarithmic scale; Shape (0 to 100 %, 50 % by default) is its window, from a flat window with 1 ms fades at 0 % through a Hann window at 50 % to a narrow Gaussian (σ 12 % of the grain) at 100 %, blending between those; Spray (0 to 100 %) scatters each grain's start by up to ±Spray × 1 s around Position; Scan (−100 % to 100 %, 0 % by default) moves Position forward or backward through the source while a note is held, at up to the source's own speed, wrapping at the source's ends. A start past the source's ends is held at them. (S1; Beatlayer's own)
- **R6.** A grain's pitch is the note's key minus 60 plus Transpose (−48 to +48 semitones) and Fine (−50 to +50 cents) and, for each grain, a random offset within ±Variation (0 to 12 semitones) from the device's seed; a sample plays at its own pitch at C3. (S1; Beatlayer's own)
- **R7.** Classic: grains start at regular intervals of Size / Density, Density (1 to 16, 4 by default) being how many overlap, from the note's frame. Each grain's level is 2 / Density, up to 1, so the stream's level doesn't climb with Density. (S1; Beatlayer's own reading)
- **R8.** Loop: grains start on a grid of Rate (1/64 to 1 bar, 1/16 by default) of the song's beats, each grain reading Size from Position, so the source's piece repeats in time with the song; the grid's frames come from the song position, so grains fall on the same frames at any block size and in an offline render. With the transport stopped, the grid runs from the note's frame at the set's tempo. A bar is four beats, whatever the time signature. (S1; Beatlayer's own reading)
- **R9.** Cloud: grains start at random times, on average Density × 10 per second, each at Position scattered by Spray, pitched within Variation and panned within Width (0 to 100 %), all from a generator seeded per device and voice. The gaps between grains are exponentially distributed, and each grain's level is scaled by the square root of the average overlap (Size times the rate, up to 1) so the stream's level stays steady. (S1; Beatlayer's own reading)
- **R10.** Every random value (start times, positions, pitches, pans) comes from a generator seeded per device and voice, restarted with each note, so a render repeats run after run, and every grain's onset is a frame worked out from its note's frame or the song position, never from a block's start. (S5)
- **R11.** Each voice's grains pass through the voice's amplitude envelope (Attack, Decay and Release 0 ms to 20 s, Sustain 0 to 100 %), then the instruments' filter (low-pass, high-pass or band-pass at 12 or 24 dB, Clean or MS2; off in a new device), and Volume (−∞ to 0 dB) sets the output. The filter's controls are Filter (on or off), Type, Slope, Circuit (Clean or MS2), Freq (20 Hz to 20 kHz) and Res (0 to 100 %). (S5; Beatlayer's own)
- **R12.** Voices (1 to 16, 8 by default) and Priority follow the voice section (instrument voices R3, R4), Transpose (instrument voices R17) is its own, and PB Range (instrument voices R14) moves the pitch wheel; each voice has its own grains. (S3)
- **R13.** Under MPE, a note's per-note bend moves its grains' pitch (instrument voices R14), Slide moves its Position from where it sits at Slide's centre, 0.5, by up to ±50 % of the source at Slide Amount (−100 % to 100 %, 50 % by default), and Pressure moves its Size along its logarithmic travel and its Shape along its own by Pressure > Size and Pressure > Shape (−100 % to 100 %, 0 % by default, each up to its whole travel); each is read when a grain starts, and each moves that note's grains alone. (S1, S3; the routings are Beatlayer's own)
- **R14.** The display draws the source's waveform, Position and the Spray range, and a mark for every sounding grain at its position, from the device's display data, which `device.display` reports headless: the waveform's peaks at a fixed resolution (128 peaks, the series Waveform) and the positions of up to 64 grains (a marker Grain n each), with markers Position and, once the source's length is known, Spray Start and Spray End, and the readings Source Length and Dropped Grains. The peaks are taken on the audio thread in small steps, so a long source's waveform fills in over a few blocks. (S5)
- **R15.** Every control is a parameter, automatable and modulatable (rule G2), and the device's banks hold every one. (S5)
- **R16.** The output is the same at any block size and run after run; nothing on the audio thread allocates, locks or blocks. (S5)
- **R17.** The body is in pages (device framework R41): Grain, Envelope and Filter, and MPE, with the display beside them and always shown. (S5)

**Capture and the panel (WP-8.27)**

- **R18.** The device has a capture source, chosen as a sidechain's Source is ([routing](routing.md) R23): No Input (the default) or a track, tapped Post FX, and a capture Length (1 to 30 s, 10 s by default). The capture buffer is made with the device at 30 s of stereo audio at the engine's rate. (S1, S4; Beatlayer's own)
- **R19.** `granulator.capture` (`edit`) starts or stops a capture by the device's ID (`action`: `start` or `stop`); the panel's Capture button runs it. A capture writes the source's audio into the buffer from the frame it starts, sample for sample, aligned for latency as a sidechain is, and stops by itself after Length. It's refused with No Input chosen. (S1, S4; the command's name is Beatlayer's own)
- **R20.** While a capture fills, the device plays grains from it: Position and Spray are shares of what has been captured so far, so grains read only audio already written. (S1; Beatlayer's own reading)
- **R21.** When a capture ends, the audio captured is the device's source, as one undo step, Capture Audio; a worker thread writes it to a WAV file in the project's `Samples/Recorded/` folder, named `<track label> Granulator III Capture [YYYY-MM-DD HHMMSS].wav` as recording R3 names files, and the set references it as a loaded sample, so a reopened set plays it identically. Undo brings back the previous source; the file stays. (S4)
- **R22.** The buffer is handed to the worker through a queue; nothing the capture does on the audio thread allocates, locks, blocks or waits on the file. (S5)
- **R23.** The query tool's `device` result reports the capture under `capture`: the source, Length, whether it is capturing, the frames captured and the file of the last capture. (S5)
- **R24.** The source and Length are device settings, saved with the set; changing them is one undo step each, Change Capture Source and Change Capture Length. (S4; Beatlayer's own)
- **R25.** The panel holds the Capture button, the source chooser and Length beside the display, and its controls and display lie within the body at 50 %, 100 % and 200 % zoom with nothing overlapping. (S5)
- **R26.** An offline render captures as playback does: the same frames, the same audio. (S4, S5)
- **R27.** The panel is Beatlayer's own look. (S5)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R15 | `crates/devices/tests/banks.rs`; `crates/devices/tests/rules.rs` | unit |
| R2 | `crates/devices/tests/granulator_iii.rs`: `without_a_sample_it_is_silent_and_with_one_a_note_plays_it_at_its_own_pitch_at_c3`; `crates/cli/tests/agent.rs`: `an_agent_loads_a_sample_into_granulator_iii_reads_its_display_and_finds_it_after_a_reopen` | agent API (`device.load_sample`); save and reopen |
| R3 to R10 | `granulator_iii.rs`: `classic_starts_a_grain_every_size_over_density_from_the_notes_frame`, `loop_starts_grains_on_the_songs_grid_at_any_block_size`, `cloud_scatters_grains_from_a_seed_that_repeats_with_each_note_and_every_run`, `size_shape_and_density_change_the_output_as_specified`, `a_full_pool_drops_grains_and_counts_them_and_the_display_reports_the_count` | unit (grain onsets on frames per mode; pool overflow counted); renders at block sizes 1, 64, 512 and 4,096 |
| R11, R12 | `granulator_iii.rs`: `the_envelope_filter_and_volume_shape_the_output` | render |
| R13 | `granulator_iii.rs`: `mpe_slide_pressure_and_per_note_pitch_move_one_notes_grains_alone` | render (MPE moves one note's grains alone) |
| R14, R17 | `granulator_iii.rs`: `the_display_reads_the_waveform_position_spray_and_each_grain_headless`, `it_is_an_instrument_named_for_live_with_the_display_and_three_pages`; `crates/cli/tests/agent.rs` (as R2) | agent API (`device.display`); layout test |
| R16 | the block-size tests above; every block of `granulator_iii.rs` runs under the allocation trap; `rtsan/tests/granulator_iii.rs`: `granulator_iii_passes_under_rtsan` | real-time trap; repeated renders identical |
| R18, R19, R20 | to be written in WP-8.27 | engine test (a generated source captured sample for sample, aligned for latency) |
| R21, R24 | to be written in WP-8.27 | shell test (one undo step; the file in the project folder; reopened identically) |
| R22 | to be written in WP-8.27 | RealtimeSanitizer (`rtsan/tests`) |
| R23 | to be written in WP-8.27 | agent API; `cargo xtask commands` |
| R25, R27 | to be written in WP-8.27 | layout test at three zooms; review |
| R26 | to be written in WP-8.27 | offline render test |

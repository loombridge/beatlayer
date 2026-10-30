# Live device mapping: Live's devices as Beatlayer's on import

- **Status:** draft
- **Work packages:** WP-9.19 (the mapping registry, the value laws, and the tables for the devices, modulators and MIDI effects Phases 2 to 7 built: R1 to R14); WP-9.20 (legacy devices to their current equivalents: R15 to R18); WP-9.23 (the sound-design devices Phase 8 builds, as each is done: R19 to R21). Where a mapped device sits in an imported set, and the placeholder for an unmapped one, are [Live set import](live-set-import.md) R43 to R50; each device's own parameters and ranges are its own spec's
- **Parity-matrix row:** Files and export: Live set import (`.als`); Files and export: Live device and rack preset import (`.adv`, `.adg`); Audio effects: Legacy devices (Auto Filter Legacy, Chorus, Flanger, Phaser, Frequency Shifter, and others)
- **Checked against:** the Live 12 reference manual (online edition), chapters 29 to 31, read 30 October 2026; Live's release notes 10.0 to 12.4

## The behaviour

Beatlayer's devices take Live's names, so a Live device maps to the Beatlayer device of the same name. Each mapping is a table: for every parameter Live stores, the Beatlayer parameter it sets, the law that turns Live's stored value into Beatlayer's, and its unit; parameters with no equivalent are listed in the import report. Live's legacy devices, which older sets still hold, map to the current device that replaced them. Devices Live builds in Max (the modulators, the CV tools and some MIDI effects) map by name to Beatlayer's native devices; their Max data is never loaded.

The mapping is data, kept in a registry in `bl-interop`, so a package adds a device's table without touching the importer. A device that has no table yet becomes the placeholder [Live set import](live-set-import.md) R48 describes.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29 "Live Audio Effect Reference", chapter 30 "Live MIDI Effect Reference" and chapter 31 "Live Instrument Reference" | Each device's controls, their ranges and units |
| S2 | Manual | Live's release notes, 10.0 to 12.4 | Which devices were replaced and kept as legacy devices: Simple Delay and Ping Pong Delay by Delay (10.0); Chorus, Flanger and Phaser by Chorus-Ensemble and Phaser-Flanger (11.0); Frequency Shifter by Shifter (11.1); Auto Filter by its redesign, the old one as Auto Filter Legacy (12.2); Auto Pan by Auto Pan-Tremolo, the old one as Auto Pan Legacy (12.3); Erosion by its redesign, the old one as Erosion Legacy (12.4) |
| S3 | Manual | Earlier editions of Live's reference manual (Live 9, 10 and 11), the legacy devices' sections | The legacy devices' controls and ranges |
| S4 | Measurement | Devices and presets saved by Live 12.4.3 on macOS, each with its parameters set to known values, read as gzipped XML output | Each parameter's stored name and the form of its value (a number in the control's unit, a normalised value, or an index), recorded in each table |
| S5 | Spec | Every built device's spec in this folder (its parameters' ranges and units); [device framework](device-framework.md) (parameter descriptors) | Beatlayer's side of each table |
| S6 | Beatlayer | CONTRIBUTING.md, "Building without Ableton's code"; the architecture's "Project files and interop" | Live's own content (its wavetables, samples, presets and Max devices) is never imported |

## Rules

### The registry and the laws (WP-9.19)

- **R1.** The registry maps each of Live's device names to a Beatlayer device type and a table. A table row gives Live's stored parameter name, Beatlayer's parameter, a law, and Live's unit. A device found in a set with no table becomes the placeholder ([Live set import](live-set-import.md) R48). (S4, S5)
- **R2.** The laws are: **linear** (a value in the same unit, scaled by a factor); **decibels** (Live's stored linear gain to decibels, −∞ dB at zero); **frequency** (a value in hertz, or a normalised value to hertz on the control's logarithmic scale between its ends); **time** (seconds or milliseconds, or a synced division by name); **percentage** (a fraction to a percentage); **switch** (on and off); **choice** (an index or a name to Beatlayer's choice of the same name). (S4, S5)
- **R3.** A choice whose name Beatlayer's parameter doesn't have takes the nearest choice the table names for it, and is listed. (S4; Beatlayer's own)
- **R4.** A value outside Beatlayer's range is clamped to the nearest end and listed. (Beatlayer's own)
- **R5.** A parameter with no equivalent is listed by its name and value; the device's other parameters still map. (Beatlayer's own)
- **R6.** Mapped parameters keep their automation, clip envelopes, macro mappings (with each mapping's minimum and maximum converted by the parameter's law) and modulation where Beatlayer has the modulator. (S4)
- **R7.** Simpler, Sampler, Drum Sampler and Impulse keep their samples, through the media store, with their zones, slices, regions and warp settings. A sample from Live's own libraries is kept by name and listed ([Live set import](live-set-import.md) R14). (S4, S6)
- **R8.** The tables for the devices Phases 2 to 7 built are, by Live's name and Beatlayer's device of the same name: the instruments Simpler, Sampler, Drift, Impulse, Drum Sampler and External Instrument; the audio effects EQ Eight, Compressor, Limiter, Reverb, Delay, Utility, Auto Filter, Auto Pan-Tremolo, Beat Repeat, Channel EQ, Chorus-Ensemble, EQ Three, Erosion, Gate, Grain Delay, Looper, Phaser-Flanger, Redux, Saturator, Tuner, Align Delay, Drum Buss, Dynamic Tube, Filter Delay, Glue Compressor, Multiband Dynamics, Overdrive, Shifter, Spectrum, Vinyl Distortion and External Audio Effect; and the MIDI effects Arpeggiator, Chord, Note Echo, Note Length, Pitch, Random, Scale and Velocity. (S1, S4, S5)
- **R9.** Live's devices built in Max map by their name to Beatlayer's native devices of the same name, from the values Live stores for their parameters: the modulators LFO, Shaper, Envelope Follower, Envelope MIDI, Shaper MIDI and Expression Control; the CV tools CV Instrument, CV Triggers, CV Clock In, CV Clock Out, CV In, CV Utility, CV Envelope Follower, CV LFO and CV Shaper; and the MIDI effects CC Control, MPE Control, MIDI Monitor and Rotating Rhythm Generator. A modulator's mappings to other devices' parameters are rebound through the table of each target. Their Max data is never loaded. (S4, S6)
- **R10.** Each table's laws are tested row by row: a generated preset with a parameter at known values maps to the values the law gives. (S5)
- **R11.** A set holding one mapped effect renders as the same device set by hand in Beatlayer, within its golden tolerance. (S5)
- **R12.** An instrument's preset data with no parameter (Simpler's warp markers, Sampler's zone map, a Drum Sampler's sample) comes across by the device's own rule in its table. (S4)
- **R13.** A device's activator, name and fold state are the importer's ([Live set import](live-set-import.md) R49), not the table's. (S4)
- **R14.** The registry lists, for `set.import_report`, every mapped device by name, so a person can see what an import mapped. (Beatlayer's own)

### Legacy devices (WP-9.20)

- **R15.** Live's legacy devices map to their current equivalents: Auto Filter Legacy to Auto Filter; Auto Pan Legacy to Auto Pan-Tremolo in its Panning mode; Chorus to Chorus-Ensemble in its Chorus mode; Flanger to Phaser-Flanger in its Flanger mode; Phaser to Phaser-Flanger in its Phaser mode; Frequency Shifter to Shifter in its Freq mode, its Ring setting to Shifter's Ring; Erosion Legacy to Erosion; Simple Delay to Delay; Ping Pong Delay to Delay with Ping Pong on. (S2, S3)
- **R16.** Each legacy device's table converts its parameters to the current device's by the laws in R2, from the legacy device's ranges in the earlier manuals and its stored values; a setting the current device can't make (a filter type it doesn't have, a range it doesn't reach) takes the nearest value and is listed. (S3, S4)
- **R17.** Each device spec that names its legacy device points to this spec for it: [Auto Filter](auto-filter.md), [Auto Pan-Tremolo](auto-pan-tremolo.md), [Pitch and frequency shifter](pitch-and-frequency-shifter.md) and [digital degradation](digital-degradation.md). (S2)
- **R18.** Table-driven tests cover every legacy law. (S5)

### Phase 8's devices (WP-9.23)

- **R19.** Each Phase 8 device Beatlayer has when the package runs gets its table, by Live's name: Wavetable, Operator, Analog, Meld, Collision, Tension, Electric, Bass, Poli, the Drum Synths (DS Kick, DS Snare, DS Tom, DS Clap, DS HH, DS Cymbal, DS FM, DS Clang), Granulator III, Hybrid Reverb, Convolution Reverb, Echo, Roar, Amp, Cabinet, Pedal, Resonators, Corpus, Vocoder, Auto Shift, Spectral Time, Spectral Resonator, Spectral Blur, Color Limiter, Re-Enveloper, Gated Delay, Pitch Hack, PitchLoop89 and Vector Delay. A device not yet built stays a placeholder and is listed. (S1, S4)
- **R20.** Live's own wavetables, impulse responses and samples are content and are never imported: a device using one keeps its name and gets Beatlayer's nearest default, and it is listed; a wavetable, impulse response or sample of the person's own comes across through the media store. (S6)
- **R21.** Each of these tables is tested row by row, and a set holding the device renders as the device set by hand in Beatlayer. (S5)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 to R14 | to be written in WP-9.19: `crates/interop/tests/live_devices.rs`, a table-driven test per device; single-device renders against the hand-built sets | interop test, golden render |
| R15 to R18 | to be written in WP-9.20 | interop test |
| R19 to R21 | to be written in WP-9.23 | interop test, golden render |

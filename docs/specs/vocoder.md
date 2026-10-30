# Vocoder: a filter-bank vocoder with four carriers

- **Status:** draft
- **Work packages:** WP-8.45 (all of it: R1 to R20). The audio sidechain that brings the External carrier is [routing](routing.md) R22 to R31; the pitch tracker behind the Pitch Tracking carrier is Beatlayer's own, `bl_dsp::pitch::Tracker` (WP-8.32, [the architecture](../02-architecture.md)); the framework is WP-2.5's ([the native device framework](device-framework.md)), its display path WP-5.43's
- **Parity-matrix row:** Audio effects: Corpus, Resonators, Vocoder
- **Checked against:** the Live 12 reference manual (online edition), chapter 29 "Live Audio Effect Reference", section 29.42 "Vocoder" with 29.42.1, read 30 October 2026

## The behaviour

A vocoder puts the spectral shape of one sound, the modulator (usually a voice, the device's input), onto another, the carrier. Both pass through banks of band-pass filters; the level of each modulator band, followed over time, sets the level of the matching carrier band, so the carrier speaks with the modulator's formants. The carrier can be noise, a track brought in through the sidechain, the modulator itself, or an oscillator that follows the modulator's pitch. Unvoiced sounds (the hiss of an s or an f) can be added from noise, so consonants stay clear; Formant moves the carrier's bands up or down to change the voice's size; Depth, Attack and Release shape how the bands follow.

The device takes Live's name with Beatlayer's own type ID, `beatlayer.vocoder`.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.42 "Vocoder" | The carriers Noise (with an X–Y control: downsampling across, density up), External (the sidechain), Modulator (the input itself) and Pitch Tracking (an oscillator following the modulator's pitch, with High and Low limits, a sawtooth or three pulse waveforms, and Pitch for coarse tuning); Enhance; Unvoiced, with Sens. (0 to 100 %) and a Fast/Slow switch; the filter bank's Bands chooser, Range, BW (100 % the most accurate), Precise or Retro, Gate and Level; Depth (0 %, 100 % classic, 200 % only the peaks), Attack and Release; Formant; the channel modes Mono, Stereo (a mono modulator, a stereo carrier) and L/R (both stereo); Dry/Wet; the display shows the bands' levels |
| S2 | Manual | Section 29.42.1 "Vocoder Tips" | A singing synthesiser (a synth as the External carrier); Modulator as the carrier for a formant shifter |
| S3 | Beatlayer | [Routing](routing.md) R22 to R31; `bl_dsp::pitch::Tracker`; [the device framework](device-framework.md) R28, R29, R35 to R38; [control surfaces](control-surfaces.md) R19; [the improvements](../05-improvements.md) G1 and G2 | Sidechains; the voice pitch tracker; the follower, seeds, display data; banks; gains and automation |

## Rules

### The filter banks

- **R1.** Vocoder (`beatlayer.vocoder`) is an audio effect with an audio sidechain (routing R22), Live's name, Beatlayer's own layout and one display cell, the band display (R13). (S1)
- **R2.** Bands is 4, 8, 12, 16, 20, 24, 28, 32, 36 or 40 (20 in a new device). The analysis bank (on the modulator) and the synthesis bank (on the carrier) each have that many fourth-order band-pass filters whose centres are spaced evenly in log frequency between Range's low and high ends (Range's two handles, 20 Hz to 20 kHz, 80 Hz and 12 kHz by default). Each band's measured centre is within 1 % and its −3 dB bandwidth within 5 % of its design, at every band count and Range. (S1; the counts' steps, the order and the defaults are Beatlayer's own)
- **R3.** BW, 10 to 200 % (100 % by default), scales every band's bandwidth against the spacing between centres: at 100 % adjacent bands meet at −3 dB, so the bank's summed response is flat within 1 dB across Range. (S1; the law is Beatlayer's own)
- **R4.** Precise runs the banks as above; Retro runs them as second-order band-passes with a gentler skirt and a 2 dB boost at each band's centre, as older vocoders' filters sound. (S1; Retro's design is Beatlayer's own)
- **R5.** Formant, −12 to +12 semitones (0 by default), moves the synthesis bank's centres by that interval against the analysis bank's, so a generated vowel's formant peaks on a saw carrier move by the interval within 5 %. (S1; the range is Beatlayer's own)

### The envelopes

- **R6.** Each analysis band's level is followed by an envelope follower (device framework R28) with Attack (1 to 300 ms, 5 ms by default) and Release (5 to 3,000 ms, 100 ms), each the time to cover 63 % of a step within 10 %; the level sets the matching synthesis band's gain. (S1; ranges are Beatlayer's own)
- **R7.** Depth, 0 to 200 % (100 % by default), raises each band's level to a power: at 0 % every band's gain is the bands' mean (no imprint), at 100 % in proportion to its level, at 200 % to its level squared, so only the peaks pass. (S1; the law is Beatlayer's own)
- **R8.** Gate, −80 to 0 dBFS (−80 by default), silences a synthesis band whose analysis level is below it. Level, a gain from −∞ to +24 dB (0 dB, rule G1), sets the output's level. (S1; ranges are Beatlayer's own)
- **R9.** With Enhance on, each synthesis band's level is normalised against its neighbours' over the last 50 ms, so quiet passages are as bright as loud ones. (S1; the law is Beatlayer's own)

### The carriers

- **R10.** Noise plays white noise from the device's seed through a sample-and-hold whose rate the X–Y control's horizontal axis lowers (from the sample rate to 1 kHz) and whose density its vertical axis lowers (from every sample to one impulse in 64, randomly placed). External plays the sidechain's signal (routing R24's mix); with no source chosen, it plays silence. Modulator plays the input itself, so the bank works as a formant shifter with Formant. (S1, S2; the X–Y laws are Beatlayer's own)
- **R11.** Pitch Tracking plays an oscillator following the input's pitch from `bl_dsp::pitch::Tracker` limited to High and Low (Low 40 Hz to High's value, 50 Hz by default; High up to 2 kHz, 1 kHz), moved by Pitch (−24 to +24 semitones, 0), within 5 cents of a synthetic voice's pitch from 80 Hz to 1 kHz; its waveform is Saw (by default) or Pulse 1, 2 or 3 (duty cycles 50, 25 and 12.5 %), band-limited. While the tracker calls a frame unvoiced, the oscillator keeps its last pitch. (S1, S3; the ranges, duties and band-limiting are Beatlayer's own)
- **R12.** Unvoiced, 0 to 100 % (0 by default), adds noise to the synthesis bank's upper half while the modulator is unvoiced, a frame being unvoiced when its zero-crossing rate is above Sens.'s threshold (0 % never, 100 % at the lowest rate) and its level above Gate; Fast follows it frame by frame (5 ms), Slow over 30 ms. On a generated fricative (white noise above 3 kHz) the output gains noise in the upper bands; on a vowel it gains none. (S1; the detector is Beatlayer's own)

### The display and channels

- **R13.** The display draws each band's synthesis level as a bar between Range's two handles, which drag to set Range's ends as one undo step each; Gate is drawn as a line. Its data (each band's level and centre, Range, Gate) is published after each block and read by `device.display` (device framework R35 to R37). (S1, S3; the look is Beatlayer's own)
- **R14.** Mono runs one analysis bank on the sum of the input's channels and one synthesis bank on the carrier's sum, its output on both channels; Stereo runs one analysis bank on the input's sum and a synthesis bank for each of the carrier's channels; L/R runs a pair of banks for each channel. Dry/Wet, 0 to 100 % (100 %), balances the input against the result. (S1)

### Throughout

- **R15.** The device's latency is the tracker's in Pitch Tracking and none otherwise; it reports the tracker's latency in every carrier mode, so its latency never changes with the carrier, and delays its dry path to match. (S3; Beatlayer's own)
- **R16.** Nothing on the audio thread allocates, locks or blocks; the banks are made with the node for 40 bands in L/R. (S3)
- **R17.** A render is bit-identical at block sizes 1, 64, 512 and 4,096 and run after run, Noise and Unvoiced included. (S3)
- **R18.** Every parameter is automatable and modulatable (rule G2) and set and read back through the agent API; every gain reaches −∞ dB. (S3)
- **R19.** Vocoder declares its banks of eight ([control surfaces](control-surfaces.md) R19): Main (Carrier, Bands, Formant, Depth, Attack, Release, Unvoiced, Dry/Wet), Filters (Range's ends, BW, Precise/Retro, Gate, Level, Enhance, Channel Mode) and Carrier (the noise X and Y, Low, High, Pitch, Waveform, Sens., Fast/Slow), so every automatable parameter is in a bank. (S3; Beatlayer's own)
- **R20.** A set with Vocoder saves and reopens with every parameter and its sidechain. (S3)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 to R5 | to be written in WP-8.45 | unit (each band's centre and bandwidth at every count and Range; the summed response; Retro; Formant on a generated vowel within 5 %) |
| R6 to R9 | to be written in WP-8.45 | unit (Attack and Release within 10 %; Depth's ends; Gate; Enhance) |
| R10 to R12 | to be written in WP-8.45 | unit (Noise's X–Y; External with no source is silent; Pitch Tracking follows a synthetic voice within 5 cents; Unvoiced on a fricative and not a vowel); engine test (External through the sidechain) |
| R13 | to be written in WP-8.45 | agent API (`device.display` reads band levels headless); UI script (Range's handles) |
| R14 | to be written in WP-8.45 | unit (each channel mode's paths) |
| R15 to R17 | to be written in WP-8.45 | unit (reported latency); real-time trap, RealtimeSanitizer; golden renders at four block sizes |
| R18 to R20 | to be written in WP-8.45 | `crates/devices/tests/rules.rs`; agent API; `crates/devices/tests/banks.rs`; save and reopen |

# Auto Shift: pitch correction and voice shifting

- **Status:** draft
- **Work packages:** WP-8.46 (all of it: R1 to R24). The voice pitch tracker and the formant-keeping shifter are Beatlayer's own, `bl_dsp::pitch` (WP-8.32, [the architecture](../02-architecture.md)); what every scale-aware device shares is [scales](scales.md) R15 to R18; MIDI sidechains are [routing](routing.md) R32 to R35 and R39; the framework is WP-2.5's ([the native device framework](device-framework.md)), its LFO WP-5.42's and display path WP-5.43's
- **Parity-matrix row:** Audio effects: Auto Shift (pitch correction)
- **Checked against:** the Live 12 reference manual (online edition), chapter 29 "Live Audio Effect Reference", section 29.4 "Auto Shift" with 29.4.1 to 29.4.6, read 30 October 2026; the Live 12.1 and 12.4 release notes

## The behaviour

Auto Shift tracks the pitch of a monophonic voice and moves it. In its Quantizer tab it pulls each sung note to the nearest note of a scale, softly or hard, quickly or slowly: the familiar pitch correction, from subtle to robotic. In its MIDI tab, notes from a MIDI track set the pitch instead, so a voice is played like an instrument, with one voice or several. Its LFO tab modulates pitch, formants, volume and pan; its pitch section transposes the voice and moves its formants apart from its pitch, so the voice keeps its character or changes size; and its vibrato section adds a vibrato, natural or regular. A display shows the detected pitch and the corrected one over time.

The device takes Live's name with Beatlayer's own type ID, `beatlayer.auto_shift`, and Live 12.4's parameter names, as the manual gives them.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.4.1 "Input Section" and 29.4.1.1 "MIDI Input" | The input pitch display (note and cents); Pitch Range High, Mid and Bass, which set the detection's range and latency; Input Gain, −24 to +24 dB; the latency readout in ms; Live Mode in the title bar, lower latency with possible glitches at onsets; MIDI In (shows the MIDI panel), MIDI On (the MIDI tab replaces the Quantizer tab); the external source and its tapping point (Pre FX, Post FX, Post Mixer); Mono with Glide, or Poly with a voice count of 2, 4 or 8 |
| S2 | Manual | Section 29.4.2 "Quantizer Tab" | The pitch correction meter (cents); Correction Strength; Smooth and its Smoothing Time (0 to 200 ms); the scale's included notes; Root and Scale; Pitch Shift in scale degrees, after correction; Use Current Scale in the title bar |
| S3 | Manual | Section 29.4.3 "MIDI Tab" | A keyboard view; Attack Time (0 to 1,000 ms) and Release Time (0 to 5 s); Note Latch; Pitch Bend Range (0 to 48 semitones); modulation from Velocity, Pressure, Mod Wheel, Pitch Bend, Note PB or Slide to Pitch, Formant, Volume and Pan, each with its depth |
| S4 | Manual | Section 29.4.4 "LFO Tab" | LFO Reset, retriggering at onsets, with its lamp; LFO Delay (0 to 1.5 s); LFO Attack (0 to 2 s); the waveforms Sine, Triangle, Triangle 8, Triangle 16, Saw Up, Saw Down, Rectangle, Random and Random S&H; Rate in ms or beat divisions; depths to Pitch, Formant, Volume and Pan |
| S5 | Manual | Section 29.4.5 "Pitch Section" | Pitch Shift in semitones and its Fine in cents; Formant Shift, −100 to +100 %; Formant Follow, how far the formants follow the pitch shift |
| S6 | Manual | Section 29.4.6 "Vibrato Section" | Vibrato Amount (0 to 200 cents), Rate (2 to 15 Hz), Fade In (ms) and Natural Vibrato; Dry/Wet, 50 % for doubling |
| S7 | Release notes | Live 12.1 release notes (Auto Shift introduced); Live 12.4 release notes (Auto Shift's parameters renamed) | The device and its current names |
| S8 | Beatlayer | `bl_dsp::pitch`; [scales](scales.md) R15 to R18; [routing](routing.md) R32 to R35, R39; [the device framework](device-framework.md) R26 to R29, R35 to R39; [control surfaces](control-surfaces.md) R19; [the improvements](../05-improvements.md) G1 and G2 | The tracker and voice shifter; current scales; MIDI sidechains; the effects LFO, seeds, displays, channel messages; banks; gains and automation |

## Rules

### The device and the input

- **R1.** Auto Shift (`beatlayer.auto_shift`) is an audio effect with a MIDI sidechain (routing R32), Live's name, Beatlayer's own layout and one display cell (R21). Its body reads the input section, the tabs (Quantizer or MIDI, and LFO) with the display, the pitch section and the vibrato section. Choosing a tab, and MIDI In's panel, are view state saved with the set and not undo steps. It is scale-aware, with Use Current Scale in its title bar ([scales](scales.md) R16), and Live Mode beside it. (S1 to S6, S8)
- **R2.** The input is tracked by `bl_dsp::pitch::Tracker` on each hop, limited by Pitch Range: High (100 Hz to 1.5 kHz), Mid (70 Hz to 1 kHz, by default) or Bass (40 Hz to 500 Hz); its detected note and cents show in the input pitch display. Input Gain (−24 to +24 dB, 0 dB) sets the level into the tracker and the shifter. (S1, S8; the ranges are Beatlayer's own)
- **R3.** The device's latency is fixed by Pitch Range and Live Mode, shown in the latency readout in ms and reported for delay compensation, and its dry path is delayed to match: with Live Mode off, 40 ms for Bass, 25 ms for Mid and 15 ms for High; with it on, half those. A change of Pitch Range or Live Mode changes the reported latency, which makes a new program. (S1; the figures are Beatlayer's own)
- **R4.** A frame the tracker calls unvoiced passes through the shifter with no pitch correction, keeping only Pitch Shift (R13) and its formants as they are. (S8; Beatlayer's own)

### The Quantizer tab

- **R5.** With MIDI On off, the target pitch of each voiced frame is the nearest note of the scale the Root and Scale choosers give (C major in a new device), or, with Use Current Scale on, of the device's current scale ([scales](scales.md) R15), the choosers then showing it and dimmed. A note held across a scale change keeps its target until the next note begins (an onset or a voiced frame after an unvoiced one), as [scales](scales.md) R17 has held notes keep theirs. (S2, S8; the onset rule is Beatlayer's own)
- **R6.** Correction Strength, 0 to 100 % (100 % by default), is the share of the distance to the target the correction covers: at 100 %, a synthetic voice gliding between scale notes is held within 5 cents of the nearest scale note; at 0 % it is unchanged. With Smooth on, the correction moves to a new target over Smoothing Time (0 to 200 ms, 50 ms), a one-pole glide; with it off, from the frame the target changes. (S2; the glide's law is Beatlayer's own)
- **R7.** Pitch Shift in the Quantizer tab, −7 to +7 scale degrees (0), moves each target that many degrees along the scale after correction. The pitch correction meter shows the correction in cents. (S2; the range is Beatlayer's own)
- **R8.** The included notes are shown as keys lit for the scale; clicking a key leaves it out of or brings it back into the scale this device corrects to, saved with the device, one undo step. (S2; saving per device is Beatlayer's own)

### The MIDI tab and MIDI input

- **R9.** With MIDI On, the MIDI tab replaces the Quantizer tab and the target pitch comes from the MIDI source's notes ([routing](routing.md) R32, with its tapping point, R39): in Mono, the newest held note, moving to it over Glide (0 to 1,000 ms, 0); in Poly, up to the voice count (2, 4 or 8) of held notes each make a shifted copy of the voice, a new note past the count taking the oldest voice's place. Each note takes effect on its frame. (S1, S3; the stealing rule is Beatlayer's own)
- **R10.** Each MIDI voice fades in over Attack Time (0 to 1,000 ms, 10 ms) and out over Release Time (0 to 5 s, 100 ms) after its note-off; with Note Latch on, a voice holds its last note's pitch after the note-off until the next note. With no note held and Note Latch off, the voice passes unshifted at the dry path's level. Pitch Bend Range (0 to 48 semitones, 2) scales the source's pitch bend (device framework R39). (S3; the defaults and the no-note rule are Beatlayer's own)
- **R11.** The MIDI tab's modulation: each of Pitch (±12 semitones), Formant (±100 %), Volume (−∞ to 0 dB) and Pan (left to right) takes one source (Velocity, Pressure, Mod Wheel, Pitch Bend, Note PB or Slide) and a depth, −100 to +100 %, each depth an automatable parameter. (S3; the targets' spans are Beatlayer's own)

### The LFO tab

- **R12.** The LFO has the waveforms Sine, Triangle, Triangle 8, Triangle 16, Saw Up, Saw Down, Rectangle, Random and Random S&H (device framework R26), a Rate in ms per cycle (10 ms to 10 s) or a synced division (1/64 note to 8 bars, on the beat grid, device framework R27), LFO Delay (0 to 1.5 s) and LFO Attack (0 to 2 s), which hold it at zero and then fade it in after each reset; LFO Reset restarts it at each onset the tracker reports, its lamp lit on that frame. Its depths move Pitch (±12 semitones), Formant (±100 %), Volume (−∞ to 0 dB) and Pan, each −100 to +100 %. Its random waves follow the device's seed. (S4, S8; the spans are Beatlayer's own)

### The pitch and vibrato sections

- **R13.** Pitch Shift (−24 to +24 semitones, 0) and its Fine (−50 to +50 cents) move every output pitch after correction. Formant Shift (−100 to +100 %, 0) moves the formants by up to an octave either way, apart from the pitch; Formant Follow (0 to 100 %, 0) is how far the formants move with the pitch's shift and correction (100 % moving them by the same interval, a chipmunk at +12). A synthetic vowel shifted from −12 to +12 semitones with Formant Follow 0 keeps its first three formant peaks within 5 %. (S5, S8; the octave's span is Beatlayer's own)
- **R14.** The vibrato adds a sine to the target pitch, of Vibrato Amount (0 to 200 cents peak to peak, 0) at Vibrato Rate (2 to 15 Hz, 5.5 Hz), faded in over Fade In (0 to 2,000 ms, 300 ms) from each onset; with Natural Vibrato on, its rate and depth wander by up to 10 % at random, from the device's seed. (S6; the defaults and the wander are Beatlayer's own)
- **R15.** Dry/Wet, 0 to 100 % (100 %), balances the dry signal (delayed by the latency) against the shifted one; at 50 % a corrected voice doubles the dry one. (S6)

### The display and readback

- **R16.** The display draws, over the last four seconds, the detected pitch as a trace and the corrected (target) pitch as a second trace, against a piano ruler with the scale's notes marked; its data (both pitches per hop, the scale, the correction in cents) is published and read by `device.display` (device framework R35 to R37). (S1, S2, S8; the look is Beatlayer's own)
- **R17.** The query tool's `device` result reports the detected pitch, the target, whether the frame is voiced and the latency in ms, and agents set the MIDI source with `device.set_midi_sidechain_source` and read it back. (S8)

### Throughout

- **R18.** Use Current Scale follows the clip's scale on the frame it changes ([scales](scales.md) R15), at any block size. (S2, S8)
- **R19.** A render is bit-identical at block sizes 1, 64, 512 and 4,096 and run after run, with the LFO's random waves and Natural Vibrato. (S8)
- **R20.** Nothing on the audio thread allocates, locks or blocks; Poly's voices come from a pool of eight made with the node. (S8)
- **R21.** Every parameter is automatable and modulatable (rule G2), Use Current Scale and Live Mode included, and set and read back through the agent API; every gain reaches −∞ dB. (S8)
- **R22.** Auto Shift declares its banks of eight ([control surfaces](control-surfaces.md) R19): Main (Correction Strength, Smoothing Time, Root, Scale, Pitch Shift, Formant Shift, Formant Follow, Dry/Wet), Input and Vibrato, MIDI, and LFO, so every automatable parameter is in a bank. (S8; Beatlayer's own)
- **R23.** A set with Auto Shift saves and reopens with every parameter, its included notes and its MIDI source. (S8)
- **R24.** Every control takes the keyboard focus in the Device View, the tabs and the included-note keys among them, and assistive technology reads each with its name and value. (S8)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 to R4 | to be written in WP-8.46 | unit (description; Pitch Range limits; reported latency per range and Live Mode; unvoiced frames uncorrected) |
| R5 to R8 | to be written in WP-8.46 | unit (a synthetic voice gliding between scale notes held within 5 cents at full strength; Smoothing Time; scale-degree shift; included notes); engine test (Use Current Scale follows a clip's scale on its frame) |
| R9 to R11 | to be written in WP-8.46 | engine test (MIDI notes from a MIDI track set the target on their frames; Poly voices and stealing; Note Latch; pitch bend); unit (modulation depths) |
| R12 to R15 | to be written in WP-8.46 | unit (the LFO's rate synced and free; Reset at onsets; Shift and Formant on a synthetic vowel within 5 %; vibrato's rate and depth) |
| R16, R17 | to be written in WP-8.46 | agent API (`device.display` reads detected and corrected pitch headless) |
| R18 to R21 | to be written in WP-8.46 | real-time trap, RealtimeSanitizer; golden renders at four block sizes; `crates/devices/tests/rules.rs`; `crates/devices/tests/scales.rs` |
| R22 to R24 | to be written in WP-8.46 | `crates/devices/tests/banks.rs`; save and reopen; UI script with the accessibility tree |

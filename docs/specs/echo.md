# Echo: two delay lines with modulation, character and a reverb

- **Status:** draft
- **Work packages:** WP-8.37 (all of it: R1 to R24). The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)); its LFO, envelope follower and seeded random source are WP-5.42's (device framework R26 to R29); its display path is WP-5.43's (device framework R35 to R38); synced delay times are [the delay](delay.md)'s and [Filter Delay](filter-delay.md)'s way
- **Parity-matrix row:** Audio effects: Echo, Hybrid Reverb, Roar, Spectral Resonator, Spectral Time
- **Checked against:** the Live 12 reference manual (online edition), chapter 29 "Live Audio Effect Reference", section 29.14 "Echo" with 29.14.1 to 29.14.4, read 30 October 2026

## The behaviour

Echo is a modulated delay with the character of tape and analogue units. Two delay lines, left and right (or mid and side), each take a beat division or a time in milliseconds; their echoes are drawn as a tunnel on its display, where dragging sets the times. The input can be driven into a clip, the feedback inverted, and the repeats filtered by a high-pass and a low-pass with resonance. Three tabs hold the rest: Echo (the filters and the tunnel), Modulation (an LFO and an envelope moving the delay time and the filter) and Character (a gate and ducking that follow the input, noise and wobble that age the sound, and Repitch, which glides the pitch when the time changes). A small reverb sits before the delay, after it, or inside its feedback loop.

The device takes Live's name with Beatlayer's own type ID, `beatlayer.echo`, its own look and its own tunnel. Its input reaches silence (rule G1, IMP-08).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.14 "Echo" | Channel Mode (Stereo, Ping Pong, Mid/Side); left and right delay lines (mid and side in Mid/Side), each in beat divisions or milliseconds with its Sync switch and Sync Mode (Notes, Triplet, Dotted, 16th); Stereo Link; a Delay Offset slider per channel that swings the time; Input with a D button that drives the dry signal; Feedback with Ø, which inverts it |
| S2 | Manual | Section 29.14.1 "Echo Tab" | The Echo Tunnel shows the echoes and its handles drag the delay times; Filter switches the HP and LP filters, each with its cutoff and resonance (HP Res, LP Res); a button shows the filter's display in place of the tunnel |
| S3 | Manual | Section 29.14.2 "Modulation Tab" | Waveform (Sine, Triangle, Sawtooth Up, Sawtooth Down, Square, Noise); Sync with Rate in beat divisions, or Freq in Hz; Phase; Mod Delay, with Modulation ×4 multiplying its depth by four; Mod Filter; Env Mix, from the LFO alone at 0 % to the envelope alone at 100 % |
| S4 | Manual | Section 29.14.3 "Character Tab" | Gate (Threshold, Release); Ducking (Threshold, Release); Noise (Amount, Morph) and Wobble (Amount, Morph), each with an on switch; Repitch, a pitch glide when the delay time changes |
| S5 | Manual | Section 29.14.4 "Global Controls" | Reverb, with its location before the delay, after it, or in the feedback loop, and Decay; Stereo, mono at 0 % and wider above 100 %; Output; Dry/Wet, with Equal-Loudness in its context menu |
| S6 | Beatlayer | [The device framework](device-framework.md) R4, R8, R26 to R29, R35 to R38; [control surfaces](control-surfaces.md) R19; [the improvements](../05-improvements.md) G1, G2 and IMP-08 | Smoothing, the oversampler, the effects LFO, follower and seeded random source, display data, banks of eight; every gain reaches −∞ dB, every parameter is automatable, a delay's input reaches silence |
| S7 | Beatlayer | [The delay](delay.md) R1 and R6; [Filter Delay](filter-delay.md) R2 and R3; [the musical time spec](musical-time.md) | Synced times through the tempo map; Repitch as a glide of the read position |

## Rules

### The delay lines

- **R1.** Echo (`beatlayer.echo`) is an audio effect with Live's name, Beatlayer's own layout and one display cell. Its body is the left section (the two delay lines' controls, Input and Feedback), the tabs Echo, Modulation and Character with the display, and the global controls (Reverb, Decay, Stereo, Output, Dry/Wet). Choosing a tab is view state, saved with the set and not an undo step. (S1 to S5; saving the tab with the set is Beatlayer's own)
- **R2.** Each line's time is, with its Sync on, a division counted as Sync Mode says: Notes (1/64 note to 16 bars: 1/64, 1/32, 1/16, 1/8, 1/4, 1/2, 1, 2, 4, 8 and 16 bars), Triplet and Dotted (the same divisions, two thirds and one and a half times as long), or 16th (1 to 16 sixteenth notes); and with Sync off, a time from 1 ms to 5 s, logarithmic. Each mode's amount is a parameter of its own. A synced echo lands on the exact frame its division names through the tempo map, at any tempo and across a tempo change. (S1, S7; the ranges are Beatlayer's own)
- **R3.** Stereo Link gives the right line the left line's Sync, Sync Mode and time; each line's Delay Offset, −33 to +33 % of its division, stays its own and moves its echoes later or earlier, as a swing. Offset applies only while the line is synced. (S1; the range is Beatlayer's own)
- **R4.** Channel Mode Stereo runs the left input through the left line and the right through the right. Ping Pong sums the input into the left line, whose output feeds the right line, whose output returns to the left through Feedback, so the echoes alternate sides as [the delay](delay.md) R4's do. Mid/Side encodes the input to mid and side, runs mid through the left line and side through the right, and decodes after; the lines' controls then read Mid and Side. (S1)
- **R5.** Input is a gain from −∞ to +12 dB, 0 dB by default, on the signal entering the lines (rule G1, IMP-08): at −∞ nothing new enters and the echoes already in the lines play out. With D on, the signal after Input goes through a soft clip before the lines and to the dry path too, the curve tanh(x) normalised to unit slope at zero. (S1, S6; the range and the clip's curve are Beatlayer's own)
- **R6.** Feedback, 0 to 100 %, returns each line's output to its input through the filters, the reverb when it is in the loop (R16) and the character stages; Ø inverts the returned signal. Whatever the settings, the loop's gain at every frequency is held below 1 by a soft limit in the loop at 0 dBFS, so the echoes never grow without bound. (S1; the limit is Beatlayer's own)

### The Echo tab

- **R7.** With Filter on, each loop has a 12 dB per octave high-pass (HP, 20 Hz to 20 kHz, 20 Hz by default) and low-pass (LP, 20 Hz to 20 kHz, 20 kHz by default) in series, each with its resonance (HP Res and LP Res, 0 to 100 %, a Q from 0.5 at 0 % to 10 at 100 %). Each filter's response, measured on the first echo of an impulse, is within 0.5 dB of its design from 20 Hz to 20 kHz. With Filter off they pass everything. (S2; the slopes, ranges and Q law are Beatlayer's own)
- **R8.** The display draws the tunnel: one ring per echo, its size falling with the echo's level and its place across the cell by its channel, for the first 16 echoes of each line as the times and Feedback give them, with a handle for each line's time; dragging a handle sideways sets that line's time as one undo step. The filter-display button draws the two filters' response instead, with handles for HP and LP, dragging which sets the cutoff and, vertically, the resonance. The display data (each line's echo times and levels, the filter curve) is published as device framework R35 says and read by `device.display`, whose two views are `tunnel` and `filter`. (S2, S6; the look is Beatlayer's own)

### The Modulation tab

- **R9.** The LFO's Waveform is Sine, Triangle, Sawtooth Up, Sawtooth Down, Square or Noise (the effects LFO's Sine, Triangle, Saw Up, Saw Down, Rectangle and Random, device framework R26). With Sync on, its Rate is a division from 1/64 note to 8 bars, its cycles on the beat grid as device framework R27 says; with Sync off, Freq runs from 0.01 to 40 Hz. Phase, 0 to 360°, puts the right line's LFO ahead of the left's. (S3, S6; the ranges are Beatlayer's own)
- **R10.** The modulation source is the LFO and an envelope follower of the input (attack 10 ms, release 200 ms, device framework R28) mixed by Env Mix: the LFO alone at 0 %, the envelope alone at 100 %. Mod Delay, 0 to 100 %, moves each line's delay time by up to ±2 % of the time times the source, or ±8 % with Modulation ×4 on; Mod Filter, 0 to 100 %, moves both filters' cutoffs by up to ±2 octaves times the source. (S3, S6; the depths and the follower's times are Beatlayer's own)

### The Character tab

- **R11.** With Gate on, the signal entering the lines passes only while the input's level is above Gate Threshold (−60 to 0 dBFS, −30 dBFS by default), and closes over Gate Release (1 to 1,000 ms, 100 ms) once it falls below, the level read by an envelope follower with a 1 ms attack. (S4; ranges, defaults and the follower are Beatlayer's own)
- **R12.** With Ducking on, the wet signal is lowered while the input's level is above Ducking Threshold (−60 to 0 dBFS, −30 dBFS), by up to 30 dB in proportion to how far above it the input is, and comes back over Ducking Release (1 to 1,000 ms, 200 ms). The detector reads the input, never the echoes. (S4; the depth law, ranges and defaults are Beatlayer's own)
- **R13.** With Noise on, a hiss is added inside the loop at Amount (0 to 100 %, up to −40 dBFS), its colour moved by Morph (0 % white, 100 % a 6 dB per octave fall above 1 kHz). With Wobble on, the delay time is moved by a slow irregular motion, a sum of a 0.5 Hz and a 3.3 Hz sine and a random line ([the device framework](device-framework.md) R26's Wander at 2 Hz), by up to ±0.5 % of the time at Amount 100 %, Morph moving the balance from the slow motion to the fast. Both follow the device's seed (device framework R29), so a render repeats run after run. (S4, S6; the laws are Beatlayer's own)
- **R14.** With Repitch on, a change of delay time moves the read position to the new time over 200 ms at a constant rate, so the echoes glide in pitch as tape does, interpolating between frames so no alias passes −80 dB below the signal; with it off, a change crossfades from the old time to the new over 50 ms with no change of pitch. (S4, S7; the times are Beatlayer's own)

### The global controls

- **R15.** Reverb, 0 to 100 %, mixes a small reverb into the wet signal; Decay, 0.2 to 10 s, is its reverberation time, within 10 % measured as RT30 on its own output. The reverb is Beatlayer's own, built from the parts of [the reverb](reverb.md). (S5; the design and ranges are Beatlayer's own)
- **R16.** The reverb's location is Pre (before the delay lines, so the echoes repeat a reverberated signal), Post (after them, on their output) or Feedback (inside the loop, so each echo is more diffuse than the last); in Feedback the loop's gain stays below 1 as R6 says. (S5)
- **R17.** Stereo, 0 to 200 %, sets the wet signal's width by its mid and side: mono at 0 %, unchanged at 100 %, the side doubled at 200 %. Output is a gain from −∞ to +12 dB on the wet signal (rule G1). Dry/Wet, 0 to 100 %, balances the dry signal against the wet; with Equal-Loudness on (Dry/Wet's context menu, a switch saved with the device) the two follow sine and cosine laws, so the sum of a steady noise stays within 0.5 dB at every setting. (S5, S6; the laws are Beatlayer's own)

### Throughout

- **R18.** The delay lines are made with the node, long enough for the longest time at the slowest tempo the transport allows with the largest offset and modulation; nothing Echo does on the audio thread allocates, locks or blocks. (S6)
- **R19.** A render is bit-identical at block sizes 1, 64, 512 and 4,096 and run after run, with every modulation, Noise and Wobble on. (S6)
- **R20.** Every parameter is automatable and modulatable (rule G2) and set and read back through the agent API (`device.insert`, `device.set_param`, `device.parameters`); every gain parameter reaches −∞ dB. (S6)
- **R21.** Echo declares its banks of eight ([control surfaces](control-surfaces.md) R19): Main (the two times, Sync, Feedback, Input, Channel Mode, Reverb, Dry/Wet), Filter, Modulation, Character and Global, so every automatable parameter is in a bank. (S6; the banks are Beatlayer's own)
- **R22.** Echo reports no latency. (Beatlayer's own)
- **R23.** A set with Echo saves and reopens with every parameter, its tab and Equal-Loudness. (S6)
- **R24.** Every control takes the keyboard focus in the Device View, the tabs among them, and assistive technology reads each with its name and value. (S6)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | to be written in WP-8.37 | unit (description, layout); UI script (the tabs) |
| R2 to R6 | to be written in WP-8.37 | unit, real-time trap (echoes on their frames through a tempo change; Ping Pong and Mid/Side paths; Input at −∞; Feedback bounded at 100 % with every stage on) |
| R7, R8 | to be written in WP-8.37 | unit (measured filter responses); agent API (`device.display` in both views); UI script (a handle drag is one undo step) |
| R9, R10 | to be written in WP-8.37 | unit, real-time trap (synced LFO on the beat; Env Mix's ends) |
| R11 to R14 | to be written in WP-8.37 | unit, real-time trap (gate and ducking times on generated bursts; seeded Noise and Wobble repeat; Repitch's glide and its aliasing) |
| R15 to R17 | to be written in WP-8.37 | unit (RT30, the three locations, Stereo, Equal-Loudness) |
| R18, R19 | to be written in WP-8.37 | real-time trap, RealtimeSanitizer, golden renders at four block sizes |
| R20, R21 | to be written in WP-8.37 | `crates/devices/tests/rules.rs`; agent API; `crates/devices/tests/banks.rs` |
| R22 to R24 | to be written in WP-8.37 | unit; save and reopen; UI script with the accessibility tree |

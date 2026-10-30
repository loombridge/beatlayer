# Spectral Resonator: harmonic resonances in the spectrum, played by notes

- **Status:** draft
- **Work packages:** WP-8.48 (all of it: R1 to R20). The short-time Fourier transform and the spectrogram display kind are Beatlayer's own, `bl_dsp::stft` (WP-8.28, [the architecture](../02-architecture.md)) and [the device framework](device-framework.md)'s spectrogram rule; MIDI sidechains are [routing](routing.md) R32 to R35 and R39; what every scale-aware device shares is [scales](scales.md) R15 to R18; tunings are [tunings](tunings.md)'
- **Parity-matrix row:** Audio effects: Echo, Hybrid Reverb, Roar, Spectral Resonator, Spectral Time
- **Checked against:** the Live 12 reference manual (online edition), chapter 29 "Live Audio Effect Reference", section 29.36 "Spectral Resonator" with 29.36.1 to 29.36.6, read 30 October 2026; the Live 12.4.3 release notes

## The behaviour

Spectral Resonator makes any sound ring at a pitch and its harmonics, in the spectral domain. The input's spectrum is drawn toward a comb of partials at a fundamental frequency, each partial ringing on with its own decay, damped in the highs and lows as the person sets; Stretch spreads the partials apart or squeezes them together, Harmonics sets how many there are, and Shift moves the input's spectrum before it rings. The fundamental is set by a control or by notes from a MIDI track, one voice or up to sixteen, with glide and pitch bend. A modulation section adds chorus, a wandering motion or granular scatter to the partials, Unison thickens them, and a spectrogram shows the result.

The device takes Live's name with Beatlayer's own type ID, `beatlayer.spectral_resonator`.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.36 "Spectral Resonator", its pitch mode | Internal or MIDI; in MIDI, the external source and tapping point, Mono or Poly with Polyphony (2 to 16 voices), MIDI Gate (on or off; always on in Poly), Glide (ms, Mono only) and PB (0 to 24 semitones) |
| S2 | Manual | The same section, its frequency section | Freq, in Hz or as a pitch; Use Current Scale, which turns Freq into SD Shift; Transp. (±48 semitones, or ±28 scale degrees with Use Current Scale); Stretch (below 0 % compressing, above 0 % expanding, 100 % only the odd harmonics); Shift (±48 semitones, on the input only); Decay; HF Damp; LF Damp |
| S3 | Manual | The same section, its modulation section, spectrogram and global parameters | The modes None, Chorus, Wander and Granular, with Mod Rate and Pch. Mod; Harmonics, the number of partials, more being brighter; Quantize, which keeps partials on the scale or tuning, chromatic without one; Input Send with a limiter lamp; Unison (the voice count) and Uni. Amt (its detune); Dry/Wet |
| S4 | Release notes | Live 12.4.3 release notes | Control-surface mappings of the spectral devices improved |
| S5 | Beatlayer | `bl_dsp::stft`; [routing](routing.md) R32 to R35, R39; [scales](scales.md) R15 to R18; [tunings](tunings.md) R1, R6, R12, R16; [the device framework](device-framework.md) R29, R30, R35 to R39 and its spectrogram rule; [control surfaces](control-surfaces.md) R19; [the improvements](../05-improvements.md) G1 and G2 | The STFT; MIDI sidechains; current scales; tunings; seeds, notes, channel messages, displays; banks; gains and automation |

## Rules

### The device and the resonance

- **R1.** Spectral Resonator (`beatlayer.spectral_resonator`) is an audio effect with a MIDI sidechain (routing R32), Live's name, Beatlayer's own layout and one display cell, the spectrogram (R15). It is scale-aware, with Use Current Scale in its title bar ([scales](scales.md) R16). (S1 to S3, S5)
- **R2.** The input, raised by Input Send (a gain from −∞ to +12 dB, 0 dB, rule G1) and kept below 0 dBFS by a limiter whose lamp lights while it acts, is analysed by `bl_dsp::stft` (a 2,048-frame Hann window at 48 kHz, scaled with the rate, four times overlap). Each voice's partials sit at f_k = f0 · k^(1 + s) for k from 1 to Harmonics (1 to 64, 32 by default), where f0 is the voice's fundamental and s is Stretch (−100 to +100 %, 0) as a fraction; at Stretch +100 % only the odd partials (k = 1, 3, 5 …) sound, as the manual says, the law moving between. Partials above the Nyquist frequency are dropped. Each partial's resonance falls within 5 cents of f_k. (S2, S3; the law, the window and the ranges are Beatlayer's own)
- **R3.** Each bin near a partial (within its bandwidth, a quarter of a semitone) accumulates the input's energy there and rings on: its magnitude decays exponentially so it falls 60 dB in Decay (50 ms to 20 s, 1 s by default) for a partial at 1 kHz, faster above it by HF Damp (0 to 100 %, up to 8 times faster at 20 kHz) and below it by LF Damp (0 to 100 %, up to 8 times faster at 20 Hz). A partial's ring time is within 10 % of its law. Bins away from every partial are silent in the wet signal. (S2; the law is Beatlayer's own)
- **R4.** Shift (−48 to +48 semitones, 0) moves the input's spectrum by its interval before it reaches the resonances; the partials stay where they are. (S2)

### The pitch modes

- **R5.** In Internal, the fundamental is Freq (20 Hz to 5 kHz, logarithmic, 110 Hz by default), shown in Hz or as a note as its mode buttons choose, plus Transp. (−48 to +48 semitones, 0). With Use Current Scale on, Freq becomes SD Shift, a degree of the current scale (−28 to +28 degrees from the scale's root in the octave of C3), and Transp. counts degrees. (S2, S5; the ranges and the octave are Beatlayer's own)
- **R6.** In MIDI, the fundamental is the pitch of the source's notes ([routing](routing.md) R32, with its tapping point, R39) under the set's tuning, plus Transp. In Mono, the newest held note leads, gliding to a new note over Glide (0 to 5,000 ms, 0); with MIDI Gate on, the resonances sound only while a note is held, their Decay ringing out after the note-off; with it off, they ring at the last note. In Poly, each held note is a voice, up to Polyphony (2 to 16, 8 by default); a note past it takes the oldest voice's place; MIDI Gate is always on. PB (0 to 24 semitones, 2) scales the source's pitch bend (device framework R39), which moves every voice. Each note takes effect on its frame. (S1, S5; the stealing rule and defaults are Beatlayer's own)
- **R7.** With Quantize on, each partial is moved to the nearest note of the current scale (or, under a tuning, the tuning's nearest note); with no scale and no tuning, to the nearest semitone. (S3, S5)
- **R8.** Under a tuning, Freq's note readout, the MIDI notes' pitches and Quantize follow the tuning ([tunings](tunings.md) R1, R6, R12); under one that doesn't hold twelve notes, Use Current Scale is disabled ([tunings](tunings.md) R16). (S3, S5)

### Modulation and unison

- **R9.** Modulation is None, Chorus, Wander or Granular, with Mod Rate (0.01 to 20 Hz, 1 Hz) and Pch. Mod (0 to 12 semitones, 0.2): Chorus moves each partial's frequency by a sine of Pch. Mod depth at Mod Rate, each partial's phase offset by its number; Wander moves each by its own random line (the effects LFO's Wander, device framework R26) at Mod Rate; Granular gates each partial on and off at random at Mod Rate, a new draw each grain, and moves its pitch by up to Pch. Mod at random. Every random part follows the device's seed. (S3, S5; the laws are Beatlayer's own)
- **R10.** Unison (1 to 8 voices, 1) duplicates each voice's partials, detuned evenly across ±Uni. Amt (0 to 100 %, up to ±50 cents), spread between the channels. (S3; the laws are Beatlayer's own)
- **R11.** Dry/Wet, 0 to 100 % (100 %), balances the dry signal (delayed by the latency) against the resonances. The device reports the STFT's latency. (S3, S5)

### Throughout

- **R12.** Voices come from a pool of 16 made with the node, and Unison's copies from a pool sized for 16 voices of 8; allocating a voice, gliding and bending allocate, lock and block nothing on the audio thread. (S5)
- **R13.** A render is bit-identical at block sizes 1, 64, 512 and 4,096 and run after run, with every modulation mode and MIDI notes. (S5)
- **R14.** Agents set the MIDI source with `device.set_midi_sidechain_source` and read it back; the query tool's `device` result lists the sounding voices' fundamentals. (S5)
- **R15.** The spectrogram draws the wet output's magnitudes over the last four seconds against log frequency, with the partials of each sounding voice marked, in Beatlayer's own colour ramp, as [the device framework](device-framework.md)'s spectrogram kind lays it out; `device.display` reads it headless. (S3, S5)
- **R16.** Every parameter is automatable and modulatable (rule G2), Use Current Scale included, and set and read back through the agent API; every gain reaches −∞ dB. (S5)
- **R17.** Spectral Resonator declares its banks of eight ([control surfaces](control-surfaces.md) R19): Main (Freq, Transp., Decay, Stretch, Shift, Harmonics, Input Send, Dry/Wet), Damping and Modulation (HF Damp, LF Damp, Modulation, Mod Rate, Pch. Mod, Unison, Uni. Amt, Quantize) and MIDI (Pitch Mode, Mono/Poly, Polyphony, MIDI Gate, Glide, PB), so every automatable parameter is in a bank. (S4, S5; Beatlayer's own)
- **R18.** A set with the device saves and reopens with every parameter and its MIDI source. (S5)
- **R19.** Use Current Scale follows the clip's scale on the frame it changes ([scales](scales.md) R15), and a held note keeps its pitch across a scale change ([scales](scales.md) R17). (S5)
- **R20.** Every control takes the keyboard focus in the Device View and assistive technology reads each with its name and value. (S5)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 to R4 | to be written in WP-8.48 | unit (partials within 5 cents of the law; Stretch at −100, 0 and +100 %; Harmonics; partials above Nyquist dropped; Decay and the damps within 10 %; Shift) |
| R5 to R8 | to be written in WP-8.48 | engine test (MIDI notes tune it on their frames; Poly up to its count with the oldest stolen; MIDI Gate; glide; bend); unit (SD Shift in a scale; Quantize; under a tuning) |
| R9 to R11 | to be written in WP-8.48 | unit (each modulation mode's law, seeded; Unison's detune; reported latency) |
| R12, R13 | to be written in WP-8.48 | real-time trap, RealtimeSanitizer; golden renders at four block sizes |
| R14, R15 | to be written in WP-8.48 | agent API (`device.display` reads the spectrogram headless; the MIDI source read back) |
| R16 to R20 | to be written in WP-8.48 | `crates/devices/tests/rules.rs`; `crates/devices/tests/scales.rs`; `crates/devices/tests/banks.rs`; save and reopen; UI script |

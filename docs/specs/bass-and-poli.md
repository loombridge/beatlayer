# Bass and Poli: a monophonic and a polyphonic virtual analogue synthesiser

- **Status:** draft
- **Work packages:** WP-8.16 (this spec, R1 to R18). Voices, glide and per-note bends are [the instruments' voice section](instrument-voices.md); the filter is [the instruments' filter](instrument-filter.md); Poli's chorus reuses [Chorus-Ensemble](chorus-ensemble.md)'s chorus
- **Parity-matrix row:** Instruments: Bass, Poli
- **Checked against:** Ableton's Creative Extensions pack page (ableton.com/en/packs/creative-extensions/), read 30 October 2026; neither device is described in the Live 12 reference manual or the Live 12 release notes, read 30 October 2026

## The behaviour

Bass and Poli are two instruments of Live's Creative Extensions pack. Bass is a monophonic virtual analogue synthesiser meant to cover the whole range of bass sounds in one device, from deep and clean to heavily distorted. Poli is a polyphonic virtual analogue synthesiser for classic chords and phrases, whose character comes from detunable oscillators, a modulation section and a built-in chorus.

Beatlayer builds both with Live's names, on the instruments' shared voice section and filter. The pack page names no controls, so every control, range and law below is Beatlayer's own, in the shape of a classic monophonic bass synthesiser and a classic polyphonic synthesiser.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Product page | Ableton, "Creative Extensions" pack page (ableton.com/en/packs/creative-extensions/), read 30 October 2026 | Bass, a monophonic virtual analogue synthesiser covering bass sounds from deep and clean to heavily distorted; Poli, a virtual analogue synthesiser for classic polyphonic chords and phrases, with detunable oscillators, a modulation section and a built-in chorus; the pack's other devices (Melodic Steps, Pitch Hack, Gated Delay, Color Limiter, Re-Enveloper and Spectral Blur) |
| S2 | Beatlayer | [The instruments' voice section](instrument-voices.md) and [filter](instrument-filter.md); [Chorus-Ensemble](chorus-ensemble.md); [the polysynth](polysynth.md) R3 and R15 | The shared voices and filter; the chorus; band-limited oscillators and the velocity law |
| S3 | Beatlayer | [The device framework](device-framework.md) R1 to R16; [the improvements](../05-improvements.md), rules G1 and G2 | Descriptors, the generic panel, gains reaching −∞ dB, every parameter automatable and modulatable |

## Rules

**Both**

- **R1.** Beatlayer ships Bass (`beatlayer.bass`) and Poli (`beatlayer.poli`), instruments listed under the browser's Instruments label, each with its generic panel in pages (device framework R41) and factory presets of Beatlayer's own. (S1, S3)
- **R2.** Every control of both is a parameter, automatable and modulatable (rule G2), set by `device.set_param`, read by `device.parameters` and held in the device's banks; every gain reaches −∞ dB (rule G1). (S3)
- **R3.** Their oscillators are band-limited at every key, folding back less than −80 dB at the top key, as Drift's are ([polysynth](polysynth.md) R3). (S2)

**Bass**

- **R4.** Bass plays one voice (instrument voices R6): the newest held key sounds, and releasing it returns to the most recent key still held. (S1, S2)
- **R5.** A note that arrives while another key is held glides to its pitch over Glide (0 ms to 2 s, 60 ms by default) in a straight line in semitones and doesn't restart the envelopes; a note after every key was released starts at its pitch and restarts them (instrument voices R7, R9, with Legato always on). (S1; Beatlayer's own)
- **R6.** Bass's oscillator morphs from saw to square by Shape (0 to 100 %, 0 % by default), at Octave (−2 to +1, −1 by default) and Tune (−12 to +12 semitones); Sub (−∞ to 0 dB, −∞ by default) adds a sine an octave below, and Noise (−∞ to 0 dB, −∞ by default) adds white noise from the device's seed. (S1; Beatlayer's own)
- **R7.** Bass's filter is the instruments' low-pass at 24 dB per octave with every circuit it offers (instrument filter R2), Freq (20 Hz to 20 kHz, 400 Hz by default), Res, Drive and Key (0 to 100 %); its envelope is an Attack and Decay (0 ms to 10 s each) moving the cutoff by Env Amount (−100 % to 100 %, up to six octaves). (S1, S2; Beatlayer's own)
- **R8.** Bass's amplifier envelope has Attack, Decay, Release (0 ms to 10 s) and Sustain (0 to 100 %), Vel > Vol (0 to 100 %, Drift's law) and Volume (−∞ to 0 dB). After the amplifier, Distortion (0 to 100 %, 0 % by default) drives a soft clipper by up to 36 dB with its output scaled back by half the drive, for the heavily distorted end. (S1, S2; Beatlayer's own)

**Poli**

- **R9.** Poli plays up to Voices (1 to 16, 8 by default) through the voice section (instrument voices R3, R4), with Glide (0 ms to 2 s, 0 ms) as polyphonic Portamento and a pitch bend range (0 to 24 semitones, 2 by default). (S1, S2; Beatlayer's own)
- **R10.** Poli's two oscillators each have Shape (Saw, Square or Pulse), Octave (−2 to +2), Level (−∞ to 0 dB) and, on the second, Detune (−50 to +50 cents, 7 cents by default), so the two beat against each other; Pulse's width is PW (5 to 95 %, 50 % by default). (S1; Beatlayer's own)
- **R11.** Poli's filter is the instruments' low-pass or high-pass at 12 or 24 dB per octave with Clean, MS2 and PRD (instrument filter R2), Freq, Res, Key (0 to 100 %) and its own ADSR envelope with Env Amount (−100 % to 100 %, up to six octaves). (S2; Beatlayer's own)
- **R12.** Poli's amplifier envelope is an ADSR with Vel > Vol and Volume, as Bass's (R8) without the distortion. (S2; Beatlayer's own)
- **R13.** Poli's modulation section is an LFO (Sine, Triangle, Square, Saw or S&H; Rate 0.05 to 20 Hz or synced from 1/16 to 4 bars; Delay 0 ms to 5 s, fading in from the note's start) sent to Pitch (up to an octave), PW (its whole range) and Filter (up to four octaves) by three amounts from −100 % to 100 %, with the mod wheel scaling the LFO's depth by Wheel (0 to 100 %). The LFO runs per voice, restarting with each note. (S1; Beatlayer's own)
- **R14.** Poli's chorus, after the amplifier, is Chorus-Ensemble's chorus ([Chorus-Ensemble](chorus-ensemble.md)) in its two-voice mode, with a switch (on in a new device), Rate, Amount and Dry/Wet, in stereo. (S1, S2; Beatlayer's own)

**Throughout**

- **R15.** Each device's output is the same at any block size and run after run; nothing either does on the audio thread allocates, locks or blocks. (S3)
- **R16.** Sixteen voices of Poli render at 48 kHz inside the device budget. (S3)
- **R17.** Every control's name is the one Beatlayer's panel shows, and every one reads its value with its unit (device framework R5). (S3)
- **R18.** A per-note bend on either device moves its note as instrument voices R14 says, with 48 semitones per ±1. (S2)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R2, R17 | `crates/devices/tests/banks.rs`; `crates/devices/tests/rules.rs`; to be written in WP-8.16 (agent API) | unit; agent API |
| R3 | to be written in WP-8.16 | unit (fold-back at the top key) |
| R4, R5 | to be written in WP-8.16 | render under the allocation trap (one voice; legato glide without retrigger) |
| R6, R7, R8 | to be written in WP-8.16 | render; golden render |
| R9, R10, R11, R12, R13, R14 | to be written in WP-8.16 | render; golden render |
| R15, R16 | to be written in WP-8.16 | render at block sizes 1, 64, 512 and 4,096; benchmark |
| R18 | to be written in WP-8.16 | render |

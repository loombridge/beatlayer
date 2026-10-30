# Color Limiter: a limiter with character

- **Status:** draft
- **Work packages:** WP-8.50, with [Re-Enveloper](re-enveloper.md) (all of it: R1 to R11). Its look-ahead limiting is [the limiter](limiter.md)'s, the same code; the framework is WP-2.5's ([the native device framework](device-framework.md))
- **Parity-matrix row:** Audio effects: Convolution Reverb, Color Limiter, Gated Delay, Pitch Hack, PitchLoop89, Re-Enveloper, Spectral Blur, Vector Delay
- **Checked against:** Ableton's Creative Extensions pack page, read 30 October 2026; the Live 12 reference manual, which doesn't describe the device

## The behaviour

Color Limiter is a limiter with the grit of hardware: Loudness drives the signal into it, Ceiling sets the level it never passes, Lookahead and Release set how it catches peaks and lets go, and Saturation and Color add harmonics and tilt the tone.

Live's Color Limiter comes in its Creative Extensions pack as a Max device; Beatlayer's is native, with Live's name, Beatlayer's own type ID (`beatlayer.color_limiter`) and look, and its controls drafted from the pack page.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Product page | Ableton, "Creative Extensions" pack page (ableton.com/en/packs/creative-extensions/), read 30 October 2026 | Color Limiter, a limiter inspired by hardware, with Loudness (the input level), Ceiling (the threshold), Lookahead, Release, Saturation and Color |
| S2 | Beatlayer | [The limiter](limiter.md) R2, R3 and R5; [the saturator](saturator.md); [the device framework](device-framework.md) R4, R8; [control surfaces](control-surfaces.md) R19; [the improvements](../05-improvements.md) G1 and G2 | Look-ahead limiting under a ceiling with its latency reported; shapers; smoothing, the oversampler; banks; gains and automation |

## Rules

- **R1.** Color Limiter (`beatlayer.color_limiter`) is an audio effect with Live's name, Beatlayer's own layout and a gain-reduction meter in its one display cell, whose reading `device.display` reports. (S1; the layout is Beatlayer's own)
- **R2.** Loudness, a gain from −∞ to +24 dB (0 dB by default, rule G1), drives the input into the device. (S1; the range is Beatlayer's own)
- **R3.** Saturation, 0 to 100 % (0 by default), passes the signal after Loudness through a soft clipper, y = tanh(k x) / k with k from 1 to 4, run at twice the sample rate (device framework R8), before the limiter. Color, −100 to +100 % (0), tilts the saturated signal about 1 kHz by up to ±6 dB at the ends of the band, a pair of first-order shelves, before the limiter, so a brighter Color is limited harder in the highs. (S1; the laws are Beatlayer's own)
- **R4.** The limiter is [the limiter](limiter.md)'s look-ahead stage, the same code: Ceiling (−24 to 0 dB, −0.3 dB) is the highest sample peak the output reaches; Lookahead is 1, 3 or 6 ms (3 ms) and is reported as latency, with the oversampler's, and the dry path delayed to match; Release (1 to 1,000 ms, 100 ms, logarithmic) is the gain's recovery time constant. (S1, S2; ranges and defaults are Beatlayer's own)
- **R5.** The output's sample peak never passes Ceiling, at any setting of Loudness, Saturation and Color. (S1, S2)
- **R6.** Saturation's and Color's measured static curve and tilt are within 0.01 and 0.5 dB of their laws. (Beatlayer's own)
- **R7.** A render is bit-identical at block sizes 1, 64, 512 and 4,096, and nothing on the audio thread allocates, locks or blocks. (S2)
- **R8.** Every parameter is automatable and modulatable (rule G2) and set and read back through the agent API; every gain reaches −∞ dB. (S2)
- **R9.** Color Limiter declares one bank of eight ([control surfaces](control-surfaces.md) R19): Loudness, Ceiling, Lookahead, Release, Saturation, Color. (S2; Beatlayer's own)
- **R10.** A set with the device saves and reopens with every parameter. (S2)
- **R11.** The device reuses the limiter's look-ahead and the saturator's oversampling rather than copies of them. (S2)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | to be written in WP-8.50 | unit; agent API (`device.display`) |
| R2 to R6 | to be written in WP-8.50 | unit (output never above Ceiling; latency reported; Saturation's and Color's measured curves) |
| R7 | to be written in WP-8.50 | golden renders at four block sizes; real-time trap |
| R8 to R11 | to be written in WP-8.50 | `crates/devices/tests/rules.rs`; agent API; `crates/devices/tests/banks.rs`; save and reopen |

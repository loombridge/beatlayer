# Spectral Blur: a frequency range smeared into a cloud

- **Status:** draft
- **Work packages:** WP-8.49 (all of it: R1 to R12). The short-time Fourier transform is Beatlayer's own, `bl_dsp::stft` (WP-8.28, [the architecture](../02-architecture.md)); the framework is WP-2.5's ([the native device framework](device-framework.md)), its seeded random source WP-5.42's
- **Parity-matrix row:** Audio effects: Convolution Reverb, Color Limiter, Gated Delay, Pitch Hack, PitchLoop89, Re-Enveloper, Spectral Blur, Vector Delay
- **Checked against:** Ableton's Creative Extensions pack page, read 30 October 2026; the Live 12 reference manual, which doesn't describe the device

## The behaviour

Spectral Blur takes a band of frequencies the person chooses and smears it into a dense, reverb-like cloud, while the rest of the sound passes through. Halo sets how long the blurred grains are, so how far each moment is smeared in time; Residual sets how much of the unblurred signal is kept; Freeze holds the cloud as it is.

Live's Spectral Blur comes in its Creative Extensions pack as a Max device; Beatlayer's is native, with Live's name, Beatlayer's own type ID (`beatlayer.spectral_blur`) and look, and its controls drafted from the pack page.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Product page | Ableton, "Creative Extensions" pack page (ableton.com/en/packs/creative-extensions/), read 30 October 2026 | Spectral Blur blurs a chosen frequency range into a reverb-like cloud; Halo is the length of the blurred grains; Residual the level of the unprocessed signal; Freeze holds the affected signal while on |
| S2 | Beatlayer | `bl_dsp::stft`; [the device framework](device-framework.md) R4, R29, R35 to R38; [control surfaces](control-surfaces.md) R19; [the improvements](../05-improvements.md) G1 and G2 | The STFT and its latency; smoothing, seeds, displays; banks; gains and automation |

## Rules

- **R1.** Spectral Blur (`beatlayer.spectral_blur`) is an audio effect with Live's name, Beatlayer's own layout and one display cell showing the chosen range over a live spectrum, its two edges draggable. (S1; the layout is Beatlayer's own)
- **R2.** The range is Low (20 Hz to 20 kHz, 200 Hz by default) and High (20 Hz to 20 kHz, 5 kHz), High kept at or above Low. The input is analysed by `bl_dsp::stft` (a 2,048-frame Hann window at 48 kHz, scaled with the rate, four times overlap); the device reports its latency and delays the dry path to match. (S1; ranges and the window are Beatlayer's own)
- **R3.** Inside the range, each bin's magnitude is spread over time: each hop it feeds a decaying accumulator whose time to fall 60 dB is Halo (20 ms to 10 s, 1 s by default), and the bin's phase is replaced each hop by a random phase from the device's seed, so the output is a diffuse cloud whose energy envelope, measured on a generated tone burst, decays at Halo's rate within 10 %. Outside the range the bins pass as they came. (S1; the law is Beatlayer's own)
- **R4.** Residual, 0 to 100 % (0 by default), is the share of the range's own unblurred signal kept alongside the cloud: at 0 % the range holds only the cloud, at 100 % its original signal too. (S1; the law is Beatlayer's own)
- **R5.** With Freeze on, the accumulators stop decaying and stop taking input, so the cloud holds within 0.5 dB for as long as Freeze stays on, while the signal outside the range keeps passing; turning it off returns them to their decay. Freeze is a switch parameter, automatable. (S1)
- **R6.** Wet Gain, a gain from −∞ to +12 dB (0 dB, rule G1), sets the cloud's level, and Dry/Wet, 0 to 100 % (100 %), balances the dry signal against the result. (S1; the controls are Beatlayer's own reading of the pack page's "wet controls")
- **R7.** A render is bit-identical at block sizes 1, 64, 512 and 4,096 and run after run. (S2)
- **R8.** Nothing on the audio thread allocates, locks or blocks. (S2)
- **R9.** Every parameter is automatable and modulatable (rule G2) and set and read back through the agent API; every gain reaches −∞ dB. (S2)
- **R10.** Spectral Blur declares one bank of eight ([control surfaces](control-surfaces.md) R19): Low, High, Halo, Residual, Freeze, Wet Gain, Dry/Wet. (S2; Beatlayer's own)
- **R11.** The display's data (the spectrum and the range) is published and read by `device.display`; dragging an edge sets Low or High as one undo step. (S2)
- **R12.** A set with the device saves and reopens with every parameter. (S2)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R2 | to be written in WP-8.49 | unit (description; reported latency) |
| R3 to R6 | to be written in WP-8.49 | unit (energy outside the range passes; inside, spread over Halo's time within 10 %; Residual's ends; Freeze holds within 0.5 dB) |
| R7, R8 | to be written in WP-8.49 | golden renders at four block sizes; real-time trap |
| R9 to R12 | to be written in WP-8.49 | `crates/devices/tests/rules.rs`; agent API; `crates/devices/tests/banks.rs`; UI script; save and reopen |

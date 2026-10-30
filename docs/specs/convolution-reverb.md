# Convolution Reverb: a native convolution reverb

- **Status:** draft
- **Work packages:** WP-8.36 (all of it: R1 to R14). Impulse-response slots, the chooser and the bundled responses are [impulse responses](impulse-responses.md)'; the convolution path it shares with [Hybrid Reverb](hybrid-reverb.md)'s convolution engine (R4 to R6 there) is the same code; the framework is WP-2.5's ([the native device framework](device-framework.md))
- **Parity-matrix row:** Audio effects: Convolution Reverb, Color Limiter, Gated Delay, Pitch Hack, PitchLoop89, Re-Enveloper, Spectral Blur, Vector Delay
- **Checked against:** Ableton's Convolution Reverb pack page, read 30 October 2026; the Live 12 reference manual, which doesn't describe the device

## The behaviour

Convolution Reverb places a sound in a space by convolving it with that space's impulse response. The person chooses a response from Beatlayer's bundled ones or drops their own audio file on the device, then shapes it: a predelay before it, its size, how it fades in and dies away, the balance of its early reflections against its late tail, an EQ on the reverb, the stereo width and the mix with the dry signal. It adds no latency.

Live's Convolution Reverb comes in a pack as a Max device; Beatlayer's is native, with Live's name, Beatlayer's own type ID (`beatlayer.convolution_reverb`) and look, and its controls drafted from the pack page. Convolution Reverb Pro is not covered.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Product page | Ableton, "Convolution Reverb" pack page (ableton.com/en/packs/convolution-reverb/), read 30 October 2026 | A sample-based convolution reverb using impulse responses of real spaces and hardware, with no latency; an audio file dragged onto the device becomes its response; a larger Pro version adds EQ, damping, modulation, reverse and two responses |
| S2 | Beatlayer | [Impulse responses](impulse-responses.md); [Hybrid Reverb](hybrid-reverb.md) R4 to R6, R9 and R10 | Slots, loading, the chooser; Attack, Decay and Size by preparing a new response off the audio thread; the EQ's bands; Stereo's law |
| S3 | Beatlayer | [The device framework](device-framework.md) R4, R35 to R38; [control surfaces](control-surfaces.md) R19; [the improvements](../05-improvements.md) G1 and G2 | Smoothing, display data, banks of eight, gains to −∞ dB, automatable parameters |

## Rules

- **R1.** Convolution Reverb (`beatlayer.convolution_reverb`) is an audio effect with Live's name, Beatlayer's own layout, one impulse-response slot named IR holding a bundled room in a new device, the chooser of [impulse responses](impulse-responses.md) R10 to R14 at the top of its body, and one display cell. (S1, S2; the layout and the starting response are Beatlayer's own)
- **R2.** The device convolves the input with the slot's response on the shared convolution engine, through the same code as Hybrid Reverb's convolution engine, with no latency, in mono, stereo or true stereo as the response's channels say. A file dropped on the display or dragged from the browser loads as [impulse responses](impulse-responses.md) R2 and R14 say, one undo step. (S1, S2)
- **R3.** Predelay, 0 to 250 ms (0 by default), delays the reverb against the dry signal, landing on its frame. (S1; Beatlayer's own)
- **R4.** Size, 20 to 100 % (100 % by default), plays the response shortened in time by that factor as [Hybrid Reverb](hybrid-reverb.md) R5 says. Attack (0 to 1,000 ms) and Decay (50 ms to 30 s) shape the response's envelope as Hybrid Reverb's do; a change prepares a new response off the audio thread and crossfades to it (Hybrid Reverb R6). (S1, S2; Beatlayer's own)
- **R5.** Early/Late, 0 to 100 % (50 % by default), balances the response's first 80 ms against the rest: at 50 % the response is as it is; below it the late part is lowered, to silence at 0 %, and above it the early part is lowered, to silence at 100 %, with a 5 ms crossfade at the split so no step is heard. (Beatlayer's own)
- **R6.** The EQ is a low cut (a 12 dB per octave high-pass, 20 Hz to 2 kHz, 20 Hz by default) and a high cut (a 12 dB per octave low-pass, 1 to 20 kHz, 20 kHz by default) on the reverb, each within 0.5 dB of its design. (Beatlayer's own)
- **R7.** Width, 0 to 200 % (100 % by default), scales the reverb's side against its mid as Hybrid Reverb's Stereo does (R10 there): mono at 0 %, the side doubled at 200 %. (S2; Beatlayer's own)
- **R8.** Gain is a gain from −∞ to +12 dB on the reverb (rule G1), and Dry/Wet, 0 to 100 % (30 % by default), balances the dry signal against it. (S3; Beatlayer's own)
- **R9.** The display draws the response's waveform with Predelay, Size, Attack, Decay and the early/late split drawn on it, and its category and name; its data is published and read by `device.display` (device framework R35 to R37). (S3; the look is Beatlayer's own)
- **R10.** Switching the device off and on clears its tail. (Beatlayer's own, as [Hybrid Reverb](hybrid-reverb.md) R15)
- **R11.** A render is bit-identical at block sizes 1, 64, 512 and 4,096, and nothing on the audio thread allocates, locks or blocks. (S3)
- **R12.** Every parameter is automatable and modulatable (rule G2), set and read back through the agent API; its response is read back as [impulse responses](impulse-responses.md) R12 says. (S3)
- **R13.** Convolution Reverb declares its banks of eight ([control surfaces](control-surfaces.md) R19): Main (Predelay, Size, Attack, Decay, Early/Late, Width, Gain, Dry/Wet) and EQ (Low Cut, High Cut), so every automatable parameter is in a bank. (S3; Beatlayer's own)
- **R14.** A set with the device saves and reopens with every parameter and its response. (S2, S3)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R2 | to be written in WP-8.36 | unit (description; the output equals the shared engine's for the same response); UI script (a drop is one undo step) |
| R3 to R8 | to be written in WP-8.36 | unit on generated responses (Predelay's frame, Size's arrival times, the envelopes, Early/Late's split, the EQ's responses, Width from mono to stereo, Gain at −∞) |
| R9 | to be written in WP-8.36 | agent API (`device.display`) |
| R10, R11 | to be written in WP-8.36 | unit; real-time trap; golden renders at four block sizes |
| R12 to R14 | to be written in WP-8.36 | `crates/devices/tests/rules.rs`; agent API; `crates/devices/tests/banks.rs`; save and reopen |

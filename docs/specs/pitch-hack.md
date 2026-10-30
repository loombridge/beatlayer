# Pitch Hack: a pitch-shifting delay line

- **Status:** draft
- **Work packages:** WP-8.51, with [Gated Delay](gated-delay.md) (all of it: R1 to R11). Delay-line pitch shifting is [the pitch and frequency shifter](pitch-and-frequency-shifter.md) R3's, the same code; the framework is WP-2.5's ([the native device framework](device-framework.md)), its seeded random source WP-5.42's
- **Parity-matrix row:** Audio effects: Convolution Reverb, Color Limiter, Gated Delay, Pitch Hack, PitchLoop89, Re-Enveloper, Spectral Blur, Vector Delay
- **Checked against:** Ableton's Creative Extensions pack page, read 30 October 2026; the Live 12 reference manual, which doesn't describe the device

## The behaviour

Pitch Hack is a delay whose echoes come out transposed. Each echo can be reversed, the transposition can be chosen at random from echo to echo, and feedback folds the transposed echoes back into the line, so they climb or fall further each time.

Live's Pitch Hack comes in its Creative Extensions pack as a Max device; Beatlayer's is native, with Live's name, Beatlayer's own type ID (`beatlayer.pitch_hack`) and look, and its controls drafted from the pack page.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Product page | Ableton, "Creative Extensions" pack page (ableton.com/en/packs/creative-extensions/), read 30 October 2026 | Pitch Hack, a real-time pitch-shifting delay line: transposition in semitones and cents, reversing the audio, randomising the transposition, and folding the delayed signal back into the line |
| S2 | Beatlayer | [The pitch and frequency shifter](pitch-and-frequency-shifter.md) R3; [the delay](delay.md) R1; [the device framework](device-framework.md) R4, R29; [control surfaces](control-surfaces.md) R19; [the improvements](../05-improvements.md) G1, G2 and IMP-08 | Delay-line pitch shifting with two crossfaded heads; synced times; smoothing, seeds; banks; gains and automation; a delay's input reaching silence |

## Rules

- **R1.** Pitch Hack (`beatlayer.pitch_hack`) is an audio effect with Live's name, Beatlayer's own layout and no display cell. (S1; the layout is Beatlayer's own)
- **R2.** The delay's time is, with Sync on (the default), a division from 1/32 note to 1 bar with triplet and dotted forms (1/8 by default), landing on its frame through the tempo map, or with Sync off a time from 10 ms to 2 s (250 ms). The input enters through Input, a gain from −∞ to +12 dB (0 dB, rule G1 and IMP-08). (S1, S2; the ranges are Beatlayer's own)
- **R3.** Each echo leaves the line transposed by Transpose (−24 to +24 semitones, +7) plus Cents (−50 to +50), through the shifter's delay-line pitch shifting ([the pitch and frequency shifter](pitch-and-frequency-shifter.md) R3, the same code) with a 30 ms window, so a steady tone's echo is at the transposition within 5 cents. (S1, S2; the window is Beatlayer's own)
- **R4.** Reverse, 0 to 100 % (0), is the chance each echo plays its window of the line backwards, drawn per echo from the device's seed; at 100 % every echo is reversed. (S1; the chance law is Beatlayer's own)
- **R5.** Random, 0 to 100 % (0), is the chance each echo takes, instead of Transpose, an interval drawn at random from the set −12, −7, −5, 0, +5, +7, +12 semitones, from the device's seed. (S1; the set and the law are Beatlayer's own)
- **R6.** Feedback, 0 to 100 % (30 %), returns each transposed echo to the line's input, so the next echo is transposed again; the loop's gain stays below 1. (S1)
- **R7.** Dry/Wet, 0 to 100 % (50 %), balances the dry signal against the echoes. The device reports no latency. (S1; Beatlayer's own)
- **R8.** The line is made with the node for the longest time at the slowest tempo; nothing on the audio thread allocates, locks or blocks; a render is bit-identical at block sizes 1, 64, 512 and 4,096 and run after run. (S2)
- **R9.** Every parameter is automatable and modulatable (rule G2) and set and read back through the agent API; every gain reaches −∞ dB. (S2)
- **R10.** Pitch Hack declares one bank of eight ([control surfaces](control-surfaces.md) R19): Time, Sync, Transpose, Cents, Reverse, Random, Feedback, Dry/Wet, with Input in a second. (S2; Beatlayer's own)
- **R11.** A set with the device saves and reopens with every parameter. (S2)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R2 | to be written in WP-8.51 | unit (description; echo times synced and free; Input at −∞) |
| R3 to R7 | to be written in WP-8.51 | unit (echoes at their transposition within 5 cents; reversed when set; random intervals follow the seed run after run; feedback's cumulative transposition) |
| R8 | to be written in WP-8.51 | real-time trap; golden renders at four block sizes |
| R9 to R11 | to be written in WP-8.51 | `crates/devices/tests/rules.rs`; agent API; `crates/devices/tests/banks.rs`; save and reopen |

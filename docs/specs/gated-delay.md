# Gated Delay: a delay fed through a step-sequenced gate

- **Status:** draft
- **Work packages:** WP-8.51, with [Pitch Hack](pitch-hack.md) (all of it: R1 to R12). Synced times are [the delay](delay.md)'s way, steps on the song's grid [Beat Repeat](beat-repeat.md)'s; the framework is WP-2.5's ([the native device framework](device-framework.md)), its seeded random source WP-5.42's
- **Parity-matrix row:** Audio effects: Convolution Reverb, Color Limiter, Gated Delay, Pitch Hack, PitchLoop89, Re-Enveloper, Spectral Blur, Vector Delay
- **Checked against:** Ableton's Creative Extensions pack page, read 30 October 2026; the Live 12 reference manual, which doesn't describe the device

## The behaviour

Gated Delay sends the signal into a delay only on the active steps of a gate sequencer that runs in time with the song, so echoes come in rhythmic patterns. The person sets the steps, their number and rate, the delay's time as a number of steps, whether the dry signal plays on the inactive steps, how far the steps are randomised, and the feedback.

Live's Gated Delay comes in its Creative Extensions pack as a Max device; Beatlayer's is native, with Live's name, Beatlayer's own type ID (`beatlayer.gated_delay`) and look, and its controls drafted from the pack page.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Product page | Ableton, "Creative Extensions" pack page (ableton.com/en/packs/creative-extensions/), read 30 October 2026 | Gated Delay sends the signal to a delay on the active steps of a gate sequencer synced to the song; the step length and rate; the dry signal on or off for inactive steps; the delay's rate against the steps; randomised steps; feedback |
| S2 | Beatlayer | [The delay](delay.md) R1; [Beat Repeat](beat-repeat.md); [the device framework](device-framework.md) R4, R29, R35 to R38; [control surfaces](control-surfaces.md) R19; [the improvements](../05-improvements.md) G1, G2 and IMP-08 | Synced times through the tempo map; steps on the song's grid; smoothing, seeds, displays; banks; gains and automation; a delay's input reaching silence |

## Rules

- **R1.** Gated Delay (`beatlayer.gated_delay`) is an audio effect with Live's name, Beatlayer's own layout and one display cell, the step row (R9). (S1; the layout is Beatlayer's own)
- **R2.** The sequencer has 16 steps, each a switch parameter (Step 1 to Step 16; steps 1, 4, 7, 11 and 14 on in a new device), automatable (rule G2); Steps (1 to 16, 16) sets how many play before the row repeats, and Rate (1/32, 1/16, 1/8, 1/4 note, with triplet and dotted forms; 1/16 by default) the length of a step. Step 1 begins on each bar line, counted on the song's grid through the tempo map, so a step opens on its frame at any tempo and block size. (S1, S2; the count, the defaults and the bar alignment are Beatlayer's own)
- **R3.** While a step is on, the input (raised by Input, a gain from −∞ to +12 dB, 0 dB, rule G1 and IMP-08) enters the delay line; while it is off nothing enters, the gate opening and closing over 2 ms so no click is heard. (S1; the ramp is Beatlayer's own)
- **R4.** The delay's time is Delay Steps (1 to 16, 3) steps of Rate, so an echo lands exactly that many steps after its step, through the tempo map. Feedback, 0 to 100 % (40 %), returns the line's output to its input; the echoes already in the line repeat whether the current step is on or off. (S1, S2; the ranges are Beatlayer's own)
- **R5.** With Dry on Rests on (the default), the dry signal plays during off steps and the gate's on steps send it only to the delay; with it off, the dry signal plays throughout. (S1; the reading is Beatlayer's own)
- **R6.** Random, 0 to 100 % (0), is the chance each step is turned over for one pass of the row, drawn at the start of each pass from the device's seed (device framework R29) counted from the song's bar, so a render repeats run after run and the same bar draws the same steps. The step switches themselves aren't changed. (S1, S2; the law is Beatlayer's own)
- **R7.** Dry/Wet, 0 to 100 % (50 %), balances the dry signal against the echoes. (S1; Beatlayer's own)
- **R8.** The delay line is made with the node for 16 steps of a dotted quarter note at the slowest tempo the transport allows; nothing on the audio thread allocates, locks or blocks. A render is bit-identical at block sizes 1, 64, 512 and 4,096. (S2)
- **R9.** The step row draws the 16 steps lit while on and the playing step marked; a click turns a step over, one undo step named after it. Its data (the steps, the playing step, which steps Random turned over this pass) is published and read by `device.display`. (S2; the look is Beatlayer's own)
- **R10.** Every parameter is automatable and modulatable (rule G2) and set and read back through the agent API; every gain reaches −∞ dB. (S2)
- **R11.** Gated Delay declares its banks of eight ([control surfaces](control-surfaces.md) R19): Main (Steps, Rate, Delay Steps, Feedback, Random, Dry on Rests, Input, Dry/Wet), Steps 1–8 and Steps 9–16. (S2; Beatlayer's own)
- **R12.** A set with the device saves and reopens with every parameter. (S2)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R2 | to be written in WP-8.51 | unit (description; steps on their frames through a tempo change at four block sizes) |
| R3 to R7 | to be written in WP-8.51 | unit (the gate's ramp; echoes at Delay Steps; Feedback; Dry on Rests; Random follows the seed run after run; Input at −∞) |
| R8 | to be written in WP-8.51 | real-time trap; golden renders at four block sizes |
| R9 | to be written in WP-8.51 | agent API (`device.display`); UI script (a click is one undo step) |
| R10 to R12 | to be written in WP-8.51 | `crates/devices/tests/rules.rs`; agent API; `crates/devices/tests/banks.rs`; save and reopen |

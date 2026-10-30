# Vector Delay: a multi-tap delay whose taps move as particles

- **Status:** draft
- **Work packages:** WP-8.52, with [PitchLoop89](pitchloop89.md) (all of it: R1 to R13). The framework is WP-2.5's ([the native device framework](device-framework.md)), its seeded random source WP-5.42's and display path WP-5.43's
- **Parity-matrix row:** Audio effects: Convolution Reverb, Color Limiter, Gated Delay, Pitch Hack, PitchLoop89, Re-Enveloper, Spectral Blur, Vector Delay
- **Checked against:** Ableton's Inspired by Nature pack page, read 30 October 2026; the Live 12 reference manual, which doesn't describe the device

## The behaviour

Vector Delay is a delay with several taps, each drawn as a particle on its display. The particles move under simulated gravity and attraction, or through a flowing field, and a particle's place sets its tap's delay time and level. Because the taps move, their echoes slide in pitch, detune against one another, and reverse when a tap moves fast enough.

Live's Vector Delay comes in its Inspired by Nature pack as a Max device; Beatlayer's is native, with Live's name, Beatlayer's own type ID (`beatlayer.vector_delay`) and look, and its controls drafted from the pack page.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Product page | Ableton, "Inspired by Nature" pack page (ableton.com/en/packs/inspired-nature/), read 30 October 2026 | Vector Delay is a multitap delay in which each particle on screen is a delay line; the pack's vector devices move particles under simulated gravity and magnetism or through a flow field, which modulates audio parameters; pitch shifting, detuning and reversed delays result |
| S2 | Beatlayer | [The device framework](device-framework.md) R4, R29, R35 to R38; [modulation](modulation.md) (the 32-frame grid); [control surfaces](control-surfaces.md) R19; [the improvements](../05-improvements.md) G1, G2 and IMP-08 | Smoothing, seeds, displays; the grid; banks; gains and automation; a delay's input reaching silence |

## Rules

- **R1.** Vector Delay (`beatlayer.vector_delay`) is an audio effect with Live's name, Beatlayer's own layout and one display cell, the particle field (R8). (S1; the layout is Beatlayer's own)
- **R2.** Taps (1 to 8, 4 by default) particles move in a unit square. A particle's horizontal place x sets its tap's delay time, Max Time × x (Max Time 50 ms to 4 s, or synced, a division from 1/16 note to 2 bars; 1 s by default); its vertical place y sets its level, from −∞ at the bottom to 0 dB at the top; its tap is panned by its number, alternating left and right by Spread (0 to 100 %, 50 %). (S1; the mapping is Beatlayer's own)
- **R3.** The particles move by forces worked out on a fixed grid of 32 frames, independent of the block size: Gravity (−100 to +100 %, 0) pulls them down (or up); Magnet (−100 to +100 %, 0) pulls them toward the centre (or pushes them away); Flow (0 to 100 %, 0) moves them along a slowly turning field of noise from the device's seed; Speed (0 to 400 %, 100 %) scales time for the motion; Friction (0 to 100 %, 20 %) damps their velocity. They bounce off the square's walls. A tap's delay time moves smoothly between grid points. (S1, S2; the forces and laws are Beatlayer's own)
- **R4.** A tap whose delay time changes at a rate r (seconds of delay per second) plays its echo at a pitch ratio of 1 − r, interpolated so no alias passes −70 dB below the signal: a tap whose delay grows is lower, one whose delay shrinks higher, and one whose delay grows faster than time passes (r above 1) plays backwards. On a steady tone, each tap's echo is within 5 cents of 12 log2(|1 − r|) semitones at its rate. (S1; the law is the Doppler effect of a moving read head)
- **R5.** Reset, a button (a parameter whose rising edge acts), puts the particles back to their starting places, evenly spaced along the diagonal, at rest; the transport starting does the same, so a render from the song's start repeats run after run. (S2; Beatlayer's own)
- **R6.** Feedback, 0 to 100 % (30 %), returns the taps' sum to the line's input; Input is a gain from −∞ to +12 dB (0 dB, rule G1 and IMP-08); Dry/Wet, 0 to 100 % (50 %), balances the dry signal against the taps. The device reports no latency. (S1; ranges are Beatlayer's own)
- **R7.** The line is made with the node for 4 seconds at 192 kHz; nothing on the audio thread allocates, locks or blocks; a render is bit-identical at block sizes 1, 64, 512 and 4,096 and run after run. (S2)
- **R8.** The display draws each particle at its place with its tap's number, and a dragged particle moves to the drop place, as an undo step that sets that tap's starting place (Tap N X and Tap N Y, parameters). Its data (each particle's place and velocity, each tap's delay time and level) is published on each grid point and read by `device.display`. (S1, S2; the look is Beatlayer's own)
- **R9.** Every parameter is automatable and modulatable (rule G2) and set and read back through the agent API; every gain reaches −∞ dB. (S2)
- **R10.** Vector Delay declares its banks of eight ([control surfaces](control-surfaces.md) R19): Main (Taps, Max Time, Gravity, Magnet, Flow, Speed, Feedback, Dry/Wet), Motion (Friction, Spread, Input, Reset) and Places (the taps' starting places), so every automatable parameter is in a bank. (S2; Beatlayer's own)
- **R11.** A set with the device saves and reopens with every parameter. (S2)
- **R12.** With Gravity, Magnet and Flow at 0 and every particle at rest, the device is a plain multi-tap delay: each tap's echo lands on Max Time × x within one frame. (S1)
- **R13.** Every control takes the keyboard focus in the Device View and assistive technology reads each with its name and value; each particle is reachable from the keyboard on the display, the arrow keys moving its starting place. (S2; Beatlayer's own)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R2, R12 | to be written in WP-8.52 | unit (description; taps at their times at rest; level and pan mapping) |
| R3 to R5 | to be written in WP-8.52 | unit (motion on the 32-frame grid at four block sizes; a moving tap's pitch within 5 cents of the law; reversal; Reset and transport start repeat the motion) |
| R6, R7 | to be written in WP-8.52 | unit (Feedback; Input at −∞); real-time trap; golden renders at four block sizes |
| R8 | to be written in WP-8.52 | agent API (`device.display` reads the taps' places headless); UI script (a drag is one undo step) |
| R9 to R11, R13 | to be written in WP-8.52 | `crates/devices/tests/rules.rs`; agent API; `crates/devices/tests/banks.rs`; save and reopen; UI script with the accessibility tree |

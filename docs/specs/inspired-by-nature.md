# Inspired by Nature: Tree Tone, Emit, Vector FM and Vector Grain

- **Status:** draft
- **Work packages:** WP-9.44 (what the four share, Tree Tone and Emit: R1 to R18); WP-9.45 (Vector FM and Vector Grain: R19 to R32). Each is a bundled extension built with the SDK ([extensions](extensions.md) R46 to R48), drawing its display through the extension device panels ([extensions](extensions.md) R32 to R37). Vector Delay is Phase 8's, native; Bouncy Notes is [sequencer devices](sequencer-devices.md)'
- **Parity-matrix row:** Instruments: Inspired by Nature (Tree Tone, Emit, Vector FM, Vector Grain)
- **Checked against:** Ableton's Inspired by Nature pack page, read 30 October 2026

## The behaviour

Four devices make sound from simulated natural motion and growth, each drawing what it simulates. Tree Tone grows a fractal tree whose branches are resonators: longer, thicker branches sound lower, louder and longer; an internal noise source excites them, or incoming audio passes through them as a filter bank. Emit is a granular instrument whose grains are particles moving across a sample's spectrogram: their horizontal motion sets where in the sample they play, their vertical motion their filtering and panning, with walls and friction shaping the motion, and playback that keeps its pitch at any speed. Vector FM is an FM synthesiser whose operators are particles, so their motion moves the sound. Vector Grain is a granular looper whose grains are particles, their direction mapped to start position, pitch, filter cutoff, reverse and grain size.

They take Live's names, with Beatlayer's own type IDs, look and laws: where Ableton's page doesn't say how something sounds or moves, the law here is Beatlayer's own. All their motion comes from seeded randomness, so a render repeats exactly.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Product page | Ableton's Inspired by Nature pack page (ableton.com/en/packs/inspired-nature) | Tree Tone: a resonator device; each branch of a grown tree is a resonator with its frequency, decay and amplitude, longer and thicker branches lower, louder and longer; an internal noise generator excites them, or external audio passes through as a filter bank. Emit: a visual granular instrument playing a sample, particles moving across a spectrogram, horizontal motion for playback and vertical for filtering and panning, with walls and friction, and phase-vocoder playback so particles move at any speed without changing pitch. Vector FM: an FM synthesiser in which each particle is an operator or a pair, with voices and formations. Vector Grain: a granular looper in which each particle is a grain, its direction mapped to start position, pitch, filter cutoff, reverse and grain size |
| S2 | Paper | Lindenmayer, "Mathematical models for cellular interactions in development", Journal of Theoretical Biology, 1968; Chowning, "The Synthesis of Complex Audio Spectra by Means of Frequency Modulation", JAES, 1973; Roads, "Microsound", 2001 | Branching growth by rewriting rules; FM synthesis; granular synthesis |
| S3 | Spec | [extensions](extensions.md) R24 to R37; [device framework](device-framework.md) | How extension devices run, draw and are tested |

## Rules

### What they share (WP-9.44)

- **R1.** Each device is a bundled extension's component, listed with Live's name: Tree Tone, Emit and Vector FM among the instruments, Vector Grain among the audio effects; each with presets in Beatlayer's own format. (S1; the categories are Beatlayer's own)
- **R2.** Each simulation steps at a fixed internal rate of 1,000 steps a second in the device's own time, from a seed saved with the device, so the same notes and audio give the same output on the same frames at any block size. (S3; Beatlayer's own)
- **R3.** Each display draws its simulation from the device's published state; a gesture on it (dragging a particle's source, a wall, the tree's root) sets the parameter it moves as one gesture and one undo step. (S1, S3)
- **R4.** Nothing a device does allocates, locks or blocks on the audio thread: particle and grain pools are made with the instance. (S3)

### Tree Tone (WP-9.44)

- **R5.** Tree Tone grows a tree from its Seed, Branches (2 to 6 per node), Depth (1 to 7 levels), Angle and Length Ratio, by a branching rule: each branch splits into Branches children, each Length Ratio times its parent's length. (S1, S2; the rule is Beatlayer's own)
- **R6.** Each branch is a resonator: a two-pole band-pass whose frequency falls as the branch's length rises (the trunk at the played note's frequency, each child at its length ratio's inverse times its parent's, folded into the audible range), whose decay rises with its thickness, and whose gain rises with its thickness. (S1; the laws are Beatlayer's own)
- **R7.** A note played excites the tree with a burst of the internal noise source, shaped by Exciter Attack and Decay, at a level from its velocity, and sets the trunk's pitch. (S1; Beatlayer's own)
- **R8.** With Input on, audio from the device's sidechain passes through the tree as a filter bank, mixed with the noise by Input Mix. (S1)
- **R9.** Up to 128 branches sound; past it, the shortest branches are left out. (Beatlayer's own)
- **R10.** Its display draws the tree, each branch lit by its resonator's level. (S1)
- **R11.** Growth (re-growing the tree when its shape parameters change) crossfades the old tree into the new over 50 ms. (Beatlayer's own)

### Emit (WP-9.44)

- **R12.** Emit plays a sample dropped on it. Particles are emitted at Rate per second from an emitter on the sample's spectrogram, each with a direction, a speed and a life; each particle plays a grain of the sample at the time its horizontal position gives, through a phase vocoder, so its pitch doesn't depend on its speed. (S1)
- **R13.** A particle's vertical position sets its grain's filter (low positions low-passed, high positions high-passed) and its pan. (S1; the mapping is Beatlayer's own)
- **R14.** Walls at the spectrogram's edges reflect particles with a Bounce factor; Friction slows them; Gravity pulls them down. (S1; the controls are Beatlayer's own)
- **R15.** Notes play the grains at their pitch from C3, polyphonic up to 8 voices, each with an amplitude envelope. (Beatlayer's own)
- **R16.** Grain size is 10 ms to 1 s, with a window shape. (Beatlayer's own)
- **R17.** Up to 256 particles live at once; past it, the oldest dies. (Beatlayer's own)
- **R18.** Its display draws the spectrogram and the particles moving over it. (S1)

### Vector FM (WP-9.45)

- **R19.** Vector FM has up to 8 particles, each an operator (a sine oscillator at a ratio of the note's frequency) or a pair, moving in a square field; a particle's distance from the field's centre sets its modulation index, and its angle its feedback. (S1, S2; the mapping is Beatlayer's own)
- **R20.** The particles modulate in a chain in their order, the last a carrier; Formation sets their starting places and speeds (Circle, Line, Swarm, Grid, Random). (S1; the formations are Beatlayer's own)
- **R21.** Each note is a voice with its own particles, up to 16 voices, with an amplitude envelope and Glide. (S1)
- **R22.** Motion follows Speed, Gravity towards the centre, and Friction. (Beatlayer's own)
- **R23.** Its display draws the field and each particle, sized by its level. (S1)
- **R24.** A filter (low-pass, 12 or 24 dB per octave) follows the voices. (Beatlayer's own)
- **R25.** With the motion stopped (Speed 0), a note sounds as a fixed FM patch, identical on every note. (Beatlayer's own)

### Vector Grain (WP-9.45)

- **R26.** Vector Grain's source is a loop buffer of 1 to 16 bars: a sample dropped on it fills it, and Capture records its input into it from the next bar. (S1; capture is Beatlayer's own)
- **R27.** Each particle is a grain; its direction sets, through four mappings each with an amount, the grain's start position in the loop, its pitch (±24 semitones), its filter cutoff, and whether it plays reversed; its speed sets its grain size (10 ms to 500 ms). (S1)
- **R28.** Up to 64 particles live at once, emitted at Density per second. (Beatlayer's own)
- **R29.** Dry/Wet mixes the grains with the input. (Beatlayer's own)
- **R30.** With Sync on, emissions fall on divisions of the beat. (Beatlayer's own)
- **R31.** Its display draws the loop's waveform and the particles over it. (S1)
- **R32.** The loop buffer is reserved with the instance at its longest length, so capturing allocates nothing. (S3)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 to R18 | to be written in WP-9.44: tests under `extensions/nature/` and renders identical at block sizes 1, 61 and 512 under the allocation trap | unit, engine test |
| R19 to R32 | to be written in WP-9.45 | unit, engine test |

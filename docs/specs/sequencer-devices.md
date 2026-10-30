# Sequencer devices: Melodic Steps, Step Arp, SQ Sequencer, Bouncy Notes and Rhythmic Steps

- **Status:** draft
- **Work packages:** WP-9.40 (what the five share, Melodic Steps, Step Arp and SQ Sequencer: R1 to R24); WP-9.41 (Bouncy Notes and Rhythmic Steps: R25 to R36). Each is a bundled extension built with the SDK ([extensions](extensions.md) R46 to R48), drawing its display through the extension device panels ([extensions](extensions.md) R32 to R37)
- **Parity-matrix row:** MIDI effects: Melodic Steps, Bouncy Notes, Step Arp, SQ Sequencer, Rhythmic Steps (the Sequencer Pack)
- **Checked against:** Ableton's product pages for the Sequencers pack, the Creative Extensions pack and the Inspired by Nature pack, read 30 October 2026

## The behaviour

Five MIDI effects play notes of their own from steps, lanes or motion, in time with the song. Melodic Steps plays shifting, polyrhythmic melodies from lanes of different lengths. Step Arp is an arpeggiator whose steps are each edited, in velocity, length, chance and transposition. SQ Sequencer, after classic hardware step sequencers, has up to 16 steps with a lane per value, each lane with its own length and direction, and conditions that decide whether a step plays. Bouncy Notes drops a ball onto the key of each note it receives, and the ball sends a note each time it lands. Rhythmic Steps sequences a Drum Rack's pads, a lane per pad, with a chance per step, swing and randomisation.

They take Live's names, with Beatlayer's own type IDs, look and laws. Each is a bundled extension, built from source in the repository with Beatlayer's SDK, and lists among the MIDI effects.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Product page | Ableton's Sequencers pack page (ableton.com/en/packs/sequencers) | Step Arp: an arpeggio sequencer whose steps are each edited, among them velocity, note length, chance and transposition. SQ Sequencer: after classic hardware step sequencers, up to 16 steps, a lane per parameter, each lane with its own length and playback direction, and a Condition per step deciding whether it plays. Rhythmic Steps: for Drum Racks and polyrhythmic beats, a trigger chance per step, a page assigning a drum sound to each lane, swing, and randomisation |
| S2 | Product page | Ableton's Creative Extensions pack page (ableton.com/en/packs/creative-extensions) | Melodic Steps: a MIDI sequencer for shifting, polyrhythmic melodies |
| S3 | Product page | Ableton's Inspired by Nature pack page (ableton.com/en/packs/inspired-nature) | Bouncy Notes: a gravity-driven MIDI effect; a played note drops a ball onto its key on a piano roll, and the ball bounces and sends a new note wherever it lands; the roll's key and note range can be set |
| S4 | Spec | [Arpeggiator](arpeggiator.md) (rates, synced divisions, retriggering); [scales](scales.md) (Use Current Scale); [extensions](extensions.md) R24 to R37; [device framework](device-framework.md) | How Beatlayer's MIDI effects keep time and use the current scale; how extension devices run and draw |

## Rules

### What they share (WP-9.40)

- **R1.** Each device is a bundled extension's `midi-effect` component, listed among the MIDI effects with Live's name, with presets in Beatlayer's own preset format. (S1, S2, S3, S4)
- **R2.** A step's time is a division of the song's beat at the device's Rate (1/1 to 1/32, with triplets and dotted values), and a step plays on the exact frame its time falls on, at any block size. (S4; Beatlayer's own)
- **R3.** The devices play while the transport plays, from the song position: step *n* at a song position is the same however the song got there. While the transport is stopped they play nothing, except Bouncy Notes (R25). (Beatlayer's own)
- **R4.** Each device's randomness comes from a seed saved with it, so a render repeats exactly. (Beatlayer's own)
- **R5.** Each device has Use Current Scale, which keeps the notes it makes in the clip's current scale ([scales](scales.md)). (S4)
- **R6.** Each edit in a step display is one parameter gesture and one undo step, and reads back headless through `device.display` and the query tool's `device`. (S4)
- **R7.** Nothing a device does allocates, locks or blocks on the audio thread. (S4)

### Melodic Steps (WP-9.40)

- **R8.** Melodic Steps has five lanes, Pitch, Octave, Velocity, Length and Chance, each of 1 to 16 steps with its own length, so lanes of different lengths drift against each other and the pattern repeats only after their lengths' least common multiple of steps. (S2; the lanes and their counts are Beatlayer's own)
- **R9.** Pitch steps are scale degrees from the device's Root in its Scale (or the current scale); Octave steps add −2 to +2 octaves; Velocity steps are 1 to 127; Length steps are a share of the step from 1 % to 200 %; Chance steps are 0 to 100 %. (Beatlayer's own)
- **R10.** Each lane has a direction (forward, backward, ping-pong, random) and its own rate multiplier (×1/4 to ×4). (Beatlayer's own)
- **R11.** A step whose Chance fails plays nothing. (Beatlayer's own)
- **R12.** Incoming notes transpose the pattern by their distance from C3 while held, with a Hold switch keeping the last. (Beatlayer's own)

### Step Arp (WP-9.40)

- **R13.** Step Arp arpeggiates the held notes in a style (Up, Down, Up-Down, Played, Random) at its Rate, through up to 16 steps that each set Velocity, Length, Chance, Transpose (−24 to +24 semitones) and Octave, and whether the step plays. (S1; the styles and ranges are Beatlayer's own)
- **R14.** The step count is 1 to 16; the arpeggio's position and the step's position advance together, one note a step. (Beatlayer's own)
- **R15.** With no notes held, nothing plays; the first note held restarts the steps at step 1 when Retrigger is on. (S4)
- **R16.** Its display shows each step's values as bars, the playing step lit. (Beatlayer's own)

### SQ Sequencer (WP-9.40)

- **R17.** SQ Sequencer has up to 16 steps and lanes for Pitch, Velocity, Gate Length and Chance, plus two Modulation lanes sending a MIDI controller each. (S1; the lanes beyond the page's are Beatlayer's own)
- **R18.** Each lane has its own length (1 to 16) and direction (forward, backward, ping-pong, random), so lanes run against each other. (S1)
- **R19.** Each step has a Condition: Always, a chance (1 % to 99 %), every *n*th pass of the lane (1:2 to 4:4 style ratios, play on pass *a* of every *b*), First pass only, or Not First; a step plays only when its Condition holds. (S1; the conditions are Beatlayer's own)
- **R20.** Pitch steps are notes from C−2 to G8, or scale degrees with Use Current Scale. (Beatlayer's own)
- **R21.** A step's Gate Length over 100 % ties into the next step. (Beatlayer's own)
- **R22.** Incoming notes transpose the sequence from C3 and, with Trigger on, start it at step 1. (Beatlayer's own)
- **R23.** Its display shows a column per step with a knob row per lane, as hardware sequencers do, the playing step lit per lane. (S1; the look is Beatlayer's own)
- **R24.** The Modulation lanes' controllers are chosen from 1 to 119, with each step's value 0 to 127. (Beatlayer's own)

### Bouncy Notes (WP-9.41)

- **R25.** Each note Bouncy Notes receives drops a ball onto its key on a piano roll of the device's Root and range; the ball falls under Gravity, bounces with the Bounce factor (0 to 1), and sends a note of its key each time it lands, at a velocity from its speed, until its bounces fall below the Threshold or its Lifetime ends. Balls bounce while the transport is stopped too. (S3; the controls and laws are Beatlayer's own)
- **R26.** The motion is worked out per frame in song time at a fixed internal rate of 1,000 steps a second, from the seed for any randomness, so the same input gives the same notes on the same frames at any block size. (Beatlayer's own)
- **R27.** Sync quantises each landing's note to a division of the beat. (Beatlayer's own)
- **R28.** With Scale on, the roll shows and plays only the scale's keys. (S3)
- **R29.** A note's length is a share of the time to the next landing. (Beatlayer's own)
- **R30.** Its display draws the roll and the balls as they fall, from the device's published state. (S3; the look is Beatlayer's own)

### Rhythmic Steps (WP-9.41)

- **R31.** Rhythmic Steps has eight lanes, each sending one note (a Drum Rack pad's), with 1 to 32 steps per lane and its own length, so lanes run polyrhythmically. (S1; the counts are Beatlayer's own)
- **R32.** Each step has on or off, Velocity and Chance; a step whose Chance fails plays nothing. (S1)
- **R33.** A second page assigns each lane its note, chosen from the pads of the Drum Rack after the device or by number. (S1)
- **R34.** Swing delays every other step by 0 to 75 % of a step, per lane or for all. (S1; the range is Beatlayer's own)
- **R35.** Randomise sets the steps, velocities and chances of the chosen lanes from the seed, as one undo step. (S1)
- **R36.** Its display shows the lanes as rows of steps, the playing steps lit. (Beatlayer's own)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 to R24 | to be written in WP-9.40: tests under `extensions/sequencers/` and `crates/shell/tests/sequencer_devices.rs` at block sizes 1, 61 and 512 under the allocation trap; display edits read back | unit, engine test |
| R25 to R36 | to be written in WP-9.41 | unit, engine test |

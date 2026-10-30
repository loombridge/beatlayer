# Modular: Beatlayer's native modular device

- **Status:** draft
- **Work packages:** WP-9.36 (the patch, typed ports, compiling and swapping, voices, patch parameters, saving, presets, the commands and the first blocks: R1 to R20, and R21 to R30 for its blocks); WP-9.37 (the rest of the blocks: R31 to R42); WP-9.38 (the patch editor: R43 to R52). The device framework's descriptors, layout and floating windows are [device framework](device-framework.md)'s; macros are [macros](macros.md)'; the current scale is [scales](scales.md)'
- **Parity-matrix row:** none: Beatlayer's own device, which takes no Live device's place
- **Checked against:** Beatlayer's own design, 30 October 2026

## The behaviour

Modular is an instrument, audio effect or MIDI-driven effect a person builds from blocks without code. A patch is a set of blocks (oscillators, filters, envelopes, LFOs, amplifiers, mixers, arithmetic, delays, sample players, sequencers and Beatlayer's own effects) joined by cables from typed output ports to input ports. Patch parameters are the device's controls: automatable, modulatable and mappable to macros like any device's. An instrument patch plays polyphonically, each voice running its own copy of the blocks that depend on the note.

Every edit compiles the patch into a new schedule off the audio thread, swapped in at a block boundary with the blocks' state carried over, so playing never clicks when a cable moves. The patch editor opens in the device's display cell, which floats for room; every edit is a registered command and one undo step, so a keyboard alone, a screen reader or an agent can build a patch as a mouse can.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Beatlayer | The architecture's "Extensions" (a native modular device of `bl-dsp` blocks), D5 (program swaps at block boundaries), D12, "Real-time rules" and "Agent control" (D17) | A patcher of `bl-dsp` blocks for people who build instruments and effects without code; programs built off the audio thread and swapped; nothing on the audio thread allocates, locks or blocks |
| S2 | Spec | [Device framework](device-framework.md) (descriptors, layout, display cells, floating windows under IMP-14); [macros](macros.md) R1 and R2; [scales](scales.md) (the current scale on devices); [polysynth](polysynth.md) (voices and stealing); [accessibility](accessibility.md) | How devices, macros, scales, voices and keyboard paths work in Beatlayer |
| S3 | Paper | Puckette, "The Theory and Technique of Electronic Music", 2007, chapters on unit generators, delay networks and feedback | Signal graphs of unit generators, and why a feedback loop needs a delay of at least one sample |

## Rules

### The patch (WP-9.36)

- **R1.** Modular is a native device in three kinds, each its own device in the browser: Modular Instrument, Modular Effect and Modular MIDI Effect. A new one holds a patch with its kind's in and out blocks connected: Note In to Audio Out through an oscillator and an amplifier with an envelope for the instrument, Audio In to Audio Out for the effect, Note In to Note Out for the MIDI effect. (S1)
- **R2.** A patch holds blocks, each with an ID, a type, a place in the editor and its settings, and cables, each from one output port to one input port. Ports have types: **audio** (a sample per frame), **control** (a value per frame, from −1 to 1 unless the block says otherwise), **gate** (on above 0.5) and **notes** (note and expression events). A cable joins ports of one type, except that control and audio join each other; an input port takes one cable, an output port any number. (S1)
- **R3.** A cycle of cables is allowed only through a Delay block of at least one sample; a patch edit that would make a cycle without one is refused with the reason and changes nothing. (S1, S3)
- **R4.** Every edit compiles the patch off the audio thread into a schedule (the blocks in an order where each follows the blocks it reads), and the engine swaps it in at the next block boundary. Blocks kept by the edit carry their state (phases, filter memories, envelope stages, delay lines) into the new schedule; new blocks start from rest. The output doesn't click at the swap. (S1)
- **R5.** An instrument patch's blocks that depend on Note In run once per voice; the rest once per device. The patch's Voices parameter sets how many voices play, 1 to 32, 8 on a new device; a new note past it steals the oldest voice with a 5 ms fade, as [polysynth](polysynth.md) does. (S1, S2)
- **R6.** A block setting can be made a patch parameter: it becomes one of the device's parameters, with the setting's name, range, unit and default, automatable, modulatable and mappable to a macro. A patch has up to 64 patch parameters, in the order made. (S1, S2)
- **R7.** Every block works out its output frame by frame, so a patch renders identically at any block size, and nothing in it allocates, locks or blocks on the audio thread. (S1)
- **R8.** A patch is saved with the set as part of the device, and with its presets; a preset holds the whole patch and its editor layout. (S1)
- **R9.** `modular.add_block`, `modular.remove_block`, `modular.connect`, `modular.disconnect`, `modular.move_block`, `modular.set` (a block's setting) and `modular.expose` (make a setting a patch parameter, or stop) are `edit` commands, each one undo step; `modular.patch` (`read`) gives the patch's blocks, ports, cables, parameters and layout, and is each edit's readback. (S1)
- **R10.** A patch whose blocks Beatlayer doesn't know (from a later version) keeps them and their cables, silent, and says so in its title bar. (Beatlayer's own)
- **R11.** A patch has at most 256 blocks and 1,024 cables. (Beatlayer's own)
- **R12.** Modular reports the latency its patch's blocks report, along the longest path from in to out, for delay compensation. (S2)
- **R13.** A Note In block gives each voice its pitch (as a control in octaves from C3), velocity, gate and per-note expressions; Note Out sends notes and expressions from a MIDI effect's patch. (S1)
- **R14.** Audio In gives the device's input, Audio Out takes its output; an effect with no cable to Audio Out is silent. Sidechain In gives the device's sidechain. (S1)
- **R15.** The device's CPU cost is measured per schedule and shown as its performance impact. (Beatlayer's own)
- **R16.** Patch parameters show in the device's control band in order, under the display cell holding the editor (R43). (S2)
- **R17.** A cable's value is the sum of its output; an unconnected input takes the block's own setting for it. (S1)
- **R18.** A control output feeding an audio input passes its value; an audio output feeding a control input passes its sample. (S1)
- **R19.** Each kind of block has a type ID of Beatlayer's own, and its settings their own IDs, which never change once released. (S1)
- **R20.** Moving or adding a block doesn't change what the patch sounds like, except the blocks it touches. (S1)

### The first blocks (WP-9.36)

- **R21.** **Oscillator**: sine, triangle, saw, square with pulse width, band-limited; frequency from a pitch input in octaves plus a coarse and fine setting; phase-modulation input; sync input. (S1)
- **R22.** **Filter**: low-pass, high-pass, band-pass and notch, 12 and 24 dB per octave, with cutoff (and a cutoff input in octaves) and resonance, sharing the code of Beatlayer's own devices' filters. (S1)
- **R23.** **Envelope**: attack, decay, sustain and release, triggered by a gate input, with a curve per segment. (S1)
- **R24.** **LFO**: sine, triangle, saw, square and random, at a rate in hertz or synced to the song's tempo, with a reset input. (S1)
- **R25.** **Amplifier**: an audio input times a control input times a gain in decibels. (S1)
- **R26.** **Mixer**: four inputs with a level each, summed. (S1)
- **R27.** **Arithmetic**: add, subtract, multiply, minimum, maximum, absolute value and a constant. (S1)
- **R28.** **Delay**: a delay of 1 sample to 4 seconds, with a time input; the only block that may close a cycle (R3). (S1, S3)
- **R29.** **Note In**, **Note Out**, **Audio In**, **Audio Out** and **Sidechain In** as R13 and R14 say. (S1)
- **R30.** Each block's rules have a test, sample-accurate at any block size. (S1)

### The other blocks (WP-9.37)

- **R31.** **Sample Player**: plays a sample from the patch's media, by gate and pitch, one-shot or looping, with start and length. (S1)
- **R32.** **Waveshaper**: soft clip, hard clip, fold and a drawn curve, with drive and bias. (S1)
- **R33.** **Noise**: white, pink and brown, from the device's seed, so renders repeat. (S1)
- **R34.** **Sample and Hold**: holds its input on each trigger. (S1)
- **R35.** **Slew**: limits how fast a control rises and falls, each in milliseconds. (S1)
- **R36.** **Comparator** and **Logic**: a gate when one input passes another; and, or, not and exclusive or of gates. (S1)
- **R37.** **Clock Divider**: a gate every *n* gates in, with a reset. (S1)
- **R38.** **Step Sequencer**: up to 32 steps of a value and a gate, advanced by a clock input or by the song's tempo at a division. (S1)
- **R39.** **Quantiser**: rounds a pitch to the nearest note of the clip's current scale ([scales](scales.md)) from the frame a clip launches, or to a scale the block sets. (S2)
- **R40.** **Effects**: Reverb, Delay, Chorus, Saturator and Compressor blocks, each sharing its code with Beatlayer's device of the same name and rendering as that device does with the same settings. (S1)
- **R41.** Each block's rules have a test, sample-accurate at any block size. (S1)
- **R42.** No block allocates, locks or blocks on the audio thread. (S1)

### The patch editor (WP-9.38)

- **R43.** The patch editor is drawn in Modular's display cell, which floats for room as any display cell does under IMP-14 ([device framework](device-framework.md)). It shows blocks as boxes with their ports on their edges, inputs left and outputs right, and cables as curves coloured by port type. (S2)
- **R44.** A double-click on empty space, or Return with the editor focused, opens a list of blocks to add, searched by typing; the block goes where the pointer was, or beside the selected block. (Beatlayer's own)
- **R45.** A drag from an output port to an input port connects them; a drag from a connected input moves its cable; Delete removes the selected blocks or cables. (Beatlayer's own)
- **R46.** Dragging a block moves it; its place is saved with the patch. (Beatlayer's own)
- **R47.** Each port shows its live value as a small meter while the device plays, from values the engine publishes. (Beatlayer's own)
- **R48.** With the keyboard: Tab moves between blocks, and within a block between its ports; with a port focused, Return opens a list of the ports it can connect to, by block and port name, and choosing one connects them. Arrow keys move the selected block by one grid step. (S2)
- **R49.** Every block, port and cable reaches assistive technology: a block as a group named by its type and name, a port as a button named by its name, type and what it's connected to, a cable by its two ends. (S2)
- **R50.** Every edit in the editor runs one of R9's commands and is one undo step, matching the agent commands' results. (S1)
- **R51.** The editor's layout (the blocks' places and its zoom) saves with the patch. (S1)
- **R52.** The editor redraws from the patch and the published values without waiting on the engine. (S1)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 to R30 | to be written in WP-9.36: `crates/devices/tests/modular.rs` at block sizes 1, 61 and 512 under the allocation trap; `crates/shell/tests/modular.rs`; `crates/agent/tests/modular.rs` | unit, engine test, agent API |
| R31 to R42 | to be written in WP-9.37 | unit |
| R43 to R52 | to be written in WP-9.38: `crates/ui/tests/modular_editor.rs` | UI script |

# The instruments' modulation matrix: sources, destinations, route slots, per-voice evaluation and the route commands

- **Status:** draft
- **Work packages:** WP-8.4 (this spec, R1 to R17, with Drift's Mod section moved onto it). Wavetable's sources, destinations and slot count are WP-8.10's ([wavetable](wavetable.md)), Meld's WP-8.20's ([meld](meld.md)); Operator's and Analog's routings are in their specs. How a parameter's base and its relative modulation make the value it plays is [modulation](modulation.md) R1 to R11, and the matrix's place after it is modulation R30 and R31; the per-voice sources are [the instruments' voice section](instrument-voices.md) R15
- **Parity-matrix row:** Instruments: Wavetable, Meld (their Matrix tabs); Drift (its Mod section); Operator and Analog (their modulation routings)
- **Checked against:** the Live 12 reference manual (online edition), read 30 October 2026 (chapter 31, sections 31.3.6, 31.8.6, 31.9.6, 31.9.10.2 and 31.13.5); the plug-in kit's `lspk-mod` at the revision Beatlayer pins

## The behaviour

Several of Live's instruments route their own sources to their own parameters inside the device: Wavetable's and Meld's Matrix tabs are grids of sources across and destinations down, where dragging in a cell sets how much a source moves a destination; Drift's Mod section has three slots of a source, a destination and an amount beside its dedicated routes; Operator and Analog send velocity, the key, the wheels and their LFOs to chosen destinations with amounts. Beatlayer gives them one matrix, built on the plug-in kit's modulation engine. It runs per voice: each note's envelopes, LFOs, velocity, key and MPE expressions move that note's copy of a parameter, frame by frame, without touching the other notes or the set.

Each device has a fixed number of route slots. A slot is fixed by the device type (Drift's Shape Mod), chosen by two of the device's own parameters (Drift's three slots, Operator's and Analog's routings), or free, set by the route commands and by the Matrix tab's grid (Wavetable, Meld). Every slot's amount is a parameter, so it automates, can be a modulator's target, and an agent reads and sets it like any other; a free slot's source and destination are device state, saved with the set and changed as one undo step each. A route moves its destination after automation and the set's modulators have made the value the parameter plays, and the sum is held inside the parameter's travel.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 31 "Live Instrument Reference", section 31.13.5 "Matrix Tab" (Wavetable) | Sources run across and targets down; dragging in the grid sets an amount; a target is additive (its sources are summed and added) or multiplicative (they multiply, 1 being neutral); a source is bipolar or unipolar; a Time slider scales every modulator's speed (negative faster) and an Amount slider scales all modulation |
| S2 | Manual | Section 31.8.6 "Matrix" (Meld) | Sources across, targets down, an amount dragged in a cell; negative amounts speed envelopes and LFOs up and positive ones slow them; some targets add and some multiply; clicking a parameter adds it to the matrix for the engine shown, and an unmodulated one leaves when another is chosen |
| S3 | Manual | Section 31.3.6 "Mod Section" (Drift) | Up to three sources and destinations with amounts from −100 % to 100 %, and the dedicated Shape Mod, Pitch Mods and Freq Mods |
| S4 | Manual | Section 31.9.6 "Global Controls" and 31.9.10.2 "Modulation Targets" (Operator) | Velocity, Key, Aftertouch, Pitch Bend and Mod Wheel each go to two destinations with amounts, from a list of targets |
| S5 | Beatlayer | The plug-in kit's `lspk-mod` (its `source`, `dest`, `matrix`, `program`, `engine` and `mailbox` modules) | A slot's value is the curved source times its amount; a unipolar route adds from the base upward and a bipolar one swings around it; amounts are fractions of the destination's travel; routes into one destination add and the sum is clamped; destinations declare an audio or control rate; compiled programs reach the audio thread through a mailbox |
| S6 | Beatlayer | [Modulation](modulation.md) R1 to R11; [the polysynth](polysynth.md) R19 to R23; [the improvements](../05-improvements.md), rule G2 | A parameter's base, relative modulation and the value it plays; Drift's slots, its sources' units, every amount a parameter; every parameter automatable and modulatable |
| S7 | Beatlayer | [The device framework](device-framework.md) R1, R3, R4 and R9; [the instruments' voice section](instrument-voices.md) R15, R19 and R20 | Fixed descriptors, smoothing, no allocation on the audio thread; the per-voice and global sources; Hi-Quality's rates |

## Rules

**Sources, destinations and slots**

- **R1.** A device declares its sources and its destinations once, in its type. A source has a name, a range (unipolar, 0 to 1, or bipolar, −1 to 1) and whether it is per voice or global: the voice section's (instrument voices R15) and the device's own envelopes, LFOs and other signals. A destination is one of the device's parameters, named by the parameter's name (with its engine's letter where a device has two engines, as Meld's "A Filter Frequency"), with a rate, audio (worked out every frame: pitch, phase and FM depth, amplitude, pan) or control (everything else), and a kind, additive or multiplicative. (S1, S2, S5)
- **R2.** A device has a fixed number of route slots, which its spec gives. Each slot is fixed (its source and destination set by the type), chosen (its source and destination set by two of the device's choice parameters) or free (set by R8 and R9). Every slot's amount is a parameter of the device, from −100 % to 100 % (0 % by default), automatable and modulatable like any other (rule G2), so the device's descriptors never change when a route does. (S3, S4, S5, S6)

**What a route does**

- **R3.** A route's contribution is its source's value times its amount. A unipolar source moves its destination from the destination's value upward for a positive amount and downward for a negative one; a bipolar source swings it both ways around that value. For an additive destination the contribution is a share of the destination's travel (0 to 1, logarithmic where its taper is), the contributions of every route into it add, and their sum moves the value's position on the travel. (S1, S5)
- **R4.** For a multiplicative destination each route gives a factor, 1 − a × (1 − s) for a positive amount a and 1 + a × s for a negative one, s being the source's 0-to-1 reading ((x + 1) / 2 for a bipolar source), never below 0, and the factors multiply the destination's value on its control's travel: a route at 0 % leaves it as it is, and at 100 % the source alone scales it from silence (s = 0) to its value (s = 1). Which destinations multiply is each device's spec's (Wavetable's Volume, Sustain, Amount and the LFOs' Amount). (S1, S2; the factor law is Beatlayer's own)
- **R5.** A route acts on the value the parameter plays ([modulation](modulation.md) R30): its base with the set's automation, macros and modulators already applied. The matrix's contributions move that value and the result is clamped to the travel's ends; a stepped parameter plays the step nearest. The matrix never writes into the set, and the value a parameter plays without its routes is what automation lanes, `device.parameters` and `modulation.state` report as its value. (S5, S6)
- **R6.** A per-voice destination moves each voice by that voice's own sources, so one note's velocity, envelope or pressure never moves another note's copy. A global destination (one the device runs once for all voices, as its output Volume) reads a per-voice source from the oldest voice still sounding, or 0 when none sounds; a global source moves every voice alike. (S5, S6; the oldest voice is Drift's reading, polysynth R19)
- **R7.** An audio-rate destination's routes are worked out every frame. A control-rate destination's routes are worked out every frame while the device's Hi-Quality is on or the device has none, and otherwise once every 32 frames, on frames counted from the moment the device's node was made, so the result is the same at any block size (instrument voices R20). (S1, S5, S7)

**Changing routes**

- **R8.** `device.set_route` (`edit`) sets a free slot: by its `slot` number, or, without one, the free slot that already routes that `source` to that `destination`, else the first empty free slot. It takes the `source` and `destination` by name and an optional `amount`, and is one undo step, Change Modulation Route. It's refused for a fixed or chosen slot, an unknown source or destination, a source and destination the device doesn't allow together, or when no free slot is empty. (S1, S2; the command's names are Beatlayer's own)
- **R9.** `device.clear_route` (`edit`) empties a free slot, by `slot` or by its `source` and `destination`, and sets its amount to 0 %, as one undo step, Clear Modulation Route. Undoing a route change brings back the slot's source, destination and amount together. (S2; Beatlayer's own)
- **R10.** A free slot's source and destination are saved with the set and with the device's presets. A change is compiled on the main thread and reaches the audio thread through the kit's mailbox; the audio thread swaps the program at a block's first frame and frees nothing, so changing routes while the device plays allocates, locks and blocks nothing there. A chosen slot's choosers are parameters and change on their frame like any parameter. (S5, S7)
- **R11.** The query tool's `device` result gains `matrix`: the device's sources (name, range, per voice or global) and destinations (parameter ID and name, rate, kind), and every slot with its number, kind, source, destination and its amount's parameter ID and value. It's the readback of R8 and R9. (S5)

**Shown and spoken**

- **R12.** A control whose parameter the matrix moves shows it as [modulation](modulation.md) R31 says, and its accessibility value names the routes into it. (S6)

**Throughout**

- **R13.** The device's slots, programs and per-voice evaluation are made with the device: nothing the matrix does on the audio thread allocates, locks or blocks, and its output for a frame depends only on its sources, its amounts and its program, never on how blocks fall. (S5, S7)
- **R14.** A route has no curve, auxiliary source or output scaler of its own unless a device's spec gives it one: of the kit's engine, a device exposes only what its manual describes. (S5)
- **R15.** A device whose manual has a matrix Time control (Wavetable) scales the rates of every envelope and LFO the matrix reads by 2^(−2t) at Time t from −100 % to 100 % (negative faster), and a matrix Amount control scales every route's contribution by its value, as a multiplicative factor on the amounts (Beatlayer's own laws). (S1, S2)
- **R16.** Every parameter the manual lets a device's matrix reach is one of its destinations, and every source the manual lists is one of its sources, each listed in the device's spec. (S1, S2, S3, S4)
- **R17.** Drift's three Mod slots are chosen slots, its Shape Mod, Pitch Mods and Freq Mods and the LFO's own rate slot fixed slots, all on the shared matrix with [polysynth](polysynth.md) R19 to R23 unchanged: a Drift set renders as it did, sample for sample. (S3, S6)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R2, R16 | to be written in WP-8.4 | unit (a test instrument's sources, destinations and slots; descriptors fixed) |
| R3, R4, R5 | to be written in WP-8.4 | render under the allocation trap (velocity to cutoff per voice; matrix on top of automation and a Phase 5 LFO; clamping) |
| R6 | to be written in WP-8.4 | render under the allocation trap |
| R7 | to be written in WP-8.4 | render at block sizes 1, 64, 512 and 4,096 |
| R8, R9, R10, R11 | to be written in WP-8.4 | shell test (undo steps, save and reopen); agent API (set and clear a route by ID, read `matrix` back); `cargo xtask commands` |
| R12 | to be written in WP-8.4 | UI script |
| R13 | to be written in WP-8.4 | real-time trap; RealtimeSanitizer (`rtsan/tests`) |
| R14, R15 | to be written in WP-8.4 | unit |
| R17 | to be written in WP-8.4 | golden render (Drift unchanged) |

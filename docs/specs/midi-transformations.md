# MIDI Tools: the transformations

- **Status:** partial (R1 to R22, and R43 for Chop's toggles, each have a passing test; R23 to R42, and R43 for the other tools' displays, are WP-6.13's and WP-6.13b's)
- **Work packages:** WP-6.12 (what every transformation shares, Arpeggiate, Chop, Connect, Ornament and Quantize: R1 to R22); WP-6.13 (Recombine, Span, Strum, Time Warp and Velocity Shaper: R23 to R35); WP-6.13b (the MPE transformations, Glissando and LFO: R36 to R42); R43, the tools' displays, is built by each of the three packages for its own tools. The Transform panel, choosing a tool, what a tool acts on, applying, undo, seeds and chains are [midi-tools.md](midi-tools.md)'s (WP-6.11, WP-6.15)
- **Parity-matrix row:** MIDI editing: "MIDI Tools, transformations (Arpeggiate, Chop, Connect, Glissando, LFO, Ornament, Quantize, Recombine, Span, Strum, Time Warp, Velocity Shaper)"
- **Checked against:** the Live 12 reference manual (online edition), read 20 October 2026: chapter 11, sections 11.2.1 to 11.2.12, chapter 10, section 10.5.11, and chapter 8, section 8.6; the Live 12 release notes, 12.0.5 and 12.1; the UI parity reference's M-EDIT-035, M-EDIT-036, S-NOTE-011, S-NOTE-012 and S-SAMPLE-002

## The behaviour

A transformation takes notes and changes them. Arpeggiate turns held notes into arpeggios, with Arpeggiator's styles. Chop cuts each note into parts, some of them gaps, some stretched. Connect fills the gaps between notes with new ones that wander from one pitch to the next. Ornament puts a flam or grace notes before each note. Quantize moves notes onto the grid. Recombine swaps one property among the notes: their places, pitches, lengths or velocities. Span makes notes legato, tenuto or staccato. Strum spreads the notes of a chord in time. Time Warp speeds time up or slows it down across the range along a curve. Velocity Shaper draws the notes' velocities from an envelope. Glissando bends each note's pitch into the next, and LFO writes an oscillating pitch bend, slide or pressure over each note; both write per-note expressions.

Every tool here is applied as [midi-tools.md](midi-tools.md) says: to the selected notes, the notes in the time selection, or the notes in the loop, with Auto applying each change at once, and one undo step for each application. Where the clip has a scale, the parameters marked scale-aware count scale degrees. Each tool's parameters, ranges and defaults are below; where the manual gives none, they are Beatlayer's own, and so is every law the manual describes only in words.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 11 "MIDI Tools", section 11.2.1 "Arpeggiate" | Arpeggiate turns the selected notes into arpeggios with the core of the Arpeggiator device. Style chooses one of Arpeggiator's 18 patterns; Distance sets how far the pattern's steps are transposed, in scale degrees with a clip scale and semitones without; Steps sets how many transposed steps there are; Rate sets the pattern's rate, which also sets note length; Gate below 100 % shortens notes and above lengthens them |
| S2 | Manual | Same chapter, section 11.2.2 "Chop" | Chop cuts the selected notes into 2 to 64 parts each, the pattern repeating beyond 16. Gaps adds silences: a positive value puts a gap after that many notes, a negative one that many gaps after each note, its range depending on Parts. The Pattern toggles show the pattern and add or remove gaps by hand, and Gaps then shows a dash until it's set again. Emphasis toggles mark notes or gaps that Stretch Chunk(s) makes 2 to 8 times as long, and their Pattern toggles are unavailable. Variation moves the parts' starts and ends at random |
| S3 | Manual | Same chapter, section 11.2.3 "Connect" | Connect adds notes in the gaps between notes, placed at random within its settings. Spread is the most the new notes' pitches move at random from the original notes', in scale degrees or semitones; Density is the share of the time between notes that new notes fill, every gap at 100 %; Rate is the new notes' length; Tie is the chance that a new note lasts to the next original note |
| S4 | Manual | Same chapter, section 11.2.4 "Glissando" | Glissando bends each selected note's pitch along a curve into the next note's, and needs two notes or more. Start sets where the bend begins, as a share of the note's length; Curve sets its shape. In the editor, dragging the curve's breakpoint sideways sets Start and dragging the line up or down sets Curve. The result is pitch bend, shown in the MPE view |
| S5 | Manual | Same chapter, section 11.2.5 "LFO" | LFO, an MPE transformation, writes a low-frequency oscillation to one target: Pitch Bend, Slide or Pressure. Attack and Decay shape its amplitude over the oscillation's length and limit each other (Attack at 100 % sets Decay to 0 %). Shape adjusts the wave the Shape Type chooser gives: Sine, Square, Triangle or Random, with a Reseed button for Random. Rate is the period, from 1 to 1/128, 1 being four beats. Time Shift delays the start for positive values and moves the phase for negative ones. Amount reaches twice the clip's pitch-bend range for Pitch Bend and −127 to 127 for Slide and Pressure; Amplitude Base is the level it moves around, within the pitch-bend range or −127 to 127 |
| S6 | Manual | Same chapter, section 11.2.6 "Ornament" | Ornament adds a flam or grace notes at the start of each selected note, and applying it again adds more. Flam Position is a share of the grid step: positive takes over the note's start, negative puts the flam before it, and ±100 % fills a grid step; Flam Velocity is relative to the note's. Grace Notes adds equal-length notes before each note: Pitch High puts every other grace note a semitone or scale degree above the note, Low below, Same at its pitch; Position works as Flam Position; Velocity is relative to the note's; Chance relative to the note's chance; Amount is how many grace notes each note gets |
| S7 | Manual | Same chapter, section 11.2.7 "Quantize"; chapter 10, section 10.5.11 "Quantizing Notes"; chapter 8, section 8.6 "Transform and Generate Panels" | Quantize moves notes, or stretches them, to the grid or a chosen division, triplets included: their starts, their ends, or both. Amount moves them part of the way. Quantize Settings… in the Edit menu, with its key, shows the Quantize tool; Cmd+U (Ctrl+U) quantises with the Quantize tool's settings. An audio clip's Transform panel holds Quantize, which moves warp markers by Amount |
| S8 | Manual | Same chapter, section 11.2.8 "Recombine" | Recombine rearranges one property among the selected notes, chosen by the Dimension chooser: Position, Pitch, Duration or Velocity. Shuffle puts the values in a random order, anew each time the tool is applied; Mirror reverses their order; Rotate moves them round by Rotation Steps, at most one fewer than the notes, set by dragging the display's columns or with the step buttons, positive and negative turning opposite ways. They apply in that order. Rotate on Grid, for Position, counts the grid's cells in the selection instead of the notes |
| S9 | Manual | Same chapter, section 11.2.9 "Span" | Span sets the selected notes' lengths by articulation. Legato makes each note last to the next note's start, the last to the time selection's end or the loop's end; Tenuto keeps lengths; Staccato halves the smallest gap between the notes' starts and gives every note that length. Offset moves ends by up to a grid step, positive lengthening; Variation changes lengths at random, positive shortening and negative lengthening, anew each time the tool is applied |
| S10 | Manual | Same chapter, section 11.2.10 "Strum" | Strum moves the start times of a chord's notes. Strum Low offsets them counting from the lowest note, Strum High from the highest, positive later; at ±100 % the lowest note moves by up to a grid step, and the others in proportion along the Strum Position display's curve, whose breakpoints set both. At least one of them must be away from 0 %. Tension at 0 % spaces the notes evenly; positive makes the gaps widest at the start and shrinking, negative the opposite |
| S11 | Manual | Same chapter, section 11.2.11 "Time Warp" | Time Warp moves the selected notes along a speed curve, for accelerando and ritardando, the curve's time spanning the time selection. One to three breakpoints, each with a switch, set the curve, dragged in the display or set with the Breakpoint Time and Breakpoint Speed fields. Quantize puts the warped notes on the grid; Preserve Time Range keeps them within the selection's time; Include Note End warps the notes' ends too, changing their lengths |
| S12 | Manual | Same chapter, section 11.2.12 "Velocity Shaper" | Velocity Shaper sets velocities from an envelope drawn in its display, clicking to add breakpoints and dragging them. Minimum Velocity and Maximum Velocity set the output's range; Loop how many times the envelope repeats across the selection; Rotate shifts it by steps of Division ("Grid" or a division) from the selection's start |
| S13 | Release notes | Live 12 release notes, 12.0.5 | Arpeggiate, Connect and Ornament give their new notes the original notes' velocity and chance deviation. Strum's Tension is set by dragging in its display; Time Warp's Breakpoint Time and Speed take typed values; Arpeggiate's and Connect's Rate take typed values. When quantising note ends, a note whose nearest line is at or before its start goes to the next line |
| S14 | Release notes | Live 12 release notes, 12.1 | Chop, Glissando and LFO are added; Glissando also joins notes that start together into the next group. Recombine changes one property at a time, rotates positions, and has step buttons. Arpeggiate moves notes past the edge of the key range back by an octave, with or without a scale. Quantize has a Triplets control. Time Warp takes notes' MPE data with them |
| S15 | Reference | [menus.md](../ui-parity/menus.md) M-EDIT-035 (Quantize, ⌘U) and M-EDIT-036 (Quantize Settings..., ⇧⌘U); [shortcuts.toml](../ui-parity/shortcuts.toml) S-NOTE-011, S-NOTE-012 and S-SAMPLE-002 | Cmd+U quantises; Cmd+Shift+U opens the quantise settings, in the note editor and the Sample Editor |
| S16 | Beatlayer | [midi-tools.md](midi-tools.md) R5, R8, R9, R13, R14, R16, R19 and R20; [arpeggiator.md](arpeggiator.md) R1 and its ranges; [midi-chord.md](midi-chord.md) (Strum's Tension law); [piano-roll.md](piano-roll.md) R3, R18 and R19; [warping.md](warping.md) R49 and R50; [scales.md](scales.md) R2; [midi-controller-data.md](midi-controller-data.md) R2 | The framework every tool here sits in; Arpeggiator's styles and ranges; the strum law Beatlayer already uses; the notes' order, quantise and its settings; audio quantise; how a key moves by degrees; a note's pressure points |
| S17 | Beatlayer | [mpe.md](mpe.md) R1, R2, R4, R22 and R24 | A note's Pitch, Slide and Pressure envelopes, their ranges (Pitch ±48 semitones, Slide and Pressure 0 to 127) and breakpoints; how edits carry them; the MPE tab, where Pitch is drawn over the notes |

## Rules

**What the transformations share (WP-6.12)**

- **R1.** A parameter of the note-division kind offers 1, 1/2, 1/2T, 1/4, 1/4T, 1/8, 1/8T, 1/16, 1/16T, 1/32, 1/32T, 1/64 and 1/128, in that order (Beatlayer's own list), where 1 is a whole note, four quarter notes (2,822,400 ticks), 1/n is a whole note divided by n, and a T value is a triplet, two-thirds as long. (S5, S16)
- **R2.** In these rules the grid step is the one [midi-tools.md](midi-tools.md) R16 names; a chord is the input notes that start on the same tick (Beatlayer's own); a step of a scale-aware parameter moves a key as [midi-tools.md](midi-tools.md) R8 and R9 say; percentages are whole numbers; a length or place worked out from a fraction is rounded to the nearest tick, half up, and every note keeps at least a tick; and "random" means drawn from the application's seed ([midi-tools.md](midi-tools.md) R20). A tool's ID in the commands is its name in lower case, words joined by underscores (`arpeggiate`, `time_warp`, `velocity_shaper`), and so is each parameter's (`stretch_chunks` for Stretch Chunk(s)). (S16)

**Arpeggiate (WP-6.12)**

- **R3.** Arpeggiate's parameters (the ranges are Arpeggiator's, [arpeggiator.md](arpeggiator.md); Steps' and Rate's defaults are Beatlayer's own): (S1, S16)

  | Parameter | Kind and range | Default |
  |---|---|---|
  | Style | a choice of Arpeggiator's 18 styles, in [arpeggiator.md](arpeggiator.md) R1's order | Up |
  | Distance | a whole number, −48 to +48, scale-aware | +12 |
  | Steps | a whole number, 0 to 8 | 0 |
  | Rate | a note division (R1) | 1/16 |
  | Gate | 1 % to 200 % | 100 % |

- **R4.** Arpeggiate splits its input into groups: taking the notes in the clip's order ([piano-roll.md](piano-roll.md) R3), a note joins the group before it when it starts before that group's latest end, and otherwise starts a group. Each group plays an arpeggio from its earliest start, a step every Rate, the last step before its latest end. At a step the held notes are the group's notes that sound then: those that start at or before it and end after it. A step with no note held writes nothing. (S1; how the input is held is Beatlayer's own)
- **R5.** At each step the held notes, lowest key first (for Play Order, earliest start first, then lowest key), are ordered as Style orders an Arpeggiator's held notes ([arpeggiator.md](arpeggiator.md) R1), and the step writes the note at the arpeggio's place in that order, the place moving on by one each step; when the held notes change, the order is built again and the place carries on, counted round the new order. Chord Trigger writes every held note at each step; Random a random held note each step; Random Other a random held note other than the step before's while more than one is held; Random Once an order drawn at random once for the group and kept. Each time the place goes round the order, the cycle count rises by one, from 0 up to Steps, then back to 0, and a step's note is its held note moved by the cycle count times Distance, in steps (R2). (S1, S16)
- **R6.** Each note Arpeggiate writes starts on its step and lasts Rate × Gate, so a Gate above 100 % overlaps the next step's note. It takes its held note's velocity and the values [midi-tools.md](midi-tools.md) R14 lists. A key moved below 0 or above 127 moves back by octaves until it is inside. The notes it writes replace its input. (S1, S13, S14)

**Chop (WP-6.12)**

- **R7.** Chop's parameters, where P is Parts or 16, whichever is smaller (Gaps' range and the defaults are Beatlayer's own): (S2)

  | Parameter | Kind and range | Default |
  |---|---|---|
  | Parts | a whole number, 2 to 64 | 2 |
  | Gaps | a whole number, −(P − 1) to P − 1 | 0 |
  | Pattern | P toggles, each a part (on) or a gap (off) | as Gaps writes it |
  | Emphasis | P toggles | all off |
  | Stretch Chunk(s) | a whole number, 2 to 8 | 2 |
  | Variation | 0 % to 100 % | 0 % |

- **R8.** The pattern is P elements, each a part or a gap. Setting Gaps writes them: 0 makes every element a part; a positive g makes every (g + 1)th element a gap, so g parts come before each gap; a negative −g makes one part followed by g gaps, repeated. Clicking a Pattern toggle switches its element between part and gap; Gaps then shows a dash and holds no number until it is set again, which writes the pattern afresh. An element with Emphasis on keeps its Pattern state, and its Pattern toggle is unavailable. The pattern's last part can't be switched to a gap (Beatlayer's own). (S2)
- **R9.** Chop cuts each input note into Parts pieces in order; piece j takes the pattern's element j modulo P, so the pattern repeats past 16 parts. A piece's length is in proportion to its weight, Stretch Chunk(s) for an element with Emphasis on and 1 for the rest, and the pieces together fill the note exactly, each boundary rounded from the note's start. A part becomes a note at the input note's key, with its velocity and the values [midi-tools.md](midi-tools.md) R14 lists; a gap writes nothing; a piece shorter than a tick is left out. (S2)
- **R10.** With Variation v, each note Chop writes has its start and its end each moved by a random amount up to v × half its piece's length, earlier or later, staying within its input note and keeping at least a tick (the law is Beatlayer's own). (S2)

**Connect (WP-6.12)**

- **R11.** Connect's parameters (the ranges besides Density's and the defaults are Beatlayer's own): (S3)

  | Parameter | Kind and range | Default |
  |---|---|---|
  | Spread | a whole number, 0 to 12, scale-aware | 0 |
  | Density | 0 % to 100 % | 100 % |
  | Rate | a note division (R1) | 1/16 |
  | Tie | 0 % to 100 % | 0 % |

- **R12.** Connect keeps its input notes and adds notes in the gaps between them. Taking the input's different start times in order, the gap after a start s runs from the latest end of the notes that start at s to the next start; a gap with no time in it gets nothing. A gap holds as many slots as whole Rates fit in it, each a Rate long, from the gap's start, and Density chooses that many slots times Density, rounded, at random, to fill. (S3; the gaps and slots are Beatlayer's own reading)
- **R13.** A filled slot's key is the key that a straight line from the highest note starting at s to the highest note starting at the next start reaches at the slot's middle, counted in steps (R2) and rounded to the nearest step, then moved by a random whole number of steps from −Spread to +Spread. Its note lasts the slot, or, with chance Tie, until the next start, and then the gap's later slots stay empty. It takes the velocity of the highest note starting at s and the values [midi-tools.md](midi-tools.md) R14 lists. (S3, S13)

**Ornament (WP-6.12)**

- **R14.** Ornament's parameters (the ranges besides the positions' and every default are Beatlayer's own): (S6)

  | Parameter | Kind and range | Default |
  |---|---|---|
  | Mode | a choice: Flam, Grace Notes | Flam |
  | Flam Position | −100 % to +100 % of the grid step | −50 % |
  | Flam Velocity | 1 % to 100 % of the note's velocity | 50 % |
  | Grace Notes Pitch | a choice: High, Low, Same | High |
  | Grace Notes Position | −100 % to +100 % of the grid step | −50 % |
  | Grace Notes Velocity | 1 % to 100 % of the note's velocity | 50 % |
  | Grace Notes Chance | 0 % to 100 % of the note's chance | 100 % |
  | Grace Notes Amount | a whole number, 1 to 8 | 2 |

- **R15.** In Flam mode each input note gets one flam at its key, d long, d being |Flam Position| × the grid step. With a negative position the flam ends where the note starts. With a positive one the flam starts where the note started, and the note now starts d later, its end staying put; d is then at most the note's length less a tick. The flam's velocity is the note's times Flam Velocity, at least 1, and it takes the values [midi-tools.md](midi-tools.md) R14 lists. A position of 0 % adds nothing. (S6)
- **R16.** In Grace Notes mode each input note gets Amount grace notes of equal length that together fill d, d being |Grace Notes Position| × the grid step, placed before the note or over its start as R15 places a flam. Counting back from the note, the grace note next to it is a step (R2) above the note's key with High, a step below with Low and at the key with Same; with High and Low the grace notes before it alternate between the note's key and that step. Each grace note's velocity is the note's times Grace Notes Velocity, at least 1, and its chance the note's times Grace Notes Chance. (S6; which grace note takes the step is Beatlayer's own)
- **R17.** Ornament keeps its input notes, moved only by a positive position, and adds its ornaments. Applying it again ornaments the notes as they are then, earlier ornaments included ([midi-tools.md](midi-tools.md) R19), so more flams or grace notes build up. (S6)

**Quantize (WP-6.12)**

- **R18.** Quantize's parameters (Grid's list and the defaults besides Amount's are Beatlayer's own; Amount's is [piano-roll.md](piano-roll.md) R19's): (S7, S14)

  | Parameter | Kind and range | Default |
  |---|---|---|
  | Start | a switch | on |
  | End | a switch | off |
  | Grid | a choice: Current Grid, 1, 1/2, 1/4, 1/8, 1/16, 1/32 | Current Grid |
  | Triplets | a switch, unavailable with Current Grid | off |
  | Amount | 0 % to 100 % | 100 % |

- **R19.** Quantize moves each input note toward the lines of its grid (the grid step with Current Grid; otherwise the division chosen, two-thirds as long with Triplets on), laid out from each bar's start as [piano-roll.md](piano-roll.md) R18 lays them. With Start on, the note moves Amount of the way from its start to its start's nearest line, keeping its length. With End on, its end then moves Amount of the way to its end's nearest line, its start staying put; where that line is at or before the note's start, the end goes toward the first line after the start instead. With both off it changes nothing. (S7, S13)
- **R20.** The Quantize tool's settings are the Clip View's Quantize settings: `clip_view.quantize_settings` reads and sets them, its `target` being `note_start`, `note_end` or `both`, with `triplets` beside `grid` and `amount`; Cmd+U (S-NOTE-011; Edit ▸ Quantize, M-EDIT-035) quantises the selected notes with them through `note.quantize`; and, like every tool's settings, they are saved with the set ([midi-tools.md](midi-tools.md) R5). (S7, S15, S16)
- **R21.** Cmd+Shift+U (S-NOTE-012) and Edit ▸ Quantize Settings... (M-EDIT-036), with the note editor focused, show the Transform panel with Quantize chosen; with the Sample Editor focused (S-SAMPLE-002), the audio clip's Transform panel. Neither changes notes or is an undo step. (S7, S15)
- **R22.** An audio clip's Transform panel holds Quantize with its Grid, Triplets and Amount, without Start, End or an Auto switch (Beatlayer's own). Its apply button and Cmd+Enter run `clip.quantize_audio` with those settings ([warping.md](warping.md) R49, R50), as Cmd+U does in the Sample Editor. (S7, S16)

**Recombine (WP-6.13)**

- **R23.** Recombine's parameters, where n is the number of input notes or, with Rotate on Grid on, the number of grid steps in the range (the defaults are Beatlayer's own). Rotate Step Down and Rotate Step Up lower and raise Rotation Steps by one, and dragging across the display's columns sets it: (S8, S14)

  | Parameter | Kind and range | Default |
  |---|---|---|
  | Dimension | a choice: Position, Pitch, Duration, Velocity | Position |
  | Shuffle | a switch | off |
  | Mirror | a switch | off |
  | Rotation Steps | a whole number, −(n − 1) to n − 1 | 0 |
  | Rotate on Grid | a switch, available with Position only | off |

- **R24.** Recombine takes its input in the clip's order ([piano-roll.md](piano-roll.md) R3) and makes a list of each note's chosen value: its start for Position, its key for Pitch, its length for Duration, its velocity for Velocity. Shuffle puts the list in a random order; then Mirror reverses it; then Rotation Steps r moves each value r places later in the list, going round from the end to the start, and a negative r earlier. The first note then takes the list's first value, the second its second, and so on, each note keeping its other values. Each application with Shuffle on shuffles afresh ([midi-tools.md](midi-tools.md) R20). (S8; which way a positive rotation turns is Beatlayer's own)
- **R25.** With Rotate on Grid on and Position chosen, a rotation moves every input note's start by Rotation Steps grid steps, later for positive, a start that passes the range's end going round to its start and one that passes its start round to its end; Shuffle and Mirror still rearrange the starts first. (S8)

**Span (WP-6.13)**

- **R26.** Span's parameters (Offset's and Variation's ranges and the defaults are Beatlayer's own): (S9)

  | Parameter | Kind and range | Default |
  |---|---|---|
  | Mode | a choice: Legato, Tenuto, Staccato | Legato |
  | Offset | −100 % to +100 % of the grid step | 0 % |
  | Variation | −100 % to +100 % | 0 % |

- **R27.** Legato ends each input note at the input's next later start; the notes with the latest start end at the range's end, or keep their length when they start at or after it. Tenuto keeps every note's length. Staccato takes the smallest gap between two of the input's different starts next to each other, halves it, and gives every input note that length; with fewer than two different starts, the lengths stay. Then Offset moves every note's end by Offset × the grid step, later for positive. (S9; notes past the range's end are Beatlayer's own case)
- **R28.** Then, with Variation v, each note's length is multiplied by 1 − v × u, u a random number from 0 to 1 drawn for each note, so a positive v shortens notes by up to v of their length and a negative one lengthens them by up to |v| (the law is Beatlayer's own). Span never moves a start. (S9)

**Strum (WP-6.13)**

- **R29.** Strum's parameters (the defaults are Beatlayer's own). The Strum Position display draws the offsets as a curve from Strum Low's breakpoint, at the lowest note, to Strum High's, at the highest; dragging a breakpoint up or down sets its value, and dragging the curve sets Tension: (S10, S13)

  | Parameter | Kind and range | Default |
  |---|---|---|
  | Strum Low | −100 % to +100 % | 0 % |
  | Strum High | −100 % to +100 % | 0 % |
  | Tension | −100 % to +100 % | 0 % |

- **R30.** In each chord of k notes (R2), the note j places above the lowest, j from 0 to k − 1, has its start moved by g × (L + (H − L) × (1 − (1 − j ÷ (k − 1))^q)), later for positive, where g is the grid step, L and H are Strum Low and Strum High as fractions from −1 to 1, and q is 2 to the power of 2 × Tension, Tension as a fraction from −1 to 1. So at 100 % the lowest note (with Strum Low) or the highest (with Strum High) moves a whole grid step, and positive Tension spaces the notes widest at the start. Each note's end stays put, a start never comes closer than a tick to its end or goes before the contents' start, and a note alone at its start doesn't move. (S10; the law is Beatlayer's own, the one [midi-chord.md](midi-chord.md)'s Strum uses)

**Time Warp (WP-6.13)**

- **R31.** Time Warp's parameters (Speed's range and the defaults are Beatlayer's own). At least one breakpoint stays on; Breakpoint Time and Breakpoint Speed show and set the selected breakpoint's values and take typed values, and dragging a breakpoint in the display sets both: (S11, S13)

  | Parameter | Kind and range | Default |
  |---|---|---|
  | Breakpoints 1, 2 and 3 | each a switch, a Time from 0 % to 100 % of the range and a Speed from 25 % to 400 % | 1: on, 0 %, 100 %; 2: on, 100 %, 100 %; 3: off, 50 %, 100 % |
  | Quantize | a switch | off |
  | Preserve Time Range | a switch | on |
  | Include Note End | a switch | off |

- **R32.** Time Warp's speed at a point of the range follows straight lines between the switched-on breakpoints in order of Time, level before the first and after the last. A point a fraction u of the way through the range moves to the fraction U(u), the integral from 0 to u of 100 % divided by the speed, so where the speed is above 100 % notes draw closer together. With Preserve Time Range on, U(u) is divided by U(1), so the range's end stays put. Each input note's start moves to its new place; with Include Note End on, its end moves the same way, and otherwise the note keeps its length. A note outside the range takes the level speed beyond the nearest breakpoint. (S11; the law is Beatlayer's own)
- **R33.** With Quantize on, each start Time Warp moves, and each end with Include Note End on, then lands on the nearest line of the grid step, as R19 does at 100 %. (S11)

**Velocity Shaper (WP-6.13)**

- **R34.** Velocity Shaper's parameters (the ranges and defaults are Beatlayer's own). Clicking the envelope display adds a breakpoint and dragging one moves it: (S12)

  | Parameter | Kind and range | Default |
  |---|---|---|
  | Envelope | 2 to 32 breakpoints, each a time and a value from 0 to 1, the first at time 0 and the last at time 1 | (0, 0) and (1, 1) |
  | Minimum Velocity | 1 to 127 | 1 |
  | Maximum Velocity | 1 to 127 | 127 |
  | Loop | a whole number, 1 to 16 | 1 |
  | Rotate | a whole number, −16 to +16 | 0 |
  | Division | a choice: Grid, then the note divisions (R1) | Grid |

- **R35.** Each input note's velocity becomes Minimum + e × (Maximum − Minimum), rounded to the nearest whole number and held within 1 to 127, where e is the envelope's value, along straight lines between its breakpoints, at the note's phase. The phase is (the note's start − the range's start − Rotate × the Division's length) ÷ (the range's length ÷ Loop), with its whole part dropped, so a negative phase wraps round; Grid's length is the grid step. A Minimum above the Maximum turns the shape upside down. Nothing else about the notes changes. (S12)

**Glissando (WP-6.13b)**

- **R36.** Glissando's parameters (the ranges besides Start's and the defaults are Beatlayer's own). In the Clip View's MPE tab ([mpe.md](mpe.md) R22, R24), while Glissando is the shown tool, dragging the breakpoint at its curve's start sideways sets Start, and dragging the curve up or down sets Curve: (S4)

  | Parameter | Kind and range | Default |
  |---|---|---|
  | Start | 0 % to 100 % of the note's length | 50 % |
  | Curve | −100 % to +100 % | 0 % |

- **R37.** Glissando needs at least two input notes, and with fewer is refused, saying why. It takes the input's chords (R2), a note alone at its start being a chord of one, in order of their starts, and pairs each note of a chord with a note of the next chord by rank from the lowest; a chord's extra notes pair with the next chord's highest (Beatlayer's own). Each note in a pair gets a Pitch envelope ([mpe.md](mpe.md) R1): 0 from its start to Start × its length, then a curve to the difference between its key and its partner's, in semitones, reached at its end. The last chord's notes get none. Keys, starts and lengths don't change. (S4, S14)
- **R38.** The curve's share of the difference at a fraction x of the way from Start to the note's end is x to the power of 2^(−2c), c being Curve as a fraction from −1 to 1, so a positive Curve gets there sooner (the law is Beatlayer's own). It is written as straight segments between breakpoints at the note's start, at Start, and at 16 points evenly spaced after it (Beatlayer's own), replacing the note's Pitch envelope, and a bend beyond ±48 semitones ([mpe.md](mpe.md) R1) is held at ±48. (S4, S17)

**LFO (WP-6.13b)**

- **R39.** LFO's parameters, where R is 48 semitones, the per-note pitch range of [mpe.md](mpe.md) R1 (the defaults are Beatlayer's own). Attack and Decay together are at most 100 %: setting one past what the other leaves lowers the other. Switching Target keeps Amount's and Amplitude Base's numbers, held within the new ranges. The Reseed button, shown with Random, runs `midi_tool.reseed` ([midi-tools.md](midi-tools.md) R20): (S5)

  | Parameter | Kind and range | Default |
  |---|---|---|
  | Target | a choice: Pitch Bend, Slide, Pressure | Pitch Bend |
  | Shape Type | a choice: Sine, Square, Triangle, Random | Sine |
  | Shape | −100 % to +100 % | 0 % |
  | Rate | a choice: 1, 1/2, 1/4, 1/8, 1/16, 1/32, 1/64, 1/128 | 1/8 |
  | Time Shift | −100 % to +100 % of a period | 0 % |
  | Amount | with Pitch Bend, −2R to +2R semitones; with Slide and Pressure, −127 to 127 | 0 |
  | Amplitude Base | with Pitch Bend, −R to +R semitones; with Slide and Pressure, −127 to 127 | 0 |
  | Attack | 0 % to 100 % of the note's length | 0 % |
  | Decay | 0 % to 100 % of the note's length | 0 % |

- **R40.** For each input note of length T, LFO writes its target's envelope over the note ([mpe.md](mpe.md) R1), replacing the one the note had: at a time t from the note's start the value is Amplitude Base + Amount × a(t) × w(t), held within the target's range (−R to +R semitones for Pitch Bend; 0 to 127 for Slide and Pressure). a(t) rises from 0 to 1 over the note's first Attack × T, stays at 1, and falls to 0 over its last Decay × T. w(t) is the wave, from −1 to 1, with Rate's length as its period P: with a Time Shift s of 0 % or more it is 0 until s × P and starts its cycle there; with a negative s it starts at once, |s| of the way through its cycle. (S5; the laws are Beatlayer's own)
- **R41.** At a point x of the way through its cycle, from 0 to 1, with m = (1 + Shape) ÷ 2 held within 0.05 to 0.95 and Shape as a fraction from −1 to 1: Square is 1 for x below m and −1 after; Sine is sin(2πy) and Triangle rises from −1 at y = 0 to 1 at y = ½ and falls back to −1 at y = 1, where y = x ÷ (2m) for x below m and ½ + (x − m) ÷ (2 × (1 − m)) after; Random holds a random value from −1 to 1 for each cycle and moves in a straight line to the next cycle's over the cycle's last 1 − m. (S5; Beatlayer's own)
- **R42.** LFO writes the envelope as breakpoints, 32 a period, and one at the note's start and one at its end, at most 512 a note (Beatlayer's own), each segment straight. Pitch Bend writes the note's Pitch envelope, Slide its Slide envelope and Pressure its Pressure envelope ([mpe.md](mpe.md) R1, R2). (S5, S17)

**Displays (WP-6.12, WP-6.13 and WP-6.13b, each for its own tools)**

- **R43.** Besides its parameters' controls, a tool's panel draws Chop's Pattern and Emphasis toggles, Recombine's display of the chosen values as columns, Strum's Strum Position display, Time Warp's speed curve with its breakpoints, Velocity Shaper's envelope and LFO's curve, each from the tool's settings and each as the rules above say a gesture on it sets them (the look is Beatlayer's own). A display's gesture runs `midi_tool.set`, and an agent reads what a display shows in `midi_tool.state`. (S2, S8, S10, S11, S12, S5)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/midi-tools/tests/interface.rs`, `r1_note_divisions_are_whole_note_fractions_and_triplets`; `crates/midi-tools/tests/transformations.rs`, `r3_arpeggiates_parameters_have_the_rules_ranges_and_defaults` | unit |
| R2 | `crates/midi-tools/tests/transformations.rs`, `r2_ids_are_names_in_lower_case_and_a_length_keeps_a_tick`, `r2_a_note_keeps_a_tick_and_ids_name_the_tools` | unit |
| R3 | `crates/midi-tools/tests/transformations.rs`, `r3_arpeggiates_parameters_have_the_rules_ranges_and_defaults`; `crates/shell/tests/midi_tools.rs`, `r2_r3_r5_the_list_describes_every_tool_in_order`; `crates/agent/tests/midi_tools.rs`, `each_transformation_applies_by_id_with_a_seed_and_reads_back` | unit, agent |
| R4 | `crates/midi-tools/tests/transformations.rs`, `r4_overlapping_notes_make_a_group_and_a_step_with_nothing_held_writes_nothing`, `r4_a_step_holds_only_the_notes_that_sound_then` | unit |
| R5 | `crates/midi-tools/tests/transformations.rs`, `r5_steps_raise_the_cycle_each_time_the_place_goes_round`, `r5_the_styles_order_the_held_notes_as_arpeggiator_does`, `r5_chord_trigger_writes_every_held_note_and_the_random_styles_follow_the_seed`, `r5_and_r9_arpeggiate_stays_on_a_drum_racks_keys`; `crates/shell/tests/midi_tools.rs`, `r4_to_r6_arpeggiate_in_and_out_of_scale_mode_and_on_a_drum_rack` | unit |
| R6 | `crates/midi-tools/tests/transformations.rs`, `r6_a_step_lasts_rate_times_gate_and_takes_its_held_notes_values`, `r6_a_step_carries_the_stretch_of_its_notes_envelopes`, `r6_arpeggiates_notes_start_on_its_steps_and_stay_on_the_keyboard` | unit, property |
| R7 | `crates/midi-tools/tests/transformations.rs`, `r7_chops_parameters_have_the_rules_ranges_and_defaults` | unit |
| R8 | `crates/midi-tools/tests/transformations.rs`, `r8_gaps_write_the_pattern_and_a_toggle_leaves_gaps_without_a_number`; `crates/shell/tests/midi_tools.rs`, `r8_r43_chops_gaps_write_its_pattern_and_a_toggle_click_is_a_set` | unit |
| R9 | `crates/midi-tools/tests/transformations.rs`, `r9_pieces_come_in_proportion_to_their_weights_and_gaps_write_nothing`, `r9_chops_pieces_tile_each_note_exactly` | unit, property |
| R10 | `crates/midi-tools/tests/transformations.rs`, `r10_variation_moves_starts_and_ends_within_the_note` | unit |
| R11 | `crates/midi-tools/tests/transformations.rs`, `r11_connects_parameters_have_the_rules_ranges_and_defaults` | unit |
| R12 | `crates/midi-tools/tests/transformations.rs`, `r12_and_r13_a_gap_gets_slots_whose_keys_follow_the_line_between_the_notes`, `r12_connect_never_adds_a_note_outside_a_gap` | unit, property |
| R13 | `crates/midi-tools/tests/transformations.rs`, `r12_and_r13_a_gap_gets_slots_whose_keys_follow_the_line_between_the_notes`, `r13_connect_counts_steps_in_a_scale_and_on_a_drum_rack`; `crates/shell/tests/midi_tools.rs`, `r12_to_r13_connect_in_a_scale_and_on_a_drum_rack_stays_on_their_keys` | unit |
| R14 | `crates/midi-tools/tests/transformations.rs`, `r14_ornaments_parameters_have_the_rules_ranges_and_defaults` | unit |
| R15 | `crates/midi-tools/tests/transformations.rs`, `r15_a_flam_ends_where_the_note_starts_or_takes_over_its_start` | unit |
| R16 | `crates/midi-tools/tests/transformations.rs`, `r16_grace_notes_fill_the_span_and_alternate_the_note_and_its_step` | unit |
| R17 | `crates/midi-tools/tests/transformations.rs`, `r17_applying_ornament_again_ornaments_the_notes_as_they_are`; `crates/shell/tests/midi_tools.rs`, `r3_to_r19_each_transformation_applies_by_id_with_a_seed_as_one_undo_step` | unit |
| R18 | `crates/midi-tools/tests/transformations.rs`, `r18_quantizes_parameters_have_the_rules_defaults`; `crates/ui/tests/midi_tools.rs`, `quantize_has_its_five_controls_and_triplets_wait_for_a_grid` | unit, UI script |
| R19 | `crates/midi-tools/tests/transformations.rs`, `r19_quantize_moves_starts_and_ends_toward_the_lines`, `r19_an_end_whose_line_is_at_or_before_the_start_goes_to_the_first_line_after_it`, `r19_at_100_percent_every_start_is_on_a_line`; `crates/shell/tests/clip_view.rs`, `quantising_both_moves_the_start_and_then_the_end` | unit, property |
| R20 | `crates/shell/tests/midi_tools.rs`, `r20_r19_cmd_u_quantizes_with_the_tools_settings_which_are_saved_and_reloaded`; `crates/ui/tests/keymap.rs`, `quantize_and_quantize_settings_are_bound_in_the_note_editor` | unit, UI script |
| R21 | `crates/shell/tests/midi_tools.rs`, `r21_quantize_settings_shows_the_transform_panel_with_quantize_chosen_without_an_undo_step`; `crates/ui/tests/midi_tools.rs`, `cmd_shift_u_and_the_edit_menu_show_the_transform_panel_with_quantize_chosen`; `crates/ui/tests/keymap.rs`, `quantize_and_quantize_settings_are_bound_in_the_sample_editor` | unit, UI script |
| R22 | `crates/shell/tests/warping.rs`, `the_audio_clips_quantize_panel_gives_the_same_markers_as_quantize_audio`; `crates/ui/tests/audio_panel.rs`, `the_right_group_holds_audio_utilities_and_a_transform_panel_of_quantize` | unit, UI script |
| R23 | to be written in WP-6.13 | |
| R24 | to be written in WP-6.13 | |
| R25 | to be written in WP-6.13 | |
| R26 | to be written in WP-6.13 | |
| R27 | to be written in WP-6.13 | |
| R28 | to be written in WP-6.13 | |
| R29 | to be written in WP-6.13 | |
| R30 | to be written in WP-6.13 | |
| R31 | to be written in WP-6.13 | |
| R32 | to be written in WP-6.13 | |
| R33 | to be written in WP-6.13 | |
| R34 | to be written in WP-6.13 | |
| R35 | to be written in WP-6.13 | |
| R36 | to be written in WP-6.13b | |
| R37 | to be written in WP-6.13b | |
| R38 | to be written in WP-6.13b | |
| R39 | to be written in WP-6.13b | |
| R40 | to be written in WP-6.13b | |
| R41 | to be written in WP-6.13b | |
| R42 | to be written in WP-6.13b | |
| R43 | Chop's toggles, WP-6.12's: `crates/ui/tests/midi_tools.rs`, `chops_pattern_and_emphasis_toggles_each_click_a_set_and_gaps_shows_a_dash`; `crates/shell/tests/midi_tools.rs`, `r8_r43_chops_gaps_write_its_pattern_and_a_toggle_click_is_a_set`. The other displays: to be written in WP-6.13 and WP-6.13b | UI script, unit |

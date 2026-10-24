# MIDI Tools: the generators

- **Status:** implemented (R1 to R28 each have a passing test)
- **Work packages:** WP-6.14 (what every generator shares, Rhythm, Seed, Shape and Euclidean: R1 to R16); WP-6.14b (Stacks, its chord banks in Beatlayer's own format and Beatlayer's built-in bank: R17 to R28). The Generate panel, choosing a tool, where a generator writes, applying, undo, seeds and chains are [midi-tools.md](midi-tools.md)'s (WP-6.11, WP-6.15)
- **Parity-matrix row:** MIDI editing: "MIDI Tools, generators (Rhythm, Seed, Shape, Stacks, Euclidean)", whose note says chord banks are in Beatlayer's own format
- **Checked against:** the Live 12 reference manual (online edition), read 20 October 2026: chapter 11, its introduction and sections 11.3.1 to 11.3.5; the Live 12 release notes, 12.0, 12.0.5, 12.1 and 12.2; the UI parity reference's I-CLICK-049 and S-NOTE-052

## The behaviour

A generator writes new notes into the time selection, or into the loop when there is none. Rhythm writes a rhythm on one pitch from a pattern of steps, with accents and split steps. Seed scatters notes at random within ranges of pitch, length and velocity, never more at once than its Voices. Shape draws a melody from a curve between two pitches, with jitter and ties. Euclidean spreads a number of hits as evenly as it can over a number of steps, for up to four voices, each on its own pitch and rotated on its own. Stacks writes chords, one or a progression of them, chosen from a chord bank on a pad of chord diagrams, each with its root, inversion, length and offset.

Every generator is applied as [midi-tools.md](midi-tools.md) says: with Auto on, each change writes the notes again into the same range; its notes replace the notes they overlap at the same pitch; with a clip scale, it writes only the scale's keys. Option-clicking the piano ruler sets a generator's pitch. Each generator's parameters, ranges and defaults are below; where the manual gives none, they are Beatlayer's own, and so is every law the manual describes only in words.

Stacks' chord banks are files in Beatlayer's own format, which a person writes or shares and loads from the browser, and Beatlayer has a bank of its own built in.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 11 "MIDI Tools", introduction | Generators take no input and add notes to the loop or the time selection |
| S2 | Manual | Same chapter, section 11.3.1 "Rhythm" | Rhythm writes a pattern and repeats it across the time selection or the loop. Pitch sets the pitch or Drum Rack pad, also set by Option-clicking (Alt-clicking) the piano ruler. Steps sets the pattern's steps, up to 16; Pattern chooses where its notes fall, the number of patterns depending on Steps and Density; Density sets how many notes it has, at most Steps. Step Duration sets a step's length, so how many times the pattern fits (a one-bar selection with 8 steps holds the pattern once at 1/8 and twice at 1/16), and it limits the Steps that can be used. Split is the chance that a step is halved; Shift moves the notes by steps, right for positive. Velocity and Accent set normal and accented notes' velocities; Accent Frequency, from 0 up to Density, sets how often a note is accented, an accented note counting as one, so 1 accents every note; the Accent Offset arrows move which notes take the accent. A second layer is made by deselecting the notes made and generating again on another pitch |
| S3 | Manual | Same chapter, section 11.3.2 "Seed" | Seed generates notes at random within a pitch range, a duration range and a velocity range. The pitch range is set by Minimum Pitch and Maximum Pitch, by a Key Track slider, or by the range slider's handles, which merge into one pitch where they meet; Option-clicking the piano ruler chooses one pitch and dragging a range. Durations run from 1/128 note to a whole note, velocities from 1 to 127. Voices sets how many notes may sound at once; Density sets how many notes are made, as a share of the pitch range filled. The range slider shows the scale's colour when the clip has a scale |
| S4 | Manual | Same chapter, section 11.3.3 "Shape" | Shape writes notes within a pitch range, spread by a shape chosen from Shape Presets or drawn in the Shape Levels display. Minimum Pitch and Maximum Pitch set the range, also by Option-dragging. Rate sets the shortest note; Tie the chance a note lasts into the next; Density how much of the shape gets notes; Jitter moves pitches at random away from the shape, staying within the range |
| S5 | Manual | Same chapter, section 11.3.4 "Stacks" | Stacks writes single chords or progressions within the clip's scale, filling the time selection or the loop. The Chord Selector Pad chooses a chord pattern by clicking and dragging, or with Cmd (Ctrl) and Up or Down; its diagrams show the chord's intervals on a Tonnetz, and hovering one names the chord in the Status Bar. Banks of chords are JSON text files, loaded by double-clicking them in the browser from a folder in Places, replacing the bank. Add Chord and Delete Chord, beside the pad, add and remove chords of a progression; the controls show the selected chord's settings. Chord Root sets the root, also from Option-clicking the piano ruler or with Up and Down; with a clip scale the root follows the scale's root and other roots are the scale's notes. Chord Inversion steps through inversions, negative values giving the same inversions an octave lower. Chord Duration and Chord Offset set the chord's length and place in eighths of its original length |
| S6 | Manual | Same chapter, section 11.3.5 "Euclidean" | Euclidean writes Euclidean rhythms for up to four voices in the time selection or the loop. The Pattern tab draws where the notes will go, with a switch and a Rotation slider for each voice (its offset from the selection's start) and a button that sets every Rotation at random; the Voices tab has each voice's switch, its pitch or Drum Rack pad, arrows that move every voice's pitch together, and each voice's Velocity. Steps sets the pattern's length, repeated across the selection; the manual says Density sets how many times the pattern repeats in the selection; Division sets a step's length |
| S7 | Release notes | Live 12 release notes, 12.0, 12.0.5, 12.1 and 12.2 | The generators are Rhythm, Seed, Shape and Stacks, with Euclidean (12.0). Seed, Shape, Rhythm and Stacks' Root and Inversion take typed values, and pitch fields take note numbers; Shape writes only to a Drum Rack's key tracks that hold a pad (12.0.5). Stacks loads banks of its own from JSON files, works with Drum Racks, and takes negative inversions; generators take a pitch or range from the piano ruler (12.1). The browser's MIDI Tools filters include Stacks (12.2) |
| S8 | Beatlayer | [midi-tools.md](midi-tools.md) R5, R8, R9, R12 to R17 and R20; [midi-transformations.md](midi-transformations.md) R1 and R2; [scales.md](scales.md) R1 and R2; [piano-roll.md](piano-roll.md) R26; [browser.md](browser.md) | The framework every generator sits in; note divisions and shared terms; the scales and the nearest scale key; what a click on the piano ruler does without a modifier; the browser's places and items |
| S9 | Beatlayer | [The architecture](../02-architecture.md), "Project files and interop" and "Agent control" | Presets, racks and clips have file types of Beatlayer's own; what an agent sets it reads back |
| S10 | Reference | [interactions.md](../ui-parity/interactions.md) I-CLICK-049; [shortcuts.toml](../ui-parity/shortcuts.toml) S-NOTE-052 (from the manual, sections 11.3.1 to 11.3.4, and the 12.1 release notes) | Option-clicking or Option-dragging the piano ruler sets a generator's pitch or range; Cmd+Up and Cmd+Down choose the previous or next chord on Stacks' Chord Selector Pad |

## Rules

**What the generators share (WP-6.14)**

- **R1.** The terms of [midi-transformations.md](midi-transformations.md) R1 and R2 apply here: note divisions, the grid step, steps, rounding and "random". A generator's keys are those [midi-tools.md](midi-tools.md) R8 and R9 let it write; a pitch parameter holds any key from 0 to 127, and a generator writes the allowed key nearest it, the lower of two equally near ([scales.md](scales.md) R2). A generated note's velocity is 100 unless the generator's rules say otherwise (Beatlayer's own). (S8)
- **R2.** While the Generate panel shows Rhythm, Seed, Shape or Stacks, Option-clicking (Alt-clicking) a key in the piano ruler (I-CLICK-049) sets the generator's pitch: Rhythm's Pitch, the selected chord's Chord Root for Stacks, and for Seed and Shape a range of that one key; Option-dragging across keys sets Seed's and Shape's range to the keys dragged over. It runs `midi_tool.set`, so with Auto on the generator applies ([midi-tools.md](midi-tools.md) R17); a click without the modifier selects notes as [piano-roll.md](piano-roll.md) R26 says. (S2, S3, S4, S5, S7, S10)
- **R3.** A pitch control shows its key by name and takes a typed note name or note number. (S7)

**Rhythm (WP-6.14)**

- **R4.** Rhythm's parameters (the ranges besides Steps', Density's and Accent Frequency's, and the defaults, are Beatlayer's own): (S2)

  | Parameter | Kind and range | Default |
  |---|---|---|
  | Pitch | a key | 60 (C3) |
  | Steps | a whole number, 1 to 16 | 16 |
  | Density | a whole number, 0 to Steps | 4 |
  | Pattern | a whole number, 0 to one less than the number of patterns (R5) | 0 |
  | Step Duration | a note division | 1/16 |
  | Split | 0 % to 100 % | 0 % |
  | Shift | a whole number, −(Steps − 1) to Steps − 1 | 0 |
  | Velocity | 1 to 127 | 100 |
  | Accent | 1 to 127 | 127 |
  | Accent Frequency | a whole number, 0 to Density | 0 |
  | Accent Offset | a whole number, 0 to Density − 1, moved by two arrow buttons | 0 |

- **R5.** A pattern is a choice of Density of the Steps steps to hold notes. There are as many patterns as such choices. Pattern 0 is the even spread Euclidean writes for those numbers (R15); Pattern p, from 1, is the p-th of the other choices, in order of their step numbers compared from the first (Beatlayer's own order). Shift then moves every chosen step Shift places later, going round from the last step to the first, and a negative Shift earlier. (S2)
- **R6.** Rhythm repeats its pattern from the range's start, each step Step Duration long, as many whole and part patterns as reach the range's end. Steps can't be more than the range holds whole steps: a larger value acts as that many, at least 1. Each chosen step writes a note at Pitch, a step long; with chance Split it writes two notes instead, each half a step long. (S2; the halves' length is Beatlayer's own)
- **R7.** Counting the steps that write, in order from the range's start, n from 0, step n's notes are accented when Accent Frequency f is above 0 and n − Accent Offset is a whole multiple of f, so f = 1 accents every one. An accented step's notes take Accent's velocity and the rest Velocity's. (S2)

**Seed (WP-6.14)**

- **R8.** Seed's parameters, where a range is a lowest and a highest value, the lowest at most the highest. Key Track moves the pitch range up or down, keeping its width; the range's handles meet in one pitch when its lowest and highest are the same. (Voices' and Density's ranges and the defaults are Beatlayer's own): (S3)

  | Parameter | Kind and range | Default |
  |---|---|---|
  | Pitch Range | two keys | 48 (C2) to 72 (C4) |
  | Duration Range | two lengths, whole numbers of 1/128 notes from 1/128 to 1 | 1/16 to 1/4 |
  | Velocity Range | two velocities, 1 to 127 | 64 to 127 |
  | Voices | a whole number, 1 to 16 | 4 |
  | Density | 0 % to 100 % | 50 % |

- **R9.** Seed makes as many notes as Density times the number of keys it may write in the pitch range (R1) for each whole note the range lasts, rounded (Beatlayer's own law). For each in turn it draws a key from those keys, a length from the Duration Range in whole 1/128 notes, a velocity from the Velocity Range, and a start from the grid steps in the range, each with every value equally likely. (S3)
- **R10.** Seed keeps a drawn note only when, with it, no more than Voices notes sound at any tick and no two notes of one key overlap; otherwise it draws the note's start again, up to 16 times, and then leaves the note out. A note that would run past the range's end is cut there. (S3; the retries are Beatlayer's own)

**Shape (WP-6.14)**

- **R11.** Shape's parameters (the presets, the number of levels, the ranges besides Density's and Jitter's, and the defaults are Beatlayer's own). Choosing a preset writes its levels; drawing in the Shape Levels display sets the levels under the pointer, and the preset chooser then reads User: (S4)

  | Parameter | Kind and range | Default |
  |---|---|---|
  | Shape Presets | a choice: Rise, Fall, Arch, Valley, Wave, Steps, and User once drawn | Rise |
  | Shape Levels | 16 levels, each from 0 to 1 | Rise's |
  | Minimum Pitch, Maximum Pitch | two keys, the minimum at most the maximum | 48 (C2), 72 (C4) |
  | Rate | a note division | 1/16 |
  | Tie | 0 % to 100 % | 0 % |
  | Density | 0 % to 100 % | 100 % |
  | Jitter | 0 % to 100 % | 0 % |

  The presets' levels, for level k from 0 to 15: Rise k ÷ 15; Fall 1 − k ÷ 15; Arch sin(π × k ÷ 15); Valley 1 − sin(π × k ÷ 15); Wave ½ + ½ × sin(2π × k ÷ 16); Steps (k modulo 4) ÷ 3.
- **R12.** Shape divides the range into slots a Rate long from its start, as many as fit whole, and Density chooses that many slots times Density, rounded, at random, to fill. A slot a fraction x of the way through the range takes the level the levels give there, the 16 levels spread evenly from 0 to 1 with straight lines between them; its key is Minimum Pitch + level × (Maximum Pitch − Minimum Pitch), moved by a random amount up to Jitter × (Maximum Pitch − Minimum Pitch) either way, held within the two pitches, and then the nearest key the generator may write (R1). (S4)
- **R13.** A filled slot's note lasts the slot; with chance Tie it lasts the next slot too, and that slot stays empty. (S4; the law is Beatlayer's own)

**Euclidean (WP-6.14)**

- **R14.** Euclidean's parameters (the ranges and defaults are Beatlayer's own; Density as the number of hits in the pattern is Beatlayer's own reading). The Voices tab's arrows move every voice's pitch together by a step (R1): (S6)

  | Parameter | Kind and range | Default |
  |---|---|---|
  | Voices 1 to 4 | each a switch, a pitch (a key), a Velocity from 1 to 127 and a Rotation, a whole number from 0 to Steps − 1 | voice 1 on, 2 to 4 off; pitches 36, 38, 42 and 46; Velocity 100; Rotation 0 |
  | Steps | a whole number, 1 to 32 | 16 |
  | Density | a whole number, 0 to Steps | 4 |
  | Division | a note division | 1/16 |

- **R15.** Euclidean's pattern has Steps steps, of which Density hold a hit, spread as evenly as they go: step i, from 0, holds a hit when (i × Density) modulo Steps is less than Density, so step 0 always does. A voice's Rotation r moves its hits r steps later: step i holds a hit when ((i − r) × Density) modulo Steps is less than Density. (S6; the law is Beatlayer's own)
- **R16.** Each switched-on voice repeats its pattern from the range's start, each step a Division long, as many whole and part patterns as reach the range's end, and writes a note a step long at its pitch and Velocity on each hit. The Pattern tab's button sets every voice's Rotation to a random value from 0 to Steps − 1, drawn from a new seed or the seed given to `midi_tool.randomize` (`edit`), and with Auto on applies (Beatlayer's own command). (S6)

**Stacks (WP-6.14b)**

- **R17.** Stacks writes a progression of 1 to 16 chords, one after another; a new Stacks has one. The progression shows as a row of cells, the selected chord's lit, and clicking a cell selects that chord (the look is Beatlayer's own). Add Chord (`midi_tool.add_chord`), right of the Chord Selector Pad, adds a chord after the selected one, the bank's first entry with the selected chord's root, and selects it; Delete Chord (`midi_tool.delete_chord`) deletes the selected chord unless it is the only one. With Auto on, each applies Stacks. The pad and the controls below show the selected chord's settings: (S5; the new chord's place and settings, and the ranges besides Chord Inversion's sign, are Beatlayer's own)

  | Parameter, each chord | Kind and range | Default |
  |---|---|---|
  | Chord | an entry of the chord bank | the bank's first |
  | Chord Root | a key | 60 (C3); in Scale Mode the scale's root from 60 up |
  | Chord Inversion | a whole number, −(n − 1) to n − 1, n the chord's notes | 0 |
  | Chord Duration | a whole number, 1 to 8 eighths of the chord's slot | 8 |
  | Chord Offset | a whole number, 0 to 7 eighths of the chord's slot | 0 |

- **R18.** The range is split into as many equal slots as the progression has chords, in order. A chord's notes start Chord Offset eighths of its slot after the slot's start and last Chord Duration eighths of the slot, cut at the slot's end. (S5)
- **R19.** A chord's notes are its Chord Root and the bank entry's intervals above it (R24): in semitones for a semitone entry; for a degree entry, in degrees of the clip's scale counted from the scale note nearest the root, or of the major scale on the root while Scale Mode is off ([scales.md](scales.md) R1, R2; the major scale is Beatlayer's own). Inversion i above 0 moves the lowest note up an octave i times, one note at a time; below 0, the highest note down an octave, |i| times. A note past 0 or 127 moves by octaves until it is inside, and in Scale Mode a note off the scale moves to the nearest scale key (R1). (S5; the inversion law is Beatlayer's own reading)
- **R20.** In Scale Mode, a new Stacks starts with the scale's root in the octave from 60 up as its first chord's root, and a root that is set is written on the nearest key of the scale (R1, R19); the setting keeps the number it was given. (S5)
- **R21.** The Chord Selector Pad draws the bank's entries as diagrams, each the chord's pitch classes marked on a lattice of fifths and thirds (a Tonnetz; the drawing is Beatlayer's own), the selected chord's entry lit. Clicking an entry, or dragging across the pad to it, makes it the selected chord's; with the pad focused, Cmd+Up and Cmd+Down (Ctrl on Windows; S-NOTE-052) choose the entry before or after. While the pointer is over an entry, the Status Bar shows its name and its notes on the selected chord's root. Each runs `midi_tool.set`, so with Auto on Stacks applies; choosing which chord the controls show (`selected`) changes no notes. The pad shows nine diagrams at a time, three across, on pages that follow the selected entry and turn with two arrows beside it (Beatlayer's own). (S5, S10)
- **R22.** Option-clicking a key in the piano ruler sets the selected chord's root (R2), and with Chord Root focused, Up and Down move it a step (R1). (S5)
- **R23.** Stacks has one chord bank at a time, Beatlayer's built-in bank (R27) until another is loaded. The bank is part of Stacks' settings, saved with the set with every entry in it, so a set opens with its bank whether or not the file is still there. Loading a bank keeps the progression's chords by their place in the bank, a place past the new bank's end taking its first entry. (S5; keeping the bank in the set is Beatlayer's own)

**Chord banks (WP-6.14b)**

- **R24.** A chord bank file is UTF-8 JSON, Beatlayer's own format, with the extension `.blstacks`: an object with `"format": "beatlayer-stacks"`, `"schema": 1`, a `"name"`, and `"chords"`, a list of 1 to 256 entries. An entry has a `"name"` of 1 to 64 characters and either `"semitones"` or `"degrees"`: 2 to 8 whole numbers from 0 to 48, the first 0, each larger than the one before. Beatlayer writes the fields in that order. (S5, S9)
- **R25.** A file larger than 1 MiB, one that isn't such an object, or one with an entry that breaks R24 doesn't load: nothing changes, and the Status Bar and the command's result say what's wrong and where (Beatlayer's own). Fields the format doesn't name are ignored. (S9)
- **R26.** The browser lists `.blstacks` files as chord banks in every place it lists files, and double-clicking one, or `midi_tool.load_bank` (`edit`, read back in `midi_tool.state`) with its path, loads it into Stacks, replacing the bank, as one change of Stacks' settings ([midi-tools.md](midi-tools.md) R5); `midi_tool.load_bank` with `built_in` loads the built-in bank back. Loading changes no notes and is no undo step. (S5, S7, S8)
- **R27.** Beatlayer's built-in bank, "Beatlayer Chords", holds these entries in this order (its content is Beatlayer's own): (S5)

  | Name | Intervals |
  |---|---|
  | Triad | degrees 0 2 4 |
  | Seventh | degrees 0 2 4 6 |
  | Ninth | degrees 0 2 4 6 8 |
  | Sixth | degrees 0 2 4 5 |
  | Add Nine | degrees 0 2 4 8 |
  | Sus2 | degrees 0 1 4 |
  | Sus4 | degrees 0 3 4 |
  | Open Triad | degrees 0 4 9 |
  | Fifth | degrees 0 4 |
  | Major | semitones 0 4 7 |
  | Minor | semitones 0 3 7 |
  | Diminished | semitones 0 3 6 |
  | Augmented | semitones 0 4 8 |
  | Suspended 2 | semitones 0 2 7 |
  | Suspended 4 | semitones 0 5 7 |
  | Major 7 | semitones 0 4 7 11 |
  | Minor 7 | semitones 0 3 7 10 |
  | Dominant 7 | semitones 0 4 7 10 |
  | Half-Diminished 7 | semitones 0 3 6 10 |
  | Diminished 7 | semitones 0 3 6 9 |
  | Minor-Major 7 | semitones 0 3 7 11 |
  | Major 6 | semitones 0 4 7 9 |
  | Minor 6 | semitones 0 3 7 9 |
  | Major 9 | semitones 0 4 7 11 14 |
  | Minor 9 | semitones 0 3 7 10 14 |
  | Dominant 9 | semitones 0 4 7 10 14 |
  | Power | semitones 0 7 |

- **R28.** `midi_tool.state` reads Stacks' progression, each chord's settings, and its bank: the bank's name, whether it is the built-in one, and every entry with its name and intervals. (S8)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/midi-tools/tests/generators.rs`, `r1_a_pitch_is_written_on_the_allowed_key_nearest_it`, `every_generated_note_lies_in_its_range_and_its_allowed_keys` | unit, property |
| R2 | `crates/ui/tests/clip_view.rs`, `option_clicking_and_dragging_the_piano_ruler_sets_the_shown_generators_pitch_or_range` | UI script |
| R3 | `crates/midi-tools/tests/generators.rs`, `r3_a_pitch_has_a_name_and_takes_a_typed_name_or_number`; `crates/ui/tests/midi_tools.rs`, `rhythm_draws_its_step_pattern_names_its_pitch_and_steps_accent_offset_with_arrows` | unit, UI script |
| R4 | `crates/midi-tools/tests/generators.rs`, `r4_rhythms_parameters_have_the_ranges_and_defaults_of_the_table`; `crates/ui/tests/midi_tools.rs`, `rhythm_draws_its_step_pattern_names_its_pitch_and_steps_accent_offset_with_arrows` | unit, UI script |
| R5 | `crates/midi-tools/tests/generators.rs`, `r5_pattern_0_is_the_even_spread_and_the_others_follow_in_step_order`, `every_rhythm_pattern_holds_density_steps` | unit, property |
| R6 | `crates/midi-tools/tests/generators.rs`, `r6_the_pattern_repeats_across_the_range_and_a_split_halves_a_step`, `euclidean_writes_exactly_density_hits_per_pattern_and_rhythm_density_notes`; `crates/agent/tests/midi_tools.rs`, `each_generator_applies_by_id_with_a_seed_replacing_only_the_notes_it_overlaps` | unit, property, agent |
| R7 | `crates/midi-tools/tests/generators.rs`, `r7_accents_count_the_steps_that_write_from_the_ranges_start` | unit |
| R8 | `crates/midi-tools/tests/generators.rs`, `r8_seeds_ranges_keep_their_lowest_below_their_highest`; `crates/ui/tests/midi_tools.rs`, `seeds_pitch_range_slider_moves_a_handle_or_both_and_merges_handles` | unit, UI script |
| R9 | `crates/midi-tools/tests/generators.rs`, `r9_seed_makes_density_times_the_keys_for_each_whole_note` | unit |
| R10 | `crates/midi-tools/tests/generators.rs`, `r10_seed_keeps_to_its_voices_and_never_overlaps_a_key_with_itself`, `every_generated_note_lies_in_its_range_and_its_allowed_keys` | unit, property |
| R11 | `crates/midi-tools/tests/generators.rs`, `r11_shape_has_six_presets_of_sixteen_levels_and_drawing_reads_user`; `crates/ui/tests/midi_tools.rs`, `shapes_levels_display_draws_levels_and_the_preset_reads_user` | unit, UI script |
| R12 | `crates/midi-tools/tests/generators.rs`, `r12_shape_gives_each_slot_the_level_of_its_place_in_the_range` | unit |
| R13 | `crates/midi-tools/tests/generators.rs`, `r13_a_tie_lasts_into_the_next_slot_which_stays_empty` | unit |
| R14 | `crates/midi-tools/tests/generators.rs`, `r14_euclideans_voices_and_parameters_have_the_defaults_of_the_table`; `crates/ui/tests/midi_tools.rs`, `euclideans_tabs_have_the_voices_rotations_pitches_arrows_and_randomise_button` | unit, UI script |
| R15 | `crates/midi-tools/tests/generators.rs`, `r15_density_hits_are_spread_as_evenly_as_they_go` | unit |
| R16 | `crates/midi-tools/tests/generators.rs`, `r16_each_switched_on_voice_repeats_its_pattern_from_the_ranges_start`, `r16_the_randomise_button_draws_every_rotation_from_the_seed`; `crates/agent/tests/midi_tools.rs`, `the_randomise_button_sets_every_rotation_from_a_seed_and_reads_back` | unit, agent |
| R17 | `crates/midi-tools/tests/stacks.rs`, `r17_a_new_stacks_has_one_chord_with_the_tables_defaults`, `r17_add_chord_follows_the_selected_one_and_delete_chord_keeps_one`, `r17_a_change_to_the_selected_chord_is_the_progressions_and_back`; `crates/shell/tests/midi_tools.rs`, `r17_r28_state_reads_the_progression_and_the_bank_and_the_chord_commands_change_them`; `crates/ui/tests/midi_tools.rs`, `stacks_add_chord_and_delete_chord_and_the_progression_cells`; `crates/agent/tests/midi_tools.rs`, `stacks_loads_a_bank_and_applies_by_id_and_the_chord_commands_read_back` | unit, shell, UI script, agent |
| R18 | `crates/midi-tools/tests/stacks.rs`, `r18_the_range_is_equal_slots_and_a_chord_takes_eighths_of_its_own`, `every_written_note_lies_in_its_chords_slot` | unit, property |
| R19 | `crates/midi-tools/tests/stacks.rs`, `r19_semitone_entries_sit_on_the_root_and_degree_entries_follow_the_scale`, `r19_inversions_move_one_note_an_octave_each_and_negative_ones_go_down`, `r19_a_note_past_the_keyboard_moves_by_octaves_until_it_is_inside`, `r19_on_drum_rack_keys_every_note_is_a_pad_and_chords_do_not_repeat_one`, `every_written_note_lies_in_its_chords_slot` | unit, property |
| R20 | `crates/midi-tools/tests/stacks.rs`, `r20_in_scale_mode_a_new_stacks_starts_on_the_scales_root_and_a_set_root_is_written_on_a_scale_key`; `crates/shell/tests/midi_tools.rs`, `r20_in_scale_mode_a_new_stacks_starts_on_the_scales_root` | unit, shell |
| R21 | `crates/midi-tools/tests/stacks.rs`, `r21_selecting_a_chord_changes_no_notes_but_choosing_its_entry_does`; `crates/shell/tests/midi_tools.rs`, `r21_choosing_a_chord_to_edit_writes_no_notes_but_changing_it_does`; `crates/ui/tests/midi_tools.rs`, `stacks_pad_chooses_by_click_and_drag_and_cmd_up_and_down_and_names_the_chord_on_hover`; `crates/ui/tests/keymap/editors.rs`, `zero_deactivates_the_selected_notes_and_the_value_keys_are_the_editors` (S-NOTE-052 is the pad's) | unit, shell, UI script, keymap |
| R22 | `crates/ui/tests/clip_view.rs`, `option_clicking_the_piano_ruler_sets_the_selected_chords_root_in_stacks`; `crates/ui/tests/midi_tools.rs`, `stacks_add_chord_and_delete_chord_and_the_progression_cells` | UI script |
| R23 | `crates/midi-tools/tests/stacks.rs`, `r23_a_loaded_bank_keeps_the_chords_by_place_and_a_place_past_its_end_takes_the_first`; `crates/shell/tests/midi_tools.rs`, `r23_a_loaded_bank_is_saved_with_the_set_and_reopens_after_the_file_is_deleted` | unit, shell |
| R24 | `crates/midi-tools/tests/stacks.rs`, `r24_a_valid_bank_round_trips_through_write_and_read_unchanged` | unit |
| R25 | `crates/midi-tools/tests/stacks.rs`, `r25_each_broken_file_is_refused_with_what_is_wrong_and_where`; `crates/shell/tests/midi_tools.rs`, `r25_each_broken_bank_file_is_refused_with_the_reason_and_changes_nothing` | unit, shell |
| R26 | `crates/shell/tests/midi_tools.rs`, `r26_a_bank_loads_from_a_file_replaces_the_bank_and_changes_no_notes`; `crates/shell/tests/browser.rs`, `the_browser_lists_chord_banks_and_loading_one_replaces_stacks_bank`; `crates/ui/tests/browser.rs`, `double_clicking_a_chord_bank_loads_it_into_stacks`; `crates/agent/tests/midi_tools.rs`, `stacks_loads_a_bank_and_applies_by_id_and_the_chord_commands_read_back` | shell, UI script, agent |
| R27 | `crates/midi-tools/tests/stacks.rs`, `r27_the_built_in_bank_holds_the_twenty_seven_entries_in_order` | unit |
| R28 | `crates/shell/tests/midi_tools.rs`, `r17_r28_state_reads_the_progression_and_the_bank_and_the_chord_commands_change_them`; `crates/agent/tests/midi_tools.rs`, `stacks_loads_a_bank_and_applies_by_id_and_the_chord_commands_read_back` | shell, agent |

# Musical time: positions, time signatures, and tempo changes

This spec covers how Live shows a song position as bars, beats, and sixteenths, which time signatures it accepts and where changes can go, and how tempo changes over time. Beatlayer's implementation is `bl_core::time` (WP-0.5). The design it sits in is "Musical time" in [the architecture](../02-architecture.md#musical-time).

## Sources

Every source is either public documentation, paraphrased rather than quoted, or an observation of Live running, as [the clean-room policy](https://github.com/loombridge/beatlayer/blob/main/CONTRIBUTING.md#building-without-abletons-code) allows. The documentation was read on 10 October 2026.

| Ref | Source |
|---|---|
| M6.1 | Live 12 Reference Manual, chapter 6 "Arrangement View", section 6.1 "Layout" (the beat-time ruler) |
| M6.3 | Live 12 Reference Manual, section 6.3 "Transport and Playback" (the Arrangement Position fields) |
| M6.5 | Live 12 Reference Manual, section 6.5 "Time Signature Changes" |
| M11-6.2 | Live 11 Reference Manual, section 6.2 "Transport" (returning the play position to the start) |
| M25.5.2 | Live 12 Reference Manual, chapter "Automation and Editing Envelopes", section 25.5.2 "Editing Breakpoints" |
| M25.5.8 | Live 12 Reference Manual, section 25.5.8 "Editing the Tempo Automation" |
| H-scene | Ableton Help Center, "Scene Tempo and Time Signature" (scene tempo range) |
| O | Observed in Live 12.4.3 on macOS, 10 October 2026, between 20:28 and 20:38, in a new set with nothing signed in or saved. The steps and readings are in "What Live does", items 7 to 11 |

The manuals are at `ableton.com/en/live-manual/12/` and `ableton.com/en/live-manual/11/`. The automation chapter's page heading numbers it 25; the manual's table of contents numbers it 26.

## What Live does

1. **Position display.** The Arrangement Position fields in the Control Bar show the play position as bars, beats, and sixteenths, and the beat-time ruler above the tracks uses the same format (M6.3, M6.1).
2. **Counting from 1.** The start of the arrangement is position 1.1.1: the Live 11 manual describes returning the play position there (M11-6.2). So bars, beats, and sixteenths are each numbered from 1.
3. **Time signatures.** A time signature has a numerator of one or two digits and a denominator of 1, 2, 4, 8, or 16. When you type one, the two numbers can be separated by a slash, a comma, a full stop, or spaces (M6.5).
4. **Where changes go.** Time-signature markers aren't quantised to bars, so a change can sit anywhere the editing grid allows. A change placed before the previous bar is complete leaves a fragmentary bar, drawn hatched, which Live keeps unless you choose to delete its time or fill it out (M6.5).
5. **Tempo changes.** Song tempo is automated like any other control, on the Main track's envelope (M25.5.8). An envelope is a series of breakpoints joined by straight segments, and a segment can be bent into a curve with Alt (Windows) or Option (Mac) (M25.5.2).
6. **Tempo range.** Scene tempos take values from 20 to 999 BPM (H-scene).
7. **Tempo and the denominator.** At 120 BPM a 6/4 bar lasts 3.0 seconds and a 6/8 bar 1.5 seconds, read from the bar and time rulers (56 bars against 80 seconds). So Live's tempo counts quarter notes per minute whatever the denominator (O).
8. **Bars before 1.1.1.** There's no bar 0: the bar before bar 1 is bar −1. Typing 0.3.1 into a clip's Start field gives −1. 3. 1, half a bar before 1.1.1 in 4/4, and the Clip View's ruler reads −1.3, −1.4, 1, 1.2 (O). Live's fields draw a space after each dot.
9. **Fragmentary bars in the display.** With the insert marker at 3.3.1 and the Arrangement focused, Create > Insert Time Signature Change adds a 4/4 marker there. Live hatches the two beats of bar 3 before it in the time-signature lane, numbers the bar that starts at the marker 4, one more than the short bar, and labels the ruler from the marker on (4, 8, 12, ...) (O).
10. **Lengths.** A new one-bar Session MIDI clip's Length field reads 1. 0. 0 and a three-bar Arrangement clip's reads 3. 0. 0: lengths count bars, beats, and sixteenths from 0 (O).
11. **Typing positions and signatures.** Typing 3.3.1 into the song position field moves the insert marker exactly there. The time signature's numerator and denominator are separate fields: click one, type a number, and press Return. Each field's edit is its own undo step, and tempo breakpoint work shows in the undo history as adding and moving breakpoints (O).

## What Beatlayer does

Unit tests are in `crates/core/src/time/`, and property tests in `crates/core/tests/`.

| Rule | From | Test |
|---|---|---|
| Tick zero is the start of the arrangement and displays as `1.1.1`. | 1, 2 | `signature::tests::four_four` |
| A position displays as `bar.beat.sixteenth`, each counted from 1. A beat is one note of the denominator, so 6/8 has six beats of two sixteenths and 2/2 has two beats of eight. | 1, 2 | `signature::tests::other_denominators` |
| There's no bar 0. Bars before tick zero are numbered −1, −2, and so on, so two beats before `1.1.1` in 4/4 displays as `-1.3.1`. A bar of 0, typed or given, reads as −1: `0.3.1` is the same position as `-1.3.1`. | 8 | `signature::tests::before_zero`, `before_zero_in_other_signatures`, `bbs::tests::bar_zero_reads_as_bar_minus_one`, `bars_tile_the_timeline` |
| A position partway through a sixteenth displays as that sixteenth; the ticks past it are kept but not shown. | 1 | `bbs::tests::display`, `bbs_round_trip` |
| Numerators run from 1 to 99 and denominators are 1, 2, 4, 8, or 16. Typed signatures accept `/`, `,`, `.`, or spaces between the numbers. | 3 | `signature::tests::signatures`, `parse_signatures` |
| A time-signature change can sit at any tick and always starts a new bar. A change that comes before the previous bar is full cuts that bar short, and the short bar still counts as a bar, so the bar starting at the change is numbered one more than it. | 4, 9 | `signature::tests::fragmentary_bars`, `bars_tile_the_timeline` |
| Tempo changes are a list of segments, each constant or a straight-line ramp against musical position, with jumps where two breakpoints share a position. | 5 | `tempo::tests::breakpoints`, the tempo property tests |
| Tempo counts quarter notes per minute, whatever the time signature. | 7 | `tempo::tests::constant_tempo` |
| `bl-core` accepts tempos from 1 to 1,000 BPM, a margin around Live's range; the transport and UI packages limit them to 20 to 999. | 6 | `tempo::tests::bpm_range` |
| The Main track's Song Tempo envelope is a list of breakpoints. A command sets, moves, or deletes one, and straight segments join them, so a step is two breakpoints at one tick. | 5, 11 | `command::tests::tempo_breakpoints_and_signature_markers_edit_one_at_a_time`; `command_registry::every_command_runs_by_id` |
| A time-signature marker can be added, moved, or deleted at any tick from 0 on. Each starts a new bar, so a change before the previous bar is full cuts it short. | 4, 9 | The two tests above, and `signature::tests::fragmentary_bars` |
| `song.set_tempo_breakpoint`, `song.move_tempo_breakpoint`, `song.delete_tempo_breakpoint`, `song.set_time_signature_change`, `song.move_time_signature_change`, and `song.delete_time_signature_change` are the commands. The first breakpoint and marker at 1.1.1 cannot move or be deleted. | 5, 11 | `command_registry::every_command_runs_by_id`, `every_document_command_is_registered` |

The text form `bl-core` writes has no spaces (`-1.3.1`); drawing Live's space after each dot is the UI's job. The UI packages also take these from items 9 to 11: ruler labels that count from a signature change, lengths shown from 0 (`bl-core` has no length display yet), and separate numerator and denominator fields, each edit one undo step.

The tempo map has no curved segment yet. `TempoMap` merges equal neighbouring constant segments, so a breakpoint that does not change the tempo is not stored on its own. The arrangement draws no lane for the time-signature markers yet (WP-2.20).

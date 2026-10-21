# The arranger track: sections of the song, moved, copied and deleted across every track

- **Status:** draft
- **Work packages:** WP-6.45 (R1 to R16)
- **Parity-matrix row:** none (Beatlayer's own); [the improvements](../05-improvements.md), IMP-22 (arranger track)
- **Checked against:** the Live 12 reference manual (online edition), read 20 October 2026, for the locators and …Time commands it builds on ([time-editing.md](time-editing.md)); [the improvements](../05-improvements.md), IMP-22 and the rules for improvements

## The behaviour

The arranger track is a lane of Beatlayer's own above the Arrangement's tracks, hidden until a person shows it from the View menu. It holds named sections of the song, such as an intro, a verse and a chorus, each a span of time with a colour. Dragging a section along the lane moves everything inside it on every track, clips, automation, tempo and time-signature changes and locators, to the place it's dropped, and closes the gap it left; holding Option copies it there instead. A section can be duplicated straight after itself or deleted with everything in it, and the song closes up. Sections are made by dragging across the lane, from the time selection, or from the locators.

Moving a section is a Cut Time and a Paste Time in one step ([time-editing.md](time-editing.md)), so a section carries exactly what those commands carry. Nothing of Live's moves while the lane is hidden, and the lane adds no key of its own. Undo steps named in code type, such as `Move Section`, are Beatlayer's own names, and everything in this spec is Beatlayer's own.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Beatlayer | [The improvements](../05-improvements.md), IMP-22 | An optional lane above the tracks with named sections: dragging a section moves everything in it across every track, with its automation; sections can be duplicated or deleted. It builds on locators and inserting and deleting time, and is hidden by default |
| S2 | Beatlayer | [The improvements](../05-improvements.md), rules 1 to 4 for improvements; [the architecture](../02-architecture.md), decisions D16 to D18 | An improvement adds and moves nothing a Live user knows, is an option that defaults to Live's behaviour, goes where Live users would look, and is a registered command an agent reaches |
| S3 | Spec | [time-editing.md](time-editing.md) R1, R5, R14 to R20, R23; [arrangement-view.md](arrangement-view.md) R1, R2, R12; [arrangement-editing.md](arrangement-editing.md) R13, R18; [accessibility.md](accessibility.md) R8, R10 | Locators; what the …Time commands act on and how they split, close up and open the song; the Arrangement's layout, rows and time selection; a gesture as one undo step; Loop Selection; the keyboard paths every command needs |
| S4 | Beatlayer | [The UI parity reference](../ui-parity/README.md): [layout.md](../ui-parity/layout.md) L-ARR-023 | The arranger track's row, a Beatlayer addition |

## Rules

- **R1.** View ▸ Arranger Track, straight after Overview (M-VIEW-034) and ticked while shown, shows and hides the arranger track (`arrangement.arranger_track`, `read`, view state). It's hidden in a new set and in a set saved before it; whether it shows is saved with the set and isn't an undo step. While it's hidden the Arrangement is laid out as [arrangement-view.md](arrangement-view.md) R1 says. (S1, S2)
- **R2.** While shown, the arranger track (L-ARR-023) is an 18-point row across the lanes at the top of the track lanes, under the rulers, the scrub area and the time-signature lane, which stays in place as the tracks scroll; the track lanes start under it. Its header, in the header column, reads Arranger. (S1, S3, S4)
- **R3.** A set holds any number of sections. Each has a stable ID, a start in ticks from 1.1.1 on, a length longer than zero, a name, and a colour from the theme's palette; sections don't overlap and may leave gaps between them. The set saves them, and a set saved before them loads with none; sections that overlap, or one with no length or before 1.1.1, don't load. (S1)
- **R4.** Each section is drawn over its span in the lane in its colour, with its name, and a selected section in the selection colour. Assistive technology reads the lane as a list named Arranger Track and each section as an item named for it, its value its span in bars, beats and sixteenths, and whether it's selected. (S1, S3)
- **R5.** Dragging across an empty stretch of the lane makes a section over the span dragged, on the editing grid, cut short where it would reach another section. Create Section, in the lane's context menu, makes one over the time selection's span, and Create Sections From Locators makes one from each locator to the next, the last to the Arrangement's end, wherever no section is, each named after its locator. A new section is named "Section" and the next whole number, one more than the highest any section is named with, and takes the next colour of the palette. Each is one undo step, `Add Section`. (S1, S3)
- **R6.** Clicking a section selects it and makes its span the time selection on every track, so the …Time commands, Loop Selection and Zoom to Time Selection act on it; Shift-click selects the sections from the first selected to the one clicked. Selecting is view state, not an undo step. (S1, S3)
- **R7.** Rename (Cmd-R) and a double-click rename the selected section in place, Return keeping the typed name and Esc dropping it; the lane's context menu sets its colour. Dragging a section's left or right edge moves that edge on the editing grid, not into another section, changing only the section's span and nothing on the tracks. Each is one undo step. (S1)
- **R8.** Dragging a section moves it and its span's material on every track, as Cut Time and then Paste Time at the drop would ([time-editing.md](time-editing.md) R16, R19): the clips (split at the span's edges), every track's automation, the Song Tempo, the time-signature markers, the locators, and the section itself. The drop goes to the edge of a section nearest the pointer, or to the editing grid away from sections, Cmd (Alt on Windows) bypassing both; a drop inside the span it came from changes nothing. The clipboard is left as it was. One undo step, `Move Section`. (S1, S3)
- **R9.** Option-dragging (Ctrl-dragging on Windows) a section puts a copy of it and its span's material at the drop instead, as Copy Time and Paste Time would, leaving the original where it is: one undo step, `Copy Section`. (S1, S3)
- **R10.** Duplicate (Cmd-D) with a section selected puts a copy of it and its material straight after it, as Duplicate Time does ([time-editing.md](time-editing.md) R20), the copy taking the same name and colour and becoming the selected section: one undo step, `Duplicate Section`. (S1, S3)
- **R11.** Delete with a section selected, and Delete Section in the lane's context menu, delete the section and its span's material from every track, closing the song up as Delete Time does ([time-editing.md](time-editing.md) R15): `destructive`, one undo step, `Delete Section`. Remove Section, in the same menu, removes only the section and leaves the tracks as they are: one undo step, `Remove Section`. (S1, S3)
- **R12.** Move Earlier and Move Later, in a section's context menu, swap it with the section before or after it, moving both spans' material as R8 does: one undo step each. They're disabled where there's no section that way. (S1, S3)
- **R13.** The …Time commands treat sections as they treat locators ([time-editing.md](time-editing.md) R14): Delete Time and Cut Time remove the part of a section inside the span, deleting a section wholly inside it, and move later ones earlier; Insert Silence and Paste Time inside a section lengthen it, and at or before its start move it later; Copy Time and Paste Time carry the sections inside the span with the rest, cut to it. Edits of clips alone never move sections. (S1, S3)
- **R14.** The lane's sections are in the Arrangement's Tab order straight after Lock Envelopes ([accessibility.md](accessibility.md) R10), in time order: Tab reaching a section selects it, Cmd-R renames it, Cmd-D duplicates it, Delete deletes it as R11 says, and the context-menu key opens its context menu, which reaches Move Earlier and Move Later. (S2, S3)
- **R15.** Every action here is a registered command with a schema, a description written for a model, and a permission class, run headless with explicit section IDs, spans and positions in ticks or as bars, beats and sixteenths, or on the selection: `arranger.add_section`, `arranger.sections_from_locators`, `arranger.rename_section`, `arranger.set_section_colour`, `arranger.resize_section`, `arranger.move_section`, `arranger.copy_section`, `arranger.duplicate_section` and `arranger.remove_section` are `edit`; `arranger.delete_section` is `destructive`; `arranger.select_section` and `arrangement.arranger_track` are `read`. The query tool's song reads sections back (`settings.sections`), and `arrangement.state` reports whether the lane shows and the selected sections. (S2)
- **R16.** After any section edit no two sections overlap, no two clips on a track overlap, and undo returns the set exactly as it was before the edit. (S3)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | to be written in WP-6.45 | |
| R2 | to be written in WP-6.45 | |
| R3 | to be written in WP-6.45 | |
| R4 | to be written in WP-6.45 | |
| R5 | to be written in WP-6.45 | |
| R6 | to be written in WP-6.45 | |
| R7 | to be written in WP-6.45 | |
| R8 | to be written in WP-6.45 | |
| R9 | to be written in WP-6.45 | |
| R10 | to be written in WP-6.45 | |
| R11 | to be written in WP-6.45 | |
| R12 | to be written in WP-6.45 | |
| R13 | to be written in WP-6.45 | |
| R14 | to be written in WP-6.45 | |
| R15 | to be written in WP-6.45 | |
| R16 | to be written in WP-6.45 | |

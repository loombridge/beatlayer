# Default bindings of the document's commands

- **Status:** partial (R1 to R7 implemented as registry data, the key handling that acts on them coming with the UI packages; R8 is WP-6.43's)
- **Work packages:** WP-0.14; WP-6.43 (R8, with [custom-shortcuts.md](custom-shortcuts.md))
- **Parity-matrix row:** Interface: Keyboard shortcuts, macOS and Windows, including held (momentary) shortcuts
- **Checked against:** Live 12.4.3, through the UI parity reference recorded on 10 October 2026

## The behaviour

Live's keyboard shortcuts act on what's selected: Delete deletes the selected track, clips, or device; Cmd-R (Ctrl-R) renames the selection; S, C, and 0 solo, arm, and deactivate the selected tracks. In Beatlayer each such shortcut runs a registered command, and the registry records which shortcut runs which command, which arguments the shortcut fixes (Cmd-T fixes an audio track), and which switch it toggles. The UI supplies the remaining arguments from the selection in the shortcut's context, from a text field, or from the key itself (F1 to F8 name tracks 1 to 8).

This spec covers the document's commands from WP-0.6, and undo and redo. It records which of Live's shortcuts reach them, not how the keymap resolves a key press: the keys themselves, their contexts, and whether they act on press or while held are in [the UI parity reference](../ui-parity/shortcuts.md), and the command reference is [docs/commands.md](../commands.md). Shortcuts that reach these commands only through a gesture (nudging, resizing a clip from the keyboard, moving tracks) are left to the UI packages that build those gestures: nudging the selection (S-ARR-028) and resizing a clip from the keyboard (S-ARR-004) reach `arrangement.nudge` and `arrangement.resize_clips` (WP-1.8b; [arrangement-editing](arrangement-editing.md) R12), which run the document's clip commands.

## Sources

Every row of the UI parity reference cited here rests on the manual section it names, transcribed in our own words in WP-0.13.

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 42 (keyboard shortcuts), section 42.5 (editing), via S-EDIT-005 to S-EDIT-008 | Delete, Undo, Redo, and Rename act on the selection; Redo is Ctrl-Y on Windows |
| S2 | Manual | Same chapter, section 42.19 (tracks), via S-TRACK-001 to S-TRACK-004 and S-TRACK-013 to S-TRACK-018 | The insert-track keys for audio, MIDI, and return tracks; renaming, arming, soloing, deactivating, and deleting the selected tracks |
| S3 | Manual | Same chapter, sections 42.20 (transport) and 42.25 (momentary shortcuts), via S-TRANS-010 and S-TRACK-014 | F1 to F8 switch tracks 1 to 8 on and off; S and F1 to F8 toggle when tapped and act only while held when held |
| S4 | Manual | Same chapter, section 42.16 (Arrangement), via S-ARR-027 | 0 deactivates the selection in the Arrangement |
| S5 | Observation | Live 12.4.3 on macOS, 10 October 2026, recorded in [menus.md](../ui-parity/menus.md) (WP-0.13) | The Edit menu's items Activate/Deactivate Clip(s), Solo Track, and Arm Track, and the Create menu's Insert Audio, MIDI, and Return Track |
| S6 | Manual | Live 12 reference manual, chapter 18 "Mixing", section 18.1, as recorded in [tracks-and-undo.md](tracks-and-undo.md) | A track's activator mutes the track when it's off |

## Rules

- **R1.** Undo (Cmd-Z, Ctrl-Z) runs `edit.undo`, and Redo (Cmd-Shift-Z, Ctrl-Y) runs `edit.redo`. (S1)
- **R2.** The insert-track keys run `track.create` with the kind they name fixed: Cmd-T (Ctrl-T) an audio track, Cmd-Shift-T (Ctrl-Shift-T) a MIDI track, Cmd-Option-T (Ctrl-Alt-T) a return track. (S2, S5)
- **R3.** Delete runs `track.delete` for the selected track when a track title bar is selected and `device.remove` for the selected device in the Device View. In the Arrangement it runs `arrangement.delete` (WP-1.8b), which deletes the selected clips or the material in the time selection ([arrangement-editing](arrangement-editing.md) R9), so `clip.delete` has no key of its own. (S1, S2)
- **R4.** The rename key runs `track.rename` for a selected track, and `clip.rename` or `device.rename` for a selected clip or device, with the name typed in place. (S1, S2)
- **R5.** With a track title bar selected, S toggles the track's solo (`mixer.set_solo`), C toggles its arm (`mixer.set_arm`), and 0 toggles its activator (`mixer.set_mute`, where deactivated means muted). F1 to F8 toggle the activators of tracks 1 to 8 wherever the focus is. (S2, S3, S5, S6)
- **R6.** In the Arrangement, 0 toggles the activator of the selected clips (`clip.set_mute`). (S4, S5)
- **R7.** No other document command has a default binding. Setting volume, pan, tempo, or time signatures, and the device commands other than delete and rename, are reached by controls, gestures, and agents. (S1 to S4: chapter 42 has no other shortcut that runs one of these commands directly)
- **R8.** These are the default bindings. A person's own shortcuts ([custom-shortcuts.md](custom-shortcuts.md), IMP-17) layer over them: a person can give any of these commands other keys, or keys where R7 gives none, and take a default key away; with no user keymap, the bindings are exactly R1 to R7's. The registry's default bindings never change with a person's keymap. (S1 to S4; [the improvements](../05-improvements.md), IMP-17)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 to R7 | `crates/model/tests/command_registry.rs`, `bindings_follow_the_spec` | unit |
| R1 to R6 | `xtask/src/commands.rs`, `the_repository_reference_is_in_sync`: every bound shortcut exists in the reference and applies globally or in one of the command's contexts, and no two commands share a shortcut in a context | unit |
| R8 | to be written in WP-6.43 | |

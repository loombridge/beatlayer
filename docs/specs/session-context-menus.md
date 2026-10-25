# The Session View's context menus and the Clip View title bar's

- **Status:** draft
- **Work packages:** WP-6.56 (R1 to R16). The items other packages bring are listed in R13 with the package that brings them
- **Parity-matrix row:** Interface placement and key bindings: Menu bar and context menus: structure, order, and wording of generic commands
- **Checked against:** the Live 12 reference manual (online edition), read 20 October 2026, chapters 7 "Session View" (7.1, 7.2, 7.2.1, 7.2.2, 7.4, 7.5), 8 "Clip View" (8.1.1.1 to 8.1.1.3, 8.1.2), 13, 20 and 22; the UI parity reference's [menus.md](../ui-parity/menus.md) C-SLOT, C-SCENE and C-CLIP-SES, recorded from Live 12.4.3 on macOS

## The behaviour

Right-clicking in the Session View opens a menu of what can be done to what was clicked. An empty slot's menu holds Freeze and the bounce items, Crop Clip, the slot's stop button and Insert Empty MIDI Clip. A clip's menu holds the clipboard items, Rename and Edit Info Text, Extract Groove(s), Deactivate Clip, Export MIDI Clip..., Freeze and the bounce items, Crop Clip, the stop button, Create Follow Action Chain, Select on Launch and the colour palette. A scene's menu, opened from the Main track's column, holds the clipboard items, Rename and Edit Info Text, Insert Scene and Capture and Insert Scene, Select All Slots in Scene, Cancel Scene Launch, two launch preferences, Scene Tempo and Time Signature, and No Color with the palette. The Clip View's title bar has a menu of its own: Rename, Edit Info Text, the colours, Fold, and how the clip's panels are arranged.

As with the track header's menu ([track-context-menu.md](track-context-menu.md)), the items whose commands exist run them, and the rest are drawn in Live's place, disabled, each naming the package that makes it work. A menu acts on the selection, so with several slots or scenes selected an item acts on all of them.

This spec covers the four menus, their rows, which item runs which command, the Session View's Delete and Rename keys those items show, and opening the menus from the keyboard. The commands are specified where they live: [session-view.md](session-view.md), [launch-quantisation.md](launch-quantisation.md), [follow-actions.md](follow-actions.md), [audio-clip-editing.md](audio-clip-editing.md) (Crop Clip), [midi-clips.md](midi-clips.md) (Export MIDI Clip), [themes.md](themes.md) (colours).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Observation | Live 12.4.3 on macOS, 10 October 2026, recorded in [menus.md](../ui-parity/menus.md): C-SLOT-001 to C-SLOT-007 (an empty slot of a MIDI track), C-SCENE-001 to C-SCENE-014 (a scene in the Main track's column) and C-CLIP-SES-001 to C-CLIP-SES-018 (an empty MIDI clip on track 1), with the note that Live's context menus end with a palette of 5 rows of 14 swatches | Each menu's items in order, their separators, the keys Live shows (none for a scene's Rename), and which are disabled or ticked in a new set; no Paste while there was nothing to paste; Remove Stop Button reads Add Stop Button on a slot without one; Insert Empty MIDI Clip on MIDI tracks; Export MIDI Clip... on MIDI clips |
| S2 | Manual | Live 12 reference manual, sections 7.1, 7.2, 7.2.1, 7.2.2, 7.4 and 7.5, chapters 13 and 22, and section 20.2, through menus.md's "Other states" (C-SLOT-008, C-SLOT-009, C-SCENE-015, C-SCENE-016, C-CLIP-SES-019, C-CLIP-SES-020) | Paste in a slot's and a scene's menu when there's something to paste; Bounce Group to New Track on a group's slots; Return to Default on the scene tempo and signature fields; Slice to New MIDI Track, the Convert … to New MIDI Track commands and Separate Stems to New Audio Tracks on audio clips. Session clips and scenes are renamed and given info text from the Edit menu or their context menu, which holds a colour palette; Cancel Scene Launch cancels a scene launch already triggered; the Scene View's title bar's menu offers colours for one or several scenes |
| S3 | Manual | Same manual, chapter 8 "Clip View", sections 8.1.1.2, 8.1.1.3 and 8.1.2 | The clip title bar's context menu renames the clip and holds Edit Info Text and the colour palette; Fold in it folds the clip's panels, as double-clicking the title bar does; Arrange Clip View Panels Automatically is in it as in the View menu |
| S4 | Observation | Live 12.4.3 on macOS, 10 October 2026 ([shortcuts.toml](../ui-parity/shortcuts.toml) S-EDIT-001 to S-EDIT-005, S-EDIT-008, S-SES-005, S-SES-007 to S-SES-009, S-SES-014, S-SES-018, S-SES-028, S-TRACK-017, S-BOUNCE-001, S-BOUNCE-002, S-FILE-008, S-CTX-001, S-CTX-002) | The keys the items show and run, and the keys that open a context menu |
| S5 | Beatlayer | [track-context-menu.md](track-context-menu.md) R2, R5, R6 and R9; [the architecture](../02-architecture.md), "Commands are the API", D16, D17 and D21; [accessibility.md](accessibility.md) R8 | How a menu selects what it was opened on, types a name in place, draws an item whose package isn't built and opens from the keyboard; every action is a registered command with a permission class; Live's words and order |

## Rules

**Opening the menus**

- **R1.** A right-click on an empty slot of an audio, MIDI or group track opens the slot's menu: C-SLOT-001 to C-SLOT-007 in Live's order, with Live's separators, labels and the keys menus.md records. Insert Empty MIDI Clip is on a MIDI track's slot only, and Remove Stop Button reads Add Stop Button on a slot whose stop button was removed. With clips on the clipboard, Paste (C-SLOT-008) comes first, followed by a separator; a group track's slot adds Bounce Group to New Track (C-SLOT-009) after Bounce to New Track (both places Beatlayer's own). A return track's slots and the Main column's open no slot menu (Beatlayer's own). (S1, S2)
- **R2.** A right-click on a slot that holds a clip opens the Session clip's menu: C-CLIP-SES-001 to C-CLIP-SES-018 in Live's order, with Live's separators, labels and keys, with Export MIDI Clip... on MIDI clips only. With clips on the clipboard, Paste comes straight after Copy. An audio clip's menu adds, after Crop Clip and a separator, Slice to New MIDI Track, the three Convert … to New MIDI Track items and Separate Stems to New Audio Tracks (C-CLIP-SES-019, C-CLIP-SES-020), in the order the Arrangement's audio clip menu has them (C-CLIP-ARR-027); the places are Beatlayer's own. (S1, S2)
- **R3.** A right-click on a scene in the Main track's column (its launch button, name or number) opens the scene's menu: C-SCENE-001 to C-SCENE-014 in Live's order. With scenes on the clipboard, Paste (C-SCENE-015) comes straight after Copy (Beatlayer's place). A right-click on the Scene View's title bar ([session-view.md](session-view.md) R26) opens the same menu for the selected scenes (Beatlayer's own, beside the manual's colours there). A right-click on a scene's tempo or signature field opens a menu of one item, Return to Default (C-SCENE-016), enabled while the field holds the scene's own value. (S1, S2)
- **R4.** A right-click on the Clip View's title bar opens its menu, in this order (Beatlayer's own): Rename, Edit Info Text, a separator, the colour palette, a separator, Fold, a separator, then Arrange Clip View Panels Automatically, Arrange Clip View Panels Horizontally and Arrange Clip View Panels Vertically, the one in use ticked, labelled as the View menu's are (M-VIEW-015 to M-VIEW-017). It acts on the clips the Clip View shows. (S3)
- **R5.** A right-click on a slot or a scene that isn't selected selects it first; with several selected, the one clicked keeps the selection, and an item acts on every selected slot, clip or scene as its command does ([track-context-menu.md](track-context-menu.md) R2). An empty slot's menu acts on the slot clicked. (S5)

**What works**

- **R6.** In the slot's menu, Remove Stop Button and Add Stop Button run `slot.set_stop_button` (Cmd+E), and Insert Empty MIDI Clip runs `session.insert_empty_midi_clip` (Cmd+Shift+M). Crop Clip is disabled in an empty slot's menu, as in Live's new set. (S1, S4)
- **R7.** In the Session clip's menu: Delete runs `session.delete` (the Delete key); Rename types the name in the slot over the clip's colour, starting from the clip's name, and Return runs `clip.rename` with it while Esc leaves it, the keys going to the name while it's typed, as the track's Rename does ([track-context-menu.md](track-context-menu.md) R5); Deactivate Clip runs `session.deactivate` (0); Export MIDI Clip... runs `midi.export_clip`, which asks for a file; Crop Clip runs `clip.crop` (Cmd+Shift+J); Remove Stop Button runs `slot.set_stop_button`; Create Follow Action Chain runs `session.create_follow_action_chain` (Cmd+Shift+Enter); Select on Launch (Pref), ticked while it's on, runs `settings.set_select_on_launch`; the colour palette runs `clip.set_colour` ([themes.md](themes.md) R16). (S1, S2, S4)
- **R8.** In the scene's menu: Duplicate runs `session.duplicate_scene` and Delete `session.delete` on the selected scenes; Rename types the name in the scene's name field in the Main column, and Return runs `scene.rename`; Insert Scene runs `session.insert_scene` (Cmd+I) and Capture and Insert Scene `session.capture_and_insert_scene` (Cmd+Shift+I); Select All Slots in Scene selects every slot of the selected scenes through `session.select`, on the tracks that have slots; Cancel Scene Launch runs R10's command; Select Next Scene on Launch (Pref) and Trigger Recording on Launch (Pref), each ticked while on, run `settings.set_select_next_scene_on_launch` and `settings.set_start_recording_on_scene_launch`; Scene Tempo and Time Signature, ticked while the fields show, runs `mixer_view.scene_tempo`; No Color and the palette run `scene.set_colour` ([themes.md](themes.md) R16). Return to Default (C-SCENE-016) runs `scene.set_tempo` or `scene.set_time_signature` with no value of the scene's own. (S1, S2, S4)
- **R9.** In the Clip View title bar's menu: Rename types the name in the title bar and runs `clip.rename` on the clips it shows, as one undo step; the colour palette runs `clip.set_colour` on them. (S3)
- **R10.** Cancel Scene Launch (`session.cancel_scene_launch`, `read`, as the launch commands are) drops every launch and stop the selected scene's last launch queued that hasn't happened yet, and the tempo and signature that launch would have set, leaving every clip as it is playing or stopped; it's disabled while the scene has nothing queued. It reads back through `session.state`'s slot states. (S2)
- **R11.** With the Session View focused, the Delete key (S-EDIT-005) runs `session.delete` on the selection, and Cmd+R (S-EDIT-008) renames the selected clip or scene as the menus' Rename does. Each item shows the key menus.md records for it, and none for a scene's Rename (C-SCENE-005). Cut, Copy, Paste and Duplicate in the slot's, the clip's and the scene's menus (C-CLIP-SES-001 to C-CLIP-SES-003, C-SCENE-001, C-SCENE-002, C-SLOT-008, C-SCENE-015) run `session.cut`, `session.copy`, `session.paste` and `session.duplicate` ([session-view.md](session-view.md) R39 to R43), and are enabled exactly when those can run. (S1, S4)
- **R12.** Every item is enabled exactly when its command can run on the selection: Paste Bounced Audio with bounced audio to paste, Crop Clip with a clip, Cancel Scene Launch with a launch queued, Export MIDI Clip... on a MIDI clip. (S1, S5)

**What waits on other packages**

- **R13.** An item whose command isn't built is drawn disabled in its place and names its package in its AccessKit description and its hover text, as "Not built yet: arrives with …" ([track-context-menu.md](track-context-menu.md) R6):
  - Freeze Track (C-SLOT-001, C-CLIP-SES-010): WP-4.14.
  - Bounce Track in Place, Bounce to New Track and Paste Bounced Audio (C-SLOT-002 to C-SLOT-004, C-CLIP-SES-011 to C-CLIP-SES-013), and Bounce Group to New Track (C-SLOT-009): WP-4.15.
  - Edit Info Text (C-CLIP-SES-006, C-SCENE-006, and the Clip View title bar's): the command works from Edit ▸ Edit Info Text with the clip or scene selected ([info-view.md](info-view.md) R10, WP-6.58); the items wait for these menus.
  - Extract Groove(s) (C-CLIP-SES-007): WP-6.20.
  - Slice to New MIDI Track (C-CLIP-SES-019): WP-4.16.
  - Convert Harmony, Melody and Drums to New MIDI Track (C-CLIP-SES-019): Phase 9 (audio to MIDI, IMP-06).
  - Separate Stems to New Audio Tracks on a Session clip (C-CLIP-SES-020): Phase 9 (stem separation of Session clips, IMP-11).
  - Fold and the three Arrange Clip View Panels items (the Clip View title bar's): WP-6.72, the clip panels' layout ([detail-views.md](detail-views.md) R20, R23).
  Duplicate as Linked Clip and Unlink Clip, straight after Duplicate in the Session clip's menu, are added by WP-6.44 ([linked-clips.md](linked-clips.md) R1, R8) and aren't drawn before it. (S1, S2, S5)

**Keys, assistive technology and readback**

- **R14.** S-CTX-002 opens the menu of the selection: the selected slot's or clip's, or the selected scene's, with the Session View focused, and the Clip View title bar's with the Clip View focused (Beatlayer's own for the title bar); S-CTX-001 opens the menu of what's under the pointer. The first item that can be chosen is focused, Up and Down move through the items that can be chosen, Enter runs one and Esc closes the menu. Every item reaches AccessKit with its label, its keys, its ticked state and whether it's enabled; a palette is reached as [themes.md](themes.md) R22 says. ([track-context-menu.md](track-context-menu.md) R9; [accessibility.md](accessibility.md) R8.) (S4, S5)
- **R15.** The menus are data beside the others ([track-context-menu.md](track-context-menu.md)): each row is tied to its shortcut, its command, or its pending package, so the menu reference, the UI and the agent API's discovery read the same rows, and a check fails when a row of menus.md's C-SLOT, C-SCENE or C-CLIP-SES is missing or out of Live's order. (S1, S5)
- **R16.** Export MIDI Clip... in the Arrangement clip's menu (C-CLIP-ARR-011), drawn pending today, runs `midi.export_clip` too, on MIDI clips. (S1, S4)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | to be written in WP-6.56 | |
| R2 | to be written in WP-6.56 | |
| R3 | to be written in WP-6.56 | |
| R4 | to be written in WP-6.56 | |
| R5 | to be written in WP-6.56 | |
| R6 | to be written in WP-6.56 | |
| R7 | to be written in WP-6.56 | |
| R8 | to be written in WP-6.56 | |
| R9 | to be written in WP-6.56 | |
| R10 | to be written in WP-6.56 | |
| R11 | to be written in WP-6.56 | |
| R12 | to be written in WP-6.56 | |
| R13 | to be written in WP-6.56 | |
| R14 | to be written in WP-6.56 | |
| R15 | to be written in WP-6.56 | |
| R16 | to be written in WP-6.56 | |

# Menus

The menu bar and the main context menus of Live 12, as Beatlayer reproduces them (D16). Read the [README](README.md) first for the sources, the method, and how UI packages cite rows.

## How to read this page

- **Item labels** are the generic command names Live shows, recorded so Beatlayer's menus read the same. Labels that name an Ableton product or service are marked; Beatlayer replaces or drops those (each needs a decisions-log entry).
- **Order** is the order Live 12.4.3 lists the items on macOS, observed on 10 October 2026: first through the menu bar's accessibility interface (titles and order of every menu and submenu), then by opening each menu to read its separators, the keys it shows, and which items are disabled or ticked. Every row here was observed, except Beatlayer's own additions, keyed by the ID of their decision or improvement (D24, IMP-16 and so on), and the context menus that section says weren't opened in Live.
- **Separators** are the rows with — in the Item column. Live draws a line there.
- **Keys shown** are the keys Live displays beside the item on macOS, written as it writes them: ⌘ Cmd, ⌥ Option, ⇧ Shift, ^ Ctrl, ⇥ Tab, ⌫ Delete, ↩ Return. An empty cell means Live shows no key. Submenu contents were read from the accessibility tree only, so their keys and separators aren't recorded, except for the Mixer Controls and Arrangement Track Controls submenus.
- **Shortcut** gives the ID of the entry in [shortcuts.toml](shortcuts.toml) that binds the command, which holds both platforms' bindings. Where Live shows no key, an entry from the manual may still bind one; the notes say so.
- **In a new set** is the item's state in a new, empty set as observed: *disabled*, *ticked* (a check mark), or both. Blank means enabled. Ticks were recorded for the View menu and the context menus; elsewhere a blank cell says nothing about a tick.
- **Labels change with focus and selection.** The menus were read with the Session View focused, an empty clip slot of a MIDI track selected, and the Learn View open. Label changes seen in 12.4.3 are in the notes, for example Add Time Signature Change, which reads Insert Time Signature Change when the Arrangement has focus.
- On Windows there's no application menu: Settings lives in the Options menu and Quit in the File menu (manual 2.3). Windows menus open with Alt and the first letter of their title (S-MENU-001).

Menu-bar order on macOS: Live (application menu), File, Edit, Create, Playback, View, Navigate, Options, Help. The Playback menu is new since the manual's list of menus; it holds the transport commands.

## Live (application menu, macOS only)

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| M-APP-001 | About Live | | | | Beatlayer: About Beatlayer (D21); it holds an Acknowledgements button opening the bundled third-party notices ([app shell](../specs/app-shell.md) R26, WP-10.35) |
| | — | | | | |
| M-APP-002 | Settings... | ⌘, | S-VIEW-019 | | On Windows: Options menu |
| | — | | | | |
| M-APP-003 | Services ▸ | | | | Supplied by macOS |
| | — | | | | |
| M-APP-004 | Hide Live | ⌘H | S-FILE-006 | | Beatlayer: Hide Beatlayer (D21) |
| M-APP-005 | Hide Others | ⌥⌘H | S-FILE-010 | | macOS standard item |
| M-APP-006 | Show All | | | | macOS standard item |
| | — | | | | |
| M-APP-007 | Quit Live | ⌘Q | S-FILE-005 | | Beatlayer: Quit Beatlayer (D21). On Windows: File menu |

## File

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| M-FILE-001 | New Live Set | ⌘N | S-FILE-001 | | Beatlayer: New Beatlayer Set (D21) |
| M-FILE-002 | Open Live Set... | ⌘O | S-FILE-002 | | Beatlayer: Open Beatlayer Set... (D21); its dialog also lists .dawproject files, which it imports as a new set ([dawproject](../specs/dawproject.md) R83) |
| M-FILE-003 | Open Recent Set ▸ | | | | Submenu of recently opened sets; its contents are the user's files and weren't recorded |
| M-FILE-004 | Close Window | ⌘W | S-FILE-009 | disabled | Not in the manual |
| | — | | | | |
| M-FILE-005 | Install Pack... | | | | Installs a pack file. Beatlayer: installs a Beatlayer pack (`.blpack`); Ableton's packs are out ([set exchange](../specs/set-exchange.md) R20). Built in WP-6.35b |
| M-FILE-006 | Add Folder to Browser... | | | | Adds a folder to the browser's Places |
| | — | | | | |
| M-FILE-007 | Manage Files | | | | Opens the File Manager (L-OTH-002); built in WP-6.63 |
| IMP-29 | Manage Plug-Ins | | | | Beatlayer addition: the set's plug-ins, in the File Manager's place (L-OTH-002; [plug-in list](../specs/plugin-list.md) R1); built in WP-6.41 |
| M-FILE-008 | Open Live Set Backups ▸ | | | | Beatlayer: Open Beatlayer Set Backups (D21). A submenu of the set's backups; its contents weren't recorded. Beatlayer lists the open set's backups, newest first, by local time; choosing one opens it as the set (WP-6.63) |
| IMP-05 | Save History | | | | Beatlayer addition: the set's save points and saved versions, in the right-hand area (L-OTH-001; [crash recovery](../specs/crash-recovery.md) R12); built in WP-6.39 |
| | — | | | | |
| M-FILE-009 | Save Live Set | ⌘S | S-FILE-003 | | Beatlayer: Save Beatlayer Set (D21) |
| M-FILE-010 | Save Live Set As... | ⇧⌘S | S-FILE-004 | | Beatlayer: Save Beatlayer Set As... (D21) |
| M-FILE-011 | Save a Copy... | | | | Built in WP-6.63 |
| M-FILE-012 | Collect All and Save | | | | |
| M-FILE-013 | Save Live Set As Template... | | | | Beatlayer: Save Beatlayer Set As Template... (D21) |
| M-FILE-014 | Save Live Set As Default Set... | | | | Beatlayer: Save Beatlayer Set As Default Set... (D21) |
| | — | | | | |
| M-FILE-015 | Export Audio/Video... | ⇧⌘R | S-FILE-007 | | Opens the Export Audio/Video dialog (L-OTH-003; [export](../specs/export.md) R28) |
| M-FILE-016 | Export MIDI Clip... | | S-FILE-008 | disabled | Live shows no key here or in the clip context menus; the manual's binding (S-FILE-008) is unconfirmed. Beatlayer adds Export DAWproject... straight after it ([dawproject](../specs/dawproject.md) R86; not in Live), with no key |
| DAWPROJECT-export | Export DAWproject... | | | | Beatlayer addition, straight after Export MIDI Clip...: opens a save dialog and writes the set as a DAWproject file, then lists what the format couldn't carry ([dawproject](../specs/dawproject.md) R86, R87); built in WP-6.37b |

## Edit

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| M-EDIT-001 | Undo | ⌘Z | S-EDIT-006 | disabled | Reads "Nothing to Undo" when there's nothing to undo; otherwise names the action |
| M-EDIT-002 | Redo | ⇧⌘Z | S-EDIT-007 | disabled | Reads "Nothing to Redo" likewise, including after a new edit discards the undone actions |
| | — | | | | |
| M-EDIT-003 | Cut | ⌘X | S-EDIT-001 | | |
| M-EDIT-004 | Copy | ⌘C | S-EDIT-002 | | |
| M-EDIT-005 | Paste | ⌘V | S-EDIT-003 | disabled | |
| M-EDIT-006 | Duplicate | ⌘D | S-EDIT-004 | | Beatlayer adds Duplicate Track Without Clips straight after it (IMP-12; not in Live), and Duplicate as Linked Clip after that, with no key (IMP-21; [linked clips](../specs/linked-clips.md) R1) |
| M-EDIT-007 | Delete | ⌫ | S-EDIT-005 | | |
| | — | | | | |
| M-EDIT-008 | Deactivate Track | 0 | S-TRACK-016, S-SES-018, S-ARR-027, S-NOTE-041 | | Label follows the selection: Deactivate Track with a track's slot selected, Deactivate Clip in the clip context menus, and Deactivate Note(s) with notes selected (man 10.5.7). Beatlayer reads Activate Note(s) when every selected note is already deactivated ([note editing](../specs/note-editing.md) R18) |
| | — | | | | |
| M-EDIT-009 | Select All | ⌘A | S-EDIT-009 | | |
| M-EDIT-010 | Invert Selection | ⇧⌘A | S-NOTE-034 | disabled | |
| M-EDIT-011 | Select Loop | ⇧⌘L | S-ARR-013 | disabled | |
| M-EDIT-012 | Loop Selection | ⌘L | S-ARR-011 | disabled | |
| M-EDIT-013 | Clip Markers ▸ | | | | Submenu, rows M-EDIT-013a to 013d. With the note editor focused, the items set the markers to the time selection (release notes 12.0) |
| M-EDIT-013a | Set Start Marker | | S-LOOP-001 | | |
| M-EDIT-013b | Set Loop Start | | S-LOOP-002 | | |
| M-EDIT-013c | Set Loop End | | S-LOOP-003 | | |
| M-EDIT-013d | Set End Marker | | S-LOOP-004 | | |
| | — | | | | |
| M-EDIT-014 | Group Tracks | ⌘G | S-TRACK-006, S-DEV-001, S-NOTE-031 | | Label follows the selection (Group Tracks, Group for devices, Group Notes) |
| M-EDIT-015 | Ungroup Tracks | ⇧⌘G | S-TRACK-007, S-DEV-002, S-NOTE-032 | disabled | Likewise |
| | — | | | | |
| M-EDIT-016 | Compare: Switch to B | P | S-DEV-011 | disabled | Reads "Switch to A" when B is active |
| M-EDIT-017 | Compare: Copy A to B | | | disabled | Reverses when B is active |
| | — | | | | |
| M-EDIT-018 | Cut Time | ⇧⌘X | S-ARR-015 | disabled | |
| M-EDIT-019 | Copy Time | ⇧⌘C | S-ARR-016 | disabled | |
| M-EDIT-020 | Paste Clip(s) | ⇧⌘V | S-ARR-017 | disabled | The earlier accessibility read gave Paste Time at this position; the manual calls the command Paste Time |
| M-EDIT-021 | Duplicate Time | ⇧⌘D | S-ARR-018 | disabled | |
| M-EDIT-022 | Delete Time | ⇧⌘⌫ | S-ARR-019 | disabled | |
| | — | | | | |
| M-EDIT-023 | Apply Transformation/Generator | ⌘↩ | S-NOTE-033 | disabled | Applies the chosen tool of the Transform or Generate panel shown last; disabled while no MIDI clip shows ([MIDI tools](../specs/midi-tools.md) R19) |
| | — | | | | |
| M-EDIT-024 | Rename | ⌘R | S-EDIT-008 | | |
| M-EDIT-025 | Edit Info Text | | | | Writes the Info View text for the selection |
| | — | | | | |
| M-EDIT-026 | Solo Track | S | S-TRACK-014 | | |
| M-EDIT-027 | Arm Track | C | S-TRACK-013 | | |
| | — | | | | |
| M-EDIT-028 | Freeze Track | ⌥⇧⌘F | S-TRACK-017 | | |
| M-EDIT-029 | Bounce Track in Place | | | | |
| M-EDIT-030 | Bounce to New Track | ⌘B | S-BOUNCE-001 | disabled | |
| M-EDIT-031 | Paste Bounced Audio | ⌥⌘V | S-BOUNCE-002 | disabled | |
| | — | | | | |
| M-EDIT-032 | Remove Stop Button | ⌘E | S-SES-005 | disabled | Reads "Add Stop Button" on a slot without one |
| M-EDIT-033 | Consolidate | ⌘J | S-ARR-002 | disabled | |
| M-EDIT-034 | Fit to Time Range | ⌥⌘J | S-NOTE-008 | disabled | |
| | — | | | | |
| M-EDIT-035 | Quantize | ⌘U | S-NOTE-011, S-SAMPLE-001 | disabled | |
| M-EDIT-036 | Quantize Settings... | ⇧⌘U | S-NOTE-012, S-SAMPLE-002 | disabled | With the note editor focused, shows the Transform panel with Quantize chosen (man 11.2.7, 10.5.11; [MIDI transformations](../specs/midi-transformations.md) R21) |
| M-EDIT-037 | Extract Groove(s) | | | disabled | |
| M-EDIT-038 | Record Quantization ▸ | | | | Submenu, rows M-EDIT-038a to 038i |
| M-EDIT-038a | No Quantization | | | | |
| M-EDIT-038b | Quarter-Note Quantization | | | | |
| M-EDIT-038c | Eighth-Note Quantization | | | | |
| M-EDIT-038d | Eighth-Note Triplets Quantization | | | | |
| M-EDIT-038e | Eighth-Note and Triplets Quantization | | | | |
| M-EDIT-038f | Sixteenth-Note Quantization | | | | |
| M-EDIT-038g | Sixteenth-Note Triplets Quantization | | | | |
| M-EDIT-038h | Sixteenth-Note and Triplets Quantization | | | | |
| M-EDIT-038i | Thirty-Second Note Quantization | | | | |
| | — | | | | |
| M-EDIT-039 | Simplify Envelope | | | disabled | Enabled with a time selection in an automation lane ([automation editing](../specs/automation-editing.md) R18) |
| | — | | | | |
| M-EDIT-040 | Emoji & Symbols | ^⌘Space | S-MENU-006 | | Added by macOS to every Edit menu |

Commands the manual places in the Edit menu that didn't show with this focus, and so appear with other selections: Split, Crop, Reverse Clip(s), Chop Note(s) on Grid, Split Note(s), Join Notes, Deactivate Note(s), Edit Time Signature (manual 6.5, 10.5). Crop Clip and Reverse Clip(s) were seen in the clip context menus (C-CLIP-SES-014, C-CLIP-ARR-017, C-CLIP-ARR-018).

## Create

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| M-CREATE-001 | Insert Audio Track | ⌘T | S-TRACK-001 | | Inserts the track directly after the selected track and selects it (observed in 12.4.3 (macOS), 13 October 2026, with one track selected) |
| M-CREATE-002 | Insert MIDI Track | ⇧⌘T | S-TRACK-002 | | Inserts the track after the last selected track and selects it (observed in 12.4.3 (macOS), 13 October 2026, with tracks 1 to 3 selected) |
| M-CREATE-003 | Insert Return Track | ⌥⌘T | S-TRACK-003 | | |
| M-CREATE-004 | Insert Take Lane | ⌥⇧T | S-COMP-004 | disabled | |
| | — | | | | |
| M-CREATE-005 | Insert Scene | ⌘I | S-SES-008 | disabled | Enabled in the scene's context menu (C-SCENE-007). With the Arrangement focused, ⌘I is Insert Silence (S-ARR-014, manual 6.11, 42.16), which Beatlayer shows in this place ([time editing](../specs/time-editing.md) R17) |
| M-CREATE-006 | Capture and Insert Scene | ⇧⌘I | S-SES-009 | | Inserts a new scene directly after the scene last launched, holding copies of the clips that are playing, in their tracks; playback moves to the copies, and the new scene's tempo and signature are set to the current ones (observed in 12.4.3 (macOS), 15 October 2026) |
| M-CREATE-007 | Consolidate Time to New Scene | | | disabled | |
| | — | | | | |
| M-CREATE-008 | Insert Empty MIDI Clip | ⇧⌘M | S-SES-007, S-ARR-037 | disabled | Enabled in the empty slot's and time selection's context menus. A new clip is one bar long in the time signature in force at the time (observed in 12.4.3 (macOS), 15 October 2026) |
| M-CREATE-009 | Capture MIDI | | | disabled | Same as the Control Bar's Capture MIDI button. Disabled until notes have been played, then enabled (observed in 12.4.3 (macOS), 15 October 2026). Runs `midi.capture` ([capture MIDI](../specs/capture-midi.md)) |
| | — | | | | |
| M-CREATE-010 | Create Fade | ⌥⌘F | S-ARR-008 | disabled | The manual names it Create Fade In/Out or Create Crossfade depending on the selection |
| | — | | | | |
| M-CREATE-011 | Slice to New MIDI Track | | | disabled | |
| M-CREATE-012 | Convert Harmony to New MIDI Track | | | disabled | [audio to MIDI](../specs/audio-to-midi.md) R1; acts on one audio clip in either view, disabled otherwise (WP-9.7) |
| M-CREATE-013 | Convert Melody to New MIDI Track | | | disabled | [audio to MIDI](../specs/audio-to-midi.md) R1; acts on one audio clip in either view, disabled otherwise (WP-9.7) |
| M-CREATE-014 | Convert Drums to New MIDI Track | | | disabled | [audio to MIDI](../specs/audio-to-midi.md) R1; acts on one audio clip in either view, disabled otherwise (WP-9.7) |
| M-CREATE-015 | Separate Stems to New Audio Tracks | | | disabled | [stem separation](../specs/stem-separation.md) R22; acts on one audio clip in either view, a time selection holding audio, or one audio file in the browser (WP-9.2) |
| | — | | | | |
| M-CREATE-016 | Add Locator | | | disabled | Reads "Delete Locator" with a locator selected (manual 6.4) |
| M-CREATE-017 | Add Time Signature Change | | | disabled | Reads "Insert Time Signature Change", enabled, when the Arrangement has focus; it adds a marker at the insert marker (observed) |
| | — | | | | |
| M-CREATE-018 | Import Audio/MIDI File... | | | disabled | Reads "Import MIDI File..." with a MIDI track selected (observed) |

Commands the manual places in the Create menu for other focus: Insert Silence (Arrangement, S-ARR-014), Insert Warp Marker and Insert Transient (Sample Editor, S-SAMPLE-009, S-SAMPLE-011), Delete Transient (S-SAMPLE-012).

## Playback

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| M-PLAY-001 | Play | Space | S-TRANS-001 | | Toggles with Stop |
| M-PLAY-002 | Continue Playback | ⇧Space | S-TRANS-002 | | |
| M-PLAY-003 | Play From Insert Marker | ⌥Space | S-ARR-032 | | Shown with the Session View focused; S-TRANS-003 binds the same keys from the manual. With a time selection in the Arrangement it plays the selection and stops at its end (WP-6.74) |
| M-PLAY-004 | Return Play Position to 1.1.1 | | | | No key shown; the manual's Home key (S-TRANS-005) moves the insert marker to the start |
| M-PLAY-005 | Move Insert Marker To Playhead | ⇧⌘Space | S-ARR-033 | | |
| M-PLAY-006 | Options ▸ | | | | Submenu, rows M-PLAY-006a to 006d |
| M-PLAY-006a | Follow | | S-ZOOM-005 | | Same as the Control Bar's Follow switch, which names the key Shift+Opt+F (observed in 12.4.3 (macOS), 15 October 2026) |
| M-PLAY-006b | Start Transport With Record | | | | Also in Settings, Record Warp & Launch |
| M-PLAY-006c | Start Transport With Tap Tempo | | | | Also in Settings, Record Warp & Launch |
| M-PLAY-006d | Link Start Stop Sync | | | | |
| | — | | | | |
| M-PLAY-007 | Record to Arrangement | F9 | S-TRANS-006 | | |
| M-PLAY-008 | Arm Recording to Arrangement | ⇧F9 | S-TRANS-007 | | |
| M-PLAY-009 | Record to Session | ⇧⌘F9 | S-TRANS-008 | | |
| | — | | | | |
| M-PLAY-010 | Metronome | O | S-TRANS-011 | | |
| M-PLAY-011 | Hear Metronome Only While Recording | | | | |
| | — | | | | |
| M-PLAY-012 | Back to Arrangement | F10 | S-TRANS-009 | disabled | Enabled once Session clips have played; it stops the Session clips at once, not on the launch grid, and the transport keeps running (observed in 12.4.3 (macOS), 15 October 2026) |

## View

The View menu is taller than the screen at 1600 × 1000 and scrolls.

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| M-VIEW-001 | Full Screen | ^⌘F | S-VIEW-001 | | |
| M-VIEW-002 | Second Window | ⇧⌘W | S-VIEW-002 | | Beatlayer ticks it while the second window is open (its own; [second window](../specs/second-window.md) R1) |
| M-VIEW-003 | Zoom ▸ | | | | Submenu, rows M-VIEW-003a to 003c (interface zoom) |
| M-VIEW-003a | Zoom In | | S-ZOOM-001 | | |
| M-VIEW-003b | Zoom Out | | S-ZOOM-002 | | |
| M-VIEW-003c | Reset Zoom to 100% | | | | |
| | — | | | | |
| M-VIEW-004 | Info | ? | S-VIEW-008 | ticked | Info View. Whether it is shown is an application setting, not part of the set: hiding it doesn't mark the set as changed, and a new set and a relaunch keep it hidden (observed in 12.4.3 (macOS), 14 October 2026) |
| M-VIEW-005 | Learn View | ⌥⌘7 | S-VIEW-018 | ticked | Built in WP-10.23 ([Learn View](../specs/learn-view.md) R1, R3) |
| M-VIEW-006 | Pack Overview | | | | Ableton pack service; Beatlayer's equivalent is its own content overview or nothing. Left out of Beatlayer's menu (D21) |
| M-VIEW-007 | File Manager | | | | Built in WP-6.63 ([file management](../specs/file-management.md)) |
| M-VIEW-008 | Undo History | ⌥⌘Z | S-VIEW-021 | | Opens in the right-hand column where the Learn View sits (L-OTH-001). Beatlayer ticks it while the panel shows, and running it again hides the panel (its own; [undo history](../specs/undo-history.md) R1) |
| D24 | Assistant | | | | Beatlayer addition (D24): opens and closes the assistant pane (`view.toggle_assistant`); no key |
| | — | | | | |
| M-VIEW-009 | Toggle Arrangement/Session View | ⇥ | S-VIEW-003 | | |
| | — | | | | |
| M-VIEW-010 | Clip View | ⌥⌘3 | S-VIEW-015 | | |
| M-VIEW-011 | Device View | ⌥⌘4 | S-VIEW-016 | ticked | |
| M-VIEW-012 | Toggle Clip/Device View | ⇧⇥ | S-VIEW-004 | | |
| M-VIEW-013 | Expand Clip View | ⌥⌘E | S-NOTE-030 | disabled | Beatlayer enables it while the Clip View shows and ticks it while expanded (its own; [detail views](../specs/detail-views.md) R11) |
| M-VIEW-014 | Hide Device View | ⌥⌘L | S-VIEW-022 | | Label read with the Device View shown; hides the detail area. Beatlayer's labels in other states: Hide Clip View, Hide Clip and Device View, and Show Device View, Show Clip View or Show Clip and Device View for what it would bring back (its own words; [detail views](../specs/detail-views.md) R6) |
| | — | | | | |
| M-VIEW-015 | Arrange Clip View Panels Automatically | | | ticked | One of three choices (015 to 017). Live from WP-6.72 ([detail views](../specs/detail-views.md) R20); only Arrange Clip View Panels Automatically is in the Clip View title bar's menu (C-CLIPTITLE-003) |
| M-VIEW-016 | Arrange Clip View Panels Horizontally | | | | As M-VIEW-015 |
| M-VIEW-017 | Arrange Clip View Panels Vertically | | | | As M-VIEW-015 |
| | — | | | | |
| M-VIEW-018 | Content Editor | ⌥⇧1 | S-CLIPMODE-003 | ticked | Sample Editor or MIDI Note Editor |
| M-VIEW-019 | Envelope Editor | ⌥⇧2 | S-CLIPMODE-004 | disabled | |
| M-VIEW-020 | Expression Editor | ⌥⇧3 | S-CLIPMODE-005 | disabled | MPE editor |
| | — | | | | |
| M-VIEW-021 | Browser | ⌥⌘5 | S-VIEW-010 | ticked | |
| M-VIEW-022 | Groove Pool | ⌥⌘6 | S-VIEW-017 | | |
| M-VIEW-023 | Tuning | | | | Tuning section of the browser column |
| M-VIEW-024 | Full-Height Browser | | | | Built in WP-6.32 ([browser](../specs/browser.md) R26) |
| | — | | | | |
| M-VIEW-025 | Search in Browser | ⌘F | S-BRW-006 | | |
| M-VIEW-026 | Show Similar Files | ⇧⌘F | S-BRW-010 | disabled | [sound similarity](../specs/sound-similarity.md) R11; built in WP-9.9 |
| M-VIEW-027 | Show Filter View | ⌥⌘G | S-BRW-013 | disabled | Built in WP-6.32 ([browser tags](../specs/browser-tags.md) R18) |
| | — | | | | |
| M-VIEW-028 | Plug-In Windows | ⌥⌘P | S-DEV-009 | ticked, disabled | Beatlayer's floating native devices' windows (IMP-14) follow it too (WP-2.5b), with every plug-in editor window the device's window button opens (L-DEV-010, WP-2.10) |
| M-VIEW-029 | Video Window | ⌥⇧⌘V | S-VIEW-009 | ticked, disabled | Not the manual's ⌥⌘V, which Live shows for Paste Bounced Audio (M-EDIT-031) |
| | — | | | | |
| M-VIEW-030 | Show the Max for Live Window | ⌥⇧⌘M | S-VIEW-023 | | Max-specific; Beatlayer's extensions are managed elsewhere ([extensions](../specs/extensions.md) R6). Left out of Beatlayer's menu (D21) |
| | — | | | | |
| M-VIEW-031 | Mixer | ⌥⌘M | S-VIEW-014 | ticked | |
| M-VIEW-032 | Mixer Controls ▸ | | | | Submenu, rows M-VIEW-032a to 032g; observed in 12.4.6, 11 October 2026. Each view's mixer keeps its own sections (release notes 12.0; [mixer view](../specs/mixer-view.md) R37) |
| M-VIEW-032a | In/Out | ⌥⌘I | S-VIEW-012 | ticked | |
| M-VIEW-032b | Sends | ⌥⌘S | S-VIEW-013 | ticked | |
| M-VIEW-032c | Volume | | | ticked | |
| | — | | | | |
| M-VIEW-032d | Track Options | | | | Beatlayer's Track Options section; live since WP-5.36 ([mixer](../specs/mixer.md) R28, R30) |
| M-VIEW-032e | Crossfader | | | | |
| M-VIEW-032f | Performance Impact | | | | |
| IMP-25 | Devices | | | | Beatlayer addition: shows the Devices section of the view in front's mixer, unticked on a new install ([mixer view](../specs/mixer-view.md) R24) |
| | — | | | | |
| M-VIEW-032g | Return Tracks | ⌥⌘R | S-TRACK-011 | ticked | |
| IMP-01 | Return Tracks Scroll with Tracks | | | | Beatlayer addition, unticked on a new install; one switch, shown in both this submenu and Arrangement Track Controls ([mixer view](../specs/mixer-view.md) R21) |
| M-VIEW-033 | Arrangement Track Controls ▸ | | | disabled | Submenu, rows M-VIEW-033a to 033d; observed in 12.4.6, 11 October 2026: disabled while the Session View is in front, no keys |
| M-VIEW-033a | In/Out | | | ticked | Hides the track headers' In/Out sub-column ([arrangement view](../specs/arrangement-view.md) R21) |
| M-VIEW-033b | Volume | | | ticked | Hides the second and third rows of the headers' mixer sub-column ([arrangement view](../specs/arrangement-view.md) R22) |
| M-VIEW-033c | Track Options | | | | As M-VIEW-032d; live since WP-5.36 |
| | — | | | | |
| M-VIEW-033d | Return Tracks | | | ticked | Hides the return tracks' rows in the lanes ([arrangement view](../specs/arrangement-view.md) R23) |
| IMP-01 | Return Tracks Scroll with Tracks | | | | As in Mixer Controls: the same switch |
| | — | | | | |
| M-VIEW-034 | Overview | ⌥⌘O | S-VIEW-011 | | |
| IMP-22 | Arranger Track | | | | Beatlayer addition: shows the arranger track (L-ARR-023; `arrangement.arranger_track`), unticked in a new set; no key ([arranger track](../specs/arranger-track.md) R1) |
| IMP-33 | Chord Lane | | | | Beatlayer addition: shows the chord lane (L-ARR-024; `view.chord_lane`), unticked in a new set; no key ([chord lane](../specs/chord-lane.md) R1) |
| M-VIEW-035 | Scene Tempo and Time Signature | | | | Shows the Main track's scene tempo and signature fields |
| | — | | | | |
| M-VIEW-036 | Automation Mode | A | S-ENV-001 | disabled | |
| M-VIEW-037 | Show Grouped Tracks | ⌥U | S-ARR-021 | disabled | |
| M-VIEW-038 | Show Selected Grouped Track | U | S-TRACK-010 | disabled | |
| IMP-34 | Mark Empty Tracks | | | | Beatlayer addition: a switch, unticked on a new install ([view options](../specs/view-options.md) R6) |
| | — | | | | |
| M-VIEW-039 | Fold to Notes | F | S-NOTE-038 | disabled | |
| M-VIEW-040 | Fold to Scale | G | S-NOTE-039 | disabled | |
| IMP-35 | Note Colors ▸ | | | | Beatlayer addition: a submenu of By Velocity (ticked on a new install), By Pitch Class and By Scale Degree ([view options](../specs/view-options.md) R2) |
| | — | | | | |
| M-VIEW-041 | Show Chains | + | S-DEV-012 | disabled | Rack chain list |
| M-VIEW-042 | Hide Chains | − | S-DEV-013 | disabled | |
| M-VIEW-043 | Zoom to Time Selection | Z | S-ARR-030 | disabled | |
| M-VIEW-044 | Zoom Back from Time Selection | X | S-ARR-031 | disabled | The menu's last item |

## Navigate

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| M-NAV-001 | Control Bar | ⌥0 | S-FOCUS-001 | | |
| M-NAV-002 | Session View | ⌥1 | S-FOCUS-002 | | |
| M-NAV-003 | Arrangement View | ⌥2 | S-FOCUS-003 | | Moves keyboard focus to the Arrangement. While a value field is being edited, ⌥2 types 2 into the field instead (observed) |
| M-NAV-004 | Clip View | ⌥3 | S-FOCUS-004 | | |
| M-NAV-005 | Device View | ⌥4 | S-FOCUS-005 | | |
| M-NAV-006 | Browser | ⌥5 | S-FOCUS-006 | | |
| M-NAV-007 | Groove Pool | ⌥6 | S-FOCUS-007 | | |
| M-NAV-008 | Learn View | ⌥7 | S-FOCUS-008 | | Built in WP-10.23 ([Learn View](../specs/learn-view.md) R1, R3) |
| M-NAV-009 | Tuning | | | | |
| | — | | | | |
| M-NAV-010 | Mixer | ⌥⇧M | S-ARR-035 | disabled | In the Arrangement, puts the keyboard focus in the mixer at the selected track; Right then moves the selection to the next track (observed in 12.4.6 (macOS), 11 October 2026) |
| M-NAV-011 | Clip Panels | ⌥⇧P | S-FOCUS-010 | disabled | |
| | — | | | | |
| M-NAV-012 | Previous | | S-FOCUS-012 | | No key shown: Shift+Tab moves focus only with Use Tab Key to Move Focus on |
| M-NAV-013 | Next | | S-FOCUS-011 | | No key shown: Tab moves focus only with Use Tab Key to Move Focus on |
| M-NAV-014 | Previous Neighbor | ⌥⇧⇥ | S-FOCUS-014 | disabled | Enabled whenever the track with the focus has a previous neighbour, whatever the Tab setting: with Use Tab Key to Move Focus off and track 1 selected, disabled (observed in 12.4.6, 11 October 2026) |
| M-NAV-015 | Next Neighbor | ⌥⇥ | S-FOCUS-013 | disabled | Enabled whenever there's a next track: with Use Tab Key to Move Focus off and track 1 selected, enabled; it moved the focus to the same control on the next track without changing the selected track (observed in 12.4.6, 11 October 2026) |
| IMP-18 | Find Track | ⇧⌘O | S-FOCUS-015 | | Beatlayer addition: opens the search field (layout.md's IMP-16 row) on its Tracks scope; straight after Next Neighbor ([command palette](../specs/command-palette.md) R1) |
| | — | | | | |
| M-NAV-016 | Browser History Back | ⌘[ | S-BRW-011 | disabled | Built in WP-6.32 ([browser tags](../specs/browser-tags.md) R30) |
| M-NAV-017 | Browser History Forward | ⌘] | S-BRW-012 | disabled | Built in WP-6.32 ([browser tags](../specs/browser-tags.md) R30) |
| | — | | | | |
| M-NAV-018 | Use Tab Key to Move Focus | | | | Switch; also in Settings, Display & Input. Off on this install (observed in 12.4.6, 11 October 2026) |
| M-NAV-019 | Wrap Keyboard Navigation | | | | Switch, ticked on this install (observed in 12.4.6, 11 October 2026); Settings calls it Wrap Tab Navigation |

## Options

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| M-OPT-001 | Accessibility ▸ | | | | Submenu, rows M-OPT-001a to 001e |
| M-OPT-001a | Speak Menu Commands | | | | Ticked on this install (observed in 12.4.6, 11 October 2026) |
| M-OPT-001b | Speak Minimum and Maximum Sliders Values | | | | |
| M-OPT-001c | Speak Time in Seconds | | | | |
| M-OPT-001d | Speak Parameter States | | | | |
| M-OPT-001e | Speak Extended MIDI Note Description | | | | Ticked on this install (observed in 12.4.6, 11 October 2026) |
| | — | | | | |
| M-OPT-002 | Edit MIDI Map | ⌘M | S-MAP-001 | | |
| M-OPT-003 | Edit Key Map | ⌘K | S-MAP-002 | | |
| M-OPT-004 | Computer MIDI Keyboard | M | S-MAP-003 | | |
| M-OPT-005 | MIDI Note Editor Preview | | | | The editor's Preview switch (L-CLIP-012) |
| M-OPT-006 | Browser File Preview | | | | Same as the browser's preview switch |
| | — | | | | |
| M-OPT-007 | External Sync | | | disabled | Enabled while any MIDI input has Sync on, and ticked while EXT (L-CTRL-028) is on ([MIDI clock](../specs/midi-clock.md)) |
| M-OPT-008 | Delay Compensation | | | ticked | On in a new set; Beatlayer's switch ([delay compensation](../specs/delay-compensation.md) R7), live since WP-5.36 |
| M-OPT-009 | Reduced Latency When Monitoring | | | | Enabled only while Delay Compensation is on; Beatlayer's switch ([delay compensation](../specs/delay-compensation.md) R8), live since WP-5.36 |
| | — | | | | |
| M-OPT-010 | Time Ruler Format ▸ | | | | Submenu, rows M-OPT-010a to 010f |
| M-OPT-010a | Time | | | | |
| M-OPT-010b | 24 fps (Film) | | | | |
| M-OPT-010c | 25 fps (PAL) | | | | |
| M-OPT-010d | 30 fps | | | | |
| M-OPT-010e | 29.97 fps (Drop Frame / NTSC-TV) | | | | |
| M-OPT-010f | 29.97 fps (Non-Drop / NTSC) | | | | |
| | — | | | | |
| M-OPT-011 | Smaller Launch Quantization | ⌘1 | S-SES-024 | | With the Session View focused. In the Arrangement the same keys are Narrow Grid (S-GRID-002), as its context menus show |
| M-OPT-012 | Larger Launch Quantization | ⌘2 | S-SES-025 | | As M-OPT-011; Widen Grid (S-GRID-003) in the Arrangement |
| M-OPT-013 | Triplet Launch Quantization | ⌘3 | S-SES-026 | disabled | As M-OPT-011; Triplet Grid (S-GRID-004) in the Arrangement |
| M-OPT-014 | Launch Quantization Enabled | ⌘4 | S-SES-027 | | As M-OPT-011; Snap to Grid (S-GRID-005) in the Arrangement |
| M-OPT-015 | Snap Automation to Grid | | | ticked | Observed disabled in a new set; Beatlayer's is live and on in a new install ([automation](../specs/automation.md) R31) |
| M-OPT-016 | Fixed Grid | ⌘5 | S-GRID-006 | disabled | |
| | — | | | | |
| M-OPT-017 | Draw Mode | B | S-GRID-001 | | Reads "Draw Mode (Pitch Lock On/Off)" with the MIDI Note Editor focused (manual 10.4.1) |
| M-OPT-018 | Step Input Mode | ⇧⌘B | S-NOTE-044 | disabled | A command, never ticked (release notes 12.0) |
| | — | | | | |
| M-OPT-019 | Highlight Scales | K | S-NOTE-035 | | |
| M-OPT-020 | Chase MIDI Notes | | | | |
| M-OPT-021 | MIDI Envelope Auto-Reset | | | | |
| M-OPT-022 | Retune Set On Loading Tuning Systems | | | | |
| | — | | | | |
| M-OPT-023 | Solo Switches | | | disabled | One of two choices with M-OPT-024, and one setting with the Main strip's Solo/Cue button; both are greyed while Cue Out and Main Out share a channel, with this one ticked |
| M-OPT-024 | Cue Switches | | | disabled | |
| M-OPT-025 | Solo in Place | | | | |
| | — | | | | |
| M-OPT-026 | Lock Envelopes | | | | Same as the Arrangement's Lock Envelopes switch |
| M-OPT-027 | MIDI Arrangement Overdub | | | | Same as the Control Bar's overdub switch |
| | — | | | | |
| M-OPT-028 | Max for Live Developer Mode | | | | Max-specific; see M-VIEW-030. Left out of Beatlayer's menu (D21) |
| | — | | | | |
| M-OPT-029 | Audio Engine On | ⌥⇧⌘E | S-ENGINE-001 | | |

On Windows the Options menu also holds Settings... (S-VIEW-019), per manual 2.3.

## Help

On macOS the system adds a search field at the top of the Help menu (S-MENU-005).

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| IMP-16 | Search Commands… | ⇧⌘P | S-MENU-007 | | Beatlayer addition: opens the search field on its Commands scope; the menu's first item, after the system's search field, with a separator after it ([command palette](../specs/command-palette.md) R1) |
| M-HELP-001 | Learn View | | S-VIEW-018 | | No key shown here; the View menu shows ⌥⌘7; built in WP-10.23 ([Learn View](../specs/learn-view.md) R1, R3) |
| M-HELP-002 | Pack Overview | | | | Ableton service; see M-VIEW-006. Left out of Beatlayer's menu (D21) |
| | — | | | | |
| M-HELP-003 | Load Demo Set | | | | Beatlayer: its own demo song, opened as an untitled copy ([Help menu](../specs/help-menu.md) R15); built in WP-10.24 |
| M-HELP-004 | Read the Live Manual... | | | | Beatlayer: Read the Beatlayer Manual... (D21); its own manual, bundled and opened in the browser ([Help menu](../specs/help-menu.md) R2); built in WP-10.19 |
| M-HELP-005 | Learn Live at ableton.com... | | | | Ableton website; Beatlayer links its own site. Left out of Beatlayer's menu (D21) |
| | — | | | | |
| M-HELP-006 | Join the User Forum... | | | | Beatlayer: its own community link: the project's forum page, a pinned address ([Help menu](../specs/help-menu.md) R3); built in WP-10.19 |
| M-HELP-007 | Get Support | | | | Beatlayer: its issue tracker, a pinned address ([Help menu](../specs/help-menu.md) R3); built in WP-10.19 |
| M-HELP-008 | User Account and Licenses... | | | | Ableton account; no Beatlayer equivalent (open source, no licence keys). Left out of Beatlayer's menu (D21) |
| M-HELP-009 | Purchase License... | | | | Ableton store; no Beatlayer equivalent. Left out of Beatlayer's menu (D21) |
| | — | | | | |
| M-HELP-010 | Check for Updates... | | | | Beatlayer's updater ([updates](../specs/updates.md) R9); built in WP-10.17 |

## Control Bar menus

Two Control Bar controls open menus of their own (observed in 12.4.3 (macOS), 13 and 15 October 2026). The Metronome Settings menu's choices are written group: choice.

### Quantization menu (L-CTRL-010)

The global launch quantisation, in Live's order.

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| M-CTRL-001 | None | ⌘0 | S-GQ-005 | | |
| M-CTRL-002 | 8 Bars | | | | |
| M-CTRL-003 | 4 Bars | | | | |
| M-CTRL-004 | 2 Bars | | | | |
| M-CTRL-005 | 1 Bar | ⌘9 | S-GQ-004 | | The default |
| M-CTRL-006 | 1/2 | | | | |
| M-CTRL-007 | 1/2T | | | | |
| M-CTRL-008 | 1/4 | ⌘8 | S-GQ-003 | | |
| M-CTRL-009 | 1/4T | | | | |
| M-CTRL-010 | 1/8 | ⌘7 | S-GQ-002 | | |
| M-CTRL-011 | 1/8T | | | | |
| M-CTRL-012 | 1/16 | ⌘6 | S-GQ-001 | | |
| M-CTRL-013 | 1/16T | | | | |
| M-CTRL-014 | 1/32 | | | | |

### Metronome Settings menu (L-CTRL-009)

Opened by the small arrow beside the metronome switch. It has three groups of single choices, each with a tick, and one switch.

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| M-CTRL-015 | Count-In: None | | | ticked | The default of the Count-In group |
| M-CTRL-016 | Count-In: 1 Bar | | | | |
| M-CTRL-017 | Count-In: 2 Bars | | | | |
| M-CTRL-018 | Count-In: 4 Bars | | | | |
| M-CTRL-019 | Sound: Classic | | | ticked | The default of the Sound group |
| M-CTRL-020 | Sound: Click | | | | |
| M-CTRL-021 | Sound: Wood | | | | |
| M-CTRL-022 | Rhythm: Auto | | | ticked | The default of the Rhythm group |
| M-CTRL-023 | Rhythm: 1 Bar | | | | |
| M-CTRL-024 | Rhythm: 1/2 | | | | |
| M-CTRL-025 | Rhythm: 1/2T | | | | |
| M-CTRL-026 | Rhythm: 1/4 | | | | |
| M-CTRL-027 | Rhythm: 1/4T | | | | |
| M-CTRL-028 | Rhythm: 1/8 | | | | |
| M-CTRL-029 | Rhythm: 1/8T | | | | |
| M-CTRL-030 | Rhythm: 1/16 | | | | |
| M-CTRL-031 | Rhythm: 1/16T | | | | |
| M-CTRL-032 | Enable Only While Recording | | | | A switch, off by default |

## Context menus

Context menus open on right-click (or Ctrl-click on macOS) and from the keyboard (S-CTX-001, S-CTX-002).

**Live draws its context menus itself** rather than using the system's menus. They take Live's theme, write keys their own way (Del for the Delete key, Enter for Return, where the Edit menu shows ⌫ and ↩), lay some choices out as grids (the Arrangement's grid settings), and the track header, scene, and Session clip menus end with a colour palette of 5 rows of 14 swatches. Beatlayer draws its context menus the same way, with its own colours.

Each menu below, up to the crossfader's, was opened once in a new set on 10 October 2026 (the crossfader's on 15 October 2026), on the object named; its rows are in Live's order, with separators, keys shown, and state as in the menu-bar tables. **Other states** lists commands the manual places in the same menu for other objects or selections (audio clips, group tracks, automation lanes); their source is the manual. The menus after the crossfader's that name Live 12.4.16 and 2 November 2026 were opened then, in a new set: their rows are in Live's order with its separators, headings ("Adaptive Grid:" and the like, dim text that is no item), keys shown and dim states, and the Source column says `obs`. Beatlayer's menus match them; an item Live names that Beatlayer has no feature for is shown dim, and an item Beatlayer had that Live's menu hasn't is not in the menu (its command and key stay). The menus that follow and say they weren't opened in Live give their source and keep Beatlayer's own order.

### Session clip slot, empty (C-SLOT)

Opened on an empty slot of a MIDI track. No Paste item showed while Edit > Paste was disabled.

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| C-SLOT-001 | Freeze Track | ⌥⇧⌘F | S-TRACK-017 | | |
| C-SLOT-002 | Bounce Track in Place | | | | |
| C-SLOT-003 | Bounce to New Track | ⌘B | S-BOUNCE-001 | disabled | |
| C-SLOT-004 | Paste Bounced Audio | ⌥⌘V | S-BOUNCE-002 | disabled | |
| | — | | | | |
| C-SLOT-005 | Crop Clip | ⇧⌘J | S-SES-028 | disabled | |
| | — | | | | |
| C-SLOT-006 | Remove Stop Button | ⌘E | S-SES-005 | | Reads "Add Stop Button" on a slot without one (manual 7.4.2) |
| C-SLOT-007 | Insert Empty MIDI Clip | ⇧⌘M | S-SES-007 | | MIDI tracks |

Other states:

| ID | Command | Source |
|---|---|---|
| C-SLOT-008 | Paste (with something to paste); Beatlayer puts it first, followed by a separator ([session context menus](../specs/session-context-menus.md) R1) | manual 7.4 |
| C-SLOT-009 | Bounce Group to New Track (group slots); Beatlayer puts it after Bounce to New Track (C-SLOT-003) ([session context menus](../specs/session-context-menus.md) R1) | manual 20.2 |

### Track header (C-TRACK)

Opened on a MIDI track's title bar.

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| C-TRACK-001 | Cut | ⌘X | S-EDIT-001 | | |
| C-TRACK-002 | Copy | ⌘C | S-EDIT-002 | | |
| C-TRACK-003 | Duplicate | ⌘D | S-EDIT-004 | | |
| C-TRACK-004 | Delete | Del | S-TRACK-018 | | |
| | — | | | | |
| C-TRACK-005 | Rename | ⌘R | S-TRACK-004 | | |
| C-TRACK-006 | Edit Info Text | | | | |
| | — | | | | |
| C-TRACK-007 | Freeze Track | ⌥⇧⌘F | S-TRACK-017 | | |
| C-TRACK-008 | Bounce Track in Place | | | | |
| | — | | | | |
| C-TRACK-009 | Insert Audio Track | ⌘T | S-TRACK-001 | | |
| C-TRACK-010 | Insert MIDI Track | ⇧⌘T | S-TRACK-002 | | |
| C-TRACK-011 | Insert Return Track | ⌥⌘T | S-TRACK-003 | | |
| C-TRACK-012 | Group Tracks | ⌘G | S-TRACK-006 | | |
| | — | | | | |
| C-TRACK-013 | Select Track Content | | | | |
| | — | | | | |
| C-TRACK-014 | Save as Default MIDI Track | | | | Names the track's kind |
| | — | | | | |
| C-TRACK-015 | Colour palette | | | | 5 rows of 14 swatches |
| | — | | | | |
| C-TRACK-016 | Assign Track Color to Clips | | | | |

Other states:

| ID | Command | Source |
|---|---|---|
| C-TRACK-017 | Ungroup Tracks; Assign Track Color to Grouped Tracks and Clips; Bounce Group to New Track (group tracks) | manual 18.3, 8.1.1.3, 20 |
| C-TRACK-018 | Link Tracks, Unlink Track(s) (several tracks selected, or a group track's header) | manual 6.14.1 |
| C-TRACK-019 | Show Take Lanes, Insert Take Lane, Delete All Take Lanes, Delete All Unused Take Lanes (Arrangement) | manual 21.1, 21.2; release notes 12.2 |
| C-TRACK-020 | Crop Clip (from a time selection in the track) | manual 22.2 |
| IMP-33 | Follow Chord Lane, ticked while the track follows the chord lane (a MIDI track; after Save as Default MIDI Track, in a group of its own) | Beatlayer ([chord lane](../specs/chord-lane.md) R5) |

### Scene (C-SCENE)

Opened on a scene in the Main track's column. Beatlayer opens the same menu from the Scene View's title bar for the selected scenes (its own; [session context menus](../specs/session-context-menus.md) R3).

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| C-SCENE-001 | Cut | ⌘X | S-EDIT-001 | | |
| C-SCENE-002 | Copy | ⌘C | S-EDIT-002 | | |
| C-SCENE-003 | Duplicate | ⌘D | S-EDIT-004 | | |
| C-SCENE-004 | Delete | Del | S-EDIT-005 | | |
| | — | | | | |
| C-SCENE-005 | Rename | | S-EDIT-008 | | No key shown here, unlike the track and clip menus |
| C-SCENE-006 | Edit Info Text | | | | |
| | — | | | | |
| C-SCENE-007 | Insert Scene | ⌘I | S-SES-008 | | |
| C-SCENE-008 | Capture and Insert Scene | ⇧⌘I | S-SES-009 | | |
| | — | | | | |
| C-SCENE-009 | Select All Slots in Scene | | | | |
| | — | | | | |
| C-SCENE-010 | Cancel Scene Launch | | | | |
| | — | | | | |
| C-SCENE-011 | Select Next Scene on Launch (Pref) | | | ticked | "(Pref)" is part of the label: the Settings option of the same name (L-SET-018) |
| C-SCENE-012 | Trigger Recording on Launch (Pref) | | | | Settings has an option of a similar name, Start Recording on Scene Launch (L-SET-018) |
| | — | | | | |
| C-SCENE-013 | Scene Tempo and Time Signature | | | | As M-VIEW-035 |
| | — | | | | |
| C-SCENE-014 | No Color, then the colour palette | | | | 5 rows of 14 swatches |

Other states:

| ID | Command | Source |
|---|---|---|
| C-SCENE-015 | Paste (with something to paste); Beatlayer puts it straight after Copy ([session context menus](../specs/session-context-menus.md) R3) | manual 7.2 |
| C-SCENE-016 | Return to Default (on the scene tempo and signature fields) | manual 7.2.1 |

### Browser item (C-BROWSER)

Opened on a preset under the Sounds label.

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| C-BROWSER-001 | Show in Places | | | | |
| C-BROWSER-002 | Show in Finder | | | disabled | macOS |
| C-BROWSER-003 | Show Similar Files | ⇧⌘F | S-BRW-010 | | [sound similarity](../specs/sound-similarity.md) R11; built in WP-9.9 |
| | — | | | | |
| C-BROWSER-004 | Clear All Colors | 0 | S-BRW-009 | | |
| C-BROWSER-005 | Favorites | 1 | S-BRW-008 | | Collection labels 005 to 011 take the keys 1 to 7 |
| C-BROWSER-006 | Orange | 2 | S-BRW-008 | | |
| C-BROWSER-007 | Yellow | 3 | S-BRW-008 | | |
| C-BROWSER-008 | Green | 4 | S-BRW-008 | | |
| C-BROWSER-009 | Blue | 5 | S-BRW-008 | | |
| C-BROWSER-010 | Purple | 6 | S-BRW-008 | | |
| C-BROWSER-011 | Gray | 7 | S-BRW-008 | | |

Other states:

| ID | Command | Source |
|---|---|---|
| C-BROWSER-012 | Rename, Delete (user files) | manual 4, 5.8 |
| C-BROWSER-013 | Separate Stems to New Audio Tracks (audio files); Beatlayer puts it after Show Similar Files ([stem separation](../specs/stem-separation.md) R22, WP-9.2) | manual 22.2 |
| C-BROWSER-014 | Manage Project (project folders); built in WP-6.63 | manual 5.5.3 |
| C-BROWSER-015 | Set Default Live Set (template sets) | manual 5.4.5 |
| C-BROWSER-016 | Show Pack Info; Download, Pause, Resume, Cancel, Install (packs) | manual 4.7 |
| C-BROWSER-017 | Locate Folder, Remove from Sidebar (user folders) | manual 4.7.8 |
| C-BROWSER-018 | Rename label, choose label icon, remove custom label (sidebar labels) | manual 4.2.1 |
| C-BROWSER-019 | Hide Plug-In, or Show Plug-In for a hidden one, at the end of a plug-in's menu (Beatlayer's own, IMP-07; [browser tags](../specs/browser-tags.md) R41) | Beatlayer |
| C-BROWSER-020 | New Collection, Rename, Delete (a collection's label; New Collection, and Delete on a nested collection, are Beatlayer's own, IMP-07; [browser tags](../specs/browser-tags.md) R38, R45, R47) | manual 4.5 (Rename); Beatlayer |
| C-BROWSER-021 | Reset Label Order (the Library heading; Beatlayer's own name, [browser tags](../specs/browser-tags.md) R26) | release notes 12.1 |

### Clip, in the Session View (C-CLIP-SES)

Opened on an empty MIDI clip on track 1.

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| C-CLIP-SES-001 | Cut | ⌘X | S-EDIT-001 | | |
| C-CLIP-SES-002 | Copy | ⌘C | S-EDIT-002 | | |
| C-CLIP-SES-003 | Duplicate | ⌘D | S-EDIT-004 | | Beatlayer adds Duplicate as Linked Clip and then Unlink Clip straight after it, with no keys (IMP-21; [linked clips](../specs/linked-clips.md) R1, R8) |
| C-CLIP-SES-004 | Delete | Del | S-EDIT-005 | | |
| | — | | | | |
| C-CLIP-SES-005 | Rename | ⌘R | S-EDIT-008 | | |
| C-CLIP-SES-006 | Edit Info Text | | | | |
| C-CLIP-SES-007 | Extract Groove(s) | | | | |
| C-CLIP-SES-008 | Deactivate Clip | 0 | S-SES-018 | | |
| C-CLIP-SES-009 | Export MIDI Clip... | | S-FILE-008 | | MIDI clips. No key shown |
| | — | | | | |
| C-CLIP-SES-010 | Freeze Track | ⌥⇧⌘F | S-TRACK-017 | | |
| C-CLIP-SES-011 | Bounce Track in Place | | | | |
| C-CLIP-SES-012 | Bounce to New Track | ⌘B | S-BOUNCE-001 | | |
| C-CLIP-SES-013 | Paste Bounced Audio | ⌥⌘V | S-BOUNCE-002 | disabled | |
| | — | | | | |
| C-CLIP-SES-014 | Crop Clip | ⇧⌘J | S-SES-028 | | |
| | — | | | | |
| C-CLIP-SES-015 | Remove Stop Button | ⌘E | S-SES-005 | | |
| C-CLIP-SES-016 | Create Follow Action Chain | ⇧⌘Enter | S-SES-014 | | |
| | — | | | | |
| C-CLIP-SES-017 | Select on Launch (Pref) | | | ticked | "(Pref)" is part of the label: the Settings option of the same name (L-SET-018) |
| | — | | | | |
| C-CLIP-SES-018 | Colour palette | | | | 5 rows of 14 swatches |

Other states:

| ID | Command | Source |
|---|---|---|
| C-CLIP-SES-019 | Slice to New MIDI Track and the three Convert ... to New MIDI Track commands (audio clips); Beatlayer puts them after Crop Clip and a separator, in C-CLIP-ARR-027's order ([session context menus](../specs/session-context-menus.md) R2); the Convert commands built in WP-9.7, each running `audio_to_midi.convert` with its kind | manual 13 |
| C-CLIP-SES-020 | Separate Stems to New Audio Tracks (audio clips); after C-CLIP-SES-019, as C-CLIP-ARR-027d follows the Convert items; built in WP-9.2 | manual 22.2 |

### Clip, in the Arrangement (C-CLIP-ARR)

Opened on an empty MIDI clip on track 1, bars 1 to 5. No colour palette was recorded in this menu, unlike the Session clip's.

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| C-CLIP-ARR-001 | Cut | ⌘X | S-EDIT-001 | | |
| C-CLIP-ARR-002 | Copy | ⌘C | S-EDIT-002 | | |
| C-CLIP-ARR-003 | Duplicate | ⌘D | S-EDIT-004 | | Beatlayer adds Duplicate as Linked Clip and then Unlink Clip straight after it, with no keys (IMP-21; [linked clips](../specs/linked-clips.md) R1, R8) |
| C-CLIP-ARR-004 | Delete | Del | S-EDIT-005 | | |
| | — | | | | |
| C-CLIP-ARR-005 | Zoom to Time Selection | Z | S-ARR-030 | | |
| C-CLIP-ARR-006 | Zoom Back from Time Selection | X | S-ARR-031 | | |
| | — | | | | |
| C-CLIP-ARR-007 | Rename | ⌘R | S-EDIT-008 | | |
| C-CLIP-ARR-008 | Edit Info Text | | | | |
| C-CLIP-ARR-009 | Extract Groove(s) | | | | |
| C-CLIP-ARR-010 | Deactivate Clip | 0 | S-ARR-027 | | |
| C-CLIP-ARR-011 | Export MIDI Clip... | | S-FILE-008 | | MIDI clips. No key shown |
| | — | | | | |
| C-CLIP-ARR-012 | Loop Selection | ⌘L | S-ARR-011 | | |
| | — | | | | |
| C-CLIP-ARR-013 | Freeze Track | ⌥⇧⌘F | S-TRACK-017 | | |
| C-CLIP-ARR-014 | Bounce Track in Place | | | | |
| C-CLIP-ARR-015 | Bounce to New Track | ⌘B | S-BOUNCE-001 | | |
| C-CLIP-ARR-016 | Paste Bounced Audio | ⌥⌘V | S-BOUNCE-002 | disabled | |
| | — | | | | |
| C-CLIP-ARR-017 | Crop Clip | ⇧⌘J | S-ARR-003 | | |
| C-CLIP-ARR-018 | Reverse Clip(s) | R | S-ARR-029 | | Offered for a MIDI clip too |
| | — | | | | |
| C-CLIP-ARR-019 | Snap to Grid | ⌘4 | S-GRID-005 | ticked | |
| C-CLIP-ARR-020 | Adaptive Grid: Widest, Wide, Medium, Narrow, Narrowest | | | Narrow ticked | One choice of five, laid out as a grid of three columns |
| C-CLIP-ARR-021 | Fixed Grid: 8 Bars, 4 Bars, 2 Bars, 1 Bar, 1/2, 1/4, 1/8, 1/16, 1/32 | | | | One choice of nine, laid out as a grid of five columns |
| | — | | | | |
| C-CLIP-ARR-022 | Narrow Grid | ⌘1 | S-GRID-002 | disabled | |
| C-CLIP-ARR-023 | Widen Grid | ⌘2 | S-GRID-003 | | |
| | — | | | | |
| C-CLIP-ARR-024 | Triplet Grid | ⌘3 | S-GRID-004 | disabled | |

In the Arrangement, ⌘1 to ⌘4 are the grid and snap commands; the Options menu lists the same keys as launch quantisation with the Session View focused (M-OPT-011 to 014).

Other states:

| ID | Command | Source |
|---|---|---|
| C-CLIP-ARR-025 | Split | manual 6.12 |
| C-CLIP-ARR-026 | Consolidate | manual 6.13 |
| C-CLIP-ARR-027 | Slice and Convert to New MIDI Track commands, Separate Stems to New Audio Tracks (audio clips) | manual 13, 22 |
| C-CLIP-ARR-027e | Slice to New MIDI Track (audio clips) | manual 13; observed in Live 12 on macOS, 18 October 2026: an audio clip's menu, first in that group |
| C-CLIP-ARR-027a | Convert Harmony to New MIDI Track (audio clips) | observed in Live 12 on macOS, 18 October 2026: an audio clip's menu, after a separator below Reverse Clip(s) |
| C-CLIP-ARR-027b | Convert Melody to New MIDI Track (audio clips) | observed, as C-CLIP-ARR-027a, next |
| C-CLIP-ARR-027c | Convert Drums to New MIDI Track (audio clips) | observed, as C-CLIP-ARR-027a, next |
| C-CLIP-ARR-027d | Separate Stems to New Audio Tracks (audio clips) | observed, as C-CLIP-ARR-027a, last in that group, before the clip colours |
| C-CLIP-ARR-028 | The clip colours, last in the menu, after a separator | observed in Live 12 on macOS, 18 October 2026: an audio clip's menu, after C-CLIP-ARR-027d |

### Arrangement time selection (C-TIME)

Opened on a time selection in an empty lane, MIDI track 2, bars 9 to 13. The menu has no Paste or Delete with this selection.

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| C-TIME-001 | Cut | ⌘X | S-EDIT-001 | | |
| C-TIME-002 | Copy | ⌘C | S-EDIT-002 | | |
| C-TIME-003 | Duplicate | ⌘D | S-EDIT-004 | | |
| | — | | | | |
| C-TIME-004 | Zoom to Time Selection | Z | S-ARR-030 | | |
| C-TIME-005 | Zoom Back from Time Selection | X | S-ARR-031 | | |
| | — | | | | |
| C-TIME-006 | Insert Empty MIDI Clip | ⇧⌘M | S-ARR-037 | | MIDI tracks |
| | — | | | | |
| C-TIME-007 | Loop Selection | ⌘L | S-ARR-011 | | |
| | — | | | | |
| C-TIME-008 | Freeze Track | ⌥⇧⌘F | S-TRACK-017 | | |
| C-TIME-009 | Bounce Track in Place | | | | |
| C-TIME-010 | Bounce to New Track | ⌘B | S-BOUNCE-001 | disabled | |
| C-TIME-011 | Paste Bounced Audio | ⌥⌘V | S-BOUNCE-002 | disabled | |
| | — | | | | |
| C-TIME-012 | Snap to Grid | ⌘4 | S-GRID-005 | ticked | |
| C-TIME-013 | Adaptive Grid choices | | | Narrow ticked | As C-CLIP-ARR-020 |
| C-TIME-014 | Fixed Grid choices | | | | As C-CLIP-ARR-021 |
| | — | | | | |
| C-TIME-015 | Narrow Grid | ⌘1 | S-GRID-002 | disabled | |
| C-TIME-016 | Widen Grid | ⌘2 | S-GRID-003 | | |
| | — | | | | |
| C-TIME-017 | Triplet Grid | ⌘3 | S-GRID-004 | disabled | |

Other states, including the scrub area's own menu:

| ID | Command | Source |
|---|---|---|
| C-TIME-018 | Paste, Delete, and the ... Time commands | manual 6.9, 6.11. Beatlayer's time selection menu shows Cut, Copy, Paste, Duplicate and Delete Time after Duplicate, a placement of its own ([time editing](../specs/time-editing.md) R22) |
| C-TIME-019 | Split, Consolidate, Crop Clip (a selection that holds clips) | manual 6.12, 6.13, 22.2 |
| C-TIME-020 | Consolidate Time to New Scene | manual 7.5 |
| C-TIME-021 | Insert automation shapes, Simplify Envelope (automation lanes) | manual 25.5.5, 25.5.4 |
| C-TIME-022 | Add Locator, Insert Time Signature Change (scrub area) | manual 6.4, 6.5 |
| C-TIME-023 | Loop to Next Locator, Set Song Start Time Here (locator or loop brace) | manual 6.4, 6.6 |
| C-TIME-024 | Delete Fragmentary Bar Time, Complete Fragmentary Bar (fragmentary bars) | manual 6.5 |
| C-TIME-025 | Edit Info Text (a locator) | manual 6.4 |
| C-TIME-026 | Edit Value (an automation breakpoint); Add Value (the preview point on an envelope's line) | manual 25.5.2 |
| C-TIME-027 | Edit Time Signature (a selected time-signature marker) | manual 6.5 |
| C-TIME-028 | Separate Stems to New Audio Tracks (a selection holding audio on audio tracks); Beatlayer puts it after Crop Clip ([stem separation](../specs/stem-separation.md) R22, WP-9.2) | release notes 12.3, 12.4; manual 22 |

### Device title bar (C-DEVICE)

Opened by right-clicking the title bar of an audio effect on return track A. The title bar's Show Options toggle opens the same menu (L-DEV-009).

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| C-DEVICE-001 | Cut | ⌘X | S-EDIT-001 | | |
| C-DEVICE-002 | Copy | ⌘C | S-EDIT-002 | | |
| C-DEVICE-003 | Duplicate | ⌘D | S-EDIT-004 | | |
| | — | | | | |
| C-DEVICE-004 | Rename | ⌘R | S-EDIT-008 | | |
| C-DEVICE-005 | Edit Info Text | | | | |
| | — | | | | |
| C-DEVICE-006 | Delete | | S-EDIT-005 | | No key shown here, and Delete sits in this group rather than with Cut, Copy, and Duplicate |
| C-DEVICE-007 | Group | ⌘G | S-DEV-001 | | |
| C-DEVICE-008 | Fold | − | S-DEV-014 | | The same key as Hide Chains (M-VIEW-042). Beatlayer adds Float straight after it, ticked while the device floats, on ⌥F (Ctrl+Alt+F on Windows and Linux, scoped to the Device View; IMP-14, S-DEV-015; not in Live) |
| C-DEVICE-009 | Show Preset Name | | | ticked | Ticked by default in Auto Filter's menu (observed in 12.4.6 (macOS), 12 October 2026) |
| | — | | | | |
| C-DEVICE-010 | Compare: Switch to B | P | S-DEV-011 | | |
| C-DEVICE-011 | Compare: Copy A to B | | | | |
| | — | | | | |
| C-DEVICE-012 | Save as Default Preset | | | | |

Other states:

| ID | Command | Source |
|---|---|---|
| C-DEVICE-013 | Group to Drum Rack (several devices selected) | manual 25.2 |
| C-DEVICE-014 | Lock to (a control surface): one Lock to <surface> item per running surface that can lock, ticked while locked, after Save as Default Preset and a separator; one disabled Lock to Control Surface with none ([device chains](../specs/device-chains.md) R41) | manual 34.1.1.1 |
| C-DEVICE-015 | Device-specific options (for example Mono Sidechain on the filter device; on plug-ins, MPE Mode, ticked while on, and MPE Settings…, then, while the set has a tuning and for every plug-in, Apply Tuning to Per-Note Pitch Bend, ticked while on (on in a new device), which with MPE Mode off is disabled like MPE Settings…, [tunings](../specs/tunings.md) R20; the Drum Sampler's Enable Per-Note Pitch Bend, Envelope Follows Pitch, a swap to Simpler, and Save as Default Pad; Beatlayer's Autosample… on an instrument, IMP-27; and Simpler's Slice to Drum Rack and Slice to New MIDI Track in Slicing mode, also in its sample display's menu, [slice to MIDI](../specs/slice-to-midi.md) R15 to R17; Hi-Quality on Wavetable and Drift, ticked while on and off in a new device, [instrument voices](../specs/instrument-voices.md) R19; on Operator, Enable Per-Note Pitch Bend (on in a new device), Copy Oscillator settings, Set All Envelope Levels to maximum, minimum or middle, the harmonics editor's Even Only, Odd Only and Normalize, and Play By Key in the filter frequency's menu, [Operator](../specs/operator.md) R20, R28; Echo's Equal-Loudness in its Dry/Wet control's menu, ticked while on; Pedal's Hi-Quality, ticked while on; Spectral Time's Zero Dry Signal Latency, ticked while on) | manual 23.2.1, 12.5, 12.6, 12.6.1, 31.4.5, 31.11.7; release notes 12.1; manual 29.14.4, 29.28, 29.37.4 |
| C-DEVICE-016 | Plug-In Hosting (plug-in devices only): a submenu after Save as Default Preset and Lock to, with a separator: Default (the setting's mode), a separator, Within Beatlayer, Together, By Plug-In, Individually, the device's choice ticked; disabled with the reason for ARA and bridged plug-ins ([device chains](../specs/device-chains.md) R45, WP-10.5) | Beatlayer addition |
| C-DEVICE-017 | Reload Plug-In (a plug-in device whose host has crashed): first, with a separator after it ([device chains](../specs/device-chains.md) R46, WP-10.5) | Beatlayer addition |

### Crossfader (C-XFADE)

Opened on the crossfader in the Main track's strip (shown by View > Mixer Controls > Crossfader). The crossfader's Info text says Ctrl-click opens it to choose one of seven crossfade curves. Observed in 12.4.3 (macOS), 15 October 2026.

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| C-XFADE-001 | Dipped | | | | |
| C-XFADE-002 | Intermediate | | | | |
| C-XFADE-003 | Constant Power | | | ticked | The default curve |
| C-XFADE-004 | Slow Fade | | | | |
| C-XFADE-005 | Slow Cut | | | | |
| C-XFADE-006 | Fast Cut | | | | |
| C-XFADE-007 | Transition | | | | |
| C-XFADE-008 | Show Automation | | | | |
| C-XFADE-009 | Show Automation In New Lane | | | | |

### Pan control (C-PAN)

Opened on a track's pan control (the strip's or the Arrangement header's). Opened in Live 12.4.16 (macOS) on 2 November 2026. On an audio track in the Session mixer, in the stereo pan mode: the menu offers only the mode to switch to, not the one in force, and goes on with the items every automatable control has (C-PARAM). It is the volume fader's menu with its own item first.

| ID | Item | Keys shown | State | Source |
|---|---|---|---|---|
| C-PAN-001 | Select Split Stereo Pan Mode |  |  | obs; in the split mode Beatlayer reads Select Stereo Pan Mode, the other mode's name in the same words, which wasn't opened in Live |
| | — | | | |
| C-PARAM-003 | Show Automation |  |  | obs |
| C-PARAM-004 | Show Automation In New Lane |  |  | obs |
| | — | | | |
| C-PARAM-005 | Show Modulation Source 1: Clip Envelope |  | dim | obs |
| | — | | | |
| C-PARAM-006 | Edit MIDI Map | ⌘M |  | obs |
| C-PARAM-007 | Edit Key Map | ⌘K |  | obs |
| | — | | | |
| C-PARAM-010 | Copy Max for Live Path |  | dim | obs; Beatlayer has no Max for Live |

Other states: an automated control's menu begins with Delete Automation, and with Re-Enable Automation while its envelope is overridden (manual 25.3, 25.4; not opened with an envelope drawn). Beatlayer shows both only while they apply, before Show Automation, with no separator.

### Mixer control (C-PARAM)

Opened on a track's mixer control that has no menu of its own, such as the volume fader. Opened in Live 12.4.16 (macOS) on 2 November 2026. The crossfader's menu shows the last two items before this one's rest (C-XFADE-008, C-XFADE-009).

| ID | Item | Keys shown | State | Source |
|---|---|---|---|---|
| = C-PARAM-003 | Show Automation |  |  | obs |
| = C-PARAM-004 | Show Automation In New Lane |  |  | obs |
| | — | | | |
| = C-PARAM-005 | Show Modulation Source 1: Clip Envelope |  | dim | obs |
| | — | | | |
| = C-PARAM-006 | Edit MIDI Map | ⌘M |  | obs |
| = C-PARAM-007 | Edit Key Map | ⌘K |  | obs |
| | — | | | |
| = C-PARAM-010 | Copy Max for Live Path |  | dim | obs; Beatlayer has no Max for Live |

Other states: Delete Automation and Re-Enable Automation, as in C-PAN. A device parameter (C-PARAM rack, below) adds Copy Parameter Name after Copy Max for Live Path. Show Modulation Source 1 and Copy Max for Live Path are shown dim: Beatlayer has no clip envelope as a modulation source of its controls' menus yet, and no Max for Live.

### Device parameter inside a rack (C-PARAM rack)

Opened on Utility's Gain in an Audio Effect Rack. Opened in Live 12.4.16 (macOS) on 2 November 2026. Outside a rack the macro items are absent, as the mixer controls' menus show; Copy Parameter Name shows for a device parameter.

| ID | Item | Keys shown | State | Source |
|---|---|---|---|---|
| = C-PARAM-003 | Show Automation |  |  | obs |
| = C-PARAM-004 | Show Automation In New Lane |  |  | obs |
| | — | | | |
| = C-PARAM-005 | Show Modulation Source 1: Clip Envelope |  | dim | obs |
| | — | | | |
| = C-PARAM-006 | Edit MIDI Map | ⌘M |  | obs |
| = C-PARAM-007 | Edit Key Map | ⌘K |  | obs |
| C-PARAM-008 | Edit Macro Map |  |  | obs |
| | — | | | |
| C-PARAM-009a | Map to Macro 1 |  |  | obs |
| C-PARAM-009b | Map to Macro 2 |  |  | obs |
| C-PARAM-009c | Map to Macro 3 |  |  | obs |
| C-PARAM-009d | Map to Macro 4 |  |  | obs |
| C-PARAM-009e | Map to Macro 5 |  |  | obs |
| C-PARAM-009f | Map to Macro 6 |  |  | obs |
| C-PARAM-009g | Map to Macro 7 |  |  | obs |
| C-PARAM-009h | Map to Macro 8 |  |  | obs |
| C-PARAM-009i | Map to Macro 9 |  |  | obs |
| C-PARAM-009j | Map to Macro 10 |  |  | obs |
| C-PARAM-009k | Map to Macro 11 |  |  | obs |
| C-PARAM-009l | Map to Macro 12 |  |  | obs |
| C-PARAM-009m | Map to Macro 13 |  |  | obs |
| C-PARAM-009n | Map to Macro 14 |  |  | obs |
| C-PARAM-009o | Map to Macro 15 |  |  | obs |
| C-PARAM-009p | Map to Macro 16 |  |  | obs |
| | — | | | |
| = C-PARAM-010 | Copy Max for Live Path |  | dim | obs; Beatlayer has no Max for Live |
| C-PARAM-011 | Copy Parameter Name |  | dim | obs; Beatlayer has no command to copy it yet |

Other states: Map to Macro rows beyond the rack's macro count are shown dim (Live showed sixteen rows in a rack of eight macros; whether it enables the rest wasn't observed). Copy Parameter Name is shown dim: no registered command copies a name yet.

### Macro knob (C-MACRO)

Opened on Macro 1 of an Audio Effect Rack. Opened in Live 12.4.16 (macOS) on 2 November 2026. The rack's title bar shows Rand and Map buttons while the macros show; a new rack shows 8 macros in 2 rows of 4, with + and − down the view column to change the count, and a Macro Variations panel with New left of the macros.

| ID | Item | Keys shown | State | Source |
|---|---|---|---|---|
| C-MACRO-001 | Show Automation |  |  | obs |
| C-MACRO-002 | Show Automation In New Lane |  |  | obs |
| | — | | | |
| C-MACRO-003 | Show Modulation Source 1: Clip Envelope |  | dim | obs |
| | — | | | |
| C-MACRO-004 | Edit MIDI Map | ⌘M |  | obs |
| C-MACRO-005 | Edit Key Map | ⌘K |  | obs |
| | — | | | |
| C-MACRO-006 | Copy Max for Live Path |  | dim | obs |
| C-MACRO-007 | Copy Parameter Name |  | dim | obs |
| | — | | | |
| C-MACRO-008 | Show Generic 0-127 Value |  | dim | obs |
| C-MACRO-009 | Rename | ⌘R |  | obs |
| C-MACRO-010 | Edit Info Text |  |  | obs |
| C-MACRO-011 | Exclude Macro from Randomization |  | ticked while excluded | obs |
| C-MACRO-012 | Exclude Macro From Variations |  | ticked while excluded | obs |
| | — | | | |
| C-MACRO-013 | No Color |  |  | obs |
| C-MACRO-014 | Colour palette |  |  | obs; 5 rows of 14 swatches |

Beatlayer's macro menu had a Macro Style submenu and Include in Randomization and Include in Variations items of its own; Live's menu has none of them, so they are gone from it (`rack.set_macro_style` and the commands that set the exclusions stay). Show Generic 0-127 Value is dim: no such display is built.

### Rack chain (C-CHAIN)

Opened on a chain in an Audio Effect Rack's chain list. Opened in Live 12.4.16 (macOS) on 2 November 2026. The chain list's header shows a small icon button at its left and Chain, Hide (lit) at its right; a chain row shows its name in its colour, volume 0.0 dB (blue), pan C, a speaker (lit), S, and a hot-swap icon.

| ID | Item | Keys shown | State | Source |
|---|---|---|---|---|
| C-CHAIN-001 | Duplicate | ⌘D |  | obs |
| | — | | | |
| C-CHAIN-002 | Rename | ⌘R |  | obs |
| C-CHAIN-003 | Edit Info Text |  |  | obs |
| C-CHAIN-004 | Create Chain |  |  | obs |
| | — | | | |
| C-CHAIN-005 | Color Automatically |  | ticked in Live; dim here | obs |
| C-CHAIN-006 | Colour palette |  |  | obs; 5 rows of 14 swatches |

Color Automatically is dim: chains take the colour of the palette only. Beatlayer's own Cut, Copy and Delete are not in this menu; their keys still work.

### Main track header (C-MAIN)

Opened on the Main track's header in the Session View. Opened in Live 12.4.16 (macOS) on 2 November 2026. No rename, delete, freeze, group or Color Automatically items.

| ID | Item | Keys shown | State | Source |
|---|---|---|---|---|
| C-MAIN-001 | Edit Info Text |  |  | obs |
| | — | | | |
| C-MAIN-002 | Insert Audio Track | ⌘T |  | obs |
| C-MAIN-003 | Insert MIDI Track | ⇧⌘T |  | obs |
| C-MAIN-004 | Insert Return Track | ⌥⌘T |  | obs |
| | — | | | |
| C-MAIN-005 | Select Track Content |  | dim | obs |
| | — | | | |
| C-MAIN-006 | Colour palette |  |  | obs; 5 rows of 14 swatches, the Main track's colour (red) framed |

### CPU meter (C-CPU)

Opened by a click on the Control Bar's CPU meter, which is a box with ▼; the arrow beside it opens the same menu (L-CTRL-024). Opened in Live 12.4.16 (macOS) on 2 November 2026.

| ID | Item | Keys shown | State | Source |
|---|---|---|---|---|
| C-CPU-001 | Enable CPU Meter |  | ticked; dim here | obs |
| | — | | | |
| C-CPU-002 | Average |  | ticked; its reading at the right (1 %) | obs |
| C-CPU-003 | Current |  | its reading at the right (---) | obs |
| | — | | | |
| C-CPU-004 | Meter Text Shows: |  | heading | obs |
| C-CPU-005 | Average, Current |  | one row of two choices, Average ticked | obs |
| | — | | | |
| C-CPU-006 | Warn on Current CPU Overload |  |  | obs |
| | — | | | |
| C-CPU-007 | Audio Engine On | ⌥⇧⌘E | ticked | obs |
| C-CPU-008 | Configure Audio… |  |  | obs |

In Beatlayer the meter measures one load, which is the Current reading; Enable CPU Meter, Average, the choice of meter text and Warn on Current CPU Overload have nothing behind them yet and are shown dim, with the readings of Average as ---.

### Automation lane header (C-ALANE)

Opened on an automation lane's header in the Arrangement, here Track Volume in its own lane with nothing drawn. Opened in Live 12.4.16 (macOS) on 2 November 2026. The lane header reads Mixer / Track Volume with a round − (remove) button at its right; the track's header grows a ▼ (show or hide lanes) button.

| ID | Item | Keys shown | State | Source |
|---|---|---|---|---|
| C-ALANE-001 | Remove Automation Lane |  |  | obs |
| C-ALANE-002 | Add Lane for Each Automated Envelope |  | dim with nothing automated | obs |
| | — | | | |
| C-ALANE-003 | Clear Envelope | ⌘Del | dim with nothing automated | obs |
| C-ALANE-004 | Clear All Envelopes of Mixer |  | dim with nothing automated | obs |
| C-ALANE-005 | Clear All Envelopes |  | dim with nothing automated | obs |

The second item of the third group names the device the lane shows (Mixer, here). In Beatlayer Show Automated Parameters Only, Show All Lanes and Hide All Lanes are no longer in this menu; the track header's ▼ button and the commands keep them ([automation](../specs/automation.md) R28).

### Take lane header (C-TAKE)

Opened on a take lane's header in the Arrangement ([comping](../specs/comping.md) R29), which it selects first. Opened in Live 12.4.16 (macOS) on 2 November 2026. On an empty take lane inserted on an audio track (Insert Take Lane from the track header's menu): the header is pale blue, named Lane, with a speaker (audition) button at its right.

| ID | Item | Keys shown | State | Source |
|---|---|---|---|---|
| C-TAKE-001 | Duplicate | ⌘D |  | obs |
| C-TAKE-002 | Delete | Del |  | obs |
| | — | | | |
| C-TAKE-003 | Rename | ⌘R |  | obs |
| C-TAKE-004 | Edit Info Text |  | dim here | obs |
| | — | | | |
| C-TAKE-005 | Insert Take Lane | ⌥⇧T |  | obs |
| C-TAKE-006 | Show Take Lanes | ⌥⌘U | ticked | obs |
| C-TAKE-007 | Delete All Take Lanes |  |  | obs |
| C-TAKE-008 | Delete Unused Take Lanes |  |  | obs |

Copy Selection to Main Lane and Paste Bounced Audio are not in Live's menu and are not in Beatlayer's; their keys (S-COMP-002, S-BOUNCE-002) still run them. Edit Info Text is dim: a take lane has no info text yet.

### Drum Rack pad (C-PAD)

Opened on a pad that holds a device (DS Kick dropped on C1). Opened in Live 12.4.16 (macOS) on 2 November 2026. An empty pad opens no menu: a right-click only selects it.

| ID | Item | Keys shown | State | Source |
|---|---|---|---|---|
| C-PAD-001 | Rename | ⌘R |  | obs |
| C-PAD-002 | Delete | Del |  | obs |
| | — | | | |
| C-PAD-003 | Save as Default Pad |  |  | obs |
| | — | | | |
| C-PAD-005 | Next Similar | ⌘→ | dim for a pad holding an instrument | obs |
| C-PAD-006 | Previous Similar | ⌘← | dim for a pad holding an instrument | obs |
| C-PAD-007 | Save as Similarity Reference | ⌘↑ | dim for a pad holding an instrument | obs |
| C-PAD-008 | Return to Reference | ⌘↓ | dim for a pad holding an instrument | obs |

The similar items are dim for a pad holding an instrument rather than a sample ([sound similarity](../specs/sound-similarity.md) R21).

### Clip View title bar (C-CLIPTITLE)

Opened on the Clip View's title bar (L-CLIP-002); it acts on the clips the Clip View shows ([session context menus](../specs/session-context-menus.md) R4). Opened in Live 12.4.16 (macOS) on 2 November 2026. On a Session MIDI clip. Nothing else is in it.

| ID | Item | Keys shown | State | Source |
|---|---|---|---|---|
| C-CLIPTITLE-001 | Rename | ⌘R |  | obs |
| | — | | | |
| C-CLIPTITLE-002 | Colour palette |  |  | obs; 5 rows of 14 swatches, the clip's colour framed |
| | — | | | |
| C-CLIPTITLE-003 | Arrange Clip View Panels Automatically |  | ticked | obs |
| C-CLIPTITLE-004 | Fold |  |  | obs |

Beatlayer's Edit Info Text, Arrange Clip View Panels Horizontally and Arrange Clip View Panels Vertically are not in this menu any more; the Edit menu (M-EDIT-025) and the View menu (M-VIEW-016, M-VIEW-017) have them.

### Clip envelope editor (C-ENV)

Opened on the Envelopes tab of a MIDI clip's Clip View, with no envelope drawn. Opened in Live 12.4.16 (macOS) on 2 November 2026. With nothing drawn there are no Clear items.

| ID | Item | Keys shown | State | Source |
|---|---|---|---|---|
| C-ENV-001 | Snap to Grid | ⌘4 | ticked; dim here | obs; the key's command works in the note editor only |
| C-ENV-002 | Adaptive Grid: |  | heading | obs |
| C-ENV-003 | Adaptive Grid: Widest, Wide, Medium / Narrow, Narrowest |  | three a row | obs |
| C-ENV-004 | Fixed Grid: |  | heading | obs |
| C-ENV-005 | Fixed Grid: 8 Bars, 4 Bars, 2 Bars, 1 Bar, 1/2 / 1/4, 1/8, 1/16, 1/32 |  | five a row, 1/16 ticked | obs |
| | — | | | |
| C-ENV-006 | Narrow Grid | ⌘1 | dim here | obs; likewise |
| C-ENV-007 | Widen Grid | ⌘2 | dim here | obs; likewise |
| | — | | | |
| C-ENV-008 | Triplet Grid | ⌘3 | dim here | obs; likewise |
| | — | | | |
| C-ENV-009 | Draw Mode | B |  | obs |
| | — | | | |
| C-ENV-010 | Insert Shape: |  | heading | obs |
| C-ENV-011 | Waveforms: sine, triangle, saw down, saw up, square |  | five shape icons | obs |
| C-ENV-012 | Ramps and ADSR: five ramps and curves |  | five shape icons | obs |

Beatlayer's right-click menus on a breakpoint (Edit Value), on the line (Add Value) and on a time selection (the shapes, Simplify Envelope) were not opened in Live and stay as they were ([clip envelopes](../specs/clip-envelopes.md) R13).

### MIDI Note Editor, on a note (C-NOTE)

Opened on a note in the Session MIDI clip's note editor, which selects the note alone first if it isn't selected ([piano roll](../specs/piano-roll.md) R30). Opened in Live 12.4.16 (macOS) on 2 November 2026. Three notes, one other selected before. There is no Cut, Copy, Paste or Delete: those run from their keys. "Deactivate Note" is singular for one note, while Chop Note(s) and Extract Groove(s) keep the (s).

| ID | Item | Keys shown | State | Source |
|---|---|---|---|---|
| C-NOTE-001 | Invert Selection | ⇧⌘A |  | obs |
| C-NOTE-002 | Duplicate | ⌘D |  | obs |
| | — | | | |
| C-NOTE-003 | Quantize | ⌘U |  | obs |
| C-NOTE-004 | Quantize Settings... | ⇧⌘U |  | obs |
| C-NOTE-005 | Deactivate Note | 0 | singular for one note | obs |
| C-NOTE-006 | Extract Groove(s) |  |  | obs |
| | — | | | |
| C-NOTE-007 | Chop Note(s) on Grid | ⌘E |  | obs |
| C-NOTE-008 | Join Notes | ⌘J | dim with one note | obs |
| C-NOTE-009 | Fit to Time Range | ⌥⌘J |  | obs |
| | — | | | |
| C-NOTE-010 | Crop Clip to Time Selection | ⇧⌘J |  | obs |
| | — | | | |
| C-NOTE-012 | Loop Selection | ⌘L |  | obs |
| C-NOTE-013 | Set Start Marker | ⌘F9 |  | obs |
| C-NOTE-014 | Set Loop Start | ⌘F10 |  | obs |
| C-NOTE-015 | Set Loop End | ⌘F11 |  | obs |
| C-NOTE-016 | Set End Marker | ⌘F12 |  | obs |
| | — | | | |
| C-NOTE-017 | Zoom to Clip Selection | Z |  | obs |
| C-NOTE-018 | Zoom Out Clip Completely | X |  | obs |
| C-NOTE-019 | Fit Content To View Width | W |  | obs |
| C-NOTE-020 | Fit Content To View Height | H |  | obs |
| | — | | | |
| C-NOTE-029 | Draw Mode (Pitch Lock Off) | B |  | obs |
| | — | | | |
| C-NOTE-030 | Highlight Scales | K |  | obs |

Other states: with several notes selected Deactivate Note reads Deactivate Note(s), and Activate Note(s) when every one is deactivated (Beatlayer's wording; one note reads Activate Note). Beatlayer's Cut, Copy, Paste, Delete, Group Notes, Ungroup Notes, Split Note(s) and Split Clip were in this menu and aren't now; their keys and commands stay. Loop Selection, Crop Clip to Time Selection and Set 1.1.1 Here are dim where Beatlayer has no command for the note editor.

### MIDI Note Editor, on empty grid (C-NOTE grid)

Opened on empty grid in the note editor, with one note still selected. Opened in Live 12.4.16 (macOS) on 2 November 2026. It is the menu on a note with two more groups: Set 1.1.1 Here after Crop Clip to Time Selection, and the grid's settings after Fit Content To View Height. The grid items don't show on a note. The grid is the Arrangement's ([view options](../specs/view-options.md)).

| ID | Item | Keys shown | State | Source |
|---|---|---|---|---|
| = C-NOTE-001 | Invert Selection | ⇧⌘A |  | obs |
| = C-NOTE-002 | Duplicate | ⌘D |  | obs |
| | — | | | |
| = C-NOTE-003 | Quantize | ⌘U |  | obs |
| = C-NOTE-004 | Quantize Settings... | ⇧⌘U |  | obs |
| = C-NOTE-005 | Deactivate Note | 0 | singular for one note | obs |
| = C-NOTE-006 | Extract Groove(s) |  |  | obs |
| | — | | | |
| = C-NOTE-007 | Chop Note(s) on Grid | ⌘E |  | obs |
| = C-NOTE-008 | Join Notes | ⌘J | dim with one note | obs |
| = C-NOTE-009 | Fit to Time Range | ⌥⌘J |  | obs |
| | — | | | |
| = C-NOTE-010 | Crop Clip to Time Selection | ⇧⌘J |  | obs |
| | — | | | |
| C-NOTE-011 | Set 1.1.1 Here |  | dim here | obs |
| | — | | | |
| = C-NOTE-012 | Loop Selection | ⌘L |  | obs |
| = C-NOTE-013 | Set Start Marker | ⌘F9 |  | obs |
| = C-NOTE-014 | Set Loop Start | ⌘F10 |  | obs |
| = C-NOTE-015 | Set Loop End | ⌘F11 |  | obs |
| = C-NOTE-016 | Set End Marker | ⌘F12 |  | obs |
| | — | | | |
| = C-NOTE-017 | Zoom to Clip Selection | Z |  | obs |
| = C-NOTE-018 | Zoom Out Clip Completely | X |  | obs |
| = C-NOTE-019 | Fit Content To View Width | W |  | obs |
| = C-NOTE-020 | Fit Content To View Height | H |  | obs |
| | — | | | |
| C-NOTE-021 | Snap to Grid | ⌘4 | ticked | obs |
| C-NOTE-022 | Adaptive Grid: |  | heading | obs |
| C-NOTE-023 | Adaptive Grid: Widest, Wide, Medium / Narrow, Narrowest |  | three a row | obs |
| C-NOTE-024 | Fixed Grid: |  | heading | obs |
| C-NOTE-025 | Fixed Grid: 8 Bars, 4 Bars, 2 Bars, 1 Bar, 1/2 / 1/4, 1/8, 1/16, 1/32 |  | five a row, 1/16 ticked | obs |
| | — | | | |
| C-NOTE-026 | Narrow Grid | ⌘1 |  | obs |
| C-NOTE-027 | Widen Grid | ⌘2 |  | obs |
| | — | | | |
| C-NOTE-028 | Triplet Grid | ⌘3 |  | obs |
| | — | | | |
| = C-NOTE-029 | Draw Mode (Pitch Lock Off) | B |  | obs |
| | — | | | |
| = C-NOTE-030 | Highlight Scales | K |  | obs |

### Piano ruler (C-KEYS)

Opened on the MIDI Note Editor's piano ruler (L-CLIP-012). Opened in Live 12.4.16 (macOS) on 2 November 2026. Auto: Sharps (♯) was ticked, and its label names the spelling Auto gives for the clip's scale.

| ID | Item | Keys shown | State | Source |
|---|---|---|---|---|
| C-KEYS-001 | Deactivate Note | 0 |  | obs |
| | — | | | |
| C-KEYS-002 | Accidentals |  | heading | obs |
| C-KEYS-003 | Auto: Sharps (♯) |  | ticked | obs |
| C-KEYS-004 | Sharps (♯) |  |  | obs |
| C-KEYS-005 | Flats (♭) |  |  | obs |
| C-KEYS-006 | Sharps and Flats (♯/♭) |  |  | obs |
| | — | | | |
| C-KEYS-007 | MIDI Note Number |  |  | obs |

Beatlayer's Sharps, Flats, Sharps and Flats, Auto and MIDI Note Numbers labels are Live's now ([scales](../specs/scales.md) R24); the choice is the same.

### Probability group marker (C-PGROUP)

Opened on a probability group's marker in the Chance lane ([note editing](../specs/note-editing.md) R23). Live's order wasn't recorded.

| ID | Command | Source |
|---|---|---|
| C-PGROUP-001 | Play All, ticked while the group plays all its notes | manual 10.5.13.1 |
| C-PGROUP-002 | Play One, ticked while the group plays one of its notes | manual 10.5.13.1 |
| C-PGROUP-003 | Ungroup Notes, ending the group | manual 10.5.13.1 |

### Expression lane header (C-LANE)

Opened on the header of an expression lane under the note area (L-CLIP-014; [note editing](../specs/note-editing.md) R2), here the Velocity lane's header (its icon and 127/64/1 scale; the header lights blue while the menu shows). Opened in Live 12.4.16 (macOS) on 2 November 2026. A right-click on the lane's foot row (Velocity, Randomize, Ramp, Deviation) opens nothing. The MPE tab's lane headers have the same menu ([MPE](../specs/mpe.md) R23).

| ID | Item | Keys shown | State | Source |
|---|---|---|---|---|
| C-LANE-001 | Swap Lane: |  | heading | obs |
| C-LANE-002 | Velocity |  | ticked, dim: the lane shown | obs |
| C-LANE-003 | Release Velocity |  |  | obs |
| C-LANE-004 | Chance |  |  | obs |
| | — | | | |
| C-LANE-005 | Hide Lane |  |  | obs |

The lane shown is the ticked, dim row; choosing another swaps it in. The rows are the lanes the editor has.

### Multi-clip loop bar (C-LOOPBAR)

Opened on a loop bar above the note editor while it shows several clips (L-CLIP-021; [multi-clip editing](../specs/multi-clip-editing.md) R11).

| ID | Command | Source |
|---|---|---|
| C-LOOPBAR-001 | Duplicate (S-LOOP-012): doubles each chosen clip's loop and copies its notes into the new half | manual 10.8 |

### MPE tab (C-MPE)

Opened on the note editor's grid in the Clip View's MPE tab with a note selected ([MPE](../specs/mpe.md) R26, R31). Opened in Live 12.4.16 (macOS) on 2 November 2026. It is the empty-grid menu with two changes: a group after Duplicate that clears envelopes (dim while there is no envelope), and Pitch Bend Range Settings... before Highlight Scales. In this tab Snap to Grid wasn't ticked and no grid size was ticked.

| ID | Item | Keys shown | State | Source |
|---|---|---|---|---|
| = C-NOTE-001 | Invert Selection | ⇧⌘A |  | obs |
| = C-NOTE-002 | Duplicate | ⌘D |  | obs |
| | — | | | |
| C-MPE-001 | Clear Envelope (Pitch Bend) | ⌘Del | dim with no envelope | obs |
| C-MPE-002 | Clear All MPE Envelopes |  | dim with no envelope | obs |
| | — | | | |
| = C-NOTE-003 | Quantize | ⌘U |  | obs |
| = C-NOTE-004 | Quantize Settings... | ⇧⌘U |  | obs |
| = C-NOTE-005 | Deactivate Note | 0 | singular for one note | obs |
| = C-NOTE-006 | Extract Groove(s) |  |  | obs |
| | — | | | |
| = C-NOTE-007 | Chop Note(s) on Grid | ⌘E |  | obs |
| = C-NOTE-008 | Join Notes | ⌘J | dim with one note | obs |
| = C-NOTE-009 | Fit to Time Range | ⌥⌘J |  | obs |
| | — | | | |
| = C-NOTE-010 | Crop Clip to Time Selection | ⇧⌘J |  | obs |
| | — | | | |
| = C-NOTE-011 | Set 1.1.1 Here |  | dim here | obs |
| | — | | | |
| = C-NOTE-012 | Loop Selection | ⌘L |  | obs |
| = C-NOTE-013 | Set Start Marker | ⌘F9 |  | obs |
| = C-NOTE-014 | Set Loop Start | ⌘F10 |  | obs |
| = C-NOTE-015 | Set Loop End | ⌘F11 |  | obs |
| = C-NOTE-016 | Set End Marker | ⌘F12 |  | obs |
| | — | | | |
| = C-NOTE-017 | Zoom to Clip Selection | Z |  | obs |
| = C-NOTE-018 | Zoom Out Clip Completely | X |  | obs |
| = C-NOTE-019 | Fit Content To View Width | W |  | obs |
| = C-NOTE-020 | Fit Content To View Height | H |  | obs |
| | — | | | |
| = C-NOTE-021 | Snap to Grid | ⌘4 | ticked | obs |
| = C-NOTE-022 | Adaptive Grid: |  | heading | obs |
| = C-NOTE-023 | Adaptive Grid: Widest, Wide, Medium / Narrow, Narrowest |  | three a row | obs |
| = C-NOTE-024 | Fixed Grid: |  | heading | obs |
| = C-NOTE-025 | Fixed Grid: 8 Bars, 4 Bars, 2 Bars, 1 Bar, 1/2 / 1/4, 1/8, 1/16, 1/32 |  | five a row, 1/16 ticked | obs |
| | — | | | |
| = C-NOTE-026 | Narrow Grid | ⌘1 |  | obs |
| = C-NOTE-027 | Widen Grid | ⌘2 |  | obs |
| | — | | | |
| = C-NOTE-028 | Triplet Grid | ⌘3 |  | obs |
| | — | | | |
| = C-NOTE-029 | Draw Mode (Pitch Lock Off) | B |  | obs |
| | — | | | |
| C-MPE-003 | Pitch Bend Range Settings... |  | dim here | obs |
| | — | | | |
| = C-NOTE-030 | Highlight Scales | K |  | obs |

Other states (not opened in Live): Edit Value on a breakpoint and Add Value on the preview breakpoint at the pointer, which Beatlayer puts at the top of the menu ([MPE](../specs/mpe.md) R26). Clear Envelope names the envelope that has the keyboard (Pitch Bend, Slide, Pressure).

### Filter View Menu (C-FILTER)

Opened by the ▼ beside the Filter View toggle on the browser's top row (L-BRW-001; [browser tags](../specs/browser-tags.md) R21), with the Audio Effects label shown. Opened in Live 12.4.16 (macOS) on 2 November 2026.

| ID | Item | Keys shown | State | Source |
|---|---|---|---|---|
| C-FILTER-001 | Open Tag Editor |  |  | obs |
| C-FILTER-002 | Open Quick Tags |  | ticked | obs |
| C-FILTER-003 | Enable Auto Tags |  | ticked | obs |
| | — | | | |
| C-FILTER-004 | Show All Filter Groups |  |  | obs |
| C-FILTER-005 | Hide All Filter Groups |  |  | obs |
| C-FILTER-006 | Reset Filter Groups to Default |  | dim | obs |
| | — | | | |
| C-FILTER-007 | Content |  |  | obs |
| C-FILTER-008 | Devices |  | ticked | obs |
| C-FILTER-009 | Function |  |  | obs |
| C-FILTER-010 | Format |  |  | obs |
| C-FILTER-011 | Creator |  |  | obs |

The last group is one item for each filter group the chosen label can show, ticked while it shows (manual 4.4.1). Beatlayer's Tag Editor and Show Quick Tags toggles are Open Tag Editor and Open Quick Tags now, ticked while they show.

### Content Options (C-CONTENT)

Opened by the ⋯ at the right of the browser's Name header, or on a column's name (L-BRW-009; [browser tags](../specs/browser-tags.md) R27, R43). Opened in Live 12.4.16 (macOS) on 2 November 2026. None of the columns is ticked in a new install; Show File Extensions is.

| ID | Item | Keys shown | State | Source |
|---|---|---|---|---|
| C-CONTENT-001 | Date Modified |  |  | obs |
| C-CONTENT-002 | Size |  |  | obs |
| C-CONTENT-003 | Type |  |  | obs |
| C-CONTENT-004 | Rank |  | dim here | obs |
| C-CONTENT-005 | Place |  | dim here | obs |
| | — | | | |
| C-CONTENT-006 | Hide All Extra Columns |  | dim while none shows | obs |
| | — | | | |
| C-CONTENT-007 | Show File Extensions |  | ticked | obs |

Beatlayer's Kind column is Type now. Length, Sample Rate and Channels are not in this menu; `browser.set_columns` still shows them. Rank and Place are dim: Beatlayer has no such columns. Under the Plug-Ins label Beatlayer adds Show Hidden Plug-Ins after Show File Extensions (IMP-07).

### Tag Editor (C-TAG)

Opened on a person's own tag or group, or on the list's background, in the Tag Editor (L-BRW-013; [browser tags](../specs/browser-tags.md) R12).

| ID | Command | Source |
|---|---|---|
| C-TAG-001 | Add Subtag In…, on a tag outside the Creator group that isn't a subtag | manual 4.4.3 |
| C-TAG-002 | Rename | manual 4.4.3 |
| C-TAG-003 | Delete | manual 4.4.3 |
| C-TAG-004 | Reset Order, on the list's background (Beatlayer's own name and place) | Beatlayer |

### Quick Tags tag (C-QTAG)

Opened on a tag in the Tags bar under the browser's item list (L-BRW-012; [browser tags](../specs/browser-tags.md) R13), with Utility selected: "Tags: Utility Add…", above "1 item selected". Opened in Live 12.4.16 (macOS) on 2 November 2026. Add… is dim text after the tags.

| ID | Item | Keys shown | State | Source |
|---|---|---|---|---|
| C-QTAG-001 | Show "Utility" tag in Editor |  |  | obs |

The item names the tag it was opened on.

### Automation Arm button (IMP-23)

Beatlayer's addition, opened on the Control Bar's Automation Arm button (L-CTRL-015; [automation](../specs/automation.md) R18). Live has no such menu.

| ID | Command | Source |
|---|---|---|
| IMP-23 | Touch, Latch, Write: the Arrangement's automation recording mode, the current one ticked; Touch, Live's behaviour, on a new install (the names are Beatlayer's own) | Beatlayer |

### Link menu (C-LINK)

Opened from the ▼ beside the Control Bar's Link toggle (L-CTRL-002; [Link](../specs/link.md)). Opened in Live 12.4.16 (macOS) on 2 November 2026. With Link off. There is no Link item: the toggle is that.

| ID | Item | Keys shown | State | Source |
|---|---|---|---|---|
| C-LINK-001 | Link Start Stop Sync |  | ticked while on | obs |
| C-LINK-002 | Link Audio |  | ticked while on | obs |
| | — | | | |
| C-LINK-003 | No Link Audio peers available. |  | dim note, not an item | obs |
| | — | | | |
| C-LINK-004 | Configure Link… |  |  | obs |

With Link Audio peers the note gives way to their names, not clickable (manual 37.1.1; not opened in Live).

### Mapping Browser row (C-MAP)

Opened on a mapping in the Mapping Browser (L-OTH-007; [the map modes](../specs/map-modes.md)).

| ID | Command | Source |
|---|---|---|
| C-MAP-001 | Invert Range: swaps the mapping's Min and Max | manual 34.2, 25.7.1 |

# Menus

The menu bar and the main context menus of Live 12, as Beatlayer reproduces them (D16). Read the [README](README.md) first for the sources, the method, and how UI packages cite rows.

## How to read this page

- **Item labels** are the generic command names Live shows, recorded so Beatlayer's menus read the same. Labels that name an Ableton product or service are marked; Beatlayer replaces or drops those (each needs a decisions-log entry).
- **Order** is the order Live 12.4.3 lists the items on macOS, observed on 10 October 2026: first through the menu bar's accessibility interface (titles and order of every menu and submenu), then by opening each menu to read its separators, the keys it shows, and which items are disabled or ticked. Every row here was observed.
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
| M-APP-001 | About Live | | | | Beatlayer: About Beatlayer (D21) |
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
| M-FILE-002 | Open Live Set... | ⌘O | S-FILE-002 | | Beatlayer: Open Beatlayer Set... (D21) |
| M-FILE-003 | Open Recent Set ▸ | | | | Submenu of recently opened sets; its contents are the user's files and weren't recorded |
| M-FILE-004 | Close Window | ⌘W | S-FILE-009 | disabled | Not in the manual |
| | — | | | | |
| M-FILE-005 | Install Pack... | | | | Installs a pack file |
| M-FILE-006 | Add Folder to Browser... | | | | Adds a folder to the browser's Places |
| | — | | | | |
| M-FILE-007 | Manage Files | | | | Opens the File Manager (L-OTH-002) |
| M-FILE-008 | Open Live Set Backups ▸ | | | | Beatlayer: Open Beatlayer Set Backups (D21). A submenu of the set's backups; its contents weren't recorded |
| | — | | | | |
| M-FILE-009 | Save Live Set | ⌘S | S-FILE-003 | | Beatlayer: Save Beatlayer Set (D21) |
| M-FILE-010 | Save Live Set As... | ⇧⌘S | S-FILE-004 | | Beatlayer: Save Beatlayer Set As... (D21) |
| M-FILE-011 | Save a Copy... | | | | |
| M-FILE-012 | Collect All and Save | | | | |
| M-FILE-013 | Save Live Set As Template... | | | | Beatlayer: Save Beatlayer Set As Template... (D21) |
| M-FILE-014 | Save Live Set As Default Set... | | | | Beatlayer: Save Beatlayer Set As Default Set... (D21) |
| | — | | | | |
| M-FILE-015 | Export Audio/Video... | ⇧⌘R | S-FILE-007 | | |
| M-FILE-016 | Export MIDI Clip... | | S-FILE-008 | disabled | Live shows no key here or in the clip context menus; the manual's binding (S-FILE-008) is unconfirmed |

## Edit

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| M-EDIT-001 | Undo | ⌘Z | S-EDIT-006 | disabled | Reads "Nothing to Undo" when there's nothing to undo; otherwise names the action |
| M-EDIT-002 | Redo | ⇧⌘Z | S-EDIT-007 | disabled | Reads "Nothing to Redo" likewise, including after a new edit discards the undone actions |
| | — | | | | |
| M-EDIT-003 | Cut | ⌘X | S-EDIT-001 | | |
| M-EDIT-004 | Copy | ⌘C | S-EDIT-002 | | |
| M-EDIT-005 | Paste | ⌘V | S-EDIT-003 | disabled | |
| M-EDIT-006 | Duplicate | ⌘D | S-EDIT-004 | | Beatlayer adds Duplicate Track Without Clips straight after it (IMP-12; not in Live) |
| M-EDIT-007 | Delete | ⌫ | S-EDIT-005 | | |
| | — | | | | |
| M-EDIT-008 | Deactivate Track | 0 | S-TRACK-016, S-SES-018, S-ARR-027 | | Label follows the selection: Deactivate Track with a track's slot selected, Deactivate Clip in the clip context menus, and likewise for notes |
| | — | | | | |
| M-EDIT-009 | Select All | ⌘A | S-EDIT-009 | | |
| M-EDIT-010 | Invert Selection | ⇧⌘A | S-NOTE-034 | disabled | |
| M-EDIT-011 | Select Loop | ⇧⌘L | S-ARR-013 | disabled | |
| M-EDIT-012 | Loop Selection | ⌘L | S-ARR-011 | disabled | |
| M-EDIT-013 | Clip Markers ▸ | | | | Submenu, rows M-EDIT-013a to 013d |
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
| M-EDIT-023 | Apply Transformation/Generator | ⌘↩ | S-NOTE-033 | disabled | |
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
| M-EDIT-036 | Quantize Settings... | ⇧⌘U | S-NOTE-012, S-SAMPLE-002 | disabled | |
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
| M-EDIT-039 | Simplify Envelope | | | disabled | |
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
| M-CREATE-005 | Insert Scene | ⌘I | S-SES-008 | disabled | Enabled in the scene's context menu (C-SCENE-007) |
| M-CREATE-006 | Capture and Insert Scene | ⇧⌘I | S-SES-009 | | Inserts a new scene directly after the scene last launched, holding copies of the clips that are playing, in their tracks; playback moves to the copies, and the new scene's tempo and signature are set to the current ones (observed in 12.4.3 (macOS), 15 October 2026) |
| M-CREATE-007 | Consolidate Time to New Scene | | | disabled | |
| | — | | | | |
| M-CREATE-008 | Insert Empty MIDI Clip | ⇧⌘M | S-SES-007, S-ARR-037 | disabled | Enabled in the empty slot's and time selection's context menus. A new clip is one bar long in the time signature in force at the time (observed in 12.4.3 (macOS), 15 October 2026) |
| M-CREATE-009 | Capture MIDI | | | disabled | Same as the Control Bar's Capture MIDI button. Disabled until notes have been played, then enabled (observed in 12.4.3 (macOS), 15 October 2026). Runs `midi.capture` ([capture MIDI](../specs/capture-midi.md)) |
| | — | | | | |
| M-CREATE-010 | Create Fade | ⌥⌘F | S-ARR-008 | disabled | The manual names it Create Fade In/Out or Create Crossfade depending on the selection |
| | — | | | | |
| M-CREATE-011 | Slice to New MIDI Track | | | disabled | |
| M-CREATE-012 | Convert Harmony to New MIDI Track | | | disabled | |
| M-CREATE-013 | Convert Melody to New MIDI Track | | | disabled | |
| M-CREATE-014 | Convert Drums to New MIDI Track | | | disabled | |
| M-CREATE-015 | Separate Stems to New Audio Tracks | | | disabled | |
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
| M-PLAY-003 | Play From Insert Marker | ⌥Space | S-ARR-032 | | Shown with the Session View focused; S-TRANS-003 binds the same keys from the manual |
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
| M-VIEW-002 | Second Window | ⇧⌘W | S-VIEW-002 | | |
| M-VIEW-003 | Zoom ▸ | | | | Submenu, rows M-VIEW-003a to 003c (interface zoom) |
| M-VIEW-003a | Zoom In | | S-ZOOM-001 | | |
| M-VIEW-003b | Zoom Out | | S-ZOOM-002 | | |
| M-VIEW-003c | Reset Zoom to 100% | | | | |
| | — | | | | |
| M-VIEW-004 | Info | ? | S-VIEW-008 | ticked | Info View. Whether it is shown is an application setting, not part of the set: hiding it doesn't mark the set as changed, and a new set and a relaunch keep it hidden (observed in 12.4.3 (macOS), 14 October 2026) |
| M-VIEW-005 | Learn View | ⌥⌘7 | S-VIEW-018 | ticked | |
| M-VIEW-006 | Pack Overview | | | | Ableton pack service; Beatlayer's equivalent is its own content overview or nothing. Left out of Beatlayer's menu (D21) |
| M-VIEW-007 | File Manager | | | | |
| M-VIEW-008 | Undo History | ⌥⌘Z | S-VIEW-021 | | Opens in the right-hand column where the Learn View sits (L-OTH-001) |
| D24 | Assistant | | | | Beatlayer addition (D24): opens and closes the assistant pane (`view.toggle_assistant`); no key |
| | — | | | | |
| M-VIEW-009 | Toggle Arrangement/Session View | ⇥ | S-VIEW-003 | | |
| | — | | | | |
| M-VIEW-010 | Clip View | ⌥⌘3 | S-VIEW-015 | | |
| M-VIEW-011 | Device View | ⌥⌘4 | S-VIEW-016 | ticked | |
| M-VIEW-012 | Toggle Clip/Device View | ⇧⇥ | S-VIEW-004 | | |
| M-VIEW-013 | Expand Clip View | ⌥⌘E | S-NOTE-030 | disabled | |
| M-VIEW-014 | Hide Device View | ⌥⌘L | S-VIEW-022 | | Label read with the Device View shown; hides the detail area |
| | — | | | | |
| M-VIEW-015 | Arrange Clip View Panels Automatically | | | ticked | One of three choices (015 to 017) |
| M-VIEW-016 | Arrange Clip View Panels Horizontally | | | | |
| M-VIEW-017 | Arrange Clip View Panels Vertically | | | | |
| | — | | | | |
| M-VIEW-018 | Content Editor | ⌥⇧1 | S-CLIPMODE-003 | ticked | Sample Editor or MIDI Note Editor |
| M-VIEW-019 | Envelope Editor | ⌥⇧2 | S-CLIPMODE-004 | disabled | |
| M-VIEW-020 | Expression Editor | ⌥⇧3 | S-CLIPMODE-005 | disabled | MPE editor |
| | — | | | | |
| M-VIEW-021 | Browser | ⌥⌘5 | S-VIEW-010 | ticked | |
| M-VIEW-022 | Groove Pool | ⌥⌘6 | S-VIEW-017 | | |
| M-VIEW-023 | Tuning | | | | Tuning section of the browser column |
| M-VIEW-024 | Full-Height Browser | | | | |
| | — | | | | |
| M-VIEW-025 | Search in Browser | ⌘F | S-BRW-006 | | |
| M-VIEW-026 | Show Similar Files | ⇧⌘F | S-BRW-010 | disabled | |
| M-VIEW-027 | Show Filter View | ⌥⌘G | S-BRW-013 | disabled | |
| | — | | | | |
| M-VIEW-028 | Plug-In Windows | ⌥⌘P | S-DEV-009 | ticked, disabled | Beatlayer's floating native devices' windows (IMP-14) follow it too (WP-2.5b) |
| M-VIEW-029 | Video Window | ⌥⇧⌘V | S-VIEW-009 | ticked, disabled | Not the manual's ⌥⌘V, which Live shows for Paste Bounced Audio (M-EDIT-031) |
| | — | | | | |
| M-VIEW-030 | Show the Max for Live Window | ⌥⇧⌘M | S-VIEW-023 | | Max-specific; Beatlayer's extension host takes this place in M9 or drops it. Left out of Beatlayer's menu (D21) |
| | — | | | | |
| M-VIEW-031 | Mixer | ⌥⌘M | S-VIEW-014 | ticked | |
| M-VIEW-032 | Mixer Controls ▸ | | | | Submenu, rows M-VIEW-032a to 032g; observed in 12.4.6, 11 October 2026 |
| M-VIEW-032a | In/Out | ⌥⌘I | S-VIEW-012 | ticked | |
| M-VIEW-032b | Sends | ⌥⌘S | S-VIEW-013 | ticked | |
| M-VIEW-032c | Volume | | | ticked | |
| | — | | | | |
| M-VIEW-032d | Track Options | | | | |
| M-VIEW-032e | Crossfader | | | | |
| M-VIEW-032f | Performance Impact | | | | |
| | — | | | | |
| M-VIEW-032g | Return Tracks | ⌥⌘R | S-TRACK-011 | ticked | |
| M-VIEW-033 | Arrangement Track Controls ▸ | | | disabled | Submenu, rows M-VIEW-033a to 033d; observed in 12.4.6, 11 October 2026: disabled while the Session View is in front, no keys |
| M-VIEW-033a | In/Out | | | ticked | |
| M-VIEW-033b | Volume | | | ticked | |
| M-VIEW-033c | Track Options | | | | |
| | — | | | | |
| M-VIEW-033d | Return Tracks | | | ticked | |
| | — | | | | |
| M-VIEW-034 | Overview | ⌥⌘O | S-VIEW-011 | | |
| M-VIEW-035 | Scene Tempo and Time Signature | | | | Shows the Main track's scene tempo and signature fields |
| | — | | | | |
| M-VIEW-036 | Automation Mode | A | S-ENV-001 | disabled | |
| M-VIEW-037 | Show Grouped Tracks | ⌥U | S-ARR-021 | disabled | |
| M-VIEW-038 | Show Selected Grouped Track | U | S-TRACK-010 | disabled | |
| | — | | | | |
| M-VIEW-039 | Fold to Notes | F | S-NOTE-038 | disabled | |
| M-VIEW-040 | Fold to Scale | G | S-NOTE-039 | disabled | |
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
| M-NAV-008 | Learn View | ⌥7 | S-FOCUS-008 | | |
| M-NAV-009 | Tuning | | | | |
| | — | | | | |
| M-NAV-010 | Mixer | ⌥⇧M | S-ARR-035 | disabled | In the Arrangement, puts the keyboard focus in the mixer at the selected track; Right then moves the selection to the next track (observed in 12.4.6 (macOS), 11 October 2026) |
| M-NAV-011 | Clip Panels | ⌥⇧P | S-FOCUS-010 | disabled | |
| | — | | | | |
| M-NAV-012 | Previous | | S-FOCUS-012 | | No key shown: Shift+Tab moves focus only with Use Tab Key to Move Focus on |
| M-NAV-013 | Next | | S-FOCUS-011 | | No key shown: Tab moves focus only with Use Tab Key to Move Focus on |
| M-NAV-014 | Previous Neighbor | ⌥⇧⇥ | S-FOCUS-014 | disabled | Enabled whenever the track with the focus has a previous neighbour, whatever the Tab setting: with Use Tab Key to Move Focus off and track 1 selected, disabled (observed in 12.4.6, 11 October 2026) |
| M-NAV-015 | Next Neighbor | ⌥⇥ | S-FOCUS-013 | disabled | Enabled whenever there's a next track: with Use Tab Key to Move Focus off and track 1 selected, enabled; it moved the focus to the same control on the next track without changing the selected track (observed in 12.4.6, 11 October 2026) |
| | — | | | | |
| M-NAV-016 | Browser History Back | ⌘[ | S-BRW-011 | disabled | |
| M-NAV-017 | Browser History Forward | ⌘] | S-BRW-012 | disabled | |
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
| M-OPT-005 | MIDI Note Editor Preview | | | | |
| M-OPT-006 | Browser File Preview | | | | Same as the browser's preview switch |
| | — | | | | |
| M-OPT-007 | External Sync | | | disabled | |
| M-OPT-008 | Delay Compensation | | | | |
| M-OPT-009 | Reduced Latency When Monitoring | | | | |
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
| M-OPT-015 | Snap Automation to Grid | | | disabled | |
| M-OPT-016 | Fixed Grid | ⌘5 | S-GRID-006 | disabled | |
| | — | | | | |
| M-OPT-017 | Draw Mode | B | S-GRID-001 | | Reads "Draw Mode (Pitch Lock On/Off)" with the MIDI Note Editor focused (manual 10.4.1) |
| M-OPT-018 | Step Input Mode | ⇧⌘B | S-NOTE-044 | disabled | |
| | — | | | | |
| M-OPT-019 | Highlight Scales | K | S-NOTE-035 | | |
| M-OPT-020 | Chase MIDI Notes | | | | |
| M-OPT-021 | MIDI Envelope Auto-Reset | | | | |
| M-OPT-022 | Retune Set On Loading Tuning Systems | | | | |
| | — | | | | |
| M-OPT-023 | Solo Switches | | | disabled | One of two choices with M-OPT-024 |
| M-OPT-024 | Cue Switches | | | disabled | |
| M-OPT-025 | Solo in Place | | | | |
| | — | | | | |
| M-OPT-026 | Lock Envelopes | | | disabled | Same as the Arrangement's Lock Envelopes switch |
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
| M-HELP-001 | Learn View | | S-VIEW-018 | | No key shown here; the View menu shows ⌥⌘7 |
| M-HELP-002 | Pack Overview | | | | Ableton service; see M-VIEW-006. Left out of Beatlayer's menu (D21) |
| | — | | | | |
| M-HELP-003 | Load Demo Set | | | | Beatlayer: its own demo song |
| M-HELP-004 | Read the Live Manual... | | | | Beatlayer: Read the Beatlayer Manual... (D21); its own manual |
| M-HELP-005 | Learn Live at ableton.com... | | | | Ableton website; Beatlayer links its own site. Left out of Beatlayer's menu (D21) |
| | — | | | | |
| M-HELP-006 | Join the User Forum... | | | | Beatlayer: its own community link |
| M-HELP-007 | Get Support | | | | Beatlayer: its issue tracker |
| M-HELP-008 | User Account and Licenses... | | | | Ableton account; no Beatlayer equivalent (open source, no licence keys). Left out of Beatlayer's menu (D21) |
| M-HELP-009 | Purchase License... | | | | Ableton store; no Beatlayer equivalent. Left out of Beatlayer's menu (D21) |
| | — | | | | |
| M-HELP-010 | Check for Updates... | | | | Beatlayer's updater |

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

Each menu below was opened once in a new set on 10 October 2026 (the crossfader's on 15 October 2026), on the object named; its rows are in Live's order, with separators, keys shown, and state as in the menu-bar tables. **Other states** lists commands the manual places in the same menu for other objects or selections (audio clips, group tracks, automation lanes); their source is the manual.

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
| C-SLOT-008 | Paste (with something to paste) | manual 7.4 |
| C-SLOT-009 | Bounce Group to New Track (group slots) | manual 20.2 |

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
| C-TRACK-018 | Link Tracks, Unlink Track(s) (several tracks selected) | manual 6.14.1 |
| C-TRACK-019 | Show Take Lanes, Insert Take Lane, Delete All Take Lanes (Arrangement) | manual 21 |
| C-TRACK-020 | Crop Clip (from a time selection in the track) | manual 22.2 |

### Scene (C-SCENE)

Opened on a scene in the Main track's column.

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
| C-SCENE-015 | Paste (with something to paste) | manual 7.2 |
| C-SCENE-016 | Return to Default (on the scene tempo and signature fields) | manual 7.2.1 |

### Browser item (C-BROWSER)

Opened on a preset under the Sounds label.

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| C-BROWSER-001 | Show in Places | | | | |
| C-BROWSER-002 | Show in Finder | | | disabled | macOS |
| C-BROWSER-003 | Show Similar Files | ⇧⌘F | S-BRW-010 | | |
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
| C-BROWSER-013 | Separate Stems to New Audio Tracks (audio files) | manual 22.2 |
| C-BROWSER-014 | Manage Project (project folders) | manual 5.5.3 |
| C-BROWSER-015 | Set Default Live Set (template sets) | manual 5.4.5 |
| C-BROWSER-016 | Show Pack Info; Download, Pause, Resume, Cancel, Install (packs) | manual 4.7 |
| C-BROWSER-017 | Locate Folder, Remove from Sidebar (user folders) | manual 4.7.8 |
| C-BROWSER-018 | Rename label, choose label icon, remove custom label (sidebar labels) | manual 4.2.1 |

### Clip, in the Session View (C-CLIP-SES)

Opened on an empty MIDI clip on track 1.

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| C-CLIP-SES-001 | Cut | ⌘X | S-EDIT-001 | | |
| C-CLIP-SES-002 | Copy | ⌘C | S-EDIT-002 | | |
| C-CLIP-SES-003 | Duplicate | ⌘D | S-EDIT-004 | | |
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
| C-CLIP-SES-019 | Slice to New MIDI Track and the three Convert ... to New MIDI Track commands (audio clips) | manual 13 |
| C-CLIP-SES-020 | Separate Stems to New Audio Tracks (audio clips) | manual 22.2 |

### Clip, in the Arrangement (C-CLIP-ARR)

Opened on an empty MIDI clip on track 1, bars 1 to 5. No colour palette was recorded in this menu, unlike the Session clip's.

| ID | Item | Keys shown | Shortcut | In a new set | Notes |
|---|---|---|---|---|---|
| C-CLIP-ARR-001 | Cut | ⌘X | S-EDIT-001 | | |
| C-CLIP-ARR-002 | Copy | ⌘C | S-EDIT-002 | | |
| C-CLIP-ARR-003 | Duplicate | ⌘D | S-EDIT-004 | | |
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
| C-TIME-018 | Paste, Delete, and the ... Time commands | manual 6.9, 6.11 |
| C-TIME-019 | Split, Consolidate, Crop Clip (a selection that holds clips) | manual 6.12, 6.13, 22.2 |
| C-TIME-020 | Consolidate Time to New Scene | manual 7.5 |
| C-TIME-021 | Insert automation shapes, Simplify Envelope (automation lanes) | manual 26.6.3, 26.7.3 |
| C-TIME-022 | Add Locator, Insert Time Signature Change (scrub area) | manual 6.4, 6.5 |
| C-TIME-023 | Loop to Next Locator, Set Song Start Time Here (locator or loop brace) | manual 6.4, 6.6 |
| C-TIME-024 | Delete Fragmentary Bar Time, Complete Fragmentary Bar (fragmentary bars) | manual 6.5 |

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
| C-DEVICE-014 | Lock to (a control surface) | manual 34 |
| C-DEVICE-015 | Device-specific options (for example Mono Sidechain on the filter device, MPE settings on MPE plug-ins; the Drum Sampler's Enable Per-Note Pitch Bend, Envelope Follows Pitch, a swap to Simpler, and Save as Default Pad) | manual 23.2.1, 12.6, 31.4.5 |

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
